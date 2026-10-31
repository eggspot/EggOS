//! The Files boot test (D135): one boot of the install test's release kernel on a blank disk
//! with the host folder holding `win-paint.exe`, our static Linux program and a text file, each
//! frame compared with the desktop the host renders with the Files window drawn by the
//! `eggos-files` crate from the same input over a file system in memory that mirrors the
//! guest's (the rows' times, which follow the guest's clock, are not compared), and the disk
//! image read back with `eggos-eggfs` after the power-off.
//!
//! The dock's Files opens the window at home; the sidebar's `Host share` lists the share; a
//! double-click on the Linux program runs it in the Terminal window; back in Files a
//! double-click on `win-paint.exe` opens the installer's prompt (on riscv64 its refusal), which
//! Esc closes; Ctrl+C on the text file and Ctrl+V at home copy it out of the share; `^` shows
//! `/home`; Ctrl+N makes `New folder`, F2 renames it `work`; Ctrl+X and Ctrl+V move the file into
//! it; Delete moves it to the Trash, whose place lists it; the close control closes Files;
//! `trash -l` lists the item with where it was; and the image holds the empty folder and the
//! trashed file with its record.

use super::install::{
    LINUX_PROGRAM, WINDOWS_PROGRAM, entries_at, file_at, host_destroy, host_window, image_volume,
    node_at,
};
use super::settings::{Boot, Screen};
use super::shell::Shell;
use super::*;
use eggos_app::architecture::Architecture;
use eggos_app::dialog::{self as installer_window, Dialog};
use eggos_drivers::desktop::{Action, AppId, Catalogue, Profile};
use eggos_files::files_window::{
    DEFAULT_SIZE, FilesWindow, MINIMUM_SIZE, Place, Request, TITLE, UP_BUTTON,
};
use eggos_files::memory::MemoryFiles;
use eggos_toolkit::geometry::Rect as ContentRect;
use eggos_toolkit::input::{Input, Modifiers as Held, MouseButton};

/// The text file the host folder holds for this test: its name and contents.
pub const HOST_TEXT_FILE: (&str, &str) = ("notes.txt", "Notes from the host folder.\n");

const SHELL_HISTORY_NAME: &str = ".eggsh_history";
const TEST_DISK_BYTES: u64 = 64 << 20;
const FRAME_TIMEOUT: Duration = Duration::from_secs(20);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(40);
const FILES_CLIENT: u16 = 0;
const INSTALLER_CLIENT: u16 = 1;
const HOST_CLOCK: u64 = 0;
const FOLDER_NAME: &str = "work";
const LISTED_LINE: &str = "EGGOS-FILES listed 0";
const LINUX_PROGRAM_LAST_LINE: &str = "LINUX-STATIC stderr";
const POWER_OFF: &[u8] = b"poweroff\r";
const RECORD_START: &str = "[Trash Info]\nPath=/home/root/work/notes.txt\nDeletionDate=";
const PROTOCOL_CTRL: u32 = 2;

mod code {
    pub const C: u16 = 46;
    pub const X: u16 = 45;
    pub const V: u16 = 47;
    pub const N: u16 = 49;
    pub const W: u16 = 17;
    pub const O: u16 = 24;
    pub const R: u16 = 19;
    pub const K: u16 = 37;
}

struct FilesTest<'a> {
    shell: Shell<'a>,
    disk: MemoryFiles,
    files: Option<(WindowId, FilesWindow)>,
    installer: Option<(WindowId, Dialog)>,
    machine: Architecture,
}

fn guest_disk(arch: &Arch, folder: &Path) -> Result<MemoryFiles, String> {
    let mut disk = MemoryFiles::new();
    for path in [
        "/bin",
        "/dev",
        "/proc",
        "/tmp",
        "/home",
        "/Applications",
        "/media/host",
        "/usr/libexec",
    ] {
        disk.folder(path);
    }
    for name in [WINDOWS_PROGRAM, LINUX_PROGRAM, HOST_TEXT_FILE.0] {
        let bytes = std::fs::read(folder.join(name))
            .map_err(|e| format!("{}: {}: {e}", arch.name, folder.join(name).display()))?;
        disk.file(&format!("/media/host/{name}"), &bytes, 0);
    }
    if runs_windows_programs(arch.name) {
        disk.file("/usr/lib/eggos/windows", b"\x7fELF layer", 0);
    }
    if crate::macos::layer_runs_on(arch.name) {
        disk.file("/usr/lib/eggos/macos", b"\x7fELF layer", 0);
    }
    disk.device("/home", 1)
        .device("/Applications", 2)
        .device("/tmp", 3)
        .device("/media/host", 4)
        .read_only("/media/host");
    Ok(disk)
}

fn to_screen(content: Rect, rect: ContentRect) -> Rect {
    Rect::new(
        content.x + rect.x.max(0) as u32,
        content.y + rect.y.max(0) as u32,
        rect.width.max(0) as u32,
        rect.height.max(0) as u32,
    )
}

impl FilesTest<'_> {
    fn arch(&self) -> &'static str {
        self.shell.arch()
    }

    fn files(&mut self) -> Result<&mut FilesWindow, String> {
        let arch = self.arch();
        self.files
            .as_mut()
            .map(|(_, files)| files)
            .ok_or_else(|| format!("{arch}: Files is not open"))
    }

    fn content(&self) -> Result<Rect, String> {
        let (id, _) = self
            .files
            .as_ref()
            .ok_or_else(|| format!("{}: Files is not open", self.arch()))?;
        self.shell
            .screen
            .desktop
            .content_rect(*id)
            .ok_or_else(|| format!("{}: the Files window is gone", self.arch()))
    }

    fn feed(&mut self, input: Input, time_ms: u32) -> Result<(), String> {
        let (files, disk) = match self.files.as_mut() {
            Some((_, files)) => (files, &mut self.disk),
            None => return Ok(()),
        };
        files.handle(disk, input, time_ms, HOST_CLOCK);
        Ok(())
    }

    fn check(&mut self, at: &str, timeout: Duration) -> Result<(), String> {
        let (width, height) = DEFAULT_SIZE;
        let mut windows = Vec::new();
        let mut masks = Vec::new();
        if let Some((id, files)) = &self.files {
            let mut pixels = vec![0; (width * height) as usize];
            files.render(&mut pixels, width, height, 1);
            windows.push((*id, WindowContent::Pixels { width, pixels }));
            if let Some(content) = self.shell.screen.desktop.content_rect(*id) {
                masks.push(to_screen(content, files.time_column()));
                masks.push(to_screen(content, files.free_space_area()));
            }
        }
        if let Some((id, dialog)) = &self.installer {
            let (width, height) = installer_window::SIZE;
            let mut pixels = vec![0; (width * height) as usize];
            dialog.widgets().render(&mut pixels, width, height, 1);
            windows.push((*id, WindowContent::Pixels { width, pixels }));
        }
        let shell = &mut self.shell;
        check_desktop_frame_masked(
            shell.boot.arch,
            &mut shell.screen.qmp,
            &mut shell.boot.console,
            &shell.boot.output,
            at,
            &shell.screen.desktop,
            &[&windows],
            &masks,
            timeout,
        )
    }

    fn park(&mut self) -> Result<(), String> {
        self.shell.screen.park_pointer()?;
        let now = self.shell.now_ms;
        self.feed(Input::PointerLeave, now)
    }

    fn click(&mut self, rect: ContentRect) -> Result<(), String> {
        let content = self.content()?;
        let screen = to_screen(content, rect);
        self.shell.click(screen)?;
        self.shell.now_ms += 1000;
        let now = self.shell.now_ms;
        let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        self.feed(Input::PointerMove { x, y }, now)?;
        for pressed in [true, false] {
            self.feed(
                Input::Button {
                    x,
                    y,
                    button: MouseButton::Left,
                    pressed,
                },
                now,
            )?;
        }
        Ok(())
    }

    fn double_click(&mut self, rect: ContentRect) -> Result<(), String> {
        let content = self.content()?;
        self.shell.double_click(to_screen(content, rect))?;
        let now = self.shell.now_ms;
        let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        self.feed(Input::PointerMove { x, y }, now)?;
        for (offset, pressed) in [(0, true), (40, false), (80, true), (120, false)] {
            self.feed(
                Input::Button {
                    x,
                    y,
                    button: MouseButton::Left,
                    pressed,
                },
                now + offset,
            )?;
        }
        Ok(())
    }

    fn key(
        &mut self,
        qcode: &'static str,
        code: u16,
        character: char,
        ctrl: bool,
    ) -> Result<(), String> {
        let (held, protocol) = match ctrl {
            true => (Modifiers::CTRL, PROTOCOL_CTRL),
            false => (Modifiers::NONE, 0),
        };
        if ctrl {
            self.shell
                .key("ctrl", key::LEFT_CTRL, true, Modifiers::NONE)?;
        }
        for pressed in [true, false] {
            self.shell.key(qcode, code, pressed, held)?;
            let now = self.shell.now_ms;
            self.feed(
                Input::Key {
                    code,
                    pressed,
                    modifiers: Held(protocol),
                    character: character as u32,
                },
                now,
            )?;
        }
        if ctrl {
            self.shell
                .key("ctrl", key::LEFT_CTRL, false, Modifiers::NONE)?;
        }
        Ok(())
    }

    fn row(&mut self, name: &str) -> Result<ContentRect, String> {
        let arch = self.arch();
        let files = self.files()?;
        let index = files
            .listing()
            .find(name.as_bytes())
            .ok_or_else(|| format!("{arch}: the host's Files lists no {name}"))?;
        Ok(files.row_rect(index))
    }

    fn place(&mut self, place: Place) -> Result<(), String> {
        let arch = self.arch();
        let files = self.files()?;
        let index = files
            .places()
            .iter()
            .position(|shown| *shown == place)
            .ok_or_else(|| format!("{arch}: the host's Files has no {place:?} place"))?;
        let rect = files.place_rect(index);
        self.click(rect)
    }

    fn take_request(&mut self) -> Result<Request, String> {
        let arch = self.arch();
        self.files()?
            .take_request()
            .ok_or_else(|| format!("{arch}: the host's Files asked nothing of the system"))
    }

    fn focus_files_from_the_dock(&mut self) -> Result<(), String> {
        let dock = self.shell.rect(
            self.shell.screen.desktop.dock_icon(AppId::Files),
            "dock Files",
        )?;
        let action = self.shell.click(dock)?;
        self.shell
            .expect(action, Action::None, "the dock's Files with its window")?;
        let now = self.shell.now_ms;
        self.feed(Input::FocusIn, now)
    }
}

fn open_files(test: &mut FilesTest<'_>) -> Result<(), String> {
    let dock = test.shell.rect(
        test.shell.screen.desktop.dock_icon(AppId::Files),
        "dock Files",
    )?;
    let action = test.shell.click(dock)?;
    test.shell
        .expect(action, Action::Start(AppId::Files), "the dock's Files")?;
    test.shell.screen.park_pointer()?;
    let (width, height) = DEFAULT_SIZE;
    let id = host_window(
        &mut test.shell,
        FILES_CLIENT,
        TITLE,
        (width, height),
        Some(MINIMUM_SIZE),
    )?;
    let files = FilesWindow::new(&mut test.disk, None, test.machine, false, HOST_CLOCK);
    test.files = Some((id, files));
    test.check("Files opened at home from the dock", WINDOW_TIMEOUT)
}

fn browse_the_share_and_open_its_programs(test: &mut FilesTest<'_>) -> Result<(), String> {
    test.place(Place::HostShare)?;
    test.park()?;
    test.check("Files on the host share", FRAME_TIMEOUT)?;
    let prompts = test.shell.boot.prompts();
    let program = test.row(LINUX_PROGRAM)?;
    test.double_click(program)?;
    match test.take_request()? {
        Request::TypeInTerminal(_) => {}
        other => return Err(format!("{}: Files asked {other:?}", test.arch())),
    }
    test.shell
        .screen
        .desktop
        .launch(AppId::Terminal, &mut Damage::new(), &mut DiscardEvents);
    let now = test.shell.now_ms;
    test.feed(Input::FocusOut, now)?;
    test.park()?;
    test.shell.wait_for_line(LINUX_PROGRAM_LAST_LINE)?;
    test.shell.boot.wait_for_prompts(prompts + 1)?;
    test.check(
        "the Linux program run in the Terminal from Files",
        FRAME_TIMEOUT,
    )?;
    test.focus_files_from_the_dock()?;
    test.park()?;
    test.check("Files focused again from the dock", FRAME_TIMEOUT)?;
    let paint = test.row(WINDOWS_PROGRAM)?;
    test.double_click(paint)?;
    let path = match test.take_request()? {
        Request::Install(path) => path,
        other => return Err(format!("{}: Files asked {other:?}", test.arch())),
    };
    let (dialog, _) =
        installer_window::install_prompt(&mut test.disk, path.as_bytes(), None, test.machine);
    let (width, height) = installer_window::SIZE;
    let window = host_window(
        &mut test.shell,
        INSTALLER_CLIENT,
        installer_window::INSTALL_TITLE,
        (width, height),
        Some((width, height)),
    )?;
    test.installer = Some((window, dialog));
    let now = test.shell.now_ms;
    test.feed(Input::FocusOut, now)?;
    test.park()?;
    test.check("the installer's window opened from Files", WINDOW_TIMEOUT)?;
    test.shell.tap("esc", key::ESC, Modifiers::NONE)?;
    host_destroy(&mut test.shell, INSTALLER_CLIENT, window)?;
    test.installer = None;
    let now = test.shell.now_ms;
    test.feed(Input::FocusIn, now)?;
    test.check("the installer closed, Files focused", WINDOW_TIMEOUT)
}

fn copy_create_rename_move_and_trash(test: &mut FilesTest<'_>) -> Result<(), String> {
    let (name, _) = HOST_TEXT_FILE;
    let notes = test.row(name)?;
    test.click(notes)?;
    test.key("c", code::C, 'c', true)?;
    test.place(Place::Home)?;
    test.key("v", code::V, 'v', true)?;
    test.park()?;
    test.check(
        "the text file copied out of the share into home",
        FRAME_TIMEOUT,
    )?;
    test.click(UP_BUTTON)?;
    test.park()?;
    test.check("/home after ^", FRAME_TIMEOUT)?;
    test.key("ret", key::ENTER, '\r', false)?;
    test.key("n", code::N, 'n', true)?;
    test.key("ret", key::ENTER, '\r', false)?;
    test.park()?;
    test.check("New folder made at home", FRAME_TIMEOUT)?;
    test.key("f2", key::F2, '\0', false)?;
    for _ in 0.."New folder".len() {
        test.key("backspace", key::BACKSPACE, '\u{8}', false)?;
    }
    for (qcode, code, character) in [
        ("w", code::W, 'w'),
        ("o", code::O, 'o'),
        ("r", code::R, 'r'),
        ("k", code::K, 'k'),
    ] {
        test.key(qcode, code, character, false)?;
    }
    test.key("ret", key::ENTER, '\r', false)?;
    test.check("New folder renamed work", FRAME_TIMEOUT)?;
    let notes = test.row(name)?;
    test.click(notes)?;
    test.key("x", code::X, 'x', true)?;
    let folder = test.row(FOLDER_NAME)?;
    test.double_click(folder)?;
    test.key("v", code::V, 'v', true)?;
    test.park()?;
    test.check("the text file moved into work", FRAME_TIMEOUT)?;
    let notes = test.row(name)?;
    test.click(notes)?;
    test.key("delete", key::DELETE, '\0', false)?;
    test.park()?;
    test.check("the text file moved to the Trash", FRAME_TIMEOUT)?;
    test.place(Place::Trash)?;
    test.park()?;
    test.check("the Trash listing the text file", FRAME_TIMEOUT)
}

fn close_files(test: &mut FilesTest<'_>) -> Result<(), String> {
    let arch = test.arch();
    let (id, _) = test
        .files
        .take()
        .ok_or_else(|| format!("{arch}: Files is not open"))?;
    let close = test.shell.rect(
        test.shell.screen.desktop.close_control(id),
        "Files' close control",
    )?;
    test.shell.click(close)?;
    host_destroy(&mut test.shell, FILES_CLIENT, id)?;
    test.shell.screen.park_pointer()?;
    test.check("Files closed by its close control", WINDOW_TIMEOUT)
}

fn trash_listed(arch: &Arch, console: &str) -> Result<(), String> {
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    let listed = lines.iter().any(|line| {
        line.starts_with(&format!("{}\t", HOST_TEXT_FILE.0))
            && line.ends_with("\t/home/root/work/notes.txt")
    });
    match listed && lines.contains(&LISTED_LINE) {
        true => Ok(()),
        false => Err(format!(
            "{}: `trash -l` did not list notes.txt from /home/root/work",
            arch.name
        )),
    }
}

fn check_the_image(arch: &Arch, image: &Path) -> Result<(), String> {
    let fail = |what: String| format!("{}: {what}", arch.name);
    let mut volume = image_volume(image).map_err(fail)?;
    let mut home = entries_at(&mut volume, "home/root").map_err(fail)?;
    home.sort();
    if home != [SHELL_HISTORY_NAME, ".local", FOLDER_NAME] {
        return Err(fail(format!("/home/root holds {home:?}")));
    }
    let work = entries_at(&mut volume, "home/root/work").map_err(fail)?;
    if !work.is_empty() {
        return Err(fail(format!("/home/root/work still holds {work:?}")));
    }
    let trashed =
        file_at(&mut volume, "home/root/.local/share/Trash/files/notes.txt").map_err(fail)?;
    if trashed.as_deref() != Some(HOST_TEXT_FILE.1.as_bytes()) {
        return Err(fail(format!("the Trash holds {trashed:?} for notes.txt")));
    }
    let record = file_at(
        &mut volume,
        "home/root/.local/share/Trash/info/notes.txt.trashinfo",
    )
    .map_err(fail)?
    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    if !record
        .as_deref()
        .is_some_and(|record| record.starts_with(RECORD_START))
    {
        return Err(fail(format!("the Trash's record is {record:?}")));
    }
    if node_at(&mut volume, "home/root/notes.txt")
        .map_err(fail)?
        .is_some()
    {
        return Err(fail("/home/root/notes.txt is still there".into()));
    }
    Ok(())
}

fn files_steps(test: &mut FilesTest<'_>) -> Result<(), String> {
    test.shell.screen.park_pointer()?;
    test.check("the first prompt with the host share's icon", FRAME_TIMEOUT)?;
    open_files(test)?;
    browse_the_share_and_open_its_programs(test)?;
    copy_create_rename_move_and_trash(test)?;
    close_files(test)?;
    let prompts = test.shell.boot.prompts();
    let arch = test.arch();
    test.shell
        .boot
        .keyboard
        .write_all(b"trash -l; echo EGGOS-FILES listed $?\r")
        .and_then(|()| test.shell.boot.keyboard.flush())
        .map_err(|e| format!("{arch}: could not type on the console: {e}"))?;
    test.shell.wait_for_line(LISTED_LINE)?;
    test.shell.boot.wait_for_prompts(prompts + 1)
}

fn files_boot(arch: &Arch, kernel: &Path, args: &[String], folder: &Path) -> Result<(), String> {
    let disk = guest_disk(arch, folder)?;
    let mut boot = Boot::start(arch, kernel, args)?;
    boot.wait_for_prompts(1)?;
    let mut screen = match Screen::new(&boot, Profile::Macos) {
        Ok(screen) => screen,
        Err(error) => return Err(boot.fail(error)),
    };
    let mut catalogue = Catalogue::new();
    catalogue.set_host_share(true);
    catalogue.set_files_present(true);
    screen
        .desktop
        .set_catalogue(catalogue, &mut Damage::new(), &mut DiscardEvents);
    let mut test = FilesTest {
        shell: Shell {
            boot,
            screen,
            now_ms: 0,
        },
        disk,
        files: None,
        installer: None,
        machine: Architecture::from_machine(arch.name.as_bytes()),
    };
    let stepped = files_steps(&mut test);
    let FilesTest {
        shell: Shell { mut boot, .. },
        ..
    } = test;
    if let Err(error) = stepped {
        return Err(boot.fail(error));
    }
    boot.keyboard
        .write_all(POWER_OFF)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    let (console, clean_exit) = boot.finish()?;
    trash_listed(arch, &console)?;
    match console
        .lines()
        .any(|line| line.trim_end() == POWER_DOWN_LINE)
        && clean_exit
    {
        true => Ok(()),
        false => Err(format!(
            "{}: QEMU did not power off cleanly after the Files test",
            arch.name
        )),
    }
}

/// Runs the Files boot test (see the module documentation) on `kernel`, the install test's
/// release kernel for `arch`, with `folder` (the install test's host folder, which also holds
/// [`HOST_TEXT_FILE`]) attached.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-files-disk.img", arch.name));
    reset_disk_image(&image, TEST_DISK_BYTES)?;
    let mut args = disk_args(arch, &image.to_string_lossy());
    args.extend(host_folder_args(arch, &folder.to_string_lossy()));
    files_boot(arch, kernel, &args, folder)?;
    check_the_image(arch, &image)?;
    eprintln!("xtask: {}: files boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_mirror_lists_the_share_as_the_guest_does() {
        let folder = crate::target_directory().join("files-test-mirror");
        std::fs::create_dir_all(&folder).unwrap();
        for name in [WINDOWS_PROGRAM, LINUX_PROGRAM, HOST_TEXT_FILE.0] {
            std::fs::write(folder.join(name), b"MZ").unwrap();
        }
        let arch = crate::ARCHES
            .iter()
            .find(|arch| arch.name == "x86_64")
            .expect("x86_64 is an architecture");
        let mut disk = guest_disk(arch, &folder).unwrap();
        let mut files = FilesWindow::new(
            &mut disk,
            Some(b"/media/host"),
            Architecture::X86_64,
            false,
            0,
        );
        assert!(files.places().contains(&Place::HostShare));
        assert_eq!(files.listing().len(), 3);
        files.reload(&mut disk);
        assert_eq!(
            disk.node("/home/root"),
            Some(&eggos_files::memory::Node::Folder)
        );
    }
}
