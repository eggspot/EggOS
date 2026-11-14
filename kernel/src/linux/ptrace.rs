//! `ptrace` for debuggers and tracers (D561): a process traced by another stops before each
//! signal is delivered to it (`SIGKILL` aside) and at `execve` (`SIGTRAP`), and its tracer learns
//! of the stop through `waitpid` as of a stopped child; the tracer reads and writes its memory
//! (`PEEK*`, `POKE*`) and registers (`GETREGS`/`SETREGS` on x86_64, `GETREGSET`/`SETREGSET` with
//! `NT_PRSTATUS` everywhere the ABI lays them out), then resumes it with `CONT` (the signal to
//! deliver, 0 for none), `SINGLESTEP`, `DETACH` or `KILL`. Tracing is per process: every thread
//! of a traced process stops at its own signals. `PTRACE_SYSCALL` stops it at each system
//! call's entry and exit, the `PTRACE_O_TRACE*` options at its forks, clones, `execve` and exit,
//! and `PTRACE_POKEUSER` and the `NT_ARM_HW_*` register sets give it hardware breakpoints and
//! watchpoints (D598).

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
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_hal::Platform;
use eggos_hal::user::{UserContext, UserRegisters};

pub mod request {
    pub const TRACEME: u64 = 0;
    pub const PEEKTEXT: u64 = 1;
    pub const PEEKDATA: u64 = 2;
    pub const PEEKUSER: u64 = 3;
    pub const POKETEXT: u64 = 4;
    pub const POKEDATA: u64 = 5;
    pub const POKEUSER: u64 = 6;
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

/// `PTRACE_SETOPTIONS`' options (D598).
pub mod option {
    pub const TRACESYSGOOD: u64 = 1;
    pub const TRACEFORK: u64 = 1 << 1;
    pub const TRACEVFORK: u64 = 1 << 2;
    pub const TRACECLONE: u64 = 1 << 3;
    pub const TRACEEXEC: u64 = 1 << 4;
    pub const TRACEVFORKDONE: u64 = 1 << 5;
    pub const TRACEEXIT: u64 = 1 << 6;
    pub const EXITKILL: u64 = 1 << 20;
}

/// The events a stop reports in its status's bits above the signal (D598).
pub mod event {
    pub const FORK: u16 = 1;
    pub const VFORK: u16 = 2;
    pub const CLONE: u16 = 3;
    pub const EXEC: u16 = 4;
    pub const VFORK_DONE: u16 = 5;
    pub const EXIT: u16 = 6;
}

const NT_PRSTATUS: u64 = 1;
const NT_ARM_HW_BREAK: u64 = 0x402;
const NT_ARM_HW_WATCH: u64 = 0x403;
/// Where `struct user` keeps `u_debugreg` on x86_64, for `PEEKUSER` and `POKEUSER`.
const USER_DEBUG_REGISTERS: u64 = 848;
const DEBUG_REGISTER_WORDS: u64 = 8;
const HW_DEBUG_SLOTS: usize = 2;
const WATCHPOINT_WORDS: usize = 8;
const HW_DEBUG_ARCH_V8: u32 = 6;
const SYSCALL_GOOD: u16 = 0x80;

/// How many traced processes want a look at each system call: their tracer asked for
/// system-call stops, an event stop is due, or they stop at their exit (D598). The system-call
/// path reads it once, so untraced programs pay a load.
static ATTENTION: AtomicUsize = AtomicUsize::new(0);

/// Whether some traced process wants its system calls looked at (D598).
#[inline]
pub fn needs_attention() -> bool {
    ATTENTION.load(Ordering::Relaxed) != 0
}

fn refresh_attention(traced: &[Traced]) {
    let wanting = traced
        .iter()
        .filter(|entry| {
            entry.syscall_stops || entry.pending.is_some() || entry.options & option::TRACEEXIT != 0
        })
        .count();
    ATTENTION.store(wanting, Ordering::Relaxed);
}
const WORD: usize = super::abi::WORD_BYTES;

/// A traced process's state.
#[derive(Debug, Clone)]
struct Traced {
    tracee: u32,
    tracer: u32,
    /// The status it stopped with (the signal, with an event above it), while it waits for its
    /// tracer; reported once.
    stopped: Option<u16>,
    reported: bool,
    /// What its tracer resumed it with: the signal to deliver (0 for none) and whether to step.
    resumed: Option<(u8, bool)>,
    /// Its registers while it is stopped, which its tracer reads and changes.
    registers: Option<UserRegisters>,
    options: u64,
    /// The stopped thread, for single-step.
    stopped_thread: u32,
    /// What `GETEVENTMSG` gives: the new child's id, the exit status, the former thread id.
    event_message: u64,
    /// Its tracer resumed it with `PTRACE_SYSCALL`.
    syscall_stops: bool,
    /// An event stop due at the end of its current system call, and its message.
    pending: Option<(u16, u64)>,
    /// The bytes a step made in software replaced, by address, until the step's breakpoint.
    stepped_over: Vec<(u64, [u8; 2])>,
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
pub fn take_stop_report(tracer: u32, selected: impl Fn(u32) -> bool) -> Option<(u32, u16)> {
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
    if u32::from(signal) == SIGTRAP {
        undo_software_step(process);
    }
    let (given, _) = stop(process, thread, context, u16::from(signal))?;
    ControlFlow::Continue(Some(given))
}

/// Puts back what a step made in software replaced, once its breakpoint was hit (D598).
fn undo_software_step<P: Platform>(process: &Process<P>) {
    let replaced = {
        let mut traced = TRACED.lock();
        traced
            .iter_mut()
            .find(|entry| entry.tracee == process.id().get())
            .map(|entry| core::mem::take(&mut entry.stepped_over))
            .unwrap_or_default()
    };
    for (address, bytes) in replaced {
        let _ = process.write_user_memory(address, &bytes);
    }
}

/// Stops the calling thread of a traced process for its tracer with `status`, saving its
/// registers for the tracer, and waits until resumed: the signal it was given and whether it
/// steps (D561, D598).
fn stop<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    status: u16,
) -> ControlFlow<(), (u8, bool)> {
    let pid = process.id().get();
    let signal = status as u8 & 0x7f;
    let tracer = {
        let mut traced = TRACED.lock();
        let Some(entry) = traced.iter_mut().find(|entry| entry.tracee == pid) else {
            return ControlFlow::Continue((signal, false));
        };
        entry.stopped = Some(status);
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
            Info::child_job_change(pid, 0, super::process_table::JobChange::Traced(status)),
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
            super::abi::set_single_step(context, step && !super::abi::STEPS_IN_SOFTWARE);
            return ControlFlow::Continue((given, step));
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

fn give_signal<P: Platform>(process: &Process<P>, thread: &mut LinuxThread, given: u8) {
    if given != 0 {
        super::signals::send_from_kernel_to_self(process, thread, u32::from(given));
    }
}

/// At a system call's entry or exit, for a process whose tracer asked for system-call stops
/// (D598): stops with `SIGTRAP`, `| 0x80` with `PTRACE_O_TRACESYSGOOD`. The tracer may change
/// the registers, the call's number and arguments at entry, its result at exit.
pub fn syscall_stop<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let pid = process.id().get();
    let status = {
        let traced = TRACED.lock();
        match traced.iter().find(|entry| entry.tracee == pid) {
            Some(entry) if entry.syscall_stops => {
                let good = entry.options & option::TRACESYSGOOD != 0;
                SIGTRAP as u16 | if good { SYSCALL_GOOD } else { 0 }
            }
            _ => return ControlFlow::Continue(()),
        }
    };
    let (given, _) = stop(process, thread, context, status)?;
    give_signal(process, thread, given);
    ControlFlow::Continue(())
}

fn event_option(event: u16) -> u64 {
    match event {
        event::FORK => option::TRACEFORK,
        event::VFORK => option::TRACEVFORK,
        event::CLONE => option::TRACECLONE,
        event::EXEC => option::TRACEEXEC,
        event::VFORK_DONE => option::TRACEVFORKDONE,
        _ => option::TRACEEXIT,
    }
}

/// Makes the event stop due at the end of the current system call, if its tracer asked for
/// the event (D598).
fn note_event(pid: u32, event: u16, message: u64) -> bool {
    let mut traced = TRACED.lock();
    let Some(entry) = traced.iter_mut().find(|entry| entry.tracee == pid) else {
        return false;
    };
    if entry.options & event_option(event) == 0 {
        return false;
    }
    entry.pending = Some((event, message));
    refresh_attention(&traced);
    true
}

/// The event stop due at the end of the system call that ends now, if any (D598).
pub fn pending_event_stop<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let pid = process.id().get();
    let pending = {
        let mut traced = TRACED.lock();
        let taken = traced
            .iter_mut()
            .find(|entry| entry.tracee == pid)
            .and_then(|entry| {
                let pending = entry.pending.take()?;
                entry.event_message = pending.1;
                Some(pending)
            });
        refresh_attention(&traced);
        taken
    };
    let Some((event, _)) = pending else {
        return ControlFlow::Continue(());
    };
    let (given, _) = stop(process, thread, context, SIGTRAP as u16 | event << 8)?;
    give_signal(process, thread, given);
    ControlFlow::Continue(())
}

/// Before a traced process exits with wait status `status`: the exit event stop, with
/// `PTRACE_O_TRACEEXIT` (D598).
pub fn before_exit<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    status: u64,
) -> ControlFlow<()> {
    if note_event(process.id().get(), event::EXIT, status << 8) {
        return pending_event_stop(process, thread, context);
    }
    ControlFlow::Continue(())
}

/// A traced process `parent` made child `child` with `fork`, `vfork` or `clone` (`event`):
/// with the matching option the child is traced by the same tracer, with the same options,
/// and starts stopped; the parent stops at the call's end (D598). Whether the child is traced.
pub fn child_made(parent: u32, child: u32, event: u16) -> bool {
    let (tracer, options) = {
        let traced = TRACED.lock();
        match traced.iter().find(|entry| entry.tracee == parent) {
            Some(entry) if entry.options & event_option(event) != 0 => {
                (entry.tracer, entry.options)
            }
            _ => return false,
        }
    };
    if add(child, tracer).is_err() {
        return false;
    }
    let _ = with_traced(tracer, child, |entry| {
        entry.options = options;
        Ok(())
    });
    note_event(parent, event, u64::from(child));
    true
}

/// After a successful `execve` of a traced process: its tracer stops it with `SIGTRAP`.
pub fn after_exec<P: Platform>(process: &Process<P>, thread: &mut LinuxThread) {
    let pid = process.id().get();
    if !is_traced(pid) {
        return;
    }
    if !note_event(pid, event::EXEC, u64::from(thread.id())) {
        super::signals::send_from_kernel_to_self(process, thread, SIGTRAP);
    }
}

/// A traced process ended: its tracing ends too.
pub fn process_ended<P: Platform>(pid: u32) {
    let killed: Vec<u32> = {
        let mut traced = TRACED.lock();
        if traced.is_empty() {
            return;
        }
        let killed = traced
            .iter()
            .filter(|entry| entry.tracer == pid && entry.options & option::EXITKILL != 0)
            .map(|entry| entry.tracee)
            .collect();
        traced.retain(|entry| entry.tracee != pid && entry.tracer != pid);
        refresh_attention(&traced);
        killed
    };
    for tracee in killed {
        if let Some(running) = TABLE.lock().target(tracee).and_then(|target| target.running) {
            super::wait::send_to_running::<P>(&running, Info::from_kernel(SIGKILL));
        }
        futex::wake::<P>(resume_key(tracee), u32::MAX, u32::MAX);
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
        event_message: 0,
        syscall_stops: false,
        pending: None,
        stepped_over: Vec::new(),
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

fn resume<P: Platform>(
    tracer: u32,
    tracee: u32,
    signal: u64,
    step: bool,
    syscall_stops: bool,
) -> Result<u64, Errno> {
    let signal = u8::try_from(signal).map_err(|_| Errno::EIO)?;
    if step && !super::abi::SINGLE_STEPS {
        return Err(Errno::EIO);
    }
    if step && super::abi::STEPS_IN_SOFTWARE {
        step_in_software::<P>(tracer, tracee)?;
    }
    {
        let mut traced = TRACED.lock();
        let entry = traced
            .iter_mut()
            .find(|entry| entry.tracee == tracee && entry.tracer == tracer)
            .ok_or(Errno::ESRCH)?;
        if entry.stopped.is_none() {
            return Err(Errno::ESRCH);
        }
        entry.resumed = Some((signal, step));
        entry.syscall_stops = syscall_stops;
        refresh_attention(&traced);
    }
    futex::wake::<P>(resume_key(tracee), u32::MAX, u32::MAX);
    Ok(0)
}

/// A step where the CPU makes none in user mode (RISC-V): a breakpoint at each instruction
/// the stopped one may go to, whose bytes are put back at the step's stop (D598).
fn step_in_software<P: Platform>(tracer: u32, tracee: u32) -> Result<(), Errno> {
    let process = traced_process::<P>(tracee)?;
    let registers = with_stopped(tracer, tracee, |entry| entry.registers.ok_or(Errno::ESRCH))?;
    let mut read = |address: u64| {
        let mut word = [0u8; 4];
        process.read_user_memory(address, &mut word).ok()?;
        Some(u32::from_le_bytes(word))
    };
    let targets = super::abi::next_instructions(&registers, &mut read);
    let mut replaced = Vec::new();
    for address in targets.into_iter().flatten() {
        if replaced.iter().any(|&(at, _)| at == address) {
            continue;
        }
        let mut original = [0u8; 2];
        process
            .read_user_memory(address, &mut original)
            .map_err(|_| Errno::EIO)?;
        process
            .write_user_memory(address, &super::abi::SOFTWARE_STEP_BREAKPOINT)
            .map_err(|_| Errno::EIO)?;
        replaced.push((address, original));
    }
    with_stopped(tracer, tracee, |entry| {
        entry.stepped_over = replaced;
        Ok(())
    })
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
        request::CONT => resume::<P>(caller, pid, data, false, false),
        request::SYSCALL => resume::<P>(caller, pid, data, false, true),
        request::SINGLESTEP => resume::<P>(caller, pid, data, true, false),
        request::PEEKUSER | request::POKEUSER => {
            tracer_check(caller, pid)?;
            let index = address
                .checked_sub(USER_DEBUG_REGISTERS)
                .filter(|offset| offset % 8 == 0 && offset / 8 < DEBUG_REGISTER_WORDS)
                .map(|offset| (offset / 8) as usize)
                .ok_or(Errno::EIO)?;
            let tracee = traced_process::<P>(pid)?;
            let mut words = tracee.user_debug_registers().unwrap_or_default();
            if what == request::POKEUSER {
                if index < 4 && !tracee.is_user_address(data) {
                    return Err(Errno::EINVAL);
                }
                words.0[index] = data;
                tracee.set_user_debug_registers(Some(words));
                return Ok(0);
            }
            process
                .write_user_memory(data, &words.0[index].to_ne_bytes()[..WORD])
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        request::DETACH => {
            let stopped = with_traced(caller, pid, |entry| Ok(entry.stopped.is_some()))?;
            if stopped {
                resume::<P>(caller, pid, data, false, false)?;
            }
            let mut traced = TRACED.lock();
            traced.retain(|entry| entry.tracee != pid);
            refresh_attention(&traced);
            drop(traced);
            futex::wake::<P>(resume_key(pid), u32::MAX, u32::MAX);
            Ok(0)
        }
        request::KILL => {
            tracer_check(caller, pid)?;
            super::signals::kill(process, thread, u64::from(pid), u64::from(SIGKILL))?;
            let _ = resume::<P>(caller, pid, 0, false, false);
            Ok(0)
        }
        request::SETOPTIONS => {
            with_traced(caller, pid, |entry| {
                entry.options = data;
                Ok(())
            })?;
            refresh_attention(&TRACED.lock());
            Ok(0)
        }
        request::GETEVENTMSG => {
            let message = with_traced(caller, pid, |entry| Ok(entry.event_message))?;
            process
                .write_user_memory(data, &message.to_ne_bytes()[..WORD])
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        request::GETSIGINFO => {
            let signal = with_stopped(caller, pid, |entry| Ok(entry.stopped.unwrap_or(0)))?;
            let info = Info::from_kernel(u32::from(signal & 0x7f)).to_bytes();
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
        request::GETREGSET | request::SETREGSET
            if matches!(address, NT_ARM_HW_BREAK | NT_ARM_HW_WATCH) =>
        {
            hardware_debug_regset::<P>(process, caller, pid, what == request::GETREGSET, address, data)
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

/// `NT_ARM_HW_BREAK` and `NT_ARM_HW_WATCH` (D598): `struct user_hwdebug_state`, the slot count
/// and debug architecture, then an address and a control word for each of the two slots EggOS
/// loads, kept as the traced process's breakpoints (words 0 to 3) or watchpoints (8 to 11).
fn hardware_debug_regset<P: Platform>(
    process: &Process<P>,
    caller: u32,
    pid: u32,
    get: bool,
    set: u64,
    data: u64,
) -> Result<u64, Errno> {
    tracer_check(caller, pid)?;
    let tracee = traced_process::<P>(pid)?;
    let first = match set {
        NT_ARM_HW_BREAK => 0,
        _ => WATCHPOINT_WORDS,
    };
    let mut iovec = [0u8; 16];
    process
        .read_user_memory(data, &mut iovec[..2 * WORD])
        .map_err(|_| Errno::EFAULT)?;
    let base = super::abi::word(&iovec);
    let length = super::abi::word(&iovec[WORD..]) as usize;
    let bytes = 8 + 16 * HW_DEBUG_SLOTS;
    let size = bytes.min(length);
    let mut words = tracee.user_debug_registers().unwrap_or_default();
    if get {
        let mut state = alloc::vec![0u8; bytes];
        let info = HW_DEBUG_ARCH_V8 << 8 | HW_DEBUG_SLOTS as u32;
        state[..4].copy_from_slice(&info.to_ne_bytes());
        for slot in 0..HW_DEBUG_SLOTS {
            let at = 8 + 16 * slot;
            state[at..at + 8].copy_from_slice(&words.0[first + 2 * slot].to_ne_bytes());
            state[at + 8..at + 12]
                .copy_from_slice(&(words.0[first + 2 * slot + 1] as u32).to_ne_bytes());
        }
        process
            .write_user_memory(base, &state[..size])
            .map_err(|_| Errno::EFAULT)?;
        let mut length_bytes = [0u8; 8];
        super::abi::put_word(&mut length_bytes, size as u64);
        process
            .write_user_memory(data + WORD as u64, &length_bytes[..WORD])
            .map_err(|_| Errno::EFAULT)?;
        return Ok(0);
    }
    let mut state = alloc::vec![0u8; bytes];
    process
        .read_user_memory(base, &mut state[..size])
        .map_err(|_| Errno::EFAULT)?;
    for slot in 0..HW_DEBUG_SLOTS {
        let at = 8 + 16 * slot;
        if at + 12 > size {
            break;
        }
        let address = u64::from_ne_bytes(state[at..at + 8].try_into().unwrap_or_default());
        let control = u32::from_ne_bytes(state[at + 8..at + 12].try_into().unwrap_or_default());
        if control & 1 != 0 && !tracee.is_user_address(address) {
            return Err(Errno::EINVAL);
        }
        words.0[first + 2 * slot] = address;
        words.0[first + 2 * slot + 1] = u64::from(control);
    }
    tracee.set_user_debug_registers(Some(words));
    Ok(0)
}

/// Whether `reader` may read process `target`'s `/proc` files that `PTRACE_MODE_READ` guards
/// (`maps`, `stat`, `fd`, `exe`, D598): itself, root, or the same real user with the target's
/// effective user too.
pub fn may_read<P: Platform>(reader: &Process<P>, target: u32) -> bool {
    if reader.id().get() == target {
        return true;
    }
    match traced_process::<P>(target) {
        Ok(other) => may_trace(reader, &other),
        Err(_) => true,
    }
}

