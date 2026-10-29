//! A minimal `/proc` (D125): `cmdline` (D137), `meminfo`, `uptime`, `ps`, `net/unix` (D138) and
//! `driver/rtc` (D150), read-only regular files made when they are opened, mounted on `/proc` at
//! boot when the root file system has that directory.
//!
//! - `cmdline`: the kernel command line the host started the machine with (QEMU's `-append`), and
//!   a newline, as Linux writes it; the built-in MCP server reads its grants there (D137).
//! - `meminfo`: `MemTotal`, `MemFree` and `MemAvailable` in kB, as Linux writes them: the
//!   frames the allocator had once the kernel's page tables were built, and the frames it has
//!   now (every free frame is available: there is no page cache to reclaim).
//! - `uptime`: the seconds since boot and the seconds CPUs spent idle, with two decimals, as
//!   Linux writes them; idle time is not counted yet, so it is `0.00`.
//! - `ps`: EggOS's own, one line per process in the process table, by id: its id, its
//!   parent's, its process group, its session, its state (`R` running, `Z` ended and not yet
//!   waited for) and its name, separated by single spaces. `ps` reads it.
//! - `net/unix`: the bound `AF_UNIX` sockets, in Linux's format (D138,
//!   [`super::unix_socket::proc_net_unix`]).
//! - `driver/rtc`: the real-time clock's date and time, read from the clock when the file is
//!   opened (at most once a second, [`crate::wall_clock::read_clock`]), in Linux's layout
//!   (`rtc_time`, `rtc_date`, `24hr`), only on a machine with one; `hwclock` reads it. Opening
//!   it fails with `EIO` when the clock cannot be read.

use super::process_table::{NAME_LENGTH, TABLE};
use crate::memory;
use crate::vfs::{Attributes, DirectoryEntry, FileSystem, Name, NodeKind, StorageError};
use alloc::vec::Vec;
use core::fmt::Write;
use core::marker::PhantomData;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

const ROOT: u64 = 1;

const FILES: [(&[u8], u64); 6] = [
    (b"cmdline", 5),
    (b"driver", 8),
    (b"meminfo", 2),
    (b"net", 6),
    (b"ps", 3),
    (b"uptime", 4),
];

const NET: u64 = 6;

const NET_FILES: [(&[u8], u64); 1] = [(b"unix", 7)];

const NET_UNIX: u64 = 7;

const DRIVER: u64 = 8;

const DRIVER_FILES: [(&[u8], u64); 1] = [(b"rtc", 9)];

const DRIVER_RTC: u64 = 9;

fn is_directory(node: u64) -> bool {
    matches!(node, ROOT | NET | DRIVER)
}

fn listing(directory: u64) -> &'static [(&'static [u8], u64)] {
    match directory {
        ROOT => &FILES,
        NET => &NET_FILES,
        DRIVER if crate::wall_clock::has_clock() => &DRIVER_FILES,
        _ => &[],
    }
}

const MEMINFO: u64 = 2;

const PS: u64 = 3;

const UPTIME: u64 = 4;

const CMDLINE: u64 = 5;

const DEVICE: (u32, u32) = (0, 4);

/// The `/proc` file system on platform `P`.
pub struct Proc<P> {
    platform: PhantomData<fn() -> P>,
}

impl<P> core::fmt::Debug for Proc<P> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("Proc")
    }
}

impl<P: Platform> Proc<P> {
    /// The file system; it holds nothing, so it costs nothing until a file is opened.
    pub const fn new() -> Self {
        Self {
            platform: PhantomData,
        }
    }
}

impl<P: Platform> Default for Proc<P> {
    fn default() -> Self {
        Self::new()
    }
}

struct Text(Vec<u8>);

impl Write for Text {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.0
            .try_reserve(text.len())
            .map_err(|_| core::fmt::Error)?;
        self.0.extend_from_slice(text.as_bytes());
        Ok(())
    }
}

/// `/proc/meminfo`'s text for `total` and `free` frames.
pub fn meminfo(total: u64, free: u64) -> Option<Vec<u8>> {
    let kilobytes = |frames: u64| frames * (FRAME_SIZE / 1024);
    let mut text = Text(Vec::new());
    for (name, frames) in [
        ("MemTotal", total),
        ("MemFree", free),
        ("MemAvailable", free),
    ] {
        writeln!(
            text,
            "{:<16}{:>8} kB",
            alloc::format!("{name}:"),
            kilobytes(frames)
        )
        .ok()?;
    }
    Some(text.0)
}

/// `/proc/uptime`'s text after `ticks` of a clock of `frequency` ticks per second.
pub fn uptime(ticks: u64, frequency: u64) -> Option<Vec<u8>> {
    let hundredths = u128::from(ticks) * 100 / u128::from(frequency.max(1));
    let mut text = Text(Vec::new());
    writeln!(text, "{}.{:02} 0.00", hundredths / 100, hundredths % 100).ok()?;
    Some(text.0)
}

/// `/proc/cmdline`'s text for the command line `line`: the line and a newline, as Linux writes it.
pub fn cmdline_text(line: &[u8]) -> Option<Vec<u8>> {
    let mut text = Vec::new();
    text.try_reserve(line.len() + 1).ok()?;
    text.extend_from_slice(line);
    text.push(b'\n');
    Some(text)
}

/// `/proc/driver/rtc`'s text for a real-time clock reading `seconds` since the epoch (D150).
pub fn driver_rtc(seconds: u64) -> Option<Vec<u8>> {
    let mut text = Text(Vec::new());
    eggos_calendar::format::proc_driver_rtc(&mut text, seconds as i64).ok()?;
    Some(text.0)
}

fn cmdline<P: Platform>() -> Option<Vec<u8>> {
    let mut text = None;
    P::command_line(&mut |line| text = cmdline_text(line));
    text.or_else(|| cmdline_text(b""))
}

fn ps() -> Option<Vec<u8>> {
    let mut text = Text(Vec::new());
    let table = TABLE.lock();
    text.0.try_reserve(table.list().count() * 48).ok()?;
    for process in table.list() {
        let length = process
            .name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(NAME_LENGTH);
        let state = if process.ended { 'Z' } else { 'R' };
        write!(
            text,
            "{} {} {} {} {state} ",
            process.id, process.parent, process.group, process.session
        )
        .ok()?;
        text.0.extend_from_slice(&process.name[..length]);
        text.0.push(b'\n');
    }
    Some(text.0)
}

impl<P: Platform> FileSystem for Proc<P> {
    fn root(&self) -> u64 {
        ROOT
    }

    fn device(&self) -> (u32, u32) {
        DEVICE
    }

    fn kind(&self, node: u64) -> NodeKind {
        match is_directory(node) {
            true => NodeKind::Directory,
            false => NodeKind::Regular,
        }
    }

    fn attributes(&self, node: u64) -> Attributes {
        let directory = is_directory(node);
        Attributes {
            kind: self.kind(node),
            permissions: if directory { 0o555 } else { 0o444 },
            file_id: node,
            links: if directory { 2 } else { 1 },
            owner: 0,
            group: 0,
            size: 0,
            modified: 0,
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        listing(directory)
            .iter()
            .find(|(file, _)| *file == name)
            .map(|&(_, node)| node)
    }

    fn parent(&self, node: u64) -> u64 {
        if NET_FILES.iter().any(|&(_, file)| file == node) {
            return NET;
        }
        match DRIVER_FILES.iter().any(|&(_, file)| file == node) {
            true => DRIVER,
            false => ROOT,
        }
    }

    fn name(&self, node: u64) -> Name {
        Name::new(
            FILES
                .iter()
                .chain(NET_FILES.iter())
                .chain(DRIVER_FILES.iter())
                .find(|&&(_, file)| file == node)
                .map_or(&[][..], |(name, _)| name),
        )
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn generate(&self, node: u64) -> Result<Option<Vec<u8>>, StorageError> {
        let generated = match node {
            MEMINFO => {
                let free = memory::FRAMES
                    .lock()
                    .as_ref()
                    .map_or(0, |frames| frames.free_frames());
                meminfo(memory::total_frames(), free)
            }
            UPTIME => uptime(P::clock_ticks(), P::clock_frequency()),
            PS => ps(),
            CMDLINE => cmdline::<P>(),
            NET_UNIX => super::unix_socket::proc_net_unix(),
            DRIVER_RTC => match crate::wall_clock::read_clock::<P>() {
                Some((_, Ok(nanoseconds))) => driver_rtc(nanoseconds / 1_000_000_000),
                _ => return Err(StorageError),
            },
            _ => None,
        };
        Ok(generated)
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let &(name, node) = listing(directory).get(usize::try_from(position).ok()?)?;
        Some(DirectoryEntry {
            name: Name::new(name),
            node,
            kind: self.kind(node),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wall_clock::host::with_clock;
    use alloc::boxed::Box;
    use eggos_drivers::rtc::{RealTimeClock, RtcError};

    #[test]
    fn meminfo_is_in_kilobytes_aligned_as_linux_s() {
        let text = meminfo(32768, 1000).unwrap();
        assert_eq!(
            core::str::from_utf8(&text).unwrap(),
            "MemTotal:         131072 kB\nMemFree:            4000 kB\nMemAvailable:       4000 kB\n"
        );
    }

    #[test]
    fn cmdline_is_the_line_and_a_newline() {
        assert_eq!(
            cmdline_text(b"eggos.mcp_allow=write").unwrap(),
            b"eggos.mcp_allow=write\n"
        );
        assert_eq!(cmdline_text(b"").unwrap(), b"\n");
        let proc = Proc::<crate::tests::TestPlatform>::new();
        assert_eq!(proc.generate(CMDLINE), Ok(Some(b"\n".to_vec())));
    }

    #[test]
    fn uptime_has_two_decimals() {
        assert_eq!(uptime(12_345, 1000).unwrap(), b"12.34 0.00\n");
        assert_eq!(uptime(5, 100).unwrap(), b"0.05 0.00\n");
        assert_eq!(uptime(0, 0).unwrap(), b"0.00 0.00\n");
    }

    #[test]
    fn driver_rtc_is_linux_s_layout() {
        assert_eq!(
            driver_rtc(1_793_282_709).unwrap(),
            b"rtc_time\t: 14:05:09\nrtc_date\t: 2026-10-29\n24hr\t\t: yes\n"
        );
    }

    #[derive(Debug)]
    struct AnsweringClock(Result<u64, RtcError>);

    impl RealTimeClock for AnsweringClock {
        fn name(&self) -> &'static str {
            "rtc-test"
        }

        fn read(&self) -> Result<u64, RtcError> {
            self.0
        }

        fn write(&self, _nanoseconds: u64) -> Result<(), RtcError> {
            Ok(())
        }
    }

    #[test]
    fn driver_rtc_reads_the_clock_and_an_unreadable_one_is_an_error_not_an_empty_file() {
        let _state = crate::thread::host::frames_for_test();
        let proc = Proc::<crate::tests::TestPlatform>::new();
        let readable = AnsweringClock(Ok(1_793_282_709 * 1_000_000_000));
        let generated = with_clock(Box::new(readable), || proc.generate(DRIVER_RTC));
        assert_eq!(generated, Ok(driver_rtc(1_793_282_709)));
        for error in [RtcError::Busy, RtcError::Invalid] {
            let generated = with_clock(Box::new(AnsweringClock(Err(error))), || {
                proc.generate(DRIVER_RTC)
            });
            assert_eq!(generated, Err(StorageError), "{error:?}");
        }
    }

    #[test]
    fn the_root_lists_and_finds_its_files_and_net() {
        let proc = Proc::<crate::tests::TestPlatform>::new();
        let names: Vec<Vec<u8>> = (0..)
            .map_while(|position| proc.entry(ROOT, position))
            .map(|entry| entry.name.as_bytes().to_vec())
            .collect();
        assert_eq!(
            names,
            [
                &b"cmdline"[..],
                b"driver",
                b"meminfo",
                b"net",
                b"ps",
                b"uptime"
            ]
        );
        assert_eq!(proc.kind(NET), NodeKind::Directory);
        assert_eq!(proc.entry(ROOT, 3).unwrap().kind, NodeKind::Directory);
        assert_eq!(proc.kind(DRIVER), NodeKind::Directory);
        assert_eq!(proc.parent(DRIVER_RTC), DRIVER);
        assert_eq!(proc.parent(DRIVER), ROOT);
        assert_eq!(proc.name(DRIVER_RTC).as_bytes(), b"rtc");
        assert_eq!(proc.lookup(NET, b"unix"), Some(NET_UNIX));
        assert_eq!(proc.lookup(ROOT, b"unix"), None);
        assert_eq!(proc.parent(NET_UNIX), NET);
        assert_eq!(proc.parent(NET), ROOT);
        assert_eq!(proc.name(NET_UNIX).as_bytes(), b"unix");
        assert_eq!(proc.entry(NET, 0).unwrap().node, NET_UNIX);
        assert!(matches!(proc.generate(NET_UNIX), Ok(Some(_))));
        assert_eq!(proc.lookup(ROOT, b"cmdline"), Some(CMDLINE));
        assert_eq!(proc.lookup(ROOT, b"ps"), Some(PS));
        assert_eq!(proc.lookup(ROOT, b"self"), None);
        assert_eq!(proc.name(UPTIME).as_bytes(), b"uptime");
        assert_eq!(proc.attributes(MEMINFO).permissions, 0o444);
        assert_eq!(proc.kind(ROOT), NodeKind::Directory);
    }
}
