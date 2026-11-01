//! C++ exceptions as MSVC's runtime gives them (D177): `_CxxThrowException` raises exception
//! `0xE06D7363` with the thrown object, its `ThrowInfo` and the image base; the language
//! handlers `__CxxFrameHandler3` and `__CxxFrameHandler4` read the function's C++ data
//! (`cxx_data.rs`), find the state of the frame's instruction, search the try blocks around it
//! for a catch handler whose type the object can be caught as, and catch it: unwind the frames
//! in between (their handlers run their destructors) and the catching frame down to its try
//! block, copy the object into the catch block's parameter, call the catch block's funclet,
//! destroy the object, and resume the function where the funclet says. A catch block runs
//! through a bridge that remembers its function, so `throw;` rethrows its object, and an
//! exception thrown inside it is caught by the try blocks around it, in the same function or
//! in its callers, with that function's destructors run once: by the funclet's handler, which
//! under `__CxxFrameHandler3` reads its function's data and states, or by the function's own
//! handler from the state around the catch's try block, once a `__CxxFrameHandler4` funclet
//! (whose data, if it has any, covers only its own locals) is left.

use super::super::heap;
use super::super::thread_environment::{self, teb};
use super::context::{Context, capturing_entry};
use super::dispatch::{self, Origin, UnwindTarget};
use super::frames::{self, Bridge};
use super::{
    CatchState, DispatcherContext, ExceptionRecord, FrameData, disposition, flag, status, thrown,
};
use crate::cxx_data::{
    self, Action, CXX_EXCEPTION, CXX_MAGIC, CatchableType, FunctionData, Handler, Images,
    flags as cxx_flags,
};
use crate::cxx_walk::{
    self, Caught, Displacement, FrameRole, ObjectMemory, RunningCatch, Thrown,
    OUTSIDE_EVERY_STATE,
};
use crate::library::Export;
use crate::unwind::Image;

const TERMINATE_STATUS: u32 = 3;

fn is_cxx(record: &ExceptionRecord) -> bool {
    cxx_walk::is_cxx_exception(record.code, record.parameters())
}

/// The program's memory, read where its own code would read it: an object's virtual base
/// table and virtual function table, and what they point at.
pub(super) struct ProgramMemory;

impl ObjectMemory for ProgramMemory {
    fn u64_at(&self, address: u64) -> u64 {
        // SAFETY: the address comes from an object of the program and the tables MSVC laid out
        // for its type; a wrong one faults as the program's own read would.
        unsafe { (address as *const u64).read_unaligned() }
    }

    fn i32_at(&self, address: u64) -> i32 {
        // SAFETY: as in `u64_at`.
        unsafe { (address as *const i32).read_unaligned() }
    }

    fn u8_at(&self, address: u64) -> u8 {
        // SAFETY: as in `u64_at`; a type descriptor's name is NUL-terminated.
        unsafe { (address as *const u8).read() }
    }
}

pub(super) fn terminate_with(reason: &[u8]) -> ! {
    super::super::crt::end_abnormally(reason, TERMINATE_STATUS)
}

/// The innermost catch block running on this thread.
pub(super) fn innermost_catch() -> Option<&'static CatchState> {
    frames::bridges().find_map(|bridge| bridge.catch.as_ref())
}

fn running_catch(matching: impl Fn(&CatchState) -> bool) -> Option<CatchState> {
    frames::bridges()
        .find_map(|bridge| bridge.catch.as_ref().filter(|catch| matching(catch)))
        .copied()
}

fn slot(offset: usize) -> *mut u64 {
    (thread_environment::current() + offset) as *mut u64
}

/// Counts a C++ exception thrown on this thread and not yet caught (`std::uncaught_exceptions`).
pub(super) fn count_thrown() {
    let count = slot(teb::UNCAUGHT_EXCEPTIONS);
    // SAFETY: this thread's TEB word.
    unsafe { count.write(count.read().wrapping_add(1)) };
}

fn count_caught() {
    let count = slot(teb::UNCAUGHT_EXCEPTIONS);
    // SAFETY: this thread's TEB word.
    unsafe { count.write(count.read().saturating_sub(1)) };
}

fn uncaught() -> u64 {
    // SAFETY: this thread's TEB word.
    unsafe { slot(teb::UNCAUGHT_EXCEPTIONS).read() }
}

extern "efiapi" fn uncaught_exceptions() -> i32 {
    i32::try_from(uncaught()).unwrap_or(i32::MAX)
}

extern "efiapi" fn uncaught_exception() -> bool {
    uncaught() != 0
}

/// Points `__current_exception` and `__current_exception_context` at the exception of the
/// innermost catch block still running on this thread, or at nothing.
pub fn refresh_current_exception() {
    let (record, context) = match innermost_catch() {
        Some(catch) => (
            &raw const catch.record as u64,
            catch.exception_context as u64,
        ),
        None => (0, 0),
    };
    // SAFETY: this thread's TEB words.
    unsafe { slot(teb::CURRENT_EXCEPTION).write(record) };
    // SAFETY: as above.
    unsafe { slot(teb::CURRENT_EXCEPTION_CONTEXT).write(context) };
}

fn destroy_object(record: &ExceptionRecord) {
    if !is_cxx(record) || record.parameters[1] == 0 {
        return;
    }
    let image = thrown::image_of(record);
    let Some(info) = cxx_data::throw_info(&image, record.parameters[2]) else {
        return;
    };
    if info.destructor != 0 {
        let mut bridge = Bridge::new(None, None);
        // SAFETY: the thrown type's destructor, from its `ThrowInfo`, on the thrown object.
        unsafe {
            frames::call_program(
                &mut bridge,
                info.destructor,
                [record.parameters[1], 0, 0, 0],
            )
        };
    }
    super::exception_ptr::forget_rethrown_copy(record.parameters[1]);
}

/// An exception leaves the catch block `catch` (by `record`, being unwound): the object the
/// block caught is destroyed unless `record` is that object rethrown.
pub fn leave_catch(catch: &CatchState, record: *mut ExceptionRecord) {
    // SAFETY: the unwind's record.
    let leaving = unsafe { &*record };
    if cxx_walk::leaving_destroys_caught(
        catch.record.parameters[1],
        is_cxx(leaving),
        leaving.parameters[1],
    ) {
        destroy_object(&catch.record);
    }
}

capturing_entry!(throw_entry => throw);

extern "C" fn throw(object: u64, info: u64, _: u64, _: u64, context: *mut Context) -> ! {
    // SAFETY: the entry's `CONTEXT`.
    let pc = unsafe { (*context).pc() };
    let parameters = match (object, info) {
        (0, 0) => match innermost_catch() {
            Some(catch) => [
                catch.record.parameters[0],
                catch.record.parameters[1],
                catch.record.parameters[2],
                catch.record.parameters[3],
            ],
            None => terminate_with(b"the program rethrew with no exception being handled"),
        },
        _ => [CXX_MAGIC, object, info, frames::image().base],
    };
    count_thrown();
    let mut record = ExceptionRecord::new(CXX_EXCEPTION, flag::NONCONTINUABLE, pc, &parameters);
    dispatch::dispatch(&raw mut record, context, Origin::Raised)
}

fn function_data(data: FrameData) -> Option<FunctionData<'static>> {
    data.read(frames::image())
}

fn unwind_states(function: &FunctionData<'_>, frame: u64, frame_pointer: u64, from: i32, to: i32) {
    for action in cxx_walk::unwind_actions(*function, from, to) {
        let mut bridge = Bridge::new(None, None);
        match action {
            Action::Nothing => {}
            Action::Funclet(address) => {
                // SAFETY: an unwind funclet of the frame's function, which is still on the
                // stack.
                unsafe { frames::call_funclet(&mut bridge, address, frame, frame, frame_pointer) };
            }
            Action::Destructor {
                function,
                object,
                through_pointer,
            } => {
                let at = frame.wrapping_add(object);
                let this = match through_pointer {
                    // SAFETY: the frame's slot that holds the object's address.
                    true => unsafe { (at as *const u64).read_unaligned() },
                    false => at,
                };
                // SAFETY: the object's destructor, from the function's unwind map, on the
                // object in the frame.
                unsafe { frames::call_program(&mut bridge, function, [this, 0, 0, 0]) };
            }
        }
    }
}

fn adjust(object: u64, catchable: &CatchableType) -> u64 {
    Displacement::of(catchable).apply(object, &ProgramMemory)
}

fn build_catch_object(
    thrown_image: &Image<'_>,
    slot: u64,
    object: u64,
    handler: &Handler,
    catchable: &CatchableType,
) {
    let adjusted = adjust(object, catchable);
    let size = catchable.size as usize;
    if handler.adjectives & cxx_flags::HANDLER_REFERENCE != 0 {
        // SAFETY: the catch block's parameter slot in the catching frame.
        unsafe { (slot as *mut u64).write_unaligned(adjusted) };
        return;
    }
    if catchable.properties & cxx_flags::CATCHABLE_SIMPLE_TYPE != 0 {
        // SAFETY: the parameter slot holds an object of the catchable type's size, as the
        // thrown object does.
        unsafe { core::ptr::copy_nonoverlapping(object as *const u8, slot as *mut u8, size) };
        if size == 8 && cxx_data::is_pointer_type(thrown_image, catchable) {
            // SAFETY: as above; the slot holds a pointer.
            let value = unsafe { (slot as *const u64).read_unaligned() };
            if value != 0 {
                // SAFETY: as above.
                unsafe { (slot as *mut u64).write_unaligned(adjust(value, catchable)) };
            }
        }
        return;
    }
    match catchable.copy_function {
        0 => {
            // SAFETY: as above, for a class with no copy constructor of its own.
            unsafe { core::ptr::copy_nonoverlapping(adjusted as *const u8, slot as *mut u8, size) }
        }
        copy => {
            let virtual_base = catchable.properties & cxx_flags::CATCHABLE_VIRTUAL_BASE != 0;
            let mut bridge = Bridge::new(None, None);
            // SAFETY: the type's copy constructor, constructing the parameter from the object
            // (with the flag that says the most derived object constructs its virtual bases).
            unsafe {
                frames::call_program(
                    &mut bridge,
                    copy,
                    [slot, adjusted, u64::from(virtual_base), 0],
                )
            };
        }
    }
}

fn catch(
    record: *mut ExceptionRecord,
    function: &FunctionData<'_>,
    data: FrameData,
    frame: u64,
    caught: Caught,
) -> ! {
    let Caught {
        block,
        handler,
        catchable: thrown,
    } = caught;
    let Some(dispatch) = dispatch::current_dispatch() else {
        terminate_with(b"a C++ exception was caught outside a dispatch");
    };
    // SAFETY: the dispatch's `CONTEXT`, in a frame below this one.
    let start = unsafe { *dispatch.context };
    let target = UnwindTarget {
        frame,
        ip: handler.funclet,
        state: block.low,
    };
    let parent = dispatch::unwind(&start, dispatch.start_unwound, &target, record);
    // SAFETY: the dispatch's record.
    let record = unsafe { &mut *record };
    record.flags &= !(flag::UNWINDING | flag::EXIT_UNWIND | flag::TARGET_UNWIND);
    let thrown_image = thrown::image_of(record);
    if let Some(catchable) = thrown
        && handler.catch_object != 0
    {
        let slot = frame.wrapping_add(handler.catch_object);
        build_catch_object(&thrown_image, slot, record.parameters[1], &handler, &catchable);
    }
    let enclosing_state = function
        .unwind_entry(block.low)
        .map_or(OUTSIDE_EVERY_STATE, |entry| entry.to_state);
    let state = CatchState {
        record: *record,
        exception_context: dispatch.context,
        parent_frame: frame,
        parent_context: parent,
        funclet: handler.funclet,
        enclosing_state,
        data,
    };
    let mut bridge = Bridge::new(None, Some(state));
    let frame_pointer = frames::frame_pointer(&parent);
    let caught = bridge
        .catch
        .as_ref()
        .map_or(0, |catch| &raw const catch.record as u64);
    // SAFETY: this thread's TEB words; the record lives in the bridge while the block runs.
    unsafe { slot(teb::CURRENT_EXCEPTION).write(caught) };
    // SAFETY: as above.
    unsafe { slot(teb::CURRENT_EXCEPTION_CONTEXT).write(dispatch.context as u64) };
    if is_cxx(record) {
        count_caught();
    }
    // SAFETY: the catch block's funclet, of the function whose frame is `frame`, which the
    // unwind left in the try block's state.
    let returned =
        unsafe { frames::call_funclet(&mut bridge, handler.funclet, frame, frame, frame_pointer) };
    let continuation = cxx_walk::continuation(&handler, returned);
    if let Some(caught) = bridge.catch.as_ref() {
        destroy_object(&caught.record);
    }
    let mut resumed = parent;
    resumed.set_pc(continuation);
    dispatch::resume(&resumed)
}

fn search(
    record: *mut ExceptionRecord,
    function: &FunctionData<'_>,
    data: FrameData,
    frame: u64,
    state: i32,
) {
    // SAFETY: the dispatch's record.
    let exception = unsafe { &*record };
    let images = Images {
        handler: frames::image(),
        thrown: thrown::image_of(exception),
    };
    let thrown = match is_cxx(exception) {
        true => match cxx_data::throw_info(&images.thrown, exception.parameters[2]) {
            Some(info) => Thrown::Cxx(info),
            None => return,
        },
        false => Thrown::Structured,
    };
    if let Some(caught) =
        cxx_walk::find_handler(&images, function, data.function_begin, state, thrown)
    {
        catch(record, function, data, frame, caught);
    }
}

unsafe fn frame_handler(
    compressed: bool,
    record: *mut ExceptionRecord,
    establisher: u64,
    dispatcher: *mut DispatcherContext,
) -> i32 {
    // SAFETY: the handler's contract.
    let dispatcher = unsafe { &*dispatcher };
    let image = frames::image();
    let function_begin = dispatcher.function_begin();
    // SAFETY: as above; the handler data starts with the C++ data's address.
    let rva = u64::from(unsafe { (dispatcher.handler_data as *const u32).read_unaligned() });
    let own = FrameData {
        compressed,
        rva,
        function_begin,
    };
    let Some(own_function) = function_data(own) else {
        return disposition::CONTINUE_SEARCH;
    };
    let pc = dispatcher.control_rva();
    let begin_address = image.base + u64::from(function_begin);
    // SAFETY: the handler's contract.
    let unwinding = unsafe { (*record).flags } & (flag::UNWINDING | flag::EXIT_UNWIND) != 0;
    // SAFETY: the dispatcher context's `CONTEXT` holds the frame's registers.
    let own_context = unsafe { &*dispatcher.context_record };
    let own_state = own_function.state_at(pc, function_begin);
    let funclet_catch = running_catch(|catch| catch.funclet == begin_address);
    let catch_of_frame = match funclet_catch {
        Some(_) => None,
        None => running_catch(|catch| catch.parent_frame == establisher),
    };
    let role = cxx_walk::frame_role(
        own,
        own_state,
        own_function.is_catch_funclet(),
        establisher,
        funclet_catch.as_ref().map(running),
        catch_of_frame.as_ref().map(running),
    );
    let catch_frame_pointer =
        || funclet_catch.map_or(0, |catch| frames::frame_pointer(&catch.parent_context));
    let (frame, frame_pointer, state, data) = match role {
        FrameRole::Function {
            frame,
            state,
            data,
            in_catch_function: true,
        } => (frame, catch_frame_pointer(), state, data),
        FrameRole::Function {
            frame, state, data, ..
        } => (frame, frames::frame_pointer(own_context), state, data),
        FrameRole::FuncletLocals { frame, from } => {
            if unwinding {
                unwind_states(
                    &own_function,
                    frame,
                    catch_frame_pointer(),
                    from,
                    OUTSIDE_EVERY_STATE,
                );
            }
            return disposition::CONTINUE_SEARCH;
        }
        FrameRole::Nothing => return disposition::CONTINUE_SEARCH,
    };
    let Some(function) = function_data(data) else {
        return disposition::CONTINUE_SEARCH;
    };
    if unwinding {
        // SAFETY: the handler's contract.
        let long_jump = unsafe { (*record).code } == status::LONGJMP;
        let to = cxx_walk::state_to_unwind_to(
            &function,
            data.function_begin,
            image.base,
            frame,
            dispatch::unwind_target(),
            long_jump,
        );
        unwind_states(&function, frame, frame_pointer, state, to);
        return disposition::CONTINUE_SEARCH;
    }
    search(record, &function, data, frame, state);
    // SAFETY: as above.
    if function.is_noexcept() && is_cxx(unsafe { &*record }) {
        terminate_with(b"an exception left a noexcept function");
    }
    disposition::CONTINUE_SEARCH
}

fn running(catch: &CatchState) -> RunningCatch {
    RunningCatch {
        parent_frame: catch.parent_frame,
        enclosing_state: catch.enclosing_state,
        data: catch.data,
    }
}

unsafe extern "efiapi" fn frame_handler_3(
    record: *mut ExceptionRecord,
    establisher: u64,
    _context: *mut Context,
    dispatcher: *mut DispatcherContext,
) -> i32 {
    // SAFETY: the caller's duty.
    unsafe { frame_handler(false, record, establisher, dispatcher) }
}

unsafe extern "efiapi" fn frame_handler_4(
    record: *mut ExceptionRecord,
    establisher: u64,
    _context: *mut Context,
    dispatcher: *mut DispatcherContext,
) -> i32 {
    // SAFETY: the caller's duty.
    unsafe { frame_handler(true, record, establisher, dispatcher) }
}

#[repr(C)]
struct ExceptionData {
    what: *mut u8,
    do_free: u8,
}

unsafe extern "efiapi" fn std_exception_copy(from: *const ExceptionData, to: *mut ExceptionData) {
    // SAFETY: `std::exception`'s copy: two `__std_exception_data`.
    let from = unsafe { &*from };
    // SAFETY: as above.
    let to = unsafe { &mut *to };
    if from.do_free == 0 || from.what.is_null() {
        to.what = from.what;
        to.do_free = 0;
        return;
    }
    // SAFETY: an owned message is a NUL-terminated string.
    let length = unsafe { core::ffi::CStr::from_ptr(from.what.cast()) }
        .to_bytes_with_nul()
        .len();
    let copy = heap::allocate(length);
    if copy.is_null() {
        to.what = core::ptr::null_mut();
        to.do_free = 0;
        return;
    }
    // SAFETY: both hold `length` bytes.
    unsafe { core::ptr::copy_nonoverlapping(from.what, copy, length) };
    to.what = copy;
    to.do_free = 1;
}

unsafe extern "efiapi" fn std_exception_destroy(data: *mut ExceptionData) {
    // SAFETY: `std::exception`'s destructor: its `__std_exception_data`.
    let data = unsafe { &mut *data };
    if data.do_free != 0 {
        // SAFETY: an owned message came from this heap (`__std_exception_copy`).
        unsafe { heap::free(data.what) };
    }
    data.what = core::ptr::null_mut();
    data.do_free = 0;
}

extern "efiapi" fn std_terminate() -> ! {
    terminate_with(b"the program called terminate")
}

extern "efiapi" fn call_new_handler(_size: usize) -> i32 {
    0
}

const TYPE_INFO_NAME: usize = 16;

fn decorated_name(type_info: *const u8) -> &'static [u8] {
    // SAFETY: a `type_info` holds its decorated name, NUL-terminated, after two pointers.
    unsafe { core::ffi::CStr::from_ptr(type_info.wrapping_add(TYPE_INFO_NAME).cast()) }.to_bytes()
}

unsafe extern "efiapi" fn type_info_compare(left: *const u8, right: *const u8) -> i32 {
    match crate::rtti::compare_decorated(decorated_name(left), decorated_name(right)) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

unsafe extern "efiapi" fn type_info_hash(type_info: *const u8) -> u64 {
    crate::rtti::hash_decorated(decorated_name(type_info))
}

extern "efiapi" fn type_info_destroy(_type_info: u64) {}

extern "efiapi" fn type_info_delete(type_info: u64, _flags: u32) -> u64 {
    type_info
}

extern "efiapi" fn current_exception() -> *mut u64 {
    slot(teb::CURRENT_EXCEPTION)
}

extern "efiapi" fn current_exception_context() -> *mut u64 {
    slot(teb::CURRENT_EXCEPTION_CONTEXT)
}

#[repr(C)]
pub(super) struct VirtualTable([*const (); 1]);

// SAFETY: the table holds one function's address, immutable for the program's life.
unsafe impl Sync for VirtualTable {}

pub(super) static TYPE_INFO_TABLE: VirtualTable = VirtualTable([export_entry!(type_info_delete)]);

pub(super) static TYPE_INFO_TABLE_EXPORT: [Export; 1] = [Export {
    name: b"??_7type_info@@6B@",
    address: &raw const TYPE_INFO_TABLE as *const (),
}];

pub(super) const FUNCTIONS: &[Export] = exports![
    b"_CxxThrowException" => throw_entry [direct],
    b"__CxxFrameHandler3" => frame_handler_3,
    b"__CxxFrameHandler4" => frame_handler_4,
    b"__std_exception_copy" => std_exception_copy,
    b"__std_exception_destroy" => std_exception_destroy,
    b"__std_terminate" => std_terminate,
    b"__std_type_info_compare" => type_info_compare,
    b"__std_type_info_hash" => type_info_hash,
    b"__current_exception" => current_exception,
    b"__current_exception_context" => current_exception_context,
    b"??1type_info@@UEAA@XZ" => type_info_destroy,
];

/// The Universal C Runtime's C++ functions.
pub(super) const UCRT_EXPORTS: &[Export] = exports![
    b"terminate" => std_terminate,
    b"_callnewh" => call_new_handler,
    b"__uncaught_exceptions" => uncaught_exceptions,
    b"__uncaught_exception" => uncaught_exception,
];
