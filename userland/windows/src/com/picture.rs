//! The pure parts of `OleLoadPicture`'s picture objects (D231): reading a program's `IStream`
//! into memory, and the HIMETRIC units `IPicture` measures and renders in (0.01 mm, at the
//! 96 pixels per inch of the layer's screens), where a source rectangle's y axis points up.

use super::{E_OUTOFMEMORY, HResult, call_method};
use alloc::vec::Vec;

const READ_SLOT: usize = 3;
const READ_CHUNK: usize = 64 << 10;
const HIMETRIC_PER_INCH: i64 = 2540;
const PIXELS_PER_INCH: i64 = 96;

/// Reads `stream` from its seek pointer: `size` bytes when given (fewer if it ends first),
/// otherwise to its end. More than `limit` bytes is `E_OUTOFMEMORY`, as is a buffer that cannot
/// be allocated; a failing `Read` gives its status.
///
/// # Safety
/// `stream` must be a live `IStream` (or `ISequentialStream`).
pub unsafe fn read_stream(
    stream: usize,
    size: Option<usize>,
    limit: usize,
) -> Result<Vec<u8>, HResult> {
    if size.is_some_and(|size| size > limit) {
        return Err(E_OUTOFMEMORY);
    }
    let mut bytes = Vec::new();
    loop {
        let wanted = match size {
            Some(size) => size.saturating_sub(bytes.len()).min(READ_CHUNK),
            None if bytes.len() >= limit => return Err(E_OUTOFMEMORY),
            None => READ_CHUNK.min(limit - bytes.len()),
        };
        if wanted == 0 {
            return Ok(bytes);
        }
        let start = bytes.len();
        bytes.try_reserve(wanted).map_err(|_| E_OUTOFMEMORY)?;
        bytes.resize(start + wanted, 0);
        let mut got = 0u32;
        let arguments = [
            bytes[start..].as_mut_ptr() as usize,
            wanted,
            &raw mut got as usize,
        ];
        // SAFETY: the caller's duty; `Read` is the stream's fourth method and writes at most
        // `wanted` bytes into the space just made for them.
        let status = unsafe { call_method(stream, READ_SLOT, arguments) } as u32 as i32;
        let got = (got as usize).min(wanted);
        bytes.truncate(start + got);
        if status < 0 {
            return Err(status);
        }
        if got < wanted {
            return Ok(bytes);
        }
    }
}

/// A length of `pixels` in HIMETRIC, rounded to the nearest unit.
pub fn himetric(pixels: u32) -> i32 {
    let units = (i64::from(pixels) * HIMETRIC_PER_INCH + PIXELS_PER_INCH / 2) / PIXELS_PER_INCH;
    i32::try_from(units).unwrap_or(i32::MAX)
}

/// One axis of a picture: its length in pixels and in HIMETRIC, and whether its HIMETRIC
/// coordinates count from the far end (the y axis, which points up).
#[derive(Debug, Clone, Copy)]
pub struct Axis {
    pub pixels: u32,
    pub himetric: i32,
    pub from_far_end: bool,
}

/// The source pixel `IPicture::Render` shows at `destination`, one of `span` destination
/// pixels, when the source runs from `from` for `extent` HIMETRIC units (negative runs
/// backwards); `None` outside the picture.
pub fn source_pixel(
    axis: Axis,
    destination: i32,
    span: i32,
    from: i32,
    extent: i32,
) -> Option<u32> {
    if span <= 0 || axis.himetric <= 0 || axis.pixels == 0 {
        return None;
    }
    let scale = 2 * i64::from(span);
    let along = i64::from(from) * scale + i64::from(extent) * (2 * i64::from(destination) + 1);
    let position = match axis.from_far_end {
        true => i64::from(axis.himetric) * scale - along,
        false => along,
    };
    if position < 0 {
        return None;
    }
    let index = position * i64::from(axis.pixels) / (i64::from(axis.himetric) * scale);
    u32::try_from(index).ok().filter(|&index| index < axis.pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::memory_stream::{self, Stream};
    use crate::com::allocate;

    fn table() -> &'static [usize; 4] {
        static TABLE: std::sync::OnceLock<[usize; 4]> = std::sync::OnceLock::new();
        TABLE.get_or_init(|| {
            [
                memory_stream::query_interface as usize,
                memory_stream::add_reference as usize,
                memory_stream::release as usize,
                memory_stream::read as usize,
            ]
        })
    }

    fn stream_over(bytes: &[u8]) -> *mut Stream {
        let block = allocate(bytes.len());
        // SAFETY: the block holds `bytes.len()` bytes.
        unsafe { block.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
        memory_stream::create(table().as_ptr() as usize, block, bytes.len(), true)
    }

    #[test]
    fn streams_read_whole_in_part_and_within_the_limit() {
        let content: Vec<u8> = (0..200_000u32).map(|value| value as u8).collect();
        let stream = stream_over(&content) as usize;
        // SAFETY: a live stream with a table whose `Read` is the memory stream's.
        let whole = unsafe { read_stream(stream, None, 1 << 20) };
        assert_eq!(whole.as_deref(), Ok(&content[..]));
        let stream = stream_over(&content) as usize;
        // SAFETY: as above.
        let part = unsafe { read_stream(stream, Some(10), 1 << 20) };
        assert_eq!(part.as_deref(), Ok(&content[..10]));
        let stream = stream_over(&content) as usize;
        // SAFETY: as above.
        assert_eq!(unsafe { read_stream(stream, None, 1000) }, Err(E_OUTOFMEMORY));
        // SAFETY: as above.
        assert_eq!(unsafe { read_stream(stream, Some(1001), 1000) }, Err(E_OUTOFMEMORY));
        let empty = stream_over(&[]) as usize;
        // SAFETY: as above.
        assert_eq!(unsafe { read_stream(empty, None, 1000) }, Ok(Vec::new()));
    }

    #[test]
    fn himetric_is_hundredths_of_a_millimetre_at_96_pixels_an_inch() {
        assert_eq!(himetric(96), 2540);
        assert_eq!(himetric(32), 847);
        assert_eq!(himetric(0), 0);
    }

    #[test]
    fn render_maps_the_usual_upright_call_and_a_flipped_one() {
        let rows = Axis {
            pixels: 4,
            himetric: himetric(4),
            from_far_end: true,
        };
        let upright = |row| source_pixel(rows, row, 4, rows.himetric, -rows.himetric);
        assert_eq!([upright(0), upright(3)], [Some(0), Some(3)]);
        let flipped = |row| source_pixel(rows, row, 4, 0, rows.himetric);
        assert_eq!([flipped(0), flipped(3)], [Some(3), Some(0)]);
        let columns = Axis {
            pixels: 4,
            himetric: himetric(4),
            from_far_end: false,
        };
        let doubled = |column| source_pixel(columns, column, 8, 0, columns.himetric);
        assert_eq!([doubled(0), doubled(1), doubled(7)], [Some(0), Some(0), Some(3)]);
        assert_eq!(source_pixel(columns, 0, 0, 0, columns.himetric), None);
        assert_eq!(source_pixel(columns, 9, 8, 0, columns.himetric), None);
    }
}
