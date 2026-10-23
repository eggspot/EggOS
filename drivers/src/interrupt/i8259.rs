//! The PC's legacy pair of Intel 8259A interrupt controllers (8259A datasheet).
//!
//! EggOS routes interrupts through the APICs, so the 8259s are only moved out of the way: the
//! firmware may leave them delivering the PC timer on vector 8, which the CPU also uses for
//! double faults. [`LegacyPic::remap_and_mask`] moves their 16 inputs to vectors the kernel
//! chooses and masks all of them.

use crate::io::RegisterIo;

const COMMAND: usize = 0;
const DATA: usize = 1;

const ICW1_INIT_CASCADE_EDGE_TRIGGERED_WITH_ICW4: u8 = 0x11;
const ICW3_PRIMARY_SECONDARY_ON_IRQ2: u8 = 1 << 2;
const ICW3_SECONDARY_CASCADE_IDENTITY: u8 = 2;
const ICW4_8086_MODE_NORMAL_END_OF_INTERRUPT: u8 = 0x01;
const OCW1_MASK_ALL_INPUTS: u8 = 0xff;
const OCW2_NON_SPECIFIC_END_OF_INTERRUPT: u8 = 0x20;

/// Each 8259's register block: two consecutive ports.
pub const REGISTER_BLOCK_SIZE: usize = 2;

/// The primary 8259 (inputs 0-7) and the secondary cascaded on its input 2 (inputs 8-15).
#[derive(Debug)]
pub struct LegacyPic<R: RegisterIo> {
    primary: R,
    secondary: R,
}

impl<R: RegisterIo> LegacyPic<R> {
    pub const fn new(primary: R, secondary: R) -> Self {
        Self { primary, secondary }
    }

    /// Re-initialises both controllers with inputs 0-7 on vectors `vector_base..+8` and
    /// inputs 8-15 on `vector_base + 8..+8`, and masks every input. `vector_base` must be a
    /// multiple of 8 (the 8259 ignores the low three bits).
    pub fn remap_and_mask(&self, vector_base: u8) {
        assert!(
            vector_base.is_multiple_of(8) && vector_base <= 0xf0,
            "8259 vector base {vector_base:#x} must be a multiple of 8 below 0xf8"
        );
        self.primary
            .write8(COMMAND, ICW1_INIT_CASCADE_EDGE_TRIGGERED_WITH_ICW4);
        self.secondary
            .write8(COMMAND, ICW1_INIT_CASCADE_EDGE_TRIGGERED_WITH_ICW4);
        self.primary.write8(DATA, vector_base);
        self.secondary.write8(DATA, vector_base + 8);
        self.primary.write8(DATA, ICW3_PRIMARY_SECONDARY_ON_IRQ2);
        self.secondary.write8(DATA, ICW3_SECONDARY_CASCADE_IDENTITY);
        self.primary
            .write8(DATA, ICW4_8086_MODE_NORMAL_END_OF_INTERRUPT);
        self.secondary
            .write8(DATA, ICW4_8086_MODE_NORMAL_END_OF_INTERRUPT);
        self.primary.write8(DATA, OCW1_MASK_ALL_INPUTS);
        self.secondary.write8(DATA, OCW1_MASK_ALL_INPUTS);
    }

    /// Ends a spurious interrupt that arrived through the secondary: the primary saw a real
    /// request on its cascade input and must be told it is over. (A spurious interrupt from
    /// the primary itself needs no end of interrupt.)
    pub fn end_secondary_spurious(&self) {
        self.primary
            .write8(COMMAND, OCW2_NON_SPECIFIC_END_OF_INTERRUPT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn remap_sends_the_init_sequence_then_masks_everything() {
        let pic = LegacyPic::new(FakeRegs::default(), FakeRegs::default());
        pic.remap_and_mask(0x20);
        assert_eq!(
            pic.primary.writes(),
            vec![
                (COMMAND, 0x11),
                (DATA, 0x20),
                (DATA, 0x04),
                (DATA, 0x01),
                (DATA, 0xff)
            ]
        );
        assert_eq!(
            pic.secondary.writes(),
            vec![
                (COMMAND, 0x11),
                (DATA, 0x28),
                (DATA, 0x02),
                (DATA, 0x01),
                (DATA, 0xff)
            ]
        );
    }

    #[test]
    #[should_panic(expected = "multiple of 8")]
    fn unaligned_vector_base_is_rejected() {
        LegacyPic::new(FakeRegs::default(), FakeRegs::default()).remap_and_mask(0x21);
    }

    #[test]
    fn secondary_spurious_ends_on_the_primary_only() {
        let pic = LegacyPic::new(FakeRegs::default(), FakeRegs::default());
        pic.end_secondary_spurious();
        assert_eq!(pic.primary.writes(), vec![(COMMAND, 0x20)]);
        assert!(pic.secondary.writes().is_empty());
    }
}
