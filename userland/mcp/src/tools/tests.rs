use super::*;
use crate::json::{self, Value};

const SHIPPED_SCHEMAS: [(&str, &str); 26] = [
    (
        "ls",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"An absolute path in /home, /media/host or /work.","default":"/home"},"limit":{"type":"integer","minimum":1,"maximum":500,"description":"At most this many items; when more follow, the result's structuredContent.next_cursor names where the next page starts: call again with it as after."},"after":{"type":"string","pattern":"^[A-Za-z0-9._~@+,=:%][A-Za-z0-9._~@+,=:%-]*$","minLength":1,"maxLength":765,"description":"The next_cursor a page ended with: the items after it."}},"additionalProperties":false}"#,
    ),
    (
        "cat",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"An absolute path in /home, /media/host or /work."}},"required":["path"],"additionalProperties":false}"#,
    ),
    (
        "ps",
        r#"{"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":100,"description":"At most this many items; when more follow, the result's structuredContent.next_cursor names where the next page starts: call again with it as after."},"after":{"type":"string","pattern":"^[A-Za-z0-9._~@+,=:%][A-Za-z0-9._~@+,=:%-]*$","minLength":1,"maxLength":765,"description":"The next_cursor a page ended with: the items after it."}},"additionalProperties":false}"#,
    ),
    (
        "wm_list",
        r#"{"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":100,"description":"At most this many items; when more follow, the result's structuredContent.next_cursor names where the next page starts: call again with it as after."},"after":{"type":"string","pattern":"^[A-Za-z0-9._~@+,=:%][A-Za-z0-9._~@+,=:%-]*$","minLength":1,"maxLength":765,"description":"The next_cursor a page ended with: the items after it."}},"additionalProperties":false}"#,
    ),
    (
        "app_list",
        r#"{"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":100,"description":"At most this many items; when more follow, the result's structuredContent.next_cursor names where the next page starts: call again with it as after."},"after":{"type":"string","pattern":"^[A-Za-z0-9._~@+,=:%][A-Za-z0-9._~@+,=:%-]*$","minLength":1,"maxLength":765,"description":"The next_cursor a page ended with: the items after it."}},"additionalProperties":false}"#,
    ),
    (
        "clock_get",
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
    ),
    (
        "net_status",
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
    ),
    (
        "net_ping",
        r#"{"type":"object","properties":{"host":{"type":"string","description":"An IPv4 address such as 10.0.2.2, or a host name."},"count":{"type":"integer","minimum":1,"maximum":5,"default":4,"description":"How many echo requests to send, one a second."}},"required":["host"],"additionalProperties":false}"#,
    ),
    (
        "net_lookup",
        r#"{"type":"object","properties":{"host":{"type":"string","description":"The host name to look up, such as example.com."}},"required":["host"],"additionalProperties":false}"#,
    ),
    (
        "tee",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"An absolute path below /home/root, outside its Trash."},"content":{"type":"string","description":"The text to write, at most 128 KiB."},"content_base64":{"type":"string","description":"The bytes to write, in base64, instead of content."},"append":{"type":"boolean","description":"Add to the end of the file instead of replacing it.","default":false},"confirm":{"type":"boolean","description":"Set to true to replace an existing file when the client cannot ask the user: a guard against accidents, not the user's approval.","default":false},"dry_run":{"type":"boolean","description":"Set to true to see what the call would do: nothing is changed and the user is not asked.","default":false}},"required":["path"],"additionalProperties":false}"#,
    ),
    (
        "trash",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"An absolute path below /home/root, outside its Trash."},"confirm":{"type":"boolean","description":"Set to true when the client cannot ask the user: a guard against accidents, not the user's approval.","default":false},"dry_run":{"type":"boolean","description":"Set to true to see what the call would do: nothing is changed and the user is not asked.","default":false}},"required":["path"],"additionalProperties":false}"#,
    ),
    (
        "fetch",
        r#"{"type":"object","properties":{"url":{"type":"string","pattern":"^[Hh][Tt][Tt][Pp][Ss]?://","description":"The http:// or https:// URL to download, at most 2048 bytes."},"path":{"type":"string","description":"The file to save it as: an absolute path below /home/root, outside its Trash."},"confirm":{"type":"boolean","description":"Set to true to replace an existing file when the client cannot ask the user: a guard against accidents, not the user's approval.","default":false}},"required":["url","path"],"additionalProperties":false}"#,
    ),
    (
        "run",
        r#"{"type":"object","properties":{"program":{"type":"string","description":"The program's absolute path, such as /bin/uname."},"arguments":{"type":"array","items":{"type":"string"},"description":"Its arguments, at most 32.","default":[]},"timeout_seconds":{"type":"integer","minimum":1,"maximum":60,"default":10}},"required":["program"],"additionalProperties":false}"#,
    ),
    (
        "clock_set",
        r#"{"type":"object","properties":{"time":{"type":"string","pattern":"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$","description":"The new date and time in UTC, as YYYY-MM-DDTHH:MM:SSZ."},"confirm":{"type":"boolean","description":"Set to true when the client cannot ask the user: a guard against accidents, not the user's approval.","default":false}},"required":["time"],"additionalProperties":false}"#,
    ),
    (
        "clock_sync",
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
    ),
    (
        "app_install",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"The app's file, bundle or .zip in /media/host."},"entry":{"type":"string","description":"For a .zip holding several programs: the path inside it of the one to install (an .app bundle's own path names its program)."},"arguments":{"type":"string","description":"Words the app opens with, as app install --arguments takes them: letters, digits, spaces and -_./:=+,@% only, at most 128 bytes."},"environment":{"type":"string","description":"NAME=value words set in the app's environment when it opens, separated by spaces, as app install --environment takes them: values of letters, digits and -_./:=+,@% only, at most 128 bytes in all."}},"required":["path"],"additionalProperties":false}"#,
    ),
    (
        "app_open",
        r#"{"type":"object","properties":{"name":{"type":"string","description":"The app's name, as app_list shows it, or Terminal, Files, Settings or Text Editor."}},"required":["name"],"additionalProperties":false}"#,
    ),
    (
        "edit_open",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"The text file's absolute path, in /home, /media/host or /work."}},"required":["path"],"additionalProperties":false}"#,
    ),
    (
        "app_uninstall",
        r#"{"type":"object","properties":{"name":{"type":"string","description":"The app's name, as app_list shows it."},"confirm":{"type":"boolean","description":"Set to true when the client cannot ask the user: a guard against accidents, not the user's approval.","default":false},"dry_run":{"type":"boolean","description":"Set to true to see what the call would do: nothing is changed and the user is not asked.","default":false}},"required":["name"],"additionalProperties":false}"#,
    ),
    (
        "wm_capture",
        r#"{"type":"object","properties":{"window":{"type":"integer","minimum":0,"maximum":999999999,"description":"The window's id, as wm_list shows it."},"max_width":{"type":"integer","minimum":16,"maximum":1568,"default":1024,"description":"The widest image wanted, in pixels; a wider window is shrunk by a whole factor."}},"required":["window"],"additionalProperties":false}"#,
    ),
    (
        "wm_click",
        r#"{"type":"object","properties":{"window":{"type":"integer","minimum":0,"maximum":999999999,"description":"The window's id, as wm_list shows it."},"x":{"type":"integer","minimum":0,"maximum":65535,"description":"Window pixels from the content's left edge."},"y":{"type":"integer","minimum":0,"maximum":65535,"description":"Window pixels from the content's top edge."},"button":{"type":"string","enum":["left","right","middle"],"default":"left"},"double":{"type":"boolean","description":"Set to true for a double-click: two clicks in one step, which apps count as one double-click.","default":false}},"required":["window","x","y"],"additionalProperties":false}"#,
    ),
    (
        "wm_type",
        r#"{"type":"object","properties":{"window":{"type":"integer","minimum":0,"maximum":999999999,"description":"The window's id, as wm_list shows it."},"text":{"type":"string","minLength":1,"maxLength":96,"description":"Printable ASCII text."}},"required":["window","text"],"additionalProperties":false}"#,
    ),
    (
        "wm_press",
        r#"{"type":"object","properties":{"window":{"type":"integer","minimum":0,"maximum":999999999,"description":"The window's id, as wm_list shows it."},"keys":{"type":"string","minLength":1,"maxLength":32,"description":"One key after any modifiers, such as Enter or ctrl+s."}},"required":["window","keys"],"additionalProperties":false}"#,
    ),
    (
        "wm_scroll",
        r#"{"type":"object","properties":{"window":{"type":"integer","minimum":0,"maximum":999999999,"description":"The window's id, as wm_list shows it."},"dy":{"type":"integer","minimum":-16,"maximum":16,"description":"Notches, up when positive; 0 only with dx."},"x":{"type":"integer","minimum":0,"maximum":65535,"description":"Window pixels from the content's left edge; with y."},"y":{"type":"integer","minimum":0,"maximum":65535,"description":"Window pixels from the content's top edge; with x."},"dx":{"type":"integer","minimum":-16,"maximum":16,"default":0,"description":"Notches sideways, right when positive."}},"required":["window","dy"],"additionalProperties":false}"#,
    ),
    (
        "wm_menu_list",
        r#"{"type":"object","properties":{"window":{"type":"integer","minimum":0,"maximum":999999999,"description":"The window's id, as wm_list shows it."}},"required":["window"],"additionalProperties":false}"#,
    ),
    (
        "wm_menu_choose",
        r#"{"type":"object","properties":{"window":{"type":"integer","minimum":0,"maximum":999999999,"description":"The window's id, as wm_list shows it."},"menu":{"type":"integer","minimum":0,"maximum":5,"description":"The menu's number, as wm_menu_list shows it."},"entry":{"type":"integer","minimum":0,"maximum":15,"description":"The item's number in that menu, as wm_menu_list shows it."},"label":{"type":"string","maxLength":64,"description":"The item's label, as wm_menu_list shows it (not case-sensitive); the choice is refused if the entry has another."}},"required":["window","menu","entry"],"additionalProperties":false}"#,
    ),
];

fn schema_of(tool: &Tool) -> String {
    let mut buffer = vec![0u8; 8 * 1024];
    let mut out = JsonWriter::new(&mut buffer);
    write_input_schema(&mut out, tool.definition);
    assert!(!out.overflowed());
    String::from_utf8(out.as_bytes().to_vec()).unwrap()
}

#[test]
fn every_shipped_tool_lists_the_schema_it_shipped_with() {
    for ((name, shipped), tool) in SHIPPED_SCHEMAS.iter().zip(&TOOLS) {
        assert_eq!(tool.name, *name);
        assert_eq!(schema_of(tool), *shipped, "{name}");
    }
}

#[test]
fn every_schema_is_an_object_schema_naming_exactly_the_tool_s_arguments() {
    for tool in &TOOLS {
        let text = schema_of(tool);
        let schema = json::parse(text.as_bytes()).unwrap().value;
        assert!(
            schema
                .get("type")
                .unwrap()
                .as_str()
                .unwrap()
                .equals("object")
        );
        assert_eq!(schema.get("additionalProperties"), Some(Value::Bool(false)));
        let properties = schema.get("properties").unwrap().as_object().unwrap();
        let names: Vec<String> = properties
            .members()
            .map(|(name, _)| name.chars().collect())
            .collect();
        let arguments: Vec<&str> = tool
            .definition
            .arguments
            .iter()
            .map(|argument| argument.name)
            .collect();
        assert_eq!(names, arguments, "{}", tool.name);
        if let Operand::Path(path) = tool.twin.operand {
            assert!(tool.takes(path.name), "{}", tool.name);
            let required = schema.get("required").is_some();
            assert_eq!(required, path.default.is_none(), "{}", tool.name);
            if let Some(default) = path.default {
                let given = properties.get(path.name).unwrap().get("default").unwrap();
                assert!(given.as_str().unwrap().equals(default));
            }
        }
    }
}

#[test]
fn each_twin_runs_its_registry_command_with_its_fixed_options() {
    for tool in &TOOLS {
        let definition = tool.definition;
        match (tool.twin.program, definition.command) {
            (Program::FromArguments, []) => assert_eq!(tool.kind, Kind::Run),
            (Program::Fixed(program), [name, verb @ ..]) => {
                assert_eq!(program.strip_prefix("/bin/"), Some(*name), "{}", tool.name);
                let expected: Vec<&str> = verb.iter().chain(definition.fixed).copied().collect();
                assert!(
                    tool.twin.words.starts_with(&expected),
                    "{}: {:?} does not start with {expected:?}",
                    tool.name,
                    tool.twin.words
                );
            }
            (program, command) => panic!("{}: {program:?} and {command:?}", tool.name),
        }
    }
}

#[test]
fn names_follow_mcp_s_rules_and_are_unique() {
    for (index, tool) in TOOLS.iter().enumerate() {
        assert!(!tool.name.is_empty() && tool.name.len() <= 128);
        assert!(
            tool.name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
        );
        assert!(TOOLS[..index].iter().all(|other| other.name != tool.name));
        match tool.twin.program {
            Program::Fixed(program) => assert!(program.starts_with("/bin/")),
            Program::FromArguments => assert_eq!(tool.kind, Kind::Run),
        }
        assert_eq!(find(tool.name), Some(tool));
        assert_eq!(tool.name, tool.definition.name);
    }
    assert_eq!(find("rm"), None);
    for (registry, tool) in eggos_commands::TOOLS.iter().zip(&TOOLS) {
        assert_eq!(registry.name, tool.name);
    }
    assert_eq!(eggos_commands::TOOLS.len(), TOOLS.len());
}

#[test]
fn the_window_tools_run_wm_and_input_needs_run() {
    for tool in TOOLS
        .iter()
        .filter(|tool| tool.twin.operand == Operand::Window)
    {
        assert_eq!(
            tool.twin.program,
            Program::Fixed("/bin/wm"),
            "{}",
            tool.name
        );
        assert!(tool.name.starts_with("wm_"), "{}", tool.name);
        assert_eq!(
            tool.definition
                .arguments
                .first()
                .map(|argument| argument.name),
            Some("window"),
            "{}",
            tool.name
        );
        assert_eq!(tool.confirmation, Confirmation::Never, "{}", tool.name);
        let expected = match tool.kind {
            Kind::WmCapture | Kind::WmMenuList | Kind::WmShow => Grant::View,
            _ => Grant::Run,
        };
        assert_eq!(tool.grant, Some(expected), "{}", tool.name);
    }
    let capture = find("wm_capture").unwrap();
    assert_eq!(capture.twin.words, ["capture", "-o", CAPTURE_PATH]);
    assert!(schema_of(capture).contains(&format!(
        r#""minimum":{MIN_IMAGE_WIDTH},"maximum":{MAX_IMAGE_WIDTH},"default":{DEFAULT_IMAGE_WIDTH}"#
    )));
    assert!(CAPTURE_PATH.starts_with("/run/"));
    assert_eq!(find("wm_menu_list").unwrap().twin.words, ["menu"]);
    assert_eq!(
        find("wm_menu_choose").unwrap().twin.words,
        ["menu", "--choose"]
    );
    for (name, verb) in [
        ("wm_focus", "focus"),
        ("wm_move", "move"),
        ("wm_size", "size"),
        ("wm_minimise", "minimise"),
        ("wm_maximise", "maximise"),
        ("wm_restore", "restore"),
        ("wm_close", "close"),
    ] {
        let tool = find(name).unwrap();
        assert_eq!(tool.twin.words, [verb], "{name}");
        assert_eq!(
            tool.hints,
            Hints {
                read_only: false,
                destructive: false,
                idempotent: name != "wm_close",
                open_world: false,
            },
            "{name}"
        );
    }
}

#[test]
fn changing_needs_a_grant_and_reading_needs_none_or_view() {
    for tool in &TOOLS {
        match tool.hints.read_only {
            true => assert!(
                matches!(tool.grant, None | Some(Grant::View)),
                "{}",
                tool.name
            ),
            false => assert!(
                matches!(tool.grant, Some(grant) if grant != Grant::View),
                "{}",
                tool.name
            ),
        }
        if let Some(grant) = tool.grant {
            assert!(
                tool.definition
                    .description
                    .contains(&format!("{} grant", grant.name()))
            );
        }
        if tool.confirmation != Confirmation::Never {
            assert!(tool.hints.destructive, "{}", tool.name);
            assert!(tool.takes("confirm"), "{}", tool.name);
        }
    }
}

#[test]
fn the_shipped_tools_keep_their_annotations() {
    let shipped = |read_only, destructive, idempotent| Hints {
        read_only,
        destructive,
        idempotent,
        open_world: false,
    };
    for tool in &TOOLS[..SHIPPED_SCHEMAS.len()] {
        let expected = match tool.name {
            "tee" | "fetch" | "clock_set" => shipped(false, true, true),
            "trash" | "run" | "app_uninstall" => shipped(false, true, false),
            "wm_click" | "wm_type" | "wm_press" | "wm_scroll" | "wm_menu_choose" => {
                shipped(false, true, false)
            }
            "clock_sync" => shipped(false, false, true),
            "app_install" | "app_open" | "edit_open" => shipped(false, false, false),
            _ => shipped(true, false, true),
        };
        assert_eq!(tool.hints, expected, "{}", tool.name);
    }
}

#[test]
fn the_list_is_valid_json_with_annotations() {
    let mut buffer = vec![0u8; 128 * 1024];
    let mut out = JsonWriter::new(&mut buffer);
    write_list(&mut out);
    assert!(!out.overflowed());
    let list = json::parse(out.as_bytes()).unwrap().value;
    let Some(Value::Array(tools)) = list.get("tools") else {
        panic!("tools is an array");
    };
    assert_eq!(tools.elements().count(), TOOLS.len());
    for (tool, listed) in TOOLS.iter().zip(tools.elements()) {
        let annotations = listed.get("annotations").unwrap();
        assert_eq!(
            annotations.get("readOnlyHint"),
            Some(Value::Bool(tool.hints.read_only))
        );
        assert_eq!(
            annotations.get("destructiveHint"),
            Some(Value::Bool(tool.hints.destructive))
        );
        assert_eq!(annotations.get("openWorldHint"), Some(Value::Bool(false)));
        assert!(listed.get("inputSchema").unwrap().as_object().is_some());
    }
}

#[test]
fn ln_makes_links_below_the_home_folder_with_its_twin_never_replacing() {
    let tool = find("ln").unwrap();
    assert_eq!(tool.kind, Kind::Ln);
    assert_eq!(tool.twin.program, Program::Fixed("/bin/ln"));
    assert_eq!(tool.twin.words, ["-T"]);
    let Operand::Path(link) = tool.twin.operand else {
        panic!("ln's operand is the link's path");
    };
    assert_eq!(
        (link.name, link.roots, link.strictly, link.excluded),
        (
            "link",
            &crate::paths::WRITE_ROOTS[..],
            true,
            &[crate::paths::TRASH][..]
        )
    );
    assert_eq!(tool.grant, Some(Grant::Write));
    assert_eq!(tool.confirmation, Confirmation::Never);
    assert_eq!(
        tool.hints,
        Hints {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        }
    );
    assert_eq!(
        schema_of(tool),
        r#"{"type":"object","properties":{"target":{"type":"string","minLength":1,"maxLength":4095,"description":"What the link points to: for a symbolic link any path text (relative to the link's folder, not starting with -), for a hard link an absolute path of a file below /home/root."},"link":{"type":"string","description":"The link to make: an absolute path below /home/root, outside its Trash, where nothing is yet."},"symbolic":{"type":"boolean","description":"true for a symbolic link, false for a hard link.","default":true}},"required":["target","link"],"additionalProperties":false}"#
    );
    let hard = crate::tools::HARD_LINK_TARGET;
    assert_eq!((hard.name, hard.roots), ("target", link.roots));
    assert!(!tool.takes("confirm") && !tool.takes("dry_run"));
}
