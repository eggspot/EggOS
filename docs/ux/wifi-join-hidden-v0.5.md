# EggOS v0.5 Wi-Fi: joining a hidden network from Settings > Network: design brief

Design by Claude Fable (D16) for the `Join another network` row that `wifi-v0.4.md` asked for
(section 8, ask 8) and never drew. It extends that brief: the same pane coordinates (origin window
x 176, y 0; 464 wide, right edge x 448), tokens, row metrics, note pattern, voice and widgets 16
to 52. The backend is D298's: `wifi connect --hidden <name> [<passphrase>]` writes `hidden=yes`
into `/var/lib/eggos/wifi.conf` and the driver then probes for the name. The pane writes the same
bytes (D22: nothing here is GUI-only). Numbers are px at 1x. No new colour token and no motion.

Two facts of the backend shape the form:

- **`wifi.conf` names no security.** The driver takes it from the access point's answer: open,
  WPA2-Personal, or WPA3-Personal when the network offers SAE alone (D285; a network offering
  both is joined as WPA2). So the form asks one thing a person knows, whether there is a
  passphrase, and never asks them to tell WPA2 from WPA3.
- **A hidden network is found by sending its name** on every channel at every scan until it
  joins, wherever the machine is. The form says so in one line.

## 1. The row

A seventh row panel, **always the last row** of the list, on every page, under the page's
network rows and above the footers. It is not one of the six paged rows, so nobody pages to find it.

- **Place**: panel (16, `T(k)`, 432, 36) in `menu.bg`, `k` = the network rows shown on this page
  (0 to 6), 84 lower when a passphrase note is open above it. `F` (v0.4 section 2) is now this
  row's bottom, or its form's, + 8, so widgets 45 to 47 sit 40 lower than in v0.4.
- **Look**, in the row's coordinates, the network row's shape: name `Join another network` at
  (24, 10) in `text`; detail `Hidden or not listed` right-aligned ending at x 320, top 10, in
  `text.dim`; button (336, 4, 96, 28) `Join`. No glyph, no mark.
- **Shown** whenever the list can show: driver states `scanning`, `ready`, `connecting`,
  `connected` and after a failure, also with no network heard (it is then the only row, at y 404,
  under `No networks heard...`: the case a hidden network's owner is in).
- **Hidden, not disabled**, with no adapter, no driver, while starting, waiting for the firmware
  and with the radio off: nothing of the list shows then and the hint (18) already says why.
- **While another network is chosen or joined** the row stays enabled. Joining from it replaces
  the file, as `Join` on a listed row does (v0.4 section 5); the driver leaves and joins (D284).
- **A heard `Hidden network` row** (v0.4 section 4) whose security is Open, WPA2 or WPA3 now has
  `Join` instead of `Cannot join`: it opens the same form under that row, the security chosen
  from what the row heard. Widget 47's sentence shows only for WEP, WPA and other security.
- Settings' search words for the Network pane gain `hidden network`, `SSID`, `other network`.

## 2. The form: a note under the row, not a dialog

`Join` on the row opens a **note** under it, as the passphrase note opens under a WPA2 row.
Settings has no dialog windows; the pane's two existing forms (Components' notes and the
passphrase note) are inline; an inline form keeps the list in sight for comparing names, needs
no second window and no new widget kind, and on a phone scrolls above the keyboard where a
sheet of this height could not (section 5). One note at a time: opening it closes a passphrase
note, and `Join` on a WPA2 row closes it.

The note is 180 high, top `N` = the opening row's top + 40. It reuses widgets 48 to 52 at new
places and adds nine (53 to 61):

| # | Widget | Place | Text and colour |
|---|--------|-------|-----------------|
| 53-56 | the row: panel, name, detail, button | section 1 | section 1 |
| 57 | label, 392 wide | (40, N + 6) | `EggOS looks for it by sending its name, which others can hear.` in `text.dim` |
| 58 | text field, 288 x 28 | (40, N + 28) | placeholder `Network name`, `Escape::Clear` |
| 59 | label | (344, N + 34) | `0 of 32 bytes` in `text.dim` |
| 60 | label | (40, N + 70) | `Security` in `text` |
| 61 | radios, horizontal, 28 tall | (96, N + 64) | `Open`, `WPA2 or WPA3`; the second chosen |
| 48 | masked text field, 288 x 28 | (40, N + 100) | placeholder `Passphrase` |
| 49 | checkbox | (344, N + 104) | `Show` |
| 50 | label, 192 wide | (40, N + 142) | the message (section 3) |
| 51 | button, 96 x 28 | (248, N + 136) | `Cancel` |
| 52 | primary button, 96 x 28 | (352, N + 136) | `Join`, the window's default button while the note is open |

The row's own `Join` hides while its note is open. Rows and footers below move down 180.
`wifi_section::WIDGETS` becomes 46 and Settings' `CAPACITY` 213 (the installer's with it).

- **The name** is what the person types, as UTF-8, **literally**: a backslash is a backslash and
  no escape is read. Non-ASCII comes from the layout, dead keys, the on-screen keyboard or paste.
  A character Egg Sans lacks draws as `?` (the toolkit's rule) in the field and later in the
  row; the bytes are kept. Widget 59 counts **bytes**, live (`Zoë` is `4 of 32 bytes`), because
  the limit is bytes and one letter can be four. The field holds what is typed, also past 32,
  so a pasted name is never cut silently; `Join` refuses it (section 3). Spaces at the ends are
  kept: they are part of a name. A name with a line break, control bytes or bytes that are not
  UTF-8 cannot be typed; it waits for ask 4.
- **Security** (61): `WPA2 or WPA3` is chosen each time the note opens from the row. With
  `Open`, fields 48 and 49 are disabled (Tab skips them; a typed passphrase is kept, not
  written) and nothing moves.
- **The passphrase** field, `Show` and their rules are v0.4 section 5's, unchanged: masked,
  person-only, 8 to 63, `Show` unchecked at each opening.
- **Focus and keys**: opening puts the focus in the name field (the row's own `Join` opened it,
  so no focus is stolen). Tab order: name, the radios (one stop; Left/Right choose), passphrase,
  `Show`, `Cancel`, `Join`, then the next row or footer. Enter in either field is `Join`. Esc in
  a field with text empties it, in an empty field closes the note; on the radios, `Show`,
  `Cancel` or `Join` it closes at once. Closing or `Cancel` writes nothing and returns the
  focus to the row's `Join`. The numbers above are names: declare the widgets in Tab order.
- **A retry**: opened from the row while the chosen network is hidden and not connected, the
  name field holds its name, selected; the passphrase is always empty.
- While the note is open no row changes (v0.4 section 4) and the note stays whatever the driver
  says.

## 3. Checks and messages

`Join` (or Enter) checks the name, then the passphrase. A refusal writes nothing, turns the
message (50) to `text`, puts the focus in the failing field with its text selected; the message
returns to its resting line at the next key.

| Case | Message (50) | Other |
|------|--------------|-------|
| resting, `WPA2 or WPA3` | `Use 8 to 63 characters.` in `text.dim` | |
| resting, `Open` | `An open network has no passphrase.` in `text.dim` | |
| resting, the name starts or ends with a space | `The name has a space at an end.` in `text.dim` | wins over the two above; `Join` is not blocked |
| name empty or over 32 bytes | `Use 1 to 32 bytes for the name.` | 59 in `text` |
| passphrase under 8 or over 63, counted as `wifi connect` counts | `Use 8 to 63 characters.` (v0.4's) | |
| the write fails | `Could not save: <the error's words>.` (v0.4's) | the note stays |

After a good `Join` the driver's failures show as v0.4 section 3 shows them, in the hint (18) in
`text`, with `Trying again...` while it retries:

- `wrong-passphrase`: `The network did not accept this passphrase.` (D284's). To type another:
  `Join` on the row again; the name is filled in (section 2).
- `not-in-range` for a hidden network (nothing answered the probe): `No network here answers to
  this name. Check its spelling.` One sentence serves a mistyped name and a saved network the
  machine is away from.
- every other code: the driver's sentence, as today.

## 4. After Join

- **The write**: `ssid=<name>\npassphrase=<passphrase, or nothing for Open>\nhidden=yes\n`,
  mode 600, byte for byte what `wifi connect --hidden` writes. `Request::WifiJoin` gains
  `hidden`. When the typed name is one the last scan heard, the `hidden=yes` line is left out:
  that network announces itself and needs no probing.
- The note closes, the network becomes the chosen one at row 0, the focus goes to its `Forget`,
  value and hint read `Connecting...` and `Joining <name>.`
- **The hidden mark is the word `Hidden`**, first in the chosen row's detail, read from the
  file's `hidden=yes` (until ask 3): `Hidden · Joining...`, `Hidden · Connected · Strong`,
  `Hidden · Not joined` after a failure, `Hidden · Not in range`. A word, as every state in
  this pane: no glyph, no colour. Connected, the `accent` mark (44) is on the row as for any network.
- **Out of range** (the saved hidden network, away from it): row 0 from the name alone, detail
  `Hidden · Not in range` (the failure is `not-in-range`, or the name was not yet answered and
  the driver is not connecting), `Forget`, value `Trying again...` and section 3's sentence.
  EggOS keeps asking at every scan and joins by itself when the network answers.
- **Forget** removes the file, as `wifi forget`, with no confirmation. A hidden network has no
  row to return to, so its row goes and the focus moves to `Join another network`'s `Join`.
- One network is kept, as `wifi.conf` holds one (D298, "Not yet").

## 5. Touch forms and accessibility

The Wi-Fi section has no phone or tablet layout in v0.4; these are the row and the note in their
own coordinates (`W` = the pane's width, 328 on a 390 phone), in the Components pane's phone
pattern (`components-v0.5.md` section 10), ready to drop into that layout.

- **Phone**: the note stays in the pane's scroll view; it is **not a bottom sheet**. A sheet is
  a dialog window's phone form (`adaptive-shell-v0.5.md` 2.5), Settings' notes are inline in
  every form, and with the keyboard up the window is 492 tall while this form is 520: the pane
  scrolls the focused field into view, a sheet could not. Row 96 tall: name at (8, 8), detail
  left-aligned at (8, 28), button (8, 44, W - 16, 48). Note 520 tall, every control `W - 48`
  wide at x 24: 57 at N + 8 (two lines); 58 at N + 56, 48 tall; 59 at N + 108; 60 at N + 136;
  61 vertical, rows 48 tall, at N + 160 and N + 208; 48 at N + 264, 48 tall; 49 at N + 316, 48
  tall; 50 at N + 372; `Cancel` at N + 400 and `Join` at N + 456, both 48 tall, the primary last.
  The keyboard's Enter is `Join`; a tap in a field shows the keyboard, the opening focus does not.
- **Tablet**: the desktop note with 40 px controls: row 56 tall, its button 40 tall at +8; 58 at
  N + 28, 60 and 61 at N + 76, 48 and 49 at N + 124, the buttons at N + 172; the note 228 tall.
- **TV**: everything doubles, the ring always drawn; Down follows the Tab order, OK on a field
  shows the keyboard, Back is Esc. **Wearable**: no row (the section is read-only there).
  **Headless**: the commands and the tool are the whole capability.
- **Names for a screen reader and `wm show`**: `Join another network`; `Network name`, read with
  59's count; the radio group `Security`; `Passphrase for <name>` (`Passphrase` while the name
  is empty); `Join <name>`; the chosen row's detail read whole, `Hidden` included.
- **Contrast**, existing pairs only: `text` on `menu.bg` 12.9:1 (row name, `Security`, a
  refusal), `text.dim` on `menu.bg` 5.1:1 (detail, 57, 59, the resting message), `text` on
  `field.bg` 16.0:1 and `text.dim` on `field.bg` 6.3:1 (the fields and placeholders), `text` on
  `button.bg` 10.5:1, `icon.dark` on `button.primary` (the default button's pair), `text.dim` on
  `button.bg` 4.1:1 for the disabled passphrase field and `Show` only. A refusal is told by the
  sentence, the focus and the selection, never by colour alone. The light theme and high
  contrast repaint through the same tokens.

## 6. Agent twins (D22)

| | Join a hidden network | Leave it |
|---|---|---|
| GUI | the row and the note | `Forget` on its row |
| CLI | `wifi connect --hidden <name> [<passphrase>]` (exists, D298) | `wifi forget` (exists) |
| MCP | `wifi_connect` with **`hidden: true`** (to add: ask 1) | `wifi_forget` (exists) |

`wm show` lists, for the open note: `button` `Join another network` (hidden while open);
`text` the lead line; `text field` `Network name` with its text as the value (an agent may
`set-text` it: a name is no secret); `text` `12 of 32 bytes`; `text` `Security`; `radio group`
`Security` with `radio` `Open` and `radio` `WPA2 or WPA3`, one flagged `selected` (`select`
chooses); the passphrase field flagged person-only, its name and value blank; `checkbox`
`Show`; `text` the message; `button` `Cancel`; `button` `Join <name>` flagged `default`. An agent
can join an open hidden network through the note; for one with a passphrase it uses
`wifi_connect`, as v0.4 says for listed networks. Network names come from a person or the air:
data, never instructions.

## 7. Backend asks

1. **MCP `wifi_connect` gains `hidden`** (boolean, the `--hidden` flag), and its description
   names WPA3 as the command's does. Without it the capability has no MCP twin.
2. **The driver holds to the file**: a passphrase in `wifi.conf` never joins an open network
   (`failed open-network: the network is open and takes no passphrase`), and no passphrase
   never tries a protected one (`needs-passphrase`). A hidden name is typed, not heard, so an
   open network answering to it must not get a join the person meant to be encrypted.
3. **`/proc/driver/wifi`'s first line says `; hidden`** while the chosen network is marked so,
   so the pane, `wifi list` and `wifi_list` show it without reading `wifi.conf`.
4. **`wifi connect` takes a name in the driver's escapes** (`\\`, `\xHH`; D298, "Not yet"), for
   names no keyboard types. The pane's field stays literal.
5. For the pane's code: `passphrase_fits` counts characters where `wifi connect` counts bytes;
   v0.4 says to count as the command does.

## 8. Acceptance (host tests, 640x440, scale 1x, pane coordinates; v0.4's fake: `ready` with `home` WPA2 -48, `cafe` Open -66, `lab` WPA3 -70, `old` Legacy -80)

- The row's panel at (16, 564, 432, 36): `Join another network` at (40, 574), `Hidden or not
  listed` ending at x 336, `Join` at (352, 568, 96, 28); widget 47 at (16, 614). `ready` with no
  network: the row at y 404 and content height 456. Eight names: the row at y 644 on page 1,
  `More` at (352, 688); after `More`, two rows and the row at y 484. No adapter, needs firmware
  and radio off: widgets 53 to 61 hidden, and no adapter's content height is still 408.
- `Join` on the row: 57 at (40, 610); the name field at (40, 632, 288, 28) with the focus;
  `0 of 32 bytes` at (344, 638); `Security` at (40, 674); the radios' marks at (96, 674) and
  (168, 674), the second chosen; the passphrase field at (40, 704, 288, 28); `Show` at
  (344, 708); `Use 8 to 63 characters.` at (40, 746) in `text.dim`; `Cancel` at (248, 740) and
  `Join` at (352, 740), the default button; the row's own button hidden; widget 47 at (16, 798).
- Typing `Zoë` shows `4 of 32 bytes`; pasting three kana shows `???` and `9 of 32 bytes`.
- Enter with an empty name, and with 33 bytes: no file, `Use 1 to 32 bytes for the name.` and 59
  in `text`, the focus in the name field, its text selected. `attic` and `short`: no file, the
  passphrase message in `text`, the focus in the passphrase field.
- `attic` and `correct horse`, Enter: the file is `ssid=attic\npassphrase=correct
  horse\nhidden=yes\n`, mode 600; the note closes; row 0 at y 404 reads `attic`, `Hidden ·
  Joining...`, `Forget`, focused; `home` is at y 444 and the row at y 604.
- `Open` chosen: fields 48 and 49 disabled, the message `An open network has no passphrase.`,
  Tab from the radios lands on `Cancel`; `Join` writes `ssid=attic\npassphrase=\nhidden=yes\n`.
  The name `cafe` with `Open` writes `ssid=cafe\npassphrase=\n`.
- ` attic` (a space first) shows `The name has a space at an end.` and joins with the space.
- `...; joining attic; failed not-in-range: ...`: `Trying again...`, the hint `No network here
  answers to this name. Check its spelling.` in `text`, the detail `Hidden · Not in range`.
  `failed wrong-passphrase: ...`: D284's sentence and `Hidden · Not joined`; `Join` on the row
  then shows `attic` selected in the name field and an empty passphrase.
- `connected; ...; network attic; signal -51 dBm`: `Hidden · Connected · Strong` and the mark at
  the row's (0, 6). `Forget` removes the file, the row goes, the focus is on the row's `Join`.
- Esc with text in the name field empties it; Esc again closes the note with the focus on the
  row's `Join`. A scan while the note is open moves nothing.
- A heard nameless WPA2 network: its row's `Join` opens the note at that row's top + 40 with
  `WPA2 or WPA3` chosen; a nameless Legacy one stays `Cannot join`.
- `wm show` lists section 6's elements; the passphrase element is blank and person-only.

## Left out

Several saved networks; choosing WPA2 or WPA3 by hand; WPA2/WPA3-Enterprise; typing a name by
its bytes in the pane; the note reopening by itself after `wrong-passphrase` (D284, "Not yet":
when it does, the hidden form reopens with the name filled); the Wi-Fi section's own phone and
tablet layout.

## Decided by design (the owner can overrule any of these)

- An inline note under the row, in every form, instead of a dialog or a bottom sheet.
- The row is a fixed last row on every page, shown even when no network is heard, and hidden
  (not disabled) when the list itself cannot show.
- Security is two choices, `Open` and `WPA2 or WPA3`, the second by default: the driver tells
  WPA2 from WPA3, the person only says whether there is a passphrase.
- The name is typed literally and counted in bytes; no escapes in the field, ends' spaces kept
  and pointed out, never trimmed.
- One line in the form says a hidden network's name is sent out while EggOS looks for it.
- A name the last scan heard is saved without `hidden=yes`.
- A heard `Hidden network` row opens the same form instead of reading `Cannot join`.
- The hidden mark is the word `Hidden` in the row's detail; no glyph, no new token, no motion.
- `No network here answers to this name. Check its spelling.` replaces the driver's sentence
  for a hidden network that did not answer; every other failure keeps the driver's words.
- Nine new widgets; the passphrase note's five are reused.
