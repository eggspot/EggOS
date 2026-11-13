//! Field units (ACPI 6.5, 19.6.48, 19.6.64 and 19.6.7): their bits read and written in units of
//! their access width, through an operation region, an index and data pair, or a banked region,
//! with the update rule for a unit's other bits.

use super::Interpreter;
use crate::Error;
use crate::host::Host;
use crate::name::{NameSeg, Path};
use crate::namespace::NodeId;
use crate::object::{Access, Field, FieldKind, Object, PciFunction, RegionSpace, UpdateRule};
use alloc::vec;
use alloc::vec::Vec;

impl<H: Host> Interpreter<H> {
    /// The value of `bits` bits read into `bytes`: an integer when they fit one, else a buffer.
    pub(crate) fn field_value(&self, bytes: Vec<u8>, bits: u64) -> Object {
        match bits <= self.integer_bytes() as u64 * 8 {
            true => Object::Integer(
                bytes
                    .iter()
                    .rev()
                    .fold(0u64, |value, &byte| value << 8 | u64::from(byte)),
            ),
            false => Object::Buffer(bytes),
        }
    }

    pub(crate) fn read_field(&mut self, field: &Field) -> Result<Object, Error> {
        let width = unit_bits(field);
        let end = field.bit_offset + field.bit_length;
        let mut out = vec![0u8; field.bit_length.div_ceil(8) as usize];
        let mut unit = field.bit_offset / width * width;
        while unit < end {
            let value = self.read_unit(field, unit / 8, width)?;
            let from = unit.max(field.bit_offset);
            let to = (unit + width).min(end);
            let bits = value >> (from - unit) & mask(to - from);
            deposit_bits(&mut out, from - field.bit_offset, to - from, bits);
            unit += width;
        }
        Ok(self.field_value(out, field.bit_length))
    }

    pub(crate) fn write_field(&mut self, field: &Field, value: &Object) -> Result<(), Error> {
        let mut bytes = self.to_buffer(value)?;
        bytes.resize(field.bit_length.div_ceil(8) as usize, 0);
        let width = unit_bits(field);
        let end = field.bit_offset + field.bit_length;
        let mut unit = field.bit_offset / width * width;
        while unit < end {
            let from = unit.max(field.bit_offset);
            let to = (unit + width).min(end);
            let bits = extract_bits(&bytes, from - field.bit_offset, to - from);
            let whole = from == unit && to == unit + width;
            let base = match (whole, field.update) {
                (true, _) | (false, UpdateRule::WriteAsZeros) => 0,
                (false, UpdateRule::WriteAsOnes) => mask(width),
                (false, UpdateRule::Preserve) => self.read_unit(field, unit / 8, width)?,
            };
            let shift = from - unit;
            let covered = mask(to - from) << shift;
            let merged = base & !covered | bits << shift & covered;
            self.write_unit(field, unit / 8, width, merged)?;
            unit += width;
        }
        Ok(())
    }

    fn field_of(&self, node: NodeId) -> Result<Field, Error> {
        match self.namespace.get(node) {
            Some(Object::Field(field)) => Ok(*field),
            _ => Err(Error::WrongType),
        }
    }

    fn read_unit(&mut self, field: &Field, byte_offset: u64, width: u64) -> Result<u64, Error> {
        match field.kind {
            FieldKind::Region(region) => self.region_access(region, byte_offset, width, None),
            FieldKind::Indexed { index, data } => {
                let index = self.field_of(index)?;
                self.write_field(&index, &Object::Integer(byte_offset))?;
                let data = self.field_of(data)?;
                let value = self.read_field(&data)?;
                Ok(self.to_integer(&value)? & mask(width))
            }
            FieldKind::Banked {
                region,
                bank,
                value,
            } => {
                let bank = self.field_of(bank)?;
                self.write_field(&bank, &Object::Integer(value))?;
                self.region_access(region, byte_offset, width, None)
            }
        }
    }

    fn write_unit(
        &mut self,
        field: &Field,
        byte_offset: u64,
        width: u64,
        value: u64,
    ) -> Result<(), Error> {
        match field.kind {
            FieldKind::Region(region) => {
                self.region_access(region, byte_offset, width, Some(value))?;
            }
            FieldKind::Indexed { index, data } => {
                let index = self.field_of(index)?;
                self.write_field(&index, &Object::Integer(byte_offset))?;
                let data = self.field_of(data)?;
                self.write_field(&data, &Object::Integer(value))?;
            }
            FieldKind::Banked {
                region,
                bank,
                value: bank_value,
            } => {
                let bank = self.field_of(bank)?;
                self.write_field(&bank, &Object::Integer(bank_value))?;
                self.region_access(region, byte_offset, width, Some(value))?;
            }
        }
        Ok(())
    }

    fn region_access(
        &mut self,
        region: NodeId,
        byte_offset: u64,
        width: u64,
        value: Option<u64>,
    ) -> Result<u64, Error> {
        let Some(Object::Region(held)) = self.namespace.get(region).cloned() else {
            return Err(Error::WrongType);
        };
        if byte_offset + width / 8 > held.length {
            return Err(Error::OutsideRegion);
        }
        let width = width as u32;
        let result = match held.space {
            RegionSpace::PciConfig => {
                let function = self.pci_function(region)?;
                let offset =
                    u16::try_from(held.offset + byte_offset).map_err(|_| Error::OutsideRegion)?;
                match value {
                    Some(value) => self
                        .host
                        .write_pci(function, offset, width, value)
                        .map(|()| 0),
                    None => self.host.read_pci(function, offset, width),
                }
            }
            space => {
                let address = held.offset + byte_offset;
                match value {
                    Some(value) => self.host.write(space, address, width, value).map(|()| 0),
                    None => self.host.read(space, address, width),
                }
            }
        };
        result.map_err(Error::Host)
    }

    fn pci_function(&mut self, region: NodeId) -> Result<PciFunction, Error> {
        if let Some(Object::Region(held)) = self.namespace.get(region)
            && let Some(function) = held.pci
        {
            return Ok(function);
        }
        let device = self
            .namespace
            .path_of(region)
            .and_then(Path::parent)
            .ok_or(Error::NameNotFound)?;
        let address = self.integer_child(&device, b"_ADR")?;
        let mut function = PciFunction {
            segment: 0,
            bus: 0,
            device: (address >> 16) as u8,
            function: address as u8,
        };
        let mut at = Some(device);
        while let Some(path) = at {
            if self.is_pci_host_bridge(&path) {
                function.bus = self.integer_child(&path, b"_BBN")? as u8;
                function.segment = self.integer_child(&path, b"_SEG")? as u16;
                break;
            }
            at = path.parent();
        }
        if let Some(Object::Region(held)) = self.namespace.get_mut(region) {
            held.pci = Some(function);
        }
        Ok(function)
    }

    fn integer_child(&mut self, scope: &Path, name: &[u8; 4]) -> Result<u64, Error> {
        match self.namespace.find(&scope.child(NameSeg::new(name))) {
            Some(node) => {
                let value = self.evaluate_node(node, Vec::new())?;
                self.to_integer(&value)
            }
            None => Ok(0),
        }
    }
}

fn unit_bits(field: &Field) -> u64 {
    match field.access {
        Access::Byte | Access::Buffer => 8,
        Access::Word => 16,
        Access::DoubleWord => 32,
        Access::QuadWord => 64,
        Access::Any => {
            let last = field.bit_offset + field.bit_length.max(1) - 1;
            [8, 16, 32]
                .into_iter()
                .find(|width| field.bit_offset / width == last / width)
                .unwrap_or(32)
        }
    }
}

fn mask(bits: u64) -> u64 {
    match bits {
        64.. => u64::MAX,
        bits => (1 << bits) - 1,
    }
}

/// The `length` bits (at most 64) of `bytes` from bit `at`, as an integer.
pub(crate) fn extract_bits(bytes: &[u8], at: u64, length: u64) -> u64 {
    (0..length).fold(0u64, |value, bit| {
        let position = at + bit;
        let set = bytes
            .get((position / 8) as usize)
            .is_some_and(|byte| byte >> (position % 8) & 1 != 0);
        value | u64::from(set) << bit
    })
}

/// Writes `value`'s low `length` bits (at most 64) into `bytes` from bit `at`.
pub(crate) fn deposit_bits(bytes: &mut [u8], at: u64, length: u64, value: u64) {
    for bit in 0..length {
        let position = at + bit;
        let Some(byte) = bytes.get_mut((position / 8) as usize) else {
            return;
        };
        let flag = 1u8 << (position % 8);
        match value >> bit & 1 {
            0 => *byte &= !flag,
            _ => *byte |= flag,
        }
    }
}
