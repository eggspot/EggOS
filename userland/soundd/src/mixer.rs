//! The mixer (D774): each stream programs play, at its own rate and channel count, resampled
//! by linear interpolation to the output's rate and spread or folded to stereo, scaled by its
//! own volume, summed, then scaled by the output's volume (its square, so the slider's middle
//! sounds half as loud) and clipped to 16 bits.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// The most samples a stream keeps waiting: two seconds of 48 kHz stereo.
pub const MOST_WAITING: usize = 2 * 48_000 * 2;
const UNITY: i64 = 1 << 16;
const FRACTION_BITS: u32 = 32;

/// The output gain for `volume` percent, in 16.16 fixed point: 0 when muted.
pub fn gain(volume: u8, mute: bool) -> i64 {
    if mute {
        return 0;
    }
    let volume = i64::from(volume.min(100));
    volume * volume * UNITY / 10_000
}

/// One stream being played.
#[derive(Debug)]
pub struct Stream {
    rate: u32,
    channels: u8,
    waiting: VecDeque<i16>,
    position: u64,
    volume: i64,
    ended: bool,
}

impl Stream {
    /// A stream of `channels` interleaved channels at `rate` frames a second.
    pub fn new(rate: u32, channels: u8) -> Self {
        Self {
            rate: rate.max(1),
            channels: channels.max(1),
            waiting: VecDeque::new(),
            position: 0,
            volume: UNITY,
            ended: false,
        }
    }

    /// Sets the stream's own volume, in percent.
    pub fn set_volume(&mut self, volume: u8) {
        self.volume = i64::from(volume.min(100)) * UNITY / 100;
    }

    /// How many more samples it takes before it is full.
    pub fn room(&self) -> usize {
        MOST_WAITING.saturating_sub(self.waiting.len())
    }

    /// Queues little-endian 16-bit samples from `bytes` (an odd last byte is dropped); how
    /// many bytes it took.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> usize {
        let samples = (bytes.len() / 2).min(self.room());
        for pair in bytes[..samples * 2].chunks_exact(2) {
            self.waiting.push_back(i16::from_le_bytes([pair[0], pair[1]]));
        }
        samples * 2
    }

    /// Queues samples.
    pub fn push(&mut self, samples: &[i16]) {
        let taken = samples.len().min(self.room());
        self.waiting.extend(&samples[..taken]);
    }

    /// Marks the stream as having no more samples coming: it ends once they are played.
    pub fn end(&mut self) {
        self.ended = true;
    }

    /// Whether it is over: ended and played out.
    pub fn is_over(&self) -> bool {
        self.ended && self.frames_waiting() < 2
    }

    fn frames_waiting(&self) -> usize {
        self.waiting.len() / usize::from(self.channels)
    }

    fn stereo_frame(&self, frame: usize) -> (i32, i32) {
        let channels = usize::from(self.channels);
        let at = frame * channels;
        let sample = |offset: usize| {
            i32::from(self.waiting.get(at + offset).copied().unwrap_or(0))
        };
        match channels {
            1 => (sample(0), sample(0)),
            _ => (sample(0), sample(1)),
        }
    }

    fn add_into(&mut self, out: &mut [i32], output_rate: u32) {
        let step = (u64::from(self.rate) << FRACTION_BITS) / u64::from(output_rate.max(1));
        for pair in out.chunks_exact_mut(2) {
            let frame = (self.position >> FRACTION_BITS) as usize;
            if frame + 1 >= self.frames_waiting() {
                break;
            }
            let fraction = (self.position & ((1 << FRACTION_BITS) - 1)) as i64;
            let (left0, right0) = self.stereo_frame(frame);
            let (left1, right1) = self.stereo_frame(frame + 1);
            let between = |a: i32, b: i32| {
                i64::from(a) + ((i64::from(b - a) * fraction) >> FRACTION_BITS)
            };
            let scaled = |value: i64| (value * self.volume >> 16) as i32;
            pair[0] += scaled(between(left0, left1));
            pair[1] += scaled(between(right0, right1));
            self.position += step;
        }
        let played = (self.position >> FRACTION_BITS) as usize;
        let drained = (played * usize::from(self.channels)).min(self.waiting.len());
        self.waiting.drain(..drained);
        self.position -= (played as u64) << FRACTION_BITS;
    }
}

/// Mixes `streams` into `out` (interleaved stereo at `output_rate`) at `gain`; frames no
/// stream reaches are silence. The caller drops the streams that are over afterwards.
pub fn mix<'a>(
    streams: impl IntoIterator<Item = &'a mut Stream>,
    out: &mut [i16],
    output_rate: u32,
    gain: i64,
) {
    let mut sums: Vec<i32> = alloc::vec![0; out.len()];
    for stream in streams {
        stream.add_into(&mut sums, output_rate);
    }
    for (sample, sum) in out.iter_mut().zip(sums) {
        let scaled = i64::from(sum) * gain >> 16;
        *sample = scaled.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
    }
}

/// `frames` frames of a full-scale-quarter sine at `hz` and `rate`, stereo, as a stream's
/// samples.
pub fn tone(hz: u32, rate: u32, frames: usize) -> Vec<i16> {
    let step = 2.0 * core::f64::consts::PI * f64::from(hz) / f64::from(rate.max(1));
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let value = (eggos_math::sin(step * frame as f64) * 8192.0) as i16;
        samples.push(value);
        samples.push(value);
    }
    samples
}

/// Samples as little-endian bytes, for the output device.
pub fn to_bytes(samples: &[i16], out: &mut Vec<u8>) {
    out.clear();
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_gain_is_the_volume_squared_and_mute_is_silence() {
        assert_eq!(gain(100, false), 1 << 16);
        assert_eq!(gain(50, false), (1 << 16) / 4);
        assert_eq!(gain(80, true), 0);
    }

    #[test]
    fn two_streams_sum_and_clip_and_a_mono_stream_is_heard_on_both_sides() {
        let mut first = Stream::new(48_000, 2);
        first.push(&[20_000, -100, 20_000, -100, 20_000, -100]);
        let mut second = Stream::new(48_000, 1);
        second.push(&[20_000, 20_000, 20_000]);
        let mut streams = alloc::vec![first, second];
        let mut out = [0i16; 4];
        mix(streams.iter_mut(), &mut out, 48_000, 1 << 16);
        assert_eq!(out, [i16::MAX, 19_900, i16::MAX, 19_900]);
    }

    #[test]
    fn a_stream_at_half_the_rate_is_interpolated_and_an_ended_stream_goes() {
        let mut slow = Stream::new(24_000, 1);
        slow.push(&[0, 1000, 2000]);
        slow.end();
        let mut streams = alloc::vec![slow];
        let mut out = [0i16; 6];
        mix(streams.iter_mut(), &mut out, 48_000, 1 << 16);
        streams.retain(|stream| !stream.is_over());
        assert_eq!(out, [0, 0, 500, 500, 1000, 1000]);
        let mut more = [0i16; 6];
        mix(streams.iter_mut(), &mut more, 48_000, 1 << 16);
        streams.retain(|stream| !stream.is_over());
        assert!(streams.is_empty());
        let mut bytes = Vec::new();
        to_bytes(&[1, -2], &mut bytes);
        assert_eq!(bytes, [1, 0, 0xfe, 0xff]);
        let samples = tone(440, 48_000, 4);
        assert_eq!(samples.len(), 8);
        assert_eq!(samples[0], 0);
    }
}
