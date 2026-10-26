//! The graphical terminal (D114): the console's session shown on the display in the Egg Mono
//! font, by the terminal engine of [`eggos_drivers::terminal`], over the whole screen.
//!
//! Once [`start`] made it, every byte [`console::write`] sends to the serial console is fed to
//! the terminal as well, under the terminal's lock, so both see the same bytes in the same
//! order. Feeding only updates cells; the drawing thread ([`start_drawing`]) renders what changed
//! and flushes it to the display, at most once every [`FRAME_INTERVAL_MS`] and at once after an
//! idle frame, so a burst of output costs one flush. The display's flush, which waits for the
//! device, never holds the terminal's lock: a stalled device stops the picture, not the console.
//! Without a display nothing is made and a console write costs one load.

use crate::console;
use crate::display;
use crate::sync::LeafSpinLock;
use crate::thread::{self, SpawnError, WaitQueue};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};
use eggos_drivers::display::Damage;
use eggos_drivers::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use eggos_drivers::terminal::{REPLY_CAPACITY, TERM_BACKGROUND, Terminal, TextArea};
use eggos_hal::Platform;
use eggos_hal::time::mul_div_ceil;

pub use eggos_drivers::terminal::ViewMove;

/// The shortest time between two flushes of the terminal: 60 frames a second.
pub const FRAME_INTERVAL_MS: u64 = 16;

struct Screen {
    terminal: Terminal,
    area: TextArea,
}

static SCREEN: LeafSpinLock<Option<Box<Screen>>> = LeafSpinLock::new_leaf(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static RENDER_PENDING: AtomicBool = AtomicBool::new(false);
static APPLICATION_CURSOR_KEYS: AtomicBool = AtomicBool::new(false);
static DRAWER: WaitQueue = WaitQueue::new();

/// Whether the terminal is shown, so the console's output goes to it too.
#[inline(always)]
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Makes the terminal over the whole display, blank, shows it, and from then on mirrors the
/// console's output on it; returns its grid. `None`, and nothing made, without a display, when
/// not one cell fits, or without memory for it.
pub fn start<P: Platform>() -> Option<TextArea> {
    let mode = display::mode()?;
    let area = TextArea::full_screen(mode.width, mode.height)?;
    let mut terminal = Terminal::new(area.columns, area.rows)?;
    terminal.set_onlcr(true);
    let screen = Box::new(Screen { terminal, area });
    display::draw(|framebuffer| framebuffer.fill(mode.rect(), TERM_BACKGROUND))?;
    *SCREEN.lock() = Some(screen);
    crate::linux::terminal::set_window_size(
        area.rows,
        area.columns,
        GLYPH_WIDTH as u16,
        GLYPH_HEIGHT as u16,
    );
    ACTIVE.store(true, Ordering::Release);
    let _ = show::<P>();
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
#[cold]
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

fn request_render<P: Platform>() {
    if !RENDER_PENDING.swap(true, Ordering::AcqRel) {
        DRAWER.wake_one::<P>();
    }
}

/// Draws what changed on the terminal into the display's framebuffer and flushes it, then
/// hands the replies programs asked for to the console's input. Returns how many rectangles
/// the flush showed.
pub fn show<P: Platform>() -> Result<usize, &'static str> {
    display::draw(|framebuffer| {
        let stride = framebuffer.mode().width as usize;
        let mut damage = Damage::new();
        if let Some(screen) = SCREEN.lock().as_mut() {
            let (x, y) = (screen.area.x, screen.area.y);
            screen
                .terminal
                .render(framebuffer.pixels(), stride, x, y, &mut damage);
        }
        for &rect in damage.rects() {
            framebuffer.damage(rect);
        }
    });
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
    if !is_active() {
        return Ok(false);
    }
    thread::spawn::<P, _>(|| {
        let interval = mul_div_ceil(FRAME_INTERVAL_MS, P::clock_frequency(), 1000);
        let mut next_frame = 0;
        loop {
            DRAWER.wait_until::<P>(|| RENDER_PENDING.load(Ordering::Acquire));
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

/// The [`display::pixel_hash`] of a 1280x800 display after the terminal showed
/// [`TEST_SCREEN`].
pub const TEST_SCREEN_HASH_1280X800: u64 = 0x8f43_5a92_ff54_c00f;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_test_screen_s_hash_is_the_one_the_qemu_self_test_checks() {
        let area = TextArea::full_screen(1280, 800).unwrap();
        let mut terminal = Terminal::new(area.columns, area.rows).unwrap();
        terminal.set_onlcr(true);
        terminal.feed(TEST_SCREEN.as_bytes());
        let mut pixels = vec![TERM_BACKGROUND; 1280 * 800];
        terminal.render(&mut pixels, 1280, area.x, area.y, &mut Damage::new());
        assert_eq!(
            display::pixel_hash(&pixels),
            TEST_SCREEN_HASH_1280X800,
            "{:#x}",
            display::pixel_hash(&pixels)
        );
    }
}
