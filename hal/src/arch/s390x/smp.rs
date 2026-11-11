//! Secondary CPUs (D324). QEMU leaves every CPU but the boot one stopped. Starting one gives it
//! a lowcore of its own (its prefix area, 8 KiB): the boot CPU's new PSWs and continuation PSWs
//! copied in, a restart PSW that enters `s390x_secondary_real` in real mode, and the address of
//! its start block. Then `SIGP` resets the CPU, sets its prefix and restarts it. The real-mode
//! code loads the control registers from the block, takes the stack and enters the kernel in the
//! home space, where [`secondary_started`] sets the CPU's roots and timer and calls the entry.

use super::exception::{self, lowcore, psw};
use super::{DIRECT_MAP_OFFSET, KERNEL_VIRTUAL_OFFSET, MAX_CPUS, timer};
use crate::cpu::{CpuStart, StartCpuError};
use crate::paging::s390x::ASCE_REGION_SECOND;

const LOWCORE_BYTES: usize = 8192;
const SIGP_SENSE: u64 = 0x01;
const SIGP_RESTART: u64 = 0x06;
const SIGP_INITIAL_CPU_RESET: u64 = 0x0b;
const SIGP_SET_PREFIX: u64 = 0x0d;
const NOT_OPERATIONAL: u64 = 3;
const BUSY: u64 = 2;
/// CR0's AFP-register control: a new CPU starts with the extra FP registers trapping.
const CR0_AFP: u64 = 1 << 18;

#[repr(C, align(8192))]
struct Lowcore([u8; LOWCORE_BYTES]);

/// What the real-mode code reads, at offsets it knows: keep in step with `s390x_secondary_real`.
#[derive(Clone, Copy)]
#[repr(C, align(16))]
struct SecondaryStart {
    cr0: u64,
    root: u64,
    stack_top: u64,
    block: u64,
    entry_psw: [u64; 2],
    entry: u64,
    argument: u64,
    page_table_root: u64,
}

const EMPTY_START: SecondaryStart = SecondaryStart {
    cr0: 0,
    root: 0,
    stack_top: 0,
    block: 0,
    entry_psw: [0; 2],
    entry: 0,
    argument: 0,
    page_table_root: 0,
};

static mut LOWCORES: [Lowcore; MAX_CPUS] = [const { Lowcore([0; LOWCORE_BYTES]) }; MAX_CPUS];
static mut STARTS: [SecondaryStart; MAX_CPUS] = [EMPTY_START; MAX_CPUS];

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .global s390x_secondary_real
s390x_secondary_real:
    lg          %r1, {cpu_start}
    lctlg       %c0, %c0, 0(%r1)
    lctlg       %c1, %c1, 8(%r1)
    lctlg       %c13, %c13, 8(%r1)
    lg          %r15, 16(%r1)
    lg          %r2, 24(%r1)
    lpswe       32(%r1)

    .text
    .global s390x_secondary_entry
s390x_secondary_entry:
    aghi        %r15, -160
    lghi        %r0, 0
    stg         %r0, 0(%r15)
    brasl       %r14, {started}
2:  j           2b
"#,
    cpu_start = const lowcore::CPU_START,
    started = sym secondary_started,
);

unsafe extern "C" {
    fn s390x_secondary_real();
    fn s390x_secondary_entry();
}

fn sigp(cpu: u64, order: u64, parameter: u64) -> u64 {
    let code: u64;
    // SAFETY: `sigp` signals CPU `cpu`; the status it may store goes to `r2`, which is
    // clobbered, and the orders used here change only that CPU's state.
    unsafe {
        core::arch::asm!(
            "sigp %r2, {cpu}, 0({order})",
            "ipm {code}",
            inout("r2") 0u64 => _,
            in("r3") parameter,
            cpu = in(reg) cpu,
            order = in(reg_addr) order,
            code = out(reg) code,
            options(nostack),
        )
    };
    (code >> 28) & 3
}

/// Calls `cpu` with every CPU address that answers `SIGP SENSE`, from 0 up to [`MAX_CPUS`].
pub(super) fn cpus(cpu: &mut dyn FnMut(u64)) {
    for address in 0..MAX_CPUS as u64 {
        if address == super::cpu_address() || sigp(address, SIGP_SENSE, 0) != NOT_OPERATIONAL {
            cpu(address);
        }
    }
}

fn store_control_0() -> u64 {
    let mut value = 0u64;
    // SAFETY: `stctg` stores CR0 into `value`, which this function owns.
    unsafe { core::arch::asm!("stctg %c0, %c0, 0({0})", in(reg_addr) &raw mut value, options(nostack)) };
    value
}

/// [`crate::Platform::start_cpu`] on s390x.
///
/// # Safety
/// As for [`crate::Platform::start_cpu`].
pub(super) unsafe fn start_cpu(cpu: u64, start: &CpuStart) -> Result<(), StartCpuError> {
    let index = cpu as usize;
    if index >= MAX_CPUS || cpu == super::cpu_address() {
        return Err(StartCpuError::InvalidCpu);
    }
    let block = SecondaryStart {
        cr0: store_control_0() & !CR0_AFP,
        root: start.page_table_roots[0] | ASCE_REGION_SECOND,
        stack_top: start.stack_top as u64,
        block: 0,
        entry_psw: [psw::KERNEL, s390x_secondary_entry as *const () as u64],
        entry: start.entry as usize as u64,
        argument: start.argument as u64,
        page_table_root: start.page_table_roots[0],
    };
    // SAFETY: CPU `index` is stopped (QEMU leaves it so, and calls do not overlap), so nothing
    // reads its block and lowcore while they are written.
    let (block_physical, lowcore_physical) = unsafe {
        let starts = &raw mut STARTS;
        let slot = &raw mut (*starts)[index];
        slot.write(SecondaryStart {
            block: slot as u64 - KERNEL_VIRTUAL_OFFSET,
            ..block
        });
        let lowcores = &raw mut LOWCORES;
        let lowcore = &raw mut (*lowcores)[index];
        (slot as u64 - KERNEL_VIRTUAL_OFFSET, lowcore as u64 - KERNEL_VIRTUAL_OFFSET)
    };
    let new = (DIRECT_MAP_OFFSET + lowcore_physical) as *mut u8;
    let own = DIRECT_MAP_OFFSET as *const u8;
    // SAFETY: the new CPU's 8 KiB lowcore, reached at its absolute address through the direct
    // map (prefixing swaps only this CPU's own first 8 KiB, which `own` reads); the CPU is
    // stopped, so nothing else reaches it.
    unsafe {
        core::ptr::write_bytes(new, 0, LOWCORE_BYTES);
        let copied = lowcore::COPIED;
        core::ptr::copy_nonoverlapping(
            own.add(copied.start as usize),
            new.add(copied.start as usize),
            (copied.end - copied.start) as usize,
        );
        let words = new as *mut u64;
        words.add(lowcore::RESTART_NEW as usize / 8).write_volatile(psw::REAL);
        words
            .add(lowcore::RESTART_NEW as usize / 8 + 1)
            .write_volatile(s390x_secondary_real as *const () as u64 - KERNEL_VIRTUAL_OFFSET);
        words.add(lowcore::CPU_START as usize / 8).write_volatile(block_physical);
    }
    super::doorbell_barrier();
    for (order, parameter) in [
        (SIGP_INITIAL_CPU_RESET, 0),
        (SIGP_SET_PREFIX, lowcore_physical),
        (SIGP_RESTART, 0),
    ] {
        match sigp(cpu, order, parameter) {
            0 => {}
            BUSY => return Err(StartCpuError::AlreadyRunning),
            NOT_OPERATIONAL => return Err(StartCpuError::InvalidCpu),
            code => return Err(StartCpuError::Refused(code as i64)),
        }
    }
    Ok(())
}

extern "C" fn secondary_started(block_physical: u64) -> ! {
    // SAFETY: the start block `start_cpu` wrote for this CPU, through the direct map; nothing
    // writes it until this CPU called its entry.
    let block = unsafe { ((DIRECT_MAP_OFFSET + block_physical) as *const SecondaryStart).read() };
    exception::set_home_root(block.page_table_root);
    exception::sync_program_root(block.page_table_root);
    exception::purge_translations();
    timer::init();
    // SAFETY: `entry` is the `extern "C" fn(usize) -> !` the kernel gave `start_cpu`.
    let entry: extern "C" fn(usize) -> ! = unsafe { core::mem::transmute(block.entry as usize) };
    entry(block.argument as usize)
}
