use super::protocol::DiscardEvents;
use super::shell::PanelItem;
use super::tests::{at, fnv, painted};
use super::*;
use crate::input::keymap::Modifiers;
use alloc::vec;
use alloc::vec::Vec;

const QEMU_ADDRESS: [u8; 4] = [10, 0, 2, 15];
const CONNECTED: NetworkState = NetworkState::Connected(QEMU_ADDRESS);
const TOP_BAR_PROFILES: [Profile; 2] = [Profile::Macos, Profile::Linux];
const TOP_BAR_POINT: (u32, u32) = (1230, 14);
const TASKBAR_POINT: (u32, u32) = (1116, 780);

const CONNECTED_MAP: [&str; 16] = [
    "XXXXXXXXX.......",
    "X.......X.......",
    "X.......X.......",
    "X.......X.......",
    "X.......X.......",
    "XXXXXXXXX.......",
    "....X...........",
    "....X...........",
    "....XXXXXXXX....",
    "...........X....",
    ".......XXXXXXXXX",
    ".......X.......X",
    ".......X.......X",
    ".......X.......X",
    ".......X.......X",
    ".......XXXXXXXXX",
];

const GETTING_ADDRESS_MAP: [&str; 16] = [
    "XXXXXXXXX.......",
    "X.......X.......",
    "X.......X.......",
    "X.......X.......",
    "X.......X.......",
    "XXXXXXXXX.......",
    "....X...........",
    "....X...........",
    "....XXXXXXXX....",
    "...........X....",
    ".......ddddddddd",
    ".......d.......d",
    ".......d.a.a.a.d",
    ".......d.......d",
    ".......d.......d",
    ".......ddddddddd",
];

const NO_ADDRESS_MAP: [&str; 16] = [
    "XXXXXXXXX.......",
    "X.......X.......",
    "X.......X.......",
    "X.......X.......",
    "X.......X.......",
    "XXXXXXXXX.......",
    "....X...........",
    "....X...........",
    "....XXXXXXXX....",
    "...........X....",
    ".......ddddddddd",
    ".......d...a...d",
    ".......d...a...d",
    ".......d.......d",
    ".......d...a...d",
    ".......ddddddddd",
];

const CABLE_UNPLUGGED_MAP: [&str; 16] = [
    "XXXXXXXXX.......",
    "X.......X.......",
    "X.......X.......",
    "X.......X.......",
    "X.......X.......",
    "XXXXXXXXX.......",
    "....X...........",
    "....X...........",
    "....XX..........",
    "................",
    ".......ddddddddd",
    ".......d.......d",
    ".......d.......d",
    ".......d.......d",
    ".......d.......d",
    ".......ddddddddd",
];

const STATES: [(NetworkState, &[&str; 16]); 4] = [
    (CONNECTED, &CONNECTED_MAP),
    (NetworkState::GettingAddress, &GETTING_ADDRESS_MAP),
    (NetworkState::NoAddress, &NO_ADDRESS_MAP),
    (NetworkState::CableUnplugged, &CABLE_UNPLUGGED_MAP),
];

const US: KeyboardLayoutChoice = KeyboardLayoutChoice {
    number: 0,
    name: "us",
    title: "English (US)",
    code: "US",
};
const GERMAN: KeyboardLayoutChoice = KeyboardLayoutChoice {
    number: 11,
    name: "de",
    title: "German",
    code: "DE",
};

fn new_desktop(profile: Profile) -> Desktop {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    desktop.set_profile(profile, &mut Damage::new(), &mut DiscardEvents);
    desktop
}

fn with_network(profile: Profile, state: NetworkState) -> Desktop {
    let mut desktop = new_desktop(profile);
    desktop.set_network(Some(state), &mut Damage::new());
    desktop
}

fn ink(cell: u8) -> u32 {
    match cell {
        b'X' => colour::TEXT,
        b'd' => colour::TEXT_DIM,
        b'a' => colour::ACCENT,
        _ => colour::BAR,
    }
}

fn glyph_pixels(desktop: &Desktop, scale: u32) -> Vec<u32> {
    let (x, y) = desktop.network_glyph_at().unwrap();
    let pixels = painted(desktop);
    let side = NETWORK_GLYPH_SIDE * scale;
    (0..side)
        .flat_map(|row| (0..side).map(move |column| (column, row)))
        .map(|(column, row)| at(&pixels, x + column, y + row))
        .collect()
}

fn assert_glyph(desktop: &Desktop, map: &[&str; 16], scale: u32) {
    let drawn = glyph_pixels(desktop, scale);
    let side = NETWORK_GLYPH_SIDE * scale;
    for (index, &pixel) in drawn.iter().enumerate() {
        let (column, row) = (index as u32 % side, index as u32 / side);
        let cell = map[(row / scale) as usize].as_bytes()[(column / scale) as usize];
        assert_eq!(pixel, ink(cell), "pixel ({column}, {row}) at scale {scale}");
    }
}

fn rest(desktop: &mut Desktop, (x, y): (u32, u32)) {
    let mut damage = Damage::new();
    desktop.set_time(1000);
    desktop.pointer_moved(x, y, &mut damage, &mut DiscardEvents);
    desktop.tick(1000 + TOOLTIP_DELAY_MS + 100, &mut damage, &mut DiscardEvents);
}

fn click(desktop: &mut Desktop, (x, y): (u32, u32)) -> Action {
    let mut damage = Damage::new();
    desktop.pointer_moved(x, y, &mut damage, &mut DiscardEvents);
    let action = desktop.left_button(true, &mut damage, &mut DiscardEvents);
    desktop.left_button(false, &mut damage, &mut DiscardEvents);
    action
}

fn press(desktop: &mut Desktop, code: u16, modifiers: Modifiers) -> Action {
    let mut damage = Damage::new();
    let action = desktop.key(code, true, modifiers, &mut damage, &mut DiscardEvents);
    desktop.key(code, false, modifiers, &mut damage, &mut DiscardEvents);
    action
}

fn inset(rect: Rect) -> Rect {
    Rect::new(rect.x + 2, rect.y + 2, rect.width - 4, rect.height - 4)
}

fn contains_rect(damage: &Damage, rect: Rect) -> bool {
    damage
        .rects()
        .iter()
        .any(|damaged| damaged.union(rect) == *damaged)
}

fn taskbar_items_right_edge(desktop: &Desktop) -> u32 {
    let mut right = 0;
    desktop.for_each_panel_item(|item, rect| {
        if matches!(item, PanelItem::Start | PanelItem::TaskbarWindow(_)) {
            right = right.max(rect.right());
        }
    });
    right
}

fn open_windows(desktop: &mut Desktop, count: usize) {
    for index in 0..count {
        let client = 3 + (index / 8) as u16;
        let created = desktop.create(client, 0, 0, 200, 150, 0).unwrap();
        desktop
            .commit(
                client,
                u32::from(created.window),
                &[],
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .unwrap();
    }
}

#[test]
fn on_the_top_bar_the_indicator_sits_left_of_the_power_button_and_the_agent_chip_left_of_it() {
    for profile in TOP_BAR_PROFILES {
        let mut desktop = with_network(profile, CONNECTED);
        assert_eq!(
            desktop.network_indicator(),
            Some(Rect::new(1216, 0, 28, 28)),
            "{profile:?}"
        );
        assert_eq!(desktop.network_glyph_at(), Some((1222, 6)));
        assert_eq!(desktop.panel_item_at(1230, 14), Some(PanelItem::Network));
        desktop.set_agent_connected(true, &mut Damage::new());
        assert_eq!(desktop.agent_chip(), Some(Rect::new(1152, 4, 56, 20)));

        let mut bare = new_desktop(profile);
        bare.set_network(None, &mut Damage::new());
        assert_eq!(bare.panel_item_at(1230, 14), None);
        bare.set_agent_connected(true, &mut Damage::new());
        assert_eq!(bare.agent_chip(), Some(Rect::new(1180, 4, 56, 20)));
    }
}

#[test]
fn on_the_taskbar_the_indicator_sits_left_of_the_clock_and_the_items_stop_before_it() {
    let mut desktop = with_network(Profile::Windows, CONNECTED);
    assert_eq!(
        desktop.network_indicator(),
        Some(Rect::new(1096, 760, 40, 40))
    );
    assert_eq!(desktop.network_glyph_at(), Some((1108, 772)));
    assert_eq!(desktop.panel_item_at(1116, 780), Some(PanelItem::Network));
    open_windows(&mut desktop, 30);
    assert_eq!(taskbar_items_right_edge(&desktop), 1060);
    assert!(taskbar_items_right_edge(&desktop) <= 1080);
    desktop.set_agent_connected(true, &mut Damage::new());
    assert_eq!(desktop.agent_chip(), Some(Rect::new(1032, 770, 56, 20)));

    let mut bare = new_desktop(Profile::Windows);
    open_windows(&mut bare, 30);
    assert_eq!(taskbar_items_right_edge(&bare), 1104);
    bare.set_agent_connected(true, &mut Damage::new());
    assert_eq!(bare.agent_chip(), Some(Rect::new(1072, 770, 56, 20)));
}

#[test]
fn each_state_draws_its_map_on_every_bar_and_the_four_glyphs_differ() {
    for profile in Profile::ALL {
        let mut hashes = Vec::new();
        for (state, map) in STATES {
            let desktop = with_network(profile, state);
            assert_glyph(&desktop, map, 1);
            hashes.push(fnv(&glyph_pixels(&desktop, 1)));
        }
        for (index, hash) in hashes.iter().enumerate() {
            assert!(!hashes[..index].contains(hash), "{profile:?}: state {index}");
        }
    }
}

#[test]
fn at_scale_2_the_target_and_the_glyph_pixel_double() {
    for profile in Profile::ALL {
        let mut desktop = with_network(profile, CONNECTED);
        desktop.layout = layout(1280, 800, 2, profile);
        let indicator = desktop.network_indicator().unwrap();
        let (x, y) = desktop.network_glyph_at().unwrap();
        let (side, inset) = match profile {
            Profile::Windows => (80, 24),
            Profile::Macos | Profile::Linux => (56, 12),
        };
        assert_eq!((indicator.width, indicator.height), (side, side));
        assert_eq!((x - indicator.x, y - indicator.y), (inset, inset));
        assert_glyph(&desktop, &CONNECTED_MAP, 2);
    }
}

#[test]
fn the_top_bar_draws_no_hover_and_the_pressed_fill_above_its_edge_line() {
    let mut desktop = with_network(Profile::Linux, CONNECTED);
    desktop.pointer_moved(1230, 14, &mut Damage::new(), &mut DiscardEvents);
    assert_eq!(at(&painted(&desktop), 1217, 1), colour::BAR);
    let mut damage = Damage::new();
    let action = desktop.left_button(true, &mut damage, &mut DiscardEvents);
    assert_eq!(action, Action::OpenSettingsPane(SettingsPane::Network));
    let pressed = painted(&desktop);
    assert_eq!(at(&pressed, 1217, 1), colour::BAR_PRESSED);
    assert_eq!(at(&pressed, 1217, 26), colour::BAR_PRESSED);
    assert_eq!(at(&pressed, 1217, 27), colour::BAR_EDGE);
    assert_eq!(at(&pressed, 1222, 6), colour::TEXT);
    desktop.left_button(false, &mut damage, &mut DiscardEvents);
    assert_eq!(at(&painted(&desktop), 1217, 1), colour::BAR);
}

#[test]
fn the_taskbar_draws_hover_and_pressed_below_its_edge_line() {
    let mut desktop = with_network(Profile::Windows, CONNECTED);
    assert_eq!(at(&painted(&desktop), 1097, 761), colour::BAR);
    desktop.pointer_moved(1116, 780, &mut Damage::new(), &mut DiscardEvents);
    let hovered = painted(&desktop);
    assert_eq!(at(&hovered, 1097, 761), colour::BAR_HOVER);
    assert_eq!(at(&hovered, 1097, 760), colour::BAR_EDGE);
    desktop.left_button(true, &mut Damage::new(), &mut DiscardEvents);
    let pressed = painted(&desktop);
    assert_eq!(at(&pressed, 1097, 761), colour::BAR_PRESSED);
    assert_eq!(at(&pressed, 1097, 799), colour::BAR_PRESSED);
    assert_eq!(at(&pressed, 1108, 772), colour::TEXT);
}

#[test]
fn resting_on_the_indicator_shows_the_state_s_words_where_the_design_puts_them() {
    for (state, text, top_bar, taskbar) in [
        (
            NetworkState::CableUnplugged,
            "Ethernet: cable unplugged",
            Rect::new(1056, 32, 216, 24),
            Rect::new(1008, 732, 216, 24),
        ),
        (
            NetworkState::GettingAddress,
            "Ethernet: getting an address...",
            Rect::new(1008, 32, 264, 24),
            Rect::new(984, 732, 264, 24),
        ),
        (
            NetworkState::NoAddress,
            "Ethernet: no address. EggOS keeps trying.",
            Rect::new(928, 32, 344, 24),
            Rect::new(928, 732, 344, 24),
        ),
        (
            CONNECTED,
            "Ethernet: connected, 10.0.2.15",
            Rect::new(1016, 32, 256, 24),
            Rect::new(988, 732, 256, 24),
        ),
    ] {
        for (profile, point, rect) in [
            (Profile::Linux, TOP_BAR_POINT, top_bar),
            (Profile::Macos, TOP_BAR_POINT, top_bar),
            (Profile::Windows, TASKBAR_POINT, taskbar),
        ] {
            let mut desktop = with_network(profile, state);
            assert_eq!(desktop.network_indicator_name(), Some(text));
            rest(&mut desktop, point);
            assert_eq!(desktop.tooltip_rect(), Some(rect), "{profile:?}: {text}");
        }
    }
}

#[test]
fn the_name_carries_every_address_in_its_shortest_dotted_form() {
    for (address, text) in [
        ([0, 0, 0, 0], "Ethernet: connected, 0.0.0.0"),
        ([192, 168, 100, 9], "Ethernet: connected, 192.168.100.9"),
        ([255, 255, 255, 255], "Ethernet: connected, 255.255.255.255"),
    ] {
        let desktop = with_network(Profile::Linux, NetworkState::Connected(address));
        assert_eq!(desktop.network_indicator_name(), Some(text));
    }
}

#[test]
fn a_change_of_state_under_the_tooltip_re_places_it_with_the_new_words() {
    for (profile, point, before, after) in [
        (
            Profile::Linux,
            TOP_BAR_POINT,
            Rect::new(1016, 32, 256, 24),
            Rect::new(1056, 32, 216, 24),
        ),
        (
            Profile::Windows,
            TASKBAR_POINT,
            Rect::new(988, 732, 256, 24),
            Rect::new(1008, 732, 216, 24),
        ),
    ] {
        let mut desktop = with_network(profile, CONNECTED);
        rest(&mut desktop, point);
        assert_eq!(desktop.tooltip_rect(), Some(before));
        let mut damage = Damage::new();
        desktop.set_network(Some(NetworkState::CableUnplugged), &mut damage);
        assert_eq!(desktop.tooltip_rect(), Some(after), "{profile:?}");
        assert_eq!(
            desktop.network_indicator_name(),
            Some(CABLE_UNPLUGGED_TOOLTIP)
        );
        let indicator = desktop.network_indicator().unwrap();
        for rect in [before, after, indicator] {
            assert!(contains_rect(&damage, rect), "{profile:?}: {rect:?}");
        }
    }
}

#[test]
fn a_change_of_state_redraws_only_the_indicator() {
    let mut desktop = with_network(Profile::Linux, NetworkState::GettingAddress);
    let before = painted(&desktop);
    let mut damage = Damage::new();
    desktop.set_network(Some(CONNECTED), &mut damage);
    let indicator = desktop.network_indicator().unwrap();
    assert_eq!(damage.rects(), &[indicator]);
    let after = painted(&desktop);
    assert_ne!(before, after);
    for (index, (old, new)) in before.iter().zip(&after).enumerate() {
        if old != new {
            let (x, y) = ((index % 1280) as u32, (index / 1280) as u32);
            assert!(contains(indicator, x, y), "({x}, {y}) changed");
        }
    }
    let mut damage = Damage::new();
    desktop.set_network(Some(CONNECTED), &mut damage);
    assert!(damage.rects().is_empty());
}

#[test]
fn without_an_adapter_nothing_is_drawn_and_nothing_is_hit() {
    for profile in Profile::ALL {
        let plain = new_desktop(profile);
        let mut none = new_desktop(profile);
        none.set_network(None, &mut Damage::new());
        assert_eq!(none.network_indicator(), None);
        assert_eq!(none.network_glyph_at(), None);
        assert_eq!(none.network_indicator_name(), None);
        assert_eq!(fnv(&painted(&none)), fnv(&painted(&plain)), "{profile:?}");
        let mut gone = with_network(profile, CONNECTED);
        let mut damage = Damage::new();
        gone.set_network(None, &mut damage);
        let bar = gone.layout().panels().next().unwrap();
        assert!(contains_rect(&damage, bar));
        assert_eq!(fnv(&painted(&gone)), fnv(&painted(&plain)), "{profile:?}");
        for (x, y) in [TOP_BAR_POINT, TASKBAR_POINT] {
            assert_ne!(gone.panel_item_at(x, y), Some(PanelItem::Network));
        }
    }
}

#[test]
fn the_layout_indicator_and_the_chips_move_left_of_the_network_indicator() {
    let mut top = with_network(Profile::Linux, CONNECTED);
    top.set_keyboard_layouts(&[US, GERMAN], 0, None, &mut Damage::new());
    assert_eq!(
        top.keyboard_layout_indicator(),
        Some(Rect::new(1188, 0, 28, 28))
    );
    assert_eq!(top.keyboard_layout_text_at(), Some((1194, 6)));
    top.set_agent_connected(true, &mut Damage::new());
    assert_eq!(top.agent_chip(), Some(Rect::new(1124, 4, 56, 20)));

    let mut taskbar = with_network(Profile::Windows, CONNECTED);
    taskbar.set_keyboard_layouts(&[US, GERMAN], 0, None, &mut Damage::new());
    assert_eq!(
        taskbar.keyboard_layout_indicator(),
        Some(Rect::new(1048, 760, 40, 40))
    );
    assert_eq!(taskbar.keyboard_layout_text_at(), Some((1060, 772)));
    taskbar.set_agent_connected(true, &mut Damage::new());
    assert_eq!(taskbar.agent_chip(), Some(Rect::new(984, 770, 56, 20)));

    for (profile, strip) in [
        (Profile::Macos, Rect::new(1152, 4, 56, 20)),
        (Profile::Windows, Rect::new(1032, 770, 56, 20)),
    ] {
        let mut desktop = with_network(profile, CONNECTED);
        desktop.set_latched(Modifiers::SHIFT, Modifiers::NONE, &mut Damage::new());
        assert_eq!(desktop.sticky_strip(), Some(strip), "{profile:?}");
    }
}

#[test]
fn a_click_opens_settings_on_network_once_while_it_starts() {
    for (profile, point) in [
        (Profile::Linux, TOP_BAR_POINT),
        (Profile::Macos, TOP_BAR_POINT),
        (Profile::Windows, TASKBAR_POINT),
    ] {
        let mut desktop = with_network(profile, CONNECTED);
        desktop.set_time(1000);
        let opens = Action::OpenSettingsPane(SettingsPane::Network);
        assert_eq!(click(&mut desktop, point), opens, "{profile:?}");
        assert_eq!(click(&mut desktop, point), Action::None);
        desktop.set_time(3100);
        assert_eq!(click(&mut desktop, point), opens);
    }
    let desktop = with_network(Profile::Linux, CONNECTED);
    assert_eq!(
        desktop
            .settings_pane_command(SettingsPane::Network)
            .map(|command| command.words().map(<[u8]>::to_vec).collect::<Vec<_>>()),
        Some(vec![
            b"/bin/settings".to_vec(),
            b"open".to_vec(),
            b"network".to_vec()
        ])
    );
}

#[test]
fn a_click_focuses_the_settings_window_when_one_is_open() {
    let mut desktop = with_network(Profile::Linux, CONNECTED);
    let created = desktop.create(3, 0, 0, 400, 300, 0).unwrap();
    let id = u32::from(created.window);
    desktop
        .set_title(3, id, b"Settings", &mut Damage::new())
        .unwrap();
    desktop
        .commit(3, id, &[], &mut Damage::new(), &mut DiscardEvents)
        .unwrap();
    desktop.focus_console(&mut Damage::new(), &mut DiscardEvents);
    let settings = desktop.find(created.window).unwrap();
    assert_ne!(desktop.focused_normal(), Some(settings));
    assert_eq!(click(&mut desktop, TOP_BAR_POINT), Action::None);
    assert_eq!(desktop.focused_normal(), Some(settings));
    desktop.minimise(settings, &mut Damage::new(), &mut DiscardEvents);
    assert!(desktop.window_at(settings).minimised);
    assert_eq!(click(&mut desktop, TOP_BAR_POINT), Action::None);
    assert!(!desktop.window_at(settings).minimised);
    assert_eq!(desktop.focused_normal(), Some(settings));
}

#[test]
fn the_keys_reach_the_indicator_in_the_bar_s_order_and_enter_clicks_it() {
    for profile in Profile::ALL {
        let mut desktop = with_network(profile, CONNECTED);
        let ring = inset(desktop.network_indicator().unwrap());
        let ctrl_alt = Modifiers::from_bits(Modifiers::CTRL.bits() | Modifiers::ALT.bits());
        press(&mut desktop, key::TAB, ctrl_alt);
        let mut steps = 0;
        while desktop.panel_focus_ring() != Some(ring) {
            press(&mut desktop, key::RIGHT, Modifiers::NONE);
            steps += 1;
            assert!(steps < 64, "{profile:?}: the ring never reached the indicator");
        }
        press(&mut desktop, key::RIGHT, Modifiers::NONE);
        assert_eq!(
            desktop.panel_focus_ring(),
            Some(inset(desktop.power_button())),
            "{profile:?}: the power button follows the indicator"
        );
        press(&mut desktop, key::LEFT, Modifiers::NONE);
        assert_eq!(
            press(&mut desktop, key::ENTER, Modifiers::NONE),
            Action::OpenSettingsPane(SettingsPane::Network)
        );
        assert!(!desktop.panel_has_focus());
    }
}
