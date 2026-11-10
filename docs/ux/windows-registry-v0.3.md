# EggOS v0.3.0 Windows registry: design brief (D231 slice b)

Status: v0.3 design, D231 slice b. Design by Claude Fable (D16) for the GUI front-end of the
Windows layer's registry capability (D22): the Settings `Windows` pane, where a person browses
and edits the registry that Windows programs on EggOS see, with the same data and words as
`windows registry get|set|delete|reset` and the MCP tools `windows_registry_get`,
`windows_registry_set`, `windows_registry_delete`, `windows_registry_reset`. Everything in
`toolkit-v0.3.md` (section 11 for Settings), `toolkit-v0.4.md` (dropdown, selection and the
clipboard), `toolkit-v0.5.md` (table, text area, device classes, the light palette),
`storage-v0.3.md` and `agents-v0.3.md` (the in-pane modal panel) and `first-run-accessibility-v0.3.md`
section 2.4 (high contrast) stands. Numbers are px at 1x and double at 2x; the UI font is Egg Sans
16 everywhere, the only motion is the caret's blink, and **no new colour token** is added: every
colour below is an existing token, so the dark, light and both high-contrast palettes are covered
by their tables.

Familiar to someone who knows regedit (the same root names, type names, `(Default)`, `0x0000002a
(42)`, F5, Delete), in EggOS Settings' shape: a sidebar pane with a path bar, a key outline on the
left, a value table on the right, actions as buttons and panels, never a copy of regedit's menus.

## 1. The pane in the sidebar

- **Name** `Windows`. Its row is shown **only while the Windows layer (the `windows` component,
  D18) is installed**: `PANES` holds it at index 9, right before `Components` (10) and `About`
  (11), so the pane that exists because of a component sits next to the pane that manages
  components, and the two about the system itself stay last. Its row is y 332..363 (44 + 9 x 32).
  The program asks the component manager once when the window opens and each time the sidebar
  list is shown; without the layer the row is hidden (`set_shown_items`, as Storage hides absent
  disks) and `Components` and `About` move up 32. A layer installed while Settings is open
  appears the next time the list is shown; a layer removed while the pane shows switches to
  `Components` and hides the row.
- **Search words**: `registry`, `regedit`, `Registry Editor`, `reg`, `hive`, `HKEY`, `HKCU`,
  `HKLM`, `HKCR`, `HKEY_CURRENT_USER`, `HKEY_LOCAL_MACHINE`, `HKEY_CLASSES_ROOT`, `key`,
  `value`, `REG_SZ`, `DWORD`, `QWORD`, `binary`, `string`, `Win32`, `exe`, `.exe`, `Windows
  programs`, plus its button names `New key`, `New value`, `Edit`, `Delete`, `Refresh`, `Copy
  path`. `Windows` also matches Appearance's profile card; `key` also matches Keyboard;
  nothing else collides.
- **Budget**: this pane is its container plus exactly **35 widgets** (12 in the pane, section 2;
  9 in the edit panel, 8 in the name panel, 6 in the confirmation panel, section 5), so
  `CAPACITY` grows by 36 (211 to **247** at the time of writing; the installer's
  `eggos_app::dialog::CAPACITY` stays equal to it).

## 2. Layout at desktop and laptop (pane coordinates: origin window x 176, y 0; 464 wide at the
default size, so the right-aligned edge is x 448; H is the pane's height, 332 at the minimum
window and 348 at the default)

Every widget has one fixed place; a state shows, hides, enables or retitles widgets and never
moves them. The outline and the table are the only widgets whose height follows H.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Windows` |
| 2 | text field | (16, 64), 232x28 | the path bar, section 3; placeholder `Key path`, `escape: Clear` |
| 3 | push button | (256, 64), 96x28 | `Copy path` |
| 4 | push button | (352, 64), 96x28 | `Refresh` |
| 5 | outline (section 4) | (16, 100), 176 x (H - 176) | the keys; name `Keys` |
| 6 | table | (200, 100), 248 x (H - 176) | the values; name `Values`; columns section 3 |
| 7 | push button | (16, H - 68), 96x28 | `New key...` |
| 8 | push button | (120, H - 68), 96x28 | `New value...` |
| 9 | push button | (224, H - 68), 96x28 | `Edit...` |
| 10 | push button | (328, H - 68), 96x28 | `Delete...` |
| 11 | push button | (16, H - 68), 136x28 | `Replace file...`; hidden except in the damaged state (section 6), where 7-10 are hidden |
| 12 | label | (16, H - 32) | the status line, section 6, `text.dim` or `text`, end-ellipsised at 448 |

At the minimum window the outline is 156 tall (six rows and 12 px of surface) and the table
shows its header and five rows; at the default, seven and six. The 8 px gap between the outline
and the table is the pane's `menu.bg`; the split is fixed in this slice (a draggable splitter is
listed under "Left out"). Both scroll on their own (toolkit section 8; the table also sideways,
v0.5 section 6); the pane itself never scrolls at desktop widths.

## 3. The path bar and the value table: one set of words for the pane, the CLI and MCP

**Roots.** The outline's top level is three rows, always present and never deletable:
`HKEY_CURRENT_USER` (the user's hive, writable), `HKEY_LOCAL_MACHINE` (the machine's hive,
writable by root only) and `HKEY_CLASSES_ROOT` (the merged view of `HKEY_CURRENT_USER\Software\
Classes` over `HKEY_LOCAL_MACHINE\Software\Classes`). `HKEY_USERS` and `HKEY_CURRENT_CONFIG` are
**not shown**: the first is the current user's hive under a SID and the second an alias into
`HKEY_LOCAL_MACHINE`, so a row for either would show what another row already shows. Programs
still open them through the API, and the CLI accepts them (and the short forms `HKCU`, `HKLM`,
`HKCR`, `HKU`, `HKCC`) as input; the GUI and every printed path use the resolved long name.

**The path bar** (widget 2) shows the selected key's full path, `HKEY_CURRENT_USER\Software\
AutoHotkey`, with the caret at the end, so a path wider than the field shows its tail (the field
scrolls to its caret). Typing a path and Enter selects that key, expanding the outline down to
it; an unknown path keeps the field's text, draws the field's border in `button.close` until the
next keystroke, and the status line reads `No such key: HKEY_CURRENT_USER\Software\Nothing.` in
`text`. Esc in the field restores the selected key's path. A path longer than the field's 255
bytes shows `...` and its last 252 bytes and cannot be edited there (the outline still reaches
the key). `Copy path` (widget 3, and Mod+C while the outline has focus) puts the full path on the
clipboard and the status line reads `Copied HKEY_CURRENT_USER\Software\AutoHotkey.` in `text`
until the next change. `Refresh` (widget 4, F5) re-reads the hive, keeps the selection where the
key still exists and otherwise selects its nearest existing parent.

**Subkeys** are listed in the outline sorted case-insensitively (regedit's order), read when
a key is expanded and on Refresh; a key with no subkeys has no disclosure glyph. A key with more
than 1,000 subkeys shows the first 1,000 and a last unselectable row `and 2,415 more: windows
registry get <key> lists them all.` in `text.dim`.

**The value table** (widget 6) has three columns, all sortable, the first row always the
unnamed value, as regedit does:

| Column | Width, min | Cell |
|--------|------------|------|
| `Name` | 96, 64 | the value's name; `(Default)` for the unnamed value, always the first row whatever the sort |
| `Type` | 104, 64 | `REG_SZ`, `REG_EXPAND_SZ`, `REG_MULTI_SZ`, `REG_DWORD`, `REG_QWORD`, `REG_BINARY`; another numeric type as `REG_<n>` (`REG_5`), shown as hex bytes and not editable |
| `Data` | 200, 64 | below |

Content width 400 against the table's 248, so the sideways bar shows at the default window and
`Data` is reached by Shift+wheel, the bar or a wider window; the columns resize by their handles
as v0.5 section 4.4 says. Default sort `Name` ascending. Data per type, one function for all
three front-ends (`registry::data_text`), end-ellipsised at the cell and bounded at 64 bytes:

| Type | Data as shown | Example |
|------|---------------|---------|
| `REG_SZ`, `REG_EXPAND_SZ` | the string as stored (no expansion); glyphs Egg Sans lacks draw `?`; a control character draws `?` | `2.0.19` |
| `REG_MULTI_SZ` | the strings joined by ` · ` (U+00B7) | `first · second · third` |
| `REG_DWORD` | `0x` + 8 lowercase hex digits, a space, the decimal in parentheses | `0x0000002a (42)` |
| `REG_QWORD` | `0x` + 16 hex digits, the decimal in parentheses | `0x000000000000002a (42)` |
| `REG_BINARY` and `REG_<n>` | lowercase hex bytes separated by spaces | `4a 00 ff 1e` |
| any, zero bytes | `(zero-length value)` | |
| `(Default)` when unset | type `REG_SZ`, data `(value not set)` | |

The CLI prints the same cells: `windows registry get <key> <name>` prints one line `<name>  <type>
<data>` (two spaces between columns, as `disk list`); `windows registry get <key>` prints the
subkeys first, one per line with a trailing `\` (`AutoHotkey\`), then the values, `(Default)`
first; `--json` gives one record per line: `{"key": "HKEY_CURRENT_USER\\Software\\AutoHotkey",
"name": "Count", "type": "REG_DWORD", "data": 42}` with `data` a string for the string types, an
array of strings for `REG_MULTI_SZ`, an integer for the number types and a hex string without
spaces (`"4a00ff1e"`) for binary and unknown types; a subkey record is `{"key": "...", "subkey":
"AutoHotkey"}`. `windows_registry_get` returns the same records.

## 4. The outline widget (`Kind::Outline`, new in the toolkit)

The toolkit has no tree; a key hierarchy needs one, and Files, the element tree viewer and the
Components pane will too, so it is a widget, not a Settings trick. It follows the Table's
pattern: the tree holds **no row text** and asks the program for a row when it draws or
describes it.

```
pub struct OutlineRow { pub depth: u8, pub expandable: bool, pub expanded: bool, pub text: Text<64> }
pub type RowText = fn(data: u32, row: usize, out: &mut OutlineRow);
pub struct Outline {
    pub name: &'static str, pub rows: usize, pub row: RowText, pub data: u32,
    pub selected: usize, pub offset_y: i32, pub empty_text: &'static str,
}
Kind::outline(name, rows, row, data) -> Kind
```

The program keeps the flattened list of **visible** rows (expanded keys' children follow their
parent) and calls `set_rows` after an expand, a collapse or a refresh; the widget only draws
depth, glyph and text. `WidgetTree::selected`, `set_selected`, `set_rows` work on it as on the
table. Roles for agents: **`tree`** (name `Keys`, value `row <selected + 1> of <rows>`, flags
`focusable`, `focused`, `disabled`) with one **`treeitem`** per shown row (name the row's text,
value its full path from the program's `item_value` callback, here `HKEY_CURRENT_USER\Software\
AutoHotkey`; flags `selected`, `expanded`, and `expandable` written as `haspopup` is not: a new
flag `expandable`); `select` selects (`Chosen`), `invoke` toggles (`Toggled`), `set-text "<n>"`
selects row `n` as the table's does.

- **Geometry at 1x**: rows 24 tall (`OUTLINE_ROW_HEIGHT`, the list's), row `r` at `y + r * 24 -
  offset_y`, the vertical bar in the right 8 px when the content is taller; a row's fill spans
  the widget's width. Indent 16 per level (`OUTLINE_INDENT`). The **disclosure glyph** at `x + 8
  + depth * 16`, cell top +7: the dropdown's chevron (nine 2x2 squares) pointing **right** (6x10)
  when collapsed and **down** (10x6) when expanded; a row that is not expandable draws a 4x4
  square at `x + 11 + depth * 16`, top +10. Text at `x + 24 + depth * 16`, cell top +4,
  end-ellipsised at `right - 8` (`- 16` with the bar). Glyphs are `text.dim`; **the selected
  row's glyph is `accent`**, the non-colour cue every selectable list has (the `>` of toolkit
  section 6, which the outline cannot also draw at the mark x). A row deeper than the width
  allows still shows its glyph: the text ellipsises to nothing before the indent is clipped;
  the widget scrolls sideways later (section 14 of v0.5 has the mechanism; left out here).
- **States**: as the table's rows (normal the surface, hover `menu.hover`, selected `selection`,
  plus the ring at inset 0 when focused; disabled: `text.dim` everywhere, glyphs included).
  Hover on the selected row changes nothing. An empty outline draws `empty_text` in `text.dim`
  at `x + 24` of the first row.
- **Pointer**: a press on a row selects it (`Chosen`) and focuses the widget; a press on the
  glyph's 16x24 cell toggles (`Toggled(id, row, expanded)`) **without** changing the selection
  (regedit, Explorer and Finder all do this); a double-click on the text toggles too; a
  right-click selects first, then is passed to the window (a context menu acts on what is
  under the pointer). Wheel 72 px, rounded to whole rows.
- **Keys** (focused): Up/Down one row, Home/End, PageUp/PageDown by the visible rows minus
  one, the selection kept in view; **Right** expands a collapsed row, moves to the first child
  of an expanded one, and does nothing on a leaf; **Left** collapses an expanded row and
  otherwise moves to the parent (nothing on a root); Enter toggles (`Toggled`); Backspace moves
  to the parent; a printable character selects the next visible row whose text starts with it,
  case-insensitively, cycling; Space nothing; Tab leaves.
- **Touch** (v0.5 section 11): rows 40, glyph cell top +15, text top +12, the glyph's hit cell
  40x40. **TV**: Right on a leaf and Left on a root leave the widget in that direction (the
  D-pad's two-dimensional Tab); Enter toggles. **Wearable**: the outline draws one level only
  (the drill-down of section 7 is used instead; the widget itself is unchanged).

Damage: a toggle damages the widget; a selection change its two rows; `set_rows` the widget.
The widget costs what a list costs (`toolkit-v0.5.md` section 12's list benchmark gains a row
of 1,000 outline rows, three levels deep).

## 5. The actions: panels, validation and the confirmation

Every write goes through the same function the CLI's `windows registry set` and `delete` call,
so the words below are the only words. Panels are the in-pane modal panel of `storage-v0.3.md`
section 5 (400 wide, `menu.bg`, `INSET_R6`, 1 px `menu.border` inside, 2 px `field.bg` outside,
x 32, y `(H - height) / 2`); while one shows, the pane's widgets ignore pointer, wheel and Tab
and the outline and table draw unfocused. Every panel: title at (16, 16) in `text`, a sub-line
at (16, 36) in `text.dim` (end-ellipsised at 368), `Cancel` at (192, height - 44) and the
primary at (288, height - 44), 96x28, the primary the window's default button while the panel
shows; Esc clears a focused field or area with text, else cancels; closing the window cancels.
The status label of each panel reads a **validation** (in `text`) while the primary is disabled,
nothing while it is enabled, and the **error** (in `text`) after a write failed, the panel staying
open. On success the panel hides, the pane re-reads the key, the status line (widget 12) says
what happened in `text`, and focus returns to the button that opened the panel.

### 5.1 Edit... (widgets 13-21): double-click or Enter on a value row, `Edit...`, the context menu

| # | Widget | Place | Text |
|---|--------|-------|------|
| 13 | panel | (0, 0), 400 x height | |
| 14 | label | (16, 16) | `Edit Version` (`Edit the (Default) value`) |
| 15 | label | (16, 36) | `REG_SZ in HKEY_CURRENT_USER\Software\AutoHotkey` in `text.dim` |
| 16 | text area | (16, 64), 368x92 | strings, multi-strings and binary; placeholder `Value data` |
| 17 | text field | (16, 64), 368x28 | numbers; placeholder `Value data`, `escape: Clear` |
| 18 | radio group | (16, 100), horizontal | `Hexadecimal` `Decimal` (numbers only) |
| 19 | label | (16, 164) or (16, 136) | the validation or error line |
| 20 | push button | (192, height - 44) | `Cancel` |
| 21 | push button, primary | (288, height - 44) | `Save` |

- **Strings (`REG_SZ`, `REG_EXPAND_SZ`)**: panel height **236**, the area shown (a string value
  is often longer than a field's 255 bytes: a `Path`), wrapping as v0.5 section 5.3. Enter
  **saves** (a string holds no line break; the key is not inserted), Mod+Enter the same. The
  sub-line for `REG_EXPAND_SZ` adds nothing: `%VAR%` is kept as typed and the type is shown.
- **`REG_MULTI_SZ`**: height 236, the area shown, **one string per line**; Enter inserts a line,
  Mod+Enter saves. Empty lines are dropped on save (Windows ends the list at the first empty
  string); the sub-line reads `REG_MULTI_SZ in <key> · one string per line`.
- **Numbers (`REG_DWORD`, `REG_QWORD`)**: height **208**, the field and the radios shown. The
  field opens showing the hex digits without `0x` (regedit's base is hexadecimal by default),
  `Hexadecimal` chosen; choosing `Decimal` converts the field's current number in place and
  back, so switching never loses a value. Accepted: hex digits (a leading `0x` tolerated) or
  decimal digits, spaces trimmed. Validation: `Not a number. A REG_DWORD is 0 to 4294967295
  (0 to ffffffff in hex).` and, for `REG_QWORD`, `... 0 to 18446744073709551615 (0 to
  ffffffffffffffff in hex).`; an empty field saves 0.
- **`REG_BINARY`**: height 236, the area shown with the bytes as `4a 00 ff 1e`, 16 bytes a line
  (`4a 00 ff 1e 00 00 00 00 ...`, 47 characters, inside the 352 px wrap width). Accepted: pairs
  of hex digits; spaces, line breaks and a `0x` before a pair are ignored. Validation: `Binary
  data is pairs of hex digits, such as 4a 00 ff.`; an odd digit `Binary data has an odd number
  of hex digits.`; an empty area saves a zero-length value.
- **Too large for the panel**: data over the area's 64 KiB (a binary value over 21 KiB of text,
  a string over 64 KiB) opens the panel read-only: the area disabled showing the first 64 KiB,
  line 19 `Too large to edit here: 240 KB. windows registry set replaces it.` in `text.dim`,
  `Save` disabled. A `REG_<n>` type opens the same way with `A REG_5 value cannot be edited
  here.`
- **Save** writes the whole value; the sub-line keeps the type (changing a value's type is
  deleting and creating it; left out). The status line then reads `Saved Version.` (`Saved the
  (Default) value.`).

### 5.2 New key... and New value... (widgets 22-29): the name panel

| # | Widget | Place | Text |
|---|--------|-------|------|
| 22 | panel | (0, 0), 400 x 172 or 208 | |
| 23 | label | (16, 16) | `New key in AutoHotkey` or `New value in AutoHotkey` (the selected key's name; `in HKEY_CURRENT_USER` at a root) |
| 24 | label | (16, 36) | `Under HKEY_CURRENT_USER\Software\AutoHotkey` in `text.dim` |
| 25 | text field | (16, 64), 368x28 | placeholder `Key name` or `Value name`, `escape: Clear` |
| 26 | dropdown | (16, 100), 368x28 | name `Type`, options `REG_SZ`, `REG_EXPAND_SZ`, `REG_MULTI_SZ`, `REG_DWORD`, `REG_QWORD`, `REG_BINARY`, `REG_SZ` chosen; hidden for a key (then the panel is 172 tall and 27 moves to (16, 100)) |
| 27 | label | (16, 136) | the validation or error line |
| 28 | push button | (192, height - 44) | `Cancel` |
| 29 | push button, primary | (288, height - 44) | `Create` |

- **A key**: `Create` is disabled while the field is empty or invalid. Validation lines: `Key
  names cannot contain \ .`; `Key names are 255 characters at most.`; `A key named Settings
  already exists here.` (case-insensitive, as Windows compares); `This key is as deep as the
  registry goes (512 levels).` (shown at once, before typing, when the selected key is at the
  limit; `New key...` itself stays enabled so the reason is readable). On Create the parent is
  expanded, the new key selected, and the status line reads `Created Settings.` Nothing is
  pre-filled (regedit's `New Key #1` is a name nobody keeps).
- **A value**: an empty name is allowed and means the `(Default)` value (the sub-line then says
  `Under <key> · an empty name is the (Default) value`); `A value named Version already exists
  here.`; `Value names are 16,383 characters at most.` Create writes the type's empty data
  (`""`, no strings, 0, a zero-length value), selects the row and **opens the edit panel on it**
  in the same frame, focus in its data control, so the data is typed right away; Cancel there
  keeps the empty value, as regedit keeps the value it created. The status line after the edit
  reads `Created Count.`; after a cancel, `Created Count with no data.`

### 5.3 Delete... (widgets 30-35): the confirmation panel, 152 tall

| # | Widget | Place | Text |
|---|--------|-------|------|
| 30 | panel | (0, 0), 400x152 | |
| 31 | label | (16, 16) | `Delete AutoHotkey?` or `Delete the value Version?` (`Delete the (Default) value?`) in `text` |
| 32 | label | (16, 36) | a key: `With its 2 subkeys and 3 values.` (`With its 3 values.`, `With its 1 subkey.`, `It is empty.`; every key under it counted); a value: `REG_SZ · 2.0.19` (the row's type and data cell); `(Default)`: `It becomes (value not set).`; all in `text.dim` |
| 33 | label | (16, 56) | `This cannot be undone.` in `text.dim`; under `HKEY_CLASSES_ROOT` when the key also exists in the machine's half: `Removes your copy; the machine's stays.` |
| 34 | push button | (192, 108) | `Cancel` |
| 35 | push button, primary | (288, 108) | `Delete` |

Focus opens on `Cancel` (a destructive panel never opens with the destructive button focused,
Files' rule); Enter still presses the primary, as in every panel. After the delete, the outline
selects the parent (a key) or the table the row above (a value), and the status line reads
`Deleted AutoHotkey.` or `Deleted Version.` A root cannot be deleted: `Delete...` is disabled
while a root is selected and the table has focus on no row.

**Context menus** (right-click, Shift+F10, the Menu key; the desktop's menu chrome, 160 wide,
items 32 tall): on an outline row `New key...` `New value...` ─ `Copy path` ─ `Delete...`; on a
table row `Edit...` ─ `New value...` ─ `Copy data` ─ `Delete...`. Items a state disables are
`text.dim`. `Copy data` copies the data cell's full text (not ellipsised; up to 64 KiB).

**The CLI and MCP** write and delete the same way: `windows registry set <key> [<name>]
[--type <REG_...>] [<data>...]` creates the key when it is missing (every level), creates or
replaces the value when a name is given (`--type` defaults to the existing value's type, else
`REG_SZ`; a number as `42` or `0x2a`; a multi-string as one operand per string; binary as hex
pairs in one operand); `windows registry delete <key> [<name>] [--yes] [--dry-run]` is effect
`destroy` and asks on a terminal `Delete HKEY_CURRENT_USER\Software\AutoHotkey, 2 subkeys and 3
values? [y/N]` (`Delete the value Version from <key>? [y/N]`), refuses with `needs_confirmation`
without a terminal or `--yes`, and `--dry-run` prints `would delete <key>, 2 subkeys and 3
values`. `windows_registry_set` takes `key`, `name`, `type`, `data`; `windows_registry_delete`
takes `key`, `name`, `dry_run` and the `confirm` argument every `destroy` tool has.

## 6. States of the pane and the status line

| State | What shows | Status line (widget 12) |
|-------|------------|-------------------------|
| normal, writable key | everything enabled | `Enter edits a value, Delete removes it, F5 reads the registry again.` in `text.dim` |
| no program has written yet (`HKEY_CURRENT_USER\Software` has only the layer's own keys) | as normal | `No Windows program has saved settings yet. Keys appear here as programs write them.` in `text.dim` |
| a root selected | `Edit...` and `Delete...` disabled | `Roots are always there: add a key under one.` in `text.dim` |
| under `HKEY_LOCAL_MACHINE`, not root | 7-10 disabled; rows, cells, panels all readable; `Copy path`, `Copy data` work | `HKEY_LOCAL_MACHINE is machine-wide: only an administrator can change it.` in `text.dim` |
| under `HKEY_LOCAL_MACHINE`, root | enabled | `Changes here apply to every user of this machine.` in `text.dim` |
| under `HKEY_CLASSES_ROOT` | enabled; writes go to the user's half | `Changes here are saved in HKEY_CURRENT_USER\Software\Classes.` in `text.dim` |
| after an action | as the state | `Saved Version.`, `Created Settings.`, `Deleted AutoHotkey.`, `Copied <path>.` in `text`, until the next selection change or action |
| the machine's hive cannot be read | the `HKEY_LOCAL_MACHINE` row in `text.dim` with no glyph; selecting it shows an empty table with `empty_text` `Could not be read.` | `The machine's registry file is damaged: windows registry reset --machine replaces it (administrator).` in `text` |
| **the user's hive cannot be read** | the `HKEY_CURRENT_USER` and `HKEY_CLASSES_ROOT` rows in `text.dim`, the table empty with `Could not be read.`; 7-10 hidden, **11 `Replace file...`** shown | `Your registry file is damaged. Windows programs start with an empty registry until it is replaced.` in `text` |

A disabled button's reason is always the status line, never a tooltip alone. A pane shown
again re-reads the hive (never on a tick: a program writing while the pane shows is seen on F5
or the next action; live updates are left out).

**Replace file...** opens the confirmation panel with `Replace the damaged registry file?` /
`The damaged file is kept beside it as user.hive.damaged.` / `Windows programs' saved settings
are lost unless that file is repaired by hand.`; `Cancel`, `Replace` (primary). It runs
`windows registry reset` (effect `destroy`, `--yes` on the CLI, `confirm` over MCP; `--machine`
for the machine's hive, root only): the damaged file is renamed, an empty hive written, and the
pane re-reads it; the status line reads `Replaced the registry file. It is empty.`

**Write errors**, the same message in the panel's line, the CLI and MCP (`<what happened>:
<why>. <what to do>`), with the code and category of `command-model.md`:

| Code (category) | Message |
|-----------------|---------|
| `no_such_key` (`not_found`) | `No such key: HKEY_CURRENT_USER\Software\Nothing.` |
| `no_such_value` (`not_found`) | `No such value: Version in HKEY_CURRENT_USER\Software\AutoHotkey.` |
| `key_exists` (`exists`) | `A key named Settings already exists here.` |
| `value_exists` (`exists`) | `A value named Version already exists here.` |
| `read_only_key` (`read_only`) | `Not saved: HKEY_LOCAL_MACHINE is machine-wide. Only an administrator can change it.` |
| `root_key` (`refused`) | `Roots cannot be created, deleted or edited.` |
| `bad_name` (`usage`) | `Key names cannot contain \ .` / `Key names are 255 characters at most.` / `Value names are 16,383 characters at most.` |
| `bad_data` (`usage`) | the validation lines of 5.1 (`Not a number. ...`, `Binary data is pairs of hex digits, ...`) |
| `value_too_large` (`too_large`) | `Not saved: a value is 1 MiB at most (this one is 1.2 MB).` |
| `key_too_deep` (`too_large`) | `Not saved: keys can be 512 levels deep at most.` |
| `hive_full` (`too_large`) | `Not saved: the registry file has reached its limit of 16 MiB. Delete keys you no longer need.` |
| `disk_full` (`failed`) | `Not saved: the disk is full. Free some space and try again.` |
| `hive_damaged` (`damaged`) | `Your registry file is damaged. try: windows registry reset` |
| `write_failed` (`failed`) | `Not saved: the registry file could not be written.` |

The limits (1 MiB a value, 512 levels, 255 and 16,383 characters, 16 MiB a hive) are the hive's
constants; the implementer sets the exact numbers and the messages print the constants, so the
words and the code never disagree. Sizes use Files' `size_text`.

## 7. Every device class (D23)

- **Tablet and any pane 448 or wider with touch**: the desktop layout with rows of 40 in the
  outline and the table (header 40), buttons 40 tall, fields 28; the outline and the table
  shrink by the difference.
- **Phone (one column, a pane narrower than 448)**: a **drill-down by key**, one page per key
  showing its subkeys and its values together, as a folder view does (fewer taps than a keys
  page and a separate values page, and the value a person came for is seen without a second
  tap). Widgets keep their numbers; the layout: heading; the path field full width minus 32 at
  (16, 64); `Copy path` and `Refresh` sharing a row at (16, 100), each `(width - 40) / 2`
  wide, 40 tall; the outline at (16, 148), width minus 32, showing **one level only** (the
  selected key's subkeys, no glyphs, rows 40) with `empty_text` `No subkeys.`, as tall as its
  rows (at most 8, then it scrolls); a tap or Enter on a row **drills in** (the page becomes
  that key's; the path field changes); the phone's `Back` input, Backspace, or a tap on the
  path field's `..` row (the first row of the list, `..` in `text.dim`, absent at the three
  roots' page, whose rows are the roots) goes up one level. The table follows 8 px under the
  outline, width minus 32, header 40 and rows 40, content panned sideways by swipe, as tall as
  its rows (at most 8); then the four buttons two a row (`New key...` `New value...`, `Edit...`
  `Delete...`), each `(width - 40) / 2` x 40; then the status line. The pane's panel grows to
  its content and scrolls in the window's existing scroll view (wheel 72 px; the focused
  widget is kept in view). Panels are **bottom sheets** (`desktop-v0.3-gui.md` section 12): the
  pane's width, anchored to the bottom, 16 px padding, the controls full width, the area 132
  tall (six lines), the radios on one row, `Cancel` and the primary sharing the last row 40
  tall. Long-press on a row opens the context menu as a bottom sheet (items 48 tall).
- **TV (scale 2, D-pad)**: everything doubles; the ring is the cue; Up/Down/Left/Right move
  between widgets as v0.5 section 11 says, with the outline's own Left/Right of section 4 taking
  precedence while they can act (so Right on a leaf and Left on a root leave the widget); OK is
  Enter, `Back` is Esc (closes a panel; else returns focus to the sidebar), the `Menu` key opens
  the context menu for the focused row. No handles: the columns keep their widths.
- **Wearable (round or 200 px)**: **browsing only, no editing**: the drill-down list of the
  phone, one level a page, the table drawn as a list of its first column with the data cell
  as a second line (`Version` / `2.0.19`, rows 48) and no header; the path shown as the page's
  heading, start-ellipsised (`...\AutoHotkey`); `Back` goes up. No buttons, no panels, no
  context menu: a registry write is a destructive act whose controls (hex pairs, one string per
  line, a type dropdown) need a screen that can show the data being changed, and a wearable
  cannot, so editing is the phone's, the desktop's or the CLI's. The status line is the last
  row of the page.
- **Keyboard-only** on every form factor: the outline and the table are one Tab stop each;
  every panel needs only Tab, typing and Enter.
- **Headless** (no desktop): no pane; the CLI and MCP tools are the whole capability.

## 8. Keys, focus and pointer

- **Tab order** from the sidebar list: the path field, `Copy path`, `Refresh`, the outline, the
  table, `New key...`, `New value...`, `Edit...`, `Delete...` (disabled ones skipped; `Replace
  file...` in the damaged state), then wrapping to the search field; Shift+Tab the reverse.
  Showing the pane does not move the focus; the first time it is shown `HKEY_CURRENT_USER` is
  selected and collapsed; the selection is kept while Settings runs and reset when it is
  opened again.
- **In the outline**: section 4's keys; a selection change re-reads the key's values and sets
  the path field, the table and the status line in the same frame; Delete opens the
  confirmation for the key; Mod+C copies the path; F2 does nothing (rename is left out).
- **In the table**: Up/Down, Home/End, PageUp/PageDown; Enter or a double-click opens `Edit...`
  on the row; Delete opens the confirmation for the value; Mod+C copies the data cell; Left and
  Right scroll the columns.
- **Anywhere in the pane**: F5 refreshes; Mod+N opens `New key...` when the outline has focus
  and `New value...` when the table has (the two buttons carry those as their tooltip's
  shortcut: `New key... Mod+N`); Esc goes to the window, which ignores it. No other shortcuts.
- **Pointer**: `Arrow` everywhere but the field and the areas (`IBeam`) and the table's handles
  (`ResizeEW`); a click selects and focuses; a right-click selects, then opens the context menu.
- **Tooltips** (v0.5 section 2) on the four buttons and `Copy path`/`Refresh`: `Create a key
  under the selected key`, `Create a value in the selected key`, `Edit the selected value`
  (`Enter`), `Delete the selected key or value` (`Delete`), `Copy the selected key's path`
  (`Mod+C`), `Read the registry again` (`F5`).

## 9. What an agent sees (D161, `wm show`)

The path field is a `text field` named `Key path` with the path as its value; `Copy path` and
`Refresh` are `button`s; the outline a `tree` named `Keys` with its shown rows as `treeitem`s
named by the key name and valued by the full path (section 4); the table a `table` named
`Values` with `columnheader`s `Name`, `Type`, `Data` and one `row` per shown value named by its
name and valued `Version\tREG_SZ\t2.0.19`; the four buttons `button`s, disabled ones flagged;
the status line a `label`; the panels' controls by their placeholders and names (`Value data`,
`Key name`, `Value name`, `Type`, `Hexadecimal`, `Decimal`) and their buttons `Cancel`, `Save`,
`Create`, `Delete`, `Replace`. An agent that wants the registry calls `windows_registry_get`,
`_set` and `_delete`, not the tree (D22: the tree is for operating the window); a `set` or
`delete` through MCP needs the agent's `write` grant, and `delete` and `reset` confirm as every
`destroy` tool does (D21).

## 10. Acceptance (host tests, default 640x440, scale 1x, the `windows` component installed, a
fake hive with `HKEY_CURRENT_USER\Software\AutoHotkey` holding the values `(Default)` unset,
`Version` `REG_SZ` `2.0.19`, `Count` `REG_DWORD` 42, `Paths` `REG_MULTI_SZ` `a`, `b`, `Blob`
`REG_BINARY` `4a 00 ff 1e`, and the subkeys `Settings` and `Window`)

- `PANES` holds `Windows` at index 9; its row is hidden when the component is absent and
  `Components` is then at y 332; `registry`, `regedit`, `HKCU`, `REG_SZ` and `hive` match it and
  nothing else; `Windows` matches it and Appearance.
- In window coordinates: heading (192, 16); the path field (192, 64, 232, 28) reading
  `HKEY_CURRENT_USER`; `Copy path` (432, 64), `Refresh` (528, 64); the outline (192, 100, 176,
  172) with the three roots, the first selected, its chevron at (200, 107) pointing right in
  `accent`; the table (376, 100, 248, 172) with the header's `Name`, `Type`, `Data` and one row
  `(Default)` `REG_SZ` `(value not set)`; the buttons at (192, 280), (296, 280), (400, 280),
  (504, 280), `Edit...` and `Delete...` disabled; the status line at (192, 316) reading the
  roots line in `text.dim`.
- Right expands `HKEY_CURRENT_USER` (`Toggled`, `Software` at depth 1 with its chevron at
  (216, 131)); typing `HKEY_CURRENT_USER\Software\AutoHotkey` in the field and Enter selects
  it, expanded down to it, and the table shows five rows, `(Default)` first then `Blob`,
  `Count`, `Paths`, `Version`, `Count`'s data `0x0000002a (42)`, `Paths`'s `a · b`, `Blob`'s
  `4a 00 ff 1e`; Tab, Down to `Count`, Enter opens the edit panel at (208, 70, 400, 208) with
  `Edit Count`, the field reading `2a`, `Hexadecimal` chosen; choosing `Decimal` shows `42`;
  typing `x` disables `Save` with the `Not a number.` line; `43` and Enter saves, the row reads
  `0x0000002b (43)`, the status line `Saved Count.`, and the fake's `set` was called once with
  `REG_DWORD` 43.
- `New key...` opens the name panel at (208, 88, 400, 172); `Create` disabled; `Set\tings` with
  a backslash shows the `cannot contain \` line; `Settings` shows the exists line; `Themes` and
  Enter creates it, selects it in the outline at depth 3, and the status line reads `Created
  Themes.` `New value...` with the name `Flag`, type `REG_DWORD`, Enter creates it and opens the
  edit panel on it in the same frame; Esc there leaves the row `Flag` `REG_DWORD` `0x00000000
  (0)` and the status line `Created Flag with no data.`
- Delete on the outline with `AutoHotkey` selected opens the confirmation at (208, 98, 400,
  152) with `Delete AutoHotkey?` / `With its 3 subkeys and 5 values.` / `This cannot be undone.`,
  focus on `Cancel`; Enter deletes, `Software` is selected, the status line reads `Deleted
  AutoHotkey.`; `windows registry get HKCU\Software\AutoHotkey` then answers `no_such_key`.
- A non-root user with a key under `HKEY_LOCAL_MACHINE` selected: the four buttons disabled and
  the machine-wide line; `Copy path` still copies. A fake whose user hive fails to parse shows
  the two dim roots, `Replace file...` at (192, 280, 136, 28), the damaged line in `text`; its
  panel and Enter call `reset` once and the status line reads `Replaced the registry file. It is
  empty.` A fake whose `set` fails with `disk_full` keeps the edit panel open with the disk-full
  line.
- `windows registry get`, `windows_registry_get` and the table agree cell for cell on the fake
  (`data_text` is one function); `windows registry get HKCU\Software\AutoHotkey Count` prints
  `Count  REG_DWORD  0x0000002a (42)`; `--json` gives `"data": 42`.
- A pane 360 wide (phone): the outline at (16, 148) shows `Settings`, `Window` at one level with
  the `..` row first; a tap on `Settings` changes the path field and the list; `Back` returns;
  the buttons two a row at y under the table; the edit panel is a sheet at the pane's bottom. A
  wearable fake shows no buttons and the table as a two-line list.
- The element tree holds the `tree` `Keys` with three `treeitem`s, the first `HKEY_CURRENT_USER`
  valued `HKEY_CURRENT_USER` and flagged `selected`, `expandable`; after Right, flagged
  `expanded` with `Software` below it.

## Decided by design (the owner can overrule any of these)

- Three roots shown, `HKEY_USERS` and `HKEY_CURRENT_CONFIG` hidden (section 3).
- A new toolkit widget, `Kind::Outline`, rather than a Settings-only tree (section 4).
- Strings edit in a text area, numbers in a field with a hex/decimal radio, binary as hex
  pairs in a text area; Enter saves a string and inserts a line in a multi-string (5.1).
- `New value...` creates the empty value and opens its edit panel at once; Cancel keeps the
  empty value, as regedit does (5.2).
- One page per key on a phone, subkeys and values together; browsing only on a wearable (7).
- The pane sits right before `Components` and is hidden, not greyed, without the layer (1).

## Left out of this slice

Rename (F2) of keys and values, since neither the CLI nor MCP has it yet and a rename moves
every value and subkey (it lands in all three front-ends together); changing a value's type;
find (Mod+F) and `windows registry find`; favourites; import and export of `.reg` files;
`HKEY_USERS` and `HKEY_CURRENT_CONFIG` rows; a draggable splitter between the outline and the
table; the outline scrolling sideways; live updates while a program writes (F5 instead);
editing on a wearable; permissions per key; `REG_<n>` types' editing; a middle ellipsis for
long paths in the field; more than 1,000 subkeys in the outline (the CLI lists them); binary
data over 21 KiB and strings over 64 KiB in the panel (the CLI replaces them).
