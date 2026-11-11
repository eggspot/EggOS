# EggOS v0.5 adaptive shell: the TV form: design brief

Design by Claude Fable (D16) for slice 4 of the adaptive shell (roadmap M12, D23, D303). The
parent brief, `adaptive-shell-v0.5.md`, says how the shell picks its form (section 1), what a
TV shows in outline (section 4), what stays the same in every form (section 5) and that the
TV form "gets its own numbered brief when its slice is built" (section 9, item 4). This is
that brief: every element numbered, every rect given, every key in every place.

Slice 1 is written (`drivers/src/desktop/form.rs`: `Form::Tv` is chosen with a remote and no
pointer at 960x540 or more; `layout.rs`: `layout_in_form` gives the TV the desktop layout until
this slice; `kernel/src/input.rs`: `input::pretend_remote`). This brief replaces that fallback.

Tokens, the 4 px grid, Egg Mono 8x16 for every label, the corner tables, the shadow and the
voice are the desktop's (`desktop-v0.3.md`, `desktop-v0.3-gui.md`, `desktop-polish-v0.3.md`).
**No colour token is added and nothing moves**: every surface appears in one frame, and a
scroll is a jump of whole tiles or whole rows.

Numbers are logical px at **960x540** with the origin at the screen's top left; on a 1920x1080
panel at scale 2 each doubles (32 px glyphs, a 4 px ring). A rect is (x, y, width, height).

Three sentences of the parent's section 4 are made exact here, on purpose: a row's title sits
on a pill whose left edge is x 48, so its glyphs start at x 52 (1.3); the strip's two buttons
are drawn as small tiles, not push buttons (1.2); and a menu's unfocused rows stay `text`,
because `text.dim` means "not available" in a menu (3.4).

## 0. What a person gets, and how that is checked

- A TV with a remote shows three rows of big tiles: what is open, the apps, the settings.
  One thing is in focus at all times; five keys do everything (arrows, OK, Back, Home, Menu).
- Apps made for a keyboard or the toolkit are driven by the focus. Apps made for a mouse get a
  pointer the arrows move, and the screen says so.
- Everything the remote does, `wm`, `settings` and their MCP twins do (D22, section 7), so an
  agent or a script drives a TV with no remote at all.
- Checked by host tests at 960x540 logical, scale 2 (section 10).

## 1. The home screen

The home screen is the `Background` window's TV layout, drawn on the wallpaper (the gradient
and the egg, scaled as on the desktop). There are no panels: the work area is the screen.

### 1.1 The frame

| # | Element | Rect | Draws |
|---|---------|------|-------|
| 1 | screen | (0, 0, 960, 540) | the wallpaper |
| 2 | safe area | (48, 28, 864, 484) | nothing of its own: every element below lies inside it |
| 3 | top strip | (48, 28, 864, 44), rows 28 to 71 | no fill: its items sit on the wallpaper |
| 4 | rows view | (48, 84, 864, 428), rows 84 to 511 | the rows, clipped to this rect |

- **Overscan margins**: 48 px left and right, 28 px top and bottom (5 % of each side). The
  shell draws only wallpaper there, because a TV may cut its edges off. The margins are the
  constants `TV_MARGIN_X` and `TV_MARGIN_Y`, and the toolkit's root padding is the same pair
  (section 5.2).
- **On a larger screen** (`shell.form` `tv` forced at 1280x800, say) the safe area is the
  screen less the same margins, the strip's right run packs from `W - 48`, and the rows view
  is (48, 84, `W` - 96, `H` - 112). Whole tiles in a row: `(W - 80) / 160`, rounded down (5 at
  960, 7 at 1280). Whole rows: `(H - 256) / 156 + 1`, rounded down (2 at 540, 4 at 800).

### 1.2 The top strip

The strip never scrolls. Items on the right are a run packed from x 912 leftwards, 16 px
apart; an item that is absent takes no room.

| # | Element | Rect | Draws | Focus stop |
|---|---------|------|-------|------------|
| 5 | egg mark | (48, 40, 14, 20) | the egg, H 20, W 14, `accent` | no |
| 6 | button `Settings` | (80, 36, 96, 28) | `dock.bg`, 1 px `menu.border`, radius 4; `Settings` from (96, 42) in `text.dim` | yes |
| 7 | button `Power` | (184, 36, 96, 28) | the same; `Power` from (212, 42) in `text.dim` | yes |
| 8 | clock | glyphs end at x 912, top 42 | `09:14` in `text` from (872, 42); `up 3m` until the clock is set | no |
| 9 | network indicator | (840, 42, 16, 16) | the bar's glyph of `network-v0.3.md` section 4; shown when an adapter exists | no |
| 10 | Agent chip | (768, 40, 56, 20), text from (776, 42) | idle and acting as `desktop-v0.3-gui.md` 14.1; shown while an agent is connected | no |

- With no adapter the chip is (800, 40, 56, 20). With `up 3m` (5 glyphs) the clock starts at
  x 872 as well; a longer uptime starts further left and the run moves with it.
- **The egg is a mark here, not a button**: the home screen is already the launcher, and the
  remote's Home key is the egg's job. It still means "EggOS" in every form.
- **6 and 7 are drawn as small tiles**, not as the desktop's push button, so their resting
  label can be `text.dim` on `dock.bg` (5.1:1) like every other unfocused label on this
  screen; `text.dim` on `button.bg` would be 4.1:1.
- OK on `Settings` opens the Settings app. OK on `Power` opens the power menu (3.4).
- The clock, the indicator and the chip are read-only on a TV: `Network` has its own tile in
  the `Settings` row.

### 1.3 Rows

A row is a title and a line of tiles. Row `r` (0 for the first shown row) with the view
scrolled by `s` rows has its origin at `y0 = 96 + 156 x (r - s)`.

| # | Element | Rect | Draws |
|---|---------|------|-------|
| 11 | row title | pill (48, `y0` - 2, 8 x glyphs + 8, 20) | `bar.bg`, radius 4; the title from (52, `y0`) in `text.dim` |
| 12 | row position | pill ending at x 912, top `y0` - 2, 20 tall | `bar.bg`, radius 4; `3 of 24` in `text.dim`, its glyphs ending at x 908 |
| 13 | tile `i`, row scrolled by `o` tiles | (48 + 160 x (`i` - `o`), `y0` + 24, 144, 108) | `dock.bg`, 1 px `menu.border`, radius 6 |

- **The pill** is the desktop icon label's (`desktop-v0.3-gui.md` section 8): `text.dim` on
  the lower wallpaper would be 3.8:1; on `bar.bg` it is 5.7:1.
- **12 shows only on the focused row, and only when the row has more tiles than fit whole**
  (more than 5 at 960). It says where the focus is in the row, in words, so a person need not
  count tiles from three metres and position is never told by the scroll alone.
- **The rows**, in this order:
  1. `Open`: one tile per `Normal` window on the current desktop, most recently shown first.
     Hidden, taking no room, when nothing is open. A dialog has no tile (its parent's does).
  2. `Apps`: every app in the home screen's order (Terminal, Files, Settings, `Host share`
     when mounted, then installed apps by name).
  3. `Settings`: one tile per Settings pane, in the app's order: `Appearance`, `Network`,
     `Display`, `Keyboard`, `Mouse`, `Accessibility`, `Storage`, `Date & time`, `Agents`,
     `Windows`, `Components`, `About`.
- With nothing open at 960x540: `Apps` has its title at y 96 and tiles at y 120 to 227;
  `Settings` has its title at y 252 and tiles at y 276 to 383. With a window open the three
  rows' tiles are at y 120, 276 and 432.
- **Clipping tells there is more.** A sixth tile shows its first 64 px (x 848 to 911). A
  third row shows its tiles' first 80 px (y 432 to 511). A tile that is cut draws its panel
  and its icon or thumbnail clipped, and **no label**: a label is drawn whole or not at all.
- Later rows (an app's own row from its `app.actions`, recent files) join under these.

### 1.4 Tiles

| # | Tile | Inside the 144x108 tile | Label |
|---|------|-------------------------|-------|
| 14 | `Open` tile | the window's thumbnail, 128x72, at (8, 8) | the window's title, centred, top 84, at most 16 glyphs (13 and `...`) |
| 15 | `Apps` tile | the app's 48x48 icon at (48, 16); for an app with a window, the dock's 4x4 `text` square at (70, 68) | the app's name, centred, top 76, at most 16 glyphs |
| 16 | `Settings` tile | the Settings gear icon, 48x48, at (48, 16) | the pane's name, centred, top 76 |

- **The thumbnail** is the window's last committed content scaled by 2/15 (960x540 to
  128x72): thumbnail pixel (`x`, `y`) is the content's pixel (`x` x 15 / 2, `y` x 15 / 2),
  rounded down, in device px at any scale. It reuses the overview's scaling
  (`desktops-overview-screenshots-v0.3.md` section 4.3) and repaints at most once a second
  while the home screen shows.
- **Labels at rest are `text.dim`** on the tile's `dock.bg` (5.1:1). A window driven by the
  agent carries ` (agent)` in its tile's title, a window that stopped answering ` (not
  responding)`; both are cut like any title.
- **A pane tile** runs `settings open <pane>` (the catalogue's `settings_pane`); when Settings
  is already open it shows that window on that pane.

## 2. Focus

### 2.1 The three cues, and no scale-up

Exactly one element has the focus at all times. It is drawn with three cues together:

| # | Cue | On a tile or a strip button |
|---|-----|-----------------------------|
| 17 | fill | `selection` in place of `dock.bg` (a thumbnail or an icon stays as it is) |
| 18 | ring | 2 px `focus.ring` on the element's outer 2 px, in place of the 1 px border, with the element's own corner table |
| 19 | label | `text` in place of `text.dim` (7.6:1 on `selection`) |

- **The focused tile does not grow.** A TV's usual "scale-up" needs either motion or scaled
  glyphs and icons; Egg Mono and the icons are pixel art that scales by whole factors only,
  and a tile grown by even 4 px would leave the safe area at x 48. The ring at 4 device px,
  the fill and the label read from three metres without it.
- A row keeps no cue for the tile it remembers (2.2): at rest every tile but the focused one
  looks the same.
- When a menu or a dialog takes the focus, the element under it keeps its fill and its label
  and loses its ring: the desktop's rule ("the `selection` fill stays when focus leaves; the
  ring is drawn only with focus"), so there is one ring on the screen at any time.

### 2.2 Moving

- **At boot, and when the form becomes `tv`**: the first tile of the first row.
- **Left and Right** move one tile within the row and stop at its ends. The row's scroll `o`
  changes only as far as needed to keep the focused tile whole: to `i` when `i` < `o`, to
  `i` - 4 when `i` > `o` + 4. Nothing shows left of x 48 after a scroll; the row position (12)
  says how far along the focus is.
- **Up and Down** move one row and stop at the last row. The focus lands on the tile that
  last had the focus in that row (its first tile when none had), and that row keeps its own
  scroll. The view's scroll `s` changes only as far as needed: to `r` when `r` < `s`, to
  `r` - 1 when `r` > `s` + 1, so the focused row and its title are whole.
- **Up from the first row** goes to the strip, to the button that last had the focus there
  (`Settings` the first time). **Down from the strip** returns to the first row. Left and
  Right move between 6 and 7 and stop. Up in the strip does nothing.
- **No wrap anywhere**: a held key must end somewhere. A key that cannot move changes nothing
  and makes no sound.
- **A held arrow** repeats with the keyboard's delay and rate (`keyboard-v0.3.md`).
- **A tile that goes away while focused** (its window closed, its app uninstalled): the focus
  goes to the next tile in that row, else the one before, else the first tile of `Apps`.
- **A row that appears** (`Open`, when the first window opens) joins above without moving the
  focus off its tile; `s` is recomputed by the rule above.

### 2.3 Taking and giving back

A new window, a dialog or a menu takes the focus when it opens. When it closes, the focus
returns to the element that had it. Leaving an app by Home or Back (4.1) puts the focus on
that window's `Open` tile, which is the first of the first row, with `s` and that row's `o` 0.

## 3. The remote

### 3.1 The keys, in each place

"Focus app" is a window with `FOCUS_NAVIGABLE` (every toolkit window, the terminal); "pointer
app" is one without it (section 5.3).

| Key | Home screen | Shell menu or dialog | Focus app | Pointer app, pointer mode | Pointer app, keys mode |
|-----|-------------|----------------------|-----------|---------------------------|------------------------|
| arrows | move the focus (2.2) | Up and Down move the row; Left and Right move between a dialog's buttons | to the app: the toolkit moves the ring to the nearest widget that way | move the pointer (5.3) | to the app as arrow keys |
| OK | opens the focused element | chooses | to the app as Enter | primary button, down and up with the key | to the app as Enter |
| Back | nothing | closes it | 4.1's rules | sends Esc | sends Esc |
| Home | the focus goes to the first tile of the first row; every scroll returns to 0 | closes it, then as on the home screen | shows the home screen (2.3) | the same | the same |
| Menu | the focused tile's menu (3.4); nothing on 6 and 7 | closes it | to the app as the Menu key: arms the menu bar, or the context menu | the pointer menu (5.4) | the pointer menu |
| Play/pause, Play, Pause, Stop, Rewind, Fast forward, Next, Previous | nothing | nothing | to the app, untouched | to the app | to the app |
| Volume up, Volume down, Mute | nothing until sound exists | nothing | to the app, untouched | to the app | to the app |
| Channel up, Channel down, Info, digits, the four colour keys | nothing | nothing | to the app, untouched | to the app | to the app |
| Power | the power menu (3.4), from any place, over anything | closes it; a second press opens the power menu | the power menu | the power menu | the power menu |

- **Home always works.** No window can take Home, Power or (in a pointer app) Menu, so a
  person is never trapped in an app.
- **Back never closes an app** (the parent's 2.6): it closes a menu, a dialog or a panel, and
  otherwise goes home with the window kept in `Open`. Closing is `Close` in a tile's menu,
  `Close app` in the pointer menu, the app's own command, or `wm close`.
- A held OK is a held button in pointer mode and repeats as Enter elsewhere.

### 3.2 An attached keyboard

The form stays `tv` (the parent's 1.2, "sticky"). The keyboard is a second remote and more:

| Keyboard | Is |
|----------|----|
| arrows, Enter | the arrows, OK |
| Esc | Back |
| Super alone | Home |
| the Menu key, Shift+F10 | Menu |
| Tab, Shift+Tab | on the home screen: the next and the previous focus stop in reading order (strip, then each row from its first tile), stopping at the ends |
| the profile's close keys | close the shown window |
| everything else | to the shown window; nothing on the home screen |

### 3.3 An attached pointer

Plugging in a mouse does not change the form. The arrow is uploaded when the device first
moves and hidden 3 s after it stops. A click on a tile or a strip button moves the focus
there and opens it; a click on a menu row chooses it; a click outside a menu closes it. Hover
changes nothing (focus is the remote's), and the wheel does nothing in the shell. In an app
the pointer works as on the desktop, whatever the window's flag.

### 3.4 Shell menus

One shape for the tile menu, the power menu and the pointer menu: a `Popup` centred on the
screen, 320 wide, `menu.bg`, 1 px `menu.border`, radius 6, the window shadow; rows 32 tall
from 4 px under its top; height 32 x rows + 8. Nothing behind it is darkened.

| # | Element | Rect | Rows |
|---|---------|------|------|
| 20 | tile menu, two rows | (320, 234, 320, 72), rows at y 238 and 270 | `Open`, then `Close` on an `Open` tile or `Uninstall...` on an installed app's tile |
| 21 | tile menu, one row | (320, 250, 320, 40), row at y 254 | `Open` (a built-in app's tile, a pane's tile) |
| 22 | power menu | (320, 234, 320, 72) | `Power off`, `Restart` |
| 23 | menu row | (321, row y, 318, 32) | the label from (344, row y + 8) in `text`; the focused row is `selection` with the 2 px ring inside its edge and the list's `>` mark at (328, row y + 8) |

- The first row has the focus when the menu opens. Up and Down stop at the ends.
- `Close` sends the window the close request; `Uninstall...` opens the desktop's confirmation
  as a dialog (5.1) with the focus on `Cancel`; `Power off` and `Restart` do what the
  desktop's power menu does.
- A menu's three cues are the fill, the ring and the `>` mark: unfocused rows stay `text`.

### 3.5 The notice

`notice(text)` is 320x40 at (320, 456, 320, 40), its bottom at y 496, in every place on a TV.
`Starting <name>...` shows from OK on a tile until the window maps
(`desktop-polish-v0.3.md` 4.5); the tile keeps the focus meanwhile and a second OK does
nothing.

## 4. Apps on a TV

### 4.1 Opening, leaving, Back

- **Opening**: the window is placed at (0, 0, 960, 540) in one frame, with no title bar, no
  border, no shadow and no controls. In `wm list` it is `maximised` and focused; the window
  that was shown becomes `minimised`. Exactly one `Normal` window, or none, is shown. A tile
  of an app that has a window shows that window (the dock's rule); it never starts a second.
- **A window's minimum size** is clamped to 960x540, and a client that cannot draw at that
  size is still given it.
- **Back** in a focus app goes to the first of these that applies: an open dialog or menu of
  the window gets the close request; a window with `HANDLES_BACK` gets the key `Back`, which
  the toolkit treats as Esc and reports `BACK_UNUSED` when nothing used it; then the window
  becomes `minimised` and the home screen shows (2.3).
- **When the shown window closes**, the home screen shows with the focus where 2.2's
  "a tile that goes away" puts it, or on the tile that opened the app when that still exists.
- **A window that stops answering**: OK or an arrow after 5 s opens the End app dialog of
  `desktop-polish-v0.3.md` 4.5, centred; Home still works.

## 5. Windows, the toolkit and pointer mode

### 5.1 Dialogs

| # | Element | Place | Draws |
|---|---------|-------|-------|
| 24 | dialog | its desktop frame centred on the screen, on whole px, never outside the safe area | the desktop's dialog frame (title bar, border, shadow) **without the three control circles**; nothing behind it is darkened |

A dialog is never a sheet on a TV. It takes the focus, its default button has the ring, and
Back sends it the close request. A dialog larger than 864x484 is given that size.

### 5.2 The toolkit (focus apps)

| # | Element | Rect | Notes |
|---|---------|------|-------|
| 25 | window | (0, 0, 960, 540) | the root's background fills it all |
| 26 | root content | (48, 28, 864, 484) | the root's padding is (48, 28); the menu bar is (48, 28, 864, 28); every widget is laid out inside |

- The toolkit learns the form from `GET_FORM`, never from the size, and applies the padding
  only when the form is `tv` and the window is a full `Normal` window (not a dialog).
- The ring is always drawn (the window is always "keyboard focused"); arrows move it to the
  nearest widget in that direction and stop at the edges; Menu arms the menu bar; `Back` is
  Esc (`toolkit-v0.5.md` section 11). Rows, bars and buttons keep the desktop's sizes: scale 2
  is what makes them readable, and no target needs a finger.
- A widget that is safe to cut (a picture, a video, a map) may ask for the whole window with
  one root option, `full_bleed`; text and controls never do.
- The terminal sets `FOCUS_NAVIGABLE` (its program reads the arrows) and draws its grid
  inside the padding: 108 columns by 30 rows from (48, 28).

### 5.3 Pointer mode (pointer apps)

A window without `FOCUS_NAVIGABLE` (a foreign GUI program, a canvas, a game made for a mouse)
is shown at (0, 0, 960, 540) like any other, in **pointer mode**:

| # | Element | Place | Draws |
|---|---------|-------|-------|
| 27 | pointer | hot spot at (480, 270) each time the window is shown | the desktop's arrow shape |
| 28 | notice | 3.5's rect, for 3 s each time such a window is shown | `Arrows move the pointer. OK clicks. Menu for more.` |

- **Moving**: a press moves the pointer 8 px. Held, it repeats every 50 ms from 400 ms on, 16
  px a repeat, and 32 px a repeat from 1 s on. Two arrows held move it on both axes. It stops
  at x 0 and 959 and y 0 and 539. The window gets `PointerMove` for each step.
- **OK** is the primary button: down with the key, up with it. Holding OK while pressing
  arrows drags, on remotes that send two keys at once.
- **Back** sends Esc. **Home** shows the home screen. **Menu** opens the pointer menu.
- The window's own cursor shape is used when it sets one.
- On the home screen a pointer app's tile carries no mark: it opens like any other.

### 5.4 The pointer menu

| # | Element | Rect | Rows, in order |
|---|---------|------|----------------|
| 29 | pointer menu | (320, 186, 320, 168), rows at y 190, 222, 254, 286, 318 | `Right click`, `Scroll up`, `Scroll down`, `Send arrows to the app`, `Close app` |

- `Right click` closes the menu and clicks the secondary button at the pointer.
- `Scroll up` and `Scroll down` send three wheel notches at the pointer and **keep the menu
  open** with the same row focused, so OK again scrolls again; Back closes.
- `Send arrows to the app` closes the menu and starts **keys mode**: arrows and OK reach the
  window as keys and the pointer is hidden. The notice reads `Arrows go to the app. Menu for
  more.` for 3 s. While keys mode is on, the row reads `Move the pointer with arrows`, and
  choosing it returns to pointer mode with the pointer where it was.
- `Close app` sends the close request.
- The mode is the window's and lasts until it closes; `Right click` and the two scrolls act
  at the pointer's last place in keys mode too.

## 6. Accessibility

- **The remote alone does everything**, and so does a keyboard alone (3.2): no pointer, no
  gesture and no timed press is needed. Nothing needs a held key; repeats only save presses.
- **Focus is never colour alone**: fill, ring and label on tiles (2.1); fill, ring and `>` in
  menus (3.4); the row position in words (12).
- **Contrast**, existing pairs only: `text` on `selection` 7.6:1, on `menu.bg` and `dock.bg`
  12.9:1; `text.dim` on `dock.bg` 5.1:1 and on `bar.bg` 5.7:1; `text` on the wallpaper at
  least 9:1. `text.dim` is never drawn on `selection` or on the bare wallpaper.
- **Size**: a 32 px glyph on a 1080p panel is about 20 minutes of arc at three metres on a
  55 inch screen. High contrast and the light theme repaint through the same palette as the
  desktop; nothing here names a literal colour.
- **Time**: the two 3 s notices only repeat what the pointer menu says at any time. The
  pointer hiding after 3 s affects an attached mouse only.
- **Reduced motion** has nothing to reduce.
- **Names** in the shell's element tree (`wm show shell`), which a screen reader and an
  agent read alike:

  | Role | Name | Value | Notes |
  |------|------|-------|-------|
  | `button` | `Settings` | | strip; `focused` when it is |
  | `button` | `Power` | | `expanded` while the power menu is open |
  | `text` | `Clock` | `09:14` | |
  | `text` | `Network` | the state's word | listed when an adapter exists |
  | `text` | `Agent` | `idle` or `acting` | listed while connected |
  | `list` | `Open` | `2 windows` | listed when a window is open |
  | `button` | the window's title | `shown last`, or empty | child of `Open` |
  | `list` | `Apps` | `24 apps` | |
  | `button` | the app's name | `running`, or empty | child of `Apps` |
  | `list` | `Settings` | `12 panes` | |
  | `button` | `<pane> settings` | | child of `Settings`: `Network settings`, `Display settings` |
  | `menu` | `Menu of <tile's name>`, `Power`, or `Pointer menu` | | listed while open |
  | `menu item` | the row's text | | `focused` when it is |
  | `text` | `Notice` | the notice's text | listed while shown |

  Exactly one element is flagged `focused`. A tile outside the clip is listed (the tree is
  not the picture), with the flag `offscreen`. In an app the shell's tree has `Clock`,
  `Network`, `Agent` and, for a pointer app, `text` `Remote` valued `pointer` or `keys`.

## 7. `wm`, `settings` and MCP in this form (D22)

Nothing here is remote-only. Each row is one capability and its three front-ends.

| A person, with the remote | CLI | MCP |
|---------------------------|-----|-----|
| moves the focus | `wm act shell <element> focus`; `wm press shell Left`, `Right`, `Up`, `Down` | `wm_act`, `wm_press` |
| OK on a tile, a strip button or a menu row | `wm act shell <element> invoke`; `wm press shell Enter` | `wm_act`, `wm_press` |
| Home | `wm home`; `wm press <id or shell> HomeScreen` | `wm_home`, `wm_press` |
| Back | `wm back`; `wm press <id or shell> Back` | `wm_back`, `wm_press` |
| Menu | `wm press <id or shell> Menu`; a tile's entries are also `wm focus`, `wm close` and `app uninstall` | `wm_press`, `wm_focus`, `wm_close`, `app_uninstall` |
| a media, volume, channel or colour key | `wm press <id> PlayPause` (the names of 9, ask 1) | `wm_press` |
| Power, then a row | `wm press shell Power`; `power off`, `power restart` | `wm_press`; the power tools |
| reads what is on screen | `wm show shell`, `wm list`, `wm capture`, `screenshot` | `wm_show`, `wm_list`, `wm_capture`, `screenshot` |
| opens a pane | `settings open <pane>` | `settings_open` |
| moves the pointer, clicks, scrolls in a pointer app | `wm click`, `wm drag`, `wm scroll` | the same tools |
| `Send arrows to the app` and back | `wm remote <id>` prints `focus`, `pointer` or `keys`; `wm remote <id> keys` and `wm remote <id> pointer` set it | `wm_remote` |
| chooses the TV form | `settings set shell.form tv`; `settings get shell.form_in_use` | `settings_set`, `settings_get` |

- **The `wm` verbs** are the parent's section 5 table, column "Phone and TV": `wm focus`,
  `wm maximise` and `wm restore` show the window; `wm minimise` shows the home screen;
  `wm move` and `wm size` are refused with `fixed_by_form` (`wm size: the TV form shows every
  window at 960x540. Use wm focus to show it.`, `fix_command` `wm focus <id>`).
- **`wm press shell <key>`** is new: the key goes where the remote's would with the home
  screen or a shell menu in front. With an app shown it is refused with `shell_not_in_front`
  (category `refused`, `fix_command` `wm home`), so a script never steers an app by mistake.
- **`wm act shell <element> focus`** is new, for every form's shell tree; on a TV it scrolls
  as 2.2 does. It is refused with `shell_not_in_front` while an app is shown.
- **`wm remote`** answers `not_pointer_driven` (category `refused`) when asked to set a mode
  on a focus app. `wm click` reaches any window, in pointer mode or not.
- **The agent's input never moves the person's focus** on a TV, except through the rows
  above that say so (`wm act shell`, `wm press shell`, `wm home`, `wm back`, `wm focus`).
  `wm click`, `wm type` and `wm press <id>` on a window that is not shown act on it where it
  is. The Agent chip (10) lights as on the desktop, and the agent's arrow is drawn at an
  injected point.
- **Settings**: no key is added. `shell.form`, `shell.form_in_use` and `input.present`
  (`remote`, or `keyboard,remote`) are slice 1's. Settings > Display's line under `Layout`
  reads `Now: TV, chosen from a 960 x 540 screen with a remote.` Every other key works; the
  mouse's keys are kept and matter when a mouse is attached. Arrow repeat is the keyboard's
  delay and rate. `display.resolution` and `display.scale` say what the parent's 1.3 says.

## 8. Rendering and lightness (D24)

- `layout_in_form(…, Form::Tv)` returns no panels, the work area (0, 0, `W`, `H`), the window
  rule `Full`, and the safe area (48, 28, `W` - 96, `H` - 112). The home screen's numbers
  live in one module, `tv.rs`, beside `phone.rs`; nothing else reads the form.
- No new buffers. A focus move damages two tiles (and the row position); a row scroll
  damages the row's 864x134 band; a view scroll damages the rows view once. Idle costs
  nothing: the clock damages its own 40x16 once a minute, a thumbnail at most once a second.
- The pointer in pointer mode is the hardware cursor where there is one.
- A machine in another form carries this layout's code (a few kilobytes) and none of its
  state; a headless machine carries none of it (D18).

## 9. Backend asks

For the kernel and the drivers:

1. **Key codes.** Beside `key::MENU` (127), in the Linux input numbering the table already
   follows (the code checks each against that table): `BACK` 158, `HOME_SCREEN` 172, `POWER`
   116, `MUTE` 113, `VOLUME_DOWN` 114, `VOLUME_UP` 115, `PAUSE` 119, `NEXT` 163, `PLAY_PAUSE`
   164, `PREVIOUS` 165, `STOP` 166, `REWIND` 168, `PLAY` 207, `FAST_FORWARD` 208, `INFO` 358,
   `RED` 398, `GREEN` 399, `YELLOW` 400, `BLUE` 401, `CHANNEL_UP` 402, `CHANNEL_DOWN` 403. A
   remote's OK is Enter, its arrows the arrow keys, its digits the digit keys. `wm press`
   names: `Back`, `HomeScreen`, `Menu`, `Power`, `Mute`, `VolumeDown`, `VolumeUp`, `Play`,
   `Pause`, `PlayPause`, `Stop`, `Rewind`, `FastForward`, `Next`, `Previous`, `Info`, `Red`,
   `Green`, `Yellow`, `Blue`, `ChannelUp`, `ChannelDown`.
2. **Sources mapped to those codes.** USB and Bluetooth HID consumer-control usages (Menu
   0x40, Menu Pick 0x41 as OK, Menu Up, Down, Left, Right 0x42 to 0x45, Menu Escape 0x46 as
   Back, AC Home 0x223, AC Back 0x224, Play 0xB0, Pause 0xB1, Fast Forward 0xB3, Rewind 0xB4,
   Next 0xB5, Previous 0xB6, Stop 0xB7, Play/Pause 0xCD, Mute 0xE2, Volume 0xE9 and 0xEA,
   Channel 0x9C and 0x9D, Power 0x30); HDMI-CEC user control codes (Select, Up, Down, Left,
   Right, Root Menu as Home, Contents Menu as Menu, Exit as Back, Play, Pause, Stop, Rewind,
   Fast forward, the volume and channel keys, the digits, F1 to F4 as the colour keys); and
   an infrared receiver's decoded keys through one table.
3. **Remote detection.** A device is a `remote` when it is a CEC source, an infrared
   receiver, or a HID device that reports Menu Pick or the arrow usages, has no pointer
   collection, and declares no letter keys (keyboard usages 0x04 to 0x1D). A remote with a
   built-in air mouse is `remote` and `pointer`, so `auto` picks `desktop` (the parent's row
   5) and the owner of such a remote sets `shell.form tv`. The kernel tells the desktop when
   a remote comes or goes (`Desktop::set_input_present`), backs `input.present`, and keeps
   `input::pretend_remote` behind its boot option for QEMU and the tests.
4. **Key routing.** The server keeps `HOME_SCREEN` and `POWER` for itself in the `tv` form,
   keeps `MENU` when the shown window lacks `FOCUS_NAVIGABLE`, and sends every other key of
   ask 1 to the shown window untouched. Two arrows held at once are reported as two keys
   down.
5. **Window flags** (the parent's ask 6): `FOCUS_NAVIGABLE` and `HANDLES_BACK` with
   `BACK_UNUSED`; `CHANGE_FLAGS` may change both on a live window.
6. **Per-window remote mode.** The server holds `pointer` or `keys` for each pointer app,
   synthesises `PointerMove`, `Button` and `Wheel` from the keys in pointer mode, and answers
   `GET_REMOTE_MODE` and `SET_REMOTE_MODE` for `wm remote`.
7. **Thumbnails.** The buffers of minimised windows stay readable for the `Open` row, as the
   overview needs.
8. **The cursor.** Shown and hidden by the server: at the centre on showing a pointer app,
   hidden in keys mode, and for an attached mouse hidden 3 s after it stops.

For the toolkit:

9. The TV's root padding (48, 28) from `GET_FORM`, the `full_bleed` root option, and the menu
   bar inside the padding.
10. `FOCUS_NAVIGABLE` and `HANDLES_BACK` set by `application::run`; the terminal sets the
    first itself. Arrow navigation to the nearest widget, stopping at the edges, with the
    ring always drawn; `Back` as Esc, then `BACK_UNUSED`.
11. Dialogs drawn with the ring on their default button from the first frame.

For the shell and the tools: `wm press shell`, `wm act … focus`, `wm remote` and their MCP
twins; the errors `shell_not_in_front` and `not_pointer_driven`; the TV rows in `wm show
shell`.

## 10. Acceptance (host tests, 1920x1080 device px at scale 2: 960x540 logical, dark palette,
`shell.form` `auto`, a pretend remote and no pointer, an adapter, no agent; Terminal, Files,
Settings and four installed apps; rects in logical px)

1. **The form and the layout**: `shell.form_in_use` prints `tv` and `input.present` prints
   `remote`. `layout_in_form(960, 540, 2, profile, Tv)` gives no panels, the work area (0, 0,
   960, 540), the rule `Full` and the safe area (48, 28, 864, 484), the same for all three
   profiles. `settings set shell.form tv` at 1280x800 with a keyboard and a pointer shows the
   TV form with 7 whole tiles in a row and the three rows whole.
2. **Margins**: with the home screen shown, no pixel outside (48, 28, 864, 484) differs from
   the wallpaper, in every state of tests 3 to 9.
3. **The strip**: the egg's top row is at y 40, centred on x 55, in `accent`. `Settings` is
   (80, 36, 96, 28) in `dock.bg` with a `menu.border` edge and its glyphs from (96, 42) in
   `text.dim`; `Power`'s glyphs start at (212, 42). The clock's glyphs start at (872, 42) in
   `text`, the network glyph is at (840, 42), and with an agent connected the chip is (768,
   40, 56, 20) with its text from (776, 42).
4. **The rows with nothing open**: the title `Apps` is on a `bar.bg` pill (48, 94, 40, 20)
   with glyphs from (52, 96) in `text.dim`; no `Open` title is drawn. Terminal's tile is (48,
   120, 144, 108), Files' starts at x 208, the sixth tile at x 848 with pixels drawn to x 911
   and no label. The title `Settings` is on a pill (48, 250, 72, 20); the tile `Appearance`
   is (48, 276, 144, 108) with the gear at (96, 292) and its name from (80, 352) in
   `text.dim`. Twelve pane tiles exist, in 1.3's order.
5. **The focus at boot**: Terminal's tile has `focus.ring` at (56, 121) and `selection` at
   (56, 123); its icon is at (96, 136); `Terminal` starts at (88, 196) in `text`. Files' tile
   has `menu.border` at (216, 120), `dock.bg` at (216, 122) and `Files` from (260, 196) in
   `text.dim`. The row position reads `1 of 7` on a pill (856, 94, 56, 20) with its glyphs
   ending at x 908. Exactly one element is drawn with the ring.
6. **Left and Right**: Left at the first tile changes nothing. Four Rights focus the fifth
   tile at x 688 with no scroll. A fifth Right focuses the sixth tile, now at x 688, with
   Files at x 48 and the position `6 of 7`. A sixth focuses the seventh at x 688, and the
   columns x 848 to 911 of the row are wallpaper. A seventh changes nothing. A held Right
   from the first tile ends on the seventh.
7. **Up and Down**: Down focuses `Appearance` (the ring at (56, 277)), Terminal's tile is
   `dock.bg` with its label `text.dim`, and the row position reads `1 of 12` on the
   `Settings` row only. Down again changes nothing. Up returns to the tile `Apps` last had. Up again
   focuses `Settings` in the strip (`selection`, the ring, `text`); Right focuses `Power`;
   Right again and Up change nothing; Down returns to `Apps`.
8. **Opening and Home**: OK on Terminal shows the notice `Starting Terminal...` at (320, 456,
   320, 40). When the window maps, `wm list` has it `maximised` at (0, 0, 960, 540), no strip
   pixel remains, and the terminal's first cell is at (48, 28) with 108 columns and 30 rows.
   Home shows the home screen with three rows: `Open` at y 96 with Terminal's tile focused,
   its thumbnail at (56, 128, 128, 72) and its title from (88, 204); `Apps` at y 252, with
   Terminal's 4x4 `text` square at (118, 344); `Settings`' tiles from y 432 with (56, 511)
   `dock.bg`, (56, 512) wallpaper and no label drawn. Two Downs put `Apps` at y 96 and
   `Settings`' tiles at y 276, and `Open` is not drawn. OK on Terminal's `Apps` tile shows
   the same window (no second one).
9. **The tile menu**: Menu on Terminal's `Open` tile opens (320, 234, 320, 72); `Open` is at
   (344, 246) in `text` on `selection` with `>` at (328, 246); `Close` is at (344, 278) in
   `text`; the tile under it keeps `selection` and has no ring; pixel (10, 100) is unchanged.
   Down focuses `Close`; Down again changes nothing; Back closes the menu and the tile has
   the ring again. Menu, Down, OK closes Terminal: `Open` is gone and the focus is on the
   first tile of `Apps`. Menu on an installed app's tile lists `Open` and `Uninstall...`; on
   a pane's tile only `Open`, at (320, 250, 320, 40); on a strip button nothing opens.
10. **Power**: OK on `Power` opens (320, 234, 320, 72) with `Power off` and `Restart`; Back
    closes it. The key `Power` opens it over a shown app, and Back returns to the app.
11. **Back**: on the home screen Back changes nothing. In a toolkit window with a find bar
    open, Back closes the bar; the next Back shows the home screen with the window
    `minimised` and its `Open` tile focused. With a dialog open, Back sends the dialog the
    close request and the window stays shown.
12. **A dialog** of 320x160 opened by the shown window is centred on x 480 and y 270, has a
    title bar with no control circle, is not at the screen's bottom, and pixel (10, 100) is
    unchanged. Its default button has the ring in its first frame.
13. **The toolkit**: a toolkit window in the `tv` form has its menu bar at (48, 28, 864, 28)
    and no widget outside (48, 28, 864, 484); its background fills (0, 0, 960, 540); the ring
    is drawn before any key; Right moves it to the nearest widget to the right and stops at
    the last; Menu arms the menu bar. The same window in the `desktop` form has no padding.
14. **Pointer mode**: showing a window without `FOCUS_NAVIGABLE` draws the arrow with its
    hot spot at (480, 270) and the notice `Arrows move the pointer. OK clicks. Menu for
    more.` One Right moves it to (488, 270) and the window gets `PointerMove`. A Right held
    for 2 s ends at x 959. OK sends the primary button down, then up. Back sends Esc. Home
    shows the home screen.
15. **The pointer menu**: Menu opens (320, 186, 320, 168) with five rows from y 190. `Scroll
    down` sends three wheel notches and the menu stays open on that row. `Right click` closes
    it and sends the secondary button. `Send arrows to the app` hides the pointer; Right then
    reaches the window as a key; the menu's fourth row reads `Move the pointer with arrows`,
    and choosing it shows the pointer again where it was. `wm remote <id>` prints `pointer`,
    then `keys`; on a toolkit window it prints `focus`, and `wm remote <id> keys` fails with
    `not_pointer_driven`.
16. **Media keys**: `PlayPause`, `VolumeUp` and `Red` reach the shown window as the keys of
    ask 1, in a focus app and in a pointer app; on the home screen they change nothing.
17. **`wm`**: with Terminal shown, `wm focus <Files' id>` shows Files and Terminal is
    `minimised`; `wm size <id> 300 300` and `wm move <id> 10 10` fail with `fixed_by_form`
    naming 960x540; `wm home` and `wm back` do what the keys do; `wm press shell Right`
    fails with `shell_not_in_front` while an app is shown and moves the focus on the home
    screen; `wm press <id> Back` and `wm press <id> HomeScreen` act as the remote's keys.
18. **The tree**: `wm show shell` on the home screen lists `Settings`, `Power`, `Clock`,
    `Network`, the lists `Open` (`1 window`), `Apps` (`7 apps`) and `Settings` (`12 panes`)
    with their buttons (`Network settings` among them), Terminal's app button valued
    `running`, and exactly one element `focused`. `wm act shell <Files' id> focus` moves the
    ring to Files; `invoke` shows Files; `invoke` on `Display settings` opens Settings on
    Display. With the tile menu open, a `menu` named `Menu of Terminal` and its items are
    listed.
19. **A keyboard and a mouse**: with a keyboard added the form stays `tv`; Esc is Back, Super
    is Home, Shift+F10 is Menu, Tab moves from `Power` to the first tile. With a mouse added
    the form stays `tv`; a click on Files' tile focuses and opens it; hover changes no pixel.
20. **The other forms are untouched**: every desktop and phone test passes, and changing
    from `tv` to `desktop` on the same screen with two windows keeps both, the shown one
    maximised. At 960x540 device px and scale 1 (`shell.form` `tv` forced) every rect above
    holds in device px.

## 11. Left out, on purpose

The remote's on-screen keyboard (its own brief; until then text needs a keyboard or the
agent), type-ahead on the home screen, a search row, sound and its volume overlay, rows from
`app.actions` and recent files, arranging or hiding tiles, a `Hold click` entry for dragging
with one-key remotes, sideways scrolling in the pointer menu, a setting for the margins on
panels that cut nothing (`display.overscan`), CEC power control of the TV itself, voice, and
scale 4 for 2160p.

## Decided by design (the owner can overrule any of these)

- The focused tile does not scale up: fill, ring and label are the cues, because nothing
  moves and pixel art does not scale by fractions.
- The top strip is fixed and its only focus stops are `Settings` and `Power`; the egg is a
  mark, since Home is the launcher's key.
- Row titles sit on the `bar.bg` pill, and the focused row says its position in words (`3 of
  24`), so no new colour is needed on the wallpaper and scroll is never the only cue.
- A cut tile draws no label; rows and tiles scroll by whole steps, only as far as needed.
- Menus are centred popups with nothing darkened, and one ring is on screen at a time.
- Dialogs keep their title bar and lose their control circles; Back closes them.
- Home, Power and (in a pointer app) Menu belong to the shell; no app can take them.
- `Scroll up` and `Scroll down` keep the pointer menu open; the remote mode is per window
  and has a twin, `wm remote`.
- `wm press shell` and `wm act shell … focus` are added so the focus itself has a CLI and an
  MCP front-end; both are refused while an app is in front.
- Every window is 960x540 and the toolkit pads its root by (48, 28); a foreign app is not
  padded, and a margin setting waits for a panel that needs it.
- Key codes follow the Linux input numbers the key table already uses; a remote with an air
  mouse counts as a pointer, so its owner forces the form.
- No setting is added, no colour token is added, and nothing moves.
