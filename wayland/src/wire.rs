//! The Wayland wire format (`wayland.xml`'s "Wire Format" section): a message is a header of
//! two 32-bit words in the machine's byte order (the object it is for; its size in bytes,
//! header included, in the high 16 bits and its opcode in the low 16), then its arguments in
//! the order of its signature, each a multiple of four bytes. Descriptors travel beside the
//! bytes, in the socket's control data, in the order their messages name them.

/// The header's size in bytes.
pub const HEADER_SIZE: usize = 8;
/// The largest message, header included, that either side sends or takes.
pub const MAX_MESSAGE_SIZE: usize = 4096;
/// The most arguments a message of ours has.
pub const MAX_ARGUMENTS: usize = 8;

/// The type of one argument in a signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    /// A signed 32-bit integer.
    Int,
    /// An unsigned 32-bit integer.
    Uint,
    /// A signed 24.8 fixed-point number.
    Fixed,
    /// A string that may not be null.
    String,
    /// A string or null (a length of 0), decoded as [`Value::None`] when null.
    NullableString,
    /// An object's id that may not be null (0).
    Object,
    /// An object's id or 0.
    NullableObject,
    /// The id of an object the message creates, of the interface the request says.
    NewId,
    /// The id of an object the message creates, preceded by its interface's name and version
    /// (`wl_registry.bind`).
    UntypedNewId,
    /// A byte array.
    Array,
    /// A file descriptor, carried in the control data.
    Fd,
}

/// One decoded argument, or one to encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Value<'a> {
    /// No argument there.
    #[default]
    None,
    Int(i32),
    Uint(u32),
    /// The raw 24.8 fixed-point bits.
    Fixed(i32),
    /// A string's bytes without its terminating NUL.
    String(&'a [u8]),
    /// An object's id, 0 for null.
    Object(u32),
    NewId(u32),
    UntypedNewId {
        interface: &'a [u8],
        version: u32,
        id: u32,
    },
    Array(&'a [u8]),
    /// A descriptor: nothing in the bytes.
    Fd,
}

impl<'a> Value<'a> {
    /// The value as an unsigned integer: a `Uint`, an object's or a new object's id, an `Int`'s
    /// bits; 0 for anything else.
    pub fn uint(self) -> u32 {
        match self {
            Self::Uint(value) | Self::Object(value) | Self::NewId(value) => value,
            Self::Int(value) | Self::Fixed(value) => value as u32,
            Self::UntypedNewId { id, .. } => id,
            _ => 0,
        }
    }

    /// The value as a signed integer: an `Int`, a `Uint`'s bits; 0 for anything else.
    pub fn int(self) -> i32 {
        self.uint() as i32
    }

    /// The bytes of a string or an array; empty for anything else.
    pub fn bytes(self) -> &'a [u8] {
        match self {
            Self::String(bytes) | Self::Array(bytes) => bytes,
            Self::UntypedNewId { interface, .. } => interface,
            _ => &[],
        }
    }
}

/// Why bytes are no valid message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// A size below the header's, not a multiple of four, or above [`MAX_MESSAGE_SIZE`].
    BadSize,
    /// The arguments run past the message's end.
    Truncated,
    /// Bytes are left after the last argument.
    TrailingBytes,
    /// A string without its terminating NUL, or null where it may not be.
    BadString,
    /// A null object where the signature takes none.
    NullObject,
    /// More arguments than [`MAX_ARGUMENTS`].
    TooManyArguments,
    /// The message does not fit the buffer it is written into, or [`MAX_MESSAGE_SIZE`].
    TooLong,
}

/// A message's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The object the message is for.
    pub object: u32,
    /// The request's or event's opcode.
    pub opcode: u16,
    /// The whole message's size in bytes.
    pub size: usize,
}

fn word(bytes: &[u8], offset: usize) -> Option<u32> {
    let bytes = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

const fn padded(length: usize) -> usize {
    (length + 3) & !3
}

impl Header {
    /// The header at the start of `bytes`: `Ok(None)` when fewer than [`HEADER_SIZE`] bytes are
    /// there, [`WireError::BadSize`] for a size no message has.
    pub fn read(bytes: &[u8]) -> Result<Option<Self>, WireError> {
        let (Some(object), Some(second)) = (word(bytes, 0), word(bytes, 4)) else {
            return Ok(None);
        };
        let size = (second >> 16) as usize;
        if size < HEADER_SIZE || !size.is_multiple_of(4) || size > MAX_MESSAGE_SIZE {
            return Err(WireError::BadSize);
        }
        Ok(Some(Self {
            object,
            opcode: second as u16,
            size,
        }))
    }
}

fn next_word(body: &[u8], at: &mut usize) -> Result<u32, WireError> {
    let value = word(body, *at).ok_or(WireError::Truncated)?;
    *at += 4;
    Ok(value)
}

fn sized_bytes<'a>(body: &'a [u8], at: &mut usize) -> Result<(u32, &'a [u8]), WireError> {
    let length = word(body, *at).ok_or(WireError::Truncated)?;
    let start = *at + 4;
    let end = start
        .checked_add(padded(length as usize))
        .ok_or(WireError::Truncated)?;
    if end > body.len() {
        return Err(WireError::Truncated);
    }
    *at = end;
    Ok((length, &body[start..start + length as usize]))
}

fn string<'a>(body: &'a [u8], at: &mut usize) -> Result<&'a [u8], WireError> {
    let (length, bytes) = sized_bytes(body, at)?;
    match bytes.split_last() {
        Some((&0, text)) if length > 0 => Ok(text),
        _ => Err(WireError::BadString),
    }
}

/// Decodes `body`, a message's bytes after its header, by `signature` into `values`, in order;
/// how many descriptors it names. Every byte must belong to an argument.
pub fn decode<'a>(
    body: &'a [u8],
    signature: &[Arg],
    values: &mut [Value<'a>; MAX_ARGUMENTS],
) -> Result<usize, WireError> {
    if signature.len() > MAX_ARGUMENTS {
        return Err(WireError::TooManyArguments);
    }
    let mut at = 0;
    let mut descriptors = 0;
    for (slot, arg) in values.iter_mut().zip(signature) {
        *slot = match arg {
            Arg::Int => Value::Int(next_word(body, &mut at)? as i32),
            Arg::Uint => Value::Uint(next_word(body, &mut at)?),
            Arg::Fixed => Value::Fixed(next_word(body, &mut at)? as i32),
            Arg::Object => match next_word(body, &mut at)? {
                0 => return Err(WireError::NullObject),
                id => Value::Object(id),
            },
            Arg::NullableObject => Value::Object(next_word(body, &mut at)?),
            Arg::NewId => match next_word(body, &mut at)? {
                0 => return Err(WireError::NullObject),
                id => Value::NewId(id),
            },
            Arg::String => Value::String(string(body, &mut at)?),
            Arg::NullableString => match word(body, at) {
                Some(0) => {
                    at += 4;
                    Value::None
                }
                _ => Value::String(string(body, &mut at)?),
            },
            Arg::UntypedNewId => {
                let interface = string(body, &mut at)?;
                let version = word(body, at).ok_or(WireError::Truncated)?;
                let id = word(body, at + 4).ok_or(WireError::Truncated)?;
                at += 8;
                if id == 0 {
                    return Err(WireError::NullObject);
                }
                Value::UntypedNewId {
                    interface,
                    version,
                    id,
                }
            }
            Arg::Array => Value::Array(sized_bytes(body, &mut at)?.1),
            Arg::Fd => {
                descriptors += 1;
                Value::Fd
            }
        };
    }
    for slot in values.iter_mut().skip(signature.len()) {
        *slot = Value::None;
    }
    match at == body.len() {
        true => Ok(descriptors),
        false => Err(WireError::TrailingBytes),
    }
}

struct Writer<'b> {
    out: &'b mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn word(&mut self, value: u32) -> Result<(), WireError> {
        let end = self.at + 4;
        let slot = self.out.get_mut(self.at..end).ok_or(WireError::TooLong)?;
        slot.copy_from_slice(&value.to_ne_bytes());
        self.at = end;
        Ok(())
    }

    fn sized(&mut self, length: u32, bytes: &[u8], terminated: bool) -> Result<(), WireError> {
        self.word(length)?;
        let total = padded(length as usize);
        let end = self.at.checked_add(total).ok_or(WireError::TooLong)?;
        let slot = self.out.get_mut(self.at..end).ok_or(WireError::TooLong)?;
        slot.fill(0);
        slot[..bytes.len()].copy_from_slice(bytes);
        if terminated {
            slot[bytes.len()] = 0;
        }
        self.at = end;
        Ok(())
    }

    fn string(&mut self, text: &[u8]) -> Result<(), WireError> {
        let length = u32::try_from(text.len() + 1).map_err(|_| WireError::TooLong)?;
        self.sized(length, text, true)
    }
}

/// Writes the message for `object` with `opcode` and `values` at the start of `out`; its size
/// in bytes. A [`Value::Fd`] writes nothing: its descriptor goes in the control data.
pub fn encode(
    out: &mut [u8],
    object: u32,
    opcode: u16,
    values: &[Value],
) -> Result<usize, WireError> {
    let mut writer = Writer {
        out,
        at: HEADER_SIZE,
    };
    for value in values {
        match *value {
            Value::None | Value::Fd => {}
            Value::Int(value) | Value::Fixed(value) => writer.word(value as u32)?,
            Value::Uint(value) | Value::Object(value) | Value::NewId(value) => {
                writer.word(value)?
            }
            Value::String(text) => writer.string(text)?,
            Value::UntypedNewId {
                interface,
                version,
                id,
            } => {
                writer.string(interface)?;
                writer.word(version)?;
                writer.word(id)?;
            }
            Value::Array(bytes) => {
                let length = u32::try_from(bytes.len()).map_err(|_| WireError::TooLong)?;
                writer.sized(length, bytes, false)?;
            }
        }
    }
    let size = writer.at;
    if size > MAX_MESSAGE_SIZE {
        return Err(WireError::TooLong);
    }
    writer.at = 0;
    writer.word(object)?;
    writer.word((size as u32) << 16 | u32::from(opcode))?;
    Ok(size)
}

/// `value` as 24.8 fixed point, saturating.
pub const fn fixed(value: i32) -> i32 {
    value.saturating_mul(256)
}

/// A 24.8 fixed-point number's integer part, rounded towards negative infinity.
pub const fn fixed_to_int(value: i32) -> i32 {
    value >> 8
}
