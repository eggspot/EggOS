//! Certificates in PEM (RFC 7468): `-----BEGIN CERTIFICATE-----`, base64, `-----END
//! CERTIFICATE-----`, decoded as the text arrives in pieces of any size, so a file of roots is
//! read without holding it whole. Text outside the blocks is skipped.

const BEGIN: &[u8] = b"-----BEGIN CERTIFICATE-----";
const END: &[u8] = b"-----END CERTIFICATE-----";
const LINE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Outside,
    Inside,
    Marker,
}

/// Decodes the certificates in PEM text fed piece by piece.
#[derive(Debug)]
pub struct Certificates<'b> {
    der: &'b mut [u8],
    length: usize,
    overflowed: bool,
    place: Place,
    line: [u8; LINE_BYTES],
    line_length: usize,
    bits: u32,
    sextets: u32,
}

fn sextet(byte: u8) -> Option<u32> {
    let value = match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

impl<'b> Certificates<'b> {
    /// A decoder that decodes each certificate into `der`; a certificate longer than `der` is
    /// skipped.
    pub fn new(der: &'b mut [u8]) -> Self {
        Self {
            der,
            length: 0,
            overflowed: false,
            place: Place::Outside,
            line: [0; LINE_BYTES],
            line_length: 0,
            bits: 0,
            sextets: 0,
        }
    }

    fn put(&mut self, byte: u8) {
        match self.der.get_mut(self.length) {
            Some(slot) => {
                *slot = byte;
                self.length += 1;
            }
            None => self.overflowed = true,
        }
    }

    fn take(&mut self, value: u32) {
        self.bits = (self.bits << 6) | value;
        self.sextets += 1;
        if self.sextets == 4 {
            let bits = self.bits;
            self.put((bits >> 16) as u8);
            self.put((bits >> 8) as u8);
            self.put(bits as u8);
            self.bits = 0;
            self.sextets = 0;
        }
    }

    fn flush(&mut self) {
        let bits = self.bits;
        match self.sextets {
            2 => self.put((bits >> 4) as u8),
            3 => {
                self.put((bits >> 10) as u8);
                self.put((bits >> 2) as u8);
            }
            _ => {}
        }
        self.bits = 0;
        self.sextets = 0;
    }

    fn start_block(&mut self) {
        self.place = Place::Inside;
        self.length = 0;
        self.overflowed = false;
        self.bits = 0;
        self.sextets = 0;
    }

    /// Feeds the next piece of text, calling `visit` with each certificate's DER as its block
    /// ends, until `visit` returns true; returns whether it did.
    pub fn feed(&mut self, text: &[u8], visit: &mut dyn FnMut(&[u8]) -> bool) -> bool {
        for &byte in text {
            match self.place {
                Place::Inside => match byte {
                    b'-' => {
                        self.place = Place::Marker;
                        self.line[0] = byte;
                        self.line_length = 1;
                    }
                    b'=' | b' ' | b'\t' | b'\r' | b'\n' => {}
                    _ => match sextet(byte) {
                        Some(value) => self.take(value),
                        None => self.place = Place::Outside,
                    },
                },
                Place::Outside | Place::Marker => {
                    if byte != b'\n' {
                        if let Some(slot) = self.line.get_mut(self.line_length) {
                            *slot = byte;
                        }
                        self.line_length += 1;
                        continue;
                    }
                    let length = self.line_length.min(LINE_BYTES);
                    let line = trim_end(&self.line[..length]);
                    let place = self.place;
                    self.line_length = 0;
                    if line == BEGIN {
                        self.start_block();
                    } else if place == Place::Marker {
                        self.place = Place::Outside;
                        if line == END {
                            self.flush();
                            if !self.overflowed
                                && self.length > 0
                                && visit(&self.der[..self.length])
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Ends the text: a block whose end line has no line break after it still counts.
    pub fn finish(&mut self, visit: &mut dyn FnMut(&[u8]) -> bool) -> bool {
        self.feed(b"\n", visit)
    }
}

fn trim_end(mut line: &[u8]) -> &[u8] {
    while let [rest @ .., b' ' | b'\t' | b'\r'] = line {
        line = rest;
    }
    line
}
