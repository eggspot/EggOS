use crate::bytes::{be16, byte};
use crate::{Error, Image, Limits};
use alloc::vec::Vec;

const SOI: u8 = 0xd8;
const EOI: u8 = 0xd9;
const SOS: u8 = 0xda;
const DQT: u8 = 0xdb;
const DRI: u8 = 0xdd;
const DHT: u8 = 0xc4;
const SOF_BASELINE: u8 = 0xc0;
const SOF_EXTENDED: u8 = 0xc1;
const SOF_PROGRESSIVE: u8 = 0xc2;
const APP_JFIF: u8 = 0xe0;
const APP_ADOBE: u8 = 0xee;
const RESTART_FIRST: u8 = 0xd0;
const RESTART_LAST: u8 = 0xd7;
const TEMPORARY: u8 = 0x01;
const ADOBE_TRANSFORM_AT: usize = 11;
const ADOBE_RGB: u8 = 0;
const ADOBE_YCCK: u8 = 2;
const MAX_COMPONENTS: usize = 4;
const MAX_SAMPLING: usize = 4;
const MAX_TABLES: usize = 4;
const MAX_SUCCESSIVE_BIT: u8 = 13;
const MAX_EXTEND_BITS: u8 = 16;
const LOOKUP_BITS: u32 = 9;
const MAX_CODE_LENGTH: usize = 16;
const PASS_LIMIT: i32 = 65_535;
const COEFFICIENT_LIMIT: i32 = 32_767;
const FIXED_ONE_HALF: i32 = 1 << 15;
const CR_TO_RED: i32 = 91_881;
const CB_TO_GREEN: i32 = 22_554;
const CR_TO_GREEN: i32 = 46_802;
const CB_TO_BLUE: i32 = 116_130;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const BASIS: [[i32; 4]; 8] = [
    [1448, 1448, 1448, 1448],
    [2009, 1703, 1138, 400],
    [1892, 784, -784, -1892],
    [1703, -400, -2009, -1138],
    [1448, -1448, -1448, 1448],
    [1138, -2009, 400, 1703],
    [784, -1892, 1892, -784],
    [400, -1138, 1703, -2009],
];

struct Huffman {
    lookup: Vec<u16>,
    max_code: [i32; MAX_CODE_LENGTH + 1],
    offset: [i32; MAX_CODE_LENGTH + 1],
    values: [u8; 256],
}

impl Huffman {
    fn new(counts: &[u8], symbols: &[u8]) -> Result<Self, Error> {
        let mut lookup = crate::buffer(1 << LOOKUP_BITS, 0u16)?;
        let mut values = [0u8; 256];
        values
            .get_mut(..symbols.len())
            .ok_or(Error::Corrupt)?
            .copy_from_slice(symbols);
        let mut max_code = [-1i32; MAX_CODE_LENGTH + 1];
        let mut offset = [0i32; MAX_CODE_LENGTH + 1];
        let (mut code, mut index) = (0i32, 0i32);
        for length in 1..=MAX_CODE_LENGTH {
            offset[length] = index - code;
            for _ in 0..counts[length - 1] {
                if code >= 1 << length {
                    return Err(Error::Corrupt);
                }
                if length as u32 <= LOOKUP_BITS {
                    let shift = LOOKUP_BITS - length as u32;
                    let first = (code as usize) << shift;
                    let entry = u16::from(values[index as usize]) << 8 | length as u16;
                    lookup[first..first + (1 << shift)].fill(entry);
                }
                code += 1;
                index += 1;
            }
            if counts[length - 1] > 0 {
                max_code[length] = code - 1;
            }
            code <<= 1;
        }
        Ok(Self {
            lookup,
            max_code,
            offset,
            values,
        })
    }
}

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    buffer: u64,
    count: u32,
    marker: Option<u8>,
    padded: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], at: usize) -> Self {
        Self {
            data,
            at,
            buffer: 0,
            count: 0,
            marker: None,
            padded: 0,
        }
    }

    fn next_byte(&mut self) -> u8 {
        if self.marker.is_some() {
            return 0;
        }
        let Some(&value) = self.data.get(self.at) else {
            self.padded = self.padded.saturating_add(1);
            return 0;
        };
        if value != 0xff {
            self.at += 1;
            return value;
        }
        let mut next = self.at + 1;
        while self.data.get(next) == Some(&0xff) {
            next += 1;
        }
        match self.data.get(next) {
            Some(0) => {
                self.at = next + 1;
                0xff
            }
            Some(&marker) => {
                self.at = next - 1;
                self.marker = Some(marker);
                0
            }
            None => {
                self.at = next;
                self.padded = self.padded.saturating_add(1);
                0
            }
        }
    }

    fn refill(&mut self) {
        while self.count <= 56 {
            let value = self.next_byte();
            self.buffer |= u64::from(value) << (56 - self.count);
            self.count += 8;
        }
    }

    fn consume(&mut self, count: u32) {
        self.buffer <<= count;
        self.count -= count;
    }

    fn bits(&mut self, count: u32) -> u32 {
        if count == 0 {
            return 0;
        }
        if self.count < count {
            self.refill();
        }
        let value = (self.buffer >> (64 - count)) as u32;
        self.consume(count);
        value
    }

    fn bit(&mut self) -> bool {
        self.bits(1) == 1
    }

    fn extend(&mut self, size: u8) -> Result<i32, Error> {
        if size == 0 {
            return Ok(0);
        }
        if size > MAX_EXTEND_BITS {
            return Err(Error::Corrupt);
        }
        let size = u32::from(size);
        let value = self.bits(size) as i32;
        Ok(match value < 1 << (size - 1) {
            true => value - (1 << size) + 1,
            false => value,
        })
    }

    fn decode(&mut self, table: &Huffman) -> Result<u8, Error> {
        if self.count < MAX_CODE_LENGTH as u32 {
            self.refill();
        }
        let entry = table.lookup[(self.buffer >> (64 - LOOKUP_BITS)) as usize];
        if entry != 0 {
            self.consume(u32::from(entry & 0xff));
            return Ok((entry >> 8) as u8);
        }
        for length in LOOKUP_BITS as usize + 1..=MAX_CODE_LENGTH {
            let code = (self.buffer >> (64 - length)) as i32;
            if code <= table.max_code[length] {
                self.consume(length as u32);
                let at =
                    usize::try_from(code + table.offset[length]).map_err(|_| Error::Corrupt)?;
                return table.values.get(at).copied().ok_or(Error::Corrupt);
            }
        }
        Err(Error::Corrupt)
    }

    fn restart(&mut self) {
        self.buffer = 0;
        self.count = 0;
        match self.marker {
            Some(marker) if (RESTART_FIRST..=RESTART_LAST).contains(&marker) => {
                self.at += 2;
                self.marker = None;
            }
            Some(_) => {}
            None => {
                let mut at = self.at;
                while at + 1 < self.data.len() {
                    if self.data[at] == 0xff
                        && (RESTART_FIRST..=RESTART_LAST).contains(&self.data[at + 1])
                    {
                        self.at = at + 2;
                        return;
                    }
                    at += 1;
                }
                self.at = self.data.len();
            }
        }
    }

    fn end(&self) -> usize {
        if self.marker.is_some() {
            return self.at;
        }
        let mut at = self.at;
        while at + 1 < self.data.len() {
            let next = self.data[at + 1];
            let restart = (RESTART_FIRST..=RESTART_LAST).contains(&next);
            if self.data[at] == 0xff && next != 0 && next != 0xff && !restart {
                return at;
            }
            at += 1;
        }
        self.data.len()
    }
}

struct Component {
    id: u8,
    horizontal: usize,
    vertical: usize,
    quantization: usize,
    blocks_x: usize,
    blocks_y: usize,
    width: usize,
    height: usize,
    plane: Vec<u8>,
    coefficients: Vec<i16>,
    prediction: i32,
}

impl Component {
    fn stride(&self) -> usize {
        self.blocks_x * 8
    }

    fn block_at(&self, block_x: usize, block_y: usize) -> usize {
        block_y * 8 * self.stride() + block_x * 8
    }

    fn coefficients_of(&mut self, block_x: usize, block_y: usize) -> &mut [i16] {
        let start = (block_y * self.blocks_x + block_x) * 64;
        &mut self.coefficients[start..start + 64]
    }
}

struct Frame {
    width: u32,
    height: u32,
    progressive: bool,
    components: Vec<Component>,
    horizontal_max: usize,
    vertical_max: usize,
    mcus_x: usize,
    mcus_y: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Sequential,
    DcFirst,
    DcRefine,
    AcFirst,
    AcRefine,
}

struct ScanPart<'t> {
    index: usize,
    dc: Option<&'t Huffman>,
    ac: Option<&'t Huffman>,
}

#[derive(Clone, Copy)]
struct Spectral {
    start: usize,
    end: usize,
    low: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Colour {
    Grey,
    YCbCr,
    Rgb,
    Cmyk,
    Ycck,
}

struct Decoder {
    quantization: [[u16; 64]; MAX_TABLES],
    dc: [Option<Huffman>; MAX_TABLES],
    ac: [Option<Huffman>; MAX_TABLES],
    restart_interval: usize,
    adobe: Option<u8>,
    jfif: bool,
    frame: Option<Frame>,
    scans: usize,
}

fn dequantize(value: i32, step: u16) -> i32 {
    let value = value.clamp(-COEFFICIENT_LIMIT, COEFFICIENT_LIMIT) * i32::from(step);
    value.clamp(-COEFFICIENT_LIMIT, COEFFICIENT_LIMIT)
}

fn saturated(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

fn sample(value: i32) -> u8 {
    ((value + (1 << 14)) >> 15)
        .saturating_add(128)
        .clamp(0, 255) as u8
}

fn idct(input: &[i32; 64], plane: &mut [u8], at: usize, stride: usize) {
    let mut work = [0i32; 64];
    for x in 0..8 {
        let column = |v: usize| input[v * 8 + x];
        if (1..8).all(|v| column(v) == 0) {
            let value = ((BASIS[0][0] * column(0) + 256) >> 9).clamp(-PASS_LIMIT, PASS_LIMIT);
            for n in 0..8 {
                work[n * 8 + x] = value;
            }
            continue;
        }
        for n in 0..4 {
            let even = BASIS[0][n] * column(0)
                + BASIS[2][n] * column(2)
                + BASIS[4][n] * column(4)
                + BASIS[6][n] * column(6);
            let odd = BASIS[1][n] * column(1)
                + BASIS[3][n] * column(3)
                + BASIS[5][n] * column(5)
                + BASIS[7][n] * column(7);
            work[n * 8 + x] = ((even + odd + 256) >> 9).clamp(-PASS_LIMIT, PASS_LIMIT);
            work[(7 - n) * 8 + x] = ((even - odd + 256) >> 9).clamp(-PASS_LIMIT, PASS_LIMIT);
        }
    }
    for y in 0..8 {
        let row = &work[y * 8..y * 8 + 8];
        let start = at + y * stride;
        let target = &mut plane[start..start + 8];
        for n in 0..4 {
            let even = BASIS[0][n] * row[0]
                + BASIS[2][n] * row[2]
                + BASIS[4][n] * row[4]
                + BASIS[6][n] * row[6];
            let odd = BASIS[1][n] * row[1]
                + BASIS[3][n] * row[3]
                + BASIS[5][n] * row[5]
                + BASIS[7][n] * row[7];
            target[n] = sample(even + odd);
            target[7 - n] = sample(even - odd);
        }
    }
}

fn sequential_block(
    bits: &mut Bits<'_>,
    part: &ScanPart<'_>,
    prediction: &mut i32,
    steps: &[u16; 64],
    block: &mut [i32; 64],
) -> Result<(), Error> {
    let (Some(dc), Some(ac)) = (part.dc, part.ac) else {
        return Err(Error::Corrupt);
    };
    block.fill(0);
    let size = bits.decode(dc)?;
    *prediction = prediction.wrapping_add(bits.extend(size)?);
    block[0] = dequantize(*prediction, steps[0]);
    let mut k = 1;
    while k < 64 {
        let symbol = bits.decode(ac)?;
        let (run, size) = (usize::from(symbol >> 4), symbol & 0x0f);
        if size == 0 {
            if run != 15 {
                break;
            }
            k += 16;
            continue;
        }
        k += run;
        let Some(&at) = ZIGZAG.get(k) else {
            return Err(Error::Corrupt);
        };
        block[at] = dequantize(bits.extend(size)?, steps[at]);
        k += 1;
    }
    Ok(())
}

fn dc_first(
    bits: &mut Bits<'_>,
    table: &Huffman,
    prediction: &mut i32,
    low: u32,
    coefficient: &mut i16,
) -> Result<(), Error> {
    let size = bits.decode(table)?;
    *prediction = prediction.wrapping_add(bits.extend(size)?);
    let value = prediction.clamp(-COEFFICIENT_LIMIT, COEFFICIENT_LIMIT) << low;
    *coefficient = saturated(value);
    Ok(())
}

fn ac_first(
    bits: &mut Bits<'_>,
    table: &Huffman,
    spectral: Spectral,
    end_of_bands: &mut u32,
    coefficients: &mut [i16],
) -> Result<(), Error> {
    if *end_of_bands > 0 {
        *end_of_bands -= 1;
        return Ok(());
    }
    let mut k = spectral.start;
    while k <= spectral.end {
        let symbol = bits.decode(table)?;
        let (run, size) = (u32::from(symbol >> 4), symbol & 0x0f);
        if size == 0 {
            if run < 15 {
                *end_of_bands = (1u32 << run) - 1 + bits.bits(run);
                break;
            }
            k += 16;
            continue;
        }
        k += run as usize;
        let Some(&at) = ZIGZAG.get(k) else {
            return Err(Error::Corrupt);
        };
        let value = bits
            .extend(size)?
            .clamp(-COEFFICIENT_LIMIT, COEFFICIENT_LIMIT);
        coefficients[at] = saturated(value << spectral.low);
        k += 1;
    }
    Ok(())
}

fn refine(bits: &mut Bits<'_>, coefficient: &mut i16, plus: i16) {
    if bits.bit() && *coefficient & plus == 0 {
        *coefficient = match *coefficient >= 0 {
            true => coefficient.saturating_add(plus),
            false => coefficient.saturating_sub(plus),
        };
    }
}

fn ac_refine(
    bits: &mut Bits<'_>,
    table: &Huffman,
    spectral: Spectral,
    end_of_bands: &mut u32,
    coefficients: &mut [i16],
) -> Result<(), Error> {
    let plus = 1i16 << spectral.low;
    let mut k = spectral.start;
    if *end_of_bands == 0 {
        while k <= spectral.end {
            let symbol = bits.decode(table)?;
            let (mut run, size) = (u32::from(symbol >> 4), symbol & 0x0f);
            let mut value = 0i16;
            match size {
                0 if run < 15 => {
                    *end_of_bands = (1u32 << run) + bits.bits(run);
                    break;
                }
                0 => {}
                1 => {
                    value = match bits.bit() {
                        true => plus,
                        false => -plus,
                    };
                }
                _ => return Err(Error::Corrupt),
            }
            while k <= spectral.end {
                let at = ZIGZAG[k];
                if coefficients[at] != 0 {
                    refine(bits, &mut coefficients[at], plus);
                } else {
                    if run == 0 {
                        if value != 0 {
                            coefficients[at] = value;
                        }
                        k += 1;
                        break;
                    }
                    run -= 1;
                }
                k += 1;
            }
        }
    }
    if *end_of_bands > 0 {
        while k <= spectral.end {
            let at = ZIGZAG[k];
            if coefficients[at] != 0 {
                refine(bits, &mut coefficients[at], plus);
            }
            k += 1;
        }
        *end_of_bands -= 1;
    }
    Ok(())
}

struct Progress {
    pass: Pass,
    spectral: Spectral,
    end_of_bands: u32,
    block: [i32; 64],
}

fn decode_block(
    bits: &mut Bits<'_>,
    part: &ScanPart<'_>,
    component: &mut Component,
    steps: &[u16; 64],
    progress: &mut Progress,
    (block_x, block_y): (usize, usize),
) -> Result<(), Error> {
    let spectral = progress.spectral;
    match progress.pass {
        Pass::Sequential => {
            let prediction = &mut component.prediction;
            sequential_block(bits, part, prediction, steps, &mut progress.block)?;
            let (at, stride) = (component.block_at(block_x, block_y), component.stride());
            idct(&progress.block, &mut component.plane, at, stride);
        }
        Pass::DcFirst => {
            let table = part.dc.ok_or(Error::Corrupt)?;
            let mut prediction = component.prediction;
            let coefficients = component.coefficients_of(block_x, block_y);
            dc_first(
                bits,
                table,
                &mut prediction,
                spectral.low,
                &mut coefficients[0],
            )?;
            component.prediction = prediction;
        }
        Pass::DcRefine => {
            if bits.bit() {
                component.coefficients_of(block_x, block_y)[0] |= 1 << spectral.low;
            }
        }
        Pass::AcFirst => {
            let table = part.ac.ok_or(Error::Corrupt)?;
            let coefficients = component.coefficients_of(block_x, block_y);
            ac_first(
                bits,
                table,
                spectral,
                &mut progress.end_of_bands,
                coefficients,
            )?;
        }
        Pass::AcRefine => {
            let table = part.ac.ok_or(Error::Corrupt)?;
            let coefficients = component.coefficients_of(block_x, block_y);
            ac_refine(
                bits,
                table,
                spectral,
                &mut progress.end_of_bands,
                coefficients,
            )?;
        }
    }
    Ok(())
}

fn next_marker(bytes: &[u8], mut at: usize) -> Option<(u8, usize)> {
    loop {
        while *bytes.get(at)? != 0xff {
            at += 1;
        }
        while *bytes.get(at)? == 0xff {
            at += 1;
        }
        let marker = bytes[at];
        at += 1;
        if marker != 0 {
            return Some((marker, at));
        }
    }
}

impl Decoder {
    fn new() -> Self {
        Self {
            quantization: [[1; 64]; MAX_TABLES],
            dc: [None, None, None, None],
            ac: [None, None, None, None],
            restart_interval: 0,
            adobe: None,
            jfif: false,
            frame: None,
            scans: 0,
        }
    }

    fn quantization_tables(&mut self, mut segment: &[u8]) -> Result<(), Error> {
        while let Some((&first, rest)) = segment.split_first() {
            let (precision, table) = (first >> 4, usize::from(first & 0x0f));
            let wide = match precision {
                0 => false,
                1 => true,
                _ => return Err(Error::Corrupt),
            };
            let steps = self.quantization.get_mut(table).ok_or(Error::Corrupt)?;
            let size = if wide { 128 } else { 64 };
            let values = rest.get(..size).ok_or(Error::Truncated)?;
            for (k, &at) in ZIGZAG.iter().enumerate() {
                steps[at] = match wide {
                    true => u16::from_be_bytes([values[2 * k], values[2 * k + 1]]),
                    false => u16::from(values[k]),
                };
            }
            segment = &rest[size..];
        }
        Ok(())
    }

    fn huffman_tables(&mut self, mut segment: &[u8]) -> Result<(), Error> {
        while let Some((&first, rest)) = segment.split_first() {
            let (class, index) = (first >> 4, usize::from(first & 0x0f));
            if index >= MAX_TABLES {
                return Err(Error::Corrupt);
            }
            let counts = rest.get(..MAX_CODE_LENGTH).ok_or(Error::Truncated)?;
            let total: usize = counts.iter().map(|&count| usize::from(count)).sum();
            if total > 256 {
                return Err(Error::Corrupt);
            }
            let symbols = rest
                .get(MAX_CODE_LENGTH..MAX_CODE_LENGTH + total)
                .ok_or(Error::Truncated)?;
            let table = Huffman::new(counts, symbols)?;
            match class {
                0 => self.dc[index] = Some(table),
                1 => self.ac[index] = Some(table),
                _ => return Err(Error::Corrupt),
            }
            segment = &rest[MAX_CODE_LENGTH + total..];
        }
        Ok(())
    }

    fn frame(&mut self, segment: &[u8], progressive: bool, limits: &Limits) -> Result<(), Error> {
        if self.frame.is_some() {
            return Err(Error::Unsupported);
        }
        if byte(segment, 0)? != 8 {
            return Err(Error::Unsupported);
        }
        let height = u32::from(be16(segment, 1)?);
        let width = u32::from(be16(segment, 3)?);
        let count = usize::from(byte(segment, 5)?);
        if height == 0 || count == 2 || count > MAX_COMPONENTS {
            return Err(Error::Unsupported);
        }
        if width == 0 || count == 0 {
            return Err(Error::Corrupt);
        }
        crate::check_size(width, height, limits)?;
        let mut specified = [(0u8, 0usize, 0usize, 0usize); MAX_COMPONENTS];
        for (index, entry) in specified.iter_mut().take(count).enumerate() {
            let at = 6 + 3 * index;
            let sampling = byte(segment, at + 1)?;
            let (horizontal, vertical) = (usize::from(sampling >> 4), usize::from(sampling & 15));
            let table = usize::from(byte(segment, at + 2)?);
            let valid = (1..=MAX_SAMPLING).contains(&horizontal)
                && (1..=MAX_SAMPLING).contains(&vertical)
                && table < MAX_TABLES;
            if !valid {
                return Err(Error::Corrupt);
            }
            *entry = (byte(segment, at)?, horizontal, vertical, table);
        }
        let specified = &specified[..count];
        let horizontal_max = specified.iter().map(|entry| entry.1).max().unwrap_or(1);
        let vertical_max = specified.iter().map(|entry| entry.2).max().unwrap_or(1);
        let mcus_x = (width as usize).div_ceil(8 * horizontal_max);
        let mcus_y = (height as usize).div_ceil(8 * vertical_max);
        let mut components = Vec::new();
        components
            .try_reserve_exact(count)
            .map_err(|_| Error::OutOfMemory)?;
        for &(id, horizontal, vertical, quantization) in specified {
            let (blocks_x, blocks_y) = (mcus_x * horizontal, mcus_y * vertical);
            let blocks = blocks_x * blocks_y;
            let coefficients = match progressive {
                true => crate::buffer(blocks * 64, 0i16)?,
                false => Vec::new(),
            };
            components.push(Component {
                id,
                horizontal,
                vertical,
                quantization,
                blocks_x,
                blocks_y,
                width: (width as usize * horizontal).div_ceil(horizontal_max),
                height: (height as usize * vertical).div_ceil(vertical_max),
                plane: crate::buffer(blocks * 64, 0u8)?,
                coefficients,
                prediction: 0,
            });
        }
        self.frame = Some(Frame {
            width,
            height,
            progressive,
            components,
            horizontal_max,
            vertical_max,
            mcus_x,
            mcus_y,
        });
        Ok(())
    }

    fn scan(&mut self, bytes: &[u8], segment: &[u8], start: usize) -> Result<usize, Error> {
        let Decoder {
            quantization,
            dc,
            ac,
            restart_interval,
            frame,
            scans,
            ..
        } = self;
        let frame = frame.as_mut().ok_or(Error::Corrupt)?;
        let count = usize::from(byte(segment, 0)?);
        if count == 0 || count > frame.components.len() {
            return Err(Error::Corrupt);
        }
        let mut parts: [Option<ScanPart<'_>>; MAX_COMPONENTS] = [None, None, None, None];
        for (index, part) in parts.iter_mut().take(count).enumerate() {
            let id = byte(segment, 1 + 2 * index)?;
            let tables = byte(segment, 2 + 2 * index)?;
            let found = frame
                .components
                .iter()
                .position(|component| component.id == id)
                .ok_or(Error::Corrupt)?;
            let (dc_table, ac_table) = (usize::from(tables >> 4), usize::from(tables & 15));
            *part = Some(ScanPart {
                index: found,
                dc: dc.get(dc_table).and_then(Option::as_ref),
                ac: ac.get(ac_table).and_then(Option::as_ref),
            });
        }
        let at = 1 + 2 * count;
        let start_band = usize::from(byte(segment, at)?);
        let end_band = usize::from(byte(segment, at + 1)?);
        let approximation = byte(segment, at + 2)?;
        let (high, low) = (approximation >> 4, approximation & 15);
        if high > MAX_SUCCESSIVE_BIT || low > MAX_SUCCESSIVE_BIT {
            return Err(Error::Corrupt);
        }
        let pass = match (frame.progressive, start_band, high) {
            (false, _, _) if start_band == 0 && end_band == 63 && approximation == 0 => {
                Pass::Sequential
            }
            (false, _, _) => return Err(Error::Corrupt),
            (true, 0, 0) if end_band == 0 => Pass::DcFirst,
            (true, 0, _) if end_band == 0 => Pass::DcRefine,
            (true, 0, _) => return Err(Error::Corrupt),
            (true, _, _) if count != 1 || end_band < start_band || end_band > 63 => {
                return Err(Error::Corrupt);
            }
            (true, _, 0) => Pass::AcFirst,
            (true, _, _) => Pass::AcRefine,
        };
        let parts = &parts[..count];
        let spectral = Spectral {
            start: start_band,
            end: end_band,
            low: u32::from(low),
        };
        for part in parts.iter().flatten() {
            frame.components[part.index].prediction = 0;
        }
        let single = count == 1;
        let (units_x, units_y) = match (single, &parts[0]) {
            (true, Some(part)) => {
                let component = &frame.components[part.index];
                (component.width.div_ceil(8), component.height.div_ceil(8))
            }
            _ => (frame.mcus_x, frame.mcus_y),
        };
        let mut bits = Bits::new(bytes, start);
        let mut progress = Progress {
            pass,
            spectral,
            end_of_bands: 0,
            block: [0; 64],
        };
        let mut left = *restart_interval;
        for unit_y in 0..units_y {
            for unit_x in 0..units_x {
                if *restart_interval > 0 {
                    if left == 0 {
                        bits.restart();
                        progress.end_of_bands = 0;
                        for part in parts.iter().flatten() {
                            frame.components[part.index].prediction = 0;
                        }
                        left = *restart_interval;
                    }
                    left -= 1;
                }
                for part in parts.iter().flatten() {
                    let component = &mut frame.components[part.index];
                    let steps = &quantization[component.quantization];
                    let (across, down) = match single {
                        true => (1, 1),
                        false => (component.horizontal, component.vertical),
                    };
                    for down_at in 0..down {
                        for across_at in 0..across {
                            let place = (unit_x * across + across_at, unit_y * down + down_at);
                            decode_block(&mut bits, part, component, steps, &mut progress, place)?;
                        }
                    }
                }
            }
        }
        if bits.padded > 0 {
            return Err(Error::Truncated);
        }
        *scans += 1;
        Ok(bits.end())
    }

    fn colour(&self, frame: &Frame) -> Colour {
        match frame.components.len() {
            1 => Colour::Grey,
            3 => {
                let ids = [
                    frame.components[0].id,
                    frame.components[1].id,
                    frame.components[2].id,
                ];
                match self.adobe {
                    Some(ADOBE_RGB) => Colour::Rgb,
                    Some(_) => Colour::YCbCr,
                    None if !self.jfif && ids == *b"RGB" => Colour::Rgb,
                    None => Colour::YCbCr,
                }
            }
            _ => match self.adobe {
                Some(ADOBE_YCCK) => Colour::Ycck,
                _ => Colour::Cmyk,
            },
        }
    }

    fn finish(mut self, limits: &Limits) -> Result<Image, Error> {
        let mut frame = self.frame.take().ok_or(Error::Corrupt)?;
        if self.scans == 0 {
            return Err(Error::Truncated);
        }
        if frame.progressive {
            let mut block = [0i32; 64];
            for component in &mut frame.components {
                let steps = &self.quantization[component.quantization];
                let stride = component.stride();
                for block_y in 0..component.blocks_y {
                    for block_x in 0..component.blocks_x {
                        let start = (block_y * component.blocks_x + block_x) * 64;
                        let stored = &component.coefficients[start..start + 64];
                        for ((value, &coefficient), &step) in
                            block.iter_mut().zip(stored).zip(steps.iter())
                        {
                            *value = dequantize(i32::from(coefficient), step);
                        }
                        let at = component.block_at(block_x, block_y);
                        idct(&block, &mut component.plane, at, stride);
                    }
                }
            }
        }
        let colour = self.colour(&frame);
        convert(&frame, colour, limits)
    }
}

fn upsample_row(
    component: &Component,
    frame: &Frame,
    y: usize,
    scratch: &mut [u16],
    out: &mut [u8],
) {
    let stride = component.stride();
    let last_row = component.height - 1;
    let last_column = component.width - 1;
    let row = |index: usize| &component.plane[index * stride..index * stride + stride];
    let vertical_factor = frame.vertical_max / component.vertical;
    let exact = frame.vertical_max % component.vertical == 0;
    match (vertical_factor, exact) {
        (1, true) => {
            for (cell, &value) in scratch.iter_mut().zip(row(y.min(last_row))) {
                *cell = u16::from(value) * 4;
            }
        }
        (2, true) => {
            let near = (y / 2).min(last_row);
            let far = match y % 2 {
                0 => near.saturating_sub(1),
                _ => (near + 1).min(last_row),
            };
            for ((cell, &close), &other) in scratch.iter_mut().zip(row(near)).zip(row(far)) {
                *cell = u16::from(close) * 3 + u16::from(other);
            }
        }
        _ => {
            let source = (y * component.vertical / frame.vertical_max).min(last_row);
            for (cell, &value) in scratch.iter_mut().zip(row(source)) {
                *cell = u16::from(value) * 4;
            }
        }
    }
    let horizontal_factor = frame.horizontal_max / component.horizontal;
    let exact = frame.horizontal_max % component.horizontal == 0;
    match (horizontal_factor, exact) {
        (1, true) => {
            for (value, &cell) in out.iter_mut().zip(scratch.iter()) {
                *value = ((cell + 2) >> 2) as u8;
            }
        }
        (2, true) => {
            for (x, value) in out.iter_mut().enumerate() {
                let near = (x / 2).min(last_column);
                let far = match x % 2 {
                    0 => near.saturating_sub(1),
                    _ => (near + 1).min(last_column),
                };
                *value = ((scratch[near] * 3 + scratch[far] + 8) >> 4) as u8;
            }
        }
        _ => {
            for (x, value) in out.iter_mut().enumerate() {
                let source = (x * component.horizontal / frame.horizontal_max).min(last_column);
                *value = ((scratch[source] + 2) >> 2) as u8;
            }
        }
    }
}

fn ycc(luma: u8, blue: u8, red: u8) -> (u8, u8, u8) {
    let (luma, blue, red) = (i32::from(luma), i32::from(blue) - 128, i32::from(red) - 128);
    let channel = |value: i32| value.clamp(0, 255) as u8;
    (
        channel(luma + ((CR_TO_RED * red + FIXED_ONE_HALF) >> 16)),
        channel(luma - ((CB_TO_GREEN * blue + CR_TO_GREEN * red - FIXED_ONE_HALF) >> 16)),
        channel(luma + ((CB_TO_BLUE * blue + FIXED_ONE_HALF) >> 16)),
    )
}

fn scaled(value: u8, by: u8) -> u8 {
    ((u32::from(value) * u32::from(by) + 127) / 255) as u8
}

fn convert(frame: &Frame, colour: Colour, limits: &Limits) -> Result<Image, Error> {
    let mut image = crate::canvas(frame.width, frame.height, limits)?;
    let width = frame.width as usize;
    let widest = frame
        .components
        .iter()
        .map(Component::stride)
        .max()
        .unwrap_or(0);
    let mut scratch = crate::buffer(widest, 0u16)?;
    let mut rows = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for row in rows.iter_mut().take(frame.components.len()) {
        *row = crate::buffer(width, 0u8)?;
    }
    for (y, target) in image.pixels.chunks_exact_mut(width).enumerate() {
        for (component, row) in frame.components.iter().zip(rows.iter_mut()) {
            upsample_row(component, frame, y, &mut scratch, row);
        }
        for (x, pixel) in target.iter_mut().enumerate() {
            let first = rows[0][x];
            *pixel = match colour {
                Colour::Grey => crate::rgb(first, first, first),
                Colour::Rgb => crate::rgb(first, rows[1][x], rows[2][x]),
                Colour::YCbCr => {
                    let (red, green, blue) = ycc(first, rows[1][x], rows[2][x]);
                    crate::rgb(red, green, blue)
                }
                Colour::Cmyk => {
                    let key = rows[3][x];
                    crate::rgb(
                        scaled(first, key),
                        scaled(rows[1][x], key),
                        scaled(rows[2][x], key),
                    )
                }
                Colour::Ycck => {
                    let key = rows[3][x];
                    let (red, green, blue) = ycc(first, rows[1][x], rows[2][x]);
                    crate::rgb(scaled(red, key), scaled(green, key), scaled(blue, key))
                }
            };
        }
    }
    Ok(image)
}

pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, Error> {
    if !bytes.starts_with(&[0xff, SOI]) {
        return Err(Error::Corrupt);
    }
    let mut decoder = Decoder::new();
    let mut at = 2;
    loop {
        let Some((marker, after)) = next_marker(bytes, at) else {
            if decoder.scans > 0 {
                break;
            }
            return Err(Error::Truncated);
        };
        at = after;
        match marker {
            EOI => break,
            RESTART_FIRST..=RESTART_LAST | TEMPORARY => continue,
            _ => {}
        }
        let length = usize::from(be16(bytes, at)?);
        if length < 2 {
            return Err(Error::Corrupt);
        }
        let segment = bytes.get(at + 2..at + length).ok_or(Error::Truncated)?;
        at += length;
        match marker {
            SOF_BASELINE | SOF_EXTENDED => decoder.frame(segment, false, limits)?,
            SOF_PROGRESSIVE => decoder.frame(segment, true, limits)?,
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => return Err(Error::Unsupported),
            DHT => decoder.huffman_tables(segment)?,
            DQT => decoder.quantization_tables(segment)?,
            DRI => decoder.restart_interval = usize::from(be16(segment, 0)?),
            SOS => at = decoder.scan(bytes, segment, at)?,
            APP_JFIF => decoder.jfif |= segment.starts_with(b"JFIF\0"),
            APP_ADOBE if segment.starts_with(b"Adobe") => {
                decoder.adobe = segment.get(ADOBE_TRANSFORM_AT).copied();
            }
            _ => {}
        }
    }
    decoder.finish(limits)
}
