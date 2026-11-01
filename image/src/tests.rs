use crate::*;

const PATTERN: &[u8] = include_bytes!("../testdata/pattern.bmp");
const BASELINE_420: &[u8] = include_bytes!("../testdata/pattern-baseline-420.jpg");
const BASELINE_420_DECODED: &[u8] = include_bytes!("../testdata/pattern-baseline-420.bmp");
const BASELINE_444_RESTART: &[u8] = include_bytes!("../testdata/pattern-baseline-444-restart.jpg");
const BASELINE_444_RESTART_DECODED: &[u8] =
    include_bytes!("../testdata/pattern-baseline-444-restart.bmp");
const PROGRESSIVE_420: &[u8] = include_bytes!("../testdata/pattern-progressive-420.jpg");
const PROGRESSIVE_420_DECODED: &[u8] = include_bytes!("../testdata/pattern-progressive-420.bmp");
const PROGRESSIVE_422_RESTART: &[u8] =
    include_bytes!("../testdata/pattern-progressive-422-restart.jpg");
const PROGRESSIVE_422_RESTART_DECODED: &[u8] =
    include_bytes!("../testdata/pattern-progressive-422-restart.bmp");
const GREY_PROGRESSIVE: &[u8] = include_bytes!("../testdata/pattern-grey-progressive.jpg");
const GREY_PROGRESSIVE_DECODED: &[u8] = include_bytes!("../testdata/pattern-grey-progressive.bmp");
const PHOTO: &[u8] = include_bytes!("../testdata/photo-640x480.jpg");
const GRADIENT_GIF: &[u8] = include_bytes!("../testdata/gradient.gif");
const GRADIENT_GIF_DECODED: &[u8] = include_bytes!("../testdata/gradient-gif.bmp");
const ANIMATED_GIF: &[u8] = include_bytes!("../testdata/animated.gif");
const CODES_GIF: &[u8] = include_bytes!("../testdata/codes.gif");
const TIFF_NONE: &[u8] = include_bytes!("../testdata/pattern-none.tiff");
const TIFF_PACKBITS: &[u8] = include_bytes!("../testdata/pattern-packbits.tiff");
const TIFF_LZW: &[u8] = include_bytes!("../testdata/pattern-lzw.tiff");
const TIFF_BILEVEL: &[u8] = include_bytes!("../testdata/bilevel-packbits.tiff");
const TIFF_GREY16: &[u8] = include_bytes!("../testdata/grey16-be.tiff");
const TIFF_PALETTE4: &[u8] = include_bytes!("../testdata/palette4-be.tiff");
const TIFF_RGBA: &[u8] = include_bytes!("../testdata/rgba-deflate-predictor.tiff");

const RED: u32 = 0xffff_0000;
const GREEN: u32 = 0xff00_ff00;
const BLUE: u32 = 0xff00_00ff;
const BLACK: u32 = 0xff00_0000;
const WHITE: u32 = 0xffff_ffff;

fn decoded(bytes: &[u8]) -> Image {
    decode(bytes, &Limits::DEFAULT).expect("the fixture decodes")
}

fn difference(ours: &Image, reference: &Image) -> (f64, u32) {
    assert_eq!((ours.width, ours.height), (reference.width, reference.height));
    let (mut total, mut largest) = (0u64, 0u32);
    for (&a, &b) in ours.pixels.iter().zip(&reference.pixels) {
        for shift in [0, 8, 16] {
            let gap = ((a >> shift) & 0xff).abs_diff((b >> shift) & 0xff);
            total += u64::from(gap);
            largest = largest.max(gap);
        }
    }
    (total as f64 / (ours.pixels.len() * 3) as f64, largest)
}

fn icon_file(kind: u16, hotspot: (u16, u16)) -> Vec<u8> {
    let mut dib = Vec::new();
    for value in [40u32, 2, 4] {
        dib.extend_from_slice(&value.to_le_bytes());
    }
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&24u16.to_le_bytes());
    dib.extend_from_slice(&[0; 24]);
    dib.extend_from_slice(&[0xff, 0, 0, 0xff, 0xff, 0xff, 0, 0]);
    dib.extend_from_slice(&[0, 0, 0xff, 0, 0xff, 0, 0, 0]);
    dib.extend_from_slice(&[0x40, 0, 0, 0, 0, 0, 0, 0]);
    let mut file = Vec::new();
    for value in [0u16, kind, 1] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    file.extend_from_slice(&[2, 2, 0, 0]);
    file.extend_from_slice(&hotspot.0.to_le_bytes());
    file.extend_from_slice(&hotspot.1.to_le_bytes());
    file.extend_from_slice(&(dib.len() as u32).to_le_bytes());
    file.extend_from_slice(&22u32.to_le_bytes());
    file.extend_from_slice(&dib);
    file
}

fn every_fixture() -> Vec<&'static [u8]> {
    vec![
        PATTERN,
        BASELINE_420,
        BASELINE_444_RESTART,
        PROGRESSIVE_420,
        PROGRESSIVE_422_RESTART,
        GREY_PROGRESSIVE,
        PHOTO,
        GRADIENT_GIF,
        ANIMATED_GIF,
        CODES_GIF,
        TIFF_NONE,
        TIFF_PACKBITS,
        TIFF_LZW,
        TIFF_BILEVEL,
        TIFF_GREY16,
        TIFF_PALETTE4,
        TIFF_RGBA,
    ]
}

#[test]
fn formats_are_told_apart_by_their_first_bytes() {
    assert_eq!(sniff(BASELINE_420), Some(Format::Jpeg));
    assert_eq!(sniff(GRADIENT_GIF), Some(Format::Gif));
    assert_eq!(sniff(TIFF_NONE), Some(Format::Tiff));
    assert_eq!(sniff(TIFF_GREY16), Some(Format::Tiff));
    assert_eq!(sniff(PATTERN), Some(Format::Bmp));
    assert_eq!(sniff(&eggos_png::SIGNATURE), Some(Format::Png));
    assert_eq!(sniff(&icon_file(1, (0, 0))), Some(Format::Icon));
    assert_eq!(sniff(&icon_file(2, (1, 0))), Some(Format::Cursor));
    assert_eq!(sniff(b"plain text"), None);
    assert_eq!(decode(b"plain text", &Limits::DEFAULT), Err(Error::UnknownFormat));
}

#[test]
fn baseline_jpeg_matches_a_reference_decoder() {
    let (mean, largest) = difference(
        &decoded(BASELINE_444_RESTART),
        &decoded(BASELINE_444_RESTART_DECODED),
    );
    assert!(mean < 1.5 && largest <= 24, "4:4:4 off by {mean} on average, {largest} at most");
    let (mean, _) = difference(&decoded(BASELINE_420), &decoded(BASELINE_420_DECODED));
    assert!(mean < 4.0, "4:2:0 off by {mean} on average");
}

#[test]
fn progressive_jpeg_matches_a_reference_decoder_and_its_baseline_twin() {
    let progressive = decoded(PROGRESSIVE_420);
    assert_eq!(progressive, decoded(BASELINE_420));
    let (mean, _) = difference(&progressive, &decoded(PROGRESSIVE_420_DECODED));
    assert!(mean < 4.0, "progressive 4:2:0 off by {mean} on average");
    let (mean, _) = difference(
        &decoded(PROGRESSIVE_422_RESTART),
        &decoded(PROGRESSIVE_422_RESTART_DECODED),
    );
    assert!(mean < 4.0, "progressive 4:2:2 with restarts off by {mean} on average");
    let (mean, largest) = difference(
        &decoded(GREY_PROGRESSIVE),
        &decoded(GREY_PROGRESSIVE_DECODED),
    );
    assert!(mean < 1.5 && largest <= 24, "grey off by {mean} on average, {largest} at most");
}

#[test]
fn a_photo_decodes_at_its_size_opaque() {
    let photo = decoded(PHOTO);
    assert_eq!((photo.width, photo.height), (640, 480));
    assert!(photo.pixels.iter().all(|pixel| pixel >> 24 == 0xff));
    assert!(photo.pixels.iter().any(|&pixel| pixel != photo.pixels[0]));
}

#[test]
fn gif_matches_its_reference_and_keeps_wide_codes() {
    assert_eq!(decoded(GRADIENT_GIF), decoded(GRADIENT_GIF_DECODED));
    let codes = decoded(CODES_GIF);
    assert_eq!((codes.width, codes.height), (50, 40));
    assert_eq!(codes.pixel(0, 0), Some(0xff00_ff00));
    assert_eq!(codes.pixel(1, 0), Some(0xff07_f815));
    assert_eq!(codes.pixel(49, 0), Some(0xff57_a805));
    assert_eq!(codes.pixel(3, 20), Some(0xff25_da6f));
    assert_eq!(codes.pixel(49, 39), Some(0xff25_da6f));
}

#[test]
fn animated_gif_frames_follow_their_disposal() {
    let animation = decode_animation(ANIMATED_GIF, &Limits::DEFAULT).unwrap();
    assert_eq!(animation.loop_count, Some(0));
    let delays: Vec<u32> = animation.frames.iter().map(|frame| frame.delay_ms).collect();
    assert_eq!(delays, [100, 200, 0, 0]);
    let at = |frame: usize, x: u32, y: u32| animation.frames[frame].image.pixel(x, y).unwrap();
    assert!(animation.frames[0].image.pixels.iter().all(|&pixel| pixel == RED));
    assert_eq!(at(1, 2, 1), GREEN);
    assert_eq!(at(1, 4, 1), RED);
    assert_eq!(at(1, 5, 2), GREEN);
    assert_eq!(at(2, 0, 0), WHITE);
    assert_eq!(at(2, 1, 0), 0xff0a_141e);
    assert_eq!(at(2, 1, 5), WHITE);
    assert_eq!(at(2, 2, 1), 0);
    assert_eq!(at(2, 5, 3), 0);
    assert_eq!(at(3, 0, 0), RED);
    assert_eq!(at(3, 1, 4), RED);
    assert_eq!(at(3, 3, 2), 0);
    assert_eq!(at(3, 7, 5), BLUE);
    assert_eq!(decoded(ANIMATED_GIF), animation.frames[0].image);
    let two = Limits {
        max_frames: 2,
        ..Limits::DEFAULT
    };
    assert_eq!(decode_animation(ANIMATED_GIF, &two).unwrap().frames.len(), 2);
}

#[test]
fn tiff_strips_decode_in_every_compression() {
    let pattern = decoded(PATTERN);
    assert_eq!(decoded(TIFF_NONE), pattern);
    assert_eq!(decoded(TIFF_PACKBITS), pattern);
    assert_eq!(decoded(TIFF_LZW), pattern);
}

#[test]
fn tiff_bit_depths_palettes_and_alpha() {
    let bilevel = decoded(TIFF_BILEVEL);
    assert_eq!((bilevel.width, bilevel.height), (13, 5));
    assert_eq!(bilevel.pixel(0, 0), Some(BLACK));
    assert_eq!(bilevel.pixel(1, 0), Some(WHITE));
    assert_eq!(bilevel.pixel(12, 2), Some(BLACK));
    let grey = decoded(TIFF_GREY16);
    assert_eq!(grey.pixel(1, 0), Some(0xff27_2727));
    assert_eq!(grey.pixel(5, 3), Some(0xffe6_e6e6));
    let palette = decoded(TIFF_PALETTE4);
    assert_eq!(palette.pixel(0, 0), Some(BLACK));
    assert_eq!(palette.pixel(9, 0), Some(0xff90_6ff0));
    assert_eq!(palette.pixel(0, 6), Some(0xff60_9fa0));
    let rgba = decoded(TIFF_RGBA);
    assert_eq!((rgba.width, rgba.height), (9, 4));
    assert_eq!(rgba.pixel(0, 0), Some(0));
    assert_eq!(rgba.pixel(2, 0), Some(argb(60, 38, 0, 128)));
    assert_eq!(rgba.pixel(4, 2), Some(argb(120, 79, 100, 128)));
}

#[test]
fn icons_and_cursors_list_and_mask_their_images() {
    let icon = icon_file(1, (1, 24));
    let entries = icon_entries(&icon).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!((entries[0].width, entries[0].height), (2, 2));
    assert_eq!((entries[0].bit_count, entries[0].hotspot), (24, None));
    let image = decoded(&icon);
    assert_eq!(image.pixels, [RED, GREEN, BLUE, 0]);
    assert_eq!(decode_icon_entry(&icon, 0, &Limits::DEFAULT), Ok(image));
    assert_eq!(decode_icon_entry(&icon, 1, &Limits::DEFAULT), Err(Error::Corrupt));
    let cursor = icon_file(2, (1, 0));
    assert_eq!(icon_entries(&cursor).unwrap()[0].hotspot, Some((1, 0)));
}

#[test]
fn limits_refuse_large_images_in_every_format() {
    let tiny = Limits {
        max_side: 8,
        ..Limits::DEFAULT
    };
    for bytes in [PATTERN, BASELINE_420, PROGRESSIVE_420, GRADIENT_GIF, TIFF_NONE] {
        assert_eq!(decode(bytes, &tiny), Err(Error::TooLarge));
    }
    let few = Limits {
        max_pixels: 100,
        ..Limits::DEFAULT
    };
    assert_eq!(decode(PHOTO, &few), Err(Error::TooLarge));
}

#[test]
fn truncated_files_fail_or_decode_without_panicking() {
    for bytes in every_fixture() {
        let step = match bytes.len() > 8192 {
            true => bytes.len() / 12,
            false => (bytes.len() / 200).max(1),
        };
        for length in (0..bytes.len()).step_by(step) {
            let _ = decode(&bytes[..length], &Limits::DEFAULT);
            if sniff(bytes) == Some(Format::Gif) {
                let _ = decode_animation(&bytes[..length], &Limits::DEFAULT);
            }
        }
    }
    assert!(decode(&BASELINE_420[..BASELINE_420.len() / 2], &Limits::DEFAULT).is_err());
    assert!(decode(&TIFF_LZW[..100], &Limits::DEFAULT).is_err());
    assert!(decode(&GRADIENT_GIF[..12], &Limits::DEFAULT).is_err());
}

#[test]
fn damaged_files_fail_or_decode_without_panicking() {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for bytes in every_fixture() {
        let rounds = if bytes.len() > 8192 { 6 } else { 160 };
        for _ in 0..rounds {
            let mut damaged = bytes.to_vec();
            for _ in 0..1 + next() % 4 {
                let at = (next() % damaged.len() as u64) as usize;
                damaged[at] ^= (next() % 255 + 1) as u8;
            }
            let _ = decode(&damaged, &Limits::DEFAULT);
            if sniff(bytes) == Some(Format::Gif) {
                let _ = decode_animation(&damaged, &Limits::DEFAULT);
            }
        }
    }
}

#[test]
fn damaged_headers_are_errors() {
    let mut tiff = TIFF_NONE.to_vec();
    tiff[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(decode(&tiff, &Limits::DEFAULT), Err(Error::Truncated));
    let mut gif = GRADIENT_GIF.to_vec();
    gif[6..10].fill(0);
    assert_eq!(decode(&gif, &Limits::DEFAULT), Err(Error::Corrupt));
    let mut jpeg = BASELINE_420.to_vec();
    let frame = jpeg
        .windows(4)
        .position(|marker| marker == [0xff, 0xc0, 0x00, 0x11])
        .unwrap();
    jpeg[frame + 1] = 0xc9;
    assert_eq!(decode(&jpeg, &Limits::DEFAULT), Err(Error::Unsupported));
}
