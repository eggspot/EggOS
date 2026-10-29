# EggOS v0.3.0 Text Editor: design brief (item 46)

Design by Claude Fable (D16) for roadmap item 46: a native GUI editor, `/bin/text-editor`, that
Files uses for text files, replacing the terminal `edit` of old item 24. It is built on the
toolkit (`toolkit-v0.3.md`) with the vocabulary of Files (`desktop-v0.3-gui.md` sections 6 and
8.1): one `Normal` window per document, a 32 px toolbar, a 24 px status bar, dialogs as the
in-window panel, errors as the in-window notice, the context menu. Everything there stands
unless this brief says otherwise. Numbers are px at 1x and double at 2x; 1 px strokes, no
anti-aliasing, no alpha, no motion but the caret's blink. The editing text is Egg Mono 8x16
(one column = 8 px, one row = 16 px); every other text is Egg Sans 16. The feel to match is
Notepad, TextEdit (plain text) and gedit: open, type, save, find, and nothing in the way.

## 1. Window, process and title

- **Process.** `/bin/text-editor [<path>]`, one window per process, one document per window.
  Several can run at once (Files starts one per file it opens; opening the same file twice
  gives two windows, a listed 0.3.0 limit). Started with no path it holds an empty document
  named `Untitled`. Started with a path that does not exist, it holds an empty document with
  that name, the status bar says `New file`, and the first Save creates it (as vi, nano and
  gedit do). A path that exists is loaded as section 5 says; one that cannot be loaded leaves
  the window open on `Untitled` and shows the notice (section 8).
- **Window.** A resizable `Normal` window, default content **720x480**, minimum **480x300**
  (Files' sizes), placed by the cascade. No `FIXED_SIZE`, no `TAKES_CTRL_KEYS`: Ctrl+W (Windows
  and Linux profiles), Cmd+W (macOS), Alt+F4 and the close control reach the editor as a
  `CloseRequest`, which section 8 answers.
- **Title.** `<name> - Text Editor`, where `<name>` is the file's name (the last path
  component, not the folder) or `Untitled`. While the document has unsaved changes the title
  starts with `*`: `*notes.txt - Text Editor` (Notepad's and gedit's marker; the window
  controls carry no dot). The suffix ` - Text Editor` is how the shell knows the window
  (section 13). The title is at most 64 bytes: the suffix is 14 and the marker 1, so a name
  longer than 49 bytes is cut to its first 46 bytes, backed off to a character boundary, plus
  `...` (so `*` + 46 + `...` + suffix = 64). The title bar's own `...` cut applies on top when
  the window is narrow.

## 2. Layout

Content coordinates, width `W`, height `H`. Three or four horizontal bands:

| Band | Rows | Fill | Notes |
|------|------|------|-------|
| Toolbar | `0..31` | `bar.bg`, `bar.edge` line at row 31 | section 3 |
| Bar slot | `32..63`, only while a bar is open | `bar.bg`, `bar.edge` line at row 63 | the find bar (section 6) or the path bar (section 7); never both |
| Editing area | from `32` (or `64`) to `H - 25` | gutter `menu.bg`, text view `field.bg` | section 4 |
| Status bar | `H - 24 .. H - 1` | `bar.bg`, `bar.edge` on its top row | section 9 |

`area` below is the editing area's rect; `G` is the gutter's width. The text view is
`x G .. W - 1` of the area. Panels centre in `area` (as Files centres them between its toolbar
and status bar); notices centre on the text view's width, bottom at the status bar's top - 16.

## 3. Toolbar

Five flat text buttons, 64x28 at y 2, pitch 68, left to right: `New` at x 4, `Open` at 72,
`Save` at 140, `Save As` at 208, `Find` at 276 (`Save As` with no `...`: it opens a bar, not a
window). Look: no fill at rest, `bar.hover` with `INSET_R4` while hovered, `bar.pressed` while
pressed (Files' toolbar buttons, with words instead of glyphs); the label centred in `text`,
cell top at +6; never disabled (saving an unchanged document writes it again, harmlessly).
Pointer rules of toolkit section 3 (press captures, release inside acts). They are not Tab
stops (the keys of section 10 are theirs). Nothing else on the toolbar; a wrap toggle is not in
0.3.0.

## 4. The text view

- **Gutter.** `x 0 .. G - 1` of the area, `menu.bg`, with `G = 16 + 8 * d`, `d` the number of
  digits in the line count, at least 2 (32 px up to 99 lines, 40 up to 999, ...). It changes
  width only when `d` changes. Line numbers in Egg Mono, 1-based, right-aligned ending at
  `G - 8`, on the same row as their line, `text.dim`; the caret's line's number in `text` (the
  only current-line cue; no row highlight). No line between gutter and text: the fill change
  is the edge.
- **Rows and columns.** Row `r` (0-based) has its top at `area.y + 4 + 16 r - scroll_y`;
  column `c` has its left at `G + 8 + 8 c - scroll_x`. Content height is `lines * 16 + 8`,
  content width `(longest_line_columns + 1) * 8 + 16`, where a tab counts as the columns it
  spans. Text in `text` on `field.bg` (16.0:1). A tab advances to the next multiple of 8
  columns and is drawn as spaces (the terminal's and Notepad's tab width). A character Egg
  Mono lacks (anything outside printable ASCII and its 32 extras, control characters included)
  draws as `?` but stays itself in the document; the column count counts it as one.
- **Caret.** A 1x16 `text` bar at the caret's column, full row height, in the pixel column
  just before the glyph after it (so after the last character it is 1 px past that glyph's
  cell). Blinks 500 ms on, 500 off, restarted "on" by every edit, caret move and click; drawn
  only while the text view has focus and the window has it. No overwrite mode (Insert does
  nothing).
- **Selection.** The selected characters' cells filled `selection` (same value as
  `term.selection`), text in `text` (7.5:1). A selected line break is shown by one extra 8 px
  cell after the line's last character. The selection is an anchor and the caret; it is never
  empty when shown. Any edit replaces it. It is cleared by a plain caret move or click, kept by
  Shift+moves, by the find bar (section 6) and when focus leaves the window.
- **Keeping the caret visible.** After every caret move or edit: if the caret's row is above
  the view, `scroll_y` becomes its row's top; if below, the row's bottom becomes the view's
  bottom; the same for columns with an 8 px margin on both sides. `scroll_y` and `scroll_x` are
  clamped to `0 .. content - view`.
- **Scroll bars.** The toolkit's `scroll_bar` look: 8 px, track `progress.track`, thumb
  `bar.pressed` with `INSET_R4`, at least 24 long, `button.inactive` while hovered or dragged.
  Vertical at the text view's right edge (`x W - 8 .. W - 1`), horizontal at its bottom (from
  `x G`), each only while its content is larger than the view; when both show, each stops 8 px
  short of the corner and the 8x8 corner is `progress.track`. A click on a track scrolls by the
  view's length minus 24 towards the click; dragging the thumb scrolls. Rows are laid out in
  the full width and height regardless of the bars (the bars cover the padding, not text, in
  the common case; a line's last 8 px can sit under the vertical bar, as in the terminal).
- **Wheel.** One notch scrolls 48 px (3 rows) vertically; with Shift held, 24 px (3 columns)
  horizontally. The caret does not move.
- **Pointer.** `IBeam` over the text view, `Arrow` over the gutter, the bars and the scroll
  bars.

## 5. The document

- **Buffer.** UTF-8 text with `\n` line breaks, at most **4 MiB** (4 194 304 bytes). A file
  larger than that is refused before it is read (`<name> is too large to open (over 4 MB).`).
  An edit that would pass the limit (typing, paste, redo) is ignored and the notice `The
  document is full (4 MB).` shows. The implementer bounds the line count and line length by
  the same 4 MiB; nothing else is bounded.
- **What opens.** The whole file is read and must be valid UTF-8 with no NUL byte, the test
  Files applies to its first 1 KiB (`kind.rs`) applied to all of it; otherwise it is refused
  with `<name> is not a text file.` (gedit's refusal, not Notepad's garbage; no read-only
  mode in 0.3.0). A folder: `<name> is a folder.`
- **Line endings.** On load, if the text holds `\r\n` anywhere, the document is **CRLF**: every
  `\r\n` becomes `\n` in the buffer and the status bar says `CRLF`; otherwise it is **LF**. A
  lone `\r` stays in the text (drawn `?`). On save, a CRLF document writes every `\n` as
  `\r\n`, so a Windows file stays a Windows file. New documents are LF. No way to change it in
  0.3.0.
- **Byte-order mark.** A leading `EF BB BF` is kept out of the buffer and written back on
  save; nothing shows it.
- **Saving.** The text is written to the path in one go (create or truncate, 64 KiB per
  write). Success clears the modified state, the `*` and `New file`; nothing else is shown
  (the title and status bar changing are the feedback, as in Notepad and gedit; no `Saved.`
  notice). Failure keeps the document modified and shows a notice (section 8).
- **Undo and redo.** A list of steps, each the text removed and inserted and the caret and
  anchor before it. Typing printable characters coalesces into one step, as does a run of
  Backspace or Delete; Enter, Tab, a paste, a cut, a replace by find and any caret move by
  key or click end the open step. At most 1024 steps; the oldest are dropped. A new edit
  clears the redo list. Undo and redo restore the caret and selection of before.
- **Clipboard.** One text, held by the editor process (no system clipboard in 0.3.0, as in
  Files). Cut and Copy replace it with the selection; Paste inserts it at the caret, replacing
  the selection. Cut, Copy with no selection and Paste with an empty clipboard do nothing (the
  menu dims them).
- **Words**, for Ctrl+arrows and double-click: a maximal run of ASCII letters, digits and `_`,
  or of characters beyond ASCII; every other non-space character is a word of its own; a run
  of spaces and tabs is not a word but is what double-click selects when clicked.

## 6. The find bar

Opened by `Find`, Mod+F, F3 or Mod+G (section 10) in the bar slot (rows 32..63); it replaces an
open path bar. Contents, y 34 unless said, left to right:

| Part | Rect | Look |
|------|------|------|
| field | `(12, 34)`, 180x28 | toolkit field, placeholder `Find`, keeps its text while the window lives |
| `<` previous | `(200, 34)`, 28x28 | flat glyph button as Files' `<`, `text.dim` and inert while the field is empty |
| `>` next | `(232, 34)`, 28x28 | the same |
| `Match case` | control at x 272, 28 tall; mark 16x16 at `(272, 40)`, label at x 296 | toolkit checkbox, unchecked by default |
| result | right-aligned ending at `W - 44`, cell top 40 | `text.dim`; empty, `<n> of <m>`, `<n> of <m>, from top`, `<n> of <m>, from end` or `Not found`; `<m>` reads `999+` past 999; end-ellipsised in the room from x 360 to `W - 44` |
| `×` close | `(W - 36, 34)`, 28x28 | flat glyph button, the `×` is U+00D7 |

- **Searching.** The search text is the field's bytes; with `Match case` unchecked ASCII
  letters match either case, every other byte matches itself. An empty field searches nothing
  and the result is empty. Every change of the field or of `Match case` searches forward from
  the selection's start (or the caret), so typing more letters keeps the same match; a hit
  becomes the selection (anchor at its start, caret at its end), scrolled into view, and the
  result reads `<n> of <m>` (its 1-based index among all matches). `>`, Enter in the field,
  F3 and Mod+G search forward from the selection's end; `<`, Shift+Enter, Shift+F3 and
  Mod+Shift+G search backwards from the selection's start. Matches never overlap.
- **Wrapping.** Forward with nothing after the start, the search continues from the document's
  top and the result reads `<n> of <m>, from top`; backwards, from the end, `<n> of <m>, from
  end`. With no match anywhere the result reads `Not found` and the selection and view stay.
- **Focus and keys.** Opening the bar focuses its field with the caret at the end of its text
  (there is no field selection to replace; Backspace edits, Esc with text in the field closes
  the bar, its `escape` is irrelevant). Tab moves between the field and `Match case`, in both
  directions; `<`, `>` and `×` are not Tab stops. Esc anywhere in the window closes the bar,
  returns focus to the text and keeps the selection. Clicking in the text focuses it and leaves
  the bar open, as in Firefox; F3 and Mod+G still search. F3 or Mod+G with the bar closed opens
  it with its last text, searches once, and leaves the focus in the text; with no text yet it
  only opens and focuses the field.

## 7. The path bar: Open and Save As

There is no file picker in 0.3.0; opening and saving by path use one bar in the slot:

| Part | Rect | Look |
|------|------|------|
| label | `(12, 40)` | `Open:` or `Save as:` in `text` |
| field | from x 80 to `W - 152`, y 34, 28 tall | toolkit field, no placeholder |
| action | `(W - 140, 34)`, 64x28 | flat text button `Open` or `Save` |
| `Cancel` | `(W - 72, 34)`, 64x28 | flat text button |

- **Open** (`Open`, Mod+O): the field holds the document's folder with a trailing `/`
  (`/home/root/` for `Untitled`), caret at the end; the user types the name. Enter or `Open`
  resolves a relative path against that folder, loads the file as section 5 says, replacing
  the document (after the unsaved-changes question of section 8 when it is modified; the
  question comes first, before the bar opens), and closes the bar. One window holds one
  document, so Open and New replace it, as classic Notepad does; a second file wants a second
  window (Files gives one per file).
- **Save As** (`Save As`, Mod+Shift+S, and Save on `Untitled`): the field holds the document's
  full path, or `/home/root/Untitled.txt`, caret at the end. Enter or `Save` writes to the
  path; when a file exists there and it is not the document's own path, the panel `Replace
  <name>?` asks first. On success the document takes the new path and name (title, gutter
  and status bar follow) and the bar closes.
- **Errors** keep the bar open with its text and show a notice (section 8), as rename in place
  does. Esc and `Cancel` close the bar; when the save was the answer to a close request
  (section 8), cancelling it cancels the close too.
- Enter in the field is the action, Tab does nothing, Esc closes. The field is the only focus
  stop in this bar.

## 8. Panels and notices (every string)

**Panels** are the toolkit `Panel` centred in `area`, 400 wide, as Files' dialogs: the first
line `text`, the rest `text.dim`, buttons right-aligned 8 apart, the primary last; Esc is the
cancel button, Enter the primary, Tab moves between buttons. While one shows, the text ignores
pointer, wheel and keys and draws unfocused.

| When | Line 1 | Line 2 | Buttons (left to right) |
|------|--------|--------|-------------------------|
| close, New or Open with unsaved changes | `Save changes to <name>?` | `Your changes will be lost if you don't save them.` | `Don't Save`, `Cancel`, `Save` (primary) |
| Save As onto an existing file | `Replace <name>?` | `A file with that name already exists in <folder>.` | `Cancel`, `Replace` (primary) |

The first panel's words are Notepad's and TextEdit's in every profile (gedit's `Close without
Saving` is not used: one wording, no profile variants, since nothing here is named
differently per platform the way the Trash is). `Save` saves and then carries out the
action (close, new, open); on `Untitled` it opens the Save As bar first, and the action
follows a successful save. `Don't Save` carries the action out at once. `Cancel` leaves
everything as it was. A `CloseRequest` on a clean document closes at once. `<name>` is the
document's name, `<folder>` the folder's full path; lines are end-ellipsised at 368 px.

**Notices** are Files' in-window notice (8.1 item 6): `menu.bg`, 1 px `menu.border`,
`INSET_R6`, the 2 px `field.bg` edge, 40 tall, width = text + 32 clamped to 160 .. text view
width - 32, text centred in `text`, glyph top at row 12; centred on the text view's width,
bottom at the status bar's top - 16; closed after 3000 ms, on any key or on a button-down
(delivered normally); a new one replaces it.

| Notice | When |
|--------|------|
| `<name> does not exist.` | Open of a path that is not there |
| `<name> is a folder.` | Open of a directory |
| `<name> is not a text file.` | not UTF-8, or holds NUL |
| `<name> is too large to open (over 4 MB).` | over 4 MiB |
| `Could not open <name> (<errno name>).` | any other open or read error |
| `<folder> does not exist.` | Save or Save As into a missing folder |
| `Cannot save here: <folder> is read-only.` | `EROFS`, `EACCES` or `EPERM` on save (the host share, `/Applications` without a disk) |
| `Could not save <name> (<errno name>).` | any other save error |
| `The document is full (4 MB).` | an edit that would pass the limit |

`<name>` is the last path component of what was typed or given; errno names as Files prints
them (`ENOENT`, `EISDIR`, ...).

## 9. The status bar

24 tall, `bar.bg`, `bar.edge` on its top row, Egg Sans in `text.dim`, cell top +4:

- Left, at x 12: `Ln <line>, Col <column>`, both 1-based, the column in characters from the
  line's start (a tab is one character, a character drawn as `?` is one). With a selection,
  ` · <n> selected` follows (`·` is U+00B7; `<n>` characters, line breaks counted).
- Right, ending at `W - 12`: `LF` or `CRLF`.
- Before it, ending 16 px before the line-ending word starts: the state, `Modified` while the
  document has unsaved changes, else `New file` while the document's path does not exist yet,
  else nothing.

The status bar never carries messages (notices do).

## 10. Keys

`Mod` is Ctrl or Cmd (the Super key); both are accepted in every profile, as Files does. Shift
with any caret move extends the selection from the anchor (set at the caret when the selection
starts). A key that does not apply does nothing.

| Key | Does |
|-----|------|
| Mod+N | new document (the question of section 8 first when modified) |
| Mod+O | the Open bar (question first) |
| Mod+S | save; on `Untitled`, the Save As bar |
| Mod+Shift+S | the Save As bar |
| Mod+F | the find bar, focused |
| F3, Mod+G | find next (opens the bar when closed) |
| Shift+F3, Mod+Shift+G | find previous |
| Mod+Z | undo |
| Mod+Shift+Z, Mod+Y | redo |
| Mod+X, Mod+C, Mod+V | cut, copy, paste |
| Mod+A | select all (anchor at the start, caret at the end) |
| Left, Right, Up, Down | one character or line; Up and Down keep the wanted column across shorter lines |
| Home, End, Cmd+Left, Cmd+Right | the line's start and end (Cmd+arrows are TextEdit's habit) |
| Ctrl+Left, Ctrl+Right, Alt+Left, Alt+Right | the previous word's start, the next word's start (Alt+arrows are TextEdit's) |
| Ctrl+Home, Ctrl+End, Cmd+Home, Cmd+End | the document's start and end (Cmd+Up and Cmd+Down are the window server's maximise and minimise and never reach the editor) |
| PageUp, PageDown | the caret and the view move by the view's rows minus one |
| Backspace, Delete | delete the selection, else the character before or after the caret |
| Tab | inserts a tab character (replacing the selection; no indent of several lines) |
| Enter | inserts a line break; no auto-indent (Notepad and TextEdit do none) |
| Esc | closes the context menu, else the open bar; otherwise nothing |
| Shift+F10, Menu | the context menu at the caret (section 12) |
| a printable character | inserts it, replacing the selection |

Ctrl+W, Cmd+W and Alt+F4 are the window server's close request. The keys of the bars and
panels are in sections 6 to 8. Shift+Tab and Tab never move focus between the toolbar and the
text: the text view is the window's one focus stop, plus a bar's stops while it is open.

## 11. Pointer

| Action | Does |
|--------|------|
| click in the text | places the caret at the nearest column boundary (the half-cell rule of fields), rows clamped to the last line, columns to the line's end; clears the selection; focuses the text |
| Shift+click | extends the selection from the anchor to there |
| drag from a click | selects from the press to the pointer; beyond the view's edges the view scrolls one row or column towards the pointer every 100 ms and the selection follows |
| double-click (400 ms, 4 px) | selects the word under the pointer, or the run of spaces and tabs |
| triple-click | selects the line, its line break included |
| click on a gutter number | selects that line; dragging in the gutter extends by whole lines |
| right-click in the text or gutter | the context menu at the pointer; a right-click outside the selection first places the caret there |
| wheel | section 4 |
| scroll bars | section 4 |

## 12. The context menu

The toolkit `Menu` (items 32 tall, text at x + 12, `menu.hover`, 1 px `menu.border`,
`INSET_R6`), opened at the pointer or, by Shift+F10 and the Menu key, at the caret's x with its
top at the caret's row's bottom; clamped 8 px inside the content. `─` is a separator, `[dim]`
the condition that greys an item:

`Undo` [dim: nothing to undo] `Redo` [dim: nothing to redo] ─ `Cut` [dim: no selection] `Copy`
[dim: no selection] `Paste` [dim: empty clipboard] ─ `Select All` [dim: empty document]

Up/Down/Enter/Esc as the toolkit says; a button-down outside closes it and is delivered
normally; losing window focus closes it.

## 13. The shell: launcher, icon and Files

- **Catalogue.** A fourth built-in, `AppId::TextEditor`, name `Text Editor`, program
  `/bin/text-editor`, present only when that file is in the image (as Files is, D18); synonyms
  `Notepad`, `TextEdit`, `gedit`, `Kate`, `editor`, `notes`. The launcher lists it among the
  built-ins alphabetically (`Files`, `Settings`, `Terminal`, `Text Editor`), with the context
  menu of a built-in (`Open`); `app open "Text Editor"` opens it (D22).
- **Not pinned.** It is not on the dock, the Linux dash or the desktop: neither Notepad,
  TextEdit nor gedit is pinned by default on its own system, the dock's pinned row stays three
  wide, and the editor is reached from the launcher and from Files. A running or minimised
  editor window shows in the dock, the taskbar, the switcher and the launcher's `Minimised:`
  line like any app, with this icon.
- **Window matching.** `app_of` names a window whose title ends with ` - Text Editor` as the
  editor's (the three built-ins still match by exact title), so its icon, tooltip (the app's
  name on the dock, the window's title on the taskbar) and dock grouping are right.
- **Icon.** A drawn icon in the shape vocabulary of `icons.rs`: an off-white page with grey
  text lines and the yolk caret at the end of the last line, which says "a page being typed
  into" without a badge, a pencil or a second colour. Three colour tokens are added to the
  desktop's set with values already in use: `icon.page` `#F6F1E7` (the `shell` value),
  `icon.page.edge` `#C9D1DB` (the `icon.mac` value), `icon.page.line` `#4A5466` (the
  `button.inactive` value); the caret is `accent`. Rectangles are `x, y, w, h`:

  | Size | Shapes, in drawing order |
  |------|--------------------------|
  | 48x48 | `Tile(8,0,32,48, R4, icon.page, icon.page.edge)`; lines `Fill(14,9,20,2)`, `Fill(14,15,20,2)`, `Fill(14,21,20,2)`, `Fill(14,27,20,2)`, `Fill(14,33,10,2)` in `icon.page.line`; caret `Fill(26,30,2,8)` in `accent` |
  | 32x32 | `Tile(5,0,22,32, R4, icon.page, icon.page.edge)`; lines `Fill(9,6,14,2)`, `Fill(9,11,14,2)`, `Fill(9,16,14,2)`, `Fill(9,21,7,2)` in `icon.page.line`; caret `Fill(18,19,2,6)` in `accent` |

  The page is 32 wide on the 48 canvas so it reads as a portrait sheet, not a tile; the
  generic letter tile was rejected because `T` with no badge says nothing. Contrast of the
  lines on the page is 7.5:1, of the caret 1.9:1 (decoration, not text). Both sizes join the
  pinned hash table.
- **Files.** Open (double-click, Enter, the context menu's `Open`) on a `Kind::Text` file
  starts `/bin/text-editor <path>` as a child (as Files starts the installer's window) when
  `/bin/text-editor` exists, with no size limit on Files' side (the editor refuses over 4 MiB
  itself, so Files' 256 KiB terminal limit no longer applies); without the editor in the
  image, today's `cat` in the Terminal stays. `Properties` keeps saying `Text`. Files' roadmap
  entry and D135's "until the Text Editor" are satisfied by this.

## 14. CLI and MCP (D22)

The editor is the GUI face of editing text; the CLI faces of the same capability already
exist (`cat`, `tee`, the coreutils) and the MCP server's `cat` and `tee` are their tools.
`text-editor [<path>]` from eggsh opens the window (gedit's and `open -e`'s habit); `app open
"Text Editor"` and the MCP `app_open` open it by name; an agent operates it through
`wm_capture`, `wm_click`, `wm_type` and `wm_press` like any window (D143). No further CLI or
MCP surface is added by this item.

## 15. Tests that pin the look and the behaviour

Host tests on the editor's state (the window is pure state over a `Files` trait, as Files'
is): the title for `Untitled`, a name, a modified name and a 60-byte name cut to 46 + `...`;
a CRLF file loads as LF, saves as CRLF and keeps a BOM; a NUL and an invalid byte refuse with
their notices; 4 MiB + 1 refuses, 4 MiB opens, and one more character is `The document is
full (4 MB).`; find forward, backward, wrapping from top and end, `Match case`, `Not found`,
`999+`; undo coalesces typing and splits at Enter, redo clears on an edit; words for Ctrl+arrows
and double-click; the gutter width at 99, 100, 999 and 1000 lines; the status bar's `Ln`,
`Col` and `selected`; the close question's three answers, Save on `Untitled` going through the
Save As bar, and Cancel in that bar cancelling the close; `Replace <name>?`; the read-only
notice on `EROFS`. Expected frames (the QEMU GUI test): the empty `Untitled` window, a file
opened from Files with the find bar showing `2 of 3`, the close question, and the icon in the
launcher row and the dock while a window runs; then `cat` reads back what was typed and saved.

## 16. Not in 0.3.0

Word wrap and a wrap toggle; a file picker (the path bar stands in); a system clipboard
(section 5's is the window's); changing the line ending or the encoding; a read-only mode;
an auto-indent; indenting a selection with Tab; find and replace; go to line; a current-line
highlight; one window per file (opening a file twice gives two windows); reload when the file
changes on disk; printing; fonts, sizes and themes; syntax colouring; a `Saved.` notice; the
terminal `edit` (moved after the release).
