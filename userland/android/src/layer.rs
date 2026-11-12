//! The layer's run on x86_64 (D472): the app's files read, its methods compiled into one block
//! of memory made executable, the entry table filled (the runtime's helpers, then one entry
//! per method of the app's DEX file: compiled code, a framework native, or the missing-method
//! stop), the launcher activity created and its `<init>` and `onCreate` called through the
//! table, and the text view it set shown in a window. Objects are boxes the layer keeps for
//! the app's life; the app runs on this one thread.

use crate::framework::{self, ACTIVITY_CLASSES, Native, Object, STRING_CLASS, Target};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};
use eggos_android_package::dex::Dex;
use eggos_android_package::manifest::Manifest;
use eggos_dex_compiler::{self as compiler, helper, method_entry};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application::{self, Application};
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetTree};
use eggos_toolkit::widget::{Kind, Length, Place};
use eggos_toolkit::window::Window;

type Entry = extern "sysv64" fn(*const u64, *const usize) -> u64;

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
const WINDOW_SIZE: (u32, u32) = (360, 640);
const PADDING: i32 = 16;

struct State {
    dex: Dex<'static>,
    content: Option<u64>,
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
            [AT_CURRENT_FOLDER, named.as_ptr() as usize, READ_ONLY, 0, 0, 0],
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
                [descriptor, chunk.as_mut_ptr() as usize, chunk.len(), 0, 0, 0],
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
    // SAFETY: changes the protection of the mapping made above, which nothing writes after.
    let protected = unsafe {
        syscall::call(
            number::MPROTECT,
            [base, length, PROTECT_READ | PROTECT_EXECUTE, 0, 0, 0],
        )
    };
    (protected == 0).then_some(base)
}

fn new_object(object: Object) -> u64 {
    Box::leak(Box::new(object)) as *mut Object as u64
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

extern "sysv64" fn string(words: *const u64, _: *const usize) -> u64 {
    let [index] = arguments::<1>(words);
    let text = state().dex.string(index as u32);
    new_object(Object {
        class: String::from(STRING_CLASS),
        text,
        ..Object::default()
    })
}

extern "sysv64" fn new_instance(words: *const u64, _: *const usize) -> u64 {
    let [index] = arguments::<1>(words);
    let class = state().dex.type_name(index as u32).unwrap_or_default();
    new_object(Object {
        class,
        ..Object::default()
    })
}

extern "sysv64" fn field_get(words: *const u64, _: *const usize) -> u64 {
    let [target, field] = arguments::<2>(words);
    let Some(target) = object(target) else {
        stop(&[b"android: NullPointerException reading a field"]);
    };
    target.fields.get(&(field as u32)).copied().unwrap_or(0)
}

extern "sysv64" fn field_put(words: *const u64, _: *const usize) -> u64 {
    let [target, field, value] = arguments::<3>(words);
    let Some(target) = object(target) else {
        stop(&[b"android: NullPointerException writing a field"]);
    };
    target.fields.insert(field as u32, value);
    0
}

fn divisor_of(word: u64) -> u64 {
    match word {
        0 => stop(&[b"android: ArithmeticException: divide by zero"]),
        word => word,
    }
}

extern "sysv64" fn divide_int(words: *const u64, _: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    let divisor = divisor_of(u64::from(right as u32)) as u32 as i32;
    (left as u32 as i32).wrapping_div(divisor) as u32 as u64
}

extern "sysv64" fn remainder_int(words: *const u64, _: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    let divisor = divisor_of(u64::from(right as u32)) as u32 as i32;
    (left as u32 as i32).wrapping_rem(divisor) as u32 as u64
}

extern "sysv64" fn divide_long(words: *const u64, _: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    (left as i64).wrapping_div(divisor_of(right) as i64) as u64
}

extern "sysv64" fn remainder_long(words: *const u64, _: *const usize) -> u64 {
    let [left, right] = arguments::<2>(words);
    (left as i64).wrapping_rem(divisor_of(right) as i64) as u64
}

extern "sysv64" fn missing_method(_: *const u64, _: *const usize) -> u64 {
    stop(&[b"android: the app called a method the Android layer does not provide yet"])
}

extern "sysv64" fn nothing(_: *const u64, _: *const usize) -> u64 {
    0
}

extern "sysv64" fn set_content_view(words: *const u64, _: *const usize) -> u64 {
    let [_, view] = arguments::<2>(words);
    state().content = Some(view);
    0
}

extern "sysv64" fn text_view_init(words: *const u64, _: *const usize) -> u64 {
    let [view, _] = arguments::<2>(words);
    if let Some(view) = object(view) {
        view.text = Some(String::new());
    }
    0
}

extern "sysv64" fn set_text(words: *const u64, _: *const usize) -> u64 {
    let [view, text] = arguments::<2>(words);
    let text = object(text).and_then(|text| text.text.clone()).unwrap_or_default();
    if let Some(view) = object(view) {
        view.text = Some(text);
    }
    0
}

fn native_entry(native: Native) -> Entry {
    match native {
        Native::Nothing => nothing,
        Native::SetContentView => set_content_view,
        Native::TextViewInit => text_view_init,
        Native::SetText => set_text,
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
    entries
}

fn compile_all(dex: &Dex<'_>) -> (Vec<u8>, BTreeMap<u32, usize>) {
    let mut code = Vec::new();
    let mut compiled = BTreeMap::new();
    for class in 0..dex.class_count() {
        for method in dex.methods(class) {
            let Some(body) = method.code else {
                continue;
            };
            match compiler::compile(&body) {
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

fn call(table: &[usize], method: u32, words: &[u64]) -> u64 {
    let address = table[method_entry(method)];
    // SAFETY: every table entry is a compiled method in the executable block or one of the
    // layer's `extern "sysv64"` functions, all of type `Entry`.
    let entry: Entry = unsafe { core::mem::transmute::<usize, Entry>(address) };
    entry(words.as_ptr(), table.as_ptr())
}

struct ContentWindow {
    tree: WidgetTree<4>,
}

impl Application<4> for ContentWindow {
    fn tree(&mut self) -> &mut WidgetTree<4> {
        &mut self.tree
    }

    fn respond(&mut self, _response: Response, _now_ms: u32) {}
}

fn show(title: &str, text: &str) -> i32 {
    let Ok(mut window) = Window::create(title, WINDOW_SIZE.0, WINDOW_SIZE.1, (160, 120)) else {
        stop(&[b"android: no window for the app"]);
    };
    let mut tree = WidgetTree::<4>::new();
    let place = Place::at(PADDING, PADDING, Length::Fill(PADDING), Length::Content);
    let _ = tree.add(None, place, Kind::label(text, Token::Text));
    let mut content = ContentWindow { tree };
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
        stop(&[b"android: no readable AndroidManifest.xml beside ", code_path]);
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
    let (code, compiled) = compile_all(&dex);
    let Some(base) = executable(&code) else {
        stop(&[b"android: no executable memory for the app's code"]);
    };
    let mut table: Vec<usize> = helpers().iter().map(|&entry| entry as usize).collect();
    for method in 0..dex.method_count() {
        table.push(match framework::resolve(&dex, method, &compiled) {
            Target::Compiled(offset) => base + offset,
            Target::Native(native) => native_entry(native) as usize,
            Target::Missing => missing_method as usize,
        });
    }
    STATE.store(Box::leak(Box::new(State { dex, content: None })), Ordering::Release);
    let Some(class) = dex.class_index(&activity) else {
        stop(&[b"android: the app does not define its activity ", activity.as_bytes()]);
    };
    if dex.superclass(class).is_none_or(|parent| !ACTIVITY_CLASSES.contains(&parent.as_str()))
        && method_named(&dex, class, "onCreate").is_none()
    {
        stop(&[b"android: ", activity.as_bytes(), b" is not an activity"]);
    }
    let this = new_object(Object {
        class: activity.clone(),
        ..Object::default()
    });
    if let Some(init) = method_named(&dex, class, "<init>") {
        call(&table, init, &[this]);
    }
    if let Some(on_create) = method_named(&dex, class, "onCreate") {
        call(&table, on_create, &[this, 0]);
    }
    let text = state()
        .content
        .and_then(object)
        .and_then(|view| view.text.clone())
        .unwrap_or_default();
    show(&manifest.package, &text)
}
