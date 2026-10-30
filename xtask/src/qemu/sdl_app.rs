//! The SDL app boot test (D172), on x86_64 and aarch64: one boot of the release kernel with a
//! blank disk and a host folder holding `brogue.zip` (`sdl_app.rs`). The shell installs it with
//! `app install --arguments`, which lists it; in `/tmp` `app open` opens it as its icon does, so
//! the terminal runs it through the loader it carries; its window appears on the display, a
//! click focuses it, keys play turns (rest, then a step each way) and change what it shows,
//! `S` and Enter save the game and close it, so its window goes and the shell's prompt comes
//! back. `/tmp` then holds the saved game, `wm list` no Brogue window, and after the power-off
//! the disk's manifest names the loader, the library folder and the arguments.

use super::install::{file_at, image_volume};
use super::settings::{Boot, Screen};
use super::*;
use crate::sdl_app::{APP, ARCHIVE, LIBRARY_FOLDER, install_arguments};
use eggos_drivers::desktop::Profile;

const TEST_DISK_BYTES: u64 = 128 << 20;
const MEMORY: &str = "1G";
const TIMEOUT: Duration = Duration::from_secs(1200);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(300);
const EXIT_TIMEOUT: Duration = Duration::from_secs(300);
const SETTLE: Duration = Duration::from_secs(15);
const KEY_PAUSE: Duration = Duration::from_millis(700);
const POLL: Duration = Duration::from_secs(2);
const WINDOW_PIXELS: usize = 100_000;
const WINDOW_ROW_PIXELS: usize = 300;
const PLAYED_PIXELS: usize = 50;
const TITLE_BAR_DEPTH: u32 = 14;
const PROMPT_TEXT: &str = "Save game as";
const STATUS_PREFIX: &str = "SDL-APP ";
const SAVED_SUFFIX: &str = ".broguesave";
const WINDOW_TITLE: &str = "Brogue";
const TURN_KEYS: [char; 9] = ['z', 'z', 'z', 'z', 'z', 'l', 'j', 'h', 'k'];

fn install_line() -> String {
    format!(
        "app install --arguments '{}' /media/host/{ARCHIVE}; echo {STATUS_PREFIX}install $?\r",
        install_arguments()
    )
}

fn list_line() -> &'static str {
    "app list\r"
}

fn open_line() -> String {
    format!("cd /tmp; app open {APP}; echo {STATUS_PREFIX}open $?\r")
}

fn after_line() -> String {
    format!("ls /tmp; wm list; echo {STATUS_PREFIX}after $?\r")
}

const POWER_OFF: &str = "poweroff\r";

fn listed_line(arch: &str) -> String {
    format!("{APP}\tlinux\t{arch}\t/Applications/{APP}/{APP}\t-")
}

fn type_line(boot: &mut Boot<'_>, line: &str) -> Result<(), String> {
    boot.keyboard
        .write_all(line.as_bytes())
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", boot.arch.name))
}

fn picture(screen: &mut Screen, arch: &Arch, suffix: &str) -> Result<Picture, String> {
    let path = screendump_path(arch, suffix)?;
    screen.qmp.screendump(&path)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Picture::from_ppm(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn changed(before: &Picture, after: &Picture, area: Rect) -> usize {
    if (before.width, before.height) != (after.width, after.height) {
        return usize::MAX;
    }
    let mut count = 0;
    for y in area.y..area.bottom().min(after.height) {
        for x in area.x..area.right().min(after.width) {
            let at = (y * after.width + x) as usize;
            if before.pixels[at] != after.pixels[at] {
                count += 1;
            }
        }
    }
    count
}

fn window_area(before: &Picture, after: &Picture) -> Option<Rect> {
    if (before.width, before.height) != (after.width, after.height) {
        return None;
    }
    let width = after.width as usize;
    let differs = |x: usize, y: usize| before.pixels[y * width + x] != after.pixels[y * width + x];
    let rows: Vec<usize> = (0..after.height as usize)
        .filter(|&y| (0..width).filter(|&x| differs(x, y)).count() >= WINDOW_ROW_PIXELS)
        .collect();
    let (&top, &bottom) = (rows.first()?, rows.last()?);
    let columns: Vec<usize> = (0..width)
        .filter(|&x| (top..=bottom).filter(|&y| differs(x, y)).count() >= (bottom - top + 1) / 2)
        .collect();
    let (&left, &right) = (columns.first()?, columns.last()?);
    let area = Rect::new(
        left as u32,
        top as u32,
        (right - left + 1) as u32,
        (bottom - top + 1) as u32,
    );
    (changed(before, after, area) >= WINDOW_PIXELS).then_some(area)
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
        let now = picture(screen, arch, "-sdl-app")?;
        if let Some(area) = window_area(before, &now) {
            return Ok(area);
        }
        if started.elapsed() > WINDOW_TIMEOUT {
            return Err(boot.fail(format!(
                "{}: no Brogue window appeared within {}s of `app open {APP}`",
                arch.name,
                WINDOW_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(POLL);
    }
}

fn press(screen: &mut Screen, character: char) -> Result<(), String> {
    screen.qmp.send_input(&qmp::typing(character)?)?;
    std::thread::sleep(KEY_PAUSE);
    Ok(())
}

fn tap(screen: &mut Screen, qcode: &'static str) -> Result<(), String> {
    for down in [true, false] {
        screen.qmp.send_input(&[InputEvent::Key { qcode, down }])?;
    }
    std::thread::sleep(KEY_PAUSE);
    Ok(())
}

fn play_and_save(boot: &mut Boot<'_>, screen: &mut Screen, area: Rect) -> Result<(), String> {
    let arch = boot.arch;
    std::thread::sleep(SETTLE);
    let title = (area.x + area.width / 2, area.y + TITLE_BAR_DEPTH);
    screen.pointer(title.0, title.1, None)?;
    screen.pointer(title.0, title.1, Some(true))?;
    screen.pointer(title.0, title.1, Some(false))?;
    screen.park_pointer()?;
    let before = picture(screen, arch, "-sdl-app-before")?;
    for key in TURN_KEYS {
        press(screen, key)?;
    }
    std::thread::sleep(SETTLE);
    let played = picture(screen, arch, "-sdl-app-played")?;
    let difference = changed(&before, &played, area);
    if difference < PLAYED_PIXELS {
        return Err(boot.fail(format!(
            "{}: Brogue's window changed in {difference} pixels after {} turns' keys: the keys \
             did not reach it",
            arch.name,
            TURN_KEYS.len()
        )));
    }
    press(screen, 'S')?;
    std::thread::sleep(SETTLE);
    tap(screen, "ret")?;
    std::thread::sleep(SETTLE);
    tap(screen, "ret")?;
    Ok(())
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
                "{}: Brogue did not end within {}s of saving",
                boot.arch.name,
                EXIT_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn wm_list_lines(console: &str) -> Vec<&str> {
    let after = format!("{STATUS_PREFIX}after ");
    console
        .lines()
        .skip_while(|line| line.trim_end() != WM_LIST_HEADER)
        .take_while(|line| !line.starts_with(&after))
        .collect()
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
        &format!("{STATUS_PREFIX}after 0"),
        POWER_DOWN_LINE,
    ] {
        if !has_line(console, wanted) {
            return Err(format!("{arch}: the console has no `{wanted}` line"));
        }
    }
    let saved = console.lines().any(|line| {
        let line = line.trim_end();
        line.ends_with(SAVED_SUFFIX) && !line.starts_with("LastGame") && line.contains('#')
    });
    if !saved {
        return Err(format!(
            "{arch}: /tmp holds no saved game (`... #<seed> at depth 1{SAVED_SUFFIX}`): \
             Brogue did not save on `S` ({PROMPT_TEXT}...) and Enter"
        ));
    }
    let windows = wm_list_lines(console);
    if windows.is_empty() {
        return Err(format!("{arch}: the console has no `wm list` listing"));
    }
    if let Some(left) = windows.iter().find(|line| line.contains(WINDOW_TITLE)) {
        return Err(format!(
            "{arch}: Brogue's window is still listed after it ended: `{left}`"
        ));
    }
    match clean_exit {
        true => Ok(()),
        false => Err(format!(
            "{arch}: QEMU did not exit cleanly after the SDL app test"
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
    let loader = format!("loader={LIBRARY_FOLDER}/ld-musl-{}.so.1", arch.name);
    let libraries = format!("libraries={LIBRARY_FOLDER}");
    let arguments = format!("arguments={}", install_arguments());
    for wanted in [loader, libraries, arguments] {
        if !manifest.lines().any(|line| line == wanted) {
            return Err(fail(format!(
                "the installed manifest has no `{wanted}` line: {manifest:?}"
            )));
        }
    }
    Ok(())
}

/// Runs the SDL app boot test (see the module documentation) on `kernel`, the release kernel
/// for `arch`, with the host folder `folder` holding `brogue.zip`.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-sdl-app-disk.img", arch.name));
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
    let before = picture(&mut screen, arch, "-sdl-app-desktop")?;
    type_line(&mut boot, &open_line())?;
    let area = wait_for_window(&mut boot, &mut screen, &before)?;
    play_and_save(&mut boot, &mut screen, area)?;
    wait_for_exit(&mut boot, 5)?;
    type_line(&mut boot, &after_line())?;
    boot.wait_for_prompts(6)?;
    type_line(&mut boot, POWER_OFF)?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    check_manifest(arch, &image)?;
    eprintln!("xtask: {}: SDL app boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console(saved: &str, windows: &str) -> String {
        format!(
            "Installed brogue in /Applications/brogue.\nSDL-APP install 0\n\
             brogue\tlinux\tx86_64\t/Applications/brogue/brogue\t-\nOpened brogue.\n\
             SDL-APP open 0\nLastGame.broguesave\n{saved}\n{WM_LIST_HEADER}\n{windows}\
             SDL-APP after 0\n{POWER_DOWN_LINE}\n"
        )
    }

    #[test]
    fn the_verdict_wants_the_install_the_saved_game_and_no_window_left() {
        let saved = "Saved CE 1.15.1 #1 at depth 1.broguesave";
        let good = console(saved, "1 normal 0 0 640 400 65535 Terminal\n");
        assert_eq!(verdict("x86_64", &good, true), Ok(()));
        assert!(verdict("x86_64", &good, false).is_err());
        let unsaved = console("", "");
        assert!(
            verdict("x86_64", &unsaved, true)
                .unwrap_err()
                .contains("saved game")
        );
        let open = console(saved, "2 normal 10 10 937 585 3 Brogue\n");
        assert!(
            verdict("x86_64", &open, true)
                .unwrap_err()
                .contains("still listed")
        );
    }

    #[test]
    fn the_window_is_where_the_screen_changed_most() {
        let blank = Picture {
            width: 400,
            height: 300,
            pixels: vec![0x0050_6070; 400 * 300],
        };
        let mut shown = blank.clone();
        for y in 40..290 {
            for x in 20..380 {
                shown.pixels[y * 400 + x] = 0;
            }
        }
        shown.pixels[5 * 400 + 5] = 0xffffff;
        assert_eq!(window_area(&blank, &shown), None);
        let small = Rect::new(20, 40, 360, 250);
        assert_eq!(changed(&blank, &shown, small), 360 * 250);
        let wide = Picture {
            width: 1280,
            height: 800,
            pixels: vec![0x0050_6070; 1280 * 800],
        };
        let mut brogue = wide.clone();
        for y in 100..685 {
            for x in 170..1107 {
                brogue.pixels[y * 1280 + x] = 0x0000_0010;
            }
        }
        assert_eq!(
            window_area(&wide, &brogue),
            Some(Rect::new(170, 100, 937, 585))
        );
        assert!(open_line().starts_with("cd /tmp; app open brogue;"));
        assert!(install_line().contains("--arguments '-n --data-dir"));
    }
}
