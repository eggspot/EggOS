# The EggOS core utilities

`userland/coreutils` (D108) is one program, `/bin/coreutils`, with the utilities as hard links
to it in `/bin`: `cat`, `cp`, `echo`, `false`, `free`, `grep`, `halt`, `head`, `ls`, `mkdir`,
`mv`, `poweroff`, `ps`, `reboot`, `rm`, `sleep`, `tail`, `touch`, `true`, `uname`, `uptime` and
`wc` (D108, D125). It runs the utility named by the last component of the name it was started by, so
`ls`, `/bin/ls` and a link called `ls` anywhere are all `ls`. Started as `coreutils`, it runs
the utility its first argument names (`coreutils ls /`); `coreutils --list` lists them. It is
our own Rust program on our own runtime, like the shell: no C library, no allocator, about
45 KiB, and every name costs only a directory entry in the initramfs.

The shell's builtins (`echo`, `true`, `false`, docs/shell.md) still run first; the programs are
what `PATH` finds for everything else, and what other programs start (`/bin/echo`).

## The utilities

Options can come before or after operands, and `--` ends them, as GNU's do. An error is
written to standard error as `utility: what: why`, the way GNU's are worded, and sets the exit
status; a bad option is `invalid option -- 'x'` (or `unrecognized option '--x'`).

| Utility | What it does | Exit status |
|---------|--------------|-------------|
| `cat [-u] [file...]` | Writes each file, `-` or no file meaning standard input, in order; a file it cannot open or read is reported and the rest still written. `-u` is accepted (output is never held back). | 0, or 1 if any file failed |
| `echo [-n] [word...]` | Writes the words separated by spaces and a newline; `-n` as the first word leaves the newline out. No escapes, the same as the shell's builtin. | 0 |
| `false`, `true` | Nothing; arguments are ignored. | 1, 0 |
| `ls [-aAdl1] [path...]` | Lists each directory's entries, sorted by name (byte order), one per line, without names starting with `.` unless `-a` (all, `.` and `..` included) or `-A` (all but those two). Operands that are not directories, or all of them with `-d`, are listed first, then each directory under a `path:` header when there are several operands. `-l` is the long format: type and permissions, links, owner and group ids, size, modification time as `YYYY-MM-DD HH:MM` in UTC, name, after a `total` of 1 KiB blocks. `-1` is accepted (one per line is the only layout). With no operand, `.`. | 0; 1 if an entry could not be examined or a directory has more than 1024 entries or 32 KiB of names (the listing is cut short and says so); 2 if an operand could not be reached or a directory not read, or for a bad option |
| `sleep interval...` | Sleeps for the sum of the intervals: a number of seconds with an optional fraction (`0.5`, `.25`, `2.`) and an optional unit, `s`, `m`, `h` or `d`. Every interval is checked before it sleeps. | 0, or 1 for a missing or bad interval |
| `uname [-asnrvmpio]` | Writes the system's names: `-s` the kernel's (the default), `-n` the host name, `-r` the kernel release, `-v` its version, `-m` the machine, `-p` the processor and `-i` the hardware platform (both `unknown`), `-o` the operating system (`EggOS`), in that order whatever the order of the options; `-a` all but the unknown ones. The long options GNU has (`--all`, `--kernel-name`, ...) work too. | 0, or 1 for a bad option or an operand |
| `cp file target`, `cp file... directory` | Copies each file to `target`, or into `directory` under its own name, with the source's permissions; an existing target is overwritten. Directories are refused (`-r not specified; omitting directory`). | 0, or 1 if any copy failed |
| `mv file target`, `mv file... directory` | Renames each file or directory to `target` or into `directory`; across file systems (`/tmp` to `/home`) a file is copied, then removed. `-f` is accepted. | 0, or 1 if any move failed |
| `rm [-rRf] path...` | Removes each file; with `-r` (or `-R`) directories and everything in them; `-f` ignores missing files and no operands. `.` and `..` are refused. | 0, or 1 if anything could not be removed |
| `mkdir [-p] directory...` | Makes each directory; `-p` makes its missing parents too and accepts existing directories. | 0, or 1 if any failed |
| `touch [-c] file...` | Sets each file's modification time to now, creating an empty file for a missing one unless `-c`. | 0, or 1 if any failed |
| `grep [-icnqvFG] pattern [file...]` | Writes the lines that match `pattern`, a POSIX basic regular expression subset (`^`, `$`, `.`, `*`, `[...]` with ranges and `[^...]`, `\` before a character), or a fixed string with `-F`; `-i` ignores ASCII case, `-v` selects the lines that do not match, `-n` adds line numbers, `-c` writes only the count, `-q` writes nothing and stops at the first match. With several files each line starts with its file's name. | 0 if a line was selected, 1 if none, 2 for an error |
| `wc [-lwc] [file...]` | Writes the lines, words and bytes of each file (or standard input), or those asked for, and a total for several; a single count of a single input is written alone, standard input's counts 7 wide, files' as wide as the largest. | 0, or 1 if a file failed |
| `head [-n count] [-qv] [file...]` | Writes the first 10 lines of each file, or `count`; `==> file <==` headers for several files (`-q` none, `-v` always). | 0, or 1 if a file failed |
| `tail [-n count] [-n +line] [-qv] [file...]` | Writes the last 10 lines, or `count`, or everything from line `line` on with `+`; headers as `head`'s. It keeps the last 64 KiB and 4096 lines. | 0, or 1 if a file failed |
| `ps` | Lists every process from `/proc/ps`: its id, its parent's, its process group, its state (`R` running, `Z` ended and not yet waited for) and its name. `-A`, `-e` and `-f` are accepted. | 0, or 1 if `/proc` cannot be read |
| `free [-b\|-k\|-m]` | Writes the memory's total, used, free and available amounts from `/proc/meminfo`, in KiB, or bytes or MiB (`shared` and `buff/cache` are 0: there is no page cache). | 0, or 1 if `/proc` cannot be read |
| `uptime [-p]` | Writes how long the system has been up (`up 1 hour, 2 minutes`), from `/proc/uptime`, as `uptime -p` does elsewhere; there are no load averages or users to count. | 0, or 1 if `/proc` cannot be read |
| `poweroff`, `reboot`, `halt` | Power the machine off, reset it or halt it at once with `reboot(2)` (only for root; `-f` and `--force` are accepted and change nothing). | Does not return; 1 with the error when the kernel refuses |

## Limits and differences from GNU

- Lines are read in pieces of 16 KiB: `grep` and `head` treat a longer line as several. The
  text utilities flush their output before each read, so `cat | grep x` shows each match as it
  is typed.
- `cp` and `mv` copy regular files only (no `-r`, `-p`, `-i` or `-n`); `rm` asks nothing (no
  `-i`); `grep` has no `-E`, `-r`, `-o`, `-l` or back-references; `touch` sets only the
  modification time (no access time is kept, no `-d` or `-r`).
- These utilities are command-line only: the GUI and MCP front-ends every capability gets
  (D22) come with the capability framework and the MCP server (M14).

- `ls` shows owners and groups as numbers (there is no user database yet), times always as
  `YYYY-MM-DD HH:MM` in UTC (GNU's `--time-style=long-iso`; there is no time zone database),
  one name per line even on a terminal, and no colours. It holds one directory at a time:
  1024 entries and 32 KiB of names. A command takes at most 256 arguments.
- `poweroff`, `reboot` and `halt` act at once, as `poweroff -f` does elsewhere: there is no
  service manager to stop first and no writable file system to sync.
- `echo` takes no escapes (`-e`), like the shell's builtin; `sleep` takes no `inf`.
