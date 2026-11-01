//! The `Date & time` pane's `Set automatically` (D224, docs/ux/date-time-v0.3.md section 9):
//! the check box, `Set to` disabled while the network clock is in charge, and the pane's two
//! status lines, the second read from the network clock's record.

use crate::agents_pane::{show_line, shortened};
use crate::facts::Facts;
use crate::settings_window::{CAPACITY, Request, shown_status};
use core::fmt::Write;
use eggos_calendar::zone::{self, ZONES};
use eggos_calendar::{DateTime, MONTHS, SECONDS_PER_DAY};
use eggos_runtime::errno::Errno;
use eggos_net::status::{InterfaceStatus, State};
use eggos_settings_file::clock_sync::{Outcome, Record};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::Kind;

/// The check box's words, which also name it for agents.
pub const LABEL: &str = "Set automatically";
/// Line 1 with automatic on and no network adapter.
pub const NO_ADAPTER: &str = "No network adapter: the clock cannot be set from the network.";
/// Line 1 when the adapter has no address yet.
pub const WAITING: &str = "Waiting for a network address to set the clock.";
/// Line 1 with no record while connected.
pub const NOT_SET_YET: &str = "Not set from the network yet.";
/// Line 1 while a sync runs.
pub const SYNCING: &str = "Setting the clock from the network...";
/// Line 1 when the kernel refused the sync.
pub const SYNC_NOT_ALLOWED: &str = "Not allowed: this user may not set the clock.";
/// Line 1 when the setting could not be written for lack of permission.
pub const TOGGLE_NOT_ALLOWED: &str = "Not allowed: this user may not change Set automatically.";
/// Line 1 when the setting could not be written for another reason.
pub const TOGGLE_FAILED: &str = "Set automatically could not be saved.";
/// Line 2 with no record while connected.
pub const SYNC_NOW: &str = "Turn this off and on to set it now.";
/// Line 2 after a large step HTTPS could not confirm.
pub const UNCONFIRMED_REMEDY: &str = "Turn this off and use Set to, or add /etc/ssl/roots.pem.";
/// Line 2 after the time server did not answer.
pub const NO_ANSWER_REMEDY: &str = "Check the network, then turn this off and on to try again.";
/// Line 2 without a disk when no remedy claims it.
pub const NO_DISK: &str = "No disk attached: this choice lasts until power-off.";

const MS_PER_SECOND: u64 = 1000;
const MS_PER_TENTH: u64 = 100;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;

/// Whether the machine has a network adapter and whether one has an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adapter {
    /// No adapter but the loopback one.
    Absent,
    /// An adapter, none with an address.
    WithoutAddress,
    /// An adapter with an address.
    Connected,
}

impl Adapter {
    /// What the network adapter `adapter` (`None`: the machine has none) is to the network
    /// clock: an address is `net_status`'s `connected`.
    pub fn of(adapter: Option<&InterfaceStatus>) -> Self {
        match adapter.map(InterfaceStatus::state) {
            None => Self::Absent,
            Some(State::Connected) => Self::Connected,
            Some(_) => Self::WithoutAddress,
        }
    }
}

/// How far the clock moved or is off, `ms` milliseconds either way: `40 ms`, `0.4 s`, `12 s`,
/// `59.9 s`, `1 h 0 min`, `3 h 2 min`, `1 d 0 h`, every unit floored.
pub fn moved_text(ms: u64) -> Text<32> {
    let mut text = Text::new();
    let seconds = ms / MS_PER_SECOND;
    let tenths = ms % MS_PER_SECOND / MS_PER_TENTH;
    let day = SECONDS_PER_DAY as u64;
    let _ = if ms < MS_PER_TENTH {
        write!(text, "{ms} ms")
    } else if seconds < SECONDS_PER_MINUTE && tenths == 0 {
        write!(text, "{seconds} s")
    } else if seconds < SECONDS_PER_MINUTE {
        write!(text, "{seconds}.{tenths} s")
    } else if seconds < SECONDS_PER_HOUR {
        write!(text, "{} min", seconds / SECONDS_PER_MINUTE)
    } else if seconds < day {
        write!(
            text,
            "{} h {} min",
            seconds / SECONDS_PER_HOUR,
            seconds % SECONDS_PER_HOUR / SECONDS_PER_MINUTE
        )
    } else {
        write!(text, "{} d {} h", seconds / day, seconds % day / SECONDS_PER_HOUR)
    };
    text
}

fn local_seconds(at: i64, zone: usize) -> i64 {
    let offset = ZONES
        .get(zone)
        .and_then(|(_, rule)| zone::parse(rule))
        .map_or(0, |rule| rule.at(at).offset);
    at + offset
}

/// When a sync ended, `at` against `now_s`, both seen in zone `zone`: `today at 14:05`,
/// `yesterday at 14:05` or `on 27 Oct at 14:05`.
pub fn when_text(at: i64, now_s: i64, zone: usize) -> Text<32> {
    let local = local_seconds(at, zone);
    let today = local_seconds(now_s, zone).div_euclid(SECONDS_PER_DAY);
    let days = today - local.div_euclid(SECONDS_PER_DAY);
    let moment = DateTime::from_unix(local);
    let mut text = Text::new();
    let _ = match days {
        0 => text.write_str("today"),
        1 => text.write_str("yesterday"),
        _ => {
            let month = MONTHS[moment.month as usize - 1];
            write!(text, "on {} {}", moment.day, &month[..3])
        }
    };
    let _ = write!(text, " at {:02}:{:02}", moment.hour, moment.minute);
    text
}

/// What the two status lines say while the network clock is in charge (section 9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// Line 1, cut to a label's 64 bytes with `...`.
    pub line_1: Text<64>,
    /// Line 1's colour.
    pub ink: Token,
    /// Line 2, the remedy; `None` when there is none.
    pub remedy: Option<&'static str>,
}

fn status(line_1: &str, ink: Token, remedy: Option<&'static str>) -> Status {
    Status {
        line_1: shortened(line_1),
        ink,
        remedy,
    }
}

fn server_text(record: &Record) -> &str {
    core::str::from_utf8(record.server()).unwrap_or("?")
}

/// What the lines say for `record` (`None`: no sync since boot) with `adapter`, at `now_s` in
/// zone `zone`, while automatic is on and the machine has an adapter.
pub fn status_of(record: Option<&Record>, adapter: Adapter, now_s: i64, zone: usize) -> Status {
    let Some(record) = record else {
        return match adapter {
            Adapter::Connected => status(NOT_SET_YET, Token::TextDim, Some(SYNC_NOW)),
            _ => status(WAITING, Token::TextDim, None),
        };
    };
    let when = when_text(record.finished.unwrap_or(record.started), now_s, zone);
    let moved = moved_text(record.moved_ms.unsigned_abs());
    let direction = match record.moved_ms < 0 {
        true => "back",
        false => "forward",
    };
    let mut line = Text::<128>::new();
    let (ink, remedy) = match record.outcome {
        Outcome::Syncing => return status(SYNCING, Token::TextDim, None),
        Outcome::NoAddress => return status(WAITING, Token::TextDim, None),
        Outcome::NotAllowed => return status(SYNC_NOT_ALLOWED, Token::Text, None),
        Outcome::Set if record.moved_ms == 0 => {
            let _ = write!(
                line,
                "Set {} from {}; it was already right.",
                when.as_str(),
                server_text(record)
            );
            (Token::Text, None)
        }
        Outcome::Set => {
            let _ = write!(
                line,
                "Set {} from {}; moved {} {direction}.",
                when.as_str(),
                server_text(record),
                moved.as_str()
            );
            (Token::Text, None)
        }
        Outcome::Confirmed => {
            let _ = write!(
                line,
                "Set {}, HTTPS agreed; moved {} {direction}.",
                when.as_str(),
                moved.as_str()
            );
            (Token::Text, None)
        }
        Outcome::Unconfirmed => {
            let _ = write!(
                line,
                "Off by {}; not applied: HTTPS could not confirm it.",
                moved.as_str()
            );
            (Token::Text, Some(UNCONFIRMED_REMEDY))
        }
        Outcome::NoAnswer => {
            let _ = write!(line, "No answer from {}.", server_text(record));
            (Token::Text, Some(NO_ANSWER_REMEDY))
        }
    };
    status(line.as_str(), ink, remedy)
}

/// The widgets `Set automatically` works with, all in the `Date & time` pane.
#[derive(Debug, Clone, Copy)]
pub struct ClockWidgets {
    /// The `Set automatically` check box.
    pub check_box: WidgetId,
    /// The `Set to` label.
    pub set_to: WidgetId,
    /// The `Set to` field.
    pub field: WidgetId,
    /// The `Set` button.
    pub button: WidgetId,
    /// Status line 1.
    pub line_1: WidgetId,
    /// Status line 2.
    pub line_2: WidgetId,
}

/// `Set automatically`'s widgets and what the network clock last said.
#[derive(Debug)]
pub struct AutomaticClock {
    widgets: ClockWidgets,
    adapter: Adapter,
    record: Option<Record>,
    shown: Option<Status>,
    sync_started_at: Option<i64>,
}

fn set_ink(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, ink: Token) {
    let same = matches!(tree.kind(id), Some(Kind::Label { colour, .. }) if *colour == ink);
    if !same {
        tree.change(id, |kind| {
            if let Kind::Label { colour, .. } = kind {
                *colour = ink;
            }
        });
    }
}

impl AutomaticClock {
    /// `Set automatically` over `widgets`, showing `facts`.
    pub fn new(tree: &mut WidgetTree<CAPACITY>, widgets: ClockWidgets, facts: &Facts) -> Self {
        let mut clock = Self {
            widgets,
            adapter: Adapter::of(facts.network_adapter.as_ref()),
            record: facts.clock_sync,
            shown: None,
            sync_started_at: None,
        };
        clock.reset(tree, facts);
        clock
    }

    /// The widgets it works with.
    pub fn widgets(&self) -> ClockWidgets {
        self.widgets
    }

    /// Whether the network clock is in charge: automatic on and an adapter to sync through.
    pub fn in_charge(&self, facts: &Facts) -> bool {
        facts.time_automatic && self.adapter != Adapter::Absent
    }

    /// The two status lines as shown; line 2 `None` while hidden.
    pub fn lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [Option<&'a str>; 2] {
        let line_2 = self.widgets.line_2;
        [
            tree.text_of(self.widgets.line_1),
            tree.is_shown(line_2)
                .then(|| tree.text_of(line_2))
                .flatten(),
        ]
    }

    fn show_controls(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        let widgets = self.widgets;
        let in_charge = self.in_charge(facts);
        tree.set_selected(widgets.check_box, usize::from(facts.time_automatic));
        if in_charge {
            tree.set_text(widgets.field, "");
        }
        tree.set_enabled(widgets.field, !in_charge);
        let typed = tree
            .text_of(widgets.field)
            .is_some_and(|text| !text.is_empty());
        tree.set_enabled(widgets.button, !in_charge && typed);
        let ink = match in_charge {
            true => Token::TextDim,
            false => Token::Text,
        };
        set_ink(tree, widgets.set_to, ink);
    }

    fn show_line_2(&self, tree: &mut WidgetTree<CAPACITY>, remedy: Option<&str>, facts: &Facts) {
        let line_2 = self.widgets.line_2;
        let shown = remedy.or((!facts.on_disk).then_some(NO_DISK));
        if let Some(words) = shown {
            tree.set_text(line_2, words);
        }
        tree.set_shown(line_2, shown.is_some());
    }

    fn show_record(&mut self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        let shown = match self.sync_started_at {
            Some(_) => status(SYNCING, Token::TextDim, None),
            None => status_of(
                self.record.as_ref(),
                self.adapter,
                facts.now_s as i64,
                facts.time_zone,
            ),
        };
        if self.shown == Some(shown) {
            return;
        }
        let line_1 = self.widgets.line_1;
        if tree.text_of(line_1) != Some(shown.line_1.as_str()) {
            tree.set_text(line_1, shown.line_1.as_str());
        }
        set_ink(tree, line_1, shown.ink);
        self.show_line_2(tree, shown.remedy, facts);
        self.shown = Some(shown);
    }

    fn show_mode(&mut self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        self.show_controls(tree, facts);
        self.shown = None;
        if self.in_charge(facts) {
            self.show_record(tree, facts);
            return;
        }
        let (line, ink) = match facts.time_automatic {
            true => (NO_ADAPTER, Token::TextDim),
            false => (shown_status(facts), Token::TextDim),
        };
        show_line(tree, self.widgets.line_1, line, ink);
        self.show_line_2(tree, None, facts);
    }

    /// Shows the pane afresh, as when it is shown: the box at `facts`, the field empty, the
    /// lines at their "shown" state.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        tree.set_text(self.widgets.field, "");
        self.show_mode(tree, facts);
    }

    /// What a toggle of the box asks for, `None` for any other response.
    pub fn respond(&self, response: Response) -> Option<Request> {
        match response {
            Response::Toggled(id, on) if id == self.widgets.check_box => {
                Some(Request::SetTimeAutomatic {
                    on,
                    sync_now: on && self.adapter != Adapter::Absent,
                })
            }
            _ => None,
        }
    }

    /// How saving `time.automatic` as `on` went, with `facts` already holding what is in force
    /// after it; `sync_started_at`, the wall clock's seconds when this pane started a sync.
    pub fn saved(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        facts: &Facts,
        saved: Result<(), Errno>,
        sync_started_at: Option<i64>,
    ) {
        if let Err(error) = saved {
            self.show_controls(tree, facts);
            let line = match error {
                Errno::EPERM | Errno::EACCES => TOGGLE_NOT_ALLOWED,
                _ => TOGGLE_FAILED,
            };
            show_line(tree, self.widgets.line_1, line, Token::Text);
            return;
        }
        self.sync_started_at = sync_started_at.filter(|_| facts.time_automatic);
        self.show_mode(tree, facts);
    }

    /// Shows what the network clock's record (`record`) and the network (`adapter`) say now,
    /// with `facts`; `sync_running`, whether the sync this pane started still runs. Only the
    /// lines that change are retitled.
    pub fn show_sync(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        facts: &Facts,
        record: Option<Record>,
        adapter: Adapter,
        sync_running: bool,
    ) {
        if let Some(started_at) = self.sync_started_at {
            let newer = record.is_some_and(|record| record.started >= started_at);
            if newer || !sync_running {
                self.sync_started_at = None;
            }
        }
        self.record = record;
        let was_in_charge = self.in_charge(facts);
        self.adapter = adapter;
        if self.in_charge(facts) != was_in_charge {
            self.show_mode(tree, facts);
        } else if was_in_charge {
            self.show_record(tree, facts);
        }
    }
}

#[cfg(test)]
mod tests;
