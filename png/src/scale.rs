//! Shrinking an image by a whole factor, a row at a time, without an allocator (D141).
//!
//! The contract: for a factor `k >= 1`, a `w` by `h` image becomes `ceil(w / k)` by
//! `ceil(h / k)`; each output pixel is, per channel of `0x00RRGGBB`, the integer mean rounded
//! down of the source pixels in its `k` by `k` cell that exist (the cells on the right and
//! bottom edges may be smaller). A factor of 1 copies the image.

/// The largest factor [`Downscale`] takes.
pub const MAX_FACTOR: u32 = 4096;

/// The smallest factor `k` with `ceil(width / k) <= max_width`; 1 when `max_width` is 0 or the
/// image already fits.
pub fn factor_for(width: u32, max_width: u32) -> u32 {
    if max_width == 0 || width <= max_width {
        return 1;
    }
    width.div_ceil(max_width).max(1)
}

/// The size a `width` by `height` image has after shrinking by `factor`.
pub fn scaled_size(width: u32, height: u32, factor: u32) -> (u32, u32) {
    let factor = factor.max(1);
    (width.div_ceil(factor), height.div_ceil(factor))
}

/// A shrink in progress: source rows in through [`Self::push_row`], an output row out at the
/// end of each cell's rows.
#[derive(Debug)]
pub struct Downscale<'s> {
    factor: u32,
    width: u32,
    height: u32,
    sums: &'s mut [u64],
    row: u32,
}

impl<'s> Downscale<'s> {
    /// A shrink of a `width` by `height` image by `factor`, summing in `sums`, which needs
    /// three entries per output column; `None` for a factor of 0 or past [`MAX_FACTOR`], an
    /// empty image or too few sums.
    pub fn new(width: u32, height: u32, factor: u32, sums: &'s mut [u64]) -> Option<Self> {
        if factor == 0 || factor > MAX_FACTOR || width == 0 || height == 0 {
            return None;
        }
        let (out_width, _) = scaled_size(width, height, factor);
        let sums = sums.get_mut(..out_width as usize * 3)?;
        sums.fill(0);
        Some(Self {
            factor,
            width,
            height,
            sums,
            row: 0,
        })
    }

    /// The output's width.
    pub fn output_width(&self) -> u32 {
        scaled_size(self.width, self.height, self.factor).0
    }

    /// Adds the next source row (`width` pixels; a shorter one is padded with black, extra
    /// pixels are ignored). When it ends a cell's rows, writes the output row into `out`
    /// ([`Self::output_width`] pixels) and returns true.
    pub fn push_row(&mut self, row: &[u32], out: &mut [u32]) -> bool {
        if self.row >= self.height {
            return false;
        }
        for (column, sums) in self.sums.chunks_exact_mut(3).enumerate() {
            let start = column * self.factor as usize;
            let end = (start + self.factor as usize).min(self.width as usize);
            for &pixel in row.get(start..end.min(row.len())).unwrap_or_default() {
                sums[0] += u64::from(pixel >> 16 & 0xff);
                sums[1] += u64::from(pixel >> 8 & 0xff);
                sums[2] += u64::from(pixel & 0xff);
            }
        }
        self.row += 1;
        let cell_top = (self.row - 1) / self.factor * self.factor;
        let cell_rows = (self.height - cell_top).min(self.factor);
        if self.row - cell_top < cell_rows {
            return false;
        }
        for (column, (sums, place)) in self.sums.chunks_exact_mut(3).zip(out.iter_mut()).enumerate()
        {
            let start = column as u32 * self.factor;
            let cell_columns = (self.width - start).min(self.factor);
            let count = u64::from(cell_columns) * u64::from(cell_rows);
            let mean = |sum: u64| (sum / count) as u32;
            *place = mean(sums[0]) << 16 | mean(sums[1]) << 8 | mean(sums[2]);
        }
        self.sums.fill(0);
        true
    }
}
