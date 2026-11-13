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
mod legacy_input;
mod message_interrupts;
mod port;
mod power_button;
mod random;
mod thread;
mod timer;

use crate::acpi::{self, RootTable};
use crate::arch::boot_memory;
use crate::atomic::AtomicU64;
use crate::command_line::{self, CommandLineCopy};
use crate::cpu::{CpuStart, StartCpuError};
use crate::memory::{MemoryMap, PhysicalRange};
use crate::paging::{self, KernelImage};
use crate::{ExitStatus, Platform};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use eggos_drivers::pci::ecam::ecam_length;
use eggos_drivers::pci::mcfg;
use eggos_drivers::rtc::{Cmos, RealTimeClock, cmos};
use eggos_drivers::serial::{Ns16550, Uart};
use port::PortIo;

core::arch::global_asm!(
    r#"
    .set    KERNEL_VIRTUAL_OFFSET, {kernel_virtual_offset}

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

    movl    $(__bss_start - KERNEL_VIRTUAL_OFFSET), %edi
    movl    $(__bss_end - KERNEL_VIRTUAL_OFFSET), %ecx
    subl    %edi, %ecx
    xorl    %eax, %eax
    rep stosb

    movl    %esi, ({start_info} - KERNEL_VIRTUAL_OFFSET)

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
    xorl    %ecx, %ecx
1:  movl    %ecx, %eax
    shll    $12, %eax
    addl    $(boot_pd - KERNEL_VIRTUAL_OFFSET), %eax
    orl     $0x3, %eax
    movl    %eax, (boot_pdpt - KERNEL_VIRTUAL_OFFSET)(, %ecx, 8)
    incl    %ecx
    cmpl    $4, %ecx
    jne     1b

    xorl    %ecx, %ecx
2:  movl    %ecx, %eax
    shll    $21, %eax
    orl     $0x83, %eax             // present, writable, 2 MiB page
    movl    %eax, (boot_pd - KERNEL_VIRTUAL_OFFSET)(, %ecx, 8)
    incl    %ecx
    cmpl    $2048, %ecx
    jne     2b

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

    .pushsection .note.eggos, "a", @note
    .balign 4
    .long 6
    .long 8
    .long 1
    .asciz "EggOS"
    .balign 4
    .quad _start64_uefi - KERNEL_VIRTUAL_OFFSET
    .popsection

    .pushsection .text.boot, "ax"
    .code64
    .global _start64_uefi
_start64_uefi:
    cli
    cld
    movq    %rdi, %r12
    movq    %rsi, %r13
    leaq    __bss_start(%rip), %rdi
    leaq    __bss_end(%rip), %rcx
    subq    %rdi, %rcx
    xorl    %eax, %eax
    rep stosb
    leaq    {start_info}(%rip), %rax
    movl    %r12d, (%rax)
    leaq    {boot_handoff}(%rip), %rax
    movq    %r13, (%rax)

    leaq    boot_pml4(%rip), %rdx
    leaq    boot_pdpt(%rip), %rax
    orq     $0x3, %rax
    movq    %rax, (%rdx)
    movq    %rax, ({direct_map_index} * 8)(%rdx)
    leaq    boot_pdpt_kernel(%rip), %rax
    orq     $0x3, %rax
    movq    %rax, (511 * 8)(%rdx)
    leaq    boot_pd(%rip), %rax
    orq     $0x3, %rax
    leaq    boot_pdpt_kernel(%rip), %rcx
    movq    %rax, ({kernel_pdpt_index} * 8)(%rcx)
    leaq    boot_pdpt(%rip), %rsi
    leaq    boot_pd(%rip), %rbx
    xorl    %ecx, %ecx
4:  movq    %rcx, %rax
    shlq    $12, %rax
    addq    %rbx, %rax
    orq     $0x3, %rax
    movq    %rax, (%rsi, %rcx, 8)
    incl    %ecx
    cmpl    $4, %ecx
    jne     4b
    xorl    %ecx, %ecx
5:  movq    %rcx, %rax
    shlq    $21, %rax
    orq     $0x83, %rax
    movq    %rax, (%rbx, %rcx, 8)
    incl    %ecx
    cmpl    $2048, %ecx
    jne     5b

    movq    %cr4, %rax
    andq    $~(1 << 7), %rax
    movq    %rax, %cr4
    movq    %rdx, %cr3
    movabsq $uefi_start64_high, %rax
    jmpq    *%rax
uefi_start64_high:
    lgdt    boot_gdt_pointer64(%rip)
    movq    $__stack_top, %rsp
    pushq   $0x08
    movabsq $start64_high, %rax
    pushq   %rax
    lretq
    .popsection

    .pushsection .text.boot, "ax"
secondary_start64:
    movq    {trampoline} + (eggos_trampoline_start_slot - eggos_trampoline_start), %rbx
    movq    {trampoline} + (eggos_trampoline_page_table_root - eggos_trampoline_start), %rax
    movl    $1, {trampoline} + (eggos_trampoline_taken - eggos_trampoline_start)
    movq    %rax, %cr3
    movq    {start_stack_top}(%rbx), %rsp
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
    orl     $((1 << 8) | {efer_nxe}), %eax // EFER.LME, and NXE for the kernel's tables
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
    .global eggos_trampoline_start_slot
eggos_trampoline_start_slot:
    .quad 0
    .global eggos_trampoline_page_table_root
eggos_trampoline_page_table_root:
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
    boot_handoff = sym crate::handoff::HANDOFF_ADDRESS,
    install_exception_vectors = sym exception::install,
    init_timer = sym timer::init,
    load_idt = sym exception::load,
    secondary_main = sym secondary_main,
    start_stack_top = const core::mem::offset_of!(CpuStart, stack_top),
    trampoline = const TRAMPOLINE,
    trampoline_page = const TRAMPOLINE >> 12,
    efer_nxe = const EFER_NXE,
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

#[inline]
pub(crate) fn doorbell_barrier() {
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

static PVH_START_INFO: AtomicU32 = AtomicU32::new(0);

/// CR3 bit 63 with `CR4.PCIDE`: the load keeps the translations cached under its PCID (D552).
const CR3_NO_FLUSH: u64 = 1 << 63;
const PVH_START_MAGIC: u32 = 0x336e_c578;
const PVH_CMDLINE_PADDR: usize = 24;
const PVH_RSDP_PADDR: usize = 32;
const PVH_START_INFO_READ_LENGTH: usize = PVH_RSDP_PADDR + 8;
const PVH_VERSION: usize = 4;
const PVH_MEMMAP_PADDR: usize = 40;
const PVH_MEMMAP_ENTRIES: usize = 48;
const PVH_START_INFO_V1_LENGTH: usize = 56;
const PVH_MEMMAP_ENTRY_LENGTH: usize = 24;
const PVH_MEMMAP_TYPE_RAM: u32 = 1;
const PVH_MAX_MEMMAP_ENTRIES: usize = 1024;

const DIRECT_MAP_OFFSET: usize = 0xffff_8000_0000_0000;

const RESET_CONTROL_PORT: u16 = 0xcf9;
const RESET_CONTROL_SYSTEM_RESET: u8 = 0x02;
const RESET_CONTROL_FULL_RESET: u8 = 0x06;
const KEYBOARD_CONTROLLER_COMMAND_PORT: u16 = 0x64;
const KEYBOARD_CONTROLLER_PULSE_RESET: u8 = 0xfe;

const EFER: u32 = 0xc000_0080;
const IA32_PAT: u32 = 0x277;
const EFER_NXE: u32 = 1 << 11;
const CR0_WP: u64 = 1 << 16;

const LOW_MEMORY: PhysicalRange = PhysicalRange {
    start: 0,
    end: 0x10_0000,
};

const TRAMPOLINE: usize = 0x8000;
const TRAMPOLINE_PAGE_SIZE: usize = 4096;

static TRAMPOLINE_READY: AtomicBool = AtomicBool::new(false);

const INIT_DELAY_US: u64 = 10_000;
const STARTUP_RETRY_US: u64 = 200;
const STARTUP_TIMEOUT_US: u64 = 100_000;

const IA32_GS_BASE: u32 = 0xc000_0101;

unsafe extern "C" {
    static eggos_trampoline_start: u8;
    static eggos_trampoline_start_slot: u8;
    static eggos_trampoline_page_table_root: u8;
    static eggos_trampoline_taken: u8;
    static eggos_trampoline_end: u8;
}

extern "C" fn secondary_main(start: &CpuStart) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own, which map
    // everything this CPU uses at the addresses it uses now, as they did for the boot CPU;
    // interrupts are masked (the trampoline starts with `cli`).
    unsafe { X86_64::activate_page_tables(&start.page_table_roots[..1]) };
    timer::init_this_cpu();
    (start.entry)(start.argument)
}

fn trampoline_offset(symbol: *const u8) -> usize {
    symbol as usize - &raw const eggos_trampoline_start as usize
}

unsafe fn trampoline_word<T>(symbol: *const u8) -> &'static T {
    let address = DIRECT_MAP_OFFSET + TRAMPOLINE + trampoline_offset(symbol);
    // SAFETY: the copy is in the trampoline page, which the direct map maps and the allocator
    // never hands out; the caller guarantees the word is a `T` there. `T` is an atomic, so
    // the starting CPU's plain accesses to it are its only other use.
    unsafe { &*(address as *const T) }
}

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

fn delay_us(microseconds: u64) {
    let end = timer::now() + crate::time::mul_div_ceil(microseconds, timer::frequency(), 1_000_000);
    while timer::now() < end {
        core::hint::spin_loop();
    }
}

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

const COM1_PORT: u16 = 0x3f8;

const CMOS_CLOCK_PORT: u16 = 0x70;

static CMOS_CLOCK: AtomicU16 = AtomicU16::new(0);

fn cmos_clock() -> acpi::CmosClock {
    if let Some(known) = acpi::CmosClock::from_bits(CMOS_CLOCK.load(Ordering::Relaxed)) {
        return known;
    }
    let found = acpi_table(b"FACP").map_or(
        acpi::CmosClock::Present {
            century_register: None,
        },
        acpi::cmos_clock,
    );
    CMOS_CLOCK.store(found.to_bits(), Ordering::Relaxed);
    found
}

const BIOS_AREA: core::ops::Range<usize> = 0xe_0000..0x10_0000;

const BOOT_DIRECT_MAP_END: u64 = 1 << 32;

const MAX_TABLE_LENGTH: usize = 1 << 20;

const ACPI_ENABLE_POLLS: u32 = 1_000_000;

#[inline]
fn com1() -> Ns16550<PortIo> {
    // SAFETY: COM1's ports are the PC's first serial port by platform convention; only the early
    // console's transmit and receive paths use them. Turning receive interrupts on changes
    // registers transmit relies on, which is sound because the boot CPU does it once, before
    // any other CPU starts.
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

    fn enable_power_button() -> Result<(), &'static str> {
        power_button::enable()
    }

    fn enable_system_control() -> Result<(), &'static str> {
        power_button::forward_events()
    }

    fn unmask_system_control() {
        power_button::set_masked(false);
    }

    fn start_legacy_input() -> eggos_drivers::input::ps2::Found {
        legacy_input::start()
    }

    fn poll_legacy_input(
        budget: usize,
        event: &mut dyn FnMut(eggos_drivers::input::RawEvent),
    ) -> usize {
        legacy_input::poll(budget, event)
    }

    fn set_legacy_lights(lights: eggos_drivers::input::LockLights) -> bool {
        legacy_input::set_lights(lights)
    }

    fn message_interrupt() -> Result<(crate::DeviceLine, crate::InterruptMessage), &'static str> {
        message_interrupts::take()
    }

    fn route_legacy_interrupt(global: u32, edge: bool) -> Result<crate::DeviceLine, &'static str> {
        let signal = eggos_drivers::interrupt::io_apic::LineSignal {
            active_low: !edge,
            level_triggered: !edge,
        };
        message_interrupts::take_pin(global, signal)
    }

    fn unmask_device_interrupt(line: crate::DeviceLine) {
        message_interrupts::unmask(line);
    }

    fn release_device_interrupt(line: crate::DeviceLine) {
        message_interrupts::release(line);
    }

    fn steer_console_input(cpu: u64) -> Result<(), &'static str> {
        console_input::steer(cpu)
    }

    fn steer_device_interrupt(line: crate::DeviceLine, cpu: u64) -> Result<(), &'static str> {
        message_interrupts::steer(line, cpu)
    }

    fn message_for(
        line: crate::DeviceLine,
        cpu: u64,
    ) -> Result<crate::InterruptMessage, &'static str> {
        message_interrupts::message_for(line, cpu)
    }

    fn exit(_status: ExitStatus) -> ! {
        if let Some(target) = s5_target() {
            enter_s5(target);
        }
        Self::halt()
    }

    fn restart() -> ! {
        for (register, value) in [
            (RESET_CONTROL_PORT, RESET_CONTROL_SYSTEM_RESET),
            (RESET_CONTROL_PORT, RESET_CONTROL_FULL_RESET),
            (
                KEYBOARD_CONTROLLER_COMMAND_PORT,
                KEYBOARD_CONTROLLER_PULSE_RESET,
            ),
        ] {
            // SAFETY: port 0xCF9 is the chipset's reset control register on PC chipsets (QEMU's
            // q35 among them), and writing the full-reset value resets the machine; the 8042's
            // command port takes the pulse-reset-line command where there is no such register.
            // The machine is going down: nothing else drives these ports.
            unsafe { port::out8(register, value) };
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

    fn command_line(line: &mut dyn FnMut(&[u8])) {
        capture_command_line();
        let mut copy = [0u8; command_line::MAX_BYTES];
        let text = COMMAND_LINE.load(&mut copy);
        if !text.is_empty() {
            line(text);
        }
    }

    fn memory_map() -> MemoryMap {
        capture_command_line();
        let mut map = MemoryMap::new();
        let Some(pvh) = PvhMemoryMap::read() else {
            return map;
        };
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
        for range in pvh.boot_data() {
            map.reserve(range);
        }
        map
    }

    fn boot_direct_map_end() -> u64 {
        BOOT_DIRECT_MAP_END
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
        let mut map = MemoryMap::new();
        map.add_ram(LOW_MEMORY);
        if let Some(pvh) = PvhMemoryMap::read() {
            for (range, kind) in pvh.entries() {
                if matches!(kind, E820_RAM | E820_ACPI_RECLAIMABLE | E820_ACPI_NVS) {
                    map.add_ram(range.covering_frames());
                }
            }
            for range in pvh.boot_data() {
                map.add_ram(range.covering_frames());
            }
        }
        acpi_table_ranges(&mut |range| map.add_ram(range.covering_frames()));
        map
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        region(timer::local_apic_registers());
        console_input::device_regions(region);
    }

    fn acpi_table(signature: &[u8; 4]) -> Option<&'static [u8]> {
        acpi_table(signature)
    }

    fn acpi_tables(signature: &[u8; 4], table: &mut dyn FnMut(&'static [u8])) {
        acpi_tables(signature, table);
    }

    fn acpi_interrupt_model() -> Option<u64> {
        Some(ACPI_APIC_MODEL)
    }

    fn read_io_port(port: u16, width: u32) -> Option<u32> {
        match width {
            // SAFETY: only the ACPI interpreter calls this, for a `SystemIO` region the
            // firmware's AML declares: the ports of the firmware's own devices, which its
            // methods read.
            8 => Some(u32::from(unsafe { port::in8(port) })),
            // SAFETY: as above.
            16 => Some(u32::from(unsafe { port::in16(port) })),
            // SAFETY: as above.
            32 => Some(unsafe { port::in32(port) }),
            _ => None,
        }
    }

    fn write_io_port(port: u16, width: u32, value: u32) -> bool {
        match width {
            // SAFETY: as in `read_io_port`: a port of a `SystemIO` region the firmware
            // declares, written as its own methods write it.
            8 => unsafe { port::out8(port, value as u8) },
            // SAFETY: as above.
            16 => unsafe { port::out16(port, value as u16) },
            // SAFETY: as above.
            32 => unsafe { port::out32(port, value) },
            _ => return false,
        }
        true
    }

    fn acpi_dsdt() -> Option<&'static [u8]> {
        let fadt = acpi_table(b"FACP").and_then(acpi::parse_fadt)?;
        table(fadt.dsdt, b"DSDT")
    }

    unsafe fn platform_clock(wait_duty: fn(), clock: &mut dyn FnMut(&dyn RealTimeClock)) {
        let acpi::CmosClock::Present { century_register } = cmos_clock() else {
            return;
        };
        // SAFETY: ports 0x70 and 0x71 are the PC real-time clock's index and data ports, which
        // nothing but this driver uses, and the caller holds the one lock every call of this
        // function and every use of its clock is made under (its safety contract), so the
        // ports' accesses never interleave.
        let ports = unsafe { PortIo::new(CMOS_CLOCK_PORT, cmos::REGISTER_BLOCK_SIZE) };
        let monotonic = cmos::MonotonicClock {
            ticks: timer::now,
            frequency: timer::frequency(),
            wait_duty,
        };
        clock(&Cmos::new(ports, century_register, monotonic));
    }

    fn device_apertures(aperture: &mut dyn FnMut(PhysicalRange)) {
        let Some(mcfg) = acpi_table(b"MCFG") else {
            return;
        };
        for region in mcfg::regions(mcfg) {
            aperture(PhysicalRange::from_start_length(
                region
                    .base
                    .saturating_add(u64::from(region.start_bus) << 20),
                ecam_length(region.start_bus, region.end_bus),
            ));
        }
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [root] = roots else {
            panic!("x86_64 page tables have one root");
        };
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
        // SAFETY: `IA32_PAT` exists on every x86_64 CPU; the value keeps entries 0, 2 and 3 as at
        // reset (write-back, uncached-minus, uncached) and turns entry 1 (`PWT` alone, which no
        // mapping used) into write-combining, the same on every CPU before its tables load.
        unsafe {
            core::arch::asm!(
                "wrmsr",
                in("ecx") IA32_PAT,
                in("eax") paging::x86_64::PAT as u32,
                in("edx") (paging::x86_64::PAT >> 32) as u32,
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
        exception::barrier_predictions();
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        let [root] = roots else {
            panic!("x86_64 page tables have one root");
        };
        // SAFETY: the caller guarantees the root's kernel half is the active one's, so the
        // kernel runs on at the same addresses, and that nothing uses the low half meanwhile.
        // Loading CR3 without bit 63 drops the cached translations of its PCID, 0, which only
        // kernel tables and untagged switches use; global kernel ones stay. No `nomem`: memory
        // accesses must not move across the switch.
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) *root, options(nostack, preserves_flags));
        }
    }

    unsafe fn switch_tagged_address_space(roots: &[u64], tag: crate::asid::AddressSpaceTag) {
        if !exception::pcids_on() {
            // SAFETY: the caller's guarantees include `switch_address_space`'s.
            return unsafe { Self::switch_address_space(roots) };
        }
        let [root] = roots else {
            panic!("x86_64 page tables have one root");
        };
        let block = Self::cpu_local();
        // SAFETY: the per-CPU register holds this CPU's block, with a `HalCpuLocal` at its
        // offset; only this CPU uses its tags, with interrupts masked (the caller's duty).
        let choice = unsafe {
            let hal = &*((block + crate::cpu::HAL_CPU_LOCAL_OFFSET) as *const crate::cpu::HalCpuLocal);
            hal.address_space_tags.choose(tag)
        };
        let keep = if choice.drop_old { 0 } else { CR3_NO_FLUSH };
        let cr3 = *root | u64::from(choice.hardware_tag) | keep;
        // SAFETY: as in `switch_address_space`; with `CR4.PCIDE` set, the PCID in bits 11:0 tags
        // the space's translations, and bit 63 keeps the ones cached under it, which the tags
        // say are current: its generation has not moved since this CPU last ran it.
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags));
        }
    }

    fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
        if !exception::smap_on() {
            return access();
        }
        // SAFETY: `stac` only allows this CPU's ring 0 to reach user pages until the `clac`
        // below, which SMAP (D259) otherwise faults; nothing in `access` relies on it staying set.
        unsafe { core::arch::asm!("stac", options(nomem, nostack)) };
        let result = access();
        // SAFETY: `clac` forbids user-page access again, as SMAP requires outside this function.
        unsafe { core::arch::asm!("clac", options(nomem, nostack)) };
        result
    }

    unsafe fn run_user(context: &mut crate::user::UserContext) -> crate::user::UserExit {
        // SAFETY: the caller's guarantees are `run_user`'s.
        unsafe { exception::run_user(context) }
    }

    fn isolates_user_view() -> bool {
        exception::isolates_user_view()
    }

    fn user_view_ranges(range: &mut dyn FnMut(u64, u64)) {
        exception::user_view_ranges(Self::cpu_local(), range);
    }

    unsafe fn set_user_view(root: u64) {
        // SAFETY: the caller's guarantees are `set_user_view`'s.
        unsafe { exception::set_user_view(Self::cpu_local(), root) };
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

    fn user_counter_readable() -> bool {
        true
    }

    fn cpu_random(bytes: &mut [u8; 32]) -> bool {
        random::fill(bytes)
    }

    fn linux_hardware_capabilities() -> [u64; 2] {
        let features = core::arch::x86_64::__cpuid(1).edx;
        [u64::from(features), 0]
    }

    fn make_instructions_visible(_start: usize, _length: usize) {}

    fn page_fault(kind: crate::ExceptionKind, code: u64) -> Option<paging::PageFault> {
        exception::page_fault(kind, code)
    }

    fn drop_kernel_translations(start: u64, length: u64) {
        for page in 0..length / crate::memory::FRAME_SIZE {
            let address = start + page * crate::memory::FRAME_SIZE;
            // SAFETY: `invlpg` drops this CPU's translations of one page, global ones included;
            // it changes no memory contents.
            unsafe { core::arch::asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags)) };
        }
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
        // SAFETY: the root is the trampoline's u64; as for the slot.
        let root =
            unsafe { trampoline_word::<AtomicU64>(&raw const eggos_trampoline_page_table_root) };
        // SAFETY: the taken flag is the trampoline's u32; as above.
        let taken = unsafe { trampoline_word::<AtomicU32>(&raw const eggos_trampoline_taken) };
        slot.store(core::ptr::from_ref(start) as u64, Ordering::Relaxed);
        root.store(start.page_table_roots[0], Ordering::Relaxed);
        taken.store(0, Ordering::Release);
        core::sync::atomic::fence(Ordering::SeqCst);
        let page = (TRAMPOLINE >> 12) as u8;
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
        timer::wake(cpu as u8);
    }
}

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

    fn boot_data(&self) -> [PhysicalRange; 2] {
        [
            PhysicalRange::from_start_length(self.start_info, PVH_START_INFO_V1_LENGTH as u64),
            PhysicalRange::from_start_length(self.table_address, self.table.len() as u64),
        ]
    }
}

struct S5Target {
    fadt: acpi::Fadt,
    sleep_type: acpi::S5SleepType,
}

fn acpi_table(signature: &[u8; 4]) -> Option<&'static [u8]> {
    let root = pvh_rsdp()
        .and_then(acpi::parse_rsdp)
        .or_else(bios_area_rsdp)?;
    let (RootTable::Rsdt(root_address) | RootTable::Xsdt(root_address)) = root;
    let root_table = table(root_address, signature_of(root))?;
    acpi::root_table_entries(root_table, root).find_map(|address| table(address, signature))
}

const ACPI_APIC_MODEL: u64 = 1;

fn acpi_tables(signature: &[u8; 4], each: &mut dyn FnMut(&'static [u8])) {
    let Some(root) = pvh_rsdp()
        .and_then(acpi::parse_rsdp)
        .or_else(bios_area_rsdp)
    else {
        return;
    };
    let (RootTable::Rsdt(root_address) | RootTable::Xsdt(root_address)) = root;
    let Some(root_table) = table(root_address, signature_of(root)) else {
        return;
    };
    for address in acpi::root_table_entries(root_table, root) {
        if let Some(found) = table(address, signature) {
            each(found);
        }
    }
}

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

static COMMAND_LINE: CommandLineCopy = CommandLineCopy::new();

fn capture_command_line() {
    if COMMAND_LINE.is_taken() {
        return;
    }
    let handoff_line = crate::handoff::handoff::<X86_64>()
        .map(|handoff| crate::handoff::command_line::<X86_64>(&handoff))
        .filter(|line| !line.is_empty());
    COMMAND_LINE.store(handoff_line.or_else(pvh_command_line).unwrap_or_default());
}

fn pvh_command_line() -> Option<&'static [u8]> {
    let start_info = in_boot_direct_map(
        PVH_START_INFO.load(Ordering::Relaxed).into(),
        PVH_START_INFO_READ_LENGTH,
    )?;
    // SAFETY: the loader passed the address of its `hvm_start_info`, which spans at least
    // `PVH_START_INFO_READ_LENGTH` bytes inside the boot direct map (checked), and nothing
    // writes it.
    let info = unsafe { physical_slice(start_info, PVH_START_INFO_READ_LENGTH) };
    let magic = u32::from_le_bytes(info.get(..4)?.try_into().ok()?);
    if magic != PVH_START_MAGIC {
        return None;
    }
    let field = info.get(PVH_CMDLINE_PADDR..PVH_CMDLINE_PADDR + 8)?;
    let address = u64::from_le_bytes(field.try_into().ok()?);
    let ram = PvhMemoryMap::read();
    let mut ram_ranges = ram
        .iter()
        .flat_map(|pvh| pvh.entries())
        .filter(|&(_, kind)| kind == PVH_MEMMAP_TYPE_RAM)
        .map(|(range, _)| range);
    let region = command_line::region(address, LOW_MEMORY, &mut ram_ranges, BOOT_DIRECT_MAP_END)?;
    let length = usize::try_from(region.end - region.start).ok()?;
    let start = in_boot_direct_map(region.start, length)?;
    // SAFETY: `region` is inside the low memory, which the boot direct map covers whole and the
    // allocator never hands out, or inside a RAM entry of the loader's map, before the
    // allocator exists (the first `memory_map`); either way inside the boot direct map
    // (checked) and written by nothing while it is read here.
    let bytes = unsafe { physical_slice(start, length) };
    Some(command_line::prefix(bytes))
}

fn pvh_rsdp() -> Option<&'static [u8]> {
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

fn bios_area_rsdp() -> Option<RootTable> {
    // SAFETY: the legacy BIOS area is in the direct map, readable ROM/RAM on PCs, and nothing
    // writes it.
    let area = unsafe { physical_slice(BIOS_AREA.start, BIOS_AREA.len()) };
    let offset = acpi::find_rsdp(area)?;
    acpi::parse_rsdp(&area[offset..])
}

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

fn mapped(address: u64, length: usize) -> Option<usize> {
    let pointer = in_boot_direct_map(address, length)?;
    let end = address + length as u64;
    let in_range = |range: PhysicalRange| range.start <= address && end <= range.end;
    let described = in_range(LOW_MEMORY)
        || PvhMemoryMap::read().is_some_and(|pvh| {
            pvh.entries().any(|(range, _)| in_range(range.covering_frames()))
                || pvh.boot_data().into_iter().any(|range| in_range(range.covering_frames()))
        });
    described.then_some(pointer)
}

/// The PVH memory map's entry types the direct map covers as cached memory (D554): RAM and the
/// two ACPI kinds. Reserved ranges are left out, so a device's registers a firmware marked
/// reserved are never mapped with the wrong memory type; the ACPI tables in one (QEMU puts them
/// there) are added on their own by [`acpi_table_ranges`].
const E820_RAM: u32 = 1;
const E820_ACPI_RECLAIMABLE: u32 = 3;
const E820_ACPI_NVS: u32 = 4;

/// Calls `range` with the physical range of every ACPI table the firmware gives: the RSDP, the
/// root table, every table it lists, and the FADT's DSDT and FACS (D554).
fn acpi_table_ranges(range: &mut dyn FnMut(PhysicalRange)) {
    let mut add_table = |address: u64| {
        let Some(header_address) = mapped(address, acpi::TABLE_HEADER_LENGTH) else {
            return;
        };
        // SAFETY: firmware table addresses point at ACPI memory, which nothing writes; `mapped`
        // checked the header lies in memory the firmware described, inside the boot direct map.
        let header = unsafe { physical_slice(header_address, acpi::TABLE_HEADER_LENGTH) };
        let length = acpi::table_length(header).unwrap_or(acpi::TABLE_HEADER_LENGTH);
        range(PhysicalRange::from_start_length(address, length.min(MAX_TABLE_LENGTH) as u64));
    };
    let start_info = PVH_START_INFO.load(Ordering::Relaxed);
    let root = match pvh_rsdp() {
        Some(rsdp) => {
            if let Some(info) = mapped(start_info.into(), PVH_START_INFO_READ_LENGTH) {
                // SAFETY: as in `pvh_rsdp`.
                let info = unsafe { physical_slice(info, PVH_START_INFO_READ_LENGTH) };
                if let Ok(bytes) = info[PVH_RSDP_PADDR..].try_into() {
                    range(PhysicalRange::from_start_length(u64::from_le_bytes(bytes), 36));
                }
            }
            acpi::parse_rsdp(rsdp)
        }
        None => bios_area_rsdp(),
    };
    let Some(root) = root else {
        return;
    };
    let (RootTable::Rsdt(root_address) | RootTable::Xsdt(root_address)) = root;
    add_table(root_address);
    let Some(root_table) = table(root_address, signature_of(root)) else {
        return;
    };
    for address in acpi::root_table_entries(root_table, root) {
        add_table(address);
    }
    if let Some(fadt) = acpi_table(b"FACP") {
        if let Some(parsed) = acpi::parse_fadt(fadt) {
            add_table(parsed.dsdt);
        }
        let firmware_control = fadt
            .get(FADT_X_FIRMWARE_CTRL..FADT_X_FIRMWARE_CTRL + 8)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u64::from_le_bytes)
            .filter(|&address| address != 0)
            .or_else(|| {
                fadt.get(FADT_FIRMWARE_CTRL..FADT_FIRMWARE_CTRL + 4)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(|bytes| u64::from(u32::from_le_bytes(bytes)))
            });
        if let Some(facs) = firmware_control.filter(|&address| address != 0) {
            range(PhysicalRange::from_start_length(facs, FACS_LENGTH));
        }
    }
}

/// The FADT's `FIRMWARE_CTRL` and `X_FIRMWARE_CTRL` fields: the FACS's address (ACPI 6.5, 5.2.9).
const FADT_FIRMWARE_CTRL: usize = 36;
const FADT_X_FIRMWARE_CTRL: usize = 132;
/// The FACS is 64 bytes.
const FACS_LENGTH: u64 = 64;

fn in_boot_direct_map(address: u64, length: usize) -> Option<usize> {
    let end = address.checked_add(u64::try_from(length).ok()?)?;
    if address == 0 || end > BOOT_DIRECT_MAP_END {
        return None;
    }
    usize::try_from(address).ok()
}

unsafe fn physical_slice(address: usize, length: usize) -> &'static [u8] {
    let virt = address.wrapping_add(DIRECT_MAP_OFFSET);
    // SAFETY: guaranteed by the caller.
    unsafe { core::slice::from_raw_parts(virt as *const u8, length) }
}

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
