//! An app's own icon file (D148, `docs/ux/app-icons-v0.3.md` section 3): the installer writes
//! it beside the manifest, already scaled to the two sizes the shell draws, and the shell only
//! blends its pixels. Exactly [`FILE_BYTES`] bytes, no header: the [`LARGE`] by [`LARGE`]
//! image, then the [`SMALL`] by [`SMALL`] one, each row-major from the top row, one
//! little-endian `u32` per pixel as `0xAARRGGBB` with premultiplied alpha. A file of another
//! length is not an icon.

/// The icon file's name in an app's folder, which the installer writes.
pub const FILE_NAME: &str = "app.icon";

/// The large image's side: the dock, the desktop and the installer's dialogs.
pub const LARGE: usize = 48;

/// The small image's side: the taskbar, the launcher's rows and the switcher.
pub const SMALL: usize = 32;

/// How many pixels the large image has.
pub const LARGE_PIXELS: usize = LARGE * LARGE;

/// How many pixels the small image has.
pub const SMALL_PIXELS: usize = SMALL * SMALL;

/// How many pixels the file holds, large then small.
pub const PIXELS: usize = LARGE_PIXELS + SMALL_PIXELS;

/// The file's length in bytes.
pub const FILE_BYTES: usize = PIXELS * 4;

/// `pixel` with each colour channel clamped to its alpha, so blending it can never overflow.
pub fn sanitised(pixel: u32) -> u32 {
    let alpha = pixel >> 24;
    let clamp = |shift: u32| (pixel >> shift & 0xff).min(alpha);
    alpha << 24 | clamp(16) << 16 | clamp(8) << 8 | clamp(0)
}

/// The pixels of `file`, large image first, each [`sanitised`]; `None` when it is not
/// [`FILE_BYTES`] long.
pub fn pixels(file: &[u8]) -> Option<impl Iterator<Item = u32> + '_> {
    (file.len() == FILE_BYTES).then(|| {
        file.chunks_exact(4)
            .map(|bytes| sanitised(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])))
    })
}

/// Writes `pixels` (large image first, [`PIXELS`] of them) as the file's bytes into `out`.
pub fn write(pixels: &[u32; PIXELS], out: &mut [u8; FILE_BYTES]) {
    for (bytes, pixel) in out.chunks_exact_mut(4).zip(pixels) {
        bytes.copy_from_slice(&pixel.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_holds_the_large_image_then_the_small_one_little_endian() {
        assert_eq!(FILE_BYTES, 13_312);
        let mut pixels = [0u32; PIXELS];
        pixels[0] = 0xff11_2233;
        pixels[LARGE_PIXELS] = 0x8040_2010;
        let mut file = [0u8; FILE_BYTES];
        write(&pixels, &mut file);
        assert_eq!(file[..4], [0x33, 0x22, 0x11, 0xff]);
        assert_eq!(
            file[LARGE_PIXELS * 4..LARGE_PIXELS * 4 + 4],
            [0x10, 0x20, 0x40, 0x80]
        );
        let read: Vec<u32> = super::pixels(&file).unwrap().collect();
        assert_eq!(read, pixels);
    }

    #[test]
    fn channels_above_the_alpha_are_clamped_and_other_lengths_are_no_icon() {
        assert_eq!(sanitised(0x80ff_4080), 0x8080_4080);
        assert_eq!(sanitised(0x00ff_ffff), 0);
        assert_eq!(sanitised(0xffff_ffff), 0xffff_ffff);
        assert!(super::pixels(&[0u8; FILE_BYTES - 1]).is_none());
        assert!(super::pixels(&[0u8; FILE_BYTES + 1]).is_none());
        assert!(super::pixels(&[]).is_none());
    }
}
