# EggOS v0.1 Wi-Fi: joining an Enterprise network (802.1X) from Settings > Network: design brief

Design by Claude Fable (D16) for the sign-in sheet that WPA2-Enterprise and WPA3-Enterprise
networks need now that EggOS joins them. It extends `wifi-v0.4.md` and
`wifi-join-hidden-v0.5.md`: the same pane coordinates (origin window x 176, y 0; 464 wide,
right edge x 448), tokens, row metrics (rows 36 high at a 40 pitch, `T(r) = 404 + 40 r`), the
note pattern (an inline note under the row, top `N` = the row's top + 40, fields 288 x 28 at
x 40, the right column at x 344, `Cancel` and `Join` 96 x 28 at x 248 and 352), the message
line, the voice and widgets 16 to 61. Numbers are px at 1x; no new colour token and no
motion. "Enterprise" below means both WPA2-Enterprise and WPA3-Enterprise: the driver tells
them apart (`Wpa2Enterprise`, `Wpa3Enterprise`), the person sees one form.

Two facts shape the form:

- **The server must prove itself before the sign-in is sent.** Without the CA certificate and
  the server's domain, a look-alike access point could collect the user name and password (or
  be sent the client certificate). So both are **required, in both methods, never skippable**;
  the form says why in one line.
- **EggOS offers two methods**: a password inside a TLS tunnel (PEAP with MSCHAPv2) and a
  client certificate (EAP-TLS, P-256 keys). A network that asks for another (TTLS, EAP-FAST)
  fails with a sentence, not a guess.

## 1. The rows

- **An Enterprise row** (`wifi-v0.4.md` section 4): security words `WPA2 Enterprise` and `WPA3
  Enterprise` (`Security::words`; `Security::of` gains the two driver words), detail `WPA2
  Enterprise · Strong`; the button reads `Join` and opens the **sign-in note** (section 2)
  under the row. Widget 47's sentence shows only for WEP, WPA and other security, as before.
- **The hidden-network note** (`wifi-join-hidden-v0.5.md` section 2) gains a third security
  radio: `Open`, `WPA2 or WPA3`, **`Enterprise`** (61 is three radios; the marks at x 96, 168
  and 288: `Open` 24 + 34, `WPA2 or WPA3` 24 + 95, `Enterprise` 24 + 72, 16 px between, ending
  at 384). With `Enterprise` chosen the passphrase row hides and section 2's block from
  `Method` down sits 72 lower in the same note, which becomes **382** tall (section 2.3).
- **The chosen row** while joining: detail `Joining...` while the driver associates, then
  **`Signing in...`** from the driver's `signing-in` state until `connected`, then `Connected`
  with the mark (44); after a failure `Not joined`. The value (17) reads `Connecting...`
  through both and the hint `Signing in to <name>.` during the second. **Roaming** to another
  access point of the same network (the driver's `roamed` event) changes nothing on screen:
  the row stays `Connected`, the mark stays, no sentence; the only trace is the signal word,
  which follows the new access point at the next scan.

## 2. The sign-in note

`Join` on an Enterprise row opens a note under it, top `N` = the row's top + 40, **310**
tall (`ENTERPRISE_NOTE_HEIGHT`), both methods the same height so switching the method moves
nothing. Rows and footers below move down 310. It reuses widgets 48 (the masked field), 49
(`Show`), 50 (the message), 51 (`Cancel`), 52 (`Join`) and 57 (the lead line) at new places
and adds twelve (62 to 73). Places in pane coordinates:

| # | Widget | Place | Text and colour | Shown |
|---|--------|-------|-----------------|-------|
| 57 | label, 392 wide | (40, N + 6) | `This network signs you in with an account or a certificate.` in `text.dim` | always |
| 62 | label | (40, N + 34) | `Method` in `text` | always |
| 63 | radios, horizontal, 28 tall | marks at (104, N + 28) and (256, N + 28) | `Password (PEAP)`, `Certificate (EAP-TLS)`; the first chosen each time the note opens | always |
| 64 | text field, 288 x 28 | (40, N + 64) | placeholder `User name`, `Escape::Clear` | always |
| 48 | masked text field, 288 x 28 | (40, N + 100) | placeholder `Password`; masked, person-only, as the passphrase field | PEAP |
| 49 | checkbox | (344, N + 104) | `Show`; unchecked at each opening | PEAP |
| 65 | text field, 288 x 28 | (40, N + 136) | placeholder `Anonymous identity (optional)` | PEAP |
| 66 | text field, 288 x 28 | (40, N + 100) | placeholder `Client certificate (PEM)`; the chosen file's path | EAP-TLS |
| 67 | button, 96 x 28 | (344, N + 100) | `Choose...`; opens the picker for 66 (2.2) | EAP-TLS |
| 68 | text field, 288 x 28 | (40, N + 136) | placeholder `Private key (PEM, P-256)` | EAP-TLS |
| 69 | button, 96 x 28 | (344, N + 136) | `Choose...`; for 68 | EAP-TLS |
| 70 | label, 392 wide | (40, N + 176) | `Both prove the server is yours, not a look-alike.` in `text.dim` | always |
| 71 | text field, 288 x 28 | (40, N + 194) | placeholder `CA certificate (PEM)` | always |
| 72 | button, 96 x 28 | (344, N + 194) | `Choose...`; for 71 | always |
| 73 | text field, 288 x 28 | (40, N + 230) | placeholder `Server domain (radius.example.com)` | always |
| 50 | label, 392 wide | (40, N + 272) | the message (section 3) | always |
| 51 | button, 96 x 28 | (248, N + 266) | `Cancel` | always |
| 52 | primary button, 96 x 28 | (352, N + 266) | `Join`; the window's default button while enabled; **disabled until the required fields are filled** (section 3) | always |

The row's own `Join` hides while its note is open; one note at a time, as before (opening it
closes a passphrase or hidden note). `wifi_section::WIDGETS` becomes **58** and Settings'
`CAPACITY` **225** (the installer's with it). Widgets 48, 49, 65 and 66 to 69 share two rows:
a method switch (a click or Left/Right on 63) hides one set and shows the other in the same
frame; the texts typed in the hidden set are kept until the note closes, so switching back
loses nothing.

### 2.1 The fields

- **User name** (64): the identity sent inside the tunnel (PEAP) or the certificate's
  identity (EAP-TLS); 1 to 253 bytes, as typed, `user@example.com` or `DOMAIN\user` alike
  (a backslash is a backslash, the hidden note's rule).
- **Password** (48): any length 1 to 255 bytes; no 8-to-63 rule (that is WPA-Personal's).
  Masked, `Show` unmasks while checked, copy and cut do nothing, person-only for agents.
- **Anonymous identity** (65): the outer identity the access point sees before the tunnel is
  up. Empty sends `anonymous`, which the placeholder's `(optional)` and the agent's value say;
  at most 253 bytes.
- **The three PEM files** (66, 68, 71): editable path fields (a path is no secret and an
  agent may `set-text` one), filled by `Choose...` or typed. The field shows the full path,
  scrolled to its end so the file's name is what is read. A relative path is taken from the
  home folder.
- **Server domain** (73): the name the server's certificate must carry (its subject or a DNS
  subject alternative name; a wildcard certificate matches one label, as browsers do): letters,
  digits, `-` and `.`, 1 to 253 bytes, no spaces, as typed (lower-cased at the match, not in
  the field).

### 2.2 `Choose...` and the picker

A `Choose...` button starts the file picker (`file-picker-v0.3.md`: its own process and
window, `pick open --title "<title>" --folder <start> --filter "PEM files|*.pem;*.crt;*.cer;
*.key" --over <Settings' window id>`), titles `Choose the CA certificate`, `Choose the client
certificate`, `Choose the private key`; the start folder is the home folder, or the folder of
the file already in that field. While it runs Settings is blocked and draws unfocused (the
picker's rule 1); nothing else in the pane changes. Exit 0 puts the chosen path in the field
and the focus on the field's `Choose...` button; exit 1 (Cancel) changes nothing, the focus
returns to the button. A chosen file is checked at once (section 3) so a wrong file is told
before `Join`.

### 2.3 In the hidden-network note

With `Enterprise` chosen on radios 61, the passphrase row is hidden and widgets 62 to 73, 48,
49 and 50 to 52 are placed with **`N' = N + 72`** in place of `N` in the table above (so
`Method` is at `N + 106`, `User name` at `N + 136`, the buttons at `N + 338`); 57 keeps the
hidden note's lead line at `N + 6`, and 70 its own place in the block. The note is **382**
tall. Choosing `Open` or `WPA2 or WPA3` again hides the block and the note returns to 180;
the typed values are kept until the note closes.

### 2.4 Focus and keys

- **Opening** puts the focus in `User name` (the row's own `Join` opened the note). The
  method radios are above it so a certificate user presses Shift+Tab once, or clicks.
- **Tab order**, PEAP: the method radios (one stop; Left/Right choose), `User name`,
  `Password`, `Show`, `Anonymous identity`, `CA certificate`, its `Choose...`, `Server
  domain`, `Cancel`, `Join`, then the next row or footer; Shift+Tab the reverse. EAP-TLS: the
  radios, `User name`, `Client certificate`, its `Choose...`, `Private key`, its `Choose...`,
  `CA certificate`, `Choose...`, `Server domain`, `Cancel`, `Join`. In the hidden note the
  name field and the security radios come first. Hidden widgets are not stops; a disabled
  `Join` is skipped.
- **Enter** in any field is `Join` when `Join` is enabled; otherwise it moves the focus to
  the first required field still empty (the message names it, section 3), so Enter never
  does nothing. Enter or Space on a `Choose...` opens the picker.
- **Esc** in a field with text empties it; in an empty field closes the note; on the radios,
  `Show`, a `Choose...`, `Cancel` or `Join` it closes at once. Closing or `Cancel` writes
  nothing, keeps no typed value, and returns the focus to the row's `Join`.
- **A retry** after a failure: `Join` on the row again opens the note with the method, user
  name, anonymous identity, the three paths and the domain filled from `wifi.conf`; the
  password is always empty.
- While the note is open no row changes (v0.4 section 4).

## 3. Checks and messages

`Join` is **enabled** only when every required field has text: PEAP `User name`, `Password`,
`CA certificate`, `Server domain`; EAP-TLS `User name`, `Client certificate`, `Private key`,
`CA certificate`, `Server domain`. The message (50) says what is still needed, live, in
`text.dim`, naming the fields in Tab order: `Still needed: password, CA certificate, server
domain.`; with everything filled, `The server is checked before your sign-in is sent.` in
`text.dim`. A chosen or typed file is read when the field changes (the first 64 bytes and
the PEM header), a refusal turning the message to `text` and leaving `Join` disabled until
the field changes. Pressing `Join` (or Enter) runs the remaining checks in Tab order; a
refusal writes nothing, turns the message to `text`, puts the focus in the failing field with
its text selected, and the message returns to its resting line at the next key.

| Case | Message (50) |
|------|--------------|
| user name empty after trimming, or over 253 bytes | `Use 1 to 253 characters for the user name.` |
| password over 255 bytes | `Use up to 255 characters for the password.` |
| anonymous identity over 253 bytes | `Use up to 253 characters for the anonymous identity.` |
| a path that does not exist or cannot be read | `Cannot read <name>: <the error's words>.` (the file's last component) |
| the CA or client certificate file is not a PEM certificate (`-----BEGIN CERTIFICATE-----`) | `<name> is not a PEM certificate.` |
| the private key file is not a PEM private key (`-----BEGIN PRIVATE KEY-----` or `-----BEGIN EC PRIVATE KEY-----`) | `<name> is not a PEM private key.` |
| the private key is not P-256 (`wifi connect`'s parser says so) | `EggOS takes P-256 private keys only for now.` |
| the private key is encrypted (`ENCRYPTED PRIVATE KEY`, or `Proc-Type: 4,ENCRYPTED`) | `The private key must not have a passphrase.` |
| the domain is empty, has a space, a character outside letters, digits, `-` and `.`, or is over 253 bytes | `Type the server's domain name, like radius.example.com.` |
| the files could not be copied (section 4) or the write fails | `Could not save: <the error's words>.` |

The checks are `wifi connect`'s own (one parser for the PEM files and the domain, D22), so
the command and the pane refuse the same files with the same words.

**After a good `Join`** the driver's failures show in the hint (18) in `text`, with `Trying
again...` as the value while it retries, and `Not joined` on the row. The exact sentences
for the codes the kernel reports:

| Code | Hint (18) |
|------|-----------|
| `wrong-password` | `The network did not accept the user name or password.` |
| `server-not-trusted` | `The server did not prove it is yours: its certificate is not from this CA, or not for <domain>.` |
| `certificate-refused` | `The network refused the client certificate.` |
| `unsupported-method` | `The network asks for a sign-in method EggOS does not offer yet (not PEAP or EAP-TLS).` |
| `no-answer` | `The network did not answer the sign-in.` |
| any other code | the driver's sentence, as today |

`<domain>` is the saved domain, shortened to 24 characters and `...` as names are in hints.
After `wrong-password` the note reopens from the row with the password empty (2.4's retry);
after `server-not-trusted` with the focus in `CA certificate`, so the two likeliest fixes are
one click away. EggOS keeps trying at each scan, as for any chosen network; a
`server-not-trusted` is retried too (the access point may have been the look-alike), the
sentence staying until the next try ends.

## 4. After Join

- **The files**: the three PEM files are **copied** into `/var/lib/eggos/wifi/` as `ca.pem`,
  `client.pem` and `key.pem` (mode 600, the folder 700), so the join survives the originals
  being moved or on a removable disk, exactly as `wifi connect --ca <file> ...` copies them.
  `Forget` removes them with `wifi.conf`.
- **The write**: `ssid=<name>\nsecurity=enterprise\neap=peap\nidentity=<user name>\n
  password=<password>\nanonymous=<anonymous identity, or nothing>\nca=/var/lib/eggos/wifi/
  ca.pem\ndomain=<domain>\n` (PEAP), or `eap=tls\nidentity=...\ncert=/var/lib/eggos/wifi/
  client.pem\nkey=/var/lib/eggos/wifi/key.pem\nca=...\ndomain=...\n` (EAP-TLS), plus
  `hidden=yes` from the hidden note; mode 600, byte for byte what `wifi connect` writes
  (section 7). `Request::WifiJoin` gains the enterprise fields.
- Then as the hidden note: the note closes, the network becomes the chosen one at row 0, the
  focus goes to its `Forget`, value `Connecting...`, hint `Joining <name>.`, then `Signing in
  to <name>.` with the row's `Signing in...` (section 1), then `Connected`.
- **Forget** removes the file and the copies, with no confirmation, as before.
- One network is kept (D298).

## 5. Touch forms and accessibility

The note in its own coordinates (`W` = the pane's width, 328 on a 390 phone), the hidden
note's phone pattern; every control `W - 48` wide at x 24 and 48 tall, labels 20 tall.

- **Phone**, PEAP: 57 at N + 8 (two lines); `Method` at N + 56; the radios vertical at N +
  80 and N + 128; `User name` N + 184; `Password` N + 236; `Show` N + 288; `Anonymous
  identity` N + 340; 70 at N + 396 (two lines); `CA certificate` N + 444 with `Choose...` under
  it at N + 496; `Server domain` N + 552; the message N + 608; `Cancel` N + 636 and `Join` N +
  692, the primary last: **756** tall. EAP-TLS: `User name` N + 184; `Client certificate` N +
  236, `Choose...` N + 288; `Private key` N + 340, `Choose...` N + 392; 70 at N + 448; `CA
  certificate` N + 496, `Choose...` N + 548; `Server domain` N + 604; the message N + 660;
  `Cancel` N + 688, `Join` N + 744: **808** tall. The note stays inline in the pane's scroll
  view (not a sheet: the hidden note's reasoning), the focused field scrolled into view
  above the keyboard; the keyboard's Enter is `Join`; the picker is the phone's full-window
  picker (`file-picker-v0.3.md` section 16).
- **Tablet**: the desktop note with 40 px controls at a 48 pitch: `Method` N + 28; `User
  name` N + 76; `Password` / `Client certificate` N + 124; `Anonymous identity` / `Private
  key` N + 172; 70 at N + 218; `CA certificate` N + 240; `Server domain` N + 288; the message
  N + 342; the buttons N + 336, 40 tall: **392** tall.
- **TV**: everything doubles, the ring always drawn; Down follows the Tab order, OK on a
  field shows the keyboard, OK on `Choose...` opens the picker at scale 2, Back is Esc.
  **Wearable**: no row (read-only). **Headless**: `wifi connect` and the tool.
- **Names for a screen reader and `wm show`**: the radio group `Method`; `User name for
  <name>`; `Password for <name>` (person-only); `Anonymous identity`; `Client certificate`,
  `Choose client certificate`; `Private key`, `Choose private key`; `CA certificate`, `Choose
  CA certificate`; `Server domain`; `Join <name>`, flagged `disabled` while incomplete, with
  the message read after it; the chosen row's detail read whole (`Signing in...`).
- **Contrast**: the hidden note's pairs, nothing new; a disabled `Join` is `text.dim` on
  `button.bg` 4.1:1, the one disabled pair the pane already allows; a refusal is told by the
  sentence, the focus and the selection, never by colour alone.

## 6. Agent twins (D22)

The sheets carry no footer hint (none of the existing notes do), so none is added; the
twins are these:

| | Join an Enterprise network | Leave it |
|---|---|---|
| GUI | the row and the note | `Forget` |
| CLI | `wifi connect <name> --eap peap --identity <user> --password <pw> [--anonymous <id>] --ca <file> --domain <domain>` and `wifi connect <name> --eap tls --identity <user> --cert <file> --key <file> --ca <file> --domain <domain>` (`--hidden` as before) | `wifi forget` |
| MCP | `wifi_connect` with `eap` (`peap`, `tls`), `identity`, `password`, `anonymous`, `cert`, `key`, `ca`, `domain` | `wifi_forget` |

`wm show` lists, for the open note, the elements of section 5 with their texts as values
(the password blank and person-only); `set-text` fills a path or a name, `select` chooses a
method, `invoke` on `Join <name>` joins. An agent can join an **EAP-TLS** network through the
note (no secret is typed); for PEAP it uses `wifi_connect` with the password, as for
passphrases. `wifi list` and `wifi_list` print the security words of section 1 and the
`signing-in` state.

## 7. Backend asks

1. **`wifi.conf` keys and `wifi connect` flags** as section 4 and 6 name them (`security=
   enterprise`, `eap`, `identity`, `password`, `anonymous`, `cert`, `key`, `ca`, `domain`),
   with the files copied to `/var/lib/eggos/wifi/` by the command, so the pane writes the
   same bytes; `wifi_connect` gains the same fields.
2. **One parser** for PEM certificates and P-256 keys and for the domain, in the shared crate,
   with section 3's refusals as codes (`not-a-certificate`, `not-a-key`, `key-unsupported`,
   `key-encrypted`, `bad-domain`), so the pane's early checks are the command's.
3. **`/proc/driver/wifi`'s first line** says `signing-in` between associating and
   `connected`, and the failure codes `wrong-password`, `server-not-trusted`,
   `certificate-refused`, `unsupported-method`, `no-answer` with their sentences; `roamed` is
   an event in the log, not a state.
4. The driver's security words `Wpa2Enterprise` and `Wpa3Enterprise` in the scan lines.
5. After `wrong-password`, the driver keeps the failure until the next try ends (ask 4 of the
   v0.4 brief), so the note can reopen with the password empty.

## 8. Acceptance (host tests, 640x440, scale 1x, pane coordinates; v0.4's fake plus `corp` WPA2-Enterprise -55, so the rows are `home`, `corp`, `cafe`, `lab`, `old`)

- `corp`'s row at y 444: detail `WPA2 Enterprise · Strong`, button `Join` at (352, 448, 96,
  28). `Join`: the note at N 484: 57 at (40, 490); `Method` at (40, 518); the radios' marks at
  (104, 512) and (256, 512), the first chosen; `User name` at (40, 548, 288, 28) with the
  focus; `Password` at (40, 584) and `Show` at (344, 588); `Anonymous identity` at (40, 620);
  70 at (40, 660); `CA certificate` at (40, 678) and `Choose...` at (344, 678, 96, 28);
  `Server domain` at (40, 714); the message at (40, 756) reading `Still needed: user name,
  password, CA certificate, server domain.`; `Cancel` at (248, 750), `Join` at (352, 750)
  disabled; `cafe`'s row at y 794; the row's own `Join` hidden.
- Right on the radios: `Password`, `Show` and `Anonymous identity` hidden; `Client
  certificate` at (40, 584) with `Choose...` at (344, 584); `Private key` at (40, 620) with
  `Choose...` at (344, 620); nothing else moves; Left brings the first set back with its
  texts kept.
- Typing `kim`, `secret`, choosing `ca.pem` (the fake picker answers `/home/kim/ca.pem`,
  the command `pick open --title "Choose the CA certificate" ...` recorded), typing
  `radius.example.com`: the message turns to `The server is checked before your sign-in is
  sent.` and `Join` enables; Enter writes section 4's PEAP file with `anonymous=` empty and
  `ca=/var/lib/eggos/wifi/ca.pem`, copies the file (mode 600), closes the note, row 0 reads
  `corp`, `Joining...`, `Forget`, focused.
- A domain `radius example` turns the message to the domain sentence in `text`, the focus in
  the field, text selected, nothing written; a chosen `notes.txt` for the CA shows
  `notes.txt is not a PEM certificate.` and keeps `Join` disabled; an encrypted key shows the
  passphrase sentence; a P-384 key the P-256 sentence; an empty user name after Enter the
  user-name sentence.
- The fake picker exiting 1: the field unchanged, the focus on `Choose...`; Settings drawn
  unfocused while the picker runs.
- Driver `associating; ... corp`: `Joining...`; `signing-in; ... corp`: the row `Signing in...`,
  value `Connecting...`, hint `Signing in to corp.`; `connected; ...`: `Connected` with the
  mark; a `roamed` event: no change in the pane's widgets. `failed wrong-password: ...`: the
  hint's sentence, `Not joined`, and `Join` on the row reopening the note with `kim`,
  `/var/lib/eggos/wifi/ca.pem` and the domain filled and the password empty; `failed
  server-not-trusted`: its sentence with `radius.example.com`, the reopened note focused on
  `CA certificate`; `certificate-refused`, `unsupported-method` and `no-answer`: their
  sentences.
- The hidden note with `Enterprise` chosen: the passphrase row hidden, `Method` at (40, N +
  106), the buttons at N + 338, the note 382 tall; `Open` chosen again: 180 tall with the
  block hidden.
- Tab order of 2.4 in both methods; Esc rules; Enter with `Join` disabled moving the focus
  to the first empty required field.
- `wm show` lists section 5's elements, the password blank and person-only, `Join corp`
  `disabled` until filled; `set-text` on `CA certificate` with a path is taken.
- Phone 390: the PEAP note 756 tall and the EAP-TLS note 808 with section 5's places;
  tablet: 392; TV: the ring on `User name` at opening.

## Left out

Networks that use TTLS, EAP-FAST, EAP-PWD or SIM methods (they fail with the sentence);
encrypted private keys and keys other than P-256; a certificate chosen from a system store
rather than a file; a per-user certificate store; several saved networks; remembering the
password across a `Forget`; checking the server's certificate by its fingerprint instead of
a CA; Hotspot 2.0 / Passpoint.

## Decided by design (the owner can overrule any of these)

- One note for both methods, the same height, the method switch moving nothing.
- The CA certificate and the server domain are required in both methods, with one line
  saying why; `Join` is disabled, with a live `Still needed:` line, rather than refusing late.
- Paths are editable fields plus `Choose...`; the chosen files are copied into
  `/var/lib/eggos/wifi/` so a moved original does not break the join.
- The password has no length rule beyond 255 bytes; the user name is typed literally.
- Early checks on each chosen file (PEM header, key curve, encryption) with the command's
  parser; the domain checked as a host name.
- The row says `Signing in...` between `Joining...` and `Connected`; roaming shows nothing.
- The five failure sentences of section 3; after `wrong-password` the note reopens with the
  password empty, after `server-not-trusted` focused on the CA field.
- The hidden note's third radio is `Enterprise`, which grows that note by 202.
- No footer hint: none of the pane's notes has one.

## Decisions for the engineer

1. `Security::{Wpa2Enterprise, Wpa3Enterprise}` with words `WPA2 Enterprise`, `WPA3
   Enterprise`, joinable; `SECURITY_LABELS` gains `Enterprise` (marks at 96, 168, 288).
2. `ENTERPRISE_NOTE_HEIGHT` 310 (382 inside the hidden note, block offset 72); widgets 62 to
   73 at section 2's places; `WIDGETS` 58, `CAPACITY` 225; method switch shows and hides the
   two sets in place, texts kept.
3. `Join` enabled only when the method's required fields have text; the `Still needed:` line
   in Tab order; early file checks on field change; section 3's refusals and sentences as
   constants, one parser shared with `wifi connect`.
4. `Choose...` runs `pick open` with the titles and filter of 2.2, blocked while it runs; exit
   0 fills the field, focus back on the button.
5. The write of section 4 with the copies in `/var/lib/eggos/wifi/`; `Request::WifiJoin`
   gains `eap`, `identity`, `password`, `anonymous`, `cert`, `key`, `ca`, `domain`.
6. States: `signing-in` → row `Signing in...`, hint `Signing in to <name>.`; `roamed` ignored;
   the five failure codes → section 3's sentences; the reopen rules after `wrong-password`
   and `server-not-trusted`.
7. Keys of 2.4 (Enter on a disabled `Join` goes to the first empty required field); element
   names of section 5; phone, tablet and TV numbers of section 5.
