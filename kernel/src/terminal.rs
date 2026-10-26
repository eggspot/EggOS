//! The graphical terminal (D114) on the desktop (D115): the console's session shown on the
//! display in the Egg Mono font, by the terminal engine of [`eggos_drivers::terminal`], in the
//! terminal window of [`eggos_drivers::desktop`], over the wallpaper and under the top bar.
//!
//! Once [`start`] made it, every byte [`console::write`] sends to the serial console is fed to
//! the terminal as well, under the terminal's lock, so both see the same bytes in the same
//! order. Feeding only updates cells; the drawing thread ([`start_drawing`]) renders what changed
//! and flushes it to the display, at most once every [`FRAME_INTERVAL_MS`] and at once after an
//! idle frame, so a burst of output costs one flush. The same thread hands the desktop the
//! pointer and key events the input layer queued and repaints the rectangles they changed.
//! The display's flush, which waits for the device, never holds the terminal's lock: a stalled
//! device stops the picture, not the console. Without a display nothing is made and a console
//! write costs one load.

use crate::console;
use crate::display;
use crate::input::{self, Button, InputEvent};
use crate::linux::reboot::{self, take_machine_down};
use crate::sync::LeafSpinLock;
use crate::thread::{self, SpawnError, WaitQueue};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use eggos_drivers::desktop::{Action, Desktop};
use eggos_drivers::display::Damage;
use eggos_drivers::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use eggos_drivers::terminal::{REPLY_CAPACITY, Terminal, TextArea};
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
    desktop: Desktop,
    session: Session,
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

/// Draws the desktop with the terminal's window, blank, over the whole display, and from then
/// on mirrors the console's output on it; returns the window's grid. The display shows it at
/// the next [`show`], the drawing thread's. `None`, and nothing made, without a display, when
/// the window does not fit on it, or without memory for it.
pub fn start<P: Platform>(session: Session) -> Option<TextArea> {
    let mode = display::mode()?;
    let desktop = Desktop::new(mode.width, mode.height)?;
    let area = desktop.text_area();
    let mut terminal = Terminal::new(area.columns, area.rows)?;
    terminal.set_onlcr(true);
    terminal.assume_drawn_blank();
    let screen = Box::new(Screen {
        terminal,
        desktop,
        session,
    });
    if drawer().is_none() {
        DRAWER.store(Box::into_raw(Box::new(WaitQueue::new())), Ordering::Release);
    }
    display::draw(|framebuffer| {
        let stride = framebuffer.mode().width as usize;
        screen
            .desktop
            .draw(framebuffer.pixels(), stride, mode.rect());
        framebuffer.damage(mode.rect());
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
/// it, or when the clock's second changed since it last looked; called by the input poller.
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

fn take_events<P: Platform>(screen: &mut Screen, damage: &mut Damage) -> Action {
    let mut action = Action::None;
    while let Some(event) = input::next_event() {
        let desktop = &mut screen.desktop;
        let asked = match event {
            InputEvent::PointerMove { x, y } => {
                desktop.pointer_moved(x, y, damage);
                Action::None
            }
            InputEvent::Button {
                button: Button::Left,
                pressed,
            } => desktop.left_button(pressed, damage),
            InputEvent::Key { code, pressed, .. } => desktop.key(code, pressed, damage),
            InputEvent::Button { .. } | InputEvent::Wheel { .. } => Action::None,
        };
        if asked != Action::None {
            action = asked;
        }
    }
    let second = P::clock_ticks() / P::clock_frequency().max(1);
    SHOWN_SECOND.store(second, Ordering::Relaxed);
    screen.desktop.set_uptime(second, damage);
    KEYS_TO_TERMINAL.store(screen.desktop.keys_go_to_terminal(), Ordering::Relaxed);
    action
}

fn draw_screen<P: Platform>(
    screen: &mut Screen,
    framebuffer: &mut display::Framebuffer<'_>,
) -> Action {
    let stride = framebuffer.mode().width as usize;
    let mut damage = Damage::new();
    let action = match screen.session {
        Session::Interactive => take_events::<P>(screen, &mut damage),
        Session::SelfTest => Action::None,
    };
    let grid = screen.desktop.text_area().rect();
    for &rect in damage.rects() {
        screen.desktop.draw(framebuffer.pixels(), stride, rect);
        framebuffer.damage(rect);
        if rect.overlaps(&grid) {
            screen.terminal.invalidate();
        }
    }
    if screen.desktop.terminal_shown() {
        let mut drawn = Damage::new();
        screen
            .terminal
            .render(framebuffer.pixels(), stride, grid.x, grid.y, &mut drawn);
        for &rect in drawn.rects() {
            framebuffer.damage(rect);
        }
    }
    action
}

/// Hands the desktop the input events queued for it, draws what they and the terminal changed
/// into the display's framebuffer and flushes it, then hands the replies programs asked for to
/// the console's input. Returns how many rectangles the flush showed. Does not return when the
/// power menu powered the machine off or restarted it.
pub fn show<P: Platform>() -> Result<usize, &'static str> {
    let action = display::draw(|framebuffer| {
        SCREEN
            .lock()
            .as_mut()
            .map_or(Action::None, |screen| draw_screen::<P>(screen, framebuffer))
    });
    match action {
        Some(Action::PowerOff) => take_machine_down::<P>(reboot::Action::PowerOff),
        Some(Action::Restart) => take_machine_down::<P>(reboot::Action::Restart),
        Some(Action::None) | None => {}
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

/// What the terminal self-test writes: a prompt line and lines in SGR colours, with box
/// drawing, blocks and an arrow.
pub const TEST_SCREEN: &str = "eggsh:/# echo EggOS\nEggOS\n\x1b[1;31mred \x1b[32mgreen\x1b[0m \
     \x1b[7minverse\x1b[27m \x1b[4;94munderlined\x1b[0m \x1b[38;5;208;48;5;17mcube\x1b[m\n\
     ┌──┐ █▓▒░ → ●\n└──┘\n";

/// The [`display::pixel_hash`] of a 1280x800 display after the desktop's terminal showed
/// [`TEST_SCREEN`] in a [`Session::SelfTest`].
pub const TEST_SCREEN_HASH_1280X800: u64 = 0x3832_d9da_93f3_eeef;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_test_screen_s_hash_is_the_one_the_qemu_self_test_checks() {
        let desktop = Desktop::new(1280, 800).unwrap();
        let area = desktop.text_area();
        let mut terminal = Terminal::new(area.columns, area.rows).unwrap();
        terminal.set_onlcr(true);
        terminal.feed(TEST_SCREEN.as_bytes());
        let mut pixels = vec![0; 1280 * 800];
        desktop.draw(&mut pixels, 1280, desktop.screen());
        terminal.render(&mut pixels, 1280, area.x, area.y, &mut Damage::new());
        assert_eq!(
            display::pixel_hash(&pixels),
            TEST_SCREEN_HASH_1280X800,
            "{:#x}",
            display::pixel_hash(&pixels)
        );
    }
}
