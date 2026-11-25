# EggOS v0.2 sound: per-app volumes, the alert sound, the output switch in the popup, `sound play`: design brief

Design by Claude Fable (D16) for the sound front-ends' second slice, now that the stack is
ours (virtio-sound and Intel HD Audio drivers, the mixer in `soundd`, `NSSound`,
`PlaySound`/`waveOut` and SDL audio on top). It extends **`sound-v0.1.md`, which stands
whole**: the indicator's place and glyph (2.1, 2.2: the speaker with zero to three waves by
thirds, crossed when muted), its tooltip, the popup's slider and mute, the volume keys and the
1.5 s on-screen bar (section 3), the Settings > Sound pane's 16 widgets (4.1), the `sound`
command and the tools (section 5), the protocol (6). This brief adds what v0.1's "Later"
listed and the stack now has: **per-app volumes** from the mixer, an **alert sound** with its
own volume, the **output switch in the popup**, the **wheel over the indicator** confirmed,
`sound play <file>` and `sound alert`, and the phone and TV forms of the new parts. Tokens
are `macos-gui-v0.3.md` section 20's (light / dark where quoted); numbers are px at 1x at
1280x800 and double at 2x; the only motion is the level meter following the input.

## 0. What a person gets, and how that is checked

- Settings > Sound shows each app that is playing, with its own slider and mute.
- An alert sound (`Chime`, `Pop`, `Glass` or `None`) with its own volume, previewed on
  choice, used by every beep an app or EggOS makes.
- The bar's popup switches the output when there is more than one; a wheel over the
  indicator changes the volume by 5 %.
- `sound apps`, `sound app <id> volume|mute`, `sound alert`, `sound play <file>`; the tools
  `sound_apps` and `sound_play`.
- Host tests pin the rows, the words and the tools (section 6).

## 1. The model, extended

- **Streams.** Every open playback stream in the mixer belongs to an **app**: the program's
  component or app id when it has one (`files`, `windows-apps:notepad`, `mac-apps:
  com.example.Player`, `android-apps:org.example.music`), else its executable's name
  (`eggsh`); display name the app's title (the catalogue's, D148), else the executable's. The
  server keeps per app `volume` (0..100, default 100: the app's own level under the master)
  and `mute`, applied as `master x app / 100` with the master's mute, remembered in
  `/var/lib/eggos/sound-apps` across runs (at most 64 apps, the oldest dropped), and lists the
  apps with an open stream (`Apps` message), each with its id, name, volume, mute and
  `playing` (bytes flowed in the last second).
- **Keys** (D22): `sound.app.<id>.volume` and `sound.app.<id>.mute`, dynamic keys the server
  answers for the apps it remembers (`settings list` shows those with an open stream;
  `settings set` on one that is not open answers `no_such_app`, fix: `sound apps`);
  `sound.alert` (`chime` default, `pop`, `glass`, `none`); `sound.alert_volume` (0..100, 70).
- **The alert** is one synthesised sound per name, no files shipped (D24): `Chime` two sine
  notes, 880 Hz then 1320 Hz, 120 ms each with a 20 ms fade; `Pop` a 60 ms sine sweep from
  200 to 80 Hz; `Glass` 1760 Hz for 200 ms decaying to silence; `None` nothing. It plays at
  `alert_volume x master / 100` on the output, unmuted only by `sound.mute` (a muted machine
  beeps not at all). Callers: the toolkit's `beep()` (the shell's refusals), `NSBeep`,
  Win32's `MessageBeep` and `Beep`, Android's default notification sound, SDL none. With
  `accessibility.visual_alerts` the flash of `first-run-accessibility-v0.3.md` 2.5 plays with
  it, and alone with `none`.

## 2. The popup, extended (`sound-v0.1.md` 2.4)

With **more than one output device**, an **output row** is inserted between the slider row
and the separator: the popup becomes **248x116**, the row (0, 48, 248, 32), the separator at
row 80, `Sound Settings` at (0, 82, 248, 32).

| Part | Rect (popup coordinates) | Draws |
|------|--------------------------|-------|
| output row | (0, 48, 248, 32), text at x 12, cell top +8 | `Output: Headphones` in `text`, the device's display name ellipsised at x 204; the dropdown's chevron (`toolkit-v0.4.md` 3.2, 10x6) at (214, 61) in `text`; hover `menu.hover`; `Down` from the slider or a click opens the **device list** |
| device list | a second popup at the row's bottom edge, 248 wide, `2 + 32 x devices` tall, `menu.bg`, 1 px `menu.border`, r6 | one row per output: `Automatic` first, then each display name; the one in use has the tick of `toolkit-v0.6.md` 2.1 at (12, +12); hover `menu.hover`; a click or Enter chooses (`sound.output`), the list closes, the row's text changes in the same frame; Esc closes the list only |

One output: the popup is v0.1's 248x84, no row. Keys while open: v0.1's, plus Down from the
slider selecting the output row, Down again `Sound Settings`, Enter on the row opening the
list, Up/Down in it, Enter choosing, Esc closing it. **The wheel over the indicator** (the
bar item, popup open or not) is one notch 5 % (v0.1 said so for the slider and the
indicator; confirmed), a notch over the output row nothing. The popup does not show per-app
rows: the pane is the mixer (Windows' separate mixer and macOS's one slider agree here).

## 3. Settings > Sound, extended (`sound-v0.1.md` 4.1)

Widgets 1 to 16 stay at their places. Two sections are added **between `Play test tone`
(row at 190) and `Input` (236)**: the pane grows, and `Input` and everything after it move
down by `80 + 40 x apps` (the alert block, then the app rows). Pane coordinates; `A` is the
number of apps shown (0 to 8, the first eight by name; more: a line `and 3 more in sound
apps` in `text.dim`).

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 17 | label | (16, 236) | `Alert sound` in `text` |
| 18 | dropdown | (288, 230), 160x28 | name `Alert sound`: `Chime`, `Pop`, `Glass`, `None`; `sound.alert`; choosing plays the chosen alert once (a preview, macOS's rule) |
| 19 | label | (16, 276) | `Alert volume` |
| 20 | slider | (232, 270), 216x28 | name `Alert volume`, 0..100 by 1, `%`; `sound.alert_volume`; releasing the thumb plays the alert once at the new volume |
| 21 | label | (16, 316) | `Apps` in `text`; `No app is playing.` in `text.dim` when `A` is 0 (then rows 22.. hidden and the block is 40 tall) |
| 22 + 3 i | label, 168 wide | (16, 356 + 40 i) | app `i`'s display name in `text`, ellipsised |
| 23 + 3 i | mute button, 24x24 | (196, 352 + 40 i) | the 16x16 speaker glyph of v0.1 2.2 at the level of the app's volume, crossed when the app is muted, in `text` (`text.dim` while the master is muted); name `Mute <app>`; a click or Space toggles `sound.app.<id>.mute`; hover `button.hover` fill r4, pressed `button.pressed` |
| 24 + 3 i | slider | (232, 350 + 40 i), 216x28 | name `<app> volume`, 0..100 by 1, `%`; `sound.app.<id>.volume` |

`Input` (v0.1's widget 9) moves to `356 + 40 A` when `A` > 0 (396 with `A` 0), the rest with
it; `SOUND_PANE_HEIGHT` becomes `508 + 40 x max(A, 1)` (588 with one app). The app rows are
rebuilt from the server's `Apps` answer when the pane is shown and on each `set_sound` (a
stream opening or closing while the pane shows adds or drops its row in the same frame, the
rows below moving; the focus stays on the same app's control by id, or goes to `Apps`'s
next control when its app left). An app row's slider follows the server when the app's
volume is set from elsewhere (`sound app`). Tab order: `Output`, `Volume`, `Mute`, `Play
test tone`, `Alert sound`, `Alert volume`, then each app's mute and slider, `Input`, ...
Each new control's change is one `Request::SetSound(SoundChange)` more: `Alert(name)`,
`AlertVolume(u8)`, `AppVolume(id, u8)`, `AppMute(id, bool)`. `WIDGETS` is 21 + 24;
`CAPACITY` + 29.

**Why a glyph button, not a checkbox**: a `Mute` checkbox per row (109 wide) would leave 72 px
for the name; the glyph is the indicator people already read, and the row's name says whose.

## 4. `sound` and the tools, extended (`sound-v0.1.md` section 5)

- `sound apps [--json]`: `ID  VOLUME  MUTE  PLAYING  NAME` lines
  (`files  100%  no  yes  Files`); `No app is playing.` (exit 0) with none; JSON one record a
  line (`{"id":"files","name":"Files","volume":100,"mute":false,"playing":true}`).
- `sound app <id> volume <0-100 | +n | -n>`: `Files: volume 40%.`; `sound app <id> mute
  on|off|toggle`: `Files: muted.` / `Files: unmuted.`; error `no_such_app` (fix: `sound apps`).
- `sound alert [<chime|pop|glass|none>] [--volume <0-100>]`: with no argument plays the
  current alert, `Played the alert (Chime).`; with one, sets `sound.alert` (and the volume)
  and plays it, `Alert sound: Pop.`; `sound alert none` `Alert sound: none.` and plays
  nothing.
- `sound play <file> [--volume <0-100>] [--no-wait]`: plays a WAV (PCM 8 or 16 bit, 1 or 2
  channels, any rate; the server resamples) on the output at the master volume (or the given
  one for this play), prints `Playing notes.wav (3.2 s)...` then `Played notes.wav.` when it
  ends (Ctrl+C stops it: `Stopped.`); `--no-wait` prints `Playing notes.wav (3.2 s).` and
  returns. Errors: `no_such_file`, `not_a_wav` (fix: `a PCM WAV file`), `no_sound_device`.
  Other formats (MP3, OGG, FLAC): later, through the decoders the media components bring.
- **MCP** (`docs/mcp.md`'s table):

  | Tool | Description | Twin | Grant and confirmation |
  |------|-------------|------|------------------------|
  | `sound_apps` | Every app playing sound: `id`, `name`, `volume`, `mute`, `playing` | `sound apps --json` | none (`readOnlyHint`) |
  | `sound_play` | Plays a WAV file on the output (`path`, optional `volume` 0..100, `seconds` at most 60: longer files are cut) | `sound play <file> --volume <n>` | `run`: it makes noise in the room, lights the Agent chip and is logged, as `sound_test` (D21); no confirmation prompt beyond the grant, because a sound stops when the agent is revoked and destroys nothing |

  App volumes, mutes and the alert are `settings_set` (`run`) on the new keys, as v0.1's
  rule: no `sound_set`. `settings_set sound.mute true` by an agent mutes the room, which is
  not destructive; `sound.app.<id>.mute` the same. `sound status --json` gains `"alert":
  {"name":"chime","volume":70}`.

## 5. Every device class (D23)

- **Phone**: the pane reflows (`settings-forms-v0.5.md` section 2): the alert dropdown and
  slider R2 (full width, 48 tall under their labels); each app a block: the name at cell top
  `r` with the mute glyph button 48x48 at its right (`16 + C - 48`, `r - 16`), the slider
  `(16, r + 24, C, 48)` (R2), next app at `r + 80`. The popup does not exist on the phone
  (no bar): the shade's `Sound` tile (v0.1 4.2) opens Settings > Sound on a long press. The
  alert plays through the phone's speaker at `alert_volume`; the ringer and silent modes are
  later.
- **Tablet**: the desktop pane with 40 px targets; the bar's popup as the desktop's.
- **TV** (scale 2): the pane by the remote: the mute glyph button and the slider are two
  stops per app row, Left/Right on a focused slider 1 %, OK on the glyph toggles; the alert
  plays on the TV's output; no popup (no bar), the on-screen bar of v0.1 section 3 at the
  bottom centre.
- **Wearable, headless**: `sound` on the CLI only.
- **`wm show`**: the pane's `combo-box` `Alert sound`, `slider`s `Alert volume` and `<app>
  volume`, `button`s `Mute <app>` with value `muted`/`unmuted`; the popup's output row a
  `menu item` `Output: Headphones` (`expandable`) with the device list's `menu item`s (the
  one in use `checked`), as the shell's menus are listed.

## 6. Acceptance (host tests, 1280x800, scale 1x, macOS profile unless said; a fake server with outputs `Built-in speakers` and `Headphones`, apps `Files` (100 %, playing) and `Notepad` (`windows-apps:notepad`, 60 %, muted))

- The popup at (968, 32, 248, 116): the output row at y 48 reading `Output: Built-in
  speakers`, the chevron at (214, 61); a click opens the device list at (968, 112, 248, 98)
  with `Automatic`, `Built-in speakers` (ticked), `Headphones`; a click on `Headphones` sets
  `sound.output` to the fake's id for it (`hda0-out`), closes the list and the row reads
  `Output: Headphones`; Down, Down, Enter from the slider does the same; Esc in the list
  closes it alone. With one output the popup is 248x84 and has no row. A wheel notch over
  (1200, 14) sets 55 %.
- The pane: `Alert sound` at (16, 236) with the dropdown at (288, 230) reading `Chime`;
  choosing `Pop` returns `SetSound(Alert("pop"))` and the fake records one play of `pop`;
  `Alert volume` at (232, 270) with the thumb at 70; releasing it at 40 returns
  `AlertVolume(40)` and one play; `Apps` at (16, 316); `Files` at (16, 356) with its glyph
  button at (196, 352) (three waves) and slider at (232, 350) at 100; `Notepad` at (16, 396)
  with the crossed glyph and the slider at 60; `Input` at (16, 436); the pane 588 tall... with
  two apps `508 + 80` = 588. A click on `Notepad`'s glyph returns `AppMute("windows-apps:
  notepad", false)` and the glyph shows two waves; dragging `Files`'s slider to 40 returns
  `AppVolume("files", 40)`; the fake closing `Files`'s stream removes its row and moves
  `Notepad` up 40 in the same frame; no apps: `No app is playing.` and `Input` at 396.
- `sound apps` prints the two lines; `sound app files volume 40` prints `Files: volume 40%.`
  and reaches the fake; `sound app nope mute on` `no_such_app`; `sound alert glass` prints
  `Alert sound: Glass.` and plays once; `sound play notes.wav` with a 16-bit stereo WAV of
  3.2 s prints the two lines and the fake receives 3.2 s of samples; `sound play x.txt`
  `not_a_wav`; `settings set sound.app.files.volume 40` reaches the fake and
  `settings list` shows the two apps' keys; the MCP schemas and grants; `sound_play` refused
  without `run`, cut at 60 s.
- Phone 390x752: the app block's rects; TV 960x540: the two stops per row and OK toggling.

## Decided by design (the owner can overrule any of these)

1. Per-app volumes live in the pane, not the popup; each row is a name, a glyph mute button
   and a slider; eight rows at most, the rest by the CLI.
2. An app's volume is a fraction of the master, remembered across runs for 64 apps.
3. The alert is synthesised (three sounds and `None`), previewed on choice, with its own
   volume; every beep of every layer goes through it.
4. The popup gains the output switch only with more than one output; the wheel over the
   indicator is 5 % a notch.
5. `sound play` takes PCM WAV only for now; `sound_play` is `run`, cut at 60 s, no extra
   confirmation; app mutes and volumes are `settings_set`.
6. New keys: `sound.app.<id>.volume`, `sound.app.<id>.mute` (dynamic), `sound.alert`,
   `sound.alert_volume`.

## Later

Balance and mono; per-app output devices; the phone's ringer, silent and vibrate modes;
headphone safety limits; hot-plugged USB and Bluetooth devices; MP3, OGG and FLAC in `sound
play`; a sound scheme beyond the alert (start-up, errors, notifications each); the TV's HDMI
ARC and CEC volume.

## Decisions for the engineer

1. The server: per-app `volume`, `mute`, `playing`; `Apps` message; `/var/lib/eggos/sound-apps`
   (64 entries); `SoundChange::{Alert, AlertVolume, AppVolume, AppMute}`; `Play(path, volume,
   max_ms)` and `Alert` messages; the three synthesised alerts of section 1.
2. Keys: `sound.app.<id>.volume` 100, `.mute` false, `sound.alert` `chime`, `sound.alert_volume`
   70; `no_such_app`.
3. Popup: 248x116 with two or more outputs, the output row (0, 48, 248, 32), chevron (214, 61),
   the device list 248 x `2 + 32 n` under it with the tick on the one in use; keys of section 2;
   the wheel 5 % over the indicator.
4. Pane: widgets 17 to 20 at 236..298, `Apps` at 316, rows at `356 + 40 i` (name x 16, glyph
   button (196, 352 + 40 i, 24, 24), slider (232, 350 + 40 i, 216, 28)), eight rows at most,
   `Input` at `356 + 40 A` (396 with none), `SOUND_PANE_HEIGHT` `508 + 40 x max(A, 1)`.
5. CLI forms and words of section 4; tools `sound_apps` (no grant) and `sound_play` (`run`,
   `seconds` at most 60); `sound status --json`'s `alert` object.
6. Phone block `r + 80` per app with the 48x48 glyph button; TV two stops per row; `wm show`
   names of section 5.
