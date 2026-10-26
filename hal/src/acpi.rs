//! Reading the ACPI tables the firmware leaves in memory, as far as the kernel needs them
//! today: enough to power the machine off (sleep state S5) and to find the CPUs (the MADT).
//!
//! Layouts are from the public ACPI Specification (6.5): RSDP (5.2.5), system description
//! table header (5.2.6), RSDT/XSDT (5.2.7, 5.2.8), FADT (5.2.9), MADT (5.2.12), the `\_S5`
//! object (7.4.2) and
//! the AML encoding of `Name`/`Package` (20.2). ACPI is not tied to one architecture (x86 PCs
//! and Arm servers both use it), so this module is architecture-independent and works on byte
//! slices; architecture code only turns physical addresses into slices.

/// "RSD PTR ", the RSDP signature.
pub const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";
/// Bytes covered by the ACPI 1.0 RSDP checksum.
const RSDP_V1_LENGTH: usize = 20;
/// Length of the ACPI 2.0+ RSDP.
pub const RSDP_V2_LENGTH: usize = 36;
/// Length of every system description table header.
pub const TABLE_HEADER_LENGTH: usize = 36;

/// FADT field offsets (ACPI 6.5, table 5.9).
const FADT_DSDT: usize = 40;
const FADT_SMI_CMD: usize = 48;
const FADT_ACPI_ENABLE: usize = 52;
const FADT_PM1A_CNT_BLK: usize = 64;
const FADT_PM1B_CNT_BLK: usize = 68;
const FADT_X_DSDT: usize = 140;

/// Where the MADT's interrupt controller structures start: after the header, the local
/// interrupt controller address and the flags (ACPI 6.5, table 5.19).
const MADT_ENTRIES: usize = 44;
/// MADT structure type 0, Processor Local APIC (5.2.12.2): 8 bytes, the APIC id at 3 and the
/// flags at 4, whose bit 0 says the processor is usable.
const MADT_LOCAL_APIC: u8 = 0;
const MADT_LOCAL_APIC_LENGTH: usize = 8;
const MADT_LOCAL_APIC_ENABLED: u32 = 1;
/// MADT structure type 1, I/O APIC (5.2.12.3): 12 bytes, the registers' address at 4 and the
/// first global system interrupt it handles at 8.
const MADT_IO_APIC: u8 = 1;
const MADT_IO_APIC_LENGTH: usize = 12;
/// MADT structure type 2, Interrupt Source Override (5.2.12.5): 10 bytes, the bus at 2 (0 is
/// ISA), the ISA line at 3, its global system interrupt at 4 and the MPS INTI flags at 8.
const MADT_SOURCE_OVERRIDE: u8 = 2;
const MADT_SOURCE_OVERRIDE_LENGTH: usize = 10;
const MADT_BUS_ISA: u8 = 0;
const MPS_POLARITY_MASK: u16 = 0b11;
const MPS_POLARITY_ACTIVE_LOW: u16 = 0b11;
const MPS_TRIGGER_MASK: u16 = 0b11 << 2;
const MPS_TRIGGER_LEVEL: u16 = 0b11 << 2;

/// Where the root table list lives, from the RSDP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootTable {
    /// ACPI 1.0 RSDT: 32-bit table addresses.
    Rsdt(u64),
    /// ACPI 2.0+ XSDT: 64-bit table addresses.
    Xsdt(u64),
}

/// Validates an RSDP and returns where its root table is. `rsdp` holds at least the bytes
/// the RSDP's revision says it has (20 for revision 0, 36 otherwise).
pub fn parse_rsdp(rsdp: &[u8]) -> Option<RootTable> {
    if rsdp.get(..8)? != RSDP_SIGNATURE || !checksum_ok(rsdp.get(..RSDP_V1_LENGTH)?) {
        return None;
    }
    let revision = rsdp[15];
    if revision >= 2 {
        // A bad extended length or checksum only rules out the XSDT; the ACPI 1.0 part is
        // already valid, so fall back to the RSDT.
        let extended_ok = read_u32(rsdp, 20)
            .and_then(|length| rsdp.get(..usize::try_from(length).ok()?))
            .is_some_and(|extended| extended.len() >= RSDP_V2_LENGTH && checksum_ok(extended));
        let xsdt = read_u64(rsdp, 24).unwrap_or(0);
        if extended_ok && xsdt != 0 {
            return Some(RootTable::Xsdt(xsdt));
        }
    }
    Some(RootTable::Rsdt(read_u32(rsdp, 16)?.into()))
}

/// Finds an RSDP in a legacy BIOS area (the spec's 16-byte-aligned search), returning its
/// offset in `area`.
pub fn find_rsdp(area: &[u8]) -> Option<usize> {
    (0..area.len().saturating_sub(RSDP_V1_LENGTH - 1))
        .step_by(16)
        .find(|&offset| parse_rsdp(&area[offset..]).is_some())
}

/// The table length a header declares, for sizing the slice to validate it.
pub fn table_length(header: &[u8]) -> Option<usize> {
    usize::try_from(read_u32(header, 4)?).ok()
}

/// Validates a complete table (`table` is exactly its declared length) with signature
/// `signature`.
pub fn table_ok(table: &[u8], signature: &[u8; 4]) -> bool {
    table.len() >= TABLE_HEADER_LENGTH
        && &table[..4] == signature
        && table_length(table) == Some(table.len())
        && checksum_ok(table)
}

/// The addresses of the tables a validated RSDT or XSDT lists.
pub fn root_table_entries(root: &[u8], kind: RootTable) -> impl Iterator<Item = u64> + '_ {
    let width = match kind {
        RootTable::Rsdt(_) => 4,
        RootTable::Xsdt(_) => 8,
    };
    root.get(TABLE_HEADER_LENGTH..)
        .unwrap_or(&[])
        .chunks_exact(width)
        .map(move |entry| match width {
            4 => read_u32(entry, 0).map_or(0, u64::from),
            _ => read_u64(entry, 0).unwrap_or(0),
        })
}

/// What the FADT says about powering off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fadt {
    /// Physical address of the DSDT, whose AML defines `\_S5`.
    pub dsdt: u64,
    /// I/O port to write `acpi_enable` to to switch from legacy to ACPI mode; 0 if the
    /// machine is always in ACPI mode.
    pub smi_command_port: u32,
    /// Value for `smi_command_port`.
    pub acpi_enable: u8,
    /// I/O port of the PM1a control register.
    pub pm1a_control_port: u32,
    /// I/O port of the PM1b control register; 0 if there is none.
    pub pm1b_control_port: u32,
}

/// Reads the power-off fields of a validated FADT (signature "FACP").
pub fn parse_fadt(fadt: &[u8]) -> Option<Fadt> {
    let x_dsdt = read_u64(fadt, FADT_X_DSDT).unwrap_or(0);
    let dsdt = if x_dsdt != 0 {
        x_dsdt
    } else {
        read_u32(fadt, FADT_DSDT)?.into()
    };
    Some(Fadt {
        dsdt,
        smi_command_port: read_u32(fadt, FADT_SMI_CMD)?,
        acpi_enable: *fadt.get(FADT_ACPI_ENABLE)?,
        pm1a_control_port: read_u32(fadt, FADT_PM1A_CNT_BLK)?,
        pm1b_control_port: read_u32(fadt, FADT_PM1B_CNT_BLK)?,
    })
}

/// Calls `cpu` with the local APIC id of every usable processor a validated MADT (signature
/// "APIC") lists, in its order: each Processor Local APIC structure whose Enabled flag is set.
/// Processors that are only online capable (hot-pluggable, not usable until enabled) are
/// skipped, as are Processor Local x2APIC structures (type 9), which the specification uses for
/// ids of 255 and up, beyond what xAPIC mode reaches. A local APIC structure too short to hold
/// its flags is skipped; a structure whose length runs past the table, or is below 2, ends the
/// walk.
pub fn madt_local_apic_ids(madt: &[u8], cpu: &mut dyn FnMut(u8)) {
    for entry in madt_structures(madt, MADT_LOCAL_APIC, MADT_LOCAL_APIC_LENGTH) {
        if read_u32(entry, 4).is_some_and(|flags| flags & MADT_LOCAL_APIC_ENABLED != 0) {
            cpu(entry[3]);
        }
    }
}

/// The MADT's structures of type `kind` at least `minimum_length` bytes long, in order. A
/// structure whose length runs past the table, or is below 2, ends the walk.
fn madt_structures(madt: &[u8], kind: u8, minimum_length: usize) -> impl Iterator<Item = &[u8]> {
    let mut at = MADT_ENTRIES;
    core::iter::from_fn(move || {
        let length = usize::from(*madt.get(at + 1)?);
        let entry = madt
            .get(at..at.checked_add(length)?)
            .filter(|_| length >= 2)?;
        at += length;
        Some(entry)
    })
    .filter(move |entry| entry[0] == kind && entry.len() >= minimum_length)
}

/// An I/O APIC a MADT lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoApic {
    /// Physical address of its registers.
    pub address: u64,
    /// The global system interrupt its line 0 receives.
    pub first_interrupt: u32,
}

/// The I/O APICs a validated MADT lists, in its order.
pub fn madt_io_apics(madt: &[u8]) -> impl Iterator<Item = IoApic> + '_ {
    madt_structures(madt, MADT_IO_APIC, MADT_IO_APIC_LENGTH).filter_map(|entry| {
        Some(IoApic {
            address: read_u32(entry, 4)?.into(),
            first_interrupt: read_u32(entry, 8)?,
        })
    })
}

/// Where an ISA interrupt line arrives and how it signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsaInterrupt {
    /// Its global system interrupt.
    pub global_interrupt: u32,
    /// Asserted low.
    pub active_low: bool,
    /// Pending while asserted rather than on an edge.
    pub level_triggered: bool,
}

/// Where ISA line `line` arrives: as a validated MADT's first Interrupt Source Override for it
/// says, or, with none, at the global system interrupt of the same number, active high and
/// edge-triggered, as ISA lines signal (5.2.12.5). Flags that say "conforms to the bus" keep
/// ISA's signalling.
pub fn madt_isa_interrupt(madt: &[u8], line: u8) -> IsaInterrupt {
    madt_structures(madt, MADT_SOURCE_OVERRIDE, MADT_SOURCE_OVERRIDE_LENGTH)
        .find(|entry| entry[2] == MADT_BUS_ISA && entry[3] == line)
        .and_then(|entry| {
            let flags = u16::from_le_bytes([entry[8], entry[9]]);
            Some(IsaInterrupt {
                global_interrupt: read_u32(entry, 4)?,
                active_low: flags & MPS_POLARITY_MASK == MPS_POLARITY_ACTIVE_LOW,
                level_triggered: flags & MPS_TRIGGER_MASK == MPS_TRIGGER_LEVEL,
            })
        })
        .unwrap_or(IsaInterrupt {
            global_interrupt: line.into(),
            active_low: false,
            level_triggered: false,
        })
}

/// The `SLP_TYPa` and `SLP_TYPb` values for sleep state S5 (soft off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct S5SleepType {
    pub a: u8,
    pub b: u8,
}

/// AML opcodes used by a `Name(_S5, Package() { ... })` definition.
const AML_NAME_OP: u8 = 0x08;
const AML_PACKAGE_OP: u8 = 0x12;
const AML_ZERO_OP: u8 = 0x00;
const AML_ONE_OP: u8 = 0x01;
const AML_BYTE_PREFIX: u8 = 0x0a;

/// Finds `Name(_S5, Package() { SLP_TYPa, SLP_TYPb, ... })` in DSDT AML and returns its
/// sleep types. This reads the one static object it needs without running an AML
/// interpreter, which firmware that defines `\_S5` as a plain package allows.
pub fn find_s5_sleep_type(aml: &[u8]) -> Option<S5SleepType> {
    aml.windows(4)
        .enumerate()
        .filter(|(_, name)| *name == b"_S5_")
        .find_map(|(at, _)| {
            // `Name` is `NameOp NameString`; the name may carry a root prefix `\`.
            let named = match at.checked_sub(1).map(|i| aml[i]) {
                Some(AML_NAME_OP) => true,
                Some(b'\\') => at >= 2 && aml[at - 2] == AML_NAME_OP,
                _ => false,
            };
            if !named {
                return None;
            }
            parse_s5_package(aml.get(at + 4..)?)
        })
}

fn parse_s5_package(aml: &[u8]) -> Option<S5SleepType> {
    if *aml.first()? != AML_PACKAGE_OP {
        return None;
    }
    // PkgLength: bits 7-6 of the lead byte give how many more bytes follow (20.2.4).
    let package_length_bytes = 1 + usize::from(aml.get(1)? >> 6);
    let element_count_at = 1 + package_length_bytes;
    if *aml.get(element_count_at)? < 2 {
        return None;
    }
    let mut rest = aml.get(element_count_at + 1..)?;
    let mut next_integer = || -> Option<u8> {
        let (value, used) = match *rest.first()? {
            AML_ZERO_OP => (0, 1),
            AML_ONE_OP => (1, 1),
            AML_BYTE_PREFIX => (*rest.get(1)?, 2),
            _ => return None,
        };
        rest = &rest[used..];
        Some(value)
    };
    let a = next_integer()?;
    let b = next_integer()?;
    Some(S5SleepType { a, b })
}

/// PM1 control register bits (ACPI 6.5, table 4.16).
pub const PM1_CONTROL_SCI_EN: u16 = 1 << 0;
const PM1_CONTROL_SLP_TYP_SHIFT: u32 = 10;
const PM1_CONTROL_SLP_TYP_MASK: u16 = 0b111 << PM1_CONTROL_SLP_TYP_SHIFT;
pub const PM1_CONTROL_SLP_EN: u16 = 1 << 13;

/// The PM1 control register value that enters the sleep state with type `sleep_type`, given
/// the register's `current` value: `SLP_TYPx` and `SLP_EN` are set, every other bit is
/// preserved as the spec requires.
pub const fn pm1_control_sleep(current: u16, sleep_type: u8) -> u16 {
    let slp_typ = ((sleep_type as u16) << PM1_CONTROL_SLP_TYP_SHIFT) & PM1_CONTROL_SLP_TYP_MASK;
    (current & !(PM1_CONTROL_SLP_TYP_MASK | PM1_CONTROL_SLP_EN)) | slp_typ | PM1_CONTROL_SLP_EN
}

fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte)) == 0
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sets the byte at `at` so that `bytes` sums to zero.
    fn fix_checksum(bytes: &mut [u8], at: usize) {
        bytes[at] = 0;
        let sum = bytes.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte));
        bytes[at] = sum.wrapping_neg();
    }

    fn rsdp(revision: u8, rsdt: u32, xsdt: u64) -> [u8; RSDP_V2_LENGTH] {
        let mut rsdp = [0u8; RSDP_V2_LENGTH];
        rsdp[..8].copy_from_slice(RSDP_SIGNATURE);
        rsdp[15] = revision;
        rsdp[16..20].copy_from_slice(&rsdt.to_le_bytes());
        rsdp[20..24].copy_from_slice(&(RSDP_V2_LENGTH as u32).to_le_bytes());
        rsdp[24..32].copy_from_slice(&xsdt.to_le_bytes());
        fix_checksum(&mut rsdp[..RSDP_V1_LENGTH], 8);
        fix_checksum(&mut rsdp, 32);
        rsdp
    }

    fn table(signature: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut table = vec![0u8; TABLE_HEADER_LENGTH];
        table[..4].copy_from_slice(signature);
        table.extend_from_slice(body);
        let length = table.len() as u32;
        table[4..8].copy_from_slice(&length.to_le_bytes());
        fix_checksum(&mut table, 9);
        table
    }

    fn madt_ids(body: &[u8]) -> Vec<u8> {
        let madt = table(b"APIC", &[&[0u8; 8][..], body].concat());
        let mut ids = Vec::new();
        madt_local_apic_ids(&madt, &mut |id| ids.push(id));
        ids
    }

    /// A Processor Local APIC structure.
    fn local_apic(id: u8, flags: u32) -> Vec<u8> {
        [&[0, 8, id, id][..], &flags.to_le_bytes()].concat()
    }

    #[test]
    fn madt_lists_enabled_local_apics_and_skips_other_structures() {
        let io_apic = [1u8, 12, 0, 0, 0, 0, 0xc0, 0xfe, 0, 0, 0, 0];
        let x2apic = [9u8, 16, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
        let body = [
            local_apic(0, 1),
            local_apic(1, 1),
            io_apic.to_vec(),
            local_apic(2, 0), // disabled
            local_apic(3, 2), // online capable only
            x2apic.to_vec(),
            local_apic(6, 3),
        ]
        .concat();
        assert_eq!(madt_ids(&body), [0, 1, 6]);
    }

    fn madt(body: &[u8]) -> Vec<u8> {
        table(b"APIC", &[&[0u8; 8][..], body].concat())
    }

    fn io_apic(address: u32, first_interrupt: u32) -> Vec<u8> {
        [
            &[1u8, 12, 0, 0][..],
            &address.to_le_bytes(),
            &first_interrupt.to_le_bytes(),
        ]
        .concat()
    }

    fn source_override(bus: u8, line: u8, global_interrupt: u32, flags: u16) -> Vec<u8> {
        [
            &[2u8, 10, bus, line][..],
            &global_interrupt.to_le_bytes(),
            &flags.to_le_bytes(),
        ]
        .concat()
    }

    #[test]
    fn madt_lists_io_apics_with_their_first_interrupt() {
        let body = [
            local_apic(0, 1),
            io_apic(0xfec0_0000, 0),
            source_override(0, 0, 2, 0),
            io_apic(0xfec0_1000, 24),
            vec![1, 8, 0, 0, 0, 0, 0, 0],
        ]
        .concat();
        let found: Vec<IoApic> = madt_io_apics(&madt(&body)).collect();
        assert_eq!(
            found,
            [
                IoApic {
                    address: 0xfec0_0000,
                    first_interrupt: 0
                },
                IoApic {
                    address: 0xfec0_1000,
                    first_interrupt: 24
                },
            ]
        );
    }

    #[test]
    fn isa_lines_follow_their_override_or_keep_their_number_and_isa_signalling() {
        let body = [
            io_apic(0xfec0_0000, 0),
            source_override(0, 0, 2, 0),
            source_override(1, 4, 40, 0b1111),
            source_override(0, 9, 9, 0b1111),
            source_override(0, 4, 20, 0b0101),
        ]
        .concat();
        let madt = madt(&body);
        let isa = |global_interrupt| IsaInterrupt {
            global_interrupt,
            active_low: false,
            level_triggered: false,
        };
        assert_eq!(madt_isa_interrupt(&madt, 0), isa(2));
        assert_eq!(madt_isa_interrupt(&madt, 4), isa(20));
        assert_eq!(madt_isa_interrupt(&madt, 3), isa(3));
        assert_eq!(
            madt_isa_interrupt(&madt, 9),
            IsaInterrupt {
                global_interrupt: 9,
                active_low: true,
                level_triggered: true
            }
        );
    }

    #[test]
    fn madt_stops_at_a_malformed_structure() {
        // A zero length would loop forever; a length past the end is truncated.
        let zero = [local_apic(4, 1), vec![0, 0, 9, 9], local_apic(5, 1)].concat();
        assert_eq!(madt_ids(&zero), [4]);
        let long = [local_apic(4, 1), vec![0, 40, 7, 7, 1, 0, 0, 0]].concat();
        assert_eq!(madt_ids(&long), [4]);
        let short = [local_apic(4, 1), vec![0, 4, 7, 7]].concat();
        assert_eq!(madt_ids(&short), [4]);
    }

    #[test]
    fn rsdp_v1_points_at_rsdt() {
        assert_eq!(
            parse_rsdp(&rsdp(0, 0x1234, 0)[..RSDP_V1_LENGTH]),
            Some(RootTable::Rsdt(0x1234))
        );
    }

    #[test]
    fn rsdp_v2_prefers_xsdt() {
        assert_eq!(
            parse_rsdp(&rsdp(2, 0x1234, 0x5678)),
            Some(RootTable::Xsdt(0x5678))
        );
    }

    #[test]
    fn rsdp_v2_with_oversized_length_falls_back_to_rsdt() {
        let mut long = rsdp(2, 0x1234, 0x5678);
        long[20..24].copy_from_slice(&64u32.to_le_bytes());
        fix_checksum(&mut long[..RSDP_V1_LENGTH], 8);
        assert_eq!(parse_rsdp(&long), Some(RootTable::Rsdt(0x1234)));
    }

    #[test]
    fn rsdp_with_bad_checksum_is_rejected() {
        let mut bad = rsdp(0, 0x1234, 0);
        bad[16] ^= 1;
        assert_eq!(parse_rsdp(&bad), None);
    }

    #[test]
    fn rsdp_search_finds_only_aligned_signatures() {
        let mut area = vec![0u8; 256];
        area[0x48..0x48 + RSDP_V2_LENGTH].copy_from_slice(&rsdp(0, 1, 0));
        assert_eq!(find_rsdp(&area), None, "0x48 is not 16-byte aligned");
        area[0x80..0x80 + RSDP_V2_LENGTH].copy_from_slice(&rsdp(0, 1, 0));
        assert_eq!(find_rsdp(&area), Some(0x80));
    }

    #[test]
    fn root_tables_list_their_entries() {
        let rsdt = table(b"RSDT", &[0x10, 0, 0, 0, 0x20, 0, 0, 0]);
        assert!(table_ok(&rsdt, b"RSDT"));
        let entries: Vec<u64> = root_table_entries(&rsdt, RootTable::Rsdt(0)).collect();
        assert_eq!(entries, [0x10, 0x20]);

        let xsdt = table(b"XSDT", &0x1_0000_0000u64.to_le_bytes());
        let entries: Vec<u64> = root_table_entries(&xsdt, RootTable::Xsdt(0)).collect();
        assert_eq!(entries, [0x1_0000_0000]);
    }

    #[test]
    fn table_with_wrong_signature_length_or_checksum_is_rejected() {
        let facp = table(b"FACP", &[1, 2, 3]);
        assert!(table_ok(&facp, b"FACP"));
        assert!(!table_ok(&facp, b"APIC"));
        assert!(!table_ok(&facp[..facp.len() - 1], b"FACP"));
        let mut corrupt = facp.clone();
        corrupt[TABLE_HEADER_LENGTH] ^= 1;
        assert!(!table_ok(&corrupt, b"FACP"));
    }

    #[test]
    fn fadt_fields_and_x_dsdt_preference() {
        let mut body = vec![0u8; 244 - TABLE_HEADER_LENGTH];
        let mut set32 = |at: usize, value: u32| {
            body[at - TABLE_HEADER_LENGTH..at - TABLE_HEADER_LENGTH + 4]
                .copy_from_slice(&value.to_le_bytes());
        };
        set32(FADT_DSDT, 0x7000);
        set32(FADT_SMI_CMD, 0xb2);
        set32(FADT_PM1A_CNT_BLK, 0x604);
        body[FADT_ACPI_ENABLE - TABLE_HEADER_LENGTH] = 0xf1;
        let fadt = table(b"FACP", &body);
        assert_eq!(
            parse_fadt(&fadt),
            Some(Fadt {
                dsdt: 0x7000,
                smi_command_port: 0xb2,
                acpi_enable: 0xf1,
                pm1a_control_port: 0x604,
                pm1b_control_port: 0,
            })
        );

        let mut with_x_dsdt = fadt.clone();
        with_x_dsdt[FADT_X_DSDT..FADT_X_DSDT + 8].copy_from_slice(&0x9_0000u64.to_le_bytes());
        assert_eq!(parse_fadt(&with_x_dsdt).unwrap().dsdt, 0x9_0000);
    }

    #[test]
    fn s5_package_with_byte_and_zero_elements() {
        // Name (_S5, Package (0x04) { 0x05, Zero, Zero, Zero })
        let aml = [
            0xff,
            AML_NAME_OP,
            b'_',
            b'S',
            b'5',
            b'_',
            AML_PACKAGE_OP,
            0x07,
            0x04,
            AML_BYTE_PREFIX,
            0x05,
            AML_ZERO_OP,
            AML_ZERO_OP,
            AML_ZERO_OP,
        ];
        assert_eq!(find_s5_sleep_type(&aml), Some(S5SleepType { a: 5, b: 0 }));
    }

    #[test]
    fn s5_package_with_root_prefix_and_two_byte_package_length() {
        let aml = [
            AML_NAME_OP,
            b'\\',
            b'_',
            b'S',
            b'5',
            b'_',
            AML_PACKAGE_OP,
            0x40,
            0x00,
            0x02,
            AML_ONE_OP,
            AML_BYTE_PREFIX,
            0x07,
        ];
        assert_eq!(find_s5_sleep_type(&aml), Some(S5SleepType { a: 1, b: 7 }));
    }

    #[test]
    fn s5_reference_that_is_not_a_name_definition_is_skipped() {
        // A method body mentioning _S5_ (no NameOp before it), then the real definition.
        let aml = [
            0x70,
            b'_',
            b'S',
            b'5',
            b'_',
            AML_NAME_OP,
            b'_',
            b'S',
            b'5',
            b'_',
            AML_PACKAGE_OP,
            0x04,
            0x02,
            AML_ZERO_OP,
            AML_ZERO_OP,
        ];
        assert_eq!(find_s5_sleep_type(&aml), Some(S5SleepType { a: 0, b: 0 }));
        assert_eq!(find_s5_sleep_type(&aml[..5]), None);
    }

    #[test]
    fn pm1_control_value_sets_sleep_type_and_preserves_other_bits() {
        assert_eq!(pm1_control_sleep(0, 5), (5 << 10) | (1 << 13));
        // SCI_EN stays set; a stale SLP_TYP is replaced, not ORed.
        assert_eq!(pm1_control_sleep(0b111 << 10 | 1, 0), (1 << 13) | 1);
        assert_eq!(pm1_control_sleep(0, 0xff), (0b111 << 10) | (1 << 13));
    }
}
