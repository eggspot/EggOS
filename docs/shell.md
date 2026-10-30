# eggsh, the EggOS shell

`eggsh` (`userland/eggsh`, D107) is the shell init starts at boot (`/bin/eggsh`). It is our own
Rust program on our own runtime: no C library, no allocator, about 50 KiB.

The shell is EggOS's command line: terminals are its window on the desktop, and an agent
reaches it through the MCP tool `run` with `/bin/eggsh` and `-c` and a command line, which
gets every feature below that does not need a terminal (`&&`, `||`, `&`, `2>`, `wait`,
`kill`, and the whole language of scripts: loops, functions, `$(...)`).

## Using it

- `eggsh` on a terminal is interactive: it starts in `$HOME` (`/home/root`, which init makes)
  and writes `/etc/motd`, then the prompt `eggsh:<working directory># `
  (`$ ` for a user other than root; `PS1` replaces it, `PS2` the `> ` of a continued line),
  reads a line, and runs its commands (D156). `^D` at an empty prompt ends it (`exit`), `^C` gives a
  fresh prompt and sets `$?` to 130.
- At the prompt the shell edits the line itself (D125, D181). Typed characters go in at the
  cursor. The left and right arrows (`^B`, `^F`) move the cursor a character, Home and End
  (`^A`, `^E`) to the start and the end of the line; delete or backspace erases the character
  before the cursor, the Delete key (and `^D` on a line that is not empty) the one under it,
  `^U` everything before the cursor, `^K` everything from it, `^W` the word before it. The up
  and down arrows go back and forth through the last 64 lines typed (8 KiB at most; an empty
  line, one repeating the last and one that starts with a space are not kept).
- The history is kept across sessions (D181): each line is added to the file `HISTFILE` names
  as it is typed, `$HOME/.eggsh_history` unless `HISTFILE` is set (empty, no file is kept), and
  a new interactive shell starts with the lines it holds. The file grows until a shell finds it
  larger than 64 KiB, which writes it again with the lines it keeps. `/home` survives a reboot
  when EggOS has its disk (docs/eggfs.md), and the history with it. A script or `eggsh -c`
  neither reads nor writes it. A line that starts with a space is kept out of it, so a secret
  typed on a command line can be.
- `eggsh file [arguments]` runs the commands of a file, `$0` the file and `$1`... the arguments.
- `eggsh -c [--] 'commands' [name [arguments]]` runs `commands`, `$0` being `name` (`--`, which
  ends the options as in `sh -c -- commands`, is how bionic's `system()` calls the shell).
- The shell exits with `exit`'s status, or the last command's. Interactive, with stopped jobs,
  the first `exit` (or `^D`) says `There are stopped jobs.` and the shell goes on; a second one
  right after leaves, and the stopped jobs get `SIGHUP` and `SIGCONT` from the kernel (D181).

## The language

- Words are separated by blanks; a command ends at a newline or `;`.
- `a | b | c` is a pipeline (D125): its commands run at the same time, each one's standard
  output piped into the next one's standard input, and the shell waits for all of them; `$?`
  is the last one's status. Interactive, the pipeline is one process group, led by its first
  program, that has the terminal while it runs, so `^C` reaches all of it. A `|` needs a
  command on each side, on the same line.
- `a && b` runs `b` only if `a` succeeded (status 0), `a || b` only if it failed (D181); they
  chain from the left with the same weight, so `a && b || c` runs `c` if `a` or `b` failed,
  and a pipeline that does not run leaves `$?` as it was. The command after `&&`, `||` or `|`
  may be on the next line (D182); at the prompt the shell asks for it with `PS2`.
- `! pipeline` runs the pipeline and inverts its status (0 becomes 1, anything else 0).
- `a &` starts the pipeline `a` in the background as a job and goes on at once (D181); `$?` is
  0 and `$!` the process id of its last program. Interactive, the shell writes `[1] 1234` (the
  job's number and that process id), the job runs in a process group of its own without the
  terminal, and the shell says before a later prompt when it ended, as bash does
  (`[1]+  Done                    sleep 3`, `Exit 3`, `Terminated`). In a script or with `-c`
  the job stays in the shell's group and reads `/dev/null` unless its input is redirected, as
  POSIX asks. A list joined by `&&` or `||` followed by `&` runs in the background as one job,
  in a copy of the shell (D182).
- `< file` reads standard input from `file`, `> file` writes standard output to `file`
  (created if missing, emptied if not), `>> file` adds to its end (created if missing). A digit
  right before the operator names another descriptor: `2> file` sends standard error to a file
  (D181). `2>&1` makes standard error go where standard output goes at that point, `>&2` the
  other way; so `cmd > file 2>&1` writes both to `file`, and `cmd 2>&1 > file` only the
  output. Redirections can come anywhere in a command, are done from left to right, and apply
  to builtins (their output and their error messages) as well as programs; the word after the
  operator, expanded as any word, is the file. A file that cannot be opened is reported
  (`eggsh: /etc/motd: Read-only file system`) and the command does not run, with status 1.
  Only descriptors 0, 1 and 2 can be redirected yet. Writable files are under `/tmp` and
  `/home` (docs/coreutils.md). `>| file` is `> file`.
- `<<word` is a here-document (D182): the command reads the lines that follow the command's
  line, up to a line that is `word` alone. Unless any of `word` is quoted (`<<'EOF'`,
  `<<\EOF`), `$` expansions and `` `...` `` in the lines are expanded and `\` keeps `$`, `` ` ``
  and `\`. `<<-word` removes the tabs at the start of each line and of the delimiter's line.
  Several here-documents on one line take their lines in turn; at the prompt the shell asks for
  them with `PS2`. The lines reach the command through an in-memory file (`memfd_create`), so
  a program can seek in it.
- `'...'` keeps everything as it is. `"..."` keeps everything but `$` expansions, and a `\`
  before `$`, `` ` ``, `"`, `\` or a newline. Outside quotes `\` keeps the next character, and
  `\` before a newline continues the line. A quote left open continues on the next line.
- `#` at the start of a word comments to the end of the line.
- `$NAME` and `${NAME}` are variables; `$?` the last status, `$$` the shell's process id, `$!`
  the last background job's last process id, `$#` the number of arguments, `$0` the script's
  name, `$1` to `$9` and `${10}` and on the arguments (D182). `"$@"` is every argument, each a
  word of its own (none at all without arguments), `"$*"` all of them in one word, separated
  by `IFS`'s first character; unquoted, both are every argument split into words. `~` alone or
  before `/` at the start of a word is `$HOME`.
- The parameter operators (D182): `${NAME:-word}` is `word` if `NAME` is unset or empty,
  `${NAME:=word}` also sets `NAME` to it, `${NAME:?word}` writes `NAME: word` (or
  `parameter null or not set`) and stops the command (a script ends, with status 1),
  `${NAME:+word}` is `word` if `NAME` is set and not empty; without the `:`, only an unset
  `NAME` counts. `${#NAME}` is the value's length, `${NAME#pattern}` and `${NAME##pattern}` the
  value without the shortest or longest start that matches `pattern`, `${NAME%pattern}` and
  `${NAME%%pattern}` without the shortest or longest end. `word` is expanded only when used.
- `$(commands)` and `` `commands` `` are the output of the commands, run in a copy of the shell
  (a subshell), its last newlines removed (D182); `$?` after a command that is only
  assignments is the substitution's status. A `case` inside `$(...)` writes its patterns with
  their leading `(`.
- Unquoted, the results of `$` expansions and of `$(...)` are split into words at the
  characters of `IFS` (a space, a tab and a newline unless it is set; empty, nothing is split):
  blanks in `IFS` run together, each other character of `IFS` ends a word, so `a::b` with
  `IFS=:` is `a`, an empty word and `b` (D182).
- Wildcards (D182): a word with an unquoted `*` (any text), `?` (one character) or `[...]`
  (one character of a set: `[abc]`, `[a-z]`, `[!x]` or `[^x]` for any other,
  `[[:alpha:]]` and the other POSIX classes) becomes the paths it matches, sorted by their
  bytes, or stays as it is if nothing matches. A name starting with `.` needs a `.` written in
  the pattern, `.` and `..` never match, and `/` is never matched by a wildcard. Assignments,
  redirections and `case` words are not expanded this way. A word that matches more paths than
  the shell holds (256, 8 KiB) is an error, never a list cut short.
- `NAME=value` alone sets a shell variable (exported if it already was); `export NAME[=value]`
  exports it, `unset NAME` removes it. `NAME=value command` gives `command` the variable in
  its environment only (builtins ignore such prefixes).
- Compound commands (D182), each of which may be followed by redirections for all of it
  (`done < file`, `} > file`):
  - `{ list; }` runs the list in the shell; `( list )` runs it in a copy of the shell, so
    `cd`, variables and `exit` there do not change the shell.
  - `if list; then list; elif list; then list; else list; fi` runs the first branch whose
    condition succeeds; its status is that branch's, 0 if none ran.
  - `while list; do list; done` and `until list; do list; done` run the body while the
    condition succeeds (or until it does); `for name in words; do list; done` runs the body
    once for each word, expanded and matched against file names, with `name` set to it
    (`for name; do` takes the arguments). `break [n]` leaves the loop (or `n` loops),
    `continue [n]` starts its next round.
  - `case word in pattern | pattern) list ;; (pattern) list ;; esac` runs the list of the
    first item one of whose patterns matches the word (patterns as wildcards, quoted
    characters plain); status 0 if none matched.
  - `name() compound-command` defines a function: `name args` runs the command with the args
    as `$1`, `$2`... (and `$#`, `$@`), in the shell itself; `return [n]` ends it, `local NAME`
    keeps a variable's value for the function only. Functions come after the special builtins
    and before the other builtins and programs; at most 64 run inside each other, and their
    definitions share 16 KiB.
  - A compound command or a function call that is one of several commands of a pipeline, or a
    pipeline run with `&`, runs in a copy of the shell, so `cmd | while read line; do ...;
    done` reads at the same time as `cmd` writes (as in dash and bash; the loop's variables are
    gone after it).
- Words such as `if`, `then`, `do`, `done`, `case`, `esac`, `{`, `}` and `!` are reserved only
  as the first word of a command, unquoted: `echo if` writes `if`.
- `set -e` ends the shell when a command fails, except in an `if`, `while` or `until`
  condition, a command before `&&` or `||`, or one after `!`; `set +e` stops that.
- A complete command (a line, or a whole `if`, loop or function definition with its
  here-documents) longer than 16 KiB is refused: in a file it ends the file with status 2; at
  the prompt the shell skips it up to its end, an open quote included.
- A command is a special builtin, a function, a builtin, or a program: a name with a `/` is
  run as it is, any other is looked for in each directory of `PATH` (`/bin:/usr/bin:/sbin` if
  unset).
- `$?` is the program's exit status, 128 plus the signal that ended or stopped it (the shell
  writes the signal's description, such as `Terminated`, but only a newline for `^C`), 127 for
  a command not found, 126 for one that cannot be run, 2 after a syntax error.

## Builtins

`.` and `source` (run a file in this shell; `return` ends it), `:` and `true`, `false`, `cd`
(`cd` alone goes to `$HOME`, `cd -` back to `$OLDPWD`; keeps `PWD` and `OLDPWD`), `echo` (`-n`:
no newline), `exit [n]`, `export`, `help`, `pwd`, `type` (a function, a builtin or a program),
`unset` (`-f` for a function), the job builtins below, and those of the language (D182):

- `test expression` and `[ expression ]` (with `]` last): status 0 if the expression is true, 1
  if false, 2 if it cannot be read. Files: `-e` exists, `-f` regular file, `-d` directory, `-h`
  and `-L` symbolic link, `-b`, `-c`, `-p`, `-S` devices, pipes and sockets, `-s` not empty,
  `-r`, `-w`, `-x` readable, writable, executable, `-u`, `-g`, `-k` the mode's bits, `-t n` a
  terminal. Strings: `-n`, `-z`, `=` (or `==`), `!=`, `<`, `>`. Numbers: `-eq`, `-ne`, `-lt`,
  `-le`, `-gt`, `-ge`. `!`, `-a` (and), `-o` (or) and `( )`; up to four words are read by
  their count as POSIX says, so `[ "$x" = "!" ]` compares two words.
- `read [-r] [name ...]` reads a line of standard input and splits it at the characters of
  `IFS` into the variables, the last taking the rest of the line (`REPLY` without names);
  without `-r`, `\` keeps the next character and joins lines; status 1 at the end of the input.
- `break [n]`, `continue [n]`, `return [n]`, `shift [n]`, `local name[=value] ...`, and `eval
  words` (runs the words, joined by spaces, as commands).
- `set` lists the variables; `set -e` and `set +e` turn the end on failure on and off, and
  `set [--] words` makes the words the arguments `$1`, `$2`...

Other commands are programs found through `PATH` (`/bin` under init), such as the core
utilities `ls`, `cat`, `grep`, `cp` and `poweroff` (docs/coreutils.md); `/bin/echo`,
`/bin/true` and `/bin/false` run the programs instead of the builtins.

A builtin in a pipeline runs in the shell itself, its output written into the pipe, so `cd`,
`export` and `exit` there act on the shell, where other shells run them in a subshell and
their effect is lost; a builtin in a background pipeline runs at once, in the shell. The shell
ignores `SIGPIPE` for this and gives programs the default back, so a program it starts dies of
a broken pipe as usual.

## Job control

Interactive, the shell runs each program in a process group of its own and hands it the
terminal while it runs, so `^C` and `^\` reach only that program; it takes the terminal back
when the program ends. The shell ignores `SIGQUIT`, `SIGTSTP`, `SIGTTIN` and `SIGTTOU`, and
catches `SIGINT` only at its prompt; programs start with all of them back to their defaults.
Run from a file or with `-c`, the shell leaves signals and groups as they are.

Jobs (D181), as bash has them:

- `^Z` stops the program running in the foreground (the terminal sends it `SIGTSTP`); the
  shell writes `[1]+  Stopped                 sleep 30` and its prompt, and the program is now
  job 1. `$?` is 148 (128 plus `SIGTSTP`).
- `jobs` lists the jobs, one line each: `[1]+  Stopped                 sleep 30`,
  `[2]-  Running                 make &`; `+` marks the current job (the one stopped or
  started last), `-` the previous one. `jobs -l` adds each job's process id, `jobs -p` writes
  only the ids. A job that is done is listed once, then forgotten.
- `fg [job]` brings a job to the foreground: the shell writes its command line, gives it the
  terminal, continues it if it is stopped (`SIGCONT`) and waits for it as for a command typed
  at the prompt, which `^Z` can stop again.
- `bg [job]` continues a stopped job in the background and writes `[1]+ sleep 30 &`.
- `wait [pid | %job ...]` waits for every job, or for each one named, and has the status of the
  last; 127 for a process that is not the shell's child. It works in scripts too.
- `kill [-s signal | -signal] pid | %job ...` sends a signal, `TERM` unless named (by name, with
  or without `SIG`, in any case, or by number), to each process, to a process group (`-group`),
  or to a job's process group; a stopped job is continued too so it takes the signal, unless
  the signal is `KILL` or `CONT`. `kill -l` lists the signals' names.
- A job is named by `%n` or `n` (its number), `%%`, `%+` or `%` (the current one), `%-` (the
  previous one), or `%name` (the newest whose command line starts with `name`); `fg` and `bg`
  alone take the current one. `fg` and `bg` need an interactive shell
  (`eggsh: fg: no job control`).
- A job in the background that reads the terminal is stopped (`SIGTTIN`, reported as
  `Stopped (tty input)`), and one that changes the terminal's settings is too (`SIGTTOU`,
  `Stopped (tty output)`); `fg` lets it go on. Writing to the terminal from the background is
  allowed. A program in the foreground is told of a change of the terminal's size
  (`SIGWINCH`).
- The shell keeps at most 16 jobs; a job that is done makes room for a new one.

## Not supported yet

Arithmetic (`$((...))`), `<<<`, `&>` and `|&` are refused with `eggsh: `$((` is not supported
yet` (never read as plain words, so adding them later changes no command that works now).
Also not yet: `printf`, `trap`, `getopts`, `set -u`, `set -x` and the other options, `command`,
`exec`, aliases, `~user`, a here-document inside `$(...)` or after a function definition's
line, a `case` pattern without its leading `(` inside `$(...)`, redirections of descriptors
past 2 and `>&-`, `^C` interrupting `wait` or `read`, and stopping a job that writes to the
terminal (`stty tostop`).
