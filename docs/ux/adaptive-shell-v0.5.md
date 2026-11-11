# EggOS v0.5 adaptive shell: the phone, tablet and TV forms: design brief

Design by Claude Fable (D16) for roadmap M12's second half: "desktop first, then phone/tablet
and TV" (D23). The desktop shell is built (`desktop-v0.3.md`, `desktop-v0.3-gui.md`,
`desktop-polish-v0.3.md`, `desktops-overview-screenshots-v0.3.md`; `drivers/src/desktop/`). This
brief adds three **forms** to that one shell: it is still one component, one layout function,
one window server, one catalogue and one set of `wm` commands. A form decides which panels
exist, how windows are placed and which input moves the focus; nothing else. Tokens, the 4 px
grid, Egg Mono 8x16 for every shell label, the corner tables, the shadow and the voice are the
desktop's; **no colour token is added** and no motion is added (every surface appears in one
frame, as the launcher and the switcher do).

Numbers are logical px (device px divided by the scale) with the origin at the screen's top
left. The phone form is given element by element for 390x844; the tablet and TV forms are one
level less detailed, and each gets its own numbered brief when its slice is built (section 9).

This brief replaces three earlier sentences, on purpose: the phone's "28 px strip" for the
title and close control (`desktop-v0.3-gui.md` sections 6.1 and 12) becomes the status bar; the
touch target minimum of 40 px used by pane designs so far becomes **44** (section 2.1); and a
touch screen too small for the desktop gets the phone form, not the full-screen console
(section 1.3).

## 0. What a person gets, and how that is checked

- A phone, a tablet and a TV running EggOS each show a shell made for them, chosen by the
  machine without a question, and changeable in one setting.
- The same apps, the same Settings, the same agent: `wm list`, `wm show`, `wm act`, `settings`
  and every MCP twin work in every form (D21, D22), so an agent that learned the desktop
  drives a phone.
- Everything works without a gesture: each swipe has a button that does the same (section 2.8).
- Checked by host tests in the existing style (section 10) at 390x844, and later at 844x390,
  1024x768, 1366x1024 and 1920x1080.

## 1. How the shell picks its form

### 1.1 Inputs

1. **The screen**, in logical px: width `W`, height `H`, the short side `S = min(W, H)`.
2. **The input devices present**, four kinds, each present or not: `touch` (a touch screen on
   this display), `keyboard`, `pointer` (mouse, touchpad, tablet), `remote` (a TV remote: an
   infrared or CEC receiver, or a HID device with a D-pad and no letter keys).
3. **The setting** `shell.form`: `auto` (the default), `desktop`, `tablet`, `phone`, `tv`.

### 1.2 `auto`: the first row that matches

| # | Touch screen | Other input | Screen | Form |
|---|--------------|-------------|--------|------|
| 1 | yes | any | `S` < 600 | `phone` |
| 2 | yes | a keyboard **and** a pointer | `S` ≥ 600 | `desktop`, with touch sizes (section 3.6) |
| 3 | yes | anything else | `S` ≥ 600 | `tablet` |
| 4 | no | a remote and no pointer | `W` ≥ 960 and `H` ≥ 540 | `tv` |
| 5 | no | anything else | any | `desktop` (or the full-screen console, 1.3) |

- **600** is the one breakpoint between phone and tablet (the desktop designs' "phone width
  < 600"), now measured on the short side, so a phone turned sideways (844x390) stays a phone
  and a small tablet turned upright (768x1024) stays a tablet.
- **A keyboard attached to a tablet** (row 3): the form stays `tablet`. The on-screen keyboard
  stops showing, the focus ring is drawn from the first key on, and the desktop's shortcuts
  work with their tablet meaning (section 3.5). Nothing moves on screen.
- **A keyboard and a pointer attached to a tablet** (row 2, a keyboard cover with a touchpad):
  the form becomes `desktop` at once, with the notice `Desktop layout: keyboard and pointer
  attached.` A full window becomes maximised; a split pair becomes two windows tiled to their
  halves of the work area; minimised windows stay minimised. Detaching either returns to
  `tablet`: the focused window becomes full, the others go to the switcher. A person who wants
  one form whatever is attached sets `shell.form`.
- **Sticky phone and TV**: plugging a keyboard or a pointer into a phone or a TV does not
  change the form (rows 1 and 4 are decided by the screen and the remote). On a TV the pointer
  shows while it moves, as `desktop-v0.3-gui.md` section 12 says.
- **When it is decided**: at boot, when the display's size or rotation changes, when a device of
  one of the four kinds is added or removed, and when the setting is set. A change of form
  keeps every window and its state; the layout function runs again and the whole screen is
  damaged once.

### 1.3 Scale, the console, and forms that do not fit

- **Scale** stays the display's (`display.scale`, 2 at a device height ≥ 1440), with one
  addition: the `tv` form uses scale 2 at 1920x1080, so its logical screen is **960x540** and
  every TV number in earlier designs ("everything doubles") holds. A 3840x2160 TV runs
  at 1920x1080 (`display.resolution`) until scale 4 exists; a phone panel of 1170x2532 needs
  scale 3 (section 11).
- **The full-screen console** (`desktop-polish-v0.3.md` section 2) keeps its rule for screens
  without touch. A touch screen gets it only when `W` < 320 or `H` < 480 (too small for the
  phone form).
- **A forced form that does not fit** is saved but not used: `desktop` where the default
  terminal window does not fit, `tablet` at `S` < 600 (the phone form is used), `tv` under
  960x540 (the desktop form is used), `phone` under 320x480. `settings set` then answers the
  existing `not_applied` error (category `failed`) with the reason, for example `shell.form:
  saved, but this screen is 390 wide and the tablet form needs 600. EggOS shows the phone
  form.`
- **A forced touch form on a machine with a pointer and no touch screen** (QEMU, a developer's
  window) works: the primary button is the finger (press is touch down, release is touch up, a
  drag is a pan), the arrow pointer is drawn, and nothing hovers. This is how slice 1 is built
  and tested before a touch driver exists.

### 1.4 The setting and its twins (D22)

| Key | Value | Settable |
|-----|-------|----------|
| `shell.form` | `auto` (the default), `desktop`, `tablet`, `phone` or `tv`; applied at once | yes |
| `shell.form_in_use` | the form on screen now: `desktop`, `tablet`, `phone`, `tv` or `console` | no |
| `shell.navigation` | `buttons` (the default) or `gestures` (section 2.8; slice 5) | yes |
| `input.present` | the kinds present, in this order, separated by commas: `touch,keyboard,pointer,remote`; `none` when empty | no |
| `display.rotation` | `auto` (the default: follows the device's orientation sensor, or `0` without one), `0`, `90`, `180` or `270` degrees clockwise | yes |
| `power.battery` | `none`, or the charge and state: `87% discharging`, `100% charging` | no |

CLI: `settings set shell.form phone`, `settings get shell.form_in_use`, `settings set
--dry-run shell.form tv` (`would set shell.form from auto to tv`). MCP: `settings_set` with
`key` `shell.form` and `value` `phone`; `settings_get`. GUI: Settings > Display gains a row
`Layout` with five radios (`Automatic`, `Desktop`, `Tablet`, `Phone`, `TV`) and, under it in
`text.dim`, `Now: phone, chosen from a 390 x 844 touch screen.` (the form in use and why). The
settings file gets `shell.form=phone` beside `shell.profile`.

`shell.profile` keeps its meaning in every form where it has one: the names in search
(`Explorer`, `Finder`, `Files`), the primary modifier of an attached keyboard, and `Trash`
against `Recycle Bin`. The panels are the form's, not the profile's, outside `desktop`.

## 2. Phone form (390x844, portrait, scale 1)

### 2.1 The frame

| # | Element | Rect | Draws |
|---|---------|------|-------|
| 1 | status bar | (0, 0, 390, 44) | `bar.bg`; row 43 `bar.edge` |
| 2 | work area | (0, 44, 390, 752) | the home screen, or the shown window's content |
| 3 | navigation bar | (0, 796, 390, 48) | `bar.bg`; row 796 `bar.edge` |

- **Touch targets**: every target the shell draws in the phone and tablet forms is at least
  44x44. The toolkit's phone rows, bars and buttons rise from 40 to 44 with it (section 11,
  ask 9), so apps and the shell agree. Targets sit at least 4 px apart or share an edge.
- **Why 44 for the status bar**, not the desktop's 28: it is a button (2.2), and real phone
  panels have a camera cut-out in their top 30 to 50 px (ask 5 gives its rect).
- A window's content is exactly the work area; the server draws no title bar, no border, no
  shadow and no controls for a `Normal` window in this form.

### 2.2 Status bar

Cell tops are y 14 (16 px glyphs centred in 44). Items on the right are a run packed from x 374
leftwards; an item that is absent takes no room.

| # | Element | Place | Text and colour | Shown |
|---|---------|-------|-----------------|-------|
| 4 | clock | (16, 14), 40 wide | `09:14` in `text` (`up 3m` as the top bar's, until the clock is set) | always |
| 5 | title | from (72, 14), ends at most 8 px left of the leftmost of 6 to 9 | the shown window's title in `text`, cut with `...`; empty on the home screen | a window is shown |
| 6 | battery icon | 22x12, right edge 374, top 16 | a 20x12 body with a 1 px `text` outline, a 2x6 `text` nub at its right, and inside it a `text` fill (16 wide at 100 %, at (2, 2), 8 tall) | a battery is reported |
| 7 | battery percent | right edge 4 px left of 6, top 14 | `87%` in `text`; `87%+` while charging | a battery is reported |
| 8 | network indicator | 16x16, right edge 8 px left of 7 (or at 374), top 14 | the bar's glyph of `network-v0.3.md` section 4 | an adapter exists |
| 9 | Agent chip | 56x20, right edge 8 px left of 8, top 12 | `Agent`, idle and acting as `desktop-v0.3-gui.md` 14.1 | an agent is connected |

- In QEMU (no battery, an adapter): the indicator is at (358, 14), the chip at (294, 12, 56,
  20) with its text at (302, 14), and the title ends at x 286 (26 glyphs) with the chip, at
  x 350 (34 glyphs) without it.
- **Battery placeholder**: until ask 4 lands nothing reports a battery, so 6 and 7 never draw;
  the element is still in the tree, named `Battery` with the value `not reported`. The
  percentage is always text, so the charge is never told by the fill's length alone, and a
  low battery adds no colour: at 15 % and under the shell shows the notice `Battery at 15%.`
  once.
- **The whole bar is one button**, `Status bar`: a tap opens the shade (2.7). Pressed: the
  bar is `bar.pressed`. The items are not targets of their own (too small); each has a 44 px
  row in the shade.

### 2.3 Navigation bar

Three zones of 130x48, each a button. Pressed: the zone is `bar.pressed`. With a pointer
(1.3), hover is `bar.hover`.

| # | Button | Zone | Glyph | Does |
|---|--------|------|-------|------|
| 10 | `Back` | (0, 796, 130, 48) | `←` (U+2190) at 2x, cell (57, 804, 16, 32), `text`; `text.dim` when there is nothing to go back to | section 2.6 |
| 11 | `Home` | (130, 796, 130, 48) | the egg mark, H 20, W 14, `accent`, at (188, 810) | shows the home screen (2.4) |
| 12 | `Apps` | (260, 796, 130, 48) | a 14x14 square outline, 1 px, `text`, at (318, 813) | opens the app switcher (2.5) |

The egg in the middle is the desktop's Start button and system item: the same mark means
"the launcher" in every form.

### 2.4 Home screen and the launcher search

The home screen is the `Background` window's phone layout, on the wallpaper (the gradient and
the egg, scaled as on the desktop). It is the launcher too: there is no separate popup.

| # | Element | Place | Draws |
|---|---------|-------|-------|
| 13 | search field | (16, 56, 358, 44) | `field.bg`, 1 px `button.border`, radius 4; the placeholder `Search apps` at (28, 70) in `text.dim` |
| 14 | app grid | a scroll view, (0, 116, 390, 680) | 4 columns of 88, the first at x 19; row `r` top at 116 + 104 x `r`, less the scroll |
| 15 | app cell | 88x104, the whole cell the target | the app's 48x48 icon at the cell's (20, 8); under it, for an app with a window, the dock's 4x4 `text` square at (42, 58); the label centred, top 64, at most 10 glyphs (cut with `...`), `text` on the desktop icon's `bar.bg` pill (20 tall) |

- **Order**: the desktop icons' order (Terminal, Files, Settings, `Host share` when mounted,
  then installed apps by name). Six rows show whole (24 apps); the seventh row's top 56 px
  shows when there are more, which is the sign that the grid scrolls. The content's height is
  the rows x 104 + 16.
- **A tap** on a cell launches the app, or shows its most recently shown window when it has
  one (the dock's rule). While pressed the cell is `selection` (radius 6). A launch keeps the
  cell `selection` and shows the notice `Starting <name>...` until the window maps
  (`desktop-polish-v0.3.md` 4.5).
- **A long press** (500 ms) on an installed app's cell opens the desktop's context menu
  (`Open`, `Uninstall...`) as a bottom sheet. The same is in Settings and `app uninstall`.
- **Searching**: a tap on the field focuses it (the caret of `desktop-polish-v0.3.md` 4.4)
  and shows the on-screen keyboard (2.9). From the first character the grid is replaced by
  the launcher's list, filtered by its own rule and synonyms (`desktop-v0.3-gui.md` 6.2):
  rows 48 tall from y 116, the 32x32 icon at the row's (24, 8), the name at x 64 with its cell
  top at +16, a foreign app's source right-aligned ending at x 374 in `text.dim`, the first row
  `selection`. A tap or Enter launches. `Back` empties the field and returns the grid; a
  second `Back` takes the focus out of the field. The `Minimised:` line is not shown (the
  switcher has it).
- **Keys** (a keyboard attached): arrows move a `selection` cell with the ring, Enter
  launches, typing goes to the search field at once, the Menu key or Shift+F10 opens the
  context menu.
- **The notice** (`notice(text)`, 320x40) sits centred with its bottom 16 px above the
  navigation bar: (35, 740, 320, 40).

### 2.5 Opening an app, going home, the switcher

- **Opening**: the window is placed on the work area, (0, 44, 390, 752), in one frame; its
  title goes to the status bar (5). In `wm list` it is `maximised` and focused; the window that
  was shown becomes `minimised`. In this form exactly one `Normal` window, or none, is shown.
- **A window's minimum size** is clamped to the work area (as section 12 of the desktop
  design already requires); a client that cannot draw at 390x752 is still given that size.
- **Dialogs** are bottom sheets over their parent: full width, their bottom on the navigation
  bar's top (y 796), `menu.bg` with a 1 px `menu.border` top edge, buttons 44 tall, the rows
  above the sheet darkened by `half(p)`. A tap on the darkened part is `Back`.
- **Home** (button 11): the shown window becomes `minimised` and the home screen shows, with
  the scroll and the search as they were left. On the home screen it scrolls the grid to the
  top and empties the search. It is the desktop's Show desktop (Super+D).
- **Apps** (button 12): the app switcher, which is the overview's phone layout
  (`desktops-overview-screenshots-v0.3.md` section 8) with touch numbers: one column of cards
  in the work area, most recently shown first; each card is the window scaled to 156x300 (0.4
  of the content), centred, on a 358x348 `menu.bg` panel 16 apart, with the title at the
  panel's (16, 14) and a `Close` target of 44x44 at its top right corner (the 12 px close
  circle centred in it). A tap on a card shows that window; `Close` sends it the close
  request. Under the last card, a button `Close all`, 358x44. With no windows: `No open apps.`
  in `text.dim`, centred. `Back`, `Home` or `Apps` again leave it. The desktops strip shows
  only when there is more than one desktop (a phone starts with one).
- **A window that stops answering**: the status bar's title carries ` (not responding)`, and a
  tap on its content after 5 s opens the End app sheet, as `desktop-polish-v0.3.md` 4.5 says.

### 2.6 Back

`Back` goes to the first of these that applies:

1. The shade, the switcher, a shell sheet (a context menu, the power sheet) or the on-screen
   keyboard is open: it closes.
2. The search field has text or the focus: 2.4's rule.
3. The shown window has an open dialog or menu sheet: that gets the close request.
4. The shown window set the `HANDLES_BACK` flag (ask 6; every toolkit window does): it gets
   the key `Back`. The toolkit treats it as Esc first (closes a panel, a note, a find bar;
   in a one-column pane layout returns from the detail to the list), and when nothing used it,
   tells the server, which goes to 5.
5. The shown window becomes `minimised` and the home screen shows. Back never closes an app.
6. On the home screen with nothing open: nothing, and the glyph is `text.dim`.

Esc on an attached keyboard is `Back` in the phone and tablet forms.

### 2.7 The shade: quick settings and notices

Opened by the status bar. A sheet under the bar, full width, `menu.bg`, a 1 px `menu.border`
bottom edge; everything below it darkened by `half(p)`. Phone forms have no notification
service yet, so the shade lists the shell's **notices** (the last 4 that `notice(text)` showed
since boot): on a phone a 3 s notice is easy to miss.

| # | Element | Rect | Text |
|---|---------|------|------|
| 16 | tile `Network` | (16, 60, 171, 56) | line 1 `Network`, line 2 the state's word from `network-v0.3.md` (`Connected`, `No cable`, or the Wi-Fi name) |
| 17 | tile `Dark theme` | (203, 60, 171, 56) | `Dark theme`, `On` or `Off` |
| 18 | tile `High contrast` | (16, 132, 171, 56) | `High contrast`, `On` or `Off` |
| 19 | tile `Rotation` | (203, 132, 171, 56) | `Rotation`, `Automatic` or `Locked` |
| 20 | tile `Battery` | (16, 204, 171, 56) | `Battery`, `87%, discharging`; hidden with no battery (21 then takes the row's full width, 358) |
| 21 | button `Settings` | (203, 204, 171, 56) | `Settings` |
| 22 | button `Power` | (16, 276, 358, 44) | `Power` |
| 23 | label | (16, 340) | `Notices` in `text.dim` |
| 24 | button `Clear` | (294, 326, 80, 44) | `Clear`; hidden with no notices |
| 25-28 | notice rows | (16, 376 + 48 x `i`, 358, 44) | the notice's text at the row's (12, 14), cut with `...`, newest first; with none, one line `No notices.` in `text.dim` at (16, 390) |

- A **tile** is a `button.bg` panel, radius 4, line 1 at its (16, 10) in `text`, line 2 at
  (16, 30) in `text.dim`. A tile that is **on** is `selection` with a 4x40 `accent` mark at its
  (0, 8) and its line 2 in `text`: the word, the mark and the fill together, never the colour
  alone.
- Each tile is one setting, so nothing here is GUI-only: `Dark theme` sets `appearance.theme`,
  `High contrast` `accessibility.high_contrast`, `Rotation` switches `display.rotation` between
  `auto` and the rotation in force. `Network` and `Battery` change nothing and open their
  Settings panes (`settings open network`); `Settings` opens Settings; `Power` opens the power
  menu's two entries (`Power off`, `Restart`) as a bottom sheet with rows 48 tall.
- The sheet ends 16 px under its last shown element (y 436 with no notices, 580 with four).
- Closes on `Back`, a tap on the status bar, a tap below the sheet, or a choice that opens
  something. Sound and brightness tiles wait for their backends.

### 2.8 Gestures, and the button that does the same

A gesture is never the only way (WCAG 2.5.1; a switch, a pointer in QEMU and an agent have no
swipes). With `shell.navigation` `buttons`, the default, all of these work together:

| Gesture | Recognised when | Same as |
|---------|-----------------|---------|
| swipe down from the status bar | touch down at y < 44, moved down ≥ 48 | a tap on the status bar |
| swipe up on the shade | moved up ≥ 48 inside the sheet | `Back` |
| swipe up from the navigation bar | touch down at y ≥ 796, moved up ≥ 64, released | `Home` |
| the same, held | moved up ≥ 64 and still for 300 ms before release | `Apps` |
| swipe right from the left edge | touch down at x < 16, moved right ≥ 48 | `Back` |
| drag a switcher card sideways | moved ≥ 120 | the card's `Close` |
| pan inside a scroll view | moved ≥ 8 | the view's scroll, also `wm scroll` and the arrow keys |
| long press | still for 500 ms | the Menu key; and the entries are reachable elsewhere |

- Edge swipes belong to the shell and are never sent to the window. A touch that goes down
  inside a window and moves is the window's.
- `shell.navigation` `gestures` (slice 5) shrinks the navigation bar to 20 px with a 120x4
  `text.dim` pill and gives the 28 px to the work area. The three swipes stay; `Back`, `Home`
  and `Apps` stay in the shade as a row of three 44 px buttons, so the rule above still holds.
- No pinch, no two-finger gesture, no shake anywhere in the shell.

### 2.9 The on-screen keyboard's area

The keyboard itself (keys, layouts from `keyboard.layouts`, dead keys, the caret handles) is a
later brief, `on-screen-keyboard`. This design fixes only its place, so apps and the shell can
be built for it now:

- It shows when a text field takes the focus by touch and `input.present` has no `keyboard`,
  and hides on `Back`, on a tap outside a field, and when a keyboard is attached.
- It is a shell panel across the bottom of the work area: (0, 536, 390, 260) in portrait,
  (0, 214, 796, 176) in landscape. While it shows, the work area is the part above it
  (390x492 in portrait) and the shown window is resized to it, so a toolkit window scrolls its
  focused field into view the way it does for any resize.
- Until it exists, the phone form needs an attached keyboard or the agent (`wm type`) to enter
  text. The grid, the switcher, the shade and every button work without it.

### 2.10 Orientation

- The screen turns when `display.rotation` is `auto` and the device reports a new orientation
  (ask 3), or when the setting is set. The layout function runs for the new `W` and `H`, the
  shown window gets its new size, and the whole screen is damaged once.
- **Landscape (844x390)**: the status bar stays on top, (0, 0, 844, 44). The navigation bar
  moves to the right edge, (796, 44, 48, 346), column 796 `bar.edge`, its three zones 48 wide
  and 115, 116 and 115 tall, top to bottom `Apps`, `Home`, `Back` (the portrait bar turned with
  the device). The work area is (0, 44, 796, 346). The home grid has 8 columns of 88 from x 46
  and shows 2 rows whole; the search field is (16, 56, 764, 44). Sheets keep a width of at
  most 480, centred on the work area.
- An app cannot hold the screen in one orientation yet (a window flag, later).

## 3. Tablet form (1024x768 and 1366x1024)

The phone's parts, rearranged for room: the same status bar and shade, the navigation buttons
inside a dock, and up to two apps side by side.

### 3.1 The frame

| | 1024x768 | 1366x1024 |
|---|---|---|
| status bar | (0, 0, 1024, 44) | (0, 0, 1366, 44) |
| work area | (0, 44, 1024, 660) | (0, 44, 1366, 916) |
| dock | (0, 704, 1024, 64) | (0, 960, 1366, 64) |
| split in halves | 508, the 8 px divider, 508 | 679, 8, 679 |
| split one third and two thirds | 336, 8, 680 | 452, 8, 906 |
| home grid | 8 columns of 112 from x 64 | 11 columns of 112 from x 67 |
| search field | (272, 60, 480, 44) | (443, 60, 480, 44) |
| dock's app icons | at most 9 | at most 14 |

The narrow side of a split is `(W - 8) / 3` rounded down to a multiple of 4; a half is
`(W - 8) / 2`. A side is never under 320: at 768 wide (upright) only halves (380) are offered.
Upright, the bars stay top and bottom and the same rules give the numbers.

### 3.2 The dock

A 64 px bar across the bottom, `bar.bg`, its top row `bar.edge`: the phone's navigation bar
with the apps added.

- **Left**: `Back`, `Home`, `Apps` in three 64x64 zones from x 0, with the phone's glyphs
  centred in them.
- **Centre**: one 64x64 target per app, the group centred on the bar's midpoint: the pinned
  apps (Terminal, Files, Settings), a 1 px `menu.border` separator with 8 px either side, then
  each other app that has a window. The 48x48 icon is at the target's (8, 6); an app with a
  window has the dock's 4x4 `text` square at (30, 56); the focused app also has a 16x2
  `accent` line at (24, 60). A tap launches, or shows the app's most recently shown window.
  When the apps do not fit, the least recently shown unpinned ones are left to the switcher.
- A long press on an icon opens `Open`, `Split left`, `Split right` and `Close` as a popup
  above it (rows 44 tall).

### 3.3 The status bar

The phone's (2.2), with one change: the title (5) becomes a **title button** per shown window,
44 tall, as wide as its text + 32 (at least 96), centred over its window: at the screen's
middle when one window is full, over each side when two are split. The focused side's title is
`text` with a 2 px `accent` line under it; the other is `text.dim`. A tap on a title button
focuses that side and opens the **window menu** under it (the desktop's menu of
`desktop-v0.3-gui.md` 5.2, rows 44 tall): `Full screen`, `Split left`, `Split right`, and
while split `Make wider`, `Make narrower`, `Swap sides`; then `Close`. A tap anywhere else on
the bar opens the shade, which is the phone's, 390 wide, under the bar's right end.

### 3.4 Windows: full or split

- A `Normal` window is **full** (the work area, state `maximised`) or one side of a **split**
  (state shown, with its side's rectangle). No window floats in this form, and none has chrome.
- **Making a split**: `Split left` (or `Split right`) puts the window on that half; the other
  half shows the **chooser**: the switcher's cards and, under them, the app grid, in that half.
  A tap fills the half with that window or app. `Back` cancels and the first window is full
  again.
- **The divider**: 8 px of `bar.bg` with a 4x48 `text.dim` grip at its middle; its target is
  44 wide. A drag follows the finger and, on release, snaps to the nearest of one third, a
  half and two thirds; released within 160 px of a screen edge, the smaller side goes to the
  switcher and the other is full. `Make wider` and `Make narrower` step through the same three
  places without a drag.
- **A side under 600 wide** lays its app out as a phone does: the toolkit's width class comes
  from the window's own width (ask 9), so Settings in a 336 px side is one column. The input
  class stays touch.
- **Opening an app while split**: it takes the focused side; the window that was there goes to
  the switcher. **Closing or sending one side home** leaves the other full. **Home** shows the
  home screen and keeps the pair: the switcher shows it as one card, and a tap brings both back.
- **Dialogs** are centred on their parent's side as on the desktop when the side is at least
  600 wide, and bottom sheets of that side otherwise. Menus drop down as on the desktop with
  rows 44 tall.
- The home screen, the search, `Back`, the switcher (two columns of cards at 1024, three at
  1366) and the on-screen keyboard (across the bottom of the work area, 300 tall) follow the
  phone's rules.

### 3.5 With a keyboard and no pointer

Still the tablet form (1.2). Super alone shows the home screen with the focus in the search
field; Alt+Tab (and Cmd+Tab) is the desktop's switcher list; Super+Left and Super+Right are
`Split left` and `Split right`; Super+Up is `Full screen`; Super+Down sends the window home;
Esc is `Back`; the close keys close. Tab and the arrows move the ring through the dock and
the title buttons when the shell has the focus (F6 moves between the window and the shell, as
the panel keys do on the desktop).

### 3.6 The desktop form on a touch screen

When rule 2 of 1.2 chose `desktop` on a touch screen, the profile's panels and floating windows
are used with touch sizes, so a finger still works beside the touchpad: the bars and title
bars are 44, each window control keeps its 12 px circle inside a 44x44 target, taskbar and dock
items are at least 48, resize zones are 16 wide, and the toolkit's rows are 44. A screen with
no touch keeps today's desktop numbers exactly.

## 4. TV form (1920x1080 at scale 2: 960x540 logical)

A screen read from three metres with a remote in one hand: big, few, and always one thing in
focus. All numbers are logical; on the panel they double (32 px glyphs).

### 4.1 Input: the remote

| Remote | Is | In the shell | In an app |
|--------|----|--------------|-----------|
| arrows | the arrow keys | move the focus | the toolkit moves the focus to the nearest widget in that direction (`toolkit-v0.5.md` section 11) |
| OK | Enter | opens the focused tile | activates |
| Back | the key `Back` | closes a menu or dialog; on the home screen, nothing | 2.6's rules 3 to 5 |
| Home | the key `Home screen` | the focus goes to the first tile | shows the home screen, the focus on that app's tile |
| Menu | the Menu key | the focused tile's menu | arms the menu bar, or the context menu |

An attached keyboard does the same with its arrows, Enter, Esc, Super and Menu keys. Held
arrows repeat with the keyboard's delay and rate. Play, pause and the volume keys go to the
focused window untouched. There is no pointer: no cursor is uploaded until a pointer device
moves, and it is hidden again 3 s after it stops.

### 4.2 The home screen

Nothing is drawn in the outer 48 px left and right and 28 px top and bottom except the
wallpaper (a TV may cut its edges off).

- **Top strip**, y 28 to 71: the egg mark (`accent`, H 20) at (48, 40); the buttons `Settings`
  at (80, 36, 96, 28) and `Power` at (184, 36, 96, 28); at the right, packed from x 912
  leftwards with 16 px gaps, the clock (cell top 42), the network indicator and the Agent
  chip. The buttons are the strip's only focus stops.
- **Rows**, each a title and a line of tiles. A row's title is at x 48 in `text.dim`; its
  tiles start 24 px under the title's top, 144x108 each, 16 apart, the first at x 48. Rows are
  156 apart; the first title's top is y 96.
  1. `Open`: one tile per window, most recently shown first: its thumbnail, 128x72, at the
     tile's (8, 8) and its title centred with its top at 84. Hidden when nothing is open.
  2. `Apps`: every app in the home screen's order: the 48x48 icon at (48, 16) and the name
     centred with its top at 76, at most 16 glyphs.
  3. `Settings`: one tile per Settings pane (`Network`, `Display`, ...), each `settings open
     <pane>`; the gear icon and the pane's name.
- A tile is `dock.bg` with a 1 px `menu.border`, radius 6. Five tiles show whole and the
  sixth shows its first 64 px, which is the sign that the row goes on.
- Later rows (an app's own row from its `app.actions`, recent files) join under these.

### 4.3 Focus

- **Exactly one element has the focus at all times**, and it is drawn with three cues: the
  `selection` fill, the 2 px `focus.ring` (4 px on the panel) just inside its edge, and its
  label in `text` while the other tiles' labels are `text.dim`.
- **Left and Right** move within the row and stop at its ends (no wrap: a held key must end
  somewhere). The row scrolls by whole tiles so the focused one lies inside x 48 to 912.
- **Up and Down** move to the next row, to the tile that last had the focus there (the first,
  when none had); the screen scrolls by whole rows so the focused row and its title are inside
  y 28 to 512. Up from the first row goes to the top strip; Down from the strip returns.
- **OK** opens: a window from `Open`, an app (or its window), a pane. **Menu** on a tile opens
  a centred menu, 320 wide with rows 32 tall: `Open`, then `Close` for an `Open` tile or
  `Uninstall...` for an installed app. **Back** closes it.
- A new window, a dialog or a menu takes the focus when it opens and gives it back to the
  element that had it when it closes.

### 4.4 Apps on a TV

- A `Normal` window fills the screen, (0, 0, 960, 540), with no chrome and no bar; the app
  keeps its own content out of the outer margins (the toolkit pads its root by 48 and 28 in
  this form). Dialogs are centred, as on the desktop, never sheets. The notice sits centred
  with its bottom at y 496.
- **Toolkit apps** are driven by the focus (`toolkit-v0.5.md` section 11): the ring is always
  drawn, arrows move it, Menu arms the menu bar. They set the window flag `FOCUS_NAVIGABLE`
  (ask 6).
- **Pointer-driven apps** (a window without that flag: a foreign GUI program, a canvas, a game
  made for a mouse) are shown in **pointer mode**, and said to be:
  - the arrow pointer is drawn at the window's centre, and a notice reads `Arrows move the
    pointer. OK clicks. Menu for more.` for 3 s each time such a window is shown;
  - an arrow press moves the pointer 8 px; held, 16 px a repeat after 400 ms and 32 px after
    1 s; it stops at the screen's edges;
  - OK is the primary button (down and up with the key); Back sends Esc; Home shows the home
    screen;
  - Menu opens the shell's **pointer menu** (centred, 320 wide): `Right click`, `Scroll up`,
    `Scroll down`, `Send arrows to the app`, `Close app`. `Send arrows to the app` gives the
    arrows and OK to the window as keys (a terminal program, a game played with keys) and
    reads `Move the pointer with arrows` while on; the pointer is hidden meanwhile.
  - On the home screen such an app's tile carries no mark: it opens like any other.
- The on-screen keyboard for a remote (a grid of letters moved through by arrows) belongs to
  the on-screen keyboard's brief; until then text on a TV needs a keyboard or the agent.

## 5. What stays the same in every form

- **The agent channel and the CLI drive every form** (D21, D22). No form adds a capability
  that only fingers or a remote have: each shell button has a command (below), each quick
  setting is a `settings` key, and each gesture has a button.
- **One window list.** `wm list` prints the same columns; the geometry is what the form gave.
- **Every shell element is in an element tree with a name.** `wm show shell` (MCP `wm_show`
  with `window` `shell`) lists the shell's own elements in the form in use, and `wm act shell
  <element> invoke` presses one. This is new for all four forms: the desktop's panels are
  listed the same way. Windows' own trees are unchanged. The phone's tree:

  | Role | Name | Value | Notes |
  |------|------|-------|-------|
  | `button` | `Status bar` | | `expanded` while the shade is open |
  | `text` | `Clock` | `09:14` | child of the status bar, as the next four |
  | `text` | `Title` | the shown window's title | |
  | `text` | `Network` | the state's word | |
  | `text` | `Battery` | `87% discharging`, or `not reported` | |
  | `text` | `Agent` | `idle` or `acting` | listed while connected |
  | `button` | `Back` | | `disabled` when 2.6's rule 6 applies |
  | `button` | `Home` | | |
  | `button` | `Apps` | | `expanded` while the switcher is open |
  | `field` | `Search apps` | its text | `set-text` filters |
  | `list` | `Apps` | `24 apps` | the grid, or the search's rows |
  | `button` | the app's name | `running`, or empty | one per app; `invoke` is the tap |

  The shade's tiles are `button`s named as in 2.7 with their line 2 as the value; switcher
  cards are `button`s named by the window's title, each with a child `button` `Close <title>`;
  a TV row is a `list` named by its title with its tiles as `button`s, the focused one flagged
  `focused`; the tablet's title buttons are `button`s named `Window menu of <title>`.
- **New commands for what the buttons do**: `wm home` (MCP `wm_home`) and `wm back`
  (`wm_back`), in every form: on the desktop `wm home` is Show desktop and `wm back` is Esc to
  the focused window. The switcher needs none: it is `wm list`, `wm focus` and `wm close`.
- **The `wm` verbs in each form**:

  | Verb | Desktop | Tablet | Phone and TV |
  |------|---------|--------|--------------|
  | `wm focus` | raises and focuses | shows it full, or focuses its side when it is in the shown split | shows it; the one that was shown becomes minimised |
  | `wm minimise` | minimises | sends it to the switcher; the other side becomes full | the same; the home screen shows |
  | `wm maximise` | maximises | `Full screen` | as `wm focus` |
  | `wm restore` | restores | shows it as it last was (full or its side) | as `wm focus` |
  | `wm close` | closes | closes | closes |
  | `wm move <id> <x> <y>` | moves | chooses the side: `x` left of the work area's middle is `Split left`, else `Split right`; `y` is ignored | refused: `fixed_by_form` |
  | `wm size <id> <w> <h>` | sizes | snaps `w` to the nearest of one third, a half, two thirds and full; `h` is ignored | refused: `fixed_by_form` |
  | `wm click`, `wm drag`, `wm scroll`, `wm type`, `wm press` | to the window | the same: a click is a tap, a drag is a finger's drag, a scroll is a pan | the same; on a TV a click reaches any window, in pointer mode or not |
  | `wm capture`, `screenshot` | the window, the screen | the same | the same |
  | `wm desktop ...` | virtual desktops | the same model; the switcher shows the strip from two desktops on | the same |

  `wm move` and `wm size` on a tablet answer with the rectangle the window got, so a script
  reads what happened. `fixed_by_form` is a new code in the category `refused`, with the
  message `wm size: the phone form shows every window at 390x752. Use wm focus to show it.`
  (`fix_command` `wm focus <id>`). `wm press` takes the new key names `Back`, `HomeScreen` and
  `Menu`, so an agent can press a remote's keys.
- **The agent is visible**: the Agent chip sits in the status bar (phone, tablet) and the top
  strip (TV), the driven window's title gets ` (agent)` where a title is drawn, and the
  agent's arrow is drawn at an injected point in every form, the only arrow a phone ever
  shows. Injected input never moves the person's focus on a TV: an agent acting on a window
  that is not shown acts on it where it is, as on the desktop.
- **Person-only surfaces** (D193) stay person-only: a masked field's value is not listed in
  any form.
- **Settings**: every key works in every form; a key with no effect in the form in use (the
  mouse's on a phone) is still set and kept.

## 6. Accessibility

- Keyboard-only use works in every form: section 2.4's keys, 3.5 and the TV's whole model.
- Contrast: only existing pairs are used. `text` on `bar.bg` 14.3:1, on `menu.bg` 12.9:1, on
  `button.bg` 10.5:1, on `selection` 7.6:1, on `bar.pressed` 8.7:1; `text.dim` on `bar.bg`
  5.7:1, on `menu.bg` 5.1:1. `text.dim` is never drawn on `selection`, `bar.hover` or
  `bar.pressed`: a pressed zone's glyph is `text`, and an on tile's second line turns `text`.
- State is never colour alone: on tiles have a word and a mark; the focused app has a line
  and its square; the battery has its number; the TV's focus has the fill, the ring and the
  label.
- High contrast and the light theme repaint the forms through the same palette as the desktop
  (`toolkit-v0.5.md` section 8); nothing here names a literal colour.
- Reduced motion has nothing to reduce: no surface slides. If transitions are added later
  they are off under `accessibility.reduce_motion`.
- Timing: nothing in the shell needs a timed gesture; long press and the held swipe have
  untimed equivalents (2.8).

## 7. Rendering and lightness (D24)

- The layout function gains the form: `layout(screen_w, screen_h, scale, profile, form)`
  returns the panels, the work area, the launcher's anchor and the window rule (`Floating`,
  `FullOrSplit`, `Full`). Nothing else in the shell reads the screen's size or the form
  (`desktop-v0.3-gui.md` section 12's rule, kept).
- No new buffers: the status bar, the navigation bar and the dock are panels like the top bar
  and the taskbar; the home screen is the `Background` window; the shade and the sheets are
  popups. Switcher cards and the TV's thumbnails reuse the overview's scaling.
- Damage per action is one panel or one sheet, except a change of form or rotation (the whole
  screen, once). Idle still costs nothing: the clock damages its own 40x16 once a minute.
- A machine in one form carries the other forms' layout code (a few kilobytes) and none of
  their state; a headless machine carries none of it (D18).

## 8. Left out, on purpose

Notifications from apps (a service, its permission and its list; the shade's notices are the
shell's own), a lock screen, widgets on the home screen, arranging or foldering the grid,
more than one split pair, floating or picture-in-picture windows on a tablet, sound and
brightness tiles, haptics, per-app orientation, two displays in different forms, the wearable
and car forms (their own briefs: the pane designs already say what a wearable drops), voice
input, and scale 3 and 4.

## 9. The order to build it in

1. **Form selection and the phone's frame** (slice 1): `shell.form`, `shell.form_in_use`,
   `input.present`; `layout` with the form; the phone's status bar (clock, title, network
   indicator, Agent chip), the navigation bar with `Back` and `Home` (the `Apps` zone is drawn
   empty and does nothing yet), the home grid and its search with an attached keyboard, full
   windows without chrome, dialogs as bottom sheets, `Back`'s rules 2, 3, 5 and 6, the
   pointer as the finger (1.3), `wm show shell`, `wm home`, `wm back`, `fixed_by_form`, and
   the Display pane's `Layout` row. Needs no backend ask: it runs in QEMU with `settings set
   shell.form phone` on a 390x844 display. Acceptance: section 10.
2. **Touch and the rest of the phone**: the touch path (asks 1 and 2), the gestures of 2.8,
   the switcher and the `Apps` button, the shade, `HANDLES_BACK`, landscape and
   `display.rotation`, the 44 px toolkit sizes.
3. **Tablet**: the dock, title buttons and the window menu, full and split, the chooser, the
   divider, the keys of 3.5, the automatic change to `desktop` and back, 3.6's touch sizes.
4. **TV**: the remote's keys, the home rows, the focus model, pointer mode, the toolkit's
   margins.
5. **After their own briefs or backends**: the on-screen keyboard (touch, then remote), the
   battery, `shell.navigation` `gestures`, the camera cut-out, scale 3 and 4.

Each slice ends with the earlier forms' tests still passing: the desktop's numbers do not
change in any slice.

## 10. Acceptance for slice 1 (host tests, 390x844, scale 1x, dark palette, `shell.form` `phone`,
an adapter, no battery, a pointer and a keyboard present, no touch screen)

1. **Choosing the form**: `form(390, 844, touch)` is `phone`; `form(844, 390, touch)` is
   `phone`; `form(1024, 768, touch)` and `form(768, 1024, touch)` are `tablet`; `form(1024,
   768, touch + keyboard)` is `tablet`; `form(1024, 768, touch + keyboard + pointer)` is
   `desktop`; `form(1920, 1080 at scale 2, remote)` is `tv`; `form(1920, 1080 at scale 2,
   remote + pointer)` and `form(1280, 800, keyboard + pointer)` are `desktop`; `form(599, 900,
   touch)` is `phone` and `form(600, 900, touch)` is `tablet`; a touch screen of 300x400 gets
   the console.
2. **The setting**: `settings get shell.form` prints `auto` on a fresh machine and
   `shell.form_in_use` prints `desktop` at 1280x800. `settings set shell.form phone` at
   390x844 makes `shell.form_in_use` `phone` with no restart, and the settings file holds
   `shell.form=phone`. `settings set shell.form tablet` there fails with `not_applied`, keeps
   `phone` on screen and saves `tablet`. `settings set shell.form watch` fails with
   `invalid_value` naming the five values. `settings set shell.form_in_use phone` fails with
   `setting_read_only`. `input.present` prints `keyboard,pointer`.
3. **The frame**: `layout(390, 844, 1, profile, Phone)` gives the status bar (0, 0, 390, 44),
   the work area (0, 44, 390, 752) and the navigation bar (0, 796, 390, 48), the same for all
   three profiles. Pixel (0, 0) is `bar.bg`, (0, 43) `bar.edge`, (0, 796) `bar.edge`, (0, 797)
   `bar.bg`, (0, 843) `bar.bg`.
4. **The status bar on the home screen**: the clock's glyphs start at (16, 14) in `text`; the
   network glyph is at (358, 14); no pixel between x 72 and 349 in rows 14 to 29 is `text`
   (no title, no chip). With an agent connected the chip is (294, 12, 56, 20) and its text
   starts at (302, 14).
5. **The navigation bar**: the `←` cell is (57, 804, 16, 32) in `text.dim` with nothing open;
   the egg's top row is at y 810 centred on x 195 in `accent`; no pixel in (260, 797, 130, 47)
   differs from `bar.bg`. Button down at (60, 820) with a window shown turns (0, 797, 130, 47)
   `bar.pressed` outside the glyph; button up restores it.
6. **The home grid** with Terminal, Files, Settings and four installed apps: the search field
   is (16, 56, 358, 44) with `Search apps` from (28, 70) in `text.dim`; Terminal's icon is at
   (39, 124), Files' at (127, 124), Settings' at (215, 124), the first installed app's at
   (303, 124) and the second's at (39, 228); each label's pill top is its cell's top + 64. With
   30 apps the content is 848 tall, the seventh row's icons start at y 748, and a wheel notch
   scrolls the grid, not the bars.
7. **Opening**: a press and release at (63, 150) starts Terminal; the cell is `selection`
   meanwhile; when its window maps, `wm list` shows it `maximised` at (0, 44, 390, 752), the
   status bar's title `Terminal` starts at (72, 14) in `text`, the `←` glyph is `text`, and no
   pixel of the work area is title bar, border or shadow. A title of 40 glyphs is cut to 31
   and `...` (34 glyphs, ending at x 344).
8. **Home**: a click at (195, 820) shows the home screen; `wm list` shows Terminal
   `minimised`; Terminal's cell has the 4x4 `text` square at (61, 174). A click on its cell
   shows the same window again (no second one). `wm home` does the same as the click, and on
   the home screen scrolls the grid to the top.
9. **Back**: with Terminal shown, a click at (65, 820) minimises it and shows the home
   screen; `wm back` does the same. With a dialog open over a window, `Back` sends the close
   request to the dialog and the window stays shown. With text in the search field, `Back`
   empties it and the grid returns; the next `Back` takes the focus from the field; the next
   does nothing. Esc does each of these.
10. **Search**: typing `fin` on the home screen fills the field, shows one row at (0, 116,
    390, 48) for Files (`Finder` is one of its synonyms) in `selection`, its icon
    at (24, 124) and its name from (64, 132); Enter shows Files and empties the field.
11. **A dialog** of 320x160 opened by the shown window is drawn at (0, 636, 390, 160) with a
    `menu.border` row at y 636, and pixel (10, 100) is `half` of what it was.
12. **Two windows**: with Terminal shown, `wm focus <Files' id>` shows Files and `wm list`
    has Terminal `minimised`; `wm size <id> 300 300` and `wm move <id> 10 10` fail with
    `fixed_by_form` (category `refused`, `fix_command` `wm focus <id>`) and change nothing;
    `wm maximise` and `wm restore` on Terminal show it; `wm close` on the shown window shows
    the home screen.
13. **The tree**: `wm show shell` lists `Status bar`, `Clock` valued with the time, `Title`
    valued `Terminal`, `Network`, `Battery` valued `not reported`, `Back`, `Home`, `Search
    apps`, the `Apps` list valued `7 apps` and one `button` per app, Terminal's valued
    `running`; no `Apps` button is listed in slice 1. `wm act shell <Home's id> invoke` shows
    the home screen; `invoke` on Files' button shows Files; `set-text` `control panel` on
    `Search apps` leaves one row, Settings. With nothing open `Back` is flagged `disabled` and `invoke` on it
    answers `element_disabled`.
14. **The desktop is untouched**: `settings set shell.form auto` at 1280x800 gives
    `layout`'s existing three results for the three profiles, value for value, and every
    existing desktop test passes with the `form` argument `Desktop`. Changing from `phone` to
    `desktop` on a 1280x800 display with two windows keeps both, the shown one maximised.
15. **2x**: at 780x1688 device px and scale 2 every rect above doubles (the status bar is 88
    device px, the clock's glyphs are 16x32 from (32, 28)).

## 11. Backend asks

For the kernel and the drivers:

1. **Input device kinds.** The input layer names each device's kind (`touch`, `keyboard`,
   `pointer`, `remote`) from its descriptor, tells the desktop when one is added or removed
   (`Desktop::set_input_present(kinds)`), and backs `input.present`. A remote is a HID
   consumer-control or CEC source with a D-pad and no letter keys. In QEMU: a
   `virtio-multitouch` device is `touch`; a boot option adds a pretend `remote` for tests.
2. **A touch path.** Multi-touch events from the driver (virtio first, then I2C HID and USB
   HID touch screens): per contact an id, down, move and up in display px, and cancel. The
   server turns one contact into the events clients already get (a tap is `PointerMove` and
   `Button` down and up at the point, then `PointerLeave`; a pan past 8 px is `Wheel` and
   `SidewaysWheel` in px, not notches; a still touch of 500 ms is a new `LongPress(x, y)`, after
   which the contact's moves are a drag), keeps the edge swipes for itself, and ignores every
   contact after the first until the multi-touch brief.
3. **Orientation.** An accelerometer's orientation as one of four values with debouncing,
   `Desktop::set_orientation`, a display that can be drawn rotated (the framebuffer written
   through a rotated blit, or the display engine's own rotation), and `display.rotation`.
4. **Battery.** Charge in percent, the state (`charging`, `discharging`, `full`), whether a
   battery exists, and a change event at each percent and state change, from ACPI or the
   device tree's fuel gauge; `power.battery`; `/proc/power/battery` for the CLI.
5. **The panel's shape.** The rect of a camera cut-out and the corner radius, so the status
   bar keeps its items clear of them; none is assumed until it is reported.
6. **Window flags.** `HANDLES_BACK` (the client wants the `Back` key and reports when it did
   not use it, `BACK_UNUSED`) and `FOCUS_NAVIGABLE` (the client moves its own focus by
   arrows). `CHANGE_FLAGS` may change both on a live window.
7. **Keys.** Key codes `Back` and `HomeScreen` beside `key::MENU`, from remotes, Android-style
   keyboards and `wm press`.
8. **The form for clients.** `GET_FORM`: the form in use and the input kinds present, with a
   `FormChanged` event, so the toolkit never guesses from the screen's size.

For the toolkit:

9. `widget::form_factor()` splits in two: the **width class** from the window's own content
   width (under 600 is one column) and the **input class** from `GET_FORM`. The touch sizes
   become 44 (rows, bars, buttons, the header) where they were 40.
10. `Back` as Esc, then `BACK_UNUSED` when no widget used it; the TV's root padding (48, 28);
    `FOCUS_NAVIGABLE` set by `application::run`.
11. A sheet's and a menu's rows at 44 and 48 as this brief uses them, from one constant.

## Decided by design (the owner can overrule any of these)

- One breakpoint, 600 on the short side; a keyboard alone never changes a tablet's form, a
  keyboard with a pointer makes it the desktop.
- Three navigation buttons by default on phones and tablets; gestures are extra, and
  gesture-only navigation is an opt-in that still keeps the buttons in the shade.
- The phone's status bar is 44 tall and is the shade's button; the app's title is in it.
- The home screen is the launcher: one scrolling grid and one search, no pages, no dock on a
  phone.
- `Back` never closes an app, and `Home` never does either: closing is the switcher's.
- A tablet shows one or two apps, never floating windows; `wm move` and `wm size` snap there
  and are refused on a phone and a TV rather than ignored.
- The TV runs at scale 2 on 1080p and puts pointer-driven apps in a pointer mode with a
  notice, instead of hiding them from the home screen.
- The shell gets an element tree (`wm show shell`) in every form, the desktop included.
- No motion and no new colour.
