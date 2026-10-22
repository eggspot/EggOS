//! What the kernel does with a CPU exception, on every architecture.
//!
//! The HAL decodes each exception into an [`Exception`] and hands it to the kernel binary,
//! which calls [`handle`]. Breakpoints are counted and resumed; anything else is fatal for now
//! (no page faults to fix up, no user processes to signal yet): the kernel reports it with
//! [`report`] and stops with the self-test fail marker.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::{Exception, ExceptionKind};

/// What to do after an exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Return to the interrupted code.
    Resume,
    /// The kernel cannot continue.
    Fatal,
}

/// Breakpoints taken since boot.
static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);

/// Decides what to do with `exception`.
pub fn handle(exception: &Exception) -> Outcome {
    match exception.kind {
        ExceptionKind::Breakpoint => {
            BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
            Outcome::Resume
        }
        _ => Outcome::Fatal,
    }
}

/// How many breakpoints [`handle`] has resumed from since boot.
pub fn breakpoints_taken() -> u64 {
    BREAKPOINTS.load(Ordering::Relaxed)
}

/// Writes one line describing a fatal `exception`, starting with `kernel exception: `.
pub fn report<W: Write>(console: &mut W, exception: &Exception) -> fmt::Result {
    write!(
        console,
        "kernel exception: {} (code {:#x}) at pc {:#x}",
        exception.name, exception.code, exception.pc
    )?;
    if let Some(address) = exception.address {
        write!(console, ", address {address:#x}")?;
    }
    writeln!(console)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exception(kind: ExceptionKind, address: Option<u64>) -> Exception {
        Exception {
            kind,
            name: "data abort",
            code: 0x9600_0045,
            pc: 0x4008_1234,
            address,
        }
    }

    #[test]
    fn breakpoints_resume_and_everything_else_is_fatal() {
        assert_eq!(
            handle(&exception(ExceptionKind::Breakpoint, None)),
            Outcome::Resume
        );
        for kind in [
            ExceptionKind::IllegalInstruction,
            ExceptionKind::InstructionFault,
            ExceptionKind::DataFault,
            ExceptionKind::Alignment,
            ExceptionKind::Interrupt,
            ExceptionKind::Other,
        ] {
            assert_eq!(handle(&exception(kind, None)), Outcome::Fatal, "{kind:?}");
        }
    }

    #[test]
    fn report_names_the_exception_and_where_it_happened() {
        let mut out = String::new();
        report(
            &mut out,
            &exception(ExceptionKind::DataFault, Some(0xdead_0000)),
        )
        .unwrap();
        assert_eq!(
            out,
            "kernel exception: data abort (code 0x96000045) at pc 0x40081234, address 0xdead0000\n"
        );
        out.clear();
        report(&mut out, &exception(ExceptionKind::DataFault, None)).unwrap();
        assert_eq!(
            out,
            "kernel exception: data abort (code 0x96000045) at pc 0x40081234\n"
        );
    }
}
