//! The work disk boot test (D166, CI stage 1): a host folder packed into a work disk with
//! `cargo xtask work pack`'s code is attached before a blank disk for `/home`, and the release
//! kernel must mount the work disk at `/work` and the blank one at `/home` whatever their
//! order. The session reads the folder back, writes results into `/work`, removes a file and
//! writes to `/home`, syncs and powers off. Then the work disk is unpacked on the host: the
//! new folder must hold the folder's files, the results and not the removed file, and the
//! home disk must hold what was written to `/home`.

use super::persistent::{Then, boot_on_disk, contents, image_volume};
use super::*;
use crate::work_disk::{pack, unpack};

const TEST_HOME_DISK_BYTES: u64 = 64 << 20;

const HELLO: &[u8] = b"hello from the host\n";

const MAIN: &[u8] = b"fn main() {}\n";

const WORK_MOUNTED_LINE: Line = Line::Around("vfs: work disk mounted at /work (", " MiB free)");

const HOME_MOUNTED_LINE: Line = Line::Around(
    "vfs: EggFS disk mounted at /home, /Applications and /var (",
    " MiB free)",
);

const SESSION: [&[u8]; 3] = [
    b"cat /work/hello.txt; ls /work\r",
    b"mkdir /work/out; echo built > /work/out/log.txt; cp /work/hello.txt /work/out/copy.txt; rm /work/src/main.rs; echo kept > /home/kept.txt; sync; echo EGGOS-WORK synced $?\r",
    b"poweroff\r",
];

fn whole(lines: &[&str]) -> Vec<Line> {
    lines
        .iter()
        .map(|line| Line::Whole((*line).to_owned()))
        .collect()
}

fn outputs() -> [Vec<Line>; SESSION.len()] {
    [
        whole(&["hello from the host", "hello.txt", "src"]),
        whole(&["EGGOS-WORK synced 0"]),
        whole(&[POWER_DOWN_LINE]),
    ]
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_back(arch: &str, path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{arch}: {}: {error}", path.display())),
    }
}

fn expect_file(arch: &str, path: &Path, expected: Option<&[u8]>) -> Result<(), String> {
    let found = read_back(arch, path)?;
    if found.as_deref() != expected {
        return Err(format!(
            "{arch}: the unpacked {} is {:?}, not {:?}",
            path.display(),
            found.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            expected.map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        ));
    }
    Ok(())
}

/// Runs the work disk boot test (see the module documentation) on `kernel`, the release kernel
/// for `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    let folder = directory.join(format!("work-{}", arch.name));
    let out = directory.join(format!("work-{}.out", arch.name));
    for stale in [&folder, &out] {
        if stale.exists() {
            std::fs::remove_dir_all(stale).map_err(|e| format!("{}: {e}", stale.display()))?;
        }
    }
    write_file(&folder.join("hello.txt"), HELLO)?;
    write_file(&folder.join("src").join("main.rs"), MAIN)?;
    let image = directory.join(format!("eggos-{}-work.img", arch.name));
    pack(&folder, &image)?;
    let home = directory.join(format!("eggos-{}-work-home.img", arch.name));
    reset_disk_image(&home, TEST_HOME_DISK_BYTES)?;
    let mut args = work_disk_args(arch, &image.to_string_lossy());
    args.extend(disk_args(arch, &home.to_string_lossy()));

    let ended = boot_on_disk(arch, kernel, &args, &SESSION, Then::Nothing)?;
    let lines: Vec<&str> = ended.console.lines().map(str::trim_end).collect();
    if !lines.iter().any(|line| WORK_MOUNTED_LINE.matches(line)) {
        return Err(format!(
            "{}: the work disk was not mounted at /work",
            arch.name
        ));
    }
    if !lines.iter().any(|line| HOME_MOUNTED_LINE.matches(line)) {
        return Err(format!(
            "{}: the blank disk was not mounted at /home beside the work disk",
            arch.name
        ));
    }
    session_verdict(
        arch.name,
        ended.clean_exit,
        &ended.console,
        &SESSION,
        outputs(),
        "work disk",
    )?;

    let unpacked = unpack(&image, &out)?;
    eprintln!(
        "xtask: {}: unpacked {} files and {} folders from the work disk",
        arch.name, unpacked.files, unpacked.folders
    );
    expect_file(arch.name, &out.join("hello.txt"), Some(HELLO))?;
    expect_file(
        arch.name,
        &out.join("out").join("log.txt"),
        Some(b"built\n".as_slice()),
    )?;
    expect_file(arch.name, &out.join("out").join("copy.txt"), Some(HELLO))?;
    expect_file(arch.name, &out.join("src").join("main.rs"), None)?;
    if !out.join("src").is_dir() {
        return Err(format!("{}: the unpacked src folder is missing", arch.name));
    }
    let mut volume = image_volume(&home)?;
    if contents(&mut volume, "/home/kept.txt")?.as_deref() != Some(b"kept\n".as_slice()) {
        return Err(format!(
            "{}: the home disk does not hold /home/kept.txt",
            arch.name
        ));
    }
    if contents(&mut volume, "/work/hello.txt")?.is_some() {
        return Err(format!(
            "{}: the work disk's files were written to the home disk",
            arch.name
        ));
    }
    eprintln!("xtask: {}: work disk boot test passed", arch.name);
    Ok(())
}
