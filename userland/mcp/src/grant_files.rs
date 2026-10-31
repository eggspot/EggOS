//! Reading and writing the agents' grants file (D193) the one way every writer uses: the
//! `agent` command and Settings' Agents pane. A write goes to `agent-grants.new`, is synced and
//! renamed over the file, after making `/var/lib/eggos`, so a reader sees the old file or the
//! new one, never half of one.

use crate::grants::{GrantStore, STORE_BYTES, StoreError, StoreState};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;

/// The grants file.
pub const STORE: &CStr = c"/var/lib/eggos/agent-grants";
/// Where a new grants file is written before it is renamed over [`STORE`].
pub const STORE_TEMPORARY: &CStr = c"/var/lib/eggos/agent-grants.new";
/// The folders made, in order, before the grants file is written.
pub const STORE_FOLDERS: [&CStr; 3] = [c"/var", c"/var/lib", c"/var/lib/eggos"];
/// The grants file's mode: only its owner reads or writes it.
pub const STORE_MODE: u32 = 0o600;
/// The mode of the folders made for it.
pub const FOLDER_MODE: u32 = 0o755;

/// The file operations reading and writing the grants file needs.
pub trait GrantFiles {
    /// Reads the file at `path` from its start into `out`, until its end or `out` is full.
    fn read_whole(&mut self, path: &CStr, out: &mut [u8]) -> Result<usize, Errno>;
    /// Makes the folder `path` with `mode`; `EEXIST` when it is there.
    fn make_directory(&mut self, path: &CStr, mode: u32) -> Result<(), Errno>;
    /// Creates or truncates the file at `path` with `mode` and writes all of `bytes` to it.
    fn write_new(&mut self, path: &CStr, mode: u32, bytes: &[u8]) -> Result<(), Errno>;
    /// Makes what was written durable.
    fn sync(&mut self);
    /// Renames `from` over `to`.
    fn rename(&mut self, from: &CStr, to: &CStr) -> Result<(), Errno>;
}

/// Why the grants file was not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveError {
    /// The store cannot be written as a file (too many agents, too large).
    Store(StoreError),
    /// This file or folder could not be made, written or renamed.
    File(&'static CStr, Errno),
}

/// Reads the grants file: [`StoreState::Missing`] when there is none.
pub fn load(files: &mut impl GrantFiles) -> StoreState {
    let mut file = [0u8; STORE_BYTES];
    let read = files.read_whole(STORE, &mut file);
    StoreState::from_read(read.map(|length| file.get(..length).unwrap_or_default()))
}

/// Writes `store` as the grants file, through [`STORE_TEMPORARY`], a sync and a rename.
pub fn save(files: &mut impl GrantFiles, store: &GrantStore) -> Result<(), SaveError> {
    let mut file = [0u8; STORE_BYTES];
    let length = store.encode(&mut file).map_err(SaveError::Store)?;
    for folder in STORE_FOLDERS {
        match files.make_directory(folder, FOLDER_MODE) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(errno) => return Err(SaveError::File(folder, errno)),
        }
    }
    let bytes = file.get(..length).unwrap_or_default();
    files
        .write_new(STORE_TEMPORARY, STORE_MODE, bytes)
        .map_err(|errno| SaveError::File(STORE_TEMPORARY, errno))?;
    files.sync();
    files
        .rename(STORE_TEMPORARY, STORE)
        .map_err(|errno| SaveError::File(STORE, errno))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grants::{AgentId, AgentName, Grant, Permission, STORE_PATH, STORE_TEMPORARY_PATH};
    use std::string::String;
    use std::vec::Vec;

    #[derive(Default)]
    struct Fake {
        file: Option<Vec<u8>>,
        temporary: Option<Vec<u8>>,
        steps: Vec<String>,
        failing_write: Option<Errno>,
    }

    impl GrantFiles for Fake {
        fn read_whole(&mut self, _: &CStr, out: &mut [u8]) -> Result<usize, Errno> {
            let file = self.file.as_ref().ok_or(Errno::ENOENT)?;
            let length = file.len().min(out.len());
            out[..length].copy_from_slice(&file[..length]);
            Ok(length)
        }

        fn make_directory(&mut self, path: &CStr, _: u32) -> Result<(), Errno> {
            self.steps.push(String::from(path.to_str().unwrap_or("?")));
            Err(Errno::EEXIST)
        }

        fn write_new(&mut self, path: &CStr, mode: u32, bytes: &[u8]) -> Result<(), Errno> {
            assert_eq!(mode, STORE_MODE);
            self.steps
                .push(std::format!("write {}", path.to_str().unwrap_or("?")));
            if let Some(errno) = self.failing_write {
                return Err(errno);
            }
            self.temporary = Some(bytes.to_vec());
            Ok(())
        }

        fn sync(&mut self) {
            self.steps.push(String::from("sync"));
        }

        fn rename(&mut self, from: &CStr, to: &CStr) -> Result<(), Errno> {
            self.steps.push(std::format!(
                "rename {} {}",
                from.to_str().unwrap_or("?"),
                to.to_str().unwrap_or("?")
            ));
            self.file = self.temporary.take();
            Ok(())
        }
    }

    fn phone_store() -> GrantStore {
        let mut store = GrantStore::new();
        let id = AgentId::new(b"phone").unwrap();
        let name = AgentName::new(b"Claude on my phone").unwrap();
        store
            .add(id, name)
            .unwrap()
            .set(Grant::View, Permission::Allow, None);
        store
    }

    #[test]
    fn the_paths_are_the_grants_module_s() {
        assert_eq!(STORE.to_str(), Ok(STORE_PATH));
        assert_eq!(STORE_TEMPORARY.to_str(), Ok(STORE_TEMPORARY_PATH));
    }

    #[test]
    fn a_save_makes_the_folders_writes_the_new_file_syncs_and_renames_it() {
        let mut fake = Fake::default();
        assert_eq!(save(&mut fake, &phone_store()), Ok(()));
        assert_eq!(
            fake.steps,
            [
                "/var",
                "/var/lib",
                "/var/lib/eggos",
                "write /var/lib/eggos/agent-grants.new",
                "sync",
                "rename /var/lib/eggos/agent-grants.new /var/lib/eggos/agent-grants",
            ]
        );
        assert_eq!(load(&mut fake), StoreState::Loaded(phone_store()));
    }

    #[test]
    fn a_failed_write_names_the_new_file_and_renames_nothing() {
        let mut fake = Fake {
            failing_write: Some(Errno::EROFS),
            ..Fake::default()
        };
        assert_eq!(
            save(&mut fake, &phone_store()),
            Err(SaveError::File(STORE_TEMPORARY, Errno::EROFS))
        );
        assert!(!fake.steps.iter().any(|step| step.starts_with("rename")));
        assert_eq!(load(&mut fake), StoreState::Missing);
    }
}
