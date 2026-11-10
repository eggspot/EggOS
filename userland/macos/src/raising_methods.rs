const SOURCES: [(&str, &str); 20] = [
    ("foundation/array.rs", include_str!("foundation/array.rs")),
    ("foundation/bundle.rs", include_str!("foundation/bundle.rs")),
    ("foundation/data.rs", include_str!("foundation/data.rs")),
    ("foundation/date.rs", include_str!("foundation/date.rs")),
    (
        "foundation/description.rs",
        include_str!("foundation/description.rs"),
    ),
    (
        "foundation/dictionary.rs",
        include_str!("foundation/dictionary.rs"),
    ),
    ("foundation/error.rs", include_str!("foundation/error.rs")),
    (
        "foundation/exception.rs",
        include_str!("foundation/exception.rs"),
    ),
    (
        "foundation/file_manager.rs",
        include_str!("foundation/file_manager.rs"),
    ),
    (
        "foundation/notification.rs",
        include_str!("foundation/notification.rs"),
    ),
    ("foundation/number.rs", include_str!("foundation/number.rs")),
    (
        "foundation/process_info.rs",
        include_str!("foundation/process_info.rs"),
    ),
    ("foundation/set.rs", include_str!("foundation/set.rs")),
    ("foundation/string.rs", include_str!("foundation/string.rs")),
    ("foundation/timer.rs", include_str!("foundation/timer.rs")),
    ("foundation/uuid.rs", include_str!("foundation/uuid.rs")),
    ("objc/block.rs", include_str!("objc/block.rs")),
    ("objc/message.rs", include_str!("objc/message.rs")),
    (
        "objc/reference_count.rs",
        include_str!("objc/reference_count.rs"),
    ),
    ("objc/root.rs", include_str!("objc/root.rs")),
];

const RAISES_OR_CALLS_A_BLOCK: [&str; 2] = ["raise_named(", "invoke_address("];

const VARIADIC_ENTRIES_A_GUARD_STUB_CANNOT_WRAP: [&str; 2] =
    ["appendFormat:", "dictionaryWithObjectsAndKeys:"];

struct Function<'s> {
    name: &'s str,
    entry: bool,
    body: Vec<&'s str>,
}

fn function_name(line: &str) -> Option<&str> {
    let mut rest = line;
    for prefix in ["pub(crate) ", "pub ", "unsafe ", "extern \"C\" "] {
        rest = rest.strip_prefix(prefix).unwrap_or(rest);
    }
    let rest = rest.strip_prefix("fn ")?;
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
    Some(&rest[..end])
}

fn functions(source: &str) -> Vec<Function<'_>> {
    let mut found = Vec::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        let Some(name) = function_name(line) else {
            continue;
        };
        let body = match line.trim_end().ends_with('}') {
            true => Vec::new(),
            false => lines.by_ref().take_while(|line| *line != "}").collect(),
        };
        found.push(Function {
            name,
            entry: line.contains("extern \"C\" fn "),
            body,
        });
    }
    found
}

fn calls(body: &[&str], name: &str) -> bool {
    body.iter().any(|line| {
        line.match_indices(name).any(|(at, _)| {
            let before = line[..at].chars().next_back();
            let called = line[at + name.len()..].starts_with('(');
            called && !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        })
    })
}

fn raising<'s>(functions: &[Function<'s>]) -> Vec<&'s str> {
    let mut names: Vec<&str> = functions
        .iter()
        .filter(|function| {
            function.body.iter().any(|line| {
                RAISES_OR_CALLS_A_BLOCK
                    .iter()
                    .any(|call| line.contains(call))
            })
        })
        .map(|function| function.name)
        .collect();
    loop {
        let more: Vec<&str> = functions
            .iter()
            .filter(|function| !names.contains(&function.name))
            .filter(|function| names.iter().any(|name| calls(&function.body, name)))
            .map(|function| function.name)
            .collect();
        if more.is_empty() {
            return names;
        }
        names.extend(more);
    }
}

fn stub_targets<'s>(source: &'s str, macro_name: &str) -> Vec<(&'s str, &'s str)> {
    source
        .match_indices(macro_name)
        .filter_map(|(at, _)| {
            let rest = &source[at + macro_name.len()..];
            let end = rest.find([',', ')'])?;
            let (stub, target) = rest[..end].split_once(" => ")?;
            Some((stub.trim(), target.trim()))
        })
        .collect()
}

fn table_entries(source: &str) -> Vec<(&str, &str)> {
    source
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix('"')?;
            let (selector, target) = rest.split_once("\" => ")?;
            let target = target.trim_end_matches(',').rsplit("::").next()?;
            Some((selector, target))
        })
        .collect()
}

fn unguarded_raising_methods(source: &str) -> Vec<String> {
    let source = source
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .unwrap_or(source);
    let functions = functions(source);
    let raising = raising(&functions);
    let variadic = stub_targets(source, "variadic!(");
    let mut unguarded = Vec::new();
    for (selector, target) in table_entries(source) {
        let wrapped = variadic
            .iter()
            .find(|(stub, _)| *stub == target)
            .map(|(_, wrapped)| *wrapped);
        let entry = functions
            .iter()
            .any(|function| function.entry && function.name == target);
        let raises = match wrapped {
            Some(wrapped) => {
                raising.contains(&wrapped)
                    && !VARIADIC_ENTRIES_A_GUARD_STUB_CANNOT_WRAP.contains(&selector)
            }
            None => entry && raising.contains(&target),
        };
        if raises {
            unguarded.push(format!("{selector} => {target}"));
        }
    }
    unguarded
}

#[test]
fn every_method_that_raises_or_calls_a_block_is_entered_through_a_guard_stub() {
    let mut unguarded = Vec::new();
    for (file, source) in SOURCES {
        for method in unguarded_raising_methods(source) {
            unguarded.push(format!("{file}: {method}"));
        }
    }
    assert_eq!(unguarded, Vec::<String>::new());
}

#[test]
fn the_scan_finds_the_raising_methods_through_their_helpers() {
    let (_, string) = SOURCES
        .iter()
        .find(|(file, _)| *file == "foundation/string.rs")
        .unwrap();
    let raising = raising(&functions(string));
    for name in ["insert_string", "append_string", "character_at_index"] {
        assert!(raising.contains(&name), "{name}");
    }
    assert!(
        stub_targets(string, "guard!(").contains(&("eggos_guarded_insert_string", "insert_string"))
    );
}

#[test]
fn an_unguarded_raising_method_is_reported_and_a_guarded_one_is_not() {
    let source = r#"
fn fail(this: Id) -> ! {
    super::raise_named(b"NSRangeException", &[b"out of range"])
}

fn check(this: Id, index: usize) {
    if index > 3 {
        fail(this);
    }
}

extern "C" fn insert(this: Id, _: Sel, index: usize) {
    check(this, index);
}

extern "C" fn remove(this: Id, _: Sel, index: usize) {
    check(this, index);
}

extern "C" fn count(_: Id, _: Sel) -> usize { 3 }

extern "C" fn each(this: Id, _: Sel, block: Id) {
    let function: extern "C" fn(Id) = unsafe { core::mem::transmute(invoke_address(block)) };
    function(block);
}

guard!(eggos_guarded_remove => remove);

pub fn define() {
    let instance = methods![
        "insertAt:" => insert,
        "removeAt:" => eggos_guarded_remove,
        "count" => count,
        "each:" => each,
    ];
}
"#;
    assert_eq!(
        unguarded_raising_methods(source),
        ["insertAt: => insert", "each: => each"]
    );
}
