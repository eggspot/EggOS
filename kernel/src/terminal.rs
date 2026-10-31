//! The graphical terminal (D114) on the desktop (D115) and its window server (D127): the
//! console's session shown on the display in the Egg Mono font, by the terminal engine of
//! [`eggos_drivers::terminal`], in the terminal window of [`eggos_drivers::desktop`], among the
//! windows programs open through `/dev/window` ([`crate::window`]), over the wallpaper and
//! under the top bar; or, on a display too small for the desktop, alone on the whole display
//! (the full-screen console, D186).
//!
//! Once [`start`] made it, every byte [`console::write`] sends to the serial console is fed to
//! the terminal as well, under the terminal's own lock, so both see the same bytes in the same
//! order. Feeding only updates cells; the drawing thread ([`start_drawing`]) renders what changed
//! into the terminal's own buffer, the grid's pixels, then, with the terminal's lock released,
//! composes the rectangles that changed on screen into the framebuffer (each window's content
//! from its buffer, clipped by the windows above it) and flushes them, at most once every
//! [`FRAME_INTERVAL_MS`] and at once after an idle frame, so a burst of output costs one flush.
//! A console write waits at most for the terminal's render, never for the desktop's repaint
//! (D115's follow-up). The same thread hands the desktop the pointer and key events the input
//! layer queued, which may become events for the windows' programs, composes what they changed,
//! blinks the cursor and shows the pointer on a display that draws it itself; a program's
//! requests compose what they change during the call ([`with_windows`]). The display's flush,
//! which waits for the device, never holds the screen's lock: a stalled device stops the
//! picture, not the console. Without a display nothing is made and a console write costs one
//! load.
//!
//! In an interactive session the boot screen stays on the display until the first program waits
//! to read the console, its prompt written ([`console_waited`]), or [`BOOT_SCREEN_LIMIT_MS`]
//! after the desktop was made; then one frame draws the whole desktop.

use crate::console;
use crate::devices::interrupts::DeviceEvents;
use crate::display;
use crate::input::{self, Button, InputEvent};
use crate::linux::reboot;
use crate::sync::LeafSpinLock;
use crate::thread::{self, SpawnError, WaitQueue};
use crate::vfs;
use crate::window::{self, Buffers, Clients, Queues};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, Ordering};
use eggos_drivers::desktop::protocol::{DiscardEvents, EventSink, PointerShape};
use eggos_drivers::desktop::{
    Action, AppId, Contents, Desktop, GRID_SIZE, Launch, Profile, TerminalInput, WindowId, colour,
    shortcut,
};
use eggos_drivers::display::{Damage, Rect};
use eggos_drivers::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use eggos_drivers::input::button;
use eggos_drivers::input::keymap::{self, CursorKeys, Modifiers};
use eggos_drivers::terminal::{
    CursorLook, REPLY_CAPACITY, TERM_BACKGROUND, Terminal, TextArea, WHEEL_LINES,
};
use eggos_hal::Platform;
use eggos_hal::time::mul_div_ceil;

pub use eggos_drivers::terminal::ViewMove;

/// The shortest time between two flushes of the terminal: 60 frames a second.
pub const FRAME_INTERVAL_MS: u64 = 16;

/// How long the cursor shows, then hides, in each blink (docs/ux/terminal-v0.3.md, "Cursor").
pub const BLINK_HALF_MS: u64 = 500;

/// How long the boot screen waits for the first prompt once the desktop is made, at most
/// (docs/ux/desktop-polish-v0.3.md, section 3).
pub const BOOT_SCREEN_LIMIT_MS: u64 = 1000;

/// The smallest display whose full-screen console keeps the terminal's padding around its grid;
/// a smaller one uses every pixel (docs/ux/desktop-polish-v0.3.md, section 2).
pub const PADDED_CONSOLE_LEAST: (u32, u32) = (320, 240);

/// Who the desktop serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// The user: the desktop takes the pointer and key events, the clock shows the time of
    /// day once the wall clock is set (D150), else the uptime, and the cursor blinks.
    Interactive,
    /// A self-test: no input is taken, the clock stays at `up 0:00` and the cursor does not
    /// blink, so the picture is the same on every boot.
    SelfTest,
}

struct Screen {
    terminal_pixels: Box<[u32]>,
    desktop: Option<Desktop>,
    console: TextArea,
    session: Session,
    buffers: Buffers,
    clients: Clients,
    boot_screen_until: Option<u64>,
    active_at: u64,
}

impl Screen {
    fn text_area(&self) -> TextArea {
        self.desktop
            .as_ref()
            .map_or(self.console, Desktop::text_area)
    }

    fn grid_stride(&self) -> usize {
        match self.desktop {
            Some(_) => GRID_SIZE.0 as usize,
            None => self.console.rect().width as usize,
        }
    }
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

fn compose(screen: &Screen, framebuffer: &mut display::Framebuffer<'_>, damage: &Damage) {
    if screen.boot_screen_until.is_some() {
        return;
    }
    let Some(desktop) = screen.desktop.as_ref() else {
        for &rect in damage.rects() {
            draw_console(framebuffer, screen.console, &screen.terminal_pixels, rect);
        }
        return;
    };
    let stride = framebuffer.mode().width as usize;
    let contents = ScreenContents {
        terminal: desktop.terminal_window(),
        terminal_pixels: &screen.terminal_pixels,
        buffers: &screen.buffers,
    };
    for &rect in damage.rects() {
        desktop.draw(framebuffer.pixels(), stride, rect, &contents);
        framebuffer.damage(rect);
    }
}

fn draw_console(
    framebuffer: &mut display::Framebuffer<'_>,
    area: TextArea,
    grid: &[u32],
    rect: Rect,
) {
    let mode = framebuffer.mode();
    let rect = rect.clipped(mode.width, mode.height);
    let width = mode.width as usize;
    let grid_rect = area.rect();
    let grid_width = grid_rect.width as usize;
    let pixels = framebuffer.pixels();
    for row in rect.y..rect.bottom() {
        let start = row as usize * width;
        let Some(line) = pixels.get_mut(start + rect.x as usize..start + rect.right() as usize)
        else {
            break;
        };
        line.fill(TERM_BACKGROUND);
        if row < grid_rect.y || row >= grid_rect.bottom() {
            continue;
        }
        let (left, right) = (rect.x.max(grid_rect.x), rect.right().min(grid_rect.right()));
        if left >= right {
            continue;
        }
        let from = (row - grid_rect.y) as usize * grid_width + (left - grid_rect.x) as usize;
        let count = (right - left) as usize;
        let into = (left - rect.x) as usize;
        if let Some(source) = grid.get(from..from + count) {
            line[into..into + count].copy_from_slice(source);
        }
    }
    framebuffer.damage(rect);
}

fn grid_pixels(area: TextArea) -> Option<Box<[u32]>> {
    let rect = area.rect();
    let count = rect.width as usize * rect.height as usize;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(count).ok()?;
    pixels.resize(count, TERM_BACKGROUND);
    Some(pixels.into_boxed_slice())
}

/// The grid of the full-screen console on a `width` by `height` display: inside the terminal's
/// padding from [`PADDED_CONSOLE_LEAST`] up, else on every pixel; `None` without room for a
/// cell.
pub fn console_area(width: u32, height: u32) -> Option<TextArea> {
    if width >= PADDED_CONSOLE_LEAST.0 && height >= PADDED_CONSOLE_LEAST.1 {
        return TextArea::full_screen(width, height);
    }
    let columns = width / GLYPH_WIDTH as u32;
    let rows = height / GLYPH_HEIGHT as u32;
    (columns > 0 && rows > 0).then(|| TextArea {
        x: 0,
        y: 0,
        columns: columns.min(u32::from(u16::MAX)) as u16,
        rows: rows.min(u32::from(u16::MAX)) as u16,
    })
}

static TERMINAL: LeafSpinLock<Option<Box<Terminal>>> = LeafSpinLock::new_leaf(None);
static SCREEN: LeafSpinLock<Option<Box<Screen>>> = LeafSpinLock::new_leaf(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static RENDER_PENDING: AtomicBool = AtomicBool::new(false);

static AGENT_CONNECTED: AtomicBool = AtomicBool::new(false);
static APPLICATION_CURSOR_KEYS: AtomicBool = AtomicBool::new(false);
static DRAWER: AtomicPtr<WaitQueue> = AtomicPtr::new(core::ptr::null_mut());
static KEYS_TO_TERMINAL: AtomicBool = AtomicBool::new(true);
static SHOWN_CLOCK: AtomicU64 = AtomicU64::new(0);
static BOOT_SCREEN_UP: AtomicBool = AtomicBool::new(false);
static TYPED: AtomicBool = AtomicBool::new(false);
static LOOK_AGAIN_AT: AtomicU64 = AtomicU64::new(NO_LOOK);
static CLOCK_EVENTS: DeviceEvents = DeviceEvents::new();

const WALL_CLOCK_KEY: u64 = 1 << 63;
const NO_LOOK: u64 = 0;

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
/// settings file holds, read at boot ([`load_saved_settings`]), or the last one set through
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
/// before the desktop starts, so its first frame is already in it, and its keyboard layout the
/// keyboard's (D185), so the first key types in it; the defaults stay when there is no file, it
/// holds no known profile or layout, or it cannot be read.
pub fn load_saved_settings() {
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
    if let Some(layout) = eggos_settings_file::keyboard_layout(&file[..length]) {
        crate::input::set_layout(layout);
    }
    let keys = eggos_settings_file::key_accessibility(&file[..length]);
    crate::input::set_key_accessibility(
        keys.sticky_keys,
        keys.slow_keys.then_some(keys.slow_keys_ms),
    );
    let input = eggos_settings_file::input_preferences(&file[..length]);
    crate::input::set_preferences(crate::input::Preferences {
        left_handed: input.left_handed,
        natural_scrolling: input.natural_scrolling,
        pointer_speed: input.pointer_speed,
        repeat_delay_ms: input.repeat_delay_ms,
        repeat_rate: input.repeat_rate,
    });
    if let Some(scale) = eggos_settings_file::display_scale(&file[..length])
        && scale != crate::display::scale()
    {
        crate::display::set_scale(scale);
    }
}

/// Whether key `code` pressed with `modifiers` is a shortcut the desktop takes from the
/// terminal, which takes every Ctrl key: its bytes do not go to the console. The full-screen
/// console has no shortcuts.
pub fn takes_from_terminal(code: u16, modifiers: Modifiers) -> bool {
    is_active()
        && !CONSOLE_ALONE.load(Ordering::Relaxed)
        && shortcut(code, modifiers, desktop_profile(), true).is_some()
}

static CONSOLE_ALONE: AtomicBool = AtomicBool::new(false);

/// Makes the desktop with the terminal's window, blank, or the full-screen console on a display
/// too small for the desktop, and from then on mirrors the console's output on it; returns the
/// grid. A self-test session draws it over the whole display at once; an interactive one leaves
/// the boot screen showing until the first prompt ([`console_waited`]) or
/// [`BOOT_SCREEN_LIMIT_MS`]. `None`, and nothing made, without a display, without room for one
/// cell, or without memory for it.
pub fn start<P: Platform>(session: Session) -> Option<TextArea> {
    let mode = display::mode()?;
    let desktop = match Desktop::new(mode.width, mode.height) {
        Some(mut desktop) => {
            desktop.set_profile(desktop_profile(), &mut Damage::new(), &mut DiscardEvents);
            if session == Session::Interactive {
                desktop.set_catalogue(
                    crate::apps::catalogue(),
                    &mut Damage::new(),
                    &mut DiscardEvents,
                );
                desktop.set_pointer_on_display(display::draws_pointer(), &mut Damage::new());
            }
            Some(desktop)
        }
        None => None,
    };
    let console = match &desktop {
        Some(desktop) => desktop.text_area(),
        None => console_area(mode.width, mode.height)?,
    };
    let mut terminal = Box::new(Terminal::new(console.columns, console.rows)?);
    terminal.set_onlcr(true);
    terminal.assume_drawn_blank();
    let now = P::clock_ticks();
    let booting = session == Session::Interactive
        && display::draw(|framebuffer| framebuffer.boot_screen_shown()).unwrap_or(false);
    let limit = mul_div_ceil(BOOT_SCREEN_LIMIT_MS, P::clock_frequency(), 1000);
    let boot_screen_until = match booting {
        true => Some(now.saturating_add(limit)),
        false => None,
    };
    let grid = match desktop {
        Some(_) => TextArea {
            x: 0,
            y: 0,
            columns: console.columns,
            rows: console.rows,
        },
        None => console,
    };
    let screen = Box::new(Screen {
        terminal_pixels: grid_pixels(grid)?,
        desktop,
        console,
        session,
        buffers: Buffers::new(),
        clients: Clients::new(),
        boot_screen_until,
        active_at: now,
    });
    if drawer().is_none() {
        DRAWER.store(Box::into_raw(Box::new(WaitQueue::new())), Ordering::Release);
    }
    display::draw(|framebuffer| {
        if booting {
            return;
        }
        framebuffer.end_boot_screen();
        let mut whole = Damage::new();
        whole.add(mode.rect());
        compose(&screen, framebuffer, &whole);
    })?;
    KEYS_TO_TERMINAL.store(true, Ordering::Relaxed);
    CONSOLE_ALONE.store(screen.desktop.is_none(), Ordering::Relaxed);
    SHOWN_CLOCK.store(0, Ordering::Relaxed);
    BOOT_SCREEN_UP.store(screen.boot_screen_until.is_some(), Ordering::Release);
    LOOK_AGAIN_AT.store(
        screen.boot_screen_until.unwrap_or(NO_LOOK),
        Ordering::Relaxed,
    );
    if session == Session::Interactive {
        input::subscribe();
    }
    *TERMINAL.lock() = Some(terminal);
    *SCREEN.lock() = Some(screen);
    crate::linux::terminal::set_window_size(
        console.rows,
        console.columns,
        GLYPH_WIDTH as u16,
        GLYPH_HEIGHT as u16,
    );
    ACTIVE.store(true, Ordering::Release);
    request_render::<P>();
    Some(console)
}

/// Runs `change` on the window server's desktop and buffers with the damage it records and
/// the sink for its clients' events, under the display's and the screen's locks; then composes
/// the damage into the framebuffer, has it flushed and wakes the programs that got events.
/// `None` without a screen, and on the full-screen console, which has no window server.
pub fn with_windows<P: Platform, R>(
    change: impl FnOnce(&mut Desktop, &mut Buffers, &mut Damage, &mut dyn EventSink) -> R,
) -> Option<R> {
    let time_ms = window::time_ms::<P>();
    let (result, sent, damaged) = display::draw(|framebuffer| {
        let mut guard = SCREEN.lock();
        let screen = &mut **guard.as_mut()?;
        let desktop = screen.desktop.as_mut()?;
        let mut queues = Queues::new(&screen.clients, time_ms);
        let mut damage = Damage::new();
        desktop.set_time(time_ms);
        let pointer = desktop.pointer();
        let result = change(desktop, &mut screen.buffers, &mut damage, &mut queues);
        let keys = desktop.keys_go_to_terminal();
        let pointer_changed = desktop.pointer_on_display() && desktop.pointer() != pointer;
        let sent = queues.sent();
        compose(screen, framebuffer, &damage);
        KEYS_TO_TERMINAL.store(keys, Ordering::Relaxed);
        Some((result, sent, !damage.is_empty() || pointer_changed))
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
    BOOT_SCREEN_UP.store(false, Ordering::Release);
    LOOK_AGAIN_AT.store(NO_LOOK, Ordering::Relaxed);
    CONSOLE_ALONE.store(false, Ordering::Relaxed);
    let screen = SCREEN.lock().take();
    drop(screen);
    let terminal = TERMINAL.lock().take();
    drop(terminal);
}

/// Writes `bytes` to the platform's console and feeds them to the terminal, in one step under
/// the terminal's lock, then asks the drawing thread to show them. Called by
/// [`console::write`] while the terminal [`is_active`]. The lock is the terminal's alone, which
/// the drawing thread holds only while it renders the terminal's changed cells.
#[inline(never)]
pub fn write_mirrored<P: Platform>(bytes: &[u8]) {
    let mut terminal = TERMINAL.lock();
    P::console_write(bytes);
    if let Some(terminal) = terminal.as_mut() {
        terminal.feed(bytes);
        APPLICATION_CURSOR_KEYS.store(terminal.application_cursor_keys(), Ordering::Relaxed);
    }
    drop(terminal);
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
    let moved = TERMINAL.lock().as_mut().is_some_and(|terminal| {
        let before = terminal.view_back();
        terminal.move_view(change);
        terminal.view_back() != before
    });
    if moved {
        request_render::<P>();
    }
}

/// Bytes were typed for the console: the cursor's blink starts again with the cursor shown.
/// One store, and nothing without a terminal.
#[inline]
pub fn note_typing() {
    if is_active() {
        TYPED.store(true, Ordering::Relaxed);
    }
}

/// A program waits to read the console, so its prompt is written: the first time after the
/// desktop was made, the desktop replaces the boot screen. One load after that.
#[inline]
pub fn console_waited<P: Platform>() {
    if BOOT_SCREEN_UP.load(Ordering::Relaxed) && BOOT_SCREEN_UP.swap(false, Ordering::AcqRel) {
        request_render::<P>();
    }
}

/// Has the drawing thread look at the desktop when `input_queued` says input events wait for
/// it, when a new second started since it last looked, or when the time it asked to look again
/// at came (the cursor's blink, the boot screen's limit): each second runs the desktop's timers
/// (a close's timeout, a notice's expiry, a tooltip's delay, D144) and updates the bar's clock,
/// which redraws only when its text changes (the wall clock's minute once it is set, else the
/// uptime's second, D150); called by the input poller, or without one by the thread
/// [`start_clock`] starts.
pub fn tick<P: Platform>(input_queued: bool) {
    if !is_active() {
        return;
    }
    let new_clock = SHOWN_CLOCK.load(Ordering::Relaxed) != clock_key::<P>();
    let look_again = LOOK_AGAIN_AT.load(Ordering::Relaxed);
    let due = look_again != NO_LOOK && P::clock_ticks() >= look_again;
    if input_queued || new_clock || due {
        request_render::<P>();
    }
}

fn clock_key<P: Platform>() -> u64 {
    clock_key_at(
        crate::wall_clock::is_set().then(crate::wall_clock::seconds::<P>),
        P::clock_ticks() / P::clock_frequency().max(1),
    )
}

fn clock_key_at(set_wall_seconds: Option<u64>, uptime_seconds: u64) -> u64 {
    match set_wall_seconds {
        Some(seconds) => WALL_CLOCK_KEY | seconds,
        None => uptime_seconds,
    }
}

fn request_render<P: Platform>() {
    if !RENDER_PENDING.swap(true, Ordering::AcqRel)
        && let Some(drawer) = drawer()
    {
        drawer.wake_one::<P>();
    }
}

/// The cursor's look at clock tick `now` and the tick it next changes at, if it does: a terminal
/// without focus shows the outline, one that does not blink the block; a blinking one shows the
/// block for `half` ticks from `since`, the last byte in or out, then nothing for `half`, and so
/// on.
pub fn cursor_look_at(
    focused: bool,
    blinks: bool,
    since: u64,
    now: u64,
    half: u64,
) -> (CursorLook, Option<u64>) {
    if !focused {
        return (CursorLook::Outline, None);
    }
    if !blinks || half == 0 {
        return (CursorLook::Block, None);
    }
    let halves = now.saturating_sub(since) / half;
    let look = match halves % 2 {
        0 => CursorLook::Block,
        _ => CursorLook::Blinked,
    };
    let next = since.saturating_add(halves.saturating_add(1).saturating_mul(half));
    (look, Some(next))
}

#[derive(Debug, Clone, Copy)]
struct Asked {
    action: Action,
    sent: bool,
    catalogue: bool,
    pointer: Option<(u32, u32, PointerShape)>,
    look_again_at: u64,
}

impl Asked {
    const NOTHING: Asked = Asked {
        action: Action::None,
        sent: false,
        catalogue: false,
        pointer: None,
        look_again_at: NO_LOOK,
    };
}

fn take_events<P: Platform>(screen: &mut Screen, damage: &mut Damage) -> Asked {
    let Screen {
        desktop,
        clients,
        buffers,
        ..
    } = screen;
    let Some(desktop) = desktop.as_mut() else {
        while input::next_event().is_some() {}
        return Asked::NOTHING;
    };
    let mut action = Action::None;
    let now = window::time_ms::<P>();
    let mut queues = Queues::new(clients, now);
    desktop.set_time(now);
    desktop.set_agent_connected(AGENT_CONNECTED.load(Ordering::Relaxed), damage);
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
                character,
            } => desktop.key_typing(code, pressed, modifiers, character, damage, &mut queues),
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
    let clock = clock_key::<P>();
    SHOWN_CLOCK.store(clock, Ordering::Relaxed);
    match clock & WALL_CLOCK_KEY != 0 {
        true => desktop.set_wall_clock(clock & !WALL_CLOCK_KEY, damage),
        false => desktop.set_uptime(clock, damage),
    };
    KEYS_TO_TERMINAL.store(desktop.keys_go_to_terminal(), Ordering::Relaxed);
    Asked {
        action,
        sent,
        catalogue: desktop.take_catalogue_request(),
        pointer: match desktop.pointer_on_display() {
            true => desktop.pointer(),
            false => None,
        },
        look_again_at: NO_LOOK,
    }
}

fn end_boot_screen_if_due(
    screen: &mut Screen,
    framebuffer: &mut display::Framebuffer<'_>,
    now: u64,
    damage: &mut Damage,
) {
    let Some(until) = screen.boot_screen_until else {
        return;
    };
    if BOOT_SCREEN_UP.load(Ordering::Acquire) && now < until {
        return;
    }
    BOOT_SCREEN_UP.store(false, Ordering::Release);
    screen.boot_screen_until = None;
    framebuffer.end_boot_screen();
    damage.add(framebuffer.mode().rect());
}

fn render_terminal<P: Platform>(screen: &mut Screen, now: u64, damage: &mut Damage) -> u64 {
    let mut terminal = TERMINAL.lock();
    let Some(terminal) = terminal.as_mut() else {
        return NO_LOOK;
    };
    if terminal.needs_render() || TYPED.swap(false, Ordering::Relaxed) {
        screen.active_at = now;
    }
    let focused = screen.desktop.as_ref().is_none_or(Desktop::focused);
    let blinks = screen.session == Session::Interactive && terminal.cursor_blinks();
    let half = mul_div_ceil(BLINK_HALF_MS, P::clock_frequency(), 1000);
    let (look, next) = cursor_look_at(focused, blinks, screen.active_at, now, half);
    terminal.set_cursor_look(look);
    let stride = screen.grid_stride();
    let mut drawn = Damage::new();
    terminal.render(&mut screen.terminal_pixels, stride, 0, 0, &mut drawn);
    let shown = screen.desktop.as_ref().is_none_or(Desktop::terminal_shown);
    if shown {
        let area = screen.text_area();
        for rect in drawn.rects() {
            damage.add(Rect::new(
                area.x + rect.x,
                area.y + rect.y,
                rect.width,
                rect.height,
            ));
        }
    }
    next.unwrap_or(NO_LOOK)
}

fn draw_screen<P: Platform>(
    screen: &mut Screen,
    framebuffer: &mut display::Framebuffer<'_>,
) -> Asked {
    let now = P::clock_ticks();
    let mut damage = Damage::new();
    let mut asked = match screen.session {
        Session::Interactive => take_events::<P>(screen, &mut damage),
        Session::SelfTest => Asked::NOTHING,
    };
    end_boot_screen_if_due(screen, framebuffer, now, &mut damage);
    let blink = render_terminal::<P>(screen, now, &mut damage);
    compose(screen, framebuffer, &damage);
    asked.look_again_at = match (blink, screen.boot_screen_until) {
        (NO_LOOK, until) => until.unwrap_or(NO_LOOK),
        (blink, Some(until)) => blink.min(until),
        (blink, None) => blink,
    };
    if screen.boot_screen_until.is_some() {
        asked.pointer = None;
    }
    asked
}

fn launch_command(app: AppId) -> Option<Launch> {
    SCREEN
        .lock()
        .as_ref()
        .and_then(|screen| screen.desktop.as_ref()?.launch_command(app))
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
            .and_then(|screen| screen.desktop.as_ref()?.uninstall_command(app))
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
/// when there is no interactive desktop or the line does not fit. On the full-screen console
/// the line is typed as well, there being nothing to focus.
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
    let focused =
        with_windows::<P, _>(|desktop, _, damage, events| desktop.focus_console(damage, events));
    if focused.is_none() && !CONSOLE_ALONE.load(Ordering::Relaxed) {
        return false;
    }
    move_view::<P>(ViewMove::Live);
    console::receive_from_keyboard::<P>(&typed[..end]);
    true
}

/// Hands input sent to the terminal's window (`SEND_INPUT`, D141) to the terminal as a keyboard
/// or a wheel would: a key's bytes, in the cursor key mode a program asked for, or a character
/// to the console, a scroll to the view. Called with no lock held: the console's echo takes the
/// terminal's lock.
pub fn send_to_terminal<P: Platform>(input: TerminalInput) {
    let mut character = [0u8; 4];
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
        TerminalInput::Character(typed) => typed.encode_utf8(&mut character).as_bytes(),
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
/// into the display's framebuffer, shows the pointer on a display that draws it and flushes,
/// then hands the replies programs asked for to the console's input. Returns how many
/// rectangles the flush showed. The power menu asks init to power off or restart
/// ([`reboot::shut_down`]), and does it at once without an init.
pub fn show<P: Platform>() -> Result<usize, &'static str> {
    AGENT_CONNECTED.store(crate::agent_channel::is_connected(), Ordering::Relaxed);
    let drawn = display::draw(|framebuffer| {
        SCREEN
            .lock()
            .as_mut()
            .map(|screen| draw_screen::<P>(screen, framebuffer))
    })
    .flatten();
    if let Some(asked) = drawn {
        look_again_at::<P>(asked.look_again_at);
    }
    if drawn.is_some_and(|asked| asked.sent) {
        console::wake_input_waiters::<P>();
    }
    match drawn.map(|asked| asked.action) {
        Some(Action::PowerOff) => reboot::shut_down::<P>(reboot::Action::PowerOff),
        Some(Action::Restart) => reboot::shut_down::<P>(reboot::Action::Restart),
        Some(action @ (Action::Start(_) | Action::Type(_) | Action::Uninstall(_))) => {
            run_launch::<P>(action)
        }
        Some(Action::ShowAgentLog) => {
            move_view::<P>(ViewMove::Live);
            console::receive_from_keyboard::<P>(eggos_drivers::desktop::AGENT_LOG_LINE);
        }
        Some(Action::None) | None => {}
    }
    if drawn.is_some_and(|asked| asked.catalogue) {
        read_catalogue_again::<P>();
    }
    if let Some(asked) = drawn.filter(|asked| asked.pointer.is_some()) {
        let _ = display::show_pointer::<P>(asked.pointer);
    }
    let shown = display::flush::<P>();
    let mut replies = [0u8; REPLY_CAPACITY];
    let length = TERMINAL.lock().as_mut().map_or(0, |terminal| {
        let length = terminal.replies().len();
        replies[..length].copy_from_slice(terminal.replies());
        terminal.clear_replies();
        length
    });
    if length > 0 {
        console::receive_from_keyboard::<P>(&replies[..length]);
    }
    shown
}

fn look_again_at<P: Platform>(at: u64) {
    let before = LOOK_AGAIN_AT.swap(at, Ordering::Relaxed);
    if at != NO_LOOK && (before == NO_LOOK || at < before) {
        CLOCK_EVENTS.raise::<P>();
    }
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

/// Starts the thread that has the desktop look at its clock once a second, and at the times the
/// drawing thread asks for (the cursor's blink, the boot screen's limit), through [`tick`], when
/// no input poller does it: input devices on interrupts (D184), or none, as an offscreen boot has
/// (D144). It ends when the terminal stops; `Ok(false)`, and nothing started, without a terminal.
pub fn start_clock<P: Platform>() -> Result<bool, SpawnError> {
    if !is_active() {
        return Ok(false);
    }
    thread::spawn::<P, _>(|| {
        let frame = mul_div_ceil(FRAME_INTERVAL_MS, P::clock_frequency(), 1000);
        let mut seen = CLOCK_EVENTS.raised();
        while is_active() {
            let now = P::clock_ticks();
            let second = next_second(now, P::clock_frequency());
            let deadline = match LOOK_AGAIN_AT.load(Ordering::Relaxed) {
                NO_LOOK => second,
                at => second.min(at.max(now.saturating_add(frame))),
            };
            seen = CLOCK_EVENTS.wait::<P>(seen, deadline);
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
pub const TEST_SCREEN_HASH_1280X800: u64 = 0x4f80_909d_8dc5_a395;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use eggos_drivers::desktop::LAUNCH_FAILED;
    use eggos_drivers::terminal::PADDING;

    #[test]
    fn the_clock_wakes_at_the_start_of_the_next_second() {
        assert_eq!(next_second(0, 1000), 1000);
        assert_eq!(next_second(999, 1000), 1000);
        assert_eq!(next_second(1000, 1000), 2000);
        assert_eq!(next_second(5, 0), 6);
        assert_eq!(next_second(u64::MAX, 1000), u64::MAX);
    }

    #[test]
    fn once_the_wall_clock_is_set_the_desktop_still_looks_each_second_and_a_notice_expires() {
        let wall = 1_793_282_700;
        let mut desktop = Desktop::new(1280, 800).unwrap();
        let mut damage = Damage::new();
        desktop.set_time(1000);
        desktop.show_notice(LAUNCH_FAILED, &mut damage);
        assert!(desktop.set_wall_clock(wall, &mut damage));
        let mut shown = clock_key_at(Some(wall), 1);
        for second in 1..=3 {
            let key = clock_key_at(Some(wall + second), 1 + second);
            assert_ne!(key, shown, "second {second}");
            shown = key;
            let mut damage = Damage::new();
            desktop.tick(1000 + second as u32 * 1000, &mut damage, &mut DiscardEvents);
            assert!(!desktop.set_wall_clock(key & !WALL_CLOCK_KEY, &mut damage));
        }
        assert!(desktop.notice_area().is_none());
        assert_eq!(clock_key_at(None, 2 * 86_400), 2 * 86_400);
        assert_eq!(clock_key_at(Some(3600), 3600), WALL_CLOCK_KEY | 3600);
    }

    #[test]
    fn the_test_screen_s_hash_is_the_one_the_qemu_self_test_checks() {
        let desktop = Desktop::new(1280, 800).unwrap();
        let area = desktop.text_area();
        let mut terminal = Terminal::new(area.columns, area.rows).unwrap();
        terminal.set_onlcr(true);
        terminal.feed(TEST_SCREEN.as_bytes());
        let grid_area = TextArea {
            x: 0,
            y: 0,
            columns: area.columns,
            rows: area.rows,
        };
        let mut grid = grid_pixels(grid_area).unwrap();
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

    #[test]
    fn the_cursor_shows_half_a_second_then_hides_from_the_last_byte_and_keeps_still_unfocused() {
        let half = 500;
        assert_eq!(
            cursor_look_at(true, true, 1000, 1000, half),
            (CursorLook::Block, Some(1500))
        );
        assert_eq!(
            cursor_look_at(true, true, 1000, 1499, half),
            (CursorLook::Block, Some(1500))
        );
        assert_eq!(
            cursor_look_at(true, true, 1000, 1500, half),
            (CursorLook::Blinked, Some(2000))
        );
        assert_eq!(
            cursor_look_at(true, true, 1000, 2250, half),
            (CursorLook::Block, Some(2500))
        );
        assert_eq!(
            cursor_look_at(true, false, 1000, 1700, half),
            (CursorLook::Block, None)
        );
        assert_eq!(
            cursor_look_at(false, true, 1000, 1700, half),
            (CursorLook::Outline, None)
        );
        assert_eq!(
            cursor_look_at(true, true, 2000, 1000, half),
            (CursorLook::Block, Some(2500))
        );
    }

    #[test]
    fn a_small_display_gets_a_full_screen_console_padded_from_320x240() {
        assert!(!Desktop::fits(640, 400));
        assert_eq!(
            console_area(640, 400),
            Some(TextArea {
                x: PADDING,
                y: PADDING,
                columns: 78,
                rows: 24
            })
        );
        assert_eq!(
            console_area(320, 240),
            Some(TextArea {
                x: PADDING,
                y: PADDING,
                columns: 38,
                rows: 14
            })
        );
        assert_eq!(
            console_area(160, 128),
            Some(TextArea {
                x: 0,
                y: 0,
                columns: 20,
                rows: 8
            })
        );
        assert_eq!(console_area(7, 100), None);
        assert_eq!(console_area(100, 15), None);
    }
}
