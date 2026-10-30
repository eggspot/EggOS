//! Walking the program's frames (D177): from a `CONTEXT`, each step finds the function in the
//! program's exception directory and undoes its frame with the unwinder, as Windows'
//! dispatcher does. A function with no entry is a leaf (its caller's address is on top of the
//! stack on x64, in `lr` on ARM64). The program's code is the only code with unwind data:
//! where the layer calls back into the program (a language handler, a filter, a catch block,
//! a destructor) it goes through a bridge, which records where that call returns, so a walk
//! that reaches the layer's frames continues from the program frame the bridge names instead.

use super::super::thread_environment::{self, teb};
use super::context::{Context, unwinder};
use super::{CatchState, Frame};
use crate::unwind::{HandlerKinds, Image, Memory};
use core::sync::atomic::{AtomicU64, Ordering};

static IMAGE_BASE: AtomicU64 = AtomicU64::new(0);
static IMAGE_SIZE: AtomicU64 = AtomicU64::new(0);
static TABLE: AtomicU64 = AtomicU64::new(0);
static LAYER_CODE: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
const MAX_FRAMES: usize = 4096;

/// Records where the program's image is and its exception directory (image-relative address
/// and size), and the range of the layer's own code.
pub fn set_image(base: u64, size: u64, table: (u32, u32), layer_code: (u64, u64)) {
    IMAGE_BASE.store(base, Ordering::Relaxed);
    TABLE.store(
        u64::from(table.0) << 32 | u64::from(table.1),
        Ordering::Relaxed,
    );
    LAYER_CODE[0].store(layer_code.0, Ordering::Relaxed);
    LAYER_CODE[1].store(layer_code.1, Ordering::Relaxed);
    IMAGE_SIZE.store(size, Ordering::Release);
}

/// The program's image.
pub fn image() -> Image<'static> {
    let size = IMAGE_SIZE.load(Ordering::Acquire);
    let base = IMAGE_BASE.load(Ordering::Relaxed);
    let bytes = match size {
        0 => &[][..],
        // SAFETY: `set_image` named the program's mapped image, which stays mapped for the
        // process's life; only its unwind data, code and constant data are read through it.
        _ => unsafe { core::slice::from_raw_parts(base as *const u8, size as usize) },
    };
    Image { base, bytes }
}

/// Whether `pc` is in the layer's own code, where the program's unwind data cannot describe
/// the frames.
pub fn in_layer(pc: u64) -> bool {
    (LAYER_CODE[0].load(Ordering::Relaxed)..LAYER_CODE[1].load(Ordering::Relaxed)).contains(&pc)
}

/// The function whose code holds `pc`, from the program's exception directory.
pub fn lookup(pc: u64) -> Option<unwinder::Function> {
    let image = image();
    let rva = image.rva_of(pc)?;
    let table = TABLE.load(Ordering::Relaxed);
    let (start, size) = ((table >> 32) as u32, table as u32);
    if size == 0 {
        return None;
    }
    unwinder::lookup(&image, start, size, rva)
}

/// Reads the thread's memory for the unwinder: any 8-byte-aligned word, as the program's own
/// frames hold it.
pub struct ThreadMemory;

impl Memory for ThreadMemory {
    fn u64_at(&self, address: u64) -> Option<u64> {
        if address == 0 || !address.is_multiple_of(8) {
            return None;
        }
        // SAFETY: the unwinder reads the stack slots the program's unwind data names, which
        // lie in the thread's mapped stack.
        Some(unsafe { (address as *const u64).read_volatile() })
    }
}

/// A call from the layer into the program, linked into the thread's list while it runs.
#[repr(C)]
pub struct Bridge {
    previous: *mut Bridge,
    return_address: u64,
    stack: u64,
    /// Where a walk continues once it leaves the program code this bridge called; none ends
    /// the walk.
    pub resume: Option<Context>,
    /// The catch block this bridge runs, if it runs one.
    pub catch: Option<CatchState>,
}

impl Bridge {
    pub fn new(resume: Option<Context>, catch: Option<CatchState>) -> Self {
        Self {
            previous: core::ptr::null_mut(),
            return_address: 0,
            stack: 0,
            resume,
            catch,
        }
    }
}

fn slot(offset: usize) -> *mut u64 {
    (thread_environment::current() + offset) as *mut u64
}

fn bridges_head() -> *mut Bridge {
    // SAFETY: the slot is this thread's TEB word, which only this thread uses.
    unsafe { slot(teb::EXCEPTION_BRIDGES).read() as *mut Bridge }
}

fn set_bridges_head(bridge: *mut Bridge) {
    // SAFETY: as in `bridges_head`.
    unsafe { slot(teb::EXCEPTION_BRIDGES).write(bridge as u64) };
}

/// The bridges of this thread's calls into the program, innermost first.
pub fn bridges() -> impl Iterator<Item = &'static Bridge> {
    let mut next = bridges_head();
    core::iter::from_fn(move || {
        // SAFETY: every linked bridge lives in a frame of the layer below the program frames
        // that are still running, and is unlinked before its frame ends.
        let bridge = unsafe { next.as_ref()? };
        next = bridge.previous;
        Some(bridge)
    })
}

/// Unlinks the bridges and dispatch records below `stack`, whose frames a resumed context
/// leaves behind.
pub fn forget_below(stack: u64) {
    let mut head = bridges_head();
    // SAFETY: as in `bridges`.
    while let Some(bridge) = unsafe { head.as_ref() } {
        if (head as u64) >= stack {
            break;
        }
        head = bridge.previous;
    }
    set_bridges_head(head);
    super::dispatch::forget_dispatches_below(stack);
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn enter_program(
    _bridge: *mut Bridge,
    _target: u64,
    _first: u64,
    _second: u64,
    _third: u64,
    _fourth: u64,
    _frame_pointer: u64,
) -> u64 {
    core::arch::naked_asm!(
        "push rbx",
        "mov rbx, rdi",
        "mov rax, rsi",
        "mov r10, rdx",
        "mov rdx, rcx",
        "mov rcx, r10",
        "sub rsp, 32",
        "lea r10, [rip + 2f]",
        "mov [rbx + {return_address}], r10",
        "mov [rbx + {stack}], rsp",
        "call rax",
        "2:",
        "add rsp, 32",
        "pop rbx",
        "ret",
        return_address = const core::mem::offset_of!(Bridge, return_address),
        stack = const core::mem::offset_of!(Bridge, stack),
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn enter_program(
    _bridge: *mut Bridge,
    _target: u64,
    _first: u64,
    _second: u64,
    _third: u64,
    _fourth: u64,
    _frame_pointer: u64,
) -> u64 {
    core::arch::naked_asm!(
        "stp x29, x30, [sp, #-32]!",
        "str x19, [sp, #16]",
        "mov x19, x0",
        "mov x16, x1",
        "mov x0, x2",
        "mov x1, x3",
        "mov x2, x4",
        "mov x3, x5",
        "cbz x6, 1f",
        "mov x29, x6",
        "1:",
        "adr x9, 2f",
        "str x9, [x19, #{return_address}]",
        "mov x9, sp",
        "str x9, [x19, #{stack}]",
        "mrs x18, tpidr_el0",
        "blr x16",
        "2:",
        "ldr x19, [sp, #16]",
        "ldp x29, x30, [sp], #32",
        "ret",
        return_address = const core::mem::offset_of!(Bridge, return_address),
        stack = const core::mem::offset_of!(Bridge, stack),
    )
}

/// Calls the program's function at `target` with up to four arguments, as Windows calls it,
/// through `bridge`, and returns its result register.
///
/// # Safety
/// `target` must be a function of the program that takes those arguments.
pub unsafe fn call_program(bridge: &mut Bridge, target: u64, arguments: [u64; 4]) -> u64 {
    // SAFETY: the caller's duty.
    unsafe { enter(bridge, target, arguments, 0) }
}

/// Calls a funclet of the program (a filter, a `__finally` block, a catch block or an unwind
/// action) with `first` and the establisher frame `establisher` of the function it belongs
/// to, as MSVC's runtime calls them: on ARM64 the function's frame pointer `frame_pointer` is
/// in `x29` too, which the funclets address the function's locals through.
///
/// # Safety
/// `target` must be a funclet of the function whose frame `establisher` and `frame_pointer`
/// describe, which is still on the stack.
pub unsafe fn call_funclet(
    bridge: &mut Bridge,
    target: u64,
    first: u64,
    establisher: u64,
    frame_pointer: u64,
) -> u64 {
    // SAFETY: the caller's duty.
    unsafe { enter(bridge, target, [first, establisher, 0, 0], frame_pointer) }
}

unsafe fn enter(bridge: &mut Bridge, target: u64, arguments: [u64; 4], frame_pointer: u64) -> u64 {
    bridge.previous = bridges_head();
    set_bridges_head(bridge);
    // SAFETY: the caller's duty; the bridge stays linked, in this frame, while the program's
    // function runs, and the entry keeps the registers the layer's convention keeps.
    let result = unsafe {
        enter_program(
            bridge,
            target,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
            frame_pointer,
        )
    };
    set_bridges_head(bridge.previous);
    result
}

/// The frame pointer of the function whose registers `context` holds, for its funclets:
/// `x29` on ARM64.
#[cfg(target_arch = "aarch64")]
pub fn frame_pointer(context: &Context) -> u64 {
    context.x[29]
}

/// The frame pointer of the function whose registers `context` holds, for its funclets:
/// none on x64, whose funclets find the function's frame from the establisher alone.
#[cfg(target_arch = "x86_64")]
pub fn frame_pointer(_context: &Context) -> u64 {
    0
}

/// What a walk meets next.
pub enum Event {
    Frame(Frame),
    /// It left the program code a bridge called and continued where the bridge says.
    Bridge(&'static Bridge),
    End,
}

/// A walk up the program's frames from a `CONTEXT`.
pub struct Walk {
    pub context: Context,
    unwound: bool,
    kinds: HandlerKinds,
    steps: usize,
}

impl Walk {
    /// A walk from `context`, whose `pc` is a return address when `unwound` (the thread is
    /// stopped after a call rather than at a faulting instruction), asking each function for the
    /// handlers `kinds` names.
    pub fn new(context: &Context, unwound: bool, kinds: HandlerKinds) -> Self {
        Self {
            context: *context,
            unwound,
            kinds,
            steps: 0,
        }
    }

    pub fn next(&mut self) -> Event {
        self.steps += 1;
        if self.steps > MAX_FRAMES {
            return Event::End;
        }
        let pc = self.context.pc();
        let sp = self.context.sp();
        if self.unwound
            && let Some(bridge) =
                bridges().find(|bridge| bridge.return_address == pc && bridge.stack == sp)
        {
            return match bridge.resume {
                Some(resume) => {
                    self.context = resume;
                    self.unwound = true;
                    Event::Bridge(bridge)
                }
                None => Event::End,
            };
        }
        let lookup_pc = if self.unwound { pc.wrapping_sub(1) } else { pc };
        let image = image();
        let function = match image.rva_of(lookup_pc) {
            Some(_) => lookup(lookup_pc),
            None if in_layer(pc) || pc == 0 => return Event::End,
            None => None,
        };
        let before = self.context;
        let mut registers = before.registers();
        let unwound = match function {
            Some(function) => {
                match unwinder::virtual_unwind(
                    &image,
                    function,
                    self.kinds,
                    &mut registers,
                    &ThreadMemory,
                ) {
                    Ok(unwound) => unwound,
                    Err(_) => return Event::End,
                }
            }
            None => match leaf(&mut registers) {
                Some(establisher) => crate::unwind::Unwound {
                    handler: None,
                    handler_data: 0,
                    establisher,
                },
                None => return Event::End,
            },
        };
        let mut caller = before;
        caller.set_registers(&registers);
        if caller.pc() == 0 {
            return Event::End;
        }
        let frame = Frame {
            context: before,
            caller,
            function,
            handler: unwound.handler,
            handler_data: unwound.handler_data,
            establisher: unwound.establisher,
            unwound: self.unwound,
        };
        self.context = caller;
        self.unwound = true;
        Event::Frame(frame)
    }
}

/// Undoes a leaf function's frame, which has no unwind data: its return address is on top
/// of the stack. Returns its establisher frame.
#[cfg(target_arch = "x86_64")]
fn leaf(registers: &mut unwinder::Registers) -> Option<u64> {
    let rsp = registers.rsp();
    registers.rip = ThreadMemory.u64_at(rsp)?;
    registers.general[unwinder::register::RSP] = rsp + 8;
    Some(rsp)
}

/// Undoes a leaf function's frame, which has no unwind data: it returns to `lr`. Returns its
/// establisher frame.
#[cfg(target_arch = "aarch64")]
fn leaf(registers: &mut unwinder::Registers) -> Option<u64> {
    registers.pc = registers.x[30];
    Some(registers.sp)
}

/// The caller's `CONTEXT` of the frame whose registers `context` holds (after a call, when
/// `unwound`), if the frame can be undone.
pub fn caller_of(context: &Context, unwound: bool) -> Option<Context> {
    let kinds = HandlerKinds {
        exception: false,
        termination: false,
    };
    let mut walk = Walk::new(context, unwound, kinds);
    match walk.next() {
        Event::Frame(frame) => Some(frame.caller),
        Event::Bridge(_) => Some(walk.context),
        Event::End => None,
    }
}
