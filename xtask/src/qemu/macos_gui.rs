//! The macOS GUI boot test (D147, D148): one boot of the release kernel with the macOS layer,
//! on a blank disk, with a host folder holding `mac-paint.zip`, the `mac-paint.app` bundle (our
//! own AppKit app, `tests/macos-gui/main.m`, with the icon xtask draws) archived, each frame
//! compared with the desktop the host renders from the same input, the app's window content and
//! alert painted by `eggos-macos-gui-tests`, its own icon made by the installer's own code.
//!
//! `app install /media/host/mac-paint.zip` in the terminal extracts and installs the bundle,
//! whose own icon then shows on the desktop and in the launcher's row; Cmd+Space,
//! `mac-paint` and Enter in the launcher open it: its fixed-size window, focused, with its main
//! menu's titles in the top bar; `Hello` and Enter in its text field rename the window; a click
//! in the paint view paints a square, `c` clears it; another click and the `Clear` button do the
//! same; `Info` opens the alert over the window (the menu titles dimmed), which Esc closes; a
//! click on the `File` title opens that menu and Esc closes it; Cmd+Q quits the app with status
//! 0, its window and menu gone.

use super::install::host_destroy;
use super::settings::{Boot, Screen};
use super::shell::Shell;
use super::*;
use eggos_app::icon::{self, OwnIcon, Workspace};
use eggos_app::reader::Bytes;
use eggos_app_manifest::{self as manifest, Manifest, Source};
use eggos_drivers::desktop::protocol::flags;
use eggos_drivers::desktop::{Action, Catalogue, Profile};
use eggos_macos_gui_tests::{self as app, CLEAR, CONTENT, INFO, MacPaint, NAME};

const TEST_DISK_BYTES: u64 = 64 << 20;
const FRAME_TIMEOUT: Duration = Duration::from_secs(20);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);
const APP_CLIENT: u16 = 0;
const ROLE_NORMAL: u32 = 0;
const ROLE_DIALOG: u32 = 1;
const DIALOG_FLAGS: u32 = 0;
const NO_PARENT: u32 = 0;
const FILE_MENU: usize = 1;
const TYPED_TITLE: &str = "Hello";
const FIRST_SQUARE: (i32, i32) = (100, 100);
const SECOND_SQUARE: (i32, i32) = (150, 120);
const INSTALLED_LINE: &str = "EGGOS-APP status 0";
const QUIT_STATUS: &[u8] = b"echo MAC-PAINT status $?\r";
const QUIT_LINE: &str = "MAC-PAINT status 0";
const POWER_OFF: &[u8] = b"poweroff\r";
const LETTERS: &str = "abcdefghijklmnopqrstuvwxyz";
const LETTER_CODES: [u16; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45,
    21, 44,
];

const CHECKS: [&str; 14] = [
    "the first prompt with the host share",
    "mac-paint.zip installed from the terminal: its own icon on the desktop",
    "the launcher filtered by `mac-paint`, its own icon in the row",
    "mac-paint's window opened from the launcher, its menu in the top bar",
    "`Hello` typed and Enter: the window titled Hello",
    "a click in the paint view: a blue square",
    "`c` in the paint view: the squares cleared",
    "a second click in the paint view: a blue square",
    "the Clear button: the squares cleared",
    "Info: the alert over the window, the menu titles dimmed",
    "Esc: the alert closed",
    "a click on the File title: the File menu open",
    "Esc: the File menu closed",
    "Cmd+Q: mac-paint quit, its window and menu gone",
];

/// The archive the host folder holds and the test installs.
pub fn archive_name() -> String {
    format!("{NAME}{}", crate::macos::ARCHIVE_SUFFIX)
}

fn expected_icon_file() -> Result<Vec<u8>, String> {
    let icns = crate::macos::test_icns()?;
    let mut work = Box::new(Workspace::new());
    let mut icon = OwnIcon::new();
    match icon::from_icns(&mut Bytes(&icns), &mut work, &mut icon) {
        true => Ok(icon.file().to_vec()),
        false => Err("the installer's code made no icon of the test app's .icns".to_owned()),
    }
}

fn program_inside_the_bundle() -> String {
    format!("{NAME}.app/Contents/MacOS/{NAME}")
}

fn install_line() -> String {
    format!(
        "app install /media/host/{}; echo EGGOS-APP status $?\r",
        archive_name()
    )
}

fn install_output() -> [String; 2] {
    [
        format!("Installed {NAME} in /Applications/{NAME}."),
        INSTALLED_LINE.to_owned(),
    ]
}

fn launched_program() -> String {
    format!("/Applications/{NAME}/{}", program_inside_the_bundle())
}

fn manifest_of(arch: &Arch) -> Vec<u8> {
    let run = program_inside_the_bundle();
    let entry = Manifest {
        name: NAME,
        source: Source::Macos,
        run: &run,
        architecture: Some(arch.name),
        synonyms: "",
        icon: Some(manifest::icon::FILE_NAME),
    };
    let mut file = [0u8; manifest::LARGEST];
    let length = manifest::write(&entry, &mut file).expect("mac-paint's manifest is valid");
    file[..length].to_vec()
}

fn catalogue(arch: &Arch, installed: bool) -> Result<Catalogue, String> {
    let mut catalogue = Catalogue::new();
    catalogue.set_host_share(true);
    if installed {
        catalogue.add_installed(NAME.as_bytes(), &manifest_of(arch));
        if !catalogue.set_icon(NAME.as_bytes(), &expected_icon_file()?) {
            return Err(format!(
                "{}: the host's catalogue refused mac-paint's icon",
                arch.name
            ));
        }
    }
    Ok(catalogue)
}

fn key_of(character: char) -> Option<(&'static str, u16)> {
    if character == '-' {
        return Some(("minus", key::MINUS));
    }
    let index = LETTERS.find(character.to_ascii_lowercase())?;
    Some((&LETTERS[index..index + 1], LETTER_CODES[index]))
}

fn window_content(paint: &MacPaint) -> WindowContent {
    let (width, height) = CONTENT;
    let mut pixels = vec![0; (width * height) as usize];
    paint.paint(&mut pixels);
    WindowContent::Pixels { width, pixels }
}

fn alert_content() -> WindowContent {
    let (width, height) = app::alert().size();
    let mut pixels = vec![0; (width * height) as usize];
    app::paint_alert(&mut pixels);
    WindowContent::Pixels { width, pixels }
}

fn lines_in_order(console: &str, wanted: &[String]) -> Result<(), String> {
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    let mut at = 0;
    for line in wanted {
        match lines[at..].iter().position(|candidate| candidate == line) {
            Some(found) => at += found + 1,
            None => return Err(format!("no `{line}` line where expected")),
        }
    }
    Ok(())
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    let mut wanted = install_output().to_vec();
    wanted.push(QUIT_LINE.to_owned());
    wanted.push(POWER_DOWN_LINE.to_owned());
    lines_in_order(console, &wanted)
        .map_err(|problem| format!("{arch}: the macOS GUI test's console has {problem}"))?;
    let program = launched_program();
    if !console
        .lines()
        .any(|line| line.trim_end().ends_with(&program))
    {
        return Err(format!(
            "{arch}: the terminal does not show `{program}` typed by the launcher"
        ));
    }
    match clean_exit {
        true => Ok(()),
        false => Err(format!(
            "{arch}: QEMU did not exit cleanly after the macOS GUI test"
        )),
    }
}

struct MacGui<'a> {
    shell: Shell<'a>,
    paint: MacPaint,
    window: Option<WindowId>,
    alert: Option<WindowId>,
    checked: usize,
}

impl MacGui<'_> {
    fn arch(&self) -> &'static str {
        self.shell.arch()
    }

    fn refused(&self, what: &str, error: impl std::fmt::Debug) -> String {
        format!(
            "{}: the host's desktop refused {what}: {error:?}",
            self.arch()
        )
    }

    fn check(&mut self, timeout: Duration) -> Result<(), String> {
        let at = CHECKS
            .get(self.checked)
            .copied()
            .ok_or_else(|| format!("{}: more checks than CHECKS lists", self.arch()))?;
        self.checked += 1;
        let mut windows = Vec::new();
        if let Some(window) = self.window {
            windows.push((window, window_content(&self.paint)));
        }
        if let Some(alert) = self.alert {
            windows.push((alert, alert_content()));
        }
        let shell = &mut self.shell;
        check_desktop_frame_within(
            shell.boot.arch,
            &mut shell.screen.qmp,
            &mut shell.boot.console,
            &shell.boot.output,
            at,
            &shell.screen.desktop,
            &windows,
            timeout,
        )
    }

    fn type_on_console(&mut self, line: &[u8]) -> Result<(), String> {
        let arch = self.arch();
        self.shell
            .boot
            .keyboard
            .write_all(line)
            .and_then(|()| self.shell.boot.keyboard.flush())
            .map_err(|e| format!("{arch}: could not type on the console: {e}"))
    }

    fn type_keys(&mut self, text: &str) -> Result<(), String> {
        for character in text.chars() {
            let (qcode, code) = key_of(character)
                .ok_or_else(|| format!("{}: no key types {character:?}", self.arch()))?;
            let shifted = character.is_ascii_uppercase();
            let held = match shifted {
                true => Modifiers::SHIFT,
                false => Modifiers::NONE,
            };
            if shifted {
                self.shell
                    .key("shift", key::LEFT_SHIFT, true, Modifiers::NONE)?;
            }
            self.shell.tap(qcode, code, held)?;
            if shifted {
                self.shell
                    .key("shift", key::LEFT_SHIFT, false, Modifiers::NONE)?;
            }
        }
        Ok(())
    }

    fn command_key(&mut self, character: char) -> Result<(), String> {
        let (qcode, code) = key_of(character)
            .ok_or_else(|| format!("{}: no key types {character:?}", self.arch()))?;
        let command = Modifiers::SUPER;
        self.shell.key("meta_l", key::LEFT_SUPER, true, command)?;
        self.shell.tap(qcode, code, command)?;
        self.shell
            .key("meta_l", key::LEFT_SUPER, false, Modifiers::NONE)?;
        Ok(())
    }

    fn create(
        &mut self,
        role: u32,
        parent: u32,
        size: (u32, u32),
        window_flags: u32,
    ) -> Result<WindowId, String> {
        let created = self
            .shell
            .screen
            .desktop
            .create(APP_CLIENT, role, parent, size.0, size.1, window_flags)
            .map_err(|error| self.refused("a window", error))?;
        let id = u32::from(created.window);
        self.shell
            .screen
            .desktop
            .set_title(APP_CLIENT, id, NAME.as_bytes(), &mut Damage::new())
            .map_err(|error| self.refused("a title", error))?;
        self.shell
            .screen
            .desktop
            .commit(APP_CLIENT, id, &[], &mut Damage::new(), &mut DiscardEvents)
            .map_err(|error| self.refused("a commit", error))?;
        Ok(created.window)
    }

    fn window(&self) -> Result<WindowId, String> {
        self.window
            .ok_or_else(|| format!("{}: mac-paint's window is not open", self.arch()))
    }

    fn click_content(&mut self, (x, y): (i32, i32)) -> Result<(), String> {
        let window = self.window()?;
        let content = self
            .shell
            .screen
            .desktop
            .content_rect(window)
            .ok_or_else(|| format!("{}: mac-paint's window is gone", self.arch()))?;
        let at = Rect::new(content.x + x as u32, content.y + y as u32, 1, 1);
        self.shell.click(at)?;
        self.shell.screen.park_pointer()
    }

    fn install_from_the_terminal(&mut self, arch: &Arch) -> Result<(), String> {
        self.shell.screen.park_pointer()?;
        self.check(FRAME_TIMEOUT)?;
        let prompts = self.shell.boot.prompts();
        self.type_on_console(install_line().as_bytes())?;
        self.shell.wait_for_line(INSTALLED_LINE)?;
        self.shell.boot.wait_for_prompts(prompts + 1)?;
        self.shell.screen.desktop.set_catalogue(
            catalogue(arch, true)?,
            &mut Damage::new(),
            &mut DiscardEvents,
        );
        self.check(FRAME_TIMEOUT)
    }

    fn open_from_the_launcher(&mut self) -> Result<(), String> {
        let id = self
            .shell
            .screen
            .desktop
            .catalogue()
            .installed_named(NAME)
            .ok_or_else(|| format!("{}: the host's catalogue has no {NAME}", self.arch()))?;
        let command = Modifiers::SUPER;
        self.shell.key("meta_l", key::LEFT_SUPER, true, command)?;
        self.shell.tap("spc", key::SPACE, command)?;
        self.shell
            .key("meta_l", key::LEFT_SUPER, false, Modifiers::NONE)?;
        self.type_keys(NAME)?;
        self.check(FRAME_TIMEOUT)?;
        let action = self.shell.tap("ret", key::ENTER, Modifiers::NONE)?;
        self.shell
            .expect(action, Action::Type(id), "Enter in the launcher")?;
        let window = self.create(ROLE_NORMAL, NO_PARENT, CONTENT, flags::FIXED_SIZE)?;
        self.shell
            .screen
            .desktop
            .set_menu(
                APP_CLIENT,
                u32::from(window),
                app::menu_pool().as_bytes(),
                &mut Damage::new(),
            )
            .map_err(|error| self.refused("the main menu", error))?;
        self.window = Some(window);
        self.check(WINDOW_TIMEOUT)
    }

    fn rename_from_the_field(&mut self) -> Result<(), String> {
        let window = self.window()?;
        self.type_keys(TYPED_TITLE)?;
        self.paint.type_text(TYPED_TITLE);
        self.shell.tap("ret", key::ENTER, Modifiers::NONE)?;
        self.paint.enter();
        let title = self.paint.title;
        self.shell
            .screen
            .desktop
            .set_title(
                APP_CLIENT,
                u32::from(window),
                title.as_str().as_bytes(),
                &mut Damage::new(),
            )
            .map_err(|error| self.refused("the new title", error))?;
        self.check(FRAME_TIMEOUT)
    }

    fn paint_and_clear(&mut self) -> Result<(), String> {
        self.click_content(FIRST_SQUARE)?;
        self.paint.click_paint(FIRST_SQUARE.0, FIRST_SQUARE.1);
        self.check(FRAME_TIMEOUT)?;
        let (qcode, code) = key_of('c').ok_or("no key types c")?;
        self.shell.tap(qcode, code, Modifiers::NONE)?;
        self.paint.clear();
        self.check(FRAME_TIMEOUT)?;
        self.click_content(SECOND_SQUARE)?;
        self.paint.click_paint(SECOND_SQUARE.0, SECOND_SQUARE.1);
        self.check(FRAME_TIMEOUT)?;
        self.click_content(MacPaint::centre_of(CLEAR))?;
        self.paint.clear();
        self.check(FRAME_TIMEOUT)
    }

    fn alert_and_escape(&mut self) -> Result<(), String> {
        let window = self.window()?;
        self.click_content(MacPaint::centre_of(INFO))?;
        let alert = self.create(
            ROLE_DIALOG,
            u32::from(window),
            app::alert().size(),
            DIALOG_FLAGS,
        )?;
        self.alert = Some(alert);
        self.paint.key = false;
        self.check(WINDOW_TIMEOUT)?;
        self.shell.tap("esc", key::ESC, Modifiers::NONE)?;
        host_destroy(&mut self.shell, APP_CLIENT, alert)?;
        self.alert = None;
        self.paint.key = true;
        self.check(FRAME_TIMEOUT)
    }

    fn file_menu(&mut self) -> Result<(), String> {
        let title = self.shell.rect(
            self.shell.screen.desktop.menu_title(FILE_MENU),
            "File title",
        )?;
        self.shell.click(title)?;
        self.shell.screen.park_pointer()?;
        self.shell
            .rect(self.shell.screen.desktop.main_menu_area(), "open File menu")?;
        self.check(FRAME_TIMEOUT)?;
        self.shell.tap("esc", key::ESC, Modifiers::NONE)?;
        self.check(FRAME_TIMEOUT)
    }

    fn quit(&mut self) -> Result<(), String> {
        let window = self.window()?;
        let prompts = self.shell.boot.prompts();
        self.command_key('q')?;
        host_destroy(&mut self.shell, APP_CLIENT, window)?;
        self.window = None;
        self.shell.boot.wait_for_prompts(prompts + 1)?;
        self.type_on_console(QUIT_STATUS)?;
        self.shell.wait_for_line(QUIT_LINE)?;
        self.shell.boot.wait_for_prompts(prompts + 2)?;
        self.check(FRAME_TIMEOUT)
    }

    fn run(&mut self, arch: &Arch) -> Result<(), String> {
        self.install_from_the_terminal(arch)?;
        self.open_from_the_launcher()?;
        self.rename_from_the_field()?;
        self.paint_and_clear()?;
        self.alert_and_escape()?;
        self.file_menu()?;
        self.quit()?;
        match self.checked == CHECKS.len() {
            true => Ok(()),
            false => Err(format!(
                "{}: {} of the {} checks ran",
                self.arch(),
                self.checked,
                CHECKS.len()
            )),
        }
    }
}

/// Runs the macOS GUI boot test (see the module documentation) on `kernel`, a release kernel
/// for `arch` with the macOS layer, with `folder` (the host folder holding `mac-paint.zip`)
/// attached.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-macos-gui-disk.img", arch.name));
    reset_disk_image(&image, TEST_DISK_BYTES)?;
    let mut args = disk_args(arch, &image.to_string_lossy());
    args.extend(host_folder_args(arch, &folder.to_string_lossy()));
    args.extend(
        crate::macos::qemu_cpu_args(arch.name)
            .iter()
            .map(|arg| arg.to_string()),
    );
    let mut boot = Boot::start(arch, kernel, &args)?;
    boot.wait_for_prompts(1)?;
    let mut screen = match Screen::new(&boot, Profile::Macos) {
        Ok(screen) => screen,
        Err(error) => return Err(boot.fail(error)),
    };
    let empty = match catalogue(arch, false) {
        Ok(empty) => empty,
        Err(error) => return Err(boot.fail(error)),
    };
    screen
        .desktop
        .set_catalogue(empty, &mut Damage::new(), &mut DiscardEvents);
    let mut gui = MacGui {
        shell: Shell {
            boot,
            screen,
            now_ms: 0,
        },
        paint: MacPaint::new(),
        window: None,
        alert: None,
        checked: 0,
    };
    let stepped = gui.run(arch);
    let MacGui {
        shell: Shell { mut boot, .. },
        ..
    } = gui;
    if let Err(error) = stepped {
        return Err(boot.fail(error));
    }
    boot.keyboard
        .write_all(POWER_OFF)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    eprintln!("xtask: {}: macOS GUI boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arch(name: &str) -> &'static Arch {
        crate::ARCHES
            .iter()
            .find(|arch| arch.name == name)
            .expect("an architecture")
    }

    #[test]
    fn the_expected_manifest_and_catalogue_are_those_the_installer_writes_for_the_bundle() {
        assert_eq!(
            manifest_of(arch("x86_64")),
            b"eggos-app 1\nname=mac-paint\nsource=macos\n\
              run=mac-paint.app/Contents/MacOS/mac-paint\narchitecture=x86_64\nicon=app.icon\n"
        );
        assert!(
            String::from_utf8(manifest_of(arch("aarch64")))
                .unwrap()
                .ends_with("architecture=aarch64\nicon=app.icon\n")
        );
        let before = catalogue(arch("aarch64"), false).unwrap();
        assert!(before.host_share());
        assert_eq!(before.installed_count(), 0);
        let after = catalogue(arch("aarch64"), true).unwrap();
        let id = after.installed_named(NAME).expect("mac-paint is installed");
        let Some(eggos_drivers::desktop::Launch::Type(line)) = after.launch(id) else {
            panic!("an installed app is typed in the terminal");
        };
        assert_eq!(
            line.as_bytes(),
            b"\x15/Applications/mac-paint/mac-paint.app/Contents/MacOS/mac-paint\r"
        );
        assert!(launched_program().ends_with("Contents/MacOS/mac-paint"));
        assert!(matches!(
            after.icon(id),
            eggos_drivers::desktop::Icon::Own(_)
        ));
        let icon = expected_icon_file().unwrap();
        assert_eq!(icon.len(), manifest::icon::FILE_BYTES);
        assert_ne!(icon[..4], icon[24 * 4..24 * 4 + 4]);
    }

    #[test]
    fn the_session_installs_the_bundle_and_the_verdict_needs_each_line_in_order() {
        assert_eq!(archive_name(), "mac-paint.zip");
        assert_eq!(
            install_line(),
            "app install /media/host/mac-paint.zip; echo EGGOS-APP status $?\r"
        );
        let ok = [
            "eggsh:/# app install /media/host/mac-paint.zip; echo EGGOS-APP status $?",
            "Installed mac-paint in /Applications/mac-paint.",
            "EGGOS-APP status 0",
            "eggsh:/# /Applications/mac-paint/mac-paint.app/Contents/MacOS/mac-paint",
            "eggsh:/# echo MAC-PAINT status $?",
            "MAC-PAINT status 0",
            POWER_DOWN_LINE,
        ]
        .join("\n");
        assert_eq!(verdict("x86_64", &ok, true), Ok(()));
        assert!(verdict("x86_64", &ok, false).is_err());
        let failed = ok.replace("MAC-PAINT status 0", "MAC-PAINT status 1");
        assert!(verdict("x86_64", &failed, true).is_err());
        let not_launched = ok.replace("/Applications/mac-paint/mac-paint.app", "/bin");
        assert!(verdict("x86_64", &not_launched, true).is_err());
        let reordered = ok.replace(
            "Installed mac-paint in /Applications/mac-paint.\nEGGOS-APP status 0",
            "EGGOS-APP status 0\nInstalled mac-paint in /Applications/mac-paint.",
        );
        assert!(verdict("x86_64", &reordered, true).is_err());
    }

    #[test]
    fn every_typed_character_has_a_key_and_the_checks_are_in_order() {
        for character in NAME.chars().chain(TYPED_TITLE.chars()).chain(['c', 'q']) {
            assert!(key_of(character).is_some(), "{character:?}");
        }
        assert_eq!(key_of('c'), Some(("c", 46)));
        assert_eq!(key_of('q'), Some(("q", 16)));
        assert_eq!(key_of('H'), Some(("h", key::H)));
        assert_eq!(key_of('a'), Some(("a", key::A)));
        assert_eq!(key_of('m'), Some(("m", key::M)));
        assert_eq!(key_of('-'), Some(("minus", key::MINUS)));
        assert_eq!(key_of('!'), None);
        assert_eq!(CHECKS.len(), 14);
        assert!(CHECKS[1].contains("installed"));
        assert!(CHECKS[3].contains("launcher"));
        assert!(CHECKS[9].starts_with("Info"));
        assert!(CHECKS[13].starts_with("Cmd+Q"));
    }

    #[test]
    fn the_host_paints_the_window_and_the_alert_at_their_sizes() {
        let WindowContent::Pixels { width, pixels } = window_content(&MacPaint::new()) else {
            panic!("the window is pixels");
        };
        assert_eq!((width, pixels.len()), (400, 400 * 300));
        let mut painted = MacPaint::new();
        painted.click_paint(FIRST_SQUARE.0, FIRST_SQUARE.1);
        let mut empty = MacPaint::new();
        empty.field_focused = false;
        assert_ne!(window_content(&painted), window_content(&empty));
        painted.clear();
        assert_eq!(window_content(&painted), window_content(&empty));
        let WindowContent::Pixels { width, pixels } = alert_content() else {
            panic!("the alert is pixels");
        };
        assert_eq!((width, pixels.len()), (400, 400 * 112));
        assert_eq!(flags::FIXED_SIZE, 1);
    }
}
