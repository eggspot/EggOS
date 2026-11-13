//! Starting the system file picker, `/bin/pick` (D392, docs/ux/file-picker-v0.3.md section
//! 14), for the layer's file dialogs: its command line built in a fixed arena, the program
//! started with its standard output on a pipe, the calling thread's messages run while it is
//! open (the owner is disabled meanwhile, as a modal dialog does), and its output and exit
//! status read when it ends.

use super::gui::{self, Next};
use super::user32::{dispatch, translate};
use crate::gui::queue::Filter;
use crate::linux;
use core::ffi::CStr;

const ARENA_BYTES: usize = 8192;
const MOST_ARGUMENTS: usize = 64;
const O_CLOEXEC: usize = 0o2_000_000;
const POLL_IN: u16 = 0x001;
const POLL_ERROR: u16 = 0x008;
const POLL_HANG_UP: u16 = 0x010;
const IDLE_NANOSECONDS: u64 = 20_000_000;
const PICK: &CStr = c"/bin/pick";

/// `pick`'s command line, each argument NUL-terminated in the arena.
pub(super) struct Arguments {
    arena: [u8; ARENA_BYTES],
    used: usize,
    starts: [usize; MOST_ARGUMENTS],
    count: usize,
}

impl Arguments {
    /// The command line `pick <mode>`.
    pub(super) fn new(mode: &[u8]) -> Self {
        let mut arguments = Self {
            arena: [0; ARENA_BYTES],
            used: 0,
            starts: [0; MOST_ARGUMENTS],
            count: 0,
        };
        arguments.push(b"pick");
        arguments.push(mode);
        arguments
    }

    /// Adds `argument`; false when it does not fit or holds a NUL.
    pub(super) fn push(&mut self, argument: &[u8]) -> bool {
        let end = self.used + argument.len() + 1;
        if self.count + 1 >= MOST_ARGUMENTS || end > ARENA_BYTES || argument.contains(&0) {
            return false;
        }
        self.arena[self.used..end - 1].copy_from_slice(argument);
        self.arena[end - 1] = 0;
        self.starts[self.count] = self.used;
        self.count += 1;
        self.used = end;
        true
    }

    /// Adds an option and its value, both or neither.
    pub(super) fn option(&mut self, name: &[u8], value: &[u8]) -> bool {
        let (used, count) = (self.used, self.count);
        if self.push(name) && self.push(value) {
            return true;
        }
        self.used = used;
        self.count = count;
        false
    }

    fn pointers(&self) -> [*const u8; MOST_ARGUMENTS] {
        let mut pointers = [core::ptr::null(); MOST_ARGUMENTS];
        for (pointer, &start) in pointers.iter_mut().zip(&self.starts[..self.count]) {
            *pointer = self.arena[start..].as_ptr();
        }
        pointers
    }
}

/// How `pick` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Picked {
    /// It printed this many bytes of chosen paths into the output.
    Chosen(usize),
    Cancelled,
}

fn pump_messages() -> bool {
    match gui::wait_for(Filter::default(), true, false, &|_| false) {
        Next::Message(next) => {
            translate(&next);
            dispatch(&next);
            true
        }
        _ => false,
    }
}

/// Runs `pick` with `arguments`, its output into `out`, the thread's messages served while it
/// runs; its error when it could not start or failed (exit 2 or 3).
pub(super) fn run(arguments: &Arguments, out: &mut [u8]) -> Result<Picked, u32> {
    let (read_end, write_end) = linux::pipe_with(O_CLOEXEC).map_err(super::error_for)?;
    let pointers = arguments.pointers();
    let started =
        super::processes::spawn_with_output(PICK, &pointers[..arguments.count + 1], write_end);
    let _ = linux::close(write_end);
    let id = match started {
        Ok(id) => id,
        Err(code) => {
            let _ = linux::close(read_end);
            return Err(code);
        }
    };
    let mut filled = 0;
    loop {
        let events = linux::events_now(read_end).unwrap_or(POLL_HANG_UP);
        if events & (POLL_IN | POLL_ERROR | POLL_HANG_UP) != 0 {
            let room = out.len().saturating_sub(filled);
            let mut discard = [0u8; 256];
            let target = match room {
                0 => &mut discard[..],
                _ => &mut out[filled..],
            };
            match linux::read(read_end, target) {
                Ok(0) | Err(_) => break,
                Ok(read) if room > 0 => filled += read,
                Ok(_) => {}
            }
            continue;
        }
        if !pump_messages() {
            linux::sleep_nanoseconds(IDLE_NANOSECONDS);
        }
    }
    let _ = linux::close(read_end);
    let status = loop {
        match linux::reap(id) {
            Ok(Some(status)) => break status,
            Ok(None) => linux::sleep_nanoseconds(IDLE_NANOSECONDS),
            Err(_) => break 1 << 8,
        }
    };
    match (status >> 8) & 0xff {
        0 => Ok(Picked::Chosen(filled)),
        1 => Ok(Picked::Cancelled),
        _ => Err(super::error::INVALID_PARAMETER),
    }
}
