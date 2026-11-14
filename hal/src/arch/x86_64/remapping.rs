//! Interrupt remapping (D594): Intel VT-d's remapping hardware units, from the ACPI `DMAR`
//! table, and AMD-Vi's IOMMUs, from the `IVRS` table, so device interrupts reach CPUs whose
//! x2APIC IDs are 256 and above. Each device line owns the remapping table entry of its index;
//! a message names its entry, and the entry names the vector and the destination CPU. VT-d's
//! entries are 128 bits wide with a 32-bit destination; AMD-Vi's are the 128-bit
//! guest-virtual-APIC format with guest mode off. Learned from Intel's VT-d specification
//! (chapters 5, 6 and 10) and AMD's I/O virtualization specification (chapter 2 and 3).

use super::{BOOT_DIRECT_MAP_END, DIRECT_MAP_OFFSET, acpi_table};
use crate::DeviceLine;
use crate::InterruptMessage;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use eggos_drivers::io::{Mmio, RegisterIo};

const MESSAGE_ADDRESS: u64 = 0xfee0_0000;
const PAGE: usize = 4096;
const ENTRY_BYTES: usize = 16;
const MAX_UNITS: usize = 8;
const SPIN_LIMIT: u32 = 10_000_000;

const NONE: u8 = 0;
const INTEL: u8 = 1;
const AMD: u8 = 2;

static KIND: AtomicU8 = AtomicU8::new(NONE);
static TABLE: AtomicUsize = AtomicUsize::new(0);
static UNITS: [AtomicUsize; MAX_UNITS] = [const { AtomicUsize::new(0) }; MAX_UNITS];
static QUEUES: [AtomicUsize; MAX_UNITS] = [const { AtomicUsize::new(0) }; MAX_UNITS];
static AMD_DEVICES: [AtomicUsize; MAX_UNITS] = [const { AtomicUsize::new(0) }; MAX_UNITS];

/// Memory the kernel gives: a zeroed, page-aligned block of at least the bytes asked, as its
/// physical address and its address in the direct map.
pub(super) type Memory<'a> = &'a mut dyn FnMut(usize) -> Option<(u64, usize)>;

/// Whether remapping is on: lines then take [`message`] and [`io_apic_entry`].
pub(super) fn is_on() -> bool {
    KIND.load(Ordering::Acquire) != NONE
}

fn registers(base: u64, bytes: usize) -> Option<Mmio> {
    if base == 0 || base + bytes as u64 > BOOT_DIRECT_MAP_END {
        return None;
    }
    // SAFETY: the ACPI table names this block as a remapping unit's registers, in the boot
    // direct map (checked above), which nothing else drives.
    Some(unsafe { Mmio::new(DIRECT_MAP_OFFSET + base as usize, bytes) })
}

fn spin_until(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..SPIN_LIMIT {
        if done() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

const TIMER_ENTRY: usize = DeviceLine::COUNT;

fn table_entry(index: usize) -> Option<*mut u64> {
    let table = TABLE.load(Ordering::Acquire);
    (table != 0 && index <= TIMER_ENTRY).then(|| (table + index * ENTRY_BYTES) as *mut u64)
}

fn write_entry(index: usize, low: u64, high: u64) {
    let Some(entry) = table_entry(index) else {
        return;
    };
    // SAFETY: the table is the kernel's block given at `enable`, kept for good, 16-byte
    // entries; the low half carries the present bit, so it is cleared first and written last,
    // and the unit never sees a half-written entry as present.
    unsafe {
        entry.write_volatile(0);
        entry.add(1).write_volatile(high);
        entry.write_volatile(low);
    }
}

/// Turns remapping on with the first kind the firmware describes, Intel's then AMD's.
pub(super) fn enable(memory: Memory<'_>) -> Result<(), &'static str> {
    if is_on() {
        return Ok(());
    }
    let enabled = if let Some(dmar) = acpi_table(b"DMAR") {
        enable_intel(dmar, memory)
    } else if let Some(ivrs) = acpi_table(b"IVRS") {
        enable_amd(ivrs, memory)
    } else {
        Err("the firmware describes no interrupt remapping unit")
    };
    if enabled.is_ok() {
        super::timer::after_remapping();
    }
    enabled
}

/// The message that raises `line`'s vector on the CPU with APIC ID `cpu` through its entry.
pub(super) fn message(line: DeviceLine, vector: u8, cpu: u32) -> InterruptMessage {
    indexed_message(line.index(), vector, cpu)
}

/// The HPET timer's message (D594), through the entry after the device lines', as a
/// comparator's route takes it: address, then data.
pub(super) fn timer_message(vector: u8, cpu: u32) -> (u32, u32) {
    let message = indexed_message(TIMER_ENTRY, vector, cpu);
    (message.address as u32, message.data)
}

fn indexed_message(entry: usize, vector: u8, cpu: u32) -> InterruptMessage {
    let index = entry as u64;
    set_entry(entry, vector, cpu, false);
    match KIND.load(Ordering::Acquire) {
        AMD => InterruptMessage {
            address: MESSAGE_ADDRESS,
            data: index as u32,
        },
        _ => InterruptMessage {
            address: MESSAGE_ADDRESS | (index & 0x7fff) << 5 | INTEL_REMAPPABLE | (index >> 15) << 2,
            data: 0,
        },
    }
}

/// What an I/O APIC entry routed through remapping holds instead of a destination: the low
/// word's vector field and the high word, for `line` to raise `vector` on `cpu`, level-
/// triggered when `level` (D594).
pub(super) fn io_apic_entry(line: DeviceLine, vector: u8, cpu: u32, level: bool) -> (u8, u32) {
    let index = line.index() as u32;
    set_entry(line.index(), vector, cpu, level);
    match KIND.load(Ordering::Acquire) {
        AMD => (index as u8, 0),
        _ => (vector, INTEL_IO_APIC_REMAPPABLE | (index & 0x7fff) << 17),
    }
}

fn set_entry(index: usize, vector: u8, cpu: u32, level: bool) {
    match KIND.load(Ordering::Acquire) {
        INTEL => {
            let trigger = if level { INTEL_ENTRY_LEVEL } else { 0 };
            let low = INTEL_ENTRY_PRESENT | trigger | u64::from(vector) << 16 | u64::from(cpu) << 32;
            write_entry(index, low, 0);
            intel_invalidate(Some(index));
        }
        AMD => {
            let low = AMD_ENTRY_REMAP | u64::from(cpu & 0x00ff_ffff) << 8;
            let high = u64::from(vector) | u64::from(cpu >> 24) << 56;
            write_entry(index, low, high);
            amd_invalidate();
        }
        _ => {}
    }
}

const INTEL_REMAPPABLE: u64 = 1 << 4;
const INTEL_IO_APIC_REMAPPABLE: u32 = 1 << 16;
const INTEL_ENTRY_PRESENT: u64 = 1;
const INTEL_ENTRY_LEVEL: u64 = 1 << 4;

const DMAR_FLAGS: usize = 37;
const DMAR_INTERRUPT_REMAPPING: u8 = 1;
const DMAR_STRUCTURES: usize = 48;
const DMAR_HARDWARE_UNIT: u16 = 0;
const DRHD_REGISTER_BASE: usize = 8;

const VTD_REGISTER_BYTES: usize = 0x1000;
const VTD_EXTENDED_CAPABILITY: usize = 0x10;
const VTD_GLOBAL_COMMAND: usize = 0x18;
const VTD_GLOBAL_STATUS: usize = 0x1c;
const VTD_QUEUE_HEAD: usize = 0x80;
const VTD_QUEUE_TAIL: usize = 0x88;
const VTD_QUEUE_ADDRESS: usize = 0x90;
const VTD_TABLE_ADDRESS: usize = 0xb8;
const VTD_HAS_QUEUE: u64 = 1 << 1;
const VTD_HAS_REMAPPING: u64 = 1 << 3;
const VTD_HAS_EXTENDED_IDS: u64 = 1 << 4;
const VTD_QUEUE_ENABLE: u32 = 1 << 26;
const VTD_REMAPPING_ENABLE: u32 = 1 << 25;
const VTD_SET_TABLE: u32 = 1 << 24;
const VTD_ONE_SHOT_BITS: u32 = VTD_SET_TABLE | 1 << 30 | 1 << 29 | 1 << 27;
const VTD_EXTENDED_MODE: u64 = 1 << 11;
const VTD_TABLE_SIZE_256: u64 = 7;
const VTD_INVALIDATE_ENTRY_CACHE: u64 = 0x4;
const VTD_INVALIDATE_ONE: u64 = 1 << 4;
const VTD_QUEUE_ENTRIES: usize = PAGE / ENTRY_BYTES;

fn intel_units(dmar: &[u8], each: &mut dyn FnMut(u64)) {
    let mut at = DMAR_STRUCTURES;
    while let Some(header) = dmar.get(at..at + 4) {
        let kind = u16::from_le_bytes([header[0], header[1]]);
        let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if length < 4 {
            return;
        }
        if kind == DMAR_HARDWARE_UNIT
            && let Some(base) = dmar.get(at + DRHD_REGISTER_BASE..at + DRHD_REGISTER_BASE + 8)
        {
            each(u64::from_le_bytes(base.try_into().unwrap_or_default()));
        }
        at += length;
    }
}

fn vtd_command(unit: &Mmio, bit: u32) -> bool {
    let status = unit.read32(VTD_GLOBAL_STATUS) & !VTD_ONE_SHOT_BITS;
    unit.write32(VTD_GLOBAL_COMMAND, status | bit);
    spin_until(|| unit.read32(VTD_GLOBAL_STATUS) & bit != 0)
}

fn enable_intel(dmar: &[u8], memory: Memory<'_>) -> Result<(), &'static str> {
    if dmar.get(DMAR_FLAGS).is_none_or(|flags| flags & DMAR_INTERRUPT_REMAPPING == 0) {
        return Err("the DMAR table says interrupt remapping is not supported");
    }
    let mut bases = [0u64; MAX_UNITS];
    let mut count = 0;
    intel_units(dmar, &mut |base| {
        if count < MAX_UNITS {
            bases[count] = base;
            count += 1;
        }
    });
    if count == 0 {
        return Err("the DMAR table lists no remapping unit");
    }
    let (table_physical, table) = memory(PAGE).ok_or("no memory for the remapping table")?;
    TABLE.store(table, Ordering::Release);
    for (slot, &base) in bases[..count].iter().enumerate() {
        let unit = registers(base, VTD_REGISTER_BYTES).ok_or("a remapping unit is out of reach")?;
        let capabilities = unit.read64(VTD_EXTENDED_CAPABILITY);
        let needed = VTD_HAS_QUEUE | VTD_HAS_REMAPPING | VTD_HAS_EXTENDED_IDS;
        if capabilities & needed != needed {
            return Err("a remapping unit lacks the queue, remapping or 32-bit destinations");
        }
        let (queue_physical, queue) = memory(PAGE).ok_or("no memory for an invalidation queue")?;
        QUEUES[slot].store(queue, Ordering::Release);
        unit.write64(VTD_QUEUE_TAIL, 0);
        unit.write64(VTD_QUEUE_ADDRESS, queue_physical);
        if !vtd_command(&unit, VTD_QUEUE_ENABLE) {
            return Err("a remapping unit did not start its invalidation queue");
        }
        unit.write64(
            VTD_TABLE_ADDRESS,
            table_physical | VTD_EXTENDED_MODE | VTD_TABLE_SIZE_256,
        );
        if !vtd_command(&unit, VTD_SET_TABLE) {
            return Err("a remapping unit did not take the remapping table");
        }
        UNITS[slot].store(DIRECT_MAP_OFFSET + base as usize, Ordering::Release);
    }
    KIND.store(INTEL, Ordering::Release);
    intel_invalidate(None);
    for &base in &bases[..count] {
        let unit = registers(base, VTD_REGISTER_BYTES).ok_or("a remapping unit is out of reach")?;
        if !vtd_command(&unit, VTD_REMAPPING_ENABLE) {
            return Err("a remapping unit did not turn remapping on");
        }
    }
    Ok(())
}

/// Has every unit drop what it cached of entry `index`, or of every entry, and waits until it
/// has.
fn intel_invalidate(index: Option<usize>) {
    let descriptor = match index {
        Some(index) => VTD_INVALIDATE_ENTRY_CACHE | VTD_INVALIDATE_ONE | (index as u64) << 32,
        None => VTD_INVALIDATE_ENTRY_CACHE,
    };
    for slot in 0..MAX_UNITS {
        let (unit, queue) = (
            UNITS[slot].load(Ordering::Acquire),
            QUEUES[slot].load(Ordering::Acquire),
        );
        if unit == 0 || queue == 0 {
            continue;
        }
        // SAFETY: `unit` is a remapping unit's registers in the direct map, recorded by
        // `enable_intel`, which nothing else drives.
        let registers = unsafe { Mmio::new(unit, VTD_REGISTER_BYTES) };
        let tail = registers.read64(VTD_QUEUE_TAIL) as usize;
        let place = (queue + tail) as *mut u64;
        // SAFETY: the queue is the kernel's page given at `enable`, kept for good; `tail` is a
        // multiple of 16 below its size, the slot the unit reads next, which it has finished
        // with once the head reached the tail.
        unsafe {
            place.write_volatile(descriptor);
            place.add(1).write_volatile(0);
        }
        let next = (tail + ENTRY_BYTES) % (VTD_QUEUE_ENTRIES * ENTRY_BYTES);
        registers.write64(VTD_QUEUE_TAIL, next as u64);
        spin_until(|| registers.read64(VTD_QUEUE_HEAD) as usize == next);
    }
}

const AMD_ENTRY_REMAP: u64 = 1;
const IVRS_STRUCTURES: usize = 48;
const IVHD_LEGACY: u8 = 0x10;
const IVHD_EXTENDED: u8 = 0x11;
const IVHD_MIXED: u8 = 0x40;
const IVHD_BASE: usize = 8;
const IVHD_LEGACY_ENTRIES: usize = 24;
const IVHD_EXTENDED_ENTRIES: usize = 40;
const AMD_REGISTER_BYTES: usize = 0x4000;
const AMD_DEVICE_TABLE: usize = 0x00;
const AMD_COMMAND_BUFFER: usize = 0x08;
const AMD_CONTROL: usize = 0x18;
const AMD_EXTENDED_FEATURES: usize = 0x30;
const AMD_COMMAND_HEAD: usize = 0x2000;
const AMD_COMMAND_TAIL: usize = 0x2008;
const AMD_ENABLE: u64 = 1;
const AMD_COMMANDS_ENABLE: u64 = 1 << 12;
const AMD_GUEST_APIC_ENABLE: u64 = 1 << 17;
const AMD_X2APIC_ENABLE: u64 = 1 << 50;
const AMD_INTERRUPT_X2APIC: u64 = 1 << 51;
const AMD_HAS_GUEST_APIC: u64 = 1 << 7;
const AMD_HAS_X2APIC: u64 = 1 << 2;
const AMD_COMMAND_LENGTH_256: u64 = 8 << 56;
const AMD_DEVICES: usize = 1 << 16;
const AMD_DEVICE_ENTRY_BYTES: usize = 32;
const AMD_DEVICE_TABLE_BYTES: usize = AMD_DEVICES * AMD_DEVICE_ENTRY_BYTES;
const AMD_DEVICE_TABLE_PAGES: u64 = (AMD_DEVICE_TABLE_BYTES / PAGE) as u64;
const AMD_INTERRUPTS_VALID: u64 = 1;
const AMD_TABLE_LENGTH_256: u64 = 8 << 1;
const AMD_REMAP_INTERRUPTS: u64 = 2 << 60;
const AMD_PASS_INIT_EXTINT_NMI_LINTS: u64 = 0b111 << 56 | 0b11 << 62;
const AMD_INVALIDATE_ALL: u64 = 0x8 << 60;
const AMD_COMMAND_ENTRIES: usize = PAGE / ENTRY_BYTES;

fn amd_units(ivrs: &[u8], each: &mut dyn FnMut(u64)) {
    let mut at = IVRS_STRUCTURES;
    while let Some(header) = ivrs.get(at..at + 4) {
        let kind = header[0];
        let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if length < IVHD_LEGACY_ENTRIES {
            return;
        }
        let described = match kind {
            IVHD_LEGACY => true,
            IVHD_EXTENDED | IVHD_MIXED => length >= IVHD_EXTENDED_ENTRIES,
            _ => false,
        };
        if described && let Some(base) = ivrs.get(at + IVHD_BASE..at + IVHD_BASE + 8) {
            each(u64::from_le_bytes(base.try_into().unwrap_or_default()));
        }
        at += length;
    }
}

fn enable_amd(ivrs: &[u8], memory: Memory<'_>) -> Result<(), &'static str> {
    let mut bases = [0u64; MAX_UNITS];
    let mut count = 0;
    amd_units(ivrs, &mut |base| {
        if count < MAX_UNITS && !bases[..count].contains(&base) {
            bases[count] = base;
            count += 1;
        }
    });
    if count == 0 {
        return Err("the IVRS table lists no IOMMU");
    }
    let (table_physical, table) = memory(PAGE).ok_or("no memory for the remapping table")?;
    TABLE.store(table, Ordering::Release);
    let entry = AMD_INTERRUPTS_VALID
        | AMD_TABLE_LENGTH_256
        | table_physical & 0x000f_ffff_ffff_ffc0
        | AMD_REMAP_INTERRUPTS
        | AMD_PASS_INIT_EXTINT_NMI_LINTS;
    for (slot, &base) in bases[..count].iter().enumerate() {
        let unit = registers(base, AMD_REGISTER_BYTES).ok_or("an IOMMU is out of reach")?;
        let features = unit.read64(AMD_EXTENDED_FEATURES);
        if features & (AMD_HAS_GUEST_APIC | AMD_HAS_X2APIC) != AMD_HAS_GUEST_APIC | AMD_HAS_X2APIC {
            return Err("an IOMMU lacks the 128-bit remapping entries or x2APIC destinations");
        }
        let (devices_physical, devices) =
            memory(AMD_DEVICE_TABLE_BYTES).ok_or("no memory for an IOMMU's device table")?;
        for device in 0..AMD_DEVICES {
            let place = (devices + device * AMD_DEVICE_ENTRY_BYTES) as *mut u64;
            // SAFETY: the device table is the kernel's zeroed block of 65 536 32-byte entries
            // given just above, not yet seen by the IOMMU; the third quadword of each is its
            // interrupt half, and translation stays off (the first quadword stays 0).
            unsafe { place.add(2).write_volatile(entry) };
        }
        AMD_DEVICES[slot].store(devices, Ordering::Release);
        let (commands_physical, commands) =
            memory(PAGE).ok_or("no memory for an IOMMU's command buffer")?;
        QUEUES[slot].store(commands, Ordering::Release);
        unit.write64(AMD_DEVICE_TABLE, devices_physical | (AMD_DEVICE_TABLE_PAGES - 1));
        unit.write64(AMD_COMMAND_BUFFER, commands_physical | AMD_COMMAND_LENGTH_256);
        unit.write64(AMD_COMMAND_HEAD, 0);
        unit.write64(AMD_COMMAND_TAIL, 0);
        let control = unit.read64(AMD_CONTROL);
        unit.write64(
            AMD_CONTROL,
            control
                | AMD_COMMANDS_ENABLE
                | AMD_GUEST_APIC_ENABLE
                | AMD_X2APIC_ENABLE
                | AMD_INTERRUPT_X2APIC
                | AMD_ENABLE,
        );
        UNITS[slot].store(DIRECT_MAP_OFFSET + base as usize, Ordering::Release);
    }
    KIND.store(AMD, Ordering::Release);
    amd_invalidate();
    Ok(())
}

/// Has every IOMMU drop everything it cached, the interrupt tables included, and waits until
/// it has.
fn amd_invalidate() {
    for slot in 0..MAX_UNITS {
        let (unit, commands) = (
            UNITS[slot].load(Ordering::Acquire),
            QUEUES[slot].load(Ordering::Acquire),
        );
        if unit == 0 || commands == 0 {
            continue;
        }
        // SAFETY: `unit` is an IOMMU's registers in the direct map, recorded by `enable_amd`,
        // which nothing else drives.
        let registers = unsafe { Mmio::new(unit, AMD_REGISTER_BYTES) };
        let tail = registers.read64(AMD_COMMAND_TAIL) as usize;
        let place = (commands + tail) as *mut u64;
        // SAFETY: the command buffer is the kernel's page given at `enable`, kept for good;
        // `tail` is a multiple of 16 below its size, the slot the IOMMU reads next.
        unsafe {
            place.write_volatile(AMD_INVALIDATE_ALL);
            place.add(1).write_volatile(0);
        }
        let next = (tail + ENTRY_BYTES) % (AMD_COMMAND_ENTRIES * ENTRY_BYTES);
        registers.write64(AMD_COMMAND_TAIL, next as u64);
        spin_until(|| registers.read64(AMD_COMMAND_HEAD) as usize == next);
    }
}
