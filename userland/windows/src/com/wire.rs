//! The messages between a program that uses a COM class out of process and the program that
//! serves it (D400), over a Unix stream socket: each message is its length (a little-endian
//! `u32`, the rest's bytes), its kind and its fields, little-endian. Automation is what
//! crosses: creating an object, `IDispatch::GetIDsOfNames`, `IDispatch::Invoke` with its
//! arguments, result and exception, and releasing an object. Values are VARIANTs' contents;
//! an object inside one travels as the number the serving side gave it.

use super::HResult;
use alloc::vec::Vec;

/// The longest message either side sends or accepts.
pub const MOST_MESSAGE: usize = 1 << 20;
/// The most names one `GetIDsOfNames` asks for, and arguments one `Invoke` passes.
pub const MOST_ITEMS: usize = 64;
const LENGTH_BYTES: usize = 4;

/// The kinds of request.
pub mod request {
    pub const CREATE: u8 = 1;
    pub const NAMES: u8 = 2;
    pub const INVOKE: u8 = 3;
    pub const RELEASE: u8 = 4;
}

/// The kinds of value.
pub mod value_kind {
    pub const PLAIN: u8 = 0;
    pub const DECIMAL: u8 = 1;
    pub const TEXT: u8 = 2;
    pub const OBJECT: u8 = 3;
}

/// A VARIANT's content as it travels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A value whose eight bytes are its whole content (`VT_EMPTY`, numbers, dates, `VT_BOOL`).
    Plain { kind: u16, word: u64 },
    /// A `VT_DECIMAL`'s sixteen bytes, the VARIANT's whole.
    Decimal([u8; 16]),
    /// A `VT_BSTR`'s text; `None` for a null BSTR.
    Text(Option<Vec<u16>>),
    /// A `VT_DISPATCH` or `VT_UNKNOWN`: the serving side's number for it, 0 for null.
    Object { kind: u16, id: u32 },
}

/// An `EXCEPINFO`'s content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exception {
    pub code: u16,
    pub source: Vec<u16>,
    pub description: Vec<u16>,
    pub help_file: Vec<u16>,
    pub help_context: u32,
    pub status: HResult,
}

/// A request, as the serving side reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Create {
        class: [u8; 16],
    },
    Names {
        object: u32,
        locale: u32,
        names: Vec<Vec<u16>>,
    },
    Invoke {
        object: u32,
        member: i32,
        locale: u32,
        flags: u16,
        arguments: Vec<Value>,
        named: Vec<i32>,
    },
    Release {
        object: u32,
    },
}

/// The answer to `Invoke`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoked {
    pub status: HResult,
    pub result: Value,
    pub argument_error: u32,
    pub exception: Option<Exception>,
}

/// Writes a message.
#[derive(Debug, Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// A message of `kind`.
    pub fn new(kind: u8) -> Self {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0; LENGTH_BYTES]);
        bytes.push(kind);
        Self { bytes }
    }

    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.bytes.push(value);
        self
    }

    pub fn u16(&mut self, value: u16) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn u32(&mut self, value: u32) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn i32(&mut self, value: i32) -> &mut Self {
        self.u32(value as u32)
    }

    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(value);
        self
    }

    pub fn text(&mut self, units: &[u16]) -> &mut Self {
        self.u32(units.len() as u32);
        for unit in units {
            self.u16(*unit);
        }
        self
    }

    pub fn value(&mut self, value: &Value) -> &mut Self {
        match value {
            Value::Plain { kind, word } => self.u8(value_kind::PLAIN).u16(*kind).u64(*word),
            Value::Decimal(bytes) => self.u8(value_kind::DECIMAL).bytes(bytes),
            Value::Text(None) => self.u8(value_kind::TEXT).u32(u32::MAX),
            Value::Text(Some(units)) => self.u8(value_kind::TEXT).text(units),
            Value::Object { kind, id } => self.u8(value_kind::OBJECT).u16(*kind).u32(*id),
        }
    }

    pub fn exception(&mut self, exception: &Exception) -> &mut Self {
        self.u16(exception.code)
            .text(&exception.source)
            .text(&exception.description)
            .text(&exception.help_file)
            .u32(exception.help_context)
            .i32(exception.status)
    }

    /// The finished message, its length in front.
    pub fn finish(mut self) -> Vec<u8> {
        let length = (self.bytes.len() - LENGTH_BYTES) as u32;
        self.bytes[..LENGTH_BYTES].copy_from_slice(&length.to_le_bytes());
        self.bytes
    }
}

/// Reads a message's body (its kind and fields, without its length).
#[derive(Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let taken = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(taken)
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn i32(&mut self) -> Option<i32> {
        self.u32().map(|value| value as i32)
    }

    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn units(&mut self, count: usize) -> Option<Vec<u16>> {
        let bytes = self.take(count.checked_mul(2)?)?;
        Some(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect(),
        )
    }

    pub fn text(&mut self) -> Option<Vec<u16>> {
        let count = self.u32()? as usize;
        self.units(count)
    }

    pub fn value(&mut self) -> Option<Value> {
        Some(match self.u8()? {
            value_kind::PLAIN => Value::Plain {
                kind: self.u16()?,
                word: self.u64()?,
            },
            value_kind::DECIMAL => Value::Decimal(self.array()?),
            value_kind::TEXT => match self.u32()? {
                u32::MAX => Value::Text(None),
                count => Value::Text(Some(self.units(count as usize)?)),
            },
            value_kind::OBJECT => Value::Object {
                kind: self.u16()?,
                id: self.u32()?,
            },
            _ => return None,
        })
    }

    pub fn exception(&mut self) -> Option<Exception> {
        Some(Exception {
            code: self.u16()?,
            source: self.text()?,
            description: self.text()?,
            help_file: self.text()?,
            help_context: self.u32()?,
            status: self.i32()?,
        })
    }

    /// Whether every byte was read.
    pub fn ended(&self) -> bool {
        self.at == self.bytes.len()
    }
}

/// The message asking to create an object of `class`.
pub fn create(class: [u8; 16]) -> Vec<u8> {
    let mut writer = Writer::new(request::CREATE);
    writer.bytes(&class);
    writer.finish()
}

/// The message asking object `object` for the numbers of `names`.
pub fn names(object: u32, locale: u32, names: &[&[u16]]) -> Vec<u8> {
    let mut writer = Writer::new(request::NAMES);
    writer.u32(object).u32(locale).u16(names.len() as u16);
    for name in names {
        writer.text(name);
    }
    writer.finish()
}

/// The message calling member `member` of object `object`.
pub fn invoke(
    object: u32,
    (member, locale, flags): (i32, u32, u16),
    arguments: &[Value],
    named: &[i32],
) -> Vec<u8> {
    let mut writer = Writer::new(request::INVOKE);
    writer
        .u32(object)
        .i32(member)
        .u32(locale)
        .u16(flags)
        .u16(arguments.len() as u16);
    for argument in arguments {
        writer.value(argument);
    }
    writer.u16(named.len() as u16);
    for number in named {
        writer.i32(*number);
    }
    writer.finish()
}

/// The message releasing object `object`.
pub fn release(object: u32) -> Vec<u8> {
    let mut writer = Writer::new(request::RELEASE);
    writer.u32(object);
    writer.finish()
}

fn count(reader: &mut Reader<'_>) -> Option<usize> {
    let count = usize::from(reader.u16()?);
    (count <= MOST_ITEMS).then_some(count)
}

/// The request in the message body `body`; `None` for a malformed one.
pub fn read_request(body: &[u8]) -> Option<Request> {
    let mut reader = Reader::new(body);
    let request = match reader.u8()? {
        request::CREATE => Request::Create {
            class: reader.array()?,
        },
        request::NAMES => {
            let object = reader.u32()?;
            let locale = reader.u32()?;
            let total = count(&mut reader)?;
            let names = (0..total)
                .map(|_| reader.text())
                .collect::<Option<Vec<_>>>()?;
            Request::Names {
                object,
                locale,
                names,
            }
        }
        request::INVOKE => {
            let object = reader.u32()?;
            let member = reader.i32()?;
            let locale = reader.u32()?;
            let flags = reader.u16()?;
            let total = count(&mut reader)?;
            let arguments = (0..total)
                .map(|_| reader.value())
                .collect::<Option<Vec<_>>>()?;
            let named_total = count(&mut reader)?;
            let named = (0..named_total)
                .map(|_| reader.i32())
                .collect::<Option<Vec<_>>>()?;
            Request::Invoke {
                object,
                member,
                locale,
                flags,
                arguments,
                named,
            }
        }
        request::RELEASE => Request::Release {
            object: reader.u32()?,
        },
        _ => return None,
    };
    reader.ended().then_some(request)
}

/// The answer to `Create`: its status and the new object's number.
pub fn created(status: HResult, object: u32) -> Vec<u8> {
    let mut writer = Writer::new(request::CREATE);
    writer.i32(status).u32(object);
    writer.finish()
}

/// Reads [`created`]'s answer.
pub fn read_created(body: &[u8]) -> Option<(HResult, u32)> {
    let mut reader = Reader::new(body);
    (reader.u8()? == request::CREATE).then_some(())?;
    let answer = (reader.i32()?, reader.u32()?);
    reader.ended().then_some(answer)
}

/// The answer to `Names`: its status and the numbers.
pub fn named(status: HResult, numbers: &[i32]) -> Vec<u8> {
    let mut writer = Writer::new(request::NAMES);
    writer.i32(status).u16(numbers.len() as u16);
    for number in numbers {
        writer.i32(*number);
    }
    writer.finish()
}

/// Reads [`named`]'s answer.
pub fn read_named(body: &[u8]) -> Option<(HResult, Vec<i32>)> {
    let mut reader = Reader::new(body);
    (reader.u8()? == request::NAMES).then_some(())?;
    let status = reader.i32()?;
    let total = count(&mut reader)?;
    let numbers = (0..total)
        .map(|_| reader.i32())
        .collect::<Option<Vec<_>>>()?;
    reader.ended().then_some((status, numbers))
}

/// The answer to `Invoke`.
pub fn invoked(answer: &Invoked) -> Vec<u8> {
    let mut writer = Writer::new(request::INVOKE);
    writer
        .i32(answer.status)
        .value(&answer.result)
        .u32(answer.argument_error);
    match &answer.exception {
        Some(exception) => writer.u8(1).exception(exception),
        None => writer.u8(0),
    };
    writer.finish()
}

/// Reads [`invoked`]'s answer.
pub fn read_invoked(body: &[u8]) -> Option<Invoked> {
    let mut reader = Reader::new(body);
    (reader.u8()? == request::INVOKE).then_some(())?;
    let status = reader.i32()?;
    let result = reader.value()?;
    let argument_error = reader.u32()?;
    let exception = match reader.u8()? {
        0 => None,
        1 => Some(reader.exception()?),
        _ => return None,
    };
    reader.ended().then_some(Invoked {
        status,
        result,
        argument_error,
        exception,
    })
}

/// The answer to `Release`: its status.
pub fn released(status: HResult) -> Vec<u8> {
    let mut writer = Writer::new(request::RELEASE);
    writer.i32(status);
    writer.finish()
}

/// Reads [`released`]'s answer.
pub fn read_released(body: &[u8]) -> Option<HResult> {
    let mut reader = Reader::new(body);
    (reader.u8()? == request::RELEASE).then_some(())?;
    let status = reader.i32()?;
    reader.ended().then_some(status)
}

/// The length a message's first four bytes give, if it is one either side accepts.
pub fn body_length(head: [u8; LENGTH_BYTES]) -> Option<usize> {
    let length = u32::from_le_bytes(head) as usize;
    (1..=MOST_MESSAGE).contains(&length).then_some(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(message: &[u8]) -> &[u8] {
        let length = body_length(message[..4].try_into().unwrap()).unwrap();
        assert_eq!(length, message.len() - 4);
        &message[4..]
    }

    #[test]
    fn a_create_request_carries_its_class() {
        let class = [7u8; 16];
        let message = create(class);
        assert_eq!(
            read_request(body(&message)),
            Some(Request::Create { class })
        );
    }

    #[test]
    fn names_travel_and_their_numbers_come_back() {
        let visible: Vec<u16> = "Visible".encode_utf16().collect();
        let message = names(3, 0x409, &[&visible]);
        assert_eq!(
            read_request(body(&message)),
            Some(Request::Names {
                object: 3,
                locale: 0x409,
                names: vec![visible],
            })
        );
        let answer = named(0, &[12]);
        assert_eq!(read_named(body(&answer)), Some((0, vec![12])));
    }

    #[test]
    fn an_invoke_carries_every_kind_of_value() {
        let arguments = vec![
            Value::Plain { kind: 3, word: 42 },
            Value::Decimal([1; 16]),
            Value::Text(Some("hi".encode_utf16().collect())),
            Value::Text(None),
            Value::Object { kind: 9, id: 5 },
        ];
        let message = invoke(2, (7, 0, 1), &arguments, &[-3]);
        assert_eq!(
            read_request(body(&message)),
            Some(Request::Invoke {
                object: 2,
                member: 7,
                locale: 0,
                flags: 1,
                arguments,
                named: vec![-3],
            })
        );
    }

    #[test]
    fn an_exception_comes_back_whole() {
        let answer = Invoked {
            status: 0x8002_0009_u32 as i32,
            result: Value::Plain { kind: 0, word: 0 },
            argument_error: 0,
            exception: Some(Exception {
                code: 1001,
                source: "Calc".encode_utf16().collect(),
                description: "Division by zero".encode_utf16().collect(),
                help_file: Vec::new(),
                help_context: 0,
                status: 0,
            }),
        };
        assert_eq!(read_invoked(body(&invoked(&answer))), Some(answer));
    }

    #[test]
    fn malformed_messages_are_refused() {
        assert_eq!(read_request(&[request::CREATE, 1, 2]), None);
        assert_eq!(read_request(&[99]), None);
        let mut long = create([0; 16]);
        long.push(0);
        assert_eq!(read_request(&long[4..]), None);
        assert_eq!(body_length([0, 0, 0, 0]), None);
        assert_eq!(body_length([0xff, 0xff, 0xff, 0xff]), None);
        let mut too_many = Writer::new(request::NAMES);
        too_many.u32(1).u32(0).u16((MOST_ITEMS + 1) as u16);
        assert_eq!(read_request(&too_many.finish()[4..]), None);
    }

    #[test]
    fn create_and_release_answers_round_trip() {
        assert_eq!(read_created(&created(0, 9)[4..]), Some((0, 9)));
        assert_eq!(read_released(&released(1)[4..]), Some(1));
    }
}
