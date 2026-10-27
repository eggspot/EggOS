# EggOS v0.3.0 GUI toolkit and Settings: design brief

Design by Claude Fable (D16) for v0.3.0 item 34: the native toolkit (`userland/toolkit`) and the
Settings app. It extends `desktop-v0.3.md` (tokens, corners, shadow, pointer) and
`desktop-v0.3-gui.md` (section 1 "Tokens added" and its metrics, section 10 "The Settings
app", section 11 "Layout profiles"): everything there stays, and this brief only fixes what
those leave open. Every number is in px at scale 1x and doubles at 2x; colours are the existing
tokens plus the four in section 1. Everything is drawn by our own Rust code on the software
framebuffer: 1 px strokes, no anti-aliasing, no alpha, no motion except the caret's blink.
"Later" marks what 0.3.0 does not build.

## 0. The two fonts

- **Egg Sans 16** (`font-ui-16.txt`) is the UI font: every label, button, list row, field,
  heading and value. Proportional, cell 16 rows, baseline row 12, cap height 10 (rows 3-12),
  x-height 7 (rows 6-12), ascenders from row 2, descenders to row 15; accents of capitals in
  rows 0-1 and of small letters in rows 3-4, so Latin-1 fits the 16 rows. Each glyph's advance
  width (1-16) includes its right-side spacing: text is set by placing glyphs side by side with
  no extra gap and no kerning. Space 4, every digit 6 (tabular), capitals mostly 6, small
  letters mostly 6 (i and l 2, m and w 8), the widest (© ® æ and the fractions) 10. Coverage
  is ASCII and Latin-1
  (191 glyphs); anything else draws `?`.
- **Egg Mono 8x16** stays the terminal's font and nothing else uses it. Columns that section 8
  of `desktop-v0.3-gui.md` gave in glyphs keep their px values (80 and 128) and set numbers in
  Egg Sans, whose tabular digits keep columns aligned.
- The file is the source of truth: the toolkit holds the 191 glyphs as `(width: u8,
  rows: [u16; 16])` (bit 15 = column 0), in codepoint order, parsed from the file at compile
  time; a host test renders a known string and compares its hash.

## 1. Tokens added

| Token | Value | Used for |
|-------|-------|----------|
| `button.hover` | `#344050` | hovered push button, hovered unselected card |
| `button.pressed` | `#202833` | pressed push button, pressed unselected card, pressed unchecked box |
| `button.primary.hover` | `#F6C25C` | hovered primary button, hovered checked box or selected radio |
| `button.primary.pressed` | `#D99E2E` | pressed primary button, pressed checked box or selected radio |

Contrast: `text` on `button.hover` 8.7:1, on `button.pressed` 12.3:1; `icon.dark` on
`button.primary.hover` 11.8:1, on `button.primary.pressed` 8.2:1. Already fixed and reused
here: `text` on `menu.bg` 12.9:1, on `menu.hover` 10.5:1, on `selection` 7.5:1, on `field.bg`
16.0:1, on `button.bg` 10.5:1; `text.dim` on `menu.bg` 5.1:1 and on `field.bg` 6.3:1 (hints,
placeholders), on `button.bg` 4.1:1 (disabled controls only, where the reduced contrast is the
point); `accent` on `menu.bg` 8.4:1, on `button.bg` 6.8:1, on `selection` 4.9:1 (the focus ring
and the `>` mark on every surface they appear on).

The content surface of a window built with the toolkit (the Settings pane, dialogs) is
`menu.bg` (`#1C2430`), the same value as the sidebar's `dock.bg`; the 1 px `menu.border` line
between them is the separator, as section 10 draws it. Circles: a circle of diameter 16 is a
filled rectangle whose rows are shortened by `INSET_R8 = [5, 3, 2, 1, 1, 0, 0, 0]` (row 0 is
the outermost, mirrored for the lower half); a 6 px dot uses `INSET_R3 = [2, 1, 0]`. Like
`INSET_R6` and `INSET_R4`, a 1 px outline follows the same insets.

## 2. Text

- **Sizes.** One size: the 16-row cell at 1x (body) and the same bitmap pixel-doubled for
  headings (a 32-row cell, every advance doubled). Nothing else. Emphasis is colour or 2x.
- **Measure.** The width of a string is the sum of its glyphs' advances (the last glyph's
  spacing column counts; nothing corrects for it). Centred text is placed at
  `x + (w - width) / 2`, rounded down; right-aligned text ends at the right edge (its last
  spacing column is the 1 px before the edge).
- **Vertical placement.** In a control or row of height `h` the cell's top is at
  `y + (h - 16) / 2`: +6 in a 28 px control, +4 in a 24 px row, +8 in a 32 px row, +12 in a
  40 px row; the baseline is 12 below the cell's top. Body lines are set 20 px apart; a 2x
  heading occupies 32 and the next body line starts 48 below the heading's top, as section 10's
  Appearance pane does (heading at 16, cards at 64).
- **Ellipsis.** When a string is wider than its space, glyphs are dropped from the end until
  the rest plus `...` (three periods, 9 px) fits, and `...` is drawn after them. If not even one
  glyph plus `...` fits, as many whole glyphs as fit are drawn and nothing more; a glyph is
  never cut through. End ellipsis everywhere in 0.3.0; a middle ellipsis for file names is
  later. Text never wraps in 0.3.0 (every string is one line); a widget that needs two lines
  uses two labels.
- **Colour.** `text` on every surface (`menu.bg`, `dock.bg`, `bar.bg`, `button.bg`,
  `button.hover`, `button.pressed`, `menu.hover`, `selection`, `field.bg`). `text.dim` for
  secondary text: placeholders, hints, section headings inside a pane (`Places`), column
  headers, and every disabled control's text. `icon.dark` on `button.primary` and its hover and
  pressed fills. Nothing else; no third text colour.
- **Disabled.** A disabled control keeps its shape and draws its text, mark and border in
  `text.dim` on its normal fill; it ignores the pointer (no hover, no press, pointer stays
  `Arrow`) and is skipped by Tab.

## 3. Focus and keyboard navigation

- **Order.** Focusable widgets: push button, checkbox, radio group, card picker, list, text
  field. Tab moves to the next one in the order the window declares them (reading order: top
  to bottom, left to right), Shift+Tab to the previous; both wrap at the ends. A radio group
  and a card picker are one Tab stop each; a list is one. Disabled widgets are skipped. A
  window gives focus to its first focusable widget when it opens, unless it says otherwise
  (Settings: section 11).
- **Arrows.** Inside a list Up/Down move the selection one row, Home/End to the first and last,
  PageUp/PageDown by the visible rows minus one; the view scrolls to keep the selection
  visible. Inside a radio group Up/Down and Left/Right move the selection and select at once,
  not wrapping. Inside a card picker Left/Right and Up/Down move the focus ring between cards,
  not wrapping, and do not select (section 10: the change applies at once, so moving and
  choosing are two steps). Arrows elsewhere do nothing. In a text field Left/Right move the
  caret (section 7).
- **Enter and Space.** Space activates the focused push button (the pressed look while it is
  held, the action on release), toggles the focused checkbox, selects the focused radio or card.
  Enter activates the focused push button at once (no pressed look) and selects the focused
  card; otherwise it goes to the window's default button (section 4) if there is one, else
  nothing. In a list Enter is the row's open action (section 6).
- **Esc.** In a text field, as section 7. Otherwise it goes to the window: a `Dialog` closes as
  if its cancel button were pressed; a `Normal` window ignores it (Settings stays open).
- **The ring.** Section 1 of `desktop-v0.3-gui.md`: 2 px wide, `focus.ring`, `INSET_R4`
  corners, drawn 2 px inside the widget's edge, so the ring's outer edge is 2 px in and its inner
  edge 4 px in. Per widget: push button and text field, the control's rect; checkbox and radio,
  a 26x26 rect centred on the 16 px box or circle (so the ring sits 1 px outside the box, the
  label is not ringed); card, the card's rect, or 4 px inside it when the card is selected
  (the 2 px `accent` border, 2 px of `selection`, then the ring); list, the selected row's rect
  at inset 0 (the row's outermost 2 px: a 24 px row has no room for the 2 px inset, and
  neighbouring rows are never ringed at the same time). On the primary button the ring is
  `icon.dark`, because `focus.ring` is the button's own fill. The ring is drawn only while the
  window has keyboard focus and only on the focused widget; a pointer click on a widget focuses
  it and shows the ring (no "mouse mode" without rings in 0.3.0).
- **Pointer rules, every widget.** A press captures the widget: it shows its pressed look while
  the pointer is inside; moving out while held shows the hover look (or normal, when the
  pointer leaves the window); releasing inside activates; releasing outside does nothing. Only
  the primary button of the mouse does this; a right-click is passed to the window (context
  menus are the shell's, later in the toolkit). Pointer shape: `Arrow` over every widget except
  the text field, `IBeam`.

## 4. Push button

28 tall, `INSET_R4` corners, text centred with 16 px padding each side, at least 96 wide, 1 px
`button.border` inside the edge (section 1 of `desktop-v0.3-gui.md`). The text's cell top is at
+6.

| State | Fill | Border | Text |
|-------|------|--------|------|
| normal | `button.bg` | `button.border` | `text` |
| hover | `button.hover` | `button.border` | `text` |
| pressed | `button.pressed` | `button.border` | `text` |
| focused | as its other state | as its other state | `text`, plus the ring |
| primary (the default button) | `button.primary` | none (fill to the edge) | `icon.dark` |
| primary hover | `button.primary.hover` | none | `icon.dark` |
| primary pressed | `button.primary.pressed` | none | `icon.dark` |
| disabled (both kinds) | `button.bg` | `button.border` | `text.dim` |

A window has at most one default button; Enter reaches it from any widget that does not use
Enter itself (section 3). The text does not move when pressed. A button wider than its text
(a column of equal buttons) keeps its text centred. Buttons in a dialog's row are 8 px apart,
right-aligned, the default one last (rightmost) in every profile.

## 5. Checkbox and radio button

Both are a 16x16 mark at the control's left edge, vertically centred in a 28 px control (mark
top at +6), and a label whose cell starts 24 px right of the mark's left edge (16 px mark + 8 px
gap). The hit target is the whole control, mark and label, 28 tall; clicking the label acts
like clicking the mark.

- **Checkbox**: a 16x16 square with 1 px `button.border` and `field.bg` fill when unchecked;
  when checked, filled `button.primary` with no border and a tick in `icon.dark` made of nine
  2x2 squares at (3,8) (4,9) (5,10) (6,9) (7,8) (8,7) (9,6) (10,5) (11,4) (x, y from the
  square's top-left). Hover: unchecked, border `text.dim`; checked, fill
  `button.primary.hover`. Pressed: unchecked, fill `button.pressed`; checked, fill
  `button.primary.pressed`. Disabled: border and tick `text.dim`, the checked fill `button.bg`.
  Space or a click toggles.
- **Radio button**: a 16 px circle (`INSET_R8`), 1 px `button.border` outline and `field.bg`
  fill when unselected; when selected, filled `button.primary` with a 6 px `icon.dark` dot
  (`INSET_R3`) at (5,5). Hover, pressed and disabled as the checkbox. Radios are a group: one is
  always selected, a click or Space selects, arrows move the selection (section 3). Rows of a
  vertical group are 28 apart; a horizontal group (Settings' `Scale`) places the next radio 24 px
  after the previous label's end.
- **Card picker** (section 10's three profile cards, 128x112, 16 apart, `INSET_R6`): a radio
  group drawn as cards. Unselected: `button.bg` fill, 1 px `button.border`; hover
  `button.hover`; pressed `button.pressed`. Selected: `selection` fill and a 2 px `accent`
  border; hover and press do not change a selected card. Focused: the ring of section 3 (inset
  2, or 4 on the selected card). The miniature and label inside are as section 10 draws them,
  the label centred with its cell top at 76; the miniature does not change with the state.
  Left/Right and Up/Down move the ring, Enter, Space or a click selects.

## 6. List rows

A list is a column of 24 px rows in a scroll view (section 8), text cell top at +4. Text at
x + 24 when the list has a mark column (the `>` mark or a type mark at x + 8, section 8 of
`desktop-v0.3-gui.md`), at x + 16 otherwise (the Settings sidebar, whose rows are 32 and whose
`>` goes at x + 4, which fits before the text at 16). States, in increasing priority:

| State | Fill | Mark | Text |
|-------|------|------|------|
| normal | the list's surface | the row's own type mark, if any | `text` |
| hover | `menu.hover` | unchanged | `text` |
| selected, list unfocused | `selection` | `>` in `accent` replaces the type mark | `text` |
| selected, list focused | `selection` | `>` in `accent` | `text`, plus the ring at inset 0 |

The `>` is Egg Sans' `>` (5 wide) drawn at the mark x, cell top +4; it is the non-colour cue of
section 1, so every selectable list has it. Hover on the selected row changes nothing. Exactly
one row is selected whenever the list is not empty (the first, by default); a click selects
and focuses the list, a double-click or Enter is the row's open action (switch pane, open a
directory), which the owning window defines. No multi-selection, no drag, no column resizing
(later). An empty list draws its empty text (`No matches`, `No items`) in `text.dim` at x + 16
of the first row.

## 7. Single-line text field

28 tall, `INSET_R4` corners, `field.bg` fill, 1 px `button.border`; the text's cell top at +6,
the text inset 8 from the left and right edges (the inner rect is `x + 8 .. right - 8`). Hover:
border `text.dim`. Focused: border `button.border`, the ring, and the caret. Disabled: text and
border `text.dim`, no caret.

- **Caret**: a 1x16 `text` rectangle at the insertion point, the cell's full height, drawn in
  the spacing column before the next glyph (so after the last glyph it is 1 px past its ink).
  It blinks 500 ms on, 500 ms off like the terminal's cursor, restarted "on" by every keystroke,
  caret move and click; it is drawn only while the field is focused and its window has focus.
- **Placeholder**: shown in `text.dim` while the text is empty, focused or not, with the caret
  before it.
- **Scrolling**: the field keeps a scroll offset in px, 0 while the text fits the inner rect.
  After every edit or caret move, if the caret is right of the inner rect the offset grows so
  the caret is at the inner rect's right edge; if it is left, the offset shrinks so the caret
  is at the left edge; if the text's end is left of the right edge with a positive offset, the
  offset shrinks to show as much text as fits. Glyphs are clipped at the inner rect's edges
  (partial glyphs are allowed here, unlike the ellipsis rule).
- **Keys**: a printable key inserts its character at the caret (the US layout's characters,
  Latin-1 included; anything the font lacks is inserted and drawn as `?`); Backspace deletes the
  character before the caret, Delete the one after; Left and Right move the caret one character;
  Home and End move it to the ends. The buffer holds up to 255 bytes of UTF-8; input beyond
  that is ignored. Enter goes to the window's default button (section 3), else nothing. Esc is
  the field's `escape` setting: `Clear` empties the text (search and filter fields; when
  already empty Esc goes on to the window) or `Revert` restores the text the field had when it
  got focus (the Files path field). A click focuses the field and puts the caret at the nearest
  glyph boundary to the pointer; the pointer over the field is `IBeam`.
- **No selection in 0.3.0**: no Shift+arrows, no drag selection, no double-click, no clipboard,
  no Ctrl/Cmd+A, no word moves. Later.

## 8. Scroll view

A clipping rect with a content height; vertical only. When the content is taller than the
view, a scroll bar 8 px wide is drawn inside the view's right edge, full height: track
`progress.track`, thumb `bar.pressed` with `INSET_R4` corners, the thumb's length
`view_h * view_h / content_h`, at least 24, its position proportional to the offset; thumb
`button.inactive` while hovered or dragged. When the content fits, no bar is drawn and the
content uses the full width. The content's rows are laid out in the full width minus 8 only
when the bar shows (so text does not move when a list grows past the view; lists reserve the 8 px
from their first row on, the Files list already does).

Wheel: one tick scrolls 72 px (3 rows of 24, section 8 of `desktop-v0.3-gui.md`, the Mouse
pane's `Wheel lines: 3`), clamped to `0 .. content_h - view_h`; a list inside a scroll view
scrolls in whole rows (72 px is three of them). Dragging the thumb scrolls; a click on the
track above or below the thumb scrolls by `view_h - 24`. Keys reach the view only through the
list inside it (section 3); the bar itself is never focused. Horizontal scrolling and inertia:
later.

## 9. Damage and scale

A widget state change (hover, press, focus, caret, text) damages that widget's rect only; the
caret's blink damages the 1x16 caret rect; a list selection change damages the two rows; a
scroll damages the view. Scale 2x: every number here doubles, including the 1 px strokes,
which become 2 px, and the fonts, which pixel-double; the engineer's layout code takes the
scale once at the window's creation (a scale change applies at the next boot, section 10).

## 10. Later

Text selection and clipboard; middle ellipsis; wrapping and multi-line text; tooltips; sliders,
spinners and dropdowns (every 0.3.0 choice is radios or cards); menus and context menus inside
the toolkit; tables with resizable columns; horizontal scrolling; drag and drop; a light theme;
IME and dead keys; right-to-left text; a second weight of Egg Sans; focus rings hidden in
"mouse mode".

## 11. Settings in 0.3.0

Everything section 10 of `desktop-v0.3-gui.md` fixes stays, with these settlements:

- **Window**: `Settings`, default content 640x440; the minimum is **640x360** (section 10 said
  560: at 560 the three cards, 16 + 3 x 128 + 2 x 16 + 16 = 448 wide, do not fit the 384 px
  pane, so the minimum width is the default). Sidebar x 0..175 `dock.bg`, `menu.border` on its
  right; the pane is `menu.bg` with 16 px padding.
- **Sidebar**: the search field at (8, 8), 160x28, placeholder `Search settings`, `escape:
  Clear`; rows of 32 from y 44 (`Appearance`, `Display`, `Keyboard`, `Mouse`, `Components`,
  `About`), text at x 16, the current pane's row `selection` with the `>` at x 4 and, when the
  list has focus, the ring. The search filters rows by case-insensitive substring of the pane
  name, its synonyms (section 10's list) and the pane's setting names; when the current pane
  is filtered out, the first remaining row becomes current; when none remains, the list shows
  `No matches` (section 6) and the pane keeps showing. Initial focus is the search field (type
  to find); Tab goes to the sidebar list, then into the pane; Down in the search field also
  moves focus to the list.
- **Appearance** (the first pane): as section 10, with the states of section 5 for the cards.
  Rows from y 192: `Primary modifier` and `Window controls` as read-only values in `text`,
  right-aligned to pane right - 16 (`Cmd (Super key)` on macOS, `Ctrl` otherwise; `Left` on
  macOS, `Right` otherwise). `Scale`: a horizontal radio group `1x` `2x` right-aligned (two
  radios, replacing section 10's two push buttons: the chosen one must look chosen), and the
  hint `Takes effect at the next boot` in `text.dim` right-aligned 16 px before the group. When
  no EggFS disk is attached (D131: nothing under `/home` survives), one more line at y 316,
  x 16, `text.dim`: `No disk attached: this choice lasts until power-off.` (252 px; its cell ends
  at 332, the pane's height at the minimum window size). With a disk, the line is absent and
  the choice is written to `/home/<user>/.config/eggos/shell`.
- **Applying a profile**: instant and silent. The clicked card becomes selected in the same
  frame the panels re-lay out, the read-only rows update, nothing else is shown: no spinner,
  no toast, no dialog. (If a later version makes it slower than one frame, it shows the pressed
  card until done; not 0.3.0.)
- **Display**: rows of 40 from y 64, read-only values in `text` right-aligned: `Resolution`
  `1280 × 800` (the `×` is U+00D7, in the font), `Scale` `1x` or `2x`, `Pointer` `GPU cursor`
  or `Software` (whichever the server is using, section 2 of `desktop-v0.3-gui.md`).
- **Keyboard**: one row, `Layout` `US`, read-only. `Key repeat`: later.
- **Mouse** and **Components**: the heading, then one line at (16, 64) in `text.dim`: `Coming
  in a later version.` Nothing else; no controls. (Components' list and `Remove`: later, with
  item 36.)
- **About**: the boot screen's egg at (16, 64), 48 tall; `EggOS <version>` at 2x at (80, 64);
  then body lines from y 104, 20 apart, x 80, in `text`: `<arch> · <n> CPUs · <memory> MB`
  (the `·` is U+00B7; memory rounded to whole MB), `Up <h>:<mm>:<ss>` (refreshed once a second,
  damaging that line only), `MIT OR Apache-2.0`. The values are what the kernel already
  reports to `uname` and the boot log; nothing new is measured.
- **CLI and MCP (D22)**: `settings get <key>`, `settings set <key> <value>` and `settings list`;
  `settings_get`, `settings_set`, `settings_list` as MCP tools. Keys in 0.3.0: `shell.profile`
  (`windows|macos|linux`, settable), `display.scale` (`1|2`, settable, next boot),
  `display.resolution`, `display.pointer`, `keyboard.layout`, `system.version`, `system.arch`,
  `system.cpus`, `system.memory_mb`, `system.uptime_s` (read-only; `set` on them answers
  `read-only`). What the GUI shows and what the CLI prints are the same values from the same
  code.
- **Later**: `Key repeat`, `Wheel lines`, `Primary button`, the Components list and `Remove`,
  a `Reset` of the profile, searching inside panes' values, scrolling panes (every 0.3.0 pane
  fits 332 px), keyboard shortcuts for panes.
