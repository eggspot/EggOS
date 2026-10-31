use super::*;
use crate::agents_pane::{GrantsRead, VIEWS};
use crate::facts::Facts;
use crate::settings_window::{AGENTS, SettingsWindow};
use eggos_mcp::grants::{GrantStore, Grants, StoreState};
use eggos_mcp::pairing::{fingerprint_text, token_from};
use eggos_toolkit::input::{Input, Modifiers, MouseButton};
use eggos_toolkit::widget::radio_rect;

const AGENTS_ROW_Y: i32 = 44 + 7 * 32 + 10;
const NOW: u64 = 1_793_438_042;
const CODE: Code = *b"BCDEFGHJ";

fn grants() -> GrantsRead {
    let mut store = GrantStore::new();
    store
        .add(
            AgentId::new(b"phone").unwrap(),
            AgentName::new(b"Claude on my phone").unwrap(),
        )
        .unwrap();
    GrantsRead {
        store: StoreState::Loaded(store),
        boot: Grants::NONE,
        now: NOW,
        on_disk: true,
    }
}

fn network(state: NetworkState, pending: Option<PendingPairing>) -> NetworkRead {
    NetworkRead {
        state,
        fingerprint: Some(fingerprint_text(b"a certificate")),
        pending,
        now: NOW,
    }
}

fn waiting(agent: &str, now: u64) -> PendingPairing {
    PendingPairing::new(
        AgentId::new(agent.as_bytes()).unwrap(),
        &CODE,
        token_from(&[3; 32]),
        now,
    )
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

fn choose(window: &mut SettingsWindow, radios: WidgetId, option: usize) -> Option<Request> {
    let tree = window.widgets();
    let rect = radio_rect(
        tree.kind(radios).unwrap(),
        tree.rect(radios).unwrap(),
        option,
    );
    click(window, rect.x + 4, rect.y + rect.height / 2)
}

fn press_button(window: &mut SettingsWindow, id: WidgetId) -> Option<Request> {
    let rect = window.widgets().rect(id).unwrap();
    click(window, rect.x + 10, rect.y + 10)
}

fn type_into(window: &mut SettingsWindow, field: WidgetId, text: &str) {
    window.tree().set_focus(Some(field), 0);
    for character in text.chars() {
        let key = Input::Key {
            code: 30,
            pressed: true,
            modifiers: Modifiers::default(),
            character: character as u32,
        };
        handle(window, key);
    }
}

fn on_network_view(read: NetworkRead) -> SettingsWindow {
    let mut window = SettingsWindow::new(Facts::example());
    assert_eq!(
        click(&mut window, 40, AGENTS_ROW_Y),
        Some(Request::ReadGrants)
    );
    window.show_grants(grants());
    let view = window.agents_pane().widget_ids().view;
    assert_eq!(choose(&mut window, view, 2), Some(Request::ReadNetwork));
    assert_eq!(window.pane(), AGENTS);
    window.show_network(read);
    window
}

fn ids(window: &SettingsWindow) -> NetworkWidgets {
    window.agents_pane().network().widget_ids()
}

fn status(window: &SettingsWindow) -> &str {
    window.agents_pane().status(window.widgets()).unwrap_or("")
}

#[test]
fn the_network_view_says_where_agents_connect_and_shows_the_fingerprint_in_halves() {
    assert_eq!(VIEWS, ["Grants", "Activity", "Network"]);
    assert_eq!(CAPACITY, 140);
    let window = on_network_view(network(NetworkState::On(8443), None));
    let lines = window.agents_pane().network().lines(window.widgets());
    assert_eq!(lines[0], "Network");
    assert_eq!(
        lines[1],
        "Agents connect to https://<this machine>:8443/mcp"
    );
    assert_eq!(lines[2], PEM_LINE);
    assert!(
        lines[3].starts_with("SHA-256 ") && lines[3].ends_with(':'),
        "{}",
        lines[3]
    );
    assert_eq!(lines[3].len(), 56);
    assert_eq!(lines[4].len(), 47);
    assert_eq!(status(&window), IDLE_LINE);
    let widgets = window.widgets();
    assert_eq!(widgets.selected(ids(&window).switch), Some(1));
    for id in [ids(&window).pair, ids(&window).switch] {
        assert!(widgets.is_person_only(id));
    }
}

#[test]
fn off_is_written_at_once_and_said_to_take_effect_at_the_next_start() {
    let mut window = on_network_view(network(NetworkState::On(8443), None));
    let switch = ids(&window).switch;
    assert_eq!(choose(&mut window, switch, 1), None);
    assert_eq!(
        choose(&mut window, switch, 0),
        Some(Request::TurnNetwork(None))
    );
    window.network_turned(Ok(()), network(NetworkState::Off, None));
    let lines = window.agents_pane().network().lines(window.widgets());
    assert_eq!(lines[0], "Network \u{b7} from EggOS's next start");
    assert_eq!(lines[1], OFF_LINE);
    assert_eq!(status(&window), NOT_ANSWERED_LINE);
    assert_eq!(
        choose(&mut window, switch, 1),
        Some(Request::TurnNetwork(Some(8443)))
    );
    window.network_turned(
        Err(eggos_runtime::errno::Errno::EROFS),
        network(NetworkState::Off, None),
    );
    assert!(status(&window).starts_with("Network file not written: "));
    assert_eq!(window.widgets().selected(switch), Some(0));
}

#[test]
fn pairing_asks_for_a_valid_id_then_shows_the_code_until_it_is_used() {
    let mut window = on_network_view(network(NetworkState::On(8443), None));
    let widgets = ids(&window);
    assert_eq!(press_button(&mut window, widgets.pair), None);
    let lines = window
        .agents_pane()
        .network()
        .panel_lines(window.widgets())
        .unwrap();
    assert_eq!(lines[0], ASK_TITLE);
    assert_eq!(lines[1], BAD_ID_LINE);
    assert!(!window.widgets().is_enabled(widgets.primary));
    type_into(&mut window, widgets.id_field, "local");
    assert_eq!(
        window
            .agents_pane()
            .network()
            .panel_lines(window.widgets())
            .unwrap()[1],
        LOCAL_LINE
    );
    window.tree().set_text(widgets.id_field, "");
    type_into(&mut window, widgets.id_field, "phone");
    assert_eq!(
        window
            .agents_pane()
            .network()
            .panel_lines(window.widgets())
            .unwrap()[1],
        "phone is listed: a new token, and its grants stay."
    );
    window.tree().set_text(widgets.id_field, "");
    type_into(&mut window, widgets.id_field, "tv");
    assert!(window.widgets().is_enabled(widgets.primary));
    assert_eq!(
        press_button(&mut window, widgets.primary),
        Some(Request::BeginPairing)
    );
    let (agent, name) = window.to_pair().unwrap();
    assert_eq!(agent.as_bytes(), b"tv");
    assert_eq!(name, None);
    window.paired(
        agent,
        Ok(CODE),
        grants(),
        network(NetworkState::On(8443), Some(waiting("tv", NOW))),
        0,
    );
    let lines = window
        .agents_pane()
        .network()
        .panel_lines(window.widgets())
        .unwrap();
    assert_eq!(lines[0], "Pairing tv");
    assert_eq!(lines[1], SENDS_LINE);
    assert_eq!(lines[2], "BCDE-FGHJ");
    assert_eq!(lines[3], "https://<this machine>:8443/pair");
    assert_eq!(lines[4], "Ends in 5:00.");
    assert!(window.pairing_counting_down());
    let mut later = network(NetworkState::On(8443), Some(waiting("tv", NOW)));
    later.now = NOW + 1;
    window.show_network(later);
    assert_eq!(
        window
            .agents_pane()
            .network()
            .panel_lines(window.widgets())
            .unwrap()[4],
        "Ends in 4:59."
    );
    window.show_network(network(NetworkState::On(8443), None));
    assert!(
        window
            .agents_pane()
            .network()
            .panel_lines(window.widgets())
            .is_none()
    );
    assert_eq!(
        status(&window),
        "Paired tv: it gets nothing until you grant it."
    );
}

#[test]
fn a_burned_code_holds_pairing_back_and_counts_wrong_tries() {
    let mut pending = waiting("phone", NOW - 10);
    for _ in 0..MAX_WRONG_CODES {
        pending.attempt(b"ZZZZ-ZZZZ", NOW - 10);
    }
    let mut window = on_network_view(network(NetworkState::On(8443), Some(pending)));
    let pair = ids(&window).pair;
    assert_eq!(press_button(&mut window, pair), Some(Request::ReadNetwork));
    assert_eq!(
        status(&window),
        "The last code was burned by wrong tries: pair again in 20 s."
    );
    let mut tried = waiting("tv", NOW);
    tried.attempt(b"ZZZZ-ZZZZ", NOW);
    tried.attempt(b"ZZZZ-ZZZZ", NOW);
    assert_eq!(
        countdown_text(&tried, NOW + 2),
        "Ends in 4:58; 2 wrong tries, 5 burn it."
    );
    assert_eq!(countdown_text(&tried, NOW + CODE_LIFETIME_S), ENDED_LINE);
    assert_eq!(clock_time(NOW + CODE_LIFETIME_S), clock_time(NOW + 300));
}
