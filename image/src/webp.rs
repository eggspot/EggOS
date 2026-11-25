//! WebP (D519): the RIFF container (`RIFF`, `WEBP`, then a `VP8L` chunk, or `VP8X` with the
//! image chunk later) and lossless VP8L images as RFC 9649 specifies them: the header's size,
//! the transforms (predictor, colour, subtract-green, colour indexing with pixel bundling), the
//! colour cache, meta prefix codes, canonical prefix codes (simple, or normal through a code
//! length code), and LZ77 backward references with the 120 short distances. Lossy `VP8 ` images
//! go to the VP8 decoder (D520), their `ALPH` chunk's alpha (raw, or a header-less lossless
//! image's green, then unfiltered horizontally, vertically or by gradient) laid over them.
//! Animated files (D770: `VP8X` with the animation flag, `ANIM`, then `ANMF` frames) are drawn
//! frame by frame on a transparent canvas, each frame alpha-blended or copied over its
//! rectangle and that rectangle cleared afterwards when the frame asks.

use crate::bytes::le16;
use crate::{Animation, Error, Frame, Image, Limits};
use alloc::vec;
use alloc::vec::Vec;

const LOSSLESS_SIGNATURE: u8 = 0x2f;
const GREEN_ALPHABET: usize = 256 + 24;
const DISTANCE_ALPHABET: usize = 40;
const CODE_LENGTH_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];
const MAX_CODE_LENGTH: usize = 15;
const COLOUR_CACHE_MULTIPLIER: u32 = 0x1e35_a7bd;
const DISTANCES: [(i8, i8); 120] = [
    (0, 1),
    (1, 0),
    (1, 1),
    (-1, 1),
    (0, 2),
    (2, 0),
    (1, 2),
    (-1, 2),
    (2, 1),
    (-2, 1),
    (2, 2),
    (-2, 2),
    (0, 3),
    (3, 0),
    (1, 3),
    (-1, 3),
    (3, 1),
    (-3, 1),
    (2, 3),
    (-2, 3),
    (3, 2),
    (-3, 2),
    (0, 4),
    (4, 0),
    (1, 4),
    (-1, 4),
    (4, 1),
    (-4, 1),
    (3, 3),
    (-3, 3),
    (2, 4),
    (-2, 4),
    (4, 2),
    (-4, 2),
    (0, 5),
    (3, 4),
    (-3, 4),
    (4, 3),
    (-4, 3),
    (5, 0),
    (1, 5),
    (-1, 5),
    (5, 1),
    (-5, 1),
    (2, 5),
    (-2, 5),
    (5, 2),
    (-5, 2),
    (4, 4),
    (-4, 4),
    (3, 5),
    (-3, 5),
    (5, 3),
    (-5, 3),
    (0, 6),
    (6, 0),
    (1, 6),
    (-1, 6),
    (6, 1),
    (-6, 1),
    (2, 6),
    (-2, 6),
    (6, 2),
    (-6, 2),
    (4, 5),
    (-4, 5),
    (5, 4),
    (-5, 4),
    (3, 6),
    (-3, 6),
    (6, 3),
    (-6, 3),
    (0, 7),
    (7, 0),
    (1, 7),
    (-1, 7),
    (5, 5),
    (-5, 5),
    (7, 1),
    (-7, 1),
    (4, 6),
    (-4, 6),
    (6, 4),
    (-6, 4),
    (2, 7),
    (-2, 7),
    (7, 2),
    (-7, 2),
    (3, 7),
    (-3, 7),
    (7, 3),
    (-7, 3),
    (5, 6),
    (-5, 6),
    (6, 5),
    (-6, 5),
    (8, 0),
    (4, 7),
    (-4, 7),
    (7, 4),
    (-7, 4),
    (8, 1),
    (8, 2),
    (6, 6),
    (-6, 6),
    (8, 3),
    (5, 7),
    (-5, 7),
    (7, 5),
    (-7, 5),
    (8, 4),
    (6, 7),
    (-6, 7),
    (7, 6),
    (-7, 6),
    (8, 5),
    (7, 7),
    (-7, 7),
    (8, 6),
    (8, 7),
];

/// Whether `bytes` is a WebP file.
pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

fn chunk_size(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(bytes.get(at + 4..at + 8)?.try_into().ok()?) as usize)
}

const ANIMATION_FLAG: u8 = 0x02;
const NO_BLEND_FLAG: u8 = 0x02;
const DISPOSE_FLAG: u8 = 0x01;
const FRAME_HEADER: usize = 16;

fn le24(bytes: &[u8], at: usize) -> Result<u32, Error> {
    let part = bytes.get(at..at + 3).ok_or(Error::Truncated)?;
    Ok(u32::from(part[0]) | u32::from(part[1]) << 8 | u32::from(part[2]) << 16)
}

fn next_chunk(bytes: &[u8], at: usize) -> Result<Option<([u8; 4], &[u8], usize)>, Error> {
    if at + 8 > bytes.len() {
        return Ok(None);
    }
    let size = chunk_size(bytes, at).ok_or(Error::Truncated)?;
    let body = bytes.get(at + 8..at + 8 + size).ok_or(Error::Truncated)?;
    let tag = [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
    Ok(Some((tag, body, at + 8 + size + (size & 1))))
}

fn decode_chunks(bytes: &[u8], mut at: usize, limits: &Limits) -> Result<Image, Error> {
    let mut alpha = None;
    while let Some((tag, body, next)) = next_chunk(bytes, at)? {
        match &tag {
            b"VP8L" => return decode_lossless(body, limits),
            b"ALPH" => alpha = Some(body),
            b"VP8 " => {
                let mut image = crate::vp8::decode(body, limits)?;
                if let Some(chunk) = alpha {
                    apply_alpha(&mut image, chunk)?;
                }
                return Ok(image);
            }
            _ => {}
        }
        at = next;
    }
    Err(Error::Corrupt)
}

fn animated_canvas(bytes: &[u8]) -> Result<Option<(u32, u32)>, Error> {
    let Some((tag, body, _)) = next_chunk(bytes, 12)? else {
        return Ok(None);
    };
    if &tag != b"VP8X" || body.first().is_none_or(|flags| flags & ANIMATION_FLAG == 0) {
        return Ok(None);
    }
    Ok(Some((le24(body, 4)? + 1, le24(body, 7)? + 1)))
}

/// Decodes the WebP file `bytes`: an animation's first frame.
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, Error> {
    if !is_webp(bytes) {
        return Err(Error::UnknownFormat);
    }
    if animated_canvas(bytes)?.is_none() {
        return decode_chunks(bytes, 12, limits);
    }
    let first = Limits {
        max_frames: 1,
        ..*limits
    };
    decode_animation(bytes, &first)?
        .frames
        .into_iter()
        .next()
        .map(|frame| frame.image)
        .ok_or(Error::Corrupt)
}

struct Placement {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

fn blend(source: u32, destination: u32) -> u32 {
    let source_alpha = source >> 24;
    match source_alpha {
        255 => return source,
        0 => return destination,
        _ => {}
    }
    let kept = (destination >> 24) * (255 - source_alpha) / 255;
    let alpha = source_alpha + kept;
    let channel = |shift: u32| {
        ((source >> shift & 0xff) * source_alpha + (destination >> shift & 0xff) * kept) / alpha
    };
    alpha << 24 | channel(16) << 16 | channel(8) << 8 | channel(0)
}

fn draw_frame(canvas: &mut Image, frame: &Image, place: &Placement, blended: bool) {
    let canvas_width = canvas.width as usize;
    for row in 0..place.height {
        let source = &frame.pixels[row * place.width..(row + 1) * place.width];
        let start = (place.y + row) * canvas_width + place.x;
        let target = &mut canvas.pixels[start..start + place.width];
        for (destination, &pixel) in target.iter_mut().zip(source) {
            *destination = match blended {
                true => blend(pixel, *destination),
                false => pixel,
            };
        }
    }
}

fn clear(canvas: &mut Image, place: &Placement) {
    let canvas_width = canvas.width as usize;
    for row in 0..place.height {
        let start = (place.y + row) * canvas_width + place.x;
        canvas.pixels[start..start + place.width].fill(0);
    }
}

fn frame_placement(body: &[u8], canvas: &Image) -> Result<Placement, Error> {
    let place = Placement {
        x: le24(body, 0)? as usize * 2,
        y: le24(body, 3)? as usize * 2,
        width: le24(body, 6)? as usize + 1,
        height: le24(body, 9)? as usize + 1,
    };
    let fits = place.x + place.width <= canvas.width as usize
        && place.y + place.height <= canvas.height as usize;
    match fits {
        true => Ok(place),
        false => Err(Error::Corrupt),
    }
}

fn snapshot(canvas: &Image) -> Result<Image, Error> {
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(canvas.pixels.len())
        .map_err(|_| Error::OutOfMemory)?;
    pixels.extend_from_slice(&canvas.pixels);
    Ok(Image {
        width: canvas.width,
        height: canvas.height,
        pixels,
    })
}

/// Decodes every frame of the WebP file `bytes`, each the whole canvas as it shows, at most
/// [`Limits::max_frames`] and [`Limits::max_pixels`] in all; a still file is one frame.
pub fn decode_animation(bytes: &[u8], limits: &Limits) -> Result<Animation, Error> {
    if !is_webp(bytes) {
        return Err(Error::UnknownFormat);
    }
    let mut frames: Vec<Frame> = Vec::new();
    let Some((width, height)) = animated_canvas(bytes)? else {
        let image = decode_chunks(bytes, 12, limits)?;
        frames.try_reserve_exact(1).map_err(|_| Error::OutOfMemory)?;
        frames.push(Frame { image, delay_ms: 0 });
        return Ok(Animation {
            frames,
            loop_count: None,
        });
    };
    let mut canvas = crate::canvas(width, height, limits)?;
    let per_frame = u64::from(width) * u64::from(height);
    let mut loop_count = None;
    let mut at = 12;
    while let Some((tag, body, next)) = next_chunk(bytes, at)? {
        at = next;
        match &tag {
            b"ANIM" => {
                loop_count = match le16(body, 4)? {
                    0 => Some(0),
                    1 => None,
                    plays => Some(plays - 1),
                };
            }
            b"ANMF" => {
                let kept = (frames.len() as u64).saturating_mul(per_frame);
                let full = frames.len() >= limits.max_frames
                    || kept.saturating_add(per_frame) > limits.max_pixels;
                if full {
                    break;
                }
                let place = frame_placement(body, &canvas)?;
                let delay_ms = le24(body, 12)?;
                let flags = *body.get(15).ok_or(Error::Truncated)?;
                let image = decode_chunks(body, FRAME_HEADER, limits)?;
                if (image.width as usize, image.height as usize) != (place.width, place.height) {
                    return Err(Error::Corrupt);
                }
                draw_frame(&mut canvas, &image, &place, flags & NO_BLEND_FLAG == 0);
                frames.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
                frames.push(Frame {
                    image: snapshot(&canvas)?,
                    delay_ms,
                });
                if flags & DISPOSE_FLAG != 0 {
                    clear(&mut canvas, &place);
                }
            }
            _ => {}
        }
    }
    match frames.is_empty() {
        true => Err(Error::Corrupt),
        false => Ok(Animation { frames, loop_count }),
    }
}

struct Bits<'a> {
    bytes: &'a [u8],
    bit: usize,
}

impl Bits<'_> {
    fn read(&mut self, count: u32) -> Result<u32, Error> {
        let mut value = 0;
        for index in 0..count {
            let byte = *self.bytes.get(self.bit / 8).ok_or(Error::Truncated)?;
            value |= u32::from(byte >> (self.bit % 8) & 1) << index;
            self.bit += 1;
        }
        Ok(value)
    }
}

#[derive(Default, Clone)]
struct Code {
    counts: [u16; MAX_CODE_LENGTH + 1],
    symbols: Vec<u16>,
    single: Option<u16>,
}

impl Code {
    fn from_lengths(lengths: &[u8]) -> Result<Self, Error> {
        let mut code = Self::default();
        let used: Vec<usize> = (0..lengths.len()).filter(|&s| lengths[s] != 0).collect();
        if used.len() == 1 {
            code.single = Some(used[0] as u16);
            return Ok(code);
        }
        for &length in lengths {
            code.counts[usize::from(length)] += 1;
        }
        code.counts[0] = 0;
        let mut offsets = [0u16; MAX_CODE_LENGTH + 2];
        for length in 1..=MAX_CODE_LENGTH {
            offsets[length + 1] = offsets[length] + code.counts[length];
        }
        code.symbols = vec![0; used.len()];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let slot = &mut offsets[usize::from(length)];
                *code
                    .symbols
                    .get_mut(usize::from(*slot))
                    .ok_or(Error::Corrupt)? = symbol as u16;
                *slot += 1;
            }
        }
        Ok(code)
    }

    fn read(&self, bits: &mut Bits<'_>) -> Result<u16, Error> {
        if let Some(symbol) = self.single {
            return Ok(symbol);
        }
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for length in 1..=MAX_CODE_LENGTH {
            code |= bits.read(1)? as i32;
            let count = i32::from(self.counts[length]);
            if code - count < first {
                return self
                    .symbols
                    .get((index + code - first) as usize)
                    .copied()
                    .ok_or(Error::Corrupt);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Corrupt)
    }
}

fn read_code(bits: &mut Bits<'_>, alphabet: usize) -> Result<Code, Error> {
    let mut lengths = vec![0u8; alphabet];
    if bits.read(1)? == 1 {
        let count = bits.read(1)? + 1;
        let first_width = if bits.read(1)? == 1 { 8 } else { 1 };
        let first = bits.read(first_width)? as usize;
        *lengths.get_mut(first).ok_or(Error::Corrupt)? = 1;
        if count == 2 {
            let second = bits.read(8)? as usize;
            *lengths.get_mut(second).ok_or(Error::Corrupt)? = 1;
        }
        return Code::from_lengths(&lengths);
    }
    let mut length_lengths = [0u8; 19];
    let count = 4 + bits.read(4)? as usize;
    for &place in &CODE_LENGTH_ORDER[..count] {
        length_lengths[place] = bits.read(3)? as u8;
    }
    let length_code = Code::from_lengths(&length_lengths)?;
    let mut remaining = match bits.read(1)? {
        1 => {
            let width = 2 + 2 * bits.read(3)?;
            2 + bits.read(width)? as usize
        }
        _ => alphabet,
    };
    let (mut symbol, mut previous) = (0usize, 8u8);
    while symbol < alphabet && remaining > 0 {
        remaining -= 1;
        match length_code.read(bits)? {
            value @ 0..=15 => {
                lengths[symbol] = value as u8;
                if value != 0 {
                    previous = value as u8;
                }
                symbol += 1;
            }
            run => {
                let (repeat, value) = match run {
                    16 => (3 + bits.read(2)? as usize, previous),
                    17 => (3 + bits.read(3)? as usize, 0),
                    _ => (11 + bits.read(7)? as usize, 0),
                };
                if symbol + repeat > alphabet {
                    return Err(Error::Corrupt);
                }
                lengths[symbol..symbol + repeat].fill(value);
                symbol += repeat;
            }
        }
    }
    Code::from_lengths(&lengths)
}

struct Group {
    codes: [Code; 5],
}

fn prefix_value(bits: &mut Bits<'_>, prefix: u32) -> Result<u32, Error> {
    if prefix < 4 {
        return Ok(prefix + 1);
    }
    let extra = (prefix - 2) >> 1;
    let offset = (2 + (prefix & 1)) << extra;
    Ok(offset + bits.read(extra)? + 1)
}

fn cache_index(pixel: u32, cache_bits: u32) -> usize {
    (COLOUR_CACHE_MULTIPLIER.wrapping_mul(pixel) >> (32 - cache_bits)) as usize
}

fn decode_stream(
    bits: &mut Bits<'_>,
    width: usize,
    height: usize,
    top_level: bool,
) -> Result<Vec<u32>, Error> {
    let cache_bits = match bits.read(1)? {
        1 => {
            let value = bits.read(4)?;
            if !(1..=11).contains(&value) {
                return Err(Error::Corrupt);
            }
            value
        }
        _ => 0,
    };
    let cache_size = if cache_bits > 0 {
        1usize << cache_bits
    } else {
        0
    };
    let (meta_bits, meta) = match top_level && bits.read(1)? == 1 {
        true => {
            let prefix_bits = bits.read(3)? + 2;
            let block = 1usize << prefix_bits;
            let meta_width = width.div_ceil(block);
            let meta = decode_stream(bits, meta_width, height.div_ceil(block), false)?;
            (prefix_bits, Some((meta_width, meta)))
        }
        false => (0, None),
    };
    let groups_count = meta.as_ref().map_or(1, |(_, meta)| {
        meta.iter()
            .map(|&pixel| (pixel >> 8 & 0xffff) as usize + 1)
            .max()
            .unwrap_or(1)
    });
    let mut groups = Vec::new();
    groups
        .try_reserve(groups_count)
        .map_err(|_| Error::OutOfMemory)?;
    for _ in 0..groups_count {
        groups.push(Group {
            codes: [
                read_code(bits, GREEN_ALPHABET + cache_size)?,
                read_code(bits, 256)?,
                read_code(bits, 256)?,
                read_code(bits, 256)?,
                read_code(bits, DISTANCE_ALPHABET)?,
            ],
        });
    }
    let total = width * height;
    let mut pixels: Vec<u32> = Vec::new();
    pixels
        .try_reserve_exact(total)
        .map_err(|_| Error::OutOfMemory)?;
    let mut cache = vec![0u32; cache_size];
    let remember = |pixel: u32, cache: &mut [u32]| {
        if cache_bits > 0 {
            cache[cache_index(pixel, cache_bits)] = pixel;
        }
    };
    while pixels.len() < total {
        let position = pixels.len();
        let group = match &meta {
            Some((meta_width, meta)) => {
                let (x, y) = (
                    (position % width) >> meta_bits,
                    (position / width) >> meta_bits,
                );
                (meta[y * meta_width + x] >> 8 & 0xffff) as usize
            }
            None => 0,
        };
        let codes = &groups.get(group).ok_or(Error::Corrupt)?.codes;
        let green = usize::from(codes[0].read(bits)?);
        if green < 256 {
            let red = u32::from(codes[1].read(bits)?);
            let blue = u32::from(codes[2].read(bits)?);
            let alpha = u32::from(codes[3].read(bits)?);
            let pixel = alpha << 24 | red << 16 | (green as u32) << 8 | blue;
            pixels.push(pixel);
            remember(pixel, &mut cache);
        } else if green < GREEN_ALPHABET {
            let length = prefix_value(bits, (green - 256) as u32)? as usize;
            let distance_symbol = u32::from(codes[4].read(bits)?);
            let code = prefix_value(bits, distance_symbol)? as usize;
            let distance = match code {
                1..=120 => {
                    let (dx, dy) = DISTANCES[code - 1];
                    (i64::from(dx) + i64::from(dy) * width as i64).max(1) as usize
                }
                _ => code - 120,
            };
            if distance > position || position + length > total {
                return Err(Error::Corrupt);
            }
            for _ in 0..length {
                let pixel = pixels[pixels.len() - distance];
                pixels.push(pixel);
                remember(pixel, &mut cache);
            }
        } else {
            let pixel = *cache.get(green - GREEN_ALPHABET).ok_or(Error::Corrupt)?;
            pixels.push(pixel);
            remember(pixel, &mut cache);
        }
    }
    Ok(pixels)
}

enum Transform {
    Predictor { bits: u32, image: Vec<u32> },
    Colour { bits: u32, image: Vec<u32> },
    SubtractGreen,
    Indexing { palette: Vec<u32>, bundle_bits: u32 },
}

fn add(a: u32, b: u32) -> u32 {
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        out |= (((a >> shift & 0xff) + (b >> shift & 0xff)) & 0xff) << shift;
    }
    out
}

fn average(a: u32, b: u32) -> u32 {
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        out |= (((a >> shift & 0xff) + (b >> shift & 0xff)) / 2) << shift;
    }
    out
}

fn channels(pixel: u32) -> [i32; 4] {
    [24, 16, 8, 0].map(|shift| (pixel >> shift & 0xff) as i32)
}

fn from_channels(values: [i32; 4]) -> u32 {
    values
        .iter()
        .zip([24, 16, 8, 0])
        .fold(0, |pixel, (&value, shift)| {
            pixel | (value.clamp(0, 255) as u32) << shift
        })
}

fn select(left: u32, top: u32, top_left: u32) -> u32 {
    let (l, t, tl) = (channels(left), channels(top), channels(top_left));
    let estimate: [i32; 4] = core::array::from_fn(|index| l[index] + t[index] - tl[index]);
    let distance = |of: [i32; 4]| {
        (0..4)
            .map(|index| (estimate[index] - of[index]).abs())
            .sum::<i32>()
    };
    if distance(l) < distance(t) { left } else { top }
}

fn predict(mode: u32, left: u32, top: u32, top_right: u32, top_left: u32) -> u32 {
    match mode {
        0 => 0xff00_0000,
        1 => left,
        2 => top,
        3 => top_right,
        4 => top_left,
        5 => average(average(left, top_right), top),
        6 => average(left, top_left),
        7 => average(left, top),
        8 => average(top_left, top),
        9 => average(top, top_right),
        10 => average(average(left, top_left), average(top, top_right)),
        11 => select(left, top, top_left),
        12 => {
            let (l, t, tl) = (channels(left), channels(top), channels(top_left));
            from_channels(core::array::from_fn(|i| l[i] + t[i] - tl[i]))
        }
        _ => {
            let (a, tl) = (channels(average(left, top)), channels(top_left));
            from_channels(core::array::from_fn(|i| a[i] + (a[i] - tl[i]) / 2))
        }
    }
}

fn undo_predictor(pixels: &mut [u32], width: usize, bits: u32, image: &[u32]) {
    let block_width = width.div_ceil(1 << bits);
    for index in 0..pixels.len() {
        let (x, y) = (index % width, index / width);
        let prediction = match (x, y) {
            (0, 0) => 0xff00_0000,
            (_, 0) => pixels[index - 1],
            (0, _) => pixels[index - width],
            _ => {
                let mode = image[(y >> bits) * block_width + (x >> bits)] >> 8 & 0xf;
                let top_right = match x + 1 < width {
                    true => pixels[index - width + 1],
                    false => pixels[index - x],
                };
                let (left, top, top_left) = (
                    pixels[index - 1],
                    pixels[index - width],
                    pixels[index - width - 1],
                );
                predict(mode, left, top, top_right, top_left)
            }
        };
        pixels[index] = add(pixels[index], prediction);
    }
}

fn colour_delta(transform: u32, colour: u32) -> u32 {
    ((i32::from(transform as u8 as i8) * i32::from(colour as u8 as i8)) >> 5) as u32
}

fn undo_colour(pixels: &mut [u32], width: usize, bits: u32, image: &[u32]) {
    let block_width = width.div_ceil(1 << bits);
    for (index, pixel) in pixels.iter_mut().enumerate() {
        let (x, y) = (index % width, index / width);
        let element = image[(y >> bits) * block_width + (x >> bits)];
        let (green_to_red, green_to_blue, red_to_blue) =
            (element & 0xff, element >> 8 & 0xff, element >> 16 & 0xff);
        let green = *pixel >> 8 & 0xff;
        let red = (*pixel >> 16).wrapping_add(colour_delta(green_to_red, green)) & 0xff;
        let blue = (*pixel)
            .wrapping_add(colour_delta(green_to_blue, green))
            .wrapping_add(colour_delta(red_to_blue, red))
            & 0xff;
        *pixel = *pixel & 0xff00_ff00 | red << 16 | blue;
    }
}

fn undo_indexing(
    packed: &[u32],
    width: usize,
    height: usize,
    palette: &[u32],
    bundle_bits: u32,
) -> Vec<u32> {
    let per_pixel = 8 >> bundle_bits;
    let packed_width = width.div_ceil(1 << bundle_bits);
    let mask = (1u32 << per_pixel) - 1;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let word = packed[y * packed_width + (x >> bundle_bits)];
            let index = word >> 8 >> ((x & ((1 << bundle_bits) - 1)) as u32 * per_pixel) & mask;
            pixels.push(palette.get(index as usize).copied().unwrap_or(0));
        }
    }
    pixels
}

fn decode_lossless(body: &[u8], limits: &Limits) -> Result<Image, Error> {
    if body.first() != Some(&LOSSLESS_SIGNATURE) {
        return Err(Error::Corrupt);
    }
    let mut bits = Bits {
        bytes: body,
        bit: 8,
    };
    let width = bits.read(14)? as usize + 1;
    let height = bits.read(14)? as usize + 1;
    let _alpha = bits.read(1)?;
    if bits.read(3)? != 0 {
        return Err(Error::Unsupported);
    }
    if width as u32 > limits.max_side
        || height as u32 > limits.max_side
        || (width * height) as u64 > limits.max_pixels
    {
        return Err(Error::TooLarge);
    }
    let pixels = decode_image_stream(&mut bits, width, height)?;
    Ok(Image {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

fn decode_image_stream(
    bits: &mut Bits<'_>,
    width: usize,
    height: usize,
) -> Result<Vec<u32>, Error> {
    let mut transforms = Vec::new();
    let mut coded_width = width;
    let mut seen = 0u8;
    while bits.read(1)? == 1 {
        let kind = bits.read(2)?;
        if seen & 1 << kind != 0 {
            return Err(Error::Corrupt);
        }
        seen |= 1 << kind;
        let width_at_read = coded_width;
        let transform = match kind {
            0 | 1 => {
                let block_bits = bits.read(3)? + 2;
                let block = 1usize << block_bits;
                let image = decode_stream(
                    bits,
                    coded_width.div_ceil(block),
                    height.div_ceil(block),
                    false,
                )?;
                match kind {
                    0 => Transform::Predictor {
                        bits: block_bits,
                        image,
                    },
                    _ => Transform::Colour {
                        bits: block_bits,
                        image,
                    },
                }
            }
            2 => Transform::SubtractGreen,
            _ => {
                let colours = bits.read(8)? as usize + 1;
                let mut palette = decode_stream(bits, colours, 1, false)?;
                for index in 1..palette.len() {
                    palette[index] = add(palette[index], palette[index - 1]);
                }
                let bundle_bits = match colours {
                    0..=2 => 3,
                    3..=4 => 2,
                    5..=16 => 1,
                    _ => 0,
                };
                coded_width = coded_width.div_ceil(1 << bundle_bits);
                Transform::Indexing {
                    palette,
                    bundle_bits,
                }
            }
        };
        transforms.push((transform, width_at_read));
    }
    let mut pixels = decode_stream(bits, coded_width, height, true)?;
    for (transform, width_at_read) in transforms.iter().rev() {
        let width_at_read = *width_at_read;
        match transform {
            Transform::Predictor { bits, image } => {
                undo_predictor(&mut pixels, width_at_read, *bits, image)
            }
            Transform::Colour { bits, image } => {
                undo_colour(&mut pixels, width_at_read, *bits, image)
            }
            Transform::SubtractGreen => {
                for pixel in &mut pixels {
                    let green = *pixel >> 8 & 0xff;
                    let red = (*pixel >> 16).wrapping_add(green) & 0xff;
                    let blue = (*pixel).wrapping_add(green) & 0xff;
                    *pixel = *pixel & 0xff00_ff00 | red << 16 | blue;
                }
            }
            Transform::Indexing {
                palette,
                bundle_bits,
            } => {
                pixels = undo_indexing(&pixels, width_at_read, height, palette, *bundle_bits);
            }
        }
    }
    Ok(pixels)
}

fn apply_alpha(image: &mut Image, chunk: &[u8]) -> Result<(), Error> {
    let header = *chunk.first().ok_or(Error::Truncated)?;
    let (width, height) = (image.width as usize, image.height as usize);
    let mut alpha = match header & 3 {
        0 => chunk
            .get(1..1 + width * height)
            .ok_or(Error::Truncated)?
            .to_vec(),
        1 => {
            let mut bits = Bits {
                bytes: &chunk[1..],
                bit: 0,
            };
            let pixels = decode_image_stream(&mut bits, width, height)?;
            pixels.iter().map(|pixel| (pixel >> 8) as u8).collect()
        }
        _ => return Err(Error::Corrupt),
    };
    unfilter_alpha(&mut alpha, width, header >> 2 & 3);
    for (pixel, value) in image.pixels.iter_mut().zip(alpha) {
        *pixel = *pixel & 0x00ff_ffff | u32::from(value) << 24;
    }
    Ok(())
}

fn unfilter_alpha(alpha: &mut [u8], width: usize, filter: u8) {
    if filter == 0 {
        return;
    }
    for index in 1..alpha.len() {
        let predictor = match (index % width, index / width) {
            (_, 0) => alpha[index - 1],
            (0, _) => alpha[index - width],
            _ => {
                let (left, above) = (alpha[index - 1], alpha[index - width]);
                match filter {
                    1 => left,
                    2 => above,
                    _ => {
                        let corner = i32::from(alpha[index - width - 1]);
                        (i32::from(left) + i32::from(above) - corner).clamp(0, 255) as u8
                    }
                }
            }
        };
        alpha[index] = alpha[index].wrapping_add(predictor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Writer {
        bytes: Vec<u8>,
        bit: usize,
    }

    impl Writer {
        fn put(&mut self, value: u32, count: u32) {
            for index in 0..count {
                if self.bit / 8 >= self.bytes.len() {
                    self.bytes.push(0);
                }
                let last = self.bytes.len() - 1;
                self.bytes[last] |= ((value >> index & 1) as u8) << (self.bit % 8);
                self.bit += 1;
            }
        }
    }

    fn simple_code(writer: &mut Writer, symbol: u32) {
        writer.put(1, 1);
        writer.put(0, 1);
        writer.put(1, 1);
        writer.put(symbol, 8);
    }

    fn one_colour(width: u32, height: u32, colour: u32) -> Vec<u8> {
        let mut writer = Writer {
            bytes: vec![LOSSLESS_SIGNATURE],
            bit: 8,
        };
        writer.put(width - 1, 14);
        writer.put(height - 1, 14);
        writer.put(1, 1);
        writer.put(0, 3);
        writer.put(0, 1);
        writer.put(0, 1);
        writer.put(0, 1);
        for shift in [8, 16, 0, 24] {
            simple_code(&mut writer, colour >> shift & 0xff);
        }
        simple_code(&mut writer, 0);
        writer.bytes
    }

    fn chunk(file: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
        file.extend_from_slice(tag);
        file.extend_from_slice(&(body.len() as u32).to_le_bytes());
        file.extend_from_slice(body);
        if body.len() % 2 == 1 {
            file.push(0);
        }
    }

    fn riff(chunks: &[u8]) -> Vec<u8> {
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&((chunks.len() + 4) as u32).to_le_bytes());
        file.extend_from_slice(b"WEBP");
        file.extend_from_slice(chunks);
        file
    }

    fn u24(value: u32) -> [u8; 3] {
        [value as u8, (value >> 8) as u8, (value >> 16) as u8]
    }

    fn frame(place: [u32; 4], duration: u32, flags: u8, colour: u32) -> Vec<u8> {
        let [x, y, width, height] = place;
        let mut body = Vec::new();
        for value in [x / 2, y / 2, width - 1, height - 1, duration] {
            body.extend_from_slice(&u24(value));
        }
        body.push(flags);
        chunk(&mut body, b"VP8L", &one_colour(width, height, colour));
        body
    }

    #[test]
    fn a_lossless_image_of_one_colour_decodes_to_that_colour() {
        let mut chunks = Vec::new();
        chunk(&mut chunks, b"VP8L", &one_colour(2, 2, 0xff20_4010));
        let image = decode(&riff(&chunks), &Limits::DEFAULT).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert!(image.pixels.iter().all(|&pixel| pixel == 0xff20_4010));
    }

    #[test]
    fn a_canonical_code_reads_its_symbols_by_length() {
        let code = Code::from_lengths(&[2, 1, 3, 3]).unwrap();
        let mut bits = Bits {
            bytes: &[0b0000_0110],
            bit: 0,
        };
        assert_eq!(code.read(&mut bits).unwrap(), 1);
        assert_eq!(code.read(&mut bits).unwrap(), 2);
    }

    #[test]
    fn an_animation_blends_disposes_and_keeps_its_loop_count() {
        let mut chunks = Vec::new();
        let mut header = vec![ANIMATION_FLAG | 0x10, 0, 0, 0];
        header.extend_from_slice(&u24(3));
        header.extend_from_slice(&u24(1));
        chunk(&mut chunks, b"VP8X", &header);
        chunk(&mut chunks, b"ANIM", &[0, 0, 0, 0, 3, 0]);
        chunk(&mut chunks, b"ANMF", &frame([0, 0, 2, 2], 100, DISPOSE_FLAG, 0xff00_00ff));
        chunk(&mut chunks, b"ANMF", &frame([2, 0, 2, 2], 50, 0, 0x80ff_0000));
        chunk(&mut chunks, b"ANMF", &frame([0, 0, 2, 2], 70, NO_BLEND_FLAG, 0x0000_ff00));
        let file = riff(&chunks);
        let animation = decode_animation(&file, &Limits::DEFAULT).unwrap();
        assert_eq!(animation.loop_count, Some(2));
        let delays: Vec<u32> = animation.frames.iter().map(|frame| frame.delay_ms).collect();
        assert_eq!(delays, vec![100, 50, 70]);
        let first = &animation.frames[0].image;
        assert_eq!((first.width, first.height), (4, 2));
        assert_eq!(first.pixel(0, 0), Some(0xff00_00ff));
        assert_eq!(first.pixel(3, 1), Some(0));
        let second = &animation.frames[1].image;
        assert_eq!(second.pixel(0, 0), Some(0));
        assert_eq!(second.pixel(2, 0), Some(0x80ff_0000));
        let third = &animation.frames[2].image;
        assert_eq!(third.pixel(1, 1), Some(0x0000_ff00));
        assert_eq!(third.pixel(3, 0), Some(0x80ff_0000));
        assert_eq!(decode(&file, &Limits::DEFAULT).unwrap(), animation.frames[0].image);
    }

    #[test]
    fn blending_mixes_straight_alpha() {
        assert_eq!(blend(0x8000_00ff, 0xffff_0000), 0xff7f_0080);
        assert_eq!(blend(0x0012_3456, 0xff00_ff00), 0xff00_ff00);
        assert_eq!(blend(0x8012_3456, 0), 0x8012_3456);
    }
}
