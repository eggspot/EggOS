//! YUV pictures shown as `0x00RRGGBB` pixels (D587): the planes of DRM's `NV12` (a luma plane
//! and an interleaved half-resolution chroma plane) and `YUV420` (three planes), converted with
//! ITU-R BT.601's limited-range coefficients in integer arithmetic, as video decoders write
//! them.

/// DRM's `NV12` four-character code.
pub const NV12: u32 = 0x3231_564e;
/// DRM's `YUV420` four-character code.
pub const YUV420: u32 = 0x3231_5559;

/// How many planes `format` has; `None` for a format that is not YUV.
pub const fn planes_of(format: u32) -> Option<usize> {
    match format {
        NV12 => Some(2),
        YUV420 => Some(3),
        _ => None,
    }
}

/// The bytes plane `plane` of `format` needs at `stride` for a picture `height` rows high.
pub const fn plane_bytes(format: u32, plane: usize, stride: usize, height: usize) -> usize {
    match (format, plane) {
        (_, 0) => stride * height,
        (NV12 | YUV420, _) => stride * height.div_ceil(2),
        _ => 0,
    }
}

fn clamp(value: i32) -> u32 {
    value.clamp(0, 255) as u32
}

/// One pixel from its luma and chroma bytes.
pub fn pixel(y: u8, u: u8, v: u8) -> u32 {
    let c = i32::from(y) - 16;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let red = clamp((298 * c + 409 * e + 128) >> 8);
    let green = clamp((298 * c - 100 * d - 208 * e + 128) >> 8);
    let blue = clamp((298 * c + 516 * d + 128) >> 8);
    (red << 16) | (green << 8) | blue
}

/// One plane: its bytes from the picture's first row and the bytes from one row to the next.
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    pub bytes: &'a [u8],
    pub stride: usize,
}

/// Converts the `width` by `height` picture in `planes` of `format` into `out`, rows
/// `out_stride` pixels apart; rows or planes too short are left as they are.
pub fn convert(format: u32, planes: &[Plane<'_>], width: usize, height: usize, out: &mut [u32], out_stride: usize) {
    let Some(count) = planes_of(format) else {
        return;
    };
    if planes.len() < count {
        return;
    }
    for row in 0..height {
        let Some(luma) = planes[0].bytes.get(row * planes[0].stride..row * planes[0].stride + width) else {
            return;
        };
        let Some(target) = out.get_mut(row * out_stride..row * out_stride + width) else {
            return;
        };
        let chroma_row = row / 2;
        for (column, pixel_out) in target.iter_mut().enumerate() {
            let (u, v) = match format {
                NV12 => {
                    let at = chroma_row * planes[1].stride + (column / 2) * 2;
                    (
                        planes[1].bytes.get(at).copied().unwrap_or(128),
                        planes[1].bytes.get(at + 1).copied().unwrap_or(128),
                    )
                }
                _ => {
                    let u = planes[1].bytes.get(chroma_row * planes[1].stride + column / 2);
                    let v = planes[2].bytes.get(chroma_row * planes[2].stride + column / 2);
                    (u.copied().unwrap_or(128), v.copied().unwrap_or(128))
                }
            };
            *pixel_out = pixel(luma[column], u, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_white_and_red_convert_to_their_colours() {
        assert_eq!(pixel(16, 128, 128), 0x00_0000);
        assert_eq!(pixel(235, 128, 128), 0xff_ffff);
        let red = pixel(81, 90, 240);
        assert!(red >> 16 > 0xf0 && (red >> 8) & 0xff < 0x10 && red & 0xff < 0x10, "{red:06x}");
    }

    #[test]
    fn an_nv12_picture_fills_every_pixel_from_its_shared_chroma() {
        let luma = [235u8; 4 * 2];
        let chroma = [128u8; 4];
        let planes = [
            Plane { bytes: &luma, stride: 4 },
            Plane { bytes: &chroma, stride: 4 },
        ];
        let mut out = [0u32; 8];
        convert(NV12, &planes, 4, 2, &mut out, 4);
        assert!(out.iter().all(|&pixel| pixel == 0xff_ffff));
        assert_eq!(planes_of(YUV420), Some(3));
        assert_eq!(plane_bytes(NV12, 1, 4, 3), 8);
    }
}
