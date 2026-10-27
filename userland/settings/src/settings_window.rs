//! The Settings window (docs/ux/desktop-v0.3-gui.md section 10, docs/ux/toolkit-v0.3.md
//! section 11): the sidebar with its search and its panes, and the panes: Appearance with the
//! profile picker, Display, Keyboard, Mouse, Components and About. It holds only widgets and
//! answers what the user does with what the program must do; the program talks to the system.

use crate::art;
use crate::facts::Facts;
use core::fmt::Write;
use eggos_toolkit::colour;
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};

/// The window's title.
pub const TITLE: &str = "Settings";
/// The content's size when the window opens.
pub const DEFAULT_SIZE: (u32, u32) = (640, 440);
/// The smallest content the user may resize it to: the three cards must fit the pane.
pub const MINIMUM_SIZE: (u32, u32) = (640, 360);
/// The most widgets the window holds.
pub const CAPACITY: usize = 48;

const SIDEBAR_WIDTH: i32 = 176;
const SEARCH: (i32, i32, i32, i32) = (8, 8, 160, 28);
const LIST_TOP: i32 = 44;
const SIDEBAR_ROW: i32 = 32;
const PADDING: i32 = 16;
const HEADING_TOP: i32 = 16;
const FIRST_ROW: i32 = 64;
const SETTING_ROW: i32 = 40;
const TEXT_IN_ROW: i32 = 12;
const CONTROL_IN_ROW: i32 = 6;
const APPEARANCE_ROWS: i32 = 192;
const NO_DISK_LINE: i32 = 316;
const PANE_HEIGHT: i32 = 348;
const CARD: (i32, i32) = (128, 112);
const CARD_GAP: i32 = 16;
const ABOUT_TEXT_X: i32 = 80;
const ABOUT_LINES: i32 = 104;
const ABOUT_LINE_PITCH: i32 = 20;
const EGG_HEIGHT: i32 = 48;

/// The panes, in the sidebar's order.
pub const PANES: [&str; 6] = [
    "Appearance",
    "Display",
    "Keyboard",
    "Mouse",
    "Components",
    "About",
];

const APPEARANCE: usize = 0;
const DISPLAY: usize = 1;
const KEYBOARD: usize = 2;
const MOUSE: usize = 3;
const COMPONENTS: usize = 4;
const ABOUT: usize = 5;

const EVERY_PANE_SYNONYMS: [&str; 4] =
    ["Control Panel", "System Settings", "Preferences", "options"];

const PANE_WORDS: [&[&str]; 6] = [
    &[
        "theme",
        "layout",
        "shortcuts",
        "profile",
        "Windows",
        "macOS",
        "Linux",
        "Primary modifier",
        "Window controls",
        "Scale",
        "HiDPI",
    ],
    &[
        "Resolution",
        "Scale",
        "HiDPI",
        "Pointer",
        "screen",
        "monitor",
    ],
    &["Layout", "Key repeat", "shortcuts"],
    &["Primary button", "Wheel", "trackpad", "pointer"],
    &["layers", "apps", "Remove"],
    &[
        "version", "system", "licence", "license", "memory", "CPU", "uptime",
    ],
];

/// The profile cards' labels, in the settings file's `PROFILES` order.
pub const PROFILE_LABELS: [&str; 3] = ["Windows", "macOS", "Linux"];

const SCALES: [&str; 2] = ["1x", "2x"];

/// What the program must do for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Make the profile at this index of the settings file's `PROFILES` the desktop's and
    /// save it.
    SetProfile(usize),
}

/// The primary modifier of each profile, in `PROFILES` order.
pub const PRIMARY_MODIFIERS: [&str; 3] = ["Ctrl", "Cmd (Super key)", "Ctrl"];
/// The side of each profile's window controls.
pub const CONTROL_SIDES: [&str; 3] = ["Right", "Left", "Right"];

#[derive(Debug, Clone, Copy)]
struct Widgets {
    search: WidgetId,
    list: WidgetId,
    panes: [WidgetId; 6],
    cards: WidgetId,
    modifier: WidgetId,
    controls: WidgetId,
    resolution: WidgetId,
    uptime: WidgetId,
}

/// The Settings window's widgets and what they show.
#[derive(Debug)]
pub struct SettingsWindow {
    tree: WidgetTree<CAPACITY>,
    widgets: Widgets,
    facts: Facts,
    pane: usize,
}

fn lowercase_contains(haystack: &str, needle: &str) -> bool {
    let lower = |byte: u8| byte.to_ascii_lowercase();
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    needle.is_empty()
        || haystack.windows(needle.len()).any(|window| {
            window
                .iter()
                .zip(needle)
                .all(|(&a, &b)| lower(a) == lower(b))
        })
}

/// Whether pane `pane` stays in the sidebar while the search holds `query`: the query is in
/// its name, a synonym or one of its settings' names, ignoring case.
pub fn pane_matches(pane: usize, query: &str) -> bool {
    let query = query.trim();
    lowercase_contains(PANES[pane], query)
        || EVERY_PANE_SYNONYMS
            .iter()
            .chain(PANE_WORDS[pane])
            .any(|word| lowercase_contains(word, query))
}

fn uptime_text(seconds: u64) -> Text<64> {
    let mut text = Text::new();
    let _ = write!(
        text,
        "Up {}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    text
}

struct Builder<'a> {
    tree: &'a mut WidgetTree<CAPACITY>,
}

impl Builder<'_> {
    fn add(&mut self, parent: Option<WidgetId>, place: Place, kind: Kind) -> WidgetId {
        self.tree
            .add(parent, place, kind)
            .expect("the Settings window fits its tree")
    }

    fn label(&mut self, parent: WidgetId, x: i32, y: i32, text: &str, ink: u32) -> WidgetId {
        self.add(
            Some(parent),
            Place::at(x, y, Length::Content, Length::Content),
            Kind::label(text, ink),
        )
    }

    fn value(&mut self, parent: WidgetId, y: i32, text: &str) -> WidgetId {
        self.add(
            Some(parent),
            Place::right(PADDING, y),
            Kind::label(text, colour::TEXT),
        )
    }

    fn heading(&mut self, parent: WidgetId, text: &str) -> WidgetId {
        self.add(
            Some(parent),
            Place::at(PADDING, HEADING_TOP, Length::Content, Length::Content),
            Kind::heading(text),
        )
    }

    fn pane(&mut self, parent: WidgetId) -> WidgetId {
        self.add(
            Some(parent),
            Place::at(0, 0, Length::Fill(0), Length::Px(PANE_HEIGHT)),
            Kind::Panel {
                fill: None,
                border_right: None,
            },
        )
    }

    fn row(&mut self, pane: WidgetId, row: i32, name: &str, value: &str) -> WidgetId {
        let y = FIRST_ROW + row * SETTING_ROW + TEXT_IN_ROW;
        self.label(pane, PADDING, y, name, colour::TEXT);
        self.value(pane, y, value)
    }
}

impl SettingsWindow {
    /// The window showing `facts`, the Appearance pane first, the search field focused.
    pub fn new(facts: Facts) -> Self {
        let mut tree = WidgetTree::new();
        let mut build = Builder { tree: &mut tree };
        let sidebar = build.add(
            None,
            Place::at(0, 0, Length::Px(SIDEBAR_WIDTH), Length::Fill(0)),
            Kind::Panel {
                fill: Some(colour::DOCK),
                border_right: Some(colour::MENU_BORDER),
            },
        );
        let search = build.add(
            Some(sidebar),
            Place::at(
                SEARCH.0,
                SEARCH.1,
                Length::Px(SEARCH.2),
                Length::Px(SEARCH.3),
            ),
            Kind::text_field("Search settings", Escape::Clear),
        );
        let list = build.add(
            Some(sidebar),
            Place::at(0, LIST_TOP, Length::Fill(1), Length::Fill(0)),
            Kind::list(&PANES, SIDEBAR_ROW, 4, 16),
        );
        let pane_area = build.add(
            None,
            Place::at(SIDEBAR_WIDTH, 0, Length::Fill(0), Length::Fill(0)),
            Kind::panel(colour::MENU),
        );
        let scroll = build.add(
            Some(pane_area),
            Place::fill(),
            Kind::ScrollView {
                offset: 0,
                content_height: 0,
            },
        );
        let panes = [(); 6].map(|()| build.pane(scroll));
        let profile = facts.profile.min(PROFILE_LABELS.len() - 1);

        let appearance = panes[APPEARANCE];
        build.heading(appearance, "Where are you coming from?");
        let cards = build.add(
            Some(appearance),
            Place::at(PADDING, FIRST_ROW, Length::Content, Length::Content),
            Kind::cards(
                &PROFILE_LABELS,
                profile,
                CARD,
                CARD_GAP,
                art::profile_miniature,
            ),
        );
        let row_y = |row: i32| APPEARANCE_ROWS + row * SETTING_ROW;
        build.label(
            appearance,
            PADDING,
            row_y(0) + TEXT_IN_ROW,
            "Primary modifier",
            colour::TEXT,
        );
        let modifier = build.value(
            appearance,
            row_y(0) + TEXT_IN_ROW,
            PRIMARY_MODIFIERS[profile],
        );
        build.label(
            appearance,
            PADDING,
            row_y(1) + TEXT_IN_ROW,
            "Window controls",
            colour::TEXT,
        );
        let controls = build.value(appearance, row_y(1) + TEXT_IN_ROW, CONTROL_SIDES[profile]);
        build.label(
            appearance,
            PADDING,
            row_y(2) + TEXT_IN_ROW,
            "Scale",
            colour::TEXT,
        );
        let scale_radios = Kind::radios(&SCALES, 0, true);
        let radios_width = scale_radios.content_size().0;
        let radios = build.add(
            Some(appearance),
            Place::right(PADDING, row_y(2) + CONTROL_IN_ROW),
            scale_radios,
        );
        build.add(
            Some(appearance),
            Place::right(PADDING + radios_width + PADDING, row_y(2) + TEXT_IN_ROW),
            Kind::label("2x comes in a later version", colour::TEXT_DIM),
        );
        let no_disk = build.label(
            appearance,
            PADDING,
            NO_DISK_LINE,
            "No disk attached: this choice lasts until power-off.",
            colour::TEXT_DIM,
        );

        let display = panes[DISPLAY];
        build.heading(display, PANES[DISPLAY]);
        let mut resolution_text = Text::<64>::new();
        let _ = write!(
            resolution_text,
            "{} \u{d7} {}",
            facts.resolution.0, facts.resolution.1
        );
        let resolution = build.row(display, 0, "Resolution", resolution_text.as_str());
        let mut scale_text = Text::<64>::new();
        let _ = write!(scale_text, "{}x", facts.scale);
        build.row(display, 1, "Scale", scale_text.as_str());
        build.row(display, 2, "Pointer", "Software");

        let keyboard = panes[KEYBOARD];
        build.heading(keyboard, PANES[KEYBOARD]);
        build.row(keyboard, 0, "Layout", "US");

        for later in [MOUSE, COMPONENTS] {
            build.heading(panes[later], PANES[later]);
            build.label(
                panes[later],
                PADDING,
                FIRST_ROW,
                "Coming in a later version.",
                colour::TEXT_DIM,
            );
        }

        let about = panes[ABOUT];
        build.add(
            Some(about),
            Place::at(
                PADDING,
                FIRST_ROW,
                Length::Px(EGG_HEIGHT),
                Length::Px(EGG_HEIGHT),
            ),
            Kind::Custom {
                painter: art::egg,
                data: 0,
            },
        );
        let mut title = Text::<64>::new();
        let _ = write!(title, "EggOS {}", facts.version);
        build.add(
            Some(about),
            Place::at(ABOUT_TEXT_X, FIRST_ROW, Length::Content, Length::Content),
            Kind::heading(title.as_str()),
        );
        let mut machine = Text::<64>::new();
        let _ = write!(
            machine,
            "{} \u{b7} {} CPUs \u{b7} {} MB",
            facts.architecture.as_str(),
            facts.cpus,
            facts.memory_mb
        );
        build.label(
            about,
            ABOUT_TEXT_X,
            ABOUT_LINES,
            machine.as_str(),
            colour::TEXT,
        );
        let uptime = build.label(
            about,
            ABOUT_TEXT_X,
            ABOUT_LINES + ABOUT_LINE_PITCH,
            uptime_text(facts.uptime_s).as_str(),
            colour::TEXT,
        );
        build.label(
            about,
            ABOUT_TEXT_X,
            ABOUT_LINES + 2 * ABOUT_LINE_PITCH,
            "MIT OR Apache-2.0",
            colour::TEXT,
        );

        tree.set_enabled(radios, false);
        tree.set_shown(no_disk, !facts.on_disk);
        for (index, pane) in panes.iter().enumerate() {
            tree.set_shown(*pane, index == APPEARANCE);
        }
        tree.set_focus(Some(search), 0);
        let (width, height) = DEFAULT_SIZE;
        tree.resize(width as i32, height as i32);
        Self {
            tree,
            widgets: Widgets {
                search,
                list,
                panes,
                cards,
                modifier,
                controls,
                resolution,
                uptime,
            },
            facts,
            pane: APPEARANCE,
        }
    }

    /// The window's widgets.
    pub fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        &mut self.tree
    }

    /// The window's widgets, to draw them.
    pub fn widgets(&self) -> &WidgetTree<CAPACITY> {
        &self.tree
    }

    /// The pane shown.
    pub fn pane(&self) -> usize {
        self.pane
    }

    /// Where the card of profile `index` is in the window.
    pub fn card_rect(&self, index: usize) -> Option<eggos_toolkit::geometry::Rect> {
        let kind = self.tree.kind(self.widgets.cards)?;
        let rect = self.tree.rect(self.widgets.cards)?;
        Some(eggos_toolkit::widget::card_rect(kind, rect, index))
    }

    /// The resolution shown in the Display pane.
    pub fn resolution_shown(&self) -> Option<&str> {
        self.tree.text_of(self.widgets.resolution)
    }

    fn show_pane(&mut self, pane: usize) {
        if pane == self.pane || pane >= PANES.len() {
            return;
        }
        self.tree.set_shown(self.widgets.panes[self.pane], false);
        self.tree.set_shown(self.widgets.panes[pane], true);
        self.pane = pane;
    }

    /// Shows profile `index` as the chosen one: its card selected, its modifier and control
    /// side in the rows.
    pub fn show_profile(&mut self, index: usize) {
        let index = index.min(PROFILE_LABELS.len() - 1);
        self.facts.profile = index;
        self.tree.set_selected(self.widgets.cards, index);
        self.tree
            .set_text(self.widgets.modifier, PRIMARY_MODIFIERS[index]);
        self.tree
            .set_text(self.widgets.controls, CONTROL_SIDES[index]);
    }

    /// Shows `seconds` as the uptime in the About pane.
    pub fn show_uptime(&mut self, seconds: u64) {
        self.facts.uptime_s = seconds;
        self.tree
            .set_text(self.widgets.uptime, uptime_text(seconds).as_str());
    }

    /// The facts shown.
    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    fn filter(&mut self) {
        let query = Text::<255>::from(self.tree.text_of(self.widgets.search).unwrap_or(""));
        let mut shown = 0u32;
        for pane in 0..PANES.len() {
            if pane_matches(pane, query.as_str()) {
                shown |= 1 << pane;
            }
        }
        self.tree.set_shown_items(self.widgets.list, shown);
        if shown != 0
            && let Some(selected) = self.tree.selected(self.widgets.list)
        {
            self.show_pane(selected);
        }
    }

    /// Acts on what an input did at `now_ms`: what the program must do, if anything.
    pub fn respond(&mut self, response: Response, now_ms: u32) -> Option<Request> {
        let widgets = self.widgets;
        match response {
            Response::Chosen(id, card) if id == widgets.cards => {
                self.show_profile(card);
                Some(Request::SetProfile(card))
            }
            Response::Chosen(id, pane) | Response::Opened(id, pane) if id == widgets.list => {
                self.show_pane(pane);
                None
            }
            Response::Edited(id) if id == widgets.search => {
                self.filter();
                None
            }
            Response::Unused {
                code: key::DOWN, ..
            } if self.tree.focus() == Some(widgets.search) => {
                self.tree.set_focus(Some(widgets.list), now_ms);
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_toolkit::input::{Input, Modifiers, MouseButton};

    fn click(window: &mut SettingsWindow, x: i32, y: i32) -> Option<Request> {
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

    fn type_text(window: &mut SettingsWindow, text: &str) {
        for character in text.chars() {
            let response = window.tree().handle(
                Input::Key {
                    code: 30,
                    pressed: true,
                    modifiers: Modifiers::default(),
                    character: character as u32,
                },
                0,
            );
            window.respond(response, 0);
        }
    }

    #[test]
    fn the_window_is_laid_out_as_the_design_says() {
        let window = SettingsWindow::new(Facts::example());
        let tree = window.widgets();
        assert_eq!(
            tree.rect(window.widgets.search),
            Some(eggos_toolkit::geometry::Rect::new(8, 8, 160, 28))
        );
        assert_eq!(
            window.card_rect(0),
            Some(eggos_toolkit::geometry::Rect::new(176 + 16, 64, 128, 112))
        );
        assert_eq!(
            window.card_rect(2).map(|rect| rect.x),
            Some(176 + 16 + 2 * 144)
        );
        let modifier = tree.rect(window.widgets.modifier).unwrap();
        assert_eq!((modifier.right(), modifier.y), (640 - 16, 192 + 12));
        assert_eq!(
            tree.text_of(window.widgets.modifier),
            Some("Cmd (Super key)")
        );
        assert_eq!(tree.focus(), Some(window.widgets.search));
        assert_eq!(window.resolution_shown(), Some("1280 \u{d7} 800"));
    }

    #[test]
    fn clicking_a_card_asks_for_its_profile_and_updates_the_rows() {
        let mut window = SettingsWindow::new(Facts::example());
        let card = window.card_rect(0).unwrap();
        let request = click(&mut window, card.x + 20, card.y + 20);
        assert_eq!(request, Some(Request::SetProfile(0)));
        let tree = window.widgets();
        assert_eq!(tree.text_of(window.widgets.modifier), Some("Ctrl"));
        assert_eq!(tree.text_of(window.widgets.controls), Some("Right"));
        assert_eq!(tree.selected(window.widgets.cards), Some(0));
        assert_eq!(window.facts().profile, 0);
        assert_eq!(click(&mut window, card.x + 20, card.y + 20), None);
    }

    #[test]
    fn the_sidebar_switches_panes_and_the_search_filters_them_by_synonyms() {
        let mut window = SettingsWindow::new(Facts::example());
        click(&mut window, 40, 44 + 5 * 32 + 10);
        assert_eq!(window.pane(), ABOUT);
        assert!(window.widgets().rect(window.widgets.uptime).is_some());
        click(&mut window, 40, 20);
        type_text(&mut window, "hidpi");
        assert_eq!(window.pane(), APPEARANCE);
        type_text(&mut window, "x");
        assert_eq!(window.pane(), APPEARANCE);
        let response = window.tree().handle(
            Input::Key {
                code: key::ESC,
                pressed: true,
                modifiers: Modifiers::default(),
                character: 27,
            },
            0,
        );
        window.respond(response, 0);
        type_text(&mut window, "resolution");
        assert_eq!(window.pane(), DISPLAY);
        assert!(pane_matches(ABOUT, "control panel"));
        assert!(pane_matches(KEYBOARD, "SHORTCUTS"));
        assert!(!pane_matches(MOUSE, "licence"));
    }

    #[test]
    fn the_no_disk_line_shows_only_without_a_disk() {
        let render = |on_disk: bool| {
            let window = SettingsWindow::new(Facts {
                on_disk,
                ..Facts::example()
            });
            let mut pixels = vec![0u32; 640 * 440];
            window.widgets().render(&mut pixels, 640, 440, 1);
            pixels
        };
        let with_disk = render(true);
        let without = render(false);
        let line = |pixels: &[u32]| {
            (316..332).any(|y| (192..500).any(|x| pixels[y * 640 + x] == colour::TEXT_DIM))
        };
        assert!(!line(&with_disk));
        assert!(line(&without));
    }

    #[test]
    fn the_uptime_is_hours_minutes_and_seconds() {
        assert_eq!(uptime_text(3725).as_str(), "Up 1:02:05");
        assert_eq!(uptime_text(59).as_str(), "Up 0:00:59");
    }
}
