use super::*;
use eggos_mcp::grant_prompt::read_question;
use eggos_mcp::grants::AgentName;
use eggos_toolkit::elements::{Action, Outcome};
use eggos_toolkit::input::Modifiers;
use eggos_toolkit::tree::element_of;

const QUESTION: &[u8] = br#"{"agent":"phone","name":"Claude on my phone","tool":"tee","title":"Write a file","class":"write","destructive":false,"arguments":{"path":"/home/a","content":"40 bytes"},"timeout_s":90}"#;

fn prompt_for(line: &[u8]) -> GrantPrompt {
    let question = read_question(line).expect("a question");
    let mut prompt = GrantPrompt::new(&question, 0);
    prompt.tree().resize(WIDTH as i32, HEIGHT as i32);
    prompt
}

fn key(prompt: &mut GrantPrompt, code: u16, shift: bool) {
    let modifiers = match shift {
        true => Modifiers(Modifiers::SHIFT),
        false => Modifiers::default(),
    };
    for pressed in [true, false] {
        prompt.handle(
            Input::Key {
                code,
                pressed,
                modifiers,
                character: 0,
            },
            0,
        );
    }
}

#[test]
fn the_prompt_places_its_countdown_and_buttons_with_deny_focused() {
    let mut prompt = prompt_for(QUESTION);
    let countdown = prompt.countdown_id();
    let buttons = prompt.button_ids();
    let tree = prompt.tree();
    assert_eq!(
        tree.text_of(countdown),
        Some("Denied in 1:30 if you do nothing.")
    );
    let countdown_rect = tree.rect(countdown).unwrap();
    assert_eq!((countdown_rect.x, countdown_rect.y), (16, 132));
    let places: Vec<(i32, i32, i32, i32)> = buttons
        .iter()
        .map(|&id| {
            let rect = tree.rect(id).unwrap();
            (rect.x, rect.y, rect.width, rect.height)
        })
        .collect();
    assert_eq!(
        places,
        [
            (36, 156, 96, 28),
            (140, 156, 116, 28),
            (264, 156, 96, 28),
            (368, 156, 96, 28)
        ]
    );
    assert_eq!(tree.focus(), Some(buttons[3]));
    assert!(buttons.iter().all(|&id| tree.is_shown(id)));
    assert!(buttons.iter().all(|&id| tree.is_person_only(id)));
}

#[test]
fn the_arguments_are_key_value_lines_path_first_and_cut() {
    let question = read_question(QUESTION).unwrap();
    assert_eq!(
        argument_lines(question.arguments),
        [
            String::from("path: /home/a"),
            String::from("content: 40 bytes"),
            String::new()
        ]
    );
    let long = format!(
        r#"{{"agent":"a","tool":"tee","title":"T","class":"write","arguments":{{"path":"/home/{}","append":false,"x":1,"y":2}}}}"#,
        "n".repeat(100)
    );
    let lines = argument_lines(read_question(long.as_bytes()).unwrap().arguments);
    assert!(lines[0].starts_with("path: /home/nnn"));
    assert!(lines[0].ends_with("..."));
    assert!(lines[0].len() <= 64);
    assert_eq!(lines[1], "append: false");
    assert_eq!(lines[2], "x: 1");
}

#[test]
fn enter_and_esc_deny_and_space_presses_the_focused_button() {
    for code in [key::ENTER, key::ESC] {
        let mut prompt = prompt_for(QUESTION);
        key(&mut prompt, key::TAB, true);
        key(&mut prompt, code, false);
        assert_eq!(prompt.answer(), Some(PromptAnswer::Deny));
    }
    let mut prompt = prompt_for(QUESTION);
    key(&mut prompt, key::TAB, true);
    assert_eq!(prompt.tree().focus(), Some(prompt.button_ids()[2]));
    key(&mut prompt, key::SPACE, false);
    assert_eq!(prompt.answer(), Some(PromptAnswer::Once));
    let mut prompt = prompt_for(QUESTION);
    prompt.close_requested();
    assert_eq!(prompt.answer(), Some(PromptAnswer::Deny));
}

#[test]
fn always_is_hidden_for_run_and_destructive_calls_and_tab_skips_it() {
    for line in [
        QUESTION.to_vec(),
        String::from_utf8(QUESTION.to_vec())
            .unwrap()
            .replace(r#""class":"write""#, r#""class":"run""#)
            .into_bytes(),
        String::from_utf8(QUESTION.to_vec())
            .unwrap()
            .replace(r#""destructive":false"#, r#""destructive":true"#)
            .into_bytes(),
    ] {
        let mut prompt = prompt_for(&line);
        let buttons = prompt.button_ids();
        let offered = line == QUESTION;
        assert_eq!(prompt.tree().is_shown(buttons[0]), offered);
        for _ in 0..3 {
            key(&mut prompt, key::TAB, true);
        }
        let expected = match offered {
            true => buttons[0],
            false => buttons[3],
        };
        assert_eq!(prompt.tree().focus(), Some(expected));
    }
}

#[test]
fn the_countdown_reads_1_29_after_a_second_and_times_out_after_90() {
    let mut prompt = prompt_for(QUESTION);
    assert_eq!(prompt.deadline(0), 1000);
    assert_eq!(prompt.deadline(1500), 2000);
    prompt.tick(1000);
    let countdown = prompt.countdown_id();
    assert_eq!(
        prompt.tree().text_of(countdown),
        Some("Denied in 1:29 if you do nothing.")
    );
    assert_eq!(prompt.answer(), None);
    prompt.tick(89_999);
    assert_eq!(prompt.answer(), None);
    prompt.tick(90_000);
    assert_eq!(prompt.answer(), Some(PromptAnswer::Timeout));
    assert_eq!(
        prompt.tree().text_of(countdown),
        Some("Denied in 0:00 if you do nothing.")
    );
}

#[test]
fn no_agent_action_answers_the_prompt() {
    let mut prompt = prompt_for(QUESTION);
    for id in prompt.button_ids() {
        for action in [Action::Invoke, Action::Focus] {
            let (outcome, response) = prompt.tree().act(element_of(id), action, "", 0);
            assert_eq!(outcome, Outcome::NotSupported);
            prompt.respond(response);
        }
    }
    assert_eq!(prompt.answer(), None);
}

#[test]
fn always_writes_allow_for_the_agent_s_class_or_nothing() {
    let phone = AgentId::new(b"phone").unwrap();
    let mut store = GrantStore::new();
    store
        .add(phone, AgentName::new(b"Claude on my phone").unwrap())
        .unwrap()
        .set(Grant::Write, Permission::Ask, None);
    let allowed = allowed_always(StoreState::Loaded(store), &phone, Grant::Write).unwrap();
    assert_eq!(
        allowed.get(&phone).unwrap().class(Grant::Write).permission,
        Permission::Allow
    );
    let tablet = AgentId::new(b"tablet").unwrap();
    assert!(allowed_always(StoreState::Loaded(store), &tablet, Grant::Write).is_none());
    assert!(allowed_always(StoreState::Missing, &phone, Grant::Write).is_none());
}

#[test]
fn hidden_characters_in_names_are_shown_as_question_marks() {
    let line = br#"{"agent":"a","name":"x\u001b[2Jy","tool":"tee","title":"T","class":"write","arguments":{}}"#;
    let question = read_question(line).unwrap();
    assert_eq!(shown_text(question.name), "x?[2Jy");
}
