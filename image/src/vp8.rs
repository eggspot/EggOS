//! Lossy WebP's images: VP8 key frames as RFC 6386 specifies them (D520). The frame header and
//! the first partition, read through the boolean entropy decoder, give the segments, the loop
//! filter's settings, the quantizers, the coefficient probabilities and each macroblock's
//! segment, skip flag and intra modes (16x16 luma or sixteen 4x4 subblocks with their
//! context-dependent probabilities, and 8x8 chroma); each macroblock row's coefficients come
//! from its token partition. The inverse Walsh-Hadamard transform gives the luma blocks' DC,
//! the inverse DCT the residues added to the prediction from unfiltered neighbours (127 above
//! the frame, 129 left of it); the normal or simple loop filter then runs over the frame, and
//! BT.601 turns it into RGB, each chroma sample covering two by two pixels.

use crate::vp8_tables::{
    AC_STEPS, COEFFICIENT_UPDATE_PROBABILITIES, DC_STEPS, DEFAULT_COEFFICIENT_PROBABILITIES,
    KEY_FRAME_SUBBLOCK_MODE_PROBABILITIES,
};
use crate::{Error, Image, Limits};
use alloc::vec::Vec;

const BANDS: [usize; 17] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7, 0];
const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
const CATEGORY_BASES: [i32; 6] = [5, 7, 11, 19, 35, 67];
const CATEGORY_PROBABILITIES: [&[u8]; 6] = [
    &[159],
    &[165, 145],
    &[173, 148, 140],
    &[176, 155, 140, 135],
    &[180, 157, 141, 134, 130],
    &[254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129],
];
const LUMA_MODE_PROBABILITIES: [u8; 4] = [145, 156, 163, 128];
const CHROMA_MODE_PROBABILITIES: [u8; 3] = [142, 114, 183];
const START_CODE: [u8; 3] = [0x9d, 0x01, 0x2a];
const FIRST_PARTITION_START: usize = 10;
const MACROBLOCK_BYTES: u64 = 16 * 16 + 2 * 8 * 8;
const Y2_CONTEXT: usize = 8;

const DC_PRED: u8 = 0;
const V_PRED: u8 = 1;
const H_PRED: u8 = 2;
const TM_PRED: u8 = 3;
const B_PRED: u8 = 4;

const B_DC: u8 = 0;
const B_TM: u8 = 1;
const B_VE: u8 = 2;
const B_HE: u8 = 3;
const B_LD: u8 = 4;
const B_RD: u8 = 5;
const B_VR: u8 = 6;
const B_VL: u8 = 7;
const B_HD: u8 = 8;
const B_HU: u8 = 9;

const fn leaf(mode: u8) -> i8 {
    -(mode as i8)
}

const SUBBLOCK_TREE: [i8; 18] = [
    leaf(B_DC),
    2,
    leaf(B_TM),
    4,
    leaf(B_VE),
    6,
    8,
    12,
    leaf(B_HE),
    10,
    leaf(B_RD),
    leaf(B_VR),
    leaf(B_LD),
    14,
    leaf(B_VL),
    16,
    leaf(B_HD),
    leaf(B_HU),
];
const LUMA_TREE: [i8; 8] = [
    leaf(B_PRED),
    2,
    4,
    6,
    leaf(DC_PRED),
    leaf(V_PRED),
    leaf(H_PRED),
    leaf(TM_PRED),
];
const CHROMA_TREE: [i8; 6] = [leaf(DC_PRED), 2, leaf(V_PRED), 4, leaf(H_PRED), leaf(TM_PRED)];
const SEGMENT_TREE: [i8; 6] = [2, 4, 0, -1, -2, -3];

struct BoolDecoder<'a> {
    data: &'a [u8],
    at: usize,
    value: u32,
    range: u32,
    shifted: u32,
}

impl<'a> BoolDecoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        let byte = |index: usize| u32::from(data.get(index).copied().unwrap_or(0));
        Self {
            data,
            at: 2,
            value: byte(0) << 8 | byte(1),
            range: 255,
            shifted: 0,
        }
    }

    fn read_bool(&mut self, probability: u8) -> bool {
        let split = 1 + (((self.range - 1) * u32::from(probability)) >> 8);
        let big_split = split << 8;
        let bit = self.value >= big_split;
        if bit {
            self.range -= split;
            self.value -= big_split;
        } else {
            self.range = split;
        }
        while self.range < 128 {
            self.value <<= 1;
            self.range <<= 1;
            self.shifted += 1;
            if self.shifted == 8 {
                self.shifted = 0;
                self.value |= u32::from(self.data.get(self.at).copied().unwrap_or(0));
                self.at += 1;
            }
        }
        bit
    }

    fn read_literal(&mut self, count: u32) -> u32 {
        (0..count).fold(0, |value, _| value << 1 | u32::from(self.read_bool(128)))
    }

    fn read_optional_signed(&mut self, count: u32) -> i32 {
        if !self.read_bool(128) {
            return 0;
        }
        let magnitude = self.read_literal(count) as i32;
        if self.read_bool(128) {
            -magnitude
        } else {
            magnitude
        }
    }

    fn read_tree(&mut self, tree: &[i8], probabilities: &[u8]) -> u8 {
        let mut node = 0usize;
        loop {
            let next = tree[node + usize::from(self.read_bool(probabilities[node >> 1]))];
            if next <= 0 {
                return next.unsigned_abs();
            }
            node = next as usize;
        }
    }
}

#[derive(Clone, Copy)]
struct Quantizer {
    y: (i32, i32),
    y2: (i32, i32),
    uv: (i32, i32),
}

impl Quantizer {
    fn new(index: i32, deltas: [i32; 5]) -> Self {
        let index = index.clamp(0, 127);
        let step = |table: &[u16; 128], delta: i32| {
            i32::from(table[(index + delta).clamp(0, 127) as usize])
        };
        Self {
            y: (step(&DC_STEPS, deltas[0]), step(&AC_STEPS, 0)),
            y2: (
                step(&DC_STEPS, deltas[1]) * 2,
                (step(&AC_STEPS, deltas[2]) * 155 / 100).max(8),
            ),
            uv: (step(&DC_STEPS, deltas[3]).min(132), step(&AC_STEPS, deltas[4])),
        }
    }
}

struct Segments {
    update_map: bool,
    absolute: bool,
    quantizer: [i32; 4],
    filter_level: [i32; 4],
    tree_probabilities: [u8; 3],
}

struct Header {
    segments: Option<Segments>,
    simple_filter: bool,
    filter_level: i32,
    sharpness: i32,
    filter_deltas: Option<([i32; 4], [i32; 4])>,
    partitions_log2: u32,
    quantizers: [Quantizer; 4],
    probabilities: Vec<u8>,
    skip_probability: Option<u8>,
}

impl Header {
    fn read(bools: &mut BoolDecoder<'_>) -> Result<Self, Error> {
        let _colour_space = bools.read_literal(1);
        let _clamping = bools.read_literal(1);
        let segments = bools.read_bool(128).then(|| Self::read_segments(bools));
        let simple_filter = bools.read_bool(128);
        let filter_level = bools.read_literal(6) as i32;
        let sharpness = bools.read_literal(3) as i32;
        let filter_deltas = bools.read_bool(128).then(|| {
            let mut deltas = ([0; 4], [0; 4]);
            if bools.read_bool(128) {
                deltas.0 = core::array::from_fn(|_| bools.read_optional_signed(6));
                deltas.1 = core::array::from_fn(|_| bools.read_optional_signed(6));
            }
            deltas
        });
        let partitions_log2 = bools.read_literal(2);
        let base = bools.read_literal(7) as i32;
        let deltas: [i32; 5] = core::array::from_fn(|_| bools.read_optional_signed(4));
        let quantizers = core::array::from_fn(|segment| {
            let index = match &segments {
                Some(data) if data.absolute => data.quantizer[segment],
                Some(data) => base + data.quantizer[segment],
                None => base,
            };
            Quantizer::new(index, deltas)
        });
        let _refresh_entropy = bools.read_literal(1);
        let mut probabilities = Vec::new();
        probabilities
            .try_reserve_exact(DEFAULT_COEFFICIENT_PROBABILITIES.len())
            .map_err(|_| Error::OutOfMemory)?;
        probabilities.extend_from_slice(&DEFAULT_COEFFICIENT_PROBABILITIES);
        let updates = COEFFICIENT_UPDATE_PROBABILITIES.iter();
        for (probability, &update) in probabilities.iter_mut().zip(updates) {
            if bools.read_bool(update) {
                *probability = bools.read_literal(8) as u8;
            }
        }
        let skip_probability = bools.read_bool(128).then(|| bools.read_literal(8) as u8);
        Ok(Self {
            segments,
            simple_filter,
            filter_level,
            sharpness,
            filter_deltas,
            partitions_log2,
            quantizers,
            probabilities,
            skip_probability,
        })
    }

    fn read_segments(bools: &mut BoolDecoder<'_>) -> Segments {
        let update_map = bools.read_bool(128);
        let update_data = bools.read_bool(128);
        let mut segments = Segments {
            update_map,
            absolute: false,
            quantizer: [0; 4],
            filter_level: [0; 4],
            tree_probabilities: [255; 3],
        };
        if update_data {
            segments.absolute = bools.read_bool(128);
            segments.quantizer = core::array::from_fn(|_| bools.read_optional_signed(7));
            segments.filter_level = core::array::from_fn(|_| bools.read_optional_signed(6));
        }
        if update_map {
            segments.tree_probabilities = core::array::from_fn(|_| {
                if bools.read_bool(128) {
                    bools.read_literal(8) as u8
                } else {
                    255
                }
            });
        }
        segments
    }

    fn macroblock_filter_level(&self, segment: usize, luma_mode: u8) -> i32 {
        let mut level = match &self.segments {
            Some(data) if data.absolute => data.filter_level[segment],
            Some(data) => self.filter_level + data.filter_level[segment],
            None => self.filter_level,
        };
        if let Some((reference, mode)) = self.filter_deltas {
            level += reference[0];
            if luma_mode == B_PRED {
                level += mode[0];
            }
        }
        level.clamp(0, 63)
    }
}

struct Modes {
    segment: usize,
    skip: bool,
    luma: u8,
    subblocks: [u8; 16],
    chroma: u8,
}

fn read_modes(
    bools: &mut BoolDecoder<'_>,
    header: &Header,
    above: &mut [u8],
    left: &mut [u8; 4],
) -> Modes {
    let segment = match &header.segments {
        Some(data) if data.update_map => {
            usize::from(bools.read_tree(&SEGMENT_TREE, &data.tree_probabilities))
        }
        _ => 0,
    };
    let skip = header.skip_probability.is_some_and(|probability| bools.read_bool(probability));
    let luma = bools.read_tree(&LUMA_TREE, &LUMA_MODE_PROBABILITIES);
    let mut subblocks = [implied_subblock_mode(luma); 16];
    if luma == B_PRED {
        for index in 0..16 {
            let above_mode = if index < 4 {
                above[index]
            } else {
                subblocks[index - 4]
            };
            let left_mode = if index % 4 == 0 {
                left[index / 4]
            } else {
                subblocks[index - 1]
            };
            let at = (usize::from(above_mode) * 10 + usize::from(left_mode)) * 9;
            let probabilities = &KEY_FRAME_SUBBLOCK_MODE_PROBABILITIES[at..at + 9];
            subblocks[index] = bools.read_tree(&SUBBLOCK_TREE, probabilities);
        }
    }
    above.copy_from_slice(&subblocks[12..16]);
    *left = core::array::from_fn(|row| subblocks[row * 4 + 3]);
    let chroma = bools.read_tree(&CHROMA_TREE, &CHROMA_MODE_PROBABILITIES);
    Modes {
        segment,
        skip,
        luma,
        subblocks,
        chroma,
    }
}

fn implied_subblock_mode(luma_mode: u8) -> u8 {
    match luma_mode {
        V_PRED => B_VE,
        H_PRED => B_HE,
        TM_PRED => B_TM,
        _ => B_DC,
    }
}

fn token_probabilities(probabilities: &[u8], kind: usize, band: usize, context: usize) -> &[u8] {
    let at = ((kind * 8 + band) * 3 + context) * 11;
    &probabilities[at..at + 11]
}

struct BlockCoding<'a> {
    probabilities: &'a [u8],
    kind: usize,
    first: usize,
    steps: (i32, i32),
}

fn read_block(
    tokens: &mut BoolDecoder<'_>,
    coding: &BlockCoding<'_>,
    context: usize,
    block: &mut [i32; 16],
) -> bool {
    let probabilities = |index: usize, context: usize| {
        token_probabilities(coding.probabilities, coding.kind, BANDS[index], context)
    };
    let mut index = coding.first;
    let mut table = probabilities(index, context);
    let mut nonzero = false;
    let mut after_zero = false;
    while index < 16 {
        if !after_zero && !tokens.read_bool(table[0]) {
            break;
        }
        if !tokens.read_bool(table[1]) {
            after_zero = true;
            index += 1;
            if index < 16 {
                table = probabilities(index, 0);
            }
            continue;
        }
        after_zero = false;
        let value = read_token_value(tokens, table);
        let signed = if tokens.read_bool(128) { -value } else { value };
        let step = if index == 0 { coding.steps.0 } else { coding.steps.1 };
        block[ZIGZAG[index]] = signed * step;
        nonzero = true;
        index += 1;
        table = probabilities(index, if value == 1 { 1 } else { 2 });
    }
    nonzero
}

fn read_token_value(tokens: &mut BoolDecoder<'_>, table: &[u8]) -> i32 {
    if !tokens.read_bool(table[2]) {
        return 1;
    }
    if !tokens.read_bool(table[3]) {
        if !tokens.read_bool(table[4]) {
            return 2;
        }
        return 3 + i32::from(tokens.read_bool(table[5]));
    }
    let category = if !tokens.read_bool(table[6]) {
        usize::from(tokens.read_bool(table[7]))
    } else if !tokens.read_bool(table[8]) {
        2 + usize::from(tokens.read_bool(table[9]))
    } else {
        4 + usize::from(tokens.read_bool(table[10]))
    };
    let extra = CATEGORY_PROBABILITIES[category]
        .iter()
        .fold(0, |value, &probability| value << 1 | i32::from(tokens.read_bool(probability)));
    CATEGORY_BASES[category] + extra
}

fn read_coefficients(
    tokens: &mut BoolDecoder<'_>,
    header: &Header,
    modes: &Modes,
    above: &mut [u8; 9],
    left: &mut [u8; 9],
    blocks: &mut [[i32; 16]; 25],
) -> bool {
    let has_y2 = modes.luma != B_PRED;
    if modes.skip {
        let (above_y2, left_y2) = (above[Y2_CONTEXT], left[Y2_CONTEXT]);
        *above = [0; 9];
        *left = [0; 9];
        if !has_y2 {
            above[Y2_CONTEXT] = above_y2;
            left[Y2_CONTEXT] = left_y2;
        }
        return false;
    }
    let quantizer = header.quantizers[modes.segment];
    let probabilities = &header.probabilities;
    let mut any = false;
    if has_y2 {
        let coding = BlockCoding {
            probabilities,
            kind: 1,
            first: 0,
            steps: quantizer.y2,
        };
        let context = usize::from(above[Y2_CONTEXT] + left[Y2_CONTEXT]);
        let nonzero = read_block(tokens, &coding, context, &mut blocks[24]);
        above[Y2_CONTEXT] = u8::from(nonzero);
        left[Y2_CONTEXT] = u8::from(nonzero);
        any |= nonzero;
    }
    let luma = BlockCoding {
        probabilities,
        kind: if has_y2 { 0 } else { 3 },
        first: usize::from(has_y2),
        steps: quantizer.y,
    };
    for (index, block) in blocks[..16].iter_mut().enumerate() {
        let (row, column) = (index / 4, index % 4);
        let nonzero = read_block(tokens, &luma, usize::from(above[column] + left[row]), block);
        above[column] = u8::from(nonzero);
        left[row] = u8::from(nonzero);
        any |= nonzero;
    }
    let chroma = BlockCoding {
        probabilities,
        kind: 2,
        first: 0,
        steps: quantizer.uv,
    };
    for (index, block) in blocks[16..24].iter_mut().enumerate() {
        let slot = 4 + (index / 4) * 2;
        let (row, column) = (slot + (index % 4) / 2, slot + index % 2);
        let nonzero = read_block(tokens, &chroma, usize::from(above[column] + left[row]), block);
        above[column] = u8::from(nonzero);
        left[row] = u8::from(nonzero);
        any |= nonzero;
    }
    any
}

fn inverse_walsh(input: &[i32; 16]) -> [i32; 16] {
    let mut temp = [0i32; 16];
    for i in 0..4 {
        let (a, b) = (input[i] + input[12 + i], input[4 + i] + input[8 + i]);
        let (c, d) = (input[4 + i] - input[8 + i], input[i] - input[12 + i]);
        temp[i] = a + b;
        temp[4 + i] = c + d;
        temp[8 + i] = a - b;
        temp[12 + i] = d - c;
    }
    let mut out = [0i32; 16];
    for (row, values) in temp.chunks_exact(4).enumerate() {
        let (a, b) = (values[0] + values[3], values[1] + values[2]);
        let (c, d) = (values[1] - values[2], values[0] - values[3]);
        out[row * 4] = (a + b + 3) >> 3;
        out[row * 4 + 1] = (c + d + 3) >> 3;
        out[row * 4 + 2] = (a - b + 3) >> 3;
        out[row * 4 + 3] = (d - c + 3) >> 3;
    }
    out
}

fn inverse_dct(input: &[i32; 16]) -> [i32; 16] {
    const COS_MINUS_ONE: i32 = 20091;
    const SIN: i32 = 35468;
    let rotate = |x: i32, y: i32| {
        let c = ((x * SIN) >> 16) - (y + ((y * COS_MINUS_ONE) >> 16));
        let d = (x + ((x * COS_MINUS_ONE) >> 16)) + ((y * SIN) >> 16);
        (c, d)
    };
    let mut temp = [0i32; 16];
    for i in 0..4 {
        let (a, b) = (input[i] + input[8 + i], input[i] - input[8 + i]);
        let (c, d) = rotate(input[4 + i], input[12 + i]);
        temp[i] = a + d;
        temp[4 + i] = b + c;
        temp[8 + i] = b - c;
        temp[12 + i] = a - d;
    }
    let mut out = [0i32; 16];
    for (row, values) in temp.chunks_exact(4).enumerate() {
        let (a, b) = (values[0] + values[2], values[0] - values[2]);
        let (c, d) = rotate(values[1], values[3]);
        out[row * 4] = (a + d + 4) >> 3;
        out[row * 4 + 1] = (b + c + 4) >> 3;
        out[row * 4 + 2] = (b - c + 4) >> 3;
        out[row * 4 + 3] = (a - d + 4) >> 3;
    }
    out
}

struct Plane {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
}

impl Plane {
    fn new(width: usize, height: usize) -> Result<Self, Error> {
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(width * height).map_err(|_| Error::OutOfMemory)?;
        pixels.resize(width * height, 0);
        Ok(Self {
            pixels,
            width,
            height,
        })
    }

    fn at(&self, x: isize, y: isize) -> i32 {
        if y < 0 {
            return 127;
        }
        if x < 0 {
            return 129;
        }
        let x = (x as usize).min(self.width - 1);
        let y = (y as usize).min(self.height - 1);
        i32::from(self.pixels[y * self.width + x])
    }

    fn put(&mut self, x: usize, y: usize, value: i32) {
        self.pixels[y * self.width + x] = value.clamp(0, 255) as u8;
    }

    fn add_residue(
        &mut self,
        x: usize,
        y: usize,
        prediction: impl Fn(usize, usize) -> i32,
        block: &[i32; 16],
    ) {
        let residue = inverse_dct(block);
        for (index, value) in residue.iter().enumerate() {
            let (row, column) = (index / 4, index % 4);
            self.put(x + column, y + row, prediction(row, column) + value);
        }
    }
}

fn predict_block(plane: &Plane, x: usize, y: usize, size: usize, mode: u8) -> Vec<i32> {
    let (sx, sy) = (x as isize, y as isize);
    let above: Vec<i32> = (0..size as isize).map(|i| plane.at(sx + i, sy - 1)).collect();
    let left: Vec<i32> = (0..size as isize).map(|i| plane.at(sx - 1, sy + i)).collect();
    let corner = plane.at(sx - 1, sy - 1);
    (0..size * size)
        .map(|index| {
            let (row, column) = (index / size, index % size);
            match mode {
                V_PRED => above[column],
                H_PRED => left[row],
                TM_PRED => (left[row] + above[column] - corner).clamp(0, 255),
                _ => edge_average(&above, &left, x > 0, y > 0),
            }
        })
        .collect()
}

fn edge_average(above: &[i32], left: &[i32], has_left: bool, has_above: bool) -> i32 {
    let size = above.len() as i32;
    let shift = size.trailing_zeros();
    let (above_sum, left_sum) = (above.iter().sum::<i32>(), left.iter().sum::<i32>());
    match (has_above, has_left) {
        (true, true) => (above_sum + left_sum + size) >> (shift + 1),
        (true, false) => (above_sum + size / 2) >> shift,
        (false, true) => (left_sum + size / 2) >> shift,
        (false, false) => 128,
    }
}

fn avg3(x: i32, y: i32, z: i32) -> i32 {
    (x + y + y + z + 2) >> 2
}

fn avg2(x: i32, y: i32) -> i32 {
    (x + y + 1) >> 1
}

fn grid(cell: impl Fn(usize, usize) -> i32) -> [[i32; 4]; 4] {
    core::array::from_fn(|row| core::array::from_fn(|column| cell(row, column)))
}

fn placed(cells: &[(usize, usize, i32); 16]) -> [[i32; 4]; 4] {
    let mut block = [[0i32; 4]; 4];
    for &(row, column, value) in cells {
        block[row][column] = value;
    }
    block
}

fn predict_subblock(above: &[i32; 8], left: &[i32; 4], corner: i32, mode: u8) -> [[i32; 4]; 4] {
    let edge: [i32; 13] = core::array::from_fn(|i| match i {
        0..=3 => left[3 - i],
        4 => corner,
        _ => above[i - 5],
    });
    let smooth = |i: usize| avg3(edge[i - 1], edge[i], edge[i + 1]);
    let half = |i: usize| avg2(edge[i], edge[i + 1]);
    let [l0, l1, l2, l3] = *left;
    match mode {
        B_TM => grid(|row, column| (left[row] + above[column] - corner).clamp(0, 255)),
        B_VE => grid(|_, column| smooth(5 + column)),
        B_HE => [smooth(3), smooth(2), smooth(1), avg3(l2, l3, l3)].map(|value| [value; 4]),
        B_LD => grid(|row, column| match row + column {
            6 => avg3(edge[11], edge[12], edge[12]),
            sum => smooth(6 + sum),
        }),
        B_RD => grid(|row, column| smooth(4 + column - row)),
        B_VR => placed(&[
            (3, 0, smooth(2)),
            (2, 0, smooth(3)),
            (3, 1, smooth(4)),
            (1, 0, smooth(4)),
            (2, 1, half(4)),
            (0, 0, half(4)),
            (3, 2, smooth(5)),
            (1, 1, smooth(5)),
            (2, 2, half(5)),
            (0, 1, half(5)),
            (3, 3, smooth(6)),
            (1, 2, smooth(6)),
            (2, 3, half(6)),
            (0, 2, half(6)),
            (1, 3, smooth(7)),
            (0, 3, half(7)),
        ]),
        B_VL => placed(&[
            (0, 0, half(5)),
            (1, 0, smooth(6)),
            (2, 0, half(6)),
            (0, 1, half(6)),
            (1, 1, smooth(7)),
            (3, 0, smooth(7)),
            (2, 1, half(7)),
            (0, 2, half(7)),
            (3, 1, smooth(8)),
            (1, 2, smooth(8)),
            (2, 2, half(8)),
            (0, 3, half(8)),
            (3, 2, smooth(9)),
            (1, 3, smooth(9)),
            (2, 3, smooth(10)),
            (3, 3, smooth(11)),
        ]),
        B_HD => placed(&[
            (3, 0, half(0)),
            (3, 1, smooth(1)),
            (2, 0, half(1)),
            (3, 2, half(1)),
            (2, 1, smooth(2)),
            (3, 3, smooth(2)),
            (2, 2, half(2)),
            (1, 0, half(2)),
            (2, 3, smooth(3)),
            (1, 1, smooth(3)),
            (1, 2, half(3)),
            (0, 0, half(3)),
            (1, 3, smooth(4)),
            (0, 1, smooth(4)),
            (0, 2, smooth(5)),
            (0, 3, smooth(6)),
        ]),
        B_HU => placed(&[
            (0, 0, avg2(l0, l1)),
            (0, 1, avg3(l0, l1, l2)),
            (0, 2, avg2(l1, l2)),
            (1, 0, avg2(l1, l2)),
            (0, 3, avg3(l1, l2, l3)),
            (1, 1, avg3(l1, l2, l3)),
            (1, 2, avg2(l2, l3)),
            (2, 0, avg2(l2, l3)),
            (1, 3, avg3(l2, l3, l3)),
            (2, 1, avg3(l2, l3, l3)),
            (2, 2, l3),
            (2, 3, l3),
            (3, 0, l3),
            (3, 1, l3),
            (3, 2, l3),
            (3, 3, l3),
        ]),
        _ => [[(above[..4].iter().sum::<i32>() + left.iter().sum::<i32>() + 4) >> 3; 4]; 4],
    }
}

fn reconstruct(
    luma: &mut Plane,
    chroma: &mut [Plane; 2],
    (mb_x, mb_y): (usize, usize),
    modes: &Modes,
    blocks: &mut [[i32; 16]; 25],
) {
    if modes.luma != B_PRED {
        let dc = inverse_walsh(&blocks[24]);
        for (block, value) in blocks[..16].iter_mut().zip(dc) {
            block[0] = value;
        }
    }
    let (x0, y0) = (mb_x * 16, mb_y * 16);
    if modes.luma == B_PRED {
        for (index, block) in blocks[..16].iter().enumerate() {
            let (x, y) = (x0 + (index % 4) * 4, y0 + (index / 4) * 4);
            let (sx, sy) = (x as isize, y as isize);
            let (right_x, right_y) = if index % 4 == 3 {
                ((x0 + 16) as isize, y0 as isize - 1)
            } else {
                (sx + 4, sy - 1)
            };
            let above: [i32; 8] = core::array::from_fn(|i| match i {
                0..=3 => luma.at(sx + i as isize, sy - 1),
                _ => luma.at(right_x + i as isize - 4, right_y),
            });
            let left: [i32; 4] = core::array::from_fn(|i| luma.at(sx - 1, sy + i as isize));
            let corner = luma.at(sx - 1, sy - 1);
            let prediction = predict_subblock(&above, &left, corner, modes.subblocks[index]);
            luma.add_residue(x, y, |row, column| prediction[row][column], block);
        }
    } else {
        let prediction = predict_block(luma, x0, y0, 16, modes.luma);
        for (index, block) in blocks[..16].iter().enumerate() {
            let (bx, by) = ((index % 4) * 4, (index / 4) * 4);
            let predicted = |row: usize, column: usize| prediction[(by + row) * 16 + bx + column];
            luma.add_residue(x0 + bx, y0 + by, predicted, block);
        }
    }
    for (plane, plane_blocks) in chroma.iter_mut().zip(blocks[16..24].chunks_exact(4)) {
        let (cx, cy) = (mb_x * 8, mb_y * 8);
        let prediction = predict_block(plane, cx, cy, 8, modes.chroma);
        for (index, block) in plane_blocks.iter().enumerate() {
            let (bx, by) = ((index % 2) * 4, (index / 2) * 4);
            let predicted = |row: usize, column: usize| prediction[(by + row) * 8 + bx + column];
            plane.add_residue(cx + bx, cy + by, predicted, block);
        }
    }
}

#[derive(Clone, Copy)]
struct EdgeLimits {
    edge: i32,
    interior: i32,
    high_variance: i32,
}

#[derive(Clone, Copy)]
struct FilterStrength {
    level: i32,
    inner_edges: bool,
}

fn clamp_signed(value: i32) -> i32 {
    value.clamp(-128, 127)
}

fn filter_edge(
    pixels: &mut [u8],
    origin: usize,
    step: usize,
    limits: EdgeLimits,
    macroblock_edge: bool,
    simple: bool,
) {
    let at = |offset: isize| origin.wrapping_add_signed(offset * step as isize);
    let get = |pixels: &[u8], offset: isize| i32::from(pixels[at(offset)]);
    let (p3, p2, p1, p0) = (get(pixels, -4), get(pixels, -3), get(pixels, -2), get(pixels, -1));
    let (q0, q1, q2, q3) = (get(pixels, 0), get(pixels, 1), get(pixels, 2), get(pixels, 3));
    if (p0 - q0).abs() * 2 + ((p1 - q1).abs() >> 1) > limits.edge {
        return;
    }
    let to_pixel = |value: i32| (clamp_signed(value) + 128) as u8;
    let (sp2, sp1, sp0) = (p2 - 128, p1 - 128, p0 - 128);
    let (sq0, sq1, sq2) = (q0 - 128, q1 - 128, q2 - 128);
    let adjust = |pixels: &mut [u8], outer_taps: bool| {
        let outer = if outer_taps { clamp_signed(sp1 - sq1) } else { 0 };
        let a = clamp_signed(outer + 3 * (sq0 - sp0));
        let q_step = clamp_signed(a + 4) >> 3;
        let p_step = clamp_signed(a + 3) >> 3;
        pixels[at(0)] = to_pixel(sq0 - q_step);
        pixels[at(-1)] = to_pixel(sp0 + p_step);
        q_step
    };
    if simple {
        adjust(pixels, true);
        return;
    }
    let interior = [p3 - p2, p2 - p1, p1 - p0, q1 - q0, q2 - q1, q3 - q2];
    if interior.iter().any(|difference| difference.abs() > limits.interior) {
        return;
    }
    let high_variance =
        (p1 - p0).abs() > limits.high_variance || (q1 - q0).abs() > limits.high_variance;
    if high_variance {
        adjust(pixels, true);
    } else if macroblock_edge {
        let w = clamp_signed(clamp_signed(sp1 - sq1) + 3 * (sq0 - sp0));
        for (offset, weight, p, q) in [(0, 27, sp0, sq0), (1, 18, sp1, sq1), (2, 9, sp2, sq2)] {
            let a = clamp_signed((weight * w + 63) >> 7);
            pixels[at(offset)] = to_pixel(q - a);
            pixels[at(-1 - offset)] = to_pixel(p + a);
        }
    } else {
        let a = (adjust(pixels, false) + 1) >> 1;
        pixels[at(1)] = to_pixel(sq1 - a);
        pixels[at(-2)] = to_pixel(sp1 + a);
    }
}

fn filter_plane(
    plane: &mut Plane,
    size: usize,
    strengths: &[FilterStrength],
    columns: usize,
    sharpness: i32,
    simple: bool,
) {
    let width = plane.width;
    let inner_offsets: &[usize] = if size == 16 { &[4, 8, 12] } else { &[4] };
    for (index, strength) in strengths.iter().enumerate() {
        let level = strength.level;
        if level == 0 {
            continue;
        }
        let mut interior = level;
        if sharpness > 0 {
            interior >>= if sharpness > 4 { 2 } else { 1 };
            interior = interior.min(9 - sharpness);
        }
        let interior = interior.max(1);
        let high_variance = match level {
            40.. => 2,
            15.. => 1,
            _ => 0,
        };
        let macroblock = EdgeLimits {
            edge: (level + 2) * 2 + interior,
            interior,
            high_variance,
        };
        let subblock = EdgeLimits {
            edge: level * 2 + interior,
            ..macroblock
        };
        let (x0, y0) = ((index % columns) * size, (index / columns) * size);
        let column = |x: usize| (0..size).map(move |i| (y0 + i) * width + x);
        let row = |y: usize| (0..size).map(move |i| y * width + x0 + i);
        if x0 > 0 {
            for origin in column(x0) {
                filter_edge(&mut plane.pixels, origin, 1, macroblock, true, simple);
            }
        }
        if strength.inner_edges {
            for &offset in inner_offsets {
                for origin in column(x0 + offset) {
                    filter_edge(&mut plane.pixels, origin, 1, subblock, false, simple);
                }
            }
        }
        if y0 > 0 {
            for origin in row(y0) {
                filter_edge(&mut plane.pixels, origin, width, macroblock, true, simple);
            }
        }
        if strength.inner_edges {
            for &offset in inner_offsets {
                for origin in row(y0 + offset) {
                    filter_edge(&mut plane.pixels, origin, width, subblock, false, simple);
                }
            }
        }
    }
}

fn to_argb(
    luma: &Plane,
    chroma: &[Plane; 2],
    width: usize,
    height: usize,
) -> Result<Vec<u32>, Error> {
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(width * height).map_err(|_| Error::OutOfMemory)?;
    let high = |value: i32, coefficient: i32| (value * coefficient) >> 8;
    let channel = |value: i32| (value >> 6).clamp(0, 255) as u32;
    for y in 0..height {
        for x in 0..width {
            let scaled = high(i32::from(luma.pixels[y * luma.width + x]), 19077);
            let at = (y / 2) * chroma[0].width + x / 2;
            let (u, v) = (i32::from(chroma[0].pixels[at]), i32::from(chroma[1].pixels[at]));
            let red = scaled + high(v, 26149) - 14234;
            let green = scaled - high(u, 6419) - high(v, 13320) + 8708;
            let blue = scaled + high(u, 33050) - 17685;
            pixels.push(0xff00_0000 | channel(red) << 16 | channel(green) << 8 | channel(blue));
        }
    }
    Ok(pixels)
}

fn token_partitions(
    data: &[u8],
    start: usize,
    count: usize,
) -> Result<Vec<BoolDecoder<'_>>, Error> {
    let sizes_bytes = 3 * (count - 1);
    let sizes = data.get(start..start + sizes_bytes).ok_or(Error::Truncated)?;
    let mut partitions = Vec::with_capacity(count);
    let mut offset = start + sizes_bytes;
    for index in 0..count {
        let end = match sizes.get(3 * index..3 * index + 3) {
            Some(size) => {
                let size = u32::from_le_bytes([size[0], size[1], size[2], 0]) as usize;
                offset.checked_add(size).ok_or(Error::Corrupt)?
            }
            None => data.len(),
        };
        partitions.push(BoolDecoder::new(data.get(offset..end).ok_or(Error::Truncated)?));
        offset = end;
    }
    Ok(partitions)
}

/// Decodes the VP8 key frame `data`, a `VP8 ` chunk's body, to opaque pixels.
pub fn decode(data: &[u8], limits: &Limits) -> Result<Image, Error> {
    let tag = data.get(..3).ok_or(Error::Truncated)?;
    let tag = u32::from_le_bytes([tag[0], tag[1], tag[2], 0]);
    if tag & 1 != 0 {
        return Err(Error::Unsupported);
    }
    if data.get(3..6) != Some(&START_CODE[..]) {
        return Err(Error::Corrupt);
    }
    let dimension = |at: usize| -> Result<usize, Error> {
        let bytes = data.get(at..at + 2).ok_or(Error::Truncated)?;
        Ok(usize::from(u16::from_le_bytes([bytes[0], bytes[1]]) & 0x3fff))
    };
    let (width, height) = (dimension(6)?, dimension(8)?);
    if width == 0 || height == 0 {
        return Err(Error::Corrupt);
    }
    if width as u32 > limits.max_side
        || height as u32 > limits.max_side
        || (width * height) as u64 > limits.max_pixels
    {
        return Err(Error::TooLarge);
    }
    let (columns, rows) = (width.div_ceil(16), height.div_ceil(16));
    let working = (columns as u64 * rows as u64).checked_mul(MACROBLOCK_BYTES);
    crate::check_working_bytes(working, limits)?;
    let first_end = FIRST_PARTITION_START + (tag >> 5) as usize;
    let first = data.get(FIRST_PARTITION_START..first_end).ok_or(Error::Truncated)?;
    let mut bools = BoolDecoder::new(first);
    let header = Header::read(&mut bools)?;
    let mut partitions = token_partitions(data, first_end, 1 << header.partitions_log2)?;
    let partition_count = partitions.len();
    let mut luma = Plane::new(columns * 16, rows * 16)?;
    let mut chroma = [Plane::new(columns * 8, rows * 8)?, Plane::new(columns * 8, rows * 8)?];
    let mut above_modes = vec_of(columns * 4, B_DC)?;
    let mut above_contexts = vec_of(columns, [0u8; 9])?;
    let mut strengths = Vec::new();
    strengths.try_reserve_exact(columns * rows).map_err(|_| Error::OutOfMemory)?;
    for mb_y in 0..rows {
        let mut left_modes = [B_DC; 4];
        let mut left_context = [0u8; 9];
        let tokens = &mut partitions[mb_y % partition_count];
        for mb_x in 0..columns {
            let above = &mut above_modes[mb_x * 4..mb_x * 4 + 4];
            let modes = read_modes(&mut bools, &header, above, &mut left_modes);
            let mut blocks = [[0i32; 16]; 25];
            let above_context = &mut above_contexts[mb_x];
            let coded = read_coefficients(
                tokens,
                &header,
                &modes,
                above_context,
                &mut left_context,
                &mut blocks,
            );
            reconstruct(&mut luma, &mut chroma, (mb_x, mb_y), &modes, &mut blocks);
            strengths.push(FilterStrength {
                level: header.macroblock_filter_level(modes.segment, modes.luma),
                inner_edges: coded || modes.luma == B_PRED,
            });
        }
    }
    if header.filter_level > 0 {
        let simple = header.simple_filter;
        filter_plane(&mut luma, 16, &strengths, columns, header.sharpness, simple);
        if !simple {
            for plane in &mut chroma {
                filter_plane(plane, 8, &strengths, columns, header.sharpness, false);
            }
        }
    }
    Ok(Image {
        width: width as u32,
        height: height as u32,
        pixels: to_argb(&luma, &chroma, width, height)?,
    })
}

fn vec_of<T: Clone>(count: usize, value: T) -> Result<Vec<T>, Error> {
    let mut items = Vec::new();
    items.try_reserve_exact(count).map_err(|_| Error::OutOfMemory)?;
    items.resize(count, value);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    struct BoolEncoder {
        output: Vec<u8>,
        range: u32,
        bottom: u32,
        bit_count: i32,
    }

    impl BoolEncoder {
        fn new() -> Self {
            Self {
                output: Vec::new(),
                range: 255,
                bottom: 0,
                bit_count: 24,
            }
        }

        fn write_bool(&mut self, probability: u8, value: bool) {
            let split = 1 + (((self.range - 1) * u32::from(probability)) >> 8);
            if value {
                self.bottom = self.bottom.wrapping_add(split);
                self.range -= split;
            } else {
                self.range = split;
            }
            while self.range < 128 {
                self.range <<= 1;
                if self.bottom & 1 << 31 != 0 {
                    for byte in self.output.iter_mut().rev() {
                        if *byte == 255 {
                            *byte = 0;
                        } else {
                            *byte += 1;
                            break;
                        }
                    }
                }
                self.bottom <<= 1;
                self.bit_count -= 1;
                if self.bit_count == 0 {
                    self.output.push((self.bottom >> 24) as u8);
                    self.bottom &= (1 << 24) - 1;
                    self.bit_count = 8;
                }
            }
        }

        fn write_literal(&mut self, value: u32, count: u32) {
            for bit in (0..count).rev() {
                self.write_bool(128, value >> bit & 1 != 0);
            }
        }

        fn write_tree(&mut self, tree: &[i8], probabilities: &[u8], value: u8) {
            let path = tree_path(tree, 0, value).unwrap();
            let mut node = 0usize;
            for bit in path {
                self.write_bool(probabilities[node >> 1], bit);
                node = tree[node + usize::from(bit)].max(0) as usize;
            }
        }

        fn finish(mut self) -> Vec<u8> {
            for _ in 0..32 {
                self.write_bool(128, false);
            }
            self.output
        }
    }

    fn tree_path(tree: &[i8], node: usize, value: u8) -> Option<Vec<bool>> {
        for bit in [false, true] {
            let next = tree[node + usize::from(bit)];
            let found = if next <= 0 {
                (next.unsigned_abs() == value).then(Vec::new)
            } else {
                tree_path(tree, next as usize, value)
            };
            if let Some(mut rest) = found {
                rest.insert(0, bit);
                return Some(rest);
            }
        }
        None
    }

    #[test]
    fn literals_and_tree_leaves_survive_the_boolean_coder() {
        let mut encoder = BoolEncoder::new();
        encoder.write_literal(0x5a, 8);
        encoder.write_tree(&SUBBLOCK_TREE, &[120; 9], B_HU);
        encoder.write_tree(&LUMA_TREE, &LUMA_MODE_PROBABILITIES, DC_PRED);
        encoder.write_bool(1, true);
        let bytes = encoder.finish();
        let mut decoder = BoolDecoder::new(&bytes);
        assert_eq!(decoder.read_literal(8), 0x5a);
        assert_eq!(decoder.read_tree(&SUBBLOCK_TREE, &[120; 9]), B_HU);
        assert_eq!(decoder.read_tree(&LUMA_TREE, &LUMA_MODE_PROBABILITIES), DC_PRED);
        assert!(decoder.read_bool(1));
    }

    #[test]
    fn the_inverse_transforms_spread_a_dc_evenly() {
        let mut block = [0i32; 16];
        block[0] = 8 * 16;
        assert!(inverse_dct(&block).iter().all(|&value| value == 16));
        let mut walsh = [0i32; 16];
        walsh[0] = 8 * 3;
        assert!(inverse_walsh(&walsh).iter().all(|&value| value == 3));
    }

    #[test]
    fn subblock_dc_averages_its_eight_edge_pixels() {
        let above = [10, 10, 10, 10, 0, 0, 0, 0];
        let left = [20, 20, 20, 20];
        assert_eq!(predict_subblock(&above, &left, 0, B_DC), [[15; 4]; 4]);
    }

    #[test]
    fn subblock_vertical_left_follows_the_above_edge() {
        let above = [0, 4, 8, 12, 16, 20, 24, 28];
        let block = predict_subblock(&above, &[0; 4], 0, B_VL);
        assert_eq!(block[0], [2, 6, 10, 14]);
        assert_eq!(block[3][3], 24);
    }

    fn skipped_frame(width: u16, height: u16) -> Vec<u8> {
        let mut header = BoolEncoder::new();
        header.write_literal(0, 2);
        header.write_bool(128, false);
        header.write_bool(128, false);
        header.write_literal(0, 6);
        header.write_literal(0, 3);
        header.write_bool(128, false);
        header.write_literal(0, 2);
        header.write_literal(10, 7);
        for _ in 0..5 {
            header.write_bool(128, false);
        }
        header.write_literal(0, 1);
        for &update in &COEFFICIENT_UPDATE_PROBABILITIES {
            header.write_bool(update, false);
        }
        header.write_bool(128, true);
        header.write_literal(200, 8);
        let macroblocks = usize::from(width).div_ceil(16) * usize::from(height).div_ceil(16);
        for _ in 0..macroblocks {
            header.write_bool(200, true);
            header.write_tree(&LUMA_TREE, &LUMA_MODE_PROBABILITIES, DC_PRED);
            header.write_tree(&CHROMA_TREE, &CHROMA_MODE_PROBABILITIES, DC_PRED);
        }
        let first = header.finish();
        let tag = (first.len() as u32) << 5 | 1 << 4;
        let mut data = tag.to_le_bytes()[..3].to_vec();
        data.extend_from_slice(&START_CODE);
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        data.extend(first);
        data
    }

    #[test]
    fn a_skipped_dc_frame_decodes_to_mid_grey() {
        let image = decode(&skipped_frame(20, 18), &Limits::DEFAULT).unwrap();
        assert_eq!((image.width, image.height), (20, 18));
        assert_eq!(image.pixels, vec![0xff82_8282; 20 * 18]);
    }

    #[test]
    fn inter_frames_are_unsupported_and_a_bad_start_code_is_corrupt() {
        let mut frame = skipped_frame(16, 16);
        frame[0] |= 1;
        assert_eq!(decode(&frame, &Limits::DEFAULT).unwrap_err(), Error::Unsupported);
        let mut frame = skipped_frame(16, 16);
        frame[3] = 0;
        assert_eq!(decode(&frame, &Limits::DEFAULT).unwrap_err(), Error::Corrupt);
    }
}
