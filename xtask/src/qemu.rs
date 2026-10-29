//! Booting kernel images in QEMU. What the boot tests run, the console lines they match (which
//! must change with the kernel's and the test programs' own) and why QEMU gets the arguments it
//! does: docs/build.md, "Developer commands".

pub mod dynamic_linking;
pub mod files;
pub mod install;
mod linux_gui;
pub mod macos_gui;
pub mod mcp;
pub mod network;
pub mod persistent;
pub mod real_time;
pub mod reboot;
pub mod run_scripts;
pub mod settings;
pub mod shell;
pub mod text_editor;
mod win_controls;
mod windows_crt;
mod windows_gui;
pub mod work_disk;

use crate::qmp::{self, InputEvent, Picture, Qmp};
use crate::{Arch, elf};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eggos_drivers::desktop::protocol::{DiscardEvents, PointerShape};
use eggos_drivers::desktop::{Contents, Desktop, GRID_SIZE, Part, WindowId, colour};
use eggos_drivers::display::{Damage, Rect};
use eggos_drivers::input::keymap::Modifiers;
use eggos_drivers::input::{AxisRange, key, scale_to_screen};
use eggos_drivers::terminal::Terminal;

const PASS_MARKER: &str = "EGGOS-SELFTEST: PASS";
const FAIL_MARKER: &str = "EGGOS-SELFTEST: FAIL";

const USER_HELLO: &str = "EGGOS-USER: hello from user mode";

const ELF_START_LINE: &str =
    "EGGOS-USER: started from an ELF image with arguments and an environment";

const RUST_PROGRAM_LINE: &str =
    "EGGOS-USER: a program built from Rust source passed its startup checks";

const STD_PROGRAM_LINE: &str = "EGGOS-USER: a std program built from Rust source ran its checks";

const CONSOLE_PROGRAM_LINE: &str =
    "EGGOS-USER: a std program read the console through its line discipline";

const USER_LINES: [&str; 4] = [
    USER_HELLO,
    ELF_START_LINE,
    RUST_PROGRAM_LINE,
    STD_PROGRAM_LINE,
];

const EXCEPTION_PREFIX: &str = "kernel exception: ";

const WRITE_PROTECT_PROBE: &str = "fatal exception test: writing read-only data at ";

const INPUT_READY_LINE: &str = "EGGOS-CONSOLE: input ready";

const CONSOLE_INPUT_SCRIPT: &[u8] = b"EggOS reads its console\r";

const CONSOLE_SCRIPTS: [(&str, &[u8]); 8] = [
    (INPUT_READY_LINE, CONSOLE_INPUT_SCRIPT),
    ("EGGOS-CONSOLE: type lines", LINES_SCRIPT),
    ("EGGOS-CONSOLE: type an interrupt", b"doomed\x03"),
    (
        "EGGOS-CONSOLE: type an interrupt and a line",
        b"gone\x03after\r",
    ),
    ("EGGOS-CONSOLE: type raw bytes", b"xy\x7fz\r"),
    (
        "EGGOS-CONSOLE: type a line to read without waiting",
        b"ready\r",
    ),
    (
        "EGGOS-CONSOLE: type lines for stdio",
        b"from musl's stdio\rfrom Rust's std\r",
    ),
    (
        "EGGOS-CONSOLE: type an interrupt for the foreground group",
        b"\x03",
    ),
];

const LINES_SCRIPT: &[u8] = b"hello\rab\x08c\rwrongx\x7f\x7f\x7f\x7f\x7f\x7fright\r\
    two words \x17\x17one\rdiscard me\x15kept\rpartial\x04\x04last\r";

const LINES_ECHO: &str = concat!(
    "hello\r\n",
    "ab\x08 \x08c\r\n",
    "wrongx",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "right\r\n",
    "two words ",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "one\r\n",
    "discard me",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "kept\r\n",
    "partiallast\r\n",
);

/// What a boot test expects the kernel to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Pass every self-test and power off, the std program reading the console as the
    /// runner types (D100).
    Pass,
    /// Pass every self-test but the interactive console test, which the benchmark kernels skip
    /// (under `-icount sleep=off`, QEMU does not wake an idle RISC-V hart for serial input),
    /// and power off.
    Benchmarks,
    /// Pass every self-test, then report a fatal exception with the fail marker and power off
    /// (the kernel's `qemu-test-fatal-exception` feature).
    FatalException,
    /// The release kernel starts init, which writes its banner, runs the std program as its
    /// login program (each of whose checks writes its line) and powers off once it ends (D106).
    Init,
    /// As [`Expect::Init`], but init resets the machine, which ends QEMU under `-no-reboot`.
    InitRestart,
    /// The release kernel starts init, which starts the shell; the test types one line of its
    /// session each time a prompt appears, as a person would, so the console's echo never mixes
    /// with what the commands write, and `^C` when the program waiting for it says so; the
    /// shell writes each line the session expects and exits with 7, and init powers off (D107).
    Shell,
    /// The release kernel boots to the prompt of the shell init starts by default; the test
    /// types a line of its session at each prompt, which runs the core utilities, each writing
    /// the lines the session expects, the last `poweroff` (D108). The time from QEMU's start to
    /// the first prompt is reported.
    BootToPrompt,
    /// The release kernel with two builds of an ordinary static Linux program added to its
    /// initramfs as `cargo xtask image --add` adds them, a static executable and one built with
    /// `relocation-model=pie` (a static PIE on x86_64), boots to the shell's prompt; the test
    /// types a line at each prompt that runs one of them, by name or by its path, and each
    /// command must write its lines in order before the next prompt; the shell exits with 7 and
    /// init powers off (D117). The machine has the offscreen display, and `linux-memory` uses
    /// memory up four ways while the kernel keeps its reserve (D155).
    LinuxPrograms,
    /// The release kernel with Windows console programs built for this CPU's Windows target
    /// (`x86_64-pc-windows-msvc` or `aarch64-pc-windows-msvc`, x64's on riscv64) and one built
    /// for the other CPU's, added as `cargo xtask image --add` adds them (with the Windows layer
    /// on x86_64 and aarch64), boots to the shell's prompt; the test types a line at each prompt
    /// that runs one, and each command must write its lines in order before the next prompt: on
    /// x86_64 and aarch64 the programs' checks, exit codes, the refusal of an unsupported import
    /// and of the other CPU's program, on riscv64 `ENOEXEC` (D118, D124).
    WindowsPrograms,
    /// The release kernel with a static Android console program, linked by the Android NDK
    /// against bionic, added as `cargo xtask image --add` adds it (with `/system/bin/sh`), boots
    /// to the shell's prompt; the test types a line at each prompt that runs it, and each
    /// command must write its lines in order before the next prompt: the program's checks
    /// (including `system`, which runs `/system/bin/sh`), an exit status and an abort (D119).
    AndroidPrograms,
    /// The release kernel with macOS console programs built by Apple's linker added as
    /// `cargo xtask image --add` adds them (with the macOS layer on aarch64 and x86_64): this
    /// CPU's program, one that links a library EggOS lacks, the other CPU's program and a fat
    /// file of both. The test types a line at each prompt that runs one, and each command must
    /// write its lines in order before the next prompt: the program's checks, exit statuses,
    /// an abort, an unimplemented function and the refusals; on riscv64, `ENOEXEC` (D120).
    MacosPrograms,
    /// The release kernel boots with a host folder attached as the run scripts attach one
    /// (`--apps`, [`host_folder_args`]): a text file and our static Linux program in a
    /// subfolder with a long name; the test types a line at each prompt that lists the folder
    /// at `/media/host`, reads the file, runs the program from it and tries to change it, and
    /// each command must write its lines in order before the next prompt: the listing, the
    /// file, the program's checks and `Read-only file system` for each change (D130).
    HostFolder,
    /// The release kernel with `hello-window` added boots to the shell's prompt; the test types
    /// a pipeline of two of them, each opening a window through `/dev/window` (D127), then
    /// through QMP clicks the first to raise it, drags it by its title bar, resizes it from its
    /// corner, maximises it by its control and by a double-click on its title bar and restores
    /// it both ways, cycles the windows with Alt+Tab, closes the first with Alt+F4 and the
    /// second with its close control (D128), checking the display, the pointer drawn on it
    /// included, against the host's render after each step; each program writes its lines in
    /// order (`shown`, its resizes and states, `close request`), and `wm list` lists the
    /// terminal once they are gone.
    ProgramWindows,
    /// The release kernel with `win-paint.exe`, a Windows GUI program built from our own source
    /// for this CPU's Windows target (x64's on riscv64), added with the Windows layer as
    /// `cargo xtask image --add` adds it, boots to the shell's prompt; the test runs it with its
    /// standard output redirected to a file and, on x86_64 and aarch64, checks the display
    /// against the host's render as the window appears painted, a click recolours its ellipse,
    /// a key adds a letter, Space opens a message box, Enter closes it with OK, F2 has a second
    /// thread open a message box while the first runs its loop, Enter closes that one too, and
    /// the close control closes the window (D129); then the program's lines in the file, its
    /// status 0, and on riscv64 `ENOEXEC`.
    WindowsGui,
    /// The release kernel with `eggpad.exe`, our own C notepad built by MSVC as a `/MD` program
    /// on the C runtime's DLLs for this CPU (x86_64 and aarch64 only), added with the Windows
    /// layer as `cargo xtask image --add` adds it, boots to the shell's prompt; the test writes
    /// a file from the shell and runs eggpad on it with its standard output redirected to a
    /// file, checks the display against the host's render as the window shows the file, three
    /// typed letters and Enter appear, Ctrl+S saves (its status line says so) and the close
    /// control closes it (D146); then the program's lines in the file (its arguments, the bytes
    /// it opened and saved, the keys it took and its `atexit` handler), the saved file read
    /// back by `cat`, and its status 0.
    WindowsCrt,
    /// The release kernel with `linux-paint`, a Wayland client built from our own source with
    /// Rust's standard library on musl, added at `/bin/linux-paint`, boots to the shell's prompt
    /// with `eggway` started by init; the test runs it with its standard output redirected to a
    /// file and checks the display against the host's render as its window appears painted with
    /// the desktop's chrome, a click recolours its target, a key adds a mark, its title bar is
    /// dragged, its handle's press moves it through `xdg_toplevel.move` (and a stale serial
    /// moves nothing), its maximise control gives it the work area and its close control ends
    /// it (D140); then the program's lines in the file and its status 0.
    LinuxGui,
    /// A kernel built with `qemu-boot-clock` boots to the prompt of the shell init starts and
    /// writes its `boot_to_prompt` benchmark line; the test ends QEMU once the line is whole,
    /// since nothing types at that prompt (D116).
    BootClock,
}

const BOOT_CLOCK_LINE: &str = "EGGOS-BENCH: boot_to_prompt ";

const SHELL_PROMPT: &str = "eggsh:/";

const SHELL_SESSION: [&[u8]; 17] = [
    b"echo EGGSH-TEST quoting 'single  quoted' \"double $HOME\" \\$HOME\r",
    b"X=abc; Y=\"x y\"; echo EGGSH-TEST variables $X${X}def \"$Y\" $Y\r",
    b"cd /usr/libexec; echo EGGSH-TEST cd $PWD $OLDPWD; cd; echo EGGSH-TEST home $PWD\r",
    b"false; echo EGGSH-TEST status $?\r",
    b"no-such-command; echo EGGSH-TEST not found $?\r",
    b"PATH=$PATH:/usr/libexec; GREETING=hi eggos-std-selftest shell-child show GREETING 'a b' c\r",
    b"echo EGGSH-TEST child $?\r",
    b"eggos-std-selftest shell-child kill-self; echo EGGSH-TEST killed $?\r",
    b"eggsh /usr/share/eggsh/test-script one two; echo EGGSH-TEST script status $?\r",
    b"eggsh -c 'echo EGGSH-TEST dash-c $0 $1' name arg\r",
    b"echo a && b; echo EGGSH-TEST never\r",
    b"echo EGGSH-TEST after unsupported $?\r",
    b"eggos-std-selftest shell-child wait-for-interrupt\r",
    b"echo EGGSH-TEST interrupted $?\r",
    b"\x03",
    b"echo EGGSH-TEST prompt interrupt $?\r",
    b"exit 7\r",
];

const LINUX_PROGRAM_SESSION: [&[u8]; 8] = [
    b"GREETING=hi linux-static a 'b c'; echo LINUX-STATIC status $?\r",
    b"GREETING=hi /bin/linux-static-pie a 'b c'; echo LINUX-STATIC status $?\r",
    b"linux-static exit 42; echo LINUX-STATIC status $?\r",
    b"linux-static-pie panic; echo LINUX-STATIC status $?\r",
    b"linux-sockets; echo LINUX-SOCKETS status $?\r",
    b"linux-memory; echo LINUX-MEMORY status $?\r",
    b"linux-mappings; echo LINUX-MAPPINGS status $?\r",
    b"exit 7\r",
];

const LINUX_SOCKETS_LINES: [&str; 10] = [
    "LINUX-SOCKETS errors ENOENT ECONNREFUSED ECONNREFUSED EADDRINUSE EAFNOSUPPORT \
     ESOCKTNOSUPPORT",
    "LINUX-SOCKETS pair ping poll timeout EAGAIN hangup eof EPIPE",
    "LINUX-SOCKETS abstract ok",
    "LINUX-SOCKETS accepted peer pid ok name /tmp/eggos-sockets.sock peer /tmp/eggos-sockets.sock",
    "LINUX-SOCKETS poll woken by the client",
    "LINUX-SOCKETS rights pipe through a passed pipe",
    "LINUX-SOCKETS memfd shared both ways size 65536 seal EPERM private ENODEV",
    "LINUX-SOCKETS large 1048576 bytes ok",
    "LINUX-SOCKETS peer closed eof EPIPE child 0",
    "LINUX-SOCKETS status 0",
];

const LINUX_MEMORY_LINES: [&str; 7] = [
    "LINUX-MEMORY memfd ENOMEM for a mapping as large as memory",
    "LINUX-MEMORY memfd ENOMEM with the reserve still free",
    "LINUX-MEMORY anonymous killed by SIGKILL",
    "LINUX-MEMORY sockets ENOMEM within an eighth of memory",
    "LINUX-MEMORY windows ENOMEM within a quarter of memory",
    "LINUX-MEMORY memory back",
    "LINUX-MEMORY status 0",
];

const LINUX_MAPPINGS_LINES: [&str; 9] = [
    "LINUX-MAPPINGS private read ok",
    "LINUX-MAPPINGS past the end SIGBUS",
    "LINUX-MAPPINGS private writes stay private, DONTNEED reads the file again",
    "LINUX-MAPPINGS shared read-only, EACCES for writing",
    "LINUX-MAPPINGS the program's own file",
    "LINUX-MAPPINGS fork reads the file's pages",
    "LINUX-MAPPINGS mremap grows into the file",
    "LINUX-MAPPINGS msync 0 ENOMEM",
    "LINUX-MAPPINGS status 0",
];

const HOST_FOLDER_SESSION: [&[u8]; 7] = [
    b"ls /media/host; ls -l /media/host/readme.txt\r",
    b"cat /media/host/readme.txt; wc -c < /media/host/README.TXT\r",
    b"cd '/media/host/Programs and Tools'; pwd; ls\r",
    b"GREETING=hi ./linux-static a 'b c'; echo LINUX-STATIC status $?\r",
    b"cd ..; pwd; echo x > new.txt; echo EGGOS-HOST-FOLDER read-only $?\r",
    b"mkdir /media/host/new; rm /media/host/readme.txt; cat readme.txt\r",
    b"exit 7\r",
];

const HOST_FOLDER_MOUNT_LINE: &str =
    "vfs: FAT16 disk \"QEMU VVFAT\" mounted read-only at /media/host";

fn host_folder_output() -> [Vec<Line>; HOST_FOLDER_SESSION.len()] {
    let whole = |lines: &[&str]| {
        lines
            .iter()
            .map(|line| Line::Whole(line.to_string()))
            .collect::<Vec<_>>()
    };
    let program = linux_program_output()[0].clone();
    [
        vec![
            Line::Whole("Programs and Tools".into()),
            Line::Whole("readme.txt".into()),
            Line::Around("-rwxr-xr-x 1 0 0 38 ", " /media/host/readme.txt"),
        ],
        whole(&["EGGOS-HOST-FOLDER hello from the host", "38"]),
        whole(&["/media/host/Programs and Tools", "linux-static"]),
        program,
        whole(&[
            "/media/host",
            "eggsh: new.txt: Read-only file system",
            "EGGOS-HOST-FOLDER read-only 1",
        ]),
        whole(&[
            "mkdir: cannot create directory '/media/host/new': Read-only file system",
            "rm: cannot remove '/media/host/readme.txt': Read-only file system",
            "EGGOS-HOST-FOLDER hello from the host",
        ]),
        whole(&[SHELL_EXIT_LINE, POWER_DOWN_LINE]),
    ]
}

/// QEMU's arguments that attach the host's `folder` (an absolute path) as a read-only FAT
/// disk on `arch`'s virtio block device: QEMU's `vvfat` driver, which presents the folder as a
/// FAT16 volume in an MBR partition (D130). `cargo xtask run --apps`, the run scripts and the
/// host folder boot test all use them.
pub fn host_folder_args(arch: &Arch, folder: &str) -> Vec<String> {
    vec![
        "-blockdev".into(),
        format!(
            "driver=vvfat,node-name=host,dir={},read-only=on",
            folder.replace(',', ",,")
        ),
        "-device".into(),
        format!("{},drive=host", arch.disk_device),
    ]
}

/// The size of the disk image `cargo xtask run` and the run scripts create when there is
/// none: 1 GiB, sparse where the host's file system allows, so it takes only what is written
/// (D131).
pub const DISK_IMAGE_BYTES: u64 = 1 << 30;

/// QEMU's arguments that attach the raw disk image `image` (an absolute path) writable on
/// `arch`'s virtio block device, which EggOS formats with EggFS if it is blank and mounts at
/// `/home` and `/Applications` (D131). EggOS tells it from a host folder by what it holds, not
/// by the order the disks come in. A guest flush reaches the host file (QEMU's default
/// write-back cache).
pub fn disk_args(arch: &Arch, image: &str) -> Vec<String> {
    vec![
        "-blockdev".into(),
        format!(
            "driver=file,node-name=eggfs-file,filename={}",
            image.replace(',', ",,")
        ),
        "-blockdev".into(),
        "driver=raw,node-name=eggfs,file=eggfs-file".into(),
        "-device".into(),
        format!("{},drive=eggfs", arch.disk_device),
    ]
}

/// QEMU's arguments that attach the work disk image `image` (an absolute path, made by
/// `cargo xtask work pack`) writable on another of `arch`'s virtio block devices, which EggOS
/// mounts at `/work` (D166). EggOS tells it from the disk that holds `/home` by the marker in
/// its root, not by the order the disks come in.
pub fn work_disk_args(arch: &Arch, image: &str) -> Vec<String> {
    vec![
        "-blockdev".into(),
        format!(
            "driver=file,node-name=work-file,filename={}",
            image.replace(',', ",,")
        ),
        "-blockdev".into(),
        "driver=raw,node-name=work,file=work-file".into(),
        "-device".into(),
        format!("{},drive=work", arch.disk_device),
    ]
}

/// Creates the disk image `path`, `bytes` long and empty, unless it exists with something in
/// it; returns whether it made one. An empty file is extended, as the run scripts do; a file
/// with contents is never cut. The file is extended without writing (sparse on APFS, ext4 and
/// most Unix file systems; on NTFS Windows reserves the space, see docs/build.md).
pub fn create_disk_image(path: &Path, bytes: u64) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > 0 => Ok(false),
        _ => reset_disk_image(path, bytes).map(|()| true),
    }
}

/// Makes the disk image `path` `bytes` long and empty, whatever it held: `--format-disk`.
pub fn reset_disk_image(path: &Path, bytes: u64) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.set_len(bytes)
        .map_err(|e| format!("{}: {e}", path.display()))
}

const PROGRAM_WINDOW_SESSION: [&[u8]; 4] = [
    b"ls /dev\r",
    b"/usr/libexec/hello-window -o first | /usr/libexec/hello-window -i second; \
      echo EGGOS-WINDOWS status $?\r",
    b"wm list\r",
    b"exit 7\r",
];

const WINDOW_TITLES: [&str; 2] = ["first", "second"];

const WM_LIST_HEADER: &str = "ID STATE X Y WIDTH HEIGHT CLIENT TITLE";

const WINDOW_CONTENT: (u32, u32) = (400, 300);

fn program_window_output() -> [Vec<Line>; PROGRAM_WINDOW_SESSION.len()] {
    let line = |text: &str| Line::Whole(text.to_owned());
    [
        vec![line("window")],
        vec![
            line("hello-window first: shown"),
            line("hello-window second: shown"),
            line("hello-window first: state maximised"),
            line("hello-window first: state normal"),
            line("hello-window first: state maximised"),
            line("hello-window first: state normal"),
            line("hello-window first: close request"),
            line("hello-window second: close request"),
            line("EGGOS-WINDOWS status 0"),
        ],
        vec![
            Line::Whole(WM_LIST_HEADER.to_owned()),
            Line::Around("64 normal ", " kernel Terminal"),
        ],
        vec![line(SHELL_EXIT_LINE), line(POWER_DOWN_LINE)],
    ]
}

const LINUX_GUI_SESSION: [&[u8]; 3] = [
    b"linux-paint > /tmp/linux-paint.log; echo LINUX-PAINT status $?\r",
    b"cat /tmp/linux-paint.log\r",
    b"exit 7\r",
];

fn linux_gui_output() -> [Vec<Line>; LINUX_GUI_SESSION.len()] {
    let whole = |line: &str| Line::Whole(line.to_owned());
    let keymap = format!(
        "LINUX-PAINT keymap xkb_v1 {}",
        eggos_wayland::keymap::US_KEYMAP.len() + 1
    );
    [
        vec![whole("LINUX-PAINT status 0")],
        vec![
            whole("LINUX-PAINT globals wl_compositor wl_shm wl_seat wl_output xdg_wm_base"),
            Line::Whole(keymap),
            whole("LINUX-PAINT configured"),
            whole("LINUX-PAINT shown"),
            Line::Around("LINUX-PAINT clicked ", ""),
            whole("LINUX-PAINT key 30 pressed"),
            whole("LINUX-PAINT move sent"),
            whole("LINUX-PAINT stale move sent"),
            Line::Around("LINUX-PAINT maximised ", ""),
            whole("LINUX-PAINT close"),
        ],
        vec![whole(SHELL_EXIT_LINE), whole(POWER_DOWN_LINE)],
    ]
}

const WINDOWS_GUI_SESSION: [&[u8]; 5] = [
    b"win-paint.exe > /tmp/win-paint.log; echo WIN-PAINT status $?; sleep 3; \
      win-controls.exe > /tmp/c.log; echo WIN-CONTROLS status $?\r",
    b"cat /tmp/win-paint.log\r",
    b"cat /tmp/c.log\r",
    b"cat /tmp/win-controls/note.txt\r",
    b"exit 7\r",
];

const WIN_PAINT_LINES: [&str; 14] = [
    "WIN-PAINT class registered",
    "WIN-PAINT created",
    "WIN-PAINT window 400x300 caption 28",
    "WIN-PAINT extra 4660",
    "WIN-PAINT sent 42",
    "WIN-PAINT posted 7 waiting",
    "WIN-PAINT posted 7 dispatched",
    "WIN-PAINT clicked 296 124",
    "WIN-PAINT typed a",
    "WIN-PAINT message box 1",
    "WIN-PAINT thread started",
    "WIN-PAINT thread message box 1",
    "WIN-PAINT destroyed",
    "WIN-PAINT quit 0",
];

const WINDOWS_CRT_SESSION: [&[u8]; 4] = [
    b"echo Hello from the shell > /tmp/note.txt; eggpad.exe 'C:\\tmp\\note.txt' > /tmp/eggpad.log; echo EGGPAD status $?\r",
    b"cat /tmp/eggpad.log\r",
    b"cat /tmp/note.txt\r",
    b"exit 7\r",
];

const EGGPAD_LINES: [&str; 5] = [
    "EGGPAD arguments 2 C:\\tmp\\note.txt",
    "EGGPAD opened 21 bytes",
    "EGGPAD saved 27 bytes",
    "EGGPAD closed typed 4",
    "EGGPAD exit",
];

fn windows_crt_output() -> [Vec<Line>; WINDOWS_CRT_SESSION.len()] {
    let whole = |lines: &[&str]| -> Vec<Line> {
        lines
            .iter()
            .map(|line| Line::Whole(line.to_string()))
            .collect()
    };
    [
        whole(&["EGGPAD status 0"]),
        whole(&EGGPAD_LINES),
        whole(&["Hello from the shell", "abc"]),
        whole(&[SHELL_EXIT_LINE, POWER_DOWN_LINE]),
    ]
}

const WIN_CONTROLS_LINES: [&str; 13] = [
    "WIN-CONTROLS folder made 1",
    "WIN-CONTROLS controls created, thread timer 1",
    "WIN-CONTROLS window created",
    "WIN-CONTROLS timer 1 fired",
    "WIN-CONTROLS timer 2 fired",
    "WIN-CONTROLS added egg at 1 after 4 edit changes",
    "WIN-CONTROLS upper case 1",
    "WIN-CONTROLS selected 1 egg",
    "WIN-CONTROLS saved C:\\tmp\\win-controls\\note.txt offset 20 extension 25 wrote 4",
    "WIN-CONTROLS opened C:\\tmp\\win-controls\\note.txt offset 20 read egg",
    "WIN-CONTROLS exit chosen",
    "WIN-CONTROLS destroyed",
    "WIN-CONTROLS quit 0",
];

fn runs_windows_programs(arch: &str) -> bool {
    matches!(arch, "x86_64" | "aarch64")
}

fn windows_gui_output(arch: &str) -> [Vec<Line>; WINDOWS_GUI_SESSION.len()] {
    let whole = |lines: &[&str]| -> Vec<Line> {
        lines
            .iter()
            .map(|line| Line::Whole(line.to_string()))
            .collect()
    };
    let end = whole(&[SHELL_EXIT_LINE, POWER_DOWN_LINE]);
    match runs_windows_programs(arch) {
        true => [
            whole(&["WIN-PAINT status 0", "WIN-CONTROLS status 0"]),
            whole(&WIN_PAINT_LINES),
            whole(&WIN_CONTROLS_LINES),
            whole(&["egg"]),
            end,
        ],
        false => [
            whole(&[
                "eggsh: win-paint.exe: Exec format error",
                "WIN-PAINT status 126",
                "eggsh: win-controls.exe: Exec format error",
                "WIN-CONTROLS status 126",
            ]),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            end,
        ],
    }
}

const WINDOWS_PROGRAM_SESSION: [&[u8]; 9] = [
    b"GREETING=hi windows-console.exe a 'b c'; echo WINDOWS-CONSOLE status $?\r",
    b"/bin/windows-console.exe exit 300; echo WINDOWS-CONSOLE status $?\r",
    b"windows-unsupported.exe; echo WINDOWS-CONSOLE status $?\r",
    b"other-cpu.exe; echo WINDOWS-CONSOLE status $?\r",
    b"windows-tools.exe dir 'C:\\bin\\windows-*.exe' 'C:\\bin\\windows-?ools.exe' \
      'C:\\usr\\share\\windows-tools\\*' 'C:\\nope\\*' 'C:\\bin\\*.zzz'; \
      echo WINDOWS-TOOLS status $?\r",
    b"windows-tools.exe cat 'C:\\usr\\share\\windows-tools\\*.txt' 'C:\\etc\\hostname' \
      'C:\\etc\\missing' 'C:\\nope\\x'; echo WINDOWS-TOOLS status $?\r",
    b"windows-tools.exe checks; echo WINDOWS-TOOLS status $?\r",
    b"windows-tools.exe detach; echo WINDOWS-TOOLS status $?\r",
    b"exit 7\r",
];

const ANDROID_PROGRAM_SESSION: [&[u8]; 4] = [
    b"GREETING=hi android-console a 'b c'; echo ANDROID-CONSOLE status $?\r",
    b"/bin/android-console exit 42; echo ANDROID-CONSOLE status $?\r",
    b"android-console abort; echo ANDROID-CONSOLE status $?\r",
    b"exit 7\r",
];

const MACOS_PROGRAM_SESSION: [&[u8]; 16] = [
    b"GREETING=hi macos-console a 'b c'; echo MACOS-CONSOLE status $?\r",
    b"/bin/macos-console exit 42; echo MACOS-CONSOLE status $?\r",
    b"macos-fat exit 3; echo MACOS-CONSOLE status $?\r",
    b"macos-console abort; echo MACOS-CONSOLE status $?\r",
    b"macos-console unimplemented; echo MACOS-CONSOLE status $?\r",
    b"macos-unsupported; echo MACOS-CONSOLE status $?\r",
    b"macos-other; echo MACOS-CONSOLE status $?\r",
    b"GREETING=hi macos-std a 'b c'; echo MACOS-CONSOLE status $?\r",
    b"macos-std exit 5; echo MACOS-CONSOLE status $?\r",
    b"macos-tools -v --verbose -n 3 --list /usr/share/eggos --name=eggos rest more; \
      echo MACOS-CONSOLE status $?\r",
    b"macos-tools -x; echo MACOS-CONSOLE status $?\r",
    b"macos-objc; echo MACOS-CONSOLE status $?\r",
    b"macos-objc unrecognized; echo MACOS-CONSOLE status $?\r",
    b"macos-objc unimplemented; echo MACOS-CONSOLE status $?\r",
    b"macos-objc bench; echo MACOS-CONSOLE status $?\r",
    b"exit 7\r",
];

const OBJC_LOAD_LINES: [&str; 2] = [
    "MACOS-OBJC load Counter",
    "MACOS-OBJC load NSString(EggShout)",
];

const OBJC_PROGRAM_LINES: [&str; 55] = [
    "MACOS-OBJC main",
    "MACOS-OBJC initialize Counter",
    "MACOS-OBJC <Counter egg 2>",
    "MACOS-OBJC items 1,2",
    "MACOS-OBJC triple 2 4 6",
    "MACOS-OBJC kinds 1 0 1 0",
    "MACOS-OBJC class Counter initWithName: Counter",
    "MACOS-OBJC dealloc egg",
    "MACOS-OBJC counter pool done",
    "MACOS-OBJC named spam 0",
    "MACOS-OBJC dealloc spam",
    "MACOS-OBJC named pool done",
    "MACOS-OBJC shout ABC!",
    "MACOS-OBJC wide 5 h\u{e9}llo 233",
    "MACOS-OBJC built 11 w\u{f6}rld equal prefix",
    "MACOS-OBJC a|b||c",
    "MACOS-OBJC array (",
    "    1,",
    "    two,",
    "    \"3.5\",",
    "        (",
    "        4,",
    "        five",
    "    ),",
    "        {",
    "        k = v;",
    "    }",
    ")",
    "MACOS-OBJC dictionary {",
    "    Beta = \"2.5\";",
    "    alpha = 1;",
    "    mid = \"a b\";",
    "    zeta = 1;",
    "}",
    "MACOS-OBJC empty (",
    ") {",
    "}",
    "MACOS-OBJC item 1",
    "MACOS-OBJC item two",
    "MACOS-OBJC item 3.5",
    "MACOS-OBJC mutable w+y 2",
    "MACOS-OBJC table 2 3 a,b",
    "MACOS-OBJC table {",
    "    a = 1;",
    "    b = 3;",
    "}",
    "MACOS-OBJC text big eggs 12 11",
    "MACOS-OBJC numbers 42 -7 0.1 3.25 1 9223372036854775808 99",
    "MACOS-OBJC compare 1 1 -1 42",
    "MACOS-OBJC bundle 1 (null) 0",
    "MACOS-OBJC thread noisy",
    "MACOS-OBJC dealloc Noisy on its thread",
    "MACOS-OBJC joined",
    "MACOS-OBJC done",
    "MACOS-CONSOLE status 0",
];

const OBJC_UNRECOGNIZED: &str = "*** Terminating app due to uncaught exception \
     'NSInvalidArgumentException', reason: '-[Counter frobnicate]: unrecognized selector sent to \
     instance 0x";

fn objc_program_output() -> [Vec<Line>; 4] {
    let whole = |line: &str| Line::Whole(line.to_owned());
    let loads = || OBJC_LOAD_LINES.iter().map(|line| whole(line));
    let mut program: Vec<Line> = loads().collect();
    program.extend(OBJC_PROGRAM_LINES.iter().map(|line| whole(line)));
    program.insert(
        3,
        Line::Around("", "] MACOS-OBJC NSLog hello 42 world (null)"),
    );
    let mut unrecognized: Vec<Line> = loads().collect();
    unrecognized.extend([
        whole("MACOS-OBJC initialize Counter"),
        Line::Around(OBJC_UNRECOGNIZED, "'"),
        whole("libc++abi: terminating due to uncaught exception of type NSException"),
        whole("Aborted"),
        whole("MACOS-CONSOLE status 134"),
    ]);
    let mut unimplemented: Vec<Line> = loads().collect();
    unimplemented.extend([
        whole("eggos-macos: unimplemented CoreFoundation class NSDate"),
        whole("MACOS-CONSOLE status 127"),
    ]);
    let mut bench: Vec<Line> = loads().collect();
    bench.extend([
        whole("MACOS-OBJC initialize Counter"),
        Line::Around("MACOS-OBJC bench send ", " ns (0)"),
        whole("MACOS-OBJC dealloc bench"),
        whole("MACOS-CONSOLE status 0"),
    ]);
    [program, unrecognized, unimplemented, bench]
}

fn macos_program_output(arch: &str) -> [Vec<Line>; MACOS_PROGRAM_SESSION.len()] {
    let whole = |lines: &[&str]| -> Vec<Line> {
        lines
            .iter()
            .map(|line| Line::Whole(line.to_string()))
            .collect()
    };
    let end = whole(&[SHELL_EXIT_LINE, POWER_DOWN_LINE]);
    let status = |code: u8| Line::Whole(format!("MACOS-CONSOLE status {code}"));
    if arch == "riscv64" {
        let refused = |name: &str| {
            vec![
                Line::Whole(format!("eggsh: {name}: Exec format error")),
                status(126),
            ]
        };
        return [
            refused("macos-console"),
            refused("/bin/macos-console"),
            refused("macos-fat"),
            refused("macos-console"),
            refused("macos-console"),
            refused("macos-unsupported"),
            refused("macos-other"),
            refused("macos-std"),
            refused("macos-std"),
            refused("macos-tools"),
            refused("macos-tools"),
            refused("macos-objc"),
            refused("macos-objc"),
            refused("macos-objc"),
            refused("macos-objc"),
            end,
        ];
    }
    let own = if arch == "x86_64" { "x86_64" } else { "arm64" };
    let mut objc_program = objc_program_output().into_iter();
    [
        whole(&[
            "MACOS-CONSOLE macos-console args a|b c",
            "MACOS-CONSOLE apple executable_path",
            "MACOS-CONSOLE env GREETING=hi envp setenv",
            "MACOS-CONSOLE printf",
            "MACOS-CONSOLE ints -42 7 3000000000 -1234567890123 9000000000000000000 12 beef BEEF \
             10 Z %",
            "MACOS-CONSOLE widths [   42] [42   ] [00042] [+42] [abc] [   right] [left    ] \
             [0xff] [   7]",
            "MACOS-CONSOLE floats 3.25 -0.500000 2 1.234568e+04 0.0001",
            "MACOS-CONSOLE snprintf 15 truncat",
            "MACOS-CONSOLE puts",
            "MACOS-CONSOLE strings strlen strcmp strchr strstr strdup memcmp",
            "MACOS-CONSOLE numbers qsort strtol atoi",
            "MACOS-CONSOLE file eggos",
            "MACOS-CONSOLE missing ENOENT, read-only, open read close",
            "MACOS-CONSOLE stat regular 6 bytes, directory",
            "MACOS-CONSOLE dir about",
            "MACOS-CONSOLE cwd chdir getcwd relative",
            "MACOS-CONSOLE malloc small large realloc",
            "MACOS-CONSOLE time monotonic realtime mach gettimeofday",
            "MACOS-CONSOLE pid positive",
            "MACOS-CONSOLE stderr",
            "MACOS-CONSOLE atexit",
            "MACOS-CONSOLE status 0",
        ]),
        vec![status(42)],
        vec![status(3)],
        whole(&["Aborted", "MACOS-CONSOLE status 134"]),
        whole(&[
            "eggos-macos: unimplemented libSystem function posix_spawn",
            "MACOS-CONSOLE status 127",
        ]),
        vec![
            Line::Around(
                "macos: ",
                "macos-unsupported: needs /usr/lib/libz.1.dylib, which EggOS does not provide yet",
            ),
            status(126),
        ],
        vec![
            Line::Around(
                "macos: ",
                if own == "x86_64" {
                    "macos-other: has code only for arm64, not for this x86_64 machine"
                } else {
                    "macos-other: has code only for x86_64, not for this arm64 machine"
                },
            ),
            status(126),
        ],
        whole(&[
            "MACOS-STD hello from Rust std",
            "MACOS-STD args a|b c",
            "MACOS-STD env GREETING=hi",
            "MACOS-STD file EggOS",
            "MACOS-STD dir /bin eggsh macos-std",
            "MACOS-STD missing NotFound",
            "MACOS-STD threads 4 sum 10 tls [2, 2, 2, 2] main 1 names \
             worker-1,worker-2,worker-3,worker-4",
            "MACOS-CONSOLE status 0",
        ]),
        vec![status(5)],
        whole(&[
            "MACOS-TOOLS options v=2 n=3 name=eggos optind=8 rest=rest",
            "MACOS-TOOLS dir /usr/share/eggos: about",
            "MACOS-TOOLS time 1970-01-01 00:00:00 Thu Jan 001 UTC | 2023-11-14 22:13:20 \
             Tuesday timegm nanosleep now",
            "MACOS-TOOLS errno -1 2 No such file or directory",
            "MACOS-TOOLS locale C US-ASCII . other refused",
            "MACOS-CONSOLE status 0",
        ]),
        whole(&["macos-tools: illegal option -- x", "MACOS-CONSOLE status 2"]),
        objc_program.next().unwrap_or_default(),
        objc_program.next().unwrap_or_default(),
        objc_program.next().unwrap_or_default(),
        objc_program.next().unwrap_or_default(),
        end,
    ]
}

fn android_program_output() -> [Vec<Line>; ANDROID_PROGRAM_SESSION.len()] {
    let whole = |lines: &[&str]| -> Vec<Line> {
        lines
            .iter()
            .map(|line| Line::Whole(line.to_string()))
            .collect()
    };
    [
        whole(&[
            "ANDROID-CONSOLE android-console args a|b c",
            "ANDROID-CONSOLE env GREETING=hi",
            "ANDROID-CONSOLE file eggos",
            "ANDROID-CONSOLE dir about",
            "ANDROID-CONSOLE malloc small large realloc",
            "ANDROID-CONSOLE time monotonic realtime",
            "ANDROID-CONSOLE threads 4 joined sum 30",
            "ANDROID-CONSOLE property ro.build.version.sdk unset",
            "ANDROID-CONSOLE pagesize 4096",
            "ANDROID-CONSOLE random differs",
            "ANDROID-CONSOLE shell",
            "ANDROID-CONSOLE system status 0",
            "ANDROID-CONSOLE stderr",
            "ANDROID-CONSOLE status 0",
        ]),
        whole(&["ANDROID-CONSOLE status 42"]),
        whole(&["Aborted", "ANDROID-CONSOLE status 134"]),
        whole(&[SHELL_EXIT_LINE, POWER_DOWN_LINE]),
    ]
}

fn windows_program_output(arch: &str) -> [Vec<Line>; WINDOWS_PROGRAM_SESSION.len()] {
    let whole = |lines: &[&str]| -> Vec<Line> {
        lines
            .iter()
            .map(|line| Line::Whole(line.to_string()))
            .collect()
    };
    let end = whole(&[SHELL_EXIT_LINE, POWER_DOWN_LINE]);
    let (architecture, other_cpu) = match arch {
        "x86_64" => (
            9,
            "other-cpu.exe: an ARM64 program, and this computer runs x64 programs",
        ),
        "aarch64" => (
            12,
            "other-cpu.exe: an x64 program, and this computer runs ARM64 programs \
             (EggOS does not translate x64 programs for Arm yet)",
        ),
        _ => {
            let refused = |name: &str| {
                vec![
                    Line::Whole(format!("eggsh: {name}: Exec format error")),
                    Line::Whole("WINDOWS-CONSOLE status 126".into()),
                ]
            };
            let tools = || {
                vec![
                    Line::Whole("eggsh: windows-tools.exe: Exec format error".into()),
                    Line::Whole("WINDOWS-TOOLS status 126".into()),
                ]
            };
            return [
                refused("windows-console.exe"),
                refused("/bin/windows-console.exe"),
                refused("windows-unsupported.exe"),
                refused("other-cpu.exe"),
                tools(),
                tools(),
                tools(),
                tools(),
                end,
            ];
        }
    };
    let system = format!(
        "WINDOWS-TOOLS system arch {architecture} page 4096 granularity 65536 cpus some \
         version 10.0.26100 module C:\\bin\\windows-tools.exe"
    );
    [
        whole(&[
            "WINDOWS-CONSOLE windows-console.exe args a|b c",
            "WINDOWS-CONSOLE env GREETING=hi",
            "WINDOWS-CONSOLE teb ok",
            "WINDOWS-CONSOLE file eggos",
            "WINDOWS-CONSOLE missing file refused error 3",
            "WINDOWS-CONSOLE heap ok",
            "WINDOWS-CONSOLE time ok",
            "WINDOWS-CONSOLE stack ok",
            "WINDOWS-CONSOLE library ok",
            "WINDOWS-CONSOLE tls ok",
            "WINDOWS-CONSOLE stderr",
            "WINDOWS-CONSOLE status 0",
        ]),
        whole(&["WINDOWS-CONSOLE status 44"]),
        vec![
            Line::Around(
                "windows: ",
                "windows-unsupported.exe: needs CreateProcessW from kernel32.dll, \
                 which EggOS does not provide yet",
            ),
            Line::Whole("WINDOWS-CONSOLE status 126".into()),
        ],
        vec![
            Line::Around("windows: ", other_cpu),
            Line::Whole("WINDOWS-CONSOLE status 126".into()),
        ],
        whole(&[
            concat!(
                r"WINDOWS-TOOLS dir C:\bin\windows-*.exe: windows-console.exe ",
                "windows-tools.exe windows-unsupported.exe"
            ),
            r"WINDOWS-TOOLS dir C:\bin\windows-?ools.exe: windows-tools.exe",
            r"WINDOWS-TOOLS dir C:\usr\share\windows-tools\*: ..\ .\ über.txt",
            r"WINDOWS-TOOLS dir C:\nope\*: error 3",
            r"WINDOWS-TOOLS dir C:\bin\*.zzz: error 2",
            "WINDOWS-TOOLS status 1",
        ]),
        whole(&[
            "WINDOWS-TOOLS über.txt: grüße aus EggOS 🥚",
            "WINDOWS-TOOLS über.txt: zweite Zeile",
            r"WINDOWS-TOOLS C:\etc\hostname: eggos",
            concat!(
                r"WINDOWS-TOOLS cat C:\etc\missing: error 2 ",
                "The system cannot find the file specified."
            ),
            concat!(
                r"WINDOWS-TOOLS cat C:\nope\x: error 3 ",
                "The system cannot find the path specified."
            ),
            "WINDOWS-TOOLS status 1",
        ]),
        whole(&[
            "WINDOWS-TOOLS threads 4 joined exits 10 11 12 13 interlocked 4000 critical 4000 \
             mutex 4000 srw 4000 once 1",
            "WINDOWS-TOOLS threads semaphore 4 suspended ok exit-thread 77 fls-cleanups 4 \
             per-thread tls and last error ok",
            "WINDOWS-TOOLS sync wait-on-address ok timeout 1460 condition ok shared-srw ok \
             check-only 0 current-thread 259",
            "WINDOWS-TOOLS errors 2 3 16 183 2 267 6 203 10 122 1 6 288 298 0",
            "WINDOWS-TOOLS message 2 The system cannot find the file specified.",
            "WINDOWS-TOOLS text grüße 🥚 Ελληνικά units 17 bytes 29 round trip ok invalid 1 1113 \
             unpaired 1 1113 ordinal ok",
            &system,
            "WINDOWS-TOOLS time epoch 1970-1-1 weekday 4 now ok",
            concat!(
                r"WINDOWS-TOOLS cwd C:\etc full C:\etc\hostname part hostname relative ",
                "open ok small buffer 7"
            ),
            "WINDOWS-TOOLS env EGG=yolk ü block ok removed 0 203",
            "WINDOWS-TOOLS status 0",
        ]),
        whole(&[
            "WINDOWS-TOOLS detach the first thread returns 0",
            "WINDOWS-TOOLS detach the last thread ran after the first one returned",
            "WINDOWS-TOOLS status 5",
        ]),
        end,
    ]
}

fn linux_program_output() -> [Vec<Line>; LINUX_PROGRAM_SESSION.len()] {
    let checks = |name: &'static str| {
        vec![
            Line::Whole(format!("LINUX-STATIC {name} args a|b c")),
            Line::Whole("LINUX-STATIC env GREETING=hi".into()),
            Line::Whole("LINUX-STATIC file eggos".into()),
            Line::Whole("LINUX-STATIC dir about".into()),
            Line::Whole("LINUX-STATIC time monotonic realtime".into()),
            Line::Whole("LINUX-STATIC hashmap egg=3 shell=1 yolk=1".into()),
            Line::Whole("LINUX-STATIC threads 4 joined sum 30".into()),
            Line::Whole(format!("LINUX-STATIC cpus {TEST_CPUS}")),
            Line::Whole("LINUX-STATIC stderr".into()),
            Line::Whole("LINUX-STATIC status 0".into()),
        ]
    };
    [
        checks("linux-static"),
        checks("linux-static-pie"),
        vec![Line::Whole("LINUX-STATIC status 42".into())],
        vec![
            Line::Around("thread 'main' ", ":"),
            Line::Whole("LINUX-STATIC panicked as asked".into()),
            Line::Whole("Aborted".into()),
            Line::Whole("LINUX-STATIC status 134".into()),
        ],
        LINUX_SOCKETS_LINES
            .iter()
            .map(|line| Line::Whole((*line).into()))
            .collect(),
        LINUX_MEMORY_LINES
            .iter()
            .map(|line| Line::Whole((*line).into()))
            .collect(),
        LINUX_MAPPINGS_LINES
            .iter()
            .map(|line| Line::Whole((*line).into()))
            .collect(),
        vec![
            Line::Whole(SHELL_EXIT_LINE.into()),
            Line::Whole(POWER_DOWN_LINE.into()),
        ],
    ]
}

const PROMPT_SESSION: [&[u8]; 26] = [
    b"ls /\r",
    b"ls -l /bin/ls; ls /nope; echo EGGOS-COREUTILS ls status $?\r",
    b"cat /etc/motd /etc/hostname\r",
    UNAME_LINE,
    b"/bin/echo -n EGGOS-COREUTILS external; /bin/echo ' echo'\r",
    b"sleep 0; sleep 0.05; echo EGGOS-COREUTILS slept $?\r",
    b"/bin/false; echo EGGOS-COREUTILS false $?\r",
    SHIFTED_KEYS_LINE,
    b"echo hello | wc\r",
    b"ls / | grep bin | head -n 1\r",
    b"cat < /etc/motd | grep -c -i eggos; wc -l < /etc/hostname\r",
    b"ls /nope | grep x; echo EGGOS-PIPE last $?; true | false; echo EGGOS-PIPE status $?\r",
    b"tail -n 1 /etc/motd /etc/hostname | head -n 4 | tail -n +2\r",
    PIPELINE_READING_THE_TERMINAL,
    b"echo EGGOS-PIPE interrupted $?\r",
    b"ls /proc; cd /proc; pwd; cd ..; pwd; grep -c kB /proc/meminfo\r",
    b"ps | head -n 1; ps | grep ' init$'; ps | grep -c ' R ps$'\r",
    b"free | head -n 1; free | grep -c '^Mem: '; uptime | grep -c '^up [0-9]'\r",
    b"echo a > /tmp/x; echo b >> /tmp/x; cat /tmp/x; cat < /tmp/x\r",
    b"mkdir /home/u; touch /home/u/f; cp /tmp/x /home/u/y; mv /home/u/y /home/u/z; ls /home/u\r",
    b"rm /home/u/z; ls /home/u; echo x > /etc/motd; echo EGGOS-FILES read-only $?\r",
    b"mkdir -p /tmp/a/b; mv /tmp/x /home; rm -r /home/u /tmp/a; ls /tmp /home; wc -c < /home/x\r",
    b"echo EGGOS-HISTORY recalled\r",
    b"echo EGGOS-HISTORY newest\r",
    RECALL_THE_LINE_BEFORE_LAST,
    b"poweroff\r",
];

const RECALL_THE_LINE_BEFORE_LAST: &[u8] = b"\x1b[A\x1b[A\r";

fn shown_command(session: &[&[u8]], index: usize) -> String {
    let typed = match session[index] {
        RECALL_THE_LINE_BEFORE_LAST => session[index - 2],
        typed => typed,
    };
    String::from_utf8_lossy(typed)
        .trim_end_matches('\r')
        .to_owned()
}

const PIPELINE_READING_THE_TERMINAL: &[u8] = b"cat | grep EGGOS-PIPE\r";

const PIPELINE_INPUT_LINE: &str = "EGGOS-PIPE interrupt me";

#[derive(Debug, Clone, Copy)]
struct Reaction {
    line_ends_with: &'static str,
    occurrences: usize,
    keys: &'static [u8],
}

const REACTIONS: [Reaction; 2] = [
    Reaction {
        line_ends_with: "# cat | grep EGGOS-PIPE",
        occurrences: 1,
        keys: b"EGGOS-PIPE interrupt me\r",
    },
    Reaction {
        line_ends_with: PIPELINE_INPUT_LINE,
        occurrences: 2,
        keys: b"\x03",
    },
];

const PROMPT_SESSION_FRAME_PROMPT: usize = 9;

const _: () =
    assert!(PROMPT_SESSION[PROMPT_SESSION_FRAME_PROMPT - 2].len() == SHIFTED_KEYS_LINE.len());

const UNAME_LINE: &[u8] = b"uname -a\r";

const SHIFTED_KEYS_LINE: &[u8] = b"echo EGGOS-KEYBOARD Typed_On+Virtio:OK\r";

const KEYBOARD_LINES: [&[u8]; 2] = [UNAME_LINE, SHIFTED_KEYS_LINE];

const KEY_PAUSE: Duration = Duration::from_millis(10);

const KEYS_BEFORE_ECHO: usize = 8;

#[derive(Debug)]
struct KeyboardTyping {
    text: Vec<u8>,
    typed: usize,
    echo_from: usize,
}

impl KeyboardTyping {
    fn new(text: &[u8], echo_from: usize) -> Self {
        Self {
            text: text.to_vec(),
            typed: 0,
            echo_from,
        }
    }

    fn next_keys(&mut self, console: &[u8]) -> Option<String> {
        let echoed = console.get(self.echo_from..).unwrap_or_default();
        if self.is_done() || !echoed.starts_with(&self.text[..self.typed]) {
            return None;
        }
        let end = (self.typed + KEYS_BEFORE_ECHO).min(self.text.len());
        let keys = String::from_utf8_lossy(&self.text[self.typed..end]).into_owned();
        self.typed = end;
        Some(keys)
    }

    fn is_done(&self) -> bool {
        self.typed == self.text.len()
    }
}

const INPUT_EVENTS_LINE: &str = "EGGOS-INPUT: send pointer and key events";

const TABLET_POSITION: (i32, i32) = (8192, 24576);
const MOUSE_MOTION: (i32, i32) = (7, -5);

const DEVICE_PAUSE: Duration = Duration::from_millis(100);

const BOOT_INPUT_LINE: &str = "boot: input keyboard pointer";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Whole(String),
    Around(&'static str, &'static str),
}

impl Line {
    fn matches(&self, line: &str) -> bool {
        match self {
            Line::Whole(whole) => line == whole,
            Line::Around(start, end) => {
                line.len() >= start.len() + end.len()
                    && line.starts_with(start)
                    && line.ends_with(end)
            }
        }
    }
}

fn prompt_session_output(arch: &str) -> [Vec<Line>; PROMPT_SESSION.len()] {
    let whole = |lines: &[&str]| {
        lines
            .iter()
            .map(|line| Line::Whole(line.to_string()))
            .collect()
    };
    [
        whole(&[
            "Applications",
            "bin",
            "dev",
            "etc",
            "home",
            "init",
            "media",
            "proc",
            "run",
            "tmp",
            "usr",
            "var",
            "work",
        ]),
        vec![
            Line::Around("-rwxr-xr-x 30 0 0 ", " /bin/ls"),
            Line::Whole("ls: cannot access '/nope': No such file or directory".into()),
            Line::Whole("EGGOS-COREUTILS ls status 2".into()),
        ],
        whole(&[
            "Welcome to EggOS. Type help. Apps: the egg, or Super.",
            "eggos",
        ]),
        vec![Line::Whole(format!(
            "Linux localhost 6.1.0-eggos #1 SMP EggOS {} {arch} EggOS",
            env!("CARGO_PKG_VERSION")
        ))],
        whole(&["EGGOS-COREUTILS external echo"]),
        whole(&["EGGOS-COREUTILS slept 0"]),
        whole(&["EGGOS-COREUTILS false 1"]),
        whole(&["EGGOS-KEYBOARD Typed_On+Virtio:OK"]),
        whole(&["      1       1       6"]),
        whole(&["bin"]),
        whole(&["1", "1"]),
        whole(&[
            "ls: cannot access '/nope': No such file or directory",
            "EGGOS-PIPE last 1",
            "EGGOS-PIPE status 1",
        ]),
        whole(&[
            "Welcome to EggOS. Type help. Apps: the egg, or Super.",
            "",
            "==> /etc/hostname <==",
        ]),
        whole(&[PIPELINE_INPUT_LINE, PIPELINE_INPUT_LINE]),
        whole(&["EGGOS-PIPE interrupted 130"]),
        whole(&[
            "cmdline", "driver", "meminfo", "net", "ps", "uptime", "/proc", "/", "4",
        ]),
        whole(&["  PID  PPID  PGID S CMD", "    1     0     1 R init", "1"]),
        whole(&[
            "               total        used        free      shared  buff/cache   available",
            "1",
            "1",
        ]),
        whole(&["a", "b", "a", "b"]),
        whole(&["f", "z"]),
        whole(&[
            "f",
            "eggsh: /etc/motd: Read-only file system",
            "EGGOS-FILES read-only 1",
        ]),
        whole(&["/home:", "x", "", "/tmp:", "4"]),
        whole(&["EGGOS-HISTORY recalled"]),
        whole(&["EGGOS-HISTORY newest"]),
        whole(&["EGGOS-HISTORY recalled"]),
        whole(&[POWER_DOWN_LINE]),
    ]
}

const SHELL_INTERRUPT: (&str, &[u8]) = (
    "EGGOS-SHELL-CHILD: waiting for an interrupt in the foreground group",
    b"\x03",
);

#[derive(Debug, Default)]
struct ShellTyping {
    lines: usize,
    interrupts: usize,
    reactions: [bool; REACTIONS.len()],
}

const SHELL_LINES: [&str; 19] = [
    "EGGSH-TEST quoting single  quoted double / $HOME",
    "EGGSH-TEST variables abcabcdef x y x y",
    "EGGSH-TEST cd /usr/libexec /",
    "EGGSH-TEST home /home/root",
    "EGGSH-TEST status 1",
    "eggsh: no-such-command: command not found",
    "EGGSH-TEST not found 127",
    "EGGOS-SHELL-CHILD: GREETING=hi args=a b|c",
    "EGGSH-TEST child 3",
    "Terminated",
    "EGGSH-TEST killed 143",
    "EGGSH-TEST script /usr/share/eggsh/test-script one 2",
    "EGGSH-TEST sourced yes",
    "EGGSH-TEST script status 5",
    "EGGSH-TEST dash-c name arg",
    "eggsh: `&&` is not supported yet",
    "EGGSH-TEST after unsupported 2",
    "EGGSH-TEST interrupted 130",
    "EGGSH-TEST prompt interrupt 130",
];

const SHELL_EXIT_LINE: &str = "init: /bin/eggsh exited with status 7";

const INIT_BANNER: &str = concat!("EggOS ", env!("CARGO_PKG_VERSION"));

const INIT_LOGIN_LINES: [&str; 4] = [
    "EGGOS-INIT-TEST: the login program is init's child, leading the console's session",
    "EGGOS-INIT-TEST: an orphan was reparented to init",
    "EGGOS-INIT-TEST: init reaped the orphan",
    "EGGOS-INIT-TEST: reboot with bad magic was refused",
];

const POWER_DOWN_LINE: &str = "reboot: Power down";

const RESTART_LINE: &str = "reboot: Restarting system";

const KERNEL_PANIC_PREFIX: &str = "Kernel panic";

const BOOT_TIMEOUT: Duration = Duration::from_secs(30);

const BENCHMARK_BOOT_TIMEOUT: Duration = Duration::from_secs(90);

const BANNER_PREFIX: &str = "EggOS ";

const COMMON_ARGS: &[&str] = &["-m", "128M"];

/// Whether a boot gives the machine its display device ([`Arch::display_args`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Attached,
    /// No display device: the kernel must behave as it did before it drove displays (D112).
    Absent,
}

const TERMINAL_LINE_PREFIX: &str = "boot: terminal ";

const TERMINAL_LINE_1280X800: &str = "boot: terminal 80x24";

/// What the kernel says when it shows its display at 1280x800, the default mode.
pub const DISPLAY_LINE_1280X800: &str = "boot: display 1280x800";

/// How the kernel's line about an offscreen display it made starts (D144).
pub const OFFSCREEN_LINE_PREFIX: &str = "boot: offscreen display ";

/// What the kernel says when it made an offscreen display at the default mode (D144).
pub const OFFSCREEN_LINE_1280X800: &str = "boot: offscreen display 1280x800";

/// What the kernel says when it found no input device.
pub const NO_INPUT_LINE: &str = "boot: no input";

const TERMINAL_FRAME_TIMEOUT: Duration = Duration::from_secs(5);

const TERMINAL_FRAME_RETRY: Duration = Duration::from_millis(50);

/// CPUs in the boot tests and interactive runs, so every boot starts the secondary CPUs
/// (D40). Must match `eggos_kernel::selftest::QEMU_CPUS`.
pub const TEST_CPUS: u32 = 4;

const BSS_PROBE_SYMBOL: &str = "eggos_selftest_bss_probe";

const BSS_POISON: u64 = 0xa5a5_a5a5_a5a5_a5a5;

/// QEMU arguments that fill the kernel's `.bss` probe with [`BSS_POISON`] before the first
/// instruction runs (one `loader` device per 8-byte word). QEMU applies them at reset after
/// placing the kernel image and before any firmware runs: observed on QEMU 10 for all three
/// architectures (a kernel without its zeroing loop fails), not a documented guarantee, so a
/// QEMU upgrade that changes it shows up as `.bss` failing here. QEMU hands over zeroed RAM, so
/// without this the `bss_is_zeroed` self-test would pass even if the entry point never
/// cleared `.bss`.
pub fn bss_poison_args(kernel: &Path) -> Result<Vec<String>, String> {
    let image = std::fs::read(kernel).map_err(|e| format!("{}: {e}", kernel.display()))?;
    let probe = elf::find_symbol(&image, BSS_PROBE_SYMBOL)
        .map_err(|e| format!("{}: {e}", kernel.display()))?;
    if probe.size == 0 || probe.size % 8 != 0 {
        return Err(format!(
            "{}: `{BSS_PROBE_SYMBOL}` is {} bytes, not a whole number of 8-byte words",
            kernel.display(),
            probe.size
        ));
    }
    Ok((0..probe.size / 8)
        .flat_map(|word| {
            let address = probe.physical_address + word * 8;
            [
                "-device".to_string(),
                format!("loader,addr={address:#x},data={BSS_POISON:#x},data-len=8"),
            ]
        })
        .collect())
}

/// Boots `kernel` headless on `cpus` CPUs, with its display device unless `display` is
/// [`Display::Absent`] and `extra_args` added to QEMU's, and passes only if QEMU powers off
/// cleanly within the timeout and the console shows what `expect` asks for (see [`verdict`]).
/// A boot to the prompt with a display also checks, through QMP, that the display shows the
/// first frame. Returns the console output.
pub fn boot_test(
    arch: &Arch,
    kernel: &Path,
    expect: Expect,
    cpus: u32,
    display: Display,
    extra_args: &[&str],
) -> Result<String, String> {
    boot(arch, kernel, expect, cpus, display, extra_args).map(|booted| booted.console)
}

/// What a boot test saw: the console's output and, for a boot to the prompt, the time from
/// QEMU's start to the first prompt.
#[derive(Debug)]
pub struct Booted {
    pub console: String,
    pub first_prompt: Option<Duration>,
}

/// Runs the boot-to-prompt test ([`Expect::BootToPrompt`]) on `kernel` with its display, as a
/// user boots it, and returns the time from QEMU's start to the first prompt.
pub fn boot_to_prompt_time(arch: &Arch, kernel: &Path) -> Result<Duration, String> {
    let booted = boot(
        arch,
        kernel,
        Expect::BootToPrompt,
        TEST_CPUS,
        Display::Attached,
        &[],
    )?;
    booted
        .first_prompt
        .ok_or_else(|| format!("{}: no prompt was seen", arch.name))
}

fn boot(
    arch: &Arch,
    kernel: &Path,
    expect: Expect,
    cpus: u32,
    display: Display,
    extra_args: &[&str],
) -> Result<Booted, String> {
    eprintln!(
        "xtask: booting {} in {} ({expect:?}, display {display:?})",
        arch.name, arch.qemu
    );
    let (display_args, input_args) = match display {
        Display::Attached => (arch.display_args, arch.input_args),
        Display::Absent => (&[][..], &[][..]),
    };
    let qmp_port = match (expect, display) {
        (Expect::Benchmarks | Expect::BootClock, _) | (_, Display::Absent) => None,
        _ => Some(qmp::free_port()?),
    };
    let mut child = Command::new(arch.qemu)
        .args(arch.machine_args)
        .args(display_args)
        .args(input_args)
        .args(COMMON_ARGS)
        .arg("-smp")
        .arg(cpus.to_string())
        .args(qmp_port.map(qmp::qemu_args).unwrap_or_default())
        .args(extra_args)
        .args(["-display", "none", "-serial", "stdio", "-monitor", "none"])
        .arg("-kernel")
        .arg(kernel)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;

    let rx = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
    let mut keyboard = child.stdin.take().expect("stdin is piped");

    let timeout = match expect {
        Expect::Benchmarks | Expect::BootClock => BENCHMARK_BOOT_TIMEOUT,
        _ => BOOT_TIMEOUT,
    };
    let started = Instant::now();
    let mut console = Vec::new();
    let mut typed = [0; CONSOLE_SCRIPTS.len()];
    let mut shell_typed = ShellTyping::default();
    let mut first_prompt = None;
    let mut terminal_frames = Vec::new();
    let mut input_events_sent = 0;
    let mut qmp_errors = Vec::new();
    let mut keyboard_typing: Option<KeyboardTyping> = None;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        console.extend(rx.try_iter().flatten());
        if expect == Expect::BootClock && boot_clock_line(&String::from_utf8_lossy(&console)) {
            let _ = child.kill();
            break child.wait().map_err(|e| e.to_string())?;
        }
        let scripts = match expect {
            Expect::Init | Expect::InitRestart | Expect::BootClock => Vec::new(),
            Expect::Shell => shell_scripts_to_type(
                &String::from_utf8_lossy(&console),
                &SHELL_SESSION,
                &mut shell_typed,
            ),
            Expect::LinuxPrograms => shell_scripts_to_type(
                &String::from_utf8_lossy(&console),
                &LINUX_PROGRAM_SESSION,
                &mut shell_typed,
            ),
            Expect::HostFolder => shell_scripts_to_type(
                &String::from_utf8_lossy(&console),
                &HOST_FOLDER_SESSION,
                &mut shell_typed,
            ),
            Expect::ProgramWindows => {
                let text = String::from_utf8_lossy(&console).into_owned();
                let both_shown = WINDOW_TITLES
                    .iter()
                    .all(|title| has_line(&text, &format!("hello-window {title}: shown")));
                if let Some(port) = qmp_port
                    && both_shown
                    && terminal_frames.is_empty()
                {
                    let checked = check_program_windows(arch, port, &mut console, &rx);
                    if let Err(error) = checked {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error);
                    }
                    terminal_frames.push(Ok(()));
                }
                shell_scripts_to_type(&text, &PROGRAM_WINDOW_SESSION, &mut shell_typed)
            }
            Expect::WindowsPrograms => shell_scripts_to_type(
                &String::from_utf8_lossy(&console),
                &WINDOWS_PROGRAM_SESSION,
                &mut shell_typed,
            ),
            Expect::WindowsGui => {
                if let Some(port) = qmp_port
                    && shell_typed.lines >= 1
                    && terminal_frames.is_empty()
                    && runs_windows_programs(arch.name)
                {
                    let checked = windows_gui::check(arch, port, &mut console, &rx);
                    if let Err(error) = checked {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error);
                    }
                    terminal_frames.push(Ok(()));
                }
                shell_scripts_to_type(
                    &String::from_utf8_lossy(&console),
                    &WINDOWS_GUI_SESSION,
                    &mut shell_typed,
                )
            }
            Expect::WindowsCrt => {
                if let Some(port) = qmp_port
                    && shell_typed.lines >= 1
                    && terminal_frames.is_empty()
                {
                    let checked = windows_crt::check(arch, port, &mut console, &rx);
                    if let Err(error) = checked {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error);
                    }
                    terminal_frames.push(Ok(()));
                }
                shell_scripts_to_type(
                    &String::from_utf8_lossy(&console),
                    &WINDOWS_CRT_SESSION,
                    &mut shell_typed,
                )
            }
            Expect::LinuxGui => {
                if let Some(port) = qmp_port
                    && shell_typed.lines >= 1
                    && terminal_frames.is_empty()
                {
                    let checked = linux_gui::check(arch, port, &mut console, &rx);
                    if let Err(error) = checked {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error);
                    }
                    terminal_frames.push(Ok(()));
                }
                shell_scripts_to_type(
                    &String::from_utf8_lossy(&console),
                    &LINUX_GUI_SESSION,
                    &mut shell_typed,
                )
            }
            Expect::AndroidPrograms => shell_scripts_to_type(
                &String::from_utf8_lossy(&console),
                &ANDROID_PROGRAM_SESSION,
                &mut shell_typed,
            ),
            Expect::MacosPrograms => shell_scripts_to_type(
                &String::from_utf8_lossy(&console),
                &MACOS_PROGRAM_SESSION,
                &mut shell_typed,
            ),
            Expect::BootToPrompt => {
                let prompts = String::from_utf8_lossy(&console)
                    .matches(SHELL_PROMPT)
                    .count();
                if first_prompt.is_none() && prompts > 0 {
                    first_prompt = Some(started.elapsed());
                }
                let at = match prompts {
                    1 => Some("the first prompt"),
                    PROMPT_SESSION_FRAME_PROMPT => {
                        Some("the output of the line typed on the virtio keyboard")
                    }
                    _ => None,
                };
                if let (Some(port), Some(at)) = (qmp_port, at)
                    && terminal_frames.len() < prompts.min(2)
                {
                    let with_pointer = prompts == PROMPT_SESSION_FRAME_PROMPT;
                    terminal_frames.push(check_terminal_frame(
                        arch,
                        port,
                        &mut console,
                        &rx,
                        at,
                        with_pointer,
                    ));
                }
                let text = String::from_utf8_lossy(&console);
                shell_scripts_to_type(&text, &PROMPT_SESSION, &mut shell_typed)
            }
            _ => scripts_to_type(&String::from_utf8_lossy(&console), &mut typed),
        };
        for script in scripts {
            if qmp_port.is_some() && KEYBOARD_LINES.contains(&script) {
                keyboard_typing = Some(KeyboardTyping::new(script, console.len()));
                continue;
            }
            if let Err(e) = keyboard.write_all(script).and_then(|()| keyboard.flush()) {
                eprintln!("xtask: {}: could not type on the console: {e}", arch.name);
            }
        }
        if let (Some(port), Some(typing)) = (qmp_port, keyboard_typing.as_mut())
            && let Some(keys) = typing.next_keys(&console)
        {
            if let Err(e) = Qmp::connect(port).and_then(|mut qmp| qmp.type_text(&keys, KEY_PAUSE)) {
                qmp_errors.push(format!("{}: could not type through QMP: {e}", arch.name));
            }
            if typing.is_done() {
                eprintln!(
                    "xtask: {}: typed `{}` on the virtio keyboard",
                    arch.name,
                    String::from_utf8_lossy(&typing.text).trim_end()
                );
                keyboard_typing = None;
            }
        }
        if let Some(port) = qmp_port {
            let asked = count_lines(&String::from_utf8_lossy(&console), INPUT_EVENTS_LINE);
            while input_events_sent < asked {
                input_events_sent += 1;
                if let Err(e) = send_pointer_and_key_events(port) {
                    qmp_errors.push(format!("{}: could not send input events: {e}", arch.name));
                }
            }
        }
        if banner_count(&String::from_utf8_lossy(&console)) > 1 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: the machine reset and booted again (crash or triple fault?)",
                arch.name
            ));
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: no power-off within {}s (kernel hung?)",
                arch.name,
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let rest_until_qemu_closes_stdout = rx.iter().flatten();
    console.extend(rest_until_qemu_closes_stdout);
    let console = String::from_utf8_lossy(&console).into_owned();
    verdict(arch.name, expect, status.success(), &console)?;
    if let Some(error) = qmp_errors.into_iter().next() {
        return Err(error);
    }
    for frame in terminal_frames {
        frame?;
    }
    if let Some(first_prompt) = first_prompt {
        eprintln!(
            "xtask: {}: boot to prompt in {} ms (QEMU started to the first `{SHELL_PROMPT}` prompt, {cpus} CPUs)",
            arch.name,
            first_prompt.as_millis()
        );
    }
    Ok(Booted {
        console,
        first_prompt,
    })
}

fn boot_clock_line(console: &str) -> bool {
    console
        .split_inclusive('\n')
        .any(|line| line.ends_with('\n') && line.starts_with(BOOT_CLOCK_LINE))
}

fn count_lines(console: &str, wanted: &str) -> usize {
    console
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n') && line.trim_end() == wanted)
        .count()
}

fn input_self_test_reports() -> [Vec<InputEvent>; 5] {
    let button = |button, down| InputEvent::Button { button, down };
    let shift = |down| InputEvent::Key {
        qcode: "shift",
        down,
    };
    [
        vec![
            InputEvent::Absolute {
                axis: "x",
                value: TABLET_POSITION.0,
            },
            InputEvent::Absolute {
                axis: "y",
                value: TABLET_POSITION.1,
            },
        ],
        vec![button("left", true), button("left", false)],
        vec![
            InputEvent::Relative {
                axis: "x",
                value: MOUSE_MOTION.0,
            },
            InputEvent::Relative {
                axis: "y",
                value: MOUSE_MOTION.1,
            },
        ],
        vec![button("wheel-up", true), button("wheel-up", false)],
        vec![shift(true), shift(false)],
    ]
}

fn send_pointer_and_key_events(port: u16) -> Result<(), String> {
    let mut qmp = Qmp::connect(port)?;
    for report in input_self_test_reports() {
        qmp.send_input(&report)?;
        std::thread::sleep(DEVICE_PAUSE);
    }
    Ok(())
}

fn kill_process_tree(child: &mut std::process::Child) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn echo_console_to_stderr(mut stdout: std::process::ChildStdout) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = stdout.read(&mut buf) {
            let _ = std::io::stderr().write_all(&buf[..n]);
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    rx
}

fn scripts_to_type(
    console: &str,
    typed: &mut [usize; CONSOLE_SCRIPTS.len()],
) -> Vec<&'static [u8]> {
    let mut scripts = Vec::new();
    for ((line, script), typed) in CONSOLE_SCRIPTS.iter().zip(typed) {
        let asked = console
            .split_inclusive('\n')
            .filter(|l| l.ends_with('\n') && l.trim_end() == *line)
            .count();
        while *typed < asked {
            scripts.push(*script);
            *typed += 1;
        }
    }
    scripts
}

fn shell_scripts_to_type(
    console: &str,
    session: &[&'static [u8]],
    typed: &mut ShellTyping,
) -> Vec<&'static [u8]> {
    let mut scripts = Vec::new();
    let prompts = console.matches(SHELL_PROMPT).count().min(session.len());
    while typed.lines < prompts {
        scripts.push(session[typed.lines]);
        typed.lines += 1;
    }
    let (text, interrupt) = SHELL_INTERRUPT;
    let asked = console.matches(text).count();
    while typed.interrupts < asked {
        scripts.push(interrupt);
        typed.interrupts += 1;
    }
    for (reaction, done) in REACTIONS.iter().zip(&mut typed.reactions) {
        let seen = console
            .split_inclusive('\n')
            .filter(|line| {
                line.ends_with('\n') && line.trim_end().ends_with(reaction.line_ends_with)
            })
            .count();
        if !*done && seen >= reaction.occurrences {
            scripts.push(reaction.keys);
            *done = true;
        }
    }
    scripts
}

fn banner_count(console: &str) -> usize {
    console
        .lines()
        .filter(|l| l.starts_with(BANNER_PREFIX) && l.trim_end().ends_with(')'))
        .count()
}

fn verdict(arch: &str, expect: Expect, clean_exit: bool, console: &str) -> Result<(), String> {
    let lines = || console.lines().map(str::trim_end);
    let passed = lines().any(|l| l == PASS_MARKER);
    let failed = lines().any(|l| l == FAIL_MARKER);
    if banner_count(console) > 1 {
        return Err(format!(
            "{arch}: the machine reset and booted again (crash or triple fault?)"
        ));
    }
    match expect {
        Expect::Init => return init_verdict(arch, clean_exit, console, POWER_DOWN_LINE),
        Expect::InitRestart => return init_verdict(arch, clean_exit, console, RESTART_LINE),
        Expect::Shell => return shell_verdict(arch, clean_exit, console),
        Expect::BootToPrompt => return prompt_verdict(arch, clean_exit, console),
        Expect::LinuxPrograms => {
            return session_verdict(
                arch,
                clean_exit,
                console,
                &LINUX_PROGRAM_SESSION,
                linux_program_output(),
                "Linux programs",
            );
        }
        Expect::HostFolder => {
            if !lines().any(|line| line == HOST_FOLDER_MOUNT_LINE) {
                return Err(format!("{arch}: no `{HOST_FOLDER_MOUNT_LINE}`"));
            }
            return session_verdict(
                arch,
                clean_exit,
                console,
                &HOST_FOLDER_SESSION,
                host_folder_output(),
                "host folder",
            );
        }
        Expect::ProgramWindows => {
            return session_verdict(
                arch,
                clean_exit,
                console,
                &PROGRAM_WINDOW_SESSION,
                program_window_output(),
                "program windows",
            );
        }
        Expect::WindowsPrograms => {
            return session_verdict(
                arch,
                clean_exit,
                console,
                &WINDOWS_PROGRAM_SESSION,
                windows_program_output(arch),
                "Windows programs",
            );
        }
        Expect::WindowsGui => {
            return session_verdict(
                arch,
                clean_exit,
                console,
                &WINDOWS_GUI_SESSION,
                windows_gui_output(arch),
                "Windows GUI",
            );
        }
        Expect::WindowsCrt => {
            return session_verdict(
                arch,
                clean_exit,
                console,
                &WINDOWS_CRT_SESSION,
                windows_crt_output(),
                "Windows C runtime",
            );
        }
        Expect::LinuxGui => {
            return session_verdict(
                arch,
                clean_exit,
                console,
                &LINUX_GUI_SESSION,
                linux_gui_output(),
                "Linux GUI",
            );
        }
        Expect::AndroidPrograms => {
            return session_verdict(
                arch,
                clean_exit,
                console,
                &ANDROID_PROGRAM_SESSION,
                android_program_output(),
                "Android programs",
            );
        }
        Expect::MacosPrograms => {
            return session_verdict(
                arch,
                clean_exit,
                console,
                &MACOS_PROGRAM_SESSION,
                macos_program_output(arch),
                "macOS programs",
            );
        }
        Expect::BootClock => return boot_clock_verdict(arch, console),
        _ => {}
    }
    if expect == Expect::FatalException {
        let target = lines().find_map(|l| l.strip_prefix(WRITE_PROTECT_PROBE));
        let fault = target.map(|target| format!(", address {target}"));
        let reported = lines()
            .skip_while(|l| {
                !(l.starts_with(EXCEPTION_PREFIX)
                    && fault
                        .as_ref()
                        .is_some_and(|fault| l.ends_with(fault.as_str())))
            })
            .skip(1)
            .any(|l| l == FAIL_MARKER);
        return match (clean_exit, passed, reported) {
            (true, true, true) => {
                eprintln!("xtask: {arch}: fatal exception test passed");
                Ok(())
            }
            (false, _, _) => Err(format!("{arch}: QEMU exited with an error")),
            (true, false, _) => Err(format!(
                "{arch}: the self-tests did not pass before the fatal exception test"
            )),
            (true, true, false) => Err(format!(
                "{arch}: the write to read-only data was not reported as a fatal exception at its address"
            )),
        };
    }
    let interactive = expect == Expect::Pass;
    let user_lines = USER_LINES
        .iter()
        .chain(interactive.then_some(&CONSOLE_PROGRAM_LINE))
        .all(|&user_line| lines().any(|l| l == user_line));
    let echoed = !interactive || console.contains(LINES_ECHO);
    match (clean_exit, passed, failed) {
        (true, true, false) if user_lines && echoed => {
            eprintln!("xtask: {arch}: boot test passed");
            Ok(())
        }
        (true, true, false) if user_lines => Err(format!(
            "{arch}: the console did not echo the lines typed as its line discipline should"
        )),
        (true, true, false) => Err(format!(
            "{arch}: a Linux test program's line did not reach the console"
        )),
        (_, _, true) => Err(format!(
            "{arch}: kernel reported a self-test failure or panic"
        )),
        (false, _, _) => Err(format!("{arch}: QEMU exited with an error")),
        (true, false, false) => Err(format!("{arch}: kernel powered off without a verdict")),
    }
}

fn init_verdict(
    arch: &str,
    clean_exit: bool,
    console: &str,
    going_down: &str,
) -> Result<(), String> {
    let lines = || console.lines().map(str::trim_end);
    let has = |wanted: &str| lines().any(|line| line == wanted);
    if let Some(panic) = lines().find(|line| line.starts_with(KERNEL_PANIC_PREFIX)) {
        return Err(format!("{arch}: init ended: {panic}"));
    }
    if !clean_exit {
        return Err(format!("{arch}: QEMU exited with an error"));
    }
    if !has(INIT_BANNER) {
        return Err(format!(
            "{arch}: init did not write its banner `{INIT_BANNER}`"
        ));
    }
    if let Some(missing) = INIT_LOGIN_LINES.iter().find(|line| !has(line)) {
        return Err(format!(
            "{arch}: the login program did not write `{missing}`"
        ));
    }
    if !has(going_down) {
        return Err(format!(
            "{arch}: init did not take the machine down: no `{going_down}`"
        ));
    }
    eprintln!("xtask: {arch}: init boot test passed ({going_down})");
    Ok(())
}

fn shell_verdict(arch: &str, clean_exit: bool, console: &str) -> Result<(), String> {
    let lines = || console.lines().map(str::trim_end);
    if let Some(panic) = lines().find(|line| line.starts_with(KERNEL_PANIC_PREFIX)) {
        return Err(format!("{arch}: init ended: {panic}"));
    }
    if !clean_exit {
        return Err(format!("{arch}: QEMU exited with an error"));
    }
    let shown = |wanted: &str| lines().any(|line| line.ends_with(wanted));
    if let Some(missing) = SHELL_LINES.iter().find(|line| !shown(line)) {
        return Err(format!(
            "{arch}: the shell session did not write `{missing}`"
        ));
    }
    if !shown(SHELL_EXIT_LINE) || !lines().any(|line| line == POWER_DOWN_LINE) {
        return Err(format!(
            "{arch}: no `{SHELL_EXIT_LINE}` and `{POWER_DOWN_LINE}` after the shell session"
        ));
    }
    eprintln!("xtask: {arch}: shell boot test passed");
    Ok(())
}

fn session_verdict<const N: usize>(
    arch: &str,
    clean_exit: bool,
    console: &str,
    session: &[&[u8]; N],
    outputs: [Vec<Line>; N],
    name: &str,
) -> Result<(), String> {
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    if let Some(panic) = lines
        .iter()
        .find(|line| line.starts_with(KERNEL_PANIC_PREFIX))
    {
        return Err(format!("{arch}: init ended: {panic}"));
    }
    if !clean_exit {
        return Err(format!("{arch}: QEMU exited with an error"));
    }
    for (typed, expected) in session.iter().zip(outputs) {
        let command = String::from_utf8_lossy(typed);
        let command = command.trim_end_matches('\r');
        let Some(echoed) = lines
            .iter()
            .position(|line| line.starts_with(SHELL_PROMPT) && line.ends_with(command))
        else {
            return Err(format!("{arch}: `{command}` was not typed at a prompt"));
        };
        let output = lines[echoed + 1..]
            .iter()
            .take_while(|line| !line.starts_with(SHELL_PROMPT));
        if let Some(missing) = missing_in_order(output, &expected) {
            return Err(format!(
                "{arch}: `{command}` did not write {missing:?} in order before the next prompt"
            ));
        }
    }
    eprintln!("xtask: {arch}: {name} boot test passed");
    Ok(())
}

fn missing_in_order<'a>(
    mut output: impl Iterator<Item = &'a &'a str>,
    expected: &[Line],
) -> Option<Line> {
    expected
        .iter()
        .find(|wanted| !output.any(|line| wanted.matches(line)))
        .cloned()
}

fn prompt_verdict(arch: &str, clean_exit: bool, console: &str) -> Result<(), String> {
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    if let Some(panic) = lines
        .iter()
        .find(|line| line.starts_with(KERNEL_PANIC_PREFIX))
    {
        return Err(format!("{arch}: init ended: {panic}"));
    }
    if !clean_exit {
        return Err(format!("{arch}: QEMU exited with an error"));
    }
    if !lines.contains(&TERMINAL_LINE_1280X800) {
        return Err(format!(
            "{arch}: no `{TERMINAL_LINE_1280X800}`: the graphical terminal did not start"
        ));
    }
    if !lines.contains(&BOOT_INPUT_LINE) {
        return Err(format!(
            "{arch}: no `{BOOT_INPUT_LINE}`: the virtio keyboard and pointers were not found"
        ));
    }
    if !lines.contains(&DISPLAY_LINE_1280X800) {
        return Err(format!(
            "{arch}: no `{DISPLAY_LINE_1280X800}`: the virtio GPU was not shown"
        ));
    }
    if let Some(line) = lines
        .iter()
        .find(|line| line.starts_with(OFFSCREEN_LINE_PREFIX))
    {
        return Err(format!(
            "{arch}: `{line}` with a virtio GPU: the GPU must win over eggos.display"
        ));
    }
    let mut from = 0;
    for (index, expected) in prompt_session_output(arch).into_iter().enumerate() {
        let command = shown_command(&PROMPT_SESSION, index);
        let command = command.as_str();
        let Some(echoed) = lines[from..]
            .iter()
            .position(|line| line.starts_with(SHELL_PROMPT) && line.ends_with(command))
            .map(|at| from + at)
        else {
            return Err(format!("{arch}: `{command}` was not typed at a prompt"));
        };
        from = echoed + 1;
        let written = lines.get(echoed + 1..echoed + 1 + expected.len());
        let matched = written.is_some_and(|written| {
            expected
                .iter()
                .zip(written)
                .all(|(expected, line)| expected.matches(line))
        });
        if !matched {
            return Err(format!(
                "{arch}: `{command}` did not write {expected:?} but {:?}",
                written.unwrap_or(&lines[echoed + 1..])
            ));
        }
    }
    eprintln!("xtask: {arch}: boot-to-prompt test passed");
    Ok(())
}

fn boot_clock_verdict(arch: &str, console: &str) -> Result<(), String> {
    if let Some(panic) = console
        .lines()
        .find(|line| line.starts_with(KERNEL_PANIC_PREFIX))
    {
        return Err(format!("{arch}: init ended: {panic}"));
    }
    if !boot_clock_line(console) {
        return Err(format!(
            "{arch}: the kernel wrote no `{}` line",
            BOOT_CLOCK_LINE.trim_end()
        ));
    }
    eprintln!("xtask: {arch}: boot clock read");
    Ok(())
}

fn terminal_stream(console: &[u8]) -> Option<&[u8]> {
    let prefix = TERMINAL_LINE_PREFIX.as_bytes();
    let start = console
        .windows(prefix.len() + 1)
        .position(|window| window[0] == b'\n' && &window[1..] == prefix)?
        + 1;
    let end = start + console[start..].iter().position(|&byte| byte == b'\n')?;
    Some(&console[end + 1..])
}

/// What a program's window shows in the host's render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowContent {
    /// `hello-window`'s checkerboard of 16 px cells in two colours.
    Checkerboard([u32; 2]),
    /// The pixels of a content `width` wide.
    Pixels { width: u32, pixels: Vec<u32> },
}

struct HostContents<'a> {
    terminal: WindowId,
    grid: &'a [u32],
    windows: &'a [(WindowId, WindowContent)],
}

impl Contents for HostContents<'_> {
    fn copy_row(&self, window: WindowId, x: u32, y: u32, out: &mut [u32]) {
        if window == self.terminal {
            let start = y as usize * GRID_SIZE.0 as usize + x as usize;
            if let Some(row) = self.grid.get(start..start + out.len()) {
                out.copy_from_slice(row);
            }
            return;
        }
        let content = self
            .windows
            .iter()
            .find(|(id, _)| *id == window)
            .map(|(_, content)| content);
        for (index, pixel) in out.iter_mut().enumerate() {
            let column = x + index as u32;
            *pixel = match content {
                Some(WindowContent::Checkerboard(colours)) => window_pattern(*colours, column, y),
                Some(WindowContent::Pixels { width, pixels }) if column < *width => pixels
                    .get(y as usize * *width as usize + column as usize)
                    .copied()
                    .unwrap_or(colour::WINDOW_BACKGROUND),
                _ => colour::WINDOW_BACKGROUND,
            };
        }
    }
}

fn window_colours(title: &str) -> [u32; 2] {
    match title {
        "first" => [0x00f2_b33d, 0x000b_0e13],
        _ => [0x004f_8fe0, 0x00e6_eaf0],
    }
}

fn window_pattern(colours: [u32; 2], x: u32, y: u32) -> u32 {
    colours[((x / 16 + y / 16) % 2) as usize]
}

/// What a `width` by `height` display shows once the release kernel's desktop is in
/// `desktop`'s state with its clock at `up 0:00` and its terminal was fed `stream`, rendered
/// by the same desktop and terminal engine on the host; `windows` are the programs' windows
/// and what each shows.
pub fn desktop_picture(
    desktop: &Desktop,
    stream: &[u8],
    windows: &[(WindowId, WindowContent)],
) -> Option<Picture> {
    let screen = desktop.screen();
    let (width, height) = (screen.width, screen.height);
    let area = desktop.text_area();
    let mut terminal = Terminal::new(area.columns, area.rows)?;
    terminal.set_onlcr(true);
    terminal.feed(stream);
    let mut grid =
        vec![eggos_drivers::terminal::TERM_BACKGROUND; GRID_SIZE.0 as usize * GRID_SIZE.1 as usize];
    terminal.render(&mut grid, GRID_SIZE.0 as usize, 0, 0, &mut Damage::new());
    let contents = HostContents {
        terminal: desktop.terminal_window(),
        grid: &grid,
        windows,
    };
    let mut pixels = vec![0; width as usize * height as usize];
    desktop.draw(&mut pixels, width as usize, screen, &contents);
    Some(Picture {
        width,
        height,
        pixels,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClockFace {
    Uptime(u64),
    WallClock(u64),
}

const WALL_CLOCK_MINUTES_ACCEPTED: std::ops::RangeInclusive<i64> = -5..=1;

/// The seconds since the epoch on the host's clock, which QEMU's real-time clocks follow
/// (`-rtc base=utc`, D150).
pub fn host_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn clock_faces(host: u64) -> impl Iterator<Item = ClockFace> {
    (0..CLOCK_SECONDS_ACCEPTED).map(ClockFace::Uptime).chain(
        WALL_CLOCK_MINUTES_ACCEPTED
            .map(move |minutes| ClockFace::WallClock(host.saturating_add_signed(minutes * 60))),
    )
}

fn with_clock(desktop: &Desktop, picture: &mut Picture, face: ClockFace) {
    let mut desktop_then = Desktop::new(picture.width, picture.height).expect("the desktop fits");
    desktop_then.set_profile(desktop.profile(), &mut Damage::new(), &mut DiscardEvents);
    match face {
        ClockFace::Uptime(seconds) => desktop_then.set_uptime(seconds, &mut Damage::new()),
        ClockFace::WallClock(seconds) => desktop_then.set_wall_clock(seconds, &mut Damage::new()),
    };
    let clock = desktop.clock_rect();
    let contents = HostContents {
        terminal: desktop_then.terminal_window(),
        grid: &[],
        windows: &[],
    };
    desktop_then.draw(
        &mut picture.pixels,
        picture.width as usize,
        clock,
        &contents,
    );
}

const TABLET_RANGE: AxisRange = AxisRange { min: 0, max: 32767 };

fn tablet_value(pixel: u32, size: u32) -> i32 {
    let span = TABLET_RANGE.max as u64;
    ((u64::from(pixel) * span + u64::from(size) / 2) / u64::from(size)) as i32
}

#[derive(Debug, Clone, Copy)]
struct PointerStep {
    x: u32,
    y: u32,
    button: Option<bool>,
}

fn pointer_steps(desktop: &Desktop) -> Vec<(&'static str, Vec<PointerStep>)> {
    let minimise = desktop.minimise_control();
    let chip = desktop
        .window_entry(desktop.terminal_window())
        .unwrap_or(desktop.chip());
    let title = desktop.title_bar();
    let step = |x, y, button| PointerStep { x, y, button };
    let (grab_x, grab_y) = (title.x + title.width / 2, title.y + title.height / 2);
    vec![
        (
            "the window minimised to the window list",
            vec![
                step(minimise.x + 10, minimise.y + 14, Some(true)),
                step(minimise.x + 10, minimise.y + 14, Some(false)),
            ],
        ),
        (
            "the window restored from the window list",
            vec![
                step(chip.x + 10, chip.y + 10, Some(true)),
                step(chip.x + 10, chip.y + 10, Some(false)),
            ],
        ),
        (
            "the window dragged 100 px right and down",
            vec![
                step(grab_x, grab_y, Some(true)),
                step(grab_x + 50, grab_y + 50, None),
                step(grab_x + 100, grab_y + 100, None),
                step(grab_x + 100, grab_y + 100, Some(false)),
            ],
        ),
    ]
}

fn send_pointer_step(qmp: &mut Qmp, step: PointerStep, screen: (u32, u32)) -> Result<(), String> {
    qmp.send_input(&[
        InputEvent::Absolute {
            axis: "x",
            value: tablet_value(step.x, screen.0),
        },
        InputEvent::Absolute {
            axis: "y",
            value: tablet_value(step.y, screen.1),
        },
    ])?;
    std::thread::sleep(DEVICE_PAUSE);
    if let Some(down) = step.button {
        qmp.send_input(&[InputEvent::Button {
            button: "left",
            down,
        }])?;
        std::thread::sleep(DEVICE_PAUSE);
    }
    Ok(())
}

fn apply_pointer_step(
    desktop: &mut Desktop,
    step: PointerStep,
) -> Option<eggos_drivers::desktop::Action> {
    let screen = desktop.screen();
    let mut damage = Damage::new();
    let events = &mut DiscardEvents;
    desktop.pointer_moved(
        scale_to_screen(
            tablet_value(step.x, screen.width),
            TABLET_RANGE,
            screen.width,
        ),
        scale_to_screen(
            tablet_value(step.y, screen.height),
            TABLET_RANGE,
            screen.height,
        ),
        &mut damage,
        events,
    );
    step.button
        .map(|pressed| desktop.left_button(pressed, &mut damage, events))
}

fn check_terminal_frame(
    arch: &Arch,
    port: u16,
    console: &mut Vec<u8>,
    output: &mpsc::Receiver<Vec<u8>>,
    at: &str,
    with_pointer: bool,
) -> Result<(), String> {
    let mut qmp = Qmp::connect(port)?;
    let (width, height) = screen_size(&mut qmp, arch)?;
    let mut desktop = Desktop::new(width, height)
        .ok_or_else(|| format!("{}: a {width}x{height} display holds no desktop", arch.name))?;
    check_desktop_frame(arch, &mut qmp, console, output, at, &desktop, &[])?;
    if !with_pointer {
        return Ok(());
    }
    for (state, steps) in pointer_steps(&desktop) {
        for step in steps {
            send_pointer_step(&mut qmp, step, (width, height))?;
            apply_pointer_step(&mut desktop, step);
        }
        check_desktop_frame(arch, &mut qmp, console, output, state, &desktop, &[])?;
    }
    Ok(())
}

const CLOSE_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

fn has_line(console: &str, wanted: &str) -> bool {
    console
        .split_inclusive('\n')
        .any(|line| line.ends_with('\n') && line.trim_end() == wanted)
}

fn click_steps(x: u32, y: u32) -> [PointerStep; 2] {
    [
        PointerStep {
            x,
            y,
            button: Some(true),
        },
        PointerStep {
            x,
            y,
            button: Some(false),
        },
    ]
}

fn check_program_windows(
    arch: &Arch,
    port: u16,
    console: &mut Vec<u8>,
    output: &mpsc::Receiver<Vec<u8>>,
) -> Result<(), String> {
    let mut qmp = Qmp::connect(port)?;
    let (width, height) = screen_size(&mut qmp, arch)?;
    let mut desktop = Desktop::new(width, height)
        .ok_or_else(|| format!("{}: a {width}x{height} display holds no desktop", arch.name))?;
    let mut windows = Vec::new();
    let refused = |error| {
        format!(
            "{}: the host's desktop refused a window: {error:?}",
            arch.name
        )
    };
    for (client, title) in WINDOW_TITLES.iter().enumerate() {
        let client = client as u16;
        let (content_width, content_height) = WINDOW_CONTENT;
        let created = desktop
            .create(client, 0, 0, content_width, content_height, 0)
            .map_err(refused)?;
        let id = u32::from(created.window);
        desktop
            .set_title(client, id, title.as_bytes(), &mut Damage::new())
            .map_err(refused)?;
        desktop
            .commit(client, id, &[], &mut Damage::new(), &mut DiscardEvents)
            .map_err(refused)?;
        windows.push((
            created.window,
            WindowContent::Checkerboard(window_colours(title)),
        ));
    }
    let (first, second) = (windows[0].0, windows[1].0);
    let screen = (width, height);
    let check = |qmp: &mut Qmp, desktop: &Desktop, console: &mut Vec<u8>, at: &str| {
        check_desktop_frame(arch, qmp, console, output, at, desktop, &windows)
    };
    check(
        &mut qmp,
        &desktop,
        console,
        "two programs' windows cascaded",
    )?;
    let rect = |desktop: &Desktop, id: WindowId| {
        desktop
            .window_rect(id)
            .ok_or_else(|| format!("{}: the window {id} is gone", arch.name))
    };
    let content = desktop.content_rect(first).unwrap_or_default();
    for step in click_steps(content.x + 8, content.bottom() - 40) {
        send_pointer_step(&mut qmp, step, screen)?;
        apply_pointer_step(&mut desktop, step);
    }
    if desktop.stacking().last() != Some(first) {
        return Err(format!(
            "{}: the click did not raise the first window",
            arch.name
        ));
    }
    check(
        &mut qmp,
        &desktop,
        console,
        "the first window raised by a click",
    )?;
    let before: Rect = rect(&desktop, first)?;
    let (grab_x, grab_y) = (before.x + before.width / 2, before.y + 14);
    for (x, y, button) in [
        (grab_x, grab_y, Some(true)),
        (grab_x + 30, grab_y + 20, None),
        (grab_x + 60, grab_y + 40, None),
        (grab_x + 60, grab_y + 40, Some(false)),
    ] {
        let step = PointerStep { x, y, button };
        send_pointer_step(&mut qmp, step, screen)?;
        apply_pointer_step(&mut desktop, step);
    }
    let after = rect(&desktop, first)?;
    if (after.x, after.y) != (before.x + 60, before.y + 40) {
        return Err(format!(
            "{}: the drag did not move the first window",
            arch.name
        ));
    }
    check(
        &mut qmp,
        &desktop,
        console,
        "the first window dragged by its title bar",
    )?;
    let mut windows_test = WindowSteps {
        arch,
        qmp: &mut qmp,
        desktop: &mut desktop,
        console,
        output,
        windows: &windows,
        screen,
        now_ms: 0,
        held: Modifiers::NONE,
    };
    windows_test.manage(first, second)
}

struct WindowSteps<'a> {
    arch: &'a Arch,
    qmp: &'a mut Qmp,
    desktop: &'a mut Desktop,
    console: &'a mut Vec<u8>,
    output: &'a mpsc::Receiver<Vec<u8>>,
    windows: &'a [(WindowId, WindowContent)],
    screen: (u32, u32),
    now_ms: u32,
    held: Modifiers,
}

const DOUBLE_CLICK_PAUSE: Duration = Duration::from_millis(40);

impl WindowSteps<'_> {
    fn check(&mut self, at: &str) -> Result<(), String> {
        check_desktop_frame(
            self.arch,
            self.qmp,
            self.console,
            self.output,
            at,
            self.desktop,
            self.windows,
        )
    }

    fn pointer(&mut self, x: u32, y: u32, button: Option<bool>) -> Result<(), String> {
        let step = PointerStep { x, y, button };
        send_pointer_step(self.qmp, step, self.screen)?;
        self.now_ms += 1000;
        self.desktop.set_time(self.now_ms);
        apply_pointer_step(self.desktop, step);
        Ok(())
    }

    fn click(&mut self, x: u32, y: u32) -> Result<(), String> {
        self.pointer(x, y, Some(true))?;
        self.pointer(x, y, Some(false))
    }

    fn double_click(&mut self, x: u32, y: u32) -> Result<(), String> {
        let (tablet_x, tablet_y) = (
            tablet_value(x, self.screen.0),
            tablet_value(y, self.screen.1),
        );
        self.qmp.send_input(&[
            InputEvent::Absolute {
                axis: "x",
                value: tablet_x,
            },
            InputEvent::Absolute {
                axis: "y",
                value: tablet_y,
            },
        ])?;
        std::thread::sleep(DEVICE_PAUSE);
        for down in [true, false, true, false] {
            self.qmp.send_input(&[InputEvent::Button {
                button: "left",
                down,
            }])?;
            std::thread::sleep(DOUBLE_CLICK_PAUSE);
        }
        std::thread::sleep(DEVICE_PAUSE);
        self.now_ms += 1000;
        for (offset, down) in [(0, true), (40, false), (80, true), (120, false)] {
            self.desktop.set_time(self.now_ms + offset);
            apply_pointer_step(
                self.desktop,
                PointerStep {
                    x,
                    y,
                    button: Some(down),
                },
            );
        }
        Ok(())
    }

    fn key(&mut self, qcode: &'static str, code: u16, down: bool) -> Result<(), String> {
        self.qmp.send_input(&[InputEvent::Key { qcode, down }])?;
        std::thread::sleep(DEVICE_PAUSE);
        let modifier = match code {
            key::LEFT_ALT => Modifiers::ALT,
            key::LEFT_SHIFT => Modifiers::SHIFT,
            _ => Modifiers::NONE,
        };
        self.held = match down {
            true => self.held.with(modifier),
            false => Modifiers::from_bits(self.held.bits() & !modifier.bits()),
        };
        self.now_ms += 100;
        self.desktop.set_time(self.now_ms);
        self.desktop.key(
            code,
            down,
            self.held,
            &mut Damage::new(),
            &mut DiscardEvents,
        );
        Ok(())
    }

    fn tap(&mut self, qcode: &'static str, code: u16) -> Result<(), String> {
        self.key(qcode, code, true)?;
        self.key(qcode, code, false)
    }

    fn wait_for(&mut self, wanted: &str, count: usize) -> Result<(), String> {
        let started = Instant::now();
        loop {
            self.console.extend(self.output.try_iter().flatten());
            if count_lines(&String::from_utf8_lossy(self.console), wanted) >= count {
                return Ok(());
            }
            if started.elapsed() > CLOSE_REQUEST_TIMEOUT {
                return Err(format!("{}: no {count} `{wanted}` lines", self.arch.name));
            }
            std::thread::sleep(TERMINAL_FRAME_RETRY);
        }
    }

    fn content(&self, id: WindowId) -> Result<Rect, String> {
        self.desktop
            .content_rect(id)
            .ok_or_else(|| format!("{}: the window {id} is gone", self.arch.name))
    }

    fn answer_resize(&mut self, client: u16, id: WindowId, resizes: usize) -> Result<(), String> {
        let content = self.content(id)?;
        self.wait_for(
            &format!(
                "hello-window first: resized {}x{}",
                content.width, content.height
            ),
            resizes,
        )?;
        let refused = |error| format!("{}: the host refused a resize: {error:?}", self.arch.name);
        self.desktop
            .set_size(
                client,
                u32::from(id),
                content.width,
                content.height,
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(refused)?;
        self.desktop
            .commit(
                client,
                u32::from(id),
                &[],
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(refused)
    }

    fn expect(&self, holds: bool, what: &str) -> Result<(), String> {
        match holds {
            true => Ok(()),
            false => Err(format!("{}: {what}", self.arch.name)),
        }
    }

    fn manage(&mut self, first: WindowId, second: WindowId) -> Result<(), String> {
        let outside = self.desktop.window_rect(first).unwrap_or_default();
        let (corner_x, corner_y) = (outside.right() - 2, outside.bottom() - 2);
        self.pointer(corner_x, corner_y, None)?;
        self.expect(
            self.desktop.pointer().map(|(_, _, shape)| shape)
                == Some(PointerShape::ResizeNorthWestSouthEast),
            "the pointer over the corner is not the diagonal resize shape",
        )?;
        self.check("the resize pointer over the first window's corner")?;
        self.pointer(corner_x, corner_y, Some(true))?;
        self.pointer(corner_x + 50, corner_y + 30, None)?;
        self.pointer(corner_x + 100, corner_y + 60, None)?;
        self.pointer(corner_x + 100, corner_y + 60, Some(false))?;
        let resized = self.content(first)?;
        self.expect(
            (resized.width, resized.height) == (WINDOW_CONTENT.0 + 100, WINDOW_CONTENT.1 + 60),
            "the corner drag did not resize the first window by (100, 60)",
        )?;
        self.answer_resize(0, first, 1)?;
        self.check("the first window resized from its corner")?;

        let maximise = self
            .desktop
            .control_of(first, Part::Maximise)
            .unwrap_or_default();
        self.click(maximise.x + 10, maximise.y + 14)?;
        self.expect(
            self.desktop.is_maximised(first),
            "the maximise control did not maximise the first window",
        )?;
        self.wait_for("hello-window first: state maximised", 1)?;
        self.answer_resize(0, first, 1)?;
        self.check("the first window maximised by its control")?;

        let title = self.desktop.title_bar_of(first).unwrap_or_default();
        let (title_x, title_y) = (title.x + title.width / 2, title.y + 14);
        self.double_click(title_x, title_y)?;
        self.expect(
            !self.desktop.is_maximised(first),
            "a double-click on the title bar did not restore the first window",
        )?;
        self.wait_for("hello-window first: state normal", 1)?;
        self.answer_resize(0, first, 2)?;
        self.check("the first window restored by a double-click")?;

        let title = self.desktop.title_bar_of(first).unwrap_or_default();
        let (title_x, title_y) = (title.x + title.width / 2, title.y + 14);
        self.double_click(title_x, title_y)?;
        self.expect(
            self.desktop.is_maximised(first),
            "a double-click on the title bar did not maximise the first window",
        )?;
        self.wait_for("hello-window first: state maximised", 2)?;
        self.answer_resize(0, first, 2)?;
        self.check("the first window maximised by a double-click")?;

        let restore = self
            .desktop
            .control_of(first, Part::Maximise)
            .unwrap_or_default();
        self.click(restore.x + 10, restore.y + 14)?;
        self.wait_for("hello-window first: state normal", 2)?;
        self.answer_resize(0, first, 3)?;
        self.check("the first window restored by its control")?;

        self.key("alt", key::LEFT_ALT, true)?;
        self.tap("tab", key::TAB)?;
        let terminal = self.desktop.terminal_window();
        let entries = self.desktop.switcher_entries();
        self.expect(
            entries == Some((vec![first, second, terminal], 1)),
            "Alt+Tab did not open the switcher on the second window",
        )?;
        self.check("the switcher open on the second window")?;
        self.tap("tab", key::TAB)?;
        self.check("the switcher on the terminal")?;
        self.key("alt", key::LEFT_ALT, false)?;
        self.expect(
            self.desktop.focused_window() == Some(terminal)
                && self.desktop.stacking().last() == Some(terminal),
            "releasing Alt did not raise and focus the terminal",
        )?;
        self.check("the terminal chosen in the switcher")?;
        self.key("alt", key::LEFT_ALT, true)?;
        self.tap("tab", key::TAB)?;
        self.key("alt", key::LEFT_ALT, false)?;
        self.expect(
            self.desktop.focused_window() == Some(first),
            "Alt+Tab did not go back to the first window",
        )?;
        self.check("the first window chosen with Alt+Tab")?;

        self.key("alt", key::LEFT_ALT, true)?;
        self.tap("f4", key::F4)?;
        self.key("alt", key::LEFT_ALT, false)?;
        self.wait_for("hello-window first: close request", 1)?;
        self.destroy(0, first)?;
        self.check("the first window closed with Alt+F4")?;

        self.key("alt", key::LEFT_ALT, true)?;
        self.tap("tab", key::TAB)?;
        self.key("alt", key::LEFT_ALT, false)?;
        self.expect(
            self.desktop.stacking().last() == Some(second),
            "Alt+Tab did not raise the second window",
        )?;
        let close = self.desktop.close_control(second).unwrap_or_default();
        self.click(close.x + 10, close.y + 14)?;
        self.wait_for("hello-window second: close request", 1)?;
        self.destroy(1, second)?;
        self.check("the second window closed by its close control")
    }

    fn destroy(&mut self, client: u16, id: WindowId) -> Result<(), String> {
        self.desktop
            .destroy(
                client,
                u32::from(id),
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(|error| {
                format!(
                    "{}: the host's desktop refused to destroy a window: {error:?}",
                    self.arch.name
                )
            })
    }
}

fn screen_size(qmp: &mut Qmp, arch: &Arch) -> Result<(u32, u32), String> {
    let path = screendump_path(arch, "")?;
    qmp.screendump(&path)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let picture = Picture::from_ppm(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((picture.width, picture.height))
}

fn screendump_path(arch: &Arch, suffix: &str) -> Result<std::path::PathBuf, String> {
    let target = crate::target_directory();
    std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(target.join(format!("screendump-{}{suffix}.ppm", arch.name)))
}

const CLOCK_SECONDS_ACCEPTED: u64 = 120;

fn check_desktop_frame(
    arch: &Arch,
    qmp: &mut Qmp,
    console: &mut Vec<u8>,
    output: &mpsc::Receiver<Vec<u8>>,
    at: &str,
    desktop: &Desktop,
    windows: &[(WindowId, WindowContent)],
) -> Result<(), String> {
    check_desktop_frame_within(
        arch,
        qmp,
        console,
        output,
        at,
        desktop,
        windows,
        TERMINAL_FRAME_TIMEOUT,
    )
}

#[allow(clippy::too_many_arguments)]
fn check_desktop_frame_within(
    arch: &Arch,
    qmp: &mut Qmp,
    console: &mut Vec<u8>,
    output: &mpsc::Receiver<Vec<u8>>,
    at: &str,
    desktop: &Desktop,
    windows: &[(WindowId, WindowContent)],
    timeout: Duration,
) -> Result<(), String> {
    check_desktop_frame_among(arch, qmp, console, output, at, desktop, &[windows], timeout)
}

#[allow(clippy::too_many_arguments)]
fn check_desktop_frame_among(
    arch: &Arch,
    qmp: &mut Qmp,
    console: &mut Vec<u8>,
    output: &mpsc::Receiver<Vec<u8>>,
    at: &str,
    desktop: &Desktop,
    alternatives: &[&[(WindowId, WindowContent)]],
    timeout: Duration,
) -> Result<(), String> {
    check_desktop_frame_masked(
        arch,
        qmp,
        console,
        output,
        at,
        desktop,
        alternatives,
        &[],
        timeout,
    )
}

fn copy_masked(from: &Picture, into: &mut Picture, masks: &[Rect]) {
    if from.width != into.width || from.height != into.height {
        return;
    }
    let width = from.width as usize;
    for mask in masks {
        let right = (mask.x + mask.width).min(from.width) as usize;
        let bottom = (mask.y + mask.height).min(from.height) as usize;
        for row in mask.y as usize..bottom {
            let start = row * width + (mask.x as usize).min(right);
            let end = row * width + right;
            into.pixels[start..end].copy_from_slice(&from.pixels[start..end]);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_desktop_frame_masked(
    arch: &Arch,
    qmp: &mut Qmp,
    console: &mut Vec<u8>,
    output: &mpsc::Receiver<Vec<u8>>,
    at: &str,
    desktop: &Desktop,
    alternatives: &[&[(WindowId, WindowContent)]],
    masks: &[Rect],
    timeout: Duration,
) -> Result<(), String> {
    let path = screendump_path(arch, "")?;
    let started = Instant::now();
    loop {
        console.extend(output.try_iter().flatten());
        qmp.screendump(&path)?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let picture = Picture::from_ppm(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let stream = terminal_stream(console).ok_or_else(|| {
            format!(
                "{}: no `{TERMINAL_LINE_PREFIX}...` line before {at}",
                arch.name
            )
        })?;
        let mut candidates = Vec::new();
        for windows in alternatives {
            candidates.push(
                desktop_picture(desktop, stream, windows)
                    .ok_or_else(|| format!("{}: the desktop holds no terminal", arch.name))?,
            );
        }
        let host = host_seconds();
        let matched = candidates.iter_mut().any(|expected| {
            copy_masked(&picture, expected, masks);
            clock_faces(host).any(|face| {
                with_clock(desktop, expected, face);
                picture == *expected
            })
        });
        if matched {
            eprintln!(
                "xtask: {}: the display shows the desktop at {at} ({}x{}, hash {:#x})",
                arch.name,
                picture.width,
                picture.height,
                picture.hash()
            );
            return Ok(());
        }
        if started.elapsed() > timeout {
            let Some(expected) = candidates.first_mut() else {
                return Err(format!("{}: no expected picture at {at}", arch.name));
            };
            with_clock(desktop, expected, ClockFace::WallClock(host_seconds()));
            let expected_path = screendump_path(arch, "-expected")?;
            std::fs::write(&expected_path, expected.to_ppm())
                .map_err(|e| format!("{}: {e}", expected_path.display()))?;
            return Err(format!(
                "{}: at {at} the display shows a picture with hash {:#x}, not the desktop the \
                 host renders ({:#x} with the clock at the host's time); compare {} with {}",
                arch.name,
                picture.hash(),
                expected.hash(),
                path.display(),
                expected_path.display()
            ));
        }
        std::thread::sleep(TERMINAL_FRAME_RETRY);
    }
}

/// Boots `kernel` with the console on this terminal, until the user quits QEMU; its display in
/// a window when `window` says so, and `extra_args` added to QEMU's (a host folder's).
pub fn run_interactive(
    arch: &Arch,
    kernel: &Path,
    window: bool,
    extra_args: &[String],
) -> Result<(), String> {
    let status = Command::new(arch.qemu)
        .args(interactive_args(arch, window))
        .args(extra_args)
        .arg("-kernel")
        .arg(kernel)
        .status()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("QEMU exited with {status}"))
    }
}

/// QEMU's arguments, but the kernel's, for an interactive boot of `arch`: its display in a
/// window when `window` says so, with the console on the terminal either way. `cargo xtask
/// run` and the release's run scripts both use them (D116). There is no `-no-reboot`, so
/// `reboot` restarts EggOS in the same QEMU (D153).
pub fn interactive_args(arch: &Arch, window: bool) -> Vec<String> {
    let console: &[&str] = match window {
        true => &["-serial", "mon:stdio"],
        false => &["-nographic"],
    };
    let cpus = TEST_CPUS.to_string();
    arch.machine_args
        .iter()
        .chain(arch.display_args)
        .chain(arch.input_args)
        .chain(COMMON_ARGS)
        .copied()
        .chain(["-smp", cpus.as_str()])
        .chain(console.iter().copied())
        .map(str::to_owned)
        .collect()
}

/// QEMU's arguments that put EggOS on the host's network (D157): user networking (`-netdev
/// user`, QEMU's own NAT and DHCP server: EggOS is 10.0.2.15, the host 10.0.2.2, DNS 10.0.2.3)
/// behind the architecture's virtio network device.
pub fn network_args(arch: &Arch) -> Vec<String> {
    vec![
        "-netdev".into(),
        format!("user,id={NETWORK_ID}"),
        "-device".into(),
        format!("{},netdev={NETWORK_ID}", arch.network_device),
    ]
}

/// QEMU's arguments for a machine without a network adapter (`--no-network`): no default one.
pub fn no_network_args() -> Vec<String> {
    vec!["-nic".into(), "none".into()]
}

const NETWORK_ID: &str = "net0";

/// The name the agent channel's port has, which the kernel looks for (D136).
pub const AGENT_PORT_NAME: &str = "org.eggos.mcp";

/// QEMU's arguments that add the agent channel (D136): a virtio console device with one named
/// port (`virtserialport`, port 1, which QEMU throttles instead of dropping output the host
/// has not read) on the character device `chardev`.
pub fn agent_channel_args(arch: &Arch, chardev: &str) -> Vec<String> {
    vec![
        "-device".into(),
        format!("{},id=agent", arch.agent_bus_device),
        "-device".into(),
        format!("virtserialport,bus=agent.0,nr=1,chardev={chardev},name={AGENT_PORT_NAME}"),
    ]
}

/// The kernel command line's word that asks for the offscreen display, which `--no-window`
/// passes (D144).
pub const OFFSCREEN_WORD: &str = "eggos.display=offscreen";

/// The grants `--mcp-allow` takes (D137), as the kernel command line names them.
pub const GRANT_NAMES: [&str; 4] = ["view", "write", "run", "install"];

/// Checks a `--mcp-allow` list: names from [`GRANT_NAMES`], lowercase, separated by single
/// commas, none empty.
pub fn check_grants(list: &str) -> Result<(), String> {
    for name in list.split(',') {
        if !GRANT_NAMES.contains(&name) {
            return Err(format!(
                "--mcp-allow: unknown grant '{name}' (view, write, run or install, separated by commas)"
            ));
        }
    }
    Ok(())
}

/// The kernel command line's word that hands the grants `list` (checked, or a script's
/// variable holding it) to EggOS's built-in MCP server, which only the host sets and the server
/// reads in `/proc/cmdline` (D137).
pub fn grant_word(list: &str) -> String {
    format!("eggos.mcp_allow={list}")
}

/// QEMU's arguments that make `line` (words separated by spaces) the kernel command line. QEMU
/// keeps only the last `-append`, so a boot passes every word in this one (D144). `-append`
/// takes a plain string: commas stay single.
pub fn command_line_args(line: &str) -> Vec<String> {
    vec!["-append".into(), line.into()]
}

/// QEMU's arguments, but the kernel's, for an MCP boot of `arch` (`run-eggos.sh --mcp`): the
/// agent channel on QEMU's standard input and output, which then carry nothing else, the
/// console in the file `console_log` (commas doubled), no monitor, and the display and input
/// devices with the display in a window when `window` says so; otherwise none of them
/// ([`Arch::offscreen_args`]), for a boot whose command line asks for the offscreen display
/// ([`OFFSCREEN_WORD`], D144). It keeps `-no-reboot`: a reboot ends QEMU and with it the MCP
/// session, which the client then starts again with a new `initialize` (D153).
pub fn mcp_args(arch: &Arch, window: bool, console_log: &str) -> Vec<String> {
    let cpus = TEST_CPUS.to_string();
    let devices: Vec<&str> = match window {
        true => arch
            .display_args
            .iter()
            .chain(arch.input_args)
            .copied()
            .collect(),
        false => arch.offscreen_args.to_vec(),
    };
    let mut args: Vec<String> = arch
        .machine_args
        .iter()
        .copied()
        .chain(devices)
        .chain(COMMON_ARGS.iter().copied())
        .chain(["-smp", cpus.as_str(), "-no-reboot"])
        .map(str::to_owned)
        .collect();
    if !window {
        args.extend(["-display".into(), "none".into()]);
    }
    args.extend([
        "-chardev".into(),
        format!("file,id=console,path={console_log}"),
        "-serial".into(),
        "chardev:console".into(),
        "-monitor".into(),
        "none".into(),
        "-chardev".into(),
        "stdio,id=mcp,signal=off".into(),
    ]);
    args.extend(agent_channel_args(arch, "mcp"));
    args
}

const SCREENSHOT_SESSION: [&[u8]; 2] = [b"uname -a\r", b"ls /bin\r"];

/// Boots `kernel` headless with its display, types [`SCREENSHOT_SESSION`] at the shell's
/// prompts, and once the display shows the desktop with the last prompt (checked against the
/// host's render, as the boot-to-prompt test does) returns the picture from QMP's
/// `screendump`.
pub fn screenshot(arch: &Arch, kernel: &Path) -> Result<Picture, String> {
    let port = qmp::free_port()?;
    let mut child = Command::new(arch.qemu)
        .args(arch.machine_args)
        .args(arch.display_args)
        .args(arch.input_args)
        .args(COMMON_ARGS)
        .arg("-smp")
        .arg(TEST_CPUS.to_string())
        .args(qmp::qemu_args(port))
        .args(["-display", "none", "-serial", "stdio", "-monitor", "none"])
        .arg("-kernel")
        .arg(kernel)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;
    let rx = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
    let mut keyboard = child.stdin.take().expect("stdin is piped");
    let picture = screenshot_session(arch, port, &rx, &mut keyboard);
    let _ = child.kill();
    let _ = child.wait();
    picture
}

fn screenshot_session(
    arch: &Arch,
    port: u16,
    rx: &mpsc::Receiver<Vec<u8>>,
    keyboard: &mut std::process::ChildStdin,
) -> Result<Picture, String> {
    let started = Instant::now();
    let mut console = Vec::new();
    let mut typed = 0;
    loop {
        console.extend(rx.try_iter().flatten());
        let prompts = String::from_utf8_lossy(&console)
            .matches(SHELL_PROMPT)
            .count();
        if prompts > SCREENSHOT_SESSION.len() {
            break;
        }
        if prompts > typed {
            keyboard
                .write_all(SCREENSHOT_SESSION[typed])
                .and_then(|()| keyboard.flush())
                .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
            typed += 1;
        }
        if started.elapsed() > BOOT_TIMEOUT {
            return Err(format!(
                "{}: no prompt after the screenshot's session within {}s",
                arch.name,
                BOOT_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut qmp = Qmp::connect(port)?;
    let (width, height) = screen_size(&mut qmp, arch)?;
    let desktop = Desktop::new(width, height)
        .ok_or_else(|| format!("{}: a {width}x{height} display holds no desktop", arch.name))?;
    check_desktop_frame(
        arch,
        &mut qmp,
        &mut console,
        rx,
        "the screenshot's last prompt",
        &desktop,
        &[],
    )?;
    let path = screendump_path(arch, "-screenshot")?;
    qmp.screendump(&path)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Picture::from_ppm(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_virtio_gpu_and_the_offscreen_display_share_the_default_mode() {
        use eggos_drivers::display::DEFAULT_MODE;
        let size = format!("xres={},yres={}", DEFAULT_MODE.width, DEFAULT_MODE.height);
        for arch in crate::ARCHES {
            assert!(
                arch.display_args.iter().any(|arg| arg.ends_with(&size)),
                "{}",
                arch.name
            );
        }
        let mode = format!("{}x{}", DEFAULT_MODE.width, DEFAULT_MODE.height);
        assert_eq!(
            OFFSCREEN_LINE_1280X800,
            format!("{OFFSCREEN_LINE_PREFIX}{mode}")
        );
        assert_eq!(DISPLAY_LINE_1280X800, format!("boot: display {mode}"));
    }

    #[test]
    fn an_offscreen_mcp_boot_has_no_display_or_input_device_but_keeps_its_channels() {
        for arch in crate::ARCHES {
            let args = mcp_args(arch, false, "/tmp/serial.log");
            let joined = args.join(" ");
            for device in [
                "virtio-gpu",
                "virtio-keyboard",
                "virtio-tablet",
                "virtio-mouse",
            ] {
                assert!(!joined.contains(device), "{}: {joined}", arch.name);
            }
            assert!(joined.contains("-display none"), "{joined}");
            assert_eq!(
                joined.contains("-vga none"),
                arch.name == "x86_64",
                "{joined}"
            );
            assert!(joined.contains("-chardev file,id=console,path=/tmp/serial.log"));
            assert!(joined.contains("-serial chardev:console"));
            assert!(joined.contains("-chardev stdio,id=mcp,signal=off"));
            assert!(joined.ends_with(&agent_channel_args(arch, "mcp").join(" ")));
            assert!(!joined.contains("-append"));
            let windowed = mcp_args(arch, true, "/tmp/serial.log").join(" ");
            for part in arch.display_args.iter().chain(arch.input_args) {
                assert!(windowed.contains(part), "{}: {part}", arch.name);
            }
        }
    }

    #[test]
    fn the_command_line_is_one_append_with_the_words_as_given() {
        assert_eq!(
            command_line_args("eggos.display=offscreen eggos.mcp_allow=write,run"),
            [
                "-append",
                "eggos.display=offscreen eggos.mcp_allow=write,run"
            ]
        );
        assert_eq!(OFFSCREEN_WORD, "eggos.display=offscreen");
        assert_eq!(grant_word("view,run"), "eggos.mcp_allow=view,run");
    }

    #[test]
    fn interactive_args_are_the_machine_s_display_input_and_console() {
        let arch = &crate::ARCHES[0];
        let window = interactive_args(arch, true);
        let joined = window.join(" ");
        for part in arch
            .machine_args
            .iter()
            .chain(arch.display_args)
            .chain(arch.input_args)
        {
            assert!(window.iter().any(|arg| arg == part), "{part}");
        }
        assert!(joined.ends_with("-smp 4 -serial mon:stdio"), "{joined}");
        let serial = interactive_args(arch, false).join(" ");
        assert!(serial.ends_with("-smp 4 -nographic"), "{serial}");
        assert!(!joined.contains("-no-reboot") && !serial.contains("-no-reboot"));
        let mcp = mcp_args(arch, false, "/tmp/serial.log").join(" ");
        assert!(mcp.contains("-smp 4 -no-reboot"), "{mcp}");
    }

    #[test]
    fn a_work_disk_is_a_raw_block_device_of_its_own_beside_the_home_disk() {
        for arch in crate::ARCHES {
            let args = work_disk_args(arch, "/x/a,b.img");
            assert_eq!(
                args,
                [
                    "-blockdev".to_string(),
                    "driver=file,node-name=work-file,filename=/x/a,,b.img".into(),
                    "-blockdev".into(),
                    "driver=raw,node-name=work,file=work-file".into(),
                    "-device".into(),
                    format!("{},drive=work", arch.disk_device),
                ]
            );
            let home = disk_args(arch, "/x/home.img");
            for name in ["node-name=work", "node-name=work-file", "drive=work"] {
                assert!(home.iter().all(|arg| !arg.contains(name)), "{name}");
            }
        }
    }

    #[test]
    fn a_host_folder_is_a_vvfat_block_device_with_its_commas_doubled() {
        for arch in crate::ARCHES {
            let args = host_folder_args(arch, "/home/me/a,b:c");
            assert_eq!(
                args,
                [
                    "-blockdev".to_string(),
                    "driver=vvfat,node-name=host,dir=/home/me/a,,b:c,read-only=on".into(),
                    "-device".into(),
                    format!("{},drive=host", arch.disk_device),
                ]
            );
        }
        assert_eq!(
            crate::ARCHES[2].disk_device,
            "virtio-blk-pci,disable-legacy=on"
        );
    }

    #[test]
    fn the_boot_clock_line_counts_only_whole() {
        let line = "EGGOS-BENCH: boot_to_prompt iterations=1 ticks=5 frequency=1000000000";
        assert!(!boot_clock_line(&format!("eggsh:/# \r\n{line}")));
        assert!(boot_clock_line(&format!("eggsh:/# \r\n{line}\r\n")));
        assert!(!boot_clock_line(&format!("x {line}\r\n")));
        assert!(verdict("x", Expect::BootClock, false, &format!("\r\n{line}\r\n")).is_ok());
        assert!(verdict("x", Expect::BootClock, false, "eggsh:/# ").is_err());
    }

    fn all_user_lines() -> Vec<String> {
        USER_LINES
            .iter()
            .chain([&CONSOLE_PROGRAM_LINE])
            .map(|line| line.to_string())
            .collect()
    }

    fn passing_console(user_lines: &[String]) -> String {
        format!(
            "EggOS\r\n{LINES_ECHO}{}\r\n{PASS_MARKER}\r\n",
            user_lines.join("\r\n")
        )
    }

    #[test]
    fn passes_only_on_clean_exit_with_pass_marker() {
        let ok = passing_console(&all_user_lines());
        assert!(verdict("x", Expect::Pass, true, &ok).is_ok());
        assert!(verdict("x", Expect::Pass, false, &ok).is_err());
        assert!(verdict("x", Expect::Pass, true, "EggOS\r\n").is_err());
    }

    #[test]
    fn a_pass_needs_every_linux_test_program_s_line_whole() {
        let silent = format!("EggOS\r\n{LINES_ECHO}{PASS_MARKER}\r\n");
        assert!(verdict("x", Expect::Pass, true, &silent).is_err());
        for (index, line) in all_user_lines().iter().enumerate() {
            let mut garbled = all_user_lines();
            garbled[index] = format!("{line}x");
            assert!(verdict("x", Expect::Pass, true, &passing_console(&garbled)).is_err());
            let mut missing = all_user_lines();
            missing.remove(index);
            assert!(verdict("x", Expect::Pass, true, &passing_console(&missing)).is_err());
        }
    }

    #[test]
    fn a_benchmark_boot_needs_no_console_program_line_or_echo() {
        let console = format!("EggOS\r\n{}\r\n{PASS_MARKER}\r\n", USER_LINES.join("\r\n"));
        assert!(verdict("x", Expect::Benchmarks, true, &console).is_ok());
        assert!(verdict("x", Expect::Pass, true, &console).is_err());
        let without_hello = console.replace(USER_HELLO, "");
        assert!(verdict("x", Expect::Benchmarks, true, &without_hello).is_err());
    }

    #[test]
    fn a_pass_needs_the_typed_lines_echoed_as_edited() {
        let ok = passing_console(&all_user_lines());
        let unechoed = ok.replace(LINES_ECHO, "");
        assert!(verdict("x", Expect::Pass, true, &unechoed).is_err());
        let raw = ok.replace(LINES_ECHO, "hello\r\nab\x08c\r\n");
        assert!(verdict("x", Expect::Pass, true, &raw).is_err());
    }

    #[test]
    fn each_script_is_typed_once_each_time_its_line_appears() {
        let mut typed = [0; CONSOLE_SCRIPTS.len()];
        assert!(scripts_to_type("EGGOS-CONSOLE: input rea", &mut typed).is_empty());
        assert!(scripts_to_type("x EGGOS-CONSOLE: input ready", &mut typed).is_empty());
        let console = "boot\r\nEGGOS-CONSOLE: input ready\r\n";
        assert_eq!(scripts_to_type(console, &mut typed), [CONSOLE_INPUT_SCRIPT]);
        assert!(scripts_to_type(console, &mut typed).is_empty());
        let console = format!(
            "{console}EGGOS-CONSOLE: type lines\r\n{LINES_ECHO}EGGOS-CONSOLE: type raw bytes\r\n\
             EGGOS-CONSOLE: type lines\r\nnot EGGOS-CONSOLE: type an interrupt\r\n"
        );
        assert_eq!(
            scripts_to_type(&console, &mut typed),
            [LINES_SCRIPT, LINES_SCRIPT, CONSOLE_SCRIPTS[4].1]
        );
        assert_eq!(typed, [1, 2, 0, 0, 1, 0, 0, 0]);
        let partial = format!("{console}EGGOS-CONSOLE: type an interrupt");
        assert!(scripts_to_type(&partial, &mut typed).is_empty());
        let whole = format!("{partial} and a line\r\n");
        assert_eq!(scripts_to_type(&whole, &mut typed), [CONSOLE_SCRIPTS[3].1]);
    }

    fn android_programs_console() -> String {
        let mut console = String::from("EggOS 0.2.0 (aarch64)\r\n");
        for (typed, output) in ANDROID_PROGRAM_SESSION.iter().zip(android_program_output()) {
            let command = String::from_utf8_lossy(typed);
            console.push_str(&format!("{SHELL_PROMPT}# {}\r\n", command.trim_end()));
            for line in output {
                if let Line::Whole(line) = line {
                    console.push_str(&format!("{line}\r\n"));
                }
            }
        }
        console
    }

    fn macos_programs_console(arch: &str) -> String {
        let mut console = format!("EggOS 0.2.0 ({arch})\r\n");
        for (typed, output) in MACOS_PROGRAM_SESSION.iter().zip(macos_program_output(arch)) {
            let command = String::from_utf8_lossy(typed);
            console.push_str(&format!("{SHELL_PROMPT}# {}\r\n", command.trim_end()));
            for line in output {
                match line {
                    Line::Whole(line) => console.push_str(&format!("{line}\r\n")),
                    Line::Around(start, end) => console.push_str(&format!("{start}/bin/{end}\r\n")),
                }
            }
        }
        console
    }

    #[test]
    fn the_macos_programs_session_passes_only_with_each_command_s_lines_in_order() {
        for arch in ["aarch64", "x86_64", "riscv64"] {
            let ok = macos_programs_console(arch);
            assert!(
                verdict(arch, Expect::MacosPrograms, true, &ok).is_ok(),
                "{arch}"
            );
            assert!(verdict(arch, Expect::MacosPrograms, false, &ok).is_err());
            let without = ok.replacen("MACOS-CONSOLE status 126\r\n", "", 1);
            assert!(verdict(arch, Expect::MacosPrograms, true, &without).is_err());
        }
        let ok = macos_programs_console("aarch64");
        for missing in [
            "MACOS-CONSOLE atexit",
            "MACOS-CONSOLE status 3",
            "Aborted",
            "MACOS-CONSOLE dir about",
        ] {
            let without = ok.replacen(&format!("{missing}\r\n"), "", 1);
            assert!(
                verdict("aarch64", Expect::MacosPrograms, true, &without).is_err(),
                "{missing}"
            );
        }
        let elsewhere = macos_programs_console("x86_64");
        assert!(verdict("aarch64", Expect::MacosPrograms, true, &elsewhere).is_err());
    }

    #[test]
    fn the_android_programs_session_passes_only_with_each_command_s_lines_in_order() {
        let ok = android_programs_console();
        assert!(verdict("aarch64", Expect::AndroidPrograms, true, &ok).is_ok());
        assert!(verdict("aarch64", Expect::AndroidPrograms, false, &ok).is_err());
        for missing in [
            "ANDROID-CONSOLE shell",
            "ANDROID-CONSOLE status 42",
            "Aborted",
            "ANDROID-CONSOLE stderr",
        ] {
            let without = ok.replacen(&format!("{missing}\r\n"), "", 1);
            assert!(
                verdict("aarch64", Expect::AndroidPrograms, true, &without).is_err(),
                "{missing}"
            );
        }
        let set = ok.replace("ro.build.version.sdk unset", "ro.build.version.sdk set");
        assert!(verdict("aarch64", Expect::AndroidPrograms, true, &set).is_err());
    }

    fn windows_programs_console(arch: &str) -> String {
        let mut console = format!("EggOS 0.2.0 ({arch})\r\n");
        for (typed, output) in WINDOWS_PROGRAM_SESSION
            .iter()
            .zip(windows_program_output(arch))
        {
            let command = String::from_utf8_lossy(typed);
            console.push_str(&format!("{SHELL_PROMPT}# {}\r\n", command.trim_end()));
            for line in output {
                let line = match line {
                    Line::Whole(whole) => whole,
                    Line::Around(start, end) => format!("{start}/bin/{end}"),
                };
                console.push_str(&format!("{line}\r\n"));
            }
        }
        console
    }

    #[test]
    fn the_windows_programs_session_runs_the_checks_where_the_layer_runs_and_enoexec_elsewhere() {
        for arch in ["x86_64", "aarch64", "riscv64"] {
            let ok = windows_programs_console(arch);
            assert!(
                verdict(arch, Expect::WindowsPrograms, true, &ok).is_ok(),
                "{arch}"
            );
            let without = ok.replace("WINDOWS-CONSOLE status 126\r\n", "");
            assert!(verdict(arch, Expect::WindowsPrograms, true, &without).is_err());
        }
        let x86 = windows_programs_console("x86_64");
        let refused = x86.replace("WINDOWS-CONSOLE heap ok", "WINDOWS-CONSOLE heap failed");
        assert!(verdict("x86_64", Expect::WindowsPrograms, true, &refused).is_err());
        for (arch, other) in [
            ("x86_64", "aarch64"),
            ("aarch64", "x86_64"),
            ("x86_64", "riscv64"),
        ] {
            let elsewhere = windows_programs_console(other);
            assert!(verdict(arch, Expect::WindowsPrograms, true, &elsewhere).is_err());
        }
        let arm = windows_programs_console("aarch64");
        assert!(arm.contains("system arch 12 "));
        assert!(arm.contains("other-cpu.exe: an x64 program"));
    }

    fn windows_crt_console() -> String {
        let mut console = "EggOS 0.2.0 (x86_64)\r\n".to_owned();
        for (typed, output) in WINDOWS_CRT_SESSION.iter().zip(windows_crt_output()) {
            let command = String::from_utf8_lossy(typed);
            console.push_str(&format!("{SHELL_PROMPT}# {}\r\n", command.trim_end()));
            for line in output {
                if let Line::Whole(whole) = line {
                    console.push_str(&format!("{whole}\r\n"));
                }
            }
        }
        console
    }

    #[test]
    fn the_windows_crt_session_checks_eggpad_s_lines_and_the_saved_file() {
        let ok = windows_crt_console();
        assert!(verdict("x86_64", Expect::WindowsCrt, true, &ok).is_ok());
        assert!(verdict("x86_64", Expect::WindowsCrt, false, &ok).is_err());
        let unsaved = ok.replace("EGGPAD saved 27 bytes\r\n", "");
        assert!(verdict("x86_64", Expect::WindowsCrt, true, &unsaved).is_err());
        let lost = ok.replace("abc\r\n", "");
        assert!(verdict("x86_64", Expect::WindowsCrt, true, &lost).is_err());
        let failed = ok.replace("EGGPAD status 0", "EGGPAD status 126");
        assert!(verdict("x86_64", Expect::WindowsCrt, true, &failed).is_err());
    }

    #[test]
    fn the_windows_crt_session_matches_the_display_check() {
        let typed = String::from_utf8_lossy(WINDOWS_CRT_SESSION[0]);
        let seeded = windows_crt::SEEDED_TEXT.trim_end();
        assert!(typed.starts_with(&format!("echo {seeded} > /tmp/note.txt; ")));
        let saved = format!("EGGPAD {}", windows_crt::SAVED_STATUS.to_lowercase());
        assert!(EGGPAD_LINES.contains(&saved.as_str()));
        assert!(
            EGGPAD_LINES.contains(&format!("EGGPAD opened {} bytes", seeded.len() + 1).as_str())
        );
    }

    fn windows_gui_console(arch: &str) -> String {
        let mut console = format!("EggOS 0.2.0 ({arch})\r\n");
        for (typed, output) in WINDOWS_GUI_SESSION.iter().zip(windows_gui_output(arch)) {
            let command = String::from_utf8_lossy(typed);
            console.push_str(&format!("{SHELL_PROMPT}# {}\r\n", command.trim_end()));
            for line in output {
                if let Line::Whole(whole) = line {
                    console.push_str(&format!("{whole}\r\n"));
                }
            }
        }
        console
    }

    #[test]
    fn the_windows_gui_session_checks_the_programs_lines_where_the_layer_runs() {
        for arch in ["x86_64", "aarch64", "riscv64"] {
            let ok = windows_gui_console(arch);
            assert!(
                verdict(arch, Expect::WindowsGui, true, &ok).is_ok(),
                "{arch}"
            );
            assert!(verdict(arch, Expect::WindowsGui, false, &ok).is_err());
        }
        let x86 = windows_gui_console("x86_64");
        let unanswered = x86.replace("WIN-PAINT message box 1\r\n", "");
        assert!(verdict("x86_64", Expect::WindowsGui, true, &unanswered).is_err());
        let failed = x86.replace("WIN-PAINT status 0", "WIN-PAINT status 1");
        assert!(verdict("x86_64", Expect::WindowsGui, true, &failed).is_err());
        let refused = windows_gui_console("riscv64");
        assert!(verdict("aarch64", Expect::WindowsGui, true, &refused).is_err());
        assert!(verdict("riscv64", Expect::WindowsGui, true, &x86).is_err());
    }

    fn linux_programs_console() -> String {
        let mut console = String::from("EggOS 0.2.0 (riscv64)\r\n");
        for (typed, output) in LINUX_PROGRAM_SESSION.iter().zip(linux_program_output()) {
            let command = String::from_utf8_lossy(typed);
            console.push_str(&format!("{SHELL_PROMPT}# {}\r\n", command.trim_end()));
            for line in output {
                let line = match line {
                    Line::Whole(whole) => whole,
                    Line::Around(start, end) => format!("{start}(9) panicked at main.rs:1:1{end}"),
                };
                console.push_str(&format!("{line}\r\n"));
            }
        }
        console
    }

    #[test]
    fn the_program_windows_session_passes_only_with_both_programs_lines_in_order() {
        let mut ok = String::from("EggOS 0.2.0 (aarch64)\r\n");
        for (typed, output) in PROGRAM_WINDOW_SESSION.iter().zip(program_window_output()) {
            let command = String::from_utf8_lossy(typed);
            ok.push_str(&format!("{SHELL_PROMPT}# {}\r\n", command.trim_end()));
            for line in output {
                match line {
                    Line::Whole(line) => ok.push_str(&format!("{line}\r\n")),
                    Line::Around(start, end) => {
                        ok.push_str(&format!("{start}311 192 658 458{end}\r\n"))
                    }
                }
            }
        }
        assert!(verdict("aarch64", Expect::ProgramWindows, true, &ok).is_ok());
        let unclosed = ok.replacen("hello-window second: close request\r\n", "", 1);
        assert!(verdict("aarch64", Expect::ProgramWindows, true, &unclosed).is_err());
        let swapped = ok.replacen(
            "first: close request\r\nhello-window second: close request",
            "second: close request\r\nhello-window first: close request",
            1,
        );
        assert!(verdict("aarch64", Expect::ProgramWindows, true, &swapped).is_err());
        assert!(has_line(&ok, "hello-window first: shown"));
        assert!(!has_line(&ok, "hello-window first: show"));
        assert!(!has_line(
            "hello-window first: shown",
            "hello-window first: shown"
        ));
    }

    #[test]
    fn the_linux_programs_session_passes_only_with_each_command_s_lines_in_order() {
        let ok = linux_programs_console();
        assert!(verdict("riscv64", Expect::LinuxPrograms, true, &ok).is_ok());
        assert!(verdict("riscv64", Expect::LinuxPrograms, false, &ok).is_err());
        for missing in [
            "LINUX-STATIC status 42",
            "Aborted",
            "LINUX-STATIC stderr",
            "LINUX-SOCKETS poll woken by the client",
            "LINUX-SOCKETS status 0",
            "LINUX-MEMORY anonymous killed by SIGKILL",
        ] {
            let without = ok.replacen(&format!("{missing}\r\n"), "", 1);
            assert!(
                verdict("riscv64", Expect::LinuxPrograms, true, &without).is_err(),
                "{missing}"
            );
        }
        let swapped = ok.replacen(
            "LINUX-STATIC file eggos\r\nLINUX-STATIC dir about",
            "LINUX-STATIC dir about\r\nLINUX-STATIC file eggos",
            1,
        );
        assert!(verdict("riscv64", Expect::LinuxPrograms, true, &swapped).is_err());
        let untyped = ok.replacen("linux-static exit 42", "linux-static exit 41", 1);
        assert!(verdict("riscv64", Expect::LinuxPrograms, true, &untyped).is_err());
        let panicked = format!("{ok}{KERNEL_PANIC_PREFIX} - not syncing\r\n");
        assert!(verdict("riscv64", Expect::LinuxPrograms, true, &panicked).is_err());
    }

    #[test]
    fn fail_marker_wins_over_pass_marker() {
        let mixed = format!("{PASS_MARKER}\n{FAIL_MARKER}\n");
        assert!(verdict("x", Expect::Pass, true, &mixed).is_err());
    }

    #[test]
    fn a_second_boot_banner_fails_even_after_pass() {
        let rebooted = format!("EggOS 0.1.0 (x86_64)\n{PASS_MARKER}\nEggOS 0.1.0 (x86_64)\n");
        assert!(verdict("x", Expect::Pass, true, &rebooted).is_err());
    }

    fn reported_fatal_exception() -> String {
        format!(
            "{PASS_MARKER}\n{WRITE_PROTECT_PROBE}0x5000\n\n\
             {EXCEPTION_PREFIX}#PF page fault (code 0x3) at pc 0x1000, address 0x5000\n{FAIL_MARKER}\n"
        )
    }

    #[test]
    fn fatal_exception_needs_the_report_then_the_fail_marker() {
        let reported = reported_fatal_exception();
        assert!(verdict("x", Expect::FatalException, true, &reported).is_ok());
        assert!(verdict("x", Expect::FatalException, false, &reported).is_err());
    }

    #[test]
    fn a_plain_pass_a_panic_or_an_early_fail_marker_is_not_the_exception_path() {
        let pass = format!("{PASS_MARKER}\n");
        assert!(verdict("x", Expect::FatalException, true, &pass).is_err());
        let panic = format!("{PASS_MARKER}\nkernel panic: x\n{FAIL_MARKER}\n");
        assert!(verdict("x", Expect::FatalException, true, &panic).is_err());
        let early = format!("{FAIL_MARKER}\n{EXCEPTION_PREFIX}x\n");
        assert!(verdict("x", Expect::FatalException, true, &early).is_err());
    }

    #[test]
    fn a_fatal_exception_without_the_self_tests_passing_first_proves_nothing() {
        let no_pass = format!("{EXCEPTION_PREFIX}x\n{FAIL_MARKER}\n");
        assert!(verdict("x", Expect::FatalException, true, &no_pass).is_err());
    }

    #[test]
    fn a_fault_elsewhere_or_the_illegal_instruction_after_an_unprotected_write_is_not_caught() {
        let elsewhere = reported_fatal_exception().replace("address 0x5000", "address 0x6000");
        assert!(verdict("x", Expect::FatalException, true, &elsewhere).is_err());
        let unprotected = format!(
            "{PASS_MARKER}\n{WRITE_PROTECT_PROBE}0x5000\n\n\
             {EXCEPTION_PREFIX}#UD invalid opcode (code 0x0) at pc 0x1000\n{FAIL_MARKER}\n"
        );
        assert!(verdict("x", Expect::FatalException, true, &unprotected).is_err());
    }

    #[test]
    fn the_fatal_exception_run_fails_when_a_pass_is_expected() {
        assert!(verdict("x", Expect::Pass, true, &reported_fatal_exception()).is_err());
    }

    fn init_console() -> String {
        format!(
            "EggOS 0.2.0 (x86_64)\r\n{INIT_BANNER}\r\n{}\r\n{POWER_DOWN_LINE}\r\n",
            INIT_LOGIN_LINES.join("\r\n")
        )
    }

    #[test]
    fn an_init_boot_needs_the_banner_every_login_line_and_the_power_down() {
        let ok = init_console();
        assert!(verdict("x", Expect::Init, true, &ok).is_ok());
        assert!(verdict("x", Expect::Init, false, &ok).is_err());
        for missing in INIT_LOGIN_LINES
            .iter()
            .chain([&INIT_BANNER, &POWER_DOWN_LINE])
        {
            let without = ok.replace(&format!("{missing}\r\n"), "");
            assert!(
                verdict("x", Expect::Init, true, &without).is_err(),
                "{missing}"
            );
        }
        let panicked = format!("{ok}Kernel panic - not syncing: Attempted to kill init!\r\n");
        assert!(verdict("x", Expect::Init, true, &panicked).is_err());
        let reset = format!("{ok}EggOS 0.2.0 (x86_64)\r\n");
        assert!(verdict("x", Expect::Init, true, &reset).is_err());
        assert!(verdict("x", Expect::InitRestart, true, &ok).is_err());
        let restarted = ok.replace(POWER_DOWN_LINE, RESTART_LINE);
        assert!(verdict("x", Expect::InitRestart, true, &restarted).is_ok());
        assert!(verdict("x", Expect::Init, true, &restarted).is_err());
    }

    fn shell_console() -> String {
        format!(
            "EggOS 0.2.0 (x86_64)\r\n{INIT_BANNER}\r\neggsh:/# {}\r\n{SHELL_EXIT_LINE}\r\n\
             {POWER_DOWN_LINE}\r\n",
            SHELL_LINES.join("\r\neggsh:/# ")
        )
    }

    #[test]
    fn a_shell_boot_needs_every_session_line_the_shell_s_exit_and_the_power_down() {
        let ok = shell_console();
        assert!(verdict("x", Expect::Shell, true, &ok).is_ok());
        assert!(verdict("x", Expect::Shell, false, &ok).is_err());
        for missing in SHELL_LINES
            .iter()
            .chain([&SHELL_EXIT_LINE, &POWER_DOWN_LINE])
        {
            let without = ok.replace(missing, "");
            assert!(
                verdict("x", Expect::Shell, true, &without).is_err(),
                "{missing}"
            );
        }
        let garbled = ok.replace("EGGSH-TEST child 3", "EGGSH-TEST child 3x");
        assert!(verdict("x", Expect::Shell, true, &garbled).is_err());
        let panicked = format!("{ok}Kernel panic - not syncing: Attempted to kill init!\r\n");
        assert!(verdict("x", Expect::Shell, true, &panicked).is_err());
    }

    #[test]
    fn the_shell_session_types_a_line_at_each_prompt_and_c_for_the_waiting_program() {
        let mut typed = ShellTyping::default();
        let session = &SHELL_SESSION;
        let starting = "init: starting /bin/eggsh\r\n";
        assert!(shell_scripts_to_type(starting, session, &mut typed).is_empty());
        let started = "init: starting /bin/eggsh\r\neggsh:/# ";
        assert_eq!(
            shell_scripts_to_type(started, session, &mut typed),
            [SHELL_SESSION[0]]
        );
        assert!(shell_scripts_to_type(started, session, &mut typed).is_empty());
        let error = format!("{started}x\r\neggsh: x: command not found\r\neggsh:/usr# ");
        assert_eq!(
            shell_scripts_to_type(&error, session, &mut typed),
            [SHELL_SESSION[1]]
        );
        let waiting = format!("{error}{}\r\n", SHELL_INTERRUPT.0);
        assert_eq!(
            shell_scripts_to_type(&waiting, session, &mut typed),
            [SHELL_INTERRUPT.1]
        );
        let many = SHELL_PROMPT.repeat(SHELL_SESSION.len() + 5);
        assert_eq!(
            shell_scripts_to_type(&many, session, &mut typed).len(),
            SHELL_SESSION.len() - 2
        );
        assert_eq!(typed.lines, SHELL_SESSION.len());
    }

    fn prompt_console(arch: &str) -> String {
        let mut console = format!(
            "EggOS 0.2.0 ({arch})\r\n{DISPLAY_LINE_1280X800}\r\n{BOOT_INPUT_LINE}\r\n{TERMINAL_LINE_1280X800}\r\n{INIT_BANNER}\r\n"
        );
        for (index, written) in prompt_session_output(arch).into_iter().enumerate() {
            let command = shown_command(&PROMPT_SESSION, index);
            console.push_str(&format!("{SHELL_PROMPT}# {command}\r\n"));
            for line in written {
                match line {
                    Line::Whole(line) => console.push_str(&format!("{line}\r\n")),
                    Line::Around(start, end) => {
                        console.push_str(&format!("{start}42408 1970-01-01 00:00{end}\r\n"))
                    }
                }
            }
        }
        console
    }

    #[test]
    fn a_boot_to_prompt_needs_each_command_s_lines_right_after_it_and_the_power_down() {
        let ok = prompt_console("riscv64");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &ok).is_ok());
        assert!(verdict("riscv64", Expect::BootToPrompt, false, &ok).is_err());
        assert!(verdict("x86_64", Expect::BootToPrompt, true, &ok).is_err());
        for missing in [
            "usr\r\n",
            "EGGOS-COREUTILS ls status 2\r\n",
            "eggos\r\n",
            POWER_DOWN_LINE,
        ] {
            let without = ok.replacen(missing, "", 1);
            assert!(
                verdict("riscv64", Expect::BootToPrompt, true, &without).is_err(),
                "{missing}"
            );
        }
        let reordered = ok.replace("bin\r\ndev\r\n", "dev\r\nbin\r\n");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &reordered).is_err());
        let one_link = ok.replace("-rwxr-xr-x 30 ", "-rwxr-xr-x 1 ");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &one_link).is_err());
        let untyped = ok.replace("# uname -a", "# uname");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &untyped).is_err());
        let panicked = format!("{ok}Kernel panic - not syncing: Attempted to kill init!\r\n");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &panicked).is_err());
        let no_input = ok.replace(BOOT_INPUT_LINE, "boot: no input");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &no_input).is_err());
        let unshifted = ok.replace("Typed_On+Virtio:OK\r\n", "typed-on=virtio;ok\r\n");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &unshifted).is_err());
        let no_terminal = ok.replace(TERMINAL_LINE_1280X800, "boot: no display");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &no_terminal).is_err());
        let no_display = ok.replace(DISPLAY_LINE_1280X800, "boot: no display");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &no_display).is_err());
        let offscreen = ok.replace(
            DISPLAY_LINE_1280X800,
            &format!("{OFFSCREEN_LINE_1280X800}\r\n{DISPLAY_LINE_1280X800}"),
        );
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &offscreen).is_err());
    }

    #[test]
    fn the_terminal_s_bytes_are_the_console_s_after_the_terminal_line() {
        let console = b"EggOS 0.2.0 (aarch64)\r\nboot: terminal 80x24\r\nEggOS 0.2.0\r\neggsh:/# ";
        assert_eq!(
            terminal_stream(console),
            Some(&b"EggOS 0.2.0\r\neggsh:/# "[..])
        );
        assert_eq!(terminal_stream(b"EggOS\r\nboot: terminal 80x24"), None);
        assert_eq!(terminal_stream(b"EggOS\r\nboot: no display\r\nx"), None);
    }

    #[test]
    fn the_expected_picture_is_the_desktop_with_the_engine_s_render_of_the_stream() {
        let desktop = Desktop::new(1024, 768).unwrap();
        let picture = desktop_picture(&desktop, b"eggsh:/# ", &[]).unwrap();
        assert_eq!((picture.width, picture.height), (1024, 768));
        assert_eq!(picture.pixels.len(), 1024 * 768);
        assert_eq!(picture.pixels[10], eggos_drivers::desktop::colour::BAR);
        let area = desktop.text_area();
        let cursor = picture.pixels[(area.y as usize) * 1024 + area.x as usize + 9 * 8];
        assert_eq!(cursor, eggos_drivers::terminal::TERM_CURSOR);
        assert_ne!(
            picture,
            desktop_picture(&desktop, b"eggsh:/#", &[]).unwrap()
        );
    }

    #[test]
    fn a_pointer_step_lands_on_the_pixel_it_aims_at() {
        for (pixel, size) in [(0, 1280), (348, 1280), (1279, 1280), (213, 800), (799, 800)] {
            let value = tablet_value(pixel, size);
            assert_eq!(scale_to_screen(value, TABLET_RANGE, size), pixel);
        }
    }

    #[test]
    fn the_pointer_steps_minimise_restore_and_drag_the_window() {
        let mut desktop = Desktop::new(1280, 800).unwrap();
        let window = desktop.window();
        let states: Vec<_> = pointer_steps(&desktop)
            .into_iter()
            .map(|(_, steps)| {
                for step in steps {
                    apply_pointer_step(&mut desktop, step);
                }
                (desktop.minimised(), desktop.window())
            })
            .collect();
        let dragged = eggos_drivers::display::Rect::new(window.x + 100, window.y + 100, 658, 430);
        assert_eq!(states, [(true, window), (false, window), (false, dragged)]);
    }

    #[test]
    fn every_keyboard_line_is_a_session_line_qmp_can_type() {
        for line in KEYBOARD_LINES {
            assert!(PROMPT_SESSION.contains(&line));
            for character in String::from_utf8_lossy(line).chars() {
                assert!(qmp::typing(character).is_ok(), "{character:?}");
            }
        }
    }

    #[test]
    fn keyboard_lines_are_typed_a_few_keys_at_a_time_each_after_the_last_one_s_echo() {
        let mut console = b"eggsh:/# ".to_vec();
        let mut typing = KeyboardTyping::new(b"echo EGGOS-KEYBOARD\r", console.len());
        assert_eq!(typing.next_keys(&console).as_deref(), Some("echo EGG"));
        assert_eq!(typing.next_keys(&console), None);
        console.extend(b"echo EG");
        assert_eq!(typing.next_keys(&console), None);
        console.extend(b"G");
        assert_eq!(typing.next_keys(&console).as_deref(), Some("OS-KEYBO"));
        console.extend(b"OS-KEYBO");
        assert_eq!(typing.next_keys(&console).as_deref(), Some("ARD\r"));
        assert!(typing.is_done());
        console.extend(b"ARD\r\n");
        assert_eq!(typing.next_keys(&console), None);
    }

    #[test]
    fn the_input_line_is_counted_only_whole() {
        let console = format!("x {INPUT_EVENTS_LINE}\n{INPUT_EVENTS_LINE}\r\n{INPUT_EVENTS_LINE}");
        assert_eq!(count_lines(&console, INPUT_EVENTS_LINE), 1);
    }

    #[test]
    fn a_line_around_a_middle_needs_its_start_and_end_apart() {
        let line = Line::Around("ab", "ba");
        assert!(line.matches("ab ba"));
        assert!(line.matches("abba"));
        assert!(!line.matches("aba"));
        assert!(!line.matches("xabba"));
    }

    #[test]
    fn marker_must_be_a_whole_line() {
        let embedded = format!("not {PASS_MARKER}\n");
        assert!(verdict("x", Expect::Pass, true, &embedded).is_err());
    }
}
