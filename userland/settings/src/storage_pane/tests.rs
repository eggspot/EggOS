use super::*;
use crate::facts::Facts;
use crate::settings_window::{PANES, STORAGE, SettingsWindow, pane_matches};
use alloc::string::String;
use eggos_eggfs::{BLOCK_SIZE, Finding, Problem};
use eggos_files::storage::memory::{MemoryDisks, MemoryWholeDisk};
use eggos_files::storage::{Usage, list};
use eggos_toolkit::input::{Input, Modifiers, MouseButton};

const BLOCKS: usize = 512;
const STORAGE_ROW_Y: i32 = 44 + 5 * 32 + 10;
const PANE_X: i32 = 176;

fn machine() -> MemoryDisks {
    let mut home = MemoryWholeDisk::eggfs("vda", BLOCKS, "Home");
    home.mounted_on = Some("/home");
    home.usage = Some(Usage {
        size_bytes: 2 << 20,
        used_bytes: 640 << 10,
        available_bytes: 1408 << 10,
    });
    MemoryDisks::of(vec![
        home,
        MemoryWholeDisk::eggfs("vdb", BLOCKS, "Photos"),
        MemoryWholeDisk::blank("vdc", BLOCKS),
    ])
}

fn kept(words: &str) -> String {
    String::from(shortened(words).as_str())
}

fn serve(window: &mut SettingsWindow, disks: &mut MemoryDisks, request: Option<Request>) {
    match request {
        Some(Request::ReadDisks) => window.show_disks(list(disks)),
        Some(Request::CheckDisk(device)) | Some(Request::RepairDisk(device)) => {
            let mode = match request {
                Some(Request::RepairDisk(_)) => CheckMode::Repair,
                _ => CheckMode::ReadOnly,
            };
            let result = storage::check(disks, device, mode);
            window.show_disks(list(disks));
            window.show_check(device, mode, result);
        }
        Some(Request::FormatDisk(device)) => {
            let label = String::from(window.format_label());
            let result = storage::format(disks, device, &label);
            window.show_disks(list(disks));
            window.show_formatted(device, &label, result, 0);
        }
        _ => {}
    }
}

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

fn press(window: &mut SettingsWindow, code: u16, character: u32) -> Option<Request> {
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

fn type_text(window: &mut SettingsWindow, text: &str) {
    for character in text.chars() {
        press(window, 30, character as u32);
    }
}

fn opened(disks: &mut MemoryDisks) -> SettingsWindow {
    let mut window = SettingsWindow::new(Facts::example());
    let request = click(&mut window, 40, STORAGE_ROW_Y);
    assert_eq!(window.pane(), STORAGE);
    assert_eq!(request, Some(Request::ReadDisks));
    serve(&mut window, disks, request);
    window
}

fn select(window: &mut SettingsWindow, downs: usize) {
    let list = window.storage_pane().widgets.list;
    window.tree().set_focus(Some(list), 0);
    for _ in 0..downs {
        press(window, key::DOWN, 0);
    }
}

fn button_at(window: &SettingsWindow, id: WidgetId) -> (i32, i32) {
    let rect = window.widgets().rect(id).unwrap();
    (rect.x + 10, rect.y + 10)
}

fn press_button(window: &mut SettingsWindow, id: WidgetId) -> Option<Request> {
    let (x, y) = button_at(window, id);
    click(window, x, y)
}

#[test]
fn the_storage_pane_comes_after_mouse_and_is_found_by_its_words() {
    assert_eq!(PANES[STORAGE], "Storage");
    assert_eq!(PANES[STORAGE - 1], "Accessibility");
    assert_eq!(PANES[STORAGE + 1], "Date & time");
    for word in [
        "storage",
        "format",
        "fsck",
        "chkdsk",
        "disk utility",
        "repair",
    ] {
        for pane in 0..PANES.len() {
            assert_eq!(pane_matches(pane, word), pane == STORAGE, "{word} {pane}");
        }
    }
    assert!(pane_matches(STORAGE, "disk") && pane_matches(STORAGE, "Disks"));
}

#[test]
fn the_pane_is_laid_out_as_the_design_says() {
    let mut disks = machine();
    let window = opened(&mut disks);
    let pane = window.storage_pane();
    let widgets = pane.widgets;
    let tree = window.widgets();
    let rect = |id: WidgetId| tree.rect(id).unwrap();
    assert_eq!(rect(widgets.list), Rect::new(PANE_X + 16, 64, 208, 96));
    assert_eq!(
        tree.kind(widgets.list).map(|kind| match kind {
            Kind::List(list) => list.shown_items().count(),
            _ => 0,
        }),
        Some(3)
    );
    assert_eq!(tree.selected(widgets.list), Some(0));
    assert_eq!(
        pane.facts(tree),
        [
            Some("EggFS"),
            Some("2.0 MB \u{b7} 640 KB used \u{b7} 1.3 MB free"),
            Some("Mounted at /home"),
            None,
        ]
    );
    assert_eq!(
        (rect(widgets.contents).x, rect(widgets.contents).y),
        (PANE_X + 240, 64)
    );
    assert_eq!(rect(widgets.usage), Rect::new(PANE_X + 240, 148, 208, 8));
    assert!(matches!(
        tree.kind(widgets.usage),
        Some(Kind::Progress { permille: 312 })
    ));
    assert_eq!(rect(widgets.check), Rect::new(PANE_X + 16, 172, 96, 28));
    assert_eq!(rect(widgets.repair), Rect::new(PANE_X + 120, 172, 96, 28));
    assert_eq!(rect(widgets.format), Rect::new(PANE_X + 224, 172, 96, 28));
    assert_eq!(pane.buttons_enabled(tree), [false, false, false]);
    let status = rect(widgets.status);
    assert_eq!((status.x, status.y), (PANE_X + 16, 212));
    let in_use = idle_text(pane.selected(tree).unwrap());
    assert_eq!(pane.status(tree), Some(kept(&in_use).as_str()));
    assert!(matches!(
        tree.kind(widgets.status),
        Some(Kind::Label { colour: ink, .. }) if *ink == colour::TEXT_DIM
    ));
    assert!(pane.report_lines(tree).is_empty());
}

#[test]
fn checking_finds_a_damaged_disk_and_repair_follows_the_check() {
    let mut disks = machine();
    let mut window = opened(&mut disks);
    select(&mut window, 1);
    let widgets = window.storage_pane().widgets;
    let pane = |window: &SettingsWindow| {
        let tree = window.widgets();
        let pane = window.storage_pane();
        (
            pane.buttons_enabled(tree),
            String::from(pane.status(tree).unwrap_or("")),
        )
    };
    let (buttons, status) = pane(&window);
    assert_eq!(buttons, [true, false, true]);
    assert!(status.starts_with("Check reads the disk and changes nothing."));

    let request = press_button(&mut window, widgets.check);
    assert_eq!(request, Some(Request::CheckDisk("/dev/vdb")));
    assert_eq!(
        pane(&window),
        ([false, false, false], "Checking /dev/vdb...".into())
    );
    serve(&mut window, &mut disks, request);
    let (buttons, status) = pane(&window);
    assert!(status.starts_with("Clean. "), "{status}");
    assert_eq!(buttons, [true, false, true]);

    disks
        .disks
        .iter_mut()
        .find(|disk| disk.name == "vdb")
        .unwrap()
        .image[BLOCK_SIZE..2 * BLOCK_SIZE]
        .fill(0);
    let request = press_button(&mut window, widgets.check);
    serve(&mut window, &mut disks, request);
    let (buttons, status) = pane(&window);
    assert!(status.starts_with("Found "), "{status}");
    assert_eq!(buttons, [true, true, true]);
    let lines = window.storage_pane().report_lines(window.widgets());
    assert!(!lines.is_empty());
    assert!(lines[0].starts_with("Repairable \u{b7} "), "{}", lines[0]);

    let request = press_button(&mut window, widgets.repair);
    assert_eq!(request, Some(Request::RepairDisk("/dev/vdb")));
    serve(&mut window, &mut disks, request);
    let (buttons, status) = pane(&window);
    assert!(status.starts_with("Repaired "), "{status}");
    assert_eq!(buttons, [true, false, true]);

    let request = press_button(&mut window, widgets.check);
    serve(&mut window, &mut disks, request);
    assert!(pane(&window).1.starts_with("Clean. "));

    window.tree().set_focus(Some(widgets.list), 0);
    press(&mut window, key::UP, 0);
    assert!(
        window
            .storage_pane()
            .report_lines(window.widgets())
            .is_empty()
    );
}

#[test]
fn formatting_asks_for_the_disk_s_name_and_then_shows_the_new_file_system() {
    let mut disks = machine();
    let mut window = opened(&mut disks);
    select(&mut window, 2);
    let widgets = window.storage_pane().widgets;
    {
        let tree = window.widgets();
        let pane = window.storage_pane();
        assert_eq!(pane.buttons_enabled(tree), [false, false, true]);
        assert!(
            pane.status(tree)
                .unwrap()
                .starts_with("No EggFS file system on the disk")
        );
    }
    assert_eq!(press_button(&mut window, widgets.format), None);
    {
        let tree = window.widgets();
        let pane = window.storage_pane();
        assert!(pane.panel_open());
        assert_eq!(
            tree.rect(widgets.panel),
            Some(Rect::new(PANE_X + 32, 64, 400, 204))
        );
        assert_eq!(
            pane.panel_lines(tree),
            [
                Some("Format /dev/vdc?"),
                Some("Erases everything on it: Blank, 2.0 MB."),
                Some("This cannot be undone. Type vdc to confirm."),
            ]
        );
        assert_eq!(tree.focus(), Some(widgets.label_field));
        assert!(!tree.is_enabled(widgets.format_now));
        assert!(!tree.is_enabled(widgets.list));
    }
    type_text(&mut window, "Photos");
    window.tree().set_focus(Some(widgets.confirm_field), 0);
    type_text(&mut window, "vd");
    assert!(!window.widgets().is_enabled(widgets.format_now));
    type_text(&mut window, "c");
    assert!(window.widgets().is_enabled(widgets.format_now));
    let request = press(&mut window, key::ENTER, 13);
    assert_eq!(request, Some(Request::FormatDisk("/dev/vdc")));
    assert_eq!(
        window.storage_pane().panel_lines(window.widgets())[2],
        Some("Formatting /dev/vdc...")
    );
    serve(&mut window, &mut disks, request);
    let tree = window.widgets();
    let pane = window.storage_pane();
    assert!(!pane.panel_open());
    assert!(!tree.is_shown(widgets.panel));
    assert_eq!(
        pane.status(tree),
        Some("Formatted /dev/vdc as EggFS \"Photos\".")
    );
    assert_eq!(pane.facts(tree)[0], Some("EggFS \"Photos\""));
    assert_eq!(pane.facts(tree)[2], Some("Not mounted"));
    assert_eq!(tree.selected(widgets.list), Some(2));
    assert_eq!(tree.focus(), Some(widgets.format));
    assert_eq!(pane.buttons_enabled(tree), [true, false, true]);
}

#[test]
fn a_long_label_keeps_format_off_and_escape_writes_nothing() {
    let mut disks = machine();
    let mut window = opened(&mut disks);
    select(&mut window, 2);
    let widgets = window.storage_pane().widgets;
    press_button(&mut window, widgets.format);
    type_text(&mut window, &"x".repeat(65));
    assert_eq!(
        window.storage_pane().panel_lines(window.widgets())[2],
        Some("The label is too long: 64 bytes at most.")
    );
    window.tree().set_focus(Some(widgets.confirm_field), 0);
    type_text(&mut window, "vdc");
    assert!(!window.widgets().is_enabled(widgets.format_now));
    window.tree().set_focus(Some(widgets.label_field), 0);
    press(&mut window, key::ESC, 27);
    assert!(window.widgets().is_enabled(widgets.format_now));
    assert_eq!(press(&mut window, key::ESC, 27), None);
    assert!(!window.storage_pane().panel_open());
    assert_eq!(window.widgets().focus(), Some(widgets.format));
    assert_eq!(disks.disk("vdc").writes, 0);
    assert!(window.widgets().is_enabled(widgets.list));
}

#[test]
fn read_only_failed_refused_and_missing_disks_say_why() {
    let mut read_only = MemoryDisks::of(vec![MemoryWholeDisk {
        read_only: true,
        ..MemoryWholeDisk::eggfs("vda", BLOCKS, "Old")
    }]);
    let mut window = opened(&mut read_only);
    {
        let tree = window.widgets();
        let pane = window.storage_pane();
        assert_eq!(pane.buttons_enabled(tree), [true, false, false]);
        assert_eq!(pane.facts(tree)[3], Some("Read-only"));
        assert_eq!(
            pane.status(tree),
            Some("Read-only disk: it can be checked, not repaired or formatted.")
        );
    }
    window.show_check("/dev/vda", CheckMode::ReadOnly, Err(StorageError::Failed));
    assert_eq!(
        window.storage_pane().status(window.widgets()),
        Some("The disk could not be read or written.")
    );
    window.show_check(
        "/dev/vda",
        CheckMode::ReadOnly,
        Err(StorageError::NotAllowed),
    );
    assert_eq!(
        window.storage_pane().status(window.widgets()),
        Some("Not allowed: only root may open disks.")
    );

    let mut none = MemoryDisks::of(Vec::new());
    let window = opened(&mut none);
    let tree = window.widgets();
    let pane = window.storage_pane();
    assert_eq!(pane.status(tree), Some(NOTHING_TO_DO));
    assert_eq!(pane.facts(tree), [None; 4]);
    assert_eq!(pane.buttons_enabled(tree), [false, false, false]);
    assert!(matches!(
        tree.kind(pane.widgets.list),
        Some(Kind::List(list)) if list.empty_text == NO_DISKS && list.shown == 0
    ));
}

#[test]
fn a_long_report_ends_with_how_many_more_the_command_lists() {
    let mut disks = machine();
    let mut window = opened(&mut disks);
    select(&mut window, 1);
    let finding = |inode: u32| Finding {
        problem: Problem::LinkCount,
        inode,
        block: 0,
        outcome: Outcome::Repairable,
    };
    let check = DiskCheck {
        mode: CheckMode::ReadOnly,
        findings: (1..=8).map(finding).collect(),
        summary: None,
        blocks: 262_144,
        free_blocks: 182_000,
    };
    window.show_check("/dev/vdb", CheckMode::ReadOnly, Ok(check));
    let tree = window.widgets();
    let pane = window.storage_pane();
    assert_eq!(
        pane.status(tree),
        Some("Found 8 problems: 8 repairable. 262,144 blocks, 182,000 free.")
    );
    let lines = pane.report_lines(tree);
    assert_eq!(lines.len(), 5);
    assert!(lines[0].starts_with("Repairable \u{b7} inode 1 \u{b7} "));
    assert_eq!(lines[4], "and 4 more: disk check /dev/vdb lists them all.");
    assert_eq!(pane.buttons_enabled(tree), [true, true, true]);
}
