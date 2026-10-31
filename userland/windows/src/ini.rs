//! Initialization files (`.ini`) as `GetPrivateProfileString` and `WritePrivateProfileString`
//! read and change them (D178): text in UTF-16 units, `[section]` lines and `key=value` lines,
//! names compared ignoring case, spaces around names and values dropped, lines starting with
//! `;` kept as notes. A change rewrites the text through a callback, keeping every other line
//! as it was and the file's line endings (`\r\n` for a new file).

use crate::text::upper;

/// What one line of the file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
    Section(&'a [u16]),
    Entry { key: &'a [u16], value: &'a [u16] },
    Note,
    Blank,
    Other(&'a [u16]),
}

const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;
const EQUALS: u16 = b'=' as u16;
const NEW_LINE: u16 = b'\n' as u16;
const RETURN: u16 = b'\r' as u16;

fn trim(mut text: &[u16]) -> &[u16] {
    while let [SPACE | TAB, rest @ ..] = text {
        text = rest;
    }
    while let [rest @ .., SPACE | TAB] = text {
        text = rest;
    }
    text
}

/// What `line` (without its line end) is.
pub fn parse_line(line: &[u16]) -> Line<'_> {
    let line = trim(line);
    match line {
        [] => Line::Blank,
        [open, rest @ ..] if *open == u16::from(b'[') => {
            let end = rest
                .iter()
                .position(|unit| *unit == u16::from(b']'))
                .unwrap_or(rest.len());
            Line::Section(trim(&rest[..end]))
        }
        [semicolon, ..] if *semicolon == u16::from(b';') => Line::Note,
        _ => match line.iter().position(|unit| *unit == EQUALS) {
            Some(equals) => Line::Entry {
                key: trim(&line[..equals]),
                value: trim(&line[equals + 1..]),
            },
            None => Line::Other(line),
        },
    }
}

/// Whether two names are the same, ignoring case.
pub fn same_name(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| upper(*x) == upper(*y))
}

fn lines(text: &[u16]) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let mut at = 0;
    core::iter::from_fn(move || {
        if at >= text.len() {
            return None;
        }
        let start = at;
        let end = text[start..]
            .iter()
            .position(|unit| *unit == NEW_LINE)
            .map_or(text.len(), |offset| start + offset + 1);
        let mut content_end = end;
        if content_end > start && text[content_end - 1] == NEW_LINE {
            content_end -= 1;
        }
        if content_end > start && text[content_end - 1] == RETURN {
            content_end -= 1;
        }
        at = end;
        Some((start, content_end, end))
    })
}

/// Calls `visit` with the name of every section, in order.
pub fn section_names(text: &[u16], visit: &mut dyn FnMut(&[u16])) {
    for (start, end, _) in lines(text) {
        if let Line::Section(name) = parse_line(&text[start..end]) {
            visit(name);
        }
    }
}

/// Calls `visit` with every entry and other line of the first section named `section`, in
/// order; whether the section is there.
pub fn section_lines<'a>(
    text: &'a [u16],
    section: &[u16],
    visit: &mut dyn FnMut(Line<'a>),
) -> bool {
    let mut found = false;
    for (start, end, _) in lines(text) {
        match parse_line(&text[start..end]) {
            Line::Section(_) if found => return true,
            Line::Section(name) => found = same_name(name, section),
            line @ (Line::Entry { .. } | Line::Other(_)) if found => visit(line),
            _ => {}
        }
    }
    found
}

/// The value of `key` in section `section`, without the quotation marks that enclose it.
pub fn value<'a>(text: &'a [u16], section: &[u16], key: &[u16]) -> Option<&'a [u16]> {
    let mut found = None;
    section_lines(text, section, &mut |line| {
        if let Line::Entry { key: name, value } = line
            && found.is_none()
            && same_name(name, key)
        {
            found = Some(value);
        }
    });
    found.map(unquoted)
}

/// `value` without the double or single quotation marks around it, if both ends have one.
pub fn unquoted(value: &[u16]) -> &[u16] {
    match value {
        [first, inner @ .., last]
            if first == last && (*first == u16::from(b'"') || *first == u16::from(b'\'')) =>
        {
            inner
        }
        _ => value,
    }
}

/// A change to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change<'a> {
    /// Sets `key` to `value`, adding the key (and the section) when missing.
    Set { key: &'a [u16], value: &'a [u16] },
    /// Removes `key`.
    RemoveKey(&'a [u16]),
    /// Removes the section with all its lines.
    RemoveSection,
    /// Replaces the section's lines with `entries`, `key=value` strings each ended by a NUL,
    /// the list ended by another NUL, adding the section when missing.
    ReplaceSection(&'a [u16]),
}

fn entries(list: &[u16]) -> impl Iterator<Item = &[u16]> {
    list.split(|unit| *unit == 0)
        .take_while(|entry| !entry.is_empty())
}

fn write_entry(out: &mut dyn FnMut(&[u16]), key: &[u16], value: &[u16], line_end: &[u16]) {
    out(key);
    out(&[EQUALS]);
    out(value);
    out(line_end);
}

fn write_added(change: Change<'_>, out: &mut dyn FnMut(&[u16]), line_end: &[u16]) {
    match change {
        Change::Set { key, value } => write_entry(out, key, value, line_end),
        Change::ReplaceSection(list) => {
            for entry in entries(list) {
                out(entry);
                out(line_end);
            }
        }
        Change::RemoveKey(_) | Change::RemoveSection => {}
    }
}

/// Writes `text` changed by `change` to the first section named `section` through `out`.
pub fn change(text: &[u16], section: &[u16], change: Change<'_>, out: &mut dyn FnMut(&[u16])) {
    let crlf = text.is_empty() || text.windows(2).any(|pair| pair == [RETURN, NEW_LINE]);
    let line_end: &[u16] = if crlf {
        &[RETURN, NEW_LINE]
    } else {
        &[NEW_LINE]
    };
    let setting = matches!(change, Change::Set { .. });
    let dropping_lines = matches!(change, Change::RemoveSection | Change::ReplaceSection(_));
    let (mut visited, mut in_section, mut done) = (false, false, false);
    let mut last_ended = true;
    let mut held_blanks: Option<usize> = None;
    for (start, content_end, end) in lines(text) {
        let line = parse_line(&text[start..content_end]);
        let ended = end > content_end;
        if let Line::Section(name) = line {
            if in_section {
                if setting && !done {
                    if !last_ended {
                        out(line_end);
                    }
                    write_added(change, out, line_end);
                }
                if let Some(blank) = held_blanks.take() {
                    out(&text[blank..start]);
                }
                in_section = false;
                done = true;
            }
            if !visited && same_name(name, section) {
                visited = true;
                in_section = true;
                if matches!(change, Change::RemoveSection) {
                    continue;
                }
                out(&text[start..end]);
                last_ended = ended;
                if let Change::ReplaceSection(_) = change {
                    if !ended {
                        out(line_end);
                    }
                    write_added(change, out, line_end);
                    last_ended = true;
                }
                continue;
            }
            out(&text[start..end]);
            last_ended = ended;
            continue;
        }
        if !in_section {
            out(&text[start..end]);
            last_ended = ended;
            continue;
        }
        if dropping_lines {
            continue;
        }
        if matches!(line, Line::Blank) {
            held_blanks.get_or_insert(start);
            continue;
        }
        let matched = match line {
            Line::Entry { key: name, .. } => match change {
                Change::Set { key, .. } | Change::RemoveKey(key) => !done && same_name(name, key),
                _ => false,
            },
            _ => false,
        };
        if matched && matches!(change, Change::RemoveKey(_)) {
            continue;
        }
        if let Some(blank) = held_blanks.take() {
            out(&text[blank..start]);
        }
        match (matched, change, line) {
            (true, Change::Set { value, .. }, Line::Entry { key: name, .. }) => {
                let ending: &[u16] = if ended { line_end } else { &[] };
                write_entry(out, name, value, ending);
                done = true;
            }
            _ => out(&text[start..end]),
        }
        last_ended = ended;
    }
    if in_section {
        if setting && !done {
            if !last_ended {
                out(line_end);
            }
            write_added(change, out, line_end);
        }
        if let Some(blank) = held_blanks {
            out(&text[blank..]);
        }
    }
    if !visited && matches!(change, Change::Set { .. } | Change::ReplaceSection(_)) {
        if !last_ended {
            out(line_end);
        }
        out(&[u16::from(b'[')]);
        out(section);
        out(&[u16::from(b']')]);
        out(line_end);
        write_added(change, out, line_end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn changed(text: &str, section: &str, change_to: Change<'_>) -> String {
        let mut out = Vec::new();
        change(&wide(text), &wide(section), change_to, &mut |piece| {
            out.extend_from_slice(piece)
        });
        String::from_utf16(&out).unwrap()
    }

    const FILE: &str =
        "; settings\r\n[General]\r\nName = Egg\r\nSize=\"12\"\r\n\r\n[Other]\r\nx=1\r\n";

    #[test]
    fn lines_are_sections_entries_notes_and_others() {
        assert_eq!(
            parse_line(&wide("  [ Main ] ")),
            Line::Section(&wide("Main"))
        );
        assert_eq!(
            parse_line(&wide(" key = some value ")),
            Line::Entry {
                key: &wide("key"),
                value: &wide("some value")
            }
        );
        assert_eq!(parse_line(&wide("; note")), Line::Note);
        assert_eq!(parse_line(&wide(" \t")), Line::Blank);
        assert_eq!(parse_line(&wide("loose")), Line::Other(&wide("loose")));
    }

    #[test]
    fn values_and_names_are_found_ignoring_case_and_quotes() {
        let text = wide(FILE);
        assert_eq!(
            value(&text, &wide("general"), &wide("NAME")),
            Some(&wide("Egg")[..])
        );
        assert_eq!(
            value(&text, &wide("General"), &wide("size")),
            Some(&wide("12")[..])
        );
        assert_eq!(value(&text, &wide("Other"), &wide("Name")), None);
        assert_eq!(value(&text, &wide("Missing"), &wide("x")), None);
        let mut names = Vec::new();
        section_names(&text, &mut |name| {
            names.push(String::from_utf16(name).unwrap())
        });
        assert_eq!(names, ["General", "Other"]);
        let mut count = 0;
        assert!(section_lines(&text, &wide("GENERAL"), &mut |_| count += 1));
        assert_eq!(count, 2);
        assert!(!section_lines(&text, &wide("None"), &mut |_| {}));
    }

    #[test]
    fn setting_replaces_a_key_or_adds_it_before_the_blank_lines() {
        let set = |key: &str, value: &str, section: &str| {
            changed(
                FILE,
                section,
                Change::Set {
                    key: &wide(key),
                    value: &wide(value),
                },
            )
        };
        assert_eq!(
            set("name", "Hen", "General"),
            "; settings\r\n[General]\r\nName=Hen\r\nSize=\"12\"\r\n\r\n[Other]\r\nx=1\r\n"
        );
        assert_eq!(
            set("Color", "red", "General"),
            "; settings\r\n[General]\r\nName = Egg\r\nSize=\"12\"\r\nColor=red\r\n\r\n[Other]\r\nx=1\r\n"
        );
        assert_eq!(set("y", "2", "Other"), format!("{FILE}y=2\r\n"));
        assert_eq!(set("z", "3", "New"), format!("{FILE}[New]\r\nz=3\r\n"));
        assert_eq!(
            changed(
                "[A]\na=1",
                "A",
                Change::Set {
                    key: &wide("b"),
                    value: &wide("2")
                }
            ),
            "[A]\na=1\nb=2\n"
        );
        assert_eq!(
            changed(
                "",
                "A",
                Change::Set {
                    key: &wide("k"),
                    value: &wide("v")
                }
            ),
            "[A]\r\nk=v\r\n"
        );
    }

    #[test]
    fn keys_and_sections_are_removed_and_sections_replaced() {
        assert_eq!(
            changed(FILE, "general", Change::RemoveKey(&wide("NAME"))),
            "; settings\r\n[General]\r\nSize=\"12\"\r\n\r\n[Other]\r\nx=1\r\n"
        );
        assert_eq!(
            changed(FILE, "General", Change::RemoveSection),
            "; settings\r\n[Other]\r\nx=1\r\n"
        );
        assert_eq!(
            changed(FILE, "Other", Change::ReplaceSection(&wide("a=1\0b=2\0\0"))),
            "; settings\r\n[General]\r\nName = Egg\r\nSize=\"12\"\r\n\r\n[Other]\r\na=1\r\nb=2\r\n"
        );
        assert_eq!(
            changed("[A]\r\nx=1", "B", Change::ReplaceSection(&wide("c=3\0\0"))),
            "[A]\r\nx=1\r\n[B]\r\nc=3\r\n"
        );
        assert_eq!(changed(FILE, "Missing", Change::RemoveSection), FILE);
    }
}
