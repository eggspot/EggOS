//! The UDCalc boot test (D410), on aarch64 and x86_64: one boot of the release kernel with a
//! blank disk and a host folder holding UDCalc's release archive (`udcalc_app.rs`). The shell
//! installs it with `app install` and opens it with `app open`, so its window appears (the
//! layer loading its Sparkle framework from the bundle, D409, and its nibs). Typing
//! `12+30=` on the keyboard makes the display read `42`, which `wm show` finds in the window's
//! element tree; `wm menu --choose` opens `About UDCalc`, Esc closes it, and Cmd+Q quits the
//! app, after which `wm list` has none of its windows.

use super::settings::{Boot, Screen};
use super::win32_app::{SUSPEND, changed, picture, tap, type_line, whole, window_area};
use super::*;
use crate::udcalc_app::{APP, ARCHIVE, VERSION};
use eggos_drivers::desktop::Profile;

const TEST_DISK_BYTES: u64 = 64 << 20;
const MEMORY: &str = "1G";
const TIMEOUT: Duration = Duration::from_secs(2400);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(300);
const SETTLE: Duration = Duration::from_secs(4);
const ECHO_PAUSE: Duration = Duration::from_secs(3);
const KEY_PAUSE: Duration = Duration::from_millis(400);
const POLL: Duration = Duration::from_secs(2);
const STATUS_PREFIX: &str = "UDCALC-APP ";
const PANEL_PREFIX: &str = "About ";
const TYPED: [&[&str]; 6] = [
    &["1"],
    &["2"],
    &["shift", "equal"],
    &["3"],
    &["0"],
    &["equal"],
];
const RESULT: &str = "42";
const POWER_OFF: &str = "poweroff\r";

fn install_line() -> String {
    format!("app install /media/host/{ARCHIVE}; echo {STATUS_PREFIX}install $?\r")
}

fn open_line() -> String {
    format!("app list; cd /tmp; app open {APP}; echo {STATUS_PREFIX}open $?\r")
}

fn list_line(step: &str) -> String {
    format!("wm list; echo {STATUS_PREFIX}{step} $?\r")
}

fn show_line(window: u32, step: &str) -> String {
    format!("wm show {window}; echo {STATUS_PREFIX}{step} $?\r")
}

fn about_line(window: u32) -> String {
    format!("wm menu --choose {window} {APP} '{PANEL_PREFIX}{APP}'; echo {STATUS_PREFIX}about $?\r")
}

fn listed_windows(console: &str, step: &str) -> Vec<(u32, String)> {
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
            let id = fields.first()?.parse().ok()?;
            Some((id, fields.get(7)?.trim().to_owned()))
        })
        .collect()
}

fn window_titled(console: &str, step: &str, title: &str) -> Option<u32> {
    listed_windows(console, step)
        .into_iter()
        .find(|(_, listed)| listed == title)
        .map(|(id, _)| id)
}

fn shown_tree(console: &str, step: &str) -> Vec<String> {
    let marker = format!("{STATUS_PREFIX}{step} 0");
    let Some(end) = console.rfind(&marker) else {
        return Vec::new();
    };
    console[..end]
        .lines()
        .rev()
        .take_while(|line| !line.contains("wm show"))
        .map(|line| line.trim().to_owned())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Whether the tree shows the result: an element whose name or value is exactly `42`.
fn shows_result(lines: &[String]) -> bool {
    lines.iter().any(|line| {
        line.split('"')
            .skip(1)
            .step_by(2)
            .any(|quoted| quoted.trim() == RESULT)
    })
}

fn wait_for_window(
    boot: &mut Boot<'_>,
    screen: &mut Screen,
    before: &Picture,
) -> Result<(), String> {
    let arch = boot.arch;
    let started = Instant::now();
    loop {
        boot.console.extend(boot.output.try_iter().flatten());
        let now = picture(screen, arch, "-udcalc-app")?;
        if window_area(before, &now).is_some() {
            return Ok(());
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

/// Installs, opens and uses UDCalc as the module's description says.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-udcalc-app-disk.img", arch.name));
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
    let mut screen = match Screen::new(&boot, Profile::Windows) {
        Ok(screen) => screen,
        Err(error) => return Err(boot.fail(error)),
    };
    type_line(&mut boot, &install_line())?;
    boot.wait_for_prompts(2)?;
    std::thread::sleep(ECHO_PAUSE);
    let before = picture(&mut screen, arch, "-udcalc-app-desktop")?;
    type_line(&mut boot, &open_line())?;
    boot.wait_for_prompts(3)?;
    wait_for_window(&mut boot, &mut screen, &before)?;
    std::thread::sleep(SETTLE);
    for keys in TYPED {
        tap(&mut screen, keys)?;
        std::thread::sleep(KEY_PAUSE);
    }
    std::thread::sleep(SETTLE);
    let typed = picture(&mut screen, arch, "-udcalc-app-typed")?;
    type_line(&mut boot, SUSPEND)?;
    boot.wait_for_prompts(4)?;
    type_line(&mut boot, &format!("bg; {}", list_line("listed")))?;
    boot.wait_for_prompts(5)?;
    let console = console_text(&mut boot);
    let Some(window) = window_titled(&console, "listed", APP) else {
        return Err(boot.fail(format!("{}: `wm list` does not list {APP}", arch.name)));
    };
    type_line(&mut boot, &show_line(window, "tree"))?;
    boot.wait_for_prompts(6)?;
    let console = console_text(&mut boot);
    if !shows_result(&shown_tree(&console, "tree")) {
        return Err(boot.fail(format!(
            "{}: after typing 12+30=, `wm show {window}` has no element reading {RESULT}",
            arch.name
        )));
    }
    type_line(&mut boot, &about_line(window))?;
    boot.wait_for_prompts(7)?;
    std::thread::sleep(SETTLE);
    type_line(&mut boot, &list_line("panel"))?;
    boot.wait_for_prompts(8)?;
    let console = console_text(&mut boot);
    let title = format!("{PANEL_PREFIX}{APP}");
    if window_titled(&console, "panel", &title).is_none() {
        return Err(boot.fail(format!("{}: `wm list` does not list `{title}`", arch.name)));
    }
    tap(&mut screen, &["esc"])?;
    std::thread::sleep(SETTLE);
    tap(&mut screen, &["meta_l", "q"])?;
    std::thread::sleep(SETTLE);
    let quit = picture(&mut screen, arch, "-udcalc-app-quit")?;
    if changed(&typed, &quit, whole(&quit)) == 0 {
        return Err(boot.fail(format!("{}: Cmd+Q left {APP} on the screen", arch.name)));
    }
    type_line(&mut boot, &list_line("after"))?;
    boot.wait_for_prompts(9)?;
    let console = console_text(&mut boot);
    if window_titled(&console, "after", APP).is_some() {
        return Err(boot.fail(format!("{}: {APP}'s window outlived Cmd+Q", arch.name)));
    }
    type_line(&mut boot, POWER_OFF)?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    eprintln!("xtask: {}: UDCalc boot test passed ({APP} {VERSION})", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_result_is_found_as_an_element_s_name_or_value() {
        let lines = vec![
            "1 window \"UDCalc\"".to_owned(),
            "  12 label \"42\" \"\"".to_owned(),
        ];
        assert!(shows_result(&lines));
        let other = vec!["  12 label \"420\" \"\"".to_owned()];
        assert!(!shows_result(&other));
    }
}
