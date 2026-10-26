//! Booting kernel images in QEMU. What the boot tests run, the console lines they match (which
//! must change with the kernel's and the test programs' own) and why QEMU gets the arguments it
//! does: docs/build.md, "Developer commands".

use crate::qmp::{self, InputEvent, Picture, Qmp};
use crate::{Arch, elf};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
}

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
    b"echo a | b; echo EGGSH-TEST never\r",
    b"echo EGGSH-TEST after unsupported $?\r",
    b"eggos-std-selftest shell-child wait-for-interrupt\r",
    b"echo EGGSH-TEST interrupted $?\r",
    b"\x03",
    b"echo EGGSH-TEST prompt interrupt $?\r",
    b"exit 7\r",
];

const PROMPT_SESSION: [&[u8]; 9] = [
    b"ls /\r",
    b"ls -l /bin/ls; ls /nope; echo EGGOS-COREUTILS ls status $?\r",
    b"cat /etc/motd /etc/hostname\r",
    UNAME_LINE,
    b"/bin/echo -n EGGOS-COREUTILS external; /bin/echo ' echo'\r",
    b"sleep 0; sleep 0.05; echo EGGOS-COREUTILS slept $?\r",
    b"/bin/false; echo EGGOS-COREUTILS false $?\r",
    SHIFTED_KEYS_LINE,
    b"poweroff\r",
];

const UNAME_LINE: &[u8] = b"uname -a\r";

const SHIFTED_KEYS_LINE: &[u8] = b"echo EGGOS-KEYBOARD Typed_On+Virtio:OK\r";

/// The session lines the boot to the prompt types on the virtio keyboard through QMP (D113)
/// rather than on the serial console, when the machine has one.
const KEYBOARD_LINES: [&[u8]; 2] = [UNAME_LINE, SHIFTED_KEYS_LINE];

/// How long the runner waits between the keys it types through QMP, so the device's event
/// queue (64 events, about 3 to 5 per key) never fills between two of the kernel's polls.
const KEY_PAUSE: Duration = Duration::from_millis(10);

/// The line the input self-test writes for the runner's pointer and key events. Must match
/// `eggos_kernel::devices::selftest::INPUT_EVENTS_LINE`.
const INPUT_EVENTS_LINE: &str = "EGGOS-INPUT: send pointer and key events";

/// The tablet position and the mouse motion the runner sends. Must match the self-test's.
const TABLET_POSITION: (i32, i32) = (8192, 24576);
const MOUSE_MOTION: (i32, i32) = (7, -5);

/// How long the runner waits between the reports it sends to different input devices, so the
/// kernel sees them in order.
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
        whole(&["bin", "etc", "init", "usr"]),
        vec![
            Line::Around("-rwxr-xr-x 11 0 0 ", " /bin/ls"),
            Line::Whole("ls: cannot access '/nope': No such file or directory".into()),
            Line::Whole("EGGOS-COREUTILS ls status 2".into()),
        ],
        whole(&["Welcome to EggOS.", "eggos"]),
        vec![Line::Whole(format!(
            "Linux localhost 6.1.0-eggos #1 SMP EggOS {} {arch} EggOS",
            env!("CARGO_PKG_VERSION")
        ))],
        whole(&["EGGOS-COREUTILS external echo"]),
        whole(&["EGGOS-COREUTILS slept 0"]),
        whole(&["EGGOS-COREUTILS false 1"]),
        whole(&["EGGOS-KEYBOARD Typed_On+Virtio:OK"]),
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
}

const SHELL_LINES: [&str; 19] = [
    "EGGSH-TEST quoting single  quoted double / $HOME",
    "EGGSH-TEST variables abcabcdef x y x y",
    "EGGSH-TEST cd /usr/libexec /",
    "EGGSH-TEST home /",
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
    "eggsh: `|` is not supported yet",
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

/// The first frame the release kernel shows at 1280x800, as `eggos_kernel::display` draws it:
/// its pixel hash, `FIRST_FRAME_HASH_1280X800`.
const FIRST_FRAME: (u32, u32, u64) = (1280, 800, 0x1125_09e9_e81d_2325);

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
    eprintln!(
        "xtask: booting {} in {} ({expect:?}, display {display:?})",
        arch.name, arch.qemu
    );
    let (display_args, input_args) = match display {
        Display::Attached => (arch.display_args, arch.input_args),
        Display::Absent => (&[][..], &[][..]),
    };
    let qmp_port = match (expect, display) {
        (Expect::Benchmarks, _) | (_, Display::Absent) => None,
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
        Expect::Benchmarks => BENCHMARK_BOOT_TIMEOUT,
        _ => BOOT_TIMEOUT,
    };
    let started = Instant::now();
    let mut console = Vec::new();
    let mut typed = [0; CONSOLE_SCRIPTS.len()];
    let mut shell_typed = ShellTyping::default();
    let mut first_prompt = None;
    let mut first_frame = None;
    let mut input_events_sent = 0;
    let mut qmp_errors = Vec::new();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        console.extend(rx.try_iter().flatten());
        let scripts = match expect {
            Expect::Init | Expect::InitRestart => Vec::new(),
            Expect::Shell => shell_scripts_to_type(
                &String::from_utf8_lossy(&console),
                &SHELL_SESSION,
                &mut shell_typed,
            ),
            Expect::BootToPrompt => {
                let text = String::from_utf8_lossy(&console);
                if first_prompt.is_none() && text.contains(SHELL_PROMPT) {
                    first_prompt = Some(started.elapsed());
                    if let Some(port) = qmp_port {
                        first_frame = Some(check_first_frame(arch, port));
                    }
                }
                shell_scripts_to_type(&text, &PROMPT_SESSION, &mut shell_typed)
            }
            _ => scripts_to_type(&String::from_utf8_lossy(&console), &mut typed),
        };
        for script in scripts {
            if let Some(port) = qmp_port.filter(|_| KEYBOARD_LINES.contains(&script)) {
                let text = String::from_utf8_lossy(script);
                match Qmp::connect(port).and_then(|mut qmp| qmp.type_text(&text, KEY_PAUSE)) {
                    Ok(()) => eprintln!(
                        "xtask: {}: typed `{}` on the virtio keyboard",
                        arch.name,
                        text.trim_end()
                    ),
                    Err(e) => {
                        qmp_errors.push(format!("{}: could not type through QMP: {e}", arch.name))
                    }
                }
                continue;
            }
            if let Err(e) = keyboard.write_all(script).and_then(|()| keyboard.flush()) {
                eprintln!("xtask: {}: could not type on the console: {e}", arch.name);
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
    if let Some(first_frame) = first_frame {
        first_frame?;
    }
    if let Some(first_prompt) = first_prompt {
        eprintln!(
            "xtask: {}: boot to prompt in {} ms (QEMU started to the first `{SHELL_PROMPT}` prompt, {cpus} CPUs)",
            arch.name,
            first_prompt.as_millis()
        );
    }
    Ok(console)
}

fn count_lines(console: &str, wanted: &str) -> usize {
    console
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n') && line.trim_end() == wanted)
        .count()
}

/// The reports the input self-test waits for, each to one device: the tablet's position, a
/// left click, the mouse's motion, a wheel notch up, and Shift pressed and released.
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
    if !lines.contains(&BOOT_INPUT_LINE) {
        return Err(format!(
            "{arch}: no `{BOOT_INPUT_LINE}`: the virtio keyboard and pointers were not found"
        ));
    }
    for (typed, expected) in PROMPT_SESSION.iter().zip(prompt_session_output(arch)) {
        let command = String::from_utf8_lossy(typed);
        let command = command.trim_end_matches('\r');
        let Some(echoed) = lines
            .iter()
            .position(|line| line.starts_with(SHELL_PROMPT) && line.ends_with(command))
        else {
            return Err(format!("{arch}: `{command}` was not typed at a prompt"));
        };
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

/// Takes a picture of the display through QMP at `port` and checks it is the first frame.
fn check_first_frame(arch: &Arch, port: u16) -> Result<(), String> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::workspace_root().join("target"));
    std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    let path = target.join(format!("screendump-{}.ppm", arch.name));
    Qmp::connect(port)?.screendump(&path)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let picture = Picture::from_ppm(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let (width, height, hash) = FIRST_FRAME;
    if (picture.width, picture.height, picture.hash()) != (width, height, hash) {
        return Err(format!(
            "{}: the display shows a {}x{} picture with hash {:#x}, not the {width}x{height} \
             first frame ({hash:#x}); see {}",
            arch.name,
            picture.width,
            picture.height,
            picture.hash(),
            path.display()
        ));
    }
    eprintln!(
        "xtask: {}: the display shows the first frame ({width}x{height}, hash {hash:#x})",
        arch.name
    );
    Ok(())
}

/// Boots `kernel` with the console on this terminal, until the user quits QEMU; its display in
/// a window when `window` says so.
pub fn run_interactive(arch: &Arch, kernel: &Path, window: bool) -> Result<(), String> {
    let console: &[&str] = match window {
        true => &["-serial", "mon:stdio"],
        false => &["-nographic"],
    };
    let status = Command::new(arch.qemu)
        .args(arch.machine_args)
        .args(arch.display_args)
        .args(arch.input_args)
        .args(COMMON_ARGS)
        .arg("-smp")
        .arg(TEST_CPUS.to_string())
        .arg("-no-reboot")
        .args(console)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut console = format!("EggOS 0.2.0 ({arch})\r\n{BOOT_INPUT_LINE}\r\n{INIT_BANNER}\r\n");
        for (typed, written) in PROMPT_SESSION.iter().zip(prompt_session_output(arch)) {
            let command = String::from_utf8_lossy(typed);
            console.push_str(&format!("{SHELL_PROMPT}# {}\r\n", command.trim_end()));
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
        let reordered = ok.replace("bin\r\netc\r\n", "etc\r\nbin\r\n");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &reordered).is_err());
        let one_link = ok.replace("-rwxr-xr-x 11 ", "-rwxr-xr-x 1 ");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &one_link).is_err());
        let untyped = ok.replace("# uname -a", "# uname");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &untyped).is_err());
        let panicked = format!("{ok}Kernel panic - not syncing: Attempted to kill init!\r\n");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &panicked).is_err());
        let no_input = ok.replace(BOOT_INPUT_LINE, "boot: no input");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &no_input).is_err());
        let unshifted = ok.replace("Typed_On+Virtio:OK\r\n", "typed-on=virtio;ok\r\n");
        assert!(verdict("riscv64", Expect::BootToPrompt, true, &unshifted).is_err());
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
