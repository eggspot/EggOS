use super::*;
use crate::facts::Facts;
use crate::settings_window::{AGENTS, PANES, SettingsWindow, pane_matches};
use eggos_mcp::grant_files::STORE_TEMPORARY;
use eggos_mcp::grants::ClassGrant;
use eggos_runtime::errno::Errno;
use eggos_toolkit::elements::{Action, Outcome};
use eggos_toolkit::input::{Input, Modifiers, MouseButton};
use eggos_toolkit::tree::element_of;
use eggos_toolkit::widget::radio_rect;

const AGENTS_ROW_Y: i32 = 44 + 6 * 32 + 10;
const NOW: u64 = 1_793_438_042;
const WRITE_ENDS: u64 = 1_793_445_240;
const DENY: usize = 0;
const ASK: usize = 1;
const ALLOW: usize = 2;

fn id(text: &str) -> AgentId {
    AgentId::new(text.as_bytes()).unwrap()
}

fn machine() -> GrantsRead {
    let mut store = GrantStore::new();
    let name = AgentName::new(b"Claude on my phone").unwrap();
    let phone = store.add(id("phone"), name).unwrap();
    phone.set(Grant::View, Permission::Allow, None);
    phone.set(Grant::Write, Permission::Ask, Some(WRITE_ENDS));
    let laptop = store
        .add(id("laptop"), AgentName::new(b"laptop").unwrap())
        .unwrap();
    laptop.set(Grant::Run, Permission::Allow, None);
    GrantsRead {
        store: StoreState::Loaded(store),
        boot: Grants::NONE.with(Grant::View).with(Grant::Write),
        now: NOW,
        on_disk: true,
    }
}

fn serve(window: &mut SettingsWindow, read: &mut GrantsRead, request: Option<Request>) {
    match request {
        Some(Request::ReadGrants) => window.show_grants(*read),
        Some(Request::WriteGrants) => {
            read.store = StoreState::Loaded(*window.grants_to_write());
            window.grants_written(Ok(()), *read);
        }
        _ => {}
    }
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
    let key = Input::Key {
        code,
        pressed: true,
        modifiers: Modifiers::default(),
        character,
    };
    handle(window, key)
}

fn opened(read: &mut GrantsRead) -> SettingsWindow {
    let mut window = SettingsWindow::new(Facts::example());
    let request = click(&mut window, 40, AGENTS_ROW_Y);
    assert_eq!(window.pane(), AGENTS);
    assert_eq!(request, Some(Request::ReadGrants));
    serve(&mut window, read, request);
    window
}

fn widgets(window: &SettingsWindow) -> AgentsWidgets {
    window.agents_pane().widget_ids()
}

fn select(window: &mut SettingsWindow, downs: usize) {
    let list = widgets(window).list;
    window.tree().set_focus(Some(list), 0);
    for _ in 0..downs {
        press(window, key::DOWN, 0);
    }
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

fn type_id(window: &mut SettingsWindow, text: &str) {
    let field = widgets(window).id_field;
    window.tree().set_focus(Some(field), 0);
    for character in text.chars() {
        press(window, 30, character as u32);
    }
}

fn status(window: &SettingsWindow) -> &str {
    window.agents_pane().status(window.widgets()).unwrap_or("")
}

fn class_lines(window: &SettingsWindow) -> [&str; 4] {
    window.agents_pane().class_lines(window.widgets())
}

fn choices(window: &SettingsWindow) -> [Option<usize>; 4] {
    window.agents_pane().class_choices(window.widgets())
}

fn written(window: &SettingsWindow, agent: &str) -> AgentGrants {
    *window.grants_to_write().get(&id(agent)).unwrap()
}

#[test]
fn the_agents_pane_comes_after_date_and_time_and_is_found_by_its_words() {
    assert_eq!(PANES[AGENTS], "Agents");
    assert_eq!(PANES[AGENTS - 1], "Date & time");
    assert_eq!(PANES[AGENTS + 1], "Components");
    for word in [
        "agent",
        "grant",
        "mcp",
        "privacy",
        "revoke",
        "audit",
        "permission",
    ] {
        for pane in 0..PANES.len() {
            assert_eq!(pane_matches(pane, word), pane == AGENTS, "{word} {pane}");
        }
    }
}

#[test]
fn the_pane_lists_local_first_and_shows_its_boot_grants() {
    let mut read = machine();
    let window = opened(&mut read);
    let pane = window.agents_pane();
    assert_eq!(
        pane.row_lines(window.widgets()),
        ["local", "phone \u{b7} Claude on my phone", "laptop"]
    );
    assert_eq!(
        class_lines(&window),
        [
            "View",
            "Write",
            "Install \u{b7} started without it",
            "Run \u{b7} started without it"
        ]
    );
    assert_eq!(
        choices(&window),
        [Some(ALLOW), Some(ALLOW), Some(DENY), Some(DENY)]
    );
    assert_eq!(pane.buttons_enabled(window.widgets()), [false, true, false]);
    assert_eq!(status(&window), LOCAL_LINE);
    let heading = window.widgets().rect(widgets(&window).list).unwrap();
    assert_eq!((heading.x, heading.y, heading.width), (192, 64, 208));
}

#[test]
fn selecting_an_agent_shows_its_grants_ends_and_the_boot_ceiling() {
    let mut read = machine();
    let mut window = opened(&mut read);
    select(&mut window, 1);
    assert_eq!(
        class_lines(&window),
        [
            "View",
            "Write \u{b7} until 2026-10-31 11:14",
            "Install \u{b7} started without it",
            "Run \u{b7} started without it"
        ]
    );
    assert_eq!(
        choices(&window),
        [Some(ALLOW), Some(ASK), Some(DENY), Some(DENY)]
    );
    assert_eq!(
        window.agents_pane().buttons_enabled(window.widgets()),
        [false, true, true]
    );
    assert_eq!(
        status(&window),
        "Started with: view write. No agent gets more."
    );
}

#[test]
fn a_choice_writes_the_store_at_once_with_the_grants_last_end() {
    let mut read = machine();
    let mut window = opened(&mut read);
    select(&mut window, 1);
    let radios = widgets(&window).class_radios;
    let request = choose(&mut window, radios[1], ALLOW);
    assert_eq!(request, Some(Request::WriteGrants));
    assert_eq!(
        written(&window, "phone").class(Grant::Write),
        ClassGrant {
            permission: Permission::Allow,
            until: None
        }
    );
    serve(&mut window, &mut read, request);
    assert_eq!(status(&window), "Gave write to phone, from its next call.");
    assert_eq!(choose(&mut window, radios[1], ALLOW), None);
    let lasts = widgets(&window).lasts;
    assert_eq!(choose(&mut window, lasts, 2), None);
    let request = choose(&mut window, radios[0], ASK);
    assert_eq!(
        written(&window, "phone").class(Grant::View),
        ClassGrant {
            permission: Permission::Ask,
            until: Some(NOW + 7200)
        }
    );
    serve(&mut window, &mut read, request);
    assert_eq!(
        class_lines(&window)[0],
        "View \u{b7} until 2026-10-31 11:14"
    );
    assert_eq!(status(&window), "Gave view to phone, asking each time.");
    let request = choose(&mut window, radios[1], DENY);
    assert_eq!(
        written(&window, "phone").class(Grant::Write),
        ClassGrant::DENIED
    );
    serve(&mut window, &mut read, request);
    assert_eq!(
        status(&window),
        "Took write back from phone, from its next call."
    );
}

#[test]
fn run_and_install_ask_first_in_the_panel() {
    let mut read = machine();
    let mut window = opened(&mut read);
    select(&mut window, 1);
    let ids = widgets(&window);
    assert_eq!(choose(&mut window, ids.class_radios[3], ALLOW), None);
    assert_eq!(choices(&window)[3], Some(DENY));
    let pane = window.agents_pane();
    assert_eq!(
        pane.panel_lines(window.widgets()),
        Some(["Grant run to phone?", "Run gives it full control of EggOS."])
    );
    assert_eq!(window.widgets().focus(), Some(ids.cancel));
    assert_eq!(press(&mut window, key::ESC, 27), None);
    assert!(!window.agents_pane().panel_open());
    assert_eq!(
        written(&window, "phone").class(Grant::Run),
        ClassGrant::DENIED
    );
    choose(&mut window, ids.class_radios[3], ALLOW);
    let request = press_button(&mut window, ids.grant);
    assert_eq!(request, Some(Request::WriteGrants));
    assert_eq!(
        written(&window, "phone").class(Grant::Run).permission,
        Permission::Allow
    );
    serve(&mut window, &mut read, request);
    assert_eq!(choices(&window)[3], Some(ALLOW));
    assert_eq!(class_lines(&window)[1], "Write \u{b7} run brings it");
}

#[test]
fn revoke_all_and_forget_change_the_store_and_say_so() {
    let mut read = machine();
    let mut window = opened(&mut read);
    select(&mut window, 2);
    assert_eq!(class_lines(&window)[0], "View \u{b7} run brings it");
    let ids = widgets(&window);
    let request = press_button(&mut window, ids.revoke_all);
    serve(&mut window, &mut read, request);
    let laptop = written(&window, "laptop");
    assert!(
        Grant::ALL
            .iter()
            .all(|&grant| laptop.class(grant) == ClassGrant::DENIED)
    );
    assert_eq!(status(&window), "Took every grant back from laptop.");
    let request = press_button(&mut window, ids.forget);
    serve(&mut window, &mut read, request);
    assert!(window.grants_to_write().get(&id("laptop")).is_none());
    assert_eq!(
        window
            .agents_pane()
            .selected(window.widgets())
            .map(|agent| *agent.id()),
        Some(id("phone"))
    );
    assert_eq!(
        status(&window),
        "Forgot laptop: it gets nothing until granted again."
    );
}

#[test]
fn add_takes_a_valid_new_id_and_refuses_local_and_bad_ones() {
    let mut read = machine();
    let mut window = opened(&mut read);
    type_id(&mut window, "local");
    assert_eq!(status(&window), LOCAL_ID_LINE);
    assert!(!window.agents_pane().buttons_enabled(window.widgets())[0]);
    let field = widgets(&window).id_field;
    window.tree().set_text(field, "");
    type_id(&mut window, "x!");
    assert_eq!(status(&window), BAD_ID_LINE);
    window.tree().set_text(field, "");
    type_id(&mut window, "tv");
    let request = press(&mut window, key::ENTER, 13);
    assert_eq!(request, Some(Request::WriteGrants));
    serve(&mut window, &mut read, request);
    let tv = written(&window, "tv");
    assert!(
        Grant::ALL
            .iter()
            .all(|&grant| tv.class(grant) == ClassGrant::DENIED)
    );
    assert_eq!(window.widgets().text_of(field), Some(""));
    assert_eq!(
        window
            .agents_pane()
            .selected(window.widgets())
            .map(|agent| *agent.id()),
        Some(id("tv"))
    );
    assert_eq!(
        status(&window),
        "Added tv with no grants: choose what it may do."
    );
}

#[test]
fn a_damaged_store_lists_local_alone_and_writes_nothing() {
    let mut read = GrantsRead {
        store: StoreState::Unreadable(StoreError::Checksum),
        ..machine()
    };
    let mut window = opened(&mut read);
    let pane = window.agents_pane();
    assert_eq!(pane.row_lines(window.widgets()), ["local", "", ""]);
    assert_eq!(
        pane.buttons_enabled(window.widgets()),
        [false, false, false]
    );
    assert_eq!(
        status(&window),
        "Grants file cannot be trusted: its checksum does not match."
    );
    let radios = widgets(&window).class_radios;
    assert!(radios.iter().all(|&id| !window.widgets().is_enabled(id)));
    assert_eq!(choose(&mut window, radios[0], DENY), None);
}

#[test]
fn a_failed_write_says_why_and_keeps_the_store_s_choice() {
    let mut read = machine();
    let mut window = opened(&mut read);
    select(&mut window, 1);
    let radios = widgets(&window).class_radios;
    assert_eq!(
        choose(&mut window, radios[1], ALLOW),
        Some(Request::WriteGrants)
    );
    window.grants_written(Err(SaveError::File(STORE_TEMPORARY, Errno::EROFS)), read);
    assert!(status(&window).starts_with("Grants file not written: "));
    assert_eq!(choices(&window)[1], Some(ASK));
}

#[test]
fn without_a_disk_an_agent_s_status_says_changes_last_until_power_off() {
    let mut read = GrantsRead {
        on_disk: false,
        ..machine()
    };
    let mut window = opened(&mut read);
    assert_eq!(status(&window), LOCAL_LINE);
    select(&mut window, 1);
    assert_eq!(status(&window), NO_DISK_LINE);
}

#[test]
fn an_agent_cannot_change_grants_through_the_element_tree() {
    let mut read = machine();
    let mut window = opened(&mut read);
    let ids = widgets(&window);
    let tree = window.tree();
    assert_eq!(
        tree.act(element_of(ids.revoke_all), Action::Invoke, "", 0)
            .0,
        Outcome::NotSupported
    );
    assert_eq!(
        tree.act(element_of(ids.class_radios[0]) + 1, Action::Select, "", 0)
            .0,
        Outcome::NotSupported
    );
    assert_eq!(
        tree.act(element_of(ids.add), Action::Invoke, "", 0).0,
        Outcome::NotSupported
    );
    assert_eq!(
        tree.act(element_of(ids.list) + 2, Action::Select, "", 0).0,
        Outcome::Done
    );
    assert_eq!(
        tree.act(element_of(ids.id_field), Action::SetText, "tv", 0)
            .0,
        Outcome::Done
    );
    assert!(window.grants_to_write().get(&id("tv")).is_none());
}
