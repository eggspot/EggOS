use core::ffi::CStr;
use eggos_runtime::errno::Errno;

pub const STANDARD_INPUT: usize = 0;
pub const STANDARD_OUTPUT: usize = 1;
pub const STANDARD_ERROR: usize = 2;

pub const STATX_SIZE: usize = 256;
pub const UTSNAME_FIELD: usize = 65;
pub const UTSNAME_SIZE: usize = 6 * UTSNAME_FIELD;

pub const FILE_TYPE: u32 = 0o170_000;
pub const DIRECTORY: u32 = 0o040_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    pub mode: u32,
    pub links: u32,
    pub owner: u32,
    pub group: u32,
    pub size: u64,
    pub blocks: u64,
    pub modified: i64,
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
        }
    }

    pub fn is_directory(&self) -> bool {
        self.mode & FILE_TYPE == DIRECTORY
    }
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
    fn reboot(&mut self, command: u32) -> Result<(), Errno>;
    fn create(&mut self, path: &CStr, mode: u32, truncate: bool) -> Result<usize, Errno>;
    fn make_directory(&mut self, path: &CStr, mode: u32) -> Result<(), Errno>;
    fn remove(&mut self, path: &CStr, directory: bool) -> Result<(), Errno>;
    fn rename(&mut self, from: &CStr, to: &CStr) -> Result<(), Errno>;
    fn set_modified_now(&mut self, path: &CStr) -> Result<(), Errno>;
    fn device_control(
        &mut self,
        descriptor: usize,
        request: u32,
        argument: &mut [u8],
    ) -> Result<(), Errno>;
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

    fn reboot(&mut self, command: u32) -> Result<(), Errno> {
        (**self).reboot(command)
    }

    fn create(&mut self, path: &CStr, mode: u32, truncate: bool) -> Result<usize, Errno> {
        (**self).create(path, mode, truncate)
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

    fn device_control(
        &mut self,
        descriptor: usize,
        request: u32,
        argument: &mut [u8],
    ) -> Result<(), Errno> {
        (**self).device_control(descriptor, request, argument)
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
        bytes[40..48].copy_from_slice(&4096u64.to_ne_bytes());
        bytes[48..56].copy_from_slice(&8u64.to_ne_bytes());
        bytes[112..120].copy_from_slice(&1_700_000_000i64.to_ne_bytes());
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
            }
        );
        assert!(status.is_directory());
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
