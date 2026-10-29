use super::scale::{Downscale, MAX_FACTOR, factor_for, scaled_size};
use super::*;

struct Bytes(Vec<u8>);

impl PngSink for Bytes {
    type Error = ();

    fn write(&mut self, bytes: &[u8]) -> Result<(), ()> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

struct Refusing(usize);

impl PngSink for Refusing {
    type Error = &'static str;

    fn write(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        match self.0.checked_sub(bytes.len()) {
            Some(left) => {
                self.0 = left;
                Ok(())
            }
            None => Err("full"),
        }
    }
}

fn encode(width: u32, height: u32, pixels: &[u32]) -> Vec<u8> {
    let mut work = Box::new(EncoderWorkspace::new());
    let mut encoder = Encoder::new(width, height, &mut work, Bytes(Vec::new())).unwrap();
    for row in pixels.chunks(width as usize) {
        encoder.write_row(row).unwrap();
    }
    encoder.finish().unwrap().0
}

fn decoded(png: &[u8]) -> (u32, u32, Vec<u32>) {
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    let mut inflated = vec![0u8; decoded_size(width, height).unwrap()];
    let mut pixels = vec![0u32; (width * height) as usize];
    assert_eq!(
        decode(png, &mut inflated, &mut pixels),
        Ok((width, height))
    );
    (width, height, pixels)
}

fn noise(count: usize, seed: u32) -> Vec<u32> {
    let mut state = seed | 1;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state & 0x00ff_ffff
        })
        .collect()
}

fn chunks(png: &[u8]) -> Vec<([u8; 4], usize)> {
    let mut at = 8;
    let mut found = Vec::new();
    while at < png.len() {
        let length = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        found.push((png[at + 4..at + 8].try_into().unwrap(), length));
        at += 12 + length;
    }
    found
}

#[test]
fn images_round_trip_through_the_decoder() {
    for (width, height) in [(1, 1), (2, 3), (17, 5), (255, 2), (4097, 3)] {
        let flat = vec![0x0012_3456; (width * height) as usize];
        let varied = noise((width * height) as usize, width + height);
        for pixels in [flat, varied] {
            let png = encode(width, height, &pixels);
            assert_eq!(png[..8], SIGNATURE);
            assert_eq!(decoded(&png), (width, height, pixels), "{width}x{height}");
        }
    }
}

#[test]
fn data_past_the_history_and_many_chunks_still_round_trip() {
    let (width, height) = (640, 200);
    let mut pixels = noise(width * height / 2, 7);
    pixels.extend((0..width * height / 2).map(|index| (index as u32 / 37) * 0x0001_0203));
    let png = encode(width as u32, height as u32, &pixels);
    let idat: Vec<usize> = chunks(&png)
        .into_iter()
        .filter(|(kind, _)| kind == b"IDAT")
        .map(|(_, length)| length)
        .collect();
    assert!(idat.len() > 2, "{idat:?}");
    assert!(idat[..idat.len() - 1].iter().all(|&length| length == IDAT_BYTES));
    assert_eq!(decoded(&png).2, pixels);
}

#[test]
fn flat_images_compress_well() {
    let pixels = vec![0x00f0_f0e8; 1024 * 768];
    assert!(encode(1024, 768, &pixels).len() < 64 * 1024);
}

#[test]
fn an_all_zero_workspace_encodes_and_one_reused_encodes_the_same() {
    let mut work = Box::new(EncoderWorkspace::new());
    let pixels = noise(300 * 40, 3);
    let mut first = Encoder::new(300, 40, &mut work, Bytes(Vec::new())).unwrap();
    for row in pixels.chunks(300) {
        first.write_row(row).unwrap();
    }
    let first = first.finish().unwrap().0;
    let mut again = Encoder::new(300, 40, &mut work, Bytes(Vec::new())).unwrap();
    for row in pixels.chunks(300) {
        again.write_row(row).unwrap();
    }
    assert_eq!(again.finish().unwrap().0, first);
}

#[test]
fn the_header_and_every_chunk_are_checked() {
    let png = encode(2, 1, &[0x00ff_0000, 0x0000_ff00]);
    assert_eq!(&png[12..16], b"IHDR");
    assert_eq!(&png[16..29], &[0, 0, 0, 2, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
    assert_eq!(crc32(&png[12..29]).to_be_bytes(), png[29..33]);
    assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    let mut inflated = [0u8; 64];
    let mut pixels = [0u32; 2];
    for at in [20, 40, png.len() - 6] {
        let mut broken = png.clone();
        broken[at] ^= 1;
        assert!(decode(&broken, &mut inflated, &mut pixels).is_err(), "byte {at}");
    }
    assert_eq!(
        decode(&png[..png.len() - 3], &mut inflated, &mut pixels),
        Err(DecodeError::Truncated)
    );
    assert_eq!(
        decode(b"GIF89a", &mut inflated, &mut pixels),
        Err(DecodeError::NotPng)
    );
    assert_eq!(
        decode(&png, &mut inflated[..3], &mut pixels),
        Err(DecodeError::BufferTooSmall)
    );
}

#[test]
fn rows_of_the_wrong_width_or_number_are_refused() {
    let mut work = Box::new(EncoderWorkspace::new());
    assert!(matches!(
        Encoder::new(0, 1, &mut work, Bytes(Vec::new())),
        Err(EncodeError::BadSize)
    ));
    assert!(matches!(
        Encoder::new(65535, 65535, &mut work, Bytes(Vec::new())),
        Err(EncodeError::BadSize)
    ));
    let mut encoder = Encoder::new(2, 1, &mut work, Bytes(Vec::new())).unwrap();
    assert_eq!(encoder.write_row(&[0]), Err(EncodeError::WrongRow));
    encoder.write_row(&[0, 0]).unwrap();
    assert_eq!(encoder.write_row(&[0, 0]), Err(EncodeError::TooManyRows));
    let encoder = Encoder::new(2, 2, &mut work, Bytes(Vec::new())).unwrap();
    assert!(matches!(encoder.finish(), Err(EncodeError::MissingRows)));
}

#[test]
fn a_sink_s_refusal_is_returned() {
    let mut work = Box::new(EncoderWorkspace::new());
    assert!(matches!(
        Encoder::new(1, 1, &mut work, Refusing(10)),
        Err(EncodeError::Sink("full"))
    ));
    let pixels = noise(512 * 64, 9);
    let mut encoder = Encoder::new(512, 64, &mut work, Refusing(100)).unwrap();
    let mut refused = false;
    for row in pixels.chunks(512) {
        refused |= encoder.write_row(row) == Err(EncodeError::Sink("full"));
    }
    assert!(refused || encoder.finish().is_err());
}

#[test]
fn checksums_match_their_reference_values() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(crc32_update(crc32(b"1234"), b"56789"), 0xcbf4_3926);
    let mut adler = Adler32::new();
    adler.update(b"Wikipedia");
    assert_eq!(adler.value(), 0x11e6_0398);
    let mut long = Adler32::new();
    long.update(&vec![0xff; 100_000]);
    let (mut a, mut b) = (1u64, 0u64);
    for _ in 0..100_000 {
        a = (a + 0xff) % 65521;
        b = (b + a) % 65521;
    }
    assert_eq!(long.value(), (b << 16 | a) as u32);
}

#[test]
fn the_factor_is_the_smallest_that_fits_the_width() {
    assert_eq!(factor_for(1200, 0), 1);
    assert_eq!(factor_for(1200, 1200), 1);
    assert_eq!(factor_for(1200, 600), 2);
    assert_eq!(factor_for(1201, 600), 3);
    assert_eq!(factor_for(640, 320), 2);
    assert_eq!(factor_for(641, 321), 2);
    for (width, max) in [(1000, 7), (4096, 1024), (333, 100)] {
        let factor = factor_for(width, max);
        assert!(width.div_ceil(factor) <= max);
        assert!(factor == 1 || width.div_ceil(factor - 1) > max);
    }
    assert_eq!(scaled_size(5, 3, 2), (3, 2));
}

fn shrink(width: u32, height: u32, factor: u32, pixels: &[u32]) -> Vec<u32> {
    let mut sums = vec![0u64; 3 * width as usize];
    let mut downscale = Downscale::new(width, height, factor, &mut sums).unwrap();
    let mut row = vec![0u32; downscale.output_width() as usize];
    let mut out = Vec::new();
    for source in pixels.chunks(width as usize) {
        if downscale.push_row(source, &mut row) {
            out.extend_from_slice(&row);
        }
    }
    out
}

#[test]
fn shrinking_averages_each_cell_the_edges_included() {
    let pixels = noise(5 * 3, 11);
    assert_eq!(shrink(5, 3, 1, &pixels), pixels);
    let shrunk = shrink(5, 3, 2, &pixels);
    assert_eq!(shrunk.len(), 3 * 2);
    let channel = |pixel: u32, shift: u32| u64::from(pixel >> shift & 0xff);
    for (index, &out) in shrunk.iter().enumerate() {
        let (column, row) = (index as u32 % 3, index as u32 / 3);
        let mut cell = Vec::new();
        for y in row * 2..(row * 2 + 2).min(3) {
            for x in column * 2..(column * 2 + 2).min(5) {
                cell.push(pixels[(y * 5 + x) as usize]);
            }
        }
        for shift in [16, 8, 0] {
            let mean = cell.iter().map(|&pixel| channel(pixel, shift)).sum::<u64>()
                / cell.len() as u64;
            assert_eq!(channel(out, shift), mean, "cell {column},{row}");
        }
    }
    assert_eq!(shrink(4, 4, 4, &[0x00ff_ffff; 16]), [0x00ff_ffff]);
    let mut sums = [0u64; 3];
    assert!(Downscale::new(4, 4, 0, &mut sums).is_none());
    assert!(Downscale::new(4, 4, MAX_FACTOR + 1, &mut sums).is_none());
    assert!(Downscale::new(4, 4, 1, &mut sums).is_none(), "too few sums");
}
