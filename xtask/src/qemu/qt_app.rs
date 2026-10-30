//! The Qt app boot test (D179), on x86_64 and aarch64: one boot of the release kernel with a
//! blank disk and a host folder holding `corepad.zip` (`qt_app.rs`). The shell installs it with
//! `app install --entry --environment --arguments`, which lists it and links its fonts into
//! `/Applications/.fonts`. It writes `alpha` into the note, and in `/tmp` `app open` opens
//! CorePad on it, as its icon does; its window appears on the display, a click on its title
//! bar focuses it, and keys edit the note: Ctrl+End and `beta`, then Ctrl+A and Ctrl+C copy
//! everything, Ctrl+End and Ctrl+V paste it at the end (the window changes each time). Ctrl+S
//! opens CorePad's Save As dialog, a second window of Qt's own file dialog, where Ctrl+A, the
//! new name and Return save the note as `saved.txt` beside it, and Ctrl+Q quits, so the
//! shell's prompt comes back. The saved file then holds `alpha` and `beta` twice, the copy
//! pasted right after the first `beta`, while the note still holds `alpha` alone; `wm list`
//! has no CorePad window, and after the power-off the disk's manifest names the loader, the
//! environment and the arguments.

use super::install::{file_at, image_volume};
use super::settings::{Boot, Screen};
use super::win32_app::{changed, picture, press, tap, type_line, whole, window_area};
use super::*;
use crate::qt_app::{
    APP, ARCHIVE, COREPAD_VERSION, LIBRARY_FOLDER, NOTE, QT_VERSION, SAVED_NAME, install_arguments,
    install_environment,
};
use eggos_drivers::desktop::Profile;

const TEST_DISK_BYTES: u64 = 512 << 20;
const MEMORY: &str = "2G";
const TIMEOUT: Duration = Duration::from_secs(2400);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(600);
const EXIT_TIMEOUT: Duration = Duration::from_secs(300);
const SETTLE: Duration = Duration::from_secs(20);
const STEP: Duration = Duration::from_secs(4);
const POLL: Duration = Duration::from_secs(2);
const TYPED_PIXELS: usize = 60;
const DIALOG_PIXELS: usize = 2000;
const TITLE_BAR_DEPTH: u32 = 14;
const STATUS_PREFIX: &str = "QT-APP ";
const WINDOW_TITLE: &str = "corepad";
const FIRST_TEXT: &str = "alpha";
const TYPED_TEXT: &str = "beta";
const FONTS_LINK_FOLDER: &str = "/Applications/.fonts";

fn saved_path() -> String {
    format!("/tmp/{SAVED_NAME}")
}

fn install_line() -> String {
    format!(
        "app install --entry {APP} --environment '{}' --arguments '{}' /media/host/{ARCHIVE}; \
         echo {STATUS_PREFIX}install $?\r",
        install_environment(),
        install_arguments()
    )
}

fn list_line() -> &'static str {
    "app list\r"
}

fn open_line() -> String {
    format!("echo {FIRST_TEXT} > {NOTE}; cd /tmp; app open {APP}; echo {STATUS_PREFIX}open $?\r")
}

fn after_line() -> String {
    format!(
        "cat {}; echo; echo {STATUS_PREFIX}saved $?; cat {NOTE}; echo {STATUS_PREFIX}note $?; \
         ls {FONTS_LINK_FOLDER}; wm list; echo {STATUS_PREFIX}after $?\r",
        saved_path()
    )
}

const POWER_OFF: &str = "poweroff\r";

fn listed_line(arch: &str) -> String {
    format!("{APP}\tlinux\t{arch}\t/Applications/{APP}/{APP}\t-")
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
        let now = picture(screen, arch, "-qt-app")?;
        if let Some(area) = window_area(before, &now) {
            return Ok(area);
        }
        if started.elapsed() > WINDOW_TIMEOUT {
            return Err(boot.fail(format!(
                "{}: no CorePad window appeared within {}s of `app open {APP}`",
                arch.name,
                WINDOW_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(POLL);
    }
}

fn chord(screen: &mut Screen, key: &'static str) -> Result<(), String> {
    tap(screen, &["ctrl", key])?;
    std::thread::sleep(STEP);
    Ok(())
}

fn wants_change(
    boot: &mut Boot<'_>,
    screen: &mut Screen,
    before: &Picture,
    (area, least): (Rect, usize),
    what: &str,
) -> Result<Picture, String> {
    let arch = boot.arch;
    std::thread::sleep(STEP);
    screen.park_pointer()?;
    let after = picture(screen, arch, "-qt-app-edited")?;
    let difference = changed(before, &after, area);
    match difference >= least {
        true => Ok(after),
        false => Err(boot.fail(format!(
            "{}: the display changed in {difference} pixels after {what}: the keys did not \
             reach CorePad, or it drew nothing new",
            arch.name
        ))),
    }
}

fn edit_copy_paste_and_save(
    boot: &mut Boot<'_>,
    screen: &mut Screen,
    area: Rect,
) -> Result<(), String> {
    let arch = boot.arch;
    std::thread::sleep(SETTLE);
    let title = (area.x + area.width / 2, area.y + TITLE_BAR_DEPTH);
    screen.pointer(title.0, title.1, None)?;
    screen.pointer(title.0, title.1, Some(true))?;
    screen.pointer(title.0, title.1, Some(false))?;
    screen.park_pointer()?;
    std::thread::sleep(STEP);
    let opened = picture(screen, arch, "-qt-app-opened")?;
    chord(screen, "end")?;
    for character in TYPED_TEXT.chars() {
        press(screen, character)?;
    }
    let typed = wants_change(boot, screen, &opened, (area, TYPED_PIXELS), "typing")?;
    chord(screen, "a")?;
    chord(screen, "c")?;
    chord(screen, "end")?;
    chord(screen, "v")?;
    let pasted = wants_change(boot, screen, &typed, (area, TYPED_PIXELS), "pasting")?;
    chord(screen, "s")?;
    std::thread::sleep(SETTLE);
    let display = whole(&pasted);
    wants_change(
        boot,
        screen,
        &pasted,
        (display, DIALOG_PIXELS),
        "Ctrl+S, which opens the Save As dialog",
    )?;
    chord(screen, "a")?;
    for character in SAVED_NAME.chars() {
        press(screen, character)?;
    }
    tap(screen, &["ret"])?;
    std::thread::sleep(SETTLE);
    chord(screen, "q")
}

fn wait_for_exit(boot: &mut Boot<'_>, prompts: usize) -> Result<(), String> {
    let started = Instant::now();
    loop {
        boot.console.extend(boot.output.try_iter().flatten());
        if boot.prompts() >= prompts {
            return Ok(());
        }
        if started.elapsed() > EXIT_TIMEOUT {
            return Err(boot.fail(format!(
                "{}: CorePad did not end within {}s of Ctrl+Q",
                boot.arch.name,
                EXIT_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn lines_between<'a>(console: &'a str, shown: &str, done: &str) -> Vec<&'a str> {
    console
        .lines()
        .skip_while(|line| !line.contains(shown))
        .skip(1)
        .take_while(|line| !line.starts_with(done))
        .collect()
}

fn saved_lines(console: &str) -> Vec<&str> {
    let shown = format!("cat {}", saved_path());
    lines_between(console, &shown, &format!("{STATUS_PREFIX}saved "))
}

fn note_lines(console: &str) -> Vec<&str> {
    let saved = format!("{STATUS_PREFIX}saved ");
    console
        .lines()
        .skip_while(|line| !line.starts_with(&saved))
        .skip(1)
        .take_while(|line| !line.starts_with(&format!("{STATUS_PREFIX}note ")))
        .collect()
}

fn listing_after(console: &str, header: &str) -> Vec<String> {
    let after = format!("{STATUS_PREFIX}after ");
    console
        .lines()
        .skip_while(|line| line.trim_end() != header)
        .take_while(|line| !line.starts_with(&after))
        .map(str::to_owned)
        .collect()
}

fn saved_problem(saved: &str) -> Option<String> {
    let firsts = saved.matches(FIRST_TEXT).count();
    let typed = saved.matches(TYPED_TEXT).count();
    let pasted = format!("{TYPED_TEXT}{FIRST_TEXT}");
    match (firsts, typed, saved.contains(&pasted)) {
        (2, 2, true) => None,
        _ => Some(format!(
            "{} holds {firsts} `{FIRST_TEXT}` and {typed} `{TYPED_TEXT}` (2 each, the copy \
             right after the first `{TYPED_TEXT}`, after copying, pasting and saving as): \
             {saved:?}",
            saved_path()
        )),
    }
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    let installed = format!("Installed {APP} in /Applications/{APP}.");
    let opened = format!("Opened {APP}.");
    for wanted in [
        installed.as_str(),
        &format!("{STATUS_PREFIX}install 0"),
        &listed_line(arch),
        opened.as_str(),
        &format!("{STATUS_PREFIX}open 0"),
        &format!("{STATUS_PREFIX}saved 0"),
        &format!("{STATUS_PREFIX}note 0"),
        &format!("{STATUS_PREFIX}after 0"),
        POWER_DOWN_LINE,
    ] {
        if !has_line(console, wanted) {
            return Err(format!("{arch}: the console has no `{wanted}` line"));
        }
    }
    if let Some(problem) = saved_problem(&saved_lines(console).join("\n")) {
        return Err(format!("{arch}: {problem}"));
    }
    let note: Vec<&str> = note_lines(console)
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if note != [FIRST_TEXT] {
        return Err(format!(
            "{arch}: {NOTE} changed, though CorePad saved as {SAVED_NAME}: {note:?}"
        ));
    }
    let fonts = format!("{STATUS_PREFIX}note 0");
    let linked = console
        .lines()
        .skip_while(|line| line.trim_end() != fonts)
        .take_while(|line| line.trim_end() != WM_LIST_HEADER)
        .any(|line| line.trim_end() == APP);
    if !linked {
        return Err(format!(
            "{arch}: `ls {FONTS_LINK_FOLDER}` does not list {APP}: its fonts were not linked"
        ));
    }
    let windows = listing_after(console, WM_LIST_HEADER);
    if windows.is_empty() {
        return Err(format!("{arch}: the console has no `wm list` listing"));
    }
    if let Some(left) = windows
        .iter()
        .find(|line| line.to_ascii_lowercase().contains(WINDOW_TITLE))
    {
        return Err(format!(
            "{arch}: a CorePad window is still listed after it ended: `{left}`"
        ));
    }
    match clean_exit {
        true => Ok(()),
        false => Err(format!(
            "{arch}: QEMU did not exit cleanly after the Qt app test"
        )),
    }
}

fn check_manifest(arch: &Arch, image: &Path) -> Result<(), String> {
    let fail = |what: String| format!("{}: {what}", arch.name);
    let mut volume = image_volume(image).map_err(fail)?;
    let manifest = file_at(&mut volume, &format!("Applications/{APP}/app.manifest"))
        .map_err(fail)?
        .ok_or_else(|| {
            fail(format!(
                "the disk holds no /Applications/{APP}/app.manifest"
            ))
        })?;
    let manifest = String::from_utf8_lossy(&manifest);
    for wanted in [
        format!("loader={LIBRARY_FOLDER}/ld-musl-{}.so.1", arch.name),
        format!("libraries={LIBRARY_FOLDER}"),
        format!("arguments={}", install_arguments()),
        format!("environment={}", install_environment()),
    ] {
        if !manifest.lines().any(|line| line == wanted) {
            return Err(fail(format!(
                "the installed manifest has no `{wanted}` line: {manifest:?}"
            )));
        }
    }
    Ok(())
}

/// Runs the Qt app boot test (see the module documentation) on `kernel`, the release kernel
/// for `arch`, with the host folder `folder` holding `corepad.zip`.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-qt-app-disk.img", arch.name));
    reset_disk_image(&image, TEST_DISK_BYTES)?;
    let mut args = disk_args(arch, &image.to_string_lossy());
    args.extend(host_folder_args(arch, &folder.to_string_lossy()));
    args.extend(["-m".to_owned(), MEMORY.to_owned()]);
    let mut boot = Boot::start(arch, kernel, &args)?.with_timeout(TIMEOUT);
    boot.wait_for_prompts(1)?;
    let mut screen = match Screen::new(&boot, Profile::Windows) {
        Ok(screen) => screen,
        Err(error) => return Err(boot.fail(error)),
    };
    type_line(&mut boot, &install_line())?;
    boot.wait_for_prompts(2)?;
    type_line(&mut boot, list_line())?;
    boot.wait_for_prompts(3)?;
    let before = picture(&mut screen, arch, "-qt-app-desktop")?;
    type_line(&mut boot, &open_line())?;
    let area = wait_for_window(&mut boot, &mut screen, &before)?;
    edit_copy_paste_and_save(&mut boot, &mut screen, area)?;
    wait_for_exit(&mut boot, 5)?;
    type_line(&mut boot, &after_line())?;
    boot.wait_for_prompts(6)?;
    type_line(&mut boot, POWER_OFF)?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    check_manifest(arch, &image)?;
    eprintln!(
        "xtask: {}: Qt app boot test passed (CorePad {COREPAD_VERSION} on Qt {QT_VERSION})",
        arch.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console(saved: &str, note: &str, fonts: &str, windows: &str) -> String {
        format!(
            "Installed corepad in /Applications/corepad.\nQT-APP install 0\n\
             corepad\tlinux\tx86_64\t/Applications/corepad/corepad\t-\n\
             Opened corepad.\nQT-APP open 0\n\
             # cat /tmp/saved.txt; echo; echo QT-APP saved $?; cat /tmp/note.txt; echo QT-APP \
             note $?; ls /Applications/.fonts; wm list; echo QT-APP after $?\n{saved}\n\n\
             QT-APP saved 0\n{note}\nQT-APP note 0\n{fonts}\n{WM_LIST_HEADER}\n\
             {windows}QT-APP after 0\n{POWER_DOWN_LINE}\n"
        )
    }

    #[test]
    fn the_verdict_wants_the_saved_copy_the_note_kept_the_fonts_and_no_window_left() {
        let pasted = "alpha\nbetaalpha\nbeta";
        let good = console(
            pasted,
            "alpha",
            "corepad",
            "1 normal 0 0 640 400 65535 Terminal\n",
        );
        assert_eq!(verdict("x86_64", &good, true), Ok(()));
        assert!(verdict("x86_64", &good, false).is_err());
        let unpasted = console("alpha\nbeta", "alpha", "corepad", "");
        assert!(
            verdict("x86_64", &unpasted, true)
                .unwrap_err()
                .contains("saving as")
        );
        let overwritten = console(pasted, pasted, "corepad", "");
        assert!(
            verdict("x86_64", &overwritten, true)
                .unwrap_err()
                .contains("/tmp/note.txt changed")
        );
        let unlinked = console(pasted, "alpha", "", "");
        assert!(
            verdict("x86_64", &unlinked, true)
                .unwrap_err()
                .contains("fonts were not linked")
        );
        let open = console(
            pasted,
            "alpha",
            "corepad",
            "2 normal 10 10 600 400 3 note.txt - CorePad\n",
        );
        assert!(
            verdict("x86_64", &open, true)
                .unwrap_err()
                .contains("still listed")
        );
    }

    #[test]
    fn the_typed_lines_install_open_and_read_back() {
        assert!(install_line().starts_with(
            "app install --entry corepad --environment 'QT_QPA_PLATFORM=wayland \
             QT_PLUGIN_PATH=/Applications/corepad/plugins ICU_DATA=/Applications/corepad/share/icu \
             LANG=C.UTF-8' --arguments '/tmp/note.txt' /media/host/corepad.zip;"
        ));
        assert!(open_line().starts_with("echo alpha > /tmp/note.txt; cd /tmp; app open corepad;"));
        assert!(after_line().starts_with("cat /tmp/saved.txt; echo; echo QT-APP saved $?;"));
        let console = console("alpha\nbetaalpha\nbeta", "alpha", "corepad", "");
        assert_eq!(saved_lines(&console), ["alpha", "betaalpha", "beta", ""]);
        assert_eq!(note_lines(&console), ["alpha"]);
    }
}
