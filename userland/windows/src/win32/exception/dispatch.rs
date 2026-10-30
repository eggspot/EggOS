//! Dispatching and unwinding (D177). `dispatch` gives an exception to the vectored handlers,
//! then walks the program's frames calling each language handler that asks for exceptions,
//! then calls the unhandled-exception filter; `unwind` walks from the exception to a target
//! frame calling each termination handler and returns the target frame's registers, which the
//! caller resumes at the target address. A fault reaches `dispatch` through the signal
//! handler, which rewrites its frame so the thread leaves the handler into `dispatch_fault`
//! with the exception's record and `CONTEXT` on its own stack, below the faulting frame.

use super::super::thread_environment::{self, teb};
use super::context::{
    Context, UCONTEXT_ARGUMENTS, UCONTEXT_PC, UCONTEXT_SP, capturing_entry, fault_entry, restore,
    unwinder,
};
use super::frames::{self, Bridge, Event, ThreadMemory, Walk};
use super::{
    DispatcherContext, ExceptionPointers, ExceptionRecord, MAX_PARAMETERS, cxx, disposition,
    filter, flag, status, vectored,
};
use crate::fault;
use crate::unwind::HandlerKinds;
use core::sync::atomic::{AtomicUsize, Ordering};

static UNHANDLED_EXCEPTION_FILTER: AtomicUsize = AtomicUsize::new(0);
const STACK_GAP: u64 = 128;
/// No state to unwind a C++ frame to: the frame is left, so every state is undone.
pub const NO_STATE: i32 = i32::MIN;

/// A dispatch in progress on this thread: its `CONTEXT`, and whether the `CONTEXT`'s `pc` is a
/// return address (a raised exception) rather than a faulting instruction.
pub struct Dispatch {
    previous: *mut Dispatch,
    pub context: *mut Context,
    pub start_unwound: bool,
}

/// Where an unwind goes: the target frame's establisher, the address it resumes at, and for a
/// C++ frame the state its catch needs it in.
#[derive(Debug, Clone, Copy)]
pub struct UnwindTarget {
    pub frame: u64,
    pub ip: u64,
    pub state: i32,
}

/// Where an exception came from: the program raised it, or the CPU did with this signal.
#[derive(Debug, Clone, Copy)]
pub enum Origin {
    Raised,
    Fault(i32),
}

fn slot(offset: usize) -> *mut u64 {
    (thread_environment::current() + offset) as *mut u64
}

fn dispatches_head() -> *mut Dispatch {
    // SAFETY: the slot is this thread's TEB word, which only this thread uses.
    unsafe { slot(teb::EXCEPTION_DISPATCHES).read() as *mut Dispatch }
}

fn set_dispatches_head(dispatch: *mut Dispatch) {
    // SAFETY: as in `dispatches_head`.
    unsafe { slot(teb::EXCEPTION_DISPATCHES).write(dispatch as u64) };
}

/// The innermost dispatch in progress on this thread.
pub fn current_dispatch() -> Option<&'static Dispatch> {
    // SAFETY: a linked dispatch lives in a frame of the layer that has not ended: a dispatch
    // ends only by resuming a context above it, which unlinks it first.
    unsafe { dispatches_head().as_ref() }
}

/// Unlinks the dispatches below `stack`, whose frames a resumed context leaves behind.
pub fn forget_dispatches_below(stack: u64) {
    let mut head = dispatches_head();
    // SAFETY: as in `current_dispatch`.
    while let Some(dispatch) = unsafe { head.as_ref() } {
        if (head as u64) >= stack {
            break;
        }
        head = dispatch.previous;
    }
    set_dispatches_head(head);
}

/// The unwind in progress on this thread, if any.
pub fn unwind_target() -> Option<&'static UnwindTarget> {
    // SAFETY: the slot holds null or the target of an unwind whose frame is running.
    unsafe { (slot(teb::UNWIND_TARGET).read() as *const UnwindTarget).as_ref() }
}

/// Resumes the thread at `context`, forgetting the layer's calls and dispatches below it.
pub fn resume(context: &Context) -> ! {
    frames::forget_below(context.sp());
    cxx::refresh_current_exception();
    // SAFETY: `context` holds registers the thread ran with in a frame still on its stack (or
    // a handler changed them, as Windows lets it).
    unsafe { restore(context, 0) }
}

/// Gives the exception `record`, raised in `context`, to the program's handlers. Ends only by
/// resuming a context or ending the process.
pub fn dispatch(record: *mut ExceptionRecord, context: *mut Context, origin: Origin) -> ! {
    let start_unwound = matches!(origin, Origin::Raised);
    let mut this = Dispatch {
        previous: dispatches_head(),
        context,
        start_unwound,
    };
    set_dispatches_head(&raw mut this);
    let mut pointers = ExceptionPointers { record, context };
    if vectored::call_exception_handlers(&mut pointers, context) {
        continue_execution(record, context);
    }
    // SAFETY: the dispatch's `CONTEXT`, in a frame below this one or this one's caller's.
    let start = unsafe { *context };
    let kinds = HandlerKinds {
        exception: true,
        termination: false,
    };
    let mut walk = Walk::new(&start, start_unwound, kinds);
    loop {
        match walk.next() {
            Event::End => break,
            Event::Bridge(_) => {}
            Event::Frame(frame) => {
                let Some(handler) = frame.handler else {
                    continue;
                };
                let mut frame_context = frame.context;
                let mut dispatcher = DispatcherContext::of(&frame, &mut frame_context, 0);
                let mut bridge = Bridge::new(Some(frame.caller), None);
                // SAFETY: `handler` is the language handler the program's unwind data names
                // for this frame, called with the four arguments every language handler takes.
                let answer = unsafe {
                    frames::call_program(
                        &mut bridge,
                        handler,
                        [
                            record as u64,
                            frame.establisher,
                            context as u64,
                            &raw mut dispatcher as u64,
                        ],
                    )
                } as i32;
                if answer == disposition::CONTINUE_EXECUTION {
                    continue_execution(record, context);
                }
            }
        }
    }
    unhandled(record, context, origin)
}

fn continue_execution(record: *mut ExceptionRecord, context: *mut Context) -> ! {
    // SAFETY: the dispatch's record and `CONTEXT`.
    let (flags, pc) = unsafe { ((*record).flags, (*context).pc()) };
    if flags & flag::NONCONTINUABLE != 0 {
        crate::fault_handler::end_with_exception(status::NONCONTINUABLE_EXCEPTION, pc);
    }
    let mut pointers = ExceptionPointers { record, context };
    vectored::call_continue_handlers(&mut pointers, context);
    // SAFETY: as above.
    resume(unsafe { &*context })
}

fn unhandled(record: *mut ExceptionRecord, context: *mut Context, origin: Origin) -> ! {
    let top_level = UNHANDLED_EXCEPTION_FILTER.load(Ordering::Acquire);
    if top_level != 0 {
        let mut pointers = ExceptionPointers { record, context };
        let mut bridge = Bridge::new(None, None);
        // SAFETY: the program set this filter with `SetUnhandledExceptionFilter`; it takes the
        // exception's pointers.
        let answer = unsafe {
            frames::call_program(
                &mut bridge,
                top_level as u64,
                [&raw mut pointers as u64, 0, 0, 0],
            )
        } as i32;
        if answer == filter::CONTINUE_EXECUTION {
            continue_execution(record, context);
        }
    }
    // SAFETY: the dispatch's record.
    let record = unsafe { &*record };
    match origin {
        Origin::Fault(signal) => {
            crate::fault_handler::end_with_fault(signal, record.address, record.parameters[1])
        }
        Origin::Raised => crate::fault_handler::end_with_exception(record.code, record.address),
    }
}

/// Unwinds from `start` (whose `pc` is a return address when `start_unwound`) to `target`,
/// calling the termination handler of every frame on the way with `record` marked as
/// unwinding (and the target's as its target), and returns the target frame's registers.
pub fn unwind(
    start: &Context,
    start_unwound: bool,
    target: &UnwindTarget,
    record: *mut ExceptionRecord,
) -> Context {
    let unwind_slot = slot(teb::UNWIND_TARGET);
    // SAFETY: this thread's TEB word.
    let previous = unsafe { unwind_slot.read() };
    // SAFETY: as above; `target` outlives the walk, after which the slot is set back.
    unsafe { unwind_slot.write(target as *const UnwindTarget as u64) };
    let exit = if target.frame == 0 {
        flag::EXIT_UNWIND
    } else {
        0
    };
    // SAFETY: the caller's record.
    unsafe { (*record).flags |= flag::UNWINDING | exit };
    let kinds = HandlerKinds {
        exception: false,
        termination: true,
    };
    let mut walk = Walk::new(start, start_unwound, kinds);
    let reached = loop {
        match walk.next() {
            Event::End => crate::fault_handler::end_with_exception(
                match target.frame {
                    0 => status::UNWIND,
                    _ => status::INVALID_UNWIND_TARGET,
                },
                start.pc(),
            ),
            Event::Bridge(bridge) => {
                if let Some(catch) = bridge.catch.as_ref() {
                    cxx::leave_catch(catch, record);
                    if catch.parent_frame == target.frame {
                        break catch.parent_context;
                    }
                }
            }
            Event::Frame(frame) => {
                let is_target = frame.establisher == target.frame;
                if let Some(handler) = frame.handler {
                    // SAFETY: the caller's record.
                    let flags = unsafe { (*record).flags };
                    let marked = match is_target {
                        true => flags | flag::TARGET_UNWIND,
                        false => flags,
                    };
                    // SAFETY: as above.
                    unsafe { (*record).flags = marked };
                    let mut frame_context = frame.context;
                    let mut dispatcher =
                        DispatcherContext::of(&frame, &mut frame_context, target.ip);
                    let mut bridge = Bridge::new(Some(frame.caller), None);
                    // SAFETY: the frame's language handler, called as Windows calls it to
                    // unwind the frame.
                    unsafe {
                        frames::call_program(
                            &mut bridge,
                            handler,
                            [
                                record as u64,
                                frame.establisher,
                                &raw mut frame_context as u64,
                                &raw mut dispatcher as u64,
                            ],
                        )
                    };
                    // SAFETY: as above.
                    unsafe { (*record).flags = flags };
                }
                if is_target {
                    break frame.context;
                }
            }
        }
    };
    // SAFETY: as when it was set.
    unsafe { unwind_slot.write(previous) };
    reached
}

/// Points the faulting thread of the signal frame `ucontext` at the dispatch of `exception`:
/// its record and `CONTEXT` go on the thread's stack below the faulting frame, and the frame
/// is rewritten so that returning from the signal handler enters `dispatch_fault` with them.
pub fn deliver_fault(ucontext: *mut u8, exception: &fault::Exception, signal: i32) {
    // SAFETY: the kernel's `ucontext_t` for this signal.
    let mut context = unsafe { Context::from_signal(ucontext) };
    context.set_pc(exception.address);
    let parameters = &exception.parameters[..exception.parameter_count as usize];
    let record = ExceptionRecord::new(exception.code, 0, exception.address, parameters);
    let context_size = core::mem::size_of::<Context>() as u64;
    let block_size =
        (context_size + core::mem::size_of::<ExceptionRecord>() as u64).next_multiple_of(16);
    let block = context.sp().wrapping_sub(STACK_GAP + block_size) & !15;
    let context_at = block as *mut Context;
    let record_at = (block + context_size) as *mut ExceptionRecord;
    // SAFETY: the block lies on the thread's stack below the faulting frame's stack pointer
    // (and the gap Windows' conventions leave free), which nothing uses; it is 16-byte aligned.
    unsafe {
        context_at.write(context);
        record_at.write(record);
    }
    let arguments = [record_at as u64, context_at as u64, signal as u64];
    let put = |offset: usize, value: u64| {
        // SAFETY: the offsets are the frame's `sp`, `pc` and argument registers, which
        // `rt_sigreturn` loads back.
        unsafe { ucontext.add(offset).cast::<u64>().write_unaligned(value) };
    };
    put(UCONTEXT_SP, block);
    put(UCONTEXT_PC, fault_entry as usize as u64);
    for (offset, value) in UCONTEXT_ARGUMENTS.iter().zip(arguments) {
        put(*offset, value);
    }
}

/// Where a fault's dispatch starts (through `fault_entry`).
pub extern "C" fn dispatch_fault(
    record: *mut ExceptionRecord,
    context: *mut Context,
    signal: i32,
) -> ! {
    dispatch(record, context, Origin::Fault(signal))
}

capturing_entry!(pub(super) raise_exception_entry => raise_exception);
capturing_entry!(pub(super) raise_record_entry => raise_record);
capturing_entry!(pub(super) unwind_ex_entry => unwind_ex);
capturing_entry!(pub(super) unwind_entry => unwind_ex);
capturing_entry!(pub(super) local_unwind_entry => local_unwind);

extern "C" fn raise_exception(
    code: u64,
    flags: u64,
    count: u64,
    parameters: u64,
    context: *mut Context,
) -> ! {
    // SAFETY: the entry's `CONTEXT`.
    let pc = unsafe { (*context).pc() };
    let mut record =
        ExceptionRecord::new(code as u32, flags as u32 & flag::NONCONTINUABLE, pc, &[]);
    if parameters != 0 {
        let count = (count as u32 as usize).min(MAX_PARAMETERS);
        for (index, parameter) in record.parameters.iter_mut().take(count).enumerate() {
            // SAFETY: `RaiseException`'s contract: `count` parameters at `parameters`.
            *parameter = unsafe { (parameters as *const u64).add(index).read_unaligned() };
        }
        record.parameter_count = count as u32;
    }
    dispatch(&raw mut record, context, Origin::Raised)
}

extern "C" fn raise_record(record: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    // SAFETY: `RtlRaiseException`'s contract: a readable `EXCEPTION_RECORD`.
    let mut own = unsafe { (record as *const ExceptionRecord).read_unaligned() };
    // SAFETY: the entry's `CONTEXT`.
    own.address = unsafe { (*context).pc() };
    own.parameter_count = own.parameter_count.min(MAX_PARAMETERS as u32);
    dispatch(&raw mut own, context, Origin::Raised)
}

extern "C" fn unwind_ex(frame: u64, ip: u64, record: u64, value: u64, context: *mut Context) -> ! {
    // SAFETY: the entry's `CONTEXT`.
    let start = unsafe { *context };
    let mut own = match record {
        0 => ExceptionRecord::new(status::UNWIND, 0, start.pc(), &[]),
        // SAFETY: `RtlUnwindEx`'s contract: null or a readable `EXCEPTION_RECORD`.
        _ => unsafe { (record as *const ExceptionRecord).read_unaligned() },
    };
    let target = UnwindTarget {
        frame,
        ip,
        state: NO_STATE,
    };
    let mut reached = unwind(&start, true, &target, &raw mut own);
    reached.set_pc(ip);
    reached.set_return_value(value);
    resume(&reached)
}

/// `_local_unwind`: unwinds the caller's own frame to `ip`, running the termination handlers
/// of the scopes it leaves, and continues there.
extern "C" fn local_unwind(frame: u64, ip: u64, _: u64, _: u64, context: *mut Context) -> ! {
    unwind_ex(frame, ip, 0, 0, context)
}

pub(super) extern "efiapi" fn lookup_function_entry(pc: u64, image_base: *mut u64, _: u64) -> u64 {
    let Some(function) = frames::lookup(pc) else {
        return 0;
    };
    let base = frames::image().base;
    if !image_base.is_null() {
        // SAFETY: `RtlLookupFunctionEntry`'s contract: a writable word for the image base.
        unsafe { image_base.write_unaligned(base) };
    }
    base + function.entry
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "efiapi" fn virtual_unwind(
    kind: u32,
    image_base: u64,
    pc: u64,
    _function_entry: u64,
    context: *mut Context,
    handler_data: *mut u64,
    establisher: *mut u64,
    _context_pointers: u64,
) -> u64 {
    let image = frames::image();
    let Some(function) = frames::lookup(pc) else {
        return 0;
    };
    if context.is_null() || image_base != image.base {
        return 0;
    }
    // SAFETY: `RtlVirtualUnwind`'s contract: a `CONTEXT` the caller owns.
    let context = unsafe { &mut *context };
    let mut copy = *context;
    copy.set_pc(pc);
    let mut registers = copy.registers();
    let kinds = HandlerKinds::from_flags(kind);
    match unwinder::virtual_unwind(&image, function, kinds, &mut registers, &ThreadMemory) {
        Ok(unwound) => {
            context.set_registers(&registers);
            if !handler_data.is_null() {
                // SAFETY: the caller's word for the handler data.
                unsafe { handler_data.write_unaligned(unwound.handler_data) };
            }
            if !establisher.is_null() {
                // SAFETY: the caller's word for the establisher frame.
                unsafe { establisher.write_unaligned(unwound.establisher) };
            }
            unwound.handler.unwrap_or(0)
        }
        Err(_) => 0,
    }
}

pub(super) unsafe extern "efiapi" fn pc_to_file_header(pc: u64, base: *mut u64) -> u64 {
    let image = frames::image();
    let found = match image.rva_of(pc) {
        Some(_) => image.base,
        None => 0,
    };
    if !base.is_null() {
        // SAFETY: `RtlPcToFileHeader`'s contract: a writable word for the base.
        unsafe { base.write_unaligned(found) };
    }
    found
}

pub(super) extern "efiapi" fn set_unhandled_exception_filter(filter: usize) -> usize {
    UNHANDLED_EXCEPTION_FILTER.swap(filter, Ordering::AcqRel)
}

pub(super) extern "efiapi" fn unhandled_exception_filter(_pointers: usize) -> i32 {
    filter::EXECUTE_HANDLER
}
