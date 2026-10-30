//! Structured exception handling for Windows programs (D177), as Windows does it on x64 and
//! ARM64: an exception, raised by the program (`RaiseException`, a C++ `throw`) or by the CPU
//! (an access violation, a division by zero, an illegal instruction, a breakpoint, which the
//! kernel signals and the layer turns into exceptions), goes first to the vectored handlers,
//! then up the program's frames to each function's language handler, found through the
//! unwind data in its image (`unwind/`), then to the unhandled-exception filter. A handler
//! that takes it unwinds the frames in between, running their termination handlers and
//! destructors, and resumes the thread in its own frame. The layer implements the language
//! handlers MSVC's code uses: `__C_specific_handler` for `__try`/`__except`/`__finally`
//! (`c_specific.rs`) and `__CxxFrameHandler3`/`__CxxFrameHandler4` for C++ (`cxx.rs`).

mod c_specific;
mod context;
mod cxx;
mod dispatch;
mod frames;
mod signal;
mod vectored;

use crate::library::Export;
use context::{Context, unwinder};

pub use dispatch::deliver_fault;
pub use frames::{in_layer, set_image};

/// Exception flags (`EXCEPTION_RECORD.ExceptionFlags`).
pub mod flag {
    pub const NONCONTINUABLE: u32 = 0x01;
    pub const UNWINDING: u32 = 0x02;
    pub const EXIT_UNWIND: u32 = 0x04;
    pub const TARGET_UNWIND: u32 = 0x20;
}

/// What a language handler returns (`EXCEPTION_DISPOSITION`).
pub mod disposition {
    pub const CONTINUE_EXECUTION: i32 = 0;
    pub const CONTINUE_SEARCH: i32 = 1;
}

/// What a filter returns.
pub mod filter {
    pub const EXECUTE_HANDLER: i32 = 1;
    pub const CONTINUE_SEARCH: i32 = 0;
    pub const CONTINUE_EXECUTION: i32 = -1;
}

/// Exception codes the layer raises itself.
pub mod status {
    pub const NONCONTINUABLE_EXCEPTION: u32 = 0xc000_0025;
    pub const UNWIND: u32 = 0xc000_0027;
    pub const INVALID_UNWIND_TARGET: u32 = 0xc000_0029;
}

/// The most parameters an exception record holds.
pub const MAX_PARAMETERS: usize = 15;

/// `EXCEPTION_RECORD`, 64-bit.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ExceptionRecord {
    pub code: u32,
    pub flags: u32,
    pub nested: u64,
    pub address: u64,
    pub parameter_count: u32,
    pub parameters: [u64; MAX_PARAMETERS],
}

const _: () = assert!(core::mem::size_of::<ExceptionRecord>() == 152);

impl ExceptionRecord {
    pub fn new(code: u32, flags: u32, address: u64, parameters: &[u64]) -> Self {
        let count = parameters.len().min(MAX_PARAMETERS);
        let mut all = [0; MAX_PARAMETERS];
        all[..count].copy_from_slice(&parameters[..count]);
        Self {
            code,
            flags,
            nested: 0,
            address,
            parameter_count: count as u32,
            parameters: all,
        }
    }
}

/// `EXCEPTION_POINTERS`, what a filter gets.
#[repr(C)]
pub struct ExceptionPointers {
    pub record: *mut ExceptionRecord,
    pub context: *mut Context,
}

/// `DISPATCHER_CONTEXT`, what a language handler gets about its frame (ARM64's layout, of
/// which x64's is the first 76 bytes; the layer sets `control_pc_is_unwound` on both).
#[repr(C)]
pub struct DispatcherContext {
    pub control_pc: u64,
    pub image_base: u64,
    pub function_entry: u64,
    pub establisher_frame: u64,
    pub target_ip: u64,
    pub context_record: *mut Context,
    pub language_handler: u64,
    pub handler_data: u64,
    pub history_table: u64,
    pub scope_index: u32,
    pub control_pc_is_unwound: u8,
    pub non_volatile_registers: u64,
}

const _: () = assert!(core::mem::offset_of!(DispatcherContext, scope_index) == 72);
const _: () = assert!(core::mem::offset_of!(DispatcherContext, control_pc_is_unwound) == 76);

impl DispatcherContext {
    fn of(frame: &Frame, context: &mut Context, target_ip: u64) -> Self {
        let image_base = frames::image().base;
        Self {
            control_pc: frame.context.pc(),
            image_base,
            function_entry: frame
                .function
                .map_or(0, |function| image_base + function.entry),
            establisher_frame: frame.establisher,
            target_ip,
            context_record: context,
            language_handler: frame.handler.unwrap_or(0),
            handler_data: frame.handler_data,
            history_table: 0,
            scope_index: 0,
            control_pc_is_unwound: u8::from(frame.unwound),
            non_volatile_registers: 0,
        }
    }

    /// The image-relative address of the instruction the frame is in: before the return
    /// address when the frame called out, so the call itself decides the scope or state.
    fn control_rva(&self) -> u64 {
        self.control_pc
            .wrapping_sub(self.image_base)
            .wrapping_sub(u64::from(self.control_pc_is_unwound != 0))
    }

    /// The image-relative start of the frame's function.
    fn function_begin(&self) -> u32 {
        match self.function_entry {
            0 => 0,
            // SAFETY: a dispatcher context's function entry is a `.pdata` entry of the image,
            // whose first word is the function's start.
            entry => unsafe { (entry as *const u32).read_unaligned() },
        }
    }
}

/// One frame a walk found: its registers, its caller's, its function, its language handler
/// and handler data, its establisher, and whether its `pc` is a return address.
#[derive(Clone, Copy)]
pub struct Frame {
    pub context: Context,
    pub caller: Context,
    pub function: Option<unwinder::Function>,
    pub handler: Option<u64>,
    pub handler_data: u64,
    pub establisher: u64,
    pub unwound: bool,
}

/// Which C++ data a function uses: `__CxxFrameHandler4`'s compressed data or
/// `__CxxFrameHandler3`'s, where, and where the function starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameData {
    pub compressed: bool,
    pub rva: u64,
    pub function_begin: u32,
}

/// A catch block the layer is running: the exception it handles, the frame of the function
/// it belongs to (its establisher, and its registers as the catch began), its funclet, the
/// state around its try block, and the function's C++ data.
#[derive(Clone, Copy)]
pub struct CatchState {
    pub record: ExceptionRecord,
    pub exception_context: *mut Context,
    pub parent_frame: u64,
    pub parent_context: Context,
    pub funclet: u64,
    pub enclosing_state: i32,
    pub data: FrameData,
}

/// The kernel32 functions of exception handling.
pub(super) const KERNEL32_EXPORTS: &[Export] = exports![
    b"RaiseException" => dispatch::raise_exception_entry [direct],
    b"RtlCaptureContext" => context::capture [direct],
    b"RtlRestoreContext" => context::restore [direct],
    b"RtlLookupFunctionEntry" => dispatch::lookup_function_entry,
    b"RtlVirtualUnwind" => dispatch::virtual_unwind,
    b"RtlUnwind" => dispatch::unwind_entry [direct],
    b"RtlUnwindEx" => dispatch::unwind_ex_entry [direct],
    b"RtlPcToFileHeader" => dispatch::pc_to_file_header,
    b"AddVectoredExceptionHandler" => vectored::add_exception_handler,
    b"RemoveVectoredExceptionHandler" => vectored::remove_exception_handler,
    b"AddVectoredContinueHandler" => vectored::add_continue_handler,
    b"RemoveVectoredContinueHandler" => vectored::remove_continue_handler,
    b"SetUnhandledExceptionFilter" => dispatch::set_unhandled_exception_filter,
    b"UnhandledExceptionFilter" => dispatch::unhandled_exception_filter,
];

/// ntdll's: the same functions under their native names.
pub(super) const NTDLL_EXPORTS: &[Export] = exports![
    b"RtlRaiseException" => dispatch::raise_record_entry [direct],
    b"RtlCaptureContext" => context::capture [direct],
    b"RtlRestoreContext" => context::restore [direct],
    b"RtlLookupFunctionEntry" => dispatch::lookup_function_entry,
    b"RtlVirtualUnwind" => dispatch::virtual_unwind,
    b"RtlUnwind" => dispatch::unwind_entry [direct],
    b"RtlUnwindEx" => dispatch::unwind_ex_entry [direct],
    b"RtlPcToFileHeader" => dispatch::pc_to_file_header,
    b"RtlAddVectoredExceptionHandler" => vectored::add_exception_handler,
    b"RtlRemoveVectoredExceptionHandler" => vectored::remove_exception_handler,
];

/// vcruntime's language handler of C's `__try` blocks and `_local_unwind`.
pub(super) const C_SPECIFIC_EXPORTS: &[Export] = c_specific::EXPORTS;

/// vcruntime's C++ functions: the throw, the frame handlers and their support.
pub(super) const CXX_EXPORTS: &[Export] = cxx::FUNCTIONS;

/// vcruntime's `type_info` virtual function table, which programs import as data.
pub(super) const TYPE_INFO_EXPORTS: &[Export] = &cxx::TYPE_INFO_TABLE_EXPORT;

/// The Universal C Runtime's C++ functions: `terminate` and `_callnewh`.
pub(super) const CXX_UCRT_EXPORTS: &[Export] = cxx::UCRT_EXPORTS;

/// The Universal C Runtime's `signal`, `raise` and the startup's exception filter.
pub(super) const SIGNAL_EXPORTS: &[Export] = signal::EXPORTS;
