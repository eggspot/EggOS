# EggOS v0.1 power: the battery indicator, `power status`, the lid, critical temperature and the Power pane: design brief

Design by Claude Fable (D16) for the front-ends of the power state the kernel already reads
from ACPI (D556: lids, AC adapters, batteries, thermal zones and docks, shown in
`/proc/driver/power`), the GUI, CLI and MCP faces D22 asks for, and the two actions D556 left
out: the lid and a critical temperature. Everything in `desktop-v0.3.md` section 4 (the top
bar, the power button and its menu), `desktop-v0.3-gui.md` 6.1 (the taskbar) and 6.4 item 2
(the notice), `network-v0.3.md` section 4 (the indicator pattern this one copies) and
`adaptive-shell-v0.5.md` 2.2 (the phone's battery) stands. Numbers are px at 1x at 1280x800
and double at 2x; the bar's tooltips are Egg Mono (glyphs x 8 + 16, as the network's), chips
and menus Egg Sans. "Later" marks what this slice does not build.

## 0. What a person gets, and how that is checked

- **A battery indicator** in the bar of every profile (the taskbar's tray run, the top bar's
  indicator run), a 16x16 battery whose fill is the charge, a bolt while on line power and an
  `!` when low; a tooltip with the words; a click opens a small menu with the facts and
  `Power Settings`.
- **`power status [--json]`** and the MCP tool **`power_status`** print the same facts.
- **The lid**: `power.lid_action` (`sleep`, `nothing`, `off`). While only S5 exists, `sleep`
  powers off after a 30 s countdown notice that opening the lid cancels.
- **Heat and low battery**: a notice when a zone is hot, a 30 s countdown and an orderly
  power-off at its critical temperature; notices at 15 % and 5 %, a 60 s countdown at 3 %.
- **Settings > Power**: the facts, the lid setting and the two limits, refreshed each second.
- Host tests pin the glyphs, the words, the countdowns and the pane (section 7).

## 1. The facts, and where they flow

- The kernel keeps `PowerState { lid_open, on_line_power, batteries: [(percent, charging)],
  temperatures: [tenths of a kelvin], docked }` (D556) and gains per zone its **critical** point
  (`_CRT`; `CRITICAL_FALLBACK_C` 95.0 when the zone has none) and **hot** point (`_HOT`, else
  critical - 10). It re-reads `_TMP` every `THERMAL_POLL_S` 10 s and `_BST` every
  `BATTERY_POLL_S` 30 s besides the notifications, since not every firmware notifies.
- `/proc/driver/power` keeps its lines and the thermal line gains the limit: `thermal 0 47.0
  limit 95.0`.
- **The desktop is told on every change** (`Desktop::set_power(PowerState)`, each percent and
  each state word), which damages the glyph's rect, re-places a shown tooltip and redraws the
  Power pane's facts when it shows (section 5). No tick in the shell.
- **A battery's state word** is `charging` (from `_BST`), `full` (on line power, not charging,
  100 %), else `discharging`. The glyph shows the **mean** percent of several batteries and the
  bolt when line power is online; the words list each.
- **Read-only settings keys** (D22; `power.battery` exists in `adaptive-shell-v0.5.md`):

  | Key | Value |
  |-----|-------|
  | `power.battery` | `none`, or `87% discharging`, `100% full`, `40% charging` (the first battery) |
  | `power.line` | `online`, `offline` or `none` |
  | `power.lid` | `open`, `closed` or `none` |
  | `power.temperature` | the hottest zone in °C with one decimal, `47.0`, or `none` |
  | `power.dock` | `attached`, `detached` or `none` |

  One settable key, **`power.lid_action`**: `sleep` (the default), `nothing` or `off` (4.1).

## 2. The battery indicator

### 2.1 Place

Directly beside the power control, in the indicator run, in every profile: it is the one
indicator about power. The order from the right edge is **power button, battery, volume
(`sound-v0.1.md`), network, keyboard, tray icons, the Agent chip, the sticky chips**: Windows's
own order (network, volume, battery, clock) read from the clock leftwards.

- **Top bar (macOS and Linux)**: a 28x28 target directly left of the power button, `Rect::new(
  power_button().x - 28, bar.y, 28, 28)`: **(1216, 0, 28, 28)** at 1280, the glyph at (1222, 6).
  The volume target follows at 1188 and the network's at 1160; the Agent chip's right edge is
  8 px left of the run's leftmost target, as today.
- **Taskbar (Windows)**: a 40x40 item whose right edge is 8 px left of the clock's rect:
  **(1096, 760, 40, 40)** at 1280, the glyph at (1108, 772); volume at 1056, network at 1016;
  the taskbar items' room ends 16 px before the run's leftmost item (6.4 item 13).
- **Shown only when a battery is reported**: a desktop with an adapter and no battery draws
  nothing and shifts nothing (the network brief's rule). Whether a battery exists is known at
  boot and never changes while the desktop runs (a hot-plugged battery is later).
- Hover and pressed as the network's (no hover on the top bar, `bar.pressed` rows 0..26 while
  pressed; `bar.hover` and `bar.pressed` rows 1..39 on the taskbar); `bar.pressed` while its
  menu is open (6.4 item 8's rule). Pointer `Arrow`.

### 2.2 The glyph (16x16; `X` = `text`, `F` = the fill, `a` = `accent`, `.` = the bar)

A battery lying down: body outline rows 4..11 across x 0..12, a nub at x 13..14 rows 6..9,
and nine fill cells at x 2..10 on rows 6..9. Drawn from a table of rectangles as the network's.

```
................
................
................
................
XXXXXXXXXXXXX...
X...........X...
X.FFFFFFFFF.XXX.
X.FFFFFFFFF...X.
X.FFFFFFFFF...X.
X.FFFFFFFFF.XXX.
X...........X...
XXXXXXXXXXXXX...
................
................
................
................
```

- **Fill**: `filled = (percent x 9 + 99) / 100` cells from the left (1 % shows one cell, 100 %
  nine, 0 none), in `text`.
- **On line power** (charging or full): a 4-row bolt in `accent` above the body, rows 0..3:
  row 0 x 8..9, row 1 x 7..8, row 2 x 6..9, row 3 x 7. The bolt means "plugged in", whatever
  the battery does; `full` and `charging` differ in the words, not the glyph.
- **Low** (15 % or under, not on line power): the fill cells are `accent` and an `accent` `!`
  sits at x 6, rows 6, 7 and 9 (the fill is at most two cells, so they never meet). Shape and
  colour both change (the network brief's rule: no state by colour alone).
- Contrast: `text` on `bar.bg` 14.3:1, `accent` on `bar.bg` 9.4:1; in the light theme `text`
  `#161B22` 14.7:1 and `accent` `#9A6200` 4.3:1 (a graphic ≥ 3:1); the high-contrast palettes
  as `self-drawn-themes-v0.1.md`.
- At scale 2 the glyph pixel-doubles (32x32).

### 2.3 Tooltip and accessible name

After 600 ms, the panel tooltip (y 32 under the top bar, y 732 above the taskbar, centred on
the target, clamped 8 px inside the screen):

| State | Text | Glyphs | Width |
|-------|------|--------|-------|
| discharging | `Battery: 87%, discharging` | 26 | 224 |
| charging | `Battery: 87%, charging` | 22 | 192 |
| full | `Battery: 100%, full` | 19 | 168 |
| low | `Battery: 12%, low` | 17 | 152 |
| two batteries | `Batteries: 87% and 40%, discharging` | 35 | 296 |

The text is the item's accessible name (`wm show`: a `button` named by it; the screen reader
of `accessibility-v0.4.md` reads `Battery: 87%, discharging, button`).

### 2.4 The battery menu

A click opens a menu with the power menu's chrome (`menu.bg`, 1 px `menu.border`, r6, the
window shadow, items 32 tall, text at x + 12), **240 wide**, under the top bar (top y 32) or
above the taskbar (bottom at 756), its right edge aligned with the target's:

| Row | Text | Kind |
|-----|------|------|
| 1 | `Battery: 87%` (`Battery 1: 87%`, `Battery 2: 40%` with several, one row each) | fact, `text.dim`, skipped by the arrows, a click does nothing |
| 2 | `Discharging`, `Charging`, `Plugged in, full` or `Low: plug in soon` | fact |
| | separator (9 tall) | |
| 3 | `Power Settings` | item: runs `settings open power` (`Action::OpenSettingsPane(SettingsPane::Power)`), as the network indicator opens its pane |

Height 2 + 32 x 3 + 9 = 107 (one battery). Up/Down move between the items that are not facts
(so Down from nothing selects `Power Settings`), Enter chooses, Esc, a second click or a click
outside closes it, as the power menu. No time estimate (`2 h 10 min left`): it needs a rate
history the kernel does not keep (later).

## 3. `power` on the CLI and `power_status` (D22)

- **`power status [--json]`** (`userland/coreutils/src/power.rs`, beside `poweroff`): one line
  per fact the machine reports, in this order, from `/proc/driver/power`:

  ```
  Battery: 87%, discharging
  Line power: offline
  Lid: open
  Temperature: 47.0 °C, limit 95.0 °C
  Dock: detached
  ```

  `Battery 1:` and `Battery 2:` with several; `Temperature:` lists every zone (`47.0 °C and
  52.5 °C, limit 95.0 °C`); `Line power: online`; `Lid: closed`; `Dock: attached`. A machine
  with none prints `No power devices.` and exits 0. `--json`: `{"batteries":[{"percent":87,
  "state":"discharging"}],"line_power":"offline","lid":"open","temperatures_c":[47.0],
  "critical_c":[95.0],"dock":"detached"}`, with `null` for a fact not reported and `[]` for
  none. `power` alone is `power status`. Errors: none but usage (exit 2). Needs no root.
- **`power_status`**: `The power state (D556): batteries with percent and state, line power,
  lid, temperatures in °C with each zone's critical point, dock; null for what the machine
  does not report`; twin `power status --json`; no grant (a fact, as `net_status`); `readOnlyHint`.
- The lid action is `settings set power.lid_action off` and `settings_set` (`run`); no verb
  of its own, as the first accessibility brief's keys have none.

## 4. Actions: the lid, heat, a low battery

### 4.1 The lid

`power.lid_action` applies when the kernel sees `_LID` go from open to closed:

| Value | Does |
|-------|------|
| `sleep` (default) | enters the deepest sleep state the machine has; **while only S5 exists**, the same as `off` below, and the pane says so (5.1) |
| `nothing` | nothing |
| `off` | the countdown of 4.4, `Lid closed: powering off in 30 s. Open the lid to cancel.`, `COUNTDOWN_LID_S` 30, then an orderly power-off; opening the lid cancels it |

The countdown exists so a lid closed to carry the machine, or by a cat, is not a lost
session: the person opens it and sees `Power-off cancelled.`. The default stays `sleep` so
that the day sleep lands nothing changes for anyone. On a dock with another display the lid
is still the lid (closed-lid docking is later).

### 4.2 Heat

Per zone, from the polled `_TMP`:

| Crossing | Notice | Then |
|----------|--------|------|
| hot point upwards (`_HOT`, else critical - 10) | `Temperature 85 °C: close to the limit.` (3000 ms) | once; re-armed when the zone falls 5 °C under hot |
| critical upwards (`_CRT`, else 95.0) | the countdown `Temperature 95 °C: powering off in 30 s to protect the hardware.`, `COUNTDOWN_THERMAL_S` 30 | an orderly power-off; cancelled when the zone falls 5 °C under critical (`Power-off cancelled.`) |

The firmware's own trip point may cut power sooner; 30 s is what EggOS gives a person to save
and the kernel to sync. Headless: the same text as a serial line, `power: powering off in 30
s: temperature 95 °C`, then every 10 s the seconds left.

### 4.3 A low battery (discharging only; re-armed by line power)

| Percent | Notice |
|---------|--------|
| 15 (`LOW_BATTERY_PERCENT`) | `Battery at 15%.` (the phone brief's, 3000 ms) |
| 5 (`VERY_LOW_BATTERY_PERCENT`) | `Battery at 5%: plug in now.` |
| 3 (`CRITICAL_BATTERY_PERCENT`) | the countdown `Battery at 3%: powering off in 60 s. Plug in to cancel.`, `COUNTDOWN_BATTERY_S` 60; line power online cancels it |

### 4.4 The countdown notice

A notice (6.4 item 2) with three differences: **its width is its text** (glyphs x 8 + 32, at
least 320, at most the work area's width - 32; the thermal line, 61 glyphs, is 520 wide), **it
closes on no key or click** and replaces any other popup until it ends, and **its seconds
count down**: the text is redrawn each second (`in 29 s`, `in 28 s`, ...; the rect damaged).
When it reaches 0 the kernel runs the orderly power-off (`sync`, then `poweroff`'s path, D106).
When cancelled it becomes `Power-off cancelled.` for 1500 ms. Only one countdown runs at a
time; a second reason joins the one running (the earlier deadline wins). The screen reader
announces the start, the cancel and each 10 s (`keys`, queue).

## 5. Settings > Power

### 5.1 The pane

Sidebar: `Power` **after `Sound`**, which follows `Display` (`Appearance`, `Network`,
`Display`, `Sound`, `Power`, `Keyboard`, ...): the two panes about the machine's hardware
beside Display, where macOS (`Sound`, `Battery`) and GNOME (`Sound`, `Power`) keep them; 14
rows scroll as 12 do. Search words: `battery`, `power`, `lid`, `sleep`, `temperature`,
`thermal`, `heat`, `charging`, `plugged in`, `dock`, `Power & battery` (Windows), `Energy`
(macOS Ventura's `Energy Saver`). Pane coordinates (origin window x 176, y 0; right edge 448):

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Power` |
| 2 | label | (16, 76) | `Battery` in `text` |
| 3 | label, right-aligned | ends at 448, cell top 76 | `87%, discharging` (84), `100%, full`, `40%, charging` in `text`; `Not reported` (64) in `text.dim`; one row per battery (the rows below move down 40 each) |
| 4 | label | (16, 116) | `Line power` |
| 5 | label, right-aligned | ends at 448, cell top 116 | `Plugged in` (50) / `On battery` (53) / `Not reported` |
| 6 | label | (16, 156) | `Lid` |
| 7 | label, right-aligned | ends at 448, cell top 156 | `Open` / `Closed` / `Not reported` |
| 8 | label | (16, 196) | `When the lid closes` (96) |
| 9 | dropdown | (288, 190), 160x28 | name `When the lid closes`: `Sleep`, `Do nothing`, `Power off`; the saved `power.lid_action`; disabled while no lid is reported; popup (276, 222), 184 wide, 3 rows, 98 tall |
| 10 | label | (16, 236) | `Sleep is not available yet: the machine powers off instead.` (293) in `text.dim`; shown while `Sleep` is chosen and no sleep state exists, else hidden |
| 11 | label | (16, 276) | `Temperature` |
| 12 | label, right-aligned | ends at 448, cell top 276 | `47.0 °C` (the hottest zone; `47.0 °C and 52.5 °C` with two) / `Not reported` |
| 13 | label | (16, 316) | `Dock` |
| 14 | label, right-aligned | ends at 448, cell top 316 | `Attached` / `Detached` / `Not reported` |
| 15 | label | (16, 356) | `Low battery` |
| 16 | label, right-aligned | ends at 448, cell top 356 | `Notices at 15% and 5%; powers off at 3%.` (211) in `text.dim` |
| 17 | label | (16, 396) | `Critical temperature` (99) |
| 18 | label, right-aligned | ends at 448, cell top 396 | `Powers off at 95 °C after a 30 s notice.` (202; the hottest zone's limit) in `text.dim` |
| 19 | label | (16, 436) | the status line (the Accessibility pane's 2.3 words with `the lid setting`); hidden when it has nothing to say |

`POWER_PANE_HEIGHT` 468 (436 + 16 + 16), in the scroll view. The facts (3, 5, 7, 12, 14) are
read again **once a second** while the pane shows, from `/proc/driver/power`, damaging a
label's rect only when its text changed (the Network pane's rule). The dropdown's `Chosen` is
`Request::SetPower(LidAction)`: saved, then applied through `SET_POWER_ACCESSIBILITY`'s
sibling `SET_LID_ACTION` to the kernel; a failed save puts the dropdown back and the status
line says why. 19 widgets; `CAPACITY` + 20 with the pane's container.

### 5.2 Every device class, and what an agent sees

- **Phone**: the status bar's battery icon and percent (`adaptive-shell-v0.5.md` 2.2) stand;
  the shade's `Battery` tile opens this pane (`settings open power`); the notices of 4.3 are
  the phone's too; a phone has no lid (rows 6-10 read `Not reported`, the dropdown disabled).
  The one-column reflow of `settings-forms-v0.5.md` applies.
- **Tablet, TV**: no battery on a TV, so no indicator in its strip; a thermal countdown is the
  TV's notice (`adaptive-shell-tv-v0.5.md` 3.5); the pane at scale 2.
- **Headless**: `power status`, `settings set power.lid_action`, the serial lines of 4.2.
- **`wm show`**: the bar item is a `button` named by its tooltip; the menu is the server's
  (no tree); the pane lists `label`s with their drawn text and a `combo-box` `When the lid
  closes` valued `Sleep`; `settings_get power.battery` is how an agent reads the charge.

## 6. Protocol

`Desktop::set_power(PowerState)` (the kernel to the shell); `SET_LID_ACTION { action: u8 }`
(`Manage`, root), read at boot from the settings file with the rest; `power.*` read-only keys
served from the kernel's state as `network.*` are; `countdown(text, seconds, reason)` and
`cancel_countdown(reason)` on the desktop, driven by the kernel's power thread, which owns the
deadline (the shell only draws it, so a headless machine counts down the same).

## 7. Acceptance (host tests, 1280x800, scale 1x, macOS profile unless said)

- The glyph's pixels for 0, 1, 12, 15, 16, 50, 100 %, on and off line power (the bolt's 10
  pixels, the `!`, the fill widths 0, 1, 2, 2, 2, 5, 9); the target at (1216, 0, 28, 28) and the
  volume and network targets at 1188 and 1160; in the Windows profile (1096, 760, 40, 40);
  nothing drawn and the network at its old place without a battery; the tooltip texts and
  rects of 2.3; the menu at (1004, 32, 240, 107) with its rows, Down selecting `Power
  Settings`, Enter starting `settings open power`.
- `power status` text and JSON for a full state, two batteries, two zones, and `No power
  devices.`; `power_status`'s schema and `readOnlyHint`; `settings list` carrying the six
  `power.` lines; `settings set power.lid_action nap` answering `invalid value`.
- The kernel's power thread: a lid close with `sleep` and no S3 starts the 30 s countdown, an
  open at 12 s cancels it (`Power-off cancelled.`), with `nothing` nothing; a zone at hot shows
  the hot notice once and again only after falling 5 °C under it; a zone at critical starts the
  30 s countdown, the text at 29, 28, ..., and `poweroff`'s path at 0; a drop to critical - 5
  cancels; battery 15, 5, 3 % notices, the 60 s countdown, line power cancelling, and the
  thresholds re-armed by charging; two reasons sharing one countdown with the earlier deadline.
- The countdown notice 520 wide for the thermal text, 320 for `Lid closed: ...` (49 glyphs,
  424), not closed by a key, redrawn each second.
- The pane: every widget of 5.1 at its rect; `Sleep` chosen with no S3 shows line 10; `Do
  nothing` hides it; choosing `Power off` returns `SetPower(LidAction::Off)`; the facts change
  within a second of `/proc/driver/power` changing; `Not reported` and the disabled dropdown
  without a lid; the sidebar's 14 rows with `Sound` at index 3 and `Power` at 4.

## Decided by design (the owner can overrule any of these)

The indicator sits beside the power button and shows only with a battery; the fill is nine
cells, the bolt means line power, low adds the `!` and `accent`; a click opens a facts menu
with `Power Settings`, no time estimate; `power status` is the one new command, read-only
facts in `power.*` keys and one settable `power.lid_action`; `sleep` is the default and falls
back to a 30 s countdown to off while only S5 exists; heat warns at hot and counts down 30 s
at critical with 5 °C of hysteresis; battery notices at 15 and 5, a 60 s countdown at 3; the
countdown is a notice that keys do not close and that sizes to its text; the Power pane
follows Sound after Display.

## Later

Sleep states (S3, s2idle) and resume; a time estimate; closed-lid docking; a hot-plugged
battery; a charge limit; a power mode (performance, balanced, saver) and display dimming on
battery; the taskbar's `Battery saver` quick toggle; a per-app power report.

## Decisions for the engineer

1. Indicator targets: top bar `(power_button().x - 28, 0, 28, 28)` = (1216, 0, 28, 28), glyph
   at + (6, 6); taskbar `(clock_rect().x - 8 - 40, 760, 40, 40)` = (1096, 760, 40, 40), glyph
   at + (12, 12); the run's order right to left: power, battery, volume, network, keyboard,
   tray, Agent chip, sticky chips.
2. Glyph: body rows 4..11 x 0..12, nub x 13..14 rows 6..9 (row 6 x 13..14, rows 7..8 x 14,
   row 9 x 13..14), fill cells x 2..10 rows 6..9, `filled = (percent x 9 + 99) / 100`; bolt
   rows 0..3 (x 8..9, 7..8, 6..9, 7) in `accent` while line power is online; low at ≤ 15 %
   (`LOW_BATTERY_PERCENT`): fill `accent`, `!` at (6, 6), (6, 7), (6, 9).
3. Tooltip texts of 2.3, width glyphs x 8 + 16; the menu 240 wide, rows 32, separator 9,
   facts in `text.dim` skipped by the arrows, `Power Settings` runs `settings open power`.
4. `power status [--json]` lines and JSON of section 3; `No power devices.`; `power_status`
   twin `power status --json`, no grant, `readOnlyHint`.
5. Keys: `power.battery`, `power.line`, `power.lid`, `power.temperature`, `power.dock`
   read-only; `power.lid_action` `sleep` (default) | `nothing` | `off`.
6. Kernel: `_CRT` and `_HOT` per zone, `CRITICAL_FALLBACK_C` 95.0, hot = critical - 10 when
   absent; `THERMAL_POLL_S` 10; `BATTERY_POLL_S` 30; `/proc/driver/power` thermal lines gain
   `limit <c>`; state word `full` = line power online, not charging, 100 %.
7. Countdowns: `COUNTDOWN_LID_S` 30, `COUNTDOWN_THERMAL_S` 30, `COUNTDOWN_BATTERY_S` 60;
   thresholds `LOW_BATTERY_PERCENT` 15, `VERY_LOW_BATTERY_PERCENT` 5,
   `CRITICAL_BATTERY_PERCENT` 3; hysteresis `THERMAL_HYSTERESIS_C` 5; one countdown at a
   time, the earliest deadline wins; `Power-off cancelled.` 1500 ms.
8. The countdown notice: width `max(320, glyphs x 8 + 32)` capped at the work area - 32,
   closes on no input, text redrawn each second, replaces other popups.
9. The pane: `POWER_PANE_HEIGHT` 468, the 19 widgets of 5.1, facts re-read once a second,
   dropdown popup (276, 222) 184x98; sidebar order Appearance, Network, Display, Sound, Power,
   Keyboard, Mouse, Accessibility, Storage, Date & time, Agents, Windows, Components, About.
10. Protocol: `Desktop::set_power`, `SET_LID_ACTION`, `countdown`/`cancel_countdown` on the
    desktop driven by the kernel's power thread; serial lines `power: ...` for headless.
