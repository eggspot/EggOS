//! x86_64 (AMD64 / Intel 64).
//!
//! Boot contract: PVH, the boot ABI of Xen's public `arch-x86/hvm/start_info.h`, which QEMU's
//! `-kernel` supports for ELF64 images. The loader enters `_start32` (named by the
//! `XEN_ELFNOTE_PHYS32_ENTRY` ELF note) in 32-bit protected mode with paging off and `ebx`
//! holding the physical address of an `hvm_start_info`. The entry code below identity-maps
//! the first 4 GiB with 2 MiB pages, switches to long mode and calls `kernel_main`.
//!
//! The console is COM1, the PC's first serial port. Power-off uses ACPI sleep state S5, read
//! from the firmware's ACPI tables (see [`crate::acpi`]).

mod exception;
mod port;
mod timer;

use crate::acpi::{self, RootTable};
use crate::arch::boot_memory;
use crate::memory::{MemoryMap, PhysicalRange};
use crate::paging::{self, KernelImage};
use crate::{ExitStatus, Platform};
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_drivers::serial::{Ns16550, Uart};
use port::PortIo;

core::arch::global_asm!(
    r#"
    // PVH entry point note (XEN_ELFNOTE_PHYS32_ENTRY = 18), read by the loader.
    .pushsection .note.Xen, "a", @note
    .balign 4
    .long 4                     // name size
    .long 8                     // descriptor size
    .long 18                    // XEN_ELFNOTE_PHYS32_ENTRY
    .asciz "Xen"
    .balign 4
    .quad _start32
    .popsection

    .pushsection .text.boot, "ax"
    .code32
    .global _start32
_start32:
    cli
    cld
    movl    %ebx, %esi              // keep the hvm_start_info address

    // Zero .bss (16-byte aligned and sized by the linker script); the page tables live there.
    movl    $__bss_start, %edi
    movl    $__bss_end, %ecx
    subl    %edi, %ecx
    xorl    %eax, %eax
    rep stosb

    movl    %esi, {start_info}
    movl    $__stack_top, %esp

    // PML4[0] -> PDPT; PDPT[0..4] -> the four page directories.
    movl    $boot_pdpt, %eax
    orl     $0x3, %eax              // present, writable
    movl    %eax, boot_pml4
    xorl    %ecx, %ecx
1:  movl    %ecx, %eax
    shll    $12, %eax
    addl    $boot_pd, %eax
    orl     $0x3, %eax
    movl    %eax, boot_pdpt(, %ecx, 8)
    incl    %ecx
    cmpl    $4, %ecx
    jne     1b

    // 2048 2 MiB pages: identity map of 0 .. 4 GiB (RAM below 4 GiB, ACPI tables, MMIO).
    xorl    %ecx, %ecx
2:  movl    %ecx, %eax
    shll    $21, %eax
    orl     $0x83, %eax             // present, writable, 2 MiB page
    movl    %eax, boot_pd(, %ecx, 8)
    incl    %ecx
    cmpl    $2048, %ecx
    jne     2b

    // Long mode: PAE, page tables, EFER.LME, then paging.
    movl    %cr4, %eax
    orl     $(1 << 5), %eax
    movl    %eax, %cr4
    movl    $boot_pml4, %eax
    movl    %eax, %cr3
    movl    $0xc0000080, %ecx
    rdmsr
    orl     $(1 << 8), %eax
    wrmsr
    movl    %cr0, %eax
    orl     $0x80000001, %eax       // PG | PE
    movl    %eax, %cr0

    lgdt    boot_gdt_pointer
    ljmp    $0x08, $start64

    .code64
start64:
    movw    $0x10, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    movw    %ax, %fs
    movw    %ax, %gs
    movq    $__stack_top, %rsp
    call    {install_exception_vectors}
    call    {init_timer}
    call    kernel_main
3:  cli
    hlt
    jmp     3b
    .popsection

    .pushsection .rodata.boot, "a"
    .balign 8
boot_gdt:
    .quad 0                         // null descriptor
    // Accessed bits preset, so the CPU never writes the GDT (it is in read-only memory once
    // the kernel's page tables are active).
    .quad 0x00af9b000000ffff        // 0x08: 64-bit code, ring 0
    .quad 0x00cf93000000ffff        // 0x10: data, ring 0
boot_gdt_end:
boot_gdt_pointer:
    .word boot_gdt_end - boot_gdt - 1
    .long boot_gdt
    .popsection

    .pushsection .bss.boot, "aw", @nobits
    .balign 4096
boot_pml4:
    .skip 4096
boot_pdpt:
    .skip 4096
boot_pd:
    .skip 4 * 4096
    .popsection
"#,
    start_info = sym PVH_START_INFO,
    install_exception_vectors = sym exception::install,
    init_timer = sym timer::init,
    options(att_syntax)
);

/// Physical address of the PVH `hvm_start_info`, stored by the entry code. PVH loads the
/// kernel below 4 GiB and passes a 32-bit address.
static PVH_START_INFO: AtomicU32 = AtomicU32::new(0);

/// `hvm_start_info` (start_info.h): `magic` (u32) at 0, then `version`, `flags`, `nr_modules`
/// (u32 each), `modlist_paddr` and `cmdline_paddr` (u64 each), and `rsdp_paddr` (u64) at 32.
const PVH_START_MAGIC: u32 = 0x336e_c578;
const PVH_RSDP_PADDR: usize = 32;
/// Bytes of `hvm_start_info` read: up to and including `rsdp_paddr` (present since version 0).
const PVH_START_INFO_READ_LENGTH: usize = PVH_RSDP_PADDR + 8;
/// Version 1 adds `memmap_paddr` (u64) at 40 and `memmap_entries` (u32) at 48: the memory map,
/// an array of `hvm_memmap_table_entry` (`addr`, `size` as u64, `type`, `reserved` as u32).
const PVH_VERSION: usize = 4;
const PVH_MEMMAP_PADDR: usize = 40;
const PVH_MEMMAP_ENTRIES: usize = 48;
const PVH_START_INFO_V1_LENGTH: usize = 56;
const PVH_MEMMAP_ENTRY_LENGTH: usize = 24;
/// `XEN_HVM_MEMMAP_TYPE_RAM`: usable RAM (the E820 type 1 encoding).
const PVH_MEMMAP_TYPE_RAM: u32 = 1;
/// More entries than any real memory map, against a corrupt count.
const PVH_MAX_MEMMAP_ENTRIES: usize = 1024;

/// The direct map: the start of the high half (PML4 entry 256, D29).
const DIRECT_MAP_OFFSET: usize = 0xffff_8000_0000_0000;

/// `IA32_EFER` and its no-execute enable bit; `CR0.WP`, which makes read-only pages read-only
/// for the kernel too.
const EFER: u32 = 0xc000_0080;
const EFER_NXE: u32 = 1 << 11;
const CR0_WP: u64 = 1 << 16;

/// Memory below 1 MiB stays out of the allocator: it holds the real-mode IVT, the BIOS data
/// areas, the EBDA and the legacy BIOS area (where the RSDP is found), and the trampoline that
/// starts the other CPUs will need it (M4).
const LOW_MEMORY: PhysicalRange = PhysicalRange {
    start: 0,
    end: 0x10_0000,
};

/// COM1's I/O ports.
const COM1_PORT: u16 = 0x3f8;

/// The legacy BIOS area the ACPI spec says to search for the RSDP when the loader does not
/// pass its address.
const BIOS_AREA: core::ops::Range<usize> = 0xe_0000..0x10_0000;

/// End of the boot identity map, which covers 0 .. 4 GiB. Page 0 is mapped too, so a null
/// pointer dereference reads low memory instead of faulting until the kernel builds its own
/// page tables (M2).
const IDENTITY_MAP_END: u64 = 1 << 32;

/// Upper bound on any ACPI table we map as a slice, against corrupt length fields.
const MAX_TABLE_LENGTH: usize = 1 << 20;

/// How long to wait for the firmware to switch to ACPI mode, in polls of the PM1 control
/// register.
const ACPI_ENABLE_POLLS: u32 = 1_000_000;

#[derive(Debug)]
pub struct X86_64;

impl Platform for X86_64 {
    const ARCH: &'static str = "x86_64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        // SAFETY: COM1's ports are the PC's first serial port by platform convention, and the
        // early console is their only user.
        let regs = unsafe { PortIo::new(COM1_PORT, Ns16550::<PortIo>::register_block_size(0)) };
        Ns16550::new(regs, 0).write_bytes(bytes);
    }

    fn exit(_status: ExitStatus) -> ! {
        // ACPI S5 has no exit status; the QEMU runner reads the verdict from the console. If
        // the tables are missing or malformed we fall through and halt.
        if let Some(target) = s5_target() {
            enter_s5(target);
        }
        Self::halt()
    }

    fn halt() -> ! {
        loop {
            // SAFETY: masking interrupts and halting only stops this CPU.
            unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
        }
    }

    fn breakpoint() {
        // SAFETY: `int3` traps to the IDT installed at boot, whose handler saves and restores
        // every general-purpose register and returns to the instruction after the `int3`.
        // No `nomem`/`nostack`: the kernel's handler may read and write any memory, and the
        // exception frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("int3") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: `ud2` always raises #UD; it traps to the IDT installed at boot, and the
        // kernel never resumes from that exception.
        unsafe { core::arch::asm!("ud2", options(noreturn)) }
    }

    #[inline]
    fn clock_ticks() -> u64 {
        timer::now()
    }

    #[inline]
    fn clock_frequency() -> u64 {
        timer::frequency()
    }

    fn set_timer_deadline(deadline: u64) {
        timer::set_deadline(deadline);
    }

    fn wait_for_interrupt() {
        // SAFETY: `sti` unmasks interrupts only after the next instruction, so `hlt` cannot
        // miss an interrupt that arrives in between; the pending interrupt then goes through
        // the IDT installed at boot, whose handler restores every register, and `cli` masks
        // interrupts again. No `nomem`/`nostack`: the kernel's handler may read and write any
        // memory, and the interrupt frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("sti", "hlt", "cli") };
    }

    fn memory_map() -> MemoryMap {
        let mut map = MemoryMap::new();
        let Some(pvh) = PvhMemoryMap::read() else {
            return map;
        };
        // All RAM first, then every other type as reserved: where a loader reports
        // overlapping entries, the more restrictive type wins.
        for (range, kind) in pvh.entries() {
            if kind == PVH_MEMMAP_TYPE_RAM {
                map.add_ram(range);
            }
        }
        for (range, kind) in pvh.entries() {
            if kind != PVH_MEMMAP_TYPE_RAM {
                map.reserve(range);
            }
        }
        map.reserve(LOW_MEMORY);
        map.reserve(boot_memory::kernel_image());
        // The start info and its memory map are read again (the start info at power-off, for
        // the RSDP).
        for range in pvh.boot_data() {
            map.reserve(range);
        }
        // The kernel's page tables are built through the boot identity map, so only memory
        // inside it is handed out.
        map.retain_within(PhysicalRange {
            start: 0,
            end: IDENTITY_MAP_END,
        });
        map
    }

    #[inline]
    fn physical_memory_offset() -> usize {
        boot_memory::physical_memory_offset()
    }

    type PageTableFormat = paging::x86_64::X86_64;

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    fn kernel_image() -> KernelImage {
        boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        // Every entry of the loader's memory map, whatever its type (the ACPI tables are in
        // reserved or ACPI entries), the low 1 MiB (where the RSDP is searched for) and the
        // start info. Partial frames at the ends count whole.
        let mut map = MemoryMap::new();
        map.add_ram(LOW_MEMORY);
        if let Some(pvh) = PvhMemoryMap::read() {
            for (range, _) in pvh.entries() {
                map.add_ram(range.covering_frames());
            }
            for range in pvh.boot_data() {
                map.add_ram(range.covering_frames());
            }
        }
        map
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        region(timer::local_apic_registers());
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [root] = roots else {
            panic!("x86_64 page tables have one root");
        };
        // No-execute bits are reserved until EFER.NXE is set: set it before loading tables
        // that use them.
        // SAFETY: EFER is architectural on every x86_64 CPU, and reading and writing it back
        // with NXE set changes only how page-table bit 63 is read. Every x86_64 CPU QEMU
        // emulates, and every 64-bit CPU sold since 2004, has NX.
        unsafe {
            core::arch::asm!(
                "rdmsr",
                "or eax, {nxe:e}",
                "wrmsr",
                nxe = in(reg) EFER_NXE,
                in("ecx") EFER,
                out("eax") _,
                out("edx") _,
                options(nomem, nostack),
            );
        }
        // SAFETY: `CR0.WP` makes read-only pages read-only for the kernel too; nothing writes
        // read-only memory, so the kernel only gains faults it should get.
        unsafe {
            core::arch::asm!(
                "mov {tmp}, cr0",
                "or {tmp}, {wp}",
                "mov cr0, {tmp}",
                tmp = out(reg) _,
                wp = in(reg) CR0_WP,
                options(nomem, nostack),
            );
        }
        // SAFETY: the caller guarantees the tables map everything this CPU uses from here on at
        // the addresses it uses now: the code runs on at its physical address, the stack, data,
        // GDT and IDT likewise, and the local APIC too. Loading CR3 flushes the TLB. No
        // `nomem`: memory accesses must not move across the switch.
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) *root, options(nostack, preserves_flags));
        }
        boot_memory::set_physical_memory_offset(DIRECT_MAP_OFFSET);
    }
}

/// The PVH loader's memory map (start info version 1 and later).
struct PvhMemoryMap {
    start_info: u64,
    table_address: u64,
    table: &'static [u8],
}

impl PvhMemoryMap {
    fn read() -> Option<Self> {
        let start_info_address = u64::from(PVH_START_INFO.load(Ordering::Relaxed));
        let start_info = mapped(start_info_address, PVH_START_INFO_V1_LENGTH)?;
        // SAFETY: the loader passed the address of its `hvm_start_info`; `mapped` checked the
        // bytes are reachable and nothing writes them. A version 0 structure is shorter: the
        // bytes after it are still mapped memory, and the version 1 fields are used only once
        // the version says they exist.
        let info = unsafe { physical_slice(start_info, PVH_START_INFO_V1_LENGTH) };
        let field32 = |offset: usize| {
            u32::from_le_bytes(info[offset..offset + 4].try_into().unwrap_or_default())
        };
        if field32(0) != PVH_START_MAGIC || field32(PVH_VERSION) < 1 {
            return None;
        }
        let table_address = u64::from_le_bytes(
            info[PVH_MEMMAP_PADDR..PVH_MEMMAP_PADDR + 8]
                .try_into()
                .unwrap_or_default(),
        );
        let entries = usize::try_from(field32(PVH_MEMMAP_ENTRIES)).unwrap_or(usize::MAX);
        if entries > PVH_MAX_MEMMAP_ENTRIES {
            return None;
        }
        let table_length = entries * PVH_MEMMAP_ENTRY_LENGTH;
        let table = mapped(table_address, table_length)?;
        Some(Self {
            start_info: start_info_address,
            table_address,
            // SAFETY: the loader's memory map table, `entries` entries long; `mapped` checked
            // it is reachable, and nothing writes it.
            table: unsafe { physical_slice(table, table_length) },
        })
    }

    /// Each entry's range and type.
    fn entries(&self) -> impl Iterator<Item = (PhysicalRange, u32)> + '_ {
        self.table
            .as_chunks::<PVH_MEMMAP_ENTRY_LENGTH>()
            .0
            .iter()
            .map(|entry| {
                let [address, length, kind] = [0, 8, 16].map(|offset| {
                    u64::from_le_bytes(entry[offset..offset + 8].try_into().unwrap_or_default())
                });
                (
                    PhysicalRange::from_start_length(address, length),
                    kind as u32,
                )
            })
    }

    /// The start info and the memory map table themselves.
    fn boot_data(&self) -> [PhysicalRange; 2] {
        [
            PhysicalRange::from_start_length(self.start_info, PVH_START_INFO_V1_LENGTH as u64),
            PhysicalRange::from_start_length(self.table_address, self.table.len() as u64),
        ]
    }
}

/// Everything needed to enter S5.
struct S5Target {
    fadt: acpi::Fadt,
    sleep_type: acpi::S5SleepType,
}

/// Reads the ACPI tables for the S5 sleep type and the PM1 control ports.
fn s5_target() -> Option<S5Target> {
    let root = pvh_rsdp()
        .and_then(acpi::parse_rsdp)
        .or_else(bios_area_rsdp)?;
    let (RootTable::Rsdt(root_address) | RootTable::Xsdt(root_address)) = root;
    let root_table = table(root_address, signature_of(root))?;
    let fadt = acpi::root_table_entries(root_table, root)
        .find_map(|address| table(address, b"FACP"))
        .and_then(acpi::parse_fadt)?;
    let sleep_type = table(fadt.dsdt, b"DSDT")
        .and_then(|dsdt| acpi::find_s5_sleep_type(&dsdt[acpi::TABLE_HEADER_LENGTH..]))?;
    Some(S5Target { fadt, sleep_type })
}

fn signature_of(root: RootTable) -> &'static [u8; 4] {
    match root {
        RootTable::Rsdt(_) => b"RSDT",
        RootTable::Xsdt(_) => b"XSDT",
    }
}

/// The RSDP's bytes, at the address the PVH loader passed in `hvm_start_info`, if any.
/// QEMU 10's loader passes 0 (no address), so on QEMU the BIOS-area search finds the RSDP.
fn pvh_rsdp() -> Option<&'static [u8]> {
    let start_info = mapped(
        PVH_START_INFO.load(Ordering::Relaxed).into(),
        PVH_START_INFO_READ_LENGTH,
    )?;
    // SAFETY: the loader passed the address of its `hvm_start_info`, which spans at least
    // `PVH_START_INFO_READ_LENGTH` bytes; `mapped` checked they are in the identity map, and
    // nothing writes them.
    let info = unsafe { physical_slice(start_info, PVH_START_INFO_READ_LENGTH) };
    let magic = u32::from_le_bytes(info[..4].try_into().ok()?);
    let rsdp = u64::from_le_bytes(info[PVH_RSDP_PADDR..].try_into().ok()?);
    if magic != PVH_START_MAGIC {
        return None;
    }
    let rsdp = mapped(rsdp, acpi::RSDP_V2_LENGTH)?;
    // SAFETY: the loader reports an RSDP there, inside the identity map (checked by `mapped`);
    // `parse_rsdp` validates the bytes, so a wrong address only fails validation.
    Some(unsafe { physical_slice(rsdp, acpi::RSDP_V2_LENGTH) })
}

/// The RSDP found by the ACPI spec's search of the legacy BIOS area.
fn bios_area_rsdp() -> Option<RootTable> {
    // SAFETY: the legacy BIOS area is identity-mapped, readable ROM/RAM on PCs, and nothing
    // writes it.
    let area = unsafe { physical_slice(BIOS_AREA.start, BIOS_AREA.len()) };
    let offset = acpi::find_rsdp(area)?;
    acpi::parse_rsdp(&area[offset..])
}

/// The validated ACPI table with `signature` at physical `address`.
fn table(address: u64, signature: &[u8; 4]) -> Option<&'static [u8]> {
    let header_address = mapped(address, acpi::TABLE_HEADER_LENGTH)?;
    // SAFETY: firmware table addresses point at ACPI memory, which nothing writes; `mapped`
    // checked the header lies inside the identity map.
    let header = unsafe { physical_slice(header_address, acpi::TABLE_HEADER_LENGTH) };
    if &header[..4] != signature {
        return None;
    }
    let length = acpi::table_length(header)?;
    if !(acpi::TABLE_HEADER_LENGTH..=MAX_TABLE_LENGTH).contains(&length) {
        return None;
    }
    let table_address = mapped(address, length)?;
    // SAFETY: as above, for the bounded length the header declares, checked by `mapped`.
    let table = unsafe { physical_slice(table_address, length) };
    acpi::table_ok(table, signature).then_some(table)
}

/// `address` as a pointer-sized value if `address..address + length` is inside the first 4 GiB,
/// where the firmware's tables are: the boot identity map covers all of it, the direct map the
/// parts [`Platform::physical_memory`] lists. Address 0 is rejected as "no table".
fn mapped(address: u64, length: usize) -> Option<usize> {
    let end = address.checked_add(u64::try_from(length).ok()?)?;
    if address == 0 || end > IDENTITY_MAP_END {
        return None;
    }
    usize::try_from(address).ok()
}

/// # Safety
/// Physical `address..address + length` must be memory the kernel reaches through the physical
/// memory offset (the boot identity map, or the direct map), that nothing writes while the
/// returned slice is used.
unsafe fn physical_slice(address: usize, length: usize) -> &'static [u8] {
    let virt = address.wrapping_add(boot_memory::physical_memory_offset());
    // SAFETY: guaranteed by the caller.
    unsafe { core::slice::from_raw_parts(virt as *const u8, length) }
}

/// Switches to ACPI mode if needed, then sets `SLP_TYP` and `SLP_EN` in the PM1 control
/// registers (ACPI 6.5, 16.1.7). Returns only if the machine did not power off.
fn enter_s5(target: S5Target) {
    let fadt = target.fadt;
    let (Ok(pm1a), Ok(pm1b), Ok(smi_command)) = (
        u16::try_from(fadt.pm1a_control_port),
        u16::try_from(fadt.pm1b_control_port),
        u16::try_from(fadt.smi_command_port),
    ) else {
        return;
    };
    if pm1a == 0 {
        return;
    }
    // The only ports touched below are the FADT's PM1 control and SMI command ports. They come
    // from the validated FADT, the firmware's description of the ACPI fixed hardware; the
    // kernel is powering off and nothing else drives these registers.
    let read16 = |p: u16| {
        // SAFETY: `p` is one of the FADT ports above, which are safe to read.
        unsafe { port::in16(p) }
    };
    let write16 = |p: u16, value: u16| {
        // SAFETY: `p` is a PM1 control port from the FADT; we write the spec's sleep sequence.
        unsafe { port::out16(p, value) }
    };
    let in_legacy_mode = read16(pm1a) & acpi::PM1_CONTROL_SCI_EN == 0;
    if in_legacy_mode && smi_command != 0 && fadt.acpi_enable != 0 {
        // SAFETY: writing the FADT's ACPI_ENABLE value to its SMI command port is the spec's
        // handshake for switching to ACPI mode.
        unsafe { port::out8(smi_command, fadt.acpi_enable) };
        for _ in 0..ACPI_ENABLE_POLLS {
            if read16(pm1a) & acpi::PM1_CONTROL_SCI_EN != 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }
    // Set SLP_TYP first, then SLP_EN in a second write, as the spec's sleep sequence does;
    // every other bit (SCI_EN included) keeps its value.
    let pm1a_value = acpi::pm1_control_sleep(read16(pm1a), target.sleep_type.a);
    let pm1b_value =
        (pm1b != 0).then(|| acpi::pm1_control_sleep(read16(pm1b), target.sleep_type.b));
    write16(pm1a, pm1a_value & !acpi::PM1_CONTROL_SLP_EN);
    if let Some(pm1b_value) = pm1b_value {
        write16(pm1b, pm1b_value & !acpi::PM1_CONTROL_SLP_EN);
    }
    write16(pm1a, pm1a_value);
    if let Some(pm1b_value) = pm1b_value {
        write16(pm1b, pm1b_value);
    }
}
