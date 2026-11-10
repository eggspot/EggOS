//! Image files decoded to pixels (D231), our own, for the foreign layers that hand programs
//! pictures (GDI+ and `OleLoadPicture` in the Windows layer, `NSImage` in the macOS layer):
//! baseline, extended and progressive JPEG with Huffman coding (ITU T.81), GIF87a and GIF89a
//! with every frame composited, TIFF 6.0 strips (uncompressed, PackBits, LZW and Deflate, with
//! the horizontal predictor), BMP and DIBs (every bit depth, bit fields, RLE4 and RLE8), icons
//! and cursors (`.ico`, `.cur`, their DIB or PNG images) and PNG through `eggos-png`. One entry
//! point, [`decode`], sniffs the format ([`sniff`]); [`decode_animation`] keeps every frame of
//! an animated GIF; [`icon_entries`] and [`decode_icon_entry`] pick one image of an icon.
//!
//! Every file is untrusted: each side, the pixel count, the frame count (and every GIF frame's
//! declared area) and the working buffers' bytes are capped by a [`Limits`], a JPEG's scans
//! by [`MAX_JPEG_SCANS`], every buffer is reserved fallibly ([`Error::OutOfMemory`] instead of an abort),
//! and a damaged file is an [`Error`], never a panic. Pixels are straight-alpha `0xAARRGGBB`,
//! top row first.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod bmp;
mod bytes;
mod gif;
mod ico;
mod jpeg;
mod lzw;
mod png;
mod tiff;

#[cfg(test)]
mod tests;

use alloc::vec::Vec;

pub use ico::IconEntry;

/// The file formats [`decode`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// PNG.
    Png,
    /// JPEG (JFIF, Exif or Adobe).
    Jpeg,
    /// GIF87a or GIF89a.
    Gif,
    /// TIFF, either byte order.
    Tiff,
    /// A Windows bitmap file (`.bmp`).
    Bmp,
    /// A Windows icon (`.ico`).
    Icon,
    /// A Windows cursor (`.cur`).
    Cursor,
}

/// Why a file could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// It is none of the formats in [`Format`].
    UnknownFormat,
    /// A valid file using a feature this decoder does not have (arithmetic coding, 12-bit
    /// JPEG, tiled TIFF, ...).
    Unsupported,
    /// A table, a code, a size or an offset that makes no sense.
    Corrupt,
    /// The file ends before its image does.
    Truncated,
    /// A side, the pixel count, the frame area or the working bytes pass the [`Limits`], or a
    /// JPEG has more than [`MAX_JPEG_SCANS`] scans.
    TooLarge,
    /// A buffer the image needs could not be allocated.
    OutOfMemory,
}

/// The caps on what one file may make the decoder allocate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest width or height, in pixels.
    pub max_side: u32,
    /// The most pixels of one image, and of every frame of an animation together.
    pub max_pixels: u64,
    /// The most frames [`decode_animation`] keeps; later frames are left out. The GIF frames'
    /// declared areas together count toward [`Limits::max_pixels`] too.
    pub max_frames: usize,
    /// The most bytes one decode reserves for working buffers besides the pixels: a JPEG's
    /// sample planes (and a progressive JPEG's coefficients), a TIFF's undecoded rows.
    pub max_working_bytes: u64,
}

impl Limits {
    /// 16 384 pixels a side, 32 Mi pixels (128 MiB of pixels), 1 024 frames and 256 MiB of
    /// working buffers.
    pub const DEFAULT: Limits = Limits {
        max_side: 16_384,
        max_pixels: 1 << 25,
        max_frames: 1024,
        max_working_bytes: 1 << 28,
    };
}

/// The most scans a JPEG may have: each scan walks the whole image, so a file of many tiny
/// scans would otherwise cost far more time than its size.
pub const MAX_JPEG_SCANS: usize = 1000;

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A decoded image: `width * height` straight-alpha `0xAARRGGBB` pixels, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Its width in pixels, at least 1.
    pub width: u32,
    /// Its height in pixels, at least 1.
    pub height: u32,
    /// Its pixels, row after row.
    pub pixels: Vec<u32>,
}

impl Image {
    /// The pixel at `x`, `y`, if it is inside.
    pub fn pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.pixels
            .get(y as usize * self.width as usize + x as usize)
            .copied()
    }
}

/// One frame of an animation, the whole canvas as it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The canvas after this frame was drawn.
    pub image: Image,
    /// How long it shows before the next frame, in milliseconds (0 when the file says none).
    pub delay_ms: u32,
}

/// Every frame of a file: one for a still image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Animation {
    /// The frames, in order, at least one.
    pub frames: Vec<Frame>,
    /// The GIF's `NETSCAPE2.0` loop count when it has one: 0 loops forever, `n` plays the
    /// animation `n` more times; `None` plays it once.
    pub loop_count: Option<u16>,
}

/// Which format `bytes` is in, from its first bytes.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(&eggos_png::SIGNATURE) {
        return Some(Format::Png);
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(Format::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some(Format::Tiff);
    }
    if bmp::is_bmp(bytes) {
        return Some(Format::Bmp);
    }
    ico::kind(bytes)
}

/// Decodes `bytes`, whatever its format: the first frame of a GIF, the first image of a TIFF,
/// the largest and deepest image of an icon or cursor.
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, Error> {
    match sniff(bytes).ok_or(Error::UnknownFormat)? {
        Format::Png => png::decode(bytes, limits),
        Format::Jpeg => jpeg::decode(bytes, limits),
        Format::Gif => gif::decode(bytes, limits, false).and_then(first_frame),
        Format::Tiff => tiff::decode(bytes, limits),
        Format::Bmp => bmp::decode(bytes, limits),
        Format::Icon | Format::Cursor => ico::decode_largest(bytes, limits),
    }
}

/// Decodes every frame of `bytes`: each frame of a GIF composited as GIF89a's disposal
/// methods say, at most [`Limits::max_frames`] and [`Limits::max_pixels`] in all; one frame
/// for every other format.
pub fn decode_animation(bytes: &[u8], limits: &Limits) -> Result<Animation, Error> {
    if sniff(bytes) == Some(Format::Gif) {
        return gif::decode(bytes, limits, true);
    }
    let image = decode(bytes, limits)?;
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(1)
        .map_err(|_| Error::OutOfMemory)?;
    frames.push(Frame { image, delay_ms: 0 });
    Ok(Animation {
        frames,
        loop_count: None,
    })
}

/// The images of an icon or cursor file, in the order of its directory.
pub fn icon_entries(bytes: &[u8]) -> Result<Vec<IconEntry>, Error> {
    ico::entries(bytes)
}

/// Decodes image `index` of an icon or cursor file (from [`icon_entries`]); a DIB image's AND
/// mask makes its masked pixels transparent unless the image has alpha of its own.
pub fn decode_icon_entry(bytes: &[u8], index: usize, limits: &Limits) -> Result<Image, Error> {
    ico::decode_entry(bytes, index, limits)
}

fn first_frame(animation: Animation) -> Result<Image, Error> {
    animation
        .frames
        .into_iter()
        .next()
        .map(|frame| frame.image)
        .ok_or(Error::Corrupt)
}

pub(crate) fn check_size(width: u32, height: u32, limits: &Limits) -> Result<(), Error> {
    if width == 0 || height == 0 {
        return Err(Error::Corrupt);
    }
    let pixels = u64::from(width) * u64::from(height);
    if width > limits.max_side || height > limits.max_side || pixels > limits.max_pixels {
        return Err(Error::TooLarge);
    }
    Ok(())
}

pub(crate) fn check_working_bytes(bytes: Option<u64>, limits: &Limits) -> Result<(), Error> {
    match bytes {
        Some(bytes) if bytes <= limits.max_working_bytes => Ok(()),
        _ => Err(Error::TooLarge),
    }
}

pub(crate) fn buffer<T: Clone>(count: usize, value: T) -> Result<Vec<T>, Error> {
    let mut items = Vec::new();
    items
        .try_reserve_exact(count)
        .map_err(|_| Error::OutOfMemory)?;
    items.resize(count, value);
    Ok(items)
}

pub(crate) fn canvas(width: u32, height: u32, limits: &Limits) -> Result<Image, Error> {
    check_size(width, height, limits)?;
    Ok(Image {
        width,
        height,
        pixels: buffer(width as usize * height as usize, 0)?,
    })
}

pub(crate) const OPAQUE: u32 = 0xff00_0000;

pub(crate) fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    OPAQUE | u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue)
}

pub(crate) fn argb(alpha: u8, red: u8, green: u8, blue: u8) -> u32 {
    u32::from(alpha) << 24 | u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue)
}
