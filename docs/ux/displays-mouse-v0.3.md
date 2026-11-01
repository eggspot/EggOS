# EggOS v0.3.0 Display and Mouse: design brief (D195, slices 43a and 43b)

Design by Claude Fable (D16) for the GUI front-ends (D22) of the settings slices 43a and 43b
built: `display.scale` (1 or 2, drawn at half the device's size and doubled on flush, in force
from the next boot), `mouse.buttons` (`right` or `left`), `mouse.scrolling` (`traditional` or
`natural`), `mouse.speed` (1 to 10), `keyboard.repeat_delay_ms` (150 to 2000) and
`keyboard.repeat_rate` (2 to 60 a second). The CLI (`settings set`) and MCP (`settings_set`)
front-ends exist; this brief adds the panes, which are the same writes. Everything in
`toolkit-v0.3.md` (section 11 for Settings), `keyboard-v0.3.md` and `date-time-v0.3.md`
stands except where a section below says otherwise; numbers are px at 1x and double at 2x;
the only motion is the caret's blink.

## 1. The panes in the sidebar

- **Names and order stay**: `Display` and `Mouse` are rows 2 and 4 of the sidebar already
  (`Appearance`, `Display`, `Keyboard`, `Mouse`, `Storage`, `Date & time`, `Agents`,
  `Components`, `About`; y 76..107 and 140..171, text at x 16). `Display`, singular, because
  0.3.0 drives one screen and the row is the one people have found since 0.3.0's first build;
  `Displays` (macOS, GNOME) is a search word so switchers find it. The sidebar has no icons.
- **Key repeat lives in the Keyboard pane**, not in Mouse: macOS (`Keyboard`), Windows
  (Control Panel `Keyboard`) and GNOME (`Keyboard` and `Accessibility`) all keep it with the
  keyboard, a person looking for "my key repeats too fast" opens Keyboard, and
  `keyboard-v0.3.md` section 4 reserved the room under the keyboard line for it. So the
  Mouse pane holds the three mouse settings and nothing else, and no pane is named
  `Mouse & keyboard`.
- **Search words**. Display: today's (`Resolution`, `Scale`, `HiDPI`, `Pointer`, `screen`,
  `monitor`) plus `Displays`, `2x`, `Retina`, `DPI`, `zoom`, `size of text`, `restart`.
  Mouse: today's (`Primary button`, `Wheel`, `trackpad`, `pointer`) plus `Buttons`,
  `Right-handed`, `Left-handed`, `swap`, `Scrolling`, `natural scrolling`, `scroll direction`,
  `Pointer speed`, `sensitivity`, `tracking speed`, `acceleration`, `cursor`, `touchpad`.
  Keyboard: today's plus `Repeat delay`, `Repeat rate`, `delay until repeat`, `typing`,
  `held key` (`Key repeat` is there already). `Scale` and `HiDPI` leave Appearance's words
  (section 2.4), so `scale` finds Display alone; `pointer` goes on matching Display and Mouse
  as today; `delay` and `repeat` find Keyboard alone; `speed`, `natural` and `swap` Mouse alone.

## 2. The Display pane

### 2.1 Layout (pane coordinates: origin window x 176, y 0; 464 wide at the default size, so
the right-aligned edge is x 448; usable height to y 332)

Rows of the 40 px grid from y 64, text cell top at row + 12, controls at row + 6.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Display` |
| 2 | label | (16, 76) | `Resolution` in `text` |
| 3 | label, right-aligned | ends at x 448, cell top 76 | the device's pixels, `1280 × 800` (the `×` is U+00D7), in `text` |
| 4 | label | (16, 116) | `Scale` in `text` |
| 5 | radios, horizontal | right-aligned, ends at x 448, y 110, 96x28 (from x 352) | `1x` `2x`; the saved choice (2.2) |
| 6 | label, right-aligned | ends at x 336, cell top 116 (163 wide, from x 173) | `Takes effect at the next restart` in `text.dim` |
| 7 | label | (16, 156) | the status line (2.3), `text.dim` or `text` |
| 8 | label | (16, 196) | `Pointer` in `text` |
| 9 | label, right-aligned | ends at x 448, cell top 196 | `Software` in `text` (`display.pointer`, read-only) |

The two read-only rows are today's `Resolution` and `Pointer`; `Scale` stops being a value
and becomes the choice, with the hint 16 px before the group as the Appearance pane's `Scale`
row had it (toolkit section 11), worded `restart` rather than `boot` because a person
restarts and a kernel boots. The status line sits under the `Scale` row, so it reads as that
row's explanation; `Pointer` follows it. Content ends at y 212. `Resolution` is the device's
pixels, the number `settings get display.resolution` prints: at 2x the window server's work
area is the halved one, so the pane multiplies it by the scale in force and still shows
`1280 × 800`.

### 2.2 The choice

The radios show **the saved setting** (`display.scale` in the settings file; the scale in
force when the file has none), not the scale on screen: the pane is the front-end of the
setting, and the status line says which of the two is on screen. The window needs both, so
`Facts` keeps `scale` (the saved choice, as today) and gains `scale_in_force` (the server's
work-area scale; 1 without a server).

- **Choosing** (`Response::Chosen` on widget 5, a click or an arrow step) writes
  `display.scale` at once (`save_display_scale(1|2)`, the same write as `settings set
  display.scale 2`), nothing is applied now, and in the same frame the status line shows the
  "from the next restart" line or, when the choice is back to what is on screen, the plain
  line (2.3). No button, no dialog: the setting is a one-line file, one step back undoes it.
- **A failed write** chooses nothing: the radios go back to the saved choice and the status
  line says why, in `text`: `EPERM` or `EACCES` → `Not allowed: this user may not set the
  scale.`; any other errno (`ENOENT`, `EROFS`, `ENOSPC`) → `No disk attached: the scale could
  not be saved.`, the two lines of `date-time-v0.3.md` section 8.4 for this setting. Showing
  the pane again puts the state line back.
- **No disk** (`on_disk` false, D131): the choice cannot reach the next boot, so the group is
  disabled (`text.dim` marks, toolkit section 2) showing the scale in force, and the status
  line says `No disk attached: the scale cannot be saved for the next restart.` from the
  start, rather than letting a click fail.
- **An odd resolution** (width or height not divisible by 2: the kernel's `set_scale` refuses
  2x then): the group is disabled showing the scale in force and the status line says
  `2x is not available: 1281 × 801 cannot be halved.` A saved `2` from the CLI on such a
  screen is shown as `1x`, which is what is in force.
- The window follows a scale set from the CLI or MCP the next time it is focused, as it
  follows `keyboard.layout`.

### 2.3 The status line (widget 7)

`W × H` is the device's size, `w × h` its half (`1280 × 800` and `640 × 400`).

| State (saved choice, scale in force) | Text | Colour |
|------|------|--------|
| 1, 1 | `At 2x the desktop looks like 640 × 400.` | `text.dim` |
| 2, 2 | `Drawn like 640 × 400 and doubled to 1280 × 800.` | `text.dim` |
| 2, 1 | `From the next restart the desktop looks like 640 × 400.` | `text` |
| 1, 2 | `From the next restart the desktop looks like 1280 × 800.` | `text` |
| no disk | `No disk attached: the scale cannot be saved for the next restart.` | `text.dim` |
| odd resolution | `2x is not available: 1281 × 801 cannot be halved.` | `text.dim` |
| write refused | `Not allowed: this user may not set the scale.` | `text` |
| write failed otherwise | `No disk attached: the scale could not be saved.` | `text` |

A pending change is in `text` because it is a result the user caused, like the `Set` lines of
Date & time; the plain state lines are `text.dim` like its "shown" lines. The widest state
line is 332 px (no disk); with a 4K screen the "from the next restart" line is 290 px; every
line fits the 432 px between the paddings.

### 2.4 What leaves the Appearance pane

The Appearance pane's `Scale` row (its label, the disabled `1x` `2x` group and the hint
`2x comes in a later version`, toolkit section 11) goes: one capability, one place, and the
pane about where the user comes from keeps `Primary modifier` and `Window controls` at rows
y 192 and 232; nothing else there moves, the no-disk line stays at 316. That is 3 widgets
fewer. `Scale` and `HiDPI` leave its search words.

## 3. The Mouse pane

### 3.1 Layout (pane coordinates as 2.1)

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Mouse` |
| 2 | label | (16, 76) | `Buttons` in `text` |
| 3 | radios, horizontal | right-aligned, ends at x 448, y 70, 195x28 (from x 253) | `Right-handed` `Left-handed`; `Right-handed` for `mouse.buttons=right` |
| 4 | label | (16, 116) | `Scrolling` in `text` |
| 5 | radios, horizontal | right-aligned, ends at x 448, y 110, 158x28 (from x 290) | `Traditional` `Natural`; `Traditional` for `mouse.scrolling=traditional` |
| 6 | label | (16, 156) | `Pointer speed` in `text` |
| 7 | push button | (328, 150), 28x28 | `-` (U+002D) |
| 8 | label, centred | in (364, 156, 48, 16) | the speed, `5`, in `text` |
| 9 | push button | (420, 150), 28x28 | `+` |
| 10 | label | (16, 316) | the status line (3.3), `text.dim` or `text`; hidden when it has nothing to say |

Three rows of the grid, content to y 178; the status line at Appearance's and Keyboard's
no-disk y so the three panes say `No disk attached` in the same place. The radio texts are
exact: `mouse.buttons` names the hand (`right`, `left`), as KDE's `Button order` does, so the
pane says `Right-handed` and `Left-handed`, never `Primary button: Right`, which would be
false for every right-handed user (their primary button is the left one); `Primary button`
stays a search word for macOS and Windows users. `Traditional` and `Natural` are GNOME's words
and the setting's own values, so a person who read `settings list` recognises them; a
checkbox `Natural scrolling` (macOS's control) would be one widget fewer but would hide the
value's name. The widest group, `Right-handed` `Left-handed`, is (24 + 64) + 24 + (24 + 59) =
195 px (`LABEL_OFFSET` 24, `RADIO_GAP` 24).

### 3.2 Behaviour

- **The current values** are `Facts::input` when the window opens and whenever the pane is
  shown; an open window follows values set from the CLI or MCP the next time it is focused.
- **Choosing a radio** or **pressing a stepper button** is the change: the pane asks the
  program for one `InputChange` (`LeftHanded(bool)`, `NaturalScrolling(bool)`,
  `PointerSpeed(n)`), which the program **saves, then applies** (`save_input`, then
  `apply_input` with the changed `InputPreferences`): the same write and the same server
  call as `settings set mouse.speed 6`. Applying is instant and silent, like a keyboard
  layout; the control shows the new value in the same frame.
- **The stepper** is section 5. `Pointer speed` steps by 1 from 1 to 10.
- **A failed save** changes nothing: the control goes back (the radio to the saved option,
  the value label to the saved number), nothing is applied, and the status line says why
  (3.3). A save that worked but an apply the window server refused keeps the control (the
  value is saved for the next boot) and says so.

### 3.3 The status line (widget 10)

| When | Text | Colour |
|------|------|--------|
| shown, no disk | `No disk attached: this choice lasts until power-off.` | `text.dim` |
| shown, with a disk | hidden | |
| save refused (`EPERM`, `EACCES`) | `Not allowed: this user may not change the mouse settings.` | `text` |
| save failed otherwise | `No disk attached: the mouse settings could not be saved.` | `text` |
| saved, the window server refused the apply | `Saved for the next boot, but the window server did not take it.` | `text` |

Without a disk the settings file is in memory (docs/settings.md), so the save works and the
change lasts the session: that is the first line, not an error. Showing the pane again
restores the "shown" state. Widest: 318 px.

## 4. Key repeat in the Keyboard pane

### 4.1 Added rows (pane coordinates; widgets 1 to 7 of `keyboard-v0.3.md` section 3 stay
where they are)

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 8 | label | (240, 188) | `Repeat delay` in `text` |
| 9 | push button | (328, 182), 28x28 | `-` |
| 10 | label, centred | in (364, 188, 48, 16) | the delay, `400 ms`, in `text` |
| 11 | push button | (420, 182), 28x28 | `+` |
| 12 | label | (240, 228) | `Repeat rate` in `text` |
| 13 | push button | (328, 222), 28x28 | `-` |
| 14 | label, centred | in (364, 228, 48, 16) | the rate, `25/s`, in `text` |
| 15 | push button | (420, 222), 28x28 | `+` |

Two grid rows at y 176 and 216 in the right column (x 240 to 448), under the keyboard line
(cell 156..172) and above the list's end (304) and the no-disk line (316). The names end at
x 304 and 299, before the `-` at 328; the widest values, `2000 ms` (42 px) and `60/s` (23),
fit the 48 px between the buttons. `Repeat delay` and `Repeat rate` are Windows's words
(macOS: `Delay until repeat`, `Key repeat rate`, both search words); the values carry their
units so the pane and `settings list` show the same numbers: `400 ms`, `25/s`.

- **Steps**: the delay by 50 ms (150 to 2000; from 437, set on the CLI, `-` gives 400 and
  `+` 450); the rate by 5 (2 to 60: `+` from 2 gives 5, `-` from 5 gives 2; from 23, `-` 20
  and `+` 25). Every value the CLI can set is shown exactly; the stepper only lands on
  multiples of the step and the ends.
- **Saving and applying** as 3.2, with `RepeatDelayMs(n)` and `RepeatRate(n)`; a change is in
  force from the next key held.
- **No keyboard**: the steppers stay enabled, as the layouts list does: the values are saved
  and apply to the keyboard present at the next boot.
- **Widget 7**, the no-disk line at (16, 316), becomes the pane's status line with the lines
  of 3.3, worded `keyboard settings` (`Not allowed: this user may not change the keyboard
  settings.`, 309 px; `No disk attached: the keyboard settings could not be saved.`, 303 px);
  a failed save of the layout uses it too, instead of only stderr.
- **Tab order** (replacing section 4's): the layouts list, the try field, delay `-`, delay
  `+`, rate `-`, rate `+`, then wraps to the search field.
- Section 4's "Later: `Key repeat`" is done by this brief; the list keeps its end at 304.

## 5. The stepper (both panes)

The toolkit has no slider or spinner (section 10 of `toolkit-v0.3.md`), so a number is set
with existing widgets: a push button `-`, a centred value label 48 wide, and a push button
`+`, 8 px apart, right-aligned to the row (the `+` ends at x 448). A radio group cannot show
1 to 10 (four options at most, and a value set on the CLI must show exactly), a list of ten
rows is 240 px tall, and a text field needs Enter and a parse error; two buttons and a number
are what every person has used on a thermostat.

- **The buttons are 28x28**, the one exception to toolkit section 4's 96 px minimum: the
  minimum is `Length::Content`'s, and these are placed with `Length::Px(28)`; the glyph is
  centred (`-` 4 px wide at x + 12, `+` 6 px at x + 11), every state as section 4 (hover,
  pressed, the ring 2 px inside the square). Not primary: the pane has no default button.
- **A press** (`Response::Activated`, a click, Space or Enter) moves the value one step and
  is the change (3.2). `-` is disabled at the bottom of the range and `+` at the top, so the
  ends show themselves without a `1 to 10` hint; a disabled button is skipped by Tab.
- **The value label** is retitled in the same frame, damaging its 48x16 rect only.
- Holding a button or a key does not repeat it (no auto-repeat in the toolkit); a wheel over
  the stepper does nothing. Exact values beyond the steps: `settings set` (D22).

## 6. Keys, focus and pointer

- **Display**: Tab from the sidebar list enters the pane at the `Scale` group (skipped when
  disabled, so Tab wraps to the search field), then wraps. Left/Right and Up/Down on the
  group move and choose at once (toolkit section 3), each step a write.
- **Mouse**: Tab enters at `Buttons`, then `Scrolling`, `-`, `+`, then wraps; Shift+Tab the
  reverse. Arrows on a group choose; Space or Enter on a button steps.
- **Keyboard**: 4.1's order.
- Showing a pane does not move the focus. Pointer: `Arrow` everywhere in these panes except
  the try field; a click on a radio's label chooses it; a right-click does nothing.

## 7. Every device class (D23)

- **One column** when the pane is narrower than 448 (a phone's maximised window). Display:
  the `Resolution` and `Pointer` rows keep their line (name left, value right: 50 + 56 and
  35 + 45 px fit any width); the `Scale` row keeps its name and its group (right-aligned,
  96 wide) and the hint moves under it to (16, 156), the status line to (16, 176) and the
  `Pointer` row to y 204 (text at 216). Mouse: each row becomes two lines 64 apart, the name
  at (16, y + 12) and its control at (16, y + 36), 28 tall, left-aligned (the stepper's `-` at
  x 16, the value centred in (52, y + 42, 48, 16), the `+` at x 108); rows at 64, 128, 192;
  the status line at (16, 276). Keyboard: the list at (16, 64), the width minus 32, 240 tall;
  the two fact lines at (16, 324) and (16, 344); the try field at (16, 368), the width minus
  32; the keyboard line at (16, 404); the repeat rows in the two-line form at y 424 and 488;
  the status line at (16, 572). The pane grows and scrolls in the window's scroll view, the
  focused widget kept in view.
- **Touch**: radios and stepper buttons 40 tall, rows 48 apart (64, 112, 160; the control's
  top at the row's y, its text cell top at +12); the stepper's buttons 40x40 at x 304 and
  408 with the value centred between; everything below moves down by the difference. A tap
  on a radio's label chooses it; a tap on `-` or `+` steps.
- **TV (scale 2)**: everything doubles; the ring on the chosen radio or the focused button is
  the cue; the panes work by arrows (a group), Tab and Enter or Space (a stepper).
- **Keyboard-only** on every form factor: each group is one Tab stop, each stepper two; from
  the default, the farthest value is 7 presses for the rate, 5 for the speed and 32 for a
  2000 ms delay, which the CLI sets in one line.
- **Headless**: no pane; `settings set` and `settings_set` are the whole capability.

## 8. What an agent sees (D161, `wm show`)

Display: a `radio group` named `Scale` with `radio`s `1x` and `2x`, the saved one `selected`,
the group flagged `disabled` without a disk or on an odd resolution; `Resolution`,
`1280 × 800`, `Takes effect at the next restart`, the status line, `Pointer` and `Software`
`label`s with their drawn text. Mouse: `radio group`s named `Buttons` (`Right-handed`,
`Left-handed`) and `Scrolling` (`Traditional`, `Natural`); `button`s named `-` and `+`, each
stepper's pair listed right after its name label (`Pointer speed`, `Repeat delay`, `Repeat
rate`) with the value `label` between them, the end one flagged `disabled`; the status line a
`label`. Nothing here is person-only: an agent with the grant `settings_set` needs may set
every one of these already, and `Select` on a radio or `Invoke` on a button is the same write.
An agent that wants a value calls `settings_get`, and one that wants an exact number calls
`settings_set` rather than pressing `+` thirty times (D22).

## 9. Budget

`CAPACITY` is 149 once the time zone picker lands (140 before it). This brief changes the tree
by **+15**: Appearance −3 (2.4), Display +2 (today 7, now 9: the value of `Scale` becomes the
group, plus the hint and the status line), Mouse +8 (today 2, now 10: the `Coming in a later
version.` line goes), Keyboard +8 (today 7, now 15). So `CAPACITY` becomes **164**, and the
installer's `eggos_app::dialog::CAPACITY` stays equal to it. Per pane, container excluded:
Display 9, Mouse 10, Keyboard 15, Appearance 7.

## 10. Acceptance (host tests, default 640x440, scale 1x; facts `resolution` (1280, 800),
`scale` 1, `scale_in_force` 1, `on_disk` true, `input` the defaults unless said)

- The sidebar's `Display` row is (0, 76, 176, 32) and `Mouse` (0, 140, 176, 32); `displays`,
  `retina`, `restart` and `scale` each match Display and nothing else; `speed`, `natural`,
  `left-handed`, `swap` and `tracking` Mouse alone; `repeat delay`, `delay` and `typing`
  Keyboard alone; `pointer` Display and Mouse.
- `CAPACITY` is 164 and the tree holds 164. Display, in window coordinates: heading at
  (192, 16); `Resolution` at (192, 76) and `1280 × 800` ending at 624, cell top 76; `Scale`
  at (192, 116); the group (528, 110, 96, 28) with `1x` at (528, 110, 36, 28) selected and `2x`
  at (588, 110, 36, 28); the hint ending at 512, cell top 116, `Takes effect at the next
  restart` in `text.dim`; the status line at (192, 156) reading `At 2x the desktop looks like
  640 × 400.` in `text.dim`; `Pointer` at (192, 196) and `Software` ending at 624.
- A click at (600, 124) writes `display.scale=2` (one `save_display_scale(2)`), selects `2x`
  and shows `From the next restart the desktop looks like 640 × 400.` in `text`; a click at
  (540, 124) writes `1` and shows the `At 2x` line again. Right on the focused group writes
  `2`. With `scale` 2 and `scale_in_force` 2 the group shows `2x` and the line reads `Drawn
  like 640 × 400 and doubled to 1280 × 800.`; with `scale` 1 and `scale_in_force` 2,
  `Resolution` still reads `1280 × 800` and the line `From the next restart the desktop looks
  like 1280 × 800.` in `text`. With `on_disk` false the group is disabled, a click at
  (600, 124) writes nothing and the line reads `No disk attached: the scale cannot be saved for
  the next restart.`; with `resolution` (1281, 801) the group is disabled on `1x` and the line
  reads `2x is not available: 1281 × 801 cannot be halved.` A write answered `EPERM` puts `1x`
  back and shows `Not allowed: this user may not set the scale.`; `ENOENT` shows `No disk
  attached: the scale could not be saved.`; showing another pane and coming back shows the
  state line.
- Appearance holds no `Scale` row: nothing at y 272, `Primary modifier` and `Window controls`
  unchanged, `hidpi` matches no pane.
- Mouse: heading at (192, 16); `Buttons` at (192, 76); the group (429, 70, 195, 28) with
  `Right-handed` at (429, 70, 88, 28) selected and `Left-handed` at (541, 70, 83, 28);
  `Scrolling` at (192, 116); the group (466, 110, 158, 28) with `Traditional` (466, 110, 75,
  28) selected and `Natural` (565, 110, 59, 28); `Pointer speed` at (192, 156); `-` at
  (504, 150, 28, 28), the value `5` centred in (540, 156, 48, 16), `+` at (596, 150, 28, 28);
  the status line at (192, 316) hidden.
- A click at (560, 84) asks for `LeftHanded(true)` (saved as `mouse.buttons=left`, then
  applied) and selects `Left-handed`; at (580, 124) `NaturalScrolling(true)`
  (`mouse.scrolling=natural`); at (610, 164) `PointerSpeed(6)` (`mouse.speed=6`) and the value
  reads `6`; at (518, 164) then `PointerSpeed(5)`. With speed 10 the `+` is disabled and a
  click at (610, 164) asks nothing; with 1 the `-` is. Space on the focused `+` steps once.
  With `on_disk` false the status line reads `No disk attached: this choice lasts until
  power-off.` in `text.dim` and a click still asks for the change. A save answered `EPERM`
  leaves `Right-handed` selected, applies nothing and shows `Not allowed: this user may not
  change the mouse settings.`; `ENOENT` shows `No disk attached: the mouse settings could not
  be saved.`; a save that worked and an apply the server refused keeps `Left-handed` and shows
  `Saved for the next boot, but the window server did not take it.` Facts with speed 7 show
  `7`; showing the pane again after `settings set mouse.speed 3` on the CLI (new facts) shows
  `3`.
- Keyboard: `Repeat delay` at (416, 188); `-` at (504, 182, 28, 28); `400 ms` centred in
  (540, 188, 48, 16); `+` at (596, 182, 28, 28); `Repeat rate` at (416, 228); `-` at (504, 222,
  28, 28); `25/s` centred in (540, 228, 48, 16); `+` at (596, 222, 28, 28); widgets 1 to 7 at
  section 7 of `keyboard-v0.3.md`'s places.
- A click at (610, 196) asks for `RepeatDelayMs(450)` (`keyboard.repeat_delay_ms=450`) and
  the value reads `450 ms`; at (518, 196) from 437 asks for 400, at (610, 196) from 437 for
  450; at 2000 the delay `+` is disabled, at 150 the `-`. A click at (610, 236) asks for
  `RepeatRate(30)`; at (518, 236) from 25 for 20, from 5 for 2, from 23 for 20; at (610, 236)
  from 2 for 5, from 23 for 25; at 60 the `+` is disabled, at 2 the `-`. Tab from the try
  field focuses the delay `-`, then `+`, the rate `-`, `+`, then the search field. A save
  answered `EPERM` keeps `400 ms` and shows `Not allowed: this user may not change the
  keyboard settings.` at (192, 316) in `text`; without a disk that line reads `No disk
  attached: this choice lasts until power-off.` in `text.dim` and the change is still asked
  for.
- `settings get display.scale` after the click at (600, 124) prints `2`; `settings get
  mouse.buttons` after the click at (560, 84) prints `left`; `settings list` carries
  `mouse.speed=6` after the `+` click.

### Left out of 0.3.0

A slider widget (the steppers go when the toolkit grows one); exact numbers typed in the
pane (the CLI has them); a `Reset to defaults` button; a pointer-speed test area; mouse
acceleration as a setting of its own; wheel lines (the toolkit's 3 rows a tick stays fixed);
a second display, rotation and refresh rate; applying 2x without a restart; a restart
button in the Display pane (there is no power capability to call yet).
