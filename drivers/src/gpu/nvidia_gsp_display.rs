//! NVIDIA's displays through GSP-RM, part 1 (D524): which screens are there. GSP-RM keeps
//! objects in a tree of handles a client names itself: a root client, the GPU's device under
//! it, and the display's common object under that, made by remote "alloc" calls; questions go
//! to an object as remote "control" calls, a command number and its parameters, which GSP-RM
//! answers in the same call with its status. The display common object says which display ids
//! the board has, which of them have a screen connected, and each screen's EDID.
//!
//! Part 2 (D529) brings the display engine's core channel up: GSP-RM's own internal client,
//! device and subdevice (from its static information) take the display's instance memory and
//! the core channel's push buffer; the display object and the core channel are made under the
//! driver's device; methods (`count << 18 | offset`) then go through the push buffer, the
//! channel's PUT register in BAR 0 moved past them. Layouts and numbers are NVIDIA's r535 ABI
//! (the MIT-licensed open GPU kernel modules, `NVIDIA-NOTICE.md`).

use alloc::vec;
use alloc::vec::Vec;

/// The remote call making an object.
pub const ALLOC: u32 = 103;
/// The remote call asking an object.
pub const CONTROL: u32 = 76;
/// The handle of the client EggOS makes.
pub const CLIENT: u32 = 0xc1d0_0000;
/// The handle of the GPU's device object.
pub const DEVICE: u32 = 0xde1d_0000;
/// The handle of the display's common object.
pub const DISPLAY: u32 = 0xd150_0000;

const ROOT_CLASS: u32 = 0x0000;
const DEVICE_CLASS: u32 = 0x0080;
const DISPLAY_COMMON_CLASS: u32 = 0x0073;
const GET_SUPPORTED: u32 = 0x0073_0120;
const GET_CONNECT_STATE: u32 = 0x0073_0122;
const GET_EDID: u32 = 0x0073_0245;
const ALLOC_HEADER_BYTES: usize = 32;
const CONTROL_HEADER_BYTES: usize = 40;
const ALLOC_STATUS: usize = 16;
const CONTROL_STATUS: usize = 12;
const ROOT_PARAMETERS_BYTES: usize = 108;
const DEVICE_PARAMETERS_BYTES: usize = 56;
const NO_PROCESS: u32 = u32::MAX;
const EDID_HEADER_BYTES: usize = 16;
const EDID_MOST_BYTES: usize = 2048;

/// The remote call asking GSP-RM's static information.
pub const STATIC_INFO: u32 = 65;
/// The bytes of GSP-RM's static information (r535).
pub const STATIC_INFO_BYTES: usize = 2168;
const INTERNAL_HANDLES_AT: usize = 2152;
const WRITE_INSTANCE_MEMORY: u32 = 0x2080_0a49;
const DISPLAY_STATIC_INFO: u32 = 0x2080_0a01;
const CHANNEL_PUSH_BUFFER: u32 = 0x2080_0a58;
const FRAMEBUFFER_MEMORY: u32 = 2;
const SYSTEM_MEMORY: u32 = 1;
const WRITE_COMBINED: u32 = 2;
const CHANNEL_PARAMETERS_BYTES: usize = 32;
const METHOD_COUNT_SHIFT: u32 = 18;
/// The core channel's PUT register in BAR 0.
pub const CORE_PUT: usize = 0x68_0000;
/// The core channel's GET register in BAR 0.
pub const CORE_GET: usize = 0x68_0004;
/// The core channel's update method, which applies what was pushed before it.
pub const CORE_UPDATE: u32 = 0x0200;
/// The bytes of the display's instance memory.
pub const INSTANCE_BYTES: u64 = 0x1_0000;

/// GSP-RM's internal client, device and subdevice handles in its static information `answer`.
pub fn internal_handles(answer: &[u8]) -> Option<(u32, u32, u32)> {
    Some((
        word(answer, INTERNAL_HANDLES_AT)?,
        word(answer, INTERNAL_HANDLES_AT + 4)?,
        word(answer, INTERNAL_HANDLES_AT + 8)?,
    ))
}

/// The display engine's classes on a chip of architecture `architecture`: the display object,
/// the core channel, the window channel and the window immediate channel; `None` before
/// Ampere.
pub fn display_classes(architecture: u16) -> Option<(u32, u32, u32, u32)> {
    match architecture {
        0x17 => Some((0xc670, 0xc67d, 0xc67e, 0xc67b)),
        0x19 => Some((0xc770, 0xc77d, 0xc67e, 0xc67b)),
        _ => None,
    }
}

/// A method header: `count` data words for the methods from byte offset `offset` on.
pub fn method(offset: u32, count: u32) -> u32 {
    count << METHOD_COUNT_SHIFT | offset
}

/// What part 2 needs: GSP-RM's internal client and subdevice, the chip's display classes, the
/// display's instance memory in video memory (its offset), and the core channel's push buffer
/// in system memory (its device address and bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreSetup {
    pub internal_client: u32,
    pub internal_subdevice: u32,
    pub classes: (u32, u32, u32, u32),
    pub instance_memory: u64,
    pub push_buffer: (u64, u64),
}

/// What the display engine has: its heads and the windows present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayEngineInfo {
    pub heads: u32,
    pub windows: u32,
}

/// Hands the display its instance memory, asks its heads and windows, makes the display
/// object, registers the core channel's push buffer and makes the core channel (D529), through
/// `call`; the [`find_sinks`] objects must exist.
pub fn bring_up_core(
    call: &mut dyn FnMut(u32, &[u8]) -> Result<Vec<u8>, &'static str>,
    setup: &CoreSetup,
) -> Result<DisplayEngineInfo, DisplayError> {
    let mut internal = |command: u32, parameters: &[u8]| -> Result<Vec<u8>, DisplayError> {
        let payload =
            control_call(setup.internal_client, setup.internal_subdevice, command, parameters);
        let answer = call(CONTROL, &payload).map_err(DisplayError::Call)?;
        let returned = answered(&answer, CONTROL_STATUS, CONTROL_HEADER_BYTES)?;
        returned
            .get(..parameters.len())
            .map(<[u8]>::to_vec)
            .ok_or(DisplayError::ShortAnswer)
    };
    let mut instance = [0u8; 24];
    instance[0..8].copy_from_slice(&setup.instance_memory.to_le_bytes());
    instance[8..16].copy_from_slice(&INSTANCE_BYTES.to_le_bytes());
    put(&mut instance, 16, FRAMEBUFFER_MEMORY);
    put(&mut instance, 20, WRITE_COMBINED);
    internal(WRITE_INSTANCE_MEMORY, &instance)?;
    let info = internal(DISPLAY_STATIC_INFO, &[0u8; 28])?;
    let engine = DisplayEngineInfo {
        windows: word(&info, 4).ok_or(DisplayError::ShortAnswer)?,
        heads: word(&info, 12).ok_or(DisplayError::ShortAnswer)?,
    };
    let (root, core, _, _) = setup.classes;
    let mut push = [0u8; 40];
    put(&mut push, 0, SYSTEM_MEMORY);
    push[8..16].copy_from_slice(&setup.push_buffer.0.to_le_bytes());
    push[16..24].copy_from_slice(&(setup.push_buffer.1 - 1).to_le_bytes());
    put(&mut push, 24, 1);
    put(&mut push, 28, core);
    push[36] = 1;
    internal(CHANNEL_PUSH_BUFFER, &push)?;
    let mut alloc = |parent: u32, object: u32, class: u32, parameters: &[u8]| {
        let answer = call(ALLOC, &alloc_call(CLIENT, parent, object, class, parameters))
            .map_err(DisplayError::Call)?;
        answered(&answer, ALLOC_STATUS, ALLOC_HEADER_BYTES).map(|_| ())
    };
    alloc(DEVICE, root << 16, root, &[])?;
    alloc(root << 16, core << 16, core, &[0u8; CHANNEL_PARAMETERS_BYTES])?;
    Ok(engine)
}

/// Why the screens are not known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayError {
    /// The remote call itself failed; the reason.
    Call(&'static str),
    /// GSP-RM refused, with this status.
    Refused(u32),
    /// The answer is shorter than its parameters.
    ShortAnswer,
}

fn put(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// The payload of an [`ALLOC`] call making `object` of `class` under `parent`, for client
/// `client`, with `parameters`.
pub fn alloc_call(
    client: u32,
    parent: u32,
    object: u32,
    class: u32,
    parameters: &[u8],
) -> Vec<u8> {
    let mut payload = vec![0u8; ALLOC_HEADER_BYTES + parameters.len()];
    put(&mut payload, 0, client);
    put(&mut payload, 4, parent);
    put(&mut payload, 8, object);
    put(&mut payload, 12, class);
    put(&mut payload, 20, parameters.len() as u32);
    payload[ALLOC_HEADER_BYTES..].copy_from_slice(parameters);
    payload
}

/// The payload of a [`CONTROL`] call sending `command` to `object` of client `client`, with
/// `parameters`.
pub fn control_call(client: u32, object: u32, command: u32, parameters: &[u8]) -> Vec<u8> {
    let mut payload = vec![0u8; CONTROL_HEADER_BYTES + parameters.len()];
    put(&mut payload, 0, client);
    put(&mut payload, 4, object);
    put(&mut payload, 8, command);
    put(&mut payload, 16, parameters.len() as u32);
    payload[CONTROL_HEADER_BYTES..].copy_from_slice(parameters);
    payload
}

fn answered(answer: &[u8], status_at: usize, header: usize) -> Result<&[u8], DisplayError> {
    match word(answer, status_at).ok_or(DisplayError::ShortAnswer)? {
        0 => answer.get(header..).ok_or(DisplayError::ShortAnswer),
        status => Err(DisplayError::Refused(status)),
    }
}

/// A screen GSP-RM found: its display id (one bit of the display masks) and its EDID's bytes,
/// when it gave one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sink {
    pub display: u32,
    pub edid: Option<Vec<u8>>,
}

/// Makes the client, the device and the display common object through `call` (a remote call:
/// its function and payload in, the answer's payload out), then asks which displays have a
/// screen and each screen's EDID.
pub fn find_sinks(
    call: &mut dyn FnMut(u32, &[u8]) -> Result<Vec<u8>, &'static str>,
) -> Result<Vec<Sink>, DisplayError> {
    let mut alloc = |parent: u32, object: u32, class: u32, parameters: &[u8]| {
        let answer = call(ALLOC, &alloc_call(CLIENT, parent, object, class, parameters))
            .map_err(DisplayError::Call)?;
        answered(&answer, ALLOC_STATUS, ALLOC_HEADER_BYTES).map(|_| ())
    };
    let mut root = [0u8; ROOT_PARAMETERS_BYTES];
    put(&mut root, 0, CLIENT);
    put(&mut root, 4, NO_PROCESS);
    alloc(CLIENT, CLIENT, ROOT_CLASS, &root)?;
    let mut device = [0u8; DEVICE_PARAMETERS_BYTES];
    put(&mut device, 4, CLIENT);
    alloc(CLIENT, DEVICE, DEVICE_CLASS, &device)?;
    alloc(DEVICE, DISPLAY, DISPLAY_COMMON_CLASS, &[])?;
    let mut control = |command: u32, parameters: &[u8]| -> Result<Vec<u8>, DisplayError> {
        let answer = call(CONTROL, &control_call(CLIENT, DISPLAY, command, parameters))
            .map_err(DisplayError::Call)?;
        let returned = answered(&answer, CONTROL_STATUS, CONTROL_HEADER_BYTES)?;
        returned
            .get(..parameters.len())
            .map(<[u8]>::to_vec)
            .ok_or(DisplayError::ShortAnswer)
    };
    let supported = control(GET_SUPPORTED, &[0u8; 12])?;
    let mask = word(&supported, 4).ok_or(DisplayError::ShortAnswer)?;
    let mut question = [0u8; 16];
    put(&mut question, 8, mask);
    let connected = control(GET_CONNECT_STATE, &question)?;
    let connected = word(&connected, 8).ok_or(DisplayError::ShortAnswer)? & mask;
    let mut sinks = Vec::new();
    for display in (0..32).filter(|bit| connected & 1 << bit != 0) {
        let mut question = vec![0u8; EDID_HEADER_BYTES + EDID_MOST_BYTES];
        put(&mut question, 4, 1 << display);
        put(&mut question, 8, EDID_MOST_BYTES as u32);
        let edid = control(GET_EDID, &question).ok().and_then(|answer| {
            let bytes = (word(&answer, 8)? as usize).min(EDID_MOST_BYTES);
            (bytes > 0).then(|| answer[EDID_HEADER_BYTES..EDID_HEADER_BYTES + bytes].to_vec())
        });
        sinks.push(Sink { display, edid });
    }
    Ok(sinks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer_to(function: u32, payload: &[u8]) -> Result<Vec<u8>, &'static str> {
        let mut answer = payload.to_vec();
        if function == CONTROL {
            let parameters = &mut answer[CONTROL_HEADER_BYTES..];
            match word(payload, 8).unwrap() {
                GET_SUPPORTED => put(parameters, 4, 0b0110),
                GET_CONNECT_STATE => put(parameters, 8, 0b0100),
                GET_EDID => {
                    put(parameters, 8, 3);
                    let edid = &mut parameters[EDID_HEADER_BYTES..EDID_HEADER_BYTES + 3];
                    edid.copy_from_slice(&[1, 2, 3]);
                }
                _ => put(&mut answer, CONTROL_STATUS, 0x56),
            }
        }
        Ok(answer)
    }

    #[test]
    fn connected_screens_come_with_their_edids() {
        let mut calls = Vec::new();
        let sinks = find_sinks(&mut |function, payload| {
            calls.push((function, word(payload, 12).unwrap(), word(payload, 8).unwrap()));
            answer_to(function, payload)
        })
        .unwrap();
        assert_eq!(sinks, vec![Sink { display: 2, edid: Some(vec![1, 2, 3]) }]);
        assert_eq!(calls[0], (ALLOC, ROOT_CLASS, CLIENT));
        assert_eq!(calls[2], (ALLOC, DISPLAY_COMMON_CLASS, DISPLAY));
        assert_eq!(calls.len(), 6);
    }

    #[test]
    fn the_core_comes_up_through_internal_controls_and_two_allocs() {
        let mut calls = Vec::new();
        let setup = CoreSetup {
            internal_client: 0xc1e0_0004,
            internal_subdevice: 0x5c00_0003,
            classes: display_classes(0x17).unwrap(),
            instance_memory: 0x1_ff00_0000,
            push_buffer: (0x8000_0000, 0x1000),
        };
        let engine = bring_up_core(
            &mut |function, payload| {
                calls.push((function, word(payload, 0).unwrap(), word(payload, 8).unwrap()));
                let mut answer = payload.to_vec();
                if function == CONTROL && word(payload, 8) == Some(DISPLAY_STATIC_INFO) {
                    put(&mut answer[CONTROL_HEADER_BYTES..], 4, 0xf);
                    put(&mut answer[CONTROL_HEADER_BYTES..], 12, 4);
                }
                Ok(answer)
            },
            &setup,
        )
        .unwrap();
        assert_eq!(engine, DisplayEngineInfo { heads: 4, windows: 0xf });
        assert_eq!(calls[0], (CONTROL, 0xc1e0_0004, WRITE_INSTANCE_MEMORY));
        assert_eq!(calls[2], (CONTROL, 0xc1e0_0004, CHANNEL_PUSH_BUFFER));
        assert_eq!(calls[4], (ALLOC, CLIENT, 0xc67d << 16));
        assert_eq!(method(CORE_UPDATE, 1), 1 << 18 | 0x200);
    }

    #[test]
    fn a_refused_alloc_stops_the_search() {
        let result = find_sinks(&mut |_, payload| {
            let mut answer = payload.to_vec();
            put(&mut answer, ALLOC_STATUS, 0x1f);
            Ok(answer)
        });
        assert_eq!(result, Err(DisplayError::Refused(0x1f)));
    }
}
