//! Allwinner's Display Engine 2 (D548), beside the Arm Mali-400 and 450 of the H3, H5 and A64:
//! the mixer's UI channel the firmware lit, taken over at its mode. A flip writes the channel's
//! top-field address and asks the mixer to apply its double-buffered registers
//! (`GLB_DBUFFER`), which it does at the next frame and then clears.

use crate::io::RegisterIo;

/// The device tree `compatible` strings of the mixers.
pub const COMPATIBLE: [&[u8]; 3] = [
    b"allwinner,sun50i-a64-de2-mixer-0",
    b"allwinner,sun50i-a64-de2-mixer-1",
    b"allwinner,sun8i-h3-de2-mixer-0",
];

const DOUBLE_BUFFER: usize = 0x08;
const APPLY: u32 = 1;
const UI_BASE: usize = 0x3000;
const UI_CHANNEL_BYTES: usize = 0x1000;
const CHANNELS_LOOKED_AT: usize = 4;
const ATTRIBUTES: usize = 0x00;
const ENABLED: u32 = 1 << 0;
const PITCH: usize = 0x0c;
const TOP_LOW_ADDRESS: usize = 0x10;
const TOP_HIGH_ADDRESS: usize = 0x80;
const HIGH_ADDRESS_MASK: u32 = 0xff;
const SIZE: usize = 0x88;
const SIZE_MASK: u32 = 0x1fff;

/// What the firmware left a UI channel showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LitLayer {
    pub channel: usize,
    pub address: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: u32,
}

/// One mixer, through its registers in `regs`.
#[derive(Debug)]
pub struct Mixer<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Mixer<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    fn channel(channel: usize, register: usize) -> usize {
        UI_BASE + channel * UI_CHANNEL_BYTES + register
    }

    /// The first enabled UI channel with a frame address, as the firmware lit it.
    pub fn lit_layer(&self) -> Option<LitLayer> {
        (0..CHANNELS_LOOKED_AT).find_map(|channel| {
            let attributes = self.regs.read32(Self::channel(channel, ATTRIBUTES));
            let low = self.regs.read32(Self::channel(channel, TOP_LOW_ADDRESS));
            if attributes & ENABLED == 0 || low == 0 {
                return None;
            }
            let high_place = Self::channel(channel, TOP_HIGH_ADDRESS);
            let high = self.regs.read32(high_place) & HIGH_ADDRESS_MASK;
            let size = self.regs.read32(Self::channel(channel, SIZE));
            Some(LitLayer {
                channel,
                address: u64::from(high) << 32 | u64::from(low),
                width: (size & SIZE_MASK) + 1,
                height: (size >> 16 & SIZE_MASK) + 1,
                stride: self.regs.read32(Self::channel(channel, PITCH)),
            })
        })
    }

    /// Shows the frame at `address` on `channel` from the next frame.
    pub fn flip(&self, channel: usize, address: u64) {
        self.regs
            .write32(Self::channel(channel, TOP_LOW_ADDRESS), address as u32);
        let high = (address >> 32) as u32 & HIGH_ADDRESS_MASK;
        self.regs
            .write32(Self::channel(channel, TOP_HIGH_ADDRESS), high);
        self.regs.write32(DOUBLE_BUFFER, APPLY);
    }

    /// Whether the mixer applied the last flip.
    pub fn flipped(&self) -> bool {
        self.regs.read32(DOUBLE_BUFFER) & APPLY == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn the_lit_channel_is_found_and_a_flip_applies_at_the_next_frame() {
        let regs = FakeRegs::default();
        regs.script_reads(0x4000, &[1]);
        regs.script_reads(0x4010, &[0x4800_0000]);
        regs.script_reads(0x4088, &[719 << 16 | 1279]);
        regs.script_reads(0x400c, &[5120]);
        let mixer = Mixer::new(regs);
        let lit = mixer.lit_layer().unwrap();
        assert_eq!(
            lit,
            LitLayer {
                channel: 1,
                address: 0x4800_0000,
                width: 1280,
                height: 720,
                stride: 5120,
            }
        );
        mixer.flip(1, 0x4900_0000);
        assert_eq!(
            mixer.regs.writes(),
            vec![(0x4010, 0x4900_0000), (0x4080, 0), (0x08, 1)]
        );
    }
}
