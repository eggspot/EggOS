//! The runtime's record of each class (`ClassRecord`), kept in the class object's `vtable`
//! word: registration from the image's class and category lists, lazy realization (superclass
//! first, non-fragile ivars slid, the method table built from the categories then the base
//! list), method lookup up the superclass chain, cache filling and `+initialize`. The runtime
//! lock covers realization, attaching methods, cache fills and flushes and the class table;
//! no program code ever runs under it.

use super::abi::{self, ClassObject, Id, Imp, ListHeader, Sel};
use super::buffer::Buffer;
use super::cache::{self, Cache};
use super::{RUNTIME, Runtime, register_selector, send};
use crate::libsystem::{heap, thread};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

pub const IMMORTAL: u32 = 1;
pub const CUSTOM_REFERENCE_COUNTING: u32 = 2;
pub const HAS_CXX_DESTRUCT: u32 = 4;
pub const MISSING: u32 = 8;
pub const META: u32 = 16;
pub const BUILTIN: u32 = 32;
pub const LOAD_CALLED: u32 = 64;

const REGISTERED: u32 = 0;
const REALIZED: u32 = 1;
const INITIALIZING: u32 = 2;
const INITIALIZED: u32 = 3;

/// The runtime's record of one class or metaclass.
#[derive(Debug)]
pub struct ClassRecord {
    pub class: *mut ClassObject,
    pub pair: *mut ClassRecord,
    pub name: *const u8,
    pub library: &'static [u8],
    pub flags: AtomicU32,
    state: AtomicU32,
    initializer: AtomicUsize,
    pub instance_size: AtomicUsize,
    methods: UnsafeCell<Buffer<(usize, usize)>>,
    pending: UnsafeCell<Buffer<usize>>,
}

impl ClassRecord {
    pub fn has(&self, flag: u32) -> bool {
        self.flags.load(Ordering::Relaxed) & flag != 0
    }

    pub fn is_meta(&self) -> bool {
        self.has(META)
    }

    /// The class's name, without its NUL.
    pub fn name(&self) -> &'static [u8] {
        if self.name.is_null() {
            return b"?";
        }
        // SAFETY: a record's name is a permanent NUL-terminated string (the image's, a
        // symbol's or the layer's own).
        unsafe { abi_name(self.name) }
    }

    /// The record's own method for `sel`, without its superclasses'; under the runtime lock.
    pub fn own_method(&self, sel: Sel) -> Option<Imp> {
        // SAFETY: the method table changes only under the runtime lock, which the callers hold.
        unsafe { &*self.methods.get() }
            .as_slice()
            .iter()
            .find(|(found, _)| *found == sel as usize)
            .map(|&(_, imp)| imp)
    }

    fn class(&self) -> &ClassObject {
        // SAFETY: a record's class object lives as long as the program.
        unsafe { &*self.class }
    }

    /// The record of the class's superclass, if it has one the runtime knows.
    pub fn superclass(&self) -> Option<&'static ClassRecord> {
        record_of(self.class().superclass.load(Ordering::Acquire))
    }

    pub fn pair(&self) -> &'static ClassRecord {
        // SAFETY: every record is made with its pair, and both live as long as the program.
        unsafe { &*self.pair }
    }

    pub fn is_realized(&self) -> bool {
        self.state.load(Ordering::Acquire) >= REALIZED
    }
}

/// # Safety
/// `name` must be a permanent NUL-terminated string.
pub unsafe fn abi_name(name: *const u8) -> &'static [u8] {
    // SAFETY: the caller's promise.
    unsafe { super::selector::name_bytes(name) }
}

/// The record of `class`, if the runtime knows it.
pub fn record_of(class: *const ClassObject) -> Option<&'static ClassRecord> {
    if class.is_null() {
        return None;
    }
    // SAFETY: class objects the runtime is handed are mapped class objects: the image's, the
    // layer's own or a placeholder.
    let word = unsafe { &*class }.record.load(Ordering::Acquire);
    // SAFETY: a nonzero record word is a record the runtime made, which is never freed.
    (word != 0).then(|| unsafe { &*(word as *const ClassRecord) })
}

/// The class of `object` (not null).
pub fn class_of(object: Id) -> *mut ClassObject {
    // SAFETY: the callers pass live objects, whose first word is their class.
    unsafe { &*object }.isa.load(Ordering::Relaxed)
}

fn new_record(class: *mut ClassObject, name: *const u8, flags: u32) -> Option<*mut ClassRecord> {
    let memory = heap::allocate(core::mem::size_of::<ClassRecord>(), true).cast::<ClassRecord>();
    if memory.is_null() {
        return None;
    }
    let record = ClassRecord {
        class,
        pair: core::ptr::null_mut(),
        name,
        library: b"",
        flags: AtomicU32::new(flags),
        state: AtomicU32::new(REGISTERED),
        initializer: AtomicUsize::new(0),
        instance_size: AtomicUsize::new(0),
        methods: UnsafeCell::new(Buffer::new()),
        pending: UnsafeCell::new(Buffer::new()),
    };
    // SAFETY: a new block of the record's size, which nothing else uses yet.
    unsafe { memory.write(record) };
    Some(memory)
}

/// Makes the records of `class` and its metaclass and enters them in the class table; false if
/// there is no memory. `name` and both class objects must be permanent.
pub fn register_pair(
    runtime: &mut Runtime,
    class: *mut ClassObject,
    name: *const u8,
    flags: u32,
    library: &'static [u8],
) -> Option<(&'static ClassRecord, &'static ClassRecord)> {
    // SAFETY: as in `record_of`.
    let object = unsafe { &*class };
    let meta = object.isa.load(Ordering::Relaxed);
    let class_record = new_record(class, name, flags)?;
    let meta_record = new_record(meta, name, (flags & (MISSING | BUILTIN)) | META | IMMORTAL)?;
    // SAFETY: both records are new; nothing else reaches them before they are published.
    unsafe {
        (*class_record).pair = meta_record;
    }
    // SAFETY: as above.
    unsafe {
        (*meta_record).pair = class_record;
    }
    // SAFETY: as above.
    unsafe {
        (*class_record).library = library;
    }
    // SAFETY: as above.
    unsafe {
        (*meta_record).library = library;
    }
    for (object, record) in [(class, class_record), (meta, meta_record)] {
        // SAFETY: as in `record_of`.
        let object = unsafe { &*object };
        if object.cache.load(Ordering::Relaxed).is_null() {
            object
                .cache
                .store((&raw const cache::EMPTY).cast_mut(), Ordering::Relaxed);
        }
        object.record.store(record as usize, Ordering::Release);
    }
    if !runtime.classes.push(class_record) || !runtime.classes.push(meta_record) {
        return None;
    }
    // SAFETY: both records live as long as the program.
    let class_record = unsafe { &*class_record };
    // SAFETY: as above.
    let meta_record = unsafe { &*meta_record };
    Some((class_record, meta_record))
}

#[allow(clippy::mut_from_ref)]
fn methods_mut(record: &ClassRecord) -> &mut Buffer<(usize, usize)> {
    // SAFETY: the method table is only reached under the runtime lock, which the callers hold,
    // and no other reference to it is live while they change it.
    unsafe { &mut *record.methods.get() }
}

#[allow(clippy::mut_from_ref)]
fn pending_mut(record: &ClassRecord) -> &mut Buffer<usize> {
    // SAFETY: as in `methods_mut`.
    unsafe { &mut *record.pending.get() }
}

/// Adds `entries` ahead of the record's methods (a category on a built-in class, or
/// Foundation's additions to NSObject), then flushes every cache.
pub fn prepend_methods(runtime: &mut Runtime, record: &ClassRecord, entries: &[(usize, usize)]) {
    let methods = methods_mut(record);
    let mut merged = Buffer::new();
    for &entry in entries.iter().chain(methods.as_slice()) {
        merged.push(entry);
    }
    methods.free();
    *methods = merged;
    flush_all(runtime);
    refresh_flags(record);
}

/// Attaches a category's method list to `record`: kept until the class is realized, or merged
/// at once if it already is.
pub fn attach_list(runtime: &mut Runtime, record: &ClassRecord, list: *const ListHeader) {
    if list.is_null() {
        return;
    }
    if record.is_realized() {
        let mut entries = Buffer::new();
        // SAFETY: a category's list in the mapped image.
        for method in unsafe { abi::methods(list) } {
            if let Some(sel) = register_selector(method.name) {
                entries.push((sel as usize, method.imp));
            }
        }
        prepend_methods(runtime, record, entries.as_slice());
        entries.free();
    } else {
        pending_mut(record).push(list as usize);
    }
}

/// Points every class back at the empty cache.
pub fn flush_all(runtime: &mut Runtime) {
    for &record in runtime.classes.as_slice() {
        // SAFETY: the class table holds records the runtime made.
        let record = unsafe { &*record };
        record
            .class()
            .cache
            .store((&raw const cache::EMPTY).cast_mut(), Ordering::Release);
    }
}

fn refresh_flags(record: &ClassRecord) {
    let mut flags = 0;
    let defaults = super::root::default_reference_counting();
    let custom = [
        sel!("retain"),
        sel!("release"),
        sel!("autorelease"),
        sel!("retainCount"),
    ]
    .into_iter()
    .any(|sel| {
        record
            .own_method(sel)
            .is_some_and(|imp| !defaults.contains(&imp))
    });
    if custom {
        flags |= CUSTOM_REFERENCE_COUNTING;
    }
    if record
        .superclass()
        .is_some_and(|superclass| superclass.has(CUSTOM_REFERENCE_COUNTING))
    {
        flags |= CUSTOM_REFERENCE_COUNTING;
    }
    if record.own_method(sel!(".cxx_destruct")).is_some() {
        flags |= HAS_CXX_DESTRUCT;
    }
    record.flags.fetch_or(flags, Ordering::Relaxed);
}

/// Realizes `record` and its pair (the class first): superclasses first, ivars slid, method
/// tables built. A placeholder for a missing class ends the program, naming it.
pub fn realize(runtime: &mut Runtime, record: &ClassRecord) {
    if record.is_realized() {
        return;
    }
    if record.has(MISSING) {
        super::report_missing_class(record);
    }
    let class = if record.is_meta() {
        record.pair()
    } else {
        record
    };
    realize_one(runtime, class);
    realize_one(runtime, class.pair());
}

fn realize_one(runtime: &mut Runtime, record: &ClassRecord) {
    if record.is_realized() {
        return;
    }
    let superclass = record.superclass();
    if let Some(superclass) = superclass {
        realize(runtime, superclass);
    }
    let read_only = record.class().read_only();
    if !read_only.is_null() {
        // SAFETY: a registered image class's `class_ro_t`, mapped.
        let read_only = unsafe { &*read_only };
        let mut size = read_only.instance_size as usize;
        if !record.is_meta() {
            // SAFETY: the class's ivar list, mapped.
            let ivars = unsafe { abi::ivars(read_only.ivars) };
            let alignment = ivars
                .iter()
                .map(|ivar| 1u32 << ivar.alignment_shift.min(4))
                .max()
                .unwrap_or(1);
            let superclass_size = superclass.map_or(0, |superclass| {
                superclass.instance_size.load(Ordering::Relaxed)
            });
            let slide =
                abi::ivar_slide(read_only.instance_start, superclass_size as u32, alignment);
            if slide > 0 {
                for ivar in ivars {
                    if !ivar.offset.is_null() {
                        // SAFETY: the ivar's offset variable, in the image's writable data.
                        unsafe { &*ivar.offset }.fetch_add(slide, Ordering::Relaxed);
                    }
                }
                size += slide as usize;
            }
        }
        record.instance_size.store(size, Ordering::Relaxed);
        let methods = methods_mut(record);
        let pending = pending_mut(record);
        for &list in pending.as_slice().iter().rev() {
            push_list(methods, list as *const ListHeader);
        }
        pending.free();
        push_list(methods, read_only.base_methods);
    }
    refresh_flags(record);
    record.state.store(REALIZED, Ordering::Release);
}

fn push_list(methods: &mut Buffer<(usize, usize)>, list: *const ListHeader) {
    // SAFETY: lists of the mapped image.
    for method in unsafe { abi::methods(list) } {
        if let Some(sel) = register_selector(method.name) {
            methods.push((sel as usize, method.imp));
        }
    }
}

/// Defines one of the layer's own classes: its records, its built-in methods (names are
/// NUL-terminated statics, which become their canonical selectors) and its wiring to its
/// superclass and the root metaclass. The class is realized and initialized at once.
#[derive(Debug)]
pub struct BuiltinClass<'a> {
    pub name: &'static [u8],
    pub class: &'static ClassObject,
    pub meta: &'static ClassObject,
    pub superclass: Option<&'static ClassObject>,
    pub instance_size: usize,
    pub flags: u32,
    pub instance_methods: &'a [(&'static [u8], usize)],
    pub class_methods: &'a [(&'static [u8], usize)],
}

pub fn define(runtime: &mut Runtime, builtin: &BuiltinClass<'_>) -> bool {
    let class = (builtin.class as *const ClassObject).cast_mut();
    let meta = (builtin.meta as *const ClassObject).cast_mut();
    let root_meta = match builtin.superclass {
        None => meta,
        Some(_) => super::root::root_metaclass(),
    };
    builtin.class.isa.store(meta, Ordering::Relaxed);
    builtin.meta.isa.store(root_meta, Ordering::Relaxed);
    match builtin.superclass {
        Some(superclass) => {
            builtin.class.superclass.store(
                (superclass as *const ClassObject).cast_mut(),
                Ordering::Relaxed,
            );
            builtin
                .meta
                .superclass
                .store(superclass.isa.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        None => builtin.meta.superclass.store(class, Ordering::Relaxed),
    }
    let Some((class_record, meta_record)) = register_pair(
        runtime,
        class,
        builtin.name.as_ptr(),
        builtin.flags | BUILTIN,
        b"Foundation",
    ) else {
        return false;
    };
    class_record
        .instance_size
        .store(builtin.instance_size, Ordering::Relaxed);
    meta_record
        .instance_size
        .store(core::mem::size_of::<ClassObject>(), Ordering::Relaxed);
    for (record, methods) in [
        (class_record, builtin.instance_methods),
        (meta_record, builtin.class_methods),
    ] {
        let table = methods_mut(record);
        for &(name, imp) in methods {
            match register_selector(name.as_ptr()) {
                Some(sel) => {
                    table.push((sel as usize, imp));
                }
                None => return false,
            }
        }
        refresh_flags(record);
        record.state.store(INITIALIZED, Ordering::Release);
    }
    true
}

/// Adds built-in methods to an already defined class (Foundation's additions to NSObject).
pub fn extend(
    runtime: &mut Runtime,
    class: &'static ClassObject,
    instance_methods: &[(&'static [u8], usize)],
    class_methods: &[(&'static [u8], usize)],
) -> bool {
    let Some(record) = record_of(class) else {
        return false;
    };
    for (record, methods) in [(record, instance_methods), (record.pair(), class_methods)] {
        let mut entries = Buffer::new();
        for &(name, imp) in methods {
            let Some(sel) = register_selector(name.as_ptr()) else {
                return false;
            };
            entries.push((sel as usize, imp));
        }
        prepend_methods(runtime, record, entries.as_slice());
        entries.free();
    }
    true
}

/// The IMP for `sel` in `record` or its superclasses.
pub fn find_method(record: &ClassRecord, sel: Sel) -> Option<Imp> {
    let mut current = record;
    loop {
        if let Some(imp) = current.own_method(sel) {
            return Some(imp);
        }
        current = current.superclass()?;
    }
}

fn fill(class: &ClassObject, sel: Sel, imp: Imp) {
    let current = class.cache.load(Ordering::Acquire);
    // SAFETY: a class's cache word always points at a table (the empty one at least).
    let table: &Cache = unsafe { &*current };
    let empty = core::ptr::eq(current, &raw const cache::EMPTY);
    if !empty && table.has_room() {
        table.insert(sel as usize, imp);
        return;
    }
    let capacity = if empty {
        cache::FIRST_CAPACITY
    } else {
        table.capacity() * 2
    };
    let memory = heap::allocate(cache::bytes_for(capacity), true);
    if memory.is_null() {
        return;
    }
    // SAFETY: a new zeroed block of the table's size from the 16-byte aligned heap, never
    // freed.
    let grown = unsafe { cache::initialize(memory, capacity) };
    if !empty {
        table.copy_into(grown);
    }
    grown.insert(sel as usize, imp);
    class
        .cache
        .store((grown as *const Cache).cast_mut(), Ordering::Release);
}

/// What the slow path of a send finds for `sel` on `class` (the receiver's class, or the
/// superclass a super send starts at): the class realized and initialized first, the cache
/// filled once the class is initialized; `None` if no class in the chain has the method. A
/// translated program's method comes back as the native thunk that calls it, which is what the
/// cache holds, so `objc_msgSend` never branches to a guest address.
pub fn lookup(class: *mut ClassObject, sel: Sel) -> Option<Imp> {
    let record = record_of(class)?;
    RUNTIME.with(|runtime| realize(runtime, record));
    let target = if record.is_meta() {
        record.pair()
    } else {
        record
    };
    initialize(target);
    RUNTIME.with(|_| {
        let imp = crate::callback::native_callable(find_method(record, sel)?);
        if target.state.load(Ordering::Acquire) == INITIALIZED {
            // SAFETY: as in `record_of`.
            fill(unsafe { &*class }, sel, imp);
        }
        Some(imp)
    })
}

/// Sends `+initialize` to `record`'s class before its first message, superclasses first; other
/// threads wait until it returns, the initializing thread's own messages go through.
pub fn initialize(record: &ClassRecord) {
    let me = thread::current() as usize;
    loop {
        match record.state.load(Ordering::Acquire) {
            INITIALIZED => return,
            INITIALIZING => {
                if record.initializer.load(Ordering::Acquire) == me {
                    return;
                }
                thread::sched_yield();
            }
            _ => {
                if let Some(superclass) = record.superclass() {
                    initialize(superclass);
                }
                if record
                    .state
                    .compare_exchange(REALIZED, INITIALIZING, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    record.initializer.store(me, Ordering::Release);
                    send(record.class.cast(), sel!("initialize"));
                    record.state.store(INITIALIZED, Ordering::Release);
                    return;
                }
                if record.state.load(Ordering::Acquire) == REGISTERED {
                    RUNTIME.with(|runtime| realize(runtime, record));
                }
            }
        }
    }
}

/// The IMP for `sel` on `class` and its superclasses, realizing but not initializing the class
/// and filling no cache, as `class_getInstanceMethod` and `respondsToSelector:` look.
pub fn find(class: *mut ClassObject, sel: Sel) -> Option<Imp> {
    let record = record_of(class)?;
    RUNTIME.with(|runtime| {
        realize(runtime, record);
        find_method(record, sel)
    })
}

/// Realizes and initializes the class `class` (for `objc_alloc`, `objc_getClass` and their kin).
pub fn prepare(class: *mut ClassObject) -> Option<&'static ClassRecord> {
    let record = record_of(class)?;
    RUNTIME.with(|runtime| realize(runtime, record));
    initialize(if record.is_meta() {
        record.pair()
    } else {
        record
    });
    Some(record)
}

/// The class's own `+load`, from its base list only (categories' come from their own lists).
pub fn own_load_method(record: &ClassRecord) -> Option<Imp> {
    let read_only = record.pair().class().read_only();
    if read_only.is_null() {
        return None;
    }
    // SAFETY: the metaclass's `class_ro_t` and base list, mapped.
    let list = unsafe { &*read_only }.base_methods;
    load_in(list)
}

/// The `+load` in a method list.
pub fn load_in(list: *const ListHeader) -> Option<Imp> {
    let load = sel!("load");
    // SAFETY: a list of the mapped image.
    unsafe { abi::methods(list) }
        .find(|method| register_selector(method.name) == Some(load))
        .map(|method| method.imp)
}
