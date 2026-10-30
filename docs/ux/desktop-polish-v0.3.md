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
