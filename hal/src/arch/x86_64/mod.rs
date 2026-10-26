//! x86_64 (AMD64 / Intel 64).
//!
//! Boot contract: PVH, the boot ABI of Xen's public `arch-x86/hvm/start_info.h`, which QEMU's
//! `-kernel` supports for ELF64 images. The loader enters `_start32` (named by the
//! `XEN_ELFNOTE_PHYS32_ENTRY` ELF note) in 32-bit protected mode with paging off and `ebx`
//! holding the physical address of an `hvm_start_info`. The entry code below maps the first
//! 4 GiB with 2 MiB pages, switches to long mode, jumps to the kernel's virtual address in the
//! top 2 GiB and calls `kernel_main` (D33).
//!
//! The console is COM1, the PC's first serial port. Power-off uses ACPI sleep state S5, read
//! from the firmware's ACPI tables (see [`crate::acpi`]).

mod console_input;
mod exception;
mod fp;
mod port;
mod random;
mod thread;
mod timer;

use crate::acpi::{self, RootTable};
use crate::arch::boot_memory;
use crate::cpu::{CpuStart, StartCpuError};
use crate::memory::{MemoryMap, PhysicalRange};
use crate::paging::{self, KernelImage};
use crate::{ExitStatus, Platform};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use eggos_drivers::serial::{Ns16550, Uart};
use port::PortIo;

core::arch::global_asm!(
    r#"
    // The image's virtual address minus its physical address. The 32-bit code below runs at
    // physical addresses with paging off, so it subtracts this from every symbol.
    .set    KERNEL_VIRTUAL_OFFSET, {kernel_virtual_offset}

    // PVH entry point note (XEN_ELFNOTE_PHYS32_ENTRY = 18), read by the loader.
    .pushsection .note.Xen, "a", @note
    .balign 4
    .long 4                     // name size
    .long 8                     // descriptor size
    .long 18                    // XEN_ELFNOTE_PHYS32_ENTRY
    .asciz "Xen"
    .balign 4
    .quad _start32 - KERNEL_VIRTUAL_OFFSET
    .popsection

    .pushsection .text.boot, "ax"
    .code32
    .global _start32
_start32:
    cli
    cld
    movl    %ebx, %esi              // keep the hvm_start_info address

    // Zero .bss (16-byte aligned and sized by the linker script); the page tables live there.
    movl    $(__bss_start - KERNEL_VIRTUAL_OFFSET), %edi
    movl    $(__bss_end - KERNEL_VIRTUAL_OFFSET), %ecx
    subl    %edi, %ecx
    xorl    %eax, %eax
    rep stosb

    movl    %esi, ({start_info} - KERNEL_VIRTUAL_OFFSET)

    // The boot page tables (D33): the first 4 GiB with 2 MiB pages, at their physical
    // addresses (PML4[0], only for the switch to long mode and the jump below) and in the
    // boot direct map (PML4[{direct_map_index}]): RAM below 4 GiB, ACPI tables, MMIO. The
    // kernel's virtual addresses (PML4[511], PDPT entry {kernel_pdpt_index}) map its first
    // gigabyte.
    movl    $(boot_pdpt - KERNEL_VIRTUAL_OFFSET), %eax
    orl     $0x3, %eax              // present, writable
    movl    %eax, (boot_pml4 - KERNEL_VIRTUAL_OFFSET)
    movl    %eax, (boot_pml4 - KERNEL_VIRTUAL_OFFSET + {direct_map_index} * 8)
    movl    $(boot_pdpt_kernel - KERNEL_VIRTUAL_OFFSET), %eax
    orl     $0x3, %eax
    movl    %eax, (boot_pml4 - KERNEL_VIRTUAL_OFFSET + 511 * 8)
    movl    $(boot_pd - KERNEL_VIRTUAL_OFFSET), %eax
    orl     $0x3, %eax
    movl    %eax, (boot_pdpt_kernel - KERNEL_VIRTUAL_OFFSET + {kernel_pdpt_index} * 8)
    // PDPT[0..4] -> the four page directories.
    xorl    %ecx, %ecx
1:  movl    %ecx, %eax
    shll    $12, %eax
    addl    $(boot_pd - KERNEL_VIRTUAL_OFFSET), %eax
    orl     $0x3, %eax
    movl    %eax, (boot_pdpt - KERNEL_VIRTUAL_OFFSET)(, %ecx, 8)
    incl    %ecx
    cmpl    $4, %ecx
    jne     1b

    // 2048 2 MiB pages: 0 .. 4 GiB.
    xorl    %ecx, %ecx
2:  movl    %ecx, %eax
    shll    $21, %eax
    orl     $0x83, %eax             // present, writable, 2 MiB page
    movl    %eax, (boot_pd - KERNEL_VIRTUAL_OFFSET)(, %ecx, 8)
    incl    %ecx
    cmpl    $2048, %ecx
    jne     2b

    // Long mode: PAE, page tables, EFER.LME, then paging.
    movl    %cr4, %eax
    orl     $(1 << 5), %eax
    movl    %eax, %cr4
    movl    $(boot_pml4 - KERNEL_VIRTUAL_OFFSET), %eax
    movl    %eax, %cr3
    movl    $0xc0000080, %ecx
    rdmsr
    orl     $(1 << 8), %eax
    wrmsr
    movl    %cr0, %eax
    orl     $0x80000001, %eax       // PG | PE
    movl    %eax, %cr0

    lgdt    (boot_gdt_pointer32 - KERNEL_VIRTUAL_OFFSET)
    ljmp    $0x08, $(start64 - KERNEL_VIRTUAL_OFFSET)

    .code64
start64:
    // Continue at the kernel's virtual address, then point the GDTR there too: the low
    // identity map goes away once the kernel's own page tables are active.
    movabsq $start64_high, %rax
    jmpq    *%rax
start64_high:
    lgdt    boot_gdt_pointer64(%rip)
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

    // A secondary CPU's 64-bit entry, reached from the trampoline below at the kernel's
    // virtual address with the boot page tables (D40): the `CpuStart` from the trampoline's
    // slot, read through the boot tables' identity map, and its stack; then the kernel's GDT
    // and segments as in `start64_high` (CS through a far return, which needs the stack), and
    // the IDT.
    .pushsection .text.boot, "ax"
secondary_start64:
    movq    {trampoline} + (eggos_trampoline_start_slot - eggos_trampoline_start), %rbx
    movq    {start_stack_top}(%rbx), %rsp
    // The slot is read: the boot CPU may reuse it for the next CPU.
    movl    $1, {trampoline} + (eggos_trampoline_taken - eggos_trampoline_start)
    lgdt    boot_gdt_pointer64(%rip)
    movw    $0x10, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    movw    %ax, %fs
    movw    %ax, %gs
    leaq    1f(%rip), %rax
    pushq   $0x08
    pushq   %rax
    lretq
1:  call    {load_idt}
    movq    %rbx, %rdi
    call    {secondary_main}
2:  cli
    hlt
    jmp     2b
    .popsection

    // The trampoline that starts a secondary CPU (D40), copied to physical address
    // {trampoline} (page {trampoline_page}, the start-up vector) before the CPUs are started;
    // never run where it is linked. The start-up interrupt starts the CPU in real mode at
    // {trampoline}: protected mode with the trampoline's own GDT, then long mode with the boot
    // page tables (below 4 GiB, as a 32-bit CR3 needs), then on to `secondary_start64` at the
    // kernel's virtual address. Addresses are the copy's: {trampoline} + offset.
    .pushsection .rodata.trampoline, "a"
    .balign 16
    .global eggos_trampoline_start
eggos_trampoline_start:
    .code16
    cli
    cld
    xorw    %ax, %ax
    movw    %ax, %ds
    lgdtl   {trampoline} + (trampoline_gdt_pointer - eggos_trampoline_start)
    movl    %cr0, %eax
    orl     $1, %eax                // PE
    movl    %eax, %cr0
    ljmpl   $0x08, ${trampoline} + (trampoline32 - eggos_trampoline_start)
    .code32
trampoline32:
    movw    $0x10, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    movl    %cr4, %eax
    orl     $(1 << 5), %eax         // PAE
    movl    %eax, %cr4
    movl    $(boot_pml4 - KERNEL_VIRTUAL_OFFSET), %eax
    movl    %eax, %cr3
    movl    $0xc0000080, %ecx
    rdmsr
    orl     $(1 << 8), %eax         // EFER.LME
    wrmsr
    movl    %cr0, %eax
    orl     $0x80000000, %eax       // PG
    movl    %eax, %cr0
    ljmpl   $0x18, ${trampoline} + (trampoline64 - eggos_trampoline_start)
    .code64
trampoline64:
    movabsq $secondary_start64, %rax
    jmpq    *%rax
    .balign 8
trampoline_gdt:
    .quad 0                         // null descriptor
    .quad 0x00cf9b000000ffff        // 0x08: 32-bit code
    .quad 0x00cf93000000ffff        // 0x10: data
    .quad 0x00af9b000000ffff        // 0x18: 64-bit code
trampoline_gdt_end:
trampoline_gdt_pointer:
    .word trampoline_gdt_end - trampoline_gdt - 1
    .long {trampoline} + (trampoline_gdt - eggos_trampoline_start)
    .balign 8
    // Written by the boot CPU in the copy before each start: the `CpuStart`'s address, and
    // a flag the CPU sets once it has read it.
    .global eggos_trampoline_start_slot
eggos_trampoline_start_slot:
    .quad 0
    .global eggos_trampoline_taken
eggos_trampoline_taken:
    .long 0
    .balign 8
    .global eggos_trampoline_end
eggos_trampoline_end:
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
boot_gdt_pointer32:
    .word boot_gdt_end - boot_gdt - 1
    .long boot_gdt - KERNEL_VIRTUAL_OFFSET
    .balign 8
boot_gdt_pointer64:
    .word boot_gdt_end - boot_gdt - 1
    .quad boot_gdt
    .popsection

    .pushsection .bss.boot, "aw", @nobits
    .balign 4096
boot_pml4:
    .skip 4096
boot_pdpt:
    .skip 4096
boot_pdpt_kernel:
    .skip 4096
boot_pd:
    .skip 4 * 4096
    .popsection
"#,
    kernel_virtual_offset = const KERNEL_VIRTUAL_OFFSET,
    direct_map_index = const paging::index(DIRECT_MAP_OFFSET as u64, 3),
    kernel_pdpt_index = const paging::index(KERNEL_VIRTUAL_OFFSET, 2),
    start_info = sym PVH_START_INFO,
    install_exception_vectors = sym exception::install,
    init_timer = sym timer::init,
    load_idt = sym exception::load,
    secondary_main = sym secondary_main,
    start_stack_top = const core::mem::offset_of!(CpuStart, stack_top),
    trampoline = const TRAMPOLINE,
    trampoline_page = const TRAMPOLINE >> 12,
    options(att_syntax)
);

/// The image's virtual address minus its physical address: the kernel links at
/// `0xffff_ffff_8010_0000`, in the top 2 GiB (D33), as the kernel code model requires; its
/// physical addresses map there through the boot tables' first gigabyte. `linker.ld` uses the
/// same value and checks it against this one.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = 0xffff_ffff_8000_0000;

/// Zeroes the `length` bytes at `start` ([`crate::memory::zero`]): with the compiler's
/// `memset`, which is `rep stosq` here, the CPU's own fast path for filling memory (D56).
///
/// # Safety
/// As for [`crate::memory::zero`].
#[inline]
pub(crate) unsafe fn zero(start: *mut u8, length: usize) {
    // SAFETY: guaranteed by the caller.
    unsafe { start.write_bytes(0, length) }
}

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
/// starts the other CPUs ([`TRAMPOLINE`]).
const LOW_MEMORY: PhysicalRange = PhysicalRange {
    start: 0,
    end: 0x10_0000,
};

/// Where the trampoline that starts the secondary CPUs is copied (D40): a page of RAM below
/// 1 MiB, as the start-up interrupt's 8-bit page number requires, inside [`LOW_MEMORY`], which
/// the allocator never hands out. `start_cpu` checks the loader's memory map says it is RAM and
/// holds none of the loader's data.
const TRAMPOLINE: usize = 0x8000;
const TRAMPOLINE_PAGE_SIZE: usize = 4096;

/// Set once the trampoline is copied and the other CPUs are reset with INIT.
static TRAMPOLINE_READY: AtomicBool = AtomicBool::new(false);

/// How long to wait after INIT before the first start-up interrupt (SDM vol. 3A, 9.4.4.1),
/// before a second start-up interrupt, and in all for a CPU to read its `CpuStart`.
const INIT_DELAY_US: u64 = 10_000;
const STARTUP_RETRY_US: u64 = 200;
const STARTUP_TIMEOUT_US: u64 = 100_000;

/// `IA32_GS_BASE`: the GS segment's base in 64-bit mode, the per-CPU block pointer (D40).
const IA32_GS_BASE: u32 = 0xc000_0101;

unsafe extern "C" {
    /// The trampoline in `.rodata`, and the two words in it the boot CPU writes in the copy
    /// (see the boot code above).
    static eggos_trampoline_start: u8;
    static eggos_trampoline_start_slot: u8;
    static eggos_trampoline_taken: u8;
    static eggos_trampoline_end: u8;
}

/// Called by `secondary_start64` on the `CpuStart`'s stack, at the kernel's virtual address with
/// the boot page tables and the IDT loaded: switches to the kernel's page tables, sets this CPU's
/// local APIC and timer up (D41), then calls the entry.
extern "C" fn secondary_main(start: &CpuStart) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own, which map
    // everything this CPU uses at the addresses it uses now, as they did for the boot CPU;
    // interrupts are masked (the trampoline starts with `cli`).
    unsafe { X86_64::activate_page_tables(&start.page_table_roots[..1]) };
    timer::init_this_cpu();
    (start.entry)(start.argument)
}

/// The trampoline's offset of `symbol`, one of the symbols inside it.
fn trampoline_offset(symbol: *const u8) -> usize {
    symbol as usize - &raw const eggos_trampoline_start as usize
}

/// The atomic word in the trampoline's copy at the offset `symbol` has in the original.
///
/// # Safety
/// `symbol` must be one of the trampoline's words, aligned and sized for `T`, and the copy must
/// be in place ([`prepare_trampoline`]).
unsafe fn trampoline_word<T>(symbol: *const u8) -> &'static T {
    let address = DIRECT_MAP_OFFSET + TRAMPOLINE + trampoline_offset(symbol);
    // SAFETY: the copy is in the trampoline page, which the direct map maps and the allocator
    // never hands out; the caller guarantees the word is a `T` there. `T` is an atomic, so
    // the starting CPU's plain accesses to it are its only other use.
    unsafe { &*(address as *const T) }
}

/// Whether the trampoline page is RAM in the loader's memory map and holds none of its data.
fn trampoline_page_is_free() -> bool {
    let page = PhysicalRange::from_start_length(TRAMPOLINE as u64, TRAMPOLINE_PAGE_SIZE as u64);
    let Some(pvh) = PvhMemoryMap::read() else {
        return false;
    };
    let in_ram = pvh.entries().any(|(range, kind)| {
        kind == PVH_MEMMAP_TYPE_RAM && range.start <= page.start && page.end <= range.end
    });
    let overlaps = |range: PhysicalRange| range.start < page.end && page.start < range.end;
    in_ram && !pvh.boot_data().into_iter().any(overlaps)
}

/// Copies the trampoline to [`TRAMPOLINE`] and sends INIT to every other CPU, then waits as
/// the SDM asks, once for all of them; afterwards each CPU only needs its start-up interrupts.
fn prepare_trampoline() -> Result<(), StartCpuError> {
    if TRAMPOLINE_READY.load(Ordering::Acquire) {
        return Ok(());
    }
    let length = trampoline_offset(&raw const eggos_trampoline_end);
    if !trampoline_page_is_free() || length > TRAMPOLINE_PAGE_SIZE {
        return Err(StartCpuError::Refused(0));
    }
    // SAFETY: the trampoline page is RAM that the allocator never hands out (`LOW_MEMORY`) and
    // that holds none of the loader's data (checked above); the direct map makes it writable.
    // The source is the trampoline in `.rodata`, `length` bytes, at most a page.
    unsafe {
        core::ptr::copy_nonoverlapping(
            &raw const eggos_trampoline_start,
            (DIRECT_MAP_OFFSET + TRAMPOLINE) as *mut u8,
            length,
        );
    }
    timer::send_init_to_others();
    delay_us(INIT_DELAY_US);
    TRAMPOLINE_READY.store(true, Ordering::Release);
    Ok(())
}

/// Busy-waits `microseconds` on the clock.
fn delay_us(microseconds: u64) {
    let end = timer::now() + crate::time::mul_div_ceil(microseconds, timer::frequency(), 1_000_000);
    while timer::now() < end {
        core::hint::spin_loop();
    }
}

/// Waits up to `microseconds` for the CPU being started to set the trampoline's taken flag.
fn wait_until_taken(microseconds: u64) -> bool {
    // SAFETY: the flag is the trampoline's u32, and the copy is in place (`start_cpu` waits
    // only after `prepare_trampoline`).
    let taken = unsafe { trampoline_word::<AtomicU32>(&raw const eggos_trampoline_taken) };
    let end = timer::now() + crate::time::mul_div_ceil(microseconds, timer::frequency(), 1_000_000);
    loop {
        if taken.load(Ordering::Acquire) != 0 {
            return true;
        }
        if timer::now() >= end {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// COM1's I/O ports.
const COM1_PORT: u16 = 0x3f8;

/// The legacy BIOS area the ACPI spec says to search for the RSDP when the loader does not
/// pass its address.
const BIOS_AREA: core::ops::Range<usize> = 0xe_0000..0x10_0000;

/// End of the boot direct map, which covers 0 .. 4 GiB: the most the kernel reaches before its
/// own page tables are active, and where the firmware's tables are.
const BOOT_DIRECT_MAP_END: u64 = 1 << 32;

/// Upper bound on any ACPI table we map as a slice, against corrupt length fields.
const MAX_TABLE_LENGTH: usize = 1 << 20;

/// How long to wait for the firmware to switch to ACPI mode, in polls of the PM1 control
/// register.
const ACPI_ENABLE_POLLS: u32 = 1_000_000;

/// The early console, COM1.
#[inline]
fn com1() -> Ns16550<PortIo> {
    // SAFETY: COM1's ports are the PC's first serial port by platform convention; only the early
    // console's transmit and receive paths use them, and neither relies on the other's register
    // state.
    let regs = unsafe { PortIo::new(COM1_PORT, Ns16550::<PortIo>::register_block_size(0)) };
    Ns16550::new(regs, 0)
}

#[derive(Debug)]
pub struct X86_64;

impl Platform for X86_64 {
    const ARCH: &'static str = "x86_64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        com1().write_bytes(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        com1().read_bytes(buffer)
    }

    fn enable_console_input() -> Result<(), &'static str> {
        console_input::enable()?;
        com1().enable_receive_interrupt();
        Ok(())
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

    #[inline]
    fn interrupt_window() {
        // SAFETY: as in `wait_for_interrupt`, with a `nop` for the `hlt`: `sti` unmasks
        // interrupts only after the instruction that follows it, so an interrupt already
        // pending is taken after the `nop`, before `cli` masks them again.
        unsafe { core::arch::asm!("sti", "nop", "cli") };
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
        // The kernel's page tables are built through the boot direct map, so only memory
        // inside it is handed out.
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    #[inline]
    fn physical_memory_offset() -> usize {
        DIRECT_MAP_OFFSET
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
        console_input::device_regions(region);
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
        // the addresses it uses now: the kernel image (code, stack, data, GDT and IDT) at its
        // virtual address, and the direct map with the local APIC in it. Loading CR3 flushes
        // every TLB entry (the boot tables set no global pages). No `nomem`: memory accesses
        // must not move across the switch.
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) *root, options(nostack, preserves_flags));
        }
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        let [root] = roots else {
            panic!("x86_64 page tables have one root");
        };
        // SAFETY: the caller guarantees the root's kernel half is the active one's, so the
        // kernel runs on at the same addresses, and that nothing uses the low half meanwhile.
        // Loading CR3 drops every cached translation (no page is global yet). No `nomem`:
        // memory accesses must not move across the switch.
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) *root, options(nostack, preserves_flags));
        }
    }

    fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
        // Without SMAP (not turned on yet), ring 0 reaches user pages directly.
        access()
    }

    unsafe fn run_user(context: &mut crate::user::UserContext) -> crate::user::UserExit {
        // SAFETY: the caller's guarantees are `run_user`'s.
        unsafe { exception::run_user(context) }
    }

    fn user_test_program() -> crate::user::TestProgram {
        exception::user_test_program()
    }

    fn random_seeds(seed: &mut dyn FnMut(crate::random::SeedSource, &[u8])) {
        let mut bytes = [0u8; random::SEED_SIZE];
        if random::fill(&mut bytes) {
            seed(crate::random::SeedSource::CpuInstruction, &bytes);
        }
        crate::random::wipe(&mut bytes);
    }

    fn linux_hardware_capabilities() -> [u64; 2] {
        let features = core::arch::x86_64::__cpuid(1).edx;
        // `AT_HWCAP2` stays 0: its `FSGSBASE` bit would tell programs they may set their GS
        // base, which needs D49's SWAPGS fence first.
        [u64::from(features), 0]
    }

    fn make_instructions_visible(_start: usize, _length: usize) {
        // x86 keeps instruction fetch coherent with stores to the same memory, and a program
        // is first entered through `iretq`, which serializes: nothing to do.
    }

    fn page_fault(kind: crate::ExceptionKind, code: u64) -> Option<paging::PageFault> {
        exception::page_fault(kind, code)
    }

    fn drop_user_translations(start: u64, length: u64) {
        let pages = length / crate::memory::FRAME_SIZE;
        if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
            // SAFETY: loading CR3 with its own value drops every cached translation (no page is
            // global yet) and changes no memory contents; the tables stay the active ones. Page
            // walks read the tables from memory, so the caller's writes are seen. No `nomem`:
            // memory accesses must not move across it.
            unsafe {
                core::arch::asm!(
                    "mov {0}, cr3",
                    "mov cr3, {0}",
                    out(reg) _,
                    options(nostack, preserves_flags)
                )
            };
            return;
        }
        for page in 0..pages {
            let address = start + page * crate::memory::FRAME_SIZE;
            // SAFETY: `invlpg` drops this CPU's translations of one page and changes no memory
            // contents. No `nomem`, as above.
            unsafe {
                core::arch::asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags))
            };
        }
    }

    unsafe fn prepare_thread(
        stack_top: usize,
        entry: extern "C" fn(usize) -> !,
        argument: usize,
    ) -> usize {
        // SAFETY: the frame, with the rounding down to 16 bytes, fits in `INITIAL_FRAME_MAX`
        // bytes, which the caller guarantees are writable and unused below `stack_top`.
        unsafe {
            crate::thread::write_initial_frame(stack_top, &thread::initial_frame(entry, argument))
        }
    }

    #[inline]
    unsafe fn switch_thread(save: *mut usize, resume: usize) {
        // SAFETY: the caller guarantees `save` is writable and `resume` a saved context whose
        // stack is allocated and not running; the switch saves and restores every register the
        // calling convention requires a call to preserve, so to each thread the call is an
        // ordinary function call.
        unsafe { thread::eggos_switch_thread(save, resume) }
    }

    unsafe fn set_user_fp(on: bool) {
        // SAFETY: the caller's contract is `set_user`'s.
        unsafe { fp::set_user(on) }
    }

    unsafe fn save_fp(state: *mut crate::fp::FpState) {
        // SAFETY: the caller's contract is `save`'s.
        unsafe { fp::save(state) }
    }

    unsafe fn restore_fp(state: *const crate::fp::FpState) {
        // SAFETY: the caller's contract is `restore`'s.
        unsafe { fp::restore(state) }
    }

    fn cpus(cpu: &mut dyn FnMut(u64)) {
        if let Some(madt) = acpi_table(b"APIC") {
            acpi::madt_local_apic_ids(madt, &mut |id| cpu(id.into()));
        }
    }

    fn boot_cpu() -> u64 {
        timer::boot_apic_id().into()
    }

    unsafe fn start_cpu(cpu: u64, start: &CpuStart) -> Result<(), StartCpuError> {
        let apic_id = u8::try_from(cpu).map_err(|_| StartCpuError::InvalidCpu)?;
        if cpu == Self::boot_cpu() {
            return Err(StartCpuError::AlreadyRunning);
        }
        prepare_trampoline()?;
        // SAFETY: the slot is the trampoline's u64, and the copy is in place. No CPU reads it
        // until the start-up interrupt below, and the previous CPU set the taken flag only
        // after reading it, so it no longer does.
        let slot = unsafe { trampoline_word::<AtomicU64>(&raw const eggos_trampoline_start_slot) };
        // SAFETY: the taken flag is the trampoline's u32; as above.
        let taken = unsafe { trampoline_word::<AtomicU32>(&raw const eggos_trampoline_taken) };
        slot.store(core::ptr::from_ref(start) as u64, Ordering::Relaxed);
        taken.store(0, Ordering::Release);
        // Keep the stores above, and the caller's writes to `*start`, before the start-up
        // interrupt's ICR write, which a release store alone would not order (x86 itself keeps
        // stores in order).
        core::sync::atomic::fence(Ordering::SeqCst);
        let page = (TRAMPOLINE >> 12) as u8;
        // A second start-up interrupt only if the first was missed (SDM 9.4.4.1 sends two).
        timer::send_startup(apic_id, page);
        if wait_until_taken(STARTUP_RETRY_US) {
            return Ok(());
        }
        timer::send_startup(apic_id, page);
        if wait_until_taken(STARTUP_TIMEOUT_US) {
            Ok(())
        } else {
            Err(StartCpuError::Refused(-1))
        }
    }

    unsafe fn set_cpu_local(block: usize) {
        // SAFETY: IA32_GS_BASE is architectural; only the kernel uses GS, and only through
        // `cpu_local`. Loading a selector into GS (the entry code does, before this) would
        // reset the base, which nothing does afterwards.
        unsafe { write_msr(IA32_GS_BASE, block as u64) };
        // SAFETY: the caller guarantees a block that lives as long as the kernel runs, with a
        // `HalCpuLocal` at `HAL_CPU_LOCAL_OFFSET`.
        let hal = unsafe {
            &*((block + crate::cpu::HAL_CPU_LOCAL_OFFSET) as *const crate::cpu::HalCpuLocal)
        };
        // SAFETY: only the HAL uses that part (the caller's guarantee), and it is this CPU's,
        // since its GS base points there now. Interrupts are masked, as the kernel runs.
        unsafe { exception::set_up_user_mode(hal) };
    }

    #[inline]
    fn cpu_local() -> usize {
        let block: usize;
        // SAFETY: the caller has set the GS base to a block whose first word holds the block's
        // own address (`set_cpu_local`'s contract), so the load reads it.
        unsafe {
            core::arch::asm!("mov {}, gs:[0]", out(reg) block, options(nostack, readonly, preserves_flags));
        }
        block
    }

    #[inline]
    unsafe fn cpu_local_u64<const OFFSET: usize>() -> u64 {
        let value: u64;
        // SAFETY: the caller guarantees the GS base is this CPU's block, with an aligned
        // `AtomicU64` (or a word nothing writes meanwhile) at `OFFSET`: one aligned 8-byte load,
        // which never tears.
        unsafe {
            core::arch::asm!(
                "mov {}, qword ptr gs:[{offset}]",
                out(reg) value,
                offset = const OFFSET,
                options(nostack, readonly, preserves_flags),
            );
        }
        value
    }

    fn wake_cpu(cpu: u64) {
        // `cpus` lists xAPIC ids only, which fit in a byte.
        timer::wake(cpu as u8);
    }
}

/// Reads model-specific register `msr`, which must be an architectural one every x86_64 CPU has
/// (reading one has no side effects).
fn read_msr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: `rdmsr` of an architectural MSR, which every x86_64 CPU has (the caller's duty), at
    // ring 0 has no side effects.
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high,
            options(nomem, nostack, preserves_flags));
    }
    u64::from(high) << 32 | u64::from(low)
}

/// Writes `value` to model-specific register `msr`.
///
/// # Safety
/// The MSR must exist and the write must keep the machine in a state the kernel expects.
unsafe fn write_msr(msr: u32, value: u64) {
    // SAFETY: guaranteed by the caller.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack, preserves_flags),
        );
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
        let start_info = in_boot_direct_map(start_info_address, PVH_START_INFO_V1_LENGTH)?;
        // SAFETY: the loader passed the address of its `hvm_start_info`; `in_boot_direct_map`
        // checked the bytes are reachable, and nothing writes them. A version 0 structure is
        // shorter: the bytes after it are still mapped memory, and the version 1 fields are
        // used only once the version says they exist.
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
        let table = in_boot_direct_map(table_address, table_length)?;
        Some(Self {
            start_info: start_info_address,
            table_address,
            // SAFETY: the loader's memory map table, `entries` entries long;
            // `in_boot_direct_map` checked it is reachable, and nothing writes it.
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

/// The validated ACPI table with `signature` that the root table lists.
fn acpi_table(signature: &[u8; 4]) -> Option<&'static [u8]> {
    let root = pvh_rsdp()
        .and_then(acpi::parse_rsdp)
        .or_else(bios_area_rsdp)?;
    let (RootTable::Rsdt(root_address) | RootTable::Xsdt(root_address)) = root;
    let root_table = table(root_address, signature_of(root))?;
    acpi::root_table_entries(root_table, root).find_map(|address| table(address, signature))
}

/// Reads the ACPI tables for the S5 sleep type and the PM1 control ports.
fn s5_target() -> Option<S5Target> {
    let fadt = acpi_table(b"FACP").and_then(acpi::parse_fadt)?;
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
    // `mapped`, not `in_boot_direct_map`: only a version 1 start info, which `PvhMemoryMap`
    // reads, is part of `physical_memory`.
    let start_info = mapped(
        PVH_START_INFO.load(Ordering::Relaxed).into(),
        PVH_START_INFO_READ_LENGTH,
    )?;
    // SAFETY: the loader passed the address of its `hvm_start_info`, which spans at least
    // `PVH_START_INFO_READ_LENGTH` bytes; `mapped` checked they are in the direct map, and
    // nothing writes them.
    let info = unsafe { physical_slice(start_info, PVH_START_INFO_READ_LENGTH) };
    let magic = u32::from_le_bytes(info[..4].try_into().ok()?);
    let rsdp = u64::from_le_bytes(info[PVH_RSDP_PADDR..].try_into().ok()?);
    if magic != PVH_START_MAGIC {
        return None;
    }
    let rsdp = mapped(rsdp, acpi::RSDP_V2_LENGTH)?;
    // SAFETY: the loader reports an RSDP there, inside the direct map (checked by `mapped`);
    // `parse_rsdp` validates the bytes, so a wrong address only fails validation.
    Some(unsafe { physical_slice(rsdp, acpi::RSDP_V2_LENGTH) })
}

/// The RSDP found by the ACPI spec's search of the legacy BIOS area.
fn bios_area_rsdp() -> Option<RootTable> {
    // SAFETY: the legacy BIOS area is in the direct map, readable ROM/RAM on PCs, and nothing
    // writes it.
    let area = unsafe { physical_slice(BIOS_AREA.start, BIOS_AREA.len()) };
    let offset = acpi::find_rsdp(area)?;
    acpi::parse_rsdp(&area[offset..])
}

/// The validated ACPI table with `signature` at physical `address`.
fn table(address: u64, signature: &[u8; 4]) -> Option<&'static [u8]> {
    let header_address = mapped(address, acpi::TABLE_HEADER_LENGTH)?;
    // SAFETY: firmware table addresses point at ACPI memory, which nothing writes; `mapped`
    // checked the header lies inside the direct map.
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

/// `address` as a pointer-sized value if `address..address + length` lies inside one range of
/// [`Platform::physical_memory`], where the firmware's tables are: the boot direct map covers
/// the first 4 GiB, the kernel's every range of `physical_memory` (early device pages as device
/// memory, D33). A table in a hole of the loader's map, or in a range a full map dropped, is
/// refused rather than read through an unmapped address. Address 0 is rejected as "no table".
fn mapped(address: u64, length: usize) -> Option<usize> {
    let pointer = in_boot_direct_map(address, length)?;
    // No overflow: `in_boot_direct_map` checked the end.
    let end = address + length as u64;
    X86_64::physical_memory()
        .ranges()
        .iter()
        .any(|range| range.start <= address && end <= range.end)
        .then_some(pointer)
}

/// `address` as a pointer-sized value if `address..address + length` is inside the first 4 GiB,
/// which the boot direct map covers; address 0 is rejected as "none". Enough for the loader's
/// own data, the start info and its memory map, which [`Platform::physical_memory`] always
/// includes, so the kernel's direct map covers them too; firmware tables need [`mapped`] (which
/// reads the memory map through this).
fn in_boot_direct_map(address: u64, length: usize) -> Option<usize> {
    let end = address.checked_add(u64::try_from(length).ok()?)?;
    if address == 0 || end > BOOT_DIRECT_MAP_END {
        return None;
    }
    usize::try_from(address).ok()
}

/// # Safety
/// Physical `address..address + length` must be memory the kernel reaches through the physical
/// memory offset (the direct map), that nothing writes while the
/// returned slice is used.
unsafe fn physical_slice(address: usize, length: usize) -> &'static [u8] {
    let virt = address.wrapping_add(DIRECT_MAP_OFFSET);
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
