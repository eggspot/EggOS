//! s390x on QEMU `s390-ccw-virtio` (z/Architecture, big-endian; D321).
//!
//! Boot contract: QEMU loads the ELF image at its physical addresses and starts `_start` in real
//! mode (DAT off) with 64-bit addressing. `_start` builds boot tables that map the first 2 GiB
//! at the direct map (`1 << 52`) and at the image's virtual address (the top 2 GiB of the
//! 53-bit space) with 1 MiB segments, fills the lowcore's new PSWs with the interruption stubs
//! (`exception.rs`), and enters the kernel in the home space. The SCLP is the console and says
//! how much memory there is; devices (virtio-ccw on the channel subsystem) come with part 3.
//! The per-CPU block's address is in the lowcore, which every CPU's prefix makes its own.

mod exception;
mod fp;
mod sclp;
mod thread;
mod timer;

use crate::cpu::{CpuStart, StartCpuError};
use crate::memory::PhysicalRange;
use crate::paging::{self, KernelImage};
use crate::{ExitStatus, MemoryMap, Platform};
use exception::{lowcore, psw};

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .global _start
_start:
    larl        %r2, __bss_start
    larl        %r3, __bss_end
1:  clgr        %r2, %r3
    jhe         2f
    mvghi       0(%r2), 0
    aghi        %r2, 8
    j           1b
2:  larl        %r2, {boot_root}
    lghi        %r0, {empty_region_second}
    lghi        %r1, 2048
3:  stg         %r0, 0(%r2)
    aghi        %r2, 8
    brctg       %r1, 3b
    lghi        %r0, {empty_region_third}
    larl        %r2, {boot_direct}
    lghi        %r1, 4096
4:  stg         %r0, 0(%r2)
    aghi        %r2, 8
    brctg       %r1, 4b
    larl        %r2, {boot_segments}
    lghi        %r0, {large}
    lghi        %r1, 2048
5:  stg         %r0, 0(%r2)
    agfi        %r0, 0x100000
    aghi        %r2, 8
    brctg       %r1, 5b
    larl        %r2, {boot_segments}
    oill        %r2, {segment_designation}
    larl        %r3, {boot_direct}
    stg         %r2, 0(%r3)
    stg         %r2, {kernel_third_slot}(%r3)
    larl        %r4, {boot_root}
    larl        %r3, {boot_direct}
    oill        %r3, {third_designation}
    stg         %r3, {direct_slot}(%r4)
    larl        %r3, {boot_direct}
    aghi        %r3, 16384
    oill        %r3, {third_designation}
    stg         %r3, {kernel_slot}(%r4)
    oill        %r4, {asce_type}
    stg         %r4, 0x340
    lctlg       %c1, %c1, 0x340
    lctlg       %c13, %c13, 0x340
    stctg       %c0, %c0, 0x340
    larl        %r1, {cr0_bits}
    oc          0x340(8), 0(%r1)
    lctlg       %c0, %c0, 0x340
    llilh       %r0, 0x8000
    oihl        %r0, 0x0001
    llihh       %r5, 0x0400
    oihl        %r5, 0xc001
    oilh        %r5, 0x8000
    llihh       %r6, {offset_high}
    oihl        %r6, {offset_high_low}
    oilh        %r6, {offset_low_high}
    .irp        new, {external_new}, {svc_new}, {program_new}, {machine_check_new}, {io_new}, {to_user_psw}, {kernel_return_psw}
    stg         %r0, \new
    .endr
    larl        %r1, s390x_external_stub
    stg         %r1, {external_new} + 8
    larl        %r1, s390x_svc_stub
    stg         %r1, {svc_new} + 8
    larl        %r1, s390x_program_stub
    stg         %r1, {program_new} + 8
    larl        %r1, s390x_machine_check_stub
    stg         %r1, {machine_check_new} + 8
    larl        %r1, s390x_io_stub
    stg         %r1, {io_new} + 8
    larl        %r1, s390x_to_user_real
    stg         %r1, {to_user_psw} + 8
    larl        %r1, s390x_kernel_return_real
    stg         %r1, {kernel_return_psw} + 8
    stg         %r5, {kernel_entry_psw}
    larl        %r1, s390x_kernel_entry
    agr         %r1, %r6
    stg         %r1, {kernel_entry_psw} + 8
    stg         %r5, {user_exit_psw}
    larl        %r1, s390x_user_exit
    agr         %r1, %r6
    stg         %r1, {user_exit_psw} + 8
    stg         %r5, 0x340
    larl        %r1, 6f
    agr         %r1, %r6
    stg         %r1, 0x348
    lpswe       0x340
6:  larl        %r15, __stack_top
    aghi        %r15, -160
    brasl       %r14, {init_timer}
    brasl       %r14, kernel_main
7:  j           7b
"#,
    boot_root = sym BOOT_ROOT,
    boot_direct = sym BOOT_THIRDS,
    boot_segments = sym BOOT_SEGMENTS,
    empty_region_second = const 0x28,
    empty_region_third = const 0x24,
    large = const 0x400,
    segment_designation = const 0x7,
    third_designation = const 0xb,
    asce_type = const paging::s390x::ASCE_REGION_SECOND,
    direct_slot = const ((DIRECT_MAP_OFFSET >> 42) & 0x7ff) * 8,
    kernel_slot = const ((KERNEL_VIRTUAL_OFFSET >> 42) & 0x7ff) * 8,
    kernel_third_slot = const ((KERNEL_VIRTUAL_OFFSET >> 31) & 0x7ff) * 8,
    cr0_bits = sym CR0_BITS,
    offset_high = const (KERNEL_VIRTUAL_OFFSET >> 48) & 0xffff,
    offset_high_low = const (KERNEL_VIRTUAL_OFFSET >> 32) & 0xffff,
    offset_low_high = const (KERNEL_VIRTUAL_OFFSET >> 16) & 0xffff,
    external_new = const lowcore::EXTERNAL_NEW,
    svc_new = const lowcore::SVC_NEW,
    program_new = const lowcore::PROGRAM_NEW,
    machine_check_new = const lowcore::MACHINE_CHECK_NEW,
    io_new = const lowcore::IO_NEW,
    to_user_psw = const lowcore::TO_USER_PSW,
    kernel_return_psw = const lowcore::KERNEL_RETURN_PSW,
    kernel_entry_psw = const lowcore::KERNEL_ENTRY_PSW,
    user_exit_psw = const lowcore::USER_EXIT_PSW,
    init_timer = sym timer::init,
);

/// The image's virtual address minus its physical address: the top 2 GiB of the 53-bit space
/// (D33). `linker.ld` uses the same value and checks it against this one.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = 0x001f_ffff_8000_0000;
/// The direct map: the kernel half's start (D29).
const DIRECT_MAP_OFFSET: u64 = 1 << 52;
/// The physical memory the direct map covers.
const DIRECT_MAP_END: u64 = 1 << 52;
/// The kernel's load address: below it are the lowcore and QEMU's boot information.
const KERNEL_LOAD_ADDRESS: u64 = 0x10_0000;
/// The most CPUs the HAL keeps program roots for.
pub(super) const MAX_CPUS: usize = 16;
/// RAM without an SCLP answer: 256 MiB.
const DEFAULT_RAM: u64 = 256 << 20;
/// Where QEMU leaves the command line of an image it loads.
const COMMAND_LINE: u64 = 0x1_0480;
const COMMAND_LINE_BYTES: usize = 896;
/// CR0: EDAT-1, the clock comparator's, emergency signals' and the service signal's external
/// interruption subclasses.
static CR0_BITS: u64 = 1 << 23 | 1 << 14 | 1 << 11 | 1 << 9;

#[repr(C, align(16384))]
struct RegionTable([u64; 2048]);

#[repr(C, align(16384))]
struct ThirdTables([u64; 4096]);

/// Built by `_start` in real mode.
static mut BOOT_ROOT: RegionTable = RegionTable([0; 2048]);
static mut BOOT_THIRDS: ThirdTables = ThirdTables([0; 4096]);
static mut BOOT_SEGMENTS: RegionTable = RegionTable([0; 2048]);

const SIGP_STOP: u64 = 0x05;
const SIGP_STORE_ADDRESS: u64 = 0;

/// This CPU's address.
pub(super) fn cpu_address() -> u64 {
    let mut address = 0u16;
    // SAFETY: `stap` stores this CPU's address into `address`, which this function owns.
    unsafe { core::arch::asm!("stap 0({0})", in(reg) &raw mut address, options(nostack)) };
    u64::from(address)
}

/// This CPU's index into the HAL's per-CPU tables.
pub(super) fn cpu_index() -> usize {
    cpu_address() as usize % MAX_CPUS
}

fn lowcore_word(offset: u64) -> *mut u64 {
    (DIRECT_MAP_OFFSET + offset) as *mut u64
}

fn stop_this_cpu() -> ! {
    loop {
        // SAFETY: `sigp stop` to this CPU's own address stops it; QEMU ends the machine once
        // every CPU is stopped.
        unsafe {
            core::arch::asm!(
                "sigp {status}, {cpu}, {order}",
                status = out(reg) _,
                cpu = in(reg) cpu_address(),
                order = const SIGP_STOP,
                options(nostack, nomem),
            )
        };
    }
}

#[derive(Debug)]
pub struct S390x;

impl Platform for S390x {
    const ARCH: &'static str = "s390x";

    #[inline]
    fn console_write(bytes: &[u8]) {
        sclp::write(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        sclp::read(buffer)
    }

    fn enable_console_input() -> Result<(), &'static str> {
        Err("the SCLP console is polled (D321)")
    }

    fn route_device_interrupt(
        _specifier: &crate::fdt::InterruptSpecifier,
    ) -> Result<crate::DeviceLine, &'static str> {
        Err("s390x devices interrupt through the channel subsystem (D321 part 3)")
    }

    fn message_interrupt() -> Result<(crate::DeviceLine, crate::interrupt::InterruptMessage), &'static str> {
        Err("s390x has no message interrupts (D321)")
    }

    fn exit(status: ExitStatus) -> ! {
        let _ = status;
        stop_this_cpu()
    }

    fn restart() -> ! {
        stop_this_cpu()
    }

    fn halt() -> ! {
        stop_this_cpu()
    }

    fn breakpoint() {
        deliver_breakpoint();
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: the zero halfword is no instruction; it raises an operation exception, and
        // the kernel never resumes from it.
        unsafe { core::arch::asm!(".word 0", options(noreturn)) }
    }

    #[inline]
    fn clock_ticks() -> u64 {
        timer::now()
    }

    #[inline]
    fn clock_frequency() -> u64 {
        timer::FREQUENCY
    }

    fn set_timer_deadline(deadline: u64) {
        timer::set_deadline(deadline);
    }

    fn wait_for_interrupt() {
        let mut wait = [psw::KERNEL | psw::EXTERNAL | psw::IO | psw::WAIT, 0u64];
        // SAFETY: the PSW waits with external and I/O interruptions on and continues at the
        // label; the kernel entry clears the wait bit and the masks in the old PSW, so the
        // interrupted code goes on with interruptions off. No `nomem`: the handler may write
        // any memory.
        unsafe {
            core::arch::asm!(
                "larl {address}, 1f",
                "stg {address}, 8({psw})",
                "lpswe 0({psw})",
                "1:",
                psw = in(reg) wait.as_mut_ptr(),
                address = out(reg) _,
            )
        };
    }

    #[inline]
    fn interrupt_window() {
        let mut mask = 0u8;
        // SAFETY: `stosm` turns external and I/O interruptions on and `stnsm` off again; one
        // pending is taken in between through the kernel entry, which restores every register.
        unsafe {
            core::arch::asm!(
                "stosm 0({0}), 0x03",
                "stnsm 0({0}), 0xfc",
                in(reg) &raw mut mask,
            )
        };
    }

    fn memory_map() -> MemoryMap {
        let mut map = Self::physical_memory();
        map.reserve(PhysicalRange {
            start: 0,
            end: KERNEL_LOAD_ADDRESS,
        });
        map.reserve(crate::arch::boot_memory::kernel_image());
        map
    }

    #[inline]
    fn physical_memory_offset() -> usize {
        DIRECT_MAP_OFFSET as usize
    }

    type PageTableFormat = paging::s390x::S390x;

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET as usize
    }

    fn kernel_image() -> KernelImage {
        crate::arch::boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        let mut map = MemoryMap::new();
        let end = sclp::memory_bytes().unwrap_or(DEFAULT_RAM).min(DIRECT_MAP_END);
        map.add_ram(PhysicalRange { start: 0, end });
        map
    }

    fn early_device_regions(_region: &mut dyn FnMut(PhysicalRange)) {}

    fn device_tree() -> Option<&'static [u8]> {
        None
    }

    fn command_line(line: &mut dyn FnMut(&[u8])) {
        // SAFETY: the bytes QEMU left at `COMMAND_LINE`, below the image, which nothing writes
        // (the memory map reserves them), through the direct map.
        let bytes = unsafe {
            core::slice::from_raw_parts((DIRECT_MAP_OFFSET + COMMAND_LINE) as *const u8, COMMAND_LINE_BYTES)
        };
        let length = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
        line(&bytes[..length]);
    }

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        exception::purge_translations();
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [root] = roots else {
            panic!("s390x page tables have one root");
        };
        exception::set_home_root(*root);
        exception::sync_program_root(*root);
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        let [root] = roots else {
            panic!("s390x page tables have one root");
        };
        exception::set_home_root(*root);
        exception::sync_program_root(*root);
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
        [0, 0]
    }

    fn make_instructions_visible(_start: usize, _length: usize) {
        // SAFETY: a serialization: later fetches see earlier stores.
        unsafe { core::arch::asm!("bcr 14, 0", options(nostack, nomem)) };
    }

    fn page_fault(kind: crate::ExceptionKind, code: u64) -> Option<paging::PageFault> {
        exception::page_fault(kind, code)
    }

    fn drop_user_translations(_start: u64, _length: u64) {
        exception::purge_translations();
    }

    unsafe fn prepare_thread(
        stack_top: usize,
        entry: extern "C" fn(usize) -> !,
        argument: usize,
    ) -> usize {
        // SAFETY: the frame fits in `INITIAL_FRAME_MAX` bytes, which the caller guarantees are
        // writable and unused below `stack_top`.
        unsafe {
            crate::thread::write_initial_frame(stack_top, &thread::initial_frame(entry, argument))
        }
    }

    #[inline]
    unsafe fn switch_thread(save: *mut usize, resume: usize) {
        // SAFETY: the caller guarantees `save` is writable and `resume` a saved context whose
        // stack is allocated and not running.
        unsafe { thread::eggos_switch_thread(save, resume) }
    }

    fn random_seeds(_seed: &mut dyn FnMut(crate::random::SeedSource, &[u8])) {}

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
        cpu(Self::boot_cpu());
    }

    fn boot_cpu() -> u64 {
        cpu_address()
    }

    unsafe fn start_cpu(_cpu: u64, _start: &CpuStart) -> Result<(), StartCpuError> {
        let _ = SIGP_STORE_ADDRESS;
        Err(StartCpuError::InvalidCpu)
    }

    unsafe fn set_cpu_local(block: usize) {
        // SAFETY: this CPU's lowcore word for the block, which only the HAL uses; the prefix
        // makes the lowcore each CPU's own.
        unsafe { lowcore_word(lowcore::CPU_LOCAL).write_volatile(block as u64) };
    }

    #[inline]
    fn cpu_local() -> usize {
        // SAFETY: as in `set_cpu_local`, a read.
        unsafe { lowcore_word(lowcore::CPU_LOCAL).read_volatile() as usize }
    }

    #[inline]
    unsafe fn cpu_local_u64<const OFFSET: usize>() -> u64 {
        let block = Self::cpu_local();
        // SAFETY: the caller guarantees this CPU's block holds an aligned word at `OFFSET`.
        unsafe { ((block + OFFSET) as *const u64).read_volatile() }
    }

    fn wake_cpu(cpu: u64) {
        timer::wake(cpu);
    }
}

fn deliver_breakpoint() {
    crate::exception::deliver(&crate::Exception {
        kind: crate::ExceptionKind::Breakpoint,
        name: "breakpoint",
        code: 0,
        pc: 0,
        address: None,
    });
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
    // SAFETY: a serialization: it touches no memory, only orders this CPU's stores.
    unsafe { core::arch::asm!("bcr 14, 0", options(nostack, preserves_flags)) };
}
