//! The window of an installed app's own actions (D301, docs/ux/app-actions-v0.5.md):
//! `app-window do <app> [<action>]`, which `app do --window` and the desktop's app menus
//! start. Without an action it lists them all; with one it asks for its arguments, asks before
//! an action that destroys, and runs it as `app do` does, by running `app do` itself, so the
//! grants, the audit record and the errors are the command's (D22). The program's output and
//! exit status end in the result screen.

use crate::dialog::{self, AppIcon};
use crate::files::Files;
use crate::installer::APPLICATIONS;
use crate::linux::LinuxFiles;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_app_manifest::actions::{self, Action, ArgumentType, Effect};
use eggos_runtime::errno::Errno;
use eggos_runtime::process::{ChildStack, Spawn, spawn};
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application::{self, Application};
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Align, Column, Escape, Kind, Length, Place};
use eggos_toolkit::window::Window;

/// The window's size: one height for every screen.
pub const SIZE: (u32, u32) = (480, 400);
/// The most widgets one screen holds.
pub const CAPACITY: usize = 48;

const WIDTH: i32 = 480;
const PADDING: i32 = 16;
const ICON: i32 = 48;
const TEXT_X: i32 = 80;
const TEXT_WIDTH: i32 = 384;
const TITLE_TOP: i32 = 16;
const DESCRIPTION_TOPS: [i32; 2] = [36, 56];
const EFFECT_TOP: i32 = 76;
const BODY_TOP: i32 = 104;
const FIELD_PITCH: i32 = 60;
const CONTROL_GAP: i32 = 20;
const TEXT_FIELD_WIDTH: i32 = 448;
const SHORT_CONTROL_WIDTH: i32 = 160;
const CONTROL_HEIGHT: i32 = 28;
const BUTTON_WIDTH: i32 = 96;
const BUTTONS_TOP: i32 = 356;
const SECOND_X: i32 = 264;
const MAIN_X: i32 = 368;
const STATUS_WIDTH: i32 = 240;
const STATUS_DROP: i32 = 6;
const WORD_END: i32 = 464;
const DESCRIPTION_WIDTH: i32 = 336;
const TABLE: (i32, i32, i32, i32) = (16, 80, 448, 260);
const AREA: (i32, i32, i32, i32) = (16, 84, 448, 216);
const VALUE_PITCH: i32 = 20;
const ACTION_BUTTON_GLYPHS: usize = 20;
const GLYPH: i32 = 8;
const ACTION_BUTTON_PADDING: i32 = 24;
const BUTTON_GAP: i32 = 8;
const OUTPUT_LIMIT: usize = 64 * 1024;
const READ_CHUNK: usize = 4096;
const POLL_MS: u32 = 50;
const O_CLOEXEC: usize = 0o2_000_000;
const O_NONBLOCK: usize = 0o4_000;
const WNOHANG: usize = 1;
const SIGTERM: usize = 15;
const APP_PROGRAM: &core::ffi::CStr = c"/bin/app";
const REQUIRED_BOOLEAN: [&str; 2] = ["No", "Yes"];
const OPTIONAL_BOOLEAN: [&str; 3] = ["Not set", "No", "Yes"];
const OUTPUT_CUT: &str = "Output cut. See all of it in the Terminal: app do";

static mut STACK: ChildStack = ChildStack::new();
static mut LIST_CELLS: [[(usize, [u8; 64]); 3]; actions::MAX_ACTIONS] =
    [[(0, [0; 64]); 3]; actions::MAX_ACTIONS];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    List,
    Form,
    Confirm { from_form: bool },
    Running,
    Result,
}

#[derive(Debug, Clone, Copy)]
struct FieldIds {
    word: WidgetId,
    control: WidgetId,
}

#[derive(Debug, Clone, Default)]
struct Ids {
    second: Option<WidgetId>,
    main: Option<WidgetId>,
    back: Option<WidgetId>,
    status: Option<WidgetId>,
    table: Option<WidgetId>,
    area: Option<WidgetId>,
    fields: Vec<FieldIds>,
}

#[derive(Debug, Clone, Copy)]
struct Run {
    child: usize,
    pipe: usize,
    stopped: bool,
}

struct ActionWindow {
    tree: WidgetTree<CAPACITY, 1>,
    ids: Ids,
    screen: Screen,
    app: &'static [u8],
    app_title: String,
    icon: AppIcon,
    file: &'static [u8],
    chosen: Option<Action<'static>>,
    values: Vec<String>,
    from_list: bool,
    run: Option<Run>,
    output: Vec<u8>,
}

fn close() -> ! {
    syscall::exit(0)
}

fn shortened(words: &str, glyphs: usize) -> String {
    if words.chars().count() <= glyphs {
        return String::from(words);
    }
    let kept: String = words.chars().take(glyphs.saturating_sub(3)).collect();
    format!("{kept}...")
}

fn lossy(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| match byte {
            b'\n' | b'\t' | 0x20..=0x7e => char::from(byte),
            _ => '?',
        })
        .collect()
}

fn title_of(action: &Action<'_>) -> String {
    match action.title.is_empty() {
        true => lossy(action.name),
        false => lossy(action.title),
    }
}

fn effect_line(effect: Effect, app: &str) -> (String, Token) {
    match effect {
        Effect::Read => (String::from("Only reads."), Token::TextDim),
        Effect::Change => (format!("Changes something in {app}."), Token::TextDim),
        Effect::Destroy => (String::from("Deletes or overwrites something."), Token::Text),
    }
}

fn effect_word(effect: Effect) -> &'static str {
    match effect {
        Effect::Read => "Reads",
        Effect::Change => "Changes",
        Effect::Destroy => "Deletes",
    }
}

fn read_file(files: &mut LinuxFiles, app: &[u8]) -> &'static [u8] {
    let mut path = Vec::with_capacity(APPLICATIONS.len() + app.len() + 16);
    path.extend_from_slice(APPLICATIONS);
    path.push(b'/');
    path.extend_from_slice(app);
    path.push(b'/');
    path.extend_from_slice(actions::FILE_NAME.as_bytes());
    let Ok(handle) = files.open(&path) else {
        return &[];
    };
    let mut bytes = alloc::vec![0u8; actions::LARGEST + 1];
    let mut filled = 0;
    while filled < bytes.len() {
        match files.read(handle, &mut bytes[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(count) => filled += count,
        }
    }
    files.close(handle);
    bytes.truncate(filled);
    Box::leak(bytes.into_boxed_slice())
}

fn list_cell(_data: u32, row: usize, column: usize, out: &mut Text<64>) {
    // SAFETY: the program has one thread; the cells are written before the table is built and
    // read only while it paints.
    let cells = unsafe { &*(&raw const LIST_CELLS) };
    let Some((length, bytes)) = cells.get(row).and_then(|row| row.get(column)) else {
        return;
    };
    *out = Text::from(core::str::from_utf8(&bytes[..*length]).unwrap_or("?"));
}

fn keep_cell(row: usize, column: usize, words: &str) {
    // SAFETY: the program has one thread, and no table paints while the cells are written.
    let cells = unsafe { &mut *(&raw mut LIST_CELLS) };
    let Some(cell) = cells.get_mut(row).and_then(|row| row.get_mut(column)) else {
        return;
    };
    let shown = shortened(words, 63);
    let bytes = shown.as_bytes();
    let length = bytes.len().min(64);
    cell.1[..length].copy_from_slice(&bytes[..length]);
    cell.0 = length;
}

fn pipe() -> Result<(usize, usize), Errno> {
    let mut ends = [0i32; 2];
    // SAFETY: `pipe2` writes two descriptors into `ends`, which this function owns.
    let made = unsafe { syscall::call(number::PIPE2, [ends.as_mut_ptr() as usize, O_CLOEXEC, 0, 0, 0, 0]) };
    Errno::result(made)?;
    Ok((ends[0] as usize, ends[1] as usize))
}

fn close_descriptor(descriptor: usize) {
    // SAFETY: closing a descriptor this program opened and no longer uses.
    let _ = unsafe { syscall::call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]) };
}

fn at(x: i32, y: i32, width: i32, height: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Px(height))
}

fn line_at(x: i32, y: i32, width: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Content)
}

impl ActionWindow {
    fn new(app: &'static [u8], action: Option<&[u8]>) -> Self {
        let mut files = LinuxFiles::new();
        let file = read_file(&mut files, app);
        let app_title = lossy(app);
        let icon = AppIcon::of(&app_title, None);
        let mut window = Self {
            tree: WidgetTree::new(),
            ids: Ids::default(),
            screen: Screen::List,
            app,
            app_title,
            icon,
            file,
            chosen: None,
            values: Vec::new(),
            from_list: action.is_none(),
            run: None,
            output: Vec::new(),
        };
        match action {
            Some(name) => match actions::actions(file).find(|each| each.name == name) {
                Some(found) => window.choose(found),
                None => {
                    let sentence = format!("{} no longer has this action.", window.app_title);
                    window.show_result_sentence(&sentence, false);
                }
            },
            None => window.show_list(),
        }
        window
    }

    fn fresh(&mut self, screen: Screen) -> WidgetId {
        self.tree = WidgetTree::new();
        self.tree.resize(WIDTH, SIZE.1 as i32);
        self.ids = Ids::default();
        self.screen = screen;
        self.tree.add(None, Place::fill(), Kind::panel(Token::WindowBackground)).expect("a screen fits its tree")
    }

    fn add(&mut self, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
        self.tree.add(Some(parent), place, kind).expect("a screen fits its tree")
    }

    fn header(&mut self, root: WidgetId, with_effect: bool) {
        self.add(root, at(PADDING, PADDING, ICON, ICON), dialog::icon_kind(self.icon));
        let Some(action) = self.chosen else {
            return;
        };
        let title = shortened(&title_of(&action), 48);
        self.add(root, line_at(TEXT_X, TITLE_TOP, TEXT_WIDTH), Kind::label(&title, Token::Text));
        let description = lossy(action.description);
        let mut lines = [""; 2];
        let count = dialog::wrap(&description, TEXT_WIDTH, &mut lines);
        for (index, (line, top)) in lines.iter().zip(DESCRIPTION_TOPS).take(count).enumerate() {
            let shown = match index == 1 && count == 2 && description.len() > lines[0].len() + line.len() + 1 {
                true => shortened(&format!("{line}..."), 48),
                false => String::from(*line),
            };
            self.add(root, line_at(TEXT_X, top, TEXT_WIDTH), Kind::label(&shown, Token::TextDim));
        }
        if with_effect {
            let (words, ink) = effect_line(action.effect, &self.app_title);
            self.add(root, line_at(TEXT_X, EFFECT_TOP, TEXT_WIDTH), Kind::label(&words, ink));
        }
    }

    fn buttons(&mut self, root: WidgetId, second: &str, main: &str) {
        let status = self.add(
            root,
            line_at(PADDING, BUTTONS_TOP + STATUS_DROP, STATUS_WIDTH),
            Kind::label("", Token::TextDim),
        );
        let second = self.add(
            root,
            at(SECOND_X, BUTTONS_TOP, BUTTON_WIDTH, CONTROL_HEIGHT),
            Kind::button(second, false),
        );
        let main = self.add(
            root,
            at(MAIN_X, BUTTONS_TOP, BUTTON_WIDTH, CONTROL_HEIGHT),
            Kind::button(main, true),
        );
        self.tree.set_default_button(main);
        self.ids.status = Some(status);
        self.ids.second = Some(second);
        self.ids.main = Some(main);
    }

    fn show_list(&mut self) {
        let root = self.fresh(Screen::List);
        self.add(root, at(PADDING, PADDING, ICON, ICON), dialog::icon_kind(self.icon));
        let name = self.app_title.clone();
        self.add(root, line_at(TEXT_X, TITLE_TOP, TEXT_WIDTH), Kind::label(&name, Token::Text));
        let offered: Vec<Action<'static>> = actions::actions(self.file).collect();
        if offered.is_empty() {
            let line = format!("{name} has no actions.");
            self.add(root, line_at(TEXT_X, DESCRIPTION_TOPS[0], TEXT_WIDTH), Kind::label(&line, Token::TextDim));
            self.buttons(root, "Close", "Run");
            if let Some(main) = self.ids.main {
                self.tree.set_shown(main, false);
            }
            return;
        }
        let count = match offered.len() {
            1 => String::from("1 action"),
            many => format!("{many} actions"),
        };
        self.add(root, line_at(TEXT_X, DESCRIPTION_TOPS[0], TEXT_WIDTH), Kind::label(&count, Token::TextDim));
        for (row, action) in offered.iter().enumerate() {
            keep_cell(row, 0, &title_of(action));
            keep_cell(row, 1, &lossy(action.description));
            keep_cell(row, 2, effect_word(action.effect));
        }
        let column = |title: &'static str, width: i32| Column {
            title,
            width,
            min_width: width,
            align: Align::Left,
            sortable: false,
        };
        let columns = [column("Action", 160), column("What it does", 208), column("Effect", 72)];
        let table = self.add(
            root,
            at(TABLE.0, TABLE.1, TABLE.2, TABLE.3),
            Kind::table("Actions", &columns, offered.len(), list_cell, 0),
        );
        self.ids.table = Some(table);
        self.buttons(root, "Close", "Run");
        self.tree.set_selected(table, 0);
        self.tree.set_focus(Some(table), 0);
        self.follow_selection();
    }

    fn selected_action(&self) -> Option<Action<'static>> {
        let row = self.ids.table.and_then(|table| self.tree.selected(table))?;
        actions::actions(self.file).nth(row)
    }

    fn asks_first(action: &Action<'_>) -> bool {
        action.arguments().next().is_some() || action.effect == Effect::Destroy
    }

    fn follow_selection(&mut self) {
        let Some(main) = self.ids.main else {
            return;
        };
        let words = match self.selected_action() {
            Some(action) if Self::asks_first(&action) => "Run...",
            _ => "Run",
        };
        self.tree.set_text(main, words);
    }

    fn choose(&mut self, action: Action<'static>) {
        self.chosen = Some(action);
        self.values = action.arguments().map(|_| String::new()).collect();
        match (action.arguments().next().is_some(), action.effect) {
            (true, _) => self.show_form(),
            (false, Effect::Destroy) => self.show_confirm(false),
            (false, _) => self.start_run(false),
        }
    }

    fn show_form(&mut self) {
        let Some(action) = self.chosen else {
            return;
        };
        let root = self.fresh(Screen::Form);
        self.header(root, true);
        let arguments: Vec<_> = action.arguments().copied().collect();
        for (index, argument) in arguments.iter().enumerate() {
            let top = BODY_TOP + FIELD_PITCH * index as i32;
            let described = match argument.description.is_empty() {
                true => lossy(argument.name),
                false => shortened(&lossy(argument.description), 42),
            };
            self.add(root, line_at(PADDING, top, DESCRIPTION_WIDTH), Kind::label(&described, Token::Text));
            let word = self.add(
                root,
                Place::right(WIDTH - WORD_END, top),
                Kind::label(Self::requirement(argument.required), Token::TextDim),
            );
            let control_top = top + CONTROL_GAP;
            let control = match argument.kind {
                ArgumentType::Text => self.add(
                    root,
                    at(PADDING, control_top, TEXT_FIELD_WIDTH, CONTROL_HEIGHT),
                    Kind::text_field("", Escape::Clear),
                ),
                ArgumentType::Number => self.add(
                    root,
                    at(PADDING, control_top, SHORT_CONTROL_WIDTH, CONTROL_HEIGHT),
                    Kind::text_field("", Escape::Clear),
                ),
                ArgumentType::Boolean => {
                    let options: &'static [&'static str] = match argument.required {
                        true => &REQUIRED_BOOLEAN,
                        false => &OPTIONAL_BOOLEAN,
                    };
                    let chosen = match (argument.required, self.values[index].as_str()) {
                        (true, "true") => 1,
                        (false, "false") => 1,
                        (false, "true") => 2,
                        _ => 0,
                    };
                    self.add(
                        root,
                        at(PADDING, control_top, SHORT_CONTROL_WIDTH, CONTROL_HEIGHT),
                        Kind::dropdown("Value", options, chosen),
                    )
                }
            };
            if argument.kind != ArgumentType::Boolean {
                let value = self.values[index].clone();
                self.tree.set_text(control, &value);
            }
            let mut name = described.clone();
            name.push_str(match argument.required {
                true => ", required",
                false => ", optional",
            });
            self.tree.set_accessible_name(control, &name);
            self.ids.fields.push(FieldIds { word, control });
        }
        let main = match action.effect {
            Effect::Destroy => "Continue",
            _ => "Run",
        };
        self.buttons(root, "Cancel", main);
        if let Some(first) = self.ids.fields.first() {
            self.tree.set_focus(Some(first.control), 0);
        }
    }

    fn requirement(required: bool) -> &'static str {
        match required {
            true => "Required",
            false => "Optional",
        }
    }

    fn read_form(&mut self) -> bool {
        let Some(action) = self.chosen else {
            return false;
        };
        let arguments: Vec<_> = action.arguments().copied().collect();
        let mut first_problem = None;
        for (index, (argument, field)) in arguments.iter().zip(self.ids.fields.clone()).enumerate() {
            let value = match argument.kind {
                ArgumentType::Boolean => {
                    let chosen = self.tree.selected(field.control).unwrap_or(0);
                    let value = match (argument.required, chosen) {
                        (true, 1) | (false, 2) => "true",
                        (true, _) | (false, 1) => "false",
                        _ => "",
                    };
                    String::from(value)
                }
                _ => String::from(self.tree.text_of(field.control).unwrap_or("")),
            };
            let problem = match () {
                _ if value.is_empty() && argument.required => Some("Needs a value"),
                _ if !value.is_empty() && !argument.kind.accepts(value.as_bytes()) => Some(match argument.kind {
                    ArgumentType::Number => "Not a number",
                    _ => "Not a valid value",
                }),
                _ => None,
            };
            match problem {
                Some(words) => {
                    self.tree.set_text(field.word, words);
                    first_problem.get_or_insert(field.control);
                }
                None => self.tree.set_text(field.word, Self::requirement(argument.required)),
            }
            self.values[index] = value;
        }
        if let Some(control) = first_problem {
            self.tree.set_focus(Some(control), 0);
            return false;
        }
        true
    }

    fn show_confirm(&mut self, from_form: bool) {
        let Some(action) = self.chosen else {
            return;
        };
        let root = self.fresh(Screen::Confirm { from_form });
        self.header(root, true);
        let arguments: Vec<_> = action.arguments().copied().collect();
        let mut line = 0;
        for (argument, value) in arguments.iter().zip(self.values.clone()) {
            if value.is_empty() {
                continue;
            }
            let shown = match (argument.kind, value.as_str()) {
                (ArgumentType::Boolean, "true") => String::from("Yes"),
                (ArgumentType::Boolean, _) => String::from("No"),
                _ => value.clone(),
            };
            let words = shortened(&format!("{}: {shown}", lossy(argument.description)), 48);
            self.add(
                root,
                line_at(TEXT_X, BODY_TOP + VALUE_PITCH * line, TEXT_WIDTH),
                Kind::label(&words, Token::TextDim),
            );
            line += 1;
        }
        let title = shortened(&title_of(&action), ACTION_BUTTON_GLYPHS);
        let width = (title.chars().count() as i32 * GLYPH + ACTION_BUTTON_PADDING).max(BUTTON_WIDTH);
        let action_x = WORD_END - width;
        let status = self.add(
            root,
            line_at(PADDING, BUTTONS_TOP + STATUS_DROP, STATUS_WIDTH),
            Kind::label("", Token::TextDim),
        );
        let cancel = self.add(
            root,
            at(action_x - BUTTON_GAP - BUTTON_WIDTH, BUTTONS_TOP, BUTTON_WIDTH, CONTROL_HEIGHT),
            Kind::button("Cancel", false),
        );
        let main = self.add(root, at(action_x, BUTTONS_TOP, width, CONTROL_HEIGHT), Kind::button(&title, false));
        self.tree.set_person_only(main, true);
        self.tree.set_accessible_name(main, &format!("{title}, deletes or overwrites"));
        if from_form {
            let back = self.add(root, at(PADDING, BUTTONS_TOP, BUTTON_WIDTH, CONTROL_HEIGHT), Kind::button("Back", false));
            self.ids.back = Some(back);
        }
        self.ids.status = Some(status);
        self.ids.second = Some(cancel);
        self.ids.main = Some(main);
        self.tree.set_focus(Some(cancel), 0);
    }

    fn start_run(&mut self, confirmed: bool) {
        let Some(action) = self.chosen else {
            return;
        };
        self.output.clear();
        let mut words: Vec<Vec<u8>> = Vec::new();
        let mut word = |bytes: &[u8]| {
            let mut owned = bytes.to_vec();
            owned.push(0);
            words.push(owned);
        };
        word(b"app");
        word(b"do");
        if confirmed {
            word(b"--confirmed");
        }
        word(b"--");
        word(self.app);
        word(action.name);
        for (argument, value) in action.arguments().zip(&self.values) {
            if value.is_empty() {
                continue;
            }
            let mut pair = argument.name.to_vec();
            pair.push(b'=');
            pair.extend_from_slice(value.as_bytes());
            word(&pair);
        }
        let mut pointers: Vec<*const u8> = words.iter().map(|word| word.as_ptr()).collect();
        pointers.push(core::ptr::null());
        let environment = [b"PATH=/bin\0".as_ptr(), b"NO_COLOR=1\0".as_ptr(), core::ptr::null()];
        let started = pipe().and_then(|(read_end, write_end)| {
            let request = Spawn::new(APP_PROGRAM, &pointers, &environment, [write_end, write_end, write_end]);
            // SAFETY: the program has one thread and starts one child at a time on this stack,
            // which `spawn` uses only until the child's `execve`.
            let stack = unsafe { &mut *(&raw mut STACK) };
            let spawned = spawn(stack, &request);
            close_descriptor(write_end);
            match spawned {
                Ok(child) => {
                    // SAFETY: setting the read end's flags, a descriptor this program owns.
                    let _ = unsafe {
                        syscall::call(number::FCNTL, [read_end, 4, O_NONBLOCK | O_CLOEXEC, 0, 0, 0])
                    };
                    Ok(Run { child, pipe: read_end, stopped: false })
                }
                Err(error) => {
                    close_descriptor(read_end);
                    Err(error)
                }
            }
        });
        match started {
            Ok(run) => {
                self.run = Some(run);
                let root = self.fresh(Screen::Running);
                self.header(root, true);
                self.buttons(root, "Stop", "Run");
                if let Some(main) = self.ids.main {
                    self.tree.set_enabled(main, false);
                }
                if let Some(status) = self.ids.status {
                    self.tree.set_text(status, "Running...");
                }
                if let Some(stop) = self.ids.second {
                    self.tree.set_focus(Some(stop), 0);
                }
            }
            Err(error) => {
                let sentence = format!("Could not start: {}.", lossy(error.message()));
                self.show_result_sentence(&sentence, true);
            }
        }
    }

    fn stop(&mut self) {
        if let Some(run) = &mut self.run {
            run.stopped = true;
            // SAFETY: asking this program's own child to end.
            let _ = unsafe { syscall::call(number::KILL, [run.child, SIGTERM, 0, 0, 0, 0]) };
        }
    }

    fn drain(&mut self) {
        let Some(run) = self.run else {
            return;
        };
        let mut chunk = [0u8; READ_CHUNK];
        loop {
            // SAFETY: `read` writes at most `chunk.len()` bytes into `chunk`, which this
            // function owns.
            let read = unsafe { syscall::call(number::READ, [run.pipe, chunk.as_mut_ptr() as usize, chunk.len(), 0, 0, 0]) };
            match Errno::result(read) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    let room = OUTPUT_LIMIT.saturating_sub(self.output.len());
                    self.output.extend_from_slice(&chunk[..count.min(room)]);
                }
            }
        }
    }

    fn poll(&mut self) {
        let Some(run) = self.run else {
            return;
        };
        self.drain();
        let mut status = 0i32;
        // SAFETY: `wait4` writes the child's status into `status`, which this function owns.
        let waited = unsafe { syscall::call(number::WAIT4, [run.child, &raw mut status as usize, WNOHANG, 0, 0, 0]) };
        match Errno::result(waited) {
            Ok(0) | Err(Errno::EINTR) => return,
            _ => {}
        }
        self.drain();
        close_descriptor(run.pipe);
        self.run = None;
        let exited = status & 0x7f == 0;
        let code = (status >> 8) & 0xff;
        let sentence = match (run.stopped, exited, code, self.output.is_empty()) {
            (true, _, _, _) => String::from("Stopped."),
            (false, true, 0, false) => String::from("Done."),
            (false, true, 0, true) => String::from("Done. The action printed nothing."),
            (false, true, code, _) => format!("Failed with status {code}."),
            (false, false, _, _) => format!("Failed with status {}.", 128 + (status & 0x7f)),
        };
        let again = !self.output.windows(14).any(|part| part == b"no_such_action");
        self.show_result_sentence(&sentence, again);
    }

    fn show_result_sentence(&mut self, sentence: &str, again: bool) {
        let root = self.fresh(Screen::Result);
        self.add(root, at(PADDING, PADDING, ICON, ICON), dialog::icon_kind(self.icon));
        let title = self.chosen.map_or_else(|| self.app_title.clone(), |action| shortened(&title_of(&action), 48));
        self.add(root, line_at(TEXT_X, TITLE_TOP, TEXT_WIDTH), Kind::label(&title, Token::Text));
        self.add(root, line_at(TEXT_X, DESCRIPTION_TOPS[0], TEXT_WIDTH), Kind::label(sentence, Token::Text));
        if !self.output.is_empty() {
            let area = self.add(root, at(AREA.0, AREA.1, AREA.2, AREA.3), Kind::text_area(0, ""));
            let mut shown = lossy(&self.output);
            if self.output.len() >= OUTPUT_LIMIT {
                shown.push('\n');
                shown.push_str(&format!("{OUTPUT_CUT} {} ...", self.app_title));
            }
            self.tree.set_text(area, &shown);
            self.tree.set_accessible_name(area, &format!("Output of {title}"));
            self.ids.area = Some(area);
        }
        self.buttons(root, "Run again", "Close");
        if let Some(second) = self.ids.second {
            self.tree.set_shown(second, again && self.chosen.is_some());
        }
        if let Some(main) = self.ids.main {
            self.tree.set_focus(Some(main), 0);
        }
    }

    fn run_again(&mut self) {
        let Some(action) = self.chosen else {
            return;
        };
        match (action.arguments().next().is_some(), action.effect) {
            (true, _) => self.show_form(),
            (false, Effect::Destroy) => self.show_confirm(false),
            (false, _) => self.start_run(false),
        }
    }

    fn cancel(&mut self) {
        match self.from_list {
            true => self.show_list(),
            false => close(),
        }
    }

    fn press_main(&mut self) {
        match self.screen {
            Screen::List => {
                if let Some(action) = self.selected_action() {
                    self.choose(action);
                }
            }
            Screen::Form if self.read_form() => match self.chosen.map(|action| action.effect) {
                Some(Effect::Destroy) => self.show_confirm(true),
                _ => self.start_run(false),
            },
            Screen::Confirm { .. } => self.start_run(true),
            Screen::Result => close(),
            Screen::Form | Screen::Running => {}
        }
    }

    fn press_second(&mut self) {
        match self.screen {
            Screen::List => close(),
            Screen::Form | Screen::Confirm { .. } => self.cancel(),
            Screen::Running => self.stop(),
            Screen::Result => self.run_again(),
        }
    }
}

impl Application<CAPACITY, 1> for ActionWindow {
    fn tree(&mut self) -> &mut WidgetTree<CAPACITY, 1> {
        &mut self.tree
    }

    fn respond(&mut self, response: Response, _now_ms: u32) {
        let ids = self.ids.clone();
        match response {
            Response::Activated(id) if Some(id) == ids.main => self.press_main(),
            Response::Activated(id) if Some(id) == ids.second => self.press_second(),
            Response::Activated(id) if Some(id) == ids.back => self.show_form(),
            Response::Chosen(id, _) if Some(id) == ids.table => self.follow_selection(),
            Response::Opened(id, row) if Some(id) == ids.table => {
                if let Some(action) = actions::actions(self.file).nth(row) {
                    self.choose(action);
                }
            }
            Response::Edited(id) if Some(id) == ids.area => {
                let shown = lossy(&self.output);
                self.tree.set_text(id, &shown);
            }
            Response::Edited(id) => {
                let Some(action) = self.chosen else {
                    return;
                };
                if let Some((index, field)) = ids.fields.iter().enumerate().find(|(_, field)| field.control == id)
                    && let Some(argument) = action.arguments().nth(index)
                {
                    self.tree.set_text(field.word, Self::requirement(argument.required));
                }
            }
            Response::Unused { code: key::ESC, .. } => match self.screen {
                Screen::Running => self.stop(),
                Screen::Result | Screen::List => close(),
                _ => self.cancel(),
            },
            Response::Unused { code: key::ENTER, .. } if self.screen == Screen::Form => self.press_main(),
            _ => {}
        }
    }

    fn deadline(&self, now_ms: u32) -> Option<u32> {
        self.run.map(|_| now_ms.wrapping_add(POLL_MS))
    }

    fn tick(&mut self, _now_ms: u32) {
        self.poll();
    }
}

/// Runs the window for `app`'s actions, or for its `action`; returns the program's status.
pub fn run(app: &'static [u8], action: Option<&[u8]>) -> i32 {
    let title = lossy(app);
    let mut window = match Window::create(&title, SIZE.0, SIZE.1, SIZE) {
        Ok(window) => window,
        Err(_) => return 1,
    };
    let mut actions_window = ActionWindow::new(app, action);
    let ended = application::run(&mut window, &mut actions_window);
    actions_window.stop();
    match ended {
        Ok(()) => 0,
        Err(_) => 1,
    }
}
