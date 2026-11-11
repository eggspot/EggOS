# EggOS v0.5 Windows tooltips and notification-area icons: design brief

Design by Claude Fable (D16) for part 13's rest (`docs/roadmap.md`, M15): comctl32's tooltip
control (`tooltips_class32`) and the notification area (`Shell_NotifyIconW`). It extends
`windows-controls-v0.3.md`: every rule there stands (section 0's two palettes, section 1's
shared rules, section 20's common-control rules, section 29's element tree), and this brief
only adds, replacing exactly these sentences: section 20's "tooltips never exist", section 26's
"`TBSTYLE_TOOLTIPS` creates nothing", section 24's "item tooltips: no effect" and section 25's
`TVS_INFOTIP` in its no-effect list. Every number is in pixels at scale 1 and doubles at 2x;
the font inside a program is Egg Mono 8x16 (cell 8x16, the `...` and `?` rules of part 1), no
anti-aliasing, no alpha, no motion. The layer draws tooltips into the program's own top-level
window buffer, as it draws drop-downs and combo lists (clients cannot open popups, D127); the
notification area is the shell's, so the kernel's window server (`drivers/src/desktop`) draws
the icons from data the layer sends it (section 18 says what, not how: the engineer designs the
protocol).

## 0. What a person gets, and how that is checked

- **Tooltips in Windows programs**: rest the pointer on a toolbar button or any tool a program
  registered and a small white tile names it, as on Windows; it goes away the moment you press,
  type or leave. Balloons and titled tips look like the same tile with a heading. Everything
  stays inside the program's window.
- **Tray icons**: a Windows program's notification icon sits in the bar's indicator run, next
  to the network and keyboard indicators, in every desktop profile; hovering names it, a left
  click, a double click and a right click reach the program exactly as on Windows, its context
  menu opens above or below the icon in the shell's own menu chrome, and its balloons are
  EggOS notices. When the program exits, the icon is gone at once: no ghost icons.
- Host tests pin the pixels and the message sequences (sections 9 and 19); `wm show` lists
  every tooltip and every tray icon, so an agent reads them without hovering (D22).

## 1. Palette rows added

Added to section 0's table of `windows-controls-v0.3.md`. Light values are Windows 10's themed
tooltip constants; `GetSysColor(COLOR_INFOBK)` still answers `FFFFE1` and `COLOR_INFOTEXT`
`000000` (part 1's table, unchanged: programs that draw their own tips get the classic colours;
the layer's own tips are the themed ones, as Windows 10's are).

| Role | Light (inside a program) | Dark (EggOS dialogs) |
|---|---|---|
| `tip.fill` / `tip.border` / `tip.text` | `FFFFFF` / `767676` / `000000` | `menu.bg` `#1C2430` / `menu.border` `#2A3340` / `text` `#E6EAF0` |
| `tip.info` / `tip.warning` / `tip.error` | `0078D7` / `FFB900` / `E81123` | the same |
| `tip.icon.glyph` | `FFFFFF` | `icon.dark` `#0B0E13` |

Contrast: `tip.text` on `tip.fill` 21:1 in both palettes; `tip.border` on `FFFFFF` 4.5:1. High
contrast changes nothing in Light (section 0's Light values are fixed, part 1) and in Dark the
tokens take their high-contrast values (`first-run-accessibility-v0.3.md` 2.4: black fill,
white border and text), so nothing here branches on the contrast setting.

## 2. The tooltip control

`CreateWindowExW` of class `tooltips_class32` (`WS_POPUP`, usually `TTS_ALWAYSTIP`, `TTS_NOPREFIX`,
`TTS_BALLOON`, `TTS_NOANIMATE`, `TTS_NOFADE`, `TTS_CLOSE`) creates a **tooltip control**: a
window with no rect of its own (`GetWindowRect` answers the tip's last rect, or an empty rect at
(0, 0)), never a child in the `HWND` tree's painting (it is drawn as an overlay of the top-level
window of its owner, `hwndParent` or, when that is `NULL`, of the first tool's `hwnd`). One
control holds up to **64 tools** (`TTM_ADDTOOLW` past that returns `FALSE`; listed), each a
`TOOLINFOW`: `hwnd` (the owner window), `uId` (a tool id, or the child window's handle with
`TTF_IDISHWND`), `rect` (in `hwnd`'s client coordinates; ignored with `TTF_IDISHWND`, where the
tool is the child's whole window rect), `uFlags`, `lpszText`. A tool's **rect in content
coordinates** is `hwnd`'s client origin plus `rect`, or the child's rect, recomputed each time the
tip is placed (a moved control moves its tool). `TTS_NOPREFIX` clear strips `&` as a mnemonic
would (the glyph after it is not underlined: a tip has no mnemonics; `&&` is `&`); set, `&` is a
plain glyph. `TTS_ALWAYSTIP` clear: tips show only while the owner's top-level window is active
(section 6); set: in any window of the program that is visible. `TTS_NOANIMATE` and `TTS_NOFADE`
change nothing (nothing animates). Messages: `TTM_ADDTOOLW`, `TTM_DELTOOLW`, `TTM_NEWTOOLRECTW`,
`TTM_UPDATETIPTEXTW`, `TTM_SETTOOLINFOW`/`GETTOOLINFOW`, `TTM_GETTOOLCOUNT`, `TTM_ENUMTOOLSW`,
`TTM_HITTESTW`, `TTM_GETTEXTW` (the text, at most `cchTextMax - 1` UTF-16 units), `TTM_ACTIVATE`,
`TTM_RELAYEVENT`, `TTM_SETDELAYTIME`/`GETDELAYTIME`, `TTM_SETMAXTIPWIDTH`/`GETMAXTIPWIDTH`,
`TTM_SETTITLEW`/`GETTITLE`, `TTM_TRACKACTIVATE`, `TTM_TRACKPOSITION`, `TTM_POP`, `TTM_POPUP`,
`TTM_UPDATE`, `TTM_GETCURRENTTOOLW`, `TTM_GETBUBBLESIZE` (the rect of section 3 for that tool),
`TTM_ADJUSTRECT` (adds or removes the 8 px padding), `TTM_SETTIPBKCOLOR`/`SETTIPTEXTCOLOR`
(stored, returned by the getters, **not used for drawing**: one look per palette, as every
control; listed), `TTM_SETMARGIN`/`GETMARGIN` (stored, not used; listed), `TTM_SETWINDOWTHEME`
(ignored). The A messages are not registered (section 20: only the W codes exist).

**Text.** `lpszText` is a pointer to a string the layer copies at `TTM_ADDTOOLW` and
`TTM_UPDATETIPTEXTW` time (at most 255 UTF-16 units; longer is cut; listed), a resource id
(`HIWORD` 0, loaded from `hinst` as `LoadStringW` does), or `LPSTR_TEXTCALLBACKW`: then, right
before the tip is placed, the layer sends **`TTN_GETDISPINFOW`** (`WM_NOTIFY`, `NMTTDISPINFOW`:
`hdr.hwndFrom` the tooltip control, `hdr.idFrom` the tool's `uId`, `uFlags` the tool's flags,
`lParam` its `lParam`, `szText` zeroed, `lpszText` pointing at `szText`, `hinst` `NULL`) to the
tool's `hwnd`; the answer is `szText`, or `lpszText` if the program moved it (a resource id
with `hinst` works too); `TTF_DI_SETITEM` set stores the answer so the tool is not asked again.
An empty answer shows no tip. The text is in the tool's `hwnd`'s thread, so the notification is
sent as every `WM_NOTIFY` is (section 20).

## 3. Geometry and look

One function draws every tip, from the palette of the window it is drawn in (Light inside a
program; Dark only if an EggOS dialog of the layer ever registers tools, never in this version).

- **Lines.** `max_width` is `TTM_SETMAXTIPWIDTH`'s value, default -1. With -1 the text is **one
  line**: `\r\n` and `\n` draw as one space, no wrapping (Win32's rule: a tip wraps and breaks
  lines only once a maximum width is set). With `max_width >= 0`: `room = max(1, max_width / 8)`
  cells; the text breaks at `\n` (`\r` ignored) and wraps at spaces so no line passes `room`
  cells (a word longer than `room` is cut at `room`, the rest goes to the next line); at most
  **16 lines** (the rest is dropped; listed). `n` is the longest line's cells, `lines` the count.
- **Panel.** `w = 8n + 16`, `h = 8 + 16 * lines` (one line: 24, the bar's and the toolkit's
  tooltip height); a titled tip adds section 4's rows. Fill `tip.fill`, a 1 px `tip.border`
  inside the edge, corners **square in Light and `INSET_R4` in Dark** (section 0's one shape
  rule); `TTS_BALLOON` rounds both palettes `INSET_R4` (section 5). No shadow (nothing in a
  program's buffer has one). Text in `tip.text`: line `i`'s cell top at `y + 4 + 16i`, left at
  `x + 8`; no cutting (the panel is sized to the text), except that a panel wider than the
  content width is clipped by section 6's rule.
- **Minimum** `w` 24 (an empty line still shows a 24x24 panel only when the program forced it
  with `TTM_POPUP`; a tool with empty text never shows by hover).
- **2x**: everything doubles (a 48 px one-line tip, 16 px padding, 32 px cells).

| Tip | `w` x `h` | Example |
|---|---|---|
| `Save` | 48 x 24 | text at (x + 8, y + 4) |
| `Save (Ctrl+S)` | 120 x 24 | 13 cells |
| `Line one\nLine two`, `max_width` -1 | 152 x 24 | `Line one Line two`, 17 cells |
| the same, `max_width` 80 | 88 x 40 | `room` 10, two lines of 8 cells |
| `A long sentence that wraps`, `max_width` 96 | 96 x 56 | `A long`, `sentence`, `that wraps` (10 cells) |

## 4. Title and icon (`TTM_SETTITLEW`)

`wParam` the icon (`TTI_NONE` 0, `TTI_INFO` 1, `TTI_WARNING` 2, `TTI_ERROR` 3, their `_LARGE`
forms 4-6 drawn as the small ones, or an `HICON`), `lParam` the title (at most 99 UTF-16 units,
as Win32; longer fails with `FALSE`). A title makes every tip of the control a **titled tip**:

- a **title row** 16 tall at `y + 4`: the icon's 16x16 box at `x + 8` when there is one, then
  the title's cells from `x + 8` (no icon) or `x + 28` (icon, a 4 px gap) in `tip.text`;
- a 4 px gap, then the text lines from `y + 24` (`y + 4 + 16 + 4`);
- `h = 8 + 16 + 4 + 16 * lines` (one line of text: 44); `w = 8 + max(title_w, 8n) + 8`, where
  `title_w` is `8t` or `20 + 8t` with an icon, `t` the title's cells.

Windows draws the title bold; Egg Mono has one weight, so the row, the gap and the icon tell the
title from the text, as the toolkit's lists tell a heading by place. An empty title (`lParam`
`""`) with `TTI_NONE` removes the title row. The three icons, in a 16x16 box, drawn with the
existing primitives: the 14 px circle (`INSET_C14`) at (1, 1) in `tip.info`, `tip.warning` or
`tip.error`, and the glyph `i`, `!` or `x` in `tip.icon.glyph` with its cell at (4, 0) (an 8x16
cell centred in the box). Colour alone never tells them apart (three glyphs). An `HICON` is
drawn at 16x16 as section 27 draws an image-list icon (alpha ≥ 128 opaque, masked icons, a
smaller icon centred in the box, a larger one clipped).

## 5. Balloons (`TTS_BALLOON`)

**Decision: a balloon is the titled tip's panel with `INSET_R4` corners in both palettes, no
stem, anchored to the tool rather than the pointer.** EggOS draws no stems anywhere (the
toolkit's tooltips, the bar's tooltips and every popup are plain rounded panels), a stem would be
the one shape in the system that points, and what Windows' stem conveys (which control the
balloon is about) is conveyed here by placing the balloon on the tool, as a `TTF_CENTERTIP` tip
is. `TTS_CLOSE` (a close button in the title row) is not in this version: a balloon closes as
every tip does (section 6). Placement: section 7's centred rule, above the tool first (Windows'
balloons prefer above), below when no room above.

## 6. When a tip shows and hides

Delays, in ms, per control: `initial` (`TTDT_INITIAL`), `reshow` (`TTDT_RESHOW`), `autopop`
(`TTDT_AUTOPOP`); defaults **500, 100, 5000** (Win32's: the double-click time, a fifth of it,
ten times it). `TTM_SETDELAYTIME` with `TTDT_AUTOMATIC` sets `initial` to the value and derives
the other two by those ratios; a value of -1 restores the default; `TTM_GETDELAYTIME` reads them.
The layer tracks the pointer for every tool itself (Win32 needs `TTF_SUBCLASS` or
`TTM_RELAYEVENT` for that; here both are accepted and `TTM_RELAYEVENT` only feeds the same
tracking, so a program that relays and one that does not behave the same).

| Event | Effect |
|---|---|
| the pointer enters a tool's rect (the topmost tool under the pointer wins when rects overlap; a later-added tool wins a tie) and rests | `rest_since` noted; **a move inside the tool restarts it** (Win32: the pointer must be stationary) |
| `initial` ms of rest, the control active, the text not empty, no menu, drop-down, combo list or capture in the window, not `spent` | `TTN_SHOW` sent to the tool's `hwnd` (`NMHDR` only; a nonzero answer leaves the placement as is, as Win32), then the tip is drawn; `TTN_GETDISPINFOW` first for a callback tool |
| the tip is showing for `autopop` ms | hidden, `TTN_POP` sent; `spent` until the pointer leaves the tool |
| the pointer leaves the tool | hidden (`TTN_POP`), `spent` cleared, `hidden_at` noted |
| the pointer enters another tool of any tooltip control in the same top-level window within `reshow` ms of `hidden_at` | that tool's tip shows after `reshow` ms of rest instead of `initial` (the hop along a toolbar) |
| a button-down of any button, anywhere in the window | hidden, `spent` until the pointer leaves the tool; the press is delivered as usual |
| any key down (modifiers included) while the window is active | hidden, `spent` |
| a menu, drop-down, combo list or context menu opens, or the mouse is captured by a control | hidden; nothing shows while one is open |
| the owner's top-level window is deactivated (`WM_ACTIVATE` inactive; `TTS_ALWAYSTIP` clear also hides on it, set hides too: a tip over an inactive window is in the way) | hidden |
| the tool's window is hidden, disabled, destroyed, or `TTM_DELTOOLW` removes the tool | hidden; a disabled child window's tool still shows its tip (it says what the greyed thing is), only a hidden one does not |
| `TTM_POP` | hidden; `TTM_POPUP` shows the current tool's tip at once (the one under the pointer, or the tracking tool) |
| `TTM_ACTIVATE FALSE` | hidden and nothing shows until `TRUE` |
| `TTM_UPDATETIPTEXTW`, `TTM_UPDATE`, `TTM_SETTITLEW` while showing | re-laid and redrawn in place (old and new rects damaged) |

Timers run on the layer's existing hover tick (`win32/hover.rs`, `gui/timers.rs`); the window
is damaged only for the tip's rect. Closing a tip invalidates the rect under it so the program
repaints (`WM_PAINT`), as section 6 of the controls design does for a drop-down. Pointer shape:
unchanged (the control's or the program's). A tip never takes keys or focus.

## 7. Placement, in the top-level window's content coordinates

The pointer at `(px, py)`, the tool's rect `(tx, ty, tw, th)`, the tip `w` x `h`, the content
`cw` x `ch` (the menu bar's 24 rows count as content: a tip may cover the bar, as a drop-down
covers the client).

| Kind | Place |
|---|---|
| plain (the default) | `x = px`, `y = py + 20` (below the 19 px arrow, as Windows puts a tip under its cursor); when `x + w > cw`, `x = cw - w`; when `y + h > ch`, above: `y = py - 4 - h`; then `x = max(0, x)`, `y = max(0, y)` |
| `TTF_CENTERTIP`, and every `TTS_BALLOON` tip | centred on the tool: `x = tx + (tw - w) / 2`; below for a plain tip (`y = ty + th + 4`, above when `y + h > ch`), above for a balloon (`y = ty - 4 - h`, below when `y < 0`); then clamped as the plain kind |
| tracking (`TTF_TRACK`; shown and hidden only by `TTM_TRACKACTIVATE`, the timers off) | `TTM_TRACKPOSITION` gives a screen point, converted to content: with `TTF_ABSOLUTE` the tip's top-left is exactly there, no clamping; without it the point stands in for the pointer in the plain rule (`TTF_CENTERTIP` as above around the tool). A program that never set a position shows the tip at the tool's rule |

A tip wider or taller than the content is **clipped** to the content (D127: nothing leaves the
window); it is drawn last, over every control and over an open drop-down's rect it would overlap
(but section 6 hides it when one opens, so that only happens for a tracking tip). The rect is
not snapped to the pointer afterwards: a tip stays where it appeared until it hides.

## 8. Tools other controls register themselves

- **Toolbar, `TBSTYLE_TOOLTIPS`.** The toolbar creates a tooltip control (`TB_GETTOOLTIPS`
  returns it; `TB_SETTOOLTIPS` replaces it) with one rect tool per visible button (`uId` the
  command id, `hwnd` the toolbar), updated on every button or size change; separators have none.
  The text, tried in this order when the pointer rests: `TBN_GETINFOTIPW` to the parent
  (`NMTBGETINFOTIPW`, `pszText` into an 80-unit buffer, `iItem` the command id; used when it
  returns a non-empty text), else `TTN_GETDISPINFOW` to the parent (`idFrom` the command id, as
  Win32 sends it there for toolbars), else the button's own string **when the button draws no
  text** (`TB_SETMAXTEXTROWS 0`, or `TBSTYLE_EX_MIXEDBUTTONS` without `BTNS_SHOWTEXT`), else no
  tip. The tool's `hwnd` is the toolbar; the tip is drawn in the toolbar's top-level window.
- **List view, `LVS_EX_INFOTIP`.** One tool, re-targeted as the pointer moves: the row under
  it (over its text, not the empty space); the text from `LVN_GETINFOTIPW` (`NMLVGETINFOTIPW`:
  `pszText` a 1024-unit buffer, pre-filled with the item's text when `LVGIT_UNFOLDED` is clear
  and the label is cut, else empty; `cchTextMax` 1024; `iItem`, `iSubItem` 0). Empty
  answer: no tip. Placed by the plain rule. `LVS_EX_LABELTIP`: a first-column label cut with
  `...` shows its full text as a tip with its **text cell over the label's** (`x` = the cell's
  text x - 8, `y` = the row's cell top - 4), so the label appears to unfold; no notification.
- **Tree view, `TVS_INFOTIP`.** As the list view's info tip, `TVN_GETINFOTIPW`
  (`NMTVGETINFOTIPW`: `hItem`, `lParam`, `pszText`, `cchTextMax`), over the item's label.
- **Tab control, `TCS_TOOLTIPS`.** The control creates a tooltip control the program fills
  itself (`TCM_GETTOOLTIPS`/`SETTOOLTIPS`); the layer registers no tools for it.
- **Status bar `SB_SETTIPTEXTW`, header `HDS_HOTTRACK` tips, rebar, combo-box ex**: not in this
  version (listed).

## 9. Touch, TV and wearable, and the element tree

- **Phone and tablet forms** (a touch screen, `toolkit-v0.5.md` 2.4): a press held **500 ms**
  without moving past the drag threshold on a tool shows its tip for **1500 ms**, then it hides;
  a move or the release hides it earlier. The messages the press sends the program are the
  layer's touch rules, unchanged by this brief (the press reached the program at touch down and
  cannot be taken back); the tip only draws. No hover, so `initial` and `reshow` never apply.
- **TV form** (no pointer): a `TTF_IDISHWND` tool whose child window **gains the focus** shows
  its tip after `initial` ms, hidden by the next key; rect tools never show. Wearable: never
  shown.
- **Element tree** (section 29's roles): every tool is a **`tooltip`** element (role 15),
  parent the child window's element for a `TTF_IDISHWND` tool and the owner window's element
  for a rect tool, **name** its text (a callback tool's last answered text, or empty if never
  asked: building the tree sends no notification), **value** the control's title when one is
  set, bounds where it shows or where it would show for the pointer at the tool's centre;
  listed whether or not it shows, as the toolkit's. Actions: none (`NotSupported`). The toolbar's
  tool elements are children of the toolbar's `button` elements; a list or tree info tip has one
  element under its control, named by its last text.

## 10. Tests for tooltips

Host tests, pixel by pixel in the Light palette (the Dark column for the palette rows only):
the five panels of section 3's table; the titled tip with each of the three icons and with none
(`h` 44, the icon's pixels, `w` from a title wider than the text and from a text wider than the
title); `TTS_BALLOON`'s corners and its above-first placement; `TTS_NOPREFIX` on and off; the
plain placement at a pointer near the right edge, near the bottom (above) and in the corner (both
clamps), `TTF_CENTERTIP` below and above, tracking with and without `TTF_ABSOLUTE`, a tip wider
than the content clipped; the timing table: nothing at `initial - 1`, shown at `initial`, a move
inside the tool restarting it, `TTN_SHOW` then the pixels, `autopop` hiding with `TTN_POP` and
`spent`, the hop within `reshow`, a press, a key, a drop-down opening, deactivation, `TTM_POP`,
`TTM_POPUP`, `TTM_ACTIVATE FALSE`, `TTM_SETDELAYTIME` with `TTDT_AUTOMATIC 1000` giving 1000,
200, 10000 and -1 restoring; `TTN_GETDISPINFOW` asked once with `TTF_DI_SETITEM` and every time
without; a toolbar's tips from each of the three sources and none for a button that draws its
text; `LVN_GETINFOTIPW` with the pre-filled buffer, `LVS_EX_LABELTIP` over a cut label and not
over a whole one, `TVN_GETINFOTIPW`; the long press on a phone showing at 500 and hiding at
2000; the tree's `tooltip` elements under a toolbar's buttons and a rect tool's under its window,
with the title as the value. The QEMU boot test adds a tooltip step to the AutoHotkey controls
run: the pointer rests on its toolbar's first button and a screendump after 600 ms is compared.

## 11. Notification-area icons: where they live

**Decision: in every desktop profile, a Windows program's tray icons sit in the bar's
indicator run, directly left of the keyboard and network indicators and right of the Agent and
Updates chips** (the chips move left by the run's width, as they do for the indicators). On the
Windows profile's taskbar that is exactly where Windows keeps them (left of the clock's
indicators); on the macOS profile's top bar it is the menu bar extras' place; on the Linux
profile's top bar GNOME's and KDE's status area. One place, three habits.

- **Top bar (macOS and Linux)**: each icon a **28x28** target in a run that ends 0 px left of
  its right-hand neighbour (the leftmost of the keyboard indicator, the network indicator and the
  power button), its 16x16 icon at the target's (6, 6), no gap between targets (as the
  indicators). At 1280 with a network adapter and one layout: the first icon at (1188, 0, 28,
  28), icon pixels at (1194, 6); the Agent chip's right edge 8 px left of the run.
- **Taskbar (Windows)**: each a **40x40** item with the icon at (12, 12), the run ending 8 px
  left of its neighbour (the keyboard indicator, the network item or the clock), items adjacent
  (Windows' tray has no gaps). At 1280 with an adapter: the first at (1048, 760, 40, 40), icon
  at (1060, 772); the taskbar items' room (6.4 item 13) ends 16 px before the leftmost of the
  chips and the run.
- **Order**: the first icon added (`NIM_ADD`) is the **rightmost**; each later one goes to the
  left of those present, so an icon never moves while others come and go (Windows grows its
  tray leftwards too). A removed icon closes its gap.
- **At most 4 shown**; from the fifth, the run's leftmost cell becomes the **overflow button**:
  `CHEVRON_UP` (8x4, `windows-controls-v0.3.md` section 10) in `TEXT` centred in the target,
  tooltip `Hidden icons`, and the 4th and later icons are hidden in it. A click opens the
  **overflow popup** with the power menu's chrome, 256 wide, rows of 32: the icon at the row's
  (8, 8), the tip text at x + 32 cut with `...` at 26 glyphs, hover `MENU_HOVER`; anchored as the
  layout menu (under the top bar at y 32, right edge at the button's right edge; above the
  taskbar, bottom at the bar's top - 4). A click or a right-click on a row is the same as on the
  icon in the bar, and the popup closes first (so the program's own menu opens on the bar).
- **Hover and pressed** as the bar's items: top bar no hover fill and `BAR_PRESSED` while
  pressed; taskbar `BAR_HOVER` on hover, `BAR_PRESSED` while pressed and while its menu is open.
  Pointer `Arrow`. **2x**: targets double, the icon pixel-doubles (a 32x32 image when the program
  supplied one, section 18).
- **Phone and tablet forms**: the status bar has no room and no hover, so the icons are not in
  it: the **shade** (`adaptive-shell-v0.5.md` 2.7) gets a row `Programs` when at least one icon
  exists: the label `Programs` in `text.dim` at (16, 340), then 44x44 buttons from (16, 364), 8
  px apart, each with its 16x16 icon centred, and everything from element 23 (`Notices`) moves
  down by 76. A tap is a left click (section 13); a long press is a right click. **TV and
  wearable**: no icons (a TV app that lives in the tray has no place there; its balloons are
  still notices, section 15); `wm show shell` still lists them.

## 12. The icon

`NIF_ICON` gives `hIcon`; the layer renders it to **16x16** ARGB by section 27's rules (alpha ≥
128 opaque, a masked icon's mask, a smaller icon centred, a larger one scaled down by the
installer's rule of `app-icons-v0.3.md` section 4, and a 32x32 rendering when the icon has one
for 2x); the server draws the pixels as given, clipped to the 16x16 box, over the bar. No
`NIF_ICON`, or a `NULL` icon: the **generic tile at 16x16**: `icon.tile` with a 1 px
`icon.tile.border`, `INSET_R4`, the owner window's title's first ASCII letter or digit
upper-cased in `text` with its cell at (4, 0), `?` otherwise (6.4 item 1's rule, smaller).
`NIS_HIDDEN` (in `dwState` with `NIF_STATE`): not drawn, no target, keeps its place in the
order for when it shows again. `NIS_SHAREDICON`: ignored (listed). `NIM_MODIFY` with `NIF_ICON`
replaces the pixels in place (the target's rect damaged); the tooltip, if showing, is unchanged.

## 13. Tooltip, clicks, keys and the messages the program gets

**Tooltip**: `szTip` (`NIF_TIP`; up to 128 UTF-16 units, read whole whatever the icon's version:
Windows 2000's 64-unit limit is not kept, since nothing is gained by cutting it) after 600 ms of
rest, the panel tooltip's chrome (6.4 item 3: `MENU_BG`, 1 px `MENU_BORDER`, r4, 24 tall,
Egg Mono, width glyphs x 8 + 16; below the top bar at y 32, above the taskbar at y 732 at 800
tall, centred on the target and clamped 8 px inside the screen). `\r\n` and `\n` draw as one
space (the bar's tooltip is one line); the text is cut with `...` so the panel fits the screen's
width - 16. No tip (`NIF_TIP` clear or empty): the owner window's title, or the program's file
name when the title is empty, so every icon has a name. The text is the icon's accessible name.

Every pointer event over an icon reaches the program as `uCallbackMessage` (`NIF_MESSAGE`,
posted to `hWnd`), packed by the icon's version (`NIM_SETVERSION`: 0, 3 or 4 accepted and
stored per icon; another value answers `FALSE`). The server reports the events to the layer
with the screen point; the layer packs and posts.

| The person | Versions 0 and 3: `wParam` = `uID`, `lParam` = the message | Version 4: `wParam` = `MAKELONG(x, y)` screen point, `lParam` = `MAKELONG(message, uID)` |
|---|---|---|
| pointer moves over the icon | `WM_MOUSEMOVE` | `WM_MOUSEMOVE` |
| left button down / up | `WM_LBUTTONDOWN`, then `WM_LBUTTONUP`; version 3 adds `NIN_SELECT` after the up | `WM_LBUTTONDOWN`, `WM_LBUTTONUP`, then `NIN_SELECT` |
| left double click (section 17's 400 ms, 4 px) | down, up, `WM_LBUTTONDBLCLK`, up (version 3: `NIN_SELECT` after each up) | the same four, then `NIN_SELECT` once |
| right button down / up | `WM_RBUTTONDOWN`, `WM_RBUTTONUP` | `WM_RBUTTONDOWN`, `WM_RBUTTONUP`, then **`WM_CONTEXTMENU`** |
| middle button | `WM_MBUTTONDOWN`, `WM_MBUTTONUP` | the same |
| the tooltip's 600 ms rest | nothing beyond the tip | **`NIN_POPUPOPEN`** at the rest, **`NIN_POPUPCLOSE`** when the pointer leaves; the bar's tip shows regardless of `NIF_SHOWTIP` (a program's own popup window would be a window of its own, and the person is never left without a name) |
| Enter or Space with the panel focus on the icon (`first-run-accessibility-v0.3.md` 2.8: Ctrl+Alt+Tab, Super+T or Ctrl+F3, then Left/Right) | version 0: the left click's pair; version 3: `NIN_KEYSELECT` | `NIN_KEYSELECT` (a second press within 400 ms: twice, as Win32) |
| Shift+F10 or the Menu key with the panel focus on the icon | `WM_RBUTTONUP` alone | `WM_CONTEXTMENU` with the icon's centre as the point |
| the shade's button on a phone or tablet: a tap / a long press | the left click's pair / `WM_RBUTTONUP` | `NIN_SELECT` / `WM_CONTEXTMENU` |

The pointer's button-down is **consumed by the bar** (it focuses nothing and the window under
it gets nothing), as every bar item's. `WM_MOUSEMOVE` is posted at most once per server pointer
event, never on a timer. `Shell_NotifyIconGetRect` answers the icon's target rect in screen
coordinates (the server tells the layer each icon's rect whenever the bar is laid out, section
18); an icon hidden in the overflow answers the overflow button's rect, as Windows answers the
chevron's. `NIM_SETFOCUS` moves the panel focus to the icon (the ring of 2.8), and is how a
program returns the keys after its menu closed.

## 14. The program's context menu

A tray program answers the right click with `SetForegroundWindow(hWnd)` and
`TrackPopupMenu(Ex)` at the pointer's screen point. That point is on the bar, outside every
window of the program, and the program's window is usually hidden: the controls design's rule
(section 12: a context menu is drawn in `hwnd`'s buffer, clamped into it) would put the menu in
a hidden window or in the wrong corner of a shown one. **Decision: a `TrackPopupMenu` whose
anchor point lies outside the content of `hwnd`'s top-level window, or whose top-level window is
hidden or minimised, is shown by the server as a shell menu**, from the menu's entries the layer
sends it (section 18), in the power menu's chrome: `MENU_BG`, 1 px `MENU_BORDER`, radius 6, the
window shadow, width `max(160, 8 * widest + 48)` (the drop-down's rule), rows of
`MENU_ITEM_HEIGHT` 32, text at x + 24 in `TEXT`, hover and the keyboard's row `MENU_HOVER`,
disabled rows `TEXT_DIM`, a checked item's tick (the check box's tick, origin at (x + 7, row +
9)) and a radio-checked item's 6 px `INSET_R3` dot at (x + 11, row + 13) in `TEXT`, a separator
a 1 px `MENU_BORDER` line in an 8 px row (the layout menu's). Anchored to the icon the click was
on, as the layout menu is: under the top bar (top at 32) with its right edge at the target's
right edge; above the taskbar (bottom at the bar's top - 4); a menu taller than the work area is
clipped at its far edge (the hidden rows stay reachable by Up/Down; listed). `TPM_*`
alignments are ignored for a shell menu (the anchor is the icon, not the point). An `MF_POPUP`
item draws as a disabled row (submenus in shell menus: not in this version; listed). Up/Down,
Enter, Esc, a click on a row, and a click or key anywhere else work as the layout menu's; the
chosen entry's index comes back to the layer, which posts `WM_COMMAND` (or returns the id with
`TPM_RETURNCMD`); Esc or a click outside returns 0. The call blocks as today, running the
thread's modal loop. The icon's target is `BAR_PRESSED` while the menu is open. A
`TrackPopupMenu` inside the program's own content is unchanged (section 12's rule).

## 15. Balloons are notices (`NIF_INFO`)

EggOS has no notification service yet (`adaptive-shell-v0.5.md` section 8, left out on
purpose); the shell has **the notice** (`desktop-v0.3-gui.md` 6.4 item 2: a 320x40 `MENU_BG`
popup centred at the work area's top + 16, closed after 3000 ms, by any key or by a button-down
anywhere) and the phone's shade keeps the last four. **A balloon is a notice.** When the
service comes, balloons become its notifications and nothing in the program changes.

- The notice's text: `szInfoTitle`, `: `, `szInfo` when both are set; the one that is set
  otherwise; `\n` as a space; cut with `...` at 36 glyphs (the 320 px panel with 16 px of padding
  each side). `dwInfoFlags`' icon (`NIIF_INFO`, `_WARNING`, `_ERROR`, `_USER`, `_LARGE_ICON`),
  `NIIF_NOSOUND` and `NIIF_RESPECT_QUIET_TIME` change nothing (one notice chrome; listed);
  `uTimeout` is ignored (3000 ms, as Windows ignores it since Vista). An empty `szInfo` with
  `NIF_INFO` closes the icon's balloon if it is the notice showing.
- Messages to the program (packed by the icon's version as section 13): **`NIN_BALLOONSHOW`**
  when the notice appears; **`NIN_BALLOONUSERCLICK`** when a button-down lands inside the
  notice's rect (that press is consumed, and the notice closes); **`NIN_BALLOONTIMEOUT`** when it
  closes by its 3000 ms, by a key, by a press outside it, or when another notice or balloon
  replaces it; **`NIN_BALLOONHIDE`** when its icon is deleted or its `szInfo` emptied while it
  shows. A balloon from a hidden icon still shows (a notice is not tied to the icon's pixels).
  A second balloon replaces the first (Windows queues them; EggOS's notice does not, and a
  queue of program popups is not worth the wait; listed).
- On a phone or tablet the notice is the shade's row too (2.7's `Notices`), so a missed balloon
  is still readable; on a TV the notice shows as the shell's own do.

## 16. Life of an icon

`Shell_NotifyIconW(message, &nid)`: `NIM_ADD` (an icon for `(hWnd, uID)`, or `guidItem` with
`NIF_GUID`, which is the key then; a second add for the same key answers `FALSE`), `NIM_MODIFY`
(the fields `uFlags` names; an unknown key answers `FALSE`), `NIM_DELETE` (removed, its tooltip
and menu closed, a showing balloon hidden with `NIN_BALLOONHIDE`), `NIM_SETVERSION` (`uVersion`),
`NIM_SETFOCUS`. `cbSize` of any documented size is accepted (the Windows 2000, XP and Vista
layouts; a smaller one reads only the fields it has). The function is thread-safe and returns
once the server has the change (within one frame). **When `hWnd` is destroyed, or the process
exits without `NIM_DELETE`, its icons are removed at once**: the layer removes them on
`DestroyWindow` and the server drops every icon of a client whose `/dev/window` closes. Windows
leaves a ghost until the pointer passes over it; EggOS does not, and this is the one place the
two differ on purpose (a ghost is a bug a person can see). `TaskbarCreated`: the layer registers
the message name (`RegisterWindowMessageW`) and broadcasts it to the program's top-level windows
once, when the program's first window is created after the desktop started (programs re-add
their icons on it), never again (the bar never restarts under a running program). A program
with 16 icons or more answers `FALSE` for the 17th (listed).

## 17. What an agent sees

`wm show shell` (`wm_show`, window `shell`) lists, in the bar's part of the tree, a **`group`
named `Notification area`** whenever at least one icon exists, with one **`button` per icon in
bar order** (right to left as drawn, so the first added is first), **name** the tooltip text of
section 13, **value** the program's file name (`notepad++.exe`), flags `expanded` while its
shell menu or the overflow popup it is in is open, `focused` with the panel focus on it; hidden
icons (`NIS_HIDDEN`) are omitted, overflowed ones are listed with the overflow button's bounds.
Then the overflow **`button` `Hidden icons`** when it shows, and while a shell menu is open a
**`list`** named `<tip> menu` of `list item`s titled by the entries (`disabled` for disabled
ones, `checked` for checked ones; separators omitted), as the layout menu's pattern. Actions
(`wm act shell <element> ...`): `invoke` on an icon is a left click (section 13's pair and
`NIN_SELECT`), `focus` is `NIM_SETFOCUS`'s ring, `select` opens its context menu (the right
click), `invoke` on a menu item chooses it (`WM_COMMAND` to the program), `set-text`
`NotSupported`. On a phone the shade's `Programs` row lists the same buttons under the `Status
bar` element. A balloon is the notice, which the tree already lists while it shows (its text as
a `text` element) and the phone's shade keeps. In the program's own window tree nothing is
added: an icon is not in a window.

## 18. What the server needs from the layer, and sends back

Described, not designed (the engineer designs the protocol, as D127 and D147 did for popups and
menus; `SET_MENU`'s pool format is a fit for the menu entries):

- **Per icon, from the layer**: a client-unique id; the 16x16 ARGB pixels (1 KiB) and, when the
  program's icon has one, the 32x32 image (4 KiB) for 2x; the tooltip text (UTF-8, at most 256
  bytes); hidden or shown; add, modify (any subset), remove. The server keeps the order of
  adding and lays the run out by section 11.
- **Per icon, from the server**: pointer events over the target with the screen point and the
  button (enter, move, leave, down, up, double); the keyboard select and the context key; and the
  icon's target rect on every bar layout (profile change, resolution change, a neighbour
  appearing), for `Shell_NotifyIconGetRect` and `NIN_POPUPOPEN`'s rect.
- **A shell menu**: the layer sends the entries (label, disabled, checked, radio, separator, at
  most 32 rows) and the icon id to anchor on; the server answers the chosen entry's index or
  "cancelled", and tells when the menu closed. The program's window keeps its activation
  meanwhile.
- **A balloon**: the layer sends the notice text and the icon id; the server answers shown,
  clicked, closed-by-time-or-key, replaced, so the layer posts the `NIN_BALLOON*` codes.
- **Cleanup**: the server removes every icon of a client whose `/dev/window` closes, before the
  next frame.

## 19. Tests for the tray

Host tests on the desktop (`drivers/src/desktop` tests, 1280x800 at 1x, each profile): one icon's
target and pixels in all three profiles with and without the network and keyboard indicators;
the chips' shift; three icons' order after adds and a middle removal; the overflow button with
five icons, its popup's rows and a click on a row; hover and pressed fills per bar; the tooltip
rect for `Notepad++` (9 glyphs, 88 wide) below the top bar and above the taskbar, cut for a 128
unit tip at a 390 px width; the generic 16x16 tile for an icon without pixels; a hidden icon's
absent target and kept order; the shell menu's chrome, anchor and rows (a disabled, a checked, a
radio, a separator, a submenu row drawn disabled), Up/Down/Enter/Esc and a click outside; the
notice's text for title and info, info alone, the 36-glyph cut, the click inside and outside it;
the `wm show shell` tree with two icons, one in the overflow, and the open menu; the phone shade's
`Programs` row and the 76 px shift. Host tests on the layer: the message sequences of section
13's table for versions 0, 3 and 4 (packing checked field by field), `NIM_ADD` twice, `NIM_MODIFY`
of an unknown key, `NIM_SETVERSION 5`, `NIM_DELETE` while a balloon shows (`NIN_BALLOONHIDE`),
`DestroyWindow` and a process exit removing the icons, `TaskbarCreated` broadcast once,
`Shell_NotifyIconGetRect` for a shown and an overflowed icon, a `TrackPopupMenu` at a bar point
routed to the server and `WM_COMMAND` or `TPM_RETURNCMD` from the answer, and the `NIN_BALLOON*`
codes from each server answer. The QEMU boot test runs a small tray program (a window hidden at
start, one icon with a tip and a two-item menu, a balloon on a timer): a screendump of the bar
after the add, after the tooltip's 600 ms, with the menu open after a right click, and after the
balloon; then the program exits without `NIM_DELETE` and the next screendump shows no icon.

## 20. Not in this version

- `TTS_CLOSE` (a close button on balloons), `TTM_SETTIPBKCOLOR`/`SETTIPTEXTCOLOR` and
  `TTM_SETMARGIN` as drawn, `NM_CUSTOMDRAW` for tooltips, `TTN_LINKCLICK` and links in tips,
  more than 64 tools per control, more than 16 lines or 255 units of text, `TTF_PARSELINKS`,
  `TTF_TRANSPARENT`, `TTM_WINDOWFROMPOINT`, status-bar part tips, header hot-track tips, rebar
  and combo-box-ex tips.
- Submenus in a tray program's shell menu (drawn disabled), `TPM_*` alignment of a shell menu,
  a program's own `NIN_POPUPOPEN` popup window replacing the bar's tip, balloon icons and a
  queue of balloons, `NIIF_*` sounds, `NIS_SHAREDICON`, more than 16 icons per program, a tray
  on the TV form, dragging icons to reorder, a Settings page to hide icons, and the real
  notification service (balloons move to it when it exists).
