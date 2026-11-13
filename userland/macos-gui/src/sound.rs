//! WAV data as `NSSound` reads it (D383): a RIFF `WAVE` file's `fmt ` and `data` chunks, with
//! PCM, float or extensible samples, 1 to 8 channels and 8 to 32 bits.

const PCM: u16 = 1;
const FLOAT: u16 = 3;
const EXTENSIBLE: u16 = 0xfffe;
const MOST_CHANNELS: u16 = 8;
const FMT_LENGTH: usize = 16;
const EXTENSIBLE_SUB_FORMAT_AT: usize = 24;

/// How a WAV file's samples are stored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum SampleFormat {
    /// Signed integers (unsigned for 8 bits).
    #[default]
    Integer,
    /// IEEE floating point.
    Float,
}

/// A WAV file's format and where its samples are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Wave {
    /// How the samples are stored.
    pub format: SampleFormat,
    /// Channels per frame.
    pub channels: u16,
    /// Frames per second.
    pub sample_rate: u32,
    /// Bits per sample.
    pub bits: u16,
    /// Bytes per frame, every channel's sample together.
    pub frame_bytes: u16,
    /// The samples' offset in the file.
    pub data_offset: usize,
    /// The samples' length in bytes.
    pub data_length: usize,
}

impl Wave {
    /// Whole frames in the data.
    pub fn frames(&self) -> usize {
        match self.frame_bytes {
            0 => 0,
            bytes => self.data_length / usize::from(bytes),
        }
    }

    /// The sound's length in seconds.
    pub fn duration(&self) -> f64 {
        match self.sample_rate {
            0 => 0.0,
            rate => self.frames() as f64 / f64::from(rate),
        }
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn sample_format(tag: u16, chunk: &[u8]) -> Option<SampleFormat> {
    match tag {
        PCM => Some(SampleFormat::Integer),
        FLOAT => Some(SampleFormat::Float),
        EXTENSIBLE => match u16_at(chunk, EXTENSIBLE_SUB_FORMAT_AT)? {
            EXTENSIBLE => None,
            inner => sample_format(inner, &[]),
        },
        _ => None,
    }
}

fn read_format(chunk: &[u8]) -> Option<Wave> {
    if chunk.len() < FMT_LENGTH {
        return None;
    }
    let format = sample_format(u16_at(chunk, 0)?, chunk)?;
    let channels = u16_at(chunk, 2)?;
    let sample_rate = u32_at(chunk, 4)?;
    let frame_bytes = u16_at(chunk, 12)?;
    let bits = u16_at(chunk, 14)?;
    let usable_bits = match format {
        SampleFormat::Integer => matches!(bits, 8 | 16 | 24 | 32),
        SampleFormat::Float => bits == 32,
    };
    let fits = u32::from(channels) * u32::from(bits).div_ceil(8) <= u32::from(frame_bytes);
    if !usable_bits || channels == 0 || channels > MOST_CHANNELS || sample_rate == 0 || !fits {
        return None;
    }
    Some(Wave {
        format,
        channels,
        sample_rate,
        bits,
        frame_bytes,
        data_offset: 0,
        data_length: 0,
    })
}

/// Reads `bytes` as a WAV file: its format and where its samples are, or `None` when it is not
/// a WAV file this reads. A `data` chunk longer than the file is cut to what is there.
pub fn read_wave(bytes: &[u8]) -> Option<Wave> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut format = None;
    let mut at = 12;
    while let (Some(id), Some(length)) = (bytes.get(at..at + 4), u32_at(bytes, at + 4)) {
        let start = at + 8;
        let length = usize::try_from(length).ok()?;
        let end = start.saturating_add(length).min(bytes.len());
        match id {
            b"fmt " => format = Some(read_format(&bytes[start..end])?),
            b"data" => {
                let mut wave = format?;
                wave.data_offset = start;
                wave.data_length = end - start;
                return Some(wave);
            }
            _ => {}
        }
        at = start.checked_add(length)?.checked_add(length & 1)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn wave_file(tag: u16, channels: u16, rate: u32, bits: u16, samples: &[u8]) -> Vec<u8> {
        let frame = channels * bits.div_ceil(8);
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * u32::from(frame)).to_le_bytes());
        fmt.extend_from_slice(&frame.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        if tag == EXTENSIBLE {
            fmt.extend_from_slice(&22u16.to_le_bytes());
            fmt.extend_from_slice(&bits.to_le_bytes());
            fmt.extend_from_slice(&3u32.to_le_bytes());
            fmt.extend_from_slice(&PCM.to_le_bytes());
            fmt.extend_from_slice(&[0; 14]);
        }
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(b"WAVE");
        file.extend_from_slice(b"LIST");
        file.extend_from_slice(&3u32.to_le_bytes());
        file.extend_from_slice(&[1, 2, 3, 0]);
        file.extend_from_slice(b"fmt ");
        file.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        file.extend_from_slice(&fmt);
        file.extend_from_slice(b"data");
        file.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        file.extend_from_slice(samples);
        file
    }

    #[test]
    fn a_pcm_wave_gives_its_format_samples_and_duration() {
        let samples = [0u8; 8000];
        let file = wave_file(PCM, 2, 1000, 16, &samples);
        let wave = read_wave(&file).unwrap();
        assert_eq!(wave.format, SampleFormat::Integer);
        assert_eq!((wave.channels, wave.sample_rate, wave.bits), (2, 1000, 16));
        assert_eq!(wave.data_length, 8000);
        assert_eq!(&file[wave.data_offset..wave.data_offset + 4], &[0; 4]);
        assert_eq!(wave.frames(), 2000);
        assert_eq!(wave.duration(), 2.0);
    }

    #[test]
    fn float_and_extensible_waves_are_read() {
        let float = read_wave(&wave_file(FLOAT, 1, 48000, 32, &[0; 4])).unwrap();
        assert_eq!(float.format, SampleFormat::Float);
        let extensible = read_wave(&wave_file(EXTENSIBLE, 6, 44100, 24, &[0; 18])).unwrap();
        assert_eq!(extensible.format, SampleFormat::Integer);
        assert_eq!((extensible.channels, extensible.frames()), (6, 1));
    }

    #[test]
    fn data_past_the_end_of_the_file_is_cut_to_what_is_there() {
        let mut file = wave_file(PCM, 1, 8000, 8, &[0x80; 10]);
        let length_at = file.len() - 14;
        file[length_at..length_at + 4].copy_from_slice(&100u32.to_le_bytes());
        assert_eq!(read_wave(&file).unwrap().data_length, 10);
    }

    #[test]
    fn files_that_are_not_waves_this_reads_give_none() {
        assert_eq!(read_wave(b"RIFF\0\0\0\0AVI "), None);
        assert_eq!(read_wave(&wave_file(2, 1, 8000, 4, &[0; 4])), None);
        assert_eq!(read_wave(&wave_file(PCM, 9, 8000, 16, &[0; 18])), None);
        assert_eq!(read_wave(&wave_file(PCM, 1, 8000, 12, &[0; 4])), None);
        assert_eq!(read_wave(&wave_file(PCM, 1, 0, 16, &[0; 4])), None);
        let mut no_data = wave_file(PCM, 1, 8000, 16, &[0; 4]);
        no_data.truncate(no_data.len() - 12);
        assert_eq!(read_wave(&no_data), None);
    }
}
