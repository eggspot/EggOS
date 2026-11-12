//! The implicit conversions between integers, strings and buffers (ACPI 6.5, 19.3.5.7).

use super::Interpreter;
use super::expressions::push_hex;
use crate::Error;
use crate::host::Host;
use crate::object::Object;
use alloc::vec::Vec;

impl<H: Host> Interpreter<H> {
    /// How many bytes an integer takes: 8, or 4 under a DSDT of revision 0 or 1.
    pub(crate) fn integer_bytes(&self) -> usize {
        match self.has_32_bit_integers() {
            true => 4,
            false => 8,
        }
    }

    /// An integer: a string's leading hexadecimal digits, a buffer's first bytes, little-endian.
    /// An uninitialized object reads as 0.
    pub fn to_integer(&self, value: &Object) -> Result<u64, Error> {
        let integer = match value {
            Object::Integer(integer) => *integer,
            Object::String(text) => {
                let mut integer = 0u64;
                for &byte in text.iter().take(self.integer_bytes() * 2) {
                    let Some(digit) = (byte as char).to_digit(16) else {
                        break;
                    };
                    integer = integer << 4 | u64::from(digit);
                }
                integer
            }
            Object::Buffer(bytes) => bytes
                .iter()
                .take(self.integer_bytes())
                .rev()
                .fold(0u64, |integer, &byte| integer << 8 | u64::from(byte)),
            Object::Uninitialized => 0,
            _ => return Err(Error::WrongType),
        };
        Ok(integer & self.integer_mask)
    }

    /// A buffer: an integer's bytes, little-endian, a string's characters without its end.
    pub fn to_buffer(&self, value: &Object) -> Result<Vec<u8>, Error> {
        match value {
            Object::Integer(integer) => Ok(integer.to_le_bytes()[..self.integer_bytes()].to_vec()),
            Object::String(text) => Ok(text.clone()),
            Object::Buffer(bytes) => Ok(bytes.clone()),
            Object::Uninitialized => Ok(Vec::new()),
            _ => Err(Error::WrongType),
        }
    }

    /// A string: an integer in hexadecimal at full width, a buffer's bytes in hexadecimal
    /// separated by spaces.
    pub fn to_string_implicit(&self, value: &Object) -> Result<Vec<u8>, Error> {
        match value {
            Object::Integer(integer) => {
                let mut text = Vec::new();
                push_hex(&mut text, *integer, self.integer_bytes() * 2);
                Ok(text)
            }
            Object::String(text) => Ok(text.clone()),
            Object::Buffer(bytes) => {
                let mut text = Vec::new();
                for (index, &byte) in bytes.iter().enumerate() {
                    if index > 0 {
                        text.push(b' ');
                    }
                    push_hex(&mut text, u64::from(byte), 2);
                }
                Ok(text)
            }
            Object::Uninitialized => Ok(Vec::new()),
            _ => Err(Error::WrongType),
        }
    }
}
