# EggOS v0.3.0 desktop: design brief

Design by Claude Fable (D16, D109) for roadmap items 11 to 14: the boot screen, the design
tokens, the wallpaper, the top bar, the terminal window, and the rendering plan. The terminal
emulator itself (ANSI subset, keyboard map, line discipline) is in `terminal-v0.3.md`; the font
is `font-8x16.txt`. Everything here is drawn by our own Rust code on a software framebuffer:
no image files, no third-party fonts, no anti-aliasing, no alpha blending except the two shift
tricks named below. Every number is in pixels at scale 1x unless stated; at 2x every number
doubles (see "Scale and form factors").

Target hardware for 0.3.0: QEMU with `virtio-gpu` (2D scanout, 1280x800 preferred, 1024x768
fallback), `virtio-keyboard` and `virtio-tablet` (absolute pointer) or `virtio-mouse`, on
aarch64, riscv64 and x86_64. Boot to a usable desktop in under a second.

## 1. Boot screen

The first frame replaces whatever the firmware left on the display, and it is what the user
looks at whenever a boot is slow (real hardware later). It is drawn once the scanout exists,
in one flush, and never held on screen for a minimum time: on QEMU the shell prompt comes
50 to 120 ms after the kernel starts (D108), so the desktop replaces it almost at once.

- Background: solid `#0E1726` (the wallpaper's top colour), full screen.
- Logo: the egg shape (section 3) with H = 96, W = 69, filled `#F6F1E7`, centred horizontally,
  its top at `screen_h / 2 - 96`; the yolk circle r = 9, colour `#F2B33D`, centred on the
  egg's axis at 0.62·H below the egg's top.
- Name: `EggOS` in Egg Mono at 2x (16x32 per glyph, 80 px wide), colour `#E6EAF0`, centred,
  top at the egg's bottom + 16. Version: `v<CARGO_PKG_VERSION>` at 1x, colour `#8A94A6`,
  centred, top at the name's bottom + 8.
- Progress: four segments of 40x4 px with 4 px gaps (172 px wide), centred, top at the
  version's bottom + 24. Unlit `#26303F`, lit `#F2B33D`. Segment 1 is lit in the first frame
  (display ready); 2 lights when input devices are ready; 3 when init has started; 4 when the
  shell prompt has been written, at which point the desktop is drawn and the boot screen is
  gone. Each stage change flushes only the segment's rect.
- Serial console: the same stages as plain lines, `boot: display 1280x800`, `boot: input
  keyboard pointer`, `boot: init`, after the existing kernel banner; no ANSI codes, no art
  (the serial side may be a log file). When there is no virtio-gpu device (QEMU's
  `-display none` keeps the device; this is the case without one) the kernel prints
  `boot: no display` and everything else proceeds on the serial console as today.
- Timing targets on the Mac's QEMU (TCG, 4 CPUs): first frame ≤ 100 ms after kernel entry,
  desktop with the prompt ≤ 300 ms, hard limit 1 s. Measured by the first-frame test
  (section 9) and reported like boot-to-prompt (D108).

## 2. Design tokens

### Colours (sRGB, `#RRGGBB`; as a framebuffer `u32` this is `0x00RRGGBB`, section 8)

| Token | Value | Used for |
|-------|-------|----------|
| `wallpaper.top` | `#0E1726` | gradient at row 0, boot screen background |
| `wallpaper.bottom` | `#1B3A5C` | gradient at the last row |
| `wallpaper.egg` | `#1F3F63` | the egg silhouette |
| `accent` (yolk) | `#F2B33D` | yolk, progress, terminal cursor, minimise button |
| `shell` | `#F6F1E7` | boot logo egg |
| `bar.bg` | `#141B26` | top bar, opaque |
| `bar.edge` | `#202938` | 1 px line under the top bar |
| `bar.hover` | `#26303F` | hovered bar item, task chip |
| `bar.pressed` | `#334052` | pressed bar item |
| `menu.bg` | `#1C2430` | power menu |
| `menu.border` | `#2A3340` | 1 px menu border |
| `menu.hover` | `#2A3440` | hovered menu item |
| `text` | `#E6EAF0` | all UI text on dark surfaces |
| `text.dim` | `#8A94A6` | inactive title, version, uptime hint |
| `title.active` | `#2A3340` | title bar of the focused window |
| `title.inactive` | `#1F2630` | title bar of other windows |
| `border.active` | `#3A4454` | 1 px window border, focused |
| `border.inactive` | `#2A3340` | 1 px window border, unfocused |
| `button.close` | `#E5534B` | close control |
| `button.minimise` | `#E8B33C` | minimise control |
| `button.inactive` | `#4A5466` | both controls on an unfocused window |
| `term.bg` | `#0B0E13` | terminal background (also ANSI 0) |
| `term.fg` | `#D7DEE8` | terminal default foreground |
| `term.cursor` | `#F2B33D` | block cursor; the glyph under it is drawn in `term.bg` |
| `term.selection` | `#2E4A6B` | selection background (not implemented in 0.3.0) |

The 16 ANSI colours (SGR 30-37 / 90-97 and 40-47 / 100-107; bold selects 8-15 for 0-7):

| | 0 black | 1 red | 2 green | 3 yellow | 4 blue | 5 magenta | 6 cyan | 7 white |
|---|---|---|---|---|---|---|---|---|
| normal | `#0B0E13` | `#E0565B` | `#5FB865` | `#E2B341` | `#4F8FE0` | `#C46BD2` | `#44B9C9` | `#C9D1DB` |
| bright | `#5A6577` | `#F07A7E` | `#84D48A` | `#F2CC6D` | `#7FB0F0` | `#DA95E4` | `#72D3E0` | `#F2F5F9` |

Contrast: `term.fg` on `term.bg` is 15:1, `text` on `bar.bg` 13:1, `text.dim` on
`title.inactive` 5.2:1, every normal ANSI colour on `term.bg` at least 4.6:1 (red 5.1:1,
blue 4.6:1). Nothing is conveyed by colour alone (section 7).

### Spacing, corners, shadow

- Grid: 4 px. Paddings are 8 or 12, control sizes multiples of 4; the top bar and title bar
  are 28 (7 units), the terminal cell 8x16.
- Corner radius 6 on windows and menus, 4 on task chips, circles of diameter 12 for the window
  controls. A rounded rectangle is a plain filled rectangle whose first and last `r` rows are
  shortened at both ends by an inset table, so it costs nothing beyond the rectangle:
  `INSET_R6 = [4, 2, 1, 1, 0, 0]` (row 0 is the outermost), `INSET_R4 = [2, 1, 0, 0]`. The
  pixels cut away show what is behind (wallpaper or shadow); no anti-aliasing. A 1 px border
  follows the same insets (the border pixel of row `i` is at `x0 + inset[i]`).
- Shadow (windows and menus only): four rings outside the border, offset 2 px down (the ring
  at distance `d` spans the rectangle grown by `d` on the left, right and bottom and by
  `d - 2` on the top, never above the window's top edge). Darkening per ring, applied to the
  pixel already there (wallpaper): d = 1: 50%, d = 2 and 3: 25%, d = 4: 12.5%. All three are
  shifts on the packed pixel, no multiply, no table:
  `half(p) = (p >> 1) & 0x007F7F7F`, `quarter(p) = p - ((p >> 2) & 0x003F3F3F)`,
  `eighth(p) = p - ((p >> 3) & 0x001F1F1F)`.

### Fonts and metrics

- One font, **Egg Mono 8x16** (`font-8x16.txt`), for the terminal and every UI label. Cell
  8x16, advance 8, line height 16, baseline row 12, cap height 11 rows (2-12), x-height 8 rows
  (5-12), descender rows 13-15, column 7 always empty except for `_`. Bold is not a second
  face: SGR 1 brightens the colour (ANSI 8-15). Underline is row 14 in the foreground colour.
- The file is the source of truth. A data file in the drawing crate holds the 95 printable
  ASCII glyphs as `[u8; 16]` rows (bit 7 = column 0), in codepoint order, generated from
  `font-8x16.txt` by a host test that fails when they differ. The same file gets the box
  drawing and block elements below, authored by the engineer from these rules, with the same
  stroke: a single line is 1 px, horizontal at row 8 across columns 0-7, vertical at column 3
  across rows 0-15, so adjacent cells join; a double line is rows 7 and 9 / columns 2 and 4.
  Required: `─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼` (U+2500, 2502, 250C, 2510, 2514, 2518, 251C, 2524, 252C,
  2534, 253C), the double set `═ ║ ╔ ╗ ╚ ╝` (U+2550, 2551, 2554, 2557, 255A, 255D), blocks
  `█ ▀ ▄ ░ ▒ ▓` (U+2588, 2580, 2584, 2591, 2592, 2593; the shades are 1-in-4, 2-in-4 and
  3-in-4 checkerboards), `▪ ● ○ ◆` (U+25AA, 25CF, 25CB, 25C6) and `→ ← ↑ ↓` (U+2192, 2190,
  2191, 2193). Any other codepoint draws `?` (U+003F) in 0.3.0; the glyph table is indexed by
  a `u8` glyph id: 0x20-0x7E are themselves, the extras take 0x80 upwards in the order listed,
  and the terminal's UTF-8 decoder maps codepoints to ids.
- 2x (HiDPI) is the same bitmap drawn with every pixel doubled in both directions
  (nearest-neighbour): a 16x32 cell. No second drawing in 0.3.0.

### Pointer

The arrow pointer, 12x19, hotspot (0, 0); `X` = `#000000`, `o` = `#FFFFFF`, `.` transparent.
Uploaded once as a 64x64 ARGB cursor resource (section 8) and scaled 2x at scale 2.

```
X...........
XX..........
XoX.........
XooX........
XoooX.......
XooooX......
XoooooX.....
XooooooX....
XoooooooX...
XooooooooX..
XoooooooooX.
XooooooXXXXX
XoooXooX....
XooX.XooX...
XoX...XooX..
XX....XooX..
X......XooX.
........XX..
............
```

### Motion

None, except: the terminal cursor blinks (500 ms on, 500 ms off, restarted "on" by every
keystroke and every byte written), a window follows the pointer while dragged (no inertia, no
animation), and a menu appears and disappears in one frame. Frame budget: the whole desktop is
redrawn in ≤ 8 ms wall time on the Mac's TCG at 1280x800; a keystroke's echo reaches the
flush in ≤ 1 ms; flushes are paced at 60 Hz (section 8).

## 3. Wallpaper

Procedural, resolution-independent, no backing buffer: every pixel is a function of (x, y),
recomputed on demand within a damage rect.

- Gradient: vertical, `wallpaper.top` at row 0 to `wallpaper.bottom` at row `h - 1`, each
  channel interpolated as `c0 + ((c1 - c0) * y) / (h - 1)` in integers. A per-row table of
  `h` packed pixels is built once per resolution (3.2 KiB at 800 rows) so a wallpaper row is
  a fill with one colour. Banding (about 27 rows per step) is accepted in 0.3.0.
- The egg shape, used here and for the logo and the bar mark. Parameters: height `H`, width
  `W = round(0.72 * H)`, equator at `ye = round(0.58 * H)` rows below the top, centre column
  `cx`. Row `y` in `0..H`: `t = (ye - y) * 256 / ye` if `y < ye`, else
  `t = (y + 1 - ye) * 256 / (H - ye)`; `half = (W / 2) * isqrt(65536 - t * t) / 256`; the row
  spans columns `[cx - half, cx + half)`. `isqrt` is an integer square root (u32, binary
  search or Newton; one call per row). The upper half is taller than the lower, which is what
  makes it an egg rather than an ellipse. A per-row table of `(left, right)` edges is built
  with the gradient table.
- Placement: `H = screen_h`, `cx = round(0.82 * screen_w)`, top at `round(0.25 * screen_h)`
  (so the lower part is off screen), filled `wallpaper.egg`. The yolk: a filled circle of
  radius `r = round(0.09 * screen_h)` centred at `(cx, top + round(0.62 * screen_h))`, colour
  `accent`; row `dy` of the circle spans `[cx - isqrt(r*r - dy*dy), cx + isqrt(r*r - dy*dy))`.
  At 1280x800: egg W 576, cx 1050, top 200, equator at row 664; yolk r 72 at (1050, 696). At
  1024x768: W 553, cx 840, top 192, equator 637; yolk r 69 at (840, 668). Neither overlaps the
  terminal window at its default place.
- Draw order within a wallpaper pixel: gradient, then egg if the column is within the egg
  row's edges, then yolk if within the circle row's edges.

## 4. Top bar

A 28 px bar across the whole width at y = 0, `bar.bg`, with a 1 px `bar.edge` line at
row 27. It is a strip of items, each with a rectangular hit target of the bar's full height:

- Left, the system item: the egg mark (egg shape H 14, W 10, filled `accent`) at x = 12,
  vertically centred, then `EggOS` in `text` at x = 28, baseline-centred (glyph top at row 6).
  Hit target x 0..96. Pressed: `bar.pressed` background while the button is down. It is the
  app menu's place; in 0.3.0 clicking it does nothing else (stub). Super alone is reserved for
  it later and ignored now.
- After it, task chips for minimised windows (section 6), 8 px apart, starting at x = 104.
- Centre, the clock: `HH:MM` (24 h, UTC) in `text`, centred on the bar's midpoint, updated
  once a minute (its 40x16 rect flushed). Until a real-time clock driver sets the wall clock
  (D73: it starts at the epoch; the RTC is a v0.3.0 nice-to-have) it shows uptime instead,
  `up m:ss` in `text.dim`, updated once a second. The rule: realtime seconds < 86400 means
  "unset". Not a button.
- Right, the power button: a 28x28 hit target whose right edge is 8 px from the screen edge,
  showing the 13x13 icon below in `text` (centred; `#` = pixel). Hover `bar.hover`, pressed
  `bar.pressed`. Click opens the power menu; a second click, Esc, or a click anywhere outside
  closes it.

```
......#......
......#......
...#..#..#...
..#...#...#..
.#....#....#.
.#....#....#.
.#.........#.
.#.........#.
..#.......#..
...#.....#...
....#####....
.............
.............
```

- The power menu: 160 px wide, anchored under the bar (top at y = 28 + 4), right edge aligned
  with the button's, `menu.bg` with a 1 px `menu.border`, radius 6, the window shadow. Two
  items, `Power off` and `Restart`, each 32 px tall, text at x + 12, baseline-centred, hover
  `menu.hover`. Up/Down move the hover, Enter activates. `Power off` and `Restart` do what
  `poweroff` and `reboot` do (D106, D108: the kernel's `reboot(2)` paths through the HAL), at
  once, with no confirmation in 0.3.0 (nothing can be lost yet; a confirmation comes with a
  writable file system). This is the GUI front-end of the power capability whose CLI is
  `poweroff`/`reboot`; the MCP tool comes with the MCP server (D22).
- Form factors (D23): the bar's layout is computed at runtime from `(screen_w, screen_h,
  scale)` by one function, never from constants scattered in drawing code. On a phone-sized
  screen (width < 600 at 1x) the system item shows only the egg mark and the chips are
  dropped; a TV runs at scale 2. Nothing else changes in 0.3.0.

## 5. Terminal window

One window in 0.3.0, titled `Terminal`, holding the console terminal (80x24 cells). Geometry at
1x: 1 px border, 28 px title bar, content 656x400 (80·8 + 2·8 by 24·16 + 2·8: an 8 px
padding around the cell grid, in `term.bg`), so the window is 658x430 outside the border, with
radius 6 corners (the bottom corners are cut on `term.bg`). Default position: centred in the
work area (the screen below the bar): x 311, y 199 at 1280x800; x 183, y 183 at 1024x768.

- Title bar: `title.active` / `title.inactive`; the title text centred, `text` when focused,
  `text.dim` otherwise. Controls on the left (the macOS-profile side; the per-profile side
  setting is a follow-up): two filled circles of diameter 12, vertically centred (row 8-19),
  close at x 12..23 (`button.close`) and minimise at x 32..43 (`button.minimise`), both
  `button.inactive` when the window is unfocused. Hit targets 20x28 each (x 8..27, 28..47,
  full bar height). On hover the close circle shows a 1 px `#000000` `x` (5x5, diagonals) and
  the minimise circle a 1 px `#000000` horizontal line (5 px); pressing darkens the circle with
  `quarter()`. Close ends a window; the console terminal cannot end, so in 0.3.0 its close
  control is disabled: drawn in `button.inactive` always, no hover glyph, click ignored
  (documented limit; it works once terminals are userland programs).
- Focus: a button-down anywhere inside the window (border included) focuses it and brings it
  to the front; the terminal has focus at boot. Only the focused window receives keys.
- Dragging: button-down on the title bar outside the controls, then motion, moves the window
  by the pointer's delta, redrawn every frame; button-up ends it. Clamp so the title bar stays
  reachable: y ≥ 28, and at least 48 px of the title bar inside the screen horizontally.
- Size: fixed at 80x24 in 0.3.0 (no resize handles). Double-click on the title bar (two
  button-downs within 400 ms and 4 px) toggles maximised: the window fills the work area with
  square corners and no shadow, the grid becomes `(w - 2 - 16) / 8` by `(h - 28 - 2 - 16) / 16`
  cells (158x45 at 1280x800), the remainder of the padding goes right and bottom, and the
  console's window size (`TIOCGWINSZ`, rows, cols, `ws_xpixel = cols*8`, `ws_ypixel =
  rows*16`) is updated at once; `SIGWINCH` to the foreground group follows when the kernel can
  signal other processes (D105 follow-up). Toggling back restores the previous grid, position
  and the scrollback is kept; cells beyond the new width are dropped, lines beyond the new
  height go to the scrollback.
- Everything about the text area (cursor, scrollback, ANSI, keys) is in `terminal-v0.3.md`.

## 6. Minimise

There is no dock. Minimising hides the window and adds a task chip to the top bar: a 20 px
tall rounded (radius 4) `bar.hover` chip, vertically centred, text `Terminal` in `text` with
8 px padding each side (title cut to 16 glyphs with `…` drawn as three `.`), hover
`bar.pressed`. Clicking the chip restores the window where it was, focused, and removes the
chip. While the only window is minimised, keys go nowhere (the serial console still works).
Chips are laid out left to right after the system item; when they would reach the clock's
rect minus 16 px, the rest are not shown (there is one window in 0.3.0).

## 7. Accessibility baseline and form factors

- Text is never smaller than the 8x16 cell (the font's one size), which at 1280x800 on a
  13-inch screen is about 11 pt; at 2x it is 16x32.
- Contrast: every text/background pair in section 2 is ≥ 4.5:1; focus is shown by three cues
  at once (title bar colour, control colours, blinking cursor), hover by a background change
  and, on the controls, a glyph: nothing is colour-only.
- Hit targets: ≥ 28x28 px for every clickable thing (bar items, controls, menu items are
  160x32, chips are 20 px tall but sit in a 28 px bar strip that counts as the target).
- Keyboard: the terminal is focused at boot, so the whole system is usable with no pointer.
  The power menu has keyboard handling once open; opening it from the keyboard (a Super
  shortcut), moving windows and switching focus with keys are follow-ups and listed as known
  gaps in the limitations doc (item 15).
- Motion: only the cursor blink, which `CSI ? 12 l` stops; no animation anywhere.
- Scale and form factors: the drawing code takes a `scale: u8` (1 or 2) and multiplies every
  token through one `Metrics` value built from it; glyphs are drawn 2x by pixel doubling.
  The scale is 2 when the display's reported height ≥ 1440 or when a boot parameter says so
  (`eggos.scale=2`). Phone (width < 600 at 1x) and TV (scale 2, 10-foot) layouts are not
  implemented in 0.3.0; the only requirement now is that nothing assumes 1280x800, a title bar
  with controls, or a pointer: every input reaches the desktop as an event (`Key`,
  `PointerMove`, `Button`, `Wheel`), so touch and D-pads can be added as event sources.

## 8. Rendering plan

- Framebuffer: one guest-allocated 2D resource of the display's size in
  `VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM` (format id 2), bytes `B, G, R, X` per pixel, which as a
  little-endian `u32` is `0x00RRGGBB`: every hex colour in this document is that `u32`
  with X = 0. Stride = width · 4. The display size comes from
  `VIRTIO_GPU_CMD_GET_DISPLAY_INFO` (scanout 0); xtask asks QEMU for 1280x800
  (`-device virtio-gpu-pci,xres=1280,yres=800`, or `virtio-gpu-device` on the `virt`
  machines); if the device reports 0x0 or nothing, use 1024x768. The buffer (4 MB at
  1280x800) is the one large allocation of the display and is justified under D24 as the
  display itself; it is released if the display goes away.
- Damage: the desktop keeps up to 8 dirty rects per frame; a ninth merges into the one whose
  union grows least. At flush, rects that overlap are merged, then each gets
  `TRANSFER_TO_HOST_2D` and one `RESOURCE_FLUSH` (both with the rect). Flushes are paced:
  at most one every 16 ms (a periodic tick while anything is dirty; the tick also drives the
  blink and the clock), and a flush is issued immediately when the display has been idle for
  ≥ 16 ms, so a keystroke's echo is never delayed by pacing. Nothing is drawn while nothing
  is dirty (idle costs 0).
- Draw order inside a damage rect, each stage clipped to the rect: wallpaper (per-row tables,
  section 3), shadows of visible windows and menus, windows bottom to top (border, title bar,
  controls, title, content), the top bar and its items, the open menu, then the pointer when
  it is software-drawn. The pointer uses the virtio-gpu cursor queue
  (`RESOURCE_CREATE_2D` 64x64 `B8G8R8A8`, `UPDATE_CURSOR` once, `MOVE_CURSOR` per motion
  event) so moving it dirties nothing; if the device has no cursor queue, draw it in software
  last, saving the 12x19 pixels beneath it and restoring them before any other drawing.
- Primitives, all over `&mut [u32]` with a width and a clip rect, in a `no_std` drawing crate
  with no allocation (so host tests and the kernel share every byte of drawing code):
  - `fill_rect`: a row loop of `slice::fill` (one `u32` pattern; the compiler vectorises it).
  - `fill_rounded_rect` and `stroke_rounded_rect`: `fill_rect` rows with the inset tables.
  - `gradient_rows`, `egg_rows`, `circle_rows`: per-row tables, then fills.
  - `shadow`: for each ring, walk the ring's pixels and apply `half`/`quarter`/`eighth`.
  - `glyph`: for each of the 16 row bytes, write 8 pixels: `dst[i] = if bits & (0x80 >> i)
    != 0 { fg } else { bg }` (branchless select), clipped by whole cells; `glyph_2x` writes
    each pixel twice and each row twice. A terminal line of 80 cells is 80 glyph calls; a full
    80x24 redraw is 1920 calls, about 250 K pixel writes.
  - Text is drawn glyph by glyph with advance 8 (16 at 2x); no kerning, no shaping.
- Budget (release build, Mac TCG, 1280x800): full desktop redraw ≤ 8 ms, 80x24 terminal
  redraw ≤ 2 ms, one glyph cell ≤ 2 µs, a flush of the whole screen ≤ 4 ms of guest time.
  `-icount` benchmarks added with the code (D27 rules): `gfx_fill_full`, `gfx_wallpaper_full`,
  `gfx_glyph_cell`, `gfx_terminal_line`, `gfx_window_chrome`, `term_feed_line` (parser plus
  cell updates for one 80-column line with SGR), `term_scroll_line`.
- Tests: the drawing crate's host tests render the boot screen and the desktop into a
  `Vec<u32>` and check (a) the FNV-1a 64 hash of named rects, (b) exact pixels at chosen
  points (a corner's inset, a shadow ring, a glyph's row), (c) that every token colour in this
  document appears where it should. The QEMU side (section 9) checks the same hashes from
  inside the guest, so both agree on the exact bytes.

## 9. Implementation order and acceptance criteria

Each item is one PR with its own tests and benchmarks, mergeable alone; the crate split is the
engineer's (suggested: `eggos-gfx` for the pure drawing, terminal and desktop logic; the
virtio transport, virtio-gpu and virtio-input in `eggos-drivers` behind the driver model). In
0.3.0 the terminal and the desktop run in the kernel: the console's line discipline is in the
kernel (D100), programs reach it through descriptors 0-2, and there is no shared memory or
IPC to run a compositor in userland yet. Moving them out is M11's work. Record the placement
and the crate names in `docs/decisions.md`.

**Item 11, virtio-gpu 2D (plus the virtio transport it needs).** Virtqueues, feature
negotiation and device discovery over MMIO (`virt` machines) and modern PCI (`q35`); the gpu
driver: `GET_DISPLAY_INFO`, `RESOURCE_CREATE_2D`, `RESOURCE_ATTACH_BACKING`, `SET_SCANOUT`,
`TRANSFER_TO_HOST_2D`, `RESOURCE_FLUSH`, the cursor queue; the drawing crate with the
primitives, the egg, the font data and the boot screen. Accepted when: all three architectures
show the boot screen in a QEMU window; the headless QEMU test (`-display none` keeps the
device) prints `fb: boot <hash>` for the logo rect and the whole screen and xtask matches the
constants the host test produced; `boot: display WxH` is on the serial console within 100 ms
of kernel entry; the benchmarks `gfx_fill_full`, `gfx_glyph_cell` and the flush exist with
baseline lines; the kernel without a display behaves exactly as before (same serial output,
same boot-to-prompt time within noise).

**Item 12, virtio-input.** Keyboard and pointer devices (`virtio-keyboard`, `virtio-tablet`,
`virtio-mouse`), `EV_KEY`, `EV_REL`, `EV_ABS`, `EV_SYN`; the US keymap and the byte encoding of
`terminal-v0.3.md`, feeding `line_discipline::receive` exactly as the UART does; an input
event queue (fixed ring of 256 events, dropped with a counter when full) for the desktop with
`Key`, `PointerMove` (absolute screen coordinates, tablet values scaled from 0..32767, mouse
deltas accumulated and clamped), `Button`, `Wheel`; key repeat (400 ms, then every 40 ms) from
the timer. Accepted when: xtask's QEMU test types a shell line through QMP `input-send-event`
(xtask opens a QMP TCP socket on a free localhost port; this also gives `screendump`) and
sees the same echo and output as through the serial path; `boot: input ...` is printed; a
host test checks every key of the map against the byte table; no new cost on the UART path.

**Item 13, graphical terminal.** The terminal emulator of `terminal-v0.3.md` in the drawing
crate: cell grid, scrollback, the ANSI subset, the cursor, UTF-8 to glyph ids; the console
writes every byte to the UART and to the terminal; the terminal draws into the framebuffer
with damage tracking. Accepted when: the boot-to-prompt QEMU test additionally checks hashes
of the prompt line's cells and of a line printed with SGR colours on all three architectures;
the host tests cover every sequence in the table (cursor positions, erase, scroll region,
alternate screen, SGR, DSR reply) and the keymap round trip; `term_feed_line` and
`term_scroll_line` have baselines; an 80x24 redraw is under budget.

**Item 14, minimal desktop and the first-frame test.** Wallpaper, top bar with its three
items and the power menu, the terminal window with focus, drag, minimise, chips, maximise,
pointer; the boot screen replaced by the desktop at the first prompt. Accepted when: on all
three architectures the QEMU test boots with `eggos.ui=test` (clock fixed to `up 0:00`,
blink off, pointer at (0, 0)) and matches the hashes of the bar's left 120x28, the clock's
rect, the power button's rect, the window's title bar, the first terminal row, a 64x48
wallpaper rect at (1200, 740) and the yolk's bounding box; through QMP it clicks the minimise
control and checks the chip's rect and the restored window; it drags the window by 100 px and
checks the title bar's new rect; `cargo xtask screenshot --arch <a>` saves a PPM from
`screendump` for a human to look at; the full-redraw benchmark is under budget; base image
size, idle memory and boot-to-prompt are reported against D108's numbers, the display buffer
and this font being the only growth.

Not in 0.3.0, listed so nobody waits for them: selection and copy/paste, more than one
window, window resize by edges, the per-profile control side, a dock, app launcher, settings,
touch, a userland compositor, anti-aliased text, a second font size.
