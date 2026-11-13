//! AMD's display engine, DCN 3.x (D496): the pipe the firmware lit, taken over at its mode.
//! Each pipe's hub (HUBP) reads a surface at a GPU address and feeds its own DPP; a flip
//! writes the new address (its high half first, the low half latching both) to be taken at the
//! next vertical update, and the hub says while one is pending. The cursor is the hub's 64x64
//! ARGB cursor, enabled in the DPP too. Register places are dwords from the display block's
//! segment 2 base (IP discovery, [`super::amd_discovery`]), from AMD's published
//! `dcn_<version>_offset.h` and `_sh_mask.h` headers (`AMD-NOTICE.md`); every 3.x version
//! shares them for these registers.
//!
//! Modes (D521): when the lit pipe's timing generator feeds a DisplayPort stream whose pixel
//! clock comes from the DCCG's DisplayPort DTO, [`DisplayEngine::set_mode`] drives it at
//! another timing over the link the firmware trained, up to the firmware mode's pixel rate:
//! the stream and the generator stop, the DTO takes the new pixel clock, the generator, the
//! hub's viewport and request timing (the firmware's, rescaled to the new line time), the
//! scaler (bypassed, one to one) and the stream's attributes take the new timing, and both
//! start again.

use crate::edid::Timing;
use crate::io::RegisterIo;

/// The register segment these places count from.
pub const SEGMENT: usize = 2;
/// The register segment the display clocks (DCCG) count from.
pub const CLOCK_SEGMENT: usize = 1;
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

const OTG_STRIDE: u32 = 0x80;
const STREAM_STRIDE: u32 = 0x100;
const CLOCK_STRIDE: u32 = 4;
const MOST_STREAMS: usize = 6;

const VIEWPORT_START: u32 = 0x05e9;
const BLANK_OFFSET_0: u32 = 0x0644;
const BLANK_OFFSET_1: u32 = 0x0645;
const DST_DIMENSIONS: u32 = 0x0646;
const REF_FREQ_TO_PIX_FREQ: u32 = 0x065d;
const SCALER_MODE: u32 = 0x0cfb;
const SCALER_H_BLANK: u32 = 0x0d0f;
const SCALER_V_BLANK: u32 = 0x0d10;
const RECOUT_START: u32 = 0x0d11;
const RECOUT_SIZE: u32 = 0x0d12;
const MPC_SIZE: u32 = 0x0d13;
const OTG_H_TOTAL: u32 = 0x1b2a;
const OTG_H_BLANK: u32 = 0x1b2b;
const OTG_H_SYNC: u32 = 0x1b2c;
const OTG_H_SYNC_CONTROL: u32 = 0x1b2d;
const OTG_V_TOTAL: u32 = 0x1b2f;
const OTG_V_TOTAL_MIN: u32 = 0x1b30;
const OTG_V_TOTAL_MAX: u32 = 0x1b31;
const OTG_V_BLANK: u32 = 0x1b36;
const OTG_V_SYNC: u32 = 0x1b37;
const OTG_V_SYNC_CONTROL: u32 = 0x1b38;
const OTG_CONTROL: u32 = 0x1b41;
const DIG_FRONT_END_CONTROL: u32 = 0x208b;
const DP_STREAM_CONTROL: u32 = 0x210c;
const DP_MSA_TOTALS: u32 = 0x214c;
const DP_MSA_STARTS: u32 = 0x214d;
const DP_MSA_SYNCS: u32 = 0x214e;
const DP_MSA_SIZE: u32 = 0x214f;
const PIXEL_RATE_CONTROL: u32 = 0x0080;
const DP_DTO_PHASE: u32 = 0x0081;
const DP_DTO_MODULO: u32 = 0x0082;

const TIMING_MASK: u32 = 0x7fff;
const HIGH_HALF_SHIFT: u32 = 16;
const MASTER_ENABLE: u32 = 1;
const MASTER_ENABLED: u32 = 1 << 16;
const SYNC_NEGATIVE: u32 = 1;
const SOURCE_SELECT_MASK: u32 = 0x7;
const STREAM_ENABLE: u32 = 1;
const STREAM_RUNNING: u32 = 1 << 16;
const DP_DTO_ENABLE: u32 = 1 << 4;
const SCALER_MODE_MASK: u32 = 0x7;
const MSA_H_SYNC_NEGATIVE: u32 = 1 << 31;
const MSA_V_SYNC_NEGATIVE: u32 = 1 << 15;
const H_BLANK_END_CYCLES_MASK: u32 = 0x1fff;
const NEXT_START_MASK: u32 = 0x3_ffff;
const CYCLES_MASK: u32 = 0x1f_ffff;
const RATIO_FRACTION_BITS: u32 = 19;
const HTOTAL_FRACTION_BITS: u32 = 8;
const NEXT_START_FRACTION_BITS: u32 = 2;
/// A surface's pitch is a multiple of this many pixels (256 bytes).
pub const PITCH_ALIGNMENT_PIXELS: u32 = 64;

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
    /// The lit pipe drives no DisplayPort stream clocked by the DTO, so its mode stays.
    NoDisplayPortOutput,
    /// The timing needs more pixels a second than the trained link carried at the start.
    BeyondLink,
    /// The timing's sizes do not fit the timing generator.
    UnfitTiming,
    /// The stream or the timing generator did not stop or start in time.
    ModeSetTimedOut,
}

impl Refusal {
    pub fn description(self) -> &'static str {
        match self {
            Self::UnknownVersion => "the display engine is not DCN 3",
            Self::NothingLit => "no pipe shows the firmware's framebuffer",
            Self::UnknownFormat => "the firmware's surface is not 32-bit ARGB",
            Self::FlipTimedOut => "the display did not take the last frame in time",
            Self::NoDisplayPortOutput => "the screen is not on a DisplayPort output",
            Self::BeyondLink => "the mode needs more than the screen's link carries",
            Self::UnfitTiming => "the mode's timing does not fit the display engine",
            Self::ModeSetTimedOut => "the display did not stop or start in time",
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

/// The output the lit pipe drives: its timing generator, its DisplayPort stream encoder, and
/// the pixel clock the firmware set, in hertz, which bounds what the trained link carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LitOutput {
    pub timing_generator: usize,
    pub stream: usize,
    pub link_pixel_hz: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GeneratorTiming {
    width: u32,
    height: u32,
    h_total: u32,
    h_blank_start: u32,
    h_blank_end: u32,
    h_sync: u32,
    v_total: u32,
    v_blank_start: u32,
    v_blank_end: u32,
    v_sync: u32,
    h_sync_negative: bool,
    v_sync_negative: bool,
}

impl GeneratorTiming {
    fn of(timing: &Timing) -> Option<Self> {
        let h_total = timing.width.checked_add(timing.horizontal_blank)?;
        let v_total = timing.height.checked_add(timing.vertical_blank)?;
        let h_blank_start = h_total.checked_sub(timing.horizontal_sync_offset)?;
        let v_blank_start = v_total.checked_sub(timing.vertical_sync_offset)?;
        let h_blank_end = h_blank_start.checked_sub(timing.width)?;
        let v_blank_end = v_blank_start.checked_sub(timing.height)?;
        let fits = h_total <= TIMING_MASK
            && v_total <= TIMING_MASK
            && timing.width > 0
            && timing.height > 0
            && timing.horizontal_sync_width <= h_blank_end
            && timing.vertical_sync_width <= v_blank_end;
        fits.then_some(Self {
            width: timing.width,
            height: timing.height,
            h_total,
            h_blank_start,
            h_blank_end,
            h_sync: timing.horizontal_sync_width,
            v_total,
            v_blank_start,
            v_blank_end,
            v_sync: timing.vertical_sync_width,
            h_sync_negative: !timing.horizontal_sync_positive,
            v_sync_negative: !timing.vertical_sync_positive,
        })
    }
}

fn pair(low: u32, high: u32) -> u32 {
    high << HIGH_HALF_SHIFT | low
}

fn active(start_end: u32) -> u32 {
    (start_end & TIMING_MASK).saturating_sub(start_end >> HIGH_HALF_SHIFT & TIMING_MASK)
}

/// One DCN engine's registers, at the display block's segment bases, and the pipe it drives.
pub struct DisplayEngine<R: RegisterIo> {
    regs: R,
    segment_base: u32,
    clock_segment_base: u32,
    pipe: usize,
    output: Option<LitOutput>,
    pixel_hz: u32,
}

impl<R: RegisterIo> DisplayEngine<R> {
    /// Takes the engine whose segment 2 and segment 1 bases are `segment_base` and
    /// `clock_segment_base` (dwords) behind `regs` (the register BAR) and the pipe whose surface
    /// is `width` by `height`, as the firmware lit it.
    pub fn take(
        regs: R,
        version: (u8, u8, u8),
        (segment_base, clock_segment_base): (u32, u32),
        (width, height): (u32, u32),
    ) -> Result<(Self, LitSurface), Refusal> {
        if !knows(version) {
            return Err(Refusal::UnknownVersion);
        }
        let mut engine = Self {
            regs,
            segment_base,
            clock_segment_base,
            pipe: 0,
            output: None,
            pixel_hz: 0,
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
        engine.output = engine.find_output((width, height));
        engine.pixel_hz = engine.output.map_or(0, |output| output.link_pixel_hz);
        Ok((engine, lit))
    }

    /// The output whose mode [`Self::set_mode`] can change, if the lit pipe has one.
    pub fn output(&self) -> Option<LitOutput> {
        self.output
    }

    fn find_output(&self, (width, height): (u32, u32)) -> Option<LitOutput> {
        let timing_generator = (0..MOST_PIPES).find(|&generator| {
            let read = |place| self.read_at(place, OTG_STRIDE, generator);
            read(OTG_CONTROL) & MASTER_ENABLE != 0
                && active(read(OTG_H_BLANK)) == width
                && active(read(OTG_V_BLANK)) == height
        })?;
        let stream = (0..MOST_STREAMS).find(|&stream| {
            let read = |place| self.read_at(place, STREAM_STRIDE, stream);
            read(DIG_FRONT_END_CONTROL) & SOURCE_SELECT_MASK == timing_generator as u32
                && read(DP_STREAM_CONTROL) & STREAM_ENABLE != 0
        })?;
        let clock = |place| self.read_clock(place, timing_generator);
        if clock(PIXEL_RATE_CONTROL) & DP_DTO_ENABLE == 0 {
            return None;
        }
        let (phase, modulo) = (clock(DP_DTO_PHASE), clock(DP_DTO_MODULO));
        (phase != 0 && phase <= modulo).then_some(LitOutput {
            timing_generator,
            stream,
            link_pixel_hz: phase,
        })
    }

    fn read_at(&self, place: u32, stride: u32, index: usize) -> u32 {
        self.regs.read32(self.offset(place, stride, index))
    }

    fn write_at(&self, place: u32, stride: u32, index: usize, value: u32) {
        self.regs.write32(self.offset(place, stride, index), value);
    }

    fn clock_offset(&self, place: u32, index: usize) -> usize {
        (self.clock_segment_base as usize + place as usize + index * CLOCK_STRIDE as usize) * 4
    }

    fn read_clock(&self, place: u32, index: usize) -> u32 {
        self.regs.read32(self.clock_offset(place, index))
    }

    fn wait_for(
        &self,
        (place, stride, index): (u32, u32, usize),
        bit: u32,
        set: bool,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Refusal> {
        while (self.read_at(place, stride, index) & bit != 0) != set {
            if !keep_waiting() {
                return Err(Refusal::ModeSetTimedOut);
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    /// Drives the lit output at `timing`, showing the surface at GPU address `surface`,
    /// `pitch_pixels` wide (a multiple of [`PITCH_ALIGNMENT_PIXELS`]), from the first frame;
    /// `keep_waiting` bounds each wait for the stream and the timing generator.
    pub fn set_mode(
        &mut self,
        timing: &Timing,
        surface: u64,
        pitch_pixels: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Refusal> {
        let output = self.output.ok_or(Refusal::NoDisplayPortOutput)?;
        let pixel_hz = timing
            .clock_khz
            .checked_mul(1000)
            .ok_or(Refusal::BeyondLink)?;
        if pixel_hz == 0 || pixel_hz > output.link_pixel_hz {
            return Err(Refusal::BeyondLink);
        }
        let shape = GeneratorTiming::of(timing).ok_or(Refusal::UnfitTiming)?;
        if shape.width > VIEWPORT_MASK
            || shape.height > VIEWPORT_MASK
            || pitch_pixels < shape.width
            || pitch_pixels > PITCH_MASK + 1
        {
            return Err(Refusal::UnfitTiming);
        }
        let (generator, stream) = (output.timing_generator, output.stream);
        let stream_control = self.read_at(DP_STREAM_CONTROL, STREAM_STRIDE, stream);
        self.write_at(
            DP_STREAM_CONTROL,
            STREAM_STRIDE,
            stream,
            stream_control & !STREAM_ENABLE,
        );
        let stream_status = (DP_STREAM_CONTROL, STREAM_STRIDE, stream);
        self.wait_for(stream_status, STREAM_RUNNING, false, keep_waiting)?;
        let control = self.read_at(OTG_CONTROL, OTG_STRIDE, generator);
        self.write_at(OTG_CONTROL, OTG_STRIDE, generator, control & !MASTER_ENABLE);
        let generator_status = (OTG_CONTROL, OTG_STRIDE, generator);
        self.wait_for(generator_status, MASTER_ENABLED, false, keep_waiting)?;
        self.regs
            .write32(self.clock_offset(DP_DTO_PHASE, generator), pixel_hz);
        self.program_generator(generator, &shape);
        self.program_hub(&shape, surface, pitch_pixels, pixel_hz);
        self.program_scaler(&shape);
        self.program_stream_attributes(stream, &shape);
        self.write_at(OTG_CONTROL, OTG_STRIDE, generator, control | MASTER_ENABLE);
        self.wait_for(generator_status, MASTER_ENABLED, true, keep_waiting)?;
        self.write_at(
            DP_STREAM_CONTROL,
            STREAM_STRIDE,
            stream,
            stream_control | STREAM_ENABLE,
        );
        self.wait_for(stream_status, STREAM_RUNNING, true, keep_waiting)?;
        self.pixel_hz = pixel_hz;
        Ok(())
    }

    fn program_generator(&self, generator: usize, shape: &GeneratorTiming) {
        let write = |place, value| self.write_at(place, OTG_STRIDE, generator, value);
        let polarity = |place, negative: bool| {
            let value = self.read_at(place, OTG_STRIDE, generator) & !SYNC_NEGATIVE;
            write(place, value | if negative { SYNC_NEGATIVE } else { 0 });
        };
        write(OTG_H_TOTAL, shape.h_total - 1);
        write(OTG_H_SYNC, pair(0, shape.h_sync));
        write(OTG_H_BLANK, pair(shape.h_blank_start, shape.h_blank_end));
        polarity(OTG_H_SYNC_CONTROL, shape.h_sync_negative);
        write(OTG_V_TOTAL, shape.v_total - 1);
        write(OTG_V_TOTAL_MAX, shape.v_total - 1);
        write(OTG_V_TOTAL_MIN, shape.v_total - 1);
        write(OTG_V_SYNC, pair(0, shape.v_sync));
        write(OTG_V_BLANK, pair(shape.v_blank_start, shape.v_blank_end));
        polarity(OTG_V_SYNC_CONTROL, shape.v_sync_negative);
    }

    fn program_hub(&self, shape: &GeneratorTiming, surface: u64, pitch_pixels: u32, pixel_hz: u32) {
        let old_ratio = u64::from(self.read(REF_FREQ_TO_PIX_FREQ, HUBP_STRIDE) & CYCLES_MASK);
        let ratio = old_ratio * u64::from(self.pixel_hz) / u64::from(pixel_hz);
        let cycles = |pixels: u32, fraction_bits: u32| {
            ((ratio * u64::from(pixels)) >> (RATIO_FRACTION_BITS - fraction_bits)) as u32
        };
        self.write(
            REF_FREQ_TO_PIX_FREQ,
            HUBP_STRIDE,
            ratio as u32 & CYCLES_MASK,
        );
        self.write(
            DST_DIMENSIONS,
            HUBP_STRIDE,
            cycles(shape.h_total, HTOTAL_FRACTION_BITS) & CYCLES_MASK,
        );
        self.write(
            BLANK_OFFSET_0,
            HUBP_STRIDE,
            pair(
                cycles(shape.h_blank_end, 0) & H_BLANK_END_CYCLES_MASK,
                shape.v_blank_end,
            ),
        );
        self.write(
            BLANK_OFFSET_1,
            HUBP_STRIDE,
            (shape.v_blank_start << NEXT_START_FRACTION_BITS) & NEXT_START_MASK,
        );
        self.write(VIEWPORT_START, HUBP_STRIDE, 0);
        self.write(
            VIEWPORT_DIMENSION,
            HUBP_STRIDE,
            pair(shape.width, shape.height),
        );
        let pitch = self.read(SURFACE_PITCH, HUBP_STRIDE) & !PITCH_MASK;
        self.write(SURFACE_PITCH, HUBP_STRIDE, pitch | (pitch_pixels - 1));
        self.flip(surface);
    }

    fn program_scaler(&self, shape: &GeneratorTiming) {
        let mode = self.read(SCALER_MODE, DPP_STRIDE) & !SCALER_MODE_MASK;
        self.write(SCALER_MODE, DPP_STRIDE, mode);
        self.write(RECOUT_START, DPP_STRIDE, 0);
        self.write(RECOUT_SIZE, DPP_STRIDE, pair(shape.width, shape.height));
        self.write(MPC_SIZE, DPP_STRIDE, pair(shape.width, shape.height));
        self.write(SCALER_H_BLANK, DPP_STRIDE, pair(shape.width, 0));
        self.write(SCALER_V_BLANK, DPP_STRIDE, pair(shape.height, 0));
    }

    fn program_stream_attributes(&self, stream: usize, shape: &GeneratorTiming) {
        let write = |place, value| self.write_at(place, STREAM_STRIDE, stream, value);
        let h_sync_negative = if shape.h_sync_negative {
            MSA_H_SYNC_NEGATIVE
        } else {
            0
        };
        let v_sync_negative = if shape.v_sync_negative {
            MSA_V_SYNC_NEGATIVE
        } else {
            0
        };
        write(DP_MSA_TOTALS, pair(shape.v_total, shape.h_total));
        write(DP_MSA_STARTS, pair(shape.v_blank_end, shape.h_blank_end));
        write(
            DP_MSA_SYNCS,
            h_sync_negative | shape.h_sync << HIGH_HALF_SHIFT | v_sync_negative | shape.v_sync,
        );
        write(DP_MSA_SIZE, pair(shape.height, shape.width));
    }

    fn offset(&self, place: u32, stride: u32, pipe: usize) -> usize {
        (self.segment_base as usize + place as usize + pipe * stride as usize) * 4
    }

    fn read(&self, place: u32, stride: u32) -> u32 {
        self.regs.read32(self.offset(place, stride, self.pipe))
    }

    fn write(&self, place: u32, stride: u32, value: u32) {
        self.regs
            .write32(self.offset(place, stride, self.pipe), value);
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
        self.write(
            PRIMARY_ADDRESS_HIGH,
            HUBP_STRIDE,
            (address >> 32) as u32 & ADDRESS_HIGH_MASK,
        );
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
        self.write(
            CURSOR_ADDRESS_HIGH,
            HUBP_STRIDE,
            (image >> 32) as u32 & ADDRESS_HIGH_MASK,
        );
        self.write(CURSOR_ADDRESS, HUBP_STRIDE, image as u32);
        self.write(
            CURSOR_SIZE,
            HUBP_STRIDE,
            CURSOR_SIDE << SIZE_WIDTH_SHIFT | CURSOR_SIDE,
        );
        self.write(
            CURSOR_HOT_SPOT,
            HUBP_STRIDE,
            hot_x << POSITION_X_SHIFT | hot_y,
        );
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
    const CLOCK_BASE: u32 = 0x0100;

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
        let (engine, lit) =
            DisplayEngine::take(&regs, (3, 1, 4), (BASE, CLOCK_BASE), (1920, 1080)).unwrap();
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
        assert_eq!(
            regs.get(at(FLIP_CONTROL, HUBP_STRIDE, 2)) & FLIP_TYPE_IMMEDIATE,
            0
        );
        engine.flip(0xf4_0080_0000);
        assert_eq!(regs.get(at(PRIMARY_ADDRESS, HUBP_STRIDE, 2)), 0x0080_0000);
        assert_eq!(regs.get(at(PRIMARY_ADDRESS_HIGH, HUBP_STRIDE, 2)), 0xf4);
        assert!(engine.flip_done());
    }

    fn clock_at(place: u32, index: usize) -> usize {
        (CLOCK_BASE as usize + place as usize + index * CLOCK_STRIDE as usize) * 4
    }

    fn lit_output(regs: &Registers, generator: usize, stream: usize) {
        let otg = |place| at(place, OTG_STRIDE, generator);
        regs.set(otg(OTG_CONTROL), MASTER_ENABLE | MASTER_ENABLED);
        regs.set(otg(OTG_H_BLANK), pair(2120, 200));
        regs.set(otg(OTG_V_BLANK), pair(1121, 41));
        regs.set(
            at(DIG_FRONT_END_CONTROL, STREAM_STRIDE, stream),
            generator as u32,
        );
        regs.set(
            at(DP_STREAM_CONTROL, STREAM_STRIDE, stream),
            STREAM_ENABLE | STREAM_RUNNING,
        );
        regs.set(clock_at(PIXEL_RATE_CONTROL, generator), DP_DTO_ENABLE);
        regs.set(clock_at(DP_DTO_PHASE, generator), 148_500_000);
        regs.set(clock_at(DP_DTO_MODULO, generator), 600_000_000);
        regs.set(
            at(REF_FREQ_TO_PIX_FREQ, HUBP_STRIDE, 0),
            1 << RATIO_FRACTION_BITS,
        );
    }

    fn follow_enables(regs: &Registers, generator: usize, stream: usize) {
        let otg = at(OTG_CONTROL, OTG_STRIDE, generator);
        let enabled = regs.get(otg) & MASTER_ENABLE;
        regs.set(
            otg,
            regs.get(otg) & !MASTER_ENABLED | enabled * MASTER_ENABLED,
        );
        let dp = at(DP_STREAM_CONTROL, STREAM_STRIDE, stream);
        let running = regs.get(dp) & STREAM_ENABLE;
        regs.set(
            dp,
            regs.get(dp) & !STREAM_RUNNING | running * STREAM_RUNNING,
        );
    }

    #[test]
    fn a_new_mode_reprograms_the_generator_hub_scaler_stream_and_clock() {
        let regs = Registers::default();
        lit_on(&regs, 0);
        lit_output(&regs, 0, 3);
        let (mut engine, _) =
            DisplayEngine::take(&regs, (3, 1, 4), (BASE, CLOCK_BASE), (1920, 1080)).unwrap();
        assert_eq!(
            engine.output(),
            Some(LitOutput {
                timing_generator: 0,
                stream: 3,
                link_pixel_hz: 148_500_000,
            })
        );
        let timing = Timing::reduced_blanking(1280, 720, 60).unwrap();
        engine
            .set_mode(&timing, 0xf4_0080_0000, 1280, &mut || {
                follow_enables(&regs, 0, 3);
                true
            })
            .unwrap();
        let otg = |place| regs.get(at(place, OTG_STRIDE, 0));
        assert_eq!(otg(OTG_H_TOTAL), 1360 - 1);
        assert_eq!(otg(OTG_H_BLANK), pair(1352, 72));
        assert_eq!(otg(OTG_H_SYNC), pair(0, 32));
        assert_eq!(otg(OTG_V_SYNC_CONTROL) & SYNC_NEGATIVE, SYNC_NEGATIVE);
        assert_eq!(otg(OTG_CONTROL) & MASTER_ENABLE, MASTER_ENABLE);
        assert_eq!(regs.get(clock_at(DP_DTO_PHASE, 0)), timing.clock_khz * 1000);
        assert_eq!(
            regs.get(at(VIEWPORT_DIMENSION, HUBP_STRIDE, 0)),
            pair(1280, 720)
        );
        assert_eq!(regs.get(at(SURFACE_PITCH, HUBP_STRIDE, 0)), 1279);
        assert_eq!(regs.get(at(RECOUT_SIZE, DPP_STRIDE, 0)), pair(1280, 720));
        let msa = |place| regs.get(at(place, STREAM_STRIDE, 3));
        assert_eq!(msa(DP_MSA_SIZE), pair(720, 1280));
        assert_eq!(msa(DP_MSA_STARTS) >> HIGH_HALF_SHIFT, 72);
        assert_eq!(msa(DP_STREAM_CONTROL) & STREAM_ENABLE, STREAM_ENABLE);
        let faster = Timing::reduced_blanking(3840, 2160, 60).unwrap();
        assert_eq!(
            engine.set_mode(&faster, 0, 3840, &mut || true),
            Err(Refusal::BeyondLink)
        );
    }

    #[test]
    fn a_pipe_without_a_displayport_dto_keeps_its_mode() {
        let regs = Registers::default();
        lit_on(&regs, 0);
        lit_output(&regs, 0, 0);
        regs.set(clock_at(PIXEL_RATE_CONTROL, 0), 0);
        let (mut engine, _) =
            DisplayEngine::take(&regs, (3, 0, 0), (BASE, CLOCK_BASE), (1920, 1080)).unwrap();
        let timing = Timing::reduced_blanking(1280, 720, 60).unwrap();
        assert_eq!(
            engine.set_mode(&timing, 0, 1280, &mut || true),
            Err(Refusal::NoDisplayPortOutput)
        );
    }

    #[test]
    fn other_versions_sizes_and_formats_are_refused() {
        let regs = Registers::default();
        lit_on(&regs, 0);
        assert_eq!(
            DisplayEngine::take(&regs, (2, 1, 0), (BASE, CLOCK_BASE), (1920, 1080)).err(),
            Some(Refusal::UnknownVersion)
        );
        assert_eq!(
            DisplayEngine::take(&regs, (3, 2, 0), (BASE, CLOCK_BASE), (1280, 800)).err(),
            Some(Refusal::NothingLit)
        );
        regs.set(at(SURFACE_CONFIG, HUBP_STRIDE, 0), 10);
        assert_eq!(
            DisplayEngine::take(&regs, (3, 0, 1), (BASE, CLOCK_BASE), (1920, 1080)).err(),
            Some(Refusal::UnknownFormat)
        );
    }

    #[test]
    fn a_cursor_off_the_left_starts_at_the_edge_and_skips_its_hidden_part() {
        let regs = Registers::default();
        lit_on(&regs, 0);
        let (engine, _) =
            DisplayEngine::take(&regs, (3, 5, 0), (BASE, CLOCK_BASE), (1920, 1080)).unwrap();
        engine.show_cursor(0xf4_0100_0000, -10, 20);
        assert_eq!(regs.get(at(CURSOR_HOT_SPOT, HUBP_STRIDE, 0)), 10 << 16);
        assert_eq!(regs.get(at(CURSOR_POSITION, HUBP_STRIDE, 0)), 20);
        assert_eq!(
            regs.get(at(CURSOR_CONTROL, HUBP_STRIDE, 0)) & CURSOR_ENABLE,
            1
        );
        assert_eq!(
            regs.get(at(DPP_CURSOR_CONTROL, DPP_STRIDE, 0)) & CURSOR_ENABLE,
            1
        );
        engine.hide_cursor();
        assert_eq!(
            regs.get(at(CURSOR_CONTROL, HUBP_STRIDE, 0)) & CURSOR_ENABLE,
            0
        );
        assert_eq!(
            regs.get(at(DPP_CURSOR_CONTROL, DPP_STRIDE, 0)) & CURSOR_ENABLE,
            0
        );
    }
}
