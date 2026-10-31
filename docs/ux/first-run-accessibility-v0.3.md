# EggOS v0.3.0 first-run setup and the accessibility baseline: design brief (part 50)

Design by Claude Fable (D16) for part 50 of the plan: the first boot's setup (UX README
principle 2) and the accessibility baseline the README promised for the first desktop
milestone (principle 4: keyboard-only use, high contrast, reduced motion, sticky and slow
keys). Both are GUI front-ends of settings (D22): the first run writes `shell.profile`,
`keyboard.layout` and `time.zone`, which `settings set` and `settings_set` set already, and
the new `Accessibility` pane writes five new `accessibility.*` keys, which the same CLI and MCP
tools get in the same PR. Everything in `toolkit-v0.3.md` (section 11 for Settings),
`toolkit-v0.4.md` (slider, dropdown), `desktop-v0.3-gui.md` (section 10, the Appearance
cards; section 14.2, the first boot) and `date-time-v0.3.md` (section 8, the zone picker)
stands except where a section below says otherwise. Numbers are px at 1x and double at 2x;
the only motion is the caret's blink, and section 2.5 says when even that stops. "Later"
marks what 0.3.0 does not build.

## 0. What a person gets, and how that is checked

- **First run**: on the first boot of a fresh disk a window asks three questions, `Where are
  you coming from?`, the keyboard layout and the time zone, with `Back`, `Next`, `Skip`, and
  the desktop behind it already shows each answer. It is asked once; `settings --first-run`
  asks again on request.
- **Accessibility**: a Settings pane with four toggles and a slider: `High contrast` (a second
  value for every colour token, light text on black, every edge drawn, 7:1 or better wherever
  text rests), `Reduce motion` (nothing blinks), `Sticky keys` (tap a modifier, then the key;
  tapped twice it locks; the bar says which are held), `Slow keys` with `Hold a key for`
  (100 to 1000 ms), and the line that says full keyboard navigation is always on and how the
  bar and the dock are reached (`Ctrl+Alt+Tab`).
- Host tests (section 3) pin every rect, write and line; `wm show` (sections 1.8 and 2.11)
  lists each control so an agent reads the state without the screen, and `settings list`
  prints the same values.

## 1. First run

### 1.1 When it shows, and who starts it

`init` (userland/init) starts `/bin/settings --first-run` right after it spawns the login
program, without waiting for it, when a disk is attached and
`/home/root/.config/eggos/settings` holds no `shell.profile` (a missing file counts): the
rule of `desktop-v0.3-gui.md` section 14.2, whose `--welcome` this mode is, renamed because
it sets things up rather than greets, and recorded in D-decisions as designed but not built.
With `--no-disk` nothing opens: the answers could not reach the next boot, and the shell's
default profile (macOS habits, section 11 of `desktop-v0.3-gui.md`) is one Settings click
away. Without a display (`WindowServer::open` fails) the program says `settings: no display`
on stderr and exits 1, as `settings` alone does. The window appears after the Terminal's
first prompt, which eggsh prints in its first milliseconds, on top of it and focused as every
new window is (section 3.2 of `desktop-v0.3-gui.md`).

### 1.2 The window

A `Normal` window of the settings program (`userland/settings/src/first_run.rs`,
`FirstRunWindow`, its own `WidgetTree<CAPACITY>`), title `Welcome to EggOS`, content
**640x440** (the Settings size, so one `CAPACITY` serves both), flag `FIXED_SIZE`, placed by
`SetPosition` at the work area's centre (`GetWorkArea`; outside 642x470, so (319, 143) in the
macOS profile at 1280x800). **Not full-screen, and not a dialog**: the desktop behind the
window is the preview. A card click re-lays the panels in the same frame (the Appearance
pane's rule, toolkit section 11), so the person watches their taskbar or dock appear while
the window stays put, clamped into the new work area as every window is. A full-screen
welcome would hide the one thing the first question is about. On a phone the role maximises
it anyway (section 1.6). The close control is `Skip` (1.4).

### 1.3 The three pages (window content coordinates, 640 wide, so the right-aligned edge is
x 624; rows of the 40 px grid from y 64, text cell top at row + 12, controls at row + 6)

One page shows at a time, as Settings shows one pane: each page is a container the window
shows or hides. Shared widgets, on every page:

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| S1 | heading | (16, 16), 2x | the page's heading |
| S2 | label, right-aligned | ends at x 624, cell top 24 | `Step 1 of 3`, `Step 2 of 3`, `Step 3 of 3` in `text.dim` (56 px, so it starts at 568; the widest heading, `Where are you coming from?`, ends at 306) |
| S3 | label | (16, 356) | the status line (1.4): `Every choice can be changed later in Settings.` in `text.dim` (234 px) |
| S4 | push button | (16, 396), 96x28 | `Skip` |
| S5 | push button | (424, 396), 96x28 | `Back`; disabled on page 1 |
| S6 | push button, primary | (528, 396), 96x28 | `Next`; `Done` on page 3 |

`Back` and `Next` are 8 px apart and right-aligned, the default one last, as a dialog's row
(toolkit section 4); `Skip` sits at the left so it is never pressed for `Next`. The buttons'
row ends at 424, 16 px above the bottom; the status line's cell ends at 372, 24 px above the
buttons.

**Page 1, `Where are you coming from?`** (the Appearance pane's question, principle 2):

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1.1 | card picker | (112, 64): three cards 128x112, 16 apart (so x 112, 256, 400; ends at 528) | `Windows` `macOS` `Linux`, the miniatures of section 10 of `desktop-v0.3-gui.md`; the profile in force selected |
| 1.2 | label | (16, 204) | `Primary modifier` in `text` |
| 1.3 | label, right-aligned | ends at x 624, cell top 204 | `Ctrl` / `Cmd (Super key)` / `Ctrl` in `text` |
| 1.4 | label | (16, 244) | `Window controls` in `text` |
| 1.5 | label, right-aligned | ends at x 624, cell top 244 | `Right` / `Left` / `Right` in `text` |
| 1.6 | label | (16, 284) | `The desktop behind this window already shows your choice.` in `text.dim` (302 px) |

The cards are centred (416 px in 640) rather than at x 16, because this page is a question,
not a pane with a sidebar; the two read-only rows are the Appearance pane's at its y (192 and
232, text at 204 and 244) so the same code lays them out.

**Page 2, `Keyboard`**:

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 2.1 | label | (16, 76) | `Layout` in `text` |
| 2.2 | dropdown | (424, 70), 200x28 | name `Layout`; the 14 titles of `keyboard-v0.3.md` section 2 (`LAYOUT_TITLES`, in that order); the saved layout selected |
| 2.3 | text field | (16, 120), 304x28 | placeholder `Type here to try it`, `escape: Clear`; empty when the page is shown |
| 2.4 | label | (16, 164) | the third-level line of the Keyboard pane (`third_level_text`) in `text` |
| 2.5 | label | (16, 184) | the dead-keys line (`dead_keys_text`) in `text` |
| 2.6 | label | (16, 204) | the keyboard line: `Caps Lock is on` or `No keyboard attached.`, shown as the Keyboard pane shows its widget 7; hidden otherwise |

A dropdown, not the pane's 240 px list: fourteen options is what `toolkit-v0.4.md` section 3
made it for, and the page stays one screen. Its popup opens below: x 412, 224 wide, y 102,
eight rows (258 tall, bottom 360, inside 440), scrolling to the last six. Choosing writes and
applies the layout at once (1.4), so the try field types in it in the same frame; the two
fact lines are retitled from the table, as the pane's are.

**Page 3, `Time zone`** (the picker of `date-time-v0.3.md` section 8, moved to the page's
grid; nothing else from that pane):

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 3.1 | label | (16, 76) | `Time zone` in `text` |
| 3.2 | label, right-aligned | ends at x 624, cell top 76 | the zone's name, `UTC`, `Europe/Paris`, in `text` |
| 3.3 | label, right-aligned | ends at x 624, cell top 96 | the facts line of section 8.1 (`UTC+01:00 CET, standard time`) in `text.dim` |
| 3.4 | text field | (16, 120), 304x28 | placeholder `City, region or abbreviation`, `escape: Clear`; empty when the page is shown |
| 3.5 | label, right-aligned | ends at x 624, cell top 126 | the match count (`2 zones`, `32 of 144`) in `text.dim`; hidden while the field is empty or nothing matches |
| 3.6 | list, 3 rows of 24, mark x 8, text x 24 | (16, 156), 608x72 | 32 static rows whose text is empty; `set_shown_items` to the rows shown; empty text `No zone matches`; the chosen zone always first and selected (8.3) |
| 3.7-3.9 | labels | (40, 160), (40, 184), (40, 208) | the three visible rows' texts in `text` |
| 3.10 | label | (16, 244) | `Now` in `text` |
| 3.11 | label, right-aligned | ends at x 624, cell top 244 | the local time of section 8.1, `Thursday 29 October 2026, 15:05:09 CET` (widest 259 px), refreshed once a second, damaging its rect only |

The `Now` row is the feedback a city click gives on macOS: the clock changes under the
choice. The search, the matching, the count, the pinned first row and the per-step write are
section 8.3 and 8.4 unchanged.

### 1.4 What is saved, when; `Skip`, `Back`, `Next`, `Done`, close

- **Every choice is saved the moment it is made**, exactly as the Settings panes save: a card
  (`Response::Chosen` on 1.1) is `Request::SetProfile(index)`, saved and applied at once;
  the dropdown (`Chosen` on 2.2) is `SetKeyboardLayout(index)`, saved and applied through
  `SET_KEYBOARD_LAYOUT`; a zone row (`Chosen` on 3.6) is `SetTimeZone(index)`, saved. The
  window never holds an answer back for a final `Done`: a person who closes the window after
  the first page keeps the profile they chose, and nothing is lost to a crash or a power-off.
- **`Next` and `Back`** show the next or previous page and write nothing. Showing a page
  resets its field to empty and pins the chosen zone first (page 3), as showing a pane does.
- **`Done`, `Skip` and the close control** end the window. Before it closes, if the settings
  file still holds no `shell.profile` (the person chose no card), the window writes the
  profile in force (`SetProfile` of the current one) so that the question is asked once, ever:
  this answers the UX README's open question "Default profile when the user skips first-run
  setup": the default is **the profile already in force**, macOS habits in 0.3.0 (section 11),
  which the person has been looking at behind the window (decided by design; the README's
  line should be closed by the build, and the owner can overrule it). `Done` and `Skip` differ
  in nothing else: `Skip` exists so the person knows that leaving is allowed.
- **A failed write** chooses nothing, puts the control back (the card to the profile in force,
  the dropdown to the saved layout, the list to the chosen zone's row) and the status line
  (S3) says why in `text`, with the panes' own lines: `Not allowed: this user may not set the
  profile.`, `... the keyboard layout.`, `... the time zone.` for `EPERM` and `EACCES`; `No
  disk attached: the profile could not be saved.` (and the two others) for every other errno.
  The next successful write, or showing another page, puts `Every choice can be changed later
  in Settings.` back.
- **Without a disk** (`on_disk` false; only when started by hand, since init does not start it
  then) the status line reads `No disk attached: these choices last until power-off.` in
  `text.dim` from the start, and the choices still apply for the session.
- **Re-running**: `settings --first-run` from the Terminal or the launcher's search, any time,
  shows the same window with the saved answers selected; it asks nothing it did not ask the
  first time. There is no `Run setup again` button in Settings: the Appearance, Keyboard and
  Date & time panes are the same three questions. Deleting the settings file and rebooting
  also asks again (1.1's rule), which is how a test does it.

### 1.5 Keys, focus and pointer

- **Initial focus**: page 1 the cards, page 2 the dropdown, page 3 the zone field; `Next`
  or `Back` moves the focus to the new page's first widget (the window says otherwise, as
  toolkit section 3 allows).
- **Tab order**, each page: its content widgets in table order, then `Skip`, `Back` (skipped
  while disabled), `Next`/`Done`, then wraps; Shift+Tab the reverse.
- **Enter** reaches the default button (`Next`, `Done`) from every widget that does not use
  it: the cards use it (Enter selects the focused card, toolkit section 3), a closed dropdown
  opens on it (toolkit v0.4 section 3.4), the try field sends it to the default button (a
  single-line field's Enter), the zone field moves the focus to the list and never reaches
  `Done` (date-time 8.6, so Enter in the field cannot end the setup with a half-typed city),
  and **Enter on a zone row is the row's open action, which this window defines as `Done`**
  (the row is already chosen by being selected). Space selects a card, opens the dropdown,
  activates a button.
- **Esc** goes to the window, which ignores it (`Normal`): closing is the close control,
  `Skip` or `Done`, never a stray Esc.
- **Keyboard-only from a cold boot**: the window opens focused; Right, Space (the card), Tab,
  Tab, Enter (`Next`; `Back` is disabled and skipped); Down to a layout, Tab four times to
  `Next`, Enter; `par`, Down, Down (Paris), Enter (`Done`). Fourteen keys and one word; a
  pointer is never required (section 5.3 of `desktop-v0.3-gui.md`).
- **Pointer**: `IBeam` over the two fields, `Arrow` elsewhere; a click on a row's label
  reaches the list through the label; a right-click does nothing.

### 1.6 Every device class (D23)

- **One column** (content narrower than 448: a phone's maximised window): the step label
  moves under the heading to (16, 56) and the content starts at y 88; the cards stack
  vertically, centred, 16 apart (at 360 wide: x 116, y 88, 216, 344), the two rows follow 16
  under the last card (text at 484 and 524) and line 1.6 at 564; page 2's dropdown goes under
  `Layout` at (16, 112), the width minus 32, the field at (16, 156), the lines from 204; page
  3's name and facts go under `Time zone` at x 16 (cells 112 and 132), the field at (16,
  156), the count hidden, the list at (16, 192), the `Now` row at 284. The page scrolls in a
  scroll view, the focused widget kept in view. The buttons are a bottom block: `Skip` and
  `Back` side by side, each `(width - 40) / 2` wide and 40 tall, `Next`/`Done` full width
  minus 32, 40 tall, under them 8 px apart, 16 px above the bottom, nearest the thumb; the
  phone's `Back` input is `Skip` only on page 1 and `Back` on the others.
- **Touch**: buttons and list rows 40 tall (rows at 156, 196, 236; the `Now` row at 292 in the
  wide layout), the fields 28 as everywhere; a tap on a card chooses it.
- **TV (scale 2)**: everything doubles; the ring on the card, the dropdown, the field and the
  buttons is the cue; the remote's arrows, OK and Back drive every page.
- **Headless**: no window (1.1); `settings set shell.profile|keyboard.layout|time.zone` and
  `settings_set` are the whole capability. A server install is set up by the CLI line it was
  going to run anyway.

### 1.7 CLI and MCP (D22)

The first run adds **no command and no tool**: it is a GUI way of setting three keys that
`settings set shell.profile macos`, `settings set keyboard.layout de`, `settings set time.zone
Europe/Paris` and `settings_set` set already, and `settings get` and `settings list` show.
`settings --first-run` is a mode of the program, like `--grant-prompt` and `--apps`, not a
capability of its own: an agent that wants a machine set up calls `settings_set` three times.

### 1.8 What an agent sees (D161, `wm show`)

A window `Welcome to EggOS`; a `radio group` named `Where are you coming from?` with `radio`s
`Windows`, `macOS`, `Linux`, the chosen one `selected` (the card picker's existing
description); a `combo-box` named `Layout` valued by the chosen title with its 14 `list-item`s;
a `text field` `Type here to try it`; a `text field` `City, region or abbreviation` and a
`list` `Time zones` as section 8.8 of `date-time-v0.3.md`; `button`s `Skip`, `Back`
(`disabled` on page 1) and `Next` or `Done`; the step, the two rows, line 1.6, the `Now` row
and the status line `label`s with their drawn text. Nothing is person-only: `Select` on a
card, an option or a row is the same write `settings_set` makes, and `Invoke` on `Next` only
turns the page.

## 2. The Accessibility pane

### 2.1 In the sidebar

- **Name** `Accessibility`: the word all three systems use today (macOS `Accessibility`,
  Windows 11 `Accessibility`, GNOME `Accessibility`; the older names are search words). The
  row goes **right after `Mouse`**, index 4: `Appearance`, `Display`, `Keyboard`, `Mouse`,
  `Accessibility`, `Storage`, `Date & time`, `Agents`, `Components`, `About`. Three of its
  five settings change what the keys do and one changes what the screen shows, so it belongs
  with the input and display panes, where macOS and GNOME also keep it near Keyboard and
  Mouse, not at the end with the facts. Its row is y 172..203 (44 + 4 x 32); `Storage`,
  `Date & time`, `Agents`, `Components` and `About` each move down 32 (204, 236, 268, 300,
  332; `STORAGE` 5, `DATE_AND_TIME` 6, `AGENTS` 7, `COMPONENTS` 8, `ABOUT` 9). Ten rows are
  320 px: at the default 440 they fit the list's 396; at the 360 minimum the list is 316 tall
  and scrolls with the 8 px bar of toolkit section 8, which the Agents brief foresaw for the
  tenth row.
- **Search words**: `Ease of Access` (Windows 10), `Universal Access` (GNOME 3), `a11y`,
  `contrast`, `High contrast`, `Increase contrast` (macOS), `Contrast themes` (Windows 11),
  `Reduce motion`, `Reduced motion`, `animations`, `Visual effects`, `Sticky keys`, `Slow
  keys`, `Filter keys` (Windows), `modifier keys`, `Full Keyboard Access` (macOS), `keyboard
  navigation`, `focus`, `latch`, plus the five setting names (`High contrast`, `Reduce
  motion`, `Sticky keys`, `Slow keys`, `Hold a key for`). Collisions, all acceptable: `keys`
  finds Keyboard (`dead keys`) too; `modifier` finds Appearance (`Primary modifier`) too;
  `keyboard` finds Keyboard first and this pane second. `contrast`, `motion`, `sticky`,
  `slow`, `filter`, `a11y` and `access` find this pane alone. Not `Narrator`, `VoiceOver`,
  `Orca`, `Zoom`, `Magnifier`, `Bounce keys` or `Large text`: a search word must lead to the
  thing, and those things are not built; each joins the list with its feature.

### 2.2 Layout (pane coordinates: origin window x 176, y 0; 464 wide at the default size, so
the right-aligned edge is x 448; usable height to y 332)

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Accessibility` |
| 2 | checkbox | (16, 70), 91x28 (`Length::Content`: 24 + 67) | `High contrast`; checked for `accessibility.high_contrast=true` |
| 3 | label, right-aligned | ends at x 448, cell top 76 | `White on black, every edge drawn.` in `text.dim` (173 px) |
| 4 | checkbox | (16, 110), 96x28 | `Reduce motion` |
| 5 | label, right-aligned | ends at x 448, cell top 116 | `No blinking, no animation.` in `text.dim` (123 px) |
| 6 | checkbox | (16, 150), 80x28 | `Sticky keys` |
| 7 | label, right-aligned | ends at x 448, cell top 156 | `Tap a modifier, then the key; twice locks it.` in `text.dim` (216 px) |
| 8 | checkbox | (16, 190), 73x28 | `Slow keys` |
| 9 | label, right-aligned | ends at x 448, cell top 196 | `A key counts only when held this long.` in `text.dim` (192 px) |
| 10 | label | (40, 236) | `Hold a key for` in `text` (indented by the mark's 24 px: it belongs to `Slow keys`) |
| 11 | slider | (232, 230), 216x28 | name `Hold a key for`, 100 to 1000 by 50, unit ` ms`, readout; the saved `accessibility.slow_keys_ms`; disabled while `Slow keys` is off |
| 12 | label | (16, 276) | `Full keyboard navigation` in `text` |
| 13 | label, right-aligned | ends at x 448, cell top 276 | `Always on` in `text` |
| 14 | label | (16, 296) | `Tab reaches every control; Ctrl+Alt+Tab reaches the bar and the dock.` in `text.dim` (359 px) |
| 15 | label | (16, 316) | the status line (2.3), `text.dim` or `text`; hidden when it has nothing to say |

Six grid rows (64 to 264) and the two lines under them fill the pane exactly to 332; the
status line is at the three input panes' no-disk y. Each setting is a checkbox whose label is
the setting's name, with its one-line meaning right-aligned on the same row in `text.dim`: the
GNOME and Windows shape (name, description, switch) folded into one 40 px row, since every
description fits beside its name (the widest pair, `Sticky keys` 96 and its line 216, leaves
136 px between them). A checkbox rather than a right-aligned switch because the toolkit has
a checkbox and the whole row (mark and label) is its hit target; a switch is a drawing of a
checkbox. The slider is the Mouse pane's geometry (`toolkit-v0.4.md` section 2.2: track
232..392, 18 steps over 144 px, 8 px a step, readout 400..448, `1000 ms` 42 px fits).

### 2.3 Behaviour

- **The current values** are `Facts::accessibility` (an `AccessibilityPreferences`:
  `high_contrast`, `reduce_motion`, `sticky_keys`, `slow_keys`, `slow_keys_ms`) when the
  window opens and whenever the pane is shown; an open window follows values set from the
  CLI, MCP or the Shift-tap toggle (2.6) the next time it is focused, as it follows
  `keyboard.layout`.
- **Toggling a checkbox** (`Response::Toggled`) or **adjusting the slider**
  (`Response::Adjusted`, once per new value while dragging) is the change: the pane asks the
  program for one `Request::SetAccessibility(AccessibilityChange)` with `HighContrast(bool)`,
  `ReduceMotion(bool)`, `StickyKeys(bool)`, `SlowKeys(bool)` or `SlowKeysMs(u32)`, which the
  program **saves, then applies** (`save_accessibility`, then `apply_accessibility` with the
  changed preferences): the same write and the same call as `settings set
  accessibility.sticky_keys true`. Applying is instant and silent; the control shows the new
  value in the same frame; toggling `Slow keys` enables or disables the slider in the same
  frame.
- **How a change is applied** (`apply_accessibility`): one `/dev/window` request,
  `SET_ACCESSIBILITY` (the five values in one record, like `SET_KEYBOARD_LAYOUT`), which the
  window server passes on: the palette to the shell (2.4), the blink to the shell and every
  client (2.5), sticky and slow keys to the kernel's input layer (`kernel/src/input.rs`,
  beside `set_preferences`, 2.6 and 2.7). Without a display, `settings set` saves and the
  kernel still applies sticky and slow keys to the console's keyboard: they are input
  settings, not display settings, and a person on a serial console needs them too.
- **At boot** the kernel reads the five keys from the settings file with `shell.profile` and
  `keyboard.layout` (docs/settings.md), before the desktop starts: the desktop's first frame
  is already in high contrast and the first key already latches. The boot screen draws before
  any file is read and keeps the standard palette.
- **A failed save** changes nothing: the control goes back (the box to the saved state, the
  thumb to the saved value), nothing is applied, and the status line says why. A save that
  worked but an apply the window server refused keeps the control (saved for the next boot)
  and says so.
- **The status line** (widget 15), the Mouse pane's lines worded for this pane:

  | When | Text | Colour |
  |------|------|--------|
  | shown, no disk | `No disk attached: this choice lasts until power-off.` | `text.dim` |
  | shown, with a disk | hidden | |
  | save refused (`EPERM`, `EACCES`) | `Not allowed: this user may not change the accessibility settings.` (323 px) | `text` |
  | save failed otherwise | `No disk attached: the accessibility settings could not be saved.` (317 px) | `text` |
  | saved, the window server refused the apply | `Saved for the next boot, but the window server did not take it.` | `text` |

  Showing the pane again restores the "shown" state.

### 2.4 High contrast: a second value for every token

`accessibility.high_contrast=true` gives **every colour token a second value** and changes
nothing else: no layout, no stroke width, no font. The standard palette's 1 px borders exist
already on every button, field, window, menu, tile and bar edge; high contrast makes them
white, so every edge is drawn, and puts white text on black, so every resting text pair is
**7:1 or better (WCAG AAA)**; hover and pressed fills, which last a moment, keep at least
4.5:1 (AA) where `text.dim` lands on them. The theme is the dark one lifted to its extremes
(Windows's `Night sky` and macOS's `Increase contrast` on the dark appearance do the same);
a light high-contrast theme comes with the light theme (toolkit v0.4 section 9). The shell
(`drivers/src/desktop`), the toolkit (`userland/toolkit/src/colour.rs`), the terminal and the
boot screen's constants become one `Palette` with `STANDARD` and `HIGH_CONTRAST` rows,
indexed by the setting: a client learns the palette from `GetWorkArea` (one more byte beside
the scale) and is told of a change by a new event, `PaletteChanged` (the palette index), on
which `application::run` repaints the whole window; the shell repaints the whole screen in the
same frame (one full damage; a 1280x800 repaint is well inside the 8 ms budget of
`desktop-v0.3.md`). Everything the person sees is in the new palette **within one frame of
the write**, like a profile change. The pointer (black and white already), the pressed window
controls (`quarter()` of their colour) and the shadow (a darkening, invisible on black, which
is why the borders turn white) need no second value.

The table lists every token the briefs and the code name (brief name, code constant where it
differs), its standard value for reference, its high-contrast value, and the contrast of the
pair the standard brief fixed, recomputed.

**Shell tokens (`desktop-v0.3.md` section 2, `drivers/src/desktop/mod.rs`)**

| Token | Standard | High contrast | Pair and ratio in high contrast |
|-------|----------|---------------|---------------------------------|
| `wallpaper.top` | `#0E1726` | `#000000` | desktop icon labels `text` on it 21.0:1; no gradient (decoration) |
| `wallpaper.bottom` | `#1B3A5C` | `#000000` | as above |
| `wallpaper.egg` | `#1F3F63` | `#262626` | decoration; 1.4:1 against the black, faint on purpose |
| `accent` | `#F2B33D` | `#FFD24D` | on `menu.bg` 14.6:1, on `selection` 7.9:1 (the `>` mark and the ring on every surface) |
| `shell` (boot egg, `boot_screen::EGG`) | `#F6F1E7` | unchanged | the boot screen precedes the settings file |
| `boot_screen::UNLIT` | `#26303F` | unchanged | as above |
| `bar.bg` (`BAR`) | `#141B26` | `#000000` | `text` 21.0:1 |
| `bar.edge` | `#202938` | `#FFFFFF` | the bar's edge, drawn |
| `bar.hover` | `#26303F` | `#262626` | `text` 15.1:1, `text.dim` (the idle Agent chip) 8.3:1 |
| `bar.pressed` | `#334052` | `#404040` | `text` 10.4:1, `text.dim` 5.7:1 (pressed, momentary) |
| `menu.bg` (`MENU`) | `#1C2430` | `#000000` | `text` 21.0:1, `text.dim` 11.4:1 |
| `menu.border` | `#2A3340` | `#FFFFFF` | every menu, popup, dropdown and the sidebar's separator edged |
| `menu.hover` | `#2A3440` | `#262626` | `text` 15.1:1, `text.dim` 8.3:1 |
| `text` | `#E6EAF0` | `#FFFFFF` | |
| `text.dim` | `#8A94A6` | `#B8C0CC` | on black 11.4:1; still visibly dimmer than `text` |
| `title.active` | `#2A3340` | `#262626` | `text` 15.1:1; the controls `#E5534B` 4.1:1, `#E8B33C` 7.9:1, `#3DBE5C` 6.3:1 (graphics, ≥ 3:1) |
| `title.inactive` | `#1F2630` | `#000000` | `text.dim` 11.4:1; the focused window is told by its title fill and its white border |
| `border.active` | `#3A4454` | `#FFFFFF` | the focused window's edge |
| `border.inactive` | `#2A3340` | `#8A94A6` | 6.9:1 against black (a graphic) |
| `button.close` | `#E5534B` | unchanged | above |
| `button.minimise` | `#E8B33C` | unchanged | above |
| `button.maximise` | `#3DBE5C` | unchanged | above |
| `button.inactive` | `#4A5466` | `#8A94A6` | 6.9:1 against black |
| `term.bg` | `#0B0E13` | `#000000` | |
| `term.fg` | `#D7DEE8` | `#FFFFFF` | 21.0:1 |
| `term.cursor` | `#F2B33D` | `#FFD24D` | 14.6:1; the glyph under it stays `term.bg` |
| `term.selection` | `#2E4A6B` | `#153A6B` | `term.fg` on it 11.4:1 |

**ANSI colours in high contrast** (the terminal's 16): the normal row takes today's bright
values and the bright row goes lighter still, so every colour but 0 is at least 7.7:1 on
black; ANSI 0 stays `term.bg` itself, the one deliberate exception of `desktop-v0.3.md`:

| | 0 black | 1 red | 2 green | 3 yellow | 4 blue | 5 magenta | 6 cyan | 7 white |
|---|---|---|---|---|---|---|---|---|
| normal | `#000000` | `#FF6E6E` 7.7 | `#84D48A` 11.8 | `#F2CC6D` 13.6 | `#7FB0F0` 9.4 | `#DA95E4` 9.4 | `#72D3E0` 12.1 | `#F2F5F9` 19.2 |
| bright | `#B8C0CC` 11.4 | `#FF9A9A` 10.3 | `#A8F0AD` 15.7 | `#FFE08A` 16.3 | `#A8CCFF` 12.7 | `#EAB8F0` 12.6 | `#A0E8F0` 15.3 | `#FFFFFF` 21.0 |

**GUI tokens (`desktop-v0.3-gui.md` section 1 and the code's icon constants)**

| Token | Standard | High contrast | Pair and ratio in high contrast |
|-------|----------|---------------|---------------------------------|
| `window.bg` | `#12161D` | `#000000` | carries no text |
| `selection` | `#2E4A6B` | `#153A6B` | `text` 11.4:1, `accent` 7.9:1 |
| `focus.ring` | `#F2B33D` | `#FFD24D` | on black 14.6:1, on `button.bg` 11.4:1, on `selection` 7.9:1; yellow, not white, so the ring reads apart from the white borders it sits 1 px inside |
| `field.bg` | `#0B0E13` | `#000000` | `text` 21.0:1, placeholders `text.dim` 11.4:1 |
| `button.bg` | `#2A3440` | `#1F1F1F` | `text` 16.5:1, `text.dim` (disabled) 9.0:1; lifted from the field's black so a button and a field differ in fill as well as in their text's alignment |
| `button.border` | `#3A4454` | `#FFFFFF` | every button and field edged |
| `button.primary` | `#F2B33D` | `#FFD24D` | `icon.dark` 14.6:1 |
| `progress.track` | `#26303F` | `#4D4D4D` | the done part `accent` 5.9:1 against it (a graphic) |
| `dock.bg` | `#1C2430` | `#000000` | as `menu.bg` |
| `icon.tile` | `#26303F` | `#000000` | the letter `text` 21.0:1 |
| `icon.tile.border` | `#3A4454` | `#FFFFFF` | |
| `icon.prompt` | `#5FB865` | `#84D48A` | 11.8:1 against the tile |
| `icon.folder` | `#E2B341` | `#FFD24D` | |
| `icon.folder.back` | `#B88A24` | `#B38600` | 6.3:1 against black, under the front |
| `icon.gear` | `#8A94A6` | `#FFFFFF` | |
| `icon.gear.hole` | `#1F2630` | `#000000` | |
| `icon.windows` | `#4F8FE0` | `#0A3D8F` | `text` on it 10.1:1 (today 4.6:1; the badge keeps its white glyph) |
| `icon.mac` | `#C9D1DB` | `#FFFFFF` | `icon.dark` 21.0:1 |
| `icon.linux` | `#E2B341` | `#FFD24D` | `icon.dark` 14.6:1 |
| `icon.dark` | `#0B0E13` | `#000000` | |
| `icon.page` | `#F6F1E7` | `#FFFFFF` | |
| `icon.page.edge` | `#C9D1DB` | `#B8C0CC` | |
| `icon.page.line` | `#4A5466` | `#8A94A6` | 6.9:1 against black; on the page 2.9:1, a decoration |

**Toolkit tokens (`toolkit-v0.3.md` section 1)**

| Token | Standard | High contrast | Pair and ratio in high contrast |
|-------|----------|---------------|---------------------------------|
| `button.hover` | `#344050` | `#333333` | `text` 12.6:1 |
| `button.pressed` | `#202833` | `#4D4D4D` | `text` 8.5:1 |
| `button.primary.hover` | `#F6C25C` | `#FFE080` | `icon.dark` 16.2:1 |
| `button.primary.pressed` | `#D99E2E` | `#E6B800` | `icon.dark` 11.2:1 |

That is **54 named tokens plus the 16 ANSI colours: 70 rows**, of which 5 (the boot screen's
two and the three window controls) keep their value. The
miniatures on the profile cards, the Win32 system colours of `windows-gui-v0.3.md` (they map
to these tokens) and every icon follow the table without a line of their own. Foreign apps are
told: the Windows layer answers `SPI_GETHIGHCONTRAST` with the flag set and its system colours
from this palette, the macOS layer's `accessibilityDisplayShouldIncreaseContrast` is true, and
Android's `high_text_contrast_enabled` is 1, each when that layer draws UI (D17, D19; later).

### 2.5 Reduce motion: what moves today, and what stops

EggOS has almost no motion by decision (`desktop-v0.3.md` section 2 `Motion`, section 12 of
`desktop-v0.3-gui.md`: the switcher, menus and the launcher appear in one frame). What moves
or changes by itself on screen today, from the code:

| What | Where | Kind | With `reduce_motion=true` |
|------|-------|------|---------------------------|
| the caret's blink, 500 ms on and off | the toolkit (`CARET_BLINK_MS`), every text field | blink | **stops**: the caret is drawn steady, still restarted "on" by a keystroke (so it shows) |
| the terminal cursor's blink | the terminal, 500 ms, `CSI ? 12 l` stops it | blink | **stops**: steady block; a program's `CSI ? 12 h` is ignored |
| the launcher's search caret | the shell (`desktop-v0.3-gui.md` 6.2) | blink | **stops** |
| the Agent chip's acting fill and the agent's arrow, 1000 ms | `AGENT_ACTING_MS` | a state shown for a second | stays: it is information, one change and one change back, no flicker |
| the tooltip after 600 ms | `TOOLTIP_DELAY_MS` | a delay | stays: a delay is not motion |
| the launcher's notice for 3 s | `NOTICE_MS` | timed text | stays |
| `(not responding)` after 500 ms, the 2 s close timeout | `NOT_RESPONDING_AFTER_MS`, `CLOSE_TIMEOUT_MS` | timed text | stays |
| the bar's clock each minute, `up m:ss` each second, About's uptime, the Task Manager's 2 s refresh, the first run's `Now` row | counters | changing numbers | stay: a number that changes is not motion |
| the boot screen's four progress segments | before the settings file is read | progress | stays, and could not know the setting |
| a dragged window, a dragged slider thumb, a scrolled list | direct manipulation | follows the pointer | stays: the pointer moves it, nothing moves on its own |
| the progress bar of an install | the installer dialog | progress | stays (progress is information) |

So in 0.3.0 reduce motion **stops every blink**, which is the only thing that moves on its
own: the window server passes the flag to the shell and to every client (the `GetWorkArea`
byte and the `PaletteChanged` event carry both flags, so one event serves both settings), and
the toolkit's tree and the terminal stop their timers. The setting is also **a promise to
every later brief**: anything that would animate (a window opening or closing, the launcher
sliding, scroll inertia, a switcher fade, a bouncing dock icon, a spinner) checks it and
draws its end state in one frame instead; a brief that adds motion must list it here. The
compatibility layers report it when they draw UI: Windows `SPI_GETCLIENTAREAANIMATION`
false, macOS `accessibilityDisplayShouldReduceMotion` true, Android
`animator_duration_scale` 0 (later).

### 2.6 Sticky keys

Modifiers latch, as Windows and macOS do, and the bar says which. It lives in the **kernel's
input layer** (`kernel/src/input.rs`, where key events become modifier state before the
keymap, the console, the window server's shortcuts and the apps see them), so every consumer
sees ordinary modifier bits: Alt+Tab, the Terminal's Ctrl+C, a field's Shift+Left and a Win32
program's `GetKeyState` all work with latched keys, and nothing downstream knows.

- **Modifiers**: Shift, Ctrl, Alt, Super, both sides each, and AltGr (Right Alt in the
  Windows and Linux profiles, Option in macOS's, `keyboard-v0.3.md` section 5: a tap of AltGr
  then `e` types `€`). Not Caps Lock (a lock already) and not Fn (a keyboard's own key).
- **A tap** (press and release with no other key or pointer button pressed in between)
  **latches** the modifier: it is held for the next non-modifier key or pointer button press
  and released at that key's release (a latched Shift and a click is Shift+click). Several
  modifiers latch together (tap Ctrl, tap Shift, press Esc is Ctrl+Shift+Esc, the Task
  Manager). **A second tap** of a latched modifier **locks** it: held until tapped again. A
  tap of a locked one unlocks it. A modifier physically held with a key is a chord as always
  and neither latches nor clears a lock; a latched modifier is added to whatever is held.
  Turning sticky keys off clears every latch and lock.
- **The indicator**: one chip per active modifier in the panel, in the order `Shift` `Ctrl`
  `Alt` `AltGr` `Super`, only those latched or locked (nothing to say, nothing drawn: the
  bar's room is for window titles). Each chip is the Agent chip's shape (20 tall, `INSET_R4`,
  text at x + 8, glyph top at y + 2), its width the wider of its two words plus 16 so it does
  not jump when it locks: `Shift`/`SHIFT` 43, `Ctrl`/`CTRL` 39, `Alt`/`ALT` 33,
  `AltGr`/`ALTGR` 45, `Super`/`SUPER` 45, 4 px apart. **Latched**: `bar.hover` fill, the word
  in `text` (`Shift`). **Locked**: `accent` fill, the word in capitals in `icon.dark`
  (`SHIFT`): the capitals are the non-colour cue. Place: in the top bar, the strip's right
  edge at the Agent chip's x - 8 when the chip shows, else at the power button's x - 8, at
  y 4; in the taskbar, right edge at the Agent chip's x - 8, else at the clock's x - 16,
  vertically centred; the taskbar items' limit (6.4 item 13 of `desktop-v0.3-gui.md`) becomes
  the leftmost chip's x - 16. On the phone's bar the chips sit at the right edge, 8 px in. The
  kernel tells the desktop on each change (`Desktop::set_latched(modifiers, locked)`,
  damaging the strip's old and new rects).
- **Shift five times turns it on or off** (Windows: Shift x5; macOS: Shift x5 with Sticky
  Keys' shortcut option): five taps of a Shift key with no other key between and each within
  2 s of the last (`STICKY_TOGGLE_TAPS` 5, `STICKY_TOGGLE_WITHIN_MS` 2000), seen by the window
  server's key path (`shortcuts.rs`), **start `settings set accessibility.sticky_keys true`**
  (or `false`), the command that saves and applies, as the shell starts other programs
  (`Action::Start`); nothing is toggled in memory alone, so the CLI, the pane and the next
  boot agree. When the apply arrives, the indicator's strip shows `Sticky keys on` (72 px) or
  `Sticky keys off` (74 px) as a `bar.hover` chip in `text` for 3 s (`NOTICE_MS`), then the
  normal chips. The pane follows at its next focus (2.3). There is no dialog: five Shifts in
  two seconds with nothing between them is not typing, and the fifth undoes the fourth.
- With slow keys on too, each tap must be held the delay (2.7) to count; that is the rule for
  every key and it keeps one mechanism.

### 2.7 Slow keys

`accessibility.slow_keys=true` with `accessibility.slow_keys_ms` (100 to 1000, default
**300**, GNOME's default; Windows's Filter Keys starts at 1000): a key press is reported only
once the key has been held that long; a release before that reports nothing, as if the key
was never pressed. Also in the kernel's input layer, before repeat: the repeat delay
(`keyboard.repeat_delay_ms`) counts from the acceptance, not the press. Every key counts,
modifiers included (a Shift held while a letter is typed is held anyway); pointer buttons do
not (a tremor on a mouse is the pointer speed's job). No sound and no flash on acceptance:
the key's own effect is the feedback (the letter appears; on a modifier with sticky keys, the
chip). The slider's steps are 50 ms (`PageUp` 500), keys exact; `settings set
accessibility.slow_keys_ms 325` sets what the slider cannot, and the slider shows it snapped
to 350 while the value in force stays 325 (the toolkit's rule for a value between steps,
`toolkit-v0.4.md` 2.1: shown snapped; the pane never writes until the person moves it).

### 2.8 Full keyboard navigation: always on, not a toggle

**Decided: always on, no setting to turn it off.** macOS's `Full Keyboard Access` is a toggle
because its default Tab skips buttons and lists; EggOS's Tab reaches every control already
(toolkit section 3: push button, checkbox, radio group, card picker, list, text field, slider,
dropdown, in reading order, nothing skipped but disabled ones), and the shell's launcher,
switcher, menus, window menu, move and size all have keys (section 5.3 of
`desktop-v0.3-gui.md`). A toggle would only add a way to make the OS worse. What was missing,
and this brief adds, is **the panel**: the top bar's and taskbar's items and the dock's icons
had no key.

- **`Ctrl+Alt+Tab` focuses the panel** (GNOME's key), and so do `Super+T` (Windows's `Win+T`,
  free in `shortcuts.rs`) and `Ctrl+F3` (macOS's `Move focus to the Dock`); all three in every
  profile, as the switcher's keys are (`Shortcut::FocusPanel`). The first press moves the
  keyboard focus to the bar: its first item (the system item, `Activities` or `Start`) gets
  the 2 px `focus.ring` at inset 2 of its hit rect; Left/Right (and Tab/Shift+Tab) move it
  across the bar's items in their drawn order (system item, chips or taskbar items, the Agent
  chip, the power button; the clock and the sticky chips are not items and are skipped),
  wrapping; Enter or Space is the item's click; Esc returns the focus to the window that had
  it. A second press of the key moves on to the **dock** (macOS profile: its icons, Left/Right,
  Enter launches or focuses, the dock's tooltip shows for the ringed icon), a third back to the
  window; in the other profiles the second press returns to the window. While the panel has the
  focus the focused window keeps its chrome (it is not unfocused; keys simply go to the panel)
  and a click anywhere ends it. `wm show` lists the bar's and the dock's items as `button`s
  already (D161); the ring is the only new drawing.
- The pane's row says `Always on` and line 14 names the key, so a switcher who searches `Full
  Keyboard Access` gets the answer and the key in the place they looked. A read-only key,
  `accessibility.keyboard_navigation=always`, says the same to `settings list` and
  `settings_list`, as `display.pointer` reports a fact; `set` on it answers `read-only`.
- Still later: a screen reader (the README's Cmd+F5, Win+Ctrl+Enter, Super+Alt+S), which is
  the next thing keyboard-only users need and the largest; focus rings hidden in "mouse mode"
  (toolkit v0.4 section 9) is not built, so the ring always shows.

### 2.9 Keys, focus and pointer in the pane

- **Tab order** from the sidebar list: `High contrast`, `Reduce motion`, `Sticky keys`,
  `Slow keys`, the slider (skipped while disabled), then wraps to the search field; Shift+Tab
  the reverse. Showing the pane does not move the focus.
- Space toggles the focused checkbox (Enter goes to the window's default button, which this
  pane does not have, so nothing); on the slider Left/Right, Home/End, PageUp/PageDown as
  toolkit v0.4 section 2.4, each step a write.
- **Pointer**: `Arrow` everywhere; a click on a checkbox's label toggles it (the whole control
  is the target); a press on the slider's track jumps; a right-click does nothing. A click on
  a hint label does nothing (labels take no input).

### 2.10 Every device class (D23)

- **One column** when the pane is narrower than 448 (a phone's maximised window): each
  checkbox row becomes two lines 64 apart, the checkbox at (16, y + 6) and its hint at
  (16, y + 36) in `text.dim`, rows at 64, 128, 192, 256; `Hold a key for` at (40, 332) with
  the slider under it at (16, 356), the width minus 32; `Full keyboard navigation` at
  (16, 396) with `Always on` right-aligned on the same line, line 14 at (16, 416) ellipsised
  if it must, the status line at (16, 440). The pane scrolls in the window's scroll view, the
  focused widget kept in view.
- **Touch**: checkboxes 40 tall (the mark at +12), rows 48 apart (64, 112, 160, 208), the
  slider's control 40 tall (its thumb 16 as drawn, its hit area the full height), everything
  below moves down by the difference; a tap on a label toggles.
- **TV (scale 2)**: everything doubles; the ring on the focused box or thumb is the cue; the
  pane works by Tab, Space and arrows; the panel key works from the remote's keyboard.
- **Keyboard-only** on every form factor: four Tab stops and a slider; from the default, the
  farthest delay is 14 presses (`End` is one), which the CLI sets in one line.
- **Headless**: no pane; `settings set` and `settings_set` are the whole capability, and
  sticky and slow keys work on the console (2.3).

### 2.11 What an agent sees (D161, `wm show`)

`checkbox`es named `High contrast`, `Reduce motion`, `Sticky keys` and `Slow keys`, each
`checked` when on; a `slider` named `Hold a key for` with the value `300 100..1000 by 50`,
flagged `disabled` while `Slow keys` is off; the four hints, `Hold a key for`, `Full keyboard
navigation`, `Always on`, line 14 and the status line `label`s with their drawn text. Nothing
here is person-only: an agent with the grant `settings_set` needs may set every one of these
already, and `Invoke` on a checkbox or `set-text` on the slider is the same write. An agent
that wants a value calls `settings_get`; one that wants to know whether high contrast is on
reads `settings_get accessibility.high_contrast`, never a pixel. The first-run window's
agents are section 1.8.

### 2.12 The settings keys, the CLI and MCP (D22)

| Key | Value | Settable | Applies |
|-----|-------|----------|---------|
| `accessibility.high_contrast` | `true` or `false` (the default) | yes | at once, every window and the shell (2.4) |
| `accessibility.reduce_motion` | `true` or `false` (the default) | yes | at once (2.5) |
| `accessibility.sticky_keys` | `true` or `false` (the default) | yes | at once, from the next key, the console too (2.6) |
| `accessibility.slow_keys` | `true` or `false` (the default) | yes | at once, from the next key (2.7) |
| `accessibility.slow_keys_ms` | `100` to `1000`, `300` the default | yes | at once, when `slow_keys` is on |
| `accessibility.keyboard_navigation` | `always` | no | a fact (2.8) |

`settings set` accepts `true`, `false`, `on`, `off`, `yes`, `no`, `1` and `0` for the four
booleans and prints `true` or `false` (the settings file's first booleans; `on`/`off` would
read well too, and the plan named `true|false`, so that is the stored form and `settings
list` prints it). `settings set --dry-run` works on them as on every key. `settings_get`,
`settings_set` and `settings_list` take the same keys with no new tool; a set of a key out of
range or of a word that is not a boolean answers `invalid value` (status 2) as today. The
`settings` command's table in docs/settings.md gains these six lines in the build.

### 2.13 Budget

`CAPACITY` is 164 today (`displays-mouse-v0.3.md` section 9). The pane adds **15 widgets**
(section 2.2's table) and **its container** (`build.pane`, one per pane), **+16**, so
`CAPACITY` becomes **180**, and the installer's `eggos_app::dialog::CAPACITY` stays equal to
it. The first-run window is its own tree of the same `CAPACITY` and holds 32 (six shared,
three page containers, 6 + 6 + 11 on the pages), well inside it. The sidebar's list is static
(`&PANES`), so the tenth row costs nothing. Per pane, container excluded: Accessibility 15.

## 3. Acceptance (host tests, default 640x440, scale 1x; facts `on_disk` true, `profile` 1
(macOS), `keyboard.layout` `us`, `time_zone` `UTC`, realtime 1_793_282_709,
`accessibility` the defaults unless said)

**First run**

- `FirstRunWindow` is 640x440 titled `Welcome to EggOS`, `FIXED_SIZE`; page 1 shown: heading
  `Where are you coming from?` at (16, 16); `Step 1 of 3` ending at 624, cell top 24, in
  `text.dim`; the cards at (112, 64), (256, 64), (400, 64), each 128x112, `macOS` selected;
  `Primary modifier` at (16, 204) and `Cmd (Super key)` ending at 624; `Window controls` at
  (16, 244) and `Left` ending at 624; line 1.6 at (16, 284); the status line at (16, 356)
  reading `Every choice can be changed later in Settings.` in `text.dim`; `Skip`
  (16, 396, 96, 28); `Back` (424, 396, 96, 28) disabled; `Next` (528, 396, 96, 28) primary;
  the focus on the cards.
- A click at (176, 120) returns `Request::SetProfile(0)` and selects `Windows`; the rows read
  `Ctrl` and `Right` in the same frame. Right on the focused cards moves the ring without a
  request; Space on `Linux` returns `SetProfile(2)`. A click at (576, 410) shows page 2 and
  writes nothing: heading `Keyboard`, `Step 2 of 3`, `Layout` at (16, 76), the dropdown
  (424, 70, 200, 28) valued `English (US)`, the try field (16, 120, 304, 28) empty, lines at
  (16, 164), (16, 184) with the US layout's texts, (16, 204) hidden with Caps Lock off and a
  keyboard attached; `Back` enabled; the focus on the dropdown.
- Down on the focused dropdown returns `SetKeyboardLayout(1)` (`us-intl`) and the lines
  change; `g` returns `SetKeyboardLayout(8)` (`German`); a click at (500, 84) opens the popup
  at (412, 102, 224, 258) and a release on its fourth row returns `SetKeyboardLayout(3)`
  (`English (US, Dvorak)`). Tab from the dropdown focuses the try field; typing `x` edits it;
  Enter in it activates `Next`. `Back` (a click at (472, 410)) shows page 1 with the saved
  profile selected.
- Page 3: heading `Time zone`, `Step 3 of 3`; `Time zone` at (16, 76), `UTC` ending at 624
  cell top 76, `UTC+00:00` ending at 624 cell top 96; the field (16, 120, 304, 28) empty; the
  count hidden; the list (16, 156, 608, 72) with one row, `UTC · UTC+00:00`, selected, its
  label at (40, 160); `Now` at (16, 244) and `Thursday 29 October 2026, 15:05:09 UTC` ending
  at 624; `Done` at (528, 396, 96, 28) primary; the focus in the field.
- Typing `par` shows three rows and the count `2 zones`; Down from the field focuses the
  list on `UTC`; Down returns `SetTimeZone` of Paramaribo and the `Now` row reads `...,
  11:05:09 UTC-03:00`; Down again returns Paris's index and `..., 15:05:09 CET`, the name
  `Europe/Paris`, the facts `UTC+01:00 CET, standard time`; Enter on the list returns
  `Activated(done)`. Enter in the field with `par` typed moves the focus to the list and
  activates nothing.
- `Skip` on page 1 with a settings file holding no `shell.profile` returns `SetProfile(1)`
  then closes; with `shell.profile` saved it only closes; `Done` the same; a `CloseRequest`
  the same as `Skip`. A write answered `EPERM` after a click on `Windows` puts `macOS` back
  and the status line reads `Not allowed: this user may not set the profile.` in `text`;
  `ENOENT` reads `No disk attached: the profile could not be saved.`; the next successful
  write restores the hint. With `on_disk` false the status line reads `No disk attached:
  these choices last until power-off.` in `text.dim` from the start.
- `settings --first-run` with a fake server opens the window at (319, 143) in the macOS
  profile at 1280x800; without a server it writes `settings: no display` and exits 1. init's
  fake: with a disk and a settings file lacking `shell.profile` it starts
  `/bin/settings --first-run` after the login program; with `shell.profile=windows` or with
  `--no-disk` it does not. `wm show` on the window lists the radio group, the combo-box, the
  two text fields, the list and the three buttons with the names of 1.8.
- One column: at 360 wide and a content height `H` the cards are at (116, 88), (116, 216),
  (116, 344), the rows' text at 484 and 524, `Skip` (16, H - 104, 160, 40), `Back`
  (184, H - 104, 160, 40) and `Next` (16, H - 56, 328, 40) at the bottom, the page above them
  scrolling.

**The Accessibility pane**

- The sidebar's `Accessibility` row is (0, 172, 176, 32), `Storage` (0, 204, 176, 32) and
  `About` (0, 332, 176, 32); `PANES[4]` is `Accessibility`, `STORAGE` 5, `DATE_AND_TIME` 6,
  `AGENTS` 7, `COMPONENTS` 8, `ABOUT` 9; at 640x360 the sidebar list shows its scroll bar.
  `contrast`, `motion`, `sticky`, `slow keys`, `filter keys`, `ease of access`, `a11y` and
  `full keyboard access` each match Accessibility and nothing else; `keys` matches Keyboard
  and Accessibility; `modifier` matches Appearance and Accessibility; `narrator` matches no
  pane.
- `CAPACITY` is 180 and the tree holds 180. In window coordinates: heading at (192, 16);
  `High contrast` (192, 70, 91, 28) unchecked with its hint ending at 624, cell top 76, in
  `text.dim`; `Reduce motion` (192, 110, 96, 28); `Sticky keys` (192, 150, 80, 28); `Slow keys`
  (192, 190, 73, 28); `Hold a key for` at (216, 236); the slider (408, 230, 216, 28) disabled,
  value 300, thumb centred at x 448, readout `300 ms` ending at 624; `Full keyboard
  navigation` at (192, 276) and `Always on` ending at 624; line 14 at (192, 296); the status
  line at (192, 316) hidden.
- A click at (200, 84) returns `Request::SetAccessibility(HighContrast(true))` (saved as
  `accessibility.high_contrast=true`, then applied) and checks the box; a second click returns
  `HighContrast(false)`. Space on the focused `Reduce motion` returns `ReduceMotion(true)`.
  A click on the `Sticky keys` label at (260, 164) returns `StickyKeys(true)`. A click at
  (200, 204) returns `SlowKeys(true)` and enables the slider in the same frame; Tab from
  `Slow keys` then focuses the slider; with slow keys off Tab from `Slow keys` wraps to the
  search field.
- With slow keys on: a press at (560, 244) returns `SlowKeysMs(1000)` and the readout reads
  `1000 ms`; a press at (488, 244) returns `SlowKeysMs(550)`; Right on the focused slider
  from 300 returns `SlowKeysMs(350)`; PageUp from 300 returns 800; Home returns 100; End
  returns 1000. Facts with `slow_keys_ms` 325 show the thumb at 350's place and the readout
  `350 ms` and write nothing until moved. Showing the pane again after `settings set
  accessibility.slow_keys_ms 500` on the CLI (new facts) shows `500 ms`.
- A save answered `EPERM` leaves `High contrast` unchecked, applies nothing and shows `Not
  allowed: this user may not change the accessibility settings.` at (192, 316) in `text`;
  `ENOENT` shows `No disk attached: the accessibility settings could not be saved.`; a save
  that worked and an apply the server refused keeps the box checked and shows `Saved for the
  next boot, but the window server did not take it.`; with `on_disk` false the line reads
  `No disk attached: this choice lasts until power-off.` in `text.dim` and a click still
  returns the change. Showing another pane and coming back hides the line (with a disk).
- `settings get accessibility.high_contrast` after the click at (200, 84) prints `true`;
  `settings set accessibility.sticky_keys on` then `get` prints `true`; `settings set
  accessibility.slow_keys_ms 50` answers `invalid value` with status 2; `settings set
  accessibility.keyboard_navigation never` answers `read-only` with status 1; `settings list`
  carries the six `accessibility.` lines with their defaults on a fresh file.
- `wm show` lists four `checkbox`es, the `slider` `Hold a key for` valued `300 100..1000 by
  50` and `disabled`, and the labels of 2.11; `act(invoke)` on `Sticky keys` returns the same
  `StickyKeys(true)`.

**The palette, the blink, the keys and the panel (the shell's and the kernel's host tests)**

- With high contrast on, the shell's paint of the bar, a focused and an unfocused window, the
  launcher and the power menu uses the high-contrast column: pixel (0, 0) `#000000`, the bar's
  row 27 `#FFFFFF`, a focused window's border `#FFFFFF` and title fill `#262626`, an unfocused
  one's border `#8A94A6`, a menu's border `#FFFFFF`; a toolkit window after `PaletteChanged(1)`
  paints a button's border `#FFFFFF` and fill `#1F1F1F`, a field `#000000`, a selected row
  `#153A6B`, the ring `#FFD24D`; the terminal's default foreground `#FFFFFF` on `#000000`
  and ANSI 1 `#FF6E6E`. A test computes the WCAG ratio of every text pair this brief names
  from the palette table and asserts at least 7.0 for the resting pairs and 4.5 for the
  hover and pressed ones, so a changed value cannot pass unnoticed.
- With reduce motion on, the toolkit's caret is drawn at 0 ms, 600 ms and 1200 ms after a
  keystroke (never blanked); the terminal's cursor likewise, and `CSI ? 12 h` leaves it
  steady; with it off the caret is blank at 600 ms as today.
- The kernel's input layer with sticky keys on: tap Left Shift (press, release), press `a`:
  the `a` press carries Shift; its release clears the latch; the next `a` carries no Shift.
  Tap Shift twice, then `a`, `b`: both carry Shift; a third tap, then `c`: no Shift. Tap Ctrl,
  tap Shift, press Esc: Ctrl and Shift. Hold Shift and press `a`: Shift, no latch after. Tap
  Shift, then a left button press: the button event carries Shift, and the release clears
  it. Tap AltGr, press `e` on `de`: `€`. Turning sticky keys off with Shift locked clears it.
  `Desktop::set_latched` is called on each change. In the top bar at 1280 (the power button
  at x 1244) a latched Shift alone is the chip (1193, 4, 43, 20), `Shift` in `text` on
  `bar.hover`, and `SHIFT` in `icon.dark` on `accent` when locked; with the Agent chip shown
  (x 1180) it is (1129, 4, 43, 20); with Ctrl latched too the strip is `Shift` then `Ctrl`
  in that order, Ctrl's chip (1197, 4, 39, 20) and Shift's (1150, 4, 43, 20), 4 px apart,
  the strip's right edge at the power button's x - 8; in the taskbar the strip ends at the
  clock's x - 16 and the taskbar items' limit moves to the strip's x - 16.
- Five Shift taps within 2 s with nothing between make the shell start `settings set
  accessibility.sticky_keys true` (`Action::Start` with those arguments); a sixth within 2 s
  of the fifth, after the apply, starts it with `false`; four taps, or five with an `a` in the
  middle, start nothing; the apply shows `Sticky keys on` in the strip for 3000 ms, then the
  normal chips.
- With slow keys on at 300 ms: a key pressed at 0 and released at 200 reports nothing; pressed
  at 0 and still held at 300 reports its press at 300, and repeats from 300 + the repeat
  delay; released at 350 reports the release; a modifier is held to the same rule; a pointer
  button is reported at once.
- `Ctrl+Alt+Tab`, `Super+T` and `Ctrl+F3` each map to `Shortcut::FocusPanel` in every profile;
  after it the system item's rect carries the ring at inset 2 and the window's chrome is
  unchanged; Right moves the ring to the next item, wrapping past the power button to the
  first; Enter on the power button opens the power menu; Esc puts the keys back in the window;
  in the macOS profile a second `Ctrl+Alt+Tab` rings the dock's first icon and a third
  returns to the window; in the Windows profile the second returns to the window.

## Decided by design (the owner can overrule any of these)

The first run is a centred 640x440 window, not a full-screen page, because the desktop behind
it is the preview; every answer is saved as it is made, and `Skip`, `Done` and closing write
the profile in force when none is saved, so the question is asked once, which answers the UX
README's open question with "the profile already in force" (macOS habits in 0.3.0); the mode
is `settings --first-run`, replacing section 14.2's `--welcome`; no command or MCP tool for
the setup itself. The pane is named `Accessibility`, placed after `Mouse`; its booleans are
stored as `true`/`false` with `on`/`off`/`yes`/`no`/`1`/`0` accepted; `Full keyboard
navigation` is always on with a read-only key rather than a toggle; `Ctrl+Alt+Tab`,
`Super+T` and `Ctrl+F3` focus the panel in every profile; the sticky-keys indicator shows only
latched and locked modifiers (nothing when idle), with capitals as the lock's non-colour cue;
five Shift taps toggle sticky keys by starting `settings set`, with a 3 s notice and no
dialog; slow keys apply to modifiers but not to pointer buttons, default 300 ms; reduce
motion stops the blinks and promises nothing else will animate; high contrast is the dark
theme at its extremes, 7:1 resting and 4.5:1 for momentary fills, the ring kept yellow; no
search words for features not built.

## Left out of 0.3.0

A screen reader (the README's three toggles) and the text-to-speech it needs; a light theme
and its high-contrast variant; bounce keys and toggle keys (Windows), mouse keys (the numeric
pad moves the pointer), a larger pointer, zoom and a magnifier, large text (the font has one
size; 2x is the Display pane's), a font-size or a per-app scale; visual alerts (a flash for a
beep: there is no beep); live captions; the `Hold right Shift for 8 s` Filter Keys shortcut
(Windows only, surprising); a `Run setup again` button; a language or a user account page in
the first run (there are no accounts and one language); setting the clock in the first run
(the hardware clock sets it, D150); a network page in the first run (the Network pane is
designed, not built); the sticky-keys modifier chips on the console without a desktop;
foreign apps' own accessibility APIs (UIA, NSAccessibility, Android's) beyond the flags named
above; focus rings hidden in "mouse mode".
