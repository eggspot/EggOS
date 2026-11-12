//! A display engine's plane shown from two framebuffers in turn (D548): the kernel draws into
//! ordinary memory, and a flush copies the damage into whichever buffer the engine is not
//! showing, through a write-combining window, and flips the plane to it, so a frame is never
//! shown half drawn. The first buffer is the one the firmware lit, the second the kernel's own.

use super::{Mode, Rect, Scanout};
use crate::devices::Deadline;
use alloc::vec;
use alloc::vec::Vec;

const MAX_PENDING: usize = 16;

/// A display engine that shows a frame at a device address and says when it does.
pub trait FlipEngine: Send {
    /// Shows the frame at `address` from the next frame on.
    fn flip(&mut self, address: u64);
    /// Whether the engine shows what the last flip named.
    fn flipped(&mut self) -> bool;
}

/// One of the two framebuffers: where the engine reads it, and where the CPU writes it.
#[derive(Debug, Clone, Copy)]
pub struct Buffer {
    pub address: u64,
    pub window: usize,
}

/// The scanout over a [`FlipEngine`].
pub struct FlippingScanout<E: FlipEngine> {
    engine: E,
    mode: Mode,
    stride: usize,
    frame: Vec<u32>,
    buffers: [Buffer; 2],
    front: usize,
    flipped_once: bool,
    missing: [Vec<Rect>; 2],
}

impl<E: FlipEngine> core::fmt::Debug for FlippingScanout<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FlippingScanout")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl<E: FlipEngine> FlippingScanout<E> {
    /// The scanout of `engine` showing `mode`, rows `stride` bytes apart, from `buffers[0]`
    /// (the firmware's, shown now) and `buffers[1]`. `None` when the frame's memory is not
    /// free.
    pub fn new(engine: E, mode: Mode, stride: usize, buffers: [Buffer; 2]) -> Option<Self> {
        let pixels = mode.width as usize * mode.height as usize;
        let mut frame = Vec::new();
        frame.try_reserve_exact(pixels).ok()?;
        frame.resize(pixels, 0);
        let whole = Rect {
            x: 0,
            y: 0,
            width: mode.width,
            height: mode.height,
        };
        Some(Self {
            engine,
            mode,
            stride,
            frame,
            buffers,
            front: 0,
            flipped_once: false,
            missing: [vec![whole], vec![whole]],
        })
    }

    fn copy(&self, buffer: usize, rect: Rect) {
        let rect = rect.clipped(self.mode.width, self.mode.height);
        for y in rect.y..rect.y + rect.height {
            let start = (y * self.mode.width + rect.x) as usize;
            let row = &self.frame[start..start + rect.width as usize];
            let at = self.buffers[buffer].window + y as usize * self.stride + rect.x as usize * 4;
            for (index, pixel) in row.iter().enumerate() {
                let place = (at + index * 4) as *mut u32;
                // SAFETY: `window` maps the whole buffer, `stride` bytes a row for
                // `mode.height` rows, write-combining, for this scanout alone; the rectangle is
                // clipped to the mode.
                unsafe { place.write_volatile(pixel.to_le()) };
            }
        }
    }

    fn remember(missing: &mut Vec<Rect>, rects: &[Rect]) {
        missing.extend_from_slice(rects);
        if missing.len() > MAX_PENDING
            && let Some(first) = missing.first().copied()
        {
            let all = missing.iter().fold(first, |all, rect| all.union(*rect));
            missing.clear();
            missing.push(all);
        }
    }
}

impl<E: FlipEngine> Scanout for FlippingScanout<E> {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn pixels(&mut self) -> &mut [u32] {
        &mut self.frame
    }

    fn flush(&mut self, rects: &[Rect], deadline: &mut Deadline) -> Result<usize, &'static str> {
        while self.flipped_once && !self.engine.flipped() {
            if !deadline.keep_waiting() {
                return Err("the display engine did not take the last frame");
            }
            core::hint::spin_loop();
        }
        let hidden = 1 - self.front;
        let missing = core::mem::take(&mut self.missing[hidden]);
        for rect in missing.iter().chain(rects) {
            self.copy(hidden, *rect);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        Self::remember(&mut self.missing[self.front], rects);
        self.engine.flip(self.buffers[hidden].address);
        self.front = hidden;
        self.flipped_once = true;
        Ok(rects.len())
    }
}
