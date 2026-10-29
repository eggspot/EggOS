//! The Windows controls boot test's checks of the display (D145): `win-controls.exe`'s window
//! with its menu bar and controls, typing, clicks on a button, a check box and a list row, the
//! File menu, the Save As and Open pickers and the Exit item, each frame compared with the
//! desktop the host renders, the window's content painted by `eggos_windows_gui` from the
//! program's own scene.

use super::{
    Desktop, DiscardEvents, TERMINAL_FRAME_TIMEOUT, WindowContent, apply_pointer_step,
    check_desktop_frame_within, click_steps, send_pointer_step,
};
use crate::Arch;
use crate::qmp::{InputEvent, Qmp};
use eggos_drivers::desktop::WindowId;
use eggos_drivers::display::Damage;
use eggos_drivers::input::keymap::Modifiers;
use eggos_windows_gui::colour;
use eggos_windows_gui::controls::button::{self, ButtonKind, ButtonLook};
use eggos_windows_gui::controls::edit::{self, Edit, EditLook};
use eggos_windows_gui::controls::label::{self, LabelLook};
use eggos_windows_gui::controls::list_box::{self, ListBox, ListLook};
use eggos_windows_gui::controls::{DARK, LIGHT, Palette};
use eggos_windows_gui::file_picker::{self as picker_layout, PickerKind};
use eggos_windows_gui::menu::{self, BAR_HEIGHT, MenuLabel};
use eggos_windows_gui::surface::{Rect, Surface};
use eggos_windows_gui_tests::controls::{self as scene, Control, Focus, Frame};
use std::sync::mpsc;
use std::time::Duration;

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const CLIENT: u16 = 0;
const ROLE_NORMAL: u32 = 0;
const ROLE_DIALOG: u32 = 1;
const KEY_PAUSE: Duration = Duration::from_millis(100);
const BUTTON_FACE: u32 = 15;
const ENTER: (&str, u16) = ("ret", 28);
const LETTERS: [(char, &str, u16); 5] = [
    ('e', "e", 18),
    ('g', "g", 34),
    ('n', "n", 49),
    ('o', "o", 24),
    ('t', "t", 20),
];

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn rect_of(control: &Control) -> Rect {
    let (x, y, width, height) = control.rect;
    Rect::sized(x, y, width, height)
}

fn edit_holding(text: &str, read_only: bool, width: i32, caret_at_end: bool) -> Box<Edit> {
    let mut model = Box::new(Edit::new());
    model.read_only = read_only;
    let visible = edit::visible_cells(width);
    for unit in text.encode_utf16() {
        model.character(u32::from(unit), visible);
    }
    model.read_only = read_only;
    if read_only {
        model.set_text(&units(text));
    }
    if caret_at_end {
        model.caret_to_end(visible);
    }
    model
}

fn list_holding(items: &[&str], selected: Option<usize>, marks: bool, height: i32) -> Box<ListBox> {
    let mut model = Box::new(ListBox::new());
    model.marks = marks;
    for item in items {
        model.add(&units(item), !marks);
    }
    model.select(selected, list_box::visible_rows(height));
    model
}

fn at(surface: &mut Surface<'_>, rect: Rect, paint: impl FnOnce(&mut Surface<'_>, Rect)) {
    let saved = surface.clip();
    surface.translate(rect.left, rect.top);
    surface.clip_to(Rect::new(0, 0, rect.width(), rect.height()));
    paint(surface, Rect::new(0, 0, rect.width(), rect.height()));
    surface.translate(-rect.left, -rect.top);
    surface.set_clip(saved);
}

fn push_button(
    surface: &mut Surface<'_>,
    rect: Rect,
    text: &str,
    kind: ButtonKind,
    focused: bool,
    palette: &Palette,
) {
    let look = ButtonLook {
        kind,
        checked: false,
        pressed: false,
        focused,
        default: focused || kind == ButtonKind::DefaultPush,
        enabled: true,
    };
    at(surface, rect, |surface, local| {
        button::paint(surface, local, &units(text), look, palette)
    });
}

fn menu_labels(items: &[scene::MenuItem]) -> Vec<(Vec<u16>, bool)> {
    items
        .iter()
        .map(|(label, _)| (label.map(units).unwrap_or_default(), label.is_none()))
        .collect()
}

fn as_labels(labels: &[(Vec<u16>, bool)]) -> Vec<MenuLabel<'_>> {
    labels
        .iter()
        .map(|(label, separator)| MenuLabel {
            label,
            separator: *separator,
            grayed: false,
            checked: false,
        })
        .collect()
}

/// The main window's content at `frame`.
pub(super) fn window_content(frame: &Frame) -> WindowContent {
    let (width, height) = (scene::CONTENT.0 as u32, scene::CONTENT.1 as u32);
    let face = colour::system_colour(BUTTON_FACE).expect("COLOR_BTNFACE");
    let mut pixels = vec![face; (width * height) as usize];
    let mut surface = Surface::new(&mut pixels, width, height).expect("the content fits");
    let entries: Vec<(Vec<u16>, bool)> = scene::MENU
        .iter()
        .map(|(label, _)| (units(label), false))
        .collect();
    let entry_labels = as_labels(&entries);
    menu::paint_bar(
        &mut surface,
        scene::CONTENT.0,
        &entry_labels,
        frame.file_menu_open.then_some(0),
        &LIGHT,
    );
    surface.translate(0, BAR_HEIGHT);
    let label_look = LabelLook {
        style: 0,
        enabled: true,
    };
    at(
        &mut surface,
        rect_of(&scene::NAME_LABEL),
        |surface, local| {
            label::paint(
                surface,
                local,
                &units(scene::NAME_LABEL.text),
                label_look,
                &LIGHT,
            )
        },
    );
    let name_rect = rect_of(&scene::NAME);
    let name = edit_holding(frame.name, false, name_rect.width(), true);
    at(&mut surface, name_rect, |surface, local| {
        let look = EditLook {
            focused: frame.focus == Focus::Name,
            enabled: true,
        };
        edit::paint(surface, local, &name, look, &LIGHT)
    });
    push_button(
        &mut surface,
        rect_of(&scene::ADD),
        scene::ADD.text,
        ButtonKind::DefaultPush,
        frame.focus == Focus::Add,
        &LIGHT,
    );
    at(
        &mut surface,
        rect_of(&scene::UPPER_CASE),
        |surface, local| {
            let look = ButtonLook {
                kind: ButtonKind::AutoCheckBox,
                checked: frame.upper_case,
                pressed: false,
                focused: frame.focus == Focus::UpperCase,
                default: false,
                enabled: true,
            };
            button::paint(surface, local, &units(scene::UPPER_CASE.text), look, &LIGHT)
        },
    );
    let list_rect = rect_of(&scene::LIST);
    let list = list_holding(frame.items, frame.selected, false, list_rect.height());
    at(&mut surface, list_rect, |surface, local| {
        let look = ListLook {
            focused: frame.focus == Focus::List,
            enabled: true,
        };
        list_box::paint(surface, local, &list, look, &LIGHT)
    });
    at(&mut surface, rect_of(&scene::STATUS), |surface, local| {
        label::paint(surface, local, &units(frame.status), label_look, &LIGHT)
    });
    surface.translate(0, -BAR_HEIGHT);
    if frame.file_menu_open {
        let (_, items) = scene::MENU[0];
        let labels = menu_labels(items);
        let item_labels = as_labels(&labels);
        let rect = menu::drop_down_rect(&item_labels, 0, scene::CONTENT.0);
        menu::paint_drop_down(&mut surface, rect, &item_labels, None, &LIGHT);
    }
    WindowContent::Pixels { width, pixels }
}

/// What a picker shows: its kind, the entries listed, the selected one, and the file name
/// edit's text and whether it has the keys (the list has them otherwise).
#[derive(Debug, Clone, Copy)]
pub(super) struct PickerFrame {
    pub kind: PickerKind,
    pub entries: &'static [&'static str],
    pub selected: Option<usize>,
    pub name: &'static str,
    pub name_focused: bool,
}

/// A picker's content at `frame`.
pub(super) fn picker_content(frame: &PickerFrame) -> WindowContent {
    let (width, height) = (
        picker_layout::CONTENT.0 as u32,
        picker_layout::CONTENT.1 as u32,
    );
    let mut pixels = vec![DARK.surface; (width * height) as usize];
    let mut surface = Surface::new(&mut pixels, width, height).expect("the picker fits");
    push_button(
        &mut surface,
        picker_layout::UP,
        picker_layout::UP_LABEL,
        ButtonKind::Push,
        false,
        &DARK,
    );
    let folder = edit_holding(scene::FOLDER, true, picker_layout::FOLDER.width(), false);
    at(&mut surface, picker_layout::FOLDER, |surface, local| {
        let look = EditLook {
            focused: false,
            enabled: true,
        };
        edit::paint(surface, local, &folder, look, &DARK)
    });
    let list = list_holding(
        frame.entries,
        frame.selected,
        true,
        picker_layout::ENTRIES.height(),
    );
    at(&mut surface, picker_layout::ENTRIES, |surface, local| {
        let look = ListLook {
            focused: !frame.name_focused,
            enabled: true,
        };
        list_box::paint(surface, local, &list, look, &DARK)
    });
    at(&mut surface, picker_layout::NAME_LABEL, |surface, local| {
        let look = LabelLook {
            style: 0,
            enabled: true,
        };
        label::paint(
            surface,
            local,
            &units(picker_layout::NAME_LABEL_TEXT),
            look,
            &DARK,
        )
    });
    let name = edit_holding(frame.name, false, picker_layout::NAME.width(), true);
    at(&mut surface, picker_layout::NAME, |surface, local| {
        let look = EditLook {
            focused: frame.name_focused,
            enabled: true,
        };
        edit::paint(surface, local, &name, look, &DARK)
    });
    at(&mut surface, picker_layout::FILTER, |surface, local| {
        let look = LabelLook {
            style: 0,
            enabled: false,
        };
        label::paint(
            surface,
            local,
            &units(scene::FILTER_DESCRIPTION),
            look,
            &DARK,
        )
    });
    push_button(
        &mut surface,
        picker_layout::CANCEL,
        picker_layout::CANCEL_LABEL,
        ButtonKind::Push,
        false,
        &DARK,
    );
    push_button(
        &mut surface,
        picker_layout::ACCEPT,
        frame.kind.accept_label(),
        ButtonKind::DefaultPush,
        false,
        &DARK,
    );
    WindowContent::Pixels { width, pixels }
}

pub(super) struct ControlsSteps<'a> {
    pub arch: &'a Arch,
    pub qmp: &'a mut Qmp,
    pub desktop: &'a mut Desktop,
    pub console: &'a mut Vec<u8>,
    pub output: &'a mpsc::Receiver<Vec<u8>>,
    pub screen: (u32, u32),
    window: WindowId,
    frame: Frame,
    picker: Option<(WindowId, PickerFrame)>,
}

impl ControlsSteps<'_> {
    fn check(&mut self, at: &str, timeout: Duration) -> Result<(), String> {
        let mut windows = vec![(self.window, window_content(&self.frame))];
        if let Some((picker, frame)) = &self.picker {
            windows.push((*picker, picker_content(frame)));
        }
        check_desktop_frame_within(
            self.arch,
            self.qmp,
            self.console,
            self.output,
            at,
            self.desktop,
            &windows,
            timeout,
        )
    }

    fn refused(&self, what: &str, error: impl std::fmt::Debug) -> String {
        format!(
            "{}: the host's desktop refused {what}: {error:?}",
            self.arch.name
        )
    }

    fn create(
        &mut self,
        role: u32,
        parent: u32,
        size: (i32, i32),
        title: &str,
    ) -> Result<WindowId, String> {
        let created = self
            .desktop
            .create(CLIENT, role, parent, size.0 as u32, size.1 as u32, 0)
            .map_err(|error| self.refused("a window", error))?;
        let id = u32::from(created.window);
        self.desktop
            .set_title(CLIENT, id, title.as_bytes(), &mut Damage::new())
            .map_err(|error| self.refused("a title", error))?;
        self.desktop
            .commit(CLIENT, id, &[], &mut Damage::new(), &mut DiscardEvents)
            .map_err(|error| self.refused("a commit", error))?;
        Ok(created.window)
    }

    fn destroy(&mut self, window: WindowId) -> Result<(), String> {
        self.desktop
            .destroy(
                CLIENT,
                u32::from(window),
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .map_err(|error| self.refused("to destroy a window", error))
    }

    fn click_in(&mut self, window: WindowId, (x, y): (i32, i32)) -> Result<(), String> {
        let content = self
            .desktop
            .content_rect(window)
            .ok_or_else(|| format!("{}: the window {window} is gone", self.arch.name))?;
        for step in click_steps(content.x + x as u32, content.y + y as u32) {
            send_pointer_step(self.qmp, step, self.screen)?;
            apply_pointer_step(self.desktop, step);
        }
        Ok(())
    }

    fn click_control(&mut self, control: &Control, (dx, dy): (i32, i32)) -> Result<(), String> {
        let (x, y, _, _) = control.rect;
        self.click_in(self.window, (x + dx, y + BAR_HEIGHT + dy))
    }

    fn tap(&mut self, qcode: &'static str, code: u16) -> Result<(), String> {
        for down in [true, false] {
            self.qmp.send_input(&[InputEvent::Key { qcode, down }])?;
            std::thread::sleep(KEY_PAUSE);
            self.desktop.key(
                code,
                down,
                Modifiers::NONE,
                &mut Damage::new(),
                &mut DiscardEvents,
            );
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), String> {
        for letter in text.chars() {
            let (_, qcode, code) = LETTERS
                .iter()
                .find(|(known, _, _)| *known == letter)
                .ok_or_else(|| format!("no key for {letter}"))?;
            self.tap(qcode, *code)?;
        }
        Ok(())
    }

    fn open_picker(&mut self, kind: PickerKind, frame: PickerFrame) -> Result<(), String> {
        let picker = self.create(
            ROLE_DIALOG,
            u32::from(self.window),
            picker_layout::CONTENT,
            kind.title(),
        )?;
        self.picker = Some((picker, frame));
        Ok(())
    }

    fn close_picker(&mut self) -> Result<(), String> {
        if let Some((picker, _)) = self.picker.take() {
            self.destroy(picker)?;
        }
        Ok(())
    }

    fn choose_from_file_menu(&mut self, item: (i32, i32)) -> Result<(), String> {
        self.click_in(self.window, scene::FILE_ENTRY_POINT)?;
        self.click_in(self.window, item)?;
        self.frame.file_menu_open = false;
        Ok(())
    }

    fn run(&mut self) -> Result<(), String> {
        self.check(
            "win-controls' window with its timers fired",
            FIRST_FRAME_TIMEOUT,
        )?;

        self.click_control(&scene::NAME, (100, 12))?;
        self.type_text(scene::TYPED)?;
        self.frame = scene::TYPED_IN;
        self.check("egg typed into the edit", TERMINAL_FRAME_TIMEOUT)?;

        self.click_control(&scene::ADD, (44, 12))?;
        self.frame = scene::ADDED;
        self.check("Add clicked: egg in the list", TERMINAL_FRAME_TIMEOUT)?;

        self.click_control(&scene::UPPER_CASE, (80, 12))?;
        self.frame = scene::CHECKED;
        self.check("the check box checked", TERMINAL_FRAME_TIMEOUT)?;

        self.click_control(&scene::LIST, (100, 31))?;
        self.frame = scene::SELECTED;
        self.check("the egg row selected", TERMINAL_FRAME_TIMEOUT)?;

        self.click_in(self.window, scene::FILE_ENTRY_POINT)?;
        self.frame = scene::MENU_OPEN;
        self.check("the File menu open", TERMINAL_FRAME_TIMEOUT)?;

        self.click_in(self.window, scene::SAVE_ITEM_POINT)?;
        self.frame = scene::SELECTED;
        let empty = PickerFrame {
            kind: PickerKind::Save,
            entries: &[],
            selected: None,
            name: "",
            name_focused: true,
        };
        self.open_picker(PickerKind::Save, empty)?;
        self.check("the Save As picker", TERMINAL_FRAME_TIMEOUT)?;

        self.type_text(scene::SAVE_TYPED)?;
        let (qcode, code) = ENTER;
        self.tap(qcode, code)?;
        self.close_picker()?;
        self.frame = scene::SAVED;
        self.check("note.txt saved", TERMINAL_FRAME_TIMEOUT)?;

        self.choose_from_file_menu(scene::OPEN_ITEM_POINT)?;
        let listed = PickerFrame {
            kind: PickerKind::Open,
            entries: &[scene::SAVED_NAME],
            selected: None,
            name: "",
            name_focused: true,
        };
        self.open_picker(PickerKind::Open, listed)?;
        self.check("the Open picker listing note.txt", TERMINAL_FRAME_TIMEOUT)?;

        let (picker, _) = self.picker.ok_or("no picker")?;
        let row = (
            picker_layout::ENTRIES.left + 100,
            picker_layout::ENTRIES.top + 11,
        );
        self.click_in(picker, row)?;
        self.picker = Some((
            picker,
            PickerFrame {
                selected: Some(0),
                name: scene::SAVED_NAME,
                name_focused: false,
                ..listed
            },
        ));
        self.check("note.txt selected in the picker", TERMINAL_FRAME_TIMEOUT)?;

        let accept = (
            picker_layout::ACCEPT.left + 48,
            picker_layout::ACCEPT.top + 14,
        );
        self.click_in(picker, accept)?;
        self.close_picker()?;
        self.frame = scene::READ;
        self.check("note.txt read back", TERMINAL_FRAME_TIMEOUT)?;

        self.choose_from_file_menu(scene::EXIT_ITEM_POINT)?;
        self.destroy(self.window)?;
        check_desktop_frame_within(
            self.arch,
            self.qmp,
            self.console,
            self.output,
            "win-controls closed by File > Exit",
            self.desktop,
            &[],
            TERMINAL_FRAME_TIMEOUT,
        )
    }
}

/// Checks the display through the whole of `win-controls.exe`'s run, driving it through QMP,
/// on the desktop `win-paint.exe`'s check left.
pub(super) fn check(
    arch: &Arch,
    qmp: &mut Qmp,
    desktop: &mut Desktop,
    console: &mut Vec<u8>,
    output: &mpsc::Receiver<Vec<u8>>,
    screen: (u32, u32),
) -> Result<(), String> {
    let mut steps = ControlsSteps {
        arch,
        qmp,
        desktop,
        console,
        output,
        screen,
        window: WindowId::default(),
        frame: scene::STARTED,
        picker: None,
    };
    steps.window = steps.create(ROLE_NORMAL, 0, scene::CONTENT, scene::TITLE)?;
    steps.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels_of(content: WindowContent) -> (u32, Vec<u32>) {
        match content {
            WindowContent::Pixels { width, pixels } => (width, pixels),
            WindowContent::Checkerboard(_) => panic!("the content is pixels"),
        }
    }

    #[test]
    fn the_main_window_follows_the_designs_worked_example() {
        let (width, pixels) = pixels_of(window_content(&scene::TYPED_IN));
        let at = |x: i32, y: i32| pixels[(y as u32 * width + x as u32) as usize];
        assert_eq!(width, 400);
        assert_eq!(pixels.len(), 400 * 324);
        assert_eq!(at(0, 0), LIGHT.menu_bar);
        assert_eq!(at(88, 38), LIGHT.field_focus_border);
        assert_eq!(at(116, 42), LIGHT.caret);
        assert_eq!(at(296, 38), 0x0000_78d7);
        assert_eq!(at(297, 39), 0x0000_78d7);
        assert_eq!(at(18, 81), LIGHT.box_border);
        assert_eq!(at(16, 112), LIGHT.field_border);
        let (_, open) = pixels_of(window_content(&scene::MENU_OPEN));
        assert_eq!(open[(24 * 400) as usize], LIGHT.menu_border);
        assert_eq!(open[(10 * 400 + 2) as usize], LIGHT.menu_highlight);
    }

    #[test]
    fn the_picker_follows_the_designs_layout() {
        let frame = PickerFrame {
            kind: PickerKind::Open,
            entries: &[scene::SAVED_NAME],
            selected: Some(0),
            name: scene::SAVED_NAME,
            name_focused: false,
        };
        let (width, pixels) = pixels_of(picker_content(&frame));
        let at = |x: i32, y: i32| pixels[(y as u32 * width + x as u32) as usize];
        assert_eq!(width, 560);
        assert_eq!(at(0, 0), DARK.surface);
        assert_eq!(at(480, 357), DARK.default_fill);
        assert_eq!(at(30, 60), DARK.highlight);
    }
}
