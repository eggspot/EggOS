//! Reading and writing what a target names (ACPI 6.5, 19.3.5.8): named objects with their
//! implicit conversions, locals, arguments holding references, elements, and buffer fields.

use super::fields::{deposit_bits, extract_bits};
use super::{Frame, Interpreter};
use crate::Error;
use crate::host::Host;
use crate::namespace::NodeId;
use crate::object::{BufferField, Object, Target};
use alloc::vec;

const MAX_REFERENCE_DEPTH: u32 = 8;

impl<H: Host> Interpreter<H> {
    /// The value `target` holds; a field or buffer field is read.
    pub(crate) fn read_target(
        &mut self,
        frame: &mut Frame,
        target: &Target,
    ) -> Result<Object, Error> {
        match target {
            Target::Null => Ok(Object::Uninitialized),
            Target::Debug => Ok(Object::Debug),
            Target::Local(index) => Ok(frame.locals[usize::from(*index)].clone()),
            Target::Argument(index) => Ok(frame.arguments[usize::from(*index)].clone()),
            Target::Temporary(object) => Ok((**object).clone()),
            Target::Node(node) => match self.namespace.get(*node).cloned() {
                Some(Object::Field(field)) => self.read_field(&field),
                Some(Object::BufferField(field)) => self.read_buffer_field(frame, &field),
                Some(other) => Ok(other),
                None => Err(Error::NameNotFound),
            },
            Target::Element(container, index) => {
                let held = self.dereferenced(frame, container)?;
                let index = *index;
                match held {
                    Object::Package(mut elements) if index < elements.len() => {
                        Ok(elements.swap_remove(index))
                    }
                    Object::Buffer(bytes) => bytes
                        .get(index)
                        .map(|&byte| Object::Integer(u64::from(byte)))
                        .ok_or(Error::IndexOutOfRange),
                    Object::String(text) => text
                        .get(index)
                        .map(|&byte| Object::Integer(u64::from(byte)))
                        .ok_or(Error::IndexOutOfRange),
                    Object::Package(_) => Err(Error::IndexOutOfRange),
                    _ => Err(Error::WrongType),
                }
            }
        }
    }

    /// [`Self::read_target`], following the references it finds.
    pub(crate) fn dereferenced(
        &mut self,
        frame: &mut Frame,
        target: &Target,
    ) -> Result<Object, Error> {
        let mut value = self.read_target(frame, target)?;
        for _ in 0..MAX_REFERENCE_DEPTH {
            match value {
                Object::Reference(next) => value = self.read_target(frame, &next)?,
                other => return Ok(other),
            }
        }
        Err(Error::TooDeep)
    }

    /// `Store`: `value` into `target`, converted to a named integer's, string's or buffer's
    /// type, written to a field, or written through the reference an argument holds.
    pub(crate) fn store(
        &mut self,
        frame: &mut Frame,
        target: &Target,
        value: Object,
    ) -> Result<(), Error> {
        match target {
            Target::Null | Target::Temporary(_) => Ok(()),
            Target::Debug => {
                self.host.debug(&value);
                Ok(())
            }
            Target::Local(index) => {
                frame.locals[usize::from(*index)] = value;
                Ok(())
            }
            Target::Argument(index) => {
                let slot = usize::from(*index);
                if let Object::Reference(through) = frame.arguments[slot].clone() {
                    return self.store(frame, &through, value);
                }
                frame.arguments[slot] = value;
                Ok(())
            }
            Target::Node(node) => self.store_node(frame, *node, value),
            Target::Element(container, index) => {
                let integer = match &value {
                    Object::Integer(_) | Object::String(_) | Object::Buffer(_) => {
                        self.to_integer(&value).ok()
                    }
                    _ => None,
                };
                let Some(held) = self.location(frame, container, 0)? else {
                    return Ok(());
                };
                match held {
                    Object::Package(elements) => {
                        let element = elements.get_mut(*index).ok_or(Error::IndexOutOfRange)?;
                        *element = value;
                    }
                    Object::Buffer(bytes) | Object::String(bytes) => {
                        let byte = bytes.get_mut(*index).ok_or(Error::IndexOutOfRange)?;
                        *byte = integer.ok_or(Error::WrongType)? as u8;
                    }
                    _ => return Err(Error::WrongType),
                }
                Ok(())
            }
        }
    }

    fn store_node(&mut self, frame: &mut Frame, node: NodeId, value: Object) -> Result<(), Error> {
        let held = self.namespace.get(node).cloned().ok_or(Error::NameNotFound)?;
        let stored = match held {
            Object::Field(field) => return self.write_field(&field, &value),
            Object::BufferField(field) => return self.write_buffer_field(frame, &field, &value),
            Object::Integer(_) if !matches!(value, Object::Reference(_)) => {
                Object::Integer(self.to_integer(&value)?)
            }
            Object::String(_) if !matches!(value, Object::Reference(_)) => {
                Object::String(self.to_string_implicit(&value)?)
            }
            Object::Buffer(old) if !matches!(value, Object::Reference(_)) => {
                let mut bytes = self.to_buffer(&value)?;
                bytes.resize(old.len(), 0);
                Object::Buffer(bytes)
            }
            Object::Method(_)
            | Object::NativeMethod(_)
            | Object::Device
            | Object::Processor { .. }
            | Object::PowerResource { .. }
            | Object::ThermalZone
            | Object::Mutex { .. }
            | Object::Event { .. }
            | Object::Region(_)
            | Object::Scope => return Err(Error::WrongType),
            _ => value,
        };
        let slot = self.namespace.get_mut(node).ok_or(Error::NameNotFound)?;
        *slot = stored;
        Ok(())
    }

    /// `CopyObject`: `value` replaces what `target` holds, with no conversion.
    pub(crate) fn copy_object(
        &mut self,
        frame: &mut Frame,
        target: &Target,
        value: Object,
    ) -> Result<(), Error> {
        match target {
            Target::Node(node) => {
                let slot = self.namespace.get_mut(*node).ok_or(Error::NameNotFound)?;
                *slot = value;
                Ok(())
            }
            Target::Argument(index) => {
                frame.arguments[usize::from(*index)] = value;
                Ok(())
            }
            other => self.store(frame, other, value),
        }
    }

    /// The object `target` holds, to change in place, following references; `None` for a
    /// temporary, whose changes go nowhere.
    fn location<'a>(
        &'a mut self,
        frame: &'a mut Frame,
        target: &Target,
        depth: u32,
    ) -> Result<Option<&'a mut Object>, Error> {
        if depth > MAX_REFERENCE_DEPTH {
            return Err(Error::TooDeep);
        }
        let held: &Object = match target {
            Target::Node(node) => self.namespace.get(*node).ok_or(Error::NameNotFound)?,
            Target::Local(index) => &frame.locals[usize::from(*index)],
            Target::Argument(index) => &frame.arguments[usize::from(*index)],
            Target::Element(container, index) => {
                let index = *index;
                return match self.location(frame, container, depth + 1)? {
                    Some(Object::Package(elements)) => elements
                        .get_mut(index)
                        .map(Some)
                        .ok_or(Error::IndexOutOfRange),
                    Some(_) => Err(Error::WrongType),
                    None => Ok(None),
                };
            }
            Target::Null | Target::Debug | Target::Temporary(_) => return Ok(None),
        };
        if let Object::Reference(next) = held {
            let next = next.clone();
            return self.location(frame, &next, depth + 1);
        }
        Ok(match target {
            Target::Node(node) => self.namespace.get_mut(*node),
            Target::Local(index) => Some(&mut frame.locals[usize::from(*index)]),
            Target::Argument(index) => Some(&mut frame.arguments[usize::from(*index)]),
            _ => None,
        })
    }

    fn read_buffer_field(
        &mut self,
        frame: &mut Frame,
        field: &BufferField,
    ) -> Result<Object, Error> {
        let source = self.dereferenced(frame, &field.source)?;
        let bytes = self.to_buffer(&source)?;
        if field.bit_offset + field.bit_length > bytes.len() as u64 * 8 {
            return Err(Error::IndexOutOfRange);
        }
        let mut out = vec![0u8; field.bit_length.div_ceil(8) as usize];
        let mut done = 0;
        while done < field.bit_length {
            let length = (field.bit_length - done).min(64);
            let bits = extract_bits(&bytes, field.bit_offset + done, length);
            deposit_bits(&mut out, done, length, bits);
            done += length;
        }
        Ok(self.field_value(out, field.bit_length))
    }

    fn write_buffer_field(
        &mut self,
        frame: &mut Frame,
        field: &BufferField,
        value: &Object,
    ) -> Result<(), Error> {
        let mut bits = self.to_buffer(value)?;
        bits.resize(field.bit_length.div_ceil(8) as usize, 0);
        let Some(held) = self.location(frame, &field.source, 0)? else {
            return Ok(());
        };
        let Object::Buffer(bytes) = held else {
            return Err(Error::WrongType);
        };
        if field.bit_offset + field.bit_length > bytes.len() as u64 * 8 {
            return Err(Error::IndexOutOfRange);
        }
        let mut done = 0;
        while done < field.bit_length {
            let length = (field.bit_length - done).min(64);
            let chunk = extract_bits(&bits, done, length);
            deposit_bits(bytes, field.bit_offset + done, length, chunk);
            done += length;
        }
        Ok(())
    }
}
