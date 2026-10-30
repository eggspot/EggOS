//! The AutoHotkey boot test (D178), on x86_64 (AutoHotkey ships no ARM64 build): one boot of
//! the release kernel with a blank disk and a host folder holding AutoHotkey's release `.zip`,
//! our script `controls.ahk` and its picture `egg.png` (`autohotkey.rs`). The shell installs
//! the archive with `app install --entry AutoHotkey64.exe --arguments controls.ahk`, which lists
//! it; in `/tmp`, beside a copy of the script and the picture, `app open` opens it as its icon
//! does, and the script's window appears: section 30 of `docs/ux/windows-controls-v0.3.md`.
//! Each step is checked by pixels of the design's tables in a screendump: the first frame (the
//! list's first row selected, the progress bar at 40, the picture GDI+ decoded, the status bar's
//! two parts), Down Down (the third row selected), a click on the toolbar's `Step` (the progress
//! bar at 60), the element tree `wm show` reads (the selected row, the progress bar's per mille),
//! a click on the `Tree` tab (the tree shows with its `+` boxes), a click on `Fruit`'s box (it
//! opens and is selected), Down (`Apples` selected); Alt+F4 closes the window, AutoHotkey exits
//! and `wm list` has no such window.

use super::settings::{Boot, Screen};
use super::win32_app::{changed, crop, picture, pixel, tap, type_line, window_area};
use super::*;
use crate::autohotkey::{
    APP, ARCHIVE, PICTURE, PICTURE_FILL_COLOUR, PICTURE_RING_COLOUR, PICTURE_SQUARE_COLOUR,
    PROGRAM, SCRIPT, VERSION,
};
use eggos_drivers::desktop::Profile;

const TEST_DISK_BYTES: u64 = 64 << 20;
const MEMORY: &str = "1G";
const TIMEOUT: Duration = Duration::from_secs(1800);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(400);
const LINE_TIMEOUT: Duration = Duration::from_secs(120);
const SETTLE: Duration = Duration::from_secs(6);
const ECHO_PAUSE: Duration = Duration::from_secs(3);
const KEY_PAUSE: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_secs(2);
const POINTER_PAUSE: Duration = Duration::from_millis(500);
const STATUS_PREFIX: &str = "AUTOHOTKEY ";
const WINDOW_TITLE: &str = "EggOS controls";
const SURFACE: u32 = 0x00f0_f0f0;
const WHITE: u32 = 0x00ff_ffff;
const BLACK: u32 = 0x0000_0000;
const HIGHLIGHT: u32 = 0x0000_78d7;
const BAR_FILL: u32 = 0x0006_b025;
const BAR_TRACK: u32 = 0x00e6_e6e6;
const SEPARATOR: u32 = 0x00d7_d7d7;
const ROW_0: (u32, u32) = (290, 100);
const ROW_2: (u32, u32) = (290, 140);
const PROGRESS_AT_40: (u32, u32) = (100, 290);
const PROGRESS_PAST_40: (u32, u32) = (200, 290);
const PROGRESS_PAST_60: (u32, u32) = (270, 290);
const PICTURE_RING: (u32, u32) = (441, 277);
const PICTURE_FILL: (u32, u32) = (444, 280);
const PICTURE_SQUARE: (u32, u32) = (455, 291);
const STATUS_SEPARATOR: (u32, u32) = (239, 345);
const STATUS_FIRST_PART: (u32, u32, u32, u32) = (0, 336, 238, 24);
const STEP_BUTTON: (u32, u32) = (24, 16);
const TREE_TAB: (u32, u32) = (80, 48);
const FRUIT_BOX: (u32, u32) = (24, 74);
const FRUIT_BOX_PLUS: (u32, u32) = (24, 72);
const FRUIT_LABEL: (u32, u32) = (80, 75);
const APPLES_LABEL: (u32, u32) = (108, 95);

fn install_line() -> String {
    format!(
        "app install --name {APP} --entry {PROGRAM} --arguments {SCRIPT} /media/host/{ARCHIVE}; \
         echo {STATUS_PREFIX}install $?\r"
    )
}

fn open_line() -> String {
    format!(
        "app list; cp /media/host/{SCRIPT} /media/host/{PICTURE} /tmp/; cd /tmp; app open {APP}; \
         echo {STATUS_PREFIX}open $?\r"
    )
}

fn listed_line() -> String {
    format!("wm list; echo {STATUS_PREFIX}listed $?\r")
}

fn show_line(window: &str) -> String {
    format!("wm show {window}; echo {STATUS_PREFIX}shown $?\r")
}

fn after_line() -> String {
    format!("wm list; echo {STATUS_PREFIX}after $?\r")
}

const POWER_OFF: &str = "poweroff\r";

fn wait_for_line(boot: &mut Boot<'_>, wanted: &str) -> Result<String, String> {
    let started = Instant::now();
    loop {
        boot.console.extend(boot.output.try_iter().flatten());
        let console = String::from_utf8_lossy(&boot.console).into_owned();
        if has_line(&console, wanted) {
            return Ok(console);
        }
        if started.elapsed() > LINE_TIMEOUT {
            return Err(boot.fail(format!(
                "{}: no `{wanted}` line within {}s",
                boot.arch.name,
                LINE_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(200));
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
        let now = picture(screen, arch, "-autohotkey")?;
        if let Some(area) = window_area(before, &now) {
            return Ok(area);
        }
        if started.elapsed() > WINDOW_TIMEOUT {
            return Err(boot.fail(format!(
                "{}: no AutoHotkey window appeared within {}s of `app open {APP}`",
                arch.name,
                WINDOW_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(POLL);
    }
}

fn content_origin(picture: &Picture, area: Rect) -> Option<(u32, u32)> {
    let probe = area.right().checked_sub(6)?;
    let top = (area.y..area.bottom()).find(|&y| {
        pixel(picture, probe, y) == Some(SURFACE) && pixel(picture, probe, y + 1) == Some(SURFACE)
    })?;
    let left = (area.x..probe).find(|&x| pixel(picture, x, top + 1) == Some(SURFACE))?;
    Some((left, top))
}

fn at(content: (u32, u32), point: (u32, u32)) -> (u32, u32) {
    (content.0 + point.0, content.1 + point.1)
}

fn is(picture: &Picture, content: (u32, u32), point: (u32, u32), colour: u32) -> bool {
    let (x, y) = at(content, point);
    pixel(picture, x, y) == Some(colour)
}

fn window_id(console: &str) -> Option<&str> {
    console
        .lines()
        .rev()
        .find(|line| line.trim_end().ends_with(WINDOW_TITLE))
        .and_then(|line| line.split_whitespace().next())
}

fn shown_tree(console: &str) -> Vec<&str> {
    let shown = format!("{STATUS_PREFIX}shown ");
    let mut lines: Vec<&str> = console
        .lines()
        .rev()
        .skip_while(|line| !line.starts_with(&shown))
        .skip(1)
        .take_while(|line| !line.starts_with(&format!("{STATUS_PREFIX}listed ")))
        .collect();
    lines.reverse();
    lines
}

fn tree_problem(lines: &[&str]) -> Option<&'static str> {
    let has = |parts: &[&str]| {
        lines
            .iter()
            .any(|line| parts.iter().all(|part| line.contains(part)))
    };
    match () {
        () if !has(&["list-item", "selected", "\"Cherries\""]) => {
            Some("no selected `Cherries` list item")
        }
        () if !has(&["list-item", "\"Elderberries\""]) => Some("no `Elderberries` list item"),
        () if !has(&["progress", "\"600\""]) => Some("no progress bar at 600 per mille"),
        () if !has(&["radio", "selected", "\"List\""]) => Some("no selected `List` tab"),
        () => None,
    }
}

struct Session<'s, 'b> {
    boot: &'s mut Boot<'b>,
    screen: &'s mut Screen,
    content: (u32, u32),
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

    fn want(&mut self, shot: &Picture, checks: &[((u32, u32), u32, &str)]) -> Result<(), String> {
        for &(point, colour, what) in checks {
            if !is(shot, self.content, point, colour) {
                let (x, y) = at(self.content, point);
                let found = pixel(shot, x, y);
                return Err(self.fail(format!(
                    "{what}: the pixel at {point:?} of the content is {found:06x?}, not \
                     {colour:06x}"
                )));
            }
        }
        Ok(())
    }

    fn click(&mut self, point: (u32, u32)) -> Result<(), String> {
        let (x, y) = at(self.content, point);
        self.screen.pointer(x, y, None)?;
        self.screen.pointer(x, y, Some(true))?;
        self.screen.pointer(x, y, Some(false))?;
        std::thread::sleep(KEY_PAUSE);
        Ok(())
    }

    fn status_changed(
        &mut self,
        before: &Picture,
        after: &Picture,
        what: &str,
    ) -> Result<(), String> {
        let (x, y, width, height) = STATUS_FIRST_PART;
        let area = Rect::new(self.content.0 + x, self.content.1 + y, width, height);
        match changed(before, after, area) {
            0 => Err(self.fail(format!(
                "{what}: the status bar's first part did not change"
            ))),
            _ => Ok(()),
        }
    }

    fn first_frame(&mut self) -> Result<Picture, String> {
        let first = self.shot("-autohotkey-first")?;
        self.want(
            &first,
            &[
                (ROW_0, HIGHLIGHT, "the list's first row is not selected"),
                (ROW_2, WHITE, "the list's third row is not plain"),
                (
                    PROGRESS_AT_40,
                    BAR_FILL,
                    "the progress bar is not filled to 40",
                ),
                (
                    PROGRESS_PAST_40,
                    BAR_TRACK,
                    "the progress bar is filled past 40",
                ),
                (
                    PICTURE_RING,
                    PICTURE_RING_COLOUR,
                    "the picture's ring is not drawn",
                ),
                (
                    PICTURE_FILL,
                    PICTURE_FILL_COLOUR,
                    "the picture's fill is not drawn",
                ),
                (
                    PICTURE_SQUARE,
                    PICTURE_SQUARE_COLOUR,
                    "the picture's square is not drawn",
                ),
                (
                    STATUS_SEPARATOR,
                    SEPARATOR,
                    "the status bar has no separator at 240",
                ),
            ],
        )?;
        Ok(first)
    }

    fn list_and_toolbar(&mut self, first: &Picture) -> Result<Picture, String> {
        tap(self.screen, &["down"])?;
        tap(self.screen, &["down"])?;
        std::thread::sleep(SETTLE);
        let down = self.shot("-autohotkey-down")?;
        self.want(
            &down,
            &[
                (ROW_2, HIGHLIGHT, "Down, Down did not select the third row"),
                (ROW_0, WHITE, "Down, Down left the first row selected"),
            ],
        )?;
        self.status_changed(first, &down, "Down, Down")?;
        self.click(STEP_BUTTON)?;
        std::thread::sleep(SETTLE);
        let stepped = self.shot("-autohotkey-step")?;
        self.want(
            &stepped,
            &[
                (
                    PROGRESS_PAST_40,
                    BAR_FILL,
                    "`Step` did not fill the progress bar to 60",
                ),
                (
                    PROGRESS_PAST_60,
                    BAR_TRACK,
                    "`Step` filled the progress bar past 60",
                ),
                (
                    ROW_2,
                    HIGHLIGHT,
                    "`Step` took the list's selection or focus",
                ),
            ],
        )?;
        self.status_changed(&down, &stepped, "`Step`")?;
        Ok(stepped)
    }

    fn element_tree(&mut self) -> Result<(), String> {
        type_line(self.boot, &listed_line())?;
        let console = wait_for_line(self.boot, &format!("{STATUS_PREFIX}listed 0"))?;
        let Some(window) = window_id(&console).map(str::to_owned) else {
            return Err(self.fail(format!("`wm list` lists no `{WINDOW_TITLE}` window")));
        };
        type_line(self.boot, &show_line(&window))?;
        let console = wait_for_line(self.boot, &format!("{STATUS_PREFIX}shown 0"))?;
        match tree_problem(&shown_tree(&console)) {
            Some(problem) => Err(self.fail(format!("`wm show {window}`: {problem}"))),
            None => Ok(()),
        }
    }

    fn tree(&mut self, stepped: &Picture) -> Result<(), String> {
        self.click(TREE_TAB)?;
        std::thread::sleep(SETTLE);
        let tree = self.shot("-autohotkey-tree")?;
        self.want(
            &tree,
            &[
                (
                    FRUIT_BOX_PLUS,
                    BLACK,
                    "the `Tree` tab shows no `+` box on `Fruit`",
                ),
                (ROW_2, WHITE, "the `Tree` tab still shows the list"),
            ],
        )?;
        if crop(
            &tree,
            Rect::new(self.content.0, self.content.1 + 64, 480, 208),
        ) == crop(
            stepped,
            Rect::new(self.content.0, self.content.1 + 64, 480, 208),
        ) {
            return Err(self.fail("the `Tree` tab changed nothing on the page".to_owned()));
        }
        self.click(FRUIT_BOX)?;
        std::thread::sleep(SETTLE);
        let expanded = self.shot("-autohotkey-expanded")?;
        self.want(
            &expanded,
            &[
                (FRUIT_BOX_PLUS, WHITE, "`Fruit`'s box did not turn into `-`"),
                (
                    FRUIT_LABEL,
                    HIGHLIGHT,
                    "`Fruit` is not selected in the focused tree",
                ),
            ],
        )?;
        tap(self.screen, &["down"])?;
        std::thread::sleep(SETTLE);
        let apples = self.shot("-autohotkey-apples")?;
        self.want(
            &apples,
            &[
                (APPLES_LABEL, HIGHLIGHT, "Down did not select `Apples`"),
                (FRUIT_LABEL, WHITE, "Down left `Fruit` selected"),
            ],
        )?;
        self.status_changed(&expanded, &apples, "Down in the tree")
    }
}

fn wm_list_lines(console: &str) -> Vec<&str> {
    let after = format!("{STATUS_PREFIX}after ");
    let lines: Vec<&str> = console.lines().collect();
    let end = lines
        .iter()
        .rposition(|line| line.starts_with(&after))
        .unwrap_or(lines.len());
    let start = lines[..end]
        .iter()
        .rposition(|line| line.trim_end() == WM_LIST_HEADER)
        .unwrap_or(end);
    lines[start..end].to_vec()
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
            "{arch}: AutoHotkey's window is still listed after Alt+F4: `{left}`"
        ));
    }
    match clean_exit {
        true => Ok(()),
        false => Err(format!(
            "{arch}: QEMU did not exit cleanly after the AutoHotkey test"
        )),
    }
}

/// Runs the AutoHotkey boot test (see the module documentation) on `kernel`, the release kernel
/// for `arch`, with the host folder `folder` holding the archive, the script and its picture.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-autohotkey-disk.img", arch.name));
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
    wait_for_line(&mut boot, &format!("{STATUS_PREFIX}install 0"))?;
    type_line(&mut boot, &open_line())?;
    wait_for_line(&mut boot, &format!("{STATUS_PREFIX}open 0"))?;
    std::thread::sleep(ECHO_PAUSE);
    let before = picture(&mut screen, arch, "-autohotkey-desktop")?;
    let area = wait_for_window(&mut boot, &mut screen, &before)?;
    std::thread::sleep(SETTLE);
    screen.park_pointer()?;
    std::thread::sleep(POINTER_PAUSE);
    let shown = picture(&mut screen, arch, "-autohotkey-shown")?;
    let Some(content) = content_origin(&shown, area) else {
        return Err(boot.fail(format!(
            "{}: the window at {area:?} shows no content",
            arch.name
        )));
    };
    let mut session = Session {
        boot: &mut boot,
        screen: &mut screen,
        content,
    };
    let first = session.first_frame()?;
    let stepped = session.list_and_toolbar(&first)?;
    session.element_tree()?;
    session.tree(&stepped)?;
    tap(&mut screen, &["alt", "f4"])?;
    std::thread::sleep(SETTLE);
    type_line(&mut boot, &after_line())?;
    wait_for_line(&mut boot, &format!("{STATUS_PREFIX}after 0"))?;
    type_line(&mut boot, POWER_OFF)?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    eprintln!(
        "xtask: {}: AutoHotkey boot test passed (AutoHotkey {VERSION})",
        arch.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_install_with_the_script_and_open_beside_it() {
        assert!(install_line().contains("--entry AutoHotkey64.exe --arguments controls.ahk"));
        assert!(install_line().contains("/media/host/AutoHotkey_2.0.31.zip;"));
        assert!(open_line().contains("cp /media/host/controls.ahk /media/host/egg.png /tmp/;"));
        assert!(open_line().contains("cd /tmp; app open autohotkey;"));
    }

    #[test]
    fn the_window_id_and_its_tree_are_read_from_the_console() {
        let console = format!(
            "{WM_LIST_HEADER}\n1 normal 0 0 640 400 65535 Terminal\n\
             3 normal 80 60 482 390 2 EggOS controls\nAUTOHOTKEY listed 0\n\
             1 window 0,0 480x360 \"EggOS controls\"\n\
             65601 radio-group 8,36 464x240 [focusable]\n\
             1073807360 radio 8,36 52x24 [selected] \"List\"\n\
             1073807425 list-item 13,129 280x20 [selected,focused] \"Cherries\" = \"7\"\n\
             1073807427 list-item 13,169 280x20 \"Elderberries\" = \"13\"\n\
             65605 progress 8,282 424x20 = \"600\"\nAUTOHOTKEY shown 0\n"
        );
        assert_eq!(window_id(&console), Some("3"));
        let tree = shown_tree(&console);
        assert_eq!(tree.len(), 6);
        assert_eq!(tree_problem(&tree), None);
        assert_eq!(
            tree_problem(&tree[..3]),
            Some("no `Elderberries` list item")
        );
    }

    #[test]
    fn the_content_is_found_inside_the_frame() {
        let mut shot = Picture {
            width: 800,
            height: 600,
            pixels: vec![0x0012_161d; 800 * 600],
        };
        for y in 129..489 {
            for x in 101..581 {
                shot.pixels[y * 800 + x] = SURFACE;
            }
        }
        let area = Rect::new(100, 100, 482, 390);
        assert_eq!(content_origin(&shot, area), Some((101, 129)));
        assert_eq!(at((101, 129), ROW_0), (391, 229));
        assert!(is(&shot, (101, 129), (5, 5), SURFACE));
    }

    #[test]
    fn the_verdict_wants_the_install_and_no_window_left() {
        let console = |windows: &str| {
            format!(
                "Installed autohotkey in /Applications/autohotkey.\nAUTOHOTKEY install 0\n\
                 autohotkey\twindows\tx86_64\t/Applications/autohotkey/AutoHotkey64.exe\t\
                 /Applications/autohotkey/app.icon\nOpened autohotkey.\nAUTOHOTKEY open 0\n\
                 {WM_LIST_HEADER}\n{windows}AUTOHOTKEY after 0\n{POWER_DOWN_LINE}\n"
            )
        };
        let good = console("1 normal 0 0 640 400 65535 Terminal\n");
        assert_eq!(verdict("x86_64", &good, true), Ok(()));
        assert!(verdict("x86_64", &good, false).is_err());
        let open = console("3 normal 80 60 482 390 2 EggOS controls\n");
        assert!(
            verdict("x86_64", &open, true)
                .unwrap_err()
                .contains("still listed")
        );
    }
}
