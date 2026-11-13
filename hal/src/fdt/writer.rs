//! Writing a flattened device tree into a fixed buffer, allocating nothing: for machines whose
//! firmware gives no device tree (QEMU's `loongson3-virt` boots MIPS64 kernels with Loongson's
//! own boot parameters, D313), the HAL describes the machine in one, so the kernel's discovery
//! (memory, CPUs, devices, PCI) reads it as any other.

use super::{
    FDT_BEGIN_NODE, FDT_END, FDT_END_NODE, FDT_PROP, HEADER_LENGTH, MAGIC, SUPPORTED_VERSION,
};

/// The most bytes of property names one tree holds.
pub const STRINGS_CAPACITY: usize = 512;
const RESERVATIONS_LENGTH: usize = 16;
const LAST_COMPATIBLE_VERSION: u32 = 16;

/// A device tree being written into a buffer. Every call past the buffer's end, or past
/// [`STRINGS_CAPACITY`] bytes of names, marks the tree failed, and [`Writer::finish`] says so.
#[derive(Debug)]
pub struct Writer<'a> {
    out: &'a mut [u8],
    at: usize,
    strings: [u8; STRINGS_CAPACITY],
    strings_length: usize,
    depth: usize,
    failed: bool,
}

impl<'a> Writer<'a> {
    /// A tree written into `out`, its structure starting after the header and the
    /// reservations.
    pub fn new(out: &'a mut [u8]) -> Self {
        Self {
            out,
            at: HEADER_LENGTH + RESERVATIONS_LENGTH,
            strings: [0; STRINGS_CAPACITY],
            strings_length: 0,
            depth: 0,
            failed: false,
        }
    }

    fn bytes(&mut self, bytes: &[u8]) {
        match self.out.get_mut(self.at..self.at + bytes.len()) {
            Some(place) if !self.failed => {
                place.copy_from_slice(bytes);
                self.at += bytes.len();
            }
            _ => self.failed = true,
        }
    }

    fn word(&mut self, word: u32) {
        self.bytes(&word.to_be_bytes());
    }

    fn pad(&mut self) {
        while !self.at.is_multiple_of(4) && !self.failed {
            self.bytes(&[0]);
        }
    }

    fn name_offset(&mut self, name: &[u8]) -> u32 {
        let mut offset = 0;
        while offset < self.strings_length {
            let end = self.strings[offset..self.strings_length]
                .iter()
                .position(|&byte| byte == 0)
                .map_or(self.strings_length, |length| offset + length);
            if &self.strings[offset..end] == name {
                return offset as u32;
            }
            offset = end + 1;
        }
        let end = self.strings_length + name.len() + 1;
        if end > STRINGS_CAPACITY {
            self.failed = true;
            return 0;
        }
        self.strings[self.strings_length..end - 1].copy_from_slice(name);
        self.strings[end - 1] = 0;
        let offset = self.strings_length as u32;
        self.strings_length = end;
        offset
    }

    /// Opens a node named `name` (`""` for the root, `memory@200000`).
    pub fn begin(&mut self, name: &[u8]) -> &mut Self {
        self.word(FDT_BEGIN_NODE);
        self.bytes(name);
        self.bytes(&[0]);
        self.pad();
        self.depth += 1;
        self
    }

    /// Closes the node opened last.
    pub fn end(&mut self) -> &mut Self {
        if self.depth == 0 {
            self.failed = true;
            return self;
        }
        self.depth -= 1;
        self.word(FDT_END_NODE);
        self
    }

    /// A property of the open node with the raw `value`.
    pub fn property(&mut self, name: &[u8], value: &[u8]) -> &mut Self {
        let offset = self.name_offset(name);
        self.word(FDT_PROP);
        self.word(value.len() as u32);
        self.word(offset);
        self.bytes(value);
        self.pad();
        self
    }

    /// A property of big-endian 32-bit cells.
    pub fn cells(&mut self, name: &[u8], cells: &[u32]) -> &mut Self {
        let offset = self.name_offset(name);
        self.word(FDT_PROP);
        self.word((cells.len() * 4) as u32);
        self.word(offset);
        for &cell in cells {
            self.word(cell);
        }
        self
    }

    /// A property of 64-bit values, each as two cells, high first: addresses and sizes with
    /// `#address-cells` and `#size-cells` 2.
    pub fn wide_cells(&mut self, name: &[u8], values: &[u64]) -> &mut Self {
        let offset = self.name_offset(name);
        self.word(FDT_PROP);
        self.word((values.len() * 8) as u32);
        self.word(offset);
        for &value in values {
            self.word((value >> 32) as u32);
            self.word(value as u32);
        }
        self
    }

    /// A string property: `value` and its terminating NUL.
    pub fn string(&mut self, name: &[u8], value: &[u8]) -> &mut Self {
        let offset = self.name_offset(name);
        self.word(FDT_PROP);
        self.word(value.len() as u32 + 1);
        self.word(offset);
        self.bytes(value);
        self.bytes(&[0]);
        self.pad();
        self
    }

    /// Ends the structure, writes the names and the header; the tree's length, or `None` when
    /// it did not fit or a node was left open.
    pub fn finish(mut self) -> Option<usize> {
        self.word(FDT_END);
        let structure_start = HEADER_LENGTH + RESERVATIONS_LENGTH;
        let structure_length = self.at - structure_start;
        let strings_start = self.at;
        let strings = self.strings;
        let strings_length = self.strings_length;
        self.bytes(&strings[..strings_length]);
        if self.failed || self.depth != 0 {
            return None;
        }
        let total = self.at;
        let header = [
            MAGIC,
            total as u32,
            structure_start as u32,
            strings_start as u32,
            HEADER_LENGTH as u32,
            SUPPORTED_VERSION,
            LAST_COMPATIBLE_VERSION,
            0,
            strings_length as u32,
            structure_length as u32,
        ];
        for (index, word) in header.into_iter().enumerate() {
            self.out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        self.out[HEADER_LENGTH..structure_start].fill(0);
        Some(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fdt::Fdt;

    #[test]
    fn a_written_tree_reads_back() {
        let mut buffer = [0u8; 1024];
        let mut writer = Writer::new(&mut buffer);
        writer
            .begin(b"")
            .cells(b"#address-cells", &[2])
            .cells(b"#size-cells", &[2])
            .begin(b"memory@0")
            .string(b"device_type", b"memory")
            .wide_cells(b"reg", &[0, 256 << 20])
            .end()
            .begin(b"serial@1fe001e0")
            .string(b"compatible", b"ns16550a")
            .wide_cells(b"reg", &[0x1fe0_01e0, 8])
            .end()
            .end();
        let length = writer.finish().unwrap();
        let tree = Fdt::new(&buffer[..length]).unwrap();
        let serial = tree
            .nodes()
            .map_while(Result::ok)
            .find(|node| node.is_compatible(b"ns16550a"))
            .unwrap();
        assert_eq!(serial.reg_entries().next(), Some((0x1fe0_01e0, 8)));
    }

    #[test]
    fn a_tree_too_large_for_the_buffer_fails() {
        let mut buffer = [0u8; 64];
        let mut writer = Writer::new(&mut buffer);
        writer
            .begin(b"")
            .string(b"model", b"a name longer than the buffer can hold")
            .end();
        assert_eq!(writer.finish(), None);
    }
}
