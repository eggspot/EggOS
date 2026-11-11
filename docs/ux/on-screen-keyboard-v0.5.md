# EggOS v0.5 on-screen keyboard: design brief

Design by Claude Fable (D16) for the brief `adaptive-shell-v0.5.md` section 2.9 leaves open.
The parent fixes the panel's place on a phone (2.9), `adaptive-shell-tablet-v0.5.md` 8.3 on
tablets, `adaptive-shell-tv-v0.5.md` section 11 asks for a remote-driven variant, and
`keyboard-v0.3.md` 9.9 already decides three things kept here: the caps come from the layout
in force's tables, the switch key is labelled with the layout's two-letter code, and a long
press on it opens the layout menu as a bottom sheet.

The keyboard is one shell panel drawn by the server (`drivers/src/desktop/`). Numbers are
logical px from the screen's top left. Tokens are the desktop's and the toolkit's; **no colour
token and no motion is added**: the panel, the key preview and the alternates appear and go
in one frame. Egg Mono 8x16 draws every label, at 2x (16x32) for a character and at 1x for a
word (`Esc`, `123`, `Enter`); the marks on Shift, Backspace, the arrows and Hide are drawn
shapes (2.3), so the font needs no new glyph.

## 0. What a person gets, and how that is checked

- A phone or a tablet with no keyboard attached types in every text field, in the Terminal
  and in the launcher's search; a TV types with its remote.
- The caps are the person's own layout (all 20 of `KEYBOARD_LAYOUTS`); one key switches.
- Accents come two ways: hold a letter for its alternates, or tap an accent key and then the
  letter. Every ASCII character, every letter of the 20 layouts, the seven accents and
  `€ £ ¥ § ¡ ¿ ·` are typed by taps alone, with no timed gesture.
- An app gets the same `Key` events a USB keyboard would send. An agent needs none of it:
  `wm type` and `wm press` type with the keyboard shown or hidden (D22).
- Checked by host tests in the existing style (section 13).

## 1. When it shows and hides

### 1.1 The setting

| Key | Values | Default |
|-----|--------|---------|
| `keyboard.on_screen` | `auto`, `always`, `never` | `auto` |

- **`auto`**: it shows when a text field takes the focus by touch (phone, tablet) or is
  activated with OK (TV) and `input.present` has no `keyboard`.
- **`always`**: the same, with or without a keyboard attached, and for a pointer's click
  too; a window that never reports its text fields (a compatibility layer's, for now) gets
  the keyboard when it is tapped. QEMU and a person who types by pointer use this mode.
- **`never`**: it does not show; `wm keyboard show` is refused with `not_applied`.
- The desktop form has no on-screen keyboard in this brief (section 12); the key is kept
  there and has no effect. Settings > Keyboard gains one row, `On-screen keyboard`, a choice
  of `Automatic`, `Always`, `Never`; its `Switch key` row is disabled while no keyboard is
  attached (`keyboard-v0.3.md` 9.9).

### 1.2 Shows

1. A tap in a text field, a text area or the Terminal's grid (the window's `SET_TEXT_FOCUS`
   request with the cause `tap`, ask 2). A tap in a field that already has the focus shows it
   again after it was hidden.
2. A tap in the home screen's `Search apps` field (parent 2.4).
3. On a TV: OK on a focused field (cause `activate`). Moving the focus onto a field by arrows
   does not show it, or the arrows could never leave the field.
4. `wm keyboard show` (section 9), while a window or the search field has the focus.

A field focused by its program (a dialog's first field) or by Tab does not show it: the
person has not asked to type yet.

### 1.3 Hides

1. `Back` (parent 2.6, rule 1), the `Hide keyboard` key (10), `wm back`, `wm keyboard hide`.
   The field keeps the focus and its caret.
2. The focus leaves every text field: a tap on something else in the window, the window's
   `SET_TEXT_FOCUS` with kind `none`, the window closing or being minimised.
3. `Home`, `Apps`, the shade or a shell sheet opening.
4. A keyboard is attached (`auto` only). Detaching it does not show the keyboard by itself.
5. On a TV: also the `Enter` key in a one-line field (section 8).

### 1.4 The work area and the shown window

The panel never takes the keyboard focus and a touch on it never changes which window or
field has it. While it shows, the work area is the part above it and every shown window is
resized to that part with one `Resize` event; when it hides, they get their size back. A
toolkit window scrolls its focused field into view as it does for any resize.

| Form | Panel (1) | Work area while shown | Window's content size |
|------|-----------|-----------------------|-----------------------|
| phone, portrait 390x844 | (0, 536, 390, 260) | (0, 44, 390, 492) | 390x492 |
| phone, landscape 844x390 | (0, 214, 796, 176) | (0, 44, 796, 170) | 796x170 |
| tablet 1024x768 | (0, 404, 1024, 300) | (0, 44, 1024, 360) | full 1024x360; split sides keep their widths, 360 tall |
| tablet 1366x1024 | (0, 660, 1366, 300) | (0, 44, 1366, 616) | full 1366x616; split sides 616 tall |
| TV 960x540 | (0, 336, 960, 204) | (0, 0, 960, 336) | 960x336 |

- The navigation bar and the dock stay where they are; the panel's bottom is their top.
- A bottom sheet (a dialog on a phone, a narrow side on a tablet) sits with its bottom on the
  panel's top. The tablet's divider ends at the panel's top.
- On the home screen the `Background` window is not resized; the search's rows are laid out
  in the reduced work area. Showing or hiding damages the work area and the panel once.

## 2. The panel and a key

### 2.1 Anatomy

| # | Part | Draws |
|---|------|-------|
| 1 | panel | `bar.bg`; its top row `bar.edge` |
| | a key's **zone** | nothing: the rect a touch is tested against. Zones tile the panel with no gaps; the first and last zone of a row reach the panel's left and right edges |
| | a key's **cap** | the zone as drawn in section 4, inset 2 px on every side (caps are 4 px apart): the toolkit push button's corner table, a fill, a 1 px border |
| | a key's **label** | a character at 2x, a word at 1x or a drawn mark, centred in the cap on whole px (the left and the top get the odd px less) |

### 2.2 Colours (existing pairs only)

| Key | Fill | Border | Label |
|-----|------|--------|-------|
| a character key, Space | `button.bg` | `button.border` | `text` (10.5:1) |
| a function key (2 to 10, 14 to 17, 22, 23) | `menu.bg` | `menu.border` | `text` (12.9:1) |
| Enter (21) | `button.primary` | `button.primary` | `icon.dark`, the default button's pair |
| any key, pressed | `button.pressed`; Enter `button.primary.hover` | unchanged | unchanged (12.3:1) |
| a latched key (Shift `once` or `locked`, Ctrl, Alt, a pending accent) | `selection` | `button.border` | `text` (7.5:1), and a 2 px `accent` line on the cap's two bottom inner rows, 4 px in from each side |
| a key with a pointer over it (QEMU) | `button.hover` | unchanged | `text` (8.7:1) |

A latch is never colour alone: the fill, the line, and the changed mark or caps say it.

### 2.3 The drawn marks (all `text`, or the key's label colour)

| Mark | On | Shape |
|------|----|-------|
| up arrow, 14x13 | Shift | a head of 7 rows, 2, 4, 6, 8, 10, 12 and 14 px wide, on a stem 6 wide and 6 tall. `off`: its 1 px outline only. `once`: filled. `locked`: filled, with a 14x2 bar 2 px under it (14x17 in all) |
| backspace, 20x12 | Backspace | a 14x12 box with a 1 px outline whose left side is a point 6 px deep, and a 6x6 `x` of two 1 px diagonals centred in the box |
| triangle, 13x7 or 7x13 | Left, Up, Down, Right | filled; rows of 1, 3, 5, 7, 9, 11 and 13 px toward the base |
| hide, 13x11 | Hide keyboard | the Down triangle with a 13x2 bar 2 px under it |
| space, 24x4 | Space | a 24x1 line with a 1x3 upright at each end above it |
| accent bar, 12x2 | an accent key (section 6.2) | under the accent's glyph, where the letter will come |

## 3. Pages

### 3.1 The three pages

Every touch form shows three character rows (11, 12, 13) of one page at a time. The pages'
contents are the same in every form and on the TV; only the rects differ.

| Page | Row 1 (11) | Row 2 (12) | Row 3 (13) |
|------|------------|------------|------------|
| `Letters` | the layout's top letter row | its home row | its bottom row |
| `Numbers` | `1 2 3 4 5 6 7 8 9 0` | `- / : ; ( ) $ € & @` | `? ! ' " + = *` |
| `Symbols` | `[ ] { } < > ^ # % _` | `\`, `\|`, `~`, `` ` ``, `£ ¥ § ¡ ¿ ·` | the seven accent keys: acute, grave, circumflex, diaeresis, tilde, ring, cedilla (6.2) |

`,` (18) and `.` (20) are on every page. The sign pages are the same for all 20 layouts.

### 3.2 Switching

| Key | On `Letters` | On `Numbers` | On `Symbols` |
|-----|--------------|--------------|--------------|
| page key (16, and 23 where it exists) | `123`: to `Numbers` | `ABC`: to `Letters` | `ABC`: to `Letters` |
| key 14 (and 22) | Shift | `#+=`: to `Symbols` | `123`: to `Numbers` |

- A page stays until a page key changes it: typing a sign does not return to `Letters`.
- Each time the keyboard shows for a field it opens on `Letters`, or on `Numbers` for a field
  of kind `number` (5.5).

### 3.3 The `Letters` page comes from the layout in force

The caps are read from the kernel's table (`drivers/src/input/layout.rs`, `LAYOUTS`), so a
layout added there needs nothing here.

- A slot is a **letter key** when its plain level is a letter that has a capital in Latin-1
  (so `ü`, `ñ`, `ç`, `è`, `å`, `æ`, `ø` are; `º`, `µ`, digits, signs and dead keys are not).
- Row 1 is the letter keys of key codes 16 to 27 in order; row 2 those of 30 to 40; row 3
  those of 86, then 44 to 53, then 43. One more: a number-row slot whose plain level is a
  letter no accent key composes joins the end of row 3 (only `ß`, on `de`).
- Everything else on the physical layout (its sign keys, its dead keys, its third level) is
  not shown: signs are the sign pages', accents are section 6's.
- **Shifted caps are the capital of the plain level** (`ü` gives `Ü` on `ch`, where a
  physical Shift gives `è`): what Caps Lock types on a physical keyboard (`keyboard-v0.3.md`
  section 5). This narrows 9.9's "first two levels" to "the first level and its capitals".
- `n1`, `n2`, `n3` are the three rows' key counts. Examples: `us` 10, 9, 7; `de` 11
  (`qwertzuiopü`), 11 (`asdfghjklöä`), 8 (`yxcvbnmß`); `fr` 10 (`azertyuiop`), 11
  (`qsdfghjklmù`), 6 (`wxcvbn`); `it` 11, 11, 8 (`zxcvbnmù`); `es` 10, 10 (`…ñ`), 8
  (`zxcvbnmç`); `dk` 11 (`…å`), 11 (`…æø`), 7; `us-dvorak` 7 (`pyfgcrl`), 10, 9.

## 4. Geometry

Each rect is a zone (2.1). `i` counts a row's keys from 0.

### 4.1 Phone, portrait (panel (0, 536, 390, 260); `us`)

Rows: the keys row y 536, 44 tall; then four rows of 54 at y 580, 634, 688 and 742.

| # | Key | Zone | Label |
|---|-----|------|-------|
| 2 | `Esc` | (0, 536, 49, 44) | `Esc` |
| 3 | `Tab` | (49, 536, 49, 44) | `Tab` |
| 4 | `Ctrl` | (98, 536, 49, 44) | `Ctrl` |
| 5 | `Alt` | not in this form | |
| 6 | Left | (147, 536, 48, 44) | triangle |
| 7 | Up | (195, 536, 48, 44) | triangle |
| 8 | Down | (243, 536, 48, 44) | triangle |
| 9 | Right | (291, 536, 48, 44) | triangle |
| 10 | `Hide keyboard` | (339, 536, 51, 44) | hide mark |
| 11 | row 1, `q` to `p` | (39 x `i`, 580, 39, 54), `i` 0 to 9 | the character; `q`'s glyph cell is (11, 591, 16, 32) |
| 12 | row 2, `a` to `l` | (19 + 39 x `i`, 634, 39, 54), `i` 0 to 8; `a`'s zone starts at x 0, `l`'s ends at 390 | `a`'s glyph cell is (30, 645, 16, 32) |
| 13 | row 3, `z` to `m` | (58 + 39 x `i`, 688, 39, 54), `i` 0 to 6 | |
| 14 | Shift | (0, 688, 58, 54) | up arrow |
| 15 | Backspace | (331, 688, 59, 54) | backspace mark |
| 16 | page key | (0, 742, 58, 54) | `123` from (17, 761) |
| 17 | layout key | (58, 742, 44, 54); absent with one layout | the code, `US`, at 1x |
| 18 | `,` | (58, 742, 39, 54); (102, 742, 39, 54) with 17 | `,` |
| 19 | Space | (97, 742, 176, 54); (141, 742, 132, 54) with 17 | space mark |
| 20 | `.` | (273, 742, 39, 54) | `.` |
| 21 | Enter | (312, 742, 78, 54) | `Enter` from (331, 761), or 5.5's word |
| 22, 23 | second Shift, second page key | not in this form | |

- **The width exception.** Ten letters across 390 px cannot be 44 wide. Character keys are
  39x54 zones (32 or 35 wide for the layouts of 4.4): narrower than the parent's 44, taller to
  make up for it, with no dead gaps between them, a preview (24) and release-typing (5.1).
  Every other key is at least 44x44.
- On the sign pages rows 1 and 2 hold ten keys at (39 x `i`), and row 3 seven keys at the
  same seven zones as `z` to `m`.

### 4.2 Phone, landscape (panel (0, 214, 796, 176); `us`)

Four rows of 44 at y 214, 258, 302 and 346; there is no keys row, so its keys join row 4. The
unit `U` is 72.

| # | Key | Zone |
|---|-----|------|
| 11 | row 1 | (72 x `i`, 214, 72, 44), `i` 0 to 9 |
| 15 | Backspace | (720, 214, 76, 44) |
| 12 | row 2 | (36 + 72 x `i`, 258, 72, 44), `i` 0 to 8; `a`'s zone starts at x 0 |
| 21 | Enter | (684, 258, 112, 44) |
| 14 | Shift | (0, 302, 72, 44) |
| 13 | row 3 | (72 + 72 x `i`, 302, 72, 44), `i` 0 to 6 |
| 18, 20 | `,` and `.` | (576, 302, 72, 44) and (648, 302, 72, 44) |
| 22 | second Shift | (720, 302, 76, 44) |
| 16 | page key | (0, 346, 72, 44) |
| 2, 3, 4, 5 | `Esc`, `Tab`, `Ctrl`, `Alt` | (72, 346, 48, 44), (120, 346, 48, 44), (168, 346, 48, 44), (216, 346, 48, 44) |
| 17 | layout key | (264, 346, 72, 44); absent with one layout |
| 19 | Space | (264, 346, 292, 44); (336, 346, 220, 44) with 17 |
| 6, 7, 8, 9 | Left, Up, Down, Right | (556, 346, 48, 44), (604, 346, 48, 44), (652, 346, 48, 44), (700, 346, 48, 44) |
| 10 | `Hide keyboard` | (748, 346, 48, 44) |

### 4.3 Tablet (panels (0, 404, 1024, 300) and (0, 660, 1366, 300); `us`)

The keys row is 44 tall at the panel's top, then four rows of 64. `U` is 93 at 1024 and 124
at 1366. Upright (768 or 1024 wide) the same rules run with that width (4.4).

| # | Key | 1024x768 | 1366x1024 |
|---|-----|----------|-----------|
| 2, 3, 4, 5 | `Esc`, `Tab`, `Ctrl`, `Alt` | (93 x `k`, 404, 93, 44), `k` 0 to 3 | (124 x `k`, 660, 124, 44) |
| 6, 7, 8, 9 | Left, Up, Down, Right | (558 + 93 x `k`, 404, 93, 44) | (744 + 124 x `k`, 660, 124, 44) |
| 10 | `Hide keyboard` | (930, 404, 94, 44) | (1240, 660, 126, 44) |
| 11 | row 1 | (93 x `i`, 448, 93, 64) | (124 x `i`, 704, 124, 64) |
| 15 | Backspace | (930, 448, 94, 64) | (1240, 704, 126, 64) |
| 12 | row 2 | (46 + 93 x `i`, 512, 93, 64) | (62 + 124 x `i`, 768, 124, 64) |
| 21 | Enter | (883, 512, 141, 64) | (1178, 768, 188, 64) |
| 14 | Shift | (0, 576, 93, 64) | (0, 832, 124, 64) |
| 13 | row 3 | (93 + 93 x `i`, 576, 93, 64) | (124 + 124 x `i`, 832, 124, 64) |
| 18, 20 | `,` and `.` | (744, 576, 93, 64), (837, 576, 93, 64) | (992, 832, 124, 64), (1116, 832, 124, 64) |
| 22 | second Shift | (930, 576, 94, 64) | (1240, 832, 126, 64) |
| 16 | page key | (0, 640, 93, 64) | (0, 896, 124, 64) |
| 17 | layout key | (93, 640, 93, 64); absent with one layout | (124, 896, 124, 64) |
| 19 | Space | (93, 640, 837, 64); (186, 640, 744, 64) with 17 | (124, 896, 1116, 64); (248, 896, 992, 64) with 17 |
| 23 | second page key | (930, 640, 94, 64) | (1240, 896, 126, 64) |

Between `Alt` and Left the keys row is bare panel. No preview (24) is drawn where character
keys are 44 wide or more: landscape phones and tablets.

### 4.4 Other layouts and other widths: the rule behind the numbers

- **Phone, portrait.** `need` = max(`n1`, `n2`, `n3` + 3). The pitch `P` = 390 / `need`
  rounded down, at most 39: 39 for `need` 10 (`us`, `gb`, `us-colemak`), 35 for 11 (`de`,
  `fr`, `it`, `es`, the Nordic layouts), 32 for 12 (`us-dvorak`). A row of `n` keys starts at
  x (390 - `n` x `P`) / 2 rounded down. Shift is (0, 688) to row 3's first key; Backspace is
  from its last key to 390 (both at least 51). The sign pages use the same `P`.
- **Phone landscape and tablets.** `units` = max(`n1` + 1, `n2` + 1, `n3` + 3); `U` = the
  panel's width / `units` rounded down (796: 72 or 66; 1024: 93 or 85; 1366: 124 or 113; 768:
  69 or 64). Row 1 starts at x 0 and Backspace runs from its last key to the right edge. Row
  2 starts at (`units` - 1 - `n2`) x `U` / 2 rounded down and Enter runs from its last key to
  the edge. Row 3 is Shift (one `U`), its keys, `,`, `.`, then the second Shift to the edge
  when at least one `U` is left. The sign pages' rows 1 and 2 start at x 0.
- **2x**: every rect, glyph and mark doubles.

## 5. Typing

### 5.1 Press, release, two thumbs

- **A character key types when the finger lifts**, and types the key under the finger then:
  a finger that lands on `w` and slides to `e` types `e`. Lifted outside the panel, nothing
  is typed. The pressed look follows the finger.
- **Backspace and the arrows act on the press** (they repeat, 5.3). Every other function key
  acts on release inside its own zone, as a button does.
- **Two contacts.** When a second finger goes down on the panel before the first lifts, the
  first finger's key is typed at once and the second finger becomes the tracked one. So fast
  two-thumb typing loses nothing (ask 3). A third contact is ignored.
- **The preview (24)**, phone portrait only: while a character key is pressed, a 47x44 box at
  (the zone's x - 4, clamped to 0 and 343; the zone's y - 44) in `menu.bg` with a 1 px
  `menu.border` shows the character at 2x in `text`, centred. For `q`: (0, 536, 47, 44),
  glyph cell (15, 542, 16, 32). It is not drawn for a `masked` field.

### 5.2 Shift, Ctrl and Alt

- **Shift has three states and no timing**: a tap goes `off` to `once`, `once` to `locked`,
  `locked` to `off`. `once` capitalises the next character and returns to `off`; `locked` is
  caps lock. The letter caps show capitals while it is `once` or `locked`; the mark shows
  which (2.3). Both Shift keys show one state. Changing page drops `once` and keeps `locked`.
  The state is the on-screen keyboard's own: it does not set the keyboard's Caps Lock or its
  light.
- **Ctrl and Alt latch for one key**: a tap latches, the next key (a character, an arrow,
  Tab, Enter, Backspace, Space) is sent with the modifier and the latch drops; a second tap
  drops it unused. Ctrl then `c` is Ctrl+C; Shift `once` then an arrow extends a selection.
  While Ctrl or Alt is latched a held key shows no alternates.
- Nothing is typed automatically: no capitals, no correction, no prediction.

### 5.3 Repeat

Backspace and the four arrows repeat while held: the first at the press, then after
`keyboard.repeat_delay_ms` at `keyboard.repeat_rate`, the physical keyboard's two settings.
A slide off the key stops it. Characters, Space and Enter do not repeat (a held character is
its alternates, 6.1). `accessibility.slow_keys` does not delay the on-screen keys.

### 5.4 What a key sends

| Key | Sends to the focused window |
|-----|-----------------------------|
| a `Letters` key | its slot's key code through the kernel's `Typist`, with Shift, Ctrl and Alt as held modifiers: the layout's character, exactly as that key on a USB keyboard |
| a digit, a sign, `,`, `.`, an alternate | the character, as `wm type` sends it (ASCII as the US keymap's key, others as a character of their own, D185) |
| an accent key | the pending accent, as a dead key's `Key` record with `DEAD` (6.2) |
| Space, Tab, Esc, Enter, Backspace, the arrows | their key codes |

The home screen's search and shell sheets get the same keys the same way.

### 5.5 Field kinds and Enter's label

A window says what took the focus with `SET_TEXT_FOCUS` (ask 2); the toolkit fills it from
the widget (ask 5). A window that says nothing is `text` with `Enter`.

| Kind | Opens on | Changes |
|------|----------|---------|
| `text` | `Letters` | none |
| `multiline` (a text area) | `Letters` | Enter is always `Enter`: it adds a line |
| `masked` (a passphrase) | `Letters` | no preview (24) is drawn |
| `number` | `Numbers` | none |
| `email` | `Letters` | key 18 is `@` in place of `,` |
| `url` | `Letters` | key 18 is `/` in place of `,` |
| `search` | `Letters` | Enter reads `Search` |
| `terminal` | `Letters` | none; Shift never capitalises by itself there either |

Enter's label is one of six words the window names: `Enter` (the default), `Go`, `Search`,
`Next`, `Done`, `Send`. The key sends the Enter key whatever it reads; the word only tells
the person what Enter will do here.

### 5.6 The layout key (17)

- Shown only while `keyboard.layouts` has two or more layouts, as the bar's indicator is. Its
  label is the layout in force's code.
- **A tap** switches to the next layout of the set: the switch key's own path
  (`input::cycle_layout`), saved as `keyboard.layout`. The `Letters` page is laid out again
  in one frame (the pitch may change), the bar's indicator follows, and no hint notice shows:
  the caps are the feedback. A pending accent is dropped, Shift `once` too.
- **A long press** (500 ms), a right click, or the Menu key on a TV opens `keyboard-v0.3.md`
  9.3's menu as a bottom sheet over the panel: one row of 48 per layout with the `>` mark on
  the one in force, a separator, `Keyboard settings...`. Tapping through the set reaches
  every layout without the long press.

## 6. Accents

### 6.1 Alternates: hold a key (25)

A character key held still for 500 ms (`LONG_PRESS_MS`) opens its alternates in place of the
preview: a strip of 44x44 cells, `menu.bg` with a 1 px `menu.border`, each alternate at 2x in
`text`. Its top is the key's top - 44; its left is the key's middle - 22, clamped so the strip
stays inside the panel's width. For `e` on the phone: (75, 536, 220, 44).

- The cell under the finger is `selection`. Sliding to a cell and lifting types it. Lifting
  without moving **leaves the strip open**: a tap on a cell types it, a tap elsewhere or
  `Back` closes it. A right click on a key opens the strip the same way.
- Alternates are Latin-1 and `€` only, the characters both fonts draw:

| Key | Alternates | Key | Alternates |
|-----|-----------|-----|-----------|
| `a` | `à á â ä ã å æ ª` | `y` | `ý ÿ` |
| `c` | `ç ¢ ©` | `?` | `¿` |
| `d` | `ð` | `!` | `¡` |
| `e` | `è é ê ë €` | `"` | `« »` |
| `i` | `ì í î ï` | `$` | `¢ ¤` |
| `n` | `ñ` | `-` | `¬ ± ¯` |
| `o` | `ò ó ô ö õ ø º` | `/` | `÷` |
| `r` | `®` | `*` | `×` |
| `s` | `ß §` | `1 2 3` | `¹ ¼ ½`, `²`, `³ ¾` |
| `t` | `þ` | `0` | `°` |
| `u` | `ù ú û ü µ` | `.` | `·` |
| `p` | `¶` | `\|` | `¦` |

- With Shift on, the strip shows capitals; an alternate with no Latin-1 capital (`ÿ`) is left
  out, and `ß`, `µ`, `ª` and `º` stay as they are. An alternate that the `Letters` page
  already has as a key in this layout is left out. A key with no alternates shows no strip.

### 6.2 Accent keys: tap the accent, then the letter

Row 3 of `Symbols` is seven accent keys, the kernel's `DEAD_ACCENTS`, in every layout,
whether or not the physical layout has dead keys. Each cap is the spacing accent at 2x with
the accent bar under it (2.3).

- A tap makes that accent pending in the kernel's `Typist` (ask 4) and returns the keyboard
  to the `Letters` page. The key is drawn latched while the accent is pending, and the field
  shows `keyboard-v0.3.md` 9.4's highlight at the caret.
- The next key resolves it by section 5 of that brief, unchanged: a letter it can accent
  types the accented letter (with Shift, the capital), Space types the accent alone, the same
  accent twice types it once, Backspace takes it back, any other key types both.
- This is the untimed twin of 6.1: `é` is hold `e`, or `´` then `e`. The layout's own dead
  keys are not drawn; an attached keyboard's work as before.

## 7. The caret and the selection on touch

Drawn by the toolkit inside the window (ask 6), for fields and text areas; the Terminal keeps
`terminal-selection-v0.3.md`'s rules.

| # | Element | Draws | Target |
|---|---------|-------|--------|
| 26 | caret handle | under the caret, its point at the caret's bottom: a triangle of 6 rows, 2 to 12 px wide, on a 12x8 block (12x14 in all), `accent` | 44x44 centred on it |
| 27 | selection handles | the same shape twice: one with its point at the selection's start, one at its end, under the line | 44x44 each |

- **A tap** in a field puts the caret at the nearest character boundary and shows 26. The next
  typed key hides it; the next tap shows it again.
- **Dragging 26** moves the caret with the finger, by the offset it was grabbed at; past the
  field's edge the text scrolls as a pointer drag does (`toolkit-v0.5.md` 5.4).
- **A long press on a word** selects it (the run of letters and digits under the finger) and
  shows the two handles in place of 26. Dragging one moves that end; dragged past the other
  end, the two swap and the finger keeps the moving one. The selection's fill is `selection`
  as today.
- **Untimed twins**: Shift then Left or Right extends the selection a character at a time
  (5.2); Ctrl then `a` selects all; Ctrl then `c`, `x` and `v` copy, cut and paste. A field's
  own menu, where it has one, opens as the Menu key's does.
- Handles are drawn over neighbouring widgets, clipped to the window, and only after a touch:
  a mouse click or a key removes them. A `masked` field shows 26 and no word selection. No
  magnifier, no double tap.

## 8. TV: the remote's grid

The same pages and keys as a grid the arrows walk. Logical px at scale 2; labels are 1x.

- **Panel (1)**: (0, 336, 960, 204), `bar.bg`, row 336 `bar.edge`. Everything else is inside
  the safe area.
- **Cells**: 12 columns by 4 rows; cell (`c`, `r`) is (48 + 72 x `c`, 344 + 40 x `r`, 72,
  40); its cap is the cell inset 2 px.

| Row | Columns 0 to 10 | Column 11 |
|-----|-----------------|-----------|
| 0 | row 1's keys from column 0 | Backspace (840, 344, 72, 40) |
| 1 | row 2's keys from column 0 | Enter (840, 384, 72, 40) |
| 2 | key 14 at column 0 (Shift, or the other sign page's key); row 3's keys from column 1 | page key (840, 424, 72, 40) |
| 3 | layout key at column 0 (an empty cell with one layout); `,` (120, 464, 72, 40); `.` (192, 464, 72, 40); Space, columns 3 to 8, (264, 464, 432, 40); Left (696, 464, 72, 40); Right (768, 464, 72, 40) | `Hide keyboard` (840, 464, 72, 40) |

- `us`: `q` is (48, 344, 72, 40) with its glyph at (80, 356); `g` is (336, 384, 72, 40).
  Cells a row does not fill are bare panel and cannot take the focus. The widest layouts fit:
  11 letters in rows 0 and 1, Shift and 9 in row 2.
- `Esc`, `Tab`, `Ctrl`, `Alt`, Up, Down and the second keys are not on the TV. The remote's
  digit keys type digits; its other keys go where the TV brief says.
- **Focus**: exactly one cell has it, drawn with the TV's cues: `selection` fill and the 2 px
  `focus.ring` on the cap's outer 2 px. Labels are `text` on every cell. When the keyboard
  shows, the focus is on row 1, column 4 (`g` on `us`).
- **Arrows** move one cell and stop at the grid's edges (no wrap). Left and Right skip empty
  cells. Up and Down keep the column; onto an empty cell they take the nearest key to its
  left. Space remembers the column it was entered from and Up returns there.
- **OK** types the focused key; a held OK repeats it at the remote's rate. The focus stays.
- **Menu** on a letter opens its alternates (6.1) as a row of 72x40 cells above it, the focus
  on the first; Left, Right, OK; `Back` closes the strip. Menu on the layout key opens the
  layout menu as a shell menu (TV brief 3.4).
- **Back** hides the keyboard and the arrows go to the app again. Enter sends Enter and, in
  every kind but `multiline` and `terminal`, hides the keyboard too. Home works as always.
- While it shows, arrows and OK never reach the window. An attached keyboard's keys do, and
  the first one hides the on-screen keyboard (`auto`).

## 9. What an agent sees, and D22

**No capability here is GUI-only.** Typing is `wm type` and `wm press` (MCP `wm_type`,
`wm_press`), which work in every form whether the keyboard is shown, hidden or `never`, and
never show it. Layouts are `settings set keyboard.layout` and `keyboard.layouts`; the mode is
`settings set keyboard.on_screen`.

`wm show shell` lists the keyboard while it shows:

| Role | Name | Value | Notes |
|------|------|-------|-------|
| `group` | `On-screen keyboard` | the page and the layout: `letters us` | absent while hidden |
| `button` | the character it types now: `q`, or `Q` with Shift on | | one per character key; `invoke` types it |
| `button` | `Space`, `Backspace`, `Tab`, `Escape`, `Left`, `Up`, `Down`, `Right` | | |
| `button` | `Enter` | its word: `Enter`, `Search`, ... | |
| `button` | `Shift` | `off`, `once` or `locked` | listed once though two may be drawn |
| `button` | `Control`, `Alt` | `off` or `latched` | |
| `button` | `Numbers`, `Symbols` or `Letters` | | the page a page key opens, as its name |
| `button` | `Keyboard layout` | the layout's id, `de` | the bar indicator's name and value; `invoke` cycles |
| `button` | `Acute accent`, `Grave accent`, `Circumflex`, `Diaeresis`, `Tilde`, `Ring`, `Cedilla` | `pending` or empty | on `Symbols` |
| `list` | `Alternates of e` | | while the strip is open; `list item`s named by their character |
| `button` | `Hide keyboard` | | |

- On a TV the focused cell is flagged `focused`. Empty cells are not listed.
- `invoke` on a key is injected input: `wm type`'s rules hold (a person wins; a person-only
  window, D193, refuses it). A `masked` field's text is never listed.
- **`wm keyboard`** (MCP `wm_keyboard`): with no argument prints `shown` and the panel's
  rect, or `hidden`; `wm keyboard show` and `wm keyboard hide` do 1.2's rule 4 and 1.3's rule
  1. `show` with nothing focused fails with `not_applied`.

## 10. Accessibility

- **No timed gesture is the only way.** Long press (alternates, the layout menu, word
  selection) has taps that do the same: the accent keys, tapping through the layouts, Shift
  then arrows.
- **State is never colour alone**: a latch has its fill, its line and a changed mark or caps.
- **Contrast**: only the pairs of 2.2, all existing; `text.dim` is not used.
- **Targets**: every key is at least 44x44 except 4.1's exception, which passes WCAG 2.5.8.
- High contrast and the light theme repaint the panel through the palette; nothing here names
  a literal colour. Reduced motion has nothing to reduce.
- With a pointer every key is a button: hover, click, right click for the strip.

## 11. Rendering and lightness (D24)

- `layout(...)` also returns the panel's rect; the zones come from one function of the
  panel's size, the form, `n1`, `n2`, `n3` and whether key 17 shows.
- One panel buffer; the preview, the strip and the layout sheet are popups. A press damages
  one cap. Idle costs nothing: the only timers are a held key's repeat and the long press.
- A headless machine and the desktop form carry none of its state (D18).

## 12. Left out, on purpose

Prediction, correction and auto-capitals; swipe typing; a split or floating keyboard; the
on-screen keyboard in the desktop form (an accessibility item of its own); voice; emoji and
characters outside Latin-1 and `€` (Latin Extended-A waits for Egg Sans); a magnifier; key
sounds and vibration; a digit row above the letters; a shade tile to show the keyboard; the
compatibility layers' own text focus reports (their windows use `always` until then).

## 13. Acceptance (host tests, 390x844, scale 1x, dark palette, `shell.form` `phone`, `input.present` `touch`, `keyboard.layouts` `us`, Settings shown with a text field focused by a tap, unless said)

1. **Showing**: before the tap `wm keyboard` prints `hidden`. After a tap in Settings' search
   field it prints `shown 0 536 390 260`, `wm list` shows Settings at (0, 44, 390, 492), and
   the navigation bar's pixels are unchanged.
   With `input.present` `touch,keyboard` the same tap shows nothing; with `keyboard.on_screen`
   `always` it shows; with `never` it does not and `wm keyboard show` fails with `not_applied`.
2. **The panel**: pixel (0, 536) is `bar.edge`, (1, 581) `bar.bg`, (19, 582) `button.border`,
   (19, 584) `button.bg`; `q`'s glyph cell is (11, 591, 16, 32) in `text`; `a`'s is (30, 645,
   16, 32); `123` starts at (17, 761); Enter's cap at (351, 746) is `button.primary` and its
   word starts at (331, 761) in `icon.dark`.
3. **Zones**: a tap at (5, 650) types `a`; at (389, 650) `l`; at (38, 600) `q` and at (39,
   600) `w`; at (200, 760) a space; at (80, 760) `,`; at (290, 760) `.`. No key 17 is drawn or
   listed.
4. **Release-typing**: touch down at (50, 600) (`w`), move to (90, 600), up: the field holds
   `e`. Touch down on `w`, up at (200, 300): nothing typed. While down on `q` the preview is
   (0, 536, 47, 44) with the glyph at (15, 542); in a `masked` field no pixel of the keys row
   changes.
5. **Two contacts**: down on `t`, down on `h`, up, up types `th`.
6. **Shift**: a tap at (20, 710) fills the arrow, the caps read `Q` to `M`, and the cap is
   `selection` with the `accent` line; `a` then types `A` and the caps return. Two taps show
   the bar under the arrow and `abc` types `ABC`; a third tap returns to `off`. The tree's
   `Shift` reads `off`, `once`, `locked`.
7. **Pages**: a tap on `123` shows `1` to `0` in row 1, key 16 reads `ABC`, key 14 reads
   `#+=`; `3`, `+`, `4` type `3+4` and the page stays. `#+=` shows `[` first and the seven
   accent keys in row 3. `ABC` returns. A `number` field opens on `Numbers`.
8. **Backspace**: a press at (360, 710) deletes one character at the press; held, it deletes
   again after `keyboard.repeat_delay_ms` and then at `keyboard.repeat_rate`; sliding off
   stops it.
9. **Alternates**: `e` held 500 ms shows the strip (75, 536, 220, 44) with `è é ê ë €`; lifting
   leaves it; a tap at (140, 558) types `é` and closes it. With Shift on the strip reads `È É
   Ê Ë €`. `q` held shows no strip.
10. **Accent keys**: on `Symbols` a tap on `´` returns to `Letters`, the field shows the
    highlight of `keyboard-v0.3.md` 9.4, and `e` types `é`; `´` then Space types `´`; `´`
    then Backspace types nothing; `´` then `t` types `´t`. All on `us`, which has no dead key.
11. **Ctrl and arrows**: `Ctrl` then `a` selects the field's text and the latch drops; Shift,
    then Right twice from the start selects one character, then (Shift `off`) the caret moves.
    In the Terminal `Ctrl` then `c` sends byte 3; `Tab` and `Esc` send 9 and 27; Up sends the
    cursor-up sequence.
12. **Layouts**: `settings set keyboard.layouts us,de` draws key 17 at (58, 742, 44, 54)
    reading `US` and moves `,` to (102, 742, 39, 54) and Space to (141, 742, 132, 54). A tap
    on it makes `keyboard.layout` `de`: row 1 is 11 keys from (2, 580) ending in `ü`, row 3 is
    (55 + 35 x `i`, 688) reading `yxcvbnmß`, Shift is (0, 688, 55, 54); `z` types `z` from the
    sixth key; Shift then `ü` types `Ü`; the bar's indicator reads `DE`; no notice shows. A
    long press on 17 opens the sheet with two rows and `Keyboard settings...`.
13. **Kinds and Enter**: a `search` field's Enter reads `Search`; an `email` field's key 18
    types `@`; a text area's Enter adds a line and the keyboard stays.
14. **Hiding**: `Back` hides it, Settings is (0, 44, 390, 752) again with the field still
    focused, and the next `Back` goes to the window. The `Hide keyboard` key, a tap on a
    button in the window, `Home`, opening the shade and `wm keyboard hide` each hide it.
    Setting `input.present` to `touch,keyboard` hides it.
15. **The tree and D22**: `wm show shell` lists `On-screen keyboard` valued `letters us` with
    `q` ... `m`, `Space`, `Enter`, `Shift`, `Control`, `Numbers`, `Hide keyboard`; `wm act
    shell <q's id> invoke` types `q`. `wm type <id> héllo` types `héllo` with the keyboard
    shown, hidden and `never`, and changes no pixel of the panel.
16. **Handles**: a tap in the field draws the 12x14 `accent` handle with its point under the
    caret; dragging it 40 px moves the caret to the boundary nearest the new x; a typed key
    removes it. A long press on a word selects it and draws two handles.
17. **Landscape** (844x390): the panel is (0, 214, 796, 176), Settings is 796x170, `q` is (0,
    214, 72, 44), Enter (684, 258, 112, 44), `Esc` (72, 346, 48, 44), Space (264, 346, 292,
    44), `Hide keyboard` (748, 346, 48, 44), and no preview is drawn.
18. **Tablet** (1024x768, `tablet`): the panel is (0, 404, 1024, 300); two split windows are
    both 360 tall; `q` is (0, 448, 93, 64), Backspace (930, 448, 94, 64), Enter (883, 512,
    141, 64), Space (93, 640, 837, 64), `Alt` (279, 404, 93, 44). At 1366x1024: `q` (0, 704,
    124, 64), Enter (1178, 768, 188, 64), Space (124, 896, 1116, 64).
19. **TV** (960x540 logical, `tv`, `input.present` `remote`): OK on a focused field shows the
    panel (0, 336, 960, 204) and the window is 960x336; the focus is on `g` at (336, 384, 72,
    40) with the ring; Right, Right, OK types `j`; Up from row 0 changes nothing; Right from
    `p` goes to Backspace; Down from `p` goes to `l`; Menu on `e` opens its alternates; Back
    hides the keyboard and the next arrow moves the app's focus. Moving the app's focus onto
    a field by arrows shows nothing.
20. **2x and the desktop**: at 780x1688 device px and scale 2 every rect above doubles. At
    1280x800 in the desktop form a click in a field shows nothing in any mode.

## 14. Backend asks

For the kernel and the drivers:

1. **Keys from the panel.** The server sends the focused window `Key` events for the panel's
   keys by the two paths that exist: a key code with modifiers through `Typist::press` (a
   `Letters` key, the named keys), and a character as `SEND_INPUT`'s `CHARACTER` path sends it
   (signs, digits, alternates). The events carry no mark of where they came from. For a letter
   slot whose Shift level is not its capital, Shift is sent as `CAPS_LOCK`, which gives the
   capital by the existing rule.
2. **`SET_TEXT_FOCUS`**, a new window request: the window, the **kind** (`none`, `text`,
   `multiline`, `masked`, `number`, `email`, `url`, `search`, `terminal`), Enter's **word**
   (one of six) and the **cause** (`tap`, `activate`, `other`). Sent when a text widget takes
   or loses the focus, when its kind changes, and again with `tap` for each tap in a focused
   field. The server keeps the last one per window and forgets it when the window closes.
3. **Two contacts on the panel.** The touch path (parent ask 2) gives the shell's keyboard
   panel the second contact's down and up; windows still get one.
4. **An accent from the panel.** `Typist` takes a pending accent by its spacing character
   whatever the layout in force, and resolves it by its existing rules; the `DEAD` records go
   out as for a dead key.
5. **The panel in the layout function**, `Desktop::set_keyboard_shown`, the resize of the
   shown windows, `keyboard.on_screen` in the settings file, and `wm keyboard` with its MCP
   twin.

For the toolkit:

6. Fields, text areas and the Terminal send `SET_TEXT_FOCUS`. A field gains a kind (the
   `masked` flag becomes one of them) and an Enter word, both defaulting to today's behaviour.
7. The caret handle and the selection handles (section 7), drawn after touch input only, and
   the long press that selects a word.
8. On a TV, OK on a focused field sends the cause `activate` and does not otherwise change
   the field.

## Decided by design (the owner can overrule any of these)

- A server-drawn shell panel, not an app, sending ordinary `Key` events: no input-method
  protocol.
- Three pages, the same in every form; one set of sign pages for all 20 layouts; no digit row.
- `Letters` shows only the layout's letter keys, and Shift gives each one's capital, not the
  physical layout's second level where that differs.
- A keys row (`Esc`, `Tab`, `Ctrl`, arrows, `Hide keyboard`; `Alt` where there is room), so
  the Terminal works on a phone and selection needs no timed gesture.
- Phone portrait character keys are 39x54: the one exception to the 44 px target.
- Shift cycles `off`, `once`, `locked` by taps; Ctrl and Alt latch for one key.
- Accents two ways: hold for alternates, or seven accent keys that work in every layout; an
  open alternates strip stays open when the finger lifts.
- The layout key cycles on a tap with no hint; a long press opens the menu.
- Backspace and the arrows repeat at the physical keyboard's delay and rate; characters do not.
- `keyboard.on_screen` (`auto`, `always`, `never`) and `wm keyboard`; a field focused by its
  program or by Tab does not show the keyboard.
- Enter's label is one of six words; the field's kind picks the opening page and one key.
- The TV is a 12 by 4 grid with no wrap, starting on row 1, column 4; Menu opens alternates.
- One caret handle and two selection handles in `accent`, drawn by the toolkit; no magnifier.
- No motion, no new colour, no prediction or correction.
