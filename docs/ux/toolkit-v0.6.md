# EggOS toolkit v0.6: submenus, check and radio items, the menu bar's overflow, table column reordering, fit-to-content, multi-selection and cell focus, undo and word moves in text: design brief

Design by Claude Fable (D16) for the items `toolkit-v0.5.md` section 14 left for later. It
extends `toolkit-v0.3.md`, `toolkit-v0.4.md` and `toolkit-v0.5.md`, and **everything that
exists stays as it is**: `menu::Entry::Item` and `Separator`, `Menu`, `Bar` and `Button` with
their geometry (items 32 tall, separators 9, text at + 12, `INSET_R6`, at least 160 wide in
steps of 8), the table's `Column`, `CellText`, `Sorted`, `ColumnResized`, `Chosen`, `Opened`
and its agent actions (`set-text "<width>"` on a header, `set-text "<row>"` on the table), the
text area's store, wrap pass and keys, the field's keys, the roles and flags of
`eggos_element_tree`, and every element id an agent holds today. A program that uses none of
the new things draws and answers exactly as before; section 9's first case pins that.
Numbers are px at 1x and double at 2x; the font is Egg Sans (`File` 15, `Edit` 17, `View` 22,
`Tools` 26, `Help` 20, `...` 9 wide). Colours are tokens of `self-drawn-themes-v0.1.md`
section 2, in all four palettes (section 1). No motion is added: a submenu, a ghost and an
undo appear in the frame that causes them.

## 0. What a person gets, and how that is checked

- Menus with **submenus** (an arrow at the right, opening to the side), **check items** (a
  tick) and **radio items** (a dot), in menu bars, menu buttons, context menus and the macOS
  profile's top bar.
- A menu bar wider than its window shows **`...`** at its right end, holding the titles that
  do not fit.
- Tables: **drag a header** to reorder columns, **double-click a column's edge** to fit it to
  its content, select **several rows** (Mod+click, Shift+click, Shift+arrows, Mod+A), and move
  a **focused cell** with Left and Right.
- Text fields and text areas: **undo and redo** (Mod+Z, Mod+Shift+Z, Mod+Y) with typing in
  groups, and **word moves and word deletion** (Mod+Left/Right, Mod+Backspace/Delete; Alt in
  the macOS profile, where Cmd+Left/Right are the line's ends).
- Every new state is in the element tree and every new action has an agent's way (section 6);
  host tests pin pixels, responses and elements (section 9).

## 1. Tokens and constants

No new colour token. What each new mark is drawn with, and its contrast in the four palettes
(Dark / Dark high contrast / Light / Light high contrast; WCAG, computed from the values):

| Mark | Token on token | D / DH / L / LH |
|------|----------------|-----------------|
| tick, dot, submenu arrow, `...` | `text` on `menu.bg` | 12.9 / 21.0 / 16.0 / 21.0 |
| the same on the highlighted row or title | `text` on `menu.hover` | 10.5 / 15.1 / 14.0 / 16.8 |
| a disabled item's tick, dot or arrow | `text.dim` on `menu.bg` | 5.1 / 11.4 / 5.4 / 9.6 |
| the column ghost's title | `text` on `menu.hover` | 10.5 / 15.1 / 14.0 / 16.8 |
| the dragged column's own header | `text.dim` on `button.pressed` | 4.9 / 4.6 / 4.5 / 6.8 (momentary) |
| the column insertion line | `accent` on `menu.bg` | 8.4 / 14.6 / 4.7 / 7.2 |
| a selected row's text | `text` on `selection` | 7.5 / 11.4 / 12.9 / 15.7 |
| a selected row's `>` | `accent` on `selection` | 4.9 / 7.9 / 3.8 / 5.4 |
| the cell's ring | `focus.ring` on `selection` / on `menu.bg` | 4.9 / 7.9 / 3.8 / 5.4 and 8.4 / 14.6 / 4.7 / 7.2 |

Text rests at 4.5:1 or better (7:1 in the high-contrast columns); graphics at 3:1 or better.
The ghost's and a submenu's edge is `menu.border`, which is white or black in the
high-contrast palettes, so both read without their fill.

New constants, at 1x. `menu`: `MARK_COLUMN 20`, `MARK_X 12`, `TICK 8`, `DOT 6`,
`ARROW_WIDTH 6`, `ARROW_HEIGHT 10`, `SUBMENU_OPEN_MS 150`, `DEPTH 3`, `OVERFLOW_TITLE "..."`,
`OVERFLOW_NAME "More"`. `widget`: `MENU_MARKS 32`, `TABLE_FIT_SAMPLE_ROWS 1000`,
`TABLE_MAX_FIT 640`, `UNDO_GROUP_MS 1000`, `TEXT_AREA_UNDO_STEPS 64`, `TEXT_AREA_UNDO_BYTES
8192`, `FIELD_UNDO_STEPS 16`, `FIELD_UNDO_BYTES 512`. `input::key`: `Y 21`. The column drag
uses `DRAG_THRESHOLD` (6), `DRAG_EDGE` (24) and `DRAG_SCROLL_MS` (100) as they are, and the
pointer shape `Hand`.

## 2. Submenus, check items, radio items

### 2.1 Entries

```
pub enum Entry {
    Item { label, key, enabled, id },                         unchanged
    Separator,                                                unchanged
    Check { label: &'static str, key: &'static str, enabled: bool, id: u8, checked: bool },
    Radio { label: &'static str, key: &'static str, enabled: bool, id: u8, selected: bool },
    Submenu { label: &'static str, enabled: bool, entries: &'static [Entry] },
}
```

- `Check` and `Radio` are items with a mark. **The mark is the program's state and the menu
  is a view of it**: choosing one returns `Response::Invoked(widget, id)` as an `Item` does and
  closes the menu; the menu never flips a mark itself, so a refused change never shows a wrong
  tick. `checked` and `selected` in the entry are the starting values; the program changes
  them with `WidgetTree::set_entry_checked(widget, entry_id, bool)` and
  `set_entry_selected(widget, entry_id)`, kept in the tree's table of `MENU_MARKS` (32)
  overrides, keyed by widget and entry id (ids are already unique per bar or button, since
  `Invoked` carries them). A **radio group** is a run of `Radio` entries with nothing else
  between them; `set_entry_selected` selects one and clears the rest of its run.
- `Submenu` has no id and no key: it only opens. A submenu's label takes a mnemonic like any
  label. A `Submenu` below the third level (`DEPTH` 3: a title's menu, a submenu, a submenu of
  that) is drawn disabled and never opens.
- **`menu::Stack`** holds the open levels (`levels: [Menu; DEPTH]`, `depth`, the row each
  level was opened from, and the pending hover). The tree's bar and button and the programs'
  own context menus (Files, the Text Editor) drive the same `Stack`: `hover`, `press`,
  `release`, `key`, `tick`, `paint`. `Menu` itself is unchanged apart from drawing the new
  kinds.

### 2.2 Geometry at 1x

A row of any new kind is an item's row: 32 tall, its text cell at row + 8.

- **Mark column.** A menu that holds at least one `Check` or `Radio` gives **every** row a
  mark column: text at `menu.x + 32` instead of `+ 12` (`MARK_COLUMN` 20 more), in that menu
  only (a submenu decides for itself). A mark on one row alone would misalign the labels;
  Windows, GNOME and macOS reserve the column the same way.
- **Tick**: an 8x8 box at (`menu.x + 12`, row + 12). Column `c` of the box (0 to 7) has two
  pixels, at rows `r` and `r + 1`, with `r` = 4, 5, 6, 5, 4, 3, 2, 1: a short stroke down to
  column 2 and a long one up to column 7, 2 px thick so it holds at arm's length. Colour
  `text`; `text.dim` when the item is disabled.
- **Dot**: a 6x6 disc at (`menu.x + 13`, row + 13), its rows inset 2, 1, 0, 0, 1, 2 (the
  `INSET_R3` shape, mirrored), same colours. An unchecked or unselected row draws nothing in
  the column.
- **Submenu arrow**: the dropdown's nine 2x2 squares turned to point right: 6 wide, 10 tall,
  five squares in columns 0-1 (rows 0 to 9), three in columns 2-3 (rows 2 to 7), one in
  columns 4-5 (rows 4-5). Its right edge is at `menu.right - 12`, its top at row + 11. Colour
  `text`; `text.dim` when disabled.
- **Width**: `12 + (20 when the menu has a mark column) + widest label + key room + 12`,
  rounded up to 8, at least 160. Key room is `24 + max(widest key, 6 when the menu has a
  submenu)`, or 0 when it has neither keys nor submenus. Keys stay right-aligned ending at
  `menu.right - 12`, in the arrow's place.
- **Where a submenu opens**: its left edge on the parent's right border column (`parent.right
  - 1`, the two 1 px borders overlapping), its top one above the row (`row.top - 1`), so its
  first row lines up with the row it came from. `Menu::new` clamps it inside the window
  content as today. When it would pass the right bound it opens **to the left** (its right
  edge at `parent.x + 1`); when neither side fits it is clamped inside the bounds and covers
  its parent (Left or Esc brings the parent back). It moves up when it would pass the bottom.
- The row a submenu was opened from stays `menu.hover` while the submenu is open, whatever
  the pointer is over; that row is the path.

### 2.3 Pointer

- **Hover**: resting on an enabled `Submenu` row for `SUBMENU_OPEN_MS` (150) opens it, with no
  row highlighted in it. Moving to another row of the parent closes the open submenu 150 ms
  later, unless the pointer reaches the submenu first, which calls the close off. The delay
  in both directions is what lets a diagonal move toward the submenu land; there is no
  safe-triangle geometry (see Later).
- **Click**: a press on a `Submenu` row opens it at once and the menu stays open; a release
  over one (press, slide, let go) opens it too. A press on an item of any level chooses it
  and closes every level. A press outside every open level closes all and is consumed, as in
  v0.5.
- **Touch**: no hover, so a tap opens; section 7.

### 2.4 Keys

| Key | In an open menu |
|-----|-----------------|
| Up, Down, Home, End | move in the innermost open level (disabled rows and separators skipped) |
| Right, on a `Submenu` row | opens it with its first enabled row highlighted |
| Right, on any other row | a bar switches to the next title's menu, as in v0.5, from any level; a menu button and a context menu do nothing |
| Left, in a submenu | closes it; its parent row is highlighted |
| Left, at the first level | a bar switches to the previous title's menu, as in v0.5; a button and a context menu do nothing |
| Enter, Space | on a `Submenu` row as Right; on an item, a check or a radio: `Invoked`, everything closes |
| Esc | closes the innermost level only; at the first level as in v0.5 |
| a mnemonic letter | applies to the innermost level: one match chooses (or opens a submenu), several move the highlight between them |

Focus does not move: the bar is armed or the button has the focus, exactly as in v0.5.

### 2.5 The macOS profile's top bar

`SET_MENU`'s pool (`docs/windows.md`) gains three things; a pool with none of them is
byte for byte today's:

- a **mark** as a third tab-separated field of an `I` or `D` line: `c1` a checked check, `c0`
  an unchecked one, `r1` a selected radio, `r0` an unselected one (`IWrap\t\tc1`);
- **`S<label>`** opens a submenu (`s<label>` a disabled one) and **`E`** ends it; the lines
  between are its entries; at most 16 entries a level and three levels.
- `MenuChosen`'s and `CHOOSE_MENU`'s entry index is the entry's **depth-first number** in its
  title's menu: every entry of any kind counts one, a submenu's own line counts and its
  entries follow it, `E` lines do not count. With no submenu that is today's index.

The top bar draws marks, arrows and side-opening submenus with the constants of 2.2, and `wm
menu` lists a submenu's entries indented under it with `checked` or `selected` after a marked
one. The pool stays 248 bytes; `Bar::pool` still cuts at the last whole entry that fits and
closes the submenus it left open with their `E` lines.

## 3. The menu bar's overflow

When the titles' hit rects (`width + 24` each, from the bar's left edge) do not all fit the
bar's width, the bar shows the **overflow title `...`** at its right end:

- Its rect is **33 wide** (12 + 9 + 12) at `bar.right - 33`, the bar's full height, its text
  at + 12, cell top + 6, in `text`; hover, open and armed fills are a title's (`menu.hover`).
- Titles are placed from the left while their rect ends at or before `bar.right - 33`; the
  first that does not, and every title after it, are **hidden**. When every title fits the
  bar's full width there is no `...`. The bar is laid out again in the frame its width
  changes; an open menu closes when its title becomes hidden or shown.
- **Its menu** is a menu of `Submenu` rows, one per hidden title (the title without `&`, its
  entries the title's menu), using section 2 unchanged. It opens under the bar, right-aligned
  to the bar's right edge and then clamped by `Menu::new` as every menu is (8 px inside the
  window); its submenus open to the left
  where the right has no room, by 2.2's rule. A hidden title's menu is therefore one level
  deeper, so its own third level is drawn disabled; no system app nests that deep.
- **Keys**: `...` is the last title for Left and Right (wrapping) and for Down, Enter and
  Space. A hidden title's mnemonic (`Alt+T` for a hidden `&Tools`) opens **that title's menu
  directly**, right-aligned under `...`, with `...` drawn open: the person asked for Tools,
  not for the overflow; the menu is placed as the overflow's own is. F10 arms the first
  shown title.
- **macOS profile**: the window draws no bar (v0.5 3.4) and the top bar is the shell's, so
  there is no overflow there in this slice (see Later).
- **Phone**: section 7.

## 4. Table: reordering, fit, multi-selection, cell focus

`Table` gains, with defaults that leave a v0.5 table unchanged:

```
pub order: [u8; MAX_COLUMNS],            visual position -> column index; identity at start
pub selection: Option<RowSelected>,      None: one selected row, as v0.5
pub selected_count: usize,
pub anchor: usize,
pub cell_focus: bool,                    false: Left and Right scroll, as v0.5
pub column: usize,                       the focused cell's visual position
pub type RowSelected = fn(data: u32, row: usize) -> bool;
```

Setters, each damaging the table and sending nothing: `set_column_order(id, &[u8])`,
`set_row_selection(id, Option<RowSelected>)`, `set_selection_count(id, usize)`,
`set_cell_focus(id, bool)`; `focused_cell(id) -> (row, column index)`.

### 4.1 Column reordering by drag

- **A column keeps its index.** `cell`, `sort`, `Sorted`, `ColumnResized`, `set_column_width`
  and the header's element id all keep the column's original index; only drawing, hit testing
  and the order elements are listed in use `order`. A program that does not care about the
  order changes nothing.
- **Start**: a press on a header cell (not on a handle) that then moves `DRAG_THRESHOLD` (6
  px) sideways starts the drag; a release before that is the click it always was (sort). A
  table with one column never drags; a column that is not sortable still drags.
- **While dragging**: the pointer is `Hand`. The column's own header cell is drawn
  `button.pressed` with its title in `text.dim`. The **ghost** is a copy of the header cell:
  the column's width, 28 tall, `menu.hover` fill, a 1 px `menu.border` outline, the title in
  `text` at its usual place; its left edge is the pointer's x minus where the press grabbed
  the cell, its top the header's, clipped to the table. The **insertion line** is 2 px of
  `accent` from the header's top to the body's bottom, at the slot the pointer chooses: over
  the left half of a column, the boundary before it; over the right half, the boundary after
  it; the line's x is `boundary - 1`, clamped inside the table's view. The two slots beside
  the dragged column move nothing: no line shows there and a release does nothing. Within
  `DRAG_EDGE` (24) of the view's left or right edge of a table wider than its view,
  `offset_x` scrolls 24 every `DRAG_SCROLL_MS` (100).
- **End**: a release over a slot with a line reorders and returns **`Response::ColumnMoved(id,
  from, to)`**, both visual positions, `to` being where the column now is; the program saves
  the order once. Esc, a release with no line, the window losing focus or the table being
  hidden or disabled cancels and returns `Nothing`; the release after Esc is `Nothing` too.
- **The first column's mark.** The `>` and `TABLE_FIRST_TEXT_X` belong to the **visual
  first** column, whichever that is. An `Align::End` column that becomes first keeps its right
  alignment and is ellipsised from `x + 24`.

### 4.2 Fit to content

A double-click on a handle (two presses on the same handle within `DOUBLE_CLICK_MS`, no move
between) sets that column's width to the widest of:

- its title + 16, + 20 more when it is the sorted column (`TABLE_ARROW_ROOM`);
- each sampled cell + 16, + 16 more in the visual first column (its text starts at + 24);

clamped to `min_width .. TABLE_MAX_FIT` (640), and returns `ColumnResized(id, column, width)`
when the width changed. The **sample** is the rows on screen and rows `0 ..
TABLE_FIT_SAMPLE_ROWS` (1000): a million-row table stays one frame's work (at most a thousand
`cell` calls of 64 bytes), and the rule is stated, not a surprise. A program that needs an
exact fit measures its own data and calls `set_column_width`. The second press starts no
resize drag.

### 4.3 Multi-selection

Off by default. `set_row_selection(id, Some(is_selected))` turns it on.

- **The program owns the set**, as it owns the cells: a heapless tree cannot hold a bit per
  row of a million. The tree asks `is_selected(data, row)` only for the rows it draws or
  describes, and tells the program what the person did with **`Response::Selection(id,
  SelectionChange)`**:

  | Change | The set becomes |
  |--------|-----------------|
  | `Only(row)` | that row alone |
  | `Toggle(row)` | itself with that row flipped |
  | `Range(a, b)` | exactly rows `a ..= b`, in either order |
  | `AddRange(a, b)` | itself plus rows `a ..= b` |
  | `All` | every row |
  | `Clear` | empty |

  The program applies it, calls `set_selection_count(id, n)`, and the tree redraws. The
  toolkit offers `table::RowSet<const WORDS: usize>` (a bitmap, 64 rows a word, with `apply`,
  `contains` and `count`) for programs whose row count is bounded; Files uses it.
- **`selected` is the current row**: the row keys move from, the one with the ring. `anchor`
  is the row a range starts at.
- **Pointer** (Mod is Ctrl or Super, as everywhere in the toolkit):

  | Input | Returns | Current | Anchor |
  |-------|---------|---------|--------|
  | press on a row (in a table that is a drag source: its release without a drag, as v0.5) | `Only(row)` | row | row |
  | Mod + press | `Toggle(row)` | row | row |
  | Shift + press | `Range(anchor, row)` | row | kept |
  | Mod + Shift + press | `AddRange(anchor, row)` | row | kept |
  | press on a selected row that becomes a drag (v0.5 section 7) | `DragStarted(id, row)`; the set is untouched and the program drags its whole set | | |
  | press on a selected row, released without a drag | `Only(row)`, on the release | row | row |
  | right-click on an unselected row | `Only(row)`, then to the window | row | row |
  | right-click on a selected row | to the window; the set is untouched | row | kept |
  | double-click, Enter | `Opened(id, current)`; the program opens its set | | |

- **Keys** (table focused):

  | Key | Returns | Effect |
  |-----|---------|--------|
  | Up, Down, Home, End, PageUp, PageDown | `Only(current)` | current moves; anchor = current |
  | Shift + those | `Range(anchor, current)` | current moves; anchor kept |
  | Ctrl + Up, Down | `Chosen(id, current)` | current moves alone; the set is untouched |
  | Ctrl + Space | `Toggle(current)` | anchor = current |
  | Mod + A | `All` | taken by the focused table before the window's own Mod+A |
  | Mod + Shift + A | `Clear` | current keeps the ring |
  | Space | `Nothing` | as v0.5 |

  Super+Up and Super+Down are not the table's: they reach the program (`Unused`), which may
  give them Finder's meaning. `Chosen` keeps its v0.5 sense, "the current row changed".
- **Drawing**: every row `is_selected` says is selected has the `selection` fill **and the
  `>`** in `accent`, so the set is readable without colour; the current row has the ring when
  the table is focused, selected or not. Hover never changes a selected row.
- With `selection` `None` the table is v0.5's in every response and pixel.

### 4.4 Cell focus

Off by default. With `set_cell_focus(id, true)`:

- **Left and Right move the focused cell** one column (visual order) instead of scrolling by
  48, stopping at the first and last; **Mod+Left and Mod+Right** go to the first and last
  column. Home, End and the other row keys keep their meaning. `offset_x` follows so the cell
  is in view.
- **The ring** (2 px `focus.ring`, inset 0) is drawn on the cell (`x_c .. x_c + width_c` of
  the current row), clipped to the body, instead of on the row.
- A press sets the cell under the pointer as well as the row.
- **`Response::CellFocused(id, row, column index)`** on every move of the cell, by key or
  pointer. One input returns one response: when it also changes the set it returns
  `Selection`, and when it only moves the row in a single-selection table it returns
  `CellFocused` in place of `Chosen`; the program reads `focused_cell(id)` whenever it needs
  the cell. Enter is still `Opened`; what a program does with a cell (edit a value in place)
  is the program's.

## 5. Text field and text area: undo, redo and word moves

### 5.1 Undo and redo

| Keys | Does |
|------|------|
| Mod+Z | undo one step |
| Mod+Shift+Z, Mod+Y | redo one step |

In every profile, as the Text Editor's (`text-editor-v0.3.md` section 10). An undo or redo
that changes the text returns `Edited(id)`; with nothing to undo or redo the key is consumed
and returns `Nothing`. Both restore the caret and the selection of before the step, scroll
them into view, and (in an area) wrap again from the step's first byte.

**What makes one step** (the Text Editor's rules plus a pause):

- Typed printable characters join the open step; so does a run of Backspace, and a run of
  Delete. A dead key's composed character is one typed character.
- These **end** the open step, so the next edit starts a new one: Enter, a paste, a cut, a
  word deletion (5.2), typing over a selection (the removal and the first character are one
  step, then it stays open for more typing), any caret move by key or pointer, a change from
  typing to deleting or back, the widget losing the focus, and **a pause of more than
  `UNDO_GROUP_MS` (1000 ms)** since the step's last edit, so a paragraph typed in bursts
  undoes in bursts.
- Enter, a paste, a cut and a word deletion are each a step of their own.
- A new edit clears the redo list. `set_text` (the program's, or an agent's `set-text`)
  clears both lists: the text is no longer what the steps describe.

**Where the steps live**:

- **Text area**: in its text store, beside the bytes: at most `TEXT_AREA_UNDO_STEPS` (64)
  steps and `TEXT_AREA_UNDO_BYTES` (8192) bytes of removed and inserted text. When either
  runs out the oldest steps are dropped. One edit larger than the byte budget (a big paste) is
  applied, **not recorded, and clears both lists**: a history with a hole would restore the
  wrong text. The history stays while the focus is elsewhere. A tree without a store pays
  nothing.
- **Text field**: **one history for the focused field**, in the tree, not one per field:
  `FIELD_UNDO_STEPS` (16) steps and `FIELD_UNDO_BYTES` (512) bytes, which holds any one edit
  of a 255-byte field; the oldest steps are dropped when either runs out. It is cleared when the
  focus moves to another widget. A `Kind` is held by value in every node, so a history per
  field would grow every node of every window; a field that lost the focus has been accepted,
  as Windows's edit control treats it. A masked field's history follows the same rule and is
  never shown.
- A **read-only** text area (D609) records nothing; Mod+Z, Mod+Shift+Z and Mod+Y do nothing
  there and return `Nothing`.

### 5.2 Word moves and word deletion

| Does | Windows and Linux profiles | macOS profile |
|------|----------------------------|---------------|
| to the end of this or the next word | Mod+Right | Alt+Right, Ctrl+Right |
| to the start of this or the previous word | Mod+Left | Alt+Left, Ctrl+Left |
| the same, extending the selection | with Shift | with Shift |
| delete to the start of the word | Mod+Backspace | Alt+Backspace, Ctrl+Backspace |
| delete to the end of the word | Mod+Delete | Alt+Delete, Ctrl+Delete |
| to the line's start, end | Home, End | Home, End, Cmd+Left, Cmd+Right |
| to the text's start, end | Mod+Home, Mod+End | the same, Cmd+Up, Cmd+Down |
| delete to the line's start | | Cmd+Backspace |

- **One flag tells the tree which column applies**: `WidgetTree::set_mac_text_keys(bool)`,
  set by `application::run` from the desktop profile at start and on a profile change, where
  it already calls `set_global_menu`. It is the only thing the text keys read from the
  profile; `Mod is Ctrl or Super` (v0.4 section 4.3) holds for every other shortcut.
- In the Windows and Linux profiles **Alt+arrows are not the field's**: they return `Unused`
  and reach the program (Alt+Left is Back in Files). In the macOS profile Cmd is Super, and
  Option is Alt with or without the third-level bit, since arrows, Backspace and Delete type
  nothing.
- **A word** uses the three classes the double-click already uses (`Field::word_at`, moved
  to one shared function): white space (space, tab, line break), word characters (ASCII
  letters and digits, `_`, and every non-ASCII character), and other signs. A move to the
  right skips white space, then runs to the end of the run of one class; a move to the left
  mirrors it. A run of signs (`->`, `...`) is one stop. The rule is "to the end of the word"
  in every profile (macOS's and GTK's), not Windows's "to the start of the next word": one
  function, one test table. The double-click's own selection is unchanged.
- With a selection, a word deletion deletes the selection, as Backspace does. A word deletion
  is one undo step. In a read-only area the moves work and the deletions do nothing.
- In a text area Up and Down with Mod or Alt are not word keys; Mod+Up and Mod+Down return
  `Unused` outside the macOS profile.

## 6. What an agent sees (D161, `wm show`, `wm act`)

No new role, flag or action; every element that exists keeps its id, name, value and actions.

- **`Check`**: `menuitem`, flag `checked` when ticked. **`Radio`**: `menuitem`, flag
  `selected` when chosen. `invoke` on either returns `Invoked` as a click does and changes no
  mark by itself.
- **`Submenu`**: `menuitem`, flag `expandable`, `expanded` while open, no value; its entries
  are its children, **listed whether or not it is open**, so an agent invokes a nested item
  directly, which chooses it and closes the menu. `invoke` on the submenu row opens or closes
  it. A fourth-level submenu is flagged `disabled`.
- **Element ids.** A widget's hundred ids are full of titles and first-level entries, so
  nested entries get ids from a range of their own, made from the widget, the title and the
  path of entry positions; they do not change while the entries do not. The `...` title is a
  `menuitem` named **`More`**, flagged `expandable`, in that range too.
- **Hidden titles** stay the bar's children with their entries, as shown ones do, their
  bounds the `...` title's rect; nothing is hidden from an agent.
- **A table's header**: `columnheader`s are listed in visual order, each with the id it
  always had. `set-text "<integer>"` still sets the width. New texts: **`fit`** fits the
  column (4.2) and **`move <position>`** moves it to that visual position, from 1
  (`ColumnMoved`); any other text is `NotSupported`, as today.
- **A table**: `set-text "<row>"` still selects that row and scrolls to it (with
  multi-selection on it returns `Selection(Only)`). With multi-selection on, the value gains
  `, <k> selected` (`row 4 of 1000, 2 selected`), each shown row is flagged `selected` by
  `is_selected`, and the table takes **`all`**, **`none`**, **`toggle <row>`**, **`range <a>
  <b>`** and **`add <a> <b>`** (rows from 1); `select` on a row is `Only`. With cell focus on,
  the value gains `, column <n>` before the count (`row 4 of 1000, column 2, 2 selected`) and
  the table takes **`column <n>`**. With a mode off its texts are `NotSupported`. A row's
  value is its cells joined by tabs in **visual** order, so it lines up with the headers.
- **Text**: undo, redo and word moves are keys (`wm press Ctrl+Z`); nothing new is listed.
  `set-text` clears the history (5.1).

## 7. Device classes (D23)

- **Desktop and laptop**: everything above.
- **Phone** (menus are bottom sheets, v0.5 section 11): a `Submenu` row shows its arrow and a
  tap replaces the sheet's rows with the submenu's under a 48 px back row (`<` and the
  submenu's label; Back and a swipe from the edge do the same as the row). Marks sit in the
  sheet's mark column (text at + 32). The bar shows the titles that fit and `...`, 40 px
  wide; its sheet lists the hidden titles, each drilling in. Tables: a long-press (500 ms) on
  a header picks the column up and a move drags it; a double-tap on a handle's 24 px zone
  fits; with multi-selection on, a **long-press on a row toggles it** and starts selection
  mode, in which a tap toggles (`Toggle`) instead of `Only`, until the count is back to 0 or
  Back is pressed; in that mode a long-press on a selected row followed by a move is the
  drag. Cell focus needs keys and stays off unless a keyboard is attached. Undo and word
  moves come from an attached keyboard, or from the on-screen keyboard where it has the keys.
- **Tablet**: menus drop down as on the desktop with 40 px rows; a submenu opens on a tap
  (no hover) and by the 150 ms hover when a pointer is attached; the touch rules for tables
  are the phone's; with a keyboard everything is the desktop's.
- **TV** (D-pad, scale 2): Right opens a submenu, Left or Back closes it; `...` is reached by
  Right past the last shown title; OK chooses. Tables: no column drag and no handles (v0.5);
  with multi-selection on, **OK held 500 ms toggles the current row** and a short OK is
  `Opened`. Cell focus works by Left and Right when the program turns it on.
- **Wearable**: no menu bar and no table (v0.5), so only the text keys apply, with a
  keyboard.
- **Keyboard only**, on every class: column order has no keys in the table itself: the
  program offers it in its own menu (Files' `View > Columns`, later), and an agent uses `move
  <position>`.

## 8. Focus order, damage and cost

- **Focus order is unchanged.** Menus take the keyboard without moving the tree's focus
  (v0.5); a table is still one Tab stop with the cell inside it; the column drag and the
  undo history never take the focus. Tab leaves a table from any cell.
- **Damage**: a submenu's open and close damage its rect and its parent row; a mark change
  the row; the overflow a re-laid bar; a column drag the ghost's old and new rects and the
  old and new line; a reorder or a fit the header and the body from the leftmost changed
  column to the right edge; a selection change the rows whose state changed among the shown
  ones; a cell move the two cells; an undo the lines from the step's first byte down.
- **Cost**: nothing allocates. `is_selected` is called for the shown rows only; a fit is at
  most `1000 + shown` `cell` calls for one column; an undo step is a copy within the store.
  `toolkit_table_layout_1000_rows` gains a multi-selection variant that must stay within 5%
  of the plain one.

## 9. Acceptance (host tests, scale 1x, dark palette, window content 480x360 unless said)

37. **Nothing existing moves.** Every case of v0.5 section 13 passes unchanged; a menu with
    no `Check`, `Radio` or `Submenu`, a table with `selection` `None` and `cell_focus` false,
    and the fields' and areas' v0.5 keys draw and answer byte for byte as before.

Fixture H: a menu bar `m` at (0, 0), `Fill(0)` x 28. `&File`: `&New` `Mod+N` id 1; submenu
`Open &Recent` [`notes.txt` id 30, `todo.md` id 31, separator, `&Clear List` id 32 disabled];
separator; `&Quit` `Mod+Q` id 3. `&View`: check `&Wrap` id 20 checked; check `Line &Numbers`
id 21; separator; radios `&Small` id 22, `&Medium` id 23 selected, `&Large` id 24; separator;
submenu `&Zoom` [`100 %` id 40, submenu `&More` [`400 %` id 41, submenu `Deeper` [`x` id
42]]]. Title rects: `File` 0..39, `View` 39..85.

38. Press (20, 14): the `File` menu at (0, 29, 160, 107) (12 + 62 + 24 + 32 + 12 = 142 →
    160); no mark column: `New` at x 12; `Open Recent` at (12, 70) with no key and its arrow
    at (142, 73): pixels (142, 73) and (146, 77) `text`, (146, 73) `menu.bg`. `describe`:
    `Open Recent` is `expandable`, not `expanded`, with children `notes.txt`, `todo.md` and
    `Clear List` (`disabled`).
39. Move to (40, 78) at t = 0: the row is `menu.hover`; `tick(149)`: no submenu; `tick(150)`:
    the submenu at (159, 61, 160, 107), nothing highlighted in it, `Open Recent` `expanded`.
    Move to (200, 78): `notes.txt` highlighted and `Open Recent` still `menu.hover`. Move to
    (40, 46) at t = 1000: `New` highlighted, the submenu still open at `tick(1149)`, closed
    at `tick(1150)`. Open it again, move to (40, 46) and within 150 ms to (200, 78): it stays
    open. Click (200, 110): `Invoked(m, 31)`, every level closed. Open again, click (200,
    150) on `Clear List`: `Nothing`, still open. A press on `Open Recent` opens the submenu
    in that frame.
40. Keys: F10, Down, Down highlight `Open Recent`; Right opens the submenu with `notes.txt`
    highlighted; Down → `todo.md`; Down → stays (the separator and the disabled row are
    skipped); Left closes it with `Open Recent` highlighted; Enter opens it again; Esc closes
    the submenu only; Esc closes `File` and leaves the bar armed. With `New` highlighted,
    Right opens `View`'s menu. In `Open Recent`, `t` (no mnemonic there) does nothing; in
    `File`, `r` opens `Open Recent`.
41. `View > Zoom > More` opens as the third level; in it `Deeper` is drawn in `text.dim`
    with a `text.dim` arrow, Right and a click on it do nothing, and `describe` flags it
    `disabled`. A menu button at (300, 100) whose menu holds a submenu opens the submenu to
    the left: its right edge at 301.
42. Marks: the `View` menu at (39, 29, 160, 212) (12 + 20 + 66 + 30 + 12 = 140 → 160); every
    label at x 71. `Wrap`'s tick: pixels (51, 46), (51, 47), (53, 48), (53, 49), (58, 43),
    (58, 44) `text`, (51, 42) `menu.bg`. `Line Numbers`: (53, 80) `menu.bg`. `Medium`'s dot
    at (52, 148): (54, 150) `text`, (52, 148) `menu.bg`; `Small`: (54, 118) `menu.bg`. A
    click on `Wrap` returns `Invoked(m, 20)` and the tick is still there when the menu opens
    again; `set_entry_checked(m, 20, false)` removes it; `set_entry_selected(m, 24)` puts the
    dot on `Large` and takes it from `Medium`. `describe`: `Wrap` `checked`, then not;
    `Large` `selected`.
43. macOS profile: the pool holds `SOpen Recent\n`, `Inotes.txt\t\n`, `DClear List\t\n`,
    `E\n`, `IWrap\t\tc1\n`, `ILine Numbers\t\tc0\n`, `IMedium\t\tr1\n`; `MenuChosen { menu: 0,
    entry: 3 }` (the fourth entry depth-first: `todo.md`) → `Invoked(m, 31)`; `{ menu: 0,
    entry: 7 }` → `Invoked(m, 3)`. A bar with no new kinds sends v0.5 case 12's pool
    unchanged.

Fixture I: window content 200x360, a bar `o` with `&File`, `&Edit`, `&View`, `&Tools`,
`&Help`, one item each (rects 39, 41, 46, 50, 44 wide: 220 in all).

44. Paint: `File` 0..39, `Edit` 39..80, `View` 80..126; `...` at (167, 0, 33, 28) with its
    text at (179, 6) in `text`; (150, 14) is `menu.bg`; `Tools` and `Help` are not drawn.
    Press (180, 14): the overflow menu at (32, 29, 160, 66) (its right edge 8 inside the
    window, `Menu::new`'s clamp) with rows `Tools` and `Help`, each with an arrow; `...`
    filled `menu.hover`. Down, Right: `Tools`' menu opens at x 32 too (neither side fits),
    covering its parent; Left brings the overflow menu back. Esc, Esc; Alt+H: `Help`'s menu
    opens at once at x 32, `...` filled. F10 then
    Left: `...` is highlighted. Window content 240 wide: no `...`, `Tools` at 126..176, `Help`
    at 176..220. `describe` at 200: the bar's children are the five titles with their
    entries, then `menuitem` `More`, `expandable`.

Fixture J: v0.5's fixture C (table `t` at (16, 40), 400x220; `Name` 200, `Size` 80 `End`,
`Modified` 160; 1000 rows of `Row <r>`, `<r> KB`, `2026-10-31`).

45. Press (250, 50) on `Size`, move to (255, 50): no drag (5 px); move to (256, 50): the drag
    starts, `Nothing`, pointer `Hand`; the ghost at (222, 40, 80, 28): (230, 46) `menu.hover`,
    (222, 50) `menu.border`; `Size`'s own cell `button.pressed`; no line (the pointer is over
    the dragged column). Move to (150, 50) (the right half of `Name`): no line. Move to (100,
    50): the ghost at (66, 40, 80, 28) and the line at x 16..17, rows 40..251, `accent`.
    Release: `ColumnMoved(t, 1, 0)`; headers read `Size` 16..96, `Name` 96..296, `Modified`
    296..456; `cell` is still called with column 1 for `0 KB`, which ends at x 88; the `>` is
    at x 24; `Row 0` starts at x 104. `describe`: the headers in that order with their old
    ids; row 0's value `0 KB\tRow 0\t2026-10-31`.
46. After case 45: press (150, 50) and release with no move → `Sorted(t, 0, Ascending)` (the
    `Name` column's index). Press (150, 50), move to (300, 50), Esc → `Nothing`, the order
    unchanged; the release → `Nothing`. `act(set-text "move 1", header Name)` →
    `ColumnMoved(t, 1, 0)`, `Done`; `act(set-text "move 9", ..)` → `NotSupported`.
47. Fit, on a fresh fixture J: two presses at (215, 50) within 400 ms → `ColumnResized(t, 0,
    74)` (`Row 999` 42 + 16 + 16; the title 26 + 16) and no resize drag; at (295, 50) →
    `ColumnResized(t, 1, 50)` (`999 KB` 34 + 16). `act(set-text "fit", header Modified)` →
    `ColumnResized(t, 2, 72)`. With 10 000 rows a fit calls `cell` for that column at most
    1008 times. A column of one 700 px cell fits to 640.
48. Multi-selection, on a fresh fixture J with `set_row_selection(t, Some(..))` over a set
    the test owns. Click (100, 100) → `Selection(t, Only(1))`. Ctrl+click (100, 148) →
    `Toggle(3)`, current and anchor 3. Shift+click (100, 196) → `Range(3, 5)`, current 5,
    anchor 3. Ctrl+Shift+click (100, 240) → `AddRange(3, 7)`. Shift+Up → `Range(3, 6)`.
    Ctrl+Up → `Chosen(t, 5)`. Ctrl+Space → `Toggle(5)`, anchor 5. Down → `Only(6)`. Mod+A →
    `All`. Mod+Shift+A → `Clear`. Super+Down → `Unused`. With the set {1, 3} and current 3,
    focused: rows 1 and 3 are `selection` with `>` at (24, 96) and (24, 144) in `accent`, row
    2 is neither, the ring is on row 3 alone. `set_selection_count(t, 2)` → value `row 4 of
    1000, 2 selected`; rows 1 and 3 flagged `selected`. `act(set-text "all")` →
    `Selection(t, All)`; `"toggle 7"` → `Toggle(6)`; `"range 2 5"` → `Range(1, 4)`; `"add 9
    9"` → `AddRange(8, 8)`; `"none"` → `Clear`; `"500"` → `Selection(t, Only(499))` and row
    499 in view. With `selection` `None`, `"all"` → `NotSupported`.
49. Cell focus, on a fresh fixture J with `set_cell_focus(t, true)`: Right →
    `CellFocused(t, 0, 1)`; the ring is on (216, 68, 80, 24): (216, 68) `focus.ring`; the
    row's own ring is not drawn. Right → `CellFocused(t, 0, 2)` and `offset_x` 48. Right →
    `Nothing`. Mod+Left → `CellFocused(t, 0, 0)`, `offset_x` 0. Click (250, 100) →
    `CellFocused(t, 1, 1)`. Down → `CellFocused(t, 2, 1)`. Enter → `Opened(t, 2)`. Value `row
    3 of 1000, column 2`; `act(set-text "column 3")` → `CellFocused(t, 2, 2)`. After moving
    `Size` first, `CellFocused` still reports `Size` as column index 1.

Fixture K: a text field `f` at (16, 60), 200x28, focused, empty; v0.5's fixture D for the
area `a`.

50. Undo in the field: type `a`, `b`, `c` at t = 0, 100, 200; type `d`, `e`, `f` at t = 1300,
    1400, 1500. Mod+Z → `abc`, caret 3, `Edited(f)`. Mod+Z → empty. Mod+Z → `Nothing`.
    Mod+Shift+Z → `abc`. Mod+Y → `abcdef`. Mod+Z, then `x` → `abcx`, and Mod+Y → `Nothing`
    (the redo list is cleared). Three Backspaces are one step. `abc`, Left, `d` are two
    steps. Select `bc`, type `x`, `y` → one step back to `abc` with `bc` selected. Seventeen
    single steps keep the newest sixteen. Tab away and back, Mod+Z → `Nothing`.
    `set_text(f, ..)` empties the history.
51. Undo in the area: Enter is its own step; a paste of `a\nb` is one step and undoing it
    restores the line count; a paste of 9000 bytes is applied and not recorded; 65 single
    steps keep the newest 64; the history survives the focus leaving and returning; a
    read-only area answers Mod+Z with `Nothing` and no change.
52. Word moves, text `foo_bar->baz  qux`, caret 0: Mod+Right → 7, 9, 12, 17; Mod+Left → 14,
    9, 7, 0; Mod+Shift+Right selects `foo_bar`; at 17, Mod+Backspace leaves `foo_bar->baz  `
    and one Mod+Z brings `qux` back; at 0, Mod+Delete leaves `->baz  qux`. Alt+Right →
    `Unused { code: RIGHT, .. }`. In the area, Mod+Right at a line's end goes to the end of
    the next line's first word.
53. `set_mac_text_keys(true)`: Alt+Right → 7; Ctrl+Right → 9; Super+Right → 17; Super+Left →
    0; Alt+Shift+Right selects `foo_bar`; at 17, Alt+Backspace deletes `qux` and
    Super+Backspace then empties the line. In the area Super+Down goes to the text's end.

## Decided by design (the owner can overrule any of these)

1. Marks are the program's state; the menu never flips one.
2. The mark column is per menu, 20 px, and the tick is 2 px thick.
3. Submenus open on a 150 ms hover with no safe triangle, overlap the parent's border by 1 px,
   nest three deep, and cover the parent when neither side has room.
4. The overflow title is `...` (v0.5's word for it), named `More` for agents; a hidden
   title's mnemonic opens its menu directly.
5. A column keeps its index when it moves, so programs change nothing; a row's value for
   agents follows the visual order.
6. Fit samples the shown rows and the first 1000.
7. The program owns the selected set; every selected row carries the `>`, not only the
   current one, so the set never depends on colour.
8. Super+Up and Super+Down are left to the program; Ctrl moves the current row alone.
9. Cell focus is opt-in and leaves Home and End to the rows.
10. Undo grouping is the Text Editor's plus a 1 s pause; redo is Mod+Shift+Z and Mod+Y in
    every profile.
11. A field's undo history is one per tree, for the focused field, and ends with the focus.
12. Word moves stop at the end of a word in every profile; Alt is a word key only in the
    macOS profile, told to the tree by one flag.
13. Agents get the new table actions as `set-text` words, leaving the integer forms as they
    are.

## Changed from this file's first draft

The first draft of this file (2026-11-17) gave a header's `set-text "<position>"` to moving the column,
which is already the width; used an 8 px drag threshold where `DRAG_THRESHOLD` is 6; gave
fields a history each "in the tree's field table", which does not exist; tied the word rule
to runs of signs in the double-click, which selects one sign; added 24 for the first column
in a fit where the text starts 16 further in; marked only the current row of a selected set;
and had no pool form for the macOS profile. This text replaces it.

## Later

A safe triangle for submenus; the macOS top bar's overflow and a larger `SET_MENU` pool;
hiding a column by dragging it off and a `Columns` menu in Files; rubber-band selection;
editing a focused cell in place; a tree table; the selection of a field or an area in the
element tree; undo across `set_text`; a history per field; spell marks; and what v0.5
section 14 still lists.

## Decisions for the engineer

1. `Entry::Check { label, key, enabled, id, checked }`, `Entry::Radio { .., selected }`,
   `Entry::Submenu { label, enabled, entries }`; `Item` and `Separator` untouched.
   `set_entry_checked(widget, entry_id, bool)`, `set_entry_selected(widget, entry_id)`,
   `MENU_MARKS` 32 a tree; a radio group is a run of `Radio` entries.
2. `menu::Stack` of `DEPTH` 3 levels, shared by the tree and the programs' context menus.
3. Mark column 20 (text at + 32) in a menu with any mark; tick 8x8 at (+ 12, row + 12),
   columns' rows 4, 5, 6, 5, 4, 3, 2, 1, two pixels each; dot 6x6 at (+ 13, row + 13), insets
   2, 1, 0, 0, 1, 2.
4. Arrow 6x10 (5, 3, 1 squares of 2x2), right edge `right - 12`, top row + 11; key room `24 +
   max(widest key, 6)`.
5. Submenu at (`parent.right - 1`, `row.top - 1`); to the left at right edge `parent.x + 1`;
   clamped over the parent when neither fits; `SUBMENU_OPEN_MS` 150 to open and to close.
6. Keys of 2.4; a press on a submenu row opens at once.
7. Pool: third field `c0`, `c1`, `r0`, `r1`; `S<label>`, `s<label>`, `E`; depth-first entry
   numbers for `MenuChosen` and `CHOOSE_MENU`; the shell's top bar and `wm menu` draw and
   list them.
8. Overflow title `...`, 33 wide at `bar.right - 33`; titles placed while they end at or
   before it; its menu is `Submenu` rows of the hidden titles, right-aligned to the bar;
   element `More`; hidden mnemonics open directly.
9. `Table.order`, `set_column_order`, `Response::ColumnMoved(id, from, to)`; original indexes
   everywhere else; drag from `DRAG_THRESHOLD`, ghost `menu.hover` with `menu.border`, line 2
   px `accent` at `boundary - 1`, half-column slots, edge scroll 24 per 100 ms, Esc cancels.
10. Fit on a handle's double-click: title + 16 (+ 20 sorted), cells + 16 (+ 16 visual first),
    shown rows and rows 0..1000, clamped `min_width ..= 640`; returns `ColumnResized`.
11. `Table.selection: Option<RowSelected>`, `selected_count`, `anchor`;
    `set_row_selection`, `set_selection_count`; `Response::Selection(id, SelectionChange {
    Only, Toggle, Range, AddRange, All, Clear })`; `table::RowSet`; the pointer and key tables
    of 4.3; `>` on every selected row, the ring on the current one.
12. `Table.cell_focus`, `column`; `set_cell_focus`, `focused_cell`; `Response::CellFocused(id,
    row, column index)`; Left, Right, Mod+Left, Mod+Right; the ring on the cell; response
    priority `Selection`, then `CellFocused`, then `Chosen`.
13. Undo: Mod+Z, Mod+Shift+Z, Mod+Y (`key::Y` 21); steps and their ends as 5.1;
    `UNDO_GROUP_MS` 1000; per store 64 steps and 8192 bytes; one field history a tree, 16
    steps and 512 bytes, cleared on focus change; an oversized edit is applied, unrecorded
    and clears; `set_text` clears; read-only areas record nothing.
14. Word keys as 5.2's table; `set_mac_text_keys(bool)` from `application::run`; one shared
    class function (white space; ASCII alphanumerics, `_` and non-ASCII; other signs); moves
    skip white space then one run; Alt+arrows `Unused` outside the macOS profile.
15. Agents: `checked`, `selected`, `expandable`, `expanded` on entries; nested entries and
    `More` in an id range of their own; headers in visual order taking `fit` and `move <n>`;
    tables taking `all`, `none`, `toggle <n>`, `range <a> <b>`, `add <a> <b>`, `column <n>`;
    the value's `, column <n>` and `, <k> selected`.
16. Touch and TV rules of section 7: drill-in sheets, long-press column drag, long-press
    selection mode, OK held 500 ms.
