//! The offscreen display (D144): a framebuffer in memory, made at boot when the host asks for it
//! on the kernel command line (`eggos.display=offscreen[:<width>x<height>]`, which `run-eggos.sh
//! --mcp --no-window` passes) and no display device was found. The desktop draws into it as into
//! a screen, so apps open and agents capture and operate their windows with no screen at all;
//! its flush shows nothing.

use super::{Deadline, Mode, Rect, Scanout};
use crate::memory::MAX_ORDER;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_drivers::desktop::Desktop;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

pub use eggos_drivers::display::DEFAULT_MODE;

/// The kernel command line's parameter that asks for the offscreen display.
pub const PARAMETER: &[u8] = b"eggos.display=";

/// The parameter's value that asks for the offscreen display at [`DEFAULT_MODE`]; followed by
/// `:<width>x<height>` it names the size.
pub const OFFSCREEN: &[u8] = b"offscreen";

/// The most bytes the framebuffer may take: the largest block the kernel heap hands out, the
/// virtio GPU's framebuffer's ceiling too.
pub const MAX_BYTES: usize = (FRAME_SIZE as usize) << MAX_ORDER;

/// The most digits a width or a height may have.
pub const MAX_DIGITS: usize = 5;

/// What the console says before the mode of an offscreen display it made.
pub const LINE_PREFIX: &str = "boot: offscreen display ";

/// Why the offscreen display the command line asks for was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The value is not `offscreen` or `offscreen:<width>x<height>`.
    NotUnderstood,
    /// The framebuffer would take more than [`MAX_BYTES`].
    TooLarge,
    /// The desktop does not fit on a screen that size.
    TooSmall,
    /// No memory for the framebuffer.
    NoMemory,
    /// A display is already in use.
    DisplayInUse,
}

impl Refusal {
    /// What the console says about it: a fixed text, never the host's value.
    pub fn description(self) -> &'static str {
        match self {
            Refusal::NotUnderstood => {
                "not understood: the value is offscreen or offscreen:<width>x<height>"
            }
            Refusal::TooLarge => "the size is too large: at most 4 MiB of pixels",
            Refusal::TooSmall => "the size is too small for the desktop",
            Refusal::NoMemory => "no memory for the framebuffer",
            Refusal::DisplayInUse => "a display is already in use",
        }
    }
}

/// The mode of the offscreen display the kernel command line `line` asks for: the first word
/// starting with [`PARAMETER`] decides; `None` without one.
pub fn requested_mode(line: &[u8]) -> Option<Result<Mode, Refusal>> {
    let value = line
        .split(u8::is_ascii_whitespace)
        .find_map(|word| word.strip_prefix(PARAMETER))?;
    Some(mode_named(value))
}

fn mode_named(value: &[u8]) -> Result<Mode, Refusal> {
    let mode = match value.strip_prefix(OFFSCREEN) {
        Some(b"") => DEFAULT_MODE,
        Some(size) => {
            let size = size.strip_prefix(b":").ok_or(Refusal::NotUnderstood)?;
            let mut parts = size.split(|&byte| byte == b'x');
            let (Some(width), Some(height), None) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(Refusal::NotUnderstood);
            };
            Mode::new(number(width)?, number(height)?)
        }
        None => return Err(Refusal::NotUnderstood),
    };
    if mode.bytes() > MAX_BYTES {
        return Err(Refusal::TooLarge);
    }
    if !Desktop::fits(mode.width, mode.height) {
        return Err(Refusal::TooSmall);
    }
    Ok(mode)
}

fn number(digits: &[u8]) -> Result<u32, Refusal> {
    let well_formed = !digits.is_empty()
        && digits.len() <= MAX_DIGITS
        && digits.iter().all(u8::is_ascii_digit)
        && (digits.len() == 1 || digits.first() != Some(&b'0'));
    if !well_formed {
        return Err(Refusal::NotUnderstood);
    }
    Ok(digits
        .iter()
        .fold(0, |value, &digit| value * 10 + u32::from(digit - b'0')))
}

/// A framebuffer in memory, shown nowhere.
pub struct OffscreenScanout {
    pixels: Box<[u32]>,
    mode: Mode,
}

impl OffscreenScanout {
    /// A black framebuffer of `mode`; [`Refusal::NoMemory`] when the heap has no room for it.
    pub fn new(mode: Mode) -> Result<Self, Refusal> {
        let count = (mode.width as usize)
            .checked_mul(mode.height as usize)
            .ok_or(Refusal::NoMemory)?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| Refusal::NoMemory)?;
        pixels.resize(count, 0);
        Ok(Self {
            pixels: pixels.into_boxed_slice(),
            mode,
        })
    }
}

impl Scanout for OffscreenScanout {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn pixels(&mut self) -> &mut [u32] {
        &mut self.pixels
    }

    fn flush(&mut self, rects: &[Rect], _deadline: &mut Deadline) -> Result<usize, &'static str> {
        Ok(rects.len())
    }
}

impl core::fmt::Debug for OffscreenScanout {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OffscreenScanout")
            .field("mode", &self.mode)
            .finish()
    }
}

/// The offscreen display the kernel command line `line` asks for, made but not installed;
/// `None` when it asks for none.
pub fn requested_scanout(line: &[u8]) -> Option<Result<OffscreenScanout, Refusal>> {
    requested_mode(line).map(|mode| mode.and_then(OffscreenScanout::new))
}

/// What installing `scanout` came to: its mode, or [`Refusal::DisplayInUse`] when
/// [`super::install`] handed it back.
pub fn installed(
    scanout: OffscreenScanout,
    install: impl FnOnce(OffscreenScanout) -> Result<(), OffscreenScanout>,
) -> Result<Mode, Refusal> {
    let mode = scanout.mode;
    install(scanout)
        .map(|()| mode)
        .map_err(|_| Refusal::DisplayInUse)
}

/// Makes the offscreen display the kernel command line asks for the kernel's display, and
/// returns its mode or why it was not made; `None` when the line asks for none, so nothing is
/// allocated.
pub fn install_requested<P: Platform>() -> Option<Result<Mode, Refusal>> {
    let mut outcome = None;
    P::command_line(&mut |line| outcome = requested_scanout(line));
    outcome.map(|scanout| scanout.and_then(|scanout| installed(scanout, super::install)))
}

/// Writes what [`install_requested`] came to on `console`: the display's mode, or why it was
/// not made; nothing for `None`.
pub fn report<W: Write>(
    console: &mut W,
    outcome: Option<Result<Mode, Refusal>>,
) -> core::fmt::Result {
    match outcome {
        None => Ok(()),
        Some(Ok(mode)) => writeln!(console, "{LINE_PREFIX}{}x{}", mode.width, mode.height),
        Some(Err(refusal)) => writeln!(
            console,
            "boot: eggos.display refused: {}",
            refusal.description()
        ),
    }
}

#[cfg(test)]
mod tests;
