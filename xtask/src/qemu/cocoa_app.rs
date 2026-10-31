//! The Cocoa app boot test (D175), on aarch64 and x86_64: one boot of the release kernel with
//! a blank disk and a host folder holding `CocoaGame.zip` (`cocoa_app.rs`). The shell installs
//! it with `app install`, which lists it, and `app open` opens it, so its window appears on the
//! display. The game starts its player and food at random cells, so the test reads them from
//! the frame and then wants every frame equal, pixel for pixel, to the board the host renders
//! with the same colours: 21 presses of `d` bring the player back where it started, `d`, `w` or
//! `s` and the Left or Right arrow move it one cell (choosing the key that does not land on
//! the food), and steering onto the food eats it, which places new food anywhere but the
//! player's row and column. `wm show` lists the window and the game's view; `wm menu --choose`
//! chooses `About CocoaGame`, whose panel is found on the screen as the host renders it with
//! the icon the installer made; `wm show` lists the panel's elements, Esc closes it, and Cmd+Q
//! quits the game, after which `wm list` has none of its windows.

use super::settings::{Boot, Screen};
use super::win32_app::{changed, crop, picture, press, tap, type_line, whole, window_area};
use super::*;
use crate::cocoa_app::{APP, ARCHIVE, BUILD, VERSION};
use eggos_drivers::desktop::Profile;
use eggos_macos_gui::about::{self, ICON_PIXELS, Panel, VERSION_BYTES};
use eggos_macos_gui::colour::rgb;
use eggos_toolkit::canvas::Canvas;

const TEST_DISK_BYTES: u64 = 64 << 20;
const MEMORY: &str = "1G";
const TIMEOUT: Duration = Duration::from_secs(2400);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(300);
const PANEL_TIMEOUT: Duration = Duration::from_secs(120);
const SETTLE: Duration = Duration::from_secs(4);
const ECHO_PAUSE: Duration = Duration::from_secs(3);
const STEP_PAUSE: Duration = Duration::from_millis(400);
const POLL: Duration = Duration::from_secs(2);
const POINTER_PAUSE: Duration = Duration::from_millis(500);
const STATUS_PREFIX: &str = "COCOA-APP ";
const CELLS: u32 = 21;
const CELL: u32 = 25;
const BOARD: u32 = CELLS * CELL;
const PANEL_PREFIX: &str = "About ";
const CANVAS_CLASS: &str = "CanvasView";

type Cell = (u32, u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Colours {
    even: u32,
    odd: u32,
    player: u32,
    food: u32,
}

fn colours() -> Colours {
    Colours {
        even: rgb(0.49, 0.98, 0.0, 1.0).pixel,
        odd: rgb(0.69, 1.0, 0.18, 1.0).pixel,
        player: rgb(0.0, 0.0, 0.5, 1.0).pixel,
        food: rgb(1.0, 0.0, 0.0, 1.0).pixel,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Right,
    Up,
    Left,
    Down,
}

fn moved(cell: Cell, step: Move) -> Cell {
    let (x, y) = cell;
    match step {
        Move::Right => ((x + 1) % CELLS, y),
        Move::Left => ((x + CELLS - 1) % CELLS, y),
        Move::Up => (x, (y + 1) % CELLS),
        Move::Down => (x, (y + CELLS - 1) % CELLS),
    }
}

fn letter(step: Move) -> char {
    match step {
        Move::Right => 'd',
        Move::Up => 'w',
        Move::Left => 'a',
        Move::Down => 's',
    }
}

fn arrow(step: Move) -> &'static str {
    match step {
        Move::Right => "right",
        Move::Up => "up",
        Move::Left => "left",
        Move::Down => "down",
    }
}

fn path(from: Cell, to: Cell) -> Vec<Move> {
    let mut steps = Vec::new();
    let up = (to.1 + CELLS - from.1) % CELLS;
    match up <= CELLS - up {
        true => steps.extend(std::iter::repeat_n(Move::Up, up as usize)),
        false => steps.extend(std::iter::repeat_n(Move::Down, (CELLS - up) as usize)),
    }
    let right = (to.0 + CELLS - from.0) % CELLS;
    match right <= CELLS - right {
        true => steps.extend(std::iter::repeat_n(Move::Right, right as usize)),
        false => steps.extend(std::iter::repeat_n(Move::Left, (CELLS - right) as usize)),
    }
    steps
}

fn cell_colour(cell: Cell, player: Cell, food: Cell, colours: Colours) -> u32 {
    if cell == player {
        colours.player
    } else if cell == food {
        colours.food
    } else if (cell.0 + cell.1).is_multiple_of(2) {
        colours.even
    } else {
        colours.odd
    }
}

fn render_board(player: Cell, food: Cell) -> Vec<u32> {
    let colours = colours();
    let mut pixels = Vec::with_capacity((BOARD * BOARD) as usize);
    for row in 0..BOARD {
        let y = CELLS - 1 - row / CELL;
        for column in 0..BOARD {
            pixels.push(cell_colour((column / CELL, y), player, food, colours));
        }
    }
    pixels
}

fn screen_pixel(picture: &Picture, x: u32, y: u32) -> Option<u32> {
    (x < picture.width && y < picture.height)
        .then(|| picture.pixels[(y * picture.width + x) as usize] & 0x00ff_ffff)
}

fn masked(pixels: Vec<u32>) -> Vec<u32> {
    pixels
        .into_iter()
        .map(|pixel| pixel & 0x00ff_ffff)
        .collect()
}

fn board_origin(picture: &Picture, area: Rect) -> Option<(u32, u32)> {
    let colours = colours();
    let mut origin: Option<(u32, u32)> = None;
    for y in area.y..area.bottom().min(picture.height) {
        for x in area.x..area.right().min(picture.width) {
            let pixel = screen_pixel(picture, x, y);
            if pixel == Some(colours.even) || pixel == Some(colours.odd) {
                origin = Some(match origin {
                    Some((left, top)) => (left.min(x), top.min(y)),
                    None => (x, y),
                });
            }
        }
    }
    origin
}

fn read_board(picture: &Picture, origin: (u32, u32)) -> Option<(Cell, Cell)> {
    let colours = colours();
    let (mut player, mut food) = (Vec::new(), Vec::new());
    for x in 0..CELLS {
        for y in 0..CELLS {
            let centre = (
                origin.0 + x * CELL + CELL / 2,
                origin.1 + (CELLS - 1 - y) * CELL + CELL / 2,
            );
            match screen_pixel(picture, centre.0, centre.1) {
                Some(pixel) if pixel == colours.player => player.push((x, y)),
                Some(pixel) if pixel == colours.food => food.push((x, y)),
                _ => {}
            }
        }
    }
    match (player.as_slice(), food.as_slice()) {
        ([player], [food]) => Some((*player, *food)),
        _ => None,
    }
}

fn board_rect(origin: (u32, u32)) -> Rect {
    Rect::new(origin.0, origin.1, BOARD, BOARD)
}

fn render_panel(icon_file: Option<&[u8]>) -> (u32, u32, Vec<u32>) {
    let mut line = [0u8; VERSION_BYTES];
    let version = about::version_line(VERSION, BUILD, &mut line).unwrap_or("");
    let panel = Panel {
        name: APP,
        version,
        copyright: "",
    };
    let mut icon = [0u32; ICON_PIXELS];
    let has_icon = icon_file.is_some_and(|file| about::icon_pixels(file, &mut icon));
    let (width, height) = (about::WIDTH as u32, panel.height() as u32);
    let mut pixels = vec![0u32; (width * height) as usize];
    if let Some(mut canvas) = Canvas::new(&mut pixels, width, height, 1) {
        panel.paint(&mut canvas, has_icon.then_some(&icon));
    }
    (width, height, pixels)
}

fn find_exact(
    picture: &Picture,
    expected: &[u32],
    (width, height): (u32, u32),
    within: Rect,
) -> Option<(u32, u32)> {
    let last_y = within.bottom().min(picture.height).checked_sub(height)?;
    let last_x = within.right().min(picture.width).checked_sub(width)?;
    for top in within.y..=last_y {
        for left in within.x..=last_x {
            let row_matches = |row: u32| {
                let at = ((top + row) * picture.width + left) as usize;
                let screen = &picture.pixels[at..at + width as usize];
                let wanted = &expected[(row * width) as usize..((row + 1) * width) as usize];
                screen
                    .iter()
                    .zip(wanted)
                    .all(|(&shown, &want)| shown & 0x00ff_ffff == want & 0x00ff_ffff)
            };
            if row_matches(0) && (1..height).all(row_matches) {
                return Some((left, top));
            }
        }
    }
    None
}

fn changed_box(before: &Picture, after: &Picture) -> Option<Rect> {
    if (before.width, before.height) != (after.width, after.height) {
        return None;
    }
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for y in 0..after.height {
        for x in 0..after.width {
            let at = (y * after.width + x) as usize;
            if before.pixels[at] != after.pixels[at] {
                found = Some(match found {
                    Some((left, top, right, bottom)) => {
                        (left.min(x), top.min(y), right.max(x), bottom.max(y))
                    }
                    None => (x, y, x, y),
                });
            }
        }
    }
    let (left, top, right, bottom) = found?;
    Some(Rect::new(left, top, right - left + 1, bottom - top + 1))
}

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

const POWER_OFF: &str = "poweroff\r";

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

fn has_element(lines: &[String], pieces: &[&str]) -> bool {
    lines
        .iter()
        .any(|line| pieces.iter().all(|piece| line.contains(piece)))
}

fn game_tree_problem(lines: &[String]) -> Option<String> {
    let window = format!("window 0,0 {BOARD}x{BOARD}");
    let named = format!("\"{APP}\"");
    let canvas = format!("\"{CANVAS_CLASS}\"");
    let graphic = format!("graphic 0,0 {BOARD}x{BOARD}");
    if !has_element(lines, &["1 ", &window, &named]) {
        return Some(format!("no `1 {window} {named}` element in {lines:?}"));
    }
    if !has_element(lines, &["2 ", &graphic, "focused", &canvas]) {
        return Some(format!(
            "no focused `2 {graphic}` {canvas} element in {lines:?}"
        ));
    }
    None
}

fn panel_tree_problem(lines: &[String], own_icon: bool) -> Option<String> {
    let title = format!("\"{PANEL_PREFIX}{APP}\"");
    let mut line = [0u8; VERSION_BYTES];
    let version = about::version_line(VERSION, BUILD, &mut line).unwrap_or("");
    let version = format!("\"{version}\"");
    let named = format!("\"{APP}\"");
    let icon = match own_icon {
        true => "\"own\"",
        false => "\"generic\"",
    };
    let wanted: [&[&str]; 4] = [
        &["1 ", "window 0,0 288x160", &title],
        &["2 ", "graphic 120,16 48x48", &named, icon],
        &["3 ", "label 88,80 112x32", &named],
        &["4 ", "label 103,128 81x16", &version],
    ];
    wanted
        .iter()
        .find(|pieces| !has_element(lines, pieces))
        .map(|pieces| format!("no element with {pieces:?} in {lines:?}"))
}

struct Session<'s, 'b> {
    boot: &'s mut Boot<'b>,
    screen: &'s mut Screen,
    origin: (u32, u32),
    player: Cell,
    food: Cell,
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

    fn board_is(&mut self, suffix: &str, what: &str) -> Result<Picture, String> {
        std::thread::sleep(SETTLE);
        let shown = self.shot(suffix)?;
        let area = board_rect(self.origin);
        if masked(crop(&shown, area)) != render_board(self.player, self.food) {
            let read = read_board(&shown, self.origin);
            return Err(self.fail(format!(
                "{what}: the board is not the one with the player at {:?} and the food at {:?} \
                 (the frame shows {read:?})",
                self.player, self.food
            )));
        }
        Ok(shown)
    }

    fn step(&mut self, step: Move, with_arrow: bool) -> Result<(), String> {
        match with_arrow {
            true => tap(self.screen, &[arrow(step)])?,
            false => press(self.screen, letter(step))?,
        }
        self.player = moved(self.player, step);
        Ok(())
    }

    fn wrap_around(&mut self) -> Result<(), String> {
        for _ in 0..CELLS {
            press(self.screen, letter(Move::Right))?;
            std::thread::sleep(STEP_PAUSE);
        }
        self.board_is(
            "-cocoa-app-wrapped",
            "21 presses of `d` did not bring the player back",
        )?;
        Ok(())
    }

    fn single_moves(&mut self) -> Result<(), String> {
        self.step(Move::Right, false)?;
        self.board_is("-cocoa-app-right", "`d` did not move the player right")?;
        let vertical = match moved(self.player, Move::Up) == self.food {
            true => Move::Down,
            false => Move::Up,
        };
        self.step(vertical, false)?;
        self.board_is("-cocoa-app-vertical", "`w` or `s` did not move the player")?;
        let sideways = match moved(self.player, Move::Left) == self.food {
            true => Move::Right,
            false => Move::Left,
        };
        self.step(sideways, true)?;
        self.board_is("-cocoa-app-arrow", "an arrow key did not move the player")?;
        Ok(())
    }

    fn eat(&mut self) -> Result<(), String> {
        let target = self.food;
        for step in path(self.player, target) {
            self.step(step, false)?;
            std::thread::sleep(STEP_PAUSE);
        }
        std::thread::sleep(SETTLE);
        let shown = self.shot("-cocoa-app-eaten")?;
        let Some((player, food)) = read_board(&shown, self.origin) else {
            return Err(self.fail("the board after eating has no single player and food".into()));
        };
        if player != target || food.0 == player.0 || food.1 == player.1 {
            return Err(self.fail(format!(
                "after steering onto the food at {target:?} the player is at {player:?} and the \
                 food at {food:?}: the food must move off the player's row and column"
            )));
        }
        self.player = player;
        self.food = food;
        self.board_is("-cocoa-app-new-food", "the board after eating")?;
        Ok(())
    }
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
        let now = picture(screen, arch, "-cocoa-app")?;
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

fn wait_for_panel(
    boot: &mut Boot<'_>,
    screen: &mut Screen,
    before: &Picture,
    expected: &(u32, u32, Vec<u32>),
) -> Result<(u32, u32), String> {
    let arch = boot.arch;
    let started = Instant::now();
    loop {
        boot.console.extend(boot.output.try_iter().flatten());
        let now = picture(screen, arch, "-cocoa-app-about")?;
        if let Some(found) = changed_box(before, &now)
            .and_then(|area| find_exact(&now, &expected.2, (expected.0, expected.1), area))
        {
            return Ok(found);
        }
        if started.elapsed() > PANEL_TIMEOUT {
            return Err(boot.fail(format!(
                "{}: no About panel drawn as the host renders it appeared within {}s of \
                 choosing `{PANEL_PREFIX}{APP}`",
                arch.name,
                PANEL_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(POLL);
    }
}

fn console_text(boot: &mut Boot<'_>) -> String {
    boot.console.extend(boot.output.try_iter().flatten());
    String::from_utf8_lossy(&boot.console).into_owned()
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    let installed = format!("Installed {APP} in /Applications/{APP}.");
    let opened = format!("Opened {APP}.");
    for wanted in [
        installed.as_str(),
        &format!("{STATUS_PREFIX}install 0"),
        opened.as_str(),
        &format!("{STATUS_PREFIX}open 0"),
        &format!("{STATUS_PREFIX}about 0"),
        &format!("{STATUS_PREFIX}after 0"),
        POWER_DOWN_LINE,
    ] {
        if !has_line(console, wanted) {
            return Err(format!("{arch}: the console has no `{wanted}` line"));
        }
    }
    let listed = format!("{APP}\tmacos\t{arch}\t/Applications/{APP}/{APP}.app/");
    if !console.lines().any(|line| line.starts_with(&listed)) {
        return Err(format!(
            "{arch}: `app list` does not list {APP} as a macOS app"
        ));
    }
    if let Some((_, left)) = listed_windows(console, "after")
        .into_iter()
        .find(|(_, title)| title.ends_with(APP))
    {
        return Err(format!(
            "{arch}: {APP}'s window `{left}` is still listed after Cmd+Q"
        ));
    }
    match clean_exit {
        true => Ok(()),
        false => Err(format!(
            "{arch}: QEMU did not exit cleanly after the Cocoa app test"
        )),
    }
}

/// Runs the Cocoa app boot test (see the module documentation) on `kernel`, the release
/// kernel for `arch`, with the host folder `folder` holding `CocoaGame.zip`, and `icon_file`
/// the icon the installer makes of it.
pub fn check(
    arch: &Arch,
    kernel: &Path,
    folder: &Path,
    icon_file: Option<&[u8]>,
) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-cocoa-app-disk.img", arch.name));
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
    type_line(&mut boot, &open_line())?;
    boot.wait_for_prompts(3)?;
    std::thread::sleep(ECHO_PAUSE);
    let before = picture(&mut screen, arch, "-cocoa-app-desktop")?;
    let area = wait_for_window(&mut boot, &mut screen, &before)?;
    std::thread::sleep(SETTLE);
    screen.park_pointer()?;
    std::thread::sleep(POINTER_PAUSE);
    let shown = picture(&mut screen, arch, "-cocoa-app-shown")?;
    let Some(origin) = board_origin(&shown, area) else {
        return Err(boot.fail(format!(
            "{}: the window at {area:?} shows no board",
            arch.name
        )));
    };
    let Some((player, food)) = read_board(&shown, origin) else {
        return Err(boot.fail(format!(
            "{}: the board at {origin:?} has no single player and food",
            arch.name
        )));
    };
    let mut session = Session {
        boot: &mut boot,
        screen: &mut screen,
        origin,
        player,
        food,
    };
    session.board_is("-cocoa-app-start", "the starting board")?;
    session.wrap_around()?;
    session.single_moves()?;
    session.eat()?;
    let board = board_rect(session.origin);
    let last_board = session.shot("-cocoa-app-before-about")?;
    let (player, food) = (session.player, session.food);
    type_line(&mut boot, &list_line("listed"))?;
    boot.wait_for_prompts(4)?;
    let console = console_text(&mut boot);
    let Some(window) = window_titled(&console, "listed", APP) else {
        return Err(boot.fail(format!("{}: `wm list` does not list {APP}", arch.name)));
    };
    type_line(&mut boot, &show_line(window, "tree"))?;
    boot.wait_for_prompts(5)?;
    let console = console_text(&mut boot);
    if let Some(problem) = game_tree_problem(&shown_tree(&console, "tree")) {
        return Err(boot.fail(format!("{}: `wm show {window}`: {problem}", arch.name)));
    }
    type_line(&mut boot, &about_line(window))?;
    boot.wait_for_prompts(6)?;
    let expected = render_panel(icon_file);
    wait_for_panel(&mut boot, &mut screen, &last_board, &expected)?;
    type_line(&mut boot, &list_line("panel"))?;
    boot.wait_for_prompts(7)?;
    let console = console_text(&mut boot);
    let title = format!("{PANEL_PREFIX}{APP}");
    let Some(panel) = window_titled(&console, "panel", &title) else {
        return Err(boot.fail(format!("{}: `wm list` does not list `{title}`", arch.name)));
    };
    type_line(&mut boot, &show_line(panel, "panel-tree"))?;
    boot.wait_for_prompts(8)?;
    let console = console_text(&mut boot);
    if let Some(problem) =
        panel_tree_problem(&shown_tree(&console, "panel-tree"), icon_file.is_some())
    {
        return Err(boot.fail(format!("{}: `wm show {panel}`: {problem}", arch.name)));
    }
    tap(&mut screen, &["esc"])?;
    std::thread::sleep(SETTLE);
    screen.park_pointer()?;
    std::thread::sleep(POINTER_PAUSE);
    let closed = picture(&mut screen, arch, "-cocoa-app-about-closed")?;
    if masked(crop(&closed, board)) != render_board(player, food) {
        return Err(boot.fail(format!(
            "{}: the board is not as it was after the About panel closed",
            arch.name
        )));
    }
    let centre = (board.x + BOARD / 2, board.y + BOARD / 2);
    screen.pointer(centre.0, centre.1, None)?;
    screen.pointer(centre.0, centre.1, Some(true))?;
    screen.pointer(centre.0, centre.1, Some(false))?;
    std::thread::sleep(POINTER_PAUSE);
    tap(&mut screen, &["meta_l", "q"])?;
    std::thread::sleep(SETTLE);
    screen.park_pointer()?;
    std::thread::sleep(POINTER_PAUSE);
    let quit = picture(&mut screen, arch, "-cocoa-app-quit")?;
    if changed(&last_board, &quit, whole(&quit)) == 0 {
        return Err(boot.fail(format!("{}: Cmd+Q left the game on the screen", arch.name)));
    }
    type_line(&mut boot, &list_line("after"))?;
    boot.wait_for_prompts(9)?;
    type_line(&mut boot, POWER_OFF)?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    eprintln!(
        "xtask: {}: Cocoa app boot test passed ({APP} {VERSION})",
        arch.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_is_rendered_from_the_bottom_left_with_the_game_s_colours() {
        let colours = colours();
        assert_eq!(colours.even, 0x007d_fa00);
        assert_eq!(colours.odd, 0x00b0_ff2e);
        assert_eq!(colours.player, 0x0000_0080);
        assert_eq!(colours.food, 0x00ff_0000);
        let pixels = render_board((0, 0), (20, 20));
        let at = |x: u32, y: u32| pixels[(y * BOARD + x) as usize];
        assert_eq!(at(0, BOARD - 1), colours.player);
        assert_eq!(at(BOARD - 1, 0), colours.food);
        assert_eq!(at(0, 0), colours.even);
        assert_eq!(at(CELL, 0), colours.odd);
    }

    #[test]
    fn the_board_is_read_back_from_a_frame() {
        let pixels = render_board((3, 4), (10, 17));
        let mut picture = Picture {
            width: 700,
            height: 700,
            pixels: vec![0x0012_161d; 700 * 700],
        };
        for y in 0..BOARD {
            for x in 0..BOARD {
                picture.pixels[((y + 60) * 700 + x + 40) as usize] =
                    pixels[(y * BOARD + x) as usize];
            }
        }
        let area = Rect::new(20, 30, 600, 600);
        assert_eq!(board_origin(&picture, area), Some((40, 60)));
        assert_eq!(read_board(&picture, (40, 60)), Some(((3, 4), (10, 17))));
        assert_eq!(
            masked(crop(&picture, board_rect((40, 60)))),
            render_board((3, 4), (10, 17))
        );
    }

    #[test]
    fn moves_wrap_and_the_path_takes_the_shorter_way_round() {
        assert_eq!(moved((20, 5), Move::Right), (0, 5));
        assert_eq!(moved((0, 5), Move::Left), (20, 5));
        assert_eq!(moved((4, 20), Move::Up), (4, 0));
        assert_eq!(moved((4, 0), Move::Down), (4, 20));
        let steps = path((1, 1), (19, 3));
        assert_eq!(steps.iter().filter(|&&step| step == Move::Up).count(), 2);
        assert_eq!(steps.iter().filter(|&&step| step == Move::Left).count(), 3);
        let mut at = (1, 1);
        for step in steps {
            at = moved(at, step);
        }
        assert_eq!(at, (19, 3));
    }

    #[test]
    fn the_panel_is_found_where_it_is_drawn() {
        let expected = render_panel(None);
        assert_eq!((expected.0, expected.1), (288, 160));
        let mut picture = Picture {
            width: 800,
            height: 600,
            pixels: vec![0x0050_6070; 800 * 600],
        };
        let before = picture.clone();
        for y in 0..expected.1 {
            for x in 0..expected.0 {
                picture.pixels[((y + 200) * 800 + x + 250) as usize] =
                    expected.2[(y * expected.0 + x) as usize];
            }
        }
        let area = changed_box(&before, &picture).unwrap();
        assert_eq!(
            find_exact(&picture, &expected.2, (expected.0, expected.1), area),
            Some((250, 200))
        );
    }

    #[test]
    fn listings_and_trees_are_read_from_the_console() {
        let console = format!(
            "{WM_LIST_HEADER}\n1 normal 0 0 640 400 65535 Terminal\n\
             4 normal 300 100 527 555 7 CocoaGame\n5 normal 400 300 290 190 7 About CocoaGame\n\
             COCOA-APP panel 0\n\
             eggsh:/tmp# wm show 4; echo COCOA-APP tree $?\n\
             1 window 0,0 525x525 \"CocoaGame\"\n\
             \x20 2 graphic 0,0 525x525 [focused,focusable] \"CanvasView\"\n\
             COCOA-APP tree 0\n"
        );
        assert_eq!(window_titled(&console, "panel", "CocoaGame"), Some(4));
        assert_eq!(window_titled(&console, "panel", "About CocoaGame"), Some(5));
        let tree = shown_tree(&console, "tree");
        assert_eq!(tree.len(), 2);
        assert_eq!(game_tree_problem(&tree), None);
        assert!(game_tree_problem(&tree[..1]).is_some());
        assert!(about_line(4).contains("wm menu --choose 4 CocoaGame 'About CocoaGame'"));
    }

    #[test]
    fn the_verdict_wants_the_install_the_about_and_no_window_left() {
        let console = |windows: &str| {
            format!(
                "Installed CocoaGame in /Applications/CocoaGame.\nCOCOA-APP install 0\n\
                 CocoaGame\tmacos\tx86_64\t/Applications/CocoaGame/CocoaGame.app/Contents/MacOS/\
                 CocoaGame\t/Applications/CocoaGame/app.icon\n\
                 Opened CocoaGame.\nCOCOA-APP open 0\nCOCOA-APP about 0\n\
                 {WM_LIST_HEADER}\n{windows}COCOA-APP after 0\n{POWER_DOWN_LINE}\n"
            )
        };
        let good = console("1 normal 0 0 640 400 65535 Terminal\n");
        assert_eq!(verdict("x86_64", &good, true), Ok(()));
        assert!(verdict("x86_64", &good, false).is_err());
        let left = console("4 normal 300 100 527 555 7 CocoaGame\n");
        assert!(
            verdict("x86_64", &left, true)
                .unwrap_err()
                .contains("still listed")
        );
    }
}
