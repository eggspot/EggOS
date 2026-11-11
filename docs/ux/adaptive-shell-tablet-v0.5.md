# EggOS v0.5 adaptive shell, slice 3: the tablet form: design brief

Design by Claude Fable (D16). This is the numbered brief that `adaptive-shell-v0.5.md` (the
parent) promises for its section 3 and section 9 item 3: the dock, the title buttons and the
window menu, full and split windows, the chooser, the divider, the keys of 3.5, the change to
the desktop form and back, and 3.6's touch sizes. The parent's rules hold wherever this brief
is silent (how the form is chosen, the status bar's items, the home screen, `Back`, the shade,
gestures, the keyboard's area). Where the two differ on a number, this brief is the newer one.

Tokens, the 4 px grid, Egg Mono 8x16, the corner tables and the voice are the desktop's
(`desktop-v0.3-gui.md` section 1). **No colour token is added and nothing moves**: every surface
appears in one frame. Numbers are logical px at scale 1, origin top left, a rect is
(x, y, width, height). Each element is given at **1024x768** and at **1366x1024**; where one
number is given with `W`, it holds for both and for an upright tablet.

The switcher and the shade are slice 2's. If slice 3 is built before them, they are built here
at the tablet's numbers (sections 8 and 2) and the phone takes them from the same code.

## 0. What a person gets, and how that is checked

- A tablet shows a 44 px status bar, a 64 px dock, and between them one app, or two side by
  side at a third, a half or two thirds, with a divider a finger can drag.
- Everything a finger does has a key, a `wm` command and an MCP tool (D22): nothing in this
  form is GUI-only, and no gesture is the only way.
- With a keyboard cover that has a touchpad, the same machine becomes the desktop, with sizes
  a finger can still hit, and goes back when the cover comes off.
- Checked by the host tests of section 14 at 1024x768, 1366x1024 and 768x1024.

## 1. The frame

| # | Element | 1024x768 | 1366x1024 | Draws |
|---|---------|----------|-----------|-------|
| 1 | status bar | (0, 0, 1024, 44) | (0, 0, 1366, 44) | `bar.bg`; row 43 `bar.edge` |
| 2 | work area | (0, 44, 1024, 660) | (0, 44, 1366, 916) | the home screen, one window, or two and the divider |
| 3 | dock | (0, 704, 1024, 64) | (0, 960, 1366, 64) | `bar.bg`; its top row `bar.edge` |

- Upright (768x1024, 1024x1366) the bars stay top and bottom: the work area is
  (0, 44, `W`, `H` - 108). The three profiles give the same frame.
- Every target is at least 44x44, and targets sit 4 px apart or share an edge (parent 2.1).
- A `Normal` window has no title bar, border, shadow or controls in this form.

## 2. The status bar

Cell tops are y 14. The right-hand run is the phone's (parent 2.2, elements 6 to 9), packed
leftwards from x `W` - 16.

| # | Element | 1024x768 | 1366x1024 | Text and colour |
|---|---------|----------|-----------|-----------------|
| 4 | clock | (16, 14), 40 wide | the same | `09:14` in `text` |
| 5 | title button, one per shown window | section 2.1 | section 2.1 | the window's title and a caret |
| 6 | battery icon and percent | right edge 1008 | right edge 1350 | as the phone's; absent until a battery is reported |
| 7 | network indicator | (992, 14, 16, 16) | (1334, 14, 16, 16) | the bar's glyph of `network-v0.3.md` section 4 |
| 8 | Agent chip | (928, 12, 56, 20), text at (936, 14) | (1270, 12, 56, 20), text at (1278, 14) | `Agent`, as `desktop-v0.3-gui.md` 14.1; shown while an agent is connected |

The positions of 7 and 8 are QEMU's (an adapter, no battery).

### 2.1 Title buttons (5)

- **Size**: 44 tall (rows 0 to 43), as wide as its text + 32, at least 96. Inside, the text and
  a 7x4 caret (rows of 7, 5, 3 and 1 px, pointing down, its top at y 20, 6 px right of the
  text) are centred as one group of text + 13 px.
- **Place**: centred over its window: on x `W`/2 when the window is full, on the middle of its
  side when split. It must lie between x 72 and 8 px left of the leftmost item of the
  right-hand run (x 984 at 1024 with no chip, 920 with it); a button that would cross either
  limit is moved sideways just enough. The text is cut with `...` to
  `min(40, (span - 48) / 8)` glyphs, where the span is the part of its side between those
  limits.
- **Colours**: the focused window's title and caret are `text`, with a 2 px `accent` line under
  the text (the text's width, rows 34 and 35). The other side's are `text.dim`, no line. A full
  window is the focused one. Pressed, and while its menu is open, the button is `bar.pressed`
  and its text and caret are `text`. With a pointer (parent 1.3), hover is `bar.hover`.
- `Terminal` full at 1024: the button (464, 0, 96, 44), the text from (473, 14), the caret at
  (543, 20), the line (473, 34, 64, 2). At 1366: (635, 0, 96, 44), (644, 14), (714, 20),
  (644, 34, 64, 2). Split in halves at 1024 with `Terminal` left and `Files` right: (206, 0,
  96, 44) with text from (215, 14), and (722, 0, 96, 44) with text from (743, 14).
- **A tap** focuses that window and opens its window menu (section 6). ` (not responding)` and
  ` (agent)` are part of the title, as on the desktop.
- **The rest of the bar is one button**, `Status bar`: a tap anywhere outside a title button
  opens the shade, and the bar outside the title buttons is `bar.pressed` while pressed.
- **The shade** is the phone's (parent 2.7), 390 wide under the bar's right end: every rect of
  the parent's elements 16 to 28 with `W` - 390 added to x (634 at 1024, 976 at 1366), a 1 px
  `menu.border` column at its left edge, and the rest of the screen under the bar darkened by
  `half(p)`.

## 3. The dock

| # | Element | 1024x768 | 1366x1024 | Draws |
|---|---------|----------|-----------|-------|
| 9 | `Back` | zone (0, 704, 64, 64) | (0, 960, 64, 64) | `←` at 2x in the cell (24, 720, 16, 32), y 976 at 1366; `text`, or `text.dim` when there is nothing to go back to |
| 10 | `Home` | zone (64, 704, 64, 64) | (64, 960, 64, 64) | the egg mark, W 14, H 20, `accent`, at (89, 726); y 982 |
| 11 | `Apps` | zone (128, 704, 64, 64) | (128, 960, 64, 64) | a 14x14 square outline, 1 px, `text`, at (153, 729); y 985 |
| 12 | app target, one per app | 64x64, in the centred group | the same | the 48x48 icon at the target's (8, 6) |
| 13 | separator | 17 wide: 8, a 1x40 `menu.border` line, 8 | the same | the line's top 12 px under the dock's top |
| 14 | running mark | 4x4 at the target's (30, 56) | the same | `text`; the app has a window |
| 15 | focus line | 16x2 at the target's (24, 60) | the same | `accent`; the app's window is the focused one |

- **The group** (12 to 15): the pinned apps Terminal, Files and Settings, then, only when
  another app has a window, the separator and one target per such app in the order they were
  first opened. Its width is 64 per app, plus 17 with the separator; its left edge is
  (`W` - width) / 2, rounded down.
  - Three pinned apps and nothing else: targets at x 416, 480 and 544 at 1024 (icons at
    (424, 710), (488, 710), (552, 710)); x 587, 651 and 715 at 1366 (icons at y 966).
  - With one more app at 1024: the group is 273 wide from x 375; targets at 375, 439 and 503,
    the line at (575, 716, 1, 40), the fourth target at (584, 704, 64, 64).
- **How many fit**: the group keeps 16 px clear of the navigation zones and of the same room
  on the right, so it holds `(W - 433) / 64` apps, rounded down: 9 at 1024, 14 at 1366, 5 at
  768. When more have windows, the least recently shown unpinned ones are left out; they stay
  in the switcher and the home grid.
- **The right 192 px draw nothing**: they keep the group centred.
- **States**: a pressed zone or target is `bar.pressed`; with a pointer, hover is `bar.hover`.
  A zone's glyph is `text` while pressed.
- **A tap** on 9, 10 and 11 does what the phone's buttons do (parent 2.3, 2.5, 2.6). A tap on
  an app target launches the app, or shows its most recently shown window, placed by section
  5.2's rules. A launch shows the notice `Starting <name>...` until the window maps.
- **A long press** (500 ms), the Menu key or Shift+F10 on an app target opens the **icon
  menu**: a popup 200 wide, rows 44 tall, 4 px above and below the rows (184 tall), `menu.bg`,
  1 px `menu.border`, radius 6, its bottom 4 px above the dock, centred on the target and kept
  8 px inside the screen. Entries: `Open`, `Split left`, `Split right`, `Close`. For an app
  with no window `Close` is `text.dim` and skipped; `Split left` on it launches it onto that
  side. `Close` sends the close request to the app's most recently shown window.

## 4. The home screen and the search

The `Background` window's tablet layout, with the phone's rules (parent 2.4) and these numbers.

| # | Element | 1024x768 | 1366x1024 | Draws |
|---|---------|----------|-----------|-------|
| 16 | search field | (272, 60, 480, 44) | (443, 60, 480, 44) | as the phone's; `Search apps` from the field's (12, 14) in `text.dim` |
| 17 | app grid, a scroll view | (0, 120, 1024, 584): 8 columns of 112 from x 64 | (0, 120, 1366, 840): 11 columns of 112 from x 67 | row `r` top at 120 + 104 x `r`, less the scroll |
| 18 | app cell | 112x104, the whole cell the target | the same | the icon at the cell's (32, 8); the running mark at (54, 58); the label centred, top 64, at most 12 glyphs, on its 20 px `bar.bg` pill |

- Upright or at any other width: the columns are `(W - 32) / 112`, rounded down, centred.
- At 1024 the first row's icons are at x 96, 208, 320, 432, 544, 656, 768 and 880, y 128; the
  ninth app's icon is at (96, 232). Five rows show whole and the sixth shows its top 64 px. At
  1366 the first icon is at (99, 128) and eight rows show whole.
- **Search results** replace the grid: rows 48 tall and 480 wide under the field, row `i` at
  (272, 120 + 48 x `i`, 480, 48) at 1024 and x 443 at 1366; the 32x32 icon at the row's
  (12, 8), the name from its (52, 16), a foreign app's source in `text.dim` ending 12 px inside
  the right edge; the first row `selection`.
- **The notice** sits centred with its bottom 16 px above the dock: (352, 648, 320, 40) at
  1024, (523, 904, 320, 40) at 1366.

## 5. Windows: full or split

### 5.1 The places

A shown `Normal` window is **full** (the work area; `maximised` in `wm list`) or one side of
the **pair** (`shown`, with its side's rect). The pair is a left window, a right window and
the divider's place, one of three:

| # | Element | Place | 1024x768 | 1366x1024 |
|---|---------|-------|----------|-----------|
| 19 | left side | one third | (0, 44, 336, 660) | (0, 44, 452, 916) |
| | | a half | (0, 44, 508, 660) | (0, 44, 679, 916) |
| | | two thirds | (0, 44, 680, 660) | (0, 44, 906, 916) |
| 20 | divider | one third | (336, 44, 8, 660) | (452, 44, 8, 916) |
| | | a half | (508, 44, 8, 660) | (679, 44, 8, 916) |
| | | two thirds | (680, 44, 8, 660) | (906, 44, 8, 916) |
| 21 | right side | left is one third | (344, 44, 680, 660) | (460, 44, 906, 916) |
| | | a half | (516, 44, 508, 660) | (687, 44, 679, 916) |
| | | left is two thirds | (688, 44, 336, 660) | (914, 44, 452, 916) |

- A third is `(W - 8) / 3` rounded down to a multiple of 4; a half is `(W - 8) / 2` rounded
  down, and the right side takes what is left. The place is always named for the **left**
  side's share.
- **No side is under 320.** At 768 wide a third would be 252, so only the half (380, 8, 380)
  exists there: `Make wider` and `Make narrower` are `text.dim`, and the divider does not snap
  to thirds.
- **A side under 600 wide** lays its app out in one column (the toolkit's width class comes
  from the window's own width, parent ask 9). The input class stays touch.

### 5.2 What goes where

1. **Opening an app or showing a window** (the dock, the home grid, the switcher, `wm focus`):
   with one window full or the home screen shown, it becomes full and the window that was
   full becomes `minimised`. With the pair shown, it takes the **focused** side and the window
   that was there becomes `minimised`. A window already in the shown pair is only focused.
2. **A window that belongs to the pair while the pair is not shown** brings the pair back,
   with its own side focused.
3. **Home** minimises what is shown and shows the home screen. The pair is kept: the switcher
   shows it as one card (section 8).
4. **Closing one side, sending it home with `wm minimise`, or `Full screen` on the other**
   ends the pair: the remaining window is full.
5. **One pair at a time.** Making a new split ends an older pair that is not shown; its
   windows become two ordinary cards.
6. **A window's minimum size** is clamped to its side, as the phone clamps it to the work area.
7. **A tap in a side** focuses it before the window gets the touch. Only the focused side's
   title has the `accent` line; the content itself gets no outline.

## 6. The window menu

Opened by a title button (5), Alt+Space, or `invoke` on the button's element.

| # | Element | Place | Draws |
|---|---------|-------|-------|
| 22 | window menu | a popup 200 wide, its top at y 48, its left edge the title button's, kept 8 px inside the screen | `menu.bg`, 1 px `menu.border`, radius 6, the popup shadow |
| 23 | row | 200x44, the first 4 px under the popup's top | the label from the row's (16, 14) in `text`; selected `selection` with the focus ring; not applicable `text.dim`, skipped by Up and Down |
| 24 | separator | 9 px above `Close`: a 1 px `menu.border` line with 4 px each side | |

| Entry | Shown | Does | Not applicable when |
|-------|-------|------|---------------------|
| `Full screen` | always | the window is full; a pair ends | it is full |
| `Split left` | always | the window goes to the left side: from full, at a half with the chooser on the right (section 7); from the right side of the pair, the two swap | it is the left side |
| `Split right` | always | the mirror of `Split left` | it is the right side |
| `Make wider` | while split | the divider moves one place in this window's favour | it has two thirds, or thirds do not exist |
| `Make narrower` | while split | the divider moves one place against it | it has one third, or thirds do not exist |
| `Swap sides` | while split | left and right change places; the divider's place is mirrored | |
| `Close` | always | the close request | |

The popup is 193 tall for a full window (four rows) and 325 while split (seven). For `Terminal`
full at 1024 it is (464, 48, 200, 193): `Full screen` (464, 52, 200, 44) in `text.dim`, `Split
left` at y 96, `Split right` at y 140, the line at y 188, `Close` (464, 193, 200, 44). Esc,
`Back` or a tap outside closes it. Up, Down, Enter and first letters work as in the desktop's
menus.

## 7. The chooser for the second app

When a full window is sent to one side, the other half shows the chooser until a second
window fills it. The chooser always opens at a half.

| # | Element | Right half at 1024x768 | Right half at 1366x1024 | Draws |
|---|---------|------------------------|-------------------------|-------|
| 25 | chooser, one scroll view | (516, 44, 508, 660) | (687, 44, 679, 916) | `menu.bg` |
| 26 | heading | from (532, 60) | from (703, 60) | `Choose an app for this side` in `text.dim` |
| 27 | window card | 144x128; 3 columns at x 538, 698, 858; rows from y 92, 144 apart | 4 columns at x 714, 874, 1034, 1194 | `button.bg`, radius 4; the window scaled to fit a 128x80 box at the card's (8, 8), centred; its title from (8, 100), at most 16 glyphs, `text` |
| 28 | app cell | the home grid's cell (18); 4 columns from x 546 | 5 columns from x 746 | as on the home screen |

- **Cards**: one per other `Normal` window, shown or minimised, most recently shown first.
  The columns are `(side - 16) / 160` rounded down, centred in the side.
- **Cells**: every app, in the home screen's order, in `(side - 32) / 112` columns, centred.
  The first row's top is 16 px under the last row of cards (y 236 with one row of cards at
  1024, so the first icon is at (578, 244)), or y 92 with no cards.
- For the left half the same numbers are measured from the side's own left edge.
- **A tap on a card** puts that window on the side. **A tap on a cell** launches the app onto
  the side, or puts its most recently shown window there; an app whose only window is the
  other side's gets a new window. The new side is focused.
- **Cancelling**: `Back`, Esc, `Home`, or `Full screen` on the first window. The first window
  is full again (`Home` then shows the home screen).
- While the chooser shows, the divider (20) is drawn without its grip and cannot be dragged,
  the chooser's side has no title button, and the chooser has the focus: the first card (or
  cell) is `selection` with the ring, arrows move it, Enter chooses.
- Pressed, a card or a cell is `selection`.

## 8. The divider, the switcher, dialogs, the keyboard

### 8.1 The divider (20)

| # | Element | 1024x768, a half | 1366x1024, a half | Draws |
|---|---------|------------------|-------------------|-------|
| 29 | grip | (510, 350, 4, 48) | (681, 478, 4, 48) | `text.dim`; centred in the divider, which is `bar.bg` |
| 30 | target | the divider's own 8 px, full height, and a pad (490, 330, 44, 88) around the grip | the pad (661, 458, 44, 88) | nothing of its own |

- The pad is the only part of a window the shell takes: 18 px each side, 88 tall. Touches
  there are the divider's; everywhere else a side's edge belongs to its app (its scroll bar
  stays reachable).
- **Dragging**: a touch that goes down in the target and moves 8 px starts the drag. The
  divider is then drawn at the finger (its x is the finger's x - 4, kept on screen) in
  `bar.pressed` with the grip in `text`, over the two windows, **which are not resized while
  the finger is down**. On release:
  1. the finger within 160 px of the left or right edge of the screen: that side's window
     becomes `minimised` and the other is full;
  2. else the divider goes to the nearest of its three places (a tie goes to the half) and
     each window is resized once. At 1024 the divider's x up to 421 gives one third, 422 to
     594 a half, 595 and over two thirds; at 1366, up to 565, 566 to 792, 793 and over.
- Esc, a cancelled touch or a change of form during the drag puts the divider back.
- **By key**: the divider is a focus stop of the shell (section 9). With the ring on it (2 px
  `focus.ring` around the pad), Left and Right move it one place; at the last place the key
  does nothing, so a key never closes a side. Super+Shift+Left and Super+Shift+Right do the
  same from anywhere. `Make wider` and `Make narrower` do it from the menu.

### 8.2 The switcher (`Apps`)

The phone's switcher (parent 2.5) in columns. A card is a 420x320 `menu.bg` panel: the title
from its (16, 14), a 44x44 `Close` target at its top right corner, and the window scaled to
fit a 404x260 box at its (8, 52). Columns: `(W - 16) / 436` rounded down, 16 apart, centred;
rows 336 apart from y 60. At 1024: two columns at x 84 and 520. At 1366: three at x 37, 473
and 909. At 768: one. Most recently shown first; under the last row, `Close all` (358x44,
centred).

**The pair is one card**: the box shows both sides and the divider scaled together, the
title is `<left title> + <right title>`, a tap brings the pair back, and `Close` sends the
close request to both windows.

### 8.3 Dialogs, menus, the keyboard's area

- A dialog whose parent's side is at least 600 wide is centred on that side with the desktop's
  dialog chrome at touch sizes (section 11) and cannot be moved. On a narrower side it is a
  bottom sheet of that side (the side's width, its bottom on the dock's top, the side's rows
  above it darkened by `half(p)`).
- A window's menus drop down as on the desktop with rows 44 tall.
- The on-screen keyboard's panel is (0, 404, 1024, 300) and (0, 660, 1366, 300). While it
  shows, the work area ends at its top and both sides are resized to it.

## 9. With a keyboard and no pointer

The form stays `tablet` and nothing moves on screen; the on-screen keyboard stops showing and
the ring is drawn from the first key on.

| Key | Does |
|-----|------|
| Super alone | the home screen, with the focus in the search field |
| Alt+Tab, Cmd+Tab (Shift back) | the desktop's switcher list (`desktop-v0.3-gui.md` 5.1); a pair's windows are two rows |
| Super+Left, Super+Right | `Split left`, `Split right` on the focused window |
| Super+Up | `Full screen` |
| Super+Down | sends the focused window home (`wm minimise`) |
| Super+Shift+Left, Super+Shift+Right | the divider one place left or right |
| Alt+Space | the focused window's menu |
| Esc | `Back` (parent 2.6) |
| Alt+F4, the profile's close key | closes the focused window |
| Super+D | `Home` |
| F6 | moves the focus between the focused window and the shell |
| Tab, Shift+Tab (in the shell) | the next and previous stop: the title buttons left to right, `Status bar`, `Back`, `Home`, `Apps`, the dock's apps as one stop, the divider |
| arrows (in the shell) | Left and Right within the dock's apps and between title buttons; on the divider, section 8.1 |
| Enter, Space | presses the stop; Menu or Shift+F10 on a dock app opens its icon menu |

The ring is the desktop's: 2 px `focus.ring`, 2 px inside the stop's rect.

## 10. The change to the desktop form and back

With `shell.form` `auto` (parent 1.2, rows 2 and 3):

1. **A keyboard and a pointer are both present**: the form becomes `desktop` at once, in the
   profile's layout with touch sizes (section 11). Open popups, the chooser and a divider drag
   are cancelled first. A full window becomes maximised. The pair becomes two restored
   windows tiled side by side in the desktop's work area: the left one from the work area's
   left edge to the divider's x + 4, the right one from there to the right edge, both the
   work area's height, with their chrome back. Minimised windows stay minimised. The focus
   stays where it was. The notice reads `Desktop layout: keyboard and pointer attached.`
2. **Either is removed**: the form becomes `tablet`. The focused window becomes full, every
   other window `minimised`, and the notice reads `Tablet layout: keyboard or pointer
   removed.` No pair is made: the person splits again if they want one.
3. A device that drops out and returns within 2 s (a wireless keyboard waking) changes
   nothing: the input layer holds a removal back that long (ask 2).
4. A forced `shell.form` never changes with the devices.

## 11. The desktop form on a touch screen (parent 3.6)

Used when the desktop form is on screen and `input.present` has `touch`. A screen without
touch keeps today's numbers exactly.

| # | Element | Without touch | With touch |
|---|---------|---------------|------------|
| 31 | top bar (macOS and Linux profiles) | 28 | 44; cell tops at y 14 |
| 32 | taskbar (Windows profile) | 40 | 48; each item's target is 48 tall and at least 48 wide |
| 33 | title bar | 28 | 44; the title's cell top 14 px under its top |
| 34 | window controls | 12 px circles | the same circles, each centred in a 44x44 target, the three sharing edges |
| 35 | dock icons (macOS), dash icons (Linux) | 48 in a 56 px pitch | unchanged: already 48 and over |
| 36 | resize zones | 2 px outside the border and 4 inside; corners 16 | 6 outside and 10 inside (4 inside the title bar); corners 32 |
| 37 | top bar chips and menus, popup rows | their own heights | targets 44 tall; popup and menu rows 44 |
| 38 | double tap on a desktop icon | 400 ms within 4 px | 400 ms within 16 px |

The work areas at 1024x768 with touch: Windows (0, 0, 1024, 720); macOS (0, 44, 1024, 652)
with the dock band at (0, 704, 1024, 56); Linux (0, 44, 1024, 724). The toolkit's rows, bars
and buttons are 44 because its input class is touch (parent ask 9).

## 12. What `wm`, `settings` and MCP do in this form (D22)

Every action above has a command and a tool; none is new.

| GUI | CLI (MCP tool, same arguments) |
|-----|--------------------------------|
| a dock or grid tap, a switcher card | `wm focus <id>` (`wm_focus`), or `app open <name>` |
| `Full screen` | `wm maximise <id>` |
| `Split left`, `Split right`, `Swap sides` | `wm move <id> <x> <y>`: `x` left of the work area's middle is the left side, else the right; `y` is ignored |
| `Make wider`, `Make narrower`, a divider drag | `wm size <id> <w> <h>`: `w` snaps to the nearest of one third, a half, two thirds and the full width; `h` is ignored |
| the chooser | `wm move` on the first window, then `wm move` on the second |
| `Home`, `Back` | `wm home`, `wm back` |
| sending one side home | `wm minimise <id>` |
| `Close`, `Close all` | `wm close <id>` |
| any shell button | `wm show shell`, `wm act shell <element> invoke` (`wm_show`, `wm_act`) |

- `wm move` and `wm size` answer with the rect the window got. `wm move` on a full window
  with no pair opens the chooser on the other side, exactly as the menu does; on a window
  outside the shown pair it takes that side and the window that was there is `minimised`.
- At a width with no thirds, `wm size` snaps to the half or the full width.
- `wm move` and `wm size` on a dialog are refused with `fixed_by_form`: `wm move: the tablet
  form places a dialog on its window. Move the window instead.`
- `wm list` needs no new column: `maximised` is full, `shown` with a rect is a side,
  `minimised` is in the switcher.
- `wm click`, `wm drag`, `wm scroll`, `wm type` and `wm press` reach a window on either side.
  A `wm drag` that starts on the divider's target drags the divider.
- **Settings**: no new key. `settings set shell.form tablet` forces this form where the short
  side is at least 600. The Display pane's `Layout` line reads `Now: tablet, chosen from a
  1024 x 768 touch screen.` or `Now: desktop, chosen because a keyboard and a pointer are
  attached.`

## 13. Accessibility: the shell's tree in this form

`wm show shell` lists these; a name never depends on a colour or a place.

| Role | Name | Value | Notes |
|------|------|-------|-------|
| `button` | `Status bar` | | `expanded` while the shade is open |
| `text` | `Clock`, `Network`, `Battery`, `Agent` | as the phone's | children of the status bar |
| `button` | `Window menu of <title>` | `full`; or `left, one third`, `right, a half` and so on | one per shown window; `focused` on the focused one; `expanded` while its menu is open |
| `menuitem` | `Full screen`, `Split left`, ... | | listed while a menu is open; `disabled` when not applicable |
| `button` | `Back`, `Home`, `Apps` | | `Back` is `disabled` when there is nothing to go back to |
| `list` | `Dock` | `4 apps` | |
| `button` | the app's name | `running`, `focused`, or empty | a child of `Dock`; `invoke` is the tap |
| `slider` | `Divider` | `one third`, `a half` or `two thirds` (the left side's share) | listed while the pair is shown; `set-text` with one of the three values moves it |
| `list` | `Choose an app for this side` | `2 windows, 7 apps` | listed while the chooser shows; its cards are `button`s named by the window's title, its cells `button`s named by the app |
| `field`, `list` | `Search apps`, `Apps` | as the phone's | on the home screen |

- The icon menu's entries are `menuitem`s named `Open`, `Split left`, `Split right` and
  `Close` under the app's button. Switcher cards are as the parent says; the pair's card is
  named `<left title> + <right title>`.
- State is never colour alone: the focused side has the line and the brighter title; the
  focused app the line and the mark; a divider being dragged the lighter bar and the brighter
  grip; a menu entry that does not apply is also `disabled` in the tree.
- Contrast uses only the parent's pairs (its section 6). `text.dim` is never drawn on
  `selection`, `bar.hover` or `bar.pressed`: a pressed or hovered title, and a dragged grip,
  turn `text`.
- No drag is the only way: the divider has its keys, the menu entries and `wm size`.
  No timed gesture is the only way: the icon menu has the Menu key and its entries are in the
  window menu.

## 14. Acceptance for slice 3

Host tests in the existing style. Unless a line says otherwise: 1024x768, scale 1, the dark
palette, `shell.form` `tablet`, an adapter, no battery, a pointer and a keyboard present, no
touch screen (the pointer is the finger), Terminal, Files and Settings installed.

1. **The frame**: `layout_in_form(1024, 768, 1, profile, Tablet)` gives the status bar
   (0, 0, 1024, 44), the work area (0, 44, 1024, 660) and the dock (0, 704, 1024, 64) in all
   three profiles; at 1366x1024, (0, 0, 1366, 44), (0, 44, 1366, 916), (0, 960, 1366, 64); at
   768x1024 the work area is (0, 44, 768, 916). Pixel (300, 0) is `bar.bg`, (300, 43) and
   (300, 704) `bar.edge`, (300, 705) and (300, 767) `bar.bg`.
2. **The split's sizes**: for `W` 1024 a third is 336, a half 508, two thirds 680; for 1366,
   452, 679 and 906; for 768 only the half, 380. In every case left + 8 + right is `W`.
3. **The dock, nothing open**: the `←` cell is (24, 720, 16, 32) in `text.dim`; the egg's top
   row is at y 726 centred on x 96 in `accent`; the square's outline is (153, 729, 14, 14) in
   `text`. The icons are at (424, 710), (488, 710) and (552, 710); no pixel of the dock from
   x 608 on, or between x 192 and 415, differs from `bar.bg` below its edge row. At 1366x1024
   the icons are at (595, 966), (659, 966) and (723, 966).
4. **The dock with a fourth app running**: the targets start at x 375, 439 and 503; the
   separator's line is (575, 716, 1, 40) in `menu.border`; the fourth icon is at (592, 710)
   with its mark at (614, 760, 4, 4) in `text`. With ten unpinned apps running, six are
   drawn (nine in all) and the four least recently shown are not.
5. **The home screen**: the search field is (272, 60, 480, 44) with `Search apps` from
   (284, 74) in `text.dim`; Terminal's icon is at (96, 128), Files' at (208, 128), Settings'
   at (320, 128); a ninth app's is at (96, 232). At 1366x1024 the field is (443, 60, 480, 44)
   and Terminal's icon is at (99, 128). Typing `fin` leaves one row, (272, 120, 480, 48), in
   `selection`, its icon at (284, 128) and `Files` from (324, 136).
6. **Opening**: a press and release at (448, 736) starts Terminal. When it maps, `wm list`
   shows it `maximised` at (0, 44, 1024, 660); its title button is (464, 0, 96, 44) with
   `Terminal` from (473, 14) in `text`, the caret's top row (543, 20, 7, 1) and the line
   (473, 34, 64, 2) in `accent`; the dock has the mark (446, 760, 4, 4) and the line
   (440, 764, 16, 2); no pixel of the work area is title bar, border or shadow.
7. **The window menu**: a click at (512, 20) opens the popup (464, 48, 200, 193); `Full
   screen` is drawn in `text.dim` from (480, 66); the button is `bar.pressed`. Down selects
   `Split left` (the row (464, 96, 200, 44) is `selection`). Esc closes it. A click at
   (700, 20) opens the shade at (634, 44), not a menu.
8. **Split left and the chooser**: with Terminal full and Files minimised, `Split left` puts
   Terminal at (0, 44, 508, 660) (`shown`), its title button at (206, 0, 96, 44); the divider
   (508, 44, 8, 660) is all `bar.bg`; the heading starts at (532, 60) in `text.dim`; Files'
   card is (538, 92, 144, 128) in `selection` (the first stop); the first cell's icon is at
   (578, 244). Esc makes Terminal full again and the chooser is gone.
9. **Choosing**: a click on Files' card puts Files at (516, 44, 508, 660), focused. Its title
   button is (722, 0, 96, 44), `Files` from (743, 14) in `text` with the line (743, 34, 40, 2);
   Terminal's title is `text.dim` with no `accent` pixel under it; the grip (510, 350, 4, 48)
   is `text.dim`. A click on Terminal's cell in the chooser instead starts a second Terminal
   window on the right.
10. **Make narrower, Swap sides, Full screen**: `Make narrower` on Files gives Terminal
    (0, 44, 680, 660), the divider (680, 44, 8, 660) and Files (688, 44, 336, 660); a second
    `Make narrower` is not applicable. `Swap sides` then gives Files (0, 44, 336, 660) and
    Terminal (344, 44, 680, 660). `Full screen` on Files gives it the work area and Terminal
    is `minimised`.
11. **Dragging the divider** from a half: button down at (512, 374), moved to (300, 374): the
    divider is drawn at x 296 in `bar.pressed`, and neither window has been sent a new size.
    Button up: Terminal (0, 44, 336, 660), Files (344, 44, 680, 660), each resized once. The
    same drag released at (430, 374) leaves the half; at (700, 374) gives two thirds; at
    (100, 374) leaves Files full and Terminal `minimised`. Esc before the release changes
    nothing. A press at (495, 200) goes to Terminal, not the divider.
12. **Opening while split**: with Terminal left, Files right and Files focused, a click on
    Settings' dock icon puts Settings at (516, 44, 508, 660) and Files is `minimised`. A click
    on Terminal's dock icon only focuses the left side.
13. **Home and the pair**: `wm home` shows the home screen with both windows `minimised`.
    `wm focus <Terminal>` brings both back at their rects with Terminal focused. The switcher
    then lists one card named `Terminal + Settings` at (84, 60, 420, 320) and Files' card at
    (520, 60, 420, 320). `wm close <Settings>` leaves Terminal full.
14. **A narrow side**: Settings on a 336 px side lays out in one column; on a 680 px side in
    two. A 320x160 dialog of a window on a 336 px side is a sheet at the side's bottom, 336
    wide; on a 680 px side it is centred on the side.
15. **Keys**: with Terminal full and focused, Super+Left opens the chooser as in 8 and Enter
    chooses the first card; Super+Shift+Left then gives the left side one third, and a second
    press changes nothing; Super+Right on the left window swaps the sides; Super+Up makes it
    full; Super+Down shows the home screen; Alt+Space opens the window menu; F6 puts the ring
    on Terminal's title button, and Tab reaches `Status bar`, `Back`, `Home`, `Apps`, the
    dock's apps and, while split, the divider, where Right moves it one place.
16. **`wm`**: `wm move <Terminal> 0 0` on a full Terminal answers (0, 44, 508, 660) and the
    chooser shows; `wm move <Files> 900 0` answers (516, 44, 508, 660). `wm size <Files> 300
    500` answers (688, 44, 336, 660); `wm size <Files> 600 1` answers (344, 44, 680, 660);
    `wm size <Files> 900 1` makes it full. `wm move <Terminal> 900 0` while split swaps the
    sides. `wm move` on a dialog fails with `fixed_by_form`. At 768x1024 `wm size <id> 200 1`
    answers a 380 px side.
17. **The tree**: `wm show shell` while split lists `Status bar`, `Clock`, `Window menu of
    Terminal` valued `left, a half`, `Window menu of Files` valued `right, a half` and flagged
    `focused`, `Back`, `Home`, `Apps`, the `Dock` list valued `3 apps` with `Terminal` and
    `Files` valued `running` or `focused`, and the `Divider` valued `a half`. `set-text`
    `one third` on the divider gives the left side 336. `invoke` on `Window menu of Terminal`
    lists seven `menuitem`s, and `invoke` on `Swap sides` swaps. While the chooser shows, the
    list `Choose an app for this side` is there and `invoke` on a card fills the side.
18. **The change to the desktop**: `shell.form` `auto`, inputs `touch`: the form is `tablet`.
    With Terminal and Files in halves, adding a keyboard changes no pixel outside the ring.
    Adding a pointer makes the form `desktop`: in the Linux profile the work area is
    (0, 44, 1024, 724), Terminal's outer rect is (0, 44, 512, 724) and Files' (512, 44, 512,
    724), neither maximised, and the notice reads `Desktop layout: keyboard and pointer
    attached.` Removing the pointer makes it `tablet`: the focused window is at (0, 44, 1024,
    660), the other `minimised`, and the notice reads `Tablet layout: keyboard or pointer
    removed.` With `shell.form` `tablet` neither change happens.
19. **Touch sizes on the desktop**: with `touch` present, the desktop layout at 1024x768 has a
    taskbar of (0, 720, 1024, 48) in the Windows profile and a top bar of (0, 0, 1024, 44) in
    the other two; the work areas are (0, 0, 1024, 720), (0, 44, 1024, 652) and (0, 44, 1024,
    724); a window's title bar is 44 tall and a press 20 px left of the close circle's centre
    still closes. Without `touch`, every existing desktop test passes value for value.
20. **Upright**: at 768x1024 the dock holds five apps, the home grid has six columns from
    x 48, `Split left` gives (0, 44, 380, 916) and (388, 44, 380, 916), and `Make wider` is
    not applicable.
21. **2x**: at 2048x1536 device px and scale 2 every rect above doubles (the dock is 128
    device px tall, a half is 1016).
22. **The phone and the desktop are untouched**: every test of the parent's section 10 passes.

## 15. Backend asks

For the kernel and the drivers:

1. **The layout.** `layout_in_form` gives the tablet its `status_bar`, a new `tablet_dock`
   rect and the work area, and takes whether touch is present so the desktop form can use
   section 11's sizes. `Form::fixes_windows` stays false for the tablet; a new window rule
   (`FullOrSplit`) tells the server no window floats.
2. **Input kinds, debounced.** The parent's ask 1, plus: a removed device is reported only
   after it has been gone 2 s.
3. **The pair in the server.** The left and right window ids, the divider's place and whether
   the pair is shown, kept in the desktop's state and across a change of form; one `Configure`
   per window when a place changes, none during a divider drag.
4. **Chrome by form.** The server's title height, control targets and resize zones come from
   one metrics value (touch or not) instead of constants, so section 11 is a change of that
   value.
5. **`SHELL_NAVIGATE`** gains the tablet's actions for the tree and the keys: split left,
   split right, full, wider, narrower, swap.
6. **`wm move` and `wm size`** in the tablet form snap as section 12 says and return the
   rect; `fixed_by_form` for dialogs.
7. **The shell's tree** (`wm show shell`, not built in slice 1) with section 13's elements,
   and the `slider` role's `set-text` on the divider.
8. **The touch path** (the parent's ask 2) for real tablets. Slice 3 itself runs in QEMU with
   the pointer as the finger.

For the toolkit:

9. The width class from the window's own width (the parent's ask 9), so a 336 px side is one
   column and a 680 px side two, and a new size after a divider move lays out again.
10. Dialog and menu rows at 44 when the input class is touch, from the one constant of the
    parent's ask 11.

## Decided by design (the owner can overrule any of these)

- The dock is one fixed 64 px bar: navigation at the left, the apps centred, nothing at the
  right. It does not hide, and it holds 9 apps at 1024 and 14 at 1366.
- A title button carries a small caret so it reads as a menu; the rest of the status bar
  still opens the shade.
- The chooser always opens at a half, shows open windows first and every app under them, and
  an app whose only window is on the other side opens a second window.
- The divider takes only a 44x88 pad from the apps, not a full-height 44 px band; windows are
  resized once on release, never while the finger moves.
- A key never closes a side: only a drag to within 160 px of an edge, `Full screen`, `Close`
  or `wm minimise` ends a pair.
- One pair at a time; it survives `Home` and is one card in the switcher, whose `Close`
  closes both windows.
- No new `wm` verb and no new setting: `wm move` picks a side or swaps, `wm size` picks the
  share, `wm maximise` is `Full screen`. Super+Shift+Left and Right are the divider's keys.
- Going to the desktop keeps the pair as two tiled windows; coming back makes the focused
  window full and does not rebuild the pair. A device gone for under 2 s changes nothing.
- On a touch desktop the taskbar is 48 and the other bars and title bars are 44; a screen
  without touch keeps every number it has today.
