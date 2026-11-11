//! The s390x channel subsystem as a bus (D322): every I/O subchannel of subchannel set 0 with a
//! device, enabled and asked for its control unit with `SENSE ID`, becomes an
//! [`Identity::Channel`] node; virtio-ccw's bus driver takes those whose control unit is
//! virtio's. Subchannels are numbered from 0 without gaps (QEMU's), so the walk ends at the first
//! subchannel that is not there.

use super::dma::DmaRegion;
use super::{DeviceNode, DeviceTable, DmaCoherence, Identity};
use eggos_drivers::channel::{
    self, ChannelBuffer, ChannelError, ChannelSubsystem, ConditionCode, Schib, SubchannelId, subchannel_type,
};

pub(super) fn discover(channels: &'static dyn ChannelSubsystem, table: &mut DeviceTable) -> Result<(), &'static str> {
    let memory = DmaRegion::allocate(channel::BUFFER_BYTES).map_err(|_| "no memory for SENSE ID")?;
    let mut buffer = match ChannelBuffer::new(memory) {
        Ok(buffer) => buffer,
        Err((error, memory)) => {
            // SAFETY: the region was never given to a device.
            unsafe { memory.free() };
            return Err(error.description());
        }
    };
    let mut abandoned = false;
    for number in 0..=u16::MAX {
        let subchannel = SubchannelId::new(0, number);
        let mut schib = Schib::default();
        if channels.store_subchannel(subchannel, &mut schib) == ConditionCode::NOT_OPERATIONAL {
            break;
        }
        if !schib.device_number_valid() || schib.subchannel_type() != subchannel_type::IO {
            continue;
        }
        let sense = channel::enable(channels, subchannel)
            .and_then(|_| channel::sense_id(channels, subchannel, &mut buffer));
        abandoned |= sense == Err(ChannelError::TimedOut);
        let sense = sense.ok();
        let mut node = DeviceNode::new(Identity::Channel {
            subchannel: subchannel.word(),
            device_number: schib.device_number(),
            control_unit: sense.map_or(0, |sense| sense.control_unit_type),
            model: sense.map_or(0, |sense| sense.control_unit_model),
        });
        node.dma = DmaCoherence::Coherent;
        table.add(node);
    }
    if abandoned {
        core::mem::forget(buffer);
        return Ok(());
    }
    // SAFETY: every channel program run with the buffer ended (`run` waited for its final status
    // or the subchannel was no longer operational; a timed-out one keeps the buffer above), so
    // no device reaches it.
    unsafe { buffer.into_memory().free() };
    Ok(())
}

/// What a channel device is to a person, by its control unit.
pub fn class(control_unit: u16) -> &'static str {
    match control_unit {
        eggos_drivers::virtio::ccw::CONTROL_UNIT_TYPE => "bus",
        0x3270 | 0x3274 => "console",
        _ => "other",
    }
}
