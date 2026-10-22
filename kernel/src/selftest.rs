//! Boot-time self-tests: checks that only mean something on the real (or emulated) machine,
//! such as "did the architecture entry point zero `.bss`". The QEMU test runner reads the
//! verdict markers from the console.

use crate::HEAP;
use crate::memory::{self, FrameAllocator, MAX_ORDER, Memory};
use crate::sync::SpinLock;
use crate::{exception, interrupt, thread};
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format, MemoryType};

/// Printed when every self-test passed. The QEMU runner requires this exact line.
pub const PASS_MARKER: &str = "EGGOS-SELFTEST: PASS";
/// Printed when a self-test failed or the kernel panicked.
pub const FAIL_MARKER: &str = "EGGOS-SELFTEST: FAIL";
/// Printed, followed by the address, before the `qemu-test-fatal-exception` kernel writes its
/// read-only data. The QEMU runner requires the fault report to name that address.
pub const WRITE_PROTECT_PROBE: &str = "fatal exception test: writing read-only data at ";

/// A self-test: a name and a check of `context` (for the boot self-tests, the kernel's
/// [`Memory`]) returning `Err(reason)` on failure.
pub type SelfTest<C> = (&'static str, fn(&mut C) -> Result<(), &'static str>);

/// Every boot self-test for platform `P`, in run order.
pub fn all<P: Platform>() -> [SelfTest<Memory<P>>; 10] {
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
        ("threads_take_turns_and_free_their_stacks", |_| {
            threads_take_turns_and_free_their_stacks::<P>()
        }),
        ("threads_are_preempted_at_preemption_points", |_| {
            threads_are_preempted_at_preemption_points::<P>()
        }),
        ("threads_sleep_wait_and_run_tickless", |_| {
            threads_sleep_wait_and_run_tickless::<P>()
        }),
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
    let before = interrupt::timer_interrupts();
    let start = P::clock_ticks();
    let deadline = start + frequency / 1000;
    let give_up = start + frequency;
    P::set_timer_deadline(deadline);
    // Greater rather than exactly one more, as in `breakpoint_resumes`.
    while interrupt::timer_interrupts() <= before {
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
    let ticks_before = interrupt::timer_interrupts();
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
    if interrupt::timer_interrupts() == ticks_before {
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

    let interrupts_before = interrupt::timer_interrupts();
    thread::enable_preemption::<P>();
    let until = P::clock_ticks() + 3 * thread::TIME_SLICE_MS * millisecond;
    while P::clock_ticks() < until {
        thread::preemption_point::<P>();
    }
    thread::disable_preemption::<P>();
    if interrupt::timer_interrupts() != interrupts_before {
        return Err("a timer tick came while only one thread was runnable");
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
