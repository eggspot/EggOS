//! How the release kernel boots into user space (D106): it starts every CPU, turns preemption
//! on, and runs `/init` from the root file system as process 1, the process orphans go to.
//! Init ending is fatal, as on Linux: the kernel says how it ended and stops the machine.

use crate::linux::{self, exec, process_table};
use crate::memory::Memory;
use crate::process::{Process, ProcessExit};
use crate::{apps, cpu, input, terminal, thread, vfs};
use alloc::sync::Arc;
use core::fmt::Write;
use eggos_hal::Platform;

/// The program the kernel starts as process 1: Linux's default for an initramfs.
pub const PATH: &[u8] = b"/init";

const ENVIRONMENT: [&[u8]; 2] = [b"HOME=/", b"TERM=linux"];

const CPU_START_TIMEOUT_MS: u64 = 1000;

/// The start of the line the kernel writes once the graphical terminal shows the console, before
/// its window's grid's columns and rows (`80x24`): from the next line on, the serial console and the
/// terminal receive the same bytes. Must match the QEMU runner's.
pub const TERMINAL_LINE_PREFIX: &str = "boot: terminal ";

/// Why init could not run, or how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It ran and ended this way.
    Ended(ProcessExit),
    /// It could not be started, for this reason.
    NotStarted(&'static str),
}

impl Outcome {
    /// Linux's wait status for how init ended (`exitcode` in its panic message): the exit
    /// status in bits 8 to 15, or the signal that ended it.
    pub fn wait_status(self) -> Option<u32> {
        match self {
            Outcome::Ended(ProcessExit::Exited(status)) => Some((status & 0xff) << 8),
            Outcome::Ended(ProcessExit::Killed { signal }) => Some(signal & 0x7f),
            Outcome::NotStarted(_) => None,
        }
    }
}

/// Starts every CPU and runs [`PATH`] as process 1 until it ends, then reports that on
/// `console`, parks every other CPU and returns how it went.
pub fn run<P: Platform, W: Write>(
    console: &mut W,
    memory: &mut Memory<P>,
) -> Result<Outcome, core::fmt::Error> {
    let online = start_cpus::<P>(memory);
    let plural = if online == 1 { "" } else { "s" };
    writeln!(console, "cpu: {online} CPU{plural} online")?;
    thread::enable_preemption::<P>();
    if let Err(error) = input::start_polling::<P>() {
        writeln!(console, "input: not polled: {error:?}")?;
    }
    terminal::load_saved_profile();
    if let Some(area) = terminal::start::<P>(terminal::Session::Interactive) {
        writeln!(
            console,
            "{TERMINAL_LINE_PREFIX}{}x{}",
            area.columns, area.rows
        )?;
        if let Err(error) = terminal::start_drawing::<P>() {
            terminal::stop();
            writeln!(console, "terminal: not drawn: {error:?}")?;
        }
    }
    let outcome = start_and_wait::<P>(memory);
    match outcome.wait_status() {
        Some(status) => writeln!(
            console,
            "Kernel panic - not syncing: Attempted to kill init! exitcode={status:#010x}"
        )?,
        None => writeln!(console, "Kernel panic - not syncing: No working init found")?,
    }
    if let Outcome::NotStarted(reason) = outcome {
        writeln!(console, "init: {reason}")?;
    }
    cpu::stop_others::<P>();
    Ok(outcome)
}

fn start_cpus<P: Platform>(memory: &Memory<P>) -> usize {
    // SAFETY: these are the kernel's page tables, active since `paging::enable`, whose frames
    // are never freed; nothing changes them from here on.
    let Ok(started) = (unsafe { cpu::start_secondaries::<P>(memory.page_tables.roots()) }) else {
        return cpu::CPUS.online().max(1);
    };
    let expected = started.started + 1;
    let deadline = P::clock_ticks() + CPU_START_TIMEOUT_MS * (P::clock_frequency() / 1000);
    while cpu::CPUS.online() < expected && P::clock_ticks() < deadline {
        core::hint::spin_loop();
    }
    cpu::CPUS.online()
}

fn start_and_wait<P: Platform>(memory: &Memory<P>) -> Outcome {
    let root = vfs::root();
    let Ok(node) = vfs::resolve(root, root, PATH) else {
        return Outcome::NotStarted("/init is not in the root file system");
    };
    let attributes = node.attributes();
    if attributes.kind != vfs::NodeKind::Regular {
        return Outcome::NotStarted("/init is not a regular file");
    }
    let image = node.read(0, attributes.size as usize);
    // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
    let Ok(mut process) = (unsafe { Process::<P>::new(&memory.page_tables) }) else {
        return Outcome::NotStarted("no memory for process 1");
    };
    let Ok(context) = exec::load(&mut process, image, PATH, &[PATH], &ENVIRONMENT) else {
        return Outcome::NotStarted("/init is not a program this machine runs");
    };
    let process = Arc::new(process);
    let beside = Arc::clone(&process);
    apps::set_starter(Arc::new(move |path: &[u8]| apps::start::<P>(&beside, path)));
    {
        let mut table = process_table::TABLE.lock();
        table.set_init(process.id().get());
        let _ = table.add_if_missing(process.id().get());
        table.set_name(process.id().get(), process_table::name_of(b"init"));
    }
    let Ok(thread) = linux::spawn::<P>(Arc::clone(&process), context) else {
        return Outcome::NotStarted("could not start process 1's thread");
    };
    thread.join::<P>();
    match process.exit_status() {
        Some(exit) => Outcome::Ended(exit),
        None => Outcome::NotStarted("process 1's thread ended without ending it"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_status_holds_the_exit_status_or_the_signal() {
        assert_eq!(
            Outcome::Ended(ProcessExit::Exited(1)).wait_status(),
            Some(0x100)
        );
        assert_eq!(
            Outcome::Ended(ProcessExit::Exited(0x1ff)).wait_status(),
            Some(0xff00)
        );
        assert_eq!(
            Outcome::Ended(ProcessExit::Killed { signal: 9 }).wait_status(),
            Some(9)
        );
        assert_eq!(Outcome::NotStarted("x").wait_status(), None);
    }
}
