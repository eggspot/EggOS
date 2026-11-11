//! The channel subsystem's I/O instructions and the virtio-ccw notification (D322), for the
//! drivers' [`ChannelSubsystem`].

use eggos_drivers::channel::{ChannelSubsystem, ConditionCode, Irb, Orb, Schib, SubchannelId};

const VIRTIO_CCW_NOTIFY: u64 = 3;

#[derive(Debug)]
pub(super) struct Instructions;

pub(super) static INSTRUCTIONS: Instructions = Instructions;

fn condition(program_mask: u64) -> ConditionCode {
    ConditionCode::new((program_mask >> 28) as u8)
}

impl ChannelSubsystem for Instructions {
    fn store_subchannel(&self, subchannel: SubchannelId, schib: &mut Schib) -> ConditionCode {
        let code: u64;
        // SAFETY: `stsch` stores 52 bytes into `schib`, which this call borrows mutably; it
        // touches nothing else.
        unsafe {
            core::arch::asm!(
                "stsch 0({schib})",
                "ipm {code}",
                in("r1") u64::from(subchannel.word()),
                schib = in(reg_addr) schib as *mut Schib,
                code = out(reg) code,
                options(nostack),
            )
        };
        condition(code)
    }

    fn modify_subchannel(&self, subchannel: SubchannelId, schib: &Schib) -> ConditionCode {
        let code: u64;
        // SAFETY: `msch` reads the 52 bytes of `schib`; it writes no memory.
        unsafe {
            core::arch::asm!(
                "msch 0({schib})",
                "ipm {code}",
                in("r1") u64::from(subchannel.word()),
                schib = in(reg_addr) schib as *const Schib,
                code = out(reg) code,
                options(nostack, readonly),
            )
        };
        condition(code)
    }

    unsafe fn start_subchannel(&self, subchannel: SubchannelId, orb: &Orb) -> ConditionCode {
        let code: u64;
        // SAFETY: `ssch` reads `orb`; the channel program it starts reaches only memory the
        // caller guarantees stays its own until the program's end.
        unsafe {
            core::arch::asm!(
                "ssch 0({orb})",
                "ipm {code}",
                in("r1") u64::from(subchannel.word()),
                orb = in(reg_addr) orb as *const Orb,
                code = out(reg) code,
                options(nostack),
            )
        };
        condition(code)
    }

    fn test_subchannel(&self, subchannel: SubchannelId, irb: &mut Irb) -> ConditionCode {
        let code: u64;
        // SAFETY: `tsch` stores at most 96 bytes into `irb`, which this call borrows mutably.
        unsafe {
            core::arch::asm!(
                "tsch 0({irb})",
                "ipm {code}",
                in("r1") u64::from(subchannel.word()),
                irb = in(reg_addr) irb as *mut Irb,
                code = out(reg) code,
                options(nostack),
            )
        };
        condition(code)
    }

    fn notify_virtio_queue(&self, subchannel: SubchannelId, queue: u16, cookie: u64) -> i64 {
        let result: i64;
        // SAFETY: the hypervisor call reads registers 1 to 4 and returns in register 2; it
        // reaches no guest memory but the queue's rings, which the driver gave the device.
        unsafe {
            core::arch::asm!(
                "diag 2, 4, 0x500",
                in("r1") VIRTIO_CCW_NOTIFY,
                inout("r2") u64::from(subchannel.word()) => result,
                in("r3") u64::from(queue),
                in("r4") cookie,
                options(nostack),
            )
        };
        result
    }
}
