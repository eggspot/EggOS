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
