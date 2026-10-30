# EggOS v0.3.0 Windows controls, menus and file picker: design brief

Design by Claude Fable (D16) for v0.3.0 item 42, "Windows GUI apps, part 2": the standard
controls (`BUTTON`, `EDIT`, `STATIC`, `LISTBOX`), the menu bar with its drop-downs, and the
file picker behind `GetOpenFileNameW`/`GetSaveFileNameW`. It extends `windows-gui-v0.3.md`
(part 1): every rule there stays (section 0's dialog surface, section 2's system colours and
text rules, section 3's pointer mapping), and this brief only adds. The Windows layer draws all
of it into the program's own window buffer; the server draws only the chrome. Every number is
in pixels at scale 1 and doubles at 2x; the font is Egg Mono 8x16 (cell 8x16, underline row
14, `...` three `.` glyphs, a codepoint the font lacks `?`), no second font, no anti-aliasing.
Primitives: `fill`, `rounded` with an inset table, 1 px rectangles, `pixel`, `glyph`.

Sections 10-19 were added for the Real apps track's first Windows app, Pegs from Simon Tatham's Puzzles (D174); sections 0-9 stand, except where a later section says it replaces a rule.

## 0. Two palettes, one set of controls

**Decision: a program's client area, its menu bar and its drop-downs are light (Win32's
colours); EggOS's own dialogs drawn by the layer (the file picker, the message box) are dark.**
The menu is part of the program's content: programs read `COLOR_MENU`/`COLOR_MENUTEXT` and
later owner-draw items against them, Windows itself keeps classic menus light in dark mode,
and a dark drop-down over a white client would read as a foreign object. The picker is not a
Windows dialog but EggOS's, so it gets part 1's rule 0 (`menu.bg`) and the toolkit's look.
Every control is drawn by one function from a palette; the two palettes differ in the values
below and in exactly two shapes: corners (square in Light; `INSET_R4` on push buttons, edits
and lists in Dark) and the focus indicator (section 1). Nothing else branches on the palette.

| Role | Light (inside a program) | Dark (EggOS dialogs) |
|---|---|---|
| `surface` | `F0F0F0` (`COLOR_BTNFACE`) | `menu.bg` `#1C2430` |
| `text` | `000000` (`COLOR_BTNTEXT`, `COLOR_WINDOWTEXT`) | `text` `#E6EAF0` |
| `text.disabled` | `6D6D6D` (`COLOR_GRAYTEXT`) | `text.dim` `#8A94A6` |
| `caret` | `000000` | `text` |
| `button.fill` / `button.border` | `E1E1E1` / `ADADAD` | `button.bg` `#2A3440` / `button.border` `#3A4454` |
| `button.hover.fill` / `.border` | `E5F1FB` / `0078D7` | `button.hover` `#344050` / `text.dim` |
| `button.pressed.fill` / `.border` | `CCE4F7` / `005499` | `button.pressed` `#202833` / `text.dim` |
| `button.disabled.fill` / `.border` | `CCCCCC` / `BFBFBF` | `button.bg` / `button.border` |
| `button.default.fill` / `.border` / `.text` | `E1E1E1` / `0078D7` (2 px) / `000000` | `button.primary` `#F2B33D` / same / `icon.dark` `#0B0E13` |
| `button.default.hover.fill` / `.border` | `E5F1FB` / `0078D7` | `button.primary.hover` `#F6C25C` / same |
| `button.default.pressed.fill` / `.border` | `CCE4F7` / `005499` | `button.primary.pressed` `#D99E2E` / same |
| `field.fill` / `field.border` | `FFFFFF` (`COLOR_WINDOW`) / `7A7A7A` | `field.bg` `#0B0E13` / `button.border` |
| `field.hover.border` / `field.focus.border` | `171717` / `0078D7` | `text.dim` / `button.border` |
| `field.readonly.fill` / `.border` | the `WM_CTLCOLORSTATIC` answer (`F0F0F0`) / `CCCCCC` | `field.bg` / `button.border` |
| `box.fill` / `box.border` / `box.mark` | `FFFFFF` / `333333` / `000000` | `field.bg` / `button.border` / `icon.dark` |
| `box.checked.fill` / `.hover.fill` / `.pressed.fill` | `FFFFFF` / `FFFFFF` / `CCE4F7` | `button.primary` / `button.primary.hover` / `button.primary.pressed` |
| `box.hover.border` / `box.pressed.fill` | `0078D7` / `CCE4F7` | `text.dim` / `button.pressed` |
| `box.disabled.fill` / `.border` / `.mark` | `F0F0F0` / `BFBFBF` / `6D6D6D` | `button.bg` / `text.dim` / `text.dim` |
| `group.border` | `DCDCDC` | `menu.border` `#2A3340` |
| `highlight` / `highlight.text` | `0078D7` / `FFFFFF` (`COLOR_HIGHLIGHT`, `COLOR_HIGHLIGHTTEXT`) | `selection` `#2E4A6B` / `text` |
| `focus` | `000000` (1 px dotted rect) | `focus.ring` `#F2B33D` (2 px `INSET_R4` ring) |
| `scroll.track` / `scroll.thumb` | `F0F0F0` / `CDCDCD` | `progress.track` `#26303F` / `bar.pressed` `#334052` |
| `menu.bar` / `menu.fill` | `F0F0F0` (`COLOR_MENUBAR`, `COLOR_MENU`) | `bar.bg` `#141B26` / `menu.bg` |
| `menu.border` / `menu.separator` | `A0A0A0` / `D7D7D7` | `menu.border` / `menu.border` |
| `menu.highlight` / `menu.text` / `menu.text.disabled` | `91C9F7` / `000000` (`COLOR_MENUTEXT`) / `6D6D6D` | `menu.hover` `#2A3440` / `text` / `text.dim` |

Light values that are not system colours (`E1E1E1`, `91C9F7`, ...) are Windows 10's themed
constants and stay fixed; `SetSysColors` is still ignored (part 1). Inset tables added:
`INSET_C14 = [5, 3, 2, 1, 1, 0, 0]` (a 14 px circle), `INSET_C12 = [4, 2, 1, 1, 0, 0]`,
`INSET_R3 = [2, 1, 0]` (a 6 px dot); a 1 px circle outline is the 14 px circle in the border
colour with the 12 px circle at (1, 1) in the fill colour.

## 1. Rules shared by every control

- **Rect.** A control is exactly the rect the program gave (`CreateWindowExW` x, y, w, h in
  client coordinates); nothing is drawn outside it and everything inside is clipped to it. The
  client area starts below the menu bar (section 6) when the window has one.
- **Label.** One line, Egg Mono; `&` underlines the next glyph at its row 14 (part 1's rule;
  `&&` is `&`), always shown, never only on Alt. Cell top in a control of height `h` is
  `y + max(0, h - 16) / 2`; a control shorter than 16 clips the glyph rows. Horizontal
  placement is per control. A label wider than its room (per control) is cut as part 1's
  `DT_END_ELLIPSIS`: `k = room / 8` cells; `k >= 4` draws the first `k - 3` glyphs then `...`;
  `k < 4` draws the first `k` glyphs and no `...`. `BS_LEFT`, `BS_RIGHT`, `BS_TOP`,
  `BS_BOTTOM`, `BS_MULTILINE`, `BS_LEFTTEXT`, `BS_PUSHLIKE`, `ES_CENTER`, `ES_RIGHT`,
  `ES_UPPERCASE`, `ES_LOWERCASE` have no effect in this item (listed).
- **Program colours.** Before painting a control the layer sends the parent `WM_CTLCOLORSTATIC`
  (STATIC, check box, radio, group box, read-only or disabled EDIT), `WM_CTLCOLORBTN` (push
  button), `WM_CTLCOLOREDIT` or `WM_CTLCOLORLISTBOX`. The answer's brush and the DC's text
  colour replace the control's `surface` and `text` only: the STATIC's whole rect; the fill
  behind a check box's, radio's or group box's label (never the box, circle or frame); the push
  button's label colour (never its fill or border, as Windows' themed buttons); the EDIT's and
  LISTBOX's `field.fill` and unselected text (never the border, caret, highlight or scroll
  indicator). `DefWindowProc` answers `COLOR_BTNFACE` + `COLOR_WINDOWTEXT` for STATIC and
  BTN, `COLOR_WINDOW` + `COLOR_WINDOWTEXT` for EDIT and LISTBOX; a `NULL_BRUSH` answer fills
  nothing. **A disabled control's text is always `text.disabled`**, whatever the answer. The
  Dark palette never sends these messages (the picker is the layer's own window).
- **Focus.** The layer keeps one focused control per window (`SetFocus`, `WM_SETFOCUS`,
  `WM_KILLFOCUS`, a click on a focusable control). The server's `FocusIn`/`FocusOut` change
  only the window's activation (`WM_ACTIVATE`): they never move the focused control, never
  hide a caret, never clear a focus indicator, so a screendump is the same before and after
  the server's injected focus pair. Indicators are drawn whenever the control is the focused
  one. Light: a 1 px dotted rectangle (the pixels of the rect's 1 px outline where `x + y` is
  even, content coordinates) in `focus`, or in `highlight.text` on a `highlight` fill. Dark: the
  toolkit's 2 px `INSET_R4` ring in `focus` (`icon.dark` on a default button). Where each sits
  is in the control's section. Statics and group boxes are never focused.
- **Keys.** Keys reach the focused control; Tab, Shift+Tab (creation order, `WS_TABSTOP`,
  wrapping, disabled skipped), arrows between radios of a `WS_GROUP` group, Enter to the
  default push button and Esc to `IDCANCEL` work when the program's loop calls
  `IsDialogMessageW` (as the picker does for itself). Alt+mnemonic with `IsDialogMessageW`
  activates the push button, toggles the check box or selects the radio whose label has it, or
  focuses the control created after a STATIC with it.
- **Pointer rules** are the toolkit's: a left button-down on a control captures it until the
  release; pressed while the pointer is inside, hovered (or plain, outside the window) while
  outside; a release inside activates, elsewhere nothing. Only the left button. A disabled
  control ignores the pointer and draws no hover.

## 2. BUTTON

**Push button** (`BS_PUSHBUTTON`, `BS_DEFPUSHBUTTON`): the rect filled, a 1 px border inside
the edge (Light square; Dark `INSET_R4`); the default button's border is 2 px (a second 1 px
rect inset 1, same colour) in Light, and in Dark the primary fill with no border. The default
button is the one with `BS_DEFPUSHBUTTON`, or the focused push button while one has focus
(Windows moves the default with the focus; the style's button regains it when focus leaves the
push buttons). Label centred: `x + (w - 8n) / 2`, room `w - 8`.

| State | Light fill / border / text | Dark fill / border / text |
|---|---|---|
| plain | `E1E1E1` / `ADADAD` / `000000` | `button.bg` / `button.border` / `text` |
| hovered | `E5F1FB` / `0078D7` / `000000` | `button.hover` / `text.dim` / `text` |
| pressed (held inside, or Space held) | `CCE4F7` / `005499` / `000000` | `button.pressed` / `text.dim` / `text` |
| default | `E1E1E1` / `0078D7` 2 px / `000000` | `button.primary` / none / `icon.dark` |
| default hovered, pressed | `E5F1FB` / `0078D7` 2 px; `CCE4F7` / `005499` 2 px | `button.primary.hover`; `button.primary.pressed` / none / `icon.dark` |
| disabled | `CCCCCC` / `BFBFBF` / `6D6D6D` | `button.bg` / `button.border` / `text.dim` |
| focused | plus the dotted rect at inset 3 | plus the ring at inset 2 |

A release inside, Enter on it, or Space released sends `WM_COMMAND` `BN_CLICKED`. Pointer:
`Arrow`.

**Check box** (`BS_CHECKBOX`, `BS_AUTOCHECKBOX`, `BS_3STATE`, `BS_AUTO3STATE`): a 14x14 box
at `(x + 2, y + (h - 14) / 2)` (1 px border, square in both palettes), the label's cell at
`x + 22`, room `w - 24`; the fill behind the label is the control's `surface`. Checked: the
box filled `box.checked.fill` with a tick of eight 2x2 squares in `box.mark` at (3, 7) (4, 8)
(5, 9) (6, 8) (7, 7) (8, 6) (9, 5) (10, 4) from the box's top-left (columns 3..11, rows
4..10); indeterminate: a 6x6 `box.mark` square at (4, 4). Hover: border `box.hover.border`
(checked in Dark: fill `box.checked.hover.fill`). Pressed: fill `box.pressed.fill` (checked:
`box.checked.pressed.fill`). Disabled: `box.disabled.*`, label `text.disabled`. The whole rect
is the hit target; `BS_AUTO*` toggles on release or Space, `BM_SETCHECK` sets, `BM_GETCHECK`
reads. Focus, Light: the dotted rect at `(x + 21, cell_top - 1, 8n + 2, 18)` around the
label, clipped to the rect; Dark: the 2 px ring at inset 0 of `(x, box_y - 2, 18, 18)`.
Pointer: `Arrow`.

**Radio button** (`BS_RADIOBUTTON`, `BS_AUTORADIOBUTTON`): the check box's layout with a
14 px circle (`INSET_C14` outline, section 0) instead of the box; selected: filled
`box.checked.fill` with a 6 px `INSET_R3` dot in `box.mark` at (4, 4). The same hover, pressed,
disabled, focus and label rules. `BS_AUTORADIOBUTTON` selects on release or Space and clears
the others of its group (from the previous `WS_GROUP` control to the next); arrows move the
selection within the group. Pointer: `Arrow`.

**Group box** (`BS_GROUPBOX`): a 1 px `group.border` rectangle at `(x, y + 7, w, h - 7)` and
the caption's cells at `(x + 8, y)`, drawn opaque in the control's `surface` so the line is
hidden from `x + 6` to `x + 10 + 8n` (the gap is 2 px wider than the text each side); room
`w - 16`; no caption, no gap. Disabled: caption `text.disabled`. Never focused, never
pressed; the pointer is `Arrow`; clicks fall through to nothing.

## 3. EDIT, single line

The rect filled `field.fill`, a 1 px border (Light square; Dark `INSET_R4`); the inner text
rect is `x + 4 .. x + w - 4` horizontally (`V = (w - 8) / 8` visible cells), cell top
`y + max(0, h - 16) / 2`, glyphs clipped to the rect inset 1. `ES_PASSWORD` draws every
character as `*` (`EM_SETPASSWORDCHAR` changes it); `ES_NUMBER` inserts only `0`..`9`.

| State | Light fill / border / text | Dark fill / border / text |
|---|---|---|
| plain | `FFFFFF` / `7A7A7A` / `000000` | `field.bg` / `button.border` / `text` |
| hovered (not focused) | `FFFFFF` / `171717` | `field.bg` / `text.dim` |
| focused | `FFFFFF` / `0078D7` | `field.bg` / `button.border`, plus the ring at inset 2 |
| read-only (`ES_READONLY`) | the `WM_CTLCOLORSTATIC` answer (`F0F0F0`) / `CCCCCC` / `000000` | `field.bg` / `button.border` / `text` |
| disabled | as read-only, text `6D6D6D`, no caret, not focusable | `button.bg` / `button.border` / `text.dim` |

**Caret**: a 1x16 `caret` rectangle at `x + 4 + 8 (c - o)` (caret index `c`, scroll offset
`o` in cells), the cell's full height, drawn while the edit is its window's focused control,
in a read-only edit too. It is steady: never blinks, never hides on `FocusOut`, so screendumps
are deterministic. **Scrolling** is in whole cells: after every edit or caret move,
`o = max(o, c - V + 1)`, then `o = min(o, c)`, then `o = min(o, max(0, len + 1 - V))`; `o` is 0
whenever `len < V`. Glyphs left of `x + 4` are not drawn; a glyph never straddles the inner
rect's edges (cells align to it by construction).

Keys: a printable codepoint (>= 0x20, not 0x7F) inserts at the caret (limit
`EM_SETLIMITTEXT`, default 32767 units; beyond it ignored); Backspace, Delete, Left, Right,
Home, End; Tab, Enter and Esc go to `IsDialogMessageW` (section 1); Ctrl combinations do
nothing (no clipboard, no selection in this item: the caret only). Every text change sends
`EN_UPDATE` then `EN_CHANGE`; focus sends `EN_SETFOCUS`/`EN_KILLFOCUS`. A click focuses and
puts the caret at the nearest cell boundary to the pointer. Read-only: the navigation keys
work, nothing is inserted or deleted. Pointer: `IBeam` (`Arrow` when disabled).

## 4. STATIC

The whole rect filled with the `WM_CTLCOLORSTATIC` answer (default `COLOR_BTNFACE` `F0F0F0`,
text `COLOR_WINDOWTEXT`), then the text as part 1's `DrawTextW` with `DT_LEFT`, `DT_CENTER` or
`DT_RIGHT` (`SS_LEFT`, `SS_CENTER`, `SS_RIGHT`) plus `DT_WORDBREAK` wrapping at `w / 8` cells
and lines 16 apart from the rect's top (`SS_LEFTNOWORDWRAP` and `SS_SIMPLE`: one line, no
wrap; `SS_NOPREFIX`: `DT_NOPREFIX`; otherwise `&` underlines as everywhere), clipped to the
rect. Disabled: `text.disabled`. `SS_ICON`, `SS_BITMAP`, `SS_ETCHEDFRAME`, `SS_BLACKRECT` and
the other picture styles draw only the fill (listed). **Decision on the default background:
keep Windows' `COLOR_BTNFACE`.** A program that paints a `COLOR_WINDOW` window and does not
answer `WM_CTLCOLORSTATIC` sees a grey box on Windows too; programs that care already answer
the message, and a program's screenshots on EggOS should match its screenshots on Windows,
grey box included. Never focused, never hovered; a click falls through (`SS_NOTIFY` sends
`STN_CLICKED`). Pointer: `Arrow`.

## 5. LISTBOX, single selection

The rect filled `field.fill`, a 1 px `field.border` (Light square; Dark `INSET_R4`; the border
never changes with focus or hover). Inner rect: the rect inset 1. **Item height 20** (reported
by `LB_GETITEMHEIGHT`), rows from the inner top with no padding: row `i` at
`y + 1 + 20 (i - top)`, spanning the inner width (minus 8 when the scroll indicator shows);
text at `x + 4` (`x + 24` when the list has a mark column, picker only), cell top row + 2, cut
with `...` at the row's room. `V = (h - 2) / 20` whole visible rows (at least 1); the partial
row after them is drawn clipped to the inner rect. `LBS_SORT` keeps items in case-insensitive
order (ASCII folded, then by codepoint) on `LB_ADDSTRING`.

| Row state | Light fill / text | Dark fill / text |
|---|---|---|
| plain | `field.fill` / `text` (the `WM_CTLCOLORLISTBOX` answer) | `field.bg` / `text` |
| selected, focused or not | `0078D7` / `FFFFFF` | `selection` / `text` |
| caret row while the list is focused | plus the dotted rect at inset 0 of the row (`FFFFFF` on the selected row, `000000` else) | plus the 2 px ring at inset 0 of the row |

The caret row is the selected row, or row 0 while nothing is selected (`LB_SETCURSEL` -1).
Keys: Up/Down select the previous/next row (clamped; Down with no selection selects row 0),
Home/End the first/last, PageUp/PageDown by `V - 1`; a printable key selects the next row after
the caret, wrapping, whose first glyph matches case-insensitively. After any change, if
`sel < top` then `top = sel`; if `sel >= top + V` then `top = sel - V + 1`. A left button-down
on a row (whole or partial) selects it and scrolls it fully into view; a second button-down on
the same row within 400 ms and 4 px is a double-click. `LBS_NOTIFY` sends `LBN_SELCHANGE` on
every user change, `LBN_DBLCLK` on a double-click, `LBN_SETFOCUS`/`LBN_KILLFOCUS`.
`LB_SETTOPINDEX`, `LB_GETCURSEL`, `LB_GETTEXT`, `LB_GETCOUNT`, `LB_DELETESTRING`,
`LB_RESETCONTENT`, `LB_INSERTSTRING` as Win32. Hover changes nothing.

**Scroll indicator**, drawn when `count > V`: an 8 px column at the inner rect's right edge,
full inner height, `scroll.track`; the thumb `scroll.thumb`, `max(16, inner_h * inner_h /
(20 * count))` tall, top at `inner_top + (inner_h - thumb_h) * top / (count - V)`. It is not
interactive (clicks on it do nothing, not even select) and has no arrows; the wheel comes
later (listed). Rows end 8 px before the inner right edge only while it shows. Pointer:
`Arrow`.

## 6. The menu bar and its drop-downs

`SetMenu` on a window with a menu puts a **24 px bar** across the content's top
(`(0, 0, content_w, 24)`, `menu.bar`, no line under it, as Windows); the client area is the
content from `y = 24` (`GetClientRect` excludes it, `AdjustWindowRect` adds 24 with `bMenu`).
Top-level entries (`MF_POPUP` strings) from `x = 0`, each `8n + 16` wide, 24 tall, text at
`entry_x + 8`, cell top 4 (an underline is at row 18); an entry whose rect would pass the
content's width is not drawn, nor those after it (listed). States: plain `menu.text` on the
bar; hovered (pointer inside, no menu open) and open: fill `menu.highlight`, text
`menu.text`; `MF_GRAYED`/`MF_DISABLED` entries `menu.text.disabled`, never highlighted, never
opened. Pointer over the bar and drop-downs: `Arrow`.

**Drop-down** (`AppendMenuW` items of a popup; no nested popups in this item: an `MF_POPUP`
inside a drop-down draws as a disabled item): width `max(120, 8 * widest + 48)` where
`widest` is the widest label in cells; height `6 + 24 * items + 9 * separators`. Left edge at
the entry's x, shifted left so its right edge is at most the content width (never below 0);
top at 24; taller than the content's remaining height: **clipped** at the content's bottom
(the hidden items stay reachable by keyboard; no scrolling). Drawn over the client area in the
same buffer; closing invalidates the rect under it so the program repaints (`WM_PAINT`).
Inside: a 1 px `menu.border`, fill `menu.fill`; items from `top + 3`, each `(dx + 1, iy, w - 2,
24)`, text at `dx + 28`, cell top `iy + 4`; a separator is 9 rows with a 1 px `menu.separator`
line at its 5th row from `dx + 8` to `dx + w - 8`; `MF_CHECKED` draws the check box's tick in
the text colour with its box origin at `(dx + 7, iy + 5)`. The highlighted item: fill
`menu.highlight`, text `menu.text`; disabled items `menu.text.disabled` and never highlighted.
`WM_INITMENUPOPUP` goes to the window before the drop-down is laid out, so grey and check
states are current.

| Input | Does |
|---|---|
| button-down on a closed entry | opens its drop-down (no item highlighted); on the open entry: closes it |
| pointer moves onto another enabled entry while one is open | opens that one (no button needed) |
| pointer over an enabled item | highlights it; over a disabled item, a separator, the padding or outside: no item highlighted |
| left button **release** over an enabled item | activates it: the menu closes, the uncovered rect is invalidated, then `WM_COMMAND` (`wParam` = the id) is posted (so a press on the entry, a drag down and a release on an item works, and so does a click) |
| release elsewhere | nothing (the menu stays; a release on the entry that was pressed keeps it open) |
| button-down outside the bar and drop-down, inside the content | closes the menu; that press and its release are **not** delivered to the program |
| `FocusOut` not followed by `FocusIn` in the same read of the event queue | closes the menu (a click in another window); the server's injected pair leaves it open |
| F10, or Alt pressed and released with no key between | bar mode: the first enabled entry highlighted, no drop-down; again, or Esc: leaves |
| Alt+mnemonic (any time), or a mnemonic letter in bar mode | opens that entry's drop-down with its first enabled item highlighted |
| Left / Right (bar mode or open) | the previous / next enabled entry, wrapping; when a drop-down was open the new one opens too |
| Down, Enter, Space in bar mode | opens the highlighted entry's drop-down, first enabled item highlighted |
| Up / Down in a drop-down | previous / next enabled item, wrapping, skipping separators and disabled items |
| Enter or Space in a drop-down | activates the highlighted item (nothing when none) |
| a mnemonic letter in a drop-down | activates the item whose label has it |
| Esc in a drop-down | closes it, back to bar mode (the entry stays highlighted); Esc again leaves |

While a menu is open or the bar is in bar mode, no key reaches the program and pointer events
over the client are consumed. A chrome drag by the server sends the window nothing, so the
menu stays open through it. The bar and drop-downs always use the Light palette inside a
program (section 0); the Dark column exists so an EggOS dialog may carry a menu later.

## 7. The file picker

`GetOpenFileNameW` and `GetSaveFileNameW` open EggOS's own picker: a `Dialog` of `hwndOwner`'s
top-level window (centred on it, close control only), or a `Normal` `FIXED_SIZE` window centred
in the work area when there is no shown owner (part 1's table). Title `Open` / `Save As`, or
`lpstrTitle`. Content **560x400** (outside 562x430: fits a 1024x740 work area with room, and
sits well on 1280x800). Dark palette, surface `menu.bg`; the layer's own control code draws
it, so it needs no `WM_CTLCOLOR*`.

| Element | Rect (x, y, w, h) | Details |
|---|---|---|
| `Up` push button | 16, 16, 64, 28 | label at x 40, cell top 22; disabled at `C:\` |
| folder path, read-only EDIT, no Tab stop | 88, 16, 456, 28 | text at x 92, cell top 22, 56 cells; `C:\home\root` style (section 7.1) |
| entry LISTBOX, mark column | 16, 56, 528, 242 | inner 17, 57, 526, 240: exactly 12 rows of 20 (57, 77, ..., 277); text at x 40 |
| `File name:` STATIC | 16, 320, 80, 16 | `text` on `menu.bg` |
| file name EDIT | 104, 314, 440, 28 | text at x 108, cell top 320, 54 cells; initial focus |
| filter STATIC | 16, 362, 320, 16 | the selected filter's description in `text.dim`, cut with `...` at 40 cells |
| `Cancel` push button | 352, 356, 96, 28 | label at x 376, cell top 362 |
| `Open` / `Save` default push button | 448, 356, 96, 28 | label at x 480 (`Save` the same), `button.primary`, `icon.dark` |

A folder row's mark: two rects at `(20, iy + 5, 12, 8)` `icon.folder.back` `#B88A24` and
`(20, iy + 7, 12, 6)` `icon.folder` `#E2B341`; files have no mark. Selected row `selection`;
the ring at inset 0 while the list is focused. Tab order: list, file name, Open/Save, Cancel,
Up, wrapping. Pointer: `IBeam` over the two edits, `Arrow` elsewhere.

### 7.1 Behaviour

- **Folder**: `lpstrInitialDir`, else the process's current directory. Shown and returned in
  Windows form through the layer's path mapping (`C:\` is `/`, `\` for `/`): `/home/root` is
  `C:\home\root`; the root is `C:\`.
- **List**: the folder's entries without `.`, `..` and names starting with `.` (hidden, as
  Windows hides hidden files); folders first, then files matching the active filter; each
  group sorted case-insensitively (ASCII folded, then by codepoint). No `..` row: `Up` and
  Backspace in the list go to the parent. An unreadable folder shows an empty list. Nothing
  is selected on entry; the list's `top` is 0.
- **Filter**: `lpstrFilter` is pairs of description and pattern; `nFilterIndex` (1-based; 0
  or out of range: the first pair) picks the one applied, `*` and `?` case-insensitive, `;`
  separating alternatives (`*.txt;*.md`); no filter means every file; the description shows in
  the filter STATIC. Changing the filter: later (listed).
- **Selecting**: a single click or an arrow move onto a file row copies its name into the file
  name edit (caret at the end); onto a folder row leaves the edit as it is. Double-click or
  Enter on a folder row enters it (the edit is unchanged); double-click on a file row accepts.
- **Accepting** (Enter anywhere but a folder row, `Open`/`Save`): the edit's text, trimmed,
  empty does nothing; `C:\...` (or `/...`) is absolute, else joined to the folder, `/` accepted
  for `\`; a name that is a folder enters it and clears the edit. Otherwise Open with
  `OFN_FILEMUSTEXIST` and no such file shows a part-1 message box on the picker, caption the
  picker's title, `MB_OK | MB_ICONWARNING`: `<name>` / `File not found.` / `Check the file name
  and try again.` (three lines), then focus returns to the edit. Save with
  `OFN_OVERWRITEPROMPT` and an existing file asks `<name> already exists.` / `Do you want to
  replace it?` with `MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2`; `Yes` accepts, `No` returns to
  the edit. Save appends `.` + `lpstrDefExt` when the name has no extension. The result fills
  `lpstrFile` (full Windows path; too small: `FALSE`, `FNERR_BUFFERTOOSMALL`),
  `lpstrFileTitle`, `nFileOffset`, `nFileExtension`; the call returns `TRUE`.
- **Cancelling**: Esc, `Cancel`, `CloseRequest` (the close control, Alt+F4): `FALSE`,
  `CommDlgExtendedError` 0. The Save dialog pre-fills the edit with `lpstrFile`'s name part.

## 8. Worked example

A window with a menu and a client area of 400x300: content **400x324**, class brush
`COLOR_BTNFACE + 1`, so the client is `F0F0F0` from row 24 down. Menu: `File` (`&Open...` id
101, `&Save As...` 102, separator, `E&xit` 103) and `&Help` (`&About` 201). Client controls as
given, in content coordinates `(x, y + 24)`. Frame A: the user clicked the EDIT and typed
`egg`; the LISTBOX holds `alpha` and `egg` (sorted), `egg` selected by `LB_SETCURSEL 1`.

| Element | Rectangle (x, y, w, h) | Pixels |
|---|---|---|
| menu bar | 0, 0, 400, 24 | `F0F0F0` |
| `File` entry | 0, 0, 48, 24 | text `File` at (8, 4) `000000`; no mnemonic |
| `Help` entry | 48, 0, 48, 24 | text `Help` at (56, 4); `H` underlined: row 18, x 56..63 |
| STATIC `Name:` | 16, 40, 64, 24 | fill `F0F0F0`; `Name:` at (16, 40), `000000` |
| EDIT, focused | 88, 38, 200, 24 | fill `FFFFFF`, 1 px border `0078D7`; `egg` at (92, 42); caret `(116, 42, 1, 16)` `000000`; `V` 24, `o` 0 |
| `&Add`, default, not focused | 296, 38, 88, 24 | fill `E1E1E1`; 2 px border `0078D7` (rows 38-39 and 60-61, columns 296-297 and 382-383); `Add` at (328, 42); `A` underlined row 56, x 328..335 |
| check box `&Upper case`, unchecked | 16, 76, 160, 24 | fill `F0F0F0`; box (18, 81, 14, 14) border `333333`, fill `FFFFFF`; `Upper case` at (38, 80); `U` underlined row 94, x 38..45 |
| LISTBOX, not focused | 16, 112, 200, 160 | fill `FFFFFF`, border `7A7A7A`; inner (17, 113, 198, 158), `V` 7, partial 18 px; no indicator |
| row 0 `alpha` | 17, 113, 198, 20 | `FFFFFF`; text at (20, 115) `000000` |
| row 1 `egg`, selected | 17, 133, 198, 20 | `0078D7`; text at (20, 135) `FFFFFF` |
| STATIC status `Ready` | 16, 288, 368, 24 | fill `F0F0F0`; `Ready` at (16, 288) |
| the rest of the client | | `F0F0F0` |

Frame B, after Tab three times (Add, Upper case, list): the EDIT's border is `7A7A7A` and its
caret is gone; the list's row 1 keeps `0078D7` and gets the dotted rect on the 1 px outline of
`(17, 133, 198, 20)` in `FFFFFF` (pixels where `x + y` is even). Hovering `Add` turns its fill
`E5F1FB`; Space on the focused `Upper case` fills the box `CCE4F7` while held and shows the
tick on release.

Frame C, the File drop-down open with `Open...` highlighted (the user pressed Alt+F, or clicked
`File` and moved onto the item): the `File` entry `(0, 0, 48, 24)` fills `91C9F7`, text
`000000`. Widest label `Save As...` is 10 cells, width `max(120, 80 + 48) = 128`; height
`6 + 72 + 9 = 87`.

| Element | Rectangle (x, y, w, h) | Pixels |
|---|---|---|
| drop-down | 0, 24, 128, 87 | fill `F0F0F0`, 1 px border `A0A0A0` (rows 24 and 110, columns 0 and 127) |
| `Open...`, highlighted | 1, 27, 126, 24 | fill `91C9F7`; text at (28, 31) `000000`; `O` underlined row 45, x 28..35 |
| `Save As...` | 1, 51, 126, 24 | text at (28, 55); `S` underlined row 69, x 28..35 |
| separator | 1, 75, 126, 9 | line row 79, x 8..119, `D7D7D7` |
| `Exit` | 1, 84, 126, 24 | text at (28, 88); `x` underlined row 102, x 36..43 |
| bottom padding | 1, 108, 126, 2 | `F0F0F0` |

Enter, or a release over `Open...`, closes the menu, invalidates `(0, 24, 128, 87)` (the
program repaints the EDIT's top rows and the client), and posts `WM_COMMAND` 101; the program
then calls `GetOpenFileNameW` with the filter `Text files (*.txt)\0*.txt\0` and the picker opens
as a `Dialog` titled `Open`, 562x430 outside, centred on the window, laid out as section 7's
table with `C:\home\root` in the path edit, the folder's entries in the list (folders first,
each with the two-rect mark at x 20), `Text files (*.txt)` at (16, 362) in `text.dim`, the file
name edit focused with its ring at `(106, 316, 436, 24)` and the caret at (108, 320), `Open`
in `button.primary` with `Open` at (480, 362) in `icon.dark`, `Cancel` in `button.bg` with its
label at (376, 362) in `text`.

## 9. Tests

Host tests hash frames A, B and C and the picker at 560x400 and check their tables pixel by
pixel (the 2 px default border, the dotted rect's parity, the caret column, the tick's squares,
the separator row, the folder marks); every state row of every table in both palettes
(including the Dark picker's focus rings and `INSET_R4` corners); the edit's cell scrolling
(a 40-glyph text in a 24-cell edit: caret at the end gives `o = 17`; Home gives 0); the list's
`top` after Down past row 6, PageDown, End, and a click on the partial row; the indicator's
thumb size and position for 30 items in 7 rows; the menu's width and height rules, the shift
at the content's right edge and the clip at its bottom; each row of the input table, including
the injected `FocusOut`/`FocusIn` pair leaving the menu and the caret as they were; the
picker's listing order, hidden files, the filter, the typed-folder rule, the two message box
texts and the returned `lpstrFile` fields. The QEMU boot test's Windows program shows frame A,
then frame C, then the picker, and each `screendump` is compared with the host's render.

## 10. COMBOBOX

Palette rows added to section 0's table (Light values are Windows 10's scroll bar and combo
constants; Dark reuses the toolkit's tokens):

| Role | Light | Dark |
|---|---|---|
| `scroll.arrow` / `scroll.arrow.disabled` | `606060` / `BFBFBF` | `text.dim` / `button.border` |
| `scroll.arrow.hover.fill` / `scroll.arrow.pressed.fill` / `.pressed.glyph` | `DADADA` / `606060` / `FFFFFF` | `button.hover` / `button.pressed` / `text` |
| `scroll.thumb.hover` / `scroll.thumb.pressed` | `A6A6A6` / `606060` | `text.dim` / `text` |
| `combo.focus.fill` / `combo.focus.text` | `highlight` / `highlight.text` | `selection` / `text` |

Chevrons, drawn with `pixel` in the text colour of the moment: `CHEVRON_DOWN` is 8x4, rows
`##....##`, `.##..##.`, `..####..`, `...##...`; `CHEVRON_UP` is it flipped; `CHEVRON_RIGHT` is
4x8, rows `#...`, `##..`, `.##.`, `..##`, `..##`, `.##.`, `##..`, `#...`; `CHEVRON_LEFT` is it
mirrored. The same four serve the scroll bars (section 14) and submenus (section 12).

**`CBS_DROPDOWNLIST`.** The program's rect is `(x, y, w, h)`; the **closed box** is always
`(x, y, w, 24)` (`GetWindowRect`/`GetClientRect` report 24 tall; `CB_GETITEMHEIGHT` answers 22
for -1 and 20 for an item; `CB_SETITEMHEIGHT` is accepted and ignored); `h` only bounds the
list. The closed box has the push button's look: fill `button.fill`, 1 px `button.border`
(Light square, Dark `INSET_R4`); the selected item's text at `x + 4`, cell top `y + 4`, room
`w - 25` (cut with `...`), nothing when no item is selected; the arrow area is the rightmost
16 px inside the border, `(x + w - 17, y + 1, 16, 22)`, not filled separately, with
`CHEVRON_DOWN` at `(x + w - 13, y + 10)`. The whole closed box is one hit target.

| State | Light fill / border / text and chevron | Dark |
|---|---|---|
| plain | `E1E1E1` / `ADADAD` / `000000` | `button.bg` / `button.border` / `text` |
| hovered | `E5F1FB` / `0078D7` / `000000` | `button.hover` / `text.dim` / `text` |
| open, or pressed | `CCE4F7` / `005499` / `000000` | `button.pressed` / `text.dim` / `text` |
| disabled | `CCCCCC` / `BFBFBF` / `6D6D6D` | `button.bg` / `button.border` / `text.dim` |
| focused, closed | plus the text area `(x + 3, y + 3, w - 20, 18)` filled `combo.focus.fill`, text `combo.focus.text`, the dotted rect on that area's outline in `combo.focus.text` | plus the area filled `selection` and the ring at inset 2 of the box |

**The list.** `rows = min(count, max(1, min(30, (h - 26) / 20)))`; `CB_SETMINVISIBLE n`
replaces `min(30, (h - 26) / 20)` by `max(n, (h - 26) / 20)`. `list_h = 2 + 20 rows`;
`list_w = max(w, CB_SETDROPPEDWIDTH's value)`. It opens **below** at `(x, y + 24, list_w,
list_h)` when `y + 24 + list_h <= content_h`; else **above** at `(x, y - list_h, ...)` when
`y - list_h >= 0`; else below, clipped at the content's bottom. Its left edge is shifted left
so its right edge is at most the content width (never below 0). It is drawn in the top-level
window's buffer over everything, exactly as a menu drop-down (D145): opening and moving the
highlight commit its own rect, a paint while it is open draws it again, closing invalidates
the rect under it. Inside: 1 px `field.border`, fill `field.fill` (the `WM_CTLCOLORLISTBOX`
answer, `lParam` the combo), rows as section 5 from the inner top: 20 tall, text at `+4`, cell
top `+2`, cut with `...`; the **highlighted row** (the row under the pointer, else the selected
item) is `highlight`/`highlight.text`; no dotted rect in the list. When `count > rows` a
vertical scroll bar (section 14) sits inside the border at the right, rows end 16 px before
it, and section 5's `top` rules keep the highlighted row in view (on opening, the selected
item). `CB_GETDROPPEDCONTROLRECT` answers the rect the list would open at, in screen
coordinates; `CB_GETDROPPEDSTATE` whether it is open.

| Input (closed, enabled) | Does |
|---|---|
| button-down anywhere in the box | focuses it, `CBN_DROPDOWN`, opens the list (`CBN_SETFOCUS` first if it was not focused) |
| Alt+Down, Alt+Up, F4 | opens (or closes with cancel, below) |
| Up / Left, Down / Right | the previous / next item (clamped); Home / End the first / last; PageUp / PageDown by `max(1, rows - 1)`; a printable key the next item after the current, wrapping, whose first glyph matches case-insensitively. Each change: `CBN_SELENDOK`, `CBN_SELCHANGE` |
| Tab, Enter, Esc | to `IsDialogMessageW` (section 1) |

| Input (open) | Does |
|---|---|
| pointer over a row | highlights it (the pointer only, no selection change); over the scroll bar: section 14 |
| release over a row (after the opening press, a drag, or a new click) | selects it, closes: **`CBN_SELENDOK`, `CBN_CLOSEUP`, `CBN_SELCHANGE`** (the selection is already set when the first is sent, so a program handling any one of them reads the new item) |
| Up / Down, Home / End, PageUp / PageDown | move the highlight (clamped, scrolled into view) |
| Enter, or a printable key then Enter | selects the highlighted row, closes, the three notifications above |
| Esc, F4, Alt+Up, a button-down on the closed box, a button-down outside the list and box (not delivered to the program), a `FocusOut` not followed by `FocusIn` in the same read | closes with the selection as it was at opening: `CBN_SELENDCANCEL`, `CBN_CLOSEUP` |

While the list is open no key reaches the program and pointer events over the client are
consumed, as a menu. Disabling or hiding the combo closes it with cancel. Pointer: `Arrow`.

**`CBS_DROPDOWN`**: the closed box is section 3's single-line EDIT (`field.*` fill and
border, hover and focus borders, caret, selection of section 13, inner text `x + 4 .. x + w -
21`) with the arrow area `(x + w - 17, y + 1, 16, 22)` filled `button.fill`, a 1 px
`button.border` column at its left, the chevron, hover and pressed as the button's rows. Only
the arrow, F4 and Alt+Down/Up open the list; typing sends `CBN_EDITUPDATE` then
`CBN_EDITCHANGE` and never changes the selection; choosing a row (as above) sets the edit's
text to the item and selects it all; closed arrows change the selection and the text. Pointer
`IBeam` over the edit, `Arrow` over the arrow. **`CBS_SIMPLE`**: the edit at `(x, y, w, 24)`,
the list always shown at `(x, y + 24, w, h - 24)` as a section 5 LISTBOX inside the control's
rect, no arrow, no `CBN_DROPDOWN`/`CBN_CLOSEUP`; a click on a row selects it and sets the text
(`CBN_SELCHANGE`), a double-click adds `CBN_DBLCLK`.

Messages: `CB_ADDSTRING` (sorted with `CBS_SORT` as `LBS_SORT`), `CB_INSERTSTRING`,
`CB_DELETESTRING`, `CB_RESETCONTENT`, `CB_GETCOUNT`, `CB_GETCURSEL`, `CB_SETCURSEL` (no
notification; -1 clears), `CB_GETLBTEXT`, `CB_GETLBTEXTLEN`, `CB_FINDSTRING`,
`CB_FINDSTRINGEXACT`, `CB_SELECTSTRING`, `CB_SHOWDROPDOWN`, `CB_GETITEMDATA`/`SETITEMDATA`,
`CB_LIMITTEXT`, `CB_GETEDITSEL`/`SETEDITSEL`, `CB_GETMINVISIBLE`, `WM_GETTEXT` (the shown
text). `CBS_OWNERDRAW*` without `CBS_HASSTRINGS` draws blank rows (listed); `CBS_NOINTEGRALHEIGHT`,
`CBS_DISABLENOSCROLL`, `CBS_UPPERCASE`/`LOWERCASE`, `CBS_OEMCONVERT` have no effect (listed);
the wheel is out of scope as in section 5 (listed).

## 11. Program-made dialog windows and fonts

**11.1 The window.** A top-level window created with `WS_POPUP` and a non-null `hWndParent`
is **owned**, not a child (Win32's rule). It is a server window of kind `Dialog` of the owner's
top-level window (`GetAncestor(owner, GA_ROOT)`), whatever its procedure: the owner takes no
input while it is shown (the server's modal rule, part 1 section 1.1) and keeps its look, with
the chrome the server draws for an inactive window. Chrome: close control only; `WS_THICKFRAME`
and `WS_MAXIMIZEBOX` are ignored (`FIXED_SIZE`); without `WS_SYSMENU` the close control is
drawn `button.inactive` (`NO_CLOSE`) and `CloseRequest` is ignored. `DS_MODALFRAME` and
`WS_DLGFRAME` add nothing: `AdjustWindowRectEx` gives (1, 29, 1, 1) as for any captioned
window (D129). Title: the window text, as the message box. Placement: `CW_USEDEFAULT` lets the
server centre it on its owner; a given position, at creation or by `SetWindowPos`/`MoveWindow`,
is sent as `SetPosition` and clamped by the server into the work area. A `WS_POPUP` window
without an owner, or whose owner is not shown, is a `Normal` `FIXED_SIZE` window centred in
the work area (a captionless one still gets the chrome: listed). Destroying it returns focus
to the owner (the server's rule); `EnableWindow(owner, TRUE)` is the program's business and
changes nothing visible. A modeless owned dialog therefore blocks its owner too (listed gap;
the first apps use modal ones).

**11.2 `DefDlgProcA`/`W`** (a class whose procedure is it, `cbWndExtra >= DLGWINDOWEXTRA`,
and every template dialog of 11.4): `WM_ERASEBKGND` sends `WM_CTLCOLORDLG` to the dialog
procedure (`DWLP_DLGPROC`, when set) and fills the client with its brush, else with
`COLOR_BTNFACE` (`surface`), **ignoring the class brush** (Pegs' classes carry
`COLOR_BACKGROUND + 1`, black, and are grey on Windows for this reason). When the window is
first shown and no control has the focus, the first enabled `WS_TABSTOP` child in creation
order gets it (an edit selects all its text); `WM_SETFOCUS` on the dialog returns the focus to
the last focused control; `WM_NEXTDLGCTL` moves it; `DM_SETDEFID`/`DM_GETDEFID` keep the
default button's id (initially the `BS_DEFPUSHBUTTON` one; section 2's default rule applies);
`WM_CLOSE` sends itself `WM_COMMAND` `IDCANCEL`; `IsDialogMessageW` (section 1) gives Tab,
arrows, Enter to `DM_GETDEFID`'s button as `BN_CLICKED`, Esc as `IDCANCEL`, and Alt+mnemonic.
`WM_INITDIALOG` and `WM_COMMAND` go to the dialog procedure first, as Win32.

**11.3 One font, honest metrics.** Every `HFONT` is Egg Mono 8x16 (part 1); `WM_SETFONT` is
stored (`WM_GETFONT` returns it) and changes nothing drawn. So that program layouts fit what
we draw, every font reports Egg Mono's real metrics: `GetTextMetricsA`/`W` give `tmHeight 16`,
`tmAscent 13`, `tmDescent 3`, `tmInternalLeading 0`, `tmExternalLeading 0`, `tmAveCharWidth 8`,
`tmMaxCharWidth 8`, `tmWeight 400`, `tmOverhang 0`, `tmDigitizedAspectX`/`Y 96`,
`tmFirstChar 0x20`, `tmLastChar 0xFFFD`, `tmDefaultChar '?'`, `tmBreakChar ' '`, `tmItalic`,
`tmUnderlined`, `tmStruckOut 0`, `tmPitchAndFamily FF_MODERN` (low bits 0: fixed pitch),
`tmCharSet ANSI_CHARSET`; `GetTextExtentPoint32A`/`W` and `GetTextExtentExPoint` give `(8n,
16)` for `n` units (a surrogate pair is one cell); `GetTextFaceW` `Egg Mono`;
`GetDeviceCaps(LOGPIXELSX/Y)` 96; `GetDialogBaseUnits` `MAKELONG(8, 16)`. Pegs' `height =
tmAscent + tmDescent` is 16 and its `width = tmAveCharWidth` 8, so its rows are 24 tall and
its buttons 28, the brief's own numbers.

**11.4 Resource templates** (`DialogBoxParamW`, `DialogBoxIndirectParamW`,
`CreateDialogParamW`, `CreateDialogIndirectParamW`): `DLGTEMPLATE` and `DLGTEMPLATEEX` from
`RT_DIALOG`; dialog units to pixels with our base units, `px_x = 2 du`, `px_y = 2 du`
(`MapDialogRect` the same), so a template reckoned at 4 DLU per character gets 8 px cells;
`DS_SETFONT`/`DS_SHELLFONT` fonts ignored; item classes by atom (`0x80` BUTTON, `0x81` EDIT,
`0x82` STATIC, `0x83` LISTBOX, `0x84` SCROLLBAR, `0x85` COMBOBOX) or name; `DS_CENTER` and
`CW_USEDEFAULT` centre on the owner; the window is 11.1's with `WS_POPUP | WS_CAPTION |
WS_SYSMENU` added as Win32 does when the template has no `WS_CHILD`. `DialogBox*` runs the
modal loop the picker uses (D145), dispatching the thread's other messages, until `EndDialog`
(its result returned); `WM_INITDIALOG`'s return `TRUE` lets 11.2's first-control focus apply.
`DS_CONTROL`, `DS_ABSALIGN`, help ids and `WS_CHILD` templates are out of scope (listed).

## 12. Menu additions

**Submenus.** An `MF_POPUP` item inside a drop-down (replacing section 6's "draws as a disabled
item") draws its label as any item plus `CHEVRON_RIGHT` at `(dx + w - 16, iy + 8)` in the item's
text colour; `MF_GRAYED` greys both. Highlighting it (pointer, Up/Down, or a mnemonic) opens its
**cascade** at once, no delay, after `WM_INITMENUPOPUP` for the submenu: a drop-down laid out by
section 6's rules with its left edge at `dx + w` and its top at `iy - 3` (so its first item
aligns with the parent item); when `dx + w + cw > content_w` it opens to the left, right edge
at `dx` (never below 0, then at 0); taller than the room below: clipped as section 6, no
shifting up. Highlighting another item of the parent closes it; the parent item stays
highlighted while the pointer is in the cascade. Keys: Right on a submenu item opens it with
its first enabled item highlighted (Enter and Space the same); Left in a cascade closes it and
leaves the parent item highlighted; Right on an ordinary item and Left in a top-level
drop-down change the bar entry as section 6; Esc closes the innermost cascade only. At most 4
levels (deeper ones draw as disabled items, listed).

**Marks, confirmed and added.** `MF_CHECKED`: the tick at box origin `(dx + 7, iy + 5)`
(section 6, confirmed). `MFT_RADIOCHECK` with `MF_CHECKED` (`CheckMenuRadioItem`,
`SetMenuItemInfo`): a 6 px `INSET_R3` dot at `(dx + 11, iy + 9)` in the text colour instead of
the tick. `MF_GRAYED`/`MF_DISABLED` and separators: section 6, confirmed. `MF_BITMAP`,
`MF_OWNERDRAW`, `MFS_DEFAULT` (no bold), `MF_MENUBARBREAK`/`MF_MENUBREAK` and `hbmpItem` have no
effect (listed). `ModifyMenuW`, `InsertMenuW`, `InsertMenuItemW`, `SetMenuItemInfoW`,
`GetMenuItemInfoW`, `DeleteMenu`, `RemoveMenu`, `GetSubMenu`, `GetMenuItemCount`,
`GetMenuItemID`, `GetMenuState`, `GetMenuStringW`, `DrawMenuBar` as Win32; a bar change while
a menu is open closes it.

**Accelerator text.** A label's part after `\t` is drawn right-aligned, its last cell ending
at `dx + w - 20`, in the item's text colour; `widest` (section 6) counts `label + 3 +
accelerator` cells for such an item, so at least 24 px separate the two. `\b` in a label is
dropped (listed).

**`TrackPopupMenu`/`Ex`** (`hMenu`, flags, `x`, `y`, `hwnd`): a context menu. `x`, `y` are
screen coordinates, converted to `hwnd`'s content; the drop-down (section 6's layout, no bar)
opens with its top-left there (`TPM_RIGHTALIGN`, `TPM_CENTERALIGN`, `TPM_BOTTOMALIGN`,
`TPM_VCENTERALIGN` move the anchor as named), shifted left into the content, and placed above
the point when it would pass the content's bottom and fits above, else clipped; it is drawn in
`hwnd`'s top-level buffer like every drop-down (clients cannot open popups, D127), so it never
leaves the window. No item is highlighted at first. Pointer and keys as a drop-down; Left and
Right do nothing (no bar); `TPM_RIGHTBUTTON` lets the right button's release activate too. The
call blocks until the menu closes, running the modal loop that dispatches the thread's other
messages; with `TPM_RETURNCMD` it returns the id (0 for none), else it posts `WM_COMMAND` and
returns nonzero. `TPM_NONOTIFY`, `TPM_RECURSE`, `TPM_LAYOUTRTL` and `lptpm` are ignored
(listed). `DefWindowProcW` turns `WM_RBUTTONUP` into `WM_CONTEXTMENU` (`lParam` screen
coordinates), as Win32; the right button's down and up are delivered like the left's.

**Keys, confirmed.** Section 6's table stands (F10, Alt alone, Alt+mnemonic, Left/Right, Up/Down,
Enter/Space, mnemonics, Esc), with one precision: Alt+mnemonic and F10 act only while the
window is enabled and no menu or combo list is open.

**Keyboard accelerators.** `LoadAcceleratorsW`, `CreateAcceleratorTableW`,
`DestroyAcceleratorTable`, `TranslateAcceleratorW(hwnd, hAccel, &msg)`: on a `WM_KEYDOWN`/
`WM_SYSKEYDOWN` (`FVIRTKEY` with `FCONTROL`, `FSHIFT`, `FALT` matched exactly) or a `WM_CHAR`
(an entry without `FVIRTKEY`) whose key matches, it sends `WM_COMMAND` (`LOWORD` the id,
`HIWORD` 1) to `hwnd` and returns nonzero, unless `hwnd` is disabled or the id names a menu item
of its bar that is grayed or disabled (then nothing, nonzero still). It needs no menu open and
has no visual; while a menu, combo list or context menu is open the key never reaches the loop.

## 13. EDIT: selection and `ES_MULTILINE`

**Selection, every EDIT** (replacing section 3's "caret only"): an anchor and the caret
(`EM_SETSEL`, `EM_GETSEL`; the caret is the active end and stays visible). Selected cells are
filled `highlight` with glyphs in `highlight.text`, shown while the edit is focused (always
with `ES_NOHIDESEL`). Typing replaces the selection; Backspace and Delete delete it; Shift +
Left/Right/Home/End extend it; Ctrl+Left/Right move by words (a word is a run of letters,
digits and `_`, or a run of other glyphs; spaces are skipped), with Shift extending; Ctrl+A
selects all; `EM_REPLACESEL` replaces it. Tab into an edit by `IsDialogMessageW` selects all; a
click does not. Pointer: button-down puts the caret at the nearest cell boundary and the
anchor there, capturing; dragging moves the caret (auto-scrolling by section 3's rules at the
edges); Shift+button-down moves only the caret; a double-click (section 17) selects the word
under it. No clipboard, no undo: Ctrl+C/X/V/Z and `WM_COPY`/`WM_CUT`/`WM_PASTE`/`WM_UNDO` do
nothing (listed).

**`ES_MULTILINE`.** Fill and border as section 3; text from `(x + 4, y + 2)`, lines 16 apart,
clipped to the rect inset 1; `R = (h - 4) / 16` whole lines visible, the partial one drawn
clipped; the inner width ends 4 px before the right edge, 16 more while a vertical scroll bar
shows. Without `ES_AUTOHSCROLL` lines **wrap** at the inner width with part 1's greedy rule (a
word longer than the width is cut); with it they do not, and the view scrolls horizontally in
cells as section 3 keeps the caret visible. A selected line end fills one extra cell after the
line's last glyph (an empty selected line shows one 8x16 block). Keys: Up/Down keep the
column (clamped to the line's length), Home/End the line's ends, Ctrl+Home/End the text's,
PageUp/PageDown by `R - 1` lines, Enter inserts `\r\n` (always outside `IsDialogMessageW`; in a
dialog only with `ES_WANTRETURN`, else Enter goes to the default button), Tab inserts a tab
drawn as one cell (in a dialog Tab moves the focus). The first visible line `f` follows
section 5's `top` rule for the caret's line. `WS_VSCROLL`/`WS_HSCROLL` give section 14's bars
inside the border (always shown, disabled while nothing scrolls), fed by (lines, `R`, `f`) and
(widest line's cells, visible cells, `o`); `ES_AUTOVSCROLL` without `WS_VSCROLL` scrolls
without a bar. Messages: `EM_GETLINECOUNT`, `EM_LINEINDEX`, `EM_LINELENGTH`, `EM_GETLINE`,
`EM_LINEFROMCHAR`, `EM_LINESCROLL`, `EM_SCROLLCARET`, `EM_GETFIRSTVISIBLELINE`,
`EM_SETREADONLY`, `EM_GETMODIFY`/`SETMODIFY`; `WM_GETTEXT` returns `\r\n` line ends; a lone
`\n` or `\r` set by `WM_SETTEXT` is kept as given and ends a line. `EM_FMTLINES`, `EM_SETTABSTOPS`,
`EM_SETMARGINS`, `EM_SETRECT` and `ES_CENTER`/`ES_RIGHT` have no effect (listed).

## 14. Scroll bars

**Width: 16 px, not Windows' 17.** `SM_CXVSCROLL`, `SM_CYHSCROLL`, `SM_CXHSCROLL`, `SM_CYVSCROLL`,
`SM_CXHTHUMB` and `SM_CYVTHUMB` all report 16, so program layouts that subtract them fit. 16 is
two cells and the cell's height: the arrow buttons are 16x16 squares, the chevrons centre
without a half pixel, the whole bar doubles cleanly at 2x, and a bar 1 px narrower than
Windows' reads as the same thing. The one look serves the `SCROLLBAR` control (`SBS_VERT`,
`SBS_HORZ`), window scroll bars (`WS_VSCROLL`/`WS_HSCROLL` on any window) and the bars inside a
LISTBOX, a multi-line EDIT and the combo's list.

**Geometry**, for a bar of length `L` (its rect's height when vertical, width when
horizontal; the other dimension 16): an arrow button 16 long at each end, the track between
them, `track = L - 32` (when `L < 32` the two arrows are `L / 2` each and there is no track).
Fill `scroll.track` everywhere first. Chevrons: `CHEVRON_UP`/`DOWN` at `(4, 6)` of the button,
`CHEVRON_LEFT`/`RIGHT` at `(6, 4)`, in `scroll.arrow`. From `SCROLLINFO` (`nMin`, `nMax`, `nPage`,
`nPos`): `range = nMax - nMin + 1`; the bar **scrolls** when `nPage > 0` and `nPage < range`;
then `thumb = min(track, max(16, track * nPage / range))`, `pos_max = nMax - nPage + 1`, `nPos`
clamped to `[nMin, pos_max]`, the thumb's start at `16 + (track - thumb) * (nPos - nMin) /
(pos_max - nMin)`, drawn `scroll.thumb` across the bar's full 16 px. Not scrolling: no thumb,
both chevrons `scroll.arrow.disabled` (the disabled look; also an `EnableScrollBar`-disabled bar
and a disabled control). `SetScrollInfo` redraws the bar at once; nothing else moves the thumb
except a drag, below.

| Part | plain | hovered | pressed (held) |
|---|---|---|---|
| arrow button | `scroll.track` / `scroll.arrow` | `scroll.arrow.hover.fill` / `scroll.arrow` | `scroll.arrow.pressed.fill` / `scroll.arrow.pressed.glyph` |
| thumb | `scroll.thumb` | `scroll.thumb.hover` | `scroll.thumb.pressed` |
| track | `scroll.track` | unchanged | unchanged |

**Input** sends `WM_VSCROLL`/`WM_HSCROLL` to the control's parent (`lParam` the control) or to
the window itself (`lParam` 0); a LISTBOX, a multi-line EDIT and a combo list handle their own
and update their `SCROLLINFO` themselves. Arrow button-down: `SB_LINEUP`/`SB_LINEDOWN`
(`SB_LINELEFT`/`RIGHT`) at once, again after 250 ms, then every 50 ms while held with the
pointer on the button; release: `SB_ENDSCROLL`. Track button-down before the thumb:
`SB_PAGEUP`, after it: `SB_PAGEDOWN`, the same repeat timing while held and the pointer is
still beyond the thumb (so paging stops when the thumb reaches it). Thumb button-down: the
thumb follows the pointer, keeping the grab offset, `pos = nMin + (start - 16) * (pos_max -
nMin) / (track - thumb)` rounded to nearest, `SB_THUMBTRACK` with the position (16 bits in
`HIWORD`; `GetScrollInfo` with `SIF_TRACKPOS` gives it whole) on every change; release:
`SB_THUMBPOSITION` then `SB_ENDSCROLL`, and the thumb returns to `SCROLLINFO`'s `nPos` (a program
that ignores the message sees it snap back, as on Windows). Keys on a focused `SCROLLBAR`
control: Up/Left, Down/Right line, PageUp/PageDown, Home `SB_TOP`, End `SB_BOTTOM`; its focus
indicator is the dotted rect on the thumb's outline (Dark: the ring at inset 0 of the thumb).
Pointer: `Arrow`. The wheel is out of scope (listed).

**Window scroll bars** sit inside the content at the right edge (below the menu bar) and the
bottom edge; `GetClientRect` and `WM_SIZE` exclude them (`AdjustWindowRectEx` never adds them,
as Win32). When both show, the 16x16 **size box** at the corner is `scroll.track` and inert
(`SBS_SIZEGRIP`/`SBS_SIZEBOX` draw the same, listed). `ShowScrollBar` adds or removes a bar
(the client resizes, `WM_SIZE`); `SetScrollInfo` whose values make the bar unnecessary removes
it unless `SIF_DISABLENOSCROLL` (then it shows disabled). A LISTBOX with `WS_VSCROLL` draws
this bar inside its border, rows ending 16 px before it, only while `count > V` (always, disabled,
with `LBS_DISABLENOSCROLL`), replacing section 5's 8 px indicator; a LISTBOX without
`WS_VSCROLL` keeps the indicator (EggOS's choice, section 5; the picker is unchanged). A
`SCROLLBAR` control is exactly its rect; `SBS_TOPALIGN` and the other alignment styles are
ignored (listed).

## 15. Radio buttons and group boxes, confirmed

Section 2's radio button and group box stand. Added for the keyboard, under `IsDialogMessageW`:
Down and Right select the next enabled radio of the group (from the previous `WS_GROUP` control
to the next; wrapping within the group), Up and Left the previous, moving the focus with the
selection and sending `BN_CLICKED` as a click would (`BS_AUTORADIOBUTTON`; a plain
`BS_RADIOBUTTON` moves only the focus); Tab into a group lands on its selected radio, else its
first enabled one; without `IsDialogMessageW` the arrows reach the focused radio and do
nothing. A group box's `WS_GROUP` starts a group like any control's.

## 16. Pointer shapes

Part 1's table stands with two changes: `SetCursor(NULL)`, a class with `hCursor NULL`, and
`ShowCursor(FALSE)` taking the count below 0, now map to the server's **`Hidden`** shape (6,
D172) over that window; a program's own cursor resources (`LoadCursorW` with a module handle,
`LoadImageW`, `CreateCursor`) still show `Arrow`. So: `IDC_ARROW`, `IDC_SIZEALL`, `IDC_WAIT`,
`IDC_APPSTARTING`, `IDC_HAND`, `IDC_CROSS`, `IDC_NO`, `IDC_HELP`, `IDC_UPARROW` to `Arrow`;
`IDC_IBEAM` to `IBeam`; `IDC_SIZENS`, `IDC_SIZEWE`, `IDC_SIZENWSE`, `IDC_SIZENESW` to the four
resizes; null to `Hidden`. Over a control the control's pointer wins (sections 2-5, 10, 14);
`WM_SETCURSOR` goes to the control's parent first, as Win32, and a program that answers it
`TRUE` after its own `SetCursor` keeps its shape.

## 17. Double clicks

A second left button-down on the same window within 400 ms (`GetDoubleClickTime`) and 4 px
(`SM_CXDOUBLECLK`, `SM_CYDOUBLECLK`) of the first arrives as `WM_LBUTTONDBLCLK` instead of
`WM_LBUTTONDOWN` when the class has `CS_DBLCLKS` (the third is a plain down again); without the
style every down is `WM_LBUTTONDOWN`. The system classes have `CS_DBLCLKS`: a LISTBOX row sends
`LBN_DBLCLK` (section 5, confirmed; it stays selected), an EDIT selects the word under the
pointer (section 13), a `CBS_SIMPLE` list sends `CBN_DBLCLK`, a drop-down list's row acts as a
click (it already closed), a STATIC with `SS_NOTIFY` sends `STN_DBLCLK`, and a BUTTON treats it
as a press (no `BN_DOUBLECLICKED`, listed). The right button follows the same rule with
`WM_RBUTTONDBLCLK`.

## 18. Worked example: Pegs

`pegs.exe` sizes its main window to the board with `AdjustWindowRectEx` (menu: `+24`) and
`SetWindowPos`, paints the board from a memory bitmap by `BitBlt` in `WM_PAINT`, and builds
`Game`, `Type` and `Help` in code; `Type` lists nine presets (`Cross 5x7` to `Random 9x9`,
no separator) and `Custom...`, the current preset ticked by `CheckMenuItem`; `Game`'s `Copy`
is greyed by `EnableMenuItem` while the board cannot be copied (checked against the binary
when it was implemented, D174). With our
metrics `height = 16`, `width = 8`. The control rects below are the program's arithmetic as
read from its source; the brief fixes what each control draws for any rect, so the engineer
checks them against the binary's `CreateWindowEx` calls.

**About** (`Help > About`): three centred STATIC lines `Pegs` (4 cells), `from Simon Tatham's
Portable Puzzle Collection` (46 cells, 368 px, the widest) and a version line (24 cells
assumed), `winwidth = 368 + 4 * 8 = 400`, `winheight = 8 + 3 * (24 + 8) + 16 + 28 = 148`;
outside **402x178**, a `Dialog` of the main window centred on it, title `About Pegs`, close
control only. The owner's board stays as painted and takes no input.

| Element | Rectangle (x, y, w, h) | Pixels |
|---|---|---|
| surface | 0, 0, 400, 148 | `F0F0F0` (`DefDlgProc`, 11.2) |
| STATIC `Pegs`, `SS_CENTER` | 16, 8, 368, 24 | fill `F0F0F0`; text at (184, 8) `000000` |
| STATIC line 2 | 16, 40, 368, 24 | text at (16, 40) |
| STATIC version line | 16, 72, 368, 24 | text at (104, 72) |
| `OK`, default, focused | 16, 112, 368, 28 | fill `E1E1E1`; 2 px border `0078D7`; `OK` at (192, 118); dotted rect on the outline of (19, 115, 362, 22) |
| bottom padding | 0, 140, 400, 8 | `F0F0F0` |

Enter, Space, a click on `OK`, Esc or the close control send `WM_COMMAND` `IDOK`/`IDCANCEL`;
the program re-enables the owner and destroys the dialog, and the board window is focused again.

**Custom** (`Type > Custom...`): rows `Width` (EDIT), `Height` (EDIT), `Board type`
(COMBOBOX, `CBS_DROPDOWNLIST | CBS_HASSTRINGS`, items `Cross`, `Octagon`, `Random`), all with
`WS_EX_CLIENTEDGE` (which adds nothing to section 3's and 10's borders and takes no space).
`maxlabel` 80 (`Board type`), `col1l 16`, `col1r 96`, `col2l 112`, `col2r 352`, `winwidth
368`; `winheight = 8 + 3 * 32 + 8 + 28 + 8 = 148`; outside **370x178**, `Dialog`, title
`Custom`.

| Element | Rectangle (x, y, w, h) | Pixels |
|---|---|---|
| STATIC `Width` | 16, 8, 80, 24 | text at (16, 8) |
| EDIT `7`, focused on entry, all selected | 112, 8, 240, 24 | fill `FFFFFF`, border `0078D7`; cell (116, 12) filled `0078D7`, `7` in `FFFFFF`; caret (124, 12, 1, 16) |
| STATIC `Height` / EDIT `7` | 16, 40, 80, 24 / 112, 40, 240, 24 | border `7A7A7A`; `7` at (116, 44) |
| STATIC `Board type` | 16, 72, 80, 24 | text at (16, 72) |
| COMBOBOX, given (112, 72, 240, 328), closed, `Cross` | 112, 72, 240, 24 | fill `E1E1E1`, border `ADADAD`; `Cross` at (116, 76); chevron at (339, 82), rows 82-85 |
| `OK`, default | 16, 112, 160, 28 | 2 px border `0078D7`; `OK` at (88, 118) |
| `Cancel` | 192, 112, 160, 28 | border `ADADAD`; `Cancel` at (248, 118) |

Opening the combo (a click, or Tab twice then Alt+Down): `rows = min(3, (328 - 26) / 20 =
15) = 3`, `list_h 62`; below would end at `96 + 62 = 158 > 148`, above fits (`72 - 62 = 10`),
so the list is `(112, 10, 240, 62)`: border `7A7A7A`, fill `FFFFFF`, rows `(113, 11, 238, 20)`
`Cross` highlighted `0078D7`/`FFFFFF` (text at (117, 13)), `(113, 31, ...)` `Octagon`, `(113,
51, ...)` `Random`, covering the `Width` and `Height` rows until it closes; the closed box
shows the open row (`CCE4F7`/`005499`). A release on `Random` selects it, closes, invalidates
`(112, 10, 240, 62)` (the dialog repaints the edits), and sends `CBN_SELENDOK`, `CBN_CLOSEUP`,
`CBN_SELCHANGE`. Focused and closed, the combo's text area `(115, 75, 220, 18)` is `0078D7`
with `Random` in `FFFFFF` and the dotted rect on its outline.

## 19. Tests

Host tests, pixel by pixel against the tables, in both palettes where a Dark column exists:
the combo's closed box in every state (the text area's focus fill, the chevron's 32 pixels,
the `...` cut at `w - 25`), `rows` for `h` 328 / 26 / 1000 with 3, 15 and 40 items and
`CB_SETMINVISIBLE`, the list below, above and clipped, its highlight following the pointer
without changing the selection, every row of both input tables with the exact notification
order and the restored selection on cancel, `CBS_DROPDOWN`'s edit plus arrow and `CBS_SIMPLE`;
an owned `WS_POPUP` window's kind, chrome, `AdjustWindowRectEx`, position and the owner's
blocked input; `DefDlgProc`'s grey erase over a black class brush, first-control focus,
`WM_CLOSE` to `IDCANCEL`; the TEXTMETRIC fields, `GetTextExtentPoint32` for 0, 1 and 46 units,
`GetDialogBaseUnits` and a 4-item template's pixel rects; the cascade's position right, flipped
left and clipped, its chevron, the radio dot, accelerator text right-aligned at `w - 20` and
the width rule, `TrackPopupMenu`'s four alignments and `TPM_RETURNCMD`, an accelerator table
with a grayed item; selection fill in single- and multi-line edits (an empty selected line's
block), word double-click, Shift+click, Ctrl+A, wrap vs `ES_AUTOHSCROLL`, PageDown, Enter's
`\r\n` and `WM_GETTEXT`; the scroll bar's thumb size and start for (`nMin 0, nMax 99, nPage
10, nPos 0 / 45 / 90`) in a 200 px bar, the 16 px minimum thumb, the disabled look at `nPage
>= range`, the repeat timing, the drag's `SB_THUMBTRACK` positions and the snap back, the size
box, a LISTBOX's bar replacing the indicator; radio arrows within a group; the `Hidden`
pointer; `WM_LBUTTONDBLCLK` at 399 and 401 ms. Section 18's About and Custom dialogs are hashed
at 400x148 and 368x148, the combo open above included. The QEMU boot test runs `pegs.exe`:
it opens `Help > About`, then `Type > Custom...`, opens the combo, chooses `Random`, presses
`OK`, and each `screendump` is compared with the host's render.
