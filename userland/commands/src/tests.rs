use crate::tools::{self, Confirmation, DefaultValue, JsonType, Mapping, SHIPPED_NAMES, TOOLS};
use crate::{
    COMMANDS, CORE_VERBS, Category, Effect, Grant, KEPT_NAMES, NOUN_VERBS, Output, SHELL_BUILTINS,
    errors, find_command,
};

const ANY_WORDS: [&str; 3] = ["echo", "false", "true"];

fn words_of(words: &[&str]) -> Vec<Vec<u8>> {
    words.iter().map(|word| word.as_bytes().to_vec()).collect()
}

fn find(words: &[&str]) -> Option<&'static crate::CommandDefinition> {
    let owned = words_of(words);
    let borrowed: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
    find_command(&borrowed)
}

#[test]
fn every_command_is_a_kept_name_or_a_noun_and_a_verb() {
    for command in &COMMANDS {
        match command.words {
            [name] => assert!(KEPT_NAMES.contains(name), "{name}"),
            [noun, verb] => {
                let noun_verbs = NOUN_VERBS
                    .iter()
                    .find(|(known, _)| known == noun)
                    .map_or(&[][..], |(_, verbs)| *verbs);
                assert!(
                    CORE_VERBS.contains(verb) || noun_verbs.contains(verb),
                    "{noun} {verb}"
                );
                assert!(KEPT_NAMES.contains(noun), "{noun}");
            }
            other => panic!("{other:?} has too many words"),
        }
    }
}

#[test]
fn commands_are_unique_sorted_and_found_by_their_words() {
    for (index, command) in COMMANDS.iter().enumerate() {
        assert_eq!(find(command.words), Some(command), "{:?}", command.words);
        if let Some(previous) = index.checked_sub(1).and_then(|at| COMMANDS.get(at)) {
            assert!(previous.words < command.words, "{:?}", command.words);
        }
    }
    assert_eq!(find(&["wm"]), None);
    assert_eq!(find(&["frobnicate"]), None);
    assert_eq!(find(&["settings"]), Some(&crate::commands::SETTINGS));
}

#[test]
fn every_kept_name_has_a_definition_so_the_registry_holds_every_command() {
    for name in KEPT_NAMES {
        assert!(
            COMMANDS.iter().any(|command| command.words[0] == name),
            "{name} has no definition"
        );
    }
    for command in &COMMANDS {
        assert!(
            KEPT_NAMES.contains(&command.words[0]),
            "{:?}",
            command.words
        );
    }
}

#[test]
fn every_command_has_the_fields_the_model_requires() {
    for command in &COMMANDS {
        let name = command.words.join(" ");
        assert!(command.summary.ends_with('.'), "{name}");
        assert!(command.description.len() > command.summary.len(), "{name}");
        assert_eq!(
            command
                .errors
                .iter()
                .any(|error| error.error.code == "usage"),
            !ANY_WORDS.contains(&name.as_str()),
            "{name}"
        );
        for (index, error) in command.errors.iter().enumerate() {
            assert!(error.exit > 0, "{name}");
            assert!(
                command.errors[..index]
                    .iter()
                    .all(|other| other.error.code != error.error.code),
                "{name}"
            );
        }
        assert_eq!(
            command.has_json(),
            command.long_option(b"json").is_some(),
            "{name}"
        );
        if command.has_json() {
            assert!(
                matches!(command.outputs.json, Output::Records(_) | Output::Object(_)),
                "{name}"
            );
        }
        for (index, option) in command.options.iter().enumerate() {
            assert!(option.short.is_some() || option.long.is_some(), "{name}");
            assert!(!option.help.is_empty(), "{name}");
            for other in &command.options[..index] {
                assert!(
                    option.short.is_none() || option.short != other.short,
                    "{name}"
                );
                assert!(option.long.is_none() || option.long != other.long, "{name}");
            }
        }
    }
}

#[test]
fn examples_start_with_their_command_s_words_and_show_json_only_where_it_exists() {
    let mut examples = 0;
    for command in &COMMANDS {
        let name = command.words.join(" ");
        for example in command.examples {
            examples += 1;
            let rest = example
                .line
                .strip_prefix(name.as_str())
                .unwrap_or_else(|| panic!("{}", example.line));
            assert!(rest.is_empty() || rest.starts_with(' '), "{}", example.line);
            assert!(!example.line.contains("  "), "{}", example.line);
            assert!(!example.line.contains(['\'', '"']), "{}", example.line);
            assert!((0..=2).contains(&example.status), "{}", example.line);
            if example.line.split(' ').any(|word| word == "--json") {
                assert!(command.has_json(), "{}", example.line);
            }
        }
        let shown_by_a_fake = !matches!(
            command.words,
            ["agentlog" | "help"] | ["wm" | "app" | "ip" | "ping" | "nslookup", ..]
        );
        if command.max_effect == Effect::Read && shown_by_a_fake {
            assert!(!command.examples.is_empty(), "{name} has no example");
        }
    }
    assert!(examples >= 30, "{examples}");
}

#[test]
fn json_fields_are_snake_case_with_units_in_their_names() {
    for command in &COMMANDS {
        let (Output::Records(fields) | Output::Object(fields)) = command.outputs.json else {
            continue;
        };
        for field in fields {
            assert!(
                field
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{}",
                field.name
            );
            if field.value == crate::FieldType::Time {
                assert!(field.name.ends_with("_at"), "{}", field.name);
            }
        }
    }
}

#[test]
fn error_codes_are_unique_snake_case_and_their_fix_commands_exist() {
    for (index, error) in errors::ALL.iter().enumerate() {
        assert!(
            error
                .code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
            "{}",
            error.code
        );
        assert!(
            error.fix.ends_with('.') || error.fix.ends_with(')'),
            "{}",
            error.code
        );
        assert!(
            errors::ALL[..index]
                .iter()
                .all(|other| other.code != error.code),
            "{}",
            error.code
        );
        if let Some(fix) = error.fix_command {
            let words: Vec<&str> = fix
                .split(' ')
                .filter(|word| !word.starts_with('-'))
                .collect();
            assert!(find(&words).is_some(), "{fix}");
        }
    }
    for command in &COMMANDS {
        for declared in command.errors {
            assert!(
                errors::ALL.iter().any(|error| *error == declared.error),
                "{}",
                declared.error.code
            );
        }
    }
    assert_eq!(Category::ALL.len(), 13);
    assert_eq!(errors::DECLINED_BY_USER.code, "declined_by_user");
    assert_eq!(errors::CONFIRMATION_FAILED.code, "confirmation_failed");
}

#[test]
fn new_tool_names_join_their_command_s_words_and_shipped_names_are_kept() {
    for (index, tool) in TOOLS.iter().enumerate() {
        assert!(
            TOOLS[..index].iter().all(|other| other.name != tool.name),
            "{}",
            tool.name
        );
        assert_eq!(tools::find(tool.name), Some(tool));
        if !SHIPPED_NAMES.contains(&tool.name) {
            assert_eq!(tool.name, tool.command.join("_"));
        }
    }
    for (shipped, tool) in SHIPPED_NAMES.iter().zip(&TOOLS) {
        assert_eq!(*shipped, tool.name);
    }
}

#[test]
fn every_tool_runs_a_registry_command() {
    for tool in &TOOLS {
        match tool.command {
            [] => assert_eq!(tool.name, "run"),
            words => {
                let command = find(words).unwrap_or_else(|| panic!("{words:?}"));
                for argument in tool.arguments {
                    if let Mapping::Option(option) = argument.maps_to {
                        let found = match option.strip_prefix("--") {
                            Some(long) => command.long_option(long.as_bytes()),
                            None => command.short_option(option.as_bytes()[1]),
                        };
                        assert!(found.is_some(), "{} {option}", tool.name);
                    }
                }
                for fixed in tool.fixed.iter().filter(|word| word.starts_with('-')) {
                    let found = match fixed.strip_prefix("--") {
                        Some(long) => command.long_option(long.as_bytes()),
                        None => command.short_option(fixed.as_bytes()[1]),
                    };
                    assert!(found.is_some(), "{} {fixed}", tool.name);
                }
            }
        }
    }
}

#[test]
fn every_tool_s_annotations_grant_and_confirmation_agree() {
    for tool in &TOOLS {
        assert_eq!(
            tool.read_only(),
            tool.max_effect == Effect::Read,
            "{}",
            tool.name
        );
        match tool.max_effect {
            Effect::Read => assert!(matches!(tool.grant, None | Some(Grant::View))),
            _ => assert!(matches!(tool.grant, Some(grant) if grant != Grant::View)),
        }
        if let Some(grant) = tool.grant {
            assert!(
                tool.description
                    .contains(&format!("{} grant", grant.name())),
                "{}",
                tool.name
            );
        }
        if tool.confirm != Confirmation::Never {
            assert!(tool.destructive(), "{}", tool.name);
            assert!(tool.takes("confirm"), "{}", tool.name);
        }
        assert!(!tool.open_world, "{}", tool.name);
        if tool.max_effect == Effect::Control {
            assert!(!tool.idempotent, "{}", tool.name);
        }
    }
}

#[test]
fn the_new_window_tools_change_windows_under_the_run_grant() {
    for name in [
        "wm_focus",
        "wm_move",
        "wm_size",
        "wm_minimise",
        "wm_maximise",
        "wm_restore",
        "wm_close",
    ] {
        let tool = tools::find(name).unwrap();
        assert_eq!(tool.max_effect, Effect::Change, "{name}");
        assert_eq!(tool.grant, Some(Grant::Run), "{name}");
        assert_eq!(tool.arguments[0].name, "window", "{name}");
        assert!(!tool.destructive() && !tool.read_only(), "{name}");
    }
}

#[test]
fn arguments_are_consistent_with_their_types() {
    for tool in &TOOLS {
        for argument in tool.arguments {
            let label = format!("{} {}", tool.name, argument.name);
            match argument.default {
                DefaultValue::None => {}
                DefaultValue::Text(_) => assert_eq!(argument.json, JsonType::String, "{label}"),
                DefaultValue::Integer(value) => {
                    assert_eq!(argument.json, JsonType::Integer, "{label}");
                    assert!(argument.minimum.is_none_or(|minimum| minimum <= value));
                    assert!(argument.maximum.is_none_or(|maximum| value <= maximum));
                }
                DefaultValue::Boolean(_) => assert_eq!(argument.json, JsonType::Boolean, "{label}"),
                DefaultValue::EmptyList => {
                    assert_eq!(argument.json, JsonType::StringArray, "{label}")
                }
            }
            assert!(
                !(argument.required && argument.default != DefaultValue::None),
                "{label}"
            );
            if argument.minimum.is_some() || argument.maximum.is_some() {
                assert_eq!(argument.json, JsonType::Integer, "{label}");
            }
            if argument.maps_to == Mapping::Confirm {
                assert_eq!(argument.name, "confirm", "{label}");
            }
        }
    }
}

#[test]
fn the_kept_names_and_builtins_are_listed_once() {
    for (index, name) in KEPT_NAMES.iter().enumerate() {
        assert!(!KEPT_NAMES[..index].contains(name), "{name}");
    }
    for (index, name) in SHELL_BUILTINS.iter().enumerate() {
        assert!(!SHELL_BUILTINS[..index].contains(name), "{name}");
    }
    assert!(SHELL_BUILTINS.contains(&"help"));
}
