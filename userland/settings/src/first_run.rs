//! First run (D224, docs/ux/first-run-accessibility-v0.3.md section 1): `settings --first-run`'s
//! window, `Welcome to EggOS`, asking where the person comes from, the keyboard layout and the
//! time zone on three pages with `Skip`, `Back` and `Next`. Every choice is saved and applied
//! the moment it is made, as the Settings panes save; ending the window saves the profile in
//! force when none was saved, so the question is asked once.

use crate::art;
use crate::facts::Facts;
use crate::settings_window::{
    CAPACITY, CONTROL_SIDES, LAYOUT_TITLES, PRIMARY_MODIFIERS, PROFILE_LABELS, Request,
    dead_keys_text, third_level_text, time_text,
};
use crate::zone_picker::{self, PickerLayout, ZonePicker};
use eggos_runtime::errno::Errno;
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Escape, Kind, Length, Place};

/// The window's title.
pub const TITLE: &str = "Welcome to EggOS";
/// The window's content size, fixed.
pub const SIZE: (u32, u32) = (640, 440);
/// The status line while every write has worked.
pub const HINT: &str = "Every choice can be changed later in Settings.";
/// The status line without a disk.
pub const NO_DISK: &str = "No disk attached: these choices last until power-off.";
/// The try field's placeholder, which also names it for agents.
pub const TRY_PLACEHOLDER: &str = "Type here to try it";

const PAGES: usize = 3;
const HEADINGS: [&str; PAGES] = ["Where are you coming from?", "Keyboard", "Time zone"];
const STEPS: [&str; PAGES] = ["Step 1 of 3", "Step 2 of 3", "Step 3 of 3"];
const PADDING: i32 = 16;
const CARDS: (i32, i32) = (112, 64);
const CARD: (i32, i32) = (128, 112);
const CARD_GAP: i32 = 16;
const BUTTON: (i32, i32) = (96, 28);
const BUTTONS_TOP: i32 = 396;
const BACK_X: i32 = 424;
const NEXT_X: i32 = 528;
const STATUS_TOP: i32 = 356;
const DROPDOWN: (i32, i32, i32) = (424, 70, 200);
const TRY_FIELD: (i32, i32, i32) = (16, 120, 304);
const NOW_LINE: i32 = 244;
const WINDOW_CHROME: (i32, i32) = (2, 30);
const ZONE_PAGE: usize = 2;

/// Where the window's outside top-left goes, relative to a work area `width` by `height` at
/// `scale`, to sit at its centre; never above or left of it.
pub fn centred_in(width: i32, height: i32, scale: u32) -> (i32, i32) {
    let scale = scale.max(1) as i32;
    let outside = (
        SIZE.0 as i32 * scale + WINDOW_CHROME.0,
        SIZE.1 as i32 * scale + WINDOW_CHROME.1,
    );
    (
        ((width - outside.0) / 2).max(0),
        ((height - outside.1) / 2).max(0),
    )
}

/// What the program must do for the first run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstRunRequest {
    /// Save this choice and apply it now, then say how it went ([`FirstRunWindow::chosen`]):
    /// a [`Request::SetProfile`], [`Request::SetKeyboardLayout`] or [`Request::SetTimeZone`].
    Choose(Request),
    /// End the window, saving the profile at this index of `PROFILES` first when it is
    /// `Some` (none was saved yet).
    Finish { save_profile: Option<usize> },
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    pages: [WidgetId; PAGES],
    heading: WidgetId,
    step: WidgetId,
    status: WidgetId,
    skip: WidgetId,
    back: WidgetId,
    next: WidgetId,
    cards: WidgetId,
    modifier: WidgetId,
    controls: WidgetId,
    layout: WidgetId,
    try_field: WidgetId,
    third_level: WidgetId,
    dead_keys: WidgetId,
    keyboard_line: WidgetId,
    now: WidgetId,
}

/// The first run's window: its widgets, the page shown and the choices in force.
pub struct FirstRunWindow {
    tree: WidgetTree<CAPACITY>,
    widgets: Widgets,
    zones: ZonePicker,
    facts: Facts,
    page: usize,
    profile_saved: bool,
}

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the first run's window fits its tree")
}

fn label(
    tree: &mut WidgetTree<CAPACITY>,
    parent: WidgetId,
    x: i32,
    y: i32,
    words: &str,
) -> WidgetId {
    add(
        tree,
        parent,
        Place::at(x, y, Length::Content, Length::Content),
        Kind::label(words, Token::Text),
    )
}

fn value(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, y: i32, words: &str) -> WidgetId {
    add(tree, parent, Place::right(PADDING, y), Kind::label(words, Token::Text))
}

fn button(tree: &mut WidgetTree<CAPACITY>, root: WidgetId, x: i32, words: &str) -> WidgetId {
    add(
        tree,
        root,
        Place::at(x, BUTTONS_TOP, Length::Px(BUTTON.0), Length::Px(BUTTON.1)),
        Kind::button(words, x == NEXT_X),
    )
}

fn keyboard_line(facts: &Facts) -> Option<&'static str> {
    match (facts.keyboard, facts.caps_lock) {
        (false, _) => Some("No keyboard attached."),
        (true, true) => Some("Caps Lock is on"),
        (true, false) => None,
    }
}

impl FirstRunWindow {
    /// The window showing `facts` on its first page; `profile_saved` says whether the settings
    /// file already holds `shell.profile`.
    pub fn new(facts: Facts, profile_saved: bool) -> Self {
        let mut tree = WidgetTree::new();
        let root = tree
            .add(None, Place::fill(), Kind::panel(Token::Menu))
            .expect("the first run's window fits its tree");
        let pages: [WidgetId; PAGES] = core::array::from_fn(|_| {
            add(
                &mut tree,
                root,
                Place::at(0, 0, Length::Fill(0), Length::Fill(0)),
                Kind::Panel {
                    fill: None,
                    border_right: None,
                },
            )
        });
        let heading = add(
            &mut tree,
            root,
            Place::at(PADDING, PADDING, Length::Content, Length::Content),
            Kind::heading(HEADINGS[0]),
        );
        let step = add(
            &mut tree,
            root,
            Place::right(PADDING, 24),
            Kind::label(STEPS[0], Token::TextDim),
        );
        let profile = facts.profile.min(PROFILE_LABELS.len() - 1);
        let cards = add(
            &mut tree,
            pages[0],
            Place::at(CARDS.0, CARDS.1, Length::Content, Length::Content),
            Kind::cards(
                &PROFILE_LABELS,
                profile,
                CARD,
                CARD_GAP,
                art::profile_miniature,
            ),
        );
        label(&mut tree, pages[0], PADDING, 204, "Primary modifier");
        let modifier = value(&mut tree, pages[0], 204, PRIMARY_MODIFIERS[profile]);
        label(&mut tree, pages[0], PADDING, 244, "Window controls");
        let controls = value(&mut tree, pages[0], 244, CONTROL_SIDES[profile]);
        add(
            &mut tree,
            pages[0],
            Place::at(PADDING, 284, Length::Content, Length::Content),
            Kind::label(
                "The desktop behind this window already shows your choice.",
                Token::TextDim,
            ),
        );
        let layout_in_force = facts.keyboard_layout.min(LAYOUT_TITLES.len() - 1);
        label(&mut tree, pages[1], PADDING, 76, "Layout");
        let layout = add(
            &mut tree,
            pages[1],
            Place::at(
                DROPDOWN.0,
                DROPDOWN.1,
                Length::Px(DROPDOWN.2),
                Length::Px(CONTROL_HEIGHT),
            ),
            Kind::dropdown("Layout", &LAYOUT_TITLES, layout_in_force),
        );
        let try_field = add(
            &mut tree,
            pages[1],
            Place::at(
                TRY_FIELD.0,
                TRY_FIELD.1,
                Length::Px(TRY_FIELD.2),
                Length::Px(CONTROL_HEIGHT),
            ),
            Kind::text_field(TRY_PLACEHOLDER, Escape::Clear),
        );
        let third_level = label(&mut tree, pages[1], PADDING, 164, "");
        let dead_keys = label(&mut tree, pages[1], PADDING, 184, "");
        let keyboard_line = label(&mut tree, pages[1], PADDING, 204, "");
        let zones = ZonePicker::build_at(
            &mut tree,
            pages[2],
            facts.time_zone,
            facts.now_s as i64,
            PickerLayout::FIRST_RUN,
        );
        label(&mut tree, pages[2], PADDING, NOW_LINE, "Now");
        let now = value(&mut tree, pages[2], NOW_LINE, "");
        let status = add(
            &mut tree,
            root,
            Place::at(PADDING, STATUS_TOP, Length::Content, Length::Content),
            Kind::label(HINT, Token::TextDim),
        );
        let skip = button(&mut tree, root, PADDING, "Skip");
        let back = button(&mut tree, root, BACK_X, "Back");
        let next = button(&mut tree, root, NEXT_X, "Next");
        tree.set_default_button(next);
        let mut window = Self {
            tree,
            widgets: Widgets {
                pages,
                heading,
                step,
                status,
                skip,
                back,
                next,
                cards,
                modifier,
                controls,
                layout,
                try_field,
                third_level,
                dead_keys,
                keyboard_line,
                now,
            },
            zones,
            facts,
            page: 0,
            profile_saved,
        };
        window.tree.resize(SIZE.0 as i32, SIZE.1 as i32);
        window.show_keyboard_lines();
        window.show_now(facts.now_s);
        window.restore_status();
        window.show_page(0, 0);
        window
    }

    /// The widget tree, for the toolkit's loop.
    pub fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        &mut self.tree
    }

    /// The widget tree, to read.
    pub fn widgets(&self) -> &WidgetTree<CAPACITY> {
        &self.tree
    }

    /// The page shown: 0, 1 or 2.
    pub fn page(&self) -> usize {
        self.page
    }

    /// Whether the page shown has the `Now` row, which wants a tick each second.
    pub fn shows_the_clock(&self) -> bool {
        self.page == ZONE_PAGE
    }

    /// The choices in force.
    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    /// The status line's text.
    pub fn status(&self) -> &str {
        self.tree.text_of(self.widgets.status).unwrap_or("")
    }

    /// The `Back` button, `Skip` and `Next`/`Done`.
    pub fn buttons(&self) -> [WidgetId; 3] {
        [self.widgets.skip, self.widgets.back, self.widgets.next]
    }

    /// The visible zone rows' texts, the zone's name, facts and the `Now` row.
    pub fn zone_lines(&self) -> ([&str; 3], [Option<&str>; 3], &str) {
        (
            self.zones.row_lines(&self.tree),
            self.zones.lines(&self.tree),
            self.tree.text_of(self.widgets.now).unwrap_or(""),
        )
    }

    /// The rectangle of card `index` of the first page, in window content coordinates.
    pub fn card_rect(&self, index: usize) -> Option<eggos_toolkit::geometry::Rect> {
        let kind = self.tree.kind(self.widgets.cards)?;
        let rect = self.tree.rect(self.widgets.cards)?;
        Some(eggos_toolkit::widget::card_rect(kind, rect, index))
    }

    fn show_page(&mut self, page: usize, now_ms: u32) {
        self.page = page;
        for (index, id) in self.widgets.pages.iter().enumerate() {
            self.tree.set_shown(*id, index == page);
        }
        self.tree.set_text(self.widgets.heading, HEADINGS[page]);
        self.tree.set_text(self.widgets.step, STEPS[page]);
        self.tree.set_enabled(self.widgets.back, page > 0);
        let next = match page {
            ZONE_PAGE => "Done",
            _ => "Next",
        };
        self.tree.set_text(self.widgets.next, next);
        self.tree.set_text(self.widgets.try_field, "");
        if page == ZONE_PAGE {
            self.zones.reset(&mut self.tree, self.facts.now_s as i64);
        }
        self.restore_status();
        let focus = match page {
            0 => self.widgets.cards,
            1 => self.widgets.layout,
            _ => self.zones.field(),
        };
        self.tree.set_focus(Some(focus), now_ms);
    }

    fn restore_status(&mut self) {
        let words = if self.facts.on_disk { HINT } else { NO_DISK };
        crate::agents_pane::show_line(&mut self.tree, self.widgets.status, words, Token::TextDim);
    }

    fn show_profile(&mut self, profile: usize) {
        self.tree.set_selected(self.widgets.cards, profile);
        self.tree
            .set_text(self.widgets.modifier, PRIMARY_MODIFIERS[profile]);
        self.tree
            .set_text(self.widgets.controls, CONTROL_SIDES[profile]);
    }

    fn show_keyboard_lines(&mut self) {
        let layout = self.facts.keyboard_layout.min(LAYOUT_TITLES.len() - 1);
        self.tree.set_selected(self.widgets.layout, layout);
        self.tree.set_text(
            self.widgets.third_level,
            third_level_text(layout, self.facts.profile),
        );
        self.tree
            .set_text(self.widgets.dead_keys, dead_keys_text(layout).as_str());
        let line = keyboard_line(&self.facts);
        self.tree
            .set_text(self.widgets.keyboard_line, line.unwrap_or(""));
        self.tree
            .set_shown(self.widgets.keyboard_line, line.is_some());
    }

    /// Shows the clock at `seconds` in the `Now` row.
    pub fn show_now(&mut self, seconds: u64) {
        self.facts.now_s = seconds;
        let text = time_text(seconds, self.facts.time_zone);
        if self.tree.text_of(self.widgets.now) != Some(text.as_str()) {
            self.tree.set_text(self.widgets.now, text.as_str());
        }
    }

    fn in_zone_field(&self) -> bool {
        self.tree.focus() == Some(self.zones.field())
    }

    fn finish(&self) -> FirstRunRequest {
        FirstRunRequest::Finish {
            save_profile: (!self.profile_saved).then_some(self.facts.profile),
        }
    }

    /// What the window asked to close with (its close control) means: the same as `Skip`.
    pub fn close_requested(&self) -> FirstRunRequest {
        self.finish()
    }

    /// Acts on a response at `now_ms`: what the program must do, if anything.
    pub fn respond(&mut self, response: Response, now_ms: u32) -> Option<FirstRunRequest> {
        let widgets = self.widgets;
        match response {
            Response::Activated(id) if id == widgets.skip => return Some(self.finish()),
            Response::Activated(id) if id == widgets.back && self.page > 0 => {
                self.show_page(self.page - 1, now_ms);
                return None;
            }
            Response::Activated(id) if id == widgets.next && self.page < PAGES - 1 => {
                self.show_page(self.page + 1, now_ms);
                return None;
            }
            Response::Activated(id) if id == widgets.next && !self.in_zone_field() => {
                return Some(self.finish());
            }
            Response::Opened(id, _) if id == self.zones.list() => return Some(self.finish()),
            Response::Chosen(id, profile) if id == widgets.cards => {
                self.show_profile(profile);
                return (profile != self.facts.profile)
                    .then_some(FirstRunRequest::Choose(Request::SetProfile(profile)));
            }
            Response::Chosen(id, layout) if id == widgets.layout => {
                return (layout != self.facts.keyboard_layout)
                    .then_some(FirstRunRequest::Choose(Request::SetKeyboardLayout(layout)));
            }
            _ => {}
        }
        if self.page != ZONE_PAGE {
            return None;
        }
        match self.zones.respond(&mut self.tree, response, now_ms, widgets.next) {
            zone_picker::Reply::Done(request) => request.map(FirstRunRequest::Choose),
            zone_picker::Reply::NotMine => None,
        }
    }

    /// How saving and applying `request` went: on success the choice is in force; on failure
    /// the control goes back and the status line says why.
    pub fn chosen(&mut self, request: Request, result: Result<(), Errno>) {
        let what = match request {
            Request::SetProfile(profile) => {
                if result.is_ok() {
                    self.facts.profile = profile;
                    self.profile_saved = true;
                    self.show_keyboard_lines();
                }
                self.show_profile(self.facts.profile);
                "profile"
            }
            Request::SetKeyboardLayout(layout) => {
                if result.is_ok() {
                    self.facts.keyboard_layout = layout;
                }
                self.show_keyboard_lines();
                "keyboard layout"
            }
            Request::SetTimeZone(zone) => {
                match result {
                    Ok(()) => {
                        self.facts.time_zone = zone;
                        self.zones.choose(&mut self.tree, zone);
                        self.show_now(self.facts.now_s);
                    }
                    Err(_) => self.zones.reselect_chosen(&mut self.tree),
                }
                "time zone"
            }
            _ => return,
        };
        let Err(error) = result else {
            self.restore_status();
            return;
        };
        let mut line = Text::<80>::new();
        let _ = match error {
            Errno::EPERM | Errno::EACCES => {
                core::fmt::Write::write_fmt(
                    &mut line,
                    format_args!("Not allowed: this user may not set the {what}."),
                )
            }
            _ => core::fmt::Write::write_fmt(
                &mut line,
                format_args!("No disk attached: the {what} could not be saved."),
            ),
        };
        let status = self.widgets.status;
        crate::agents_pane::show_line(&mut self.tree, status, line.as_str(), Token::Text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec::Vec;
    use eggos_toolkit::elements::{MAX_TREE_BYTES, Role, TreeWriter, elements, flags};
    use eggos_toolkit::input::{Input, Modifiers, MouseButton, key};

    fn click(window: &mut FirstRunWindow, x: i32, y: i32) -> Option<FirstRunRequest> {
        let mut request = None;
        for input in [
            Input::PointerMove { x, y },
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed: true,
            },
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed: false,
            },
        ] {
            let response = window.tree().handle(input, 0);
            request = request.or(window.respond(response, 0));
        }
        request
    }

    fn press(window: &mut FirstRunWindow, code: u16, character: u32) -> Option<FirstRunRequest> {
        let response = window.tree().handle(
            Input::Key {
                code,
                pressed: true,
                modifiers: Modifiers::default(),
                character,
            },
            0,
        );
        window.respond(response, 0)
    }

    fn type_text(window: &mut FirstRunWindow, text: &str) {
        for character in text.chars() {
            press(window, 30, character as u32);
        }
    }

    fn rect(window: &FirstRunWindow, id: WidgetId) -> (i32, i32, i32, i32) {
        let rect = window.widgets().rect(id).unwrap();
        (rect.x, rect.y, rect.width, rect.height)
    }

    #[test]
    fn the_first_page_asks_where_the_person_comes_from_with_the_buttons_of_the_design() {
        let window = FirstRunWindow::new(Facts::example(), false);
        assert_eq!(window.page(), 0);
        let [skip, back, next] = window.buttons();
        assert_eq!(rect(&window, skip), (16, 396, 96, 28));
        assert_eq!(rect(&window, back), (424, 396, 96, 28));
        assert_eq!(rect(&window, next), (528, 396, 96, 28));
        assert!(!window.widgets().is_enabled(back));
        assert_eq!(rect(&window, window.widgets.cards).0, 112);
        assert_eq!(window.status(), HINT);
        assert_eq!(window.widgets().focus(), Some(window.widgets.cards));
        assert_eq!(
            window.widgets().text_of(window.widgets.modifier),
            Some("Cmd (Super key)")
        );
    }

    #[test]
    fn a_card_is_saved_at_once_and_next_turns_the_page_without_a_write() {
        let mut window = FirstRunWindow::new(Facts::example(), false);
        assert_eq!(
            click(&mut window, 176, 120),
            Some(FirstRunRequest::Choose(Request::SetProfile(0)))
        );
        assert_eq!(
            window.widgets().text_of(window.widgets.modifier),
            Some("Ctrl")
        );
        window.chosen(Request::SetProfile(0), Ok(()));
        assert_eq!(window.facts().profile, 0);
        assert_eq!(click(&mut window, 576, 410), None);
        assert_eq!(window.page(), 1);
        assert_eq!(window.widgets().focus(), Some(window.widgets.layout));
        assert_eq!(rect(&window, window.widgets.layout), (424, 70, 200, 28));
        assert_eq!(rect(&window, window.widgets.try_field), (16, 120, 304, 28));
        assert!(!window.widgets().is_shown(window.widgets.keyboard_line));
        assert_eq!(click(&mut window, 472, 410), None);
        assert_eq!(window.page(), 0);
    }

    #[test]
    fn a_refused_write_puts_the_card_back_and_says_why() {
        let mut window = FirstRunWindow::new(Facts::example(), false);
        click(&mut window, 176, 120);
        window.chosen(Request::SetProfile(0), Err(Errno::EPERM));
        assert_eq!(window.widgets().selected(window.widgets.cards), Some(1));
        assert_eq!(
            window.status(),
            "Not allowed: this user may not set the profile."
        );
        window.chosen(Request::SetProfile(2), Err(Errno::ENOENT));
        assert_eq!(
            window.status(),
            "No disk attached: the profile could not be saved."
        );
        window.chosen(Request::SetProfile(2), Ok(()));
        assert_eq!(window.status(), HINT);
    }

    #[test]
    fn skip_done_and_closing_save_the_profile_in_force_only_when_none_was_saved() {
        let mut window = FirstRunWindow::new(Facts::example(), false);
        assert_eq!(
            click(&mut window, 64, 410),
            Some(FirstRunRequest::Finish {
                save_profile: Some(1)
            })
        );
        let mut saved = FirstRunWindow::new(Facts::example(), true);
        assert_eq!(
            saved.close_requested(),
            FirstRunRequest::Finish { save_profile: None }
        );
        click(&mut saved, 576, 410);
        click(&mut saved, 576, 410);
        assert_eq!(saved.page(), 2);
        assert_eq!(saved.widgets().focus(), Some(saved.zones.field()));
        assert_eq!(
            saved.widgets().text_of(saved.widgets.next),
            Some("Done")
        );
        assert_eq!(
            click(&mut saved, 576, 410),
            Some(FirstRunRequest::Finish { save_profile: None })
        );
    }

    #[test]
    fn the_third_page_shows_the_zone_and_the_clock_and_saves_a_chosen_row() {
        let mut window = FirstRunWindow::new(Facts::example(), true);
        click(&mut window, 576, 410);
        click(&mut window, 576, 410);
        let (rows, lines, now) = window.zone_lines();
        assert_eq!(rows[0], "UTC \u{b7} UTC+00:00");
        assert_eq!(lines[0], Some("UTC"));
        assert!(now.starts_with("Thursday 29 October 2026"), "{now}");
        let paris = crate::facts::zone_index("Europe/Paris").unwrap();
        window.chosen(Request::SetTimeZone(paris), Ok(()));
        assert_eq!(window.facts().time_zone, paris);
        assert_eq!(window.zone_lines().1[0], Some("Europe/Paris"));
        let now = window.zone_lines().2;
        assert!(now.ends_with("CET"), "{now}");
    }

    #[test]
    fn without_a_disk_the_status_line_says_the_choices_last_until_power_off() {
        let window = FirstRunWindow::new(
            Facts {
                on_disk: false,
                ..Facts::example()
            },
            false,
        );
        assert_eq!(window.status(), NO_DISK);
    }

    #[test]
    fn the_window_sits_at_the_work_area_s_centre() {
        assert_eq!(centred_in(1280, 776, 1), (319, 153));
        assert_eq!(centred_in(1920, 1080, 1), (639, 305));
        assert_eq!(centred_in(600, 400, 1), (0, 0));
        assert_eq!(centred_in(2560, 1552, 2), (639, 321));
    }

    #[test]
    fn arrows_move_the_ring_and_space_chooses_a_card() {
        let mut window = FirstRunWindow::new(Facts::example(), false);
        assert_eq!(press(&mut window, key::RIGHT, 0), None);
        assert_eq!(
            press(&mut window, key::SPACE, u32::from(b' ')),
            Some(FirstRunRequest::Choose(Request::SetProfile(2)))
        );
    }

    #[test]
    fn the_keyboard_page_saves_a_layout_and_enter_in_the_try_field_turns_the_page() {
        let mut window = FirstRunWindow::new(Facts::example(), true);
        click(&mut window, 576, 410);
        assert_eq!(window.page(), 1);
        assert!(window.widgets().is_enabled(window.widgets.back));
        assert_eq!(
            press(&mut window, key::DOWN, 0),
            Some(FirstRunRequest::Choose(Request::SetKeyboardLayout(1)))
        );
        window.chosen(Request::SetKeyboardLayout(1), Ok(()));
        assert_eq!(window.facts().keyboard_layout, 1);
        assert_eq!(press(&mut window, key::TAB, 0), None);
        assert_eq!(window.widgets().focus(), Some(window.widgets.try_field));
        type_text(&mut window, "x");
        assert_eq!(window.widgets().text_of(window.widgets.try_field), Some("x"));
        assert_eq!(press(&mut window, key::ENTER, 0), None);
        assert_eq!(window.page(), 2);
        assert!(window.shows_the_clock());
    }

    #[test]
    fn enter_in_the_zone_field_moves_to_the_list_and_enter_on_a_row_is_done() {
        let mut window = FirstRunWindow::new(Facts::example(), true);
        click(&mut window, 576, 410);
        click(&mut window, 576, 410);
        type_text(&mut window, "par");
        assert_eq!(
            window.zone_lines().0,
            [
                "UTC \u{b7} UTC+00:00",
                "America/Paramaribo \u{b7} UTC-03:00",
                "Europe/Paris \u{b7} UTC+01:00 CET"
            ]
        );
        assert_eq!(window.zone_lines().1[2], Some("2 zones"));
        assert_eq!(press(&mut window, key::ENTER, 0), None);
        assert_eq!(window.widgets().focus(), Some(window.zones.list()));
        let paramaribo = crate::facts::zone_index("America/Paramaribo").unwrap();
        assert_eq!(
            press(&mut window, key::DOWN, 0),
            Some(FirstRunRequest::Choose(Request::SetTimeZone(paramaribo)))
        );
        window.chosen(Request::SetTimeZone(paramaribo), Ok(()));
        assert!(window.zone_lines().2.ends_with("UTC-03:00"));
        assert_eq!(
            press(&mut window, key::ENTER, 0),
            Some(FirstRunRequest::Finish { save_profile: None })
        );
    }

    type Described = (Role, u16, String, String);

    fn described(window: &mut FirstRunWindow) -> Vec<Described> {
        let mut bytes = alloc::vec![0u8; MAX_TREE_BYTES];
        let mut writer = TreeWriter::new(&mut bytes);
        window.tree().describe(&mut writer);
        elements(writer.bytes())
            .map(|element| {
                let element = element.unwrap();
                (
                    element.role,
                    element.flags,
                    String::from_utf8(element.name.to_vec()).unwrap(),
                    String::from_utf8(element.value.to_vec()).unwrap(),
                )
            })
            .collect()
    }

    fn named(described: &[Described], role: Role, name: &str) -> Option<Described> {
        described
            .iter()
            .find(|element| element.0 == role && element.2 == name)
            .cloned()
    }

    #[test]
    fn wm_show_lists_each_page_s_controls_with_the_names_of_the_design() {
        let mut window = FirstRunWindow::new(Facts::example(), false);
        let first = described(&mut window);
        assert!(named(&first, Role::RadioGroup, "").is_some());
        let radios: Vec<_> = first
            .iter()
            .filter(|element| element.0 == Role::Radio)
            .map(|element| (element.2.as_str(), element.1 & flags::SELECTED != 0))
            .collect();
        let profile = Facts::example().profile.min(2);
        assert_eq!(
            radios,
            [
                ("Windows", profile == 0),
                ("macOS", profile == 1),
                ("Linux", profile == 2),
            ]
        );
        assert!(named(&first, Role::Button, "Skip").is_some());
        let back = named(&first, Role::Button, "Back").unwrap();
        assert_ne!(back.1 & flags::DISABLED, 0);
        assert!(named(&first, Role::Button, "Next").is_some());
        assert!(named(&first, Role::Label, HEADINGS[0]).is_some());
        assert!(named(&first, Role::Label, HINT).is_some());
        assert!(named(&first, Role::ComboBox, "Layout").is_none());
        click(&mut window, 576, 410);
        let second = described(&mut window);
        let layout = named(&second, Role::ComboBox, "Layout").unwrap();
        assert!(LAYOUT_TITLES.contains(&layout.3.as_str()));
        assert!(named(&second, Role::TextField, TRY_PLACEHOLDER).is_some());
        let back = named(&second, Role::Button, "Back").unwrap();
        assert_eq!(back.1 & flags::DISABLED, 0);
        assert!(named(&second, Role::RadioGroup, "").is_none());
        click(&mut window, 576, 410);
        let third = described(&mut window);
        assert!(named(&third, Role::TextField, zone_picker::PLACEHOLDER).is_some());
        assert!(third.iter().any(|element| element.0 == Role::List));
        assert!(named(&third, Role::Button, "Done").is_some());
        assert!(named(&third, Role::Label, "Now").is_some());
    }
}
