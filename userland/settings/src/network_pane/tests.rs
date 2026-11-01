use super::*;
use crate::facts::{EXAMPLE_ADAPTER, Facts};
use crate::settings_window::{
    ABOUT, DATE_AND_TIME, KEYBOARD, NETWORK, PANES, SettingsWindow, pane_matches, pane_named,
};
use alloc::string::String;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::{Input, MouseButton};

const APPEARANCE: usize = 0;
const WINDOW_RIGHT: i32 = 640 - 16;
const PANE_LEFT: i32 = 176 + 16;

fn adapter_of(line: &str) -> Option<InterfaceStatus> {
    eggos_net::status::adapter(line.as_bytes())
}

fn example() -> Option<InterfaceStatus> {
    adapter_of(EXAMPLE_ADAPTER)
}

fn changed(from: &str, to: &str) -> Option<InterfaceStatus> {
    assert!(EXAMPLE_ADAPTER.contains(from), "{from}");
    adapter_of(&EXAMPLE_ADAPTER.replacen(from, to, 1))
}

const ASKING: &str = "eth0 up 52:54:00:12:34:56 1500 asking - - - - - - 3 900 0 0 3 1026 0\n";

fn click(window: &mut SettingsWindow, x: i32, y: i32) {
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
        window.respond(response, 0);
    }
}

fn opened(adapter: Option<InterfaceStatus>) -> SettingsWindow {
    let mut window = SettingsWindow::new(Facts {
        network_adapter: adapter,
        ..Facts::example()
    });
    window.open_pane(NETWORK);
    assert_eq!(window.pane(), NETWORK);
    window
}

fn widget(window: &SettingsWindow, number: usize) -> WidgetId {
    window.network_pane().widgets()[number - 1]
}

fn text(window: &SettingsWindow, number: usize) -> String {
    String::from(window.widgets().text_of(widget(window, number)).unwrap_or(""))
}

fn shown(window: &SettingsWindow, number: usize) -> bool {
    window.widgets().is_shown(widget(window, number))
}

fn rect(window: &SettingsWindow, number: usize) -> Rect {
    window.widgets().rect(widget(window, number)).unwrap()
}

fn left_at(window: &SettingsWindow, number: usize) -> (i32, i32) {
    let rect = rect(window, number);
    (rect.x, rect.y)
}

fn right_at(window: &SettingsWindow, number: usize) -> (i32, i32) {
    let rect = rect(window, number);
    (rect.right(), rect.y)
}

#[test]
fn network_is_the_second_pane_and_its_sidebar_row_is_76_to_107() {
    assert_eq!(
        PANES,
        [
            "Appearance",
            "Network",
            "Display",
            "Keyboard",
            "Mouse",
            "Accessibility",
            "Storage",
            "Date & time",
            "Agents",
            "Components",
            "About",
        ]
    );
    assert_eq!(pane_named("network"), Some(NETWORK));
    assert_eq!(pane_named("date-and-time"), Some(DATE_AND_TIME));
    for (y, pane) in [(76, NETWORK), (107, NETWORK), (108, NETWORK + 1), (75, APPEARANCE)] {
        let mut window = SettingsWindow::new(Facts::example());
        window.open_pane(ABOUT);
        click(&mut window, 40, y);
        assert_eq!(window.pane(), pane, "{y}");
    }
    let mut window = SettingsWindow::new(Facts::example());
    click(&mut window, 40, 44 + DATE_AND_TIME as i32 * 32);
    assert_eq!(window.pane(), DATE_AND_TIME);
}

#[test]
fn the_search_finds_network_by_its_words_and_by_wi_fi() {
    for word in [
        "ethernet",
        "wi-fi",
        "dns",
        "dhcp",
        "ip address",
        "router",
        "mtu",
        "lease",
    ] {
        for pane in 0..PANES.len() {
            assert_eq!(pane_matches(pane, word), pane == NETWORK, "{word} {pane}");
        }
    }
    let mac: alloc::vec::Vec<usize> = (0..PANES.len())
        .filter(|&pane| pane_matches(pane, "mac"))
        .collect();
    assert_eq!(mac, [APPEARANCE, NETWORK, KEYBOARD]);
}

#[test]
fn the_window_s_tree_is_full_with_the_pane_s_container_and_15_widgets() {
    assert_eq!(WIDGETS, 15);
    let mut window = SettingsWindow::new(Facts::example());
    let spare = window.tree().add(None, Place::fill(), Kind::label("", Token::Text));
    assert_eq!(spare, None);
}

#[test]
fn connected_the_pane_shows_every_row_where_the_design_puts_it() {
    let window = opened(example());
    assert_eq!(left_at(&window, 1), (PANE_LEFT, 16));
    assert_eq!(text(&window, 1), "Network");
    assert_eq!(left_at(&window, 2), (PANE_LEFT, 76));
    assert_eq!(text(&window, 2), "Ethernet");
    assert_eq!(right_at(&window, 3), (WINDOW_RIGHT, 76));
    assert_eq!(text(&window, 3), "Connected");
    assert!(!shown(&window, 4));
    let rows = [
        (5, 116, "IPv4 address", "10.0.2.15/24 \u{b7} mask 255.255.255.0"),
        (7, 156, "Router", "10.0.2.2"),
        (9, 196, "DNS servers", "10.0.2.3"),
        (11, 236, "DHCP server", "10.0.2.2 \u{b7} 23 h 59 min of lease left"),
    ];
    for (name, y, name_text, value_text) in rows {
        assert!(shown(&window, name) && shown(&window, name + 1), "{name_text}");
        assert_eq!(left_at(&window, name), (PANE_LEFT, y), "{name_text}");
        assert_eq!(text(&window, name), name_text);
        assert_eq!(
            right_at(&window, name + 1),
            (WINDOW_RIGHT, y),
            "{name_text}"
        );
        assert_eq!(text(&window, name + 1), value_text);
    }
    let lines = [
        (13, 276, "Hardware address 52:54:00:12:34:56 \u{b7} MTU 1500"),
        (14, 296, "Received 3.4 MB (2,905 packets)"),
        (15, 316, "Sent 1.2 MB (812 packets)"),
    ];
    for (number, y, line) in lines {
        assert!(shown(&window, number), "{line}");
        assert_eq!(left_at(&window, number), (PANE_LEFT, y), "{line}");
        assert_eq!(text(&window, number), line);
    }
}

#[test]
fn each_state_but_connected_says_what_it_means_under_the_value() {
    let states = [
        (
            changed(" up ", " down "),
            "Cable unplugged",
            "Plug in a network cable. EggOS connects by itself.",
        ),
        (
            adapter_of(ASKING),
            "Getting an address...",
            "Asking the network for an address (DHCP).",
        ),
        (
            adapter_of(&ASKING.replacen("asking", "no-address", 1)),
            "No address",
            "No DHCP server answered. EggOS keeps trying.",
        ),
    ];
    for (adapter, value, hint) in states {
        let window = opened(adapter);
        assert_eq!(text(&window, 2), "Ethernet");
        assert_eq!(text(&window, 3), value);
        assert!(shown(&window, 3) && shown(&window, 4), "{value}");
        assert_eq!(left_at(&window, 4), (PANE_LEFT, 116));
        assert_eq!(text(&window, 4), hint);
        for number in 5..=12 {
            assert!(!shown(&window, number), "{value} {number}");
        }
        for number in 13..=15 {
            assert!(shown(&window, number), "{value} {number}");
        }
    }
    let window = opened(None);
    assert_eq!(text(&window, 2), "No network adapter.");
    assert!(!shown(&window, 3));
    assert!(shown(&window, 4));
    assert_eq!(
        text(&window, 4),
        "EggOS found none at boot: there is nothing to set up."
    );
    for number in 5..=15 {
        assert!(!shown(&window, number), "{number}");
    }
}

#[test]
fn servers_errors_and_leases_are_written_as_the_design_says() {
    let three = changed(" 10.0.2.3 ", " 10.0.2.3,10.0.2.4,10.0.2.5 ").unwrap();
    assert_eq!(
        servers_text(&three, ", ").as_str(),
        "10.0.2.3, 10.0.2.4, 10.0.2.5"
    );
    let errors = changed(" 2905 3600000 0 0 ", " 2905 3600000 2 1 ").unwrap();
    assert_eq!(
        received_text(&errors).as_str(),
        "Received 3.4 MB (2,905 packets, 3 errors)"
    );
    assert_eq!(lease_left_text(90_061).as_str(), "1 d 1 h");
    assert_eq!(lease_left_text(86_400).as_str(), "1 d 0 h");
    assert_eq!(lease_left_text(86_342).as_str(), "23 h 59 min");
    assert_eq!(lease_left_text(3599).as_str(), "59 min");
    assert_eq!(lease_left_text(59).as_str(), "59 s");
    let forever = changed(" 86342 86400 ", " forever forever ").unwrap();
    assert_eq!(
        dhcp_text(&forever).as_str(),
        "10.0.2.2 \u{b7} lease does not end"
    );
    let window = opened(Some(three));
    assert_eq!(text(&window, 10), "10.0.2.3, 10.0.2.4, 10.0.2.5");
}

#[test]
fn a_tick_changes_only_what_changed_and_damages_only_its_rect() {
    let mut window = opened(adapter_of(ASKING));
    assert_eq!(text(&window, 3), "Getting an address...");
    assert!(shown(&window, 4) && !shown(&window, 5));
    window.tree().take_damage();
    window.show_network_adapter(example());
    assert_eq!(text(&window, 3), "Connected");
    assert!(!shown(&window, 4));
    for number in 5..=12 {
        assert!(shown(&window, number), "{number}");
    }
    assert_eq!(text(&window, 6), "10.0.2.15/24 \u{b7} mask 255.255.255.0");
    window.tree().take_damage();
    window.show_network_adapter(example());
    assert!(window.tree().take_damage().is_empty());
    window.show_network_adapter(changed(" 2905 ", " 2906 "));
    assert_eq!(text(&window, 14), "Received 3.4 MB (2,906 packets)");
    let received = rect(&window, 14);
    let damage = window.tree().take_damage();
    assert!(!damage.is_empty());
    for damaged in damage.rects() {
        assert!(
            damaged.y >= received.y - 8 && damaged.bottom() <= received.bottom() + 8,
            "{damaged:?} {received:?}"
        );
    }
}

#[test]
fn nothing_is_shown_again_while_another_pane_shows_but_the_facts_follow() {
    let mut window = SettingsWindow::new(Facts::example());
    window.show_network_adapter(changed(" up ", " down "));
    assert_eq!(window.pane(), APPEARANCE);
    assert_eq!(
        window.facts().network_adapter.map(|adapter| adapter.link_up),
        Some(false)
    );
    window.open_pane(NETWORK);
    assert_eq!(text(&window, 3), "Cable unplugged");
}
