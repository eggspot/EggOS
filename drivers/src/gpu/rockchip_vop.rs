//! Rockchip's VOP (D548), the RK3399's display controllers beside its Arm Mali-T860: window 0,
//! which the firmware lit, taken over at its mode. A flip writes the window's frame address and
//! `REG_CFG_DONE`, which the VOP applies at the next frame start; the raw frame-start interrupt
//! bit, cleared at the flip, says when that happened.

use crate::io::RegisterIo;

/// The device tree `compatible` strings of the RK3399's two VOPs.
pub const COMPATIBLE: [&[u8]; 2] = [b"rockchip,rk3399-vop-big", b"rockchip,rk3399-vop-lit"];

const CONFIG_DONE: usize = 0x0000;
const WINDOW_CONTROL: usize = 0x0030;
const WINDOW_ENABLED: u32 = 1 << 0;
const WINDOW_FORMAT_SHIFT: u32 = 1;
const WINDOW_FORMAT_MASK: u32 = 0x7;
const FORMAT_ARGB8888: u32 = 0;
const WINDOW_VIRTUAL_WIDTH: usize = 0x003c;
const VIRTUAL_WIDTH_MASK: u32 = 0x3fff;
const WINDOW_ADDRESS: usize = 0x0040;
const WINDOW_ACTIVE_SIZE: usize = 0x0048;
const SIZE_MASK: u32 = 0x1fff;
const INTERRUPT_CLEAR: usize = 0x0284;
const INTERRUPT_RAW_STATUS: usize = 0x028c;
const FRAME_START: u32 = 1 << 0;
const WRITE_MASK_SHIFT: u32 = 16;
const BYTES_PER_PIXEL: u32 = 4;

/// What the firmware left window 0 showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LitWindow {
    pub address: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: u32,
}

/// One VOP, through its registers in `regs`.
#[derive(Debug)]
pub struct Vop<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Vop<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    /// Window 0 as the firmware lit it: enabled, 32 bits a pixel; `None` otherwise.
    pub fn lit_window(&self) -> Option<LitWindow> {
        let control = self.regs.read32(WINDOW_CONTROL);
        let format = control >> WINDOW_FORMAT_SHIFT & WINDOW_FORMAT_MASK;
        if control & WINDOW_ENABLED == 0 || format != FORMAT_ARGB8888 {
            return None;
        }
        let size = self.regs.read32(WINDOW_ACTIVE_SIZE);
        let words = self.regs.read32(WINDOW_VIRTUAL_WIDTH) & VIRTUAL_WIDTH_MASK;
        let address = u64::from(self.regs.read32(WINDOW_ADDRESS));
        (address != 0).then_some(LitWindow {
            address,
            width: (size & SIZE_MASK) + 1,
            height: (size >> 16 & SIZE_MASK) + 1,
            stride: words * BYTES_PER_PIXEL,
        })
    }

    /// Shows the frame at `address` (below 4 GiB) from the next frame start.
    pub fn flip(&self, address: u64) {
        self.regs
            .write32(INTERRUPT_CLEAR, FRAME_START << WRITE_MASK_SHIFT | FRAME_START);
        self.regs.write32(WINDOW_ADDRESS, address as u32);
        self.regs.write32(CONFIG_DONE, 1);
    }

    /// Whether a frame started since the last flip, so the VOP shows what it named.
    pub fn flipped(&self) -> bool {
        self.regs.read32(INTERRUPT_RAW_STATUS) & FRAME_START != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn the_lit_window_is_read_and_a_flip_names_the_new_frame() {
        let regs = FakeRegs::default();
        regs.script_reads(WINDOW_CONTROL, &[1]);
        regs.script_reads(WINDOW_ACTIVE_SIZE, &[1079 << 16 | 1919]);
        regs.script_reads(WINDOW_VIRTUAL_WIDTH, &[1920]);
        regs.script_reads(WINDOW_ADDRESS, &[0x3e00_0000]);
        let vop = Vop::new(regs);
        let lit = vop.lit_window().unwrap();
        assert_eq!(
            lit,
            LitWindow {
                address: 0x3e00_0000,
                width: 1920,
                height: 1080,
                stride: 7680,
            }
        );
        vop.flip(0x3f00_0000);
        assert_eq!(
            vop.regs.writes(),
            vec![(0x284, 1 << 16 | 1), (0x40, 0x3f00_0000), (0, 1)]
        );
    }

    #[test]
    fn a_window_in_another_format_is_not_taken() {
        let regs = FakeRegs::default();
        regs.script_reads(WINDOW_CONTROL, &[1 | 1 << 1]);
        assert_eq!(Vop::new(regs).lit_window(), None);
    }
}
