# EggOS v0.3.0 terminals as programs: design brief (part 35)

Design by Claude Fable (D16) for the userland terminal `/bin/terminal`: several terminal
windows, each resizable and closable, drawn with the console terminal's engine
(`terminal-v0.3.md`: Egg Mono 8x16 cells, the 16 colours, 8 px padding in `term.bg`, the
cursor looks, the scrollback keys, the ANSI subset, the key map). The window itself is the
`Normal` window of `desktop-v0.3-gui.md` section 3 (chrome, cascade, move, resize, close); the
Text Editor (`text-editor-v0.3.md`) is the model for one window per process and the title
suffix the shell matches by. Everything in those briefs stands unless this one says otherwise.
Numbers are px at 1x and double at 2x. The feel to match is macOS Terminal, Windows Terminal
and GNOME Terminal: open one, open another, drag it to the size you want, close it.

## 1. Process, window and title

- **Process.** `/bin/terminal`, an optional component installed with the desktop (D18), one
  window per process. It forks first and the parent exits 0 at once (a process with a mapped
  window cannot fork), so `terminal` typed at a prompt returns at once, as `open -a Terminal`
  does. The child opens `/dev/window`, opens `/dev/ptmx`, and runs `/bin/eggsh` on the
  pseudo-terminal as a session leader whose controlling terminal it is, with `TERM=xterm` and
  the caller's environment and current folder (started from the shell: the shell's folder;
  from the launcher, the dock, a desktop icon or `app open`: the home folder, as Files is
  started). The program polls the window's descriptor and the pty master together, so it
  answers events while the shell is quiet and never earns `(not responding)`.
- **Window.** A resizable `Normal` window with the flag `TAKES_CTRL_KEYS` (every Ctrl key is
  the program's, as the console's are), default content **656x400** (80x24 cells, the
  console's size), `SET_MINIMUM_SIZE` **176x96** (20x5 cells). No `FIXED_SIZE`. Placed by the
  cascade (section 3.4 of the GUI brief): the first program terminal lands 32 px right and
  below the console window when that is the top window, and each next one 32 px further.
- **Title.** `Terminal` by default. A program may set it with xterm's `OSC 0 ; text BEL`,
  `OSC 2 ; text BEL` (or `ESC \` as the terminator; `OSC 1`, the icon name, is ignored): the
  title becomes `<text> - Terminal`, so the suffix ` - Terminal` is how the shell knows the
  window, as ` - Text Editor` is the editor's; an empty text restores `Terminal`. The text is
  UTF-8 with control characters dropped; the title is at most 64 bytes, the suffix 11, so a
  text longer than 53 bytes is cut to its first 50 bytes, backed off to a character boundary,
  plus `...`. The title bar's own `...` cut applies on top when the window is narrow. The
  server's ` (not responding)` and ` (agent)` suffixes apply as to any window.
- **The console window is retitled `Console`**, whether or not the program is installed, so
  the kernel's console session (boot messages, the serial console's mirror, the login shell
  init starts) is told apart from program terminals at a glance; "console" is the word every
  Linux user has for it and a launcher synonym of Terminal already. Every other rule of the
  console window stays (section 5).
- **Window matching.** `app_of` names a window titled `Console`, `Terminal`, or ending with
  ` - Terminal` as the Terminal app's, so all of them share the Terminal icon: the dock shows
  one Terminal icon with the running mark while any of them is open (a click focuses the most
  recently focused one, the next click the next, the dock's rule), the taskbar one item per
  window with its title as the tooltip, the Linux top bar one chip per minimised window, the
  switcher one row per window. Several windows titled `Terminal` are told apart by their
  place on the screen, as on Windows and GNOME; a title that says more is section 10's.

## 2. Opening

| From | With `/bin/terminal` in the image | Without it |
|------|-----------------------------------|------------|
| the launcher's `Terminal` row, its `Open`, the desktop icon, `OPEN_APP Terminal`, `app open Terminal`, MCP `app_open Terminal` | a new window each time, as the Text Editor opens (D149) | focuses the `Console` window, restoring it if minimised (today's rule) |
| the dock's Terminal icon | the dock's rule: launch when the app has no window (the `Console` counts as one), else focus and cycle | focuses the `Console` |
| `terminal` at a prompt | a new window; the prompt returns at once | `terminal: not found` (the file is not there) |
| the keys below, in a focused terminal window | a new window, started in the home folder | n/a |

Keys the program handles itself (they produce no bytes for the shell): **Ctrl+Shift+N** and
**Cmd+N** (Super+N) open a new window, in every profile, the Text Editor's habit of accepting
both modifiers everywhere, since Ctrl+Shift+N is Windows Terminal's and GNOME Terminal's and
Cmd+N macOS Terminal's. Plain Ctrl+N is `0E` for the shell, as the key map says. The new
window takes focus at its first commit and cascades from the window the key was pressed in.

## 3. Resizing

- **Grid follows the content.** Columns `floor((w - 16) / 8)`, rows `floor((h - 16) / 16)`,
  the 8 px padding on the left and top, the remainder of the padding on the right and bottom,
  all in `term.bg`. Caps: 512 columns and 256 rows; a larger content pads further. The
  minimum is 20x5. Every `Resize` (an edge or corner drag, maximise, restore, snap, the window
  menu's `Size`, `wm size`) is answered with `SET_SIZE`, the grid redrawn whole and committed;
  while the user drags, the server shows the last frame clipped and `window.bg` where nothing
  is committed yet, so the program answers every `Resize`, never only the last.
- **The shell is told.** After each grid change the program calls `TIOCSWINSZ` on the master
  with `ws_row`, `ws_col`, `ws_xpixel = cols * 8`, `ws_ypixel = rows * 16` (double at 2x), and
  the kernel sends `SIGWINCH` to the foreground group (D181); `TIOCGWINSZ` reports the grid.
- **What happens to the text.** No reflow in 0.3.0. Narrower: the cells past the new width are
  cut from every row and from the scrollback's view, and do not come back when widened (xterm's
  rule); the cursor is clamped to the last column. Shorter: blank rows below the cursor go
  first, then rows from the top go into the scrollback and the cursor moves up with them, as
  the console does when it restores from maximised. Taller: new blank rows at the bottom; the
  scrollback is not pulled back. The alternate screen is cleared to the new size (a full-screen
  program redraws on `SIGWINCH`). The scrollback keeps 256 lines; a line keeps the cells it had
  up to the width it was written at, drawn cut or padded to the current width.
- **Size indicator.** While the grid changes, the label `<cols>x<rows>` (`100x30`) in Egg Mono
  on a tooltip tile (6.4 item 3: `menu.bg`, 1 px `menu.border`, r4, 24 tall, width = glyphs x 8
  + 16, text at x + 8, glyph top at row 4, in `text`) centred on the content, drawn over the
  cells and the cursor, from the first `Resize` that changes the grid until 600 ms after the
  last one; a `Resize` that keeps the grid (a few pixels) does not show it. Nothing is sent to
  the shell for the label.

## 4. Closing

- **Ways to close**: the close control, the window menu's `Close`, `wm close`, **Alt+F4**
  (every profile, the server's), **Cmd+W** (the server's in the macOS profile), and, handled
  by the program itself since it takes Ctrl keys, **Ctrl+Shift+W** in every profile (Windows
  Terminal's and GNOME Terminal's; plain Ctrl+W stays `17`, the shell's word erase) and
  Super+W in the Windows and Linux profiles, where the server passes it on. All of these become
  the same close request inside the program.
- **Asking first.** When the pty's foreground process group is the shell's own (the prompt is
  waiting, or a job is stopped), the window closes at once. When it is not (a program runs in
  the foreground), the program shows the toolkit's dialog panel (8.1 item 3: 400 wide, `menu.bg`,
  `INSET_R6`, two lines, 112 tall; in a content narrower than 416 the panel is the content width
  minus 16 and its lines are end-ellipsised at the inner width), centred on the content:

  | Line 1 (`text`) | Line 2 (`text.dim`) | Buttons (left to right) |
  |-----------------|---------------------|-------------------------|
  | `Close this terminal?` | `A program is still running in it and will be ended.` | `Cancel`, `Close` (primary) |

  Focus starts on `Close`; Enter is `Close`, Esc is `Cancel`, Tab moves between the two, Space
  presses the focused one. While the panel shows, keys and the wheel do not reach the shell,
  output still arrives and is drawn under the panel, and the cursor is drawn unfocused. One
  wording in every profile (macOS Terminal's question in fewer words; GNOME's "will kill it" is
  not used). Background jobs do not ask: the shell's own hang-up handling covers them.
- **What closing does.** The program destroys the window and closes the pty master; the kernel
  hangs the session up, so the foreground group gets `SIGHUP` then `SIGCONT` (D181) and the
  shell and its jobs end as on a lost serial line. When the shell exits on its own (`exit`,
  Ctrl+D at the prompt), the program closes the window and exits 0 within the same frame; no
  "process completed" line is left behind (macOS Terminal's habit is not followed: a window
  with no shell in it has nothing to do).

## 5. The console window

Unchanged except its title (`Console`, section 1): the grid stays 80x24 with maximise as its
only size change, the close control stays `button.inactive` with no hover glyph and clicks
ignored, Alt+F4 and Cmd+W do nothing on it, `MANAGE_WINDOW` close is `EPERM`, `CAPTURE_WINDOW`
and `GET_TREE` stay `EOPNOTSUPP` on it until it too is drawn by a client, and
`TYPE_IN_TERMINAL`, the Agent chip's `agentlog` and installed console apps launched from the
launcher keep going to it. With the program installed, launching Terminal from the launcher
no longer focuses the console (section 2); the console is reached through the switcher, the
dock, the taskbar or the chips like any window.

## 6. Agents

- `wm capture`, `wm click`, `wm type`, `wm press`, `wm scroll`, `wm show`, `wm act` and the
  MCP twins (`wm_capture`, `wm_type`, ...) work on a program terminal as on any window: its
  content is the client's buffer, so `CAPTURE_WINDOW` copies it; `SEND_INPUT` keys and
  characters become bytes for the shell through the program's key map (a character of code 0
  as UTF-8, D185), the wheel scrolls the view, pointer input is accepted and does nothing
  (selection is section 10's). A driven window wears ` (agent)` as any other.
- **Element tree** (`GET_TREE`), so an agent reads the screen as text instead of pixels: the
  root `window` (id 1) named by the title; one `group` (id 2) named `screen`, rect = the cell
  grid, value `<cols>x<rows>`; under it one `text` element per visible row, ids 3 upwards,
  named `row <n>` (1-based from the top), rect = the row's cells, value = the row's characters
  as UTF-8 with trailing spaces trimmed (an empty row's value is empty), cut at 255 bytes on a
  character boundary. The rows are what the window shows: the alternate screen when it is on,
  the scrollback view while scrolled back. The row holding the cursor carries flag 1
  (`focused`) when the cursor is shown. Every action (`invoke`, `focus`, `set-text`,
  `select`) answers 2, not supported: an agent types with `wm_type` and `wm_press`, which is
  what a person does. With more rows than fit 64 KiB, the top rows are left out.

## 7. Accessibility and form factors

Keyboard-only use needs no pointer: Super, `t`, Enter opens one; Ctrl+Shift+N another; Alt+Tab
switches; Alt+Space then `Size` and the arrows resize by 8 px (the size indicator shows the
grid); Alt+F4, Cmd+W or Ctrl+Shift+W close, and the panel takes Tab, Enter and Esc. Focus is
shown by the cursor (block when focused, 1 px outline when not) on top of the chrome's three
cues; the indicator and the panel are text on a fill, nothing colour-only; contrast pairs are
the existing ones. At 2x every number doubles (cells 16x32, padding 16, default content
1312x800, minimum 352x192) and the server clamps the default to the work area less 32 each
way, so on a 1280x800 display at 2x the first terminal opens at the clamped size with the grid
that fits. On a display whose work area is smaller than the default, the same clamp applies and
the grid follows; the minimum window is clamped to the work area when that is smaller. On a
phone the window is maximised and chromeless by the server's rule (section 12 of the GUI
brief) and the grid fills it; an on-screen keyboard is not in 0.3.0. Motion: the cursor's
blink only.

## 8. CLI and MCP (D22)

`terminal` (no options; `-e <command>` is section 10's) is the CLI face: exit 0 once the window
process is started, 1 with `terminal: no display` when `/dev/window` cannot be opened, 2 for an
argument. `app open Terminal` and MCP `app_open Terminal` open one by name; `wm list` shows each
with its title; the `wm` verbs and their MCP twins operate it. No new MCP tool.

## 9. Tests that pin the look and the behaviour

Host tests on the program's state (pure state over the window and pty traits): the grid for
656x400, 176x96, 657x401 (remainder padding), 2x; the title for no OSC, `OSC 0`, `OSC 2`, an
empty text, a 60-byte text cut to 50 + `...`, and control characters dropped; narrower then
wider loses the cut cells; shorter moves rows into the scrollback and clamps the cursor;
`TIOCSWINSZ` values after each; Ctrl+Shift+N and Super+N produce no bytes; the close question
appears only with a foreground group other than the shell's, and its three answers; the
element tree's rows, trimmed and cut at 255 bytes, and the cursor row's flag. QEMU GUI test
frames: two terminals cascaded below the `Console` with the second focused; one resized to
100x30 with the indicator showing `100x30`; the close question over a running `sleep`; then
`wm show` on a terminal lists `row 1` with the prompt, and closing the last program terminal
leaves the `Console` focused.

## 10. Later

Selection, copy and paste (Ctrl+Shift+C/V, Cmd+C/V); the shell setting the title to its
folder (`OSC 0` from `eggsh`'s prompt) and `OSC 7`; a new window opening in the current
window's folder; tabs and split panes; pulling scrollback lines back when the window grows,
and reflow; `terminal -e <command>` and installed console apps launched from the launcher
getting their own window instead of the `Console`; a numbered title (`Terminal 2`); a
right-click context menu on the dock's icon with `New Window`; font size, themes and
transparency; the bell as a flash; mouse reporting to programs; an on-screen keyboard on
phones.
