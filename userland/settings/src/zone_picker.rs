//! The `Date & time` pane's time zone picker (D194, docs/ux/date-time-v0.3.md section 8): the
//! chosen zone's name and facts, a search field and the list of matching zones, the chosen one
//! always first. Choosing a row asks the program to save `time.zone`
//! ([`Request::SetTimeZone`]); the window shows the outcome.

use crate::agents_pane::{EMPTY_ROWS, show_line, top_row};
use crate::settings_window::{CAPACITY, Request};
use core::fmt::Write;
use eggos_calendar::format::{is_numeric_abbreviation, utc_offset};
use eggos_calendar::zone::{self, Rule, ZONES};
use eggos_toolkit::colour;
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};

const PADDING: i32 = 16;
const ZONE_LINE: i32 = 180;
const FACTS_LINE: i32 = 200;
const FIELD: (i32, i32, i32, i32) = (16, 224, 304, 28);
const COUNT_LINE: i32 = 230;
const LIST: (i32, i32, i32, i32) = (16, 260, 432, 72);
const ROW: i32 = 24;
const MARK_X: i32 = 8;
const TEXT_X: i32 = 24;
const ROW_LABEL_X: i32 = 40;
const ROW_LABEL_TOP: i32 = 264;
const ROW_LABEL_WIDTH: i32 = 400;
const VISIBLE_ROWS: usize = 3;
const SLOTS: usize = EMPTY_ROWS.len();

/// The search field's placeholder, which also names it for agents.
pub const PLACEHOLDER: &str = "City, region or abbreviation";
/// What the list says when no zone matches.
pub const NO_MATCH: &str = "No zone matches";

/// What the picker did with a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// It was not for the picker.
    NotMine,
    /// The picker acted on it, and the program must do this, if anything.
    Done(Option<Request>),
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    name: WidgetId,
    facts: WidgetId,
    field: WidgetId,
    count: WidgetId,
    list: WidgetId,
    rows: [WidgetId; VISIBLE_ROWS],
}

/// The picker's widgets and the zones its list shows.
#[derive(Debug)]
pub struct ZonePicker {
    widgets: Widgets,
    slots: [u16; SLOTS],
    slot_count: usize,
    chosen: usize,
    now_s: i64,
}

fn rule_of(index: usize) -> Rule {
    ZONES
        .get(index)
        .and_then(|(_, rule)| zone::parse(rule))
        .or_else(|| zone::parse("UTC0"))
        .expect("UTC0 is a rule")
}

fn name_of(index: usize) -> &'static str {
    ZONES.get(index).map_or("UTC", |(name, _)| *name)
}

/// The zone's facts line (section 8.1): `UTC+01:00 CET, standard time`, `UTC+05:30 IST`,
/// `UTC-03:00`, `UTC+00:00`.
pub fn facts_text(index: usize, now_s: i64) -> Text<64> {
    let rule = rule_of(index);
    let local = rule.at(now_s);
    let mut text = Text::new();
    let _ = utc_offset(&mut text, local.offset);
    let abbreviation = local.abbreviation.as_str();
    if !is_numeric_abbreviation(abbreviation) && abbreviation != "UTC" {
        let _ = write!(text, " {abbreviation}");
    }
    if rule.has_daylight() {
        let _ = text.write_str(match local.daylight {
            true => ", daylight time",
            false => ", standard time",
        });
    }
    text
}

/// A list row's text (section 8.3): `Europe/Paris · UTC+01:00 CET`.
pub fn row_text(index: usize, now_s: i64) -> Text<64> {
    let local = rule_of(index).at(now_s);
    let mut text = Text::new();
    let _ = write!(text, "{} \u{b7} ", name_of(index));
    let _ = utc_offset(&mut text, local.offset);
    let abbreviation = local.abbreviation.as_str();
    if !is_numeric_abbreviation(abbreviation) && abbreviation != "UTC" {
        let _ = write!(text, " {abbreviation}");
    }
    text
}

fn spaced_lowercase(byte: u8) -> u8 {
    match byte {
        b'_' | b'/' => b' ',
        other => other.to_ascii_lowercase(),
    }
}

/// Whether zone `index` matches `query` at `now_s` (section 8.3): its name, `_` and `/` read
/// as spaces, holds the query ignoring case, or the query is its abbreviation now.
pub fn zone_matches(index: usize, query: &str, now_s: i64) -> bool {
    let query = query.trim().as_bytes();
    if query.is_empty() {
        return false;
    }
    let name = name_of(index).as_bytes();
    let in_name = name.len() >= query.len()
        && name.windows(query.len()).any(|window| {
            window
                .iter()
                .zip(query)
                .all(|(&have, &want)| spaced_lowercase(have) == spaced_lowercase(want))
        });
    in_name
        || rule_of(index)
            .at(now_s)
            .abbreviation
            .as_str()
            .as_bytes()
            .eq_ignore_ascii_case(query)
}

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn at(place: (i32, i32, i32, i32)) -> Place {
    Place::at(place.0, place.1, Length::Px(place.2), Length::Px(place.3))
}

impl ZonePicker {
    /// Builds the picker in `pane`, the `Date & time` pane, showing zone `chosen` (an index of
    /// `ZONES`) at `now_s`.
    pub fn build(
        tree: &mut WidgetTree<CAPACITY>,
        pane: WidgetId,
        chosen: usize,
        now_s: i64,
    ) -> Self {
        add(
            tree,
            pane,
            Place::at(PADDING, ZONE_LINE, Length::Content, Length::Content),
            Kind::label("Time zone", colour::TEXT),
        );
        let name = add(
            tree,
            pane,
            Place::right(PADDING, ZONE_LINE),
            Kind::label("", colour::TEXT),
        );
        let facts = add(
            tree,
            pane,
            Place::right(PADDING, FACTS_LINE),
            Kind::label("", colour::TEXT_DIM),
        );
        let field = add(
            tree,
            pane,
            at(FIELD),
            Kind::text_field(PLACEHOLDER, Escape::Clear),
        );
        let count = add(
            tree,
            pane,
            Place::right(PADDING, COUNT_LINE),
            Kind::label("", colour::TEXT_DIM),
        );
        let list = add(
            tree,
            pane,
            at(LIST),
            Kind::list(&EMPTY_ROWS, ROW, MARK_X, TEXT_X),
        );
        tree.change(list, |kind| {
            if let Kind::List(list) = kind {
                list.empty_text = NO_MATCH;
            }
        });
        let rows = core::array::from_fn(|row| {
            add(
                tree,
                pane,
                Place::at(
                    ROW_LABEL_X,
                    ROW_LABEL_TOP + row as i32 * ROW,
                    Length::Px(ROW_LABEL_WIDTH),
                    Length::Content,
                ),
                Kind::label("", colour::TEXT),
            )
        });
        let mut picker = Self {
            widgets: Widgets {
                name,
                facts,
                field,
                count,
                list,
                rows,
            },
            slots: [0; SLOTS],
            slot_count: 0,
            chosen: chosen.min(ZONES.len() - 1),
            now_s,
        };
        picker.reset(tree, now_s);
        picker
    }

    /// Shows the pane afresh at `now_s`: the field empty, the chosen zone's row alone.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>, now_s: i64) {
        self.now_s = now_s;
        tree.set_text(self.widgets.field, "");
        self.show_chosen(tree);
        self.fill(tree, "");
    }

    /// The zone shown as chosen.
    pub fn chosen(&self) -> usize {
        self.chosen
    }

    /// Shows zone `index` as the chosen one: its name and facts, the rows staying as they are.
    pub fn choose(&mut self, tree: &mut WidgetTree<CAPACITY>, index: usize) {
        self.chosen = index.min(ZONES.len() - 1);
        self.show_chosen(tree);
    }

    /// Puts the selection back on the chosen zone's row after a failed write.
    pub fn reselect_chosen(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        if let Some(slot) = self.slots[..self.slot_count]
            .iter()
            .position(|&zone| usize::from(zone) == self.chosen)
        {
            tree.set_selected(self.widgets.list, slot);
        }
    }

    fn show_chosen(&self, tree: &mut WidgetTree<CAPACITY>) {
        tree.set_text(self.widgets.name, name_of(self.chosen));
        tree.set_text(
            self.widgets.facts,
            facts_text(self.chosen, self.now_s).as_str(),
        );
    }

    fn fill(&mut self, tree: &mut WidgetTree<CAPACITY>, query: &str) {
        let searching = !query.trim().is_empty();
        self.slots[0] = self.chosen as u16;
        let mut count = 1;
        let mut matching = 0;
        let mut matching_shown = 0;
        if searching {
            for zone in 0..ZONES.len() {
                if !zone_matches(zone, query, self.now_s) {
                    continue;
                }
                matching += 1;
                if zone == self.chosen {
                    matching_shown += 1;
                } else if count < SLOTS {
                    self.slots[count] = zone as u16;
                    count += 1;
                    matching_shown += 1;
                }
            }
            if matching == 0 {
                count = 0;
            }
        }
        self.slot_count = count;
        let shown = match count {
            SLOTS => u32::MAX,
            count => (1u32 << count) - 1,
        };
        tree.set_shown_items(self.widgets.list, shown);
        if count > 0 {
            tree.set_selected(self.widgets.list, 0);
        }
        let mut words = Text::<32>::new();
        let _ = match (matching, matching_shown == matching) {
            (1, _) => words.write_str("1 zone"),
            (matching, true) => write!(words, "{matching} zones"),
            (matching, false) => write!(words, "{count} of {matching}"),
        };
        tree.set_text(self.widgets.count, words.as_str());
        tree.set_shown(self.widgets.count, searching && matching > 0);
        self.retitle(tree);
    }

    fn retitle(&self, tree: &mut WidgetTree<CAPACITY>) {
        let top = top_row(tree, self.widgets.list);
        for (row, id) in self.widgets.rows.iter().enumerate() {
            let text = match self.slots[..self.slot_count].get(top + row) {
                Some(&zone) => row_text(usize::from(zone), self.now_s),
                None => Text::new(),
            };
            if tree.text_of(*id) != Some(text.as_str()) {
                show_line(tree, *id, text.as_str(), colour::TEXT);
            }
        }
    }

    /// The list of zones.
    pub fn list(&self) -> WidgetId {
        self.widgets.list
    }

    /// The visible rows' texts.
    pub fn row_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [&'a str; VISIBLE_ROWS] {
        self.widgets.rows.map(|id| tree.text_of(id).unwrap_or(""))
    }

    /// The name, the facts line and the count when it is shown.
    pub fn lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [Option<&'a str>; 3] {
        let count = self.widgets.count;
        [
            tree.text_of(self.widgets.name),
            tree.text_of(self.widgets.facts),
            tree.is_shown(count).then(|| tree.text_of(count)).flatten(),
        ]
    }

    /// Acts on a response at `now_ms`; `default_button` is the window's, which Enter in the
    /// search field must never reach.
    pub fn respond(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        response: Response,
        now_ms: u32,
        default_button: WidgetId,
    ) -> Reply {
        let widgets = self.widgets;
        let in_field = tree.focus() == Some(widgets.field);
        match response {
            Response::Edited(id) if id == widgets.field => {
                let query = Text::<255>::from(tree.text_of(widgets.field).unwrap_or(""));
                self.fill(tree, query.as_str());
                Reply::Done(None)
            }
            Response::Unused {
                code: key::DOWN | key::ENTER,
                ..
            } if in_field => {
                tree.set_focus(Some(widgets.list), now_ms);
                Reply::Done(None)
            }
            Response::Activated(id) if id == default_button && in_field => {
                tree.set_focus(Some(widgets.list), now_ms);
                Reply::Done(None)
            }
            Response::Chosen(id, slot) if id == widgets.list => {
                self.retitle(tree);
                let zone = self.slots[..self.slot_count]
                    .get(slot)
                    .map(|&zone| usize::from(zone));
                Reply::Done(
                    zone.filter(|&zone| zone != self.chosen)
                        .map(Request::SetTimeZone),
                )
            }
            Response::Opened(id, _) if id == widgets.list => Reply::Done(None),
            Response::Nothing => {
                self.retitle(tree);
                Reply::NotMine
            }
            _ => Reply::NotMine,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_282_709;

    fn index(name: &str) -> usize {
        crate::facts::zone_index(name).unwrap()
    }

    #[test]
    fn a_zone_s_facts_say_its_offset_abbreviation_and_season() {
        for (name, facts) in [
            ("Europe/Paris", "UTC+01:00 CET, standard time"),
            ("Australia/Sydney", "UTC+11:00 AEDT, daylight time"),
            ("Asia/Kolkata", "UTC+05:30 IST"),
            ("America/Argentina/Buenos_Aires", "UTC-03:00"),
            ("America/Nuuk", "UTC-02:00, standard time"),
            ("UTC", "UTC+00:00"),
        ] {
            assert_eq!(facts_text(index(name), NOW).as_str(), facts, "{name}");
        }
        assert_eq!(
            row_text(index("America/Argentina/Buenos_Aires"), NOW).as_str(),
            "America/Argentina/Buenos_Aires \u{b7} UTC-03:00"
        );
    }

    #[test]
    fn a_query_matches_words_of_the_name_or_the_abbreviation_now() {
        let buenos_aires = index("America/Argentina/Buenos_Aires");
        assert!(zone_matches(buenos_aires, "buenos aires", NOW));
        assert!(zone_matches(buenos_aires, " Argentina Buenos ", NOW));
        assert!(zone_matches(index("Asia/Kolkata"), "ist", NOW));
        assert!(!zone_matches(index("Europe/Paris"), "cest", NOW));
        assert!(!zone_matches(index("Europe/Paris"), "", NOW));
        let matching: Vec<&str> = (0..ZONES.len())
            .filter(|&zone| zone_matches(zone, "ist", NOW))
            .map(name_of)
            .collect();
        assert_eq!(
            matching,
            [
                "America/Boa_Vista",
                "Asia/Jerusalem",
                "Asia/Kolkata",
                "Europe/Istanbul",
                "Indian/Christmas"
            ]
        );
    }
}
