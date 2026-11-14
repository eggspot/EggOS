# EggOS toolkit v0.6: submenus and checkable items, the menu bar's overflow, table column reordering, fit-to-content, multi-selection and cell focus, the text area's undo and word moves: design brief

Design by Claude Fable (D16) for five of `toolkit-v0.5.md` section 14's later slices.
Everything in `toolkit-v0.3.md`, `toolkit-v0.4.md` and `toolkit-v0.5.md` stands (the menu of
section 3, items 32 tall, separators 9, text at + 12, `menu.hover`, disabled `text.dim`; the
table of section 4; the text area of section 5; the roles of section 10) except where a
section below says otherwise. Numbers are px at 1x and double at 2x; the font is Egg Sans
(`File` 15, `Edit` 17, `View` 22, `Help` 20, `»` 7 wide). "Later" marks what this slice does
not build.

## 0. What a person gets, and how that is checked

- Menus with **submenus** (a `>` at the right, opening to the side), **check items** (a tick)
  and **radio items** (a dot), in menu bars, menu buttons and context menus.
- A menu bar whose titles do not fit shows **`»`** at its right end holding the rest.
- Tables: **drag a header to reorder** columns, **double-click a divider** to fit a column,
  **Ctrl/Shift+click and keys** to select several rows, and **cell focus** by Left/Right.
- Text fields and areas: **undo and redo** (Mod+Z, Mod+Shift+Z, Ctrl+Y) with typing grouped,
  and **word moves** (Ctrl+arrows; Alt+arrows in the macOS profile) with Shift and deletion.
- Every new state is in the element tree and every action has an agent's way (section 5);
  host tests pin each (section 7).

## 1. Submenus, check items, radio items

### 1.1 Entries

```
pub enum Entry {
    Item { label, key, enabled, id },
    Separator,
    Submenu { label: &'static str, entries: &'static [Entry], enabled: bool },
    Check { label, key, enabled, id, checked: bool },
    Radio { label, key, enabled, id, selected: bool },
}
```

`Check` and `Radio` are items with a mark; the program sets `checked` and `selected` (the
menu never toggles them itself: choosing returns `Invoked(id)` as for an `Item`, and the
program's state, which the menu is a view of, decides). `WidgetTree::set_entry_checked(id,
entry_id, bool)` and `set_entry_selected` change a mark in a static-looking menu through the
tree's per-widget override table (`MENU_MARK_OVERRIDES` 32 per tree), so a `View > Syntax
Colours` tick follows the setting. A `Radio` group is the run of consecutive `Radio` entries
between separators or other kinds; the program keeps one selected.

### 1.2 The mark column

A menu that holds any `Check` or `Radio` entry gets a **mark column**: every item's text moves
from x + 12 to **x + 32** (`MENU_MARK_COLUMN` 20), submenus and keys unchanged, and the menu's
width grows by 20. A menu without such entries keeps x + 12 (the Files and Editor menus do
not shift). Windows and macOS give the whole menu the column; a mark that pushes one line
alone would misalign the labels.

- **The tick**: 8x8 at (x + 12, row + 12), 1 px lines in `text`: a stroke from (0, 4) to
  (2, 6), then from (2, 6) to (7, 1), drawn as pixels (0, 4), (1, 5), (2, 6), (3, 5), (4, 4),
  (5, 3), (6, 2), (7, 1). Disabled: `text.dim`.
- **The dot**: a 6x6 filled circle (`INSET_R3` rows: insets 2, 1, 0, 0, 1, 2) at (x + 13,
  row + 13) in `text`; disabled `text.dim`.
- A selected radio or checked check on the hovered row keeps `text` on `menu.hover`.

### 1.3 Submenus

- **Look**: the label as an item's, and a **chevron** pointing right, 6x10 (the dropdown's
  nine 2x2 squares rotated: columns 0..5, the point at the right), in `text` (`text.dim` when
  disabled), its right edge at `menu.right - 12`, top at row + 11. A submenu shows no key.
- **Opening by pointer**: hovering a `Submenu` row for `SUBMENU_OPEN_MS` **150** opens it (a
  click opens at once); moving to another row of the parent closes it after the same 150 ms,
  unless the pointer went into the submenu. The hover delay gives a diagonal move toward the
  submenu time to land (no safe-triangle geometry: later).
- **Place**: left edge at the parent's right - 1 (the two borders overlap), top at the row's
  top - 1 (the border row), so the submenu's first row aligns with its parent row; clamped by
  `Menu::new` inside the bounds; when it would pass the right bound it opens to the **left**
  (right edge at the parent's left + 1), and when it would pass the bottom it moves up. Depth
  at most `MENU_DEPTH` **3** (a bar's menu, a submenu, a submenu of that); a deeper `Submenu`
  is drawn disabled.
- **Keys**: Right on a highlighted `Submenu` opens it with its first enabled item highlighted;
  Enter and Space the same; Left in a submenu closes it and highlights its parent row; Esc
  closes the innermost; in a **bar** Left/Right at the parent level still switch titles, and
  from a submenu's first level Left closes the submenu rather than switching (the Windows
  rule). A mnemonic in a submenu's parent applies to the parent level only.
- **Drawing**: each open level is a `menu::Menu` drawn after the one below it; `Menu` gains
  `open_child: Option<(usize, Menu)>` boxed in the tree's menu pool (`MENU_POOL` 3 per tree).
  A disabled `Submenu` neither opens nor highlights its chevron.

## 2. The menu bar's overflow

When the titles' hit rects (`width + 24` each) do not fit the bar's width, the bar shows the
**overflow title `»`** at its right end and hides the titles that do not fit before it:

- The overflow title's rect is **31 wide** (12 + 7 + 12), its text `»` at + 12, at the bar's
  right edge (`bar.right - 31`). Titles are placed left to right; the first title whose rect
  would pass `bar.right - 31` and every title after it go into the overflow. With every title
  fitting, no `»`. A width change re-lays the bar in the same frame (`relayout`).
- `»`'s menu is a menu of **`Submenu` entries**, one per hidden title (its label the title
  without `&`, its entries the title's menu), using section 1 unchanged: the hidden `View`
  menu is one submenu to the right. The menu opens below `»`, right-aligned to it (left-opening
  submenus follow since there is no room on the right).
- Keys: `»` is the last title for Left/Right (wrapping), Down opens it; a hidden title's
  mnemonic (`Alt+V`) opens its menu **directly at the `»` position**, skipping the overflow
  level (the person asked for View, not for the overflow); F10 arms the first shown title.
- In the macOS profile the window bar is not drawn (v0.5 3.4) and the top bar is the server's,
  which cuts titles as it does today: no overflow there in this slice.
- The element tree lists hidden titles under the bar as before (nothing is hidden from an
  agent); the `»` is a `menuitem` named `More` with `expandable`.

## 3. Table: reordering, fit, multi-selection, cell focus

### 3.1 Column reordering by drag

- `Table` gains `order: [u8; MAX_COLUMNS]` (visual position to column index, identity at
  start). **Every `cell` call and `sort` keep the column's original index**; only drawing,
  hit testing and the tree use `order`. So a program needs no change: Files' `Size` stays
  column 1 wherever it is shown.
- A press on a header cell (not a handle) followed by a move of **8 px** (`DRAG_THRESHOLD`,
  the v0.5 drag's) sideways starts a column drag: the header cell is drawn as a **ghost**
  (`menu.hover` fill, the title in `text`, 28 tall, the cell's width) at the pointer's x minus
  the grab offset, over the header, and a **2 px `accent` insertion line** the body's full
  height at the nearest column boundary to the pointer (the dragged column's own boundaries
  excluded). Release reorders (`order` rotated) and returns **`Response::ColumnMoved(id, from,
  to)`** with visual positions, so a program that saves an order saves it once; Esc or a release
  outside the header cancels; a release without a move is the click it was (sort). A table
  with one column never drags. Not sortable columns drag too.
- The mark column (`>`) and `TABLE_FIRST_TEXT_X` belong to the **visual first** column.

### 3.2 Fit-to-content

A double-click on a handle (two presses within `DOUBLE_CLICK_MS` on the same handle) sets the
column's width to the widest of its title (+ 16, + 20 when sorted) and its cells (+ 16; + 24
more for the visual first column's mark) over the **shown rows and the first
`FIT_SAMPLE_ROWS` 1000 rows**, clamped to `min_width` and `TABLE_MAX_FIT` 640, then returns
`ColumnResized`. A million-row table stays cheap (1000 `cell` calls, each a 64-byte text), and
the sample is the rule, said here, not a surprise. A program wanting an exact fit calls
`set_column_width` itself.

### 3.3 Multi-selection

```
pub struct Table { ..., pub multi: bool, pub anchor: usize, pub is_selected: Option<fn(data, row) -> bool> }
Kind::table_multi(name, columns, rows, cell, is_selected, data) -> Kind
```

- **The program owns the set**, as it owns cell text (a million rows cannot have a bit each
  in a heapless tree): the tree asks `is_selected(data, row)` when it draws a row or describes
  it, and tells the program what the person did with **`Response::Selection(id,
  SelectionChange)`**: `Only(row)`, `Toggle(row)`, `Range(anchor, row)` (inclusive, either
  order), `All`, `None`. The program updates its set and the tree redraws (`set_rows` or
  `damage`). `selected` becomes the **current row** (the one with the ring and the `>`), and
  `anchor` the row a range starts from.
- **Pointer**: a plain press `Only(row)` (and current = anchor = row); **Ctrl+press** (Cmd in
  the macOS profile) `Toggle(row)` and current = anchor = row; **Shift+press** `Range(anchor,
  row)` and current = row, anchor kept; a press on a selected row with no modifier that
  becomes a drag (section 7 of v0.5) drags the whole selection (`DragStarted` carries the
  current row; the program reads its set); a right-click on an unselected row is `Only` first.
  A double-click or Enter `Opened(id, current)`; the program opens its set.
- **Keys**: Up/Down and the rest move current and `Only` it; **Shift+Up/Down/Home/End/
  PageUp/PageDown** move current and `Range(anchor, current)`; **Ctrl+Up/Down** move current
  alone (nothing selected; the ring shows where you are); **Ctrl+Space** `Toggle(current)` and
  anchor = current; **Mod+A** `All` (a table with the focus takes it before the window's own
  Mod+A); Esc `None` when more than one row is selected, else goes to the window.
- **Drawing**: every selected row `selection`; the current row adds the `>` and, focused, the
  ring; a selected row that is not current has no mark. The table's value for an agent is
  `row 3 of 1000, 4 selected`.
- `multi` false keeps v0.5's behaviour exactly (`Chosen`), so Settings' tables change nothing.

### 3.4 Cell focus

`Table.cell_focus: bool` (`set_cell_focus(id, bool)`; off by default): when on, **Left and
Right move the focused cell** (`column: usize`, visual) instead of scrolling by 48, Home/End
with Ctrl go to the first and last column, the ring is drawn on the **cell** (`x_c .. x_c +
width_c` of the current row, inset 0) instead of the row, and the table scrolls `offset_x` to
keep the cell in view. `Response::CellFocused(id, row, column)` on each move (original column
index); Enter is still `Opened`; the program reads `focused_column(id)` to act on the cell
(the registry pane's value edit, later). A click sets the cell under the pointer current when
`cell_focus` is on. Wheel and Shift+wheel unchanged.

## 4. Text field and text area: undo, redo, word moves

### 4.1 Undo and redo

- Keys: **Mod+Z** undo, **Mod+Shift+Z** redo everywhere and **Ctrl+Y** redo in the Windows and
  Linux profiles (the Text Editor's keys, `text-editor-v0.3.md` section 10). Responses
  `Edited(id)` as any edit; `Undone`/`Redone` are not distinct responses (the program sees the
  text).
- **Grouping** (the Text Editor's rules, so the two agree): typed printable characters coalesce
  into one step, as does a run of Backspace or Delete; Enter, Tab, a paste, a cut, a word
  deletion and any caret move by key or click end the open step; a new edit clears the redo
  list; undo and redo restore the caret and selection of before. One addition: a pause of
  more than `UNDO_GROUP_MS` **1000** between keystrokes ends the step (so a sentence typed in
  bursts undoes in bursts).
- **Storage**: per text store, a ring of `TEXT_AREA_UNDO_STEPS` **64** steps and
  `TEXT_AREA_UNDO_BYTES` **8192** bytes of removed and inserted text (`text::Store` grows by
  them; a tree without areas still pays nothing); the oldest steps drop when either runs
  out; a single edit larger than 8192 bytes (a big paste) is applied and **not recorded**, and
  clears the history (a half-recorded history would restore the wrong text). The text field
  gets a small ring of its own: `FIELD_UNDO_STEPS` **16**, `FIELD_UNDO_BYTES` **512** per
  field, in the tree's field table.

### 4.2 Word moves and word deletion

| Keys (Windows and Linux profiles) | macOS profile | Does |
|-----------------------------------|---------------|------|
| Ctrl+Right | Alt+Right | to the **end of the current or next word** |
| Ctrl+Left | Alt+Left | to the **start of the current or previous word** |
| Ctrl+Shift+Right / Left | Alt+Shift+Right / Left | the same, extending the selection from `anchor` |
| Ctrl+Backspace | Alt+Backspace | deletes to the start of the word (one undo step) |
| Ctrl+Delete | Alt+Delete | deletes to the end of the word |

A **word** is a run of letters, digits and `_` (`char::is_alphanumeric`), a run of other
non-space characters is a word of its own (`->`, `...`), and spaces are skipped first in the
direction of travel; a line break ends a word. One rule in both profiles, macOS's and GTK's
("to the end of the word"), rather than Windows's "to the start of the next word": the
field and the area share one function with the double-click's word rule. In the macOS profile
Mod+Left/Right stay the line's start and end (v0.5 5.4) and Ctrl+arrows reach the program.

## 5. What an agent sees (D161, `wm show`)

- A **`Check`** entry: `menuitem` with flag `checked` when checked; a **`Radio`** entry:
  `menuitem` with flag `selected`; `invoke` on either is `Invoked(id)` as a click.
- A **`Submenu`** entry: `menuitem` with `expandable`, `expanded` while open, its entries as
  children listed whether or not it is open (`invoke` opens or closes it; an agent invokes a
  child directly, which chooses it).
- The **overflow**: a `menuitem` named `More` with `expandable`; the hidden titles stay the
  bar's children as before, each `expandable`.
- A **table**: value `row 3 of 1000, 4 selected` when `multi`; each shown `row` flagged
  `selected` by `is_selected`; `columnheader`s in **visual order**, each with its original
  index as `... ` nothing new (the name is the title); `set-text "<position>"` on a header
  **moves the column** to that visual position (`ColumnMoved`), and `set-text "fit"` fits it;
  `select` on a row with `multi` is `Only`; `set-text "toggle"` on a row is `Toggle`;
  `set-text "all"` on the table is `All`; a focused cell is told by the row's value
  `cell 2` appended; `set-text "cell:<n>"` on the table focuses column `n`.
- Undo and word moves are keys: `wm press` with `Ctrl+Z` reaches them; nothing new to list.

## 6. Device classes (D23)

- **Touch**: a submenu opens on tap (no hover delay); rows 40 tall as v0.5's touch menus; a
  long press on a header starts the column drag; multi-selection by a long press entering
  selection mode is later (Ctrl/Shift need a keyboard); fit by a double-tap on the handle's
  40 px zone.
- **TV**: the remote's Right opens a submenu, Left closes it; `»` is reached by Right past the
  last title; tables select one row (multi needs modifiers; later a `Select` key).
- **Keyboard only**: everything above has keys; column reordering by keyboard is the
  program's `View > Columns` menu (Files gains one, later), or `wm_act`'s `set-text` for an
  agent.
- **Phone**: menus are the same popups inside the maximised window; a menu bar always
  overflows on a 390 px width past three titles, which is what `»` is for.

## 7. Acceptance (host tests, scale 1x, dark palette, window content 480x360)

- A menu with a `Check` draws every item's text at x + 32, the tick's 8 pixels at (x + 12,
  row + 12) for a checked one, the dot's 6x6 for a selected `Radio`; `set_entry_checked`
  flips it; `invoke` and a click return `Invoked(id)` and change no mark; a menu without
  marks keeps x + 12.
- A `Submenu` row draws the 6x10 chevron ending at `right - 12`; hover 149 ms opens nothing,
  150 ms opens the submenu at (parent.right - 1, row.top - 1); near the right bound it opens
  to the left; Right/Enter open with the first enabled item highlighted, Left closes, Esc
  closes the innermost; depth 4 is drawn disabled; a click on a submenu's item returns
  `Invoked`.
- A bar with `File Edit View Help` in a 120 px window shows `File`, `Edit` and `»` at
  (89, 0, 31, 28); its menu lists `View` and `Help` as submenus opening to the left; Alt+V
  opens View's menu at `»`; widening to 200 removes `»`; the tree lists `More` `expandable`.
- Table: a press on `Size`'s header and a move of 8 px shows the ghost and the insertion line;
  a release past `Modified` returns `ColumnMoved(id, 1, 2)` and `cell` is still called with
  column 1 for the `Size` text; Esc cancels; a double-click on `Name`'s handle returns
  `ColumnResized` with the widest of 1000 sampled cells + 16 + 24; `multi` Ctrl+click returns
  `Toggle(row)`, Shift+click `Range`, Shift+Down `Range(anchor, current)`, Ctrl+Space
  `Toggle(current)`, Mod+A `All`, Esc `None`; the drawn rows follow a fake `is_selected`; the
  value `row 3 of 1000, 4 selected`; `cell_focus` Right rings column 1 and returns
  `CellFocused`; `set-text "2"` on a header moves it; `set-text "all"` selects all.
- Text: typing `abc`, waiting 1001 ms, typing `def`, Mod+Z leaves `abc`, Mod+Z leaves ``,
  Mod+Shift+Z and Ctrl+Y restore; a 9000-byte paste is applied, not undoable, and clears the
  history; Backspace x3 is one step; a click ends the step; the field's 16-step ring; word
  moves on `foo_bar->baz  qux` from the start: Ctrl+Right to 7, 9, 12, 17; Ctrl+Left back;
  Ctrl+Shift+Right selects `foo_bar`; Ctrl+Backspace from the end deletes `qux`; the macOS
  profile uses Alt and Ctrl+Right reaches the program.

## Decided by design (the owner can overrule any of these)

Marks are the program's state, the menu a view of it; the mark column is per menu and 20 px;
submenus open on a 150 ms hover with no safe triangle, overlap the parent's border by 1 px and
nest 3 deep; the overflow is a menu of submenus and a hidden title's mnemonic skips it;
reordering keeps original column indexes so programs change nothing; fit samples 1000 rows;
multi-selection is program-owned with a `SelectionChange`; cell focus is opt-in; undo grouping
is the Text Editor's plus a 1 s pause, 64 steps and 8 KiB per area, big pastes not recorded;
word moves are "to the end of the word" in every profile.

## Later

A safe triangle for submenus; tearing a column off to hide it and a `Columns` menu in Files;
rubber-band selection and touch selection mode; editing a focused cell in place; a tree table;
selection in the element tree; the macOS top bar's overflow; undo across a `set_text`.

## Decisions for the engineer

1. `Entry::Submenu { label, entries, enabled }`, `Entry::Check { ..., checked }`,
   `Entry::Radio { ..., selected }`; `set_entry_checked`, `set_entry_selected`;
   `MENU_MARK_OVERRIDES` 32 per tree.
2. `MENU_MARK_COLUMN` 20: text at x + 32 in a menu with marks; the tick's pixels (0, 4),
   (1, 5), (2, 6), (3, 5), (4, 4), (5, 3), (6, 2), (7, 1) at (x + 12, row + 12); the dot 6x6
   `INSET_R3` at (x + 13, row + 13).
3. Submenu chevron 6x10 ending at `right - 12`, top row + 11; `SUBMENU_OPEN_MS` 150 (open and
   close); place (parent.right - 1, row.top - 1), left-opening at (parent.left + 1 - width);
   `MENU_DEPTH` 3; `MENU_POOL` 3 open menus per tree; keys Right/Enter/Space open, Left
   closes, Esc innermost.
4. Overflow title `»` 31 wide at `bar.right - 31`; titles placed until one would pass it; its
   menu is `Submenu` entries of the hidden titles; element `More`, `expandable`; hidden
   mnemonics open directly.
5. `Table.order`, `ColumnMoved(id, from, to)` with visual positions; `cell` and `sort` keep
   original indexes; drag threshold 8 px; ghost `menu.hover` 28 tall; insertion line 2 px
   `accent`, body height; Esc cancels.
6. Fit: double-click on a handle; width = max(title + 16 (+ 20 sorted), cells + 16 (+ 24 for
   the visual first column)) over shown rows and `FIT_SAMPLE_ROWS` 1000, clamped to
   `min_width` and `TABLE_MAX_FIT` 640; returns `ColumnResized`.
7. `Table.multi`, `anchor`, `is_selected`; `Response::Selection(id, SelectionChange { Only,
   Toggle, Range(a, b), All, None })`; the modifiers and keys of 3.3; the value
   `row <n> of <rows>, <k> selected`.
8. `Table.cell_focus`, `set_cell_focus`, `focused_column`, `Response::CellFocused(id, row,
   column)`; Left/Right move the cell, Ctrl+Home/End first and last column; the ring on the
   cell.
9. Undo: Mod+Z, Mod+Shift+Z, Ctrl+Y (Windows and Linux profiles); `UNDO_GROUP_MS` 1000;
   `TEXT_AREA_UNDO_STEPS` 64, `TEXT_AREA_UNDO_BYTES` 8192; `FIELD_UNDO_STEPS` 16,
   `FIELD_UNDO_BYTES` 512; an edit over the byte budget is applied, unrecorded, and clears the
   history.
10. Word moves: Ctrl+arrows (Alt in the macOS profile), Shift extends, Ctrl/Alt+Backspace and
    Delete delete a word; a word is alphanumerics and `_`, other non-space runs are words,
    spaces skipped first, line breaks end words; "to the end of the word" both ways.
11. Agent faces of section 5: `checked`, `selected`, `expandable`/`expanded`, `More`,
    `set-text "<position>"`, `"fit"`, `"toggle"`, `"all"`, `"cell:<n>"`.
