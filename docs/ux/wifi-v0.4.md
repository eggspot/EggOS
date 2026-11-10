# EggOS v0.4 Wi-Fi in Settings > Network: design brief

Design by Claude Fable (D16) for the Wi-Fi list the Network pane lacked (`docs/network.md`, "Not
yet"). It extends `network-v0.3.md`: the same grid, tokens, type and voice, and widgets 1 to 15
stay where they are. The pane gains a **Wi-Fi section** under the wired one, where a person sees
the networks the adapter hears, joins one and forgets it. The backend is as D272 to D280 left it:
the pane reads `/proc/driver/wifi`, writes `/var/lib/eggos/wifi.conf` exactly as `wifi connect`
does and removes it as `wifi forget` does (D22: nothing here is GUI-only). Numbers are px at 1x in
pane coordinates (origin window x 176, y 0; 464 wide, right edge x 448); no motion is added.

Three facts of today's driver shape the design, and section 8 asks for them to change: it joins
only after its next scan (up to 30 s); it tries a failed join again at every scan; and once
joined it stops scanning and reads `wifi.conf` no more, so leaving or changing network takes a
restart. The pane says so rather than hiding it.

## 1. Where the section sits, and the wired section's three changes

The wired section keeps y 16 to 332. The Wi-Fi section starts on the next row of the 40 px grid
(text top 356 = 76 + 7 x 40) and grows downward inside the pane's scroll view, as Components
does. It is **always shown**, also with no adapter, because `Wi-Fi` is one of the pane's search
words and the person who searched must get an answer. With no Wi-Fi adapter it is three widgets
ending at y 392, which fits the default 440 window without scrolling.

Changes to widgets 2 and 4 of `network-v0.3.md` (nothing moves):

- When the Wi-Fi state is `connected`, widget 2 reads `Wi-Fi` instead of `Ethernet`: the joined
  network is the machine's adapter (`eth0`) and rows 1 to 4 are its addresses.
- No wired adapter but a Wi-Fi adapter: widget 2 `No wired adapter.`, widget 4 `Join a Wi-Fi
  network below.` With neither, v0.3's two sentences stand.
- Cable unplugged and a Wi-Fi adapter: widget 4 `Plug in a network cable, or join a Wi-Fi
  network below.`

## 2. Widgets (37 new, numbered on from v0.3's 15)

`T(r) = 404 + 40 x r` is row `r`'s top (r = 0..5); rows under an open passphrase note are 84
lower (section 5). `F` is the bottom of the last shown row (or of the note under it) + 8.

| # | Widget | Place | Text and colour | Shown |
|---|--------|-------|-----------------|-------|
| 16 | label | (16, 356) | `Wi-Fi` in `text` | always |
| 17 | label, right-aligned | ends at 448, top 356 | the state value (section 3) in `text` | always |
| 18 | label, 432 wide | (16, 376) | the hint (section 3) in `text.dim`; a failure in `text` | always |
| 19 | button, 144 x 28 | (16, 400) | `Open Components` | waiting for the firmware |
| 20-43 | six rows of four | panel (16, T(r), 432, 36) in `menu.bg` | see below | one per listed network |
| 44 | panel, 4 x 24 | the joined row's (0, 6) | `accent`, the joined mark | connected |
| 45 | label | (16, F + 6) | `Networks 7 to 12 of 18` in `text.dim` | more than six names |
| 46 | button, 96 x 28 | (352, F) | `More`; on the last page `Strongest` | more than six names |
| 47 | label | (16, F + 6; 34 lower when 45 shows) | section 4's sentence in `text.dim` | a shown row cannot be joined |
| 48 | text field, 288 x 28 | (40, N + 4) | masked, placeholder `Passphrase` | the note is open |
| 49 | checkbox | (344, N + 8) | `Show` | the note is open |
| 50 | label, 192 wide | (40, N + 46) | the note's message (section 5) | the note is open |
| 51 | button, 96 x 28 | (248, N + 40) | `Cancel` | the note is open |
| 52 | primary button, 96 x 28 | (352, N + 40) | `Join` | the note is open |

A row's four widgets, in the row panel's own coordinates (the Components row's shape, one line):

- **name**, label at (24, 10), 160 wide, `text`: the network's name, shortened with `...`
  (`storage_pane::shortened`) when wider.
- **detail**, label right-aligned ending at x 320, top 10, `text.dim`: security and signal, or
  the chosen network's progress (sections 4 and 5).
- **button**, (336, 4, 96, 28), its right edge on the pane's x 448: `Join`, `Forget`, or disabled.
- the panel itself. Rows are 36 high at a 40 pitch, so 4 px of the pane shows between them.

`NetworkPane::WIDGETS` becomes 52 and Settings' `CAPACITY` rises by 37 (the installer's
`dialog::CAPACITY` with it, as the assertion demands). The content's height is the last shown
widget's bottom + 16; with six rows, both footers and a note that is at most 808, scrolled by the pane.

## 3. States: the value (17) and the hint (18)

`C` is the **chosen network**: the `ssid=` value of `wifi.conf`, read when the pane is shown and
after the pane's own writes (the passphrase line is dropped unread). `<name>` in a hint is
shortened to 24 characters and `...`.

| Driver's first line | Value | Hint | Below |
|---------------------|-------|------|-------|
| `no Intel Wi-Fi adapter` | `No adapter` | `EggOS found no Intel Wi-Fi 6 or 6E adapter at boot.` | nothing |
| `no Wi-Fi driver in this kernel` | `No driver` | `This build of EggOS has no Wi-Fi driver.` | nothing |
| `waiting for the file systems` | `Starting...` | `Waiting for the disk that holds the firmware.` | nothing |
| `waiting for the wifi-firmware component` | `Needs firmware` | `Install Intel Wi-Fi firmware in Settings > Components.` | widget 19 |
| `starting the adapter` | `Starting...` | `Loading the firmware into the adapter.` | nothing |
| `scanning`, nothing heard yet | `Searching...` | `Listening for networks.` | nothing |
| `scanning` or `ready`, networks, no `C` | `Not connected` | `Choose a network to join.` | the list |
| `ready`, no networks, no `C` | `Not connected` | `No networks heard. EggOS listens again every 30 seconds.` | nothing |
| `ready; the radio is switched off` | `Radio off` | `Switch Wi-Fi on with the computer's wireless key or switch.` | nothing |
| `scanning` or `ready` with `C`, no failure yet | `Connecting...` | `Joining <name> after the next scan, up to 30 seconds.` | the list |
| `connecting` | `Connecting...` | `Joining <name>.` | the list |
| `connected` | `Connected` | `Joined <name>. Its address is at the top of this pane.` | the joined row only |
| `connected`, `wifi.conf` gone | `Connected` | `Forgotten: EggOS stays on <name> until the next restart.` | the joined row only |
| any other sentence (a failure) | `Not connected` | the driver's sentence, first letter capitalised, `.` added, in `text` | the list |
| `scanning`, `ready` or `connecting` after a failure, same `C` | `Trying again...` | the failure's sentence stays, in `text` | the list |

- `Open Components` shows the Components pane in the same window (the sidebar's row becomes
  current, as a click on it). The component's title is read from the index, not typed twice.
- The driver's sentence is never reworded: it is the one `wifi list` and `wifi_list` print. The
  pane keeps the last failure it read until the state is `connected` or `C` changes, because the
  driver's next scan overwrites it within 30 s.
- Connected shows only the joined row: the driver stops scanning once joined, so the other rows
  would be an old scan. The row's detail carries no signal word for the same reason.
- The pane reads `/proc/driver/wifi` in the one-second tick that reads `/proc/net/interfaces`
  (v0.3 section 3) and sets only what changed. A line it cannot parse is skipped.

## 4. The list

One row per **name**: several access points with one name (`bssid` differs) are one row, the
strongest. Order: the chosen network first, then the driver's order, strongest first.

- **Detail** of a row: `<security> · <signal>` in `text.dim` (` · ` is U+00B7 as elsewhere).
  Security: `Open`, `WPA2`, `WPA3`, `WEP or WPA` (Legacy), `Other security` (Other). Signal in
  words, so no state is told by colour or a small glyph and an agent reads the same thing:
  `Strong` at -60 dBm or better, `Good` from -61 to -72, `Weak` at -73 or worse.
- **Can be joined** (Open, WPA2): the button reads `Join`.
- **Cannot be joined** (WPA3, Legacy, Other): the button is disabled and reads `Cannot join`
  (disabled, so Tab skips it); widget 47 says why, once for the page: `EggOS joins open and WPA2
  networks. WPA3 is not there yet.`
- **A network with no name, or one whose name is not text on one line** (hidden networks): the
  name reads `Hidden network` in `text.dim`, the button `Cannot join`; widget 47 reads the same
  sentence. `wifi connect` can still name it.
- **The chosen network** (`C`), always row 0 of page 1, its detail in `text`:
  - waiting or connecting: `Joining...`, button `Forget`;
  - connected: `Connected`, the `accent` mark (44) at its left edge, button `Forget`; the word
    and the mark together are the sign, never the colour alone;
  - connected with `wifi.conf` gone: `Connected`, the mark, button disabled reading `Forgotten`;
  - after a failure: `Not joined`, button `Forget`;
  - not in the last scan: its row is made from the name alone, detail `Not in range`, `Forget`.
- **Six rows** show. With more names, widget 45 reads `Networks 1 to 6 of 18` and `More` shows
  the next six (`Networks 7 to 12 of 18`); on the last page the button reads `Strongest` and
  returns to page 1. The page is 1 whenever the pane is shown, and is lowered when a scan leaves
  it empty. The footers sit under the last shown row, so a short page has no gap.
- **Empty list**: no rows and no footers; the hint is the message (section 3).
- **A new scan** refills the rows in place. The focus follows the network, not the slot: a focused
  row button stays on the row that now holds the same name, or goes to row 0's button when that
  network is gone. While a note is open the rows do not change (the scan is applied when it
  closes), so the field never moves under the person typing.

## 5. Joining and forgetting

- **Open network**: `Join` writes `wifi.conf` at once (`ssid=<name>\npassphrase=\n`, mode 600).
  No confirmation: the row already says `Open`, and `Forget` undoes it.
- **WPA2 network**: `Join` opens the **note** under that row, 84 high, top `N = T(r) + 40`; the
  rows and footers below move down 84 (the Components note's way). The row's own `Join` hides
  while its note is open, so there is one `Join`. One note at a time: `Join` on another row moves
  the note there and empties the field.
- **The field** (48) is masked: every character draws as `*`, the caret and selection work as
  ever, copy and cut do nothing. `Show` (49) unmasks it while checked and is unchecked each time
  the note opens. The field is person-only (D193): `wm show` lists it without its value and an
  agent joins with `wifi connect` instead.
- **The message** (50) reads `Use 8 to 63 characters.` in `text.dim`. Pressing `Join` (or Enter
  in the field) with fewer than 8 or more than 63, counted as `wifi connect` counts them, turns
  it to `text`, selects the field's text and keeps the focus there; nothing is written. It
  returns to `text.dim` at the next key. A write that fails reads `Could not save: <the error's
  words>.` in `text` and the note stays open.
- **A good passphrase**: the file is written as `wifi connect` writes it, the note closes, the
  network becomes `C` and moves to row 0, the focus goes to its `Forget`, and the value and hint
  read `Connecting...` and `Joining <name> after the next scan, up to 30 seconds.` Then the
  driver's states drive the pane: `Joining <name>.`, then `Connected` (the list closes to the
  joined row, widget 2 of the wired section reads `Wi-Fi` and its rows fill in as DHCP answers),
  or the failure's sentence with `Not joined` on the row.
- **A wrong passphrase** is the driver's sentence. To type another: `Forget`, then `Join`.
- **Joining another network while one is chosen** (not yet connected): `Join` on its row works
  as above and replaces the file; the old row returns to its place by signal.
- **Forget** removes `wifi.conf`, with no confirmation (joining again undoes it). Before the
  driver joined, the row returns to its place with `Join` and the hint to `Choose a network to
  join.` When connected, the row's button becomes the disabled `Forgotten` and the hint says the
  network stays until the next restart (section 3). A remove that fails reads `Could not
  forget: <the error's words>.` in the hint, in `text`.
- **Cancel**, or closing the window, writes nothing; the focus returns to the row's `Join`.

## 6. Focus and keys

- **Tab** from the sidebar list enters the section (the wired section has nothing to focus):
  `Open Components` when shown; then each shown row's enabled button, top to bottom; the note's
  widgets come right after their row's place, in the order field, `Show`, `Cancel`, `Join`; then
  `More`; then Tab wraps to the search field. Shift+Tab the reverse. Showing the pane does not
  move the focus.
- **Up/Down** on a row's button move to the previous or next enabled row button, as Components.
- **Enter or Space** presses the focused button; Space toggles `Show`. **Enter in the field** is
  `Join`.
- **Opening the note** puts the focus in the field. **Esc** in the field empties it when it has
  text (the toolkit's `Escape::Clear`) and closes the note when it is empty; Esc on `Show`,
  `Cancel` or `Join` closes the note at once. Closing returns the focus to the row's `Join`.
  With no note open Esc does nothing in the pane.
- The ring is the toolkit's (2 px `focus.ring`, inset 2). A row scrolled out of view scrolls into
  view when its button or its note takes the focus, the note whole.
- Accessible names carry the network: `Join <name>`, `Forget <name>`, `Passphrase for <name>`,
  `Cannot join <name>: <security>`; the mark is read as `connected` in the row's detail.

## 7. Acceptance (host tests, 640x440, scale 1x, pane coordinates)

- No adapter: `Wi-Fi` at (16, 356), `No adapter` ending at 448, the hint at (16, 376), widgets
  19 to 52 hidden, content height 408. Each other first line gives its row of section 3;
  waiting for the firmware shows `Open Components` at (16, 400, 144, 28), and pressing it makes
  `pane()` `COMPONENTS`.
- `ready` with `home` (WPA2, -48), `cafe` (Open, -66), `lab` (WPA3, -70), `old` (Legacy, -80):
  rows at y 404, 444, 484, 524; details `WPA2 · Strong`, `Open · Good`, `WPA3 · Good`, `WEP or
  WPA · Weak`; buttons `Join`, `Join`, disabled `Cannot join` twice; widget 47 at (16, 574).
  Two lines named `home` make one row. Eight names show six rows, `Networks 1 to 6 of 8` and
  `More` at (352, 648); `More` shows two rows, `Networks 7 to 8 of 8` and `Strongest`.
- `Join` on `cafe` writes `ssid=cafe\npassphrase=\n` with mode 600 and opens no note. `Join` on
  `home` shows the field at (40, 448, 288, 28), `Cancel` at (248, 484), `Join` at (352, 484),
  `cafe`'s row at y 528, and the focus in the field; typing `short` and Enter writes nothing and
  the message is in `text`; `correct horse` and Enter writes `ssid=home\npassphrase=correct
  horse\n`, closes the note and shows `Joining...` and `Forget` on row 0. The field's pixels
  hold no glyph but `*` until `Show` is checked, and `wm show` prints no passphrase.
- Esc with text empties the field; Esc again closes the note with the focus on `home`'s `Join`.
- `connecting; ...` shows `Joining home.`; `connected; ...; network home` shows one row,
  `Connected`, the mark at the row's (0, 6), and widget 2 reads `Wi-Fi`; removing the file then
  shows the disabled `Forgotten` and the restart sentence. A failure sentence shows in `text`
  with `Not joined`; the next `scanning` shows `Trying again...` with the sentence kept.
- A scan that reorders the rows keeps the focus on the same name's button, and changes no row
  while a note is open.

## 8. Backend asks

For the driver and the kernel:

1. Join as soon as `wifi.conf` changes (start a scan then), so the wait is seconds, not up to 30.
2. Keep scanning while connected, leave the network when the file is removed and join the new
   one when it changes, without a restart; then the list stays open under the joined row and
   the `Forgotten` state and its sentence go away.
3. Name the chosen network in the first line while it is not yet joined (`; joining <name>`),
   so the pane and `wifi list` need not read `wifi.conf`.
4. Failures as a short code before the sentence (`wrong-passphrase`, `not-in-range`,
   `no-answer`, `ethernet-in-use`), so a wrong passphrase can reopen the note with `The network
   did not accept this passphrase.`; and keep the last failure in the status until the next try
   ends, so the pane need not remember it.
5. The joined network's signal in the first line, refreshed, for the joined row and later for a
   Wi-Fi glyph on the bar (v0.3 section 4 draws the wired one only).
6. `/proc/net/interfaces` naming the adapter's kind (wired or Wi-Fi), so widget 2 does not infer
   it from the Wi-Fi state.
7. The radio switch's kind (a key or a software block) in the radio-off state, and a way to
   switch a software block off from the pane.
8. WPA3-Personal, then hidden networks (a `Join another network` row with a name field).
9. Names as bytes with an escape for what is not text, so a name with a space at its end or a
   line break parses the same in the pane and the CLI.

For the toolkit:

10. A masked text field: `Field` gains `masked`, drawing `*` per character, refusing copy and
    cut, and listed without its value in `wm show`. Until it exists the note cannot ship.
11. Rows from a table (`toolkit-v0.5.md` section 4) with a button cell, so the list scrolls in
    place and the six-row pages go away.
