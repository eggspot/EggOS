//! `ptrace` for debuggers and tracers (D561): a process traced by another stops before each
//! signal is delivered to it (`SIGKILL` aside) and at `execve` (`SIGTRAP`), and its tracer learns
//! of the stop through `waitpid` as of a stopped child; the tracer reads and writes its memory
//! (`PEEK*`, `POKE*`) and registers (`GETREGS`/`SETREGS` on x86_64, `GETREGSET`/`SETREGSET` with
//! `NT_PRSTATUS` everywhere the ABI lays them out), then resumes it with `CONT` (the signal to
//! deliver, 0 for none), `SINGLESTEP` (x86_64's trap flag; `EIO` elsewhere), `DETACH` or `KILL`.
//! Tracing is per process: every thread of a traced process stops at its own signals.
//! `PTRACE_SYSCALL` stops are not made yet (it resumes as `CONT`).

use super::Errno;
use super::futex::{self, Key};
use super::process_table::TABLE;
use super::signal::{SIGKILL, SIGSTOP, SIGTRAP};
use super::signals::Info;
use super::thread::LinuxThread;
use crate::process::Process;
use crate::sync::SpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::user::{UserContext, UserRegisters};

pub mod request {
    pub const TRACEME: u64 = 0;
    pub const PEEKTEXT: u64 = 1;
    pub const PEEKDATA: u64 = 2;
    pub const POKETEXT: u64 = 4;
    pub const POKEDATA: u64 = 5;
    pub const CONT: u64 = 7;
    pub const KILL: u64 = 8;
    pub const SINGLESTEP: u64 = 9;
    pub const GETREGS: u64 = 12;
    pub const SETREGS: u64 = 13;
    pub const ATTACH: u64 = 16;
    pub const DETACH: u64 = 17;
    pub const SYSCALL: u64 = 24;
    pub const SETOPTIONS: u64 = 0x4200;
    pub const GETEVENTMSG: u64 = 0x4201;
    pub const GETSIGINFO: u64 = 0x4202;
    pub const GETREGSET: u64 = 0x4204;
    pub const SETREGSET: u64 = 0x4205;
    pub const SEIZE: u64 = 0x4206;
    pub const INTERRUPT: u64 = 0x4207;
}

const NT_PRSTATUS: u64 = 1;
const WORD: usize = super::abi::WORD_BYTES;

/// A traced process's state.
#[derive(Debug, Clone)]
struct Traced {
    tracee: u32,
    tracer: u32,
    /// The signal it stopped with, while it waits for its tracer; reported once.
    stopped: Option<u8>,
    reported: bool,
    /// What its tracer resumed it with: the signal to deliver (0 for none) and whether to step.
    resumed: Option<(u8, bool)>,
    /// Its registers while it is stopped, which its tracer reads and changes.
    registers: Option<UserRegisters>,
    options: u64,
    /// The stopped thread, for single-step.
    stopped_thread: u32,
}

static TRACED: SpinLock<Vec<Traced>> = SpinLock::new(Vec::new());

fn resume_key(tracee: u32) -> Key {
    const RESUME_WORDS: u64 = 1 << 62;
    Key::new(0, RESUME_WORDS | u64::from(tracee), false)
}

/// Whether process `pid` is traced.
pub fn is_traced(pid: u32) -> bool {
    let traced = TRACED.lock();
    !traced.is_empty() && traced.iter().any(|entry| entry.tracee == pid)
}

/// The tracer of process `pid`, if it is traced.
pub fn tracer_of(pid: u32) -> Option<u32> {
    TRACED
        .lock()
        .iter()
        .find(|entry| entry.tracee == pid)
        .map(|entry| entry.tracer)
}

/// A stop of one of `tracer`'s tracees its waits have not reported, matching `selected`: the
/// tracee's id and the signal it stopped with (D561). Reported once.
pub fn take_stop_report(tracer: u32, selected: impl Fn(u32) -> bool) -> Option<(u32, u8)> {
    let mut traced = TRACED.lock();
    let entry = traced.iter_mut().find(|entry| {
        entry.tracer == tracer && entry.stopped.is_some() && !entry.reported && selected(entry.tracee)
    })?;
    entry.reported = true;
    Some((entry.tracee, entry.stopped?))
}

/// Whether `tracer` traces any process `selected` picks, so its wait has something to wait for.
pub fn traces_any(tracer: u32, selected: impl Fn(u32) -> bool) -> bool {
    TRACED
        .lock()
        .iter()
        .any(|entry| entry.tracer == tracer && selected(entry.tracee))
}

/// For the thread of a traced process about to have `signal` delivered (D561): stops it for its
/// tracer, saves its registers for the tracer, and waits until resumed. The signal to deliver
/// instead (`Some(0)`: none), or `None` when the process is not traced (deliver as usual).
pub fn stop_for_signal<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    signal: u8,
) -> ControlFlow<(), Option<u8>> {
    let pid = process.id().get();
    if u32::from(signal) == SIGKILL || !is_traced(pid) {
        return ControlFlow::Continue(None);
    }
    let tracer = {
        let mut traced = TRACED.lock();
        let Some(entry) = traced.iter_mut().find(|entry| entry.tracee == pid) else {
            return ControlFlow::Continue(None);
        };
        entry.stopped = Some(signal);
        entry.reported = false;
        entry.resumed = None;
        entry.registers = Some(context.registers());
        entry.stopped_thread = thread.id();
        entry.tracer
    };
    super::wait::wake_child_waiters::<P>(tracer);
    if let Some(running) = TABLE.lock().target(tracer).and_then(|target| target.running) {
        super::wait::send_to_running::<P>(
            &running,
            Info::child_job_change(pid, 0, super::process_table::JobChange::Stopped(signal)),
        );
    }
    loop {
        let resumed = {
            let mut traced = TRACED.lock();
            match traced.iter_mut().find(|entry| entry.tracee == pid) {
                None => Some((signal, false, None)),
                Some(entry) => entry.resumed.take().map(|(given, step)| {
                    entry.stopped = None;
                    (given, step, entry.registers.take())
                }),
            }
        };
        if let Some((given, step, registers)) = resumed {
            if let Some(registers) = registers {
                context.set_registers(&registers);
            }
            super::abi::set_single_step(context, step);
            return ControlFlow::Continue(Some(given));
        }
        if process.has_ended() {
            return ControlFlow::Break(());
        }
        let _ = futex::wait::<P>(resume_key(pid), u32::MAX, u64::MAX, || {
            let traced = TRACED.lock();
            match traced.iter().find(|entry| entry.tracee == pid) {
                Some(entry) if entry.resumed.is_none() && !process.has_ended() => Ok(()),
                _ => Err(Errno::EAGAIN),
            }
        });
    }
}

/// After a successful `execve` of a traced process: its tracer stops it with `SIGTRAP`.
pub fn after_exec<P: Platform>(process: &Process<P>, thread: &mut LinuxThread) {
    if is_traced(process.id().get()) {
        super::signals::send_from_kernel_to_self(process, thread, SIGTRAP);
    }
}

/// A traced process ended: its tracing ends too.
pub fn process_ended(pid: u32) {
    let mut traced = TRACED.lock();
    if !traced.is_empty() {
        traced.retain(|entry| entry.tracee != pid && entry.tracer != pid);
    }
}

fn traced_process<P: Platform>(pid: u32) -> Result<Arc<Process<P>>, Errno> {
    let running = TABLE
        .lock()
        .target(pid)
        .and_then(|target| target.running)
        .ok_or(Errno::ESRCH)?;
    running
        .process
        .upgrade()
        .and_then(|process| process.downcast::<Process<P>>().ok())
        .ok_or(Errno::ESRCH)
}

fn may_trace<P: Platform>(tracer: &Process<P>, tracee: &Process<P>) -> bool {
    let ids = |process: &Process<P>| {
        process
            .credentials_and_limits()
            .lock()
            .credentials
            .ids(super::credentials::Kind::User)
    };
    let (mine, theirs) = (ids(tracer), ids(tracee));
    mine.effective == 0 || (mine.real == theirs.real && mine.real == theirs.effective)
}

fn add(tracee: u32, tracer: u32) -> Result<(), Errno> {
    let mut traced = TRACED.lock();
    if traced.iter().any(|entry| entry.tracee == tracee) {
        return Err(Errno::EPERM);
    }
    traced.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
    traced.push(Traced {
        tracee,
        tracer,
        stopped: None,
        reported: false,
        resumed: None,
        registers: None,
        options: 0,
        stopped_thread: 0,
    });
    Ok(())
}

fn with_stopped<R>(
    tracer: u32,
    tracee: u32,
    action: impl FnOnce(&mut Traced) -> Result<R, Errno>,
) -> Result<R, Errno> {
    let mut traced = TRACED.lock();
    let entry = traced
        .iter_mut()
        .find(|entry| entry.tracee == tracee && entry.tracer == tracer)
        .ok_or(Errno::ESRCH)?;
    if entry.stopped.is_none() {
        return Err(Errno::ESRCH);
    }
    action(entry)
}

fn resume<P: Platform>(tracer: u32, tracee: u32, signal: u64, step: bool) -> Result<u64, Errno> {
    let signal = u8::try_from(signal).map_err(|_| Errno::EIO)?;
    if step && !super::abi::SINGLE_STEPS {
        return Err(Errno::EIO);
    }
    with_stopped(tracer, tracee, |entry| {
        entry.resumed = Some((signal, step));
        Ok(())
    })?;
    futex::wake::<P>(resume_key(tracee), u32::MAX, u32::MAX);
    Ok(0)
}

/// `ptrace(request, pid, address, data)` (D561).
pub fn ptrace<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    [what, pid, address, data, ..]: [u64; 6],
) -> Result<u64, Errno> {
    let caller = process.id().get();
    let pid = pid as u32;
    match what {
        request::TRACEME => {
            let parent = TABLE.lock().parent(caller);
            if parent == 0 {
                return Err(Errno::EPERM);
            }
            add(caller, parent).map(|()| 0)
        }
        request::ATTACH | request::SEIZE => {
            if pid == caller {
                return Err(Errno::EPERM);
            }
            let tracee = traced_process::<P>(pid)?;
            if !may_trace(process, &tracee) {
                return Err(Errno::EPERM);
            }
            add(pid, caller)?;
            if what == request::SEIZE {
                with_traced(caller, pid, |entry| {
                    entry.options = data;
                    Ok(())
                })?;
            } else {
                super::signals::kill(process, thread, u64::from(pid), u64::from(SIGSTOP))?;
            }
            Ok(0)
        }
        request::INTERRUPT => {
            tracer_check(caller, pid)?;
            super::signals::kill(process, thread, u64::from(pid), u64::from(SIGSTOP))
        }
        request::PEEKTEXT | request::PEEKDATA => {
            tracer_check(caller, pid)?;
            let tracee = traced_process::<P>(pid)?;
            let mut word = [0u8; 8];
            tracee
                .read_user_memory(address, &mut word[..WORD])
                .map_err(|_| Errno::EIO)?;
            process
                .write_user_memory(data, &word[..WORD])
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        request::POKETEXT | request::POKEDATA => {
            tracer_check(caller, pid)?;
            let tracee = traced_process::<P>(pid)?;
            let word = data.to_ne_bytes();
            tracee
                .write_user_memory(address, &word[..WORD])
                .map(|()| 0)
                .map_err(|_| Errno::EIO)
        }
        request::CONT | request::SYSCALL => resume::<P>(caller, pid, data, false),
        request::SINGLESTEP => resume::<P>(caller, pid, data, true),
        request::DETACH => {
            let stopped = with_traced(caller, pid, |entry| Ok(entry.stopped.is_some()))?;
            if stopped {
                resume::<P>(caller, pid, data, false)?;
            }
            TRACED.lock().retain(|entry| entry.tracee != pid);
            futex::wake::<P>(resume_key(pid), u32::MAX, u32::MAX);
            Ok(0)
        }
        request::KILL => {
            tracer_check(caller, pid)?;
            super::signals::kill(process, thread, u64::from(pid), u64::from(SIGKILL))?;
            let _ = resume::<P>(caller, pid, 0, false);
            Ok(0)
        }
        request::SETOPTIONS => with_traced(caller, pid, |entry| {
            entry.options = data;
            Ok(0)
        }),
        request::GETEVENTMSG => {
            tracer_check(caller, pid)?;
            process
                .write_user_memory(data, &0u64.to_ne_bytes()[..WORD])
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        request::GETSIGINFO => {
            let signal = with_stopped(caller, pid, |entry| Ok(entry.stopped.unwrap_or(0)))?;
            let info = Info::from_kernel(u32::from(signal)).to_bytes();
            process
                .write_user_memory(data, &info[..128])
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        request::GETREGS => {
            let registers = with_stopped(caller, pid, |entry| entry.registers.ok_or(Errno::ESRCH))?;
            let bytes = super::abi::ptrace_registers(&registers).ok_or(Errno::EIO)?;
            process
                .write_user_memory(data, &bytes)
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        request::SETREGS => {
            let mut bytes = alloc::vec![0u8; super::abi::PTRACE_REGISTERS_BYTES];
            process
                .read_user_memory(data, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            with_stopped(caller, pid, |entry| {
                let registers = entry.registers.as_mut().ok_or(Errno::ESRCH)?;
                super::abi::set_ptrace_registers(registers, &bytes).ok_or(Errno::EIO)
            })
            .map(|()| 0)
        }
        request::GETREGSET | request::SETREGSET => {
            if address != NT_PRSTATUS {
                return Err(Errno::EINVAL);
            }
            let mut iovec = [0u8; 16];
            process
                .read_user_memory(data, &mut iovec[..2 * WORD])
                .map_err(|_| Errno::EFAULT)?;
            let base = super::abi::word(&iovec);
            let length = super::abi::word(&iovec[WORD..]) as usize;
            let size = super::abi::PTRACE_REGISTERS_BYTES.min(length);
            if what == request::GETREGSET {
                let registers =
                    with_stopped(caller, pid, |entry| entry.registers.ok_or(Errno::ESRCH))?;
                let bytes = super::abi::ptrace_registers(&registers).ok_or(Errno::EIO)?;
                process
                    .write_user_memory(base, &bytes[..size])
                    .map_err(|_| Errno::EFAULT)?;
                let mut length_bytes = [0u8; 8];
                super::abi::put_word(&mut length_bytes, size as u64);
                process
                    .write_user_memory(data + WORD as u64, &length_bytes[..WORD])
                    .map_err(|_| Errno::EFAULT)?;
                Ok(0)
            } else {
                let mut bytes = alloc::vec![0u8; super::abi::PTRACE_REGISTERS_BYTES];
                process
                    .read_user_memory(base, &mut bytes[..size])
                    .map_err(|_| Errno::EFAULT)?;
                with_stopped(caller, pid, |entry| {
                    let registers = entry.registers.as_mut().ok_or(Errno::ESRCH)?;
                    super::abi::set_ptrace_registers(registers, &bytes).ok_or(Errno::EIO)
                })
                .map(|()| 0)
            }
        }
        _ => Err(Errno::EIO),
    }
}

fn tracer_check(tracer: u32, tracee: u32) -> Result<(), Errno> {
    match tracer_of(tracee) {
        Some(found) if found == tracer => Ok(()),
        _ => Err(Errno::ESRCH),
    }
}

fn with_traced<R>(
    tracer: u32,
    tracee: u32,
    action: impl FnOnce(&mut Traced) -> Result<R, Errno>,
) -> Result<R, Errno> {
    let mut traced = TRACED.lock();
    let entry = traced
        .iter_mut()
        .find(|entry| entry.tracee == tracee && entry.tracer == tracer)
        .ok_or(Errno::ESRCH)?;
    action(entry)
}
