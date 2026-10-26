# The EggOS core utilities

`userland/coreutils` (D108) is one program, `/bin/coreutils`, with the utilities as hard links
to it in `/bin`: `cat`, `echo`, `false`, `halt`, `ls`, `poweroff`, `reboot`, `sleep`, `true`
and `uname`. It runs the utility named by the last component of the name it was started by, so
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
| `poweroff`, `reboot`, `halt` | Power the machine off, reset it or halt it at once with `reboot(2)` (only for root; `-f` and `--force` are accepted and change nothing). | Does not return; 1 with the error when the kernel refuses |

## Limits and differences from GNU

- `ls` shows owners and groups as numbers (there is no user database yet), times always as
  `YYYY-MM-DD HH:MM` in UTC (GNU's `--time-style=long-iso`; there is no time zone database),
  one name per line even on a terminal, and no colours. It holds one directory at a time:
  1024 entries and 32 KiB of names. A command takes at most 256 arguments.
- `poweroff`, `reboot` and `halt` act at once, as `poweroff -f` does elsewhere: there is no
  service manager to stop first and no writable file system to sync.
- `echo` takes no escapes (`-e`), like the shell's builtin; `sleep` takes no `inf`.
