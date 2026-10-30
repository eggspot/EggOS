//! What display drivers share: the framebuffer's mode, rectangles of pixels and the damage a
//! frame collects before it is flushed to the screen.
//!
//! A pixel is a `u32` `0x00RRGGBB` in the CPU's byte order, which on every little-endian
//! machine is the bytes `B, G, R, X` of virtio-gpu's `B8G8R8X8_UNORM`.

pub use eggos_terminal::geometry::{Damage, MAX_DAMAGE_RECTS, Rect};

/// Bytes per pixel of a framebuffer.
pub const BYTES_PER_PIXEL: u32 = 4;

/// The mode xtask asks QEMU's virtio GPU for, and the offscreen display's default.
pub const DEFAULT_MODE: Mode = Mode::new(1280, 800);

/// A display mode: the framebuffer's size in pixels, rows packed with no padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
}

impl Mode {
    /// `width` by `height` pixels.
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Bytes from the start of one row to the next.
    pub const fn stride(self) -> usize {
        self.width as usize * BYTES_PER_PIXEL as usize
    }

    /// Bytes of the whole framebuffer, `usize::MAX` when they do not fit in one.
    pub const fn bytes(self) -> usize {
        self.stride().saturating_mul(self.height as usize)
    }

    /// The whole screen.
    pub const fn rect(self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_s_size_saturates_instead_of_wrapping() {
        assert_eq!(Mode::new(1280, 800).stride(), 5120);
        assert_eq!(Mode::new(1280, 800).bytes(), 4_096_000);
        if usize::BITS == 64 {
            assert_eq!(Mode::new(u32::MAX, u32::MAX).bytes(), usize::MAX);
        }
    }
}
