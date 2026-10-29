//! The graphical terminal (D114) on the desktop (D115) and its window server (D127): the
//! console's session shown on the display in the Egg Mono font, by the terminal engine of
//! [`eggos_drivers::terminal`], in the terminal window of [`eggos_drivers::desktop`], among the
//! windows programs open through `/dev/window` ([`crate::window`]), over the wallpaper and
//! under the top bar.
//!
//! Once [`start`] made it, every byte [`console::write`] sends to the serial console is fed to
//! the terminal as well, under the screen's lock, so both see the same bytes in the same
//! order. Feeding only updates cells; the drawing thread ([`start_drawing`]) renders what changed
//! into the terminal's own buffer, the grid's pixels, composes the rectangles that changed on
//! screen into the framebuffer (each window's content from its buffer, clipped by the windows
//! above it) and flushes them, at most once every [`FRAME_INTERVAL_MS`] and at once after an
//! idle frame, so a burst of output costs one flush. The same thread hands the desktop the
//! pointer and key events the input layer queued, which may become events for the windows'
//! programs, and composes what they changed; a program's requests compose what they change
//! during the call ([`with_windows`]). The display's flush, which waits for the device, never
//! holds the screen's lock: a stalled device stops the picture, not the console. Without a
//! display nothing is made and a console write costs one load.

use crate::console;
use crate::display;
use crate::input::{self, Button, InputEvent};
use crate::linux::reboot::{self, take_machine_down};
use crate::sync::LeafSpinLock;
use crate::thread::{self, SpawnError, WaitQueue};
use crate::vfs;
use crate::window::{self, Buffers, Clients, Queues};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, Ordering};
use eggos_drivers::desktop::protocol::{DiscardEvents, EventSink};
use eggos_drivers::desktop::{
    Action, AppId, Contents, Desktop, GRID_SIZE, Launch, Profile, TerminalInput, WindowId, colour,
    shortcut,
};
use eggos_drivers::display::{Damage, Rect};
use eggos_drivers::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use eggos_drivers::input::button;
use eggos_drivers::input::keymap::{self, CursorKeys, Modifiers};
use eggos_drivers::terminal::{REPLY_CAPACITY, TERM_BACKGROUND, Terminal, TextArea, WHEEL_LINES};
use eggos_hal::Platform;
use eggos_hal::time::mul_div_ceil;

pub use eggos_drivers::terminal::ViewMove;

/// The shortest time between two flushes of the terminal: 60 frames a second.
pub const FRAME_INTERVAL_MS: u64 = 16;

/// Who the desktop serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// The user: the desktop takes the pointer and key events and the clock shows the uptime.
    Interactive,
    /// A self-test: no input is taken and the clock stays at `up 0:00`, so the picture is the
    /// same on every boot.
    SelfTest,
}

struct Screen {
    terminal: Terminal,
    terminal_pixels: Box<[u32]>,
    desktop: Desktop,
    session: Session,
    buffers: Buffers,
    clients: Clients,
}

struct ScreenContents<'a> {
    terminal: WindowId,
    terminal_pixels: &'a [u32],
    buffers: &'a Buffers,
}

impl Contents for ScreenContents<'_> {
    fn copy_row(&self, window: WindowId, x: u32, y: u32, out: &mut [u32]) {
        if window == self.terminal {
            let start = y as usize * GRID_SIZE.0 as usize + x as usize;
            if let Some(row) = self.terminal_pixels.get(start..start + out.len()) {
                out.copy_from_slice(row);
            }
            return;
        }
        match self.buffers.of(window) {
            Some(buffer) => buffer.copy_row(x, y, out),
            None => out.fill(colour::WINDOW_BACKGROUND),
        }
    }
}

fn compose(
    desktop: &Desktop,
    terminal_pixels: &[u32],
    buffers: &Buffers,
    framebuffer: &mut display::Framebuffer<'_>,
    damage: &Damage,
) {
    let stride = framebuffer.mode().width as usize;
    let contents = ScreenContents {
        terminal: desktop.terminal_window(),
        terminal_pixels,
        buffers,
    };
    for &rect in damage.rects() {
        desktop.draw(framebuffer.pixels(), stride, rect, &contents);
        framebuffer.damage(rect);
    }
}

fn grid_pixels() -> Option<Box<[u32]>> {
    let count = GRID_SIZE.0 as usize * GRID_SIZE.1 as usize;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(count).ok()?;
    pixels.resize(count, TERM_BACKGROUND);
    Some(pixels.into_boxed_slice())
}

static SCREEN: LeafSpinLock<Option<Box<Screen>>> = LeafSpinLock::new_leaf(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static RENDER_PENDING: AtomicBool = AtomicBool::new(false);
static APPLICATION_CURSOR_KEYS: AtomicBool = AtomicBool::new(false);
static DRAWER: AtomicPtr<WaitQueue> = AtomicPtr::new(core::ptr::null_mut());
static KEYS_TO_TERMINAL: AtomicBool = AtomicBool::new(true);
static SHOWN_SECOND: AtomicU64 = AtomicU64::new(0);

fn drawer() -> Option<&'static WaitQueue> {
    let drawer = DRAWER.load(Ordering::Acquire);
    // SAFETY: `DRAWER` is null or a leaked `Box` that `start` stored once, with release ordering
    // after making it, and that nothing frees.
    unsafe { drawer.as_ref() }
}

/// Whether the terminal is shown, so the console's output goes to it too.
#[inline(always)]
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Whether the keys typed on input devices go to the console: they do unless the desktop has
/// taken them, because the terminal's window is unfocused or minimised or a menu is open.
#[inline]
pub fn keys_go_to_console() -> bool {
    !is_active() || KEYS_TO_TERMINAL.load(Ordering::Relaxed)
}

static CHOSEN_PROFILE: AtomicU8 = AtomicU8::new(NO_PROFILE_CHOSEN);

const NO_PROFILE_CHOSEN: u8 = 0;
const KILL_LINE: u8 = 0x15;
const TYPED_LINE_CAPACITY: usize = 260;

/// The desktop's layout profile: the controls' side and the shortcuts (D128): the one the
/// settings file holds, read at boot ([`load_saved_profile`]), or the last one set through
/// `/dev/window` ([`set_desktop_profile`]); macOS habits until one is chosen (D132).
pub fn desktop_profile() -> Profile {
    match CHOSEN_PROFILE.load(Ordering::Relaxed) {
        NO_PROFILE_CHOSEN => Profile::default(),
        chosen => Profile::from_number(u32::from(chosen - 1)).unwrap_or_default(),
    }
}

fn remember_profile(profile: Profile) {
    CHOSEN_PROFILE.store(profile.number() as u8 + 1, Ordering::Relaxed);
}

/// Makes `profile` the desktop's layout profile, at once when the desktop is shown: the shell
/// is laid out again, every window's controls move to the profile's side and its shortcuts
/// apply ([`Desktop::set_profile`]).
pub fn set_desktop_profile<P: Platform>(profile: Profile) {
    let shown = with_windows::<P, _>(|desktop, _, damage, events| {
        remember_profile(profile);
        desktop.set_profile(profile, damage, events);
    });
    if shown.is_none() {
        remember_profile(profile);
    }
}

/// Makes the profile the settings file holds ([`eggos_settings_file::PATH`]) the desktop's,
/// before the desktop starts, so its first frame is already in it; the default stays when
/// there is no file, it holds no known profile, or it cannot be read.
pub fn load_saved_profile() {
    let root = vfs::root();
    let Ok(node) = vfs::resolve(root, root, eggos_settings_file::PATH.as_bytes()) else {
        return;
    };
    if node.kind() != vfs::NodeKind::Regular {
        return;
    }
    let mut file = alloc::vec![0u8; eggos_settings_file::LARGEST + 1];
    let mut length = 0;
    while length < file.len() {
        match node
            .file_system()
            .read_into(node.id(), length as u64, &mut file[length..])
        {
            Ok(0) => break,
            Ok(read) => length += read,
            Err(_) => return,
        }
    }
    if let Some(profile) = eggos_settings_file::profile(&file[..length])
        .and_then(|number| Profile::from_number(number as u32))
    {
        remember_profile(profile);
    }
}

/// Whether key `code` pressed with `modifiers` is a shortcut the desktop takes from the
/// terminal, which takes every Ctrl key: its bytes do not go to the console.
pub fn takes_from_terminal(code: u16, modifiers: Modifiers) -> bool {
    is_active() && shortcut(code, modifiers, desktop_profile(), true).is_some()
}

/// Draws the desktop with the terminal's window, blank, over the whole display, and from then
/// on mirrors the console's output on it; returns the window's grid. The display shows it at
/// the next [`show`], the drawing thread's. `None`, and nothing made, without a display, when
/// the window does not fit on it, or without memory for it.
pub fn start<P: Platform>(session: Session) -> Option<TextArea> {
    let mode = display::mode()?;
    let mut desktop = Desktop::new(mode.width, mode.height)?;
    desktop.set_profile(desktop_profile(), &mut Damage::new(), &mut DiscardEvents);
    if session == Session::Interactive {
        desktop.set_catalogue(
            crate::apps::catalogue(),
            &mut Damage::new(),
            &mut DiscardEvents,
        );
    }
    let area = desktop.text_area();
    let mut terminal = Terminal::new(area.columns, area.rows)?;
    terminal.set_onlcr(true);
    terminal.assume_drawn_blank();
    let screen = Box::new(Screen {
        terminal,
        terminal_pixels: grid_pixels()?,
        desktop,
        session,
        buffers: Buffers::new(),
        clients: Clients::new(),
    });
    if drawer().is_none() {
        DRAWER.store(Box::into_raw(Box::new(WaitQueue::new())), Ordering::Release);
    }
    display::draw(|framebuffer| {
        let mut whole = Damage::new();
        whole.add(mode.rect());
        compose(
            &screen.desktop,
            &screen.terminal_pixels,
            &screen.buffers,
            framebuffer,
            &whole,
        );
    })?;
    KEYS_TO_TERMINAL.store(true, Ordering::Relaxed);
    SHOWN_SECOND.store(0, Ordering::Relaxed);
    if session == Session::Interactive {
        input::subscribe();
    }
    *SCREEN.lock() = Some(screen);
    crate::linux::terminal::set_window_size(
        area.rows,
        area.columns,
        GLYPH_WIDTH as u16,
        GLYPH_HEIGHT as u16,
    );
    ACTIVE.store(true, Ordering::Release);
    request_render::<P>();
    Some(area)
}

/// Runs `change` on the window server's desktop and buffers with the damage it records and
/// the sink for its clients' events, under the display's and the screen's locks; then composes
/// the damage into the framebuffer, has it flushed and wakes the programs that got events.
/// `None` without a screen.
pub fn with_windows<P: Platform, R>(
    change: impl FnOnce(&mut Desktop, &mut Buffers, &mut Damage, &mut dyn EventSink) -> R,
) -> Option<R> {
    let time_ms = window::time_ms::<P>();
    let (result, sent, damaged) = display::draw(|framebuffer| {
        let mut screen = SCREEN.lock();
        let Screen {
            desktop,
            terminal_pixels,
            buffers,
            clients,
            ..
        } = &mut **screen.as_mut()?;
        let mut queues = Queues::new(clients, time_ms);
        let mut damage = Damage::new();
        desktop.set_time(time_ms);
        let result = change(desktop, buffers, &mut damage, &mut queues);
        compose(desktop, terminal_pixels, buffers, framebuffer, &damage);
        KEYS_TO_TERMINAL.store(desktop.keys_go_to_terminal(), Ordering::Relaxed);
        Some((result, queues.sent(), !damage.is_empty()))
    })??;
    if sent {
        console::wake_input_waiters::<P>();
    }
    if damaged {
        request_render::<P>();
    }
    Some(result)
}

/// Runs `change` on the window server's clients; `None` without a screen.
pub fn with_clients<R>(change: impl FnOnce(&mut Clients) -> R) -> Option<R> {
    SCREEN
        .lock()
        .as_mut()
        .map(|screen| change(&mut screen.clients))
}

/// Stops mirroring the console and frees the terminal; the display keeps its last picture.
pub fn stop() {
    ACTIVE.store(false, Ordering::Release);
    let screen = SCREEN.lock().take();
    drop(screen);
}

/// Writes `bytes` to the platform's console and feeds them to the terminal, in one step under
/// the terminal's lock, then asks the drawing thread to show them. Called by
/// [`console::write`] while the terminal [`is_active`].
#[inline(never)]
pub fn write_mirrored<P: Platform>(bytes: &[u8]) {
    let mut screen = SCREEN.lock();
    P::console_write(bytes);
    if let Some(screen) = screen.as_mut() {
        screen.terminal.feed(bytes);
        APPLICATION_CURSOR_KEYS.store(screen.terminal.application_cursor_keys(), Ordering::Relaxed);
    }
    drop(screen);
    request_render::<P>();
}

/// Whether a program asked for the application form of the arrow keys (`CSI ? 1 h`).
pub fn application_cursor_keys() -> bool {
    APPLICATION_CURSOR_KEYS.load(Ordering::Relaxed)
}

/// Moves the scrollback view and has it shown; nothing without a terminal.
pub fn move_view<P: Platform>(change: ViewMove) {
    if !is_active() {
        return;
    }
    let moved = SCREEN.lock().as_mut().is_some_and(|screen| {
        let before = screen.terminal.view_back();
        screen.terminal.move_view(change);
        screen.terminal.view_back() != before
    });
    if moved {
        request_render::<P>();
    }
}

/// Has the drawing thread look at the desktop when `input_queued` says input events wait for
/// it, or when the clock's second changed since it last looked; called by the input poller, or
/// without one by the thread [`start_clock`] starts.
pub fn tick<P: Platform>(input_queued: bool) {
    if !is_active() {
        return;
    }
    let second = P::clock_ticks() / P::clock_frequency().max(1);
    let new_second = SHOWN_SECOND.load(Ordering::Relaxed) != second;
    if input_queued || new_second {
        request_render::<P>();
    }
}

fn request_render<P: Platform>() {
    if !RENDER_PENDING.swap(true, Ordering::AcqRel)
        && let Some(drawer) = drawer()
    {
        drawer.wake_one::<P>();
    }
}

#[derive(Debug, Clone, Copy)]
struct Asked {
    action: Action,
    sent: bool,
    catalogue: bool,
}

fn take_events<P: Platform>(screen: &mut Screen, damage: &mut Damage) -> Asked {
    let mut action = Action::None;
    let Screen {
        desktop,
        clients,
        buffers,
        ..
    } = screen;
    let now = window::time_ms::<P>();
    let mut queues = Queues::new(clients, now);
    desktop.set_time(now);
    while let Some(event) = input::next_event() {
        let asked = match event {
            InputEvent::PointerMove { x, y } => {
                desktop.pointer_moved(x, y, damage, &mut queues);
                Action::None
            }
            InputEvent::Button {
                button: pressed_button,
                pressed,
            } => {
                let code = match pressed_button {
                    Button::Left => button::LEFT,
                    Button::Right => button::RIGHT,
                    Button::Middle => button::MIDDLE,
                };
                desktop.button(code, pressed, damage, &mut queues)
            }
            InputEvent::Key {
                code,
                pressed,
                modifiers,
            } => desktop.key(code, pressed, modifiers, damage, &mut queues),
            InputEvent::Wheel { delta } => {
                desktop.wheel(delta, &mut queues);
                Action::None
            }
        };
        if asked != Action::None {
            action = asked;
        }
    }
    if desktop.tick(now, damage, &mut queues) {
        buffers.release_unused(desktop);
    }
    let sent = queues.sent();
    let second = P::clock_ticks() / P::clock_frequency().max(1);
    SHOWN_SECOND.store(second, Ordering::Relaxed);
    desktop.set_uptime(second, damage);
    KEYS_TO_TERMINAL.store(desktop.keys_go_to_terminal(), Ordering::Relaxed);
    Asked {
        action,
        sent,
        catalogue: desktop.take_catalogue_request(),
    }
}

fn draw_screen<P: Platform>(
    screen: &mut Screen,
    framebuffer: &mut display::Framebuffer<'_>,
) -> Asked {
    let mut damage = Damage::new();
    let asked = match screen.session {
        Session::Interactive => take_events::<P>(screen, &mut damage),
        Session::SelfTest => Asked {
            action: Action::None,
            sent: false,
            catalogue: false,
        },
    };
    let mut drawn = Damage::new();
    screen.terminal.render(
        &mut screen.terminal_pixels,
        GRID_SIZE.0 as usize,
        0,
        0,
        &mut drawn,
    );
    if screen.desktop.terminal_shown() {
        let area = screen.desktop.text_area();
        for rect in drawn.rects() {
            damage.add(Rect::new(
                area.x + rect.x,
                area.y + rect.y,
                rect.width,
                rect.height,
            ));
        }
    }
    compose(
        &screen.desktop,
        &screen.terminal_pixels,
        &screen.buffers,
        framebuffer,
        &damage,
    );
    asked
}

fn launch_command(app: AppId) -> Option<Launch> {
    SCREEN
        .lock()
        .as_ref()
        .and_then(|screen| screen.desktop.launch_command(app))
}

fn launch_failed<P: Platform>() {
    with_windows::<P, _>(|desktop, _, damage, _| desktop.launch_failed(damage));
}

fn run_launch<P: Platform>(action: Action) {
    let launch = match action {
        Action::Start(app) | Action::Type(app) => launch_command(app),
        Action::Uninstall(app) => SCREEN
            .lock()
            .as_ref()
            .and_then(|screen| screen.desktop.uninstall_command(app))
            .map(Launch::Start),
        _ => None,
    };
    match launch {
        Some(Launch::Start(command)) => {
            if crate::apps::start_program(command.as_bytes()).is_err() {
                launch_failed::<P>();
            }
        }
        Some(Launch::Type(line)) => {
            move_view::<P>(ViewMove::Live);
            console::receive_from_keyboard::<P>(line.as_bytes());
        }
        None => {}
    }
}

fn read_catalogue_again<P: Platform>() {
    let catalogue = crate::apps::catalogue();
    with_windows::<P, _>(|desktop, _, damage, events| {
        desktop.set_catalogue(catalogue, damage, events)
    });
}

fn is_interactive() -> bool {
    SCREEN
        .lock()
        .as_ref()
        .is_some_and(|screen| screen.session == Session::Interactive)
}

/// The installed apps changed (`/dev/window`'s `APPS_CHANGED`, D134): the desktop reads the
/// catalogue again, so its launcher and icons follow at once; nothing in a self-test session.
pub fn reload_catalogue<P: Platform>() {
    if is_interactive() {
        read_catalogue_again::<P>();
    }
}

/// Opens the app called `name` as a double-click on its icon does (`OPEN_APP`, D134): a
/// built-in (Terminal, Files, Settings, D141; Text Editor, D149) or an installed app, after
/// reading the catalogue
/// again; `false` when there is no such app.
pub fn open_app<P: Platform>(name: &[u8]) -> bool {
    if !is_interactive() {
        return false;
    }
    read_catalogue_again::<P>();
    let action = with_windows::<P, _>(|desktop, _, damage, events| {
        let name = core::str::from_utf8(name).ok()?;
        let app = desktop.catalogue().app_named(name)?;
        Some(desktop.launch(app, damage, events))
    })
    .flatten();
    match action {
        Some(action) => {
            run_launch::<P>(action);
            true
        }
        None => false,
    }
}

/// Types `line` in the terminal window as a keyboard would, after Ctrl-U erases a half-typed
/// line and with Enter after it, and focuses the terminal (`TYPE_IN_TERMINAL`, D135); `false`
/// when there is no interactive desktop or the line does not fit.
pub fn type_in_terminal<P: Platform>(line: &[u8]) -> bool {
    if !is_interactive() {
        return false;
    }
    let mut typed = [0u8; TYPED_LINE_CAPACITY];
    let Some(end) = line.len().checked_add(2).filter(|&end| end <= typed.len()) else {
        return false;
    };
    typed[0] = KILL_LINE;
    typed[1..end - 1].copy_from_slice(line);
    typed[end - 1] = b'\r';
    let focused = with_windows::<P, _>(|desktop, _, damage, events| {
        desktop.launch(AppId::Terminal, damage, events)
    });
    if focused.is_none() {
        return false;
    }
    move_view::<P>(ViewMove::Live);
    console::receive_from_keyboard::<P>(&typed[..end]);
    true
}

/// Hands input sent to the terminal's window (`SEND_INPUT`, D141) to the terminal as a keyboard
/// or a wheel would: a key's bytes, in the cursor key mode a program asked for, or a character
/// to the console, a scroll to the view. Called with no lock held: the console's echo takes the
/// screen's lock.
pub fn send_to_terminal<P: Platform>(input: TerminalInput) {
    let mut character = [0u8; 1];
    let bytes;
    let typed = match input {
        TerminalInput::Key { code, modifiers } => {
            let mode = match application_cursor_keys() {
                true => CursorKeys::Application,
                false => CursorKeys::Normal,
            };
            bytes = keymap::encode(code, modifiers, mode);
            bytes.as_bytes()
        }
        TerminalInput::Character(byte) => {
            character[0] = byte;
            &character[..]
        }
        TerminalInput::Scroll { notches } => {
            let lines = notches.unsigned_abs() as usize * WHEEL_LINES;
            move_view::<P>(match notches > 0 {
                true => ViewMove::Back(lines),
                false => ViewMove::Forward(lines),
            });
            return;
        }
    };
    if !typed.is_empty() {
        move_view::<P>(ViewMove::Live);
        console::receive_from_keyboard::<P>(typed);
    }
}

/// Hands the desktop the input events queued for it, draws what they and the terminal changed
/// into the display's framebuffer and flushes it, then hands the replies programs asked for to
/// the console's input. Returns how many rectangles the flush showed. Does not return when the
/// power menu powered the machine off or restarted it.
pub fn show<P: Platform>() -> Result<usize, &'static str> {
    let drawn = display::draw(|framebuffer| {
        SCREEN
            .lock()
            .as_mut()
            .map(|screen| draw_screen::<P>(screen, framebuffer))
    })
    .flatten();
    if drawn.is_some_and(|asked| asked.sent) {
        console::wake_input_waiters::<P>();
    }
    match drawn.map(|asked| asked.action) {
        Some(Action::PowerOff) => take_machine_down::<P>(reboot::Action::PowerOff),
        Some(Action::Restart) => take_machine_down::<P>(reboot::Action::Restart),
        Some(action @ (Action::Start(_) | Action::Type(_) | Action::Uninstall(_))) => {
            run_launch::<P>(action)
        }
        Some(Action::None) | None => {}
    }
    if drawn.is_some_and(|asked| asked.catalogue) {
        read_catalogue_again::<P>();
    }
    let shown = display::flush::<P>();
    let mut replies = [0u8; REPLY_CAPACITY];
    let length = SCREEN.lock().as_mut().map_or(0, |screen| {
        let length = screen.terminal.replies().len();
        replies[..length].copy_from_slice(screen.terminal.replies());
        screen.terminal.clear_replies();
        length
    });
    if length > 0 {
        console::receive_from_keyboard::<P>(&replies[..length]);
    }
    shown
}

/// Starts the thread that draws the terminal if there is one; `Ok(false)` when there is none.
pub fn start_drawing<P: Platform>() -> Result<bool, SpawnError> {
    let Some(drawer) = drawer().filter(|_| is_active()) else {
        return Ok(false);
    };
    thread::spawn::<P, _>(move || {
        let interval = mul_div_ceil(FRAME_INTERVAL_MS, P::clock_frequency(), 1000);
        let mut next_frame = 0;
        loop {
            drawer.wait_until::<P>(|| RENDER_PENDING.load(Ordering::Acquire));
            if P::clock_ticks() < next_frame {
                thread::sleep_until::<P>(next_frame);
            }
            next_frame = P::clock_ticks().saturating_add(interval);
            RENDER_PENDING.store(false, Ordering::Release);
            let _ = show::<P>();
        }
    })
    .map(|_| true)
}

/// Starts the thread that has the desktop look at its clock once a second ([`tick`]) when no
/// input poller does it: a display without input devices, as an offscreen boot has (D144). It
/// ends when the terminal stops; `Ok(false)`, and nothing started, without a terminal.
pub fn start_clock<P: Platform>() -> Result<bool, SpawnError> {
    if !is_active() {
        return Ok(false);
    }
    thread::spawn::<P, _>(|| {
        while is_active() {
            thread::sleep_until::<P>(next_second(P::clock_ticks(), P::clock_frequency()));
            tick::<P>(false);
        }
    })
    .map(|_| true)
}

/// The clock tick the second after the one holding tick `now` starts at, on a clock of
/// `frequency` ticks a second.
pub fn next_second(now: u64, frequency: u64) -> u64 {
    let frequency = frequency.max(1);
    (now / frequency)
        .saturating_add(1)
        .saturating_mul(frequency)
}

/// What the terminal self-test writes: a prompt line and lines in SGR colours, with box
/// drawing, blocks and an arrow.
pub const TEST_SCREEN: &str = "eggsh:/# echo EggOS\nEggOS\n\x1b[1;31mred \x1b[32mgreen\x1b[0m \
     \x1b[7minverse\x1b[27m \x1b[4;94munderlined\x1b[0m \x1b[38;5;208;48;5;17mcube\x1b[m\n\
     ┌──┐ █▓▒░ → ●\n└──┘\n";

/// The [`display::pixel_hash`] of a 1280x800 display after the desktop's terminal showed
/// [`TEST_SCREEN`] in a [`Session::SelfTest`].
pub const TEST_SCREEN_HASH_1280X800: u64 = 0xcc62_008e_3c66_4155;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_clock_wakes_at_the_start_of_the_next_second() {
        assert_eq!(next_second(0, 1000), 1000);
        assert_eq!(next_second(999, 1000), 1000);
        assert_eq!(next_second(1000, 1000), 2000);
        assert_eq!(next_second(5, 0), 6);
        assert_eq!(next_second(u64::MAX, 1000), u64::MAX);
    }

    #[test]
    fn the_test_screen_s_hash_is_the_one_the_qemu_self_test_checks() {
        let desktop = Desktop::new(1280, 800).unwrap();
        let area = desktop.text_area();
        let mut terminal = Terminal::new(area.columns, area.rows).unwrap();
        terminal.set_onlcr(true);
        terminal.feed(TEST_SCREEN.as_bytes());
        let mut grid = grid_pixels().unwrap();
        terminal.render(&mut grid, GRID_SIZE.0 as usize, 0, 0, &mut Damage::new());
        let buffers = Buffers::new();
        let contents = ScreenContents {
            terminal: desktop.terminal_window(),
            terminal_pixels: &grid,
            buffers: &buffers,
        };
        let mut pixels = vec![0; 1280 * 800];
        desktop.draw(&mut pixels, 1280, desktop.screen(), &contents);
        assert_eq!(
            display::pixel_hash(&pixels),
            TEST_SCREEN_HASH_1280X800,
            "{:#x}",
            display::pixel_hash(&pixels)
        );
    }
}
