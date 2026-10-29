//! The persistent files boot test (D131): four boots of the release kernel on disk images in
//! `target/qemu-disks`, each checked on the console and, after QEMU ends, on the image itself
//! with the `eggos-eggfs` crate (journal replayed in memory, then the full consistency check):
//!
//! 1. A disk holding garbage is left as it is, said so, and `/home` stays in memory.
//! 2. A blank disk is formatted with EggFS and mounted with the host folder attached; files and
//!    directories are made, appended to, copied from the host folder (larger than the journal
//!    and than the virtio bounce buffer), renamed and removed, synced, and the machine powered
//!    off. Before that, a symbolic link (`ln -s`) and a second name (`ln`) are given to a file
//!    and a `#!` script in the host folder runs (D170). The image must then hold exactly that
//!    tree, the link with its target text and the file with two links.
//! 3. The same disk boots again without formatting and reads everything back, through the link
//!    and the second name too (`readlink`, `cat`, `ls -l` counting 2 links); then a loop
//!    replaces a large file through a temporary name and renames it over the old one, and QEMU
//!    is killed in the middle. The image must check clean, the large file must be one of the
//!    two versions whole, and the temporary file absent or a prefix of one.
//! 4. The same disk boots once more, with the display, recovering through the journal if the
//!    kill left a commit half in place, and reads the files back; then a file is written and
//!    the machine powered off at once through the desktop's power menu (Super+X, Enter), before
//!    the committer would commit: the image must hold the file, which only the sync on the way
//!    down can have written.

use super::*;
use eggos_eggfs::memory::MemoryDisk;
use eggos_eggfs::{Kind, Volume};
use std::path::PathBuf;

const TEST_DISK_BYTES: u64 = 64 << 20;

const LARGE_FILE_BYTES: usize = 3 << 20;

const GARBAGE_BYTES: usize = 1 << 20;

const PERSISTENT_TIMEOUT: Duration = Duration::from_secs(180);

const KILL_DELAY: Duration = Duration::from_millis(500);

const FORMATTED_LINE: &str = "vfs: formatted the blank disk with EggFS";

const MOUNTED_LINE: Line = Line::Around(
    "vfs: EggFS disk mounted at /home, /Applications and /var (",
    " MiB free)",
);

const GARBAGE_LINE: &str = "vfs: a disk holds neither EggFS nor FAT (no FAT file system on the disk or its first partition): it was not formatted; to erase it, run with --format-disk";

const GARBAGE_SESSION: [&[u8]; 2] = [
    b"ls /home; echo EGGOS-PERSIST garbage kept\r",
    b"poweroff\r",
];

const SCRIPT_NAME: &str = "script.sh";

const SCRIPT: &[u8] = b"#!/bin/eggsh\necho script-ran $1\n";

const LINK: &str = "/home/docs/a-link";

const LINK_TARGET: &str = "a.txt";

const SECOND_NAME: &str = "/home/docs/a-second";

const FIRST_SESSION: [&[u8]; 5] = [
    b"mkdir /home/docs /home/docs/deep; echo hello > /home/docs/a.txt; echo more >> /home/docs/a.txt; cat /home/docs/a.txt\r",
    b"cp /media/host/large.bin /home/docs/large.bin; echo x > /home/old.txt; mv /home/old.txt /home/docs/deep/new.txt; echo gone > /home/gone.txt; rm /home/gone.txt; mkdir /Applications/Tool; echo tool > /Applications/Tool/run.txt; sync; echo EGGOS-PERSIST synced $?\r",
    b"ls /home /home/docs\r",
    b"ln -s a.txt /home/docs/a-link; ln /home/docs/a.txt /home/docs/a-second; readlink /home/docs/a-link; /media/host/script.sh arg; sync; echo EGGOS-PERSIST linked $?\r",
    b"poweroff\r",
];

macro_rules! replacing_round {
    () => {
        "cp /media/host/second.bin t.tmp; mv t.tmp large.bin; \
         cp /media/host/large.bin t.tmp; mv t.tmp large.bin; sync; "
    };
}

const COPY_LOOP: &str = concat!(
    "cd /home/docs; echo EGGOS-PERSIST copying; ",
    replacing_round!(),
    replacing_round!(),
    replacing_round!(),
    replacing_round!(),
    replacing_round!(),
    replacing_round!(),
    replacing_round!(),
    replacing_round!(),
    replacing_round!(),
    replacing_round!(),
    "echo EGGOS-PERSIST copied\r"
);

const SECOND_SESSION: [&[u8]; 3] = [
    b"cat /home/docs/a.txt /home/docs/deep/new.txt /Applications/Tool/run.txt; wc -c /home/docs/large.bin\r",
    b"readlink /home/docs/a-link; cat /home/docs/a-link /home/docs/a-second; ls -l /home/docs/a-second\r",
    COPY_LOOP.as_bytes(),
];

const _: () = assert!(COPY_LOOP.len() < 2000);

const COPYING_LINE: &str = "EGGOS-PERSIST copying";

const COPIED_LINE: &str = "EGGOS-PERSIST copied";

const RECOVERY_SESSION: [&[u8]; 3] = [
    b"cat /home/docs/a.txt /home/docs/deep/new.txt; ls /home/docs/deep\r",
    b"wc -c /home/docs/large.bin; sync; echo EGGOS-PERSIST recovered $?\r",
    b"echo gui > /home/gui.txt; echo EGGOS-PERSIST written\r",
];

const WRITTEN_LINE: &str = "EGGOS-PERSIST written";

#[derive(Debug, Clone, Copy)]
pub(super) enum Then {
    Nothing,
    Kill(&'static str),
    PowerMenu(&'static str),
}

fn whole(lines: &[&str]) -> Vec<Line> {
    lines
        .iter()
        .map(|line| Line::Whole((*line).to_owned()))
        .collect()
}

fn first_output() -> [Vec<Line>; FIRST_SESSION.len()] {
    [
        whole(&["hello", "more"]),
        whole(&["EGGOS-PERSIST synced 0"]),
        whole(&[
            "/home:",
            "docs",
            "",
            "/home/docs:",
            "a.txt",
            "deep",
            "large.bin",
        ]),
        whole(&[LINK_TARGET, "script-ran arg", "EGGOS-PERSIST linked 0"]),
        whole(&[POWER_DOWN_LINE]),
    ]
}

fn recovery_output() -> [Vec<Line>; RECOVERY_SESSION.len()] {
    [
        whole(&["hello", "more", "x", "new.txt"]),
        vec![
            Line::Whole(format!("{LARGE_FILE_BYTES} /home/docs/large.bin")),
            Line::Whole("EGGOS-PERSIST recovered 0".into()),
        ],
        whole(&[WRITTEN_LINE]),
    ]
}

fn power_off_through_the_menu(port: u16) -> Result<(), String> {
    let key = |qcode, down| InputEvent::Key { qcode, down };
    let mut qmp = Qmp::connect(port)?;
    for events in [
        [key("meta_l", true), key("x", true)],
        [key("x", false), key("meta_l", false)],
        [key("ret", true), key("ret", false)],
    ] {
        qmp.send_input(&events)?;
        std::thread::sleep(DEVICE_PAUSE);
    }
    Ok(())
}

fn pattern(seed: u64, length: usize) -> Vec<u8> {
    let mut state = seed | 1;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

struct Files {
    disk: PathBuf,
    garbage: PathBuf,
    apps: String,
    large: Vec<u8>,
    second: Vec<u8>,
}

fn prepare(arch: &Arch) -> Result<Files, String> {
    let directory = crate::target_directory().join("qemu-disks");
    let apps = directory.join(format!("apps-{}", arch.name));
    std::fs::create_dir_all(&apps).map_err(|e| format!("{}: {e}", apps.display()))?;
    let large = pattern(0x5eed_0001, LARGE_FILE_BYTES);
    let second = pattern(0x5eed_0002, LARGE_FILE_BYTES);
    for (name, bytes) in [
        ("large.bin", large.as_slice()),
        ("second.bin", second.as_slice()),
        (SCRIPT_NAME, SCRIPT),
    ] {
        let path = apps.join(name);
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let apps = crate::host_folder(&apps.to_string_lossy())?;
    Ok(Files {
        disk: directory.join(format!("eggos-{}-test-disk.img", arch.name)),
        garbage: directory.join(format!("eggos-{}-garbage-disk.img", arch.name)),
        apps,
        large,
        second,
    })
}

pub(super) struct Ended {
    pub(super) console: String,
    pub(super) clean_exit: bool,
    killed: bool,
}

pub(super) fn boot_on_disk(
    arch: &Arch,
    kernel: &Path,
    args: &[String],
    session: &[&'static [u8]],
    then: Then,
) -> Result<Ended, String> {
    eprintln!(
        "xtask: booting {} in {} (persistent files)",
        arch.name, arch.qemu
    );
    let cpus = TEST_CPUS.to_string();
    let port = match then {
        Then::PowerMenu(_) => Some(qmp::free_port()?),
        Then::Nothing | Then::Kill(_) => None,
    };
    let (display, input): (&[&str], &[&str]) = match port {
        Some(_) => (arch.display_args, arch.input_args),
        None => (&[], &[]),
    };
    let mut child = Command::new(arch.qemu)
        .args(arch.machine_args)
        .args(display)
        .args(input)
        .args(port.map(qmp::qemu_args).unwrap_or_default())
        .args(COMMON_ARGS)
        .args(["-smp", cpus.as_str()])
        .args(args)
        .args(["-display", "none", "-serial", "stdio", "-monitor", "none"])
        .arg("-kernel")
        .arg(kernel)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;
    let rx = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
    let mut keyboard = child.stdin.take().expect("stdin is piped");
    let started = Instant::now();
    let mut console = Vec::new();
    let mut typed = ShellTyping::default();
    let mut kill_at = None;
    let mut menu_used = false;
    let (status, killed) = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break (status, false);
        }
        console.extend(rx.try_iter().flatten());
        let text = String::from_utf8_lossy(&console).into_owned();
        for script in shell_scripts_to_type(&text, session, &mut typed) {
            if let Err(e) = keyboard.write_all(script).and_then(|()| keyboard.flush()) {
                eprintln!("xtask: {}: could not type on the console: {e}", arch.name);
            }
        }
        match then {
            Then::Kill(line) if kill_at.is_none() && has_line(&text, line) => {
                kill_at = Some(Instant::now() + KILL_DELAY);
            }
            Then::PowerMenu(line) if !menu_used && has_line(&text, line) => {
                menu_used = true;
                if let Some(port) = port
                    && let Err(error) = power_off_through_the_menu(port)
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{}: the power menu: {error}", arch.name));
                }
            }
            _ => {}
        }
        if kill_at.is_some_and(|at| Instant::now() >= at) {
            let _ = child.kill();
            break (child.wait().map_err(|e| e.to_string())?, true);
        }
        if started.elapsed() > PERSISTENT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: no power-off within {}s (kernel hung?)",
                arch.name,
                PERSISTENT_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    console.extend(rx.iter().flatten());
    Ok(Ended {
        console: String::from_utf8_lossy(&console).into_owned(),
        clean_exit: status.success(),
        killed,
    })
}

pub(super) fn image_volume(path: &Path) -> Result<Volume<MemoryDisk>, String> {
    let image = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (mut volume, _) = Volume::mount(MemoryDisk::from_image(image))
        .map_err(|(error, _)| format!("{}: {}", path.display(), error.description()))?;
    volume.check().map_err(|problem| {
        format!(
            "{}: the file system is inconsistent: {problem}",
            path.display()
        )
    })?;
    Ok(volume)
}

fn node_at(volume: &mut Volume<MemoryDisk>, path: &str) -> Option<u64> {
    let mut node = volume.root();
    for name in path.split('/').filter(|name| !name.is_empty()) {
        node = volume.lookup(node, name.as_bytes()).ok()??;
    }
    Some(node)
}

pub(super) fn contents(
    volume: &mut Volume<MemoryDisk>,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let Some(node) = node_at(volume, path) else {
        return Ok(None);
    };
    if eggos_eggfs::kind_of(node) != Kind::Regular {
        return Err(format!("{path} is not a regular file"));
    }
    let mut out = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = volume
            .read(node, out.len() as u64, &mut buffer)
            .map_err(|e| format!("{path}: {e:?}"))?;
        if read == 0 {
            return Ok(Some(out));
        }
        out.extend_from_slice(&buffer[..read]);
    }
}

fn expect_contents(
    arch: &str,
    volume: &mut Volume<MemoryDisk>,
    path: &str,
    expected: Option<&[u8]>,
) -> Result<(), String> {
    let found = contents(volume, path)?;
    if found.as_deref() != expected {
        return Err(format!(
            "{arch}: the disk image's {path} is {} bytes, not the {} expected",
            found.map_or(-1, |found| found.len() as i64),
            expected.map_or(-1, |expected| expected.len() as i64)
        ));
    }
    Ok(())
}

fn expect_links(arch: &str, volume: &mut Volume<MemoryDisk>) -> Result<(), String> {
    let Some(link) = node_at(volume, LINK) else {
        return Err(format!("{arch}: the disk image has no {LINK}"));
    };
    if eggos_eggfs::kind_of(link) != Kind::SymbolicLink {
        return Err(format!(
            "{arch}: the disk image's {LINK} is not a symbolic link"
        ));
    }
    let target = volume
        .link_target(link)
        .map_err(|e| format!("{arch}: {LINK}: {e:?}"))?;
    if target != LINK_TARGET.as_bytes() {
        return Err(format!(
            "{arch}: the disk image's {LINK} points to `{}`, not `{LINK_TARGET}`",
            String::from_utf8_lossy(&target)
        ));
    }
    let first = node_at(volume, "/home/docs/a.txt");
    let second = node_at(volume, SECOND_NAME);
    let (Some(file), true) = (first, first == second) else {
        return Err(format!(
            "{arch}: the disk image's {SECOND_NAME} is not a second name of /home/docs/a.txt"
        ));
    };
    let links = volume
        .attributes(file)
        .map_err(|e| format!("{arch}: /home/docs/a.txt: {e:?}"))?
        .links;
    if links != 2 {
        return Err(format!(
            "{arch}: the disk image's /home/docs/a.txt has {links} links, not 2"
        ));
    }
    Ok(())
}

fn second_name_listed(line: &str) -> bool {
    line.ends_with(&format!(" {SECOND_NAME}")) && line.split_whitespace().nth(1) == Some("2")
}

fn check_garbage_disk(arch: &Arch, kernel: &Path, files: &Files) -> Result<(), String> {
    let garbage = pattern(0x6a5b_a6e0, GARBAGE_BYTES);
    reset_disk_image(&files.garbage, 8 << 20)?;
    let mut image = std::fs::read(&files.garbage).map_err(|e| e.to_string())?;
    image[..GARBAGE_BYTES].copy_from_slice(&garbage);
    std::fs::write(&files.garbage, &image).map_err(|e| e.to_string())?;
    let args = disk_args(arch, &files.garbage.to_string_lossy());
    let ended = boot_on_disk(arch, kernel, &args, &GARBAGE_SESSION, Then::Nothing)?;
    if !has_line(&ended.console, GARBAGE_LINE) {
        return Err(format!("{}: no `{GARBAGE_LINE}`", arch.name));
    }
    session_verdict(
        arch.name,
        ended.clean_exit,
        &ended.console,
        &GARBAGE_SESSION,
        [
            whole(&["EGGOS-PERSIST garbage kept"]),
            whole(&[POWER_DOWN_LINE]),
        ],
        "garbage disk",
    )?;
    let after = std::fs::read(&files.garbage).map_err(|e| e.to_string())?;
    if after != image {
        return Err(format!(
            "{}: the disk holding garbage was changed",
            arch.name
        ));
    }
    Ok(())
}

/// Runs the persistent files boot test (see the module documentation) on `kernel`, the
/// release kernel for `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let files = prepare(arch)?;
    check_garbage_disk(arch, kernel, &files)?;
    reset_disk_image(&files.disk, TEST_DISK_BYTES)?;
    let disk = disk_args(arch, &files.disk.to_string_lossy());
    let apps = host_folder_args(arch, &files.apps);
    let both: Vec<String> = disk.iter().chain(&apps).cloned().collect();

    let first = boot_on_disk(arch, kernel, &both, &FIRST_SESSION, Then::Nothing)?;
    if !has_line(&first.console, FORMATTED_LINE) {
        return Err(format!("{}: the blank disk was not formatted", arch.name));
    }
    let lines: Vec<&str> = first.console.lines().map(str::trim_end).collect();
    if !lines.iter().any(|line| MOUNTED_LINE.matches(line)) {
        return Err(format!("{}: the EggFS disk was not mounted", arch.name));
    }
    if !has_line(&first.console, HOST_FOLDER_MOUNT_LINE) {
        return Err(format!(
            "{}: the host folder was not mounted next to the disk",
            arch.name
        ));
    }
    session_verdict(
        arch.name,
        first.clean_exit,
        &first.console,
        &FIRST_SESSION,
        first_output(),
        "persistent files (first boot)",
    )?;
    let mut volume = image_volume(&files.disk)?;
    expect_contents(
        arch.name,
        &mut volume,
        "/home/docs/a.txt",
        Some(b"hello\nmore\n"),
    )?;
    expect_contents(
        arch.name,
        &mut volume,
        "/home/docs/large.bin",
        Some(&files.large),
    )?;
    expect_contents(
        arch.name,
        &mut volume,
        "/home/docs/deep/new.txt",
        Some(b"x\n"),
    )?;
    expect_contents(
        arch.name,
        &mut volume,
        "/Applications/Tool/run.txt",
        Some(b"tool\n"),
    )?;
    expect_contents(arch.name, &mut volume, "/home/gone.txt", None)?;
    expect_contents(arch.name, &mut volume, "/home/old.txt", None)?;
    expect_contents(arch.name, &mut volume, SECOND_NAME, Some(b"hello\nmore\n"))?;
    expect_links(arch.name, &mut volume)?;

    let second = boot_on_disk(
        arch,
        kernel,
        &both,
        &SECOND_SESSION,
        Then::Kill(COPYING_LINE),
    )?;
    if has_line(&second.console, FORMATTED_LINE) {
        return Err(format!("{}: the EggFS disk was formatted again", arch.name));
    }
    if !second.killed {
        return Err(format!("{}: QEMU ended before it was killed", arch.name));
    }
    let lines: Vec<&str> = second.console.lines().map(str::trim_end).collect();
    let read_back = ["hello", "more", "x", "tool"];
    let read_back_line = format!("{LARGE_FILE_BYTES} /home/docs/large.bin");
    let through_links = [LINK_TARGET, "hello", "more", "hello", "more"];
    let mut from = 0;
    for wanted in read_back
        .iter()
        .copied()
        .chain([read_back_line.as_str()])
        .chain(through_links)
    {
        match lines[from..].iter().position(|line| *line == wanted) {
            Some(at) => from += at + 1,
            None => {
                return Err(format!(
                    "{}: the second boot did not read `{wanted}` back",
                    arch.name
                ));
            }
        }
    }
    if !lines[from..].iter().any(|line| second_name_listed(line)) {
        return Err(format!(
            "{}: the second boot's `ls -l {SECOND_NAME}` did not count 2 links",
            arch.name
        ));
    }
    if has_line(&second.console, COPIED_LINE) {
        return Err(format!(
            "{}: the copies ended before QEMU was killed: the kill tested no write",
            arch.name
        ));
    }
    let mut volume = image_volume(&files.disk)?;
    let large = contents(&mut volume, "/home/docs/large.bin")?;
    if large.as_deref() != Some(&files.large) && large.as_deref() != Some(&files.second) {
        return Err(format!(
            "{}: after the kill, large.bin is neither version whole",
            arch.name
        ));
    }
    if let Some(partial) = contents(&mut volume, "/home/docs/t.tmp")?
        && !files.large.starts_with(&partial)
        && !files.second.starts_with(&partial)
    {
        return Err(format!(
            "{}: after the kill, t.tmp is not a prefix of either version",
            arch.name
        ));
    }
    expect_contents(
        arch.name,
        &mut volume,
        "/home/docs/a.txt",
        Some(b"hello\nmore\n"),
    )?;

    let third = boot_on_disk(
        arch,
        kernel,
        &disk,
        &RECOVERY_SESSION,
        Then::PowerMenu(WRITTEN_LINE),
    )?;
    session_verdict(
        arch.name,
        third.clean_exit,
        &third.console,
        &RECOVERY_SESSION,
        recovery_output(),
        "persistent files (after the kill, powered off from the desktop)",
    )?;
    if !third
        .console
        .lines()
        .any(|line| line.trim_end().ends_with(POWER_DOWN_LINE))
    {
        return Err(format!(
            "{}: the desktop's power menu did not power the machine off",
            arch.name
        ));
    }
    let mut volume = image_volume(&files.disk)?;
    expect_contents(
        arch.name,
        &mut volume,
        "/home/docs/a.txt",
        Some(b"hello\nmore\n"),
    )?;
    expect_contents(arch.name, &mut volume, "/home/gui.txt", Some(b"gui\n"))?;
    eprintln!("xtask: {}: persistent files boot test passed", arch.name);
    Ok(())
}
