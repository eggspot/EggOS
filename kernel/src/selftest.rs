//! Boot-time self-tests: checks that only mean something on the real (or emulated) machine,
//! such as "did the architecture entry point zero `.bss`". The QEMU test runner reads the
//! verdict markers from the console.

use crate::HEAP;
use crate::address_space::AddressSpace;
use crate::memory::{self, FrameAllocator, MAX_ORDER, Memory};
use crate::process::{self, Process};
use crate::sync::SpinLock;
use crate::{cpu, exception, interrupt, thread};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format, MemoryType};
use eggos_hal::user::{TestProgram, UserExit};
use eggos_hal::{ExceptionKind, Platform};

/// Printed when every self-test passed. The QEMU runner requires this exact line.
pub const PASS_MARKER: &str = "EGGOS-SELFTEST: PASS";
/// Printed when a self-test failed or the kernel panicked.
pub const FAIL_MARKER: &str = "EGGOS-SELFTEST: FAIL";
/// Printed, followed by the address, before the `qemu-test-fatal-exception` kernel writes its
/// read-only data. The QEMU runner requires the fault report to name that address.
pub const WRITE_PROTECT_PROBE: &str = "fatal exception test: writing read-only data at ";

/// CPUs the QEMU boot tests run with (`-smp`). Must match `TEST_CPUS` in `xtask`. The
/// benchmarks run on one.
#[cfg(all(feature = "qemu-test", not(feature = "qemu-bench")))]
const QEMU_CPUS: usize = 4;

/// How long the started CPUs have to report in.
const CPU_START_TIMEOUT_MS: u64 = 1000;
/// How long a CPU has to take a wake-up interrupt.
const WAKEUP_TIMEOUT_MS: u64 = 1000;
/// How long the threads placed on the CPUs have to all run at once, and to wait together.
const THREADS_MEET_TIMEOUT_MS: u64 = 1000;
/// How long a thread made ready on a busy CPU has to run beside the busy thread on another CPU.
const THREADS_SPREAD_TIMEOUT_MS: u64 = 1000;

/// A self-test: a name and a check of `context` (for the boot self-tests, the kernel's
/// [`Memory`]) returning `Err(reason)` on failure.
pub type SelfTest<C> = (&'static str, fn(&mut C) -> Result<(), &'static str>);

/// Every boot self-test for platform `P`, in run order.
pub fn all<P: Platform>() -> [SelfTest<Memory<P>>; 19] {
    [
        ("bss_is_zeroed", |_| bss_is_zeroed()),
        ("breakpoint_resumes", |_| breakpoint_resumes::<P>()),
        ("timer_interrupt_fires", |_| timer_interrupt_fires::<P>()),
        ("frames_are_usable_ram", frames_are_usable_ram::<P>),
        (
            "contiguous_frames_are_aligned_and_merge_back",
            contiguous_frames_are_aligned_and_merge_back::<P>,
        ),
        (
            "page_tables_protect_the_kernel",
            page_tables_protect_the_kernel::<P>,
        ),
        (
            "heap_allocates_in_the_direct_map",
            heap_allocates_in_the_direct_map::<P>,
        ),
        (
            "address_spaces_keep_user_pages_apart",
            address_spaces_keep_user_pages_apart::<P>,
        ),
        (
            "user_programs_make_system_calls_and_fault",
            user_programs_make_system_calls_and_fault::<P>,
        ),
        ("threads_take_turns_and_free_their_stacks", |_| {
            threads_take_turns_and_free_their_stacks::<P>()
        }),
        ("threads_are_preempted_at_preemption_points", |_| {
            threads_are_preempted_at_preemption_points::<P>()
        }),
        ("threads_sleep_wait_and_run_tickless", |_| {
            threads_sleep_wait_and_run_tickless::<P>()
        }),
        (
            "processes_keep_their_own_memory_across_switches",
            |memory| processes_keep_their_own_memory::<P>(memory, 2),
        ),
        (
            "processes_are_preempted_on_return_to_user_mode",
            processes_are_preempted_on_return_to_user_mode::<P>,
        ),
        (
            "secondary_cpus_start_and_report_in",
            secondary_cpus_start_and_report_in::<P>,
        ),
        ("cpus_take_their_own_interrupts", |_| {
            cpus_take_their_own_interrupts::<P>()
        }),
        ("threads_run_on_every_cpu", |_| {
            threads_run_on_every_cpu::<P>()
        }),
        ("threads_move_to_idle_cpus", |_| {
            threads_move_to_idle_cpus::<P>()
        }),
        (
            "processes_run_on_every_cpu",
            processes_run_on_every_cpu::<P>,
        ),
    ]
}

/// Runs `tests` on `context`, reporting each and then the verdict marker. Returns whether all
/// passed.
pub fn run<W: Write, C>(
    console: &mut W,
    tests: &[SelfTest<C>],
    context: &mut C,
) -> Result<bool, core::fmt::Error> {
    let mut all_passed = true;
    for (name, test) in tests {
        match test(context) {
            Ok(()) => writeln!(console, "selftest {name} ... ok")?,
            Err(reason) => {
                all_passed = false;
                writeln!(console, "selftest {name} ... FAILED: {reason}")?;
            }
        }
    }
    let marker = if all_passed { PASS_MARKER } else { FAIL_MARKER };
    writeln!(console, "{marker}")?;
    Ok(all_passed)
}

/// Lives in `.bss` (zero-initialised and interior-mutable, so it cannot be placed in
/// read-only data). Reading anything but zero means the entry point did not clear `.bss`, as
/// on real hardware and warm reboots, where RAM holds whatever was there before.
///
/// QEMU hands over zeroed RAM, which would let a missing zeroing loop pass, so the QEMU boot
/// tests look the probe up by its symbol name and fill it with a non-zero pattern before the
/// kernel starts. The name must match `BSS_PROBE_SYMBOL` in `xtask`.
#[unsafe(export_name = "eggos_selftest_bss_probe")]
static BSS_PROBE: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn bss_is_zeroed() -> Result<(), &'static str> {
    let all_zero = BSS_PROBE.iter().all(|word| {
        // SAFETY: `word.as_ptr()` points to a live, aligned `u64` (an `AtomicU64` has the
        // layout of `u64`) that nothing writes concurrently. The read must be volatile:
        // since nothing ever stores to the probe, the compiler would otherwise fold an
        // ordinary load to the initial value and the check would test nothing.
        unsafe { core::ptr::read_volatile(word.as_ptr()) == 0 }
    });
    if all_zero {
        Ok(())
    } else {
        Err(".bss was not zeroed by the architecture entry point")
    }
}

/// A breakpoint must reach the kernel's exception handler through the architecture's vectors
/// and come back to the next instruction with the interrupted code's state intact.
fn breakpoint_resumes<P: Platform>() -> Result<(), &'static str> {
    // Values live across the breakpoint, in registers or on the stack (the `asm!` block
    // declares no clobbers), so a handler that corrupts either changes the result.
    let live = [1u64, 2, 3, 4, 5, 6, 7, 8].map(|n| core::hint::black_box(n * 0x0101_0101_0101));
    let before = exception::breakpoints_taken();
    P::breakpoint();
    // Greater rather than exactly one more: host unit tests run in parallel and may take
    // breakpoints of their own at the same time.
    if exception::breakpoints_taken() <= before {
        return Err("the breakpoint did not reach the kernel's exception handler");
    }
    let expected = [1u64, 2, 3, 4, 5, 6, 7, 8].map(|n| n * 0x0101_0101_0101);
    if core::hint::black_box(live) != expected {
        return Err("values live across the breakpoint changed (registers or stack not restored)");
    }
    Ok(())
}

/// A timer deadlined 1 ms ahead must interrupt the CPU, not before the deadline, and the
/// clock must move forward. Gives up after 1 s of clock time; a timer that never fires while
/// the clock also stops leaves the CPU asleep, which the QEMU runner's timeout reports.
fn timer_interrupt_fires<P: Platform>() -> Result<(), &'static str> {
    let frequency = P::clock_frequency();
    if frequency < 1000 {
        return Err("the clock runs slower than 1 kHz");
    }
    let before = interrupt::timer_interrupts::<P>();
    let start = P::clock_ticks();
    let deadline = start + frequency / 1000;
    let give_up = start + frequency;
    P::set_timer_deadline(deadline);
    // Greater rather than exactly one more, as in `breakpoint_resumes`.
    while interrupt::timer_interrupts::<P>() <= before {
        if P::clock_ticks() > give_up {
            return Err("no timer interrupt within 1 s of its deadline");
        }
        P::wait_for_interrupt();
    }
    if P::clock_ticks() < deadline {
        return Err("the timer interrupt came before its deadline");
    }
    Ok(())
}

/// Frames written in [`frames_are_usable_ram`].
const FRAMES_WRITTEN: usize = 64;

/// Where the kernel reaches the first word of `frame` (through the allocator's offset).
fn first_word(frames: &FrameAllocator, frame: u64) -> *mut u64 {
    (frame as usize).wrapping_add(frames.physical_memory_offset()) as *mut u64
}

/// The boot memory map must offer memory, and its frames must be RAM the kernel can write and
/// read back through the direct map, handed out once each: write a pattern to the first frames
/// allocated, check them all, free them, see them come back in reverse order, then take every
/// other frame up to the end of memory, writing each, and give them all back. A frame
/// overlapping the kernel image, its stack or a page table would most likely crash the kernel
/// here instead of passing.
fn frames_are_usable_ram<P: Platform>(_: &mut Memory<P>) -> Result<(), &'static str> {
    let mut frames = memory::FRAMES.lock();
    every_frame_is_usable_ram(frames.as_mut().ok_or("there is no frame allocator")?)
}

/// [`frames_are_usable_ram`] on `frames`.
fn every_frame_is_usable_ram(frames: &mut FrameAllocator) -> Result<(), &'static str> {
    let total = frames.free_frames();
    if total == 0 {
        return Err("the boot memory map has no free memory");
    }
    let mut taken = [0u64; FRAMES_WRITTEN];
    let mut count = 0;
    for slot in &mut taken {
        let Some(frame) = frames.allocate() else {
            break;
        };
        *slot = frame;
        count += 1;
    }
    let taken = &taken[..count];
    let words = |frames: &FrameAllocator, frame: u64| {
        let start = first_word(frames, frame);
        let length = eggos_hal::memory::FRAME_SIZE as usize / 8;
        (0..length).map(move |i| (start.wrapping_add(i), frame ^ i as u64))
    };
    for &frame in taken {
        for (word, value) in words(frames, frame) {
            // SAFETY: `frame` was just allocated, so the kernel owns it, and the allocator
            // reaches free RAM at physical address + its offset; `word` is inside the frame
            // and aligned.
            unsafe { word.write_volatile(value) };
        }
    }
    for &frame in taken {
        for (word, value) in words(frames, frame) {
            // SAFETY: as above; the frame is still owned and was written in full.
            if unsafe { word.read_volatile() } != value {
                return Err("a frame did not read back what was written (not RAM, or shared)");
            }
        }
    }
    for &frame in taken {
        // SAFETY: each frame came from `frames` and is no longer used.
        unsafe { frames.free(frame) };
    }
    if taken
        .iter()
        .rev()
        .any(|&frame| frames.allocate() != Some(frame))
    {
        return Err("freed frames did not come back last in, first out");
    }
    // Every other frame, chained through its first word so they can all be given back.
    let mut handed_out = taken.len() as u64;
    let mut chain = None;
    while let Some(frame) = frames.allocate() {
        // SAFETY: as above, for the frame just allocated.
        unsafe { first_word(frames, frame).write_volatile(chain.unwrap_or(u64::MAX)) };
        chain = Some(frame);
        handed_out += 1;
    }
    while let Some(frame) = chain {
        // SAFETY: the frame is still owned; its first word holds the next link.
        let next = unsafe { first_word(frames, frame).read_volatile() };
        chain = (next != u64::MAX).then_some(next);
        // SAFETY: `frame` came from `frames` and, its link read, is no longer used.
        unsafe { frames.free(frame) };
    }
    for &frame in taken {
        // SAFETY: each frame came from `frames` and is no longer used.
        unsafe { frames.free(frame) };
    }
    if handed_out != total || frames.free_frames() != total {
        return Err("the allocator handed out a different number of frames than the map holds");
    }
    Ok(())
}

/// Blocks of contiguous frames of every order, largest first, must come out aligned to their
/// size, be RAM the kernel can write, not overlap, and merge back when freed, so the largest
/// block can be taken again. The largest order is [`MAX_ORDER`], or less on a machine with
/// under 16 MiB free (a quarter of free memory at most, which leaves room for the page tables
/// and slabs already scattered through memory). Runs after [`frames_are_usable_ram`], which
/// sent every frame through the free stack, so the blocks must form again from freed frames.
fn contiguous_frames_are_aligned_and_merge_back<P: Platform>(
    _: &mut Memory<P>,
) -> Result<(), &'static str> {
    let mut frames = memory::FRAMES.lock();
    let frames = frames.as_mut().ok_or("there is no frame allocator")?;
    let free = frames.free_frames();
    let top = (free / 4).checked_ilog2().unwrap_or(0).min(MAX_ORDER);
    let mut blocks = [0u64; MAX_ORDER as usize + 1];
    for order in (0..=top).rev() {
        let block = frames
            .allocate_contiguous(order)
            .ok_or("no block of contiguous frames")?;
        let size = FRAME_SIZE << order;
        if !block.is_multiple_of(size) {
            return Err("a block of contiguous frames is not aligned to its size");
        }
        let start = (block as usize).wrapping_add(frames.physical_memory_offset()) as *mut u64;
        let last = start.wrapping_add(size as usize / 8 - 1);
        for word in [start, last] {
            // SAFETY: the block was just allocated, so the kernel owns it; the allocator
            // reaches it through its offset, and both words are inside it and aligned.
            unsafe { word.write_volatile(block ^ order as u64) };
        }
        blocks[order as usize] = block;
    }
    let blocks = &blocks[..=top as usize];
    for (order, &block) in blocks.iter().enumerate() {
        let size = FRAME_SIZE << order;
        let start = (block as usize).wrapping_add(frames.physical_memory_offset()) as *mut u64;
        let last = start.wrapping_add(size as usize / 8 - 1);
        for word in [start, last] {
            // SAFETY: as above; the block is still owned.
            if unsafe { word.read_volatile() } != block ^ order as u64 {
                return Err("blocks of contiguous frames overlap");
            }
        }
    }
    for (order, &block) in blocks.iter().enumerate() {
        // SAFETY: allocated above with this order and no longer used.
        unsafe { frames.free_contiguous(block, order as u32) };
    }
    if frames.free_frames() != free {
        return Err("freeing blocks of contiguous frames lost frames");
    }
    let largest = frames
        .allocate_contiguous(top)
        .ok_or("freed blocks did not merge back into the largest one")?;
    // SAFETY: just allocated with this order and not used.
    unsafe { frames.free_contiguous(largest, top) };
    Ok(())
}

/// The kernel's page tables must give each part of the kernel image only the access it needs,
/// at its virtual address and through the direct map (never executable there), leave the stack
/// guard page unmapped at both, map nothing at the image's physical addresses (the boot identity
/// map is gone), map the early devices as device memory in the direct map, and put the direct
/// map where the platform says, in use: a frame written through the direct map is where the
/// tables say.
fn page_tables_protect_the_kernel<P: Platform>(memory: &mut Memory<P>) -> Result<(), &'static str> {
    let tables = &memory.page_tables;
    let image = P::kernel_image();
    for (part, attributes) in image.mapped_parts() {
        for address in [part.start, part.end.wrapping_sub(1)] {
            let virt = image.virtual_address(address);
            if !part.is_empty() && tables.translate(virt) != Some((address, attributes)) {
                return Err("a part of the kernel image is not mapped with exactly its access");
            }
            if !part.is_empty() && tables.translate(address).is_some() {
                return Err("the kernel image is still mapped at its physical address");
            }
        }
    }
    let offset = P::direct_map_offset() as u64;
    for (part, attributes) in image.mapped_parts() {
        let alias = Some(crate::paging::direct_map_attributes(attributes));
        for address in [part.start, part.end.wrapping_sub(1)] {
            if !part.is_empty()
                && tables.translate(address + offset).map(|t| (t.0, Some(t.1)))
                    != Some((address, alias))
            {
                return Err("the direct map lets the kernel image be changed or executed");
            }
        }
    }
    let guard = image.stack_guard();
    if guard.is_empty()
        || tables
            .translate(image.virtual_address(guard.start))
            .is_some()
        || tables.translate(guard.start + offset).is_some()
    {
        return Err("the boot stack has no unmapped guard page below it");
    }
    let mut devices_ok = true;
    P::early_device_regions(
        &mut |device| match tables.translate(device.start + offset) {
            Some((phys, attributes)) => {
                devices_ok &= phys == device.start && attributes.writable && !attributes.executable;
                devices_ok &= !<P::PageTableFormat as Format>::ENCODES_MEMORY_TYPE
                    || attributes.memory_type == MemoryType::Device;
                devices_ok &= tables.translate(device.start).is_none();
            }
            None => devices_ok = false,
        },
    );
    if !devices_ok {
        return Err("an early device is not mapped as device memory in the direct map only");
    }
    if P::physical_memory_offset() != P::direct_map_offset() {
        return Err("the platform does not reach physical memory through the direct map");
    }
    let mut frames = memory::FRAMES.lock();
    let frames = frames.as_mut().ok_or("there is no frame allocator")?;
    let frame = frames.allocate().ok_or("no free frame")?;
    let word = first_word(frames, frame);
    // SAFETY: the frame was just allocated, so the kernel owns it; the allocator reaches it at
    // physical address + offset.
    unsafe { word.write_volatile(0x5eed_f00d) };
    let translated = tables.translate(word as u64);
    // SAFETY: as above.
    let read_back = unsafe { word.read_volatile() };
    // SAFETY: the frame came from `frames` and is no longer used.
    unsafe { frames.free(frame) };
    if translated != Some((frame, Attributes::READ_WRITE)) || read_back != 0x5eed_f00d {
        return Err("the direct map does not translate to the frame written through it");
    }
    Ok(())
}

/// Two user address spaces map a frame each at one user address (D45). With either active,
/// the kernel reads that space's frame there, and a write there lands in it; the kernel's own
/// low half stays empty. Then the first space maps the second's frame instead and must show it:
/// a switch that kept a stale translation shows the old frame (the second read already
/// fails if switching from one space to another keeps the first's). Dropping the spaces gives
/// every table back.
fn address_spaces_keep_user_pages_apart<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    const USER_VIRT: u64 = 0x40_0000;
    const PATTERNS: [u64; 2] = [0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];
    const WRITTEN: u64 = 0x5555_aaaa_5555_aaaa;
    let (before, pages) = {
        let mut frames = memory::FRAMES.lock();
        let frames = frames.as_mut().ok_or("there is no frame allocator")?;
        let before = frames.free_frames();
        let mut pages = [0; 2];
        for (page, pattern) in pages.iter_mut().zip(PATTERNS) {
            *page = frames.allocate().ok_or("no frame for a user page")?;
            // SAFETY: the frame was just allocated, so the test owns it; the allocator reaches
            // it through its offset.
            unsafe { first_word(frames, *page).write_volatile(pattern) };
        }
        (before, pages)
    };
    let read = |space: &AddressSpace<P>| {
        let word = user_word(space, USER_VIRT);
        // SAFETY: `space` maps `USER_VIRT` to a frame this test owns, and is active while
        // `word` is read, inside the window that lets the kernel read user pages.
        space.with_active(|| P::with_user_access(|| unsafe { word.read_volatile() }))
    };
    let result = (|| {
        // SAFETY: `memory.page_tables` are the kernel's tables, which `paging::enable`
        // activated on this CPU, and every CPU started since runs on them.
        let new =
            || unsafe { AddressSpace::<P>::new(&memory.page_tables) }.map_err(|e| e.description());
        let mut spaces = [new()?, new()?];
        for (space, page) in spaces.iter_mut().zip(pages) {
            space
                .map(USER_VIRT, page, FRAME_SIZE, Attributes::USER_READ_WRITE)
                .map_err(|e| e.description())?;
        }
        for (space, pattern) in spaces.iter().zip(PATTERNS) {
            if read(space) != pattern {
                return Err("an active address space does not show its own page");
            }
        }
        let word = user_word(&spaces[0], USER_VIRT);
        spaces[0].with_active(|| {
            // SAFETY: as in `read`: the first space maps `USER_VIRT` writable to `pages[0]`.
            P::with_user_access(|| unsafe { word.write_volatile(WRITTEN) })
        });
        let guard = memory::FRAMES.lock();
        let frames = guard.as_ref().ok_or("there is no frame allocator")?;
        // SAFETY: `pages[0]` is the test's frame, reached through the allocator's offset.
        if unsafe { first_word(frames, pages[0]).read_volatile() } != WRITTEN {
            return Err("a write to a user page did not land in its frame");
        }
        drop(guard);
        if memory.page_tables.translate(USER_VIRT).is_some() {
            return Err("a user page appeared in the kernel's own tables");
        }
        let [first, _] = &mut spaces;
        first
            .unmap(USER_VIRT, FRAME_SIZE)
            .and_then(|()| first.map(USER_VIRT, pages[1], FRAME_SIZE, Attributes::USER_READ_ONLY))
            .map_err(|e| e.description())?;
        if read(first) != PATTERNS[1] {
            return Err("an address space showed a page it no longer maps");
        }
        Ok(())
    })();
    let mut frames = memory::FRAMES.lock();
    let frames = frames.as_mut().ok_or("there is no frame allocator")?;
    for page in pages {
        // SAFETY: allocated above, and no address space maps them any more: they were
        // dropped at the end of the closure.
        unsafe { frames.free(page) };
    }
    result?;
    if frames.free_frames() != before {
        return Err("dropping the address spaces did not free their tables");
    }
    Ok(())
}

/// A program in user mode must make system calls with its arguments in the ABI's registers, get
/// each result back with every other register as it was, and, reading kernel memory, fault
/// without stopping the kernel; a timer interrupt must take the CPU back from a program that
/// never makes a system call. Everything the program took must be freed afterwards (D47).
fn user_programs_make_system_calls_and_fault<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    const FIRST_RESULT: u64 = 0x5eed;
    let free_frames = || {
        memory::FRAMES
            .lock()
            .as_ref()
            .map_or(0, FrameAllocator::free_frames)
    };
    let before = free_frames();
    let kernel_address = {
        let image = P::kernel_image();
        image.virtual_address(image.rodata_start)
    };
    // SAFETY: `memory.page_tables` are the kernel's tables, which `paging::enable` activated on
    // this CPU, and every CPU started since runs on them.
    let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }?;
    let syscall = |context: &eggos_hal::user::UserContext, exit, number| {
        if exit != UserExit::Syscall {
            return Err("the program did not make its system call");
        }
        if context.syscall_number() != number {
            return Err("a system call came with the wrong number");
        }
        Ok(context.syscall_arguments())
    };
    program.with_active(|| {
        let mut context = Process::<P>::test_program_context(|program| program.checks);
        // SAFETY: interrupts are masked, as the kernel runs; the program's address space is
        // active, sharing the kernel's half; nothing here switches threads or address spaces
        // before `run_user` returns. The same holds for each `run_user` below.
        let exit = unsafe { P::run_user(&mut context) };
        if syscall(&context, exit, TestProgram::SYSCALL_FIRST)? != [1, 2, 3, 4, 5, 6] {
            return Err("a system call's arguments did not arrive");
        }
        context.set_syscall_result(FIRST_RESULT);
        // SAFETY: as above.
        let exit = unsafe { P::run_user(&mut context) };
        let [result, differences, ..] = syscall(&context, exit, TestProgram::SYSCALL_REPORT)?;
        if result != FIRST_RESULT {
            return Err("a system call's result did not reach the program");
        }
        if differences != 0 {
            return Err("the program's registers changed across a system call");
        }
        context.set_syscall_result(kernel_address);
        // SAFETY: as above.
        match unsafe { P::run_user(&mut context) } {
            UserExit::Exception(exception)
                if exception.kind == ExceptionKind::DataFault
                    && exception.address == Some(kernel_address)
                    && (process::TEST_PROGRAM_ADDRESS
                        ..process::TEST_PROGRAM_ADDRESS + FRAME_SIZE)
                        .contains(&exception.pc) => {}
            UserExit::Exception(_) => {
                return Err("the program's read of kernel memory was misreported");
            }
            _ => return Err("the program read kernel memory without a fault"),
        }
        let mut context = Process::<P>::test_program_context(|program| program.spin);
        let spin = context.pc();
        let timer_interrupts = interrupt::timer_interrupts::<P>();
        // A deadline already passed: the interrupt is pending as the program starts.
        P::set_timer_deadline(0);
        // SAFETY: as above.
        if unsafe { P::run_user(&mut context) } != UserExit::Interrupt {
            return Err("a timer interrupt did not take the CPU back from the program");
        }
        if interrupt::timer_interrupts::<P>() <= timer_interrupts {
            return Err("the timer interrupt from user mode did not reach the kernel");
        }
        if context.pc() != spin {
            return Err("the interrupted program is not where it was");
        }
        Ok(())
    })?;
    drop(program);
    if free_frames() != before {
        return Err("the program's frame and tables were not all freed");
    }
    Ok(())
}

/// Where [`processes_keep_their_own_memory`] maps each process's data page.
const PROCESS_DATA_ADDRESS: u64 = 0x50_0000;

/// What a process's data page holds: its number, in a pattern no other page has.
fn process_tag(number: usize) -> u64 {
    0x7a6_0000_0000 | number as u64
}

/// `count` processes, each with the test program and a data page of its own at the same
/// address, holding its [`process_tag`].
fn processes_with_data<P: Platform>(
    memory: &Memory<P>,
    count: usize,
) -> Result<Vec<Arc<Process<P>>>, &'static str> {
    (0..count)
        .map(|number| {
            // SAFETY: `memory.page_tables` are the kernel's tables, which `paging::enable`
            // activated on this CPU, and every CPU started since runs on them.
            let mut process = unsafe { Process::<P>::with_test_program(&memory.page_tables) }?;
            process.map_new_page(
                PROCESS_DATA_ADDRESS,
                Attributes::USER_READ_ONLY,
                &process_tag(number).to_ne_bytes(),
            )?;
            Ok(Arc::new(process))
        })
        .collect()
}

/// Processes must each see their own memory at the same address, whatever ran in between (D48):
/// `count` processes' threads make system calls, and at each the handler yields, so the others
/// run (and load their address spaces) on its CPU, then reads the process's data page through
/// [`Process::with_user_memory`], which must load its own again. After the threads end, every
/// frame the processes took must be free and no CPU may keep an address space loaded.
fn processes_keep_their_own_memory<P: Platform>(
    memory: &mut Memory<P>,
    count: usize,
) -> Result<(), &'static str> {
    const CALLS: u64 = 20;
    /// A read that did not show the process's own page, or an exception.
    const WRONG: u64 = u64::MAX;
    let before = free_frames().ok_or("there is no frame allocator")?;
    let processes = processes_with_data(memory, count)?;
    let mut threads = Vec::new();
    for (number, process) in processes.into_iter().enumerate() {
        let calls = Arc::new(AtomicU64::new(0));
        let counted = calls.clone();
        let context = Process::<P>::test_program_context(|program| program.null_syscalls);
        let thread = process::spawn::<P, _>(process, context, move |process, _, exit| {
            if exit == UserExit::Interrupt {
                return ControlFlow::Continue(());
            }
            if exit != UserExit::Syscall {
                counted.store(WRONG, Ordering::Relaxed);
                return ControlFlow::Break(());
            }
            thread::yield_now::<P>();
            let word = user_word(process.address_space(), PROCESS_DATA_ADDRESS);
            // SAFETY: the process's data page, mapped at that address, readable by user mode,
            // which `with_user_memory` lets the kernel read while it runs; a `u64` fits there.
            let seen = process.with_user_memory(|| unsafe { word.read_volatile() });
            if seen != process_tag(number) {
                counted.store(WRONG, Ordering::Relaxed);
                return ControlFlow::Break(());
            }
            if counted.fetch_add(1, Ordering::Relaxed) + 1 < CALLS {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        });
        threads.push((
            thread.map_err(|_| "could not start a process's thread")?,
            calls,
        ));
    }
    let mut result = Ok(());
    for (thread, calls) in threads {
        thread.join::<P>();
        match calls.load(Ordering::Relaxed) {
            CALLS => {}
            WRONG => result = Err("a process saw another's memory, or faulted"),
            _ => result = Err("a process's thread ended early"),
        }
    }
    result?;
    release_loaded_address_spaces::<P>();
    if cpu::CPUS
        .all()
        .iter()
        .any(|cpu| cpu.active_address_space() != cpu::NO_ADDRESS_SPACE)
        || cpu::current::<P>().active_address_space() != cpu::NO_ADDRESS_SPACE
    {
        return Err("a CPU kept an ended process's address space loaded");
    }
    if free_frames() != Some(before) {
        return Err("the processes' frames and tables were not all freed");
    }
    Ok(())
}

/// Has every other CPU's idle thread look once more, so that a CPU that kept an ended process's
/// address space loaded lets it go (D48): wakes each, then gives them a millisecond.
fn release_loaded_address_spaces<P: Platform>() {
    let here = cpu::current::<P>();
    for &other in cpu::CPUS.all() {
        if !core::ptr::eq(other, here) {
            cpu::wake::<P>(other);
        }
    }
    if cpu::CPUS.all().len() > 1 {
        thread::sleep_until::<P>(P::clock_ticks() + P::clock_frequency() / 1000);
    }
}

/// Programs that never make a system call must still take turns (D48): with preemption on, two
/// processes' threads spinning in user mode on this CPU must each be interrupted, and so run,
/// several times while the boot thread sleeps in between; then each ends at its next
/// interrupt. Without preemption on return to user mode, the first would keep the CPU and the
/// boot thread would never run again, which the QEMU runner's timeout reports. Runs before the
/// other CPUs start, so everything runs on this one.
fn processes_are_preempted_on_return_to_user_mode<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    const TURNS: u64 = 3;
    static STOP: AtomicBool = AtomicBool::new(false);
    STOP.store(false, Ordering::Relaxed);
    let before = free_frames().ok_or("there is no frame allocator")?;
    let millisecond = P::clock_frequency() / 1000;
    let mut threads = Vec::new();
    thread::enable_preemption::<P>();
    for process in processes_with_data(memory, 2)? {
        let interrupts = Arc::new(AtomicU64::new(0));
        let counted = interrupts.clone();
        let context = Process::<P>::test_program_context(|program| program.spin);
        let thread = process::spawn::<P, _>(process, context, move |_, _, exit| {
            if exit != UserExit::Interrupt {
                counted.store(u64::MAX, Ordering::Relaxed);
                return ControlFlow::Break(());
            }
            counted.fetch_add(1, Ordering::Relaxed);
            if STOP.load(Ordering::Relaxed) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        match thread {
            Ok(thread) => threads.push((thread, interrupts)),
            Err(_) => {
                STOP.store(true, Ordering::Relaxed);
                break;
            }
        }
    }
    let give_up = P::clock_ticks() + THREADS_MEET_TIMEOUT_MS * millisecond;
    let all_turns = |threads: &[(thread::JoinHandle, Arc<AtomicU64>)]| {
        threads
            .iter()
            .all(|(_, interrupts)| interrupts.load(Ordering::Relaxed) >= TURNS)
    };
    while threads.len() == 2 && !all_turns(&threads) && P::clock_ticks() < give_up {
        thread::sleep_until::<P>(P::clock_ticks() + millisecond);
    }
    let took_turns = threads.len() == 2 && all_turns(&threads);
    STOP.store(true, Ordering::Relaxed);
    // Each ends at its next interrupt: this thread's sleeps provide them once one runs alone.
    while !threads.iter().all(|(thread, _)| thread.is_finished()) {
        thread::sleep_until::<P>(P::clock_ticks() + millisecond);
    }
    thread::disable_preemption::<P>();
    let faulted = threads
        .iter()
        .any(|(_, interrupts)| interrupts.load(Ordering::Relaxed) == u64::MAX);
    for (thread, _) in threads {
        thread.join::<P>();
    }
    if faulted {
        return Err("a spinning program left user mode other than by an interrupt");
    }
    if !took_turns {
        return Err("two programs in user mode did not take turns on one CPU");
    }
    if free_frames() != Some(before) {
        return Err("the processes' frames and tables were not all freed");
    }
    Ok(())
}

/// [`processes_keep_their_own_memory`] with two processes per CPU (up to 16), so they run, load
/// their address spaces and their code on the other CPUs too, and move between them.
fn processes_run_on_every_cpu<P: Platform>(memory: &mut Memory<P>) -> Result<(), &'static str> {
    let cpus = cpu::CPUS.all().len();
    if cpus <= 1 {
        return Ok(());
    }
    processes_keep_their_own_memory(memory, (2 * cpus).min(16))
}

/// The frame allocator's free frames, `None` if there is none.
fn free_frames() -> Option<u64> {
    memory::FRAMES
        .lock()
        .as_ref()
        .map(FrameAllocator::free_frames)
}

/// Where the kernel reaches the word at user address `virt` of `space` while it is active:
/// `virt` itself, through the MMU. Host tests have no MMU, so there it is the word in the
/// direct map of the frame `space`'s own tables map `virt` to (null if none).
fn user_word<P: Platform>(space: &AddressSpace<P>, virt: u64) -> *mut u64 {
    if cfg!(test) {
        space.translate(virt).map_or(0, |(phys, _)| {
            (phys as usize).wrapping_add(P::physical_memory_offset())
        }) as *mut u64
    } else {
        virt as *mut u64
    }
}

/// The kernel heap must hand out writable memory in the direct map, aligned as asked, without
/// overlap, from every size class and as blocks of whole frames, and give it all back: exactly
/// the whole-frame allocations return their frames. A `Vec` growing past several frames
/// (through `realloc`) must keep its contents. On the bare-metal targets the `Vec` uses the
/// kernel heap too, as the global allocator.
fn heap_allocates_in_the_direct_map<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    /// Every size class (16 to 2048 bytes), then whole frames: 1, 1, 1, 4, 2 and 16.
    const LAYOUTS: [(usize, usize); 14] = [
        (1, 1),
        (24, 8),
        (64, 64),
        (100, 8),
        (200, 8),
        (512, 512),
        (1000, 8),
        (2048, 16),
        (2049, 8),
        (4096, 4096),
        (4000, 8),
        (3 * 4096 + 1, 8),
        (8192, 8192),
        (65536, 8),
    ];
    const WHOLE_FRAMES: u64 = 25;
    let free_frames = || {
        memory::FRAMES
            .lock()
            .as_ref()
            .map(FrameAllocator::free_frames)
    };
    let mut taken = [core::ptr::null_mut::<u8>(); LAYOUTS.len()];
    for (slot, &(size, align)) in taken.iter_mut().zip(&LAYOUTS) {
        let layout = Layout::from_size_align(size, align).map_err(|_| "bad test layout")?;
        // SAFETY: the layout has a non-zero size.
        let pointer = unsafe { HEAP.alloc(layout) };
        if pointer.is_null() {
            return Err("the heap could not allocate");
        }
        *slot = pointer;
        let address = pointer as u64;
        let mapped = memory.page_tables.translate(address);
        if (pointer as usize) < P::direct_map_offset()
            || !address.is_multiple_of(align as u64)
            || !mapped.is_some_and(|(_, attributes)| attributes == Attributes::READ_WRITE)
        {
            return Err("heap memory is not aligned, writable memory in the direct map");
        }
    }
    // Each allocation filled with its own number, written only once all exist, so any overlap
    // shows.
    for (number, (&pointer, &(size, _))) in taken.iter().zip(&LAYOUTS).enumerate() {
        // SAFETY: `pointer` is a live allocation of `size` bytes.
        unsafe { pointer.write_bytes(number as u8 + 1, size) };
    }
    for (number, (&pointer, &(size, _))) in taken.iter().zip(&LAYOUTS).enumerate() {
        // SAFETY: each allocation is still owned and was written in full.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, size) };
        if bytes.iter().any(|&byte| byte != number as u8 + 1) {
            return Err("heap allocations overlap");
        }
    }
    let free_before = free_frames().ok_or("there is no frame allocator")?;
    for (&pointer, &(size, align)) in taken.iter().zip(&LAYOUTS) {
        let layout = Layout::from_size_align(size, align).map_err(|_| "bad test layout")?;
        // SAFETY: allocated above with this layout and no longer used.
        unsafe { HEAP.dealloc(pointer, layout) };
    }
    // The size classes keep the frames they carved; only the whole frames come back.
    if free_frames() != Some(free_before + WHOLE_FRAMES) {
        return Err("freeing did not give back exactly the whole-frame allocations' frames");
    }
    let mut numbers = Vec::new();
    for n in 0..5000u64 {
        numbers.push(n * n);
    }
    if numbers
        .iter()
        .enumerate()
        .any(|(n, &square)| square != (n * n) as u64)
    {
        return Err("a growing Vec lost its contents");
    }
    Ok(())
}

/// Two threads and the boot thread take turns in order through `yield_now`, each on its own
/// stack with its live values intact across every switch, and the threads' stacks are freed
/// once they have been joined (D35).
fn threads_take_turns_and_free_their_stacks<P: Platform>() -> Result<(), &'static str> {
    const TURNS: u64 = 3;
    static LOG: SpinLock<Vec<u64>> = SpinLock::new(Vec::new());
    /// Logs `thread`'s turns, checking that values live across each yield, in registers or on
    /// the stack, come back unchanged. A broken value logs `u64::MAX`.
    fn take_turns<P: Platform>(thread: u64) {
        for turn in 0..TURNS {
            let live = [1u64, 2, 3, 4, 5, 6, 7, 8].map(|n| core::hint::black_box(n * thread));
            LOG.lock().push(thread * 10 + turn);
            thread::yield_now::<P>();
            if core::hint::black_box(live) != [1u64, 2, 3, 4, 5, 6, 7, 8].map(|n| n * thread) {
                LOG.lock().push(u64::MAX);
            }
        }
    }
    let free_frames = || {
        memory::FRAMES
            .lock()
            .as_ref()
            .map(FrameAllocator::free_frames)
    };
    // Two threads that only exit, and room for the log, bring the heap's size classes for the
    // bookkeeping into use, so the frames counted below are only the stacks'.
    let warm_up = (thread::spawn::<P, _>(|| {}), thread::spawn::<P, _>(|| {}));
    let (Ok(first), Ok(second)) = warm_up else {
        return Err("could not start two threads");
    };
    first.join::<P>();
    second.join::<P>();
    {
        let mut log = LOG.lock();
        log.clear();
        log.reserve(4 * TURNS as usize);
    }
    let free_before = free_frames().ok_or("there is no frame allocator")?;
    let first = thread::spawn::<P, _>(|| take_turns::<P>(1));
    let second = thread::spawn::<P, _>(|| take_turns::<P>(2));
    let (Ok(first), Ok(second)) = (first, second) else {
        return Err("could not start two threads");
    };
    if free_frames() != Some(free_before - 2 * (1 << thread::STACK_ORDER)) {
        return Err("each thread did not take one stack's frames");
    }
    take_turns::<P>(3);
    first.join::<P>();
    second.join::<P>();
    if free_frames() != Some(free_before) {
        return Err("the joined threads' stacks were not freed");
    }
    let log = core::mem::take(&mut *LOG.lock());
    if log.contains(&u64::MAX) {
        return Err("values live across a switch changed (registers or stack not restored)");
    }
    // The boot thread (3) goes first: spawning does not switch.
    if log != [30, 10, 20, 31, 11, 21, 32, 12, 22] {
        return Err("the threads did not take turns in round-robin order");
    }
    Ok(())
}

/// With preemption on, a thread that never yields lets the others run once its time slice is
/// over: the first thread passes preemption points until the second has run, which only the
/// timer tick ending its slice allows (D37). It gives up after 2 s of clock time.
fn threads_are_preempted_at_preemption_points<P: Platform>() -> Result<(), &'static str> {
    static SECOND_RAN: AtomicBool = AtomicBool::new(false);
    static GAVE_UP: AtomicBool = AtomicBool::new(false);
    SECOND_RAN.store(false, Ordering::Relaxed);
    GAVE_UP.store(false, Ordering::Relaxed);
    let give_up = P::clock_ticks().saturating_add(2 * P::clock_frequency());
    let first = thread::spawn::<P, _>(move || {
        while !SECOND_RAN.load(Ordering::Relaxed) {
            thread::preemption_point::<P>();
            // Rechecked after the preemption point, which takes a tick already pending: a
            // host that stalled the machine past `give_up` does not fail the test.
            if !SECOND_RAN.load(Ordering::Relaxed) && P::clock_ticks() > give_up {
                GAVE_UP.store(true, Ordering::Relaxed);
                return;
            }
        }
    });
    let second = thread::spawn::<P, _>(|| SECOND_RAN.store(true, Ordering::Relaxed));
    let (Ok(first), Ok(second)) = (first, second) else {
        return Err("could not start two threads");
    };
    let ticks_before = interrupt::timer_interrupts::<P>();
    thread::enable_preemption::<P>();
    first.join::<P>();
    second.join::<P>();
    thread::disable_preemption::<P>();
    if GAVE_UP.load(Ordering::Relaxed) {
        return Err("a thread passing preemption points was not preempted within 2 s");
    }
    if !SECOND_RAN.load(Ordering::Relaxed) {
        return Err("the second thread never ran");
    }
    if interrupt::timer_interrupts::<P>() == ticks_before {
        return Err("a thread was preempted without a timer tick");
    }
    Ok(())
}

/// Threads that sleep wake in deadline order and never early, while the boot thread blocks in
/// `join` and the idle thread waits for the timer; a thread waiting in a wait queue runs once
/// its condition holds and it is woken; and with preemption on and one thread runnable, no
/// timer tick comes (D39). A sleeper that is never woken leaves the boot thread blocked, which
/// the QEMU runner's timeout reports.
fn threads_sleep_wait_and_run_tickless<P: Platform>() -> Result<(), &'static str> {
    static WOKEN: SpinLock<Vec<(u64, bool)>> = SpinLock::new(Vec::new());
    static QUEUE: thread::WaitQueue = thread::WaitQueue::new();
    static OPEN: AtomicBool = AtomicBool::new(false);
    let millisecond = P::clock_frequency() / 1000;
    WOKEN.lock().clear();
    let base = P::clock_ticks();
    let mut sleepers = Vec::new();
    for order in [3u64, 1, 2] {
        let sleeper = thread::spawn::<P, _>(move || {
            let deadline = base + order * millisecond;
            thread::sleep_until::<P>(deadline);
            let on_time = P::clock_ticks() >= deadline;
            WOKEN.lock().push((order, on_time));
        });
        sleepers.push(sleeper.map_err(|_| "could not start three threads")?);
    }
    for sleeper in sleepers {
        sleeper.join::<P>();
    }
    let woken = core::mem::take(&mut *WOKEN.lock());
    if woken.iter().any(|&(_, on_time)| !on_time) {
        return Err("a sleeping thread woke before its deadline");
    }
    if woken != [(1, true), (2, true), (3, true)] {
        return Err("sleeping threads did not wake in deadline order");
    }

    OPEN.store(false, Ordering::Relaxed);
    let waiter = thread::spawn::<P, _>(|| QUEUE.wait_until::<P>(|| OPEN.load(Ordering::Acquire)))
        .map_err(|_| "could not start a thread")?;
    // The waiter runs, finds the queue closed and waits.
    thread::yield_now::<P>();
    if waiter.is_finished() {
        return Err("a thread did not wait for its condition");
    }
    OPEN.store(true, Ordering::Release);
    if !QUEUE.wake_one::<P>() {
        return Err("the waiting thread was not in the wait queue");
    }
    waiter.join::<P>();

    let interrupts_before = interrupt::timer_interrupts::<P>();
    thread::enable_preemption::<P>();
    let until = P::clock_ticks() + 3 * thread::TIME_SLICE_MS * millisecond;
    while P::clock_ticks() < until {
        thread::preemption_point::<P>();
    }
    thread::disable_preemption::<P>();
    if interrupt::timer_interrupts::<P>() != interrupts_before {
        return Err("a timer tick came while only one thread was runnable");
    }
    Ok(())
}

/// The boot CPU is CPU 0, and every other CPU the firmware lists starts on the kernel's page
/// tables and reports in through its own per-CPU block, which only works if its per-CPU
/// register points at that block (D40). In the QEMU boot tests the firmware must list every CPU
/// QEMU runs. A CPU that never reports in fails after the timeout.
fn secondary_cpus_start_and_report_in<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    if cpu::current::<P>().index() != 0 {
        return Err("the boot CPU's per-CPU block is not CPU 0's");
    }
    // SAFETY: these are the kernel's page tables, active since `paging::enable`, whose frames
    // are never freed; nothing changes them from here on.
    let started = unsafe { cpu::start_secondaries::<P>(memory.page_tables.roots()) }
        .map_err(|_| "a CPU could not be started")?;
    let expected = started.started + 1;
    let deadline = P::clock_ticks() + CPU_START_TIMEOUT_MS * (P::clock_frequency() / 1000);
    while cpu::CPUS.online() < expected && P::clock_ticks() < deadline {
        core::hint::spin_loop();
    }
    let cpus = cpu::CPUS.all();
    if cpu::CPUS.online() != expected || !cpus.iter().all(|cpu| cpu.is_online()) {
        return Err("a started CPU did not report in");
    }
    if cpus
        .iter()
        .enumerate()
        .any(|(index, cpu)| cpu.index() != index)
    {
        return Err("the CPUs are not numbered in order");
    }
    #[cfg(all(feature = "qemu-test", not(feature = "qemu-bench")))]
    if cpus.len() != QEMU_CPUS {
        return Err("the firmware does not list every CPU QEMU runs");
    }
    Ok(())
}

/// Every CPU takes its own interrupts (D41): each secondary CPU took its own timer interrupt
/// before it reported in, and takes the wake-up interrupt the boot CPU sends it, counted in its
/// own block and in no other CPU's. Fails if a CPU's interrupt controller or timer is not set up,
/// or a wake-up goes to the wrong CPU (both checked by breaking them). Runs once the CPUs have
/// started; on one CPU (the benchmarks) it has nothing to check, and the QEMU boot tests' four
/// CPUs are checked by `secondary_cpus_start_and_report_in`.
fn cpus_take_their_own_interrupts<P: Platform>() -> Result<(), &'static str> {
    let cpus = cpu::CPUS.all();
    let Some((boot, secondaries)) = cpus.split_first() else {
        return Err("the CPUs have not started");
    };
    if secondaries.iter().any(|cpu| cpu.timer_interrupts() == 0) {
        return Err("a CPU reported in without taking its timer interrupt");
    }
    let wakeups = |cpus: &[&cpu::Cpu]| cpus.iter().map(|cpu| cpu.wakeups()).sum::<u64>();
    let boot_before = boot.wakeups();
    for (index, &cpu) in secondaries.iter().enumerate() {
        let before = cpu.wakeups();
        let others_before = wakeups(&secondaries[..index]) + wakeups(&secondaries[index + 1..]);
        cpu::wake::<P>(cpu);
        let deadline = P::clock_ticks() + WAKEUP_TIMEOUT_MS * (P::clock_frequency() / 1000);
        while cpu.wakeups() == before && P::clock_ticks() < deadline {
            core::hint::spin_loop();
        }
        if cpu.wakeups() != before + 1 {
            return Err("a CPU did not take the wake-up interrupt sent to it");
        }
        let others = wakeups(&secondaries[..index]) + wakeups(&secondaries[index + 1..]);
        if others != others_before || boot.wakeups() != boot_before {
            return Err("a wake-up interrupt reached a CPU it was not sent to");
        }
    }
    Ok(())
}

/// Threads run on every CPU at once (D43): one thread per secondary CPU, each placed on an idle
/// CPU of its own, all running at the same time (each waits until every CPU's thread, the boot
/// thread included, has arrived). Each then sleeps on its own CPU's timer and waits in a wait
/// queue until the boot thread wakes them all from afar, which takes the wake-up interrupt to
/// their idle CPUs; each must still run on its own CPU. They sleep once more before they exit,
/// so the boot thread blocks in `join` and is woken from afar too. Up to 64 CPUs take part; on
/// one CPU (the benchmarks) there is nothing to check. A thread that is never woken leaves the
/// boot thread blocked, which the QEMU runner's timeout reports.
fn threads_run_on_every_cpu<P: Platform>() -> Result<(), &'static str> {
    static ARRIVED: AtomicUsize = AtomicUsize::new(0);
    static RAN_ON: AtomicU64 = AtomicU64::new(0);
    static WAITING: AtomicUsize = AtomicUsize::new(0);
    static OPEN: AtomicBool = AtomicBool::new(false);
    static WOKE_EARLY: AtomicBool = AtomicBool::new(false);
    static MOVED: AtomicBool = AtomicBool::new(false);
    static QUEUE: thread::WaitQueue = thread::WaitQueue::new();
    let cpus = cpu::CPUS.all();
    let taking_part = cpus.len().min(64);
    if taking_part <= 1 {
        return Ok(());
    }
    ARRIVED.store(0, Ordering::Relaxed);
    RAN_ON.store(0, Ordering::Relaxed);
    WAITING.store(0, Ordering::Relaxed);
    OPEN.store(false, Ordering::Relaxed);
    WOKE_EARLY.store(false, Ordering::Relaxed);
    MOVED.store(false, Ordering::Relaxed);
    let millisecond = P::clock_frequency() / 1000;
    let give_up = P::clock_ticks() + THREADS_MEET_TIMEOUT_MS * millisecond;
    let arrive = move || {
        let here = cpu::current::<P>();
        RAN_ON.fetch_or(1 << here.index(), Ordering::Relaxed);
        ARRIVED.fetch_add(1, Ordering::AcqRel);
        while ARRIVED.load(Ordering::Acquire) < taking_part && P::clock_ticks() < give_up {
            core::hint::spin_loop();
        }
        here
    };
    let boot = cpus[0];
    let boot_wakeups = boot.wakeups();
    let mut threads = Vec::new();
    for _ in 1..taking_part {
        let spawned = thread::spawn::<P, _>(move || {
            let here = arrive();
            let deadline = P::clock_ticks() + millisecond;
            thread::sleep_until::<P>(deadline);
            if P::clock_ticks() < deadline {
                WOKE_EARLY.store(true, Ordering::Relaxed);
            }
            WAITING.fetch_add(1, Ordering::Release);
            QUEUE.wait_until::<P>(|| OPEN.load(Ordering::Acquire));
            // Long enough for the boot thread to block in `join`.
            thread::sleep_until::<P>(P::clock_ticks() + millisecond);
            if !core::ptr::eq(cpu::current::<P>(), here) {
                MOVED.store(true, Ordering::Relaxed);
            }
        });
        threads.push(spawned.map_err(|_| "could not start a thread per CPU")?);
    }
    arrive();
    let all_at_once = ARRIVED.load(Ordering::Acquire) == taking_part;
    while WAITING.load(Ordering::Acquire) < taking_part - 1 && P::clock_ticks() < give_up {
        core::hint::spin_loop();
    }
    // Long enough for the threads to block in the queue, so the wake comes from afar.
    thread::sleep_until::<P>(P::clock_ticks() + millisecond);
    let wakeups_before: Vec<u64> = cpus[1..taking_part]
        .iter()
        .map(|cpu| cpu.wakeups())
        .collect();
    OPEN.store(true, Ordering::Release);
    let woken = QUEUE.wake_all::<P>();
    for thread in threads {
        thread.join::<P>();
    }
    if !all_at_once {
        return Err("the threads placed on the CPUs did not all run at once");
    }
    if RAN_ON.load(Ordering::Relaxed) != u64::MAX >> (64 - taking_part) {
        return Err("the threads were not placed one on each CPU");
    }
    if WOKE_EARLY.load(Ordering::Relaxed) {
        return Err("a thread on another CPU woke before its deadline");
    }
    if woken != taking_part - 1 {
        return Err("a thread on another CPU was not waiting in the wait queue");
    }
    if MOVED.load(Ordering::Relaxed) {
        return Err("a thread woken from afar ran on another CPU");
    }
    if cpus[1..taking_part]
        .iter()
        .zip(wakeups_before)
        .any(|(cpu, before)| cpu.wakeups() == before)
    {
        return Err("an idle CPU was not sent the wake-up interrupt for its thread");
    }
    if boot.wakeups() == boot_wakeups {
        return Err("the boot CPU was not woken from afar when the threads it joined exited");
    }
    Ok(())
}

/// Threads move to idle CPUs (D44). A thread placed on an idle CPU sleeps there; meanwhile a
/// second thread is placed on that CPU, idle again, and keeps it busy at preemption points with
/// preemption on. The sleeper then wakes onto the busy CPU while the other CPUs are idle: when
/// the slice ends, an idle CPU is woken and takes whichever of the two is ready, so they run side
/// by side on two CPUs (each sees the other on a CPU other than its own). Without balancing they
/// would take turns on one CPU until the timeout. On one CPU (the benchmarks) there is nothing to
/// check.
fn threads_move_to_idle_cpus<P: Platform>() -> Result<(), &'static str> {
    /// Each thread's CPU, as its index plus one; 0 before it runs.
    static SLEEPER_ON: AtomicUsize = AtomicUsize::new(0);
    static BUSY_ON: AtomicUsize = AtomicUsize::new(0);
    static ASLEEP: AtomicBool = AtomicBool::new(false);
    static PLACED_APART: AtomicBool = AtomicBool::new(false);
    static GAVE_UP: AtomicBool = AtomicBool::new(false);
    if cpu::CPUS.all().len() <= 1 {
        return Ok(());
    }
    SLEEPER_ON.store(0, Ordering::Relaxed);
    BUSY_ON.store(0, Ordering::Relaxed);
    ASLEEP.store(false, Ordering::Relaxed);
    PLACED_APART.store(false, Ordering::Relaxed);
    GAVE_UP.store(false, Ordering::Relaxed);
    let millisecond = P::clock_frequency() / 1000;
    let give_up = P::clock_ticks() + THREADS_SPREAD_TIMEOUT_MS * millisecond;
    // Runs at preemption points until it and `other` are seen on two CPUs.
    let side_by_side = move |mine: &AtomicUsize, other: &AtomicUsize| loop {
        let here = cpu::current::<P>().index() + 1;
        mine.store(here, Ordering::Relaxed);
        let there = other.load(Ordering::Relaxed);
        if there != 0 && there != here {
            return;
        }
        if P::clock_ticks() > give_up {
            GAVE_UP.store(true, Ordering::Relaxed);
            return;
        }
        thread::preemption_point::<P>();
    };
    thread::enable_preemption::<P>();
    let sleeper = thread::spawn::<P, _>(move || {
        let before = cpu::current::<P>().index() + 1;
        SLEEPER_ON.store(before, Ordering::Relaxed);
        ASLEEP.store(true, Ordering::Release);
        thread::sleep_until::<P>(P::clock_ticks() + 20 * millisecond);
        side_by_side(&SLEEPER_ON, &BUSY_ON);
    });
    let Ok(sleeper) = sleeper else {
        thread::disable_preemption::<P>();
        return Err("could not start a thread");
    };
    while !ASLEEP.load(Ordering::Acquire) && P::clock_ticks() < give_up {
        core::hint::spin_loop();
    }
    // Long enough for the sleeper to block, leaving its CPU idle, the lowest-numbered one, with
    // a margin for a host that stalls the machine: the sleeper sleeps four times as long.
    thread::sleep_until::<P>(P::clock_ticks() + 5 * millisecond);
    let sleeper_on = SLEEPER_ON.load(Ordering::Relaxed);
    let busy = thread::spawn::<P, _>(move || {
        if cpu::current::<P>().index() + 1 != sleeper_on {
            PLACED_APART.store(true, Ordering::Relaxed);
        }
        side_by_side(&BUSY_ON, &SLEEPER_ON);
    });
    let Ok(busy) = busy else {
        thread::disable_preemption::<P>();
        sleeper.join::<P>();
        return Err("could not start a second thread");
    };
    sleeper.join::<P>();
    busy.join::<P>();
    thread::disable_preemption::<P>();
    if PLACED_APART.load(Ordering::Relaxed) {
        return Err("the second thread was not placed on the sleeping thread's idle CPU");
    }
    if GAVE_UP.load(Ordering::Relaxed) {
        return Err("a thread made ready on a busy CPU did not move to an idle one within 1 s");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passes(_: &mut ()) -> Result<(), &'static str> {
        Ok(())
    }
    fn fails(_: &mut ()) -> Result<(), &'static str> {
        Err("broken")
    }

    #[test]
    fn all_passing_prints_each_result_and_pass_marker() {
        let mut out = String::new();
        assert_eq!(
            run(&mut out, &[("a", passes), ("b", passes)], &mut ()),
            Ok(true)
        );
        assert_eq!(
            out,
            format!("selftest a ... ok\nselftest b ... ok\n{PASS_MARKER}\n")
        );
    }

    #[test]
    fn one_failure_runs_the_rest_and_prints_fail_marker() {
        let mut out = String::new();
        assert_eq!(
            run(&mut out, &[("a", fails), ("b", passes)], &mut ()),
            Ok(false)
        );
        assert_eq!(
            out,
            format!("selftest a ... FAILED: broken\nselftest b ... ok\n{FAIL_MARKER}\n")
        );
    }

    /// A platform whose breakpoint never reaches the kernel's handler.
    struct LostBreakpoints;

    impl Platform for LostBreakpoints {
        const ARCH: &'static str = "lost";
        fn console_write(_bytes: &[u8]) {}
        fn exit(_status: eggos_hal::ExitStatus) -> ! {
            unreachable!("not used in tests")
        }
        fn halt() -> ! {
            unreachable!("not used in tests")
        }
        fn breakpoint() {}
        fn execute_illegal_instruction() -> ! {
            unreachable!("not used in tests")
        }
        fn clock_ticks() -> u64 {
            0
        }
        fn clock_frequency() -> u64 {
            1
        }
        fn set_timer_deadline(_deadline: u64) {}
        fn wait_for_interrupt() {}
        fn interrupt_window() {}
        fn memory_map() -> eggos_hal::MemoryMap {
            eggos_hal::MemoryMap::new()
        }
        fn physical_memory_offset() -> usize {
            0
        }
        type PageTableFormat = eggos_hal::paging::x86_64::X86_64;
        fn direct_map_offset() -> usize {
            0
        }
        fn kernel_image() -> eggos_hal::paging::KernelImage {
            unreachable!("not used in tests")
        }
        fn physical_memory() -> eggos_hal::MemoryMap {
            eggos_hal::MemoryMap::new()
        }
        fn early_device_regions(_region: &mut dyn FnMut(eggos_hal::PhysicalRange)) {}
        unsafe fn activate_page_tables(_roots: &[u64]) {
            unreachable!("not used in tests")
        }
        unsafe fn switch_address_space(_roots: &[u64]) {
            unreachable!("not used in tests")
        }
        fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
            access()
        }
        unsafe fn run_user(
            _context: &mut eggos_hal::user::UserContext,
        ) -> eggos_hal::user::UserExit {
            unreachable!()
        }
        fn user_test_program() -> eggos_hal::user::TestProgram {
            unreachable!()
        }
        fn make_instructions_visible(_start: usize, _length: usize) {}
        unsafe fn prepare_thread(
            _stack_top: usize,
            _entry: extern "C" fn(usize) -> !,
            _argument: usize,
        ) -> usize {
            unreachable!("not used in tests")
        }
        unsafe fn switch_thread(_save: *mut usize, _resume: usize) {
            unreachable!("not used in tests")
        }
        fn cpus(_cpu: &mut dyn FnMut(u64)) {
            unreachable!("not used in tests")
        }
        fn boot_cpu() -> u64 {
            unreachable!("not used in tests")
        }
        unsafe fn start_cpu(
            _cpu: u64,
            _start: &eggos_hal::cpu::CpuStart,
        ) -> Result<(), eggos_hal::cpu::StartCpuError> {
            unreachable!("not used in tests")
        }
        unsafe fn set_cpu_local(_block: usize) {
            unreachable!("not used in tests")
        }
        fn cpu_local() -> usize {
            unreachable!("not used in tests")
        }
        fn wake_cpu(_cpu: u64) {
            unreachable!("not used in tests")
        }
    }

    #[test]
    fn breakpoint_that_never_reaches_the_handler_fails() {
        assert!(breakpoint_resumes::<LostBreakpoints>().is_err());
    }

    #[test]
    fn clock_too_slow_to_time_a_millisecond_fails() {
        assert_eq!(
            timer_interrupt_fires::<LostBreakpoints>(),
            Err("the clock runs slower than 1 kHz")
        );
    }

    /// A platform whose clock runs (100 ms per read) but whose timer never interrupts.
    struct DeadTimer;

    static DEAD_TIMER_CLOCK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    impl Platform for DeadTimer {
        const ARCH: &'static str = "dead-timer";
        fn console_write(_bytes: &[u8]) {}
        fn exit(_status: eggos_hal::ExitStatus) -> ! {
            unreachable!("not used in tests")
        }
        fn halt() -> ! {
            unreachable!("not used in tests")
        }
        fn breakpoint() {}
        fn execute_illegal_instruction() -> ! {
            unreachable!("not used in tests")
        }
        fn clock_ticks() -> u64 {
            DEAD_TIMER_CLOCK.fetch_add(100, core::sync::atomic::Ordering::Relaxed)
        }
        fn clock_frequency() -> u64 {
            1000
        }
        fn set_timer_deadline(_deadline: u64) {}
        fn wait_for_interrupt() {}
        fn interrupt_window() {}
        fn memory_map() -> eggos_hal::MemoryMap {
            eggos_hal::MemoryMap::new()
        }
        fn physical_memory_offset() -> usize {
            0
        }
        type PageTableFormat = eggos_hal::paging::x86_64::X86_64;
        fn direct_map_offset() -> usize {
            0
        }
        fn kernel_image() -> eggos_hal::paging::KernelImage {
            unreachable!("not used in tests")
        }
        fn physical_memory() -> eggos_hal::MemoryMap {
            eggos_hal::MemoryMap::new()
        }
        fn early_device_regions(_region: &mut dyn FnMut(eggos_hal::PhysicalRange)) {}
        unsafe fn activate_page_tables(_roots: &[u64]) {
            unreachable!("not used in tests")
        }
        unsafe fn switch_address_space(_roots: &[u64]) {
            unreachable!("not used in tests")
        }
        fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
            access()
        }
        unsafe fn run_user(
            _context: &mut eggos_hal::user::UserContext,
        ) -> eggos_hal::user::UserExit {
            unreachable!()
        }
        fn user_test_program() -> eggos_hal::user::TestProgram {
            unreachable!()
        }
        fn make_instructions_visible(_start: usize, _length: usize) {}
        unsafe fn prepare_thread(
            _stack_top: usize,
            _entry: extern "C" fn(usize) -> !,
            _argument: usize,
        ) -> usize {
            unreachable!("not used in tests")
        }
        unsafe fn switch_thread(_save: *mut usize, _resume: usize) {
            unreachable!("not used in tests")
        }
        fn cpus(_cpu: &mut dyn FnMut(u64)) {
            unreachable!("not used in tests")
        }
        fn boot_cpu() -> u64 {
            unreachable!("not used in tests")
        }
        unsafe fn start_cpu(
            _cpu: u64,
            _start: &eggos_hal::cpu::CpuStart,
        ) -> Result<(), eggos_hal::cpu::StartCpuError> {
            unreachable!("not used in tests")
        }
        unsafe fn set_cpu_local(block: usize) {
            // SAFETY: the caller's guarantees are the test platform's.
            unsafe { crate::tests::TestPlatform::set_cpu_local(block) }
        }
        fn cpu_local() -> usize {
            // The test machine's one CPU.
            crate::tests::TestPlatform::cpu_local()
        }
        fn wake_cpu(_cpu: u64) {
            unreachable!("not used in tests")
        }
    }

    #[test]
    fn timer_that_never_fires_fails_after_a_second() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        assert_eq!(
            timer_interrupt_fires::<DeadTimer>(),
            Err("no timer interrupt within 1 s of its deadline")
        );
    }

    #[test]
    fn no_free_memory_fails() {
        // One frame of RAM, which the page tables' root takes.
        let memory = vec![0u8; 2 * 4096].leak();
        let start = (memory.as_mut_ptr() as usize).next_multiple_of(4096);
        let mut map = eggos_hal::MemoryMap::new();
        map.add_ram(eggos_hal::PhysicalRange::from_start_length(0x1000, 0x1000));
        let mut frames = FrameAllocator::new(&map, start - 0x1000);
        crate::paging::PageTables::<eggos_hal::paging::x86_64::X86_64>::new(&mut frames).unwrap();
        assert_eq!(
            every_frame_is_usable_ram(&mut frames),
            Err("the boot memory map has no free memory")
        );
    }

    #[test]
    fn bss_probe_is_zero_on_host() {
        assert_eq!(bss_is_zeroed(), Ok(()));
    }
}
