//! Builds the initramfs (D102): a `newc` cpio archive of the repository's `initramfs/`
//! directory, which the kernel embeds and mounts as its root, with the programs and files the
//! kernel needs: `/init`, `/bin/eggsh` and the core utilities for the release kernel (D106,
//! D107, D108), and for test kernels the programs they start from it with `execve` (D103), in
//! `usr/libexec/`, and the files `cargo xtask image --add` adds (D117). A member whose path is
//! already in the archive is refused.
//!
//! The archive is deterministic: members in name order, each directory before what it holds,
//! the extra members last, owned by root, directories and programs `0755` and other files
//! `0644`, every modification time `SOURCE_DATE_EPOCH` (0 if unset). Only directories and
//! regular files are taken. A hard link shares its file's inode number and link count, and the
//! file's data is written once, with the last of its names, as `gen_init_cpio` does (D108).

use std::path::{Path, PathBuf};

const DIRECTORY_MODE: u32 = 0o040755;
const FILE_MODE: u32 = 0o100644;
const PROGRAM_MODE: u32 = 0o100755;
const TRAILER: &str = "TRAILER!!!";
const NAME_MAX: usize = 255;
const PATH_MAX: usize = 4096;

/// What an extra member of the archive holds.
#[derive(Debug, Clone, Copy)]
pub enum Content<'a> {
    /// The program at this path on the host, made executable.
    Program(&'a str),
    /// This text, as a file of its own.
    Text(&'a str),
    /// Another name for the extra member at this path in the archive, which comes earlier.
    HardLink(&'a str),
    /// An empty directory: where the kernel mounts a file system (`/proc`, D125).
    Directory,
}

/// A member of the archive: its path, mode and data, and the member it is another name for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub mode: u32,
    pub data: Vec<u8>,
    pub link_to: Option<usize>,
}

impl Member {
    fn new(name: &str, mode: u32, data: Vec<u8>) -> Self {
        Self {
            name: name.to_owned(),
            mode,
            data,
            link_to: None,
        }
    }
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
    let mut members = vec![Member::new(".", DIRECTORY_MODE, Vec::new())];
    collect(&source, "", &mut members)?;
    for (path, content) in extra {
        add(&mut members, path, *content)?;
    }
    let archive = archive(&members, modified);
    let target = crate::target_directory();
    std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    let path = target.join(file_name);
    if std::fs::read(&path).ok().as_deref() != Some(archive.as_slice()) {
        std::fs::write(&path, &archive).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(path)
}

fn add(members: &mut Vec<Member>, path: &str, content: Content) -> Result<(), String> {
    if members.iter().any(|member| member.name == path) {
        return Err(format!("`/{path}` is already in the image"));
    }
    if path.len() >= PATH_MAX || path.split('/').any(|name| name.len() > NAME_MAX) {
        return Err(format!(
            "`/{path}` is longer than the kernel takes: names up to {NAME_MAX} bytes, paths \
             under {PATH_MAX}"
        ));
    }
    let mut ancestors = path.match_indices('/').map(|(end, _)| &path[..end]);
    if let Some(file) = ancestors.find(|ancestor| {
        members
            .iter()
            .any(|member| member.name == *ancestor && member.mode != DIRECTORY_MODE)
    }) {
        return Err(format!(
            "`/{file}` is a file, so `/{path}` cannot go under it"
        ));
    }
    add_directories(path, members);
    let member = match content {
        Content::Program(file) => {
            let data = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            Member::new(path, PROGRAM_MODE, data)
        }
        Content::Text(text) => Member::new(path, FILE_MODE, text.as_bytes().to_vec()),
        Content::Directory => Member::new(path, DIRECTORY_MODE, Vec::new()),
        Content::HardLink(target) => {
            let index = members
                .iter()
                .position(|member| member.name == target && member.mode != DIRECTORY_MODE)
                .ok_or_else(|| format!("{path}: no file `{target}` to link to before it"))?;
            let target = members[index].link_to.unwrap_or(index);
            Member {
                link_to: Some(target),
                ..Member::new(path, members[target].mode, Vec::new())
            }
        }
    };
    members.push(member);
    Ok(())
}

fn add_directories(path: &str, members: &mut Vec<Member>) {
    let mut end = 0;
    while let Some(slash) = path[end..].find('/') {
        end += slash;
        let directory = &path[..end];
        if !members.iter().any(|member| member.name == directory) {
            members.push(Member::new(directory, DIRECTORY_MODE, Vec::new()));
        }
        end += 1;
    }
}

fn collect(directory: &Path, prefix: &str, members: &mut Vec<Member>) -> Result<(), String> {
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
            members.push(Member::new(&path, DIRECTORY_MODE, Vec::new()));
            collect(&entry.path(), &format!("{path}/"), members)?;
        } else if kind.is_file() {
            let data = std::fs::read(entry.path())
                .map_err(|e| format!("{}: {e}", entry.path().display()))?;
            members.push(Member::new(&path, FILE_MODE, data));
        }
    }
    Ok(())
}

/// The `newc` archive of `members`, each modified at `modified`, with its trailer: a hard link
/// gets its file's inode number and link count, and the file's data goes with the last of its
/// names, the others empty.
pub fn archive(members: &[Member], modified: u32) -> Vec<u8> {
    let file_of = |index: usize| members[index].link_to.unwrap_or(index);
    let names = |file: usize| (0..members.len()).filter(move |&index| file_of(index) == file);
    let mut bytes = Vec::new();
    for (index, member) in members.iter().enumerate() {
        let file = file_of(index);
        let links = match member.mode {
            DIRECTORY_MODE => 2,
            _ => names(file).count() as u32,
        };
        let data = match names(file).next_back() {
            Some(last) if last == index => members[file].data.as_slice(),
            _ => &[],
        };
        write_header(
            &mut bytes,
            &member.name,
            file as u32 + 1,
            member.mode,
            links,
            modified,
            data,
        );
    }
    write_header(&mut bytes, TRAILER, 0, 0, 1, 0, &[]);
    bytes
}

fn write_header(
    bytes: &mut Vec<u8>,
    name: &str,
    inode: u32,
    mode: u32,
    links: u32,
    modified: u32,
    data: &[u8],
) {
    let fields = [
        inode,
        mode,
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
    pad(bytes);
    bytes.extend_from_slice(data);
    pad(bytes);
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
            Member::new(".", DIRECTORY_MODE, Vec::new()),
            Member::new("etc/hostname", FILE_MODE, b"eggos\n".to_vec()),
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
        let mut members = vec![Member::new("etc", DIRECTORY_MODE, Vec::new())];
        add_directories("etc/eggos/init.conf", &mut members);
        add_directories("etc/eggos/other", &mut members);
        add_directories("init", &mut members);
        let names: Vec<&str> = members.iter().map(|member| member.name.as_str()).collect();
        assert_eq!(names, ["etc", "etc/eggos"]);
    }

    #[test]
    fn a_member_already_in_the_archive_is_refused_with_its_path() {
        let mut members = vec![Member::new("etc", DIRECTORY_MODE, Vec::new())];
        add(&mut members, "etc/motd", Content::Text("hi")).unwrap();
        for taken in ["etc", "etc/motd"] {
            assert_eq!(
                add(&mut members, taken, Content::Text("again")),
                Err(format!("`/{taken}` is already in the image"))
            );
        }
        assert_eq!(members.len(), 2);
        assert_eq!(
            add(&mut members, "etc/motd/x", Content::Text("under a file")),
            Err("`/etc/motd` is a file, so `/etc/motd/x` cannot go under it".into())
        );
        add(&mut members, "bin/tool", Content::Text("tool")).unwrap();
        assert!(add(&mut members, "bin/tool/deeper/x", Content::Text("x")).is_err());
        let long_name = format!("etc/{}", "n".repeat(NAME_MAX + 1));
        let long_path = format!("etc/{}", "d/".repeat(PATH_MAX / 2));
        for long in [long_name, long_path] {
            assert!(add(&mut members, &long, Content::Text("long")).is_err());
        }
        add(
            &mut members,
            &format!("etc/{}", "n".repeat(NAME_MAX)),
            Content::Text("ok"),
        )
        .unwrap();
        assert_eq!(members.len(), 5);
    }

    fn headers(bytes: &[u8]) -> Vec<(String, u32, u32, Vec<u8>)> {
        let field = |header: &[u8], index: usize| {
            let digits = std::str::from_utf8(&header[6 + 8 * index..14 + 8 * index]).unwrap();
            u32::from_str_radix(digits, 16).unwrap()
        };
        let mut members = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let header = &bytes[offset..offset + 110];
            let name_size = field(header, 11) as usize;
            let data_size = field(header, 6) as usize;
            let name = &bytes[offset + 110..offset + 110 + name_size - 1];
            let data_start = (offset + 110 + name_size).next_multiple_of(4);
            let data = bytes[data_start..data_start + data_size].to_vec();
            members.push((
                String::from_utf8(name.to_vec()).unwrap(),
                field(header, 0),
                field(header, 4),
                data,
            ));
            offset = (data_start + data_size).next_multiple_of(4);
        }
        members
    }

    #[test]
    fn hard_links_share_an_inode_and_link_count_and_the_last_name_carries_the_data() {
        let members = [
            Member::new("bin", DIRECTORY_MODE, Vec::new()),
            Member::new("bin/coreutils", PROGRAM_MODE, b"program".to_vec()),
            Member::new("bin/other", PROGRAM_MODE, b"other".to_vec()),
            Member {
                link_to: Some(1),
                ..Member::new("bin/ls", PROGRAM_MODE, Vec::new())
            },
            Member {
                link_to: Some(1),
                ..Member::new("bin/cat", PROGRAM_MODE, Vec::new())
            },
        ];
        let headers = headers(&archive(&members, 0));
        let expected = [
            ("bin", 1, 2, &b""[..]),
            ("bin/coreutils", 2, 3, b""),
            ("bin/other", 3, 1, b"other"),
            ("bin/ls", 2, 3, b""),
            ("bin/cat", 2, 3, b"program"),
            (TRAILER, 0, 1, b""),
        ];
        let expected: Vec<(String, u32, u32, Vec<u8>)> = expected
            .iter()
            .map(|(name, inode, links, data)| (name.to_string(), *inode, *links, data.to_vec()))
            .collect();
        assert_eq!(headers, expected);
    }
}
