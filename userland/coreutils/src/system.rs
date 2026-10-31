use core::ffi::CStr;
use eggos_commands::tools::NO_SYMLINKS_ENVIRONMENT;
use eggos_net::Ipv4Address;
use eggos_runtime::errno::Errno;

pub const STANDARD_INPUT: usize = 0;
pub const STANDARD_OUTPUT: usize = 1;
pub const STANDARD_ERROR: usize = 2;

pub const STATX_SIZE: usize = 256;
pub const UTSNAME_FIELD: usize = 65;
pub const UTSNAME_SIZE: usize = 6 * UTSNAME_FIELD;

pub const FILE_TYPE: u32 = 0o170_000;
pub const DIRECTORY: u32 = 0o040_000;
pub const SYMBOLIC_LINK: u32 = 0o120_000;

pub const MAX_LINK_HOPS: usize = 40;

pub const STATFS_SIZE: usize = 120;

const STATFS_READ_ONLY: u64 = 0x1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileSystemUsage {
    pub block_size: u64,
    pub blocks: u64,
    pub free: u64,
    pub available: u64,
    pub read_only: bool,
}

impl FileSystemUsage {
    pub fn from_statfs(bytes: &[u8; STATFS_SIZE]) -> Self {
        let word = |offset: usize| {
            let mut value = [0; 8];
            value.copy_from_slice(&bytes[offset..offset + 8]);
            u64::from_ne_bytes(value)
        };
        let fragment = word(72);
        Self {
            block_size: if fragment == 0 { word(8) } else { fragment },
            blocks: word(16),
            free: word(24),
            available: word(32),
            read_only: word(80) & STATFS_READ_ONLY != 0,
        }
    }

    pub fn size_bytes(&self) -> u64 {
        self.blocks.saturating_mul(self.block_size)
    }

    pub fn used_bytes(&self) -> u64 {
        self.blocks
            .saturating_sub(self.free)
            .saturating_mul(self.block_size)
    }

    pub fn available_bytes(&self) -> u64 {
        self.available.saturating_mul(self.block_size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    pub mode: u32,
    pub links: u32,
    pub owner: u32,
    pub group: u32,
    pub size: u64,
    pub blocks: u64,
    pub modified: i64,
    pub inode: u64,
    pub device: u64,
}

impl Status {
    pub fn from_statx(bytes: &[u8; STATX_SIZE]) -> Self {
        let u16_at = |offset: usize| u16::from_ne_bytes([bytes[offset], bytes[offset + 1]]);
        let u32_at = |offset: usize| {
            let mut word = [0; 4];
            word.copy_from_slice(&bytes[offset..offset + 4]);
            u32::from_ne_bytes(word)
        };
        let u64_at = |offset: usize| {
            let mut word = [0; 8];
            word.copy_from_slice(&bytes[offset..offset + 8]);
            u64::from_ne_bytes(word)
        };
        Self {
            mode: u32::from(u16_at(28)),
            links: u32_at(16),
            owner: u32_at(20),
            group: u32_at(24),
            size: u64_at(40),
            blocks: u64_at(48),
            modified: u64_at(112) as i64,
            inode: u64_at(32),
            device: u64::from(u32_at(136)) << 32 | u64::from(u32_at(140)),
        }
    }

    pub fn is_directory(&self) -> bool {
        self.mode & FILE_TYPE == DIRECTORY
    }

    pub fn is_link(&self) -> bool {
        self.mode & FILE_TYPE == SYMBOLIC_LINK
    }

    pub fn is_same_file(&self, other: &Status) -> bool {
        self.inode == other.inode && self.device == other.device
    }
}

pub fn folder_and_leaf(path: &[u8]) -> (&[u8], usize) {
    let Some(last) = path.iter().rposition(|&byte| byte != b'/') else {
        let folder: &[u8] = if path.is_empty() { b"." } else { b"/" };
        return (folder, path.len());
    };
    let start = path[..last]
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |slash| slash + 1);
    let folder: &[u8] = match path[..start].iter().rposition(|&byte| byte != b'/') {
        Some(end) => &path[..=end],
        None if start > 0 => b"/",
        None => b".",
    };
    (folder, start)
}

pub fn refuses_symbolic_links<'a>(environment: impl IntoIterator<Item = &'a CStr>) -> bool {
    environment
        .into_iter()
        .any(|variable| variable == NO_SYMLINKS_ENVIRONMENT)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemName(pub [u8; UTSNAME_SIZE]);

impl SystemName {
    pub fn field(&self, index: usize) -> &[u8] {
        let field = &self.0[index * UTSNAME_FIELD..][..UTSNAME_FIELD];
        let end = field
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(field.len());
        &field[..end]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duration {
    pub seconds: u64,
    pub nanoseconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolicLinks {
    Follow,
    DoNotFollow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedRows {
    pub width: u32,
    pub height: u32,
    pub rows: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowTree {
    pub pixels_only: bool,
    pub length: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatagramKind {
    Udp,
    Ping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sender {
    pub address: Ipv4Address,
    pub port: u16,
}

pub trait System {
    fn write(&mut self, descriptor: usize, bytes: &[u8]) -> Result<(), Errno>;
    fn read(&mut self, descriptor: usize, buffer: &mut [u8]) -> Result<usize, Errno>;
    fn open(&mut self, path: &CStr) -> Result<usize, Errno>;
    fn close(&mut self, descriptor: usize);
    fn directory_entries(&mut self, descriptor: usize, buffer: &mut [u8]) -> Result<usize, Errno>;
    fn status(
        &mut self,
        directory: Option<usize>,
        path: &CStr,
        links: SymbolicLinks,
    ) -> Result<Status, Errno>;
    fn system_name(&mut self) -> Result<SystemName, Errno>;
    fn sleep(&mut self, duration: Duration) -> Result<(), Errno>;
    fn sync(&mut self);
    fn reboot(&mut self, command: u32) -> Result<(), Errno>;
    fn create(&mut self, path: &CStr, mode: u32, truncate: bool) -> Result<usize, Errno>;
    fn append(&mut self, path: &CStr, mode: u32) -> Result<usize, Errno>;
    fn make_directory(&mut self, path: &CStr, mode: u32) -> Result<(), Errno>;
    fn remove(&mut self, path: &CStr, directory: bool) -> Result<(), Errno>;
    fn rename(&mut self, from: &CStr, to: &CStr) -> Result<(), Errno>;
    fn set_modified_now(&mut self, path: &CStr) -> Result<(), Errno>;
    fn read_link(&mut self, path: &CStr, target: &mut [u8]) -> Result<usize, Errno>;
    fn make_link(&mut self, target: &CStr, path: &CStr) -> Result<(), Errno>;
    fn make_hard_link(&mut self, existing: &CStr, path: &CStr) -> Result<(), Errno>;
    fn set_mode(&mut self, path: &CStr, mode: u32) -> Result<(), Errno>;
    fn file_system_usage(&mut self, path: &CStr) -> Result<FileSystemUsage, Errno>;
    fn now(&mut self) -> u64;
    fn time_zone_variable(&self) -> Option<&[u8]> {
        None
    }
    fn now_with_nanoseconds(&mut self) -> Duration;
    fn set_time(&mut self, since_epoch: Duration) -> Result<(), Errno>;
    fn working_directory(&mut self, out: &mut [u8]) -> Result<usize, Errno>;
    fn device_control(
        &mut self,
        descriptor: usize,
        request: u32,
        argument: &mut [u8],
    ) -> Result<(), Errno>;
    fn capture_window_rows(
        &mut self,
        descriptor: usize,
        window: u32,
        first_row: u32,
        row_count: u32,
        pixels: &mut [u8],
    ) -> Result<CapturedRows, Errno>;
    fn window_tree(
        &mut self,
        descriptor: usize,
        window: u32,
        tree: &mut [u8],
    ) -> Result<WindowTree, Errno>;
    fn set_clipboard(&mut self, descriptor: usize, text: &[u8]) -> Result<(), Errno>;
    fn clipboard(
        &mut self,
        descriptor: usize,
        offset: usize,
        into: &mut [u8],
    ) -> Result<(usize, u32), Errno>;
    fn open_datagram_socket(&mut self, kind: DatagramKind) -> Result<usize, Errno>;
    fn send_to(
        &mut self,
        descriptor: usize,
        bytes: &[u8],
        address: Ipv4Address,
        port: u16,
    ) -> Result<usize, Errno>;
    fn receive_from(
        &mut self,
        descriptor: usize,
        buffer: &mut [u8],
        timeout_ms: u32,
    ) -> Result<Option<(usize, Sender)>, Errno>;
    fn monotonic_us(&mut self) -> u64;
    fn open_stream_socket(&mut self) -> Result<usize, Errno>;
    fn connect(
        &mut self,
        descriptor: usize,
        address: Ipv4Address,
        port: u16,
        timeout_ms: u32,
    ) -> Result<(), Errno>;
    fn send(&mut self, descriptor: usize, bytes: &[u8]) -> Result<(), Errno>;
    fn receive(
        &mut self,
        descriptor: usize,
        buffer: &mut [u8],
        timeout_ms: u32,
    ) -> Result<Option<usize>, Errno>;
    fn random(&mut self, out: &mut [u8]) -> Result<(), Errno>;
    fn signal_init(&mut self, signal: u32) -> Result<(), Errno>;
    fn terminal_columns(&mut self, descriptor: usize) -> Option<u16>;
}

impl<T: System + ?Sized> System for &mut T {
    fn write(&mut self, descriptor: usize, bytes: &[u8]) -> Result<(), Errno> {
        (**self).write(descriptor, bytes)
    }

    fn read(&mut self, descriptor: usize, buffer: &mut [u8]) -> Result<usize, Errno> {
        (**self).read(descriptor, buffer)
    }

    fn open(&mut self, path: &CStr) -> Result<usize, Errno> {
        (**self).open(path)
    }

    fn close(&mut self, descriptor: usize) {
        (**self).close(descriptor);
    }

    fn directory_entries(&mut self, descriptor: usize, buffer: &mut [u8]) -> Result<usize, Errno> {
        (**self).directory_entries(descriptor, buffer)
    }

    fn status(
        &mut self,
        directory: Option<usize>,
        path: &CStr,
        links: SymbolicLinks,
    ) -> Result<Status, Errno> {
        (**self).status(directory, path, links)
    }

    fn system_name(&mut self) -> Result<SystemName, Errno> {
        (**self).system_name()
    }

    fn sleep(&mut self, duration: Duration) -> Result<(), Errno> {
        (**self).sleep(duration)
    }

    fn sync(&mut self) {
        (**self).sync();
    }

    fn reboot(&mut self, command: u32) -> Result<(), Errno> {
        (**self).reboot(command)
    }

    fn create(&mut self, path: &CStr, mode: u32, truncate: bool) -> Result<usize, Errno> {
        (**self).create(path, mode, truncate)
    }

    fn append(&mut self, path: &CStr, mode: u32) -> Result<usize, Errno> {
        (**self).append(path, mode)
    }

    fn make_directory(&mut self, path: &CStr, mode: u32) -> Result<(), Errno> {
        (**self).make_directory(path, mode)
    }

    fn remove(&mut self, path: &CStr, directory: bool) -> Result<(), Errno> {
        (**self).remove(path, directory)
    }

    fn rename(&mut self, from: &CStr, to: &CStr) -> Result<(), Errno> {
        (**self).rename(from, to)
    }

    fn set_modified_now(&mut self, path: &CStr) -> Result<(), Errno> {
        (**self).set_modified_now(path)
    }

    fn read_link(&mut self, path: &CStr, target: &mut [u8]) -> Result<usize, Errno> {
        (**self).read_link(path, target)
    }

    fn make_link(&mut self, target: &CStr, path: &CStr) -> Result<(), Errno> {
        (**self).make_link(target, path)
    }

    fn make_hard_link(&mut self, existing: &CStr, path: &CStr) -> Result<(), Errno> {
        (**self).make_hard_link(existing, path)
    }

    fn set_mode(&mut self, path: &CStr, mode: u32) -> Result<(), Errno> {
        (**self).set_mode(path, mode)
    }

    fn file_system_usage(&mut self, path: &CStr) -> Result<FileSystemUsage, Errno> {
        (**self).file_system_usage(path)
    }

    fn now(&mut self) -> u64 {
        (**self).now()
    }

    fn now_with_nanoseconds(&mut self) -> Duration {
        (**self).now_with_nanoseconds()
    }

    fn set_time(&mut self, since_epoch: Duration) -> Result<(), Errno> {
        (**self).set_time(since_epoch)
    }

    fn working_directory(&mut self, out: &mut [u8]) -> Result<usize, Errno> {
        (**self).working_directory(out)
    }

    fn device_control(
        &mut self,
        descriptor: usize,
        request: u32,
        argument: &mut [u8],
    ) -> Result<(), Errno> {
        (**self).device_control(descriptor, request, argument)
    }

    fn capture_window_rows(
        &mut self,
        descriptor: usize,
        window: u32,
        first_row: u32,
        row_count: u32,
        pixels: &mut [u8],
    ) -> Result<CapturedRows, Errno> {
        (**self).capture_window_rows(descriptor, window, first_row, row_count, pixels)
    }

    fn window_tree(
        &mut self,
        descriptor: usize,
        window: u32,
        tree: &mut [u8],
    ) -> Result<WindowTree, Errno> {
        (**self).window_tree(descriptor, window, tree)
    }

    fn set_clipboard(&mut self, descriptor: usize, text: &[u8]) -> Result<(), Errno> {
        (**self).set_clipboard(descriptor, text)
    }

    fn clipboard(
        &mut self,
        descriptor: usize,
        offset: usize,
        into: &mut [u8],
    ) -> Result<(usize, u32), Errno> {
        (**self).clipboard(descriptor, offset, into)
    }

    fn open_datagram_socket(&mut self, kind: DatagramKind) -> Result<usize, Errno> {
        (**self).open_datagram_socket(kind)
    }

    fn send_to(
        &mut self,
        descriptor: usize,
        bytes: &[u8],
        address: Ipv4Address,
        port: u16,
    ) -> Result<usize, Errno> {
        (**self).send_to(descriptor, bytes, address, port)
    }

    fn receive_from(
        &mut self,
        descriptor: usize,
        buffer: &mut [u8],
        timeout_ms: u32,
    ) -> Result<Option<(usize, Sender)>, Errno> {
        (**self).receive_from(descriptor, buffer, timeout_ms)
    }

    fn monotonic_us(&mut self) -> u64 {
        (**self).monotonic_us()
    }

    fn open_stream_socket(&mut self) -> Result<usize, Errno> {
        (**self).open_stream_socket()
    }

    fn connect(
        &mut self,
        descriptor: usize,
        address: Ipv4Address,
        port: u16,
        timeout_ms: u32,
    ) -> Result<(), Errno> {
        (**self).connect(descriptor, address, port, timeout_ms)
    }

    fn send(&mut self, descriptor: usize, bytes: &[u8]) -> Result<(), Errno> {
        (**self).send(descriptor, bytes)
    }

    fn receive(
        &mut self,
        descriptor: usize,
        buffer: &mut [u8],
        timeout_ms: u32,
    ) -> Result<Option<usize>, Errno> {
        (**self).receive(descriptor, buffer, timeout_ms)
    }

    fn random(&mut self, out: &mut [u8]) -> Result<(), Errno> {
        (**self).random(out)
    }

    fn signal_init(&mut self, signal: u32) -> Result<(), Errno> {
        (**self).signal_init(signal)
    }

    fn terminal_columns(&mut self, descriptor: usize) -> Option<u16> {
        (**self).terminal_columns(descriptor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_statx_record_is_read_at_linux_s_offsets() {
        let mut bytes = [0u8; STATX_SIZE];
        bytes[16..20].copy_from_slice(&3u32.to_ne_bytes());
        bytes[20..24].copy_from_slice(&1000u32.to_ne_bytes());
        bytes[24..28].copy_from_slice(&100u32.to_ne_bytes());
        bytes[28..30].copy_from_slice(&0o040_755u16.to_ne_bytes());
        bytes[32..40].copy_from_slice(&77u64.to_ne_bytes());
        bytes[40..48].copy_from_slice(&4096u64.to_ne_bytes());
        bytes[48..56].copy_from_slice(&8u64.to_ne_bytes());
        bytes[112..120].copy_from_slice(&1_700_000_000i64.to_ne_bytes());
        bytes[136..140].copy_from_slice(&254u32.to_ne_bytes());
        bytes[140..144].copy_from_slice(&1u32.to_ne_bytes());
        let status = Status::from_statx(&bytes);
        assert_eq!(
            status,
            Status {
                mode: 0o040_755,
                links: 3,
                owner: 1000,
                group: 100,
                size: 4096,
                blocks: 8,
                modified: 1_700_000_000,
                inode: 77,
                device: 254 << 32 | 1,
            }
        );
        assert!(status.is_directory() && !status.is_link());
        let link = Status {
            mode: SYMBOLIC_LINK | 0o777,
            ..status
        };
        assert!(link.is_link() && !link.is_directory());
        assert!(link.is_same_file(&status));
        assert!(!link.is_same_file(&Status {
            inode: 78,
            ..status
        }));
    }

    #[test]
    fn a_path_splits_into_its_folder_and_its_last_component() {
        let split = |path: &'static [u8]| {
            let (folder, start) = folder_and_leaf(path);
            (folder, &path[start..])
        };
        assert_eq!(split(b"/home/root/a"), (&b"/home/root"[..], &b"a"[..]));
        assert_eq!(split(b"/a"), (&b"/"[..], &b"a"[..]));
        assert_eq!(split(b"//a"), (&b"/"[..], &b"a"[..]));
        assert_eq!(split(b"a"), (&b"."[..], &b"a"[..]));
        assert_eq!(split(b"a/b/"), (&b"a"[..], &b"b/"[..]));
        assert_eq!(split(b"a//b"), (&b"a"[..], &b"b"[..]));
        assert_eq!(split(b"/"), (&b"/"[..], &b""[..]));
        assert_eq!(split(b""), (&b"."[..], &b""[..]));
    }

    #[test]
    fn links_are_refused_only_when_the_environment_says_exactly_so() {
        assert!(refuses_symbolic_links([
            c"PATH=/bin",
            c"EGGOS_NO_SYMLINKS=1"
        ]));
        assert!(!refuses_symbolic_links([c"PATH=/bin"]));
        assert!(!refuses_symbolic_links([c"EGGOS_NO_SYMLINKS=0"]));
        assert!(!refuses_symbolic_links([c"EGGOS_NO_SYMLINKS=1x"]));
    }

    #[test]
    fn a_system_name_field_ends_at_its_first_nul() {
        let mut bytes = [0u8; UTSNAME_SIZE];
        bytes[..5].copy_from_slice(b"Linux");
        bytes[UTSNAME_FIELD * 5..][..UTSNAME_FIELD].fill(b'x');
        let name = SystemName(bytes);
        assert_eq!(name.field(0), b"Linux");
        assert_eq!(name.field(1), b"");
        assert_eq!(name.field(5).len(), UTSNAME_FIELD);
    }
}
