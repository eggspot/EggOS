//! The virtio sound device (virtio 1.2, 5.14), polled (D773): its PCM streams read through the
//! control queue, the first output stream that takes 16-bit stereo at 48 or 44.1 kHz set up
//! and prepared, and the transmit queue fed one period at a time, each a buffer of the
//! stream's id, the samples and the status the device writes back. The stream starts when its
//! first periods are queued and stops when the driver is asked to.

use super::queue::{Buffer, SplitQueue};
use super::{Error, ResetDone, Transport, device_type};
use crate::dma::DmaMemory;
use crate::sound::{FRAME_BYTES, PcmFormat};

/// Entries of the control queue the driver sets up, at most.
pub const CONTROL_QUEUE_SIZE: u16 = 8;
/// Entries of the transmit queue the driver sets up, at most.
pub const TX_QUEUE_SIZE: u16 = 16;
/// The periods queued to the device at once.
pub const PERIODS: usize = 4;
/// The bytes of one period: 1024 stereo 16-bit frames.
pub const PERIOD_BYTES: usize = 1024 * FRAME_BYTES;
/// The DMA memory the driver needs.
pub const MEMORY_BYTES: usize = SLOTS_OFFSET + PERIODS * SLOT_STRIDE;

const CONTROL_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 2;
const TX_QUEUE_OFFSET: usize = 4096;
const REQUEST_OFFSET: usize = 8192;
const RESPONSE_OFFSET: usize = 8448;
const RESPONSE_BYTES: usize = 3840;
const SLOTS_OFFSET: usize = 12288;
const SLOT_STRIDE: usize = 8192;
const SLOT_HEADER: usize = 16;
const SLOT_STATUS: usize = SLOT_HEADER + PERIOD_BYTES;
const MOST_STREAMS: u32 = 16;

/// The device's configuration fields' offsets.
pub mod config {
    pub const JACKS: usize = 0;
    pub const STREAMS: usize = 4;
    pub const CHANNEL_MAPS: usize = 8;
}

/// Request codes (virtio 1.2, 5.14.6).
pub mod request {
    pub const PCM_INFO: u32 = 0x0100;
    pub const PCM_SET_PARAMS: u32 = 0x0101;
    pub const PCM_PREPARE: u32 = 0x0102;
    pub const PCM_RELEASE: u32 = 0x0103;
    pub const PCM_START: u32 = 0x0104;
    pub const PCM_STOP: u32 = 0x0105;
}

/// Status codes.
pub mod status {
    pub const OK: u32 = 0x8000;
}

/// A PCM stream's direction.
pub mod direction {
    pub const OUTPUT: u8 = 0;
    pub const INPUT: u8 = 1;
}

/// `VIRTIO_SND_PCM_FMT_S16`'s bit.
pub const FORMAT_S16: u8 = 5;
/// `VIRTIO_SND_PCM_RATE_44100`'s bit.
pub const RATE_44100: u8 = 6;
/// `VIRTIO_SND_PCM_RATE_48000`'s bit.
pub const RATE_48000: u8 = 7;
/// The bytes of one `virtio_snd_pcm_info`.
pub const PCM_INFO_BYTES: usize = 32;

/// One PCM stream as the device describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub formats: u64,
    pub rates: u64,
    pub direction: u8,
    pub channels_min: u8,
    pub channels_max: u8,
}

/// Reads a `virtio_snd_pcm_info` record.
pub fn parse_stream_info(record: &[u8]) -> Option<StreamInfo> {
    let record = record.get(..PCM_INFO_BYTES)?;
    let u64_at = |at: usize| {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&record[at..at + 8]);
        u64::from_le_bytes(bytes)
    };
    Some(StreamInfo {
        formats: u64_at(8),
        rates: u64_at(16),
        direction: record[24],
        channels_min: record[25],
        channels_max: record[26],
    })
}

/// The format the driver plays `info` at: 16-bit stereo, 48 kHz if it takes it, else 44.1 kHz;
/// `None` for an input stream or one that takes neither.
pub fn chosen_format(info: &StreamInfo) -> Option<(PcmFormat, u8)> {
    let stereo = info.channels_min <= 2 && info.channels_max >= 2;
    if info.direction != direction::OUTPUT || !stereo || info.formats & 1 << FORMAT_S16 == 0 {
        return None;
    }
    [(48_000, RATE_48000), (44_100, RATE_44100)]
        .into_iter()
        .find(|&(_, bit)| info.rates & 1 << bit != 0)
        .map(|(rate, bit)| (PcmFormat { rate, channels: 2 }, bit))
}

/// The bytes of a `virtio_snd_pcm_set_params` request for `stream`.
pub fn set_params_request(stream: u32, rate_bit: u8) -> [u8; 24] {
    let mut bytes = [0u8; 24];
    bytes[0..4].copy_from_slice(&request::PCM_SET_PARAMS.to_le_bytes());
    bytes[4..8].copy_from_slice(&stream.to_le_bytes());
    bytes[8..12].copy_from_slice(&((PERIODS * PERIOD_BYTES) as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&(PERIOD_BYTES as u32).to_le_bytes());
    bytes[20] = 2;
    bytes[21] = FORMAT_S16;
    bytes[22] = rate_bit;
    bytes
}

/// The bytes of a request about one stream (`PCM_PREPARE`, `PCM_START`, ...).
pub fn stream_request(code: u32, stream: u32) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    bytes[0..4].copy_from_slice(&code.to_le_bytes());
    bytes[4..8].copy_from_slice(&stream.to_le_bytes());
    bytes
}

/// The bytes of a `PCM_INFO` query for `count` streams from 0.
pub fn info_request(count: u32) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&request::PCM_INFO.to_le_bytes());
    bytes[8..12].copy_from_slice(&count.to_le_bytes());
    bytes[12..16].copy_from_slice(&(PCM_INFO_BYTES as u32).to_le_bytes());
    bytes
}

/// A virtio sound device with one output stream set up, polled.
#[derive(Debug)]
pub struct VirtioSound<T: Transport, M: DmaMemory> {
    transport: T,
    memory: M,
    control: SplitQueue,
    tx: SplitQueue,
    stream: u32,
    format: PcmFormat,
    busy: [Option<u16>; PERIODS],
    started: bool,
}

impl<T: Transport, M: DmaMemory> VirtioSound<T, M> {
    /// Sets the sound device behind `transport` up with `memory` ([`MEMORY_BYTES`] or more):
    /// its queues, then its first output stream that plays 16-bit stereo, prepared. On an error
    /// the transport comes back, and the memory too unless the device may still use it.
    pub fn new(
        transport: T,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, T, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, transport, Some(memory)));
        }
        if transport.device_type() != device_type::SOUND {
            return Err((Error::WrongDeviceType, transport, Some(memory)));
        }
        if let Err(error) = super::initialize(&transport, 0, keep_waiting) {
            return Err((error, transport, Some(memory)));
        }
        let queues = Self::queues(&transport, &memory);
        let (control, tx) = match queues {
            Ok(queues) => queues,
            Err(error) => {
                super::fail(&transport);
                let memory = match super::reset(&transport, keep_waiting) {
                    Ok(_) => Some(memory),
                    Err(_) => {
                        core::mem::forget(memory);
                        None
                    }
                };
                return Err((error, transport, memory));
            }
        };
        super::driver_ok(&transport);
        let mut sound = Self {
            transport,
            memory,
            control,
            tx,
            stream: 0,
            format: PcmFormat {
                rate: 48_000,
                channels: 2,
            },
            busy: [None; PERIODS],
            started: false,
        };
        match sound.set_up_output(keep_waiting) {
            Ok(()) => Ok(sound),
            Err(error) => {
                let Self {
                    transport, memory, ..
                } = sound;
                let memory = match super::reset(&transport, keep_waiting) {
                    Ok(_) => Some(memory),
                    Err(_) => {
                        core::mem::forget(memory);
                        None
                    }
                };
                Err((error, transport, memory))
            }
        }
    }

    fn queues(transport: &T, memory: &M) -> Result<(SplitQueue, SplitQueue), Error> {
        let mut made = [None, None];
        for (slot, (queue, offset, most)) in made.iter_mut().zip([
            (CONTROL_QUEUE, 0, CONTROL_QUEUE_SIZE),
            (TX_QUEUE, TX_QUEUE_OFFSET, TX_QUEUE_SIZE),
        ]) {
            let max = transport.max_queue_size(queue).min(most);
            if max == 0 {
                return Err(Error::QueueUnavailable);
            }
            let size = 1 << max.ilog2();
            let dma = memory
                .dma_addr()
                .checked_add(offset as u64)
                .ok_or(Error::MemoryTooSmall)?;
            // SAFETY: the queue's memory is in the driver's DMA memory, which outlives the queue.
            let mut made_queue = unsafe {
                SplitQueue::new(memory.as_mut_ptr().wrapping_add(offset), dma, size)?
            };
            made_queue.disable_used_notifications();
            transport.set_up_queue(queue, size, &made_queue.addresses())?;
            *slot = Some(made_queue);
        }
        let [Some(control), Some(tx)] = made else {
            return Err(Error::QueueUnavailable);
        };
        Ok((control, tx))
    }

    fn write_memory(&self, offset: usize, bytes: &[u8]) {
        let base = self.memory.as_mut_ptr().wrapping_add(offset);
        for (index, &byte) in bytes.iter().enumerate() {
            // SAFETY: inside the driver's DMA memory, in an area the device does not hold.
            unsafe { base.wrapping_add(index).write_volatile(byte) };
        }
    }

    fn read_memory(&self, offset: usize, bytes: &mut [u8]) {
        let base = self.memory.as_mut_ptr().wrapping_add(offset);
        for (index, byte) in bytes.iter_mut().enumerate() {
            // SAFETY: inside the driver's DMA memory, in an area the device handed back.
            *byte = unsafe { base.wrapping_add(index).read_volatile() };
        }
    }

    fn dma(&self, offset: usize) -> Result<u64, Error> {
        self.memory
            .dma_addr()
            .checked_add(offset as u64)
            .ok_or(Error::MemoryTooSmall)
    }

    fn command(
        &mut self,
        request: &[u8],
        response_bytes: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.write_memory(REQUEST_OFFSET, request);
        let response_bytes = response_bytes.clamp(4, RESPONSE_BYTES);
        self.write_memory(RESPONSE_OFFSET, &[0; 4]);
        self.control.submit(&[
            Buffer {
                addr: self.dma(REQUEST_OFFSET)?,
                len: request.len() as u32,
                device_writes: false,
            },
            Buffer {
                addr: self.dma(RESPONSE_OFFSET)?,
                len: response_bytes as u32,
                device_writes: true,
            },
        ])?;
        if self.control.should_notify() {
            self.transport.notify(CONTROL_QUEUE);
        }
        loop {
            if self.control.pop_used().is_some() {
                break;
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
        }
        let mut code = [0u8; 4];
        self.read_memory(RESPONSE_OFFSET, &mut code);
        match u32::from_le_bytes(code) {
            status::OK => Ok(()),
            other => Err(Error::CommandFailed(other)),
        }
    }

    fn set_up_output(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        let streams = self
            .transport
            .read_config32(config::STREAMS)
            .min(MOST_STREAMS);
        if streams == 0 {
            return Err(Error::NoOutputStream);
        }
        let bytes = 4 + streams as usize * PCM_INFO_BYTES;
        self.command(&info_request(streams), bytes, keep_waiting)?;
        let mut answer = [0u8; 4 + MOST_STREAMS as usize * PCM_INFO_BYTES];
        self.read_memory(RESPONSE_OFFSET, &mut answer[..bytes]);
        let chosen = (0..streams).find_map(|stream| {
            let at = 4 + stream as usize * PCM_INFO_BYTES;
            let info = parse_stream_info(&answer[at..])?;
            chosen_format(&info).map(|(format, bit)| (stream, format, bit))
        });
        let (stream, format, rate_bit) = chosen.ok_or(Error::NoOutputStream)?;
        self.command(&set_params_request(stream, rate_bit), 4, keep_waiting)?;
        self.command(
            &stream_request(request::PCM_PREPARE, stream),
            4,
            keep_waiting,
        )?;
        self.stream = stream;
        self.format = format;
        Ok(())
    }

    /// The transport the device is reached through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The format the output plays.
    pub fn format(&self) -> PcmFormat {
        self.format
    }

    /// Frees the periods the device has played.
    pub fn reclaim(&mut self) {
        while let Some(used) = self.tx.pop_used() {
            for slot in &mut self.busy {
                if *slot == Some(used.head) {
                    *slot = None;
                }
            }
        }
    }

    /// How many periods wait in the device, played or not yet reclaimed.
    pub fn queued(&self) -> usize {
        self.busy.iter().filter(|slot| slot.is_some()).count()
    }

    /// Queues the start of `samples` (interleaved 16-bit little-endian stereo frames) as one
    /// period, at most [`PERIOD_BYTES`], into a free slot, and starts the stream once periods
    /// are queued; how many bytes it took (0 while every slot waits in the device).
    pub fn write(
        &mut self,
        samples: &[u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<usize, Error> {
        self.reclaim();
        let Some(index) = self.busy.iter().position(Option::is_none) else {
            return Ok(0);
        };
        let length = samples.len().min(PERIOD_BYTES) / FRAME_BYTES * FRAME_BYTES;
        if length == 0 {
            return Ok(0);
        }
        let slot = SLOTS_OFFSET + index * SLOT_STRIDE;
        self.write_memory(slot, &self.stream.to_le_bytes());
        self.write_memory(slot + SLOT_HEADER, &samples[..length]);
        let head = self.tx.submit(&[
            Buffer {
                addr: self.dma(slot)?,
                len: 4,
                device_writes: false,
            },
            Buffer {
                addr: self.dma(slot + SLOT_HEADER)?,
                len: length as u32,
                device_writes: false,
            },
            Buffer {
                addr: self.dma(slot + SLOT_STATUS)?,
                len: 8,
                device_writes: true,
            },
        ])?;
        self.busy[index] = Some(head);
        if self.tx.should_notify() {
            self.transport.notify(TX_QUEUE);
        }
        if !self.started && self.queued() >= 2 {
            self.command(
                &stream_request(request::PCM_START, self.stream),
                4,
                keep_waiting,
            )?;
            self.started = true;
        }
        Ok(length)
    }

    /// Stops the stream (what is queued still plays out) and prepares it to start again.
    pub fn stop(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        if !self.started {
            return Ok(());
        }
        self.command(
            &stream_request(request::PCM_STOP, self.stream),
            4,
            keep_waiting,
        )?;
        self.started = false;
        Ok(())
    }

    /// Releases the stream, resets the device and hands back the transport and the memory.
    pub fn reset(
        mut self,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(T, M, ResetDone), Error> {
        let _ = self.stop(keep_waiting);
        let _ = self.command(
            &stream_request(request::PCM_RELEASE, self.stream),
            4,
            keep_waiting,
        );
        let Self {
            transport, memory, ..
        } = self;
        match super::reset(&transport, keep_waiting) {
            Ok(done) => Ok((transport, memory, done)),
            Err(error) => {
                core::mem::forget(memory);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(direction: u8, formats: u64, rates: u64, channels: (u8, u8)) -> [u8; 32] {
        let mut record = [0u8; 32];
        record[8..16].copy_from_slice(&formats.to_le_bytes());
        record[16..24].copy_from_slice(&rates.to_le_bytes());
        record[24] = direction;
        record[25] = channels.0;
        record[26] = channels.1;
        record
    }

    #[test]
    fn the_first_stereo_sixteen_bit_output_at_forty_eight_kilohertz_is_chosen() {
        let both = 1 << RATE_48000 | 1 << RATE_44100;
        let record = info(direction::OUTPUT, 1 << FORMAT_S16, both, (1, 2));
        let parsed = parse_stream_info(&record).unwrap();
        let (format, bit) = chosen_format(&parsed).unwrap();
        assert_eq!((format.rate, format.channels, bit), (48_000, 2, RATE_48000));
        let only_cd = info(direction::OUTPUT, 1 << FORMAT_S16, 1 << RATE_44100, (2, 2));
        let (format, _) = chosen_format(&parse_stream_info(&only_cd).unwrap()).unwrap();
        assert_eq!(format.rate, 44_100);
        let input = info(direction::INPUT, 1 << FORMAT_S16, both, (2, 2));
        assert_eq!(chosen_format(&parse_stream_info(&input).unwrap()), None);
        let mono = info(direction::OUTPUT, 1 << FORMAT_S16, both, (1, 1));
        assert_eq!(chosen_format(&parse_stream_info(&mono).unwrap()), None);
    }

    #[test]
    fn requests_are_laid_out_as_the_specification_says() {
        let params = set_params_request(3, RATE_48000);
        assert_eq!(&params[0..4], &0x0101u32.to_le_bytes());
        assert_eq!(&params[4..8], &3u32.to_le_bytes());
        assert_eq!(&params[8..12], &((PERIODS * PERIOD_BYTES) as u32).to_le_bytes());
        assert_eq!(&params[12..16], &(PERIOD_BYTES as u32).to_le_bytes());
        assert_eq!(&params[20..23], &[2, FORMAT_S16, RATE_48000]);
        assert_eq!(&stream_request(request::PCM_START, 1)[0..4], &0x0104u32.to_le_bytes());
        let query = info_request(2);
        assert_eq!(&query[8..16], &[2, 0, 0, 0, 32, 0, 0, 0]);
        assert!(MEMORY_BYTES >= SLOTS_OFFSET + (PERIODS - 1) * SLOT_STRIDE + SLOT_STATUS + 8);
    }
}
