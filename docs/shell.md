# eggsh, the EggOS shell

`eggsh` (`userland/eggsh`, D107) is the shell init starts at boot (`/bin/eggsh`). It is our own
Rust program on our own runtime: no C library, no allocator, about 50 KiB.

## Using it

- `eggsh` on a terminal is interactive: it writes the prompt `eggsh:<working directory># `
  (`$ ` for a user other than root; `PS1` replaces it, `PS2` the `> ` of a continued line),
  reads a line, and runs its commands. `^D` at an empty prompt ends it (`exit`), `^C` gives a
  fresh prompt and sets `$?` to 130.
- `eggsh file [arguments]` runs the commands of a file, `$0` the file and `$1`... the arguments.
- `eggsh -c 'commands' [name [arguments]]` runs `commands`, `$0` being `name`.
- The shell exits with `exit`'s status, or the last command's.

## The language

- Words are separated by blanks; a command ends at a newline or `;`.
- `'...'` keeps everything as it is. `"..."` keeps everything but `$` expansions, and a `\`
  before `$`, `` ` ``, `"`, `\` or a newline. Outside quotes `\` keeps the next character, and
  `\` before a newline continues the line. A quote left open continues on the next line.
- `#` at the start of a word comments to the end of the line.
- `$NAME` and `${NAME}` are variables; `$?` the last status, `$$` the shell's process id, `$#`
  the number of arguments, `$0` to `$9` the arguments. Unquoted, an expansion's value is split
  into words at blanks. `~` alone or before `/` at the start of a word is `$HOME`.
- `NAME=value` alone sets a shell variable (exported if it already was); `export NAME[=value]`
  exports it, `unset NAME` removes it. `NAME=value command` gives `command` the variable in
  its environment only (builtins ignore such prefixes).
- A line longer than 4 KiB is refused: in a file it ends the file with status 2; at the prompt
  the shell skips it up to its end, an open quote included.
- A command is a builtin, or a program: a name with a `/` is run as it is, any other is looked
  for in each directory of `PATH` (`/bin:/usr/bin:/sbin` if unset).
- `$?` is the program's exit status, 128 plus the signal that ended it (the shell writes the
  signal's description, such as `Terminated`, but only a newline for `^C`), 127 for a command
  not found, 126 for one that cannot be run, 2 after a syntax error.

## Builtins

`.` and `source` (run a file in this shell), `:` and `true`, `false`, `cd` (`cd` alone goes to
`$HOME`, `cd -` back to `$OLDPWD`; keeps `PWD` and `OLDPWD`), `echo` (`-n`: no newline),
`exit [n]`, `export`, `help`, `pwd`, `set` (lists the variables), `type`, `unset`.

## Job control

Interactive, the shell runs each program in a process group of its own and hands it the
terminal while it runs, so `^C` and `^\` reach only that program; it takes the terminal back
when the program ends. The shell ignores `SIGQUIT`, `SIGTSTP`, `SIGTTIN` and `SIGTTOU`, and
catches `SIGINT` only at its prompt; programs start with all of them back to their defaults.
Run from a file or with `-c`, the shell leaves signals and groups as they are.

## Not supported yet

Pipes `|`, redirections `<`, `>`, `>>`, `<<`, `&&`, `||`, background jobs `&`, subshells
`( )`, command substitution `` `...` `` and `$(...)` are refused with
`eggsh: `|` is not supported yet` (never read as plain words, so adding them later changes no
command that works now). Also not yet: wildcards, `${NAME:-default}` and the other parameter
operators, `$@` and `$*`, `if`, `while`, `for`, `case` and functions, stopping and resuming
jobs, line editing beyond the terminal's own, and history.
