//! NVIDIA's display mode setting through its channels (D530), Ampere's and Ada's NVDisplay: the
//! core channel takes the SOR's owner and protocol, window 0's owner and usage bounds, and head
//! 0's output resource, pixel clock, raster and viewport; window 0's channel takes the surface:
//! its size, pixel format, pitch, its context DMA (by handle) and its place. A context DMA is
//! five words in the display's instance memory (flags, start and limit in 256-byte units) named
//! by a hash table entry there: the handle, and a context holding the channel, the object's
//! offset and the client. A DisplayPort stream's watermark and blank symbols follow NVIDIA's
//! formulas for single-stream transport. Methods and fields are from NVIDIA's MIT-licensed
//! `clc67d.h` and `clc67e.h`; the rest as nouveau's MIT-licensed display code does it
//! (`NVIDIA-NOTICE.md`).

use crate::edid::Timing;
use alloc::vec::Vec;

/// The bytes of the display's hash table, at the start of its instance memory.
pub const HASH_TABLE_BYTES: u32 = 0x2000;
/// Where the surface's context DMA goes in the instance memory.
pub const CONTEXT_DMA_OFFSET: u32 = HASH_TABLE_BYTES;
/// The handle the surface's context DMA is known by.
pub const SURFACE_HANDLE: u32 = 0xfb00_0001;
/// Window 0's channel id among the display's channels (the core's is 0).
pub const WINDOW_CHANNEL: u32 = 1;
/// Window 0's channel PUT register in BAR 0; its GET register follows it.
pub const WINDOW_PUT: usize = 0x69_0000;
/// Window 0's channel GET register in BAR 0.
pub const WINDOW_GET: usize = 0x69_0004;

const HASH_BITS: u32 = 10;
const CORE_UPDATE: u32 = 0x0200;
const SOR_CONTROL: u32 = 0x0300;
const SOR_STRIDE: u32 = 0x20;
const WINDOW_CONTROL: u32 = 0x1000;
const WINDOW_FORMAT_BOUNDS: u32 = 0x1004;
const WINDOW_USAGE_BOUNDS: u32 = 0x1010;
const HEAD_OUTPUT_RESOURCE: u32 = 0x2004;
const HEAD_PIXEL_CLOCK: u32 = 0x200c;
const HEAD_PIXEL_CLOCK_MAX: u32 = 0x2028;
const HEAD_USAGE_BOUNDS: u32 = 0x2030;
const HEAD_VIEWPORT_POINT_IN: u32 = 0x2048;
const HEAD_VIEWPORT_SIZE_IN: u32 = 0x204c;
const HEAD_VIEWPORT_SIZE_OUT: u32 = 0x2058;
const HEAD_RASTER: u32 = 0x2064;
const WINDOW_UPDATE: u32 = 0x0200;
const WINDOW_SIZE: u32 = 0x0224;
const WINDOW_CONTEXT_DMA: u32 = 0x0240;
const WINDOW_OFFSET: u32 = 0x0260;
const WINDOW_POINT_IN: u32 = 0x0290;
const WINDOW_SIZE_IN: u32 = 0x0298;
const WINDOW_SIZE_OUT: u32 = 0x02a4;
const WINDOW_PRESENT: u32 = 0x0308;
const PROTOCOL_DP_A: u32 = 0x8 << 8;
const PACKED_4_BYTES: u32 = 1 << 2;
const SCALER_TWO_TAPS: u32 = 1 << 20;
const OUTPUT_SCALER_TWO_TAPS: u32 = 1 << 12;
const HSYNC_NEGATIVE: u32 = 1 << 2;
const VSYNC_NEGATIVE: u32 = 1 << 3;
const DEPTH_24_444: u32 = 4 << 4;
const FORMAT_X8R8G8B8: u32 = 0xe6;
const PITCH_UNIT_SHIFT: u32 = 6;
const RELEASE_ELV: u32 = 1;
const FIELD_HIGH_SHIFT: u32 = 16;
const CONTEXT_CHANNEL_SHIFT: u32 = 25;
const CONTEXT_OFFSET_SHIFT: u32 = 9;
const CONTEXT_CLIENT_MASK: u32 = 0x3fff;
const DMA_VIDEO_MEMORY: u32 = 0x1;
const DMA_READ_WRITE: u32 = 0x4;
const METHOD_COUNT_SHIFT: u32 = 18;
const TRANSFER_UNIT: u64 = 64;
const PRECISION: u64 = 100_000;
const WATERMARK_ADJUST: u64 = 2;
const WATERMARK_LEAST: u32 = 20;
const WATERMARK_MOST: u32 = 39;
const BITS_PER_PIXEL: u64 = 24;

/// A run of methods for one channel.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Push {
    pub words: Vec<u32>,
}

impl Push {
    fn methods(&mut self, offset: u32, data: &[u32]) {
        self.words
            .push((data.len() as u32) << METHOD_COUNT_SHIFT | offset);
        self.words.extend_from_slice(data);
    }
}

fn pair(low: u32, high: u32) -> u32 {
    high << FIELD_HIGH_SHIFT | low
}

/// The head's raster for `timing`: its totals, the sync's end, the blank's end and start, each
/// counted from the sync's start (NVDisplay's order).
pub fn raster(timing: &Timing) -> [u32; 4] {
    let total = (
        timing.width + timing.horizontal_blank,
        timing.height + timing.vertical_blank,
    );
    let sync_end = (
        timing.horizontal_sync_width - 1,
        timing.vertical_sync_width - 1,
    );
    let blank_end = (
        total.0 - timing.width - timing.horizontal_sync_offset - 1,
        total.1 - timing.height - timing.vertical_sync_offset - 1,
    );
    let blank_start = (blank_end.0 + timing.width, blank_end.1 + timing.height);
    [
        pair(total.0, total.1),
        pair(sync_end.0, sync_end.1),
        pair(blank_end.0, blank_end.1),
        pair(blank_start.0, blank_start.1),
    ]
}

/// The core channel's methods lighting head 0 at `timing` on SOR `sor` over DisplayPort, window
/// 0 owned by it and showing `surface` (width, height), then the core's update.
pub fn core_methods(sor: u32, timing: &Timing, surface: (u32, u32)) -> Push {
    let mut push = Push::default();
    let polarity = if timing.horizontal_sync_positive {
        0
    } else {
        HSYNC_NEGATIVE
    } | if timing.vertical_sync_positive {
        0
    } else {
        VSYNC_NEGATIVE
    };
    let hertz = timing.clock_khz * 1000;
    let size = pair(timing.width, timing.height);
    push.methods(SOR_CONTROL + sor * SOR_STRIDE, &[1 | PROTOCOL_DP_A]);
    push.methods(WINDOW_CONTROL, &[0, PACKED_4_BYTES]);
    push.methods(WINDOW_USAGE_BOUNDS, &[surface.0 | SCALER_TWO_TAPS]);
    push.methods(HEAD_OUTPUT_RESOURCE, &[polarity | DEPTH_24_444]);
    push.methods(HEAD_PIXEL_CLOCK, &[hertz]);
    push.methods(HEAD_PIXEL_CLOCK_MAX, &[hertz]);
    push.methods(HEAD_USAGE_BOUNDS, &[OUTPUT_SCALER_TWO_TAPS]);
    push.methods(HEAD_VIEWPORT_POINT_IN, &[0, size]);
    push.methods(HEAD_VIEWPORT_SIZE_OUT, &[size]);
    push.methods(HEAD_RASTER, &raster(timing));
    push.methods(CORE_UPDATE, &[0]);
    push
}

/// Window 0's methods showing the pitch surface (width, height, row bytes) behind context DMA
/// `handle`, then the window's update.
pub fn window_methods(surface: (u32, u32, u32), handle: u32) -> Push {
    let mut push = Push::default();
    let (width, height, pitch) = surface;
    let size = pair(width, height);
    push.methods(WINDOW_PRESENT, &[0]);
    push.methods(
        WINDOW_SIZE,
        &[size, 0, FORMAT_X8R8G8B8, pitch >> PITCH_UNIT_SHIFT],
    );
    push.methods(WINDOW_CONTEXT_DMA, &[handle]);
    push.methods(WINDOW_OFFSET, &[0]);
    push.methods(WINDOW_POINT_IN, &[0]);
    push.methods(WINDOW_SIZE_IN, &[size]);
    push.methods(WINDOW_SIZE_OUT, &[size]);
    push.methods(WINDOW_UPDATE, &[RELEASE_ELV]);
    push
}

/// The hash table slot of `handle` for channel `channel`.
pub fn hash_slot(handle: u32, channel: u32) -> u32 {
    let mut hash = 0;
    let mut rest = handle;
    while rest != 0 {
        hash ^= rest & ((1 << HASH_BITS) - 1);
        rest >>= HASH_BITS;
    }
    hash ^ channel << (HASH_BITS - 4)
}

/// A hash table entry's context: channel `channel`, the object at `offset` in the instance
/// memory, client `client`.
pub fn hash_context(channel: u32, offset: u32, client: u32) -> u32 {
    let client = client & CONTEXT_CLIENT_MASK;
    channel << CONTEXT_CHANNEL_SHIFT | offset << CONTEXT_OFFSET_SHIFT | client
}

/// The five words of a context DMA over video memory from byte `start`, `bytes` long, readable
/// and writable, pitch.
pub fn context_dma(start: u64, bytes: u64) -> [u32; 5] {
    let first = start >> 8;
    let last = (start + bytes - 1) >> 8;
    [
        DMA_VIDEO_MEMORY | DMA_READ_WRITE,
        first as u32,
        (first >> 32) as u32,
        last as u32,
        (last >> 32) as u32,
    ]
}

/// The single-stream DisplayPort configuration for `timing` over `lanes` lanes at `link_khz`
/// (the per-lane symbol rate in kHz): its watermark and its horizontal and vertical blank
/// symbols; `None` when the mode does not fit the link.
pub fn stream_symbols(
    timing: &Timing,
    link_khz: u32,
    lanes: u32,
    enhanced: bool,
) -> Option<(u32, u32, u32)> {
    let symbol_rate = u64::from(link_khz) * 1000;
    let pixel_rate = u64::from(timing.clock_khz) * 1000;
    let lanes64 = u64::from(lanes);
    let surface_width = u64::from(timing.width);
    let raster_width = surface_width + u64::from(timing.horizontal_blank);
    if pixel_rate * BITS_PER_PIXEL >= 8 * symbol_rate * lanes64 || surface_width <= 60 {
        return None;
    }
    let ratio = pixel_rate * BITS_PER_PIXEL * PRECISION / (8 * symbol_rate * lanes64);
    let ratio_part = ratio * TRANSFER_UNIT * (PRECISION - ratio) / PRECISION;
    let per_lane = BITS_PER_PIXEL * PRECISION / (8 * lanes64);
    let watermark = (WATERMARK_ADJUST + (2 * per_lane + ratio_part) / PRECISION) as u32;
    let symbols_per_line = (surface_width * BITS_PER_PIXEL / (8 * lanes64)) as u32;
    if watermark > WATERMARK_MOST || watermark > symbols_per_line {
        return None;
    }
    let watermark = watermark.max(WATERMARK_LEAST);
    let framing = if enhanced { 3 * 8 * lanes64 } else { 0 };
    let remainder = surface_width % lanes64;
    let steering = if remainder != 0 {
        (lanes64 - remainder) * BITS_PER_PIXEL
    } else {
        0
    };
    let blanking_bits = 3 * 8 * lanes64 + framing + 3 * 8 * 4 + steering;
    let blanking_clocks = blanking_bits * PRECISION / (8 * lanes64);
    let least_blank = blanking_clocks * pixel_rate / symbol_rate / PRECISION + 12;
    if least_blank > raster_width - surface_width {
        return None;
    }
    let latency = match lanes {
        1 => 9,
        2 => 6,
        _ => 3,
    };
    let horizontal = ((raster_width - surface_width - least_blank) * symbol_rate / pixel_rate)
        as i64
        - 4
        - latency;
    let vertical_latency = match lanes {
        1 => 39,
        2 => 21,
        _ => 12,
    };
    let vertical = match surface_width {
        width if width < 40 => 0,
        width => ((width - 40) * symbol_rate / pixel_rate) as i64 - 1 - vertical_latency,
    };
    Some((watermark, horizontal.max(0) as u32, vertical.max(0) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hd() -> Timing {
        Timing {
            clock_khz: 148_500,
            width: 1920,
            height: 1080,
            horizontal_blank: 280,
            vertical_blank: 45,
            horizontal_sync_offset: 88,
            horizontal_sync_width: 44,
            vertical_sync_offset: 4,
            vertical_sync_width: 5,
            horizontal_sync_positive: true,
            vertical_sync_positive: true,
        }
    }

    #[test]
    fn the_raster_counts_from_the_sync_start() {
        let [size, sync_end, blank_end, blank_start] = raster(&hd());
        assert_eq!(size, pair(2200, 1125));
        assert_eq!(sync_end, pair(43, 4));
        assert_eq!(blank_end, pair(191, 40));
        assert_eq!(blank_start, pair(2111, 1120));
    }

    #[test]
    fn methods_carry_their_counts_and_end_in_an_update() {
        let push = core_methods(2, &hd(), (1920, 1080));
        assert_eq!(push.words[0], 1 << 18 | 0x340);
        assert_eq!(push.words[1], 1 | PROTOCOL_DP_A);
        assert_eq!(&push.words[push.words.len() - 2..], &[1 << 18 | 0x200, 0]);
        let window = window_methods((1920, 1080, 7680), SURFACE_HANDLE);
        assert_eq!(window.words[2], 4 << 18 | 0x224);
        assert_eq!(window.words[6], 7680 >> 6);
    }

    #[test]
    fn hash_entries_name_the_channel_offset_and_client() {
        assert_eq!(hash_slot(0x0000_0401, 1), 0x1 ^ 0x1 ^ 0x40);
        assert_eq!(hash_context(1, 0x2000, 0xc1d0_0000), 1 << 25 | 0x2000 << 9);
        assert_eq!(context_dma(0x100, 0x200), [0x5, 0x1, 0, 0x2, 0]);
    }

    #[test]
    fn full_hd_fits_four_hbr_lanes_with_the_least_watermark() {
        let (watermark, horizontal, vertical) = stream_symbols(&hd(), 270_000, 4, true).unwrap();
        assert!((WATERMARK_LEAST..=WATERMARK_MOST).contains(&watermark));
        assert!(horizontal > 0 && vertical > 0);
        assert_eq!(stream_symbols(&hd(), 162_000, 1, true), None);
    }
}
