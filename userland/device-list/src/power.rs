//! The machine's power state as people read it (D599, docs/ux/power-v0.1.md): the lines of
//! `/proc/driver/power`, and the words `power status`, Settings' Power pane, the `power.*`
//! settings keys and the bar's battery indicator use, so every front-end says the same. No
//! allocator.

use core::fmt::Write;

/// Where the kernel tells the power state.
pub const POWER_FILE: &str = "/proc/driver/power";
/// The most batteries and thermal zones kept.
pub const MAX_DEVICES: usize = 4;
/// At or under this percent, discharging, a battery is low.
pub const LOW_BATTERY_PERCENT: u8 = 15;

/// What a battery does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charge {
    Charging,
    Discharging,
    Full,
}

impl Charge {
    /// Its word: `charging`, `discharging` or `full`.
    pub fn word(self) -> &'static str {
        match self {
            Self::Charging => "charging",
            Self::Discharging => "discharging",
            Self::Full => "full",
        }
    }
}

/// One battery: its charge in percent and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    pub charge: Charge,
}

/// One thermal zone: its temperature and its critical point, in tenths of a degree Celsius.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    pub tenths_c: i32,
    pub limit_tenths_c: i32,
}

/// The power state, `None` for what the machine does not report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerFacts {
    pub batteries: [Option<Battery>; MAX_DEVICES],
    pub line_power: Option<bool>,
    pub lid_open: Option<bool>,
    pub zones: [Option<Zone>; MAX_DEVICES],
    pub docked: Option<bool>,
}

fn tenths(text: &str) -> Option<i32> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, "0"));
    let negative = whole.starts_with('-');
    let whole: i32 = whole.trim_start_matches('-').parse().ok()?;
    let fraction: i32 = fraction.get(..1)?.parse().ok()?;
    let value = whole * 10 + fraction;
    Some(if negative { -value } else { value })
}

fn put<T: Copy>(slots: &mut [Option<T>; MAX_DEVICES], value: T) {
    if let Some(slot) = slots.iter_mut().find(|slot| slot.is_none()) {
        *slot = Some(value);
    }
}

impl PowerFacts {
    /// The facts `/proc/driver/power`'s `text` gives: `lid open|closed`, `ac online|offline`,
    /// `battery <n> <percent>% charging|discharging`, `thermal <n> <celsius> limit <celsius>`
    /// and `dock attached|detached` lines.
    pub fn parse(text: &[u8]) -> Self {
        let mut facts = Self::default();
        let Ok(text) = core::str::from_utf8(text) else {
            return facts;
        };
        for line in text.lines() {
            let mut words = line.split_ascii_whitespace();
            match (words.next(), words.next(), words.next(), words.next()) {
                (Some("lid"), Some(state), ..) => facts.lid_open = Some(state == "open"),
                (Some("ac"), Some(state), ..) => facts.line_power = Some(state == "online"),
                (Some("dock"), Some(state), ..) => facts.docked = Some(state == "attached"),
                (Some("battery"), Some(_), Some(percent), Some(state)) => {
                    let Ok(percent) = percent.trim_end_matches('%').parse::<u8>() else {
                        continue;
                    };
                    let charge = match state {
                        "charging" => Charge::Charging,
                        _ => Charge::Discharging,
                    };
                    put(&mut facts.batteries, Battery { percent, charge });
                }
                (Some("thermal"), Some(_), Some(celsius), _) => {
                    let limit = words
                        .next()
                        .and_then(tenths)
                        .unwrap_or(CRITICAL_FALLBACK_TENTHS_C);
                    if let Some(tenths_c) = tenths(celsius) {
                        put(
                            &mut facts.zones,
                            Zone {
                                tenths_c,
                                limit_tenths_c: limit,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        if facts.line_power == Some(true) {
            for battery in facts.batteries.iter_mut().flatten() {
                if battery.charge == Charge::Discharging && battery.percent >= 100 {
                    battery.charge = Charge::Full;
                }
            }
        }
        facts
    }

    /// The batteries reported.
    pub fn batteries(&self) -> impl Iterator<Item = Battery> + '_ {
        self.batteries.iter().flatten().copied()
    }

    /// The thermal zones reported.
    pub fn zones(&self) -> impl Iterator<Item = Zone> + '_ {
        self.zones.iter().flatten().copied()
    }

    /// Whether the machine reports anything at all.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The batteries' mean charge, `None` without one.
    pub fn mean_percent(&self) -> Option<u8> {
        let (sum, count) = self
            .batteries()
            .fold((0u32, 0u32), |(sum, count), battery| {
                (sum + u32::from(battery.percent), count + 1)
            });
        (count > 0).then(|| (sum / count) as u8)
    }

    /// Whether the batteries are low: at or under [`LOW_BATTERY_PERCENT`], off line power.
    pub fn is_low(&self) -> bool {
        self.line_power != Some(true)
            && self
                .mean_percent()
                .is_some_and(|percent| percent <= LOW_BATTERY_PERCENT)
    }

    /// The hottest zone.
    pub fn hottest(&self) -> Option<Zone> {
        self.zones().max_by_key(|zone| zone.tenths_c)
    }
}

/// The critical point of a zone that names none, 95.0 °C.
pub const CRITICAL_FALLBACK_TENTHS_C: i32 = 950;

/// Writes `tenths` of a degree as `47.0`.
pub fn write_celsius(out: &mut impl Write, tenths: i32) -> core::fmt::Result {
    let sign = if tenths < 0 { "-" } else { "" };
    write!(out, "{sign}{}.{}", tenths.unsigned_abs() / 10, tenths.unsigned_abs() % 10)
}

/// The battery indicator's tooltip, which is also its accessible name (D599, 2.3).
pub fn battery_tooltip(facts: &PowerFacts, out: &mut impl Write) -> core::fmt::Result {
    let mut batteries = facts.batteries();
    let Some(first) = batteries.next() else {
        return Ok(());
    };
    let state = match (facts.is_low(), first.charge) {
        (true, _) => "low",
        (false, charge) => charge.word(),
    };
    match batteries.next() {
        None => write!(out, "Battery: {}%, {state}", first.percent),
        Some(second) => write!(
            out,
            "Batteries: {}% and {}%, {state}",
            first.percent, second.percent
        ),
    }
}
