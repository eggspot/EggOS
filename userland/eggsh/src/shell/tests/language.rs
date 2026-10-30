use super::*;

#[test]
fn if_runs_the_first_branch_whose_condition_succeeds() {
    let shell = run("if true; then echo yes; else echo no; fi\n\
         if false; then echo a; elif true; then echo b; fi\n\
         if false; then echo c; fi; echo $?\n\
         if fail; then echo d; else echo e $?; fi");
    assert_eq!(output(&shell), "yes\nb\n0\ne 3\n");
    assert_eq!(errors(&shell), "");
}

#[test]
fn while_until_and_for_loop() {
    let shell = run("set -- a b c; while [ $# -gt 0 ]; do echo $1; shift; done\n\
         until true; do echo never; done; echo until $?\n\
         for word in x 'y z'; do echo \"[$word]\"; done\n\
         for each; do echo $each; done\n\
         set -- p q; for each do echo $each; done");
    assert_eq!(output(&shell), "a\nb\nc\nuntil 0\n[x]\n[y z]\np\nq\n");
    assert_eq!(errors(&shell), "");
}

#[test]
fn break_and_continue_leave_or_go_round_their_loops() {
    let shell = run("for i in 1 2 3; do if [ $i = 2 ]; then continue; fi; echo $i; done\n\
         for i in a b; do for j in 1 2; do echo $i$j; break 2; done; done\n\
         break; echo $?");
    assert_eq!(output(&shell), "1\n3\na1\n0\n");
    assert_eq!(errors(&shell), "eggsh: break: only meaningful in a loop\n");
}

#[test]
fn case_runs_the_first_item_whose_pattern_matches() {
    let shell = run("for x in apple Berry 'a*' ''; do case $x in a*) echo A $x;; \
         [Bb]*|z) echo B;; \"\") echo empty;; (*) echo other;; esac; done\n\
         case none in x) echo x;; esac; echo $?\n\
         case '*' in \"*\") echo star;; *) echo any;; esac");
    assert_eq!(output(&shell), "A apple\nB\nA a*\nempty\n0\nstar\n");
}

#[test]
fn functions_take_arguments_return_a_status_and_keep_locals() {
    let shell = run("greet() { echo hello $1 $#; return 3; echo never; }\n\
         greet world two; echo $?\n\
         X=outer; scoped() { local X=inner; echo $X; }; scoped; echo $X\n\
         type greet\n\
         down() { if [ $# -gt 0 ]; then echo $1; shift; down \"$@\"; fi; }; down a b c\n\
         echo $1");
    assert_eq!(
        output(&shell),
        "hello world 2\n3\ninner\nouter\ngreet is a function\na\nb\nc\none\n"
    );
    assert_eq!(errors(&shell), "");
}

#[test]
fn functions_cannot_nest_without_end() {
    let checked = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            let shell = run("forever() { forever; }; forever; echo $?");
            (output(&shell), errors(&shell))
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(checked.0, "1\n");
    assert_eq!(checked.1, "eggsh: forever: functions nested too deeply\n");
}

#[test]
fn a_brace_group_takes_redirections_and_compound_commands_in_a_pipeline_are_forked() {
    let mut shell = shell(false);
    shell.run_text(b"{ echo a; echo b; } > /tmp/out; (echo sub); { echo piped; } | tool\n\
          f() { echo x; }; f | ls");
    let out = shell.system.redirected[0].2;
    assert_eq!(shell.system.written.get(&out).unwrap(), b"a\nb\n");
    assert_eq!(shell.system.forks, [Group::Shell; 3]);
    assert_eq!(shell.system.ran[0].program, "/usr/bin/tool");
    assert_eq!(shell.system.ran[0].streams.input, shell.system.pipes[0].0);
    assert_eq!(shell.system.waited, [500, 501, 100, 502, 101]);
    assert_eq!(output(&shell), "");
    all_closed(&shell);
}

#[test]
fn a_subshell_runs_in_a_copy_that_ends_with_its_status() {
    let mut shell = shell(false);
    shell.system.fork_as_child = true;
    shell.run_text(b"(echo inside; exit 4); echo never");
    assert_eq!(output(&shell), "inside\n");
    assert_eq!(shell.system.ended_subshells, [4]);
    assert_eq!(shell.exit_status(), 4);
}

#[test]
fn command_substitution_reads_the_copy_s_output() {
    let mut shell = shell(false);
    shell
        .system
        .substitution_outputs
        .push_back(b"out put\n\n".to_vec());
    shell.system.fork_statuses.insert(500, 3 << 8);
    shell.run_text(b"X=$(anything); echo \"[$X]\" $?");
    assert_eq!(output(&shell), "[out put] 3\n");
    all_closed(&shell);
    let mut child = shell_for_child();
    child.run_text(b"echo $(echo inner)");
    let write = child.system.pipes[0].1;
    assert_eq!(child.system.written.get(&write).unwrap(), b"inner\n");
    assert_eq!(child.system.ended_subshells, [0]);
    assert_eq!(output(&child), "");
}

fn shell_for_child() -> Shell<'static, FakeSystem> {
    let mut shell = shell(false);
    shell.system.fork_as_child = true;
    shell
}

#[test]
fn here_documents_give_their_lines_to_standard_input() {
    let mut shell = shell(false);
    shell.run_text(
        b"X=value\nls <<EOF; ls <<'RAW'\nhello $X\nEOF\nnot $X\nRAW\n\
          ls <<-END\n\tindented\n\tEND\necho after",
    );
    assert_eq!(
        shell.system.documents,
        [
            b"hello value\n".to_vec(),
            b"not $X\n".to_vec(),
            b"indented\n".to_vec()
        ]
    );
    assert_eq!(output(&shell), "after\n");
    assert_ne!(shell.system.ran[0].streams.input, 0);
    all_closed(&shell);
}

#[test]
fn a_here_document_at_the_prompt_asks_for_its_lines() {
    let mut shell = shell_with_input(true, &["ls <<EOF\n", "line\n", "EOF\n", "echo done\n"]);
    shell.run_descriptor(0, true);
    assert_eq!(shell.system.documents, [b"line\n".to_vec()]);
    assert_eq!(output(&shell), "done\n");
    assert!(errors(&shell).contains("> > "));
}

#[test]
fn wildcards_become_the_sorted_names_they_match() {
    let shell = run("echo /bin/* /bin/l? /nothing/* '/bin/*' /bin/[fh]*");
    assert_eq!(
        output(&shell),
        "/bin/fail /bin/help /bin/ls /bin/notes /bin/ls /nothing/* /bin/* /bin/fail /bin/help\n"
    );
}

#[test]
fn read_splits_a_line_into_variables() {
    let mut shell = shell(false);
    shell
        .system
        .contents
        .insert("/in".to_owned(), b"one  two three\nsecond\n".to_vec());
    shell.run_text(
        b"read a b < /in; echo \"[$a][$b]\" $?; { read x; read y; } < /in; echo $x $y\n\
          read z < /missing; echo $?",
    );
    assert_eq!(
        output(&shell),
        "[one][two three] 0\none two three second\n1\n"
    );
    assert_eq!(errors(&shell), "eggsh: /missing: No such file or directory\n");
    all_closed(&shell);
}

#[test]
fn test_and_brackets_check_files_strings_and_numbers() {
    let shell = run("[ -d /bin ] && echo dir; test -x /bin/notes || echo plain\n\
         [ 3 -gt 2 ]; echo $?; [ a = b ]; echo $?; [ x; echo $?; test 1 -eq x; echo $?");
    assert_eq!(output(&shell), "dir\nplain\n0\n1\n2\n2\n");
    assert_eq!(
        errors(&shell),
        "eggsh: [: missing `]`\neggsh: test: x: integer expression expected\n"
    );
}

#[test]
fn parameter_operators_and_positional_parameters_expand() {
    let shell = run("echo ${U:-d} ${#HOME} ${HOME#/}; echo ${U:=set} $U; echo ${U:?gone}\n\
         set -- 'a b' c; for x in \"$@\"; do echo \"[$x]\"; done; echo $# \"$*\"; shift; echo $1\n\
         echo ${V:?missing}; echo never");
    assert_eq!(
        output(&shell),
        "d 5 home\nset set\nset\n[a b]\n[c]\n2 a b c\nc\n"
    );
    assert_eq!(errors(&shell), "eggsh: V: missing\n");
    assert_eq!(shell.exit_status(), 1);
}

#[test]
fn eval_runs_its_words_and_set_e_ends_on_a_failure() {
    let shell = run("eval 'Y=1; echo $Y' two");
    assert_eq!(output(&shell), "1 two\n");
    let shell = run("set -e; false || echo ok; if false; then :; fi; ! true; echo still\n\
         false\necho never");
    assert_eq!(output(&shell), "ok\nstill\n");
    assert_eq!(shell.exit_status(), 1);
}

#[test]
fn a_list_in_the_background_runs_in_a_copy() {
    let mut shell = shell(false);
    shell.run_text(b"true && ls &\necho $!");
    assert_eq!(shell.system.forks, [Group::Shell]);
    assert_eq!(output(&shell), "500\n");
    let mut interactive = shell_with_input(true, &["false || ls &\n"]);
    interactive.run_descriptor(0, true);
    assert_eq!(interactive.system.forks, [Group::Background]);
    assert!(errors(&interactive).contains("[1] 500\n"));
}

#[test]
fn a_command_after_and_or_or_a_pipe_may_be_on_the_next_line() {
    let mut shell = shell_with_input(true, &["false ||\n", "echo yes\n", "echo a |\n", "ls\n"]);
    shell.run_descriptor(0, true);
    assert_eq!(output(&shell), "yes\n");
    assert_eq!(shell.system.ran[0].program, "/bin/ls");
    assert_eq!(shell.system.ran[0].streams.input, shell.system.pipes[0].0);
    assert!(errors(&shell).contains("> "));
    let shell = run("true &&\n\n  # a comment\n  echo next");
    assert_eq!(output(&shell), "next\n");
}

#[test]
fn a_compound_command_typed_over_several_lines_runs_once_complete() {
    let mut shell = shell_with_input(true, &["for i in 1 2\n", "do\n", "echo $i\n", "done\n"]);
    shell.run_descriptor(0, true);
    assert_eq!(output(&shell), "1\n2\n");
    assert!(errors(&shell).contains("> > > "));
}

#[test]
fn grammar_errors_are_reported() {
    for (script, message) in [
        (
            "if true; then echo",
            "syntax error: unexpected end of file, looking for `fi`",
        ),
        ("fi", "syntax error near unexpected token `fi`"),
        (
            "for 1 in x; do :; done",
            "syntax error: `for` needs a variable's name",
        ),
        ("{ echo a; } b", "syntax error: a word after a compound command"),
        (
            "case x in a) echo",
            "syntax error: unexpected end of file, looking for `esac`",
        ),
        ("while true; done", "syntax error near unexpected token `done`"),
        (
            "echo $(echo",
            "syntax error: unexpected end of file, looking for the closing `)`",
        ),
        ("a | )", "syntax error near unexpected token `)`"),
    ] {
        let shell = run(script);
        assert_eq!(errors(&shell), format!("eggsh: {message}\n"), "{script}");
        assert_eq!(shell.exit_status(), USAGE, "{script}");
        assert!(shell.system.ran.is_empty(), "{script}");
    }
}
