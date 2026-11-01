# EggOS v0.3.0 terminals, the rest: selection, console apps in their own window, reflow, titles from the shell, `stty` (batch part 5, D222)

Design by Claude Fable (D16) for what `terminal-windows-v0.3.md` section 10 and D187's "Not
yet" left: selection, copy and paste in terminal windows; `terminal -e` and installed console
apps in a window of their own; reflow and the scrollback coming back; the shell setting the
title and the folder; the close panel in a narrow window; `stty`; touch; what agents see.
Everything in `terminal-v0.3.md` (the engine, `eggos-terminal`) and `terminal-windows-v0.3.md`
(the program, `userland/terminal`) stands unless this brief says otherwise. The field's
selection (`toolkit-v0.4.md` section 4) is the model wherever a terminal has the same thing:
same `selection` look, same keys, same word rule, same click counting. Numbers are px at 1x
and double at 2x. The feel to match: Windows Terminal, GNOME Terminal and macOS Terminal; where
they differ, the rule is chosen and the reason given.

## 0. What a person gets, and how that is checked

A person drags over a terminal's text and it turns `term.selection` blue; Ctrl+Shift+C, Cmd+C
or Ctrl+C put it on the clipboard every app shares (D197), Ctrl+Shift+V, Cmd+V or a middle
click paste from it, and a right click offers `Copy`, `Paste` and `Select All`. A console app
opened from the launcher gets a terminal window titled after it that closes when the app is
done, or stays with `[process exited with status 2]` when it is not. Dragging a window wider
re-joins the lines that wrapped; dragging it taller brings lines back from the scrollback. The
title says the folder (`~/Documents - Terminal`) or the running command, and a new window opens
in that folder. `stty size` prints `30 100` in a window dragged to 100x30. Section 9's host
tests pin each rule; its QEMU check sees two windows from the launcher and `stty size` in one.

## 1. Selection, copy and paste

### 1.1 State (program, `userland/terminal/src/selection.rs`)

Positions are **line numbers**, not rows: the engine gains `LineNumber` (`u32`, the count of
lines ever pushed into the scrollback) and `Terminal::first_line_number()` (lines the ring has
dropped), `line_number_of_row(row)` and `row_of_line_number(number)`; the live screen's row
`r` is line `first + history_length + r`, so a selection moves with its text when output
scrolls and dies with it. `Selection { anchor: Boundary, head: Boundary, mode: SelectionMode }`,
`Boundary { line: LineNumber, column: u16 }` (a column `c` is the boundary left of cell `c`,
`0..=columns`), `SelectionMode::{Cells, Words, Lines}`. The selected cells are those between the
two boundaries in reading order; equal boundaries mean none. The selection is positions, not
content: output written into selected cells stays selected (xterm's rule), and a copy takes
what the cells hold at that moment.

The selection is cleared by: a click without a drag; the alternate screen switching on or off;
`ESC c`; a reflow (section 3); `CSI 3 J` and the ring dropping its last line; Ctrl+C used as
copy (1.3); a close. It is kept across focus loss and **drawn whether or not the window has
focus** (every desktop's terminal does; a person reads a selection while typing elsewhere).
Typed keys keep it. The scrollback view (Shift+PageUp, the wheel) shows the part on view.

### 1.2 Pointer

| Input over the grid | Effect |
|--------------------|--------|
| left press | anchor = head = the nearest boundary (the cell edge nearest the pointer's x, as the field's "nearest boundary"); mode `Cells`; a selection that existed is cleared |
| drag from a press | head follows the pointer, one boundary per `PointerMove`; past the grid's right edge the head is `columns` (the line's end), past the left edge 0; above or below the grid the view scrolls one line per 50 ms towards the pointer and the head follows (the field scrolls one character per move; a terminal has lines) |
| double-click, then a drag | mode `Words`: the word under the pointer (1.4), anchor its start, head its end; a drag extends whole words at both ends (xterm) |
| triple-click, then a drag | mode `Lines`: the logical line under the pointer (the row and the rows flagged `WRAPPED` after it, 3.1); a drag extends whole logical lines |
| Shift+click | head = the nearest boundary, anchor kept, in the current mode (a word mode snaps to words) |
| right-click | the context menu (1.5) at the pointer; a right-click outside a selection keeps the selection (it is what `Copy` acts on) |
| middle-click | **paste** (1.3), in every profile: GNOME Terminal and macOS Terminal both paste on the middle button, and a Linux user's hand expects it; EggOS has one clipboard (D197), so it pastes that, and selecting never copies by itself (a path copied in Files survives a drag in the terminal) |
| wheel | scrolls the view as today, while a drag is going on too |
| release | ends the drag; a selection of no cell is none |

Clicks count against the window within `DOUBLE_CLICK_MS` (400 ms): 1, 2, 3, then over. The
pointer over the grid is `IBeam` (today `Arrow`), the `Arrow` over the padding.

### 1.3 Keys (`keys::KeyAction` gains `Copy`, `Paste`, `SelectAll`, `ContextMenu`)

| Key, every profile | Effect |
|--------------------|--------|
| Ctrl+Shift+C, Cmd+C (Super+C) | copy the selection to the shared clipboard (`Window::set_clipboard`); nothing without one; the selection stays shown |
| Ctrl+C with a selection | copy, then clear the selection; **without one `03`** as today. This is the README's rule ("copy when there is a selection") and Windows Terminal's: the next Ctrl+C is the interrupt again, because the copy cleared the selection |
| Ctrl+Shift+V, Cmd+V (Super+V) | paste the shared clipboard. Plain Ctrl+V stays `16` (`VLNEXT`, `nano`'s Page Down); a paste runs whatever ends in a newline at a prompt, so it stays on a deliberate key, unlike copy |
| Ctrl+Shift+A, Cmd+A | select all: from the scrollback's first line to the last non-blank row of the live screen |
| Shift+F10, Menu | the context menu at the cursor's cell (its left-bottom corner) |
| Esc with the menu open | closes it; otherwise `1B` as today |

**Paste.** The clipboard's text is prepared as every terminal does: `CR LF` and `LF` become
`CR` (the Enter key's byte, so a pasted script runs line by line), `TAB` is kept, every other
control character and `ESC` is dropped, `DEL` and the C1 controls U+0080 to U+009F too (the
8-bit CSI U+009B with its sequence; D222's research pass), so nothing pasted can be a sequence
and `ESC [ 201 ~` cannot be forged inside brackets; UTF-8 is passed through (the layout's non-ASCII bytes are
the same bytes, D185). With bracketed paste on (`CSI ? 2004 h`: the engine's `Modes` gains
`bracketed_paste`, read by `Terminal::bracketed_paste()`), the bytes go between `ESC [ 200 ~`
and `ESC [ 201 ~`. The paste is at most **64 KiB** (`PASTE_CAPACITY`, cut at a character
boundary; a longer clipboard pastes its first 64 KiB) and is queued in `paste::PendingPaste`,
written to the pty's controlling side **as it has room**: the kernel's controlling side drops
bytes past the line discipline's 2 KiB (D187), so the program writes at most `PASTE_CHUNK`
(1 KiB) per `POLLOUT` and the pseudo-terminal reports `POLLOUT` only with 1 KiB free (kernel
change, D187's "Not yet": a controlling side that says when it is full). While a paste is
queued, typed keys go behind it. An empty clipboard pastes nothing.

### 1.4 Words and the copied text

A word is a maximal run of cells whose characters are not a space and not one of
`` " ' ( ) [ ] { } < > , ; | & ` `` so a double-click takes `/home/mea/notes.txt`,
`--library-path=/x`, `user@host:~`, `a.b_c-d` whole (GNOME's default word characters, plus
`=` and `@`, which flags and addresses need). A run of spaces is the word a double-click on a
space selects (the field's rule); a cell of the excluded punctuation is a word of its own. A
word never crosses a row unless the row is `WRAPPED`.

Copied text: the selected cells' characters (`eggos_font::character`, the glyph's codepoint,
`?` for an unknown glyph) row by row; **each row's trailing spaces are trimmed**; rows are
joined with `\n`, except a row flagged `WRAPPED` onto the next, which joins with nothing (a
wrapped long line comes back as one line, Windows Terminal's and VTE's rule); no final `\n`
unless the selection ends past the last selected row's end (a `Lines` selection ends with
`\n`). Colours and attributes are not copied (text only, D197).

### 1.5 Drawing and the context menu

Selected cells are filled `term.selection` `#2E4A6B` (reserved by `desktop-v0.3.md`, now
used): the fill replaces the cell's background, ANSI or default, resolved after `inverse`; the
glyph keeps its foreground (`term.fg` on `term.selection` 6.7:1; every ANSI colour but one
is over 3:1 on it), except ANSI 8, bright black, which is 2.2:1 on the fill and is drawn as
`term.fg` while selected (dim text is still text). The cursor's block wins over the fill on its cell;
the unfocused outline is drawn over the fill. In the scrollback view the fill follows the view.

The context menu is the toolkit `Menu` (`menu::Entry`, items 32 tall, text at x + 12, `menu.bg`,
`menu.hover`, 1 px `menu.border`, `INSET_R6`, `MIN_WIDTH` 160), at the pointer or at the cursor,
clamped 8 px inside the content:

`Copy` [dim: no selection] `Paste` [dim: empty clipboard, read with a zero-length
`Window::clipboard` at opening] ─ `Select All` [dim: every line blank]

Up/Down/Enter/Esc as the toolkit's; a press outside closes it and is delivered; focus loss
closes it. While it is open keys do not reach the shell; output is drawn under it. Ids: `COPY
0`, `PASTE 1`, `SELECT_ALL 2`. The same three verbs are the touch sheet's (section 7).

## 2. `terminal -e`, console apps in their own window

**Command line.** `terminal [-t <title>] [-e <program> [arguments...]]` (`program.rs`,
`Options { title, command }`). `-e` takes the rest of the line as the program and its
arguments, run **directly** on the pseudo-terminal, found through `PATH` as the shell finds
commands, with the caller's environment and folder: xterm's and Windows Terminal's rule, so
quoting is the caller's shell's job, not a second shell's. A shell line is run as
`terminal -e /bin/eggsh -c '<line>'`. `-t` sets the window's title text (`<title> - Terminal`,
the `Title` rules); without it a `-e` window is titled after the program's last path component
(`ps - Terminal`), and `OSC 0`/`OSC 2` from the program still win. Exit codes as today; `2`
with `terminal: -e needs a program` for `-e` alone; `terminal: <program>: not found` is
written in the new window (so the person sees it) and the window follows the "exit 127" rule.

**When the command ends** (`window::ProcessEnd::{Exited(u8), Signalled(u8)}`, learned when the
program side closes and the child is waited for):

- **status 0: the window closes at once.** The person asked for a command and it is done;
  a window with nothing in it is the thing every terminal removes (GNOME, xterm, Windows
  Terminal's default `graceful`).
- **any other status, or a signal: the window stays**, the cursor hides, and the program itself
  writes one line into its grid (not through the pty, which is gone) at the start of the row
  after the cursor's (scrolling if needed), in ANSI 8 (bright black, the grid's `text.dim`):
  `[process exited with status 2]` or `[process ended by signal 11 (SIGSEGV)]` (the shell's
  signal names, D181). Any key that would send bytes closes the window; the window's own keys
  (copy, select all, the menu, the scrollback keys) still work, so the error can be read and
  copied; the close control, Alt+F4, Cmd+W and `wm close` close without asking (nothing runs).
  Chosen over "close always" (an error from a launcher-started app vanishes before it is read)
  and "keep always" (macOS Terminal's `[Process completed]` clutter). The element tree shows the
  line as its row; `wm show` reads it.

A `-e` window closed while the program runs asks the same question as a shell window (its
foreground group is not a shell's: the question is right, the program will be ended).

**Installed console apps.** The manifest gains `console = true | false`, written by the
installer: a Windows program by its PE subsystem (`Subsystem::Console`), a bare macOS Mach-O
tool `true` and an `.app` bundle `false`, a Linux program `true` (a Linux GUI program opening its
Wayland window from a terminal is what a Linux user has always seen, and a clean exit closes the
terminal anyway); `app install --console` and `--no-console` override, and `app list` prints
the field after the icon. The catalogue, with `terminal_present` and `console`, launches the
app as `Launch::Start` of `/bin/terminal -t <name> -e /bin/eggsh -c <line>` (`line` is today's
launch line: environment words, loader, program, arguments); without the terminal program, or
for `console = false`, as today (`Launch::Type` into the `Console`, or the layer's own start).
`app open <name>`, MCP `app_open` and `OPEN_APP` follow the catalogue, so all three front-ends
agree (D22). Launcher rows do not change (name, own icon or tile, `Open`); the window's title
`<name> - Terminal` makes `app_of` group it under Terminal in the dock and taskbar, which is
what it is.

## 3. Reflow, and the scrollback coming back

### 3.1 The flag

The engine keeps one bit per line, `line_flags::WRAPPED` (a `[u8]` beside the cells of the
screen and the ring, no per-cell cost): set on the line autowrap continued onto, cleared when
a `LF`/`ESC D`/`ESC E` leaves the line before the wrap, by `CSI 2 K`/`CSI J` erasing the line's
end, and by `ESC c`. A logical line is a row plus the `WRAPPED` rows after it. Section 1.4's
copy and 1.2's triple-click use it.

### 3.2 The rules (main screen; `Terminal::resize` does them)

1. **Width changes re-lay every logical line** of the scrollback and the screen: its cells in
   order, the last row's trailing blanks trimmed (inner rows' kept), cut into rows of the new
   width, continuations flagged `WRAPPED`. A blank logical line stays one blank row. Lines
   that were cut by an old narrowing are not recovered (they were dropped then; with reflow
   nothing is cut any more).
2. **The cursor keeps its cell**: its logical line and offset within it; after the re-lay it
   is at that cell's row and column; a cursor on blanks past a line's end keeps its offset,
   clamped to the new width; the pending-wrap flag is cleared. Saved cursors (`ESC 7`, `CSI s`)
   are clamped to the grid, not reflowed.
3. **Height, then**: the live screen is the last `rows` rows of the re-laid content, bottom
   aligned at the cursor's line or below it: rows after the cursor's line that are blank are
   dropped first, then rows above the screen go into the ring (shorter), and when the screen
   has room, **lines come back from the ring** at the top, newest first, until the ring is
   empty or the screen is full (taller); only then are blank rows added at the bottom. The
   cursor moves with its row. This is VTE's and Windows Terminal's behaviour; xterm's "blank
   rows below" is what section 3 of the windows brief did and is replaced.
4. The ring keeps the newest 256 lines at the new width (a narrower window makes more rows, the
   oldest go); the scroll region is reset to full; the view returns to the live screen; the
   selection is cleared (its columns no longer mean anything; Windows Terminal re-maps, a drag
   is cheaper than the code); the alternate screen is cleared to the new size (a full-screen
   program redraws on `SIGWINCH`), with no reflow and no flags.
5. Every `Resize` that changes the grid reflows (the shell is told each time already); a reflow
   touches at most 512 x 512 cells and is the benchmark `term_reflow` (80x24 to 100x30 with a
   full ring; D78), added with the code and reported in the PR.

## 4. Titles and folders from the shell

- **`TERM_PROGRAM=EggTerminal`** is set in the shell's environment by the terminal program
  (macOS Terminal's `Apple_Terminal`, VTE's `vte` and iTerm's are the convention scripts test).
  `eggsh` writes `OSC 0` and `OSC 7` **only when `TERM_PROGRAM` is set**, so the serial console,
  the `Console` window and every boot log stay byte for byte as they are.
- **Title text**: at each prompt, the current folder with the home folder as `~`
  (`~`, `~/Documents`, `/etc`); while a foreground job runs, the command's name (its first
  word's last path component, `sleep`), restored at the next prompt. So the title reads
  `~/Documents - Terminal` (GNOME's folder) or `sleep - Terminal` (macOS's process). A set
  `PS1` does not turn it off. Written as `ESC ] 0 ; text BEL` to standard error with the
  prompt; the `Title` cut at 50 bytes + `...` applies (a deep path loses its end, as GNOME's).
- **`OSC 7 ; file://<host>/<path> BEL`** (or `ESC \`) is written by `eggsh` at each prompt
  with `PWD` percent-encoded (RFC 3986: everything but unreserved characters and `/`), host
  empty. The engine parses it (`parser::OSC_CURRENT_FOLDER` 7; `Terminal::current_folder()`,
  `take_folder_change()`, at most `PATH_CAPACITY` 4096 bytes): a host other than empty or
  `localhost`, a relative path, a bad percent escape or invalid UTF-8 ignores the sequence.
  The folder is never shown; it is state.
- **A new window opens in the current folder**: Ctrl+Shift+N, Cmd+N and `Shell::open_new_window`
  start `/bin/terminal` in `current_folder` (the child changes folder before it starts the
  shell; `ENOENT`, a folder since removed, falls back to the home folder); a window that has
  seen no `OSC 7` (a `-e` program) uses its own starting folder. The launcher, dock, `app
  open` and `app_open` still open in the home folder.

## 5. The close panel in a narrow window, and the minimum size

The minimum content stays **176x96** (20x5 cells): 20 columns hold a prompt and a short
command, 5 rows a command and its answer, and a 160 px menu fits inside with its 8 px margin.
In a content narrower than 416 the toolkit's dialog panel (windows brief 4) is the content
width minus 16, its two lines end-ellipsised at the inner width (`Close this termi...`), the
buttons `(inner - 8) / 2` wide each, at least 64 (`Cancel` and `Close` in Egg Sans are 32 and
26 px of glyphs); when two 64 px buttons do not fit the inner width (under 136, only on a work area
smaller than the minimum, which the server clamps to), the buttons stack, `Close` below
`Cancel`, and the panel is 148 tall. The size indicator (`20x5`, 48 px wide) always fits. On a
phone the window is maximised (section 7) and never narrow.

## 6. `stty`

A new core utility `stty` (`userland/coreutils/src/stty.rs`; `TCGETS`, `TCSETS`, `TIOCGWINSZ`,
`TIOCSWINSZ` on standard input, or on `-F <device>`), GNU's grammar and output so scripts that
parse it run unchanged:

| Call | Output |
|------|--------|
| `stty size` | `<rows> <cols>\n` (`24 80`) |
| `stty` | `speed 38400 baud; line = 0;\n`, then only the modes that differ from `sane` and the control characters that differ from their defaults, in GNU's order and wrapping |
| `stty -a` | GNU's four lines after `speed 38400 baud; rows 24; columns 80; line = 0;`: the control characters (`intr = ^C; quit = ^\; erase = ^?; kill = ^U; eof = ^D; eol = <undef>; eol2 = <undef>; swtch = <undef>; start = ^Q; stop = ^S; susp = ^Z; rprnt = ^R; werase = ^W; lnext = ^V; discard = ^O; min = 1; time = 0;`), then the control, input, output and local flags, each `name` or `-name`, in GNU's order, values from the terminal's actual settings (the kernel's `SETTINGS`, D187) |
| `stty -g` | GNU's saved form: the four flag words and the 19 control characters as lowercase hex joined by `:`; `stty <that string>` restores it |
| `stty raw` | `-ignbrk -brkint -ignpar -parmrk -inpck -istrip -inlcr -igncr -icrnl -ixon -ixoff -icanon -opost -isig -iuclc -ixany -imaxbel -xcase min 1 time 0`; no output |
| `stty sane` | GNU's `sane` set (`cread -ignbrk brkint -inlcr -igncr icrnl icanon iexten echo echoe echok -echonl -noflsh -ixoff -iutf8 -iuclc -ixany imaxbel -xcase -olcuc -ocrnl opost -ofill onlcr -onocr -onlret nl0 cr0 tab0 bs0 vt0 ff0 isig -tostop -ofdel -echoprt echoctl echoke -extproc -flusho`) and every control character to its default; no output |
| `stty [-]<mode>...`, `stty <char> <value>` (`^C`, `undef`, a character), `stty rows N`, `stty cols N` / `columns N`, `stty min N`, `stty time N` | sets them; `rows`/`cols` are `TIOCSWINSZ` (the shell's foreground group gets `SIGWINCH`; the window's grid does not change, xterm's rule, and the next `Resize` reasserts it) |

Errors, GNU's words, exit 1: `stty: 'standard input': Inappropriate ioctl for device` (not a
terminal; this is what MCP `run` sees), `stty: invalid argument '<word>'`, `stty: when
specifying an output style, modes may not be set`, `stty: missing argument to '<mode>'`.
Flags the kernel does not keep (`cmspar`, `extproc`, ...) print as GNU would for a Linux
default and setting them is accepted and ignored. `stty` is the CLI face of the grid's size;
the GUI face is the size indicator, the MCP face `wm_show`'s `screen` value (D22, section 8).

## 7. Device classes (D23)

- **Phone and tablet** (touch, the window maximised and chromeless): a tap sends nothing and
  clears a selection; a **long-press** (500 ms, the toolkit's) selects the word under the
  finger and shows two **handles**: 12 px `accent` discs on 2 px stems hanging from the
  selection's first and last boundary (below the row, the first's disc left of its stem, the
  last's right, Android's shape; 40x40 hit targets), dragged to move that boundary by cell;
  releasing a long-press or a handle opens the toolkit's **bottom sheet** (toolkit v0.5
  section 11: full width, items 48 tall) with `Copy`, `Paste`, `Select All`, the dim rules of
  1.5; a tap outside the sheet closes it and keeps the selection and handles; a long-press
  without text under it opens the sheet with `Copy` dim. Two-finger drag scrolls the view.
  An on-screen keyboard is still a later item (a phone terminal reads and pastes until then).
- **TV** (D-pad, scale 2): no pointer, no selection by hand; the remote's `Menu` key opens the
  menu with `Paste` and `Select All` (`Copy` enabled once `Select All` ran); `Back` is Esc.
  A keyboard "mark mode" (Shift+arrows from the cursor, Windows Terminal's) is section 10's.
- **2x**: cells 16x32, the fill, handles (24 px discs, 4 px stems), menu rows 64, the sheet's
  96, `PASTE_CAPACITY` and the exit line unchanged (cells, not pixels).

## 8. What an agent sees (D161, D21)

- **Element tree** (`wm show`, `wm_show`, `GET_TREE`): the root `window` is named by the title
  as today; the `screen` group (id 2, value `<cols>x<rows>`) and the `row <n>` texts (ids 3
  upwards) stay; a row with a selected cell carries flag `SELECTED` (8). After the rows, when
  there is a selection, one `text` named `selection` whose value is the copied text of 1.4 cut
  at `MAX_TEXT_BYTES` on a character boundary, and when `current_folder` is known one `text`
  named `folder` with the path. Actions: `select` on a `row` selects its logical line
  (`Done`; the selection then shows), `select` on `screen` selects all; everything else stays
  `NotSupported`. An agent copies with `wm_press` `Ctrl+Shift+C` and reads `clipboard_get`,
  or reads the `selection` element directly, which needs no clipboard.
- **`wm_capture`** shows the fill, the handles and the exit line as pixels, as it shows any
  client's content. A terminal driven by an agent wears ` (agent)` as any window.
- **`run`** opens a window with `/bin/terminal -t <title> -e <program> ...` (the run grant, a
  desktop present: `terminal: no display` otherwise, exit 1), and `run` of `/bin/stty size`
  fails with GNU's "Inappropriate ioctl" because `run` has no terminal: the size of a terminal
  window is `wm_show`'s `screen` value, the same capability by another front-end (D22). No new
  MCP tool.

## 9. Acceptance

Host tests (the program's `TerminalWindow` over a fake `Shell`, 656x400, 1x; the engine; `eggsh`
over its fake system; `stty` over a fake terminal):

1. A press at x = 8 + 8·10 + 3 is boundary column 10; at + 5, column 11; a press and release
   without a move selects nothing; a drag from (row 2, col 10) to (row 2, col 20) selects cells
   10..20 of line 2 and `wm show` marks `row 3` `SELECTED`; a selected cell's pixels are
   `term.selection` under its glyph, and a selected bright-black glyph is drawn in `term.fg`.
2. A drag from (row 1, col 70) to (row 3, col 5) selects to line 1's end, all of line 2, and
   cells 0..5 of line 3; copied text is `<tail of 1>\n<2 trimmed>\n<head of 3>`; with line 1
   flagged `WRAPPED` the first `\n` is absent.
3. Double-click on `/home/mea/a.txt` selects it whole; on `x=(1,2)` the word is `x=` and a
   second double-click on `(` selects `(` alone; on three spaces, the three; a drag after a
   double-click extends by words at both ends.
4. Triple-click on a row whose next row is `WRAPPED` selects both rows and the copy ends with
   `\n`; Shift+click moves the head and keeps the anchor; a plain click clears.
5. A selection on row 23 is on row 22 after one scrolled line (same `LineNumber`), is in the
   scrollback view after 24, and is gone when its line leaves the ring; the alternate screen
   switching on clears it.
6. Ctrl+Shift+C and Super+C copy and keep the selection; Ctrl+C with a selection copies and
   clears it, and a second Ctrl+C sends `03`; Ctrl+V sends `16`; Ctrl+Shift+A selects every
   non-blank line.
7. Paste of `a\r\nb\nc\x1b[31m\td` writes `a\rb\rc\td`; with `CSI ? 2004 h` first it is
   `ESC [ 200 ~ ... ESC [ 201 ~`; a 70 KiB clipboard writes 64 KiB at a character boundary, 1 KiB
   per `POLLOUT`; a middle click pastes; an empty clipboard writes nothing.
8. Right-click opens the menu with `Copy` dim and `Paste` lit (fake clipboard non-empty);
   `Select All` then Enter on `Copy` copies; Esc closes with `1B` not sent; a press outside
   closes it and is delivered to the grid.
9. `terminal -t Build -e /bin/make all` titles `Build - Terminal` and runs `/bin/make` with
   `all`; `-e ps` titles `ps - Terminal`; exit 0 closes within the frame; exit 2 draws
   `[process exited with status 2]` in ANSI 8 on the next row, hides the cursor, keeps the
   window, closes on `a` and not on Ctrl+Shift+C; signal 11 draws
   `[process ended by signal 11 (SIGSEGV)]`; `-e` alone exits 2 with the message.
10. The catalogue, with `terminal_present` and a manifest `console = true`, launches
    `/bin/terminal -t <name> -e /bin/eggsh -c <line>`; `console = false` or no terminal
    program launches as before; the installer writes `console = true` for a PE console
    subsystem, `false` for a GUI subsystem and an `.app`, `true` for a Linux ELF, and
    `--no-console` wins.
11. Reflow: a 100-cell line in an 80-column grid is two rows, the second `WRAPPED`; at 120
    columns it is one row and the cursor at offset 95 is at column 95; back at 80 the two rows
    return; a logical line whose trailing blanks were trimmed is not padded; the blank rows
    below the cursor go first when shorter; with 10 lines in the ring and the window grown by
    6 rows, 6 come back at the top and the cursor's row moves down 6; grown by 16, all 10
    come back and 6 blank rows are added; the alternate screen is cleared, not reflowed; the
    selection is cleared; `term_reflow` has a baseline.
12. The close panel in a 176 px content is 160 wide, its lines ellipsised at 136, its buttons
    64 each; at a clamped 150 px content the buttons stack and the panel is 148 tall.
13. `eggsh` with `TERM_PROGRAM=EggTerminal` writes `ESC ] 0 ; ~/Documents BEL` and
    `ESC ] 7 ; file:///home/mea/Documents BEL` before the prompt, `ESC ] 0 ; sleep BEL` before
    running `sleep 1`, and the folder again at the next prompt; a space in `PWD` is `%20`;
    without `TERM_PROGRAM` not one `ESC ]` is written.
14. The engine parses `OSC 7 ; file:///a%20b BEL` to `/a b`, `file://localhost/x` to `/x`,
    ignores `file://other/x`, `file://x` (relative), `%zz` and invalid UTF-8; `wm show` lists
    `folder`; a new window from Ctrl+Shift+N starts in it, and in the home folder when it is
    gone.
15. `stty size` prints `24 80\n` then `30 100\n` after `TIOCSWINSZ`; `stty -a` on the kernel's
    defaults is the exact expected text; `stty raw` then `stty -g` differs from the start,
    `stty <saved>` restores it, `stty sane` equals the defaults; `stty cols 100` sends
    `TIOCSWINSZ` and `SIGWINCH`; on a pipe `stty: 'standard input': Inappropriate ioctl for
    device`, exit 1; `stty frob` exits 1 with `invalid argument 'frob'`.
16. Touch: a long-press at a word selects it, handles at its boundaries, each with a 40x40
    target; dragging the right handle two cells extends two cells; release opens the sheet
    with `Copy`, `Paste`, `Select All`; a tap elsewhere clears everything.

QEMU check (the GUI test of the windows brief, extended): the launcher opens Terminal twice
(two cascaded windows below `Console`, the second focused, both titled `~ - Terminal`); the
second is resized to 100x30 (`wm size`); `stty size` typed into it prints `30 100` and the
frame's hash matches; `wm show` on it lists `folder` as `/root` and `row 1` as the prompt.

## 10. Later

Copy with colours (HTML), a "paste a multi-line text?" question, a primary selection for
Wayland clients, mouse reporting to programs (then Shift+drag selects past it), keyboard mark
mode (TV), rectangular selection (Alt+drag), search in the scrollback, URL detection and
Ctrl+click, tabs and panes, the on-screen keyboard, `stty ispeed`/`ospeed`, the bell.
