//! The sound output (D773): the one output device a driver installed (virtio sound in QEMU,
//! Intel HD Audio on PCs), which the sound server (`soundd`) feeds through `/dev/sound` with
//! interleaved 16-bit stereo frames at the device's rate, and which `/proc/driver/sound`
//! describes: one line per device, its id, kind, rate and name (`hda0-out output 48000
//! Built-in speakers`), or `none`.
//!
//! OSS's `/dev/dsp` (D776), what SDL's `dsp` audio driver writes to, is one stream at a time
//! (its opener holds it, as OSS's devices are held): its format is set by the `SNDCTL_DSP_*`
//! `ioctl`s, its samples wait in a queue of [`DSP_QUEUE_BYTES`], and the sound server reads
//! them from `/dev/sound` as records ([`take_dsp`]) and mixes them with its other streams.

use crate::sync::SpinLock;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use core::fmt::Write;
use eggos_drivers::sound::PcmFormat;

/// A sound output a driver drives.
pub trait SoundOutput: Send {
    /// Its id, `<driver><index>-out` (`hda0-out`, `virtio0-out`).
    fn id(&self) -> &'static str;
    /// What the person calls it (`Built-in speakers`, `Headphones`, `virtio sound`).
    fn name(&self) -> &'static str;
    /// The format it plays.
    fn format(&self) -> PcmFormat;
    /// Queues the start of `samples`; how many bytes it took (0 while it is full).
    fn write(&mut self, samples: &[u8]) -> usize;
}

static OUTPUT: SpinLock<Option<Box<dyn SoundOutput>>> = SpinLock::new(None);

/// Installs `output` as the sound output; it comes back when one is installed already.
pub fn install(output: Box<dyn SoundOutput>) -> Result<(), Box<dyn SoundOutput>> {
    let mut installed = OUTPUT.lock();
    if installed.is_some() {
        return Err(output);
    }
    *installed = Some(output);
    Ok(())
}

/// Whether a sound output is installed.
pub fn is_present() -> bool {
    OUTPUT.lock().is_some()
}

/// Queues the start of `samples` to the output; how many bytes it took, 0 with no output.
pub fn write(samples: &[u8]) -> usize {
    OUTPUT
        .lock()
        .as_mut()
        .map_or(0, |output| output.write(samples))
}

/// `/proc/driver/sound`'s text, without its final newline.
pub fn status_text() -> String {
    let output = OUTPUT.lock();
    let Some(output) = output.as_ref() else {
        return String::from("none");
    };
    let mut text = String::new();
    let _ = write!(
        text,
        "{} output {} {}",
        output.id(),
        output.format().rate,
        output.name()
    );
    text
}

/// The most bytes `/dev/dsp` keeps waiting for the sound server.
pub const DSP_QUEUE_BYTES: usize = 64 * 1024;
/// The bytes of a record's header on `/dev/sound`: the stream's number, its rate, channels and
/// sample bits (8 unsigned or 16 signed), and whether it ended.
pub const DSP_RECORD_HEADER: usize = 16;
/// A record's flag: the stream's opener closed it.
pub const DSP_ENDED: u32 = 1;

/// `/dev/dsp`'s stream.
#[derive(Debug)]
pub struct DspStream {
    pub open: bool,
    pub number: u32,
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub fragment: u32,
    pub ended: bool,
    queue: VecDeque<u8>,
}

static DSP: SpinLock<DspStream> = SpinLock::new(DspStream {
    open: false,
    number: 0,
    rate: 8000,
    channels: 1,
    bits: 8,
    fragment: 4096,
    ended: false,
    queue: VecDeque::new(),
});

/// Takes `/dev/dsp` for a new opener, at OSS's defaults (8 kHz, mono, unsigned 8-bit); false
/// while another opener holds it or no sound output exists.
pub fn dsp_open() -> bool {
    if !is_present() {
        return false;
    }
    let mut dsp = DSP.lock();
    if dsp.open {
        return false;
    }
    dsp.open = true;
    dsp.number = dsp.number.wrapping_add(1);
    dsp.rate = 8000;
    dsp.channels = 1;
    dsp.bits = 8;
    dsp.fragment = 4096;
    dsp.ended = false;
    dsp.queue.clear();
    true
}

/// `/dev/dsp`'s opener closed it: the sound server plays what waits, then ends the stream.
pub fn dsp_close() {
    let mut dsp = DSP.lock();
    dsp.open = false;
    dsp.ended = true;
}

/// Works on `/dev/dsp`'s stream.
pub fn with_dsp<R>(work: impl FnOnce(&mut DspStream) -> R) -> R {
    work(&mut DSP.lock())
}

impl DspStream {
    /// Queues `bytes`; how many it took.
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        let room = DSP_QUEUE_BYTES.saturating_sub(self.queue.len());
        let taken = bytes.len().min(room);
        self.queue.extend(&bytes[..taken]);
        taken
    }

    /// The bytes waiting.
    pub fn waiting(&self) -> usize {
        self.queue.len()
    }

    /// Drops what waits.
    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

/// One record of `/dev/dsp`'s stream for the sound server into `out`: the header, then as many
/// waiting bytes as fit; 0 when there is nothing to say.
pub fn take_dsp(out: &mut [u8]) -> usize {
    let mut dsp = DSP.lock();
    if out.len() <= DSP_RECORD_HEADER || (dsp.queue.is_empty() && !dsp.ended) {
        return 0;
    }
    let length = dsp.queue.len().min(out.len() - DSP_RECORD_HEADER);
    let length = length - length % usize::from(dsp.channels * dsp.bits / 8).max(1);
    let ended = dsp.ended && length == dsp.queue.len();
    out[0..4].copy_from_slice(&dsp.number.to_le_bytes());
    out[4..8].copy_from_slice(&dsp.rate.to_le_bytes());
    out[8..10].copy_from_slice(&dsp.channels.to_le_bytes());
    out[10..12].copy_from_slice(&dsp.bits.to_le_bytes());
    let flags = if ended { DSP_ENDED } else { 0 };
    out[12..16].copy_from_slice(&flags.to_le_bytes());
    for (slot, byte) in out[DSP_RECORD_HEADER..DSP_RECORD_HEADER + length]
        .iter_mut()
        .zip(dsp.queue.drain(..length))
    {
        *slot = byte;
    }
    if ended {
        dsp.ended = false;
    }
    DSP_RECORD_HEADER + length
}
