use crate::inflate::{InflateError, Sink, SliceSource, Window, inflate, inflate_into};
use crate::{DISTANCE_BASES, DISTANCE_EXTRA_BITS, LENGTH_BASES, LENGTH_EXTRA_BITS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Token {
    Literal(u8),
    Match(usize, usize),
}

#[derive(Default)]
pub(crate) struct BitWriter {
    bytes: Vec<u8>,
    bits: u64,
    count: u32,
}

impl BitWriter {
    pub(crate) fn put(&mut self, value: u32, count: u32) {
        self.bits |= u64::from(value) << self.count;
        self.count += count;
        while self.count >= 8 {
            self.bytes.push(self.bits as u8);
            self.bits >>= 8;
            self.count -= 8;
        }
    }

    fn put_code(&mut self, code: u32, length: u8) {
        let length = u32::from(length);
        self.put(code.reverse_bits() >> (32 - length), length);
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push(self.bits as u8);
        }
        self.bytes
    }
}

fn complete_lengths(used: &[usize], size: usize) -> Vec<u8> {
    let mut lengths = vec![0u8; size];
    let count = used.len();
    if count == 1 {
        lengths[used[0]] = 1;
        return lengths;
    }
    let bits = usize::BITS - (count - 1).leading_zeros();
    let shorter = (1usize << bits) - count;
    for (rank, &symbol) in used.iter().enumerate() {
        lengths[symbol] = match rank < shorter {
            true => bits as u8 - 1,
            false => bits as u8,
        };
    }
    lengths
}

fn canonical(lengths: &[u8]) -> Vec<u32> {
    let mut counts = [0u32; 16];
    for &length in lengths.iter().filter(|&&length| length > 0) {
        counts[usize::from(length)] += 1;
    }
    let mut next = [0u32; 16];
    let mut code = 0;
    for bits in 1..16 {
        code = (code + counts[bits - 1]) << 1;
        next[bits] = code;
    }
    lengths
        .iter()
        .map(|&length| match length {
            0 => 0,
            _ => {
                let code = next[usize::from(length)];
                next[usize::from(length)] += 1;
                code
            }
        })
        .collect()
}

fn length_symbol(length: usize) -> (usize, u32, u32) {
    let code = LENGTH_BASES
        .iter()
        .rposition(|&base| usize::from(base) <= length)
        .unwrap();
    let extra = (length - usize::from(LENGTH_BASES[code])) as u32;
    (257 + code, extra, u32::from(LENGTH_EXTRA_BITS[code]))
}

fn distance_symbol(distance: usize) -> (usize, u32, u32) {
    let code = DISTANCE_BASES
        .iter()
        .rposition(|&base| usize::from(base) <= distance)
        .unwrap();
    let extra = (distance - usize::from(DISTANCE_BASES[code])) as u32;
    (code, extra, u32::from(DISTANCE_EXTRA_BITS[code]))
}

fn run_lengths(lengths: &[u8]) -> Vec<(usize, u32, u32)> {
    let mut items = Vec::new();
    let mut at = 0;
    while at < lengths.len() {
        let value = lengths[at];
        let run = lengths[at..]
            .iter()
            .take_while(|&&length| length == value)
            .count();
        let mut left = run;
        if value == 0 {
            while left >= 11 {
                let take = left.min(138);
                items.push((18, (take - 11) as u32, 7));
                left -= take;
            }
            if left >= 3 {
                items.push((17, (left - 3) as u32, 3));
                left = 0;
            }
        } else {
            items.push((usize::from(value), 0, 0));
            left -= 1;
            while left >= 3 {
                let take = left.min(6);
                items.push((16, (take - 3) as u32, 2));
                left -= take;
            }
        }
        items.extend((0..left).map(|_| (usize::from(value), 0, 0)));
        at += run;
    }
    items
}

pub(crate) fn dynamic_block(writer: &mut BitWriter, tokens: &[Token], last: bool) {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut literal_used = vec![false; 286];
    let mut distance_used = vec![false; 30];
    literal_used[256] = true;
    for &token in tokens {
        match token {
            Token::Literal(byte) => literal_used[usize::from(byte)] = true,
            Token::Match(length, distance) => {
                literal_used[length_symbol(length).0] = true;
                distance_used[distance_symbol(distance).0] = true;
            }
        }
    }
    if !distance_used.contains(&true) {
        distance_used[0] = true;
    }
    let used = |flags: &[bool]| -> Vec<usize> {
        flags
            .iter()
            .enumerate()
            .filter(|(_, used)| **used)
            .map(|(symbol, _)| symbol)
            .collect()
    };
    let literal_lengths = complete_lengths(&used(&literal_used), 286);
    let distance_lengths = complete_lengths(&used(&distance_used), 30);
    let literal_count = 1 + literal_lengths.iter().rposition(|&l| l > 0).unwrap();
    let distance_count = 1 + distance_lengths.iter().rposition(|&l| l > 0).unwrap();
    let mut all = literal_lengths[..literal_count].to_vec();
    all.extend_from_slice(&distance_lengths[..distance_count]);
    let items = run_lengths(&all);
    let mut code_length_used = vec![false; 19];
    for &(symbol, _, _) in &items {
        code_length_used[symbol] = true;
    }
    let code_lengths = complete_lengths(&used(&code_length_used), 19);
    let code_length_count = (4..=19)
        .find(|&count| ORDER[count..].iter().all(|&symbol| code_lengths[symbol] == 0))
        .unwrap();
    let literal_codes = canonical(&literal_lengths);
    let distance_codes = canonical(&distance_lengths);
    let code_length_codes = canonical(&code_lengths);
    writer.put(u32::from(last), 1);
    writer.put(2, 2);
    writer.put((literal_count - 257) as u32, 5);
    writer.put((distance_count - 1) as u32, 5);
    writer.put((code_length_count - 4) as u32, 4);
    for &symbol in &ORDER[..code_length_count] {
        writer.put(u32::from(code_lengths[symbol]), 3);
    }
    for &(symbol, extra, extra_bits) in &items {
        writer.put_code(code_length_codes[symbol], code_lengths[symbol]);
        writer.put(extra, extra_bits);
    }
    for &token in tokens {
        match token {
            Token::Literal(byte) => {
                let symbol = usize::from(byte);
                writer.put_code(literal_codes[symbol], literal_lengths[symbol]);
            }
            Token::Match(length, distance) => {
                let (symbol, extra, extra_bits) = length_symbol(length);
                writer.put_code(literal_codes[symbol], literal_lengths[symbol]);
                writer.put(extra, extra_bits);
                let (symbol, extra, extra_bits) = distance_symbol(distance);
                writer.put_code(distance_codes[symbol], distance_lengths[symbol]);
                writer.put(extra, extra_bits);
            }
        }
    }
    writer.put_code(literal_codes[256], literal_lengths[256]);
}

pub(crate) fn stored_block(writer: &mut BitWriter, data: &[u8], last: bool) {
    writer.put(u32::from(last), 1);
    writer.put(0, 2);
    let mut bytes = std::mem::take(writer).finish();
    let length = data.len() as u16;
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&(!length).to_le_bytes());
    bytes.extend_from_slice(data);
    writer.bytes = bytes;
}

pub(crate) fn dynamic_stream(tokens: &[Token]) -> Vec<u8> {
    let mut writer = BitWriter::default();
    dynamic_block(&mut writer, tokens, true);
    writer.finish()
}

pub(crate) fn literals(data: &[u8]) -> Vec<Token> {
    data.iter().map(|&byte| Token::Literal(byte)).collect()
}

fn noise(count: usize, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

fn inflated(stream: &[u8], room: usize) -> Result<Vec<u8>, InflateError> {
    let mut out = vec![0u8; room];
    let length = inflate_into(&mut SliceSource::new(stream), &mut out)?;
    out.truncate(length);
    Ok(out)
}

#[derive(Default)]
struct Collect {
    bytes: Vec<u8>,
    pieces: usize,
    largest: usize,
    limit: Option<usize>,
}

impl Sink for Collect {
    fn write(&mut self, bytes: &[u8]) -> bool {
        self.pieces += 1;
        self.largest = self.largest.max(bytes.len());
        self.bytes.extend_from_slice(bytes);
        self.limit.is_none_or(|limit| self.bytes.len() <= limit)
    }
}

#[test]
fn a_dynamic_block_with_literals_and_a_match_inflates() {
    let mut tokens = literals(b"hello ");
    tokens.push(Token::Match(11, 6));
    tokens.extend(literals(b"!"));
    let stream = dynamic_stream(&tokens);
    assert_eq!(
        inflated(&stream, 64).unwrap(),
        b"hello hello hello!".to_vec()
    );
}

#[test]
fn every_byte_value_and_long_zero_runs_in_the_lengths_inflate() {
    let data: Vec<u8> = (0..=255u8).chain(b"tail".iter().copied()).collect();
    let stream = dynamic_stream(&literals(&data));
    assert_eq!(inflated(&stream, 300).unwrap(), data);
    let sparse = b"aaaaz".to_vec();
    let stream = dynamic_stream(&literals(&sparse));
    assert_eq!(inflated(&stream, 8).unwrap(), sparse);
}

#[test]
fn stored_and_dynamic_blocks_follow_each_other() {
    let mut writer = BitWriter::default();
    stored_block(&mut writer, b"stored ", false);
    dynamic_block(&mut writer, &literals(b"then dynamic"), false);
    stored_block(&mut writer, b"", true);
    let stream = writer.finish();
    assert_eq!(
        inflated(&stream, 64).unwrap(),
        b"stored then dynamic".to_vec()
    );
}

#[test]
fn the_encoder_s_fixed_blocks_inflate() {
    let mut writer = BitWriter::default();
    writer.put(1, 1);
    writer.put(1, 2);
    for &byte in b"fix" {
        writer.put_code(0x30 + u32::from(byte), 8);
    }
    writer.put_code(0, 7);
    assert_eq!(inflated(&writer.finish(), 8).unwrap(), b"fix".to_vec());
}

#[test]
fn the_window_reaches_back_32_kib_across_its_flushes() {
    let first = noise(32 * 1024, 5);
    let mut tokens = literals(&first);
    for _ in 0..127 {
        tokens.push(Token::Match(258, 32 * 1024));
    }
    tokens.extend(literals(&first[32 * 1024 - 2..]));
    let stream = dynamic_stream(&tokens);
    let mut window = Box::new(Window::new());
    let mut sink = Collect::default();
    let written = inflate(&mut SliceSource::new(&stream), &mut window, &mut sink).unwrap();
    let mut expected = first.clone();
    expected.extend_from_slice(&first);
    assert_eq!(written, expected.len() as u64);
    assert_eq!(sink.bytes, expected);
    assert_eq!(sink.largest, 32 * 1024);
    assert_eq!(sink.pieces, 2);
    assert_eq!(inflated(&stream, 64 * 1024).unwrap(), expected);
}

#[test]
fn malformed_streams_are_errors_not_panics() {
    assert_eq!(inflated(&[0b111], 8), Err(InflateError::Corrupt));
    assert_eq!(inflated(&[1, 3, 0, 0, 0], 8), Err(InflateError::Corrupt));
    assert_eq!(inflated(&[1, 3, 0, 0xfc, 0xff], 8), Err(InflateError::Truncated));
    assert_eq!(inflated(&[], 8), Err(InflateError::Truncated));
    let stream = dynamic_stream(&literals(b"too long for the room"));
    assert_eq!(inflated(&stream, 4), Err(InflateError::TooLong));
    let mut tokens = literals(b"ab");
    tokens.push(Token::Match(3, 5));
    assert_eq!(
        inflated(&dynamic_stream(&tokens), 64),
        Err(InflateError::Corrupt)
    );
    let mut window = Box::new(Window::new());
    let mut refusing = Collect {
        limit: Some(0),
        ..Collect::default()
    };
    assert_eq!(
        inflate(
            &mut SliceSource::new(&dynamic_stream(&literals(b"x"))),
            &mut window,
            &mut refusing
        ),
        Err(InflateError::Stopped)
    );
    let mut over = BitWriter::default();
    over.put(1, 1);
    over.put(2, 2);
    over.put(0, 5);
    over.put(0, 5);
    over.put(0, 4);
    for _ in 0..4 {
        over.put(1, 3);
    }
    assert_eq!(inflated(&over.finish(), 8), Err(InflateError::Corrupt));
    let stream = dynamic_stream(&literals(&noise(500, 9)));
    for cut in 0..stream.len() {
        assert!(inflated(&stream[..cut], 600).is_err(), "cut at {cut}");
    }
    let mut state = 0x2545_f491u32;
    for _ in 0..2000 {
        let garbage: Vec<u8> = (0..40)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let _ = inflated(&garbage, 256);
    }
}
