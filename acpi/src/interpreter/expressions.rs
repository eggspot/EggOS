//! Expression opcodes (ACPI 6.5, 19.6 and 20.2.5.4): data objects, locals and arguments,
//! arithmetic and logic, conversions, references, and the synchronisation and time opcodes
//! that give a value.

use super::{Frame, INTERPRETER_REVISION, Interpreter};
use crate::Error;
use crate::host::Host;
use crate::name::{NameSeg, NameString, Path};
use crate::object::{Object, Target};
use crate::opcodes::*;
use crate::stream::Stream;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

const MATCH_TRUE: u8 = 0;
const MATCH_EQUAL: u8 = 1;
const MATCH_LESS_OR_EQUAL: u8 = 2;
const MATCH_LESS: u8 = 3;
const MATCH_GREATER_OR_EQUAL: u8 = 4;
const MATCH_GREATER: u8 = 5;
const END_TAG: u8 = 0x79;

impl<H: Host> Interpreter<H> {
    pub(crate) fn operand_with(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
        opcode: u16,
    ) -> Result<Object, Error> {
        let mask = self.integer_mask;
        match opcode {
            ZERO => Ok(Object::Integer(0)),
            ONE => Ok(Object::Integer(1)),
            ONES => Ok(Object::Integer(mask)),
            BYTE_PREFIX => Ok(Object::Integer(u64::from(stream.byte()?))),
            WORD_PREFIX => Ok(Object::Integer(u64::from(stream.word()?))),
            DWORD_PREFIX => Ok(Object::Integer(u64::from(stream.dword()?))),
            QWORD_PREFIX => Ok(Object::Integer(stream.qword()? & mask)),
            STRING_PREFIX => Ok(Object::String(stream.string()?.to_vec())),
            REVISION => Ok(Object::Integer(INTERPRETER_REVISION)),
            BUFFER => self.buffer(frame, stream),
            PACKAGE | VAR_PACKAGE => self.package(frame, stream, opcode),
            LOCAL_0..=LOCAL_7 => Ok(frame.locals[usize::from(opcode - LOCAL_0)].clone()),
            ARG_0..=ARG_6 => Ok(frame.arguments[usize::from(opcode - ARG_0)].clone()),
            DEBUG => Ok(Object::Debug),
            STORE => {
                let value = self.operand(frame, stream)?;
                let target = self.super_name(frame, stream)?;
                self.store(frame, &target, value.clone())?;
                Ok(value)
            }
            ADD | SUBTRACT | MULTIPLY | SHIFT_LEFT | SHIFT_RIGHT | AND | NAND | OR | NOR | XOR
            | MOD => {
                let left = self.integer_operand(frame, stream)?;
                let right = self.integer_operand(frame, stream)?;
                let value = match opcode {
                    ADD => left.wrapping_add(right),
                    SUBTRACT => left.wrapping_sub(right),
                    MULTIPLY => left.wrapping_mul(right),
                    SHIFT_LEFT if right < 64 => left << right,
                    SHIFT_RIGHT if right < 64 => left >> right,
                    SHIFT_LEFT | SHIFT_RIGHT => 0,
                    AND => left & right,
                    NAND => !(left & right),
                    OR => left | right,
                    NOR => !(left | right),
                    XOR => left ^ right,
                    _ => left.checked_rem(right).ok_or(Error::DivideByZero)?,
                };
                self.result(frame, stream, Object::Integer(value & mask))
            }
            DIVIDE => {
                let dividend = self.integer_operand(frame, stream)?;
                let divisor = self.integer_operand(frame, stream)?;
                let quotient = dividend.checked_div(divisor).ok_or(Error::DivideByZero)?;
                let remainder = self.super_name(frame, stream)?;
                self.store(frame, &remainder, Object::Integer(dividend % divisor))?;
                self.result(frame, stream, Object::Integer(quotient))
            }
            NOT => {
                let value = self.integer_operand(frame, stream)?;
                self.result(frame, stream, Object::Integer(!value & mask))
            }
            FIND_SET_LEFT_BIT | FIND_SET_RIGHT_BIT => {
                let value = self.integer_operand(frame, stream)?;
                let found = match (value, opcode) {
                    (0, _) => 0,
                    (_, FIND_SET_LEFT_BIT) => u64::from(64 - value.leading_zeros()),
                    _ => u64::from(value.trailing_zeros() + 1),
                };
                self.result(frame, stream, Object::Integer(found))
            }
            INCREMENT | DECREMENT => {
                let target = self.super_name(frame, stream)?;
                let held = self.dereferenced(frame, &target)?;
                let value = self.to_integer(&held)?;
                let changed = match opcode {
                    INCREMENT => value.wrapping_add(1),
                    _ => value.wrapping_sub(1),
                } & mask;
                self.store(frame, &target, Object::Integer(changed))?;
                Ok(Object::Integer(changed))
            }
            LAND | LOR => {
                let left = self.integer_operand(frame, stream)? != 0;
                let right = self.integer_operand(frame, stream)? != 0;
                let holds = match opcode {
                    LAND => left && right,
                    _ => left || right,
                };
                Ok(self.truth(holds))
            }
            LNOT => {
                let value = self.integer_operand(frame, stream)?;
                Ok(self.truth(value == 0))
            }
            LEQUAL | LGREATER | LLESS => {
                let left = self.operand(frame, stream)?;
                let right = self.operand(frame, stream)?;
                let order = self.compare(&left, &right)?;
                let holds = match opcode {
                    LEQUAL => order == Ordering::Equal,
                    LGREATER => order == Ordering::Greater,
                    _ => order == Ordering::Less,
                };
                Ok(self.truth(holds))
            }
            CONCAT => {
                let left = self.operand(frame, stream)?;
                let right = self.operand(frame, stream)?;
                let joined = self.concatenate(&left, &right)?;
                self.result(frame, stream, joined)
            }
            CONCAT_RES => {
                let left = self.operand(frame, stream)?;
                let right = self.operand(frame, stream)?;
                let (left, right) = (self.to_buffer(&left)?, self.to_buffer(&right)?);
                let joined = concatenate_resources(&left, &right);
                self.result(frame, stream, Object::Buffer(joined))
            }
            TO_BUFFER => {
                let value = self.operand(frame, stream)?;
                let buffer = match value {
                    Object::String(mut text) => {
                        text.push(0);
                        text
                    }
                    other => self.to_buffer(&other)?,
                };
                self.result(frame, stream, Object::Buffer(buffer))
            }
            TO_INTEGER => {
                let value = self.operand(frame, stream)?;
                let integer = match &value {
                    Object::String(text) => parse_integer(text) & mask,
                    other => self.to_integer(other)?,
                };
                self.result(frame, stream, Object::Integer(integer))
            }
            TO_HEX_STRING => {
                let value = self.operand(frame, stream)?;
                let text = match &value {
                    Object::Integer(integer) => {
                        let mut text = b"0x".to_vec();
                        push_hex(&mut text, *integer, self.integer_bytes() * 2);
                        text
                    }
                    Object::Buffer(bytes) => join_bytes(bytes, |out, byte| {
                        out.extend_from_slice(b"0x");
                        push_hex(out, u64::from(byte), 2);
                    }),
                    Object::String(text) => text.clone(),
                    _ => return Err(Error::WrongType),
                };
                self.result(frame, stream, Object::String(text))
            }
            TO_DECIMAL_STRING => {
                let value = self.operand(frame, stream)?;
                let text = match &value {
                    Object::Integer(integer) => {
                        let mut text = Vec::new();
                        push_decimal(&mut text, *integer);
                        text
                    }
                    Object::Buffer(bytes) => join_bytes(bytes, |out, byte| {
                        push_decimal(out, u64::from(byte));
                    }),
                    Object::String(text) => text.clone(),
                    _ => return Err(Error::WrongType),
                };
                self.result(frame, stream, Object::String(text))
            }
            TO_STRING => {
                let value = self.operand(frame, stream)?;
                let limit = self.integer_operand(frame, stream)?;
                let bytes = self.to_buffer(&value)?;
                let text: Vec<u8> = bytes
                    .iter()
                    .copied()
                    .take_while(|&byte| byte != 0)
                    .take(usize::try_from(limit).unwrap_or(usize::MAX))
                    .collect();
                self.result(frame, stream, Object::String(text))
            }
            MID => {
                let source = self.operand(frame, stream)?;
                let start = self.integer_operand(frame, stream)?;
                let length = self.integer_operand(frame, stream)?;
                let piece = |bytes: &[u8]| {
                    let start = usize::try_from(start)
                        .unwrap_or(usize::MAX)
                        .min(bytes.len());
                    let length = usize::try_from(length).unwrap_or(usize::MAX);
                    let end = start.saturating_add(length).min(bytes.len());
                    bytes[start..end].to_vec()
                };
                let value = match &source {
                    Object::String(text) => Object::String(piece(text.as_slice())),
                    other => Object::Buffer(piece(self.to_buffer(other)?.as_slice())),
                };
                self.result(frame, stream, value)
            }
            MATCH => self.match_package(frame, stream),
            SIZE_OF => {
                let target = self.super_name(frame, stream)?;
                let value = self.dereferenced(frame, &target)?;
                let size = match value {
                    Object::String(text) => text.len(),
                    Object::Buffer(bytes) => bytes.len(),
                    Object::Package(elements) => elements.len(),
                    _ => return Err(Error::WrongType),
                };
                Ok(Object::Integer(size as u64))
            }
            OBJECT_TYPE => {
                let target = self.super_name(frame, stream)?;
                let kind = match &target {
                    Target::Node(node) => self.namespace.get(*node).map_or(0, Object::object_type),
                    Target::Debug => crate::object::object_type::DEBUG,
                    other => self.dereferenced(frame, other)?.object_type(),
                };
                Ok(Object::Integer(kind))
            }
            INDEX => {
                let container = self.container(frame, stream)?;
                let index = self.integer_operand(frame, stream)?;
                let held = self.dereferenced(frame, &container)?;
                let length = match &held {
                    Object::Package(elements) => elements.len(),
                    Object::Buffer(bytes) => bytes.len(),
                    Object::String(text) => text.len(),
                    _ => return Err(Error::WrongType),
                };
                let index = usize::try_from(index)
                    .ok()
                    .filter(|&index| index < length)
                    .ok_or(Error::IndexOutOfRange)?;
                let container = match container {
                    Target::Temporary(_) => Target::Temporary(Box::new(held)),
                    location => location,
                };
                let reference = Object::Reference(Target::Element(Box::new(container), index));
                self.result(frame, stream, reference)
            }
            REF_OF => {
                let target = self.super_name(frame, stream)?;
                Ok(Object::Reference(target))
            }
            DEREF_OF => {
                let value = self.operand(frame, stream)?;
                match value {
                    Object::Reference(target) => self.read_target(frame, &target),
                    Object::String(text) => {
                        let node = self.node_named(&text, &frame.scope)?;
                        self.read_target(frame, &Target::Node(node))
                    }
                    _ => Err(Error::WrongType),
                }
            }
            COND_REF_OF => {
                let found = self.super_name_if_present(frame, stream)?;
                let target = self.super_name(frame, stream)?;
                match found {
                    Some(source) => {
                        self.store(frame, &target, Object::Reference(source))?;
                        Ok(self.truth(true))
                    }
                    None => Ok(self.truth(false)),
                }
            }
            COPY_OBJECT => {
                let value = self.operand(frame, stream)?;
                let target = self.super_name(frame, stream)?;
                self.copy_object(frame, &target, value.clone())?;
                Ok(value)
            }
            ACQUIRE => {
                let target = self.super_name(frame, stream)?;
                stream.word()?;
                let Target::Node(node) = target else {
                    return Err(Error::WrongType);
                };
                match self.namespace.get_mut(node) {
                    Some(Object::Mutex { held, .. }) => *held = held.saturating_add(1),
                    _ => return Err(Error::WrongType),
                }
                Ok(Object::Integer(0))
            }
            WAIT => {
                let target = self.super_name(frame, stream)?;
                let timeout = self.integer_operand(frame, stream)?;
                let Target::Node(node) = target else {
                    return Err(Error::WrongType);
                };
                let signalled = match self.namespace.get_mut(node) {
                    Some(Object::Event { signals }) if *signals > 0 => {
                        *signals -= 1;
                        true
                    }
                    Some(Object::Event { .. }) => false,
                    _ => return Err(Error::WrongType),
                };
                if !signalled && timeout < 0xffff {
                    self.host.sleep(timeout);
                }
                Ok(self.truth(!signalled))
            }
            TIMER => Ok(Object::Integer(self.host.timer() & mask)),
            FROM_BCD => {
                let mut value = self.integer_operand(frame, stream)?;
                let mut decimal = 0u64;
                let mut scale = 1u64;
                while value != 0 {
                    decimal = decimal.wrapping_add((value & 0xf) * scale);
                    scale = scale.wrapping_mul(10);
                    value >>= 4;
                }
                self.result(frame, stream, Object::Integer(decimal & mask))
            }
            TO_BCD => {
                let mut value = self.integer_operand(frame, stream)?;
                let mut bcd = 0u64;
                let mut shift = 0;
                while value != 0 && shift < 64 {
                    bcd |= (value % 10) << shift;
                    value /= 10;
                    shift += 4;
                }
                self.result(frame, stream, Object::Integer(bcd & mask))
            }
            LOAD_TABLE => Err(Error::Unsupported("loading tables from AML")),
            other => Err(Error::UnknownOpcode(other)),
        }
    }

    fn result(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
        value: Object,
    ) -> Result<Object, Error> {
        let target = self.super_name(frame, stream)?;
        self.store(frame, &target, value.clone())?;
        Ok(value)
    }

    fn truth(&self, holds: bool) -> Object {
        Object::Integer(if holds { self.integer_mask } else { 0 })
    }

    fn compare(&self, left: &Object, right: &Object) -> Result<Ordering, Error> {
        match left {
            Object::Integer(value) => Ok(value.cmp(&self.to_integer(right)?)),
            Object::String(text) => Ok(text.as_slice().cmp(&self.to_string_implicit(right)?)),
            Object::Buffer(bytes) => Ok(bytes.as_slice().cmp(&self.to_buffer(right)?)),
            _ => Err(Error::WrongType),
        }
    }

    fn concatenate(&self, left: &Object, right: &Object) -> Result<Object, Error> {
        match left {
            Object::Integer(value) => {
                let width = self.integer_bytes();
                let mut joined = value.to_le_bytes()[..width].to_vec();
                joined.extend_from_slice(&self.to_integer(right)?.to_le_bytes()[..width]);
                Ok(Object::Buffer(joined))
            }
            Object::String(text) => {
                let mut joined = text.clone();
                joined.extend_from_slice(&self.to_string_implicit(right)?);
                Ok(Object::String(joined))
            }
            Object::Buffer(bytes) => {
                let mut joined = bytes.clone();
                joined.extend_from_slice(&self.to_buffer(right)?);
                Ok(Object::Buffer(joined))
            }
            _ => Err(Error::WrongType),
        }
    }

    fn buffer(&mut self, frame: &mut Frame, stream: &mut Stream<'_>) -> Result<Object, Error> {
        let end = stream.package_end()?;
        let mut body = stream.until(end);
        let size = self.integer_operand(frame, &mut body)?;
        let initial = body.bytes(end - body.at)?;
        stream.skip_to(end);
        let size = usize::try_from(size).map_err(|_| Error::WrongType)?;
        let mut bytes = vec![0u8; size.max(initial.len())];
        bytes[..initial.len()].copy_from_slice(initial);
        Ok(Object::Buffer(bytes))
    }

    fn package(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
        opcode: u16,
    ) -> Result<Object, Error> {
        let end = stream.package_end()?;
        let mut body = stream.until(end);
        let count = match opcode {
            PACKAGE => usize::from(body.byte()?),
            _ => usize::try_from(self.integer_operand(frame, &mut body)?)
                .map_err(|_| Error::WrongType)?,
        };
        let mut elements = Vec::new();
        while !body.is_done() {
            elements.push(self.data_reference(frame, &mut body)?);
        }
        stream.skip_to(end);
        if elements.len() < count {
            elements.resize(count, Object::Uninitialized);
        }
        Ok(Object::Package(elements))
    }

    fn match_package(
        &mut self,
        frame: &mut Frame,
        stream: &mut Stream<'_>,
    ) -> Result<Object, Error> {
        let searched = self.operand(frame, stream)?;
        let first_test = stream.byte()?;
        let first = self.operand(frame, stream)?;
        let second_test = stream.byte()?;
        let second = self.operand(frame, stream)?;
        let start = self.integer_operand(frame, stream)?;
        let Object::Package(elements) = searched else {
            return Err(Error::WrongType);
        };
        let start = usize::try_from(start).unwrap_or(usize::MAX);
        for (index, element) in elements.iter().enumerate().skip(start) {
            let first_holds = self.matches(element, first_test, &first);
            if first_holds && self.matches(element, second_test, &second) {
                return Ok(Object::Integer(index as u64));
            }
        }
        Ok(Object::Integer(self.integer_mask))
    }

    fn matches(&self, element: &Object, test: u8, against: &Object) -> bool {
        if test == MATCH_TRUE {
            return true;
        }
        if !matches!(
            element,
            Object::Integer(_) | Object::String(_) | Object::Buffer(_)
        ) {
            return false;
        }
        let Ok(order) = self.compare(element, against) else {
            return false;
        };
        match test {
            MATCH_EQUAL => order == Ordering::Equal,
            MATCH_LESS_OR_EQUAL => order != Ordering::Greater,
            MATCH_LESS => order == Ordering::Less,
            MATCH_GREATER_OR_EQUAL => order != Ordering::Less,
            MATCH_GREATER => order == Ordering::Greater,
            _ => false,
        }
    }

    /// The node a path written as a string names: absolute, or searched for from `scope`.
    pub(crate) fn node_named(
        &self,
        text: &[u8],
        scope: &Path,
    ) -> Result<crate::namespace::NodeId, Error> {
        if text.starts_with(b"\\") {
            let path = Path::parse(text).ok_or(Error::MalformedName)?;
            return self.namespace.find(&path).ok_or(Error::NameNotFound);
        }
        let parents = text.iter().take_while(|&&byte| byte == b'^').count();
        let segments = text[parents..]
            .split(|&byte| byte == b'.')
            .map(NameSeg::from_text)
            .collect::<Option<Vec<_>>>()
            .ok_or(Error::MalformedName)?;
        let name = NameString {
            root: false,
            parents: u8::try_from(parents).map_err(|_| Error::MalformedName)?,
            segments,
        };
        self.namespace
            .search(&name, scope)
            .ok_or(Error::NameNotFound)
    }
}

fn concatenate_resources(first: &[u8], second: &[u8]) -> Vec<u8> {
    let without_end = |template: &[u8]| -> Vec<u8> {
        let mut at = 0;
        while let Some(&tag) = template.get(at) {
            if tag == END_TAG {
                return template[..at].to_vec();
            }
            let length = match tag & 0x80 {
                0 => 1 + usize::from(tag & 0x07),
                _ => match template.get(at + 1..at + 3) {
                    Some(bytes) => 3 + usize::from(u16::from_le_bytes([bytes[0], bytes[1]])),
                    None => return template[..at].to_vec(),
                },
            };
            at += length;
        }
        template.to_vec()
    };
    let mut joined = without_end(first);
    joined.extend_from_slice(&without_end(second));
    joined.extend_from_slice(&[END_TAG, 0]);
    joined
}

fn parse_integer(text: &[u8]) -> u64 {
    let text = text.trim_ascii_start();
    let (digits, radix) = match text {
        [b'0', b'x' | b'X', rest @ ..] => (rest, 16),
        other => (other, 10),
    };
    let mut value = 0u64;
    for &byte in digits {
        let Some(digit) = (byte as char).to_digit(radix) else {
            break;
        };
        value = value
            .wrapping_mul(u64::from(radix))
            .wrapping_add(u64::from(digit));
    }
    value
}

pub(crate) fn push_hex(out: &mut Vec<u8>, value: u64, digits: usize) {
    for index in (0..digits).rev() {
        let nibble = (value >> (index * 4)) as u8 & 0xf;
        out.push(match nibble {
            0..=9 => b'0' + nibble,
            _ => b'A' + nibble - 10,
        });
    }
}

fn push_decimal(out: &mut Vec<u8>, value: u64) {
    let mut digits = [0u8; 20];
    let mut count = 0;
    let mut rest = value;
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        count += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    out.extend(digits[..count].iter().rev());
}

fn join_bytes(bytes: &[u8], mut each: impl FnMut(&mut Vec<u8>, u8)) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, &byte) in bytes.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        each(&mut out, byte);
    }
    out
}
