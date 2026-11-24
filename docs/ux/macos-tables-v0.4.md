# EggOS v0.4 macOS layer: NSTableView sorting, per-row heights and view-based tables: design brief

Design by Claude Fable (D16) for the next slice of the macOS layer's `NSTableView`
(`userland/macos/src/appkit/table_view.rs`). It extends `macos-gui-v0.3.md` section 11 and
**everything there stays**: the header's 28 rows, the 24 px row, the mark column with the
`>`, the column resize handle, alternating rows, the grid, the states of 11.2 (row hover
included, D726), the selection and keys of 11.3, the palettes of section 20. The table is
still drawn as the toolkit's table (`toolkit-v0.5.md` section 4), so every look below is one
the toolkit already draws, in the four palettes; no new token, no new glyph. Numbers are px
at 1x in window content coordinates (`cx`, `cy` the clip view's origin, `x_c` and `w_c` a
column's left edge and width, `y_r` a row's top, `oy` the vertical offset) and double at 2x;
no motion is added.

## 0. What a person gets, and how that is checked

- A click on a column's title sorts the table by it, a second click reverses it; the sorted
  column shows the arrow every EggOS table shows, and its title brightens.
- Rows as tall or as short as the app asks (`rowHeight`, `tableView:heightOfRow:`), the
  text, the `>` and the ring sitting right in each.
- Apps that build their cells from views (`NSTableCellView` with a text field and an image)
  draw inside the same rows, with the same selection, hover and grid under them, and the
  same `>`.
- Host tests pin the pixels, the AppKit messages and the element tree (section 7); the
  `mac-views.app` steps of 11.4 and 18 gain a sorted column.

## 1. Sorting

### 1.1 What is sortable

A column is **sortable** when its `sortDescriptorPrototype` is set (`NSSortDescriptor` with
`key`, `ascending`, and `reversedSortDescriptor`; `sortDescriptorWithKey:ascending:` and
`...selector:` make one; the selector and the comparator are stored, never run by the layer:
the data source sorts). A column without one keeps 11.1's look: no hover, no pressed look, no
arrow, a click changes no descriptor. `tableView.sortDescriptors` is the table's array; the
indicator follows its **first** descriptor whose `key` equals a column prototype's `key`. The
table never reorders rows itself (AppKit does not either): the data source gets
`tableView:sortDescriptorsDidChange:` and reorders what `objectValueForTableColumn:row:`
reads from, then calls `reloadData`.

### 1.2 The header cell's states

| State | Fill (`cy .. cy + 26`, `x_c .. x_{c+1} - 2`: the bottom line and the separators stay) | Title |
|---|---|---|
| normal | `menu.bg` | `text.dim` |
| hover, sortable only (pointer in the cell, not on a handle, no button down) | `menu.hover` | `text.dim` |
| pressed, sortable only (left button down in the cell; pointer inside) | `button.pressed` | `text.dim` |
| sorted (any of the above) | as the row above | `text`, arrow `accent` |
| whole table disabled | `menu.bg` | `text.dim`, the arrow stays `accent` |

The pointer rules are the toolkit's: a press captures until the release; pressed while the
pointer is inside the cell, hover-less while outside; a release inside is the click, a
release elsewhere nothing. The pointer stays `Arrow`. A wheel over the header still scrolls
the body. Hover is not drawn on the TV (section 4).

### 1.3 The indicator

Filled pixels, not a glyph: **the toolkit's chevron** (`toolkit-v0.4.md` 3.2, as the toolkit's
table draws it in `toolkit-v0.5.md` 4.2: 10 wide, 6 tall), in `accent`, its right edge at
`x_c + w_c - 8` and its top at `cy + 11` (rows `cy + 11 .. cy + 16`, inside the separator's
rows 6..21), pointing **up for ascending** and down for descending. The sorted column's
title is end-ellipsised at `x_c + w_c - 20` (`TABLE_ARROW_ROOM`) instead of `- 8`. In a
right-aligned column (11.1) the chevron sits at the column's left, its left edge at `x_c + 8`,
and the title ends at `- 8` as usual. One column shows an indicator at a time: when another
column becomes sorted, the old one's arrow goes and its title returns to `text.dim` and its
`- 8` room, in the same frame. `setIndicatorImage:inTableColumn:` and
`indicatorImageInTableColumn:` store and answer the image but the chevron is what is drawn
(listed in section "Later"); `highlightedTableColumn` answers the sorted column and
`setHighlightedTableColumn:` is stored and ignored (listed).

### 1.4 The click and AppKit's messages

A **click** is a press and a release in the same header cell, not on a handle (11.1's 7 px
zones win). On the press the delegate gets `tableView:mouseDownInHeaderOfTableColumn:` (every
column). On the release, for a sortable column:

- the column is not the first in `sortDescriptors`: `sortDescriptors` becomes
  `@[prototype]` (so the first click sorts the way the prototype says: `ascending` NO sorts
  descending first, arrow down);
- it is the first: its descriptor is replaced by its `reversedSortDescriptor`.

Then, in this order: the header is redrawn (the arrow is right in the same frame, before the
rows change), the data source gets `tableView:sortDescriptorsDidChange:` with the **old**
array, the delegate gets `tableView:didClickTableColumn:` (every column, sortable or not).
`selectedRow` keeps its index through the data source's `reloadData` (11.3's rule: the
selection is an index, as AppKit's), so an app that wants the same object selected re-selects
it. `setSortDescriptors:` by the app sets the indicator the same way and sends
`sortDescriptorsDidChange:` too (AppKit does); an array with no column's key, or empty, shows
no arrow. A sideways move during the press changes nothing in this slice (column reordering
is later: the release in the cell is still the click, outside it nothing).

### 1.5 Keyboard, agents

No key sorts in the table (the toolkit's rule, 4.4: on macOS a table sorts by its header or
the app's `View > Sort By` menu, which an AppKit app provides itself). An agent sorts with
`invoke` on the column's `columnheader` element (`toolkit-v0.5.md` section 10: value
`ascending` or `descending` on the sorted column, empty otherwise, flag `selected`), which
does what a click does, messages included; `set-text "<width>"` on it resizes, as before.

### 1.6 High contrast

Nothing branches: the sorted title is `text` (white on black, black on white), the chevron
`accent` on `menu.bg` (`#FFD24D` on `#000000` 14.6:1, `#7A4E00` on `#FFFFFF` 7.2:1; dark
8.4:1, light 4.7:1), hover `menu.hover` (`#262626` / `#E6E6E6`) and pressed `button.pressed`
(`#4D4D4D` / `#D9D9D9`) with the title `text.dim` (4.6:1 and 6.8:1, momentary), the values
section 20 already lists. The arrow and the brighter title are the non-colour cues; no stroke
widens, nothing moves (`first-run-accessibility-v0.3.md` 2.4).

## 2. Per-row heights

- **Sources.** `rowHeight` (default 24, the toolkit's `TABLE_ROW_HEIGHT` through
  `form_factor()`, section 19; `setRowHeight:` re-lays every row) and, when the delegate
  responds to `tableView:heightOfRow:`, that answer per row, asked at `reloadData`, at
  `noteNumberOfRowsChanged`, at the first draw, and for the named rows at
  `noteHeightOfRowsWithIndexesChanged:` (an `NSIndexSet`); never at a paint. Answers are
  cached as `u16` per row with a running sum, so a hit test or `rectOfRow:` is a binary
  search; the cache is bounded: past `VARIABLE_HEIGHT_ROWS` (65,536) rows the delegate is not
  asked and `rowHeight` applies to every row (128 KiB at most, listed in "Later").
- **Clamp.** `h_r = clamp(answer, TABLE_ROW_MIN, TABLE_ROW_MAX)` with `TABLE_ROW_MAX` 1024
  and `TABLE_ROW_MIN` from `form_factor()`: **16** on the desktop and the TV (one text line
  always shows), **40** on the phone, tablet and wearable (every hit target at least 40, section
  19). `intercellSpacing` is still stored and ignored (listed).
- **Geometry.** `y_0 = body_top - oy`, `y_{r+1} = y_r + h_r`; the table's frame height is
  `28 + sum(h_r)` (`sum(h_r)` without a header). The row's fills (surface, alternate, hover,
  `selection`) span rows `y_r .. y_r + h_r - 1` and the table's full width. **Text cell top is
  `y_r + (h_r - 16) / 2`** (floor: a 17-row row puts the 16-row cell at `+ 0`, the toolkit's
  `(h - 16) / 2` bias), at `x_c + 8` (`x_0 + 24` in the first column); the `>` at `x_0 + 8` on
  the same cell top; 24 gives `+ 4` as today, 40 gives `+ 12` as section 19's phone rows. The
  **focus ring** is 2 px `focus.ring` at inset 0 of `(cx, y_r, table width less the scrollers,
  h_r)`, `INSET_R4` corners as 11.2; in rows under 20 its two rows cover the cell's first and
  last rows (in a 16-row row, the glyphs' rows 0, 1, 14 and 15): accepted, the ring is the
  focused table's momentary state and the x-height rows read; from 20 up nothing overlaps.
- **Grid.** The horizontal line on each row's last row `y_r + h_r - 1`, one more at the
  frame's end; the vertical lines through the body as 11.1; over the fills, under the text,
  the `>` and the ring.
- **Hit and scroll.** A press at `y` finds `r` by the sums; `rowAtPoint:` and `rectOfRow:`
  answer them; `scrollRowToVisible:` and the keys' "keep the selection fully visible" use the
  row's rect; a row taller than the body shows its top (`y_r` at `body_top`). Page Up and
  Page Down move the selection by as many rows as fully fit in the body at the current
  offset, less one. A wheel notch (72 px) lands the offset on the row top nearest to
  `oy ± 72`, at least one row away (so a 100-px row still scrolls).

## 3. View-based tables

- **When.** The delegate responds to `tableView:viewForTableColumn:row:`: the table is
  view-based for every cell (a delegate that answers nil for a cell draws that cell empty, as
  11.3 says today). The data source's count and `objectValueForTableColumn:row:` are still
  asked (the latter only when it is implemented; its answer becomes the cell view's
  `objectValue`, else nil).
- **Cost.** Views exist for the **visible rows only**: at each layout (first draw,
  `reloadData`, a scroll, a resize, `noteHeightOfRowsWithIndexesChanged:`) the table asks
  the delegate for the cells of the rows in view that have no view yet, and when a row leaves
  the view its cell views go to the table's reuse queue by `identifier`;
  `makeViewWithIdentifier:owner:` answers a queued view with that identifier (`prepareForReuse`
  sent) or nil, and the app creates one and sets `identifier` (nib-loaded views: later). At
  most `VISIBLE_CELL_VIEWS` (512) views are kept; a table of a million rows costs its visible
  ones, as 11.3's cell rule.
- **Placement.** The cell view's `frame` is the cell's rect in the table's coordinates
  (section 0's rule with `H` the table's height): width `w_c`, height `h_r`, left `x_c`; **in
  the first column** left `x_0 + 16` and width `w_0 - 16`: the 16 px mark column belongs to
  the table, no view covers it. The app's own `setFrame:` on a cell view is overwritten at
  each layout, as AppKit does. The cell view's subviews draw at their frames in the cell
  view's coordinates (one level deeper than 0.3's rule: document view, cell view, its
  subviews), clipped to the cell view.
- **Unframed parts.** An `NSTableCellView` whose `imageView` or `textField` has a zero frame
  (`[[NSTextField alloc] init]`) is laid out by the cell view itself at the toolkit's insets:
  the image view 16x16 at `x` 8, vertically centred (`scaling` `ProportionallyDown`, 12.2);
  the text field from `x` 8 (32 with an image view) to `w - 8`, 16 tall, vertically centred,
  so a first-column label lands at `x_0 + 24` exactly as a cell-based cell. A framed part
  keeps its frame.
- **Layers**, back to front: the row's fill (surface or alternate, `menu.hover`,
  `selection`), the grid lines, the cell views in column order (an `NSTableCellView` draws
  nothing itself; a label text field draws no background; `drawsBackground` YES fills its
  frame in `backgroundColor`; a custom `NSView` subclass's `drawRect:` runs clipped to its
  frame), then the `>` and the ring over everything. So hover and selection show through
  wherever a view leaves pixels alone.
- **Text colour on a selected row.** The cell view's `backgroundStyle` is
  `NSBackgroundStyleEmphasized` on the selected row and `NSBackgroundStyleNormal`
  otherwise, set (`setBackgroundStyle:`) on every selection change for the two rows; and
  on the selected row every semantic label colour (`labelColor`, `secondaryLabelColor`,
  `tertiaryLabelColor`, `quaternaryLabelColor`, `controlTextColor`, `textColor`,
  `headerTextColor`, `placeholderTextColor`) resolves to `text` for the row's views
  (AppKit's own rule, emphasised labels draw `alternateSelectedControlTextColor`, which is
  `text` here; `text.dim` on `selection` would be 2.6:1 in the dark palette). Fixed colours
  (`blackColor`, `colorWithRed:...`) stay, as everywhere. A disabled table draws every view's
  text `text.dim` and no `>`, as 11.2.
- **The `>`** is drawn on the selected row at `x_0 + 8` in `accent` whatever the views draw:
  the non-colour cue of 11.1 is the table's, not the app's.
- **Presses.** A press on an `NSButton` (push, checkbox, radio, toggle) inside a cell view
  goes to the button and does not select the row (AppKit's rule); any other press in the
  row, a label or an image included, is the row's (11.3: select, first responder, `action`,
  `doubleAction`). An editable `NSTextField` in a cell draws as section 3's bezeled field but
  a press on it is the row's in this slice (editing in place: later).
- **Agents.** A view-based table reads like a cell-based one: each `row`'s cell value is its
  cell view's `textField.stringValue` (empty when there is none); the views' own subviews are
  not listed (a button in a cell: later).

## 4. Device classes (D23)

Through `form_factor()`, no literal in the layer (section 19):

- **Phone and tablet** (touch): header 40, `TABLE_ROW_MIN` 40, text at `+ (h_r - 16) / 2`; a
  tap on a sortable title sorts (the hover look never shows); the chevron's top at `cy + 17`
  (centred in 40 as 11 is in 28); a drag that starts on the header pans the table.
- **TV** (D-pad, scale 2): no pointer, so the header has no hover, no pressed look and no
  focus (it is not a control); sorting is the app's menu or an agent's `invoke`; per-row
  heights double on the panel; the ring on the selected row as 19.
- **Wearable**: the table is a list of its first column (19); a view-based table lists the
  first column's `textField` text; rows `h_r` tall, at least 40; no sorting.
- **2x**: every number doubles, the chevron's squares pixel-double.

## 5. Palettes

No new token; the tokens this brief draws with and their four values are section 20's table:
`menu.hover` (header hover), `button.pressed` (header pressed), `text` (sorted title, cell
views' labels on a selected row), `accent` (the chevron, the `>`), `menu.border` (the grid at
every height), `focus.ring` (the ring at every height). `PaletteChanged` re-draws the header
and every cell view (`drawRect:` and `viewDidChangeEffectiveAppearance` as 20).

## 6. Worked example (11.4's table, `Name` 160 and `Size` 100, both left-aligned and sortable)

Header at (193, 17, 270, 28); `Name`'s cell fill is columns 193..351, rows 17..43 (the
separator at 352 and the bottom line at row 44 stay). A press at (250, 30): the fill is
`button.pressed` while the button is down. The release there makes `Name` sorted ascending
(`sortDescriptors` `@[name, ascending YES]`): the fill is `menu.hover` while the pointer
rests in the cell (hover is drawn on a sorted column too) and `menu.bg` once it leaves;
`Name` at (201, 23) in `text`, its ellipsis room ending at 333; the chevron in `accent` at
columns 335..344, rows 28..33, apex on row 28 (up). A second click: descending, apex on row
33. A click at (400, 30): `Size` sorted ascending, its chevron at columns 435..444, rows
28..33, `Size` at (361, 23) in `text`; `Name` back to `text.dim` with no chevron, in the
frame of the release. The data source's `sortDescriptorsDidChange:` gets the old array each
time; `selectedRow` stays 4 until the app's `reloadData` says otherwise. Were `Size`
right-aligned, its chevron would sit at columns 361..370 and its title end at 445.

With `heightOfRow:` answering 32 for even rows and 24 for odd ones: `y_0` 45 (rows 45..76),
`y_1` 77 (77..100), `y_2` 101 (101..132), `y_3` 133 (133..156), `y_4` 157; `Item 1` at (217,
53), `Item 2` at (217, 81), `Item 4` at (217, 137). A press at (320, 150) is row 3: the
selection fill rows 133..156, the ring at inset 0 of (193, 133, 262, 24), the `>` at (201,
137). The horizontal grid lines are rows 76, 100, 132, 156, 188, ...

## 7. Tests

Host tests (`table_view.rs`, `palette.rs`):

- sortable and not: the header's hover and pressed fills at (250, 30) for a column with a
  prototype, none without; a press on a handle (column 350) is a resize, not a click.
- the click sequence of section 6: `sortDescriptors` after one, two and three clicks (the
  third on `Size`), the messages in order (`mouseDownInHeaderOfTableColumn:` on the press,
  `sortDescriptorsDidChange:` with the old array then `didClickTableColumn:` on the release,
  `didClickTableColumn:` alone for a non-sortable column), `highlightedTableColumn`,
  `selectedRow` unchanged; a release outside the cell sending `didClick` not and changing
  nothing; `setSortDescriptors:` with `ascending NO` giving a down chevron and sending
  `sortDescriptorsDidChange:`; an empty array giving no chevron; the render hashed with
  `Name` sorted ascending and descending and `Size` sorted (pixels: the chevron's apex
  `accent`, (201, 23) `text`, the ellipsis end moved) in all four palettes.
- `invoke` on the `columnheader` sorting as the click; the element's value `ascending` then
  `descending`, flag `selected`.
- heights: `rowHeight` 32 re-placing every row; the 32/24 rows of section 6 (`rectOfRow:`,
  `rowAtPoint:` at (320, 150) is 3, the text tops, the grid rows, the ring rect); an answer
  of 8 clamped to 16 (desktop) with the text at `+ 0`; 2000 clamped to 1024;
  `noteHeightOfRowsWithIndexesChanged:` re-asking only the named rows (the delegate counts);
  Page Down with mixed heights; a wheel notch from offset 0 landing on a row top; a table of
  70,000 rows asking the delegate nothing and using `rowHeight`.
- view-based: the delegate asked for exactly the visible cells at offset 0 (11 rows x 2) and
  for the newly visible rows after three wheel notches, the scrolled-out views answered by
  `makeViewWithIdentifier:owner:` with `prepareForReuse` sent; the cell view frames
  ((193 + 16, ..) in the first column, `w_0 - 16` wide); an unframed `textField` placed so its
  text is at (217, 49) in row 0; `backgroundStyle` emphasised on the selected row and a
  `secondaryLabelColor` label there rendered `text`, normal elsewhere; a press on an
  `NSButton` in a cell reaching the button and leaving `selectedRow`; a press on the label
  selecting the row; the render hashed with hover, selection, grid and a view that draws
  `drawsBackground` YES (pixels: (300, 150) `selection` beside the view, the `>` at (201,
  145) `accent`, a grid pixel under a transparent view `menu.border`); `wm show` listing the
  cells' texts.
- QEMU (section 18's steps, aarch64 and x86_64): after step 6, a click on `Name`'s title and
  a screendump: the chevron and the brighter title; `wm show` reports `Name` `ascending`.

## Decided by design (the owner can overrule any of these)

1. The indicator is the toolkit's chevron in `accent`, placed as the toolkit's table places
   it, not AppKit's grey triangle image and not a font glyph: one table look on EggOS
   (11.2's reasoning), and `setIndicatorImage:` is stored, not drawn.
2. Hover is drawn on a sorted column's header too (the state table is the toolkit's).
3. A click's order: header redrawn, `sortDescriptorsDidChange:`, then `didClickTableColumn:`;
   `mouseDownInHeaderOfTableColumn:` and `didClickTableColumn:` go for every column.
4. Row heights are clamped to 16..1024 on the desktop (40 up on touch), text centred with the
   toolkit's floor; the ring may cover the glyphs' edge rows in rows under 20.
5. Variable heights are cached per row and bounded at 65,536 rows; past that `rowHeight`
   applies.
6. Cell views exist for the visible rows only, reused by identifier, at most 512 kept.
7. The first column's cell view starts 16 px in: the mark column is never the app's.
8. On a selected row the semantic label colours resolve to `text`; fixed colours stay.
9. A press on a button in a cell is the button's and selects nothing; every other press is
   the row's.
10. No key sorts; agents sort with `invoke` on the header.

## Later

`setIndicatorImage:` drawn, `setHighlightedTableColumn:`'s look, several sort descriptors
with several arrows, column reordering by drag (the toolkit's 4.1), `intercellSpacing`,
variable heights past 65,536 rows, `tableView:rowViewForRow:` and `NSTableRowView`, group
rows, `rowSizeStyle`, nib-loaded cell views, editing a cell's text field in place, buttons in
cells listed for agents, `NSOutlineView`.

## Decisions for the engineer

1. `TableColumn` gains `sort_prototype: Option<Id>` and `indicator_image: Id`; `TableFields`
   gains `sort_descriptors: [Id; 4]` (the first drawn), `header_hover: isize`,
   `header_pressed: isize` (`NO_INDEX` when none).
2. Header cell fill rect `(x_c, cy, w_c - 1, 27)`; chevron right edge `x_c + w_c - 8`, top
   `cy + 11`, up for `ascending`; title room `- 20` on the sorted column; right-aligned
   columns put the chevron at `x_c + 8`.
3. Click = press and release in one cell off the handles; descriptors as 1.4; messages in
   1.4's order; `setSortDescriptors:` the same path.
4. `row_tops: Vec<u16>` or a running-sum cache, `VARIABLE_HEIGHT_ROWS` 65,536,
   `TABLE_ROW_MAX` 1024, `TABLE_ROW_MIN` from `form_factor()` (16 desktop and TV, 40 touch
   and wearable); text top `y_r + (h_r - 16) / 2`; fills and ring over `h_r`; grid on
   `y_r + h_r - 1`; wheel lands on the nearest row top, at least one row.
5. View-based when the delegate answers `tableView:viewForTableColumn:row:`; views for
   visible rows only; reuse queue by `identifier`, `makeViewWithIdentifier:owner:`,
   `prepareForReuse`, `VISIBLE_CELL_VIEWS` 512; frames as section 3 (`+ 16` in the first
   column); unframed `imageView` 16x16 at 8, `textField` from 8 or 32 to `w - 8`.
6. Draw order: fill, grid, cell views, `>`, ring. `backgroundStyle` set on selection
   changes; semantic label colours resolve to `text` on the selected row for the row's views.
7. Presses: `NSButton` subviews first, else the row.
8. Elements: `columnheader` value and `selected` as the toolkit; cell values from
   `textField.stringValue`.
