# EggOS v0.3.0 Task Manager: design brief (part 48, roadmap item 22)

Design by Claude Fable (D16) for the GUI front-end of the processes capability (D22): see which
programs run, what they cost, and stop one. The CLI faces of the same capability are `ps`,
`free`, `uptime`, `wm list`, `wm close` and the new `kill` (section 10); the MCP faces `ps`,
`wm_list`, `wm_close`, `net`-style readers and the new `kill`. It is a toolkit app
(`toolkit-v0.3.md`) with the vocabulary of Settings' list-and-facts panes (`agents-v0.3.md`
section 2.1 for rows drawn as labels over a list of static rows, `storage-v0.3.md` for the
status line and the confirmation panel) and the Text Editor's window rules
(`text-editor-v0.3.md` section 1). Everything there stands unless this brief says otherwise.
Numbers are px at 1x and double at 2x; 1 px strokes, no anti-aliasing, no alpha, no motion but
the caret's blink and the refresh of section 5. The feel to match: Windows Task Manager's
Processes tab, Activity Monitor and its Force Quit window, GNOME System Monitor.

## 1. Name, launcher, shortcut

- **One name, `Task Manager`**, in every profile (the way `Files` and `Settings` are one name
  with every platform's words as search synonyms, `docs/ux/README.md` principle 3). It is the
  term the UX README, the roadmap and the plan already use, Windows's own word, and the one a
  GNOME or KDE user says out loud too. Not `Activity Monitor` (macOS's word, and this window has
  no graphs yet), not `System Monitor` (GNOME's app), not `Monitor` (says nothing about stopping).
  Program `/bin/task-manager`, present only when it is in the image (D18).
- **Catalogue**: a built-in `AppId::TaskManager`, name `Task Manager`, listed among the built-ins
  alphabetically (`Files`, `Settings`, `Task Manager`, `Terminal`, `Text Editor`), the context
  menu of a built-in (`Open`); `app open "Task Manager"` opens it (D22). Synonyms: `Activity
  Monitor`, `System Monitor`, `Force Quit`, `taskmgr`, `processes`, `process`, `tasks`, `kill`,
  `end task`, `top`, `htop`, `ps`, `CPU`, `memory`, `RAM`, `monitor`, `performance`, `not
  responding`, `frozen`, `hung`. `memory` also matches About in Settings; nothing else collides.
- **Not pinned**: not on the dock, the Linux dash or the desktop (none of the three systems pins
  theirs). A running or minimised window shows in the dock, the taskbar, the switcher and the
  launcher's `Minimised:` line like any app, with this icon.
- **Launching** (the Settings rule, `docs/apps.md`): focuses its window if it has one, else
  starts `/bin/task-manager`. `app_of` names a window titled exactly `Task Manager` as its (the
  built-ins' exact-title rule), so icon, tooltip and dock grouping are right.
- **The shortcut**, taken by the window server before any client sees the key (section 5 of
  `desktop-v0.3-gui.md`), in every profile, both: **Ctrl+Shift+Esc** opens or focuses the window;
  **Cmd+Opt+Esc** (Super+Alt+Esc on a PC keyboard, Cmd being the Super key code) opens it with
  `Apps only` ticked (`/bin/task-manager --apps`), or focuses it. Decided: the macOS habit is a
  small Force Quit list of apps, but it is this same window with the apps filter on, not a
  second, smaller dialog: one window to design, build, test and expose (D22), and a user who
  came for Force Quit sees only apps, the first selected, `End task` one Enter away. The key
  says the habit, not the profile, so a Mac user in the Windows profile keeps Cmd+Opt+Esc. KDE's
  Ctrl+Esc is not taken (it is Windows's Start key; left out). The shortcut works whatever has
  focus, as every server shortcut does, so a frozen full-screen program is no obstacle; the
  window server is the kernel's, so it answers even when userland is busy.
- **Icon**, in the shape vocabulary of `icons.rs` (`Tile(x,y,w,h, R, fill, edge)`, `Fill(x,y,w,h)`),
  a bar chart on the generic tile, which every OS uses for its monitor and which needs no badge:

  | Size | Shapes, in drawing order |
  |------|--------------------------|
  | 48x48 | `Tile(0,0,48,48, R6, icon.tile, icon.tile.border)`; bars `Fill(11,26,7,12)`, `Fill(21,12,7,26)`, `Fill(31,20,7,18)` in `accent`; baseline `Fill(9,40,30,2)` in `icon.page.line` |
  | 32x32 | `Tile(0,0,32,32, R4, icon.tile, icon.tile.border)`; bars `Fill(7,17,5,8)`, `Fill(14,8,5,17)`, `Fill(21,13,5,12)` in `accent`; baseline `Fill(5,27,22,2)` in `icon.page.line` |

  Both sizes join the pinned hash table.

## 2. Window and process

- **Process.** `/bin/task-manager [--apps]`, one window per process; the shell starts at most
  one (it focuses an existing window). Root only in 0.3.0 for the same reason `wm list` is
  (`/dev/window`); every program runs as root until users exist (M10), and section 4 already
  words the day they do.
- **Window.** A resizable `Normal` window titled `Task Manager`, default content **640x440**,
  minimum **480x300** (Files' and the Text Editor's sizes), placed by the cascade. No
  `FIXED_SIZE`, no `TAKES_CTRL_KEYS`: Ctrl+W, Cmd+W, Alt+F4 and the close control reach it as a
  `CloseRequest`, which it answers by closing (nothing to save; a pending forced end, section 4,
  finishes on its own timer in the kernel: a `SIGTERM` already sent needs no window).

## 3. Layout (content coordinates, width `W`, height `H`; at the default 640x440 the
right-aligned edge is x 624 and the list's rows end at x 616, 8 px before it for the bar)

Every widget has one fixed place; a state shows, hides, enables or retitles widgets and never
moves them (section 7 is the one exception, for narrow screens). The bottom block is anchored
to the content's bottom edge, so a taller window shows more rows and nothing else moves.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | text field | (16, 16), 208x28 | placeholder `Filter processes`, `escape: Clear` |
| 2 | checkbox | (240, 16), 96x28 | `Apps only`; unticked, or ticked when started with `--apps` |
| 3-7 | labels, the column headers | cell top 56: `Name` at x 40; `PID` right-aligned ending at x `W - 264` (376); `State` at x `W - 248` (392); `Memory` right-aligned ending at `W - 88` (552); `CPU` right-aligned ending at `W - 24` (616) | `text.dim` |
| 8 | list, mark x 8, text x 24 | (16, 76), `W - 32` wide, `rows x 24` tall where `rows = (H - 224) / 24` rounded down (9 at 440, 3 at 300) | 32 static rows whose text is empty; `set_shown_items` to the rows of section 4; empty text as section 6 |
| 9-53 | labels, five per visible row, row `r` at y `80 + 24r` | name at x 40, ellipsised at `W - 302` (338); PID right-aligned ending at `W - 264`; state at `W - 248`, ellipsised at `W - 152`; memory right-aligned ending at `W - 88`; CPU right-aligned ending at `W - 24` | the row's five texts (section 4) in `text`; retitled on every refresh, scroll and sort |
| 54 | label | (16, `H - 136`) (304) | the detail line or the status line (section 6), `text.dim` or `text` |
| 55 | label | (16, `H - 112`) (328) | `4 CPUs · 3 % busy · up 1 hour, 2 minutes` in `text` |
| 56 | label | (16, `H - 92`) (348) | `Memory 312 MB of 1.0 GB used · 4 MB kept by the kernel` in `text` |
| 57 | progress | (16, `H - 68`), 288x8 (372) | memory used permille, `used x 1000 / total` |
| 58 | radios, horizontal | (16, `H - 44`), 28 tall (about 256 wide, ends near x 272) | `By memory` `By CPU` `By name`; `By memory` when the window opens |
| 59 | push button | (`W - 112`, `H - 44`), 96x28 (528, 396) | `End task`, or `Force end` (section 4) |

The list draws selection, hover, the `>` mark and the focus ring; labels 9-53 are declared
after it so they draw over its rows, and labels take no pointer input, so a click on a row's
text reaches the list (the Agents pane's pattern; a toolkit table with its own cells is listed
under "Left out"). The list scrolls in whole rows (toolkit section 8), so the labels always sit
on whole rows: after a scroll the program retitles them with rows `offset .. offset + rows`.
Column headers are labels, not buttons, because labels take no clicks: sorting is the radio
group of widget 58, next to the button that acts on the sorted list. There is no rule line
under the headers (no widget draws one); `text.dim` headers 4 px above the first row's hover
fill read as a header without it. The `PID` column holds five tabular digits (30 px); `State`
holds `Not responding` (about 90 px) in its 96; `Memory`'s widest `999.9 MB` (about 50 px)
starts 8 px after `State` ends; `CPU`'s widest `100 %` is 30 px. At the minimum width the name
column is 138 px (about 20 glyphs); the CLI has the whole name. The status line's cell ends at
`H - 120`, the bar at `H - 60`, the bottom row at `H - 16`.

The confirmation panel (section 4), Files' dialog panel (`desktop-v0.3-gui.md` 8.1 item 3):

| # | Widget | Place | Text |
|---|--------|-------|------|
| 60 | panel | (`(W - 400) / 2`, `(H - 112) / 2`), 400x112 (120, 164 at the default) | `menu.bg`, `INSET_R6`, 1 px `menu.border` inside, a 2 px `field.bg` edge outside |
| 61 | label | (16, 16) in the panel | `End Text Editor?` in `text` |
| 62 | label | (16, 36) | `Unsaved work in it is lost.` in `text.dim` |
| 63 | push button | (184, 68), 96x28 | `Cancel` |
| 64 | push button, primary | (288, 68), 96x28 | `End`; the window's default button while the panel shows |

**Budget**: 64 widgets in one tree (59 in the window, 5 in the panel); the program's `CAPACITY`
is 64. While the panel shows, the window's widgets ignore pointer, wheel and Tab, hover is
cleared and the list draws unfocused; closing the window cancels the panel.

## 4. The rows: one set of words for the window, `ps` and MCP

**Sources, read together on every refresh (section 5)**: `/proc/ps` as `ps --json` reads it
(`pid`, `parent_pid`, `state`, `name`, and the two fields of section 9: `memory_bytes`,
`cpu_ms`); the window list as `wm list --json` reads it (`id`, `client_pid`, `title`, and
section 9's `not_responding`), through the same functions, moved where both can call them;
`/proc/meminfo` as `free` reads it; `/proc/uptime` as `uptime` reads it (and its idle field,
section 9); the CPU count as About and `settings get system.cpus` read it. The task manager
computes nothing the CLI could not: a row is `ps`'s record joined to `wm list`'s by `client_pid`.

**Which processes are rows.** Every process `/proc/ps` lists, the task manager itself included
(Windows lists itself too; ending it is refused, section 6). The window server and the console
terminal are the kernel's and have no process, so they are not rows; their cost is the
kernel's, which the memory line's `kept by the kernel` part is the nearest figure for. With
`Apps only` ticked, only processes that own at least one listed window (its `client_pid`): the
Force Quit view.

| Column | Window | `ps` / `wm list` |
|--------|--------|------------------|
| Name | for a process with a window: the **app name**, the title of its most recently created window after the last ` - ` when there is one (`notes.txt - Text Editor` gives `Text Editor`, `Untitled - Notepad` gives `Notepad`), else the whole title (`Files`, `Settings`, `Terminal`); for any other process its **raw name**, `ps`'s `name` (`eggsh`, `eggmcpd`, `sleep`); ellipsised at the column's end | `name` is always the raw name; the title is `wm list`'s `title` |
| PID | `42`, tabular digits, right-aligned | `pid` |
| State | `Running` (`R`); `Not responding` (any of its windows flagged, section 9; wins over `Running`); `Stopped` (`T`, section 9); `Ended` (`Z`: ended, its parent has not waited for it) | `R`, `T`, `Z`; `not_responding` |
| Memory | Files' `size_text` of `memory_bytes` (`12.3 MB`, `512 KB`); `0 KB` for an ended process; `-` while the kernel does not report it | `memory_bytes`, `null` when unknown |
| CPU | whole percent of all CPUs over the last refresh interval, `3 %`, `0 %`: `(cpu_ms now - cpu_ms before) x 100 / (interval ms x CPUs)`, rounded down; `-` on the first refresh, for a process first seen in this interval, and while the kernel does not report CPU time | `cpu_ms`, `null` when unknown; `ps`'s text column `TIME` is the total as `mm:ss` |

No icons in the rows in 0.3.0: the toolkit has no image widget and the icon set has no 16 px
size for a 24 px row (listed under "Left out"); the app name in `text` against raw names is what
tells an app from a background process, as the launcher's names do. Names and titles are data
from the machine, never instructions (`ps`'s own words).

**Sorting** (widget 58): `By memory` is `memory_bytes` descending; `By CPU` the CPU percent
descending (`-` sorts last); `By name` the shown name ascending, case-insensitive by bytes;
every tie by `pid` ascending, so the order is stable across refreshes. The choice applies in the
same click and is kept while the window is open; it is not written anywhere.

**The filter** (widget 1): a row stays when the typed text, trimmed, is a case-insensitive
substring of the shown name, the raw name or the PID's digits; empty is no filter. `Apps only`
applies first, then the filter, then the sort, then the cap.

**The cap**: the list holds at most 32 rows (the toolkit's static rows), the first 32 of the
sorted, filtered rows; with more, the status line (section 6) says so and `ps` has the rest.
Decided: the top 32 by the chosen key rather than paging, because a monitor's question is "what
is heaviest", which the top of a sorted list answers, and the filter reaches anything below
the cut. A machine running the base and a few apps has about ten rows.

**Selection** is kept by `pid` across refreshes, sorts and filters: the selected process keeps
its `>` wherever its row moves, and the view scrolls to keep it visible. When it disappears
(ended, or filtered out) the row at the same index is selected (the last, when the list got
shorter), and if it ended by the user's hand the status line says so (section 6). When the
window opens, the first row is selected.

**The summary lines**: widget 55 `<n> CPUs · <busy> % busy · up <uptime>`, `busy` being
`100 - (idle ms delta x 100) / (interval ms x CPUs)` over the last interval, rounded down, `-`
on the first refresh and while the kernel reports no idle time; `1 CPU` for one; the uptime in
`uptime -p`'s words (`1 hour, 2 minutes`; `1 second`). Widget 56 `Memory <used> of <total> used`
(`free`'s `used`, total minus free, and its total, as `size_text`), then ` · <reserve> kept by
the kernel` when `/proc/meminfo` has `KernelReserve`, nothing otherwise; the bar is used over
total. The `·` is U+00B7.

**The detail line** (widget 54, idle, `text.dim`): the selected process as `ps` knows it, for
when the shown name is an app's: `text-editor · pid 42 · parent 7 (eggsh) · notes.txt - Text
Editor`, the raw name, the pid, the parent's pid and raw name in brackets, and its most recently
created window's full title (omitted for a process without one); cut at a label's 64 bytes
with `...`. A status line (section 6) replaces it until the next selection change or action.

## 5. Refresh

Every **2 s** while the window is shown, whether or not it has focus, and paused while it is
minimised, with one refresh the moment it is restored or opened. Decided: a monitor is watched
beside the window it watches (Windows's Task Manager even has "always on top" for it), so
losing focus must not freeze it; and a refresh is two `/proc` reads and one `/dev/window`
request, cheap enough that the pause is for the unshown window only (D24: a hidden refresh
costs CPU for nothing). Windows's default is 1 s and Activity Monitor's 5 s; 2 s keeps the CPU
column meaningful (a 1 s interval at 100 Hz ticks is coarse) without lagging a hog. A refresh
damages only the labels whose text changed, the bar when its permille changed, and the list
when its row count or selection moved (toolkit section 9). F5 refreshes now. An action
(section 6) refreshes at once and again at its timer's ticks.

## 6. End task: the rules, the states and every line

**Which button the selected row gets**, decided on every refresh and selection change, with
the status line (widget 54, `text.dim`) giving the reason whenever it is disabled:

| Selected process | Button | Status line while idle |
|------------------|--------|------------------------|
| has a window, not flagged `Not responding`, no close asked yet | `End task`, enabled | the detail line |
| has a window and a close was asked (below), or is `Not responding` | `Force end`, enabled | `Asked Text Editor to close; Force end ends it without saving.` (after the ask) or the detail line |
| no window, `Running` or `Stopped` | `End task`, enabled | the detail line |
| `init` (pid 1) | `End task`, disabled | `init runs EggOS: use Power off to stop everything.` |
| the task manager itself | `End task`, disabled | `To close the Task Manager, close its window.` |
| `Ended` | `End task`, disabled | `sleep has ended; eggsh has not collected it yet.` |
| the list is empty | `End task`, disabled | the empty text says it (below) |

**Pressing it** (a click, Space or Enter on the button, or **Delete** with the focus in the list,
Files' key for "remove this row"; Enter in the list does nothing, a row has no open action):

1. **`End task` on a process with a window**: `wm close` on each of its windows, newest first
   (a `CloseRequest` each; the app may ask its user to save, as the Text Editor does). The
   button retitles to `Force end` for this process, remembered by pid until the process is
   gone, and the status line reads `Asked Text Editor to close; Force end ends it without
   saving.` in `text`. Nothing more happens by itself. Decided: no timer escalates a polite
   close into a kill, because the app may be asking "Save changes?" and the task manager
   cannot tell that dialog from a hang (apps draw their questions inside their own window);
   a user who wants it gone presses once more. A window the server has already flagged
   `(not responding)` skips this step: its app reads no events, so asking is pointless, and
   the button is `Force end` from the start, as Windows goes straight to termination for an
   app that is not responding.
2. **`Force end`, or `End task` on a process without a window**: the panel of section 3,
   `End <shown name>?` / `Unsaved work in it is lost.`, focus on `Cancel`, `End` primary (the
   destructive answer is one Enter away but the ring starts on the safe one, Storage's and
   Agents' rule). `Cancel`, Esc or closing the window does nothing. `End` sends `SIGTERM`
   (`kill <pid>`), the status reads `Ending Text Editor...`, and if the process is still listed
   3 s later it gets `SIGKILL` (init's own cadence for power-off, D183: `SIGTERM`, 3 s,
   `SIGKILL`); the timer runs on the refresh tick. A `Stopped` process also gets `SIGCONT`
   with the `SIGTERM`, as init sends it, so it can see the signal.
3. **Afterwards**: the refresh that no longer lists the process shows `Text Editor ended.`
   (after `SIGTERM`) or `Text Editor did not stop; EggOS forced it to end.` (after `SIGKILL`),
   in `text`; the row at the same index is selected. A process still listed 1 s after
   `SIGKILL` (it cannot be, short of a kernel bug) reads `Text Editor could not be ended.`

**Errors**, each in `text` on the status line, the panel closed, nothing retried:
`Text Editor has already ended.` (`ESRCH`, or the pid no longer listed when the button is
pressed: the next refresh drops the row); `You may not end eggsh: it belongs to another user.`
(`EPERM`; cannot happen while everything is root, worded now for M10); `Could not ask Text Editor
to close (no display).` when `wm close` fails (the window list then also fails; see below).

**Every other state**, the empty texts at x + 16 of the first row in `text.dim`:
`No processes match.` (the filter leaves none); `No apps are open.` (`Apps only` and none owns a
window); `The process list could not be read.` (`/proc/ps` fails; the summary lines keep their
last values, the button is disabled). When the window list cannot be read (`/dev/window`
refused or absent), every process counts as windowless, `Apps only` is disabled and unticked,
and the status line reads `Windows could not be listed: every process shows by its name.` When
`/proc/meminfo` or `/proc/uptime` fails, that line reads `Memory: not reported` or `CPUs: not
reported` and the bar hides. With more than 32 rows after the filter: `Showing the first 32 of
47 processes by memory; ps lists them all.` (`by CPU`, `by name`; `matching processes` when the
filter is set), which an action's line replaces until the next refresh.

## 7. Keys, focus, pointer and every device class (D23)

- **Tab order**: the filter field, `Apps only`, the list, the sort radios, `End task`; disabled
  ones skipped; wrapping. Initial focus is the list (the shortcut's user wants a row, not a
  search; Settings starts in its search field because its job is finding). Down in the filter
  field moves the focus to the list, as Settings' search does.
- **In the list**: Up/Down, Home/End, PageUp/PageDown as toolkit section 3; Delete is the button
  (above); Enter, Space and a double-click do nothing. In the radios Left/Right sort at once.
  F5 refreshes. Esc clears the filter field when it has focus and text, else goes to the window,
  which ignores it (`Normal`); while the panel shows, Esc is `Cancel`, Enter `End`, Tab moves
  between `Cancel` and `End`.
- **Pointer**: `Arrow` everywhere but the field (`IBeam`); a click selects and focuses the list;
  a right-click does nothing (a context menu with `End task` is later).
- **One column**, when the content is narrower than 448 (a phone's maximised window, where the
  work area clamps the minimum): the field takes the width minus 32; `Apps only` goes under it
  at (16, 52); the headers hide; the list starts at y 88 with rows 40 tall and two lines: the
  name label at the row's +4 in `text` and the PID label, retitled `pid 42 · 12.3 MB · 3 % ·
  Running` (`-` for unknowns), at +20 in `text.dim`, the other three labels hidden, the `>` at
  x 8 with its cell top at +12, the wheel 120 px (3 rows); the bottom block keeps its order
  with the sort radios wrapped to two lines of two (28 + 28) and `End task` under them, full
  width minus 32, 40 tall; the window's content scrolls in a scroll view when the block does
  not fit (the focused widget kept in view). The panel is a bottom sheet: full width, the two
  lines with 16 px padding, `Cancel` and `End` stacked full width, 40 tall, 8 apart, `End`
  last (nearest the thumb; `Back` is `Cancel`).
- **Touch**: list rows 40 tall (two lines as above, whatever the width), buttons and radios 40
  tall, the field 28 as everywhere.
- **TV (scale 2)**: everything doubles; the ring on the selected row and the focused button is
  the cue; the remote's arrows, OK and Back work the whole window and the panel.
- **Keyboard-only** on every form factor: the list is one Tab stop; ending a process is Delete,
  Tab, Enter; a pointer is never required.
- **Headless** (no desktop component): no window, no shortcut; `ps`, `kill`, `free`, `uptime`
  and the MCP tools are the whole capability, and `wm close` says there is no display.

## 8. What an agent sees (D161, `wm show`), and what it may not touch

The list is a `list` named `Processes`, its rows `list item`s named by the shown name
(`Text Editor`) with the value `<pid> <raw name> <state word> <memory_bytes> <cpu percent>`
(`42 text-editor running 12894208 3`; `-` for an unknown, `not-responding`, `stopped`,
`ended`); the field a `text field` named `Filter processes`; the checkbox a `checkbox` named
`Apps only`; the headers, the detail or status line and the two summary lines `label`s with
their drawn text; the bar a `progress` named `Memory used` with its permille as the value; the
sort group a `radio group` named `Sort` with `radio`s `By memory`, `By CPU`, `By name`, the
chosen one `selected`; `End task` (or `Force end`), `Cancel` and `End` `button`s, disabled
ones flagged.

**Person-only widgets (SECURITY)**: `End task`/`Force end` and the panel's `End` carry the
toolkit's `person-only` flag (`agents-v0.3.md` section 8): `wm act` answers `element_cannot`.
An agent ends a process through the `kill` tool (section 10), which is `destroy`, needs the
`run` grant and `confirm`, and lands in the audit log with the pid it named (D21: destructive
actions need the user's confirmation and every agent action is logged); the tree route would
skip both. Reading is open: `Select` on the list, `SetText` on the field, `Invoke` on the
checkbox and `Select` on the sort group all answer `Done`, and an agent that wants the numbers
calls `ps`, not the tree (D22).

## 9. What the kernel and the window server must report that they do not yet

The design needs these five fields; until each lands, the window shows `-` where the table of
section 4 says so, and nothing else changes. Part 48 adds 1, 3 and 4; part 51 adds 2.

1. **Memory per process**: the frames a process's address space holds (its own, plus window
   buffers it mapped, each counted once), as a seventh field of `/proc/ps` in KiB, `ps --json`'s
   `memory_bytes`, `ps`'s text column `RSS`. The kernel keeps frames per address space but does
   not count them today.
2. **CPU time per process**: the sum of its threads' run time in ms, charged at each switch
   (part 51, `CLOCK_PROCESS_CPUTIME_ID` and `getrusage` need it too), an eighth field of
   `/proc/ps`, `ps --json`'s `cpu_ms`, the text column `TIME` as `mm:ss`.
3. **Idle time** in `/proc/uptime`'s second field, the sum over CPUs, which the kernel writes as
   `0.00` today; `uptime --json` gains `idle_ms`.
4. **`not_responding` in the window list**: the server already keeps the flag per window (it
   draws `(not responding)` from 500 ms on) but the `LIST` record and `wm list --json` do not
   carry it; add it as `not_responding` (`wm list`'s text shows the state as `not-responding`).
5. **`T` in `/proc/ps`'s state** for a process every thread of which is stopped by `SIGSTOP`
   (the kernel knows; `/proc/ps` writes only `R` and `Z`). A process neither stopped nor ended
   stays `R` whether it runs or sleeps: the window says `Running` for both, as Windows does.

Not needed: thread counts, start times, users (until M10), per-process limits.

## 10. CLI and MCP (D22)

- **`kill [-s <signal> | -<signal>] [--] <pid>...`, `kill -l`**: POSIX's, in coreutils. `<signal>`
  is a name with or without `SIG` (`TERM`, `KILL`, `INT`, `HUP`, `QUIT`, `STOP`, `CONT`, `USR1`,
  `USR2`, `CHLD`, `ALRM`; not case-sensitive) or a number (`-9`); `TERM` by default; `-l` lists
  the names, `-l <number>` one name. Silent on success; errors `kill: 42: No such process`,
  `kill: 1: Operation not permitted`, `kill: x: invalid signal` (with `  try: kill -l`); exit 0,
  1 if any pid failed, 2 for a bad usage. `0`, `-1` and negative pids as POSIX. It is the GUI's
  forced step; the polite step is `wm close <id>`; the escalation is the user's own second
  command (`kill -KILL`), with no timer in the CLI.
- **MCP `kill`**: arguments `pid` (required), `signal` (a name, `TERM` by default) and `confirm`;
  effect `destroy`, grant `run`; its twin is `kill -s <signal> -- <pid>`; the result says `Sent
  TERM to 42 (text-editor).` or the error's code (`no_such_process`, `permission_denied`,
  `invalid_value`). `ps` gains the two fields of section 9 in its records; `wm_list` gains
  `not_responding`; `uptime`'s JSON gains `idle_ms`.
- **The registry**: `ps`'s GUI `surface` becomes the Task Manager (`command-model.md` said it
  waits for one); `kill`'s is the Task Manager's `End task`; `free`'s and `uptime`'s stay About
  and gain this window as a second surface.

## 11. Acceptance (host tests, default 640x440, scale 1x, a fake with `/proc/ps` lines
`1 0 1 1 R init`, `7 1 7 1 R eggsh`, `9 1 9 1 R eggmcpd`, `12 7 12 1 R terminal`,
`42 7 42 1 R text-editor`, `43 7 43 1 Z sleep`, `50 7 50 1 R files`, `60 7 60 1 R task-manager`
with memory 512 KB, 1.2 MB, 2.4 MB, 6.1 MB, 12.3 MB, 0, 9.8 MB, 3.0 MB and CPU times that give
`0 %` everywhere but `3 %` for 42 over a 2 s interval; windows `1` client 12 `Terminal`, `2`
client 42 `notes.txt - Text Editor`, `3` client 50 `Files`, `4` client 60 `Task Manager`;
`MemTotal` 1 GiB, used 312 MB, `KernelReserve` 4 MB; uptime 3725 s; 4 CPUs)

- The catalogue lists `Task Manager` third among the built-ins; `force quit`, `activity
  monitor`, `system monitor`, `processes`, `kill`, `top` and `taskmgr` match it and nothing
  else; `memory` matches it and Settings' About.
- The server's key table maps Ctrl+Shift+Esc to `app open "Task Manager"` and Super+Alt+Esc to
  `/bin/task-manager --apps` in all three profiles; with a window titled `Task Manager` shown,
  both focus it and start nothing.
- In content coordinates: the field (16, 16, 208, 28) empty and unfocused; `Apps only` at
  (240, 16) unticked; headers at cell top 56, `Name` at x 40, `PID` ending at 376, `State` at
  392, `Memory` ending at 552, `CPU` ending at 616; the list (16, 76, 608, 216) focused with
  8 rows, by memory: `Text Editor` `42` `Running` `12.3 MB` `3 %`, `Files` `50`, `Terminal` `12`,
  `Task Manager` `60`, `eggmcpd` `9`, `eggsh` `7`, `init` `1` `512 KB`, `sleep` `43` `Ended`
  `0 KB` `-`; the first selected; the detail line at (16, 304) `text-editor · pid 42 · parent 7
  (eggsh) · notes.txt - Text Editor`; (16, 328) `4 CPUs · 3 % busy · up 1 hour, 2 minutes`;
  (16, 348) `Memory 312 MB of 1.0 GB used · 4 MB kept by the kernel`; the bar (16, 372, 288, 8)
  at 305 permille; the radios at (16, 396) with `By memory` chosen; `End task` (528, 396, 96,
  28) enabled. On the first refresh every CPU cell and the busy figure read `-`.
- `By name` orders `eggmcpd`, `eggsh`, `Files`, `init`, `sleep`, `Task Manager`, `Terminal`,
  `Text Editor` and keeps `Text Editor` selected, now the last row; `By CPU` puts it first.
- Typing `ed` leaves `Text Editor` (its raw name `text-editor` too) and `Ended`'s `sleep`? no:
  the rows `Text Editor` only; `4` leaves `42`, `43` and `Files` (`50`? no) : `Text Editor`,
  `sleep`; `zz` leaves none and the list reads `No processes match.` with `End task` disabled;
  Esc empties the field and brings the eight back. `Apps only` leaves `Text Editor`, `Files`,
  `Terminal`, `Task Manager`; started with `--apps` the box is ticked at open.
- Down to `Files`, then Delete: the fake records `wm close 3`, the button reads `Force end`, the
  status line `Asked Files to close; Force end ends it without saving.`; a click on (560, 410)
  shows the panel at (120, 164, 400, 112) with `End Files?`, `Unsaved work in it is lost.`,
  focus on `Cancel`; Esc closes it and sends nothing; Delete, Tab, Enter sends `kill(50, TERM)`
  and reads `Ending Files...`; after 3 fake seconds with 50 still listed the fake records
  `kill(50, KILL)`; the next refresh without 50 reads `Files did not stop; EggOS forced it to
  end.` and selects `Terminal` (the same index); a fake where 50 leaves after `TERM` reads
  `Files ended.` and records no `KILL`.
- A window `3` flagged `not_responding` makes `Files` read `Not responding`, the button `Force
  end` at once, and Delete opens the panel without any `wm close`.
- `eggsh` selected: `End task` opens the panel directly (no window), `End` sends `kill(7, TERM)`;
  with the fake answering `EPERM` the line reads `You may not end eggsh: it belongs to another
  user.`; with `ESRCH`, `eggsh has already ended.`
- `init` selected: `End task` disabled, `init runs EggOS: use Power off to stop everything.`;
  `Task Manager`: `To close the Task Manager, close its window.`; `sleep`: `sleep has ended;
  eggsh has not collected it yet.`
- A fake of 40 processes shows 32 rows and `Showing the first 32 of 40 processes by memory; ps
  lists them all.`; a fake whose `/proc/ps` fails reads `The process list could not be read.`
  with the button disabled; one whose window list fails disables `Apps only` and reads `Windows
  could not be listed: every process shows by its name.`, every name raw.
- Two refreshes 2 fake seconds apart with 42's `cpu_ms` grown by 240 read `3 %` (240 x 100 /
  (2000 x 4) = 3); a window minimised in the fake gets no refresh for 10 s and one on restore.
- A content 400 wide shows two-line rows at 40 with `pid 42 · 12.3 MB · 3 % · Running`, the
  headers hidden, `End task` full width; the element tree holds the list `Processes` with eight
  items, the first `Text Editor` with value `42 text-editor running 12894208 3`, the group
  `Sort`, the buttons `End task`, `Cancel`, `End` flagged `person-only`; `wm act` `Invoke` on
  `End task` answers `element_cannot` and sends nothing; `Select` on the second item answers
  `Done`.
- `ps --json`, `ps` and the window agree row for row on the fake (name, state, memory, CPU
  time); `kill -l` lists the names of section 10; `kill 42` on the fake records `kill(42, TERM)`
  and prints nothing; `kill -9 42` records `KILL`; `kill x` exits 2 with `invalid signal`.
- QEMU, all three architectures: Ctrl+Shift+Esc opens the window over the desktop; the
  screendump matches the host's render; Down to the test program's row and Delete, then Delete,
  Tab, Enter ends it, and `ps` no longer lists it.

## Decided by design (the owner can overrule any of these)

One name `Task Manager` in every profile, with each platform's words as synonyms; both
shortcuts in every profile, Cmd+Opt+Esc being the same window with `Apps only` on, not a
second dialog; no timer from the polite close to the kill, a second press forces, with
`SIGTERM` then `SIGKILL` after 3 s (init's cadence); refresh every 2 s while shown, focused or
not, paused only while minimised; the top 32 rows by the chosen key rather than pages; sorting
by a radio group, since headers are labels; no icons in rows; `End task`, `Force end` and `End`
person-only for agents; initial focus in the list.

## Left out of 0.3.0

Icons in rows (a 16 px icon size and an image widget); a toolkit table with its own cells,
header clicks and resizable columns, which would replace the 45 labels; a context menu on a
row; graphs and history (CPU and memory over time); per-process threads, start time, user,
open files and windows as children; a `Details`-style second view; disk and network columns;
ending every window of an app in one step across several processes; Windows's "Startup apps"
and "Services"; a process tree; `Always on top`; `Ctrl+Esc`; paging past the top 32 in the
window; a `nice` or priority control; an out-of-memory killer (D155 lists it); the Force Quit
habit of relaunching Finder (no Finder); live updates faster than 2 s.
