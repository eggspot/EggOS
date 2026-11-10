# EggOS toolkit v0.5, slice 2: tooltips, menus, tables, multi-line text, horizontal scrolling, drag inside a window, and the light theme

Design by Claude Fable (D16) for the second slice of the toolkit's next widgets (D219 implements
it). It extends `toolkit-v0.3.md` and `toolkit-v0.4.md`: every token, metric, state rule, focus
rule and pointer rule there stays, and this brief adds the next seven things their section 9
and 10 listed: **tooltips** (`Kind::Tooltip`), a **menu bar and menu buttons** (`Kind::MenuBar`,
`Kind::MenuButton`, built on `menu::Entry`), **tables** with resizable, sortable columns
(`Kind::Table`), **multi-line text** with word wrap (`Kind::TextArea`), the **horizontal axis**
of `Kind::ScrollView`, **drag and drop inside one window**, and the **light theme**, which
gives every colour token a second value and is the first thing in `Settings > Appearance`.
Every number is in px at scale 1x and doubles at 2x; colours are tokens (section 1 and the
palette of section 8; no raw value is used anywhere else). Everything is drawn by our own Rust
code on the software framebuffer: 1 px strokes, no anti-aliasing, no alpha, no motion except
the caret's blink. The widgets fit the code's model: a `Kind` variant held by value in the
retained tree (no heap), parts numbered from 0 for the element tree, a `Response` the program
acts on, `Look` for the state the tree draws, and host tests that compare pixels and responses.

## 0. What a person gets, and how that is checked

- **Tooltips**: rest the pointer on a toolbar button, a sidebar row or a bar chip for 600 ms
  and a tile names it and its shortcut (`Save  Mod+S`); it goes away the moment you press, type
  or leave. The same tile the taskbar and dock show today (`desktop-v0.3-gui.md` 6.4 item 3),
  now for any widget in any app, and listed for agents without hovering.
- **A menu bar** (`File  Edit  View  Help`) in the window in the Windows and Linux profiles,
  and in the top bar in the macOS profile (the same `Kind::MenuBar`, sent through `SET_MENU`,
  D147), with F10 or Alt, arrows, Enter, Esc, and underlined mnemonics; **a menu button**
  (`Sort by v`) for a short menu next to a control. One `menu::Entry` list serves the context
  menu, the bar and the button, so Files' and the Text Editor's menus are declared once.
- **A table**: `Name  Size  Modified` with a header you drag to resize and click to sort (an
  arrow shows which column and which way), one selected row, keys as a list, scrolling both
  ways. Files' list view and Settings' Agents log become tables.
- **A text area**: a note, a message, a description: lines wrap at words, Up/Down move by the
  line you see, selection and the clipboard work as in the field, Enter makes a new line.
- **Horizontal scrolling**: a wide table or canvas scrolls sideways with Shift+wheel or the
  sideways wheel, with a bar along the bottom.
- **Drag**: pick up a row and drop it on another row or between rows in the same window, with
  a ghost under the pointer and a highlight where it will land; Esc cancels.
- **The light theme**: `Settings > Appearance > Theme: Dark | Light`, `settings set
  appearance.theme light`, the MCP tool `settings_set`; every open window and the shell repaint
  in the new palette within one frame.

Host tests (section 13) pin the pixels and the responses; `wm show` (section 10) lists every new
widget with its value and actions, so an agent uses each without the screen (D22).

## 1. Tokens and constants

**One new colour token, `icon.badge`**: the glyph on the Windows app badge, dark `#E6EAF0`
(what `text` is today, so nothing visible changes in the dark theme), light `#FFFFFF`. It
exists because the light theme turns `text` dark while the badge stays blue. Every other new
thing is drawn with existing tokens: tooltips and the drag ghost with `menu.bg`, `menu.border`
and `text`; the menu bar with `menu.bg`, `menu.border`, `menu.hover`, `text`, `text.dim`; the
table header with `menu.bg`, `menu.border`, `text.dim`, `text`, `accent`; the drop highlight
and the sort arrow with `accent`; the text area with the field's tokens. Section 8 gives every
token its light value and recomputes every contrast pair.

New metric constants in `widget`, at 1x: `TOOLTIP_HEIGHT 24`, `TOOLTIP_PADDING 8`,
`TOOLTIP_GAP 4`, `TOOLTIP_SHORTCUT_GAP 12`, `TOOLTIP_DELAY_MS 600`, `TOOLTIP_HOP_MS 300`,
`TOOLTIP_TOUCH_MS 1500`; `MENU_BAR_HEIGHT 28`, `MENU_TITLE_PADDING 12`, `MENU_KEY_GAP 24`,
`MAX_MENUS 6` (what `SET_MENU` takes), `menu::MAX_ENTRIES` grows from 12 to 16 (likewise);
`TABLE_HEADER_HEIGHT 28`, `TABLE_ROW_HEIGHT 24`, `TABLE_CELL_PADDING 8`, `TABLE_MARK_X 8`,
`TABLE_FIRST_TEXT_X 24`, `TABLE_HANDLE 7` (the resize zone's width, centred on a separator),
`TABLE_MIN_COLUMN 40`, `TABLE_ARROW_ROOM 20`, `TABLE_KEY_SCROLL 48`, `MAX_COLUMNS 8`;
`TEXT_AREA_PADDING 8`, `TEXT_AREA_TOP 6`, `TEXT_AREA_LINE_PITCH 20`, `TEXT_AREA_WHEEL_LINES 3`,
`TEXT_AREA_TAB 16`, `TEXT_AREA_CAPACITY 65536`, `TEXT_AREA_LINES 4096`; `DRAG_THRESHOLD 6`,
`DRAG_GHOST_OFFSET 12`, `DRAG_EDGE 24`, `DRAG_SCROLL_MS 100`, `LONG_PRESS_MS 500`. Key codes
added to `input::key`: `F10 68`, `LEFT_ALT 56`, `RIGHT_ALT 100`, `MENU 127` (the Menu key),
`LEFT_SHIFT 42` and `RIGHT_SHIFT 54` (already the tree's private constants, made public).
`PointerShape` gains `ResizeEW` (the server's shape 3, over a column handle) and `Hand`
(shape 7, while a drag is in progress); nothing else changes the pointer.

## 2. Tooltip

### 2.1 Kind

```
Kind::Tooltip(Tooltip)
pub struct Tooltip { pub text: Text<48>, pub shortcut: Text<16> }
Kind::tooltip(text, shortcut) -> Kind        (shortcut "" for none)
```

A tooltip is a **child of the widget it describes**: `tree.add(Some(save_button),
Place::at(0, 0, Length::Content, Length::Content), Kind::tooltip("Save", "Mod+S"))`. It is not
laid out among its parent's children (a `Column` skips it; it adds nothing to a scroll view's
content) and is not focusable or interactive; the tree draws it last, after popups, only while
`shown` (section 2.3). A widget has at most one; a second one replaces the first. Tooltips
describe widgets whose look does not say it all: icon-only buttons (the Text Editor's toolbar),
truncated rows, bar chips. A push button with words needs none.

### 2.2 Geometry at 1x

The panel tooltip's chrome, in Egg Sans: **24 tall**, `menu.bg` fill, 1 px `menu.border`
outline, `INSET_R4` corners, no shadow. `width = 8 + width(text) + 8`, plus `12 +
width(shortcut)` when there is a shortcut; the text's cell top at +4 at `x + 8` in `text`, the
shortcut after a 12 px gap in `text.dim` (`Save  Mod+S`: 8 + 24 + 12 + 32 + 8 = 84 wide).
Placement, in window content coordinates: **below** its owner, `y = owner.bottom + 4`, centred
on the owner's horizontal centre (`x = owner.x + (owner.width - width) / 2`), clamped
`8 .. window width - width - 8` (or `0` when the window is narrower than `width + 16`); when
`y + 24 > window height`, **above** at `owner.y - 4 - 24`; a tooltip whose owner is partly
scrolled out of view anchors to the visible part. Its rect is clipped to the window content.
Below rather than beside the pointer because the owner, not the pointer, is what it names, and
because the top bar's tooltips already sit below their item (`desktop-v0.3-gui.md` 14.1).

### 2.3 When it shows and hides

| Event | Effect |
|-------|--------|
| the pointer enters the owner (hover changes to it) | `hover_since` is noted; moving within the owner does not reset it |
| `tick` at `hover_since + 600` with the pointer still over the owner, no popup, menu or drag open, not `spent` | shown; the tooltip's rect damaged |
| the pointer leaves the owner | hidden; `spent` cleared; `hidden_at` noted |
| the pointer enters another owner within 300 ms of `hidden_at` (`TOOLTIP_HOP_MS`) | that owner's tooltip shows at once (the hop every desktop does along a toolbar) |
| a press of any button, anywhere | hidden and `spent`: it does not come back until the pointer leaves the owner |
| any key down | hidden and `spent` (a tooltip over the thing you are typing into is in the way) |
| a popup, menu or drag opens | hidden; none shows while one is open |
| focus moves to the owner by Tab or arrows (keyboard focus, window focused) | shown after 600 ms, as for hover, so a keyboard user reads the toolbar; hidden by the next key, press or focus move |
| the owner is hidden, disabled or re-laid out by a resize, or the window loses focus | hidden |

No `Response`: a tooltip changes nothing the program needs to know. Pointer shape unchanged
(the owner's). A disabled owner still shows its tooltip (it says what the greyed thing is).

### 2.4 Touch, TV, wearable

Phone: **long-press** (a press held 500 ms, `LONG_PRESS_MS`, without moving past the drag
threshold) on an owner shows its tooltip for 1500 ms (`TOOLTIP_TOUCH_MS`), then it hides; the
release is not a click (the press was spent on the tooltip). Android's long-press tooltip, so
a switcher's habit holds. TV: the focus rule of 2.3 is the only one (the pointer is hidden).
Wearable: tooltips never show (nothing hovers and the screen holds one thing); the element
tree still lists them, so an agent reads the names.

## 3. Menu bar and menu button

### 3.1 Entries

`menu::Entry::Item` gains `key: &'static str`, the shortcut shown after the label (`Mod+S`,
`F5`, `""` for none); the context menus Files and the Text Editor build today pass `""`. The
new types are over `Entry` and keep `Menu`'s rules (items 32 tall, separators 9, text at +12,
`menu.hover` for the highlighted item, disabled items `text.dim` and skipped by the arrows,
clamped 8 px inside the bounds):

```
pub struct Bar {
    pub titles: [&'static str; MAX_MENUS],
    pub menus: [&'static [Entry]; MAX_MENUS],
    pub count: usize,
    pub open: Option<usize>,
    pub highlighted: Option<usize>,
    pub mnemonics: bool,
}
pub struct Button {
    pub text: Text<32>,
    pub entries: &'static [Entry],
    pub open: bool,
    pub highlighted: Option<usize>,
}
Kind::MenuBar(menu::Bar)     Kind::menu_bar(&[(title, entries), ..]) -> Kind
Kind::MenuButton(menu::Button)   Kind::menu_button(text, entries) -> Kind
```

A label's **mnemonic** is the character after `&` (`&File`, `E&xit`); `&&` is a literal `&`.
The `&` is never drawn. Underlines show while `mnemonics` is true: from the first Alt press
until the bar closes (Windows's rule); the macOS profile never draws them and the top bar
ignores them (section 3.4).

### 3.2 The bar's geometry at 1x

**28 tall** (`MENU_BAR_HEIGHT`, the title bar's height), the window's full width
(`Length::Fill(0)`), `menu.bg` fill, a 1 px `menu.border` line along its bottom row. Titles
from `x + 12`: each title's cell at `title.x + 12` (`MENU_TITLE_PADDING`), cell top +6, in
`text`; its hit rect is `width(title) + 24` wide and the bar's full height; the next title
starts where that rect ends (`File` 15 wide: rect 0..39, `Edit` 17: 39..80, `View` 22:
80..126, `Help` 20: 126..170). The **open** title is filled `menu.hover` over its hit rect,
rows 0..27 (the border row stays); hover over a closed title while no menu is open fills it
`menu.hover` too (the one hover fill menus have). A mnemonic underline is a 1 px `text` line
under the character's advance (minus its spacing column) at row 14 of the cell.

**The open menu** is a `menu::Menu` placed at `(title.x, bar.bottom + 1)`, bounds the window
content (its `new` clamps it inside), drawn last as the dropdown's popup is, width by its
widest label plus its widest key: `12 + widest label + 24 + widest key + 12`, at least 160
(`MIN_WIDTH`), rounded up to 8; keys right-aligned ending at `menu.right - 12` in `text.dim`
(`text.dim` on `menu.hover` 8.3:1 in the dark palette). A disabled item draws label and key in
`text.dim`. No submenus in this slice (section 14).

**The menu button**: a push button of toolkit section 4 (28 tall, `button.bg`, `button.border`,
`INSET_R4`, text at least 16 in from the left) with the dropdown's chevron (nine 2x2 squares,
10x6) in the rightmost 32 px (`DROPDOWN_ARROW_ROOM`); `Length::Content` is `(16 + width(text) +
32, 28)`, at least 96. Its menu opens below it at `(button.x, button.bottom + 4)`, or above when
below does not fit, as the dropdown's popup. The button stays `button.pressed` while its menu
is open and keeps the focus (the ring on the button); the menu is part of the widget.

### 3.3 Pointer, keys

- **Bar, pointer**: a press on a title opens its menu at once, `highlighted` none; with a menu
  open, moving over another title switches to it without a press (every desktop's bar); a
  release over the title that opened it leaves it open, a release over an enabled item chooses
  it (press, slide, let go). Over an open menu, hover and highlight are one thing (`Menu::
  hover`). A click on the open title closes it. A press outside the bar and its menu closes
  the menu and is **consumed** (as the dropdown's: a mis-click closes and does nothing else).
- **Bar, keys**: F10, or Alt pressed and released alone (no other key between, the tree's
  `alt_held` already tracks it), **arms** the bar: the first title is highlighted
  (`menu.hover`, no menu open), the keyboard goes to the bar, mnemonics show. Left/Right move
  the highlight between titles, wrapping; Down, Enter or Space opens the highlighted title's
  menu with its first enabled item highlighted; in an open menu Up/Down move as `Menu::key`,
  Left/Right switch to the previous or next title's menu (wrapping) with its first item
  highlighted, Enter chooses, Esc closes the menu and leaves the bar armed, a second Esc (or
  Esc on an armed bar) disarms it and returns the keyboard to the widget that had focus. A
  mnemonic letter while armed opens that title's menu; in an open menu it chooses the item with
  that mnemonic (or highlights it when two share one). **Alt+letter** from anywhere in the window
  opens that title's menu directly (`Alt+F` for `&File`). Every key the bar does not use while
  armed is swallowed (the bar has the keyboard, as on Windows); the arming does not move the
  tree's `focus`, so disarming restores it with no event.
- **Button**: a press or Space, Enter, Alt+Down opens; in the open menu Up/Down/Home/End move,
  Enter or Space chooses, Esc closes; Tab closes and moves on; a press outside closes and is
  consumed. Up/Down on a closed menu button do nothing (unlike a dropdown: a menu is a list of
  actions, not a value).
- **Shortcuts are not the bar's job**: the `key` text is a label. The program binds the keys
  it shows (its `Unused { code, shift }` handling today), so what the menu says and what the
  keys do are the program's one table, `menu::Entry`'s `key` printed from it.

### 3.4 Response, and the top bar in the macOS profile

Choosing an item returns **`Response::Invoked(WidgetId, u8)`**: the widget and the entry's
`id`, the same for the bar, the button, a mnemonic and an agent's `invoke`. Opening, moving
and closing return `Nothing`. A disabled item cannot be chosen.

In the **macOS profile** the in-window bar is not drawn: at creation and whenever the bar
changes (`change` on it), `application::run` sends the bar's titles, labels, keys and disabled
states as the window's main menu (`SET_MENU`'s pool, `M<title>`, `I<label>\t<key>`,
`D<label>\t<key>`, `-`; mnemonic `&` stripped) and gives the `MenuBar` node height 0 (`shown`
stays true, so the element tree still lists it), and the `MenuChosen` event (menu index, entry
index) is turned into `Invoked(id, entry.id)` by the tree (`WidgetTree::menu_chosen(menu,
entry) -> Response`). When the profile changes at run time (`SET_PROFILE`), `run` repeats this
the other way: the in-window bar gets its 28 px back and `SET_MENU` with length 0 removes the
top-bar menu, in the frame the window is re-laid out. README's open question ("whether to ship
a global menu bar outside the macOS profile") stays open; this brief follows the table there:
in-window by default, global in the macOS profile.

## 4. Table

### 4.1 Kind

```
pub struct Column {
    pub title: &'static str,
    pub width: i32,
    pub min_width: i32,
    pub align: Align,
    pub sortable: bool,
}
pub enum SortOrder { Ascending, Descending }
pub type CellText = fn(data: u32, row: usize, column: usize, out: &mut Text<64>);
pub struct Table {
    pub name: &'static str,
    pub columns: [Column; MAX_COLUMNS],
    pub column_count: usize,
    pub rows: usize,
    pub cell: CellText,
    pub data: u32,
    pub selected: usize,
    pub sort: Option<(usize, SortOrder)>,
    pub offset_y: i32,
    pub offset_x: i32,
    pub empty_text: &'static str,
}
Kind::table(name, &[Column], rows, cell, data) -> Kind
```

The tree holds **no cell text**: it asks the program for a cell when it draws or describes it
(`cell(data, row, column, &mut out)`, the `Custom` painter's pattern), so a table of a million
rows costs the tree nothing but its state. **Sorting is the program's**: a header click
returns `Sorted` (4.5), the program reorders what `cell` reads from and calls `set_rows(id,
rows)` (which damages the table and keeps `selected` in range); the tree only draws the
arrow for `sort`. `Column::width` is the current width, changed by a drag; `min_width` at least
`TABLE_MIN_COLUMN` (40). `WidgetTree::selected`, `set_selected`, `set_rows`, `set_sort(id,
Option<(usize, SortOrder)>)` and `set_column_width(id, column, width)` work on it, send nothing
and damage the table. `name` is the element's name (section 10).

### 4.2 Geometry at 1x

- **Header**: the top 28 rows (`TABLE_HEADER_HEIGHT`), the table's surface (`menu.bg`), a 1 px
  `menu.border` line along its bottom row (y + 27). Column `c` spans `x_c .. x_c + width_c`
  where `x_0 = table.x - offset_x` and `x_{c+1} = x_c + width_c`; between columns a 1 px
  `menu.border` separator at `x_{c+1} - 1`, rows 6..21 of the header (a short tick, not the full
  height). Titles in `text.dim`, cell top +6, at `x_c + 8`, ellipsised at `x_c + width_c - 8`
  (`- 20` on the sorted column, `TABLE_ARROW_ROOM`); the **sorted column's** title is `text` and
  its arrow is the dropdown's chevron (nine 2x2 squares, 10x6) in `accent`, right-aligned with
  its right edge at `x_c + width_c - 8`, top at y + 11, pointing **up for ascending** (A at the
  top, the arrow shows which way the column reads) and down for descending. `Align::End`
  columns (numbers) right-align title and cells to `x_c + width_c - 8` (the arrow then sits at
  the column's left, `x_c + 8`, and the title ends at `- 8` as usual).
- **Rows**: 24 tall (`TABLE_ROW_HEIGHT`) from the header's bottom (y + 28), row `r` at `y + 28 +
  r * 24 - offset_y`, clipped to the body (`y + 28 .. bottom`, less the horizontal bar when it
  shows). Cell text cell top +4 at `x_c + 8`, ellipsised at `x_c + width_c - 8`; **the first
  column** keeps the list's mark column: its text at `x_0 + 24` (`TABLE_FIRST_TEXT_X`) and the
  selected row's `>` in `accent` at `x_0 + 8` (`TABLE_MARK_X`), the non-colour cue every
  selectable list has (toolkit section 6). A row's fill spans the table's full width (not the
  columns' total), so a selected row reads as one bar even past the last column.
- **Content width** is `sum(width_c)`; when it exceeds the table's width (less the vertical
  bar's 8 when it shows), the **horizontal bar** of section 6 lies along the body's bottom 8 px,
  and the header and the rows scroll together by `offset_x`. The **vertical bar** (toolkit
  section 8) sits in the body's right 8 px, from the header's bottom to the horizontal bar's
  top; the 8x8 corner where both meet is the surface. `Length::Content` is `(sum of widths, 28
  + rows * 24)`, so a table given `Content` height scrolls in its parent instead.
- **Hit areas**: a **handle** is the 7 px zone (`TABLE_HANDLE`) centred on each separator
  (`x_{c+1} - 4 .. x_{c+1} + 3`), header rows only, pointer `ResizeEW` over it (the last
  column's right edge has one too, so the last column resizes); the rest of a header cell is
  the **header**, pointer `Arrow`; the body's rows, the two bars' tracks and thumbs as before.
  A disabled table ignores the pointer; an empty table (`rows == 0`) draws `empty_text` in
  `text.dim` at `x_0 + 24` of the first row.

### 4.3 States

| Part | State | Fill | Text / mark |
|------|-------|------|-------------|
| header cell | normal | `menu.bg` | `text.dim` |
| header cell | hover (sortable only) | `menu.hover` | `text.dim` |
| header cell | pressed | `button.pressed` | `text.dim` |
| header cell | sorted | as above | `text`, arrow `accent` |
| handle | hover or dragging | the separator becomes 1 px `text.dim`, full header height | |
| row | normal | the surface | `text` |
| row | hover | `menu.hover` | `text` |
| row | selected, table unfocused | `selection` | `text`, `>` `accent` |
| row | selected, table focused | `selection` | `text`, `>`, plus the ring at inset 0 |
| row | drop target (section 7) | `menu.hover` and a 2 px `accent` outline, or the insertion line | `text` |
| whole table | disabled | unchanged | `text.dim` everywhere, no `>` |

Hover on the selected row changes nothing. A column that is not `sortable` has no hover or
pressed look and no arrow, and a click on its header does nothing.

### 4.4 Pointer, keys, wheel

- **Select**: a press on a row selects it (`Chosen`) and focuses the table; a double-click
  (two presses within `DOUBLE_CLICK_MS`) or Enter is `Opened`; a right-click is passed to the
  window with the row selected first (so a context menu acts on what is under the pointer).
- **Sort**: a click (press and release inside the same header cell, not on a handle) on a
  sortable column: a column not sorted becomes `Ascending`; the sorted column flips order.
  Never "no sort" by a third click (Windows and macOS flip; GNOME's third state is left out).
- **Resize**: a press on a handle starts a drag; every `PointerMove` sets `width_c = max(
  min_width_c, start_width + dx)` live, re-laying the header and the rows in the same frame;
  release ends it and returns `ColumnResized` (4.5) when the width changed; Esc during it
  restores `start_width` and sends nothing. A double-click on a handle does nothing in this
  slice (fit-to-content needs every row's width: section 14).
- **Keys** (focused): Up/Down one row, Home/End first and last, PageUp/PageDown by the visible
  rows minus one, the body scrolling to keep the selection visible (the list's rules); Left and
  Right scroll `offset_x` by 48 (`TABLE_KEY_SCROLL`) when the content is wider, else nothing
  (no cell focus in this slice); Enter `Opened`; Space nothing; Tab leaves. Sorting by keyboard
  is the program's `View > Sort by` menu (Files has one), as on macOS; an agent sorts with
  `invoke` on the header (section 10).
- **Wheel**: 72 px vertical (3 rows), rounded to whole rows as the list does; **Shift+wheel**
  and the **sideways wheel** scroll `offset_x` by 72 (section 6). A wheel on the header scrolls
  the body (the header is part of the table).
- **Disabled**: ignores the pointer and keys, skipped by Tab.

### 4.5 Responses

`Response::Chosen(id, row)` when the selected row changes; `Response::Opened(id, row)` on a
double-click or Enter; **`Response::Sorted(WidgetId, usize, SortOrder)`** on a header click
(the tree has already set `sort`, so the arrow is right in the same frame; the program reorders
and calls `set_rows`, or calls `set_sort(id, None)` to refuse); **`Response::ColumnResized(
WidgetId, usize, i32)`** on the release that ends a resize, so a program that keeps widths
saves them once, not per pixel. Scrolling returns `Nothing`.

## 5. Text area

### 5.1 Kind and where the bytes live

```
Kind::TextArea(TextArea)
pub struct TextArea {
    pub store: usize,
    pub caret: usize,
    pub anchor: usize,
    pub scroll: i32,
    pub wanted_x: Option<i32>,
    pub placeholder: &'static str,
    pub lines: usize,
}
Kind::text_area(store, placeholder) -> Kind
```

A `Kind` is held by value in every node, so 64 KiB of text cannot live in it, and the toolkit
has no heap. The bytes live in the tree: **`WidgetTree<const N: usize, const A: usize = 0>`**,
where `A` is how many text stores the tree carries (`WidgetTree<64>` stays `WidgetTree<64, 0>`,
so Settings and every program without a text area pay nothing). A store is `text::Store {
bytes: [u8; TEXT_AREA_CAPACITY], length: usize, lines: [u32; TEXT_AREA_LINES], line_count:
usize }`: 64 KiB of UTF-8 with `\n` breaks, and the **visual line index** (the byte where each
wrapped line starts) the wrap pass of 5.3 rebuilds. Typing, pasting or `set_text` beyond 64 KiB
or 4096 visual lines is ignored past the limit, as the field ignores its 256th byte. `store` is
the index of the store the area uses; two areas never share one. `text_of(id)` returns the
area's text, `set_text(id, words)` replaces it (caret at the end, rewrapped), `copied()`,
`paste(id, text, now)` work as for the field, keeping `\n` and tabs and dropping other control
characters. The Text Editor keeps its own `Document` (4 MiB, a line index of its own): a text
area is for a page of text in a form, not a file.

### 5.2 Geometry at 1x

The field's chrome made tall: `field.bg` fill, 1 px `button.border`, `INSET_R4` corners; the
program gives the height (`Length::Content` is `(240, 92)`: four lines). The **inner rect** is
`x + 8 .. right - 16` (`TEXT_AREA_PADDING` left; padding plus the vertical bar's 8 on the right,
**always reserved**, bar or not, so the wrap width never changes when the bar appears) and
`y + 6 .. bottom - 6`. Lines are 20 apart (`TEXT_AREA_LINE_PITCH`), line `i`'s cell top at
`y + 6 + i * 20 - scroll`; content height `lines * 20 + 12`; the vertical bar of toolkit
section 8 in the right 8 px of the control, inside the border (rows `y + 1 .. bottom - 1`), when
the content is taller. A tab is drawn as 16 px of space (`TEXT_AREA_TAB`); the placeholder in
`text.dim` on the first line while the text is empty. Hover: border `text.dim`; focused: the
ring, the caret; disabled: text and border `text.dim`, no caret.

### 5.3 Wrapping

The **wrap pass** turns the bytes into visual lines for one width (`wrap_width = inner
width`), in one left-to-right pass over the bytes with no second pass and no allocation:

- A hard break (`\n`) ends a line; the byte after it starts the next. The `\n` belongs to the
  line it ends and has no width.
- Within a paragraph, glyph advances are summed; a **break opportunity** is after every space
  (and after a tab). When a glyph would pass `wrap_width`, the line ends at the last
  opportunity, the spaces before the break stay on the upper line (their advance may pass the
  edge: they are not drawn past the inner rect, as every browser and editor does), and the
  next line starts at the first non-space byte after them.
- A **word wider than the line** (no opportunity on the line) breaks before the glyph that
  does not fit: character wrapping, so nothing is ever clipped.
- The index holds each line's first byte; the line's end is the next line's start (or
  `length`). The pass runs after every edit from the line containing the edit's first byte to
  the end of the text (a paragraph's wrap cannot affect the lines above it), and over the
  whole text after `set_text`, a resize of the control or a scale change.

### 5.4 Caret, selection, keys, pointer

The field's rules of toolkit v0.3 section 7 and v0.4 section 4 hold (caret 1x16 in `text`,
blink, `anchor`, Shift extends, `Mod+A/X/C/V`, `Copied`, `Cut`, `PasteWanted`, the word rule of
a double-click, the click count), with these multi-line rules:

| Input | Effect |
|-------|--------|
| Enter | inserts `\n` (never the default button); **Mod+Enter** goes to the window's default button (macOS's Cmd+Return) |
| Up / Down | the caret moves to the previous / next visual line at `wanted_x` (the x it had before the first vertical move, kept until a horizontal move, click or edit), to the nearest boundary; at the first / last line, to the start / end of the text |
| Home / End | the start / end of the visual line (End before the trailing spaces of a wrapped line); Mod+Home / Mod+End the start / end of the text |
| PageUp / PageDown | by the visible lines minus one, keeping `wanted_x` |
| Left / Right | across line breaks (the byte before a line's first byte is the previous line's last) |
| Tab | leaves the widget (a tab is pasted, never typed) |
| Esc | goes to the window (no `escape` setting: a text area is not a filter) |
| triple-click | selects the paragraph (between hard breaks), macOS's rule |
| drag past the inner rect's top or bottom | each further `PointerMove` moves the caret one more line and scrolls to show it |
| wheel | 3 lines (60 px, `TEXT_AREA_WHEEL_LINES`), clamped; the caret stays |

A caret at a wrapped line's first byte draws at that line's start (never at the end of the
line above). The **selection fill** is `selection` per visual line: from the x of the first
selected boundary to the x of the last on that line, or to the line's last glyph's end when the
selection continues past it, the cell's 16 rows; `\n` adds nothing. The caret or the selection's
moving end is kept in view after every edit or move (`scroll` changes by whole lines so the
first visible line is always a full one). Responses: `Edited(id)`, `Copied`, `Cut`,
`PasteWanted` as the field's.

## 6. Horizontal scrolling

`Kind::ScrollView { offset, content_height }` gains **`offset_x: i32` and `content_width:
i32`**. `content_width` is the greatest right edge of the shown children (a `Fill` child's width
is the view's own, so it never widens the content); when it exceeds the view's width, a
**horizontal bar** 8 tall lies inside the view's bottom edge: track `progress.track`, thumb
`bar.pressed` (`button.inactive` hovered or dragged), `INSET_R4`, length `view_w * view_w /
content_w`, at least 24, position proportional to `offset_x`. When both bars show, the vertical
one stops 8 above the bottom, the horizontal one 8 before the right edge, and the 8x8 corner
is the view's surface; content rows reserve both strips only when their bar shows (the vertical
rule of toolkit section 8, mirrored). Children are laid out at `place.left - offset_x`;
`offset_x` ranges over `0 .. content_w - track_w`, where `track_w` is the view's width less the
vertical bar's 8 when it shows, so the content's last column can be scrolled clear of the bar.

**Wheel**: `Input::SidewaysWheel { notches }` scrolls `offset_x` by `72 * notches`
(`WHEEL_STEP`; positive is right); **Shift + `Wheel`** does the same, up meaning left (the
convention of Windows, GTK and the web); the tree reads Shift from its `shift_held`. A wheel
goes to the **innermost** shown list, table, text area or scroll view under the pointer whose
content is larger than its view on that axis; when none is, it goes nowhere (no chaining to
an outer view on the other axis: a horizontal wheel over a tall list must not scroll the pane
sideways). Dragging the horizontal thumb and clicking its track work as the vertical bar's
(`Drag` gains `axis` and `grab_x`). `wm scroll` already sends sideways notches (D162). The
table (section 4) and the text area are their own scrollers and take the same rules.

## 7. Drag inside a window

### 7.1 Sources and targets

The program declares what can be picked up and where it can land, per widget:
`WidgetTree::set_drag_source(id, bool)` on a list, a table or a `Custom` widget (its rows or
the whole custom widget are the sources), and `set_drop_target(id, DropMode)`, with
`DropMode::Onto` (a row, or the whole widget, takes the drop: a folder, a bin) or
`DropMode::Between` (the drop goes between rows: reordering); `None` clears it. A list or table
may be both (Files' places list: move a bookmark, drop a folder on one).

### 7.2 What happens

- **Threshold**: a press on a source row that moves 6 px (`DRAG_THRESHOLD`, Manhattan) in any
  direction before its release starts a drag; under 6 px it stays a click. The row is not
  selected by the press until the release that was not a drag (so dragging a row does not
  change the selection; macOS Finder's habit), except that the pressed row is drawn hovered.
  The tree returns **`Response::DragStarted(source, part)`** on the move that crosses the
  threshold; the program may answer by calling `tree.cancel_drag()` (a row that cannot move),
  else the drag is on. Pointer `Hand` while it lasts.
- **The ghost**: the tooltip's tile (`menu.bg`, 1 px `menu.border`, `INSET_R4`, 24 tall, text
  at +8, cell top +4, in `text`) with the source row's first-column text (the program's `cell`,
  or the list's item; a custom source gives it with `set_drag_text(id, text)`), its top-left at
  `(pointer x + 12, pointer y + 12)` (`DRAG_GHOST_OFFSET`), clipped to the window content, drawn
  last of all, above popups. Opaque: the framebuffer has no alpha, so no translucent copy of the
  row.
- **Over a target**: `Onto`: the row under the pointer (or the whole widget when it has no
  rows) is filled `menu.hover` with a 2 px `accent` outline (`INSET_R4`) at inset 0; `Between`:
  a 2 px `accent` line from `x + 8` to `right - 8` at the nearest row boundary (above the row
  when the pointer is in its top half, below otherwise; after the last row at its bottom; at
  the first row's top for an empty target). Over the source's own row (`Onto`) or at the
  boundaries around it (`Between`) nothing highlights and a release does nothing. Over no
  target the pointer stays `Hand` and nothing highlights (the server has no "not allowed"
  shape; the missing highlight is the cue). In the top or bottom 24 px (`DRAG_EDGE`) of a
  scrollable target the target scrolls one row every 100 ms (`DRAG_SCROLL_MS`, by `tick`).
- **Release** over a highlighted spot returns **`Response::Dropped { source, part, target,
  at }`**: `at` is the row index for `Onto` (or 0 for a whole-widget target) and the insertion
  index `0 ..= rows` for `Between`. Release anywhere else, **Esc**, the window losing focus, or
  the source being hidden or disabled returns **`Response::DragCancelled(source)`**; every
  ending removes the ghost and the highlight. The data moved is the program's: the tree says
  what was picked up and where it was let go.
- **Touch**: a long-press (500 ms) on a source row picks it up (the ghost appears under the
  finger, offset `(12, -36)` so the finger does not cover it); then a move drags. **Between
  windows** is a later slice: it needs the server to carry a payload with the pointer across
  windows (the shared clipboard of D197 is the model) and D162's `wm drag` to cross windows
  too; the in-window protocol here is designed so that slice adds a `Dropped` from a foreign
  source without changing the targets.

## 8. The light theme

### 8.1 Theme and palette

```
pub enum Theme { Dark, Light }
pub struct Palette { pub text: u32, pub text_dim: u32, ... one field per token ... }
pub const DARK: Palette;  pub const LIGHT: Palette;
pub const DARK_HIGH_CONTRAST: Palette;  pub const LIGHT_HIGH_CONTRAST: Palette;
pub enum Token { Text, TextDim, Menu, MenuBorder, MenuHover, Bar, ... one per token }
impl Palette { pub const fn get(&self, token: Token) -> u32 }
```

`colour.rs`'s constants become the fields of `DARK`, and the toolkit draws through the
palette it was given: `Canvas` carries `&'static Palette`, and `Kind::Label.colour`,
`Kind::Panel.fill` and `border_right` take a **`Token`**, not a `u32`, so a label made in the
dark theme is right in the light one without being rebuilt. `Custom` painters read
`canvas.palette()`. `WidgetTree::set_palette(&'static Palette)` damages everything. The four
palettes are the two themes by the two contrast settings (`first-run-accessibility-v0.3.md`
2.4 planned the `Palette` and the `PaletteChanged` event for high contrast; this brief fills
in the light rows): index `theme + 2 * high_contrast`.

### 8.2 The light palette

Every token of `desktop-v0.3.md` section 2, `desktop-v0.3-gui.md` section 1 and `toolkit-v0.3.md`
section 1, its dark value for reference, its light value, and the light high-contrast value.
Ratios are WCAG, computed from the values; text pairs are at least **4.5:1 (AA)** in light and
**7:1 (AAA)** in light high contrast. The light theme is a cool white with the yolk kept for
the primary button and a deep amber for the marks and the ring, since the yolk itself is 1.7:1
on white and a 2 px ring must reach 3:1.

| Token | Dark | Light | Light, high contrast | Light pairs |
|-------|------|-------|----------------------|-------------|
| `wallpaper.top` | `#0E1726` | `#DCE4EE` | `#FFFFFF` | desktop icon labels `text` on it 13.5:1 |
| `wallpaper.bottom` | `#1B3A5C` | `#B9CBE0` | `#FFFFFF` | `text` 10.4:1; no gradient in high contrast |
| `wallpaper.egg` | `#1F3F63` | `#C7D6E6` | `#E6E6E6` | decoration, faint on purpose |
| `accent` | `#F2B33D` | `#9A6200` | `#7A4E00` | on `menu.bg` 4.7:1, on `button.bg` 5.1:1, on `selection` 3.8:1 (the `>` mark and the sort arrow, graphics ≥ 3:1) |
| `shell` | `#F6F1E7` | unchanged | unchanged | the boot screen precedes the settings file |
| `bar.bg` | `#141B26` | `#E9EDF2` | `#FFFFFF` | `text` 14.7:1, `text.dim` 5.0:1 |
| `bar.edge` | `#202938` | `#C9D0DA` | `#000000` | the bar's edge |
| `bar.hover` | `#26303F` | `#DCE2EA` | `#E6E6E6` | `text` 13.3:1, `text.dim` 4.5:1 (the idle Agent chip) |
| `bar.pressed` | `#334052` | `#D3DAE3` | `#D9D9D9` | `text` 12.3:1, `text.dim` 4.1:1 (pressed, momentary; never a resting pair) |
| `menu.bg` | `#1C2430` | `#F4F6F9` | `#FFFFFF` | `text` 16.0:1, `text.dim` 5.4:1 |
| `menu.border` | `#2A3340` | `#C9D0DA` | `#000000` | 1.4:1 on `menu.bg`: borders are not the only cue, as in the dark theme (1.3:1) |
| `menu.hover` | `#2A3440` | `#E3E8EF` | `#E6E6E6` | `text` 14.0:1, `text.dim` 4.7:1 |
| `text` | `#E6EAF0` | `#161B22` | `#000000` | |
| `text.dim` | `#8A94A6` | `#5B6675` | `#3D4650` | on `field.bg` 5.8:1 (placeholders), on `button.bg` 5.8:1 (disabled) |
| `title.active` | `#2A3340` | `#DCE2EA` | `#E6E6E6` | `text` 13.3:1; the controls `#E5534B` 2.8:1, `#E8B33C` 1.5:1, `#3DBE5C` 1.8:1 (coloured circles on a light title, macOS's light appearance has the same; high contrast outlines them 1 px `#000000`) |
| `title.inactive` | `#1F2630` | `#ECEFF3` | `#FFFFFF` | `text.dim` 5.1:1 |
| `border.active` | `#3A4454` | `#9AA5B4` | `#000000` | the focused window's edge, 2.5:1 on white |
| `border.inactive` | `#2A3340` | `#C9D0DA` | `#6B6B6B` | |
| `button.close` | `#E5534B` | unchanged | unchanged | |
| `button.minimise` | `#E8B33C` | unchanged | unchanged | |
| `button.maximise` | `#3DBE5C` | unchanged | unchanged | |
| `button.inactive` | `#4A5466` | `#B0B9C6` | `#6B6B6B` | the scroll bar's hovered thumb, 1.5:1 on its track (a moving shape) |
| `term.bg` | `#0B0E13` | `#FBFCFD` | `#FFFFFF` | |
| `term.fg` | `#D7DEE8` | `#1E2630` | `#000000` | 14.9:1 |
| `term.cursor` | `#F2B33D` | `#9A6200` | `#7A4E00` | 5.0:1; the glyph under it stays `term.bg` |
| `term.selection` | `#2E4A6B` | `#BBD3F0` | `#CCE0FF` | `term.fg` on it 10.0:1 |
| `window.bg` | `#12161D` | `#FFFFFF` | `#FFFFFF` | carries no text |
| `selection` | `#2E4A6B` | `#CFE0F5` | `#CCE0FF` | `text` 12.9:1, `accent` 3.8:1 |
| `focus.ring` | `#F2B33D` | `#9A6200` | `#7A4E00` | on `menu.bg` 4.7:1, on `field.bg` 5.1:1 (a graphic ≥ 3:1); `icon.dark` on the primary button, as before |
| `field.bg` | `#0B0E13` | `#FFFFFF` | `#FFFFFF` | `text` 17.3:1 |
| `button.bg` | `#2A3440` | `#FFFFFF` | `#FFFFFF` | `text` 17.3:1 |
| `button.border` | `#3A4454` | `#B8C1CC` | `#000000` | 1.7:1 on `menu.bg` (the button's edge; the fill differs from the surface too) |
| `button.primary` | `#F2B33D` | unchanged | unchanged | `icon.dark` 10.4:1 (11.3:1 in high contrast, whose `icon.dark` is `#000000`) |
| `button.hover` | `#344050` | `#F0F3F7` | `#E6E6E6` | `text` 15.5:1 |
| `button.pressed` | `#202833` | `#DCE2EA` | `#D9D9D9` | `text` 13.3:1, `text.dim` 4.5:1 |
| `button.primary.hover` | `#F6C25C` | unchanged | unchanged | `icon.dark` 11.8:1 |
| `button.primary.pressed` | `#D99E2E` | unchanged | unchanged | `icon.dark` 8.2:1 |
| `progress.track` | `#26303F` | `#DCE2EA` | `#D9D9D9` | the slider's and the bars' track |
| `dock.bg` | `#1C2430` | `#F4F6F9` | `#FFFFFF` | as `menu.bg`, as in the dark theme |
| `icon.dark` | `#0B0E13` | unchanged | `#000000` | glyphs on the yolk and the light badges |
| `icon.tile` | `#26303F` | `#DCE2EA` | `#E6E6E6` | `text` 13.3:1, `text.dim` 4.5:1 |
| `icon.tile.border` | `#3A4454` | `#B8C1CC` | `#000000` | |
| `icon.prompt` | `#5FB865` | `#2E7D32` | `#1B5E20` | 3.9:1 on `icon.tile` (a graphic) |
| `icon.folder` | `#E2B341` | unchanged | unchanged | a graphic on the tile |
| `icon.folder.back` | `#B88A24` | unchanged | unchanged | |
| `icon.gear` | `#8A94A6` | `#5B6675` | `#000000` | 4.5:1 on `icon.tile` |
| `icon.gear.hole` | `#1F2630` | `#DCE2EA` | `#E6E6E6` | the tile shows through |
| `icon.windows` | `#4F8FE0` | `#2F6FD0` | `#1F4FA0` | `icon.badge` 4.9:1 |
| `icon.badge` (new) | `#E6EAF0` | `#FFFFFF` | `#FFFFFF` | |
| `icon.mac` | `#C9D1DB` | unchanged | unchanged | `icon.dark` 12.5:1 |
| `icon.linux` | `#E2B341` | unchanged | unchanged | `icon.dark` 9.9:1 |

**ANSI colours in the light terminal** (`term.bg` `#FBFCFD`): normal 0-7 `#1E2630` `#B3261E`
`#2E7D32` `#8A5A00` `#1F5FBF` `#8E2EA0` `#0B7285` `#65707F` (14.9, 6.4, 5.0, 5.8, 5.9, 6.7, 5.4,
4.9:1); bright 8-15 `#4A5566` `#C7342B` `#2F8F3A` `#9A6200` `#2F6FD0` `#9B3DAE` `#157F95`
`#8A94A6` (7.3, 5.2, 4.0, 5.0, 4.7, 5.6, 4.5, 3.0:1). Two deliberate exceptions, mirroring the
dark theme's ANSI 0: bright green at 4.0:1 (a lighter green on white cannot reach 4.5 and stay
green) and **bright white** at 3.0:1, the colour programs use for "brightest" and which on a
white page must be the faintest; it stays visible rather than becoming `term.bg`. Light high
contrast takes the normal row for bright and darkens the normal row by one step (every colour
but 15 at 7:1 or better; a short table in the implementing PR, as the dark one was). The
terminal keeps `term.*` tokens of its own: a person may later pick a dark terminal in a light
desktop (section 14).

### 8.3 Where the setting lives and how everything repaints

- **Setting**: `appearance.theme`, values `dark` and `light`, default `dark`; a line
  `appearance.theme=light` in the settings file beside `shell.profile` (`eggos-settings-file`),
  read by the kernel at boot as the profile is (D132), so the boot screen is still the dark
  one (`shell` and the boot tokens have one value) and the desktop's first frame is already in
  the chosen theme. Without a disk the choice lasts until power-off, as the profile does, and
  Settings shows the same `No disk attached` line.
- **The server**: `GET_PALETTE` (R, 4 bytes: 0 the palette index) and `SET_PALETTE` (W, 4 bytes;
  root only, `EPERM` and `EINVAL` as `SET_PROFILE`) join `docs/windows.md`; the index is
  `theme + 2 * high_contrast`, so the accessibility brief's one mechanism carries both. On
  `SET_PALETTE` the shell repaints the whole screen in the same frame and every client gets the
  new event **`PaletteChanged`, kind 16, `a` the index**; `application::run` sets the tree's
  palette and damages the whole window, so the next frame is in the new theme (one full damage
  per window, well inside the frame budget). A window created after the change asks
  `GET_PALETTE` first. The Windows and macOS layers map their system colours from the palette
  too (`windows-gui-v0.3.md`'s `COLOR_*` table gets a light column in its own PR).
- **Settings > Appearance**: a new row **`Theme`** at the pane's row 2 (`y 272`, label at
  (16, 284) in `text`; rows 0 and 1 stay `Primary modifier` and `Window controls`; `Scale` moved
  to Display): a horizontal radio group `Dark` `Light` right-aligned ending at x 448 (two
  radios: 24 + 22 + 24 + 24 + 23 = 117 wide, from x 331, y 272, 28 tall), the saved value
  selected. `Chosen(theme_radios, i)` saves `appearance.theme` and sends `SET_PALETTE`; the
  pane itself repaints in the new palette in the same frame as every other window (no spinner,
  no dialog: instant and silent, as the profile). Search words: `theme`, `dark`, `light`, `dark
  mode`, `light mode`, `appearance`, `colours`, `colors` (Windows: `Choose your mode`; macOS:
  `Appearance`). The no-disk line at y 316 stays where it is.
- **CLI and MCP (D22)**: `settings get appearance.theme` prints `dark` or `light`; `settings
  set appearance.theme light` (and `--dry-run`: `would set appearance.theme from dark to
  light`); another value answers `invalid value 'x' (dark or light)`; `settings list` gains the
  row (`settable: true`). The MCP tools `settings_get`, `settings_set` and `settings_list`
  take the key with no new tool. One code path: the radios, the command and the tool call the
  same `Change::Theme(Theme)`.
- **High contrast** composes: `accessibility.high_contrast=true` with `appearance.theme=light`
  is `LIGHT_HIGH_CONTRAST`. The automatic "follow the time of day" switch is later (section 14).

## 9. Focus, pointer and the tree

- The focusable list gains the menu button, the table and the text area (Tab stops in
  declaration order; disabled ones skipped). The menu bar is **not** a Tab stop (F10 and Alt
  reach it, as on Windows and GNOME); a tooltip is never focusable.
- `Kind::is_focusable` and `is_interactive` include the three; a tooltip is neither. Parts:
  a menu bar's titles are parts `0 .. count` and the open menu's entries `16 .. 16 + entries`
  (`MAX_ENTRIES` 16 offsets them; a bar's element ids stay under `PER_WIDGET`); a menu button's
  entries are parts `0 .. entries`; a table's columns are parts `0 .. 8` and its shown rows
  `8 + r` for the shown row `r` (at most 91 shown rows get element ids: a 2160 px tall body
  shows 88); its handles are `HANDLE_BASE + c` (private), its thumbs `THUMB` and `THUMB_X`; a
  text area has no parts.
- The hit test checks, in order: the drag ghost (never hit), an open popup or menu (on top),
  then widgets as before; a press anywhere else while a menu is open closes it and is consumed.
- `Drag` becomes an enum: `Thumb { axis, grab, start_offset }`, `Slider { grab_x }`, `Column {
  table, column, start_width, grab_x }`, `Pick { source, part, from, started: bool }`. One drag
  at a time; a second button press during one cancels it.
- The timer (`tick`, `next_deadline`) serves the caret's blink, the tooltip's delay and
  touch timeout, the long press, and drag auto-scroll; `next_deadline` returns the earliest.
- `application::run` answers `MenuChosen` with `tree.menu_chosen`, `PaletteChanged` with
  `set_palette` and a full repaint, sends `SET_MENU` for a menu bar in the macOS profile, and
  passes `Invoked`, `Sorted`, `ColumnResized`, `DragStarted`, `Dropped` and `DragCancelled` to
  the program's `respond` unchanged.

## 10. What an agent sees (D161, `wm show`)

Seven roles join `eggos_element_tree::Role` and docs/windows.md's list: **15 `tooltip`**, **16
`menubar`**, **17 `menuitem`**, **18 `table`**, **19 `row`**, **20 `columnheader`**, **21
`textarea`** (the names of UIA and ARIA, so an agent trained on either recognises them).

- A **tooltip**: role `tooltip`, parent its owner, name its text (`Save`), value its shortcut
  (`Mod+S`), bounds where it shows or would show, **listed whether or not it is showing** (an
  agent reads what a button is for without hovering). Actions: none (`NotSupported`).
- A **menu bar**: role `menubar`, name `Menu` (the window's one bar), value the open title or
  empty, flag `expanded` while a menu is open; below it one `menuitem` per title (name the
  title without `&`, flag `expanded` when open), and below each title its entries as
  `menuitem`s (name the label, value the key label, `disabled` when disabled; separators are
  not listed), listed whether or not the menu is open. Actions: `invoke` on an entry chooses
  it (`Invoked`, the menu closed if open; `Disabled` for a disabled one); `invoke` on a title
  opens or closes its menu; `set-text` `NotSupported`. In the macOS profile the same elements
  are listed (the bar is in the top bar, but the window still owns it); `wm menu` (D147) also
  reads it through `GET_MENU`, and `wm menu --choose` arrives as `MenuChosen`.
- A **menu button**: role `button` named by its text with flag `expanded` while open, and its
  entries below it as `menuitem`s; `invoke` on the button opens or closes, on an entry chooses.
- A **table**: role `table`, name its `name` (`Files`), value `row <selected + 1> of <rows>`
  (`row 3 of 1000`; `0 rows` when empty), flags `focusable`, `focused`, `disabled`. Below it
  one `columnheader` per column (name the title, value `ascending` or `descending` for the
  sorted column and empty otherwise, flag `selected` on the sorted column) and one `row` per
  **shown** row (name the first column's cell, value every cell joined by tabs, U+0009, in
  column order; flag `selected`). Actions: `invoke` on a sortable header sorts as a click does
  (`Sorted`); `set-text "<width>"` on a header resizes the column (`ColumnResized`; a text that
  is not an integer `NotSupported`; clamped to `min_width`); `select` on a row selects it
  (`Chosen`), `invoke` opens it (`Opened`); **`set-text "<n>"` on the table selects row `n`
  (from 1) and scrolls it into view**, so an agent reaches any of a thousand rows without
  paging (`NotSupported` for a text that is not an integer, `Done` with `Nothing` for one out of
  range); `focus` as every focusable. Rows that are not shown are not listed: the element tree
  is at most 64 KiB, and a row of 64 bytes times a thousand would not fit.
- A **text area**: role `textarea`, name its placeholder, value its text when it is at most 255
  bytes (the element's limit), else its first bytes to a character boundary within 252 and
  `...`; flags as the field. Actions: `set-text` replaces the text (at most the 232 bytes a
  query carries: a longer text comes through the program's own command, as the Text Editor's
  `edit` does); `focus`. The selection is still not exposed (later).
- A **scroll view** stays a `group`; a **drag** has no element (an agent moves a row with the
  program's command, or `wm drag`, D162, which drives the same threshold and drop).

## 11. Device classes (D23)

- **Phone** (touch, a 360 px wide maximised window): every row is 40 tall (`TABLE_ROW_HEIGHT`
  and the list's row become 40, the header 40, the menu bar 40 with titles at +12 in a 40 px
  row), hit targets at least 40x40; a menu opens as a **bottom sheet** (`desktop-v0.3-gui.md`
  section 12: full width, anchored to the bottom, items 48 tall, a 1 px `menu.border` top edge,
  Esc or a tap outside closes) instead of a dropped-down list, and the menu bar's titles stay
  in the window; a column handle is 24 wide; tooltips and drags start with a long-press (2.4,
  7.2); the sideways wheel is a horizontal pan (the server sends `SidewaysWheel` from a
  horizontal swipe); the text area shows the caret's selection handles later (section 14).
  The toolkit learns the class from `GET_WORK_AREA`'s size and scale as the shell does, in one
  function (`widget::form_factor()`), so no widget tests the screen size itself.
- **TV** (D-pad, scale 2): the ring is always drawn (the window is always "keyboard focused");
  Up/Down/Left/Right move the focus between widgets (section 3's Tab order in two dimensions:
  the nearest widget in that direction), the remote's `Menu` key (`key::MENU`) arms the menu
  bar, `Back` is Esc; a table's Left/Right scroll columns, Up/Down rows; no drag, no handles
  (columns keep their widths); a tooltip shows for the focused widget after 600 ms (2.3).
- **Wearable** (a round or 200 px screen): no menu bar (a program offers a sheet of actions), no
  table (a list), no tooltips, no drag, no horizontal scrolling (every view is one column); the
  text area works (dictation later). A program that uses `Kind::Table` on a wearable gets a
  list of its first column (the tree draws it so; nothing fails).
- **2x**: every number here doubles: a 48 px tooltip, 56 px bars and headers, 48 px rows, 2 px
  separators, 4x4 chevron squares, a 14 px handle, 12 px drag threshold, 40 px line pitch, the
  fonts pixel-doubled. The light palette is the same at every scale.

## 12. Damage, performance and the two benchmarks

- **Damage**: a tooltip's show and hide damage its rect; a menu's open and close the title and
  the menu's rect, a highlight move the two entries; a table selection change the two rows, a
  sort the header, a resize drag the header and the rows right of the handle (one rect: from
  the handle to the table's right edge), a scroll the body (the header too when `offset_x`
  changes); a text area edit the lines from the edited line to the last visible one (a wrap
  can change every line below), a caret move the old and new caret lines, a selection change
  the lines it spans; a drag move the ghost's old and new rects and the old and new highlight;
  a palette change everything. The popup, menu and ghost rects are damage like any other: the
  widgets under them repaint and they are drawn over them last.
- **No allocation per frame, in layout or paint**: the tree is a fixed array; the table lays
  out only the rows in its body (`first_row = offset_y / 24`, `shown = body_height / 24 + 2`)
  and asks `cell` only for them, never for the thousand; the column x's are one pass over at
  most 8 columns; the text area's wrap runs on edit, resize and scale, never on paint, and
  paint walks the line index from `scroll / 20`; the tooltip's width is measured once when it
  shows; sorting is the program's, outside the frame. Layout cost per frame is in the visible
  rows and the widget count, not in the data.
- **`toolkit_table_layout_1000_rows`** (in the kernel's benchmark harness, `kernel/src/bench.rs`,
  which links `eggos-toolkit` as it links the graphics engine for `gfx_terminal_line`): a tree
  with one `Table` of 1000 rows and 4 columns at 400x300, 1x, after a one-row scroll: one
  `layout` and one `paint` of the table into a 400x300 buffer, instructions per frame, per
  architecture. A host test asserts the shape: the same frame with 10 000 rows runs within 5%
  of the 1000-row count (the cost does not scale with rows).
- **`text_area_wrap_64k`**: the wrap pass of 5.3 over a 64 KiB text of ~10 000 words (the
  pangram repeated, with a `\n` every 400 bytes) into a 384 px wide area: instructions per full
  wrap. Linear in bytes by construction (one pass, one advance lookup per character); the first
  run's numbers become the baseline lines (D122).

## 13. Acceptance (host tests, scale 1x, dark palette unless said; window content 480x360;
"press" and "release" are the left button; `b`, `m`, `t`, `a`, `v`, `l` are widget ids)

Fixture A: a button `b` at (16, 60), 96x28, text `Save`, with `Kind::tooltip("Save", "Mod+S")`
as its child; a second button `c` at (120, 60), 96x28, with `Kind::tooltip("Open", "")`.
Tooltip of `b`: 84 wide (8 + 24 + 12 + 32 + 8), rect (22, 92, 84, 24) (centred on 64, below
88 + 4); of `c`: 40 wide, rect (148, 92, 40, 24).

1. Move to (60, 70) at t=0 → `Nothing`, no tooltip; `tick(599)` → nothing; `tick(600)` → shown:
   pixel (22, 100) `menu.border`, (30, 100) `menu.bg`, `Save` from x 30 cell top 96 in `text`,
   `Mod+S` from x 66 in `text.dim`; `describe` lists `tooltip` named `Save` valued `Mod+S` under
   `b` before and after it shows, bounds (22, 92, 84, 24).
2. Shown; move to (70, 70) → still shown (moving within the owner); move to (60, 100) (into the
   tooltip's own rect, outside the owner) → hidden (the tooltip is never an owner); move to
   (160, 70) at t=700 → `c`'s tooltip shows at once (hop within 300 ms), rect (148, 92, 40, 24);
   `PointerLeave` → hidden; move to (160, 70) at t=2000 → not shown until `tick(2600)`.
3. Shown on `b`; press (60, 70) → hidden, `Nothing`; release → `Activated(b)`; `tick(5000)`
   with the pointer still there → not shown (spent); move to (160, 70) and back to (60, 70) at
   t=6000 → `tick(6600)` shows it again.
4. Shown on `b`; key `A` down → hidden; `tick` later → not shown until the pointer leaves.
   `set_focus(b)` by Tab with the window focused, pointer elsewhere → shown after 600 ms; key
   Tab → hidden, `c` focused, `c`'s shown 600 ms later.
5. A button at (16, 330) (bottom 358) → its tooltip goes above: rect top 330 − 4 − 24 = 302. A
   button at (0, 60) 40 wide → tooltip x clamped to 8. `set_enabled(b, false)` → hover still
   shows the tooltip; `set_shown(b, false)` while shown → hidden.
6. Phone class: press (60, 70) at t=0 without release; `tick(500)` → shown; `tick(2000)` →
   hidden; release at t=2100 → `Nothing` (spent on the tooltip). Wearable class: `tick(600)`
   after a hover → not shown; `describe` still lists it.

Fixture B: a menu bar `m` at (0, 0), `Fill(0)` x 28, menus `&File` [`&New` `Mod+N` id 1,
`&Open...` `Mod+O` id 2, `-`, `&Quit` `Mod+Q` id 3], `&Edit` [`&Undo` `Mod+Z` id 10 disabled,
`Cu&t` `Mod+X` id 11], `&View` [`&Wrap` `` id 20]; a field `f` at (16, 60) focused. Title
rects: `File` 0..39, `Edit` 39..80, `View` 80..126.

7. Paint: (10, 27) `menu.border`, (10, 20) `menu.bg`, `File` from x 12 cell top 6 in `text`;
   no underlines. Press (20, 14) → open, `Nothing`: title fill (10, 20) `menu.hover`; the menu
   at (0, 29): 160 wide (12 + 33 + 24 + 32 + 12 = 113 → 160), height 2 + 3 × 32 + 9 = 107;
   `New` at (12, 38), `Mod+N` right-aligned ending at x 148 in `text.dim`; `describe`: `menubar`
   `expanded`, `menuitem` `File` `expanded`, `menuitem` `New` valued `Mod+N`.
8. Release (20, 14) → still open; move (60, 14) → `Edit` open, `File` closed (no press); move
   (40, 100) (over `Cut`) → `Cut` `menu.hover`; press and release there → `Invoked(m, 11)`,
   closed, title fill back to `menu.bg`. Open `Edit`, hover `Undo` → no highlight; click it →
   `Nothing`, still open.
9. Open `File`, press (300, 200) (outside, over nothing) → closed, `Nothing`, consumed (a
   button placed at (280, 190) gets no `Activated` on the release). Open, `FocusOut` → closed.
10. Keys: F10 → armed: `File` title `menu.hover`, underlines under `F`, `E`, `V` (1 px `text`
    at row 14 of each cell, x 12..16 for `F`, its four ink columns), no menu; `f` keeps the caret hidden (the bar has
    the keyboard) and `describe` still flags `f` `focused`. Right → `Edit`; Right, Right → `File`
    (wrap); Down → `File` open, `New` highlighted; Down, Down → `Quit` (the separator skipped);
    Right → `Edit` open, `Cut` highlighted (`Undo` disabled, skipped); Enter → `Invoked(m, 11)`,
    disarmed, underlines gone, `f` typing again (key `a` → `Edited(f)`).
11. Left Alt down, up → armed; Alt down, `F` down (`Alt+F`) → `File` open at once; `q` (mnemonic
    of `&Quit`) → `Invoked(m, 3)`. Esc on an open menu → closed, still armed; Esc → disarmed. Alt
    down, `x` down, Alt up → not armed (a key came between). Armed, key `Z` → swallowed
    (`Nothing`, no `Unused`).
12. macOS profile: `application::run` with a fake server receives `SET_MENU` with the pool
    `MFile\nINew\tMod+N\nIOpen...\tMod+O\n-\nIQuit\tMod+Q\nMEdit\nDUndo\tMod+Z\nICut\tMod+X\nMView\nIWrap\t\n`;
    `m`'s rect is (0, 0, 480, 0) and the field laid out as if the bar were absent; `MenuChosen
    { menu: 1, entry: 1 }` → `Invoked(m, 11)`; `describe` lists the bar and its items. Back to
    the Windows profile → `SET_MENU` length 0 and the bar 28 tall again.
13. A menu button `k` at (16, 100), `Content` → (16 + 34 + 32 = 82 → 96) x 28, text `Format`,
    entries [`Bold` id 1, `Wrap` id 2]: chevron top-left at (90, 111); press (40, 114) → open,
    button `button.pressed`, menu at (16, 132), 160x66; Down, Down, Enter → `Invoked(k, 2)`,
    closed; Space → open; Esc → closed, `Nothing`; Tab → closed, focus on the next widget;
    `act(invoke, entry 1)` → `Invoked(k, 1)`, `Done`.

Fixture C: a table `t` at (16, 40), 400x220, name `Files`, columns `Name` (200, min 40,
Start, sortable), `Size` (80, End, sortable), `Modified` (160, Start, sortable), 1000 rows,
`cell` giving `Row <r>`, `<r> KB`, `2026-10-31`. Header y 40..68, body 68..260, rows 24 (8 fit);
content width 440 > 392 (the vertical bar shows: 1000 × 24 > 192), so the horizontal bar shows
in rows 252..260 and the vertical bar in x 408..416, rows 68..252.

14. Paint: (100, 67) `menu.border`; `Name` at (24, 46) in `text.dim`; separator (215, 50)
    `menu.border`, (215, 44) `menu.bg`; row 0 `Row 0` at (40, 72) `text`, `>` at (24, 72)
    `accent`, (100, 80) `selection`; row 1 at (40, 96); `0 KB` right-aligned ending at x 288;
    the vertical thumb 24 tall at (408, 68); the horizontal thumb `392 × 392 / 440 = 349` wide
    at (16, 252). `describe`: `table` `Files` valued `row 1 of 1000`, three `columnheader`s
    (no value), eight `row`s, `Row 0` `selected` valued `Row 0\t0 KB\t2026-10-31`.
15. Click (100, 100) → `Chosen(t, 1)`; Down → `Chosen(t, 2)`; End → `Chosen(t, 999)`, offset_y
    `1000 × 24 − 184 = 23816` (the body is 184 tall with the horizontal bar), `describe` lists
    rows 992..999; PageUp → `Chosen(t, 992)` (7 rows); Home → `Chosen(t, 0)`, offset 0. Double-
    click (100, 100) → `Chosen(t, 1)` then `Opened(t, 1)`; Enter → `Opened(t, 1)`.
16. Click (100, 50) (the `Name` header) → `Sorted(t, 0, Ascending)`, `Name` in `text`, the
    chevron's top row at y 51, right edge at x 208, pointing up: pixel (203, 51) `accent`,
    (203, 56) `menu.bg`, (199, 55) `accent`; click again → `Sorted(t, 0, Descending)`, (203, 56)
    `accent`, (203, 51) `menu.bg`; click (250, 50) (`Size`) → `Sorted(t, 1, Ascending)`, `Name`
    back to `text.dim`, the arrow at `Size`'s left (x 224..234). `set_sort(t, None)` → no arrow.
    `describe`: the sorted `columnheader` `selected` valued `ascending`.
17. Press (215, 50) (the handle between `Name` and `Size`, zone 212..219): pointer `ResizeEW`
    on hover at (214, 50); move (255, 50) → `Name` 240 wide live: separator at x 255, `Size`
    cells right-aligned ending at 328, the horizontal thumb shorter (content 480), `Nothing`;
    release → `ColumnResized(t, 0, 240)`. Press (255, 50), move (0, 50), release → width 40
    (the minimum), `ColumnResized(t, 0, 40)`. Press (55, 50), move (100, 50), Esc → width back
    to 40, `Nothing`, the release that follows `Nothing`. `act(set-text "120", header 0)` →
    width 120, `ColumnResized(t, 0, 120)`, `Done`; `act(set-text "x", header 0)` →
    `NotSupported`.
18. Wheel at (100, 150), −1 notch → offset_y 72 (3 rows); Shift + wheel −1 → offset_x 48
    (clamped to 440 − 392), header and rows shifted: `Name` at x −24 clipped, `Size` ends at
    240; `SidewaysWheel` +1 at (100, 150) → offset_x stays 48 (at the end); −1 → 0. Right →
    offset_x 48; Left → 0. Wheel over the header (100, 50) scrolls the body.
19. `set_rows(t, 0)` → `No items` at (40, 72) `text.dim`, `describe` value `0 rows`, no `row`s;
    `set_rows(t, 3)` → `selected` 0. `act(set-text "500", table)` → `Chosen(t, 499)`, offset
    such that row 499 is visible; `act(select, row 7)` → `Chosen(t, 7)`; `act(invoke, row 7)` →
    `Opened(t, 7)`. `set_enabled(t, false)` → press does nothing, `Row 0` in `text.dim`, no `>`.
20. Benchmark shape test: `layout` + `paint` of the 1000-row table and of a 10 000-row table
    after one `scroll_to(t, 24)` call `cell` exactly for the rows that intersect the body:
    rows 1..9, 8 × 3 = 24 calls each (the partial eighth row included, no row beyond the body),
    and run within 5% of each other's instruction count (counted with the host harness's step
    counter where available, else asserted on `cell` calls).

Fixture D: a text area `a` at (16, 40), 200x92 (inner x 24..200 = 176 wide; lines at cell tops
46, 66, 86, 106), store 0, placeholder `Describe it`, in a `WidgetTree<16, 1>`; focused by a
click at (100, 50). Egg Sans widths: `the quick brown fox jumps over the lazy dog` is 227;
`the quick brown fox` = 16 + 4 + 25 + 4 + 31 + 4 + 16 = 100, `+ jumps` 134, `+ over` 161,
`+ the` 181 > 176.

21. Empty: `Describe it` at (24, 46) `text.dim`, caret at x 24 rows 46..61. `set_text(a, "the
    quick brown fox jumps over the lazy dog")` → 2 visual lines: `the quick brown fox jumps
    over ` (bytes 0..31, its space at the end, width 161 + 4 = 165 drawn, ending at x 189) and
    `the lazy dog` (bytes 31..43, 62 wide) at cell top 66; `lines` 2; no bar (2 × 20 + 12 = 52 <
    92); caret at the end, x 24 + 62 = 86 on line 2.
22. Home → caret 31 (the line's start, drawn at x 24 line 2); Up → caret at the nearest boundary
    to x 24 on line 1 = 0; End → caret 30 (before the trailing space; x 185); Down → line 2,
    nearest boundary to x 185 → 43 (the end, x 86); Up → `wanted_x` 185 kept → 30 again; Left
    → 29, `wanted_x` dropped. Mod+End → 43; Mod+Home → 0.
23. Caret 43: Enter → text ends `dog\n`, 3 lines, caret 44 drawn at (24, 86), `Edited(a)`; type
    `a` → `Edited(a)`, `a` at (24, 86); Enter, Enter, Enter → 6 lines (content 132 > 92): the
    bar appears in x 207..215 rows 41..131 and the wrap width is unchanged (176, reserved
    before), scroll 40 so the caret's line (cell top 146 − 40 = 106) is the last visible;
    wheel +1 → scroll 0 (3 lines, clamped), the caret keeps its byte; PageUp → caret on the first
    line.
24. `set_text(a, "supercalifragilisticexpialidocioussupercalifragilistic again")` (the first
    word 248 wide > 176, no break opportunity in it) → line 1 is its first 37 characters
    (`...docioussup`, exactly 176 wide, right edge at x 200), line 2 `ercalifragilistic again`
    (106 wide); no glyph is cut (every drawn glyph's right edge ≤ 200). A text of 70 000 bytes
    through `set_text` → length ≤ 65 536 at a character boundary, `Edited` not sent (`set_text`
    is the program's).
25. Text as in case 21. Click (60, 50) → caret 7 (line 1's boundaries near x 60 are byte 6 at
    x 56 and byte 7, after the 2 px `i`, at x 58; 58 is nearer), anchor 7; Shift+click (40, 70)
    → caret 34 (line 2: `the` ends at x 40), anchor 7; fill: line 1 from x 58 to 189 (the
    line's last glyph's end), line 2 from 24 to 40, each the cell's 16 rows; Mod+C →
    `Copied(a)`, `copied()` = `ick brown fox jumps over th` (bytes 7..34; a `\n` inside a
    selection is copied too); Mod+X → `Cut(a)`; Mod+V → `PasteWanted(a)`; `paste(a,
    "a\tb\r\nc", now)` → `a`, a tab (16 px), `b`, `\n` (the `\r` dropped), `c`, `Edited(a)`.
    Triple-click on line 2 → the paragraph (bytes 0..43, there being no `\n`). Drag from
    (60, 50) to (100, 130) (below the inner rect) with the button held → each further move
    extends the caret one line down and scrolls.
26. Tab → focus leaves (no tab inserted); Esc → `Unused { code: ESC, .. }`, text unchanged; a
    default button `d`: Enter → `Edited(a)` (a new line), Mod+Enter → `Activated(d)`.
    `describe`: `textarea` named `Describe it` valued by the text; with 300 bytes of text the
    value is 252 bytes at a boundary plus `...`; `act(set-text "hello")` → text `hello`,
    `Edited(a)`, `Done`. `WidgetTree<16>` (A = 0) with a `Kind::text_area(0, ..)` → `add`
    returns `None` (no store).

Fixture E: a scroll view `v` at (0, 0), 320x200, content 600 wide (a 600x40 panel child) and
400 tall; a list `l` inside it at (0, 60), 320x120, 32 rows of 24.

27. Both bars: vertical in x 312..320 rows 0..192, horizontal in rows 192..200 x 0..312; thumb
    lengths `192 × 192 / 400 = 92` and `312 × 312 / 600 = 162`; the corner (316, 196) is the
    view's surface. `SidewaysWheel` +1 at (100, 20) → `offset_x` 72, the panel child at x −72;
    Shift + `Wheel` −1 (down) → 144; Shift + `Wheel` +1 → 72; +4 → 0 (clamped); `SidewaysWheel`
    +10 → 288 (600 − 312: the range reaches past the vertical bar's strip, so the content's
    last column can be read). Drag the horizontal thumb from (100, 196) to (200, 196) →
    `offset_x` `100 × 288 / 150 = 192`; click the track at (300, 196) → +288 (`track − 24`) →
    288 (clamped).
28. `SidewaysWheel` +1 at (100, 100) (over the list, whose content is only tall) → the view
    scrolls (`offset_x` 72): the list is not horizontally scrollable, the view is; `Wheel` −1 at
    (100, 100) → the list scrolls 72, the view's `offset` unchanged. The same view with content
    300 wide → no horizontal bar, `SidewaysWheel` → `Nothing`, nothing moves.

Fixture F: a list `l` at (16, 40), 200x120, items `alpha` `beta` `gamma` `delta` `epsilon`
(rows 40..64, 64..88, ...), `set_drag_source(l, true)`, `set_drop_target(l, Between)`; a
list `p` at (240, 40), 200x120, items `Places` `Trash`, `set_drop_target(p, Onto)`.

29. Press (100, 52) on `alpha`, move (103, 54) → `Nothing`, no drag (5 < 6), `alpha` hovered,
    not selected; release → `Chosen(l, 0)` (the click). Press (100, 76) on `beta`, move (100, 83)
    (7 px) → `DragStarted(l, 1)`, pointer `Hand`, the ghost (112, 95, 8 + 22 + 8 = 38, 24) with
    `beta` at (120, 99) in `text`, `menu.bg` fill, `menu.border` outline; selection unchanged.
30. Move (100, 130) (over `delta`'s bottom half: 112..136, pointer at 130 > 124) → the line at
    y 136..138, x 24..208 `accent`, `Nothing`; move (100, 116) (top half) → line at 112..114;
    move (100, 80) (around the source) → no line; release at (100, 116) → `Dropped { source: l,
    part: 1, target: l, at: 3 }` (before `delta`), ghost and line gone, pointer `Arrow`.
31. Press (100, 76), move (100, 90) → `DragStarted`; move (300, 76) over `Places` → `Places`'
    row `menu.hover` with a 2 px `accent` outline at inset 0, rect (240, 40, 200, 24); release →
    `Dropped { source: l, part: 1, target: p, at: 0 }`. Move over (300, 110) (past `p`'s rows) →
    no highlight; release there → `DragCancelled(l)`. Drag started, Esc → `DragCancelled(l)`;
    `FocusOut` → `DragCancelled(l)`; `DragStarted` answered by `cancel_drag()` → the ghost never
    draws, the release is `Nothing`.
32. A list of 32 rows as a `Between` target, drag held at (100, 50) (within 24 of its top) with
    `offset` 240 → `tick` every 100 ms scrolls up one row (24) until 0. Phone class: press (100,
    76) without moving; `tick(500)` → `DragStarted(l, 1)`, ghost at (112, 40) (offset (12, −36)).
33. `wm drag` (D162: a press, 8 moves and the release through `SEND_INPUT`) from (100, 76) to
    (100, 116) → `DragStarted` on the first move past 6 px and `Dropped { .., at: 3 }` on the
    release, the same as the person's.

Fixture G: the light palette (`set_palette(&LIGHT)`), a tree with a label, a button, a field,
a list with a selected row, a dropdown, a slider and a tooltip shown.

34. Every pixel is compared against `LIGHT.get(token)`: the label's text `text`, the surface
    `menu.bg`, the button fill `button.bg` with `button.border`, the field `field.bg`, the
    selected row `selection` with the `>` in `accent`, the ring `focus.ring`, the slider's
    filled track `accent` and its thumb `button.primary`, the tooltip `menu.bg` with
    `menu.border`; the same frame in `DARK` equals the frame the tests of v0.3 and v0.4 pin
    (the refactor to `Token` changes no dark pixel). `set_palette(&LIGHT)` damages the whole
    window. A computed check over every pair of section 8.2 asserts the ratios stated there
    (the test computes WCAG contrast from the palette's values, so the table and the code
    cannot drift).
35. `application::run` with a fake server: `PaletteChanged { a: 1 }` → `set_palette(&LIGHT)`
    and a full repaint in the next frame; `{ a: 3 }` → `LIGHT_HIGH_CONTRAST`; a window created
    after the change asks `GET_PALETTE` and paints its first frame in the light palette.
36. Settings: `settings get appearance.theme` → `dark`; `settings set appearance.theme light`
    → the fake machine's `SET_PALETTE` called with 1 and the settings file holds
    `appearance.theme=light`; `settings set appearance.theme blue` → status 1, `settings:
    appearance.theme: invalid value 'blue' (dark or light)`; `--dry-run` → `would set
    appearance.theme from dark to light`; `settings list --json` has `{"key":"appearance.theme",
    "value":"light","settable":true}`; the MCP tool `settings_set` with `appearance.theme`
    `light` answers as the command does. The Appearance pane: `Theme` at (16, 284), the radios
    `Dark` (331, 272, 46, 28) and `Light` (401, 272, 47, 28), `Light` selected after the
    command; `Chosen(radios, 0)` → `SET_PALETTE` 0 and the file `appearance.theme=dark`; the
    search `dark mode` finds Appearance.

Pixel cases compare the painted framebuffer against the stated tokens at the stated points;
response cases drive `handle` with `Input` records and compare the returned `Response`;
element cases read `describe` through `eggos_element_tree::Query`.

## 14. Later slices

Submenus and checkable or radio menu items; a menu bar's overflow (`...`) when the window is
narrower than its titles; fit-to-content on a handle's double-click, column reordering by
drag, multi-selection and cell focus in tables, a tree table; the text area's selection
handles on touch, spell marks, a line-number gutter (the Text Editor's), word moves (Mod+arrows)
in the field and the area, undo (the Text Editor has it, the toolkit not yet); drag between
windows and to the desktop, a drag's data types, a copy-vs-move modifier; a dark terminal in a
light desktop (`term.*` by their own setting), the Windows and macOS layers' light system
colours, the light high-contrast ANSI table, following the time of day or the display's
ambient light; the selection in the element tree; and, still from v0.4 section 9: an editable
combo box, a vertical and a range slider, a `Spinner`, inertia, focus rings hidden in "mouse
mode", IME and dead keys in the field, key repeat in the toolkit, disabled rows in a dropdown,
a middle ellipsis, the selection and copy in the terminal.
