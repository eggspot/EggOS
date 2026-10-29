//! The command line a Windows program reads (`GetCommandLineW`): its arguments joined with
//! spaces, quoted so that the C runtime's and `CommandLineToArgvW`'s parsing rules (Microsoft's
//! "Parsing C++ command-line arguments") give back exactly the same arguments.

/// Appends `argument` to the command line `out` writes, quoted when it is empty or holds a
/// space, a tab or a double quote.
pub fn push_argument(argument: &[u8], out: &mut impl FnMut(u8)) {
    let needs_quotes = argument.is_empty()
        || argument
            .iter()
            .any(|byte| matches!(byte, b' ' | b'\t' | b'"' | b'\n' | b'\x0b'));
    if !needs_quotes {
        argument.iter().for_each(|byte| out(*byte));
        return;
    }
    out(b'"');
    let mut backslashes = 0;
    for &byte in argument {
        match byte {
            b'\\' => backslashes += 1,
            b'"' => {
                (0..backslashes * 2 + 1).for_each(|_| out(b'\\'));
                out(b'"');
                backslashes = 0;
            }
            byte => {
                (0..backslashes).for_each(|_| out(b'\\'));
                backslashes = 0;
                out(byte);
            }
        }
    }
    (0..backslashes * 2).for_each(|_| out(b'\\'));
    out(b'"');
}

/// Writes `arguments` as one command line through `out`.
pub fn write<'a>(arguments: impl Iterator<Item = &'a [u8]>, out: &mut impl FnMut(u8)) {
    for (index, argument) in arguments.enumerate() {
        if index > 0 {
            out(b' ');
        }
        push_argument(argument, out);
    }
}

/// Where the arguments start in the command line `units` (no NUL): after the program's name,
/// which runs to the first blank or between quotes, and the blanks after it; the C runtime
/// passes the rest to `WinMain` and `wWinMain`.
pub fn arguments_start<T: Copy + Into<u32>>(units: &[T]) -> usize {
    let unit = |at: usize| units.get(at).map(|&unit| unit.into());
    let blank = |unit: u32| unit == u32::from(b' ') || unit == u32::from(b'\t');
    let mut at = 0;
    if unit(0) == Some(u32::from(b'"')) {
        at = 1;
        while let Some(next) = unit(at) {
            at += 1;
            if next == u32::from(b'"') {
                break;
            }
        }
    } else {
        while unit(at).is_some_and(|next| !blank(next)) {
            at += 1;
        }
    }
    while unit(at).is_some_and(blank) {
        at += 1;
    }
    at
}

/// One step of splitting a command line: a UTF-16 unit of the current argument, or its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece {
    Unit(u16),
    End,
}

const BACKSLASH: u16 = b'\\' as u16;
const QUOTE: u16 = b'"' as u16;

fn is_blank(unit: u16) -> bool {
    unit == u16::from(b' ') || unit == u16::from(b'\t')
}

/// Splits the UTF-16 command line `units` into arguments as `CommandLineToArgvW` does
/// (Microsoft's "Parsing C++ command-line arguments"): the program name runs to the first
/// blank, or between quotes with no escapes; after it, `2n` backslashes before a quote are `n`
/// backslashes and the quote toggles quoting, `2n + 1` are `n` and a literal quote, `""`
/// inside quotes is a literal quote, and other backslashes are themselves.
pub fn split(units: &[u16], out: &mut impl FnMut(Piece)) {
    if units.is_empty() {
        return;
    }
    let mut at = 0;
    if units[0] == QUOTE {
        at = 1;
        while at < units.len() && units[at] != QUOTE {
            out(Piece::Unit(units[at]));
            at += 1;
        }
        at += 1;
    } else {
        while at < units.len() && !is_blank(units[at]) {
            out(Piece::Unit(units[at]));
            at += 1;
        }
    }
    out(Piece::End);
    let mut started = false;
    let mut quoted = false;
    while at < units.len() {
        let unit = units[at];
        at += 1;
        match unit {
            BACKSLASH => {
                let mut count = 1;
                while units.get(at) == Some(&BACKSLASH) {
                    count += 1;
                    at += 1;
                }
                let before_quote = units.get(at) == Some(&QUOTE);
                let backslashes = if before_quote { count / 2 } else { count };
                (0..backslashes).for_each(|_| out(Piece::Unit(BACKSLASH)));
                if before_quote && count % 2 == 1 {
                    out(Piece::Unit(QUOTE));
                    at += 1;
                }
                started = true;
            }
            QUOTE if quoted && units.get(at) == Some(&QUOTE) => {
                out(Piece::Unit(QUOTE));
                at += 1;
            }
            QUOTE => {
                quoted = !quoted;
                started = true;
            }
            unit if is_blank(unit) && !quoted => {
                if started {
                    out(Piece::End);
                }
                started = false;
            }
            unit => {
                out(Piece::Unit(unit));
                started = true;
            }
        }
    }
    if started {
        out(Piece::End);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_main_gets_the_line_after_the_program_s_name() {
        let start = |line: &str| arguments_start(line.as_bytes());
        assert_eq!(start("eggpad.exe  C:\\tmp\\a.txt"), 12);
        assert_eq!(start("\"C:\\My Apps\\pad.exe\" -x"), 21);
        assert_eq!(start("pad.exe"), 7);
        assert_eq!(start("\"unterminated"), 13);
        assert_eq!(start(""), 0);
        let wide: Vec<u16> = "a\tb".encode_utf16().collect();
        assert_eq!(arguments_start(&wide), 2);
    }

    fn line(arguments: &[&[u8]]) -> String {
        let mut bytes = Vec::new();
        write(arguments.iter().copied(), &mut |byte| bytes.push(byte));
        String::from_utf8(bytes).unwrap()
    }

    fn parse(line: &str) -> Vec<String> {
        let units: Vec<u16> = line.encode_utf16().collect();
        let mut arguments = vec![Vec::new()];
        split(&units, &mut |piece| match piece {
            Piece::Unit(unit) => arguments.last_mut().unwrap().push(unit),
            Piece::End => arguments.push(Vec::new()),
        });
        arguments.pop();
        arguments
            .iter()
            .map(|argument| String::from_utf16(argument).unwrap())
            .collect()
    }

    #[test]
    fn the_program_name_takes_no_escapes_and_empty_lines_have_none() {
        assert_eq!(parse(r#""C:\a b\x.exe" y"#), [r"C:\a b\x.exe", "y"]);
        assert_eq!(parse(r"C:\dir\x.exe\ a"), [r"C:\dir\x.exe\", "a"]);
        assert_eq!(parse(r#"x.exe "a""b" "c"#), ["x.exe", r#"a"b"#, "c"]);
        assert_eq!(parse("x.exe   "), ["x.exe"]);
        assert_eq!(parse(""), Vec::<String>::new());
    }

    #[test]
    fn plain_arguments_are_joined_with_spaces() {
        assert_eq!(line(&[b"hello.exe", b"a", b"b"]), "hello.exe a b");
    }

    #[test]
    fn arguments_with_spaces_quotes_and_backslashes_are_quoted_to_parse_back_the_same() {
        let cases: [&[&[u8]]; 4] = [
            &[b"/bin/hello.exe", b"b c", b""],
            &[b"x", br#"say "hi""#, br"C:\dir\", br"a\\b"],
            &[br#"\"#, br#"\""#, br#"a\\"b"#, b"tab\there"],
            &[b"end\\\\", b"\"", b"  "],
        ];
        for arguments in cases {
            let text = line(arguments);
            let expected: Vec<String> = arguments
                .iter()
                .map(|argument| String::from_utf8(argument.to_vec()).unwrap())
                .collect();
            assert_eq!(parse(&text), expected, "{text}");
        }
        assert_eq!(line(&[br"C:\dir\"]), r"C:\dir\");
        assert_eq!(line(&[br"a b\"]), r#""a b\\""#);
    }
}
