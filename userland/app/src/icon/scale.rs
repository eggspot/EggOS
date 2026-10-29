//! Scaling a square icon image to a target side (docs/ux/app-icons-v0.3.md section 4), on
//! premultiplied `0xAARRGGBB`, in integers: a larger image by area average (source pixel `x`
//! covers `[x * T, (x + 1) * T)` and destination pixel `j` covers `[j * S, (j + 1) * S)` in units
//! of `1 / (S * T)`, so each destination pixel's weights sum to `S * S`), a smaller or equal one
//! by nearest neighbour at the largest whole factor, centred, the rest transparent. Rows may
//! arrive in any order.

/// `pixel`, straight `0xAARRGGBB`, with its colour channels scaled by its alpha, rounded.
pub fn premultiplied(pixel: u32) -> u32 {
    let alpha = pixel >> 24;
    let scale = |shift: u32| ((pixel >> shift & 0xff) * alpha + 127) / 255;
    alpha << 24 | scale(16) << 16 | scale(8) << 8 | scale(0)
}

fn overlap(source: usize, destination: usize, side: usize, target: usize) -> u32 {
    let start = (source * target).max(destination * side);
    let end = ((source + 1) * target).min((destination + 1) * side);
    end.saturating_sub(start) as u32
}

/// One image being scaled into `out` (`target * target` pixels), summing in `sums` (four per
/// output pixel).
pub struct Scaler<'a> {
    target: usize,
    side: usize,
    factor: usize,
    offset: usize,
    sums: &'a mut [u32],
    out: &'a mut [u32],
}

impl core::fmt::Debug for Scaler<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Scaler")
            .field("target", &self.target)
            .field("side", &self.side)
            .finish_non_exhaustive()
    }
}

impl<'a> Scaler<'a> {
    /// Starts scaling a `side` by `side` image (1 to 1024) to `target` by `target`, clearing
    /// `out` to transparent.
    pub fn new(target: usize, side: usize, sums: &'a mut [u32], out: &'a mut [u32]) -> Self {
        let out = &mut out[..target * target];
        out.fill(0);
        let sums = &mut sums[..target * target * 4];
        sums.fill(0);
        let side = side.max(1);
        let factor = (target / side).max(1);
        let offset = target.saturating_sub(factor * side) / 2;
        Self {
            target,
            side,
            factor,
            offset,
            sums,
            out,
        }
    }

    /// Adds source row `y`: `side` straight-alpha pixels.
    pub fn row(&mut self, y: usize, row: &[u32]) {
        if y >= self.side {
            return;
        }
        let (side, target) = (self.side, self.target);
        if side <= target {
            let factor = self.factor;
            for (x, &pixel) in row.iter().enumerate().take(side) {
                let value = premultiplied(pixel);
                for dy in 0..factor {
                    let line = (self.offset + y * factor + dy) * target + self.offset + x * factor;
                    self.out[line..line + factor].fill(value);
                }
            }
            return;
        }
        let rows = (y * target) / side..=((y + 1) * target - 1) / side;
        for i in rows {
            let weight_y = overlap(y, i, side, target);
            for (x, &pixel) in row.iter().enumerate().take(side) {
                let value = premultiplied(pixel);
                for j in (x * target) / side..=((x + 1) * target - 1) / side {
                    let weight = weight_y * overlap(x, j, side, target);
                    let at = (i * target + j) * 4;
                    for (sum, shift) in self.sums[at..at + 4].iter_mut().zip([24, 16, 8, 0]) {
                        *sum += (value >> shift & 0xff) * weight;
                    }
                }
            }
        }
    }

    /// Ends the image: for a larger source, writes each output pixel's rounded means.
    pub fn finish(self) {
        if self.side <= self.target {
            return;
        }
        let total = (self.side * self.side) as u32;
        for (pixel, sums) in self.out.iter_mut().zip(self.sums.as_chunks::<4>().0) {
            let mean = |channel: usize| (sums[channel] + total / 2) / total;
            *pixel = mean(0) << 24 | mean(1) << 16 | mean(2) << 8 | mean(3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaled(target: usize, side: usize, pixels: &[u32]) -> Vec<u32> {
        let mut sums = vec![0u32; target * target * 4];
        let mut out = vec![0xdead_beef; target * target];
        let mut scaler = Scaler::new(target, side, &mut sums, &mut out);
        for (y, row) in pixels.chunks(side).enumerate().rev() {
            scaler.row(y, row);
        }
        scaler.finish();
        out
    }

    #[test]
    fn premultiplying_rounds_to_the_nearest() {
        assert_eq!(premultiplied(0x80ff_ff00), 0x8080_8000);
        assert_eq!(premultiplied(0xffab_cdef), 0xffab_cdef);
        assert_eq!(premultiplied(0x00ff_ffff), 0);
    }

    #[test]
    fn a_whole_ratio_is_the_mean_of_each_block() {
        let pixels = [
            0xff00_0000,
            0xff00_0004,
            0xff00_0008,
            0xff00_000c,
            0xff00_0010,
            0xff00_0014,
            0xff00_0018,
            0xff00_001c,
            0x0000_0000,
            0x0000_0000,
            0xffff_ffff,
            0xffff_ffff,
            0x0000_0000,
            0x0000_0000,
            0xffff_ffff,
            0xff00_0000,
        ];
        assert_eq!(
            scaled(2, 4, &pixels),
            [0xff00_000a, 0xff00_0012, 0x0000_0000, 0xffbf_bfbf]
        );
    }

    #[test]
    fn a_fractional_ratio_weighs_each_overlap() {
        let mut pixels = vec![0xff00_0000; 36];
        pixels[1] = 0xff00_00ff;
        let out = scaled(4, 6, &pixels);
        assert_eq!(out[0], 0xff00_0039);
        assert_eq!(out[1], 0xff00_0039);
        assert_eq!(out[2], 0xff00_0000);
        assert_eq!(scaled(4, 6, &[0x8040_2010; 36]), [0x8020_1008; 16]);
    }

    #[test]
    fn a_smaller_image_grows_by_a_whole_factor_centred() {
        let out = scaled(48, 16, &[0xff12_3456; 256]);
        assert_eq!(
            out.iter().filter(|&&pixel| pixel == 0xff12_3456).count(),
            48 * 48
        );
        let out = scaled(48, 32, &[0xff12_3456; 1024]);
        assert_eq!(out[8 * 48 + 8], 0xff12_3456);
        assert_eq!(out[8 * 48 + 7], 0);
        assert_eq!(out[39 * 48 + 39], 0xff12_3456);
        assert_eq!(out[40 * 48 + 40], 0);
        let out = scaled(32, 16, &[0x80ff_ffff; 256]);
        assert!(out.iter().all(|&pixel| pixel == 0x8080_8080));
        let out = scaled(32, 32, &[0xff01_0203; 1024]);
        assert!(out.iter().all(|&pixel| pixel == 0xff01_0203));
    }
}
