//! The screen reader's kernel part (D603, docs/ux/accessibility-v0.4.md section 1): its
//! settings reach the window server, apps' focused elements are worded and said
//! (`REPORT_ELEMENT`), and the reading keys run on a thread of their own, which asks the focused
//! window for its tree as `GET_TREE` does and says what the reading cursor reaches. The cursor
//! is an element id, a reading position, never the focus.

use crate::sync::LeafSpinLock;
use crate::terminal::with_windows;
use crate::thread::{self, WaitQueue};
use crate::window::{self, QueryRequest};
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};
use eggos_drivers::desktop::protocol::event as record_kind;
use eggos_drivers::desktop::protocol::{ELEMENT_TEXT_CAPACITY, EventRecord};
use eggos_drivers::desktop::{ReaderCommand, ReaderSettings};
use eggos_drivers::display::Rect;
use eggos_element_tree::reader::{self, Position, Priority, Source};
use eggos_element_tree::{Action, Element, Outcome, Role, elements, query_kind, tree_answer};
use eggos_hal::Platform;

const COMMANDS: usize = 8;
const ANSWER_WAIT_MS: u64 = 500;
const ANSWER_POLL_MS: u64 = 10;
const PIXELS_ONLY: &str = "pixels only, no elements";
const END_OF_WINDOW: &str = "end of window";
const START_OF_WINDOW: &str = "start of window";
const CANNOT: &str = "cannot";

/// How an app's report changed its focused element (`REPORT_ELEMENT`'s kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reported {
    /// The focus moved to it.
    Focus,
    /// Its value or state changed.
    Change,
    /// This character was typed into it.
    Echo(char),
}

struct Commands {
    queued: [Option<ReaderCommand>; COMMANDS],
    cursor: (u32, u32),
}

static COMMANDS_WAITING: LeafSpinLock<Commands> = LeafSpinLock::new_leaf(Commands {
    queued: [None; COMMANDS],
    cursor: (0, 0),
});
static WAKE: WaitQueue = WaitQueue::new();
static STARTED: AtomicBool = AtomicBool::new(false);

/// Takes the reader's settings (`SET_ACCESSIBILITY`, the boot read): the window server says
/// it turned on or off, and apps learn whether to report their focus.
pub fn set_settings<P: Platform>(settings: ReaderSettings) {
    with_windows::<P, _>(|desktop, _, damage, events| {
        desktop.set_screen_reader(settings, damage);
        desktop.send_report_wish(events);
    });
}

fn line_of(element: &Element<'_>, parent: Option<Role>, position: Option<Position>) -> Line {
    let mut line = Line::new();
    let _ = reader::utterance(element, parent, position, &mut line);
    line
}

/// An app reported its focused element of window `window`: `bytes` holds it, then its
/// container, in an element tree's layout; `position` is its place among its siblings and
/// `caret` the caret's rect in content pixels. The reader says it, and zoom brings it, or the
/// caret, into view.
pub fn report_element<P: Platform>(
    window: u32,
    bytes: &[u8],
    position: Option<Position>,
    caret: Option<Rect>,
    reported: Reported,
) {
    let mut parsed = elements(bytes).flatten();
    let Some(element) = parsed.next() else {
        return;
    };
    let container = parsed.next();
    let line = line_of(&element, container.map(|container| container.role), position);
    let container_line = container.map(|container| {
        let mut text = Line::new();
        let _ = reader::container_line(
            &container,
            position.map_or(0, |position| position.count),
            &mut text,
        );
        (container.id, text)
    });
    if reported == Reported::Focus {
        COMMANDS_WAITING.lock().cursor = (window, element.id);
    }
    let Ok(id) = u16::try_from(window) else {
        return;
    };
    with_windows::<P, _>(|desktop, _, damage, _| {
        if desktop.focused_window() != Some(id) {
            return;
        }
        if let Some(content) = desktop.content_rect(id) {
            let on_screen = |x: i32, y: i32, width: u32, height: u32| {
                Rect::new(
                    content.x.saturating_add_signed(x),
                    content.y.saturating_add_signed(y),
                    width.max(1),
                    height.max(1),
                )
            };
            let shown = match caret {
                Some(caret) => on_screen(caret.x as i32, caret.y as i32, caret.width, caret.height),
                None => on_screen(
                    element.bounds.x,
                    element.bounds.y,
                    element.bounds.width,
                    element.bounds.height,
                ),
            };
            desktop.bring_into_zoom_view(shown, damage);
            desktop.keep_caption_clear_of(shown, damage);
        }
        match reported {
            Reported::Echo(character) => desktop.reader_echo(character, damage),
            Reported::Focus | Reported::Change => desktop.reader_element(
                line.as_str(),
                container_line
                    .as_ref()
                    .map(|(id, text)| (*id, text.as_str())),
                reported == Reported::Focus,
                damage,
            ),
        }
    });
}

/// Runs a reading key on the focused window: queued for the reader's thread, which starts the
/// first time.
pub fn command<P: Platform>(command: ReaderCommand) {
    {
        let mut waiting = COMMANDS_WAITING.lock();
        if let Some(slot) = waiting.queued.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(command);
        }
    }
    if !STARTED.swap(true, Ordering::AcqRel) && thread::spawn::<P, _>(run::<P>).is_err() {
        STARTED.store(false, Ordering::Release);
        return;
    }
    WAKE.wake_all::<P>();
}

fn next_command() -> Option<ReaderCommand> {
    let mut waiting = COMMANDS_WAITING.lock();
    let first = waiting.queued[0].take();
    waiting.queued.rotate_left(1);
    first
}

fn run<P: Platform>() {
    loop {
        WAKE.wait_until::<P>(|| COMMANDS_WAITING.lock().queued[0].is_some());
        while let Some(command) = next_command() {
            perform::<P>(command);
        }
    }
}

fn say<P: Platform>(source: Source, priority: Priority, text: &str) {
    with_windows::<P, _>(|desktop, _, damage, _| desktop.say(source, priority, text, damage));
}

fn ask<P: Platform>(
    window: u32,
    kind: u32,
    element: u32,
    action: u32,
) -> Option<window::QueryAnswer> {
    let request = QueryRequest {
        window,
        kind,
        element,
        action,
        text_length: 0,
        text: [0; ELEMENT_TEXT_CAPACITY],
    };
    let serial = with_windows::<P, _>(|desktop, _, damage, events| {
        let owner = desktop.owner_of(window).ok()?;
        let serial = window::start_query(owner, request)?;
        let record = EventRecord::new(
            record_kind::QUERY,
            0,
            serial as i32,
            kind as i32,
            element,
            action,
        );
        if desktop.ask_owner(window, record, damage, events).is_err() {
            window::finish_query(serial);
            return None;
        }
        Some(serial)
    })??;
    let polls = ANSWER_WAIT_MS / ANSWER_POLL_MS;
    for _ in 0..polls {
        if window::query_answered(serial) {
            break;
        }
        thread::sleep::<P>(core::time::Duration::from_millis(ANSWER_POLL_MS));
    }
    window::finish_query(serial)
}

fn perform<P: Platform>(command: ReaderCommand) {
    let Some(window) = with_windows::<P, _>(|desktop, _, _, _| desktop.focused_window())
        .flatten()
        .map(u32::from)
    else {
        say::<P>(Source::Read, Priority::Interrupt, "Desktop");
        return;
    };
    if command == ReaderCommand::Activate {
        activate::<P>(window);
        return;
    }
    let tree = ask::<P>(window, query_kind::TREE, 0, 0)
        .filter(|answer| answer.outcome == tree_answer::TREE)
        .map(|answer| answer.tree)
        .unwrap_or_default();
    if tree.is_empty() {
        say::<P>(Source::Read, Priority::Interrupt, PIXELS_ONLY);
        return;
    }
    let all: Vec<Element<'_>> = elements(&tree).flatten().collect();
    let read: Vec<usize> = (0..all.len())
        .filter(|&index| reader::is_read(&all[index]))
        .collect();
    let line_at = |index: usize| {
        let (position, parent) = reader::place_of(elements(&tree), index);
        line_of(&all[index], parent, position)
    };
    let title = all
        .iter()
        .find(|element| element.role == Role::Window)
        .map(|root| line_of(root, None, None));
    let (cursor_window, cursor_element) = COMMANDS_WAITING.lock().cursor;
    let at = (cursor_window == window)
        .then(|| read.iter().position(|&index| all[index].id == cursor_element))
        .flatten();
    let moved = match command {
        ReaderCommand::Next => Some(match at {
            Some(at) if at + 1 < read.len() => (at + 1, None),
            Some(_) => (0, Some(END_OF_WINDOW)),
            None => (0, None),
        }),
        ReaderCommand::Previous => Some(match at {
            Some(at) if at > 0 => (at - 1, None),
            _ => (read.len().saturating_sub(1), Some(START_OF_WINDOW)),
        }),
        ReaderCommand::First => Some((0, None)),
        ReaderCommand::Last => Some((read.len().saturating_sub(1), None)),
        ReaderCommand::Current => Some((at.unwrap_or(0), None)),
        _ => None,
    };
    if let Some((to, edge)) = moved {
        let Some(&index) = read.get(to) else {
            return;
        };
        if let Some(edge) = edge {
            say::<P>(Source::Read, Priority::Interrupt, edge);
        }
        COMMANDS_WAITING.lock().cursor = (window, all[index].id);
        let priority = match edge {
            Some(_) => Priority::Queue,
            None => Priority::Interrupt,
        };
        say::<P>(Source::Read, priority, line_at(index).as_str());
        return;
    }
    let title = title.unwrap_or_else(Line::new);
    say::<P>(Source::Read, Priority::Interrupt, title.as_str());
    let from = match command {
        ReaderCommand::Window => 0,
        ReaderCommand::FromHere => at.unwrap_or(0),
        _ => return,
    };
    for &index in &read[from.min(read.len())..] {
        say::<P>(Source::Read, Priority::Queue, line_at(index).as_str());
    }
}

fn activate<P: Platform>(window: u32) {
    let (cursor_window, element) = COMMANDS_WAITING.lock().cursor;
    if cursor_window != window || element == 0 {
        say::<P>(Source::Read, Priority::Interrupt, CANNOT);
        return;
    }
    let done = ask::<P>(window, query_kind::ACT, element, Action::Invoke.value())
        .is_some_and(|answer| Outcome::from_value(answer.outcome) == Outcome::Done);
    if !done {
        say::<P>(Source::Read, Priority::Interrupt, CANNOT);
    }
}

/// An utterance being built, cut at [`reader::MAX_UTTERANCE_BYTES`].
struct Line {
    bytes: [u8; reader::MAX_UTTERANCE_BYTES],
    length: usize,
}

impl Line {
    const fn new() -> Self {
        Self {
            bytes: [0; reader::MAX_UTTERANCE_BYTES],
            length: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = self.bytes.len() - self.length;
        let mut cut = text.len().min(room);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        self.bytes[self.length..self.length + cut].copy_from_slice(&text.as_bytes()[..cut]);
        self.length += cut;
        Ok(())
    }
}
