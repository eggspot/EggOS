//! The file system through Rust's standard library (D102): reading files, listing
//! directories, metadata, seeking and the working directory, against the initramfs the
//! kernel mounts as its root, and the errors a read-only file system gives.

use std::fs::{self, File};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;

const HOSTNAME: &str = "eggos\n";
const MOTD: &str = "Welcome to EggOS. Type help. Apps: the egg, or Super.\n";
const ENOTDIR: i32 = 20;
const EROFS: i32 = 30;

/// Whether every check passed, naming on standard output each group that did not. Leaves the
/// working directory at the root.
pub fn files_behave_as_on_linux() -> bool {
    let checks: [crate::Check; 5] = [
        ("files are read whole", files_are_read_whole),
        (
            "directories list their entries",
            directories_list_their_entries,
        ),
        (
            "metadata reports types and sizes",
            metadata_reports_types_and_sizes,
        ),
        ("an open file seeks", an_open_file_seeks),
        (
            "the working directory changes and relative paths follow it",
            the_working_directory_changes,
        ),
    ];
    let mut passed = true;
    for (name, check) in checks {
        if !check() {
            println!("EGGOS-USER: files: {name}: FAILED");
            passed = false;
        }
    }
    let _ = std::env::set_current_dir("/");
    passed
}

fn raw_error(result: std::io::Result<impl Sized>) -> Option<i32> {
    result.err().and_then(|error| error.raw_os_error())
}

fn files_are_read_whole() -> bool {
    fs::read_to_string("/etc/hostname").is_ok_and(|text| text == HOSTNAME)
        && fs::read_to_string("/etc/motd").is_ok_and(|text| text == MOTD)
        && fs::read("/usr/share/eggos/about").is_ok_and(|bytes| bytes == b"EggOS\n")
        && fs::read("//etc/../etc/./hostname").is_ok_and(|bytes| bytes == HOSTNAME.as_bytes())
        && fs::read("/etc/missing").is_err_and(|error| error.kind() == ErrorKind::NotFound)
        && raw_error(fs::read("/etc/hostname/")) == Some(ENOTDIR)
        && raw_error(File::create("/etc/hostname")) == Some(EROFS)
        && raw_error(File::create("/etc/new")) == Some(EROFS)
        && fs::read("/etc").is_err()
}

fn directories_list_their_entries() -> bool {
    let names = |path: &str| -> Option<Vec<String>> {
        let mut names: Vec<String> = fs::read_dir(path)
            .ok()?
            .map(|entry| {
                entry
                    .ok()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
            })
            .collect::<Option<_>>()?;
        names.sort();
        Some(names)
    };
    let all_of_type = |path: &str, directories: bool| {
        fs::read_dir(path).is_ok_and(|entries| {
            entries.flatten().all(|entry| {
                entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_dir() == directories)
            })
        })
    };
    let types_known = all_of_type("/", true) && all_of_type("/etc", false);
    names("/") == Some(vec!["etc".into(), "usr".into()])
        && names("/etc") == Some(vec!["hostname".into(), "motd".into()])
        && names("/usr/share") == Some(vec!["eggos".into()])
        && types_known
        && raw_error(fs::read_dir("/etc/hostname")) == Some(ENOTDIR)
}

fn metadata_reports_types_and_sizes() -> bool {
    let file = fs::metadata("/etc/motd");
    let directory = fs::metadata("/usr/share/eggos");
    let opened = File::open("/etc/hostname").and_then(|file| file.metadata());
    file.is_ok_and(|metadata| metadata.is_file() && metadata.len() == MOTD.len() as u64)
        && directory.is_ok_and(|metadata| metadata.is_dir())
        && opened.is_ok_and(|metadata| metadata.len() == HOSTNAME.len() as u64)
        && fs::metadata("/nowhere").is_err_and(|error| error.kind() == ErrorKind::NotFound)
        && fs::symlink_metadata("/etc").is_ok_and(|metadata| metadata.is_dir())
}

fn an_open_file_seeks() -> bool {
    let Ok(mut file) = File::open("/etc/motd") else {
        return false;
    };
    let mut read_at = |from: SeekFrom, length: usize| -> Option<(u64, String)> {
        let at = file.seek(from).ok()?;
        let mut bytes = vec![0; length];
        let read = file.read(&mut bytes).ok()?;
        bytes.truncate(read);
        Some((at, String::from_utf8(bytes).ok()?))
    };
    read_at(SeekFrom::Start(11), 5) == Some((11, "EggOS".into()))
        && read_at(SeekFrom::Current(-5), 3) == Some((11, "Egg".into()))
        && read_at(SeekFrom::End(-2), 10) == Some((52, ".\n".into()))
        && read_at(SeekFrom::End(5), 10) == Some((59, String::new()))
        && file.seek(SeekFrom::Current(-100)).is_err()
}

fn the_working_directory_changes() -> bool {
    let at = |path: &str| std::env::current_dir().is_ok_and(|current| current == Path::new(path));
    let starts_at_root = at("/");
    let into_etc = std::env::set_current_dir("/etc").is_ok() && at("/etc");
    let relative = fs::read_to_string("hostname").is_ok_and(|text| text == HOSTNAME)
        && fs::metadata("./motd").is_ok_and(|metadata| metadata.is_file());
    let nested = std::env::set_current_dir("../usr/share/eggos").is_ok()
        && at("/usr/share/eggos")
        && fs::read("about").is_ok();
    let refused = raw_error(std::env::set_current_dir("/etc/hostname")) == Some(ENOTDIR)
        && std::env::set_current_dir("/missing").is_err()
        && at("/usr/share/eggos");
    let back = std::env::set_current_dir("../../..").is_ok() && at("/");
    starts_at_root && into_etc && relative && nested && refused && back
}
