# EggOS v0.3 system file picker: design brief

Design by Claude Fable (D16) for the one file picker every app uses: `/bin/pick`, a program of
the base (D18: every layer and every app needs it, and it is small). An app never draws its own
picker: it starts `pick`, waits, and reads a path. EggOS apps (the Text Editor's `Open...` and
`Save As...`, Files' `Change Target...`), the Windows layer (`GetOpenFileNameW`,
`GetSaveFileNameW`, `SHBrowseForFolder`, `IFileOpenDialog` with `FOS_PICKFOLDERS`), the macOS
layer (`NSOpenPanel`, `NSSavePanel`) and, later, the Linux desktop portals all call it, so a
person learns one picker and an agent reads one element tree. It is built on the toolkit
(`toolkit-v0.5.md`) in Files' vocabulary (`desktop-v0.3-gui.md` section 8.1: the toolbar, the
places sidebar, the table, the 16 px icons, the in-window panel and notice); everything there
stands unless this brief says otherwise. Numbers are px at 1x and double at 2x; tokens are the
palette's (section 13); no new token is introduced.

**The Windows layer's own picker** (`windows-controls-v0.3.md` section 7, D145) **gives way to
`pick`**: the layer keeps the API and the path mapping and runs `pick` for the window (section
14); its message boxes for "not found" and "replace" are the picker's own notices and panel now.

## 0. What a person gets, and how that is checked

Open a file, save under a name, or choose a folder, from any app, in a window that looks and
works like Files: the same places, the same list with its sortable columns, the same keys. An
agent can see the picker and get the person to the right folder, but cannot choose for them
(section 11). Host tests pin the layout, the strings, the exit codes and the element tree
(section 16).

## 1. One process, one pipe

- `pick <mode> [options]` (section 15) is started by the app with its standard output on a pipe;
  the app reads to end of file. The picker prints the **chosen path(s), one per line, absolute,
  normalised** (no `.` or `..`, the path the person chose, through a link when they came by one,
  never resolved) and exits **0**; on Cancel it prints nothing and exits **1**. The app gets a
  path and nothing else: no handle, no bytes, no state.
- One picker per app window at a time. While it runs, the app treats itself as blocked: it
  ignores its own input and draws unfocused (the Text Editor's rule, 17.6), the Windows layer
  disables the owner as a real modal does, AppKit's `runModal` returns when the pipe closes.
  **The picker is its own top-level window** (`Normal`; a `Dialog` must belong to the client
  that owns its parent, and the app is another process), focused when it opens.
- Flags: **`PERSON_INPUT_ONLY`** (`CREATE` flag 4): synthesised input (`SEND_INPUT`,
  `CHOOSE_MENU`) never reaches it (section 11). No `FIXED_SIZE`: it resizes (section 5).

## 2. Modes

| Mode | Title (default) | Primary button | Chooses |
|------|-----------------|----------------|---------|
| `open` | `Open` | `Open` | one existing file; `--multiple` several; `--folders` lets `Open` take a selected folder too |
| `save` | `Save As` | `Save` | a folder plus a name typed in the name field; the file may or may not exist |
| `folder` | `Choose a Folder` | `Choose` | the current folder, or the selected folder row |

- **open**: double-click or Enter on a folder row enters it; on a file row chooses it. `Open`
  chooses the selected file(s) (dim with none selected); with `--folders`, a selected folder as
  well, while Enter and double-click still enter it (so a person can pick a folder or go into it).
- **save**: the name field (section 7) is prefilled with `--name` (default `Untitled`), the
  **stem selected** (up to the last `.` that is not the first character) so typing replaces the
  name and keeps the extension; a click on a file row copies its name into the field. Committing
  (`Save`, Enter in the field) joins the name to the current folder; `--extension <ext>` is
  appended when the name has no `.`. The **folder must be writable**: a read-only file system
  (`statfs`) or a refused `access(W_OK)` shows `Cannot save here: <folder> is read-only.` and
  nothing is chosen. An existing file asks the panel **`Replace <name>?`** / `A file with that
  name already exists in <folder>.`, `Cancel`, `Replace` (primary); `--overwrite` skips the
  question for callers that ask themselves (`OFN_OVERWRITEPROMPT` unset).
- **folder**: files are listed **dimmed** (`text.dim`, as hidden files are), not hidden: the
  person sees what the folder holds. Dim rows take no click and are skipped by the arrows.
  `Choose` takes the selected folder row, else the folder shown. Enter and double-click on a
  folder row enter it. No name field, no filter.

## 3. Filters

`--filter "<description>|<patterns>"`, repeatable, in the order given: `Text files|*.txt;*.md`.
Patterns are `;`-separated, `*` and `?` as the shell's, ASCII case-insensitive; `*` alone
matches every file. The drop-down (section 5) lists each description as given (the app's
description carries its patterns when it wants them shown, Windows style `Text files (*.txt)`),
and the picker appends **`All files`** last unless a given filter is `*`. The first applies
(`--filter-index <n>`, 1-based, picks another); folders always show; without any `--filter`
the drop-down is absent and every file shows. In `folder` mode filters are ignored.

## 4. Layout (content coordinates, width `W`, height `H`)

Default content **640x440**, minimum **560x360**. The surface is `menu.bg`.

| Band | Rect | Look |
|------|------|------|
| toolbar | rows `0..31` | `bar.bg`, `bar.edge` at row 31: `<` at `(4, 2)` and `^` at `(36, 2)`, 28x28 flat glyph buttons as Files'; the path field from x 72 to `W - 8`, y 2, 28 tall, the folder's path |
| sidebar | x `0..159`, y 32 to the band's top | `dock.bg`, 1 px `menu.border` at x 159; `Places` in `text.dim` at (12, 40); rows of 24 from y 60, text at x 24, `>` at x 8 on the current place |
| table | x `160..W-1`, y 32 to the band's top | toolkit `Table` named `Entries`: header rows 32..59, rows of 24 from y 60; columns `Name` (the rest, at least 160), `Size` (80, `Align::End`), `Modified` (128, `YYYY-MM-DD HH:MM`); rows look as Files' (the 16 px icon at x + 8, the name at x + 28, the focus row's `>`); the link badge as Files' |
| bottom band | the last **52** rows (`open`, `folder`) or **92** (`save`) | `bar.bg`, `bar.edge` on its top row |
| name row (`save`) | y `H - 80` | `Name:` in `text` at (12, `H - 74`); the field from x 72 to `W - 12`, 28 tall |
| buttons row | y `H - 40` | `Hidden files` checkbox, mark 16x16 at (12, `H - 34`), label at x 36; the filter drop-down at (172, `H - 40`), 160x28, when there are filters; `Cancel` 96x28 at (`W - 212`, `H - 40`); the primary 96x28 at (`W - 108`, `H - 40`), `button.primary`, label as section 2 or `--button <text>` |

Places: `Home` (`/home/root`), `Desktop`, `Documents`, `Downloads`, `Host share` (while
mounted), `Work disk` (while attached), `System` (`/`). No Trash: nothing is opened from or
saved into it. Resizing stretches the table and the path field; the sidebar keeps 160, the
bands their heights, the buttons their right edge. At 640x440 the `open` table shows 13 full
rows. The title is `--title` or the mode's default, cut at 64 bytes with `...` as every title.

## 5. Placement

`--over <window id>` names the app's window: the picker asks the server for that window's
outside rect (a read of another client's placement, which the switcher shows anyway) and
centres on it, clamped inside the work area; without `--over`, or with an id the server does
not know, it is centred in the work area. It opens focused and above the app; the app does not
raise itself while it waits. Closing the window (the control, Alt+F4, Mod+W) is Cancel.

## 6. The list

- **Contents**: the folder's entries without `.` and `..`; names starting with `.` hidden until
  `Hidden files` is checked (then dimmed, Files' 8.1 item 20); folders first, then the files the
  filter admits; the `Deleted` column and the Trash view do not exist here.
- **Sort**: every header sortable as Files' (8.1 item 18): click ascending, click again
  reversed, the `accent` arrow, folders first in either order, `Name` breaking ties; `Name`
  ascending at start, kept for the picker's life.
- **Selection**: no row is selected at start (`Open` dim); Up or Down selects the first row; a
  click selects; `--multiple` uses Files' multiple selection (8.1 item 17: Mod+click, Shift,
  Mod+A, Enter on the focus row chooses every selected file). Dim rows (hidden files, files in
  `folder` mode) are skipped by the arrows and ignore clicks; a dim row cannot be chosen.
- **Empty and errors**, one `text.dim` line at the first row's text position: `This folder is
  empty`, `No files match <description>` (files exist but the filter admits none), `Cannot read
  this folder (<errno name>)`.
- **Type-ahead**, F5, `<`, `^`, Backspace and Alt+Up as Files'. Icons are Files' 16 px set.

## 7. The path field and the name field

Typing in either and Enter: `/...` is absolute, `~/...` is under home, anything else joins the
current folder; `..` and `.` are normalised. A path to a **folder** enters it (the name field
keeps the last component when the text was `docs/notes.txt` and `docs` is a folder: the field
reads `notes.txt`); in `open` mode a path to a **file** chooses it; in `save` mode a path to a
file is the name to save under (the `Replace` question applies). The path field's text is
restored by Esc while it has focus, as Files'. **Mod+L** focuses the path field with its text
selected (Files' key, in every profile).

## 8. Keys

`Mod` is Ctrl or Cmd, both accepted everywhere, as Files.

| Key | Does |
|-----|------|
| Enter | in the table: enters a folder row, chooses a file row (section 2); in a field: commits it (section 7); on a button: presses it; elsewhere: the primary button |
| Esc | closes an open panel, notice or drop-down; else **Cancel** (exit 1) |
| Backspace (table), Alt+Up, Cmd+Up | up one folder, the folder you left selected |
| Alt+Left, Cmd+[ | back |
| Up, Down, Home, End, PageUp, PageDown | move in the table; Shift extends with `--multiple` |
| Mod+A | select all (`--multiple`), else nothing |
| Mod+L | the path field, text selected |
| Mod+H, Cmd+Shift+. | toggle `Hidden files` |
| F5 | reload |
| Tab, Shift+Tab | the focus order: path field, places, table, name field (`save`), `Hidden files`, filter, `Cancel`, the primary; wrapping |
| letters in the table | type-ahead |

Initial focus: the table (`open`, `folder`) or the name field with its stem selected (`save`).
`<` and `^` are not Tab stops. There is no context menu and no menu bar: the picker chooses, it
does not manage files (`New folder` is section 17's list).

## 9. Messages

Panels and notices are Files' (8.1 items 3 and 6), centred on the table's width. `<name>` is the
last component of what was typed or chosen, `<folder>` the folder's full path, `<description>`
the filter's text.

| Text | When | Form |
|------|------|------|
| `<name> does not exist.` | `open`: a typed path to nothing | notice |
| `<folder> does not exist.` | a typed path whose folder part is missing | notice |
| `Could not open <folder> (<errno name>).` | entering a folder fails (`EACCES`, `ENOTDIR`, ...) | notice |
| `Type a name.` | `save`: the primary or Enter with an empty name | notice |
| `That is not a file name.` | `save`: the name is `.`, `..` or ends with `/` | notice |
| `The path is too long.` | the joined path passes 4096 bytes (the picker's `PATH_CAPACITY`) | notice |
| `Cannot save here: <folder> is read-only.` | `save`: the folder refuses writes | notice |
| `Replace <name>?` / `A file with that name already exists in <folder>.` | `save`: the name exists | panel, `Cancel`, `Replace` (primary) |
| `<name> is a folder.` | `open` with `--folders` absent: a typed path to a folder is entered instead; this text is never shown, listed so no one adds it | — |

A NUL cannot be typed; a `/` in a typed name is a path separator (section 7), so no "cannot
contain" message exists here. Notices close after 3000 ms, on a key or on a button-down.

## 10. Starting folder

`--folder <dir>`, else the app's current directory. When it does not exist, is not a folder or
cannot be read, the picker opens on **home** (`/home/root`) with no notice: a stale folder an
app remembered is not the person's problem to read about. When home cannot be read either, `/`.

## 11. Security: the choice is the person's

The picker is where an app, or an agent driving one, receives a path the person decided to
hand over. So **choosing is person-only**: the toolkit's `set_person_only` flag (agents-v0.3.md
section 8) is set on the primary button (`Open`, `Save`, `Choose`), on every **file** row in
`open` mode (its `invoke` would choose), and on the `Replace` button of the panel; `wm act` on
them answers `element_cannot`, and `wm show` lists `person-only`. The window's
`PERSON_INPUT_ONLY` flag closes the other door: `wm_press Enter`, `wm_click` on the button and
`wm_type` into the name field all fail with `EPERM`, so no synthesised key or click can stand
in for the person. **Everything else is actable**: `Back`, `Up`, the path field (`set-text`
navigates), the places, the table (`select` a row, `invoke` a folder row to enter it, `set-text
"<n>"`, the headers), the name field (`set-text` types a name the person then sees and may
save), `Hidden files`, the filter, `Cancel` (cancelling hands the app nothing). An agent can set
the stage: open the right folder, type the name, point at the row; the person presses the
button. An agent that wants an app to open a file without the person uses the app's own action
with a path under its grants (D21, D193), never the picker.

## 12. What an agent sees (D161, `wm show`)

| Element | Role | Name | Value / flags |
|---------|------|------|---------------|
| the window | `window` | the title | |
| `<`, `^` | `button` | `Back`, `Up` | `disabled` when there is nothing to go to |
| the path field | `text field` | `Path` | the folder's path |
| the sidebar | `list` | `Places` | `list item`s `Home`, `Desktop`, ..., `System`; `selected` on the current place |
| the list | `table` | `Entries` | `row 3 of 42` or `3 of 42 selected`; `columnheader`s `Name`, `Size`, `Modified` (value `ascending`/`descending` on the sorted one) |
| a row | `row` | the name | cells joined by tabs; `selected`; `disabled` on a dim row; **`person-only` on a file row in `open` mode** |
| the name field | `text field` | `Name` | its text |
| the checkbox | `checkbox` | `Hidden files` | `checked` |
| the filter | `combo box` | `Filter` | the chosen description; its options as children |
| the buttons | `button` | `Cancel`; `Open` / `Save` / `Choose` (or `--button`'s text) | the primary **`person-only`** |
| the panel | `group` | `Replace` | `label`s with its two lines; `button`s `Cancel` and `Replace` (**`person-only`**) |
| a notice | `label` | its text | listed while shown |

## 13. Palettes

Only existing tokens, in the roles Files gives them: `menu.bg` surface, `bar.bg` and
`bar.edge` bands, `dock.bg` sidebar, `field.bg` fields, `button.*` buttons, `selection`,
`focus.ring`, `accent` (the `>`, the sort arrow), `text`, `text.dim`, `progress.track` (scroll
bars) and the icon tokens of Files' 16 px set. The pairs that carry meaning, WCAG from the
values in `toolkit-v0.5.md` 8.2 and `first-run-accessibility-v0.3.md` 2.4:

| Pair | Dark | Light | Dark, high contrast | Light, high contrast |
|------|------|-------|---------------------|----------------------|
| `text` on `menu.bg` (rows, labels) | 12.9:1 | 16.0:1 | 21.0:1 | 21.0:1 |
| `text.dim` on `menu.bg` (dim rows, headers, `Places`) | 5.1:1 | 5.4:1 | 11.4:1 | 9.6:1 |
| `text` on `selection` (the selected row) | 7.5:1 | 12.9:1 | 11.4:1 | 15.7:1 |
| `accent` on `menu.bg` (`>`, the arrow; a graphic) | 8.4:1 | 4.7:1 | 14.6:1 | 7.2:1 |
| `text` on `field.bg` (path, name) | 16.0:1 | 17.3:1 | 21.0:1 | 21.0:1 |
| `text.dim` on `field.bg` (placeholders) | 6.3:1 | 5.8:1 | 11.4:1 | 9.6:1 |
| `text.dim` on `bar.bg` (nothing rests here; the band's labels are `text`) | 5.7:1 | 5.0:1 | 11.4:1 | 9.6:1 |

A dim row that is the focus row draws `text` on `selection` (Files' cut-row rule), since
`text.dim` on `selection` is 3.0:1 in the dark palette. The picker reads the palette from the
server at creation and on `PaletteChanged`, as every toolkit program.

## 14. The layers map their APIs onto `pick`

| Caller | `pick` |
|--------|--------|
| `GetOpenFileNameW` | `open`; `lpstrTitle` → `--title`; `lpstrInitialDir` (mapped from `C:\`) → `--folder`; `lpstrFilter` pairs → `--filter` each, `nFilterIndex` → `--filter-index`; `OFN_ALLOWMULTISELECT` → `--multiple`; the result mapped back to `C:\...` into `lpstrFile` (and `nFileOffset`, `nFileExtension`, `lpstrFileTitle`); exit 1 → `FALSE` with `CommDlgExtendedError` 0; `FNERR_BUFFERTOOSMALL` as before; `OFN_FILEMUSTEXIST` always holds (the picker only chooses what exists) |
| `GetSaveFileNameW` | `save`; `lpstrFile`'s name part → `--name`; `lpstrDefExt` → `--extension`; no `OFN_OVERWRITEPROMPT` → `--overwrite` |
| `SHBrowseForFolder`, `IFileOpenDialog` with `FOS_PICKFOLDERS` | `folder`; `lpszTitle` / `SetTitle` → `--title`; `SetFolder` → `--folder`; `SetOkButtonLabel` → `--button` |
| `IFileOpenDialog`, `IFileSaveDialog` | `open` / `save`; `SetFileTypes` → filters, `SetFileTypeIndex` → `--filter-index`; `SetFileName` → `--name`; `SetDefaultExtension` → `--extension`; `FOS_ALLOWMULTISELECT` → `--multiple`; `GetResult(s)` from the lines |
| `NSOpenPanel` | `open`; `canChooseDirectories` with `canChooseFiles` → `--folders`, without → `folder`; `allowsMultipleSelection` → `--multiple`; `allowedContentTypes`/`allowedFileTypes` → one `--filter` per type; `directoryURL` → `--folder`; `prompt` → `--button`; `title`/`message` → `--title`; `URLs` from the lines; `NSModalResponseCancel` on exit 1 |
| `NSSavePanel` | `save`; `nameFieldStringValue` → `--name`; `prompt` → `--button`; the panel's own replace sheet is not shown (the picker asks) |
| the Text Editor, Files | section 17.6 of `text-editor-v0.3.md`; 8.1 item 24 |

The layers' own picker code (windows-controls section 7) is removed; its tests move to the
picker's. In the Windows profile the picker's wording is unchanged (nothing here is named
differently per platform).

## 15. The CLI (D22)

```
pick open   [--title <text>] [--folder <dir>] [--filter "<description>|<patterns>"]...
            [--filter-index <n>] [--multiple] [--folders] [--button <text>] [--over <window id>]
pick save   [--title <text>] [--folder <dir>] [--name <name>] [--extension <ext>] [--filter ...]...
            [--filter-index <n>] [--overwrite] [--button <text>] [--over <window id>]
pick folder [--title <text>] [--folder <dir>] [--button <text>] [--over <window id>]
```

- Exit **0**: the chosen path(s) on standard output, one per line, `\n`-terminated each.
- Exit **1**: Cancel, Esc, the close control or `CloseRequest`; nothing printed.
- Exit **2**: a usage error (unknown mode or option, a filter without `|`, a `--filter-index`
  out of range); one line on standard error, `pick: unknown option --x`; no window.
- Exit **3**: the picker cannot show (no window server; or a mode the device refuses, section
  16); `pick: cannot show a window` or `pick: save is not available on this device` on standard
  error; on a wearable the refusal is also shown to the person (section 16).

`--name` with a `/` keeps its last component; `--title` longer than 64 bytes is cut. From
eggsh, `pick open` prints what the person chose, so a script can ask the person for a file;
the MCP server exposes no `pick` tool, by design (section 11): an agent names paths itself.

## 16. Device classes (D23)

- **Phone** (touch, a 360 px wide maximised window): the picker fills the screen as a
  **full-screen sheet** (every `Normal` window is maximised there); the toolbar is 40 tall; the
  **sidebar becomes a places row** under it, rows 40..79: chips `Home` `Desktop` `Documents`
  `Downloads` `System` (and the mounted places) 40 tall, scrolling sideways, the current one
  `selection`; the table takes the full width with 40 px rows; the bottom band stacks rows of 40
  with 12 px gaps: the name field (`save`, full width), the filter drop-down (full width, when
  there are filters), `Hidden files`, and the two buttons each `(W - 36) / 2` wide, `Cancel`
  left and the primary right. `Back` is Cancel; a tap on a dim row does nothing.
- **TV** (D-pad, scale 2): the ring is always drawn; Up/Down/Left/Right move the focus between
  the toolbar, places, table and buttons (the toolkit's two-dimensional order), `Back` is Esc
  (Cancel), the remote's `OK` is Enter; no header sorting (`Name` ascending); the pointer is
  hidden. Default size 1280x880 centred over the app or the work area.
- **Wearable** (a round or 200 px screen): **`open` only**, as a one-column list of the start
  folder's entries, names only, 48 tall rows, the first filter applied, no sidebar, path field,
  filter drop-down or hidden toggle; a tap on a folder enters it, on a file chooses it (the
  person's tap: `PERSON_INPUT_ONLY` holds); the hardware back goes up, and at the start folder
  cancels. `save` and `folder` show a sheet with one line, `Saving needs a bigger screen.` or
  `Choosing a folder needs a bigger screen.`, and `OK`, then exit 3. `--multiple` is single.
- **2x**: every number doubles.

## 17. Acceptance (host tests, 640x440, scale 1x, dark palette unless said)

The frame of `pick open --folder /home/root --filter "Text files|*.txt"` with `Places` at
(12, 40), `Home` selected, the header at rows 32..59, 13 rows, `Text files` in the drop-down at
(172, 400) and `Open` dim at (532, 400); the same in the light and both high-contrast palettes
(hashes); `save` with `Untitled.txt` in the field at (72, 360), `Untitled` selected, the band 92
tall; `folder` with files dim and `Choose`; `--multiple` with three rows selected and three
lines printed; `--folders` choosing a folder with `Open` while Enter enters it; Name, Size and
Modified sorted each way with folders first; the typed paths `~/Documents`, `../x`,
`docs/notes.txt`; each message of section 9 and `Replace` with `--overwrite` skipped;
`--extension txt` appended to `notes`; a read-only folder refusing `Save`; a missing `--folder`
opening home; exit codes 0, 1, 2 and 3 with their output; the element tree listing every row of
section 12 with `person-only` on `Open`, on file rows and on `Replace`, and `wm act` `invoke`
on `Open` answering `element_cannot` while `set-text /tmp` on `Path` navigates; `SEND_INPUT`
aimed at the window refused with `EPERM`. QEMU: the picker over the Text Editor, centred, and
`pick open` from eggsh printing the path a scripted person's click chose (the test harness is
the person: it uses the server's own input path, which `PERSON_INPUT_ONLY` admits).

## 18. Left out

`New folder` inside the picker; a search field; recent files; a type-ahead across folders;
previews; the picker remembering the last folder per app (the app passes `--folder`); drag and
drop into the picker; the Linux portal front-end (the protocol here is designed for it: a path
on a pipe is what `org.freedesktop.portal.FileChooser` wants to return).
