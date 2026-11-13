//! The display coprocessor's iBoot display service (D546): the simple interface iBoot itself
//! lights the screen through, on EPIC endpoint `0x23`, class `disp0-service`. Each call is an
//! EPIC command of type `0xc0` whose input starts with the operation and its length; the service
//! powers the display, reports the hot-plug state and the timing and color modes the screen
//! takes, sets one, and shows a surface, directly or between a swap's begin and end.

use super::epic::{Endpoint, EpicError, Link};
use alloc::vec::Vec;

/// The iBoot display service's RTKit endpoint.
pub const ENDPOINT: u8 = 0x23;
const SERVICE_CLASS: &[u8] = b"disp0-service";
const COMMAND_TYPE: u16 = 0xc0;
const BUFFER_BYTES: usize = 0x4000;
const ANNOUNCEMENTS_READ: usize = 32;
const INPUT_HEADER_BYTES: usize = 16;
const OUTPUT_HEADER_BYTES: usize = 8;
const MODE_BYTES: usize = 24;
const LAYER_BYTES: usize = 164;
const PLANE_BYTES: usize = 44;
const SWAP_END_BYTES: usize = 12;

const SET_SURFACE: u32 = 1;
const SET_POWER: u32 = 2;
const GET_HOT_PLUG: u32 = 3;
const GET_TIMING_MODES: u32 = 4;
const GET_COLOR_MODES: u32 = 5;
const SET_MODE: u32 = 6;
const SWAP_BEGIN: u32 = 15;
const SWAP_SET_LAYER: u32 = 16;
const SWAP_END: u32 = 18;

/// Pixel formats a surface may have.
pub mod surface_format {
    pub const BGRA: u32 = 1;
    pub const RGBA: u32 = 3;
    pub const W30R: u32 = 9;
}

const PLANAR: u32 = 1;
const STANDARD_DYNAMIC_RANGE: u32 = 1;
const SURFACE_COLORSPACE: u32 = 2;

/// The layout of a swap's layer command, which changed with macOS 13.3's firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Firmware {
    /// Up to 13.2.
    Before13_3,
    /// 13.3 on, two more words after the layer.
    From13_3,
}

/// A timing mode the screen takes: its size, and its refresh rate in 16.16 fixed point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingMode {
    pub valid: u32,
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

/// A color mode the screen takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorMode {
    pub valid: u32,
    pub colorimetry: u32,
    pub transfer: u32,
    pub encoding: u32,
    pub bits_per_pixel: u32,
}

/// A surface to show: one plane at the display's device address `address`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surface {
    pub address: u64,
    pub stride: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
}

impl Surface {
    fn layer(&self) -> [u8; LAYER_BYTES] {
        let mut layer = [0u8; LAYER_BYTES];
        layer[4..12].copy_from_slice(&self.address.to_le_bytes());
        layer[16..20].copy_from_slice(&self.stride.to_le_bytes());
        layer[36..40].copy_from_slice(&PLANAR.to_le_bytes());
        let after_planes = 3 * PLANE_BYTES;
        let words = [
            0,
            1,
            self.width,
            self.height,
            self.format,
            SURFACE_COLORSPACE,
            STANDARD_DYNAMIC_RANGE,
        ];
        for (index, word) in words.into_iter().enumerate() {
            let at = after_planes + 4 * index;
            layer[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
        layer
    }
}

fn words(bytes: &[u8], at: usize, count: usize) -> Option<Vec<u32>> {
    (0..count)
        .map(|index| {
            let start = at + 4 * index;
            Some(u32::from_le_bytes(
                bytes.get(start..start + 4)?.try_into().ok()?,
            ))
        })
        .collect()
}

/// The service, started on its endpoint.
#[derive(Debug)]
pub struct DisplayService {
    endpoint: Endpoint,
    channel: u32,
    firmware: Firmware,
}

impl DisplayService {
    /// Starts endpoint [`ENDPOINT`] (RTKit started it already) and finds the service.
    pub fn start(link: &mut dyn Link, firmware: Firmware) -> Result<Self, EpicError> {
        let mut endpoint = Endpoint::new(ENDPOINT);
        endpoint.start(link)?;
        let channel = endpoint.find_service(
            link,
            SERVICE_CLASS,
            ANNOUNCEMENTS_READ,
            BUFFER_BYTES,
            BUFFER_BYTES,
        )?;
        Ok(Self {
            endpoint,
            channel,
            firmware,
        })
    }

    fn call(
        &mut self,
        link: &mut dyn Link,
        operation: u32,
        input: &[u8],
    ) -> Result<Vec<u8>, EpicError> {
        let mut message = Vec::with_capacity(INPUT_HEADER_BYTES + input.len());
        message.extend_from_slice(&operation.to_le_bytes());
        message.extend_from_slice(&((INPUT_HEADER_BYTES + input.len()) as u32).to_le_bytes());
        message.resize(INPUT_HEADER_BYTES, 0);
        message.extend_from_slice(input);
        let output = self
            .endpoint
            .command(link, self.channel, COMMAND_TYPE, &message)?;
        Ok(output.get(OUTPUT_HEADER_BYTES..).unwrap_or(&[]).to_vec())
    }

    /// Turns the display on or off.
    pub fn set_power(&mut self, link: &mut dyn Link, on: bool) -> Result<(), EpicError> {
        self.call(link, SET_POWER, &[u8::from(on)]).map(|_| ())
    }

    /// Whether a screen is attached, and how many timing and color modes it reports.
    pub fn hot_plug(&mut self, link: &mut dyn Link) -> Result<Option<(u32, u32)>, EpicError> {
        let output = self.call(link, GET_HOT_PLUG, &[])?;
        let attached = *output.first().ok_or(EpicError::Malformed)? != 0;
        let counts = words(&output, 4, 2).ok_or(EpicError::Malformed)?;
        Ok(attached.then_some((counts[0], counts[1])))
    }

    /// The timing modes the screen takes.
    pub fn timing_modes(&mut self, link: &mut dyn Link) -> Result<Vec<TimingMode>, EpicError> {
        let output = self.call(link, GET_TIMING_MODES, &[])?;
        let count = words(&output, 0, 1).ok_or(EpicError::Malformed)?[0] as usize;
        (0..count)
            .map(|index| {
                let at = 4 + index * MODE_BYTES;
                let fields = words(&output, at, 4).ok_or(EpicError::Malformed)?;
                Ok(TimingMode {
                    valid: fields[0],
                    width: fields[1],
                    height: fields[2],
                    refresh: fields[3],
                })
            })
            .collect()
    }

    /// The color modes the screen takes.
    pub fn color_modes(&mut self, link: &mut dyn Link) -> Result<Vec<ColorMode>, EpicError> {
        let output = self.call(link, GET_COLOR_MODES, &[])?;
        let count = words(&output, 0, 1).ok_or(EpicError::Malformed)?[0] as usize;
        (0..count)
            .map(|index| {
                let at = 4 + index * MODE_BYTES;
                let fields = words(&output, at, 5).ok_or(EpicError::Malformed)?;
                Ok(ColorMode {
                    valid: fields[0],
                    colorimetry: fields[1],
                    transfer: fields[2],
                    encoding: fields[3],
                    bits_per_pixel: fields[4],
                })
            })
            .collect()
    }

    /// Sets the screen to `timing` in `color`.
    pub fn set_mode(
        &mut self,
        link: &mut dyn Link,
        timing: TimingMode,
        color: ColorMode,
    ) -> Result<(), EpicError> {
        let mut input = [0u8; 2 * MODE_BYTES];
        let timing_words = [timing.valid, timing.width, timing.height, timing.refresh];
        let color_words = [
            color.valid,
            color.colorimetry,
            color.transfer,
            color.encoding,
            color.bits_per_pixel,
        ];
        for (index, word) in timing_words.into_iter().enumerate() {
            input[4 * index..4 * index + 4].copy_from_slice(&word.to_le_bytes());
        }
        for (index, word) in color_words.into_iter().enumerate() {
            let at = MODE_BYTES + 4 * index;
            input[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
        self.call(link, SET_MODE, &input).map(|_| ())
    }

    /// Shows `surface` at once.
    pub fn set_surface(&mut self, link: &mut dyn Link, surface: &Surface) -> Result<(), EpicError> {
        self.call(link, SET_SURFACE, &surface.layer()).map(|_| ())
    }

    /// Shows `surface` on layer 0 at the next frame, whole, through a swap.
    pub fn swap(&mut self, link: &mut dyn Link, surface: &Surface) -> Result<(), EpicError> {
        self.call(link, SWAP_BEGIN, &[])?;
        let rectangle = [surface.width, surface.height, 0, 0];
        let mut input = Vec::new();
        input.extend_from_slice(&0u32.to_le_bytes());
        input.extend_from_slice(&0u32.to_le_bytes());
        input.extend_from_slice(&surface.layer());
        if self.firmware == Firmware::From13_3 {
            input.extend_from_slice(&[0; 8]);
        }
        for _ in 0..2 {
            for word in rectangle {
                input.extend_from_slice(&word.to_le_bytes());
            }
        }
        input.extend_from_slice(&0u32.to_le_bytes());
        self.call(link, SWAP_SET_LAYER, &input)?;
        self.call(link, SWAP_END, &[0; SWAP_END_BYTES]).map(|_| ())
    }
}

/// The timing mode to light: valid first, then no larger than 1920 by 1200 at 60 Hz, then the
/// largest and fastest, as iBoot's own choice goes.
pub fn choose_timing(modes: &[TimingMode]) -> Option<TimingMode> {
    const SIXTY_HERTZ: u32 = 60 << 16;
    modes.iter().copied().max_by_key(|mode| {
        (
            mode.valid != 0,
            mode.width <= 1920,
            mode.height <= 1200,
            mode.refresh <= SIXTY_HERTZ,
            mode.width,
            mode.height,
            mode.refresh,
        )
    })
}

/// The color mode to light: valid first, then 32 bits a pixel or fewer, the most bits, and the
/// lowest colorimetry, encoding and transfer numbers (RGB, standard range).
pub fn choose_color(modes: &[ColorMode]) -> Option<ColorMode> {
    modes.iter().copied().max_by_key(|mode| {
        (
            mode.valid != 0,
            mode.bits_per_pixel <= 32,
            mode.bits_per_pixel,
            core::cmp::Reverse(mode.colorimetry),
            core::cmp::Reverse(mode.encoding),
            core::cmp::Reverse(mode.transfer),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_s_layer_holds_its_plane_and_size() {
        let surface = Surface {
            address: 0x1_2340_0000,
            stride: 7680,
            width: 1920,
            height: 1080,
            format: surface_format::BGRA,
        };
        let layer = surface.layer();
        assert_eq!(&layer[4..12], &0x1_2340_0000u64.to_le_bytes());
        assert_eq!(&layer[16..20], &7680u32.to_le_bytes());
        assert_eq!(&layer[36..40], &1u32.to_le_bytes());
        assert_eq!(&layer[136..140], &1u32.to_le_bytes());
        assert_eq!(&layer[140..144], &1920u32.to_le_bytes());
        assert_eq!(&layer[144..148], &1080u32.to_le_bytes());
        assert_eq!(&layer[148..152], &1u32.to_le_bytes());
        assert_eq!(layer.len(), 164);
    }

    #[test]
    fn modes_are_chosen_as_iboot_chooses_them() {
        let mode = |width, height, hertz: u32| TimingMode {
            valid: 1,
            width,
            height,
            refresh: hertz << 16,
        };
        let modes = [
            mode(3024, 1964, 120),
            mode(1920, 1200, 60),
            mode(1512, 982, 120),
        ];
        assert_eq!(choose_timing(&modes), Some(mode(1920, 1200, 60)));
        let color = |bits, colorimetry| ColorMode {
            valid: 1,
            colorimetry,
            transfer: 1,
            encoding: 1,
            bits_per_pixel: bits,
        };
        let colors = [color(30, 2), color(24, 1), color(30, 1), color(36, 1)];
        assert_eq!(choose_color(&colors), Some(color(30, 1)));
    }
}
