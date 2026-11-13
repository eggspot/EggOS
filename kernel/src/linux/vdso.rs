//! The vDSO (D561): `eggos-vdso`'s image, built by `cargo xtask` and named to the kernel build by
//! `EGGOS_VDSO`, mapped read-only and executable into every program `execve` starts, right after
//! one read-only page of clock data the kernel keeps current (`ClockData`'s layout, shared with
//! the image). `AT_SYSINFO_EHDR` tells the C library where the image is. One copy of both serves
//! every program: their frames are lent to each process and never freed.

use crate::memory::FRAMES;
use crate::process::{LentFrames, Placement, Process};
use crate::sync::SpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;

#[cfg(eggos_vdso)]
static IMAGE: &[u8] = include_bytes!(env!("EGGOS_VDSO"));
#[cfg(not(eggos_vdso))]
static IMAGE: &[u8] = &[];

/// The frames: the clock data page first, then the image's pages.
#[derive(Debug)]
struct Pages {
    frames: Vec<u64>,
    offset: usize,
}

impl LentFrames for Pages {
    fn frame_at(&self, offset: u64) -> u64 {
        self.frames[(offset / FRAME_SIZE) as usize]
    }

    fn length(&self) -> u64 {
        self.frames.len() as u64 * FRAME_SIZE
    }

    fn writable(&self) -> bool {
        false
    }
}

static PAGES: SpinLock<Option<Arc<Pages>>> = SpinLock::new(None);

/// The clock data page's fields, at their offsets (`eggos_vdso::ClockData`).
const SEQUENCE: usize = 0;
const COUNTER_READABLE: usize = 4;
const NANOSECONDS_PER_TICK: usize = 8;
const REALTIME_OFFSET: usize = 16;

fn pages() -> Option<Arc<Pages>> {
    if IMAGE.is_empty() {
        return None;
    }
    let mut made = PAGES.lock();
    if let Some(pages) = made.as_ref() {
        return Some(Arc::clone(pages));
    }
    let count = 1 + IMAGE.len().div_ceil(FRAME_SIZE as usize);
    let mut frames = Vec::new();
    frames.try_reserve_exact(count).ok()?;
    let offset = {
        let mut allocator = FRAMES.lock();
        let allocator = allocator.as_mut()?;
        for _ in 0..count {
            let Some(frame) = allocator.allocate() else {
                for &frame in &frames {
                    // SAFETY: allocated just above and never used.
                    unsafe { allocator.free(frame) };
                }
                return None;
            };
            frames.push(frame);
        }
        allocator.physical_memory_offset()
    };
    for (index, &frame) in frames.iter().enumerate() {
        let page = (frame as usize).wrapping_add(offset) as *mut u8;
        // SAFETY: the frame was just allocated and is reached through the allocator's offset.
        unsafe { page.write_bytes(0, FRAME_SIZE as usize) };
        if index > 0 {
            let from = (index - 1) * FRAME_SIZE as usize;
            let bytes = &IMAGE[from..(from + FRAME_SIZE as usize).min(IMAGE.len())];
            // SAFETY: as above; the image's piece fits in the frame.
            unsafe { page.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
        }
    }
    let pages = Arc::new(Pages { frames, offset });
    *made = Some(Arc::clone(&pages));
    Some(pages)
}

fn data_word<T>(pages: &Pages, at: usize) -> *mut T {
    ((pages.frames[0] as usize).wrapping_add(pages.offset) + at) as *mut T
}

/// Writes the clock data programs read (D561): the counter's scale, the wall clock's offset and
/// whether programs may read the counter, under an odd sequence while it changes.
pub fn publish(nanoseconds_per_tick: u64, realtime_offset: i64, counter_readable: bool) {
    let Some(pages) = PAGES.lock().clone() else {
        return;
    };
    let sequence = data_word::<AtomicU32>(&pages, SEQUENCE);
    // SAFETY: the data page is the kernel's own frame, reached through the direct map; its
    // sequence word is aligned and only ever changed through these atomics.
    let sequence = unsafe { &*sequence };
    let started = sequence.fetch_add(1, Ordering::AcqRel);
    // SAFETY: as above: aligned words of the data page, which programs only read.
    unsafe {
        data_word::<u32>(&pages, COUNTER_READABLE).write_volatile(u32::from(counter_readable));
        data_word::<u64>(&pages, NANOSECONDS_PER_TICK).write_volatile(nanoseconds_per_tick);
        data_word::<i64>(&pages, REALTIME_OFFSET).write_volatile(realtime_offset);
    }
    sequence.store(started.wrapping_add(2), Ordering::Release);
}

/// Maps the clock data and the vDSO into `process` below `top`, returning the image's address for
/// `AT_SYSINFO_EHDR`; `None` without an image (an architecture without one) or without memory, when
/// programs make the system calls instead.
pub fn map_into<P: Platform>(process: &Process<P>, top: u64) -> Option<u64> {
    let first = PAGES.lock().is_none();
    let pages = pages()?;
    if first {
        crate::wall_clock::publish_to_vdso::<P>();
    }
    let length = pages.length();
    let start = process
        .map_lent_frames(
            Placement::Anywhere {
                hint: None,
                floor: super::exec::MINIMUM_ADDRESS,
                top,
            },
            length,
            Attributes::USER_TEXT,
            pages,
        )
        .ok()?;
    Some(start + FRAME_SIZE)
}
