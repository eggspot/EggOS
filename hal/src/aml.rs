//! A walk of the ACPI namespace's static part (ACPI 6.5, chapter 20, AML): the devices a DSDT
//! declares, with the `_HID`, `_CID`, `_UID`, `_ADR` and `_CRS` they name as data (D252). Methods
//! are not run: a value a method computes is not seen, and a term this walk does not know ends
//! its scope's walk there, so nothing past it in that scope is misread. A device's I2C connection
//! is read from the first `I2cSerialBus` descriptor in its buffers or its methods' bodies, where
//! firmware often builds `_CRS`, and the HID over I2C descriptor's register from its `_DSM`'s
//! first constant returned after the HID UUID (D462): both stand for what running the methods
//! would give in the common shapes, and a driver checks what it finds there.

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
const RETURN_OP: u8 = 0xa4;
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
            let trimmed = segment
                .iter()
                .rposition(|&byte| byte != b'_')
                .map_or(1, |last| last + 1);
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
    /// Its connection to an I2C bus, from the first `I2cSerialBus` descriptor found (D462).
    pub i2c: Option<I2cConnection<'a>>,
    /// The HID over I2C descriptor's register its `_DSM` returns, as far as it is read.
    pub hid_descriptor_register: Option<u16>,
}

/// A target on an I2C bus (`I2cSerialBusV2`, ACPI 6.5, 6.4.3.8.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct I2cConnection<'a> {
    pub address: u16,
    pub ten_bit: bool,
    pub speed_hz: u32,
    /// The controller's namespace path as the descriptor names it, without its NUL.
    pub controller: &'a [u8],
}

const SERIAL_BUS_TAG: u8 = 0x8e;
const SERIAL_BUS_I2C: u8 = 1;
const I2C_TYPE_DATA_BYTES: usize = 6;
const I2C_TEN_BIT: u8 = 1;
const HID_I2C_UUID: [u8; 16] = [
    0xf7, 0xf6, 0xdf, 0x3c, 0x67, 0x42, 0x55, 0x45, 0xad, 0x05, 0xb3, 0x0a, 0x3d, 0x89, 0x38, 0xde,
];

/// The first I2C connection an `I2cSerialBus` descriptor in `bytes` names: a resource template,
/// or a method's body that builds one.
pub fn i2c_connection(bytes: &[u8]) -> Option<I2cConnection<'_>> {
    (0..bytes.len()).find_map(|at| {
        if bytes[at] != SERIAL_BUS_TAG {
            return None;
        }
        let length = usize::from(u16::from_le_bytes([
            *bytes.get(at + 1)?,
            *bytes.get(at + 2)?,
        ]));
        let body = bytes.get(at + 3..at + 3 + length)?;
        if *body.get(2)? != SERIAL_BUS_I2C {
            return None;
        }
        let type_data = usize::from(u16::from_le_bytes([*body.get(7)?, *body.get(8)?]));
        if type_data < I2C_TYPE_DATA_BYTES {
            return None;
        }
        let speed_hz = u32::from_le_bytes(body.get(9..13)?.try_into().ok()?);
        let address = u16::from_le_bytes([*body.get(13)?, *body.get(14)?]);
        let source = body.get(9 + type_data..)?;
        let end = source.iter().position(|&byte| byte == 0)?;
        let controller = &source[..end];
        let starts_a_name = |first: &u8| {
            matches!(*first, ROOT_CHAR | PARENT_PREFIX | b'_') || first.is_ascii_uppercase()
        };
        let named = controller.first().is_some_and(starts_a_name)
            && controller.iter().all(u8::is_ascii_graphic);
        named.then_some(I2cConnection {
            address,
            ten_bit: body[4] & I2C_TEN_BIT != 0,
            speed_hz,
            controller,
        })
    })
}

/// The HID over I2C descriptor's register a `_DSM` method's `body` returns: the first constant
/// a `Return` gives after the HID over I2C UUID.
pub fn hid_descriptor_register(body: &[u8]) -> Option<u16> {
    let uuid = body
        .windows(HID_I2C_UUID.len())
        .position(|window| window == HID_I2C_UUID)?;
    let after = &body[uuid + HID_I2C_UUID.len()..];
    (0..after.len()).find_map(|at| {
        if after[at] != RETURN_OP {
            return None;
        }
        match *after.get(at + 1)? {
            ONE_OP => Some(1),
            BYTE_PREFIX => after.get(at + 2).map(|&value| u16::from(value)),
            WORD_PREFIX => Some(u16::from_le_bytes([
                *after.get(at + 2)?,
                *after.get(at + 3)?,
            ])),
            _ => None,
        }
    })
}

/// Whether `named`, a path as a descriptor names it (`\_SB_.PCI0.I2C1`, or relative), is the
/// device at `path`: segments compared without their trailing `_` padding, a relative name by
/// its last segments.
pub fn names_path(named: &[u8], path: &Path) -> bool {
    let absolute = named.first() == Some(&ROOT_CHAR);
    let text = named
        .iter()
        .position(|&byte| byte != ROOT_CHAR && byte != PARENT_PREFIX);
    let Some(text) = text.map(|start| &named[start..]) else {
        return false;
    };
    let segments = text.split(|&byte| byte == b'.');
    let count = segments.clone().count();
    if count > path.depth || (absolute && count != path.depth) {
        return false;
    }
    segments
        .zip(&path.segments[path.depth - count..path.depth])
        .all(|(named, segment)| unpadded(named) == unpadded(segment))
}

fn unpadded(segment: &[u8]) -> &[u8] {
    let length = segment
        .iter()
        .rposition(|&byte| byte != b'_')
        .map_or(1, |last| last + 1);
    &segment[..length.min(segment.len())]
}

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
        Some(
            raw.iter()
                .rev()
                .fold(0u64, |value, &byte| (value << 8) | u64::from(byte)),
        )
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
    path.depth
        .checked_sub(1)
        .map_or([0; 4], |last| path.segments[last])
}

fn id_of(value: Value<'_>) -> Option<Id> {
    match value {
        Value::Integer(eisa) => u32::try_from(eisa).ok().map(Id::eisa),
        Value::String(text) => Some(Id::text(text)),
        _ => None,
    }
}

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
                            b"_UID" => {
                                device.uid = if let Value::Integer(uid) = value {
                                    Some(uid)
                                } else {
                                    None
                                }
                            }
                            b"_ADR" => {
                                device.address = if let Value::Integer(address) = value {
                                    Some(address)
                                } else {
                                    None
                                }
                            }
                            b"_CRS" => {
                                device.resources = if let Value::Buffer(bytes) = value {
                                    Some(bytes)
                                } else {
                                    None
                                }
                            }
                            _ => {}
                        }
                        if let Value::Buffer(bytes) = value
                            && let Some(connection) = i2c_connection(bytes)
                            && (device.i2c.is_none() || last_segment(&path) == *b"_CRS")
                        {
                            device.i2c = Some(connection);
                        }
                    }
                }
                METHOD_OP => {
                    let method_end = reader.package_end()?;
                    let start = reader.at;
                    let path = reader.name(scope)?;
                    if let Some(device) = device.as_deref_mut()
                        && let Some(body) = reader.aml.get(start..method_end)
                    {
                        if device.i2c.is_none() {
                            device.i2c = i2c_connection(body);
                        }
                        if last_segment(&path) == *b"_DSM" {
                            device.hid_descriptor_register = hid_descriptor_register(body);
                        }
                    }
                    reader.at = method_end;
                }
                IF_OP | ELSE_OP | WHILE_OP => reader.at = reader.package_end()?,
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
                    FIELD_OP | INDEX_FIELD_OP | BANK_FIELD_OP => {
                        reader.at = reader.package_end()?
                    }
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
                        let Value::Integer(_) = reader.value()? else {
                            return None;
                        };
                        let Value::Integer(_) = reader.value()? else {
                            return None;
                        };
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
        let Some(length) = template
            .get(at + 1..at + 3)
            .map(|bytes| usize::from(u16::from_le_bytes([bytes[0], bytes[1]])))
        else {
            return;
        };
        let Some(body) = template.get(at + 3..at + 3 + length) else {
            return;
        };
        let read = |from: usize, bytes: usize| -> u64 {
            body.get(from..from + bytes).map_or(0, |raw| {
                raw.iter()
                    .rev()
                    .fold(0, |value, &byte| (value << 8) | u64::from(byte))
            })
        };
        match tag {
            MEMORY32_FIXED if length >= 9 => range(read(1, 4), read(5, 4)),
            DWORD_ADDRESS if length >= 23 && body[0] == MEMORY_SPACE => {
                range(read(7, 4), read(19, 4))
            }
            QWORD_ADDRESS if length >= 43 && body[0] == MEMORY_SPACE => {
                range(read(11, 8), read(35, 8))
            }
            _ => {}
        }
        at += 3 + length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(body: &[u8]) -> Vec<u8> {
        let mut out = match body.len() + 1 < 64 {
            true => vec![(body.len() + 1) as u8],
            false => {
                let length = body.len() + 2;
                assert!(length < 4096);
                vec![0x40 | (length & 0x0f) as u8, (length >> 4) as u8]
            }
        };
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

    fn i2c_descriptor(address: u16, controller: &[u8]) -> Vec<u8> {
        let mut body = vec![2, 0, SERIAL_BUS_I2C, 0x02, 0, 0, 1, 6, 0];
        body.extend_from_slice(&400_000u32.to_le_bytes());
        body.extend_from_slice(&address.to_le_bytes());
        body.extend_from_slice(controller);
        body.push(0);
        let mut out = vec![SERIAL_BUS_TAG];
        out.extend_from_slice(&(body.len() as u16).to_le_bytes());
        out.extend(body);
        out.extend_from_slice(&[0x79, 0]);
        out
    }

    #[test]
    fn an_i2c_connection_is_read_from_its_descriptor() {
        let template = i2c_descriptor(0x15, b"\\_SB.PCI0.I2C1");
        let connection = i2c_connection(&template).unwrap();
        assert_eq!(connection.address, 0x15);
        assert_eq!(connection.speed_hz, 400_000);
        assert!(!connection.ten_bit);
        assert_eq!(connection.controller, b"\\_SB.PCI0.I2C1");
    }

    #[test]
    fn a_crs_method_s_body_gives_the_connection_and_the_dsm_the_descriptor_register() {
        let mut crs = b"_CRS".to_vec();
        crs.push(0);
        crs.push(RETURN_OP);
        crs.extend_from_slice(&[BUFFER_OP, 40, BYTE_PREFIX, 37]);
        crs.extend(i2c_descriptor(0x2c, b"\\_SB.PCI0.I2C0"));
        let mut dsm = b"_DSM".to_vec();
        dsm.push(4);
        dsm.extend_from_slice(&[IF_OP, 20]);
        dsm.extend_from_slice(&HID_I2C_UUID);
        dsm.extend_from_slice(&[RETURN_OP, BYTE_PREFIX, 0x20]);
        let mut body = vec![NAME_OP];
        body.extend_from_slice(b"_HID");
        body.push(STRING_PREFIX);
        body.extend_from_slice(b"SMO91D0\0");
        body.push(METHOD_OP);
        body.extend(package(&crs));
        body.push(METHOD_OP);
        body.extend(package(&dsm));
        let aml = device(b"SENS", &body);
        let mut found = Vec::new();
        devices(&aml, &mut |device| found.push(device));
        assert_eq!(found.len(), 1);
        let connection = found[0].i2c.unwrap();
        assert_eq!(connection.address, 0x2c);
        assert_eq!(connection.controller, b"\\_SB.PCI0.I2C0");
        assert_eq!(found[0].hid_descriptor_register, Some(0x20));
    }

    #[test]
    fn a_named_controller_matches_its_device_s_path_without_padding() {
        let path = Path::default()
            .with(*b"_SB_")
            .and_then(|path| path.with(*b"PCI0"))
            .and_then(|path| path.with(*b"I2C1"))
            .unwrap();
        assert!(names_path(b"\\_SB.PCI0.I2C1", &path));
        assert!(names_path(b"\\_SB_.PCI0.I2C1", &path));
        assert!(names_path(b"I2C1", &path));
        assert!(!names_path(b"\\_SB.PCI0.I2C2", &path));
        assert!(!names_path(b"\\PCI0.I2C1", &path));
    }

    #[test]
    fn fixed_memory_resources_are_read() {
        let template = [
            0x86, 9, 0, 1, 0x00, 0x00, 0xd1, 0xfe, 0x00, 0x10, 0x00, 0x00, 0x79, 0,
        ];
        let mut ranges = Vec::new();
        memory_resources(&template, &mut |start, length| ranges.push((start, length)));
        assert_eq!(ranges, vec![(0xfed1_0000, 0x1000)]);
    }
}
