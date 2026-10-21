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

mod port;

use crate::acpi::{self, RootTable};
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
    call    kernel_main
3:  cli
    hlt
    jmp     3b
    .popsection

    .pushsection .rodata.boot, "a"
    .balign 8
boot_gdt:
    .quad 0                         // null descriptor
    .quad 0x00af9a000000ffff        // 0x08: 64-bit code, ring 0
    .quad 0x00cf92000000ffff        // 0x10: data, ring 0
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
    options(att_syntax)
);

/// Physical address of the PVH `hvm_start_info`, stored by the entry code. PVH loads the
/// kernel below 4 GiB and passes a 32-bit address.
static PVH_START_INFO: AtomicU32 = AtomicU32::new(0);

/// `hvm_start_info` fields (start_info.h): magic at 0, `rsdp_paddr` (u64) at 16.
const PVH_START_MAGIC: u32 = 0x336e_c578;
const PVH_RSDP_PADDR: usize = 16;

/// COM1's I/O ports.
const COM1_PORT: u16 = 0x3f8;

/// The legacy BIOS area the ACPI spec says to search for the RSDP when the loader does not
/// pass its address.
const BIOS_AREA: core::ops::Range<usize> = 0xe_0000..0x10_0000;

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
}

/// Everything needed to enter S5.
struct S5Target {
    fadt: acpi::Fadt,
    sleep_type: acpi::S5SleepType,
}

/// Reads the ACPI tables for the S5 sleep type and the PM1 control ports.
fn s5_target() -> Option<S5Target> {
    let root = rsdp().and_then(acpi::parse_rsdp)?;
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

/// The RSDP's bytes: from the PVH start info, or found in the BIOS area.
fn rsdp() -> Option<&'static [u8]> {
    let start_info = PVH_START_INFO.load(Ordering::Relaxed) as usize;
    if start_info != 0 {
        // SAFETY: the loader passed a valid `hvm_start_info` (at least 24 bytes up to and
        // including `rsdp_paddr`) in identity-mapped memory below 4 GiB, which nothing writes.
        let info = unsafe { physical_slice(start_info, PVH_RSDP_PADDR + 8) };
        let magic = u32::from_le_bytes(info[..4].try_into().ok()?);
        let rsdp = u64::from_le_bytes(info[PVH_RSDP_PADDR..].try_into().ok()?);
        if magic == PVH_START_MAGIC && rsdp != 0 {
            // SAFETY: the loader reports an RSDP there; `parse_rsdp` validates it, reading at
            // most the 36 bytes of the largest RSDP.
            return Some(unsafe { physical_slice(identity_mapped(rsdp)?, acpi::RSDP_V2_LENGTH) });
        }
    }
    // SAFETY: the legacy BIOS area is identity-mapped, readable ROM/RAM on PCs.
    let area = unsafe { physical_slice(BIOS_AREA.start, BIOS_AREA.len()) };
    let offset = acpi::find_rsdp(area)?;
    Some(&area[offset..])
}

/// The validated ACPI table with `signature` at physical `address`.
fn table(address: u64, signature: &[u8; 4]) -> Option<&'static [u8]> {
    let address = identity_mapped(address)?;
    // SAFETY: firmware table addresses point at ACPI memory, which is identity-mapped below
    // 4 GiB (checked above) and never written by the kernel; the header is read first.
    let header = unsafe { physical_slice(address, acpi::TABLE_HEADER_LENGTH) };
    let length = acpi::table_length(header)?;
    if !(acpi::TABLE_HEADER_LENGTH..=MAX_TABLE_LENGTH).contains(&length)
        || identity_mapped((address + length) as u64).is_none()
    {
        return None;
    }
    // SAFETY: as above, for the length the header declares, bounded and inside the map.
    let table = unsafe { physical_slice(address, length) };
    acpi::table_ok(table, signature).then_some(table)
}

/// `address` as a pointer-sized value if the boot page tables map it (below 4 GiB).
fn identity_mapped(address: u64) -> Option<usize> {
    if address == 0 || address > u64::from(u32::MAX) {
        return None;
    }
    usize::try_from(address).ok()
}

/// # Safety
/// `address..address + length` must be identity-mapped memory that nothing writes while the
/// returned slice is used.
unsafe fn physical_slice(address: usize, length: usize) -> &'static [u8] {
    // SAFETY: guaranteed by the caller.
    unsafe { core::slice::from_raw_parts(address as *const u8, length) }
}

/// Switches to ACPI mode if needed, then writes `SLP_TYP | SLP_EN` to the PM1 control
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
    // SAFETY: (all port accesses below) the ports come from the validated FADT, which is the
    // firmware's description of the ACPI fixed hardware; the kernel is powering off and
    // nothing else drives these registers.
    unsafe {
        if port::in16(pm1a) & acpi::PM1_CONTROL_SCI_EN == 0 && smi_command != 0 {
            port::out8(smi_command, fadt.acpi_enable);
            for _ in 0..ACPI_ENABLE_POLLS {
                if port::in16(pm1a) & acpi::PM1_CONTROL_SCI_EN != 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }
        port::out16(pm1a, acpi::pm1_control_sleep(target.sleep_type.a));
        if pm1b != 0 {
            port::out16(pm1b, acpi::pm1_control_sleep(target.sleep_type.b));
        }
    }
}
