use super::*;
use crate::facts::Facts;
use crate::settings_window::{AGENTS, COMPONENTS, PANES, STORAGE, SettingsWindow, pane_matches};
use eggos_components::index::{File, Index};
use eggos_components::manager::Source;
use eggos_toolkit::elements::{Action, Outcome};
use eggos_toolkit::elements::{MAX_TREE_BYTES, Role, TreeWriter, elements};
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::{Input, Modifiers};
use eggos_toolkit::tree::element_of;

const DESKTOP_BYTES: u64 = 2_726_298;
const BUILTIN_BYTES: u64 = 1_153_434;
const WINDOWS_BYTES: u64 = 1_887_437;
const MAC_BYTES: u64 = 1_468_007;
const COMPONENTS_ROW_Y: i32 = 44 + 9 * 32 + 10;

fn component(
    name: &str,
    title: &str,
    summary: &str,
    size: u64,
    needs: &[&str],
    on_remove: &str,
) -> Component {
    Component {
        name: String::from(name),
        title: String::from(title),
        summary: String::from(summary),
        on_remove: String::from(on_remove),
        needs: needs.iter().map(|need| String::from(*need)).collect(),
        files: alloc::vec![File {
            path: format!("usr/lib/eggos/{name}/payload"),
            mode: 0o644,
            offset: 1024,
            length: size,
            sha256: [0; 32],
        }],
    }
}

fn four() -> Vec<Component> {
    alloc::vec![
        component(
            "desktop",
            "Desktop",
            "The window server, the shell and Settings",
            DESKTOP_BYTES,
            &[],
            "Settings and every window go with it.",
        ),
        component(
            "builtin-apps",
            "Built-in apps",
            "Files, Text Editor, Downloads, Task Manager",
            BUILTIN_BYTES,
            &["desktop"],
            "They leave the launcher; your files stay.",
        ),
        component(
            "windows-apps",
            "Windows apps",
            "Runs Windows programs natively, no emulation",
            WINDOWS_BYTES,
            &[],
            "Installed Windows programs stay in Apps and need it to run.",
        ),
        component(
            "mac-apps",
            "Mac apps",
            "Runs Mac programs natively, no emulation",
            MAC_BYTES,
            &[],
            "Installed Mac programs stay in Apps and need it to run.",
        ),
    ]
}

fn store() -> Store {
    Store {
        source: Source::Disk("/dev/vdc"),
        path: String::from("/dev/vdc"),
        index: Index {
            version: String::from("0.3.0"),
            arch: String::from("x86_64"),
            components: four(),
        },
    }
}

fn read_of(store: Option<Store>, installed: &[&str], runs_this_window: bool) -> ComponentsRead {
    let offered: Vec<Component> = match &store {
        Some(store) => store.index.components.clone(),
        None => four()
            .into_iter()
            .filter(|component| installed.contains(&component.name.as_str()))
            .collect(),
    };
    let rows = offered
        .iter()
        .map(|component| Row {
            component: component.clone(),
            state: match installed.contains(&component.name.as_str()) {
                true => "installed",
                false => "not-installed",
            },
            needed_by: offered
                .iter()
                .filter(|other| {
                    installed.contains(&other.name.as_str()) && other.needs(&component.name)
                })
                .map(|other| other.title.clone())
                .collect(),
            update: None,
            job: None,
        })
        .collect();
    ComponentsRead {
        store,
        rows,
        runs_this_window: (runs_this_window && installed.contains(&"desktop"))
            .then(|| String::from("desktop")),
        recorded_edition: None,
        daily: true,
        network_up: true,
        ..ComponentsRead::default()
    }
}

fn read(installed: &[&str]) -> ComponentsRead {
    read_of(Some(store()), installed, true)
}

fn opened(read: ComponentsRead) -> SettingsWindow {
    let mut window = SettingsWindow::new(Facts::example());
    assert_eq!(window.open_pane(COMPONENTS), Some(Request::ReadComponents));
    window.show_components(read);
    window
}

fn widgets(window: &SettingsWindow) -> Widgets {
    window.components_pane().widgets
}

fn rect(window: &SettingsWindow, id: WidgetId) -> Rect {
    window.widgets().rect(id).unwrap()
}

fn press(window: &mut SettingsWindow, code: u16) -> Option<Request> {
    let response = window.tree().handle(
        Input::Key {
            code,
            pressed: true,
            modifiers: Modifiers::default(),
            character: 0,
        },
        0,
    );
    window.respond(response, 0)
}

fn activate(window: &mut SettingsWindow, id: WidgetId) -> Option<Request> {
    window.respond(Response::Activated(id), 0)
}

fn row_button(window: &SettingsWindow, row: usize) -> WidgetId {
    widgets(window).rows[row].button
}

fn activate_row(window: &mut SettingsWindow, row: usize) -> Option<Request> {
    let button = row_button(window, row);
    activate(window, button)
}

fn rows(window: &SettingsWindow) -> Vec<[String; 3]> {
    window
        .components_pane()
        .rows(window.widgets())
        .into_iter()
        .map(|row| row.map(String::from))
        .collect()
}

fn notes(window: &SettingsWindow) -> Vec<String> {
    window
        .components_pane()
        .note_lines(window.widgets())
        .into_iter()
        .map(String::from)
        .collect()
}

fn row_texts(title: &str, state: &str, button: &str) -> [String; 3] {
    [title, state, button].map(String::from)
}

#[test]
fn components_is_found_by_its_words_and_its_row_is_the_tenth_shown() {
    assert_eq!(PANES[COMPONENTS], "Components");
    for word in [
        "components",
        "uninstall",
        "optional features",
        "programs and features",
        "store",
        "layer",
    ] {
        for pane in 0..PANES.len() {
            assert_eq!(
                pane_matches(pane, word),
                pane == COMPONENTS,
                "{word} {pane}"
            );
        }
    }
    for (word, other) in [("install", AGENTS), ("space", STORAGE)] {
        for pane in 0..PANES.len() {
            assert_eq!(
                pane_matches(pane, word),
                pane == COMPONENTS || pane == other,
                "{word} {pane}"
            );
        }
    }
    let mut window = SettingsWindow::new(Facts::example());
    for input in [
        Input::PointerMove {
            x: 40,
            y: COMPONENTS_ROW_Y,
        },
        Input::Button {
            x: 40,
            y: COMPONENTS_ROW_Y,
            button: eggos_toolkit::input::MouseButton::Left,
            pressed: true,
        },
        Input::Button {
            x: 40,
            y: COMPONENTS_ROW_Y,
            button: eggos_toolkit::input::MouseButton::Left,
            pressed: false,
        },
    ] {
        let response = window.tree().handle(input, 0);
        window.respond(response, 0);
    }
    assert_eq!(window.pane(), COMPONENTS);
}

#[test]
fn the_pane_is_laid_out_as_the_design_says() {
    let window = opened(read(&["desktop", "builtin-apps"]));
    let ids = widgets(&window);
    assert_eq!(
        window.components_pane().store_lines(window.widgets()),
        [
            "Component store: EggOS 0.3.0, x86_64",
            "eggos-0.3.0-x86_64-components.img on /dev/vdc",
            "2 of 4 installed \u{b7} 3.7 MB on disk",
        ]
    );
    for (id, y) in ids.store.iter().zip([56, 76, 96]) {
        let at = rect(&window, *id);
        assert_eq!((at.x, at.y), (192, y));
    }
    let row = ids.rows[0];
    assert_eq!(rect(&window, row.panel), Rect::new(192, 252, 432, 48));
    let title = rect(&window, row.title);
    assert_eq!((title.x, title.y), (216, 258));
    let state = rect(&window, row.state);
    assert_eq!((state.right(), state.y), (512, 258));
    let summary = rect(&window, row.summary);
    assert_eq!((summary.x, summary.y), (216, 278));
    assert_eq!(rect(&window, row.button), Rect::new(528, 134, 96, 28));
    for (index, y) in [(1, 172), (2, 220), (3, 268)] {
        assert_eq!(rect(&window, ids.rows[index].panel).y, y);
    }
    assert_eq!(
        rows(&window),
        [
            row_texts("Desktop", "Installed \u{b7} 2.6 MB", "Remove"),
            row_texts("Built-in apps", "Installed \u{b7} 1.1 MB", "Remove"),
            row_texts("Windows apps", "Not installed \u{b7} 1.8 MB", "Install"),
            row_texts("Mac apps", "Not installed \u{b7} 1.4 MB", "Install"),
        ]
    );
    assert!(!window.widgets().is_shown(ids.rows[4].panel));
    assert!(!window.widgets().is_shown(ids.rows[5].panel));
    assert!(notes(&window).is_empty());
    assert!(!window.widgets().is_shown(ids.bar));
}

#[test]
fn installing_shows_the_bar_under_the_row_and_moves_the_rows_below() {
    let mut window = opened(read(&["desktop", "builtin-apps"]));
    press(&mut window, key::DOWN);
    press(&mut window, key::TAB);
    assert_eq!(window.widgets().focus(), Some(row_button(&window, 0)));
    press(&mut window, key::DOWN);
    assert_eq!(window.widgets().focus(), Some(row_button(&window, 1)));
    press(&mut window, key::END);
    assert_eq!(window.widgets().focus(), Some(row_button(&window, 3)));
    press(&mut window, key::UP);
    assert_eq!(window.widgets().focus(), Some(row_button(&window, 2)));
    assert_eq!(
        press(&mut window, key::ENTER),
        Some(Request::StartComponentJob)
    );
    assert_eq!(window.components_pane().to_install(), Some("windows-apps"));
    assert_eq!(
        rows(&window)[2],
        row_texts("Windows apps", "Installing 0 %", "Cancel")
    );
    let ids = widgets(&window);
    assert_eq!(rect(&window, ids.bar), Rect::new(216, 276, 408, 8));
    assert_eq!(rect(&window, ids.first).y, 292);
    assert_eq!(notes(&window), ["0 B of 1.8 MB"]);
    assert_eq!(rect(&window, ids.rows[3].panel).y, 308);
    for row in [0, 1, 3] {
        assert!(!window.widgets().is_enabled(row_button(&window, row)));
        let tip = widgets(&window).rows[row].tip;
        assert_eq!(
            window.widgets().text_of(tip),
            Some(messages::OTHER_INSTALLING_REASON)
        );
        assert!(window.widgets().is_shown(tip));
    }
    let next = window.show_installed(Ok(()), read(&["desktop", "builtin-apps", "windows-apps"]));
    assert_eq!(next, None);
    assert_eq!(
        rows(&window)[2],
        row_texts("Windows apps", "Installed \u{b7} 1.8 MB", "Remove")
    );
    assert_eq!(rect(&window, ids.rows[3].panel).y, 268);
    assert!(!window.widgets().is_shown(widgets(&window).rows[3].tip));
    assert_eq!(
        window.components_pane().store_lines(window.widgets())[2],
        "3 of 4 installed \u{b7} 5.5 MB on disk"
    );
}

#[test]
fn removing_is_refused_or_confirmed_as_the_cli_answers() {
    let mut window = opened(read(&["desktop", "builtin-apps"]));
    assert_eq!(activate_row(&mut window, 0), None);
    assert_eq!(
        notes(&window),
        [
            "In use: the desktop is running, and this window is part of it.",
            "Boot with run-eggos.sh --serial and remove it from there.",
        ]
    );

    let mut window = opened(read_of(Some(store()), &["desktop", "builtin-apps"], false));
    assert_eq!(
        activate_row(&mut window, 0),
        Some(Request::CheckComponentRemove)
    );
    assert_eq!(window.components_pane().to_check(), Some("desktop"));
    window.show_remove_check(
        Err(Refusal::NeededBy(alloc::vec![String::from(
            "Built-in apps"
        )])),
        0,
    );
    assert_eq!(
        notes(&window),
        ["Needed by Built-in apps: remove that first."]
    );

    activate_row(&mut window, 1);
    window.show_remove_check(
        Err(Refusal::InUse {
            names: alloc::vec![String::from("Text Editor")],
            count: 1,
        }),
        0,
    );
    assert_eq!(
        notes(&window),
        ["In use: Text Editor is running. Quit it, then remove."]
    );

    activate_row(&mut window, 1);
    window.show_remove_check(Ok(Some(four()[1].clone())), 0);
    let ids = widgets(&window);
    assert_eq!(
        notes(&window),
        [
            "Remove Built-in apps (1.1 MB)?",
            "They leave the launcher; your files stay.",
        ]
    );
    let first = rect(&window, ids.first);
    assert_eq!((first.x, first.y), (216, 224));
    assert_eq!(rect(&window, ids.second).y, 244);
    assert_eq!(rect(&window, ids.cancel), Rect::new(392, 268, 96, 28));
    assert_eq!(rect(&window, ids.confirm), Rect::new(496, 268, 128, 28));
    assert_eq!(window.widgets().text_of(ids.confirm), Some("Remove"));
    assert_eq!(window.widgets().focus(), Some(ids.confirm));
    assert_eq!(rect(&window, ids.rows[2].panel).y, 220 + 84);
    assert_eq!(rect(&window, ids.rows[3].panel).y, 268 + 84);

    assert_eq!(press(&mut window, key::ESC), None);
    assert!(notes(&window).is_empty());
    assert_eq!(window.widgets().focus(), Some(row_button(&window, 1)));
    assert_eq!(rect(&window, ids.rows[2].panel).y, 220);

    activate_row(&mut window, 1);
    window.show_remove_check(Ok(Some(four()[1].clone())), 0);
    assert_eq!(
        press(&mut window, key::ENTER),
        Some(Request::RemoveComponent)
    );
    assert_eq!(window.components_pane().to_remove(), Some("builtin-apps"));
    assert_eq!(rows(&window)[1][1], "Removing...");
    assert!(!window.widgets().is_enabled(row_button(&window, 1)));
    window.show_removed(Ok(()), read_of(Some(store()), &["desktop"], false));
    assert_eq!(
        rows(&window)[1],
        row_texts("Built-in apps", "Not installed \u{b7} 1.1 MB", "Install")
    );
    assert!(notes(&window).is_empty());
}

#[test]
fn a_remove_refused_once_confirmed_says_why_and_keeps_the_component() {
    let installed = ["desktop", "builtin-apps"];
    let mut window = opened(read_of(Some(store()), &installed, false));
    activate_row(&mut window, 1);
    window.show_remove_check(Ok(Some(four()[1].clone())), 0);
    assert_eq!(
        press(&mut window, key::ENTER),
        Some(Request::RemoveComponent)
    );
    window.show_removed(
        Err(RemoveError::Refused(Refusal::InUse {
            names: alloc::vec![String::from("Text Editor")],
            count: 1,
        })),
        read_of(Some(store()), &installed, false),
    );
    assert_eq!(
        notes(&window),
        ["In use: Text Editor is running. Quit it, then remove."]
    );
    assert_eq!(rows(&window)[1][2], "Remove");
    assert_eq!(window.components_pane().to_remove(), None);
}

#[test]
fn a_missing_dependency_is_confirmed_then_installed_first() {
    let mut window = opened(read(&[]));
    assert_eq!(activate_row(&mut window, 1), None);
    let ids = widgets(&window);
    assert_eq!(
        notes(&window),
        [
            "Installs Desktop (2.6 MB) too: Built-in apps need it.",
            "3.7 MB in all.",
        ]
    );
    assert_eq!(window.widgets().text_of(ids.confirm), Some("Install both"));
    assert!(!window.widgets().is_person_only(ids.confirm));
    assert_eq!(window.widgets().focus(), Some(ids.confirm));
    assert_eq!(
        press(&mut window, key::ENTER),
        Some(Request::StartComponentJob)
    );
    assert_eq!(window.components_pane().to_install(), Some("desktop"));
    assert_eq!(rows(&window)[0][1], "Installing 0 %");
    assert_eq!(rect(&window, ids.bar).y, 252 + 48 + 8);
    let next = window.show_installed(Ok(()), read_of(Some(store()), &["desktop"], false));
    assert_eq!(next, Some(Request::StartComponentJob));
    assert_eq!(window.components_pane().to_install(), Some("builtin-apps"));
    assert_eq!(rows(&window)[1][1], "Installing 0 %");
    assert_eq!(rect(&window, ids.bar).y, 252 + 2 * 48 + 8);
    let done = window.show_installed(
        Ok(()),
        read_of(Some(store()), &["desktop", "builtin-apps"], false),
    );
    assert_eq!(done, None);
    assert_eq!(rows(&window)[1][2], "Remove");
}

#[test]
fn a_failed_or_cancelled_install_says_why_and_installs_nothing() {
    let mut window = opened(read(&["desktop", "builtin-apps"]));
    activate_row(&mut window, 2);
    window.show_installed(
        Err(Failure::Damaged(String::from("windows-apps.component"))),
        read(&["desktop", "builtin-apps"]),
    );
    assert_eq!(
        rows(&window)[2],
        row_texts("Windows apps", "Failed", "Install")
    );
    assert_eq!(
        notes(&window),
        [
            "The store's copy of windows-apps.component is damaged.",
            "Nothing was installed.",
        ]
    );

    activate_row(&mut window, 2);
    let no_space = Failure::NoSpace {
        needed: WINDOWS_BYTES,
        free: 629_146,
    };
    window.show_installed(Err(no_space.clone()), read(&["desktop", "builtin-apps"]));
    let (line, _, _) = messages::install_failure(&no_space, &four()[2]);
    assert_eq!(notes(&window)[0], line);
    assert!(line.starts_with("Not enough space: 1.8 MB needed, "));

    activate_row(&mut window, 2);
    window.show_installed(Err(Failure::Detached), read(&["desktop", "builtin-apps"]));
    assert_eq!(
        notes(&window)[0],
        "The store was detached before the copy finished."
    );

    activate_row(&mut window, 2);
    assert_eq!(
        activate_row(&mut window, 2),
        None,
        "Cancel asks for nothing more"
    );
    assert!(window.components_pane().cancelled());
    window.show_installed(Err(Failure::Cancelled), read(&["desktop", "builtin-apps"]));
    assert_eq!(notes(&window), ["Cancelled.", "Nothing was installed."]);
    assert_eq!(
        rows(&window)[2],
        row_texts("Windows apps", "Not installed \u{b7} 1.8 MB", "Install")
    );
}

#[test]
fn without_a_store_only_installed_components_show_and_none_installs() {
    let mut window = opened(read_of(None, &["desktop"], false));
    assert_eq!(
        window.components_pane().store_lines(window.widgets()),
        messages::NO_STORE_LINES
    );
    assert_eq!(
        rows(&window),
        [row_texts("Desktop", "Installed \u{b7} 2.6 MB", "Remove")]
    );
    window.show_components(read_of(None, &[], false));
    assert!(rows(&window).is_empty());
    window.show_components(read(&["desktop"]));
    assert_eq!(rows(&window).len(), 4);
    assert_eq!(rows(&window)[3][2], "Install");
}

fn described(window: &SettingsWindow) -> Vec<(u32, Role, String, String)> {
    let mut bytes = alloc::vec![0u8; MAX_TREE_BYTES];
    let mut writer = TreeWriter::new(&mut bytes);
    window.widgets().describe(&mut writer);
    elements(writer.bytes())
        .map(|element| {
            let element = element.unwrap();
            (
                element.id,
                element.role,
                String::from_utf8(element.name.to_vec()).unwrap(),
                String::from_utf8(element.value.to_vec()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn an_agent_reads_the_rows_and_may_not_say_yes_to_a_remove() {
    let mut window = opened(read_of(Some(store()), &["builtin-apps", "desktop"], false));
    let ids = widgets(&window);
    let tree = described(&window);
    let find = |id: WidgetId| tree.iter().find(|element| element.0 == element_of(id));
    let group = find(ids.rows[2].panel).unwrap();
    assert_eq!(
        (group.1, group.2.as_str(), group.3.as_str()),
        (
            Role::Group,
            "Windows apps",
            "windows-apps not-installed 1.8 MB"
        )
    );
    let install = find(ids.rows[2].button).unwrap();
    assert_eq!(
        (install.1, install.2.as_str()),
        (Role::Button, "Install Windows apps")
    );

    activate_row(&mut window, 1);
    window.show_remove_check(Ok(Some(four()[1].clone())), 0);
    assert_eq!(
        window
            .tree()
            .act(element_of(ids.confirm), Action::Invoke, "", 0)
            .0,
        Outcome::NotSupported
    );

    let mut window = opened(read(&[]));
    activate_row(&mut window, 1);
    let (outcome, response) = window
        .tree()
        .act(element_of(ids.confirm), Action::Invoke, "", 0);
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(window.respond(response, 0), Some(Request::StartComponentJob));
}

#[test]
fn every_line_of_the_pane_fits_a_label() {
    let lines = [
        messages::desktop_in_use(true).0,
        String::from(messages::BOOT_TO_REMOVE),
        messages::in_use(&[], 3),
        messages::in_use(&[String::from("Notepad"), String::from("Paint")], 2),
        messages::install_failure(&Failure::Detached, &four()[2]).0,
        messages::install_failure(
            &Failure::Damaged(String::from("builtin-apps.component")),
            &four()[1],
        )
        .0,
        messages::store_place(&store()),
        messages::count_line(4, 4, 999 << 20),
        messages::not_installed_state(999 << 20),
    ];
    for line in lines
        .iter()
        .chain(messages::NO_STORE_LINES.map(String::from).iter())
    {
        assert!(line.len() <= 64, "{line}");
    }
    let two_long_names = messages::in_use(&[String::from("Files"), String::from("Text Editor")], 2);
    assert_eq!(
        shortened(&two_long_names).as_str(),
        "In use: Files and Text Editor are running. Quit them, then re..."
    );
    let long = "Could not install Windows apps: a reason far longer than a label holds.";
    let cut = shortened(long);
    assert!(cut.as_str().ends_with("..."));
    assert_eq!(cut.len(), 64);
}

#[test]
fn the_last_check_is_shown_in_the_chosen_time_zone() {
    let checked_utc = 1_767_000_000 - 1_767_000_000 % 86_400 + 8 * 3_600 + 14 * 60;
    let mut fresh = read(&["desktop"]);
    fresh.checked = Some(Checked {
        at: checked_utc,
        source: String::from("release"),
        items: Vec::new(),
    });
    fresh.now_s = Some(checked_utc + 3_600);
    let mut window = opened(fresh);
    let line = widgets(&window).updates_line;
    assert_eq!(
        window.widgets().text_of(line),
        Some("Up to date \u{b7} checked today at 08:14")
    );
    let tokyo = crate::facts::zone_index("Asia/Tokyo").unwrap();
    window.time_zone_saved(tokyo, Ok(()));
    assert_eq!(
        window.widgets().text_of(line),
        Some("Up to date \u{b7} checked today at 17:14")
    );
}

#[test]
fn on_a_phone_the_pane_is_one_column_of_full_width_blocks() {
    let mut window = opened(read(&["desktop", "builtin-apps"]));
    window.tree().resize(360, 740);
    window.set_form_factor(crate::settings_window::SettingsForm::Phone, 360);
    window.open_pane(COMPONENTS);
    let ids = widgets(&window);
    let pane = rect(&window, ids.pane);
    let at = |id: WidgetId| {
        let r = rect(&window, id);
        Rect::new(r.x - pane.x, r.y - pane.y, r.width, r.height)
    };
    assert_eq!(at(ids.updates_band), Rect::new(16, 124, 328, 132));
    assert_eq!(at(ids.edition_band), Rect::new(16, 264, 328, 116));
    assert_eq!(at(ids.rows[0].panel), Rect::new(16, 388, 328, 128));
    assert_eq!(at(ids.rows[1].panel).y, 516);
    let button = rect(&window, ids.rows[0].button);
    let panel = rect(&window, ids.rows[0].panel);
    assert_eq!(
        Rect::new(button.x - panel.x, button.y - panel.y, button.width, button.height),
        Rect::new(8, 72, 312, 48)
    );
}

#[test]
fn on_a_tablet_the_desktop_layout_takes_forty_pixel_controls() {
    let mut window = opened(read(&["desktop"]));
    window.tree().resize(1024, 660);
    window.set_form_factor(crate::settings_window::SettingsForm::Tablet, 1024);
    let ids = widgets(&window);
    let pane = rect(&window, ids.pane);
    assert_eq!(rect(&window, ids.rows[0].panel).y - pane.y, 268);
    assert_eq!(rect(&window, ids.rows[0].panel).height, 56);
    assert_eq!(rect(&window, ids.updates_button).height, 40);
}
