# EggOS v0.5 Settings on every device class: design brief

Status: design for v0.5; D23 (every device class), D22 (GUI, CLI and MCP), D401 (the registry
pane's drill-down), D303 (the shell's forms).

Design by Claude Fable (D16) for how the Settings window takes the phone, tablet, TV and
wearable forms of `adaptive-shell-v0.5.md` and its tablet and TV briefs. **The desktop form
does not change**: the 176 px sidebar of 32 px rows, the search field, the pane to its right,
every pane's absolute layout for a 464 px wide pane. This brief adds one window shape for
narrow screens (two pages), one touch size for the tablet, the TV's focus rules, a read-only
wearable, and one **reflow rule** that turns any pane's desktop layout into a one-column
layout, so the twelve panes need no second hand-made design each. Where a pane already has a
device-class section of its own (`components-v0.5.md` section 10, `display-rotation-v0.5.md`
section 5, `windows-registry-v0.3.md` section 7), that section stands and this brief only
names it. Tokens, fonts and spacings are the desktop's (`desktop-v0.3-gui.md`,
`toolkit-v0.5.md`): nothing new is drawn in colour, and nothing moves.

Numbers are px at 1x in window coordinates unless a line says pane coordinates (origin the
pane panel's top left). `W` is the window's content width, `H` its height, `C` a narrow pane's
content width (section 2). On a TV at scale 2 every number doubles on the panel.

## 0. What a person gets, and how that is checked

- On a phone, Settings is a list of panes with a one-line summary under each name; a tap opens
  the pane as a full-width page with a top bar (`←`, the pane's name), and `Back` returns.
  Every control is at least 48 px tall and full width, so nothing needs a precise finger.
- On a tablet, the desktop's sidebar and pane with 40 px targets; in a narrow split side the
  phone's pages.
- On a TV, the desktop layout at scale 2 with the focus ring always drawn and the remote's
  arrows, OK and Back doing everything.
- On a wearable, the same list, read-only: the facts, and where to change them.
- `settings open <pane>`, `settings_open` and `wm act` work in every form: an agent and a
  screen reader see the list, the back button and every control by name (section 3).
- Checked by host tests at 390x752 (phone), 1024x660 and 336x660 (tablet), 960x540 at scale 2
  (TV) and 320x320 (wearable) (section 4).

## 1. The window in each form

The window learns its form from `GET_FORM` (`shell.form_in_use`) and its width from its own
size, in `SettingsWindow::set_form_factor(form, width)`, which chooses one of two **shapes**:

| Shape | When | What |
|-------|------|------|
| `Shape::Sidebar { row }` | `Desktop` (row 32); `Tablet` (row 40); `Tv` (row 32) | the sidebar beside the pane, as today |
| `Shape::Pages` | `Phone` and `Wearable` always; `Tablet` or `Desktop` with `W` < **624** (`PAGES_BELOW`: 176 + 448, the narrowest pane the desktop layouts were drawn for) | the list page, or the pane page |

A size change (a divider move, a rotation, the on-screen keyboard) runs it again; the pane
shown and the search text are kept across the change.

### 1.1 Phone: the list page and the pane page (390x752 portrait, 796x346 landscape)

The window's content is the work area; the shell draws the status bar and the navigation bar.

**The list page** (`Page::List`):

| # | Element | Rect | Text and colour |
|---|---------|------|-----------------|
| 1 | search field | (16, 8, `W` - 32, 48) | `Search settings`, `Escape::Clear`; the desktop's field, 48 tall |
| 2 | pane list | (0, 64, `W`, rows x 56) | `Kind::list` with `PAGE_ROW` 56 and, new, an icon and a summary per row (ask 1) |
| 3 | row icon | (16, row + 16), 24x24 | `art::pane_icon(pane)`, 2 px strokes in `text` (table below) |
| 4 | row name | cell top row + 10, from x 56 | the pane's name in `text` |
| 5 | row summary | cell top row + 30, from x 56, ends at `W` - 48 | `pane_summary(pane, &facts)` in `text.dim`, cut with `...` |
| 6 | row chevron | `>` glyph, cell (`W` - 32, row + 20) | `text.dim`; decoration, not an element |

- Twelve rows end at y 736 and fit 752; without the Windows layer eleven. In landscape (346
  tall) the list scrolls in the window's scroll view, wheel and pan 56 px a row. No row is
  marked at rest (nothing is shown beside the list); a pressed row fills `selection` while the
  finger is down. A tap, Enter or Space on a row opens its page.
- The search filters the rows as on the desktop (`pane_matches`); a focused field shows the
  on-screen keyboard (parent 2.9) and the list scrolls in what is left. `Back` with text in
  the field clears it (`Escape::Clear`); with an empty field the window reports
  `BACK_UNUSED` and the shell shows the home screen.
- **Summaries** come from `Facts` the window already holds, so showing the list reads nothing
  new; each is at most 40 characters:

  | Pane | Summary | Pane | Summary |
  |------|---------|------|---------|
  | Appearance | `macOS layout, Dark` | Storage | `2 disks, 1.2 GB free`; `No disk attached` |
  | Network | the indicator's state words (`network-v0.3.md` 4): `Connected to Home`, `Offline` | Date & time | `Automatic, Europe/Berlin`; `Set by hand, UTC` |
  | Display | `1280 × 800, scale 1` | Agents | `2 agents`; `No agents` |
  | Keyboard | the layouts in order: `US, German` | Windows | `Registry of Windows programs` |
  | Mouse | `Right hand, natural scrolling` | Components | `3 installed, 2 updates`; `Nothing installed` |
  | Accessibility | `High contrast, large text`; `Default` | About | `EggOS 0.5.0, x86_64` |

- **Icons** (24x24, 1x, `text`, no fill): Appearance a 16x12 window outline with a 3 px title
  band; Network three nested arcs over a dot; Display a 18x12 screen on a 6 px stand; Keyboard
  a 20x12 outline with two rows of 2x2 keys; Mouse a 10x16 rounded outline split by a 1 px
  line; Accessibility a circle head over a stick figure with arms out; Storage a 18x8 drum
  over a second; Date & time a 16 px circle with hands at 10 and 2; Agents the egg mark H 16
  with a 4x4 dot; Windows four 7x7 squares in a 2x2 grid; Components a 12x12 square with a
  second square's corner behind it; About a circle with an `i`.

**The pane page** (`Page::Pane`):

| # | Element | Rect | Text and colour |
|---|---------|------|-----------------|
| 7 | top bar | (0, 0, `W`, 56) | `dock.bg`; row 55 `menu.border` |
| 8 | back button | (0, 0, 56, 56) | `←` (U+2190), cell (20, 20), `text`; pressed `bar.pressed`; named `Back` |
| 9 | title | cell top 20, from x 56, ends at `W` - 16 | the pane's name (`PANES[pane]`) in `text` |
| 10 | scroll view | (0, 56, `W`, `H` - 56) | the pane's panel at (0, 0, `W`, its narrow height), laid out by section 2 |

- The pane keeps its own heading at pane (16, 16) under the bar: the bar names the page while
  the heading scrolls away, as Android's bar and iOS's large title do. Decided for, over
  hiding the heading, because every per-pane phone layout already written starts at y 64.
- **Back**, from the arrow, the shell's `Back`, Esc or Backspace with no field focused, goes
  to the first of these: an open popup, sheet or note closes; the pane's own back acts (the
  registry's `..`, one level up); the list page shows with the opened row focused and the
  list's scroll kept; then the list page's rule above. `pane()` still answers the pane last
  shown while the list page shows.
- `settings open <pane>` and `--pane` show that pane's page first; `Back` from it goes to the
  list. Showing a pane that asks the program for a read (Storage, Agents, About, Components)
  asks as on the desktop.

### 1.2 Tablet (1024x660 full, 336, 508 and 680 px sides)

- **Sidebar**: 176 wide as today; the search field (8, 8, 160, 40); the list at y 56 with
  rows of **40** (`SIDEBAR_ROW_TOUCH`); twelve rows end at 536. Selected and pressed rows as
  the desktop's.
- **The pane**: the desktop layout on a **48 px pitch with 40 px controls**: every 40 px row
  becomes 48 (its label's cell top +16 instead of +12, so it stays centred), every 28 px
  control (button, field, dropdown) 40 tall, list and table rows 40 (headers 40), cards and
  sliders unchanged, lines 20 apart as before. This is what `display-rotation-v0.5.md` 5 and
  `components-v0.5.md` 10 already give for their rows; the registry pane's tablet layout
  stands. Pane heights grow by 8 a row and scroll in the window's scroll view.
- **A narrow window** (`W` < 624: a 336 or 508 px side at 1024, a 452 px side at 1366) takes
  `Shape::Pages` exactly as the phone, 48 px targets included: the screen is touch and the
  pane is narrow. A 680 px side (pane 504) keeps the sidebar.
- The desktop form on a touch screen (parent 3.6) uses the tablet's sizes too: the sizes
  follow the input class, the shape follows the width.

### 1.3 TV (960x540 logical at scale 2, D-pad)

- The toolkit's root padding puts the content at (48, 28, 864, 484); inside it the desktop
  layout: sidebar (48, 28, 176, 484), search (56, 36, 160, 28), list rows of 32 from y 72
  (twelve end at 456), the pane at (224, 28, 688, 484) with right-anchored values and
  controls at its own right edge (x 896 - 16), as on a resized desktop window. Rows and
  controls keep the desktop's sizes: scale 2 makes them readable.
- **Focus** starts on the sidebar's selected row (`Appearance`, or the pane `settings open`
  named), not in the search field: typing is rare on a remote. The ring (2 px `focus.ring`)
  is always drawn, on exactly one widget.
- **Keys**: Up/Down in the sidebar move the selection and show the pane (the list's `Chosen`,
  as on the desktop); Up from the first row goes to the search field, Down from the field
  back. **Right** from a sidebar row puts the ring on the pane's first focusable widget (its
  Tab order's first; a pane with none keeps the ring on the sidebar). Inside the pane the
  toolkit's nearest-widget rule moves the ring; **Left** from a widget with nothing to its
  left returns to the sidebar's selected row. **OK** is Enter (presses, opens a dropdown's
  popup, toggles a checkbox); in a popup Up/Down move, OK chooses, Back closes with no write.
  **Back** closes a popup, sheet or note; else from the pane returns the ring to the sidebar;
  else from the sidebar the window reports `BACK_UNUSED` and the home screen shows. Menu does
  nothing (no menu bar). A disabled button's reason shows as its tooltip 600 ms after it
  takes the ring. An attached keyboard works as on the desktop.
- Where a pane's own brief gives TV keys (Components 10, Rotation 5, the registry 7), those
  stand inside that pane.

### 1.4 Wearable (320x320 square, or round with a 24 px inset; read-only)

- **The list page**: no search, no icons; rows of **48** from y 0, `W` wide: the name at cell
  top row + 6 from x 16 in `text`, the summary of 1.1 at row + 26 in `text.dim`; twelve rows
  scroll by pan. A tap or Enter opens the pane's page.
- **The pane page**: a bar (0, 0, `W`, 48) in `dock.bg` with `←` (0, 0, 48, 48), glyph cell
  (16, 16), and the name at cell top 16 from x 48; under it the pane's **facts as lines**, 20
  apart from cell top 56, x 16, width `W` - 32: each desktop `label: value` row as `Resolution
  1280 × 800`, each state line as it reads on the desktop, and last, in `text.dim`, `Change
  these from your phone or the CLI.` No control exists in the tree. The Components pane's
  lines are its section 10's. Back returns to the list.
- Decided for read-only, because every write here has a wider front-end (the phone, the CLI,
  MCP) and a 320 px screen cannot show a dropdown's five options and the line that explains
  them at once.

## 2. The narrow pane: one reflow rule

Applies to the pane page (phone, wearable's facts aside, and a tablet or desktop window under
624). Pane coordinates; `C` = min(`W` - 32, 480) (`PAGE_CONTENT_MAX`: a 764 px dropdown in
landscape helps no one; the column stays left-aligned at x 16). Targets are 48 tall
(`TOUCH_CONTROL`); 8 px between a control and what follows, 16 between blocks, 24 from a label
to its control. The pane's heading stays at (16, 16) and the first block at y 64. The pane's
panel is as tall as its last widget's bottom + 16 and scrolls in the window's scroll view,
which keeps the focused widget in view. `set_form_factor` applies the rule with `set_place`
from one table per pane, (desktop place, narrow place) per widget, as the Components pane
moves its blocks for a note.

| Rule | Desktop shape | Narrow shape |
|------|---------------|--------------|
| R1 read-only row | label at (16, r + 12), value right-aligned at the pane's right - 16, pitch 40 | the same, the value right-aligned at 16 + `C`; pitch 40 (nothing to press) |
| R2 label and control | label at x 16, a control 28 tall at x 288 (144, 242, ...) on the same row | label cell top at `r`; the control at (16, `r` + 24), `C` x 48; the next block at `r` + 80; a status line under it at cell top `r` + 84, then the next block at `r` + 120 |
| R3 field and button on one row | field at x 144, button at x 352 | the field (16, `r`, `C`, 48), the button (16, `r` + 56, `C`, 48), the next block at `r` + 112 |
| R4 buttons sharing a row | two to four buttons 96x28 side by side | stacked, (16, `r` + 56 k, `C`, 48), the primary (default) button last; a pair of equals with no primary shares one row, each (`C` - 8) / 2 wide at x 16 and 24 + (`C` - 8) / 2 |
| R5 two columns | a list at x 16..224 with lines or controls from x 240 | the left column first, `C` wide; the right column's blocks start 16 under the left column's last widget, each by R1 to R4; a row that spans both columns follows |
| R6 lists, outlines, tables | rows 24 (table rows 24, header 28), 208 wide | `C` wide, rows 48, header 48, as tall as its rows up to 6 (then it scrolls); a table wider than `C` pans sideways by swipe |
| R7 cards and radios | three 128x112 cards 16 apart; radios in a row | cards in a column, each (`C`, 112), 8 apart, the miniature centred (`Kind::cards` with a column option, ask 2); radios that do not fit `C` in one row go one per 48 px line |
| R8 checkbox, slider | inline 28 tall | a 48 px row whose whole width is the target; a slider `C` x 48 with its label 24 above (R2) |
| R9 lines | `text.dim` or `text` lines, 432 wide, 20 apart | x 16, width `C`, 20 apart; a line wider than `C` wraps to a second line (ask 3) and what follows moves down 20 |
| R10 panels and notes | a panel inside the pane (format, install, grant, edit) | a **bottom sheet** (`desktop-v0.3-gui.md` 12): `W` wide at the window's bottom, a 1 px `menu.border` top edge, 16 px padding, controls full width 48 tall, `Cancel` and the primary sharing the last row by R4's pair; Back closes it. A note under a row stays inline (Components 10) |

**Worked example, the Keyboard pane at `W` 390 (`C` 358)**: heading (16, 16); the layouts
list (16, 64, 358, 192), rows 48; `Add a layout...` (16, 264, 358, 48); `Move up` (16, 320,
175, 48) and `Move down` (199, 320, 175, 48); `Remove` (16, 376, 358, 48); the third-level
line at cell top 440 and the dead-keys line at 460 (R5: the right column follows); the try
field (16, 488, 358, 48); the keyboard line at 552; `Switch layouts with` at cell top 592
with its dropdown (16, 616, 358, 48); `On-screen keyboard` at 672 with its dropdown (16, 696,
358, 48); the panel 760 tall. The same arithmetic gives every other pane.

**Which rules each pane uses, and the exceptions:**

| Pane | Narrow layout |
|------|---------------|
| Appearance | R7 cards (64 to 416), then R1 rows from 432 and the theme radios on one row |
| Network | R1 rows and R9 lines as today; Wi-Fi rows **56 tall** with the name and strength only: a tap on the row does what `Join` does (the button is hidden), and the join note is a sheet (R10). The `Open Components` button `C` x 48 |
| Display | `display-rotation-v0.5.md` section 5's numbers (R2 throughout) |
| Keyboard | the worked example (R5, R4 pair, R6, R2) |
| Mouse | R2 for the sliders and the hand and scrolling radios (R7's one-per-line when they do not fit) |
| Accessibility | R2 sliders, R8 checkboxes |
| Storage | R5: the disk list, then the facts lines, the usage bar (16, y, `C`, 8), then two pairs by R4 (`Check` `Repair`, `Format` `Install EggOS...`), the status line and report lines (R9); the format and install panels R10 |
| Date & time | R1 rows; the set field and `Set` by R3; the zone picker's list by R6 |
| Agents | R5: the agent list, the view radios one row; the id field and `Add` by R3; `Revoke all` and `Forget` a R4 pair; the class and lasts radios one per line; the grant panel R10 |
| Windows | **its own**: `windows-registry-v0.3.md` section 7, the drill-down of D401, unchanged |
| Components | **its own**: `components-v0.5.md` section 10 (bands 132 tall, rows 128, notes inline), unchanged |
| About | the egg and its lines as today; the checkbox R8; the devices table R6, panned sideways |

Nothing in a pane is hidden on a narrow screen except the Wi-Fi rows' `Join` (the row is the
target): every setting reachable on the desktop is reachable on the phone.

## 3. Keys, focus and what an agent sees (D22, D161)

- **`settings open <pane>`**, `--pane` and the MCP `settings_open` work in every form: on the
  desktop, tablet and TV they select the sidebar row and show the pane; on a phone or a
  wearable they show the pane's page. `pane()` answers the pane in every shape. The shade's
  tiles (`settings open network`) and the TV's pane tiles are this call.
- **Keyboard only, every form**: list page Tab order: the search field, the list (Up/Down move,
  Enter opens, typing a letter jumps to the row); pane page: `Back`, then the pane's own Tab
  order; Esc and Backspace with no field focused are Back. Sidebar shapes keep the desktop's
  order. No step needs a pointer, a hover, a gesture or a timed press: a long-press tooltip on
  the phone and the focus tooltip on the TV only repeat what a disabled button's reason also
  says in the pane's line.
- **The element tree** (`wm show`, a screen reader), the shown page only (a hidden page's
  widgets are not listed):

  | Role | Name | Value | Notes |
  |------|------|-------|-------|
  | `field` | `Search settings` | its text | list page; `set-text` filters |
  | `list` | `Settings` | `12 panes` (`11 panes`) | list page; sidebar shapes keep today's list |
  | `list-item` | the pane's name | the summary | child of `Settings`; `select` or `invoke` opens the page |
  | `button` | `Back` | `Settings` | pane page; `invoke` returns to the list |
  | `label` | the pane's name | | the bar's title; the pane's heading is listed as today |
  | the pane's widgets | as today | | unchanged roles, names and values |

  A wearable's facts lines are `label`s with their drawn text. The TV's tree is the desktop's;
  exactly one element is flagged `focused`.
- **Agents and the phone's `Back`**: `wm back` (parent 5) presses it; an agent that wants a
  pane calls `settings_open`, not `Back` twice. Nothing here is person-only, and no action here
  is destructive: the confirmations that are (Format, Remove) keep their own rules.
- **Search words** are unchanged; the home screen's `Search apps` finds Settings' panes by the
  same words in every form (parent 2.4).

## 4. Acceptance (host tests, scale 1x unless said, window coordinates, `Facts::example()`, the
Windows layer installed)

1. **Shape**: `set_form_factor(Phone, 390)`, `(Wearable, 320)`, `(Tablet, 336)`, `(Tablet,
   508)` and `(Desktop, 600)` give `Shape::Pages`; `(Tablet, 680)`, `(Tablet, 1024)` and
   `(Desktop, 640)` give `Shape::Sidebar` with row 40, 40 and 32; `(Tv, 960)` row 32.
2. **Phone, the list page** at 390x752: the search field (16, 8, 358, 48); the list (0, 64,
   390, 672) with 12 rows of 56; row 2's icon at (16, 192), `Display` at cell top 186 from x 56,
   `1280 × 800, scale 1` at 206 in `text.dim`, the `>` at (358, 196). Without the Windows layer
   11 rows and `Components` at y 568. At 796x346 the list is 672 tall in a scroll view 282 tall.
3. **Opening a pane**: a tap at (195, 200) shows the pane page: the bar (0, 0, 390, 56) with
   `←` cell (20, 20) and `Display` at cell top 20 from x 56; the scroll view (0, 56, 390, 696);
   the heading at (16, 72); `Rotation` at (16, 352) and its dropdown (16, 376, 358, 48)
   (`display-rotation-v0.5.md` 9, 56 lower); `pane()` is `DISPLAY`. `Back` returns to the list
   with row 2 focused and asks nothing; `Back` again with an empty search reports
   `BACK_UNUSED`; with `dis` in the field, `Back` clears it first and the list shows one row.
4. **Keyboard pane at 390**: the worked example's twelve rects of section 2, and a panel 760
   tall; Tab from `Back` goes to the layouts list, then `Add a layout...`, `Move up`, `Move
   down`, `Remove`, the try field, the two dropdowns.
5. **Storage at 390** with two fake disks: the list (16, 64, 358, 96); `Check` (16, y, 175,
   48) and `Repair` (199, y, 175, 48) on one row, `Format` and `Install EggOS...` on the next,
   56 lower; `Format` opens a sheet at the window's bottom, 390 wide, `Cancel` and `Format`
   each 175 wide on its last row; Back closes it.
6. **Network at 390** with one fake network: its row 56 tall, no `Join` button, a tap on the
   row asks for the join as the desktop's `Join` does; the join note is a sheet.
7. **Windows and Components at 390**: the registry's outline at (16, 148) one level a page
   (D401), the Components bands at 124 and 264 and rows from 388 (`components-v0.5.md` 11):
   this brief moves neither.
8. **`settings open network` on the phone** shows the Network page first; `Back` shows the
   list. The tree lists `list` `Settings` valued `12 panes` with a `list-item` `Display` valued
   `1280 × 800, scale 1` on the list page, and `button` `Back` with the pane's widgets on the
   pane page; `invoke` on `Back` returns to the list.
9. **Tablet at 1024x660**: the search (8, 8, 160, 40); rows of 40 from y 56, `Display` at y
   136; the Display pane's `Rotation` dropdown at (464, 256, 160, 40), `Pointer` at cell top
   364 (`display-rotation-v0.5.md` 5). At 336x660: the phone's pages, the search (16, 8, 304,
   48), `Move up` 148 wide at x 16 and `Move down` at x 172.
10. **TV at 960x540, scale 2, root content (48, 28, 864, 484)**: the sidebar (48, 28, 176,
    484), the search (56, 36, 160, 28), `Appearance` row at (48, 72, 176, 32) focused with the
    ring, the pane at x 224, 688 wide, a Display value right-aligned ending at x 896. Right puts
    the ring on the profile cards; Left returns it to the row; Down selects `Network` and
    `pane()` is `NETWORK`; Back on a pane widget returns the ring to the sidebar; Back on the
    sidebar reports `BACK_UNUSED`; OK on the `Rotation` dropdown opens its popup, Down then OK
    asks for `SetDisplayRotation(1)`, and Back on an open popup asks nothing. Exactly one
    element is drawn with the ring at every step.
11. **Wearable at 320x320**: 12 rows of 48 in a scroll view 320 tall, `Display` at y 96 with
    its name at cell top 102 and summary at 122; a tap shows the bar (0, 0, 320, 48), `←` at (16, 16), `Display` from x 48,
    `Resolution 1280 × 800` at cell top 56, the last line `Change these from your phone or the
    CLI.` in `text.dim`; the tree holds no `combo-box`; Back returns to the list.
12. **Every pane at 390** holds every widget it holds at 640 except the Wi-Fi `Join` buttons;
    no widget's right edge passes 374 (`W` - 16), no focusable widget is under 48 tall, and
    every pane's panel height is its last widget's bottom + 16 (a test walks all twelve).

## 5. Toolkit and backend asks

1. `Kind::list` rows with an **icon and a second line** (`List::with_icons_and_summaries`),
   row height per list, for the phone's and the wearable's list pages.
2. `Kind::cards` laid out as a **column** of full-width cards with the miniature centred.
3. A label of a given width that **wraps** to a second line, for R9.
4. The window's shape and the pane's width from the window's own size on every form (the
   tablet brief's ask 9), and `set_form_factor` called by the Settings program on `GET_FORM`
   and each resize (D401's "Not yet").
5. The wearable's work area (whether a shell bar exists there) from its own brief; this brief
   lays out from y 0 until then.

## Left out

The wearable's own shell and any write from it; per-pane landscape layouts beyond the content
cap; a collapsing title; search on the pane page; a two-pane split on wide tablets in
portrait; per-pane icons in the desktop's sidebar; voice input for the search; sorting or
hiding panes; the phone's pane list remembering a scroll across sessions.

## Decided by design (the owner can overrule any of these)

- Two shapes, chosen by form and width, with 624 as the one threshold (the sidebar plus the
  narrowest pane the desktop layouts were drawn for).
- The pane page keeps the pane's heading under the bar's title, so every phone layout already
  written stands.
- 48 px targets on a phone and on any narrow page, 40 px on a tablet's sidebar and pane.
- One reflow rule with ten shapes, and per-pane exceptions only where a brief already exists.
- The Wi-Fi row is its own `Join` on a phone.
- The TV's focus starts on the sidebar, enters the pane with Right and leaves it with Left or
  Back.
- The wearable is read-only, lists all twelve panes, and names where to change things.
