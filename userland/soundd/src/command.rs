//! `sound`'s command line and its words (docs/ux/sound-v0.1.md section 5): what each form
//! asks the server for and what it prints, in text and JSON.

use crate::protocol::{Change, Device, Kind, Request, State, TEST_TONE_MS};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The exit status of a command line written wrongly or a value out of range.
pub const USAGE_STATUS: i32 = 2;
/// The exit status of a refusal (no device, no such device).
pub const FAILURE_STATUS: i32 = 1;
/// What `sound` says with no sound device.
pub const NO_DEVICE: &str = "No sound device.";

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Status { json: bool },
    Devices { json: bool },
    Volume(Step),
    Mute(Toggle),
    Output(String),
    Input(String),
    Test { seconds: u32 },
}

/// A volume to set, or a step from the one in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    To(u8),
    By(i16),
}

/// What `sound mute` asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    On,
    Off,
    Flip,
}

/// Why a command line was refused before reaching the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Usage(String),
    InvalidValue(String),
}

/// Parses the words after `sound`.
pub fn parse(arguments: &[&str]) -> Result<Command, Refusal> {
    let usage = |text: &str| Err(Refusal::Usage(String::from(text)));
    let (verb, rest) = match arguments.split_first() {
        None => return Ok(Command::Status { json: false }),
        Some((verb, rest)) => (*verb, rest),
    };
    let json = rest.contains(&"--json");
    let operands: Vec<&str> = rest.iter().copied().filter(|word| *word != "--json").collect();
    match (verb, operands.as_slice()) {
        ("status", []) => Ok(Command::Status { json }),
        ("devices", []) => Ok(Command::Devices { json }),
        ("volume", [value]) if !json => parse_step(value).map(Command::Volume),
        ("mute", [state]) if !json => match *state {
            "on" => Ok(Command::Mute(Toggle::On)),
            "off" => Ok(Command::Mute(Toggle::Off)),
            "toggle" => Ok(Command::Mute(Toggle::Flip)),
            other => Err(Refusal::InvalidValue(format!(
                "invalid value '{other}' (on, off or toggle)"
            ))),
        },
        ("output", [id]) if !json => Ok(Command::Output(String::from(*id))),
        ("input", [id]) if !json => Ok(Command::Input(String::from(*id))),
        ("test", []) if !json => Ok(Command::Test { seconds: 1 }),
        ("test", ["--seconds", seconds]) if !json => match seconds.parse::<u32>() {
            Ok(seconds @ 1..=10) => Ok(Command::Test { seconds }),
            _ => Err(Refusal::InvalidValue(format!(
                "invalid value '{seconds}' (1 to 10 seconds)"
            ))),
        },
        _ => usage(concat!(
            "usage: sound [status|devices] [--json] | volume <0-100|+n|-n> | ",
            "mute on|off|toggle | output <id|auto> | input <id|auto> | ",
            "test [--seconds <1-10>]"
        )),
    }
}

fn parse_step(value: &str) -> Result<Step, Refusal> {
    let invalid =
        || Refusal::InvalidValue(format!("invalid value '{value}' (0 to 100, +n or -n)"));
    if let Some(up) = value.strip_prefix('+') {
        return up.parse::<i16>().map(Step::By).map_err(|_| invalid());
    }
    if value.starts_with('-') {
        return value.parse::<i16>().map(Step::By).map_err(|_| invalid());
    }
    match value.parse::<u8>() {
        Ok(volume) if volume <= 100 => Ok(Step::To(volume)),
        _ => Err(invalid()),
    }
}

/// The volume `step` makes from `volume`, clamped to 0..=100.
pub fn stepped(volume: u8, step: Step) -> u8 {
    match step {
        Step::To(volume) => volume,
        Step::By(by) => (i16::from(volume) + by).clamp(0, 100) as u8,
    }
}

/// The request a change command makes, given the state in force.
pub fn request_of(command: &Command, state: &State) -> Option<Request> {
    Some(match command {
        Command::Volume(step) => Request::Set(Change::Volume(stepped(state.volume, *step))),
        Command::Mute(toggle) => Request::Set(Change::Mute(match toggle {
            Toggle::On => true,
            Toggle::Off => false,
            Toggle::Flip => !state.mute,
        })),
        Command::Output(id) => Request::Set(Change::Output(id.clone())),
        Command::Input(id) => Request::Set(Change::Input(id.clone())),
        Command::Test { seconds } => Request::Test {
            milliseconds: seconds * TEST_TONE_MS,
        },
        Command::Status { .. } | Command::Devices { .. } => return None,
    })
}

fn name_of<'a>(devices: &'a [Device], id: &str) -> &'a str {
    devices
        .iter()
        .find(|device| device.id == id)
        .map_or(id, |device| device.name.as_str())
}

fn json_text(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// `sound status`'s text.
pub fn status_text(state: &State, devices: &[Device]) -> String {
    if state.output.is_none() && state.input.is_none() {
        return format!("{NO_DEVICE}\n");
    }
    let mut text = String::new();
    if let Some(output) = &state.output {
        text.push_str(&format!(
            "Output: {} ({output}), volume {}%{}\n",
            name_of(devices, output),
            state.volume,
            if state.mute { ", muted" } else { "" }
        ));
    }
    if let Some(input) = &state.input {
        text.push_str(&format!(
            "Input: {} ({input}), volume {}%{}\n",
            name_of(devices, input),
            state.input_volume,
            if state.input_mute { ", muted" } else { "" }
        ));
    }
    text
}

/// `sound status --json`'s record.
pub fn status_json(state: &State, devices: &[Device]) -> String {
    let side = |id: &Option<String>, volume: u8, mute: bool| match id {
        None => String::from("null"),
        Some(id) => format!(
            "{{\"id\":{},\"name\":{},\"volume\":{volume},\"mute\":{mute}}}",
            json_text(id),
            json_text(name_of(devices, id))
        ),
    };
    format!(
        "{{\"output\":{},\"input\":{}}}\n",
        side(&state.output, state.volume, state.mute),
        side(&state.input, state.input_volume, state.input_mute)
    )
}

/// `sound devices`'s text, or its JSON records, one a line.
pub fn devices_text(devices: &[Device], json: bool) -> String {
    let mut text = String::new();
    for device in devices {
        text.push_str(&match json {
            true => format!(
                "{{\"id\":{},\"kind\":\"{}\",\"name\":{}}}\n",
                json_text(&device.id),
                device.kind.word(),
                json_text(&device.name)
            ),
            false => format!("{}  {}  {}\n", device.id, device.kind.word(), device.name),
        });
    }
    text
}

/// What a change command prints once the server took it.
pub fn done_text(command: &Command, state: &State, devices: &[Device]) -> String {
    match command {
        Command::Volume(_) => format!("Volume {}%.\n", state.volume),
        Command::Mute(_) => String::from(if state.mute { "Muted.\n" } else { "Unmuted.\n" }),
        Command::Output(_) => match &state.output {
            Some(id) => format!("Output: {} ({id}).\n", name_of(devices, id)),
            None => format!("{NO_DEVICE}\n"),
        },
        Command::Input(_) => match &state.input {
            Some(id) => format!("Input: {} ({id}).\n", name_of(devices, id)),
            None => String::from("No sound input.\n"),
        },
        Command::Test { seconds } => {
            let output = state.output.as_deref().unwrap_or("none");
            format!(
                "Played a {seconds} s tone on {}.\n",
                name_of(devices, output)
            )
        }
        Command::Status { .. } | Command::Devices { .. } => String::new(),
    }
}

/// Whether a device of `kind` has `id`, or `id` is `auto`.
pub fn names_a_device(devices: &[Device], kind: Kind, id: &str) -> bool {
    id == "auto" || devices.iter().any(|device| device.kind == kind && device.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<Device> {
        Device::from_driver_text(
            "hda0-out output 48000 Built-in speakers\nhda0-in input 48000 Built-in microphone\n",
        )
    }

    #[test]
    fn every_form_parses_and_bad_values_are_refused() {
        assert_eq!(parse(&[]), Ok(Command::Status { json: false }));
        assert_eq!(parse(&["status", "--json"]), Ok(Command::Status { json: true }));
        assert_eq!(parse(&["volume", "+5"]), Ok(Command::Volume(Step::By(5))));
        assert_eq!(parse(&["volume", "-10"]), Ok(Command::Volume(Step::By(-10))));
        assert_eq!(parse(&["volume", "40"]), Ok(Command::Volume(Step::To(40))));
        assert!(matches!(parse(&["volume", "101"]), Err(Refusal::InvalidValue(_))));
        assert_eq!(parse(&["mute", "toggle"]), Ok(Command::Mute(Toggle::Flip)));
        assert_eq!(
            parse(&["test", "--seconds", "3"]),
            Ok(Command::Test { seconds: 3 })
        );
        assert!(matches!(parse(&["test", "--seconds", "11"]), Err(Refusal::InvalidValue(_))));
        assert!(matches!(parse(&["louder"]), Err(Refusal::Usage(_))));
        assert_eq!(stepped(98, Step::By(5)), 100);
        assert_eq!(stepped(3, Step::By(-5)), 0);
    }

    #[test]
    fn status_and_devices_read_as_the_design_writes_them() {
        let state = State {
            output: Some(String::from("hda0-out")),
            input: Some(String::from("hda0-in")),
            input_mute: true,
            ..State::default()
        };
        assert_eq!(
            status_text(&state, &devices()),
            "Output: Built-in speakers (hda0-out), volume 50%\n\
             Input: Built-in microphone (hda0-in), volume 50%, muted\n"
        );
        assert_eq!(
            status_json(&State::default(), &[]),
            "{\"output\":null,\"input\":null}\n"
        );
        assert_eq!(status_text(&State::default(), &[]), "No sound device.\n");
        assert_eq!(
            devices_text(&devices(), false).lines().next(),
            Some("hda0-out  output  Built-in speakers")
        );
        let toggled = request_of(&Command::Mute(Toggle::Flip), &state).unwrap();
        assert_eq!(toggled, Request::Set(Change::Mute(true)));
        assert_eq!(
            done_text(&Command::Test { seconds: 1 }, &state, &devices()),
            "Played a 1 s tone on Built-in speakers.\n"
        );
        assert!(names_a_device(&devices(), Kind::Output, "auto"));
        assert!(!names_a_device(&devices(), Kind::Output, "hda0-in"));
    }
}
