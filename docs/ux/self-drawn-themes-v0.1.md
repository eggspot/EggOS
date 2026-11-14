# EggOS v0.1 the light theme and high contrast in what draws itself: the shell's panels, the terminal's grid, Files and the Text Editor: design brief

Design by Claude Fable (D16) for the part of D219 and D224 still open: the four palettes
(`DARK`, `DARK_HIGH_CONTRAST`, `LIGHT`, `LIGHT_HIGH_CONTRAST`, index `theme + 2 x
high_contrast`) exist in the toolkit, and `appearance.theme` and `accessibility.high_contrast`
repaint every toolkit window, but the shell's own panels, the terminal's grid, and Files' and
the Text Editor's own drawing still read `colour::` constants, which only know the dark
theme. This brief gives **every token's value in all four palettes**, the ANSI and syntax
tables each palette needs, and which token each self-drawn part uses, so one table drives all
of them. `desktop-v0.3.md` section 2, `desktop-v0.3-gui.md` section 1, `toolkit-v0.3.md`
section 1, `first-run-accessibility-v0.3.md` 2.4 (the dark high-contrast column) and
`toolkit-v0.5.md` 8.2 (the light and light high-contrast columns) stand; this brief repeats
their values in one place, fills the gaps and adds nothing that moves. Ratios are WCAG,
computed from the values.

## 0. What a person gets, and how that is checked

- `settings set appearance.theme light` turns **the whole screen** light within one frame:
  the bar, the dock, the taskbar, menus, notices, the switcher, the overview, the terminal's
  page, Files' sidebar and rows, the Text Editor's gutter and text, and every syntax colour.
- `accessibility.high_contrast true` does the same for the high-contrast variant of either
  theme: white on black, or black on white, every edge drawn, 7:1 wherever text rests.
- One palette crate feeds the toolkit, the shell, the terminal and the two apps; a host test
  computes the ratio of every pair named here from the table and fails under the floor.

## 1. One palette, four columns, one place

- **Crate `eggos-palette`** (`userland/palette`, `no_std`, no allocator, no dependencies; in
  the workspace and `USER_CRATES`): `Token` (every row of section 2), `Palette { get(Token)
  -> u32 }`, the four constants, `Ansi { normal: [u32; 8], bright: [u32; 8] }` per palette,
  `Syntax` (section 4) per palette, and `palette(index: u8) -> &'static Palette`. The
  toolkit's `theme.rs` re-exports it (`Token`, `Palette` keep their names, so nothing built on
  the toolkit changes); the drivers crate's `colour::` constants become `DARK`'s fields with
  the same names, kept for the boot screen (which precedes the settings file) and for tests.
- **The shell** (`drivers/src/desktop`): one `palette: &'static Palette` on `Desktop`, set at
  boot from the two keys the kernel already reads (D219, D224) and on `SET_PALETTE`; every
  `colour::X` in a paint path becomes `self.palette.get(Token::X)`. The repaint is the one
  full damage D219 gives.
- **The terminal** (the console's grid in the drivers crate and the Terminal window): `term.*`
  and the ANSI table from the palette; the grid repaints whole on a change (a cell keeps its
  ANSI index, not a colour, so a redraw recolours it: `terminal-v0.3.md`'s cell holds the
  attribute, which this confirms).
- **Files and the Text Editor**: their `View`s and `Custom` painters read `canvas.palette()`
  (D219's rule) instead of `colour::`; their `PaletteChanged` handling is `application::run`'s
  already. Files' icons and the editor's syntax read `palette.syntax()` and the icon tokens.
- The boot screen, `shell`, `unlit` and the window controls' three colours keep one value.

## 2. Every token in the four palettes

Columns: Dark (D) / Dark high contrast (DH) / Light (L) / Light high contrast (LH). "Floor"
is the pair the token rests text or a mark on, in every column: the test asserts ≥ 7:1 in DH
and LH, ≥ 4.5:1 in D and L for resting text, ≥ 3:1 for graphics and momentary fills.

| Token | D | DH | L | LH | Floor (D / DH / L / LH) |
|-------|---|----|---|----|-------------------------|
| `wallpaper.top` | `#0E1726` | `#000000` | `#DCE4EE` | `#FFFFFF` | icon labels `text`: 15.1 / 21.0 / 13.5 / 21.0 |
| `wallpaper.bottom` | `#1B3A5C` | `#000000` | `#B9CBE0` | `#FFFFFF` | `text` 9.3 / 21.0 / 10.4 / 21.0 |
| `wallpaper.egg` | `#1F3F63` | `#262626` | `#C7D6E6` | `#E6E6E6` | decoration |
| `accent` | `#F2B33D` | `#FFD24D` | `#9A6200` | `#7A4E00` | on `menu.bg` 9.5 / 14.6 / 4.7 / 7.2 |
| `shell`, `unlit` | `#F6F1E7`, `#26303F` | same | same | same | the boot screen |
| `bar.bg` | `#141B26` | `#000000` | `#E9EDF2` | `#FFFFFF` | `text` 14.3 / 21.0 / 14.7 / 21.0 |
| `bar.edge` | `#202938` | `#FFFFFF` | `#C9D0DA` | `#000000` | an edge |
| `bar.hover` | `#26303F` | `#262626` | `#DCE2EA` | `#E6E6E6` | `text` 11.0 / 15.1 / 13.3 / 16.8; `text.dim` 4.4 (never rests) / 8.3 / 4.5 / 7.9 |
| `bar.pressed` | `#334052` | `#404040` | `#D3DAE3` | `#D9D9D9` | `text` 8.7 / 10.4 / 12.3 / 15.0 (momentary) |
| `menu.bg` | `#1C2430` | `#000000` | `#F4F6F9` | `#FFFFFF` | `text` 12.9 / 21.0 / 16.0 / 21.0; `text.dim` 5.1 / 11.4 / 5.4 / 9.6 |
| `menu.border` | `#2A3340` | `#FFFFFF` | `#C9D0DA` | `#000000` | an edge |
| `menu.hover` | `#2A3440` | `#262626` | `#E3E8EF` | `#E6E6E6` | `text` 10.5 / 15.1 / 14.0 / 16.8 |
| `text` | `#E6EAF0` | `#FFFFFF` | `#161B22` | `#000000` | |
| `text.dim` | `#8A94A6` | `#B8C0CC` | `#5B6675` | `#3D4650` | on `field.bg` 6.3 / 11.4 / 5.8 / 9.6 |
| `title.active` | `#2A3340` | `#262626` | `#DCE2EA` | `#E6E6E6` | `text` 10.6 / 15.1 / 13.3 / 16.8 |
| `title.inactive` | `#1F2630` | `#000000` | `#ECEFF3` | `#FFFFFF` | `text.dim` 5.0 / 11.4 / 5.1 / 9.6 |
| `border.active` | `#3A4454` | `#FFFFFF` | `#9AA5B4` | `#000000` | an edge |
| `border.inactive` | `#2A3340` | `#8A94A6` | `#C9D0DA` | `#6B6B6B` | an edge |
| `button.close`, `.minimise`, `.maximise` | `#E5534B`, `#E8B33C`, `#3DBE5C` | same | same | same, each outlined 1 px `#000000` | graphics |
| `button.inactive` | `#4A5466` | `#8A94A6` | `#B0B9C6` | `#6B6B6B` | a graphic |
| `term.bg` | `#0B0E13` | `#000000` | `#FBFCFD` | `#FFFFFF` | |
| `term.fg` | `#D7DEE8` | `#FFFFFF` | `#1E2630` | `#000000` | 14.3 / 21.0 / 14.9 / 21.0 |
| `term.cursor` | `#F2B33D` | `#FFD24D` | `#9A6200` | `#7A4E00` | 10.4 / 14.6 / 5.0 / 7.2 |
| `term.selection` | `#2E4A6B` | `#153A6B` | `#BBD3F0` | `#CCE0FF` | `term.fg` 7.5 / 11.4 / 10.0 / 15.7 |
| `window.bg` | `#12161D` | `#000000` | `#FFFFFF` | `#FFFFFF` | no text |
| `selection` | `#2E4A6B` | `#153A6B` | `#CFE0F5` | `#CCE0FF` | `text` 7.5 / 11.4 / 12.9 / 15.7 |
| `focus.ring` | `#F2B33D` | `#FFD24D` | `#9A6200` | `#7A4E00` | ≥ 3:1 on every surface |
| `field.bg` | `#0B0E13` | `#000000` | `#FFFFFF` | `#FFFFFF` | `text` 16.0 / 21.0 / 17.3 / 21.0 |
| `button.bg` | `#2A3440` | `#1F1F1F` | `#FFFFFF` | `#FFFFFF` | `text` 10.5 / 16.5 / 17.3 / 21.0 |
| `button.border` | `#3A4454` | `#FFFFFF` | `#B8C1CC` | `#000000` | an edge |
| `button.primary` | `#F2B33D` | `#FFD24D` | `#F2B33D` | `#F2B33D` | `icon.dark` 10.4 / 14.6 / 10.4 / 11.3 |
| `button.hover` | `#344050` | `#333333` | `#F0F3F7` | `#E6E6E6` | `text` 9.4 / 12.6 / 15.5 / 16.8 |
| `button.pressed` | `#202833` | `#4D4D4D` | `#DCE2EA` | `#D9D9D9` | `text` 12.5 / 8.5 / 13.3 / 15.0 |
| `button.primary.hover` | `#F6C25C` | `#FFE080` | `#F6C25C` | `#F6C25C` | `icon.dark` 11.8 / 16.2 / 11.8 / 12.6 |
| `button.primary.pressed` | `#D99E2E` | `#E6B800` | `#D99E2E` | `#D99E2E` | `icon.dark` 8.2 / 11.2 / 8.2 / 8.8 |
| `progress.track` | `#26303F` | `#4D4D4D` | `#DCE2EA` | `#D9D9D9` | `accent` on it ≥ 3:1 |
| `dock.bg` | `#1C2430` | `#000000` | `#F4F6F9` | `#FFFFFF` | as `menu.bg` |
| `icon.dark` | `#0B0E13` | `#000000` | `#0B0E13` | `#000000` | glyphs on the yolk |
| `icon.tile` | `#26303F` | `#000000` | `#DCE2EA` | `#E6E6E6` | `text` 11.0 / 21.0 / 13.3 / 16.8 |
| `icon.tile.border` | `#3A4454` | `#FFFFFF` | `#B8C1CC` | `#000000` | an edge |
| `icon.prompt` | `#5FB865` | `#84D48A` | `#2E7D32` | `#1B5E20` | on the tile 6.3 / 11.8 / 3.9 / 5.9 |
| `icon.folder` | `#E2B341` | `#FFD24D` | `#E2B341` | `#E2B341` | a graphic |
| `icon.folder.back` | `#B88A24` | `#B38600` | `#B88A24` | `#B88A24` | |
| `icon.gear` | `#8A94A6` | `#FFFFFF` | `#5B6675` | `#000000` | on the tile 4.8 / 21.0 / 4.5 / 16.8 |
| `icon.gear.hole` | `#1F2630` | `#000000` | `#DCE2EA` | `#E6E6E6` | the tile shows through |
| `icon.windows` | `#4F8FE0` | `#0A3D8F` | `#2F6FD0` | `#1F4FA0` | `icon.badge` 4.6 / 10.1 / 4.9 / 7.3 |
| `icon.badge` | `#E6EAF0` | `#FFFFFF` | `#FFFFFF` | `#FFFFFF` | |
| `icon.mac` | `#C9D1DB` | `#FFFFFF` | `#C9D1DB` | `#FFFFFF` | `icon.dark` 12.5 / 21.0 / 12.5 / 21.0 |
| `icon.linux` | `#E2B341` | `#FFD24D` | `#E2B341` | `#E2B341` | `icon.dark` 9.9 / 14.6 / 9.9 / 10.6 |
| `icon.android` | `#3DDC84` | `#3DDC84` | `#0F6B3A` | `#0F6B3A` | on the tile 7.5 / 11.8 / 5.1 / 5.3 |
| `icon.page` | `#F6F1E7` | `#FFFFFF` | `#FFFFFF` | `#FFFFFF` | |
| `icon.page.edge` | `#C9D1DB` | `#B8C0CC` | `#9AA5B4` | `#000000` | an edge |
| `icon.page.line` | `#4A5466` | `#8A94A6` | `#5B6675` | `#000000` | a decoration |

New values in this brief (the rest are the two earlier tables, unchanged): `icon.badge` DH,
`icon.android` DH/L/LH, `icon.page`, `icon.page.edge`, `icon.page.line` L/LH, `icon.gear.hole`
LH, and the LH outline on the three window controls (8.2 named it; here it is a rule: a 1 px
`#000000` ring outside each circle, 14 px across). Every DH text pair is ≥ 8.3:1 and every
LH pair ≥ 7.2:1; the one resting pair under 7 in a high-contrast column is none.

## 3. The terminal's four ANSI tables (SGR 30-37 / 90-97; bold selects 8-15)

| Palette | | 0 | 1 red | 2 green | 3 yellow | 4 blue | 5 magenta | 6 cyan | 7 white |
|---------|---|---|---|---|---|---|---|---|---|
| Dark | normal | `#0B0E13` | `#E0565B` | `#5FB865` | `#E2B341` | `#4F8FE0` | `#C46BD2` | `#44B9C9` | `#C9D1DB` |
| | bright | `#707C8E` | `#F07A7E` | `#84D48A` | `#F2CC6D` | `#7FB0F0` | `#DA95E4` | `#72D3E0` | `#F2F5F9` |
| Dark HC | normal | `#000000` | `#FF6E6E` | `#84D48A` | `#F2CC6D` | `#7FB0F0` | `#DA95E4` | `#72D3E0` | `#F2F5F9` |
| | bright | `#B8C0CC` | `#FF9A9A` | `#A8F0AD` | `#FFE08A` | `#A8CCFF` | `#EAB8F0` | `#A0E8F0` | `#FFFFFF` |
| Light | normal | `#1E2630` | `#B3261E` | `#2E7D32` | `#8A5A00` | `#1F5FBF` | `#8E2EA0` | `#0B7285` | `#65707F` |
| | bright | `#4A5566` | `#C7342B` | `#2F8F3A` | `#9A6200` | `#2F6FD0` | `#9B3DAE` | `#157F95` | `#8A94A6` |
| **Light HC** (new) | normal | `#000000` | `#8B1A14` | `#1B5E20` | `#5C3C00` | `#153F8A` | `#5E1C6B` | `#064C5A` | `#3D4650` |
| | bright | `#1E2630` | `#9E1F18` | `#1C6021` | `#6E4700` | `#1A4FA3` | `#73258A` | `#085E6E` | `#5B6675` |

Light HC on `#FFFFFF`: normal 21.0, 9.3, 7.9, 10.0, 10.0, 11.3, 9.6, 9.6; bright 15.3, 7.9,
7.6, 8.2, 7.8, 8.9, 7.4 and **5.8 for bright white**, the one exception, mirroring the light
theme's: it is the colour programs use for "brightest", which on a white page must be the
faintest, and it stays readable. Bright is lighter than normal in every column, as a
terminal expects. ANSI 0 in the dark columns is `term.bg` itself (the deliberate exception of
`desktop-v0.3.md`); in the light columns black is black. A cell holds its ANSI index, so a
palette change recolours the scrollback too.

## 4. Syntax colours in the four palettes (the Text Editor, `text-editor-syntax-v0.1.md`)

The kinds keep their roles (blue structure, cyan names, green quoted data, magenta small
literals, yellow calls, grey comments); each palette takes its own ANSI bright row (dark) or
normal row (light), as the v0.1 brief chose, so the terminal and the editor still agree:

| Kind | Dark | Dark HC | Light | Light HC |
|------|------|---------|-------|----------|
| Plain, punctuation, operator | `text` `#E6EAF0` | `#FFFFFF` | `#161B22` | `#000000` |
| Function, macro | `#F2CC6D` | `#FFE08A` | `#8A5A00` | `#5C3C00` |
| Type, key, link | `#72D3E0` | `#A0E8F0` | `#0B7285` | `#064C5A` |
| String | `#84D48A` | `#A8F0AD` | `#2E7D32` | `#1B5E20` |
| Number, escape, attribute | `#DA95E4` | `#EAB8F0` | `#8E2EA0` | `#5E1C6B` |
| Keyword, tag, heading | `#7FB0F0` | `#A8CCFF` | `#1F5FBF` | `#153F8A` |
| Comment | `text.dim` `#8A94A6` | `#B8C0CC` | `#5B6675` | `#3D4650` |

On the field: dark 16.0, 12.5, 11.1, 10.8, 8.7, 8.6, 6.3; dark HC 21.0, 16.3, 15.3, 15.7,
12.6, 12.7, 11.4; light (on `#FFFFFF`) 17.3, 5.9, 5.6, 5.1, 6.9, 6.1, 5.8; light HC 21.0,
10.0, 9.6, 7.9, 11.3, 10.0, 9.6. On the selection: dark ≥ 3.0 (comments at the floor, as
before); dark HC on `#153A6B` 11.4, 8.8, 8.3, 8.5, 6.8, 6.9, 6.2; light on `#CFE0F5` 12.9,
4.4, 4.2, 3.8, 5.1, 4.5, 4.3; light HC on `#CCE0FF` 15.7, 7.5, 7.2, 5.9, 8.5, 7.4, 7.2. The
colour-blind rule holds in each column (no red; keyword, string and comment differ in
lightness); no kind ever uses `accent`.

## 5. Which token each self-drawn part uses

Nothing new is named: each part reads a token of section 2 (or a table of 3 or 4), so the
four columns are the whole design. Where a part today uses a constant for something the
table names otherwise, the row says so.

**The shell** (`drivers/src/desktop`):

| Part | Tokens |
|------|--------|
| top bar, taskbar, the phone's status and navigation bars, the TV's strip | `bar.bg`, `bar.edge`; items `text`; hover `bar.hover`, pressed `bar.pressed`; the egg `accent`; the clock `text` (`text.dim` for uptime); indicators (network, battery, volume, keyboard) `text`, `text.dim`, `accent` as their briefs say |
| chips (minimised windows, Agent, sticky keys, toggle keys, zoom) | `bar.hover` fill, `text`; locked sticky chips `accent` fill with `icon.dark` text; the Agent chip acting `accent` / `icon.dark` |
| dock, the Linux dash | `dock.bg`, `menu.border`; running mark `text`; hover `bar.hover`, pressed `bar.pressed` |
| menus (power, battery, volume, window, wallpaper, context), the launcher, the shade, tooltips | `menu.bg`, `menu.border`, `menu.hover`; text `text`, disabled and facts `text.dim`; the launcher's field `field.bg` with `button.border`; the selected row `selection` with the ring `focus.ring` |
| the notice, the countdown, the volume bar, the caption panel | `menu.bg`, `menu.border`, `text`, `text.dim`; the bar's track `progress.track` and done part `accent` |
| the switcher | `dock.bg` surface, rows `text` (minimised `text.dim`), the selected row `bar.pressed` with `focus.ring` |
| the overview and the desktops strip | the wallpaper tokens; tiles and thumbnails outlined `border.inactive`, the current and selected `border.active` and `focus.ring`; labels `text` and `text.dim`; close circles `button.close`; the drag outline `accent`; the empty desktop's line `text.dim` |
| the screenshot picker | the outline and the size pill `accent` and `menu.bg`; the hint pill as a notice |
| window chrome | `title.active`, `title.inactive`, `border.active`, `border.inactive`, the three controls, `button.inactive`; title text `text` / `text.dim`; the phone's title strip as the bar |
| the wallpaper and desktop icons | `wallpaper.*`; icon labels `text`, selected `selection`; the icon set's tokens |
| the shadow | `half`, `quarter`, `eighth` of what is under it; on a black high-contrast ground it is invisible, which is why the edges are drawn in white there |

**The terminal** (the console's grid and the Terminal window): `term.bg`, `term.fg`,
`term.cursor` (the glyph under it in `term.bg`), `term.selection`, the scrollback mark
`accent`, and the ANSI table of section 3; the window's 8 px padding is `term.bg`.

**Files** (its `View`s and painters; the constants its code reads today map one to one):

| Part | Tokens |
|------|--------|
| toolbar and status bar | `bar.bg`, `bar.edge`; `<` and `^` in `text`, dimmed `text.dim`; hover `bar.hover`; the path field `field.bg` |
| sidebar (`Places`) | `dock.bg`, `menu.border`; heading `text.dim`; rows `text`; the current place `selection` |
| the list | surface `window.bg`; header `text.dim` with a `bar.edge` line; rows `text`; sizes and times `text`; hover `menu.hover`; selected `selection` with `>` `accent` and `focus.ring`; the type marks `icon.folder`, `icon.folder.back`, `icon.page`, `icon.page.edge`, `icon.page.line`; source badges `icon.windows`, `icon.mac`, `icon.linux`, `icon.android` |
| the scroll bar | `progress.track`, thumb `bar.pressed`, hovered `button.inactive` |
| dialogs and context menus | the toolkit's |

**The Text Editor**:

| Part | Tokens |
|------|--------|
| toolbar, find bar, status bar | `bar.bg`, `bar.edge`, `text`, `text.dim`, hover `bar.hover`, pressed `bar.pressed` |
| gutter | `menu.bg`; line numbers `text.dim`, the caret's line's number `text` |
| text view | `field.bg`; glyphs by the syntax table of section 4 (`text` when colours are off); the caret `text`; the selection `selection` (glyphs keep their kind's colour); the find match `accent` outline |
| scroll bars | `progress.track`, `bar.pressed`, `button.inactive` |

## 6. Every device class (D23)

The phone's status bar, shade, sheets and navigation bar, the tablet's split and the TV's
home screen draw with the same tokens (their briefs name them), so the four columns cover
them without a line of their own; the TV's `text.dim` on `dock.bg` tiles is 5.1:1 light,
9.6:1 light HC. The wearable's read-only face (later) will take the same table.

## 7. Acceptance (host tests)

- The palette crate: `palette(i).get(token)` for the four indexes equals section 2; a test
  computes every ratio named in sections 2 to 4 and asserts the floors (7.0 in DH and LH
  resting pairs, 4.5 in D and L, 3.0 for graphics and momentary fills, the stated exceptions
  exempt by name); the ANSI tables and the syntax table per palette.
- The shell's paint of the bar, a focused and an unfocused window, the launcher, the power
  menu, a notice, the switcher and the overview with `SET_PALETTE` 2 (light): pixel (0, 0)
  `#E9EDF2`, the bar's row 27 `#C9D0DA`, the focused border `#9AA5B4`, a menu's fill `#F4F6F9`;
  with index 3: `#FFFFFF`, `#000000`, `#000000`, `#FFFFFF`; the dark high-contrast values of
  the first accessibility brief's acceptance unchanged (index 1); every frame equals the
  host's render (hash) in each palette.
- The terminal with index 2: default foreground `#1E2630` on `#FBFCFD`, ANSI 1 `#B3261E`,
  bright 7 `#8A94A6`; index 3: `#000000` on `#FFFFFF`, ANSI 1 `#8B1A14`, bright 15 `#5B6675`;
  a palette change recolours an existing cell of ANSI 4 to the new table.
- Files with index 2: the sidebar `#F4F6F9`, a selected row `#CFE0F5` with `>` `#9A6200`, a
  folder mark `#E2B341` over `#B88A24`, a page outline `#9AA5B4`; index 3 the LH column.
- The Text Editor with index 2: the gutter `#F4F6F9`, numbers `#5B6675`, the caret's `#161B22`,
  a keyword `#1F5FBF` on `#FFFFFF`, a comment `#5B6675`; index 3: `#153F8A` and `#3D4650` on
  `#FFFFFF`; `View > Syntax Colours` off draws `#000000`.
- No `colour::` constant is read in a paint path of the shell, the terminal, Files or the
  editor outside the boot screen (a lint test over the sources).

## Decided by design (the owner can overrule any of these)

One palette crate shared by the toolkit, the shell, the terminal and the apps, with the dark
constants kept only for the boot screen and tests; the light high-contrast ANSI rows are a
darkened pair with bright white the one exception under 7:1; the syntax table takes each
palette's own ANSI row so the editor and the terminal agree; the window controls keep their
three colours and gain a black ring in light high contrast; the shadow stays a darkening and
the edges carry the high-contrast cue; no new token.

## Later

A dark terminal in a light desktop (`term.*` by their own key); following the time of day or
ambient light; the Windows and macOS layers' light and high-contrast system colours; a
user-chosen accent; per-app themes.

## Decisions for the engineer

1. Crate `eggos-palette` with `Token`, `Palette`, `DARK`, `DARK_HIGH_CONTRAST`, `LIGHT`,
   `LIGHT_HIGH_CONTRAST`, `Ansi`, `Syntax`, `palette(index)`; index `theme + 2 x
   high_contrast`; the toolkit re-exports; `colour::` constants = `DARK`'s fields.
2. The 54 token rows of section 2 with their four values; the new values: `icon.badge` DH
   `#FFFFFF`; `icon.android` DH `#3DDC84`, L and LH `#0F6B3A`; `icon.page` L/LH `#FFFFFF`;
   `icon.page.edge` L `#9AA5B4`, LH `#000000`; `icon.page.line` L `#5B6675`, LH `#000000`;
   `icon.gear.hole` LH `#E6E6E6`; window controls in LH ringed 1 px `#000000`.
3. ANSI light HC: normal `#000000 #8B1A14 #1B5E20 #5C3C00 #153F8A #5E1C6B #064C5A #3D4650`,
   bright `#1E2630 #9E1F18 #1C6021 #6E4700 #1A4FA3 #73258A #085E6E #5B6675`; the other three
   tables as section 3.
4. Syntax per palette as section 4 (dark and dark HC from the bright rows, light and light HC
   from the normal rows; comments `text.dim`; plain `text`).
5. The token map of section 5 for the shell, the terminal, Files and the editor; cells keep
   ANSI indexes; a palette change is one full damage per surface.
6. Floors for the ratio test: 7.0 (DH, LH resting), 4.5 (D, L resting), 3.0 (graphics,
   momentary fills); exceptions by name: ANSI 0 in the dark columns, bright white in the light
   columns (3.0 L, 5.8 LH), bright green 4.0 in L.
