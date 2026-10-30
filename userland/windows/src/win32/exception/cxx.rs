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
//! in its callers, with that function's destructors run once.

use super::super::heap;
use super::super::thread_environment::{self, teb};
use super::context::{Context, capturing_entry};
use super::dispatch::{self, NO_STATE, Origin, UnwindTarget};
use super::frames::{self, Bridge};
use super::{CatchState, DispatcherContext, ExceptionRecord, FrameData, disposition, flag};
use crate::cxx_data::{
    self, Action, CXX_EXCEPTION, CXX_MAGIC, CatchableType, FunctionData, Handler, ThrowInfo,
    TryBlock, flags as cxx_flags,
};
use crate::library::Export;
use crate::unwind::Image;

const MAGIC_LAST: u64 = 0x1993_0522;
const MAX_UNWIND_STEPS: usize = 1 << 16;
const TERMINATE_STATUS: u32 = 3;

fn is_cxx(record: &ExceptionRecord) -> bool {
    record.code == CXX_EXCEPTION
        && record.parameter_count >= 3
        && (CXX_MAGIC..=MAGIC_LAST).contains(&record.parameters[0])
}

/// Ends the process as `std::terminate` does, saying why.
fn terminate_with(reason: &[u8]) -> ! {
    super::super::crt::end_abnormally(reason, TERMINATE_STATUS)
}

fn innermost_catch() -> Option<&'static CatchState> {
    frames::bridges().find_map(|bridge| bridge.catch.as_ref())
}

fn slot(offset: usize) -> *mut u64 {
    (thread_environment::current() + offset) as *mut u64
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
    unsafe {
        slot(teb::CURRENT_EXCEPTION).write(record);
        slot(teb::CURRENT_EXCEPTION_CONTEXT).write(context);
    }
}

fn destroy_object(record: &ExceptionRecord) {
    if !is_cxx(record) || record.parameters[1] == 0 {
        return;
    }
    let image = frames::image();
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
}

/// An exception leaves the catch block `catch` (by `record`, being unwound): the object the
/// block caught is destroyed unless `record` is that object rethrown.
pub fn leave_catch(catch: &CatchState, record: *mut ExceptionRecord) {
    // SAFETY: the unwind's record.
    let leaving = unsafe { &*record };
    let rethrown = is_cxx(leaving) && leaving.parameters[1] == catch.record.parameters[1];
    if !rethrown {
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
    let mut record = ExceptionRecord::new(CXX_EXCEPTION, flag::NONCONTINUABLE, pc, &parameters);
    dispatch::dispatch(&raw mut record, context, Origin::Raised)
}

fn function_data(data: FrameData) -> Option<FunctionData<'static>> {
    let image = frames::image();
    match data.compressed {
        true => FunctionData::compressed(image, data.rva, data.function_begin),
        false => FunctionData::fixed(image, data.rva),
    }
}

/// Undoes the states of a frame from `from` until `to` (or all of them), calling each state's
/// destructor or unwind funclet.
fn unwind_states(function: &FunctionData<'_>, frame: u64, frame_pointer: u64, from: i32, to: i32) {
    let mut state = from;
    for _ in 0..MAX_UNWIND_STEPS {
        if state == to || state < 0 {
            break;
        }
        let Some(entry) = function.unwind_entry(state) else {
            break;
        };
        let mut bridge = Bridge::new(None, None);
        match entry.action {
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
        state = entry.to_state;
    }
}

fn adjust(object: u64, catchable: &CatchableType) -> u64 {
    let mut result = object.wrapping_add(catchable.member_displacement as i64 as u64);
    if catchable.virtual_base_pointer >= 0 {
        let base_pointer = object.wrapping_add(catchable.virtual_base_pointer as u64);
        // SAFETY: an object with a virtual base holds its virtual base table's address at
        // `virtual_base_pointer`, and the table the base's offset at the displacement.
        let offset = unsafe {
            let table = (base_pointer as *const u64).read_unaligned();
            let entry = table.wrapping_add(catchable.virtual_base_displacement as i64 as u64);
            (entry as *const i32).read_unaligned()
        };
        result = result
            .wrapping_add(offset as i64 as u64)
            .wrapping_add(catchable.virtual_base_pointer as u64);
    }
    result
}

/// Puts the thrown `object` into the catch block's parameter at `slot`: its address for a
/// reference, a copy of its bytes for a simple type (a pointer adjusted to the base it is
/// caught as), or a copy made by its copy constructor.
fn build_catch_object(
    image: &Image<'_>,
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
        if size == 8 && cxx_data::is_pointer_type(image, catchable) {
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

/// Catches the exception `record` in the frame `frame` with `handler` of `block`: unwinds to
/// the frame, builds the catch object, runs the catch block and resumes the function after it.
fn catch(
    record: *mut ExceptionRecord,
    function: &FunctionData<'_>,
    data: FrameData,
    frame: u64,
    block: &TryBlock,
    handler: &Handler,
    thrown: Option<CatchableType>,
) -> ! {
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
    let image = frames::image();
    if let Some(catchable) = thrown
        && handler.catch_object != 0
    {
        let slot = frame.wrapping_add(handler.catch_object);
        build_catch_object(&image, slot, record.parameters[1], handler, &catchable);
    }
    let enclosing_state = function
        .unwind_entry(block.low)
        .map_or(-1, |entry| entry.to_state);
    let state = CatchState {
        record: *record,
        exception_context: dispatch.context,
        parent_frame: frame,
        parent_context: parent,
        funclet: handler.funclet,
        enclosing_state,
        data,
    };
    let mut bridge = Bridge::new(frames::caller_of(&parent, true), Some(state));
    let frame_pointer = frames::frame_pointer(&parent);
    // SAFETY: this thread's TEB words; the record lives in the bridge while the block runs.
    unsafe {
        let caught = bridge
            .catch
            .as_ref()
            .map_or(0, |catch| &raw const catch.record as u64);
        slot(teb::CURRENT_EXCEPTION).write(caught);
        slot(teb::CURRENT_EXCEPTION_CONTEXT).write(dispatch.context as u64);
    }
    // SAFETY: the catch block's funclet, of the function whose frame is `frame`, which the
    // unwind left in the try block's state.
    let returned =
        unsafe { frames::call_funclet(&mut bridge, handler.funclet, frame, frame, frame_pointer) };
    let index = returned as usize;
    let continuation = match index < handler.continuation_count {
        true => handler.continuations[index],
        false => returned,
    };
    if let Some(caught) = bridge.catch.as_ref() {
        destroy_object(&caught.record);
    }
    let mut resumed = parent;
    resumed.set_pc(continuation);
    dispatch::resume(&resumed)
}

/// Searches the try blocks around `state` for a handler of `record`; catches it (and does
/// not return) if one is found.
fn search(
    record: *mut ExceptionRecord,
    function: &FunctionData<'_>,
    data: FrameData,
    frame: u64,
    state: i32,
) {
    // SAFETY: the dispatch's record.
    let exception = unsafe { &*record };
    let cxx = is_cxx(exception);
    if !cxx && function.synchronous_only() {
        return;
    }
    let image = frames::image();
    let info: Option<ThrowInfo> = match cxx {
        true => match cxx_data::throw_info(&image, exception.parameters[2]) {
            Some(info) => Some(info),
            None => return,
        },
        false => None,
    };
    for index in 0..function.try_count() {
        let Some(block) = function.try_block(index) else {
            break;
        };
        if state < block.low || state > block.high {
            continue;
        }
        for handler_index in 0..block.handler_count {
            let Some(handler) = function.handler(&block, handler_index, data.function_begin) else {
                break;
            };
            let Some(info) = info else {
                if handler.type_descriptor == 0 {
                    catch(record, function, data, frame, &block, &handler, None);
                }
                continue;
            };
            for type_index in 0..info.catchable_count {
                let Some(catchable) = cxx_data::catchable_type(&image, &info, type_index) else {
                    break;
                };
                if cxx_data::handler_catches(&image, &handler, &catchable, info.attributes) {
                    catch(
                        record,
                        function,
                        data,
                        frame,
                        &block,
                        &handler,
                        Some(catchable),
                    );
                }
            }
        }
    }
}

/// # Safety
/// Called as a language handler: a record, the frame's establisher, a `CONTEXT` and the
/// dispatcher context of a frame whose handler data starts with the image-relative address of
/// its C++ data, compressed when `compressed`.
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
    let running = frames::bridges()
        .find_map(|bridge| {
            bridge
                .catch
                .as_ref()
                .filter(|catch| catch.funclet == begin_address)
        })
        .copied();
    // SAFETY: the dispatcher context's `CONTEXT` holds the frame's registers.
    let own_context = unsafe { &*dispatcher.context_record };
    let (frame, frame_pointer, state, data) = match running {
        Some(catch) => {
            let state = match own_function.state_at(pc, function_begin) {
                -1 => catch.enclosing_state,
                state => state,
            };
            let frame_pointer = frames::frame_pointer(&catch.parent_context);
            (catch.parent_frame, frame_pointer, state, catch.data)
        }
        None if own_function.is_catch_funclet() => return disposition::CONTINUE_SEARCH,
        None => {
            let state = own_function.state_at(pc, function_begin);
            (establisher, frames::frame_pointer(own_context), state, own)
        }
    };
    let Some(function) = function_data(data) else {
        return disposition::CONTINUE_SEARCH;
    };
    // SAFETY: the handler's contract.
    let flags = unsafe { (*record).flags };
    if flags & (flag::UNWINDING | flag::EXIT_UNWIND) != 0 {
        let to = match dispatch::unwind_target() {
            Some(target) if target.frame == frame && target.state != NO_STATE => target.state,
            _ => -1,
        };
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

/// # Safety
/// As [`frame_handler`].
unsafe extern "efiapi" fn frame_handler_3(
    record: *mut ExceptionRecord,
    establisher: u64,
    _context: *mut Context,
    dispatcher: *mut DispatcherContext,
) -> i32 {
    // SAFETY: the caller's duty.
    unsafe { frame_handler(false, record, establisher, dispatcher) }
}

/// # Safety
/// As [`frame_handler`].
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
    let (from, to) = unsafe { (&*from, &mut *to) };
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
    unsafe { core::ffi::CStr::from_ptr(type_info.add(TYPE_INFO_NAME).cast()) }.to_bytes()
}

unsafe extern "efiapi" fn type_info_compare(left: *const u8, right: *const u8) -> i32 {
    let (left, right) = (decorated_name(left), decorated_name(right));
    let skip = |name: &'static [u8]| name.get(1..).unwrap_or(&[]);
    match skip(left).cmp(skip(right)) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

unsafe extern "efiapi" fn type_info_hash(type_info: *const u8) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let name = decorated_name(type_info);
    name.get(1..)
        .unwrap_or(&[])
        .iter()
        .fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
        })
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

/// `type_info`'s virtual function table, which every type descriptor the program has points
/// at: its scalar deleting destructor.
#[repr(C)]
struct VirtualTable([*const (); 1]);

// SAFETY: the table holds one function's address, immutable for the program's life.
unsafe impl Sync for VirtualTable {}

static TYPE_INFO_TABLE: VirtualTable = VirtualTable([export_entry!(type_info_delete)]);

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
];
