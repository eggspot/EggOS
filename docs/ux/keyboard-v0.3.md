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

## 9. Keyboard and text, the rest (batch part 3, D221: D185's "Not yet")

Design by Claude Fable for D221. Everything above stands except where this section says so:
several layouts with a switch key and the bar's layout indicator, the Mac Option layout and the
dead-key highlight, five more layouts, the Num Lock and Scroll Lock lights, the launcher's search
beyond ASCII, Egg Mono's Latin Extended-A glyphs (`font-8x16-latin-ext-a.txt`) and the device
classes. Numbers are px at 1x and double at 2x; tokens are `desktop-v0.3-gui.md` section 1's and
`toolkit-v0.3.md`'s; the only motion is the caret's blink and the hint of 9.3 going away.

### 9.1 Several layouts and the switch key: the settings

| Key | Values | Default |
|-----|--------|---------|
| `keyboard.layouts` | one to four layout ids, in order, separated by commas without spaces (`us,de`), no id twice | `us` |
| `keyboard.layout` | the layout in force, one of `keyboard.layouts` | `us` |
| `keyboard.switch_key` | `profile`, `super+space`, `ctrl+space`, `alt+shift`, `ctrl+shift` or `none` | `profile` |

- **Four at most** (`MAX_KEYBOARD_LAYOUTS` = 4): the set fits four bytes of kernel state, the
  indicator's list stays short, and macOS's menu and Windows's list are rarely longer. The set's
  order is the cycling order and the pane's order.
- **`keyboard.layout` stays the one capability's value** (section 6): `settings set
  keyboard.layout de` applies live as today. When `de` is not in `keyboard.layouts` it is
  appended when the set has room, and otherwise takes the place of the layout in force, so a
  one-layout system behaves exactly as D185 (`us` becomes `de`), and a full set is never grown
  silently past four. Setting `keyboard.layouts` to a list without the layout in force makes the
  list's first layout the one in force, applied live. The kernel reads all three keys at boot
  with `shell.profile` (`terminal::load_saved_settings`).
- **`profile`** means the key of the person's previous OS: **Super+Space** in the Windows profile
  (Win+Space) and the Linux profile (GNOME's Super+Space), **Ctrl+Space** in the macOS profile
  (macOS's own; Cmd+Space stays the launcher). Choosing `super+space` by hand in the macOS
  profile gives the switch the key: an explicit choice wins over the launcher's second shortcut,
  while `shortcuts.rs`'s Super-alone launcher still works. Shift added to a Space chord goes
  backwards. `alt+shift` and `ctrl+shift` are Windows's legacy chords: they fire when the second
  key is released with no other key pressed in between, so Alt+Shift+Tab, Ctrl+Shift+Esc and
  shifted shortcuts are untouched, and have no backwards form (press again).
- **With one layout the switch key does nothing and is passed on**, exactly as macOS's Ctrl+Space
  reaches the terminal until a second input source is added; so the default costs nobody a key.
- **The switch lives in the kernel's input layer** (`input::InputState`: the set as four layout
  numbers and a count, the switch key, `input::cycle_layout(backwards)`; `Typist::set_layout`
  drops a pending accent as today), so it works on the console before the desktop starts and in
  every window and layer. A cycle sends `InputEvent::LayoutChanged(number)` to the desktop for
  the indicator and the hint; key repeat of the chord never cycles twice. `/dev/window` gains
  `SET_KEYBOARD_LAYOUTS` (the set and the switch key, the `SET_PROFILE` permission) beside
  `SET_KEYBOARD_LAYOUT`; `GET_KEYBOARD_LAYOUT` keeps answering the layout in force.

**CLI and MCP (D22)**, in section 6's form:

| Call | Says |
|------|------|
| `settings get keyboard.layouts` | `us,de` |
| `settings set keyboard.layouts us,de` | applies (the layout in force kept if listed, else the first) and saves; status 0 |
| `settings set keyboard.layouts us,de,fr,es,it` | `settings: keyboard.layouts: invalid value 'us,de,fr,es,it' (at most 4 layouts)`, status 2, JSON `invalid_value` |
| `settings set keyboard.layouts us,us` | `settings: keyboard.layouts: invalid value 'us,us' ('us' is listed twice)` |
| `settings set keyboard.layouts us,xx` | `settings: keyboard.layouts: invalid value 'xx' (us, us-intl, us-mac, gb, ... or se)`, the ids in the table's order, as `keyboard.layout`'s refusal |
| `settings set keyboard.switch_key alt+shift` | applies at once; `settings: keyboard.switch_key: invalid value 'x' (profile, super+space, ctrl+space, alt+shift, ctrl+shift or none)` on anything else |
| `settings list` | `keyboard.layouts=us,de`, `keyboard.switch_key=profile` among the others |
| MCP `settings_set` / `settings_get` | the same keys and values, the `run` grant, `Set keyboard.layouts to us,de.`; `docs/settings.md` and `docs/mcp.md` gain the two rows |

### 9.2 The layouts table, now 20 rows (replaces section 2's)

Columns as section 2 plus the indicator's **code**: two capital letters, distinct per layout, the
country code where there is one. `us-mac` goes with the US variants, the five new languages go
alphabetically by title; a layout's number is its row here minus one (the numbers of `gb` and
everything after it shift by one or more: they are internal to `/dev/window` and the kernel, the
settings file holds ids).

| # | Id | Title | Code | Third level | Dead keys |
|---|----|-------|------|-------------|-----------|
| 1 | `us` | `English (US)` | `US` | no | none |
| 2 | `us-intl` | `English (US, international)` | `UI` | yes | ´ ` ^ ¨ ~ |
| 3 | `us-mac` | `English (US, Mac)` | `UM` | yes | ´ ` ^ ¨ ~ |
| 4 | `gb` | `English (UK)` | `GB` | yes | none |
| 5 | `us-dvorak` | `English (US, Dvorak)` | `DV` | no | none |
| 6 | `us-colemak` | `English (US, Colemak)` | `CK` | no | none |
| 7 | `dk` | `Danish` | `DK` | yes | ´ ` ^ ¨ ~ |
| 8 | `fi` | `Finnish` | `FI` | yes | ´ ` ^ ¨ ~ |
| 9 | `fr` | `French` | `FR` | yes | ` ^ ¨ ~ |
| 10 | `be` | `French (Belgium)` | `BE` | yes | ´ ` ^ ¨ ~ |
| 11 | `ca` | `French (Canada)` | `CA` | yes | ` ^ ¨ ~ ¸ |
| 12 | `de` | `German` | `DE` | yes | ´ ` ^ |
| 13 | `ch` | `German (Switzerland)` | `CH` | yes | ´ ` ^ ¨ ~ |
| 14 | `it` | `Italian` | `IT` | yes | none |
| 15 | `no` | `Norwegian` | `NO` | yes | ´ ` ^ ¨ ~ |
| 16 | `br` | `Portuguese (Brazil)` | `BR` | yes | ´ ` ^ ¨ ~ |
| 17 | `pt` | `Portuguese (Portugal)` | `PT` | yes | ´ ` ^ ¨ ~ |
| 18 | `es` | `Spanish` | `ES` | yes | ´ ` ^ ¨ |
| 19 | `latam` | `Spanish (Latin America)` | `LA` | yes | ´ ` ^ ¨ |
| 20 | `se` | `Swedish` | `SE` | yes | ´ ` ^ ¨ ~ |

- **The five new layouts** are Windows's `Latin American`, `Portuguese`, `Swiss German`,
  `Belgian French` and `Canadian French` tables (D185's rule: Windows's levels where Windows
  and Linux differ, characters outside the fonts left out, which Latin Extended-A now shrinks
  to a few). `be` is AZERTY like `fr` (its `§`, `²`, `³` and `µ` are all Latin-1); `ch` is
  QWERTZ with `ü ö ä` on the `fr`-style é/è keys' places; `ca` has the **cedilla dead key**
  (`¸` then `c` is `ç`, then Space is `¸`, then anything else types both), the sixth family, so
  `DEAD_ACCENTS`'s `¸` is now used and the dead-keys line's fixed order is `´ ` ^ ¨ ~ ¸`
  (`Dead keys: ` ^ ¨ ~ ¸`, 99 px). The titles keep section 2's form, `Language (Place)`, and the
  search words (section 1) gain `swiss`, `belgian`, `canadian`, `latin america`, `portugal`,
  `mac`, `option` is still out.
- **`us-mac`** is the Mac's U.S. layout: the first two levels are `us`, the **third level is
  macOS's Option layer** and its **dead keys sit on Option**, as on a Mac: Option+E acute,
  Option+` grave, Option+I circumflex, Option+U diaeresis, Option+N tilde; Option+A `å`,
  Option+C `ç`, Option+O `ø`, Option+S `ß`, Option+' `æ`, Option+Q `œ` (U+0153, now drawn),
  Option+Y `¥`, Option+R `®`, Option+G `©`, Option+M `µ`, Option+1 `¡`, Option+/ `÷`,
  Option+3 `£`, Option+4 `¢`, Option+6 `§`, Option+7 `¶`, Option+9 `ª`, Option+0 `º`, Shift+Option+2
  `€`, Shift+Option+/ `¿`, Shift+Option+Q `Œ`, Shift+Option+B `ı`, with Shift giving the
  capital of each accented letter; keys whose Option character the fonts lack (`™ ∞ • – — ≠ ≤ ≥
  π Ω √ ∫ “ ” ‘ ’`) type nothing at that level, as D185 ruled. In the macOS profile Option is
  the third level anyway (section 5), so a Mac user on `us-mac` gets every Option habit; in the
  Windows and Linux profiles Right Alt reaches it, and the pane's third-level line says so.

### 9.3 The bar's layout indicator and the switch hint

**Shown only while `keyboard.layouts` has two or more layouts**, so a one-layout machine's bar is
pixel for pixel today's. It appears and disappears the moment the set changes (a save from the
pane, the CLI or MCP), re-laying the bar's right-hand items in one frame. Token names are the
code's `colour::` constants; `PanelItem::KeyboardLayout`, `Action::OpenSettingsPane(SettingsPane::
Keyboard)` as the network indicator's design names them.

- **Text**: the layout in force's two-letter **code** (9.2), 2 Egg Mono glyphs, 16 px wide, in
  `TEXT`, as the clock and `EggOS` are set in Egg Mono. Hover `BAR_HOVER`, pressed and while
  its menu is open `BAR_PRESSED`, as the power button's.
- **Top bar (macOS and Linux profiles)**: a 28x28 hit target directly left of its right-hand
  neighbour, the network indicator when that shows and the power button otherwise:
  `Rect::new(neighbour.x - 28, bar.y, 28, 28)`, the text at the target's (6, 6) (`BAR_TEXT_Y`).
  At 1280 without a network adapter: (1216, 0, 28, 28), text at (1222, 6), the Agent chip's
  right edge 8 px left of it, (1152, 4, 56, 20); with the adapter: the network indicator at
  (1216, 0), the layout indicator at (1188, 0, 28, 28), text (1194, 6), the chip (1124, 4, 56,
  20). The Linux chips and the macOS menu titles still stop at the clock's left - 16.
- **Taskbar (Windows profile)**: a 40x40 item whose right edge is 8 px left of its neighbour,
  the network item or the clock: `Rect::new(neighbour.x - 8 - 40, bar.y, 40, 40)`, text at the
  item's (12, 12). At 1280 without an adapter: (1096, 760, 40, 40), text (1108, 772), the chip
  (1032, 770, 56, 20); with it: (1048, 760, 40, 40), text (1060, 772), the chip (984, 770, 56,
  20). The taskbar items' room ends 16 px before the leftmost of the chip, the indicators and
  the clock.
- **Phone bar** (6.1's 48 px bar): a 48x48 item 8 px from the right edge, text at (16, 16).
  **2x**: every rect and the glyphs double (a 56x56 target, 32 px of text).
- **Tooltip**, after 600 ms, the panel tooltip's chrome: the title, two spaces, the switch key,
  in the system item's form `Apps  Super`: `German  Super+Space`, `German  Ctrl+Space`,
  `German  Alt+Shift`, or the title alone with `none`. Top bar: below the bar, centred on the
  target and clamped 8 px inside the screen; taskbar: above it.
- **Click opens a menu** (both profiles; a second click, Esc or a click outside closes it),
  not a cycle: a list is what macOS's and Windows's indicators open, it shows the whole set
  and it is the one place to reach the pane from the bar. The power menu's chrome (`MENU_BG`,
  1 px `MENU_BORDER`, radius 6, the window shadow), `LAYOUT_MENU_WIDTH` = 256 (the widest
  title, 27 Egg Mono glyphs = 216, after a 24 px text inset and 8 px right padding), rows of
  `MENU_ITEM_HEIGHT`: one per layout of the set in order, the title at x + 24 and the `>` mark
  in `ACCENT` at x + 8 on the layout in force (the toolkit list's non-colour cue), then a 1 px
  `MENU_BORDER` separator in an 8 px row, then `Keyboard settings...`. Anchored like the power
  menu: under the top bar (top at 32) with its right edge at the indicator's right edge; above
  the taskbar (bottom at the bar's top - 4). Hover `MENU_HOVER`; Up/Down, Enter and Esc work
  while it is open. Choosing a layout switches to it (the same path as the key, saving
  `keyboard.layout`; no hint, the menu was the feedback); `Keyboard settings...` is
  `Action::OpenSettingsPane(SettingsPane::Keyboard)`, as the network indicator's click opens
  its pane.
- **The hint**: when the switch key (not the menu, not the pane, not the CLI) changes the
  layout, the desktop shows the new layout's title in the notice of `desktop-v0.3-gui.md` 6.2
  (`Desktop::show_notice`: 320x40, centred, 16 px under the work area's top, `MENU_BG` chrome),
  for `LAYOUT_HINT_MS` = 1000 rather than `NOTICE_MS` = 3000: long enough to read `German`, gone
  before the next word is typed, GNOME's OSD at a tenth of the size. A second switch replaces
  it. On the console with no desktop, the switch is silent (there is no bar either; `settings
  get keyboard.layout` tells).
- **For agents** (`wm show`, D161): the indicator is a `button` named `Keyboard layout` with
  the layout's id as its value (`de`, what `settings_set` takes) and the flag `expanded` while
  the menu is open; the open menu is a `list` named `Keyboard layouts` of `list item`s titled
  by the layouts' titles, `selected` on the one in force, and a `button` `Keyboard settings...`.
  An agent switches with `settings_set keyboard.layout de` (D22) rather than by clicking.

### 9.4 The dead-key highlight (every layout, not only `us-mac`)

macOS shows the pending accent highlighted at the insertion point; Windows and Linux show
nothing, which is why D185 showed nothing. Now that a Mac layout exists, every layout gets the
Mac's cue: it is the one that tells a person why their next letter came out accented.

- **What is shown**: while a dead key's accent is pending, the caret's cell shows the spacing
  accent (`´ ` ^ ¨ ~ ¸`, U+00B4 U+0060 U+005E U+00A8 U+007E U+00B8) in `ICON_DARK` on an
  `ACCENT` fill: in Egg Mono the 8x16 cell at the cursor (the console and the Terminal app's
  grid; the cell's own character is hidden under it and comes back when the accent resolves);
  in Egg Sans fields and the Text Editor a provisional cell as wide as the accent's advance
  (at least 4 px) and 16 tall at the caret, the text after the caret moved right by its width
  until the accent resolves, as macOS's marked text does. The caret is hidden meanwhile: the
  highlight is the caret. No blink.
- **Resolving** follows section 5 exactly: the composed letter, or the accent and the character
  both, replace the cell in one damage of the line; Backspace removes the cell and deletes
  nothing; Space leaves the accent; a non-printing key drops the cell and does its own thing.
- **How programs know**: the dead key's `Key` event, which carries character 0 today (D185),
  carries the spacing accent with a new modifier bit `DEAD` (`protocol::modifier::DEAD`,
  `Modifiers::DEAD` in the toolkit), and a Backspace that takes an accent back, which sends
  nothing today, sends a `Key` of character 0 with `DEAD` set so the cell can go. Programs that
  ignore the bit (Windows edit controls, AppKit fields, Wayland clients) show nothing and still
  receive finished characters; the console draws it in `terminal.rs` from the kernel's own
  `Typist::is_pending`. Both layers' native cues are a later item.

### 9.5 Num Lock and Scroll Lock

- **Lights**: `InputDevice::set_caps_lock_light` becomes `set_lock_lights(LockLights { caps,
  num, scroll })`; virtio-input writes one `EV_LED` event per light that changed (`LED_CAPSL`,
  `LED_NUML`, `LED_SCROLLL`), only for the lights `EV_BITS` says the keyboard has, through the
  status queue as D185's; the retry rule from D185 covers all three. The states are the
  keyboard's, shared by the console and the desktop, from power-on through every toggle.
- **Num Lock starts on** (the PC habit; Windows and most firmware light it at boot; a Mac
  keyboard has no Num Lock and nothing changes). With it on the keypad types digits and the
  layout's decimal (`keypad_decimal`); with it off the keypad keys are the US keymap's
  navigation keys (7 Home, 8 Up, 9 PgUp, 4 Left, 5 nothing, 6 Right, 1 End, 2 Down, 3 PgDn,
  0 Insert, . Delete), as on Windows and the Linux console, in the terminal, every window and
  both layers (`Modifiers::NUM_LOCK`, bit 64, so the Windows layer's `GetKeyState(VK_NUMLOCK)`
  can follow). **Scroll Lock toggles its light and nothing else** (the terminal keeps its
  scrollback; `terminal-v0.3.md`'s "nothing" row stands); `Modifiers::SCROLL_LOCK`, bit 128,
  for programs that ask.
- **On screen**: no indicator or OSD (macOS has none, Windows none by default). The Keyboard
  pane's keyboard line (section 3's widget 6) reports the unusual states, `Caps Lock is on`,
  `Num Lock is off`, or both as `Caps Lock is on, Num Lock is off` (161 px, inside the 208 px
  column), in `TEXT`; Scroll Lock is not reported (nothing in EggOS reads it); `No keyboard
  attached.` as today; hidden otherwise. It follows the keys while the pane is shown, as today.

### 9.6 The launcher's search beyond ASCII (and Settings' search)

- **One fold, both sides**: `eggos_font::ascii_fold(character) -> &'static str` maps every
  letter the fonts draw to one or two lower-case ASCII bytes: `É é` to `e`, `Ÿ ÿ` to `y`,
  `ß` to `ss`, `Æ æ` to `ae`, `Œ œ` to `oe`, `Ĳ ĳ` to `ij`, `Ø ø Ő ő` to `o`, `Ð ð Đ đ` to `d`,
  `Þ þ` to `th`, `Ł ł Ŀ ŀ` to `l`, `Ħ ħ` to `h`, `Ŧ ŧ` to `t`, `ı İ` to `i`, `ĸ` to `k`, `Ŋ ŋ ŉ`
  to `n`, `ſ` to `s`; ASCII capitals to small letters; everything else (digits, signs, `€`,
  spaces) unchanged. A table of U+00C0-U+017F in the `font` crate (about 400 bytes of rodata;
  the kernel's desktop and the userland share it), no allocation.
- **Matching**: `contains_folded(text, query)` replaces `contains_ignoring_case` in
  `catalogue::matches` and in Settings' search (`settings_window.rs`): the folded query is a
  substring of the folded text. So typing `e` finds `Éditeur`, `strasse` finds `Straße`, `oe`
  and `œ` both find `Œuvre`, `lodz` finds `Łódź`, and `é` finds `Text Editor` too: the rule is
  symmetric (macOS's Spotlight and GNOME's search both forgive accents either way), which is
  one rule to learn and one function to test. Pure ASCII text and queries match exactly as
  before. Synonyms (`FILES_SYNONYMS` and the others) stay ASCII.
- **Sorting** (`catalogue::ordered`) folds the same way, so `Édition` sorts under E, then by
  the raw bytes as today, so the order is total.

### 9.7 The Keyboard pane with several layouts (replaces section 3's widget 2 and section 4's
"Choosing"; sections 3's right column and `displays-mouse-v0.3.md` section 4's rows stay)

The left column becomes **your layouts**, not every layout: the set in order, the one in force
selected; adding is a dropdown, the rest push buttons. Pane coordinates as section 3.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 2 | list | (16, 64), 208x96: 4 rows of 24, `Kind::list` with 4 empty static rows, mark x 8, text x 24; `set_shown_items` to the set's size | the row of the layout in force selected |
| 2a-2d | labels | (40, 68), (40, 92), (40, 116), (40, 140) | the set's titles in `text`, retitled on every change; declared after the list so they draw over its rows and take no pointer input, as the zone picker's (`date-time-v0.3.md` 8.2) |
| 16 | dropdown | (16, 168), 208x28 | `Add a layout...` (`Kind::dropdown`), options: that prompt first, then the 20 titles of 9.2 in order; `set_selected(id, 0)` after every choice, so it always reads as the prompt; disabled (`text.dim`) while the set has four |
| 17 | push button | (16, 204), 100x28 | `Move up`; disabled on the first row |
| 18 | push button | (124, 204), 100x28 | `Move down`; disabled on the last row |
| 19 | push button | (16, 240), 96x28 | `Remove`; disabled with one layout |
| 20 | label | (16, 282) | `Switch key` in `text` |
| 21 | dropdown | (96, 276), 128x28 | options `Profile's key`, `Super+Space`, `Ctrl+Space`, `Alt+Shift`, `Ctrl+Shift`, `None` (the six values of 9.1 in order); disabled without a keyboard |

Grid: the list ends at 160, the dropdown 8 below it, the button rows 8 below each other
(204, 240), the switch row at 276 ending at 304, where the old list ended, so the status line
(widget 7, (16, 316)) does not move. `Move up` and `Move down` are 100 wide (100 + 8 + 100 =
208, the one exception to the 96 minimum besides the steppers' 28s: `Length::Px(100)`);
`Switch key` is 53 px and the dropdown's widest option, `Profile's key`, 60 + 12 + 32 = 104,
inside 128. The right column (facts, try field, keyboard line, repeat rows) is unchanged and
shows the layout in force's facts.

- **Choosing a row** (click, arrows, Home/End) makes that layout the one in force, applied at
  once and saved as `keyboard.layout`, as section 4 said: every row is already in the set, so
  browsing with the arrows is harmless. `Opened` does nothing more.
- **Adding** (a choice other than the prompt): a layout not in the set is appended and **made
  the one in force** (the person added it to type with it; the switch key or the menu brings
  the others back); the labels, the mark and the facts change in the same frame; one save of
  `keyboard.layouts` and `keyboard.layout`, one `SET_KEYBOARD_LAYOUTS`. A layout already in the
  set is selected instead (the same as clicking its row) and nothing is added. When the set
  reaches four the dropdown is disabled and the status line (widget 7) reads `Four layouts is
  the most.` in `text.dim` (a state, like the no-disk line; a save error's line wins).
- **Removing** takes the selected layout out; the next row, or the previous when it was last,
  becomes the one in force. **Moving** swaps the selected row with its neighbour and keeps it
  selected; the bar's menu and the switch key follow the new order at once.
- **The switch key** dropdown writes `keyboard.switch_key` on `Chosen`; its `Profile's key`
  row is the default and the tooltip of 9.3 shows what it resolves to. Without a keyboard it is
  disabled (`No keyboard attached.` says why), as the try field is.
- **Saving** as `displays-mouse-v0.3.md` 3.2 and 3.3, the status line worded `keyboard
  settings`; the pane follows a set changed from the CLI or MCP the next time the window is
  focused, as section 4 says for the layout.
- **Tab order**: the layouts list, `Add a layout...`, `Move up`, `Move down`, `Remove`, the
  switch key, the try field, the four stepper buttons, then wraps to the search field;
  disabled buttons are skipped. **One column** (`displays-mouse-v0.3.md` 7): the left column
  is already 208 wide and self-contained, so it stays as drawn and the right column's rows
  follow it at y 324 onwards as that section lays them out.
- **Budget**: Keyboard goes from 15 widgets to 25 (+4 labels, +2 dropdowns, +3 buttons, +1
  label), so `CAPACITY` grows by **10** from its value when this is built (176 today, so 186)
  and the installer's `eggos_app::dialog::CAPACITY` stays equal to it.
- **For agents**: the list is a `list` named `Your layouts` whose `list item`s are the titles
  with `selected` on the one in force; the dropdowns carry toolkit-v0.4 section 6's role,
  named `Add a layout` and `Switch key`; the buttons `button`s named by their text with
  `disabled` as drawn. `settings_set` with `keyboard.layouts` is the one-call way (D22).

### 9.8 Glyphs: Latin Extended-A in Egg Mono (`font-8x16-latin-ext-a.txt`)

The file holds the 128 glyphs of U+0100-U+017F (Ÿ, U+0178, among them, so diaeresis + Y now
types `Ÿ` and section 5's exception goes) in `font-8x16-latin-1.txt`'s exact format, read while
the `font` crate compiles (`font/src/latin_ext_a.rs`, a parser like `latin_1.rs`'s). Egg Mono's
`u8` glyph ids are full (ASCII, the 32 extras, `€` at 0x7F, Latin-1), so the extension is a
second table indexed by `codepoint - 0x100`; `glyph_id` grows to a `u16` (or `rows` takes the
character and picks the table), `covers` says yes for U+0100-U+017F, and nothing else in the
drawing paths changes: callers already pass code points. **Egg Sans** draws these characters
from Egg Mono's glyphs (8 wide, 16 tall, the shared baseline at row 12) until it has its own,
drawn glyph for glyph from the recipe below as Latin-1's were (D185): a mixed-width `Ł` in a
label beats a `?`.

The recipe, in the file's header too: every letter keeps its ASCII body pixel for pixel and
the accent sits in the rows the font reserves (section 8): rows 0-1 on capitals and on small
letters with an ascender (`ĥ`, `ĺ`; `ď`'s caron sits over its ascender at columns 4-6), rows
2-3 on the other small letters with row 4 empty, the i-forms dotless. New accents from
section 8's shapes: the **caron** is the circumflex upside down; the **breve** `.#...#..` over
`..###...`; the **macron** one row `..####..` at row 0 (capitals) or row 2 (small) with the row
under it empty, so it never thickens the letter's top; the **dot above** one row at row 0 on
capitals and `i`'s own dot on small letters; the **double acute** `...#.#..` over `..#.#...`;
the **ring** of Latin-1 (`Ů`'s U starts at row 3 like `Å`'s A); the **cedilla** of `Ç` under
the stem or the centre, `ţ`'s under its foot; the **ogonek** `.....#..` over `.....##.` at
rows 13-14 hanging from the letter's right, centred under `į`; `ģ`'s comma above is the
cedilla's shape at rows 2-3. **Strokes**: `Đ` is `Ð`'s glyph; `đ` bars its ascender at row 3;
`Ħ` bars row 4 across columns 0-6; `ħ` columns 0-2 at row 3; `Ł` adds (2, 6) and (0, 8) to L's
stem and `ł` (4, 7) and (2, 9); `Ŧ` bars columns 2-4 at row 6, `ŧ` columns 1-3 at row 9. The
caron on `d L l t` is the apostrophe form (a 2 px stroke at rows 2-3 right of the letter's
top), as Czech type sets it. **Ligatures** share one spine at column 3 like `æ`: `Œ` a
rounded 4-wide O and a 4-wide E, `œ` the same at x-height, `Ĳ ĳ` I in columns 0-2 and J in
4-6 with both dots. `ĸ` is `k` without its ascender, `Ŋ ŋ` continue the right stem three rows
to a 4-wide foot at row 15, `ſ` is `f` without its bar, `ŉ` is `n` with an apostrophe at
column 0. Column 7 stays empty in every glyph.

### 9.9 Every device class (D23)

- **Phone and tablet**: the on-screen keyboard, when it comes (`terminal-windows-v0.3.md` lists
  it), draws its key caps from the layout in force's first two levels (the same tables, so the
  caps match a Bluetooth keyboard's), its switch key is a key labelled with the code (`DE`,
  no globe glyph in the fonts), a tap cycles and a long press opens 9.3's menu as a bottom
  sheet; the Keyboard pane's `Switch key` row is disabled without a physical keyboard. The
  phone bar shows the indicator as 9.3 says; the hint notice is 320 wide or the work area's
  width, whichever is less, as today.
- **TV (scale 2)**: everything doubles; the indicator is reachable by arrows like every bar item
  and the focus ring is its cue; the remote's keyboard is the on-screen one, so the switch key
  is its code key.
- **Wearable** (< 300 px): no bar indicator and no menu; the hint alone says which layout a
  paired keyboard switched to.
- **Headless**: `settings set` and `settings_set` are the whole capability, the switch key
  works on the serial console's keyboard if there is one, and the hint is nothing.
- **Keyboard-only**: the indicator's menu opens with Enter on the focused item and the pane's
  every control is a Tab stop (9.7); the switch key itself needs no pointer.

### 9.10 Acceptance (host tests, 1280x800 and the pane at 640x440, scale 1x unless said)

1. **The table**: `KEYBOARD_LAYOUTS` and `LAYOUTS` have 20 entries in 9.2's order with its ids,
   titles and codes; every code is two ASCII capitals and distinct; `named(b"us-mac")` is 2 and
   `named(b"gb")` 3; every new layout's rows decode (a const assert per row); the dead-keys line
   per layout matches the table (`ca`: `Dead keys: ` ^ ¨ ~ ¸`).
2. **Typing on the new layouts**: `ca`: `¸` then `c` is `ç`, `¸` then Space is `¸`, `¸` then `k`
   is `¸k`; `pt`: `~` then `a` is `ã`; `ch`: `¨` then `a` is `ä`, `¨` then Shift+`a` is `Ä`;
   `be`: AltGr+`ù` then `e` is `é`; `latam`: `´` then `e` is `é`; `us-mac` in the macOS profile:
   Option+E then `e` is `é`, Option+U then Shift+`u` is `Ü`, Option+Q is `œ`, Shift+Option+Q is
   `Œ`, Option+S is `ß`, Shift+Option+2 is `€`, Option+T types nothing, and in the Windows
   profile Right Alt+E then `e` is `é`; `¨` then `y` on `us-intl` now yields `ÿ` and Shift+`y`
   `Ÿ`.
3. **Settings**: `keyboard.layouts=us,de` parses to the set and `keyboard.layout=de` to the
   second; `us,de,fr,es,it`, `us,us` and `us,xx` are refused with 9.1's lines, status 2, JSON
   `invalid_value`; `settings set keyboard.layout fr` with `us,de` gives `us,de,fr`; with
   `us,de,fr,es` and `de` in force it gives `us,fr,fr,es` minus the repeat, that is `us,fr,es`
   with `fr` in `de`'s place and in force;
   `settings set keyboard.layouts gb,se` with `de` in force makes `gb` the layout in force;
   `settings get keyboard.switch_key` is `profile` by default and `settings list` has both
   lines.
4. **Cycling**: with `us,de,fr` and `us` in force, `cycle_layout(false)` gives `de`, `fr`, `us`;
   `cycle_layout(true)` from `us` gives `fr`. `profile` is Super+Space in the Windows and Linux
   profiles and Ctrl+Space in the macOS profile. Super+Space (Linux profile) cycles on the press
   and not on its repeat, Shift+Super+Space cycles backwards; Alt+Shift cycles on the release
   when nothing else was pressed and not when `Alt+Shift+Tab` or `Alt+Shift+a` happened in
   between; with `none` nothing cycles; with the single layout `us` Super+Space in the macOS
   profile opens the launcher as today and in the Linux profile reaches the window as a key.
   A cycle drops a pending accent and sends `LayoutChanged`.
5. **The indicator**: with `us` alone `panel_item_at(1230, 14)` finds nothing and the bar hashes
   as today; with `us,de` the top bar (macOS and Linux profiles, no adapter) has
   `PanelItem::KeyboardLayout` at (1216, 0, 28, 28) with `US` at (1222, 6) and the Agent chip at
   (1152, 4, 56, 20); with an adapter (1188, 0, 28, 28), text (1194, 6), the chip (1124, 4, 56,
   20); the taskbar has the item at (1096, 760, 40, 40), text (1108, 772), the chip (1032, 770,
   56, 20), and with an adapter (1048, 760, 40, 40), (1060, 772), (984, 770, 56, 20). Setting
   the layout to `de` retitles the text to `DE` and damages only the target. At scale 2 every
   rect doubles. Resting the pointer on it for 700 ms shows `English (US)  Super+Space` (Linux
   profile), `English (US)  Ctrl+Space` (macOS), `English (US)` with `none`.
6. **The menu**: a button-down on the indicator opens a 256-wide menu with its right edge at
   the target's right edge and its top at 32 (top bar) or its bottom at 756 (taskbar), rows
   `English (US)` (marked) and `German`, the separator, `Keyboard settings...`; a click on
   `German` asks the program for `keyboard.layout` `de` and closes; `Keyboard settings...`
   returns `Action::OpenSettingsPane(SettingsPane::Keyboard)`; Esc and a click outside close it;
   Down then Enter chooses `German`.
7. **The hint**: `LayoutChanged(de)` from the switch key shows the notice `German` at the work
   area's top + 16, 320x40 centred, and the notice is gone 1000 ms later; a switch from the menu
   or `settings set` shows none.
8. **The highlight**: in a toolkit field on `de`, `´` makes the caret cell an `ACCENT`-filled
   `´` in `ICON_DARK` and hides the caret; `e` replaces it with `é` and the caret is back;
   `´` then Backspace leaves the field as before with nothing deleted; the console's cell at the
   cursor hashes the same way (the frame with the pending accent differs from the frame before
   only in that cell, and the frame after `e` equals typing `é` directly). The dead key's `Key`
   event carries `´` with `DEAD`; the Backspace one carries 0 with `DEAD`.
9. **Locks**: at the first poll a keyboard with `LED_NUML` gets one `EV_LED` for Num Lock on and
   none for Caps or Scroll; KP_7 types `7`; after Num Lock, KP_7 is Home and KP_. is Delete and
   the light goes out; Scroll Lock lights `LED_SCROLLL` and changes no key; a keyboard without
   `LED_NUML` gets no event. The keyboard line reads `Caps Lock is on`, `Num Lock is off`,
   `Caps Lock is on, Num Lock is off` (width 161) in `TEXT`, and is hidden with Caps off, Num on.
10. **The fold**: `ascii_fold` maps `É` to `e`, `ß` to `ss`, `Œ` to `oe`, `Ÿ` to `y`, `Ł` to
    `l`, `ı` to `i`, `€` to `€`, `7` to `7`; `contains_folded("Éditeur", "e")`,
    `("Straße", "strasse")`, `("Œuvre", "oe")`, `("Œuvre", "œ")`, `("Łódź", "lodz")`,
    `("Text Editor", "é")` are true, `("Files", "z")` false, every ASCII pair agrees with
    `contains_ignoring_case`; `ordered` puts `Édition` between `Dolphin` and `Files`; Settings'
    search finds `Keyboard` with `azertÿ`.
11. **The glyphs**: Egg Mono renders every codepoint U+0100-U+017F from
    `font-8x16-latin-ext-a.txt` (128 glyphs in codepoint order, 16 rows of 8, column 7 empty);
    the capitals' rows 2-12 equal the ASCII capitals' except `Ů` (ring), `Ą Ę Į Ų` (rows 13-14
    only differ, rows 2-12 equal), `Ľ Ŀ Ł Ħ Ŧ Ŋ` (strokes and marks in the body), `Đ Œ Ĳ`; the
    small letters' rows 5-12 equal the ASCII small letters' except `ď ľ ŀ ł ħ ŧ đ ŋ ŉ ĸ ı œ ĳ ſ`
    and the cedilla and ogonek rows below; `Đ` equals `Ð` row for row; `covers(0x141)` is true
    and `covers(0x180)` false; the terminal draws `Łódź Ÿ Œuvre ĳ` without a `?`; a toolkit label
    with `Ł` draws Egg Mono's glyph 8 wide at the Egg Sans baseline.
12. **The pane** (window coordinates, pane x + 176): the list (192, 64, 208, 96) with labels at
    (216, 68), (216, 92), (216, 116), (216, 140); `Add a layout...` (192, 168, 208, 28); `Move up`
    (192, 204, 100, 28), `Move down` (300, 204, 100, 28), `Remove` (192, 240, 96, 28); `Switch
    key` at (192, 282); its dropdown (272, 276, 128, 28). With `us` alone: one label `English
    (US)` selected, `Remove`, `Move up` and `Move down` disabled, the dropdown enabled. Choosing
    `German` in the dropdown asks the program for `keyboard.layouts` `us,de` and `keyboard.layout`
    `de`, the second label reads `German` and is selected, the facts read German's, and the
    dropdown reads `Add a layout...` again; choosing `English (US)` then selects row 1 and asks
    for `keyboard.layout` `us` only. `Move down` on row 1 asks for `de,us` and keeps `English
    (US)` selected on row 2; `Remove` asks for `de` and `de`; with four layouts the dropdown is
    disabled and the status line reads `Four layouts is the most.` in `text.dim`. The switch
    dropdown reads `Profile's key` by default and choosing `Alt+Shift` asks for
    `keyboard.switch_key` `alt+shift`. Without a keyboard both the switch dropdown and the try
    field are disabled. `CAPACITY` equals the installer's.
13. **QEMU** (all booting architectures, the boot test): `settings set keyboard.layouts us,de`
    then a screendump shows the indicator with `US` at (1222, 6) and the Agent chip at (1152,
    4); Super+Space over QMP, then a screendump within 500 ms shows `DE` and the `German` hint,
    and `echo EGGOS SWITCH caf` + the German dead acute + `e` prints `EGGOS SWITCH café`.

### 9.11 Later

Egg Sans's own Latin Extended-A glyphs; the layers' native dead-key cues (an edit control's
and `NSTextField`'s marked text); eggway's `wl_keyboard` keymap per layout; per-user layouts;
the on-screen keyboard itself; `Ÿ`'s and the other new letters' Option chords on `us-mac`
beyond the ones listed; Latin Extended-B, the typographic dashes and quotes and `…` (the
recipe of 9.8 extends to them).
