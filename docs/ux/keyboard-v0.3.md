# EggOS v0.3.0 Keyboard: design brief (batch part 33)

Design by Claude Fable (D16) for batch part 33 (D185), "Text beyond ASCII and keyboard layouts":
both fonts gain Latin-1 and the euro sign (`font-8x16-latin-1.txt` for Egg Mono, U+20AC appended
to `font-ui-16.txt` for Egg Sans), the keymap gains layouts with dead keys and a third level, and
Caps Lock lights its LED. This brief is the GUI front-end of the one capability `keyboard.layout`
(D22): the Settings `Keyboard` pane, what dead keys and the third level feel like, the words of
the CLI and MCP front-ends, and the notes on the glyphs. Everything in `toolkit-v0.3.md`
(section 11 for Settings) and `desktop-v0.3-gui.md` section 10 stands; numbers are px at 1x and
double at 2x; the only motion is the caret's blink.

## 1. The pane in the sidebar

- **Name** `Keyboard`, the third row as today (y 108..139 of the sidebar, text at x 16).
- **Search words**: `layout`, `keymap`, `language`, `input source`, `AltGr`, `dead keys`,
  `accents`, `Caps Lock`, `QWERTY`, `QWERTZ`, `AZERTY`, `international`, `Key repeat`,
  `shortcuts`, plus every layout's title from the layouts table (section 2), so `german`,
  `french` and `dvorak` find the pane. Not `Option` (`options` already matches every pane).
- **Budget**: the pane is its container plus 7 widgets (one under the limit of 8). Today's pane
  holds 3 (heading, `Layout`, `US`), so the window's tree grows by 4: `CAPACITY` 48 becomes 52
  and the installer's dialog `CAPACITY` stays equal to it.

## 2. The layouts, in the list's order

The pane, the CLI's error line, `settings list` and the kernel's keymap read one table of
`(id, title, has a third level, dead keys)`; nothing below is typed twice. The titles are what
the list shows; people coming from macOS ("U.S.", "British", "German"), Windows ("US",
"United Kingdom", "German") and Linux ("English (US)", "English (UK)", "German") all
recognise them. English first because `us` is the default and the system's own language, its
variants under it (the two arrangements last, below the ordinary keyboards), then the other
languages alphabetically by title. A layout the build drops leaves the order of the rest
unchanged; a layout it adds goes alphabetically among the other languages.

| # | Id | Title | Third level | Dead keys |
|---|----|-------|-------------|-----------|
| 1 | `us` | `English (US)` | no | none |
| 2 | `us-intl` | `English (US, international)` | yes | ´ ` ^ ¨ ~ |
| 3 | `gb` | `English (UK)` | yes | none |
| 4 | `us-dvorak` | `English (US, Dvorak)` | no | none |
| 5 | `us-colemak` | `English (US, Colemak)` | no | none |
| 6 | `dk` | `Danish` | yes | ´ ` ^ ¨ ~ |
| 7 | `fi` | `Finnish` | yes | ´ ` ^ ¨ ~ |
| 8 | `fr` | `French` | yes | ` ^ ¨ ~ |
| 9 | `de` | `German` | yes | ´ ` ^ |
| 10 | `it` | `Italian` | yes | none |
| 11 | `no` | `Norwegian` | yes | ´ ` ^ ¨ ~ |
| 12 | `br` | `Portuguese (Brazil)` | yes | ´ ` ^ ¨ ~ |
| 13 | `es` | `Spanish` | yes | ´ ` ^ ¨ |
| 14 | `se` | `Swedish` | yes | ´ ` ^ ¨ ~ |

`fi` and `se` are the same key table under two names, so a Finn finds `Finnish` under F and
`settings set keyboard.layout fi` works; if the build keeps one entry, its title is `Swedish and
Finnish` at row 14 and the id `se`. The third-level and dead-key columns are what Windows's
layouts of those names have; the pane's two lines (section 3) are computed from the table, so
if the build's tables differ the lines follow the tables, not this page. `English (US, Dvorak)`
and `English (US, Colemak)` rather than `Dvorak (US)`: they are arrangements of the US keyboard
and belong with it in the list, as macOS and Windows file them under English.

## 3. Layout (pane coordinates: origin window x 176, y 0; 464 wide at the default size, so the
right-aligned edge is x 448; usable height to y 332)

Two columns: the list of layouts on the left, the chosen layout's facts and a place to type on
the right.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Keyboard` |
| 2 | list | (16, 64), 208x240: 10 rows of 24 | the titles of section 2 in `text`; `>` at x + 8, text at x + 24 (`Kind::list(titles, 24, 8, 24)`); the scroll view's 8 px bar inside its right edge once there are more than 10 layouts |
| 3 | label | (240, 68) | the third-level line (below) in `text` |
| 4 | label | (240, 88) | the dead-keys line (below) in `text` |
| 5 | text field | (240, 120), 208x28 | placeholder `Type here to try it`, `escape: Clear` |
| 6 | label | (240, 156) | the keyboard line (below): `Caps Lock is on` in `text`, or `No keyboard attached.` in `text.dim`; hidden otherwise |
| 7 | label | (16, 316) | `No disk attached: this choice lasts until power-off.` in `text.dim`, shown only without a disk, exactly as Appearance's |

The right column starts 16 px after the list (x 240) and ends at 448, 208 wide like the list.
Its first line's cell top (68) is level with the list's first row text (64 + 4); the two fact
lines are body lines 20 apart; the field starts 16 below the second line's cell (104 + 16); the
keyboard line is 8 below the field, at y 156 like the Date & time pane's status line. The list
ends at y 304, 12 above the no-disk line, which keeps Appearance's y so the two panes say it in
the same place. The widest title, `English (US, international)`, is about 125 px, so it never
meets the bar (text from x 40 to at most 216).

**The third-level line** (widget 3), from the layout's table entry and the desktop profile:

| Layout has a third level | Profile | Text |
|--------------------------|---------|------|
| yes | Windows, Linux | `Right Alt (AltGr) types € and more` |
| yes | macOS | `Option types € and more` |
| no | any | `No third level, so no € key` |

(`€` is U+20AC, now in both fonts; about 166 px at its widest.) The line changes with the
profile the moment a profile is applied (Appearance or `settings set shell.profile`), like
Appearance's `Primary modifier` row.

**The dead-keys line** (widget 4): `Dead keys: ` followed by the layout's dead accents in the
fixed order ´ ` ^ ¨ ~ separated by spaces (`Dead keys: ´ ` ^ ¨ ~`, about 95 px), or `No dead
keys`. The spacing accents are U+00B4, U+0060, U+005E, U+00A8 and U+007E.

**The keyboard line** (widget 6) reports the keyboard itself: `Caps Lock is on` in `text` while
Caps Lock is on (it appears and disappears with the key, damaging only its rect, while the pane
is shown; the keyboard's LED follows the same state); `No keyboard attached.` in `text.dim`
when the machine has no keyboard device; hidden when there is a keyboard and Caps Lock is off.

## 4. Behaviour

- **The current layout** is the selected row when the window opens and whenever the pane is
  shown; the list scrolls to keep it visible. An open window follows a layout set from the CLI
  or MCP the next time it is focused, as it follows `shell.profile`.
- **Choosing**: a click, or Up/Down/Home/End/PageUp/PageDown while the list has focus, selects
  a row and that is the choice: the layout applies at once, everywhere (the console, every
  window, the compatibility layers), and `keyboard.layout` is saved to the settings file in the
  same step, instant and silent like Appearance's cards: no button, no dialog, no toast. Enter
  or a double-click on the selected row does nothing more. Applying is reversible and cheap, so
  browsing with the arrows is fine; the file is rewritten per step (at most 14 writes).
- **The kernel reads `keyboard.layout`** after mounting the disks, before the first prompt and
  before the desktop starts, like `shell.profile`, so the saved layout is in force from the
  first key.
- **Try it** (widget 5): an ordinary text field. It shows what the layout types, dead keys and
  third level included, in Egg Sans; its text is never saved or sent anywhere. Enter does
  nothing (the pane has no default button); Esc clears it; the text is kept while the user
  switches layouts (so two can be compared) and emptied when the pane is shown again. With no
  keyboard the field is disabled (placeholder in `text.dim`, no caret).
- **Focus**: Tab from the sidebar list enters the pane at the layouts list, then the field, then
  wraps to the search field. Showing the pane does not move the focus.
- **No disk**: the choice applies for this session and the no-disk line (widget 7) says it lasts
  until power-off; everything else is unchanged.
- **No keyboard**: the list still works with the pointer, the choice is saved and applies to the
  keyboard present at the next boot (there is no hot-plug), the keyboard line says `No keyboard
  attached.` and the field is disabled.
- **No display**: Settings has no window (the command exits 1 as today); the CLI and the MCP
  tool apply the layout to the console at once and save it.
- **Later**: `Key repeat` (its row goes under the list when it comes, which is why the list
  ends at 304 and not 328), a keyboard picture, more than one layout at a time with a switch
  key, per-user layouts, showing the pending accent at the caret.

## 5. Dead keys and the third level, as the user feels them

Dead keys and the third level live in the kernel's keymap, so the terminal, the toolkit's
fields, the Windows layer and every other program receive finished characters and nothing has
to be taught per program. A dead key's own press delivers a `Key` event whose character is 0,
which is why nothing shows.

- **Pressing a dead key shows nothing**: no caret change, no preview, no mark (Windows and
  Linux desktops show nothing either; macOS's highlighted accent is "later"). The accent is
  pending until the next key press, wherever that press lands; moving the keyboard focus does
  not cancel it in this release (a listed limit).
- **Dead key, then a letter it can accent** (the Latin-1 pairs: grave on a e i o u, acute on
  a e i o u y, circumflex on a e i o u, diaeresis on a e i o u y, tilde on a n o, and on
  `English (US, international)` the apostrophe on c for ç) types the accented letter. Shift, or
  Caps Lock, gives the accented capital (`´` then Shift+E is `É`). A pair whose result is
  outside Latin-1 (diaeresis + Y would be U+0178, which neither font has) is not a pair.
- **Dead key, then Space** types the accent alone: ´ (U+00B4), ` (U+0060), ^ (U+005E),
  ¨ (U+00A8) or ~ (U+007E). **Pressing the same dead key twice** types its accent once.
- **Dead key, then a key with no accented form** (a letter not in the pairs, a digit, a sign,
  a third-level character) types the accent and then that character, both, as Windows does, so
  nothing typed is lost; `^` then `2` is `^2`. A different dead key types the first's accent and
  becomes pending itself.
- **Dead key, then Backspace** cancels the pending accent and deletes nothing. Any other
  non-printing key (Enter, Tab, Esc, arrows, function keys, a shortcut with Ctrl, Cmd or Super)
  drops the accent and does its own thing.
- **The third level**: in the Windows and Linux profiles, **Right Alt is AltGr** and reaches it
  (`AltGr+E` is `€` on every layout that has a third level: also `AltGr+4` on `English (UK)` and
  `AltGr+5` on `English (US, international)`); Left Alt stays Alt (menus, Alt+Tab, Alt+F4, and
  `ESC`+key in the terminal). Ctrl+Alt is not AltGr, unlike Windows: it stays a shortcut
  (decided without research; the owner may overrule). In the **macOS profile, Option** (either
  Alt key) reaches the third level, after the shell's own Option shortcuts (Option+Tab,
  Option+F4) have had their turn; a key with no third-level character keeps today's behaviour
  (`ESC`+key in the terminal, nothing in the toolkit). Shift with the third-level key reaches
  the fourth level where the layout defines one, else the third level's character. Layouts
  without a third level (`us`, Dvorak, Colemak) have no `€`: the pane says so.
- **Caps Lock** capitalises letters only, accented letters included (`é` to `É`, `ü` to `Ü`,
  `ø` to `Ø`), never digits or signs, on every layout and profile: macOS's rule, and the one
  the terminal already has for ASCII. (Windows's French layout also shifts the number row under
  Caps Lock; EggOS does not, a difference worth a line in the docs.) Shift with Caps Lock gives
  the small letter. The LED follows the state on every keyboard that has one, from power-on
  (off) through every toggle; the state is the keyboard's, shared by the console and the
  desktop, and the pane's keyboard line shows it.

## 6. The CLI and MCP front-ends (D22)

Nothing new is added; `keyboard.layout` stops being read-only.

| Call | Says |
|------|------|
| `settings get keyboard.layout` | `us` (the id, never the title) |
| `settings set keyboard.layout de` | applies at once and saves; the same success line as `shell.profile`'s today; status 0 |
| `settings set --dry-run keyboard.layout de` | `would set keyboard.layout from us to de`; nothing changes |
| `settings set keyboard.layout xx` | `settings: keyboard.layout: invalid value 'xx' (us, us-intl, gb, us-dvorak, us-colemak, dk, fi, fr, de, it, no, br, es or se)`, the ids in the list's order, status 2, the JSON error `invalid_value`, exactly the form `shell.profile`'s refusal has |
| `settings list` | the line `keyboard.layout=us` among the others |
| MCP `settings_set` with `key` `keyboard.layout`, `value` `de` | needs the `run` grant as every set; answers `Set keyboard.layout to de.`; `dry_run` as the CLI's; the same `invalid_value` error |
| MCP `settings_get` with `key` `keyboard.layout` | `de` |

Ids are exact and lower-case (`DE` is invalid), like `macos`. `docs/settings.md`'s table
changes the row to settable with the ids, and `docs/mcp.md`'s `settings_set` description names
`keyboard.layout` beside `shell.profile`.

## 7. Acceptance (host tests, default 640x440, scale 1x)

- The sidebar's `Keyboard` row is (0, 108, 176, 32); `keymap`, `altgr`, `dead keys`, `caps
  lock`, `azerty` and `german` each match it and nothing else; `layout` matches Keyboard and
  Appearance as today.
- In window coordinates: heading at (192, 16); the list (192, 64, 208, 240) with 24 px rows, its
  `>` at x 200 and texts at x 216, the first row `English (US)` selected when the facts say `us`;
  the third-level line at (416, 68); the dead-keys line at (416, 88); the field (416, 120, 208,
  28) with placeholder `Type here to try it`; the keyboard line at (416, 156); the no-disk line
  at (192, 316) only without a disk.
- With `us` and the Windows profile the lines read `No third level, so no € key` and `No dead
  keys`; clicking `German` asks the program to set `keyboard.layout` to `de` and the lines read
  `Right Alt (AltGr) types € and more` and `Dead keys: ´ ` ^`; applying the macOS profile
  changes the first to `Option types € and more`; `French` reads `Dead keys: ` ^ ¨ ~`;
  `English (US, international)` reads `Dead keys: ´ ` ^ ¨ ~`.
- Down from `English (US)` with the list focused selects and asks for `us-intl`; a second click
  on the selected row asks for nothing.
- With Caps Lock on, the keyboard line reads `Caps Lock is on` in `text`; off, it is hidden;
  without a keyboard it reads `No keyboard attached.` in `text.dim` and the field is disabled.
- Showing another pane and coming back empties the field and keeps the selection.
- The keymap: on `de`, `´` then `e` yields `é`, `´` then Shift+`e` yields `É`, `´` then Space
  yields `´`, `´` then `´` yields `´`, `^` then `2` yields `^2`, `´` then Backspace yields nothing
  and deletes nothing, Right Alt+`e` yields `€`, Right Alt+`q` yields `@`; on `us`, Right Alt+`e`
  yields nothing new; in the macOS profile Alt+`e` on `de` yields `€`; Caps Lock with `é` yields
  `É` and with Shift+`a` yields `a`; `¨` then `y` on `us-intl` yields `¨y`.
- The fonts: Egg Mono renders every codepoint U+00A0-U+00FF and U+20AC from
  `font-8x16-latin-1.txt` (97 glyphs, each 16 rows of 8, column 7 empty, NBSP empty, the
  accented capitals' rows 2-12 equal to the ASCII capitals' except A-ring's row 2, the accented
  small letters' rows 5-12 equal to the ASCII small letters'), and `?` for anything else; Egg
  Sans renders U+20AC, 6 wide, and its known-string hash test includes `€`.

## 8. Notes on the glyphs

Egg Mono's supplement keeps the ASCII letters pixel for pixel and adds the accents in the rows
the font reserves, so a terminal line of `é` and `e` differs only above the x-height.

- **Accent placement.** Capitals' accents sit in rows 0-1 and touch the capital at row 2
  (there is no spare row: the font's capitals use 2-12); A-ring's ring takes rows 0-2 and its A
  starts at row 3, as Egg Sans shortens its A. Small letters' accents sit in rows 2-3 with row 4
  empty, exactly where the dot of `i` sits, and a-ring's ring takes rows 2-4; `ì í î ï` drop the
  dot. The shapes are Egg Sans's at one more column: acute and grave 2 px diagonals, circumflex
  3 px, diaeresis two 1x2 dots three columns apart (columns 2 and 5; 2 and 4 over the narrow
  I and i), tilde the 5-wide two-row wave, rings a 4-wide loop on the capital and a 3-wide one
  on the small letter. The cedilla is the 2-row hook of rows 13-14 (row 15 stays empty so it
  never meets the next line's accents); the descenders of `ý ÿ þ` are `y`'s and `p`'s.
- **The euro sign.** Egg Mono: the font's `C` with two bars in rows 6 and 8 from column 0 to 4,
  crossing the stem, so it reads as € beside `E` (bar at row 7) and `C`. Egg Sans: 6 wide like
  the digits and the other currency signs (`¢ £ ¥`), built like its `£`: a 4-wide hook in
  columns 1-4, two 4-wide bars in rows 6 and 8 from column 0, one spacing column; so prices set
  in tabular columns keep their width.
- **Symbols.** `©` and `®` use all seven columns (a 7x9 ring, rows 3-11, with a 3-wide c or R
  inside); the fractions set 3x5 digits (numerator columns 0-2, rows 2-6; denominator columns
  4-6, rows 8-12) around a slash from (6, 4) to (2, 8); `¹ ² ³` are 7 rows (2-8) and `ª º` 5
  (2-6); `« »` are 5-row chevrons centred on row 8 like `<` and the hyphen; `× ÷ ± ¬ ·` centre
  on row 8 as well; `¦` breaks at rows 7-8; `¡` and `¿` descend (`¿` to row 15, Egg Sans's
  rule); `µ` is `u` with its left stem continued to row 15; `ß` is Egg Sans's construction at
  seven columns; `æ` shares one spine at column 3; `Ø` and `ø` carry their slash one pixel past
  the bowl at both ends; `Ð` is `D` with a bar at row 7 from column 0; `ð`'s crossed stroke sits
  in rows 3-5 over a bowl of rows 6-12, as Egg Sans's does over a one-row-shorter o.
- **For a later Unicode extension** (Latin Extended-A, the typographic dashes and quotes,
  `…`): keep the accent rows and the dotless-i rule, keep column 7 empty, and build new accents
  from these shapes (a caron is the circumflex upside down, a macron row 2 or row 0, a stroke
  through `ł` at row 8). `Ÿ` (U+0178) is the first letter worth adding: until then diaeresis + Y
  on a dead-key layout types `¨Y`. Egg Sans takes the same extension glyph for glyph, with the
  widths its header gives.
