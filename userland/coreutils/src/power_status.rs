use crate::network::read_text;
use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use core::fmt::Write;
use eggos_commands::commands;
use eggos_device_list::power::{PowerFacts, write_celsius};

const POWER_PATH: &CStr = c"/proc/driver/power";
const NO_POWER_DEVICES: &[u8] = b"No power devices.\n";
const LINE_CAPACITY: usize = 160;

struct Line {
    bytes: [u8; LINE_CAPACITY],
    length: usize,
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        let place = self.bytes.get_mut(self.length..end).ok_or(core::fmt::Error)?;
        place.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

impl Line {
    fn new() -> Self {
        Self {
            bytes: [0; LINE_CAPACITY],
            length: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

pub fn power<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let arguments = match arguments.first().map(|word| word.to_bytes()) {
        Some(b"status") => &arguments[1..],
        _ => arguments,
    };
    let Ok(operands) = parse_defined(output, arguments, &commands::POWER_STATUS, |_, _| Ok(()))
    else {
        return crate::wm::USAGE_STATUS;
    };
    if !operands.all().is_empty() {
        output.coded(
            &eggos_commands::errors::USAGE,
            &[b"usage: power [status] [--json]"],
        );
        return crate::wm::USAGE_STATUS;
    }
    let facts = match read_text(&mut output.system, POWER_PATH) {
        Ok(text) => PowerFacts::parse(text.as_bytes()),
        Err(errno) => {
            output.file_error(POWER_PATH, errno);
            return FAILURE;
        }
    };
    match output.is_json() {
        true => write_json(output, &facts),
        false => write_text(output, &facts),
    }
    0
}

fn state_word(facts: &PowerFacts, charge_word: &'static str) -> &'static str {
    match facts.is_low() {
        true => "low",
        false => charge_word,
    }
}

fn write_text<S: System>(output: &mut Output<S>, facts: &PowerFacts) {
    if facts.is_empty() {
        output.write(NO_POWER_DEVICES);
        return;
    }
    let count = facts.batteries().count();
    for (index, battery) in facts.batteries().enumerate() {
        let mut line = Line::new();
        let state = state_word(facts, battery.charge.word());
        let _ = match count {
            1 => writeln!(line, "Battery: {}%, {state}", battery.percent),
            _ => writeln!(line, "Battery {}: {}%, {state}", index + 1, battery.percent),
        };
        output.write(line.as_bytes());
    }
    if let Some(online) = facts.line_power {
        output.write(match online {
            true => b"Line power: online\n",
            false => b"Line power: offline\n",
        });
    }
    if let Some(open) = facts.lid_open {
        output.write(match open {
            true => b"Lid: open\n",
            false => b"Lid: closed\n",
        });
    }
    if let Some(hottest) = facts.hottest() {
        let mut line = Line::new();
        let _ = line.write_str("Temperature: ");
        for (index, zone) in facts.zones().enumerate() {
            if index > 0 {
                let _ = line.write_str(" and ");
            }
            let _ = write_celsius(&mut line, zone.tenths_c);
            let _ = line.write_str(" °C");
        }
        let _ = line.write_str(", limit ");
        let _ = write_celsius(&mut line, hottest.limit_tenths_c);
        let _ = line.write_str(" °C\n");
        output.write(line.as_bytes());
    }
    if let Some(docked) = facts.docked {
        output.write(match docked {
            true => b"Dock: attached\n",
            false => b"Dock: detached\n",
        });
    }
}

fn write_json<S: System>(output: &mut Output<S>, facts: &PowerFacts) {
    let mut line = Line::new();
    let _ = line.write_str("{\"batteries\":[");
    for (index, battery) in facts.batteries().enumerate() {
        if index > 0 {
            let _ = line.write_str(",");
        }
        let _ = write!(
            line,
            "{{\"percent\":{},\"state\":\"{}\"}}",
            battery.percent,
            state_word(facts, battery.charge.word())
        );
    }
    let word = |fact: Option<bool>, yes: &'static str, no: &'static str| match fact {
        Some(true) => yes,
        Some(false) => no,
        None => "",
    };
    let quoted = |line: &mut Line, key: &str, value: &str| {
        let _ = match value.is_empty() {
            true => write!(line, ",\"{key}\":null"),
            false => write!(line, ",\"{key}\":\"{value}\""),
        };
    };
    let _ = line.write_str("]");
    quoted(&mut line, "line_power", word(facts.line_power, "online", "offline"));
    quoted(&mut line, "lid", word(facts.lid_open, "open", "closed"));
    for (key, limit) in [("temperatures_c", false), ("critical_c", true)] {
        let _ = write!(line, ",\"{key}\":[");
        for (index, zone) in facts.zones().enumerate() {
            if index > 0 {
                let _ = line.write_str(",");
            }
            let _ = write_celsius(
                &mut line,
                match limit {
                    true => zone.limit_tenths_c,
                    false => zone.tenths_c,
                },
            );
        }
        let _ = line.write_str("]");
    }
    quoted(&mut line, "dock", word(facts.docked, "attached", "detached"));
    let _ = line.write_str("}\n");
    output.write(line.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(power: &str, arguments: &[&CStr]) -> (i32, String) {
        let fake = Fake::default().with_file("/proc/driver/power", 0o444, power.as_bytes(), 0);
        let mut output = Output::new(fake, b"power");
        let status = power(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system.output())
    }

    const FULL: &str = "lid open\nac offline\nbattery 0 87% discharging\nthermal 0 47.0 limit 95.0\ndock detached";

    #[test]
    fn status_prints_one_line_per_fact() {
        assert_eq!(
            run(FULL, &[c"status"]),
            (
                0,
                "Battery: 87%, discharging\nLine power: offline\nLid: open\nTemperature: 47.0 °C, limit 95.0 °C\nDock: detached\n"
                    .to_owned()
            )
        );
        assert_eq!(run("none", &[]), (0, "No power devices.\n".to_owned()));
    }

    #[test]
    fn json_is_one_record() {
        assert_eq!(
            run(FULL, &[c"status", c"--json"]).1,
            "{\"batteries\":[{\"percent\":87,\"state\":\"discharging\"}],\"line_power\":\"offline\",\"lid\":\"open\",\"temperatures_c\":[47.0],\"critical_c\":[95.0],\"dock\":\"detached\"}\n"
        );
    }
}
