//! The desktop shell boot test (D133): one boot of the release kernel on a blank disk, each
//! frame compared with the desktop the host renders from the same input. The shell writes an
//! app's manifest under `/Applications`; then, in the default macOS profile, the dock's
//! Settings icon opens Settings and its close control closes it; Cmd+Space opens the
//! launcher, which reads the catalogue again and lists the new app, typing `pref` leaves
//! Settings and Enter starts it; Settings' Windows card brings the taskbar, whose Start
//! button opens the launcher above it and whose items switch between the terminal and
//! Settings; the Linux card brings the top bar with `Activities`; a double-click on the
//! app's desktop icon runs its program in the terminal; the terminal minimises to a chip that
//! restores it.

use super::settings::{Boot, LINUX_CARD, Screen, WINDOWS_CARD};
use super::*;
use eggos_drivers::desktop::{Action, AppId, Catalogue, Profile};

const FRAME_TIMEOUT: Duration = Duration::from_secs(20);
const TEST_DISK_BYTES: u64 = 64 << 20;
const APP_NAME: &str = "Hello";
const APP_PROGRAM: &str = "/bin/uname";
const APP_OUTPUT: &str = "Linux";
const MANIFEST_WRITTEN: &str = "EGGOS-SHELL manifest 0";

const SESSION: [&[u8]; 1] = [b"mkdir /Applications/Hello; \
      echo 'eggos-app 1' > /Applications/Hello/app.manifest; \
      echo name=Hello >> /Applications/Hello/app.manifest; \
      echo source=linux >> /Applications/Hello/app.manifest; \
      echo run=/bin/uname >> /Applications/Hello/app.manifest; \
      echo EGGOS-SHELL manifest $?\r"];

const POWER_OFF: &[u8] = b"poweroff\r";

fn manifest() -> Vec<u8> {
    format!("eggos-app 1\nname={APP_NAME}\nsource=linux\nrun={APP_PROGRAM}\n").into_bytes()
}

fn catalogue_with_the_app() -> Catalogue {
    let mut catalogue = Catalogue::new();
    catalogue.add_installed(APP_NAME.as_bytes(), &manifest());
    catalogue
}

struct Shell<'a> {
    boot: Boot<'a>,
    screen: Screen,
    now_ms: u32,
}

impl Shell<'_> {
    fn arch(&self) -> &'static str {
        self.boot.arch.name
    }

    fn check(&mut self, at: &str) -> Result<(), String> {
        self.screen.check(&mut self.boot, at, FRAME_TIMEOUT)
    }

    fn click(&mut self, rect: Rect) -> Result<Action, String> {
        let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        let mut action = Action::None;
        for button in [None, Some(true), Some(false)] {
            let step = PointerStep { x, y, button };
            send_pointer_step(&mut self.screen.qmp, step, self.screen.size)?;
            if let Some(done) = apply_pointer_step(&mut self.screen.desktop, step)
                && done != Action::None
            {
                action = done;
            }
        }
        Ok(action)
    }

    fn double_click(&mut self, rect: Rect) -> Result<Action, String> {
        let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        send_pointer_step(
            &mut self.screen.qmp,
            PointerStep { x, y, button: None },
            self.screen.size,
        )?;
        apply_pointer_step(&mut self.screen.desktop, PointerStep { x, y, button: None });
        for down in [true, false, true, false] {
            self.screen.qmp.send_input(&[InputEvent::Button {
                button: "left",
                down,
            }])?;
            std::thread::sleep(DOUBLE_CLICK_PAUSE);
        }
        std::thread::sleep(DEVICE_PAUSE);
        self.now_ms += 1000;
        let mut action = Action::None;
        for (offset, down) in [(0, true), (40, false), (80, true), (120, false)] {
            self.screen.desktop.set_time(self.now_ms + offset);
            if let Some(done) = apply_pointer_step(
                &mut self.screen.desktop,
                PointerStep {
                    x,
                    y,
                    button: Some(down),
                },
            ) && done != Action::None
            {
                action = done;
            }
        }
        Ok(action)
    }

    fn key(
        &mut self,
        qcode: &'static str,
        code: u16,
        down: bool,
        held: Modifiers,
    ) -> Result<Action, String> {
        self.screen
            .qmp
            .send_input(&[InputEvent::Key { qcode, down }])?;
        std::thread::sleep(DEVICE_PAUSE);
        self.now_ms += 100;
        self.screen.desktop.set_time(self.now_ms);
        Ok(self
            .screen
            .desktop
            .key(code, down, held, &mut Damage::new(), &mut DiscardEvents))
    }

    fn tap(&mut self, qcode: &'static str, code: u16, held: Modifiers) -> Result<Action, String> {
        let action = self.key(qcode, code, true, held)?;
        self.key(qcode, code, false, held)?;
        Ok(action)
    }

    fn expect(&self, action: Action, wanted: Action, at: &str) -> Result<(), String> {
        match action == wanted {
            true => Ok(()),
            false => Err(format!(
                "{}: the host's desktop asked {action:?} at {at}, not {wanted:?}",
                self.arch()
            )),
        }
    }

    fn rect(&self, rect: Option<Rect>, what: &str) -> Result<Rect, String> {
        rect.ok_or_else(|| format!("{}: the host's desktop has no {what}", self.arch()))
    }

    fn wait_for_line(&mut self, wanted: &str) -> Result<(), String> {
        let started = Instant::now();
        loop {
            self.boot
                .console
                .extend(self.boot.output.try_iter().flatten());
            if has_line(&String::from_utf8_lossy(&self.boot.console), wanted) {
                return Ok(());
            }
            if started.elapsed() > FRAME_TIMEOUT {
                return Err(format!("{}: no `{wanted}` line", self.arch()));
            }
            std::thread::sleep(TERMINAL_FRAME_RETRY);
        }
    }
}

fn macos_steps(shell: &mut Shell<'_>) -> Result<(), String> {
    let arch = shell.arch();
    shell.screen.park_pointer()?;
    shell.check("the first prompt with the top bar, the dock and the desktop icons")?;
    let settings = shell.rect(
        shell.screen.desktop.dock_icon(AppId::Settings),
        "dock Settings",
    )?;
    let action = shell.click(settings)?;
    shell.expect(
        action,
        Action::Start(AppId::Settings),
        "the dock's Settings",
    )?;
    shell.screen.park_pointer()?;
    shell.screen.open_settings(arch, true)?;
    shell.check("Settings opened from the dock")?;
    shell.screen.close_settings(arch)?;
    shell.screen.park_pointer()?;
    shell.check("Settings closed")?;
    let command = Modifiers::SUPER;
    shell.key("meta_l", key::LEFT_SUPER, true, command)?;
    shell.tap("spc", key::SPACE, command)?;
    shell.key("meta_l", key::LEFT_SUPER, false, Modifiers::NONE)?;
    shell
        .screen
        .desktop
        .set_catalogue(catalogue_with_the_app(), &mut Damage::new());
    shell.check("the launcher opened by Cmd+Space, the new app listed")?;
    for (qcode, code) in [("p", 25), ("r", 19), ("e", 18), ("f", 33)] {
        shell.tap(qcode, code, Modifiers::NONE)?;
    }
    shell.check("the launcher filtered by `pref`")?;
    let action = shell.tap("ret", key::ENTER, Modifiers::NONE)?;
    shell.expect(
        action,
        Action::Start(AppId::Settings),
        "Enter in the launcher",
    )?;
    shell.screen.open_settings(arch, true)?;
    shell.check("Settings started from the launcher")
}

fn windows_steps(shell: &mut Shell<'_>) -> Result<(), String> {
    let arch = shell.arch();
    shell.screen.click_card(arch, WINDOWS_CARD)?;
    shell.screen.park_pointer()?;
    shell.check("the Windows profile: the taskbar")?;
    let start = shell.rect(shell.screen.desktop.start_button(), "Start button")?;
    shell.click(start)?;
    shell.check("Start opened the launcher above the taskbar")?;
    shell.tap("esc", key::ESC, Modifiers::NONE)?;
    let terminal = shell.screen.desktop.terminal_window();
    let item = shell.rect(
        shell.screen.desktop.window_entry(terminal),
        "terminal's taskbar item",
    )?;
    shell.click(item)?;
    shell.screen.park_pointer()?;
    shell.check("the terminal focused from its taskbar item")?;
    let settings = shell
        .screen
        .settings
        .as_ref()
        .map(|(id, _)| *id)
        .ok_or_else(|| format!("{arch}: Settings is not open"))?;
    let item = shell.rect(
        shell.screen.desktop.window_entry(settings),
        "Settings' taskbar item",
    )?;
    shell.click(item)?;
    shell.screen.park_pointer()?;
    shell.check("Settings focused from its taskbar item")
}

fn linux_steps(shell: &mut Shell<'_>) -> Result<(), String> {
    let arch = shell.arch();
    shell.screen.click_card(arch, LINUX_CARD)?;
    shell.screen.park_pointer()?;
    shell.check("the Linux profile: the top bar with Activities")?;
    shell.screen.close_settings(arch)?;
    shell.screen.park_pointer()?;
    shell.check("Settings closed in the Linux profile")?;
    let app = shell
        .screen
        .desktop
        .catalogue()
        .installed_named(APP_NAME)
        .ok_or_else(|| format!("{arch}: the host's catalogue has no {APP_NAME}"))?;
    let icon = shell.rect(shell.screen.desktop.desktop_icon(app), "app's desktop icon")?;
    let action = shell.double_click(icon)?;
    shell.expect(action, Action::Type(app), "the app's desktop icon")?;
    shell.screen.park_pointer()?;
    shell.wait_for_line(APP_OUTPUT)?;
    shell.boot.wait_for_prompts(3)?;
    shell.check("the app run in the terminal from its desktop icon")?;
    let minimise = shell.rect(
        shell
            .screen
            .desktop
            .control_of(shell.screen.desktop.terminal_window(), Part::Minimise),
        "terminal's minimise control",
    )?;
    shell.click(minimise)?;
    shell.screen.park_pointer()?;
    shell.check("the terminal minimised to its chip")?;
    let chip = shell.rect(
        shell
            .screen
            .desktop
            .window_entry(shell.screen.desktop.terminal_window()),
        "terminal's chip",
    )?;
    shell.click(chip)?;
    shell.screen.park_pointer()?;
    shell.check("the terminal restored from its chip")
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    for wanted in [MANIFEST_WRITTEN, APP_OUTPUT, POWER_DOWN_LINE] {
        if !lines.contains(&wanted) {
            return Err(format!(
                "{arch}: the shell test's console has no `{wanted}` line"
            ));
        }
    }
    if !lines.iter().any(|line| line.ends_with(APP_PROGRAM)) {
        return Err(format!(
            "{arch}: the terminal does not show `{APP_PROGRAM}` typed by the desktop icon"
        ));
    }
    if !clean_exit {
        return Err(format!(
            "{arch}: QEMU did not exit cleanly after the shell test"
        ));
    }
    Ok(())
}

/// Runs the desktop shell boot test (see the module documentation) on `kernel`, the release
/// kernel for `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-shell-disk.img", arch.name));
    reset_disk_image(&image, TEST_DISK_BYTES)?;
    let disk = disk_args(arch, &image.to_string_lossy());
    let mut boot = Boot::start(arch, kernel, &disk)?;
    boot.type_next(&SESSION)?;
    boot.wait_for_prompts(2)?;
    let screen = match Screen::new(&boot, Profile::Macos) {
        Ok(screen) => screen,
        Err(error) => return Err(boot.fail(error)),
    };
    let mut shell = Shell {
        boot,
        screen,
        now_ms: 0,
    };
    let stepped = shell
        .wait_for_line(MANIFEST_WRITTEN)
        .and_then(|()| macos_steps(&mut shell))
        .and_then(|()| windows_steps(&mut shell))
        .and_then(|()| linux_steps(&mut shell));
    let Shell { mut boot, .. } = shell;
    if let Err(error) = stepped {
        return Err(boot.fail(error));
    }
    boot.keyboard
        .write_all(POWER_OFF)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    eprintln!("xtask: {}: desktop shell boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_writes_the_manifest_the_host_catalogue_reads() {
        let typed = String::from_utf8_lossy(SESSION[0]);
        for line in String::from_utf8_lossy(&manifest()).lines() {
            assert!(typed.contains(line), "{line}");
        }
        let catalogue = catalogue_with_the_app();
        assert_eq!(catalogue.installed_count(), 1);
        assert!(catalogue.installed_named(APP_NAME).is_some());
    }
}
