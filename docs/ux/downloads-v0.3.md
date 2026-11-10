# EggOS v0.3.0 Downloads: design brief (D225, batch part 7)

Design by Claude Fable (D16) for the GUI front-end of downloading (D22): `fetch <url> [<file>]`
and the MCP tool `fetch` download over HTTP and HTTPS (D158, D159); this brief adds the
window that shows every download in flight or finished since boot, lets a person start one,
stop one and clear the finished ones, and the CLI and MCP twins of each of those (`download
list`, `download cancel`, `download clear`; `download_list`, `download_cancel`,
`download_clear`). It is its own brief rather than a section of `network-v0.3.md` because that
brief is the Settings `Network` pane and the bar's indicator, a read-only view of the
adapter, while this is an app of its own, an optional component (D18) with a launcher entry,
a window, an icon and actions; a reader looking for "the downloads window" would not look
under Network. It is a toolkit app (`toolkit-v0.3.md`, the table of `toolkit-v0.5.md` section
4) with the Task Manager's window rules (`task-manager-v0.3.md` sections 2, 5 and 7) and
Files' size words. Numbers are px at 1x and double at 2x; 1 px strokes, no anti-aliasing, no
alpha, no motion but the caret's blink and the numbers of section 5. The feel to match: the
downloads list of Chrome, Safari, Firefox's Library and GNOME Web: a list, newest first, each
row a name, a size, a state, and a way to stop it.

## 0. What a person gets, and how that is checked

- Open the launcher, type `down`, Enter: a window `Downloads` with one row per download since
  boot, the newest at the top, the running ones counting up in bytes and percent once a
  second, done ones saying `Done`, failed ones saying why in a few words.
- Paste a web address in the field at the top and press Enter: the file lands in
  `/home/root/Downloads` and its row appears within a second.
- Select a running row and press Delete, or click `Cancel`: it stops, its partial file is
  gone, the row says `Cancelled`.
- `Clear` removes the rows that are over; the running ones stay.
- `download list` on the CLI prints the same rows; an agent reads `download_list` and stops one
  with `download_cancel` (section 10). Section 11's host tests pin every coordinate and text.

## 1. Name, launcher, icon

- **Name `Downloads`**, the word every browser and every desktop uses for the folder and the
  list; one name in every profile. Program `/bin/downloads`, present only when the component
  is in the image (D18): the catalogue lists `AppId::Downloads` **when the build has it**, as
  the Task Manager and the Text Editor are listed, so an image without it has no entry, no
  process and no cost.
- **Catalogue**: name `Downloads`, listed alphabetically among the built-ins (`Downloads`,
  `Files`, `Settings`, `Task Manager`, `Terminal`, `Text Editor`); `app open Downloads` opens it
  (D22). Synonyms: `download`, `fetch`, `transfers`, `wget`, `curl`, `Library` (Firefox's
  window), `download manager`, `downloading`. Nothing collides.
- **Not pinned** (no browser's downloads list is a dock item); a running or minimised window
  shows in the dock, the taskbar, the switcher and the launcher's `Minimised:` line like any
  app, with this icon. **Not offered on a wearable** (section 7).
- **Launching** (the Settings rule, `docs/apps.md`): focuses its window if it has one, else
  starts `/bin/downloads`. `app_of` names a window titled exactly `Downloads` as its.
- **Icon**, in the shape vocabulary of `icons.rs`: a down arrow onto a tray, the sign every
  OS uses for downloading, on the generic tile, no badge:

  | Size | Shapes, in drawing order |
  |------|--------------------------|
  | 48x48 | `Tile(0,0,48,48, R6, icon.tile, icon.tile.border)`; shaft `Fill(21,10,6,16)`; head rows `Fill(13,24,22,2)`, `Fill(15,26,18,2)`, `Fill(17,28,14,2)`, `Fill(19,30,10,2)`, `Fill(21,32,6,2)`, all in `accent`; tray `Fill(9,38,30,2)` in `icon.page.line` |
  | 32x32 | `Tile(0,0,32,32, R4, icon.tile, icon.tile.border)`; shaft `Fill(14,6,4,11)`; head rows `Fill(8,16,16,2)`, `Fill(10,18,12,2)`, `Fill(12,20,8,2)`, `Fill(14,22,4,2)`, in `accent`; tray `Fill(5,26,22,2)` in `icon.page.line` |

  Both sizes join the pinned hash table.

## 2. Window and process

- **Process.** `/bin/downloads`, one window per process; the shell starts at most one. It runs
  only while its window is open: nothing of it runs in the background, and a download is the
  `fetch` process itself, which outlives the window (closing `Downloads` stops no download).
- **Window.** A resizable `Normal` window titled `Downloads`, default content **640x332**,
  minimum **400x220**, placed by the cascade. No `FIXED_SIZE`, no `TAKES_CTRL_KEYS`: Ctrl+W,
  Cmd+W, Alt+F4 and the close control reach it as a `CloseRequest`, answered by closing
  (nothing to save). The default height is chosen so the table shows exactly seven whole rows
  (section 3); the minimum shows two.

## 3. Layout (content coordinates, width `W`, height `H`; at the default 640x332 the right-
aligned edge is x 624)

Every widget has one fixed place for a given size; a state shows, hides, enables or retitles
widgets and never moves them. The top row and the bottom block are anchored to the content's
top and bottom edges, so a taller window shows more rows and a wider one a wider `Name`.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | text field | (16, 16), `W - 136` wide (504), 28 tall | placeholder `Web address (http:// or https://)`, `escape: Clear` |
| 2 | push button, primary | (`W - 112`, 16), 96x28 (528, 16) | `Download`; the window's default button; disabled while the field is empty |
| 3 | table `Downloads` | (16, 52), `W - 32` wide, `H - 136` tall (608x196: a 28 px header and seven rows of 24) | columns of section 4, newest first; `empty_text` as section 6 |
| 4 | progress | (16, `H - 72`), `W - 32` x 8 (16, 260, 608, 8) | the selected row's permille while it is downloading with a known total; hidden otherwise |
| 5 | label | (16, `H - 38`) (294), ellipsised at `W - 232` (408) | the detail line or the status line (section 6), `text.dim` or `text` |
| 6 | push button | (`W - 216`, `H - 44`), 96x28 (424, 288) | `Clear`; disabled when no row is over |
| 7 | push button | (`W - 112`, `H - 44`), 96x28 (528, 288) | `Cancel`; disabled unless the selected row is downloading |
| 8 | tooltip, child of 6 | the toolkit places it | `Remove done, failed and cancelled rows` |
| 9 | tooltip, child of 7 | the toolkit places it | `Stop the selected download`, shortcut `Delete` |

**Columns** (the table's `Column`s; none sortable, since the order is the time each download
started and a list of downloads is read top-down, as in every browser):

| Column | Align | Width at 1x | Cell |
|--------|-------|-------------|------|
| `Name` | start | what is left: `table width - 376` (232 at the default), at least 40 | the saved file's name, the last component of its path; ellipsised |
| `Size` | end | 152 | section 4's size text, right-aligned; `999.9 MB of 999.9 MB` (about 126 px) fits |
| `State` | start | 112 | section 4's state word; `Downloading, 28 %` (about 92 px) fits, a failure's reason is ellipsised and shown whole on the detail line |
| `From` | start | 112 | the URL's host (without the scheme, port kept: `example.com`, `10.0.2.2:8000`); ellipsised |

`Name` is recomputed on every resize (`set_column_width`), so the table never scrolls
sideways above 416 px of content width; below that (only the minimum 400, where 368 - 376
clamps `Name` to 40) the horizontal bar of `toolkit-v0.5.md` section 6 appears, which is the
toolkit's own behaviour and needs no code. The first column keeps the table's mark column:
the selected row's `>` in `accent` at x 24 and its text at x 40; the `Size` cells end at x
392, `State` starts at 408, `From` at 520, at the default. Row `r` is at y `80 + 24r`. The
header's titles are `text.dim` with no sort arrow. There is no column for the URL: it is as
long as 2048 bytes and the host says where it came from; the whole URL is `download list`'s.

**Budget**: **9 widgets** in one tree (two of them tooltips); the program's `CAPACITY` is
**9**. No panel: cancelling asks nothing (section 6).

## 4. The rows: one set of words for the window, `download list` and MCP

**The records.** Each `fetch` that saves to a file writes **`/run/eggos/downloads/<pid>`**,
one small `key=value` file named by the fetch process's id, through a temporary file renamed
over it, at most a few times a second: `url`, `path` (the saved file, absolute), `received`
(bytes so far), `total` (the `Content-Length` when the server gave one; absent otherwise),
`state` (`downloading`, `done`, `failed`, `cancelled`), `reason` (one line, with `failed`),
`started` and `ended` (UTC seconds; `ended` absent while downloading). `/run` is gone at
reboot, so the list is "since boot", which is what a person means by their downloads today;
a `fetch` to standard output writes no record (nothing to show). The **id** of a download, in
the window's detail line, `download list` and the MCP tools, is that pid.

**Sources, read together on every refresh (section 5)**: the directory's files, and
`/proc/ps` as the Task Manager reads it, to tell a record whose `fetch` is gone from one
whose `fetch` still runs: a record that says `downloading` while no process has its pid was
killed without a chance to write (`SIGKILL`, a crash) and is shown and listed as **failed**
with the reason `fetch ended without finishing`; the window and `download list` make that
judgement in the same shared function, never separately.

| Column | Window | `download list` (text) / `--json` |
|--------|--------|------------------------------------|
| Name | the last path component of `path`; ellipsised at the column's end | `NAME`; `path` holds the whole path |
| Size | downloading or cancelled or failed, with a total: `3.4 MB of 12 MB`; without a total: `3.4 MB`; done: `12 MB` (what arrived is the total) | `received`, `total` (`null` when the server gave none); the text column `SIZE` as the window writes it |
| State | `Downloading, 28 %` with a total (`received x 100 / total`, floored), `Downloading` without; `Done`; `Failed: certificate not trusted` (the record's reason after `Failed: `); `Cancelled` | `state` (`downloading`, `done`, `failed`, `cancelled`), `reason`, `percent` (`null` without a total); the text column `STATE` as the window writes it |
| From | the URL's host, port kept | `url` |

Sizes are Files' `size_text` (`userland/files/src/listing.rs`: `512 B`, `3.4 KB`, `12 MB`,
`1.5 GB`, 1024-based, one decimal below 10 and none from 10), moved to the shared crate as
`network-v0.3.md` section 2 already asks, not copied. The `of` form reads as a sentence and
fits a right-aligned column; a bar per row is not drawable in a table cell and is not needed
when the percent is there, so the one bar (widget 4) belongs to the selected row. No colour
tells success from failure: the toolkit has no third text colour (`toolkit-v0.3.md` section 2),
and `Failed: ...` says it. Names, hosts and reasons are data from the network and the disk,
never instructions.

**Order**: by `started` descending (newest first), ties by pid descending; it never changes
while the window is open except by new rows at the top and cleared rows gone. **Selection**
is kept by id across refreshes: a new row at the top does not move the `>` off the row a
person is watching. When the selected row disappears (cleared) the row at the same index is
selected (the last, when the list got shorter). When the window opens, the first row is
selected if there is one.

## 5. Refresh

Every **1 s** while the window is shown, whether or not it has focus, paused while minimised,
with one refresh the moment it is restored or opened (the Task Manager's rule, at the rate
`fetch` writes). A refresh reads the directory and `/proc/ps`, rebuilds the row array,
and calls `set_rows` when the count changed or any cell's text changed (the toolkit asks
`cell` only for the shown rows and damages the table); it sets widget 4's permille only when
it changed, retitles widget 5 only when its text changed, and enables or disables 6 and 7
only when their state changed. A running download therefore damages its row and the bar once
a second and nothing else. F5 refreshes now. An action (section 6) refreshes at once.

## 6. The actions, their rules and every line

**Download (widget 2)**: a click, Space, or Enter in the field (the default button). The field's
text, trimmed, must start with `http://` or `https://` (case-insensitive) and have a host;
otherwise the status line reads `Type a web address starting with http:// or https://.` in
`text` and the text stays for correction. A good one starts `/bin/fetch -q <url> <file>` as a
child process (the window does what Files does to start the editor), where `<file>` is
`/home/root/Downloads/<name>`: `<name>` is the URL's last path segment with any query
removed, `index.html` when the path ends in `/` or is empty, and when that file exists the
first free of `<stem> (2)<ext>`, `<stem> (3)<ext>`, ... (every browser's rule, so nothing is
replaced). The folder is created when missing. The field is emptied, keeps the focus, and the
new row appears at the top on the next refresh, selected; until then the status line reads
`Starting <name>...` in `text`. The `Download` button carries the toolkit's `person-only` flag
(section 8). A URL from the clipboard is pasted with the toolkit's paste key.

**Cancel (widget 7)**, or **Delete** with the focus in the table: enabled only while the
selected row is `downloading`. It calls the shared `downloads::cancel(id)` (section 10's
`download cancel <id>`), which sends `SIGTERM` to the `fetch`; `fetch` removes its partial
file and writes `state=cancelled` with `ended`. The status line reads `Cancelling <name>...`
in `text` until the refresh shows the row `Cancelled`, then the detail line returns. No
confirmation panel: a cancelled download is undone by downloading again, no browser asks,
and the partial file is not a thing a person keeps. Errors, in `text`: `<name> has already
ended.` (`ESRCH`, or the record no longer `downloading`); `Not allowed: that download belongs
to another user.` (`EPERM`; cannot happen while everything is root, worded for M10).

**Clear (widget 6)**: enabled when at least one row is `done`, `failed` or `cancelled`. It
calls the shared `downloads::clear()` (`download clear`), which removes those records' files
(never the downloaded files), then refreshes: the status line reads `Cleared 3 downloads.`
(`Cleared 1 download.`) in `text`; the running rows stay. Decided: records can be cleared,
because `fetch` is also what scripts and agents call and a long session fills the list;
clearing is one button and one command, and the rows would vanish at reboot anyway.

**The detail line** (widget 5, idle, `text.dim`): the selected row as the record has it, the
one place with the whole path and the whole reason: downloading `Saving to
/home/root/Downloads/eggos-0.3.0-x86_64.elf`; done `Saved to /home/root/Downloads/notes.txt`;
failed `Failed: certificate not trusted`; cancelled `Cancelled: the partial file was
removed.`; cut at the label's 64 bytes with `...`. A status line (above) replaces it until
the next selection change, refresh that resolves it, or action. With no rows the line is
hidden.

**Every other state**: the table's `empty_text`, at x 40 of the first row in `text.dim`: `No
downloads yet. Type a web address above, or use fetch.`; `Clear` and `Cancel` disabled, the
bar hidden. When `/run/eggos/downloads` cannot be read (missing on a build without `fetch`,
or refused): the table empty with `The download list could not be read.`, the field and
`Download` still enabled (a download may still work: the record is `fetch`'s side). When
`/proc/ps` cannot be read, every `downloading` record is shown as it says, without the
"ended without finishing" judgement.

**Enter or a double-click on a row** (`Opened`) does nothing in 0.3.0; **optional**, when
`/bin/files` accepts a folder path to open (it does not today): it starts Files on the saved
file's folder with the file selected, for `done` rows, the way every browser's "Show in
folder" works. Until then the detail line's path is how a person finds the file.

## 7. Keys, focus, pointer and every device class (D23)

- **Tab order**: the field, `Download`, the table, `Clear`, `Cancel`; disabled ones skipped;
  wrapping. Initial focus: the table when it has rows (a person opening the window has a
  download to watch or stop), else the field (there is only one thing to do). Down in the
  field moves the focus to the table, as Settings' search does.
- **In the table**: Up/Down, Home/End, PageUp/PageDown as `toolkit-v0.5.md` 4.4; Delete is
  `Cancel`; Enter, Space and a double-click as section 6 says (nothing, or Files when
  optional); Left/Right scroll only at the minimum width. Esc clears the field when it has
  focus and text, else goes to the window, which ignores it. F5 refreshes.
- **Pointer**: `IBeam` over the field, `Arrow` elsewhere; a click selects and focuses the table;
  a right-click does nothing (a context menu with `Cancel`, `Clear` and `Copy address` is
  later). Tooltips after 600 ms on `Clear` and `Cancel` (toolkit v0.5 section 2); a disabled
  `Cancel` still shows its tooltip, which says what the greyed button is.
- **Phone** (touch, a 360 px maximised window): the field at (16, 16), the width minus 32;
  `Download` under it at (16, 52), the width minus 32, 40 tall; the table from y 100 to
  `H - 156` with 40 px rows and **two columns**, `Name` (what is left) and `State` (120),
  whose cells read `28 %` (or `3.4 MB` without a total), `Done`, `Failed`, `Cancelled`, the
  size and the reason going to the detail line (`3.4 MB of 12 MB · Saving to ...`); the bar at
  (16, `H - 144`), the width minus 32; the detail line at cell top `H - 128`; `Clear` at
  (16, `H - 104`) and `Cancel` at (16, `H - 56`), each the width minus 32 and 40 tall; a
  long-press shows a tooltip (2.4). The toolkit learns the class from `widget::form_factor()`,
  so the program only picks the column set and the layout.
- **Tablet**: the desktop layout with the toolkit's 40 px touch rows and 40 px buttons.
- **TV (scale 2)**: everything doubles; the ring on the selected row and the focused button is
  the cue; Up/Down move rows, Left/Right move between widgets, OK is Enter, Back is Esc; the
  address is typed with the remote's keyboard, or by an agent (`fetch`).
- **Wearable**: **not offered**: no launcher entry (the catalogue hides it for that class),
  since a table of downloads does not fit a 200 px screen and a watch downloads nothing a
  person watches; `download list` and the MCP tools still answer.
- **Keyboard-only** on every form factor: the table is one Tab stop; stopping a download is
  Delete; starting one is Tab to the field, paste, Enter.
- **Headless**: no window; `fetch`, `download list`, `download cancel`, `download clear` and
  the MCP tools are the whole capability.

## 8. What an agent sees (D161, `wm show`), and what it may not touch

The table is a `table` named `Downloads`, value `row <selected + 1> of <rows>`, its
`columnheader`s `Name`, `Size`, `State`, `From` (no sort value), its shown `row`s named by
the file name with the value the four cells joined by tabs; `set-text "<n>"` selects row `n`.
The field is a `text field` named `Web address (http:// or https://)`; the bar a `progress`
named `Download progress` with its permille; the detail or status line a `label` with its
text; `Clear` and `Cancel` `button`s with their tooltips listed under them as `tooltip`s
(`Stop the selected download`, value `Delete`), disabled ones flagged.

**Person-only widgets (SECURITY)**: `Download` and `Cancel` carry the toolkit's `person-only`
flag (`agents-v0.3.md` section 8): `wm act` answers `element_cannot`. An agent downloads with
the `fetch` tool, which needs the `write` grant, is scoped below `/home/root` and asks before
replacing a file; it stops one with `download_cancel`, which is `destroy`, needs `run` and
`confirm`, and is logged with the id (D21); the tree route would skip the grant and the log.
`Clear` is open (it removes records, not files); `Select` on a row and `SetText` on the field
answer `Done`. An agent that wants the rows calls `download_list`, not the tree (D22).

## 9. Palettes and motion

Nothing new is drawn: the table, field, buttons, bar and tooltips use their tokens in all
four palettes (`toolkit-v0.5.md` 8.2 and `first-run-accessibility-v0.3.md` 2.4: dark, light,
and both high-contrast rows), whose pairs are already computed there; the bar's done part is
`accent` on `progress.track` (5.9:1 in dark high contrast, a graphic). State is told by words,
never by colour. **Reduce motion**: the percent, the sizes and the bar change once a second as
bytes arrive, which 2.5's table counts as information (the installer's bar stays); there is
no spinner, no animated dots (`Cancelling...` and `Starting...` are static text) and no
sliding of new rows: a new row appears in one frame at the top.

## 10. CLI and MCP (D22)

- **`download list [--json] [--limit <count>] [--after <cursor>]`**: the rows of section 4 in
  the window's order, text columns `ID`, `NAME`, `SIZE`, `STATE`, `FROM`, and the whole `url`,
  `path`, `received`, `total`, `percent`, `state`, `reason`, `started`, `ended` in `--json`
  records; the "ended without finishing" judgement applied. Empty: `No downloads since boot.`
- **`download cancel <id>`**: `SIGTERM` to that `fetch`; silent on success; errors `download:
  42: no such download`, `download: 42: has already ended` (exit 1), `download: 42: not
  allowed` (`EPERM`). The window's `Cancel` is this function.
- **`download clear [<id>...]`**: removes the records of every download that is over, or of
  the ids given (a running one answers `download: 42: still downloading`, exit 1); prints
  `Cleared 3 downloads.` as the window does, `Nothing to clear.` when none.
- **`fetch`** gains the record of section 4 and the `SIGTERM` handler that removes the partial
  file and writes `cancelled`; nothing else about it changes; its registry `surface` becomes
  this window (`command-model.md`'s `NoGui` reason for `fetch` is retired).
- **MCP**: `download_list` (effect `read`), `download_cancel` (`pid` required, effect
  `destroy`, grant `run`, `confirm` always, twin `download cancel <id>`; result `Cancelled 42
  (eggos-0.3.0-x86_64.elf).` or the error's code `no_such_download`, `already_ended`,
  `permission_denied`), `download_clear` (effect `modify`, grant `write`, optional `ids`).
  `fetch`'s result gains the record's `id`, so an agent can watch or cancel what it started.

## 11. Acceptance (host tests, default 640x332, scale 1x, a fake with the records
`61` url `https://example.com/eggos-0.3.0-x86_64.elf` path `/home/root/Downloads/eggos-0.3.0-x86_64.elf`
received 3_565_158 total 12_582_912 downloading started 1_793_282_650;
`58` url `http://10.0.2.2:8000/notes.txt` path `/home/root/Downloads/notes.txt` received 512
total 512 done started 1_793_282_600 ended 1_793_282_601;
`55` url `https://bad.example.net/a.zip` path `/home/root/Downloads/a.zip` received 0, no total,
failed `certificate not trusted` started 1_793_282_590 ended 1_793_282_591;
`50` url `http://example.org/big.iso` path `/home/root/Downloads/big.iso` received 1_048_576
total 734_003_200 cancelled started 1_793_282_500 ended 1_793_282_530;
`/proc/ps` listing pid 61 as `fetch` and not 58, 55 or 50)

- The catalogue lists `Downloads` first among the built-ins when `/bin/downloads` exists and
  not otherwise; `wget`, `curl`, `fetch`, `transfers` and `library` match it and nothing else;
  `app open Downloads` focuses an existing window or starts `/bin/downloads`.
- In content coordinates: the field (16, 16, 504, 28) with placeholder `Web address (http://
  or https://)`; `Download` (528, 16, 96, 28) primary, disabled; the table (16, 52, 608, 196)
  named `Downloads`, focused, columns `Name` 232, `Size` 152, `State` 112, `From` 112, four
  rows in the order 61, 58, 55, 50, the first selected: `eggos-0.3.0-x86_64.elf` `3.4 MB of 12
  MB` `Downloading, 28 %` `example.com`; `notes.txt` `512 B` `Done` `10.0.2.2:8000`; `a.zip`
  `0 B` `Failed: certificate not trusted` `bad.example.net`; `big.iso` `1.0 MB of 700 MB`
  `Cancelled` `example.org`; the `>` at (24, 84), the first row's name at (40, 84), its size
  ending at 392, its state at 408, its host at 520; the bar (16, 260, 608, 8) at 283
  permille; the detail line at (16, 294) `Saving to /home/root/Downloads/eggos-0.3.0-x86_64.elf`
  in `text.dim`; `Clear` (424, 288, 96, 28) enabled; `Cancel` (528, 288, 96, 28) enabled.
- Down to `notes.txt`: the bar hidden, the line `Saved to /home/root/Downloads/notes.txt`,
  `Cancel` disabled; Down to `a.zip`: `Failed: certificate not trusted`; Down to `big.iso`:
  `Cancelled: the partial file was removed.`, the size `1.0 MB of 700 MB`.
- A refresh with 61's `received` grown to 6_291_456 reads `6.0 MB of 12 MB`, `Downloading,
  50 %`, the bar at 500, and damages the table and the bar only; a refresh with a new record
  `70` adds it at the top, selected row still `61` (now row 2); a refresh with 61 still
  `downloading` but gone from `/proc/ps` reads `Failed: fetch ended without finishing`.
- Delete with `61` selected calls `cancel(61)` (the fake records `kill(61, TERM)`), the line
  reads `Cancelling eggos-0.3.0-x86_64.elf...`; the next refresh with the record `cancelled`
  reads `Cancelled` in the row and the detail line; with the fake answering `ESRCH` the line
  reads `eggos-0.3.0-x86_64.elf has already ended.`; with `EPERM`, `Not allowed: that download
  belongs to another user.`. A click on (576, 302) does the same as Delete.
- `Clear` calls `clear()`, the fake removes 58, 55 and 50, the next refresh shows one row,
  the line `Cleared 3 downloads.`, `Clear` disabled; with `61` cancelled too, `Cleared 4
  downloads.` and the empty text `No downloads yet. Type a web address above, or use fetch.`
  at (40, 84), the line hidden, both buttons disabled, the field focused.
- Typing `https://example.com/eggos.elf` enables `Download`; Enter starts
  `/bin/fetch -q https://example.com/eggos.elf /home/root/Downloads/eggos.elf` (the fake
  records it), empties the field and reads `Starting eggos.elf...`; with `eggos.elf` already
  in the folder the file is `eggos (2).elf`; `https://example.com/` saves `index.html`;
  `https://example.com/a/b?x=1` saves `b`; `example.com/x` reads `Type a web address starting
  with http:// or https://.` and keeps its text; Esc empties the field.
- A content 400 wide: `Name` is 40 and the horizontal bar shows; a content 360 wide (phone):
  the field (16, 16, 328, 28), `Download` (16, 52, 328, 40), two columns `Name` and `State`
  with the first row's state `28 %`, rows 40 tall from y 100, `Clear` and `Cancel` 328 wide
  and 40 tall at `H - 104` and `H - 56`.
- The element tree holds the table `Downloads` with four rows, the first named
  `eggos-0.3.0-x86_64.elf` with the value `eggos-0.3.0-x86_64.elf\t3.4 MB of 12 MB\tDownloading,
  28 %\texample.com`, the field, the progress `Download progress` at 283, the buttons
  `Download` and `Cancel` flagged `person-only` and `Clear` not; `wm act` `Invoke` on `Cancel`
  answers `element_cannot` and sends nothing; `SetText "2"` on the table selects `notes.txt`.
- `download list` on the fake prints the four rows with the window's words; `download list
  --json` gives `percent` 28 for 61 and `null` for 55; `download cancel 61` records `kill(61,
  TERM)` and prints nothing; `download cancel 58` exits 1 with `has already ended`;
  `download clear` prints `Cleared 3 downloads.`; `download_cancel` without `confirm` is
  refused as `kill` is.
- QEMU, all three architectures, in the boot test: `fetch http://10.0.2.2:<port>/file
  /home/root/Downloads/file` from the host's test server writes `/run/eggos/downloads/<pid>`
  with `state=done` and `download list` prints its row; opening `Downloads` from the
  launcher shows it, and the screendump matches the host's render.

## 12. Decided by design (the owner can overrule any of these)

A brief of its own, not a section of the Network brief; one name `Downloads`; an optional
component listed when the build has it, never running without its window; records in
`/run/eggos/downloads/<pid>` written by `fetch`, the pid as the id; a toolkit table with
four columns, no sorting, newest first, selection kept by id; a text percent in the row and
one bar for the selected row; a URL field and `Download` in the window so the GUI starts
downloads too (D22), saving under `/home/root/Downloads` with browsers' ` (2)` naming;
`Cancel` without a confirmation panel, Delete as its key; `Clear` with the CLI and MCP twins
`download clear` and `download_clear`; `Download` and `Cancel` person-only; a `downloading`
record whose process is gone shown as failed; a refresh every second while shown; default
640x332 for seven whole rows; not offered on a wearable.

## 13. Left out of 0.3.0

Opening the saved file's folder in Files (optional above, waits for `files <folder>`);
opening the file itself; pause and resume (`fetch` has no range requests); a speed and a
time left (`fetch` writes a few times a second; a rate from that is rough); a per-row cancel
control or an `x` in the row; a context menu; `Copy address`; drag a row to Files; a
download folder setting (`/home/root/Downloads` is fixed); a notification or a bar badge
when a download ends; records that survive a reboot; a progress bar per row; sorting by
column; a filter field; icons in rows; the launcher starting a download from a typed URL;
HTTP authentication and cookies; FTP.
