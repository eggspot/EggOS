//! Qualcomm's MDP5 (D549), the display processor of the Snapdragon 400 to 820 generation beside
//! their Adreno GPUs: the source pipe the bootloader lit, taken over at its mode. Its registers
//! are counted from the `qcom,mdp5` node's base, whose first word is the hardware revision; from
//! revision 1.5 the control paths and pipes moved further out. A flip writes the pipe's frame
//! address and flushes the pipe, layer mixer 0 and the control path through control path 0's
//! `CTL_FLUSH`, whose bits clear once the next frame took them.

use crate::io::RegisterIo;

/// The device tree `compatible` string of the display processor's node.
pub const COMPATIBLE: &[u8] = b"qcom,mdp5";

const HARDWARE_REVISION: usize = 0x0;
const MAJOR_SHIFT: u32 = 28;
const MINOR_SHIFT: u32 = 16;
const MINOR_MASK: u32 = 0xfff;
const FIRST_WIDE_LAYOUT_MINOR: u32 = 5;
const SOURCE_SIZE: usize = 0x00;
const SOURCE_ADDRESS: usize = 0x14;
const SOURCE_STRIDE: usize = 0x24;
const STRIDE_MASK: u32 = 0xffff;
const SOURCE_FORMAT: usize = 0x30;
const BYTES_SHIFT: u32 = 9;
const BYTES_MASK: u32 = 0x3;
const FOUR_BYTES: u32 = 3;
const SIZE_MASK: u32 = 0xffff;
const CONTROL_FLUSH: usize = 0x18;
const FLUSH_MIXER_0: u32 = 1 << 6;
const FLUSH_CONTROL: u32 = 1 << 17;

/// The pipes the bootloader may have lit, with their flush bits.
const PIPES: [(Pipe, u32); 3] = [(Pipe::Vig0, 1 << 0), (Pipe::Rgb0, 1 << 3), (Pipe::Dma0, 1 << 11)];

/// A source pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pipe {
    Vig0,
    Rgb0,
    Dma0,
}

/// Where the blocks lie from the node's base: up to revision 1.4, or from 1.5 on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    control: usize,
    vig: usize,
    rgb: usize,
    dma: usize,
}

const NARROW: Layout = Layout {
    control: 0x500,
    vig: 0x1100,
    rgb: 0x1d00,
    dma: 0x2900,
};

const WIDE: Layout = Layout {
    control: 0x1000,
    vig: 0x4000,
    rgb: 0x14000,
    dma: 0x24000,
};

/// What the bootloader left a pipe showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LitPipe {
    pub pipe: Pipe,
    pub address: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: u32,
}

/// The display processor, through its registers in `regs`.
#[derive(Debug)]
pub struct Mdp5<R: RegisterIo> {
    regs: R,
    layout: Layout,
}

impl<R: RegisterIo> Mdp5<R> {
    /// The display processor behind `regs`; `None` when its revision is not an MDP5's.
    pub fn new(regs: R) -> Option<Self> {
        let revision = regs.read32(HARDWARE_REVISION);
        if revision >> MAJOR_SHIFT != 1 {
            return None;
        }
        let layout = match revision >> MINOR_SHIFT & MINOR_MASK {
            minor if minor >= FIRST_WIDE_LAYOUT_MINOR => WIDE,
            _ => NARROW,
        };
        Some(Self { regs, layout })
    }

    fn pipe_base(&self, pipe: Pipe) -> usize {
        match pipe {
            Pipe::Vig0 => self.layout.vig,
            Pipe::Rgb0 => self.layout.rgb,
            Pipe::Dma0 => self.layout.dma,
        }
    }

    /// The first of VIG0, RGB0 and DMA0 with a frame address and four bytes a pixel.
    pub fn lit_pipe(&self) -> Option<LitPipe> {
        PIPES.iter().find_map(|&(pipe, _)| {
            let base = self.pipe_base(pipe);
            let address = self.regs.read32(base + SOURCE_ADDRESS);
            let format = self.regs.read32(base + SOURCE_FORMAT);
            if address == 0 || format >> BYTES_SHIFT & BYTES_MASK != FOUR_BYTES {
                return None;
            }
            let size = self.regs.read32(base + SOURCE_SIZE);
            Some(LitPipe {
                pipe,
                address: u64::from(address),
                width: size & SIZE_MASK,
                height: size >> 16 & SIZE_MASK,
                stride: self.regs.read32(base + SOURCE_STRIDE) & STRIDE_MASK,
            })
        })
    }

    fn flush_bits(pipe: Pipe) -> u32 {
        let pipe_bit = PIPES
            .iter()
            .find_map(|&(candidate, bit)| (candidate == pipe).then_some(bit))
            .unwrap_or(0);
        pipe_bit | FLUSH_MIXER_0 | FLUSH_CONTROL
    }

    /// Shows the frame at `address` (below 4 GiB) on `pipe` from the next frame.
    pub fn flip(&self, pipe: Pipe, address: u64) {
        self.regs
            .write32(self.pipe_base(pipe) + SOURCE_ADDRESS, address as u32);
        self.regs
            .write32(self.layout.control + CONTROL_FLUSH, Self::flush_bits(pipe));
    }

    /// Whether the last flip on `pipe` was taken.
    pub fn flipped(&self, pipe: Pipe) -> bool {
        self.regs.read32(self.layout.control + CONTROL_FLUSH) & Self::flush_bits(pipe) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn a_wide_layout_s_lit_rgb_pipe_is_read_and_flipped() {
        let regs = FakeRegs::default();
        regs.script_reads(0, &[1 << 28 | 6 << 16]);
        regs.script_reads(0x4014, &[0]);
        regs.script_reads(0x4030, &[0]);
        regs.script_reads(0x14014, &[0x8e00_0000]);
        regs.script_reads(0x14030, &[0x0002_37ff]);
        regs.script_reads(0x14000, &[1280 << 16 | 720]);
        regs.script_reads(0x14024, &[2880]);
        let mdp = Mdp5::new(regs).unwrap();
        let lit = mdp.lit_pipe().unwrap();
        assert_eq!(
            lit,
            LitPipe {
                pipe: Pipe::Rgb0,
                address: 0x8e00_0000,
                width: 720,
                height: 1280,
                stride: 2880,
            }
        );
        mdp.flip(Pipe::Rgb0, 0x8f00_0000);
        assert_eq!(
            mdp.regs.writes(),
            vec![(0x14014, 0x8f00_0000), (0x1018, 1 << 3 | 1 << 6 | 1 << 17)]
        );
    }

    #[test]
    fn an_older_revision_uses_the_narrow_layout_and_a_non_mdp5_is_refused() {
        let regs = FakeRegs::default();
        regs.script_reads(0, &[1 << 28 | 2 << 16]);
        assert_eq!(Mdp5::new(regs).unwrap().layout, NARROW);
        let other = FakeRegs::default();
        other.script_reads(0, &[3 << 28]);
        assert!(Mdp5::new(other).is_none());
    }
}
