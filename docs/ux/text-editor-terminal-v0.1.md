# EggOS Text Editor: terminal mode (v0.1)

Design by Claude Fable (D16) for `edit [path]` on a console or terminal with no desktop, which
today refuses with "there is no desktop" (`userland/editor/src/command.rs`, D149). It edits the
same document as the GUI editor (`text-editor-v0.3.md` section 5 and D149: UTF-8 text of at most
4 MiB, undo and redo, the editor's own clipboard, find, line endings and the byte-order mark
kept, files refused with the editor's words), with the same words wherever the GUI has them, on
a VT100/xterm byte stream: the ANSI subset of `terminal-v0.3.md`, 16 colours, no mouse. The feel
to match is nano and micro for a Linux hand, `edit.com` and the new Microsoft Edit for a Windows
hand, and the GUI editor itself for anyone who used it: `Ctrl+S` saves, `Ctrl+Q` quits, the
bottom row says so. One cell is one character column; rows and columns below are 1-based,
`R` rows by `C` columns.

## 0. What a person gets, and how that is checked

On the serial console, or in a terminal window with `edit -t`, `edit notes.txt` fills the screen
with the file, its name in a bar on top, `Ln 1, Col 1` and `LF` in a bar at the bottom, and a row
of keys under it. They type, select with Shift+arrows, cut and paste with Ctrl+X/C/V, undo with
Ctrl+Z, find with Ctrl+F, save with Ctrl+S and leave with Ctrl+Q, which asks `Save changes to
notes.txt?` when needed; the shell's screen comes back as it was. It works on an 80x24 serial
console and on a 200x60 terminal alike, in colour on xterm and the EggOS terminal and in plain
inverse video on anything else. The checks are section 9's host tests on the frame bytes and one
serial QEMU run.

## 1. When the terminal mode runs

- `edit [--] [<path>]` keeps its shape (D149). With a desktop it opens the window and returns,
  as today. **With no desktop and a terminal** on standard input and output (`TCGETS` succeeds
  on both) it runs the terminal mode in the foreground until the person quits, exit status 0.
- **`edit -t <path>` and `edit --terminal <path>`** run the terminal mode even on the desktop
  (in a terminal window, over `terminal -e`, or in a QEMU test), as `nano` would; `-t` with
  no terminal refuses (section 7). `parse` accepts `-t` and `--terminal` before the path.
- **No terminal and no desktop**: the refusal of section 7, exit status 1. `--help` prints
  `usage: edit [-t|--terminal] [--] [<path>]`.
- **Before the screen is taken**, the path is checked as today (`file::check`): a folder, a
  non-text file, one over 4 MiB or an unreadable one is refused on standard error with the
  editor's words, `edit: notes.bin is not a text file.`, exit 1, and nothing is drawn. A missing
  path is a new file named by it (`New file` in the status row); no path is `Untitled`.
- **Taking the screen**: the settings are read (`TCGETS`) and kept; raw mode is set with
  `TCSETSW` (`ICANON`, `ECHO`, `ISIG`, `IEXTEN`, `IXON`, `ICRNL` and `OPOST` off, `VMIN 1`,
  `VTIME 0`), so Ctrl+C, Ctrl+Z, Ctrl+S, Ctrl+Q and Ctrl+V reach the editor as keys; then
  `CSI ? 1049 h` (the alternate screen), `CSI ? 2004 h` (bracketed paste), `CSI ? 25 h`.
  **Giving it back** (quit, hangup, a fatal error): `CSI 0 m`, `CSI ? 2004 l`, `CSI ? 1049 l`,
  `CSI ? 25 h`, then the kept settings with `TCSETSW`, so the shell's screen and prompt come back
  exactly (nano's and micro's habit; `edit.com` cleared the screen, which loses the person's
  command history from view).
- **Signals.** `SIGWINCH` relays out (section 5.6). `SIGHUP` and `SIGTERM` with unsaved changes
  write the document to `<path>.save` beside the file (`Untitled.save` in the working folder;
  `.save` appended to the name, nano's habit, so a dropped serial line loses nothing), then give
  the screen back and exit 1; without changes they exit 0. `SIGINT` and `SIGTSTP` never arrive
  (`ISIG` off): Ctrl+C is copy and Ctrl+Z is undo, as in the GUI.

## 2. The screen, row by row

| Row | Content | Look |
|-----|---------|------|
| 1 | the title, centred | inverse (`SGR 7`) over the whole row |
| 2 .. R - 2 | the text area: `R - 3` document rows | default colours, syntax colours per glyph |
| R - 1 | the status row, or the open prompt | inverse over the whole row |
| R | the key hints | key names inverse, the words plain |

At 80x24 the text area is rows 2 to 22, 21 lines. Rows past the document's end are blank (nano's
way; not vim's `~`). Nothing scrolls into the terminal's scrollback: every row is written in
place (`CSI r ; 1 H`, the row's cells, `CSI K`), and only the rows that changed since the last
frame, with the cursor hidden during the write (`CSI ? 25 l` ... `h`) and the whole frame in
one `write` of at most 64 KiB (several when larger), so a serial line at 115200 baud paints a
keystroke in a few ms and a full 80x24 frame in under 200 ms.

- **The title** is the GUI's: `<name> - Text Editor`, `*` first while the document has unsaved
  changes (`*notes.txt - Text Editor`), `<name>` the last path component or `Untitled`. It is
  centred (the title bar of a terminal window centres it too), with at least one blank cell on
  each side: when it is longer than `C - 2`, the suffix ` - Text Editor` is dropped; when
  `*<name>` is still longer, the name is end-cut to fit with `...`. With the keys list open
  (section 4.3) the title is `Keys - Text Editor`.
- **The text area.** Column 1 is the document's column `scroll_x + 1`, or the gutter's first
  cell when line numbers are on (section 5.2). A tab advances to the next multiple of 8 document
  columns and draws as spaces, as the GUI. A control character (U+0000-001F but tab, U+007F,
  U+0080-009F; a lone `\r` too) draws as `?` in one cell and stays itself in the document.
  Everything else is written as its UTF-8 and takes the cells section 5.4 says. The caret is the
  terminal's own cursor, placed on the caret's cell after every frame (`CSI r ; c H`); it blinks
  as the terminal blinks it. No overwrite mode.
- **Selection** draws its cells in `SGR 7` with the **default** colours (a token colour is reset
  before the inverse: a selected keyword is inverse plain, not inverse blue, so the fill is one
  solid band and the text keeps 14.3:1 on the EggOS terminal); a selected line break is one extra
  inverse cell after the line's last character, as the GUI's extra 8 px cell. The selection is
  never empty when shown.
- **The status row** (`R - 1`), inverse, one blank cell at each end:
  - left, from column 2: `Ln <line>, Col <column>`, both 1-based, the column in characters as the
    GUI counts them (a tab is one, a `?` is one, a wide character is one); with a selection,
    ` · <n> selected` follows (`·` U+00B7 on the EggOS terminal and xterm, `-` when monochrome,
    section 3.3), `<n>` characters with line breaks counted;
  - right, ending at column `C - 1`: `LF` or `CRLF`;
  - before it, two blank cells apart: the state, `Modified` while unsaved changes exist, else
    `New file` while the path does not exist yet, else nothing;
  - before that, two cells apart: the language name of `text-editor-syntax-v0.1.md` section 5
    (`Rust`, `C/C++`, `Python`, `JavaScript/TypeScript`, `Shell`, `JSON`, `TOML`, `YAML`,
    `Markdown`, `HTML/XML`, `CSS`, `INI`, `Plain Text`), shown whether colouring is on or off.
  - **Messages** (the GUI's notices, section 6.4) replace the left part until the next key.
  - When the row is too narrow (section 5.1) the parts go in this order: the language, the state,
    the line ending, ` · <n> selected`, `, Col <column>`; `Ln <line>` is the last to stay.
- **The key hints** (`R`): nano's notation, the key in inverse, a space, the word in plain, two
  blank cells between hints, from column 1:
  `^S Save  ^Q Quit  F1 Keys  ^F Find  ^R Replace  ^G Go to  ^Z Undo  ^Y Redo` (74 cells).
  Hints that do not fit are dropped from the right end, whole. `^S` means Ctrl+S; the keys list
  (F1) spells `Ctrl+S`. Cut, copy and paste are not hinted: every switcher knows them, and the
  row teaches what differs from their old editor (Quit, Find, Go to) and where the rest is (F1).
  While a prompt is open the row shows the prompt's keys (section 6).

## 3. Colours

### 3.1 The screen

| Part | SGR | On the EggOS terminal |
|------|-----|------------------------|
| text area, default text | none (`39`, `49`) | `term.fg` `#D7DEE8` on `term.bg` |
| title row, status row | `7` | `term.bg` on `term.fg`, 14.3:1 |
| hint keys (`^S`, `F1`, `Enter`, `Esc`, ...) | `7` | the same |
| hint words | none | `term.fg` |
| selection | `7` with `39`, `49` first | the same as the bars |
| gutter numbers, the `<` and `>` edge marks, the `2 of 3` result, the `Line 1-<N>` hint | `90` (bright black) | `#707C8E`, 4.6:1 |
| the caret line's gutter number | `39` | `term.fg` |
| messages in the status row | `7` (the row's) | nothing else: a message is words, not a colour |

The background is never set: the text area takes the terminal's own background, so a host
terminal with a light theme stays light and its 16 colours are what that theme chose for them
(nano's and micro's choice; a dark fill forced over a light theme is what makes `edit.com`'s
blue look wrong in a modern terminal). No bold, no underline, no 256-colour or truecolour
sequences: the desktop's bright colours **are** the syntax colours (section 3.2), so the EggOS
terminal shows the GUI's exact hex with plain SGR, and a 24-bit sequence would gain nothing
there and fight a host theme elsewhere.

### 3.2 Syntax colours, mapped to ANSI

`text-editor-syntax-v0.1.md` section 1, kind by kind; the hex is the EggOS terminal's value of
that ANSI colour (`desktop-v0.3.md`), identical to the GUI's, so a `.rs` file looks the same in
the window and on the console:

| Token kind | GUI colour | SGR | EggOS hex |
|---|---|---|---|
| Plain, Punctuation/Operator | `text` | `39` (default) | `#D7DEE8` |
| Function, Macro/Preprocessor | bright yellow | `93` | `#F2CC6D` |
| Type, Key, Link | bright cyan | `96` | `#72D3E0` |
| String | bright green | `92` | `#84D48A` |
| Number, Escape, Attribute | bright magenta | `95` | `#DA95E4` |
| Keyword, Tag, Heading | bright blue | `94` | `#7FB0F0` |
| Comment | `text.dim` | `90` (bright black) | `#707C8E` |

Plain text is the default foreground (`term.fg`), not `text` `#E6EAF0`: on a terminal the
default is the person's own, and the two are 1 % apart. The same highlighter (`highlight.rs`,
per-line scanner, re-scanned from the edit) feeds both front-ends; the terminal only emits an
SGR where the kind changes along a row, never per cell. Comments at bright black are 4.6:1 on
`term.bg`, the one kind at the floor, as in the GUI. **Alt+Y** toggles syntax colours (nano's
key), on by default; off, every glyph is default.

### 3.3 Monochrome

Colour is used when `TERM` starts with `xterm`, `linux`, `screen`, `tmux`, `rxvt`, `vt100`,
`vt220`, `ansi` or `eggos`, or contains `color`, and `NO_COLOR` is unset or empty. Otherwise
(`TERM` unset, `dumb`, anything else, or `NO_COLOR` set) the editor is **monochrome**: no
`30-37`/`90-97` at all, only `SGR 7` and `SGR 0`. Then the gutter, the edge marks and the find
result are plain, syntax colours are off (Alt+Y does nothing and the status row still names the
language), and `·` in the status row is `-` (a dumb terminal may lack U+00B7). Nothing is told
by colour alone in either mode: the selection and the bars are inverse, the edges are glyphs,
the caret is the cursor, and syntax colour is an extra on top of the characters (the syntax
brief's rule: a colour marks a role the reader could tell anyway).

## 4. Keys

Every key, what it does, and the bytes the editor reads for it. A key that does not apply does
nothing. Shift with a caret move extends the selection from the anchor (set at the caret when the
selection starts), as in the GUI.

### 4.1 The table

| Key | Does | Bytes read |
|-----|------|------------|
| Ctrl+S | save; on `Untitled`, the Save as prompt | `13` |
| Alt+S | the Save as prompt (Ctrl+Shift+S has no byte sequence) | `1B 73` |
| Ctrl+Q | quit; the `Save changes` prompt first when modified | `11` |
| Ctrl+O | the Open prompt (question first when modified) | `0F` |
| Ctrl+N | new document (question first) | `0E` |
| Ctrl+Z | undo | `1A` |
| Ctrl+Y | redo (Ctrl+Shift+Z has no sequence) | `19` |
| Ctrl+X, Ctrl+C, Ctrl+V | cut, copy, paste; nothing without a selection or with an empty clipboard | `18`, `03`, `16` |
| Ctrl+A | select all | `01` |
| Ctrl+F | the Find prompt, with its last text | `06` |
| F3, Shift+F3 | find next, find previous (opens the prompt when there is no text yet) | `ESC O R`, `ESC [ 13 ~`, `ESC [ [ C`; Shift: `ESC [ 1 ; 2 R`, `ESC [ 13 ; 2 ~` |
| Ctrl+R | the Replace prompts | `12` |
| Ctrl+G | the Go to line prompt | `07` |
| Ctrl+^ (Ctrl+6), Ctrl+Space | **mark**: start selecting from the caret, until the next mark key, Esc or an edit (nano's `^^`; the fallback for Shift+arrows a console cannot send) | `1E`, `00` |
| Left, Right, Up, Down | one character or line; Up and Down keep the wanted column | `ESC [ A`-`D`, `ESC O A`-`D` |
| Shift+arrows | extend the selection | `ESC [ 1 ; 2 A` ... |
| Ctrl+Left, Ctrl+Right, Alt+Left, Alt+Right | previous word's start, next word's start (the GUI's words) | `ESC [ 1 ; 5 D`, `ESC [ 1 ; 3 D`, `ESC ESC [ D` |
| Home, End | the line's start and end | `ESC [ H`, `ESC [ F`, `ESC O H`, `ESC O F`, `ESC [ 1 ~`, `ESC [ 4 ~`, `ESC [ 7 ~`, `ESC [ 8 ~` |
| Ctrl+Home, Ctrl+End, Alt+Home, Alt+End | the document's start and end (Alt for consoles that send no Ctrl with Home) | `ESC [ 1 ; 5 H`, `ESC [ 1 ; 3 H`, `ESC ESC [ H` ... |
| PageUp, PageDown | caret and view by the text area's rows minus one | `ESC [ 5 ~`, `ESC [ 6 ~` |
| Shift + Home, End, Page keys, Ctrl+Home, Ctrl+End | the move, extending the selection | `ESC [ 1 ; 2 H`, `ESC [ 5 ; 2 ~`, `ESC [ 1 ; 6 H` ... |
| Backspace | delete the selection, else the character before the caret | `7F`, `08` (Ctrl+H on terminals that send it) |
| Delete | delete the selection, else the character after | `ESC [ 3 ~` |
| Tab | insert a tab (replacing the selection) | `09` |
| Shift+Tab | nothing | `ESC [ Z` |
| Enter | insert a line break, no auto-indent | `0D`, `0A` |
| Esc | close the keys list or the prompt; else clear the selection and the mark; else nothing | `1B` alone |
| F1 | the keys list (4.3) | `ESC O P`, `ESC [ 11 ~`, `ESC [ [ A` |
| Ctrl+L | redraw the whole screen (noise on a serial line) | `0C` |
| Alt+N | line numbers on or off (nano's key) | `1B 6E` |
| Alt+Y | syntax colours on or off (nano's key) | `1B 79` |
| Insert, other F keys, other Ctrl letters, Alt+other | nothing | |
| a printable character | insert it, replacing the selection | its UTF-8 |
| a paste | insert it as one undo step (4.2) | `ESC [ 200 ~` ... `ESC [ 201 ~` |

The modified forms follow xterm, `m = 1 + Shift·1 + Alt·2 + Ctrl·4`: `ESC [ 1 ; m X` for arrows,
Home and End, `ESC [ n ; m ~` for the others; `ESC O x` is the application-cursor form. Both
Enter bytes are accepted (`ICRNL` is off, so the Enter key is `0D`; a paste may carry `0A`).
Ctrl+W, Ctrl+K and Ctrl+U do nothing in the text: nano's exit, cut and paste keys are taken by
the Ctrl+Q/X/V family the hint row names, and a nano hand that presses Ctrl+X with nothing
selected loses nothing. Ctrl+Shift+anything has no sequence on a VT100 stream, hence Alt+S for
Save as and Ctrl+Y for redo (both are Microsoft Edit's and micro's).

### 4.2 Reading keys

- **Esc versus Alt and sequences.** `1B` followed by more bytes **in the same read** is a
  sequence or an Alt chord; `1B` alone in its read is Esc, confirmed by waiting 50 ms (`poll` on
  standard input) for nothing more. A person cannot type two keys in 50 ms; a serial line
  delivers a sequence in one or two reads well inside it. `ESC ESC [ D` is Alt+Left (Alt as an
  Esc prefix, the EggOS terminal's and xterm's form).
- **A sequence the table does not list** is read to its final byte (`0x40-0x7E` after `CSI`,
  one byte after `ESC O`) and ignored; a lone byte below `0x20` not in the table is ignored.
- **A paste** between `ESC [ 200 ~` and `ESC [ 201 ~` is inserted at the caret in one undo
  step, replacing the selection, with `0D` and `0D 0A` turned into line breaks and every other
  byte kept (the terminal already strips sequences, `terminal-selection-v0.3.md` 1.3); it is cut
  at the document's room with the `The document is full (4 MB).` message. A terminal without
  bracketed paste types the paste key by key, which also works, one undo step per line.
- **The clipboard.** Cut and Copy fill the editor's own clipboard (D149's 4 MiB) and, when
  `/dev/window` opens, the shared one too; Paste takes the shared one when it is newer, else the
  editor's (D197, the GUI's rule), so `edit -t` in a terminal window exchanges text with Files
  and the GUI editor, while the serial console keeps its own. The terminal's own paste (Ctrl+
  Shift+V, the middle button) arrives as the paste above.
- **Key repeat** is the terminal's; the editor coalesces a run of arrow keys already read into
  one frame (it drains standard input before drawing), so holding Down on a slow line never
  lags behind the drawing.

### 4.3 The keys list (F1)

The text area shows this table as plain text, the key spelled out (`Ctrl+S`, `Alt+N`, `F3`) in
a 16-cell column and the words after it, one row per key, title `Keys - Text Editor`, the
status row `Keys` and the hint row `Esc Back  Up Down PgUp PgDn Scroll`. Up, Down, PageUp,
PageDown, Home and End scroll it; Esc, F1 and Ctrl+Q return to the document as it was. No
help text beyond the table: the GUI has none either, and the table is the whole editor.

## 5. Sizes, long lines, tabs, wide characters, resizing

### 5.1 Small terminals

| Rows | Layout |
|------|--------|
| `R >= 4` | title, `R - 3` text rows, status, hints |
| `R = 3` | title, 1 text row, status (no hints) |
| `R = 2` | 1 text row, status |
| `R = 1` | 1 text row; a prompt takes the row while it is open, hiding the text |

Columns: the status row drops its parts as section 2 says; the hint row drops hints from the
right; the title drops its suffix and then cuts the name; the text area always has at least one
cell (the gutter is turned off below `C = 2 + d + 8`, so eight columns of text remain). The
editor never refuses a size: at 1x1 it still edits, one character at a time, which is what a
broken `stty` leaves and what a person fixes with `stty cols 80 rows 24` from another console.

### 5.2 Line numbers

Off by default (nano's and Microsoft Edit's default; 80 columns are scarce), Alt+N toggles them
per run, not saved. On, the gutter is `d + 1` cells, `d` the digits of the line count and at
least 2 (3 cells up to 99 lines, 4 up to 999, as the GUI's `16 + 8 d` px): the number
right-aligned in `d` cells, bright black, the caret's line's number in the default colour, one
blank cell, then the text. It widens only when `d` changes. Nothing is drawn between gutter and
text.

### 5.3 Long lines: horizontal scroll, no wrap

No word wrap in v0.1 (the GUI's 17.2 wrap is a later slice here too). The view holds `scroll_x`
document columns off the left. When the caret leaves the view to the right, `scroll_x` becomes
`caret_column - width + 1 + width / 4` (the caret lands a quarter of the width from the right
edge, so typing a long line redraws every quarter width, not every column); to the left,
`caret_column - width / 4`, clamped at 0; `width` is the text cells of a row. A row whose line
continues past the right edge shows **`>`** in the last cell, bright black, and a row scrolled
past its start shows **`<`** in the first text cell (less's and Microsoft Edit's marks; vim's
`$` means end-of-line elsewhere). The marks replace the character that would be there; the
caret is never placed on a mark (the scroll rule above keeps it a quarter width inside).

### 5.4 Tabs and wide characters

- A **tab** spans to the next multiple of 8 document columns, drawn as blanks; `Col` counts it
  as one character, as the GUI.
- **Cells per character**: ASCII and every character the terminal draws in one cell take one;
  on the **EggOS terminal** (`TERM_PROGRAM=EggTerminal`) every character takes **one** cell,
  since it draws one cell per codepoint and `?` for what its font lacks (`terminal-v0.3.md`).
  On any other terminal, a character in the **wide** ranges takes **two** cells and a
  **combining** mark takes **none**, drawn with its base. The ranges are our own table in
  `columns.rs` (East Asian Wide and Fullwidth, as the Unicode `EastAsianWidth.txt` data lists
  them, learned from the public data file, not from any program's code): wide `U+1100-115F`,
  `U+2E80-303E`, `U+3041-33FF`, `U+3400-4DBF`, `U+4E00-9FFF`, `U+A000-A4CF`, `U+AC00-D7A3`,
  `U+F900-FAFF`, `U+FE30-FE4F`, `U+FF00-FF60`, `U+FFE0-FFE6`, `U+1F300-1F64F`,
  `U+1F900-1F9FF`, `U+20000-2FFFD`, `U+30000-3FFFD`; combining `U+0300-036F`, `U+1AB0-1AFF`,
  `U+1DC0-1DFF`, `U+20D0-20FF`, `U+FE20-FE2F`.
- A wide character that would not fit whole at the right edge (its second cell is the `>`
  mark's or beyond) or at the left edge (its first cell is off the view) is not drawn: its
  visible cell is blank. `scroll_x` counts document columns (one per character), so the view's
  width in characters is what fits in cells, computed per row.
- Every row is positioned absolutely before it is written and the caret's cell is computed from
  the table, never from where the terminal left its cursor, so a terminal that disagrees with
  the table (an old one with no combining support) misplaces at most that row's caret, never
  the layout.

### 5.5 A terminal that reports no size

The size is taken in this order: `TIOCGWINSZ` on standard output when it gives non-zero rows
and columns (the console reports zero until something sets it, D105); else `LINES` and
`COLUMNS` from the environment when both parse; else the terminal is asked once, `CSI 999 ; 999
H` then `CSI 6 n`, and its reply `ESC [ r ; c R` is awaited for 200 ms (`poll`); else
**80x24**, the serial console's size and the VT100's. The DSR reply arrives on standard input
among keys and is taken out by the key reader (it is `CSI ... R` with two numbers; F3's
`ESC O R` and `ESC [ 1 ; 2 R` differ). A later `SIGWINCH` re-reads `TIOCGWINSZ` only.

### 5.6 Resizing (`SIGWINCH`)

The handler sets a flag; the blocked `read` returns `EINTR`, the loop re-reads the size, lays
the rows out again, keeps the caret visible with the rules of 5.3 and the GUI's vertical rule
(`scroll_y` moves the least that shows the caret's row), and redraws every row. Shrinking never
loses text (the document is the state; the screen is a view of it), and widening shows more of
each line, so a terminal window dragged wider while `edit -t` runs behaves as a resized editor
window does. Below 1x1 nothing is drawn until the next size.

## 6. Prompts, in the status row

A prompt replaces the status row's content (row `R - 1`, still inverse) and the hint row shows
its keys. One prompt at a time; while one is open the text area is drawn as it was and ignores
keys. A field is edited inline from after its label to column `C - 1`, the caret the terminal's
cursor; the field scrolls by itself when longer than the room, the way the GUI's field does;
Left, Right, Home, End, Backspace and Delete edit it, Ctrl+U clears it (the shell's habit), Tab
does nothing (no completion in v0.1), printable characters type. **Esc and Ctrl+C** cancel every
prompt (nano's `^C Cancel`, so a nano hand's cancel works), **Enter** is the primary action.
`<name>` is the document's name, `<folder>` the folder's full path; a row longer than `C - 2`
is end-cut with `...`.

| Prompt | Status row (from column 2) | Hint row | Keys |
|--------|----------------------------|----------|------|
| unsaved changes on quit, New or Open | `Save changes to <name>?` | `Y Save  N Don't save  Esc Cancel` | `y`, `Y`, Enter: save (on `Untitled`, the Save as prompt first, the action following a successful save); `n`, `N`: carry the action out at once; Esc, Ctrl+C: leave everything as it was |
| Save as (Alt+S, Save on `Untitled`) | `Save as: <field>`, the field holding the document's full path or `/home/root/Untitled.txt`, caret at its end | `Enter Save  Esc Cancel` | Enter writes to the path (relative to the working folder); cancelling a save that answered the quit question cancels the quit |
| replace a file | `Replace <name>? A file with that name already exists in <folder>.` | `Y Replace  Esc Cancel` | `y`, `Y`, Enter: replace; `n`, `N`, Esc, Ctrl+C: back to the Save as prompt with its text |
| Open (Ctrl+O) | `Open: <field>`, the field holding the document's folder with a trailing `/` (`/home/root/` for `Untitled`) | `Enter Open  Esc Cancel` | Enter loads the path, replacing the document (the question first when modified, before the prompt opens); a refused file keeps the prompt open and shows the message in it for one key |
| Find (Ctrl+F, F3 with no text) | `Find: <field>`, or `Find (match case): <field>`; at the right end, ending at `C - 1`, the result in bright black: `<n> of <m>`, `<n> of <m>, from top`, `<n> of <m>, from end`, `Not found`, `999+` past 999, or nothing while the field is empty | `Enter Next  Down Next  Up Previous  Alt+C Match case  Esc Close` | every change searches forward from the selection's start, the hit becoming the selection scrolled into view; Enter and Down search from the selection's end, Up backwards from its start (Shift+Enter has no sequence; the arrows are free in a one-line field); Alt+C toggles match case (nano's key); Esc closes and keeps the selection; F3 and Shift+F3 outside the prompt search with the last text |
| Replace (Ctrl+R), step 1 | `Replace: <field>` (the find text, with the same result at the right) | `Enter Next  Esc Close` | Enter with text goes to step 2 |
| Replace, step 2 | `Replace with: <field>` | `Enter Replace  Esc Close` | Enter goes to step 3 at the first match from the caret (`Not found` in step 1's result and back to step 1 when there is none) |
| Replace, step 3 | `Replace this one?` with the match selected and scrolled into view; the result at the right, `<n> of <m>` | `Y Replace  N Skip  A All  Esc Stop` | `y`, `Y`, Enter: replace it and move to the next match, one undo step each; `n`, `N`: next match without replacing; `a`, `A`: every remaining match in one undo step, caret after the last; Esc, Ctrl+C: stop. When the matches run out, or after All, the prompt closes and the status row says `<N> replaced` (`1 replaced`, `0 replaced` after skipping all) until the next key |
| Go to line (Ctrl+G) | `Go to line: <field>`, then, two cells after the field's end, `Line 1-<N>` in bright black | `Enter Go  Esc Cancel` | only ASCII digits are typed, at most 7; Enter with `1..=N` puts the caret at that line's first column, clears the selection, centres the row in the text area and closes; empty or out of range keeps the prompt and the hint becomes `Enter a line from 1 to <N>` in the default colour |

The find and replace fields keep their text while the editor runs; Go to line opens empty.
Replace's three steps are nano's and `edit.com`'s shape (ask, then ask per match), the one
prompt row allowing no second field; its words are the GUI's buttons. `Match case` is the GUI's
checkbox, shown as the prompt's label because a one-row prompt has no room for a checkbox.

### 6.1 Messages

The GUI's notices (section 8 of its brief) are the status row's left part until the next key,
same words, no colour change: `<name> does not exist.`, `<name> is a folder.`, `<name> is not a
text file.`, `<name> is too large to open (over 4 MB).`, `Could not open <name> (<errno name>).`,
`<folder> does not exist.`, `Cannot save here: <folder> is read-only.`, `Could not save <name>
(<errno name>).`, `The document is full (4 MB).`, `<name> points to <target>, which cannot be
created.`, plus `<N> replaced` from the replace prompt. A successful save says nothing, as the
GUI: the `*` leaving the title and `Modified` leaving the status row are the feedback (micro's
`Saved` line was weighed and dropped so the two front-ends behave alike).

## 7. No terminal at all

When standard input or output is not a terminal (`TCGETS` fails) the editor cannot draw and
cannot be driven, so `edit` refuses before touching anything, on standard error, exit 1:

- with no desktop, today's `NO_DESKTOP` text with "and no terminal to edit in" added, so an
  agent's `run` or a script that calls `edit` with a pipe gets the same pointer as before:

  ```
  edit: there is no desktop to open the Text Editor on and no terminal to edit in; `cat` shows a file and `tee` writes one
  ```

- with `-t` or `--terminal` and a desktop:

  ```
  edit: there is no terminal to edit in; `cat` shows a file and `tee` writes one
  ```

`eggmcpd` runs commands with pipes and `TERM=dumb` (D174), so an agent's `edit` always takes the
first branch and never hangs waiting for keys; the MCP `edit_open` tool stays the window's.
`TERM=dumb` on a real terminal (someone's `export`) is not refused: it gets the monochrome mode
of 3.3, since every terminal a person can type on positions its cursor.

## 8. An 80x24 screen

Rows 1 and 23 are inverse; in row 24 the `^S`, `^Q`, `F1`, `^F`, `^R`, `^G`, `^Z` and `^Y` cells
are inverse. The cursor is on row 4, column 32 (after `name`). Line numbers off, `main.rs` edited
once, so the title has its `*` and the status row `Modified`; `fn`, `let` are bright blue,
`main`, `println!` bright yellow, `"world"` and `"Hello, {}!"` bright green.

```
 1|                             *main.rs - Text Editor                             |
 2|fn main() {                                                                     |
 3|    let name = "world";                                                         |
 4|    println!("Hello, {}!", name);                                               |
 5|}                                                                               |
 6|                                                                                |
 7|                                                                                |
 8|                                                                                |
 9|                                                                                |
10|                                                                                |
11|                                                                                |
12|                                                                                |
13|                                                                                |
14|                                                                                |
15|                                                                                |
16|                                                                                |
17|                                                                                |
18|                                                                                |
19|                                                                                |
20|                                                                                |
21|                                                                                |
22|                                                                                |
23| Ln 3, Col 32                                                Rust  Modified  LF |
24|^S Save  ^Q Quit  F1 Keys  ^F Find  ^R Replace  ^G Go to  ^Z Undo  ^Y Redo      |
```

The same screen's rows 23 and 24 while finding `name` with the second match selected, and
while quitting with unsaved changes:

```
23| Find: name                                                              2 of 2 |
24|Enter Next  Down Next  Up Previous  Alt+C Match case  Esc Close                 |

23| Save changes to main.rs?                                                       |
24|Y Save  N Don't save  Esc Cancel                                                |
```

Rows 2 to 5 with line numbers on (Alt+N; 4 lines, `d = 2`, a 3-cell gutter, `3` in the default
colour and the others bright black):

```
 2| 1 fn main() {                                                                  |
 3| 2     let name = "world";                                                      |
 4| 3     println!("Hello, {}!", name);                                            |
 5| 4 }                                                                            |
```

## 9. Checks

The terminal mode is pure state over the `Editor` and a small `Console` trait (bytes in, a
frame of bytes out, the size, the environment), as the window is pure state over `Files`, so
host tests feed key bytes and assert the frame's cells and SGR runs (a test-side parser of the
ANSI subset turns the frame into a grid, as the terminal crate's tests do). Host tests
(`userland/editor/tests/all`):

1. Layout: at 80x24 rows 1, 2-22, 23, 24 hold the title, text, status and hints; at 80x3 no
   hints; 80x2 no title; 80x1 text only and a prompt hides it; at 20x24 the status row keeps
   `Ln 3` and the hint row keeps `^S Save  ^Q Quit`; the title drops its suffix at 24 columns
   and cuts a 60-byte name with `...` at 40.
2. Title: `Untitled - Text Editor`, `*notes.txt - Text Editor` after one edit, centred with the
   computed padding; `Keys - Text Editor` while F1 is open.
3. Status: `Ln 1, Col 1`, ` · 5 selected`, `Rust  Modified  LF`, `New file`, `CRLF`, `Plain
   Text`; the drop order at 60, 40, 30 and 12 columns.
4. Keys: every row of 4.1's table, each listed byte form, produces its action; `ESC [ 1 ; 5 D`
   and `ESC ESC [ D` both move a word; `1B` alone after the 50 ms wait clears the selection;
   `ESC [ 99 z` is ignored; `ESC [ 12 ; 34 R` sets the size and is not a key.
5. Mark: Ctrl+^, three Rights, a cut leaves the clipboard holding three characters; Esc clears
   the mark; Shift+Right extends without the mark.
6. Colours: the SGR runs of section 4 of the syntax brief's Rust lines; a selected keyword is
   `SGR 7` after `39`; `TERM=dumb`, unset `TERM` and `NO_COLOR=1` emit no `3x`/`9x` and keep
   `SGR 7`; `TERM=xterm-256color` emits only the 16-colour codes.
7. Long lines: a 100-column line at 80 columns shows `>` in cell 80; End scrolls so the caret is
   20 cells from the right edge and cell 1 shows `<`; Home brings `scroll_x` back to 0.
8. Widths: `日本` takes four cells with `TERM=xterm` and two with `TERM_PROGRAM=EggTerminal`;
   `e` + U+0301 takes one cell on xterm and two on EggTerminal; `Col` counts characters in both;
   a wide character split by the edge leaves a blank cell.
9. Size: `TIOCGWINSZ` 0x0 with `LINES=30 COLUMNS=100` lays out 30x100; with neither, the DSR
   reply `ESC [ 24 ; 80 R` sets 24x80; no reply within 200 ms gives 80x24; `SIGWINCH` to 40x100
   redraws every row and keeps the caret visible.
10. Prompts: the three answers to `Save changes to <name>?` and Enter as Save; Save on
    `Untitled` goes through `Save as:` and Esc there cancels the quit; `Replace <name>?` on an
    existing path, `n` returning to the field; `Find:` with `2 of 3`, `, from top`, `, from end`,
    `Not found`, Up and Down; Alt+C turning the label into `Find (match case):`; replace's
    `y`, `n`, `a` and `3 replaced`, one Ctrl+Z undoing an All; `Go to line:` 0, N + 1 and empty
    giving `Enter a line from 1 to <N>`, 7 centring line 7; Ctrl+C cancelling each prompt.
11. Paste: `ESC [ 200 ~ a \r\n b \t c ESC [ 201 ~` inserts `a\nb\tc` as one undo step; the same
    bytes without the brackets insert the same text in two steps.
12. Start and end: the first frame starts with `CSI ? 1049 h`, `CSI ? 2004 h`; quitting ends
    with `CSI 0 m`, `CSI ? 2004 l`, `CSI ? 1049 l`, `CSI ? 25 h` and the kept settings;
    `SIGHUP` with changes writes `<path>.save` with the document's bytes and line endings.
13. Refusals: a pipe on standard input gives section 7's first string with no desktop and the
    second with `-t`; `edit -t notes.bin` on a terminal prints `edit: notes.bin is not a text
    file.` before any sequence; `parse` takes `-t`, `--terminal`, `-t --`, and rejects `-x`.
14. The keys list: F1 fills the text area with `Ctrl+S` at column 1 and `Save` at column 17 of
    row 2, PageDown scrolls it, Esc restores the document's frame unchanged.

QEMU (serial, `-display none`): `edit notes.txt` typed at the console prompt shows the title row
and hint row in the serial capture within 200 ms; `hello`, Ctrl+S, Ctrl+Q typed as bytes end
with `CSI ? 1049 l` and the prompt back; `cat notes.txt` prints `hello`.

## 10. Later

Word wrap (the GUI's 17.2); mouse reporting (`CSI ? 1000 h`, clicks and drags; the EggOS
terminal reports none yet); a file picker in the Open and Save as prompts (Tab completion of
paths); the editor's settings (line numbers and syntax colours remembered); several documents
in one run; 256-colour and truecolour output when a terminal's own 16 are too far from the
desktop's; East Asian ambiguous-width characters as a setting.
