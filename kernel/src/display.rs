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
use crate::devices::interrupts::DeviceEvents;
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;
use eggos_drivers::desktop::boot_screen::BootScreen;
use eggos_drivers::desktop::protocol::PointerShape;
use eggos_drivers::desktop::write_cursor_image;
pub use eggos_drivers::display::{Damage, Mode, Rect};
use eggos_hal::Platform;

pub mod boot_framebuffer;
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

const RENDER_TIMEOUT_MS: u64 = 5000;

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
    /// Whether [`Self::change_mode`] can work (D287): no for a firmware's framebuffer.
    fn can_change_mode(&self) -> bool {
        false
    }
    /// The size the display itself prefers, asked again when it says it changed; `None` when
    /// it has no say.
    fn preferred_mode(&mut self, _deadline: &mut Deadline) -> Option<Mode> {
        None
    }
    /// Shows a new, blank framebuffer of `mode` in place of the one shown.
    fn change_mode(&mut self, _mode: Mode, _deadline: &mut Deadline) -> Result<(), &'static str> {
        Err("this display keeps the size the firmware set")
    }
    /// The events the device's interrupt raises, when it has one connected (D414); a display
    /// without them flushes by polling.
    fn flush_events(&self) -> Option<&'static DeviceEvents> {
        None
    }
    /// Posts the flush of `rects` without waiting ([`Self::finish_flush`] collects it): how
    /// many rectangles it posted. Not supported by default.
    fn start_flush(&mut self, _rects: &[Rect]) -> Result<usize, &'static str> {
        Err("this display flushes only by waiting")
    }
    /// The count of a started flush once the device answered it, `None` while it has not.
    fn finish_flush(&mut self) -> Result<Option<usize>, &'static str> {
        Ok(Some(0))
    }
    /// Gives up on a started flush the device never answered.
    fn abandon_flush(&mut self) {}
    /// The device's 3D commands, when it takes them (D415).
    fn renderer(&mut self) -> Option<&mut dyn crate::rendering::Renderer> {
        None
    }
}

/// What [`Display::start_flush`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushStart {
    /// The flush is done (or there was nothing to show): this many rectangles.
    Done(usize),
    /// The device has the flush; wait for its interrupt, then [`Display::finish_flush`].
    Posted(&'static DeviceEvents),
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
    rotation: Rotation,
    logical: Vec<u32>,
    in_flight: Damage,
    posted: bool,
}

/// How far the picture turns clockwise on the device (`display.rotation`, D330): the desktop
/// draws upright at its own size, the device's with width and height swapped for a quarter or
/// three quarters, and each flush turns what changed onto the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    Upright,
    Quarter,
    Half,
    ThreeQuarters,
}

impl Rotation {
    /// The rotation of `degrees` (0, 90, 180 or 270).
    pub const fn of_degrees(degrees: u32) -> Option<Self> {
        match degrees {
            0 => Some(Self::Upright),
            90 => Some(Self::Quarter),
            180 => Some(Self::Half),
            270 => Some(Self::ThreeQuarters),
            _ => None,
        }
    }

    /// Whether it swaps width and height.
    pub const fn swaps(self) -> bool {
        matches!(self, Self::Quarter | Self::ThreeQuarters)
    }

    /// The point (`x`, `y`) of an upright picture `width` by `height` once turned: where it
    /// shows on the device.
    pub const fn turn(self, x: u32, y: u32, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Upright => (x, y),
            Self::Quarter => (height - 1 - y, x),
            Self::Half => (width - 1 - x, height - 1 - y),
            Self::ThreeQuarters => (y, width - 1 - x),
        }
    }

    /// The point of the upright picture `width` by `height` that shows at device point
    /// (`x`, `y`): [`Self::turn`] undone.
    pub const fn unturn(self, x: u32, y: u32, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Upright => (x, y),
            Self::Quarter => (y, height - 1 - x),
            Self::Half => (width - 1 - x, height - 1 - y),
            Self::ThreeQuarters => (width - 1 - y, x),
        }
    }

    /// A movement (`dx`, `dy`) on the device as it reads in the upright picture.
    pub const fn unturn_delta(self, dx: i32, dy: i32) -> (i32, i32) {
        match self {
            Self::Upright => (dx, dy),
            Self::Quarter => (dy, -dx),
            Self::Half => (-dx, -dy),
            Self::ThreeQuarters => (-dy, dx),
        }
    }

    /// `rect` of the upright picture `width` by `height` once turned.
    pub fn turn_rect(self, rect: Rect, width: u32, height: u32) -> Rect {
        let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
        match self {
            Self::Upright => rect,
            Self::Quarter => Rect::new(height - (y + h), x, h, w),
            Self::Half => Rect::new(width - (x + w), height - (y + h), w, h),
            Self::ThreeQuarters => Rect::new(y, width - (x + w), h, w),
        }
    }
}

/// The largest scale a display takes.
pub const MAX_SCALE: u32 = 2;

/// The smallest size [`Display::change_mode`] sets: the desktop's smallest screen.
pub const MIN_MODE: Mode = Mode::new(1024, 768);

/// The most bytes [`Display::change_mode`] gives a framebuffer: 3840x2160 at 4 bytes a pixel.
pub const MAX_MODE_BYTES: usize = 3840 * 2160 * 4;

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
            rotation: Rotation::Upright,
            logical: Vec::new(),
            in_flight: Damage::new(),
            posted: false,
        }
    }

    /// Starts showing what was drawn (D414): on a device with its interrupt connected, upright
    /// and unscaled, the damage moves to the flush in flight and the device gets it without the
    /// caller waiting; otherwise the flush runs to its end here.
    pub fn start_flush(&mut self, deadline: &mut Deadline) -> Result<FlushStart, &'static str> {
        self.settle(deadline)?;
        let events = self.scanout.flush_events();
        let Some(events) = events.filter(|_| !self.draws_off_device()) else {
            return self.flush(deadline).map(FlushStart::Done);
        };
        if self.damage.is_empty() {
            return Ok(FlushStart::Done(0));
        }
        match self.scanout.start_flush(self.damage.rects()) {
            Ok(0) => {
                self.damage.clear();
                Ok(FlushStart::Done(0))
            }
            Ok(_) => {
                self.in_flight = core::mem::replace(&mut self.damage, Damage::new());
                self.posted = true;
                Ok(FlushStart::Posted(events))
            }
            Err(_) => self.flush(deadline).map(FlushStart::Done),
        }
    }

    /// Collects a posted flush: `Some` with its count once the device answered. A failed flush
    /// puts its damage back for the next one.
    pub fn finish_flush(&mut self) -> Result<Option<usize>, &'static str> {
        if !self.posted {
            return Ok(Some(0));
        }
        match self.scanout.finish_flush() {
            Ok(None) => Ok(None),
            Ok(Some(shown)) => {
                self.posted = false;
                self.in_flight.clear();
                self.flushes += 1;
                Ok(Some(shown))
            }
            Err(error) => {
                self.posted = false;
                for rect in self.in_flight.rects().to_vec() {
                    self.damage.add(rect);
                }
                self.in_flight.clear();
                Err(error)
            }
        }
    }

    /// Waits, polling, for a posted flush before anything else uses the device; gives it up at
    /// `deadline`.
    fn settle(&mut self, deadline: &mut Deadline) -> Result<(), &'static str> {
        self.settle_renderer(deadline)?;
        while self.posted {
            if self.finish_flush()?.is_some() {
                return Ok(());
            }
            if !deadline.keep_waiting() {
                self.scanout.abandon_flush();
                self.posted = false;
                for rect in self.in_flight.rects().to_vec() {
                    self.damage.add(rect);
                }
                self.in_flight.clear();
                return Err("the display did not answer a flush");
            }
        }
        Ok(())
    }

    /// Waits, polling, for a posted 3D command (D438) before anything else uses the device;
    /// gives it up at `deadline`.
    fn settle_renderer(&mut self, deadline: &mut Deadline) -> Result<(), &'static str> {
        let Some(renderer) = self.scanout.renderer() else {
            return Ok(());
        };
        while !renderer.finish()? {
            if !deadline.keep_waiting() {
                renderer.abandon();
                return Err("the display did not answer a 3D command");
            }
        }
        Ok(())
    }

    /// Whether the desktop draws into the display's own buffer, which flushes scale or turn.
    fn draws_off_device(&self) -> bool {
        self.scale > 1 || self.rotation != Rotation::Upright
    }

    /// Gives the buffer the desktop draws in for the scale and rotation in force: the device's
    /// own when neither changes the picture. `false` without memory for it.
    fn reallocate_logical(&mut self) -> bool {
        if !self.draws_off_device() {
            self.logical = Vec::new();
            return true;
        }
        let mode = self.mode();
        let pixels = mode.width as usize * mode.height as usize;
        let mut logical = Vec::new();
        if logical.try_reserve_exact(pixels).is_err() {
            return false;
        }
        logical.resize(pixels, 0);
        self.logical = logical;
        self.boot = None;
        self.damage.add(mode.rect());
        true
    }

    /// Turns the picture `rotation` clockwise (D330): the framebuffer is new and blank, at the
    /// size the turn gives; the caller resizes the desktop to [`Self::mode`]. `false` without
    /// memory for it, the rotation unchanged.
    pub fn set_rotation(&mut self, rotation: Rotation) -> bool {
        let previous = self.rotation;
        self.rotation = rotation;
        if !self.reallocate_logical() {
            self.rotation = previous;
            return false;
        }
        true
    }

    /// The rotation in force.
    pub fn rotation(&self) -> Rotation {
        self.rotation
    }

    /// Makes the display show at `scale` (1 or 2), drawn at its device's size divided by it;
    /// whether it could (the device's size must divide, and memory for the smaller
    /// framebuffer be found). The framebuffer is new and blank: call it before the desktop
    /// starts.
    pub fn set_scale(&mut self, scale: u32) -> bool {
        let device = self.scanout.mode();
        if scale != 1
            && (scale != MAX_SCALE
                || !device.width.is_multiple_of(scale)
                || !device.height.is_multiple_of(scale))
        {
            return false;
        }
        let previous = self.scale;
        self.scale = scale;
        if !self.reallocate_logical() {
            self.scale = previous;
            return false;
        }
        true
    }

    /// The scale the display shows at.
    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// Makes the device show `mode` (D287), before the desktop starts: the framebuffer is new and
    /// blank, at the scale in force when `mode` divides by it, else at scale 1; the boot screen
    /// is drawn again if it showed. Refused when the device cannot change or `mode` is outside
    /// [`MIN_MODE`] and [`MAX_MODE_BYTES`].
    pub fn change_mode(&mut self, mode: Mode, deadline: &mut Deadline) -> Result<(), &'static str> {
        if !self.scanout.can_change_mode() {
            return Err("this display keeps the size the firmware set");
        }
        if mode.width < MIN_MODE.width || mode.height < MIN_MODE.height || mode.bytes() > MAX_MODE_BYTES {
            return Err("a size this display does not take");
        }
        self.scanout.change_mode(mode, deadline)?;
        let booting = self.boot.is_some();
        let scale = self.scale;
        self.scale = 1;
        if scale > 1 && eggos_drivers::desktop::Desktop::fits(mode.width / scale, mode.height / scale) {
            self.scale = scale;
        }
        if !self.reallocate_logical() {
            self.scale = 1;
            self.rotation = Rotation::Upright;
            self.logical = Vec::new();
        }
        self.damage.add(self.mode().rect());
        if booting {
            self.draw(|framebuffer| framebuffer.show_boot_screen());
        }
        Ok(())
    }

    /// Whether the device can change size ([`Scanout::can_change_mode`]).
    pub fn can_change_mode(&self) -> bool {
        self.scanout.can_change_mode()
    }

    /// The size the device prefers ([`Scanout::preferred_mode`]).
    pub fn preferred_mode(&mut self, deadline: &mut Deadline) -> Option<Mode> {
        self.scanout.preferred_mode(deadline)
    }

    /// The device's own size, before the scale divides it.
    pub fn device_mode(&self) -> Mode {
        self.scanout.mode()
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
                let at = self.device_point(x, y);
                self.scanout.move_pointer(at, deadline)?;
            }
            Some((x, y, shape)) => {
                let image = self
                    .scanout
                    .pointer_image()
                    .ok_or("the display draws no pointer")?;
                let hotspot =
                    write_cursor_image(shape, image).ok_or("the pointer's image is too small")?;
                let at = self.device_point(x, y);
                self.scanout.show_pointer(hotspot, at, deadline)?;
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
    /// the scale, width and height swapped when the rotation turns a quarter.
    pub fn mode(&self) -> Mode {
        let device = self.scanout.mode();
        let (width, height) = (device.width / self.scale, device.height / self.scale);
        match self.rotation.swaps() {
            true => Mode::new(height, width),
            false => Mode::new(width, height),
        }
    }

    /// Where the drawn point (`x`, `y`) shows on the device.
    fn device_point(&self, x: u32, y: u32) -> (u32, u32) {
        let mode = self.mode();
        let (x, y) = self.rotation.turn(x.min(mode.width - 1), y.min(mode.height - 1), mode.width, mode.height);
        (x * self.scale, y * self.scale)
    }

    /// How many flushes have shown something.
    pub fn flushes(&self) -> u64 {
        self.flushes
    }

    /// Lends `draw` the framebuffer and returns what it returns.
    pub fn draw<R>(&mut self, draw: impl FnOnce(&mut Framebuffer<'_>) -> R) -> R {
        let mode = self.mode();
        let pixels = match self.draws_off_device() {
            false => self.scanout.pixels(),
            true => &mut self.logical[..],
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
        if self.posted {
            self.settle(deadline)?;
        }
        if self.damage.is_empty() {
            return Ok(0);
        }
        if self.rotation != Rotation::Upright {
            return self.flush_turned(deadline);
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
    /// Copies what changed onto the device turned by the rotation, each pixel a square of the
    /// scale, then flushes the device's rectangles.
    fn flush_turned(&mut self, deadline: &mut Deadline) -> Result<usize, &'static str> {
        let scale = self.scale;
        let rotation = self.rotation;
        let mode = self.mode();
        let device_width = self.scanout.mode().width as usize;
        let mut device_damage = Damage::new();
        let device = self.scanout.pixels();
        for rect in self.damage.rects() {
            for y in rect.y..rect.bottom().min(mode.height) {
                for x in rect.x..rect.right().min(mode.width) {
                    let pixel = self.logical[(y * mode.width + x) as usize];
                    let (turned_x, turned_y) = rotation.turn(x, y, mode.width, mode.height);
                    for row in 0..scale {
                        let start = (turned_y * scale + row) as usize * device_width + (turned_x * scale) as usize;
                        if let Some(target) = device.get_mut(start..start + scale as usize) {
                            target.fill(pixel);
                        }
                    }
                }
            }
            let turned = rotation.turn_rect(*rect, mode.width, mode.height);
            device_damage.add(Rect::new(
                turned.x * scale,
                turned.y * scale,
                turned.width * scale,
                turned.height * scale,
            ));
        }
        let shown = self.scanout.flush(device_damage.rects(), deadline)?;
        self.damage.clear();
        self.flushes += 1;
        Ok(shown)
    }

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

/// Whether the display follows the size the device asks for (a host window resized), which it
/// does unless a size is chosen in Settings (D311).
static FOLLOWS_DEVICE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Makes the display follow the device's size, or keep the one chosen.
pub fn set_follows_device(follows: bool) {
    FOLLOWS_DEVICE.store(follows, core::sync::atomic::Ordering::Relaxed);
}

/// When the device asked for another size since the last look (virtio-gpu's display event)
/// and no size is chosen, changes to it; the new mode, as the desktop sees it (D311).
pub fn follow_device<P: Platform>() -> Option<Mode> {
    if !FOLLOWS_DEVICE.load(core::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let mut display = DISPLAY.lock();
    let display = display.as_mut()?;
    if !display.can_change_mode() {
        return None;
    }
    let mut deadline = Deadline::after::<P>(FLUSH_TIMEOUT_MS);
    let wanted = display.preferred_mode(&mut deadline)?;
    if wanted == display.device_mode() {
        return None;
    }
    display.change_mode(wanted, &mut deadline).ok()?;
    Some(display.mode())
}

/// Makes `scanout` the kernel's display. Refused, and handed back, when there is one already.
pub fn install<S: Scanout + 'static>(scanout: S) -> Result<(), S> {
    let mut display = DISPLAY.lock();
    if display.is_some() {
        return Err(scanout);
    }
    *display = Some(Box::new(Display::new(Box::new(scanout))));
    Ok(())
}

/// The rotation in force (D330), kept apart from the display's lock so the input layer reads
/// it from anywhere; upright without a display.
static ROTATION: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// The rotation in force (D330).
pub fn rotation() -> Rotation {
    match ROTATION.load(core::sync::atomic::Ordering::Relaxed) {
        1 => Rotation::Quarter,
        2 => Rotation::Half,
        3 => Rotation::ThreeQuarters,
        _ => Rotation::Upright,
    }
}

/// Turns the display's picture ([`Display::set_rotation`]): the mode the desktop now draws at,
/// `None` without a display or when it already turned so; an error without memory for it.
pub fn set_rotation(rotation: Rotation) -> Result<Option<Mode>, &'static str> {
    let mut display = DISPLAY.lock();
    let Some(display) = display.as_mut() else {
        return Ok(None);
    };
    if display.rotation() == rotation {
        return Ok(None);
    }
    if !display.set_rotation(rotation) {
        return Err("no memory to turn the screen");
    }
    ROTATION.store(rotation as u8, core::sync::atomic::Ordering::Relaxed);
    Ok(Some(display.mode()))
}

/// Makes the display show at `scale` ([`Display::set_scale`]); whether it could.
pub fn set_scale(scale: u32) -> bool {
    DISPLAY
        .lock()
        .as_mut()
        .is_some_and(|display| display.set_scale(scale))
}

/// Makes the display show `mode`, or its preferred size for `None` ([`Display::change_mode`]),
/// before the desktop starts (D287).
pub fn change_mode<P: Platform>(mode: Option<Mode>) -> Result<(), &'static str> {
    let mut display = DISPLAY.lock();
    let display = display.as_mut().ok_or("no display")?;
    let mut deadline = Deadline::after::<P>(FLUSH_TIMEOUT_MS);
    let mode = match mode {
        Some(mode) => mode,
        None => display.preferred_mode(&mut deadline).ok_or("the display prefers no size")?,
    };
    if mode == display.device_mode() {
        return Ok(());
    }
    display.change_mode(mode, &mut deadline)
}

/// Whether the display can change size; false without one.
pub fn can_change_mode() -> bool {
    DISPLAY.lock().as_ref().is_some_and(|display| display.can_change_mode())
}

/// The device's own size, before the scale divides it; `None` without a display.
pub fn device_mode() -> Option<Mode> {
    DISPLAY.lock().as_ref().map(|display| display.device_mode())
}

/// `/proc/driver/display` (D287): `<width>x<height> scale <n> changeable yes|no`, the device's
/// size, or `none` without a display.
pub fn status_text() -> alloc::string::String {
    let display = DISPLAY.lock();
    let Some(display) = display.as_ref() else {
        return alloc::string::String::from("none");
    };
    let mode = display.device_mode();
    let changeable = match display.can_change_mode() {
        true => "yes",
        false => "no",
    };
    alloc::format!("{}x{} scale {} changeable {changeable}", mode.width, mode.height, display.scale())
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

/// Lends `work` the display device's 3D commands (D415) once any posted flush is answered,
/// with the deadline its commands wait until; the display's lock is held meanwhile. Refused
/// without a display or when its device takes no 3D commands.
pub fn with_renderer<P: Platform, R>(
    work: impl FnOnce(&mut dyn crate::rendering::Renderer, &mut Deadline) -> R,
) -> Result<R, &'static str> {
    let mut display = DISPLAY.lock();
    let display = display.as_mut().ok_or("no display")?;
    let mut deadline = Deadline::after::<P>(RENDER_TIMEOUT_MS);
    display.settle(&mut deadline)?;
    let renderer = display
        .scanout
        .renderer()
        .ok_or("the display's device takes no 3D commands")?;
    Ok(work(renderer, &mut deadline))
}

/// Waits until a posted 3D command is answered (D438), with the display's lock let go while
/// the device works: sleeps on its interrupt, or polls on a device without one. With
/// `nowait`, only looks. Whether it is answered (`false` only with `nowait`).
pub fn settle_renderer_sleeping<P: Platform>(nowait: bool) -> Result<bool, &'static str> {
    let mut deadline = Deadline::after::<P>(RENDER_TIMEOUT_MS);
    let events = {
        let mut display = DISPLAY.lock();
        let Some(renderer) = display.as_mut().and_then(|display| display.scanout.renderer()) else {
            return Ok(true);
        };
        if renderer.finish()? {
            return Ok(true);
        }
        if nowait {
            return Ok(false);
        }
        renderer.events()
    };
    let Some(events) = events else {
        let mut display = DISPLAY.lock();
        return match display.as_mut() {
            Some(display) => display.settle_renderer(&mut deadline).map(|()| true),
            None => Ok(true),
        };
    };
    let mut seen = events.raised();
    loop {
        let finished = DISPLAY
            .lock()
            .as_mut()
            .and_then(|display| display.scanout.renderer().map(|renderer| renderer.finish()))
            .unwrap_or(Ok(true))?;
        if finished {
            return Ok(true);
        }
        if !deadline.sleep_until_raised(events, &mut seen) {
            let mut display = DISPLAY.lock();
            return match display.as_mut() {
                Some(display) => display.settle_renderer(&mut deadline).map(|()| true),
                None => Ok(true),
            };
        }
    }
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

/// Shows what was drawn as [`flush`] does, but with the display's lock let go while the
/// device works: the flush is posted, the caller sleeps until the device's interrupt (or the
/// deadline), and the answer is collected (D414). Polls as [`flush`] on a display without an
/// interrupt. Call it with no lock held, from a thread that may sleep.
pub fn flush_sleeping<P: Platform>() -> Result<usize, &'static str> {
    let mut deadline = Deadline::after::<P>(FLUSH_TIMEOUT_MS);
    let started = {
        let mut display = DISPLAY.lock();
        match display.as_mut() {
            Some(display) => display.start_flush(&mut deadline)?,
            None => return Ok(0),
        }
    };
    let events = match started {
        FlushStart::Done(shown) => return Ok(shown),
        FlushStart::Posted(events) => events,
    };
    let mut seen = events.raised();
    loop {
        let finished = DISPLAY
            .lock()
            .as_mut()
            .map_or(Ok(Some(0)), |display| display.finish_flush())?;
        if let Some(shown) = finished {
            return Ok(shown);
        }
        if !deadline.sleep_until_raised(events, &mut seen) {
            let mut display = DISPLAY.lock();
            return match display.as_mut() {
                Some(display) => display.settle(&mut deadline).map(|()| 0),
                None => Ok(0),
            };
        }
    }
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
