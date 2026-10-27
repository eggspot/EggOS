//! `cargo xtask lint`: fails on comments the repository's rules disallow (CLAUDE.md, "No other
//! comments in code"), in every tracked `.rs`, `.toml` and `.yml` file.
//!
//! Files that still have such comments are listed with their counts in
//! `xtask/lint-legacy-files.txt`, so the lint fails only for a violation in a file that is not
//! listed or beyond a listed file's count, and for a list that no longer matches (a count that
//! went down, a file that is clean or gone), which makes the list only shrink.

use std::collections::BTreeMap;
use std::path::Path;

const LEGACY_FILES: &str = "xtask/lint-legacy-files.txt";

#[derive(Debug, PartialEq, Eq)]
pub struct Violation {
    pub line: usize,
    pub text: String,
}

pub fn run(shrink: bool) -> Result<(), String> {
    let root = crate::workspace_root();
    let legacy_path = root.join(LEGACY_FILES);
    let legacy = parse_legacy(&read(&legacy_path)?)?;
    let mut found: BTreeMap<String, Vec<Violation>> = BTreeMap::new();
    let mut scanned = 0;
    for file in tracked_files(&root)? {
        let Some(text) = read_if_present(&root.join(&file))? else {
            continue;
        };
        scanned += 1;
        let violations = scan(&file, &text);
        if !violations.is_empty() {
            found.insert(file, violations);
        }
    }
    let problems = problems(&legacy, &found);
    if shrink {
        return write_shrunk(&legacy_path, &legacy, &found, &problems);
    }
    if !problems.is_empty() {
        return Err(format!(
            "{}\n(CLAUDE.md allows no comments in code but `// SAFETY:`, lint directives and \
             `///` on public items; run `cargo xtask lint --shrink` after removing comments \
             from a file listed in {LEGACY_FILES})",
            problems.join("\n")
        ));
    }
    let remaining: usize = legacy.values().sum();
    eprintln!(
        "xtask: lint: {scanned} files scanned, {remaining} comments left in {} legacy files",
        legacy.len()
    );
    Ok(())
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_if_present(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn tracked_files(root: &Path) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .map_err(|e| format!("could not run git ls-files: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| is_scanned(path))
        .map(str::to_owned)
        .collect())
}

fn is_scanned(path: &str) -> bool {
    matches!(
        path.rsplit_once('.').map(|(_, extension)| extension),
        Some("rs" | "toml" | "yml")
    )
}

fn parse_legacy(text: &str) -> Result<BTreeMap<String, usize>, String> {
    let mut legacy = BTreeMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let [path, count] = line.split_whitespace().collect::<Vec<_>>()[..] else {
            return Err(format!("{LEGACY_FILES}: malformed line `{line}`"));
        };
        let count: usize = count
            .parse()
            .ok()
            .filter(|count| *count > 0)
            .ok_or_else(|| format!("{LEGACY_FILES}: malformed count in `{line}`"))?;
        if legacy.insert(path.to_owned(), count).is_some() {
            return Err(format!("{LEGACY_FILES}: {path} is listed twice"));
        }
    }
    Ok(legacy)
}

fn problems(
    legacy: &BTreeMap<String, usize>,
    found: &BTreeMap<String, Vec<Violation>>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let describe = |path: &str, violations: &[Violation]| {
        violations
            .iter()
            .map(|v| format!("  {path}:{}: {}", v.line, v.text))
            .collect::<Vec<_>>()
            .join("\n")
    };
    for (path, violations) in found {
        match legacy.get(path) {
            None => problems.push(format!(
                "{path}: {} disallowed comment(s)\n{}",
                violations.len(),
                describe(path, violations)
            )),
            Some(&listed) if violations.len() > listed => problems.push(format!(
                "{path}: {} disallowed comment(s), more than the {listed} listed in \
                 {LEGACY_FILES}\n{}",
                violations.len(),
                describe(path, violations)
            )),
            Some(&listed) if violations.len() < listed => problems.push(format!(
                "{path}: {} disallowed comment(s), fewer than the {listed} listed in \
                 {LEGACY_FILES}: shrink the list",
                violations.len()
            )),
            Some(_) => {}
        }
    }
    for path in legacy.keys().filter(|path| !found.contains_key(*path)) {
        problems.push(format!(
            "{path}: listed in {LEGACY_FILES} but has no disallowed comments (or is gone): \
             remove it from the list"
        ));
    }
    problems
}

fn write_shrunk(
    legacy_path: &Path,
    legacy: &BTreeMap<String, usize>,
    found: &BTreeMap<String, Vec<Violation>>,
    problems: &[String],
) -> Result<(), String> {
    let grown: Vec<&String> = problems
        .iter()
        .filter(|problem| !problem.contains("shrink the list") && !problem.contains("remove it"))
        .collect();
    if !grown.is_empty() {
        return Err(format!(
            "--shrink only lowers counts and removes files; fix these first:\n{}",
            grown
                .iter()
                .map(|problem| problem.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    let mut text = String::new();
    for (path, &listed) in legacy {
        let now = found.get(path).map_or(0, Vec::len).min(listed);
        if now > 0 {
            text.push_str(&format!("{path} {now}\n"));
        }
    }
    std::fs::write(legacy_path, text).map_err(|e| format!("{}: {e}", legacy_path.display()))?;
    eprintln!("xtask: wrote {}", legacy_path.display());
    Ok(())
}

pub fn scan(path: &str, text: &str) -> Vec<Violation> {
    if text.lines().take(3).any(|line| line.contains("@generated")) {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let flagged = if path.ends_with(".rs") {
        scan_rust(text, &lines)
    } else if path.ends_with(".toml") {
        scan_toml(&lines)
    } else {
        scan_yaml(&lines)
    };
    flagged
        .into_iter()
        .map(|line| Violation {
            line: line + 1,
            text: lines[line].trim().to_owned(),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RustComment {
    Plain,
    OuterDoc,
    InnerDoc,
    Block,
}

#[derive(Debug)]
struct Found {
    line: usize,
    column: usize,
    kind: RustComment,
    body: String,
}

fn is_identifier_character(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn scan_rust(text: &str, lines: &[&str]) -> Vec<usize> {
    let comments = rust_comments(text);
    let mut flagged = Vec::new();
    let mut safety_line: Option<usize> = None;
    for comment in comments {
        let own_line = lines
            .get(comment.line)
            .is_some_and(|line| line.chars().take(comment.column).all(char::is_whitespace));
        match comment.kind {
            RustComment::InnerDoc => {}
            RustComment::OuterDoc => {
                if !outer_doc_is_on_public_item(lines, comment.line) {
                    flagged.push(comment.line);
                }
            }
            RustComment::Block => flagged.push(comment.line),
            RustComment::Plain => {
                let starts_safety = comment.body.trim_start().starts_with("SAFETY:");
                let continues_safety =
                    own_line && comment.line > 0 && safety_line == Some(comment.line - 1);
                if starts_safety || continues_safety {
                    safety_line = Some(comment.line);
                } else {
                    flagged.push(comment.line);
                }
            }
        }
    }
    flagged
}

fn outer_doc_is_on_public_item(lines: &[&str], line: usize) -> bool {
    let is_doc = |index: usize| lines[index].trim_start().starts_with("///");
    let mut first = line;
    while first > 0 && is_doc(first - 1) {
        first -= 1;
    }
    let mut index = line;
    while index + 1 < lines.len() && is_doc(index + 1) {
        index += 1;
    }
    let mut item = index + 1;
    let mut attribute_depth = 0i32;
    while item < lines.len() {
        let trimmed = lines[item].trim();
        let starts_attribute = trimmed.starts_with("#[");
        if starts_attribute || attribute_depth > 0 {
            attribute_depth += trimmed.matches('[').count() as i32;
            attribute_depth -= trimmed.matches(']').count() as i32;
            item += 1;
        } else if trimmed.is_empty() || trimmed.starts_with("//") {
            item += 1;
        } else {
            break;
        }
    }
    let Some(item_line) = lines.get(item) else {
        return false;
    };
    if is_public_item(item_line.trim_start()) {
        return true;
    }
    let indent = indentation(lines[line]);
    if indent == 0 {
        return false;
    }
    let enclosing = (0..first).rev().find(|&candidate| {
        let trimmed = lines[candidate].trim();
        !trimmed.is_empty()
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("#[")
            && indentation(lines[candidate]) < indent
    });
    enclosing.is_some_and(|candidate| is_public_container(lines[candidate].trim_start()))
}

fn indentation(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_public_item(trimmed: &str) -> bool {
    trimmed.starts_with("pub ") || trimmed.starts_with("pub(")
}

fn is_public_container(trimmed: &str) -> bool {
    if !is_public_item(trimmed) {
        return false;
    }
    let after_visibility = match trimmed.strip_prefix("pub(") {
        Some(rest) => rest.split_once(')').map_or("", |(_, rest)| rest),
        None => &trimmed["pub".len()..],
    };
    let item = after_visibility.trim_start();
    let item = item.strip_prefix("unsafe ").unwrap_or(item);
    ["struct ", "enum ", "trait ", "union "]
        .iter()
        .any(|keyword| item.starts_with(keyword))
}

fn rust_comments(text: &str) -> Vec<Found> {
    let chars: Vec<char> = text.chars().collect();
    let mut comments = Vec::new();
    let (mut i, mut line, mut line_start) = (0, 0, 0);
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
                line_start = i;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                let end = chars[i..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(chars.len(), |offset| i + offset);
                let body: String = chars[i + 2..end].iter().collect();
                let kind = if body.starts_with('!') {
                    RustComment::InnerDoc
                } else if body.starts_with('/') && !body.starts_with("//") {
                    RustComment::OuterDoc
                } else {
                    RustComment::Plain
                };
                comments.push(Found {
                    line,
                    column: i - line_start,
                    kind,
                    body,
                });
                i = end;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                comments.push(Found {
                    line,
                    column: i - line_start,
                    kind: RustComment::Block,
                    body: String::new(),
                });
                let mut depth = 0;
                while i < chars.len() {
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        if chars[i] == '\n' {
                            line += 1;
                            line_start = i + 1;
                        }
                        i += 1;
                    }
                }
            }
            '"' => {
                i = skip_string(&chars, i + 1, &mut line, &mut line_start);
            }
            '\'' => {
                i = skip_character_or_lifetime(&chars, i);
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && is_identifier_character(chars[i]) {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if matches!(word.as_str(), "r" | "br" | "cr") {
                    let hashes = chars[i..].iter().take_while(|&&c| c == '#').count();
                    if chars.get(i + hashes) == Some(&'"') {
                        i = skip_raw_string(
                            &chars,
                            i + hashes + 1,
                            hashes,
                            &mut line,
                            &mut line_start,
                        );
                    }
                }
            }
            _ => i += 1,
        }
    }
    comments
}

fn skip_string(chars: &[char], mut i: usize, line: &mut usize, line_start: &mut usize) -> usize {
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                if chars.get(i + 1) == Some(&'\n') {
                    *line += 1;
                    *line_start = i + 2;
                }
                i += 2;
            }
            '"' => return i + 1,
            '\n' => {
                *line += 1;
                i += 1;
                *line_start = i;
            }
            _ => i += 1,
        }
    }
    i
}

fn skip_raw_string(
    chars: &[char],
    mut i: usize,
    hashes: usize,
    line: &mut usize,
    line_start: &mut usize,
) -> usize {
    while i < chars.len() {
        if chars[i] == '"' && chars[i + 1..].iter().take_while(|&&c| c == '#').count() >= hashes {
            return i + 1 + hashes;
        }
        if chars[i] == '\n' {
            *line += 1;
            *line_start = i + 1;
        }
        i += 1;
    }
    i
}

fn skip_character_or_lifetime(chars: &[char], i: usize) -> usize {
    match chars.get(i + 1) {
        Some('\\') => {
            let mut j = i + 3;
            while j < chars.len() && chars[j] != '\'' && chars[j] != '\n' {
                j += 1;
            }
            j + 1
        }
        Some(_) if chars.get(i + 2) == Some(&'\'') => i + 3,
        _ => i + 1,
    }
}

fn scan_toml(lines: &[&str]) -> Vec<usize> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Code,
        Basic,
        Literal,
        MultilineBasic,
        MultilineLiteral,
    }
    let mut flagged = Vec::new();
    let mut state = State::Code;
    for (number, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let triple = |quote: char| chars[i..].starts_with(&[quote, quote, quote]);
            match state {
                State::Code => match chars[i] {
                    '#' => {
                        if !toml_directive(&chars[i..].iter().collect::<String>()) {
                            flagged.push(number);
                        }
                        break;
                    }
                    '"' if triple('"') => {
                        state = State::MultilineBasic;
                        i += 3;
                    }
                    '"' => {
                        state = State::Basic;
                        i += 1;
                    }
                    '\'' if triple('\'') => {
                        state = State::MultilineLiteral;
                        i += 3;
                    }
                    '\'' => {
                        state = State::Literal;
                        i += 1;
                    }
                    _ => i += 1,
                },
                State::Basic => match chars[i] {
                    '\\' => i += 2,
                    '"' => {
                        state = State::Code;
                        i += 1;
                    }
                    _ => i += 1,
                },
                State::Literal => {
                    if chars[i] == '\'' {
                        state = State::Code;
                    }
                    i += 1;
                }
                State::MultilineBasic => match chars[i] {
                    '\\' => i += 2,
                    '"' if triple('"') => {
                        state = State::Code;
                        i += 3;
                    }
                    _ => i += 1,
                },
                State::MultilineLiteral => {
                    if triple('\'') {
                        state = State::Code;
                        i += 3;
                    } else {
                        i += 1;
                    }
                }
            }
        }
        if matches!(state, State::Basic | State::Literal) {
            state = State::Code;
        }
    }
    flagged
}

fn toml_directive(comment: &str) -> bool {
    comment.starts_with("#:schema") || comment.starts_with("# taplo:")
}

fn scan_yaml(lines: &[&str]) -> Vec<usize> {
    let mut flagged = Vec::new();
    let mut block_parent: Option<usize> = None;
    for (number, line) in lines.iter().enumerate() {
        if let Some(parent) = block_parent {
            if line.trim().is_empty() || indentation(line) > parent {
                continue;
            }
            block_parent = None;
        }
        let chars: Vec<char> = line.chars().collect();
        let comment_at = yaml_comment_start(&chars);
        let code: String = chars[..comment_at.unwrap_or(chars.len())].iter().collect();
        if let Some(at) = comment_at {
            let comment: String = chars[at..].iter().collect();
            if !yaml_comment_allowed(&code, &comment) {
                flagged.push(number);
            }
        }
        if ends_with_block_scalar_header(&code) {
            block_parent = Some(key_column(&code));
        }
    }
    flagged
}

fn yaml_comment_start(chars: &[char]) -> Option<usize> {
    let mut i = 0;
    while i < chars.len() {
        let value_start = chars[..i]
            .iter()
            .rev()
            .find(|c| !c.is_whitespace())
            .is_none_or(|&c| {
                matches!(c, '[' | '{' | ',' | ':') || (c == '-' && sole_marker(chars, i))
            });
        match chars[i] {
            '#' if i == 0 || chars[i - 1].is_whitespace() => return Some(i),
            '"' if value_start => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
            }
            '\'' if value_start => {
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        if chars.get(i + 1) == Some(&'\'') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn sole_marker(chars: &[char], at: usize) -> bool {
    let before: String = chars[..at].iter().collect();
    let trimmed = before.trim_end();
    trimmed.trim_start_matches(['-', ' ']).is_empty() && trimmed.ends_with('-')
}

fn yaml_comment_allowed(code: &str, comment: &str) -> bool {
    if comment.starts_with("# yaml-language-server:") {
        return true;
    }
    code.contains("uses:") && is_version_comment(comment)
}

fn is_version_comment(comment: &str) -> bool {
    let version = comment.trim_start_matches('#').trim();
    let version = version.strip_prefix('v').unwrap_or(version);
    version.chars().next().is_some_and(|c| c.is_ascii_digit())
        && !version.contains(char::is_whitespace)
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
}

fn ends_with_block_scalar_header(code: &str) -> bool {
    let trimmed = code.trim_end();
    let Some(token) = trimmed.split_whitespace().last() else {
        return false;
    };
    let mut characters = token.chars();
    let indicator = matches!(characters.next(), Some('|' | '>'));
    indicator
        && characters.all(|c| matches!(c, '+' | '-') || c.is_ascii_digit())
        && (trimmed.trim_start().starts_with("- ") || trimmed.contains(": "))
}

fn key_column(code: &str) -> usize {
    let mut column = 0;
    let mut rest = code;
    loop {
        let trimmed = rest.trim_start();
        column += rest.len() - trimmed.len();
        match trimmed.strip_prefix("- ") {
            Some(after) => {
                column += 2;
                rest = after;
            }
            None => return column,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(path: &str, text: &str) -> Vec<usize> {
        scan(path, text).iter().map(|v| v.line).collect()
    }

    #[test]
    fn a_plain_line_comment_is_flagged_on_its_own_line_and_after_code() {
        let text = "fn a() {\n    // what it does\n    let x = 1; // why\n}\n";
        assert_eq!(lines_of("a.rs", text), vec![2, 3]);
    }

    #[test]
    fn slashes_inside_strings_raw_strings_and_characters_are_not_comments() {
        let text = concat!(
            "let a = \"http://x\";\n",
            "let b = r#\"// not\n// a comment \"quoted\"\"#;\n",
            "let c = b\"//\";\n",
            "let d = '/';\n",
            "let e = '\\'';\n",
            "let f = \"line one \\\n    // still a string\";\n",
            "let g = br\"//\";\n",
        );
        assert_eq!(lines_of("a.rs", text), Vec::<usize>::new());
    }

    #[test]
    fn lifetimes_and_raw_identifiers_do_not_start_a_character_literal_or_string() {
        let text = "fn f<'a>(x: &'a str) -> &'a str { let r#type = x; x } // after\n";
        assert_eq!(lines_of("a.rs", text), vec![1]);
    }

    #[test]
    fn four_slashes_are_a_plain_comment() {
        assert_eq!(lines_of("a.rs", "//// banner\n"), vec![1]);
    }

    #[test]
    fn block_comments_are_flagged_once_nested_or_not_and_lines_stay_in_step() {
        let text = "/* one\n /* nested */\n still */\nlet x = 1; // after\n";
        assert_eq!(lines_of("a.rs", text), vec![1, 4]);
    }

    #[test]
    fn a_safety_comment_and_its_continuation_lines_are_allowed() {
        let text = concat!(
            "// SAFETY: the pointer is valid\n",
            "// and aligned.\n",
            "// So the read is sound.\n",
            "unsafe { read() }\n",
            "// not a continuation\n",
            "// SAFETY: another\n",
            "unsafe { read() }\n",
        );
        assert_eq!(lines_of("a.rs", text), vec![5]);
    }

    #[test]
    fn a_comment_after_a_blank_line_does_not_continue_a_safety_comment() {
        let text = "// SAFETY: yes\n\n// stray\nunsafe { read() }\n";
        assert_eq!(lines_of("a.rs", text), vec![3]);
    }

    #[test]
    fn module_docs_are_allowed_anywhere() {
        assert_eq!(
            lines_of("a.rs", "//! crate docs\n//! more\nuse x;\n"),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn a_doc_comment_is_allowed_on_a_public_item_through_attributes() {
        let text = concat!(
            "/// Does it.\n",
            "#[derive(Debug)]\n",
            "#[cfg(any(\n",
            "    feature = \"a\",\n",
            "))]\n",
            "pub struct S;\n",
            "/// Also.\n",
            "pub(crate) fn f() {}\n",
        );
        assert_eq!(lines_of("a.rs", text), Vec::<usize>::new());
    }

    #[test]
    fn a_doc_comment_on_a_private_item_is_flagged() {
        let text = "/// Private.\nfn f() {}\n/// Also private.\nstruct S;\n";
        assert_eq!(lines_of("a.rs", text), vec![1, 3]);
    }

    #[test]
    fn fields_and_variants_of_a_public_type_may_be_documented() {
        let text = concat!(
            "pub struct S {\n",
            "    /// The field.\n",
            "    field: u8,\n",
            "}\n",
            "pub enum E {\n",
            "    /// The variant.\n",
            "    A,\n",
            "}\n",
            "pub trait T {\n",
            "    /// The method.\n",
            "    fn f(&self);\n",
            "}\n",
            "struct Private {\n",
            "    /// Not public.\n",
            "    field: u8,\n",
            "}\n",
            "impl T for S {\n",
            "    /// In an impl.\n",
            "    fn f(&self) {}\n",
            "}\n",
        );
        assert_eq!(lines_of("a.rs", text), vec![14, 18]);
    }

    #[test]
    fn a_doc_comment_run_is_judged_as_one() {
        let text = "/// First.\n/// Second.\npub fn f() {}\n/// First.\n/// Second.\nfn g() {}\n";
        assert_eq!(lines_of("a.rs", text), vec![4, 5]);
    }

    #[test]
    fn generated_files_are_skipped() {
        assert!(scan("a.rs", "// @generated\n// x\n").is_empty());
    }

    #[test]
    fn toml_comments_are_flagged_outside_strings_only() {
        let text = concat!(
            "# a comment\n",
            "[package]\n",
            "name = \"a#b\" # trailing\n",
            "path = 'c#d'\n",
            "text = \"\"\"\n",
            "# inside\n",
            "\"\"\"\n",
            "raw = '''\n",
            "# inside\n",
            "'''\n",
            "escaped = \"quote \\\" # still a string\"\n",
            "#:schema https://example.com/schema.json\n",
        );
        assert_eq!(lines_of("Cargo.toml", text), vec![1, 3]);
    }

    #[test]
    fn yaml_comments_are_flagged_outside_values_only() {
        let text = concat!(
            "# yaml-language-server: $schema=x\n",
            "# a comment\n",
            "name: CI # trailing\n",
            "url: http://x/#anchor\n",
            "quoted: \"a # b\"\n",
            "single: 'it''s # fine'\n",
            "list: [a, \"b # c\"]\n",
            "plain: it's # comment\n",
            "run: |\n",
            "  echo hi # shell comment is data\n",
            "  # also data\n",
            "next: 1 # flagged\n",
        );
        assert_eq!(lines_of("a.yml", text), vec![2, 3, 8, 12]);
    }

    #[test]
    fn a_version_comment_after_uses_is_allowed() {
        let text = concat!(
            "      - uses: actions/checkout@abc # v4.2.2\n",
            "      - uses: actions/checkout@abc # v4\n",
            "      - uses: actions/checkout@abc # because\n",
            "      - name: x # v4.2.2\n",
        );
        assert_eq!(lines_of("a.yml", text), vec![3, 4]);
    }

    #[test]
    fn block_scalars_in_list_items_end_at_the_next_key() {
        let text = concat!(
            "steps:\n",
            "  - name: a\n",
            "    run: |\n",
            "      # data\n",
            "\n",
            "      echo\n",
            "    env: x # flagged\n",
            "  - run: >-\n",
            "      # data too\n",
            "  - name: b # flagged\n",
        );
        assert_eq!(lines_of("a.yml", text), vec![7, 10]);
    }

    #[test]
    fn the_legacy_list_parses_and_rejects_malformed_lines() {
        let list = parse_legacy("a.rs 3\n\nb/c.toml 1\n").unwrap();
        assert_eq!(list.get("a.rs"), Some(&3));
        assert_eq!(list.len(), 2);
        assert!(parse_legacy("a.rs").is_err());
        assert!(parse_legacy("a.rs x").is_err());
        assert!(parse_legacy("a.rs 0").is_err());
        assert!(parse_legacy("a.rs 1\na.rs 2").is_err());
    }

    fn found(entries: &[(&str, usize)]) -> BTreeMap<String, Vec<Violation>> {
        entries
            .iter()
            .map(|&(path, count)| {
                let violations = (1..=count)
                    .map(|line| Violation {
                        line,
                        text: "// x".into(),
                    })
                    .collect();
                (path.to_owned(), violations)
            })
            .collect()
    }

    fn legacy(entries: &[(&str, usize)]) -> BTreeMap<String, usize> {
        entries
            .iter()
            .map(|&(path, count)| (path.to_owned(), count))
            .collect()
    }

    #[test]
    fn a_new_violation_in_an_unlisted_file_fails() {
        let problems = problems(&legacy(&[]), &found(&[("new.rs", 1)]));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("new.rs: 1 disallowed comment(s)"));
        assert!(problems[0].contains("new.rs:1: // x"));
    }

    #[test]
    fn a_listed_file_may_keep_its_count_but_not_grow() {
        let listed = legacy(&[("a.rs", 2)]);
        assert!(problems(&listed, &found(&[("a.rs", 2)])).is_empty());
        let grown = problems(&listed, &found(&[("a.rs", 3)]));
        assert_eq!(grown.len(), 1);
        assert!(grown[0].contains("more than the 2 listed"));
    }

    #[test]
    fn a_listed_file_that_shrank_or_is_clean_must_shrink_the_list() {
        let listed = legacy(&[("a.rs", 2), ("b.rs", 1)]);
        let problems = problems(&listed, &found(&[("a.rs", 1)]));
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("fewer than the 2 listed"));
        assert!(problems[1].starts_with("b.rs: listed in"));
        assert!(problems[1].contains("remove it from the list"));
    }

    #[test]
    fn shrinking_lowers_counts_and_drops_clean_files_but_refuses_growth() {
        let dir = std::env::temp_dir().join(format!("eggos-lint-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.txt");
        let listed = legacy(&[("a.rs", 2), ("b.rs", 1), ("c.rs", 4)]);
        let now = found(&[("a.rs", 1), ("c.rs", 4)]);
        write_shrunk(&path, &listed, &now, &problems(&listed, &now)).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "a.rs 1\nc.rs 4\n");
        let grown = found(&[("a.rs", 3), ("c.rs", 4)]);
        assert!(write_shrunk(&path, &listed, &grown, &problems(&listed, &grown)).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn only_rust_toml_and_yaml_files_are_scanned() {
        for path in ["a.rs", "x/Cargo.toml", ".github/workflows/ci.yml"] {
            assert!(is_scanned(path), "{path}");
        }
        for path in [
            "a.md",
            "Cargo.lock",
            "Dockerfile",
            "a.rs.txt",
            "LICENSE-MIT",
        ] {
            assert!(!is_scanned(path), "{path}");
        }
    }
}
