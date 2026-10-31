//! The display (D112): the framebuffer a display driver set up, which the kernel draws into and
//! shows. The virtio GPU's driver installs it ([`crate::devices::virtio`], feature
//! `virtio-gpu`), or, with no display device and `eggos.display=offscreen` on the kernel command
//! line, the kernel makes one in memory ([`offscreen`], D144); the graphical terminal and the
//! desktop (v0.3.0 items 13 and 14) draw through it.
//!
//! [`draw`] lends the [`Framebuffer`] under the display's lock; what it changes is recorded as
//! damage ([`Framebuffer::damage`], [`Framebuffer::fill`]), and [`flush`] shows the damage and
//! nothing else. Nothing reaches the device while nothing is damaged.
//!
//! The first frame is the boot screen (D186), whose progress lights as the boot passes its
//! stages ([`light_boot_stages`]) until the desktop replaces it ([`Framebuffer::end_boot_screen`]).
//! A display that draws the pointer itself, a GPU's cursor, shows the desktop's pointer through
//! [`show_pointer`], so moving it redraws nothing.
//!
//! At scale 2 (`display.scale`, D195, set at boot before the desktop starts: [`set_scale`]) the
//! kernel and every program draw into a framebuffer of half the device's width and height, and
//! a flush doubles each damaged pixel onto the device, so the desktop, its apps and the terminal
//! show twice as large, as every 2x rule in the designs asks.

use crate::devices::Deadline;
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;
use eggos_drivers::desktop::boot_screen::BootScreen;
use eggos_drivers::desktop::protocol::PointerShape;
use eggos_drivers::desktop::write_cursor_image;
pub use eggos_drivers::display::{Damage, Mode, Rect};
use eggos_hal::Platform;

pub mod offscreen;

/// The boot screen's background, `0x00RRGGBB`: the design's `wallpaper.top` (docs/ux/
/// desktop-v0.3.md, sections 1 and 2), and the whole first frame of a display too small for the
/// boot screen.
pub const FIRST_FRAME_COLOUR: u32 = 0x000e_1726;

/// The [`pixel_hash`] of a 1280x800 frame ([`offscreen::DEFAULT_MODE`], the mode xtask asks QEMU
/// for) all in [`FIRST_FRAME_COLOUR`].
pub const FIRST_FRAME_HASH_1280X800: u64 = 0x1125_09e9_e81d_2325;

/// The boot screen's stage lit once the input devices were looked for, found or not.
pub const BOOT_STAGE_INPUT: u8 = 2;

/// The boot screen's stage lit once init is starting.
pub const BOOT_STAGE_INIT: u8 = 3;

/// Where [`pixel_hash`] starts: FNV-1a's offset basis.
pub const PIXEL_HASH_START: u64 = 0xcbf2_9ce4_8422_2325;

/// How long a flush polls the device before giving up.
pub const FLUSH_TIMEOUT_MS: u64 = 1000;

/// What a display driver lends the kernel: a framebuffer shown on a display output.
pub trait Scanout: Send {
    /// The framebuffer's mode.
    fn mode(&self) -> Mode;
    /// The framebuffer's pixels: `mode().width` per row, row after row, each `0x00RRGGBB`.
    fn pixels(&mut self) -> &mut [u32];
    /// Shows the pixels inside `rects` on the display, polling the device until `deadline`, and
    /// returns how many rectangles were shown.
    fn flush(&mut self, rects: &[Rect], deadline: &mut Deadline) -> Result<usize, &'static str>;
    /// Whether the display draws a pointer over the framebuffer itself, from the image
    /// [`Self::pointer_image`] lends: a GPU's cursor. No by default.
    fn draws_pointer(&self) -> bool {
        false
    }
    /// The pointer's image, [`eggos_drivers::desktop::CURSOR_SIDE`] rows of as many
    /// `0xAARRGGBB` pixels; `None` when the display draws no pointer.
    fn pointer_image(&mut self) -> Option<&mut [u32]> {
        None
    }
    /// Shows the pointer's image as it is now, with the pixel `hotspot` of it at `at`.
    fn show_pointer(
        &mut self,
        _hotspot: (u32, u32),
        _at: (u32, u32),
        _deadline: &mut Deadline,
    ) -> Result<(), &'static str> {
        Err("the display draws no pointer")
    }
    /// Moves the shown pointer's hotspot to `at`.
    fn move_pointer(
        &mut self,
        _at: (u32, u32),
        _deadline: &mut Deadline,
    ) -> Result<(), &'static str> {
        Err("the display draws no pointer")
    }
    /// Hides the pointer.
    fn hide_pointer(&mut self, _deadline: &mut Deadline) -> Result<(), &'static str> {
        Err("the display draws no pointer")
    }
}

/// A display: a driver's [`Scanout`], the damage drawn on it since its last flush, the boot
/// screen while it shows, and the pointer it draws, if it draws one.
pub struct Display {
    scanout: Box<dyn Scanout>,
    damage: Damage,
    flushes: u64,
    boot: Option<BootScreen>,
    pointer: Option<(u32, u32, PointerShape)>,
    scale: u32,
    logical: Vec<u32>,
}

/// The largest scale a display takes.
pub const MAX_SCALE: u32 = 2;

impl Display {
    /// The display `scanout` shows, nothing damaged yet.
    pub fn new(scanout: Box<dyn Scanout>) -> Self {
        Self {
            scanout,
            damage: Damage::new(),
            flushes: 0,
            boot: None,
            pointer: None,
            scale: 1,
            logical: Vec::new(),
        }
    }

    /// Makes the display show at `scale` (1 or 2), drawn at its device's size divided by it;
    /// whether it could (the device's size must divide, and memory for the smaller
    /// framebuffer be found). The framebuffer is new and blank: call it before the desktop
    /// starts.
    pub fn set_scale(&mut self, scale: u32) -> bool {
        let device = self.scanout.mode();
        if scale == 1 {
            self.scale = 1;
            self.logical = Vec::new();
            return true;
        }
        if scale != MAX_SCALE || device.width % scale != 0 || device.height % scale != 0 {
            return false;
        }
        let pixels = (device.width / scale) as usize * (device.height / scale) as usize;
        let mut logical = Vec::new();
        if logical.try_reserve_exact(pixels).is_err() {
            return false;
        }
        logical.resize(pixels, 0);
        self.logical = logical;
        self.scale = scale;
        self.boot = None;
        self.damage.add(self.mode().rect());
        true
    }

    /// The scale the display shows at.
    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// Whether the display draws the pointer itself ([`Scanout::draws_pointer`]).
    pub fn draws_pointer(&self) -> bool {
        self.scanout.draws_pointer()
    }

    /// Has the display show the pointer `wanted`, its hotspot and shape, or none: nothing when it
    /// shows that already or draws no pointer; a new image only when the shape changed, else a
    /// move.
    pub fn show_pointer(
        &mut self,
        wanted: Option<(u32, u32, PointerShape)>,
        deadline: &mut Deadline,
    ) -> Result<(), &'static str> {
        if wanted == self.pointer || !self.scanout.draws_pointer() {
            return Ok(());
        }
        let shown = self.pointer.map(|(_, _, shape)| shape);
        match wanted {
            None | Some((_, _, PointerShape::Hidden)) => {
                if shown.is_some_and(|shape| shape != PointerShape::Hidden) {
                    self.scanout.hide_pointer(deadline)?;
                }
            }
            Some((x, y, shape)) if shown == Some(shape) => {
                self.scanout
                    .move_pointer((x * self.scale, y * self.scale), deadline)?;
            }
            Some((x, y, shape)) => {
                let image = self
                    .scanout
                    .pointer_image()
                    .ok_or("the display draws no pointer")?;
                let hotspot =
                    write_cursor_image(shape, image).ok_or("the pointer's image is too small")?;
                self.scanout
                    .show_pointer(hotspot, (x * self.scale, y * self.scale), deadline)?;
            }
        }
        self.pointer = wanted;
        Ok(())
    }

    /// The [`pixel_hash`] of the boot screen as it shows now, computed a row at a time; `None`
    /// once the desktop replaced it, or without memory for a row.
    pub fn boot_screen_hash(&self) -> Option<u64> {
        let screen = self.boot?;
        let mode = self.mode();
        let mut row = Vec::new();
        row.try_reserve_exact(mode.width as usize).ok()?;
        row.resize(mode.width as usize, 0);
        let mut hash = PIXEL_HASH_START;
        for y in 0..mode.height {
            screen.draw_span(y, 0, &mut row);
            hash = continue_pixel_hash(hash, &row);
        }
        Some(hash)
    }

    /// The framebuffer's mode, as the kernel and programs draw in it: the device's divided by
    /// the scale.
    pub fn mode(&self) -> Mode {
        let device = self.scanout.mode();
        Mode::new(device.width / self.scale, device.height / self.scale)
    }

    /// How many flushes have shown something.
    pub fn flushes(&self) -> u64 {
        self.flushes
    }

    /// Lends `draw` the framebuffer and returns what it returns.
    pub fn draw<R>(&mut self, draw: impl FnOnce(&mut Framebuffer<'_>) -> R) -> R {
        let mode = self.mode();
        let pixels = match self.scale {
            1 => self.scanout.pixels(),
            _ => &mut self.logical[..],
        };
        let mut framebuffer = Framebuffer {
            pixels,
            mode,
            damage: &mut self.damage,
            boot: &mut self.boot,
        };
        draw(&mut framebuffer)
    }

    /// Shows what was drawn since the last flush, polling the device until `deadline`, and
    /// returns how many rectangles that took: 0 when nothing was drawn. When the device fails,
    /// the damage stays for the next flush.
    pub fn flush(&mut self, deadline: &mut Deadline) -> Result<usize, &'static str> {
        if self.damage.is_empty() {
            return Ok(0);
        }
        if self.scale > 1 {
            return self.flush_scaled(deadline);
        }
        let shown = self.scanout.flush(self.damage.rects(), deadline)?;
        self.damage.clear();
        self.flushes += 1;
        Ok(shown)
    }
}

impl Display {
    fn flush_scaled(&mut self, deadline: &mut Deadline) -> Result<usize, &'static str> {
        let scale = self.scale;
        let logical_width = self.mode().width as usize;
        let device_width = self.scanout.mode().width as usize;
        let mut device_damage = Damage::new();
        let device = self.scanout.pixels();
        for rect in self.damage.rects() {
            double_into(
                &self.logical,
                logical_width,
                device,
                device_width,
                *rect,
                scale,
            );
            device_damage.add(Rect::new(
                rect.x * scale,
                rect.y * scale,
                rect.width * scale,
                rect.height * scale,
            ));
        }
        let shown = self.scanout.flush(device_damage.rects(), deadline)?;
        self.damage.clear();
        self.flushes += 1;
        Ok(shown)
    }
}

/// Copies `rect` of `logical` (rows of `logical_width`) into `device` (rows of
/// `device_width`), each pixel as a `scale` by `scale` square.
pub fn double_into(
    logical: &[u32],
    logical_width: usize,
    device: &mut [u32],
    device_width: usize,
    rect: Rect,
    scale: u32,
) {
    let scale = scale as usize;
    for y in rect.y as usize..rect.bottom() as usize {
        let Some(source) = logical
            .get(y * logical_width + rect.x as usize..y * logical_width + rect.right() as usize)
        else {
            continue;
        };
        for row in 0..scale {
            let start = (y * scale + row) * device_width + rect.x as usize * scale;
            let Some(target) = device.get_mut(start..start + source.len() * scale) else {
                continue;
            };
            for (pair, &pixel) in target.chunks_exact_mut(scale).zip(source) {
                pair.fill(pixel);
            }
        }
    }
}

impl core::fmt::Debug for Display {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Display")
            .field("mode", &self.mode())
            .field("damage", &self.damage)
            .field("flushes", &self.flushes)
            .finish()
    }
}

static DISPLAY: LeafSpinLock<Option<Box<Display>>> = LeafSpinLock::new_leaf(None);

/// Makes `scanout` the kernel's display. Refused, and handed back, when there is one already.
pub fn install<S: Scanout + 'static>(scanout: S) -> Result<(), S> {
    let mut display = DISPLAY.lock();
    if display.is_some() {
        return Err(scanout);
    }
    *display = Some(Box::new(Display::new(Box::new(scanout))));
    Ok(())
}

/// Makes the display show at `scale` ([`Display::set_scale`]); whether it could.
pub fn set_scale(scale: u32) -> bool {
    DISPLAY
        .lock()
        .as_mut()
        .is_some_and(|display| display.set_scale(scale))
}

/// The scale the display shows at; 1 without a display.
pub fn scale() -> u32 {
    DISPLAY.lock().as_ref().map_or(1, |display| display.scale())
}

/// The display's mode; `None` without a display.
pub fn mode() -> Option<Mode> {
    DISPLAY.lock().as_ref().map(|display| display.mode())
}

/// How many flushes have shown something on the display.
pub fn flushes() -> u64 {
    DISPLAY
        .lock()
        .as_ref()
        .map_or(0, |display| display.flushes())
}

/// Lends `draw` the display's framebuffer and returns what it returns; `None` without a
/// display.
pub fn draw<R>(draw: impl FnOnce(&mut Framebuffer<'_>) -> R) -> Option<R> {
    DISPLAY.lock().as_mut().map(|display| display.draw(draw))
}

/// Shows what was drawn on the display since the last flush ([`Display::flush`]); 0 without a
/// display.
pub fn flush<P: Platform>() -> Result<usize, &'static str> {
    let mut display = DISPLAY.lock();
    let mut deadline = Deadline::after::<P>(FLUSH_TIMEOUT_MS);
    display
        .as_mut()
        .map_or(Ok(0), |display| display.flush(&mut deadline))
}

/// Draws the first frame, the boot screen with its first stage lit ([`BootScreen`]), or the
/// whole screen in [`FIRST_FRAME_COLOUR`] on a display too small for it, and shows it. Returns the
/// display's mode; `None` without a display.
pub fn show_first_frame<P: Platform>() -> Result<Option<Mode>, &'static str> {
    let Some(mode) = draw(|framebuffer| {
        framebuffer.show_boot_screen();
        framebuffer.mode()
    }) else {
        return Ok(None);
    };
    flush::<P>().map(|_| Some(mode))
}

/// Lights the boot screen's first `lit` stages and shows the segments that changed; nothing
/// without a display or once the desktop replaced the boot screen.
pub fn light_boot_stages<P: Platform>(lit: u8) -> Result<(), &'static str> {
    if draw(|framebuffer| framebuffer.light_boot_stages(lit)).is_none() {
        return Ok(());
    }
    flush::<P>().map(|_| ())
}

/// The [`pixel_hash`] the display would have if it showed the boot screen as it is now
/// ([`Display::boot_screen_hash`]); `None` without a display or once the desktop replaced it.
pub fn boot_screen_hash() -> Option<u64> {
    DISPLAY.lock().as_ref()?.boot_screen_hash()
}

/// Whether the display draws the pointer itself ([`Scanout::draws_pointer`]).
pub fn draws_pointer() -> bool {
    DISPLAY
        .lock()
        .as_ref()
        .is_some_and(|display| display.draws_pointer())
}

/// Has the display show the pointer `wanted` ([`Display::show_pointer`]); nothing without a
/// display.
pub fn show_pointer<P: Platform>(
    wanted: Option<(u32, u32, PointerShape)>,
) -> Result<(), &'static str> {
    let mut display = DISPLAY.lock();
    let mut deadline = Deadline::after::<P>(FLUSH_TIMEOUT_MS);
    display.as_mut().map_or(Ok(()), |display| {
        display.show_pointer(wanted, &mut deadline)
    })
}

/// The display's framebuffer, lent by [`draw`].
pub struct Framebuffer<'a> {
    pixels: &'a mut [u32],
    mode: Mode,
    damage: &'a mut Damage,
    boot: &'a mut Option<BootScreen>,
}

impl Framebuffer<'_> {
    /// Its mode: `mode().width` pixels per row.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The pixels, row after row, each `0x00RRGGBB`. What is written here shows only once
    /// marked with [`Self::damage`] and flushed.
    pub fn pixels(&mut self) -> &mut [u32] {
        self.pixels
    }

    /// Records that the pixels inside `rect` changed.
    pub fn damage(&mut self, rect: Rect) {
        self.damage
            .add(rect.clipped(self.mode.width, self.mode.height));
    }

    /// Fills the part of `rect` on the screen with `colour` and records it as damaged.
    pub fn fill(&mut self, rect: Rect, colour: u32) {
        let rect = rect.clipped(self.mode.width, self.mode.height);
        let width = self.mode.width as usize;
        for row in rect.y..rect.bottom() {
            let start = row as usize * width + rect.x as usize;
            if let Some(pixels) = self.pixels.get_mut(start..start + rect.width as usize) {
                pixels.fill(colour);
            }
        }
        self.damage.add(rect);
    }

    /// Makes the boot screen of the framebuffer's size, its first stage lit, what it shows, and
    /// draws it all; fills it with [`FIRST_FRAME_COLOUR`] when it is too small for one.
    pub fn show_boot_screen(&mut self) {
        *self.boot = BootScreen::new(self.mode.width, self.mode.height);
        self.redraw_boot_screen(self.mode.rect());
    }

    /// Draws the part of the boot screen inside `rect` again, or fills it with
    /// [`FIRST_FRAME_COLOUR`] when no boot screen shows, and records it as damaged.
    pub fn redraw_boot_screen(&mut self, rect: Rect) {
        let Some(screen) = *self.boot else {
            self.fill(rect, FIRST_FRAME_COLOUR);
            return;
        };
        screen.draw(self.pixels, self.mode.width as usize, rect);
        self.damage(rect);
    }

    /// Lights the boot screen's first `lit` stages and redraws the segments that changed;
    /// nothing when no boot screen shows.
    pub fn light_boot_stages(&mut self, lit: u8) {
        if let Some(changed) = self.boot.as_mut().and_then(|screen| screen.light(lit)) {
            self.redraw_boot_screen(changed);
        }
    }

    /// Whether the boot screen still shows.
    pub fn boot_screen_shown(&self) -> bool {
        self.boot.is_some()
    }

    /// The desktop or the console replaced the boot screen: from now on nothing lights it.
    pub fn end_boot_screen(&mut self) {
        *self.boot = None;
    }
}

impl core::fmt::Debug for Framebuffer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Framebuffer")
            .field("mode", &self.mode)
            .field("damage", &self.damage)
            .finish()
    }
}

/// FNV-1a, 64 bits, over the pixels' little-endian bytes: the hash the display tests compare a
/// frame by (docs/ux/desktop-v0.3.md, section 8).
pub fn pixel_hash(pixels: &[u32]) -> u64 {
    continue_pixel_hash(PIXEL_HASH_START, pixels)
}

/// [`pixel_hash`] of the pixels hashed into `hash` followed by `pixels`: a frame hashed a row at
/// a time.
pub fn continue_pixel_hash(hash: u64, pixels: &[u32]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    pixels
        .iter()
        .flat_map(|pixel| pixel.to_le_bytes())
        .fold(hash, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
}

#[cfg(test)]
mod tests;
