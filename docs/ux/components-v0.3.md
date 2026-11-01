# EggOS v0.3.0 Components: design brief (batch part 20)

Status: design for v0.3, D237.

Design by Claude Fable (D16) for the GUI front-end of the `component` capability (D18, D22,
D237): the Settings `Components` pane in its **minimal** first version, one list of the
components the component store offers and the machine has installed, each with an `Install` or
`Remove` button, with the same data and words as `component list`, `component install <name>`,
`component remove [--yes] [--dry-run] <name>` and the MCP tools `component_list`,
`component_install`, `component_remove`. Everything in `toolkit-v0.3.md` (section 11 for
Settings), `toolkit-v0.5.md` (palettes, device classes), `desktop-v0.3-gui.md` section 10,
`storage-v0.3.md` and `network-v0.3.md` (the panes this one is shaped like) and `agents-v0.3.md`
section 5 (the grant prompt) stands; numbers are px at 1x and double at 2x; the only motion is
the progress bar following bytes. A later part (65) makes the full Components pane with the
installer's flow; this brief keeps to the list and leaves the rest in "Later".

**The store.** Components come from a read-only disk the run scripts attach,
`eggos-<version>-<arch>-components.img` (`run-eggos.sh` and `run-eggos.ps1` attach it
automatically next to the kernel image), or from the same file put in the host share
(`/media/host`). The store carries an index (each component's `name`, `title`, `summary`, its
file, its installed size, its checksum, what it `needs` and its one-sentence `on_remove`
note) and one file per component, `<name>.component`. This version's store lists four, in the
index's order, which puts a component after what it needs:

| `name` | Title | Summary | Size | Needs | `on_remove` |
|--------|-------|---------|------|-------|-------------|
| `desktop` | `Desktop` | `The window server, the shell and Settings` | `2.6 MB` | nothing | `Settings and every window go with it.` |
| `builtin-apps` | `Built-in apps` | `Files, Text Editor, Downloads, Task Manager` | `1.1 MB` | `desktop` | `They leave the launcher; your files stay.` |
| `windows-apps` | `Windows apps` | `Runs Windows programs natively, no emulation` | `1.8 MB` | nothing | `Installed Windows programs stay in Apps and need it to run.` |
| `mac-apps` | `Mac apps` | `Runs Mac programs natively, no emulation` | `1.4 MB` | nothing | `Installed Mac programs stay in Apps and need it to run.` |

Sizes are examples for the numbers' shapes; the index has the real ones. The pane is built for
1 to 6 rows (`MAX_COMPONENTS` 6); the index is the only source of titles, summaries, sizes
and dependencies, so the pane holds no component's name in its code (D18: nothing assumes a
component exists).

## 0. The five states, named once

One vocabulary for the three front-ends; the GUI never invents a sixth word.

| State | When | Pane (section 2) | `component list` `STATE` | `--json` `state` | Button |
|-------|------|------------------|--------------------------|------------------|--------|
| not installed | in the store's index, not on the machine | `Not installed · 1.8 MB` | `not-installed` | `"not-installed"` | `Install` |
| installed | on the machine (the manager's record, `/var/lib/eggos/components`) | `Installed · 1.8 MB` | `installed` | `"installed"` | `Remove` |
| installing | the copy from the store is running | `Installing 42 %` | `installing 42 %` | `"installing"`, `percent` 42 | `Cancel` |
| removing | the files are being deleted | `Removing...` | `removing` | `"removing"` | `Remove`, disabled |
| failed | the last install of it failed; nothing was installed | `Failed` | `not-installed` | `"not-installed"` | `Install` |

`Failed` is the pane's memory of the last action, not a state of the machine: an install that
fails leaves nothing installed (section 5), so `component list` says `not-installed`, and the
pane says `Failed` with the reason under the row until the next action on that row or the
pane being shown again. The percent is `received x 100 / total`, floored, written `42 %` with
a space as `downloads-v0.3.md` section 4 writes it. Three periods, not U+2026. The `·` is
U+00B7. Sizes everywhere are Files' `size_text` (`1.8 MB`, `712 KB`).

## 1. The pane in the sidebar

- **Name** `Components` (already in `PANES`, index 9, between `Agents` and `About`:
  `Appearance`, `Network`, `Display`, `Keyboard`, `Mouse`, `Accessibility`, `Storage`, `Date &
  time`, `Agents`, `Components`, `About`). Its row is y 332..363 (44 + 9 x 32), text at x 16.
  Eleven rows are 352 px: they fit the list's 396 at the default 440 and scroll at the 360
  minimum, as the Accessibility brief foresaw. Not `Apps` (that is the launcher's catalogue and
  Files' place), not `Optional features` (Windows' name for a different list): a switcher who
  types `features`, `programs` or `software` finds it by the search words.
- **Search words**: `component`, `components`, `install`, `uninstall`, `remove`, `add or
  remove`, `Add/Remove Programs`, `Programs and Features`, `Apps & features`, `Optional
  features`, `Windows features`, `Software`, `store`, `component store`, `layer`, `layers`,
  `compatibility`, `Windows apps`, `Mac apps`, `Built-in apps`, `Desktop`, `size`, `disk
  space`, plus its button names `Install`, `Remove`. `install` also matches Agents (its brief
  says so); `desktop` matches nothing else; `size` and `space` also match Storage. Nothing
  else collides.
- **Budget**: the tree holds 211 widgets (`settings_window::CAPACITY`). This pane is its
  container plus exactly **39** widgets (4 at the top, 5 per row for 6 rows, 5 in the shared
  note, section 2), so `CAPACITY` becomes **250** (the installer's `eggos_app::dialog::CAPACITY`
  stays equal to it). The count is this small because the note under a row (a message, a
  progress bar or a confirmation) is one set of five widgets placed under whichever row is
  acting, never one set per row: the manager does one thing at a time (section 4).

## 2. Layout (pane coordinates: origin window x 176, y 0; 464 wide at the default size, so the
right-aligned edge is x 448; usable height 332 at the minimum window, 412 at the default)

Every widget has one fixed place, with one exception this brief is the first to use: when the
note (widgets 35-39) opens under a row, the rows below it move down by the note's height
through `WidgetTree::set_place` (D224, made for windows whose widgets change places with
their size), and move back when it closes. A state otherwise shows, hides, enables or retitles
widgets and never moves them.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Components` |
| 2 | label | (16, 56) | the store line (section 3) in `text` |
| 3 | label | (16, 76) | the store's detail line in `text.dim` |
| 4 | label | (16, 96) | the count line or the store hint in `text.dim` |
| 5 + 5r | panel, row `r` (0..5) | (16, 124 + 48 r), 432x48, fill `menu.bg` (the pane's surface, no border) | holds its row's four widgets; hidden with them for `r` at or beyond the component count |
| 6 + 5r | label | (24, 6) in the panel | the title, `Windows apps`, in `text`; ellipsised 16 px before widget 7's left edge |
| 7 + 5r | label, right-aligned | ends at x 320 of the panel, cell top 6 | the state (section 0) in `text`; `Failed` in `text` too (the reason under the row says what happened; no colour-only cue) |
| 8 + 5r | label | (24, 26) in the panel | the summary in `text.dim`, ellipsised at x 320 of the panel |
| 9 + 5r | push button | (336, 10) in the panel, 96x28 | `Install`, `Remove` or `Cancel` (section 0) |
| 35 | label | (40, note + 4) | the note's first line in `text` |
| 36 | label | (40, note + 24) | the note's second line in `text.dim`; hidden when there is none |
| 37 | progress | (40, note + 8), 408x8 | the install's permille; shown only while installing |
| 38 | push button | (216, note + 48), 96x28 | `Cancel`; shown only in a confirmation |
| 39 | push button, primary | (320, note + 48), 128x28 | `Remove`, `Install both` or `Install all`; the window's default button while shown |

Row `r`'s panel spans x 16..448, so its button at panel x 336 ends at the pane's right edge
448 and its texts start at pane x 40, the list text column of every pane (a 24 px mark
column kept empty: the rows have no selection, section 6). The state label ends 16 px before
the button. The widest idle state, `Not installed · 999.9 MB`, is about 170 px, so a title
has at least 130 px (`Built-in apps`, the longest, is about 95); the summary has 280 px and
the longest above, `Runs Windows programs natively, no emulation`, is about 275 (the index
keeps summaries under 44 characters; longer ones ellipsise and `component list --json` has
the whole text). Four rows end at y 316, inside 332: nothing scrolls at the minimum window
while no note is open. `note` is the acting row's bottom, `124 + 48 (r + 1)`; the note is 24
tall for a one-line message (widget 35 alone) and 44 for a two-line one (35 and 36), 40 for
progress (37 and 35: the bar at +8, the bytes line at cell top +24), 84 for a confirmation
(35, 36, 38, 39). At the minimum window a
confirmation under the fourth row ends at y 400, so the pane's panel grows and scrolls in the
window's existing scroll view (wheel 72 px, the focused widget kept in view); at the default
440 it ends at 400 of 412 and nothing scrolls. The 128 px wide confirm button fits `Install
both` (about 88 px of text and 32 of padding) and `Install all`; `Remove` is centred in it.

## 3. The store line and the rows: one set of words for the pane, the CLI and MCP

The program reads the store and the manager's record each time the pane is shown and after
every action (and once a second while a row is installing or removing, for the percent),
through the same function `component list` prints and `component_list` returns. The record
holds, per component, `name`, `title`, `summary`, `state`, `percent` (`null` unless
installing), `size_bytes`, `needs` (names), `needed_by` (the **installed** components that
need it), `in_use` (the running programs that need it: a count and up to three names) and
`file`; and once, `store`: `null`, or `file`, `version`, `arch` and `source` (`"disk"` with
`device`, or `"share"`).

**Widgets 2-4** by the store's state:

| Store | Widget 2 (`text`) | Widget 3 (`text.dim`) | Widget 4 (`text.dim`) |
|-------|-------------------|-----------------------|-----------------------|
| attached as a disk | `Component store: EggOS 0.3.0, x86_64` | `eggos-0.3.0-x86_64-components.img on /dev/vdc` | `2 of 4 installed · 3.7 MB on disk` |
| in the host share | the same | `eggos-0.3.0-x86_64-components.img in the host share` | the same |
| none | `No component store is attached.` | `Attach the store disk, or put the .img in the host share.` | `run-eggos.sh and run-eggos.ps1 attach it automatically.` |

`x86_64` is the machine's architecture word as About prints it. The count line counts the
rows shown and sums the installed sizes (`1 of 4 installed · 2.6 MB on disk`, `Nothing
installed · 0 B on disk`). The longest line, the no-store hint, is about 400 px of the 432.

**The rows** are the store's index in its order, plus any installed component the index
lacks (an older store), after them, in the record's order. **Without a store** the list is
the installed components alone (the manager's record is the only source then), `Remove`
working as ever and no `Install` anywhere; with nothing installed either, widgets 5-34 are
hidden and widgets 2-4 keep the three no-store lines: the first says why the list is empty
and the other two say what to do, so no fourth sentence is needed. The store's state is
checked on every read: a store that is detached while the pane shows (the disk pulled, the
share unmounted) turns widgets 2-4 into the no-store lines and hides the not-installed rows
on the next read; one that appears adds them.

**The CLI table.** `component list` prints one line a component in the pane's order,
`NAME  STATE  SIZE  TITLE`, with section 0's words; `--json` the records. The store goes on
**standard error** as the list commands' page line does (`command-model.md`, "Pages"): `store:
eggos-0.3.0-x86_64-components.img (EggOS 0.3.0, x86_64) on /dev/vdc`, `store: ... in the host
share`, or `store: none. Attach the store disk, or put eggos-0.3.0-x86_64-components.img in
the host share; run-eggos.sh attaches it automatically.`; with `--json`, `{"store": {...}}` or
`{"store": null}` there. Standard output stays records; the exit status is 0 either way (no
store is a fact, not an error, until someone tries to install).

```
NAME          STATE          SIZE    TITLE
desktop       installed      2.6 MB  Desktop
builtin-apps  installed      1.1 MB  Built-in apps
windows-apps  installing 42 %  1.8 MB  Windows apps
mac-apps      not-installed  1.4 MB  Mac apps
```

(`installing 42 %` widens its column while it lasts; `--json` is for scripts.) `component_list`
returns `{"store": ..., "items": [records]}`, the list tools' shape.

## 4. The actions, their rules and every word

One action at a time: the manager installs or removes one component at a time, and the pane
disables every other row's button while one is installing or removing (they keep their
titles, `text.dim`), so the note under the acting row is the only note. Pressing a row's
button (or Enter or Space on it) does, by state:

**`Install` (not installed, or failed), the store attached.**

- Nothing missing: the row's state becomes `Installing 0 %`, its button `Cancel`, and the
  note opens with the bar (widget 37, from 0) and widget 35 reading `0 B of 1.8 MB` in
  `text`, updated on every read while it runs, the bar's and the label's rects damaged only.
  The frame is committed before the copy starts; the sidebar still works. When it is done the
  note closes, the rows are re-read, the state reads `Installed · 1.8 MB`, the button
  `Remove`, and widget 4's count line updates. A 2 MB copy from a virtio disk takes under a
  second on QEMU: the bar is for the host share over 9p and for real hardware, where it is
  slower.
- **A dependency missing** (the component's `needs` lists one not installed): nothing is
  installed yet; the note opens as a confirmation under the row: widget 35 `Installs Desktop
  (2.6 MB) too: Built-in apps need it.`, widget 36 `3.7 MB in all.`, `Cancel` and `Install
  both` (primary). Two or more missing: `Installs Desktop (2.6 MB) and Mac apps (1.4 MB) too:
  X needs them.` and `Install all`; the list is ellipsised at the line's end and the CLI's
  `--dry-run` has it whole. `Install both` installs them in the index's order, the needed one
  first: the note moves to the Desktop row with its bar, then to the Built-in apps row;
  `Cancel`, Esc or closing the window installs nothing. The confirmation opens with the focus
  on `Install both`, as the remove confirmation opens on `Remove` (below). A dependency that is itself
  installing (another row is busy) is covered by the one-at-a-time rule above: every other
  `Install` is disabled until it ends.
- **`Cancel`** while installing (the button, or Delete on it): the copy stops, the partial
  file is deleted, the state goes back to `Not installed · 1.8 MB` and the note under that
  row reads `Cancelled.` with `Nothing was installed.` under it. Cancelling the first of two
  leaves the second not installed too (the second never started).

**`Remove` (installed).** Two refusals are checked first, in this order, and shown as a
message note under the row in `text` (the toolkit's inline line, not a dialog: a refusal is
information, and the person stays where they were):

| Refusal | Widget 35 | When |
|---------|-----------|------|
| needed | `Needed by Built-in apps: remove that first.` (`Needed by Built-in apps and Mac apps: remove those first.`) | an installed component lists it in `needs` |
| in use | `In use: Notepad is running. Quit it, then remove.`; several: `In use: 3 Windows programs are running. Quit them, then remove.` (`Mac programs`; `Text Editor is running`; `Files and Text Editor are running`) | a program that needs the component is running (a layer's process, one of the built-in apps) |
| in use, the desktop | widget 35 `In use: the desktop is running, and this window is part of it.`, widget 36 `Boot with run-eggos.sh --serial and remove it from there.` | always, from the pane: Settings is drawn by the desktop |

The button stays enabled before the press: whether something is in use changes from one
second to the next, and a greyed button with no reason is what switchers hate most about
every "Uninstall" list. The press answers, exactly as the CLI's `component remove` answers,
and the message stays until the next action on any row, a state change on that row, or the
pane being shown again.

Neither refusal: **confirmation**, inline, because removing is `destroy`
(`command-model.md`): widget 35 `Remove Windows apps (1.8 MB)?`, widget 36 the index's
`on_remove` sentence (`Installed Windows programs stay in Apps and need it to run.`), `Cancel`
and `Remove` (primary, the default button: a component is 2 MB from a store that is attached,
so a slip costs a reinstall, which is why this is a button and not Storage's typed name).
When the confirmation opens the focus moves to its primary button (as the grant prompt opens
on its default, `agents-v0.3.md` section 5), so Enter or Space confirms, Shift+Tab reaches
`Cancel` and Esc cancels. `Remove`: the note closes (the state label says what is happening,
so no second line repeats it), the row's state reads `Removing...`, its button is disabled,
the files are deleted and the manager's record is written last; then the rows are re-read,
the state reads `Not installed · 1.8 MB`, the button `Install`, the count line updates.
`Cancel`, Esc or closing the window removes nothing and returns the focus to the row's
`Remove`. The CLI asks the same question (section 7).

**Failures** (a two-line message note: widget 35 in `text` says what happened, widget 36 in
`text.dim` what it means; the state `Failed`, the button `Install` again). Every install
failure leaves nothing installed: the component is copied to a temporary name, checked, and
only then moved into place and recorded, so a failure at any step is a delete of the
temporary file. A label holds 64 bytes, which is why the pane says it in two lines where the
CLI says it in one (`...; nothing was installed.`, section 7):

| Failure | Widget 35 | Widget 36 |
|---------|-----------|-----------|
| checksum mismatch | `The store's copy of windows-apps.component is damaged.` | `Nothing was installed.` |
| the store was detached (the disk pulled, the share unmounted, a read answered `EIO` or `ENOENT` mid-copy) | `The store was detached before the copy finished.` | `Nothing was installed.` |
| disk full (`ENOSPC`) | `Not enough space: 1.8 MB needed, 0.6 MB free.` | `Nothing was installed.` |
| anything else | `Could not install: <errno text>.` | `Nothing was installed.` |
| a remove that fails | `Could not remove Windows apps: <errno text>.` (the state back to `Installed · 1.8 MB`) | `It stays installed.` |
| not root (`EPERM`) | `Not allowed: only root installs or removes components.` | hidden |

The free space in the disk-full line is the volume the components live on, the same number
Files' status bar shows for it (`files-free-space-v0.3.md`). When the first of two fails, the
second is not attempted and its row stays `Not installed`; the message sits under the row that
failed. A remove that fails midway is finished by the manager at the next boot (the record is
written last, so an unrecorded half-removal is retried), and the pane shows `Installed` until
then: the honest word for a component the record still lists.

## 5. The note's forms, drawn

Three forms, one place (under the acting row, full width from x 40 to 448), `menu.bg` under
them as everywhere in the pane: no box, no border, the indent to x 40 is what ties the lines
to the row above.

- **Message**: widget 35 at cell top +4; 24 tall. With a second line (the failures, the
  cancel, the desktop's in-use reason), widget 36 at +24; 44 tall.
- **Progress**: the bar at +8, 408x8, `progress.track` with the done part `accent`, radius 4
  (`INSET_R4`, `desktop-v0.3-gui.md` section 9), then widget 35 at cell top +24, `0.8 MB of
  1.8 MB`; 40 tall. The bar is determinate because the store's index gives the size; a
  component whose size the index lacks (never, in this store) would show the bar hidden and
  the bytes alone.
- **Confirmation**: widgets 35 and 36 at +4 and +24, the two buttons at +48, 8 apart, the
  primary one last (toolkit section 4's dialog row), ending at x 448; 84 tall.

## 6. Keys, focus and pointer

- **Tab** from the sidebar list enters the pane at the first row's button, then each row's
  button in order (disabled ones skipped, so while a row installs Tab goes to its `Cancel`
  alone), then the note's `Cancel` and its primary button when a confirmation shows, then
  wraps to the search field; Shift+Tab the reverse. Showing the pane does not move the focus.
- **Up/Down** on a row's button move the focus to the previous or next row's button (the
  pane answers them, as Downloads' field answers Down; the toolkit's buttons do nothing with
  arrows themselves); Home/End to the first and last row's. The rows are not a list and have
  no selection: a row's one action is its button, so the button is the row's focus, and a
  selection mark with nothing to open would be a lie.
- **Enter and Space** press the focused button; Enter elsewhere in the pane presses the
  confirmation's primary button while one shows and nothing otherwise (no default button
  while the note is not a confirmation, so Enter in the search field never installs
  anything). A confirmation opens with the focus on its primary button (section 4), so the
  whole flow is Enter, Enter, or Enter, Esc. **Delete** on an installing row's `Cancel`
  presses it. **Esc** closes an open confirmation (nothing happens, the focus returns to the
  row's button); otherwise it goes to the window, which ignores it.
- **The ring**: the toolkit's, 2 px `focus.ring` at inset 2 on the focused button (`icon.dark`
  on the primary one); nothing else in the pane is focusable.
- **Pointer**: `Arrow` everywhere; a row's panel has no hover or press look (nothing to
  select); a right-click does nothing. A tooltip after 600 ms on a disabled row button, in
  the words of the reason: `Another component is installing` (`is being removed`), so a greyed
  button still explains itself (toolkit v0.5 section 2).

## 7. The CLI and MCP mirror (D22): the same words from one table

The sentences of sections 3 and 4 live in one function per message, shared by the pane, the
command and the tool. Codes follow `command-model.md` (`<what happened>: <why>. <what to
do>`, `try:` where one command fixes it).

**`component install <name> [--dry-run]`**, effect `change` (it adds, it destroys nothing,
so it never asks; a missing dependency is said, then done, because the person asked for the
component and the dependency is part of what that means):

| Case | Standard output / error | Code, category, exit |
|------|-------------------------|----------------------|
| plain | `Installing Windows apps (1.8 MB)...` then `Installed Windows apps (1.8 MB).` | 0 |
| dependency | `Installs Desktop (2.6 MB) too: Built-in apps need it. 3.7 MB in all.`, then `Installing Desktop (2.6 MB)...`, `Installed Desktop (2.6 MB).`, `Installing Built-in apps (1.1 MB)...`, `Installed Built-in apps (1.1 MB).` | 0 |
| `--dry-run` | `would install Desktop (2.6 MB) and Built-in apps (1.1 MB): 3.7 MB in all` | 0 |
| already installed | `Windows apps is already installed.` | 0 (idempotent) |
| no store | `No component store is attached: attach the store disk, or put eggos-0.3.0-x86_64-components.img in the host share. run-eggos.sh attaches it automatically.` | `no_store`, not_found, 1 |
| unknown name | `No component named "winapps": component list names them.` `  try: component list` | `no_such_component`, not_found, 1 |
| another in progress | `Busy: Desktop is installing. Wait for it, then try again.` (`is being removed`) | `component_busy`, busy, 1 |
| checksum | `The store's copy of windows-apps.component is damaged; nothing was installed.` (the pane's two lines joined with `; ` and the second lower-cased, one function) | `store_damaged`, damaged, 1 |
| detached | `The store was detached before the copy finished; nothing was installed.` | `store_detached`, failed, 1 |
| disk full | `Not enough space: 1.8 MB needed, 0.6 MB free; nothing was installed.` | `no_space`, too_large, 1 |
| other | `Could not install Windows apps: <errno text>; nothing was installed.` | `install_failed`, failed, 1 |
| not root | `Not allowed: only root installs or removes components.` | `permission_denied`, refused, 1 |

On a terminal the `Installing ...` line is followed by the percent and bytes rewritten in
place (`Installing Windows apps (1.8 MB)... 42 %, 0.8 MB`), as `fetch` writes its progress;
when standard output is not a terminal, the line is printed once.

**`component remove [--yes] [--dry-run] <name>`**, effect `destroy`: on a terminal it asks
`Remove Windows apps (1.8 MB)? Installed Windows programs stay in Apps and need it to run.
[y/N]` (the pane's two lines, joined); `--yes` answers it; without a terminal and without
`--yes` it refuses with `needs_confirmation`: `Removing Windows apps needs a yes: add --yes.`
The refusals are checked **before** the question, so nobody is asked about something that
will be refused:

| Case | Output | Code, category, exit |
|------|--------|----------------------|
| removed | `Removed Windows apps; 1.8 MB freed.` | 0 |
| `--dry-run` | `would remove Windows apps (1.8 MB)` (after the same refusal checks) | 0 |
| not installed | `Windows apps is not installed.` | 0 (idempotent) |
| needed | `Needed by Built-in apps: remove that first.` `  try: component remove builtin-apps` | `needed_by`, refused, 1 |
| in use | `In use: Notepad is running. Quit it, then remove.` / `In use: 3 Windows programs are running. Quit them, then remove.` / `In use: the desktop is running. Boot with run-eggos.sh --serial and remove it from there.` | `in_use`, busy, 1 |
| declined at the prompt | `Nothing removed.` | `declined_by_user`, declined, 1 |
| failed | `Could not remove Windows apps: <errno text>. It stays installed.` | `remove_failed`, failed, 1 |
| unknown, busy, not root | as install's lines | as install's codes |

The desktop's in-use line differs between the pane and the console only in its first
sentence: the pane adds `, and this window is part of it` because that is the fact the person
is looking at; the console (where it can actually be done, with the desktop not running) gets
the sentence without it. Both are one function with a flag.

**MCP.** `component_list` (`read`, the `view` grant) returns `{"store", "items"}` (section 3).
`component_install` (`name`, `dry_run`; `change`, the **`install`** grant, `destructiveHint:
false`) returns `{"installed": ["desktop", "builtin-apps"], "size_bytes": 3879731}` and, for
a dry run, `{"would_install": [...], "size_bytes": ...}`; an agent whose `install` is `ask` is
asked on screen with the grant prompt of `agents-v0.3.md` section 5, lines `component_install
· Install a component`, `name: builtin-apps`, `installs: Desktop, Built-in apps`, `size: 3.7
MB`. `component_remove` (`name`, `confirm`, `dry_run`; `destroy`, the `install` grant,
`destructiveHint: true`) **always asks the user** (`Confirmation::Always`): on screen when the
desktop runs, lines `component_remove · Remove a component`, `name: windows-apps`, `title:
Windows apps`, `size: 1.8 MB`, with `Always` hidden as for every destructive call, and through
the client's elicitation otherwise (`An agent asks to remove Windows apps (1.8 MB). Allow
it?`); `confirm: true` alone never counts. It returns `{"removed": "windows-apps",
"freed_bytes": 1887436}`. Errors are the CLI's codes in `structuredContent.error` with
`isError: true`; `needed_by` and `in_use` are checked before the question, as on the CLI, so
the user is never asked to allow something that will be refused.

## 8. Every device class (D23)

- **Desktop and laptop**: section 2.
- **Phone** (touch, a 360 px wide maximised window, 48 px targets): the pane is one column
  below 448 wide. Widgets 2-4 keep x 16 at pitch 20. Each row's panel is the pane's width
  minus 32 and **128** tall: the title at (8, 8); the state on its own line at (8, 28) in
  `text` (at 328 px a right-aligned `Not installed · 1.8 MB` would meet the title); the
  summary at (8, 48) in `text.dim`; the button at (8, 72), the panel's width minus 16, **48**
  tall. Row `r` at `124 + 128 r` plus the open note's
  height for the rows below it. The note's lines keep x 24 (the mark column is 8 on a phone:
  no `>` anywhere in the pane, so the indent is a tie to the row, not a column); its bar is
  the width minus 48; its two buttons stack full width, 48 tall, 8 apart, `Cancel` first and
  the primary one last (nearest the thumb): the confirmation note is 4 + 20 + 20 + 8 + 48 +
  8 + 48 + 8 = 164 tall. The pane's panel grows and scrolls in the window's scroll view. A
  long-press on a disabled button shows its tooltip (toolkit v0.5 2.4).
- **Tablet**: the desktop layout with the toolkit's touch sizes: rows 56 tall (the button 40
  tall at +8, the texts at +8 and +28), the note's buttons 40 tall (the confirmation 96 tall),
  row `r` at `124 + 56 r`; four rows end at 348, so the pane scrolls at the minimum window and
  fits the default.
- **TV** (1920x1080 seen from 3 m, scale 2, D-pad): everything doubles (32 px glyphs, 96 px
  rows, 192 px buttons); the ring is always drawn and sits on the first row's button when the
  pane is entered (Right from the sidebar); Up/Down move between the rows' buttons, Left goes
  back to the sidebar, OK is Enter, Back is Esc (closes a confirmation, else nothing); the
  confirmation's buttons are reached with Down from the acting row's button and Left/Right
  between the two; nothing needs a pointer. The summary line stays, since 32 px `text.dim` at
  a 3 m viewing distance is readable (toolkit v0.5 section 11); nothing in the pane depends on
  hover.
- **Wearable** (round or square, about 200x200): a **read-only list**: widget 2 only
  (`4 components, 2 installed` or `No component store.`), then one row per component of two
  lines, the title in `text` and the state word in `text.dim` (`Installed`, `Not installed`,
  `Installing 42 %`), 48 tall each, scrolling, no buttons, no summaries, no sizes, and a last
  line `Install or remove from your phone or the CLI.` in `text.dim`. Justification: an
  installer's confirmation and its failure lines cannot be read on a 200 px screen, a watch
  has no store disk to attach and no host share, and a component a watch would install (a
  layer, a desktop) is not a watch's task; but a watch that is mid-update should still tell its
  wearer what it has. `component list` and the MCP tools answer in full there, as everywhere.
- **Headless** (no desktop component, or booted without a display): no pane; the CLI and the
  MCP tools are the whole capability, and the way the desktop itself is removed (section 4).

## 9. Palettes: dark, light, and both high contrasts

The pane uses existing tokens only: `menu.bg` (the surface), `text`, `text.dim`, the button's
tokens, `button.primary` with `icon.dark`, `progress.track` with `accent`, `focus.ring`. Every
pair, recomputed from the four palettes' values (`toolkit-v0.5.md` 8.2 and
`first-run-accessibility-v0.3.md` 2.4); nothing is told by colour alone (a `Failed` state is
a word with a sentence under it, the bar has its bytes line, a disabled button has `text.dim`
and its tooltip):

| Pair | Where | Dark | Light | Dark, high contrast | Light, high contrast |
|------|-------|------|-------|---------------------|----------------------|
| `text` on `menu.bg` | titles, states, store line, notes | 12.9:1 | 16.0:1 | 21.0:1 | 21.0:1 |
| `text.dim` on `menu.bg` | summaries, detail lines, bytes | 5.1:1 | 5.4:1 | 11.4:1 | 9.6:1 |
| `text` on `button.bg` | `Install`, `Remove`, `Cancel` | 10.5:1 | 17.3:1 | 16.5:1 | 21.0:1 |
| `text.dim` on `button.bg` | disabled buttons | 4.1:1 | 5.8:1 | 9.0:1 | 9.6:1 |
| `icon.dark` on `button.primary` | the confirmation's primary button | 10.4:1 | 10.4:1 | 14.6:1 | 11.3:1 |
| `accent` on `progress.track` | the bar's done part (a graphic, ≥ 3:1) | 7.2:1 | 3.9:1 | 5.9:1 | 5.1:1 |
| `focus.ring` on `menu.bg` | the ring (a graphic, ≥ 3:1) | 8.4:1 | 4.7:1 | 14.6:1 | 7.2:1 |

The dark `text.dim` on a disabled `button.bg` at 4.1:1 is the toolkit's existing disabled
pair (momentary in Storage, lasting here while another row installs); it is why the disabled
button carries a tooltip and why the disabling lasts seconds. In both high contrasts the
button borders are white or black (every button edged) and the bar's track lifts from the
black (`#4D4D4D`) so the empty part of the bar is visible too; the panels have no border in
any palette, since they are the surface.

## 10. What an agent sees (D161, `wm show`), and what it may not touch

Each row's panel is a `group` named by the title (`Windows apps`) with the value the record's
words, `windows-apps installed 1.8 MB` (`not-installed`, `installing 42`, `removing`); under
it the title, state and summary `label`s with their drawn text, and the row's `button`. The
button's **accessible name** is `Install Windows apps`, `Remove Windows apps` or `Cancel
installing Windows apps`, not its drawn text, so a screen reader and an agent tell four
`Install` buttons apart (section 11 adds the one toolkit call this needs). Widgets 2-4 are
`label`s named by their text; the note's lines `label`s; the bar a `progress` named
`Installing Windows apps` with the value its permille (the toolkit's existing role); the
confirmation's `Cancel` and `Remove` (`Install both`) `button`s. Disabled buttons are
flagged.

**Person-only (SECURITY)**: the confirmation's primary button when it reads `Remove`
(`WidgetTree::set_person_only`, `agents-v0.3.md` section 8): the yes to a `destroy` is the
person's, and an agent removes through `component_remove`, which asks the person on screen.
`Install`, `Cancel`, `Install both` and the confirmation's `Cancel` stay actable (`invoke`):
installing adds and never destroys, and an agent that can press them holds `run`, which is
full control already (D137). An agent that wants the records calls `component_list`,
`component_install` and `component_remove`, not the tree (D22).

## 11. New widgets

None. One addition to an existing one: a **push button's accessible name apart from its
text**, `WidgetTree::set_accessible_name(id, &str)` (up to 64 bytes, as a label; empty means
"the text", today's behaviour), read by `describe` for the element's name and by nothing
else. Without it a screen reader hears `Install` four times. Every other thing here is the
toolkit's: labels, push buttons, the progress bar, a panel, `set_place`, `set_person_only`,
tooltips.

## 12. Acceptance (host tests, default 640x440, scale 1x, a fake store with the four
components of the table above on `/dev/vdc`, `desktop` and `builtin-apps` installed)

- `PANES[COMPONENTS]` is `Components` at row (0, 332, 176, 32); `components`, `uninstall`,
  `optional features`, `programs and features`, `store` and `layer` match it alone; `install`
  matches it and Agents; `size` matches it and Storage.
- The tree holds 250 widgets. In window coordinates: heading (192, 16); `Component store:
  EggOS 0.3.0, x86_64` at (192, 56); `eggos-0.3.0-x86_64-components.img on /dev/vdc` at
  (192, 76); `2 of 4 installed · 3.7 MB on disk` at (192, 96); row 0's panel (192, 124, 432,
  48) with `Desktop` at (216, 130), `Installed · 2.6 MB` ending at 512 on cell top 130, `The
  window server, the shell and Settings` at (216, 150), `Remove` at (528, 134, 96, 28); rows
  1-3 at y 172, 220, 268 reading `Built-in apps` `Installed · 1.1 MB` `Remove`, `Windows
  apps` `Not installed · 1.8 MB` `Install`, `Mac apps` `Not installed · 1.4 MB` `Install`;
  panels 4 and 5 hidden; the note hidden.
- Tab from the sidebar focuses row 0's `Remove`; Down focuses row 1's, End row 3's, Up row
  2's. Enter on row 2's `Install`: the state reads `Installing 0 %`, the button `Cancel`, the
  note at y 268 (window) shows the bar (216, 276, 408, 8) and `0 B of 1.8 MB` at cell top
  292, rows 3 moved to y 308, every other button disabled; a tick with 42 % shows `Installing
  42 %`, the bar at 420 permille and `0.8 MB of 1.8 MB`, damaging those three rects only; a
  tick with done re-reads: `Installed · 1.8 MB`, `Remove`, row 3 back at 268, the count line
  `3 of 4 installed · 5.5 MB on disk`.
- `Remove` on row 0 (desktop) shows under it `In use: the desktop is running, and this window
  is part of it.` and the second line; `Remove` on row 0 of a fake where the desktop is not
  running but `builtin-apps` is installed shows `Needed by Built-in apps: remove that first.`;
  `Remove` on row 1 with a fake running `Text Editor` shows `In use: Text Editor is running.
  Quit it, then remove.`; with nothing running it shows the confirmation `Remove Built-in
  apps (1.1 MB)?` at (216, 224) and `They leave the launcher; your files stay.` at (216,
  244), `Cancel` at (392, 268, 96, 28) and `Remove` (primary) at (496, 268, 128, 28) with the
  focus on `Remove`, rows 2 and 3 moved down 84; Enter presses `Remove`: the state
  `Removing...`, the button disabled, then `Not installed · 1.1 MB` and `Install`. Esc with
  the confirmation open closes it, removes nothing and focuses row 1's button.
- `Install` on `builtin-apps` when `desktop` is not installed shows `Installs Desktop (2.6
  MB) too: Built-in apps need it.` / `3.7 MB in all.` with `Cancel` and `Install both`;
  `Install both` installs `desktop` first (its row shows the bar) and `builtin-apps` second.
- A fake whose read fails with a checksum mismatch shows `Failed`, `The store's copy of
  windows-apps.component is damaged.` over `Nothing was installed.`, the button `Install`,
  and `component list` says `not-installed`; `ENOSPC` with 0.6 MB free shows the space line;
  `ENOENT` mid-copy the detached line; `Cancel` mid-copy `Cancelled.` over `Nothing was
  installed.` and no file in the components folder.
- No store, `desktop` installed: widgets 2-4 read the three no-store lines, one row shows
  (`Desktop`, `Remove`), nothing reads `Install`; no store and nothing installed: no panels.
  The store appearing on a read adds the rows in the same frame.
- `component list`, `component_list` and the pane agree row for row and word for word on the
  fake; `component install mac-apps --dry-run` prints `would install Mac apps (1.4 MB): 1.4 MB
  in all`; `component remove desktop` from a console with the desktop running prints the in-use
  line without `, and this window is part of it`; `component remove windows-apps` without a
  terminal exits 1 with `needs_confirmation`; `component_remove` with `confirm: true` and no
  way to ask is refused the same way.
- A pane 328 wide (phone) lays each row 128 tall with the state on its own line and a 48 px
  button; the element tree holds the `group` `Windows apps` with the value `windows-apps
  not-installed 1.8 MB` and a `button` named `Install Windows apps`; `wm act invoke` on the
  confirmation's `Remove` answers `element_cannot`, on `Install both` presses it.
- Every pane text here fits a label's 64 bytes with the four titles and sizes under 1 GB:
  the longest are `In use: 3 Windows programs are running. Quit them, then remove.` (63) and
  `In use: the desktop is running, and this window is part of it.` (62); a test pins every
  line's byte length, and a line that would not fit (a long errno text) ends in `...` as
  Storage's `shortened` does, the CLI having the whole sentence.

## Later (part 65, the full Components pane, and after)

The installer's flow in the pane (what a component adds: launcher entries, commands, MCP
tools, as the app installer's `Adds:` line); the component's own page (version, files,
what needs it, its running programs by name); updates (a newer store than the installed
version: `Update` as a third button state, delta updates applied atomically, D69);
downloading components from the network instead of a store disk (D69's on-demand background
download, with the Downloads app's table); several installs at once with a queue; a search
field over the components; sorting; `component store` as a command of its own (`component
list` carries the store on standard error until then); choosing components at first run
(the setup's page); the base's own size as a read-only row (About shows it); removing the
desktop from inside the desktop (a "remove at next boot" marker, if research finds it
worth having); per-user components; the wearable's own install path (from a paired phone).
