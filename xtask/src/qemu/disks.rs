//! The disks boot test (D192): two blank disk images of the same size boot with the release
//! kernel, which formats the first it finds for `/home` and leaves the other alone. The
//! session lists the disks, finds the unmounted one (`disk format --dry-run` refuses a mounted
//! disk), formats it with the label `scratch` and checks it clean, then powers off. On the
//! host the image the session formatted must hold an empty EggFS labelled `scratch` that checks
//! clean. xtask then writes a file into it with the `eggos-eggfs` crate, zeroes its block
//! bitmap and makes sure its own read-only check finds repairable problems. The second boot,
//! with the same disks in the same order, checks the damaged disk (problems found), repairs
//! it and checks it clean; on the host it must check clean and still hold the file.

use super::persistent::{Then, boot_on_disk, contents};
use super::*;
use eggos_eggfs::memory::MemoryDisk;
use eggos_eggfs::{BLOCK_SIZE, CheckMode, Kind, Volume, check_disk};

const DISK_BYTES: u64 = 32 << 20;

const SCRATCH_LABEL: &[u8] = b"scratch";

const NOW: u64 = 1_800_000_000;

const NOTE_PATH: &str = "/notes.txt";

const NOTE: &[u8] = b"kept through the repair\n";

const BLOCK_BITMAP: usize = 1;

const LIST_HEADER: &str = "DEVICE  SIZE  RW  FS  LABEL  MOUNTED  USED  FREE";

const BLANK_DISK_LINE: Line = Line::Around("/dev/vd", "  32 MB  rw  blank  -  -  -  -");

const FORMATTED_LINE: Line = Line::Around("Formatted /dev/vd", " as EggFS \"scratch\".");

const CLEAN_LINE: Line = Line::Around("Clean. ", " free.");

const FOUND_LINE: Line = Line::Around("Found ", " free.");

const REPAIRED_LINE: Line = Line::Around("Repaired ", " free.");

const FORMAT_SESSION: [&[u8]; 4] = [
    b"disk list\r",
    b"for d in /dev/vda /dev/vdb; do disk format --dry-run $d > /dev/null 2>&1 && s=$d; done; disk format --yes --label scratch $s\r",
    b"disk check $s; echo EGGOS-DISK clean $?\r",
    b"poweroff\r",
];

const REPAIR_SESSION: [&[u8]; 4] = [
    b"for d in /dev/vda /dev/vdb; do disk format --dry-run $d > /dev/null 2>&1 && s=$d; done; disk check $s; echo EGGOS-DISK found $?\r",
    b"disk repair $s; echo EGGOS-DISK repaired $?\r",
    b"disk check $s; echo EGGOS-DISK checked $?\r",
    b"poweroff\r",
];

fn whole(line: &str) -> Line {
    Line::Whole(line.to_owned())
}

fn format_output() -> [Vec<Line>; FORMAT_SESSION.len()] {
    [
        vec![whole(LIST_HEADER), BLANK_DISK_LINE],
        vec![FORMATTED_LINE],
        vec![CLEAN_LINE, whole("EGGOS-DISK clean 0")],
        vec![whole(POWER_DOWN_LINE)],
    ]
}

fn repair_output() -> [Vec<Line>; REPAIR_SESSION.len()] {
    [
        vec![FOUND_LINE, whole("EGGOS-DISK found 0")],
        vec![REPAIRED_LINE, whole("EGGOS-DISK repaired 0")],
        vec![CLEAN_LINE, whole("EGGOS-DISK checked 0")],
        vec![whole(POWER_DOWN_LINE)],
    ]
}

fn read_image(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn mounted(path: &Path, image: Vec<u8>) -> Result<Volume<MemoryDisk>, String> {
    Volume::mount(MemoryDisk::from_image(image))
        .map(|(volume, _)| volume)
        .map_err(|(error, _)| format!("{}: {}", path.display(), error.description()))
}

fn checked_clean(arch: &str, path: &Path, image: Vec<u8>) -> Result<(), String> {
    let mut disk = MemoryDisk::from_image(image);
    match check_disk(&mut disk, CheckMode::ReadOnly, NOW) {
        Ok(check) if check.is_clean() => Ok(()),
        Ok(check) => Err(format!(
            "{arch}: {} has {} problems: {:?}",
            path.display(),
            check.findings.len(),
            check.findings
        )),
        Err(error) => Err(format!(
            "{arch}: {} did not check: {error:?}",
            path.display()
        )),
    }
}

fn labelled_scratch(path: &Path) -> Result<bool, String> {
    let image = read_image(path)?;
    match Volume::mount(MemoryDisk::from_image(image)) {
        Ok((volume, _)) => Ok(volume.label() == SCRATCH_LABEL),
        Err(_) => Ok(false),
    }
}

fn damaged(arch: &str, path: &Path, image: Vec<u8>) -> Result<Vec<u8>, String> {
    let fail = |error: eggos_eggfs::Error| format!("{arch}: {}: {error:?}", path.display());
    let mut volume = mounted(path, image)?;
    let root = volume.root();
    let note = volume
        .create(root, &NOTE_PATH.as_bytes()[1..], Kind::Regular, 0o644, NOW)
        .map_err(fail)?;
    volume.write(note, None, NOTE, NOW).map_err(fail)?;
    volume.commit().map_err(fail)?;
    let mut image = volume.into_disk().image().to_vec();
    image[BLOCK_BITMAP * BLOCK_SIZE..(BLOCK_BITMAP + 1) * BLOCK_SIZE].fill(0);
    let mut disk = MemoryDisk::from_image(image.clone());
    match check_disk(&mut disk, CheckMode::ReadOnly, NOW) {
        Ok(check) if !check.is_clean() && check.is_repairable() => Ok(image),
        other => Err(format!(
            "{arch}: zeroing the block bitmap did not give repairable problems: {other:?}"
        )),
    }
}

/// Runs the disks boot test (see the module documentation) on `kernel`, the release kernel for
/// `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let first = directory.join(format!("eggos-{}-disks-first.img", arch.name));
    let second = directory.join(format!("eggos-{}-disks-second.img", arch.name));
    reset_disk_image(&first, DISK_BYTES)?;
    reset_disk_image(&second, DISK_BYTES)?;
    let mut args = disk_args(arch, &first.to_string_lossy());
    args.extend(scratch_disk_args(arch, &second.to_string_lossy()));

    let formatting = boot_on_disk(arch, kernel, &args, &FORMAT_SESSION, Then::Nothing)?;
    session_verdict(
        arch.name,
        formatting.clean_exit,
        &formatting.console,
        &FORMAT_SESSION,
        format_output(),
        "disks (format and check)",
    )?;
    let (scratch, home) = match labelled_scratch(&first)? {
        true => (first, second),
        false => (second, first),
    };
    let image = read_image(&scratch)?;
    let volume = mounted(&scratch, image.clone())?;
    if volume.label() != SCRATCH_LABEL {
        return Err(format!(
            "{}: neither disk is labelled scratch after `disk format`",
            arch.name
        ));
    }
    checked_clean(arch.name, &scratch, image.clone())?;
    persistent::image_volume(&home)?;

    let broken = damaged(arch.name, &scratch, image)?;
    std::fs::write(&scratch, &broken).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let repairing = boot_on_disk(arch, kernel, &args, &REPAIR_SESSION, Then::Nothing)?;
    session_verdict(
        arch.name,
        repairing.clean_exit,
        &repairing.console,
        &REPAIR_SESSION,
        repair_output(),
        "disks (repair a damaged disk)",
    )?;
    let image = read_image(&scratch)?;
    checked_clean(arch.name, &scratch, image.clone())?;
    let mut volume = mounted(&scratch, image)?;
    if contents(&mut volume, NOTE_PATH)?.as_deref() != Some(NOTE) {
        return Err(format!("{}: the repaired disk lost {NOTE_PATH}", arch.name));
    }
    eprintln!("xtask: {}: disks boot test passed", arch.name);
    Ok(())
}

/// QEMU's arguments that attach the raw disk image `image` (an absolute path) writable on
/// another of `arch`'s virtio block devices, beside the one [`disk_args`] attaches.
pub fn scratch_disk_args(arch: &Arch, image: &str) -> Vec<String> {
    vec![
        "-blockdev".into(),
        format!(
            "driver=file,node-name=scratch-file,filename={}",
            image.replace(',', ",,")
        ),
        "-blockdev".into(),
        "driver=raw,node-name=scratch,file=scratch-file".into(),
        "-device".into(),
        format!("{},drive=scratch", arch.disk_device),
    ]
}
