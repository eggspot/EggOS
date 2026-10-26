//! RISC-V Platform-Level Interrupt Controller (RISC-V PLIC Specification 1.0.0): routes up to
//! 1023 device interrupt sources to hart contexts, each a hart in one privilege mode.
//!
//! The register block is up to 64 MiB, mostly per-context pages. [`Plic`] takes one
//! [`RegisterIo`] for the source priorities and enables, at the block's start, and one for a
//! single context's threshold and claim registers, so a kernel maps only the pages it uses.

use crate::io::RegisterIo;

const ENABLES: usize = 0x2000;
const ENABLES_PER_CONTEXT: usize = 0x80;
const CONTEXTS: usize = 0x20_0000;
const CONTEXT_STRIDE: usize = 0x1000;
const THRESHOLD: usize = 0x0;
const CLAIM_COMPLETE: usize = 0x4;
const MAX_SOURCE: u32 = 1023;
const MAX_CONTEXT: usize = 15871;
const NO_INTERRUPT: u32 = 0;

/// Bytes of the source-priority and enable area a [`Plic`] for `context` reaches, from the
/// block's start: up to and including the context's enable words.
pub const fn sources_and_enables_size(context: usize) -> usize {
    ENABLES + (context + 1) * ENABLES_PER_CONTEXT
}

/// Byte offset of `context`'s threshold and claim registers in the block.
pub const fn context_offset(context: usize) -> usize {
    CONTEXTS + context * CONTEXT_STRIDE
}

/// Size of one context's register page.
pub const CONTEXT_BLOCK_SIZE: usize = CONTEXT_STRIDE;

/// One hart context's view of the PLIC.
#[derive(Debug)]
pub struct Plic<R: RegisterIo> {
    sources: R,
    context_regs: R,
    context: usize,
}

impl<R: RegisterIo> Plic<R> {
    /// `sources` reaches the block from offset 0 for [`sources_and_enables_size`]`(context)`
    /// bytes, `context_regs` reaches [`CONTEXT_BLOCK_SIZE`] bytes at [`context_offset`].
    pub fn new(sources: R, context_regs: R, context: usize) -> Self {
        assert!(
            context <= MAX_CONTEXT,
            "PLIC context {context} out of range"
        );
        Self {
            sources,
            context_regs,
            context,
        }
    }

    /// Gives `source` (1-1023) `priority` (0 never interrupts) and lets it interrupt this
    /// context.
    pub fn enable(&self, source: u32, priority: u32) {
        assert!(
            (1..=MAX_SOURCE).contains(&source),
            "PLIC source {source} out of range"
        );
        let source = source as usize;
        self.sources.write32(source * 4, priority);
        let enable = ENABLES + self.context * ENABLES_PER_CONTEXT + source / 32 * 4;
        let others = self.sources.read32(enable);
        self.sources.write32(enable, others | 1 << (source % 32));
    }

    /// Lets this context take every source whose priority is above `threshold`.
    pub fn set_threshold(&self, threshold: u32) {
        self.context_regs.write32(THRESHOLD, threshold);
    }

    /// Claims the highest-priority pending source for this context, or `None` when another
    /// context claimed it first. Each claimed source must be passed to [`Self::complete`].
    pub fn claim(&self) -> Option<u32> {
        let source = self.context_regs.read32(CLAIM_COMPLETE);
        (source != NO_INTERRUPT).then_some(source)
    }

    /// Ends `source`'s claim, so its gateway forwards its next request.
    pub fn complete(&self, source: u32) {
        self.context_regs.write32(CLAIM_COMPLETE, source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    fn plic(context: usize) -> Plic<FakeRegs> {
        Plic::new(FakeRegs::default(), FakeRegs::default(), context)
    }

    #[test]
    fn enabling_sets_the_priority_and_only_this_contexts_bit() {
        let plic = plic(3);
        let enable = 0x2000 + 3 * 0x80;
        plic.sources.script_reads(enable, &[1 << 1]);
        plic.enable(10, 1);
        assert_eq!(
            plic.sources.writes(),
            vec![(40, 1), (enable, 1 << 1 | 1 << 10)]
        );
        assert!(plic.context_regs.writes().is_empty());
    }

    #[test]
    fn sources_above_31_use_the_next_enable_word() {
        let plic = plic(0);
        plic.enable(33, 7);
        assert_eq!(plic.sources.writes(), vec![(33 * 4, 7), (0x2004, 1 << 1)]);
    }

    #[test]
    fn claims_and_completes_through_the_context_registers() {
        let plic = plic(1);
        plic.context_regs.script_reads(CLAIM_COMPLETE, &[10, 0]);
        plic.set_threshold(0);
        assert_eq!(plic.claim(), Some(10));
        plic.complete(10);
        assert_eq!(plic.claim(), None);
        assert_eq!(
            plic.context_regs.writes(),
            vec![(THRESHOLD, 0), (CLAIM_COMPLETE, 10)]
        );
    }

    #[test]
    fn the_areas_a_context_reaches_are_sized_from_its_index() {
        assert_eq!(sources_and_enables_size(0), 0x2080);
        assert_eq!(sources_and_enables_size(5), 0x2000 + 6 * 0x80);
        assert_eq!(context_offset(5), 0x20_5000);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn source_zero_is_not_a_source() {
        plic(0).enable(0, 1);
    }
}
