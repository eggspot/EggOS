//! The messages between a program that uses a COM class out of process and the program that
//! serves it (D400), over a Unix stream socket: each message is its length (a little-endian
//! `u32`, the rest's bytes), its kind and its fields, little-endian. Automation is what
//! crosses: creating an object, `IDispatch::GetIDsOfNames`, `IDispatch::Invoke` with its
//! arguments, result and exception, and releasing an object. Values are VARIANTs' contents;
//! an object inside one travels as the number its owner gave it. Arrays travel with their
//! element kind, bounds and elements, and arguments passed by reference come back changed in
//! the answer (D743). Either end may serve objects (D744): an object the sender serves travels
//! as an object, one the receiver serves (a proxy handed back) as a peer object, and either
//! end may send requests while it waits for an answer, which carries the reply flag. Queries
//! for other interfaces and the connection-point calls `FindConnectionPoint`, `Advise` and
//! `Unadvise` have requests of their own. A call of a custom interface's vtable slot travels
//! with each argument's value and how the method takes it (D746).

use super::HResult;
use alloc::vec::Vec;

/// The longest message either side sends or accepts.
pub const MOST_MESSAGE: usize = 1 << 20;
/// The most elements one array carries, and dimensions it has.
pub const MOST_ELEMENTS: usize = 1 << 16;
pub const MOST_DIMENSIONS: usize = 8;
/// How deep arrays of VARIANTs holding arrays may nest.
pub const MOST_NESTING: usize = 4;
/// The most names one `GetIDsOfNames` asks for, and arguments one `Invoke` passes.
pub const MOST_ITEMS: usize = 64;
const LENGTH_BYTES: usize = 4;

/// The kinds of request.
pub mod request {
    pub const CREATE: u8 = 1;
    pub const NAMES: u8 = 2;
    pub const INVOKE: u8 = 3;
    pub const RELEASE: u8 = 4;
    pub const QUERY: u8 = 5;
    pub const FIND_POINT: u8 = 6;
    pub const ADVISE: u8 = 7;
    pub const UNADVISE: u8 = 8;
    pub const CALL: u8 = 9;
    /// Set in the kind of every answer.
    pub const REPLY: u8 = 0x80;
}

/// The kinds of value.
pub mod value_kind {
    pub const PLAIN: u8 = 0;
    pub const DECIMAL: u8 = 1;
    pub const TEXT: u8 = 2;
    pub const OBJECT: u8 = 3;
    pub const ARRAY: u8 = 4;
    pub const PEER: u8 = 5;
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
    /// A `VT_DISPATCH` or `VT_UNKNOWN` the sender serves: its number for it, 0 for null.
    Object { kind: u16, id: u32 },
    /// A `VT_DISPATCH` or `VT_UNKNOWN` the receiver serves, by the receiver's number.
    Peer { kind: u16, id: u32 },
    /// A `VT_ARRAY`: its element kind, each dimension's element count and lowest index
    /// (leftmost first), and its elements in the array's data order.
    Array {
        element: u16,
        bounds: Vec<(u32, i32)>,
        items: Vec<Value>,
    },
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
        by_reference: Vec<Reference>,
    },
    Release {
        object: u32,
    },
    Query {
        object: u32,
        interface: [u8; 16],
    },
    FindPoint {
        object: u32,
        interface: [u8; 16],
    },
    Advise {
        object: u32,
        sink: Value,
    },
    Unadvise {
        object: u32,
        cookie: u32,
    },
    Call {
        object: u32,
        slot: u16,
        arguments: Vec<Argument>,
    },
}

/// How a vtable method takes one argument (D746).
pub mod passing {
    /// The value itself in the argument's word.
    pub const WORD: u8 = 0;
    /// A pointer to a VARIANT holding it (a `VARIANT` passed by value).
    pub const VARIANT: u8 = 1;
    /// A pointer to a place of the kind holding it, whose value comes back after the call.
    pub const PLACE: u8 = 2;
}

/// One argument of a vtable call: how it is passed, its kind and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub passing: u8,
    pub kind: u16,
    pub value: Value,
}

/// An argument passed by reference: its index among the arguments and the kind it points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reference {
    pub index: u16,
    pub kind: u16,
}

/// The answer to `Invoke`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoked {
    pub status: HResult,
    pub result: Value,
    pub argument_error: u32,
    pub exception: Option<Exception>,
    /// The values the arguments passed by reference hold after the call, in their order.
    pub changed: Vec<Value>,
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
            Value::Peer { kind, id } => self.u8(value_kind::PEER).u16(*kind).u32(*id),
            Value::Array {
                element,
                bounds,
                items,
            } => {
                self.u8(value_kind::ARRAY).u16(*element).u8(bounds.len() as u8);
                for (elements, lower) in bounds {
                    self.u32(*elements).i32(*lower);
                }
                self.u32(items.len() as u32);
                for item in items {
                    self.value(item);
                }
                self
            }
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
        self.nested_value(0)
    }

    fn array_value(&mut self, depth: usize) -> Option<Value> {
        let element = self.u16()?;
        let dimensions = usize::from(self.u8()?);
        if dimensions == 0 || dimensions > MOST_DIMENSIONS || depth >= MOST_NESTING {
            return None;
        }
        let bounds = (0..dimensions)
            .map(|_| Some((self.u32()?, self.i32()?)))
            .collect::<Option<Vec<_>>>()?;
        let total = self.u32()? as usize;
        let expected = bounds
            .iter()
            .try_fold(1usize, |product, &(elements, _)| {
                product.checked_mul(elements as usize)
            })?;
        if total != expected || total > MOST_ELEMENTS {
            return None;
        }
        let items = (0..total)
            .map(|_| self.nested_value(depth + 1))
            .collect::<Option<Vec<_>>>()?;
        Some(Value::Array {
            element,
            bounds,
            items,
        })
    }

    fn nested_value(&mut self, depth: usize) -> Option<Value> {
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
            value_kind::PEER => Value::Peer {
                kind: self.u16()?,
                id: self.u32()?,
            },
            value_kind::ARRAY => self.array_value(depth)?,
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

/// The message calling member `member` of object `object`; `by_reference` names the
/// arguments whose changed values should come back.
pub fn invoke(
    object: u32,
    (member, locale, flags): (i32, u32, u16),
    arguments: &[Value],
    named: &[i32],
    by_reference: &[Reference],
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
    writer.u16(by_reference.len() as u16);
    for reference in by_reference {
        writer.u16(reference.index).u16(reference.kind);
    }
    writer.finish()
}

/// The message releasing object `object`.
pub fn release(object: u32) -> Vec<u8> {
    let mut writer = Writer::new(request::RELEASE);
    writer.u32(object);
    writer.finish()
}

/// The message asking object `object` for `interface`.
pub fn query(object: u32, interface: [u8; 16]) -> Vec<u8> {
    let mut writer = Writer::new(request::QUERY);
    writer.u32(object).bytes(&interface);
    writer.finish()
}

/// The message asking the connection point container `object` for `interface`'s point.
pub fn find_point(object: u32, interface: [u8; 16]) -> Vec<u8> {
    let mut writer = Writer::new(request::FIND_POINT);
    writer.u32(object).bytes(&interface);
    writer.finish()
}

/// The message connecting `sink` to the connection point `object`.
pub fn advise(object: u32, sink: &Value) -> Vec<u8> {
    let mut writer = Writer::new(request::ADVISE);
    writer.u32(object).value(sink);
    writer.finish()
}

/// The message ending connection `cookie` of the connection point `object`.
pub fn unadvise(object: u32, cookie: u32) -> Vec<u8> {
    let mut writer = Writer::new(request::UNADVISE);
    writer.u32(object).u32(cookie);
    writer.finish()
}

/// The message calling vtable slot `slot` of object `object` with `arguments`.
pub fn call(object: u32, slot: u16, arguments: &[Argument]) -> Vec<u8> {
    let mut writer = Writer::new(request::CALL);
    writer.u32(object).u16(slot).u16(arguments.len() as u16);
    for argument in arguments {
        writer.u8(argument.passing).u16(argument.kind).value(&argument.value);
    }
    writer.finish()
}

/// The answer to `Call`: the method's status and the values its places hold after it.
pub fn called(status: HResult, changed: &[Value]) -> Vec<u8> {
    let mut writer = reply(request::CALL);
    writer.i32(status).u16(changed.len() as u16);
    for value in changed {
        writer.value(value);
    }
    writer.finish()
}

/// Reads [`called`]'s answer.
pub fn read_called(body: &[u8]) -> Option<(HResult, Vec<Value>)> {
    let mut reader = reading_reply(body, request::CALL)?;
    let status = reader.i32()?;
    let total = count(&mut reader)?;
    let changed = (0..total)
        .map(|_| reader.value())
        .collect::<Option<Vec<_>>>()?;
    reader.ended().then_some((status, changed))
}

/// Whether the message body `body` is an answer rather than a request.
pub fn is_reply(body: &[u8]) -> bool {
    body.first().is_some_and(|kind| kind & request::REPLY != 0)
}

fn reply(kind: u8) -> Writer {
    Writer::new(kind | request::REPLY)
}

fn reading_reply<'a>(body: &'a [u8], kind: u8) -> Option<Reader<'a>> {
    let mut reader = Reader::new(body);
    (reader.u8()? == kind | request::REPLY).then_some(reader)
}

/// The answer to `Query`, `FindPoint` or `Advise`: its status, a number (the new object's, or
/// the connection's cookie) and whether the object answers `IDispatch` at the same address.
pub fn numbered(kind: u8, status: HResult, number: u32, dispatch: bool) -> Vec<u8> {
    let mut writer = reply(kind);
    writer.i32(status).u32(number).u8(u8::from(dispatch));
    writer.finish()
}

/// Reads [`numbered`]'s answer of `kind`.
pub fn read_numbered(body: &[u8], kind: u8) -> Option<(HResult, u32, bool)> {
    let mut reader = reading_reply(body, kind)?;
    let answer = (reader.i32()?, reader.u32()?, reader.u8()? != 0);
    reader.ended().then_some(answer)
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
            let reference_total = count(&mut reader)?;
            let by_reference = (0..reference_total)
                .map(|_| {
                    let index = reader.u16()?;
                    let kind = reader.u16()?;
                    (usize::from(index) < arguments.len()).then_some(Reference { index, kind })
                })
                .collect::<Option<Vec<_>>>()?;
            Request::Invoke {
                object,
                member,
                locale,
                flags,
                arguments,
                named,
                by_reference,
            }
        }
        request::RELEASE => Request::Release {
            object: reader.u32()?,
        },
        request::QUERY => Request::Query {
            object: reader.u32()?,
            interface: reader.array()?,
        },
        request::FIND_POINT => Request::FindPoint {
            object: reader.u32()?,
            interface: reader.array()?,
        },
        request::ADVISE => Request::Advise {
            object: reader.u32()?,
            sink: reader.value()?,
        },
        request::UNADVISE => Request::Unadvise {
            object: reader.u32()?,
            cookie: reader.u32()?,
        },
        request::CALL => {
            let object = reader.u32()?;
            let slot = reader.u16()?;
            let total = count(&mut reader)?;
            let arguments = (0..total)
                .map(|_| {
                    let how = reader.u8()?;
                    let kind = reader.u16()?;
                    let value = reader.value()?;
                    (how <= passing::PLACE).then_some(Argument {
                        passing: how,
                        kind,
                        value,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Request::Call {
                object,
                slot,
                arguments,
            }
        }
        _ => return None,
    };
    reader.ended().then_some(request)
}

/// The answer to `Create`: its status and the new object's number.
pub fn created(status: HResult, object: u32) -> Vec<u8> {
    let mut writer = reply(request::CREATE);
    writer.i32(status).u32(object);
    writer.finish()
}

/// Reads [`created`]'s answer.
pub fn read_created(body: &[u8]) -> Option<(HResult, u32)> {
    let mut reader = reading_reply(body, request::CREATE)?;
    let answer = (reader.i32()?, reader.u32()?);
    reader.ended().then_some(answer)
}

/// The answer to `Names`: its status and the numbers.
pub fn named(status: HResult, numbers: &[i32]) -> Vec<u8> {
    let mut writer = reply(request::NAMES);
    writer.i32(status).u16(numbers.len() as u16);
    for number in numbers {
        writer.i32(*number);
    }
    writer.finish()
}

/// Reads [`named`]'s answer.
pub fn read_named(body: &[u8]) -> Option<(HResult, Vec<i32>)> {
    let mut reader = reading_reply(body, request::NAMES)?;
    let status = reader.i32()?;
    let total = count(&mut reader)?;
    let numbers = (0..total)
        .map(|_| reader.i32())
        .collect::<Option<Vec<_>>>()?;
    reader.ended().then_some((status, numbers))
}

/// The answer to `Invoke`.
pub fn invoked(answer: &Invoked) -> Vec<u8> {
    let mut writer = reply(request::INVOKE);
    writer
        .i32(answer.status)
        .value(&answer.result)
        .u32(answer.argument_error);
    match &answer.exception {
        Some(exception) => writer.u8(1).exception(exception),
        None => writer.u8(0),
    };
    writer.u16(answer.changed.len() as u16);
    for value in &answer.changed {
        writer.value(value);
    }
    writer.finish()
}

/// Reads [`invoked`]'s answer.
pub fn read_invoked(body: &[u8]) -> Option<Invoked> {
    let mut reader = reading_reply(body, request::INVOKE)?;
    let status = reader.i32()?;
    let result = reader.value()?;
    let argument_error = reader.u32()?;
    let exception = match reader.u8()? {
        0 => None,
        1 => Some(reader.exception()?),
        _ => return None,
    };
    let changed_total = count(&mut reader)?;
    let changed = (0..changed_total)
        .map(|_| reader.value())
        .collect::<Option<Vec<_>>>()?;
    reader.ended().then_some(Invoked {
        status,
        result,
        argument_error,
        exception,
        changed,
    })
}

/// The answer to `Release`: its status.
pub fn released(status: HResult) -> Vec<u8> {
    status_reply(request::RELEASE, status)
}

/// Reads [`released`]'s answer.
pub fn read_released(body: &[u8]) -> Option<HResult> {
    read_status(body, request::RELEASE)
}

/// An answer of `kind` holding only a status (`Release`, `Unadvise`).
pub fn status_reply(kind: u8, status: HResult) -> Vec<u8> {
    let mut writer = reply(kind);
    writer.i32(status);
    writer.finish()
}

/// Reads [`status_reply`]'s answer of `kind`.
pub fn read_status(body: &[u8], kind: u8) -> Option<HResult> {
    let mut reader = reading_reply(body, kind)?;
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
        let by_reference = [Reference { index: 0, kind: 3 }];
        let message = invoke(2, (7, 0, 1), &arguments, &[-3], &by_reference);
        assert_eq!(
            read_request(body(&message)),
            Some(Request::Invoke {
                object: 2,
                member: 7,
                locale: 0,
                flags: 1,
                arguments,
                named: vec![-3],
                by_reference: by_reference.to_vec(),
            })
        );
    }

    #[test]
    fn arrays_travel_with_their_bounds_and_nest() {
        let inner = Value::Array {
            element: 3,
            bounds: vec![(2, 0)],
            items: vec![Value::Plain { kind: 3, word: 1 }, Value::Plain { kind: 3, word: 2 }],
        };
        let outer = Value::Array {
            element: 12,
            bounds: vec![(1, 1), (2, -1)],
            items: vec![inner.clone(), Value::Text(None)],
        };
        let message = invoke(1, (0, 0, 1), core::slice::from_ref(&outer), &[], &[]);
        let Some(Request::Invoke { arguments, .. }) = read_request(body(&message)) else {
            panic!("the request did not read back");
        };
        assert_eq!(arguments, vec![outer]);
        let mut wrong = Writer::new(request::INVOKE);
        wrong.u32(1).i32(0).u32(0).u16(1).u16(1);
        wrong.u8(value_kind::ARRAY).u16(3).u8(1).u32(3).i32(0).u32(2);
        assert_eq!(read_request(&wrong.finish()[4..]), None);
    }

    #[test]
    fn changed_references_come_back_in_the_answer() {
        let answer = Invoked {
            status: 0,
            result: Value::Plain { kind: 0, word: 0 },
            argument_error: 0,
            exception: None,
            changed: vec![Value::Plain { kind: 3, word: 9 }, Value::Text(Some(vec![65]))],
        };
        assert_eq!(read_invoked(body(&invoked(&answer))), Some(answer));
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
            changed: Vec::new(),
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
        assert!(is_reply(&created(0, 9)[4..]));
        assert!(!is_reply(&create([0; 16])[4..]));
    }

    #[test]
    fn vtable_calls_round_trip() {
        let arguments = vec![
            Argument {
                passing: passing::WORD,
                kind: 3,
                value: Value::Plain { kind: 3, word: 7 },
            },
            Argument {
                passing: passing::PLACE,
                kind: 8,
                value: Value::Text(None),
            },
        ];
        assert_eq!(
            read_request(body(&call(2, 9, &arguments))),
            Some(Request::Call {
                object: 2,
                slot: 9,
                arguments,
            })
        );
        let changed = vec![Value::Text(Some(vec![104, 105]))];
        assert_eq!(read_called(body(&called(0, &changed))), Some((0, changed)));
    }

    #[test]
    fn connection_point_requests_and_answers_round_trip() {
        let interface = [3u8; 16];
        assert_eq!(
            read_request(body(&query(4, interface))),
            Some(Request::Query {
                object: 4,
                interface
            })
        );
        assert_eq!(
            read_request(body(&find_point(4, interface))),
            Some(Request::FindPoint {
                object: 4,
                interface
            })
        );
        let sink = Value::Object { kind: 9, id: 2 };
        assert_eq!(
            read_request(body(&advise(5, &sink))),
            Some(Request::Advise { object: 5, sink })
        );
        assert_eq!(
            read_request(body(&unadvise(5, 7))),
            Some(Request::Unadvise {
                object: 5,
                cookie: 7
            })
        );
        let answer = numbered(request::ADVISE, 0, 7, false);
        assert_eq!(read_numbered(body(&answer), request::ADVISE), Some((0, 7, false)));
        assert_eq!(read_numbered(body(&answer), request::QUERY), None);
        let peer = Value::Peer { kind: 13, id: 8 };
        let message = invoke(1, (0, 0, 1), core::slice::from_ref(&peer), &[], &[]);
        let Some(Request::Invoke { arguments, .. }) = read_request(body(&message)) else {
            panic!("the request did not read back");
        };
        assert_eq!(arguments, vec![peer]);
    }
}
