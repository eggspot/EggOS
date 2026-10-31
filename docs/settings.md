# Settings

Settings is EggOS's settings app (D132), designed by Claude Fable
([desktop-v0.3-gui.md](ux/desktop-v0.3-gui.md) section 10,
[toolkit-v0.3.md](ux/toolkit-v0.3.md) section 11). One program, `/bin/settings`, is all of its
front-ends (D22): run with no arguments it opens the Settings window; with arguments it is the
`settings` command. Its MCP tools (`settings_get`, `settings_set`, `settings_list`) come with
the MCP server (D21). The same program, started as `/usr/libexec/app-window` (a hard link),
is the installer's window instead ([Apps](apps.md), D134), so the toolkit is in the image once.

## The window

Open it from the dock (macOS profile), the launcher (Super, Cmd+Space or Start, then type
`settings`, `control panel` or `preferences`), its desktop icon, or type `settings` at the
prompt (D133). The window has a sidebar with a search field and the panes
`Appearance`, `Display`, `Keyboard`, `Mouse`, `Storage`, `Date & time`, `Agents`, `Components` and `About`; typing in the search
field keeps the panes whose name, synonyms (`Control Panel`, `Preferences`, `theme`, `HiDPI`,
...) or settings match. Tab moves between the search, the sidebar and the pane's controls;
Down in the search goes to the sidebar.

**Appearance** asks "Where are you coming from?" with three cards, `Windows`, `macOS` and
`Linux`: click one, or move to it with the arrows and press Enter or Space. The desktop changes
at once: the window controls move to the right (Windows, Linux) or the left (macOS) on every
window, and the profile's shortcuts apply (Cmd+W and Cmd+M in the macOS profile, Ctrl+W in the
others, Super+H to minimise in the Linux profile; docs/windows.md), and the shell changes with
it: the taskbar (Windows), the top bar and the dock (macOS) or the top bar with `Activities`
(Linux). An open Settings window follows a profile set with `settings set` the next time it is
focused. The pane shows the
profile's primary modifier and control side. The choice is saved and the desktop starts in it
at the next boot; without a disk the pane says `No disk attached: this choice lasts until
power-off.`

**Display** shows the resolution, the scale (1x) and how the pointer is drawn; **Keyboard** the
layout (US); **About** the version, the architecture, the CPUs, the memory and the uptime;
**Mouse** and **Components** come in a later version. The scale is 1x in 0.3.0 (the `2x`
choice is shown disabled).

**Date & time** (D150, Claude Fable's docs/ux/date-time-v0.3.md) shows the time in UTC,
`Thursday 29 October 2026, 14:05:09 UTC`, ticking while the pane is shown, and where it comes
from (`From the hardware clock, read at boot.`, or `No hardware clock: ...`). `Set to` takes
`YYYY-MM-DD HH:MM[:SS]` or `HH:MM[:SS]` (today's date), in the years 2000 to 2099, the same
rules as `date -s` and the MCP tool `clock_set`; Enter or `Set` sets the clock and the hardware
clock, and the line under them says how it went (`Set, and saved to the hardware clock.`, or
why not). There are no time zones: everything is UTC. Its command-line front-ends are `date`
and `hwclock` (docs/coreutils.md), not `settings`.

**Storage** (D192, Claude Fable's docs/ux/storage-v0.3.md) lists the disks attached
(`/dev/vda` to `/dev/vdd`) and shows the selected one's contents (`EggFS "Photos"`, `Blank`,
...), size, used and free space with a bar, where it is mounted and whether it is read-only,
with the same words as `disk list` and the MCP tool `disk_list`. It reads the disks each time
it is shown and after every action. `Check` checks an unmounted EggFS disk without writing and
shows the headline (`Clean. 4,096 blocks, 3,900 free.` or `Found 3 problems: 2 repairable, 1
not. ...`) and up to five findings; `Repair` is enabled once a check of the same disk found
something repairable, and repairs what the check listed. `Format...` opens a panel that names
what will be erased, takes an optional label (64 bytes at most) and enables `Format` only once
the disk's short name (`vdb`) is typed; Esc or `Cancel` writes nothing. A mounted disk is not
checked, repaired or formatted in this version, and the status line says why whenever a button
is disabled. Its command-line front-end is `disk` (docs/coreutils.md), its MCP tools
`disk_list`, `disk_check`, `disk_repair` and `disk_format`.

**Agents** (D193, Claude Fable's docs/ux/agents-v0.3.md) lists the AI agents EggOS knows,
`local` (the local channel's agent) first, then the agents in the grants file
(`/var/lib/eggos/agent-grants`), and sets the selected one's grants: for each of `View`,
`Write`, `Install` and `Run` a `Deny` `Ask` `Allow` choice that writes the file at once (the
server reads it before every call, so it applies from the agent's next call), with the end
chosen in `Grants last` (`No end`, `15 min`, `2 h`, `7 d`). Each class line says what holds it
back or brings it: `started without it` (EggOS was not started with that grant, every agent's
ceiling), `run brings it`, `until 2026-10-31 11:14` or `ended ...`. `Ask` or `Allow` on `Run` or
`Install` opens a panel that asks first, as `agent grant` needs `--yes`; `Revoke all` takes
every grant back, `Forget` removes an agent (never `local`), and `Add` adds an id with no
grants. A damaged grants file disables every change and says why. The controls that change
grants are person-only: an agent's `wm act` on them answers `element_cannot`. Its command-line
front-end is `agent` (docs/coreutils.md), its MCP tools `agent_list` and `agent_show` (an agent
reads its own grants; none changes them).

The pane's `Activity` view (the `Grants` `Activity` choice beside its heading) browses the
audit log: `agentlog`'s rows, one per call (`interrupted` for a call still open), the newest 32
that match, newest first, with `7 of 418` saying how many match in all. `This agent only` keeps
the selected agent's calls, `Tool name` one tool's, the result choice `ok`, `error` or
`refused` calls that ended so, and `Last hour`, `Last day` or `Last week` the recent ones; the
status line shows the chosen call's detail line (the agent, the detail and the arguments). The
log is read when the view shows and when a filter changes, never on a tick. Its controls only
filter, so agents may use them; the CLI front-end is `agentlog`, and no MCP tool reads the log
(agents read it through `run`).

**The grant prompt** (D193 slice 41c3, design section 5) is EggOS's own window, `An agent
asks`, that the MCP server opens when it must ask you about a call (a grant set to `Ask`, or a
call that removes or replaces something) and the desktop runs: `settings --grant-prompt` reads
the question as one JSON line on stdin and writes one word on stdout, `once`, `session`,
`always`, `deny` or `timeout`. It shows the agent's name (and `id phone` when it differs), the
tool and its title, up to three `key: value` lines of the arguments and `Denied in 1:30 if you
do nothing.` counting down; `Deny` is the default and focused, Enter and Esc deny from
anywhere, closing the window denies, and after 90 s the answer is `timeout`. `This session`
allows the agent's calls of that class until its client disconnects (the server keeps it in
memory only); `Always` writes `allow` for that class into the grants file, as `agent grant`
does, and is offered only for `view` and `write` calls that need no yes of their own. Its
buttons are person-only and its window takes the person's input only: `wm click`, `wm type`,
`wm key` and menu choices aimed at it are refused (`EPERM`), and `wm act` answers
`element_cannot`.

## The command

| Command | Does | Status |
|---------|------|--------|
| `settings` | Opens the window and serves it until it is closed. | 0; 1 without a display |
| `settings list` | Writes every setting as `key=value`, one per line. | 0 |
| `settings get <key>` | Writes the setting's value. | 0; 1 for an unknown key |
| `settings set [--dry-run] <key> <value>` | Sets it: saved in the settings file, then applied at once on the display (with no display it applies at the next boot). `--dry-run` checks the key and the value and says `would set <key> from <old> to <new>`, changing nothing (D164). | 0; 1 for an unknown or read-only key, or when saving or applying failed (`settings: <key>: <why>`); 2 for an invalid value or a bad command |

| Key | Value | Settable |
|-----|-------|----------|
| `shell.profile` | `windows`, `macos` (the default) or `linux` | yes |
| `display.scale` | `1` | no (2x comes later) |
| `display.resolution` | the screen's size, `1280x800` | no |
| `display.pointer` | `software` | no |
| `keyboard.layout` | `us` (the default), `us-intl`, `gb`, `us-dvorak`, `us-colemak`, `dk`, `fi`, `fr`, `de`, `it`, `no`, `br`, `es` or `se` (D185, the Keyboard pane, docs/ux/keyboard-v0.3.md) | yes |
| `system.version`, `system.arch`, `system.cpus`, `system.memory_mb`, `system.uptime_s` | facts about the machine | no |

## The settings file

`/home/root/.config/eggos/settings`: under `/home`, so on the disk when one is attached (D131)
and in memory for the session otherwise; in `root`'s home until there are accounts. A small
text file: the line `eggos-settings 1`, then `key=value` lines. A file of another version, a
longer one than 4 KiB or garbage holds nothing, so every setting takes its default; unknown
keys are kept when the file is written, and the last line of a key wins. A change is written to
`settings.tmp.<process id>` beside it (one per writer), synced, and renamed over it, so a failure leaves the old file whole. Before it writes, a writer removes the temporary files of writers that `/proc/ps` no longer lists (one that crashed), best effort (D134).
The kernel reads `shell.profile` and `keyboard.layout` from it after mounting the disks and
before the desktop starts, so the first frame is already in the saved profile and the first key
types in the saved layout. `settings set keyboard.layout` and the Keyboard pane apply a layout
at once through `/dev/window`'s `SET_KEYBOARD_LAYOUT` (without a display, from the next boot). The format is the `eggos-settings-file`
crate (`settings-file/`), shared by the kernel and the program.

Not yet: the boot parameter `eggos.profile=` (the kernel has no command line yet), per-user
settings (there are no accounts), 2x, key repeat, the mouse settings, the components list.
