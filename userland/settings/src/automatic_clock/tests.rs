use super::*;
use crate::settings_window::{DATE_AND_TIME, KEYBOARD, SettingsWindow};
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::{Input, Modifiers, MouseButton, key};

const NOW: i64 = 1_793_282_709;
const MIDNIGHT_AFTER_NOW: i64 = 1_793_318_400;
const SIDEBAR_X: i32 = 40;

fn utc() -> usize {
    crate::facts::utc_zone()
}

fn record(outcome: Outcome, finished: i64, server: &str, moved_ms: i64) -> Record {
    let mut record = Record::new(outcome, finished - 9, server.as_bytes());
    record.finished = (outcome != Outcome::Syncing).then_some(finished);
    record.moved_ms = moved_ms;
    record
}

fn set(moved_ms: i64) -> Record {
    record(Outcome::Set, NOW, "pool.ntp.org", moved_ms)
}

fn line_1(record: Option<&Record>, adapter: Adapter, zone: usize) -> Status {
    status_of(record, adapter, NOW, zone)
}

fn says(status: Status) -> (String, Token, Option<&'static str>) {
    (
        String::from(status.line_1.as_str()),
        status.ink,
        status.remedy,
    )
}

#[test]
fn moved_text_floors_each_unit() {
    for (ms, text) in [
        (40, "40 ms"),
        (400, "0.4 s"),
        (12_000, "12 s"),
        (59_999, "59.9 s"),
        (60_000, "1 min"),
        (3_599_999, "59 min"),
        (3_600_000, "1 h 0 min"),
        (10_920_000, "3 h 2 min"),
        (90_061_000, "1 d 1 h"),
    ] {
        assert_eq!(moved_text(ms).as_str(), text, "{ms}");
    }
}

#[test]
fn the_record_s_outcome_is_worded_as_the_design_s_table() {
    let lines = |record: Record| says(line_1(Some(&record), Adapter::Connected, utc()));
    assert_eq!(
        lines(set(12_000)),
        (
            "Set today at 14:05 from pool.ntp.org; moved 12 s forward.".into(),
            Token::Text,
            None
        )
    );
    assert_eq!(
        lines(set(-400)).0,
        "Set today at 14:05 from pool.ntp.org; moved 0.4 s back."
    );
    assert_eq!(
        lines(set(0)).0,
        "Set today at 14:05 from pool.ntp.org; it was already right."
    );
    assert_eq!(
        lines(set(-3_600_000)).0,
        "Set today at 14:05 from pool.ntp.org; moved 1 h 0 min back."
    );
    let earlier = |seconds: i64| record(Outcome::Set, NOW - seconds, "pool.ntp.org", 12_000);
    assert_eq!(
        lines(earlier(86_400)).0,
        "Set yesterday at 14:05 from pool.ntp.org; moved 12 s forward."
    );
    assert_eq!(
        lines(earlier(172_800)).0,
        "Set on 27 Oct at 14:05 from pool.ntp.org; moved 12 s forward."
    );
    let paris = crate::facts::zone_index("Europe/Paris").unwrap();
    assert_eq!(
        says(line_1(Some(&set(12_000)), Adapter::Connected, paris)).0,
        "Set today at 15:05 from pool.ntp.org; moved 12 s forward."
    );
    assert_eq!(
        lines(record(Outcome::Confirmed, NOW, "pool.ntp.org", -10_920_000)),
        (
            "Set today at 14:05, HTTPS agreed; moved 3 h 2 min back.".into(),
            Token::Text,
            None
        )
    );
    assert_eq!(
        lines(record(
            Outcome::Unconfirmed,
            NOW,
            "pool.ntp.org",
            -10_920_000
        )),
        (
            "Off by 3 h 2 min; not applied: HTTPS could not confirm it.".into(),
            Token::Text,
            Some("Turn this off and use Set to, or add /etc/ssl/roots.pem.")
        )
    );
    assert_eq!(
        lines(record(Outcome::NoAnswer, NOW, "pool.ntp.org", 0)),
        (
            "No answer from pool.ntp.org.".into(),
            Token::Text,
            Some("Check the network, then turn this off and on to try again.")
        )
    );
    assert_eq!(
        lines(record(Outcome::NotAllowed, NOW, "pool.ntp.org", 0)),
        (
            "Not allowed: this user may not set the clock.".into(),
            Token::Text,
            None
        )
    );
    assert_eq!(
        lines(record(Outcome::Syncing, NOW, "pool.ntp.org", 0)),
        (
            "Setting the clock from the network...".into(),
            Token::TextDim,
            None
        )
    );
    assert_eq!(
        lines(record(Outcome::NoAddress, NOW, "pool.ntp.org", 0)),
        (
            "Waiting for a network address to set the clock.".into(),
            Token::TextDim,
            None
        )
    );
    let long = lines(record(
        Outcome::Set,
        NOW - 90_000,
        "time.example.com:123",
        12_000,
    ))
    .0;
    assert_eq!(
        long,
        "Set yesterday at 13:05 from time.example.com:123; moved 12 s ..."
    );
    assert_eq!(long.len(), 64);
}

#[test]
fn without_a_record_the_lines_wait_for_an_address_or_offer_a_sync() {
    assert_eq!(
        says(line_1(None, Adapter::WithoutAddress, utc())),
        (
            "Waiting for a network address to set the clock.".into(),
            Token::TextDim,
            None
        )
    );
    assert_eq!(
        says(line_1(None, Adapter::Connected, utc())),
        (
            "Not set from the network yet.".into(),
            Token::TextDim,
            Some("Turn this off and on to set it now.")
        )
    );
}

#[test]
fn an_adapter_is_connected_once_it_has_an_address() {
    let parse = |line: &str| eggos_net::status::parse_interface(line.as_bytes());
    assert_eq!(Adapter::of(None), Adapter::Absent);
    let waiting = parse("eth0 up 52:54:00:12:34:56 1500 asking - - - - - - 0 0 0 0 0 0 0\n");
    assert_eq!(Adapter::of(waiting.as_ref()), Adapter::WithoutAddress);
    let connected = parse(crate::facts::EXAMPLE_ADAPTER);
    assert_eq!(Adapter::of(connected.as_ref()), Adapter::Connected);
}

fn handle(window: &mut SettingsWindow, input: Input) -> Option<Request> {
    let response = window.tree().handle(input, 0);
    window.respond(response, 0)
}

fn click(window: &mut SettingsWindow, x: i32, y: i32) -> Option<Request> {
    let mut request = handle(window, Input::PointerMove { x, y });
    for pressed in [true, false] {
        let button = Input::Button {
            x,
            y,
            button: MouseButton::Left,
            pressed,
        };
        request = request.or(handle(window, button));
    }
    request
}

fn press(window: &mut SettingsWindow, code: u16, character: u32) -> Option<Request> {
    let input = Input::Key {
        code,
        pressed: true,
        modifiers: Modifiers::default(),
        character,
    };
    handle(window, input)
}

fn sidebar_row(pane: usize) -> i32 {
    44 + pane as i32 * 32 + 10
}

fn adapter_facts(automatic: bool, adapter: Option<&str>) -> Facts {
    Facts {
        time_automatic: automatic,
        network_adapter: adapter
            .and_then(|line| eggos_net::status::parse_interface(line.as_bytes())),
        ..Facts::example()
    }
}

const WAITING_ADAPTER: &str = "eth0 up 52:54:00:12:34:56 1500 asking - - - - - - 0 0 0 0 0 0 0\n";

fn shown(facts: Facts) -> SettingsWindow {
    let mut window = SettingsWindow::new(facts);
    click(&mut window, SIDEBAR_X, sidebar_row(DATE_AND_TIME));
    assert_eq!(window.pane(), DATE_AND_TIME);
    window
}

fn controls(window: &SettingsWindow) -> ClockWidgets {
    window.automatic_clock().widgets()
}

fn lines(window: &SettingsWindow) -> [Option<&str>; 2] {
    window.automatic_clock().lines(window.widgets())
}

fn ink(window: &SettingsWindow, id: WidgetId) -> Option<Token> {
    match window.widgets().kind(id) {
        Some(Kind::Label { colour, .. }) => Some(*colour),
        _ => None,
    }
}

fn ticked(window: &SettingsWindow) -> bool {
    window.widgets().selected(controls(window).check_box) == Some(1)
}

fn set_to_enabled(window: &SettingsWindow) -> bool {
    let tree = window.widgets();
    tree.is_enabled(controls(window).field)
}

#[test]
fn the_row_goes_under_now_and_everything_under_it_moves_down() {
    let mut window = shown(adapter_facts(false, Some(crate::facts::EXAMPLE_ADAPTER)));
    let rect = |window: &SettingsWindow, id: WidgetId| window.widgets().rect(id).unwrap();
    let widgets = controls(&window);
    let check_box = rect(&window, widgets.check_box);
    assert_eq!((check_box.x, check_box.y, check_box.height), (192, 110, 28));
    assert_eq!(
        window.widgets().text_of(widgets.check_box),
        Some("Set automatically")
    );
    let set_to = rect(&window, widgets.set_to);
    assert_eq!((set_to.x, set_to.y), (192, 156));
    assert_eq!(rect(&window, widgets.field), Rect::new(320, 150, 200, 28));
    assert_eq!(rect(&window, widgets.button), Rect::new(528, 150, 96, 28));
    let line_1 = rect(&window, widgets.line_1);
    assert_eq!((line_1.x, line_1.y), (192, 196));
    let line_2 = rect(&window, widgets.line_2);
    assert_eq!((line_2.x, line_2.y), (192, 216));
    let zones = window.zone_picker();
    assert_eq!(rect(&window, zones.field()), Rect::new(192, 284, 304, 28));
    assert_eq!(rect(&window, zones.list()), Rect::new(192, 320, 432, 72));
    let pane = |window: &SettingsWindow, pane: usize| rect(window, window.pane_widget(pane));
    assert_eq!(pane(&window, DATE_AND_TIME).height, 408);
    assert_eq!(pane(&window, KEYBOARD).height, 348);
    window.tree().resize(640, 360);
    handle(
        &mut window,
        Input::Wheel {
            x: 400,
            y: 40,
            notches: -100,
        },
    );
    assert_eq!(pane(&window, DATE_AND_TIME).y, -48);
}

#[test]
fn while_the_network_clock_is_in_charge_set_to_is_disabled_and_skipped() {
    let mut window = shown(adapter_facts(true, Some(crate::facts::EXAMPLE_ADAPTER)));
    let widgets = controls(&window);
    assert!(ticked(&window));
    assert_eq!(ink(&window, widgets.set_to), Some(Token::TextDim));
    assert!(!set_to_enabled(&window));
    assert!(!window.widgets().is_enabled(widgets.button));
    click(&mut window, SIDEBAR_X, sidebar_row(DATE_AND_TIME));
    press(&mut window, key::TAB, 0);
    assert_eq!(window.widgets().focus(), Some(widgets.check_box));
    press(&mut window, key::TAB, 0);
    assert_eq!(window.widgets().focus(), Some(window.zone_picker().field()));

    let window = shown(adapter_facts(false, Some(crate::facts::EXAMPLE_ADAPTER)));
    assert!(!ticked(&window));
    assert_eq!(ink(&window, widgets.set_to), Some(Token::Text));
    assert!(set_to_enabled(&window));
    assert_eq!(window.widgets().text_of(widgets.field), Some(""));
    assert_eq!(
        lines(&window),
        [Some("From the hardware clock, read at boot."), None]
    );

    let window = shown(adapter_facts(true, None));
    assert!(ticked(&window));
    assert!(set_to_enabled(&window));
    assert_eq!(
        lines(&window),
        [
            Some("No network adapter: the clock cannot be set from the network."),
            None
        ]
    );
    assert_eq!(ink(&window, widgets.line_1), Some(Token::TextDim));
}

#[test]
fn the_lines_follow_the_record_and_retitle_only_what_changed() {
    let mut window = shown(adapter_facts(true, Some(WAITING_ADAPTER)));
    let waiting = window.facts().network_adapter;
    assert_eq!(
        lines(&window),
        [
            Some("Waiting for a network address to set the clock."),
            None
        ]
    );
    let connected = eggos_net::status::parse_interface(crate::facts::EXAMPLE_ADAPTER.as_bytes());
    window.show_clock_sync(None, connected, false);
    assert_eq!(
        lines(&window),
        [
            Some("Not set from the network yet."),
            Some("Turn this off and on to set it now.")
        ]
    );
    window.show_clock_sync(
        Some(record(Outcome::Syncing, NOW, "pool.ntp.org", 0)),
        connected,
        false,
    );
    assert_eq!(
        lines(&window),
        [Some("Setting the clock from the network..."), None]
    );
    window.show_clock_sync(Some(set(12_000)), connected, false);
    assert_eq!(
        lines(&window),
        [
            Some("Set today at 14:05 from pool.ntp.org; moved 12 s forward."),
            None
        ]
    );
    window.tree().take_damage();
    window.show_clock_sync(Some(set(12_000)), connected, false);
    assert!(window.tree().take_damage().is_empty());
    window.show_time(MIDNIGHT_AFTER_NOW as u64 - 1);
    window.tree().take_damage();
    window.show_clock_sync(Some(set(12_000)), connected, false);
    assert!(window.tree().take_damage().is_empty());
    window.show_time(MIDNIGHT_AFTER_NOW as u64);
    window.show_clock_sync(Some(set(12_000)), connected, false);
    assert_eq!(
        lines(&window)[0],
        Some("Set yesterday at 14:05 from pool.ntp.org; moved 12 s forward.")
    );
    window.show_clock_sync(Some(set(12_000)), waiting, false);
    assert_eq!(
        lines(&window)[0],
        Some("Set yesterday at 14:05 from pool.ntp.org; moved 12 s forward."),
        "a record says more than the address"
    );
}

#[test]
fn ticking_the_box_saves_it_and_syncs_at_once_and_unticking_gives_set_to_back() {
    let mut window = shown(adapter_facts(false, Some(crate::facts::EXAMPLE_ADAPTER)));
    window.show_clock_sync(Some(set(12_000)), window.facts().network_adapter, false);
    let widgets = controls(&window);
    assert_eq!(
        click(&mut window, 216, 124),
        Some(Request::SetTimeAutomatic {
            on: true,
            sync_now: true
        })
    );
    window.time_automatic_saved(true, Ok(()), Some(NOW));
    assert!(ticked(&window) && window.facts().time_automatic);
    assert!(!set_to_enabled(&window));
    assert!(!window.widgets().is_enabled(widgets.button));
    assert_eq!(ink(&window, widgets.set_to), Some(Token::TextDim));
    assert_eq!(
        lines(&window),
        [Some("Setting the clock from the network..."), None]
    );
    let adapter = window.facts().network_adapter;
    window.show_clock_sync(Some(set(12_000)), adapter, true);
    assert_eq!(
        lines(&window)[0],
        Some("Setting the clock from the network..."),
        "the record from before the sync is not its result"
    );
    let mut result = record(Outcome::Set, NOW + 2, "pool.ntp.org", -400);
    result.started = NOW;
    window.show_clock_sync(Some(result), adapter, false);
    assert_eq!(
        lines(&window)[0],
        Some("Set today at 14:05 from pool.ntp.org; moved 0.4 s back.")
    );
    assert_eq!(
        click(&mut window, 216, 124),
        Some(Request::SetTimeAutomatic {
            on: false,
            sync_now: false
        })
    );
    window.time_automatic_saved(false, Ok(()), None);
    assert!(!ticked(&window));
    assert!(set_to_enabled(&window));
    assert_eq!(ink(&window, widgets.set_to), Some(Token::Text));
    assert_eq!(
        lines(&window),
        [Some("From the hardware clock, read at boot."), None]
    );

    window.tree().set_focus(Some(widgets.check_box), 0);
    assert_eq!(
        press(&mut window, key::SPACE, u32::from(b' ')),
        Some(Request::SetTimeAutomatic {
            on: true,
            sync_now: true
        })
    );

    let mut alone = shown(adapter_facts(false, None));
    assert_eq!(
        click(&mut alone, 216, 124),
        Some(Request::SetTimeAutomatic {
            on: true,
            sync_now: false
        })
    );
    alone.time_automatic_saved(true, Ok(()), None);
    assert!(ticked(&alone) && set_to_enabled(&alone));
    assert_eq!(
        lines(&alone)[0],
        Some("No network adapter: the clock cannot be set from the network.")
    );
}

#[test]
fn a_refused_write_puts_the_box_back_and_without_a_disk_the_choice_lasts_until_power_off() {
    let mut window = shown(adapter_facts(false, Some(crate::facts::EXAMPLE_ADAPTER)));
    click(&mut window, 216, 124);
    window.time_automatic_saved(true, Err(Errno::EPERM), None);
    assert!(!ticked(&window) && !window.facts().time_automatic);
    assert!(set_to_enabled(&window));
    assert_eq!(
        lines(&window)[0],
        Some("Not allowed: this user may not change Set automatically.")
    );
    click(&mut window, SIDEBAR_X, sidebar_row(KEYBOARD));
    click(&mut window, SIDEBAR_X, sidebar_row(DATE_AND_TIME));
    assert_eq!(
        lines(&window)[0],
        Some("From the hardware clock, read at boot.")
    );

    let mut diskless = shown(Facts {
        on_disk: false,
        ..adapter_facts(true, Some(crate::facts::EXAMPLE_ADAPTER))
    });
    assert_eq!(
        lines(&diskless),
        [
            Some("Not set from the network yet."),
            Some("Turn this off and on to set it now.")
        ],
        "a remedy wins line 2"
    );
    click(&mut diskless, 216, 124);
    diskless.time_automatic_saved(false, Ok(()), None);
    assert!(!ticked(&diskless));
    assert_eq!(
        lines(&diskless),
        [
            Some("From the hardware clock, read at boot."),
            Some("No disk attached: this choice lasts until power-off.")
        ]
    );
}

#[test]
fn wm_show_lists_the_box_and_the_disabled_set_to() {
    use eggos_toolkit::elements::{MAX_TREE_BYTES, Role, TreeWriter, elements, flags};
    let mut window = shown(adapter_facts(true, Some(crate::facts::EXAMPLE_ADAPTER)));
    let mut bytes = alloc::vec![0u8; MAX_TREE_BYTES];
    let mut writer = TreeWriter::new(&mut bytes);
    window.tree().describe(&mut writer);
    let described: Vec<(Role, u16, String)> = elements(writer.bytes())
        .map(|element| {
            let element = element.unwrap();
            let name = String::from_utf8(element.name.to_vec()).unwrap();
            (element.role, element.flags, name)
        })
        .collect();
    let named = |role: Role, name: &str| {
        described
            .iter()
            .find(|element| element.0 == role && element.2 == name)
            .map(|element| element.1)
    };
    let check_box = named(Role::Checkbox, "Set automatically").unwrap();
    assert_ne!(check_box & flags::CHECKED, 0);
    let field = named(Role::TextField, "YYYY-MM-DD HH:MM").unwrap();
    assert_ne!(field & flags::DISABLED, 0);
    assert!(named(Role::Label, "Turn this off and on to set it now.").is_some());
}
