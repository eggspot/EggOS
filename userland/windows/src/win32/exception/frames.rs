//! Walking the program's frames (D177): from a `CONTEXT`, each step finds the function in the
//! program's exception directory and undoes its frame with the unwinder, as Windows'
//! dispatcher does. A function with no entry is a leaf (its caller's address is on top of the
//! stack on x64, in `lr` on ARM64). The program's code is the only code with unwind data:
//! where the layer calls back into the program (a language handler, a filter, a catch block,
//! a destructor) it goes through a bridge, which records where that call returns, so a walk
//! that reaches the layer's frames continues from the program frame the bridge names instead.
//! A walk that leaves a running catch block's funclet continues in the catch's function, whose
//! handler then searches and unwinds it from the state around the catch's try block; when the
//! funclet's own `__CxxFrameHandler3` handler already stood in for the function (its data is
//! the function's, its states the function's), the walk continues in the function's caller.

use super::super::thread_environment::{self, teb};
use super::context::{Context, unwinder};
use super::{CatchState, Frame};
use crate::unwind::{HandlerKinds, Image, Memory, StackRange};
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
        // process's life. The layer reads only the bytes the program's exception directory and
        // unwind data name; a table that names a section the program mapped without read access
        // faults as the program's own read of it would, and ends the process.
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

/// Reads the thread's stack for the unwinder: only the aligned words of the stack range a walk
/// may use, as Windows bounds a walk by the TEB's `StackLimit` and `StackBase`.
pub struct ThreadMemory {
    range: StackRange,
}

impl ThreadMemory {
    /// This thread's stack, for a walk that starts at stack pointer `sp`.
    pub fn from_stack_pointer(sp: u64) -> Self {
        // SAFETY: the slots are this thread's TEB words, which the layer filled when the thread
        // started.
        let base = unsafe { slot(teb::STACK_BASE).read() };
        // SAFETY: as above.
        let limit = unsafe { slot(teb::STACK_LIMIT).read() };
        Self {
            range: StackRange::new(limit, base, sp),
        }
    }

    /// The stack another context ran on, for `RtlVirtualUnwind` on a context that is not on
    /// this thread's stack (another thread's, or a copy): the committed, readable pages from
    /// `sp` up to `end`, the end of the run of such pages that holds `sp`.
    pub fn readable_from(sp: u64, end: u64) -> Self {
        Self {
            range: StackRange { low: sp, high: end },
        }
    }

    /// Whether `address` lies in the stack range the walk may read.
    pub fn holds(&self, address: u64) -> bool {
        self.range.holds(address)
    }
}

impl Memory for ThreadMemory {
    fn u64_at(&self, address: u64) -> Option<u64> {
        if !self.range.holds_word(address) {
            return None;
        }
        // SAFETY: the word lies either between the stack pointer the thread ran with and the
        // base of the stack its TEB records, the part of the thread's stack that is in use and
        // mapped, or in a run of committed, readable pages `VirtualQuery` reported; a program
        // that unmaps that run meanwhile faults as its own read of it would.
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
    /// the walk. A walk that leaves a catch block's bridge continues in the catch's function
    /// or its caller instead (`Event::Catch`).
    pub resume: Option<Context>,
    /// The catch block this bridge runs, if it runs one.
    pub catch: Option<CatchState>,
    /// Where a walk for termination handlers continues instead of `resume`, and whether its
    /// `pc` is a return address: an unwind started in an exception handler goes on from the
    /// frame the exception was raised in, through every frame the dispatch walked.
    pub unwind_resume: Option<(Context, bool)>,
}

impl Bridge {
    pub fn new(resume: Option<Context>, catch: Option<CatchState>) -> Self {
        Self {
            previous: core::ptr::null_mut(),
            return_address: 0,
            stack: 0,
            resume,
            catch,
            unwind_resume: None,
        }
    }

    /// A bridge into an exception handler of the dispatch that started at `start` (whose `pc`
    /// is a return address when `start_unwound`) and found the handler's frame, whose caller
    /// is `caller`.
    pub fn for_exception_handler(caller: Context, start: Context, start_unwound: bool) -> Self {
        Self {
            unwind_resume: Some((start, start_unwound)),
            ..Self::new(Some(caller), None)
        }
    }

    /// Where a walk asking for `kinds` continues once it leaves the program code this bridge
    /// called, and whether that `pc` is a return address; none ends the walk.
    pub fn continuation(&self, kinds: HandlerKinds) -> Option<(Context, bool)> {
        match (kinds.termination, self.unwind_resume) {
            (true, Some(unwind_resume)) => Some(unwind_resume),
            _ => self.resume.map(|resume| (resume, true)),
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
#[allow(clippy::large_enum_variant)]
pub enum Event {
    Frame(Frame),
    /// It left the program code a bridge called and continued where the bridge says.
    Bridge,
    /// It left the funclet of the running catch block `catch` and continued in the catch's
    /// function, or in that function's caller when `function_skipped` (the funclet's handler
    /// stood in for the function).
    Catch {
        catch: &'static CatchState,
        function_skipped: bool,
    },
    End,
}

/// Whether the handler of a running catch block's funclet stands in for the catch's function:
/// a `__CxxFrameHandler3` funclet's handler data is its function's, so it searches and unwinds
/// the function's states itself; a `__CxxFrameHandler4` funclet's data, if it has a handler at
/// all, covers only its own locals.
pub fn funclet_stands_in(catch: &CatchState, funclet_has_handler: bool) -> bool {
    funclet_has_handler && !catch.data.compressed
}

/// A walk up the program's frames from a `CONTEXT`.
pub struct Walk {
    pub context: Context,
    unwound: bool,
    kinds: HandlerKinds,
    steps: usize,
    memory: ThreadMemory,
    last_frame_had_handler: bool,
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
            memory: ThreadMemory::from_stack_pointer(context.sp()),
            last_frame_had_handler: false,
        }
    }

    pub fn next(&mut self) -> Event {
        self.steps += 1;
        if self.steps > MAX_FRAMES {
            return Event::End;
        }
        let pc = self.context.pc();
        let sp = self.context.sp();
        if !self.memory.range.holds(sp) {
            return Event::End;
        }
        if self.unwound
            && let Some(bridge) =
                bridges().find(|bridge| bridge.return_address == pc && bridge.stack == sp)
        {
            if let Some(catch) = bridge.catch.as_ref() {
                return self.leave_catch(catch);
            }
            return match bridge.continuation(self.kinds) {
                Some((resume, unwound)) => {
                    self.context = resume;
                    self.unwound = unwound;
                    self.last_frame_had_handler = false;
                    Event::Bridge
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
                    &self.memory,
                ) {
                    Ok(unwound) => unwound,
                    Err(_) => return Event::End,
                }
            }
            None => match leaf(&mut registers, &self.memory) {
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
        if caller.pc() == 0
            || !self
                .memory
                .range
                .allows_step(sp, caller.sp(), unwound.establisher)
        {
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
        self.last_frame_had_handler = frame.handler.is_some();
        Event::Frame(frame)
    }

    fn leave_catch(&mut self, catch: &'static CatchState) -> Event {
        let function_skipped = funclet_stands_in(catch, self.last_frame_had_handler);
        let resume = match function_skipped {
            true => caller_of(&catch.parent_context, true),
            false => Some(catch.parent_context),
        };
        let Some(resume) = resume else {
            return Event::End;
        };
        self.context = resume;
        self.unwound = true;
        self.last_frame_had_handler = false;
        Event::Catch {
            catch,
            function_skipped,
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn leaf(registers: &mut unwinder::Registers, memory: &ThreadMemory) -> Option<u64> {
    let rsp = registers.rsp();
    registers.rip = memory.u64_at(rsp)?;
    registers.general[unwinder::register::RSP] = rsp + 8;
    Some(rsp)
}

#[cfg(target_arch = "aarch64")]
fn leaf(registers: &mut unwinder::Registers, _memory: &ThreadMemory) -> Option<u64> {
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
        Event::Bridge | Event::Catch { .. } => Some(walk.context),
        Event::End => None,
    }
}
