# EggOS v0.3.0 multi-window desktop and shell: design brief

Design by Claude Fable (D16) for the window server and window management (items 30 and 31,
**build now**) and for the shell, the installer, the Files app and the Settings app (items 35,
36, 37 and 34, **later**). It extends `desktop-v0.3.md` (D109): every token, metric, primitive
and rule there stays, and this brief only adds to them. The font is `font-8x16.txt` (Egg Mono
8x16, 2x by pixel doubling); the terminal is `terminal-v0.3.md`. Everything is drawn by our
own Rust code on the software framebuffer: no image files, no second font, no anti-aliasing,
no alpha except the shadow's three shifts. Every number is in pixels at scale 1x and doubles at
2x. "The work area" is the screen minus the panels (section 6); until item 35 exists it is the
screen below the 28 px top bar, as today.

## 0. Scope and schedule

| Item | Builds | Sections | When |
|------|--------|----------|------|
| 30 | the window server: `/dev/window`, shared pixel buffers, commits, events, the console terminal as its first client | 4, 13 | now |
| 31 | window management: roles, stacking, focus, chrome with three controls, move, edge resize, maximise, cascade, the switcher, the window menu, keyboard operation, pointer shapes | 1, 2, 3, 5, 12, 13 | now |
| 35 | the shell: panels per profile (taskbar, menu bar, dock), launcher, desktop icons, the icon set | 6, 7, 11 | later |
| 36 | install and uninstall of a `.exe`, `.app` or ELF from the host share | 9 | later |
| 37 | the Files app | 8 | later |
| 34 | the Settings app with the profile picker | 10, 11 | later |

Items 30 and 31 are one two-week schedule: 30 first (the terminal moves behind the contract
without a visible change), then 31. Nothing in 30-31 depends on 34-37; the profile's control
side and shortcuts (section 11) are a value the server reads, set by a boot parameter until
Settings exists.

## 1. Tokens added

Colours (sRGB; as a framebuffer `u32`, `0x00RRGGBB`), on top of `desktop-v0.3.md` section 2:

| Token | Value | Used for |
|-------|-------|----------|
| `button.maximise` | `#3DBE5C` | maximise control (green, the third circle) |
| `window.bg` | `#12161D` | a window's content before its client's first commit, and during a resize |
| `selection` | `#2E4A6B` | selected row or icon (same value as `term.selection`) |
| `focus.ring` | `#F2B33D` | keyboard focus outline (the accent) |
| `field.bg` | `#0B0E13` | text fields (search, path, rename) |
| `button.bg` | `#2A3440` | push buttons |
| `button.border` | `#3A4454` | 1 px border of buttons and fields |
| `button.primary` | `#F2B33D` | the default button, text in `#0B0E13` |
| `progress.track` | `#26303F` | progress bar track |
| `dock.bg` | `#1C2430` | dock and launcher surface (same as `menu.bg`) |
| `icon.tile` | `#26303F` | generic app icon tile |
| `icon.tile.border` | `#3A4454` | its 1 px border |
| `icon.prompt` | `#5FB865` | the terminal icon's `>` |
| `icon.folder` | `#E2B341` | folder front |
| `icon.folder.back` | `#B88A24` | folder back and tab |
| `icon.gear` | `#8A94A6` | settings gear |
| `icon.gear.hole` | `#1F2630` | its hole |
| `icon.windows` | `#4F8FE0` | Windows app badge |
| `icon.mac` | `#C9D1DB` | macOS app badge |
| `icon.linux` | `#E2B341` | Linux app badge |
| `icon.dark` | `#0B0E13` | glyphs on light badges |

Contrast of every new text pair: `text` on `button.bg` 10.5:1, on `icon.tile` 11.0:1, on
`selection` 7.6:1, on `dock.bg` 12.9:1; `icon.dark` on `button.primary` 10.4:1, on `icon.mac`
12.6:1, on `icon.linux` 9.9:1; `text` on `icon.windows` 4.6:1. `window.bg` carries no text.
Focus is never shown by colour alone: the ring is 2 px wide, and a selected row also moves the
list's `>` mark (section 8).

Metrics added: push button 28 tall, text with 16 px padding each side, at least 96 wide,
radius 4; text field 28 tall, text at x + 8, radius 4; list row 24 (dense lists) or 40 (icon
rows); body text line pitch 20 (16 px glyph + 4); dialog padding 16; the focus ring is a 2 px
rounded (`INSET_R4`) outline drawn 2 px inside the item's edge.

## 2. Pointer shapes

Seven shapes, each uploaded once as a 64x64 ARGB cursor resource (or software-drawn,
`desktop-v0.3.md` section 8). `X` = `#000000`, `o` = `#FFFFFF`, `.` transparent. The server
shows its own shape over chrome, panels and popups (arrow; a resize shape over a resize zone,
section 3.5) and the client's chosen shape (section 4.3) over a window's content. Arrow is the
one in `desktop-v0.3.md` (12x19, hotspot (0, 0)).

Resize vertical `ResizeNS`, 9x19, hotspot (4, 9); `ResizeEW` is its transpose, 19x9, hotspot
(9, 4), written out after it:

```
....X....          ....X.........X....
...XoX...          ...XX.........XX...
..XoooX..          ..XoX.........XoX..
.XoooooX.          .XooXXXXXXXXXXXooX.
XXXXoXXXX          XoooooooooooooooooX
...XoX...          .XooXXXXXXXXXXXooX.
...XoX...          ..XoX.........XoX..
...XoX...          ...XX.........XX...
...XoX...          ....X.........X....
...XoX...
...XoX...
...XoX...
...XoX...
...XoX...
XXXXoXXXX
.XoooooX.
..XoooX..
...XoX...
....X....
```

Resize diagonal `ResizeNWSE`, 16x16, hotspot (7, 7); `ResizeNESW` is each row reversed,
hotspot (8, 7):

```
XXXXXXXX........
XoooooX.........
XooooX..........
XoooX...........
XooXoX..........
XoX.XoX.........
XX...XoX........
X.....XoX.......
.......XoX.....X
........XoX...XX
.........XoX.XoX
..........XoXooX
...........XoooX
..........XooooX
.........XoooooX
........XXXXXXXX
```

Text `IBeam`, 9x16, hotspot (4, 8), for text fields and later editors:

```
XXXXXXXXX
XoooXoooX
XXXXoXXXX
...XoX...
...XoX...
...XoX...
...XoX...
...XoX...
...XoX...
...XoX...
...XoX...
...XoX...
...XoX...
XXXXoXXXX
XoooXoooX
XXXXXXXXX
```

`Move` (while a window is dragged) is the arrow; `Busy` and `Hand` are not in 0.3.0 (a client
asking for them gets the arrow).

## 3. Windows (item 31)

### 3.1 Roles

| Role | Who | Chrome | Layer | Focusable | In the window list |
|------|-----|--------|-------|-----------|--------------------|
| `Normal` | apps, the terminal | border, title bar, three controls, shadow, radius 6 | 1 | yes | yes |
| `Dialog` | an app's prompt; has a parent `Normal` window | border, title bar, close control only, shadow, radius 6 | 1, always directly above its parent | yes; while open its parent receives no input | no (its parent's entry) |
| `Panel` | the shell's bars and dock (the server itself in 0.3.0) | none; reserves screen space | 2 | no (keys reach it only through the shell's shortcuts) | no |
| `Popup` | menus, the launcher, the switcher, the window menu, context menus | border 1 px `menu.border`, radius 6, shadow, no title bar | 3, at most one at a time | takes keys while open | no |
| `Background` | desktop icons (item 35) | none; fills the work area | 0 | yes, when nothing else is | no |

Opening a popup closes the open one; a button-down outside a popup closes it and the
button-down is then delivered normally (so a click on another window both closes the menu and
focuses the window; a click on the control that opened the popup only closes it). Esc closes a
popup. A `Dialog` whose parent closes closes with it. In 0.3.0 only the server creates panels,
popups and the background; clients may create `Normal` and `Dialog` windows (a client asking
for another role gets `EPERM`).

### 3.2 Stacking and focus

- Z order is one list per layer, top first. A button-down inside a window (its border, title
  bar or content; the shadow is not the window) raises it to the top of its layer, its dialogs
  with it, and focuses it. A new `Normal` or `Dialog` window goes to the top and takes focus
  when it is first shown (its first commit). There is no focus stealing prevention in 0.3.0.
- Exactly one window has keyboard focus, or none. When the focused window is minimised, closed
  or hidden, focus goes to the top-most shown `Normal` or `Dialog` window; with none left, to
  the background (desktop icons, item 35) or nowhere. The server's shortcuts (section 5) work
  whatever has focus.
- Focused chrome: `title.active`, `border.active`, the controls in their colours, the title in
  `text`. Unfocused: `title.inactive`, `border.inactive`, `button.inactive`, `text.dim`. A
  dialog's parent draws as unfocused while the dialog is focused, and its controls are
  `button.inactive` with clicks ignored, as the console terminal's close control is today.
- Keys go to the focused window only (as events, section 4.4), after the server has taken its
  shortcuts. Pointer events go to the window under the pointer, except during a drag or resize
  started on chrome (the server keeps them) and during a button press started in a content
  area (that window keeps every pointer event until the release, an implicit grab).

### 3.3 Chrome

The window of `desktop-v0.3.md` section 5, generalised: 1 px border, 28 px title bar, content
of any size, radius 6 (the bottom corners are cut on the client's own pixels, so a client
sees its corner pixels covered; a maximised window has square corners and no shadow, a dialog
keeps both). The shadow is the four rings. Metrics (x from the window's left border, outside
size `w` by `h`, content `w - 2` by `h - 30`):

- Controls, three filled circles of diameter 12, rows 8-19, hit targets 20x28 side by side
  from the window's edge: with controls on the **left** (macOS profile), close at x 12..23,
  minimise 32..43, maximise 52..63, hit targets x 8..27, 28..47, 48..67. With controls on the
  **right** (Windows and Linux profiles), the same three mirrored: close at x `w-24..w-13`,
  maximise `w-44..w-33`, minimise `w-64..w-53`, hit targets `w-28..w-9`, `w-48..w-29`,
  `w-68..w-49` (so the order from the edge is close, maximise, minimise on both sides, the
  habit each side's users have). Hover glyphs, 5x5, 1 px `#000000` (`X`) centred in the
  12x12 circle at its rows 4-8 and columns 4-8 (title-bar rows 12-16): close, minimise,
  maximise, and restore, which the maximise circle shows while the window is maximised:

  ```
  X...X   .....   XXXXX   .XXXX
  .X.X.   .....   X...X   .X..X
  ..X..   XXXXX   X...X   XXXXX
  .X.X.   .....   X...X   X..XX
  X...X   .....   XXXXX   XXXX.
  ```

  Pressing darkens the circle with `quarter()`. A dialog has only the close circle at the
  close position; a window that declared itself fixed-size (section 4.2) draws its maximise
  circle in `button.inactive` and ignores it.
- Title: the client's title (UTF-8, at most 64 bytes, drawn through the font's glyph ids),
  centred in the title bar between the controls' side and the other side's 8 px margin; if
  longer than the room, cut and ended with `...` (three `.` glyphs). The title bar carries
  nothing else in 0.3.0 (no icon, no menus; menus are a client's own rows inside its content).
- Minimum outside size 160x92 (content 158x62); a client's declared minimum (section 4.2) adds
  to it. Maximum: the work area. Default size: the client's requested content size, clamped
  to the work area minus 32 in each direction when it is not maximised.

### 3.4 Placement: cascade

A window with no requested position: if it is the only shown `Normal` window, centred in the
work area (the terminal's place today). Otherwise at the current top `Normal` window's
position plus (32, 32); if that puts its right or bottom edge outside the work area, at the
work area's origin plus (32, 32); if it still does not fit (the window is as large as the
work area), at the origin. A requested position is honoured, clamped so the whole window is
inside the work area. A dialog is centred on its parent, then clamped into the work area.

### 3.5 Move and resize

- Move: button-down on the title bar outside the controls, then motion, moves the window by
  the pointer's delta, redrawn each frame (no animation). Clamp: the top edge never above the
  work area's top, and at least 48 px of the title bar's width and the full title bar height
  inside the work area horizontally and vertically (so a window may hang off the left, right
  or bottom). Button-up ends it. Moving a maximised window by more than 8 px restores it
  first, placed so the pointer keeps the same fraction of the title bar's width under it.
  Dialogs move; their parent does not follow.
- Resize zones, on `Normal` windows that are not maximised and have not declared themselves
  fixed-size: a ring 6 px wide around the window, 2 px outside the border (inside the
  shadow) and 4 px inside it, except across the title bar's top edge where the ring is 2 px
  outside and 2 px inside (so the title bar keeps its drag). Corners are 16x16 squares at
  each corner of that ring (both edges). The pointer shows `ResizeNS` on the top and bottom
  edges, `ResizeEW` on the left and right, `ResizeNWSE` on the top-left and bottom-right
  corners, `ResizeNESW` on the other two. Button-down in a zone starts a live resize: the
  opposite edge stays put, the window's outside size follows the pointer, clamped to the
  minimum and the work area; the server sends the client `Resize` (section 4.4) with the new
  content size, coalesced so only the latest size is pending, and draws the content in
  `window.bg` wherever the client has not yet committed a frame at the current size (it keeps
  showing the client's last buffer in its old size, clipped, until then). Button-up ends it.
  Keys: the window menu's `Size` (section 5).
- Maximise/restore: the maximise control, double-click on the title bar (two button-downs
  within 400 ms and 4 px), `Super+Up`. Maximised: fills the work area, square corners, no
  shadow, border kept, the controls as usual. Restore (same control, double-click,
  `Super+Down`) returns to the geometry from before, which the server remembers per window.
  Snap: `Super+Left`/`Super+Right` set the window to the work area's left or right half
  (square corners, shadow kept; it is a plain geometry change the user can resize out of).
- Minimise: the control, `Super+Down` on a window that is not maximised, or `Super+D` for all
  (show desktop; `Super+D` again restores them). A minimised window is unshown and reachable
  from the window list, the switcher and the launcher; restoring puts it back at its geometry,
  focused.

### 3.6 Close

The close control, `Alt+F4`, `Cmd+W` (macOS profile; Super is Cmd on PC keyboards) and
`Ctrl+W` (Windows and Linux profiles, unless the focused window declared that it takes every
Ctrl key, as the terminal does) send the focused window a `CloseRequest` and close nothing
themselves: the client destroys the window (or shows a dialog). A client that does not read
its events for 2 s after a `CloseRequest` gets its window destroyed by the server with the
title bar showing `(not responding)` after the title during those 2 s; its process is not
killed. The console terminal declines close in 0.3.0 (its control is `button.inactive`, as
today) until terminals are userland programs.

### 3.7 Damage and redraw

The server composes from scratch per damage rect, as today, in this order, each clipped to the
rect: wallpaper (per-row tables), background window, then for each layer 1 window bottom to
top its shadow (four rings over whatever is already drawn), border, title bar, controls,
title, content (copied from the client's buffer; `window.bg` where no committed frame
covers), then panels, then the popup with its shadow, then the pointer when software-drawn.
A rect wholly inside the content of the top-most window containing it skips every layer below
(the common case: a client's commit). What dirties what: a commit dirties its rects
intersected with the window's visible part; a move dirties the old and new rectangles with
shadow; a resize both geometries; focus changes dirty both windows' chrome; the switcher and
menus their rects; minimise the window's rect and the list entry's. Budgets (release, Mac TCG,
1280x800): composing a full screen with four overlapping 658x430 windows ≤ 10 ms; a commit of
one 656x400 content from the client's call to the flush ≤ 2 ms of guest time; the flush pacing
of `desktop-v0.3.md` section 8 is unchanged (60 Hz, immediate when idle).

## 4. The window server contract (item 30)

`/dev/window` is a character device. Each `open` is one client connection owning its windows;
closing the descriptor destroys them. Requests are `ioctl`s on the descriptor (numbers and
struct layouts are the kernel engineer's; the fields below are the contract), events are read
from the descriptor as fixed-size records, and `poll` reports `POLLIN` when an event is
pending. The console terminal is the server's first client, in-kernel in 0.3.0, calling the
same functions through a Rust interface instead of the descriptor, so the contract is
exercised from day one; userland programs reach it over the descriptor. In 0.3.0 one client
may have 16 windows and the server 64 windows in all, in fixed tables: no allocation on any
per-event or per-frame path; the only allocation per window is its pixel buffer.

### 4.1 Pixels

A window's content is one shared buffer: `u32` pixels `0x00RRGGBB` (the framebuffer's
format), stride `width * 4`, `height` rows, allocated by the server at the content size and
mapped into the client with `mmap` on the descriptor at the offset `Create` or `SetSize`
returns. The client draws into it, then `Commit`s: the server copies the committed rects
(clipped to what is visible) into the framebuffer **during the call**, so when `Commit`
returns the client may draw again with no tearing and no second buffer. A buffer is
reallocated only by `SetSize` (a new offset is returned; the old mapping stays valid until
the client unmaps it, its content is not carried over). Cost: `width * height * 4` bytes per
window (the terminal's 656x400 is 1 MB), released on destroy; this is the lightness cost of
more than one window and is reported in the PR as D24 asks.

### 4.2 Requests

| Request | Fields | Behaviour |
|---------|--------|-----------|
| `Create` | role (`Normal`, `Dialog`), parent window id (dialogs), content width and height, flags | Returns the window id and the buffer offset. Nothing is shown until the first `Commit`. Flags: `FIXED_SIZE` (no resize zones, no maximise), `TAKES_CTRL_KEYS` (the shell's Ctrl shortcuts are passed through; `Alt+F4` and the Super shortcuts never are), `NO_DECORATIONS` is not offered in 0.3.0. |
| `SetTitle` | id, UTF-8 bytes (≤ 64) | Redraws the title bar and the window list entry. |
| `SetSize` | id, content width, height | The client's own resize (clamped to the minimum and the work area); returns the new buffer offset; the window keeps its top-left. Also how a client acknowledges a server `Resize` event: it calls `SetSize` with the size the event gave, draws, commits. |
| `SetMinimumSize` | id, width, height | Added to the chrome's minimum. |
| `SetPosition` | id, x, y (work-area coordinates of the window's outside top-left) | A hint, clamped as in 3.4; most clients never call it. |
| `SetState` | id, `Normal` / `Minimised` / `Maximised` | What the controls do, from the program. |
| `SetPointer` | id, shape (`Arrow`, `IBeam`, `ResizeNS`, `ResizeEW`, `ResizeNWSE`, `ResizeNESW`) | The pointer shown over this window's content. |
| `Commit` | id, up to 8 rects in content coordinates (none = the whole content) | Shows the window if it is the first commit; copies and flushes as in 4.1. Never returns `EAGAIN`: the copy is synchronous. |
| `Destroy` | id | Unshows and frees the window and its buffer; its dialogs with it. |
| `GetWorkArea` | | The work area's size (what a maximised window gets) and the scale (1 or 2), so a client can size itself. |

Errors: a bad id `EINVAL`, a role the client may not create `EPERM`, the table full `ENOSPC`,
a buffer that cannot be allocated `ENOMEM` (the window is not created).

### 4.3 What the user sees from a client's calls

Create a 400x300 window and commit: a framed window appears on top, focused, cascaded, with
the title (empty until `SetTitle`: the title bar then shows nothing). Draw and commit a rect:
only that rect changes on screen, within one flush. The user drags, resizes, minimises,
maximises, switches and closes with the shell's chrome and keys; the client only learns
about it through events and only has to react to `Resize` (draw at the new size) and
`CloseRequest` (destroy, or ask). A client that never reads events still works for display
(the server drops its events when the queue is full) but closes only through the 2 s rule.

### 4.4 Events

Records of 32 bytes: `kind: u16`, `window: u16`, `time_ms: u32` (the monotonic clock), then
`a, b: i32` and `c, d: u32`, the rest zero. Content coordinates have (0, 0) at the content's
top-left; the chrome sends nothing. The queue holds 256 records per client; when it is full,
`PointerMove` records are merged first (the newest replaces the last one), then the oldest
record is dropped and the next record read is `Overflow` with the count dropped.

| Kind | a | b | c | d | When |
|------|---|---|---|---|------|
| `PointerEnter` | x | y | | | the pointer enters the content (not the chrome) |
| `PointerMove` | x | y | buttons held (bit 0 left, 1 right, 2 middle) | | motion over the content, or anywhere during an implicit grab (coordinates may then be negative or past the size) |
| `PointerLeave` | | | | | the pointer leaves the content, or a popup opens over it |
| `Button` | x | y | button code (`BTN_LEFT` 0x110, `BTN_RIGHT`, `BTN_MIDDLE`) | 1 pressed / 0 released | over the content, and the release of a grab |
| `Wheel` | x | y | | notches as `i32` (+1 up) | over the content |
| `Key` | Linux key code | 1 pressed / 0 released (2 repeat) | modifiers: bit 0 Shift, 1 Ctrl, 2 Alt, 3 Super, 4 Caps Lock | the key's Unicode codepoint under the US keymap with the modifiers applied, or 0 | the window is focused and the server did not take the key; repeat 400 ms then 40 ms, as the terminal's |
| `FocusIn` / `FocusOut` | | | | | focus changes (a `FocusOut` arrives before a popup takes keys and `FocusIn` after it closes) |
| `CloseRequest` | | | | | section 3.6 |
| `Resize` | content width | content height | | | the server changed the size (edge resize, maximise, restore, snap, work area change); the client answers with `SetSize` + `Commit` |
| `StateChanged` | state (`Normal` 0, `Minimised` 1, `Maximised` 2) | | | | after the change; a minimised client may stop drawing |
| `Scale` | scale | | | | the scale changed (not in 0.3.0; reserved) |
| `Overflow` | dropped count | | | | the queue overflowed |

The terminal's bytes (`terminal-v0.3.md`) are made from `Key` events by the terminal itself,
so the keymap lives in one place: the server translates codes to codepoints for every client,
and the terminal turns codepoints and codes into bytes as the table there says.

### 4.5 CLI and MCP front-ends (D22)

The window capability's CLI is `wm`, one multi-call binary: `wm list` (id, title, state,
geometry, client pid), `wm focus <id>`, `wm move <id> <x> <y>`, `wm size <id> <w> <h>`,
`wm minimise|maximise|restore|close <id>`, `wm screenshot <file.png>` (what `cargo xtask
screenshot` reads). It talks to the server through a second device interface, `/dev/window`
opened with a `Manage` request that only root may make in 0.3.0 (the permission model comes
with the MCP server). The MCP tools mirror the subcommands (`wm_list`, `wm_focus`, ...) when
the MCP server lands; item 31 ships the CLI and marks the tools as a listed gap, as D115 did.

## 5. Keyboard (item 31)

The server takes these before any client sees the key. `Primary` is Ctrl in the Windows and
Linux profiles and Cmd (the Super key code, 125/126) in the macOS profile; the other
profile's combination also works wherever the table says "both".

| Action | Windows profile | macOS profile | Linux profile | Also |
|--------|-----------------|---------------|---------------|------|
| launcher | Super (press and release alone) | Cmd+Space, Super | Super | Super everywhere |
| switch window (switcher, 5.1) | Alt+Tab, Shift back | Cmd+Tab, Shift back | Alt+Tab, Shift back | Alt+Tab everywhere; Alt+Esc cycles without the popup |
| close window | Alt+F4, Ctrl+W | Cmd+W | Alt+F4, Ctrl+W | Alt+F4 everywhere; Ctrl+W not when the window takes Ctrl keys |
| window menu (5.2) | Alt+Space | Alt+Space | Alt+Space | |
| maximise / restore | Super+Up / Super+Down | Super+Up / Super+Down | Super+Up / Super+Down | |
| minimise | Super+Down (not maximised) | Cmd+M, Super+Down | Super+Down, Super+H | |
| snap halves | Super+Left / Super+Right | Super+Left / Super+Right | Super+Left / Super+Right | |
| show desktop | Super+D | Super+D, F11 | Super+D | toggles |
| power menu | Super+X | Super+X | Super+X | opens the top bar's power menu with keyboard handling (closes the gap D115 listed) |
| close popup | Esc | Esc | Esc | |

### 5.1 The switcher

A popup, centred in the work area, 360 wide, `dock.bg` surface with the popup chrome: one
row of 32 px per shown or minimised `Normal` window (dialogs count under their parent), most
recently focused first, at most 12 rows (more windows: the list scrolls so the selected row
stays visible), 8 px padding top and bottom; each row shows the title (cut with `...` at 40
glyphs) at x + 48 and, from item 35, the app's 32x32 icon at x + 8 (before item 35, 32 px of
empty room so the layout does not change), a minimised window's title in `text.dim`. The
selected row is `bar.pressed` with the focus ring. Holding Alt (or Cmd), each Tab advances,
Shift+Tab goes back; releasing Alt focuses and raises the selected window (restoring it if
minimised) and closes the popup; Esc closes without changing focus. With one window the popup
still shows (one row) so the key has visible effect. Alt+Esc cycles focus directly in the
same order with no popup.

### 5.2 The window menu

Alt+Space on a focused window opens a popup anchored under the title bar's control side
(top at the title bar's bottom + 4, left edge at the window's x + 8, or right-aligned to the
window's right - 8 when the controls are on the right), 160 wide, items 32 tall as the power
menu: `Restore`, `Move`, `Size`, `Minimise`, `Maximise`, `Close`; items that do not apply
(Restore on a normal window, Maximise on a fixed-size one) are drawn in `text.dim` and
skipped by Up/Down. `Move` and `Size` enter keyboard mode: arrows move the window, or grow
and shrink its bottom-right corner, by 8 px per press (repeat applies), the pointer shape is
`Move` or `ResizeNWSE`, Enter confirms, Esc restores the geometry from before. Right-click on
the title bar opens the same menu at the pointer.

### 5.3 Keyboard-only operation

Every action in this brief has a key: launch (Super, type, Enter), switch (Alt+Tab), move
and size (Alt+Space), maximise and snap (Super+arrows), close (Alt+F4), power (Super+X), and
inside the apps Tab moves the focus ring between fields, lists and buttons, arrows move in
lists, Enter activates, Space toggles, Esc backs out. A pointer is never required.

## 6. The shell: panels, launcher, desktop icons (item 35)

The shell is one component (the server's in 0.3.0) laid out per profile by one function of
`(screen_w, screen_h, scale, profile)`. The profile picks which panels exist and where; the
parts are the same.

### 6.1 Panels

- **Top bar** (macOS and Linux profiles): the 28 px bar of `desktop-v0.3.md` section 4, with
  these changes: the system item opens the launcher on click (no longer a stub); in the Linux
  profile its label is `Activities` instead of `EggOS` (hit target x 0..96 either way); the
  task chips of minimised windows stay in the Linux profile (its window list) and are dropped
  in the macOS profile (the dock lists windows); the clock and the power button stay.
- **Taskbar** (Windows profile): a 40 px bar across the bottom, `bar.bg`, with a 1 px
  `bar.edge` line at its top row. Items are 40x40 hit targets, centred as a group on the bar's
  midpoint, 4 px apart: first the Start button (the egg mark, H 20, W 14, `accent`, centred),
  then one item per shown or minimised `Normal` window in creation order: its app's 32x32
  icon centred, a 2 px `text.dim` line across the bottom 2 rows (x + 8..x + 31) for a shown
  window, `accent` for the focused one, nothing for a minimised one; hover `bar.hover`,
  pressed `bar.pressed`. Click focuses and raises the window, or minimises it if it is the
  focused one; a minimised one is restored. Hovering 600 ms shows a tooltip popup with the
  title (menu chrome, 24 tall, text with 8 px padding, above the item). Right, the clock
  (`HH:MM` or `up ...`, as the top bar's) with its right edge 56 px from the screen's right
  edge, and the power button, 40x40, right edge 8 px from the screen edge, opening the power
  menu above it (bottom at the bar's top - 4). The work area is the screen above the taskbar.
- **Dock** (macOS profile): a floating strip 56 px tall at the bottom, 8 px above the screen's
  bottom edge, centred, `dock.bg` with 1 px `menu.border`, radius 6, the window shadow, 8 px
  padding left and right: 48x48 icons 8 px apart with their top at row 2, first the pinned
  apps (Terminal, Files, Settings, in that order), then a 1 px `menu.border` vertical
  separator with 8 px either side, then one icon per app that has a window and is not
  pinned, then one per minimised window (its app's icon; several of one app: several icons,
  each with the window's title as its tooltip). A running app's icon has a 4x4 `text` square
  centred under it at rows 52-55 (2 + 48 + 2 + 4 = 56). Click: launch, or focus the
  app's most recently focused window (a second click cycles to the next), or restore the
  minimised one. The work area is the screen between the top bar and the dock's top - 8.
- Phone width (< 600 at 1x): one bottom bar of 48 px with the launcher button centred, no
  dock, every `Normal` window maximised and chromeless (title and close move into a 28 px
  strip the server draws at the top of the content area); not implemented in 0.3.0, but the
  layout function and the roles are what make it possible (section 12).

### 6.2 The launcher

Opened by the keys of section 5, the Start button, the dock's egg (the system item) or the
`Activities` item. A popup 480 wide, 400 tall: in the Windows profile centred horizontally
with its bottom 8 px above the taskbar; in the macOS and Linux profiles centred in the work
area. Surface `dock.bg`, popup chrome. Inside, 12 px padding: a search field (28 tall, full
width, `field.bg`, `button.border`, the `IBeam` pointer, the text cursor a 1 px `text` line
blinking 500 ms like the terminal's) with the placeholder `Search apps` in `text.dim`; below
it, 8 px down, rows of 40 px: a 32x32 icon at x + 8, the name at x + 48 in `text`, and for a
foreign app its source at the right edge in `text.dim` (`Windows`, `macOS`, `Linux`); the
selected row `selection` with the focus ring. The list starts with every app alphabetically
(the built-in three, then installed ones), and each typed character filters it: a row stays
when the typed text is a case-insensitive substring of the app's name or of one of its
synonyms. Synonyms are part of the catalogue: Terminal has `shell`, `console`, `cmd`,
`iTerm`, `bash`; Files has `Explorer`, `Finder`, `Dolphin`, `Nautilus`, `folders`; Settings
has `Control Panel`, `System Settings`, `Preferences`, `options`; an installed app has its
file name. Up/Down move, Enter or click launches and closes, Esc closes, Backspace edits; a
right-click or Shift+F10 on an installed app opens a context menu (`Open`, `Uninstall...`,
section 9). Below the list's last row, when it is not full, a line in `text.dim`:
`Minimised:` followed by the titles of minimised windows as chips (section 6 of
`desktop-v0.3.md`), each restoring its window. In the Linux profile the launcher is also the
overview: its left 64 px column holds the dock's pinned icons vertically (a dash) and the
rest is the same search and list.

Launching: a native EggOS app (Terminal, Files, Settings, in-kernel or `/bin/<name>`) starts
as a process that opens `/dev/window`; in 0.3.0 Terminal is the console terminal, so
launching it focuses that window (a second terminal waits for the userland terminal and is a
listed gap). A foreign console program (every installed app in 0.3.0) runs in the Terminal
window as if its name had been typed at the prompt, and the window is focused; its window
list entry stays the terminal's.

### 6.3 Desktop icons

The `Background` window draws, on the wallpaper, a grid of 96x96 cells, 16 px from the work
area's edges: in the Windows and Linux profiles the first column at the left, filled top to
bottom; in the macOS profile the first column at the right. Each cell: the 48x48 icon at
(24, 8), the label centred below with its top at 64, up to 12 glyphs (cut with `...`), `text`
on a `bar.bg` pill (radius 4, 4 px padding, 20 tall) so it reads on the yolk. Items: Terminal,
Files, Settings, then `Host share` (the folder icon, present when the share is mounted) and
every installed app. Single click selects (the cell's `selection` fill, radius 6, with the
focus ring), double-click (400 ms, 4 px) or Enter launches; arrows move the selection when
the background is focused (a click on the wallpaper focuses it); right-click opens the
context menu (`Open`, and for an installed app `Uninstall...`). No dragging or arranging in
0.3.0; the order is fixed (built-ins, share, installed alphabetically).

## 7. The icon set (item 35)

Six icons at 48x48 and 32x32, drawn with `fill_rect`, `fill_rounded_rect` (`INSET_R6`,
`INSET_R4`), `stroke_rounded_rect`, `circle_rows` and `glyph`/`glyph_2x`, from a table of
shapes in the drawing crate (no bitmaps), on a transparent background (the icon draws only its
own pixels). Coordinates are (x, y) from the icon's top-left; rectangles are `x, y, w, h`.

| Icon | 48x48 | 32x32 |
|------|-------|-------|
| Terminal | tile: rounded rect r6 `0,0,48,48` fill `term.bg`, 1 px border `icon.tile.border`; `>` at 2x (16x32) at (8, 8) in `icon.prompt`; `_` at 2x at (24, 8) in `text` | tile r4 `0,0,32,32` same colours; `>` at 1x at (8, 8) `icon.prompt`; `_` at 1x at (16, 8) `text` |
| Files | tab: rect `6,8,18,8` `icon.folder.back`; back: rounded r4 `6,12,36,28` `icon.folder.back`; front: rounded r4 `6,20,36,20` `icon.folder`; a 1 px `icon.folder.back` line at row 20 across x 8..39 (the fold, inside the front's corner insets) | tab `4,5,12,5` back; back r4 `4,8,24,19`; front r4 `4,13,24,14`; fold line row 13 across x 6..25 |
| Settings | gear: circle d 28 (r 14) at centre (24, 24) `icon.gear`; teeth: rects `20,2,8,44` and `2,20,44,8` `icon.gear`, and four 8x8 squares at `8,8`, `32,8`, `8,32`, `32,32` `icon.gear`; hole: circle r 6 at (24, 24) `icon.gear.hole` | circle r 9 at (16, 16); teeth `13,2,6,28` and `2,13,28,6`, squares 6x6 at `5,5`, `21,5`, `5,21`, `21,21`; hole r 4 |
| Generic app (any source) | tile r6 `0,0,48,48` `icon.tile`, 1 px `icon.tile.border`; the app name's first letter, upper-cased ASCII (else `?`), at 2x at (8, 6) in `text`; the source badge: rounded r4 `30,26,16,20` in the source colour with the 1x letter at (34, 28): `W` in `text` on `icon.windows`, `M` in `icon.dark` on `icon.mac`, `L` in `icon.dark` on `icon.linux` | tile r4 `0,0,32,32`; the letter at 1x at (6, 6) `text`; badge: a 10x10 r4 square at `20,20` in the source colour, no letter |
| Host share (desktop and Files) | the Files icon with the front in `icon.gear` (grey: not the user's own folder) | same at 32 |

The 32x32 set is a separate table, not a scaled 48 (scaling by 2/3 has no integer form); at
scale 2 both are pixel-doubled. Host tests hash every icon at both sizes.

## 8. The Files app (item 37)

A `Normal` window titled `Files`, default content 720x480, minimum 480x300, resizable. A
userland program (`/bin/files`) drawing into its buffer with the shared widget code (buttons,
fields, lists, dialogs as sections 1 and 9 define them; the engineer's crate, suggested
`eggos-ui`, shared by Files, Settings and the installer).

Layout, content coordinates:

- Toolbar, rows 0..31, `bar.bg`, `bar.edge` line at row 31: `<` and `^` buttons (28x28 at
  x 4 and x 36, y 2, the glyphs `<` and `^` centred, `bar.hover` on hover, `text.dim` when
  there is nothing to go back or up to) and the path field from x 72 to the right edge - 8
  (28 tall at y 2, `field.bg`, the current directory's path; typing a path and Enter goes
  there, Esc restores the text).
- Sidebar, x 0..159 below the toolbar, `dock.bg`, a 1 px `menu.border` on its right: a
  heading `Places` in `text.dim` at (12, 40), then rows of 24 from y 60: `Home` (`/home/<user>`),
  `Documents`, `Downloads`, `Desktop` (the XDG dirs under home, created on first use),
  `tmp` (`/tmp`), `Host share` (`/mnt/host`, shown only when mounted), `Apps` (the catalogue's
  directory), `System` (`/`); text at x 24; the current place `selection`.
- List, from x 160 to the right edge, rows of 24 from y 32, header row first (`Name`, `Size`,
  `Modified` in `text.dim`, a `bar.edge` line under it): the type mark at x + 8 (a 12x10
  folder, two rects `0,2,12,8` `icon.folder.back` and `0,4,12,6` `icon.folder`, or a 10x12
  page, a `text.dim` 1 px outline with its top-right 3x3 corner cut), the name at x + 28 in
  `text` (directories first, each group sorted case-insensitively), the size right-aligned in
  a 10-glyph column ending 144 px from the right edge (`4.2 KB`, `12 MB`, `-` for
  directories), the modified time `YYYY-MM-DD HH:MM` in a 16-glyph column (128 px) ending
  8 px from the right edge. A selected row `selection` with a `>` in `accent` replacing its type mark
  and the focus ring when the list has keyboard focus. Programs (ELF, `MZ`, Mach-O, `.app`)
  show their source badge colour as a 6x6 square after the name (+ 8 px).
- Status bar, the last 24 rows, `bar.bg`, `bar.edge` on its top row: `N items` and, for a
  writable file system, `X free` right-aligned, `text.dim`.
- A vertical scrollbar 8 px wide at the list's right edge when the rows do not fit: track
  `progress.track`, thumb `bar.pressed` at least 24 tall; the wheel scrolls 3 rows; dragging
  the thumb scrolls.

Actions: double-click or Enter opens a directory, or a program through the install flow
(section 9), or a text file in the Terminal window with `cat` (0.3.0; an editor later);
Backspace or Alt+Up goes up; Alt+Left back; arrows, Home, End, PageUp, PageDown move the
selection; typing letters jumps to the next name starting with them; F2 renames in place (the
name becomes a text field in the row); Delete removes after a dialog (`Delete <name>?`, with
`Delete` as the primary button; there is no Trash yet because nothing survives a reboot, so
the dialog says `This cannot be undone`); Ctrl+N (Cmd+N) makes `New folder` selected for
renaming; F5 reloads; Ctrl+L (Cmd+L) focuses the path field. Right-click on a row opens the
context menu (`Open`, `Install...` on a program, `Rename`, `Delete`, `Properties`); on empty
space (`New folder`, `Reload`). `Properties` is a dialog with name, kind (`Directory`,
`Windows program, x86_64`, `macOS program, arm64`, `Linux program`, `Text`, `File`), size,
modified time and path. Every action is also the `files` CLI and its MCP tools when they
land (`files ls|rm|mv|mkdir|info`); in 0.3.0 the core utilities are that CLI.

## 9. Install and uninstall (item 36)

An "app" in 0.3.0 is a console program from another system that EggOS runs natively
(D17-D19): a Windows `.exe` (x86_64 or ARM64 PE32+), a macOS Mach-O tool or an `.app` bundle
whose `Contents/MacOS/<name>` is a console tool, or a Linux static ELF. Installing copies it
to `/home/<user>/Apps/<name>/` with the files beside it (an `.app` whole), records it in the
catalogue (`/home/<user>/.config/eggos/apps/<name>` holding name, source, architecture, the
path to run and synonyms: a plain `key=value` file, no format of our own to parse beyond
lines), and publishes it: a launcher entry, a desktop icon, the CLI command `<name>` (a
symlink in `/home/<user>/.local/bin`, which is on `eggsh`'s `PATH`) and, with the MCP
server, a tool named `app_<name>` (D21). Nothing survives a reboot in 0.3.0 (D125), which the
result dialog says.

Flow, from Files (Enter or `Install...` on a program, typically in `Host share`), from the
launcher (`Install from host share...` as the last row when the share is mounted, opening
Files at the share) or from the CLI (`app install <path>`):

1. **Prompt**, a `Dialog` of the caller (or of a small installer window when called from the
   CLI with a display), 480 wide, title `Install`: the 48x48 generic icon of the source with
   the name's letter at (16, 16); from x 80, lines at pitch 20: `Install <name>?` in `text`,
   then in `text.dim`: `<Windows program, x86_64 | macOS program, arm64 | Linux program>`,
   `From: <path>`, `To: /home/<user>/Apps/<name>`, `Adds: launcher entry, desktop icon,
   command "<name>"`, and when the layer is not installed `Installs: the <Windows|macOS> layer
   (<size> KB)` (D18). Buttons at the bottom right: `Cancel`, `Install` (primary, Enter). A
   program the machine cannot run (a macOS program on riscv64, an x86_64 program on aarch64
   until translation exists) shows instead `EggOS cannot run this on <arch>: <reason>` and
   only `Close`.
2. **Progress**, the same dialog with its body replaced: a progress bar (full width minus the
   padding, 8 tall, `progress.track`, the done part `accent`, radius 4) and a line under it
   in `text.dim`: `Copying <name>...` (the bar follows bytes), `Installing the <x> layer...`,
   `Registering...`; `Cancel` stays until the copy ends, then is `text.dim` and ignored. A
   whole install of a 10 MB program from the share takes under 2 s on QEMU; the dialog never
   blocks the rest of the desktop (the installer is its own process).
3. **Result**: `Installed <name>.` with `Open` (primary, runs it in the Terminal window) and
   `Close`, and in `text.dim` `Until a disk driver lands, installed apps do not survive a
   reboot.` On failure: `Could not install <name>: <reason>` (`not a program EggOS can run`,
   `not enough memory (ENOSPC)`, `the share is not mounted`, the layer's own refusal with its
   status) and `Close`.

Uninstall, from the launcher's or desktop icon's context menu, Files' `Apps` place (Delete on
an app) or `app remove <name>`: a dialog `Uninstall <name>?` with `Removes: the launcher
entry, desktop icon, command "<name>" and /home/<user>/Apps/<name>` and `Cancel` /
`Uninstall` (primary); the layer stays installed (Settings > Components removes it). `app
list` prints the catalogue; the MCP tools `app_install`, `app_remove`, `app_list` mirror the
three.

## 10. The Settings app (item 34)

A `Normal` window titled `Settings`, default content 640x440, minimum 560x360, a userland
program (`/bin/settings`). Sidebar x 0..175 (`dock.bg`, `menu.border` on its right), a search
field at (8, 8) of 160x28 with placeholder `Search settings`, then rows of 32 from y 44:
`Appearance`, `Display`, `Keyboard`, `Mouse`, `Components`, `About`, text at x 16, the
current one `selection`; typing in the field filters the rows and the panes' setting names by
substring, synonyms included (`Control Panel`, `Preferences`, `theme`, `scale`, `HiDPI`,
`layout`, `shortcuts`). The pane fills the rest, padding 16, with a 2x heading (16x32 glyphs)
and settings in rows of 40: the name at x 16, the control right-aligned to the pane's right
edge - 16.

The **Appearance** pane, the first shown, is the profile picker (pane coordinates, the pane
being 464 wide at the default size): heading `Where are you coming from?` at (16, 16), under
it three cards 128x112, 16 px apart, starting at x 16, y 64 (so 416 of the 432 px inside the
padding): `Windows`, `macOS`, `Linux`. Each card is a rounded rect r6 `button.bg` with
`button.border`; the selected one has a 2 px `accent` border and `selection` fill; the focus
ring when focused. In the card, a miniature of the profile's layout, 96x56 at (16, 8): a
`wallpaper.top` rect with a 1 px `border.active` outline, the panel as a `bar.bg` strip
(Windows: 6 tall at the bottom; macOS: 4 tall at the top and a 40x8 `dock.bg` strip centred
at the bottom, 2 px up; Linux: 4 tall at the top), one 40x26 mini window centred in the
remaining area (`title.active` strip 5 tall with three 2x2 dots 1 px apart in `button.close`,
`button.minimise`, `button.maximise` at the profile's side, `window.bg` body with a 1 px
`border.active` outline), and the card's label centred in `text` with its top at 76. Below
the cards, from y 192, rows: `Primary modifier` (`Ctrl` / `Cmd (Super
key)` / `Ctrl`, read-only, follows the profile), `Window controls` (`Right` / `Left` / `Right`,
read-only), `Scale` (`1x`, `2x`, two push buttons; applies on the next boot in 0.3.0, the row
says so). Left/Right move between cards, Enter or Space or a click selects; the change applies
at once: the panels are re-laid out, the controls change side, every window keeps its size
and is clamped into the new work area, the launcher's and switcher's keys switch. The setting
lives in `/home/<user>/.config/eggos/shell` (`profile=windows|macos|linux`) and the boot
parameter `eggos.profile=` overrides it; the CLI is `settings get shell.profile` and `settings
set shell.profile macos`, the MCP tools `settings_get` and `settings_set` with the same keys.

Other panes in 0.3.0, kept small: **Display** (resolution and scale, read-only, `Pointer
shown by: GPU cursor | software`), **Keyboard** (`Layout: US` read-only; `Key repeat`: delay
400 ms and rate 40 ms as two fields), **Mouse** (`Primary button: Left | Right`; `Wheel
lines: 3`), **Components** (the installed layers and the base, each with its size and a
`Remove` button that asks `Remove the <x> layer? Installed apps that need it stop running.`),
**About** (the boot screen's egg at H 48 with the name, `EggOS <version>`, architecture, CPUs,
memory, uptime, the licence line `MIT OR Apache-2.0`). Every setting is a key in the same
`settings` CLI.

## 11. Layout profiles

| | Windows habits | macOS habits | Linux habits |
|---|---|---|---|
| Panels | taskbar, bottom, 40 | top bar 28 + dock 56 (8 above the bottom) | top bar 28 with `Activities` |
| Work area at 1280x800 | y 0..759 | y 28..727 | y 28..799 |
| Window list | taskbar items | dock (per app, minimised windows after the separator) | chips in the top bar, and the launcher/overview |
| Window controls | right: close, maximise, minimise from the edge | left: close, minimise, maximise from the edge | right: close, maximise, minimise from the edge |
| Primary modifier | Ctrl | Cmd (Super key code) | Ctrl; Super for the shell |
| Launcher | Super alone, Start button | Cmd+Space, Super, the system item | Super, `Activities` |
| Switcher | Alt+Tab | Cmd+Tab (Alt+Tab too) | Alt+Tab |
| Close window | Alt+F4, Ctrl+W | Cmd+W (Alt+F4 too) | Alt+F4, Ctrl+W |
| Minimise key | Super+Down | Cmd+M | Super+H, Super+Down |
| Show desktop | Super+D | Super+D, F11 | Super+D |
| Launcher anchor | above the taskbar, centred | centred in the work area | centred, with the dash column |
| Desktop icon column | left | right | left |
| Clock and power | taskbar right | top bar centre and right | top bar centre and right |
| Names in search | `Explorer`, `Control Panel`, `Recycle Bin` | `Finder`, `System Settings`, `Trash` | `Files`, `Settings`, `Trash` |

**Default profile for 0.3.0: macOS habits** (decided by design; the owner can overrule it; the
installer's first-run question, D69, replaces the default when it exists). Reasons: it is the
layout already built and tested (top bar, controls on the left, chips became the dock's
minimised entries), so items 30-31 ship on it with no shell work; the top bar is also the
Linux profile's shape, so two of three groups see a familiar top edge on first boot; the
Windows profile is one Settings click or `eggos.profile=windows` away; and every profile's
shortcuts work in every profile, so a Windows user's Alt+Tab and Alt+F4 work before they find
Settings. Until item 35, the profile only changes the control side and the primary modifier.

## 12. Form factors and accessibility

- Form factors (D23): the layout function takes `(screen_w, screen_h, scale, profile)` and
  returns the panels, the work area and the launcher's anchor; nothing else in the shell knows
  the screen size. Roles are what adapt: on a phone every `Normal` window is maximised with
  the title and close control drawn by the server in a 28 px strip over the content's top,
  `Dialog` windows are bottom sheets (full width, anchored to the bottom), the taskbar is the
  48 px bar with the launcher, and a `Back` input is a `CloseRequest` to the top dialog or a
  `Minimised` state to the top window. On a TV, scale 2, the pointer is hidden until it moves,
  and the focus ring is the primary cue: every panel item, launcher row and desktop icon is
  reachable by arrows. Not implemented in 0.3.0; the server must not assume a pointer, a title
  bar with controls, or 1280x800, and must not reject a 360x640 or 3840x2160 screen (the
  minimum window size is clamped to the work area when the work area is smaller).
- Keyboard-only: section 5.3; the console terminal has focus at boot; opening the power menu,
  moving, sizing and switching windows now have keys, which closes the gaps D115 listed.
- Contrast: every new text pair is listed in section 1 (≥ 4.5:1); focus is the 2 px ring plus
  a fill (`selection`) or a mark (`>`); hover is a fill plus, on controls, a glyph.
- Hit targets: ≥ 28 px tall everywhere, taskbar and dock items 40 and 48, resize zones 6 px
  wide with 16x16 corners (resize is also available by keys, so the thin zones are not the
  only way).
- Motion: none added; the switcher, menus and launcher appear in one frame.
- Text: the one 8x16 size; 2x by doubling.

## 13. Rendering, budgets, tests and acceptance (items 30-31)

Placement: the server's logic (roles, stacking, focus, hit testing, geometry, the switcher
and window menu, events) in the drawing crate beside `desktop` (`drivers/src/desktop` grows
`windows.rs`, `switcher.rs`, `pointer.rs`), pure state with host tests, as D115; the device,
the buffers' mapping and the event queues in the kernel. The console terminal becomes a
client with its own buffer (its grid rendered into the buffer, committed per damaged line;
the clipping D115 deferred comes for free, so partly off-screen and maximised terminals now
work, closing those gaps).

Benchmarks (`-icount`, baselines added with the code): `gfx_desktop_full` (one window, the
D115 follow-up), `gfx_compose_four_windows` (four overlapping 658x430 windows, full screen),
`window_commit_content` (one 656x400 commit, client call to framebuffer), `window_event_key`
(one key from the input queue to the client's record), `window_hit_test` (the pointer over
eight windows).

Host tests: every role's chrome hashed focused and unfocused on both control sides; the
controls' hit targets and hover glyphs; stacking after clicks, creates, closes, minimise;
focus after each; the cascade positions for six windows at 1280x800 and 1024x768; move and
resize clamps on every edge and corner and the minimum size; maximise, restore, snap
geometries; the switcher's order and keys; the window menu's keyboard move and size; the
2 s close rule; the event queue's coalescing and `Overflow`; the pointer shapes' pixels; the
event records' layout; painting in 80 rects equals painting the whole screen with three
windows.

QEMU, all three architectures, in the boot test: a test program from our own source opens
`/dev/window`, creates a 400x300 window titled `Test`, draws a checkerboard (16 px cells,
`accent` and `term.bg`) and commits; xtask's `screendump` matches the host's render of the
desktop with the terminal and the test window cascaded over it; through QMP it drags the test
window by its title bar, resizes it from the bottom-right corner by (100, 60) (the program
answers `Resize` with a new checkerboard, and the frame before its commit shows `window.bg`
in the new area), presses Alt+Tab and checks the switcher's rect, releases and checks the
terminal focused; presses Alt+F4 on the test window and checks that the program printed
`close request` and exited and the window is gone; types in the terminal and checks the echo
as today. `wm list` prints the two windows. Serial-only boots are unchanged byte for byte.

Accepted when all of the above pass, the base image, idle memory and boot-to-prompt are
reported against D116's numbers (the growth being the server's code and one buffer for the
terminal), `docs/limitations.md` lists what moved out of it (maximise, off-screen drag,
keyboard window operations, the power menu's key) and what is still out (a second terminal,
item 35's shell, panels per profile), and `docs/decisions.md` records the placement, the
default profile and the ioctl numbers.
