//! The layer's run on x86_64 (D472): the app's files read, its methods compiled into one block
//! of memory made executable, the entry table filled (the runtime's helpers, then one entry
//! per method of the app's DEX file: compiled code, a framework native, or the missing-method
//! stop), the launcher activity created and its `<init>` and `onCreate` called through the
//! table, and the text view it set shown in a window. Objects are boxes the layer keeps for
//! the app's life; the app runs on this one thread. Virtual calls are dispatched on the
//! receiver's class, each class's static initializer runs on its first static field or new
//! object, arrays are objects with elements, and an uncaught `throw` stops the app with the
//! exception's class (D499). `setContentView(R.layout.…)` inflates the compiled layout the
//! resource table names into framework views, and the window shows them as labels and buttons
//! whose clicks call the activity's `android:onClick` method or the view's click listener,
//! then shows the texts they changed (D500).

use crate::framework::{
    self, ACTIVITY_CLASSES, BUTTON, LINEAR_LAYOUT, Native, Object, STRING_CLASS, TEXT_VIEWS, Target,
    View,
};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};
use eggos_android_package::dex::Dex;
use eggos_android_package::layout::{self, Text, ViewNode};
use eggos_android_package::manifest::Manifest;
use eggos_android_package::resources::ResourceTable;
use eggos_dex_compiler::{self as compiler, helper, method_entry};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application::{self, Application};
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
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
const RESOURCES_NAME: &[u8] = b"resources.arsc";
const MOST_WIDGETS: usize = 64;
const ROW: i32 = 44;
const COLUMN: i32 = 160;
const DEEPEST_VIEW: usize = 32;
const WINDOW_SIZE: (u32, u32) = (360, 640);
const PADDING: i32 = 16;

struct State {
    dex: Dex<'static>,
    content: Option<u64>,
    base: usize,
    compiled: BTreeMap<u32, usize>,
    statics: BTreeMap<u32, u64>,
    initialized: BTreeSet<String>,
    dispatch: BTreeMap<(String, u32), usize>,
    resources: Option<ResourceTable>,
    folder: Vec<u8>,
    activity: u64,
    table: usize,
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

extern "sysv64" fn new_instance(words: *const u64, table: *const usize) -> u64 {
    let [index] = arguments::<1>(words);
    let class = state().dex.type_name(index as u32).unwrap_or_default();
    initialize(&class, table);
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

fn entry_at(address: usize) -> Entry {
    // SAFETY: every address the layer calls is a compiled method in the executable block or
    // one of the layer's `extern "sysv64"` functions, all of type `Entry`.
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
}

extern "sysv64" fn invoke_virtual(words: *const u64, table: *const usize) -> u64 {
    let [method, receiver] = arguments::<2>(words);
    let Some(class) = object(receiver).map(|receiver| receiver.class.clone()) else {
        stop(&[b"android: NullPointerException calling a method"]);
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

extern "sysv64" fn static_get(words: *const u64, table: *const usize) -> u64 {
    let [field] = arguments::<1>(words);
    static_field_class(field, table);
    state().statics.get(&(field as u32)).copied().unwrap_or(0)
}

extern "sysv64" fn static_put(words: *const u64, table: *const usize) -> u64 {
    let [field, value] = arguments::<2>(words);
    static_field_class(field, table);
    state().statics.insert(field as u32, value);
    0
}

const LARGEST_ARRAY: usize = 1 << 24;

extern "sysv64" fn new_array(words: *const u64, _: *const usize) -> u64 {
    let [length, index] = arguments::<2>(words);
    let length = length as u32 as i32;
    if length < 0 {
        stop(&[b"android: NegativeArraySizeException"]);
    }
    if length as usize > LARGEST_ARRAY {
        stop(&[b"android: OutOfMemoryError: array too large"]);
    }
    let class = state().dex.type_name(index as u32).unwrap_or_default();
    new_object(Object {
        class,
        elements: Some(vec![0; length as usize]),
        ..Object::default()
    })
}

fn elements_of(array: u64) -> &'static mut Object {
    match object(array) {
        Some(array) if array.elements.is_some() => array,
        Some(_) => stop(&[b"android: ClassCastException: not an array"]),
        None => stop(&[b"android: NullPointerException using an array"]),
    }
}

fn element_index(array: &Object, index: u64) -> usize {
    let length = array.elements.as_ref().map_or(0, Vec::len);
    match usize::try_from(index as u32 as i32) {
        Ok(index) if index < length => index,
        _ => stop(&[b"android: ArrayIndexOutOfBoundsException"]),
    }
}

extern "sysv64" fn array_length(words: *const u64, _: *const usize) -> u64 {
    let [array] = arguments::<1>(words);
    elements_of(array).elements.as_ref().map_or(0, Vec::len) as u64
}

extern "sysv64" fn array_get(words: *const u64, _: *const usize) -> u64 {
    let [array, index] = arguments::<2>(words);
    let array = elements_of(array);
    let index = element_index(array, index);
    array.elements.as_ref().map_or(0, |elements| elements[index])
}

extern "sysv64" fn array_put(words: *const u64, _: *const usize) -> u64 {
    let [array, index, value] = arguments::<3>(words);
    let array = elements_of(array);
    let index = element_index(array, index);
    let value = framework::array_element(&array.class, value);
    if let Some(elements) = array.elements.as_mut() {
        elements[index] = value;
    }
    0
}

extern "sysv64" fn throw(words: *const u64, _: *const usize) -> u64 {
    let [exception] = arguments::<1>(words);
    let Some(exception) = object(exception) else {
        stop(&[b"android: NullPointerException thrown"]);
    };
    stop(&[b"android: uncaught ", exception.class.as_bytes()])
}

fn class_of(index: u64) -> String {
    state().dex.type_name(index as u32).unwrap_or_default()
}

extern "sysv64" fn check_cast(words: *const u64, _: *const usize) -> u64 {
    let [value, index] = arguments::<2>(words);
    if let Some(value) = object(value) {
        let wanted = class_of(index);
        if framework::is_instance(&state().dex, &value.class, &wanted) == Some(false) {
            let class = value.class.as_bytes();
            stop(&[b"android: ClassCastException: ", class, b" is no ", wanted.as_bytes()]);
        }
    }
    0
}

extern "sysv64" fn instance_of(words: *const u64, _: *const usize) -> u64 {
    let [value, index] = arguments::<2>(words);
    let Some(value) = object(value) else {
        return 0;
    };
    let wanted = class_of(index);
    u64::from(framework::is_instance(&state().dex, &value.class, &wanted) == Some(true))
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
        view.view = Some(View::default());
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

fn resource_string(id: u64) -> Option<String> {
    let state = state();
    state.resources.as_ref()?.string(id as u32).map(String::from)
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
        true => node.children.iter().map(|child| inflate_node(child, depth + 1)).collect(),
        false => Vec::new(),
    };
    let text = text_of(&node.text).or_else(|| shows_text.then(|| text_of(&node.hint)).flatten());
    new_object(Object {
        class,
        text: text.or_else(|| shows_text.then(String::new)),
        view: Some(View {
            id: node.id.unwrap_or(0),
            children,
            vertical: node.vertical || node.element != "LinearLayout",
            on_click: node.on_click.clone(),
            listener: 0,
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

extern "sysv64" fn set_content_layout(words: *const u64, _: *const usize) -> u64 {
    let [_, layout_id] = arguments::<2>(words);
    let root = inflate(u64::from(layout_id as u32));
    state().content = Some(root);
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
    children.into_iter().find_map(|child| find_view(child, id, depth + 1))
}

extern "sysv64" fn find_view_by_id(words: *const u64, _: *const usize) -> u64 {
    let [this, id] = arguments::<2>(words);
    let is_view = object(this).is_some_and(|this| this.view.is_some());
    let root = match is_view {
        true => Some(this),
        false => state().content,
    };
    root.and_then(|root| find_view(root, id as u32, 0)).unwrap_or(0)
}

extern "sysv64" fn view_init(words: *const u64, _: *const usize) -> u64 {
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

extern "sysv64" fn set_on_click_listener(words: *const u64, _: *const usize) -> u64 {
    let [view, listener] = arguments::<2>(words);
    if let Some(view) = object(view).and_then(|view| view.view.as_mut()) {
        view.listener = listener;
    }
    0
}

extern "sysv64" fn set_text_resource(words: *const u64, _: *const usize) -> u64 {
    let [view, id] = arguments::<2>(words);
    let text = resource_string(u64::from(id as u32)).unwrap_or_default();
    if let Some(view) = object(view) {
        view.text = Some(text);
    }
    0
}

extern "sysv64" fn get_text(words: *const u64, _: *const usize) -> u64 {
    let [view] = arguments::<1>(words);
    let text = object(view).and_then(|view| view.text.clone()).unwrap_or_default();
    new_string(text)
}

extern "sysv64" fn get_string(words: *const u64, _: *const usize) -> u64 {
    let [_, id] = arguments::<2>(words);
    new_string(resource_string(u64::from(id as u32)).unwrap_or_default())
}

extern "sysv64" fn add_view(words: *const u64, _: *const usize) -> u64 {
    let [group, child] = arguments::<2>(words);
    if let Some(group) = object(group).and_then(|group| group.view.as_mut()) {
        group.children.push(child);
    }
    0
}

extern "sysv64" fn set_orientation(words: *const u64, _: *const usize) -> u64 {
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
        Native::AddView => add_view,
        Native::SetOrientation => set_orientation,
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
    entry_at(table[method_entry(method)])(words.as_ptr(), table.as_ptr())
}

fn compiled_method(class: &str, name: &str) -> Option<usize> {
    let state = state();
    let mut class = String::from(class);
    for _ in 0..DEEPEST_VIEW {
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
        let words = [receiver, view];
        entry_at(address)(words.as_ptr(), state().table as *const usize);
    }
}

struct ContentWindow {
    tree: WidgetTree<MOST_WIDGETS>,
    shown: Vec<(WidgetId, u64)>,
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
        let Some(text) = text else {
            return 0;
        };
        let kind = match class.as_str() {
            BUTTON => Kind::button(&text, false),
            _ => Kind::label(&text, Token::Text),
        };
        let place = Place::at(x, y, Length::Content, Length::Content);
        if let Some(widget) = self.tree.add(None, place, kind) {
            self.shown.push((widget, view));
        }
        ROW
    }

    fn refresh(&mut self) {
        for &(widget, view) in &self.shown {
            if let Some(text) = object(view).and_then(|view| view.text.clone()) {
                self.tree.set_text(widget, &text);
            }
        }
    }
}

impl Application<MOST_WIDGETS> for ContentWindow {
    fn tree(&mut self) -> &mut WidgetTree<MOST_WIDGETS> {
        &mut self.tree
    }

    fn respond(&mut self, response: Response, _now_ms: u32) {
        if let Response::Activated(widget) = response
            && let Some(&(_, view)) = self.shown.iter().find(|(shown, _)| *shown == widget)
        {
            click(view);
            self.refresh();
        }
    }
}

fn show(title: &str) -> i32 {
    let Ok(mut window) = Window::create(title, WINDOW_SIZE.0, WINDOW_SIZE.1, (160, 120)) else {
        stop(&[b"android: no window for the app"]);
    };
    let mut content = ContentWindow {
        tree: WidgetTree::<MOST_WIDGETS>::new(),
        shown: Vec::new(),
    };
    if let Some(root) = state().content {
        content.add(root, PADDING, PADDING, 0);
    }
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
    let resources_path = [folder, b"/", RESOURCES_NAME].concat();
    let resources = read_file(&resources_path).and_then(|bytes| ResourceTable::parse(&bytes));
    let (code, compiled) = compile_all(&dex);
    let Some(base) = executable(&code) else {
        stop(&[b"android: no executable memory for the app's code"]);
    };
    let targets: Vec<Target> = (0..dex.method_count())
        .map(|method| framework::resolve(&dex, method, &compiled))
        .collect();
    let state = State {
        dex,
        content: None,
        base,
        compiled,
        statics: BTreeMap::new(),
        initialized: BTreeSet::new(),
        dispatch: BTreeMap::new(),
        resources,
        folder: Vec::from(folder),
        activity: 0,
        table: 0,
    };
    STATE.store(Box::leak(Box::new(state)), Ordering::Release);
    let mut table: Vec<usize> = helpers().iter().map(|&entry| entry as usize).collect();
    table.extend(targets.into_iter().map(target_address));
    let table: &'static [usize] = Box::leak(table.into_boxed_slice());
    state().table = table.as_ptr() as usize;
    let Some(class) = dex.class_index(&activity) else {
        stop(&[b"android: the app does not define its activity ", activity.as_bytes()]);
    };
    if dex.superclass(class).is_none_or(|parent| !ACTIVITY_CLASSES.contains(&parent.as_str()))
        && method_named(&dex, class, "onCreate").is_none()
    {
        stop(&[b"android: ", activity.as_bytes(), b" is not an activity"]);
    }
    initialize(&activity, table.as_ptr());
    let this = new_object(Object {
        class: activity.clone(),
        ..Object::default()
    });
    state().activity = this;
    if let Some(init) = method_named(&dex, class, "<init>") {
        call(table, init, &[this]);
    }
    if let Some(on_create) = method_named(&dex, class, "onCreate") {
        call(table, on_create, &[this, 0]);
    }
    show(&manifest.package)
}
