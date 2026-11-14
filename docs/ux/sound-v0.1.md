# EggOS v0.1 sound: the volume indicator and popup, the volume keys, Settings > Sound, `sound` and its MCP tools: design brief

Design by Claude Fable (D16) for the front-ends of sound, ahead of the virtio-sound and Intel
HD Audio drivers and the sound server (the mixer) that will back them: the GUI, CLI and MCP
faces D22 asks for, designed once so the backend lands into them. Everything in
`desktop-v0.3-gui.md` 6.1 and 6.4 item 2, `network-v0.3.md` section 4 (the indicator pattern)
and `power-v0.1.md` section 2.1 (the indicator run's order) stands. Numbers are px at 1x at
1280x800 and double at 2x; bar tooltips Egg Mono (glyphs x 8 + 16), chips, menus and panes
Egg Sans. "Later" marks what this slice does not build.

## 0. What a person gets, and how that is checked

- **A volume indicator** in every profile's bar, a speaker with zero to three waves, crossed
  when muted; a click opens a popup with a slider, a mute button and `Sound Settings`.
- **Volume keys** (the keyboard's Volume Up, Volume Down and Mute, every profile) change the
  volume by 5 % and show a 1.5 s on-screen bar.
- **Settings > Sound**: output and input device pickers, volume, mute, a test tone, input
  volume, input mute and a live input level.
- **`sound status|volume|mute|devices|output|input|test`** and the MCP tools `sound_status`,
  `sound_devices` and `sound_test`; the volume, mute and devices are `sound.*` settings keys,
  so `settings set` and `settings_set` set them too.
- Host tests pin the glyphs, the popup, the keys, the pane and the words (section 7).

## 1. The model

- **The sound server** (`userland/soundd`, started by init only when a sound device exists:
  D18) owns the mixer state: `output` and `input` device, `volume` and `mute` for each, and
  tells the desktop on each change (`Desktop::set_sound(SoundState)`), which damages the
  glyph's rect and redraws a shown popup or pane. The settings keys below are applied through
  it (`settings set sound.volume 40` asks the server, which saves and applies; the pane and
  the keys do the same) so the file, the pane, the CLI and the next boot agree.
- **Devices** are named `<driver><index>-<direction>`: `hda0-out`, `hda0-in`, `virtio0-out`,
  `virtio0-in`; their display names come from the driver (`Built-in speakers`, `Headphones`
  when HDA's jack sense says one is plugged, `Built-in microphone`, `Line in`, `virtio sound`
  in QEMU). `auto` means the server's choice: headphones when plugged, else the first output.
- **Keys** (D22; `docs/settings.md` gains them):

  | Key | Value | Applies |
  |-----|-------|---------|
  | `sound.volume` | `0` to `100`, `50` the default | at once |
  | `sound.mute` | `true` / `false` (default) | at once |
  | `sound.output` | `auto` (default) or a device id | at once; an id that is not present answers `no_such_device` |
  | `sound.input` | `auto` (default) or a device id | at once |
  | `sound.input_volume` | `0` to `100`, `50` | at once |
  | `sound.input_mute` | `true` / `false` (default) | at once |
  | `sound.devices` | read-only: the ids present, comma-separated, or `none` | |

- **Cost**: without a sound device nothing runs, nothing draws and the keys read `none`
  (D18, D24). The indicator is drawn only while an output device exists.

## 2. The volume indicator

### 2.1 Place

In the indicator run between the battery and the network (`power-v0.1.md` 2.1): top bar
**(1188, 0, 28, 28)** at 1280 with a battery, (1216, 0, 28, 28) without one, the glyph at the
target's (6, 6); taskbar **(1056, 760, 40, 40)** with a battery, (1096, 760, 40, 40) without,
the glyph at (12, 12). Hover and pressed as the network's; `bar.pressed` while the popup is
open; pointer `Arrow`. Shown while an output device exists (known at boot; a hot-plugged USB
device is later).

### 2.2 The glyph (16x16; `X` = `text`, `.` = the bar)

A speaker: a 4x6 box at x 1..4 rows 5..10 and a cone to x 7 (rows 2..13), then up to three
1 px arcs, drawn from rectangles. **Level 3** (67 to 100 %):

```
................
............X...
.......X.....X..
......XX..X..X..
.....XXX...X.X..
XXXXXXXXX..X.X..
XXXXXXXX.X.X.X..
XXXXXXXX.X.X.X..
XXXXXXXX.X.X.X..
XXXXXXXX.X.X.X..
XXXXXXXXX..X.X..
.....XXX...X.X..
......XX..X..X..
.......X.....X..
............X...
................
```

As rectangles `(x, y, w, h)`: the box `(0, 5, 4, 6)`; the cone `(4, 5, 1, 6)`, `(5, 4, 1, 8)`,
`(6, 3, 1, 10)`, `(7, 2, 1, 12)` (a solid speaker, as Windows's); wave 1 `(9, 6, 1, 4)` with
its end pixels (8, 5) and (8, 10); wave 2 `(11, 4, 1, 8)` with (10, 3) and (10, 12); wave 3
`(13, 2, 1, 12)` with (12, 1) and (12, 14). The arcs are 4, 8 and 12 tall, centred on rows
7..8, each end bent one column back.

- **Levels**: volume 0 and not muted: the speaker alone; 1..33 wave 1; 34..66 waves 1 and 2;
  67..100 all three (`VOLUME_WAVE_1` 33, `VOLUME_WAVE_2` 66).
- **Muted**: the speaker alone and an `x` in `text`: the two diagonals of a 5x5 square at
  x 9..13, rows 5..9. Shape, not colour, tells it.
- Contrast as the battery's; at scale 2 pixel-doubled.

### 2.3 Tooltip and accessible name

After 600 ms, the panel tooltip: `Volume: 50%` (11 glyphs, 104 wide), `Volume: 50%, muted`
(18, 160), `Volume: 100%` (12, 112); with the output named when it is not the only one:
`Volume: 50%, Headphones` (23, 200). The text is the item's accessible name.

### 2.4 The popup

A click opens a popup (`menu.bg`, 1 px `menu.border`, r6, the window shadow) **248x84** under
the top bar (top y 32) or above the taskbar (bottom at 756), its right edge aligned with the
target's:

| Part | Rect (popup coordinates) | Draws |
|------|--------------------------|-------|
| mute button | (8, 16, 24, 16) hit target 8..31 x 8..39 | the glyph of 2.2 at (12, 16) (muted when muted); a click or `M` toggles `sound.mute` |
| slider | (40, 10, 160, 28) | the toolkit slider's look drawn by the shell: track 4 tall `progress.track` at rows 22..25 from x 48 to 192 (144 px of travel), the done part `accent`, the thumb 16x16 `text` with a 1 px `menu.border`, centred at `48 + round(volume x 144 / 100)`; a press on the track jumps; a drag sets `sound.volume` once per new value |
| readout | right-aligned ending at x 236, cell top 16 | `50%` in `text` (`Muted` in `text.dim` while muted) |
| separator | row 48, x 8..239 | 1 px `menu.border` |
| `Sound Settings` | (0, 50, 248, 32), text at x 12 | an item: runs `settings open sound` |

Keys while open: Left/Right 1 %, Shift+Left/Right 5 %, PageUp/PageDown 10, Home 0, End 100,
`M` mute, Down selects `Sound Settings` (`menu.hover`) and Enter opens it, Esc, a second click
or a click outside closes. The wheel over the slider or the indicator: one notch 5 %.

## 3. The volume keys and the on-screen bar

- The server takes `KEY_VOLUMEUP` (115), `KEY_VOLUMEDOWN` (114) and `KEY_MUTE` (113) in
  every profile before any client sees them (`Shortcut::Volume(VolumeCommand::Up | Down |
  Mute)`): every keyboard's media keys and a phone's buttons send these. Up and Down change
  `sound.volume` by **5** (`VOLUME_STEP`), by 1 with Shift held, clamped 0..100; Up while
  muted unmutes first (Windows's and macOS's rule); Mute toggles `sound.mute`. Key repeat
  applies (hold to sweep). No profile-specific chords: F10-F12 are the menu bar's and the
  apps', and Apple keyboards already send 113-115.
- **The on-screen bar**: the notice of 6.4 item 2 (320x40, centred at the work area's top +
  16), replaced on every press, closing after **1500 ms**: the glyph at (12, 12); a track
  (40, 18, 216, 4) `progress.track` with the done part `accent` from x 40 to `40 + round(volume
  x 216 / 100)`; the readout right-aligned ending at x 308, cell top 12, `50%` in `text`;
  muted: the crossed glyph, no done part, `Muted` (5 glyphs, 40 px). The screen reader says
  `Volume 55 percent` or `Muted` (`keys`, queue).
- With no output device the keys do nothing and show nothing.

## 4. Settings > Sound

### 4.1 The pane

Sidebar: `Sound` after `Display` (index 3; `power-v0.1.md` 5.1 has the order). Search words:
`sound`, `audio`, `volume`, `mute`, `speaker`, `headphones`, `microphone`, `input`, `output`,
`test tone`. Pane coordinates (origin window x 176, y 0; right edge 448):

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Sound` |
| 2 | label | (16, 76) | `Output` in `text` |
| 3 | dropdown | (288, 70), 160x28 | name `Output`: `Automatic`, then each output device's display name; the saved `sound.output`; popup (276, 102), 184 wide, up to 6 rows |
| 4 | label | (16, 116) | `Volume` |
| 5 | slider | (232, 110), 216x28 | name `Volume`, 0 to 100 by 1, unit `%`; the thumb at `round(value x 144 / 100)` on the track; `sound.volume` |
| 6 | checkbox | (16, 150), 48x28 | `Mute`; `sound.mute` |
| 7 | push button | (16, 190), 102x28 | `Play test tone`: a 440 Hz sine for 1 s (`TEST_TONE_HZ` 440, `TEST_TONE_MS` 1000) on the chosen output at the current volume (muted: unmuted for the tone, not saved); disabled while it plays |
| 8 | label, right-aligned | ends at 448, cell top 196 | `Plays a 1 s tone on the output above.` (191) in `text.dim` |
| 9 | label | (16, 236) | `Input` |
| 10 | dropdown | (288, 230), 160x28 | name `Input`: `Automatic`, then the input devices; `sound.input`; popup (276, 262) |
| 11 | label | (16, 276) | `Input volume` (64) |
| 12 | slider | (232, 270), 216x28 | name `Input volume`, 0 to 100 by 1, `%`; `sound.input_volume` |
| 13 | checkbox | (16, 310), 109x28 | `Mute microphone` (85 + 24); `sound.input_mute` |
| 14 | label | (16, 356) | `Level` |
| 15 | progress | (232, 360), 216x8 | name `Level`: the input's peak over the last 100 ms, 0 to 100, read from the server **10 times a second** while the pane shows and the input is not muted (0 while muted); `progress.track` with the done part `accent` |
| 16 | label | (16, 396) | the status line: `No sound device.` (85) in `text.dim` with none (rows 2-15 then disabled), else the Accessibility pane's save and no-disk words with `the sound settings`; hidden otherwise |

`SOUND_PANE_HEIGHT` 428. Each control's change is one `Request::SetSound(SoundChange)`
(`Volume(u8)`, `Mute(bool)`, `Output(id)`, `Input(id)`, `InputVolume(u8)`, `InputMute(bool)`,
`Test`), saved then applied through the server; the controls follow the server's state when
the pane is shown and on each `set_sound` (a volume key moves the slider while the pane is
open). A dropdown's device list is rebuilt when the pane is shown. Tab order: `Output`,
`Volume`, `Mute`, `Play test tone`, `Input`, `Input volume`, `Mute microphone`, wrap.
16 widgets; `CAPACITY` + 17 with the container.

### 4.2 Every device class, and what an agent sees

- **Phone**: the hardware buttons are 115 and 114; the bar of section 3 shows under the status
  bar (y 60); the status bar shows the crossed speaker (16x16, in the run left of the battery)
  **only while muted**, as Android does; the shade gains a `Sound` tile (`Sound`, `50%` or
  `Muted`; a tap toggles mute, as the brief's tiles toggle; the asks of `adaptive-shell-v0.5.md`
  2.7 reserved it). The pane reflows to one column.
- **TV**: the remote's Volume and Mute keys; the bar shows at the bottom centre, 16 px above
  the screen's bottom (the TV's notice place), 640x80 at scale 2; no indicator in the strip.
  The pane at scale 2 by the remote's arrows.
- **Headless**: `sound` on the CLI, when a device exists; nothing else.
- **`wm show`**: the bar item a `button` named by its tooltip; the popup is the server's (no
  tree; its slider is `settings_set sound.volume`); the pane's `combo-box`es `Output` and
  `Input`, `slider`s `Volume` and `Input volume` (`50 0..100 by 1`), `checkbox`es `Mute` and
  `Mute microphone`, `button` `Play test tone`, `progress` `Level`.

## 5. `sound` on the CLI and the MCP tools (D22)

- `sound status [--json]`:

  ```
  Output: Built-in speakers (hda0-out), volume 50%
  Input: Built-in microphone (hda0-in), volume 50%, muted
  ```

  `No sound device.` (exit 0) with none; `--json` `{"output":{"id":"hda0-out","name":
  "Built-in speakers","volume":50,"mute":false},"input":{...}}`, `null` for a missing side.
  `sound` alone is `sound status`.
- `sound volume <0-100 | +n | -n>`: sets or steps `sound.volume`, prints `Volume 55%.`;
  `sound mute on|off|toggle`: `Muted.` / `Unmuted.`; `sound output <id>|auto` and `sound
  input <id>|auto`: `Output: Headphones (hda0-out).`; `sound devices [--json]`: `ID  KIND
  NAME` lines (`hda0-out  output  Built-in speakers`), JSON one record a line; `sound test
  [--seconds <1-10>]`: the tone, `Played a 1 s tone on Built-in speakers.`.
- Errors: `no_sound_device` (category state), `no_such_device` (fix: `sound devices`),
  `invalid_value`; exit 0, 1, 2. Needs no root (the server's socket is world-usable: volume
  is not a secret, and a phone's buttons are the same path).
- **MCP** (`docs/mcp.md`'s table):

  | Tool | Description | Twin | Grant |
  |------|-------------|------|-------|
  | `sound_status` | The output and input devices in use with their volume and mute | `sound status --json` | |
  | `sound_devices` | Every sound device: `id`, `kind` (`output` or `input`), `name` | `sound devices --json` | |
  | `sound_test` | Plays a 1 s test tone on the output (`seconds` 1 to 10) | `sound test --seconds <n>` | `run` |

  Volume, mute and the devices are `settings_set` (`run`) on the `sound.*` keys: no
  `sound_set` tool. `sound_test` makes noise in the person's room, so it is `run`, lights the
  Agent chip and is logged; the two readers are `readOnlyHint` with no grant (facts).

## 6. Protocol

`Desktop::set_sound(SoundState { output, input, volume, mute, input_volume, input_mute })`;
the server's socket `/run/soundd` with `Get`, `Set(SoundChange)`, `Devices`, `Level` and
`Test(ms)` messages (the CLI, the pane and the shell's popup all speak it; the shell's
`Shortcut::Volume` sends `Set`); the server reads the six keys at start and writes them
through the settings file's writer, so `settings set` and the server never disagree.

## 7. Acceptance (host tests, 1280x800, scale 1x, macOS profile unless said)

- The glyph's pixels at 0, 1, 33, 34, 66, 67, 100 % and muted (the three arcs' rectangles, the
  `x`'s 9 pixels); the target at (1188, 0, 28, 28) with a battery and (1216, ...) without;
  Windows (1056, 760, 40, 40); nothing without an output device; the tooltips of 2.3.
- The popup at (968, 32, 248, 84) under the target: a press at track x 48 + 72 sets 50;
  Right sets 51; Shift+Right 56; `M` mutes and the readout reads `Muted`; Down then Enter
  starts `settings open sound`; Esc closes; the wheel one notch sets 55.
- Keys: 115 from 50 sets 55 and shows the bar with the done part 118 px wide and `55%`; 114
  with Shift sets 49; 113 mutes and shows `Muted`; 115 while muted unmutes and sets 55; a
  second press replaces the bar; it closes at 1500 ms; no device: nothing.
- The pane: every widget of 4.1 at its rect; `Output` lists `Automatic`, `Built-in speakers`,
  `Headphones` from a fake server; choosing returns `SetSound(Output("hda0-out"))`; the slider
  at 50 has its thumb centred at track x + 72; `Play test tone` returns `SetSound(Test)` and is
  disabled for 1000 ms; `Level` reads the fake's 40 within 100 ms and 0 while `Mute
  microphone` is checked; `No sound device.` disables rows 2-15; a volume key while the pane
  shows moves the slider.
- CLI: every form's text and JSON against the fake; `sound volume 101` `invalid value`;
  `sound output usb9-out` `no_such_device`; the MCP schemas and grants; `sound_test` refused
  without `run`; `settings set sound.volume 40` reaching the fake server.

## Decided by design (the owner can overrule any of these)

The indicator sits between the battery and the network; three waves by thirds and a crossed
speaker for mute; the popup holds the slider, mute and `Sound Settings` only (device picking
is the pane's); the keys step 5 % (1 with Shift) and Up unmutes; one on-screen bar in the
notice's place for 1500 ms; the state lives in a sound server and the six `sound.*` keys go
through it; `sound` is the command, `sound_test` the one `run` tool, readers need no grant;
`Sound` follows Display in the sidebar; the test tone is 440 Hz for 1 s.

## Later

Hot-plugged USB and Bluetooth devices; per-app volumes; a balance and a mono toggle; an alert
volume; sound schemes and the beep (which `visual alerts` in accessibility waits for); the
phone's silent and vibrate modes; the TV's HDMI ARC and CEC volume; headphone safety limits.

## Decisions for the engineer

1. Targets: top bar `(battery_target().x - 28, 0, 28, 28)` or `(power_button().x - 28, ...)`
   without a battery, glyph at + (6, 6); taskbar `(battery_item().x - 40, 760, 40, 40)`, glyph
   at + (12, 12).
2. Glyph: box `(0, 5, 4, 6)`; cone `(4, 5, 1, 6)`, `(5, 4, 1, 8)`, `(6, 3, 1, 10)`,
   `(7, 2, 1, 12)`; wave 1 `(9, 6, 1, 4)` + (8, 5), (8, 10); wave 2 `(11, 4, 1, 8)` + (10, 3),
   (10, 12); wave 3 `(13, 2, 1, 12)` + (12, 1), (12, 14); `VOLUME_WAVE_1` 33, `VOLUME_WAVE_2`
   66; muted `x` the diagonals of (9, 5, 5, 5).
3. Tooltips of 2.3; popup 248x84 with the parts of 2.4 (track x 48..192, thumb at `48 +
   round(v x 144 / 100)`, readout ending at 236, separator row 48, `Sound Settings` at y 50).
4. Keys 115, 114, 113 in every profile; `VOLUME_STEP` 5, Shift 1, PageUp/PageDown 10 in the
   popup; Up unmutes; the bar 320x40 at the notice's place, track (40, 18, 216, 4), readout
   ending at 308, `VOLUME_BAR_MS` 1500.
5. Settings keys and defaults: `sound.volume` 50, `sound.mute` false, `sound.output` auto,
   `sound.input` auto, `sound.input_volume` 50, `sound.input_mute` false, `sound.devices`
   read-only; device ids `<driver><index>-<direction>`.
6. The pane: `SOUND_PANE_HEIGHT` 428, the 16 widgets of 4.1, dropdown popups at (276, 102)
   and (276, 262), the level read at 10 Hz, `TEST_TONE_HZ` 440, `TEST_TONE_MS` 1000.
7. CLI forms, outputs and errors of section 5; tools `sound_status` and `sound_devices` (no
   grant, `readOnlyHint`), `sound_test` (`run`, `seconds` 1..10).
8. The server `soundd` with `/run/soundd` and the messages of section 6, started only with a
   device; `Desktop::set_sound`; `Shortcut::Volume(VolumeCommand)`.
9. Phone: the crossed glyph in the status bar only while muted; the `Sound` tile in the shade;
   TV: the bar 640x80 at the bottom centre 16 px up.
