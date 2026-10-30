use super::*;

#[derive(Default)]
struct Fixed {
    assigned: Vec<(String, String)>,
    failed: Vec<(String, String)>,
    substituted: Vec<(String, bool)>,
}

impl Parameters for Fixed {
    fn value(&self, name: &[u8]) -> Value<'_> {
        if let Some((_, value)) = self
            .assigned
            .iter()
            .rev()
            .find(|(assigned, _)| assigned.as_bytes() == name)
        {
            return Value::Text(value.as_bytes());
        }
        match name {
            b"HOME" => Value::Text(b"/home/egg"),
            b"X" => Value::Text(b"abc"),
            b"SPACED" => Value::Text(b"  one two\tthree "),
            b"EMPTY" => Value::Text(b""),
            b"PATH" => Value::Text(b"/usr/lib/file.tar.gz"),
            b"STAR" => Value::Text(b"*.rs"),
            b"?" => Value::Number(127),
            b"$" => Value::Number(42),
            b"#" => Value::Number(2),
            b"0" => Value::Text(b"eggsh"),
            b"1" => Value::Text(b"first arg"),
            b"2" => Value::Text(b""),
            _ => Value::Unset,
        }
    }

    fn positional_count(&self) -> usize {
        2
    }

    fn positional(&self, number: usize) -> &[u8] {
        match number {
            1 => b"first arg",
            _ => b"",
        }
    }

    fn assign(&mut self, name: &[u8], value: &[u8]) -> Result<(), SyntaxError> {
        let text = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).unwrap();
        self.assigned.push((text(name), text(value)));
        Ok(())
    }

    fn fail(&mut self, name: &[u8], message: &[u8]) {
        let text = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).unwrap();
        self.failed.push((text(name), text(message)));
    }

    fn substitute(
        &mut self,
        command: &[u8],
        backquoted: bool,
        output: &mut dyn FnMut(&[u8]) -> Result<(), SyntaxError>,
    ) -> Result<(), SyntaxError> {
        let command = String::from_utf8(command.to_vec()).unwrap();
        self.substituted.push((command.clone(), backquoted));
        let printed = command.strip_prefix("echo ").unwrap_or("");
        output(printed.as_bytes())?;
        output(b"\n\n")
    }
}

fn collected(words: &Words) -> Vec<String> {
    (0..words.count())
        .map(|index| String::from_utf8(words.get(index).to_vec()).unwrap())
        .collect()
}

fn parse_with(input: &str, fixed: &mut Fixed) -> Result<(Vec<String>, Parsed), SyntaxError> {
    let mut words = Words::default();
    let parsed = next_command(input.as_bytes(), true, fixed, &mut words)?;
    Ok((collected(&words), parsed))
}

fn parse(input: &str) -> Result<(Vec<String>, Parsed), SyntaxError> {
    parse_with(input, &mut Fixed::default())
}

fn words(input: &str) -> Vec<String> {
    parse(input).unwrap().0
}

#[test]
fn blanks_split_words_and_end_the_command() {
    assert_eq!(words("  echo   a\tb  "), ["echo", "a", "b"]);
    assert_eq!(words(""), Vec::<String>::new());
    let (all, parsed) = parse("a b\nc").unwrap();
    assert_eq!(all, ["a", "b"]);
    assert_eq!(parsed.consumed, 4);
    assert_eq!(parsed.terminator, Terminator::Newline);
}

#[test]
fn single_quotes_keep_everything() {
    assert_eq!(
        words(r#"echo 'a  b $X \n "q"' x'y'z ''"#),
        ["echo", r#"a  b $X \n "q""#, "xyz", ""]
    );
}

#[test]
fn double_quotes_expand_without_splitting_and_escape_four_characters() {
    assert_eq!(
        words(
            r#"echo "$X $SPACED" "\$X \" \\ \a" "a\
b""#
        ),
        ["echo", "abc   one two\tthree ", r#"$X " \ \a"#, "ab"]
    );
    assert_eq!(words(r#""""#), [""]);
    assert_eq!(words(r#""$UNSET""#), [""]);
}

#[test]
fn backslash_keeps_the_next_character_and_joins_lines() {
    assert_eq!(words(r"a\ b \$X \\ c\;d"), [r"a b", "$X", r"\", "c;d"]);
    assert_eq!(words("ab\\\ncd"), ["abcd"]);
    assert_eq!(words(r"end\"), [r"end\"]);
    let mut words = Words::default();
    let incomplete = next_command(br"end\", false, &mut Fixed::default(), &mut words);
    assert_eq!(incomplete, Err(SyntaxError::Incomplete));
}

#[test]
fn unquoted_expansions_are_split_on_blanks_and_empty_ones_vanish() {
    assert_eq!(
        words("cmd $SPACED x$X$X y${X}z $EMPTY $UNSET"),
        ["cmd", "one", "two", "three", "xabcabc", "yabcz"]
    );
    assert_eq!(
        words(r#"cmd ""$SPACED"#),
        ["cmd", "", "one", "two", "three"]
    );
    assert_eq!(words("a$SPACED"), ["a", "one", "two", "three"]);
    assert_eq!(words("$EMPTY; b").len(), 0);
}

#[test]
fn ifs_decides_where_fields_split() {
    let mut fixed = Fixed::default();
    fixed.assigned.push(("IFS".into(), ":".into()));
    fixed.assigned.push(("V".into(), "a::b:c d:".into()));
    assert_eq!(parse_with("x $V", &mut fixed).unwrap().0, ["x", "a", "", "b", "c d"]);
    fixed.assigned.push(("IFS".into(), ", ".into()));
    fixed.assigned.push(("V".into(), " a , b,,c ".into()));
    assert_eq!(parse_with("x $V", &mut fixed).unwrap().0, ["x", "a", "b", "", "c"]);
    fixed.assigned.push(("IFS".into(), String::new()));
    fixed.assigned.push(("V".into(), "a b".into()));
    assert_eq!(parse_with("x $V", &mut fixed).unwrap().0, ["x", "a b"]);
}

#[test]
fn special_parameters_expand() {
    assert_eq!(
        words("echo $? $$ $# $0 ${?} \"$1\" $1x $"),
        ["echo", "127", "42", "2", "eggsh", "127", "first arg", "first", "argx", "$"]
    );
    assert_eq!(words("echo $- $/ a$"), ["echo", "$-", "$/", "a$"]);
}

#[test]
fn at_and_star_give_the_positional_parameters() {
    assert_eq!(words("a \"$@\" b"), ["a", "first arg", "", "b"]);
    assert_eq!(words("a $@ b"), ["a", "first", "arg", "b"]);
    assert_eq!(words("a \"$*\" b"), ["a", "first arg ", "b"]);
    assert_eq!(words("a \"x$@y\""), ["a", "xfirst arg", "y"]);
    assert_eq!(words("A=$@ a"), ["A=first arg ", "a"]);
    assert_eq!(words("${#} ${@}"), ["2", "first", "arg"]);
    struct NoArguments;
    impl Parameters for NoArguments {
        fn value(&self, _: &[u8]) -> Value<'_> {
            Value::Unset
        }
    }
    let mut words = Words::default();
    next_command(b"a \"$@\" \"x$@\" b", true, &mut NoArguments, &mut words).unwrap();
    assert_eq!(collected(&words), ["a", "x", "b"]);
}

#[test]
fn parameter_operators_choose_defaults_alternatives_and_lengths() {
    for (input, expected) in [
        ("${UNSET:-def ault}", vec!["def", "ault"]),
        ("\"${UNSET:-def ault}\"", vec!["def ault"]),
        ("${EMPTY-x}|", vec![]),
        ("${EMPTY:-x}", vec!["x"]),
        ("${X:-unused}", vec!["abc"]),
        ("${X:+alt}", vec!["alt"]),
        ("${UNSET:+alt}", vec![]),
        ("\"${UNSET:+alt}\"", vec![""]),
        ("${#X} ${#UNSET} ${#?}", vec!["3", "0", "3"]),
        ("${UNSET:-$X}", vec!["abc"]),
        ("${UNSET:-'a b'}", vec!["a b"]),
        ("${UNSET:-${X}}", vec!["abc"]),
    ] {
        let input = input.trim_end_matches('|');
        assert_eq!(words(input), expected, "{input}");
    }
}

#[test]
fn removing_a_pattern_takes_the_shortest_or_the_longest_match() {
    assert_eq!(words("${PATH#*/}"), ["usr/lib/file.tar.gz"]);
    assert_eq!(words("${PATH##*/}"), ["file.tar.gz"]);
    assert_eq!(words("${PATH%.*}"), ["/usr/lib/file.tar"]);
    assert_eq!(words("${PATH%%.*}"), ["/usr/lib/file"]);
    assert_eq!(words("${PATH#\"*\"}"), ["/usr/lib/file.tar.gz"]);
    assert_eq!(words("${X#$X}x"), ["x"]);
}

#[test]
fn assigning_a_default_sets_the_variable_and_an_error_stops_the_command() {
    let mut fixed = Fixed::default();
    assert_eq!(parse_with("echo ${NEW:=a b}", &mut fixed).unwrap().0, ["echo", "a", "b"]);
    assert_eq!(fixed.assigned, [("NEW".to_owned(), "a b".to_owned())]);
    assert_eq!(parse_with("echo ${3:=x}", &mut fixed), Err(SyntaxError::BadSubstitution));
    assert_eq!(parse_with("echo ${UNSET:?gone $X}", &mut fixed), Err(SyntaxError::Failed));
    assert_eq!(parse_with("echo ${EMPTY:?}", &mut fixed), Err(SyntaxError::Failed));
    assert_eq!(
        fixed.failed,
        [
            ("UNSET".to_owned(), "gone abc".to_owned()),
            ("EMPTY".to_owned(), "parameter null or not set".to_owned())
        ]
    );
    assert_eq!(parse_with("echo ${EMPTY?}", &mut fixed).unwrap().0, ["echo"]);
}

#[test]
fn command_substitutions_run_and_lose_their_trailing_newlines() {
    let mut fixed = Fixed::default();
    let (all, _) = parse_with("a $(echo b  c) \"$(echo d  e)\" x`echo f`", &mut fixed).unwrap();
    assert_eq!(all, ["a", "b", "c", "d  e", "xf"]);
    assert_eq!(
        fixed.substituted,
        [
            ("echo b  c".to_owned(), false),
            ("echo d  e".to_owned(), false),
            ("echo f".to_owned(), true)
        ]
    );
    let (all, _) = parse_with("a $(echo (x) $(echo y)) b", &mut fixed).unwrap();
    assert_eq!(all, ["a", "(x)", "$(echo", "y)", "b"]);
    assert_eq!(parse("a $((1+2))"), Err(SyntaxError::NotSupported("$((")));
    assert_eq!(parse("a $(echo"), Err(SyntaxError::UnexpectedEnd(b'(')));
    assert_eq!(parse("a `echo"), Err(SyntaxError::UnexpectedEnd(b'`')));
}

#[test]
fn checking_syntax_runs_nothing() {
    let mut words = Words::default();
    let parsed = next_command(b"a $(rm x) `y` ${Z:?no}; b", true, &mut NoParameters, &mut words);
    assert_eq!(parsed.unwrap().terminator, Terminator::Semicolon);
}

#[test]
fn a_substitution_s_end_skips_quotes_and_nesting() {
    assert_eq!(substitution_end(b"echo a)", true), Ok(6));
    assert_eq!(substitution_end(b"echo ')' \")\" \\) $(b) (c))x", true), Ok(24));
    assert_eq!(substitution_end(b"echo # )\n)", true), Ok(9));
    assert_eq!(substitution_end(b"echo (", false), Err(SyntaxError::Incomplete));
}

#[test]
fn tilde_is_home_only_alone_or_before_a_slash_at_a_word_s_start() {
    assert_eq!(
        words("cd ~ ~/bin a~ ~x '~' ~;"),
        ["cd", "/home/egg", "/home/egg/bin", "a~", "~x", "~", "/home/egg"]
    );
}

#[test]
fn assignments_come_only_before_the_command() {
    let mut words = Words::default();
    let mut fixed = Fixed::default();
    next_command(b"A=1 B=\"x y\" C=$SPACED cmd D=2", true, &mut fixed, &mut words).unwrap();
    assert_eq!(words.assignment_count(), 3);
    let assignments: Vec<&[u8]> = words.assignments().collect();
    assert_eq!(assignments, [&b"A=1"[..], b"B=x y", b"C=  one two\tthree "]);
    assert_eq!(words.command_length(), 2);
    assert_eq!(words.command(0), b"cmd");
    assert_eq!(words.command(1), b"D=2");
    assert_eq!(words.command_with_nul(1), b"D=2\0");
    next_command(b"1A=x =y 'A'=z", true, &mut fixed, &mut words).unwrap();
    assert_eq!(words.assignment_count(), 0);
    assert_eq!(words.command_length(), 3);
    next_command(b"ONLY=", true, &mut fixed, &mut words).unwrap();
    assert_eq!(words.assignment_count(), 1);
    next_words(b"A=1 b", true, &mut fixed, &mut words).unwrap();
    assert_eq!(words.assignment_count(), 0);
    assert_eq!(words.count(), 2);
}

#[test]
fn comments_run_to_the_end_of_the_line() {
    let (all, parsed) = parse("echo a#b # c 'd\nnext").unwrap();
    assert_eq!(all, ["echo", "a#b"]);
    assert_eq!(parsed.terminator, Terminator::Newline);
    assert_eq!(parsed.consumed, 16);
    assert_eq!(words("# only"), Vec::<String>::new());
}

#[test]
fn terminators_end_a_command() {
    for (input, terminator, consumed) in [
        ("a b; c", Terminator::Semicolon, 4),
        ("a && b", Terminator::And, 4),
        ("a||b", Terminator::Or, 3),
        ("a & b", Terminator::Background, 3),
        ("a | b", Terminator::Pipe, 3),
        ("a )", Terminator::Close, 2),
        ("a ;; b", Terminator::CaseEnd, 2),
    ] {
        let (all, parsed) = parse(input).unwrap();
        assert_eq!(all[0], "a", "{input}");
        assert_eq!(parsed.terminator, terminator, "{input}");
        assert_eq!(parsed.consumed, consumed, "{input}");
    }
    for (input, token) in [("; a", ";"), ("| a", "|"), ("&& a", "&&"), ("echo >", "newline")] {
        assert_eq!(parse(input), Err(SyntaxError::UnexpectedToken(token)), "{input}");
    }
    assert_eq!(parse("echo a(b"), Err(SyntaxError::UnexpectedToken("(")));
}

#[test]
fn operators_not_supported_yet_are_refused_not_read_as_words() {
    for (input, operator) in [
        ("a <<< f", "<<<"),
        ("a &> f", "&>"),
        ("a |& b", "|&"),
        ("a >&f", ">&"),
        ("a 2>&-", ">&"),
    ] {
        assert_eq!(parse(input), Err(SyntaxError::NotSupported(operator)), "{input}");
    }
    assert_eq!(words("echo '|' \\& \"<>\""), ["echo", "|", "&", "<>"]);
}

#[test]
fn unterminated_quotes_need_more_text_or_fail_at_the_end() {
    let mut words = Words::default();
    for input in ["echo 'a", "echo \"a", "echo ${X", "echo $X", "echo a", "# c", "a $(b", "a <<"] {
        let result = next_command(input.as_bytes(), false, &mut Fixed::default(), &mut words);
        assert_eq!(result, Err(SyntaxError::Incomplete), "{input}");
    }
    assert_eq!(parse("echo 'a"), Err(SyntaxError::UnexpectedEnd(b'\'')));
    assert_eq!(parse("echo \"a"), Err(SyntaxError::UnexpectedEnd(b'"')));
    assert_eq!(parse("echo ${X"), Err(SyntaxError::UnexpectedEnd(b'{')));
}

#[test]
fn bad_substitutions_are_refused() {
    for input in ["${}", "${1x}", "${X:}", "${X^}", "${#X:-a}"] {
        assert_eq!(parse(input), Err(SyntaxError::BadSubstitution), "{input}");
    }
}

#[test]
fn too_many_words_or_bytes_are_refused() {
    let many = "a ".repeat(MAX_WORDS + 1);
    assert_eq!(parse(&many), Err(SyntaxError::TooLong));
    let long = "a".repeat(WORDS_CAPACITY);
    assert_eq!(parse(&long), Err(SyntaxError::TooLong));
    let fits = "a ".repeat(MAX_WORDS);
    assert_eq!(words(&fits).len(), MAX_WORDS);
}

#[test]
fn quoted_pattern_characters_are_escaped_and_plain_ones_make_a_pattern() {
    let mut words = Words::default();
    let mut fixed = Fixed::default();
    next_command(b"ls *.c '*'x \"a?\"[b] $STAR plain", true, &mut fixed, &mut words).unwrap();
    let patterns: Vec<bool> = (0..words.count()).map(|index| words.is_pattern(index)).collect();
    assert_eq!(patterns, [false, true, false, true, true, false]);
    let mut buffer = [0; 32];
    assert_eq!(words.pattern(2, &mut buffer), Some(&br"\*x"[..]));
    assert_eq!(words.pattern(3, &mut buffer), Some(&br"a\?[b]"[..]));
    assert_eq!(words.pattern(4, &mut buffer), Some(&b"*.rs"[..]));
}

#[test]
fn a_word_is_replaced_by_its_matches() {
    let mut words = Words::default();
    next_command(b"ls *.c end", true, &mut Fixed::default(), &mut words).unwrap();
    words.replace(1, &[b"a.c", b"bb.c", b"c.c"]).unwrap();
    assert_eq!(collected(&words), ["ls", "a.c", "bb.c", "c.c", "end"]);
    assert_eq!(words.command_with_nul(4), b"end\0");
    words.replace(4, &[b"x"]).unwrap();
    assert_eq!(collected(&words), ["ls", "a.c", "bb.c", "c.c", "x"]);
    let many = vec![&b"m"[..]; MAX_WORDS];
    assert_eq!(words.replace(1, &many), Err(SyntaxError::TooLong));
}

#[test]
fn a_digit_before_a_redirection_names_the_descriptor_and_ampersand_copies_one() {
    let mut words = Words::default();
    next_command(
        b"cc 2> err 2>&1 >&2 x2>y 2 >|z",
        true,
        &mut Fixed::default(),
        &mut words,
    )
    .unwrap();
    let file = |descriptor, redirection, path| Redirect::File {
        descriptor,
        redirection,
        path,
    };
    let redirections: Vec<_> = words.redirections().collect();
    assert_eq!(
        redirections,
        [
            file(2, Redirection::Output, &b"err\0"[..]),
            Redirect::Duplicate {
                descriptor: 2,
                source: 1
            },
            Redirect::Duplicate {
                descriptor: 1,
                source: 2
            },
            file(1, Redirection::Output, &b"y\0"[..]),
            file(1, Redirection::Output, &b"z\0"[..]),
        ]
    );
    let words_after: Vec<&[u8]> = (0..words.command_length())
        .map(|index| words.command(index))
        .collect();
    assert_eq!(words_after, [&b"cc"[..], b"x2", b"2"]);
}

#[test]
fn here_document_operators_record_their_position() {
    let mut words = Words::default();
    let parsed = next_command(b"cat <<EOF 3<<-'E F' x\nbody", true, &mut Fixed::default(), &mut words)
        .unwrap();
    assert_eq!(parsed.terminator, Terminator::Newline);
    let redirections: Vec<_> = words.redirections().collect();
    assert_eq!(
        redirections,
        [
            Redirect::Document {
                descriptor: 0,
                position: 4
            },
            Redirect::Document {
                descriptor: 3,
                position: 11
            }
        ]
    );
    assert_eq!(collected(&words), ["cat", "x"]);
    let delimiter = document_delimiter(b"<<EOF x").unwrap();
    assert_eq!((delimiter.text(), delimiter.quoted, delimiter.strip_tabs), (&b"EOF"[..], false, false));
    let delimiter = document_delimiter(b"<<- 'E F'\"x\"\\y").unwrap();
    assert_eq!((delimiter.text(), delimiter.quoted, delimiter.strip_tabs), (&b"E Fxy"[..], true, true));
    assert_eq!(parse("cat <<"), Err(SyntaxError::UnexpectedToken("newline")));
}

#[test]
fn a_here_document_ends_at_its_delimiter_line() {
    let text = b"cat <<E\none\n\tE\ntwo\nE\nafter";
    assert_eq!(document_end(text, 8, b"E", false, true), Some((19, 21)));
    assert_eq!(document_end(text, 8, b"E", true, true), Some((12, 15)));
    assert_eq!(document_end(b"x\nE", 0, b"E", false, false), None);
    assert_eq!(document_end(b"x\nE", 0, b"E", false, true), Some((2, 3)));
    assert_eq!(document_end(b"x\n", 0, b"E", false, true), None);
}

#[test]
fn a_here_document_s_body_expands_dollars_and_backquotes_only() {
    let mut words = Words::default();
    let mut fixed = Fixed::default();
    expand_document(b"$X \"q\" '$1' \\$X \\a `echo b`\n", &mut fixed, &mut words).unwrap();
    assert_eq!(words.get(0), b"abc \"q\" 'first arg' $X \\a b\n");
}

#[test]
fn reserved_words_are_whole_unquoted_first_words() {
    for (input, expected) in [
        ("if true", Some(Keyword::If)),
        ("then\n", Some(Keyword::Then)),
        ("{ a; }", Some(Keyword::OpenBrace)),
        ("}", Some(Keyword::CloseBrace)),
        ("done;", Some(Keyword::Done)),
        ("! a", Some(Keyword::Bang)),
        ("iffy", None),
        ("'if'", None),
        ("{a", None),
        ("", None),
    ] {
        assert_eq!(keyword(input.as_bytes(), true), Ok(expected), "{input}");
    }
    assert_eq!(keyword(b"fi", false), Err(SyntaxError::Incomplete));
    assert_eq!(function_definition(b"greet () { a; }"), Some((5, 8)));
    assert_eq!(function_definition(b"f()"), Some((1, 3)));
    assert_eq!(function_definition(b"f (x)"), None);
    assert_eq!(function_definition(b"1f()"), None);
}

#[test]
fn one_word_is_read_without_splitting() {
    let mut words = Words::default();
    let mut fixed = Fixed::default();
    assert_eq!(next_word(b"  $SPACED) x", true, &mut fixed, &mut words), Ok(Some(9)));
    assert_eq!(words.get(0), b"  one two\tthree ");
    assert_eq!(next_word(b" | x", true, &mut fixed, &mut words), Ok(None));
    assert_eq!(next_word(b" $UNSET in", true, &mut fixed, &mut words), Ok(Some(7)));
    assert_eq!(words.count(), 0);
}

#[test]
fn a_line_ends_only_at_a_newline_outside_quotes_comments_and_escapes() {
    let mut quoting = Quoting::WordStart;
    assert_eq!(line_end(&mut quoting, b"echo a\nnext"), Some(7));
    assert_eq!(quoting, Quoting::WordStart);
    let text = b"echo 'a\nb' \"c\nd\" e\\\nf # it's \"\nls\n";
    let mut quoting = Quoting::WordStart;
    let first = line_end(&mut quoting, text).unwrap();
    assert_eq!(&text[..first], b"echo 'a\nb' \"c\nd\" e\\\nf # it's \"\n");
    assert_eq!(line_end(&mut quoting, &text[first..]), Some(3));
}

#[test]
fn decimal_writes_every_digit() {
    let mut buffer = [0; 20];
    assert_eq!(decimal(0, &mut buffer), b"0");
    assert_eq!(decimal(u64::MAX, &mut buffer), b"18446744073709551615");
}
