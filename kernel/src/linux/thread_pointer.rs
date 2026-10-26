//! The thread pointer (M6, D57): where a thread's thread-local storage is, which every C
//! library and Rust's std set before `main` runs. Each thread has its own, in its
//! [`UserContext`], and the HAL loads it on every way into user mode: `TPIDR_EL0` on AArch64
//! and `tp` on RISC-V, which programs set themselves, and the FS base on x86_64, which they
//! set with `arch_prctl(ARCH_SET_FS)`, here.
//!
//! `arch_prctl` answers as Linux does on a CPU without CPUID faulting (decided by research,
//! D57): the FS and GS bases can be set and read, and `ARCH_GET_CPUID` says `CPUID` works.
//! The GS base is where the Windows layer keeps a thread's environment block (D118), which the
//! SWAPGS fence on the HAL's exception entry makes safe to choose (D49). Every other code
//! (shadow stacks, AMX permissions, vDSO mapping, LAM) is `EINVAL`, which their callers take
//! as the feature being absent.

use super::Errno;
use crate::process::Process;
use eggos_hal::Platform;
use eggos_hal::paging::Format;
use eggos_hal::user::UserContext;

/// `arch_prctl`'s codes (x86_64).
pub mod code {
    pub const ARCH_SET_GS: u32 = 0x1001;
    pub const ARCH_SET_FS: u32 = 0x1002;
    pub const ARCH_GET_FS: u32 = 0x1003;
    pub const ARCH_GET_GS: u32 = 0x1004;
    pub const ARCH_GET_CPUID: u32 = 0x1011;
    pub const ARCH_SET_CPUID: u32 = 0x1012;
}

// The HAL's test program sets its FS base with this code.
const _: () = assert!(eggos_hal::user::TestProgram::LINUX_ARCH_SET_FS == code::ARCH_SET_FS as u64);

/// `arch_prctl(code, address)` (x86_64): sets or reads the calling thread's FS base, its
/// thread pointer, which takes effect when the program runs again. `code` is a C `int`, so
/// only its low 32 bits count.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub fn arch_prctl<P: Platform>(
    process: &Process<P>,
    context: &mut UserContext,
    code: u64,
    address: u64,
) -> Result<u64, Errno> {
    match code as u32 {
        // Any user address, mapped or not; nothing above, where the FS base would point into
        // the kernel's half or be non-canonical (the HAL refuses those too).
        code::ARCH_SET_FS if address < <P::PageTableFormat as Format>::USER_END => context
            .set_thread_pointer(address)
            .map(|()| 0)
            .map_err(|_| Errno::EPERM),
        code::ARCH_SET_FS => Err(Errno::EPERM),
        code::ARCH_GET_FS => write_word(process, address, context.thread_pointer()),
        #[cfg(target_arch = "x86_64")]
        code::ARCH_SET_GS if address < <P::PageTableFormat as Format>::USER_END => context
            .set_gs_base(address)
            .map(|()| 0)
            .map_err(|_| Errno::EPERM),
        #[cfg(target_arch = "x86_64")]
        code::ARCH_SET_GS => Err(Errno::EPERM),
        code::ARCH_GET_GS => write_word(process, address, gs_base(context)),
        code::ARCH_GET_CPUID => Ok(1),
        // CPUID faulting is not offered, whatever the CPU has.
        code::ARCH_SET_CPUID => Err(Errno::ENODEV),
        _ => Err(Errno::EINVAL),
    }
}

fn gs_base(context: &UserContext) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        context.gs_base()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = context;
        0
    }
}

/// Writes `value` to the program's memory at `address`, as an `unsigned long`.
fn write_word<P: Platform>(process: &Process<P>, address: u64, value: u64) -> Result<u64, Errno> {
    process
        .write_user_memory(address, &value.to_ne_bytes())
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}
