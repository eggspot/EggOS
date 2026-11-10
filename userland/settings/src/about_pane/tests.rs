use super::*;
use alloc::string::String;
use crate::facts::Facts;
use crate::settings_window::{ABOUT, DATE_AND_TIME, PANES, SettingsWindow, pane_matches};
use eggos_toolkit::input::{Input, MouseButton};

const ABOUT_ROW_Y: i32 = 44 + (ABOUT as i32 - 1) * 32 + 10;
const REFUSED: &[u8] =
    b"5\t-\tdevicetree\tbus\tvirtio_mmio@a000400\tvirtio,mmio\tvirtio-mmio\trefused\t-\ta legacy device\n";

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

fn opened(read: Result<&[u8], Errno>) -> SettingsWindow {
    let mut window = SettingsWindow::new(Facts::example());
    let request = click(&mut window, 40, ABOUT_ROW_Y);
    assert_eq!(window.pane(), ABOUT);
    assert_eq!(request, Some(Request::ReadDevices));
    window.show_devices(read);
    window
}

fn table_of(window: &SettingsWindow) -> Table {
    match window.widgets().kind(window.about_pane().table()) {
        Some(Kind::Table(table)) => *table,
        _ => panic!("the devices table is a table"),
    }
}

fn row_words(window: &SettingsWindow, row: usize) -> [String; 3] {
    let table = table_of(window);
    [0, 1, 2].map(|column| String::from(table.cell_text(row, column).as_str()))
}

fn toggle(window: &mut SettingsWindow) {
    let rect = window
        .widgets()
        .rect(window.about_pane().show_all_box())
        .unwrap();
    click(window, rect.x + 8, rect.y + 8);
}


#[test]
fn about_shows_the_devices_people_recognise_in_the_lists_order_and_words() {
    let window = opened(Ok(eggos_device_list::EXAMPLE));
    assert_eq!(window.about_pane().shown(), [4, 0, 1]);
    let table = table_of(&window);
    assert_eq!(table.rows, 3);
    assert_eq!(table.selected, 0);
    assert_eq!(
        row_words(&window, 0),
        ["Input", "virtio input", "virtio-input"].map(String::from)
    );
    assert_eq!(
        row_words(&window, 1),
        ["Serial port", "pl011@9000000 · arm,pl011", "No driver"].map(String::from)
    );
    assert_eq!(
        row_words(&window, 2),
        ["Clock", "pl031@9010000 · arm,pl031", "pl031"].map(String::from)
    );
    let tree = window.widgets();
    let show_all = window.about_pane().show_all_box();
    assert_eq!(tree.text_of(show_all), Some("Show all 5 devices"));
    assert!(tree.is_enabled(show_all));
    assert_eq!(tree.selected(show_all), Some(0));
}

#[test]
fn the_section_is_laid_out_under_the_facts_and_fills_the_pane() {
    let window = opened(Ok(eggos_device_list::EXAMPLE));
    let tree = window.widgets();
    let table = tree.rect(window.about_pane().table()).unwrap();
    assert_eq!((table.x, table.y, table.width), (192, 212, 432));
    assert_eq!(table.y + table.height, 440 - PADDING);
    let show_all = tree.rect(window.about_pane().show_all_box()).unwrap();
    assert_eq!((show_all.x + show_all.width, show_all.y), (624, 176));
    assert_eq!(show_all.height, CONTROL_HEIGHT);
}

#[test]
fn ticking_the_box_shows_every_device_and_keeps_the_selected_one() {
    let text = [eggos_device_list::EXAMPLE, REFUSED].concat();
    let mut window = opened(Ok(&text[..]));
    assert_eq!(window.about_pane().shown(), [4, 0, 1, 5]);
    window.tree().set_selected(window.about_pane().table(), 2);
    toggle(&mut window);
    assert_eq!(window.about_pane().shown(), [4, 0, 1, 2, 3, 5]);
    assert_eq!(table_of(&window).selected, 2);
    assert_eq!(
        row_words(&window, 5)[2],
        String::from("Refused: a legacy device")
    );
    window.tree().set_selected(window.about_pane().table(), 3);
    toggle(&mut window);
    assert_eq!(window.about_pane().shown(), [4, 0, 1, 5]);
    assert_eq!(table_of(&window).selected, 0);
}

#[test]
fn a_file_that_cannot_be_read_says_so_and_disables_the_box() {
    let window = opened(Err(Errno::ENOENT));
    let table = table_of(&window);
    assert_eq!((table.rows, table.empty_text), (0, UNREADABLE));
    let tree = window.widgets();
    let show_all = window.about_pane().show_all_box();
    assert_eq!(tree.text_of(show_all), Some(SHOW_ALL));
    assert!(!tree.is_enabled(show_all));
}

#[test]
fn a_default_view_that_shows_everything_disables_the_box() {
    let only = b"0\t-\tvirtio\tdisplay\tgpu\tvirtio gpu\tvirtio-gpu\tbound\tinterrupts\t-\n";
    let window = opened(Ok(&only[..]));
    assert_eq!(window.about_pane().shown(), [0]);
    assert!(!window.widgets().is_enabled(window.about_pane().show_all_box()));
    let window = opened(Ok(&b""[..]));
    assert_eq!(table_of(&window).empty_text, NONE_BY_DEFAULT);
}

#[test]
fn about_is_found_by_the_names_other_systems_give_the_device_list() {
    for word in ["device manager", "system information", "driver", "virtio"] {
        for pane in 0..PANES.len() {
            assert_eq!(pane_matches(pane, word), pane == ABOUT, "{word} {pane}");
        }
    }
    assert!(pane_matches(ABOUT, "hardware") && pane_matches(DATE_AND_TIME, "hardware"));
}
