//! The Power pane (D599, docs/ux/power-v0.1.md section 5): the batteries, line power, the lid
//! and what closing it does, the temperature, the dock, and the low-battery and critical
//! temperature limits. The facts are read again once a second while it shows; the lid choice
//! is one [`Request::SetLidAction`] the program saves, then applies.

use crate::agents_pane::show_line;
use crate::facts::Facts;
use crate::input_panes::Reply;
use crate::settings_window::{CAPACITY, Request};
use alloc::string::String;
use core::fmt::Write;
use eggos_device_list::power::{
    CRITICAL_FALLBACK_TENTHS_C, LOW_BATTERY_PERCENT, MAX_DEVICES, PowerFacts, write_celsius,
};
use eggos_runtime::errno::Errno;
use eggos_settings_file::LidAction;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Horizontal, Kind, Length, Place};

const PADDING: i32 = 16;
const FIRST_ROW: i32 = 76;
const ROW: i32 = 40;
const DROPDOWN_WIDTH: i32 = 160;
const CONTROL_IN_ROW: i32 = 6;
const NARROW_ROW: i32 = 56;
const NARROW_VALUE: i32 = 22;
const TOUCH_CONTROL: i32 = 48;
/// The pane's height on the desktop with one battery row.
pub const POWER_PANE_HEIGHT: i32 = 468;
/// The battery rows the pane shows.
pub const BATTERY_ROWS: usize = MAX_DEVICES;
/// The dropdown's options, in [`LidAction`]'s order.
pub const LID_OPTIONS: [&str; 3] = ["Sleep", "Do nothing", "Power off"];
const LID_NAME: &str = "When the lid closes";
const NOT_REPORTED: &str = "Not reported";
const NO_SLEEP: &str = "Sleep is not available yet: the machine powers off instead.";
/// Below this percent, discharging, a second notice shows.
pub const VERY_LOW_BATTERY_PERCENT: u8 = 5;
/// At this percent, discharging, the power-off countdown starts.
pub const EMPTY_BATTERY_PERCENT: u8 = 3;
/// How long the critical temperature's notice lasts before the power-off, in seconds.
pub const HEAT_NOTICE_SECONDS: u32 = 30;

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn text_at(x: i32, y: i32) -> Place {
    Place::at(x, y, Length::Content, Length::Content)
}

/// The words a row of the pane shows for `facts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerWords {
    pub batteries: [Option<String>; BATTERY_ROWS],
    pub line_power: &'static str,
    pub lid: &'static str,
    pub temperature: Option<String>,
    pub dock: &'static str,
    pub low_battery: String,
    pub critical: String,
}

fn reported(fact: Option<bool>, yes: &'static str, no: &'static str) -> &'static str {
    match fact {
        Some(true) => yes,
        Some(false) => no,
        None => NOT_REPORTED,
    }
}

impl PowerWords {
    /// The words for `facts`.
    pub fn of(facts: &PowerFacts) -> Self {
        let mut batteries: [Option<String>; BATTERY_ROWS] = Default::default();
        for (slot, battery) in batteries.iter_mut().zip(facts.batteries()) {
            let mut words = String::new();
            let _ = write!(words, "{}%, {}", battery.percent, battery.charge.word());
            *slot = Some(words);
        }
        let mut zones = facts.zones().collect::<alloc::vec::Vec<_>>();
        zones.sort_by_key(|zone| zone.tenths_c);
        let temperature = (!zones.is_empty()).then(|| {
            let mut words = String::new();
            for (index, zone) in zones.iter().enumerate() {
                if index > 0 {
                    words.push_str(" and ");
                }
                let _ = write_celsius(&mut words, zone.tenths_c);
                words.push_str(" °C");
            }
            words
        });
        let mut low_battery = String::new();
        let _ = write!(
            low_battery,
            "Notices at {LOW_BATTERY_PERCENT}% and {VERY_LOW_BATTERY_PERCENT}%; \
             powers off at {EMPTY_BATTERY_PERCENT}%."
        );
        let limit = facts
            .hottest()
            .map_or(CRITICAL_FALLBACK_TENTHS_C, |zone| zone.limit_tenths_c);
        let mut critical = String::from("Powers off at ");
        let _ = write!(critical, "{}", limit / 10);
        if limit % 10 != 0 {
            let _ = write!(critical, ".{}", (limit % 10).unsigned_abs());
        }
        let _ = write!(critical, " °C after a {HEAT_NOTICE_SECONDS} s notice.");
        Self {
            batteries,
            line_power: reported(facts.line_power, "Plugged in", "On battery"),
            lid: reported(facts.lid_open, "Open", "Closed"),
            temperature,
            dock: reported(facts.docked, "Attached", "Detached"),
            low_battery,
            critical,
        }
    }

    fn battery_rows(&self) -> usize {
        self.batteries.iter().flatten().count().max(1)
    }
}

/// The Power pane's widgets.
#[derive(Debug)]
pub struct PowerPane {
    battery_titles: [WidgetId; BATTERY_ROWS],
    battery_values: [WidgetId; BATTERY_ROWS],
    titles: [WidgetId; 7],
    values: [WidgetId; 6],
    lid_choice: WidgetId,
    no_sleep: WidgetId,
    status: WidgetId,
    shown: Option<PowerWords>,
    rows: usize,
    content: Option<i32>,
}

impl PowerPane {
    /// The control that sets `key` on this pane, for a lock (D666).
    pub fn control(&self, key: crate::keys::Key) -> Option<WidgetId> {
        (key == crate::keys::Key::PowerLidAction).then_some(self.lid_choice)
    }

    /// Builds the pane's rows in `pane`, under its heading, showing `facts`.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, facts: &Facts) -> Self {
        let mut battery_titles = [pane; BATTERY_ROWS];
        let mut battery_values = [pane; BATTERY_ROWS];
        for index in 0..BATTERY_ROWS {
            let title = if index == 0 { "Battery" } else { "" };
            battery_titles[index] = add(
                tree,
                pane,
                text_at(PADDING, FIRST_ROW),
                Kind::label(title, Token::Text),
            );
            battery_values[index] = add(
                tree,
                pane,
                Place::right(PADDING, FIRST_ROW),
                Kind::label(NOT_REPORTED, Token::TextDim),
            );
        }
        let mut titles = [pane; 7];
        for (title, words) in titles.iter_mut().zip([
            "Line power",
            "Lid",
            LID_NAME,
            "Temperature",
            "Dock",
            "Low battery",
            "Critical temperature",
        ]) {
            *title = add(
                tree,
                pane,
                text_at(PADDING, FIRST_ROW),
                Kind::label(words, Token::Text),
            );
        }
        let mut values = [pane; 6];
        for value in values.iter_mut() {
            *value = add(
                tree,
                pane,
                Place::right(PADDING, FIRST_ROW),
                Kind::label(NOT_REPORTED, Token::TextDim),
            );
        }
        let lid_choice = add(
            tree,
            pane,
            Place {
                horizontal: Horizontal::Right(PADDING),
                top: FIRST_ROW,
                width: Length::Px(DROPDOWN_WIDTH),
                height: Length::Px(CONTROL_HEIGHT),
            },
            Kind::dropdown(LID_NAME, &LID_OPTIONS, facts.lid_action as usize),
        );
        let no_sleep = add(
            tree,
            pane,
            text_at(PADDING, FIRST_ROW),
            Kind::label(NO_SLEEP, Token::TextDim),
        );
        let status = add(
            tree,
            pane,
            text_at(PADDING, FIRST_ROW),
            Kind::label("", Token::TextDim),
        );
        let mut power = Self {
            battery_titles,
            battery_values,
            titles,
            values,
            lid_choice,
            no_sleep,
            status,
            shown: None,
            rows: 1,
            content: None,
        };
        power.show(tree, facts);
        tree.set_shown(status, false);
        power.reflow(tree, None);
        power
    }

    /// Shows `facts`, changing only the labels whose words changed; returns the pane's new
    /// height when the battery rows changed in number and moved the rows below them.
    pub fn show(&mut self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) -> Option<i32> {
        let words = PowerWords::of(&facts.power);
        let before = self.shown.take();
        let changed = |pick: &dyn Fn(&PowerWords) -> Option<&str>| {
            before.as_ref().map_or(true, |before| pick(before) != pick(&words))
        };
        for index in 0..BATTERY_ROWS {
            if changed(&|words| words.batteries[index].as_deref()) {
                let (text, ink) = match words.batteries[index].as_deref() {
                    Some(text) => (text, Token::Text),
                    None => (NOT_REPORTED, Token::TextDim),
                };
                show_line(tree, self.battery_values[index], text, ink);
            }
        }
        let rows: [(Option<&str>, &dyn Fn(&PowerWords) -> Option<&str>, Token); 6] = [
            (Some(words.line_power), &|words| Some(words.line_power), Token::Text),
            (Some(words.lid), &|words| Some(words.lid), Token::Text),
            (
                words.temperature.as_deref(),
                &|words| words.temperature.as_deref(),
                Token::Text,
            ),
            (Some(words.dock), &|words| Some(words.dock), Token::Text),
            (
                Some(words.low_battery.as_str()),
                &|words| Some(words.low_battery.as_str()),
                Token::TextDim,
            ),
            (
                Some(words.critical.as_str()),
                &|words| Some(words.critical.as_str()),
                Token::TextDim,
            ),
        ];
        for (value, (text, pick, ink)) in self.values.iter().zip(rows) {
            if changed(pick) {
                let (text, ink) = match text {
                    Some(text) if text != NOT_REPORTED => (text, ink),
                    _ => (NOT_REPORTED, Token::TextDim),
                };
                show_line(tree, *value, text, ink);
            }
        }
        tree.set_selected(self.lid_choice, facts.lid_action as usize);
        tree.set_enabled(self.lid_choice, facts.power.lid_open.is_some());
        tree.set_shown(
            self.no_sleep,
            facts.lid_action == LidAction::Sleep && !facts.sleep_available,
        );
        let rows = words.battery_rows();
        let moved = rows != self.rows;
        self.rows = rows;
        self.shown = Some(words);
        moved.then(|| self.reflow(tree, self.content))
    }

    /// Shows the pane afresh at `facts`, the status line hidden.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) -> Option<i32> {
        self.shown = None;
        let height = self.show(tree, facts);
        tree.set_shown(self.status, false);
        height
    }

    /// Places the rows for the desktop (`None`) or one column `content` wide on a narrow page
    /// (settings-forms-v0.5.md 2: each value under its title, the dropdown a 48 px row);
    /// returns the pane's height.
    pub fn reflow(&mut self, tree: &mut WidgetTree<CAPACITY>, content: Option<i32>) -> i32 {
        self.content = content;
        let pitch = if content.is_some() { NARROW_ROW } else { ROW };
        let mut y = FIRST_ROW;
        let mut row = |tree: &mut WidgetTree<CAPACITY>, title: WidgetId, value: WidgetId| {
            tree.set_place(title, text_at(PADDING, y));
            match content {
                None => tree.set_place(value, Place::right(PADDING, y)),
                Some(_) => tree.set_place(value, text_at(PADDING, y + NARROW_VALUE)),
            }
            y += pitch;
        };
        for index in 0..BATTERY_ROWS {
            let shown = index < self.rows;
            tree.set_shown(self.battery_titles[index], shown);
            tree.set_shown(self.battery_values[index], shown);
            if shown {
                row(tree, self.battery_titles[index], self.battery_values[index]);
            }
        }
        let [line_power, lid, lid_name, temperature, dock, low, critical] = self.titles;
        row(tree, line_power, self.values[0]);
        row(tree, lid, self.values[1]);
        tree.set_place(lid_name, text_at(PADDING, y));
        match content {
            None => tree.set_place(
                self.lid_choice,
                Place {
                    horizontal: Horizontal::Right(PADDING),
                    top: y - CONTROL_IN_ROW,
                    width: Length::Px(DROPDOWN_WIDTH),
                    height: Length::Px(CONTROL_HEIGHT),
                },
            ),
            Some(c) => tree.set_place(
                self.lid_choice,
                Place::at(
                    PADDING,
                    y + NARROW_VALUE,
                    Length::Px(c),
                    Length::Px(TOUCH_CONTROL),
                ),
            ),
        }
        y += match content {
            None => ROW,
            Some(_) => NARROW_VALUE + TOUCH_CONTROL + PADDING,
        };
        tree.set_place(self.no_sleep, text_at(PADDING, y));
        y += pitch;
        row(tree, temperature, self.values[2]);
        row(tree, dock, self.values[3]);
        row(tree, low, self.values[4]);
        row(tree, critical, self.values[5]);
        tree.set_place(self.status, text_at(PADDING, y));
        y + 2 * PADDING
    }

    /// How saving, then applying, the lid choice went; `facts` hold what is in force after.
    pub fn changed(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        facts: &Facts,
        saved: Result<(), Errno>,
        applied: Result<(), Errno>,
    ) -> Option<i32> {
        let height = self.show(tree, facts);
        let line = match (saved, applied) {
            (Err(Errno::EPERM | Errno::EACCES), _) => {
                "Not allowed: this user may not change the lid setting."
            }
            (Err(_), _) => "No disk attached: the lid setting could not be saved.",
            (Ok(()), Err(_)) => "Saved for the next boot, but the kernel did not take it.",
            (Ok(()), Ok(())) => {
                tree.set_shown(self.status, false);
                return height;
            }
        };
        show_line(tree, self.status, line, Token::Text);
        tree.set_shown(self.status, true);
        height
    }

    /// Acts on a response, `in_force` being what closing the lid does now.
    pub fn respond(&self, response: Response, in_force: LidAction) -> Reply {
        match response {
            Response::Chosen(id, choice) if id == self.lid_choice => {
                let action = match choice {
                    0 => LidAction::Sleep,
                    1 => LidAction::Nothing,
                    _ => LidAction::Off,
                };
                Reply::Done((action != in_force).then_some(Request::SetLidAction(action)))
            }
            _ => Reply::NotMine,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: &str = "lid open\nac offline\nbattery 0 87% discharging\n\
                          thermal 0 47.0 limit 95.0\nthermal 1 52.5 limit 100.0\n\
                          dock detached\n";

    #[test]
    fn a_laptop_s_facts_read_as_the_brief_words_them() {
        let words = PowerWords::of(&PowerFacts::parse(LAPTOP.as_bytes()));
        assert_eq!(words.batteries[0].as_deref(), Some("87%, discharging"));
        assert_eq!(words.batteries[1], None);
        assert_eq!(words.line_power, "On battery");
        assert_eq!(words.lid, "Open");
        assert_eq!(words.temperature.as_deref(), Some("47.0 °C and 52.5 °C"));
        assert_eq!(words.dock, "Detached");
        assert_eq!(
            words.low_battery,
            "Notices at 15% and 5%; powers off at 3%."
        );
        assert_eq!(words.critical, "Powers off at 100 °C after a 30 s notice.");
    }

    #[test]
    fn a_machine_reporting_nothing_reads_not_reported() {
        let words = PowerWords::of(&PowerFacts::default());
        assert_eq!(words.batteries[0], None);
        assert_eq!(words.line_power, NOT_REPORTED);
        assert_eq!(words.lid, NOT_REPORTED);
        assert_eq!(words.temperature, None);
        assert_eq!(words.dock, NOT_REPORTED);
        assert_eq!(words.critical, "Powers off at 95 °C after a 30 s notice.");
        assert_eq!(words.battery_rows(), 1);
    }

    #[test]
    fn two_batteries_take_two_rows() {
        let facts = PowerFacts::parse(
            b"battery 0 40% charging\nbattery 1 100% full\nac online\n",
        );
        let words = PowerWords::of(&facts);
        assert_eq!(words.battery_rows(), 2);
        assert_eq!(words.batteries[1].as_deref(), Some("100%, full"));
        assert_eq!(words.line_power, "Plugged in");
    }

    #[test]
    fn the_desktop_pane_is_468_tall_with_one_battery() {
        assert_eq!(FIRST_ROW + 9 * ROW + 2 * PADDING, POWER_PANE_HEIGHT);
    }
}
