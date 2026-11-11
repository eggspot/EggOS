//! The boot parameters Loongson firmware, and QEMU's `loongson3-virt` in its place, hand a
//! MIPS64 kernel in `a2` ("LEFI", Linux's `asm/mach-loongson64/boot_param.h`; D313): packed
//! structures whose `loongson_params` names, by offsets from itself, the memory map and the CPU
//! information. Read once at boot through the cached `XKPHYS` window.

use super::CACHED_WINDOW;

/// `struct boot_params`: `efi_loongson` (`mps`, `acpi`, `acpi20`, then `smbios_tables`, whose
/// `vers` and `vga_bios` precede `loongson_params`).
const LOONGSON_PARAMS: usize = 3 * 8 + 2 + 8;
const MEMORY_OFFSET: usize = 0;
const CPU_OFFSET: usize = 8;
/// `efi_memory_map_loongson`: `vers`, `nr_map`, `mem_freq`, then 20-byte entries of
/// `node_id`, `mem_type`, `mem_start` and `mem_size` in MiB.
const MAP_COUNT: usize = 2;
const MAP_ENTRIES: usize = 10;
const MAP_ENTRY: usize = 20;
const MAP_ENTRIES_MAX: usize = 128;
const SYSTEM_RAM_LOW: u32 = 1;
const SYSTEM_RAM_HIGH: u32 = 2;
/// `efi_cpuinfo_loongson`: the clock at 18 and the CPU count at 22.
const CPU_CLOCK: usize = 18;
const CPU_COUNT: usize = 22;
/// More is not believed: a corrupt table.
const MAX_CPUS: u32 = 64;

/// Physical addresses above this are not the low memory QEMU keeps its tables in.
const TABLES_END: u64 = 1 << 32;

/// What the kernel takes from the boot parameters.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct BootParameters {
    /// RAM, as (start, length) physical ranges.
    pub(super) ram: [(u64, u64); 8],
    pub(super) ram_count: usize,
    /// The CPU clock in Hz; the counter runs at half of it.
    pub(super) cpu_clock: u64,
    pub(super) cpus: u32,
}

fn physical(address: u64) -> Option<u64> {
    let physical = match address >> 32 {
        0xffff_ffff => address & 0x1fff_ffff,
        _ if address >> 62 == 2 => address & ((1 << 48) - 1),
        0 => address,
        _ => return None,
    };
    (physical < TABLES_END).then_some(physical)
}

fn bytes<const N: usize>(address: u64) -> [u8; N] {
    let mut value = [0; N];
    for (index, byte) in value.iter_mut().enumerate() {
        // SAFETY: `address` is in the low memory the firmware's tables are in, reached through
        // the cached window, which maps all physical memory from the first instruction on;
        // QEMU wrote the tables before the kernel started and nothing writes them since.
        *byte = unsafe { core::ptr::read_volatile((CACHED_WINDOW + address + index as u64) as *const u8) };
    }
    value
}

fn u16_at(address: u64) -> u16 {
    u16::from_le_bytes(bytes(address))
}

fn u32_at(address: u64) -> u32 {
    u32::from_le_bytes(bytes(address))
}

fn u64_at(address: u64) -> u64 {
    u64::from_le_bytes(bytes(address))
}

/// Reads the boot parameters at `pointer` (the kernel address the firmware passed); `None`
/// when it points nowhere believable.
pub(super) fn read(pointer: u64) -> Option<BootParameters> {
    let base = physical(pointer)?;
    let params = base + LOONGSON_PARAMS as u64;
    let memory = params.checked_add(u64_at(params + MEMORY_OFFSET as u64))?;
    let cpu = params.checked_add(u64_at(params + CPU_OFFSET as u64))?;
    if memory >= TABLES_END || cpu >= TABLES_END {
        return None;
    }
    let _ = u16_at(memory);
    let mut found = BootParameters::default();
    let count = (u32_at(memory + MAP_COUNT as u64) as usize).min(MAP_ENTRIES_MAX);
    for index in 0..count {
        let entry = memory + (MAP_ENTRIES + index * MAP_ENTRY) as u64;
        let kind = u32_at(entry + 4);
        if kind != SYSTEM_RAM_LOW && kind != SYSTEM_RAM_HIGH {
            continue;
        }
        let start = u64_at(entry + 8);
        let length = u64::from(u32_at(entry + 16)) << 20;
        if length == 0 || found.ram_count == found.ram.len() {
            continue;
        }
        found.ram[found.ram_count] = (start, length);
        found.ram_count += 1;
    }
    found.cpu_clock = u64::from(u32_at(cpu + CPU_CLOCK as u64));
    found.cpus = u32_at(cpu + CPU_COUNT as u64).clamp(1, MAX_CPUS);
    Some(found)
}
