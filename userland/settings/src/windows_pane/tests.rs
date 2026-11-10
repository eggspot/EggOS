use super::*;
use crate::facts::Facts;
use crate::settings_window::{COMPONENTS, PANES, Request, SettingsWindow, WINDOWS, pane_matches};
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::{Input, Modifiers, MouseButton};
use eggos_windows_registry::command::Console;
use std::cell::RefCell;
use std::rc::Rc;

const SIDEBAR_ROW_Y: i32 = 44 + 9 * 32 + 10;
const AUTOHOTKEY: &str = "HKEY_CURRENT_USER\\Software\\AutoHotkey";
const APPEARANCE: usize = 0;

#[derive(Debug, Default)]
struct Hives {
    files: [Option<Vec<u8>>; 2],
    temporaries: [Option<Vec<u8>>; 2],
    damaged: [Option<Vec<u8>>; 2],
    stamps: [u64; 2],
    user_id: u32,
    clock: u64,
    writes: usize,
    disk_full: bool,
}

#[derive(Debug, Clone, Default)]
struct SharedStore(Rc<RefCell<Hives>>);

fn slot(file: HiveFile) -> usize {
    match file {
        HiveFile::User => 0,
        HiveFile::Machine => 1,
    }
}

impl Store for SharedStore {
    fn read(&mut self, file: HiveFile) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.0.borrow().files[slot(file)].clone())
    }

    fn stamp(&mut self, file: HiveFile) -> Option<u64> {
        let hives = self.0.borrow();
        hives.files[slot(file)]
            .as_ref()
            .map(|_| hives.stamps[slot(file)])
    }

    fn write_temporary(&mut self, file: HiveFile, bytes: &[u8]) -> Result<(), StoreError> {
        let mut hives = self.0.borrow_mut();
        if hives.disk_full {
            return Err(StoreError::DiskFull);
        }
        hives.temporaries[slot(file)] = Some(bytes.to_vec());
        Ok(())
    }

    fn replace_with_temporary(&mut self, file: HiveFile) -> Result<(), StoreError> {
        let mut hives = self.0.borrow_mut();
        let index = slot(file);
        hives.files[index] = hives.temporaries[index].take();
        hives.stamps[index] += 1;
        hives.writes += 1;
        Ok(())
    }

    fn set_aside(&mut self, file: HiveFile) -> Result<(), StoreError> {
        let mut hives = self.0.borrow_mut();
        let index = slot(file);
        if let Some(bytes) = hives.files[index].take() {
            hives.damaged[index] = Some(bytes);
            hives.stamps[index] += 1;
        }
        Ok(())
    }

    fn user_id(&mut self) -> u32 {
        self.0.borrow().user_id
    }

    fn now(&mut self) -> u64 {
        let mut hives = self.0.borrow_mut();
        hives.clock += 1;
        hives.clock
    }
}

#[derive(Debug, Default)]
struct Output {
    out: Vec<u8>,
}

impl Console for Output {
    fn out(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    fn err(&mut self, _: &[u8]) {}

    fn confirm(&mut self, _: &str) -> Option<bool> {
        None
    }

    fn wants_coded_errors(&self) -> bool {
        false
    }
}

fn path(text: &str) -> KeyPath {
    KeyPath::parse(text).unwrap()
}

fn sample() -> SharedStore {
    let store = SharedStore::default();
    store.0.borrow_mut().user_id = 0;
    let mut registry = Registry::new(store.clone());
    let autohotkey = path(AUTOHOTKEY);
    for subkey in ["Settings", "Window"] {
        registry
            .create_key(&autohotkey.join(&utf16(subkey)).unwrap())
            .unwrap();
    }
    for (name, kind, data) in [
        ("Version", REG_SZ, string_data("2.0.19")),
        ("Count", REG_DWORD, 42u32.to_le_bytes().to_vec()),
        ("Paths", REG_MULTI_SZ, strings_data(&["a", "b"])),
        ("Blob", REG_BINARY, alloc::vec![0x4a, 0x00, 0xff, 0x1e]),
    ] {
        registry
            .set_value(&autohotkey, &utf16(name), kind, data)
            .unwrap();
    }
    registry
        .create_key(&path("HKLM\\Software\\Vendor"))
        .unwrap();
    store.0.borrow_mut().user_id = 1000;
    store
}

fn opened(store: &SharedStore) -> SettingsWindow {
    let mut window = SettingsWindow::new(Facts::example());
    window.attach_registry(Box::new(store.clone()));
    window.show_windows_layer(true);
    assert_eq!(window.open_pane(WINDOWS), None);
    window
}

fn ids(window: &SettingsWindow) -> Widgets {
    window.windows_pane().widgets
}

fn rect(window: &SettingsWindow, id: WidgetId) -> Rect {
    window.widgets().rect(id).unwrap()
}

fn key_input(window: &mut SettingsWindow, code: u16, character: u32) -> Option<Request> {
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

fn press(window: &mut SettingsWindow, code: u16) -> Option<Request> {
    key_input(window, code, 0)
}

fn type_text(window: &mut SettingsWindow, text: &str) {
    for character in text.chars() {
        key_input(window, key::A, character as u32);
    }
}

fn replace_text(window: &mut SettingsWindow, id: WidgetId, text: &str) {
    window.tree().set_text(id, text);
    window.respond(Response::Edited(id), 0);
}

fn activate(window: &mut SettingsWindow, id: WidgetId) -> Option<Request> {
    window.respond(Response::Activated(id), 0)
}

fn status(window: &SettingsWindow) -> String {
    String::from(window.windows_pane().status(window.widgets()).unwrap_or(""))
}

fn value_rows(window: &SettingsWindow) -> Vec<[String; 3]> {
    window.windows_pane().value_rows()
}

fn cells(name: &str, kind: &str, data: &str) -> [String; 3] {
    [name, kind, data].map(String::from)
}

fn go_to(window: &mut SettingsWindow, typed: &str) {
    let field = ids(window).path;
    window.tree().set_focus(Some(field), 0);
    window.tree().set_text(field, "");
    type_text(window, typed);
    press(window, key::ENTER);
}

fn registry_of(store: &SharedStore) -> Registry<SharedStore> {
    Registry::new(store.clone())
}

#[test]
fn the_windows_row_shows_only_with_the_layer_and_its_words_find_it() {
    for word in ["registry", "regedit", "HKCU", "REG_SZ", "hive"] {
        for pane in 0..PANES.len() {
            assert_eq!(pane_matches(pane, word), pane == WINDOWS, "{word} {pane}");
        }
    }
    for pane in 0..PANES.len() {
        assert_eq!(
            pane_matches(pane, "Windows"),
            pane == WINDOWS || pane == APPEARANCE || pane == COMPONENTS,
            "{pane}"
        );
    }
    let click = |window: &mut SettingsWindow| {
        for pressed in [true, false] {
            let response = window.tree().handle(
                Input::Button {
                    x: 40,
                    y: SIDEBAR_ROW_Y,
                    button: MouseButton::Left,
                    pressed,
                },
                0,
            );
            window.respond(response, 0);
        }
    };
    let mut window = SettingsWindow::new(Facts::example());
    click(&mut window);
    assert_eq!(window.pane(), COMPONENTS);
    let mut window = SettingsWindow::new(Facts::example());
    window.show_windows_layer(true);
    click(&mut window);
    assert_eq!(window.pane(), WINDOWS);
    assert_eq!(window.show_windows_layer(false), Some(Request::ReadComponents));
    assert_eq!(window.pane(), COMPONENTS);
}

#[test]
fn the_pane_is_laid_out_as_the_design_says() {
    let window = opened(&sample());
    let widgets = ids(&window);
    assert_eq!(
        window.widgets().text_of(widgets.path),
        Some("HKEY_CURRENT_USER")
    );
    assert_eq!(rect(&window, widgets.path), Rect::new(192, 64, 232, 28));
    let copy = rect(&window, widgets.copy);
    assert_eq!((copy.x, copy.y), (432, 64));
    let refresh = rect(&window, widgets.refresh);
    assert_eq!((refresh.x, refresh.y), (528, 64));
    assert_eq!(rect(&window, widgets.keys), Rect::new(192, 100, 176, 172));
    assert_eq!(rect(&window, widgets.values), Rect::new(376, 100, 248, 172));
    assert_eq!(
        window.windows_pane().key_rows(),
        [
            "+ HKEY_CURRENT_USER",
            "+ HKEY_LOCAL_MACHINE",
            "  HKEY_CLASSES_ROOT"
        ]
    );
    assert_eq!(window.widgets().selected(widgets.keys), Some(0));
    assert_eq!(
        value_rows(&window),
        [cells("(Default)", "REG_SZ", "(value not set)")]
    );
    for (id, x) in [
        (widgets.new_key, 192),
        (widgets.new_value, 296),
        (widgets.edit, 400),
        (widgets.delete, 504),
    ] {
        let at = rect(&window, id);
        assert_eq!((at.x, at.y), (x, 280));
    }
    assert!(!window.widgets().is_enabled(widgets.edit));
    assert!(!window.widgets().is_enabled(widgets.delete));
    let line = rect(&window, widgets.status);
    assert_eq!((line.x, line.y), (192, 316));
    assert_eq!(status(&window), ROOT_LINE);
}

#[test]
fn a_key_opens_by_enter_or_by_its_typed_path_and_shows_its_values() {
    let store = sample();
    let mut window = opened(&store);
    let widgets = ids(&window);
    window.tree().set_focus(Some(widgets.keys), 0);
    press(&mut window, key::ENTER);
    assert_eq!(window.windows_pane().key_rows()[1], "    + Software");
    go_to(&mut window, AUTOHOTKEY);
    assert_eq!(window.windows_pane().selected_path(), AUTOHOTKEY);
    assert_eq!(window.widgets().text_of(widgets.path), Some(AUTOHOTKEY));
    assert_eq!(
        value_rows(&window),
        [
            cells("(Default)", "REG_SZ", "(value not set)"),
            cells("Blob", "REG_BINARY", "4a 00 ff 1e"),
            cells("Count", "REG_DWORD", "0x0000002a (42)"),
            cells("Paths", "REG_MULTI_SZ", "a \u{b7} b"),
            cells("Version", "REG_SZ", "2.0.19"),
        ]
    );
    assert!(
        window
            .windows_pane()
            .key_rows()
            .contains(&String::from("        + AutoHotkey"))
    );
    go_to(&mut window, "HKCU\\Software\\Nothing");
    assert_eq!(
        status(&window),
        "No such key: HKCU\\Software\\Nothing."
    );
    assert_eq!(window.windows_pane().selected_path(), AUTOHOTKEY);
}

#[test]
fn editing_a_number_switches_bases_checks_it_and_saves_it_through_the_registry() {
    let store = sample();
    let mut window = opened(&store);
    go_to(&mut window, AUTOHOTKEY);
    let widgets = ids(&window);
    window.tree().set_focus(Some(widgets.values), 0);
    press(&mut window, key::DOWN);
    press(&mut window, key::DOWN);
    assert_eq!(press(&mut window, key::ENTER), None);
    assert!(window.windows_pane().panel_open());
    assert_eq!(rect(&window, widgets.edit_panel), Rect::new(208, 70, 400, 208));
    assert_eq!(window.widgets().text_of(widgets.edit_title), Some("Edit Count"));
    assert_eq!(window.widgets().text_of(widgets.edit_field), Some("2a"));
    assert_eq!(window.widgets().selected(widgets.edit_bases), Some(0));
    window.tree().set_selected(widgets.edit_bases, 1);
    window.respond(Response::Chosen(widgets.edit_bases, 1), 0);
    assert_eq!(window.widgets().text_of(widgets.edit_field), Some("42"));
    replace_text(&mut window, widgets.edit_field, "42x");
    assert!(!window.widgets().is_enabled(widgets.edit_save));
    assert!(
        window
            .widgets()
            .text_of(widgets.edit_line)
            .is_some_and(|line| line.starts_with("Not a number."))
    );
    replace_text(&mut window, widgets.edit_field, "43");
    window.tree().set_focus(Some(widgets.edit_field), 0);
    let writes = store.0.borrow().writes;
    press(&mut window, key::ENTER);
    assert!(!window.windows_pane().panel_open());
    assert_eq!(value_rows(&window)[2], cells("Count", "REG_DWORD", "0x0000002b (43)"));
    assert_eq!(status(&window), "Saved Count.");
    assert_eq!(store.0.borrow().writes, writes + 1);
    let saved = registry_of(&store)
        .value(&path(AUTOHOTKEY), &utf16("Count"))
        .unwrap();
    assert_eq!((saved.kind, saved.data), (REG_DWORD, 43u32.to_le_bytes().to_vec()));
}

#[test]
fn new_keys_and_values_are_checked_then_made() {
    let store = sample();
    let mut window = opened(&store);
    go_to(&mut window, AUTOHOTKEY);
    let widgets = ids(&window);
    activate(&mut window, widgets.new_key);
    assert_eq!(rect(&window, widgets.name_panel), Rect::new(208, 88, 400, 172));
    assert!(!window.widgets().is_enabled(widgets.name_create));
    replace_text(&mut window, widgets.name_field, "Set\\tings");
    assert_eq!(
        window.widgets().text_of(widgets.name_line),
        Some("Key names cannot contain \\ .")
    );
    replace_text(&mut window, widgets.name_field, "settings");
    assert_eq!(
        window.widgets().text_of(widgets.name_line),
        Some("A key named settings already exists here.")
    );
    replace_text(&mut window, widgets.name_field, "Themes");
    window.tree().set_focus(Some(widgets.name_field), 0);
    press(&mut window, key::ENTER);
    assert_eq!(
        window.windows_pane().selected_path(),
        "HKEY_CURRENT_USER\\Software\\AutoHotkey\\Themes"
    );
    assert_eq!(status(&window), "Created Themes.");
    assert!(
        window
            .windows_pane()
            .key_rows()
            .contains(&String::from("              Themes"))
    );

    go_to(&mut window, AUTOHOTKEY);
    activate(&mut window, widgets.new_value);
    assert_eq!(rect(&window, widgets.name_panel), Rect::new(208, 70, 400, 208));
    replace_text(&mut window, widgets.name_field, "Flag");
    window.tree().set_selected(widgets.name_type, 3);
    window.respond(Response::Chosen(widgets.name_type, 3), 0);
    window.tree().set_focus(Some(widgets.name_field), 0);
    press(&mut window, key::ENTER);
    assert!(window.windows_pane().panel_open());
    assert_eq!(window.widgets().text_of(widgets.edit_title), Some("Edit Flag"));
    assert_eq!(window.widgets().focus(), Some(widgets.edit_field));
    press(&mut window, key::ESC);
    press(&mut window, key::ESC);
    assert!(!window.windows_pane().panel_open());
    assert!(
        value_rows(&window).contains(&cells("Flag", "REG_DWORD", "0x00000000 (0)"))
    );
    assert_eq!(status(&window), "Created Flag with no data.");
}

#[test]
fn deleting_a_key_asks_first_then_selects_its_parent() {
    let store = sample();
    let mut window = opened(&store);
    go_to(&mut window, AUTOHOTKEY);
    let widgets = ids(&window);
    window.tree().set_focus(Some(widgets.keys), 0);
    press(&mut window, key::DELETE);
    assert_eq!(rect(&window, widgets.confirm_panel), Rect::new(208, 98, 400, 152));
    assert_eq!(
        [
            widgets.confirm_title,
            widgets.confirm_detail,
            widgets.confirm_note
        ]
        .map(|id| window.widgets().text_of(id).unwrap_or("")),
        [
            "Delete AutoHotkey?",
            "With its 2 subkeys and 4 values.",
            "This cannot be undone."
        ]
    );
    assert_eq!(window.widgets().focus(), Some(widgets.confirm_cancel));
    assert!(window.widgets().is_person_only(widgets.confirm_primary));
    press(&mut window, key::TAB);
    press(&mut window, key::ENTER);
    assert_eq!(
        window.windows_pane().selected_path(),
        "HKEY_CURRENT_USER\\Software"
    );
    assert_eq!(status(&window), "Deleted AutoHotkey.");
    let mut output = Output::default();
    let status = eggos_windows_registry::command::run(
        &mut registry_of(&store),
        &mut output,
        &["registry", "get", "HKCU\\Software\\AutoHotkey"],
    );
    assert_eq!(status, 1);
}

#[test]
fn the_machine_s_keys_are_read_only_for_a_user_but_their_path_copies() {
    let store = sample();
    let mut window = opened(&store);
    go_to(&mut window, "HKLM\\Software\\Vendor");
    let widgets = ids(&window);
    for id in [
        widgets.new_key,
        widgets.new_value,
        widgets.edit,
        widgets.delete,
    ] {
        assert!(!window.widgets().is_enabled(id));
    }
    assert_eq!(status(&window), MACHINE_WIDE_LINE);
    assert_eq!(activate(&mut window, widgets.copy), Some(Request::CopyToClipboard));
    assert_eq!(
        window.windows_pane().to_copy(),
        "HKEY_LOCAL_MACHINE\\Software\\Vendor"
    );
    assert_eq!(status(&window), "Copied HKEY_LOCAL_MACHINE\\Software\\Vendor.");
}

#[test]
fn a_damaged_registry_file_is_replaced_after_asking() {
    let store = sample();
    store.0.borrow_mut().files[0] = Some(b"not a hive".to_vec());
    let mut window = opened(&store);
    let widgets = ids(&window);
    assert_eq!(status(&window), USER_DAMAGED_LINE);
    assert_eq!(rect(&window, widgets.replace), Rect::new(192, 280, 136, 28));
    assert!(!window.widgets().is_shown(widgets.new_key));
    activate(&mut window, widgets.replace);
    assert_eq!(
        window.widgets().text_of(widgets.confirm_title),
        Some("Replace the damaged registry file?")
    );
    press(&mut window, key::TAB);
    press(&mut window, key::ENTER);
    assert_eq!(status(&window), REPLACED_LINE);
    assert_eq!(
        store.0.borrow().damaged[0].as_deref(),
        Some(&b"not a hive"[..])
    );
    assert!(window.widgets().is_shown(widgets.new_key));
}

#[test]
fn a_write_that_fails_keeps_the_panel_open_with_the_reason() {
    let store = sample();
    let mut window = opened(&store);
    go_to(&mut window, AUTOHOTKEY);
    let widgets = ids(&window);
    window.tree().set_focus(Some(widgets.values), 0);
    press(&mut window, key::END);
    press(&mut window, key::ENTER);
    assert_eq!(window.widgets().text_of(widgets.edit_title), Some("Edit Version"));
    store.0.borrow_mut().disk_full = true;
    activate(&mut window, widgets.edit_save);
    assert!(window.windows_pane().panel_open());
    assert_eq!(
        window.widgets().text_of(widgets.edit_line),
        Some("Not saved: the disk is full. Free some space and try again.")
    );
}

#[test]
fn the_table_and_the_command_show_the_same_cells() {
    let store = sample();
    let mut window = opened(&store);
    go_to(&mut window, AUTOHOTKEY);
    let mut output = Output::default();
    let status = eggos_windows_registry::command::run(
        &mut registry_of(&store),
        &mut output,
        &["registry", "get", "HKCU\\Software\\AutoHotkey", "Count"],
    );
    assert_eq!(status, 0);
    assert_eq!(
        String::from_utf8(output.out).unwrap(),
        "Count  REG_DWORD  0x0000002a (42)\n"
    );
    let row = value_rows(&window)
        .into_iter()
        .find(|row| row[0] == "Count")
        .unwrap();
    assert_eq!(row.join("  "), "Count  REG_DWORD  0x0000002a (42)");
}
