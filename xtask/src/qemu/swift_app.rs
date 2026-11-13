//! The Swift app boot test (D389), on aarch64 and x86_64: one boot of the release kernel with
//! a blank disk and a host folder holding Minesweeper's archive (`swift_app.rs`). The shell
//! installs it with `app install` and opens it with `app open`; its window must appear and be
//! listed by `wm list`; a click on the middle of the board must change it (a cell opened),
//! choosing the game menu's new game must change it again, and Cmd+Q must leave none of its
//! windows.

use super::settings::{Boot, Screen};
use super::win32_app::{SUSPEND, changed, picture, tap, type_line, window_area};
use super::*;
use crate::swift_app::{APP, ARCHIVE};
use eggos_drivers::desktop::Profile;

const TEST_DISK_BYTES: u64 = 64 << 20;
const MEMORY: &str = "1G";
const TIMEOUT: Duration = Duration::from_secs(2400);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(300);
const SETTLE: Duration = Duration::from_secs(4);
const ECHO_PAUSE: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_secs(2);
const POINTER_PAUSE: Duration = Duration::from_millis(500);
const STATUS_PREFIX: &str = "SWIFT-APP ";
const POWER_OFF: &str = "poweroff\r";
const NEW_GAME_KEYS: [&str; 2] = ["meta_l", "n"];

fn install_line() -> String {
    format!("app install /media/host/{ARCHIVE}; echo {STATUS_PREFIX}install $?\r")
}

fn open_line() -> String {
    format!("app list; cd /tmp; app open {APP}; echo {STATUS_PREFIX}open $?\r")
}

fn list_line(step: &str) -> String {
    format!("wm list; echo {STATUS_PREFIX}{step} $?\r")
}

fn listed_titles(console: &str, step: &str) -> Vec<String> {
    let marker = format!("{STATUS_PREFIX}{step} 0");
    let Some(end) = console.rfind(&marker) else {
        return Vec::new();
    };
    let before = &console[..end];
    let Some(start) = before.rfind(WM_LIST_HEADER) else {
        return Vec::new();
    };
    before[start + WM_LIST_HEADER.len()..]
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.trim().splitn(8, ' ').collect();
            Some(fields.get(7)?.trim().to_owned())
        })
        .collect()
}

fn wait_for_window(
    boot: &mut Boot<'_>,
    screen: &mut Screen,
    before: &Picture,
) -> Result<Rect, String> {
    let arch = boot.arch;
    let started = Instant::now();
    loop {
        boot.console.extend(boot.output.try_iter().flatten());
        let now = picture(screen, arch, "-swift-app")?;
        if let Some(area) = window_area(before, &now) {
            return Ok(area);
        }
        if started.elapsed() > WINDOW_TIMEOUT {
            return Err(boot.fail(format!(
                "{}: no {APP} window appeared within {}s of `app open {APP}`",
                arch.name,
                WINDOW_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(POLL);
    }
}

fn settled(boot: &mut Boot<'_>, screen: &mut Screen, suffix: &str) -> Result<Picture, String> {
    std::thread::sleep(SETTLE);
    screen.park_pointer()?;
    std::thread::sleep(POINTER_PAUSE);
    picture(screen, boot.arch, suffix)
}

fn click(screen: &mut Screen, (x, y): (u32, u32)) -> Result<(), String> {
    screen.pointer(x, y, None)?;
    screen.pointer(x, y, Some(true))?;
    screen.pointer(x, y, Some(false))
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    let installed = format!("Installed {APP} in /Applications/{APP}.");
    let opened = format!("Opened {APP}.");
    for wanted in [
        installed.as_str(),
        &format!("{STATUS_PREFIX}install 0"),
        opened.as_str(),
        &format!("{STATUS_PREFIX}open 0"),
        &format!("{STATUS_PREFIX}listed 0"),
        &format!("{STATUS_PREFIX}after 0"),
        POWER_DOWN_LINE,
    ] {
        if !has_line(console, wanted) {
            return Err(format!("{arch}: the console has no `{wanted}` line"));
        }
    }
    if !listed_titles(console, "listed")
        .iter()
        .any(|title| title == APP)
    {
        return Err(format!("{arch}: `wm list` does not list {APP}'s window"));
    }
    if listed_titles(console, "after")
        .iter()
        .any(|title| title == APP)
    {
        return Err(format!(
            "{arch}: {APP}'s window is still listed after Cmd+Q"
        ));
    }
    match clean_exit {
        true => Ok(()),
        false => Err(format!(
            "{arch}: QEMU did not exit cleanly after the Swift app test"
        )),
    }
}

/// Runs the Swift app boot test (see the module documentation) on `kernel`, the release
/// kernel for `arch` with the layers, with the host folder `folder` holding the archive.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-swift-app-disk.img", arch.name));
    reset_disk_image(&image, TEST_DISK_BYTES)?;
    let mut args = disk_args(arch, &image.to_string_lossy());
    args.extend(host_folder_args(arch, &folder.to_string_lossy()));
    args.extend(["-m".to_owned(), MEMORY.to_owned()]);
    args.extend(
        crate::macos::qemu_cpu_args(arch.name)
            .iter()
            .map(|argument| argument.to_string()),
    );
    let mut boot = Boot::start(arch, kernel, &args)?.with_timeout(TIMEOUT);
    boot.wait_for_prompts(1)?;
    let mut screen = match Screen::new(&boot, Profile::Macos) {
        Ok(screen) => screen,
        Err(error) => return Err(boot.fail(error)),
    };
    type_line(&mut boot, &install_line())?;
    boot.wait_for_prompts(2)?;
    std::thread::sleep(ECHO_PAUSE);
    let before = picture(&mut screen, arch, "-swift-app-desktop")?;
    type_line(&mut boot, &open_line())?;
    let area = wait_for_window(&mut boot, &mut screen, &before)?;
    let shown = settled(&mut boot, &mut screen, "-swift-app-shown")?;
    let middle = (area.x + area.width / 2, area.y + area.height / 2);
    click(&mut screen, middle)?;
    let opened = settled(&mut boot, &mut screen, "-swift-app-opened")?;
    if changed(&shown, &opened, area) == 0 {
        return Err(boot.fail(format!(
            "{}: a click on the middle of {APP}'s board changed nothing",
            arch.name
        )));
    }
    tap(&mut screen, &NEW_GAME_KEYS)?;
    let renewed = settled(&mut boot, &mut screen, "-swift-app-new-game")?;
    if changed(&opened, &renewed, area) == 0 {
        return Err(boot.fail(format!(
            "{}: Cmd+N did not start a new game of {APP}",
            arch.name
        )));
    }
    type_line(&mut boot, SUSPEND)?;
    boot.wait_for_prompts(3)?;
    type_line(&mut boot, &format!("bg; {}", list_line("listed")))?;
    boot.wait_for_prompts(4)?;
    click(&mut screen, (area.x + area.width / 2, area.y + 8))?;
    tap(&mut screen, &["meta_l", "q"])?;
    std::thread::sleep(SETTLE);
    type_line(&mut boot, &list_line("after"))?;
    boot.wait_for_prompts(5)?;
    type_line(&mut boot, POWER_OFF)?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    eprintln!("xtask: {}: Swift app boot test passed ({APP})", arch.name);
    Ok(())
}
