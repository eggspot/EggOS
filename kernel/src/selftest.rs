//! Boot-time self-tests: checks that only mean something on the real (or emulated) machine,
//! such as "did the architecture entry point zero `.bss`". The QEMU test runner reads the
//! verdict markers from the console.

use crate::HEAP;
use crate::address_space::AddressSpace;
use crate::elf::{self, ProgramHeader, segment};
use crate::linux::exec;
use crate::memory::{self, FrameAllocator, MAX_ORDER, Memory};
use crate::process::{self, Process, ProcessExit};
use crate::sync::SpinLock;
use crate::{console, cpu, exception, interrupt, linux, random, thread};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering, fence};
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format, MemoryType};
use eggos_hal::random::SeedSource;
use eggos_hal::time::mul_div_ceil;
use eggos_hal::user::{TestProgram, UserContext, UserExit};
use eggos_hal::{ExceptionKind, Platform};

/// Printed when every self-test passed. The QEMU runner requires this exact line.
pub const PASS_MARKER: &str = "EGGOS-SELFTEST: PASS";
/// Printed when a self-test failed or the kernel panicked.
pub const FAIL_MARKER: &str = "EGGOS-SELFTEST: FAIL";
/// Printed, followed by the address, before the `qemu-test-fatal-exception` kernel writes its
/// read-only data. The QEMU runner requires the fault report to name that address.
pub const WRITE_PROTECT_PROBE: &str = "fatal exception test: writing read-only data at ";

/// What the QEMU runner types on the serial console once the kernel writes
/// [`console::INPUT_READY_LINE`], ending in the carriage return a terminal sends for Enter.
/// Must match the runner's.
pub const CONSOLE_INPUT_SCRIPT: &[u8] = b"EggOS reads its console\r";

const CONSOLE_INPUT_LINE: &[u8] = b"EggOS reads its console\n";

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
pub fn all<P: Platform>() -> [SelfTest<Memory<P>>; 47] {
    [
        ("bss_is_zeroed", |_| bss_is_zeroed()),
        ("breakpoint_resumes", |_| breakpoint_resumes::<P>()),
        ("timer_interrupt_fires", |_| timer_interrupt_fires::<P>()),
        ("console_input_arrives_by_interrupt", |_| {
            console_input_arrives_by_interrupt::<P>()
        }),
        // Before the secondary CPUs start: it asks the HAL for seeds again.
        ("random_generator_is_seeded", |_| {
            random_generator_is_seeded::<P>()
        }),
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
        ("futex_waiters_wake_in_order_and_time_out", |_| {
            futex_waiters_wake_in_order_and_time_out::<P>()
        }),
        ("futex_waiters_are_requeued_in_order", |_| {
            futex_waiters_are_requeued_in_order::<P>()
        }),
        (
            "processes_keep_their_own_memory_across_switches",
            |memory| processes_keep_their_own_memory::<P>(memory, 2, 1),
        ),
        (
            "processes_keep_their_fp_registers_across_switches",
            |memory| processes_keep_their_fp_registers::<P>(memory, 2),
        ),
        (
            "processes_are_preempted_on_return_to_user_mode",
            processes_are_preempted_on_return_to_user_mode::<P>,
        ),
        (
            "linux_programs_write_and_exit",
            linux_programs_write_and_exit::<P>,
        ),
        (
            "linux_programs_start_from_elf_images",
            linux_programs_start_from_elf_images::<P>,
        ),
        (
            "linux_stacks_grow_on_demand",
            linux_stacks_grow_on_demand::<P>,
        ),
        (
            "linux_memory_calls_take_effect_at_once",
            linux_memory_calls_take_effect_at_once::<P>,
        ),
        // Before the secondary CPUs start: every program on one CPU.
        ("linux_programs_keep_their_thread_pointers", |memory| {
            linux_programs_keep_their_thread_pointers::<P>(memory, 3)
        }),
        (
            "linux_programs_poll_the_console",
            linux_programs_poll_the_console::<P>,
        ),
        (
            "linux_programs_signal_themselves",
            linux_programs_signal_themselves::<P>,
        ),
        (
            "linux_programs_run_signal_handlers",
            linux_programs_run_signal_handlers::<P>,
        ),
        (
            "linux_programs_handle_their_faults",
            linux_programs_handle_their_faults::<P>,
        ),
        (
            "linux_programs_queue_signals_with_their_info",
            linux_programs_queue_signals_with_their_info::<P>,
        ),
        (
            "linux_programs_wait_for_signals_with_sigsuspend",
            linux_programs_wait_for_signals_with_sigsuspend::<P>,
        ),
        (
            "linux_programs_read_clocks_and_sleep",
            linux_programs_read_clocks_and_sleep::<P>,
        ),
        (
            "linux_programs_read_the_console_s_status",
            linux_programs_read_the_console_s_status::<P>,
        ),
        (
            "linux_programs_change_their_credentials",
            linux_programs_change_their_credentials::<P>,
        ),
        (
            "linux_programs_move_and_discard_memory",
            linux_programs_move_and_discard_memory::<P>,
        ),
        (
            "linux_programs_built_from_rust_source_start",
            linux_programs_built_from_rust_source_start::<P>,
        ),
        (
            "linux_programs_built_with_rust_std_run",
            linux_programs_built_with_rust_std_run::<P>,
        ),
        (
            "linux_programs_read_the_console_through_its_line_discipline",
            linux_programs_read_the_console_through_its_line_discipline::<P>,
        ),
        (
            "linux_programs_end_from_a_thread_while_others_wait",
            linux_programs_end_from_a_thread_while_others_wait::<P>,
        ),
        (
            "linux_programs_start_programs_from_files",
            linux_programs_start_programs_from_files::<P>,
        ),
        (
            "linux_programs_create_processes",
            linux_programs_create_processes::<P>,
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
        (
            "linux_programs_with_threads_run_on_every_cpu",
            linux_programs_with_threads_run_on_every_cpu::<P>,
        ),
        (
            "linux_threads_stop_using_a_page_unmapped_on_another_cpu",
            linux_threads_stop_using_a_page_unmapped_on_another_cpu::<P>,
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

/// The random number generator was seeded at boot (D56): every QEMU machine the tests boot
/// offers a seed (the device tree's `rng-seed`, or `RDSEED`). Its draws differ, and the
/// device tree's seeds are gone once taken, so asking the HAL again finds none.
fn random_generator_is_seeded<P: Platform>() -> Result<(), &'static str> {
    if !random::is_seeded() {
        return Err("the random number generator was not seeded: the machine offered no seed");
    }
    let mut first = [0u8; 32];
    let mut second = [0u8; 32];
    random::fill::<P>(&mut first);
    random::fill::<P>(&mut second);
    if first == second || first == [0; 32] {
        return Err("two draws from the random number generator were the same");
    }
    let mut device_tree_seeds = 0;
    P::random_seeds(&mut |source, _| {
        if source != SeedSource::CpuInstruction {
            device_tree_seeds += 1;
        }
    });
    if device_tree_seeds > 0 {
        return Err("the device tree still holds a seed the generator took");
    }
    Ok(())
}

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

/// The bytes the QEMU runner types after [`console::INPUT_READY_LINE`] must reach
/// [`console::read_input`] through the console's receive interrupt, the only path that gives
/// the line discipline input, whole and in order, as one line ([`CONSOLE_INPUT_LINE`]). Waits
/// up to 2 s of clock time, taking interrupts without sleeping, so a clock that runs ahead
/// while the CPU sleeps (QEMU's `-icount sleep=off`) still leaves the host time to type. Only
/// kernels built for the boot tests (`qemu-test`) and host tests check it: elsewhere nobody
/// types the script, and the boot must not wait for it.
fn console_input_arrives_by_interrupt<P: Platform>() -> Result<(), &'static str> {
    if !cfg!(any(test, feature = "qemu-test")) {
        return Ok(());
    }
    let give_up = P::clock_ticks() + 2 * P::clock_frequency();
    let mut received = [0u8; CONSOLE_INPUT_LINE.len() + 1];
    let mut length = 0;
    while length < CONSOLE_INPUT_LINE.len() {
        if P::clock_ticks() > give_up {
            return Err("the console's input did not arrive within 2 s");
        }
        P::interrupt_window();
        length += console::read_input(&mut received[length..]);
    }
    if received[..length] != *CONSOLE_INPUT_LINE {
        return Err("the console's input arrived changed, out of order or with extra bytes");
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
/// without stopping the kernel; a floating-point instruction must trap (D49); a timer interrupt
/// must take the CPU back from a program that never makes a system call. Everything the program
/// took must be freed afterwards (D47).
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
        let mut context = Process::<P>::test_program_context(|program| program.floating_point);
        let floating_point = context.pc();
        // SAFETY: as above.
        match unsafe { P::run_user(&mut context) } {
            UserExit::Exception(exception)
                if exception.kind == ExceptionKind::IllegalInstruction
                    && exception.pc == floating_point => {}
            UserExit::Exception(_) => {
                return Err("the program's floating-point instruction was misreported");
            }
            _ => return Err("a user program ran a floating-point instruction"),
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
/// `count` processes' threads, `threads_each` per process, make system calls, and at each the
/// handler yields, so the others run (and load their address spaces) on its CPU, then reads the
/// process's data page through [`Process::with_user_memory`], which must load its own again.
/// After the threads end, every frame the processes took must be free and no CPU may keep an
/// address space loaded, even one that several CPUs had loaded at once. The processes run
/// twice, and everything the second round took must be freed: the first may grow the heap's
/// size classes, whose frames stay with them (D30).
fn processes_keep_their_own_memory<P: Platform>(
    memory: &mut Memory<P>,
    count: usize,
    threads_each: usize,
) -> Result<(), &'static str> {
    run_processes_reading_their_own_memory(memory, count, threads_each)?;
    if !release_loaded_address_spaces::<P>(|| true) {
        return Err("a CPU kept an ended process's address space loaded");
    }
    let before = free_and_slab_frames().ok_or("there is no frame allocator")?;
    run_processes_reading_their_own_memory(memory, count, threads_each)?;
    let freed = || free_and_slab_frames() == Some(before);
    if !release_loaded_address_spaces::<P>(freed) {
        return Err("a CPU kept an ended process's address space loaded");
    }
    if !freed() {
        return Err("the processes' frames and tables were not all freed");
    }
    Ok(())
}

/// One round of [`processes_keep_their_own_memory`]: the processes' threads, until they end.
fn run_processes_reading_their_own_memory<P: Platform>(
    memory: &mut Memory<P>,
    count: usize,
    threads_each: usize,
) -> Result<(), &'static str> {
    const CALLS: u64 = 20;
    /// A read that did not show the process's own page, or an exception.
    const WRONG: u64 = u64::MAX;
    let processes = processes_with_data(memory, count)?;
    let mut threads = Vec::new();
    let threads_of = processes
        .into_iter()
        .enumerate()
        .flat_map(|(number, process)| (0..threads_each).map(move |_| (number, process.clone())));
    for (number, process) in threads_of {
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
    result
}

/// Programs must each keep their own FP/SIMD registers, whatever ran in between (D54): `count`
/// processes run [`TestProgram::fp_registers`], each with a seed of its own, and at each of its
/// null system calls the handler yields, so the others run (and load their registers) on its
/// CPU. Each must start with a new program's registers, all zero, though another's are in the
/// CPU's, and find its own values unchanged after every call.
fn processes_keep_their_fp_registers<P: Platform>(
    memory: &mut Memory<P>,
    count: usize,
) -> Result<(), &'static str> {
    const CALLS: u64 = 20;
    /// An exit other than the program's system calls, or a report of changed registers.
    const WRONG: u64 = u64::MAX;
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    if programs_see_xsave_or_protection_keys() {
        return Err("programs see XSAVE or protection keys, whose state is never switched");
    }
    let mut threads = Vec::new();
    for number in 0..count {
        // SAFETY: as in `processes_with_data`.
        let process = unsafe { Process::<P>::with_test_program(&memory.page_tables) }?;
        let seed = 0x5eed_0000_0000 + (number as u64) * 0x101;
        let result = Arc::new(AtomicU64::new(WRONG));
        let reported = result.clone();
        let mut calls = 0;
        let context = Process::<P>::test_program_context(|program| program.fp_registers);
        let thread = process::spawn::<P, _>(Arc::new(process), context, move |_, context, exit| {
            match exit {
                UserExit::Interrupt => return ControlFlow::Continue(()),
                UserExit::Syscall => {}
                UserExit::Exception(exception) if process::first_fp_use::<P>(exception.kind) => {
                    return ControlFlow::Continue(());
                }
                UserExit::Exception(_) => return ControlFlow::Break(()),
            }
            match context.syscall_number() {
                TestProgram::SYSCALL_FIRST => context.set_syscall_result(seed),
                TestProgram::SYSCALL_NULL => {
                    thread::yield_now::<P>();
                    calls += 1;
                    context.set_syscall_result(u64::from(calls >= CALLS));
                }
                _ => {
                    let differences = context.syscall_arguments()[0];
                    reported.store(
                        if calls == CALLS { differences } else { WRONG },
                        Ordering::Relaxed,
                    );
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        });
        threads.push((
            thread.map_err(|_| "could not start a process's thread")?,
            result,
        ));
    }
    let mut result = Ok(());
    for (thread, differences) in threads {
        thread.join::<P>();
        match differences.load(Ordering::Relaxed) {
            0 => {}
            WRONG => result = Err("an FP/SIMD program faulted or ended early"),
            _ => result = Err("a program's FP/SIMD registers changed, or did not start at zero"),
        }
    }
    result
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn programs_see_xsave_or_protection_keys() -> bool {
    use core::arch::x86_64::{__cpuid, __cpuid_count};
    const CPUID_1_ECX_OSXSAVE: u32 = 1 << 27;
    const CPUID_7_ECX_OSPKE: u32 = 1 << 4;
    __cpuid(1).ecx & CPUID_1_ECX_OSXSAVE != 0
        || (__cpuid(0).eax >= 7 && __cpuid_count(7, 0).ecx & CPUID_7_ECX_OSPKE != 0)
}

/// Has every other CPU's idle thread look again until none keeps an ended process's address
/// space loaded (D48) and `freed` holds, waking them each millisecond, for up to
/// [`THREADS_MEET_TIMEOUT_MS`]. Returns whether every CPU, this one included, has none loaded;
/// if so, everything they let go is freed, as a CPU records none only after that (D81).
fn release_loaded_address_spaces<P: Platform>(freed: impl Fn() -> bool) -> bool {
    let here = cpu::current::<P>();
    let millisecond = P::clock_frequency() / 1000;
    let give_up = P::clock_ticks() + THREADS_MEET_TIMEOUT_MS * millisecond;
    let none_loaded = || {
        here.active_address_space() == cpu::NO_ADDRESS_SPACE
            && cpu::CPUS
                .all()
                .iter()
                .all(|cpu| cpu.active_address_space() == cpu::NO_ADDRESS_SPACE)
    };
    while !(none_loaded() && freed()) && P::clock_ticks() < give_up {
        for &other in cpu::CPUS.all() {
            if !core::ptr::eq(other, here) {
                cpu::wake::<P>(other);
            }
        }
        thread::sleep_until::<P>(P::clock_ticks() + millisecond);
    }
    let released = none_loaded();
    fence(Ordering::Acquire);
    released
}

fn children_end_and_are_freed<P: Platform>(process: &Process<P>) -> bool {
    let here = cpu::current::<P>();
    let millisecond = P::clock_frequency() / 1000;
    let give_up = P::clock_ticks() + THREADS_MEET_TIMEOUT_MS * millisecond;
    let freed = || process.children().iter().all(|child| child.freed);
    while !freed() && P::clock_ticks() < give_up {
        for &other in cpu::CPUS.all() {
            if !core::ptr::eq(other, here) {
                cpu::wake::<P>(other);
            }
        }
        thread::sleep_until::<P>(P::clock_ticks() + millisecond);
    }
    freed()
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
    let before = free_and_slab_frames().ok_or("there is no frame allocator")?;
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
    let give_up = P::clock_ticks() + THREADS_MEET_TIMEOUT_MS * millisecond;
    while !threads.iter().all(|(thread, _)| thread.is_finished()) {
        if P::clock_ticks() >= give_up {
            return Err("a spinning program did not end at an interrupt");
        }
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
    if free_and_slab_frames() != Some(before) {
        return Err("the processes' frames and tables were not all freed");
    }
    Ok(())
}

/// A Linux program (M6) must write to the console through `write`, get Linux's results for a
/// bad buffer (`EFAULT`) and an unknown call (`ENOSYS`), and end with `exit_group`; one that
/// runs an undefined instruction must be ended with `SIGILL`, after it got FP/SIMD registers
/// in case that was what it wanted (D54). The QEMU runner checks that
/// the line it writes reached the console. Everything the processes took must be freed.
fn linux_programs_write_and_exit<P: Platform>(memory: &mut Memory<P>) -> Result<(), &'static str> {
    let hello = TestProgram::LINUX_HELLO.as_bytes();
    if !P::user_test_program()
        .code
        .windows(hello.len())
        .any(|bytes| bytes == hello)
    {
        return Err("the test program does not write the line it is checked against");
    }
    let before = free_and_slab_frames().ok_or("there is no frame allocator")?;
    for (entry, expected) in [
        (
            (|program: &TestProgram| program.linux_hello) as fn(&TestProgram) -> usize,
            ProcessExit::Exited(0),
        ),
        (
            |program| program.illegal_instruction,
            ProcessExit::Killed {
                signal: linux::signal::SIGILL,
            },
        ),
    ] {
        // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
        let process = Arc::new(unsafe { Process::<P>::with_test_program(&memory.page_tables) }?);
        let context = Process::<P>::test_program_context(entry);
        linux::spawn::<P>(process.clone(), context)
            .map_err(|_| "could not start a Linux program's thread")?
            .join::<P>();
        match process.exit_status() {
            Some(exit) if exit == expected => {}
            Some(ProcessExit::Exited(_)) => {
                return Err("a Linux system call returned the wrong result");
            }
            Some(ProcessExit::Killed { .. }) => {
                return Err("a Linux program was killed, or with the wrong signal");
            }
            None => return Err("a Linux program's thread ended without ending it"),
        }
    }
    let freed = || free_and_slab_frames() == Some(before);
    if !release_loaded_address_spaces::<P>(freed) || !freed() {
        return Err("the Linux programs' frames and tables were not all freed");
    }
    Ok(())
}

/// What [`linux_start_image`]'s program is started with: the strings it writes, in order,
/// arguments first. Together they are [`LINUX_START_LINE`].
pub const LINUX_START_ARGUMENTS: [&str; 2] =
    ["EGGOS-USER: started from an ELF image", " with arguments"];
pub const LINUX_START_ENVIRONMENT: [&str; 1] = [" and an environment\n"];
/// The line [`linux_start_image`]'s program writes. The QEMU runner requires it on the
/// console: it must match `ELF_START_LINE` in `xtask`.
pub const LINUX_START_LINE: &str =
    "EGGOS-USER: started from an ELF image with arguments and an environment\n";
/// Where [`linux_start_image`] puts its data: an address that is not page-aligned, as linkers
/// place data after code.
pub const LINUX_START_DATA: u64 = 0x60_0123;
/// [`linux_start_image`]'s initialized data; zero follows, up to [`LINUX_START_DATA_SIZE`].
pub const LINUX_START_DATA_BYTES: &[u8; 16] = b"EGGOS data bytes";
/// [`linux_start_image`]'s data and `.bss`: across three pages.
pub const LINUX_START_DATA_SIZE: u64 = 0x2100;

/// An ELF executable around the HAL's test program, as a linker lays one out: a read-only,
/// executable segment from the start of the file, its first page the file and program
/// headers (`PHDR` names them), its second the test program's code at
/// [`process::TEST_PROGRAM_ADDRESS`], entered at its `linux_start`; then a writable data
/// segment at [`LINUX_START_DATA`] with initialized data and `.bss`; and a non-executable
/// stack. There is no file system to load one from yet (M7).
pub fn linux_start_image<P: Platform>() -> Vec<u8> {
    linux_image::<P>(P::user_test_program().code)
}

/// [`linux_start_image`] with the test program's code only up to the end of its
/// `linux_start`, for the `linux_exec_load` benchmark: adding a test program after it does
/// not change what loading the image costs (D62).
pub fn linux_start_only_image<P: Platform>() -> Vec<u8> {
    let program = P::user_test_program();
    linux_image::<P>(&program.code[..program.linux_start_end])
}

/// [`linux_start_image`] with `code`, the start of the test program's.
fn linux_image<P: Platform>(code: &[u8]) -> Vec<u8> {
    let program = P::user_test_program();
    let text = process::TEST_PROGRAM_ADDRESS - FRAME_SIZE;
    let text_size = FRAME_SIZE + code.len() as u64;
    let data_offset = 2 * FRAME_SIZE + LINUX_START_DATA % FRAME_SIZE;
    let headers = 4;
    let header = |kind, flags, offset, virt, file_size, memory_size| ProgramHeader {
        kind,
        flags,
        offset,
        virt,
        file_size,
        memory_size,
        align: FRAME_SIZE,
    };
    let table = (headers * elf::PROGRAM_HEADER_SIZE) as u64;
    let header_at = elf::HEADER_SIZE as u64;
    elf::build::image(
        2,
        elf::machine::NATIVE,
        process::TEST_PROGRAM_ADDRESS + program.linux_start as u64,
        &[
            header(
                segment::PHDR,
                segment::READ,
                header_at,
                text + header_at,
                table,
                table,
            ),
            header(
                segment::LOAD,
                segment::READ | segment::EXECUTE,
                0,
                text,
                text_size,
                text_size,
            ),
            header(
                segment::LOAD,
                segment::READ | segment::WRITE,
                data_offset,
                LINUX_START_DATA,
                LINUX_START_DATA_BYTES.len() as u64,
                LINUX_START_DATA_SIZE,
            ),
            header(
                segment::GNU_STACK,
                segment::READ | segment::WRITE,
                0,
                0,
                0,
                0,
            ),
        ],
        &[
            (FRAME_SIZE as usize, code),
            (data_offset as usize, LINUX_START_DATA_BYTES),
        ],
    )
}

/// A Linux program started from an ELF image (M6, D51) must find its arguments and its
/// environment on its stack, as Linux lays them out, find its thread's id is its process's
/// (`getpid`, `gettid`, D55), and write them with `writev` from its stack
/// ([`LINUX_START_LINE`], which the QEMU runner checks reached the console), then end with
/// `exit_group(argc)`. Its
/// initialized data must be in place and its `.bss` zero, though the frames held other data
/// before. Everything it took must be freed.
fn linux_programs_start_from_elf_images<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let joined: Vec<u8> = LINUX_START_ARGUMENTS
        .iter()
        .chain(&LINUX_START_ENVIRONMENT)
        .flat_map(|string| string.bytes())
        .collect();
    if joined != LINUX_START_LINE.as_bytes() {
        return Err("the program's strings are not the line it is checked against");
    }
    let before = free_and_slab_frames().ok_or("there is no frame allocator")?;
    {
        let image = linux_start_image::<P>();
        let arguments = LINUX_START_ARGUMENTS.map(str::as_bytes);
        let environment = LINUX_START_ENVIRONMENT.map(str::as_bytes);
        // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
        let mut process = unsafe { Process::<P>::new(&memory.page_tables) }?;
        let context = exec::load(&mut process, &image, b"/selftest", &arguments, &environment)
            .map_err(|_| "the loader refused a valid ELF image")?;
        let mut data = alloc::vec![0xffu8; LINUX_START_DATA_SIZE as usize];
        process
            .read_user_memory(LINUX_START_DATA, &mut data)
            .map_err(|_| "the program's data is not mapped")?;
        let (initialized, bss) = data.split_at(LINUX_START_DATA_BYTES.len());
        if initialized != LINUX_START_DATA_BYTES || bss.iter().any(|&byte| byte != 0) {
            return Err("the program's data is not its image's, or its .bss is not zero");
        }
        let process = Arc::new(process);
        linux::spawn::<P>(process.clone(), context)
            .map_err(|_| "could not start a Linux program's thread")?
            .join::<P>();
        match process.exit_status() {
            Some(ProcessExit::Exited(2)) => {}
            Some(ProcessExit::Exited(255)) => {
                return Err("the program's stack pointer is not 16-byte aligned");
            }
            Some(_) => return Err("the program did not find its two arguments"),
            None => return Err("a Linux program's thread ended without ending it"),
        }
    }
    let freed = || free_and_slab_frames() == Some(before);
    if !release_loaded_address_spaces::<P>(freed) || !freed() {
        return Err("the ELF program's frames and tables were not all freed");
    }
    Ok(())
}

/// A Linux program's stack is mapped as the program touches it (D52): the test program's
/// `stack_growth`, started from the self-test's ELF image with its stack, stores to every page
/// of the stack's 8 MiB reserve from the arguments' page down, each a page fault the process
/// resolves with a zeroed frame, until it runs into the guard gap below and is killed with
/// `SIGSEGV`. Every page of the reserve must then hold its own address, nothing below it may
/// be mapped, and everything must be freed.
fn linux_stacks_grow_on_demand<P: Platform>(memory: &mut Memory<P>) -> Result<(), &'static str> {
    let user_end = <P::PageTableFormat as Format>::USER_END;
    let reserve = user_end - exec::STACK_RESERVE;
    let before = free_and_slab_frames().ok_or("there is no frame allocator")?;
    {
        let image = linux_start_image::<P>();
        // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
        let mut process = unsafe { Process::<P>::new(&memory.page_tables) }?;
        let started = exec::load(&mut process, &image, b"/selftest", &[], &[])
            .map_err(|_| "the loader refused a valid ELF image")?;
        let entry = Process::<P>::test_program_context(|program| program.stack_growth).pc();
        let context = UserContext::new(entry, started.stack());
        let process = Arc::new(process);
        linux::spawn::<P>(process.clone(), context)
            .map_err(|_| "could not start a Linux program's thread")?
            .join::<P>();
        if process.exit_status()
            != Some(ProcessExit::Killed {
                signal: linux::signal::SIGSEGV,
            })
        {
            return Err("the program did not run off its stack into a SIGSEGV");
        }
        let top = started.stack() & !(FRAME_SIZE - 1);
        for page in (reserve..=top).step_by(FRAME_SIZE as usize) {
            let mut word = [0; 8];
            process
                .read_user_memory(page, &mut word)
                .map_err(|_| "a stack page is not mapped")?;
            if u64::from_ne_bytes(word) != page {
                return Err("a stack page does not hold what the program stored there");
            }
        }
        if process
            .address_space()
            .translate(reserve - FRAME_SIZE)
            .is_some()
        {
            return Err("a page below the stack's reserve was mapped");
        }
    }
    let freed = || free_and_slab_frames() == Some(before);
    if !release_loaded_address_spaces::<P>(freed) || !freed() {
        return Err("the stack's frames and tables were not all freed");
    }
    Ok(())
}

/// The memory calls change what a running program reaches at once (D53), what its CPU cached
/// included. The program walks down an `mmap`ed range, storing to every page (the test
/// program's `stack_growth`, its stack pointer at the range's top), until it runs off the
/// bottom; then, through the Linux calls, the kernel unmaps the upper half, maps it again,
/// makes the range read-only and then writable again, running the walk from the top after
/// each. Every change follows a walk that left the CPU with writable translations of the
/// pages, so the first store must fault at the top page while it is unmapped or read-only (a
/// store through a stale translation would walk on), and the walk must cover the whole range
/// when it is writable; the mapped-again half must read as zero, and every frame be freed.
fn linux_memory_calls_take_effect_at_once<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    use linux::memory::{map, prot};
    const PAGES: u64 = 8;
    const HALF: u64 = PAGES / 2 * FRAME_SIZE;
    let before = free_and_slab_frames().ok_or("there is no frame allocator")?;
    {
        let image = linux_start_image::<P>();
        // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
        let mut process = unsafe { Process::<P>::new(&memory.page_tables) }?;
        exec::load(&mut process, &image, b"/selftest", &[], &[])
            .map_err(|_| "the loader refused a valid ELF image")?;
        let process = Arc::new(process);
        let read_write = prot::READ | prot::WRITE;
        let anonymous = map::PRIVATE | map::ANONYMOUS;
        let start = linux::memory::mmap(&process, 0, 2 * HALF, read_write, anonymous, 0)
            .map_err(|_| "mmap refused an anonymous mapping")?;
        let top = start + 2 * HALF;
        let entry = Process::<P>::test_program_context(|program| program.stack_growth).pc();
        let walk = move || UserContext::new(entry, top - 16);
        // Where each walk stopped, and the first thing that went wrong.
        let stops = Arc::new(SpinLock::new(Vec::new()));
        let failure = Arc::new(SpinLock::new(None));
        let handler = {
            let (stops, failure) = (stops.clone(), failure.clone());
            let mut walks = 0;
            move |process: &Arc<Process<P>>, context: &mut UserContext, exit| {
                let fail = |reason| {
                    failure.lock().get_or_insert(reason);
                    ControlFlow::Break(())
                };
                let exception = match exit {
                    UserExit::Interrupt => return ControlFlow::Continue(()),
                    UserExit::Syscall => return fail("the walk made a system call"),
                    UserExit::Exception(exception) => exception,
                };
                let (Some(fault), Some(address)) = (
                    P::page_fault(exception.kind, exception.code),
                    exception.address,
                ) else {
                    return fail("the walk took an exception that is not a page fault");
                };
                if process.resolve_page_fault(address, fault).is_ok() {
                    return ControlFlow::Continue(());
                }
                stops.lock().push(address & !(FRAME_SIZE - 1));
                let changed = match walks {
                    0 => linux::memory::munmap(process, start + HALF, HALF),
                    1 => {
                        let fixed = anonymous | map::FIXED;
                        let remapped =
                            linux::memory::mmap(process, start + HALF, HALF, read_write, fixed, 0);
                        let mut word = [1; 8];
                        if remapped != Ok(start + HALF)
                            || process.read_user_memory(top - 8, &mut word).is_err()
                            || word != [0; 8]
                        {
                            return fail("the upper half was not mapped again, zeroed");
                        }
                        Ok(0)
                    }
                    2 => linux::memory::mprotect(process, start, 2 * HALF, prot::READ),
                    3 => linux::memory::mprotect(process, start, 2 * HALF, read_write),
                    _ => return ControlFlow::Break(()),
                };
                if changed.is_err() {
                    return fail("a memory call refused a valid range");
                }
                walks += 1;
                *context = walk();
                ControlFlow::Continue(())
            }
        };
        process::spawn::<P, _>(process.clone(), walk(), handler)
            .map_err(|_| "could not start a Linux program's thread")?
            .join::<P>();
        if let Some(reason) = *failure.lock() {
            return Err(reason);
        }
        let last = top - FRAME_SIZE;
        let below = start - FRAME_SIZE;
        if *stops.lock() != [below, last, below, last, below] {
            return Err("a walk went through a page unmapped or read-only, or stopped short");
        }
        for page in (start..top).step_by(FRAME_SIZE as usize) {
            let mut word = [0; 8];
            process
                .read_user_memory(page, &mut word)
                .map_err(|_| "a page of the range is not mapped")?;
            if u64::from_ne_bytes(word) != page {
                return Err("a page does not hold what the program stored there");
            }
        }
    }
    let freed = || free_and_slab_frames() == Some(before);
    if !release_loaded_address_spaces::<P>(freed) || !freed() {
        return Err("the range's frames and the program's were not all freed");
    }
    Ok(())
}

/// [`processes_keep_their_own_memory`] on every CPU: first one process whose threads, one per
/// CPU (up to 16), run on different CPUs at once, so several CPUs keep its address space loaded
/// when it ends and must all let it go; then a process per CPU with two threads each, so they
/// run, load their address spaces and their code on the other CPUs too, and move between them;
/// then [`linux_programs_keep_their_thread_pointers`] with more programs than CPUs, and
/// [`processes_keep_their_fp_registers`] with two programs per CPU (up to 16), which move
/// between CPUs with their FP/SIMD registers.
fn processes_run_on_every_cpu<P: Platform>(memory: &mut Memory<P>) -> Result<(), &'static str> {
    let cpus = cpu::CPUS.all().len();
    if cpus <= 1 {
        return Ok(());
    }
    processes_keep_their_own_memory(memory, 1, cpus.min(16))?;
    processes_keep_their_own_memory(memory, cpus.min(8), 2)?;
    linux_programs_keep_their_thread_pointers(memory, (2 * cpus + 1).min(17))?;
    processes_keep_their_fp_registers(memory, (2 * cpus).min(16))
}

/// Linux programs must each keep their own thread pointer (D57): `count` processes run the
/// test program's `linux_thread_pointer`, which starts with a zero thread pointer, sets its
/// own and checks it after each of 20 `sched_yield`s, while the others run with theirs on the
/// same CPU (more programs than CPUs) or it moves to another. Each must end with
/// `exit_group(0)`. The programs run twice, and everything the second round took must be
/// freed: the first may grow the heap's size classes, whose frames stay with them (D30).
fn linux_programs_keep_their_thread_pointers<P: Platform>(
    memory: &mut Memory<P>,
    count: usize,
) -> Result<(), &'static str> {
    let mut before = None;
    for round in 0..2 {
        if round == 1 {
            // After the CPUs let go of the first round's address spaces, which they keep
            // loaded until they need another.
            if !release_loaded_address_spaces::<P>(|| true) {
                return Err("a CPU kept an ended process's address space loaded");
            }
            before = Some(free_and_slab_frames().ok_or("there is no frame allocator")?);
        }
        let mut programs = Vec::new();
        for _ in 0..count {
            // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
            let process =
                Arc::new(unsafe { Process::<P>::with_test_program(&memory.page_tables) }?);
            let context =
                Process::<P>::test_program_context(|program| program.linux_thread_pointer);
            let thread = linux::spawn::<P>(process.clone(), context)
                .map_err(|_| "could not start a Linux program's thread")?;
            programs.push((process, thread));
        }
        for (process, thread) in programs {
            thread.join::<P>();
            match process.exit_status() {
                Some(ProcessExit::Exited(0)) => {}
                Some(ProcessExit::Exited(_)) => {
                    return Err("a program's thread pointer changed, or a call failed");
                }
                Some(ProcessExit::Killed { .. }) => {
                    return Err("a program was killed (on x86_64: its FS base was not its own)");
                }
                None => return Err("a Linux program's thread ended without ending it"),
            }
        }
    }
    let freed = || free_and_slab_frames() == before;
    if !release_loaded_address_spaces::<P>(freed) || !freed() {
        return Err("the Linux programs' frames and tables were not all freed");
    }
    Ok(())
}

/// A Linux program must find the console's descriptors as Rust's std and the C libraries
/// expect at startup, and a `ppoll` that waits must sleep for its timeout (D61): the test
/// program's `linux_poll` must end with `exit_group(0)`, no sooner than
/// [`TestProgram::LINUX_POLL_WAIT_NS`] after it started. It runs twice, and everything the
/// second run took must be freed: the first may grow the heap's size classes, whose frames
/// stay with them (D30).
fn linux_programs_poll_the_console<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    // A page below the stack pointer for the program's structures.
    const STACK_TOP: u64 = 0x70_0000;
    let wait = mul_div_ceil(
        TestProgram::LINUX_POLL_WAIT_NS,
        P::clock_frequency(),
        1_000_000_000,
    );
    let mut before = None;
    for round in 0..2 {
        if round == 1 {
            // After the CPU lets go of the first run's address space, which it keeps loaded
            // until it needs another.
            if !release_loaded_address_spaces::<P>(|| true) {
                return Err("a CPU kept an ended process's address space loaded");
            }
            before = Some(free_and_slab_frames().ok_or("there is no frame allocator")?);
        }
        // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
        let mut process = unsafe { Process::<P>::with_test_program(&memory.page_tables) }?;
        process.map_new_page(STACK_TOP - FRAME_SIZE, Attributes::USER_READ_WRITE, &[])?;
        let process = Arc::new(process);
        let program = P::user_test_program();
        let context = UserContext::new(
            process::TEST_PROGRAM_ADDRESS + program.linux_poll as u64,
            STACK_TOP,
        );
        let started = P::clock_ticks();
        linux::spawn::<P>(process.clone(), context)
            .map_err(|_| "could not start a Linux program's thread")?
            .join::<P>();
        let elapsed = P::clock_ticks() - started;
        match process.exit_status() {
            Some(ProcessExit::Exited(0)) => {}
            Some(ProcessExit::Exited(_)) => {
                return Err("poll, ppoll or ioctl did not answer as on a Linux serial console");
            }
            Some(ProcessExit::Killed { .. }) => return Err("the polling program was killed"),
            None => return Err("a Linux program's thread ended without ending it"),
        }
        if elapsed < wait {
            return Err("ppoll returned before its timeout");
        }
    }
    let freed = || free_and_slab_frames() == before;
    if !release_loaded_address_spaces::<P>(freed) || !freed() {
        return Err("the polling program's frames and tables were not all freed");
    }
    Ok(())
}

/// A Linux program's signals must be delivered by their actions before the call that made
/// them due returns (D63): the test program's `linux_signals` must be ended by the `SIGUSR1`
/// it blocked, sent itself and then unblocked, after `kill(0, SIGCHLD)` was ignored and
/// `rt_sigpending` reported it. It runs twice, and everything the second run took must be
/// freed, as in [`linux_programs_poll_the_console`].
fn linux_programs_signal_themselves<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    // A page below the stack pointer for the program's signal sets.
    const STACK_TOP: u64 = 0x70_0000;
    const SIGUSR1: u32 = 10;
    let mut before = None;
    for round in 0..2 {
        if round == 1 {
            if !release_loaded_address_spaces::<P>(|| true) {
                return Err("a CPU kept an ended process's address space loaded");
            }
            before = Some(free_and_slab_frames().ok_or("there is no frame allocator")?);
        }
        // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
        let mut process = unsafe { Process::<P>::with_test_program(&memory.page_tables) }?;
        process.map_new_page(STACK_TOP - FRAME_SIZE, Attributes::USER_READ_WRITE, &[])?;
        let process = Arc::new(process);
        let program = P::user_test_program();
        let context = UserContext::new(
            process::TEST_PROGRAM_ADDRESS + program.linux_signals as u64,
            STACK_TOP,
        );
        linux::spawn::<P>(process.clone(), context)
            .map_err(|_| "could not start a Linux program's thread")?
            .join::<P>();
        match process.exit_status() {
            Some(ProcessExit::Killed { signal: SIGUSR1 }) => {}
            Some(ProcessExit::Killed { .. }) => return Err("the program died of another signal"),
            Some(ProcessExit::Exited(_)) => {
                return Err("a signal call failed, or SIGUSR1 was not delivered when unblocked");
            }
            None => return Err("a Linux program's thread ended without ending it"),
        }
    }
    let freed = || free_and_slab_frames() == before;
    if !release_loaded_address_spaces::<P>(freed) || !freed() {
        return Err("the signalling program's frames and tables were not all freed");
    }
    Ok(())
}

/// A Linux program's signal handler must run on its alternate stack with Linux's frame and
/// return with `rt_sigreturn` to the program as it was (D64): the test program's
/// `linux_signal_handler` must exit with 0 (see [`eggos_hal::user::TestProgram`]). On AArch64
/// and RISC-V its handler returns through the kernel's trampoline, which must be mapped for
/// it. It runs twice, and everything the second run took (the frames of its stack, the
/// trampoline's page, the signal state's block) must be freed, as in
/// [`linux_programs_poll_the_console`].
fn linux_programs_run_signal_handlers<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let program = P::user_test_program();
    run_handling_program(memory, program.linux_signal_handler).map_err(|failure| match failure {
        HandlingFailure::Failed => {
            "a signal call failed, or the handler or rt_sigreturn went wrong"
        }
        HandlingFailure::Killed => "the handling program was killed",
        HandlingFailure::Other(error) => error,
    })
}

/// A Linux program's faults must go to its `SIGSEGV` handler with Linux's `si_code`,
/// `si_addr` and machine context, and the handler must resume it where its frame says (D65):
/// the test program's `linux_fault_handler` must exit with 0 (see
/// [`eggos_hal::user::TestProgram`]), after a fault where nothing is mapped and one on a
/// `PROT_NONE` page. It runs twice, and everything the second run took must be freed, as in
/// [`linux_programs_run_signal_handlers`].
fn linux_programs_handle_their_faults<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let program = P::user_test_program();
    run_handling_program(memory, program.linux_fault_handler).map_err(|failure| match failure {
        HandlingFailure::Failed => "a call failed, or a fault's handler saw the wrong fault",
        HandlingFailure::Killed => "the faulting program was killed: its handler did not run",
        HandlingFailure::Other(error) => error,
    })
}

fn linux_programs_queue_signals_with_their_info<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let program = P::user_test_program();
    run_handling_program(memory, program.linux_signal_queue).map_err(|failure| match failure {
        HandlingFailure::Failed => {
            "a queued signal was not taken in order with its siginfo, or a signal call failed"
        }
        HandlingFailure::Killed => "the queueing program was killed by a signal it blocked",
        HandlingFailure::Other(error) => error,
    })
}

/// A Linux program's `rt_sigsuspend` must run the handler of the signal its mask unblocks,
/// on a frame that saved the mask from before the call, then fail with `EINTR` with that mask
/// restored (D70): the test program's `linux_signal_suspend` must exit with 0 (see
/// [`eggos_hal::user::TestProgram`]). It runs twice, and everything the second run took must
/// be freed, as in [`linux_programs_run_signal_handlers`].
fn linux_programs_wait_for_signals_with_sigsuspend<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let program = P::user_test_program();
    run_handling_program(memory, program.linux_signal_suspend).map_err(|failure| match failure {
        HandlingFailure::Failed => {
            "rt_sigsuspend did not run the handler with the mask saved, or a signal call failed"
        }
        HandlingFailure::Killed => "the suspending program was killed",
        HandlingFailure::Other(error) => error,
    })
}

/// A Linux program must read the monotonic clock and sleep with `nanosleep` for at least
/// what it asked, and be refused the CPU-time clocks (D73): the test program's `linux_clock`
/// must exit with 0 (see [`eggos_hal::user::TestProgram`]). It runs twice, and everything the
/// second run took must be freed.
fn linux_programs_read_clocks_and_sleep<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let program = P::user_test_program();
    run_handling_program(memory, program.linux_clock).map_err(|failure| match failure {
        HandlingFailure::Failed => {
            "clock_gettime or nanosleep did not answer as on Linux, or the sleep ended early"
        }
        HandlingFailure::Killed => "the clock program was killed",
        HandlingFailure::Other(error) => error,
    })
}

/// A Linux program must find the console a character device, `/dev/console`'s 5:1, through
/// `fstat`, `newfstatat` and `statx`, and be refused `lseek` on it and `fstat` on a descriptor
/// that is not open (D74): the test program's `linux_stat` must exit with 0 (see
/// [`eggos_hal::user::TestProgram`]). It runs twice, and everything the second run took must
/// be freed.
fn linux_programs_read_the_console_s_status<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let program = P::user_test_program();
    run_handling_program(memory, program.linux_stat).map_err(|failure| match failure {
        HandlingFailure::Failed => {
            "fstat, newfstatat, statx or lseek did not answer for the console as on Linux"
        }
        HandlingFailure::Killed => "the status program was killed",
        HandlingFailure::Other(error) => error,
    })
}

/// A Linux program starts as root and may give that up for good (D75): the test program's
/// `linux_credentials` must exit with 0 (see [`eggos_hal::user::TestProgram`]). It runs twice,
/// and everything the second run took must be freed.
fn linux_programs_change_their_credentials<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let program = P::user_test_program();
    run_handling_program(memory, program.linux_credentials).map_err(|failure| match failure {
        HandlingFailure::Failed => "getuid, geteuid or setuid did not answer as on Linux",
        HandlingFailure::Killed => "the credentials program was killed",
        HandlingFailure::Other(error) => error,
    })
}

/// A Linux program moves a page with `mremap`, its contents with it and nothing of it left
/// at the old address, and discards it with `madvise` (D77): the test program's
/// `linux_move_and_discard` must exit with 0 (see [`eggos_hal::user::TestProgram`]). It runs
/// twice, and everything the second run took must be freed.
fn linux_programs_move_and_discard_memory<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let program = P::user_test_program();
    run_handling_program(memory, program.linux_move_and_discard).map_err(|failure| match failure {
        HandlingFailure::Failed => "mremap or madvise did not move or discard memory as on Linux",
        HandlingFailure::Killed => "the program moving memory was killed",
        HandlingFailure::Other(error) => error,
    })
}

struct RustProgram {
    image: Option<&'static [u8]>,
    path: &'static [u8],
    arguments_after_path: &'static [&'static [u8]],
    failure_statuses: &'static [(u32, &'static str)],
    failure_signals: &'static [(u32, &'static str)],
    children_statuses: &'static [u32],
}

static NO_STD_PROGRAM: RustProgram = RustProgram {
    #[cfg(all(target_os = "none", feature = "qemu-test"))]
    image: Some(include_bytes!(env!(
        "EGGOS_USERLAND_SELFTEST",
        "build kernels with qemu-test through cargo xtask, which builds userland/selftest first"
    ))),
    #[cfg(not(all(target_os = "none", feature = "qemu-test")))]
    image: None,
    path: b"/eggos-selftest",
    arguments_after_path: &[],
    failure_statuses: &[
        (
            127,
            "the Rust program could not relocate itself or protect its data",
        ),
        (101, "the Rust program panicked"),
    ],
    failure_signals: &[],
    children_statuses: &[],
};

static STD_PROGRAM: RustProgram = RustProgram {
    #[cfg(all(target_os = "none", feature = "qemu-test"))]
    image: Some(include_bytes!(env!(
        "EGGOS_USERLAND_STD_SELFTEST",
        "build kernels with qemu-test through cargo xtask, which builds userland/std-selftest first"
    ))),
    #[cfg(not(all(target_os = "none", feature = "qemu-test")))]
    image: None,
    path: b"/eggos-std-selftest",
    arguments_after_path: &[b"first", b"second"],
    failure_statuses: &[],
    failure_signals: &[(SIGABRT, "the std Rust program panicked: it aborted")],
    children_statuses: &[],
};

static STD_PROGRAM_EXITING_FROM_A_THREAD: RustProgram = RustProgram {
    arguments_after_path: &[b"exit-from-a-thread"],
    failure_statuses: &[(
        1,
        "the std Rust program's main thread woke from its sleep before the process ended",
    )],
    ..STD_PROGRAM
};

static STD_PROGRAM_READING_THE_CONSOLE: RustProgram = RustProgram {
    arguments_after_path: &[b"read-the-console"],
    failure_statuses: &[(
        1,
        "a console check of the std Rust program failed: it named it on the console",
    )],
    ..STD_PROGRAM
};

static STD_PROGRAM_STARTING_A_COPY_FROM_A_FILE: RustProgram = RustProgram {
    arguments_after_path: &[b"exec"],
    failure_statuses: &[
        (
            2,
            "execve refused or accepted a call wrongly before the point of no return",
        ),
        (
            3,
            "execve returned instead of starting the std Rust program from its file",
        ),
        (
            4,
            "the std Rust program started by execve found something wrong: it named it on the console",
        ),
    ],
    ..STD_PROGRAM
};

static STD_PROGRAM_UNMAPPING_UNDER_A_WRITING_THREAD: RustProgram = RustProgram {
    arguments_after_path: &[b"unmap-under-a-writing-thread"],
    failure_statuses: &[
        (
            1,
            "a thread went on writing a page another thread unmapped on another CPU",
        ),
        (3, "the std Rust program's threads did not run on two CPUs"),
        (4, "the std Rust program could not map a page"),
    ],
    ..STD_PROGRAM
};

static STD_PROGRAM_CREATING_PROCESSES: RustProgram = RustProgram {
    arguments_after_path: &[b"processes"],
    failure_statuses: &[(
        1,
        "a process-creation check of the std Rust program failed: it named it on the console",
    )],
    children_statuses: &[21, 22, 24, 25, 26, 27],
    ..STD_PROGRAM
};

const SIGABRT: u32 = 6;

const SIGCHLD: u8 = 17;

const RUST_PROGRAM_ENVIRONMENT: [&[u8]; 2] = [b"HOME=/", b"TERM=linux"];

fn linux_programs_built_from_rust_source_start<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    run_rust_program(memory, &NO_STD_PROGRAM)
}

fn linux_programs_built_with_rust_std_run<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    run_rust_program(memory, &STD_PROGRAM)
}

fn linux_programs_read_the_console_through_its_line_discipline<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    if cfg!(feature = "qemu-bench") {
        return Ok(());
    }
    run_rust_program(memory, &STD_PROGRAM_READING_THE_CONSOLE)
}

fn linux_programs_end_from_a_thread_while_others_wait<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    run_rust_program(memory, &STD_PROGRAM_EXITING_FROM_A_THREAD)
}

fn linux_programs_start_programs_from_files<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    run_rust_program(memory, &STD_PROGRAM_STARTING_A_COPY_FROM_A_FILE)
}

fn linux_programs_create_processes<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    run_rust_program(memory, &STD_PROGRAM_CREATING_PROCESSES)
}

/// The std program's checks again, with every CPU running (D94): its threads spread over
/// them, so its `Mutex`, `Condvar` and joins go through futex words used on several CPUs.
fn linux_programs_with_threads_run_on_every_cpu<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    run_rust_program(memory, &STD_PROGRAM)
}

/// One thread of the std program unmaps a page another writes in a loop on another CPU: the
/// shootdown (D94) must make the writer fault at once. Needs two CPUs (the benchmarks boot
/// one); with preemption on, so two threads placed on one CPU both run and the program says
/// so instead of spinning for good.
fn linux_threads_stop_using_a_page_unmapped_on_another_cpu<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    if cpu::CPUS.online() < 2 {
        return Ok(());
    }
    thread::enable_preemption::<P>();
    let result = run_rust_program(memory, &STD_PROGRAM_UNMAPPING_UNDER_A_WRITING_THREAD);
    thread::disable_preemption::<P>();
    result
}

fn run_rust_program<P: Platform>(
    memory: &mut Memory<P>,
    program: &RustProgram,
) -> Result<(), &'static str> {
    let Some(image) = program.image else {
        return Ok(());
    };
    let mut arguments = [program.path; 3];
    let argument_count = 1 + program.arguments_after_path.len();
    let arguments = arguments
        .get_mut(..argument_count)
        .ok_or("a Rust program with too many arguments")?;
    arguments[1..].copy_from_slice(program.arguments_after_path);
    let mut before = None;
    for round in 0..2 {
        if round == 1 {
            if !release_loaded_address_spaces::<P>(|| true) {
                return Err("a CPU kept an ended process's address space loaded");
            }
            before = Some(free_and_slab_frames().ok_or("there is no frame allocator")?);
        }
        // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
        let mut process = unsafe { Process::<P>::new(&memory.page_tables) }?;
        let context = exec::load(
            &mut process,
            image,
            program.path,
            arguments,
            &RUST_PROGRAM_ENVIRONMENT,
        )
        .map_err(|_| "the loader refused the Rust program's image")?;
        let process = Arc::new(process);
        linux::spawn::<P>(process.clone(), context)
            .map_err(|_| "could not start a Linux program's thread")?
            .join::<P>();
        if !children_end_and_are_freed::<P>(&process) {
            return Err("a child of the Rust program did not end, or was not freed");
        }
        match process.exit_status() {
            Some(ProcessExit::Exited(0)) => {}
            Some(ProcessExit::Exited(status)) => {
                return Err(program
                    .failure_statuses
                    .iter()
                    .find(|(known, _)| *known == status)
                    .map_or(
                        "a check of the Rust program failed: it named it on the console",
                        |(_, failure)| failure,
                    ));
            }
            Some(ProcessExit::Killed { signal }) => {
                return Err(program
                    .failure_signals
                    .iter()
                    .find(|(known, _)| *known == signal)
                    .map_or("the Rust program was killed by a signal", |(_, failure)| {
                        failure
                    }));
            }
            None => return Err("a Linux program's thread ended without ending it"),
        }
        let children = process.children();
        let statuses_as_expected = children.len() == program.children_statuses.len()
            && children
                .iter()
                .zip(program.children_statuses)
                .all(|(child, &status)| {
                    child.exit == Some(ProcessExit::Exited(status)) && child.exit_signal == SIGCHLD
                });
        if !statuses_as_expected {
            return Err("a child of the Rust program ended otherwise than it should");
        }
    }
    let freed = || free_and_slab_frames() == before;
    if !release_loaded_address_spaces::<P>(freed) || !freed() {
        return Err("the Rust program's frames and tables were not all freed");
    }
    Ok(())
}

/// How a run of [`run_handling_program`] went wrong.
enum HandlingFailure {
    /// The program exited with a status other than 0.
    Failed,
    /// The program was killed by a signal.
    Killed,
    Other(&'static str),
}

/// Runs the test program's Linux program at `offset`, which handles signals, twice, on three
/// writable pages below its stack pointer, and requires it to exit with 0 and the second run
/// to leave every frame it took freed.
fn run_handling_program<P: Platform>(
    memory: &mut Memory<P>,
    offset: usize,
) -> Result<(), HandlingFailure> {
    use HandlingFailure::{Failed, Killed, Other};
    // Three pages below the stack pointer: the program's structures, then its alternate stack,
    // which holds the handler's frame.
    const STACK_TOP: u64 = 0x70_0000;
    let mut before = None;
    for round in 0..2 {
        if round == 1 {
            if !release_loaded_address_spaces::<P>(|| true) {
                return Err(Other("a CPU kept an ended process's address space loaded"));
            }
            before = Some(free_and_slab_frames().ok_or(Other("there is no frame allocator"))?);
        }
        // SAFETY: `memory.page_tables` are the kernel's tables, active on every CPU.
        let mut process =
            unsafe { Process::<P>::with_test_program(&memory.page_tables) }.map_err(Other)?;
        for page in 1..=3 {
            process
                .map_new_page(
                    STACK_TOP - page * FRAME_SIZE,
                    Attributes::USER_READ_WRITE,
                    &[],
                )
                .map_err(Other)?;
        }
        let process = Arc::new(process);
        let context = UserContext::new(process::TEST_PROGRAM_ADDRESS + offset as u64, STACK_TOP);
        linux::spawn::<P>(process.clone(), context)
            .map_err(|_| Other("could not start a Linux program's thread"))?
            .join::<P>();
        match process.exit_status() {
            Some(ProcessExit::Exited(0)) => {}
            Some(ProcessExit::Exited(_)) => return Err(Failed),
            Some(ProcessExit::Killed { .. }) => return Err(Killed),
            None => return Err(Other("a Linux program's thread ended without ending it")),
        }
    }
    let freed = || free_and_slab_frames() == before;
    if !release_loaded_address_spaces::<P>(freed) || !freed() {
        return Err(Other(
            "the handling program's frames and tables were not all freed",
        ));
    }
    Ok(())
}

/// The frame allocator's free frames plus those the heap's size classes keep
/// ([`crate::heap::KernelHeap::free_and_slab_frames`]), `None` if there is no frame allocator:
/// a size class that grows while a test runs, as one may when threads on several CPUs allocate
/// in another order than before, does not count as frames the test failed to free.
fn free_and_slab_frames() -> Option<u64> {
    crate::HEAP.free_and_slab_frames()
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

fn futex_waiters_are_requeued_in_order<P: Platform>() -> Result<(), &'static str> {
    use linux::futex::{self, Key};
    static QUEUED: AtomicUsize = AtomicUsize::new(0);
    static TIMED_OUT: AtomicBool = AtomicBool::new(false);
    const FROM: Key = Key::new(u32::MAX, 0x1000, false);
    const TO: Key = Key::new(u32::MAX, 0x2004, false);
    let millisecond = P::clock_frequency() / 1000;
    let counted = || {
        QUEUED.fetch_add(1, Ordering::Relaxed);
        Ok(())
    };
    let wait_until_queued = |count: usize| {
        while QUEUED.load(Ordering::Relaxed) < count {
            thread::yield_now::<P>();
        }
    };
    QUEUED.store(0, Ordering::Relaxed);
    let mut waiters = Vec::new();
    for (index, (key, bitset)) in [(TO, 0b1_0000), (FROM, 1), (FROM, 2), (FROM, 4), (FROM, 8)]
        .into_iter()
        .enumerate()
    {
        let waiter = thread::spawn::<P, _>(move || {
            let woken = futex::wait::<P>(key, bitset, u64::MAX, counted);
            assert_eq!(woken, Ok(0), "a requeued futex waiter was not woken");
        })
        .map_err(|_| "could not start five futex waiters")?;
        wait_until_queued(index + 1);
        waiters.push(waiter);
    }
    let refused = futex::requeue::<P>(FROM, TO, 1, 2, || Err(linux::Errno::EAGAIN));
    if refused != Err(linux::Errno::EAGAIN) {
        return Err("a futex requeue did not end with the word's error");
    }
    if futex::requeue::<P>(FROM, TO, 1, 2, || Ok(())) != Ok((1, 2)) {
        return Err("a futex requeue did not wake one waiter and move two");
    }
    if futex::wake::<P>(FROM, 0b0111, 3) != 0 {
        return Err("a woken or requeued waiter was left on the first futex");
    }
    if futex::wake::<P>(TO, 0b1_0110, 1) != 1 || futex::wake::<P>(TO, 0b1_0000, 1) != 0 {
        return Err("requeued futex waiters were not put behind the second futex's own");
    }
    if futex::wake::<P>(TO, 0b0110, 1) != 1 || futex::wake::<P>(TO, 0b0010, 1) != 0 {
        return Err("requeued futex waiters did not keep their order");
    }
    if futex::wake::<P>(TO, 0b0100, 1) != 1 {
        return Err("a requeued futex waiter did not keep its bitset");
    }
    if futex::requeue::<P>(FROM, FROM, 0, 5, || Ok(())) != Ok((0, 1)) {
        return Err("a futex requeue to the same futex did not count the waiter it left");
    }
    if futex::wake::<P>(FROM, u32::MAX, 5) != 1 {
        return Err("a futex waiter requeued to its own futex was lost");
    }
    for waiter in waiters {
        waiter.join::<P>();
    }
    let mut moved_before_its_deadline = false;
    for attempt in 0..8 {
        TIMED_OUT.store(false, Ordering::Relaxed);
        let deadline = P::clock_ticks() + (2 << attempt) * millisecond;
        let timed = thread::spawn::<P, _>(move || {
            let ended = futex::wait::<P>(FROM, u32::MAX, deadline, counted);
            TIMED_OUT.store(ended == Err(linux::Errno::ETIMEDOUT), Ordering::Relaxed);
        })
        .map_err(|_| "could not start a futex waiter")?;
        wait_until_queued(6 + attempt);
        let moved = futex::requeue::<P>(FROM, TO, 0, 1, || Ok(()));
        timed.join::<P>();
        match moved {
            Ok((0, 1)) => {
                moved_before_its_deadline = true;
                break;
            }
            Ok((0, 0)) => continue,
            _ => return Err("a futex requeue did not move a waiter with a deadline"),
        }
    }
    if !moved_before_its_deadline {
        return Err("a futex waiter with a deadline was never requeued before it");
    }
    if !TIMED_OUT.load(Ordering::Relaxed) {
        return Err("a requeued futex waiter did not time out");
    }
    if futex::wake::<P>(TO, u32::MAX, 5) != 0 {
        return Err("a requeued futex waiter that timed out was left queued");
    }
    Ok(())
}

fn futex_waiters_wake_in_order_and_time_out<P: Platform>() -> Result<(), &'static str> {
    use linux::futex::{self, Key};
    static QUEUED: AtomicUsize = AtomicUsize::new(0);
    const KEY: Key = Key::new(u32::MAX, 0x1000, false);
    const OTHER_SPACE: Key = Key::new(u32::MAX - 1, 0x1000, false);
    const SHARED: Key = Key::new(u32::MAX, 0x1000, true);
    let millisecond = P::clock_frequency() / 1000;
    let wait_forever = |bitset: u32| {
        futex::wait::<P>(KEY, bitset, u64::MAX, || {
            QUEUED.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    };
    QUEUED.store(0, Ordering::Relaxed);
    let mut waiters = Vec::new();
    for index in 0..3 {
        let waiter = thread::spawn::<P, _>(move || {
            assert_eq!(
                wait_forever(1 << index),
                Ok(0),
                "a futex waiter was not woken"
            );
        })
        .map_err(|_| "could not start three futex waiters")?;
        while QUEUED.load(Ordering::Relaxed) == index {
            thread::yield_now::<P>();
        }
        waiters.push(waiter);
    }
    if futex::wake::<P>(OTHER_SPACE, u32::MAX, 3) != 0 {
        return Err("a futex wake woke a waiter on another address space's word");
    }
    if futex::wake::<P>(SHARED, u32::MAX, 3) != 0 {
        return Err("a shared futex wake woke private waiters on the same word");
    }
    if futex::wake::<P>(KEY, 0b100, 0) != 0 {
        return Err("a futex wake woke more waiters than its limit");
    }
    if futex::wake::<P>(KEY, u32::MAX, 2) != 2 {
        return Err("a futex wake did not wake the two waiters it was allowed");
    }
    if futex::wake::<P>(KEY, 0b011, 2) != 0 {
        return Err("futex waiters were not woken in the order they began to wait");
    }
    if futex::wake::<P>(KEY, 0b100, 2) != 1 {
        return Err("a futex wake missed the waiter whose bitset it named");
    }
    for waiter in waiters {
        waiter.join::<P>();
    }
    if futex::wait::<P>(KEY, u32::MAX, u64::MAX, || Err(linux::Errno::EAGAIN))
        != Err(linux::Errno::EAGAIN)
    {
        return Err("a futex wait did not end with the word's error");
    }
    let deadline = P::clock_ticks() + millisecond;
    if futex::wait::<P>(KEY, u32::MAX, deadline, || Ok(())) != Err(linux::Errno::ETIMEDOUT) {
        return Err("a futex wait with a deadline did not time out");
    }
    if P::clock_ticks() < deadline {
        return Err("a futex wait timed out before its deadline");
    }
    if futex::wake::<P>(KEY, u32::MAX, u32::MAX) != 0 {
        return Err("a futex waiter that timed out was left queued");
    }
    let parker = thread::Parker::new();
    // SAFETY: `parker` is in place until this function returns.
    unsafe { thread::Parker::unpark::<P>(&parker) };
    if parker.park_until::<P>(u64::MAX) != thread::ParkEnd::Unparked {
        return Err("an unpark before the park did not end it at once");
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
/// CPUs are checked by `secondary_cpus_start_and_report_in`. Then the CPU running it raises the
/// wake-up interrupt on itself ([`cpu::interrupt_here`]) and must take it once.
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
    let here = cpu::current::<P>();
    let before = here.wakeups();
    cpu::interrupt_here::<P>();
    let deadline = P::clock_ticks() + WAKEUP_TIMEOUT_MS * (P::clock_frequency() / 1000);
    while here.wakeups() == before && P::clock_ticks() < deadline {
        P::interrupt_window();
    }
    if here.wakeups() != before + 1 {
        return Err("a CPU did not take the wake-up interrupt it raised on itself");
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
        fn console_read(_buffer: &mut [u8]) -> usize {
            unreachable!("not used in tests")
        }
        fn enable_console_input() -> Result<(), &'static str> {
            unreachable!("not used in tests")
        }
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
        fn linux_hardware_capabilities() -> [u64; 2] {
            [0, 0]
        }
        fn random_seeds(_seed: &mut dyn FnMut(eggos_hal::random::SeedSource, &[u8])) {}
        fn make_instructions_visible(_start: usize, _length: usize) {}
        fn drop_user_translations(_start: u64, _length: u64) {}
        fn page_fault(
            kind: eggos_hal::ExceptionKind,
            code: u64,
        ) -> Option<eggos_hal::paging::PageFault> {
            crate::tests::page_fault(kind, code)
        }
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
        unsafe fn set_user_fp(_on: bool) {
            unreachable!("not used in tests")
        }
        unsafe fn save_fp(_state: *mut eggos_hal::fp::FpState) {
            unreachable!("not used in tests")
        }
        unsafe fn restore_fp(_state: *const eggos_hal::fp::FpState) {
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
        fn console_read(_buffer: &mut [u8]) -> usize {
            unreachable!("not used in tests")
        }
        fn enable_console_input() -> Result<(), &'static str> {
            unreachable!("not used in tests")
        }
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
        fn linux_hardware_capabilities() -> [u64; 2] {
            [0, 0]
        }
        fn random_seeds(_seed: &mut dyn FnMut(eggos_hal::random::SeedSource, &[u8])) {}
        fn make_instructions_visible(_start: usize, _length: usize) {}
        fn drop_user_translations(_start: u64, _length: u64) {}
        fn page_fault(
            kind: eggos_hal::ExceptionKind,
            code: u64,
        ) -> Option<eggos_hal::paging::PageFault> {
            crate::tests::page_fault(kind, code)
        }
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
        unsafe fn set_user_fp(_on: bool) {
            unreachable!("not used in tests")
        }
        unsafe fn save_fp(_state: *mut eggos_hal::fp::FpState) {
            unreachable!("not used in tests")
        }
        unsafe fn restore_fp(_state: *const eggos_hal::fp::FpState) {
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
