use crate::Error;
use alloc::vec::Vec;

const MAX_CODES: usize = 4096;
const MAX_WIDTH: u32 = 12;
const NO_PREFIX: u16 = u16::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavour {
    Gif,
    Tiff,
}

impl Flavour {
    fn early_change(self) -> u16 {
        match self {
            Flavour::Gif => 0,
            Flavour::Tiff => 1,
        }
    }
}

pub(crate) struct Table {
    prefix: Vec<u16>,
    suffix: Vec<u8>,
    first: Vec<u8>,
    length: Vec<u16>,
}

impl Table {
    pub(crate) fn new() -> Result<Self, Error> {
        Ok(Self {
            prefix: crate::buffer(MAX_CODES, NO_PREFIX)?,
            suffix: crate::buffer(MAX_CODES, 0)?,
            first: crate::buffer(MAX_CODES, 0)?,
            length: crate::buffer(MAX_CODES, 0)?,
        })
    }

    fn reset(&mut self, literals: u16) {
        for code in 0..literals {
            let index = usize::from(code);
            self.prefix[index] = NO_PREFIX;
            self.suffix[index] = code as u8;
            self.first[index] = code as u8;
            self.length[index] = 1;
        }
    }

    fn add(&mut self, code: u16, prefix: u16, last: u8) {
        let (index, from) = (usize::from(code), usize::from(prefix));
        self.prefix[index] = prefix;
        self.suffix[index] = last;
        self.first[index] = self.first[from];
        self.length[index] = self.length[from].saturating_add(1);
    }

    fn emit(&self, code: u16, out: &mut [u8], written: usize) -> usize {
        let length = usize::from(self.length[usize::from(code)]);
        let mut at = written + length;
        let mut walk = usize::from(code);
        loop {
            at -= 1;
            if let Some(slot) = out.get_mut(at) {
                *slot = self.suffix[walk];
            }
            let prefix = self.prefix[walk];
            if prefix == NO_PREFIX || at == written {
                break;
            }
            walk = usize::from(prefix);
        }
        (written + length).min(out.len())
    }
}

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    buffer: u32,
    count: u32,
    flavour: Flavour,
}

impl Bits<'_> {
    fn read(&mut self, width: u32) -> Option<u16> {
        while self.count < width {
            let byte = u32::from(*self.data.get(self.at)?);
            self.at += 1;
            match self.flavour {
                Flavour::Gif => self.buffer |= byte << self.count,
                Flavour::Tiff => self.buffer = self.buffer << 8 | byte,
            }
            self.count += 8;
        }
        let mask = (1u32 << width) - 1;
        let code = match self.flavour {
            Flavour::Gif => {
                let code = self.buffer & mask;
                self.buffer >>= width;
                code
            }
            Flavour::Tiff => self.buffer >> (self.count - width) & mask,
        };
        self.count -= width;
        Some(code as u16)
    }
}

pub(crate) fn decode(
    data: &[u8],
    minimum: u32,
    flavour: Flavour,
    table: &mut Table,
    out: &mut [u8],
) -> Result<usize, Error> {
    if !(1..=8).contains(&minimum) {
        return Err(Error::Corrupt);
    }
    let clear = 1u16 << minimum;
    let end = clear + 1;
    let early = flavour.early_change();
    table.reset(clear);
    let mut bits = Bits {
        data,
        at: 0,
        buffer: 0,
        count: 0,
        flavour,
    };
    let mut next = end + 1;
    let mut width = minimum + 1;
    let mut previous: Option<u16> = None;
    let mut written = 0;
    while written < out.len() {
        let Some(code) = bits.read(width) else {
            break;
        };
        if code == clear {
            next = end + 1;
            width = minimum + 1;
            previous = None;
            continue;
        }
        if code == end {
            break;
        }
        let Some(prior) = previous else {
            if code >= clear {
                return Err(Error::Corrupt);
            }
            written = table.emit(code, out, written);
            previous = Some(code);
            continue;
        };
        let room = usize::from(next) < MAX_CODES;
        if code < next {
            if room {
                let first = table.first[usize::from(code)];
                table.add(next, prior, first);
                next += 1;
            }
        } else if code == next && room {
            let first = table.first[usize::from(prior)];
            table.add(next, prior, first);
            next += 1;
        } else {
            return Err(Error::Corrupt);
        }
        written = table.emit(code, out, written);
        if width < MAX_WIDTH && u32::from(next + early) == 1 << width {
            width += 1;
        }
        previous = Some(code);
    }
    Ok(written)
}
