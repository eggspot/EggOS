//! Objective-C exceptions (D175, D234): `@throw`, `-[NSException raise]`, `@catch`, `@finally`
//! and `objc_exception_rethrow`, unwound as Apple's runtime does, in two phases. A throw saves
//! the caller's callee-saved registers in an assembly stub; the search phase steps from frame to
//! frame with each loaded image's compact unwind information ([`unwind_info`]), or its DWARF
//! `__eh_frame` entries for the functions the compact table defers to ([`unwind_dwarf`]), and
//! asks each frame's LSDA ([`lsda`]) whether a `@catch` takes the exception; the cleanup phase
//! walks the same frames again and jumps to each cleanup's landing pad (`@finally`, ARC
//! releases) until the handler's, whose pad gets the selector of the catch that matched.
//!
//! The layer's own frames have no unwind tables; boundaries stand in for them. A method that
//! may raise or call back into the program is entered through a guard stub (`guard!`) that
//! records the program's registers at the call: an exception raised inside it
//! ([`raise_in_layer`]), or thrown by the program's code it called, carries on from the
//! program's frame as if the call had thrown. AppKit runs each event, timer and drawing pass
//! inside [`catching`], whose boundary takes whatever reaches it, as AppKit's run loop does on
//! macOS. An exception nothing takes ends the program with its name and reason.

use super::abi::{ClassObject, Id};
use super::lsda::{self, Found};
use super::reference_count::{objc_autorelease, objc_retain};
use super::root;
use super::unwind_dwarf::{self, Fde};
use super::unwind_info;
use crate::libsystem::{heap, thread};
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_arch = "aarch64")]
pub use super::unwind_info::Arm64Registers as Registers;
#[cfg(target_arch = "x86_64")]
pub use super::unwind_info::X86_64Registers as Registers;

const MAX_FRAMES: usize = 4096;
const MAX_IMAGES: usize = 64;
const STACK_WINDOW: u64 = 256 << 20;
const CATCH: usize = 1;

/// Where the thread control block keeps its innermost boundary, for the assembly stubs.
pub const BOUNDARIES: usize = core::mem::offset_of!(thread::Thread, exception_boundaries);

struct ImageSlot {
    start: AtomicUsize,
    end: AtomicUsize,
    unwind_info: AtomicUsize,
    unwind_info_size: AtomicUsize,
    eh_frame: AtomicUsize,
    eh_frame_size: AtomicUsize,
}

impl ImageSlot {
    const fn new() -> Self {
        Self {
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(0),
            unwind_info: AtomicUsize::new(0),
            unwind_info_size: AtomicUsize::new(0),
            eh_frame: AtomicUsize::new(0),
            eh_frame_size: AtomicUsize::new(0),
        }
    }
}

static IMAGES: [ImageSlot; MAX_IMAGES] = [const { ImageSlot::new() }; MAX_IMAGES];
static IMAGES_CLAIMED: AtomicUsize = AtomicUsize::new(0);
static IMAGES_READY: AtomicUsize = AtomicUsize::new(0);
static UNCAUGHT_HANDLER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
struct Image {
    start: u64,
    end: u64,
    unwind_info: (usize, usize),
    eh_frame: (usize, usize),
}

impl Image {
    fn of(slot: &ImageSlot) -> Self {
        Self {
            start: slot.start.load(Ordering::Relaxed) as u64,
            end: slot.end.load(Ordering::Relaxed) as u64,
            unwind_info: (
                slot.unwind_info.load(Ordering::Relaxed),
                slot.unwind_info_size.load(Ordering::Relaxed),
            ),
            eh_frame: (
                slot.eh_frame.load(Ordering::Relaxed),
                slot.eh_frame_size.load(Ordering::Relaxed),
            ),
        }
    }

    fn holds(&self, address: u64, length: u64) -> bool {
        address >= self.start
            && address
                .checked_add(length)
                .is_some_and(|last| last <= self.end)
    }
}

fn section((address, size): (usize, usize)) -> Option<&'static [u8]> {
    if address == 0 || size == 0 {
        return None;
    }
    // SAFETY: a section of a loaded image, checked inside a mapped segment by the loader,
    // which nothing writes and which stays mapped while the program runs.
    Some(unsafe { core::slice::from_raw_parts(address as *const u8, size) })
}

/// Records a loaded image (the program, or a framework it links) where it is mapped, with its
/// `__unwind_info` and `__eh_frame` sections (slid), so exceptions unwind through its frames;
/// `false` when the table of images is full.
pub fn add_image(
    start: usize,
    size: usize,
    unwind_info: Option<(usize, usize)>,
    eh_frame: Option<(usize, usize)>,
) -> bool {
    let index = IMAGES_CLAIMED.fetch_add(1, Ordering::AcqRel);
    let Some(slot) = IMAGES.get(index) else {
        return false;
    };
    let (info, info_size) = unwind_info.unwrap_or((0, 0));
    let (frames, frames_size) = eh_frame.unwrap_or((0, 0));
    slot.start.store(start, Ordering::Relaxed);
    slot.end
        .store(start.saturating_add(size), Ordering::Relaxed);
    slot.unwind_info.store(info, Ordering::Relaxed);
    slot.unwind_info_size.store(info_size, Ordering::Relaxed);
    slot.eh_frame.store(frames, Ordering::Relaxed);
    slot.eh_frame_size.store(frames_size, Ordering::Relaxed);
    while IMAGES_READY
        .compare_exchange_weak(index, index + 1, Ordering::Release, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    true
}

fn image_of(address: u64, length: u64) -> Option<Image> {
    let ready = IMAGES_READY.load(Ordering::Acquire).min(MAX_IMAGES);
    IMAGES[..ready]
        .iter()
        .map(Image::of)
        .find(|image| image.holds(address, length))
}

/// Whether `length` bytes at `address` lie inside a loaded image.
pub fn in_image(address: u64, length: u64) -> bool {
    image_of(address, length).is_some()
}

/// Sets the function called with an exception nothing catches, returning the previous one
/// (`objc_setUncaughtExceptionHandler`, `NSSetUncaughtExceptionHandler`).
pub fn swap_uncaught_handler(handler: usize) -> usize {
    UNCAUGHT_HANDLER.swap(handler, Ordering::AcqRel)
}

/// The function called with an exception nothing catches, 0 for none.
pub fn uncaught_handler() -> usize {
    UNCAUGHT_HANDLER.load(Ordering::Acquire)
}

/// An exception on its way: what was thrown, the handler the search phase found, and the
/// chain of exceptions the thread's catches are handling.
#[repr(C)]
#[derive(Debug)]
pub struct Raised {
    object: Id,
    handler_frame: u64,
    handler_boundary: u64,
    rethrown: bool,
    next: *mut Raised,
}

#[repr(C)]
struct Boundary {
    registers: Registers,
    previous: *mut Boundary,
    kind: usize,
}

#[cfg(target_arch = "aarch64")]
const _: () = assert!(
    core::mem::offset_of!(Boundary, previous) == 328
        && core::mem::offset_of!(Boundary, kind) == 336
        && core::mem::size_of::<Boundary>() <= 352
);
#[cfg(target_arch = "x86_64")]
const _: () = assert!(
    core::mem::offset_of!(Boundary, previous) == 136
        && core::mem::offset_of!(Boundary, kind) == 144
        && core::mem::size_of::<Boundary>() <= 152
);

/// `objc_typeinfo`: what a `@catch` clause names in the LSDA's type table.
#[repr(C)]
#[derive(Debug)]
pub struct TypeInfo {
    vtable: *const usize,
    _name: *const u8,
    class: *const ClassObject,
}

// SAFETY: the records are never written; their pointers are to permanent statics.
unsafe impl Sync for TypeInfo {}

static EHTYPE_VTABLE: [usize; 3] = [0; 3];

const fn vtable_pointer() -> *const usize {
    (&raw const EHTYPE_VTABLE).cast::<usize>().wrapping_add(2)
}

/// `@catch (id)`: any object.
pub static OBJC_EHTYPE_ID: TypeInfo = TypeInfo {
    vtable: vtable_pointer(),
    _name: c"id".as_ptr().cast(),
    class: core::ptr::null(),
};

/// `@catch (NSException *)`.
pub static OBJC_EHTYPE_NSEXCEPTION: TypeInfo = TypeInfo {
    vtable: vtable_pointer(),
    _name: c"NSException".as_ptr().cast(),
    class: &raw const crate::foundation::NSEXCEPTION,
};

/// A byte of a loaded image.
pub fn image_byte(address: u64) -> Option<u8> {
    if !in_image(address, 1) {
        return None;
    }
    // SAFETY: a byte of a mapped image, which stays mapped while the program runs.
    Some(unsafe { (address as *const u8).read() })
}

/// A word of a loaded image, at any alignment.
pub fn image_word(address: u64) -> Option<u64> {
    if !in_image(address, 8) {
        return None;
    }
    // SAFETY: eight bytes of a mapped image, as above.
    Some(unsafe { (address as *const u64).read_unaligned() })
}

fn read_word(address: u64, floor: u64) -> Option<u64> {
    let in_stack = address >= floor
        && address < floor.saturating_add(STACK_WINDOW)
        && address.is_multiple_of(8);
    if !in_stack {
        return image_word(address);
    }
    // SAFETY: a word of the thread's stack above the frame being stepped over, where the
    // frame's unwind information says a saved register is.
    Some(unsafe { (address as *const u64).read() })
}

#[derive(Debug, Clone, Copy)]
enum Unwinding {
    Compact(u32),
    Dwarf(Fde),
}

#[derive(Debug, Clone, Copy)]
struct Function {
    start: u64,
    lsda: Option<u64>,
    unwinding: Unwinding,
    image: Image,
}

enum Lookup {
    Found(Function),
    Unknown,
    Outside,
}

fn dwarf_function(image: &Image, fde: Fde) -> Function {
    Function {
        start: fde.start,
        lsda: fde.lsda,
        unwinding: Unwinding::Dwarf(fde),
        image: *image,
    }
}

fn compact_function(image: &Image, lookup: u64) -> Option<Function> {
    let info = unwind_info::find(
        section(image.unwind_info)?,
        u32::try_from(lookup - image.start).ok()?,
    )?;
    if let Some(offset) = frame::dwarf_offset(info.encoding) {
        let frames = section(image.eh_frame)?;
        let fde = unwind_dwarf::fde_at(
            frames,
            image.eh_frame.0 as u64,
            offset as usize,
            &image_word,
        )
        .ok()?;
        return Some(dwarf_function(image, fde));
    }
    Some(Function {
        start: image.start + u64::from(info.start),
        lsda: info.lsda.map(|offset| image.start + u64::from(offset)),
        unwinding: Unwinding::Compact(info.encoding),
        image: *image,
    })
}

fn scanned_function(image: &Image, lookup: u64) -> Option<Function> {
    let frames = section(image.eh_frame)?;
    let fde = unwind_dwarf::find(frames, image.eh_frame.0 as u64, lookup, &image_word).ok()?;
    Some(dwarf_function(image, fde))
}

fn function_at(pc: u64) -> Lookup {
    let Some(lookup) = pc.checked_sub(1) else {
        return Lookup::Outside;
    };
    let Some(image) = image_of(lookup, 1) else {
        return Lookup::Outside;
    };
    match compact_function(&image, lookup).or_else(|| scanned_function(&image, lookup)) {
        Some(function) => Lookup::Found(function),
        None => Lookup::Unknown,
    }
}

fn catches(type_info: u64, raised: *mut Raised) -> bool {
    // SAFETY: a live exception record.
    let object = unsafe { (*raised).object };
    if type_info == 0 || type_info == &raw const OBJC_EHTYPE_ID as u64 {
        return true;
    }
    if !in_image(type_info, 24) && type_info != &raw const OBJC_EHTYPE_NSEXCEPTION as u64 {
        return false;
    }
    // SAFETY: a type information record of an image's or the layer's, at least three words.
    let info = unsafe { &*(type_info as *const TypeInfo) };
    if info.vtable != vtable_pointer() {
        return false;
    }
    info.class.is_null() || root::is_kind_of(object, info.class)
}

fn landing(function: &Function, pc: u64, raised: *mut Raised) -> Found {
    let Some(lsda) = function.lsda else {
        return Found::Nothing;
    };
    let matches = |type_info: u64| catches(type_info, raised);
    lsda::search(lsda, function.start, pc - 1, &image_byte, &matches).unwrap_or(Found::Nothing)
}

#[cfg(target_arch = "aarch64")]
mod frame {
    use super::{Registers, unwind_info};

    pub fn pc(registers: &Registers) -> u64 {
        registers.pc
    }

    pub fn sp(registers: &Registers) -> u64 {
        registers.sp
    }

    pub fn layer_start(sp: u64) -> Registers {
        Registers {
            sp,
            ..Registers::default()
        }
    }

    pub fn land(registers: &mut Registers, pad: u64, raised: u64, selector: i64) {
        registers.pc = pad;
        registers.x[0] = raised;
        registers.x[1] = selector as u64;
    }

    pub fn dwarf_offset(encoding: u32) -> Option<u32> {
        unwind_info::arm64_dwarf_offset(encoding)
    }

    pub fn step(
        encoding: u32,
        _: u64,
        registers: &mut Registers,
        read: &dyn Fn(u64) -> Option<u64>,
    ) -> bool {
        unwind_info::step_arm64(encoding, registers, read).is_ok()
    }
}

#[cfg(target_arch = "x86_64")]
mod frame {
    use super::{Registers, unwind_info};
    use unwind_info::x86_64::{RAX, RDX, RSP};

    pub fn pc(registers: &Registers) -> u64 {
        registers.rip
    }

    pub fn sp(registers: &Registers) -> u64 {
        registers.general[RSP]
    }

    pub fn layer_start(sp: u64) -> Registers {
        let mut registers = Registers::default();
        registers.general[RSP] = sp;
        registers
    }

    pub fn land(registers: &mut Registers, pad: u64, raised: u64, selector: i64) {
        registers.rip = pad;
        registers.general[RAX] = raised;
        registers.general[RDX] = selector as u64;
    }

    pub fn dwarf_offset(encoding: u32) -> Option<u32> {
        unwind_info::x86_64_dwarf_offset(encoding)
    }

    pub fn step(
        encoding: u32,
        function_start: u64,
        registers: &mut Registers,
        read: &dyn Fn(u64) -> Option<u64>,
    ) -> bool {
        unwind_info::step_x86_64(encoding, function_start, registers, read).is_ok()
    }
}

fn step(function: &Function, registers: &mut Registers) -> bool {
    let floor = frame::sp(registers);
    let read = |address: u64| read_word(address, floor);
    match &function.unwinding {
        Unwinding::Compact(encoding) => frame::step(*encoding, function.start, registers, &read),
        Unwinding::Dwarf(fde) => {
            let Some(frames) = section(function.image.eh_frame) else {
                return false;
            };
            let pc = frame::pc(registers).wrapping_sub(1);
            let base = function.image.eh_frame.0 as u64;
            unwind_dwarf::step(frames, base, fde, pc, registers, &read).is_ok()
        }
    }
}

fn head() -> *mut Boundary {
    // SAFETY: the calling thread's own control block.
    unsafe { (*thread::current()).exception_boundaries as *mut Boundary }
}

fn set_head(boundary: *mut Boundary) {
    // SAFETY: as above.
    unsafe { (*thread::current()).exception_boundaries = boundary as usize };
}

fn boundary_above(sp: u64) -> Option<*mut Boundary> {
    let mut boundary = head();
    for _ in 0..MAX_FRAMES {
        if boundary.is_null() {
            return None;
        }
        if boundary as u64 > sp {
            return Some(boundary);
        }
        // SAFETY: a record its stub linked, on this thread's stack.
        boundary = unsafe { (*boundary).previous };
    }
    None
}

fn drop_boundaries_below(sp: u64) {
    let mut boundary = head();
    for _ in 0..MAX_FRAMES {
        if boundary.is_null() || boundary as u64 > sp {
            break;
        }
        // SAFETY: as in `boundary_above`.
        boundary = unsafe { (*boundary).previous };
    }
    set_head(boundary);
}

enum Frame {
    Program(Function),
    Boundary(*mut Boundary),
    End,
}

fn classify(registers: &Registers) -> Frame {
    match function_at(frame::pc(registers)) {
        Lookup::Found(function) => Frame::Program(function),
        Lookup::Unknown => Frame::End,
        Lookup::Outside => boundary_above(frame::sp(registers)).map_or(Frame::End, Frame::Boundary),
    }
}

enum Handler {
    Frame(u64),
    Boundary(u64),
}

fn search(start: &Registers, raised: *mut Raised) -> Option<Handler> {
    let mut registers = *start;
    for _ in 0..MAX_FRAMES {
        match classify(&registers) {
            Frame::Program(function) => {
                let pc = frame::pc(&registers);
                if let Found::Handler { .. } = landing(&function, pc, raised) {
                    return Some(Handler::Frame(frame::sp(&registers)));
                }
                if !step(&function, &mut registers) {
                    return None;
                }
            }
            Frame::Boundary(boundary) => {
                // SAFETY: a live record on this thread's stack, linked by its stub.
                let record = unsafe { &*boundary };
                if record.kind == CATCH {
                    return Some(Handler::Boundary(boundary as u64));
                }
                registers = record.registers;
            }
            Frame::End => return None,
        }
    }
    None
}

unsafe extern "C" {
    fn eggos_install_context(registers: *const Registers) -> !;
}

fn install_at(mut registers: Registers, pad: u64, raised: *mut Raised, selector: i64) -> ! {
    frame::land(&mut registers, pad, raised as u64, selector);
    // SAFETY: the registers are a frame of the program the unwinder walked to, or a catch
    // stub's own, and the pad a landing pad there, which takes the exception and selector in
    // the first two registers.
    unsafe { eggos_install_context(&registers) }
}

fn install(registers: Registers, pad: u64, raised: *mut Raised, selector: i64) -> ! {
    drop_boundaries_below(frame::sp(&registers));
    install_at(registers, pad, raised, selector)
}

fn deliver(boundary: *mut Boundary, raised: *mut Raised) -> ! {
    // SAFETY: a live catch record; its registers resume its stub's caught path.
    let previous = unsafe { (*boundary).previous };
    // SAFETY: as above.
    let registers = unsafe { (*boundary).registers };
    set_head(previous);
    install_at(registers, frame::pc(&registers), raised, 0)
}

fn clean_up_to_handler(raised: *mut Raised, start: Registers) -> ! {
    // SAFETY: an exception record made by `new_raised`, alive until its catch ends.
    let handler_frame = unsafe { (*raised).handler_frame };
    // SAFETY: as above.
    let handler_boundary = unsafe { (*raised).handler_boundary };
    let mut registers = start;
    for _ in 0..MAX_FRAMES {
        match classify(&registers) {
            Frame::Program(function) => {
                let pc = frame::pc(&registers);
                let found = landing(&function, pc, raised);
                if handler_boundary == 0 && frame::sp(&registers) == handler_frame {
                    if let Found::Handler {
                        landing_pad,
                        selector,
                    } = found
                    {
                        install(registers, landing_pad, raised, selector);
                    }
                    break;
                }
                if let Found::Cleanup(pad) = found {
                    install(registers, pad, raised, 0);
                }
                if !step(&function, &mut registers) {
                    break;
                }
            }
            Frame::Boundary(boundary) => {
                if boundary as u64 == handler_boundary {
                    deliver(boundary, raised);
                }
                // SAFETY: as in `search`.
                let record = unsafe { &*boundary };
                if record.kind == CATCH {
                    break;
                }
                registers = record.registers;
            }
            Frame::End => break,
        }
    }
    report_uncaught(raised)
}

fn raise(raised: *mut Raised, start: Registers) -> ! {
    let handler = search(&start, raised);
    let (frame, boundary) = match handler {
        Some(Handler::Frame(frame)) => (frame, 0),
        Some(Handler::Boundary(boundary)) => (0, boundary),
        None => report_uncaught(raised),
    };
    // SAFETY: as in `clean_up_to_handler`.
    unsafe { (*raised).handler_frame = frame };
    // SAFETY: as above.
    unsafe { (*raised).handler_boundary = boundary };
    clean_up_to_handler(raised, start)
}

fn new_raised(object: Id) -> *mut Raised {
    let raised = heap::allocate(core::mem::size_of::<Raised>(), true).cast::<Raised>();
    if raised.is_null() {
        report_uncaught_object(object);
    }
    objc_retain(object);
    // SAFETY: a new block of the record's size.
    unsafe {
        raised.write(Raised {
            object,
            handler_frame: 0,
            handler_boundary: 0,
            rethrown: false,
            next: core::ptr::null_mut(),
        })
    };
    raised
}

/// Throws `object` from the frame whose registers an exception stub saved at `registers`.
pub fn throw_from(object: Id, registers: *const Registers) -> ! {
    if object.is_null() {
        report_uncaught_object(object);
    }
    // SAFETY: the stub's saved registers, on its stack below the caller of this function; the
    // stub wrote every field, zero for the registers a call does not keep.
    let start = unsafe { *registers };
    raise(new_raised(object), start)
}

fn current_stack() -> u64 {
    let marker = 0u8;
    core::hint::black_box(&marker) as *const u8 as u64
}

/// Throws `object` from the layer's own code: from the program's frame that entered the
/// innermost guarded method, or to the innermost [`catching`] boundary. The layer's frames in
/// between are left as they are, so a caller holds no lock and owns nothing that needs freeing
/// when it raises.
pub fn raise_in_layer(object: Id) -> ! {
    if object.is_null() {
        report_uncaught_object(object);
    }
    raise(new_raised(object), frame::layer_start(current_stack()))
}

extern "C" fn throw(object: Id, registers: *const Registers) -> ! {
    throw_from(object, registers)
}

extern "C" fn resume(raised: *mut Raised, registers: *const Registers) -> ! {
    if raised.is_null() {
        crate::libsystem::abort()
    }
    // SAFETY: as in `throw_from`.
    let start = unsafe { *registers };
    clean_up_to_handler(raised, start)
}

fn caught() -> *mut Raised {
    // SAFETY: the calling thread's own control block.
    unsafe { (*thread::current()).caught_exceptions as *mut Raised }
}

fn set_caught(raised: *mut Raised) {
    // SAFETY: as above.
    unsafe { (*thread::current()).caught_exceptions = raised as usize };
}

extern "C" fn rethrow(_: usize, registers: *const Registers) -> ! {
    let top = caught();
    if top.is_null() {
        crate::libsystem::abort()
    }
    // SAFETY: the record of the exception the innermost `@catch` is handling.
    unsafe { (*top).rethrown = true };
    // SAFETY: as in `throw_from`.
    let start = unsafe { *registers };
    raise(top, start)
}

extern "C" fn begin_catch(raised: *mut Raised) -> Id {
    if raised.is_null() {
        return core::ptr::null_mut();
    }
    let top = caught();
    // SAFETY: a record a landing pad got from the unwinder.
    unsafe { (*raised).rethrown = false };
    if top != raised {
        // SAFETY: as above.
        unsafe { (*raised).next = top };
    }
    set_caught(raised);
    // SAFETY: as above.
    unsafe { (*raised).object }
}

extern "C" fn end_catch() {
    let top = caught();
    if top.is_null() {
        return;
    }
    // SAFETY: the record the innermost `@catch` began with.
    let next = unsafe { (*top).next };
    // SAFETY: as above.
    let rethrown = unsafe { (*top).rethrown };
    // SAFETY: as above.
    let object = unsafe { (*top).object };
    set_caught(next);
    if !rethrown {
        objc_autorelease(object);
        heap::release(top.cast());
    }
}

/// An exception a [`catching`] boundary took.
#[derive(Debug)]
pub struct Caught(*mut Raised);

impl Caught {
    /// Throws it on from the layer, after the code that caught it has cleaned up.
    pub fn rethrow(self) -> ! {
        raise(self.0, frame::layer_start(current_stack()))
    }

    /// Ends it: the object thrown, autoreleased.
    pub fn finish(self) -> Id {
        // SAFETY: a record the unwinder handed to the boundary, owned by it now.
        let object = unsafe { (*self.0).object };
        objc_autorelease(object);
        heap::release(self.0.cast());
        object
    }
}

unsafe extern "C" {
    fn eggos_catching(body: extern "C" fn(usize), data: usize) -> *mut Raised;
}

extern "C" fn run_work(data: usize) {
    // SAFETY: `catching` passes the address of its `&mut dyn FnMut()`, alive during the call.
    let work = unsafe { &mut *(data as *mut &mut dyn FnMut()) };
    work();
}

/// Runs `work` behind a boundary that takes any exception reaching it, as AppKit's run loop
/// does on macOS; `None` when `work` returned normally. The layer's frames inside `work` that
/// an exception passes are left as they are (see [`raise_in_layer`]).
pub fn catching(mut work: &mut dyn FnMut()) -> Option<Caught> {
    let data = (&raw mut work) as usize;
    // SAFETY: the stub links a catch record on its stack, calls `run_work` with `data` and
    // unlinks it; it returns once, either way, with the caller's registers kept.
    let raised = unsafe { eggos_catching(run_work, data) };
    (!raised.is_null()).then_some(Caught(raised))
}

fn report_uncaught(raised: *mut Raised) -> ! {
    // SAFETY: a live exception record.
    report_uncaught_object(unsafe { (*raised).object })
}

fn report_uncaught_object(object: Id) -> ! {
    let handler = uncaught_handler();
    if handler != 0 && !object.is_null() {
        // SAFETY: the program set it to a function that takes the exception.
        let handler: extern "C" fn(Id) = unsafe { core::mem::transmute(handler) };
        handler(object);
    }
    crate::foundation::terminate_with(object)
}

extern "C" fn personality() -> ! {
    crate::libsystem::abort()
}

extern "C" fn objc_terminate() -> ! {
    crate::libsystem::abort()
}

extern "C" fn set_uncaught_exception_handler(handler: usize) -> usize {
    swap_uncaught_handler(handler)
}

macro_rules! capture {
    ($entry:ident => $target:path, $arm64:literal, $x86_64:literal) => {
        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 2",
            concat!(".globl ", stringify!($entry)),
            concat!(".hidden ", stringify!($entry)),
            concat!(stringify!($entry), ":"),
            "sub sp, sp, #336",
            "stp xzr, xzr, [sp, #0]",
            "stp xzr, xzr, [sp, #16]",
            "stp xzr, xzr, [sp, #32]",
            "stp xzr, xzr, [sp, #48]",
            "stp xzr, xzr, [sp, #64]",
            "stp xzr, xzr, [sp, #80]",
            "stp xzr, xzr, [sp, #96]",
            "stp xzr, xzr, [sp, #112]",
            "stp xzr, xzr, [sp, #128]",
            "str xzr, [sp, #144]",
            "stp x19, x20, [sp, #152]",
            "stp x21, x22, [sp, #168]",
            "stp x23, x24, [sp, #184]",
            "stp x25, x26, [sp, #200]",
            "stp x27, x28, [sp, #216]",
            "stp x29, x30, [sp, #232]",
            "add x9, sp, #336",
            "stp x9, x30, [sp, #248]",
            "stp d8, d9, [sp, #264]",
            "stp d10, d11, [sp, #280]",
            "stp d12, d13, [sp, #296]",
            "stp d14, d15, [sp, #312]",
            concat!("mov ", $arm64, ", sp"),
            "bl {target}",
            "brk #1",
            target = sym $target,
        );
        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 4",
            concat!(".globl ", stringify!($entry)),
            concat!(".hidden ", stringify!($entry)),
            concat!(stringify!($entry), ":"),
            "sub rsp, 152",
            "mov qword ptr [rsp + 0], 0",
            "mov qword ptr [rsp + 8], 0",
            "mov qword ptr [rsp + 16], 0",
            "mov qword ptr [rsp + 32], 0",
            "mov qword ptr [rsp + 40], 0",
            "mov qword ptr [rsp + 64], 0",
            "mov qword ptr [rsp + 72], 0",
            "mov qword ptr [rsp + 80], 0",
            "mov qword ptr [rsp + 88], 0",
            "mov [rsp + 24], rbx",
            "mov [rsp + 48], rbp",
            "lea rax, [rsp + 160]",
            "mov [rsp + 56], rax",
            "mov [rsp + 96], r12",
            "mov [rsp + 104], r13",
            "mov [rsp + 112], r14",
            "mov [rsp + 120], r15",
            "mov rax, [rsp + 152]",
            "mov [rsp + 128], rax",
            concat!("mov ", $x86_64, ", rsp"),
            "call {target}",
            "ud2",
            target = sym $target,
        );
        unsafe extern "C" {
            pub fn $entry();
        }
    };
}

pub(crate) use capture;

capture!(eggos_objc_exception_throw => throw, "x1", "rsi");
capture!(eggos_objc_exception_rethrow => rethrow, "x1", "rsi");
capture!(eggos_unwind_resume => resume, "x1", "rsi");

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".text",
    ".p2align 2",
    ".globl eggos_install_context",
    ".hidden eggos_install_context",
    "eggos_install_context:",
    "ldp x19, x20, [x0, #152]",
    "ldp x21, x22, [x0, #168]",
    "ldp x23, x24, [x0, #184]",
    "ldp x25, x26, [x0, #200]",
    "ldp x27, x28, [x0, #216]",
    "ldp x29, x30, [x0, #232]",
    "ldp d8, d9, [x0, #264]",
    "ldp d10, d11, [x0, #280]",
    "ldp d12, d13, [x0, #296]",
    "ldp d14, d15, [x0, #312]",
    "ldr x9, [x0, #248]",
    "ldr x16, [x0, #256]",
    "ldr x1, [x0, #8]",
    "ldr x0, [x0, #0]",
    "mov sp, x9",
    "br x16",
);

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".text",
    ".p2align 4",
    ".globl eggos_install_context",
    ".hidden eggos_install_context",
    "eggos_install_context:",
    "mov rbx, [rdi + 24]",
    "mov rbp, [rdi + 48]",
    "mov r12, [rdi + 96]",
    "mov r13, [rdi + 104]",
    "mov r14, [rdi + 112]",
    "mov r15, [rdi + 120]",
    "mov rax, [rdi + 0]",
    "mov rdx, [rdi + 8]",
    "mov rcx, [rdi + 128]",
    "mov rsp, [rdi + 56]",
    "jmp rcx",
);

macro_rules! guard {
    ($entry:ident => $target:path) => {
        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 2",
            concat!(".globl ", stringify!($entry)),
            concat!(".hidden ", stringify!($entry)),
            concat!(stringify!($entry), ":"),
            "sub sp, sp, #352",
            "stp x19, x20, [sp, #152]",
            "stp x21, x22, [sp, #168]",
            "stp x23, x24, [sp, #184]",
            "stp x25, x26, [sp, #200]",
            "stp x27, x28, [sp, #216]",
            "stp x29, x30, [sp, #232]",
            "add x9, sp, #352",
            "stp x9, x30, [sp, #248]",
            "stp d8, d9, [sp, #264]",
            "stp d10, d11, [sp, #280]",
            "stp d12, d13, [sp, #296]",
            "stp d14, d15, [sp, #312]",
            "mrs x9, tpidr_el0",
            "ldr x10, [x9, #{boundaries}]",
            "stp x10, xzr, [sp, #328]",
            "mov x10, sp",
            "str x10, [x9, #{boundaries}]",
            "add x29, sp, #232",
            "bl {target}",
            "mrs x9, tpidr_el0",
            "ldr x10, [sp, #328]",
            "str x10, [x9, #{boundaries}]",
            "ldp x29, x30, [sp, #232]",
            "add sp, sp, #352",
            "ret",
            target = sym $target,
            boundaries = const $crate::objc::exception::BOUNDARIES,
        );
        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 4",
            concat!(".globl ", stringify!($entry)),
            concat!(".hidden ", stringify!($entry)),
            concat!(stringify!($entry), ":"),
            "sub rsp, 152",
            "mov [rsp + 24], rbx",
            "mov [rsp + 48], rbp",
            "lea r11, [rsp + 160]",
            "mov [rsp + 56], r11",
            "mov [rsp + 96], r12",
            "mov [rsp + 104], r13",
            "mov [rsp + 112], r14",
            "mov [rsp + 120], r15",
            "mov r11, [rsp + 152]",
            "mov [rsp + 128], r11",
            "mov r11, qword ptr fs:[8]",
            "mov r10, [r11 + {boundaries}]",
            "mov [rsp + 136], r10",
            "mov qword ptr [rsp + 144], 0",
            "mov [r11 + {boundaries}], rsp",
            "call {target}",
            "mov r11, qword ptr fs:[8]",
            "mov r10, [rsp + 136]",
            "mov [r11 + {boundaries}], r10",
            "add rsp, 152",
            "ret",
            target = sym $target,
            boundaries = const $crate::objc::exception::BOUNDARIES,
        );
        unsafe extern "C" {
            pub fn $entry();
        }
    };
}

pub(crate) use guard;

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".text",
    ".p2align 2",
    ".globl eggos_catching",
    ".hidden eggos_catching",
    "eggos_catching:",
    "sub sp, sp, #352",
    "stp x19, x20, [sp, #152]",
    "stp x21, x22, [sp, #168]",
    "stp x23, x24, [sp, #184]",
    "stp x25, x26, [sp, #200]",
    "stp x27, x28, [sp, #216]",
    "stp x29, x30, [sp, #232]",
    "mov x9, sp",
    "adr x10, 2f",
    "stp x9, x10, [sp, #248]",
    "stp d8, d9, [sp, #264]",
    "stp d10, d11, [sp, #280]",
    "stp d12, d13, [sp, #296]",
    "stp d14, d15, [sp, #312]",
    "mrs x9, tpidr_el0",
    "ldr x10, [x9, #{boundaries}]",
    "mov x11, #{catch}",
    "stp x10, x11, [sp, #328]",
    "mov x10, sp",
    "str x10, [x9, #{boundaries}]",
    "add x29, sp, #232",
    "mov x9, x0",
    "mov x0, x1",
    "blr x9",
    "mrs x9, tpidr_el0",
    "ldr x10, [sp, #328]",
    "str x10, [x9, #{boundaries}]",
    "mov x0, xzr",
    "ldp x29, x30, [sp, #232]",
    "add sp, sp, #352",
    "ret",
    "2:",
    "ldp x29, x30, [sp, #232]",
    "add sp, sp, #352",
    "ret",
    boundaries = const BOUNDARIES,
    catch = const CATCH,
);

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".text",
    ".p2align 4",
    ".globl eggos_catching",
    ".hidden eggos_catching",
    "eggos_catching:",
    "sub rsp, 152",
    "mov [rsp + 24], rbx",
    "mov [rsp + 48], rbp",
    "mov [rsp + 56], rsp",
    "mov [rsp + 96], r12",
    "mov [rsp + 104], r13",
    "mov [rsp + 112], r14",
    "mov [rsp + 120], r15",
    "lea r11, [rip + 2f]",
    "mov [rsp + 128], r11",
    "mov r11, qword ptr fs:[8]",
    "mov r10, [r11 + {boundaries}]",
    "mov [rsp + 136], r10",
    "mov qword ptr [rsp + 144], {catch}",
    "mov [r11 + {boundaries}], rsp",
    "mov rax, rdi",
    "mov rdi, rsi",
    "call rax",
    "mov r11, qword ptr fs:[8]",
    "mov r10, [rsp + 136]",
    "mov [r11 + {boundaries}], r10",
    "xor eax, eax",
    "add rsp, 152",
    "ret",
    "2:",
    "add rsp, 152",
    "ret",
    boundaries = const BOUNDARIES,
    catch = const CATCH,
);

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_objc_exception_throw" => eggos_objc_exception_throw as *const () as usize,
        b"_objc_exception_rethrow" => eggos_objc_exception_rethrow as *const () as usize,
        b"__Unwind_Resume" => eggos_unwind_resume as *const () as usize,
        b"_objc_begin_catch" => begin_catch as *const () as usize,
        b"_objc_end_catch" => end_catch as *const () as usize,
        b"___objc_personality_v0" => personality as *const () as usize,
        b"_objc_terminate" => objc_terminate as *const () as usize,
        b"_objc_setUncaughtExceptionHandler" => {
            set_uncaught_exception_handler as *const () as usize
        }
        b"_objc_ehtype_vtable" => &raw const EHTYPE_VTABLE as usize,
        b"_OBJC_EHTYPE_id" => &raw const OBJC_EHTYPE_ID as usize,
        b"_OBJC_EHTYPE_$_NSException" => &raw const OBJC_EHTYPE_NSEXCEPTION as usize,
        _ => return None,
    };
    Some(address as u64)
}
