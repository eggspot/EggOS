//! An Android App Bundle's protocol-buffer files (D758): the wire format read field by field,
//! a compiled XML file (`XmlNode`, as `aapt2` writes a bundle's manifest and layouts) turned
//! into the binary XML an APK holds, so every reader of binary XML takes it unchanged, and the
//! resource table (`resources.pb`) read into the same values `resources.arsc` gives.

use alloc::string::String;
use alloc::vec::Vec;

const VARINT: u8 = 0;
const FIXED_64: u8 = 1;
const LENGTH: u8 = 2;
const FIXED_32: u8 = 5;
const MOST_DEPTH: usize = 64;

const XML_CHUNK: u16 = 0x0003;
const STRING_POOL: u16 = 0x0001;
const START_ELEMENT: u16 = 0x0102;
const END_ELEMENT: u16 = 0x0103;
const RESOURCE_MAP: u16 = 0x0180;
const UTF8_FLAG: u32 = 1 << 8;
const NO_STRING: u32 = u32::MAX;
const ATTRIBUTE_BYTES: u16 = 20;

/// The value types binary XML gives an attribute.
pub mod value_type {
    pub const NULL: u8 = 0x00;
    pub const REFERENCE: u8 = 0x01;
    pub const ATTRIBUTE: u8 = 0x02;
    pub const STRING: u8 = 0x03;
    pub const FLOAT: u8 = 0x04;
    pub const DIMENSION: u8 = 0x05;
    pub const FRACTION: u8 = 0x06;
    pub const INT_DECIMAL: u8 = 0x10;
    pub const INT_HEXADECIMAL: u8 = 0x11;
    pub const INT_BOOLEAN: u8 = 0x12;
    pub const COLOUR_ARGB8: u8 = 0x1c;
    pub const COLOUR_RGB8: u8 = 0x1d;
    pub const COLOUR_ARGB4: u8 = 0x1e;
    pub const COLOUR_RGB4: u8 = 0x1f;
}

/// One field of a message: its number and its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field<'a> {
    Number(u32, u64),
    Bytes(u32, &'a [u8]),
}

fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// The fields of the message `bytes`, in order; stops at the first malformed one.
pub fn fields(bytes: &[u8]) -> impl Iterator<Item = Field<'_>> {
    let mut at = 0;
    core::iter::from_fn(move || {
        if at >= bytes.len() {
            return None;
        }
        let key = varint(bytes, &mut at)?;
        let number = u32::try_from(key >> 3).ok()?;
        match (key & 7) as u8 {
            VARINT => varint(bytes, &mut at).map(|value| Field::Number(number, value)),
            FIXED_64 => {
                let word = bytes.get(at..at + 8)?;
                at += 8;
                let mut raw = [0u8; 8];
                raw.copy_from_slice(word);
                Some(Field::Number(number, u64::from_le_bytes(raw)))
            }
            LENGTH => {
                let length = usize::try_from(varint(bytes, &mut at)?).ok()?;
                let body = bytes.get(at..at.checked_add(length)?)?;
                at += length;
                Some(Field::Bytes(number, body))
            }
            FIXED_32 => {
                let word = bytes.get(at..at + 4)?;
                at += 4;
                Some(Field::Number(
                    number,
                    u64::from(u32::from_le_bytes([word[0], word[1], word[2], word[3]])),
                ))
            }
            _ => None,
        }
    })
}

fn message(bytes: &[u8], wanted: u32) -> Option<&[u8]> {
    fields(bytes).find_map(|field| match field {
        Field::Bytes(number, body) if number == wanted => Some(body),
        _ => None,
    })
}

fn number(bytes: &[u8], wanted: u32) -> Option<u64> {
    fields(bytes).find_map(|field| match field {
        Field::Number(number, value) if number == wanted => Some(value),
        _ => None,
    })
}

fn text(bytes: &[u8], wanted: u32) -> Option<&str> {
    message(bytes, wanted).and_then(|body| core::str::from_utf8(body).ok())
}

fn messages(bytes: &[u8], wanted: u32) -> impl Iterator<Item = &[u8]> {
    fields(bytes).filter_map(move |field| match field {
        Field::Bytes(number, body) if number == wanted => Some(body),
        _ => None,
    })
}

/// A compiled value (`Item`): its binary-XML type and data, and its text when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    pub kind: u8,
    pub data: u32,
    pub text: Option<String>,
}

/// Reads an `Item` message: a reference, a string, a file, or a primitive.
pub fn item(bytes: &[u8]) -> Option<Value> {
    for field in fields(bytes) {
        let Field::Bytes(number, body) = field else {
            continue;
        };
        let value = match number {
            1 => {
                let attribute = number_or_zero(body, 1) == 1;
                Value {
                    kind: match attribute {
                        true => value_type::ATTRIBUTE,
                        false => value_type::REFERENCE,
                    },
                    data: number_or_zero(body, 2) as u32,
                    text: None,
                }
            }
            2 | 3 => string_value(text(body, 1).unwrap_or("")),
            4 => string_value(text(body, 1).unwrap_or("")),
            5 => string_value(text(body, 1).unwrap_or("")),
            6 => Value {
                kind: value_type::NULL,
                data: 0,
                text: None,
            },
            7 => primitive(body)?,
            _ => continue,
        };
        return Some(value);
    }
    None
}

fn number_or_zero(bytes: &[u8], wanted: u32) -> u64 {
    number(bytes, wanted).unwrap_or(0)
}

fn string_value(text: &str) -> Value {
    Value {
        kind: value_type::STRING,
        data: 0,
        text: Some(String::from(text)),
    }
}

fn primitive(bytes: &[u8]) -> Option<Value> {
    let (number, raw) = fields(bytes).find_map(|field| match field {
        Field::Number(number, raw) => Some((number, raw)),
        Field::Bytes(number, _) if number == 1 || number == 2 => Some((number, 0)),
        Field::Bytes(..) => None,
    })?;
    let kind = match number {
        1 | 2 => value_type::NULL,
        3 => value_type::FLOAT,
        4 | 13 => value_type::DIMENSION,
        5 | 14 => value_type::FRACTION,
        6 => value_type::INT_DECIMAL,
        7 => value_type::INT_HEXADECIMAL,
        8 => value_type::INT_BOOLEAN,
        9 => value_type::COLOUR_ARGB8,
        10 => value_type::COLOUR_RGB8,
        11 => value_type::COLOUR_ARGB4,
        12 => value_type::COLOUR_RGB4,
        _ => return None,
    };
    let data = match kind {
        value_type::INT_BOOLEAN if raw != 0 => u32::MAX,
        _ => raw as u32,
    };
    Some(Value {
        kind,
        data,
        text: None,
    })
}

#[derive(Default)]
struct Writer {
    strings: Vec<String>,
    attribute_ids: Vec<u32>,
    elements: Vec<u8>,
}

impl Writer {
    fn index(&mut self, text: &str) -> u32 {
        match self.strings.iter().position(|known| known == text) {
            Some(at) => at as u32,
            None => {
                self.strings.push(String::from(text));
                (self.strings.len() - 1) as u32
            }
        }
    }

    fn element(&mut self, element: &[u8], depth: usize) -> Option<()> {
        if depth > MOST_DEPTH {
            return None;
        }
        let name = self.index(text(element, 3).unwrap_or(""));
        let namespace = match text(element, 2) {
            Some(uri) if !uri.is_empty() => self.index(uri),
            _ => NO_STRING,
        };
        let mut attributes = Vec::new();
        for attribute in messages(element, 4) {
            let attribute_name = text(attribute, 2).unwrap_or("");
            let attribute_namespace = match text(attribute, 1) {
                Some(uri) if !uri.is_empty() => self.index(uri),
                _ => NO_STRING,
            };
            let name_index = self.index(attribute_name);
            let raw = text(attribute, 3).unwrap_or("");
            let compiled = message(attribute, 6).and_then(item);
            let value = compiled.unwrap_or_else(|| string_value(raw));
            let (raw_index, data) = match value.kind {
                value_type::STRING => {
                    let shown = value.text.as_deref().unwrap_or(raw);
                    let index = self.index(shown);
                    (index, index)
                }
                _ if !raw.is_empty() => (self.index(raw), value.data),
                _ => (NO_STRING, value.data),
            };
            attributes.push((attribute_namespace, name_index, raw_index, value.kind, data));
        }
        let size = 36 + usize::from(ATTRIBUTE_BYTES) * attributes.len();
        let chunk = &mut self.elements;
        chunk.extend_from_slice(&START_ELEMENT.to_le_bytes());
        chunk.extend_from_slice(&16u16.to_le_bytes());
        chunk.extend_from_slice(&(size as u32).to_le_bytes());
        chunk.extend_from_slice(&[0; 4]);
        chunk.extend_from_slice(&NO_STRING.to_le_bytes());
        chunk.extend_from_slice(&namespace.to_le_bytes());
        chunk.extend_from_slice(&name.to_le_bytes());
        chunk.extend_from_slice(&20u16.to_le_bytes());
        chunk.extend_from_slice(&ATTRIBUTE_BYTES.to_le_bytes());
        chunk.extend_from_slice(&(attributes.len() as u16).to_le_bytes());
        chunk.extend_from_slice(&[0; 6]);
        for (attribute_namespace, name_index, raw_index, kind, data) in attributes {
            chunk.extend_from_slice(&attribute_namespace.to_le_bytes());
            chunk.extend_from_slice(&name_index.to_le_bytes());
            chunk.extend_from_slice(&raw_index.to_le_bytes());
            chunk.extend_from_slice(&[8, 0, 0, kind]);
            chunk.extend_from_slice(&data.to_le_bytes());
        }
        for child in messages(element, 5) {
            if let Some(inner) = message(child, 1) {
                self.element(inner, depth + 1)?;
            }
        }
        let chunk = &mut self.elements;
        chunk.extend_from_slice(&END_ELEMENT.to_le_bytes());
        chunk.extend_from_slice(&16u16.to_le_bytes());
        chunk.extend_from_slice(&24u32.to_le_bytes());
        chunk.extend_from_slice(&[0; 4]);
        chunk.extend_from_slice(&NO_STRING.to_le_bytes());
        chunk.extend_from_slice(&namespace.to_le_bytes());
        chunk.extend_from_slice(&name.to_le_bytes());
        Some(())
    }

    fn note_attribute_ids(&mut self, element: &[u8], depth: usize) {
        if depth > MOST_DEPTH {
            return;
        }
        for attribute in messages(element, 4) {
            let id = number_or_zero(attribute, 5) as u32;
            let name = text(attribute, 2).unwrap_or("");
            if id != 0 {
                let index = self.index(name) as usize;
                if self.attribute_ids.len() <= index {
                    self.attribute_ids.resize(index + 1, 0);
                }
                self.attribute_ids[index] = id;
            }
        }
        for child in messages(element, 5) {
            if let Some(inner) = message(child, 1) {
                self.note_attribute_ids(inner, depth + 1);
            }
        }
    }

    fn finish(self) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for string in &self.strings {
            offsets.push(data.len() as u32);
            push_utf8_length(&mut data, string.chars().count());
            push_utf8_length(&mut data, string.len());
            data.extend_from_slice(string.as_bytes());
            data.push(0);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let header = 28usize;
        let start = header + 4 * offsets.len();
        let mut pool = Vec::new();
        pool.extend_from_slice(&STRING_POOL.to_le_bytes());
        pool.extend_from_slice(&(header as u16).to_le_bytes());
        pool.extend_from_slice(&((start + data.len()) as u32).to_le_bytes());
        pool.extend_from_slice(&(offsets.len() as u32).to_le_bytes());
        pool.extend_from_slice(&0u32.to_le_bytes());
        pool.extend_from_slice(&UTF8_FLAG.to_le_bytes());
        pool.extend_from_slice(&(start as u32).to_le_bytes());
        pool.extend_from_slice(&0u32.to_le_bytes());
        for offset in offsets {
            pool.extend_from_slice(&offset.to_le_bytes());
        }
        pool.extend_from_slice(&data);
        let mut map = Vec::new();
        if !self.attribute_ids.is_empty() {
            map.extend_from_slice(&RESOURCE_MAP.to_le_bytes());
            map.extend_from_slice(&8u16.to_le_bytes());
            map.extend_from_slice(&((8 + 4 * self.attribute_ids.len()) as u32).to_le_bytes());
            for id in &self.attribute_ids {
                map.extend_from_slice(&id.to_le_bytes());
            }
        }
        let total = 8 + pool.len() + map.len() + self.elements.len();
        let mut xml = Vec::with_capacity(total);
        xml.extend_from_slice(&XML_CHUNK.to_le_bytes());
        xml.extend_from_slice(&8u16.to_le_bytes());
        xml.extend_from_slice(&(total as u32).to_le_bytes());
        xml.extend_from_slice(&pool);
        xml.extend_from_slice(&map);
        xml.extend_from_slice(&self.elements);
        xml
    }
}

fn push_utf8_length(data: &mut Vec<u8>, length: usize) {
    if length > 0x7f {
        data.push(0x80 | ((length >> 8) & 0x7f) as u8);
    }
    data.push((length & 0xff) as u8);
}

/// The binary XML an APK would hold for the compiled XML file `proto` (an `XmlNode` whose
/// root is an element); `None` when it is not one.
pub fn binary_xml(proto: &[u8]) -> Option<Vec<u8>> {
    let root = message(proto, 1)?;
    let mut writer = Writer::default();
    writer.note_attribute_ids(root, 0);
    writer.element(root, 0)?;
    Some(writer.finish())
}

/// One value of the resource table: its resource id and its value in one configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableValue {
    pub id: u32,
    pub value: Value,
}

/// Every simple value of the bundle resource table `proto` (`resources.pb`), each
/// configuration's in the table's order.
pub fn table_values(proto: &[u8]) -> Vec<TableValue> {
    let mut found = Vec::new();
    for package in messages(proto, 2) {
        let package_id = message(package, 1).map_or(0x7f, |id| number_or_zero(id, 1)) as u32;
        for kind in messages(package, 3) {
            let type_id = message(kind, 1).map_or(0, |id| number_or_zero(id, 1)) as u32;
            for entry in messages(kind, 3) {
                let entry_id = message(entry, 1).map_or(0, |id| number_or_zero(id, 1)) as u32;
                let id = (package_id & 0xff) << 24 | (type_id & 0xff) << 16 | (entry_id & 0xffff);
                for config_value in messages(entry, 6) {
                    let Some(value) = message(config_value, 2) else {
                        continue;
                    };
                    if let Some(value) = message(value, 4).and_then(item) {
                        found.push(TableValue { id, value });
                    }
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut value = value;
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn bytes_field(number: u32, body: &[u8]) -> Vec<u8> {
        let mut out = encoded(u64::from(number) << 3 | u64::from(LENGTH));
        out.extend(encoded(body.len() as u64));
        out.extend_from_slice(body);
        out
    }

    fn number_field(number: u32, value: u64) -> Vec<u8> {
        let mut out = encoded(u64::from(number) << 3 | u64::from(VARINT));
        out.extend(encoded(value));
        out
    }

    #[test]
    fn a_compiled_manifest_becomes_binary_xml_the_manifest_reader_reads() {
        let reference = [number_field(2, 0x7f0f_0001)].concat();
        let label_item = bytes_field(1, &reference);
        let label = [
            bytes_field(1, b"http://schemas.android.com/apk/res/android"),
            bytes_field(2, b"label"),
            number_field(5, 0x0101_0001),
            bytes_field(6, &label_item),
        ]
        .concat();
        let application = [bytes_field(3, b"application"), bytes_field(4, &label)].concat();
        let package = [bytes_field(2, b"package"), bytes_field(3, b"org.example.notes")].concat();
        let root = [
            bytes_field(3, b"manifest"),
            bytes_field(4, &package),
            bytes_field(5, &bytes_field(1, &application)),
        ]
        .concat();
        let node = bytes_field(1, &root);
        let xml = binary_xml(&node).unwrap();
        let manifest = crate::manifest::Manifest::parse(&xml).unwrap();
        assert_eq!(manifest.package, "org.example.notes");
        assert_eq!(
            manifest.label,
            Some(crate::manifest::Label::Resource(0x7f0f_0001))
        );
    }

    #[test]
    fn a_bundle_table_gives_strings_and_files_by_resource_id() {
        let string_item = bytes_field(2, &bytes_field(1, b"Notes"));
        let value = bytes_field(4, &string_item);
        let config_value = bytes_field(2, &value);
        let entry = [bytes_field(1, &number_field(1, 1)), bytes_field(6, &config_value)].concat();
        let kind = [bytes_field(1, &number_field(1, 0x0f)), bytes_field(3, &entry)].concat();
        let package = [bytes_field(1, &number_field(1, 0x7f)), bytes_field(3, &kind)].concat();
        let table = bytes_field(2, &package);
        let values = table_values(&table);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].id, 0x7f0f_0001);
        assert_eq!(values[0].value.text.as_deref(), Some("Notes"));
    }
}
