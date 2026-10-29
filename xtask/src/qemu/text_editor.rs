//! The Text Editor boot test (D149): one boot of the release kernel with its display and no
//! disk, each frame compared with the desktop the host renders with the editor's window drawn
//! by the `eggos-editor` crate from the same input over a file system in memory holding the
//! same file (with the caret shown and hidden, the two frames the blink can give).
//!
//! The shell writes `/tmp/notes.txt` and `edit /tmp/notes.txt` opens it in a Text Editor
//! window, focused, its title `notes.txt - Text Editor`; typing `hi ` marks it modified
//! (`*notes.txt - Text Editor`); Ctrl+F and `egg` find the first of two matches, `1 of 2`,
//! selected; Esc closes the find bar; Ctrl+S saves (the `*` goes); the close control closes the
//! window at once; and `cat /tmp/notes.txt` on the console prints the saved text.

use super::settings::{Boot, Screen};
use super::shell::Shell;
use super::*;
use eggos_drivers::desktop::{Catalogue, Profile};
use eggos_editor::editor::{Editor, Memory};
use eggos_editor::editor_window::{DEFAULT_SIZE, EditorWindow, MINIMUM_SIZE, title_of};
use eggos_files::memory::MemoryFiles;
use eggos_toolkit::input::{Input, Modifiers as Held};

const FRAME_TIMEOUT: Duration = Duration::from_secs(20);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(40);
const EDITOR_CLIENT: u16 = 0;
const NOTES_PATH: &str = "/tmp/notes.txt";
const NOTES: &str = "one egg\ntwo eggs\n";
const SETUP: &[u8] =
    b"echo one egg > /tmp/notes.txt; echo two eggs >> /tmp/notes.txt; edit /tmp/notes.txt\r";
const OPENED_LINE: &str = "Opened /tmp/notes.txt in Text Editor.";
const READ_BACK: &[u8] = b"cat /tmp/notes.txt; echo EGGOS-EDITOR read $?\r";
const READ_LINE: &str = "EGGOS-EDITOR read 0";
const SAVED_FIRST_LINE: &str = "hi one egg";
const POWER_OFF: &[u8] = b"poweroff\r";
const PROTOCOL_CTRL: u32 = 2;
const ROOM: usize = 64 * 1024;

mod code {
    pub const E: u16 = 18;
    pub const I: u16 = 23;
    pub const F: u16 = 33;
    pub const G: u16 = 34;
    pub const H: u16 = 35;
    pub const S: u16 = 31;
    pub const SPACE: u16 = 57;
}

struct Room {
    text: Vec<u8>,
    starts: Vec<u32>,
    history: Vec<u8>,
    clipboard: Vec<u8>,
}

impl Room {
    fn new() -> Self {
        Self {
            text: vec![0; ROOM],
            starts: vec![0; ROOM + 1],
            history: vec![0; 2 * ROOM + 1024],
            clipboard: vec![0; ROOM],
        }
    }

    fn editor(&mut self) -> Editor<'_> {
        Editor::new(Memory {
            text: &mut self.text,
            starts: &mut self.starts,
            history: &mut self.history,
            clipboard: &mut self.clipboard,
        })
    }
}

struct EditorTest<'a, 'b> {
    shell: Shell<'a>,
    disk: MemoryFiles,
    editor: Option<(WindowId, EditorWindow<'b>)>,
}

fn key_of(character: char) -> Option<(&'static str, u16)> {
    match character {
        'e' => Some(("e", code::E)),
        'g' => Some(("g", code::G)),
        'h' => Some(("h", code::H)),
        'i' => Some(("i", code::I)),
        ' ' => Some(("spc", code::SPACE)),
        _ => None,
    }
}

impl EditorTest<'_, '_> {
    fn arch(&self) -> &'static str {
        self.shell.arch()
    }

    fn feed(&mut self, input: Input) {
        let now = self.shell.now_ms;
        if let Some((_, editor)) = self.editor.as_mut() {
            editor.handle(&mut self.disk, input, now);
        }
    }

    fn sync_title(&mut self) -> Result<(), String> {
        let arch = self.arch();
        let Some((id, editor)) = self.editor.as_mut() else {
            return Ok(());
        };
        let Some(title) = editor.take_title() else {
            return Ok(());
        };
        self.shell
            .screen
            .desktop
            .set_title(
                EDITOR_CLIENT,
                u32::from(*id),
                title.as_bytes(),
                &mut Damage::new(),
            )
            .map_err(|error| format!("{arch}: the host's desktop refused a title: {error:?}"))
    }

    fn check(&mut self, at: &str, timeout: Duration) -> Result<(), String> {
        self.sync_title()?;
        let (width, height) = DEFAULT_SIZE;
        let alternatives: Vec<Vec<(WindowId, WindowContent)>> = match &self.editor {
            Some((id, editor)) => [true, false]
                .into_iter()
                .map(|caret| {
                    let mut pixels = vec![0; (width * height) as usize];
                    editor.render(&mut pixels, width, height, 1, caret);
                    vec![(*id, WindowContent::Pixels { width, pixels })]
                })
                .collect(),
            None => vec![Vec::new()],
        };
        let alternatives: Vec<&[(WindowId, WindowContent)]> =
            alternatives.iter().map(Vec::as_slice).collect();
        let shell = &mut self.shell;
        check_desktop_frame_among(
            shell.boot.arch,
            &mut shell.screen.qmp,
            &mut shell.boot.console,
            &shell.boot.output,
            at,
            &shell.screen.desktop,
            &alternatives,
            timeout,
        )
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
            self.feed(Input::Key {
                code,
                pressed,
                modifiers: Held(protocol),
                character: character as u32,
            });
        }
        if ctrl {
            self.shell
                .key("ctrl", key::LEFT_CTRL, false, Modifiers::NONE)?;
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), String> {
        for character in text.chars() {
            let (qcode, code) = key_of(character)
                .ok_or_else(|| format!("{}: the test cannot type {character:?}", self.arch()))?;
            self.key(qcode, code, character, false)?;
        }
        Ok(())
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
}

fn guest_disk() -> MemoryFiles {
    let mut disk = MemoryFiles::new();
    disk.folder("/tmp")
        .folder("/home")
        .folder("/home/root")
        .file(NOTES_PATH, NOTES.as_bytes(), 0)
        .device("/tmp", 3);
    disk
}

fn open_the_file<'b>(test: &mut EditorTest<'_, 'b>, room: &'b mut Room) -> Result<(), String> {
    let prompts = test.shell.boot.prompts();
    test.type_on_console(SETUP)?;
    test.shell.wait_for_line(OPENED_LINE)?;
    test.shell.boot.wait_for_prompts(prompts + 1)?;
    test.shell.screen.park_pointer()?;
    let (width, height) = DEFAULT_SIZE;
    let title = title_of("notes.txt", false);
    let id = super::install::host_window(
        &mut test.shell,
        EDITOR_CLIENT,
        title.as_str(),
        (width, height),
        Some(MINIMUM_SIZE),
    )?;
    let now = test.shell.now_ms;
    let mut editor = EditorWindow::new(
        room.editor(),
        &mut test.disk,
        Some(NOTES_PATH.as_bytes()),
        now,
    );
    let _ = editor.take_title();
    test.editor = Some((id, editor));
    test.check(
        "the Text Editor opened by `edit` on notes.txt",
        WINDOW_TIMEOUT,
    )
}

fn type_find_and_save(test: &mut EditorTest<'_, '_>) -> Result<(), String> {
    test.type_text("hi ")?;
    test.check("`hi ` typed, the title marked modified", FRAME_TIMEOUT)?;
    test.key("f", code::F, '\0', true)?;
    test.type_text("egg")?;
    test.check("Ctrl+F and `egg`: the first of two matches", FRAME_TIMEOUT)?;
    test.key("esc", key::ESC, '\u{1b}', false)?;
    test.check("Esc closed the find bar", FRAME_TIMEOUT)?;
    test.key("s", code::S, '\0', true)?;
    test.check("Ctrl+S saved: the title unmarked", FRAME_TIMEOUT)?;
    let arch = test.arch();
    let saved = test
        .disk
        .contents(NOTES_PATH)
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
    match saved.as_deref() {
        Some(text) if text.starts_with(SAVED_FIRST_LINE) => Ok(()),
        other => Err(format!("{arch}: the host's editor saved {other:?}")),
    }
}

fn close_the_editor(test: &mut EditorTest<'_, '_>) -> Result<(), String> {
    let arch = test.arch();
    let (id, mut editor) = test
        .editor
        .take()
        .ok_or_else(|| format!("{arch}: the Text Editor is not open"))?;
    if !editor.close_requested() {
        return Err(format!(
            "{arch}: the host's editor asked before closing a saved file"
        ));
    }
    let close = test.shell.rect(
        test.shell.screen.desktop.close_control(id),
        "the Text Editor's close control",
    )?;
    test.shell.click(close)?;
    super::install::host_destroy(&mut test.shell, EDITOR_CLIENT, id)?;
    test.shell.screen.park_pointer()?;
    test.check(
        "the Text Editor closed by its close control",
        WINDOW_TIMEOUT,
    )
}

fn editor_steps<'b>(test: &mut EditorTest<'_, 'b>, room: &'b mut Room) -> Result<(), String> {
    test.shell.screen.park_pointer()?;
    test.check("the first prompt", FRAME_TIMEOUT)?;
    open_the_file(test, room)?;
    type_find_and_save(test)?;
    close_the_editor(test)?;
    let prompts = test.shell.boot.prompts();
    test.type_on_console(READ_BACK)?;
    test.shell.wait_for_line(READ_LINE)?;
    test.shell.boot.wait_for_prompts(prompts + 1)
}

fn read_back(arch: &Arch, console: &str) -> Result<(), String> {
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    let at = lines.iter().rposition(|line| *line == READ_LINE);
    let shown = at.map(|at| &lines[at.saturating_sub(2)..at]);
    match shown {
        Some([first, second]) if *first == SAVED_FIRST_LINE && *second == "two eggs" => Ok(()),
        _ => Err(format!(
            "{}: `cat` did not print the saved text before `{READ_LINE}`: {shown:?}",
            arch.name
        )),
    }
}

/// Runs the Text Editor boot test (see the module documentation) on `kernel`, the release
/// kernel for `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, &[])?;
    boot.wait_for_prompts(1)?;
    let mut screen = match Screen::new(&boot, Profile::Macos) {
        Ok(screen) => screen,
        Err(error) => return Err(boot.fail(error)),
    };
    let mut catalogue = Catalogue::new();
    catalogue.set_files_present(true);
    catalogue.set_text_editor_present(true);
    screen
        .desktop
        .set_catalogue(catalogue, &mut Damage::new(), &mut DiscardEvents);
    let mut room = Room::new();
    let mut test = EditorTest {
        shell: Shell {
            boot,
            screen,
            now_ms: 0,
        },
        disk: guest_disk(),
        editor: None,
    };
    let stepped = editor_steps(&mut test, &mut room);
    let EditorTest {
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
    read_back(arch, &console)?;
    match console
        .lines()
        .any(|line| line.trim_end() == POWER_DOWN_LINE)
        && clean_exit
    {
        true => {
            eprintln!("xtask: {}: text editor boot test passed", arch.name);
            Ok(())
        }
        false => Err(format!(
            "{}: QEMU did not power off cleanly after the Text Editor test",
            arch.name
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_mirror_opens_the_file_as_the_guest_does() {
        let mut room = Room::new();
        let mut disk = guest_disk();
        let editor = EditorWindow::new(room.editor(), &mut disk, Some(NOTES_PATH.as_bytes()), 0);
        assert_eq!(editor.title(), "notes.txt - Text Editor");
        assert_eq!(editor.editor().document().bytes(), NOTES.as_bytes());
        for character in "hi egg".chars() {
            assert!(key_of(character).is_some(), "{character:?}");
        }
    }

    #[test]
    fn the_read_back_needs_both_saved_lines_before_the_marker() {
        let arch = crate::ARCHES
            .iter()
            .find(|arch| arch.name == "x86_64")
            .expect("x86_64 is an architecture");
        let good = format!("eggsh:/# cat\n{SAVED_FIRST_LINE}\ntwo eggs\n{READ_LINE}\n");
        assert_eq!(read_back(arch, &good), Ok(()));
        let bad = format!("one egg\ntwo eggs\n{READ_LINE}\n");
        assert!(read_back(arch, &bad).is_err());
    }
}
