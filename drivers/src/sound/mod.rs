//! Sound devices (D773): what every sound driver shares (the PCM format the mixer hands them,
//! interleaved signed 16-bit little-endian frames) and Intel High Definition Audio
//! ([`hda`]); the virtio sound device is [`crate::virtio::sound`].

pub mod hda;

/// The bytes of one stereo 16-bit frame.
pub const FRAME_BYTES: usize = 4;

/// A PCM output's format: its rate in frames a second and its channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub rate: u32,
    pub channels: u8,
}

/// Why a sound device could not be driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The controller did not leave or enter reset in time.
    ResetTimedOut,
    /// No codec answered on the link.
    NoCodec,
    /// A codec did not answer a command in time.
    TimedOut,
    /// The codec has no output path from a converter to a speaker, headphone or line-out pin.
    NoOutputPath,
    /// The controller has no output stream.
    NoOutputStream,
    /// The DMA memory is too small for the rings and the buffer.
    MemoryTooSmall,
}

impl Error {
    /// A short description for the kernel's log.
    pub const fn description(self) -> &'static str {
        match self {
            Error::ResetTimedOut => "the audio controller did not come out of reset",
            Error::NoCodec => "no codec answered on the audio link",
            Error::TimedOut => "a codec did not answer in time",
            Error::NoOutputPath => "the codec has no path to a speaker or headphone pin",
            Error::NoOutputStream => "the audio controller has no output stream",
            Error::MemoryTooSmall => "the DMA memory is too small",
        }
    }
}
