//! Compiled nibs in the `NIBArchive` format (D231), from the file format as `ibtool` writes it:
//! the 10 bytes `NIBArchive`, ten little-endian 32-bit words (format version 1, coder version,
//! then the count and offset of the objects, keys, values and class names), and the four
//! tables. Counts in the tables are variable-length integers of 7-bit groups, least significant
//! first, the last group's byte carrying the high bit. An object names its class and a run of
//! values; a value names its key and holds an integer, a boolean, a float, data, nil or another
//! object; a class name may list fallback classes. A nib is untrusted input: [`NibArchive::index`]
//! checks every count, offset and reference once, against [`Limits`], before anything reads it,
//! and the readers never recurse.

/// The bytes a `NIBArchive` file starts with.
pub const MAGIC: &[u8; 10] = b"NIBArchive";
const HEADER_WORDS: usize = 10;
const HEADER_BYTES: usize = MAGIC.len() + HEADER_WORDS * 4;
const FORMAT_VERSION: u32 = 1;
const LONGEST_VARINT_BYTES: usize = 5;
const OBJECT_REFERENCE_BYTES: usize = 4;

/// The largest table sizes a nib may declare.
pub mod limits {
    pub const OBJECTS: usize = 1 << 16;
    pub const KEYS: usize = 1 << 14;
    pub const VALUES: usize = 1 << 18;
    pub const CLASSES: usize = 1 << 12;
    pub const FALLBACKS: usize = 16;
}

/// Why a nib could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NibError {
    NotANib,
    UnknownVersion,
    TooLarge,
    Damaged,
}

/// The four tables' sizes, as the header gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub objects: usize,
    pub keys: usize,
    pub values: usize,
    pub classes: usize,
}

/// An object's class and the run of values that are its fields.
#[derive(Debug, Clone, Copy, Default)]
pub struct ObjectEntry {
    class: u32,
    first_value: u32,
    value_count: u32,
}

/// A run of bytes in the file.
#[derive(Debug, Clone, Copy, Default)]
pub struct Span {
    start: u32,
    length: u32,
}

/// A class name and where its fallback class indices are.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClassEntry {
    name: Span,
    fallbacks: Span,
}

/// A value: its key, its type and where its payload is.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValueEntry {
    key: u32,
    kind: u8,
    payload: Span,
}

/// The tables [`NibArchive::index`] fills, each as long as [`Counts`] says.
#[derive(Debug)]
pub struct Tables<'t> {
    pub objects: &'t mut [ObjectEntry],
    pub keys: &'t mut [Span],
    pub values: &'t mut [ValueEntry],
    pub classes: &'t mut [ClassEntry],
}

/// One field's value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NibValue<'b> {
    Integer(i64),
    Boolean(bool),
    Real(f64),
    Data(&'b [u8]),
    Nil,
    Object(usize),
}

mod kind {
    pub const INT8: u8 = 0;
    pub const INT16: u8 = 1;
    pub const INT32: u8 = 2;
    pub const INT64: u8 = 3;
    pub const FALSE: u8 = 4;
    pub const TRUE: u8 = 5;
    pub const FLOAT: u8 = 6;
    pub const DOUBLE: u8 = 7;
    pub const DATA: u8 = 8;
    pub const NIL: u8 = 9;
    pub const OBJECT: u8 = 10;
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn to_u32(value: usize) -> Result<u32, NibError> {
    u32::try_from(value).map_err(|_| NibError::TooLarge)
}

struct Cursor<'b> {
    bytes: &'b [u8],
    at: usize,
}

impl<'b> Cursor<'b> {
    fn varint(&mut self) -> Result<usize, NibError> {
        let mut value = 0usize;
        for group in 0..LONGEST_VARINT_BYTES {
            let byte = *self.bytes.get(self.at).ok_or(NibError::Damaged)?;
            self.at += 1;
            value |= usize::from(byte & 0x7f) << (7 * group);
            if byte & 0x80 != 0 {
                return Ok(value);
            }
        }
        Err(NibError::Damaged)
    }

    fn take(&mut self, length: usize) -> Result<Span, NibError> {
        let end = self.at.checked_add(length).ok_or(NibError::Damaged)?;
        if end > self.bytes.len() {
            return Err(NibError::Damaged);
        }
        let span = Span {
            start: to_u32(self.at)?,
            length: to_u32(length)?,
        };
        self.at = end;
        Ok(span)
    }
}

/// The header's table sizes, each checked against [`limits`].
pub fn counts(bytes: &[u8]) -> Result<Counts, NibError> {
    if !bytes.starts_with(MAGIC) || bytes.len() < HEADER_BYTES {
        return Err(NibError::NotANib);
    }
    let header = |index: usize| word(bytes, MAGIC.len() + index * 4).unwrap_or(0) as usize;
    if header(0) as u32 != FORMAT_VERSION {
        return Err(NibError::UnknownVersion);
    }
    let counts = Counts {
        objects: header(2),
        keys: header(4),
        values: header(6),
        classes: header(8),
    };
    let fits = counts.objects <= limits::OBJECTS
        && counts.keys <= limits::KEYS
        && counts.values <= limits::VALUES
        && counts.classes <= limits::CLASSES
        && counts.objects > 0;
    match fits {
        true => Ok(counts),
        false => Err(NibError::TooLarge),
    }
}

/// An indexed nib: every reference in it checked.
#[derive(Debug, Clone, Copy)]
pub struct NibArchive<'b, 't> {
    bytes: &'b [u8],
    objects: &'t [ObjectEntry],
    keys: &'t [Span],
    values: &'t [ValueEntry],
    classes: &'t [ClassEntry],
}

impl<'b, 't> NibArchive<'b, 't> {
    /// Reads `bytes`' four tables into `tables` and checks them.
    pub fn index(bytes: &'b [u8], tables: Tables<'t>) -> Result<Self, NibError> {
        let counts = counts(bytes)?;
        let Tables {
            objects,
            keys,
            values,
            classes,
        } = tables;
        if objects.len() != counts.objects
            || keys.len() != counts.keys
            || values.len() != counts.values
            || classes.len() != counts.classes
        {
            return Err(NibError::Damaged);
        }
        let offset = |index: usize| word(bytes, MAGIC.len() + index * 4).unwrap_or(0) as usize;
        let mut cursor = Cursor {
            bytes,
            at: offset(3),
        };
        for entry in objects.iter_mut() {
            let class = cursor.varint()?;
            let first_value = cursor.varint()?;
            let value_count = cursor.varint()?;
            let end = first_value.checked_add(value_count);
            if class >= counts.classes || end.is_none_or(|end| end > counts.values) {
                return Err(NibError::Damaged);
            }
            *entry = ObjectEntry {
                class: to_u32(class)?,
                first_value: to_u32(first_value)?,
                value_count: to_u32(value_count)?,
            };
        }
        cursor.at = offset(5);
        for entry in keys.iter_mut() {
            let length = cursor.varint()?;
            *entry = cursor.take(length)?;
        }
        cursor.at = offset(7);
        for entry in values.iter_mut() {
            let key = cursor.varint()?;
            if key >= counts.keys {
                return Err(NibError::Damaged);
            }
            let kind = *bytes.get(cursor.at).ok_or(NibError::Damaged)?;
            cursor.at += 1;
            let length = match kind {
                kind::INT8 => 1,
                kind::INT16 => 2,
                kind::INT32 | kind::FLOAT => 4,
                kind::INT64 | kind::DOUBLE => 8,
                kind::TRUE | kind::FALSE | kind::NIL => 0,
                kind::DATA => cursor.varint()?,
                kind::OBJECT => OBJECT_REFERENCE_BYTES,
                _ => return Err(NibError::Damaged),
            };
            let payload = cursor.take(length)?;
            if kind == kind::OBJECT
                && word(bytes, payload.start as usize)
                    .is_none_or(|target| target as usize >= counts.objects)
            {
                return Err(NibError::Damaged);
            }
            *entry = ValueEntry {
                key: to_u32(key)?,
                kind,
                payload,
            };
        }
        cursor.at = offset(9);
        for entry in classes.iter_mut() {
            let length = cursor.varint()?;
            let fallback_count = cursor.varint()?;
            if fallback_count > limits::FALLBACKS || length == 0 {
                return Err(NibError::Damaged);
            }
            let fallbacks = cursor.take(fallback_count * 4)?;
            let name = cursor.take(length)?;
            entry.fallbacks = fallbacks;
            entry.name = name;
        }
        let archive = Self {
            bytes,
            objects,
            keys,
            values,
            classes,
        };
        let fallbacks_valid = (0..archive.classes.len()).all(|class| {
            archive
                .fallback_indices(class)
                .all(|fallback| fallback < counts.classes)
        });
        match fallbacks_valid {
            true => Ok(archive),
            false => Err(NibError::Damaged),
        }
    }

    fn span(&self, span: Span) -> &'b [u8] {
        &self.bytes[span.start as usize..(span.start + span.length) as usize]
    }

    fn fallback_indices(&self, class: usize) -> impl Iterator<Item = usize> {
        let span = self.classes[class].fallbacks;
        let bytes = self.bytes;
        (0..span.length as usize / 4).map(move |index| {
            word(bytes, span.start as usize + index * 4).map_or(usize::MAX, |word| word as usize)
        })
    }

    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    fn class_name_at(&self, class: usize) -> &'b [u8] {
        let name = self.span(self.classes[class].name);
        name.strip_suffix(b"\0").unwrap_or(name)
    }

    /// The class name `object` was archived with, then its fallbacks in order.
    pub fn class_names(&self, object: usize) -> impl Iterator<Item = &'b [u8]> {
        let class = self.objects.get(object).map(|entry| entry.class as usize);
        class.into_iter().flat_map(move |class| {
            core::iter::once(self.class_name_at(class)).chain(
                self.fallback_indices(class)
                    .map(move |fallback| self.class_name_at(fallback)),
            )
        })
    }

    /// The class name `object` was archived with.
    pub fn class_name(&self, object: usize) -> Option<&'b [u8]> {
        self.class_names(object).next()
    }

    fn decode(&self, entry: &ValueEntry) -> NibValue<'b> {
        let payload = self.span(entry.payload);
        let signed = |bytes: &[u8]| -> i64 {
            let mut buffer = [0u8; 8];
            buffer[..bytes.len()].copy_from_slice(bytes);
            let fill = 64 - 8 * bytes.len() as u32;
            (i64::from_le_bytes(buffer) << fill) >> fill
        };
        match entry.kind {
            kind::INT8 | kind::INT16 | kind::INT32 | kind::INT64 => {
                NibValue::Integer(signed(payload))
            }
            kind::TRUE => NibValue::Boolean(true),
            kind::FALSE => NibValue::Boolean(false),
            kind::FLOAT => NibValue::Real(f64::from(f32::from_le_bytes(
                payload.try_into().unwrap_or([0; 4]),
            ))),
            kind::DOUBLE => {
                NibValue::Real(f64::from_le_bytes(payload.try_into().unwrap_or([0; 8])))
            }
            kind::DATA => NibValue::Data(payload),
            kind::OBJECT => NibValue::Object(word(payload, 0).unwrap_or(0) as usize),
            _ => NibValue::Nil,
        }
    }

    /// `object`'s fields: each key with its value, in the order archived (a collection's
    /// members share one key).
    pub fn fields(&self, object: usize) -> impl Iterator<Item = (&'b [u8], NibValue<'b>)> {
        let range = self.objects.get(object).map_or(0..0, |entry| {
            entry.first_value as usize..(entry.first_value + entry.value_count) as usize
        });
        self.values[range]
            .iter()
            .map(move |entry| (self.span(self.keys[entry.key as usize]), self.decode(entry)))
    }

    /// The first value `object` holds under `key`.
    pub fn field(&self, object: usize, key: &[u8]) -> Option<NibValue<'b>> {
        self.fields(object)
            .find_map(|(name, value)| (name == key).then_some(value))
    }

    /// The object `object` refers to under `key`.
    pub fn reference(&self, object: usize, key: &[u8]) -> Option<usize> {
        match self.field(object, key)? {
            NibValue::Object(target) => Some(target),
            _ => None,
        }
    }

    pub fn integer(&self, object: usize, key: &[u8]) -> Option<i64> {
        match self.field(object, key)? {
            NibValue::Integer(value) => Some(value),
            _ => None,
        }
    }

    /// The UTF-8 bytes of the NSString `object` (its `NS.bytes`).
    pub fn string_bytes(&self, object: usize) -> Option<&'b [u8]> {
        let class = self.class_name(object)?;
        if class != b"NSString" && class != b"NSMutableString" {
            return None;
        }
        match self.field(object, STRING_BYTES)? {
            NibValue::Data(bytes) => core::str::from_utf8(bytes).ok().map(str::as_bytes),
            _ => None,
        }
    }

    /// The text of the NSString `object` refers to under `key`.
    pub fn string(&self, object: usize, key: &[u8]) -> Option<&'b [u8]> {
        self.string_bytes(self.reference(object, key)?)
    }

    /// The members of the array or set `object` (`UINibEncoderEmptyKey` values in order).
    pub fn members(&self, object: usize) -> impl Iterator<Item = NibValue<'b>> {
        self.fields(object)
            .filter(|(key, _)| *key == MEMBER_KEY)
            .map(|(_, value)| value)
    }
}

/// The key NSString's bytes are under.
pub const STRING_BYTES: &[u8] = b"NS.bytes";
/// The key every member of an archived collection is under.
pub const MEMBER_KEY: &[u8] = b"UINibEncoderEmptyKey";

/// A point, size or rectangle as a nib writes it (`{x, y}`, `{w, h}`, `{{x, y}, {w, h}}`):
/// its numbers in order; `None` unless exactly `N` numbers are there.
pub fn numbers<const N: usize>(text: &[u8]) -> Option<[f64; N]> {
    let text = core::str::from_utf8(text).ok()?;
    let mut values = [0.0; N];
    let mut parts = text
        .split(|character: char| matches!(character, '{' | '}' | ','))
        .map(str::trim)
        .filter(|part| !part.is_empty());
    for value in values.iter_mut() {
        *value = parts.next()?.parse::<f64>().ok()?;
    }
    parts.next().is_none().then_some(values)
}

/// What a nib's object data (`NSIBObjectData`) says: its root (File's Owner), connections and
/// the windows visible at load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectData {
    pub root: usize,
    pub visible_windows: Option<usize>,
    pub connections: Option<usize>,
    pub objects_keys: Option<usize>,
    pub objects_values: Option<usize>,
}

/// A connection the nib makes once its objects exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection<'b> {
    /// Set `source`'s outlet `name` to `destination`.
    Outlet {
        source: usize,
        destination: usize,
        name: &'b [u8],
    },
    /// Make `destination` (the first responder when `None`) `source`'s target for `action`.
    Action {
        source: usize,
        destination: Option<usize>,
        action: &'b [u8],
    },
}

impl<'b> NibArchive<'b, '_> {
    /// The object data the root object (object 0) points at under `IB.objectdata`.
    pub fn object_data(&self) -> Option<ObjectData> {
        let data = self.reference(0, b"IB.objectdata")?;
        if self.class_name(data)? != b"NSIBObjectData" {
            return None;
        }
        Some(ObjectData {
            root: self.reference(data, b"NSRoot")?,
            visible_windows: self.reference(data, b"NSVisibleWindows"),
            connections: self.reference(data, b"NSConnections"),
            objects_keys: self.reference(data, b"NSObjectsKeys"),
            objects_values: self.reference(data, b"NSObjectsValues"),
        })
    }

    /// The connection `object` describes, if it is an outlet or a control connector.
    pub fn connection(&self, object: usize) -> Option<Connection<'b>> {
        let source = self.reference(object, b"NSSource")?;
        let destination = self.reference(object, b"NSDestination");
        let label = self.string(object, b"NSLabel")?;
        match self.class_name(object)? {
            b"NSNibOutletConnector" => Some(Connection::Outlet {
                source,
                destination: destination?,
                name: label,
            }),
            b"NSNibControlConnector" => Some(Connection::Action {
                source,
                destination,
                action: label,
            }),
            _ => None,
        }
    }

    /// The objects whose parent is the root (File's Owner): the nib's top-level objects, in
    /// the order the nib lists them, the root itself left out.
    pub fn top_level_objects(&self, data: &ObjectData) -> impl Iterator<Item = usize> {
        let keys = data.objects_keys.into_iter().flat_map(|keys| self.members(keys));
        let values = data
            .objects_values
            .into_iter()
            .flat_map(|values| self.members(values));
        let root = data.root;
        keys.zip(values).filter_map(move |pair| match pair {
            (NibValue::Object(child), NibValue::Object(parent))
                if parent == root && child != root =>
            {
                Some(child)
            }
            _ => None,
        })
    }
}

/// The folders of a bundle a nib is looked for in, in order: the resources, then the base and
/// English localizations.
pub const NIB_FOLDERS: [&[u8]; 3] = [
    b"/Contents/Resources/",
    b"/Contents/Resources/Base.lproj/",
    b"/Contents/Resources/en.lproj/",
];

/// What follows a nib's name in its file's path, in order: a nib folder's object file, then a
/// nib file.
pub const NIB_SUFFIXES: [&[u8]; 2] = [b".nib/keyedobjects.nib", b".nib"];

/// `name` without a trailing `.nib`, as `NSMainNibFile` and `loadNibNamed:` may give it.
pub fn nib_stem(name: &[u8]) -> &[u8] {
    name.strip_suffix(b".nib").unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    enum Put {
        Int(i64),
        Bool(bool),
        Double(f64),
        Data(&'static [u8]),
        Nil,
        Object(u32),
    }

    fn varint(out: &mut Vec<u8>, mut value: usize) {
        loop {
            let group = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(group | 0x80);
                return;
            }
            out.push(group);
        }
    }

    fn write(classes: &[&str], objects: &[(usize, Vec<(&str, Put)>)]) -> Vec<u8> {
        let mut keys: Vec<&str> = Vec::new();
        let mut object_table = Vec::new();
        let mut value_table = Vec::new();
        let mut value_count = 0;
        for (class, fields) in objects {
            varint(&mut object_table, *class);
            varint(&mut object_table, value_count);
            varint(&mut object_table, fields.len());
            for (key, value) in fields {
                let index = keys.iter().position(|known| known == key).unwrap_or_else(|| {
                    keys.push(key);
                    keys.len() - 1
                });
                varint(&mut value_table, index);
                match value {
                    Put::Int(number) => {
                        value_table.push(kind::INT64);
                        value_table.extend_from_slice(&number.to_le_bytes());
                    }
                    Put::Bool(flag) => value_table.push(if *flag { kind::TRUE } else { kind::FALSE }),
                    Put::Double(number) => {
                        value_table.push(kind::DOUBLE);
                        value_table.extend_from_slice(&number.to_le_bytes());
                    }
                    Put::Data(bytes) => {
                        value_table.push(kind::DATA);
                        varint(&mut value_table, bytes.len());
                        value_table.extend_from_slice(bytes);
                    }
                    Put::Nil => value_table.push(kind::NIL),
                    Put::Object(target) => {
                        value_table.push(kind::OBJECT);
                        value_table.extend_from_slice(&target.to_le_bytes());
                    }
                }
                value_count += 1;
            }
        }
        let mut key_table = Vec::new();
        for key in &keys {
            varint(&mut key_table, key.len());
            key_table.extend_from_slice(key.as_bytes());
        }
        let mut class_table = Vec::new();
        for class in classes {
            varint(&mut class_table, class.len() + 1);
            varint(&mut class_table, 0);
            class_table.extend_from_slice(class.as_bytes());
            class_table.push(0);
        }
        let mut bytes = MAGIC.to_vec();
        let header_at = bytes.len();
        bytes.resize(HEADER_BYTES, 0);
        let mut words = [FORMAT_VERSION, 10, 0, 0, 0, 0, 0, 0, 0, 0];
        for (index, (table, count)) in [
            (&object_table, objects.len()),
            (&key_table, keys.len()),
            (&value_table, value_count),
            (&class_table, classes.len()),
        ]
        .into_iter()
        .enumerate()
        {
            words[2 + 2 * index] = count as u32;
            words[3 + 2 * index] = bytes.len() as u32;
            bytes.extend_from_slice(table);
        }
        for (index, word) in words.iter().enumerate() {
            bytes[header_at + index * 4..header_at + index * 4 + 4]
                .copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    fn with_archive<R>(bytes: &[u8], work: impl FnOnce(NibArchive<'_, '_>) -> R) -> Result<R, NibError> {
        let counts = counts(bytes)?;
        let mut objects = vec![ObjectEntry::default(); counts.objects];
        let mut keys = vec![Span::default(); counts.keys];
        let mut values = vec![ValueEntry::default(); counts.values];
        let mut classes = vec![ClassEntry::default(); counts.classes];
        let archive = NibArchive::index(
            bytes,
            Tables {
                objects: &mut objects,
                keys: &mut keys,
                values: &mut values,
                classes: &mut classes,
            },
        )?;
        Ok(work(archive))
    }

    fn sample() -> Vec<u8> {
        let classes = [
            "NSObject",
            "NSIBObjectData",
            "NSCustomObject",
            "NSString",
            "NSArray",
            "NSNibOutletConnector",
            "NSNibControlConnector",
            "NSWindowTemplate",
        ];
        let string = |text: &'static str| (3, vec![("NS.bytes", Put::Data(text.as_bytes()))]);
        write(
            &classes,
            &[
                (0, vec![("IB.objectdata", Put::Object(1)), ("IB.systemFontUpdateVersion", Put::Int(1))]),
                (
                    1,
                    vec![
                        ("NSRoot", Put::Object(2)),
                        ("NSConnections", Put::Object(5)),
                        ("NSObjectsKeys", Put::Object(10)),
                        ("NSObjectsValues", Put::Object(11)),
                    ],
                ),
                (2, vec![("NSClassName", Put::Object(3))]),
                string("NSApplication"),
                (2, vec![("NSClassName", Put::Object(9))]),
                (4, vec![("NSInlinedValue", Put::Bool(false)), ("UINibEncoderEmptyKey", Put::Object(6)), ("UINibEncoderEmptyKey", Put::Object(8))]),
                (5, vec![("NSSource", Put::Object(2)), ("NSDestination", Put::Object(4)), ("NSLabel", Put::Object(7))]),
                string("delegate"),
                (6, vec![("NSSource", Put::Object(12)), ("NSLabel", Put::Object(13)), ("NSDouble", Put::Double(0.5)), ("NSNothing", Put::Nil)]),
                string("NibDelegate"),
                (4, vec![("UINibEncoderEmptyKey", Put::Object(4)), ("UINibEncoderEmptyKey", Put::Object(12))]),
                (4, vec![("UINibEncoderEmptyKey", Put::Object(2)), ("UINibEncoderEmptyKey", Put::Object(2))]),
                (7, vec![("NSWindowRect", Put::Object(14)), ("NSWindowStyleMask", Put::Int(7))]),
                string("terminate:"),
                string("{{200, 300}, {320, 120}}"),
            ],
        )
    }

    #[test]
    fn a_nib_s_objects_fields_connections_and_top_level_objects_read_back() {
        let bytes = sample();
        with_archive(&bytes, |archive| {
            assert_eq!(archive.object_count(), 15);
            let data = archive.object_data().unwrap();
            assert_eq!(data.root, 2);
            assert_eq!(archive.string(data.root, b"NSClassName"), Some(&b"NSApplication"[..]));
            assert_eq!(archive.integer(0, b"IB.systemFontUpdateVersion"), Some(1));
            assert_eq!(
                archive.connection(6),
                Some(Connection::Outlet { source: 2, destination: 4, name: b"delegate" })
            );
            assert_eq!(
                archive.connection(8),
                Some(Connection::Action { source: 12, destination: None, action: b"terminate:" })
            );
            assert_eq!(archive.field(8, b"NSDouble"), Some(NibValue::Real(0.5)));
            assert_eq!(archive.field(8, b"NSNothing"), Some(NibValue::Nil));
            assert_eq!(archive.field(5, b"NSInlinedValue"), Some(NibValue::Boolean(false)));
            assert_eq!(archive.top_level_objects(&data).collect::<Vec<_>>(), [4, 12]);
            assert_eq!(archive.class_name(12), Some(&b"NSWindowTemplate"[..]));
            let rect = archive.string(12, b"NSWindowRect").unwrap();
            assert_eq!(numbers::<4>(rect), Some([200.0, 300.0, 320.0, 120.0]));
            assert_eq!(numbers::<2>(b"{320, 120}"), Some([320.0, 120.0]));
            assert_eq!(numbers::<2>(rect), None);
            assert_eq!(archive.string_bytes(12), None);
        })
        .unwrap();
    }

    #[test]
    fn a_damaged_or_oversized_nib_is_refused_never_a_panic() {
        let bytes = sample();
        assert_eq!(counts(b"NIBArchiv").err(), Some(NibError::NotANib));
        let mut version = bytes.clone();
        version[MAGIC.len()] = 2;
        assert_eq!(counts(&version).err(), Some(NibError::UnknownVersion));
        let mut huge = bytes.clone();
        huge[MAGIC.len() + 8..MAGIC.len() + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(counts(&huge).err(), Some(NibError::TooLarge));
        for cut in 0..bytes.len() {
            let _ = with_archive(&bytes[..cut], |archive| archive.object_data());
        }
        for flip in MAGIC.len()..bytes.len() {
            for mask in [0x01, 0x80, 0xff] {
                let mut damaged = bytes.clone();
                damaged[flip] ^= mask;
                let _ = with_archive(&damaged, |archive| {
                    let data = archive.object_data();
                    for object in 0..archive.object_count() {
                        let _ = archive.connection(object);
                        let _ = archive.class_names(object).count();
                        let _ = archive.fields(object).count();
                    }
                    data.map(|data| archive.top_level_objects(&data).count())
                });
            }
        }
        let dangling = write(&["NSObject"], &[(0, vec![("x", Put::Object(9))])]);
        assert_eq!(with_archive(&dangling, |_| ()).err(), Some(NibError::Damaged));
    }

    #[test]
    fn a_nib_name_loses_its_extension_and_is_looked_for_in_the_resources_first() {
        assert_eq!(nib_stem(b"MainMenu"), b"MainMenu");
        assert_eq!(nib_stem(b"MainMenu.nib"), b"MainMenu");
        assert_eq!(nib_stem(b".nib"), b"");
        assert_eq!(NIB_FOLDERS[0], b"/Contents/Resources/");
        assert_eq!(NIB_SUFFIXES[1], b".nib");
    }

    #[test]
    fn a_nib_ibtool_compiles_from_our_xib_reads_back() {
        let xib = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/macos-nib/MainMenu.xib");
        let output = std::env::temp_dir().join(format!("eggos-mac-nib-{}.nib", std::process::id()));
        let compiled = std::process::Command::new("xcrun")
            .args(["ibtool", "--compile"])
            .arg(&output)
            .arg(xib)
            .status()
            .is_ok_and(|status| status.success());
        if !compiled {
            assert!(!cfg!(target_os = "macos") || std::env::var_os("CI").is_none());
            return;
        }
        let bytes = std::fs::read(&output).unwrap();
        with_archive(&bytes, |archive| {
            let data = archive.object_data().unwrap();
            assert_eq!(archive.string(data.root, b"NSClassName"), Some(&b"NSApplication"[..]));
            let top: Vec<_> = archive
                .top_level_objects(&data)
                .filter_map(|object| archive.class_name(object))
                .collect();
            assert!(top.contains(&&b"NSWindowTemplate"[..]));
            assert!(top.contains(&&b"NSMenu"[..]));
            let field = (0..archive.object_count())
                .find(|&object| archive.class_name(object) == Some(&b"NSTextField"[..]))
                .unwrap();
            assert_eq!(archive.field(field, b"NSEnabled"), Some(NibValue::Boolean(true)));
            let connections: Vec<_> = archive
                .members(data.connections.unwrap())
                .filter_map(|member| match member {
                    NibValue::Object(object) => archive.connection(object),
                    _ => None,
                })
                .collect();
            assert!(connections.iter().any(|connection| matches!(
                connection,
                Connection::Outlet { name: b"delegate", .. }
            )));
            assert!(connections.iter().any(|connection| matches!(
                connection,
                Connection::Action { action: b"greet:", destination: Some(_), .. }
            )));
            assert!(connections.iter().any(|connection| matches!(
                connection,
                Connection::Action { action: b"terminate:", destination: None, .. }
            )));
        })
        .unwrap();
    }
}
