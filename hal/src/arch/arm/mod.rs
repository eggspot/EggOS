//! 32-bit Arm (Armv7-A with LPAE, and AArch32), part 1 (M9, D476): the boot path on QEMU
//! `virt` with a Cortex-A15.
//!
//! QEMU loads the ELF image at its physical address, 4 MiB into RAM (`0x4040_0000`, the device
//! tree sitting at RAM's start), and enters `_start` in SVC mode with the MMU off. The entry
//! masks interrupts, zeroes `.bss`, and fills the LPAE root (D474) with three 1 GiB blocks:
//! RAM's first gigabyte at [`DIRECT_MAP_OFFSET`] above it (`0xc000_0000` on, holding the kernel
//! image, linked there), the same gigabyte at its physical address for the jump, and the
//! first gigabyte of the physical space (the GIC and the PL011 UART among QEMU `virt`'s
//! devices) at its own address as device memory. It loads `MAIR0`, `TTBCR` (LPAE, `TTBR0`
//! alone, write-back walks) and `TTBR0`, turns the MMU and caches on, jumps to the virtual
//! address, sets the stack, installs the vectors (D478, D482) and the GICv2 with the virtual
//! timer, and calls the kernel's `kernel_main`, [`Arm`] being its [`crate::Platform`].

mod fp;
mod test_program;
mod thread;
mod user;

use core::sync::atomic::{AtomicU32, Ordering};

/// The direct map's offset: virtual address = physical address + this, for RAM.
pub const DIRECT_MAP_OFFSET: u32 = 0x8000_0000;
/// RAM's first physical address on QEMU `virt`, where the device tree sits.
pub const RAM_START: u32 = 0x4000_0000;
/// The PL011 UART's physical address on QEMU `virt`.
pub const UART: u32 = 0x0900_0000;
const UART_DATA: usize = 0x00;
const UART_FLAGS: usize = 0x18;
const TRANSMIT_FULL: u32 = 1 << 5;
const ROOT_ENTRIES: usize = 4;
const GIGABYTE_SHIFT: u32 = 30;
const BLOCK: u32 = 0b01;
const ACCESS_FLAG: u32 = 1 << 10;
const INNER_SHAREABLE: u32 = 0b11 << 8;
const NORMAL_INDEX: u32 = 1;
const DEVICE_INDEX: u32 = 0;
const ATTRIBUTE_SHIFT: u32 = 2;
const EXECUTE_NEVER_HIGH: u32 = (1u64 << 54 >> 32) as u32 | (1u64 << 53 >> 32) as u32;
const NORMAL_BLOCK: u32 = BLOCK | ACCESS_FLAG | INNER_SHAREABLE | NORMAL_INDEX << ATTRIBUTE_SHIFT;
const DEVICE_BLOCK: u32 = BLOCK | ACCESS_FLAG | DEVICE_INDEX << ATTRIBUTE_SHIFT;
const MAIR0: u32 = 0x0044_ff00;
const TTBCR_LPAE: u32 = 1 << 31 | 0b11 << 12 | 0b01 << 10 | 0b01 << 8;
const SCTLR_MMU_AND_CACHES: u32 = 1 | 1 << 2 | 1 << 12;
const PSCI_SYSTEM_OFF: u32 = 0x8400_0008;

#[repr(C, align(32))]
struct BootRoot([u64; ROOT_ENTRIES]);

static mut BOOT_ROOT: BootRoot = BootRoot([0; ROOT_ENTRIES]);
static DEVICE_TREE: AtomicU32 = AtomicU32::new(RAM_START);

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .arm
    .global _start
_start:
    cpsid   aif
    ldr     r4, ={offset}
    ldr     r0, =__bss_start
    sub     r0, r0, r4
    ldr     r1, =__bss_end
    sub     r1, r1, r4
    mov     r2, #0
1:  cmp     r0, r1
    strlo   r2, [r0], #4
    blo     1b

    ldr     r0, ={boot_root}
    sub     r0, r0, r4
    ldr     r1, ={ram_start}
    ldr     r2, ={normal_block}
    orr     r2, r2, r1
    mov     r3, #0
    lsr     r5, r1, #{gigabyte_shift}
    add     r6, r0, r5, lsl #3
    strd    r2, r3, [r6]
    ldr     r5, ={direct_index}
    add     r6, r0, r5, lsl #3
    strd    r2, r3, [r6]
    ldr     r2, ={device_block}
    ldr     r3, ={execute_never}
    strd    r2, r3, [r0]

    ldr     r1, ={mair0}
    mcr     p15, 0, r1, c10, c2, 0
    ldr     r1, ={ttbcr}
    mcr     p15, 0, r1, c2, c0, 2
    mov     r1, #0
    mcrr    p15, 0, r0, r1, c2
    isb
    mrc     p15, 0, r1, c1, c0, 0
    ldr     r2, ={sctlr}
    orr     r1, r1, r2
    dsb
    mcr     p15, 0, r1, c1, c0, 0
    isb

    ldr     pc, =2f
2:  ldr     sp, =__stack_top
    bl      {enable}
    bl      kernel_main
3:  wfi
    b       3b

    .global eggos_secondary_start
eggos_secondary_start:
    cpsid   aif
    mov     r8, r0
    ldr     r4, ={offset}
    ldr     r0, ={boot_root}
    sub     r0, r0, r4
    ldr     r1, ={mair0}
    mcr     p15, 0, r1, c10, c2, 0
    ldr     r1, ={ttbcr}
    mcr     p15, 0, r1, c2, c0, 2
    mov     r1, #0
    mcrr    p15, 0, r0, r1, c2
    isb
    mrc     p15, 0, r1, c1, c0, 0
    ldr     r2, ={sctlr}
    orr     r1, r1, r2
    dsb
    mcr     p15, 0, r1, c1, c0, 0
    isb
    ldr     pc, =4f
4:  ldr     sp, [r8, #{start_stack_top}]
    bl      {enable}
    mov     r0, r8
    bl      {secondary_main}
5:  wfi
    b       5b
    .ltorg
"#,
    start_stack_top = const core::mem::offset_of!(crate::cpu::CpuStart, stack_top),
    secondary_main = sym secondary_main,
    offset = const DIRECT_MAP_OFFSET,
    boot_root = sym BOOT_ROOT,
    ram_start = const RAM_START,
    normal_block = const NORMAL_BLOCK,
    device_block = const DEVICE_BLOCK,
    execute_never = const EXECUTE_NEVER_HIGH,
    gigabyte_shift = const GIGABYTE_SHIFT,
    direct_index = const (RAM_START + DIRECT_MAP_OFFSET) >> GIGABYTE_SHIFT,
    mair0 = const MAIR0,
    ttbcr = const TTBCR_LPAE,
    sctlr = const SCTLR_MMU_AND_CACHES,
    enable = sym enable_interrupts_and_fp,
);

fn uart_register(offset: usize) -> *mut u32 {
    (UART as usize + offset) as *mut u32
}

/// Writes `bytes` to the console, a carriage return before each line feed.
pub fn console_write(bytes: &[u8]) {
    for &byte in bytes {
        if byte == b'\n' {
            put_byte(b'\r');
        }
        put_byte(byte);
    }
}

fn put_byte(byte: u8) {
    // SAFETY: the boot table maps the UART's gigabyte at its physical address as device memory;
    // the flag and data registers are 32-bit and only this console uses them.
    unsafe {
        while uart_register(UART_FLAGS).read_volatile() & TRANSMIT_FULL != 0 {}
        uart_register(UART_DATA).write_volatile(u32::from(byte));
    }
}

/// The generic timer's virtual count.
pub fn clock_ticks() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: reads `CNTVCT`, which PL1 may read; no side effects.
    unsafe {
        core::arch::asm!(
            "isb",
            "mrrc p15, 1, {low}, {high}, c14",
            low = out(reg) low,
            high = out(reg) high,
        )
    };
    u64::from(high) << 32 | u64::from(low)
}

/// The generic timer's frequency, from `CNTFRQ`.
pub fn clock_frequency() -> u64 {
    let frequency: u32;
    // SAFETY: reads `CNTFRQ`; no side effects.
    unsafe { core::arch::asm!("mrc p15, 0, {f}, c14, c0, 0", f = out(reg) frequency) };
    u64::from(frequency)
}

/// The device tree's physical address: RAM's start, where QEMU puts it for an ELF image.
pub fn device_tree() -> u32 {
    DEVICE_TREE.load(Ordering::Relaxed)
}

/// Powers the machine off through PSCI's `SYSTEM_OFF`.
pub fn power_off() -> ! {
    // SAFETY: a PSCI call through the hypervisor conduit QEMU `virt` answers; `SYSTEM_OFF`
    // does not return when it works.
    unsafe { core::arch::asm!("hvc #0", inout("r0") PSCI_SYSTEM_OFF => _) };
    loop {
        // SAFETY: waits for an interrupt, with no state to keep.
        unsafe { core::arch::asm!("wfi") };
    }
}

const GIC_DISTRIBUTOR: usize = 0x0800_0000;
const GIC_CPU: usize = 0x0801_0000;
const GIC_BYTES: u64 = 0x2_0000;
const DISTRIBUTOR_CONTROL: usize = 0x000;
const SET_ENABLE: usize = 0x100;
const PRIORITY: usize = 0x400;
const CPU_CONTROL: usize = 0x000;
const PRIORITY_MASK: usize = 0x004;
const ACKNOWLEDGE: usize = 0x00c;
const END_OF_INTERRUPT: usize = 0x010;
const INTERRUPT_ID: u32 = 0x3ff;
const VIRTUAL_TIMER: u32 = 27;
const MIDDLE_PRIORITY: u8 = 0x80;
const EVERY_PRIORITY: u32 = 0xff;
const TIMER_ENABLE: u32 = 1;
const TIMER_MASKED: u32 = 1 << 1;
const UART_BYTES: u64 = 0x1000;
const BOOT_DIRECT_MAP_END: u64 = RAM_START as u64 + (768 << 20);
const DEVICE_WINDOW: core::ops::Range<u64> = 0xffc0_0000..0xffe0_0000;
const PSCI_CPU_ON: u32 = 0x8400_0003;
const PSCI_INVALID_PARAMETERS: i32 = -2;
const PSCI_ALREADY_ON: i32 = -4;
const UART_INTERRUPT_MASK: usize = 0x38;
const UART_CLEAR: usize = 0x44;
const UART_RECEIVE_EMPTY: u32 = 1 << 4;
const UART_RECEIVE_INTERRUPTS: u32 = 1 << 4 | 1 << 6;
const UART_SOURCE: u32 = 32 + 1;
const TARGETS: usize = 0x800;
const SOFTWARE_GENERATED: usize = 0xf00;
const WAKE_UP: u32 = 0;
const SPURIOUS: u32 = 1023;
const TTBR_ASID_SHIFT: u32 = 48;
const PAN_FIELD_SHIFT: u32 = 16;

static DEVICE_LINES: crate::interrupt::SourceLines = crate::interrupt::SourceLines::new();
static PAN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
const CACHE_LINE: usize = 64;
const FAULT_STATUS: u64 = 0x3f;
const TRANSLATION_ACCESS_OR_PERMISSION: [u64; 3] = [0x04, 0x08, 0x0c];
const WRITE_NOT_READ: u64 = 1 << 11;
const PREFETCH_MARK: u64 = 1 << 31;

/// The image's virtual address minus its physical address, the direct map's offset too.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = DIRECT_MAP_OFFSET as u64;

core::arch::global_asm!(
    r#"
    .section .text, "ax"
    .arm
    .balign 32
    .global arm_vectors
arm_vectors:
    b       arm_fatal
    b       arm_undefined_entry
    b       arm_svc_entry
    b       arm_prefetch_entry
    b       arm_data_entry
    b       arm_fatal
    b       arm_irq_entry
    b       arm_fatal

    .global arm_fatal
arm_fatal:
    ldr     sp, =__stack_top
    bl      {fatal}
    .ltorg
"#,
    fatal = sym fatal,
);

fn gic(base: usize, offset: usize) -> *mut u32 {
    (base + offset) as *mut u32
}

fn set_compare(deadline: u64, control: u32) {
    // SAFETY: writes `CNTV_CVAL` and `CNTV_CTL`: the virtual timer fires once the count reaches
    // `deadline`, unless `control` masks it.
    unsafe {
        core::arch::asm!(
            "mcrr p15, 3, {low}, {high}, c14",
            "mcr p15, 0, {control}, c14, c3, 1",
            "isb",
            low = in(reg) deadline as u32,
            high = in(reg) (deadline >> 32) as u32,
            control = in(reg) control,
        )
    };
}

/// Handles the interrupt the GIC has for this CPU: the virtual timer's is masked and handed to
/// the kernel.
pub(super) extern "C" fn irq() {
    // SAFETY: the GIC's CPU interface, mapped as device memory; acknowledging and ending an
    // interrupt are this handler's alone.
    let acknowledged = unsafe { gic(GIC_CPU, ACKNOWLEDGE).read_volatile() };
    let id = acknowledged & INTERRUPT_ID;
    if id == SPURIOUS {
        return;
    }
    match id {
        VIRTUAL_TIMER => {
            set_compare(u64::MAX, TIMER_ENABLE | TIMER_MASKED);
            crate::interrupt::deliver(crate::interrupt::Interrupt::Timer);
        }
        WAKE_UP => crate::interrupt::deliver(crate::interrupt::Interrupt::Wakeup),
        UART_SOURCE => {
            // SAFETY: clears the UART's receive and receive-timeout interrupts; the console
            // reads the bytes.
            unsafe { uart_register(UART_CLEAR).write_volatile(UART_RECEIVE_INTERRUPTS) };
            crate::interrupt::deliver(crate::interrupt::Interrupt::ConsoleInput);
        }
        id => {
            if let Some(line) = DEVICE_LINES.line_of(id) {
                crate::interrupt::deliver(crate::interrupt::Interrupt::Device(line));
            }
        }
    }
    // SAFETY: as above.
    unsafe { gic(GIC_CPU, END_OF_INTERRUPT).write_volatile(acknowledged) };
}

/// Enables shared interrupt `id` at the middle priority, sent to the CPU with interface number
/// `cpu` (D570).
fn enable_shared(id: u32, cpu: u32) {
    let index = id as usize;
    // SAFETY: the distributor, mapped as device memory; the byte-wide priority and target
    // registers and the set-enable bit of `id` are this kernel's to set.
    unsafe {
        ((GIC_DISTRIBUTOR + PRIORITY + index) as *mut u8).write_volatile(MIDDLE_PRIORITY);
        ((GIC_DISTRIBUTOR + TARGETS + index) as *mut u8).write_volatile(1 << (cpu & 7));
        gic(GIC_DISTRIBUTOR, SET_ENABLE + 4 * (index / 32)).write_volatile(1 << (index % 32));
    }
}

fn disable_shared(id: u32) {
    let index = id as usize;
    // SAFETY: the distributor's clear-enable bit of `id`.
    unsafe { gic(GIC_DISTRIBUTOR, 0x180 + 4 * (index / 32)).write_volatile(1 << (index % 32)) };
}

/// Called by `eggos_secondary_start` with the MMU on and the GIC's CPU interface set (D570):
/// switches to the kernel's tables, then calls the entry.
extern "C" fn secondary_main(start: &crate::cpu::CpuStart) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own, mapping what
    // this CPU uses where it uses it now; IRQs are masked.
    unsafe { <Arm as crate::Platform>::activate_page_tables(&start.page_table_roots[..1]) };
    (start.entry)(start.argument)
}

/// Turns PAN on where the CPU has it (Armv8.1's AArch32, `ID_MMFR3.PAN`; D570): the kernel then
/// reaches user pages only inside `with_user_access`.
fn enable_pan() {
    let features: u32;
    // SAFETY: reads `ID_MMFR3`; no side effects.
    unsafe { core::arch::asm!("mrc p15, 0, {}, c0, c1, 7", out(reg) features) };
    if features >> PAN_FIELD_SHIFT & 0xf == 0 {
        return;
    }
    PAN.store(true, Ordering::Relaxed);
    set_pan(true);
}

fn set_pan(on: bool) {
    // SAFETY: `SETPAN` only changes whether privileged accesses to user pages fault; the CPU has
    // it (`enable_pan` checked).
    unsafe {
        match on {
            true => core::arch::asm!(".inst 0xf1100208"),
            false => core::arch::asm!(".inst 0xf1100008"),
        }
    }
}

extern "C" fn fatal() -> ! {
    console_write(b"EggOS arm: an exception in the kernel\n");
    power_off()
}

extern "C" fn enable_interrupts_and_fp() {
    // SAFETY: points `VBAR` at the vector table, gives the kernel's coprocessor access to VFP
    // (`CPACR`, for the save and restore code only), and turns on the GIC's distributor and CPU
    // interface for the virtual timer's interrupt with the timer masked; IRQs stay masked in
    // `CPSR` but in `wait_for_interrupt`.
    unsafe {
        core::arch::asm!(
            "mcr p15, 0, {vectors}, c12, c0, 0",
            "mrc p15, 0, {access}, c1, c0, 2",
            "orr {access}, {access}, #0xf00000",
            "mcr p15, 0, {access}, c1, c0, 2",
            "isb",
            vectors = in(reg) arm_vectors_address(),
            access = out(reg) _,
        );
        let priority = (GIC_DISTRIBUTOR + PRIORITY + VIRTUAL_TIMER as usize) as *mut u8;
        priority.write_volatile(MIDDLE_PRIORITY);
        gic(GIC_DISTRIBUTOR, SET_ENABLE).write_volatile(1 << VIRTUAL_TIMER);
        gic(GIC_DISTRIBUTOR, DISTRIBUTOR_CONTROL).write_volatile(1);
        gic(GIC_CPU, PRIORITY_MASK).write_volatile(EVERY_PRIORITY);
        gic(GIC_CPU, CPU_CONTROL).write_volatile(1);
    }
    set_compare(u64::MAX, TIMER_ENABLE | TIMER_MASKED);
    enable_pan();
}

fn arm_vectors_address() -> usize {
    unsafe extern "C" {
        static arm_vectors: u8;
    }
    &raw const arm_vectors as usize
}

/// Zeroes the `length` bytes at `start` ([`crate::memory::zero`]).
///
/// # Safety
/// As for [`crate::memory::zero`].
#[inline]
pub(crate) unsafe fn zero(start: *mut u8, length: usize) {
    // SAFETY: guaranteed by the caller.
    unsafe { crate::memory::zero_by_lines(start, length) }
}

#[inline]
pub(crate) fn doorbell_barrier() {
    // SAFETY: a barrier: it orders stores and touches nothing.
    unsafe { core::arch::asm!("dsb st") };
}

fn drop_every_translation() {
    // SAFETY: invalidates this CPU's unified TLB (`TLBIALL`), then waits for it; no memory
    // changes.
    unsafe { core::arch::asm!("dsb", "mcr p15, 0, {}, c8, c7, 0", "dsb", "isb", in(reg) 0) };
}

/// 32-bit Arm on QEMU `virt` with a Cortex-A15 (D482).
#[derive(Debug)]
pub struct Arm;

impl crate::Platform for Arm {
    const ARCH: &'static str = "arm";

    type PageTableFormat = crate::paging::arm::ArmLpae;

    fn console_write(bytes: &[u8]) {
        console_write(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        let mut read = 0;
        for byte in buffer.iter_mut() {
            // SAFETY: the UART's flag and data registers, mapped as device memory.
            let empty = unsafe { uart_register(UART_FLAGS).read_volatile() } & UART_RECEIVE_EMPTY;
            if empty != 0 {
                break;
            }
            // SAFETY: as above; the receive FIFO has a byte.
            *byte = unsafe { uart_register(UART_DATA).read_volatile() } as u8;
            read += 1;
        }
        read
    }

    fn enable_console_input() -> Result<(), &'static str> {
        // SAFETY: unmasks the UART's receive and receive-timeout interrupts (D570).
        unsafe { uart_register(UART_INTERRUPT_MASK).write_volatile(UART_RECEIVE_INTERRUPTS) };
        enable_shared(UART_SOURCE, 0);
        Ok(())
    }

    fn route_device_interrupt(
        specifier: &crate::fdt::InterruptSpecifier,
    ) -> Result<crate::DeviceLine, &'static str> {
        let (id, _) =
            eggos_drivers::interrupt::gicv2::shared_interrupt_of_specifier(specifier.cells())?;
        if id == UART_SOURCE {
            return Err("the interrupt is the console's");
        }
        let (line, new) = DEVICE_LINES
            .claim(id)
            .ok_or("every device interrupt line is taken")?;
        if new {
            enable_shared(id, 0);
        }
        Ok(line)
    }

    fn release_device_interrupt(line: crate::DeviceLine) {
        if let Some(id) = DEVICE_LINES.release(line) {
            disable_shared(id);
        }
    }

    fn steer_device_interrupt(line: crate::DeviceLine, cpu: u64) -> Result<(), &'static str> {
        let id = DEVICE_LINES.source_of(line).ok_or("the line is not routed")?;
        enable_shared(id, cpu as u32);
        Ok(())
    }

    fn exit(_status: crate::ExitStatus) -> ! {
        power_off()
    }

    fn restart() -> ! {
        power_off()
    }

    fn halt() -> ! {
        loop {
            // SAFETY: waits for an interrupt, with no state to keep.
            unsafe { core::arch::asm!("wfi") };
        }
    }

    fn breakpoint() {
        // SAFETY: `bkpt` takes the prefetch abort vector, which stops the machine in the
        // kernel; the kernel calls this only in its self-test of that path.
        unsafe { core::arch::asm!("bkpt #0") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: `udf` is permanently undefined; the kernel never resumes from it.
        unsafe { core::arch::asm!("udf #0", options(noreturn)) }
    }

    fn clock_ticks() -> u64 {
        clock_ticks()
    }

    fn clock_frequency() -> u64 {
        clock_frequency()
    }

    fn set_timer_deadline(deadline: u64) {
        set_compare(deadline, TIMER_ENABLE);
    }

    fn wait_for_interrupt() {
        // SAFETY: `wfi` wakes on a pending interrupt even with IRQs masked; unmasking takes it
        // through the vectors, and masking again keeps the kernel's invariant.
        unsafe { core::arch::asm!("wfi", "cpsie i", "cpsid i") };
    }

    fn interrupt_window() {
        // SAFETY: as in `wait_for_interrupt`, without the `wfi`.
        unsafe { core::arch::asm!("cpsie i", "cpsid i") };
    }

    fn memory_map() -> crate::MemoryMap {
        // SAFETY: QEMU puts the device tree at RAM's start for an ELF image; it stays readable
        // and unwritten, in RAM the boot direct map covers.
        let mut map =
            unsafe { crate::arch::boot_memory::device_tree_memory_map(RAM_START as usize) };
        map.retain_within(crate::memory::PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    fn physical_memory_offset() -> usize {
        DIRECT_MAP_OFFSET as usize
    }

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET as usize
    }

    fn kernel_image() -> crate::paging::KernelImage {
        crate::arch::boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> crate::MemoryMap {
        // SAFETY: as in `memory_map`.
        let mut map = unsafe { crate::arch::boot_memory::device_tree_ram(RAM_START as usize) };
        map.retain_within(crate::memory::PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    fn early_device_regions(region: &mut dyn FnMut(crate::memory::PhysicalRange)) {
        region(crate::memory::PhysicalRange::from_start_length(
            u64::from(UART),
            UART_BYTES,
        ));
        region(crate::memory::PhysicalRange::from_start_length(
            GIC_DISTRIBUTOR as u64,
            GIC_BYTES,
        ));
    }

    fn device_tree() -> Option<&'static [u8]> {
        // SAFETY: as in `memory_map`.
        unsafe { crate::arch::boot_memory::device_tree_blob(RAM_START as usize) }
    }

    fn device_window() -> Option<core::ops::Range<u64>> {
        Some(DEVICE_WINDOW)
    }

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        drop_every_translation();
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [root] = roots else {
            panic!("LPAE tables here have one root");
        };
        // SAFETY: the caller guarantees the tables map the kernel and the direct map at the
        // addresses in use; `TTBR0` takes the new root, then the TLB is emptied.
        unsafe {
            core::arch::asm!(
                "dsb",
                "mcrr p15, 0, {low}, {high}, c2",
                "isb",
                low = in(reg) *root as u32,
                high = in(reg) (*root >> 32) as u32,
            )
        };
        drop_every_translation();
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        // SAFETY: the caller's guarantees include `activate_page_tables`'.
        unsafe { Self::activate_page_tables(roots) };
    }

    fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
        let pan = PAN.load(Ordering::Relaxed);
        if pan {
            set_pan(false);
        }
        let result = access();
        if pan {
            set_pan(true);
        }
        result
    }

    unsafe fn switch_tagged_address_space(roots: &[u64], tag: crate::asid::AddressSpaceTag) {
        let [root] = roots else {
            panic!("LPAE tables here have one root");
        };
        let block = Self::cpu_local();
        // SAFETY: the per-CPU register holds this CPU's block with a `HalCpuLocal` at its
        // offset; only this CPU uses its tags, with IRQs masked (the caller's duty).
        let choice = unsafe {
            let hal = &*((block + crate::cpu::HAL_CPU_LOCAL_OFFSET) as *const crate::cpu::HalCpuLocal);
            hal.address_space_tags.choose(tag)
        };
        let asid = u64::from(choice.hardware_tag) << TTBR_ASID_SHIFT;
        let ttbr = *root | asid;
        // SAFETY: as in `activate_page_tables`, with the space's ASID in `TTBR0` (D570): user
        // translations are not global and carry it, so `TLBIASID` drops only that space's,
        // and only when the tags say they are stale.
        unsafe {
            core::arch::asm!(
                "dsb",
                "mcrr p15, 0, {low}, {high}, c2",
                "isb",
                low = in(reg) ttbr as u32,
                high = in(reg) (ttbr >> 32) as u32,
            );
            if choice.drop_old {
                core::arch::asm!(
                    "mcr p15, 0, {asid}, c8, c7, 2",
                    "dsb",
                    "isb",
                    asid = in(reg) u32::from(choice.hardware_tag),
                );
            }
        }
    }

    unsafe fn run_user(context: &mut crate::user::UserContext) -> crate::user::UserExit {
        // SAFETY: the caller's guarantees are `run_user`'s.
        unsafe { user::run_user(context) }
    }

    fn user_test_program() -> crate::user::TestProgram {
        test_program::user_test_program()
    }

    fn linux_hardware_capabilities() -> [u64; 2] {
        const HALF_WORD: u64 = 1 << 1;
        const THUMB: u64 = 1 << 2;
        const FAST_MULTIPLY: u64 = 1 << 4;
        const VFP: u64 = 1 << 6;
        const EDSP: u64 = 1 << 7;
        const NEON: u64 = 1 << 12;
        const VFPV3: u64 = 1 << 13;
        const TLS: u64 = 1 << 15;
        const VFPV4: u64 = 1 << 16;
        const DIVIDE: u64 = 1 << 17 | 1 << 18;
        const LPAE: u64 = 1 << 20;
        [
            HALF_WORD
                | THUMB
                | FAST_MULTIPLY
                | VFP
                | EDSP
                | NEON
                | VFPV3
                | TLS
                | VFPV4
                | DIVIDE
                | LPAE,
            0,
        ]
    }

    fn random_seeds(seed: &mut dyn FnMut(crate::random::SeedSource, &[u8])) {
        // SAFETY: as in `memory_map`, once, on the boot CPU.
        unsafe { crate::arch::boot_memory::device_tree_take_seeds(RAM_START as usize, seed) };
    }

    fn make_instructions_visible(start: usize, length: usize) {
        let first = start & !(CACHE_LINE - 1);
        for line in (first..start + length).step_by(CACHE_LINE) {
            // SAFETY: cleans one data-cache line to the point of unification (`DCCMVAU`), so
            // instruction fetch can see what was written there; no memory contents change.
            unsafe { core::arch::asm!("mcr p15, 0, {}, c7, c11, 1", in(reg) line) };
        }
        // SAFETY: waits for the cleans, then invalidates every instruction cache in the inner
        // shareable domain (`ICIALLUIS`); no memory contents change.
        unsafe { core::arch::asm!("dsb", "mcr p15, 0, {}, c7, c1, 0", "dsb", "isb", in(reg) 0) };
    }

    fn drop_user_translations(_start: u64, _length: u64) {
        drop_every_translation();
    }

    fn page_fault(_kind: crate::ExceptionKind, code: u64) -> Option<crate::paging::PageFault> {
        use crate::paging::{Access, PageFault};
        let status = code & FAULT_STATUS & !0b11;
        if !TRANSLATION_ACCESS_OR_PERMISSION.contains(&status) {
            return None;
        }
        let access = match (code & PREFETCH_MARK != 0, code & WRITE_NOT_READ != 0) {
            (true, _) => Access::Execute,
            (false, true) => Access::Write,
            (false, false) => Access::Read,
        };
        Some(PageFault {
            access,
            may_be_stale: false,
        })
    }

    unsafe fn prepare_thread(
        stack_top: usize,
        entry: extern "C" fn(usize) -> !,
        argument: usize,
    ) -> usize {
        // SAFETY: the frame fits in `INITIAL_FRAME_MAX`, writable and unused below the top.
        unsafe {
            crate::thread::write_initial_frame(stack_top, &thread::initial_frame(entry, argument))
        }
    }

    unsafe fn switch_thread(save: *mut usize, resume: usize) {
        // SAFETY: the caller's contract is the switch's.
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
        // SAFETY: as in `memory_map`.
        unsafe { crate::arch::boot_memory::device_tree_cpus(RAM_START as usize, cpu) };
    }

    fn boot_cpu() -> u64 {
        0
    }

    unsafe fn start_cpu(
        cpu: u64,
        start: &crate::cpu::CpuStart,
    ) -> Result<(), crate::cpu::StartCpuError> {
        unsafe extern "C" {
            fn eggos_secondary_start();
        }
        let entry = (eggos_secondary_start as *const () as u32).wrapping_sub(DIRECT_MAP_OFFSET);
        let context = core::ptr::from_ref(start) as u32;
        core::sync::atomic::fence(Ordering::SeqCst);
        let result: i32;
        // SAFETY: PSCI `CPU_ON` through the hypervisor conduit QEMU `virt` answers (D570):
        // CPU `cpu` starts at the physical address of the secondary entry with `context` in
        // `r0`; the caller guarantees what that code relies on.
        unsafe {
            core::arch::asm!(
                "dsb",
                "hvc #0",
                inout("r0") PSCI_CPU_ON => result,
                in("r1") cpu as u32,
                in("r2") entry,
                in("r3") context,
            )
        };
        match result {
            0 => Ok(()),
            PSCI_INVALID_PARAMETERS => Err(crate::cpu::StartCpuError::InvalidCpu),
            PSCI_ALREADY_ON => Err(crate::cpu::StartCpuError::AlreadyRunning),
            error => Err(crate::cpu::StartCpuError::Refused(i64::from(error))),
        }
    }

    unsafe fn set_cpu_local(block: usize) {
        // SAFETY: `TPIDRPRW` is the kernel's alone; user mode cannot read it.
        unsafe { core::arch::asm!("mcr p15, 0, {}, c13, c0, 4", in(reg) block) };
    }

    fn cpu_local() -> usize {
        let block: usize;
        // SAFETY: reads `TPIDRPRW`.
        unsafe { core::arch::asm!("mrc p15, 0, {}, c13, c0, 4", out(reg) block) };
        block
    }

    fn wake_cpu(cpu: u64) {
        // SAFETY: `GICD_SGIR` sends software interrupt `WAKE_UP` to the CPU with interface
        // number `cpu` (D570); the fence orders the waker's stores before it.
        unsafe {
            core::arch::asm!("dsb");
            gic(GIC_DISTRIBUTOR, SOFTWARE_GENERATED)
                .write_volatile(1 << (16 + (cpu as u32 & 7)) | WAKE_UP);
        }
    }
}
