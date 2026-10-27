//! The Settings boot test (D132): three boots of the release kernel with its display, each
//! frame compared with the desktop the host renders with the Settings window drawn by the
//! `eggos-settings` crate from the same input, and the disk image read back with `eggos-eggfs`:
//!
//! 1. Without a disk: `settings` opens its window, which says the choice lasts until power-off;
//!    a click on the Linux card moves every window's controls to the right; the close control,
//!    now on the right, closes Settings; `settings get` and `settings list` report the Linux
//!    profile, kept for the session in `/home`, which is in memory.
//! 2. On a blank disk: `settings` opens, a click on the Windows card switches the desktop, the
//!    close control closes it, and `poweroff` ends the boot; the image holds the settings file
//!    with the Windows profile.
//! 3. On the same disk: the desktop comes up in the Windows profile in its first frame;
//!    `settings set shell.profile macos` switches it back at once, and the image holds that;
//!    the command refuses an unknown profile and a read-only key.

use super::*;
use eggos_drivers::desktop::Profile;
use eggos_eggfs::memory::MemoryDisk;
use eggos_eggfs::{Kind, Volume};
use eggos_settings::facts::Facts;
use eggos_settings::settings_window::{DEFAULT_SIZE, Request, SettingsWindow, TITLE};
use eggos_toolkit::input::{Input, MouseButton};

const SETTINGS_TIMEOUT: Duration = Duration::from_secs(180);
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const TEST_DISK_BYTES: u64 = 64 << 20;
const CLIENT: u16 = 0;
const ROLE_NORMAL: u32 = 0;
const CARET_HIDDEN_MS: u32 = 500;
const WINDOWS_CARD: usize = 0;
const LINUX_CARD: usize = 2;

const SESSION_WITHOUT_DISK: [&[u8]; 4] = [
    b"settings\r",
    b"settings get shell.profile; settings list\r",
    b"ls /home/root/.config/eggos; cat /home/root/.config/eggos/settings\r",
    b"poweroff\r",
];

const SESSION_ON_A_BLANK_DISK: [&[u8]; 2] = [b"settings\r", b"poweroff\r"];

const SESSION_AFTER_THE_REBOOT: [&[u8]; 4] = [
    b"settings get shell.profile\r",
    b"settings set shell.profile macos; settings get shell.profile\r",
    b"settings set shell.profile amiga; echo EGGOS-SETTINGS status $?; settings set system.version 9; echo EGGOS-SETTINGS status $?\r",
    b"poweroff\r",
];

fn whole(lines: &[&str]) -> Vec<Line> {
    lines
        .iter()
        .map(|line| Line::Whole((*line).to_owned()))
        .collect()
}

fn output_without_disk(arch: &str) -> [Vec<Line>; SESSION_WITHOUT_DISK.len()] {
    [
        Vec::new(),
        vec![
            Line::Whole("linux".into()),
            Line::Whole("shell.profile=linux".into()),
            Line::Whole("display.scale=1".into()),
            Line::Around("display.resolution=", ""),
            Line::Whole("display.pointer=software".into()),
            Line::Whole("keyboard.layout=us".into()),
            Line::Whole(format!("system.version={}", env!("CARGO_PKG_VERSION"))),
            Line::Whole(format!("system.arch={arch}")),
            Line::Whole(format!("system.cpus={TEST_CPUS}")),
            Line::Around("system.memory_mb=", ""),
            Line::Around("system.uptime_s=", ""),
        ],
        whole(&["settings", "eggos-settings 1", "shell.profile=linux"]),
        whole(&[POWER_DOWN_LINE]),
    ]
}

fn output_after_the_reboot() -> [Vec<Line>; SESSION_AFTER_THE_REBOOT.len()] {
    [
        whole(&["windows"]),
        whole(&["macos"]),
        whole(&[
            "settings: shell.profile: invalid value 'amiga' (windows, macos or linux)",
            "EGGOS-SETTINGS status 2",
            "settings: system.version: read-only",
            "EGGOS-SETTINGS status 1",
        ]),
        whole(&[POWER_DOWN_LINE]),
    ]
}

struct Boot<'a> {
    arch: &'a Arch,
    child: std::process::Child,
    output: mpsc::Receiver<Vec<u8>>,
    keyboard: std::process::ChildStdin,
    console: Vec<u8>,
    port: u16,
    started: Instant,
    typed: usize,
}

impl<'a> Boot<'a> {
    fn start(arch: &'a Arch, kernel: &Path, args: &[String]) -> Result<Self, String> {
        eprintln!("xtask: booting {} in {} (settings)", arch.name, arch.qemu);
        let port = qmp::free_port()?;
        let cpus = TEST_CPUS.to_string();
        let mut child = Command::new(arch.qemu)
            .args(arch.machine_args)
            .args(arch.display_args)
            .args(arch.input_args)
            .args(qmp::qemu_args(port))
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
        let output = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
        let keyboard = child.stdin.take().expect("stdin is piped");
        Ok(Self {
            arch,
            child,
            output,
            keyboard,
            console: Vec::new(),
            port,
            started: Instant::now(),
            typed: 0,
        })
    }

    fn fail(&mut self, error: String) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        error
    }

    fn prompts(&self) -> usize {
        String::from_utf8_lossy(&self.console)
            .matches(SHELL_PROMPT)
            .count()
    }

    fn wait_for_prompts(&mut self, count: usize) -> Result<(), String> {
        loop {
            self.console.extend(self.output.try_iter().flatten());
            if self.prompts() >= count {
                return Ok(());
            }
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!(
                    "{}: QEMU ended ({status}) before prompt {count}",
                    self.arch.name
                ));
            }
            if banner_count(&String::from_utf8_lossy(&self.console)) > 1 {
                return Err(self.fail(format!(
                    "{}: the machine reset and booted again",
                    self.arch.name
                )));
            }
            if self.started.elapsed() > SETTINGS_TIMEOUT {
                return Err(self.fail(format!(
                    "{}: no prompt {count} within {}s",
                    self.arch.name,
                    SETTINGS_TIMEOUT.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn type_next(&mut self, session: &[&[u8]]) -> Result<(), String> {
        self.wait_for_prompts(self.typed + 1)?;
        let line = session[self.typed];
        self.typed += 1;
        self.keyboard
            .write_all(line)
            .and_then(|()| self.keyboard.flush())
            .map_err(|e| format!("{}: could not type on the console: {e}", self.arch.name))
    }

    fn finish(mut self) -> Result<(String, bool), String> {
        let status = loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            self.console.extend(self.output.try_iter().flatten());
            if self.started.elapsed() > SETTINGS_TIMEOUT {
                return Err(self.fail(format!(
                    "{}: no power-off within {}s",
                    self.arch.name,
                    SETTINGS_TIMEOUT.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        self.console.extend(self.output.iter().flatten());
        Ok((
            String::from_utf8_lossy(&self.console).into_owned(),
            status.success(),
        ))
    }
}

struct Screen {
    qmp: Qmp,
    desktop: Desktop,
    size: (u32, u32),
    settings: Option<(WindowId, SettingsWindow)>,
}

impl Screen {
    fn new(boot: &Boot<'_>, profile: Profile) -> Result<Self, String> {
        let mut qmp = Qmp::connect(boot.port)?;
        let size = screen_size(&mut qmp, boot.arch)?;
        let mut desktop = Desktop::new(size.0, size.1).ok_or_else(|| {
            format!(
                "{}: a {}x{} display holds no desktop",
                boot.arch.name, size.0, size.1
            )
        })?;
        desktop.set_profile(profile, &mut Damage::new());
        Ok(Self {
            qmp,
            desktop,
            size,
            settings: None,
        })
    }

    fn pointer(&mut self, x: u32, y: u32, button: Option<bool>) -> Result<(), String> {
        let step = PointerStep { x, y, button };
        send_pointer_step(&mut self.qmp, step, self.size)?;
        apply_pointer_step(&mut self.desktop, step);
        Ok(())
    }

    fn park_pointer(&mut self) -> Result<(), String> {
        self.pointer(self.size.0 - 2, self.size.1 - 2, None)
    }

    fn open_settings(&mut self, arch: &str, on_disk: bool) -> Result<(), String> {
        let refused = |error| format!("{arch}: the host's desktop refused Settings: {error:?}");
        let (width, height) = DEFAULT_SIZE;
        let created = self
            .desktop
            .create(CLIENT, ROLE_NORMAL, 0, width, height, 0)
            .map_err(refused)?;
        let id = u32::from(created.window);
        self.desktop
            .set_title(CLIENT, id, TITLE.as_bytes(), &mut Damage::new())
            .map_err(refused)?;
        self.desktop
            .commit(CLIENT, id, &[], &mut Damage::new(), &mut DiscardEvents)
            .map_err(refused)?;
        let facts = Facts {
            on_disk,
            profile: self.desktop.profile().number() as usize,
            ..Facts::example()
        };
        self.settings = Some((created.window, SettingsWindow::new(facts)));
        Ok(())
    }

    fn contents(&self) -> Vec<Vec<(WindowId, WindowContent)>> {
        let Some((id, settings)) = &self.settings else {
            return vec![Vec::new()];
        };
        let (width, height) = DEFAULT_SIZE;
        let render = |caret_shown: bool| {
            let mut tree = settings.widgets().clone();
            if !caret_shown {
                tree.tick(CARET_HIDDEN_MS);
            }
            let mut pixels = vec![0; (width * height) as usize];
            tree.render(&mut pixels, width, height, 1);
            vec![(*id, WindowContent::Pixels { width, pixels })]
        };
        vec![render(true), render(false)]
    }

    fn check(&mut self, boot: &mut Boot<'_>, at: &str, timeout: Duration) -> Result<(), String> {
        let contents = self.contents();
        let alternatives: Vec<&[(WindowId, WindowContent)]> =
            contents.iter().map(Vec::as_slice).collect();
        check_desktop_frame_among(
            boot.arch,
            &mut self.qmp,
            &mut boot.console,
            &boot.output,
            at,
            &self.desktop,
            &alternatives,
            timeout,
        )
    }

    fn click_card(&mut self, arch: &str, card: usize) -> Result<(), String> {
        let Some((id, settings)) = self.settings.as_mut() else {
            return Err(format!("{arch}: Settings is not open"));
        };
        let content = self
            .desktop
            .content_rect(*id)
            .ok_or_else(|| format!("{arch}: the Settings window is gone"))?;
        let rect = settings
            .card_rect(card)
            .ok_or_else(|| format!("{arch}: Settings has no card {card}"))?;
        let (x, y) = (rect.x + rect.width / 2, rect.y + 20);
        let mut chosen = None;
        for (input, button) in [
            (Input::PointerMove { x, y }, None),
            (
                Input::Button {
                    x,
                    y,
                    button: MouseButton::Left,
                    pressed: true,
                },
                Some(true),
            ),
            (
                Input::Button {
                    x,
                    y,
                    button: MouseButton::Left,
                    pressed: false,
                },
                Some(false),
            ),
        ] {
            let response = settings.tree().handle(input, 0);
            chosen = chosen.or(settings.respond(response, 0));
            let step = PointerStep {
                x: content.x + x as u32,
                y: content.y + y as u32,
                button,
            };
            send_pointer_step(&mut self.qmp, step, self.size)?;
            apply_pointer_step(&mut self.desktop, step);
        }
        let Some(Request::SetProfile(profile)) = chosen else {
            return Err(format!("{arch}: the host's Settings chose no profile"));
        };
        let profile = Profile::from_number(profile as u32)
            .ok_or_else(|| format!("{arch}: no profile {profile}"))?;
        self.desktop.set_profile(profile, &mut Damage::new());
        Ok(())
    }

    fn close_settings(&mut self, arch: &str) -> Result<(), String> {
        let Some((id, _)) = self.settings.take() else {
            return Err(format!("{arch}: Settings is not open"));
        };
        let close = self
            .desktop
            .close_control(id)
            .ok_or_else(|| format!("{arch}: the Settings window has no close control"))?;
        let (x, y) = (close.x + close.width / 2, close.y + close.height / 2);
        self.pointer(x, y, Some(true))?;
        self.pointer(x, y, Some(false))?;
        self.desktop
            .destroy(
                CLIENT,
                u32::from(id),
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(|error| format!("{arch}: the host's desktop refused a close: {error:?}"))
    }
}

fn settings_round(
    boot: &mut Boot<'_>,
    screen: &mut Screen,
    on_disk: bool,
    card: usize,
    profile_name: &str,
) -> Result<(), String> {
    let arch = boot.arch.name;
    screen.park_pointer()?;
    screen.open_settings(arch, on_disk)?;
    screen.check(
        boot,
        "Settings opened on the Appearance pane",
        FIRST_FRAME_TIMEOUT,
    )?;
    screen.click_card(arch, card)?;
    screen.check(
        boot,
        &format!("the {profile_name} profile chosen in Settings"),
        TERMINAL_FRAME_TIMEOUT,
    )?;
    screen.close_settings(arch)?;
    screen.check(
        boot,
        &format!("Settings closed in the {profile_name} profile"),
        TERMINAL_FRAME_TIMEOUT,
    )
}

fn settings_file(arch: &str, image: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let (mut volume, _) = Volume::mount(MemoryDisk::from_image(bytes))
        .map_err(|(error, _)| format!("{}: {}", image.display(), error.description()))?;
    volume
        .check()
        .map_err(|problem| format!("{arch}: the disk image is inconsistent: {problem}"))?;
    let mut node = volume.root();
    for name in eggos_settings_file::PATH
        .split('/')
        .filter(|name| !name.is_empty())
    {
        node = volume
            .lookup(node, name.as_bytes())
            .map_err(|e| format!("{arch}: {name}: {e:?}"))?
            .ok_or_else(|| {
                format!(
                    "{arch}: the disk image has no {}",
                    eggos_settings_file::PATH
                )
            })?;
    }
    if eggos_eggfs::kind_of(node) != Kind::Regular {
        return Err(format!("{arch}: the settings file is not a regular file"));
    }
    let mut contents = vec![0u8; eggos_settings_file::LARGEST];
    let read = volume
        .read(node, 0, &mut contents)
        .map_err(|e| format!("{arch}: the settings file: {e:?}"))?;
    contents.truncate(read);
    Ok(contents)
}

fn expect_saved(arch: &str, image: &Path, profile: &str) -> Result<(), String> {
    let contents = settings_file(arch, image)?;
    let wanted = format!("eggos-settings 1\nshell.profile={profile}\n");
    if contents != wanted.as_bytes() {
        return Err(format!(
            "{arch}: the disk's settings file holds {:?}, not {wanted:?}",
            String::from_utf8_lossy(&contents)
        ));
    }
    Ok(())
}

fn without_disk(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, &[])?;
    boot.type_next(&SESSION_WITHOUT_DISK)?;
    let mut screen = Screen::new(&boot, Profile::Macos)?;
    settings_round(&mut boot, &mut screen, false, LINUX_CARD, "Linux")?;
    for _ in 1..SESSION_WITHOUT_DISK.len() {
        boot.type_next(&SESSION_WITHOUT_DISK)?;
    }
    let (console, clean_exit) = boot.finish()?;
    session_verdict(
        arch.name,
        clean_exit,
        &console,
        &SESSION_WITHOUT_DISK,
        output_without_disk(arch.name),
        "settings (no disk)",
    )
}

fn on_a_blank_disk(
    arch: &Arch,
    kernel: &Path,
    disk: &[String],
    image: &Path,
) -> Result<(), String> {
    reset_disk_image(image, TEST_DISK_BYTES)?;
    let mut boot = Boot::start(arch, kernel, disk)?;
    boot.type_next(&SESSION_ON_A_BLANK_DISK)?;
    let mut screen = Screen::new(&boot, Profile::Macos)?;
    settings_round(&mut boot, &mut screen, true, WINDOWS_CARD, "Windows")?;
    boot.type_next(&SESSION_ON_A_BLANK_DISK)?;
    let (console, clean_exit) = boot.finish()?;
    session_verdict(
        arch.name,
        clean_exit,
        &console,
        &SESSION_ON_A_BLANK_DISK,
        [Vec::new(), whole(&[POWER_DOWN_LINE])],
        "settings (a blank disk)",
    )?;
    expect_saved(arch.name, image, "windows")
}

fn after_the_reboot(
    arch: &Arch,
    kernel: &Path,
    disk: &[String],
    image: &Path,
) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, disk)?;
    boot.wait_for_prompts(1)?;
    let mut screen = Screen::new(&boot, Profile::Windows)?;
    screen.check(
        &mut boot,
        "the first prompt, in the saved Windows profile",
        FIRST_FRAME_TIMEOUT,
    )?;
    boot.type_next(&SESSION_AFTER_THE_REBOOT)?;
    boot.type_next(&SESSION_AFTER_THE_REBOOT)?;
    boot.wait_for_prompts(3)?;
    screen
        .desktop
        .set_profile(Profile::Macos, &mut Damage::new());
    screen.check(
        &mut boot,
        "the macOS profile set by `settings set`",
        TERMINAL_FRAME_TIMEOUT,
    )?;
    for _ in 2..SESSION_AFTER_THE_REBOOT.len() {
        boot.type_next(&SESSION_AFTER_THE_REBOOT)?;
    }
    let (console, clean_exit) = boot.finish()?;
    session_verdict(
        arch.name,
        clean_exit,
        &console,
        &SESSION_AFTER_THE_REBOOT,
        output_after_the_reboot(),
        "settings (after the reboot)",
    )?;
    expect_saved(arch.name, image, "macos")
}

/// Runs the Settings boot test (see the module documentation) on `kernel`, the release kernel
/// for `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-settings-disk.img", arch.name));
    let disk = disk_args(arch, &image.to_string_lossy());
    without_disk(arch, kernel)?;
    on_a_blank_disk(arch, kernel, &disk, &image)?;
    after_the_reboot(arch, kernel, &disk, &image)?;
    eprintln!("xtask: {}: settings boot test passed", arch.name);
    Ok(())
}
