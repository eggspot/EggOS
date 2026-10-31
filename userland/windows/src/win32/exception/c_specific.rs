//! `__C_specific_handler`, the language handler of C's `__try` blocks (D177). The handler
//! data is the function's scope table: a count, then for each scope (innermost first) the
//! image-relative start and end of its `__try` body, its filter (or 1, which is
//! `EXCEPTION_EXECUTE_HANDLER` itself) or `__finally` block, and the `__except` block it
//! continues at (none for a `__finally`). While dispatching, the filters of the scopes around
//! the frame's instruction decide: execute (unwind to the `__except` block, which gets the
//! exception code), continue execution, or continue the search. While unwinding, the
//! `__finally` blocks of the scopes left run, with `AbnormalTermination()` true, up to the
//! target scope of an unwind to this frame.

use super::context::Context;
use super::dispatch::{self, NO_STATE, UnwindTarget};
use super::frames::{self, Bridge};
use super::{DispatcherContext, ExceptionPointers, ExceptionRecord, disposition, filter, flag};
use crate::library::Export;

const SCOPE_SIZE: u64 = 16;
const MAX_SCOPES: u32 = 4096;
const FILTER_EXECUTES: u32 = 1;
const ABNORMAL_TERMINATION: u64 = 1;

struct Scope {
    begin: u32,
    end: u32,
    handler: u32,
    jump: u32,
}

impl Scope {
    fn at(table: u64, index: u32) -> Self {
        let entry = (table + 4 + u64::from(index) * SCOPE_SIZE) as *const u32;
        // SAFETY: the scope table the program's unwind data names holds `count` entries of
        // four image-relative addresses each, after the count.
        let word = |offset: usize| unsafe { entry.wrapping_add(offset).read_unaligned() };
        Self {
            begin: word(0),
            end: word(1),
            handler: word(2),
            jump: word(3),
        }
    }

    fn covers(&self, rva: u64) -> bool {
        (u64::from(self.begin)..u64::from(self.end)).contains(&rva)
    }
}

fn call(target: u64, first: u64, frame: u64, frame_pointer: u64) -> u64 {
    let mut bridge = Bridge::new(None, None);
    // SAFETY: `target` is a filter or `__finally` block of the program's scope table, of the
    // function whose frame is `frame`, which takes these two arguments.
    unsafe { frames::call_funclet(&mut bridge, target, first, frame, frame_pointer) }
}

fn execute(record: *mut ExceptionRecord, frame: u64, ip: u64) -> ! {
    let Some(dispatch) = dispatch::current_dispatch() else {
        crate::fault_handler::end_with_exception(super::status::INVALID_UNWIND_TARGET, ip);
    };
    // SAFETY: the dispatch's `CONTEXT`, in a frame below this one.
    let start = unsafe { *dispatch.context };
    // SAFETY: the dispatch's record, in a frame below this one.
    let code = unsafe { (*record).code };
    let target = UnwindTarget {
        frame,
        ip,
        state: NO_STATE,
    };
    let mut reached = dispatch::unwind(&start, dispatch.start_unwound, &target, record);
    reached.set_pc(ip);
    reached.set_return_value(u64::from(code));
    dispatch::resume(&reached)
}

unsafe extern "efiapi" fn c_specific_handler(
    record: *mut ExceptionRecord,
    frame: u64,
    context: *mut Context,
    dispatcher: *mut DispatcherContext,
) -> i32 {
    // SAFETY: the handler's contract.
    let dispatcher = unsafe { &mut *dispatcher };
    let table = dispatcher.handler_data;
    // SAFETY: as above; the scope table starts with its count.
    let count = unsafe { (table as *const u32).read_unaligned() }.min(MAX_SCOPES);
    let pc = dispatcher.control_rva();
    let base = dispatcher.image_base;
    // SAFETY: the dispatcher context's `CONTEXT` holds the frame's registers.
    let frame_pointer = frames::frame_pointer(unsafe { &*dispatcher.context_record });
    // SAFETY: as above.
    let flags = unsafe { (*record).flags };
    if flags & (flag::UNWINDING | flag::EXIT_UNWIND) == 0 {
        for index in dispatcher.scope_index..count {
            let scope = Scope::at(table, index);
            if !scope.covers(pc) || scope.jump == 0 {
                continue;
            }
            let verdict = match scope.handler {
                FILTER_EXECUTES => filter::EXECUTE_HANDLER,
                filter_rva => {
                    let mut pointers = ExceptionPointers { record, context };
                    let pointers = (&raw mut pointers) as u64;
                    call(base + u64::from(filter_rva), pointers, frame, frame_pointer) as i32
                }
            };
            if verdict < 0 {
                return disposition::CONTINUE_EXECUTION;
            }
            if verdict > 0 {
                execute(record, frame, base + u64::from(scope.jump));
            }
        }
    } else {
        let target = dispatcher.target_ip.wrapping_sub(base);
        let is_target = flags & flag::TARGET_UNWIND != 0;
        for index in dispatcher.scope_index..count {
            let scope = Scope::at(table, index);
            if !scope.covers(pc) {
                continue;
            }
            let continues_here = scope.jump != 0 && u64::from(scope.jump) == target;
            if is_target && (continues_here || scope.covers(target)) {
                break;
            }
            if scope.jump == 0 {
                dispatcher.scope_index = index + 1;
                call(
                    base + u64::from(scope.handler),
                    ABNORMAL_TERMINATION,
                    frame,
                    frame_pointer,
                );
            }
        }
    }
    disposition::CONTINUE_SEARCH
}

pub(super) const EXPORTS: &[Export] = exports![
    b"__C_specific_handler" => c_specific_handler,
    b"_local_unwind" => dispatch::local_unwind_entry [direct],
];
