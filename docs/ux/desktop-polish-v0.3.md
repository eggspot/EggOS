# EggOS v0.3.0 desktop polish: pointer shapes, small displays, boot screen details

Design by Claude Fable (D16) for batch part 34 (D186), "Desktop polish": the details the v0.3
briefs left open, in the same system: 1 px `#000000` outlines, `#FFFFFF` fills, no
anti-aliasing, no alpha, no motion but the cursor blink. Grids: `X` black, `o` white, `.`
transparent; every shape is uploaded once as a 64x64 ARGB cursor and pixel-doubled at scale 2.
Everything in `desktop-v0.3.md` (section 1, the boot screen) and `desktop-v0.3-gui.md` (section
2, the pointer shapes) stands.

## 1. Pointer shapes: `Hand` and `Busy`

`Hand`, 17x19, hotspot (5, 0), the tip of the index finger. Index finger raised, three folded
fingers, the thumb on the left (the pointer points up, like the arrow's tip at its top-left).

```
.....XX..........
....XooX.........
....XooX.........
....XooX.........
....XooX.........
....XooXXX.......
....XooXooXXX....
....XooXooXooXXX.
.XX.XooXooXooXooX
XooXXooXooXooXooX
XoooooooooooooooX
XoooooooooooooooX
.XooooooooooooooX
.XoooooooooooooX.
..XooooooooooooX.
..XoooooooooooX..
...XoooooooooX...
...XoooooooooX...
...XXXXXXXXXXX...
```

`Busy`, 13x16, hotspot (6, 8), the centre. An hourglass mid-pour: sand still in the upper bulb,
a 1 px stream, a pile below. Static; it never turns or trickles.

```
XXXXXXXXXXXXX
.XoooooooooX.
.XoooooooooX.
.XoXXXXXXXoX.
..XXXXXXXXX..
...XXXXXXX...
....XXXXX....
.....XXX.....
.....XXX.....
....XoXoX....
...XooXooX...
..XoooXoooX..
.XooooXooooX.
.XoooXXXoooX.
.XoXXXXXXXoX.
XXXXXXXXXXXXX
```

What maps to each (`SET_POINTER`, `desktop-v0.3-gui.md` section 4.3):

| Client request | Shape | Why |
|---|---|---|
| Windows `IDC_HAND`; macOS `pointingHandCursor`; Wayland / CSS `pointer` | `Hand` | links and web-style clickables |
| Windows `IDC_WAIT`; Wayland / CSS `wait` | `Busy` | input is blocked: wait |
| Windows `IDC_APPSTARTING`; Wayland / CSS `progress` | `Arrow` | these mean "working, but you may still click"; a bare hourglass would say the opposite, and macOS shows nothing for app starts either. A composite arrow-plus-hourglass is a follow-up. |
| macOS `openHandCursor`, `closedHandCursor`; anything else unknown | `Arrow` | not in 0.3.0 |

macOS has no client-requested busy cursor: its spinning cursor is the window server's own
verdict, and so is ours. **The server shows `Busy` itself**: over the content of any window
whose client has not read its events for 2 s, with or without a `CloseRequest` pending, the
same condition that puts `(not responding)` after the title (`desktop-v0.3-gui.md` section 3.6
generalised: the suffix and the shape appear together). The server's `Busy` outranks the
client's chosen shape; over the title bar and borders the arrow and resize shapes stay, so the
window can still be moved or closed. Both end the moment the client reads an event. The server
never shows `Hand` on its own: panels, launcher, chips, controls and icons keep the arrow, as on
every desktop people come from.

## 2. The full-screen console: displays too small for the desktop

The desktop needs its default terminal window inside the work area: at the default profile that
is 658x458 at scale 1 (658x430 plus the 28 px bar). The rule is the layout function's
(`desktop-v0.3-gui.md` section 12): if the default terminal window does not fit the work area at
scale 2, scale drops to 1 and the desktop is drawn; only when it does not fit at scale 1 either
does the display get the **full-screen console** instead. 640x400, 320x240 and smaller embedded
panels land here.

- The whole display is `term.bg`. The cell grid starts at (8, 8) with `cols = (w - 16) / 8`,
  `rows = (h - 16) / 16`; leftover pixels stay `term.bg` right and bottom. 640x400 is 78x24,
  320x240 is 38x14. Below 320x240 the padding is 0 (`cols = w / 8`, `rows = h / 16`): 160x128
  is 20x8. A display that cannot hold one cell (w < 8 or h < 16) is treated as no display
  (`boot: no display`). Always scale 1: a doubled glyph on a tiny panel wastes it.
- No bar, no title, no clock, no power menu, no window chrome, no pointer drawn (no cursor
  resource is uploaded). It is one terminal, nothing to manage.
- Keys: every key goes to the terminal exactly as in the window (`terminal-v0.3.md`, including
  Shift+PageUp/PageDown/Home/End for the scrollback). Super shortcuts, the switcher, the window
  menu and `Alt+F4` / `Cmd+W` / `Ctrl+W` do nothing. Power is the `shutdown` command (init).
- Pointer: motion and buttons are consumed and ignored; the wheel scrolls the scrollback by 3
  lines per notch. While scrolled back the `▲` sits in the top-right cell as usual.
- Cursor: the console always has focus, so the cursor is always the filled block, blinking
  500 ms on / 500 ms off, restarted by every byte, `CSI ? 25 l` hides it, `CSI ? 12 l` makes it
  steady. The 1 px unfocused outline never appears here.
- `TIOCGWINSZ` reports the grid (`ws_xpixel = cols*8`, `ws_ypixel = rows*16`). The serial side
  says which layout the display got: `boot: display 640x400 console` versus `boot: display
  1280x800 desktop`. `wm list` reports no windows; `wm type` and `wm_type` still type into it;
  the Display pane does not exist here (no Settings GUI without the desktop).
- The same layout is the right one for a display without the desktop component (D18), then at
  the display's scale (2 at height ≥ 1440); the tiny-display case above is always scale 1.

## 3. The boot screen on small displays, and its end

The full stack (section 1 of `desktop-v0.3.md`) is 196 px tall (egg 96, 16, name 32, 8, version
16, 24, progress 4) and 172 px wide (the progress bar); the background `#0E1726` always fills
the display. Three tiers, chosen once from the display's size at scale 1; the block is centred
vertically in every tier (the full tier keeps its egg top at `h / 2 - 96`):

| Tier | Used when | Layout |
|---|---|---|
| Full | w ≥ 188 and h ≥ 212 | unchanged (320x240 qualifies: egg top 24, progress bottom 220) |
| Compact | w ≥ 124 and h ≥ 104, and not Full | egg H 48, W 35, `#F6F1E7`, yolk r 5 at 30 below its top; gap 8; `EggOS` at 1x (40 wide, `#E6EAF0`); no version line; gap 12; progress of four 24x4 segments with 4 px gaps (108 wide). 88 px tall |
| Minimal | w ≥ 32 and h ≥ 8, and not Compact | the progress bar only: four segments `(w - 28) / 4` wide, 4 px tall, 4 px gaps, 8 px side margins, centred vertically |

What goes first is what carries the least: the version, then the name's size, then the egg; the
progress bar is the last thing standing, because a user staring at a slow boot needs it most.
A display below 32x8 (or none) has no boot screen; its first frame is the console.

Segment 2 on a machine with no input device: **it lights**. The segments show the boot passing
its stages, not an inventory: segment 2 lights when the input probe has finished, whether or not
it found anything. The serial line says what was found: `boot: input keyboard pointer`,
`boot: input none`. An unlit segment 2 would read as "stuck at input".

The end of the boot screen, with one precision: segment 4 is never flushed on its own. The frame
that would light it is the first desktop (or full-screen console) frame, one flush of the whole
display, no fade. Its trigger is the first read of the console by a user process after init
started, which is when the shell's prompt is out (the shell writes it, then reads). The hard
limit is 1 s after init started (segment 3): then the desktop is drawn anyway with whatever the
terminal holds, and the serial side says `boot: desktop (timeout)`. If init cannot start at all,
the desktop or console is drawn at once, so the kernel's message is on the display, not only on
serial. Serial for the normal case: `boot: desktop 212ms` (from kernel entry), the number the
first-frame test reports beside boot-to-prompt (D108).

## 4. The rest of the polish: `Working`, the server's own `Busy`, hanging off the left, the launcher's caret (D221)

Design by Claude Fable for batch part 4 (D221), D186's "Not yet". Same system as sections 1 to
3; everything above stands except where this section says otherwise. Scale 2 pixel-doubles
every bitmap and every measure below (a 22x19 shape is 44x38, 48 px of grab is 96 device px).

### 4.1 `Working`: the arrow with a small hourglass

`Working`, 22x19, hotspot (0, 0), `PointerShape` value 9. The arrow of `desktop-v0.3.md`,
unchanged in its 12 columns, and a 9x9 hourglass whose top-left is at (13, 10): one
transparent column (12) between them, its bottom row (18) one below the arrow's tail, so the
arrow still reads as the pointer and the hourglass as a badge. The small hourglass is the
`Busy` glass with one row of sand above the neck and one pile row below: still mid-pour,
still static.

```
X.....................
XX....................
XoX...................
XooX..................
XoooX.................
XooooX................
XoooooX...............
XooooooX..............
XoooooooX.............
XooooooooX............
XoooooooooX..XXXXXXXXX
XooooooXXXXX..XoooooX.
XoooXooX......XoXXXoX.
XooX.XooX......XXXXX..
XoX...XooX......XXX...
XX....XooX.....XoXoX..
X......XooX...XooXooX.
........XX....XoXXXoX.
.............XXXXXXXXX
```

173 opaque pixels: the arrow's 114 and the hourglass's 59. The 64x64 cursor image holds it
like every other shape.

**When the server shows it.** From a `Start` launch (the launcher, a desktop icon, the dock,
`app open`, `app_open`) until the started client's first commit maps a window, the started
process exits, or `APP_STARTING_MS` = 5000 ms pass, whichever comes first. It replaces the
arrow exactly where the shape function answers `Arrow` (desktop, panels, title bars, content
whose client chose `Arrow`, a drag); `IBeam` over the launcher's field, the resize shapes, a
client's `Hand` or `IBeam`, and the server's `Busy` all outrank it, because each says
something more specific about the spot under the pointer. A second launch while one is
pending restarts the deadline; a `Type` launch (a foreign console program typed into the
terminal) never shows it, the terminal shows what happens. The agent's accent arrow (14.1)
never carries the badge: it is the person's pointer that says "I am working on your request".

**Client requests.** Section 1's table changes one row: Windows `IDC_APPSTARTING`, Wayland /
CSS `progress` now map to `Working` over that window's content (the follow-up the row
announced). 5 s is the ceiling because EggOS's own apps map a window well under a second and
a foreign app's first run (translation, D17) is the only honest reason to wait longer; past
5 s the pointer must stop promising.

### 4.2 The server's own `Busy`: a client that stops reading its events

The server tracks per client the time its oldest unread event was queued. A window whose
client has held an event unread for `UNREAD_EVENTS_BUSY_MS` = 2000 ms is **not responding**:
its title bar shows ` (not responding)` after the title (the suffix of section 3.6, one term
for one state, the one Windows users know; macOS users know the pointer), the pointer over
its content is `Busy` (section 1), and `ListedWindow.responding` is false. A client with no
pending event is idle, not busy, however long it computes; hovering its content queues
motion events, so a hung app is caught within 2 s of the pointer reaching it. Everything
clears in the tick the client reads an event: suffix, pointer, flag and the dialog below.
The close path keeps its own clocks (500 ms to the suffix, 2000 ms to destruction); both
paths set the one flag.

**No dimming, no label over the content.** The last frame stays as it is: it is information
(what the person was doing), a dimmed copy would cost a content-sized compose per frame
against D24, and the state is already said twice. Clicks and keys on a busy window queue as
on any window (the client may catch up) and a click still raises and focuses it; the title
bar moves, the controls work, close goes the 3.6 way.

**The End app dialog.** Once a window has been not responding for `END_APP_OFFER_MS` =
5000 ms, a button-down on its content opens the server's own popup instead of queueing:
320x120, `dock.bg`, popup chrome, centred on the window's visible rectangle and clamped into
the work area. Line 1 at (12, 12) in `text`: the title cut to 19 cells by `cut_title`, then
` is not responding`. Line 2 at (12, 36) in `text.dim`: `Ending it loses its unsaved work.`
When the client is eggway (one `/dev/window` connection for every Wayland app), line 3 at
(12, 60) in `text.dim`, the same step: `It also ends every Linux window app.` (36 cells, 288 px,
ending 4 px above the buttons; added by the build loop, D221's research pass, in the dialog's
own words and tokens).
Two 96x28 buttons at y 80: `Wait` at x 212 (focused, Enter and Esc), `End app` at x 108, so
a hasty Enter waits. `Wait`, Esc or a click outside closes it and leaves the window alone;
`End app` kills the client's process (`Action::EndClient(client)`) and its windows go in the
same tick. The dialog closes by itself when the client reads an event. Agents never see it:
it is for the person, so `dialog_open` is not reported for it.

**What an agent sees (D22).** `wm list` keeps its columns; the title column carries the
suffix exactly as the title bar shows it, and `--json` adds `"responding": true|false` to
every row. `wm show` on a busy window does not wait on the client: it answers at once with
the root alone (`role` `window`, the name, `flags` `["not-responding"]`). `wm act`, `wm type`
and `wm menu` on a busy window answer the error `not_responding` (kind `busy`, exit 75, then
`wm list`) at once, and so does every other input an agent would put in its app's queue
(`wm click`, `wm drag`, `wm press`, `wm scroll`; D221's research pass): an agent cannot
see what it would act on. Moving, sizing and closing it are the server's and stay allowed. `wm close` stays the way to end it from the CLI: the request goes unread
and the window is destroyed 2 s later, as today. The MCP tools mirror all of it.

### 4.3 Windows hanging off the left edge

Window positions become signed (`i32`; `wm list`, `wm show` and `wm move` print and accept
negative x). The clamp of `desktop-v0.3-gui.md` 3.5 becomes symmetric and counts **grab**, not
title bar: at least `HANGING_TITLE_KEPT` = 48 px of the title bar's draggable part stays
inside the work area horizontally, so the kept width is 48 plus the controls' width
(`TITLE_SIDE`, 68) when the controls are on the side that stays visible. Hanging off the
left shows the window's right end: kept 116 in the Windows and Linux profiles, 48 in macOS.
Hanging off the right shows its left end: kept 48 in Windows and Linux, 116 in macOS. (The
plain 48 of D186 left only controls visible in one profile per side, and a window that can
only be dragged back by keyboard is a trap.) A window narrower than the kept width stays
wholly inside. The top edge still never goes above the work area's top: the title bar is the
handle and must be whole; the bottom keeps the title bar's height plus the border, as today.

No snapping back: the window stops at the limit while the pointer keeps going and the drag
offset is kept, as now. The same clamp applies to the window menu's `Move` and its arrow keys,
to `wm move` and to a client's own placement, and is re-applied to every window when the work
area shrinks (a panel appears, the display mode changes). Restore after maximise returns to a
hanging geometry unchanged, it is legal.

### 4.4 The caret in the launcher's search field

The 1x16 `text` caret of 6.2 blinks `CARET_BLINK_MS` = 500 ms shown, 500 ms hidden, from the
moment the launcher opens (shown first) and restarted shown by every keystroke that reaches
the field (a typed character, Backspace), like the toolkit's and the terminal's. After
`CARET_STEADY_AFTER_MS` = 10 000 ms without a keystroke it stops blinking and stays shown,
never hidden (a vanished caret says "no focus", and the field always has focus while
the launcher is open), and the next keystroke restarts the blink; the shell runs no timer
while it is steady (D24). With `reduce_motion` on it is always shown and steady
(`first-run-accessibility-v0.3.md` 2.5 already lists it). A toggle damages only the caret's
1x16 rectangle. The caret's x is unchanged: `FIELD_TEXT_X` plus 8 px per typed byte, before
the placeholder when the query is empty.

### 4.5 Device classes (D23)

- **Phone and tablet (no pointer).** Nothing can carry `Working` or `Busy`, so the surfaces
  speak. A launch from the launcher keeps it open with the tapped row in `selection` and the
  notice line reading `Starting <name>...` until the first window maps (then it closes) or
  the 5 s deadline or exit (notice `<name> did not start`, 3 s); other launch paths show
  nothing until the window appears. A busy window's 28 px strip carries the same suffix, and
  a tap on its content after 5 s opens the End app dialog as a bottom sheet (full width, 120
  tall, the two buttons at the right), as section 12 makes every dialog.
- **TV.** The pointer is hidden until it moves (section 12), so `Working` shows only once it
  moves; the focus ring stays where it was and nothing else is added. The End app dialog opens
  from `OK` on a busy window's content and is arrow-navigable, `Wait` focused.
- **2x.** Pixel doubling throughout: `Working` 44x38 at hotspot (0, 0), the dialog 640x240 with
  192x56 buttons, kept grab 96 or 232 device px, the caret 2x32.

### 4.6 Acceptance checks (host tests)

1. `size_and_hotspot(Working) == ((22, 19), (0, 0))`; `from_value(9) == Some(Working)`,
   `from_value(10) == None`; every bitmap row is 22 cells.
2. For every (c < 12, r < 19), `pixel_at(Working, at, c, r) == pixel_at(Arrow, at, c, r)`;
   column 12 is transparent in every row; rows 0 to 9 are transparent from column 12 on.
3. With `at` = (100, 50): (113, 60) black, (115, 61) white, (116, 62) black, (112, 60) none,
   (113, 68) black, (121, 68) black, (122, 50) none; the cursor image has 173 opaque pixels.
4. Shape function with a start pending: `Arrow` becomes `Working` over the desktop, a title
   bar, a drag and content whose client chose `Arrow`; `IBeam` over the launcher field, the
   resize shapes, a client's `Hand`, and `Busy` over a not-responding window are unchanged.
5. A start pending at t: `Working` at t + 4999, `Arrow` at t + 5000; `Arrow` in the tick of
   the started client's first commit; `Arrow` when its process exits; a second `Start` at
   t + 3000 keeps `Working` until t + 8000; a `Type` launch sets nothing.
6. A client's `SET_POINTER` 9 shows `Working` over its content; the compatibility layers map
   `IDC_APPSTARTING` and CSS `progress` to 9.
7. An event queued at t, unread: at t + 1999 the title has no suffix, the pointer over the
   content is the client's shape, `responding` is true; at t + 2000 all three flip; a read at
   any later time flips them back in the same tick. A client with no pending event never
   flips. Queued at t, read at t + 1000, queued again at t + 1500 and unread: flips at
   t + 3500 (the oldest unread event's clock, not the last read's).
8. Button-down on a busy window's content at 4999 ms busy: raises, focuses, queues, no popup;
   at 5000 ms: the popup is 320x120, centred on the window's visible rectangle and inside the
   work area, `Wait` focused; Enter, Esc and a click outside close it with the window intact;
   `End app` returns `Action::EndClient(client)` and the window is gone in that tick; a read
   by the client closes the popup by itself.
9. `wm list` text: `... 67 Editor (not responding)`; `--json`: `"responding":false`; `wm show`
   on it answers within one tick with one element flagged `not-responding`; `wm act`,
   `wm type`, `wm menu`, `wm click`, `wm drag`, `wm press` and `wm scroll` answer
   `not_responding`, exit 75; `wm move` still moves it. The End app dialog of eggway's window
   has line 3.
10. Dragging a 658-wide window: Windows profile, left limit x = work.x − 542, right limit
    x = work.right − 48; macOS profile, left limit x = work.x − 610, right limit
    x = work.right − 116. Every profile: y ≥ work.y, y ≤ work.bottom − 29. A 100-wide window
    never leaves the work area horizontally. `wm move <id> -600 100` lands on the same limits
    and `wm list` prints the negative x. Shrinking the work area re-clamps a hanging window.
11. Launcher opened at t0: caret drawn at t0 and t0 + 499, not at t0 + 500 and t0 + 999,
    drawn at t0 + 1000; a keystroke at t0 + 700: drawn from t0 + 700 to t0 + 1199, not at
    t0 + 1200; no keystroke since t0: drawn at every instant from t0 + 10 000 on and no timer
    is armed; a keystroke then restarts the blink; `reduce_motion` true: drawn at every
    instant; a toggle damages exactly the 1x16 rectangle at `FIELD_TEXT_X` + 8 × len.
12. 360x640 phone layout: a `Start` launch keeps the launcher open, the row `selection`, the
    notice `Starting Files...`; the first commit closes it; the deadline shows `Files did not
    start`; a busy window's strip reads the suffix; a tap after 5 s opens a 360x120 sheet at
    the bottom.
13. TV layout with the pointer hidden: a pending start leaves the shape `Hidden`; the first
    motion shows `Working`.
14. Scale 2: the `Working` cursor image pixel (2c + i, 2r + j), i, j in {0, 1}, equals the 1x
    pixel (c, r); the kept grab is 96 and 232 device px.
