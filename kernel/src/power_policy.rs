//! What the kernel does about the power state (D599, docs/ux/power-v0.1.md section 4): the lid
//! closing (`power.lid_action`), a thermal zone reaching its hot and critical points, and a
//! battery running low, with one power-off countdown at a time that the desktop shows and a
//! serial line tells. A thread polls the power devices every 10 s, and once a second while a
//! countdown runs. The decisions are [`Policy::observe`], pure, so host tests drive them.

use crate::devices::acpi_namespace::{self, KELVIN_OFFSET_TENTHS, PowerState};
use crate::sync::LeafSpinLock;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use core::time::Duration;
use eggos_drivers::desktop::power::PowerShown;
use eggos_hal::Platform;
use eggos_settings_file::LidAction;

/// Seconds a closed lid gives before the power-off.
pub const COUNTDOWN_LID_S: u64 = 30;
/// Seconds a critical temperature gives.
pub const COUNTDOWN_THERMAL_S: u64 = 30;
/// Seconds a nearly empty battery gives.
pub const COUNTDOWN_BATTERY_S: u64 = 60;
/// The low battery notices' and countdown's thresholds, in percent.
pub const LOW_BATTERY_PERCENT: u8 = 15;
pub const VERY_LOW_BATTERY_PERCENT: u8 = 5;
pub const CRITICAL_BATTERY_PERCENT: u8 = 3;
/// How far under a point a zone must fall to re-arm its warning or cancel its countdown, in
/// tenths of a degree.
pub const THERMAL_HYSTERESIS_TENTHS: u64 = 50;
/// How long `Power-off cancelled.` shows.
pub const CANCELLED_NOTICE_MS: u32 = 1500;
/// How long a warning notice shows.
pub const WARNING_NOTICE_MS: u32 = 3000;

const POLL: Duration = Duration::from_secs(10);
const COUNTING: Duration = Duration::from_secs(1);
const MOST_ZONES: usize = 4;
const CANCELLED: &str = "Power-off cancelled.";

/// Why a countdown runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Lid,
    Thermal { celsius: u64 },
    Battery,
}

/// What the policy asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A notice for a few seconds.
    Notice(String),
    /// A power-off countdown of so many seconds; the earlier deadline wins.
    Countdown(Reason, u64),
    /// The countdown for this reason ends.
    Cancel(Reason),
    /// Suspend to RAM now.
    Sleep,
}

/// The policy's memory between looks.
#[derive(Debug, Default)]
pub struct Policy {
    lid_open: Option<bool>,
    hot_warned: [bool; MOST_ZONES],
    critical: [bool; MOST_ZONES],
    low_battery_stage: u8,
}

impl Policy {
    /// What `state` asks for, given what the lid does (`lid_action`) and whether the machine
    /// can suspend to RAM.
    pub fn observe(&mut self, state: &PowerState, lid_action: LidAction, can_sleep: bool) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some(open) = state.lid_open {
            let closed_now = self.lid_open == Some(true) && !open;
            let opened_now = self.lid_open == Some(false) && open;
            self.lid_open = Some(open);
            match (closed_now, opened_now, lid_action) {
                (true, _, LidAction::Sleep) if can_sleep => effects.push(Effect::Sleep),
                (true, _, LidAction::Sleep | LidAction::Off) => {
                    effects.push(Effect::Countdown(Reason::Lid, COUNTDOWN_LID_S))
                }
                (_, true, _) => effects.push(Effect::Cancel(Reason::Lid)),
                _ => {}
            }
        }
        for (index, &tenths) in state.temperatures.iter().enumerate().take(MOST_ZONES) {
            let critical = state.critical.get(index).copied().unwrap_or(u64::MAX);
            let hot = state.hot.get(index).copied().unwrap_or(u64::MAX);
            let celsius = tenths.saturating_sub(KELVIN_OFFSET_TENTHS as u64) / 10;
            if tenths >= critical && !self.critical[index] {
                self.critical[index] = true;
                effects.push(Effect::Countdown(Reason::Thermal { celsius }, COUNTDOWN_THERMAL_S));
            } else if self.critical[index]
                && tenths + THERMAL_HYSTERESIS_TENTHS <= critical
            {
                self.critical[index] = false;
                effects.push(Effect::Cancel(Reason::Thermal { celsius }));
            }
            if tenths >= hot && tenths < critical && !self.hot_warned[index] {
                self.hot_warned[index] = true;
                let mut text = String::new();
                let _ = write!(text, "Temperature {celsius} °C: close to the limit.");
                effects.push(Effect::Notice(text));
            } else if self.hot_warned[index] && tenths + THERMAL_HYSTERESIS_TENTHS <= hot {
                self.hot_warned[index] = false;
            }
        }
        let charging = state.on_line_power == Some(true) || state.batteries.iter().any(|&(_, charging)| charging);
        if charging {
            if self.low_battery_stage >= 3 {
                effects.push(Effect::Cancel(Reason::Battery));
            }
            self.low_battery_stage = 0;
        } else if let Some(percent) = mean_percent(state) {
            let stage = match percent {
                ..=CRITICAL_BATTERY_PERCENT => 3,
                ..=VERY_LOW_BATTERY_PERCENT => 2,
                ..=LOW_BATTERY_PERCENT => 1,
                _ => 0,
            };
            if stage > self.low_battery_stage {
                self.low_battery_stage = stage;
                effects.push(match stage {
                    1 => Effect::Notice(String::from("Battery at 15%.")),
                    2 => Effect::Notice(String::from("Battery at 5%: plug in now.")),
                    _ => Effect::Countdown(Reason::Battery, COUNTDOWN_BATTERY_S),
                });
            }
        }
        effects
    }
}

fn mean_percent(state: &PowerState) -> Option<u8> {
    let count = state.batteries.len();
    (count > 0).then(|| {
        (state.batteries.iter().map(|&(percent, _)| u32::from(percent)).sum::<u32>() / count as u32)
            as u8
    })
}

/// The countdown line for `reason` with `seconds` left.
pub fn countdown_text(reason: Reason, seconds: u64) -> String {
    let mut text = String::new();
    let _ = match reason {
        Reason::Lid => write!(
            text,
            "Lid closed: powering off in {seconds} s. Open the lid to cancel."
        ),
        Reason::Thermal { celsius } => write!(
            text,
            "Temperature {celsius} °C: powering off in {seconds} s to protect the hardware."
        ),
        Reason::Battery => write!(
            text,
            "Battery at 3%: powering off in {seconds} s. Plug in to cancel."
        ),
    };
    text
}

static LID_ACTION: AtomicU8 = AtomicU8::new(0);
static POLICY: LeafSpinLock<Option<Policy>> = LeafSpinLock::new_leaf(None);
static COUNTDOWN: LeafSpinLock<Option<(Reason, u64)>> = LeafSpinLock::new_leaf(None);
static STARTED: AtomicBool = AtomicBool::new(false);

/// What closing the lid does from now on (`SET_LID_ACTION`, and the settings file at boot).
pub fn set_lid_action(action: LidAction) {
    LID_ACTION.store(action as u8, Ordering::Relaxed);
}

fn lid_action() -> LidAction {
    match LID_ACTION.load(Ordering::Relaxed) {
        1 => LidAction::Nothing,
        2 => LidAction::Off,
        _ => LidAction::Sleep,
    }
}

/// What the bar's battery indicator shows of `state`.
fn shown(state: &PowerState) -> Option<PowerShown> {
    if state.batteries.is_empty() {
        return None;
    }
    let mut shown = PowerShown {
        count: state.batteries.len().min(4) as u8,
        charging: state.batteries.iter().any(|&(_, charging)| charging),
        line_power: state.on_line_power == Some(true),
        ..PowerShown::default()
    };
    for (slot, &(percent, _)) in shown.percents.iter_mut().zip(&state.batteries) {
        *slot = percent;
    }
    Some(shown)
}

/// The power state changed (`acpi_namespace` read it): the bar shows it.
pub fn changed<P: Platform>(state: &PowerState) {
    let shown = shown(state);
    crate::terminal::with_windows::<P, _>(|desktop, _, damage, _| desktop.set_power(shown, damage));
}

fn notice<P: Platform>(text: &str, milliseconds: u32) {
    serial_line::<P>(text);
    crate::terminal::with_windows::<P, _>(|desktop, _, damage, _| {
        desktop.show_text_notice(text.as_bytes(), milliseconds, damage)
    });
}

fn serial_line<P: Platform>(text: &str) {
    crate::console::write::<P>(b"power: ");
    crate::console::write::<P>(text.as_bytes());
    crate::console::write::<P>(b"\n");
}

fn show_countdown<P: Platform>(text: Option<&str>) {
    crate::terminal::with_windows::<P, _>(|desktop, _, damage, _| {
        desktop.set_countdown(text.map(str::as_bytes), damage)
    });
}

fn act<P: Platform>(effects: Vec<Effect>, now_s: u64) {
    for effect in effects {
        match effect {
            Effect::Notice(text) => notice::<P>(&text, WARNING_NOTICE_MS),
            Effect::Sleep => {
                if crate::suspend::suspend_to_ram::<P>().is_err() {
                    let mut countdown = COUNTDOWN.lock();
                    *countdown = Some((Reason::Lid, now_s + COUNTDOWN_LID_S));
                }
            }
            Effect::Countdown(reason, seconds) => {
                let deadline = now_s + seconds;
                let mut countdown = COUNTDOWN.lock();
                if countdown.is_none_or(|(_, running)| deadline < running) {
                    *countdown = Some((reason, deadline));
                }
            }
            Effect::Cancel(reason) => {
                let cancelled = {
                    let mut countdown = COUNTDOWN.lock();
                    let matches = countdown.is_some_and(|(running, _)| {
                        core::mem::discriminant(&running) == core::mem::discriminant(&reason)
                    });
                    if matches {
                        *countdown = None;
                    }
                    matches
                };
                if cancelled {
                    show_countdown::<P>(None);
                    notice::<P>(CANCELLED, CANCELLED_NOTICE_MS);
                }
            }
        }
    }
}

/// Starts the thread that watches the power state (D599), once: polls every 10 s, counts a
/// countdown down each second, and powers off in order (`init` is told, as by the power
/// button) when one ends.
pub fn start<P: Platform>() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    *POLICY.lock() = Some(Policy::default());
    let _ = crate::thread::spawn::<P, _>(|| {
        let mut polled_at = 0u64;
        loop {
            let now_s = crate::boot_clock::milliseconds_since_start::<P>() / 1000;
            if now_s >= polled_at + POLL.as_secs() || polled_at == 0 {
                polled_at = now_s;
                acpi_namespace::poll_power::<P>();
                if let Some(state) = acpi_namespace::power_state() {
                    let effects = POLICY
                        .lock()
                        .as_mut()
                        .map(|policy| policy.observe(&state, lid_action(), crate::suspend::offered::<P>()))
                        .unwrap_or_default();
                    act::<P>(effects, now_s);
                }
            }
            let running = *COUNTDOWN.lock();
            match running {
                Some((_, deadline)) if now_s >= deadline => {
                    *COUNTDOWN.lock() = None;
                    show_countdown::<P>(None);
                    crate::linux::reboot::power_button::<P>();
                }
                Some((reason, deadline)) => {
                    let text = countdown_text(reason, deadline - now_s);
                    show_countdown::<P>(Some(&text));
                    if (deadline - now_s) % 10 == 0 {
                        serial_line::<P>(&text);
                    }
                }
                None => {}
            }
            let pause = match running {
                Some(_) => COUNTING,
                None => POLL,
            };
            crate::thread::sleep::<P>(pause);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(lid: bool, kelvin_tenths: u64, battery: u8, line: bool) -> PowerState {
        PowerState {
            lid_open: Some(lid),
            on_line_power: Some(line),
            batteries: alloc::vec![(battery, false)],
            temperatures: alloc::vec![kelvin_tenths],
            critical: alloc::vec![950 + 2732],
            hot: alloc::vec![850 + 2732],
            docked: None,
        }
    }

    #[test]
    fn a_closed_lid_counts_down_and_an_open_one_cancels() {
        let mut policy = Policy::default();
        assert!(policy.observe(&state(true, 3200, 80, true), LidAction::Sleep, false).is_empty());
        assert_eq!(
            policy.observe(&state(false, 3200, 80, true), LidAction::Sleep, false),
            [Effect::Countdown(Reason::Lid, COUNTDOWN_LID_S)]
        );
        assert_eq!(
            policy.observe(&state(true, 3200, 80, true), LidAction::Sleep, false),
            [Effect::Cancel(Reason::Lid)]
        );
        assert_eq!(
            policy.observe(&state(false, 3200, 80, true), LidAction::Sleep, true),
            [Effect::Sleep]
        );
        let mut nothing = Policy::default();
        nothing.observe(&state(true, 3200, 80, true), LidAction::Nothing, false);
        assert!(nothing.observe(&state(false, 3200, 80, true), LidAction::Nothing, false).is_empty());
    }

    #[test]
    fn heat_warns_once_and_counts_down_at_the_critical_point() {
        let mut policy = Policy::default();
        let hot = policy.observe(&state(true, 860 + 2732, 80, true), LidAction::Sleep, false);
        assert!(matches!(hot.as_slice(), [Effect::Notice(_)]));
        assert!(policy.observe(&state(true, 860 + 2732, 80, true), LidAction::Sleep, false).is_empty());
        let critical = policy.observe(&state(true, 960 + 2732, 80, true), LidAction::Sleep, false);
        assert!(matches!(critical.as_slice(), [Effect::Countdown(Reason::Thermal { .. }, 30)]));
        let cooled = policy.observe(&state(true, 890 + 2732, 80, true), LidAction::Sleep, false);
        assert!(matches!(cooled.as_slice(), [Effect::Cancel(Reason::Thermal { .. })]));
    }

    #[test]
    fn a_draining_battery_warns_at_15_and_5_and_counts_down_at_3() {
        let mut policy = Policy::default();
        let mut seen = Vec::new();
        for percent in [20, 15, 9, 5, 3] {
            seen.extend(policy.observe(&state(true, 3200, percent, false), LidAction::Sleep, false));
        }
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[2], Effect::Countdown(Reason::Battery, COUNTDOWN_BATTERY_S));
        assert_eq!(
            policy.observe(&state(true, 3200, 3, true), LidAction::Sleep, false),
            [Effect::Cancel(Reason::Battery)]
        );
    }

    #[test]
    fn countdown_lines_say_the_seconds_left() {
        assert_eq!(
            countdown_text(Reason::Lid, 30),
            "Lid closed: powering off in 30 s. Open the lid to cancel."
        );
        assert_eq!(
            countdown_text(Reason::Thermal { celsius: 95 }, 29),
            "Temperature 95 °C: powering off in 29 s to protect the hardware."
        );
    }
}
