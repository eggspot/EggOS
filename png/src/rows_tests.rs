use crate::decode::paeth;
use crate::inflate::SliceSource;
use crate::inflate_tests::{BitWriter, dynamic_stream, literals, stored_block};
use crate::rows::{Header, MAX_WIDTH, RowDecoder, RowWorkspace};
use crate::{Adler32, DecodeError, Encoder, EncoderWorkspace, PngSink, SIGNATURE, crc32_update};

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let crc = crc32_update(crc32_update(0, kind), data);
    png.extend_from_slice(&crc.to_be_bytes());
}

fn zlib(deflate: &[u8], raw: &[u8]) -> Vec<u8> {
    let mut stream = vec![0x78, 0x01];
    stream.extend_from_slice(deflate);
    let mut adler = Adler32::new();
    adler.update(raw);
    stream.extend_from_slice(&adler.value().to_be_bytes());
    stream
}

fn stored(raw: &[u8]) -> Vec<u8> {
    let mut writer = BitWriter::default();
    stored_block(&mut writer, raw, true);
    zlib(&writer.finish(), raw)
}

struct Image<'a> {
    width: u32,
    height: u32,
    depth: u8,
    colour: u8,
    interlace: u8,
    palette: Option<&'a [u8]>,
    transparency: Option<&'a [u8]>,
}

fn build(image: &Image<'_>, idat: &[&[u8]]) -> Vec<u8> {
    let mut png = SIGNATURE.to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&image.width.to_be_bytes());
    header.extend_from_slice(&image.height.to_be_bytes());
    header.extend_from_slice(&[image.depth, image.colour, 0, 0, image.interlace]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"tEXt", b"Comment\0skipped");
    if let Some(palette) = image.palette {
        chunk(&mut png, b"PLTE", palette);
    }
    if let Some(transparency) = image.transparency {
        chunk(&mut png, b"tRNS", transparency);
    }
    for part in idat {
        chunk(&mut png, b"IDAT", part);
    }
    chunk(&mut png, b"IEND", &[]);
    png
}

fn plain(width: u32, height: u32, depth: u8, colour: u8) -> Image<'static> {
    Image {
        width,
        height,
        depth,
        colour,
        interlace: 0,
        palette: None,
        transparency: None,
    }
}

fn rows_of(png: &[u8]) -> Result<(Header, Vec<Vec<u32>>), DecodeError> {
    let mut source = SliceSource::new(png);
    let decoder = RowDecoder::open(&mut source)?;
    let header = decoder.header();
    let mut work = Box::new(RowWorkspace::new());
    let mut rows = Vec::new();
    decoder.read_rows(&mut work, |index, row| {
        assert_eq!(index as usize, rows.len());
        rows.push(row.to_vec());
    })?;
    Ok((header, rows))
}

fn pixels_of(png: &[u8]) -> Vec<Vec<u32>> {
    rows_of(png).unwrap().1
}

#[test]
fn rgba_rows_come_out_as_straight_argb() {
    let raw = [
        0, 255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 0, 0, 0, 255, 255, 255, 255,
    ];
    let png = build(&plain(2, 2, 8, 6), &[&stored(&raw)]);
    let (header, rows) = rows_of(&png).unwrap();
    assert_eq!(
        header,
        Header {
            width: 2,
            height: 2,
            bit_depth: 8,
            colour_type: 6
        }
    );
    assert_eq!(
        rows,
        [[0xffff_0000, 0x8000_ff00], [0x0000_0000, 0xffff_ffff]]
    );
}

#[test]
fn a_palette_at_two_bits_takes_its_alpha_from_trns() {
    let palette: [u8; 9] = [10, 20, 30, 40, 50, 60, 70, 80, 90];
    let transparency: [u8; 2] = [0, 128];
    let image = Image {
        palette: Some(&palette),
        transparency: Some(&transparency),
        ..plain(3, 1, 2, 3)
    };
    let raw = [0, 0b0001_1000];
    let png = build(&image, &[&stored(&raw)]);
    assert_eq!(pixels_of(&png), [[0x000a_141e, 0x8028_323c, 0xff46_505a]]);
    let raw = [0, 0b1100_0000];
    let png = build(&image, &[&stored(&raw)]);
    assert_eq!(rows_of(&png), Err(DecodeError::Corrupt));
}

#[test]
fn greyscale_at_every_depth_scales_to_a_byte() {
    let png = build(&plain(8, 1, 1, 0), &[&stored(&[0, 0b1010_0000])]);
    assert_eq!(
        pixels_of(&png)[0][..3],
        [0xffff_ffff, 0xff00_0000, 0xffff_ffff]
    );
    let png = build(&plain(2, 1, 4, 0), &[&stored(&[0, 0x5f])]);
    assert_eq!(pixels_of(&png), [[0xff55_5555, 0xffff_ffff]]);
    let image = Image {
        transparency: Some(&[0x12u8, 0x34]),
        ..plain(2, 1, 16, 0)
    };
    let png = build(&image, &[&stored(&[0, 0x12, 0x34, 0x12, 0x35])]);
    assert_eq!(pixels_of(&png), [[0x0012_1212, 0xff12_1212]]);
    let png = build(&plain(1, 1, 8, 4), &[&stored(&[0, 0x40, 0x80])]);
    assert_eq!(pixels_of(&png), [[0x8040_4040]]);
}

#[test]
fn rgb_at_sixteen_bits_keeps_the_high_bytes_and_its_trns_colour() {
    let image = Image {
        transparency: Some(&[0u8, 1, 0, 2, 0, 3]),
        ..plain(2, 1, 16, 2)
    };
    let raw = [0, 0xab, 0xcd, 0x12, 0x34, 0x56, 0x78, 0, 1, 0, 2, 0, 3];
    let png = build(&image, &[&stored(&raw)]);
    assert_eq!(pixels_of(&png), [[0xffab_1256, 0x0000_0000]]);
}

fn filtered(rows: &[Vec<u8>], step: usize, filters: &[u8]) -> Vec<u8> {
    let width = rows[0].len();
    let mut out = Vec::new();
    let zero = vec![0u8; width];
    for (index, row) in rows.iter().enumerate() {
        let above = if index == 0 { &zero } else { &rows[index - 1] };
        let filter = filters[index % filters.len()];
        out.push(filter);
        for at in 0..width {
            let left = if at >= step { row[at - step] } else { 0 };
            let up_left = if at >= step { above[at - step] } else { 0 };
            let predicted = match filter {
                0 => 0,
                1 => left,
                2 => above[at],
                3 => ((u16::from(left) + u16::from(above[at])) / 2) as u8,
                _ => paeth(left, above[at], up_left),
            };
            out.push(row[at].wrapping_sub(predicted));
        }
    }
    out
}

#[test]
fn all_five_filters_undo_at_four_bytes_a_pixel() {
    let mut state = 0x1357_9bdfu32;
    let rows: Vec<Vec<u8>> = (0..10)
        .map(|_| {
            (0..12)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect()
        })
        .collect();
    let raw = filtered(&rows, 4, &[0, 1, 2, 3, 4]);
    let png = build(&plain(3, 10, 8, 6), &[&stored(&raw)]);
    let expected: Vec<Vec<u32>> = rows
        .iter()
        .map(|row| {
            row.chunks(4)
                .map(|p| {
                    u32::from(p[3]) << 24
                        | u32::from(p[0]) << 16
                        | u32::from(p[1]) << 8
                        | u32::from(p[2])
                })
                .collect()
        })
        .collect();
    assert_eq!(pixels_of(&png), expected);
}

#[test]
fn dynamic_huffman_rows_split_across_idat_chunks_decode() {
    let raw = [0, 1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9];
    let stream = zlib(&dynamic_stream(&literals(&raw)), &raw);
    let (first, second) = stream.split_at(5);
    let png = build(&plain(1, 3, 8, 2), &[first, &[], second]);
    assert_eq!(
        pixels_of(&png),
        [[0xff01_0203], [0xff04_0506], [0xff07_0809]]
    );
}

struct Bytes(Vec<u8>);

impl PngSink for Bytes {
    type Error = ();

    fn write(&mut self, bytes: &[u8]) -> Result<(), ()> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

#[test]
fn what_the_encoder_writes_decodes_row_by_row() {
    let pixels: Vec<u32> = (0..40 * 7)
        .map(|index| (index * 0x0001_0307) & 0x00ff_ffff)
        .collect();
    let mut work = Box::new(EncoderWorkspace::new());
    let mut encoder = Encoder::new(40, 7, &mut work, Bytes(Vec::new())).unwrap();
    for row in pixels.chunks(40) {
        encoder.write_row(row).unwrap();
    }
    let png = encoder.finish().unwrap().0;
    let rows = pixels_of(&png);
    let flat: Vec<u32> = rows.concat();
    let opaque: Vec<u32> = pixels.iter().map(|pixel| pixel | 0xff00_0000).collect();
    assert_eq!(flat, opaque);
}

#[test]
fn an_rgba_image_keeps_its_alpha_row_by_row() {
    let pixels: Vec<u32> = (0..24u32 * 5)
        .map(|index| index.wrapping_mul(0x0901_0307))
        .collect();
    let mut work = Box::new(EncoderWorkspace::new());
    let mut encoder = Encoder::new_rgba(24, 5, &mut work, Bytes(Vec::new())).unwrap();
    for row in pixels.chunks(24) {
        encoder.write_row(row).unwrap();
    }
    let png = encoder.finish().unwrap().0;
    assert_eq!(png[25], 6);
    assert_eq!(pixels_of(&png).concat(), pixels);
}

#[test]
fn what_cannot_be_decoded_says_why() {
    let raw = [0, 1, 2, 3];
    let good = build(&plain(1, 1, 8, 2), &[&stored(&raw)]);
    let interlaced = Image {
        interlace: 1,
        ..plain(1, 1, 8, 2)
    };
    assert_eq!(
        rows_of(&build(&interlaced, &[&stored(&raw)])),
        Err(DecodeError::Unsupported)
    );
    assert_eq!(
        rows_of(&build(&plain(MAX_WIDTH + 1, 1, 8, 2), &[&stored(&raw)])),
        Err(DecodeError::Unsupported)
    );
    assert_eq!(
        rows_of(&build(&plain(1, 1, 3, 2), &[&stored(&raw)])),
        Err(DecodeError::Corrupt)
    );
    assert_eq!(rows_of(b"GIF89a"), Err(DecodeError::NotPng));
    assert_eq!(
        rows_of(&build(&plain(1, 1, 8, 3), &[&stored(&[0, 0])])),
        Err(DecodeError::Corrupt)
    );
    let mut broken = good.clone();
    let text = broken.windows(4).position(|w| w == b"tEXt").unwrap();
    broken[text + 6] ^= 1;
    assert_eq!(rows_of(&broken), Err(DecodeError::BadChunk));
    let mut broken = good.clone();
    let idat = broken.windows(4).position(|w| w == b"IDAT").unwrap();
    broken[idat + 6] ^= 1;
    assert_eq!(rows_of(&broken), Err(DecodeError::BadChunk));
    let mut broken = good.clone();
    let idat_crc = good.len() - 13;
    broken[idat_crc] ^= 1;
    assert_eq!(rows_of(&broken), Err(DecodeError::BadChunk));
    let short = build(&plain(1, 2, 8, 2), &[&stored(&raw)]);
    assert_eq!(rows_of(&short), Err(DecodeError::Truncated));
    let long = build(&plain(1, 1, 8, 2), &[&stored(&[0, 1, 2, 3, 0, 4, 5, 6])]);
    assert_eq!(rows_of(&long), Err(DecodeError::Corrupt));
    let mut wrong_sum = stored(&raw);
    let last = wrong_sum.len() - 1;
    wrong_sum[last] ^= 1;
    assert_eq!(
        rows_of(&build(&plain(1, 1, 8, 2), &[&wrong_sum])),
        Err(DecodeError::Corrupt)
    );
    for cut in 0..good.len() - 12 {
        assert!(rows_of(&good[..cut]).is_err(), "cut at {cut}");
    }
}
