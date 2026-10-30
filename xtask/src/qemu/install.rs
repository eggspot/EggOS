//! The install boot test (D134): two boots of the release image `cargo xtask release` ships,
//! which carries the Windows layer (D152), on a blank disk, with a host folder holding
//! `win-paint.exe` (our own Windows GUI program) and our static Linux program, each frame of the second boot compared with the desktop the host
//! renders from the same input, and the disk image read back with `eggos-eggfs` after each.
//!
//! 1. `app install /media/host/win-paint.exe` installs it into `/Applications/win-paint/`
//!    (on riscv64, where the Windows layer does not run, it is refused with the reason and the
//!    Linux program is installed instead); `app list` lists it; installing it again is refused;
//!    the machine powers off, and the image holds the program and its manifest, and no
//!    temporary manifest.
//! 2. The same disk boots again: the desktop shows the app's icon and the launcher its row
//!    beside `Install from host share...`; a double-click on the icon opens it through its
//!    layer, so win-paint's window appears (the Linux program runs in the terminal), and its
//!    close control closes it; the icon's menu's `Uninstall...` opens the installer's window,
//!    whose confirmation Enter accepts: the window closes, the icon goes and the launcher no
//!    longer lists the app; `ls /Applications` and `app list` show nothing, and after the
//!    power-off the image's `/Applications` is empty.

use super::settings::{Boot, Screen};
use super::shell::Shell;
use super::windows_gui::scene_content;
use super::*;
use eggos_app::dialog::{self as installer_window, AppIcon, Dialog};
use eggos_app_manifest::{self as manifest, Manifest, Source};
use eggos_drivers::desktop::{Action, AppId, Catalogue, Profile};
use eggos_drivers::input::button;
use eggos_eggfs::memory::MemoryDisk;
use eggos_eggfs::{Kind, Volume};
use eggos_windows_gui_tests::{self as scene, Scene};

const TEST_DISK_BYTES: u64 = 64 << 20;
const FRAME_TIMEOUT: Duration = Duration::from_secs(20);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);
const PROGRAM_CLIENT: u16 = 0;
const INSTALLER_CLIENT: u16 = 1;
const ROLE_NORMAL: u32 = 0;
const POWER_OFF: &[u8] = b"poweroff\r";
const LISTED_LINE: &str = "EGGOS-APP listed 0";
const LINUX_PROGRAM_LAST_LINE: &str = "LINUX-STATIC stderr";

/// The Windows program the host folder holds.
pub const WINDOWS_PROGRAM: &str = "win-paint.exe";
/// The Linux program the host folder holds.
pub const LINUX_PROGRAM: &str = "linux-static";

struct TestApp {
    file: &'static str,
    name: &'static str,
    source: Source,
    window: bool,
}

fn test_app(arch: &Arch) -> TestApp {
    match runs_windows_programs(arch.name) {
        true => TestApp {
            file: WINDOWS_PROGRAM,
            name: "win-paint",
            source: Source::Windows,
            window: true,
        },
        false => TestApp {
            file: LINUX_PROGRAM,
            name: LINUX_PROGRAM,
            source: Source::Linux,
            window: false,
        },
    }
}

fn manifest_of(arch: &Arch, app: &TestApp) -> Vec<u8> {
    let entry = Manifest {
        name: app.name,
        source: app.source,
        run: app.file,
        architecture: Some(arch.name),
        synonyms: "",
        icon: None,
        loader: None,
        libraries: None,
        arguments: None,
    };
    let mut file = [0u8; manifest::LARGEST];
    let length = manifest::write(&entry, &mut file).expect("the test app's manifest is valid");
    file[..length].to_vec()
}

fn catalogue(arch: &Arch, app: Option<&TestApp>) -> Catalogue {
    let mut catalogue = Catalogue::new();
    catalogue.set_host_share(true);
    if let Some(app) = app {
        catalogue.add_installed(app.name.as_bytes(), &manifest_of(arch, app));
    }
    catalogue
}

fn first_session(app: &TestApp) -> Vec<String> {
    let mut lines = Vec::new();
    if app.source != Source::Windows {
        lines.push(format!(
            "app install /media/host/{WINDOWS_PROGRAM}; echo EGGOS-APP refused $?\r"
        ));
    }
    lines.push(format!(
        "app install /media/host/{}; echo EGGOS-APP status $?\r",
        app.file
    ));
    lines.push("app list\r".to_owned());
    lines.push(format!(
        "app install /media/host/{}; echo EGGOS-APP again $?\r",
        app.file
    ));
    lines.push("poweroff\r".to_owned());
    lines
}

fn first_output(arch: &Arch, app: &TestApp) -> Vec<String> {
    let mut lines = Vec::new();
    if app.source != Source::Windows {
        lines.push(format!(
            "app: EggOS cannot run {WINDOWS_PROGRAM} on {}: the Windows layer runs on x86_64 \
             and aarch64 only",
            arch.name
        ));
        lines.push("EGGOS-APP refused 1".to_owned());
    }
    lines.push(format!("Installed {0} in /Applications/{0}.", app.name));
    lines.push("EGGOS-APP status 0".to_owned());
    lines.push(format!(
        "{0}\t{1}\t{2}\t/Applications/{0}/{3}\t-",
        app.name,
        app.source.key(),
        arch.name,
        app.file
    ));
    lines.push(format!(
        "app: could not install {}: it is already installed",
        app.name
    ));
    lines.push("EGGOS-APP again 1".to_owned());
    lines.push(POWER_DOWN_LINE.to_owned());
    lines
}

pub(super) fn image_volume(image: &Path) -> Result<Volume<MemoryDisk>, String> {
    let bytes = std::fs::read(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let (mut volume, _) = Volume::mount(MemoryDisk::from_image(bytes))
        .map_err(|(error, _)| format!("{}: {}", image.display(), error.description()))?;
    volume.check().map_err(|problem| {
        format!(
            "{}: the disk image is inconsistent: {problem}",
            image.display()
        )
    })?;
    Ok(volume)
}

pub(super) fn node_at(volume: &mut Volume<MemoryDisk>, path: &str) -> Result<Option<u64>, String> {
    let mut node = volume.root();
    for name in path.split('/').filter(|name| !name.is_empty()) {
        match volume
            .lookup(node, name.as_bytes())
            .map_err(|e| format!("{path}: {e:?}"))?
        {
            Some(found) => node = found,
            None => return Ok(None),
        }
    }
    Ok(Some(node))
}

pub(super) fn file_at(
    volume: &mut Volume<MemoryDisk>,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let Some(node) = node_at(volume, path)? else {
        return Ok(None);
    };
    if eggos_eggfs::kind_of(node) != Kind::Regular {
        return Err(format!("{path} is not a regular file"));
    }
    let mut contents = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = volume
            .read(node, contents.len() as u64, &mut chunk)
            .map_err(|e| format!("{path}: {e:?}"))?;
        if read == 0 {
            return Ok(Some(contents));
        }
        contents.extend_from_slice(&chunk[..read]);
    }
}

pub(super) fn entries_at(
    volume: &mut Volume<MemoryDisk>,
    path: &str,
) -> Result<Vec<String>, String> {
    let node = node_at(volume, path)?.ok_or_else(|| format!("no {path}"))?;
    let mut names = Vec::new();
    let mut position = 0;
    while let Some(entry) = volume
        .entry(node, position)
        .map_err(|e| format!("{path}: {e:?}"))?
    {
        names.push(String::from_utf8_lossy(entry.name()).into_owned());
        position += 1;
    }
    Ok(names)
}

fn check_installed_on_the_image(
    arch: &Arch,
    image: &Path,
    folder: &Path,
    app: &TestApp,
) -> Result<(), String> {
    let fail = |what: String| format!("{}: {what}", arch.name);
    let mut volume = image_volume(image).map_err(fail)?;
    let base = format!("Applications/{}", app.name);
    let mut entries = entries_at(&mut volume, &base).map_err(fail)?;
    entries.sort();
    let mut wanted = vec![app.file.to_owned(), manifest::FILE_NAME.to_owned()];
    wanted.sort();
    if entries != wanted {
        return Err(fail(format!("/{base} holds {entries:?}, not {wanted:?}")));
    }
    let manifest =
        file_at(&mut volume, &format!("{base}/{}", manifest::FILE_NAME)).map_err(fail)?;
    if manifest.as_deref() != Some(&manifest_of(arch, app)[..]) {
        return Err(fail(format!(
            "the image's manifest is {:?}",
            manifest.map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        )));
    }
    let program = file_at(&mut volume, &format!("{base}/{}", app.file)).map_err(fail)?;
    let host = std::fs::read(folder.join(app.file)).map_err(|e| fail(e.to_string()))?;
    if program.as_deref() != Some(&host[..]) {
        return Err(fail(format!(
            "the image's {} differs from the host's",
            app.file
        )));
    }
    Ok(())
}

fn check_nothing_left_on_the_image(arch: &Arch, image: &Path) -> Result<(), String> {
    let fail = |what: String| format!("{}: {what}", arch.name);
    let mut volume = image_volume(image).map_err(fail)?;
    let entries = entries_at(&mut volume, "Applications").map_err(fail)?;
    match entries.is_empty() {
        true => Ok(()),
        false => Err(fail(format!(
            "/Applications still holds {entries:?} after the uninstall"
        ))),
    }
}

fn check_lines_in_order(arch: &Arch, console: &str, wanted: &[String]) -> Result<(), String> {
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    let mut at = 0;
    for line in wanted {
        match lines[at..].iter().position(|candidate| candidate == line) {
            Some(found) => at += found + 1,
            None => {
                return Err(format!(
                    "{}: the install test's console has no `{line}` line where expected",
                    arch.name
                ));
            }
        }
    }
    Ok(())
}

fn install_boot(arch: &Arch, kernel: &Path, args: &[String], app: &TestApp) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, args)?;
    let session = first_session(app);
    let session: Vec<&[u8]> = session.iter().map(|line| line.as_bytes()).collect();
    for _ in 0..session.len() {
        boot.type_next(&session)?;
    }
    let (console, clean_exit) = boot.finish()?;
    check_lines_in_order(arch, &console, &first_output(arch, app))?;
    match clean_exit {
        true => Ok(()),
        false => Err(format!(
            "{}: QEMU did not exit cleanly after the install",
            arch.name
        )),
    }
}

fn dialog_content(dialog: &Dialog) -> WindowContent {
    let (width, height) = installer_window::SIZE;
    let mut pixels = vec![0; (width * height) as usize];
    dialog.widgets().render(&mut pixels, width, height, 1);
    WindowContent::Pixels { width, pixels }
}

fn check_with(
    shell: &mut Shell<'_>,
    at: &str,
    windows: &[(WindowId, WindowContent)],
    timeout: Duration,
) -> Result<(), String> {
    check_desktop_frame_within(
        shell.boot.arch,
        &mut shell.screen.qmp,
        &mut shell.boot.console,
        &shell.boot.output,
        at,
        &shell.screen.desktop,
        windows,
        timeout,
    )
}

pub(super) fn host_window(
    shell: &mut Shell<'_>,
    client: u16,
    title: &str,
    size: (u32, u32),
    minimum: Option<(u32, u32)>,
) -> Result<WindowId, String> {
    let arch = shell.arch();
    let refused = |error| format!("{arch}: the host's desktop refused a window: {error:?}");
    let desktop = &mut shell.screen.desktop;
    let created = desktop
        .create(client, ROLE_NORMAL, 0, size.0, size.1, 0)
        .map_err(refused)?;
    let id = u32::from(created.window);
    desktop
        .set_title(client, id, title.as_bytes(), &mut Damage::new())
        .map_err(refused)?;
    if let Some((width, height)) = minimum {
        desktop
            .set_minimum_size(client, id, width, height)
            .map_err(refused)?;
    }
    desktop
        .commit(client, id, &[], &mut Damage::new(), &mut DiscardEvents)
        .map_err(refused)?;
    Ok(created.window)
}

pub(super) fn host_destroy(
    shell: &mut Shell<'_>,
    client: u16,
    window: WindowId,
) -> Result<(), String> {
    let arch = shell.arch();
    shell
        .screen
        .desktop
        .destroy(
            client,
            u32::from(window),
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .map_err(|error| format!("{arch}: the host's desktop refused to destroy: {error:?}"))
}

fn open_launcher(shell: &mut Shell<'_>) -> Result<(), String> {
    let command = Modifiers::SUPER;
    shell.key("meta_l", key::LEFT_SUPER, true, command)?;
    shell.tap("spc", key::SPACE, command)?;
    shell.key("meta_l", key::LEFT_SUPER, false, Modifiers::NONE)?;
    Ok(())
}

fn right_click(shell: &mut Shell<'_>, rect: Rect) -> Result<(), String> {
    let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
    shell.screen.pointer(x, y, None)?;
    for down in [true, false] {
        shell.screen.qmp.send_input(&[InputEvent::Button {
            button: "right",
            down,
        }])?;
        std::thread::sleep(DEVICE_PAUSE);
        shell
            .screen
            .desktop
            .button(button::RIGHT, down, &mut Damage::new(), &mut DiscardEvents);
    }
    Ok(())
}

fn open_app(shell: &mut Shell<'_>, app: &TestApp, id: AppId) -> Result<(), String> {
    let icon = shell.rect(shell.screen.desktop.desktop_icon(id), "app's desktop icon")?;
    let prompts = shell.boot.prompts();
    let action = shell.double_click(icon)?;
    shell.expect(action, Action::Type(id), "the app's desktop icon")?;
    shell.screen.park_pointer()?;
    if !app.window {
        shell.wait_for_line(LINUX_PROGRAM_LAST_LINE)?;
        shell.boot.wait_for_prompts(prompts + 1)?;
        return shell.check("the Linux app run in the terminal from its desktop icon");
    }
    let client = (scene::CLIENT.0 as u32, scene::CLIENT.1 as u32);
    let window = host_window(shell, PROGRAM_CLIENT, scene::TITLE, client, None)?;
    check_with(
        shell,
        "win-paint's window opened from its desktop icon",
        &[(window, scene_content(&Scene::new()))],
        WINDOW_TIMEOUT,
    )?;
    let close = shell.rect(
        shell.screen.desktop.close_control(window),
        "win-paint's close control",
    )?;
    shell.click(close)?;
    host_destroy(shell, PROGRAM_CLIENT, window)?;
    shell.screen.park_pointer()?;
    shell.boot.wait_for_prompts(prompts + 1)?;
    shell.check("win-paint closed by its close control")
}

fn uninstall_from_the_icon_menu(
    shell: &mut Shell<'_>,
    arch: &Arch,
    app: &TestApp,
    id: AppId,
) -> Result<(), String> {
    let icon = shell.rect(shell.screen.desktop.desktop_icon(id), "app's desktop icon")?;
    right_click(shell, icon)?;
    check_with(
        shell,
        "the app's menu with Uninstall...",
        &[],
        FRAME_TIMEOUT,
    )?;
    let menu = shell.rect(shell.screen.desktop.icon_menu_area(), "icon menu")?;
    let item = Rect::new(menu.x, menu.y + 1 + 32, menu.width, 32);
    let action = shell.click(item)?;
    shell.expect(action, Action::Uninstall(id), "the menu's Uninstall...")?;
    let (width, height) = installer_window::SIZE;
    let window = host_window(
        shell,
        INSTALLER_CLIENT,
        installer_window::UNINSTALL_TITLE,
        (width, height),
        Some((width, height)),
    )?;
    let dialog = Dialog::confirm_uninstall(AppIcon::of(app.name, Some(app.source)), app.name);
    shell.screen.park_pointer()?;
    check_with(
        shell,
        "the installer's window asking to uninstall",
        &[(window, dialog_content(&dialog))],
        WINDOW_TIMEOUT,
    )?;
    shell.tap("ret", key::ENTER, Modifiers::NONE)?;
    host_destroy(shell, INSTALLER_CLIENT, window)?;
    shell.screen.desktop.set_catalogue(
        catalogue(arch, None),
        &mut Damage::new(),
        &mut DiscardEvents,
    );
    check_with(
        shell,
        "the app uninstalled: its window, icon and menu gone",
        &[],
        WINDOW_TIMEOUT,
    )
}

fn desktop_steps(shell: &mut Shell<'_>, arch: &Arch, app: &TestApp) -> Result<(), String> {
    shell.screen.park_pointer()?;
    shell.check("the installed app's icon on the desktop after the reboot")?;
    let id = shell
        .screen
        .desktop
        .catalogue()
        .installed_named(app.name)
        .ok_or_else(|| format!("{}: the host's catalogue has no {}", arch.name, app.name))?;
    open_launcher(shell)?;
    shell.check("the launcher listing the app and Install from host share...")?;
    shell.tap("esc", key::ESC, Modifiers::NONE)?;
    open_app(shell, app, id)?;
    uninstall_from_the_icon_menu(shell, arch, app, id)?;
    open_launcher(shell)?;
    shell.check("the launcher without the app")?;
    shell.tap("esc", key::ESC, Modifiers::NONE)?;
    let prompts = shell.boot.prompts();
    shell
        .boot
        .keyboard
        .write_all(b"ls /Applications; app list; echo EGGOS-APP listed $?\r")
        .and_then(|()| shell.boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    shell.wait_for_line(LISTED_LINE)?;
    shell.boot.wait_for_prompts(prompts + 1)
}

fn open_and_uninstall_boot(
    arch: &Arch,
    kernel: &Path,
    args: &[String],
    app: &TestApp,
) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, args)?;
    boot.wait_for_prompts(1)?;
    let mut screen = match Screen::new(&boot, Profile::Macos) {
        Ok(screen) => screen,
        Err(error) => return Err(boot.fail(error)),
    };
    screen.desktop.set_catalogue(
        catalogue(arch, Some(app)),
        &mut Damage::new(),
        &mut DiscardEvents,
    );
    let mut shell = Shell {
        boot,
        screen,
        now_ms: 0,
    };
    let stepped = desktop_steps(&mut shell, arch, app);
    let Shell { mut boot, .. } = shell;
    if let Err(error) = stepped {
        return Err(boot.fail(error));
    }
    boot.keyboard
        .write_all(POWER_OFF)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    let (console, clean_exit) = boot.finish()?;
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    let listed_at = lines
        .iter()
        .position(|line| *line == LISTED_LINE)
        .ok_or_else(|| format!("{}: no `{LISTED_LINE}` line", arch.name))?;
    let before = lines[..listed_at]
        .iter()
        .rev()
        .take_while(|line| !line.contains("ls /Applications; app list"))
        .count();
    if before != 0 {
        return Err(format!(
            "{}: `ls /Applications` or `app list` still printed {} lines after the uninstall",
            arch.name, before
        ));
    }
    if !lines.contains(&POWER_DOWN_LINE) || !clean_exit {
        return Err(format!(
            "{}: QEMU did not power off cleanly after the uninstall",
            arch.name
        ));
    }
    Ok(())
}

/// Runs the install boot test (see the module documentation) on `kernel`, the release image
/// for `arch` (with the Windows layer where it runs, D152), with `folder` (the host folder
/// holding [`WINDOWS_PROGRAM`] and [`LINUX_PROGRAM`]) attached.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    let app = test_app(arch);
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-install-disk.img", arch.name));
    reset_disk_image(&image, TEST_DISK_BYTES)?;
    let mut args = disk_args(arch, &image.to_string_lossy());
    args.extend(host_folder_args(arch, &folder.to_string_lossy()));
    install_boot(arch, kernel, &args, &app)?;
    check_installed_on_the_image(arch, &image, folder, &app)?;
    open_and_uninstall_boot(arch, kernel, &args, &app)?;
    check_nothing_left_on_the_image(arch, &image)?;
    eprintln!("xtask: {}: install boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expected_manifest_and_catalogue_are_those_the_installer_writes() {
        let arch = crate::ARCHES
            .iter()
            .find(|arch| arch.name == "x86_64")
            .expect("x86_64 is an architecture");
        let app = test_app(arch);
        assert_eq!(
            manifest_of(arch, &app),
            b"eggos-app 1\nname=win-paint\nsource=windows\nrun=win-paint.exe\narchitecture=x86_64\n"
        );
        let catalogue = catalogue(arch, Some(&app));
        assert!(catalogue.installed_named("win-paint").is_some());
        assert!(catalogue.host_share());
        let session = first_session(&app);
        assert_eq!(session.len(), 4);
        assert!(first_output(arch, &app)[0].starts_with("Installed win-paint in"));
        let riscv = crate::ARCHES
            .iter()
            .find(|arch| arch.name == "riscv64")
            .expect("riscv64 is an architecture");
        let linux = test_app(riscv);
        assert_eq!(linux.source, Source::Linux);
        assert_eq!(first_session(&linux).len(), 5);
    }
}
