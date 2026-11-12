//! Intel mode setting, the values (D447): a mode's transcoder timing registers, the
//! DisplayPort data and link M/N ratios that pace pixels over the link, and the display-9
//! (Skylake to Coffee Lake) PLL's link-rate code. Pure arithmetic, tested on the host; the
//! engine writes them ([`super::intel::DisplayEngine::set_mode`]).

use crate::edid::Timing;

/// The largest value an M or N register holds.
pub const MN_LIMIT: u32 = (1 << 24) - 1;
/// The transfer unit size DisplayPort pacing uses.
pub const TRANSFER_UNIT: u32 = 64;

/// The transcoder's timing registers for `timing`, each a pair of values less one packed
/// high and low: `HTOTAL`, `HBLANK`, `HSYNC`, `VTOTAL`, `VBLANK`, `VSYNC`, then the pipe's
/// `PIPESRC`.
pub fn timing_registers(timing: &Timing) -> [u32; 7] {
    let pair = |high: u32, low: u32| (high.saturating_sub(1) & 0x1fff) << 16 | (low.saturating_sub(1) & 0x1fff);
    let h_total = timing.width + timing.horizontal_blank;
    let v_total = timing.height + timing.vertical_blank;
    let h_sync_start = timing.width + timing.horizontal_sync_offset;
    let v_sync_start = timing.height + timing.vertical_sync_offset;
    [
        pair(h_total, timing.width),
        pair(h_total, timing.width),
        pair(h_sync_start + timing.horizontal_sync_width, h_sync_start),
        pair(v_total, timing.height),
        pair(v_total, timing.height),
        pair(v_sync_start + timing.vertical_sync_width, v_sync_start),
        pair(timing.width, timing.height),
    ]
}

fn reduced(mut m: u64, mut n: u64) -> (u32, u32) {
    while m > u64::from(MN_LIMIT) || n > u64::from(MN_LIMIT) {
        m >>= 1;
        n >>= 1;
    }
    (m as u32, n.max(1) as u32)
}

/// The data and link M/N for `pixel_khz` pixels at `bits_per_pixel` over `lanes` lanes whose
/// symbol clock is `link_khz` (a 2.7 Gbps lane's is 270,000 kHz): data M/N is the payload's
/// share of the link, link M/N the pixel clock over the symbol clock.
pub fn m_n(pixel_khz: u32, bits_per_pixel: u32, link_khz: u32, lanes: u32) -> ((u32, u32), (u32, u32)) {
    let data = reduced(
        u64::from(pixel_khz) * u64::from(bits_per_pixel),
        u64::from(link_khz) * u64::from(lanes.max(1)) * 8,
    );
    let link = reduced(u64::from(pixel_khz), u64::from(link_khz));
    (data, link)
}

/// The symbol clock in kHz of a lane at link rate code `rate` (DPCD `LINK_BW_SET`).
pub fn symbol_clock_khz(rate: u8) -> Option<u32> {
    match rate {
        0x06 => Some(162_000),
        0x0a => Some(270_000),
        0x14 => Some(540_000),
        0x1e => Some(810_000),
        _ => None,
    }
}

/// The display-9 PLL's `DPLL_CTRL1` link-rate code for link rate code `rate`.
pub fn skylake_link_rate(rate: u8) -> Option<u32> {
    match rate {
        0x14 => Some(0),
        0x0a => Some(1),
        0x06 => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_hd() -> Timing {
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
    fn a_mode_s_timings_pack_less_one_high_and_low() {
        let registers = timing_registers(&full_hd());
        assert_eq!(registers[0], (2199 << 16) | 1919);
        assert_eq!(registers[2], ((2008 + 44 - 1) << 16) | 2007);
        assert_eq!(registers[3], (1124 << 16) | 1079);
        assert_eq!(registers[5], ((1084 + 5 - 1) << 16) | 1083);
        assert_eq!(registers[6], (1919 << 16) | 1079);
    }

    #[test]
    fn m_n_pace_pixels_over_the_link() {
        let ((data_m, data_n), (link_m, link_n)) = m_n(148_500, 24, 270_000, 4);
        assert_eq!(u64::from(data_m) * 8_640_000, u64::from(data_n) * 3_564_000);
        assert_eq!(u64::from(link_m) * 270_000, u64::from(link_n) * 148_500);
        assert!(data_m <= MN_LIMIT && data_n <= MN_LIMIT);
        assert_eq!(symbol_clock_khz(0x0a), Some(270_000));
        assert_eq!(skylake_link_rate(0x14), Some(0));
    }
}
