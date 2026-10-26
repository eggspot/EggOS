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

#[cfg(test)]
mod tests {
    use super::*;

    fn line(arguments: &[&[u8]]) -> String {
        let mut bytes = Vec::new();
        write(arguments.iter().copied(), &mut |byte| bytes.push(byte));
        String::from_utf8(bytes).unwrap()
    }

    fn parse(line: &str) -> Vec<String> {
        let mut arguments = Vec::new();
        let mut current = String::new();
        let mut in_argument = false;
        let mut quoted = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    let mut count = 1;
                    while chars.peek() == Some(&'\\') {
                        chars.next();
                        count += 1;
                    }
                    if chars.peek() == Some(&'"') {
                        current.extend(core::iter::repeat_n('\\', count / 2));
                        if count % 2 == 1 {
                            chars.next();
                            current.push('"');
                        }
                    } else {
                        current.extend(core::iter::repeat_n('\\', count));
                    }
                    in_argument = true;
                }
                '"' => {
                    quoted = !quoted;
                    in_argument = true;
                }
                ' ' | '\t' if !quoted => {
                    if in_argument {
                        arguments.push(core::mem::take(&mut current));
                    }
                    in_argument = false;
                }
                c => {
                    current.push(c);
                    in_argument = true;
                }
            }
        }
        if in_argument {
            arguments.push(current);
        }
        arguments
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
