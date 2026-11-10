//! A walk of the ACPI namespace's static part (ACPI 6.5, chapter 20, AML): the devices a DSDT
//! declares, with the `_HID`, `_CID`, `_UID`, `_ADR` and `_CRS` they name as data (D252). Methods
//! are not run: a value a method computes is not seen, and a term this walk does not know ends
//! its scope's walk there, so nothing past it in that scope is misread.

/// The deepest namespace path kept, in name segments.
pub const MAX_DEPTH: usize = 8;
const ZERO_OP: u8 = 0x00;
const ONE_OP: u8 = 0x01;
const ALIAS_OP: u8 = 0x06;
const NAME_OP: u8 = 0x08;
const BYTE_PREFIX: u8 = 0x0a;
const WORD_PREFIX: u8 = 0x0b;
const DWORD_PREFIX: u8 = 0x0c;
const STRING_PREFIX: u8 = 0x0d;
const QWORD_PREFIX: u8 = 0x0e;
const SCOPE_OP: u8 = 0x10;
const BUFFER_OP: u8 = 0x11;
const PACKAGE_OP: u8 = 0x12;
const VAR_PACKAGE_OP: u8 = 0x13;
const METHOD_OP: u8 = 0x14;
const EXTERNAL_OP: u8 = 0x15;
const DUAL_NAME_PREFIX: u8 = 0x2e;
const MULTI_NAME_PREFIX: u8 = 0x2f;
const EXTENDED_PREFIX: u8 = 0x5b;
const ROOT_CHAR: u8 = b'\\';
const PARENT_PREFIX: u8 = b'^';
const IF_OP: u8 = 0xa0;
const ELSE_OP: u8 = 0xa1;
const WHILE_OP: u8 = 0xa2;
const ONES_OP: u8 = 0xff;
const MUTEX_OP: u8 = 0x01;
const EVENT_OP: u8 = 0x02;
const REGION_OP: u8 = 0x80;
const FIELD_OP: u8 = 0x81;
const DEVICE_OP: u8 = 0x82;
const PROCESSOR_OP: u8 = 0x83;
const POWER_RESOURCE_OP: u8 = 0x84;
const THERMAL_ZONE_OP: u8 = 0x85;
const INDEX_FIELD_OP: u8 = 0x86;
const BANK_FIELD_OP: u8 = 0x87;

/// A namespace path: up to [`MAX_DEPTH`] four-character segments from the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Path {
    segments: [[u8; 4]; MAX_DEPTH],
    depth: usize,
}

impl Path {
    fn with(&self, segment: [u8; 4]) -> Option<Self> {
        let mut path = *self;
        *path.segments.get_mut(path.depth)? = segment;
        path.depth += 1;
        Some(path)
    }

    /// Writes `\_SB.PCI0.LPCB` into `out`: its length.
    pub fn write(&self, out: &mut [u8]) -> usize {
        let mut length = 0;
        let mut put = |byte: u8| {
            if let Some(slot) = out.get_mut(length) {
                *slot = byte;
                length += 1;
            }
        };
        put(b'\\');
        for (index, segment) in self.segments[..self.depth].iter().enumerate() {
            if index > 0 {
                put(b'.');
            }
            let trimmed = segment.iter().rposition(|&byte| byte != b'_').map_or(1, |last| last + 1);
            for &byte in &segment[..trimmed.max(1)] {
                put(byte);
            }
        }
        length
    }
}

/// A device identifier: a string `_HID`/`_CID`, or a compressed EISA id turned into its seven
/// characters (`PNP0303`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Id {
    bytes: [u8; 16],
    length: usize,
}

impl Id {
    fn text(text: &[u8]) -> Self {
        let mut id = Self::default();
        id.length = text.len().min(id.bytes.len());
        id.bytes[..id.length].copy_from_slice(&text[..id.length]);
        id
    }

    /// The id of a compressed EISA id (the DWord `EISAID()` makes).
    pub fn eisa(value: u32) -> Self {
        let swapped = value.swap_bytes();
        let letter = |shift: u32| b'@' + ((swapped >> shift) & 0x1f) as u8;
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let product = swapped & 0xffff;
        let text = [
            letter(26),
            letter(21),
            letter(16),
            HEX[(product >> 12) as usize & 0xf],
            HEX[(product >> 8) as usize & 0xf],
            HEX[(product >> 4) as usize & 0xf],
            HEX[product as usize & 0xf],
        ];
        Self::text(&text)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

/// A device the namespace declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Device<'a> {
    pub path: Path,
    pub hid: Option<Id>,
    pub cid: Option<Id>,
    pub uid: Option<u64>,
    pub address: Option<u64>,
    /// `_CRS`'s resource template, when the namespace names it as a buffer.
    pub resources: Option<&'a [u8]>,
}

/// A value a `Name` gives, as far as the walk reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Value<'a> {
    Integer(u64),
    String(&'a [u8]),
    Buffer(&'a [u8]),
    Other,
}

struct Reader<'a> {
    aml: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.aml.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    fn peek(&self) -> Option<u8> {
        self.aml.get(self.at).copied()
    }

    fn bytes(&mut self, count: usize) -> Option<&'a [u8]> {
        let bytes = self.aml.get(self.at..self.at.checked_add(count)?)?;
        self.at += count;
        Some(bytes)
    }

    fn integer(&mut self, bytes: usize) -> Option<u64> {
        let raw = self.bytes(bytes)?;
        Some(raw.iter().rev().fold(0u64, |value, &byte| (value << 8) | u64::from(byte)))
    }

    fn package_end(&mut self) -> Option<usize> {
        let start = self.at;
        let lead = self.byte()?;
        let follow = usize::from(lead >> 6);
        let length = if follow == 0 {
            usize::from(lead & 0x3f)
        } else {
            let mut length = usize::from(lead & 0x0f);
            for index in 0..follow {
                length |= usize::from(self.byte()?) << (4 + 8 * index);
            }
            length
        };
        let end = start.checked_add(length)?;
        (end <= self.aml.len() && end >= self.at).then_some(end)
    }

    fn name_segment(&mut self) -> Option<[u8; 4]> {
        self.bytes(4)?.try_into().ok()
    }

    fn name(&mut self, scope: &Path) -> Option<Path> {
        let mut path = *scope;
        match self.peek()? {
            ROOT_CHAR => {
                self.at += 1;
                path = Path::default();
            }
            PARENT_PREFIX => {
                while self.peek()? == PARENT_PREFIX {
                    self.at += 1;
                    path.depth = path.depth.saturating_sub(1);
                }
            }
            _ => {}
        }
        let count = match self.peek()? {
            ZERO_OP => {
                self.at += 1;
                0
            }
            DUAL_NAME_PREFIX => {
                self.at += 1;
                2
            }
            MULTI_NAME_PREFIX => {
                self.at += 1;
                usize::from(self.byte()?)
            }
            _ => 1,
        };
        for _ in 0..count {
            path = path.with(self.name_segment()?)?;
        }
        Some(path)
    }

    fn value(&mut self) -> Option<Value<'a>> {
        Some(match self.byte()? {
            ZERO_OP => Value::Integer(0),
            ONE_OP => Value::Integer(1),
            ONES_OP => Value::Integer(u64::MAX),
            BYTE_PREFIX => Value::Integer(self.integer(1)?),
            WORD_PREFIX => Value::Integer(self.integer(2)?),
            DWORD_PREFIX => Value::Integer(self.integer(4)?),
            QWORD_PREFIX => Value::Integer(self.integer(8)?),
            STRING_PREFIX => {
                let start = self.at;
                let length = self.aml.get(start..)?.iter().position(|&byte| byte == 0)?;
                self.at = start + length + 1;
                Value::String(&self.aml[start..start + length])
            }
            BUFFER_OP => {
                let end = self.package_end()?;
                let Value::Integer(_) = self.value()? else {
                    return None;
                };
                let start = self.at;
                self.at = end;
                Value::Buffer(self.aml.get(start..end)?)
            }
            PACKAGE_OP | VAR_PACKAGE_OP => {
                self.at = self.package_end()?;
                Value::Other
            }
            _ => return None,
        })
    }
}

fn last_segment(path: &Path) -> [u8; 4] {
    path.depth.checked_sub(1).map_or([0; 4], |last| path.segments[last])
}

fn id_of(value: Value<'_>) -> Option<Id> {
    match value {
        Value::Integer(eisa) => u32::try_from(eisa).ok().map(Id::eisa),
        Value::String(text) => Some(Id::text(text)),
        _ => None,
    }
}

/// Walks the term list `reader` holds up to `end` in scope `scope`; `device` is the device being
/// walked, which its names fill.
fn walk<'a>(
    reader: &mut Reader<'a>,
    end: usize,
    scope: &Path,
    mut device: Option<&mut Device<'a>>,
    found: &mut dyn FnMut(Device<'a>),
) {
    while reader.at < end {
        let Some(opcode) = reader.byte() else {
            return;
        };
        let walked = (|| -> Option<()> {
            match opcode {
                SCOPE_OP => {
                    let inner_end = reader.package_end()?;
                    let path = reader.name(scope)?;
                    walk(reader, inner_end, &path, None, found);
                    reader.at = inner_end;
                }
                NAME_OP => {
                    let path = reader.name(scope)?;
                    let value = reader.value()?;
                    if let Some(device) = device.as_deref_mut() {
                        match &last_segment(&path) {
                            b"_HID" => device.hid = id_of(value),
                            b"_CID" => device.cid = id_of(value),
                            b"_UID" => device.uid = if let Value::Integer(uid) = value { Some(uid) } else { None },
                            b"_ADR" => device.address = if let Value::Integer(address) = value { Some(address) } else { None },
                            b"_CRS" => device.resources = if let Value::Buffer(bytes) = value { Some(bytes) } else { None },
                            _ => {}
                        }
                    }
                }
                METHOD_OP | IF_OP | ELSE_OP | WHILE_OP => reader.at = reader.package_end()?,
                ALIAS_OP => {
                    reader.name(scope)?;
                    reader.name(scope)?;
                }
                EXTERNAL_OP => {
                    reader.name(scope)?;
                    reader.bytes(2)?;
                }
                EXTENDED_PREFIX => match reader.byte()? {
                    DEVICE_OP | PROCESSOR_OP | POWER_RESOURCE_OP | THERMAL_ZONE_OP => {
                        let kind = reader.aml[reader.at - 1];
                        let inner_end = reader.package_end()?;
                        let path = reader.name(scope)?;
                        match kind {
                            PROCESSOR_OP => {
                                reader.bytes(6)?;
                            }
                            POWER_RESOURCE_OP => {
                                reader.bytes(3)?;
                            }
                            _ => {}
                        }
                        let mut inner = Device {
                            path,
                            ..Device::default()
                        };
                        walk(reader, inner_end, &path, Some(&mut inner), found);
                        if kind == DEVICE_OP {
                            found(inner);
                        }
                        reader.at = inner_end;
                    }
                    FIELD_OP | INDEX_FIELD_OP | BANK_FIELD_OP => reader.at = reader.package_end()?,
                    MUTEX_OP => {
                        reader.name(scope)?;
                        reader.bytes(1)?;
                    }
                    EVENT_OP => {
                        reader.name(scope)?;
                    }
                    REGION_OP => {
                        reader.name(scope)?;
                        reader.bytes(1)?;
                        let Value::Integer(_) = reader.value()? else { return None };
                        let Value::Integer(_) = reader.value()? else { return None };
                    }
                    _ => return None,
                },
                _ => return None,
            }
            Some(())
        })();
        if walked.is_none() {
            reader.at = end;
            return;
        }
    }
}

/// Calls `found` with every device a definition block (a DSDT or SSDT, without its 36-byte
/// header) declares in its static part.
pub fn devices<'a>(aml: &'a [u8], found: &mut dyn FnMut(Device<'a>)) {
    let mut reader = Reader { aml, at: 0 };
    walk(&mut reader, aml.len(), &Path::default(), None, found);
}

/// The memory ranges a `_CRS` resource template names (ACPI 6.5, 6.4): 32-bit fixed memory and
/// DWord and QWord memory address spaces, as start and length.
pub fn memory_resources(template: &[u8], range: &mut dyn FnMut(u64, u64)) {
    const MEMORY32_FIXED: u8 = 0x86;
    const DWORD_ADDRESS: u8 = 0x87;
    const QWORD_ADDRESS: u8 = 0x8a;
    const END_TAG: u8 = 0x79;
    const MEMORY_SPACE: u8 = 0;
    let mut at = 0;
    while at < template.len() {
        let tag = template[at];
        if tag & 0x80 == 0 {
            if tag & 0x78 == END_TAG & 0x78 {
                return;
            }
            at += 1 + usize::from(tag & 0x07);
            continue;
        }
        let Some(length) = template.get(at + 1..at + 3).map(|bytes| usize::from(u16::from_le_bytes([bytes[0], bytes[1]]))) else {
            return;
        };
        let Some(body) = template.get(at + 3..at + 3 + length) else {
            return;
        };
        let read = |from: usize, bytes: usize| -> u64 {
            body.get(from..from + bytes).map_or(0, |raw| raw.iter().rev().fold(0, |value, &byte| (value << 8) | u64::from(byte)))
        };
        match tag {
            MEMORY32_FIXED if length >= 9 => range(read(1, 4), read(5, 4)),
            DWORD_ADDRESS if length >= 23 && body[0] == MEMORY_SPACE => range(read(7, 4), read(19, 4)),
            QWORD_ADDRESS if length >= 43 && body[0] == MEMORY_SPACE => range(read(11, 8), read(35, 8)),
            _ => {}
        }
        at += 3 + length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(body: &[u8]) -> Vec<u8> {
        let length = body.len() + 1;
        assert!(length < 64);
        let mut out = vec![length as u8];
        out.extend_from_slice(body);
        out
    }

    fn device(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut inner = name.to_vec();
        inner.extend_from_slice(body);
        let mut out = vec![EXTENDED_PREFIX, DEVICE_OP];
        out.extend(package(&inner));
        out
    }

    #[test]
    fn eisa_ids_decode_to_their_text() {
        assert_eq!(Id::eisa(0x0303_d041).as_bytes(), b"PNP0303");
    }

    #[test]
    fn devices_in_scopes_carry_their_names() {
        let mut keyboard = vec![NAME_OP];
        keyboard.extend_from_slice(b"_HID");
        keyboard.extend_from_slice(&[DWORD_PREFIX, 0x41, 0xd0, 0x03, 0x03]);
        keyboard.extend_from_slice(&[METHOD_OP, 3, 0, 0]);
        keyboard.push(NAME_OP);
        keyboard.extend_from_slice(b"_UID");
        keyboard.push(ONE_OP);
        let mut touchpad = vec![NAME_OP];
        touchpad.extend_from_slice(b"_HID");
        touchpad.push(STRING_PREFIX);
        touchpad.extend_from_slice(b"ELAN0001\0");
        touchpad.push(NAME_OP);
        touchpad.extend_from_slice(b"_CRS");
        touchpad.extend_from_slice(&[BUFFER_OP, 5, BYTE_PREFIX, 2, 0x79, 0]);
        let mut scope_body = b"_SB_".to_vec();
        scope_body.extend(device(b"PS2K", &keyboard));
        scope_body.extend(device(b"TPD0", &touchpad));
        let mut aml = vec![SCOPE_OP];
        aml.extend(package(&scope_body));
        let mut found = Vec::new();
        devices(&aml, &mut |device| found.push(device));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].hid.unwrap().as_bytes(), b"PNP0303");
        assert_eq!(found[0].uid, Some(1));
        let mut path = [0u8; 32];
        let length = found[0].path.write(&mut path);
        assert_eq!(&path[..length], b"\\_SB.PS2K");
        assert_eq!(found[1].hid.unwrap().as_bytes(), b"ELAN0001");
        assert_eq!(found[1].resources, Some(&[0x79u8, 0][..]));
    }

    #[test]
    fn an_unknown_term_ends_only_its_own_scope() {
        let mut first = vec![NAME_OP];
        first.extend_from_slice(b"_HID");
        first.extend_from_slice(&[STRING_PREFIX, b'A', 0]);
        first.push(0x70);
        first.extend_from_slice(&[NAME_OP]);
        first.extend_from_slice(b"_UID");
        first.push(ONE_OP);
        let mut aml = device(b"DEV0", &first);
        let mut second = vec![NAME_OP];
        second.extend_from_slice(b"_HID");
        second.extend_from_slice(&[STRING_PREFIX, b'B', 0]);
        aml.extend(device(b"DEV1", &second));
        let mut found = Vec::new();
        devices(&aml, &mut |device| found.push(device));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].uid, None);
        assert_eq!(found[1].hid.unwrap().as_bytes(), b"B");
    }

    #[test]
    fn fixed_memory_resources_are_read() {
        let template = [0x86, 9, 0, 1, 0x00, 0x00, 0xd1, 0xfe, 0x00, 0x10, 0x00, 0x00, 0x79, 0];
        let mut ranges = Vec::new();
        memory_resources(&template, &mut |start, length| ranges.push((start, length)));
        assert_eq!(ranges, vec![(0xfed1_0000, 0x1000)]);
    }
}
