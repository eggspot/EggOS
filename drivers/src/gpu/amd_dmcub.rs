//! AMD's display microcontroller, DMCUB, loaded on DCN 3.x (D514): the firmware's instructions
//! and constants, its data, a stack, a copy of the video BIOS, the mailbox, the trace buffer
//! and the firmware's state each get a region of video memory, which DMCUB reaches through
//! windows at fixed addresses of its own (instructions at `0x60000000`, stack `0x61000000`,
//! data right after the instructions, video BIOS `0x63000000`, mailbox `0x64000000`, trace
//! `0x65000000`, state `0x66000000`). Loading is: stop and reset it, set the instruction and
//! stack windows under the security reset ("backdoor" loading, from video memory), set the
//! other windows and the inbox, release the reset and wait for the firmware to say it runs.
//! A window's video memory place is its GPU address less the frame buffer's base plus its
//! offset (`DCN_VM_FB_LOCATION_BASE` and `DCN_VM_FB_OFFSET`). Register places are from AMD's
//! `dcn_3_1_2_offset.h` and `_sh_mask.h`, the sequence and layout from AMD's DMUB service
//! (`dmub_srv.c`, `dmub_dcn31.c`), all MIT (`AMD-NOTICE.md`).

use crate::io::RegisterIo;

const STACK_BYTES: u64 = 128 << 10;
const MAILBOX_HALF_BYTES: u64 = 8 << 10;
const TRACE_BYTES: u64 = 64 << 10;
const STATE_BYTES: u64 = 64 << 10;
const ALIGNMENT: u64 = 256;
const TRACE_ENTRY_OFFSET: u64 = 16;

const WINDOW_INSTRUCTIONS: u32 = 0x6000_0000;
const WINDOW_STACK: u32 = 0x6100_0000;
const WINDOW_BIOS: u32 = 0x6300_0000;
const WINDOW_MAILBOX: u32 = 0x6400_0000;
const WINDOW_TRACE: u32 = 0x6500_0000;
const WINDOW_STATE: u32 = 0x6600_0000;
const REGION_5: u32 = 0xa000_0000;

const CONTROL: u32 = 0x01f6;
const CONTROL_2: u32 = 0x0200;
const SECURITY_CONTROL: u32 = 0x01ce;
const WINDOW_BASE: u32 = 0x01a5;
const WINDOW_TOP: u32 = 0x01ad;
const WINDOW_OFFSET: u32 = 0x01b5;
const REGION_5_OFFSET: u32 = 0x0198;
const REGION_5_OFFSET_HIGH: u32 = 0x0199;
const REGION_5_TOP: u32 = 0x01a2;
const INBOX_BASE: u32 = 0x01d4;
const INBOX_SIZE: u32 = 0x01d5;
const INBOX_WRITE: u32 = 0x01d6;
const INBOX_READ: u32 = 0x01d7;
const OUTBOX_0_WRITE: u32 = 0x01da;
const OUTBOX_0_READ: u32 = 0x01db;
const OUTBOX_0_BASE: u32 = 0x01d8;
const OUTBOX_0_SIZE: u32 = 0x01d9;
const OUTBOX_1_BASE: u32 = 0x01dc;
const OUTBOX_1_SIZE: u32 = 0x01dd;
const OUTBOX_1_WRITE: u32 = 0x01de;
const OUTBOX_1_READ: u32 = 0x01df;
const SCRATCH_0: u32 = 0x01e3;
const SCRATCH_15: u32 = 0x01f2;
const GENERAL_INTERRUPT: u32 = 0x01f8;
const HUB_SOFT_RESET: u32 = 0x0342;
const FB_LOCATION_BASE: u32 = 0x0475;
const FB_OFFSET: u32 = 0x0477;

const ENABLE: u32 = 1 << 16;
const TRACE_PORT: u32 = 1 << 19;
const SOFT_RESET: u32 = 1;
const SECURITY_RESET: u32 = 1 << 16;
const MEMORY_UNIT_SHIFT: u32 = 8;
const MEMORY_UNIT_MASK: u32 = 0x3f << MEMORY_UNIT_SHIFT;
const MEMORY_UNIT: u32 = 0x20;
const WINDOW_ENABLE: u32 = 1 << 31;
const DMUIF_RESET: u32 = 1 << 8;
const FB_FIELD: u32 = 0x00ff_ffff;
const FIRMWARE_RUNNING: u32 = 1;
const MAILBOX_READY: u32 = 1 << 1;

/// Where each region goes in DMCUB's block of video memory, from its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub instructions: (u64, u64),
    pub stack: (u64, u64),
    pub data: (u64, u64),
    pub bios: (u64, u64),
    pub mailbox: (u64, u64),
    pub trace: (u64, u64),
    pub state: (u64, u64),
    /// The block's bytes.
    pub total: u64,
}

/// The regions for firmware of `instructions` and `data` bytes and a video BIOS of `bios`
/// bytes, each 256-byte aligned, one after another.
pub fn layout(instructions: u64, data: u64, bios: u64) -> Layout {
    let mut next = 0;
    let mut take = |bytes: u64| {
        let start = next;
        next = (start + bytes).next_multiple_of(ALIGNMENT);
        (start, bytes)
    };
    let instructions = take(instructions);
    let stack = take(STACK_BYTES);
    let data = take(data);
    let bios = take(bios);
    let mailbox = take(2 * MAILBOX_HALF_BYTES);
    let trace = take(TRACE_BYTES);
    let state = take(STATE_BYTES);
    Layout {
        instructions,
        stack,
        data,
        bios,
        mailbox,
        trace,
        state,
        total: next,
    }
}

/// Why DMCUB was not started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmcubError {
    /// The firmware did not say it runs in time.
    NotStarted,
}

impl DmcubError {
    pub fn description(self) -> &'static str {
        match self {
            Self::NotStarted => "the display microcontroller did not start",
        }
    }
}

/// One DCN engine's DMCUB, behind `regs` at the display block's segment 2 base.
#[derive(Debug)]
pub struct Dmcub<R: RegisterIo> {
    regs: R,
    segment_base: u32,
}

impl<R: RegisterIo> Dmcub<R> {
    pub fn new(regs: R, segment_base: u32) -> Self {
        Self { regs, segment_base }
    }

    fn offset(&self, place: u32) -> usize {
        (self.segment_base as usize + place as usize) * 4
    }

    fn read(&self, place: u32) -> u32 {
        self.regs.read32(self.offset(place))
    }

    fn write(&self, place: u32, value: u32) {
        self.regs.write32(self.offset(place), value);
    }

    fn update(&self, place: u32, clear: u32, set: u32) {
        self.write(place, self.read(place) & !clear | set);
    }

    fn window(&self, index: u32, base: u32, bytes: u64, offset: u64, inclusive: bool) {
        let top = base + bytes as u32 - u32::from(inclusive);
        self.write(WINDOW_OFFSET + 2 * index, offset as u32);
        self.write(WINDOW_OFFSET + 2 * index + 1, (offset >> 32) as u32);
        self.write(WINDOW_BASE + index, base);
        self.write(WINDOW_TOP + index, top | WINDOW_ENABLE);
    }

    /// Whether the firmware runs.
    pub fn running(&self) -> bool {
        self.read(CONTROL) & ENABLE != 0 && self.read(SCRATCH_0) & FIRMWARE_RUNNING != 0
    }

    /// Starts the firmware already copied into its block of video memory at GPU address
    /// `block`, laid out as `layout`; waits, while `keep_waiting` allows, for its mailbox.
    pub fn start(
        &self,
        block: u64,
        layout: &Layout,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), DmcubError> {
        let fb_base = u64::from(self.read(FB_LOCATION_BASE) & FB_FIELD) << 24;
        let fb_offset = u64::from(self.read(FB_OFFSET) & FB_FIELD) << 24;
        let place = |(start, _): (u64, u64)| (block + start).wrapping_sub(fb_base) + fb_offset;
        let raw = |(start, _): (u64, u64)| block + start;
        if self.read(CONTROL) & ENABLE != 0 {
            self.update(CONTROL_2, 0, SOFT_RESET);
            self.update(HUB_SOFT_RESET, 0, DMUIF_RESET);
            self.update(CONTROL, ENABLE, 0);
        }
        for pointer in [
            INBOX_READ,
            INBOX_WRITE,
            OUTBOX_1_READ,
            OUTBOX_1_WRITE,
            OUTBOX_0_READ,
            OUTBOX_0_WRITE,
            SCRATCH_0,
            GENERAL_INTERRUPT,
        ] {
            self.write(pointer, 0);
        }
        self.update(SECURITY_CONTROL, 0, SECURITY_RESET);
        let instructions = layout.instructions.1.next_multiple_of(ALIGNMENT);
        self.window(
            0,
            WINDOW_INSTRUCTIONS,
            instructions,
            place(layout.instructions),
            true,
        );
        self.window(1, WINDOW_STACK, layout.stack.1, place(layout.stack), true);
        self.update(
            SECURITY_CONTROL,
            SECURITY_RESET | MEMORY_UNIT_MASK,
            MEMORY_UNIT << MEMORY_UNIT_SHIFT,
        );
        self.window(3, WINDOW_BIOS, layout.bios.1, raw(layout.bios), false);
        self.window(
            4,
            WINDOW_MAILBOX,
            layout.mailbox.1,
            raw(layout.mailbox),
            false,
        );
        self.window(5, WINDOW_TRACE, layout.trace.1, raw(layout.trace), false);
        let trace = raw(layout.trace);
        self.write(REGION_5_OFFSET, trace as u32);
        self.write(REGION_5_OFFSET_HIGH, (trace >> 32) as u32);
        self.write(REGION_5_TOP, (layout.trace.1 as u32 - 1) | WINDOW_ENABLE);
        self.window(6, WINDOW_STATE, layout.state.1, raw(layout.state), false);
        self.write(OUTBOX_0_BASE, REGION_5 + TRACE_ENTRY_OFFSET as u32);
        self.write(OUTBOX_0_SIZE, (layout.trace.1 - TRACE_ENTRY_OFFSET) as u32);
        self.write(INBOX_BASE, WINDOW_MAILBOX);
        self.write(INBOX_SIZE, MAILBOX_HALF_BYTES as u32);
        self.write(OUTBOX_1_BASE, WINDOW_MAILBOX + MAILBOX_HALF_BYTES as u32);
        self.write(OUTBOX_1_SIZE, MAILBOX_HALF_BYTES as u32);
        self.update(HUB_SOFT_RESET, DMUIF_RESET, 0);
        self.write(SCRATCH_15, 0);
        self.update(CONTROL, 0, ENABLE | TRACE_PORT);
        self.update(CONTROL_2, SOFT_RESET, 0);
        while self.read(SCRATCH_0) & (FIRMWARE_RUNNING | MAILBOX_READY)
            != FIRMWARE_RUNNING | MAILBOX_READY
        {
            if !keep_waiting() {
                return Err(DmcubError::NotStarted);
            }
            core::hint::spin_loop();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use core::cell::RefCell;

    #[derive(Default)]
    struct Registers(RefCell<BTreeMap<usize, u32>>);

    impl RegisterIo for &Registers {
        fn read8(&self, _: usize) -> u8 {
            0
        }
        fn write8(&self, _: usize, _: u8) {}
        fn read16(&self, _: usize) -> u16 {
            0
        }
        fn write16(&self, _: usize, _: u16) {}
        fn read32(&self, offset: usize) -> u32 {
            if offset == (BASE + SCRATCH_0) as usize * 4 {
                return FIRMWARE_RUNNING | MAILBOX_READY;
            }
            self.0.borrow().get(&offset).copied().unwrap_or(0)
        }
        fn write32(&self, offset: usize, value: u32) {
            self.0.borrow_mut().insert(offset, value);
        }
    }

    const BASE: u32 = 0x34c0;

    #[test]
    fn regions_follow_each_other_256_byte_aligned() {
        let layout = layout(0x1_2345, 0x800, 0x2_0000);
        assert_eq!(layout.instructions, (0, 0x1_2345));
        assert_eq!(layout.stack.0, 0x1_2400);
        assert_eq!(layout.data.0, 0x3_2400);
        assert_eq!(layout.total % 256, 0);
    }

    #[test]
    fn the_instruction_window_points_at_video_memory_through_the_frame_buffer_offset() {
        let regs = Registers::default();
        regs.0
            .borrow_mut()
            .insert(((BASE + FB_LOCATION_BASE) * 4) as usize, 0x8000);
        regs.0
            .borrow_mut()
            .insert(((BASE + FB_OFFSET) * 4) as usize, 0x02);
        let dmcub = Dmcub::new(&regs, BASE);
        let layout = layout(0x1000, 0x100, 0x100);
        dmcub.start(0x80_0040_0000, &layout, &mut || true).unwrap();
        let at = |place: u32| {
            regs.0
                .borrow()
                .get(&(((BASE + place) * 4) as usize))
                .copied()
        };
        assert_eq!(at(WINDOW_OFFSET), Some(0x0240_0000));
        assert_eq!(at(WINDOW_BASE), Some(WINDOW_INSTRUCTIONS));
        assert_eq!(
            at(WINDOW_TOP),
            Some((WINDOW_INSTRUCTIONS + 0xfff) | WINDOW_ENABLE)
        );
        assert_eq!(at(INBOX_BASE), Some(WINDOW_MAILBOX));
        assert_eq!(at(CONTROL).unwrap() & ENABLE, ENABLE);
        assert_eq!(at(CONTROL_2).unwrap() & SOFT_RESET, 0);
    }
}
