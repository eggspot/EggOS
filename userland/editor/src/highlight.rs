//! Syntax colouring for the Text Editor (D574): which kind of token each byte of a line is, by
//! the language the file's name says, one line at a time with the state a line leaves for the
//! next (inside a block comment or a multi-line string). The colours of the kinds are the
//! design's (`docs/ux/text-editor-syntax-v0.1.md`, section 1); this module only classifies,
//! and leaves plain what the characters already tell (section 2).

/// What a run of a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Keyword,
    Type,
    String,
    Number,
    Comment,
    Function,
    Macro,
    Attribute,
    Escape,
    Heading,
    Link,
    Key,
    Tag,
}

/// The languages the editor colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    C,
    Python,
    JavaScript,
    Shell,
    Json,
    Toml,
    Yaml,
    Markdown,
    Markup,
    Css,
    Ini,
}

/// What a line leaves for the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Normal,
    BlockComment,
    LongString(u8),
    MarkupComment,
    CodeBlock,
}

/// The most runs one line is cut into; the rest of a longer line stays one run.
pub const MAX_RUNS: usize = 128;

/// A line's runs: kind and end (exclusive) of each, in order.
#[derive(Debug, Clone, Copy)]
pub struct Runs {
    runs: [(Kind, u32); MAX_RUNS],
    count: usize,
}

impl Runs {
    fn new() -> Self {
        Self {
            runs: [(Kind::Plain, 0); MAX_RUNS],
            count: 0,
        }
    }

    fn push(&mut self, kind: Kind, end: usize) {
        if let Some(last) = self.count.checked_sub(1)
            && self.runs[last].0 == kind
        {
            self.runs[last].1 = end as u32;
            return;
        }
        match self.count < MAX_RUNS {
            true => {
                self.runs[self.count] = (kind, end as u32);
                self.count += 1;
            }
            false => self.runs[MAX_RUNS - 1].1 = end as u32,
        }
    }

    /// The kind of the byte at `offset`.
    pub fn kind_at(&self, offset: usize) -> Kind {
        self.runs[..self.count]
            .iter()
            .find(|&&(_, end)| offset < end as usize)
            .map_or(Kind::Plain, |&(kind, _)| kind)
    }
}

impl Language {
    /// The language of a file called `name` whose first line is `first_line`: by the name's
    /// extension, else by a `#!` line naming a shell or Python; `None` for plain text.
    pub fn detect(name: &[u8], first_line: &[u8]) -> Option<Self> {
        Self::of(name).or_else(|| {
            let interpreter = first_line.strip_prefix(b"#!")?;
            let mut words = interpreter.split(|byte| byte.is_ascii_whitespace() || *byte == b'/');
            words.find_map(|word| match word {
                b"sh" | b"bash" | b"zsh" | b"dash" | b"ksh" | b"eggsh" => Some(Self::Shell),
                _ if word.starts_with(b"python") => Some(Self::Python),
                _ => None,
            })
        })
    }

    /// The language of a file called `name`, by its extension alone.
    pub fn of(name: &[u8]) -> Option<Self> {
        if name == b"Cargo.lock" {
            return Some(Self::Toml);
        }
        let extension = &name[name.iter().rposition(|&byte| byte == b'.')? + 1..];
        let lower = |text: &[u8]| text.eq_ignore_ascii_case(extension);
        Some(match () {
            _ if lower(b"rs") => Self::Rust,
            _ if [&b"c"[..], b"h", b"cc", b"cpp", b"hpp", b"cxx"].iter().any(|e| lower(*e)) => Self::C,
            _ if lower(b"py") => Self::Python,
            _ if [&b"js"[..], b"ts", b"jsx", b"tsx", b"mjs", b"cjs"].iter().any(|e| lower(*e)) => Self::JavaScript,
            _ if [&b"sh"[..], b"bash", b"zsh"].iter().any(|e| lower(*e)) => Self::Shell,
            _ if lower(b"json") => Self::Json,
            _ if lower(b"toml") => Self::Toml,
            _ if [&b"yml"[..], b"yaml"].iter().any(|e| lower(*e)) => Self::Yaml,
            _ if [&b"md"[..], b"markdown"].iter().any(|e| lower(*e)) => Self::Markdown,
            _ if [&b"html"[..], b"htm", b"xml", b"svg"].iter().any(|e| lower(*e)) => Self::Markup,
            _ if lower(b"css") => Self::Css,
            _ if [&b"ini"[..], b"conf", b"cfg"].iter().any(|e| lower(*e)) => Self::Ini,
            _ => return None,
        })
    }

    fn keywords(self) -> &'static [&'static [u8]] {
        match self {
            Self::Rust => &[
                b"as", b"async", b"await", b"break", b"const", b"continue", b"crate", b"dyn",
                b"else", b"enum", b"extern", b"false", b"fn", b"for", b"if", b"impl", b"in",
                b"let", b"loop", b"match", b"mod", b"move", b"mut", b"pub", b"ref", b"return",
                b"self", b"Self", b"static", b"struct", b"super", b"trait", b"true", b"type",
                b"unsafe", b"use", b"where", b"while",
            ],
            Self::C => &[
                b"auto", b"break", b"case", b"char", b"class", b"const", b"continue", b"default",
                b"do", b"double", b"else", b"enum", b"extern", b"float", b"for", b"goto", b"if",
                b"inline", b"int", b"long", b"namespace", b"new", b"nullptr", b"private",
                b"public", b"register", b"return", b"short", b"signed", b"sizeof", b"static",
                b"struct", b"switch", b"template", b"this", b"typedef", b"union", b"unsigned",
                b"void", b"volatile", b"while", b"true", b"false", b"bool",
            ],
            Self::Python => &[
                b"and", b"as", b"assert", b"async", b"await", b"break", b"class", b"continue",
                b"def", b"del", b"elif", b"else", b"except", b"False", b"finally", b"for",
                b"from", b"global", b"if", b"import", b"in", b"is", b"lambda", b"None",
                b"nonlocal", b"not", b"or", b"pass", b"raise", b"return", b"True", b"try",
                b"while", b"with", b"yield",
            ],
            Self::JavaScript => &[
                b"async", b"await", b"break", b"case", b"catch", b"class", b"const", b"continue",
                b"default", b"delete", b"do", b"else", b"export", b"extends", b"false",
                b"finally", b"for", b"function", b"if", b"import", b"in", b"instanceof",
                b"interface", b"let", b"new", b"null", b"of", b"return", b"static", b"super",
                b"switch", b"this", b"throw", b"true", b"try", b"type", b"typeof", b"undefined",
                b"var", b"void", b"while", b"yield",
            ],
            Self::Shell => &[
                b"case", b"do", b"done", b"elif", b"else", b"esac", b"export", b"fi", b"for",
                b"function", b"if", b"in", b"local", b"return", b"then", b"until", b"while",
            ],
            Self::Json | Self::Yaml | Self::Toml => &[b"true", b"false", b"null"],
            Self::Markdown | Self::Css | Self::Markup | Self::Ini => &[],
        }
    }

    fn line_comment(self) -> Option<&'static [u8]> {
        match self {
            Self::Rust | Self::C | Self::JavaScript => Some(b"//"),
            Self::Python | Self::Shell | Self::Toml | Self::Yaml => Some(b"#"),
            Self::Ini => Some(b";"),
            _ => None,
        }
    }

    /// The language's name as the status bar shows it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::C => "C/C++",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::Shell => "Shell",
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Yaml => "YAML",
            Self::Markdown => "Markdown",
            Self::Markup => "HTML/XML",
            Self::Css => "CSS",
            Self::Ini => "INI",
        }
    }

    fn has_block_comments(self) -> bool {
        matches!(self, Self::Rust | Self::C | Self::JavaScript | Self::Css)
    }
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Classifies `line` of a file in `language`, starting in `state`: the runs, and the state it
/// leaves for the next line.
pub fn line(language: Language, line: &[u8], state: State) -> (Runs, State) {
    let mut runs = Runs::new();
    let state = match language {
        Language::Markdown => markdown(line, state, &mut runs),
        Language::Markup => markup(line, state, &mut runs),
        _ => code(language, line, state, &mut runs),
    };
    (runs, state)
}

fn code(language: Language, line: &[u8], mut state: State, runs: &mut Runs) -> State {
    let mut at = 0;
    let line_start_key = matches!(language, Language::Toml | Language::Yaml | Language::Ini);
    if line_start_key && state == State::Normal {
        let trimmed = line.iter().position(|byte| !byte.is_ascii_whitespace()).unwrap_or(line.len());
        let section = language != Language::Yaml && line.get(trimmed) == Some(&b'[');
        let separator = match language {
            Language::Yaml => b':',
            _ => b'=',
        };
        if section {
            runs.push(Kind::Plain, trimmed);
            runs.push(Kind::Heading, line.len());
            return state;
        }
        if let Some(end) = line.iter().position(|&byte| byte == separator)
            && line[trimmed..end].iter().all(|&byte| is_word(byte) || matches!(byte, b'-' | b'.' | b' ' | b'"'))
            && end > trimmed
        {
            runs.push(Kind::Plain, trimmed);
            runs.push(Kind::Key, end);
            at = end;
        }
        if language == Language::Ini {
            match line[trimmed..].first() {
                Some(b';' | b'#') if at == 0 => {
                    runs.push(Kind::Plain, trimmed);
                    runs.push(Kind::Comment, line.len());
                }
                _ => runs.push(Kind::Plain, line.len()),
            }
            return state;
        }
    }
    while at < line.len() {
        if let State::BlockComment = state {
            match find(&line[at..], b"*/") {
                Some(end) => {
                    at += end + 2;
                    runs.push(Kind::Comment, at);
                    state = State::Normal;
                }
                None => {
                    runs.push(Kind::Comment, line.len());
                    return state;
                }
            }
            continue;
        }
        if let State::LongString(quote) = state {
            let closing = [quote; 3];
            match find(&line[at..], &closing) {
                Some(end) => {
                    at += end + 3;
                    runs.push(Kind::String, at);
                    state = State::Normal;
                }
                None => {
                    runs.push(Kind::String, line.len());
                    return state;
                }
            }
            continue;
        }
        let rest = &line[at..];
        let byte = rest[0];
        if let Some(marker) = language.line_comment()
            && rest.starts_with(marker)
            && (language != Language::Shell || at == 0 || line[at - 1].is_ascii_whitespace())
        {
            runs.push(Kind::Comment, line.len());
            return state;
        }
        if language.has_block_comments() && rest.starts_with(b"/*") {
            state = State::BlockComment;
            at += 2;
            runs.push(Kind::Comment, at);
            continue;
        }
        if language == Language::Python && (rest.starts_with(b"\"\"\"") || rest.starts_with(b"'''")) {
            state = State::LongString(byte);
            at += 3;
            runs.push(Kind::String, at);
            continue;
        }
        if language == Language::Rust && (rest.starts_with(b"#[") || rest.starts_with(b"#![")) {
            let end = rest.iter().position(|&byte| byte == b']').map_or(line.len(), |end| at + end + 1);
            runs.push(Kind::Attribute, end);
            at = end;
            continue;
        }
        if language == Language::C && byte == b'#' && line[..at].iter().all(u8::is_ascii_whitespace) {
            let end = at + 1 + rest[1..].iter().take_while(|&&byte| is_word(byte)).count();
            runs.push(Kind::Macro, end);
            at = end;
            let spaces = line[at..].iter().take_while(|byte| byte.is_ascii_whitespace()).count();
            if line.get(at + spaces) == Some(&b'<') {
                at += spaces;
                runs.push(Kind::Plain, at);
                let end = line[at..].iter().position(|&byte| byte == b'>').map_or(line.len(), |end| at + end + 1);
                runs.push(Kind::String, end);
                at = end;
            }
            continue;
        }
        if language == Language::Css && byte == b'@' {
            let end = at + 1 + rest[1..].iter().take_while(|&&byte| is_word(byte) || byte == b'-').count();
            runs.push(Kind::Keyword, end);
            at = end;
            continue;
        }
        if language == Language::Css && is_word(byte) && !byte.is_ascii_digit() {
            let length = rest.iter().take_while(|&&byte| is_word(byte) || byte == b'-').count();
            let end = at + length;
            let after = line[end..].iter().find(|byte| !byte.is_ascii_whitespace());
            let indented = line.first().is_some_and(u8::is_ascii_whitespace);
            let kind = match after {
                Some(b'(') => Kind::Function,
                Some(b':') if indented => Kind::Key,
                _ => Kind::Plain,
            };
            runs.push(kind, end);
            at = end;
            continue;
        }
        if matches!(language, Language::Python | Language::JavaScript) && byte == b'@' {
            let end = at + 1 + rest[1..].iter().take_while(|&&byte| is_word(byte) || byte == b'.').count();
            runs.push(Kind::Attribute, end);
            at = end;
            continue;
        }
        let rust_lifetime = language == Language::Rust
            && byte == b'\''
            && rest.get(2).is_some_and(|&next| next != b'\'')
            && rest.get(1).is_some_and(|&next| is_word(next));
        if matches!(byte, b'"' | b'\'' | b'`') && !rust_lifetime {
            at = string(line, at, byte, runs);
            continue;
        }
        if byte.is_ascii_digit() && language != Language::Shell {
            let end = at + rest.iter().take_while(|&&byte| is_word(byte) || byte == b'.').count();
            runs.push(Kind::Number, end);
            at = end;
            continue;
        }
        if language == Language::Shell && byte == b'$' {
            let end = at + 1 + rest[1..].iter().take_while(|&&byte| is_word(byte)).count();
            runs.push(Kind::Plain, end);
            at = end;
            continue;
        }
        if is_word(byte) {
            let length = rest[1..].iter().take_while(|&&byte| is_word(byte)).count() + 1;
            let end = at + length;
            let word = &line[at..end];
            let next = line.get(end).copied();
            let data_only = matches!(language, Language::Json | Language::Toml | Language::Yaml);
            let kind = if language.keywords().contains(&word) {
                Kind::Keyword
            } else if language == Language::Shell {
                match line[end..].starts_with(b"()") {
                    true => Kind::Function,
                    false => Kind::Plain,
                }
            } else if data_only {
                Kind::Plain
            } else if language == Language::Rust && next == Some(b'!') {
                Kind::Macro
            } else if next == Some(b'(') {
                Kind::Function
            } else if word[0].is_ascii_uppercase() || is_primitive(language, word) {
                Kind::Type
            } else {
                Kind::Plain
            };
            let end = match kind {
                Kind::Macro => end + 1,
                _ => end,
            };
            runs.push(kind, end);
            at = end;
            continue;
        }
        at += 1;
        runs.push(Kind::Plain, at);
    }
    if language == Language::Json {
        mark_json_keys(line, runs);
    }
    state
}

fn is_primitive(language: Language, word: &[u8]) -> bool {
    let primitives: &[&[u8]] = match language {
        Language::Rust => &[
            b"u8", b"u16", b"u32", b"u64", b"u128", b"usize", b"i8", b"i16", b"i32", b"i64",
            b"i128", b"isize", b"f32", b"f64", b"bool", b"char", b"str",
        ],
        Language::Python => &[
            b"int", b"str", b"float", b"bool", b"bytes", b"list", b"dict", b"set", b"tuple",
        ],
        Language::JavaScript => &[
            b"number", b"string", b"boolean", b"bigint", b"symbol", b"any", b"unknown",
        ],
        _ => &[],
    };
    primitives.contains(&word)
}

fn mark_json_keys(line: &[u8], runs: &mut Runs) {
    let is_string = |kind: Kind| matches!(kind, Kind::String | Kind::Escape);
    let mut fixed = Runs::new();
    let mut first = 0;
    for index in 0..runs.count {
        let (kind, end) = runs.runs[index];
        if !is_string(kind) {
            fixed.push(kind, end as usize);
            first = index + 1;
            continue;
        }
        let string_goes_on = index + 1 < runs.count && is_string(runs.runs[index + 1].0);
        if string_goes_on {
            continue;
        }
        let after = line[end as usize..].iter().find(|byte| !byte.is_ascii_whitespace());
        for &(part, part_end) in &runs.runs[first..=index] {
            let part = match after {
                Some(b':') => Kind::Key,
                _ => part,
            };
            fixed.push(part, part_end as usize);
        }
        first = index + 1;
    }
    *runs = fixed;
}

fn string(line: &[u8], start: usize, quote: u8, runs: &mut Runs) -> usize {
    runs.push(Kind::String, start + 1);
    let mut at = start + 1;
    while at < line.len() {
        match line[at] {
            b'\\' => {
                let end = (at + 2).min(line.len());
                runs.push(Kind::Escape, end);
                at = end;
            }
            byte if byte == quote => {
                at += 1;
                runs.push(Kind::String, at);
                return at;
            }
            _ => {
                at += 1;
                runs.push(Kind::String, at);
            }
        }
    }
    line.len()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn markdown(line: &[u8], state: State, runs: &mut Runs) -> State {
    if line.starts_with(b"```") {
        runs.push(Kind::Plain, line.len());
        return match state {
            State::CodeBlock => State::Normal,
            _ => State::CodeBlock,
        };
    }
    if state == State::CodeBlock {
        runs.push(Kind::Plain, line.len());
        return state;
    }
    if line.first() == Some(&b'#') {
        runs.push(Kind::Heading, line.len());
        return state;
    }
    let mut at = 0;
    while at < line.len() {
        match line[at] {
            b'[' => {
                let close = line[at..].iter().position(|&byte| byte == b')');
                let link = line[at..].windows(2).position(|pair| pair == b"](");
                match (link, close) {
                    (Some(link), Some(close)) if link < close => {
                        runs.push(Kind::Link, at + close + 1);
                        at += close + 1;
                    }
                    _ => {
                        at += 1;
                        runs.push(Kind::Plain, at);
                    }
                }
            }
            _ if line[at..].starts_with(b"http://") || line[at..].starts_with(b"https://") => {
                let end = line[at..].iter().position(u8::is_ascii_whitespace).map_or(line.len(), |end| at + end);
                runs.push(Kind::Link, end);
                at = end;
            }
            _ => {
                at += 1;
                runs.push(Kind::Plain, at);
            }
        }
    }
    state
}

fn markup(line: &[u8], mut state: State, runs: &mut Runs) -> State {
    let mut at = 0;
    while at < line.len() {
        if state == State::MarkupComment {
            match find(&line[at..], b"-->") {
                Some(end) => {
                    at += end + 3;
                    runs.push(Kind::Comment, at);
                    state = State::Normal;
                }
                None => {
                    runs.push(Kind::Comment, line.len());
                    return state;
                }
            }
            continue;
        }
        let rest = &line[at..];
        if rest.starts_with(b"<!--") {
            state = State::MarkupComment;
            at += 4;
            runs.push(Kind::Comment, at);
            continue;
        }
        if rest[0] == b'<' {
            let name_start = at + 1 + usize::from(rest.get(1) == Some(&b'/'));
            let name_length = line[name_start..]
                .iter()
                .take_while(|&&byte| is_word(byte) || matches!(byte, b'-' | b':' | b'?' | b'!'))
                .count();
            runs.push(Kind::Plain, name_start);
            runs.push(Kind::Tag, name_start + name_length);
            at = name_start + name_length;
            while at < line.len() && line[at] != b'>' {
                match line[at] {
                    quote @ (b'"' | b'\'') => at = string(line, at, quote, runs),
                    byte if is_word(byte) => {
                        let end = at + line[at..].iter().take_while(|&&byte| is_word(byte) || byte == b'-').count();
                        runs.push(Kind::Key, end);
                        at = end;
                    }
                    _ => {
                        at += 1;
                        runs.push(Kind::Plain, at);
                    }
                }
            }
            continue;
        }
        if rest[0] == b'&' {
            let end = rest.iter().position(|&byte| byte == b';').map_or(at + 1, |end| at + end + 1);
            runs.push(Kind::Escape, end);
            at = end;
            continue;
        }
        at += 1;
        runs.push(Kind::Plain, at);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(language: Language, text: &[u8]) -> Vec<Kind> {
        let (runs, _) = line(language, text, State::Normal);
        (0..text.len()).map(|at| runs.kind_at(at)).collect()
    }

    #[test]
    fn rust_keywords_strings_comments_and_macros_are_told_apart() {
        let text = b"fn main() { println!(\"a\\n\"); } // done";
        let kinds = kinds(Language::Rust, text);
        assert_eq!(kinds[0], Kind::Keyword);
        assert_eq!(kinds[3], Kind::Function);
        assert_eq!(kinds[12], Kind::Macro);
        assert_eq!(kinds[21], Kind::String);
        assert_eq!(kinds[23], Kind::Escape);
        assert_eq!(*kinds.last().unwrap(), Kind::Comment);
    }

    #[test]
    fn a_block_comment_runs_on_to_the_next_line() {
        let (_, state) = line(Language::C, b"int x; /* begins", State::Normal);
        assert_eq!(state, State::BlockComment);
        let (runs, state) = line(Language::C, b"ends */ int y;", state);
        assert_eq!(runs.kind_at(0), Kind::Comment);
        assert_eq!(runs.kind_at(8), Kind::Keyword);
        assert_eq!(state, State::Normal);
    }

    #[test]
    fn languages_come_from_the_extension_then_the_hash_bang_line() {
        assert_eq!(Language::of(b"name.RS"), Some(Language::Rust));
        assert_eq!(Language::of(b"Cargo.lock"), Some(Language::Toml));
        assert_eq!(Language::detect(b"notes", b"#!/bin/sh"), Some(Language::Shell));
        assert_eq!(Language::detect(b"tool", b"#!/usr/bin/env python3"), Some(Language::Python));
        assert_eq!(Language::detect(b"notes.txt", b"#!/bin/sh"), None);
        assert_eq!(Language::detect(b"README", b"Read me"), None);
    }

    #[test]
    fn the_design_s_rust_lines_get_the_design_s_kinds() {
        use Kind::{Attribute as A, Comment as C, Escape as E, Function as F, Keyword as K};
        use Kind::{Macro as M, Number as N, Plain as P, String as S, Type as T};
        let cases: [(&[u8], &[(usize, Kind)]); 4] = [
            (b"#[derive(Debug)]", &[(0, A), (15, A)]),
            (
                b"pub fn area(r: &Rect) -> u32 {",
                &[(0, K), (4, K), (7, F), (11, P), (16, T), (25, T), (29, P)],
            ),
            (b"    let w = r.width * 2;        // twice", &[(4, K), (8, P), (20, P), (22, N), (32, C)]),
            (
                b"    println!(\"w = {}\\n\", w);",
                &[(4, M), (11, M), (12, P), (13, S), (19, S), (20, E), (21, E), (22, S), (25, P)],
            ),
        ];
        for (text, expected) in cases {
            let (runs, _) = line(Language::Rust, text, State::Normal);
            for &(at, kind) in expected {
                assert_eq!(runs.kind_at(at), kind, "{} at {at}", core::str::from_utf8(text).unwrap());
            }
        }
    }

    #[test]
    fn shell_variables_and_numbers_stay_plain_and_definitions_are_functions() {
        let kinds = kinds(Language::Shell, b"greet() { echo $NAME 42; }");
        assert_eq!(kinds[0], Kind::Function);
        assert_eq!(kinds[10], Kind::Plain);
        assert_eq!(kinds[15], Kind::Plain);
        assert_eq!(kinds[21], Kind::Plain);
    }

    #[test]
    fn json_keys_are_keys_and_values_are_strings() {
        let kinds = kinds(Language::Json, b"{\"name\": \"egg\", \"n\": 3}");
        assert_eq!(kinds[1], Kind::Key);
        assert_eq!(kinds[10], Kind::String);
        assert_eq!(kinds[21], Kind::Number);
    }

    #[test]
    fn c_includes_are_a_directive_and_a_string() {
        let kinds = kinds(Language::C, b"#include <stdio.h>");
        assert_eq!(kinds[0], Kind::Macro);
        assert_eq!(kinds[8], Kind::Plain);
        assert_eq!(kinds[10], Kind::String);
    }
}
