# EggOS v0.1 Mouse devices: pointer acceleration and each device's own settings (D589)

Status: design for the Mouse pane's part of D589; D16 (Fable designs), D22 (GUI, CLI and MCP),
D23 (every device class), D195 (the pane this extends).

Design by Claude Fable for the front-ends of what the kernel now offers: a global pointer
acceleration switch (`mouse.acceleration`, `on`/`off`, on by default) beside the global speed,
buttons and scrolling, and up to 16 pointing devices each with its own optional speed,
acceleration, buttons and scrolling, each value either following the global one or set for
that device. Everything in `displays-mouse-v0.3.md` section 3 (the pane), `toolkit-v0.4.md`
sections 2 and 3 (slider, dropdown) and `settings-forms-v0.5.md` (the phone, tablet, TV and
wearable forms) stands except where a section below says otherwise. Numbers are px at 1x in
pane coordinates (origin window x 176, y 0; 464 wide at the default size, so the right-aligned
edge is x 448); they double at 2x.

## 0. What a person gets, and how that is checked

- An `Acceleration` row under `Pointer speed`, `On` or `Off`, that a Windows ("Enhance pointer
  precision"), macOS ("Pointer acceleration") or GNOME ("Mouse Acceleration") user finds by any
  of those names.
- With two or more pointing devices, a `Settings for` picker at the top of the pane: `All
  devices` shows the four global controls as today; a device shows the same four rows as
  dropdowns whose first option is `Same as all devices`, and a button that puts the whole
  device back to following all devices.
- The same values on the CLI and MCP: `mouse.device.<n>.speed`, `.acceleration`, `.buttons`,
  `.scrolling`, each `global` or a value, and `mouse.devices`, the devices present.
- Checked by host tests (section 8) at 640x440 and at 390 wide, and by the CLI's tests.

## 1. The pane: two views, one set of rows

The pane has an **All devices view** (today's controls plus the acceleration row) and a
**device view** (the same rows as dropdowns). The picker chooses between them; nothing is
written by switching. Rows are the 40 px grid: text cell top at row + 12, controls at row + 6.

| # | Widget | Place and size | Text and colour | Shown |
|---|--------|----------------|-----------------|-------|
| 1 | heading | (16, 16), 2x | `Mouse` | always |
| 2 | label | (16, 76) | `Settings for` in `text` | picker shown (1.1) |
| 3 | label, right-aligned | ends at x 216, cell top 76 | the device's kind (3.1) in `text.dim`; hidden for `All devices` | picker shown |
| 4 | dropdown | (232, 70), 216x28 | name `Settings for`; options `All devices`, `Device 1`, `Device 2`, ... (3.1) | picker shown |
| 5 | label | (16, R1 + 12) | `Buttons` | always |
| 6 | radios, horizontal | right-aligned, ends at 448, R1 + 6, 195x28 | `Right-handed` `Left-handed` | all-devices view |
| 7 | dropdown | (232, R1 + 6), 216x28 | name `Buttons`; `Same as all devices`, `Right-handed`, `Left-handed` | device view |
| 8 | label | (16, R2 + 12) | `Scrolling` | always |
| 9 | radios, horizontal | right-aligned, ends at 448, R2 + 6, 158x28 | `Traditional` `Natural` | all-devices view |
| 10 | dropdown | (232, R2 + 6), 216x28 | name `Scrolling`; `Same as all devices`, `Traditional`, `Natural` | device view |
| 11 | label | (16, R3 + 12) | `Pointer speed` | always |
| 12 | slider | (232, R3 + 6), 216x28 | name `Pointer speed`, 1 to 10, readout (toolkit v0.4 2.2) | all-devices view |
| 13 | dropdown | (232, R3 + 6), 216x28 | name `Pointer speed`; `Same as all devices`, `1`, `2`, ... `10` | device view |
| 14 | label | (16, R4 + 12) | `Acceleration` | always |
| 15 | label, right-aligned | ends 16 px before widget 16 or 17, cell top R4 + 12 | `Farther the faster you move it.` in `text.dim` | always |
| 16 | radios, horizontal | right-aligned, ends at 448, R4 + 6, about 103x28 | `On` `Off`; `On` for `mouse.acceleration=on` | all-devices view |
| 17 | dropdown | (232, R4 + 6), 216x28 | name `Acceleration`; `Same as all devices`, `On`, `Off` | device view |
| 18 | push button | right-aligned, ends at 448, (R5 + 6), `Length::Content` (about 132x28) | `Same as all devices` | device view |
| 19 | label | (16, 316) | the status line (4.3), `text.dim` or `text`; hidden when it has nothing to say | always |

**Rows**: with the picker shown, R1..R5 are 104, 144, 184, 224, 264 (the picker takes row 64,
as KDE's `Device` row does); with the picker hidden, R1..R4 are 64, 104, 144, 184 and there is
no R5 (no device view exists). Content ends at 292 or 212; the status line keeps the three
input panes' y. The device view's dropdowns sit exactly where the slider sits, so the control
column is one column in both views: a value is always at x 232..448.

### 1.1 When the picker shows

The picker (widgets 2 to 4) shows when **two or more pointing devices are present, or any
device number has a value of its own** (one set from the CLI on a one-mouse machine must stay
visible, or the slider would show 5 while the mouse moves at 7). With one device and nothing
set per device, the pane is today's plus the acceleration row. With no pointing device, the
same, and the status line says so (4.3): the values are saved and apply to the mouse that
comes, as the Keyboard pane keeps its rows without a keyboard.

## 2. The acceleration row (widgets 14 to 17)

- **A radio group `On` `Off`**, not a checkbox: the pane's rows are "name left, the value's
  own words right" (`Traditional` `Natural` are the setting's values), and `On`/`Off` are
  `mouse.acceleration`'s. A checkbox `Pointer acceleration` (macOS's and GNOME's control)
  would be one widget fewer but would hide the value's name and break the column.
- **The label is `Acceleration`**: under `Pointer speed` in a pane named `Mouse` the word needs
  no prefix, and it is the word every switcher knows. The other systems' names are **search
  words**: `Pointer acceleration`, `Mouse acceleration`, `Enhance pointer precision`,
  `precision`, plus `devices`, `tablet`, `pen` and `per device` for section 3 (`acceleration`
  and `touchpad` are there already). Any of them finds Mouse alone.
- **The help text** (widget 15) says what it does, not what others call it: `Farther the
  faster you move it.` (about 150 px), right-aligned 16 px before the group, as the Display
  pane's `Takes effect at the next restart` sits before its `1x` `2x` group. In the device
  view it sits before the dropdown (ends at x 216), where it still fits (`Acceleration` ends
  at about x 78).
- **Choosing** writes `mouse.acceleration` and applies it, as the other rows do (4.1). Default
  `On`.

## 3. The device picker and the device view

### 3.1 Naming the devices

The kernel tells devices apart by their place among the input sources and has no product
names (D589), so the pane does not invent any. A device is **`Device <n>`**, where `n` is its
place among the **pointing devices** (sources with relative or absolute pointer axes; touch
screens are not pointing devices and have no speed), in the kernel's order, counted from 1.
The same `n` is the one in `mouse.device.<n>.*` (section 6) and in `mouse.devices`: one name
in the pane, the CLI and MCP. `Mouse 1` was rejected because a touchpad or a trackpoint
reports as a relative mouse and would be called a mouse falsely.

What the kernel does know shows as the **kind hint** (widget 3) beside the picker, in
`text.dim`, for the picked device:

| The source | Hint |
|------------|------|
| relative axes | `a mouse or touchpad` |
| absolute axes (a pen tablet, QEMU's tablet) | `a tablet or pen` |
| neither known | `a pointing device` |
| a listed number with no device at that place (a value saved for an unplugged mouse, or a gap below one) | `not attached` |

Two devices of the same kind are told apart only by their number, which is the order the
kernel found them (the built-in touchpad before a plugged-in mouse, on a laptop). Telling them
apart by moving one is left out (section 9).

### 3.2 The picker (widget 4)

- Options: `All devices`, then `Device 1` up to the highest number that is present or has a
  value of its own (`Device 3` with the hint `not attached` when only two are present, so a
  value saved for an unplugged mouse can still be reset here). At most 17 options; the popup
  shows 8 and scrolls. The texts are a prefix of one static table (`All devices`, `Device 1`,
  ..., `Device 16`), which the toolkit's `&'static` options take as they are; a number in the
  middle with no device and no value is listed too, with the hint `not attached`, rather than
  asking the toolkit for program-owned option texts (section 10, point 13).
- **Choosing** (`Response::Chosen`) switches the view in the same frame and writes nothing:
  `All devices` shows widgets 6, 9, 12, 16 and hides 7, 10, 13, 17, 18 and the hint; a device
  shows the dropdowns filled from that device's saved values, the reset button, and the hint.
  Up/Down on the closed picker switch the view at once (toolkit v0.4 3.4): still no write.
- The picker opens on `All devices` when the window opens, keeps its choice while the window
  is open, and goes back to `All devices` when the pane is shown and the picked device is no
  longer listed.

### 3.3 The device view's rows (widgets 7, 10, 13, 17)

- Each dropdown's first option is **`Same as all devices`**: the device follows the global
  value, which is what the All devices view shows. The other options are the global control's
  words, so `Right-handed` means the same thing in both views.
- **Choosing an option** is the change (4.1): `Same as all devices` saves `global`, any other
  saves that value; the button is retitled in the same frame. Up/Down on a closed row dropdown
  choose and write at once, as the radios do on arrows.
- **A tablet or pen** (absolute axes): its `Pointer speed` and `Acceleration` dropdowns are
  **disabled** showing `Same as all devices`, because an absolute device moves the pointer to
  where it points and the kernel scales neither (D195); its `Buttons` and `Scrolling` work. The
  CLI still accepts a speed for it (harmless, stored for a kernel that one day uses it); the
  pane disables what has no effect.
- **`Same as all devices`** (widget 18) puts every row of the picked device back to following:
  one request, four keys saved as `global`, one apply. **Disabled** while every row already
  follows, so the pane shows "nothing set here" without a line saying it; enabled in the same
  frame a row gets a value of its own. Not primary (the pane has no default button).

## 4. Behaviour

### 4.1 Saving and applying

- **The current values** are `Facts::input` (global, as today, now with `pointer_acceleration`)
  and, new, `Facts::pointing_devices` (up to 16: the kind, and the device's four `Option`s)
  when the window opens and whenever the pane is shown; an open window follows values set from
  the CLI or MCP the next time it is focused.
- **A global change** is one `InputChange` as today, with `PointerAcceleration(bool)` added:
  saved (`mouse.acceleration=off`), then applied (`SET_INPUT_PREFERENCES` as before, and
  `SET_DEVICE_INPUT_PREFERENCES` with `EVERY_DEVICE` for the acceleration, as `settings set`
  already sends).
- **A device change** is one `Request::ChangeDevice { device: n, change }` with
  `DeviceChange::Speed(Option<u32>)`, `Acceleration(Option<bool>)`, `LeftHanded(Option<bool>)`,
  `NaturalScrolling(Option<bool>)` or `FollowAll` (`None` = follow): the program **saves,
  then applies**: writes the key (`global` removes it from the file; a value writes it), then
  sends request 60 with the device's **source place** (the program maps `n` to it from the
  device list) and all four of its saved values (0 follow, 1 on, 2 off; `right` is 2 and
  `traditional` is 2; a speed as its number, 0 to follow). The same write and the same request
  as `settings set mouse.device.2.speed 7`. Applying is instant and silent.
- **A failed save** changes nothing: the dropdown goes back to the saved option (the radios and
  slider as today), nothing is applied, and the status line says why. A save that worked but
  an apply the window server refused keeps the control and says so.

### 4.2 Summary on the phone's list page

`Right hand, natural scrolling` stays (settings-forms 1.1): the picker and acceleration do not
fit a 40-character line and are not what a person scans the list for.

### 4.3 The status line (widget 19)

| When | Text | Colour |
|------|------|--------|
| shown, no pointing device | `No mouse attached: these apply when one is.` | `text.dim` |
| shown, no disk | `No disk attached: this choice lasts until power-off.` | `text.dim` |
| shown, otherwise | hidden | |
| save refused (`EPERM`, `EACCES`) | `Not allowed: this user may not change the mouse settings.` | `text` |
| save failed otherwise | `No disk attached: the mouse settings could not be saved.` | `text` |
| saved, the window server refused the apply | `Saved for the next boot, but the window server did not take it.` | `text` |

The first line wins over the second when both hold (a no-disk machine without a mouse is told
about the mouse; the no-disk line returns once one is attached). Widest: 318 px, as today.

## 5. Keys, focus and what an agent sees

- **Tab order**: the picker (when shown), `Buttons`, `Scrolling`, `Pointer speed`,
  `Acceleration`, `Same as all devices` (device view, skipped while disabled), then wraps to the
  search field; Shift+Tab the reverse. Showing a pane does not move the focus; switching the
  view keeps the focus on the picker.
- **On a dropdown**: toolkit v0.4 section 3.4 (Space, Enter, Alt+Down open; Up/Down choose at
  once; a letter or digit jumps: `l` chooses `Left-handed`, `7` the speed 7, `s` `Same as all
  devices`). **On the radios**: arrows choose. **On the slider**: as today.
- **Pointer**: `Arrow` everywhere; a click on a radio's label chooses it; a right-click does
  nothing; a wheel over a dropdown or the slider does nothing (the pane scrolls on a phone).
- **The tree** (`wm show`, D161): a `combo-box` named `Settings for` valued `All devices` or
  `Device 2`; in the all-devices view the `radio group`s `Buttons`, `Scrolling` and
  `Acceleration` (`On`, `Off`) and the `slider` `Pointer speed`; in the device view
  `combo-box`es named `Buttons`, `Scrolling`, `Pointer speed` and `Acceleration` valued their
  option (`Same as all devices` or the value), the tablet's two flagged `disabled`; a `button`
  `Same as all devices`, flagged `disabled` when every row follows; `label`s with their drawn
  text. A hidden view's widgets are not listed. Nothing here is person-only: `Select` on a
  dropdown is the same write as `settings_set`, and an agent that wants a device's values
  calls `settings_get` (D22).

## 6. The CLI and MCP (`settings get`, `settings set`, `settings list`, `settings_get`, `settings_set`)

| Key | Values | Default | Read-only |
|-----|--------|---------|-----------|
| `mouse.acceleration` | `on`, `off` | `on` | no (exists) |
| `mouse.devices` | the pointing devices present, in order, one word each: `mouse`, `tablet` or `pointer` (3.1's kinds); the `n`th word is `Device n`; empty with none | | yes |
| `mouse.device.<n>.buttons` | `global`, `right`, `left` | `global` | no |
| `mouse.device.<n>.scrolling` | `global`, `traditional`, `natural` | `global` | no |
| `mouse.device.<n>.speed` | `global`, `1` to `10` | `global` | no |
| `mouse.device.<n>.acceleration` | `global`, `on`, `off` | `global` | no |

- **`n` is 1 to 16** (the kernel's `MOST_DEVICE_PREFERENCES`), always: the four keys exist for
  every number, present or not, so a value saved for an unplugged mouse survives and `get`
  answers it. `mouse.device.0.speed`, `mouse.device.17.speed` and `mouse.device.x.speed` are
  `no such setting`.
- **`global`**, not `default`: "default" would read as the factory value (speed 5) when the
  device actually follows a global speed of 8. It is D589's own word; the pane says it in
  plain words as `Same as all devices`, as it says `Right-handed` for `right`.
- **Output and errors** follow the command's existing wording:
  - `settings get mouse.device.1.speed` prints `global` or `7`; `settings get mouse.devices`
    prints `mouse,tablet` (or an empty line).
  - `settings set mouse.device.1.speed 7` prints nothing, exit 0, saved then applied.
  - `settings set mouse.device.1.speed 11` → stderr `mouse.device.1.speed: invalid value '11'
    (global or 1 to 10)`, the invalid-value status; `.buttons both` → `(global, right or
    left)`; `.scrolling up` → `(global, traditional or natural)`; `.acceleration yes` →
    `(global, on or off)`.
  - `settings set mouse.devices mouse` → `mouse.devices: read-only`.
  - `settings set mouse.device.3.speed 7` with two devices present is **accepted** (saved, and
    sent to slot 3, which the kernel keeps): the pane then lists `Device 3`, `not attached`.
  - `--dry-run` prints `would set mouse.device.1.speed from global to 7` as for every key.
- **`settings list`** prints, after `mouse.acceleration=on`, `mouse.devices=` and then the four
  keys of every device listed by `mouse.devices` **plus** every other number that has a value
  of its own, in number order, each device's four in the global keys' order:

  ```
  mouse.buttons=right
  mouse.scrolling=traditional
  mouse.speed=5
  mouse.acceleration=on
  mouse.devices=mouse,tablet
  mouse.device.1.buttons=global
  mouse.device.1.scrolling=global
  mouse.device.1.speed=7
  mouse.device.1.acceleration=global
  mouse.device.2.buttons=global
  mouse.device.2.scrolling=natural
  mouse.device.2.speed=global
  mouse.device.2.acceleration=global
  ```

  With no pointing device and nothing set: `mouse.devices=` and no device keys, so a one-mouse
  machine's list grows by five lines, not 64. `--json` records are the existing shape.
- **MCP**: `settings_get`, `settings_set` and the list take the same keys and values and
  answer the same texts; the tool descriptions in `userland/commands/src/tools.rs` gain one
  sentence: "`mouse.device.<n>.buttons`, `.scrolling`, `.speed` and `.acceleration` (n 1 to
  16) take the global key's values or `global` to follow it; `mouse.devices` lists the
  pointing devices present."

## 7. Every device class (D23)

- **Narrow page** (a phone, a tablet side or desktop window under 624: settings-forms 2, R2
  throughout, `C` = min(`W` - 32, 480)): each row is its label at cell top `r`, its control at
  (16, `r` + 24, `C`, 48); dropdowns and the slider `C` wide; the radios content-sized (R7: one
  per line if they do not fit `C`). With the picker: `Settings for` at 64, the picker (16, 88,
  `C`, 48), the kind hint at cell top 148 (R2's line under the control), `Buttons` 184/208,
  `Scrolling` 264/288, `Pointer speed` 344/368, `Acceleration` 424/448 with its help line at
  508, `Same as all devices` (16, 544, `C`, 48) in the device view, the status line at cell
  top 608 in both views, the panel 640 tall. Without the picker, everything 120 higher
  (`Buttons` 64/88 ... the help line at 388, the status line at 424, the panel 456). The
  popup of a dropdown opens below or above as the toolkit places it.
- **Tablet with the sidebar** (settings-forms 1.2): the desktop rows on the 48 px pitch with
  40 px controls (the picker row 64, R1..R5 at 112, 160, 208, 256, 304; the status line moves
  down by the difference and the pane scrolls).
- **Touch**: a tap on a radio's label chooses; a tap on a dropdown opens its popup, a tap on a
  row chooses; the slider as toolkit v0.4 2.4.
- **TV** (scale 2, D-pad): Right from the sidebar puts the ring on the picker (or on `Buttons`
  when the picker is hidden); OK opens a dropdown, Up/Down move in it, OK chooses, Back closes
  with no write; a disabled dropdown's or button's reason shows as its tooltip (`A tablet moves
  the pointer to where it points.`, `Every row follows all devices.`). A TV without a pointing
  device shows the `No mouse attached` line and keeps every control usable, as the remote may
  be joined by a mouse.
- **Wearable** (read-only, settings-forms 1.4): the facts as lines: `Buttons Right-handed`,
  `Scrolling Traditional`, `Pointer speed 5`, `Acceleration On`, then one line per device with
  a value of its own (`Device 2 natural scrolling`, `Device 1 speed 7, left-handed`), then
  `Change these from your phone or the CLI.`
- **Headless**: no pane; section 6 is the whole capability.

## 8. Budget and acceptance (host tests, default 640x440, scale 1x, window coordinates;
facts the defaults unless said; "two devices" is `mouse,tablet`)

- The Mouse pane holds 19 widgets, container excluded, **+11** on today's 8, so `CAPACITY`
  grows by 11.
- **One mouse, nothing per device**: no `Settings for`; `Buttons` at (192, 76), the group
  (429, 70, 195, 28); `Scrolling` at (192, 116); `Pointer speed` at (192, 156), the slider
  (408, 150, 216, 28); `Acceleration` at (192, 196), the help line ending 16 px before the
  group, the group right-aligned ending at 624 at y 190 with `On` selected; the status line
  hidden; the tree holds no `combo-box`. A click on `Off` asks for `PointerAcceleration(false)`
  (`mouse.acceleration=off`, applied with `EVERY_DEVICE`); Left/Right on the focused group
  choose; `settings get mouse.acceleration` then prints `off`.
- **Two devices**: `Settings for` at (192, 76), the picker (408, 70, 216, 28) valued `All
  devices` with options `All devices`, `Device 1`, `Device 2`; the hint hidden; `Buttons` at
  (192, 116), the group at y 110; `Scrolling` 156/150; `Pointer speed` 196/190; `Acceleration`
  236/230; nothing at row 264; Tab from the sidebar focuses the picker, then the four
  controls, then the search field.
- **Picking `Device 2`** (a click on the picker, then on its row): writes nothing; the hint
  reads `a tablet or pen` ending at 392, cell top 76; the radios and slider are hidden; four
  `combo-box`es at (408, 110/150/190/230, 216, 28) named `Buttons`, `Scrolling`, `Pointer
  speed`, `Acceleration`, the first two valued `Same as all devices`, the last two disabled;
  `Same as all devices` button ending at 624, y 270, disabled. Choosing `Natural` in
  `Scrolling` asks for `ChangeDevice { device: 2, NaturalScrolling(Some(true)) }` (saved as
  `mouse.device.2.scrolling=natural`, then request 60 with device 2's source place and
  `[place, 0, 0, 0, 1]`), the button enables; `s` on the focused closed dropdown chooses `Same
  as all devices` (saved `global`), the button disables. With facts `device 1 speed 7`,
  picking `Device 1` shows `Pointer speed` valued `7` and the button enabled; a click on it
  asks for `FollowAll`, saves four `global` keys, sends `[place, 0, 0, 0, 0]`, and every
  dropdown reads `Same as all devices`. Down on the focused closed picker shows `Device 1`
  and writes nothing; `All devices` shows the radios and slider again.
- **One mouse with `device 1 speed 7`** set on the CLI: the picker shows with `All devices`,
  `Device 1`. **Two devices and `device 3 speed 7`**: the picker lists `Device 3` with the
  hint `not attached`.
- **No pointing device**: no picker; the controls enabled; the status line reads `No mouse
  attached: these apply when one is.` in `text.dim`, and with `on_disk` false the same line.
- **A save answered `EPERM`** in the device view leaves the dropdown on its saved option,
  applies nothing and shows `Not allowed: this user may not change the mouse settings.`;
  `ENOENT` shows the no-disk line in `text`.
- **At 390 wide** with two devices: section 7's rects; every control 48 tall, none past x 374;
  Tab from `Back` reaches the picker first.
- **CLI**: `settings set mouse.device.1.speed 7` saves and applies; `11`, `both`, `up` and
  `yes` are refused with section 6's texts and apply nothing; `mouse.device.17.speed` is `no
  such setting`; `mouse.devices` is read-only; `settings list` with two devices and
  `device 1 speed 7` prints section 6's thirteen `mouse.` lines in that order.

## 9. Left out

The global value named inside the follow option (`Same as all devices (5)`: the dropdown's
options are static texts); telling two same-kind devices apart by moving one (a "which device
moved last?" hint needs a kernel read); product names (the input layer has none); a touchpad
distinguished from a mouse (same report); per-device wheel lines; a per-device test area;
a device's settings following it across reorderings (numbers are places, not identities);
per-device key repeat (keyboards are not pointing devices).

## 10. Decisions for the engineer

1. Acceleration is a radio group `On` `Off` on a row labelled `Acceleration`, with the help
   text `Farther the faster you move it.` right-aligned 16 px before the group; the foreign
   names are search words only. Default `On`.
2. Devices are `Device <n>`, `n` the place among the pointing devices (relative or absolute
   axes; not touch screens) from 1; the same `n` in `mouse.device.<n>.*` and `mouse.devices`.
   The kind shows as a hint beside the picker (`a mouse or touchpad`, `a tablet or pen`, `a
   pointing device`, `not attached`), never in the device's name.
3. The picker row shows when two or more pointing devices are present or any device number
   has a value of its own; it takes row 64 and pushes the four rows down by 40. Hidden, the
   pane is today's rows plus `Acceleration` at row 184.
4. The device view replaces each row's control with a dropdown at the slider's rect (232,
   row + 6, 216x28) whose first option is `Same as all devices`; the radios and slider are
   hidden, not disabled. Switching views writes nothing.
5. `Same as all devices` (the button) resets all four keys in one request and is disabled while
   every row follows. A tablet's `Pointer speed` and `Acceleration` dropdowns are disabled.
6. Keys: `mouse.device.<n>.buttons|scrolling|speed|acceleration` for `n` 1 to 16 always,
   value `global` or the global key's values; `mouse.devices` read-only, one kind word per
   device in order. Setting a number with no device present is accepted. Refusal texts follow
   the command's `<key>: invalid value '<v>' (global or 1 to 10)` shape; `n` outside 1 to 16
   is `no such setting`.
7. `settings list` prints `mouse.devices` and the four keys of every present device, plus any
   other number with a value of its own, after `mouse.acceleration`.
8. Save, then apply: `global` removes the key from the file; request 60 carries the device's
   source place (mapped from `n` by the program) and all four values (0 follow, 1 on, 2 off,
   `right` and `traditional` being 2; speed its number or 0).
9. `Facts` gains `pointing_devices` (up to 16: the kind and the four `Option`s), read from a
   new kernel or window-server read that lists input sources with their pointer axes; the
   engineer chooses the carrier (`/proc` or a request) and records it.
10. Status line: `No mouse attached: these apply when one is.` in `text.dim` wins over the
    no-disk line; the error lines are today's.
11. Narrow pages: R2 throughout with the numbers of section 7; the tablet's sidebar form on the
    48 px pitch; the TV enters the pane on the picker; the wearable lists one line per device
    with its own values.
12. `CAPACITY` grows by 11; the phone's summary line stays `Right hand, natural scrolling`.
13. Toolkit ask, only if needed: a dropdown with program-owned option texts, so the picker can
    list exactly the present and set numbers; until then it lists `All devices` and `Device 1`
    to the highest present or set number as a static prefix, absent numbers between marked
    `not attached`.
