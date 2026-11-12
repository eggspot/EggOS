//! AMD's display engine, DCN 3.x (D496): the pipe the firmware lit, taken over at its mode.
//! Each pipe's hub (HUBP) reads a surface at a GPU address and feeds its own DPP; a flip
//! writes the new address (its high half first, the low half latching both) to be taken at the
//! next vertical update, and the hub says while one is pending. The cursor is the hub's 64x64
//! ARGB cursor, enabled in the DPP too. Register places are dwords from the display block's
//! segment 2 base (IP discovery, [`super::amd_discovery`]), from AMD's published
//! `dcn_<version>_offset.h` and `_sh_mask.h` headers (`AMD-NOTICE.md`); every 3.x version
//! shares them for these registers.

use crate::io::RegisterIo;

/// The register segment these places count from.
pub const SEGMENT: usize = 2;
/// The pipes a DCN 3.x engine has at most.
pub const MOST_PIPES: usize = 6;
/// The cursor's side, in pixels, and its bytes (ARGB, 4 bytes a pixel).
pub const CURSOR_SIDE: u32 = 64;
pub const CURSOR_BYTES: usize = (CURSOR_SIDE * CURSOR_SIDE * 4) as usize;
/// The alignment a surface's GPU address needs.
pub const SURFACE_ALIGNMENT: u64 = 1 << 16;

const HUBP_STRIDE: u32 = 0xdc;
const DPP_STRIDE: u32 = 0x16b;

const SURFACE_CONFIG: u32 = 0x05e5;
const VIEWPORT_DIMENSION: u32 = 0x05ea;
const SURFACE_PITCH: u32 = 0x0607;
const PRIMARY_ADDRESS: u32 = 0x060a;
const PRIMARY_ADDRESS_HIGH: u32 = 0x060b;
const FLIP_CONTROL: u32 = 0x061b;
const CURSOR_CONTROL: u32 = 0x0678;
const CURSOR_ADDRESS: u32 = 0x0679;
const CURSOR_ADDRESS_HIGH: u32 = 0x067a;
const CURSOR_SIZE: u32 = 0x067b;
const CURSOR_POSITION: u32 = 0x067c;
const CURSOR_HOT_SPOT: u32 = 0x067d;
const DPP_CURSOR_CONTROL: u32 = 0x0cf1;

const VIEWPORT_MASK: u32 = 0x3fff;
const VIEWPORT_HEIGHT_SHIFT: u32 = 16;
const PITCH_MASK: u32 = 0x3fff;
const ADDRESS_HIGH_MASK: u32 = 0xffff;
const FLIP_TYPE_IMMEDIATE: u32 = 1 << 1;
const FLIP_PENDING: u32 = 1 << 8;
const PIXEL_FORMAT_MASK: u32 = 0x7f;
const PIXEL_FORMAT_ARGB8888: u32 = 8;
const CURSOR_ENABLE: u32 = 1;
const CURSOR_MODE_SHIFT: u32 = 8;
const CURSOR_PITCH_SHIFT: u32 = 16;
const CURSOR_PITCH_64: u32 = 0;
const CURSOR_MODE_STRAIGHT_ALPHA: u32 = 3;
const DPP_CURSOR_MODE_SHIFT: u32 = 4;
const SIZE_WIDTH_SHIFT: u32 = 16;
const POSITION_X_SHIFT: u32 = 16;

/// Why the engine was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The display block is not a DCN 3.x one.
    UnknownVersion,
    /// No pipe shows a surface of the firmware framebuffer's size.
    NothingLit,
    /// The lit surface is not 32-bit ARGB.
    UnknownFormat,
    /// A flip was still pending when its time ran out.
    FlipTimedOut,
}

impl Refusal {
    pub fn description(self) -> &'static str {
        match self {
            Self::UnknownVersion => "the display engine is not DCN 3",
            Self::NothingLit => "no pipe shows the firmware's framebuffer",
            Self::UnknownFormat => "the firmware's surface is not 32-bit ARGB",
            Self::FlipTimedOut => "the display did not take the last frame in time",
        }
    }
}

/// Whether display block version `version` is one whose places this driver knows.
pub fn knows(version: (u8, u8, u8)) -> bool {
    version.0 == 3 && version.1 <= 5
}

/// The surface the firmware lit: its GPU address, its pitch in pixels and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LitSurface {
    pub pipe: usize,
    pub address: u64,
    pub pitch_pixels: u32,
    pub width: u32,
    pub height: u32,
}

/// One DCN engine's registers, at the display block's segment base, and the pipe it drives.
pub struct DisplayEngine<R: RegisterIo> {
    regs: R,
    segment_base: u32,
    pipe: usize,
}

impl<R: RegisterIo> DisplayEngine<R> {
    /// Takes the engine whose segment 2 base is `segment_base` (dwords) behind `regs` (the
    /// register BAR) and the pipe whose surface is `width` by `height`, as the firmware lit it.
    pub fn take(
        regs: R,
        version: (u8, u8, u8),
        segment_base: u32,
        (width, height): (u32, u32),
    ) -> Result<(Self, LitSurface), Refusal> {
        if !knows(version) {
            return Err(Refusal::UnknownVersion);
        }
        let mut engine = Self {
            regs,
            segment_base,
            pipe: 0,
        };
        let lit = (0..MOST_PIPES)
            .map(|pipe| engine.surface_of(pipe))
            .find(|surface| {
                surface.address != 0 && (surface.width, surface.height) == (width, height)
            })
            .ok_or(Refusal::NothingLit)?;
        engine.pipe = lit.pipe;
        if engine.read(SURFACE_CONFIG, HUBP_STRIDE) & PIXEL_FORMAT_MASK != PIXEL_FORMAT_ARGB8888 {
            return Err(Refusal::UnknownFormat);
        }
        let control = engine.read(FLIP_CONTROL, HUBP_STRIDE);
        engine.write(FLIP_CONTROL, HUBP_STRIDE, control & !FLIP_TYPE_IMMEDIATE);
        Ok((engine, lit))
    }

    fn offset(&self, place: u32, stride: u32, pipe: usize) -> usize {
        (self.segment_base as usize + place as usize + pipe * stride as usize) * 4
    }

    fn read(&self, place: u32, stride: u32) -> u32 {
        self.regs.read32(self.offset(place, stride, self.pipe))
    }

    fn write(&self, place: u32, stride: u32, value: u32) {
        self.regs.write32(self.offset(place, stride, self.pipe), value);
    }

    fn surface_of(&self, pipe: usize) -> LitSurface {
        let read = |place| self.regs.read32(self.offset(place, HUBP_STRIDE, pipe));
        let dimension = read(VIEWPORT_DIMENSION);
        let high = u64::from(read(PRIMARY_ADDRESS_HIGH) & ADDRESS_HIGH_MASK);
        LitSurface {
            pipe,
            address: high << 32 | u64::from(read(PRIMARY_ADDRESS)),
            pitch_pixels: (read(SURFACE_PITCH) & PITCH_MASK) + 1,
            width: dimension & VIEWPORT_MASK,
            height: dimension >> VIEWPORT_HEIGHT_SHIFT & VIEWPORT_MASK,
        }
    }

    /// Shows the surface at GPU address `address` from the next vertical update.
    pub fn flip(&self, address: u64) {
        self.write(PRIMARY_ADDRESS_HIGH, HUBP_STRIDE, (address >> 32) as u32 & ADDRESS_HIGH_MASK);
        self.write(PRIMARY_ADDRESS, HUBP_STRIDE, address as u32);
    }

    /// Whether the last flip was taken.
    pub fn flip_done(&self) -> bool {
        self.read(FLIP_CONTROL, HUBP_STRIDE) & FLIP_PENDING == 0
    }

    /// Shows the 64x64 straight-alpha ARGB cursor at GPU address `image` with its top left
    /// corner at (`x`, `y`), which may be off the screen's top or left: the hub then starts at
    /// the screen's edge and skips the image's hidden part through its hot spot.
    pub fn show_cursor(&self, image: u64, x: i32, y: i32) {
        let hot_x = x.min(0).unsigned_abs().min(CURSOR_SIDE - 1);
        let hot_y = y.min(0).unsigned_abs().min(CURSOR_SIDE - 1);
        let (x, y) = (x.max(0) as u32, y.max(0) as u32);
        self.write(CURSOR_ADDRESS_HIGH, HUBP_STRIDE, (image >> 32) as u32 & ADDRESS_HIGH_MASK);
        self.write(CURSOR_ADDRESS, HUBP_STRIDE, image as u32);
        self.write(CURSOR_SIZE, HUBP_STRIDE, CURSOR_SIDE << SIZE_WIDTH_SHIFT | CURSOR_SIDE);
        self.write(CURSOR_HOT_SPOT, HUBP_STRIDE, hot_x << POSITION_X_SHIFT | hot_y);
        self.write(CURSOR_POSITION, HUBP_STRIDE, x << POSITION_X_SHIFT | y);
        self.write(
            CURSOR_CONTROL,
            HUBP_STRIDE,
            CURSOR_ENABLE
                | CURSOR_MODE_STRAIGHT_ALPHA << CURSOR_MODE_SHIFT
                | CURSOR_PITCH_64 << CURSOR_PITCH_SHIFT,
        );
        self.write(
            DPP_CURSOR_CONTROL,
            DPP_STRIDE,
            CURSOR_ENABLE | CURSOR_MODE_STRAIGHT_ALPHA << DPP_CURSOR_MODE_SHIFT,
        );
    }

    /// Hides the cursor.
    pub fn hide_cursor(&self) {
        let control = self.read(CURSOR_CONTROL, HUBP_STRIDE);
        self.write(CURSOR_CONTROL, HUBP_STRIDE, control & !CURSOR_ENABLE);
        let dpp = self.read(DPP_CURSOR_CONTROL, DPP_STRIDE);
        self.write(DPP_CURSOR_CONTROL, DPP_STRIDE, dpp & !CURSOR_ENABLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use core::cell::RefCell;

    #[derive(Default)]
    struct Registers(RefCell<BTreeMap<usize, u32>>);

    impl Registers {
        fn set(&self, offset: usize, value: u32) {
            self.0.borrow_mut().insert(offset, value);
        }

        fn get(&self, offset: usize) -> u32 {
            self.0.borrow().get(&offset).copied().unwrap_or(0)
        }
    }

    impl RegisterIo for &Registers {
        fn read8(&self, offset: usize) -> u8 {
            self.get(offset) as u8
        }
        fn write8(&self, offset: usize, value: u8) {
            self.set(offset, value.into());
        }
        fn read16(&self, offset: usize) -> u16 {
            self.get(offset) as u16
        }
        fn write16(&self, offset: usize, value: u16) {
            self.set(offset, value.into());
        }
        fn read32(&self, offset: usize) -> u32 {
            self.get(offset)
        }
        fn write32(&self, offset: usize, value: u32) {
            self.set(offset, value);
        }
    }

    const BASE: u32 = 0x34c0;

    fn at(place: u32, stride: u32, pipe: usize) -> usize {
        (BASE as usize + place as usize + pipe * stride as usize) * 4
    }

    fn lit_on(regs: &Registers, pipe: usize) {
        regs.set(at(VIEWPORT_DIMENSION, HUBP_STRIDE, pipe), 1080 << 16 | 1920);
        regs.set(at(PRIMARY_ADDRESS, HUBP_STRIDE, pipe), 0x0040_0000);
        regs.set(at(PRIMARY_ADDRESS_HIGH, HUBP_STRIDE, pipe), 0x00f4);
        regs.set(at(SURFACE_PITCH, HUBP_STRIDE, pipe), 1919);
        regs.set(at(SURFACE_CONFIG, HUBP_STRIDE, pipe), PIXEL_FORMAT_ARGB8888);
        regs.set(at(FLIP_CONTROL, HUBP_STRIDE, pipe), FLIP_TYPE_IMMEDIATE);
    }

    #[test]
    fn the_engine_takes_the_pipe_showing_the_firmware_framebuffer() {
        let regs = Registers::default();
        lit_on(&regs, 2);
        let (engine, lit) = DisplayEngine::take(&regs, (3, 1, 4), BASE, (1920, 1080)).unwrap();
        assert_eq!(
            lit,
            LitSurface {
                pipe: 2,
                address: 0xf4_0040_0000,
                pitch_pixels: 1920,
                width: 1920,
                height: 1080,
            }
        );
        assert_eq!(regs.get(at(FLIP_CONTROL, HUBP_STRIDE, 2)) & FLIP_TYPE_IMMEDIATE, 0);
        engine.flip(0xf4_0080_0000);
        assert_eq!(regs.get(at(PRIMARY_ADDRESS, HUBP_STRIDE, 2)), 0x0080_0000);
        assert_eq!(regs.get(at(PRIMARY_ADDRESS_HIGH, HUBP_STRIDE, 2)), 0xf4);
        assert!(engine.flip_done());
    }

    #[test]
    fn other_versions_sizes_and_formats_are_refused() {
        let regs = Registers::default();
        lit_on(&regs, 0);
        assert_eq!(
            DisplayEngine::take(&regs, (2, 1, 0), BASE, (1920, 1080)).err(),
            Some(Refusal::UnknownVersion)
        );
        assert_eq!(
            DisplayEngine::take(&regs, (3, 2, 0), BASE, (1280, 800)).err(),
            Some(Refusal::NothingLit)
        );
        regs.set(at(SURFACE_CONFIG, HUBP_STRIDE, 0), 10);
        assert_eq!(
            DisplayEngine::take(&regs, (3, 0, 1), BASE, (1920, 1080)).err(),
            Some(Refusal::UnknownFormat)
        );
    }

    #[test]
    fn a_cursor_off_the_left_starts_at_the_edge_and_skips_its_hidden_part() {
        let regs = Registers::default();
        lit_on(&regs, 0);
        let (engine, _) = DisplayEngine::take(&regs, (3, 5, 0), BASE, (1920, 1080)).unwrap();
        engine.show_cursor(0xf4_0100_0000, -10, 20);
        assert_eq!(regs.get(at(CURSOR_HOT_SPOT, HUBP_STRIDE, 0)), 10 << 16);
        assert_eq!(regs.get(at(CURSOR_POSITION, HUBP_STRIDE, 0)), 20);
        assert_eq!(regs.get(at(CURSOR_CONTROL, HUBP_STRIDE, 0)) & CURSOR_ENABLE, 1);
        assert_eq!(regs.get(at(DPP_CURSOR_CONTROL, DPP_STRIDE, 0)) & CURSOR_ENABLE, 1);
        engine.hide_cursor();
        assert_eq!(regs.get(at(CURSOR_CONTROL, HUBP_STRIDE, 0)) & CURSOR_ENABLE, 0);
        assert_eq!(regs.get(at(DPP_CURSOR_CONTROL, DPP_STRIDE, 0)) & CURSOR_ENABLE, 0);
    }
}
