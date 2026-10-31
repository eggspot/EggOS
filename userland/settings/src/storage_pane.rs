//! Settings' Storage pane (D192, docs/ux/storage-v0.3.md): the disks attached, the selected
//! one's facts, and checking, repairing and formatting an unmounted EggFS disk, with the words
//! `disk` and the MCP tools use (`eggos_files::storage`). It holds only widgets and the last
//! list; the program reads and changes the disks when a [`Request`] asks it to.

use crate::settings_window::{CAPACITY, Request};
use alloc::vec::Vec;
use eggos_eggfs::{CheckMode, DiskCheck, Outcome};
use eggos_files::storage::{
    self, DEVICES, DiskRecord, NO_DISKS, NOTHING_TO_DO, StorageError, allowed, confirm_placeholder,
    confirm_text, contents_text, erases_text, finding_line, format_question, formatted_text,
    formatting_text, headline, idle_text, label_fits, more_findings_text, place_text, running_text,
    sizes_text, used_permille,
};
use eggos_toolkit::canvas::{Canvas, INSET_R6};
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::key;
use eggos_toolkit::text::{ELLIPSIS, Text};
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};

const LABEL_BYTES: usize = 64;
const PADDING: i32 = 16;
const HEADING_TOP: i32 = 16;
const DISK_LIST: (i32, i32, i32, i32) = (16, 64, 208, 96);
const DISK_ROW: i32 = 24;
const MARK_X: i32 = 8;
const TEXT_X: i32 = 24;
const FACTS_X: i32 = 240;
const FACTS_WIDTH: i32 = 208;
const CONTENTS_LINE: i32 = 64;
const SIZES_LINE: i32 = 84;
const PLACE_LINE: i32 = 104;
const READ_ONLY_LINE: i32 = 124;
const USAGE_BAR: (i32, i32, i32, i32) = (240, 148, 208, 8);
const BUTTONS_TOP: i32 = 172;
const BUTTON_SIZE: (i32, i32) = (96, 28);
const CHECK_X: i32 = 16;
const REPAIR_X: i32 = 120;
const FORMAT_X: i32 = 224;
const STATUS_LINE: i32 = 212;
const REPORT_TOP: i32 = 232;
const LINE_PITCH: i32 = 20;
const LINE_WIDTH: i32 = 432;
/// How many report lines the pane shows.
pub const REPORT_LINES: usize = 5;
const FORMAT_PANEL: (i32, i32, i32, i32) = (32, 64, 400, 204);
const PANEL_TEXT_LINES: [i32; 3] = [16, 36, 56];
const PANEL_FIELD_WIDTH: i32 = 368;
const LABEL_FIELD_TOP: i32 = 84;
const CONFIRM_FIELD_TOP: i32 = 120;
const CANCEL_X: i32 = 176;
const FORMAT_NOW_X: i32 = 280;
const PANEL_BUTTONS_TOP: i32 = 160;
const PANEL_EDGE: i32 = 2;

/// Draws the format panel: Files' dialog panel, a 2 px field-coloured edge, a 1 px border and
/// the menu colour inside, corners cut at radius 6.
pub fn format_panel(canvas: &mut Canvas<'_>, rect: Rect, _: u32) {
    canvas.rounded(rect, &INSET_R6, colour::FIELD);
    let inner = rect.inset(PANEL_EDGE);
    canvas.rounded(inner, &INSET_R6, colour::MENU);
    canvas.outline(inner, &INSET_R6, 1, colour::MENU_BORDER);
}

fn shortened(words: &str) -> Text<LABEL_BYTES> {
    let mut text = Text::new();
    if text.push_str(words) {
        return text;
    }
    let room = LABEL_BYTES - ELLIPSIS.len();
    let end = (0..=room)
        .rev()
        .find(|&at| words.is_char_boundary(at))
        .unwrap_or(0);
    let mut text = Text::new();
    text.push_str(&words[..end]);
    text.push_str(ELLIPSIS);
    text
}

fn show_line(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, words: &str, ink: u32) {
    tree.set_text(id, shortened(words).as_str());
    tree.change(id, |kind| {
        if let Kind::Label { colour, .. } = kind {
            *colour = ink;
        }
    });
}

fn trimmed_text(tree: &WidgetTree<CAPACITY>, id: WidgetId) -> &str {
    tree.text_of(id).unwrap_or("").trim()
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    list: WidgetId,
    contents: WidgetId,
    sizes: WidgetId,
    place: WidgetId,
    read_only: WidgetId,
    usage: WidgetId,
    check: WidgetId,
    repair: WidgetId,
    format: WidgetId,
    status: WidgetId,
    report: [WidgetId; REPORT_LINES],
    panel: WidgetId,
    question: WidgetId,
    erases: WidgetId,
    confirm: WidgetId,
    label_field: WidgetId,
    confirm_field: WidgetId,
    cancel: WidgetId,
    format_now: WidgetId,
}

/// The status line's report on the disk it was made for: a check's or a format's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Report {
    device: &'static str,
    repairable: bool,
}

/// What the pane did with a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// It was not for the pane.
    NotMine,
    /// The pane acted on it, and the program must do this, if anything.
    Done(Option<Request>),
}

/// The Storage pane's widgets and the disks it shows.
#[derive(Debug)]
pub struct StoragePane {
    widgets: Widgets,
    disks: Vec<DiskRecord>,
    list_error: Option<StorageError>,
    report: Option<Report>,
    busy: bool,
    panel_open: bool,
    select_first: bool,
    default_button: WidgetId,
}

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn at(x: i32, y: i32, width: i32, height: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Px(height))
}

fn line(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, x: i32, y: i32, width: i32) -> WidgetId {
    add(
        tree,
        parent,
        Place::at(x, y, Length::Px(width), Length::Content),
        Kind::label("", colour::TEXT),
    )
}

fn button(
    tree: &mut WidgetTree<CAPACITY>,
    parent: WidgetId,
    x: i32,
    y: i32,
    text: &str,
) -> WidgetId {
    add(
        tree,
        parent,
        at(x, y, BUTTON_SIZE.0, BUTTON_SIZE.1),
        Kind::button(text, false),
    )
}

impl StoragePane {
    /// Builds the pane in `pane`, a container of the window's; `default_button` is the
    /// window's default button, given back when the format panel closes.
    pub fn build(
        tree: &mut WidgetTree<CAPACITY>,
        pane: WidgetId,
        default_button: WidgetId,
    ) -> Self {
        add(
            tree,
            pane,
            Place::at(PADDING, HEADING_TOP, Length::Content, Length::Content),
            Kind::heading("Storage"),
        );
        let list = add(
            tree,
            pane,
            at(DISK_LIST.0, DISK_LIST.1, DISK_LIST.2, DISK_LIST.3),
            Kind::list(&DEVICES, DISK_ROW, MARK_X, TEXT_X),
        );
        tree.change(list, |kind| {
            if let Kind::List(list) = kind {
                list.empty_text = NO_DISKS;
            }
        });
        let contents = line(tree, pane, FACTS_X, CONTENTS_LINE, FACTS_WIDTH);
        let sizes = line(tree, pane, FACTS_X, SIZES_LINE, FACTS_WIDTH);
        let place = line(tree, pane, FACTS_X, PLACE_LINE, FACTS_WIDTH);
        let read_only = add(
            tree,
            pane,
            Place::at(FACTS_X, READ_ONLY_LINE, Length::Content, Length::Content),
            Kind::label("Read-only", colour::TEXT),
        );
        let usage = add(
            tree,
            pane,
            at(USAGE_BAR.0, USAGE_BAR.1, USAGE_BAR.2, USAGE_BAR.3),
            Kind::Progress { permille: 0 },
        );
        let check = button(tree, pane, CHECK_X, BUTTONS_TOP, "Check");
        let repair = button(tree, pane, REPAIR_X, BUTTONS_TOP, "Repair");
        let format = button(tree, pane, FORMAT_X, BUTTONS_TOP, "Format...");
        let status = line(tree, pane, PADDING, STATUS_LINE, LINE_WIDTH);
        let report = core::array::from_fn(|index| {
            let y = REPORT_TOP + index as i32 * LINE_PITCH;
            line(tree, pane, PADDING, y, LINE_WIDTH)
        });
        let panel = add(
            tree,
            pane,
            at(
                FORMAT_PANEL.0,
                FORMAT_PANEL.1,
                FORMAT_PANEL.2,
                FORMAT_PANEL.3,
            ),
            Kind::Panel {
                fill: None,
                border_right: None,
            },
        );
        add(
            tree,
            panel,
            Place::fill(),
            Kind::Custom {
                painter: format_panel,
                data: 0,
            },
        );
        let panel_width = FORMAT_PANEL.2 - 2 * PADDING;
        let question = line(tree, panel, PADDING, PANEL_TEXT_LINES[0], panel_width);
        let erases = line(tree, panel, PADDING, PANEL_TEXT_LINES[1], panel_width);
        let confirm = line(tree, panel, PADDING, PANEL_TEXT_LINES[2], panel_width);
        let label_field = add(
            tree,
            panel,
            at(PADDING, LABEL_FIELD_TOP, PANEL_FIELD_WIDTH, BUTTON_SIZE.1),
            Kind::text_field("Label (optional)", Escape::Clear),
        );
        let confirm_field = add(
            tree,
            panel,
            at(PADDING, CONFIRM_FIELD_TOP, PANEL_FIELD_WIDTH, BUTTON_SIZE.1),
            Kind::text_field(storage::CONFIRM_PLACEHOLDERS[0], Escape::Clear),
        );
        let cancel = button(tree, panel, CANCEL_X, PANEL_BUTTONS_TOP, "Cancel");
        let format_now = add(
            tree,
            panel,
            at(
                FORMAT_NOW_X,
                PANEL_BUTTONS_TOP,
                BUTTON_SIZE.0,
                BUTTON_SIZE.1,
            ),
            Kind::button("Format", true),
        );
        tree.set_shown_items(list, 0);
        tree.set_shown(panel, false);
        let mut storage = Self {
            widgets: Widgets {
                list,
                contents,
                sizes,
                place,
                read_only,
                usage,
                check,
                repair,
                format,
                status,
                report,
                panel,
                question,
                erases,
                confirm,
                label_field,
                confirm_field,
                cancel,
                format_now,
            },
            disks: Vec::new(),
            list_error: None,
            report: None,
            busy: false,
            panel_open: false,
            select_first: true,
            default_button,
        };
        storage.refresh(tree);
        storage
    }

    /// The pane is shown again: the first disk is selected once the disks are read, and no
    /// report is kept.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        self.select_first = true;
        self.report = None;
        if self.panel_open {
            self.close_panel(tree, None);
        }
        self.refresh(tree);
    }

    /// Shows the disks just read: the selection stays on the same disk when it is still
    /// attached.
    pub fn show_disks(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        listed: Result<Vec<DiskRecord>, StorageError>,
    ) {
        let (disks, error) = match listed {
            Ok(disks) => (disks, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.disks = disks;
        self.list_error = error;
        self.busy = false;
        let shown = self
            .disks
            .iter()
            .filter_map(|record| DEVICES.iter().position(|device| *device == record.device))
            .fold(0u32, |shown, index| shown | (1 << index));
        tree.set_shown_items(self.widgets.list, shown);
        if self.select_first {
            self.select_first = false;
            if let Some(first) = (0..DEVICES.len()).find(|index| shown & (1 << index) != 0) {
                tree.set_selected(self.widgets.list, first);
            }
        }
        self.refresh(tree);
    }

    /// Shows how checking or repairing `device` went.
    pub fn show_check(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        device: &'static str,
        mode: CheckMode,
        result: Result<DiskCheck, StorageError>,
    ) {
        self.busy = false;
        let widgets = self.widgets;
        for id in widgets.report {
            tree.set_shown(id, false);
        }
        let repairable = match &result {
            Ok(check) => {
                show_line(tree, widgets.status, &headline(check), colour::TEXT);
                let findings = &check.findings;
                let listed = match findings.len() > REPORT_LINES {
                    true => REPORT_LINES - 1,
                    false => findings.len(),
                };
                for (id, finding) in widgets.report.iter().zip(&findings[..listed]) {
                    show_line(tree, *id, &finding_line(finding), colour::TEXT);
                    tree.set_shown(*id, true);
                }
                if findings.len() > REPORT_LINES {
                    let last = widgets.report[REPORT_LINES - 1];
                    let more = more_findings_text(findings.len() - listed, device);
                    show_line(tree, last, &more, colour::TEXT);
                    tree.set_shown(last, true);
                }
                mode == CheckMode::ReadOnly
                    && findings
                        .iter()
                        .any(|finding| finding.outcome == Outcome::Repairable)
            }
            Err(error) => {
                show_line(tree, widgets.status, &error.sentence(), colour::TEXT);
                false
            }
        };
        self.report = Some(Report { device, repairable });
        self.refresh(tree);
    }

    /// Shows how formatting `device` with `label` went, at `now_ms`: done, the panel closes
    /// and the status line says so; refused, the panel stays with the reason.
    pub fn show_formatted(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        device: &'static str,
        label: &str,
        result: Result<(), StorageError>,
        now_ms: u32,
    ) {
        self.busy = false;
        let widgets = self.widgets;
        match result {
            Ok(()) => {
                self.close_panel(tree, Some(now_ms));
                for id in widgets.report {
                    tree.set_shown(id, false);
                }
                show_line(
                    tree,
                    widgets.status,
                    &formatted_text(device, label),
                    colour::TEXT,
                );
                self.report = Some(Report {
                    device,
                    repairable: false,
                });
            }
            Err(error) => {
                for id in [widgets.label_field, widgets.confirm_field, widgets.cancel] {
                    tree.set_enabled(id, true);
                }
                self.update_format_now(tree);
                show_line(tree, widgets.confirm, &error.sentence(), colour::TEXT);
            }
        }
        self.refresh(tree);
    }

    /// The label typed in the format panel, trimmed.
    pub fn format_label<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> &'a str {
        trimmed_text(tree, self.widgets.label_field)
    }

    /// The disk selected, if any is attached.
    pub fn selected<'a>(&'a self, tree: &WidgetTree<CAPACITY>) -> Option<&'a DiskRecord> {
        let device = DEVICES.get(tree.selected(self.widgets.list)?)?;
        self.disks.iter().find(|record| record.device == *device)
    }

    /// Whether the format panel shows.
    pub fn panel_open(&self) -> bool {
        self.panel_open
    }

    fn refresh(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let record = self.selected(tree).cloned();
        let facts = [
            widgets.contents,
            widgets.sizes,
            widgets.place,
            widgets.check,
            widgets.repair,
            widgets.format,
        ];
        for id in facts {
            tree.set_shown(id, record.is_some());
        }
        let Some(record) = record else {
            tree.set_shown(widgets.read_only, false);
            tree.set_shown(widgets.usage, false);
            for id in widgets.report {
                tree.set_shown(id, false);
            }
            self.report = None;
            match &self.list_error {
                Some(error) => show_line(tree, widgets.status, &error.sentence(), colour::TEXT),
                None => show_line(tree, widgets.status, NOTHING_TO_DO, colour::TEXT_DIM),
            }
            return;
        };
        show_line(
            tree,
            widgets.contents,
            &contents_text(&record),
            colour::TEXT,
        );
        show_line(tree, widgets.sizes, &sizes_text(&record), colour::TEXT);
        show_line(tree, widgets.place, &place_text(&record), colour::TEXT);
        tree.set_shown(widgets.read_only, record.read_only);
        match used_permille(&record) {
            Some(permille) => {
                tree.set_progress(widgets.usage, permille);
                tree.set_shown(widgets.usage, true);
            }
            None => tree.set_shown(widgets.usage, false),
        }
        let allowed = allowed(&record);
        let report = self.report.filter(|report| report.device == record.device);
        let free = !self.busy && !self.panel_open;
        tree.set_enabled(widgets.list, free);
        tree.set_enabled(widgets.check, free && allowed.check);
        tree.set_enabled(
            widgets.repair,
            free && allowed.repair && report.is_some_and(|report| report.repairable),
        );
        tree.set_enabled(widgets.format, free && allowed.format);
        if report.is_none() && !self.busy {
            self.report = None;
            for id in widgets.report {
                tree.set_shown(id, false);
            }
            show_line(tree, widgets.status, &idle_text(&record), colour::TEXT_DIM);
        }
    }

    fn run(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        record: &DiskRecord,
        mode: CheckMode,
    ) -> Request {
        self.busy = true;
        self.report = None;
        let widgets = self.widgets;
        for id in widgets.report {
            tree.set_shown(id, false);
        }
        show_line(
            tree,
            widgets.status,
            &running_text(record.device, mode),
            colour::TEXT,
        );
        self.refresh(tree);
        match mode {
            CheckMode::ReadOnly => Request::CheckDisk(record.device),
            CheckMode::Repair => Request::RepairDisk(record.device),
        }
    }

    fn open_panel(&mut self, tree: &mut WidgetTree<CAPACITY>, record: &DiskRecord, now_ms: u32) {
        let widgets = self.widgets;
        show_line(
            tree,
            widgets.question,
            &format_question(record.device),
            colour::TEXT,
        );
        show_line(tree, widgets.erases, &erases_text(record), colour::TEXT_DIM);
        show_line(
            tree,
            widgets.confirm,
            &confirm_text(record),
            colour::TEXT_DIM,
        );
        let placeholder = confirm_placeholder(record.device);
        tree.change(widgets.confirm_field, |kind| {
            if let Kind::TextField(field) = kind {
                field.placeholder = placeholder;
            }
        });
        for id in [widgets.label_field, widgets.confirm_field] {
            tree.set_text(id, "");
            tree.set_enabled(id, true);
        }
        tree.set_enabled(widgets.cancel, true);
        tree.set_enabled(widgets.format_now, false);
        tree.set_shown(widgets.panel, true);
        tree.set_default_button(widgets.format_now);
        self.panel_open = true;
        self.refresh(tree);
        tree.set_focus(Some(widgets.label_field), now_ms);
    }

    fn close_panel(&mut self, tree: &mut WidgetTree<CAPACITY>, focus_at_ms: Option<u32>) {
        let widgets = self.widgets;
        tree.set_shown(widgets.panel, false);
        tree.set_default_button(self.default_button);
        self.panel_open = false;
        self.refresh(tree);
        if let Some(now_ms) = focus_at_ms {
            tree.set_focus(Some(widgets.format), now_ms);
        }
    }

    fn update_format_now(&self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let Some(record) = self.selected(tree) else {
            return;
        };
        let name = record.short_name();
        let fits = label_fits(trimmed_text(tree, widgets.label_field));
        let typed = trimmed_text(tree, widgets.confirm_field) == name;
        let line = match fits {
            true => (confirm_text(record), colour::TEXT_DIM),
            false => (StorageError::LabelTooLong.sentence(), colour::TEXT),
        };
        tree.set_enabled(widgets.format_now, fits && typed);
        show_line(tree, widgets.confirm, &line.0, line.1);
    }

    fn format_now(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        let widgets = self.widgets;
        let device = self.selected(tree)?.device;
        if !tree.is_shown(widgets.format_now) || self.busy {
            return None;
        }
        self.busy = true;
        for id in [
            widgets.label_field,
            widgets.confirm_field,
            widgets.cancel,
            widgets.format_now,
        ] {
            tree.set_enabled(id, false);
        }
        show_line(
            tree,
            widgets.confirm,
            &formatting_text(device),
            colour::TEXT,
        );
        Some(Request::FormatDisk(device))
    }

    /// Acts on what an input did at `now_ms` when it was for the pane.
    pub fn respond(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        response: Response,
        now_ms: u32,
    ) -> Reply {
        let widgets = self.widgets;
        let request = match response {
            Response::Chosen(id, _) | Response::Opened(id, _) if id == widgets.list => {
                if let Response::Chosen(..) = response {
                    self.report = None;
                    self.refresh(tree);
                }
                None
            }
            Response::Activated(id) if id == widgets.check || id == widgets.repair => {
                let mode = match id == widgets.check {
                    true => CheckMode::ReadOnly,
                    false => CheckMode::Repair,
                };
                let record = self.selected(tree).cloned();
                let repairable = self.report.is_some_and(|report| report.repairable);
                let may = |record: &DiskRecord| {
                    let permitted = allowed(record);
                    match mode {
                        CheckMode::ReadOnly => permitted.check,
                        CheckMode::Repair => permitted.repair && repairable,
                    }
                };
                match record {
                    Some(record) if !self.busy && may(&record) => {
                        Some(self.run(tree, &record, mode))
                    }
                    _ => None,
                }
            }
            Response::Activated(id) if id == widgets.format => {
                if let Some(record) = self.selected(tree).cloned()
                    && allowed(&record).format
                    && !self.busy
                {
                    self.open_panel(tree, &record, now_ms);
                }
                None
            }
            Response::Edited(id) if id == widgets.label_field || id == widgets.confirm_field => {
                self.update_format_now(tree);
                None
            }
            Response::Activated(id) if id == widgets.cancel => {
                if !self.busy {
                    self.close_panel(tree, Some(now_ms));
                }
                None
            }
            Response::Activated(id) if id == widgets.format_now => self.format_now(tree),
            Response::Unused { code: key::ESC, .. } if self.panel_open && !self.busy => {
                self.close_panel(tree, Some(now_ms));
                None
            }
            _ => return Reply::NotMine,
        };
        Reply::Done(request)
    }

    /// The facts shown beside the list: contents, sizes, place, and `Read-only` when shown.
    pub fn facts<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [Option<&'a str>; 4] {
        let widgets = self.widgets;
        [
            widgets.contents,
            widgets.sizes,
            widgets.place,
            widgets.read_only,
        ]
        .map(|id| tree.is_shown(id).then(|| tree.text_of(id)).flatten())
    }

    /// The status line.
    pub fn status<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Option<&'a str> {
        tree.text_of(self.widgets.status)
    }

    /// The report lines shown.
    pub fn report_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Vec<&'a str> {
        self.widgets
            .report
            .iter()
            .filter(|id| tree.is_shown(**id))
            .filter_map(|id| tree.text_of(*id))
            .collect()
    }

    /// Whether `Check`, `Repair` and `Format...` are enabled.
    pub fn buttons_enabled(&self, tree: &WidgetTree<CAPACITY>) -> [bool; 3] {
        let widgets = self.widgets;
        [widgets.check, widgets.repair, widgets.format]
            .map(|id| tree.is_shown(id) && tree.is_enabled(id))
    }

    /// The format panel's three lines.
    pub fn panel_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [Option<&'a str>; 3] {
        let widgets = self.widgets;
        [widgets.question, widgets.erases, widgets.confirm].map(|id| tree.text_of(id))
    }
}

#[cfg(test)]
mod tests;
