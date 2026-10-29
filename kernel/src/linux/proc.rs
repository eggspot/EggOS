//! `/proc` (D125, D168): `cmdline` (D137), `meminfo`, `uptime`, `ps`, `net/unix` (D138),
//! `net/interfaces`, `net/route` and `net/arp` (D157), `driver/rtc` (D150), `version` and
//! `sys/kernel/{hostname,osrelease,ostype}`, and a directory per process with `self` naming the
//! caller's (D168): read-only regular files made when they are opened, and links, mounted on
//! `/proc` at boot when the root file system has that directory.
//!
//! - `cmdline`: the kernel command line the host started the machine with (QEMU's `-append`), and
//!   a newline, as Linux writes it; the built-in MCP server reads its grants there (D137).
//! - `meminfo`: `MemTotal`, `MemFree` and `MemAvailable` in kB, as Linux writes them, and
//!   EggOS's `KernelReserve`: the frames the allocator had once the kernel's page tables were
//!   built, the frames it has now, those programs can still take (every free frame but the
//!   reserve kept for the kernel, D155: there is no page cache to reclaim) and the reserve.
//! - `uptime`: the seconds since boot and the seconds CPUs spent idle, with two decimals, as
//!   Linux writes them; idle time is not counted yet, so it is `0.00`.
//! - `ps`: EggOS's own, one line per process in the process table, by id: its id, its
//!   parent's, its process group, its session, its state (`R` running, `Z` ended and not yet
//!   waited for) and its name, separated by single spaces. `ps` reads it.
//! - `net/unix`: the bound `AF_UNIX` sockets, in Linux's format (D138,
//!   [`super::unix_socket::proc_net_unix`]).
//! - `net/interfaces`: EggOS's own, one line per network interface (`lo`, and `eth0` with an
//!   adapter): its link, addresses, DHCP state, lease and counters (D157,
//!   [`eggos_net::status`]); `ip`, `net_status` and Settings read it. `net/route` and
//!   `net/arp`: the routes and the neighbours' hardware addresses, in Linux's layouts.
//! - `driver/rtc`: the real-time clock's date and time, read from the clock when the file is
//!   opened (at most once a second, [`crate::wall_clock::read_clock`]), in Linux's layout
//!   (`rtc_time`, `rtc_date`, `24hr`), only on a machine with one; `hwclock` reads it. Opening
//!   it fails with `EIO` when the clock cannot be read.
//! - `version`, `sys/kernel/osrelease`, `sys/kernel/ostype` and `sys/kernel/hostname`: what
//!   `uname` reports ([`super::identity`]), each a line as Linux writes it.
//! - `self`: a link to the directory of the process that reads it.
//! - `<pid>/`, one per process in the process table, listed after the files above: `cmdline`
//!   (its arguments, each ended by a NUL, read from its stack), `exe` (a link to its program's
//!   file), `fd/` (a link per open descriptor to what it names: a file's path, `/dev/console`,
//!   `pipe:[inode]`, `socket:[inode]`), `maps` (its memory as Linux lists it, with files' paths,
//!   `[heap]` and `[stack]`: glibc's `pthread_getattr_np` finds the main thread's stack there
//!   and sizes it by `RLIMIT_STACK`), `stat` (Linux's 52 fields, counters EggOS does not keep
//!   yet as 0) and `status` (`Name`, `State`, the ids, `FDSize`, `Groups`, `VmSize`, `Threads`).
//!   A process that ended and was not waited for yet keeps its directory, with an empty
//!   `cmdline` and `maps`, as Linux's zombies.

use super::descriptors::FileKind;
use super::identity;
use super::process_table::{Listed, NAME_LENGTH, Running, TABLE};
use crate::memory;
use crate::process::{Mapping, MappingLabel, Process};
use crate::regions::FilePages;
use crate::vfs::{
    Attributes, DirectoryEntry, FileSystem, KERNEL_CALLER, Name, NodeKind, PATH_MAX, StorageError,
};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Write;
use core::marker::PhantomData;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

const ROOT: u64 = 1;

const MEMINFO: u64 = 2;

const PS: u64 = 3;

const UPTIME: u64 = 4;

const CMDLINE: u64 = 5;

const NET: u64 = 6;

const NET_UNIX: u64 = 7;

const DRIVER: u64 = 8;

const DRIVER_RTC: u64 = 9;

const NET_ARP: u64 = 10;

const NET_INTERFACES: u64 = 11;

const NET_ROUTE: u64 = 12;

const SELF: u64 = 13;

const SYS: u64 = 14;

const SYS_KERNEL: u64 = 15;

const VERSION: u64 = 16;

const OSRELEASE: u64 = 17;

const OSTYPE: u64 = 18;

const HOSTNAME: u64 = 19;

const FILES: [(&[u8], u64); 9] = [
    (b"cmdline", CMDLINE),
    (b"driver", DRIVER),
    (b"meminfo", MEMINFO),
    (b"net", NET),
    (b"ps", PS),
    (b"self", SELF),
    (b"sys", SYS),
    (b"uptime", UPTIME),
    (b"version", VERSION),
];

const NET_FILES: [(&[u8], u64); 4] = [
    (b"unix", NET_UNIX),
    (b"arp", NET_ARP),
    (b"interfaces", NET_INTERFACES),
    (b"route", NET_ROUTE),
];

const DRIVER_FILES: [(&[u8], u64); 1] = [(b"rtc", DRIVER_RTC)];

const SYS_FILES: [(&[u8], u64); 1] = [(b"kernel", SYS_KERNEL)];

const SYS_KERNEL_FILES: [(&[u8], u64); 3] = [
    (b"hostname", HOSTNAME),
    (b"osrelease", OSRELEASE),
    (b"ostype", OSTYPE),
];

/// What a node of a process's directory is: the low byte of its number.
mod part {
    pub const DIRECTORY: u64 = 1;
    pub const CMDLINE: u64 = 2;
    pub const EXE: u64 = 3;
    pub const FD: u64 = 4;
    pub const MAPS: u64 = 5;
    pub const STAT: u64 = 6;
    pub const STATUS: u64 = 7;
    pub const DESCRIPTOR: u64 = 8;
}

const PROCESS_FILES: [(&[u8], u64); 6] = [
    (b"cmdline", part::CMDLINE),
    (b"exe", part::EXE),
    (b"fd", part::FD),
    (b"maps", part::MAPS),
    (b"stat", part::STAT),
    (b"status", part::STATUS),
];

/// The bit of a process's nodes' numbers: the process's id is above [`PID_SHIFT`], a
/// descriptor's number above [`FD_SHIFT`], the [`part`] in the low byte.
const PROCESS_NODE: u64 = 1 << 62;

const PID_SHIFT: u32 = 32;

/// The largest process id a node can hold.
const MAX_PID: u32 = (1 << 30) - 1;

const FD_SHIFT: u32 = 8;

/// The largest descriptor number a node can hold, above Linux's largest open-file limit.
const MAX_FD: u32 = (1 << 24) - 1;

/// A node of a process's directory, decoded from its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessNode {
    pid: u32,
    part: u64,
    fd: u32,
}

impl ProcessNode {
    fn of(node: u64) -> Option<Self> {
        (node & PROCESS_NODE != 0).then_some(Self {
            pid: (node >> PID_SHIFT) as u32 & MAX_PID,
            part: node & 0xff,
            fd: (node >> FD_SHIFT) as u32 & MAX_FD,
        })
    }

    fn part_of(pid: u32, part: u64) -> Self {
        Self { pid, part, fd: 0 }
    }

    fn number(self) -> u64 {
        PROCESS_NODE | u64::from(self.pid) << PID_SHIFT | u64::from(self.fd) << FD_SHIFT | self.part
    }
}

fn is_directory(node: u64) -> bool {
    match ProcessNode::of(node) {
        Some(node) => matches!(node.part, part::DIRECTORY | part::FD),
        None => matches!(node, ROOT | NET | DRIVER | SYS | SYS_KERNEL),
    }
}

fn is_link(node: u64) -> bool {
    match ProcessNode::of(node) {
        Some(node) => matches!(node.part, part::EXE | part::DESCRIPTOR),
        None => node == SELF,
    }
}

fn listing(directory: u64) -> &'static [(&'static [u8], u64)] {
    match directory {
        ROOT => &FILES,
        NET => &NET_FILES,
        DRIVER if crate::wall_clock::has_clock() => &DRIVER_FILES,
        SYS => &SYS_FILES,
        SYS_KERNEL => &SYS_KERNEL_FILES,
        _ => &[],
    }
}

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

impl Text {
    fn bytes(&mut self, bytes: &[u8]) -> Option<()> {
        self.0.try_reserve(bytes.len()).ok()?;
        self.0.extend_from_slice(bytes);
        Some(())
    }
}

/// `value` in decimal, written into `buffer`.
fn decimal(value: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut start = buffer.len();
    let mut rest = value;
    loop {
        start -= 1;
        buffer[start] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    &buffer[start..]
}

/// The number `name` writes in decimal, as Linux reads a process's or descriptor's name: digits
/// only, no leading zero, no larger than `largest`.
fn parse_decimal(name: &[u8], largest: u32) -> Option<u32> {
    if name.is_empty() || name.len() > 10 || (name.len() > 1 && name[0] == b'0') {
        return None;
    }
    let mut value = 0u64;
    for &byte in name {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u64::from(byte - b'0');
    }
    u32::try_from(value).ok().filter(|&value| value <= largest)
}

/// `/proc/meminfo`'s text for `total` and `free` frames, `reserve` of them kept for the kernel
/// (D155): what programs can still take is `MemAvailable`.
pub fn meminfo(total: u64, free: u64, reserve: u64) -> Option<Vec<u8>> {
    let kilobytes = |frames: u64| frames * (FRAME_SIZE / 1024);
    let mut text = Text(Vec::new());
    for (name, frames) in [
        ("MemTotal", total),
        ("MemFree", free),
        ("MemAvailable", free.saturating_sub(reserve)),
        ("KernelReserve", reserve),
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

/// `/proc/version`'s text: Linux's layout, with what `uname` reports.
pub fn version_text() -> Option<Vec<u8>> {
    let mut text = Text(Vec::new());
    writeln!(
        text,
        "Linux version {} (eggos@eggos) (rustc) {}",
        identity::utsname::RELEASE,
        identity::utsname::VERSION
    )
    .ok()?;
    Some(text.0)
}

/// `bytes` and a newline, as `/proc/sys` writes a value.
fn line(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut text = Text(Vec::new());
    text.bytes(bytes)?;
    text.bytes(b"\n")?;
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
        let state = if process.ended { 'Z' } else { 'R' };
        write!(
            text,
            "{} {} {} {} {state} ",
            process.id, process.parent, process.group, process.session
        )
        .ok()?;
        text.0.extend_from_slice(name_of(&process));
        text.0.push(b'\n');
    }
    Some(text.0)
}

fn name_of(process: &Listed) -> &[u8] {
    let length = process
        .name
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(NAME_LENGTH);
    &process.name[..length]
}

/// What the process table says of a process, and its program while it runs.
struct Described<P: Platform> {
    listed: Listed,
    running: Option<Running>,
    process: Option<Arc<Process<P>>>,
}

fn described<P: Platform>(pid: u32) -> Option<Described<P>> {
    let (listed, running) = TABLE.lock().describe(pid)?;
    let process = running
        .as_ref()
        .and_then(|running| running.process.upgrade())
        .and_then(|process| process.downcast::<Process<P>>().ok());
    Some(Described {
        listed,
        running,
        process,
    })
}

fn running_process<P: Platform>(pid: u32) -> Option<Arc<Process<P>>> {
    described::<P>(pid)?.process
}

/// The absolute path of `file`, if it has one.
fn path_of(file: &dyn FilePages) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(PATH_MAX).ok()?;
    buffer.resize(PATH_MAX, 0);
    let start = file.path_into(&mut buffer)?;
    buffer.drain(..start);
    Some(buffer)
}

/// What `/proc/<pid>/fd/<fd>` names for an open file description of `kind`.
fn descriptor_target(kind: &FileKind) -> Option<Vec<u8>> {
    let mut text = Text(Vec::new());
    match kind {
        FileKind::Regular(node) | FileKind::Directory(node) | FileKind::Path(node) => {
            return path_of(node);
        }
        FileKind::Snapshot(snapshot) => return path_of(&snapshot.node),
        FileKind::MemoryDevice(_, node) => return path_of(node),
        FileKind::Console => text.bytes(b"/dev/console")?,
        FileKind::Window(_) => text.bytes(b"/dev/window")?,
        FileKind::AgentChannel(_) => text.bytes(b"/dev/agent")?,
        FileKind::Pipe(end) => write!(text, "pipe:[{}]", end.inode()).ok()?,
        FileKind::Socket(socket) => write!(text, "socket:[{}]", socket.inode()).ok()?,
        FileKind::InetSocket(socket) => write!(text, "socket:[{}]", socket.inode()).ok()?,
        FileKind::MemoryFile(_) => text.bytes(b"/memfd: (deleted)")?,
        FileKind::Event(event) => text.bytes(event.link_name())?,
    }
    Some(text.0)
}

/// `/proc/<pid>/cmdline`: the argument strings on its stack, each with its NUL.
fn process_cmdline<P: Platform>(pid: u32) -> Option<Vec<u8>> {
    let process = running_process::<P>(pid)?;
    let (start, end) = process.image_layout().arguments;
    let length = usize::try_from(end.checked_sub(start)?).ok()?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).ok()?;
    bytes.resize(length, 0);
    process.read_user_memory(start, &mut bytes).ok()?;
    Some(bytes)
}

/// Where the name of a `/proc/<pid>/maps` line starts: Linux pads the line to this column.
const MAPS_NAME_COLUMN: usize = 73;

/// One line of `/proc/<pid>/maps` for `mapping`, as Linux writes it: the range, the access
/// and `s`hared or `p`rivate, the file's offset, device and inode (zeros without one), then,
/// from [`MAPS_NAME_COLUMN`], the file's path or `[heap]` or `[stack]`.
fn write_mapping(text: &mut Text, mapping: &Mapping) -> Option<()> {
    let line_start = text.0.len();
    let (read, write, execute) = match mapping.attributes {
        Some(attributes) => (
            'r',
            if attributes.writable { 'w' } else { '-' },
            if attributes.executable { 'x' } else { '-' },
        ),
        None => ('-', '-', '-'),
    };
    let sharing = if mapping.shared { 's' } else { 'p' };
    let (offset, ((major, minor), inode)) = match &mapping.file {
        Some((file, offset)) => (*offset, file.identity()),
        None => (0, ((0, 0), 0)),
    };
    write!(
        text,
        "{:08x}-{:08x} {read}{write}{execute}{sharing} {offset:08x} {major:02x}:{minor:02x} {inode} ",
        mapping.start, mapping.end
    )
    .ok()?;
    let name = match (&mapping.file, mapping.label) {
        (Some((file, _)), _) => path_of(&**file),
        (None, MappingLabel::Heap) => Some(b"[heap]".to_vec()),
        (None, MappingLabel::Stack) => Some(b"[stack]".to_vec()),
        (None, MappingLabel::Plain) => None,
    };
    if let Some(name) = name {
        let written = text.0.len() - line_start;
        let padding = (MAPS_NAME_COLUMN - 1).saturating_sub(written) + 1;
        text.0.try_reserve(padding + name.len()).ok()?;
        text.0.resize(text.0.len() + padding, b' ');
        text.0.extend_from_slice(&name);
    }
    text.bytes(b"\n")
}

/// `/proc/<pid>/maps`: see [`write_mapping`].
fn process_maps<P: Platform>(pid: u32) -> Option<Vec<u8>> {
    let process = running_process::<P>(pid)?;
    let mappings = process.mappings()?;
    drop(process);
    let mut text = Text(Vec::new());
    text.0.try_reserve(mappings.len() * 96).ok()?;
    for mapping in &mappings {
        write_mapping(&mut text, mapping)?;
    }
    Some(text.0)
}

/// The bytes of user space `mappings` hold.
fn mapped_bytes(mappings: &[Mapping]) -> u64 {
    mappings
        .iter()
        .map(|mapping| mapping.end - mapping.start)
        .sum()
}

/// The console's device number as `stat` encodes a terminal's (`tty_nr`): major 5, minor 1.
const CONSOLE_TTY_NUMBER: u32 = 5 << 8 | 1;

/// `/proc/<pid>/stat`: Linux's 52 fields on one line, the ones EggOS does not count yet (page
/// faults, CPU times, the start time, the resident pages, the code's bounds, signals) as 0.
fn process_stat<P: Platform>(pid: u32) -> Option<Vec<u8>> {
    let (listed, terminal) = {
        let table = TABLE.lock();
        let (listed, _) = table.describe(pid)?;
        (listed, table.controlling_terminal(pid).ok())
    };
    let process = running_process::<P>(pid);
    let state = if listed.ended { 'Z' } else { 'R' };
    let (tty, foreground) = match terminal {
        Some((_, foreground)) => (CONSOLE_TTY_NUMBER, i64::from(foreground)),
        None => (0, -1),
    };
    let (threads, size, rss_limit, layout, heap_start) = match &process {
        Some(process) => (
            process.threads(),
            mapped_bytes(&process.mappings()?),
            process
                .credentials_and_limits()
                .lock()
                .limits
                .get(super::limits::resource::RSS)
                .map_or(u64::MAX, |limit| limit.soft),
            process.image_layout().clone(),
            process.program_break_range().map_or(0, |(start, _)| start),
        ),
        None => (0, 0, 0, Default::default(), 0),
    };
    let mut text = Text(Vec::new());
    write!(text, "{pid} (").ok()?;
    text.bytes(name_of(&listed))?;
    write!(
        text,
        ") {state} {} {} {} {tty} {foreground} 0 0 0 0 0 0 0 0 0 20 0 {threads} 0 0 {size} 0 \
         {rss_limit} 0 0 {} 0 0 0 0 0 0 0 0 0 {} 0 0 0 0 0 0 0 0 {heap_start} {} {} {} {} 0",
        listed.parent,
        listed.group,
        listed.session,
        layout.stack_pointer,
        listed.exit_signal,
        layout.arguments.0,
        layout.arguments.1,
        layout.environment.0,
        layout.environment.1,
    )
    .ok()?;
    text.bytes(b"\n")?;
    Some(text.0)
}

/// `/proc/<pid>/status`: the lines programs read most, in Linux's layout.
fn process_status<P: Platform>(pid: u32) -> Option<Vec<u8>> {
    let Described {
        listed,
        running,
        process,
    } = described::<P>(pid)?;
    let mut text = Text(Vec::new());
    text.bytes(b"Name:\t")?;
    text.bytes(name_of(&listed))?;
    let state = match listed.ended {
        true => "Z (zombie)",
        false => "R (running)",
    };
    write!(
        text,
        "\nUmask:\t{:04o}\nState:\t{state}\nTgid:\t{pid}\nNgid:\t0\nPid:\t{pid}\n\
         PPid:\t{}\nTracerPid:\t0\n",
        super::file_changes::UMASK,
        listed.parent
    )
    .ok()?;
    if let Some(process) = &process {
        let (user, group, groups) = {
            let state = process.credentials_and_limits().lock();
            let credentials = &state.credentials;
            let mut groups = Vec::new();
            groups.try_reserve_exact(credentials.groups().len()).ok()?;
            groups.extend_from_slice(credentials.groups());
            (
                credentials.ids(super::credentials::Kind::User),
                credentials.ids(super::credentials::Kind::Group),
                groups,
            )
        };
        for (name, ids) in [("Uid", user), ("Gid", group)] {
            writeln!(
                text,
                "{name}:\t{}\t{}\t{}\t{}",
                ids.real, ids.effective, ids.saved, ids.filesystem
            )
            .ok()?;
        }
        let slots = running
            .as_ref()
            .and_then(|running| running.files.slot_count())
            .unwrap_or(0);
        write!(text, "FDSize:\t{slots}\nGroups:\t").ok()?;
        for group in groups {
            write!(text, "{group} ").ok()?;
        }
        let size = mapped_bytes(&process.mappings()?);
        write!(
            text,
            "\nVmSize:\t{:>8} kB\nThreads:\t{}\n",
            size / 1024,
            process.threads()
        )
        .ok()?;
    }
    Some(text.0)
}

/// The number of a node of process `pid`'s directory named `name`, if it has one.
fn process_lookup(directory: ProcessNode, name: &[u8]) -> Option<u64> {
    match directory.part {
        part::DIRECTORY => PROCESS_FILES
            .iter()
            .find(|(file, _)| *file == name)
            .map(|&(_, part)| ProcessNode::part_of(directory.pid, part).number()),
        part::FD => {
            let fd = parse_decimal(name, MAX_FD)?;
            let (_, running) = TABLE.lock().describe(directory.pid)?;
            running?.files.file(fd)?;
            Some(
                ProcessNode {
                    pid: directory.pid,
                    part: part::DESCRIPTOR,
                    fd,
                }
                .number(),
            )
        }
        _ => None,
    }
}

/// Entry `position` of directory `directory` of a process.
fn process_entry(directory: ProcessNode, position: usize) -> Option<DirectoryEntry> {
    match directory.part {
        part::DIRECTORY => {
            let &(name, part) = PROCESS_FILES.get(position)?;
            let node = ProcessNode::part_of(directory.pid, part).number();
            Some(DirectoryEntry {
                name: Name::new(name),
                node,
                kind: kind_of(node),
            })
        }
        part::FD => {
            let (_, running) = TABLE.lock().describe(directory.pid)?;
            let open = running?.files.open_files()?;
            let &(fd, _) = open.get(position)?;
            let mut digits = [0; 20];
            Some(DirectoryEntry {
                name: Name::new(decimal(u64::from(fd), &mut digits)),
                node: ProcessNode {
                    pid: directory.pid,
                    part: part::DESCRIPTOR,
                    fd,
                }
                .number(),
                kind: NodeKind::SymbolicLink,
            })
        }
        _ => None,
    }
}

fn kind_of(node: u64) -> NodeKind {
    if is_directory(node) {
        NodeKind::Directory
    } else if is_link(node) {
        NodeKind::SymbolicLink
    } else {
        NodeKind::Regular
    }
}

impl<P: Platform> FileSystem for Proc<P> {
    fn root(&self) -> u64 {
        ROOT
    }

    fn device(&self) -> (u32, u32) {
        DEVICE
    }

    fn kind(&self, node: u64) -> NodeKind {
        kind_of(node)
    }

    fn attributes(&self, node: u64) -> Attributes {
        let kind = kind_of(node);
        Attributes {
            kind,
            permissions: match kind {
                NodeKind::Directory => 0o555,
                NodeKind::SymbolicLink => 0o777,
                _ => 0o444,
            },
            file_id: node,
            links: if kind == NodeKind::Directory { 2 } else { 1 },
            owner: 0,
            group: 0,
            size: 0,
            modified: 0,
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        if let Some(directory) = ProcessNode::of(directory) {
            return process_lookup(directory, name);
        }
        if let Some(&(_, node)) = listing(directory).iter().find(|(file, _)| *file == name) {
            return Some(node);
        }
        if directory != ROOT {
            return None;
        }
        let pid = parse_decimal(name, MAX_PID)?;
        TABLE.lock().describe(pid)?;
        Some(ProcessNode::part_of(pid, part::DIRECTORY).number())
    }

    fn parent(&self, node: u64) -> u64 {
        if let Some(node) = ProcessNode::of(node) {
            return match node.part {
                part::DIRECTORY => ROOT,
                part::DESCRIPTOR => ProcessNode::part_of(node.pid, part::FD).number(),
                _ => ProcessNode::part_of(node.pid, part::DIRECTORY).number(),
            };
        }
        let holds = |files: &[(&[u8], u64)]| files.iter().any(|&(_, file)| file == node);
        if holds(&NET_FILES) {
            NET
        } else if holds(&DRIVER_FILES) {
            DRIVER
        } else if holds(&SYS_FILES) {
            SYS
        } else if holds(&SYS_KERNEL_FILES) {
            SYS_KERNEL
        } else {
            ROOT
        }
    }

    fn name(&self, node: u64) -> Name {
        let mut digits = [0; 20];
        if let Some(node) = ProcessNode::of(node) {
            return Name::new(match node.part {
                part::DIRECTORY => decimal(u64::from(node.pid), &mut digits),
                part::DESCRIPTOR => decimal(u64::from(node.fd), &mut digits),
                other => PROCESS_FILES
                    .iter()
                    .find(|&&(_, file)| file == other)
                    .map_or(&[][..], |(name, _)| name),
            });
        }
        Name::new(
            FILES
                .iter()
                .chain(NET_FILES.iter())
                .chain(DRIVER_FILES.iter())
                .chain(SYS_FILES.iter())
                .chain(SYS_KERNEL_FILES.iter())
                .find(|&&(_, file)| file == node)
                .map_or(&[][..], |(name, _)| name),
        )
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn generate(&self, node: u64) -> Result<Option<Vec<u8>>, StorageError> {
        if let Some(node) = ProcessNode::of(node) {
            let generated = match node.part {
                part::CMDLINE => process_cmdline::<P>(node.pid),
                part::MAPS => process_maps::<P>(node.pid),
                part::STAT => process_stat::<P>(node.pid),
                part::STATUS => process_status::<P>(node.pid),
                _ => return Ok(None),
            };
            return Ok(Some(generated.unwrap_or_default()));
        }
        let generated = match node {
            MEMINFO => {
                let (free, reserve) = memory::FRAMES.lock().as_ref().map_or((0, 0), |frames| {
                    (frames.free_frames(), frames.user_reserve())
                });
                meminfo(memory::total_frames(), free, reserve)
            }
            UPTIME => uptime(P::clock_ticks(), P::clock_frequency()),
            PS => ps(),
            CMDLINE => cmdline::<P>(),
            NET_UNIX => super::unix_socket::proc_net_unix(),
            NET_INTERFACES => crate::net::interfaces_text::<P>(),
            NET_ROUTE => crate::net::routes_text::<P>(),
            NET_ARP => crate::net::neighbours_text::<P>(),
            DRIVER_RTC => match crate::wall_clock::read_clock::<P>() {
                Some((_, Ok(nanoseconds))) => driver_rtc(nanoseconds / 1_000_000_000),
                _ => return Err(StorageError),
            },
            VERSION => version_text(),
            OSRELEASE => line(identity::utsname::RELEASE.as_bytes()),
            OSTYPE => line(identity::utsname::SYSNAME.as_bytes()),
            HOSTNAME => {
                let mut name = [0; identity::MAX_NAME];
                let length = identity::host_name(&mut name);
                line(&name[..length])
            }
            _ => None,
        };
        Ok(generated)
    }

    fn link_target(&self, node: u64, caller: u32) -> Option<Vec<u8>> {
        if node == SELF {
            if caller == KERNEL_CALLER {
                return None;
            }
            let mut digits = [0; 20];
            return Some(decimal(u64::from(caller), &mut digits).to_vec());
        }
        let node = ProcessNode::of(node)?;
        match node.part {
            part::EXE => {
                let file = running_process::<P>(node.pid)?
                    .image_layout()
                    .executable
                    .clone()?;
                path_of(&*file)
            }
            part::DESCRIPTOR => {
                let (_, running) = TABLE.lock().describe(node.pid)?;
                let file = running?.files.file(node.fd)?;
                descriptor_target(file.kind())
            }
            _ => None,
        }
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let position = usize::try_from(position).ok()?;
        if let Some(directory) = ProcessNode::of(directory) {
            return process_entry(directory, position);
        }
        let files = listing(directory);
        if let Some(&(name, node)) = files.get(position) {
            return Some(DirectoryEntry {
                name: Name::new(name),
                node,
                kind: kind_of(node),
            });
        }
        if directory != ROOT {
            return None;
        }
        let pid = TABLE.lock().list().nth(position - files.len())?.id;
        let mut digits = [0; 20];
        Some(DirectoryEntry {
            name: Name::new(decimal(u64::from(pid), &mut digits)),
            node: ProcessNode::part_of(pid, part::DIRECTORY).number(),
            kind: NodeKind::Directory,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regions::BytesFile;
    use crate::wall_clock::host::with_clock;
    use alloc::boxed::Box;
    use eggos_drivers::rtc::{RealTimeClock, RtcError};

    #[test]
    fn meminfo_is_in_kilobytes_aligned_as_linux_s() {
        let text = meminfo(32768, 1000, 0).unwrap();
        assert_eq!(
            core::str::from_utf8(&text).unwrap(),
            "MemTotal:         131072 kB\nMemFree:            4000 kB\nMemAvailable:       4000 kB\n\
             KernelReserve:         0 kB\n"
        );
        let text = meminfo(32768, 1000, 1024 + 600).unwrap();
        assert!(
            core::str::from_utf8(&text)
                .unwrap()
                .ends_with("MemAvailable:          0 kB\nKernelReserve:      6496 kB\n")
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
    fn version_and_sys_kernel_say_what_uname_says() {
        let proc = Proc::<crate::tests::TestPlatform>::new();
        let version = proc.generate(VERSION).unwrap().unwrap();
        let version = core::str::from_utf8(&version).unwrap();
        assert!(
            version.starts_with("Linux version 6.1.0-eggos ("),
            "{version}"
        );
        assert!(version.ends_with(concat!(" #1 SMP EggOS ", env!("CARGO_PKG_VERSION"), "\n")));
        assert_eq!(proc.lookup(ROOT, b"sys"), Some(SYS));
        assert_eq!(proc.lookup(SYS, b"kernel"), Some(SYS_KERNEL));
        assert_eq!(proc.parent(SYS_KERNEL), SYS);
        assert_eq!(proc.parent(OSRELEASE), SYS_KERNEL);
        assert_eq!(proc.name(OSTYPE).as_bytes(), b"ostype");
        assert_eq!(
            proc.generate(OSRELEASE),
            Ok(Some(b"6.1.0-eggos\n".to_vec()))
        );
        assert_eq!(proc.generate(OSTYPE), Ok(Some(b"Linux\n".to_vec())));
        let _names = identity::tests::NAMES_TEST.lock().unwrap();
        identity::forget_changed_names();
        assert_eq!(proc.generate(HOSTNAME), Ok(Some(b"localhost\n".to_vec())));
        let entries: Vec<Vec<u8>> = (0..)
            .map_while(|position| proc.entry(SYS_KERNEL, position))
            .map(|entry| entry.name.as_bytes().to_vec())
            .collect();
        assert_eq!(entries, [&b"hostname"[..], b"osrelease", b"ostype"]);
    }

    #[test]
    fn numbers_are_read_as_linux_reads_process_and_descriptor_names() {
        let mut digits = [0; 20];
        assert_eq!(decimal(0, &mut digits), b"0");
        assert_eq!(decimal(u64::MAX, &mut digits), b"18446744073709551615");
        assert_eq!(parse_decimal(b"42", MAX_PID), Some(42));
        assert_eq!(parse_decimal(b"0", MAX_PID), Some(0));
        for name in [&b""[..], b"042", b"4a", b"-1", b"99999999999"] {
            assert_eq!(parse_decimal(name, MAX_PID), None);
        }
        assert_eq!(parse_decimal(b"1073741824", MAX_PID), None);
        let node = ProcessNode {
            pid: 1234,
            part: part::DESCRIPTOR,
            fd: 1 << 20,
        };
        assert_eq!(ProcessNode::of(node.number()), Some(node));
        assert_eq!(ProcessNode::of(SELF), None);
        assert!(is_directory(ProcessNode::part_of(7, part::FD).number()));
        assert!(is_link(ProcessNode::part_of(7, part::EXE).number()));
        assert_eq!(
            kind_of(ProcessNode::part_of(7, part::MAPS).number()),
            NodeKind::Regular
        );
    }

    #[test]
    fn the_root_lists_and_finds_its_files_and_net() {
        let proc = Proc::<crate::tests::TestPlatform>::new();
        let names: Vec<Vec<u8>> = (0..FILES.len() as u64)
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
                b"self",
                b"sys",
                b"uptime",
                b"version"
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
        for (name, node) in [
            (&b"arp"[..], NET_ARP),
            (b"interfaces", NET_INTERFACES),
            (b"route", NET_ROUTE),
        ] {
            assert_eq!(proc.lookup(NET, name), Some(node));
            assert_eq!(proc.parent(node), NET);
            assert_eq!(proc.name(node).as_bytes(), name);
            assert!(matches!(proc.generate(node), Ok(Some(_))));
        }
        assert_eq!(proc.lookup(ROOT, b"cmdline"), Some(CMDLINE));
        assert_eq!(proc.lookup(ROOT, b"ps"), Some(PS));
        assert_eq!(proc.lookup(ROOT, b"self"), Some(SELF));
        assert_eq!(proc.kind(SELF), NodeKind::SymbolicLink);
        assert_eq!(proc.link_target(SELF, 42), Some(b"42".to_vec()));
        assert_eq!(proc.link_target(SELF, KERNEL_CALLER), None);
        assert_eq!(proc.name(UPTIME).as_bytes(), b"uptime");
        assert_eq!(proc.attributes(MEMINFO).permissions, 0o444);
        assert_eq!(proc.kind(ROOT), NodeKind::Directory);
    }

    #[test]
    fn a_maps_line_is_padded_to_its_name_as_linux_writes_it() {
        let mut text = Text(Vec::new());
        let file: Arc<dyn FilePages> = Arc::new(BytesFile(Vec::new()));
        let read_execute = eggos_hal::paging::Attributes::USER_TEXT;
        write_mapping(
            &mut text,
            &Mapping {
                start: 0x40_0000,
                end: 0x40_2000,
                attributes: Some(read_execute),
                shared: false,
                file: Some((file, 0x1000)),
                label: MappingLabel::Plain,
            },
        )
        .unwrap();
        assert_eq!(text.0, b"00400000-00402000 r-xp 00001000 00:00 0 \n");
        let mut text = Text(Vec::new());
        write_mapping(
            &mut text,
            &Mapping {
                start: 0x7fff_f000_0000,
                end: 0x7fff_f080_0000,
                attributes: Some(eggos_hal::paging::Attributes::USER_READ_WRITE),
                shared: false,
                file: None,
                label: MappingLabel::Stack,
            },
        )
        .unwrap();
        let line = core::str::from_utf8(&text.0).unwrap();
        assert_eq!(line.find("[stack]"), Some(MAPS_NAME_COLUMN));
        assert!(line.starts_with("7ffff0000000-7ffff0800000 rw-p 00000000 00:00 0 "));
        assert!(line.ends_with(" [stack]\n"));
        let mut text = Text(Vec::new());
        write_mapping(
            &mut text,
            &Mapping {
                start: 0x1000,
                end: 0x2000,
                attributes: None,
                shared: true,
                file: None,
                label: MappingLabel::Plain,
            },
        )
        .unwrap();
        assert_eq!(text.0, b"00001000-00002000 ---s 00000000 00:00 0 \n");
    }
}
