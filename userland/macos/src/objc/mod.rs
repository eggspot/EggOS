//! The Objective-C runtime, libobjc (D142): selectors, classes and categories from the image's
//! metadata, `objc_msgSend` and its kin in assembly, reference counting, autorelease pools,
//! ARC's entry points and NSObject; blocks, weak references and exceptions with their unwinder
//! (D175). It is set up only for a program that uses Objective-C.

pub mod abi;
pub mod cache;
pub mod lsda;
pub mod selector;
pub mod unwind_info;

#[cfg(target_env = "musl")]
macro_rules! sel {
    ($name:literal) => {{
        static CANONICAL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        let mut value = CANONICAL.load(core::sync::atomic::Ordering::Relaxed);
        if value == 0 {
            value = $crate::objc::register_selector(concat!($name, "\0").as_ptr())
                .map_or(0, |sel| sel as usize);
            CANONICAL.store(value, core::sync::atomic::Ordering::Relaxed);
        }
        value as $crate::objc::abi::Sel
    }};
}

#[cfg(target_env = "musl")]
pub(crate) use sel;

#[cfg(target_env = "musl")]
macro_rules! methods {
    ($($name:literal => $imp:path),* $(,)?) => {
        [$((concat!($name, "\0").as_bytes(), $imp as *const () as usize)),*]
    };
}

#[cfg(target_env = "musl")]
pub(crate) use methods;

#[cfg(target_env = "musl")]
mod api;
#[cfg(target_env = "musl")]
pub mod autorelease;
#[cfg(target_env = "musl")]
pub mod block;
#[cfg(target_env = "musl")]
pub mod buffer;
#[cfg(target_env = "musl")]
pub mod class;
#[cfg(target_env = "musl")]
pub mod exception;
#[cfg(target_env = "musl")]
pub mod message;
#[cfg(target_env = "musl")]
pub mod reference_count;
#[cfg(target_env = "musl")]
pub mod root;
#[cfg(target_env = "musl")]
pub mod weak;

#[cfg(target_env = "musl")]
pub use runtime::*;

#[cfg(target_env = "musl")]
mod runtime {
    use super::abi::{Category, ClassObject, Id, Sel};
    use super::buffer::Buffer;
    use super::class::{self, ClassRecord};
    use super::selector::SelectorTable;
    use super::{api, autorelease, block, exception, message, reference_count, root, weak};
    use crate::libsystem::{Exclusive, heap, stdio, thread, write_error};
    use crate::macho::{ObjcSections, SectionRange};
    use core::sync::atomic::{AtomicUsize, Ordering};

    /// The runtime's state behind its lock.
    #[derive(Debug)]
    pub struct Runtime {
        pub classes: Buffer<*mut ClassRecord>,
        load_lists: [Option<(usize, usize)>; 2],
    }

    // SAFETY: the records the table points at are the runtime's, reached under its lock.
    unsafe impl Send for Runtime {}

    pub static RUNTIME: Exclusive<Runtime> = Exclusive::new(Runtime {
        classes: Buffer::new(),
        load_lists: [None; 2],
    });

    static SELECTORS: Exclusive<SelectorTable> = Exclusive::new(SelectorTable::new());

    /// The canonical selector of the permanent NUL-terminated name at `name`.
    pub fn register_selector(name: *const u8) -> Option<Sel> {
        if name.is_null() {
            return None;
        }
        SELECTORS.with(|table| {
            let mut allocate = |capacity: usize| -> *mut usize {
                heap::allocate(capacity * core::mem::size_of::<usize>(), true).cast()
            };
            // SAFETY: the callers pass names from the mapped image or the layer's statics.
            unsafe { table.register(name, &mut allocate) }
        })
    }

    /// The canonical selector of `name` (no NUL), copying the name if it is new.
    pub fn selector_named(name: &[u8]) -> Option<Sel> {
        if let Some(found) = SELECTORS.with(|table| table.find(name)) {
            return Some(found);
        }
        let copy = heap::allocate(name.len() + 1, true);
        if copy.is_null() {
            return None;
        }
        // SAFETY: the new block holds the name and its NUL; it is never freed.
        unsafe { copy.copy_from_nonoverlapping(name.as_ptr(), name.len()) };
        register_selector(copy)
    }

    /// Sends `sel` to `receiver` with no arguments.
    pub fn send(receiver: Id, sel: Sel) -> Id {
        let function: extern "C" fn(Id, Sel) -> Id = message::msg_send_as();
        function(receiver, sel)
    }

    /// Sends `sel` to `receiver` with one word argument.
    pub fn send1(receiver: Id, sel: Sel, argument: usize) -> Id {
        let function: extern "C" fn(Id, Sel, usize) -> Id = message::msg_send_as();
        function(receiver, sel, argument)
    }

    /// Sends `sel` to `receiver` with two word arguments.
    pub fn send2(receiver: Id, sel: Sel, first: usize, second: usize) -> Id {
        let function: extern "C" fn(Id, Sel, usize, usize) -> Id = message::msg_send_as();
        function(receiver, sel, first, second)
    }

    /// Sends `sel` to `receiver` with one word argument and reads a `BOOL` back.
    pub fn send_bool(receiver: Id, sel: Sel, argument: usize) -> bool {
        let function: extern "C" fn(Id, Sel, usize) -> u8 = message::msg_send_as();
        function(receiver, sel, argument) != 0
    }

    /// Sets the runtime up: the root classes and the thread-exit hook; false if there is no
    /// memory.
    pub fn set_up() -> bool {
        thread::set_finish_hook(autorelease::drain_thread);
        root::define() && RUNTIME.with(block::define)
    }

    /// The address of the libobjc function or variable `name`, if the runtime has it.
    pub fn resolve(name: &[u8]) -> Option<u64> {
        root::resolve(name)
            .or_else(|| api::resolve(name))
            .or_else(|| reference_count::resolve(name))
            .or_else(|| message::resolve(name))
            .or_else(|| autorelease::resolve(name))
            .or_else(|| exception::resolve(name))
            .or_else(|| block::resolve(name))
            .or_else(|| weak::resolve(name))
    }

    const CLASS_PREFIX: &[u8] = b"_OBJC_CLASS_$_";
    const METACLASS_PREFIX: &[u8] = b"_OBJC_METACLASS_$_";

    /// Whether `name` is the symbol of a class or metaclass object.
    pub fn is_class_symbol(name: &[u8]) -> bool {
        name.starts_with(CLASS_PREFIX) || name.starts_with(METACLASS_PREFIX)
    }

    /// How many classes a program may import that EggOS lacks.
    pub const MISSING_CLASSES: usize = 32;

    static MISSING_PAIRS: [[ClassObject; 2]; MISSING_CLASSES] =
        [const { [ClassObject::EMPTY, ClassObject::EMPTY] }; MISSING_CLASSES];
    static MISSING_USED: AtomicUsize = AtomicUsize::new(0);

    /// The address of a placeholder for the class or metaclass symbol `symbol` from `library`,
    /// which ends the program, naming the class, when it is first used; `None` once
    /// [`MISSING_CLASSES`] are taken. `symbol` must be permanent and NUL-terminated after it.
    pub fn bind_missing_class(library: &'static [u8], symbol: &'static [u8]) -> Option<u64> {
        let (meta, name) = match symbol.strip_prefix(METACLASS_PREFIX) {
            Some(name) => (true, name),
            None => (false, symbol.strip_prefix(CLASS_PREFIX)?),
        };
        let used = MISSING_USED.load(Ordering::Relaxed);
        let existing = MISSING_PAIRS[..used].iter().position(|pair| {
            class::record_of(&pair[0]).is_some_and(|record| record.name() == name)
        });
        let index = match existing {
            Some(index) => index,
            None => {
                if used == MISSING_CLASSES {
                    return None;
                }
                let [class, metaclass] = &MISSING_PAIRS[used];
                class.isa.store(
                    (metaclass as *const ClassObject).cast_mut(),
                    Ordering::Relaxed,
                );
                metaclass
                    .isa
                    .store(root::root_metaclass(), Ordering::Relaxed);
                let made = RUNTIME.with(|runtime| {
                    class::register_pair(
                        runtime,
                        (class as *const ClassObject).cast_mut(),
                        name.as_ptr(),
                        class::MISSING,
                        library,
                    )
                    .is_some()
                });
                if !made {
                    return None;
                }
                MISSING_USED.store(used + 1, Ordering::Relaxed);
                used
            }
        };
        Some(&MISSING_PAIRS[index][usize::from(meta)] as *const ClassObject as u64)
    }

    /// Ends the program because it used a class EggOS lacks.
    pub fn report_missing_class(record: &ClassRecord) -> ! {
        stdio::flush_all();
        write_error(b"eggos-macos: unimplemented ");
        write_error(record.library);
        write_error(b" class ");
        write_error(record.name());
        write_error(b"\n");
        crate::linux::exit(127)
    }

    fn words(range: Option<SectionRange>, slide: isize) -> &'static [usize] {
        let Some(range) = range else { return &[] };
        let start = (range.address as usize).wrapping_add_signed(slide) as *const usize;
        // SAFETY: the loader checked the section lies inside a mapped segment and is a whole
        // number of aligned words.
        unsafe { core::slice::from_raw_parts(start, range.size as usize / 8) }
    }

    /// Registers the image's selectors, classes and categories: the selector references are
    /// made canonical in place, every class gets its records, every category is attached.
    ///
    /// # Safety
    /// The image must be mapped, bound and rebased by `slide`, still writable, with no
    /// program code run yet, and `sections` must be the loader's checked sections.
    pub unsafe fn register_image(
        slide: isize,
        sections: &ObjcSections,
    ) -> Result<(), crate::macho::InvalidMachO> {
        let out_of_memory = crate::macho::InvalidMachO("out of memory for the Objective-C runtime");
        if let Some(range) = sections.selector_references {
            let start = (range.address as usize).wrapping_add_signed(slide) as *mut usize;
            for index in 0..range.size as usize / 8 {
                let slot = start.wrapping_add(index);
                // SAFETY: the loader checked the section is whole aligned words inside a
                // mapped segment, still writable before `protect`.
                let name = unsafe { slot.read() } as *const u8;
                let canonical = register_selector(name).ok_or(out_of_memory)?;
                // SAFETY: as above.
                unsafe { slot.write(canonical as usize) };
            }
        }
        RUNTIME.with(|runtime| {
            for &class in words(sections.class_list, slide) {
                let class = class as *mut ClassObject;
                if class::record_of(class).is_some() {
                    continue;
                }
                // SAFETY: a class of the image's class list, mapped.
                let read_only = unsafe { &*class }.read_only();
                let name = if read_only.is_null() {
                    core::ptr::null()
                } else {
                    // SAFETY: its `class_ro_t`, mapped.
                    unsafe { &*read_only }.name
                };
                class::register_pair(runtime, class, name, 0, b"").ok_or(out_of_memory)?;
            }
            for &category in words(sections.category_list, slide) {
                // SAFETY: a category of the image's category list, mapped.
                let category = unsafe { &*(category as *const Category) };
                let Some(record) = class::record_of(category.class) else {
                    continue;
                };
                class::attach_list(runtime, record, category.instance_methods);
                class::attach_list(runtime, record.pair(), category.class_methods);
            }
            runtime.load_lists = [
                sections
                    .non_lazy_class_list
                    .map(|range| (range.address as usize, range.size as usize)),
                sections
                    .non_lazy_category_list
                    .map(|range| (range.address as usize, range.size as usize)),
            ]
            .map(|list| list.map(|(address, size)| (address.wrapping_add_signed(slide), size)));
            Ok(())
        })
    }

    fn call_load(imp: usize, class: *mut ClassObject) {
        let imp = crate::callback::native_callable(imp);
        // SAFETY: a `+load` method, which takes the class and its selector.
        let load: extern "C" fn(Id, Sel) = unsafe { core::mem::transmute(imp) };
        load(class.cast(), sel!("load"));
    }

    fn load_class(record: &'static ClassRecord) {
        if record.has(class::LOAD_CALLED) || record.has(class::BUILTIN) {
            return;
        }
        record.flags.fetch_or(class::LOAD_CALLED, Ordering::Relaxed);
        if let Some(superclass) = record.superclass() {
            load_class(superclass);
        }
        RUNTIME.with(|runtime| class::realize(runtime, record));
        if let Some(imp) = class::own_load_method(record) {
            call_load(imp, record.class);
        }
    }

    /// Calls `+load` on the image's non-lazy classes, superclasses first, then on its non-lazy
    /// categories, inside an autorelease pool, as dyld does before `main`.
    pub fn run_load_methods() {
        let lists = RUNTIME.with(|runtime| runtime.load_lists);
        let words_of = |list: Option<(usize, usize)>| -> &'static [usize] {
            let Some((address, size)) = list else {
                return &[];
            };
            // SAFETY: as in `words`: a checked, slid section.
            unsafe { core::slice::from_raw_parts(address as *const usize, size / 8) }
        };
        let pool = autorelease::push();
        for &class in words_of(lists[0]) {
            if let Some(record) = class::record_of(class as *const ClassObject) {
                load_class(record);
            }
        }
        for &category in words_of(lists[1]) {
            // SAFETY: a category of the image's non-lazy category list, mapped.
            let category = unsafe { &*(category as *const Category) };
            let Some(record) = class::record_of(category.class) else {
                continue;
            };
            load_class(record);
            RUNTIME.with(|runtime| class::realize(runtime, record));
            if let Some(imp) = class::load_in(category.class_methods) {
                call_load(imp, category.class);
            }
        }
        autorelease::pop(pool);
    }

    /// Ends the program as an uncaught Objective-C exception does on macOS: its name and
    /// reason, the C++ runtime's line, then `abort`. `reason` is written in parts.
    pub fn terminate_uncaught(name: &[u8], reason: &[&[u8]]) -> ! {
        stdio::flush_all();
        write_error(b"*** Terminating app due to uncaught exception '");
        write_error(name);
        write_error(b"', reason: '");
        for part in reason {
            write_error(part);
        }
        write_error(b"'\nlibc++abi: terminating due to uncaught exception of type NSException\n");
        crate::libsystem::abort()
    }

    /// `0x` and the address in lowercase hexadecimal, as `%p` prints it.
    pub fn hexadecimal(value: usize, digits: &mut [u8; 18]) -> &[u8] {
        digits[0] = b'0';
        digits[1] = b'x';
        let mut length = 2;
        let mut started = false;
        for shift in (0..16).rev() {
            let nibble = (value >> (shift * 4)) & 0xf;
            if nibble != 0 || started || shift == 0 {
                started = true;
                digits[length] = b"0123456789abcdef"[nibble];
                length += 1;
            }
        }
        &digits[..length]
    }

    pub use super::api::class_named;
}
