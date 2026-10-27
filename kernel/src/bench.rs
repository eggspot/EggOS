//! Boot-time benchmarks of the kernel's hot paths (kernel feature `qemu-bench`).
//!
//! Each benchmark runs its body a fixed number of times between two reads of the platform
//! clock and prints one [`MARKER`] line. `cargo xtask bench` boots the kernel in QEMU with
//! `-icount shift=0`, where the guest clock advances exactly 1 ns per instruction executed,
//! so the elapsed time is an instruction count: the same on every host and every run, which
//! is what lets CI compare it against a committed baseline (D27).

use crate::HEAP;
use crate::linux::Errno;
use crate::linux::exec;
use crate::linux::futex::{self, Key};
use crate::memory::{FRAMES, FrameAllocator, Memory};
use crate::process::{self, Process};
use crate::{cpu, input, linux, random, selftest, thread};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write;
use core::hint::black_box;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_drivers::display::{Damage, Mode, Rect};
use eggos_drivers::dma::DmaAddr;
use eggos_drivers::dma::DmaMemory;
use eggos_drivers::font::{self, GLYPH_HEIGHT, GLYPH_WIDTH};
use eggos_drivers::input::{Capabilities, RawEvent, SYN_REPORT, event_type};
use eggos_drivers::terminal::{TERM_BACKGROUND, TERM_FOREGROUND, Terminal};
use eggos_drivers::virtio::gpu::{self as virtio_gpu, VirtioGpu};
use eggos_drivers::virtio::input::{self as virtio_input, VirtioInput};
use eggos_drivers::virtio::queue::{self, Buffer, SplitQueue};
use eggos_drivers::virtio::rng as virtio_rng;
use eggos_drivers::virtio::{self, Transport};
use eggos_hal::fdt::Fdt;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;
use eggos_hal::user::{UserContext, UserExit};
use eggos_hal::{MemoryMap, PhysicalRange, Platform};

/// Starts every benchmark result line. The xtask bench runner parses these lines.
pub const MARKER: &str = "EGGOS-BENCH:";

/// Iterations timed per benchmark, after [`WARMUP_ITERATIONS`] untimed ones (which bring the
/// code into the caches and QEMU's translation-block cache).
pub const ITERATIONS: u64 = 10_000;
const WARMUP_ITERATIONS: u64 = 100;

/// A benchmark: a name and a body that runs the measured operation `n` times, with the
/// kernel's memory ([`Memory`]) for those that need its page tables.
pub type Benchmark<P> = (&'static str, fn(&mut Memory<P>, u64));

/// Every benchmark for platform `P`, in run order.
pub fn all<P: Platform>() -> [Benchmark<P>; 44] {
    [
        ("empty_loop", |_, n| empty_loop(n)),
        ("clock_read", |_, n| clock_read::<P>(n)),
        ("linux_clock_monotonic_read", |_, n| {
            linux_clock_monotonic_read::<P>(n)
        }),
        ("breakpoint_round_trip", |_, n| {
            breakpoint_round_trip::<P>(n)
        }),
        ("timer_arm", |_, n| timer_arm::<P>(n)),
        ("timer_interrupt_round_trip", |_, n| {
            timer_interrupt_round_trip::<P>(n)
        }),
        ("frame_allocate_untouched", |_, n| {
            frame_allocate_untouched(n)
        }),
        ("frame_free_allocate", |_, n| frame_free_allocate(n)),
        ("frame_contiguous_allocate_free", |_, n| {
            frame_contiguous_allocate_free(n)
        }),
        ("heap_allocate_free_small", |_, n| {
            heap_allocate_free_small(n)
        }),
        ("thread_yield_round_trip", |_, n| {
            thread_yield_round_trip::<P>(n)
        }),
        ("preemption_point_idle", |_, n| {
            preemption_point_idle::<P>(n)
        }),
        ("wait_queue_wake_empty", |_, n| {
            wait_queue_wake_empty::<P>(n)
        }),
        ("thread_wait_wake_round_trip", |_, n| {
            thread_wait_wake_round_trip::<P>(n)
        }),
        ("futex_wake_empty", |_, n| futex_wake_empty::<P>(n)),
        ("futex_wait_wake_round_trip", |_, n| {
            futex_wait_wake_round_trip::<P>(n)
        }),
        ("random_fill_16", |_, n| random_fill::<P, 16>(n)),
        ("random_fill_4096", |_, n| random_fill::<P, 4096>(n)),
        ("virtqueue_submit_one", |_, n| virtqueue_submit_one(n)),
        ("virtqueue_round_trip_fake", |_, n| {
            virtqueue_round_trip_fake(n)
        }),
        ("virtio_gpu_flush_fake", |_, n| virtio_gpu_flush_fake(n)),
        ("virtio_input_poll_idle", |_, n| virtio_input_poll_idle(n)),
        ("input_key_to_bytes", |_, n| input_key_to_bytes(n)),
        ("input_event_queue_push_pop", |_, n| {
            input_event_queue_push_pop(n)
        }),
        ("device_tree_walk", |_, n| device_tree_walk(n)),
        ("syscall_round_trip", syscall_round_trip::<P>),
        (
            "process_syscall_round_trip",
            process_syscall_round_trip::<P>,
        ),
        ("process_switch_round_trip", process_switch_round_trip::<P>),
        (
            "fp_process_switch_round_trip",
            fp_process_switch_round_trip::<P>,
        ),
        ("linux_syscall_round_trip", linux_syscall_round_trip::<P>),
        ("linux_exec_load", linux_exec_load::<P>),
        ("linux_execve", linux_execve::<P>),
        ("linux_page_fault", linux_page_fault::<P>),
        ("linux_mmap_munmap", linux_mmap_munmap::<P>),
        ("linux_dup3_close", linux_dup3_close::<P>),
        ("linux_descriptor_lookup", |_, n| linux_descriptor_lookup(n)),
        ("linux_openat_close", linux_openat_close::<P>),
        ("linux_file_read", linux_file_read::<P>),
        ("linux_fork", linux_fork::<P>),
        ("linux_vfork_exec", linux_vfork_exec::<P>),
        ("gfx_glyph_cell", |_, n| gfx_glyph_cell(n)),
        ("gfx_terminal_line", |_, n| gfx_terminal_line(n)),
        ("term_feed_line", |_, n| term_feed_line(n)),
        ("term_scroll_line", |_, n| term_scroll_line(n)),
    ]
}

/// Runs `benchmarks` and prints one line per benchmark:
/// `EGGOS-BENCH: <name> iterations=<n> ticks=<clock ticks> frequency=<clock Hz>`.
pub fn run<P: Platform, W: Write>(
    console: &mut W,
    benchmarks: &[Benchmark<P>],
    memory: &mut Memory<P>,
) -> core::fmt::Result {
    let frequency = P::clock_frequency();
    for (name, body) in benchmarks {
        body(memory, WARMUP_ITERATIONS);
        let start = P::clock_ticks();
        body(memory, ITERATIONS);
        let ticks = P::clock_ticks().wrapping_sub(start);
        writeln!(
            console,
            "{MARKER} {name} iterations={ITERATIONS} ticks={ticks} frequency={frequency}"
        )?;
    }
    Ok(())
}

/// The loop alone, so the per-iteration overhead every other result includes is visible.
fn empty_loop(n: u64) {
    for i in 0..n {
        black_box(i);
    }
}

fn clock_read<P: Platform>(n: u64) {
    for _ in 0..n {
        black_box(P::clock_ticks());
    }
}

/// What `clock_gettime(CLOCK_MONOTONIC)` and every other clock read cost beyond the system-call
/// round trip (D73): the clock read and its conversion to nanoseconds.
fn linux_clock_monotonic_read<P: Platform>(n: u64) {
    for _ in 0..n {
        black_box(linux::time::monotonic_nanoseconds::<P>());
    }
}

/// `N` bytes from the random number generator (D56): with 16, one draw, what `AT_RANDOM`
/// costs each program; with 4096, a stream from a draw's key, `getrandom`'s long path
/// without the copy to the program.
fn random_fill<P: Platform, const N: usize>(n: u64) {
    let mut bytes = [0u8; N];
    for _ in 0..n {
        random::fill::<P>(&mut bytes);
        black_box(&mut bytes);
    }
}

#[repr(C, align(4096))]
struct QueueMemory<const BYTES: usize>(core::cell::UnsafeCell<[u8; BYTES]>);

// SAFETY: only the virtqueue benchmarks reach the memory, through raw pointers, one at a time
// on the one CPU the benchmarks run on.
unsafe impl<const BYTES: usize> Sync for QueueMemory<BYTES> {}

const SUBMIT_QUEUE_SIZE: u16 = 16384;
const ROUND_TRIP_QUEUE_SIZE: u16 = virtio_rng::QUEUE_SIZE;
const ROUND_TRIP_BUFFER: usize = 1024;
const ROUND_TRIP_BYTES: usize = 32;

static SUBMIT_QUEUE: QueueMemory<{ queue::memory_size(SUBMIT_QUEUE_SIZE) }> = QueueMemory(
    core::cell::UnsafeCell::new([0; queue::memory_size(SUBMIT_QUEUE_SIZE)]),
);
static ROUND_TRIP_QUEUE: QueueMemory<4096> = QueueMemory(core::cell::UnsafeCell::new([0; 4096]));

fn virtqueue_submit_one(n: u64) {
    let memory = SUBMIT_QUEUE.0.get().cast::<u8>();
    let new_queue = || {
        // SAFETY: the static holds `memory_size(SUBMIT_QUEUE_SIZE)` bytes, page-aligned, which
        // only this benchmark uses; with no device, its address stands for the device's.
        unsafe { SplitQueue::new(memory, DmaAddr::new(memory as u64), SUBMIT_QUEUE_SIZE) }
    };
    let Ok(mut queue) = new_queue() else {
        return;
    };
    let request = [Buffer {
        addr: DmaAddr::new(0x1000),
        len: 64,
        device_writes: true,
    }];
    for _ in 0..n {
        if queue.free_descriptors() == 0 {
            let Ok(fresh) = new_queue() else {
                return;
            };
            queue = fresh;
        }
        black_box(queue.submit(black_box(&request)).ok());
        black_box(queue.should_notify());
    }
}

fn virtqueue_round_trip_fake(n: u64) {
    let memory = ROUND_TRIP_QUEUE.0.get().cast::<u8>();
    let buffer = memory.wrapping_add(ROUND_TRIP_BUFFER);
    // SAFETY: the static holds 4096 page-aligned bytes, the queue's `memory_size` of them first
    // and the request buffer after them, which only this benchmark uses; the fake device reaches
    // them at the CPU's addresses.
    let queue =
        unsafe { SplitQueue::new(memory, DmaAddr::new(memory as u64), ROUND_TRIP_QUEUE_SIZE) };
    let Ok(mut queue) = queue else {
        return;
    };
    // SAFETY: the same ring, set up just above; the device side lives no longer than the queue.
    let mut device = unsafe { queue::DeviceSide::new(memory, ROUND_TRIP_QUEUE_SIZE) };
    let request = [Buffer {
        addr: DmaAddr::new(buffer as u64),
        len: ROUND_TRIP_BYTES as u32,
        device_writes: true,
    }];
    let mut bytes = [0u8; ROUND_TRIP_BYTES];
    for i in 0..n {
        if queue.submit(&request).is_err() {
            return;
        }
        black_box(queue.should_notify());
        if let Some(head) = device.next_available() {
            let (addr, len, _, _) = device.descriptor(head);
            for offset in 0..len as usize {
                let byte = (addr.get() as *mut u8).wrapping_add(offset);
                // SAFETY: the request buffer the driver posted, inside the static.
                unsafe { byte.write_volatile(i as u8) };
            }
            device.put_used(head, len);
        }
        let Some(used) = queue.pop_used() else {
            return;
        };
        for (offset, byte) in bytes.iter_mut().take(used.len as usize).enumerate() {
            // SAFETY: the request buffer, which the fake device handed back.
            *byte = unsafe { buffer.wrapping_add(offset).read_volatile() };
        }
        black_box(&mut bytes);
    }
}

static GPU_CONTROL: QueueMemory<{ virtio_gpu::MEMORY_BYTES }> =
    QueueMemory(core::cell::UnsafeCell::new([0; virtio_gpu::MEMORY_BYTES]));
const GPU_MODE: Mode = Mode::new(64, 64);
static GPU_FRAMEBUFFER: QueueMemory<{ GPU_MODE.bytes() }> =
    QueueMemory(core::cell::UnsafeCell::new([0; GPU_MODE.bytes()]));
const GPU_DAMAGE: Rect = Rect::new(8, 8, 16, 16);

struct StaticDma {
    memory: *mut u8,
    len: usize,
}

impl StaticDma {
    fn of<const BYTES: usize>(memory: &'static QueueMemory<BYTES>) -> Self {
        Self {
            memory: memory.0.get().cast(),
            len: BYTES,
        }
    }
}

impl core::fmt::Debug for StaticDma {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("StaticDma")
    }
}

// SAFETY: made only from a page-aligned static of `len` bytes that one benchmark at a time uses
// on the one CPU the benchmarks run on; the fake device reaches it at the CPU's addresses,
// coherently.
unsafe impl DmaMemory for StaticDma {
    fn as_mut_ptr(&self) -> *mut u8 {
        self.memory
    }
    fn dma_addr(&self) -> DmaAddr {
        DmaAddr::new(self.memory as u64)
    }
    fn len(&self) -> usize {
        self.len
    }
}

#[derive(Debug, Default)]
struct FakeGpu {
    status: core::cell::Cell<u8>,
    device: core::cell::RefCell<Option<queue::DeviceSide>>,
}

const GPU_GET_DISPLAY_INFO: u32 = 0x0100;
const GPU_OK_NODATA: u32 = 0x1100;
const GPU_OK_DISPLAY_INFO: u32 = 0x1101;

impl FakeGpu {
    fn put32(address: u64, value: u32) {
        // SAFETY: a response buffer the driver posted, inside one of the statics.
        unsafe { (address as *mut u32).write_volatile(value.to_le()) }
    }
}

impl Transport for FakeGpu {
    fn device_type(&self) -> u32 {
        virtio::device_type::GPU
    }
    fn status(&self) -> u8 {
        self.status.get()
    }
    fn set_status(&self, status: u8) {
        self.status.set(status);
    }
    fn device_features(&self) -> u64 {
        virtio::feature::VERSION_1
    }
    fn set_driver_features(&self, _: u64) {}
    fn max_queue_size(&self, queue: u16) -> u16 {
        if queue == 0 {
            virtio_gpu::QUEUE_SIZE
        } else {
            0
        }
    }
    fn set_up_queue(
        &self,
        _: u16,
        size: u16,
        addresses: &virtio::QueueAddresses,
    ) -> Result<(), virtio::Error> {
        let memory = addresses.descriptors.get() as *mut u8;
        // SAFETY: the queue the driver set up at the start of `GPU_CONTROL`, which outlives
        // this device.
        *self.device.borrow_mut() = Some(unsafe { queue::DeviceSide::new(memory, size) });
        Ok(())
    }
    fn notify(&self, _: u16) {
        let mut device = self.device.borrow_mut();
        let Some(device) = device.as_mut() else {
            return;
        };
        while let Some(head) = device.next_available() {
            let (request, _, _, next) = device.descriptor(head);
            let (response, len, _, _) = device.descriptor(next);
            // SAFETY: the request the driver posted, inside `GPU_CONTROL`.
            let command = unsafe { (request.get() as *const u32).read_volatile() };
            if u32::from_le(command) == GPU_GET_DISPLAY_INFO {
                Self::put32(response.get(), GPU_OK_DISPLAY_INFO);
                Self::put32(response.get() + 32, GPU_MODE.width);
                Self::put32(response.get() + 36, GPU_MODE.height);
                Self::put32(response.get() + 40, 1);
            } else {
                Self::put32(response.get(), GPU_OK_NODATA);
            }
            device.put_used(head, len);
        }
    }
    fn read_config8(&self, _: usize) -> u8 {
        0
    }
    fn write_config8(&self, _: usize, _: u8) {}
    fn read_config32(&self, _: usize) -> u32 {
        1
    }
}

fn virtio_gpu_flush_fake(n: u64) {
    let control = StaticDma::of(&GPU_CONTROL);
    let Ok(mut gpu) = VirtioGpu::new(FakeGpu::default(), control, &mut || true) else {
        panic!("virtio_gpu_flush_fake: the fake GPU could not be set up");
    };
    let framebuffer = StaticDma::of(&GPU_FRAMEBUFFER);
    if gpu
        .attach_framebuffer(framebuffer, GPU_MODE, &mut || true)
        .is_err()
    {
        panic!("virtio_gpu_flush_fake: the fake GPU refused the framebuffer");
    }
    for _ in 0..n {
        black_box(gpu.flush(black_box(&[GPU_DAMAGE]), &mut || true).ok());
    }
}

static INPUT_MEMORY: QueueMemory<{ virtio_input::MEMORY_BYTES }> =
    QueueMemory(core::cell::UnsafeCell::new([0; virtio_input::MEMORY_BYTES]));

#[derive(Debug, Default)]
struct IdleInputDevice {
    status: core::cell::Cell<u8>,
}

impl Transport for IdleInputDevice {
    fn device_type(&self) -> u32 {
        virtio::device_type::INPUT
    }
    fn status(&self) -> u8 {
        self.status.get()
    }
    fn set_status(&self, status: u8) {
        self.status.set(status);
    }
    fn device_features(&self) -> u64 {
        virtio::feature::VERSION_1
    }
    fn set_driver_features(&self, _: u64) {}
    fn max_queue_size(&self, queue: u16) -> u16 {
        if queue == 0 {
            virtio_input::QUEUE_SIZE
        } else {
            0
        }
    }
    fn set_up_queue(
        &self,
        _: u16,
        _: u16,
        _: &virtio::QueueAddresses,
    ) -> Result<(), virtio::Error> {
        Ok(())
    }
    fn notify(&self, _: u16) {}
    fn read_config8(&self, _: usize) -> u8 {
        0
    }
    fn write_config8(&self, _: usize, _: u8) {}
    fn read_config32(&self, _: usize) -> u32 {
        0
    }
}

fn virtio_input_poll_idle(n: u64) {
    let memory = StaticDma::of(&INPUT_MEMORY);
    let Ok(mut device) = VirtioInput::new(IdleInputDevice::default(), memory, &mut || true) else {
        panic!("virtio_input_poll_idle: the fake input device could not be set up");
    };
    for _ in 0..n {
        black_box(device.poll(black_box(16), &mut |event| {
            black_box(event);
        }));
    }
}

struct DiscardingSink;

impl input::InputSink for DiscardingSink {
    fn bytes(&mut self, bytes: &[u8]) {
        black_box(bytes);
    }
    fn room(&self) -> usize {
        usize::MAX
    }
    fn event(&mut self, event: input::InputEvent) {
        black_box(event);
    }
    fn view(&mut self, change: crate::terminal::ViewMove) {
        black_box(change);
    }
}

fn input_key_to_bytes(n: u64) {
    let mut state = input::InputState::new();
    let keyboard = Capabilities {
        keyboard: true,
        ..Capabilities::default()
    };
    let mut report = input::PendingReport::default();
    let timing = input::Timing {
        now: 0,
        repeat_delay: 400,
        repeat_interval: 40,
    };
    let syn = RawEvent::new(event_type::SYN, SYN_REPORT, 0);
    for i in 0..n {
        let code = 16 + (i % 10) as u16;
        for event in [
            RawEvent::new(event_type::KEY, code, 1),
            syn,
            RawEvent::new(event_type::KEY, code, 0),
            syn,
        ] {
            state.translate(
                black_box(event),
                &keyboard,
                &mut report,
                None,
                timing,
                &mut DiscardingSink,
            );
        }
    }
}

static BENCH_EVENTS: input::EventQueue<{ input::EVENT_QUEUE_CAPACITY }> = input::EventQueue::new();

fn input_event_queue_push_pop(n: u64) {
    for i in 0..n {
        let event = input::InputEvent::PointerMove {
            x: i as u32 & 1023,
            y: 7,
        };
        black_box(BENCH_EVENTS.push(black_box(event)));
        black_box(BENCH_EVENTS.pop());
    }
}

const BENCH_COLUMNS: u16 = 80;
const BENCH_ROWS: u16 = 24;
const BENCH_STRIDE: usize = BENCH_COLUMNS as usize * GLYPH_WIDTH;

fn bench_terminal(rows: u16) -> Terminal {
    let Some(mut terminal) = Terminal::new(BENCH_COLUMNS, rows) else {
        panic!("terminal benchmark: no memory for an 80-column terminal");
    };
    terminal.set_onlcr(true);
    terminal
}

fn gfx_glyph_cell(n: u64) {
    let mut pixels = alloc::vec![0u32; BENCH_STRIDE * GLYPH_HEIGHT];
    for i in 0..n {
        let column = (i % u64::from(BENCH_COLUMNS)) as usize;
        font::draw_glyph(
            black_box(&mut pixels),
            BENCH_STRIDE,
            column * GLYPH_WIDTH,
            b'!' + (i % 90) as u8,
            TERM_FOREGROUND,
            TERM_BACKGROUND,
            false,
        );
    }
}

fn gfx_terminal_line(n: u64) {
    let mut terminal = bench_terminal(1);
    terminal.feed(b"\x1b[?25l");
    terminal.feed(&[b'x'; BENCH_COLUMNS as usize]);
    let mut pixels = alloc::vec![0u32; BENCH_STRIDE * GLYPH_HEIGHT];
    let mut damage = Damage::new();
    for _ in 0..n {
        terminal.invalidate();
        terminal.render(black_box(&mut pixels), BENCH_STRIDE, 0, 0, &mut damage);
        damage.clear();
    }
}

const SGR_LINE: &[u8] = b"\x1b[1;32meggsh\x1b[0m:\x1b[34m/usr/share\x1b[0m# ls -l --color     README.md Cargo.toml docs kernel drivers hal\r";

fn term_feed_line(n: u64) {
    let mut terminal = bench_terminal(BENCH_ROWS);
    for _ in 0..n {
        terminal.feed(black_box(SGR_LINE));
    }
    black_box(terminal.cursor());
}

fn term_scroll_line(n: u64) {
    let mut terminal = bench_terminal(BENCH_ROWS);
    let mut pixels = alloc::vec![0u32; BENCH_STRIDE * BENCH_ROWS as usize * GLYPH_HEIGHT];
    let mut damage = Damage::new();
    terminal.feed(b"\x1b[24;1H");
    terminal.render(&mut pixels, BENCH_STRIDE, 0, 0, &mut damage);
    for _ in 0..n {
        damage.clear();
        terminal.feed(black_box(b"\nscrolled"));
        terminal.render(black_box(&mut pixels), BENCH_STRIDE, 0, 0, &mut damage);
    }
}

const DEVICE_TREE: &[u8] = include_bytes!("../../hal/src/fdt/testdata/qemu-aarch64-virt.dtb");

fn device_tree_walk(n: u64) {
    for _ in 0..n {
        let Ok(tree) = Fdt::new(black_box(DEVICE_TREE)) else {
            return;
        };
        let devices = tree
            .nodes()
            .filter_map(Result::ok)
            .filter(|node| node.enabled && !node.compatible.is_empty())
            .count();
        black_box(devices);
    }
}

/// Exception entry, the kernel's handler and the return: the path syscalls and page faults
/// will take (D25).
fn breakpoint_round_trip<P: Platform>(n: u64) {
    for _ in 0..n {
        P::breakpoint();
    }
}

fn timer_arm<P: Platform>(n: u64) {
    let far = P::clock_ticks()
        .saturating_add(P::clock_frequency().saturating_mul(3600))
        .min(u64::MAX - n);
    for i in 0..n {
        P::set_timer_deadline(far + i);
    }
}

fn timer_interrupt_round_trip<P: Platform>(n: u64) {
    let this = cpu::current::<P>();
    for _ in 0..n {
        let before = this.unseen_by_scheduler();
        P::set_timer_deadline(0);
        while this.unseen_by_scheduler() == before {
            P::wait_for_interrupt();
        }
    }
}

/// Allocating never-used frames, the path every frame takes first. Untouched frames are not
/// written, so the range can be any addresses. Twice the frames allocated, so the buddy
/// allocator's bitmap (taken from the top, never written here) leaves `n` untouched frames.
fn frame_allocate_untouched(n: u64) {
    let mut map = MemoryMap::new();
    map.add_ram(PhysicalRange::from_start_length(
        1 << 40,
        2 * n * FRAME_SIZE,
    ));
    let mut frames = FrameAllocator::new(&map, 0);
    for _ in 0..n {
        black_box(frames.allocate());
    }
}

/// Freeing a frame and allocating it again: the free stack's push and pop, the steady-state
/// path once memory has been recycled. The frame is a page of this function's stack, which
/// the allocator reaches at its own address (offset 0).
fn frame_free_allocate(n: u64) {
    #[repr(C, align(4096))]
    struct Page(core::mem::MaybeUninit<[u8; FRAME_SIZE as usize]>);
    let mut page = Page(core::mem::MaybeUninit::uninit());
    let mut map = MemoryMap::new();
    map.add_ram(PhysicalRange::from_start_length(
        (&raw mut page).expose_provenance() as u64,
        FRAME_SIZE,
    ));
    let mut frames = FrameAllocator::new(&map, 0);
    let Some(mut frame) = frames.allocate() else {
        return;
    };
    for _ in 0..n {
        // SAFETY: `frame` came from `frames` and nothing uses it; it is `page`, which lives
        // until the end of this function, so the allocator may write its link there.
        unsafe { frames.free(black_box(frame)) };
        frame = frames.allocate().unwrap_or(frame);
    }
    black_box(&mut page);
}

/// Allocating and freeing a block of 4 contiguous frames from the kernel's frame allocator: the
/// buddy allocator's split on allocation and merge on free (D31), the path of heap allocations
/// larger than a frame. Runs after the self-tests, so the allocator is in place; without one,
/// nothing is timed.
fn frame_contiguous_allocate_free(n: u64) {
    let mut frames = FRAMES.lock();
    let Some(frames) = frames.as_mut() else {
        return;
    };
    for _ in 0..n {
        if let Some(block) = frames.allocate_contiguous(black_box(2)) {
            // SAFETY: allocated just above with order 2 and not used.
            unsafe { frames.free_contiguous(black_box(block), 2) };
        }
    }
}

/// Allocating and freeing a small block through the kernel's global heap: taking and releasing
/// its lock around a size class's free-list pop and push, the steady state of `Box` and small
/// `Vec`s. Runs after the self-tests, so the frame allocator is in place; without one every
/// allocation fails and only the failure path is timed.
fn heap_allocate_free_small(n: u64) {
    let layout = Layout::new::<[u64; 4]>();
    for _ in 0..n {
        // SAFETY: the layout has a non-zero size.
        let pointer = unsafe { HEAP.alloc(black_box(layout)) };
        if !pointer.is_null() {
            // SAFETY: allocated just above with `layout` and not used.
            unsafe { HEAP.dealloc(black_box(pointer), layout) };
        }
    }
}

/// The boot thread and one other passing the CPU back and forth through `yield_now`: per
/// iteration two switches (D35), each with the scheduler's queue rotation and a stack canary
/// check. Includes starting and joining the thread once per run, a few hundred instructions
/// spread over the iterations. Without a frame allocator for the stack, nothing is timed.
fn thread_yield_round_trip<P: Platform>(n: u64) {
    let Ok(other) = thread::spawn::<P, _>(move || {
        for _ in 0..n {
            thread::yield_now::<P>();
        }
    }) else {
        return;
    };
    for _ in 0..n {
        thread::yield_now::<P>();
    }
    other.join::<P>();
}

/// A preemption point in a time slice that is not over: the window for a pending interrupt
/// and the check, which long-running kernel code pays per call (D37).
fn preemption_point_idle<P: Platform>(n: u64) {
    for _ in 0..n {
        thread::preemption_point::<P>();
    }
}

/// Waking a wait queue nobody waits in: what code that changes a condition pays when no
/// thread waits for it (D39).
fn wait_queue_wake_empty<P: Platform>(n: u64) {
    static QUEUE: thread::WaitQueue = thread::WaitQueue::new();
    for _ in 0..n {
        black_box(black_box(&QUEUE).wake_one::<P>());
    }
}

/// The boot thread and one other handing a turn back and forth through two wait queues: per
/// iteration each thread wakes the other and blocks until its turn comes, so two wakes, two
/// condition checks that fail and two that hold, and two switches (D39). Includes starting and
/// joining the thread once per run, as in `thread_yield_round_trip`.
fn thread_wait_wake_round_trip<P: Platform>(n: u64) {
    static TURN: AtomicU64 = AtomicU64::new(0);
    static BOOT: thread::WaitQueue = thread::WaitQueue::new();
    static OTHER: thread::WaitQueue = thread::WaitQueue::new();
    TURN.store(0, Ordering::Relaxed);
    let Ok(other) = thread::spawn::<P, _>(move || {
        for turn in 0..n {
            OTHER.wait_until::<P>(|| TURN.load(Ordering::Relaxed) == 2 * turn + 1);
            TURN.store(2 * turn + 2, Ordering::Relaxed);
            BOOT.wake_one::<P>();
        }
    }) else {
        return;
    };
    for turn in 0..n {
        TURN.store(2 * turn + 1, Ordering::Relaxed);
        OTHER.wake_one::<P>();
        BOOT.wait_until::<P>(|| TURN.load(Ordering::Relaxed) == 2 * turn + 2);
    }
    other.join::<P>();
}

fn futex_wake_empty<P: Platform>(n: u64) {
    const KEY: Key = Key::new(u32::MAX, 0x1000, false);
    for _ in 0..n {
        black_box(futex::wake::<P>(black_box(KEY), u32::MAX, 1));
    }
}

fn futex_wait_wake_round_trip<P: Platform>(n: u64) {
    const BOOT: Key = Key::new(u32::MAX, 0x1000, false);
    const OTHER: Key = Key::new(u32::MAX, 0x2000, false);
    static TURN: AtomicU64 = AtomicU64::new(0);
    fn wait_for_turn<P: Platform>(key: Key, turn: u64) {
        while TURN.load(Ordering::Relaxed) != turn {
            let _ = futex::wait::<P>(key, u32::MAX, u64::MAX, || {
                match TURN.load(Ordering::Relaxed) == turn {
                    true => Err(Errno::EAGAIN),
                    false => Ok(()),
                }
            });
        }
    }
    TURN.store(0, Ordering::Relaxed);
    let Ok(other) = thread::spawn::<P, _>(move || {
        for turn in 0..n {
            wait_for_turn::<P>(OTHER, 2 * turn + 1);
            TURN.store(2 * turn + 2, Ordering::Relaxed);
            futex::wake::<P>(BOOT, u32::MAX, 1);
        }
    }) else {
        return;
    };
    for turn in 0..n {
        TURN.store(2 * turn + 1, Ordering::Relaxed);
        futex::wake::<P>(OTHER, u32::MAX, 1);
        wait_for_turn::<P>(BOOT, 2 * turn + 2);
    }
    other.join::<P>();
}

/// A null system call from user mode and back (D47): the HAL's test program makes system call
/// 2 in a loop; each iteration is its `svc`/`ecall`/`syscall` into the kernel, the kernel
/// looking at why, and the return to the program. The program's address space stays active
/// throughout, as for a process making system calls. Setting the program up and freeing it is
/// timed too, amortized over the iterations.
fn syscall_round_trip<P: Platform>(memory: &mut Memory<P>, n: u64) {
    // SAFETY: `memory.page_tables` are the kernel's tables, active on this CPU, the only one
    // the benchmarks run on.
    let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
        .expect("the test program fits in memory");
    let mut context = Process::<P>::test_program_context(|program| program.null_syscalls);
    program.with_active(|| {
        let mut calls = 0;
        while calls < n {
            // SAFETY: interrupts are masked, as the kernel runs; the program's address space is
            // active, sharing the kernel's half; nothing here switches threads or address
            // spaces before `run_user` returns. An interrupt in between is not counted.
            if unsafe { P::run_user(&mut context) } == UserExit::Syscall {
                calls += 1;
            }
        }
    });
}

/// A null system call from a process's own thread (D48): `syscall_round_trip` plus what the
/// thread's loop adds per call, its preemption point, the check that the address space is
/// loaded and the call to its handler. Includes starting the process and its thread and
/// joining it once per run.
fn process_syscall_round_trip<P: Platform>(memory: &mut Memory<P>, n: u64) {
    // SAFETY: `memory.page_tables` are the kernel's tables, active on this CPU, the only one
    // the benchmarks run on.
    let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
        .expect("the test program fits in memory");
    let context = Process::<P>::test_program_context(|program| program.null_syscalls);
    let mut calls = 0;
    let Ok(thread) = process::spawn::<P, _>(Arc::new(program), context, move |_, _, exit| {
        if exit == UserExit::Syscall {
            calls += 1;
        }
        if calls < n {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    }) else {
        return;
    };
    thread.join::<P>();
}

/// An unknown Linux system call from a Linux program's thread (M6): `process_syscall_round_trip`
/// with the Linux layer's handler, whose dispatch answers `ENOSYS`, its cheapest path, as the
/// thread's handler. Includes starting the process and its thread and joining it once per run.
fn linux_syscall_round_trip<P: Platform>(memory: &mut Memory<P>, n: u64) {
    // SAFETY: as in `process_syscall_round_trip`.
    let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
        .expect("the test program fits in memory");
    let context = Process::<P>::test_program_context(|program| program.linux_unknown_syscalls);
    let mut linux_thread = Box::new(linux::thread::LinuxThread::first(&program));
    let mut calls = 0;
    let Ok(thread) =
        process::spawn::<P, _>(Arc::new(program), context, move |process, context, exit| {
            if exit == UserExit::Syscall {
                calls += 1;
            }
            if calls < n {
                linux::handle(process, &mut linux_thread, context, exit)
            } else {
                ControlFlow::Break(())
            }
        })
    else {
        return;
    };
    thread.join::<P>();
}

/// Starting a Linux program from its ELF image (D51): a new process, the self-test's image
/// with only the code `linux_start` needs (D62) loaded into it with the self-test's arguments
/// and environment (its code, data and `.bss` mapped, its stack's region recorded and its
/// arguments' page mapped and laid out), then the process freed. Each page is a frame of its
/// own, zeroed: 1 for the stack (the rest of it is mapped as the program touches it, D52), 6
/// for the program, and the tables, so the frames dominate.
fn linux_exec_load<P: Platform>(memory: &mut Memory<P>, n: u64) {
    let image = selftest::linux_start_only_image::<P>();
    let arguments = selftest::LINUX_START_ARGUMENTS.map(str::as_bytes);
    let environment = selftest::LINUX_START_ENVIRONMENT.map(str::as_bytes);
    for _ in 0..n {
        // SAFETY: as in `process_syscall_round_trip`.
        let mut process = unsafe { Process::<P>::new(&memory.page_tables) }
            .expect("there is memory for a process");
        let context = exec::load(&mut process, &image, b"/bench", &arguments, &environment)
            .expect("the self-test's image loads");
        black_box(context);
    }
}

fn linux_execve<P: Platform>(memory: &mut Memory<P>, n: u64) {
    const PROGRAM: &[u8] = b"/usr/libexec/eggos-selftest";
    let Ok(node) = crate::vfs::resolve(crate::vfs::root(), crate::vfs::root(), PROGRAM) else {
        return;
    };
    let image = node.read(0, node.attributes().size as usize);
    let mut count = [0u8; 20];
    let mut first = count.len();
    let mut left = n;
    loop {
        first -= 1;
        count[first] = b'0' + (left % 10) as u8;
        left /= 10;
        if left == 0 {
            break;
        }
    }
    let arguments: [&[u8]; 3] = [PROGRAM, b"exec-loop", &count[first..]];
    // SAFETY: as in `process_syscall_round_trip`.
    let mut process =
        unsafe { Process::<P>::new(&memory.page_tables) }.expect("there is memory for a process");
    let context = exec::load(&mut process, image, PROGRAM, &arguments, &[])
        .expect("the runtime's self-test program loads");
    let process = Arc::new(process);
    linux::spawn::<P>(process.clone(), context)
        .expect("there is memory for the program's thread")
        .join::<P>();
    assert_eq!(
        process.exit_status(),
        Some(process::ProcessExit::Exited(0)),
        "the exec loop ran to its end"
    );
}

fn linux_fork<P: Platform>(memory: &mut Memory<P>, n: u64) {
    run_process_loop(memory, b"fork-loop", n);
}

fn linux_vfork_exec<P: Platform>(memory: &mut Memory<P>, n: u64) {
    run_process_loop(memory, b"spawn-loop", n);
}

fn run_process_loop<P: Platform>(memory: &mut Memory<P>, mode: &[u8], n: u64) {
    const PROGRAM: &[u8] = b"/usr/libexec/eggos-selftest";
    let Ok(node) = crate::vfs::resolve(crate::vfs::root(), crate::vfs::root(), PROGRAM) else {
        return;
    };
    let image = node.read(0, node.attributes().size as usize);
    let mut count = [0u8; 20];
    let mut first = count.len();
    let mut left = n;
    loop {
        first -= 1;
        count[first] = b'0' + (left % 10) as u8;
        left /= 10;
        if left == 0 {
            break;
        }
    }
    let arguments: [&[u8]; 3] = [PROGRAM, mode, &count[first..]];
    // SAFETY: as in `process_syscall_round_trip`.
    let mut process =
        unsafe { Process::<P>::new(&memory.page_tables) }.expect("there is memory for a process");
    let context = exec::load(&mut process, image, PROGRAM, &arguments, &[])
        .expect("the runtime's self-test program loads");
    let process = Arc::new(process);
    linux::spawn::<P>(process.clone(), context)
        .expect("there is memory for the program's thread")
        .join::<P>();
    let session = process.id().get();
    while linux::process_table::TABLE.lock().in_session(session) > 0 {
        thread::yield_now::<P>();
    }
    assert_eq!(
        process.exit_status(),
        Some(process::ProcessExit::Exited(0)),
        "the process loop ran to its end"
    );
}

/// The pages a Linux program's page faults map, in chunks of this many per process.
const PAGE_FAULT_CHUNK: u64 = 512;

fn linux_page_fault<P: Platform>(memory: &mut Memory<P>, n: u64) {
    const TOP: u64 = 0x1000_0000;
    let mut left = n;
    while left > 0 {
        let pages = left.min(PAGE_FAULT_CHUNK);
        left -= pages;
        // SAFETY: as in `process_syscall_round_trip`.
        let mut program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
            .expect("the test program fits in memory");
        program
            .add_region(TOP - pages * FRAME_SIZE, TOP, Attributes::USER_READ_WRITE)
            .expect("the region is in user space");
        let start = Process::<P>::test_program_context(|program| program.stack_growth).pc();
        let context = UserContext::new(start, TOP - 16);
        let Ok(thread) = linux::spawn::<P>(Arc::new(program), context) else {
            return;
        };
        thread.join::<P>();
    }
}

/// `mmap` of 16 anonymous pages and `munmap` of them again (D53), as a program's allocator
/// reserves and returns memory: the arguments checked, the region placed top-down, inserted
/// and removed, the tables walked for pages to unmap (none: they were never touched), this
/// CPU's translations of the range dropped (the address space is active, as for the program's
/// own call) and the other CPUs' records looked at. Without the trap from user mode
/// (`linux_syscall_round_trip` has it).
fn linux_mmap_munmap<P: Platform>(memory: &mut Memory<P>, n: u64) {
    use linux::memory::{map, prot};
    const LENGTH: u64 = 16 * FRAME_SIZE;
    // SAFETY: as in `process_syscall_round_trip`.
    let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
        .expect("the test program fits in memory");
    let flags = map::PRIVATE | map::ANONYMOUS;
    program.with_active(|| {
        for _ in 0..n {
            let start =
                linux::memory::mmap(&program, 0, LENGTH, prot::READ | prot::WRITE, flags, 0)
                    .expect("16 pages fit");
            linux::memory::munmap(&program, core::hint::black_box(start), LENGTH)
                .expect("the pages were just mapped");
        }
    });
}

fn linux_descriptor_lookup(n: u64) {
    let files = linux::descriptors::Descriptors::with_console_streams();
    for _ in 0..n {
        let file = files
            .file(black_box(1))
            .expect("descriptor 1 is the console");
        black_box(file.is_writable());
    }
}

fn linux_dup3_close<P: Platform>(memory: &mut Memory<P>, n: u64) {
    use linux::descriptors::{self, Descriptors, open_flag};
    // SAFETY: as in `process_syscall_round_trip`.
    let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
        .expect("the test program fits in memory");
    let files = Descriptors::with_console_streams();
    let flags = u64::from(open_flag::CLOEXEC);
    for _ in 0..n {
        let fd = descriptors::dup3(&program, &files, 1, black_box(3), flags)
            .expect("descriptor 1 is open and 3 below the limit");
        descriptors::close(&files, black_box(fd)).expect("descriptor 3 was just opened");
    }
}

const FILE_BENCH_MEMORY: u64 = 0x1000_0000;

fn process_with_path<P: Platform>(memory: &Memory<P>, path: &[u8]) -> Process<P> {
    // SAFETY: as in `process_syscall_round_trip`.
    let mut program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
        .expect("the test program fits in memory");
    program
        .add_region(
            FILE_BENCH_MEMORY,
            FILE_BENCH_MEMORY + FRAME_SIZE,
            Attributes::USER_READ_WRITE,
        )
        .expect("the region is in user space");
    program
        .write_user_memory(FILE_BENCH_MEMORY, path)
        .expect("the region is writable");
    program
}

fn linux_openat_close<P: Platform>(memory: &mut Memory<P>, n: u64) {
    use linux::stat::at::FDCWD;
    let program = process_with_path(memory, b"/etc/hostname\0");
    let thread = linux::thread::LinuxThread::first(&program);
    for _ in 0..n {
        let fd = linux::files::openat(&program, &thread, FDCWD as u64, FILE_BENCH_MEMORY, 0, 0)
            .expect("/etc/hostname is in the initramfs");
        linux::descriptors::close(thread.files(), black_box(fd)).expect("it was just opened");
    }
}

fn linux_file_read<P: Platform>(memory: &mut Memory<P>, n: u64) {
    use linux::stat::at::FDCWD;
    let program = process_with_path(memory, b"/etc/motd\0");
    let thread = linux::thread::LinuxThread::first(&program);
    let fd = linux::files::openat(&program, &thread, FDCWD as u64, FILE_BENCH_MEMORY, 0, 0)
        .expect("/etc/motd is in the initramfs");
    for _ in 0..n {
        let read = linux::files::pread64(
            &program,
            thread.files(),
            black_box(fd),
            FILE_BENCH_MEMORY + 256,
            64,
            0,
        )
        .expect("the buffer is the program's");
        black_box(read);
    }
}

/// Two processes' threads taking turns on one CPU, each yielding at every null system call
/// (D48): per iteration two system calls, two thread switches and two address-space switches,
/// with what each switch costs on the CPU afterwards (the user translations it drops). Compare
/// with `process_syscall_round_trip` and `thread_yield_round_trip`. Includes starting and
/// joining both once per run.
fn process_switch_round_trip<P: Platform>(memory: &mut Memory<P>, n: u64) {
    let mut threads = [None, None];
    for thread in &mut threads {
        // SAFETY: as in `process_syscall_round_trip`.
        let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
            .expect("the test program fits in memory");
        let context = Process::<P>::test_program_context(|program| program.null_syscalls);
        let mut calls = 0;
        *thread = process::spawn::<P, _>(Arc::new(program), context, move |_, _, exit| {
            if exit == UserExit::Syscall {
                calls += 1;
                thread::yield_now::<P>();
            }
            if calls < n {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        })
        .ok();
    }
    for thread in threads.into_iter().flatten() {
        thread.join::<P>();
    }
}

/// `process_switch_round_trip` with programs that use their FP/SIMD registers (D54), each
/// checking every one after each call ([`TestProgram::fp_registers`]): per iteration also two
/// saves and two loads of FP/SIMD registers, and the program's checks. Compare with
/// `process_switch_round_trip`.
fn fp_process_switch_round_trip<P: Platform>(memory: &mut Memory<P>, n: u64) {
    let mut threads = [None, None];
    for (number, thread) in threads.iter_mut().enumerate() {
        // SAFETY: as in `process_syscall_round_trip`.
        let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
            .expect("the test program fits in memory");
        let context = Process::<P>::test_program_context(|program| program.fp_registers);
        let mut calls = 0;
        *thread = process::spawn::<P, _>(Arc::new(program), context, move |_, context, exit| {
            match exit {
                UserExit::Syscall
                    if context.syscall_number() == eggos_hal::user::TestProgram::SYSCALL_NULL =>
                {
                    calls += 1;
                    thread::yield_now::<P>();
                    context.set_syscall_result(0);
                }
                UserExit::Syscall => context.set_syscall_result(number as u64),
                UserExit::Exception(exception) if process::first_fp_use::<P>(exception.kind) => {}
                _ => return ControlFlow::Break(()),
            }
            if calls < n {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        })
        .ok();
    }
    for thread in threads.into_iter().flatten() {
        thread.join::<P>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interrupt;
    use core::sync::atomic::{AtomicU64, Ordering};
    use eggos_hal::{ExitStatus, Interrupt};

    struct CountingPlatform;

    static CLOCK: AtomicU64 = AtomicU64::new(0);

    impl Platform for CountingPlatform {
        const ARCH: &'static str = "counting";
        fn console_write(_bytes: &[u8]) {}
        fn console_read(_buffer: &mut [u8]) -> usize {
            unreachable!("not used in tests")
        }
        fn enable_console_input() -> Result<(), &'static str> {
            unreachable!("not used in tests")
        }
        fn exit(_status: ExitStatus) -> ! {
            unreachable!("not used in tests")
        }
        fn restart() -> ! {
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
            CLOCK.fetch_add(1, Ordering::Relaxed)
        }
        fn clock_frequency() -> u64 {
            1_000_000_000
        }
        fn set_timer_deadline(_deadline: u64) {}
        fn wait_for_interrupt() {
            interrupt::handle::<Self>(Interrupt::Timer);
        }
        fn interrupt_window() {}
        fn memory_map() -> MemoryMap {
            MemoryMap::new()
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
        unsafe fn switch_address_space(_roots: &[u64]) {}
        fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
            access()
        }
        unsafe fn run_user(
            context: &mut eggos_hal::user::UserContext,
        ) -> eggos_hal::user::UserExit {
            let pc = context.pc();
            if pc == crate::process::TEST_PROGRAM_ADDRESS + 2 {
                let page = context.stack() & !(FRAME_SIZE - 1);
                *context = eggos_hal::user::UserContext::new(pc, page - FRAME_SIZE);
                return context.simulate_exception(eggos_hal::Exception {
                    kind: eggos_hal::ExceptionKind::DataFault,
                    name: "test page fault",
                    code: crate::tests::FAULT_WRITE,
                    pc,
                    address: Some(page),
                });
            }
            let null = eggos_hal::user::TestProgram::SYSCALL_NULL;
            context.simulate_syscall(null, [0; 6], pc);
            UserExit::Syscall
        }
        fn user_test_program() -> eggos_hal::user::TestProgram {
            eggos_hal::user::TestProgram {
                code: &[0; 4],
                checks: 0,
                floating_point: 0,
                null_syscalls: 0,
                spin: 0,
                fp_registers: 0,
                illegal_instruction: 0,
                linux_hello: 0,
                linux_start: 0,
                linux_start_end: 4,
                linux_unknown_syscalls: 0,
                stack_growth: 2,
                linux_thread_pointer: 0,
                linux_poll: 0,
                linux_signals: 0,
                linux_signal_handler: 0,
                linux_fault_handler: 0,
                linux_signal_queue: 0,
                linux_signal_suspend: 0,
                linux_clock: 0,
                linux_stat: 0,
                linux_credentials: 0,
                linux_move_and_discard: 0,
            }
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
            entry: extern "C" fn(usize) -> !,
            argument: usize,
        ) -> usize {
            thread::host::prepare_thread(entry, argument)
        }
        unsafe fn switch_thread(save: *mut usize, resume: usize) {
            // SAFETY: the caller's guarantees are the host switch's.
            unsafe { thread::host::switch_thread(save, resume) }
        }
        unsafe fn set_user_fp(_on: bool) {}
        unsafe fn save_fp(_state: *mut eggos_hal::fp::FpState) {}
        unsafe fn restore_fp(_state: *const eggos_hal::fp::FpState) {}
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
            crate::tests::TestPlatform::cpu_local()
        }
        fn wake_cpu(_cpu: u64) {
            unreachable!("not used in tests")
        }
    }

    #[test]
    fn prints_one_parsable_line_per_benchmark() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        let mut memory = page_tables_for_test();
        let _scheduler = thread::host::lock_kernel_state_for_test();
        let mut out = String::new();
        let all = all::<CountingPlatform>();
        let (machine_only, terminal) = all[all.len() - 19..].split_at(15);
        let benchmarks = &[&all[..all.len() - 19], terminal].concat()[..];
        let [
            (syscall, _),
            (process_syscall, _),
            (process_switch, _),
            (fp_process_switch, _),
            (linux_syscall, _),
            (linux_exec, _),
            (linux_execve, _),
            (linux_page_fault, _),
            (linux_mmap_munmap, _),
            (linux_dup3_close, _),
            (linux_descriptor_lookup, _),
            (linux_openat_close, _),
            (linux_file_read, _),
            (linux_fork, _),
            (linux_vfork_exec, _),
        ] = machine_only
        else {
            unreachable!()
        };
        assert_eq!(
            terminal.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            [
                "gfx_glyph_cell",
                "gfx_terminal_line",
                "term_feed_line",
                "term_scroll_line"
            ]
        );
        assert_eq!(
            [
                *syscall,
                *process_syscall,
                *process_switch,
                *fp_process_switch,
                *linux_syscall,
                *linux_exec,
                *linux_execve,
                *linux_page_fault,
                *linux_mmap_munmap,
                *linux_dup3_close,
                *linux_descriptor_lookup,
                *linux_openat_close,
                *linux_file_read,
                *linux_fork,
                *linux_vfork_exec
            ],
            [
                "syscall_round_trip",
                "process_syscall_round_trip",
                "process_switch_round_trip",
                "fp_process_switch_round_trip",
                "linux_syscall_round_trip",
                "linux_exec_load",
                "linux_execve",
                "linux_page_fault",
                "linux_mmap_munmap",
                "linux_dup3_close",
                "linux_descriptor_lookup",
                "linux_openat_close",
                "linux_file_read",
                "linux_fork",
                "linux_vfork_exec"
            ]
        );
        assert_eq!(
            run::<CountingPlatform, _>(&mut out, benchmarks, &mut memory),
            Ok(())
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), benchmarks.len());
        for (line, (name, _)) in lines.iter().zip(benchmarks) {
            let prefix = format!("{MARKER} {name} iterations={ITERATIONS} ticks=");
            assert!(line.starts_with(&prefix), "{line}");
            assert!(line.ends_with(" frequency=1000000000"), "{line}");
        }
        let clock_line = lines[1];
        assert!(
            clock_line.contains(&format!("ticks={} ", ITERATIONS + 1)),
            "the clock is read once per iteration and once to end: {clock_line}"
        );
    }

    /// Kernel memory for the benchmarks: page tables, active (frozen) as the kernel's are.
    fn page_tables_for_test() -> Memory<CountingPlatform> {
        let _frames = thread::host::frames_for_test();
        let mut frames = FRAMES.lock();
        let mut page_tables = crate::paging::PageTables::new(frames.as_mut().unwrap()).unwrap();
        page_tables.mark_active();
        Memory { page_tables }
    }

    #[test]
    fn the_user_mode_benchmarks_run_their_programs_and_free_them() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        let _frames = thread::host::frames_for_test();
        let mut memory = {
            let mut frames = FRAMES.lock();
            let mut page_tables = crate::paging::PageTables::new(frames.as_mut().unwrap()).unwrap();
            page_tables.mark_active();
            Memory { page_tables }
        };
        thread::start::<crate::tests::TestPlatform>().unwrap();
        let free = || FRAMES.lock().as_ref().unwrap().free_frames();
        let before = free();
        syscall_round_trip::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        process_syscall_round_trip::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        process_switch_round_trip::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        fp_process_switch_round_trip::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        linux_syscall_round_trip::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        linux_exec_load::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        linux_page_fault::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        linux_dup3_close::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        linux_descriptor_lookup(5);
        assert_eq!(free(), before);
    }

    #[test]
    fn frame_benchmarks_run_on_the_host() {
        let _lock = crate::thread::host::lock_kernel_state_for_test();
        frame_allocate_untouched(ITERATIONS);
        frame_free_allocate(ITERATIONS);
        heap_allocate_free_small(ITERATIONS);
        frame_contiguous_allocate_free(ITERATIONS);
    }

    #[test]
    fn timer_interrupt_round_trip_waits_for_each_interrupt() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        let before = interrupt::timer_interrupts::<CountingPlatform>();
        timer_interrupt_round_trip::<CountingPlatform>(5);
        assert_eq!(
            interrupt::timer_interrupts::<CountingPlatform>(),
            before + 5
        );
    }
}
