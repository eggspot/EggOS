# EggOS v0.4 Display resolution in Settings > Display: design brief (D287)

Design by Claude Fable (D16) for the GUI front-end (D22) of the settable `display.resolution`
(D287): `settings set display.resolution <W>x<H>` or `auto`, saved to the settings file and in
force from the next boot, as `display.scale` is. It extends `displays-mouse-v0.3.md` section 2:
the same grid, tokens, type and voice; the `Resolution` row gains a dropdown (`toolkit-v0.4.md`
section 3, no toolkit addition) and a line of its own, and the rows under it move down 40 px.
Numbers are px at 1x in pane coordinates (origin window x 176, y 0; 464 wide, right edge x 448)
and double at 2x; no motion is added.

**The data.** `Facts` keeps `resolution` (the size in force, as today) and gains
`resolution_saved` (`None` for `auto`, or one of the twelve sizes), `resolution_changeable`
(the backend's `display.resolution_changeable`), `resolution_preferred` (the display's own size,
ask 2) and `desktop_minimum` (the smallest desktop the terminal window fits, ask 3). **The size
at the next restart** is `resolution_saved`, or `resolution_preferred` for `auto`. A size is
**offered** at scale `k` when its width and height, divided by `k`, are whole and not under
`desktop_minimum`; `k` is the saved scale.

## 1. Layout

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Display` |
| 2 | label | (16, 76) | `Resolution` in `text` |
| 3 | label, right-aligned | ends at x 272, cell top 76; ends at x 448 when 4 is hidden | the size in force, `1280 × 800`, in `text` |
| 4 | dropdown, name `Resolution` | (288, 70), 160x28, ends at x 448 | `Automatic`, then the offered sizes, smallest first |
| 5 | label, 432 wide | (16, 116) | the resolution line (section 3), `text.dim` or `text` |
| 6 | label | (16, 156) | `Scale` in `text` |
| 7 | radios, horizontal | right-aligned, ends at x 448, y 150, 96x28 | `1x` `2x` |
| 8 | label, right-aligned | ends at x 336, cell top 156 | `Takes effect at the next restart` in `text.dim` |
| 9 | label | (16, 196) | the scale's status line, `text.dim` or `text` |
| 10 | label | (16, 236) | `Pointer` in `text` |
| 11 | label, right-aligned | ends at x 448, cell top 236 | `Software` in `text` |

- Widgets 4 and 5 are new; v0.3's widgets 4 to 9 are 6 to 11 here, each 40 px lower. Content
  ends at y 252, inside the usable 332. Display holds 11 widgets and `CAPACITY` rises by 2 (the
  installer's `dialog::CAPACITY` with it).
- **The size in force stays on the row**, 16 px before the dropdown, as the `Scale` hint sits
  before its group: the row reads `Resolution  1280 × 800  [Automatic v]`, what the screen is now
  and then what it will be. The widest value, `3840 × 2160`, ends well clear of the name.
- **Option texts** are the pane's own form, `1920 × 1080` (U+00D7 between spaces), not the
  CLI's `1920x1080`; `Automatic` is `auto`. The dropdown is 160 wide so no option is ellipsised.
  It has 13 options at scale 1x: the popup shows 8 rows and scrolls (window rect (452, 102, 184,
  258), below the button, inside the default window).
- **Not offered means left out**: the toolkit's dropdown has no disabled rows, so a size that
  the saved scale rules out is not in the list, and the resolution line says so (section 3).

## 2. The choice

The dropdown shows **the saved setting**, as the `Scale` radios do: `Automatic` for `auto`, else
the saved size. Widget 3 says what is on screen.

- **Choosing** (`Response::Chosen` on widget 4) asks the program for
  `Request::SetDisplayResolution(None | Some((w, h)))`, which writes `display.resolution` at once
  (the same write as `settings set display.resolution 1920x1080`). Nothing is applied now; in the
  same frame the resolution line and the scale's status line show the new state. No `Apply`
  button and no "keep this size?" countdown: the screen does not change until the restart, and
  choosing `Automatic` undoes it.
- **A failed write** chooses nothing: the dropdown goes back to the saved option and the
  resolution line says why in `text` (section 3). Showing the pane again puts the state line back.
- **Cannot change on this display** (`resolution_changeable` false: a firmware framebuffer, or
  the Intel display at the firmware's mode): widget 4 is hidden, widget 3 ends at x 448 as in
  v0.3, and the resolution line says the size is fixed. A dropdown that can never be used is not
  drawn disabled: the row is a fact, as before.
- **No disk** (`on_disk` false): the dropdown is disabled showing `Automatic` and the line says
  the choice cannot be saved, from the start, as the `Scale` group does.
- **Too small for scale 2x**, both ways round, so neither control leads to a refused write:
  - saved scale `2x`: the sizes that are not offered at 2x are left out of the dropdown, and the
    resting line names the way to them.
  - saved scale `1x` and the size at the next restart is not offered at 2x: the `Scale` group is
    disabled on `1x` and its status line says why (section 3).
  - a saved pair the two rules exclude (written by hand in the settings file): the dropdown
    shows `Automatic`, the group the scale in force, and neither claims a pending change.
- An open window follows a size set from the CLI or MCP the next time it is focused.

## 3. The lines

**The resolution line (widget 5)**, first match wins. `S` is the saved size, `P` the display's own.

| State | Text | Colour |
|-------|------|--------|
| write refused (`EPERM`, `EACCES`) | `Not allowed: this user may not set the resolution.` | `text` |
| write refused (`EINVAL`) | `This size cannot be used with the scale chosen below.` | `text` |
| write failed otherwise | `No disk attached: the resolution could not be saved.` | `text` |
| cannot change | `This display's size is set by the computer and cannot be changed.` | `text.dim` |
| no disk | `No disk attached: the resolution cannot be saved for a restart.` | `text.dim` |
| a size saved, not the one in force | `From the next restart the display is S.` | `text` |
| `auto` saved, `P` not the size in force | `From the next restart the display uses its own size, P.` | `text` |
| resting, saved scale `2x` and a size left out | `Scale 2x leaves out the smallest sizes. Choose 1x to use them.` | `text.dim` |
| resting | `Automatic uses the display's own size.` | `text.dim` |

A pending change is in `text` because the user caused it, as v0.3's status line has it. Every
line is under 70 characters and fits the 432 px between the paddings at Egg Sans 16.

**The scale's status line (widget 9)** keeps v0.3 section 2.3's lines with two changes:

- The lines about the next restart (`From the next restart the desktop looks like ...`) and the
  `At 2x the desktop looks like ...` line use **the size at the next restart**, so a pending
  `1920 × 1080` at 2x reads `From the next restart the desktop looks like 960 × 540.` `Drawn
  like ... and doubled to ...` stays about the size in force.
- A new state, before the plain lines: saved scale `1x` and the size at the next restart not
  offered at 2x → the group disabled on `1x` and `2x is not available at 1024 × 768: choose a
  larger size first.` in `text.dim`.

## 4. Focus, keys and pointer

- **Tab** from the sidebar list enters the pane at the `Resolution` dropdown, then the `Scale`
  group, then wraps to the search field; Shift+Tab the reverse. A hidden or disabled control is
  skipped. Showing the pane does not move the focus.
- **On the dropdown** the toolkit's keys stand: Space, Enter or Alt+Down open it; Up/Down on the
  closed dropdown choose the previous or next option at once, each step one write; Home chooses
  `Automatic`, End the largest size; a digit jumps to the next size starting with it (`2` reaches
  `2560 × 1440`), `a` to `Automatic`; Esc closes the popup without a write.
- **Pointer**: `Arrow`; a press opens the popup with the saved option highlighted and marked
  `>`; a wheel over the popup scrolls it, over the closed dropdown does nothing.
- **Search words** added to Display: `screen size`, `mode`, `1080p`, `1440p`, `4K`, `Full HD`,
  `Automatic`. `Resolution` is there already.

## 5. Every device class (D23)

- **Phone** (one column, pane narrower than 448): the `Resolution` row keeps its line (name
  left, the size in force right-aligned to the edge); the dropdown under it at (16, 100), the
  width minus 32, 48 tall, its popup rows 48 tall; the resolution line at (16, 160); `Scale`
  and its group at cell top 196 (group y 190); the hint at (16, 236); the scale's status line
  at (16, 256); `Pointer` at cell top 296. The pane scrolls in the window's scroll view.
- **Tablet / touch**: the desktop layout on a 48 px pitch with 40 px controls: the dropdown at
  (288, 64), 160x40; the resolution line at cell top 124; `Scale` at 172 with its group at
  y 160; its status line at 220; `Pointer` at 268.
- **TV (scale 2)**: everything doubles; Right from the sidebar puts the ring on the dropdown;
  OK opens the popup, Up/Down move in it, OK chooses, Back closes it; Down goes to the `Scale`
  group. On a TV whose size cannot change the ring goes to the group.
- **Wearable**: the read-only row, as v0.3. **Headless**: no pane; `settings set` and
  `settings_set` are the whole capability.

## 6. What an agent sees (D161, `wm show`)

A `combo-box` named `Resolution`, its value the saved option's text (`Automatic`, `1920 ×
1080`), flagged `disabled` without a disk and absent when the size cannot change; under it one
`list-item` per offered option, the saved one `selected`. Before it the `label`s `Resolution`
and the size in force, after it the resolution line as a `label` with its drawn text. `select`
on an option is the same write as the click; an agent that wants the value or an exact size
calls `settings_get` and `settings_set` (D22). Nothing here is person-only.

## 7. Acceptance (host tests, default 640x440, scale 1x, window coordinates; facts
`resolution` (1280, 800), `resolution_saved` `None`, `resolution_preferred` (1280, 800),
`resolution_changeable` true, `desktop_minimum` (640, 400), `scale` 1, `scale_in_force` 1,
`on_disk` true unless said)

- `CAPACITY` is 2 more than before and the tree holds it. `Resolution` at (192, 76);
  `1280 × 800` ending at 448, cell top 76; the dropdown at (464, 70, 160, 28) showing
  `Automatic` with 13 options, the last `3840 × 2160`; the resolution line at (192, 116) reading
  `Automatic uses the display's own size.` in `text.dim`; `Scale` at (192, 156); the group
  (528, 150, 96, 28); the hint ending at 512, cell top 156; the scale's line at (192, 196);
  `Pointer` at (192, 236) and `Software` ending at 624. `resolution_shown()` is `1280 × 800`.
- A press at (540, 84) opens the popup (452, 102, 184, 258) with 8 rows and a scroll bar; a
  click at (540, 279), the sixth row, asks for `SetDisplayResolution(Some((1440, 900)))` once,
  the dropdown shows `1440 × 900`, widget 3 still reads `1280 × 800`, the line reads `From the
  next restart the display is 1440 × 900.` in `text`, and the scale's line reads `At 2x the
  desktop looks like 720 × 450.` Esc on the open popup asks nothing.
- Down on the focused closed dropdown from `Automatic` asks for `Some((1024, 768))`; Home then
  asks for `None`; End for `Some((3840, 2160))`. Tab from the sidebar list focuses the dropdown,
  Tab again the `Scale` group.
- `resolution_saved` (1920, 1080) with `resolution` (1920, 1080): the dropdown shows `1920 ×
  1080` and the line is the resting one. Choosing `Automatic` then, with `resolution_preferred`
  (2560, 1440), reads `From the next restart the display uses its own size, 2560 × 1440.`
- `resolution_changeable` false: the dropdown is hidden, `1280 × 800` ends at 624, the line
  reads `This display's size is set by the computer and cannot be changed.` in `text.dim`, a
  click at (540, 84) asks nothing, and Tab from the sidebar list focuses the `Scale` group.
- `on_disk` false: the dropdown is disabled on `Automatic`, a press at (540, 84) opens nothing,
  and the line reads `No disk attached: the resolution cannot be saved for a restart.`
- `scale` 2 (saved): the dropdown has 10 options, `Automatic` then `1280 × 800` and `1440 × 900`
  (no `1024 × 768`, `1280 × 720` or `1366 × 768`, whose halves are under 640 × 400), and the resting line reads `Scale 2x leaves out the smallest sizes.
  Choose 1x to use them.`
- `scale` 1 and `resolution_saved` (1024, 768): the `Scale` group is disabled on `1x`, a click
  at (600, 164) writes nothing, and the scale's line reads `2x is not available at 1024 × 768:
  choose a larger size first.` With `resolution_saved` (1920, 1080) and `scale` 2 pending, it
  reads `From the next restart the desktop looks like 960 × 540.`
- A write answered `EPERM` puts `Automatic` back and shows `Not allowed: this user may not set
  the resolution.`; `EINVAL` shows `This size cannot be used with the scale chosen below.`;
  `ENOENT` shows `No disk attached: the resolution could not be saved.`; showing another pane
  and coming back shows the state line.
- Narrower than 448: the dropdown at pane (16, 100), the width minus 32, 48 tall, the line at
  (16, 160). `wm show` lists a `combo-box` named `Resolution` with value `Automatic` and 13
  `list-item`s. `4k`, `1080p` and `screen size` each match Display and nothing else.
- `settings get display.resolution` after the click at (540, 279) prints `1440x900`; after
  choosing `Automatic`, `auto`.

## 8. Deviations and backend asks

Deviations from `displays-mouse-v0.3.md`: the `Resolution` value moves from x 448 to x 272 while
the dropdown shows; rows `Scale`, its status line and `Pointer` are 40 px lower; the scale's
lines about a restart use the size at the next restart; "a second display, rotation and refresh
rate" stay left out, and so does a restart button (no power capability yet).

For the backend (D287):

1. **Which key reads what.** `display.resolution` now holds the saved choice (`auto` or a size).
   The size in force needs a read-only key of its own (`display.resolution_in_force`), so the
   pane, `settings list` and an agent can tell "chosen" from "on screen", as `Facts::scale` and
   `scale_in_force` do.
2. **`display.resolution_preferred`**, read-only: the display's own size, so the pane can say
   what `Automatic` will give and that a restart is pending after going back to `auto`. Without
   it the pane shows that line only after its own write.
3. **The desktop's minimum as a fact** (`display.minimum`, or the rule in a shared crate), so
   the pane and `settings set` leave out and refuse the same sizes at each scale; and
   `settings set` answering `EINVAL` for a size and scale that do not fit, whichever is set
   second.
4. **The sizes this display can show** (`display.resolution_choices`, a subset of the twelve),
   so the pane leaves out the rest. Until then: a saved size the display refuses at boot falls
   back to `auto`, the setting is rewritten to `auto`, and the boot log says so, so the pane
   never shows a pending change that will not come.
5. **`display.resolution_changeable` per reason** later (`firmware`, `no`), if a display that
   can change after a driver loads should say `Install the display driver in Settings >
   Components.` instead of the fixed-size line.
6. Later, applying a size without a restart; then the hint, the pending lines and widget 3's
   split go away and the dropdown gets a `Keep this size?` note with a countdown.
