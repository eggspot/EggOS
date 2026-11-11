# EggOS v0.5 Rotation in Settings > Display: design brief (D330)

Design by Claude Fable (D16) for the desktop GUI front-end (D22) of `display.rotation` (D330):
`auto` (the default: an orientation sensor, or upright while no driver reports one), `0`, `90`,
`180` or `270` degrees clockwise, saved to the settings file and applied at once (the screen
turns and the desktop takes its new size). The CLI (`settings set display.rotation 90`), MCP
(`settings_set`) and the phone shade's `Rotation` tile (`adaptive-shell-v0.5.md` 2.6 item 19,
`Automatic`/`Locked`) exist; this brief adds the pane's row, which is the same write and the same
`SET_ROTATION` request. It extends `display-resolution-v0.4.md` and `displays-mouse-v0.3.md`
section 2: the same grid, tokens, type and voice; one dropdown (`toolkit-v0.4.md` section 3, no
toolkit addition) and one line of its own. Numbers are px at 1x in pane coordinates (origin
window x 176, y 0; 464 wide, right edge x 448) and double at 2x; the turn itself is one whole-
screen damage, no motion.

**The data.** `Facts::rotation` (the saved value's index in `DISPLAY_ROTATIONS`, there already)
is what the dropdown shows. `Facts::resolution` stays the device's pixels: a quarter turn makes
the desktop `800 × 1280`, but the `Resolution` row keeps reading `1280 × 800`, the display did
not change (ask 1 for a line that names the turned size).

## 1. Layout

The row sits **under the `Scale` row's status line and before `Pointer`**: the three picture
settings together, in Windows's order (`Scale`, `Display resolution`, `Display orientation`), and
the two "next restart" rows stay next to each other, so the one that acts now does not sit
between them. `Pointer` and `Layout` move down.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 10 | label | (16, 236) | `Rotation` in `text` |
| 11 | dropdown, name `Rotation` | (288, 230), 160x28, ends at x 448 | `Automatic`, `Upright`, `90° clockwise`, `Upside down`, `90° counter-clockwise` |
| 12 | label, 432 wide | (16, 276) | the rotation line (section 3), `text.dim` or `text` |
| 13 | label | (16, 316) | `Pointer` in `text` (was 236: **+80**) |
| 14 | label, right-aligned | ends at x 448, cell top 316 | `Software` in `text` (+80) |
| 15 | label | (16, 356) | `Layout` in `text` (was 276: +80) |
| 16 | dropdown, name `Layout` | (288, 350), 160x28 | as today (+80) |
| 17 | label | (16, 396) | the layout line, as today (was 316: +80) |

- Widgets 1 to 9 of `display-resolution-v0.4.md` section 1 stay where they are. Widgets 10 to
  12 are new; `ROTATION_TOP` is 236 and `LAYOUT_TOP` becomes 356. Content ends at y 412, so
  the Display pane gets a height of its own, **`DISPLAY_PANE_HEIGHT` 428** (as Date & time has
  408), inside the default window's 440; the other panes keep `PANE_HEIGHT` 348 and gain no
  scroll bar. Display holds 17 widgets; `CAPACITY`'s base rises by 3 (204 to 207), the
  installer's `dialog::CAPACITY` with it.
- **Option words** are the turn from upright, in degrees, because "landscape" and "portrait"
  mean opposite things on a monitor and a phone (D23) and the setting's own values are
  degrees, so a person who read `settings list` recognises `90` as `90° clockwise`. `Upright`
  is `0`, `Upside down` is `180` (plainer than `180°`), `90° counter-clockwise` is `270` (what
  the person did to the monitor, not the arithmetic). `°` is U+00B0, in the UI font. The widest
  option, `90° counter-clockwise`, is 21 characters, about 150 px with the 12 px text inset
  and the 32 px chevron room: the 160 px dropdown ellipsises nothing, and the popup
  (184 wide) shows it whole.
- The dropdown shows **the saved setting**, as `Resolution` and `Layout` do; the line says what
  is on screen.

## 2. The choice

- **Choosing** (`Response::Chosen` on widget 11) asks the program for
  `Request::SetDisplayRotation(index)` (0 to 4 in `DISPLAY_ROTATIONS`, the numbering
  `SetShellForm` and `SetOnScreenKeyboard` use), which **saves, then applies**:
  `save_rotation(index)`, then `apply_rotation(index)` (the server's `SET_ROTATION`), the same
  two steps as `settings set display.rotation 90`. The screen turns in that frame and the
  desktop resizes; the window keeps its place inside the new work area (the desktop's resize
  clamps it) and is re-laid out; the dropdown shows the new option and the rotation line the
  new state (section 3). The program reports back with `display_rotation_saved(index, saved,
  applied)`, which the pane shows through `rotation_saved`, as `input_changed` does.
- **No `Keep these settings?` countdown.** Windows and GNOME revert a display change after 15 s
  because a wrong mode can leave the screen black. A turn is software only: the mode does not
  change, the picture is always drawn, and a mouse's motion is turned with it (D330), so the
  pointer still goes where the hand goes on the physical screen and the row stays reachable;
  at worst a desktop user reads an upside-down window for one more click. The way back is one
  choice (`Home` on the focused dropdown chooses `Automatic`; the phone's tile, or
  `settings set display.rotation auto` from a terminal, undo it too), and the shade tile turns
  with no confirmation already. A dialog would be drawn turned as well, and a timer would be a
  new toolkit feature for a screen that was never lost.
- **A failed save** chooses nothing: the dropdown goes back to the saved option, nothing is
  applied, and the line says why in `text` (section 3). A save that worked but an apply the
  server refused keeps the chosen option (the kernel applies the saved value at the next boot,
  before the desktop starts) and says so.
- **No disk** (`on_disk` false): the settings file is in memory, so the save works and the turn
  lasts the session. The dropdown stays **enabled** and the resting line says the choice lasts
  until power-off, as the Mouse pane's does (a setting that acts now is not refused for lack of
  a disk; `Resolution`, which only acts at a restart, is).
- An open window follows a rotation set from the CLI, MCP or the shade tile the next time it is
  focused, as it follows `display.resolution`. Choosing a fixed turn here makes the tile read
  `Locked`; the tile's next tap goes back to `Automatic`, and the row follows.

## 3. The rotation line (widget 12)

First match wins.

| State | Text | Colour |
|-------|------|--------|
| save refused (`EPERM`, `EACCES`) | `Not allowed: this user may not turn the screen.` | `text` |
| save failed otherwise | `No disk attached: the rotation could not be saved.` | `text` |
| saved, apply refused `ENOMEM` | `Saved, but there is not enough memory to turn the screen now.` | `text` |
| saved, apply refused otherwise | `Saved for the next boot, but the window server did not take it.` (the Mouse pane's line, verbatim) | `text` |
| no disk | `No disk attached: the rotation lasts until power-off.` | `text.dim` |
| `auto` saved | `Automatic follows the device's orientation; with no sensor it stays upright.` | `text.dim` |
| `0` saved | `The screen is upright.` | `text.dim` |
| `90` saved | `The screen is turned 90° clockwise.` | `text.dim` |
| `180` saved | `The screen is upside down.` | `text.dim` |
| `270` saved | `The screen is turned 90° counter-clockwise.` | `text.dim` |

The state lines are `text.dim` even right after a choice: nothing is pending, the screen already
shows the result (the Mouse pane's rule), and the error lines are `text` because the user caused
them. The `Automatic` line is honest while no sensor driver exists and stays right once one does;
when the backend exposes the rotation in force (ask 1) it becomes `Automatic: the screen is
turned 90° clockwise now.` and the like. The widest line (`Automatic ...`, 76 characters) is
about 390 px and fits the 432 px between the paddings at Egg Sans 16. Showing the pane again puts
the state line back.

## 4. Focus, keys, pointer and what an agent sees

- **Tab** from the sidebar list: the `Resolution` dropdown, the `Scale` group, the `Rotation`
  dropdown, the `Layout` dropdown, then wraps to the search field; Shift+Tab the reverse. A
  hidden or disabled control is skipped. Showing the pane does not move the focus.
- **On the dropdown** the toolkit's keys stand: Space, Enter or Alt+Down open it; Up/Down on
  the closed dropdown choose the previous or next option at once, each step one save and one
  turn; Home chooses `Automatic`, End `90° counter-clockwise`; `a` jumps to `Automatic`, `u` to
  `Upright` then `Upside down`, `9` to the two 90° options in turn; Esc closes the popup with no
  write. The popup opens below the button, (276, 262, 184, 162) in pane coordinates, five rows.
- **Pointer**: `Arrow`; a press opens the popup with the saved option highlighted and marked
  `>`; press, slide and release on a row chooses it.
- **Accessibility**: the label `Rotation` is the dropdown's name, so a screen reader and
  `wm show` say `Rotation, 90° clockwise`; the state line is a `label` with its drawn text.
  High contrast changes nothing here (no colour-only cue: the `>` marks the chosen row); reduced
  motion changes nothing (there is none).
- **`wm show` (D161)**: a `combo-box` named `Rotation`, value the saved option's text, never
  disabled; under it five `list-item`s, the saved one `selected`; before it the `label`
  `Rotation`, after it the rotation line. `select` on an option is the same write and turn as
  the click; an agent that wants the value calls `settings_get`, one that wants to set it
  `settings_set` (D22). Nothing here is person-only, and the turn is not a destructive action
  (D21): it needs no confirmation from an agent either.
- **Search words** added to Display: `Rotation`, `Orientation`, `rotate`, `turn`, `portrait`,
  `landscape`, `upside down`, `sideways`. Each matches Display and nothing else.

## 5. Every device class (D23)

- **Phone** (one column, pane narrower than 448): the row shows. The shade tile is the quick
  switch between `Automatic` and `Locked`; the pane is where all five values are, and a phone
  held in a mount wants `Upside down` or a fixed 90° too. After the scale's status line at
  (16, 256): `Rotation` at (16, 296); the dropdown at (16, 320), the width minus 32, 48 tall,
  its popup rows 48 tall; the rotation line at (16, 380); `Pointer` at cell top 416; `Layout`
  at (16, 456) with its dropdown at (16, 480), 48 tall, and its line at (16, 540). The pane
  scrolls in the window's scroll view. The home screen's search finds the row by the same words.
- **Tablet / touch**: the desktop layout on the 48 px pitch with 40 px controls: `Rotation` at
  cell top 268 with the dropdown at (288, 256), 160x40; the rotation line at cell top 316;
  `Pointer` at 364; `Layout` at 412 with its dropdown at (288, 400); the layout line at 460.
- **TV (scale 2)**: everything doubles; Down from the `Scale` group puts the ring on the
  `Rotation` dropdown, Down again on `Layout`; OK opens the popup, Up/Down move in it, OK
  chooses, Back closes it. A portrait-mounted TV is why the row stays.
- **Wearable**: left out (its own brief). **Headless**: no pane; `settings set` and
  `settings_set` are the whole capability.

## 6. Acceptance (host tests, default 640x440, scale 1x, window coordinates; `Facts::example()`
with `rotation` 0 and `on_disk` true unless said)

1. `CAPACITY`'s base is 207 and the tree holds it; the Display pane is 428 tall, the others 348.
2. `Rotation` at (192, 236); the dropdown at (464, 230, 160, 28) showing `Automatic` with 5
   options, the last `90° counter-clockwise`; the rotation line at (192, 276) reading `Automatic
   follows the device's orientation; with no sensor it stays upright.` in `text.dim`.
3. `Pointer` at (192, 316) and `Software` ending at 624, cell top 316; `Layout` at (192, 356)
   with its dropdown at (464, 350, 160, 28); the layout line at (192, 396). Widgets 1 to 9
   of `display-resolution-v0.4.md` section 7 are unmoved.
4. A press at (540, 244) opens the popup (452, 262, 184, 162) with 5 rows, `>` on `Automatic`;
   a click at (540, 343), the third row, asks for `SetDisplayRotation(2)` once. After
   `display_rotation_saved(2, Ok(()), Ok(()))` the dropdown shows `90° clockwise`, the line
   reads `The screen is turned 90° clockwise.` in `text.dim`, and `resolution_shown()` is still
   `1280 × 800`. Esc on the open popup asks nothing.
5. Down on the focused closed dropdown from `Automatic` asks for `SetDisplayRotation(1)`; Home
   then for 0; End for 4. Tab from the `Scale` group focuses the `Rotation` dropdown, Tab again
   the `Layout` dropdown, Tab again the search field.
6. `rotation` 3: the dropdown shows `Upside down` and the line `The screen is upside down.`;
   `rotation` 4: `90° counter-clockwise` and `The screen is turned 90° counter-clockwise.`
7. `on_disk` false: the dropdown is enabled, the line reads `No disk attached: the rotation
   lasts until power-off.` in `text.dim`, a click at (540, 343) still asks for
   `SetDisplayRotation(2)`, and after `Ok, Ok` the line is the no-disk one still.
8. `display_rotation_saved(2, Err(EPERM), ..)` puts `Automatic` back and reads `Not allowed:
   this user may not turn the screen.` in `text`; `Err(ENOENT)` reads `No disk attached: the
   rotation could not be saved.`; `(Ok(()), Err(ENOMEM))` keeps `90° clockwise` and reads
   `Saved, but there is not enough memory to turn the screen now.`; `(Ok(()), Err(ENODEV))`
   keeps it and reads `Saved for the next boot, but the window server did not take it.`
   Showing another pane and coming back shows the state line.
9. Narrower than 448: `Rotation` at pane (16, 296), the dropdown at (16, 320), the width minus
   32, 48 tall, the line at (16, 380), `Pointer` at cell top 416.
10. `wm show` lists a `combo-box` named `Rotation` with value `Automatic` and 5 `list-item`s;
    `rotate`, `orientation`, `portrait`, `landscape` and `upside down` each match Display and
    nothing else.
11. `settings get display.rotation` after the click at (540, 343) prints `90`, and the phone
    shade's tile reads `Locked`; after choosing `Automatic` it prints `auto` and the tile reads
    `Automatic`.

## 7. Deviations and backend asks

Deviations: `Pointer`, `Layout` and the layout line are 80 px lower than
`display-resolution-v0.4.md` and D303 place them; the Display pane has a height of its own;
D330's "the Display pane has no rotation row" is closed by this brief.

For the backend (D330):

1. **`display.rotation_in_force`**, read-only (the degrees on screen, as `scale_in_force` and
   `resolution_in_force` tell "chosen" from "on screen"), so the `Automatic` line can say what
   the sensor chose and the line can name the turned desktop size.
2. **`SET_ROTATION` answers `ENOMEM`** when the display has no memory for the turned buffer:
   today `set_rotation_setting` stores the choice and answers success with the screen unturned,
   so the pane would show `The screen is turned 90° clockwise.` over an upright screen.
3. **The desktop's resize keeps every window inside the new work area**, so the Settings window
   is still reachable after a quarter turn on a wide screen.
4. Later, with a sensor driver: whether `auto` on a desktop monitor with no sensor should be
   offered at all, or the row should start at `Upright` there.
