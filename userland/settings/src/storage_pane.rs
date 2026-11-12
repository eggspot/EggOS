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
    formatting_text, headline, idle_text, install_allowed, install_idle_text, install_question,
    install_writes_lines, installed_text, installing_text, label_fits, more_findings_text,
    place_text, running_text, sizes_text, used_permille, INSTALLED_NOTE, NO_DISK_TO_INSTALL_ON,
};
use eggos_toolkit::canvas::{Canvas, INSET_R6};
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::key;
use eggos_toolkit::text::{ELLIPSIS, Text};
use eggos_toolkit::theme::Token;
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
const INSTALL_X: i32 = 328;
const INSTALL_WIDTH: i32 = 120;
const INSTALL_TEXT_LINES: [i32; 5] = [16, 36, 56, 76, 96];
const STATUS_LINE: i32 = 212;
const REPORT_TOP: i32 = 232;
const LINE_PITCH: i32 = 20;
const LINE_WIDTH: i32 = 432;
const TOUCH_CONTROL: i32 = 48;
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
    canvas.rounded(rect, &INSET_R6, canvas.palette().field);
    let inner = rect.inset(PANEL_EDGE);
    canvas.rounded(inner, &INSET_R6, canvas.palette().menu);
    canvas.outline(inner, &INSET_R6, 1, canvas.palette().menu_border);
}

pub(crate) fn shortened(words: &str) -> Text<LABEL_BYTES> {
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

fn show_line(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, words: &str, ink: Token) {
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
    install: WidgetId,
    install_line: WidgetId,
    install_panel: WidgetId,
    install_lines: [WidgetId; 5],
    install_field: WidgetId,
    install_cancel: WidgetId,
    install_now: WidgetId,
}

/// How many widgets the installer adds to the pane (docs/ux/installer-v0.4.md section 1): the
/// button, the install line, the panel with its painter, five lines, the field and two buttons.
pub const INSTALLER_WIDGETS: usize = 12;

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
    boot_files: bool,
    install_open: bool,
    installed: Option<&'static str>,
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
        Kind::label("", Token::Text),
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
    /// Moves the pane to one column `content` wide on a narrow page (settings-forms-v0.5.md 2,
    /// R5, R4 and R9: the disk list, then its facts, the usage bar and two pairs of buttons, the
    /// status and report lines), or back to the desktop's; returns the narrow pane's height.
    pub fn reflow(&self, tree: &mut WidgetTree<CAPACITY>, content: Option<i32>) -> i32 {
        self.reflow_panels(tree, content);
        let w = self.widgets;
        let facts = [w.contents, w.sizes, w.place, w.read_only];
        let Some(c) = content else {
            tree.set_place(w.list, at(DISK_LIST.0, DISK_LIST.1, DISK_LIST.2, DISK_LIST.3));
            tree.set_row_height(w.list, DISK_ROW);
            for (id, y) in facts.iter().zip([CONTENTS_LINE, SIZES_LINE, PLACE_LINE]) {
                tree.set_place(*id, Place::at(FACTS_X, y, Length::Px(FACTS_WIDTH), Length::Content));
            }
            tree.set_place(
                w.read_only,
                Place::at(FACTS_X, READ_ONLY_LINE, Length::Content, Length::Content),
            );
            tree.set_place(w.usage, at(USAGE_BAR.0, USAGE_BAR.1, USAGE_BAR.2, USAGE_BAR.3));
            for (id, x) in [(w.check, CHECK_X), (w.repair, REPAIR_X), (w.format, FORMAT_X)] {
                tree.set_place(id, at(x, BUTTONS_TOP, BUTTON_SIZE.0, BUTTON_SIZE.1));
            }
            tree.set_place(w.install, at(INSTALL_X, BUTTONS_TOP, INSTALL_WIDTH, BUTTON_SIZE.1));
            self.place_lines(tree, STATUS_LINE, LINE_WIDTH);
            return 0;
        };
        let row = TOUCH_CONTROL;
        tree.set_place(w.list, at(PADDING, 64, c, 2 * row));
        tree.set_row_height(w.list, row);
        for (index, id) in facts.iter().enumerate() {
            tree.set_place(
                *id,
                Place::at(PADDING, 176 + LINE_PITCH * index as i32, Length::Px(c), Length::Content),
            );
        }
        tree.set_place(w.usage, at(PADDING, 260, c, USAGE_BAR.3));
        let half = (c - 8) / 2;
        tree.set_place(w.check, at(PADDING, 276, half, row));
        tree.set_place(w.repair, at(PADDING + half + 8, 276, half, row));
        tree.set_place(w.format, at(PADDING, 332, half, row));
        tree.set_place(w.install, at(PADDING + half + 8, 332, half, row));
        self.place_lines(tree, 396, c);
        396 + LINE_PITCH * (1 + REPORT_LINES as i32) + PADDING
    }

    /// The format and install panels on a narrow page (settings-forms-v0.5.md 2, R10): the
    /// pane's full width, their fields the column's width and 48 tall, `Cancel` and the primary
    /// button an equal pair on the last row; on the desktop as built.
    fn reflow_panels(&self, tree: &mut WidgetTree<CAPACITY>, content: Option<i32>) {
        let w = self.widgets;
        let panel_width = FORMAT_PANEL.2 - 2 * PADDING;
        let Some(c) = content else {
            let panel = at(FORMAT_PANEL.0, FORMAT_PANEL.1, FORMAT_PANEL.2, FORMAT_PANEL.3);
            tree.set_place(w.panel, panel);
            tree.set_place(w.install_panel, panel);
            for (id, y) in [w.question, w.erases, w.confirm].into_iter().zip(PANEL_TEXT_LINES) {
                tree.set_place(id, Place::at(PADDING, y, Length::Px(panel_width), Length::Content));
            }
            for (id, y) in w.install_lines.into_iter().zip(INSTALL_TEXT_LINES) {
                tree.set_place(id, Place::at(PADDING, y, Length::Px(panel_width), Length::Content));
            }
            tree.set_place(w.label_field, at(PADDING, LABEL_FIELD_TOP, PANEL_FIELD_WIDTH, BUTTON_SIZE.1));
            for field in [w.confirm_field, w.install_field] {
                tree.set_place(field, at(PADDING, CONFIRM_FIELD_TOP, PANEL_FIELD_WIDTH, BUTTON_SIZE.1));
            }
            for (cancel, primary) in [(w.cancel, w.format_now), (w.install_cancel, w.install_now)] {
                tree.set_place(cancel, at(CANCEL_X, PANEL_BUTTONS_TOP, BUTTON_SIZE.0, BUTTON_SIZE.1));
                tree.set_place(primary, at(FORMAT_NOW_X, PANEL_BUTTONS_TOP, BUTTON_SIZE.0, BUTTON_SIZE.1));
            }
            return;
        };
        let row = TOUCH_CONTROL;
        let half = (c - 8) / 2;
        let sheet = |height: i32| Place::at(0, FORMAT_PANEL.1, Length::Fill(0), Length::Px(height));
        tree.set_place(w.panel, sheet(196 + row + PADDING));
        tree.set_place(w.install_panel, sheet(176 + row + PADDING));
        for (id, y) in [w.question, w.erases, w.confirm].into_iter().zip(PANEL_TEXT_LINES) {
            tree.set_place(id, Place::at(PADDING, y, Length::Px(c), Length::Content));
        }
        for (id, y) in w.install_lines.into_iter().zip(INSTALL_TEXT_LINES) {
            tree.set_place(id, Place::at(PADDING, y, Length::Px(c), Length::Content));
        }
        tree.set_place(w.label_field, at(PADDING, LABEL_FIELD_TOP, c, row));
        tree.set_place(w.confirm_field, at(PADDING, LABEL_FIELD_TOP + 56, c, row));
        tree.set_place(w.install_field, at(PADDING, 120, c, row));
        for (cancel, primary, top) in [(w.cancel, w.format_now, 196), (w.install_cancel, w.install_now, 176)] {
            tree.set_place(cancel, at(PADDING, top, half, row));
            tree.set_place(primary, at(PADDING + half + 8, top, half, row));
        }
    }

    fn place_lines(&self, tree: &mut WidgetTree<CAPACITY>, status: i32, width: i32) {
        let w = self.widgets;
        tree.set_place(w.status, Place::at(PADDING, status, Length::Px(width), Length::Content));
        let report = status + LINE_PITCH;
        tree.set_place(
            w.install_line,
            Place::at(PADDING, report, Length::Px(width), Length::Content),
        );
        for (index, id) in w.report.iter().enumerate() {
            tree.set_place(
                *id,
                Place::at(
                    PADDING,
                    report + index as i32 * LINE_PITCH,
                    Length::Px(width),
                    Length::Content,
                ),
            );
        }
    }

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
            Kind::label("Read-only", Token::Text),
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
        let install = add(
            tree,
            pane,
            at(INSTALL_X, BUTTONS_TOP, INSTALL_WIDTH, BUTTON_SIZE.1),
            Kind::button("Install EggOS...", false),
        );
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
        let install_line = line(tree, pane, PADDING, REPORT_TOP, LINE_WIDTH);
        let install_panel = add(
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
            install_panel,
            Place::fill(),
            Kind::Custom {
                painter: format_panel,
                data: 0,
            },
        );
        let install_lines = INSTALL_TEXT_LINES.map(|y| line(tree, install_panel, PADDING, y, panel_width));
        let install_field = add(
            tree,
            install_panel,
            at(PADDING, CONFIRM_FIELD_TOP, PANEL_FIELD_WIDTH, BUTTON_SIZE.1),
            Kind::text_field(storage::CONFIRM_PLACEHOLDERS[0], Escape::Clear),
        );
        let install_cancel = button(tree, install_panel, CANCEL_X, PANEL_BUTTONS_TOP, "Cancel");
        let install_now = add(
            tree,
            install_panel,
            at(
                FORMAT_NOW_X,
                PANEL_BUTTONS_TOP,
                BUTTON_SIZE.0,
                BUTTON_SIZE.1,
            ),
            Kind::button("Install", true),
        );
        tree.set_shown_items(list, 0);
        tree.set_shown(panel, false);
        tree.set_shown(install_panel, false);
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
                install,
                install_line,
                install_panel,
                install_lines,
                install_field,
                install_cancel,
                install_now,
            },
            disks: Vec::new(),
            list_error: None,
            report: None,
            busy: false,
            panel_open: false,
            select_first: true,
            default_button,
            boot_files: false,
            install_open: false,
            installed: None,
        };
        storage.refresh(tree);
        storage
    }

    /// The pane is shown again: the first disk is selected once the disks are read, and no
    /// report is kept.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        self.select_first = true;
        self.report = None;
        self.installed = None;
        if self.panel_open {
            self.close_panel(tree, None);
        }
        if self.install_open {
            self.close_install_panel(tree, None);
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
                show_line(tree, widgets.status, &headline(check), Token::Text);
                let findings = &check.findings;
                let listed = match findings.len() > REPORT_LINES {
                    true => REPORT_LINES - 1,
                    false => findings.len(),
                };
                for (id, finding) in widgets.report.iter().zip(&findings[..listed]) {
                    show_line(tree, *id, &finding_line(finding), Token::Text);
                    tree.set_shown(*id, true);
                }
                if findings.len() > REPORT_LINES {
                    let last = widgets.report[REPORT_LINES - 1];
                    let more = more_findings_text(findings.len() - listed, device);
                    show_line(tree, last, &more, Token::Text);
                    tree.set_shown(last, true);
                }
                mode == CheckMode::ReadOnly
                    && findings
                        .iter()
                        .any(|finding| finding.outcome == Outcome::Repairable)
            }
            Err(error) => {
                show_line(tree, widgets.status, &error.sentence(), Token::Text);
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
                    Token::Text,
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
                show_line(tree, widgets.confirm, &error.sentence(), Token::Text);
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
            widgets.install,
        ];
        for id in facts {
            tree.set_shown(id, record.is_some());
        }
        let Some(record) = record else {
            let no_disk_line = self.boot_files && self.list_error.is_none();
            show_line(tree, widgets.install_line, NO_DISK_TO_INSTALL_ON, Token::TextDim);
            tree.set_shown(widgets.install_line, no_disk_line);
            tree.set_shown(widgets.read_only, false);
            tree.set_shown(widgets.usage, false);
            for id in widgets.report {
                tree.set_shown(id, false);
            }
            self.report = None;
            match &self.list_error {
                Some(error) => show_line(tree, widgets.status, &error.sentence(), Token::Text),
                None => show_line(tree, widgets.status, NOTHING_TO_DO, Token::TextDim),
            }
            return;
        };
        show_line(tree, widgets.contents, &contents_text(&record), Token::Text);
        show_line(tree, widgets.sizes, &sizes_text(&record), Token::Text);
        show_line(tree, widgets.place, &place_text(&record), Token::Text);
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
        let free = !self.busy && !self.panel_open && !self.install_open;
        tree.set_enabled(widgets.list, free);
        tree.set_enabled(widgets.check, free && allowed.check);
        tree.set_enabled(
            widgets.repair,
            free && allowed.repair && report.is_some_and(|report| report.repairable),
        );
        tree.set_enabled(widgets.format, free && allowed.format);
        let may_install = install_allowed(&record, self.boot_files).is_ok();
        tree.set_enabled(widgets.install, free && may_install);
        let installed_here = self.installed == Some(record.device);
        if report.is_none() && !self.busy {
            self.report = None;
            for id in widgets.report {
                tree.set_shown(id, false);
            }
            if !installed_here {
                self.installed = None;
                show_line(tree, widgets.status, &idle_text(&record), Token::TextDim);
            }
        }
        let report_shown = widgets.report.iter().any(|id| tree.is_shown(*id));
        let install_words = match installed_here {
            true => alloc::string::String::from(INSTALLED_NOTE),
            false => install_idle_text(&record, self.boot_files),
        };
        show_line(tree, widgets.install_line, &install_words, Token::TextDim);
        tree.set_shown(widgets.install_line, !report_shown && !self.busy);
    }

    /// Whether the kernel kept the boot files, as the program read once (D241).
    pub fn set_boot_files(&mut self, tree: &mut WidgetTree<CAPACITY>, kept: bool) {
        self.boot_files = kept;
        self.refresh(tree);
    }

    /// Whether the install panel shows.
    pub fn install_panel_open(&self) -> bool {
        self.install_open
    }

    fn open_install_panel(&mut self, tree: &mut WidgetTree<CAPACITY>, record: &DiskRecord, now_ms: u32) {
        let widgets = self.widgets;
        let [first, second] = install_writes_lines(record);
        let lines = [
            (install_question(record.device), Token::Text),
            (erases_text(record), Token::TextDim),
            (first, Token::TextDim),
            (second, Token::TextDim),
            (confirm_text(record), Token::TextDim),
        ];
        for (id, (words, ink)) in widgets.install_lines.iter().zip(lines) {
            show_line(tree, *id, &words, ink);
        }
        let placeholder = confirm_placeholder(record.device);
        tree.change(widgets.install_field, |kind| {
            if let Kind::TextField(field) = kind {
                field.placeholder = placeholder;
            }
        });
        tree.set_text(widgets.install_field, "");
        tree.set_enabled(widgets.install_field, true);
        tree.set_enabled(widgets.install_cancel, true);
        tree.set_enabled(widgets.install_now, false);
        tree.set_shown(widgets.install_panel, true);
        tree.set_default_button(widgets.install_now);
        self.install_open = true;
        self.refresh(tree);
        tree.set_focus(Some(widgets.install_field), now_ms);
    }

    fn close_install_panel(&mut self, tree: &mut WidgetTree<CAPACITY>, focus_at_ms: Option<u32>) {
        let widgets = self.widgets;
        tree.set_shown(widgets.install_panel, false);
        tree.set_default_button(self.default_button);
        self.install_open = false;
        self.refresh(tree);
        if let Some(now_ms) = focus_at_ms {
            tree.set_focus(Some(widgets.install), now_ms);
        }
    }

    fn update_install_now(&self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let Some(record) = self.selected(tree) else {
            return;
        };
        let typed = trimmed_text(tree, widgets.install_field) == record.short_name();
        tree.set_enabled(widgets.install_now, typed && !self.busy);
        show_line(tree, widgets.install_lines[4], &confirm_text(record), Token::TextDim);
    }

    fn install_now(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        let widgets = self.widgets;
        let device = self.selected(tree)?.device;
        if !tree.is_enabled(widgets.install_now) || self.busy {
            return None;
        }
        self.busy = true;
        for id in [widgets.install_field, widgets.install_cancel, widgets.install_now] {
            tree.set_enabled(id, false);
        }
        show_line(tree, widgets.install_lines[4], &installing_text(device), Token::Text);
        Some(Request::InstallDisk(device))
    }

    /// Shows how installing EggOS on `device` went, at `now_ms`: done, the panel closes and the
    /// status line says so; refused, the panel stays with the reason.
    pub fn show_installed(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        device: &'static str,
        result: Result<(), StorageError>,
        now_ms: u32,
    ) {
        self.busy = false;
        let widgets = self.widgets;
        match result {
            Ok(()) => {
                self.installed = Some(device);
                self.close_install_panel(tree, Some(now_ms));
                for id in widgets.report {
                    tree.set_shown(id, false);
                }
                show_line(tree, widgets.status, &installed_text(device), Token::Text);
            }
            Err(error) => {
                for id in [widgets.install_field, widgets.install_cancel] {
                    tree.set_enabled(id, true);
                }
                self.update_install_now(tree);
                if let Some(record) = self.selected(tree).cloned() {
                    show_line(tree, widgets.install_lines[1], &erases_text(&record), Token::TextDim);
                }
                show_line(tree, widgets.install_lines[4], &error.sentence(), Token::Text);
            }
        }
        self.refresh(tree);
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
            Token::Text,
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
            Token::Text,
        );
        show_line(tree, widgets.erases, &erases_text(record), Token::TextDim);
        show_line(tree, widgets.confirm, &confirm_text(record), Token::TextDim);
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
            true => (confirm_text(record), Token::TextDim),
            false => (StorageError::LabelTooLong.sentence(), Token::Text),
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
        show_line(tree, widgets.confirm, &formatting_text(device), Token::Text);
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
            Response::Activated(id) if id == widgets.install => {
                if let Some(record) = self.selected(tree).cloned()
                    && install_allowed(&record, self.boot_files).is_ok()
                    && !self.busy
                    && !self.panel_open
                {
                    self.open_install_panel(tree, &record, now_ms);
                }
                None
            }
            Response::Edited(id) if id == widgets.install_field => {
                self.update_install_now(tree);
                None
            }
            Response::Activated(id) if id == widgets.install_cancel => {
                if !self.busy {
                    self.close_install_panel(tree, Some(now_ms));
                }
                None
            }
            Response::Activated(id) if id == widgets.install_now => self.install_now(tree),
            Response::Unused { code: key::ESC, .. } if self.install_open && !self.busy => {
                self.close_install_panel(tree, Some(now_ms));
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

    /// Whether `Install EggOS...` is shown and enabled.
    pub fn install_enabled(&self, tree: &WidgetTree<CAPACITY>) -> bool {
        tree.is_shown(self.widgets.install) && tree.is_enabled(self.widgets.install)
    }

    /// The install line, when shown.
    pub fn install_line<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Option<&'a str> {
        let id = self.widgets.install_line;
        tree.is_shown(id).then(|| tree.text_of(id)).flatten()
    }

    /// The install panel's five lines.
    pub fn install_panel_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [Option<&'a str>; 5] {
        self.widgets.install_lines.map(|id| tree.text_of(id))
    }

    /// The format panel's three lines.
    pub fn panel_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [Option<&'a str>; 3] {
        let widgets = self.widgets;
        [widgets.question, widgets.erases, widgets.confirm].map(|id| tree.text_of(id))
    }
}

#[cfg(test)]
mod tests;
