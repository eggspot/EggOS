//! Starting a process the way `posix_spawn` does: `clone` with the child on a stack of its
//! own ([`spawn`]), and the return path a signal handler needs on x86_64.

use crate::errno::Errno;
use crate::syscall::{self, number};
use core::arch::asm;
use core::ffi::CStr;
use core::sync::atomic::{AtomicIsize, Ordering};

/// The `clone` flag that shares the caller's memory with the child.
pub const CLONE_VM: usize = 0x100;

/// The `clone` flag that has the caller sleep until the child runs `execve` or ends.
pub const CLONE_VFORK: usize = 0x4000;

/// `clone(flags, stack_top)`: the child starts on `stack_top` and calls `child(argument)`,
/// which never returns; the caller gets the child's id, or a negative errno.
///
/// # Safety
///
/// `stack_top` is the 16-byte aligned end of memory the child may use as its stack for as long
/// as it runs, which nothing else uses meanwhile; with [`CLONE_VM`] the child shares the
/// caller's memory, so `child` must touch only what the caller lets it.
#[inline(always)]
pub unsafe fn clone_onto_stack(
    flags: usize,
    stack_top: usize,
    child: extern "C" fn(usize) -> !,
    argument: usize,
) -> isize {
    let result: isize;
    #[cfg(target_arch = "x86_64")]
    // SAFETY: Linux's x86_64 `clone`: the caller's own registers and memory are as before but
    // `rax`, `rcx` and `r11`; the child starts on its own stack and never comes back here.
    unsafe {
        asm!(
            "syscall",
            "test rax, rax",
            "jnz 2f",
            "mov rdi, r13",
            "call r12",
            "ud2",
            "2:",
            inlateout("rax") number::CLONE as isize => result,
            in("rdi") flags, in("rsi") stack_top, in("rdx") 0usize, in("r10") 0usize,
            in("r8") 0usize, in("r12") child, in("r13") argument,
            lateout("rcx") _, lateout("r11") _,
        );
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: Linux's AArch64 `clone`: the caller's registers are as before but `x0`; the
    // child starts on its own stack and never comes back here.
    unsafe {
        asm!(
            "svc 0",
            "cbnz x0, 2f",
            "mov x0, x10",
            "blr x9",
            "brk #0",
            "2:",
            in("x8") number::CLONE,
            inlateout("x0") flags as isize => result,
            in("x1") stack_top, in("x2") 0usize, in("x3") 0usize, in("x4") 0usize,
            in("x9") child, in("x10") argument,
        );
    }
    #[cfg(target_arch = "riscv64")]
    // SAFETY: Linux's RISC-V `clone`: the caller's registers are as before but `a0`; the child
    // starts on its own stack and never comes back here.
    unsafe {
        asm!(
            "ecall",
            "bnez a0, 2f",
            "mv a0, t2",
            "jalr t1",
            "unimp",
            "2:",
            in("a7") number::CLONE,
            inlateout("a0") flags as isize => result,
            in("a1") stack_top, in("a2") 0usize, in("a3") 0usize, in("a4") 0usize,
            in("t1") child, in("t2") argument,
        );
    }
    result
}

/// Where a signal handler returns on x86_64, given as `sa_restorer` with `SA_RESTORER`: it
/// calls `rt_sigreturn`. AArch64 and RISC-V handlers return through the kernel's own
/// trampoline.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
pub extern "C" fn restore_after_signal() -> ! {
    core::arch::naked_asm!(
        "mov eax, {number}",
        "syscall",
        "ud2",
        number = const number::RT_SIGRETURN,
    )
}

/// The bytes of a [`ChildStack`].
pub const CHILD_STACK_BYTES: usize = 16 * 1024;

const SIGCHLD: usize = 17;
const NOT_RUN: i32 = 127;

/// The stack a child started by [`spawn`] runs on until its `execve`. [`spawn`] borrows it
/// mutably, so one child at a time uses it.
#[repr(C, align(16))]
pub struct ChildStack {
    bytes: [u8; CHILD_STACK_BYTES],
}

impl ChildStack {
    /// An unused stack.
    pub const fn new() -> Self {
        Self {
            bytes: [0; CHILD_STACK_BYTES],
        }
    }
}

impl Default for ChildStack {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for ChildStack {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChildStack").finish_non_exhaustive()
    }
}

/// What [`spawn`] starts: the program, its arguments and environment (each a null-ended
/// array of C strings), the descriptors that become its standard input, output and error, and
/// optionally the folder it starts in and a process group of its own.
#[derive(Debug)]
pub struct Spawn<'a> {
    program: &'a CStr,
    arguments: &'a [*const u8],
    environment: &'a [*const u8],
    streams: [usize; 3],
    directory: Option<&'a CStr>,
    own_group: bool,
    exec_error: AtomicIsize,
}

impl<'a> Spawn<'a> {
    /// A request to run `program` with `arguments` and `environment`, both ending with a null
    /// pointer, and `streams` as descriptors 0, 1 and 2.
    pub fn new(
        program: &'a CStr,
        arguments: &'a [*const u8],
        environment: &'a [*const u8],
        streams: [usize; 3],
    ) -> Self {
        Self {
            program,
            arguments,
            environment,
            streams,
            directory: None,
            own_group: false,
            exec_error: AtomicIsize::new(0),
        }
    }

    /// The same request, with the child starting in `directory`; when it cannot, the child
    /// ends and its error comes back as `execve`'s would.
    pub fn in_directory(self, directory: &'a CStr) -> Self {
        Self {
            directory: Some(directory),
            ..self
        }
    }

    /// The same request, with the child leading a process group of its own, so the whole
    /// group (its own children too) can be signalled at once with `kill(-child, ..)`.
    pub fn in_own_process_group(self) -> Self {
        Self {
            own_group: true,
            ..self
        }
    }
}

/// Starts `request`'s program as a child on `stack`, sharing this process's memory until the
/// child's `execve` (`CLONE_VM | CLONE_VFORK`), so nothing is copied; returns its process id.
/// `EINVAL` for an array without its null end; when `execve` fails, the child ends, is reaped,
/// and its error comes back.
pub fn spawn(stack: &mut ChildStack, request: &Spawn<'_>) -> Result<usize, Errno> {
    let ended = |array: &[*const u8]| array.last().is_some_and(|last| last.is_null());
    if !ended(request.arguments) || !ended(request.environment) {
        return Err(Errno::EINVAL);
    }
    request.exec_error.store(0, Ordering::Relaxed);
    let top = stack.bytes.as_mut_ptr() as usize + CHILD_STACK_BYTES;
    // SAFETY: `top` is the 16-byte aligned end of the stack's bytes, borrowed mutably for this
    // call, which only the child uses while this thread sleeps in the `CLONE_VFORK` clone; the
    // child touches only `request`, which outlives the clone, then runs `execve` or ends.
    let started = unsafe {
        clone_onto_stack(
            CLONE_VM | CLONE_VFORK | SIGCHLD,
            top,
            start_child,
            request as *const Spawn<'_> as usize,
        )
    };
    let child = Errno::result(started)?;
    match request.exec_error.load(Ordering::Acquire) {
        0 => Ok(child),
        error => {
            let mut status = 0i32;
            // SAFETY: `wait4` writes the status into `status`, which this function owns.
            unsafe { syscall::call(number::WAIT4, [child, &raw mut status as usize, 0, 0, 0, 0]) };
            Err(Errno(error.unsigned_abs().min(usize::from(u16::MAX)) as u16))
        }
    }
}

extern "C" fn start_child(request: usize) -> ! {
    // SAFETY: `spawn` passes its `Spawn`, which stays in place and unchanged but for its
    // atomic error word while this child runs on the parent's memory: the parent's thread
    // sleeps until the child runs `execve` or ends.
    let request = unsafe { &*(request as *const Spawn<'_>) };
    if request.own_group {
        // SAFETY: `setpgid` touches no memory.
        unsafe { syscall::call(number::SETPGID, [0, 0, 0, 0, 0, 0]) };
    }
    if let Some(directory) = request.directory {
        // SAFETY: `chdir` reads the C string `request` names, which stays in place while the
        // parent sleeps.
        let changed = unsafe { syscall::call(number::CHDIR, [directory.as_ptr() as usize, 0, 0, 0, 0, 0]) };
        if changed < 0 {
            request.exec_error.store(changed, Ordering::Release);
            syscall::exit(NOT_RUN)
        }
    }
    for (target, &from) in request.streams.iter().enumerate() {
        if from != target {
            // SAFETY: `dup3` touches no memory.
            unsafe { syscall::call(number::DUP3, [from, target, 0, 0, 0, 0]) };
        }
    }
    // SAFETY: `execve` reads the C string and the null-ended pointer arrays `request` names,
    // which stay in place while the parent sleeps.
    let result = unsafe {
        syscall::call(
            number::EXECVE,
            [
                request.program.as_ptr() as usize,
                request.arguments.as_ptr() as usize,
                request.environment.as_ptr() as usize,
                0,
                0,
                0,
            ],
        )
    };
    request.exec_error.store(result.min(-1), Ordering::Release);
    syscall::exit(NOT_RUN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_need_their_arrays_null_ended() {
        let mut stack = Box::new(ChildStack::new());
        let program = c"/bin/true";
        let arguments = [program.as_ptr().cast::<u8>()];
        let environment = [core::ptr::null()];
        let request = Spawn::new(program, &arguments, &environment, [0, 1, 2]);
        assert_eq!(spawn(&mut stack, &request), Err(Errno::EINVAL));
        let ended = [program.as_ptr().cast::<u8>(), core::ptr::null()];
        let unended_environment = [c"A=b".as_ptr().cast::<u8>()];
        let request = Spawn::new(program, &ended, &unended_environment, [0, 1, 2]);
        assert_eq!(spawn(&mut stack, &request), Err(Errno::EINVAL));
        assert_eq!(format!("{stack:?}"), "ChildStack { .. }");
    }
}
