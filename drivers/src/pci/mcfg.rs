//! The ACPI MCFG table (PCI Firmware Specification 3.x, 4.1.2): where each PCI segment's ECAM
//! window is and which buses it covers.

const ENTRIES: usize = 44;
const ENTRY_LENGTH: usize = 16;

/// One ECAM window the MCFG describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EcamRegion {
    /// Physical address of the window, where bus 0's space would be (the window's own first
    /// bus is `start_bus`, at `base + (start_bus << 20)`).
    pub base: u64,
    pub segment: u16,
    pub start_bus: u8,
    pub end_bus: u8,
}

/// The ECAM windows the MCFG `table` lists (the whole table, its 36-byte ACPI header
/// included, checksum already validated). Entries whose bus range is empty, and a last entry
/// cut short by the table's end, are skipped.
pub fn regions(table: &[u8]) -> impl Iterator<Item = EcamRegion> + '_ {
    table
        .get(ENTRIES..)
        .unwrap_or(&[])
        .as_chunks::<ENTRY_LENGTH>()
        .0
        .iter()
        .filter_map(|entry| {
            let region = EcamRegion {
                base: u64::from_le_bytes(entry[..8].try_into().ok()?),
                segment: u16::from_le_bytes(entry[8..10].try_into().ok()?),
                start_bus: entry[10],
                end_bus: entry[11],
            };
            (region.start_bus <= region.end_bus).then_some(region)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[(u64, u16, u8, u8)]) -> Vec<u8> {
        let mut bytes = b"MCFG".to_vec();
        let length = (ENTRIES + entries.len() * ENTRY_LENGTH) as u32;
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.resize(ENTRIES, 0);
        for &(base, segment, start, end) in entries {
            bytes.extend_from_slice(&base.to_le_bytes());
            bytes.extend_from_slice(&segment.to_le_bytes());
            bytes.extend_from_slice(&[start, end, 0, 0, 0, 0]);
        }
        bytes
    }

    #[test]
    fn reads_qemu_q35s_window() {
        let regions: Vec<_> = regions(&table(&[(0xb000_0000, 0, 0, 255)])).collect();
        assert_eq!(
            regions,
            [EcamRegion {
                base: 0xb000_0000,
                segment: 0,
                start_bus: 0,
                end_bus: 255,
            }]
        );
    }

    #[test]
    fn reads_every_segment_and_skips_empty_bus_ranges() {
        let bytes = table(&[
            (0x1_0000_0000, 1, 0x10, 0x1f),
            (0xc000_0000, 2, 5, 4),
            (0xe000_0000, 3, 7, 7),
        ]);
        let segments: Vec<_> = regions(&bytes)
            .map(|r| (r.segment, r.base, r.start_bus, r.end_bus))
            .collect();
        assert_eq!(
            segments,
            [(1, 0x1_0000_0000, 0x10, 0x1f), (3, 0xe000_0000, 7, 7)]
        );
    }

    #[test]
    fn short_and_empty_tables_give_what_they_hold() {
        assert_eq!(regions(&table(&[])).count(), 0);
        assert_eq!(regions(&[]).count(), 0);
        assert_eq!(regions(&table(&[])[..20]).count(), 0);
        let bytes = table(&[(0xb000_0000, 0, 0, 255), (0xc000_0000, 1, 0, 1)]);
        assert_eq!(regions(&bytes[..bytes.len() - 1]).count(), 1);
    }
}
