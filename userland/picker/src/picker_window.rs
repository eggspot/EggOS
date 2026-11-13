//! The picker's window (D391, docs/ux/file-picker-v0.3.md sections 4 to 9 and 11) on the
//! toolkit's widget tree: the toolbar with back, up and the path field, the places, the
//! `Entries` table, the name row in `save` mode and the bottom band with `Hidden files`, the
//! filter, `Cancel` and the primary button. It runs a [`Picker`] and ends with the paths the
//! person chose, or none for Cancel. Choosing is the person's: the table, the name field and
//! the primary button take no agent's action, and the window takes no synthesised input.

use crate::options::Mode;
use crate::state::{Column, Notice, PLACES, Picker, Select, Step, shown, stem_end};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_app::files::{Files, Path};
use eggos_toolkit::application::{View, answer_clipboard};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::elements::{Action, Outcome, TreeWriter};
use eggos_toolkit::input::{Input, key};
use eggos_toolkit::table::{Column as TableColumn, SortOrder};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{PointerShape, Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Align, Escape, Kind, Length, Place};

/// The content size it opens at.
pub const DEFAULT_SIZE: (u32, u32) = (640, 440);
/// The smallest content it can be resized to.
pub const MINIMUM_SIZE: (u32, u32) = (560, 360);
/// `CREATE`'s flag that keeps synthesised input out (`PERSON_INPUT_ONLY`).
pub const PERSON_INPUT_ONLY: u32 = 4;

const WIDGETS: usize = 32;
const TOOLBAR: i32 = 32;
const SIDEBAR: i32 = 160;
const BAND: i32 = 52;
const SAVE_BAND: i32 = 92;
const ROW: i32 = 24;
const BUTTON: (i32, i32) = (96, 28);
const NOTICE_MS: u32 = 3000;
const NAME_COLUMN: i32 = 160;
const SIZE_COLUMN: i32 = 80;
const MODIFIED_COLUMN: i32 = 128;
const PLACE_NAMES: [&str; 7] = [
    PLACES[0].0,
    PLACES[1].0,
    PLACES[2].0,
    PLACES[3].0,
    PLACES[4].0,
    PLACES[5].0,
    PLACES[6].0,
];

static mut CELLS: Vec<[String; 3]> = Vec::new();

fn cell(_: u32, row: usize, column: usize, out: &mut Text<64>) {
    // SAFETY: the program has one thread; the cells are written only between paints.
    let cells = unsafe { &*(&raw const CELLS) };
    if let Some(text) = cells.get(row).and_then(|row| row.get(column)) {
        *out = Text::from(text.as_str());
    }
}

fn keep_cells(picker: &Picker) {
    let rows: Vec<[String; 3]> = (0..picker.rows().len())
        .map(|row| {
            [
                picker.cell(row, Column::Name),
                picker.cell(row, Column::Size),
                picker.cell(row, Column::Modified),
            ]
        })
        .collect();
    // SAFETY: as in `cell`; no table paints while they change.
    unsafe { *(&raw mut CELLS) = rows };
}

#[derive(Debug, Default, Clone, Copy)]
struct Ids {
    back: Option<WidgetId>,
    up: Option<WidgetId>,
    path: Option<WidgetId>,
    places: Option<WidgetId>,
    table: Option<WidgetId>,
    empty: Option<WidgetId>,
    name: Option<WidgetId>,
    hidden: Option<WidgetId>,
    filter: Option<WidgetId>,
    cancel: Option<WidgetId>,
    primary: Option<WidgetId>,
    notice: Option<WidgetId>,
    replace: Option<WidgetId>,
    keep: Option<WidgetId>,
}

/// What the picker ended with.
pub enum Ending {
    Chosen(Vec<Path>),
    Cancelled,
}

/// The window: its widgets, the picker it runs and the files it reads.
pub struct PickerWindow<F: Files> {
    tree: Box<WidgetTree<WIDGETS>>,
    picker: Picker,
    files: F,
    ids: Ids,
    ending: Option<Ending>,
    notice_until: Option<u32>,
    replacing: Option<Path>,
}

fn leaked(words: &[String]) -> &'static [&'static str] {
    let names: Vec<&'static str> = words
        .iter()
        .map(|word| &*Box::leak(word.clone().into_boxed_str()))
        .collect();
    Box::leak(names.into_boxed_slice())
}

impl<F: Files> PickerWindow<F> {
    /// The window for `picker`, reading `files`, laid out for `width` by `height`.
    pub fn new(picker: Picker, files: F, (width, height): (i32, i32)) -> Self {
        let mut window = Self {
            tree: Box::new(WidgetTree::new()),
            picker,
            files,
            ids: Ids::default(),
            ending: None,
            notice_until: None,
            replacing: None,
        };
        window.tree.resize(width, height);
        window.build();
        window.refresh();
        window
    }

    /// What the person chose, once the window is done.
    pub fn ending(&self) -> Option<&Ending> {
        self.ending.as_ref()
    }

    fn add(&mut self, parent: Option<WidgetId>, place: Place, kind: Kind) -> Option<WidgetId> {
        self.tree.add(parent, place, kind)
    }

    fn save(&self) -> bool {
        self.picker.options().mode == Mode::Save
    }

    fn band(&self) -> i32 {
        if self.save() { SAVE_BAND } else { BAND }
    }

    fn build(&mut self) {
        let root = self.add(None, Place::fill(), Kind::panel(Token::Menu));
        let toolbar = self.add(
            root,
            Place::at(0, 0, Length::Fill(0), Length::Px(TOOLBAR)),
            Kind::panel(Token::Bar),
        );
        self.ids.back = self.add(
            toolbar,
            Place::at(4, 2, Length::Px(28), Length::Px(28)),
            Kind::button("<", false),
        );
        self.ids.up = self.add(
            toolbar,
            Place::at(36, 2, Length::Px(28), Length::Px(28)),
            Kind::button("^", false),
        );
        self.ids.path = self.add(
            toolbar,
            Place::at(72, 2, Length::Fill(8), Length::Px(28)),
            Kind::text_field("", Escape::Revert),
        );
        let band = self.band();
        let middle = Length::Fill(band);
        let sidebar = self.add(
            root,
            Place::at(0, TOOLBAR, Length::Px(SIDEBAR), middle),
            Kind::panel(Token::Dock),
        );
        self.add(
            sidebar,
            Place::at(12, 8, Length::Px(SIDEBAR - 24), Length::Px(16)),
            Kind::label("Places", Token::TextDim),
        );
        self.ids.places = self.add(
            sidebar,
            Place::at(0, 28, Length::Fill(0), Length::Fill(0)),
            Kind::list(&PLACE_NAMES, ROW, 8, 24),
        );
        let columns = [
            TableColumn {
                title: "Name",
                width: NAME_COLUMN,
                min_width: NAME_COLUMN,
                align: Align::Start,
                sortable: true,
            },
            TableColumn {
                title: "Size",
                width: SIZE_COLUMN,
                min_width: SIZE_COLUMN,
                align: Align::End,
                sortable: true,
            },
            TableColumn {
                title: "Modified",
                width: MODIFIED_COLUMN,
                min_width: MODIFIED_COLUMN,
                align: Align::Start,
                sortable: true,
            },
        ];
        self.ids.table = self.add(
            root,
            Place::at(SIDEBAR, TOOLBAR, Length::Fill(0), middle),
            Kind::table("Entries", &columns, 0, cell, 0),
        );
        self.ids.empty = self.add(
            root,
            Place::at(SIDEBAR + 28, TOOLBAR + 32, Length::Fill(8), Length::Px(16)),
            Kind::label("", Token::TextDim),
        );
        let height = self.tree.size().1;
        let bottom = self.add(
            root,
            Place::at(0, height - band, Length::Fill(0), Length::Px(band)),
            Kind::panel(Token::Bar),
        );
        if self.save() {
            self.add(
                bottom,
                Place::at(12, 18, Length::Px(56), Length::Px(16)),
                Kind::label("Name:", Token::Text),
            );
            self.ids.name = self.add(
                bottom,
                Place::at(72, 12, Length::Fill(12), Length::Px(28)),
                Kind::text_field("", Escape::Revert),
            );
        }
        let row = band - 40;
        self.ids.hidden = self.add(
            bottom,
            Place::at(12, row, Length::Px(150), Length::Px(28)),
            Kind::checkbox("Hidden files", false),
        );
        let filters: Vec<String> = self
            .picker
            .options()
            .filters
            .iter()
            .map(|filter| filter.description.clone())
            .collect();
        if !filters.is_empty() {
            let index = self.picker.filter();
            self.ids.filter = self.add(
                bottom,
                Place::at(172, row, Length::Px(160), Length::Px(28)),
                Kind::dropdown("Filter", leaked(&filters), index),
            );
        }
        let (button_width, button_height) = BUTTON;
        self.ids.cancel = self.add(
            bottom,
            Place {
                horizontal: eggos_toolkit::widget::Horizontal::Right(212 - button_width),
                top: row,
                width: Length::Px(button_width),
                height: Length::Px(button_height),
            },
            Kind::button("Cancel", false),
        );
        let primary_words = self.picker.options().button.clone();
        self.ids.primary = self.add(
            bottom,
            Place {
                horizontal: eggos_toolkit::widget::Horizontal::Right(12),
                top: row,
                width: Length::Px(button_width),
                height: Length::Px(button_height),
            },
            Kind::button(&primary_words, true),
        );
        self.ids.notice = self.add(
            root,
            Place::at(SIDEBAR + 16, TOOLBAR + 8, Length::Fill(16), Length::Px(28)),
            Kind::label("", Token::Text),
        );
        self.ids.keep = self.add(
            root,
            Place::at(
                SIDEBAR + 16,
                TOOLBAR + 40,
                Length::Px(button_width),
                Length::Px(button_height),
            ),
            Kind::button("Cancel", false),
        );
        self.ids.replace = self.add(
            root,
            Place::at(
                SIDEBAR + 24 + button_width,
                TOOLBAR + 40,
                Length::Px(button_width),
                Length::Px(button_height),
            ),
            Kind::button("Replace", true),
        );
        for id in [self.ids.notice, self.ids.keep, self.ids.replace]
            .into_iter()
            .flatten()
        {
            self.tree.set_shown(id, false);
        }
        for id in [
            self.ids.table,
            self.ids.name,
            self.ids.primary,
            self.ids.replace,
        ]
        .into_iter()
        .flatten()
        {
            self.tree.set_person_only(id, true);
        }
        if let Some(primary) = self.ids.primary {
            self.tree.set_default_button(primary);
        }
        let initial = match self.save() {
            true => self.ids.name,
            false => self.ids.table,
        };
        if let Some(name) = self.ids.name {
            let words = shown(&self.picker.options().name);
            self.tree.set_text(name, &words);
            let stem = stem_end(words.as_bytes());
            self.tree.change(name, |kind| {
                if let Kind::TextField(field) = kind {
                    field.anchor = 0;
                    field.caret = stem;
                }
            });
        }
        self.tree.set_focus(initial, 0);
    }

    fn refresh(&mut self) {
        keep_cells(&self.picker);
        if let Some(table) = self.ids.table {
            self.tree.set_rows(table, self.picker.rows().len());
            let (column, reversed) = self.picker.sort();
            let order = if reversed {
                SortOrder::Descending
            } else {
                SortOrder::Ascending
            };
            let index = match column {
                Column::Name => 0,
                Column::Size => 1,
                Column::Modified => 2,
            };
            self.tree.set_sort(table, Some((index, order)));
            if let Some(&row) = self.picker.selected().first() {
                self.tree.set_selected(table, row);
            }
        }
        if let Some(path) = self.ids.path {
            self.tree
                .set_text(path, &shown(self.picker.folder().as_bytes()));
        }
        if let Some(empty) = self.ids.empty {
            let words = self.picker.empty_text().unwrap_or_default();
            self.tree.set_text(empty, &words);
            self.tree.set_shown(empty, !words.is_empty());
        }
        if let Some(places) = self.ids.places
            && let Some(place) = self.picker.place()
        {
            self.tree.set_selected(places, place);
        }
        for (id, enabled) in [
            (self.ids.back, self.picker.can_go_back()),
            (self.ids.up, self.picker.can_go_up()),
            (self.ids.primary, self.picker.primary_enabled()),
        ] {
            if let Some(id) = id {
                self.tree.set_enabled(id, enabled);
            }
        }
    }

    fn show_notice(&mut self, notice: &Notice, now_ms: u32) {
        if let Some(label) = self.ids.notice {
            self.tree.set_text(label, &notice.text());
            self.tree.set_shown(label, true);
            self.notice_until = Some(now_ms.wrapping_add(NOTICE_MS));
        }
    }

    fn hide_notice(&mut self) {
        if let Some(label) = self.ids.notice {
            self.tree.set_shown(label, false);
        }
        self.notice_until = None;
    }

    fn ask_replace(&mut self, path: Path) {
        let name = shown(
            path.as_bytes()
                .rsplit(|&byte| byte == b'/')
                .next()
                .unwrap_or(b""),
        );
        let folder = shown(eggos_files::names::parent(path.as_bytes()));
        if let Some(label) = self.ids.notice {
            let mut words = String::new();
            words.push_str("Replace ");
            words.push_str(&name);
            words.push_str("? A file with that name already exists in ");
            words.push_str(&folder);
            words.push('.');
            self.tree.set_text(label, &words);
            self.tree.set_shown(label, true);
        }
        for id in [self.ids.keep, self.ids.replace].into_iter().flatten() {
            self.tree.set_shown(id, true);
        }
        self.tree.set_focus(self.ids.replace, 0);
        self.notice_until = None;
        self.replacing = Some(path);
    }

    fn end_question(&mut self) {
        for id in [self.ids.keep, self.ids.replace].into_iter().flatten() {
            self.tree.set_shown(id, false);
        }
        self.hide_notice();
        self.replacing = None;
    }

    fn follow(&mut self, step: Step, now_ms: u32) {
        match step {
            Step::Nothing | Step::Entered => {}
            Step::Notice(notice) => self.show_notice(&notice, now_ms),
            Step::AskReplace(path) => self.ask_replace(path),
            Step::Chosen(paths) => self.ending = Some(Ending::Chosen(paths)),
        }
        self.refresh();
    }

    fn text_of(&self, id: Option<WidgetId>) -> Vec<u8> {
        id.and_then(|id| self.tree.text_of(id))
            .map(|text| text.as_bytes().to_vec())
            .unwrap_or_default()
    }

    fn primary(&mut self, now_ms: u32) {
        let name = self.text_of(self.ids.name);
        let step = self.picker.primary_with(&mut self.files, &name);
        self.follow(step, now_ms);
    }

    fn enter(&mut self, now_ms: u32) {
        let focus = self.tree.focus();
        if focus.is_some() && focus == self.ids.path {
            let typed = self.text_of(self.ids.path);
            let step = self.picker.type_path(&mut self.files, &typed);
            self.follow(step, now_ms);
        } else if focus.is_some() && focus == self.ids.name {
            let typed = self.text_of(self.ids.name);
            let step = self.picker.commit_name(&mut self.files, &typed);
            self.follow(step, now_ms);
        } else {
            self.primary(now_ms);
        }
    }

    fn respond(&mut self, response: Response, now_ms: u32) {
        let is = |wanted: Option<WidgetId>, id: WidgetId| wanted == Some(id);
        match response {
            Response::Activated(id) if is(self.ids.cancel, id) => {
                self.ending = Some(Ending::Cancelled)
            }
            Response::Activated(id) if is(self.ids.primary, id) => self.primary(now_ms),
            Response::Activated(id) if is(self.ids.keep, id) => self.end_question(),
            Response::Activated(id) if is(self.ids.replace, id) => {
                if let Some(path) = self.replacing.take() {
                    self.end_question();
                    self.ending = Some(Ending::Chosen(alloc::vec![path]));
                }
            }
            Response::Activated(id) if is(self.ids.back, id) => {
                let step = self.picker.back(&mut self.files);
                self.follow(step, now_ms);
            }
            Response::Activated(id) if is(self.ids.up, id) => {
                let step = self.picker.up(&mut self.files);
                self.follow(step, now_ms);
            }
            Response::Chosen(id, place) if is(self.ids.places, id) => {
                let step = self.picker.go_to_place(&mut self.files, place);
                self.follow(step, now_ms);
            }
            Response::Chosen(id, row) if is(self.ids.table, id) => {
                self.picker.select(row, Select::Only);
                if self.save()
                    && let (Some(name), Some(false)) = (
                        self.ids.name,
                        self.picker.rows().get(row).map(|row| row.dim),
                    )
                {
                    let words = shown(self.picker.name(row));
                    self.tree.set_text(name, &words);
                }
                self.refresh();
            }
            Response::Opened(id, row) if is(self.ids.table, id) => {
                let step = self.picker.activate(&mut self.files, row);
                self.follow(step, now_ms);
            }
            Response::Chosen(id, index) if is(self.ids.filter, id) => {
                self.picker.set_filter(index);
                self.refresh();
            }
            Response::Toggled(id, on) if is(self.ids.hidden, id) => {
                self.picker.set_hidden(&mut self.files, on);
                self.refresh();
            }
            Response::Sorted(id, column, order) if is(self.ids.table, id) => {
                self.picker
                    .sort_as(Column::at(column), order == SortOrder::Descending);
                self.refresh();
            }
            Response::Unused {
                code: key::ENTER | key::KEYPAD_ENTER,
                ..
            } => self.enter(now_ms),
            Response::Unused { code: key::ESC, .. } => {
                match self.replacing.is_some() || self.notice_until.is_some() {
                    true => self.end_question(),
                    false => self.ending = Some(Ending::Cancelled),
                }
            }
            Response::Unused { code: key::F5, .. } => {
                self.picker.reload(&mut self.files);
                self.refresh();
            }
            _ => {}
        }
    }
}

impl<F: Files> View for PickerWindow<F> {
    fn resize(&mut self, width: i32, height: i32) {
        self.tree.resize(width, height);
    }

    fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        if input == Input::CloseRequest {
            self.ending = Some(Ending::Cancelled);
            return false;
        }
        if matches!(
            input,
            Input::Key { pressed: true, .. } | Input::Button { pressed: true, .. }
        ) && self.notice_until.is_some()
        {
            self.hide_notice();
        }
        let response = self.tree.handle(input, time_ms);
        let response = answer_clipboard(&mut *self.tree, None, response, time_ms);
        self.respond(response, time_ms);
        self.ending.is_none()
    }

    fn take_damage(&mut self) -> Damage {
        self.tree.take_damage()
    }

    fn paint(&mut self, canvas: &mut Canvas<'_>) {
        self.tree.paint(canvas);
    }

    fn pointer_shape(&mut self) -> PointerShape {
        self.tree.pointer_shape()
    }

    fn deadline(&mut self, now_ms: u32) -> Option<u32> {
        let tree = self.tree.next_deadline(now_ms);
        match (tree, self.notice_until) {
            (Some(tree), Some(notice)) => Some(tree.min(notice)),
            (tree, notice) => tree.or(notice),
        }
    }

    fn tick(&mut self, now_ms: u32) {
        if self
            .notice_until
            .is_some_and(|until| now_ms.wrapping_sub(until) < u32::MAX / 2)
        {
            self.hide_notice();
        }
        let response = self.tree.tick(now_ms);
        self.respond(response, now_ms);
    }

    fn close_requested(&mut self, _time_ms: u32) -> bool {
        self.ending = Some(Ending::Cancelled);
        true
    }

    fn describe(&mut self, tree: &mut TreeWriter<'_>) -> bool {
        self.tree.describe(tree);
        true
    }

    fn act(&mut self, element: u32, action: Action, text: &str, time_ms: u32) -> Outcome {
        let (outcome, response) = self.tree.act(element, action, text, time_ms);
        self.respond(response, time_ms);
        outcome
    }
}
