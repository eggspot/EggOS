//! Intel High Definition Audio (D773, from the High Definition Audio Specification 1.0a),
//! polled: the controller out of reset, its command (CORB) and response (RIRB) rings, the
//! first codec found on the link, an output path from one of its converters (DACs) through
//! mixers and selectors to a speaker, headphone or line-out pin (headphones first when one is
//! plugged), every widget on it powered, unmuted and selected, and the first output stream
//! descriptor playing a cyclic buffer of [`PERIODS`] periods described by a buffer descriptor
//! list, at 48 kHz 16-bit stereo.

use super::{Error, FRAME_BYTES, PcmFormat};
use crate::dma::DmaMemory;
use crate::io::RegisterIo;

/// The periods of the cyclic buffer.
pub const PERIODS: usize = 4;
/// The bytes of one period: 1024 stereo 16-bit frames.
pub const PERIOD_BYTES: usize = 1024 * FRAME_BYTES;
/// The DMA memory the driver needs.
pub const MEMORY_BYTES: usize = BUFFER_OFFSET + PERIODS * PERIOD_BYTES;
/// The format the driver plays.
pub const FORMAT: PcmFormat = PcmFormat {
    rate: 48_000,
    channels: 2,
};

const CORB_OFFSET: usize = 0;
const RIRB_OFFSET: usize = 1024;
const BDL_OFFSET: usize = 4096;
const BUFFER_OFFSET: usize = 8192;
const RING_ENTRIES: u16 = 256;
const STREAM_TAG: u32 = 1;
const MOST_NODES: u8 = 96;
const MOST_DEPTH: usize = 5;
const MOST_CONNECTIONS: usize = 16;

/// The controller's registers.
pub mod register {
    pub const GCAP: usize = 0x00;
    pub const GCTL: usize = 0x08;
    pub const STATESTS: usize = 0x0e;
    pub const CORBLBASE: usize = 0x40;
    pub const CORBUBASE: usize = 0x44;
    pub const CORBWP: usize = 0x48;
    pub const CORBRP: usize = 0x4a;
    pub const CORBCTL: usize = 0x4c;
    pub const CORBSIZE: usize = 0x4e;
    pub const RIRBLBASE: usize = 0x50;
    pub const RIRBUBASE: usize = 0x54;
    pub const RIRBWP: usize = 0x58;
    pub const RINTCNT: usize = 0x5a;
    pub const RIRBCTL: usize = 0x5c;
    pub const RIRBSIZE: usize = 0x5e;
    pub const STREAMS: usize = 0x80;
    pub const STREAM_STRIDE: usize = 0x20;
    pub const SD_CTL: usize = 0x00;
    pub const SD_STS: usize = 0x03;
    pub const SD_LPIB: usize = 0x04;
    pub const SD_CBL: usize = 0x08;
    pub const SD_LVI: usize = 0x0c;
    pub const SD_FMT: usize = 0x12;
    pub const SD_BDPL: usize = 0x18;
    pub const SD_BDPU: usize = 0x1c;
}

/// Codec verbs and parameters.
pub mod verb {
    pub const GET_PARAMETER: u32 = 0xf00;
    pub const GET_CONNECTION_ENTRY: u32 = 0xf02;
    pub const GET_PIN_SENSE: u32 = 0xf09;
    pub const GET_CONFIG_DEFAULT: u32 = 0xf1c;
    pub const SET_CONNECTION_SELECT: u32 = 0x701;
    pub const SET_POWER_STATE: u32 = 0x705;
    pub const SET_STREAM_CHANNEL: u32 = 0x706;
    pub const SET_PIN_CONTROL: u32 = 0x707;
    pub const SET_EAPD: u32 = 0x70c;
    pub const SET_FORMAT: u32 = 0x2;
    pub const SET_AMPLIFIER: u32 = 0x3;
    pub const VENDOR_ID: u32 = 0x00;
    pub const NODE_COUNT: u32 = 0x04;
    pub const FUNCTION_TYPE: u32 = 0x05;
    pub const WIDGET_CAPABILITIES: u32 = 0x09;
    pub const PIN_CAPABILITIES: u32 = 0x0c;
    pub const CONNECTION_LENGTH: u32 = 0x0e;
    pub const OUTPUT_AMPLIFIER: u32 = 0x12;
}

/// A widget's type, from its capabilities' bits 23..20.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetType {
    Output,
    Input,
    Mixer,
    Selector,
    Pin,
    Other,
}

impl WidgetType {
    /// The type `capabilities` name.
    pub fn of(capabilities: u32) -> Self {
        match capabilities >> 20 & 0xf {
            0 => WidgetType::Output,
            1 => WidgetType::Input,
            2 => WidgetType::Mixer,
            3 => WidgetType::Selector,
            4 => WidgetType::Pin,
            _ => WidgetType::Other,
        }
    }
}

/// The kind of jack a pin's configuration default names, for the outputs the driver plays to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jack {
    LineOut,
    Speaker,
    Headphone,
}

/// The jack a pin's configuration default `config` describes, when it is an output jack that
/// is connected.
pub fn output_jack(config: u32) -> Option<Jack> {
    let unconnected = config >> 30 == 1;
    if unconnected {
        return None;
    }
    match config >> 20 & 0xf {
        0 => Some(Jack::LineOut),
        1 => Some(Jack::Speaker),
        2 => Some(Jack::Headphone),
        _ => None,
    }
}

/// A command for the CORB: `codec`'s node `node`, a 12-bit verb with an 8-bit payload or a
/// 4-bit verb with a 16-bit payload.
pub fn command(codec: u8, node: u8, verb: u32, payload: u32) -> u32 {
    let body = match verb > 0xf {
        true => (verb & 0xfff) << 8 | payload & 0xff,
        false => (verb & 0xf) << 16 | payload & 0xffff,
    };
    u32::from(codec & 0xf) << 28 | u32::from(node) << 20 | body
}

/// The stream format word for 16-bit samples at `format`'s rate (48 kHz or 44.1 kHz) and
/// channels.
pub fn format_word(format: PcmFormat) -> u16 {
    let base: u16 = if format.rate == 44_100 { 1 << 14 } else { 0 };
    let bits_16: u16 = 1 << 4;
    base | bits_16 | u16::from(format.channels.max(1) - 1)
}

/// One widget as the path search sees it: its node, type, connections and, for a pin, the jack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Widget {
    pub node: u8,
    pub kind: WidgetType,
    pub connections: [u8; MOST_CONNECTIONS],
    pub connection_count: usize,
    pub jack: Option<Jack>,
    pub plugged: bool,
}

/// The output path to play through: the pin first, the converter last, each step with the
/// index of the connection it takes; the pin chosen is a plugged headphone jack, else a
/// speaker, else a line-out, else an unplugged headphone jack, each the first such found.
pub fn output_path(widgets: &[Widget]) -> Option<([(u8, usize); MOST_DEPTH], usize)> {
    let rank = |widget: &Widget| match (widget.jack, widget.plugged) {
        (Some(Jack::Headphone), true) => Some(0),
        (Some(Jack::Speaker), _) => Some(1),
        (Some(Jack::LineOut), _) => Some(2),
        (Some(Jack::Headphone), false) => Some(3),
        _ => None,
    };
    for wanted in 0..4 {
        let pins = widgets
            .iter()
            .filter(|widget| widget.kind == WidgetType::Pin && rank(widget) == Some(wanted));
        for pin in pins {
            let mut path = [(0u8, 0usize); MOST_DEPTH];
            path[0] = (pin.node, 0);
            if let Some(length) = search(widgets, pin, &mut path, 1) {
                return Some((path, length));
            }
        }
    }
    None
}

fn search(
    widgets: &[Widget],
    from: &Widget,
    path: &mut [(u8, usize); MOST_DEPTH],
    depth: usize,
) -> Option<usize> {
    if depth >= MOST_DEPTH {
        return None;
    }
    for (index, &node) in from.connections[..from.connection_count].iter().enumerate() {
        let Some(next) = widgets.iter().find(|widget| widget.node == node) else {
            continue;
        };
        if path[..depth].iter().any(|&(seen, _)| seen == node) {
            continue;
        }
        path[depth - 1].1 = index;
        path[depth] = (node, 0);
        match next.kind {
            WidgetType::Output => return Some(depth + 1),
            WidgetType::Mixer | WidgetType::Selector => {
                if let Some(length) = search(widgets, next, path, depth + 1) {
                    return Some(length);
                }
            }
            _ => {}
        }
    }
    None
}

/// An Intel HD Audio controller playing through one codec's output path, polled.
#[derive(Debug)]
pub struct HdAudio<R: RegisterIo, M: DmaMemory> {
    regs: R,
    memory: M,
    codec: u8,
    jack: Jack,
    stream: usize,
    corb_write: u16,
    rirb_read: u16,
    cursor: usize,
    started: bool,
}

impl<R: RegisterIo, M: DmaMemory> HdAudio<R, M> {
    /// Brings the controller behind `regs` out of reset with `memory` ([`MEMORY_BYTES`] or
    /// more) for its rings and buffer, finds a codec and its output path and sets the first
    /// output stream up, polling while `keep_waiting` says to. On an error the memory comes
    /// back with it: the controller is left in reset, holding none of it.
    pub fn new(
        regs: R,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, M)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, memory));
        }
        let mut audio = Self {
            regs,
            memory,
            codec: 0,
            jack: Jack::Speaker,
            stream: 0,
            corb_write: 0,
            rirb_read: 0,
            cursor: 0,
            started: false,
        };
        match audio.start(keep_waiting) {
            Ok(()) => Ok(audio),
            Err(error) => {
                audio.regs.write32(register::GCTL, 0);
                Err((error, audio.memory))
            }
        }
    }

    fn start(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        self.regs.write32(register::GCTL, 0);
        self.wait(keep_waiting, |regs| regs.read32(register::GCTL) & 1 == 0)?;
        self.regs.write32(register::GCTL, 1);
        self.wait(keep_waiting, |regs| regs.read32(register::GCTL) & 1 == 1)?;
        let mut settle = 0;
        while settle < 1000 && keep_waiting() {
            settle += 1;
        }
        let present = self.regs.read16(register::STATESTS);
        if present == 0 {
            return Err(Error::NoCodec);
        }
        self.codec = present.trailing_zeros() as u8;
        self.set_up_rings();
        let capabilities = self.regs.read16(register::GCAP);
        let inputs = usize::from(capabilities >> 8 & 0xf);
        let outputs = usize::from(capabilities >> 12 & 0xf);
        if outputs == 0 {
            return Err(Error::NoOutputStream);
        }
        self.stream = register::STREAMS + inputs * register::STREAM_STRIDE;
        let dac = self.set_up_codec(keep_waiting)?;
        self.set_up_stream(dac, keep_waiting)
    }

    fn wait(
        &self,
        keep_waiting: &mut dyn FnMut() -> bool,
        done: impl Fn(&R) -> bool,
    ) -> Result<(), Error> {
        loop {
            if done(&self.regs) {
                return Ok(());
            }
            if !keep_waiting() {
                return Err(Error::ResetTimedOut);
            }
        }
    }

    fn dma(&self, offset: usize) -> u64 {
        self.memory.dma_addr().wrapping_add(offset as u64)
    }

    fn set_up_rings(&mut self) {
        let regs = &self.regs;
        regs.write8(register::CORBCTL, 0);
        regs.write8(register::RIRBCTL, 0);
        regs.write8(register::CORBSIZE, 2);
        regs.write8(register::RIRBSIZE, 2);
        let corb = self.dma(CORB_OFFSET);
        let rirb = self.dma(RIRB_OFFSET);
        regs.write32(register::CORBLBASE, corb as u32);
        regs.write32(register::CORBUBASE, (corb >> 32) as u32);
        regs.write32(register::RIRBLBASE, rirb as u32);
        regs.write32(register::RIRBUBASE, (rirb >> 32) as u32);
        regs.write16(register::CORBRP, 1 << 15);
        regs.write16(register::CORBRP, 0);
        regs.write16(register::CORBWP, 0);
        regs.write16(register::RIRBWP, 1 << 15);
        regs.write16(register::RINTCNT, 1);
        regs.write8(register::CORBCTL, 1 << 1);
        regs.write8(register::RIRBCTL, 1 << 1);
        self.corb_write = 0;
        self.rirb_read = 0;
    }

    fn send(
        &mut self,
        node: u8,
        verb: u32,
        payload: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<u32, Error> {
        let next = (self.corb_write + 1) % RING_ENTRIES;
        let entry = self
            .memory
            .as_mut_ptr()
            .wrapping_add(CORB_OFFSET + usize::from(next) * 4)
            .cast::<u32>();
        // SAFETY: a CORB entry in the driver's DMA memory, read by the controller once posted.
        unsafe { entry.write_volatile(command(self.codec, node, verb, payload)) };
        self.corb_write = next;
        self.regs.write16(register::CORBWP, next);
        let wanted = (self.rirb_read + 1) % RING_ENTRIES;
        while self.regs.read16(register::RIRBWP) & 0xff == self.rirb_read {
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
        }
        self.rirb_read = wanted;
        let response = self
            .memory
            .as_mut_ptr()
            .wrapping_add(RIRB_OFFSET + usize::from(wanted) * 8)
            .cast::<u32>();
        // SAFETY: a RIRB entry in the driver's DMA memory, written before the write pointer moved.
        Ok(unsafe { response.read_volatile() })
    }

    fn parameter(
        &mut self,
        node: u8,
        which: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<u32, Error> {
        self.send(node, verb::GET_PARAMETER, which, keep_waiting)
    }

    fn widget(
        &mut self,
        node: u8,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Widget, Error> {
        let capabilities = self.parameter(node, verb::WIDGET_CAPABILITIES, keep_waiting)?;
        let kind = WidgetType::of(capabilities);
        let mut widget = Widget {
            node,
            kind,
            connections: [0; MOST_CONNECTIONS],
            connection_count: 0,
            jack: None,
            plugged: false,
        };
        let has_list = capabilities & 1 << 8 != 0;
        if has_list {
            let length = self.parameter(node, verb::CONNECTION_LENGTH, keep_waiting)?;
            let long_form = length & 1 << 7 != 0;
            let count = (length & 0x7f) as usize;
            if !long_form {
                let mut index = 0;
                while index < count.min(MOST_CONNECTIONS) {
                    let entries = self.send(
                        node,
                        verb::GET_CONNECTION_ENTRY,
                        index as u32,
                        keep_waiting,
                    )?;
                    for part in 0..4 {
                        if index + part < count.min(MOST_CONNECTIONS) {
                            widget.connections[index + part] = (entries >> (part * 8)) as u8;
                        }
                    }
                    index += 4;
                }
                widget.connection_count = count.min(MOST_CONNECTIONS);
            }
        }
        if kind == WidgetType::Pin {
            let pin = self.parameter(node, verb::PIN_CAPABILITIES, keep_waiting)?;
            let output_capable = pin & 1 << 4 != 0;
            let config = self.send(node, verb::GET_CONFIG_DEFAULT, 0, keep_waiting)?;
            widget.jack = output_capable.then(|| output_jack(config)).flatten();
            if pin & 1 << 2 != 0 {
                let sense = self.send(node, verb::GET_PIN_SENSE, 0, keep_waiting)?;
                widget.plugged = sense & 1 << 31 != 0;
            }
        }
        Ok(widget)
    }

    fn set_up_codec(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<u8, Error> {
        let root = self.parameter(0, verb::NODE_COUNT, keep_waiting)?;
        let (first_group, groups) = ((root >> 16) as u8, root as u8);
        let mut widgets = [Widget {
            node: 0,
            kind: WidgetType::Other,
            connections: [0; MOST_CONNECTIONS],
            connection_count: 0,
            jack: None,
            plugged: false,
        }; MOST_NODES as usize];
        let mut count = 0;
        let mut audio_group = None;
        for group in first_group..first_group.saturating_add(groups) {
            let kind = self.parameter(group, verb::FUNCTION_TYPE, keep_waiting)?;
            if kind & 0xff == 1 {
                audio_group = Some(group);
                break;
            }
        }
        let group = audio_group.ok_or(Error::NoOutputPath)?;
        self.send(group, verb::SET_POWER_STATE, 0, keep_waiting)?;
        let nodes = self.parameter(group, verb::NODE_COUNT, keep_waiting)?;
        let (first, total) = ((nodes >> 16) as u8, nodes as u8);
        for node in first..first.saturating_add(total) {
            if count >= widgets.len() {
                break;
            }
            widgets[count] = self.widget(node, keep_waiting)?;
            count += 1;
        }
        let (path, length) = output_path(&widgets[..count]).ok_or(Error::NoOutputPath)?;
        for &(node, connection) in &path[..length] {
            self.send(node, verb::SET_POWER_STATE, 0, keep_waiting)?;
            let amplifier = self.parameter(node, verb::OUTPUT_AMPLIFIER, keep_waiting)?;
            let unity = amplifier & 0x7f;
            self.send(node, verb::SET_AMPLIFIER, 0xb000 | unity, keep_waiting)?;
            let input = 0x7000 | (connection as u32 & 0xf) << 8;
            self.send(node, verb::SET_AMPLIFIER, input, keep_waiting)?;
            self.send(
                node,
                verb::SET_CONNECTION_SELECT,
                connection as u32,
                keep_waiting,
            )?;
        }
        let (pin, _) = path[0];
        self.jack = widgets[..count]
            .iter()
            .find(|widget| widget.node == pin)
            .and_then(|widget| widget.jack)
            .unwrap_or(Jack::Speaker);
        let control = if self.jack == Jack::Headphone { 0xc0 } else { 0x40 };
        self.send(pin, verb::SET_PIN_CONTROL, control, keep_waiting)?;
        self.send(pin, verb::SET_EAPD, 2, keep_waiting)?;
        let (dac, _) = path[length - 1];
        Ok(dac)
    }

    fn set_up_stream(
        &mut self,
        dac: u8,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let stream = self.stream;
        let regs = &self.regs;
        regs.write8(stream + register::SD_CTL, 1);
        while regs.read8(stream + register::SD_CTL) & 1 == 0 {
            if !keep_waiting() {
                return Err(Error::ResetTimedOut);
            }
        }
        regs.write8(stream + register::SD_CTL, 0);
        while regs.read8(stream + register::SD_CTL) & 1 != 0 {
            if !keep_waiting() {
                return Err(Error::ResetTimedOut);
            }
        }
        for period in 0..PERIODS {
            let entry = self
                .memory
                .as_mut_ptr()
                .wrapping_add(BDL_OFFSET + period * 16)
                .cast::<u32>();
            let address = self.dma(BUFFER_OFFSET + period * PERIOD_BYTES);
            let words = [address as u32, (address >> 32) as u32, PERIOD_BYTES as u32, 1];
            for (word, value) in words.into_iter().enumerate() {
                // SAFETY: a descriptor in the driver's DMA memory, read once the stream runs.
                unsafe { entry.wrapping_add(word).write_volatile(value) };
            }
        }
        let buffer = self.memory.as_mut_ptr().wrapping_add(BUFFER_OFFSET);
        for index in 0..PERIODS * PERIOD_BYTES {
            // SAFETY: inside the cyclic buffer in the driver's DMA memory, not yet played.
            unsafe { buffer.wrapping_add(index).write_volatile(0) };
        }
        let bdl = self.dma(BDL_OFFSET);
        let regs = &self.regs;
        regs.write32(stream + register::SD_BDPL, bdl as u32);
        regs.write32(stream + register::SD_BDPU, (bdl >> 32) as u32);
        regs.write32(stream + register::SD_CBL, (PERIODS * PERIOD_BYTES) as u32);
        regs.write16(stream + register::SD_LVI, (PERIODS - 1) as u16);
        regs.write16(stream + register::SD_FMT, format_word(FORMAT));
        regs.write8(stream + register::SD_CTL + 2, (STREAM_TAG << 4) as u8);
        self.send(dac, verb::SET_FORMAT, u32::from(format_word(FORMAT)), keep_waiting)?;
        self.send(dac, verb::SET_STREAM_CHANNEL, STREAM_TAG << 4, keep_waiting)?;
        Ok(())
    }

    /// The format the output plays.
    pub fn format(&self) -> PcmFormat {
        FORMAT
    }

    /// The jack the output plays to.
    pub fn jack(&self) -> Jack {
        self.jack
    }

    fn played_position(&self) -> usize {
        self.regs.read32(self.stream + register::SD_LPIB) as usize % (PERIODS * PERIOD_BYTES)
    }

    /// The bytes the cyclic buffer can take now without overwriting what has not played.
    pub fn room(&self) -> usize {
        let size = PERIODS * PERIOD_BYTES;
        if !self.started {
            return size - self.cursor;
        }
        let played = self.played_position();
        (played + 2 * size - self.cursor - FRAME_BYTES) % size
    }

    /// Copies the start of `samples` (interleaved 16-bit little-endian stereo frames) into the
    /// cyclic buffer, as much as [`Self::room`] allows, and runs the stream once a period is
    /// in; how many bytes it took.
    pub fn write(&mut self, samples: &[u8]) -> usize {
        let size = PERIODS * PERIOD_BYTES;
        let length = samples.len().min(self.room()) / FRAME_BYTES * FRAME_BYTES;
        let buffer = self.memory.as_mut_ptr().wrapping_add(BUFFER_OFFSET);
        for (index, &byte) in samples[..length].iter().enumerate() {
            let at = (self.cursor + index) % size;
            // SAFETY: `at` is in the cyclic buffer in the driver's DMA memory, not yet played.
            unsafe { buffer.wrapping_add(at).write_volatile(byte) };
        }
        let filled = self.cursor + length;
        self.cursor = filled % size;
        if !self.started && filled >= PERIOD_BYTES {
            let control = self.regs.read8(self.stream + register::SD_CTL);
            self.regs.write8(self.stream + register::SD_CTL, control | 1 << 1);
            self.started = true;
        }
        length
    }

    /// Stops the stream and silences the buffer.
    pub fn stop(&mut self) {
        let control = self.regs.read8(self.stream + register::SD_CTL);
        self.regs
            .write8(self.stream + register::SD_CTL, control & !(1 << 1));
        self.started = false;
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(node: u8, kind: WidgetType, connections: &[u8], jack: Option<Jack>) -> Widget {
        let mut list = [0u8; MOST_CONNECTIONS];
        list[..connections.len()].copy_from_slice(connections);
        Widget {
            node,
            kind,
            connections: list,
            connection_count: connections.len(),
            jack,
            plugged: false,
        }
    }

    #[test]
    fn commands_and_formats_are_packed_as_the_specification_says() {
        assert_eq!(command(0, 0, verb::GET_PARAMETER, verb::VENDOR_ID), 0x000f_0000);
        assert_eq!(command(2, 0x14, verb::SET_PIN_CONTROL, 0x40), 0x2147_0740);
        assert_eq!(command(0, 3, verb::SET_FORMAT, 0x0011), 0x0032_0011);
        assert_eq!(format_word(FORMAT), 0x0011);
        assert_eq!(
            format_word(PcmFormat {
                rate: 44_100,
                channels: 2
            }),
            0x4011
        );
        assert_eq!(output_jack(0x0121_4010), Some(Jack::Headphone));
        assert_eq!(output_jack(0x9017_0110), Some(Jack::Speaker));
        assert_eq!(output_jack(0x4000_0000), None);
    }

    #[test]
    fn the_path_runs_from_a_plugged_headphone_pin_through_a_mixer_to_a_converter() {
        let mut headphone = widget(0x21, WidgetType::Pin, &[0x0c], Some(Jack::Headphone));
        headphone.plugged = true;
        let widgets = [
            widget(0x02, WidgetType::Output, &[], None),
            widget(0x03, WidgetType::Output, &[], None),
            widget(0x0c, WidgetType::Mixer, &[0x0b, 0x03], None),
            widget(0x14, WidgetType::Pin, &[0x02], Some(Jack::Speaker)),
            headphone,
        ];
        let (path, length) = output_path(&widgets).unwrap();
        assert_eq!(&path[..length], &[(0x21, 0), (0x0c, 1), (0x03, 0)]);
        let without_headphones = &widgets[..4];
        let (path, length) = output_path(without_headphones).unwrap();
        assert_eq!(&path[..length], &[(0x14, 0), (0x02, 0)]);
        assert_eq!(output_path(&widgets[2..3]), None);
    }
}
