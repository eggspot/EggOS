//! The Win32 app boot test (D174), on x86_64 (the app ships no ARM64 build): one boot of the
//! release kernel with a blank disk and a host folder holding `pegs.exe` (`win32_app.rs`). The
//! shell installs it with `app install`, which lists it; in `/tmp` `app open` opens it as its
//! icon does, so its window appears on the display. On the window: a peg jumps over another
//! by a drag (the pegs at the three cells change colour), `u` undoes it (the board's frame is the
//! first one again, pixel for pixel) and `r` redoes it (the frame after the jump again); the
//! `Type` menu shows the current board ticked, choosing `Cross 5x7` makes the window narrower
//! and moves the tick; `Help > About` opens the About box over the window and Enter closes it;
//! `Type > Custom...` opens the Custom dialog, Tab twice and Alt+Down open its board type list
//! and Esc twice close the list and the dialog, and the board's frame is unchanged by all of
//! them; `q` quits. `wm list` then has no Pegs window.

use super::settings::{Boot, Screen};
use super::*;
use crate::win32_app::{APP, PROGRAM, PUZZLES_VERSION};
use eggos_drivers::desktop::Profile;

const TEST_DISK_BYTES: u64 = 64 << 20;
const MEMORY: &str = "1G";
const TIMEOUT: Duration = Duration::from_secs(1500);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(300);
const EXIT_TIMEOUT: Duration = Duration::from_secs(120);
const SETTLE: Duration = Duration::from_secs(6);
const ECHO_PAUSE: Duration = Duration::from_secs(3);
const KEY_PAUSE: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_secs(2);
const POINTER_PAUSE: Duration = Duration::from_millis(500);
const STATUS_PREFIX: &str = "WIN32-APP ";
const WINDOW_TITLE: &str = "Pegs";
const WINDOW_ROW_PIXELS: usize = 200;
const WINDOW_PIXELS: usize = 50_000;
const DIALOG_PIXELS: usize = 20_000;
const LIST_PIXELS: usize = 3_000;
const MENU_COLOUR: u32 = 0x00f0_f0f0;
const PEG_COLOUR: u32 = 0x0000_00ff;
const TICK_COLOUR: u32 = 0x0000_0000;
const MENU_BAR: u32 = 24;
const BAR_ENTRY: u32 = 48;
const DROP_DOWN_PADDING: u32 = 3;
const MENU_ITEM: u32 = 24;
const TICK_PIXEL: (u32, u32) = (10, 12);
const TILE: u32 = 33;
const BOARD_MARGIN: u32 = 16;
const TYPE_ENTRY: u32 = 1;
const HELP_ENTRY: u32 = 2;
const TYPE_LEFT: u32 = 48;
const TYPE_ITEM_WIDTH: u32 = 128;
const HELP_ITEM_WIDTH: u32 = 120;
const CROSS_5X7: u32 = 0;
const CROSS_7X7: u32 = 1;
const CUSTOM: u32 = 9;
const NARROW_CLIENT_WIDTH: u32 = 5 * TILE + 2 * BOARD_MARGIN;
const JUMP: [(u32, u32); 3] = [(3, 1), (3, 2), (3, 3)];

fn install_line() -> String {
    format!("app install /media/host/{PROGRAM}; echo {STATUS_PREFIX}install $?\r")
}

fn open_line() -> String {
    format!("app list; cd /tmp; app open {APP}; echo {STATUS_PREFIX}open $?\r")
}

fn after_line() -> String {
    format!("wm list; echo {STATUS_PREFIX}after $?\r")
}

const POWER_OFF: &str = "poweroff\r";

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

fn pixel(picture: &Picture, x: u32, y: u32) -> Option<u32> {
    (x < picture.width && y < picture.height)
        .then(|| picture.pixels[(y * picture.width + x) as usize] & 0x00ff_ffff)
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

fn whole(picture: &Picture) -> Rect {
    Rect::new(0, 0, picture.width, picture.height)
}

fn crop(picture: &Picture, area: Rect) -> Vec<u32> {
    let mut pixels = Vec::new();
    for y in area.y..area.bottom().min(picture.height) {
        for x in area.x..area.right().min(picture.width) {
            pixels.push(picture.pixels[(y * picture.width + x) as usize]);
        }
    }
    pixels
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

fn content_origin(picture: &Picture, area: Rect) -> Option<(u32, u32)> {
    let probe = area.right().checked_sub(6)?;
    let top = (area.y..area.bottom()).find(|&y| pixel(picture, probe, y) == Some(MENU_COLOUR))?;
    let left = (area.x..probe).find(|&x| pixel(picture, x, top + 1) == Some(MENU_COLOUR))?;
    Some((left, top))
}

fn cell_centre(content: (u32, u32), cell: (u32, u32)) -> (u32, u32) {
    let middle = |index: u32| BOARD_MARGIN + index * TILE + TILE / 2;
    (
        content.0 + middle(cell.0),
        content.1 + MENU_BAR + middle(cell.1),
    )
}

fn board(content: (u32, u32), client_width: u32) -> Rect {
    let side = 7 * TILE + 2 * BOARD_MARGIN;
    Rect::new(
        content.0,
        content.1 + MENU_BAR,
        client_width.min(side),
        side,
    )
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
        let now = picture(screen, arch, "-win32-app")?;
        if let Some(area) = window_area(before, &now) {
            return Ok(area);
        }
        if started.elapsed() > WINDOW_TIMEOUT {
            return Err(boot.fail(format!(
                "{}: no Pegs window appeared within {}s of `app open {APP}`",
                arch.name,
                WINDOW_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(POLL);
    }
}

fn click(screen: &mut Screen, (x, y): (u32, u32)) -> Result<(), String> {
    screen.pointer(x, y, None)?;
    screen.pointer(x, y, Some(true))?;
    screen.pointer(x, y, Some(false))?;
    std::thread::sleep(KEY_PAUSE);
    Ok(())
}

fn press(screen: &mut Screen, character: char) -> Result<(), String> {
    screen.qmp.send_input(&qmp::typing(character)?)?;
    std::thread::sleep(KEY_PAUSE);
    Ok(())
}

fn tap(screen: &mut Screen, qcodes: &[&'static str]) -> Result<(), String> {
    for &qcode in qcodes {
        screen
            .qmp
            .send_input(&[InputEvent::Key { qcode, down: true }])?;
    }
    for &qcode in qcodes.iter().rev() {
        screen
            .qmp
            .send_input(&[InputEvent::Key { qcode, down: false }])?;
    }
    std::thread::sleep(KEY_PAUSE);
    Ok(())
}

fn drag(screen: &mut Screen, path: &[(u32, u32)]) -> Result<(), String> {
    let (first, last) = (path[0], path[path.len() - 1]);
    screen.pointer(first.0, first.1, None)?;
    screen.pointer(first.0, first.1, Some(true))?;
    for &(x, y) in &path[1..] {
        screen.pointer(x, y, None)?;
        std::thread::sleep(Duration::from_millis(300));
    }
    screen.pointer(last.0, last.1, Some(false))?;
    std::thread::sleep(KEY_PAUSE);
    Ok(())
}

fn pegs_at(picture: &Picture, content: (u32, u32)) -> [bool; 3] {
    JUMP.map(|cell| {
        let (x, y) = cell_centre(content, cell);
        pixel(picture, x, y) == Some(PEG_COLOUR)
    })
}

fn entry_centre(content: (u32, u32), entry: u32) -> (u32, u32) {
    (
        content.0 + entry * BAR_ENTRY + BAR_ENTRY / 2,
        content.1 + MENU_BAR / 2,
    )
}

fn item_centre(content: (u32, u32), left: u32, width: u32, item: u32) -> (u32, u32) {
    let top = content.1 + MENU_BAR + DROP_DOWN_PADDING + item * MENU_ITEM;
    (content.0 + left + width / 2, top + MENU_ITEM / 2)
}

fn ticked(picture: &Picture, content: (u32, u32), item: u32) -> bool {
    let top = content.1 + MENU_BAR + DROP_DOWN_PADDING + item * MENU_ITEM;
    let x = content.0 + TYPE_LEFT + TICK_PIXEL.0;
    pixel(picture, x, top + TICK_PIXEL.1) == Some(TICK_COLOUR)
}

struct Session<'s, 'b> {
    boot: &'s mut Boot<'b>,
    screen: &'s mut Screen,
}

impl Session<'_, '_> {
    fn shot(&mut self, suffix: &str) -> Result<Picture, String> {
        self.screen.park_pointer()?;
        std::thread::sleep(POINTER_PAUSE);
        picture(self.screen, self.boot.arch, suffix)
    }

    fn fail(&mut self, what: String) -> String {
        self.boot.fail(format!("{}: {what}", self.boot.arch.name))
    }

    fn jump_undo_redo(&mut self, content: (u32, u32)) -> Result<(), String> {
        let start = self.shot("-win32-app-start")?;
        if pegs_at(&start, content) != [true, true, false] {
            return Err(self.fail(format!(
                "the Cross 7x7 board is not drawn as it starts: pegs at {JUMP:?} are {:?}",
                pegs_at(&start, content)
            )));
        }
        let path = JUMP.map(|cell| cell_centre(content, cell));
        drag(self.screen, &path)?;
        std::thread::sleep(SETTLE);
        let jumped = self.shot("-win32-app-jumped")?;
        if pegs_at(&jumped, content) != [false, false, true] {
            return Err(self.fail(format!(
                "dragging the peg at {:?} to {:?} did not jump it: pegs are {:?}",
                JUMP[0],
                JUMP[2],
                pegs_at(&jumped, content)
            )));
        }
        let area = board(content, 7 * TILE + 2 * BOARD_MARGIN);
        press(self.screen, 'u')?;
        std::thread::sleep(SETTLE);
        let undone = self.shot("-win32-app-undone")?;
        if crop(&undone, area) != crop(&start, area) {
            return Err(self.fail("`u` did not bring back the starting frame".to_owned()));
        }
        press(self.screen, 'r')?;
        std::thread::sleep(SETTLE);
        let redone = self.shot("-win32-app-redone")?;
        if crop(&redone, area) != crop(&jumped, area) {
            return Err(self.fail("`r` did not bring back the frame after the jump".to_owned()));
        }
        Ok(())
    }

    fn change_board(&mut self, content: (u32, u32)) -> Result<(), String> {
        click(self.screen, entry_centre(content, TYPE_ENTRY))?;
        let open = self.shot("-win32-app-type")?;
        if !ticked(&open, content, CROSS_7X7) || ticked(&open, content, CROSS_5X7) {
            return Err(self.fail("the Type menu does not tick Cross 7x7".to_owned()));
        }
        click(
            self.screen,
            item_centre(content, TYPE_LEFT, TYPE_ITEM_WIDTH, CROSS_5X7),
        )?;
        std::thread::sleep(SETTLE);
        let narrow = self.shot("-win32-app-narrow")?;
        let beyond = content.0 + NARROW_CLIENT_WIDTH + 12;
        let middle = content.1 + MENU_BAR + 100;
        if pixel(&narrow, beyond, middle) == Some(MENU_COLOUR) {
            return Err(self.fail(
                "choosing Cross 5x7 did not make the window narrower for the smaller board"
                    .to_owned(),
            ));
        }
        click(self.screen, entry_centre(content, TYPE_ENTRY))?;
        let again = self.shot("-win32-app-type-again")?;
        if !ticked(&again, content, CROSS_5X7) || ticked(&again, content, CROSS_7X7) {
            return Err(self.fail("the Type menu did not move its tick to Cross 5x7".to_owned()));
        }
        tap(self.screen, &["esc"])?;
        Ok(())
    }

    fn dialogs(&mut self, content: (u32, u32)) -> Result<(), String> {
        std::thread::sleep(SETTLE);
        let area = board(content, NARROW_CLIENT_WIDTH);
        let before = self.shot("-win32-app-before-about")?;
        click(self.screen, entry_centre(content, HELP_ENTRY))?;
        let help_left = (2 * BAR_ENTRY).min(NARROW_CLIENT_WIDTH - HELP_ITEM_WIDTH);
        click(
            self.screen,
            item_centre(content, help_left, HELP_ITEM_WIDTH, 0),
        )?;
        std::thread::sleep(SETTLE);
        let about = self.shot("-win32-app-about")?;
        let shown = changed(&before, &about, whole(&about));
        if shown < DIALOG_PIXELS {
            return Err(self.fail(format!(
                "Help > About changed {shown} pixels: no About box appeared"
            )));
        }
        tap(self.screen, &["ret"])?;
        std::thread::sleep(SETTLE);
        let closed = self.shot("-win32-app-about-closed")?;
        if crop(&closed, area) != crop(&before, area) {
            return Err(self.fail("the board is not as it was after the About box".to_owned()));
        }
        click(self.screen, entry_centre(content, TYPE_ENTRY))?;
        click(
            self.screen,
            item_centre(content, TYPE_LEFT, TYPE_ITEM_WIDTH, CUSTOM),
        )?;
        std::thread::sleep(SETTLE);
        let custom = self.shot("-win32-app-custom")?;
        let shown = changed(&closed, &custom, whole(&custom));
        if shown < DIALOG_PIXELS {
            return Err(self.fail(format!(
                "Type > Custom... changed {shown} pixels: no Custom dialog appeared"
            )));
        }
        tap(self.screen, &["tab"])?;
        tap(self.screen, &["tab"])?;
        tap(self.screen, &["alt", "down"])?;
        std::thread::sleep(SETTLE);
        let listed = self.shot("-win32-app-board-types")?;
        let opened = changed(&custom, &listed, whole(&listed));
        if opened < LIST_PIXELS {
            return Err(self.fail(format!(
                "Alt+Down on the board type changed {opened} pixels: its list did not open"
            )));
        }
        tap(self.screen, &["esc"])?;
        tap(self.screen, &["esc"])?;
        std::thread::sleep(SETTLE);
        let after = self.shot("-win32-app-custom-closed")?;
        if crop(&after, area) != crop(&before, area) {
            return Err(self.fail("the board is not as it was after the Custom dialog".to_owned()));
        }
        Ok(())
    }
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
                "{}: Pegs did not end within {}s of `q`",
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
        opened.as_str(),
        &format!("{STATUS_PREFIX}open 0"),
        &format!("{STATUS_PREFIX}after 0"),
        POWER_DOWN_LINE,
    ] {
        if !has_line(console, wanted) {
            return Err(format!("{arch}: the console has no `{wanted}` line"));
        }
    }
    let listed = format!("{APP}\twindows\t{arch}\t/Applications/{APP}/{PROGRAM}\t");
    if !console.lines().any(|line| line.starts_with(&listed)) {
        return Err(format!(
            "{arch}: `app list` does not list {APP} as a Windows app"
        ));
    }
    let windows = wm_list_lines(console);
    if windows.is_empty() {
        return Err(format!("{arch}: the console has no `wm list` listing"));
    }
    if let Some(left) = windows.iter().find(|line| line.ends_with(WINDOW_TITLE)) {
        return Err(format!(
            "{arch}: Pegs' window is still listed after it quit: `{left}`"
        ));
    }
    match clean_exit {
        true => Ok(()),
        false => Err(format!(
            "{arch}: QEMU did not exit cleanly after the Win32 app test"
        )),
    }
}

/// Runs the Win32 app boot test (see the module documentation) on `kernel`, the release
/// kernel for `arch`, with the host folder `folder` holding `pegs.exe`.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-win32-app-disk.img", arch.name));
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
    type_line(&mut boot, &open_line())?;
    boot.wait_for_prompts(3)?;
    std::thread::sleep(ECHO_PAUSE);
    let before = picture(&mut screen, arch, "-win32-app-desktop")?;
    let area = wait_for_window(&mut boot, &mut screen, &before)?;
    std::thread::sleep(SETTLE);
    screen.park_pointer()?;
    std::thread::sleep(POINTER_PAUSE);
    let shown = picture(&mut screen, arch, "-win32-app-shown")?;
    let Some(content) = content_origin(&shown, area) else {
        return Err(boot.fail(format!(
            "{}: the window at {area:?} shows no menu bar",
            arch.name
        )));
    };
    let mut session = Session {
        boot: &mut boot,
        screen: &mut screen,
    };
    session.jump_undo_redo(content)?;
    session.change_board(content)?;
    session.dialogs(content)?;
    press(&mut screen, 'q')?;
    wait_for_exit(&mut boot, 4)?;
    type_line(&mut boot, &after_line())?;
    boot.wait_for_prompts(5)?;
    type_line(&mut boot, POWER_OFF)?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    eprintln!(
        "xtask: {}: Win32 app boot test passed (Pegs from Simon Tatham's Puzzles {PUZZLES_VERSION})",
        arch.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console(windows: &str) -> String {
        format!(
            "Installed pegs in /Applications/pegs.\nWIN32-APP install 0\n\
             pegs\twindows\tx86_64\t/Applications/pegs/pegs.exe\t/Applications/pegs/app.icon\n\
             Opened pegs.\nWIN32-APP open 0\n{WM_LIST_HEADER}\n{windows}\
             WIN32-APP after 0\n{POWER_DOWN_LINE}\n"
        )
    }

    #[test]
    fn the_verdict_wants_the_install_and_no_window_left() {
        let good = console("1 normal 0 0 640 400 65535 Terminal\n");
        assert_eq!(verdict("x86_64", &good, true), Ok(()));
        assert!(verdict("x86_64", &good, false).is_err());
        let open = console("2 normal 300 200 199 317 3 Pegs\n");
        assert!(
            verdict("x86_64", &open, true)
                .unwrap_err()
                .contains("still listed")
        );
        assert!(verdict("x86_64", "Opened pegs.\n", true).is_err());
    }

    #[test]
    fn the_board_s_cells_menus_and_ticks_are_found_from_the_content() {
        assert_eq!(cell_centre((100, 50), (3, 1)), (231, 139));
        assert_eq!(cell_centre((100, 50), (0, 0)), (132, 106));
        assert_eq!(entry_centre((100, 50), TYPE_ENTRY), (172, 62));
        assert_eq!(
            item_centre((100, 50), TYPE_LEFT, TYPE_ITEM_WIDTH, 0),
            (212, 89)
        );
        assert_eq!(NARROW_CLIENT_WIDTH, 197);
        let mut picture = Picture {
            width: 400,
            height: 400,
            pixels: vec![0x0012_161d; 400 * 400],
        };
        for y in 79..370 {
            for x in 51..316 {
                picture.pixels[y * 400 + x] = MENU_COLOUR;
            }
        }
        let area = Rect::new(50, 50, 267, 320);
        assert_eq!(content_origin(&picture, area), Some((51, 79)));
        let item = 1;
        let top = 79 + MENU_BAR + DROP_DOWN_PADDING + item * MENU_ITEM;
        let tick = (51 + TYPE_LEFT + TICK_PIXEL.0, top + TICK_PIXEL.1);
        assert!(!ticked(&picture, (51, 79), item));
        picture.pixels[(tick.1 * 400 + tick.0) as usize] = TICK_COLOUR;
        assert!(ticked(&picture, (51, 79), item));
        assert!(!ticked(&picture, (51, 79), 0));
    }

    #[test]
    fn the_window_is_found_where_the_screen_changed() {
        let blank = Picture {
            width: 800,
            height: 600,
            pixels: vec![0x0050_6070; 800 * 600],
        };
        let mut shown = blank.clone();
        for y in 100..417 {
            for x in 300..565 {
                shown.pixels[y * 800 + x] = MENU_COLOUR;
            }
        }
        assert_eq!(
            window_area(&blank, &shown),
            Some(Rect::new(300, 100, 265, 317))
        );
        assert_eq!(window_area(&blank, &blank), None);
        assert!(open_line().contains("app open pegs;"));
        assert!(install_line().contains("/media/host/pegs.exe"));
    }
}
