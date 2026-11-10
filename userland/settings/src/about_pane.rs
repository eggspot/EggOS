//! The About pane's `Devices` section (D220, docs/ux/devices-v0.3.md): the machine's devices in
//! a toolkit table, the rows, order and words `device list` and the MCP tool `device_list`
//! give (the shared crate `eggos_device_list`), with `Show all <n> devices` for every one. The
//! file is read each time the pane is shown
//! ([`crate::settings_window::Request::ReadDevices`]); the box re-filters
//! what was read and is unticked whenever the pane opens.

use crate::settings_window::CAPACITY;
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_device_list::{Device, KINDS, rows};
use eggos_runtime::errno::Errno;
use eggos_toolkit::cell_store::{CELL_BYTES, CellSlot, CellStore, data_of};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Align, CONTROL_HEIGHT, Column, Kind, Length, Place, Table};

/// The most devices the pane holds; the file's lines past it are left out of the table.
pub const MAX_ROWS: usize = 64;
/// The section heading's place in the pane.
pub const HEADING: (i32, i32) = (16, 182);
/// The checkbox's top; its right edge is the pane's less [`PADDING`].
pub const CHECKBOX_TOP: i32 = 176;
/// The table's top left corner in the pane; it reaches the pane's right and bottom less
/// [`PADDING`].
pub const TABLE_AT: (i32, i32) = (16, 212);
/// The margin around the section.
pub const PADDING: i32 = 16;
/// The table's columns: `Kind`, `Device` and `Driver`, none sortable.
pub const COLUMNS: [Column; 3] = [
    fixed("Kind", 128),
    fixed("Device", 176),
    fixed("Driver", 120),
];
/// What the table says when the default view has no row.
pub const NONE_BY_DEFAULT: &str = "No devices to show (tick Show all devices).";
/// What the table says when the file lists no device.
pub const NONE_AT_ALL: &str = "EggOS found no devices.";
/// What the table says when the file cannot be read.
pub const UNREADABLE: &str = "The device list could not be read.";
/// The checkbox's label when there is no count to give.
pub const SHOW_ALL: &str = "Show all devices";

const CELL_COLUMNS: usize = 3;
const SLOTS: usize = if cfg!(test) { 64 } else { 2 };

static DEVICE_CELLS: CellStore<SLOTS, MAX_ROWS, CELL_COLUMNS> = CellStore::new();

type DeviceSlot = CellSlot<'static, SLOTS, MAX_ROWS, CELL_COLUMNS>;

const fn fixed(title: &'static str, width: i32) -> Column {
    Column {
        sortable: false,
        ..Column::new(title, width, Align::Start)
    }
}

fn device_cell(data: u32, row: usize, column: usize, out: &mut Text<64>) {
    DEVICE_CELLS.write(data, row, column, out);
}

fn written(write: impl FnOnce(&mut dyn FnMut(&[u8]))) -> Text<64> {
    let mut words = [0u8; CELL_BYTES + 1];
    let mut length = 0;
    write(&mut |bytes: &[u8]| {
        let end = (length + bytes.len()).min(words.len());
        words[length..end].copy_from_slice(&bytes[..end - length]);
        length = end;
    });
    let mut text = Text::new();
    text.push_str(&alloc::string::String::from_utf8_lossy(&words[..length]));
    text
}

/// The About pane's `Devices` section and the devices last read.
#[derive(Debug)]
pub struct AboutPane {
    show_all: WidgetId,
    table: WidgetId,
    text: Vec<u8>,
    readable: bool,
    all: bool,
    shown: Vec<u64>,
    slot: Option<DeviceSlot>,
}

impl AboutPane {
    /// Builds the section in `pane`, under the About facts, with no devices until
    /// [`Self::show_devices`].
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId) -> Self {
        let mut add = |place: Place, kind: Kind| {
            tree.add(Some(pane), place, kind)
                .expect("the Settings window fits its tree")
        };
        add(
            Place::at(HEADING.0, HEADING.1, Length::Content, Length::Content),
            Kind::label("Devices", Token::TextDim),
        );
        let show_all = add(
            Place {
                height: Length::Px(CONTROL_HEIGHT),
                ..Place::right(PADDING, CHECKBOX_TOP)
            },
            Kind::Checkbox {
                text: Text::from(SHOW_ALL),
                checked: false,
            },
        );
        let mut table = Table::new("Devices", &COLUMNS, 0, device_cell, 0);
        table.empty_text = NONE_BY_DEFAULT;
        let table = add(
            Place::at(
                TABLE_AT.0,
                TABLE_AT.1,
                Length::Fill(PADDING),
                Length::Fill(PADDING),
            ),
            Kind::Table(table),
        );
        tree.set_enabled(show_all, false);
        Self {
            show_all,
            table,
            text: Vec::new(),
            readable: false,
            all: false,
            shown: Vec::new(),
            slot: None,
        }
    }

    /// The checkbox, `Show all <n> devices`.
    pub fn show_all_box(&self) -> WidgetId {
        self.show_all
    }

    /// The devices table.
    pub fn table(&self) -> WidgetId {
        self.table
    }

    /// The ids of the devices the table shows, in its order.
    pub fn shown(&self) -> &[u64] {
        &self.shown
    }

    /// Unticks the box, as the pane opening does; the devices are read again after.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        self.all = false;
        tree.set_selected(self.show_all, 0);
    }

    /// Shows the devices `read` from `/proc/driver/devices`, or that it could not be read.
    pub fn show_devices(&mut self, tree: &mut WidgetTree<CAPACITY>, read: Result<&[u8], Errno>) {
        self.text.clear();
        self.readable = read.is_ok();
        if let Ok(text) = read {
            self.text.extend_from_slice(text);
        }
        self.refill(tree, None);
    }

    /// Acts on `response` if it is the checkbox's: whether it was.
    pub fn respond(&mut self, tree: &mut WidgetTree<CAPACITY>, response: Response) -> bool {
        match response {
            Response::Toggled(id, checked) if id == self.show_all => {
                let selected = tree
                    .selected(self.table)
                    .and_then(|row| self.shown.get(row).copied());
                self.all = checked;
                self.refill(tree, selected);
                true
            }
            _ => false,
        }
    }

    fn refill(&mut self, tree: &mut WidgetTree<CAPACITY>, keep: Option<u64>) {
        let mut devices = [Device::EMPTY; MAX_ROWS];
        let (count, _) = eggos_device_list::parse_all(&self.text, &mut devices);
        let devices = &devices[..count];
        let listed = self
            .text
            .split_inclusive(|&byte| byte == b'\n')
            .filter(|line| Device::parse(line).is_some())
            .count();
        let mut order = [0usize; MAX_ROWS];
        let shown = rows(devices, self.all, &mut order);
        let by_default = rows(devices, false, &mut [0usize; MAX_ROWS]);
        if self.slot.is_none() {
            self.slot = DEVICE_CELLS.take();
        }
        self.shown.clear();
        for (row, &index) in order[..shown].iter().enumerate() {
            let device = &devices[index];
            if let Some(slot) = &self.slot {
                let kind = KINDS
                    .get(device.kind_order())
                    .map_or("", |(_, words)| *words);
                slot.set(row, 0, kind);
                slot.set(row, 1, written(|out| device.write_device(out)).as_str());
                slot.set(row, 2, written(|out| device.write_driver(out)).as_str());
            }
            self.shown.push(device.id);
        }
        tree.set_cell_data(self.table, data_of(self.slot.as_ref()));
        tree.set_empty_text(
            self.table,
            match (self.readable, self.all) {
                (false, _) => UNREADABLE,
                (true, true) => NONE_AT_ALL,
                (true, false) => NONE_BY_DEFAULT,
            },
        );
        tree.set_rows(self.table, shown);
        let selected = keep
            .and_then(|id| self.shown.iter().position(|&shown| shown == id))
            .unwrap_or(0);
        tree.set_selected(self.table, selected);
        let mut label = Text::<64>::new();
        match self.readable {
            true => {
                let _ = write!(label, "Show all {listed} devices");
            }
            false => label.set(SHOW_ALL),
        }
        tree.set_text(self.show_all, label.as_str());
        tree.set_enabled(
            self.show_all,
            self.readable && (self.all || by_default < listed),
        );
    }
}

#[cfg(test)]
mod tests;
