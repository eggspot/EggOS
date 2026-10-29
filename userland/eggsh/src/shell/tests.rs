use super::*;
use crate::path::Kind;
use std::collections::{BTreeSet, HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Started {
    program: String,
    arguments: Vec<String>,
    environment: Vec<String>,
    job_control: bool,
    streams: Streams,
    group: Group,
}

#[derive(Debug, Default)]
struct FakeSystem {
    files: Vec<(String, Kind)>,
    contents: HashMap<String, Vec<u8>>,
    open: HashMap<usize, VecDeque<u8>>,
    input: VecDeque<Read>,
    working_directory: String,
    output: Vec<u8>,
    errors: Vec<u8>,
    ran: Vec<Started>,
    statuses: HashMap<String, i32>,
    run_error: Option<Errno>,
    descriptors_given: usize,
    live: BTreeSet<usize>,
    written: HashMap<usize, Vec<u8>>,
    redirected: Vec<(String, Redirection, usize)>,
    pipes: Vec<(usize, usize)>,
    waited: Vec<i32>,
    terminal_taken_back: usize,
}

impl FakeSystem {
    fn descriptor(&mut self) -> usize {
        let descriptor = 3 + self.descriptors_given;
        self.descriptors_given += 1;
        self.live.insert(descriptor);
        descriptor
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.strip_suffix(b"\0").unwrap_or(bytes).to_vec()).unwrap()
}

impl Files for FakeSystem {
    fn kind(&mut self, path: &[u8]) -> Kind {
        let path = text(path);
        self.files
            .iter()
            .find(|(name, _)| *name == path)
            .map_or(Kind::Missing, |(_, kind)| *kind)
    }
}

impl System for FakeSystem {
    fn write(&mut self, descriptor: usize, bytes: &[u8]) {
        match descriptor {
            1 => self.output.extend_from_slice(bytes),
            2 => self.errors.extend_from_slice(bytes),
            _ if self.live.contains(&descriptor) => self
                .written
                .entry(descriptor)
                .or_default()
                .extend_from_slice(bytes),
            _ => panic!("write to {descriptor}"),
        }
    }

    fn read(&mut self, descriptor: usize, buffer: &mut [u8]) -> Read {
        if let Some(file) = self.open.get_mut(&descriptor) {
            let piece = match descriptor {
                0 => file
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .map_or(file.len(), |at| at + 1),
                _ => 7,
            };
            let count = file.len().min(buffer.len()).min(piece);
            for slot in &mut buffer[..count] {
                *slot = file.pop_front().unwrap();
            }
            return if count == 0 {
                Read::End
            } else {
                Read::Bytes(count)
            };
        }
        assert_eq!(descriptor, 0);
        match self.input.pop_front() {
            Some(Read::Bytes(_)) => unreachable!(),
            Some(read) => read,
            None => Read::End,
        }
    }

    fn open(&mut self, path: &[u8]) -> Result<usize, Errno> {
        let content = self.contents.get(&text(path)).ok_or(Errno::ENOENT)?.clone();
        let descriptor = self.descriptor();
        self.open
            .insert(descriptor, content.iter().copied().collect());
        Ok(descriptor)
    }

    fn close(&mut self, descriptor: usize) {
        assert!(self.live.remove(&descriptor), "close of {descriptor}");
        self.open.remove(&descriptor);
    }

    fn open_redirection(&mut self, path: &[u8], redirection: Redirection) -> Result<usize, Errno> {
        assert_eq!(path.last(), Some(&0));
        let path = text(path);
        if redirection == Redirection::Input && !self.contents.contains_key(&path) {
            return Err(Errno::ENOENT);
        }
        if path.starts_with("/readonly/") {
            return Err(Errno(30));
        }
        let descriptor = self.descriptor();
        self.redirected.push((path, redirection, descriptor));
        Ok(descriptor)
    }

    fn pipe(&mut self) -> Result<(usize, usize), Errno> {
        let ends = (self.descriptor(), self.descriptor());
        self.pipes.push(ends);
        Ok(ends)
    }

    fn wait(&mut self, child: i32) -> Result<i32, Errno> {
        self.waited.push(child);
        let program = &self.ran[(child - 100) as usize].program;
        Ok(self.statuses.get(program).copied().unwrap_or(0))
    }

    fn take_back_terminal(&mut self) {
        self.terminal_taken_back += 1;
    }

    fn change_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        let path = text(path);
        let absolute = if path.starts_with('/') {
            path
        } else if self.working_directory == "/" {
            format!("/{path}")
        } else {
            format!("{}/{path}", self.working_directory)
        };
        match self.kind(format!("{absolute}\0").as_bytes()) {
            Kind::Directory => {
                self.working_directory = absolute;
                Ok(())
            }
            Kind::Missing => Err(Errno::ENOENT),
            _ => Err(Errno::ENOTDIR),
        }
    }

    fn working_directory<'b>(&mut self, buffer: &'b mut [u8; PATH_CAPACITY]) -> Option<&'b [u8]> {
        let directory = self.working_directory.as_bytes();
        buffer[..directory.len()].copy_from_slice(directory);
        Some(&buffer[..directory.len()])
    }

    fn start(
        &mut self,
        program: &[u8],
        arguments: &[&[u8]],
        environment: &[&[u8]],
        streams: Streams,
        group: Group,
    ) -> Result<i32, Errno> {
        assert_eq!(program.last(), Some(&0));
        assert!(arguments.iter().all(|argument| argument.last() == Some(&0)));
        assert!(
            environment
                .iter()
                .all(|variable| variable.last() == Some(&0))
        );
        let program = text(program);
        for descriptor in [streams.input, streams.output] {
            assert!(descriptor < 3 || self.live.contains(&descriptor));
        }
        if let Some(error) = self.run_error {
            return Err(error);
        }
        self.ran.push(Started {
            program,
            arguments: arguments.iter().map(|argument| text(argument)).collect(),
            environment: environment.iter().map(|variable| text(variable)).collect(),
            job_control: group != Group::Shell,
            streams,
            group,
        });
        Ok(99 + self.ran.len() as i32)
    }
}

fn system() -> FakeSystem {
    FakeSystem {
        files: [
            ("/", Kind::Directory),
            ("/bin", Kind::Directory),
            ("/home", Kind::Directory),
            ("/bin/ls", Kind::Executable),
            ("/bin/fail", Kind::Executable),
            ("/bin/notes", Kind::NotExecutable),
            ("/usr/bin/tool", Kind::Executable),
        ]
        .into_iter()
        .map(|(name, kind)| (name.to_owned(), kind))
        .collect(),
        working_directory: "/".to_owned(),
        statuses: [("/bin/fail".to_owned(), 3 << 8)].into_iter().collect(),
        ..FakeSystem::default()
    }
}

const ARGUMENTS: [&[u8]; 3] = [b"script", b"one", b"two words"];

fn shell(interactive: bool) -> Shell<'static, FakeSystem> {
    let mut shell = Shell::new(system(), 42, &ARGUMENTS, 3, interactive, true);
    shell.import(b"HOME=/home");
    shell.import(b"PATH=/bin:/usr/bin");
    shell.import(b"TERM=linux");
    shell.set_working_directory_variable();
    shell
}

fn run(script: &str) -> Shell<'static, FakeSystem> {
    let mut shell = shell(false);
    shell.run_text(script.as_bytes());
    shell
}

fn output(shell: &Shell<FakeSystem>) -> String {
    String::from_utf8(shell.system.output.clone()).unwrap()
}

fn errors(shell: &Shell<FakeSystem>) -> String {
    String::from_utf8(shell.system.errors.clone()).unwrap()
}

#[test]
fn builtins_echo_quote_and_expand() {
    let shell = run("echo 'single  quoted' \"double $HOME\" \\$HOME\n\
         X=abc; Y=\"x y\"; echo $X${X}def \"$Y\" $Y\n\
         echo -n no newline; echo\n\
         echo $0 $1 \"$2\" $# $$\n\
         : ignored; true");
    assert_eq!(
        output(&shell),
        "single  quoted double /home $HOME\nabcabcdef x y x y\nno newline\n\
         script one two words 2 42\n"
    );
    assert_eq!(errors(&shell), "");
    assert_eq!(shell.exit_status(), 0);
}

#[test]
fn the_status_of_each_command_is_in_the_next_one_s_question_mark() {
    let shell = run("false; echo $?; true; echo $?; fail; echo $?; X=1; echo $?\nfalse");
    assert_eq!(output(&shell), "1\n0\n3\n0\n");
    assert_eq!(shell.exit_status(), 1);
}

#[test]
fn programs_are_found_on_path_and_get_their_arguments_and_the_exported_variables() {
    let shell = run("NOT_EXPORTED=1; TERM=vt100 ls -l 'a b'\ntool");
    let ran = &shell.system.ran;
    assert_eq!(ran.len(), 2);
    assert_eq!(ran[0].program, "/bin/ls");
    assert_eq!(ran[0].arguments, ["ls", "-l", "a b"]);
    assert_eq!(
        ran[0].environment,
        ["TERM=vt100", "HOME=/home", "PATH=/bin:/usr/bin", "PWD=/"]
    );
    assert!(!ran[0].job_control);
    assert_eq!(ran[1].program, "/usr/bin/tool");
    assert_eq!(
        ran[1].environment,
        ["HOME=/home", "PATH=/bin:/usr/bin", "PWD=/", "TERM=linux"]
    );
}

#[test]
fn a_missing_command_is_127_and_one_that_cannot_run_126() {
    let shell =
        run("nothing; echo $?; notes; echo $?; /bin/none; echo $?; /bin; echo $?; ./ls; echo $?");
    assert_eq!(output(&shell), "127\n126\n127\n126\n127\n");
    assert_eq!(
        errors(&shell),
        "eggsh: nothing: command not found\n\
         eggsh: notes: Permission denied\n\
         eggsh: /bin/none: No such file or directory\n\
         eggsh: /bin: Is a directory\n\
         eggsh: ./ls: No such file or directory\n"
    );
    assert!(shell.system.ran.is_empty());
}

#[test]
fn a_program_ended_by_a_signal_sets_128_plus_the_signal_and_is_reported() {
    let mut shell = shell(false);
    shell.system.statuses.insert("/bin/ls".to_owned(), 15);
    shell.system.statuses.insert("/usr/bin/tool".to_owned(), 2);
    shell.run_text(b"ls; echo $?; tool; echo $?");
    assert_eq!(output(&shell), "143\n130\n");
    assert_eq!(errors(&shell), "Terminated\n\n");
}

#[test]
fn a_program_that_fails_to_start_is_reported() {
    let mut shell = shell(false);
    shell.system.run_error = Some(Errno::ENOEXEC);
    shell.run_text(b"ls; echo $?");
    assert_eq!(output(&shell), "126\n");
    assert_eq!(errors(&shell), "eggsh: ls: Exec format error\n");
}

#[test]
fn cd_changes_directory_and_keeps_pwd_and_oldpwd() {
    let shell = run(
        "cd /bin; pwd; echo $PWD $OLDPWD\ncd -\ncd; pwd\ncd /missing; echo $?\n\
         cd /bin/ls; echo $?\ncd a b; echo $?",
    );
    assert_eq!(output(&shell), "/bin\n/bin /\n/\n/home\n1\n1\n1\n");
    assert_eq!(
        errors(&shell),
        "eggsh: cd: /missing: No such file or directory\n\
         eggsh: cd: /bin/ls: Not a directory\n\
         eggsh: cd: too many arguments\n"
    );
    let shell = run("unset HOME; cd; pwd; unset OLDPWD; cd -; echo $?");
    assert_eq!(output(&shell), "/\n1\n");
    assert_eq!(errors(&shell), "eggsh: cd: OLDPWD not set\n");
}

#[test]
fn exit_ends_the_shell_with_its_status() {
    let shell = run("echo before; exit 7; echo after\necho later");
    assert_eq!(output(&shell), "before\n");
    assert_eq!(shell.exit_status(), 7);
    assert_eq!(run("false; exit").exit_status(), 1);
    let shell = run("exit nope");
    assert_eq!(shell.exit_status(), 2);
    assert_eq!(
        errors(&shell),
        "eggsh: exit: nope: numeric argument required\n"
    );
    let shell = run("exit 1 2; echo still here");
    assert_eq!(output(&shell), "still here\n");
}

#[test]
fn export_unset_and_set_manage_variables() {
    let shell = run(
        "A=1; export A B=\"it's\" C; unset TERM PWD HOME PATH; export\n\
         set\nunset A; echo \"[$A]\"; export 1x; echo $?; unset -x; echo $?",
    );
    assert_eq!(
        output(&shell),
        "export A='1'\nexport B='it'\\''s'\nexport C\n\
         A='1'\nB='it'\\''s'\n[]\n1\n1\n"
    );
    assert_eq!(
        errors(&shell),
        "eggsh: export: `1x': not a valid identifier\n\
         eggsh: unset: `-x': not a valid identifier\n"
    );
    let shell = run("C=late; export C; ls");
    assert!(
        shell.system.ran[0]
            .environment
            .contains(&"C=late".to_owned())
    );
}

#[test]
fn type_says_what_a_name_runs() {
    let shell = run("type cd ls nothing; echo $?");
    assert_eq!(output(&shell), "cd is a shell builtin\nls is /bin/ls\n1\n");
    assert_eq!(errors(&shell), "eggsh: type: nothing: not found\n");
}

#[test]
fn unsupported_operators_reject_the_whole_line_and_end_a_script() {
    let shell = run("echo first\necho ran; ls || wc\necho never");
    assert_eq!(output(&shell), "first\n");
    assert_eq!(errors(&shell), "eggsh: `||` is not supported yet\n");
    assert_eq!(shell.exit_status(), 2);
    let mut interactive = shell_with_input(true, &["echo a << f\n", "echo next\n"]);
    interactive.run_descriptor(0, true);
    assert_eq!(output(&interactive), "next\n");
    assert!(errors(&interactive).contains("eggsh: `<<` is not supported yet\n"));
    assert!(interactive.system.ran.is_empty());
}

fn all_closed(shell: &Shell<FakeSystem>) {
    assert!(
        shell.system.live.is_empty(),
        "left open: {:?}",
        shell.system.live
    );
}

#[test]
fn a_pipeline_s_programs_start_together_piped_and_its_status_is_the_last_one_s() {
    let mut shell = shell(false);
    shell.run_text(b"ls / | fail | tool; echo $?; tool | fail; echo $?");
    let ran = &shell.system.ran;
    assert_eq!(ran.len(), 5);
    let pipes = &shell.system.pipes;
    assert_eq!(
        ran[0].streams,
        Streams {
            input: 0,
            output: pipes[0].1
        }
    );
    assert_eq!(
        ran[1].streams,
        Streams {
            input: pipes[0].0,
            output: pipes[1].1
        }
    );
    assert_eq!(
        ran[2].streams,
        Streams {
            input: pipes[1].0,
            output: 1
        }
    );
    assert_eq!(ran[2].program, "/usr/bin/tool");
    assert!(ran.iter().all(|ran| ran.group == Group::Shell));
    assert_eq!(shell.system.waited, [100, 101, 102, 103, 104]);
    assert_eq!(output(&shell), "0\n3\n");
    assert_eq!(shell.system.terminal_taken_back, 0);
    all_closed(&shell);
}

#[test]
fn an_interactive_pipeline_is_one_process_group_led_by_its_first_program() {
    let mut shell = shell_with_input(true, &["echo hi | ls | tool\n", "ls\n"]);
    shell.run_descriptor(0, true);
    let groups: Vec<Group> = shell.system.ran.iter().map(|ran| ran.group).collect();
    assert_eq!(groups, [Group::New, Group::Join(100), Group::New]);
    assert_eq!(shell.system.terminal_taken_back, 2);
    assert_eq!(
        shell.system.written.get(&shell.system.pipes[0].1).unwrap(),
        b"hi\n"
    );
    all_closed(&shell);
}

#[test]
fn builtins_in_a_pipeline_write_into_it_and_missing_commands_close_their_ends() {
    let mut shell = shell(false);
    shell.run_text(b"echo a b | ls; nothing | ls; echo $?; ls | nothing; echo $?");
    let pipes = shell.system.pipes.clone();
    assert_eq!(shell.system.written.get(&pipes[0].1).unwrap(), b"a b\n");
    assert_eq!(shell.system.ran[0].streams.input, pipes[0].0);
    assert_eq!(output(&shell), "0\n127\n");
    assert_eq!(
        errors(&shell),
        "eggsh: nothing: command not found\n".repeat(2)
    );
    assert_eq!(shell.system.waited, [100, 101, 102]);
    all_closed(&shell);
}

#[test]
fn redirections_open_their_files_for_builtins_and_programs() {
    let mut shell = shell(false);
    shell
        .system
        .contents
        .insert("/in".to_owned(), b"text".to_vec());
    shell.run_text(b"echo a > /tmp/x; echo b >> /tmp/x; ls < /in > /out | tool; cat < /missing; echo $?; echo c > /readonly/f; echo $?");
    let redirected = &shell.system.redirected;
    assert_eq!(redirected[0].0, "/tmp/x");
    assert_eq!(redirected[0].1, Redirection::Output);
    assert_eq!(redirected[1].1, Redirection::Append);
    assert_eq!(shell.system.written.get(&redirected[0].2).unwrap(), b"a\n");
    assert_eq!(shell.system.written.get(&redirected[1].2).unwrap(), b"b\n");
    let ls = &shell.system.ran[0];
    assert_eq!(
        ls.streams,
        Streams {
            input: redirected[2].2,
            output: redirected[3].2
        }
    );
    assert_eq!(shell.system.ran[1].streams.input, shell.system.pipes[0].0);
    assert_eq!(output(&shell), "1\n1\n");
    assert_eq!(
        errors(&shell),
        "eggsh: /missing: No such file or directory\neggsh: /readonly/f: Read-only file system\n"
    );
    all_closed(&shell);
}

#[test]
fn syntax_errors_are_reported() {
    for (script, message) in [
        (
            "echo 'open",
            "syntax error: unexpected end of file, looking for the closing `'`",
        ),
        (
            "echo \"open",
            "syntax error: unexpected end of file, looking for the closing `\"`",
        ),
        (
            "echo ${open",
            "syntax error: unexpected end of file, looking for the closing `}`",
        ),
        ("; echo", "syntax error near unexpected token `;`"),
        ("echo ${A:-b}", "bad substitution"),
    ] {
        let shell = run(script);
        assert_eq!(errors(&shell), format!("eggsh: {message}\n"), "{script}");
        assert_eq!(shell.exit_status(), USAGE);
        assert_eq!(output(&shell), "");
    }
}

fn shell_with_input(interactive: bool, lines: &[&str]) -> Shell<'static, FakeSystem> {
    let mut shell = shell(interactive);
    let input = lines.concat().into_bytes().into_iter().collect();
    shell.system.open.insert(0, input);
    shell
}

#[test]
fn an_interactive_shell_prompts_and_ends_at_the_end_of_input() {
    let mut shell = shell_with_input(true, &["echo 'a\n", "b'; cd /bin\n", "pwd\n"]);
    shell.run_descriptor(0, true);
    assert_eq!(output(&shell), "a\nb\n/bin\n");
    assert!(errors(&shell).starts_with("eggsh:/# "));
    assert!(errors(&shell).contains("eggsh:/bin# "));
    assert!(errors(&shell).ends_with("eggsh:/bin# exit\n"));
}

#[test]
fn an_open_quote_asks_for_more_with_the_continuation_prompt() {
    let mut shell = shell_with_input(true, &["echo \"x\n", "y\"\n"]);
    shell.run_descriptor(0, true);
    assert_eq!(output(&shell), "x\ny\n");
    assert_eq!(errors(&shell), "eggsh:/# > eggsh:/# exit\n");
}

#[test]
fn an_interrupt_at_the_prompt_gives_a_fresh_one_and_sets_130() {
    let mut shell = shell(true);
    shell.system.input = [Read::Interrupted, Read::End].into_iter().collect();
    shell.run_descriptor(0, true);
    assert_eq!(errors(&shell), "eggsh:/# \neggsh:/# exit\n");
    assert_eq!(shell.exit_status(), INTERRUPTED);
}

#[test]
fn programs_run_under_job_control_when_interactive() {
    let mut shell = shell_with_input(true, &["ls\n"]);
    shell.run_descriptor(0, true);
    assert!(shell.system.ran[0].job_control);
}

#[test]
fn the_prompt_follows_ps1_and_the_user() {
    let mut shell = shell_with_input(true, &["PS1='> $ '\n"]);
    shell.run_descriptor(0, true);
    assert_eq!(errors(&shell), "eggsh:/# > $ exit\n");
    let mut user = Shell::new(system(), 1, &ARGUMENTS, 3, true, false);
    user.set_working_directory_variable();
    user.run_descriptor(0, true);
    assert_eq!(errors(&user), "eggsh:/$ exit\n");
}

#[test]
fn a_line_too_long_is_refused_and_the_shell_goes_on() {
    let long = format!("echo {}\n", "x".repeat(LINE_CAPACITY));
    let mut shell = shell_with_input(true, &[&long, "echo ok\n"]);
    shell.run_descriptor(0, true);
    assert_eq!(output(&shell), "ok\n");
    assert!(errors(&shell).contains("eggsh: line too long\n"));
}

#[test]
fn source_runs_a_file_in_the_shell_itself() {
    let mut shell = shell(false);
    shell.system.contents.insert(
        "/script".to_owned(),
        b"# comment\nSOURCED=yes\necho in $0\nfalse\n".to_vec(),
    );
    shell
        .system
        .contents
        .insert("/loop".to_owned(), b". /loop\n".to_vec());
    shell.run_text(b". /script; echo $? $SOURCED; source /missing; echo $?; . ; echo $?");
    assert_eq!(output(&shell), "in script\n1 yes\n1\n2\n");
    assert_eq!(
        errors(&shell),
        "eggsh: .: /missing: No such file or directory\neggsh: .: usage: . file\n"
    );
    assert!(shell.system.open.is_empty());
    shell.system.errors.clear();
    shell.run_text(b". /loop");
    assert_eq!(errors(&shell), "eggsh: .: too deeply nested\n");
    assert!(shell.system.open.is_empty());
}

#[test]
fn a_file_read_in_pieces_runs_each_command_once() {
    let mut shell = shell(false);
    shell.system.contents.insert(
        "/file".to_owned(),
        b"echo one two three four\necho 'five\nsix'; echo seven\necho last".to_vec(),
    );
    let descriptor = shell.system.open(b"/file\0").unwrap();
    shell.run_descriptor(descriptor, false);
    assert_eq!(
        output(&shell),
        "one two three four\nfive\nsix\nseven\nlast\n"
    );
}

#[test]
fn help_lists_the_builtins() {
    let shell = run("help");
    assert!(output(&shell).contains("Builtins: . (source) : cd echo exit"));
}

#[test]
fn cd_from_a_working_directory_longer_than_a_path_keeps_the_shell_alive() {
    let x = "x".repeat(1000);
    let shell = run(&format!(
        "A={x}; PWD=$A$A$A$A$A; cd /; echo $? \"[$OLDPWD]\" $PWD"
    ));
    assert_eq!(output(&shell), "1 [] /\n");
    assert_eq!(errors(&shell), "eggsh: cd: too many variables\n");
    let mut imported = Shell::new(system(), 1, &ARGUMENTS, 3, false, true);
    imported.import(format!("PWD=/{}", "p".repeat(PATH_CAPACITY + 10)).as_bytes());
    imported.run_text(b"cd /bin; echo $? $PWD \"[$OLDPWD]\"");
    assert_eq!(output(&imported), "1 /bin []\n");
}

fn long_quoted_line() -> String {
    format!("echo 'start\n{}\nls\n'\necho after\n", "y".repeat(5000))
}

#[test]
fn a_line_too_long_in_a_file_ends_it_without_running_what_its_quote_holds() {
    let mut shell = shell(false);
    shell
        .system
        .contents
        .insert("/long".to_owned(), long_quoted_line().into_bytes());
    shell.run_text(b". /long\necho not reached");
    assert!(shell.system.ran.is_empty());
    assert_eq!(output(&shell), "");
    assert_eq!(errors(&shell), "eggsh: line too long\n");
    assert_eq!(shell.exit_status(), USAGE);
    let mut interactive = shell_with_input(true, &[". /long\n", "echo $?\n"]);
    interactive
        .system
        .contents
        .insert("/long".to_owned(), long_quoted_line().into_bytes());
    interactive.run_descriptor(0, true);
    assert!(interactive.system.ran.is_empty());
    assert_eq!(output(&interactive), "2\n");
}

#[test]
fn a_line_too_long_at_the_prompt_is_skipped_up_to_the_end_of_its_quote() {
    let mut shell = shell_with_input(true, &[&long_quoted_line()]);
    shell.run_descriptor(0, true);
    assert!(shell.system.ran.is_empty());
    assert_eq!(output(&shell), "after\n");
    assert!(errors(&shell).contains("eggsh: line too long\n"));
}

#[test]
fn a_refused_line_is_skipped_up_to_the_end_of_its_quote() {
    let mut shell = shell_with_input(true, &["echo | 'a\n", "ls\n", "'\n", "echo after\n"]);
    shell.run_descriptor(0, true);
    assert!(shell.system.ran.is_empty());
    assert_eq!(output(&shell), "after\n");
}

#[test]
fn an_interactive_shell_goes_home_and_writes_the_welcome_before_its_first_prompt() {
    let welcome = b"Welcome to EggOS. Type help. Apps: the egg, or Super.\n";
    let mut interactive = shell(true);
    interactive
        .system
        .contents
        .insert("/etc/motd".to_owned(), welcome.to_vec());
    interactive.greet();
    assert_eq!(interactive.system.working_directory, "/home");
    assert_eq!(output(&interactive).as_bytes(), welcome);
    let mut script = shell(false);
    script
        .system
        .contents
        .insert("/etc/motd".to_owned(), welcome.to_vec());
    script.greet();
    assert_eq!(script.system.working_directory, "/");
    assert_eq!(output(&script), "");
}
