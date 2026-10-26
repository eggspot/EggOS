//! Builds the initramfs (D102): a `newc` cpio archive of the repository's `initramfs/`
//! directory, which the kernel embeds and mounts as its root, with the programs and files the
//! kernel needs: `/init` for the release kernel (D106), and for test kernels the programs they
//! start from it with `execve` (D103), in `usr/libexec/`.
//!
//! The archive is deterministic: members in name order, each directory before what it holds,
//! the extra members last, owned by root, directories and programs `0755` and other files
//! `0644`,
//! every modification time `SOURCE_DATE_EPOCH` (0 if unset). Only directories and regular
//! files are taken.

use std::path::{Path, PathBuf};

const DIRECTORY_MODE: u32 = 0o040755;
const FILE_MODE: u32 = 0o100644;
const PROGRAM_MODE: u32 = 0o100755;
const TRAILER: &str = "TRAILER!!!";

/// What an extra member of the archive holds.
#[derive(Debug, Clone, Copy)]
pub enum Content<'a> {
    /// The program at this path on the host, made executable.
    Program(&'a str),
    /// This text, as a file of its own.
    Text(&'a str),
}

/// Writes the archive of `initramfs/` and `extra` (each a path in the archive and what it
/// holds, after the directory's own members, with the directories it needs) to `file_name` in
/// the target directory, unless an identical one is there (so the kernel is not rebuilt for
/// nothing), and returns its path.
pub fn build(extra: &[(&str, Content)], file_name: &str) -> Result<PathBuf, String> {
    let root = crate::workspace_root();
    let source = root.join("initramfs");
    let modified = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("SOURCE_DATE_EPOCH `{value}` is not a number of seconds"))?,
        Err(_) => 0,
    };
    let mut members = vec![(".".to_owned(), DIRECTORY_MODE, Vec::new())];
    collect(&source, "", &mut members)?;
    for (path, content) in extra {
        add_directories(path, &mut members);
        let member = match content {
            Content::Program(file) => {
                let data = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
                (path.to_string(), PROGRAM_MODE, data)
            }
            Content::Text(text) => (path.to_string(), FILE_MODE, text.as_bytes().to_vec()),
        };
        members.push(member);
    }
    let archive = archive(&members, modified);
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    let path = target.join(file_name);
    if std::fs::read(&path).ok().as_deref() != Some(archive.as_slice()) {
        std::fs::write(&path, &archive).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(path)
}

fn add_directories(path: &str, members: &mut Vec<(String, u32, Vec<u8>)>) {
    let mut end = 0;
    while let Some(slash) = path[end..].find('/') {
        end += slash;
        let directory = &path[..end];
        if !members.iter().any(|(name, _, _)| name == directory) {
            members.push((directory.to_owned(), DIRECTORY_MODE, Vec::new()));
        }
        end += 1;
    }
}

fn collect(
    directory: &Path,
    prefix: &str,
    members: &mut Vec<(String, u32, Vec<u8>)>,
) -> Result<(), String> {
    let mut entries = std::fs::read_dir(directory)
        .map_err(|e| format!("{}: {e}", directory.display()))?
        .map(|entry| entry.map_err(|e| format!("{}: {e}", directory.display())))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| format!("{}: name is not UTF-8", entry.path().display()))?;
        let path = format!("{prefix}{name}");
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", entry.path().display()))?;
        if kind.is_dir() {
            members.push((path.clone(), DIRECTORY_MODE, Vec::new()));
            collect(&entry.path(), &format!("{path}/"), members)?;
        } else if kind.is_file() {
            let data = std::fs::read(entry.path())
                .map_err(|e| format!("{}: {e}", entry.path().display()))?;
            members.push((path, FILE_MODE, data));
        }
    }
    Ok(())
}

/// The `newc` archive of `members` (path, mode, data), each modified at `modified`, with its
/// trailer.
pub fn archive(members: &[(String, u32, Vec<u8>)], modified: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let trailer = (TRAILER.to_owned(), 0, Vec::new());
    for (index, (name, mode, data)) in members.iter().chain([&trailer]).enumerate() {
        let (inode, modified) = match name.as_str() {
            TRAILER => (0, 0),
            _ => (index as u32 + 1, modified),
        };
        let links = if *mode == DIRECTORY_MODE { 2 } else { 1 };
        let fields = [
            inode,
            *mode,
            0,
            0,
            links,
            modified,
            data.len() as u32,
            0,
            0,
            0,
            0,
            name.len() as u32 + 1,
            0,
        ];
        bytes.extend_from_slice(b"070701");
        for field in fields {
            bytes.extend_from_slice(format!("{field:08X}").as_bytes());
        }
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        pad(&mut bytes);
        bytes.extend_from_slice(data);
        pad(&mut bytes);
    }
    bytes
}

fn pad(bytes: &mut Vec<u8>) {
    bytes.resize(bytes.len().next_multiple_of(4), 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_are_written_as_newc_headers_names_and_padded_data() {
        let members = [
            (".".to_owned(), DIRECTORY_MODE, Vec::new()),
            ("etc/hostname".to_owned(), FILE_MODE, b"eggos\n".to_vec()),
        ];
        let bytes = archive(&members, 0x1234);
        assert_eq!(bytes.len() % 4, 0);
        assert_eq!(&bytes[..6], b"070701");
        assert_eq!(&bytes[6..14], b"00000001");
        assert_eq!(&bytes[14..22], b"000041ED");
        assert_eq!(&bytes[46..54], b"00001234");
        assert_eq!(&bytes[94..102], b"00000002");
        assert_eq!(&bytes[110..112], b".\0");
        let second = 112;
        assert_eq!(&bytes[second..second + 6], b"070701");
        assert_eq!(&bytes[second + 54..second + 62], b"00000006");
        let name = second + 110;
        assert_eq!(&bytes[name..name + 13], b"etc/hostname\0");
        let data = (name + 13).next_multiple_of(4);
        assert_eq!(&bytes[data..data + 6], b"eggos\n");
        let trailer = (data + 6).next_multiple_of(4);
        assert_eq!(&bytes[trailer + 110..trailer + 121], b"TRAILER!!!\0");
        assert_eq!(bytes.len(), (trailer + 121).next_multiple_of(4));
    }

    #[test]
    fn an_extra_member_gets_the_directories_it_needs_once() {
        let mut members = vec![("etc".to_owned(), DIRECTORY_MODE, Vec::new())];
        add_directories("etc/eggos/init.conf", &mut members);
        add_directories("etc/eggos/other", &mut members);
        add_directories("init", &mut members);
        let names: Vec<&str> = members.iter().map(|(name, _, _)| name.as_str()).collect();
        assert_eq!(names, ["etc", "etc/eggos"]);
    }
}
