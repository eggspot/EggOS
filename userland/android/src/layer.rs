//! The layer's run on x86_64, aarch64 and riscv64 (D472, D760): the app's files read, its methods
//! compiled into one block of memory made executable, the entry table filled (the runtime's
//! helpers, then one entry per method of the app's DEX file: compiled code, a framework native, or
//! the missing-method stop), the launcher activity created and its `<init>` and `onCreate` called
//! through the table, and the text view it set shown in a window. Objects are boxes the layer's
//! collector frees once unreachable (D759); the app runs on this one thread. Virtual calls are
//! dispatched on the receiver's class, each class's static initializer runs on its first static
//! field or new object, arrays are objects with elements, and an uncaught `throw` stops the app
//! with the exception's class (D499). `setContentView(R.layout.…)` inflates the compiled layout the
//! resource table names into framework views, and the window shows them as labels and buttons whose
//! clicks call the activity's `android:onClick` method or the view's click listener, then shows the
//! texts they changed (D500).

use crate::collector;
use crate::list_rows;
use crate::framework::{
    self, ACTIVITY_CLASSES, BUTTON, EDIT_TEXT, IMAGE_VIEW, LINEAR_LAYOUT, LIST_VIEW, Native,
    Object, RECYCLER_VIEW, STRING_CLASS, TEXT_VIEWS, Target, VIEW_HOLDER, View,
};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};
use eggos_android_package::dex::Dex;
use eggos_android_package::constraint;
use eggos_android_package::layout::{self, Text, ViewNode};
use eggos_android_package::manifest::Manifest;
use eggos_android_package::resources::ResourceTable;
use eggos_dex_compiler::{self as compiler, helper, method_entry};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application::{self, Application};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::key;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};
use eggos_toolkit::window::Window;

type Entry = extern "C" fn(*const u64, *const usize) -> u64;

const AT_CURRENT_FOLDER: usize = -100isize as usize;
const READ_ONLY: usize = 0;
const PROTECT_READ: usize = 1;
const PROTECT_WRITE: usize = 2;
const PROTECT_EXECUTE: usize = 4;
const MAP_PRIVATE: usize = 2;
const MAP_ANONYMOUS: usize = 0x20;
const PAGE: usize = 4096;
const READ_CHUNK: usize = 64 << 10;
const LARGEST_FILE: usize = 64 << 20;
const MANIFEST_NAME: &[u8] = b"AndroidManifest.xml";
const RESOURCES_NAME: &[u8] = b"resources.arsc";
const BUNDLE_RESOURCES_NAME: &[u8] = b"resources.pb";
const MOST_WIDGETS: usize = 64;
const ROW: i32 = 44;
const COLUMN: i32 = 160;
const DEEPEST_VIEW: usize = 32;
const WINDOW_SIZE: (u32, u32) = (360, 640);
const PADDING: i32 = 16;
const STACK_ABOVE_RUN: usize = 4096;
const MOST_FRAMES: usize = 64;
const IME_ACTION_MASK: u32 = 0xff;
const BITMAP_IMAGE: u32 = u32::MAX;
const CENTRED: i32 = 500;
const DIVIDER: u32 = 1;
const MOST_TYPED_ROWS: usize = 10_000;
const SPAN_FIELD: u32 = u32::MAX - 2;
const GLYPH_WIDTH: i32 = 8;
const PLATFORM_VERSION: u16 = 34;
const MOST_CAUSES: usize = 8;
#[cfg(target_arch = "x86_64")]
const MACHINE: compiler::Machine = compiler::Machine::X86_64;
#[cfg(target_arch = "aarch64")]
const MACHINE: compiler::Machine = compiler::Machine::Aarch64;
#[cfg(target_arch = "riscv64")]
const MACHINE: compiler::Machine = compiler::Machine::Riscv64;

struct State {
    dex: Dex<'static>,
    content: Option<u64>,
    base: usize,
    compiled: BTreeMap<u32, usize>,
    statics: BTreeMap<u32, u64>,
    initialized: BTreeSet<String>,
    dispatch: BTreeMap<(String, u32), usize>,
    resources: Option<ResourceTable>,
    images: Vec<(usize, Vec<u32>)>,
    folder: Vec<u8>,
    activity: u64,
    table: usize,
    heap: collector::Heap,
    stack_top: usize,
    code_length: usize,
    failed_classes: BTreeSet<String>,
    depth: usize,
    placed_images: Vec<(u32, i32, i32)>,
    scaled_images: BTreeMap<(u32, i32, i32), u32>,
    below: Vec<Shown>,
    fragments: Vec<u64>,
    generation: u64,
    request: Option<(u32, u64)>,
    result: (u32, u64),
    lists: BTreeMap<u64, ListRows>,
}

/// An activity under the one shown (D765): it, its content, and what it asked the one
/// above it for (`startActivityForResult`'s request code).
#[derive(Debug, Clone, Copy)]
struct Shown {
    activity: u64,
    content: Option<u64>,
    request: Option<(u32, u64)>,
    result: (u32, u64),
}

static STATE: AtomicPtr<State> = AtomicPtr::new(core::ptr::null_mut());

fn state() -> &'static mut State {
    let state = STATE.load(Ordering::Acquire);
    // SAFETY: `run` stores a leaked `State` before any compiled code or helper runs and never
    // frees it; the app runs on one thread, so no other reference is alive while this one is.
    unsafe { &mut *state }
}

fn say(words: &[&[u8]]) {
    for part in words {
        syscall::write_all(2, part);
    }
    syscall::write_all(2, b"\n");
}

fn stop(words: &[&[u8]]) -> ! {
    say(words);
    syscall::exit(1)
}

fn read_file(path: &[u8]) -> Option<Vec<u8>> {
    let mut named = Vec::from(path);
    named.push(0);
    // SAFETY: `openat` reads the NUL-terminated path in `named`, which lives across the call.
    let descriptor = unsafe {
        syscall::call(
            number::OPENAT,
            [
                AT_CURRENT_FOLDER,
                named.as_ptr() as usize,
                READ_ONLY,
                0,
                0,
                0,
            ],
        )
    };
    let descriptor = usize::try_from(descriptor).ok()?;
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK];
    let read = loop {
        // SAFETY: `read` writes at most `chunk.len()` bytes into `chunk`, which it owns.
        let count = unsafe {
            syscall::call(
                number::READ,
                [
                    descriptor,
                    chunk.as_mut_ptr() as usize,
                    chunk.len(),
                    0,
                    0,
                    0,
                ],
            )
        };
        match usize::try_from(count) {
            Ok(0) => break true,
            Ok(count) if bytes.len() + count <= LARGEST_FILE => {
                bytes.extend_from_slice(&chunk[..count])
            }
            _ => break false,
        }
    };
    // SAFETY: closes the descriptor `openat` gave, used by nothing after.
    unsafe { syscall::call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]) };
    read.then_some(bytes)
}

fn executable(code: &[u8]) -> Option<usize> {
    let length = code.len().max(1).next_multiple_of(PAGE);
    // SAFETY: an anonymous private mapping the kernel places, owned by this process.
    let base = unsafe {
        syscall::call(
            number::MMAP,
            [
                0,
                length,
                PROTECT_READ | PROTECT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                usize::MAX,
                0,
            ],
        )
    };
    let base = usize::try_from(base).ok()?;
    // SAFETY: the mapping is `length` bytes, at least `code.len()`, writable, and only this
    // function uses it until it is made executable below.
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), base as *mut u8, code.len()) };
    synchronize_instructions(base, code.len());
    // SAFETY: changes the protection of the mapping made above, which nothing writes after.
    let protected = unsafe {
        syscall::call(
            number::MPROTECT,
            [base, length, PROTECT_READ | PROTECT_EXECUTE, 0, 0, 0],
        )
    };
    (protected == 0).then_some(base)
}

#[cfg(target_arch = "x86_64")]
fn synchronize_instructions(_: usize, _: usize) {}

#[cfg(target_arch = "aarch64")]
fn synchronize_instructions(base: usize, length: usize) {
    const LINE: usize = 64;
    for line in (base & !(LINE - 1)..base + length).step_by(LINE) {
        // SAFETY: cleans the data cache line holding written code to the point of unification.
        unsafe { core::arch::asm!("dc cvau, {}", in(reg) line, options(nostack)) };
    }
    // SAFETY: waits for the cleaning above.
    unsafe { core::arch::asm!("dsb ish", options(nostack)) };
    for line in (base & !(LINE - 1)..base + length).step_by(LINE) {
        // SAFETY: drops stale instructions for the line from the instruction cache.
        unsafe { core::arch::asm!("ic ivau, {}", in(reg) line, options(nostack)) };
    }
    // SAFETY: waits for the invalidation and refetches the following instructions.
    unsafe { core::arch::asm!("dsb ish", "isb", options(nostack)) };
}

#[cfg(target_arch = "riscv64")]
fn synchronize_instructions(_: usize, _: usize) {
    // SAFETY: orders the stores of the code before this thread fetches it.
    unsafe { core::arch::asm!("fence.i", options(nostack)) };
}

fn new_object(object: Object) -> u64 {
    let address = Box::leak(Box::new(object)) as *mut Object as u64;
    state().heap.note(address);
    address
}

#[cfg(target_arch = "x86_64")]
fn callee_saved_registers() -> [u64; 12] {
    let mut saved = [0u64; 12];
    // SAFETY: copies the callee-saved registers into a local array; nothing else changes.
    unsafe {
        core::arch::asm!(
            "mov [{0}], rbx",
            "mov [{0} + 8], rbp",
            "mov [{0} + 16], r12",
            "mov [{0} + 24], r13",
            "mov [{0} + 32], r14",
            "mov [{0} + 40], r15",
            in(reg) saved.as_mut_ptr(),
            options(nostack, preserves_flags),
        )
    };
    saved
}

#[cfg(target_arch = "aarch64")]
fn callee_saved_registers() -> [u64; 12] {
    let mut saved = [0u64; 12];
    // SAFETY: copies the callee-saved registers into a local array; nothing else changes.
    unsafe {
        core::arch::asm!(
            "stp x19, x20, [{0}]",
            "stp x21, x22, [{0}, #16]",
            "stp x23, x24, [{0}, #32]",
            "stp x25, x26, [{0}, #48]",
            "stp x27, x28, [{0}, #64]",
            "stp x29, x30, [{0}, #80]",
            in(reg) saved.as_mut_ptr(),
            options(nostack, preserves_flags),
        )
    };
    saved
}

#[cfg(target_arch = "riscv64")]
fn callee_saved_registers() -> [u64; 12] {
    let mut saved = [0u64; 12];
    // SAFETY: copies the callee-saved registers into a local array; nothing else changes.
    unsafe {
        core::arch::asm!(
            "sd s0, 0({0})",
            "sd s1, 8({0})",
            "sd s2, 16({0})",
            "sd s3, 24({0})",
            "sd s4, 32({0})",
            "sd s5, 40({0})",
            "sd s6, 48({0})",
            "sd s7, 56({0})",
            "sd s8, 64({0})",
            "sd s9, 72({0})",
            "sd s10, 80({0})",
            "sd s11, 88({0})",
            in(reg) saved.as_mut_ptr(),
            options(nostack),
        )
    };
    saved
}

fn stack_pointer() -> usize {
    let marker = 0u8;
    core::hint::black_box(&marker) as *const u8 as usize
}

fn references_of(address: u64, found: &mut Vec<u64>) {
    let Some(held) = object(address) else {
        return;
    };
    found.extend(held.fields.values().copied());
    found.extend(held.elements.iter().flatten().copied());
    if let Some(view) = &held.view {
        found.extend(view.children.iter().copied());
        found.extend(view.watchers.iter().copied());
        found.push(view.listener);
        found.push(view.editor_listener);
    }
    found.push(held.reference);
    found.extend(held.extras.iter().map(|(_, value)| *value));
}

/// Collects when one is due, at an allocation compiled code asks for: its frames and
/// registers hold its references, and the layer's own code holds none outside the stack.
fn at_safepoint() {
    if state().heap.due() {
        collect();
    }
}

/// Frees every object no root reaches (D759): the layer's own references, then every word of
/// the stack between here and where `run` started and the callee-saved registers, read
/// conservatively.
fn collect() {
    let registers = callee_saved_registers();
    let low = stack_pointer() & !7;
    let high = state().stack_top;
    let stack = (low..high).step_by(8).map(|word| {
        // SAFETY: every word up to the top recorded in `run` lies in this thread's stack.
        unsafe { core::ptr::read_volatile(word as *const u64) }
    });
    let current = state();
    let exception = match current.table {
        0 => 0,
        _ => pending(table()),
    };
    let own = [
        current.content.unwrap_or(0),
        current.activity,
        exception,
        current.result.1,
        current.request.map_or(0, |(_, caller)| caller),
    ];
    let below = current.below.iter().flat_map(|shown| {
        [
            shown.activity,
            shown.content.unwrap_or(0),
            shown.result.1,
            shown.request.map_or(0, |(_, caller)| caller),
        ]
    });
    let roots = registers
        .into_iter()
        .chain(stack)
        .chain(current.statics.values().copied())
        .chain(own)
        .chain(below)
        .chain(current.fragments.iter().copied())
        .chain(current.lists.keys().copied())
        .chain(current.lists.values().flat_map(|rows| {
            rows.made
                .values()
                .map(|&(_, holder)| holder)
                .chain(rows.pools.views())
        }));
    let marked = collector::trace(roots, &current.heap.objects, references_of);
    for address in collector::unmarked(&current.heap.objects, &marked) {
        current.heap.objects.remove(&address);
        // SAFETY: a leaked box from `new_object` that nothing reachable holds, dropped once.
        drop(unsafe { Box::from_raw(address as *mut Object) });
    }
    current.heap.collected();
}

fn object(address: u64) -> Option<&'static mut Object> {
    // SAFETY: every object address the app holds came from `new_object`, whose box is never
    // freed; 0 is `null`. The app's one thread holds no other reference across this call.
    unsafe { (address as *mut Object).as_mut() }
}

fn arguments<const N: usize>(words: *const u64) -> [u64; N] {
    // SAFETY: compiled code and the layer pass at least as many argument words as the callee
    // takes (`HELPER_WORDS` for helpers, the method's argument count for natives).
    unsafe { core::ptr::read_unaligned(words.cast::<[u64; N]>()) }
}

extern "C" fn string(words: *const u64, _: *const usize) -> u64 {
    at_safepoint();
    let [index] = arguments::<1>(words);
    let text = state().dex.string(index as u32);
    new_object(Object {
        class: String::from(STRING_CLASS),
        text,
        ..Object::default()
    })
}

extern "C" fn new_instance(words: *const u64, table: *const usize) -> u64 {
    at_safepoint();
    let [index] = arguments::<1>(words);
    let class = state().dex.type_name(index as u32).unwrap_or_default();
    initialize(&class, table);
    new_object(Object {
        class,
        ..Object::default()
    })
}

fn pending_slot(table: *const usize) -> *mut usize {
    table.cast_mut().wrapping_add(helper::PENDING_EXCEPTION)
}

fn pending(table: *const usize) -> u64 {
    // SAFETY: `table` is the layer's entry table, made from a mutable leaked block, whose
    // pending-exception word only the app's one thread reads and writes.
    unsafe { pending_slot(table).read() as u64 }
}

fn set_pending(table: *const usize, exception: u64) {
    // SAFETY: as in `pending`.
    unsafe { pending_slot(table).write(exception as usize) };
}

fn raise(table: *const usize, class: &str) -> u64 {
    raise_with(table, class, None)
}

fn raise_with(table: *const usize, class: &str, message: Option<String>) -> u64 {
    let exception = new_object(Object {
        class: String::from(class),
        text: message,
        trace: capture_trace(),
        ..Object::default()
    });
    set_pending(table, exception);
    0
}

/// The methods whose compiled code the stack holds return addresses into, innermost first
/// (D761): each word between here and where `run` started that points inside the compiled
/// code names the method around it.
fn capture_trace() -> Vec<u32> {
    let current = state();
    let mut starts: Vec<(usize, u32)> = current
        .compiled
        .iter()
        .map(|(&method, &offset)| (offset, method))
        .collect();
    starts.sort_unstable();
    let (base, end) = (current.base, current.base + current.code_length);
    let low = stack_pointer() & !7;
    let mut trace = Vec::new();
    for word in (low..current.stack_top).step_by(8) {
        // SAFETY: every word up to the top recorded in `run` lies in this thread's stack.
        let value = unsafe { core::ptr::read_volatile(word as *const u64) } as usize;
        if value <= base || value >= end {
            continue;
        }
        let at = starts.partition_point(|&(start, _)| start < value - base);
        let Some(&(_, method)) = at.checked_sub(1).and_then(|index| starts.get(index)) else {
            continue;
        };
        if trace.last() != Some(&method) {
            trace.push(method);
        }
        if trace.len() == MOST_FRAMES {
            break;
        }
    }
    trace
}

fn frame_names(method: u32) -> (String, String) {
    let dex = &state().dex;
    let class = dex.method_class(method).unwrap_or_default();
    let name = dex.method_name(method).unwrap_or_default();
    (framework::class_name(&class), name)
}

/// What `printStackTrace` writes for `exception`: its class and message, a line per frame,
/// and its causes after `Caused by: `.
fn trace_text(exception: u64) -> Vec<u8> {
    let mut text = Vec::new();
    let mut current = exception;
    for depth in 0..MOST_CAUSES {
        let Some(thrown) = object(current) else {
            break;
        };
        if depth > 0 {
            text.extend_from_slice(b"Caused by: ");
        }
        text.extend_from_slice(framework::class_name(&thrown.class).as_bytes());
        if let Some(message) = &thrown.text {
            text.extend_from_slice(b": ");
            text.extend_from_slice(message.as_bytes());
        }
        text.push(b'\n');
        for &method in &thrown.trace {
            let (class, name) = frame_names(method);
            text.extend_from_slice(format!("\tat {class}.{name}(Unknown Source)\n").as_bytes());
        }
        current = thrown.reference;
    }
    text
}

fn report_if_uncaught() {
    let thrown = pending(table());
    if object(thrown).is_none() {
        return;
    }
    let mut text = Vec::from(&b"android: FATAL EXCEPTION: main\n"[..]);
    text.extend(trace_text(thrown));
    syscall::write_all(2, &text);
    syscall::exit(1)
}

fn failed() -> bool {
    pending(table()) != 0
}

extern "C" fn field_get(words: *const u64, table: *const usize) -> u64 {
    let [target, field] = arguments::<2>(words);
    let Some(target) = object(target) else {
        return raise(table, framework::NULL_POINTER);
    };
    target.fields.get(&(field as u32)).copied().unwrap_or(0)
}

extern "C" fn field_put(words: *const u64, table: *const usize) -> u64 {
    let [target, field, value] = arguments::<3>(words);
    let Some(target) = object(target) else {
        return raise(table, framework::NULL_POINTER);
    };
    target.fields.insert(field as u32, value);
    0
}

extern "C" fn divide_int(words: *const u64, table: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    match right as u32 as i32 {
        0 => raise(table, framework::ARITHMETIC),
        divisor => (left as u32 as i32).wrapping_div(divisor) as u32 as u64,
    }
}

extern "C" fn remainder_int(words: *const u64, table: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    match right as u32 as i32 {
        0 => raise(table, framework::ARITHMETIC),
        divisor => (left as u32 as i32).wrapping_rem(divisor) as u32 as u64,
    }
}

extern "C" fn divide_long(words: *const u64, table: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    match right as i64 {
        0 => raise(table, framework::ARITHMETIC),
        divisor => (left as i64).wrapping_div(divisor) as u64,
    }
}

extern "C" fn remainder_long(words: *const u64, table: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    match right as i64 {
        0 => raise(table, framework::ARITHMETIC),
        divisor => (left as i64).wrapping_rem(divisor) as u64,
    }
}

fn entry_at(address: usize) -> Entry {
    // SAFETY: a compiled method or one of the layer's helpers, all of type `Entry`.
    unsafe { core::mem::transmute::<usize, Entry>(address) }
}

fn target_address(target: Target) -> usize {
    match target {
        Target::Compiled(offset) => state().base + offset,
        Target::Native(native) => native_entry(native) as usize,
        Target::Missing => missing_method as usize,
    }
}

fn initialize(class: &str, table: *const usize) {
    if framework::is_replaced(class) {
        return;
    }
    if state().failed_classes.contains(class) {
        let name = framework::class_name(class);
        let message = format!("Could not initialize class {name}");
        raise_with(table, framework::NO_CLASS_DEFINITION, Some(message));
        return;
    }
    let address = {
        let state = state();
        if !state.initialized.insert(String::from(class)) {
            return;
        }
        let Some(index) = state.dex.class_index(class) else {
            return;
        };
        let initializer = method_named(&state.dex, index, "<clinit>");
        match initializer.and_then(|method| state.compiled.get(&method)) {
            Some(&offset) => state.base + offset,
            None => return,
        }
    };
    let no_arguments = [0u64; 1];
    entry_at(address)(no_arguments.as_ptr(), table);
    let thrown = pending(table);
    if thrown == 0 {
        return;
    }
    state().failed_classes.insert(String::from(class));
    let is_error = object(thrown)
        .is_some_and(|exception| framework::is_error(&state().dex, &exception.class));
    if !is_error {
        let wrapper = new_object(Object {
            class: String::from(framework::EXCEPTION_IN_INITIALIZER),
            reference: thrown,
            trace: capture_trace(),
            ..Object::default()
        });
        set_pending(table, wrapper);
    }
}

extern "C" fn invoke_virtual(words: *const u64, table: *const usize) -> u64 {
    let [method, receiver] = arguments::<2>(words);
    let Some(class) = object(receiver).map(|receiver| receiver.class.clone()) else {
        return raise(table, framework::NULL_POINTER);
    };
    let key = (class, method as u32);
    let cached = state().dispatch.get(&key).copied();
    let address = match cached {
        Some(address) => address,
        None => {
            let target = {
                let state = state();
                framework::resolve_virtual(&state.dex, &key.0, method as u32, &state.compiled)
            };
            let address = target_address(target);
            state().dispatch.insert(key, address);
            address
        }
    };
    entry_at(address)(words.wrapping_add(1), table)
}

fn static_field_class(field: u64, table: *const usize) {
    if let Some(class) = state().dex.field_class(field as u32) {
        initialize(&class, table);
    }
}

extern "C" fn static_get(words: *const u64, table: *const usize) -> u64 {
    let [field] = arguments::<1>(words);
    static_field_class(field, table);
    if let Some(&value) = state().statics.get(&(field as u32)) {
        return value;
    }
    let class = state().dex.field_class(field as u32).unwrap_or_default();
    if class != framework::SCALE_TYPE {
        return 0;
    }
    let constant = new_object(Object {
        class,
        text: state().dex.field_name(field as u32),
        ..Object::default()
    });
    state().statics.insert(field as u32, constant);
    constant
}

extern "C" fn static_put(words: *const u64, table: *const usize) -> u64 {
    let [field, value] = arguments::<2>(words);
    static_field_class(field, table);
    state().statics.insert(field as u32, value);
    0
}

const LARGEST_ARRAY: usize = 1 << 24;

extern "C" fn new_array(words: *const u64, table: *const usize) -> u64 {
    at_safepoint();
    let [length, index] = arguments::<2>(words);
    let length = length as u32 as i32;
    if length < 0 {
        return raise(table, framework::NEGATIVE_ARRAY_SIZE);
    }
    if length as usize > LARGEST_ARRAY {
        return raise(table, framework::OUT_OF_MEMORY);
    }
    let class = state().dex.type_name(index as u32).unwrap_or_default();
    new_object(Object {
        class,
        elements: Some(vec![0; length as usize]),
        ..Object::default()
    })
}

fn elements_of(array: u64, table: *const usize) -> Option<&'static mut Object> {
    match object(array) {
        Some(array) if array.elements.is_some() => Some(array),
        Some(_) => {
            raise(table, framework::CLASS_CAST);
            None
        }
        None => {
            raise(table, framework::NULL_POINTER);
            None
        }
    }
}

fn element_index(array: &Object, index: u64, table: *const usize) -> Option<usize> {
    let length = array.elements.as_ref().map_or(0, Vec::len);
    match usize::try_from(index as u32 as i32) {
        Ok(index) if index < length => Some(index),
        _ => {
            raise(table, framework::ARRAY_INDEX);
            None
        }
    }
}

extern "C" fn array_length(words: *const u64, table: *const usize) -> u64 {
    let [array] = arguments::<1>(words);
    elements_of(array, table)
        .and_then(|array| array.elements.as_ref())
        .map_or(0, |elements| elements.len() as u64)
}

extern "C" fn array_get(words: *const u64, table: *const usize) -> u64 {
    let [array, index] = arguments::<2>(words);
    let Some(array) = elements_of(array, table) else {
        return 0;
    };
    let Some(index) = element_index(array, index, table) else {
        return 0;
    };
    array
        .elements
        .as_ref()
        .map_or(0, |elements| elements[index])
}

extern "C" fn array_put(words: *const u64, table: *const usize) -> u64 {
    let [array, index, value] = arguments::<3>(words);
    let Some(array) = elements_of(array, table) else {
        return 0;
    };
    let Some(index) = element_index(array, index, table) else {
        return 0;
    };
    let value = framework::array_element(&array.class, value);
    if let Some(elements) = array.elements.as_mut() {
        elements[index] = value;
    }
    0
}

extern "C" fn throw(words: *const u64, table: *const usize) -> u64 {
    let [exception] = arguments::<1>(words);
    match object(exception) {
        Some(_) => set_pending(table, exception),
        None => {
            raise(table, framework::NULL_POINTER);
        }
    }
    0
}

extern "C" fn catches(words: *const u64, table: *const usize) -> u64 {
    let [index] = arguments::<1>(words);
    let Some(exception) = object(pending(table)) else {
        return 0;
    };
    let wanted = class_of(index);
    u64::from(framework::is_instance(&state().dex, &exception.class, &wanted) == Some(true))
}

extern "C" fn take_exception(_: *const u64, table: *const usize) -> u64 {
    let exception = pending(table);
    set_pending(table, 0);
    exception
}

extern "C" fn exception_init(words: *const u64, _: *const usize) -> u64 {
    let [this, message] = arguments::<2>(words);
    let text = object(message).and_then(|message| message.text.clone());
    let trace = capture_trace();
    if let Some(this) = object(this) {
        if this.text.is_none() {
            this.text = text;
        }
        this.trace = trace;
    }
    0
}

extern "C" fn exception_init_empty(words: *const u64, _: *const usize) -> u64 {
    let [this] = arguments::<1>(words);
    let trace = capture_trace();
    if let Some(this) = object(this) {
        this.trace = trace;
    }
    0
}

extern "C" fn fill_in_stack_trace(words: *const u64, table: *const usize) -> u64 {
    exception_init_empty(words, table);
    let [this] = arguments::<1>(words);
    this
}

extern "C" fn print_stack_trace(words: *const u64, _: *const usize) -> u64 {
    let [this] = arguments::<1>(words);
    syscall::write_all(2, &trace_text(this));
    0
}

extern "C" fn get_cause(words: *const u64, _: *const usize) -> u64 {
    let [this] = arguments::<1>(words);
    object(this).map_or(0, |this| this.reference)
}

extern "C" fn get_stack_trace(words: *const u64, _: *const usize) -> u64 {
    let [this] = arguments::<1>(words);
    let trace = object(this).map(|this| this.trace.clone()).unwrap_or_default();
    let mut frames = Vec::with_capacity(trace.len());
    for method in trace {
        let (class, name) = frame_names(method);
        frames.push(new_object(Object {
            class: String::from(framework::STACK_TRACE_ELEMENT),
            text: Some(format!("{class}\n{name}")),
            ..Object::default()
        }));
    }
    new_object(Object {
        class: format!("[{}", framework::STACK_TRACE_ELEMENT),
        elements: Some(frames),
        ..Object::default()
    })
}

fn frame_part(words: *const u64, part: usize) -> u64 {
    let [this] = arguments::<1>(words);
    let text = object(this).and_then(|this| this.text.clone()).unwrap_or_default();
    let mut parts = text.split('\n');
    let class = parts.next().unwrap_or_default();
    let method = parts.next().unwrap_or_default();
    let chosen = match part {
        0 => String::from(class),
        1 => String::from(method),
        _ => format!("{class}.{method}(Unknown Source)"),
    };
    new_string(chosen)
}

extern "C" fn frame_class_name(words: *const u64, _: *const usize) -> u64 {
    frame_part(words, 0)
}

extern "C" fn frame_method_name(words: *const u64, _: *const usize) -> u64 {
    frame_part(words, 1)
}

extern "C" fn frame_text(words: *const u64, _: *const usize) -> u64 {
    frame_part(words, 2)
}

extern "C" fn get_message(words: *const u64, _: *const usize) -> u64 {
    let [this] = arguments::<1>(words);
    match object(this).and_then(|this| this.text.clone()) {
        Some(text) => new_string(text),
        None => 0,
    }
}

fn class_of(index: u64) -> String {
    state().dex.type_name(index as u32).unwrap_or_default()
}

extern "C" fn check_cast(words: *const u64, table: *const usize) -> u64 {
    let [value, index] = arguments::<2>(words);
    if let Some(value) = object(value) {
        let wanted = class_of(index);
        if framework::is_instance(&state().dex, &value.class, &wanted) == Some(false) {
            return raise(table, framework::CLASS_CAST);
        }
    }
    0
}

extern "C" fn instance_of(words: *const u64, _: *const usize) -> u64 {
    let [value, index] = arguments::<2>(words);
    let Some(value) = object(value) else {
        return 0;
    };
    let wanted = class_of(index);
    u64::from(framework::is_instance(&state().dex, &value.class, &wanted) == Some(true))
}

fn float_of(word: u64) -> f32 {
    f32::from_bits(word as u32)
}

fn double_of(word: u64) -> f64 {
    f64::from_bits(word)
}

extern "C" fn remainder_float(words: *const u64, _: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    let remainder = eggos_math::fmod(f64::from(float_of(left)), f64::from(float_of(right)));
    u64::from((remainder as f32).to_bits())
}

extern "C" fn remainder_double(words: *const u64, _: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    eggos_math::fmod(double_of(left), double_of(right)).to_bits()
}

extern "C" fn float_to_int(words: *const u64, _: *const usize) -> u64 {
    let [value] = arguments::<1>(words);
    u64::from(float_of(value) as i32 as u32)
}

extern "C" fn float_to_long(words: *const u64, _: *const usize) -> u64 {
    let [value] = arguments::<1>(words);
    float_of(value) as i64 as u64
}

extern "C" fn double_to_int(words: *const u64, _: *const usize) -> u64 {
    let [value] = arguments::<1>(words);
    u64::from(double_of(value) as i32 as u32)
}

extern "C" fn double_to_long(words: *const u64, _: *const usize) -> u64 {
    let [value] = arguments::<1>(words);
    double_of(value) as i64 as u64
}

extern "C" fn missing_method(_: *const u64, _: *const usize) -> u64 {
    stop(&[b"android: the app called a method the Android layer does not provide yet"])
}

extern "C" fn nothing(_: *const u64, _: *const usize) -> u64 {
    0
}

extern "C" fn set_content_view(words: *const u64, _: *const usize) -> u64 {
    let [_, view] = arguments::<2>(words);
    state().content = Some(view);
    state().generation += 1;
    0
}

extern "C" fn text_view_init(words: *const u64, _: *const usize) -> u64 {
    let [view, _] = arguments::<2>(words);
    if let Some(view) = object(view) {
        view.text = Some(String::new());
        view.view = Some(View::default());
    }
    0
}

extern "C" fn set_text(words: *const u64, _: *const usize) -> u64 {
    let [view, text] = arguments::<2>(words);
    let text = object(text)
        .and_then(|text| text.text.clone())
        .unwrap_or_default();
    if let Some(view) = object(view) {
        view.text = Some(text);
    }
    0
}

fn resource_string(id: u64) -> Option<String> {
    let state = state();
    state
        .resources
        .as_ref()?
        .string(id as u32)
        .map(String::from)
}

fn new_string(text: String) -> u64 {
    new_object(Object {
        class: String::from(STRING_CLASS),
        text: Some(text),
        ..Object::default()
    })
}

fn text_of(text: &Option<Text>) -> Option<String> {
    match text.as_ref()? {
        Text::Literal(text) => Some(text.clone()),
        Text::Resource(id) => resource_string(u64::from(*id)),
    }
}

fn inflate_node(node: &ViewNode, depth: usize) -> u64 {
    let class = framework::element_class(&node.element);
    let shows_text = TEXT_VIEWS.contains(&class.as_str());
    let children = match depth < DEEPEST_VIEW {
        true => node
            .children
            .iter()
            .map(|child| inflate_node(child, depth + 1))
            .collect(),
        false => Vec::new(),
    };
    let hint = text_of(&node.hint);
    let text = match class.as_str() {
        EDIT_TEXT => text_of(&node.text),
        _ => text_of(&node.text).or_else(|| shows_text.then(|| hint.clone()).flatten()),
    };
    new_object(Object {
        class,
        text: text.or_else(|| shows_text.then(String::new)),
        view: Some(View {
            id: node.id.unwrap_or(0),
            children,
            vertical: node.vertical || node.element != "LinearLayout",
            on_click: node.on_click.clone(),
            listener: 0,
            hint,
            image: node.image.and_then(load_image),
            input_type: node.input_type.unwrap_or(0),
            ime_action: node.ime_action.unwrap_or(0),
            width: pixels_of(node.layout_width),
            height: pixels_of(node.layout_height),
            scale_type: node.scale_type,
            constraints: constraints_of(node),
            ..View::default()
        }),
        ..Object::default()
    })
}

fn inflate(layout_id: u64) -> u64 {
    let Some(path) = resource_string(layout_id) else {
        stop(&[b"android: no layout resource for setContentView"]);
    };
    let file = [state().folder.as_slice(), b"/", path.as_bytes()].concat();
    let Some(node) = read_file(&file).and_then(|bytes| layout::parse(&bytes)) else {
        stop(&[b"android: cannot read the layout ", path.as_bytes()]);
    };
    inflate_node(&node, 0)
}

extern "C" fn set_content_layout(words: *const u64, _: *const usize) -> u64 {
    let [_, layout_id] = arguments::<2>(words);
    let root = inflate(u64::from(layout_id as u32));
    state().content = Some(root);
    state().generation += 1;
    0
}

fn find_view(view: u64, id: u32, depth: usize) -> Option<u64> {
    let found = object(view)?.view.as_ref()?;
    if found.id == id {
        return Some(view);
    }
    if depth >= DEEPEST_VIEW {
        return None;
    }
    let children = found.children.clone();
    children
        .into_iter()
        .find_map(|child| find_view(child, id, depth + 1))
}

extern "C" fn find_view_by_id(words: *const u64, _: *const usize) -> u64 {
    let [this, id] = arguments::<2>(words);
    let is_view = object(this).is_some_and(|this| this.view.is_some());
    let root = match is_view {
        true => Some(this),
        false => state().content,
    };
    root.and_then(|root| find_view(root, id as u32, 0))
        .unwrap_or(0)
}

extern "C" fn view_init(words: *const u64, _: *const usize) -> u64 {
    let [view] = arguments::<1>(words);
    if let Some(view) = object(view) {
        let shows_text = TEXT_VIEWS.contains(&view.class.as_str());
        view.view = Some(View {
            vertical: view.class.as_str() != LINEAR_LAYOUT,
            ..View::default()
        });
        if shows_text {
            view.text = Some(String::new());
        }
    }
    0
}

extern "C" fn set_on_click_listener(words: *const u64, _: *const usize) -> u64 {
    let [view, listener] = arguments::<2>(words);
    if let Some(view) = object(view).and_then(|view| view.view.as_mut()) {
        view.listener = listener;
    }
    0
}

extern "C" fn set_text_resource(words: *const u64, _: *const usize) -> u64 {
    let [view, id] = arguments::<2>(words);
    let text = resource_string(u64::from(id as u32)).unwrap_or_default();
    if let Some(view) = object(view) {
        view.text = Some(text);
    }
    0
}

extern "C" fn get_text(words: *const u64, _: *const usize) -> u64 {
    let [view] = arguments::<1>(words);
    let text = object(view)
        .and_then(|view| view.text.clone())
        .unwrap_or_default();
    new_string(text)
}

extern "C" fn get_string(words: *const u64, _: *const usize) -> u64 {
    let [_, id] = arguments::<2>(words);
    new_string(resource_string(u64::from(id as u32)).unwrap_or_default())
}

extern "C" fn get_resources(_: *const u64, _: *const usize) -> u64 {
    new_object(Object {
        class: String::from(framework::RESOURCES),
        ..Object::default()
    })
}

fn table_value<T>(id: u64, read: impl FnOnce(&ResourceTable, u32) -> Option<T>) -> Option<T> {
    read(state().resources.as_ref()?, id as u32)
}

extern "C" fn get_dimension(words: *const u64, table: *const usize) -> u64 {
    let [_, id] = arguments::<2>(words);
    match table_value(id, ResourceTable::dimension) {
        Some(data) => u64::from((layout::dimension_pixels(data, SCREEN_DPI) as f32).to_bits()),
        None => raise(table, framework::RESOURCE_NOT_FOUND),
    }
}

extern "C" fn get_dimension_pixels(words: *const u64, table: *const usize) -> u64 {
    let [_, id] = arguments::<2>(words);
    match table_value(id, ResourceTable::dimension) {
        Some(data) => layout::dimension_pixels(data, SCREEN_DPI) as u32 as u64,
        None => raise(table, framework::RESOURCE_NOT_FOUND),
    }
}

extern "C" fn get_colour(words: *const u64, table: *const usize) -> u64 {
    let [_, id] = arguments::<2>(words);
    match table_value(id, ResourceTable::colour) {
        Some(colour) => u64::from(colour),
        None => raise(table, framework::RESOURCE_NOT_FOUND),
    }
}

extern "C" fn get_integer(words: *const u64, table: *const usize) -> u64 {
    let [_, id] = arguments::<2>(words);
    match table_value(id, ResourceTable::integer) {
        Some(value) => value as u32 as u64,
        None => raise(table, framework::RESOURCE_NOT_FOUND),
    }
}

extern "C" fn get_boolean(words: *const u64, table: *const usize) -> u64 {
    let [_, id] = arguments::<2>(words);
    match table_value(id, ResourceTable::boolean) {
        Some(value) => u64::from(value),
        None => raise(table, framework::RESOURCE_NOT_FOUND),
    }
}

/// The device configuration the app's resources are chosen for (D764): the locale from
/// `LC_ALL` or `LANG` (`vi_VN.UTF-8`), the window's orientation, EggOS's dark palette as night
/// mode, the screen's density and the platform version the layer stands for.
fn device_configuration(startup: &Startup) -> eggos_android_package::resources::Configuration {
    let locale = ["LC_ALL=", "LANG="].iter().find_map(|name| {
        startup.environment().find_map(|entry| {
            let entry = entry.to_bytes();
            entry.strip_prefix(name.as_bytes()).map(<[u8]>::to_vec)
        })
    });
    let locale = locale.unwrap_or_default();
    let pair = |at: usize| match locale.get(at..at + 2) {
        Some(&[first, second]) if first.is_ascii_alphabetic() => [first, second],
        _ => [0, 0],
    };
    let language = pair(0);
    let country = match locale.get(2) {
        Some(b'_' | b'-') => pair(3),
        _ => [0, 0],
    };
    eggos_android_package::resources::Configuration {
        language: [language[0].to_ascii_lowercase(), language[1].to_ascii_lowercase()],
        country: [country[0].to_ascii_uppercase(), country[1].to_ascii_uppercase()],
        orientation: if WINDOW_SIZE.0 > WINDOW_SIZE.1 { 2 } else { 1 },
        night: 2,
        density: SCREEN_DPI as u16,
        sdk: PLATFORM_VERSION,
    }
}

fn constraints_of(node: &ViewNode) -> Option<(constraint::Axis, constraint::Axis)> {
    if node.anchors.iter().all(Option::is_none) {
        return None;
    }
    let margin = |side: usize| pixels_of(node.margins[side]).unwrap_or(0);
    let axis = |start: usize, end: usize, bias: Option<i32>| constraint::Axis {
        start: node.anchors[start],
        end: node.anchors[end],
        start_margin: margin(start),
        end_margin: margin(end),
        bias: bias.unwrap_or(CENTRED),
    };
    Some((axis(0, 1, node.biases[0]), axis(2, 3, node.biases[1])))
}

/// The size `view` takes where the window shows it, as `ContentWindow::add` lays it out
/// (D766): a constraint layout's children are measured to be placed.
fn measure(view: u64, depth: usize) -> (i32, i32) {
    let Some(shown) = object(view) else {
        return (0, 0);
    };
    let state_of = shown.view.as_ref();
    let fixed = state_of.map_or((None, None), |state| (state.width, state.height));
    let children = state_of.map_or(Vec::new(), |state| state.children.clone());
    let natural = if !children.is_empty() && depth < DEEPEST_VIEW {
        let vertical = state_of.is_none_or(|state| state.vertical);
        let sizes: Vec<(i32, i32)> = children
            .iter()
            .map(|&child| measure(child, depth + 1))
            .collect();
        match vertical {
            true => (
                sizes.iter().map(|size| size.0).max().unwrap_or(0),
                sizes.iter().map(|size| size.1).sum(),
            ),
            false => (
                COLUMN * sizes.len() as i32,
                sizes.iter().map(|size| size.1).max().unwrap_or(0),
            ),
        }
    } else {
        match shown.class.as_str() {
            EDIT_TEXT => (COLUMN * 2 - PADDING, ROW),
            IMAGE_VIEW => {
                let image = state_of.and_then(|state| state.image);
                image
                    .and_then(|index| state().images.get(index as usize))
                    .map_or((0, 0), |(width, pixels)| {
                        let height = (pixels.len() / (*width).max(1)) as i32;
                        (*width as i32, height + PADDING)
                    })
            }
            _ => {
                let length = shown.text.as_deref().map_or(0, |text| text.chars().count());
                (length as i32 * GLYPH_WIDTH + 2 * PADDING, ROW)
            }
        }
    };
    (fixed.0.unwrap_or(natural.0), fixed.1.unwrap_or(natural.1))
}

fn pixels_of(extent: Option<layout::Extent>) -> Option<i32> {
    match extent? {
        layout::Extent::Dimension(data) => Some(layout::dimension_pixels(data, SCREEN_DPI)),
        layout::Extent::Resource(id) => {
            let data = table_value(u64::from(id), ResourceTable::dimension)?;
            Some(layout::dimension_pixels(data, SCREEN_DPI))
        }
        _ => None,
    }
}

fn text_field(view: u64) -> Option<&'static mut View> {
    object(view).and_then(|view| view.view.as_mut())
}

extern "C" fn add_text_changed_listener(words: *const u64, _: *const usize) -> u64 {
    let [view, watcher] = arguments::<2>(words);
    if let Some(field) = text_field(view)
        && watcher != 0
    {
        field.watchers.push(watcher);
    }
    0
}

extern "C" fn remove_text_changed_listener(words: *const u64, _: *const usize) -> u64 {
    let [view, watcher] = arguments::<2>(words);
    if let Some(field) = text_field(view) {
        field.watchers.retain(|&kept| kept != watcher);
    }
    0
}

extern "C" fn set_on_editor_action_listener(words: *const u64, _: *const usize) -> u64 {
    let [view, listener] = arguments::<2>(words);
    if let Some(field) = text_field(view) {
        field.editor_listener = listener;
    }
    0
}

extern "C" fn set_input_type(words: *const u64, _: *const usize) -> u64 {
    let [view, input_type] = arguments::<2>(words);
    if let Some(field) = text_field(view) {
        field.input_type = input_type as u32;
    }
    0
}

extern "C" fn get_input_type(words: *const u64, _: *const usize) -> u64 {
    let [view] = arguments::<1>(words);
    text_field(view).map_or(0, |field| u64::from(field.input_type))
}

extern "C" fn set_ime_options(words: *const u64, _: *const usize) -> u64 {
    let [view, options] = arguments::<2>(words);
    if let Some(field) = text_field(view) {
        field.ime_action = options as u32 & IME_ACTION_MASK;
    }
    0
}

/// Calls each of `view`'s watchers' `name` with the text as `text` and the extra words.
fn tell_watchers(view: u64, name: &str, text: &str, extra: &[u64]) {
    let watchers = text_field(view).map(|field| field.watchers.clone()).unwrap_or_default();
    for watcher in watchers {
        let Some(class) = object(watcher).map(|watcher| watcher.class.clone()) else {
            continue;
        };
        let Some(address) = compiled_method(&class, name) else {
            continue;
        };
        let shown = new_string(String::from(text));
        let mut words = alloc::vec![watcher, shown];
        words.extend_from_slice(extra);
        call_at(address, &words);
    }
}

/// The person typed in `view`'s field, which now holds `typed` (D762): the text its input
/// type accepts becomes its text, between the watchers' `beforeTextChanged` and
/// `onTextChanged`, then `afterTextChanged`. The accepted text, when it differs from `typed`.
fn text_typed(view: u64, typed: &str) -> Option<String> {
    let input_type = text_field(view).map_or(0, |field| field.input_type);
    let kept = framework::accepted(input_type, typed);
    let before = object(view)?.text.clone().unwrap_or_default();
    if kept == before {
        return (kept != typed).then_some(kept);
    }
    let (start, removed, added) = framework::changed_span(&before, &kept);
    let words = |a: usize, b: usize, c: usize| [a as u64, b as u64, c as u64];
    tell_watchers(view, "beforeTextChanged", &before, &words(start, removed, added));
    object(view)?.text = Some(kept.clone());
    tell_watchers(view, "onTextChanged", &kept, &words(start, removed, added));
    tell_watchers(view, "afterTextChanged", &kept, &[]);
    (kept != typed).then_some(kept)
}

/// Enter in `view`'s field (D762): its `OnEditorActionListener` is told the field's action.
fn editor_action(view: u64) {
    let Some(field) = text_field(view) else {
        return;
    };
    let (listener, action) = (field.editor_listener, field.ime_action);
    let Some(class) = object(listener).map(|listener| listener.class.clone()) else {
        return;
    };
    if let Some(address) = compiled_method(&class, "onEditorAction") {
        call_at(address, &[listener, view, u64::from(action), 0]);
    }
}

extern "C" fn set_hint(words: *const u64, _: *const usize) -> u64 {
    let [view, hint] = arguments::<2>(words);
    let hint = object(hint).and_then(|hint| hint.text.clone());
    if let Some(view) = object(view).and_then(|view| view.view.as_mut()) {
        view.hint = hint;
    }
    0
}

extern "C" fn list_init(words: *const u64, _: *const usize) -> u64 {
    let [list] = arguments::<1>(words);
    if let Some(list) = object(list) {
        list.elements = Some(Vec::new());
    }
    0
}

fn list_elements(list: u64, table: *const usize) -> Option<&'static mut Vec<u64>> {
    match object(list).and_then(|list| list.elements.as_mut()) {
        Some(elements) => Some(elements),
        None => {
            raise(table, framework::NULL_POINTER);
            None
        }
    }
}

fn list_index(elements: &[u64], index: u64, table: *const usize) -> Option<usize> {
    match usize::try_from(index as u32 as i32) {
        Ok(index) if index < elements.len() => Some(index),
        _ => {
            raise(table, INDEX_OUT_OF_BOUNDS);
            None
        }
    }
}

const INDEX_OUT_OF_BOUNDS: &str = "Ljava/lang/IndexOutOfBoundsException;";

extern "C" fn list_add(words: *const u64, table: *const usize) -> u64 {
    let [list, value] = arguments::<2>(words);
    let Some(elements) = list_elements(list, table) else {
        return 0;
    };
    elements.push(value);
    1
}

extern "C" fn list_get(words: *const u64, table: *const usize) -> u64 {
    let [list, index] = arguments::<2>(words);
    let Some(elements) = list_elements(list, table) else {
        return 0;
    };
    list_index(elements, index, table).map_or(0, |index| elements[index])
}

extern "C" fn list_set(words: *const u64, table: *const usize) -> u64 {
    let [list, index, value] = arguments::<3>(words);
    let Some(elements) = list_elements(list, table) else {
        return 0;
    };
    match list_index(elements, index, table) {
        Some(index) => core::mem::replace(&mut elements[index], value),
        None => 0,
    }
}

extern "C" fn list_remove(words: *const u64, table: *const usize) -> u64 {
    let [list, index] = arguments::<2>(words);
    let Some(elements) = list_elements(list, table) else {
        return 0;
    };
    match list_index(elements, index, table) {
        Some(index) => elements.remove(index),
        None => 0,
    }
}

extern "C" fn list_size(words: *const u64, table: *const usize) -> u64 {
    let [list] = arguments::<1>(words);
    list_elements(list, table).map_or(0, |elements| elements.len() as u64)
}

extern "C" fn list_clear(words: *const u64, table: *const usize) -> u64 {
    let [list] = arguments::<1>(words);
    if let Some(elements) = list_elements(list, table) {
        elements.clear();
    }
    0
}

extern "C" fn adapter_init(words: *const u64, _: *const usize) -> u64 {
    let [adapter, _, _, items] = arguments::<4>(words);
    if let Some(adapter) = object(adapter) {
        adapter.reference = items;
    }
    0
}

extern "C" fn set_adapter(words: *const u64, _: *const usize) -> u64 {
    let [view, adapter] = arguments::<2>(words);
    if let Some(view) = object(view) {
        view.reference = adapter;
    }
    0
}

extern "C" fn set_on_item_click_listener(words: *const u64, _: *const usize) -> u64 {
    let [view, listener] = arguments::<2>(words);
    if let Some(view) = object(view).and_then(|view| view.view.as_mut()) {
        view.listener = listener;
        view.on_click = None;
    }
    0
}

/// An Android list's rows as the layer keeps them (D767): the scroll offset, the heights
/// measured, the rows made (their view type and holder or row view) and the pools.
#[derive(Debug, Default)]
struct ListRows {
    offset: i32,
    heights: list_rows::Heights,
    made: BTreeMap<usize, (u32, u64)>,
    pools: list_rows::Pools,
}

fn adapter_of(list: u64) -> u64 {
    object(list).map_or(0, |list| list.reference)
}

fn is_recycler(list: u64) -> bool {
    object(list).is_some_and(|shown| shown.class == RECYCLER_VIEW)
}

fn adapter_call(list: u64, name: &str, words: &[u64]) -> Option<u64> {
    let class = object(adapter_of(list))?.class.clone();
    let address = compiled_method(&class, name)?;
    let result = call_at(address, words);
    (!failed()).then_some(result)
}

fn list_count(list: u64) -> usize {
    let adapter = adapter_of(list);
    let name = match is_recycler(list) {
        true => "getItemCount",
        false => "getCount",
    };
    if let Some(count) = adapter_call(list, name, &[adapter]) {
        return count as u32 as usize;
    }
    let items = object(adapter).map_or(0, |adapter| adapter.reference);
    object(items)
        .and_then(|items| items.elements.as_ref())
        .map_or(0, Vec::len)
}

fn view_type(list: u64, position: usize) -> u32 {
    adapter_call(list, "getItemViewType", &[adapter_of(list), position as u64])
        .map_or(0, |kind| kind as u32)
}

/// Makes or rebinds the row at `position` (D767): a `RecyclerView` adapter's holder
/// (`onCreateViewHolder` only when no pooled one is given, then `onBindViewHolder`), a
/// `ListView` adapter's `getView` with the pooled view as `convertView`, or the layer's own
/// one-text row for an `ArrayAdapter`'s item.
fn make_row(list: u64, position: usize, kind: u32, pooled: Option<u64>) -> Option<u64> {
    let adapter = adapter_of(list);
    if is_recycler(list) {
        let holder = match pooled {
            Some(holder) => holder,
            None => adapter_call(list, "onCreateViewHolder", &[adapter, list, u64::from(kind)])?,
        };
        adapter_call(list, "onBindViewHolder", &[adapter, holder, position as u64])?;
        return Some(holder);
    }
    let convert = pooled.unwrap_or(0);
    if let Some(view) = adapter_call(list, "getView", &[adapter, position as u64, convert, list]) {
        return Some(view);
    }
    let items = object(adapter).map_or(0, |adapter| adapter.reference);
    let item = object(items)?.elements.as_ref()?.get(position).copied()?;
    let text = object(item)
        .map(|item| item.text.clone().unwrap_or_else(|| item.class.clone()))
        .unwrap_or_else(|| String::from("null"));
    let row = match pooled {
        Some(row) => row,
        None => new_object(Object {
            class: String::from(framework::TEXT_VIEW),
            view: Some(View::default()),
            ..Object::default()
        }),
    };
    object(row)?.text = Some(text);
    Some(row)
}

fn row_view(list: u64, holder: u64) -> Option<u64> {
    if !is_recycler(list) {
        return Some(holder);
    }
    let field = state().dex.field_named(VIEW_HOLDER, "itemView")?;
    object(holder)?.fields.get(&field).copied()
}

fn list_span(list: u64) -> usize {
    object(list)
        .and_then(|shown| shown.view.as_ref())
        .map_or(1, |state| state.span.max(1) as usize)
}

fn list_divider(list: u64) -> bool {
    object(list).is_some_and(|shown| {
        shown.class == LIST_VIEW || shown.view.as_ref().is_some_and(|state| state.divider)
    })
}

extern "C" fn grid_init(words: *const u64, _: *const usize) -> u64 {
    let [manager, _, span] = arguments::<3>(words);
    if let Some(manager) = object(manager) {
        manager.fields.insert(SPAN_FIELD, span.max(1));
    }
    0
}

extern "C" fn set_layout_manager(words: *const u64, _: *const usize) -> u64 {
    let [list, manager] = arguments::<2>(words);
    let span = object(manager)
        .and_then(|manager| manager.fields.get(&SPAN_FIELD).copied())
        .unwrap_or(1);
    if let Some(state) = object(list).and_then(|list| list.view.as_mut()) {
        state.span = span as u32;
    }
    state().generation += 1;
    0
}

extern "C" fn add_item_decoration(words: *const u64, _: *const usize) -> u64 {
    let [list, decoration] = arguments::<2>(words);
    let divider = object(decoration)
        .is_some_and(|decoration| decoration.class == framework::DIVIDER_DECORATION);
    if let Some(state) = object(list).and_then(|list| list.view.as_mut()) {
        state.divider |= divider;
    }
    state().generation += 1;
    0
}

/// `notifyDataSetChanged` and its kin (D767): every list showing the adapter makes its rows
/// again from the data.
extern "C" fn notify_changed(words: *const u64, _: *const usize) -> u64 {
    let [adapter] = arguments::<1>(words);
    let shown: Vec<u64> = state()
        .lists
        .keys()
        .copied()
        .filter(|&list| adapter_of(list) == adapter)
        .collect();
    for list in shown {
        if let Some(rows) = state().lists.get_mut(&list) {
            let made = core::mem::take(&mut rows.made);
            for (kind, holder) in made.into_values() {
                rows.pools.give(kind, holder);
            }
            rows.heights.forget_rows();
        }
    }
    state().generation += 1;
    0
}

/// Lays out list `list` for a view `height` tall (D767): the rows that leave the made range
/// go to their type's pool, the rows that enter it are made or rebound, each measured; the
/// cells' rects and the range made answer.
fn lay_out_rows(
    list: u64,
    width: i32,
    height: i32,
) -> (Vec<(i32, i32, i32, i32)>, core::ops::Range<usize>) {
    let count = list_count(list);
    let span = list_span(list);
    let rows = state().lists.entry(list).or_default();
    let offset = rows.offset;
    let kinds: Vec<u32> = (0..count.min(MOST_TYPED_ROWS))
        .map(|position| view_type(list, position))
        .collect();
    let kind_of = |position: usize| kinds.get(position).copied().unwrap_or(0);
    let heights_of = |list: u64, position: usize| {
        state()
            .lists
            .get(&list)
            .map_or(list_rows::LIST_ROW_MIN, |rows| rows.heights.of(position, kind_of(position)))
    };
    let cells = list_rows::cells(count, span, width, 0, |position| heights_of(list, position));
    let range = list_rows::made_range(&cells, span, offset, height);
    let leaving: Vec<(usize, (u32, u64))> = state()
        .lists
        .get(&list)
        .map(|rows| {
            rows.made
                .iter()
                .filter(|(position, _)| !range.contains(position))
                .map(|(&position, &made)| (position, made))
                .collect()
        })
        .unwrap_or_default();
    for (position, (kind, holder)) in leaving {
        if let Some(rows) = state().lists.get_mut(&list) {
            rows.made.remove(&position);
            rows.pools.give(kind, holder);
        }
    }
    for position in range.clone() {
        if state().lists.get(&list).is_some_and(|rows| rows.made.contains_key(&position)) {
            continue;
        }
        let kind = kind_of(position);
        let pooled = state().lists.get_mut(&list).and_then(|rows| rows.pools.take(kind));
        let Some(holder) = make_row(list, position, kind, pooled) else {
            break;
        };
        let measured = row_view(list, holder).map_or(0, |row| measure(row, 0).1);
        if let Some(rows) = state().lists.get_mut(&list) {
            rows.made.insert(position, (kind, holder));
            rows.heights.measured(position, kind, measured);
        }
    }
    let cells = list_rows::cells(count, span, width, 0, |position| heights_of(list, position));
    (cells, range)
}

fn paint_list_row(canvas: &mut Canvas<'_>, rect: Rect, data: u32) {
    if data & DIVIDER != 0 {
        let line = canvas.palette().menu_border;
        canvas.fill(Rect::new(rect.x, rect.y + rect.height - 1, rect.width, 1), line);
    }
}

fn item_clicked(view: u64, position: usize) {
    if object(view).is_some_and(|shown| shown.class == RECYCLER_VIEW) {
        let holder = state()
            .lists
            .get(&view)
            .and_then(|list| list.made.get(&position))
            .map(|&(_, holder)| holder);
        if let Some(row) = holder.and_then(|holder| row_view(view, holder)) {
            click(row);
        }
        return;
    }
    let Some(listener) = object(view)
        .and_then(|view| view.view.as_ref())
        .map(|view| view.listener)
    else {
        return;
    };
    let Some(class) = object(listener).map(|listener| listener.class.clone()) else {
        return;
    };
    if let Some(address) = compiled_method(&class, "onItemClick") {
        let position = position as u64;
        call_at(address, &[listener, view, 0, position, position]);
    }
}

const LARGEST_IMAGE: u32 = 1024;

/// The screen's density the layer draws Android apps at, in dots per inch: `mdpi`, one
/// device-independent pixel a pixel (D763).
const SCREEN_DPI: u32 = 160;

fn keep_image(width: usize, pixels: Vec<u32>) -> Option<u32> {
    let images = &mut state().images;
    images.push((width, pixels));
    u32::try_from(images.len() - 1).ok()
}

fn decode_picture(file: &[u8], webp: bool) -> Option<(usize, Vec<u32>)> {
    if webp {
        let decoded =
            eggos_image::decode(&read_file(file)?, &eggos_image::Limits::DEFAULT).ok()?;
        if decoded.width > LARGEST_IMAGE || decoded.height > LARGEST_IMAGE {
            return None;
        }
        let pixels = decoded.pixels.into_iter().map(premultiplied).collect();
        return Some((decoded.width as usize, pixels));
    }
    let bytes = read_file(file)?;
    let mut source = eggos_png::inflate::SliceSource::new(&bytes);
    let decoder = eggos_png::rows::RowDecoder::open(&mut source).ok()?;
    let header = decoder.header();
    if header.width > LARGEST_IMAGE || header.height > LARGEST_IMAGE {
        return None;
    }
    let width = header.width as usize;
    let mut pixels = vec![0u32; width * header.height as usize];
    let mut work = Box::new(eggos_png::rows::RowWorkspace::new());
    decoder
        .read_rows(&mut work, |row, line| {
            let start = row as usize * width;
            for (target, &pixel) in pixels[start..start + width].iter_mut().zip(line) {
                *target = premultiplied(pixel);
            }
        })
        .ok()?;
    Some((width, pixels))
}

/// Picture file `file` (a PNG or WebP drawn for `density`) decoded and scaled to the screen's
/// density (D763), kept among the layer's images.
fn load_picture(file: &[u8], webp: bool, density: u32) -> Option<u32> {
    let (width, pixels) = decode_picture(file, webp)?;
    let height = pixels.len() / width.max(1);
    if density == 0 || density == SCREEN_DPI {
        return keep_image(width, pixels);
    }
    let scale = |length: usize| (length * SCREEN_DPI as usize / density as usize).max(1);
    let (new_width, new_height) = (scale(width), scale(height));
    keep_image(
        new_width,
        crate::image_fit::resample(&pixels, width, new_width, new_height),
    )
}

fn load_image(id: u32) -> Option<u32> {
    let (path, density) = {
        let table = state().resources.as_ref()?;
        let (path, density) = table.density_variant(id, SCREEN_DPI)?;
        (String::from(path), density)
    };
    let file = [state().folder.as_slice(), b"/", path.as_bytes()].concat();
    if path.ends_with(".xml") {
        return load_vector(&file);
    }
    match (path.ends_with(".webp"), path.ends_with(".png")) {
        (true, _) => load_picture(&file, true, density),
        (false, true) => load_picture(&file, false, density),
        _ => None,
    }
}

fn load_vector(file: &[u8]) -> Option<u32> {
    let vector = eggos_android_package::vector::parse(&read_file(file)?)?;
    let image = crate::vector_raster::rasterize(&vector, |colour| match colour {
        eggos_android_package::vector::Colour::Value(value) => Some(value),
        eggos_android_package::vector::Colour::Resource(id) => {
            state().resources.as_ref()?.colour(id)
        }
    })?;
    let images = &mut state().images;
    images.push(image);
    u32::try_from(images.len() - 1).ok()
}

fn premultiplied(pixel: u32) -> u32 {
    let alpha = pixel >> 24;
    let scale = |shift: u32| ((pixel >> shift & 0xff) * alpha / 255) << shift;
    alpha << 24 | scale(16) | scale(8) | scale(0)
}

fn paint_image(canvas: &mut Canvas<'_>, rect: Rect, data: u32) {
    let Some(&(image, x, y)) = state().placed_images.get(data as usize) else {
        return;
    };
    if let Some((width, pixels)) = state().images.get(image as usize) {
        let clip = canvas.narrow(rect);
        canvas.composite((rect.x + x, rect.y + y), pixels, *width, false);
        canvas.restore(clip);
    }
}

/// Image `image` fitted in a `area`-sized box by scale type `kind` (D763): the image to draw
/// (scaled once and kept) and where in the box it goes.
fn fitted_image(image: u32, kind: u32, area: (i32, i32)) -> Option<(u32, i32, i32)> {
    let (width, pixels) = state().images.get(image as usize)?;
    let size = (*width as i32, (pixels.len() / (*width).max(1)) as i32);
    let placed = crate::image_fit::fit(kind, size, area);
    if (placed.width, placed.height) == size {
        return Some((image, placed.x, placed.y));
    }
    let key = (image, placed.width, placed.height);
    if let Some(&scaled) = state().scaled_images.get(&key) {
        return Some((scaled, placed.x, placed.y));
    }
    let resampled = crate::image_fit::resample(
        pixels,
        *width,
        placed.width as usize,
        placed.height as usize,
    );
    let scaled = keep_image(placed.width as usize, resampled)?;
    state().scaled_images.insert(key, scaled);
    Some((scaled, placed.x, placed.y))
}

extern "C" fn decode_resource(words: *const u64, _: *const usize) -> u64 {
    let [_, id] = arguments::<2>(words);
    bitmap_of(load_image(id as u32))
}

extern "C" fn decode_file(words: *const u64, _: *const usize) -> u64 {
    let [path] = arguments::<1>(words);
    let Some(path) = object(path).and_then(|path| path.text.clone()) else {
        return 0;
    };
    let webp = path.ends_with(".webp");
    bitmap_of(load_picture(path.as_bytes(), webp, SCREEN_DPI))
}

fn bitmap_of(image: Option<u32>) -> u64 {
    let Some(image) = image else {
        return 0;
    };
    let mut fields = BTreeMap::new();
    fields.insert(BITMAP_IMAGE, u64::from(image));
    new_object(Object {
        class: String::from(framework::BITMAP),
        fields,
        ..Object::default()
    })
}

fn bitmap_size(bitmap: u64) -> (u64, u64) {
    let image = object(bitmap).and_then(|bitmap| bitmap.fields.get(&BITMAP_IMAGE).copied());
    let Some((width, pixels)) = image.and_then(|image| state().images.get(image as usize)) else {
        return (0, 0);
    };
    (*width as u64, (pixels.len() / (*width).max(1)) as u64)
}

extern "C" fn bitmap_width(words: *const u64, _: *const usize) -> u64 {
    let [bitmap] = arguments::<1>(words);
    bitmap_size(bitmap).0
}

extern "C" fn bitmap_height(words: *const u64, _: *const usize) -> u64 {
    let [bitmap] = arguments::<1>(words);
    bitmap_size(bitmap).1
}

extern "C" fn set_image_bitmap(words: *const u64, _: *const usize) -> u64 {
    let [view, bitmap] = arguments::<2>(words);
    let image = object(bitmap).and_then(|bitmap| bitmap.fields.get(&BITMAP_IMAGE).copied());
    if let Some(view) = object(view).and_then(|view| view.view.as_mut()) {
        view.image = image.map(|image| image as u32);
    }
    0
}

extern "C" fn set_scale_type(words: *const u64, _: *const usize) -> u64 {
    let [view, kind] = arguments::<2>(words);
    let name = object(kind).and_then(|kind| kind.text.clone()).unwrap_or_default();
    let ordinal = framework::SCALE_TYPES.iter().position(|known| *known == name);
    if let (Some(view), Some(ordinal)) = (object(view).and_then(|view| view.view.as_mut()), ordinal)
    {
        view.scale_type = Some(ordinal as u32);
    }
    0
}

extern "C" fn inflater_from(_: *const u64, _: *const usize) -> u64 {
    new_object(Object {
        class: String::from("Landroid/view/LayoutInflater;"),
        ..Object::default()
    })
}

extern "C" fn inflate_layout(words: *const u64, _: *const usize) -> u64 {
    let [_, layout_id] = arguments::<2>(words);
    inflate(u64::from(layout_id as u32))
}

extern "C" fn set_image_resource(words: *const u64, _: *const usize) -> u64 {
    let [view, id] = arguments::<2>(words);
    let image = load_image(id as u32);
    if let Some(view) = object(view).and_then(|view| view.view.as_mut()) {
        view.image = image;
    }
    0
}

extern "C" fn add_view(words: *const u64, _: *const usize) -> u64 {
    let [group, child] = arguments::<2>(words);
    if let Some(group) = object(group).and_then(|group| group.view.as_mut()) {
        group.children.push(child);
    }
    0
}

extern "C" fn set_orientation(words: *const u64, _: *const usize) -> u64 {
    let [layout, orientation] = arguments::<2>(words);
    if let Some(layout) = object(layout).and_then(|layout| layout.view.as_mut()) {
        layout.vertical = orientation as u32 == 1;
    }
    0
}

fn native_entry(native: Native) -> Entry {
    match native {
        Native::Nothing => nothing,
        Native::SetContentView => set_content_view,
        Native::TextViewInit => text_view_init,
        Native::SetText => set_text,
        Native::SetContentLayout => set_content_layout,
        Native::FindViewById => find_view_by_id,
        Native::ViewInit => view_init,
        Native::SetOnClickListener => set_on_click_listener,
        Native::SetTextResource => set_text_resource,
        Native::GetText => get_text,
        Native::GetString => get_string,
        Native::GetResources => get_resources,
        Native::GridInit => grid_init,
        Native::SetLayoutManager => set_layout_manager,
        Native::AddItemDecoration => add_item_decoration,
        Native::NotifyChanged => notify_changed,
        Native::IntentInit => intent_init,
        Native::IntentAction => intent_action,
        Native::PutExtra => put_extra,
        Native::GetExtra => get_extra,
        Native::HasExtra => has_extra,
        Native::StartActivity => start_activity_native,
        Native::StartActivityForResult => start_activity_for_result,
        Native::Finish => finish,
        Native::GetIntent => get_intent,
        Native::SetResult => set_result,
        Native::GetFragmentManager => get_fragment_manager,
        Native::BeginTransaction => begin_transaction,
        Native::TransactionAdd => transaction_add,
        Native::TransactionReplace => transaction_replace,
        Native::TransactionRemove => transaction_remove,
        Native::Commit => commit,
        Native::ReturnThis => return_this,
        Native::FragmentHost => fragment_host,
        Native::FragmentView => fragment_view,
        Native::GetDimension => get_dimension,
        Native::GetDimensionPixels => get_dimension_pixels,
        Native::GetColour => get_colour,
        Native::GetInteger => get_integer,
        Native::GetBoolean => get_boolean,
        Native::AddView => add_view,
        Native::SetOrientation => set_orientation,
        Native::SetHint => set_hint,
        Native::AddTextChangedListener => add_text_changed_listener,
        Native::RemoveTextChangedListener => remove_text_changed_listener,
        Native::SetOnEditorActionListener => set_on_editor_action_listener,
        Native::SetInputType => set_input_type,
        Native::GetInputType => get_input_type,
        Native::SetImeOptions => set_ime_options,
        Native::ListInit => list_init,
        Native::ListAdd => list_add,
        Native::ListGet => list_get,
        Native::ListSet => list_set,
        Native::ListRemove => list_remove,
        Native::ListSize => list_size,
        Native::ListClear => list_clear,
        Native::AdapterInit => adapter_init,
        Native::SetAdapter => set_adapter,
        Native::SetOnItemClickListener => set_on_item_click_listener,
        Native::SetImageResource => set_image_resource,
        Native::DecodeResource => decode_resource,
        Native::DecodeFile => decode_file,
        Native::BitmapWidth => bitmap_width,
        Native::BitmapHeight => bitmap_height,
        Native::SetImageBitmap => set_image_bitmap,
        Native::SetScaleType => set_scale_type,
        Native::InflaterFrom => inflater_from,
        Native::Inflate => inflate_layout,
        Native::ExceptionInit => exception_init,
        Native::GetMessage => get_message,
        Native::ExceptionInitEmpty => exception_init_empty,
        Native::PrintStackTrace => print_stack_trace,
        Native::GetStackTrace => get_stack_trace,
        Native::GetCause => get_cause,
        Native::FillInStackTrace => fill_in_stack_trace,
        Native::FrameClassName => frame_class_name,
        Native::FrameMethodName => frame_method_name,
        Native::FrameText => frame_text,
    }
}

fn helpers() -> [Entry; helper::COUNT] {
    let mut entries: [Entry; helper::COUNT] = [missing_method; helper::COUNT];
    entries[helper::STRING] = string;
    entries[helper::NEW_INSTANCE] = new_instance;
    entries[helper::FIELD_GET] = field_get;
    entries[helper::FIELD_PUT] = field_put;
    entries[helper::DIVIDE_INT] = divide_int;
    entries[helper::REMAINDER_INT] = remainder_int;
    entries[helper::DIVIDE_LONG] = divide_long;
    entries[helper::REMAINDER_LONG] = remainder_long;
    entries[helper::INVOKE_VIRTUAL] = invoke_virtual;
    entries[helper::STATIC_GET] = static_get;
    entries[helper::STATIC_PUT] = static_put;
    entries[helper::NEW_ARRAY] = new_array;
    entries[helper::ARRAY_LENGTH] = array_length;
    entries[helper::ARRAY_GET] = array_get;
    entries[helper::ARRAY_PUT] = array_put;
    entries[helper::THROW] = throw;
    entries[helper::CHECK_CAST] = check_cast;
    entries[helper::INSTANCE_OF] = instance_of;
    entries[helper::REMAINDER_FLOAT] = remainder_float;
    entries[helper::REMAINDER_DOUBLE] = remainder_double;
    entries[helper::FLOAT_TO_INT] = float_to_int;
    entries[helper::FLOAT_TO_LONG] = float_to_long;
    entries[helper::DOUBLE_TO_INT] = double_to_int;
    entries[helper::DOUBLE_TO_LONG] = double_to_long;
    entries[helper::CATCHES] = catches;
    entries[helper::TAKE_EXCEPTION] = take_exception;
    entries[helper::CONST_CLASS] = const_class;
    entries
}

extern "C" fn const_class(words: *const u64, _: *const usize) -> u64 {
    at_safepoint();
    let [index] = arguments::<1>(words);
    new_object(Object {
        class: String::from(framework::CLASS_CLASS),
        text: state().dex.type_name(index as u32),
        ..Object::default()
    })
}

extern "C" fn intent_init(words: *const u64, _: *const usize) -> u64 {
    let [intent, _, class] = arguments::<3>(words);
    let target = object(class).and_then(|class| class.text.clone());
    if let Some(intent) = object(intent) {
        intent.text = target;
    }
    0
}

extern "C" fn intent_action(_: *const u64, _: *const usize) -> u64 {
    0
}

fn extra_name(name: u64) -> String {
    object(name).and_then(|name| name.text.clone()).unwrap_or_default()
}

extern "C" fn put_extra(words: *const u64, _: *const usize) -> u64 {
    let [intent, name, value] = arguments::<3>(words);
    let name = extra_name(name);
    if let Some(intent) = object(intent) {
        intent.extras.retain(|(kept, _)| *kept != name);
        intent.extras.push((name, value));
    }
    intent
}

fn extra(intent: u64, name: u64) -> Option<u64> {
    let name = extra_name(name);
    let intent = object(intent)?;
    intent
        .extras
        .iter()
        .find(|(kept, _)| *kept == name)
        .map(|(_, value)| *value)
}

extern "C" fn get_extra(words: *const u64, _: *const usize) -> u64 {
    let [intent, name, default] = arguments::<3>(words);
    extra(intent, name).unwrap_or(default)
}

extern "C" fn has_extra(words: *const u64, _: *const usize) -> u64 {
    let [intent, name] = arguments::<2>(words);
    u64::from(extra(intent, name).is_some())
}

/// Starts the activity `intent` names over the one shown (D765): made, its intent kept,
/// `<init>`, `onCreate`, `onStart` and `onResume` called; `request` is the request code when it
/// was started for a result.
fn start_activity(intent: u64, request: Option<u32>) {
    let Some(class) = object(intent).and_then(|intent| intent.text.clone()) else {
        return;
    };
    if state().dex.class_index(&class).is_none() {
        return;
    }
    initialize(&class, table());
    if failed() {
        return;
    }
    let current = state();
    let caller = current.activity;
    current.below.push(Shown {
        activity: caller,
        content: current.content,
        request: current.request,
        result: current.result,
    });
    let made = new_object(Object {
        class: class.clone(),
        reference: intent,
        ..Object::default()
    });
    let current = state();
    current.activity = made;
    current.content = None;
    current.request = request.map(|code| (code, caller));
    current.result = (0, 0);
    current.generation += 1;
    for (name, words) in [
        ("<init>", &[made][..]),
        ("onCreate", &[made, 0][..]),
        ("onStart", &[made][..]),
        ("onResume", &[made][..]),
    ] {
        if let Some(address) = compiled_method(&class, name) {
            call_at(address, words);
            if failed() {
                return;
            }
        }
    }
}

extern "C" fn start_activity_native(words: *const u64, _: *const usize) -> u64 {
    let [_, intent] = arguments::<2>(words);
    start_activity(intent, None);
    0
}

extern "C" fn start_activity_for_result(words: *const u64, _: *const usize) -> u64 {
    let [_, intent, code] = arguments::<3>(words);
    let request = (code as u32 as i32 >= 0).then_some(code as u32);
    start_activity(intent, request);
    0
}

/// Finishes the activity shown (D765): the one under it is shown again and, when it asked for a
/// result, told it with `onActivityResult`; finishing the last one ends the app.
fn finish_activity() {
    let current = state();
    let Some(under) = current.below.pop() else {
        syscall::exit(0);
    };
    let finished_request = current.request;
    let finished_result = current.result;
    current.activity = under.activity;
    current.content = under.content;
    current.request = under.request;
    current.result = under.result;
    current.generation += 1;
    let class = object(under.activity).map(|shown| shown.class.clone()).unwrap_or_default();
    if let Some((code, _)) = finished_request
        && let Some(address) = compiled_method(&class, "onActivityResult")
    {
        let (result, data) = finished_result;
        call_at(address, &[under.activity, u64::from(code), u64::from(result), data]);
    }
    if let Some(address) = compiled_method(&class, "onResume") {
        call_at(address, &[under.activity]);
    }
}

extern "C" fn finish(words: *const u64, _: *const usize) -> u64 {
    let [activity] = arguments::<1>(words);
    if activity == state().activity {
        finish_activity();
    }
    0
}

extern "C" fn get_intent(words: *const u64, _: *const usize) -> u64 {
    let [activity] = arguments::<1>(words);
    object(activity).map_or(0, |activity| activity.reference)
}

extern "C" fn set_result(words: *const u64, _: *const usize) -> u64 {
    let [activity, code, data] = arguments::<3>(words);
    if activity == state().activity {
        state().result = (code as u32, data);
    }
    0
}

/// The back key (D765): the shown activity's own `onBackPressed`, else it finishes.
fn back() {
    let activity = state().activity;
    let class = object(activity).map(|shown| shown.class.clone()).unwrap_or_default();
    match compiled_method(&class, "onBackPressed") {
        Some(address) => {
            call_at(address, &[activity]);
        }
        None => finish_activity(),
    }
}

extern "C" fn get_fragment_manager(words: *const u64, _: *const usize) -> u64 {
    let [owner] = arguments::<1>(words);
    new_object(Object {
        class: String::from(framework::FRAGMENT_MANAGER),
        reference: owner,
        ..Object::default()
    })
}

extern "C" fn begin_transaction(words: *const u64, _: *const usize) -> u64 {
    let [manager] = arguments::<1>(words);
    new_object(Object {
        class: String::from(framework::FRAGMENT_TRANSACTION),
        reference: manager,
        elements: Some(Vec::new()),
        ..Object::default()
    })
}

const FRAGMENT_ADD: u64 = 1;
const FRAGMENT_REPLACE: u64 = 2;
const FRAGMENT_REMOVE: u64 = 3;
const FRAGMENT_HOST: u32 = u32::MAX - 1;

fn keep_operation(transaction: u64, kind: u64, container: u64, fragment: u64) -> u64 {
    if let Some(elements) = object(transaction).and_then(|kept| kept.elements.as_mut()) {
        elements.extend_from_slice(&[kind, container, fragment]);
    }
    transaction
}

extern "C" fn transaction_add(words: *const u64, _: *const usize) -> u64 {
    let [transaction, first, second] = arguments::<3>(words);
    match first > u64::from(u32::MAX) {
        true => keep_operation(transaction, FRAGMENT_ADD, 0, first),
        false => keep_operation(transaction, FRAGMENT_ADD, first, second),
    }
}

extern "C" fn transaction_replace(words: *const u64, _: *const usize) -> u64 {
    let [transaction, container, fragment] = arguments::<3>(words);
    keep_operation(transaction, FRAGMENT_REPLACE, container, fragment)
}

extern "C" fn transaction_remove(words: *const u64, _: *const usize) -> u64 {
    let [transaction, fragment] = arguments::<2>(words);
    keep_operation(transaction, FRAGMENT_REMOVE, 0, fragment)
}

fn host_activity(fragment: u64) -> u64 {
    object(fragment)
        .and_then(|fragment| fragment.fields.get(&FRAGMENT_HOST).copied())
        .unwrap_or(state().activity)
}

fn show_fragment(fragment: u64, container: u64, replace: bool) {
    let Some(class) = object(fragment).map(|fragment| fragment.class.clone()) else {
        return;
    };
    let host = state().activity;
    if let Some(kept) = object(fragment) {
        kept.fields.insert(FRAGMENT_HOST, host);
    }
    if !state().fragments.contains(&fragment) {
        state().fragments.push(fragment);
    }
    if let Some(address) = compiled_method(&class, "onCreate") {
        call_at(address, &[fragment, 0]);
    }
    let view = match compiled_method(&class, "onCreateView") {
        Some(address) => {
            let inflater = inflater_from(core::ptr::null(), core::ptr::null());
            call_at(address, &[fragment, inflater, container, 0])
        }
        None => 0,
    };
    if failed() {
        return;
    }
    if let Some(kept) = object(fragment) {
        kept.reference = view;
    }
    if let Some(children) = object(container)
        .and_then(|container| container.view.as_mut())
        .map(|container| &mut container.children)
    {
        if replace {
            children.clear();
        }
        if view != 0 {
            children.push(view);
        }
    }
    if view != 0
        && let Some(address) = compiled_method(&class, "onViewCreated")
    {
        call_at(address, &[fragment, view, 0]);
    }
}

fn hide_fragment(fragment: u64) {
    let view = object(fragment).map_or(0, |fragment| fragment.reference);
    if let Some(root) = state().content {
        detach_view(root, view, 0);
    }
    state().fragments.retain(|&kept| kept != fragment);
}

fn detach_view(parent: u64, view: u64, depth: usize) {
    if depth > DEEPEST_VIEW {
        return;
    }
    let Some(children) = object(parent).and_then(|parent| parent.view.as_mut()) else {
        return;
    };
    children.children.retain(|&child| child != view);
    for child in children.children.clone() {
        detach_view(child, view, depth + 1);
    }
}

extern "C" fn commit(words: *const u64, _: *const usize) -> u64 {
    let [transaction] = arguments::<1>(words);
    let operations = object(transaction)
        .and_then(|kept| kept.elements.clone())
        .unwrap_or_default();
    for operation in operations.chunks_exact(3) {
        let (kind, container_id, fragment) = (operation[0], operation[1], operation[2]);
        if kind == FRAGMENT_REMOVE {
            hide_fragment(fragment);
            continue;
        }
        let container = state()
            .content
            .and_then(|root| find_view(root, container_id as u32, 0));
        let Some(container) = container else {
            continue;
        };
        if kind == FRAGMENT_REPLACE {
            for shown in state().fragments.clone() {
                if host_activity(shown) == state().activity {
                    hide_fragment(shown);
                }
            }
        }
        show_fragment(fragment, container, kind == FRAGMENT_REPLACE);
        if failed() {
            break;
        }
    }
    state().generation += 1;
    0
}

extern "C" fn return_this(words: *const u64, _: *const usize) -> u64 {
    let [this] = arguments::<1>(words);
    this
}

extern "C" fn fragment_host(words: *const u64, _: *const usize) -> u64 {
    let [fragment] = arguments::<1>(words);
    host_activity(fragment)
}

extern "C" fn fragment_view(words: *const u64, _: *const usize) -> u64 {
    let [fragment] = arguments::<1>(words);
    object(fragment).map_or(0, |fragment| fragment.reference)
}

fn compile_all(dex: &Dex<'_>) -> (Vec<u8>, BTreeMap<u32, usize>) {
    let mut code = Vec::new();
    let mut compiled = BTreeMap::new();
    for class in 0..dex.class_count() {
        for method in dex.methods(class) {
            let Some(body) = method.code else {
                continue;
            };
            match compiler::compile_for(MACHINE, &body) {
                Ok(done) => {
                    compiled.insert(method.index, code.len());
                    code.extend_from_slice(&done.code);
                }
                Err(_) => {
                    let name = dex.method_name(method.index).unwrap_or_default();
                    let class = dex.method_class(method.index).unwrap_or_default();
                    let named = [class.as_bytes(), b".", name.as_bytes()].concat();
                    say(&[b"android: not compiled yet: ", &named]);
                }
            }
        }
    }
    (code, compiled)
}

fn method_named(dex: &Dex<'_>, class: u32, name: &str) -> Option<u32> {
    dex.methods(class)
        .iter()
        .map(|method| method.index)
        .find(|&index| dex.method_name(index).as_deref() == Some(name))
}

fn table() -> *const usize {
    state().table as *const usize
}

/// Calls compiled code from the layer. An exception it leaves pending passes back to the
/// app's code when the layer was itself called from it (a framework method calling back into
/// the app, D761); at the outermost call it is uncaught and ends the app with its trace.
fn call_at(address: usize, words: &[u64]) -> u64 {
    if state().depth > 0 && failed() {
        return 0;
    }
    state().depth += 1;
    let result = entry_at(address)(words.as_ptr(), table());
    state().depth -= 1;
    if state().depth == 0 {
        report_if_uncaught();
    }
    result
}

fn call(method: u32, words: &[u64]) -> u64 {
    // SAFETY: `table()` is the entry table, with an entry for every method of the DEX file;
    // `method` is one of them.
    let address = unsafe { table().wrapping_add(method_entry(method)).read() };
    call_at(address, words)
}

fn compiled_method(class: &str, name: &str) -> Option<usize> {
    let state = state();
    let mut class = String::from(class);
    for _ in 0..DEEPEST_VIEW {
        if framework::is_replaced(&class) {
            return None;
        }
        let index = state.dex.class_index(&class)?;
        let found = method_named(&state.dex, index, name)
            .and_then(|method| state.compiled.get(&method))
            .map(|&offset| state.base + offset);
        if found.is_some() {
            return found;
        }
        class = state.dex.superclass(index)?;
    }
    None
}

fn click(view: u64) {
    let Some(shown) = object(view).and_then(|view| view.view.clone()) else {
        return;
    };
    let (receiver, name) = match shown.on_click {
        Some(name) if shown.listener == 0 => (state().activity, name),
        _ => (shown.listener, String::from("onClick")),
    };
    let Some(class) = object(receiver).map(|receiver| receiver.class.clone()) else {
        return;
    };
    if let Some(address) = compiled_method(&class, &name) {
        call_at(address, &[receiver, view]);
    }
}

struct ContentWindow {
    tree: WidgetTree<MOST_WIDGETS>,
    shown: Vec<(WidgetId, u64)>,
    lists: Vec<(WidgetId, u64)>,
    rows: Vec<(WidgetId, u64, usize)>,
    parent: Option<WidgetId>,
    generation: u64,
}

impl ContentWindow {
    fn add(&mut self, view: u64, x: i32, y: i32, depth: usize) -> i32 {
        let Some(shown) = object(view).map(|view| view.clone()) else {
            return 0;
        };
        let (class, text, view_state) = (shown.class, shown.text, shown.view);
        let children = view_state
            .as_ref()
            .map_or(Vec::new(), |state| state.children.clone());
        if !children.is_empty() && depth < DEEPEST_VIEW && class == framework::CONSTRAINT_LAYOUT {
            return self.add_constrained(&children, x, y, depth);
        }
        if !children.is_empty() && depth < DEEPEST_VIEW {
            let vertical = view_state.as_ref().is_none_or(|state| state.vertical);
            let mut used = 0;
            for (index, child) in children.into_iter().enumerate() {
                match vertical {
                    true => used += self.add(child, x, y + used, depth + 1),
                    false => {
                        let column = x + index as i32 * COLUMN;
                        used = used.max(self.add(child, column, y, depth + 1));
                    }
                }
            }
            return used;
        }
        if class.as_str() == IMAGE_VIEW {
            let image = view_state.as_ref().and_then(|state| state.image);
            let Some((index, (width, pixels))) =
                image.and_then(|index| Some((index, state().images.get(index as usize)?)))
            else {
                return 0;
            };
            let natural = (*width as i32, (pixels.len() / (*width).max(1)) as i32);
            let wanted = view_state.as_ref().map(|state| (state.width, state.height));
            let area = match wanted {
                Some((width, height)) => (width.unwrap_or(natural.0), height.unwrap_or(natural.1)),
                None => natural,
            };
            let kind = view_state
                .as_ref()
                .and_then(|state| state.scale_type)
                .unwrap_or(crate::image_fit::scale_type::FIT_CENTER);
            let Some(placed) = fitted_image(index, kind, area) else {
                return 0;
            };
            let placed_images = &mut state().placed_images;
            placed_images.push(placed);
            let place = Place::at(x, y, Length::Px(area.0), Length::Px(area.1));
            let painted = Kind::Custom {
                painter: paint_image,
                data: (placed_images.len() - 1) as u32,
            };
            let _ = self.tree.add(self.parent, place, painted);
            return area.1 + PADDING;
        }
        if class.as_str() == LIST_VIEW || class.as_str() == RECYCLER_VIEW {
            return self.add_list(view, x, y, depth);
        }
        let Some(text) = text else {
            return 0;
        };
        let hint = view_state.as_ref().and_then(|state| state.hint.clone());
        let kind = match class.as_str() {
            BUTTON => Kind::button(&text, false),
            EDIT_TEXT => {
                let hint = hint.unwrap_or_default().into_boxed_str();
                let placeholder: &'static str = Box::leak(hint);
                let input_type = view_state.as_ref().map_or(0, |state| state.input_type);
                match framework::is_password(input_type) {
                    true => Kind::masked_text_field(placeholder, Escape::Revert),
                    false => Kind::text_field(placeholder, Escape::Revert),
                }
            }
            _ => Kind::label(&text, Token::Text),
        };
        let width = match class.as_str() {
            EDIT_TEXT => Length::Px(COLUMN * 2 - PADDING),
            _ => Length::Content,
        };
        let place = Place::at(x, y, width, Length::Content);
        if let Some(widget) = self.tree.add(self.parent, place, kind) {
            if class.as_str() == EDIT_TEXT {
                self.tree.set_text(widget, &text);
            }
            self.shown.push((widget, view));
        }
        ROW
    }

    /// A list as the app's own rows in a scroll area of its own (D767,
    /// docs/ux/android-lists-v0.1.md): the rows made for the visible range, each a toolkit row
    /// (its states and click) under the row view's widgets.
    fn add_list(&mut self, list: u64, x: i32, y: i32, depth: usize) -> i32 {
        let width = WINDOW_SIZE.0 as i32 - x - PADDING;
        let height = (WINDOW_SIZE.1 as i32 - y - PADDING).max(list_rows::LIST_ROW_MIN);
        let (cells, range) = lay_out_rows(list, width, height);
        let offset = state().lists.get(&list).map_or(0, |rows| rows.offset);
        let area = Kind::ScrollView {
            offset,
            content_height: list_rows::content_height(&cells),
            offset_x: 0,
            content_width: width,
        };
        let place = Place::at(x, y, Length::Px(width), Length::Px(height));
        let Some(scroll) = self.tree.add(self.parent, place, area) else {
            return height;
        };
        self.lists.push((scroll, list));
        let divider = match list_divider(list) {
            true => DIVIDER,
            false => 0,
        };
        let outer = self.parent.replace(scroll);
        for position in range {
            let Some(&(cx, cy, cw, ch)) = cells.get(position) else {
                continue;
            };
            let holder = state()
                .lists
                .get(&list)
                .and_then(|rows| rows.made.get(&position))
                .map(|&(_, holder)| holder);
            let row = Kind::Row {
                painter: paint_list_row,
                data: divider,
            };
            let place = Place::at(cx, cy, Length::Px(cw), Length::Px(ch));
            if let Some(widget) = self.tree.add(Some(scroll), place, row) {
                self.rows.push((widget, list, position));
            }
            if let Some(shown) = holder.and_then(|holder| row_view(list, holder)) {
                self.add(shown, cx, cy, depth + 1);
            }
        }
        self.parent = outer;
        height
    }

    /// Notices a list the person scrolled (D767): its offset kept, its rows made again for
    /// the new range.
    fn track_scrolls(&mut self) {
        let mut moved = false;
        for &(scroll, list) in &self.lists {
            let Some(Kind::ScrollView { offset, .. }) = self.tree.kind(scroll).copied() else {
                continue;
            };
            if let Some(rows) = state().lists.get_mut(&list)
                && rows.offset != offset
            {
                rows.offset = offset;
                moved = true;
            }
        }
        if moved {
            state().generation += 1;
            self.refresh();
        }
    }

    /// A `ConstraintLayout`'s children placed by their constraints (D766) in the window's
    /// width from `x`: each measured, the positions solved, then each added where it goes.
    fn add_constrained(&mut self, children: &[u64], x: i32, y: i32, depth: usize) -> i32 {
        let width = WINDOW_SIZE.0 as i32 - x - PADDING;
        let placed: Vec<constraint::Child> = children
            .iter()
            .map(|&child| {
                let shown = object(child).and_then(|child| child.view.clone());
                let (horizontal, vertical) = shown
                    .as_ref()
                    .and_then(|state| state.constraints)
                    .unwrap_or_default();
                constraint::Child {
                    id: shown.as_ref().map_or(0, |state| state.id),
                    size: measure(child, depth + 1),
                    horizontal,
                    vertical,
                }
            })
            .collect();
        let height = WINDOW_SIZE.1 as i32 - y - PADDING;
        let positions = constraint::solve(&placed, (width, height));
        let mut bottom = 0;
        for ((&child, &(cx, cy)), sized) in children.iter().zip(&positions).zip(&placed) {
            let used = self.add(child, x + cx, y + cy, depth + 1);
            bottom = bottom.max(cy + used.max(sized.size.1));
        }
        bottom
    }

    /// Makes the window's widgets again when the activity shown or its content changed
    /// (D765): another activity started or finished, or a fragment transaction committed.
    fn rebuild_if_changed(&mut self) {
        if self.generation == state().generation {
            return;
        }
        self.generation = state().generation;
        self.tree = WidgetTree::<MOST_WIDGETS>::new();
        self.shown.clear();
        self.lists.clear();
        self.rows.clear();
        self.parent = None;
        state().placed_images.clear();
        if let Some(root) = state().content {
            self.add(root, PADDING, PADDING, 0);
        }
    }

    fn refresh(&mut self) {
        self.rebuild_if_changed();
        for &(widget, view) in &self.shown {
            if let Some(text) = object(view).and_then(|view| view.text.clone()) {
                self.tree.set_text(widget, &text);
            }
        }
    }

    fn edited(&mut self, widget: WidgetId) {
        let Some(&(_, view)) = self.shown.iter().find(|(shown, _)| *shown == widget) else {
            return;
        };
        let Some(typed) = self.tree.text_of(widget).map(String::from) else {
            return;
        };
        if let Some(accepted) = text_typed(view, &typed) {
            self.tree.set_text(widget, &accepted);
        }
        self.refresh();
    }

    fn entered(&mut self) {
        let Some(focused) = self.tree.focus() else {
            return;
        };
        let Some(&(_, view)) = self.shown.iter().find(|(shown, _)| *shown == focused) else {
            return;
        };
        editor_action(view);
        self.refresh();
    }
}

impl Application<MOST_WIDGETS> for ContentWindow {
    fn tree(&mut self) -> &mut WidgetTree<MOST_WIDGETS> {
        &mut self.tree
    }

    fn respond(&mut self, response: Response, _now_ms: u32) {
        match response {
            Response::Activated(widget) => {
                if let Some(&(_, view)) = self.shown.iter().find(|(shown, _)| *shown == widget) {
                    click(view);
                    self.refresh();
                } else if let Some(&(_, list, position)) =
                    self.rows.iter().find(|(row, _, _)| *row == widget)
                {
                    item_clicked(list, position);
                    self.refresh();
                }
            }
            Response::Edited(widget) | Response::Cut(widget) => self.edited(widget),
            Response::Unused { code: key::ENTER, .. } => self.entered(),
            Response::Unused {
                code: key::ESC | key::BACK,
                ..
            } => {
                back();
                self.refresh();
            }
            _ => {}
        }
        self.track_scrolls();
    }

    fn back(&mut self, _now_ms: u32) -> bool {
        back();
        self.refresh();
        true
    }
}

fn show(title: &str) -> i32 {
    let Ok(mut window) = Window::create(title, WINDOW_SIZE.0, WINDOW_SIZE.1, (160, 120)) else {
        stop(&[b"android: no window for the app"]);
    };
    let mut content = ContentWindow {
        tree: WidgetTree::<MOST_WIDGETS>::new(),
        shown: Vec::new(),
        lists: Vec::new(),
        rows: Vec::new(),
        parent: None,
        generation: u64::MAX,
    };
    content.rebuild_if_changed();
    match application::run(&mut window, &mut content) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Runs the app whose `classes.dex` the kernel names as the first argument.
pub fn run(startup: &Startup) -> i32 {
    let Some(code_path) = startup.arguments().nth(1) else {
        stop(&[b"usage: android <app folder>/classes.dex"]);
    };
    let code_path = code_path.to_bytes();
    let folder = code_path
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(&b"."[..], |slash| &code_path[..slash]);
    let manifest_path = [folder, b"/", MANIFEST_NAME].concat();
    let manifest = read_file(&manifest_path).and_then(|bytes| Manifest::parse(&bytes));
    let Some(manifest) = manifest else {
        stop(&[
            b"android: no readable AndroidManifest.xml beside ",
            code_path,
        ]);
    };
    let Some(launcher) = manifest.launcher_activity.as_deref() else {
        stop(&[b"android: the app names no launcher activity"]);
    };
    let activity = framework::activity_descriptor(&manifest.package, launcher);
    let Some(bytes) = read_file(code_path) else {
        stop(&[b"android: cannot read ", code_path]);
    };
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let Ok(dex) = Dex::parse(bytes) else {
        stop(&[b"android: ", code_path, b" is not a DEX file"]);
    };
    let resources = [RESOURCES_NAME, BUNDLE_RESOURCES_NAME]
        .iter()
        .find_map(|name| read_file(&[folder, b"/", name].concat()))
        .and_then(|bytes| ResourceTable::parse(&bytes))
        .map(|mut table| {
            table.set_device(device_configuration(startup));
            table
        });
    let (code, compiled) = compile_all(&dex);
    let Some(base) = executable(&code) else {
        stop(&[b"android: no executable memory for the app's code"]);
    };
    let targets: Vec<Target> = (0..dex.method_count())
        .map(|method| framework::resolve(&dex, method, &compiled))
        .collect();
    let initial = State {
        dex,
        content: None,
        base,
        compiled,
        statics: BTreeMap::new(),
        initialized: BTreeSet::new(),
        dispatch: BTreeMap::new(),
        resources,
        images: Vec::new(),
        folder: Vec::from(folder),
        activity: 0,
        table: 0,
        heap: collector::Heap::default(),
        code_length: code.len(),
        failed_classes: BTreeSet::new(),
        depth: 0,
        placed_images: Vec::new(),
        scaled_images: BTreeMap::new(),
        below: Vec::new(),
        fragments: Vec::new(),
        generation: 0,
        request: None,
        result: (0, 0),
        lists: BTreeMap::new(),
        stack_top: stack_pointer().saturating_add(STACK_ABOVE_RUN),
    };
    STATE.store(Box::leak(Box::new(initial)), Ordering::Release);
    let mut entries: Vec<usize> = helpers().iter().map(|&entry| entry as usize).collect();
    entries.extend(targets.into_iter().map(target_address));
    entries[helper::PENDING_EXCEPTION] = 0;
    let entries: &'static mut [usize] = Box::leak(entries.into_boxed_slice());
    state().table = entries.as_mut_ptr() as usize;
    let Some(class) = dex.class_index(&activity) else {
        stop(&[
            b"android: the app does not define its activity ",
            activity.as_bytes(),
        ]);
    };
    if dex
        .superclass(class)
        .is_none_or(|parent| !ACTIVITY_CLASSES.contains(&parent.as_str()))
        && method_named(&dex, class, "onCreate").is_none()
    {
        stop(&[b"android: ", activity.as_bytes(), b" is not an activity"]);
    }
    initialize(&activity, table());
    report_if_uncaught();
    let this = new_object(Object {
        class: activity.clone(),
        ..Object::default()
    });
    state().activity = this;
    if let Some(init) = method_named(&dex, class, "<init>") {
        call(init, &[this]);
    }
    if let Some(on_create) = method_named(&dex, class, "onCreate") {
        call(on_create, &[this, 0]);
    }
    show(&manifest.package)
}
