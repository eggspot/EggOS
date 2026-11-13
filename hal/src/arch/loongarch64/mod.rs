//! LoongArch64 (LA464 and later; D290).
//!
//! Boot contract: QEMU `virt` loads the image at its physical address and enters `_start` in
//! direct address mode, with `a0` the boot flag, `a1` the command line's and `a2` the EFI system
//! table's physical addresses, both in the first megabyte, where QEMU keeps the boot information
//! it builds. The system table's configuration tables name the device tree, which describes the
//! RAM, the CPUs and the devices.
//!
//! Translation: two direct mapping windows serve the boot (`DMW0` uncached at `0x8000...`, for
//! the console's registers, which keeps it; `DMW1` cached at `0x9000...`, until the kernel's own
//! tables are active); everything else goes through page tables in the [`paging::loongarch64`]
//! format, which the TLB refill handler walks. The per-CPU block is in `$r21`, which the
//! psABI reserves and compilers never allocate; the entry from user mode puts the kernel's back.

mod exception;
mod fp;
mod interrupts;
mod thread;
mod timer;

use crate::arch::boot_memory;
use crate::cpu::{CpuStart, StartCpuError};
use crate::memory::PhysicalRange;
use crate::paging::{self, Attributes, KernelImage, MemoryType};
use crate::{ExitStatus, MemoryMap, Platform};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_drivers::io::Mmio;
use eggos_drivers::serial::{Ns16550, Uart};

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .global _start
_start:
    move        $r21, $zero
    bl          loongarch64_boot_windows

    la.pcrel    $t0, __bss_start
    la.pcrel    $t1, __bss_end
1:  bgeu        $t0, $t1, 2f
    st.d        $zero, $t0, 0
    addi.d      $t0, $t0, 8
    b           1b

2:  li.d        $t2, {cached_window}
    or          $t3, $a2, $t2
    ld.d        $t4, $t3, 0
    li.d        $t5, {systab_signature}
    bne         $t4, $t5, 6f
    ld.d        $t4, $t3, {systab_count}
    ld.d        $t5, $t3, {systab_tables}
    or          $t5, $t5, $t2
    li.d        $t6, {fdt_guid_low}
    li.d        $t7, {fdt_guid_high}
3:  beqz        $t4, 4f
    ld.d        $t8, $t5, 0
    bne         $t8, $t6, 5f
    ld.d        $t8, $t5, 8
    bne         $t8, $t7, 5f
    ld.d        $t8, $t5, 16
    la.pcrel    $t0, {device_tree}
    st.d        $t8, $t0, 0
    b           4f
5:  addi.d      $t5, $t5, 24
    addi.d      $t4, $t4, -1
    b           3b

4:  or          $t3, $a1, $t2
    la.pcrel    $t4, {command_line}
    li.w        $t5, {command_line_words}
7:  ld.d        $t6, $t3, 0
    st.d        $t6, $t4, 0
    addi.d      $t3, $t3, 8
    addi.d      $t4, $t4, 8
    addi.w      $t5, $t5, -1
    bnez        $t5, 7b

6:  move        $t3, $t2
    li.w        $t5, 512
8:  st.d        $zero, $t3, 0
    addi.d      $t3, $t3, 8
    addi.w      $t5, $t5, -1
    bnez        $t5, 8b

    la.pcrel    $t0, {boot_root}
    la.pcrel    $t1, {boot_direct}
    bstrpick.d  $t3, $t1, 47, 0
    li.d        $t5, {direct_map_slot}
    stx.d       $t3, $t0, $t5
    la.pcrel    $t1, {boot_kernel}
    bstrpick.d  $t3, $t1, 47, 0
    li.d        $t5, {kernel_root_slot}
    stx.d       $t3, $t0, $t5
    la.pcrel    $t1, {boot_direct}
    li.d        $t3, {direct_leaf}
    li.d        $t4, 1 << 30
    li.w        $t5, 512
9:  st.d        $t3, $t1, 0
    add.d       $t3, $t3, $t4
    addi.d      $t1, $t1, 8
    addi.w      $t5, $t5, -1
    bnez        $t5, 9b
    la.pcrel    $t1, {boot_kernel}
    li.d        $t3, {kernel_leaf}
    li.d        $t5, {kernel_gigabyte_slot}
    stx.d       $t3, $t1, $t5

    bl          loongarch64_boot_walker
    la.pcrel    $t0, 10f
    ld.d        $t0, $t0, 0
    jirl        $zero, $t0, 0
    .balign 8
10: .dword      11f

11: la.pcrel    $sp, __stack_top
    la.pcrel    $t0, loongarch64_kernel_entry
    csrwr       $t0, {csr_eentry}
    csrwr       $zero, {csr_ecfg}
    bl          {init_timer}
    bl          kernel_main
12: idle        0
    b           12b

loongarch64_boot_windows:
    li.d        $t0, {dmw0}
    csrwr       $t0, {csr_dmw0}
    li.d        $t0, {dmw1}
    csrwr       $t0, {csr_dmw1}
    li.d        $t0, {cached_window}
    or          $ra, $ra, $t0
    pcaddi      $t1, 0
    or          $t1, $t1, $t0
    jirl        $zero, $t1, 12
    li.w        $t0, {crmd_paged}
    csrwr       $t0, {csr_crmd}
    csrwr       $zero, {csr_euen}
    jirl        $zero, $ra, 0

loongarch64_boot_walker:
    li.d        $t0, {pwcl}
    csrwr       $t0, {csr_pwcl}
    li.d        $t0, {pwch}
    csrwr       $t0, {csr_pwch}
    li.w        $t0, {page_shift}
    csrwr       $t0, {csr_stlbps}
    li.w        $t0, {page_shift}
    csrwr       $t0, {csr_tlbrehi}
    la.pcrel    $t0, loongarch64_tlb_refill
    bstrpick.d  $t0, $t0, 47, 0
    csrwr       $t0, {csr_tlbrentry}
    la.pcrel    $t0, {boot_root}
    bstrpick.d  $t0, $t0, 47, 0
    csrwr       $t0, {csr_pgdh}
    csrwr       $zero, {csr_pgdl}
    invtlb      0, $zero, $zero
    jirl        $zero, $ra, 0

    .global eggos_secondary_start
eggos_secondary_start:
    move        $r21, $zero
    bl          loongarch64_boot_windows
    li.d        $t0, {mailbox1}
    iocsrrd.d   $s0, $t0
    bl          loongarch64_boot_walker
    la.pcrel    $t0, 13f
    ld.d        $t0, $t0, 0
    jirl        $zero, $t0, 0
    .balign 8
13: .dword      14f
14: ld.d        $sp, $s0, {start_stack_top}
    la.pcrel    $t0, loongarch64_kernel_entry
    csrwr       $t0, {csr_eentry}
    csrwr       $zero, {csr_ecfg}
    move        $a0, $s0
    bl          {secondary_main}
15: idle        0
    b           15b
"#,
    cached_window = const CACHED_WINDOW,
    dmw0 = const DMW0,
    dmw1 = const DMW1,
    crmd_paged = const CRMD_PAGED,
    csr_crmd = const csr::CRMD,
    csr_euen = const csr::EUEN,
    csr_ecfg = const csr::ECFG,
    csr_eentry = const csr::EENTRY,
    csr_pgdl = const csr::PGDL,
    csr_pgdh = const csr::PGDH,
    csr_pwcl = const csr::PWCL,
    csr_pwch = const csr::PWCH,
    csr_stlbps = const csr::STLBPS,
    csr_tlbrentry = const csr::TLBRENTRY,
    csr_tlbrehi = const csr::TLBREHI,
    csr_dmw0 = const csr::DMW0,
    csr_dmw1 = const csr::DMW1,
    pwcl = const paging::loongarch64::PWCL,
    pwch = const paging::loongarch64::PWCH,
    page_shift = const 12,
    systab_signature = const EFI_SYSTEM_TABLE_SIGNATURE,
    systab_count = const EFI_SYSTEM_TABLE_COUNT,
    systab_tables = const EFI_SYSTEM_TABLE_TABLES,
    fdt_guid_low = const DEVICE_TREE_GUID[0],
    fdt_guid_high = const DEVICE_TREE_GUID[1],
    device_tree = sym DEVICE_TREE,
    command_line = sym COMMAND_LINE,
    command_line_words = const COMMAND_LINE_BYTES / 8,
    boot_root = sym BOOT_ROOT,
    boot_direct = sym BOOT_DIRECT,
    boot_kernel = sym BOOT_KERNEL,
    direct_map_slot = const paging::index(DIRECT_MAP_OFFSET as u64, 3) * 8,
    kernel_root_slot = const paging::index(KERNEL_VIRTUAL_OFFSET, 3) * 8,
    kernel_gigabyte_slot = const paging::index(KERNEL_VIRTUAL_OFFSET, 2) * 8,
    direct_leaf = const paging::loongarch64::leaf(0, 2, Attributes::READ_WRITE),
    kernel_leaf = const paging::loongarch64::leaf(0, 2, BOOT_KERNEL_ATTRIBUTES),
    init_timer = sym timer::init,
    mailbox1 = const iocsr::MAILBOX1,
    start_stack_top = const core::mem::offset_of!(CpuStart, stack_top),
    secondary_main = sym secondary_main,
);

/// The image's virtual address minus its physical address: the kernel links at
/// `0xffff_ffff_8020_0000`, in the top 2 GiB (D33). `linker.ld` uses the same value and checks
/// it against this one.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = 0xffff_ffff_8000_0000;

/// The control and status registers the HAL uses (LoongArch Reference Manual, volume 1, 7.1).
pub(super) mod csr {
    pub const CRMD: u32 = 0x0;
    pub const PRMD: u32 = 0x1;
    pub const EUEN: u32 = 0x2;
    pub const ECFG: u32 = 0x4;
    pub const ESTAT: u32 = 0x5;
    pub const ERA: u32 = 0x6;
    pub const BADV: u32 = 0x7;
    pub const EENTRY: u32 = 0xc;
    pub const PGDL: u32 = 0x19;
    pub const PGDH: u32 = 0x1a;
    pub const PWCL: u32 = 0x1c;
    pub const PWCH: u32 = 0x1d;
    pub const STLBPS: u32 = 0x1e;
    pub const CPUID: u32 = 0x20;
    pub const SAVE0: u32 = 0x30;
    pub const SAVE7: u32 = 0x37;
    pub const TCFG: u32 = 0x41;
    pub const TICLR: u32 = 0x44;
    pub const TLBRENTRY: u32 = 0x88;
    pub const TLBRSAVE: u32 = 0x8b;
    pub const TLBREHI: u32 = 0x8e;
    pub const DMW0: u32 = 0x180;
    pub const DMW1: u32 = 0x181;
}

/// The IOCSR registers of a core's interrupt and mail block (Loongson 3A5000 manual, 10.2 and
/// 11.2), which QEMU `virt` models.
pub(super) mod iocsr {
    pub const IPI_STATUS: u64 = 0x1000;
    pub const IPI_ENABLE: u64 = 0x1004;
    pub const IPI_CLEAR: u64 = 0x100c;
    pub const MAILBOX0: u64 = 0x1020;
    pub const MAILBOX1: u64 = 0x1028;
    pub const IPI_SEND: u64 = 0x1040;
    pub const MAIL_SEND: u64 = 0x1048;
    pub const SEND_BLOCKING: u64 = 1 << 31;
    pub const SEND_CPU_SHIFT: u32 = 16;
    pub const MAIL_BOX_SHIFT: u32 = 2;
    pub const MAIL_DATA_SHIFT: u32 = 32;
}

const DMW0: u64 = 0x8000_0000_0000_0001;
const DMW1: u64 = 0x9000_0000_0000_0011;
const UNCACHED_WINDOW: usize = 0x8000_0000_0000_0000;
const CACHED_WINDOW: u64 = 0x9000_0000_0000_0000;
const CRMD_PAGED: u64 = 0xb0;

const EFI_SYSTEM_TABLE_SIGNATURE: u64 = 0x5453_5953_2049_4249;
const EFI_SYSTEM_TABLE_COUNT: u64 = 104;
const EFI_SYSTEM_TABLE_TABLES: u64 = 112;
const DEVICE_TREE_GUID: [u64; 2] = [0x41a5_f19c_b1b6_21d5, 0xe0aa_692c_15d9_0b83];

const DIRECT_MAP_OFFSET: usize = 0xffff_8000_0000_0000;
const BOOT_DIRECT_MAP_END: u64 = 512 << 30;
const KERNEL_LOAD_ADDRESS: u64 = 0x20_0000;

const BOOT_KERNEL_ATTRIBUTES: Attributes = Attributes {
    writable: true,
    executable: true,
    memory_type: MemoryType::Normal,
    user: false,
};

#[repr(C, align(4096))]
struct BootTable([u64; paging::ENTRIES]);

static mut BOOT_ROOT: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_DIRECT: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_KERNEL: BootTable = BootTable([0; paging::ENTRIES]);

static DEVICE_TREE: AtomicUsize = AtomicUsize::new(0);

const COMMAND_LINE_BYTES: usize = 512;

static mut COMMAND_LINE: [u8; COMMAND_LINE_BYTES] = [0; COMMAND_LINE_BYTES];

const UART_BASE: usize = 0x1fe0_01e0;
const UART_REG_SHIFT: u32 = 0;

const GED_SLEEP_CONTROL: usize = 0x100e_001c;
const GED_RESET: usize = 0x100e_001e;
const GED_POWER_OFF: u8 = 0x34;
const GED_RESET_VALUE: u8 = 0x42;

const DEFAULT_RAM_END: u64 = 256 << 20;

macro_rules! csr_read {
    ($csr:expr) => {{
        let value: u64;
        // SAFETY: reading a CSR at privilege 0 has no side effects.
        unsafe {
            core::arch::asm!("csrrd {0}, {csr}", out(reg) value, csr = const $csr, options(nomem, nostack))
        };
        value
    }};
}
pub(super) use csr_read;

macro_rules! csr_write {
    ($csr:expr, $value:expr) => {{
        let value: u64 = $value;
        // SAFETY: the caller's statement about the CSR it names; `csrwr` also returns the old
        // value, which is discarded.
        unsafe {
            core::arch::asm!("csrwr {0}, {csr}", inout(reg) value => _, csr = const $csr, options(nomem, nostack))
        };
    }};
}
pub(super) use csr_write;

/// Reads the 32-bit IOCSR register `address`.
pub(super) fn iocsr_read32(address: u64) -> u32 {
    let value: u32;
    // SAFETY: reading this core's interrupt and mail block has no side effects but the ones its
    // registers define, which the callers want.
    unsafe {
        core::arch::asm!("iocsrrd.w {0}, {1}", out(reg) value, in(reg) address, options(nostack))
    };
    value
}

/// Writes the 32-bit IOCSR register `address`.
pub(super) fn iocsr_write32(address: u64, value: u32) {
    // SAFETY: as in `iocsr_read32`: the callers write the registers their meaning allows.
    unsafe {
        core::arch::asm!("iocsrwr.w {0}, {1}", in(reg) value, in(reg) address, options(nostack))
    };
}

fn iocsr_write64(address: u64, value: u64) {
    // SAFETY: as in `iocsr_write32`.
    unsafe {
        core::arch::asm!("iocsrwr.d {0}, {1}", in(reg) value, in(reg) address, options(nostack))
    };
}

fn send_mail(cpu: u64, mailbox: u64, data: u64) {
    let base = iocsr::SEND_BLOCKING | cpu << iocsr::SEND_CPU_SHIFT;
    let high = base | (mailbox * 2 + 1) << iocsr::MAIL_BOX_SHIFT | (data & 0xffff_ffff_0000_0000);
    iocsr_write64(iocsr::MAIL_SEND, high);
    let low = base | (mailbox * 2) << iocsr::MAIL_BOX_SHIFT | data << iocsr::MAIL_DATA_SHIFT;
    iocsr_write64(iocsr::MAIL_SEND, low);
}

extern "C" fn secondary_main(start: &CpuStart) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own, which map
    // everything this CPU uses at the addresses it uses now; interrupts are masked (`CRMD.IE`
    // is clear since the entry).
    unsafe { Loongarch64::activate_page_tables(&start.page_table_roots[..2]) };
    timer::init();
    interrupts::prepare_cpu();
    (start.entry)(start.argument)
}

fn device_tree() -> usize {
    let address = DEVICE_TREE.load(Ordering::Relaxed);
    if address as u64 > BOOT_DIRECT_MAP_END - boot_memory::MAX_DEVICE_TREE_SIZE as u64 {
        return 0;
    }
    address
}

#[inline]
fn uart() -> Ns16550<Mmio> {
    // SAFETY: on QEMU `virt` the UART's register block is at `UART_BASE`, which `DMW0` maps
    // uncached for the kernel from the first instruction on; only the console's transmit and
    // receive paths use it.
    let regs = unsafe {
        Mmio::new(
            UNCACHED_WINDOW + UART_BASE,
            Ns16550::<Mmio>::register_block_size(UART_REG_SHIFT),
        )
    };
    Ns16550::new(regs, UART_REG_SHIFT)
}

fn write_ged(register: usize, value: u8) {
    // SAFETY: the generic event device's registers on QEMU `virt`, through the uncached window;
    // writing them powers the machine off or resets it, which is what the callers want.
    unsafe { core::ptr::write_volatile((UNCACHED_WINDOW + register) as *mut u8, value) };
}

#[derive(Debug)]
pub struct Loongarch64;

impl Loongarch64 {
    /// The boot CPU's core number within its node, which EIOINTC routes by.
    pub(super) fn boot_cpu_index() -> u32 {
        (csr_read!(csr::CPUID) & 0x1ff) as u32
    }
}

impl Platform for Loongarch64 {
    const ARCH: &'static str = "loongarch64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        uart().write_bytes(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        uart().read_bytes(buffer)
    }

    fn enable_console_input() -> Result<(), &'static str> {
        interrupts::enable_console()?;
        uart().enable_receive_interrupt();
        Ok(())
    }

    fn route_device_interrupt(
        specifier: &crate::fdt::InterruptSpecifier,
    ) -> Result<crate::DeviceLine, &'static str> {
        interrupts::route_device(specifier.cells())
    }

    fn steer_device_interrupt(line: crate::DeviceLine, cpu: u64) -> Result<(), &'static str> {
        interrupts::steer_device(line, cpu)
    }

    fn message_interrupt()
    -> Result<(crate::DeviceLine, crate::interrupt::InterruptMessage), &'static str> {
        interrupts::message()
    }

    fn exit(status: ExitStatus) -> ! {
        let _ = status;
        write_ged(GED_SLEEP_CONTROL, GED_POWER_OFF);
        Self::halt()
    }

    fn restart() -> ! {
        write_ged(GED_RESET, GED_RESET_VALUE);
        Self::halt()
    }

    fn halt() -> ! {
        loop {
            // SAFETY: `idle` only waits for an interrupt; with interrupts masked it waits on.
            unsafe { core::arch::asm!("idle 0", options(nomem, nostack, preserves_flags)) };
        }
    }

    fn breakpoint() {
        // SAFETY: `break 0` traps to the entry installed at boot, which saves and restores every
        // general-purpose register and resumes after it. No `nomem`/`nostack`: the kernel's
        // handler may read and write any memory, and its frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("break 0") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: the all-zero word is no instruction; it traps to the entry installed at boot,
        // and the kernel never resumes from that exception.
        unsafe { core::arch::asm!(".word 0", options(noreturn)) }
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
        // SAFETY: `idle` wakes once an interrupt enabled in `ECFG` is pending, interrupts masked
        // or not; setting `CRMD.IE` then takes it through the entry installed at boot, which
        // restores every register, and clearing it masks interrupts again. No `nomem` or
        // `nostack`, as in `breakpoint`.
        unsafe {
            core::arch::asm!(
                "idle 0",
                "li.w {mask}, 4",
                "move {value}, {mask}",
                "csrxchg {value}, {mask}, 0x0",
                "csrxchg $zero, {mask}, 0x0",
                mask = out(reg) _,
                value = out(reg) _,
            )
        };
    }

    #[inline]
    fn interrupt_window() {
        // SAFETY: as in `wait_for_interrupt`, without the `idle`.
        unsafe {
            core::arch::asm!(
                "li.w {mask}, 4",
                "move {value}, {mask}",
                "csrxchg {value}, {mask}, 0x0",
                "csrxchg $zero, {mask}, 0x0",
                mask = out(reg) _,
                value = out(reg) _,
            )
        };
    }

    fn memory_map() -> MemoryMap {
        let mut map = match device_tree() {
            // SAFETY: the address came from QEMU's EFI system table, whose device tree stays
            // readable and unwritten in RAM inside the boot direct map (`device_tree` checks).
            address @ 1.. => unsafe { boot_memory::device_tree_memory_map(address) },
            0 => {
                let mut map = MemoryMap::new();
                map.add_ram(PhysicalRange {
                    start: 0,
                    end: DEFAULT_RAM_END,
                });
                map.reserve(boot_memory::kernel_image());
                map
            }
        };
        map.reserve(PhysicalRange {
            start: 0,
            end: KERNEL_LOAD_ADDRESS,
        });
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

    type PageTableFormat = paging::loongarch64::Loongarch64;

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    fn kernel_image() -> KernelImage {
        boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        let mut map = match device_tree() {
            // SAFETY: as in `memory_map`.
            address @ 1.. => unsafe { boot_memory::device_tree_ram(address) },
            0 => {
                let mut map = MemoryMap::new();
                map.add_ram(PhysicalRange {
                    start: 0,
                    end: DEFAULT_RAM_END,
                });
                map
            }
        };
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        region(PhysicalRange::from_start_length(0x1000_0000, 0x400));
        region(PhysicalRange::from_start_length(
            UART_BASE as u64,
            Ns16550::<Mmio>::register_block_size(UART_REG_SHIFT) as u64,
        ));
        region(PhysicalRange::from_start_length(
            GED_SLEEP_CONTROL as u64,
            3,
        ));
    }

    fn device_tree() -> Option<&'static [u8]> {
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_blob(device_tree()) }
    }

    fn command_line(line: &mut dyn FnMut(&[u8])) {
        // SAFETY: written once by the entry code before any Rust code ran, read-only after.
        let bytes = unsafe { &*(&raw const COMMAND_LINE) };
        let length = bytes
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(bytes.len());
        line(&bytes[..length]);
    }

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        // SAFETY: `dbar` orders the table writes before the walks that follow and `invtlb 0`
        // drops every cached translation; neither changes memory contents.
        unsafe { core::arch::asm!("dbar 0", "invtlb 0, $zero, $zero", options(nostack)) };
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [low, high] = roots else {
            panic!("LoongArch64 page tables have two roots");
        };
        csr_write!(csr::PGDL, *low);
        csr_write!(csr::PGDH, *high);
        csr_write!(csr::DMW1, 0);
        // SAFETY: the caller guarantees the tables map everything this CPU uses from here on at
        // the addresses it uses now; `dbar` orders their writes before the refills, and
        // `invtlb 0` drops what the boot tables left. No `nomem`: memory accesses must not move
        // across the switch.
        unsafe { core::arch::asm!("dbar 0", "invtlb 0, $zero, $zero", options(nostack)) };
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        let Some(low) = roots.first() else {
            panic!("LoongArch64 page tables have two roots");
        };
        // SAFETY: `dbar` orders this CPU's earlier stores before its later accesses and walks.
        unsafe { core::arch::asm!("dbar 0", options(nostack)) };
        csr_write!(csr::PGDL, *low);
        // SAFETY: `invtlb 3` drops every non-global translation, the old program's; the kernel's
        // are global and stay. No `nomem`, as in `activate_page_tables`.
        unsafe { core::arch::asm!("invtlb 3, $zero, $zero", "ibar 0", options(nostack)) };
    }

    fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
        access()
    }

    unsafe fn run_user(context: &mut crate::user::UserContext) -> crate::user::UserExit {
        // SAFETY: the caller's guarantees are `run_user`'s.
        unsafe { exception::run_user(context) }
    }

    fn user_test_program() -> crate::user::TestProgram {
        exception::user_test_program()
    }

    fn linux_hardware_capabilities() -> [u64; 2] {
        const CPUCFG: u64 = 1 << 0;
        const UNALIGNED_ACCESS: u64 = 1 << 2;
        const FPU: u64 = 1 << 3;
        [CPUCFG | UNALIGNED_ACCESS | FPU, 0]
    }

    fn make_instructions_visible(_start: usize, _length: usize) {
        // SAFETY: `ibar` orders this CPU's instruction fetches after its earlier stores; it
        // changes no memory contents.
        unsafe { core::arch::asm!("ibar 0", options(nostack)) };
    }

    fn page_fault(_kind: crate::ExceptionKind, code: u64) -> Option<paging::PageFault> {
        use crate::paging::{Access, PageFault};
        let access = match exception::ecode(code) {
            exception::ECODE_PAGE_INVALID_LOAD | exception::ECODE_PAGE_NOT_READABLE => Access::Read,
            exception::ECODE_PAGE_INVALID_STORE | exception::ECODE_PAGE_MODIFY => Access::Write,
            exception::ECODE_PAGE_INVALID_FETCH | exception::ECODE_PAGE_NOT_EXECUTABLE => {
                Access::Execute
            }
            exception::ECODE_PAGE_PRIVILEGE => Access::Read,
            _ => return None,
        };
        let may_be_stale = matches!(
            exception::ecode(code),
            exception::ECODE_PAGE_INVALID_LOAD
                | exception::ECODE_PAGE_INVALID_STORE
                | exception::ECODE_PAGE_INVALID_FETCH
                | exception::ECODE_PAGE_MODIFY
        );
        Some(PageFault {
            access,
            may_be_stale,
        })
    }

    fn drop_user_translations(start: u64, length: u64) {
        let pages = length / crate::memory::FRAME_SIZE;
        if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
            // SAFETY: `invtlb 3` drops every non-global translation; it changes no memory
            // contents.
            unsafe { core::arch::asm!("dbar 0", "invtlb 3, $zero, $zero", options(nostack)) };
            return;
        }
        // SAFETY: a barrier only, ordering the caller's table writes before the refills.
        unsafe { core::arch::asm!("dbar 0", options(nostack)) };
        for page in 0..pages {
            let address = start + page * crate::memory::FRAME_SIZE;
            // SAFETY: `invtlb 5` drops the non-global translation of one page in address-space
            // id 0, the only one used; it changes no memory contents.
            unsafe { core::arch::asm!("invtlb 5, $zero, {}", in(reg) address, options(nostack)) };
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
        // calling convention requires a call to preserve.
        unsafe { thread::eggos_switch_thread(save, resume) }
    }

    fn random_seeds(seed: &mut dyn FnMut(crate::random::SeedSource, &[u8])) {
        if device_tree() != 0 {
            // SAFETY: as in `memory_map`; called once at boot, on the boot CPU.
            unsafe { boot_memory::device_tree_take_seeds(device_tree(), seed) };
        }
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
        match device_tree() {
            0 => cpu(Self::boot_cpu()),
            // SAFETY: as in `memory_map`.
            address => unsafe { boot_memory::device_tree_cpus(address, cpu) },
        }
    }

    fn boot_cpu() -> u64 {
        csr_read!(csr::CPUID) & 0x1ff
    }

    unsafe fn start_cpu(cpu: u64, start: &CpuStart) -> Result<(), StartCpuError> {
        unsafe extern "C" {
            fn eggos_secondary_start();
        }
        if cpu > 0x1ff {
            return Err(StartCpuError::InvalidCpu);
        }
        let entry = (eggos_secondary_start as *const () as u64).wrapping_sub(KERNEL_VIRTUAL_OFFSET);
        core::sync::atomic::fence(Ordering::SeqCst);
        send_mail(cpu, 1, core::ptr::from_ref(start) as u64);
        send_mail(cpu, 0, entry);
        timer::wake(cpu);
        Ok(())
    }

    unsafe fn set_cpu_local(block: usize) {
        // SAFETY: `$r21` is reserved by the psABI, so compiled code never touches it; the entry
        // from user mode puts the kernel's value back (D290).
        unsafe { core::arch::asm!("move $r21, {}", in(reg) block, options(nomem, nostack)) };
    }

    #[inline]
    fn cpu_local() -> usize {
        let block: usize;
        // SAFETY: reading `$r21` has no side effects.
        unsafe { core::arch::asm!("move {}, $r21", out(reg) block, options(nomem, nostack)) };
        block
    }

    #[inline]
    unsafe fn cpu_local_u64<const OFFSET: usize>() -> u64 {
        let value: u64;
        // SAFETY: the caller guarantees `$r21` holds this CPU's block, with an aligned word at
        // `OFFSET`: one aligned 8-byte load, which never tears.
        unsafe {
            core::arch::asm!(
                "ld.d {}, $r21, {offset}",
                out(reg) value,
                offset = const OFFSET,
                options(nostack, readonly, preserves_flags),
            );
        }
        value
    }

    fn wake_cpu(cpu: u64) {
        timer::wake(cpu);
    }
}

/// Zeroes the `length` bytes at `start` ([`crate::memory::zero`]) in 64-byte lines.
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
    // SAFETY: a barrier: it touches no memory or register, only orders stores.
    unsafe { core::arch::asm!("dbar 0", options(nostack, preserves_flags)) };
}

#[allow(dead_code)]
fn mailbox0() -> u64 {
    let value: u64;
    // SAFETY: reading this core's mailbox has no side effects.
    unsafe {
        core::arch::asm!("iocsrrd.d {0}, {1}", out(reg) value, in(reg) iocsr::MAILBOX0, options(nostack))
    };
    value
}
