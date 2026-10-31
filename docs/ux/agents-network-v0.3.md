# EggOS v0.3.0 Agents: the network view and pairing (slice 41d)

Design by Claude Fable (D16) for the GUI front-end of `agent network [--port N] on|off|show`
and `agent pair [--name NAME] <id>` (D22, D193): the part of the Settings `Agents` pane that
says whether agents on other machines are answered over HTTPS, shows the certificate they pin,
and pairs one with a one-time code. Everything in `agents-v0.3.md` stands (its numbers, words,
states, keys and element-tree names); this brief adds to it and changes only what it names.
Numbers are px at 1x and double at 2x. The only new motion is the pairing panel's
one-a-second countdown, ticked as the grant prompt's is and as About's uptime line is.

## 1. Where it lives: a third view, `Network`, and one panel

- **A third view radio.** The view group (widget 2) becomes `Grants` `Activity` `Network`
  (about 250 wide, so from x 198; the 2x heading ends at x 84). The network's state is a
  setting with a few facts under it, exactly what a view under the shared agents list is for;
  it is not a per-agent fact, so it does not belong in `Grants`, and a panel is for one
  action, not a standing state. `Network` is the CLI's noun and what macOS's `Sharing` and
  Windows's `Bluetooth & devices` call the same idea.
- **Pairing is a modal panel** in the pane (section 2.3's shape), two steps in one set of
  widgets: ask for the id and the name, then show the code until it is used. That is how
  macOS and Windows pair a device: a sheet that asks, then shows the code and waits. The code
  is a secret with a 5-minute life, so it is shown once, in the panel, never kept in a view.
- **Budget**: the tree holds 120 after the activity view. The network view is 7 widgets and
  the panel 12, so `CAPACITY` becomes **139** (the installer's `eggos_app::dialog::CAPACITY`
  stays equal to it). The agents list, the heading, the view radios and the status line are
  the shared widgets 1-7 and cost nothing more.
- **Search words** added to the pane's: `network`, `pair`, `pairing`, `code`, `HTTPS`,
  `certificate`, `fingerprint`, `remote`, `port`, plus the button names `Pair an agent...`
  and `Pair`. `network` and `port` also match the Network pane; nothing else collides.
- **Person-only (SECURITY).** Turning the network on or off and pairing are person-only
  (41d: no MCP tool does either), so `Off` `On` and both options, `Pair an agent...`, the
  panel's two fields and its two buttons carry `set_person_only(id, true)`: `wm act` answers
  `element_cannot` on every `Action`, and `wm show` lists `person-only`. The `Network` radio
  itself and the view's labels stay readable and the radio actable: an agent may look.

## 2. The network view (shown with widgets 1-7 when `Network` is chosen; widgets 8-31 hidden)

Pane coordinates as `agents-v0.3.md` section 2 (origin window x 176, y 0; right edge x 448;
usable height 332). The program reads `/var/lib/eggos/agent-network`, the certificate
`/var/lib/eggos/agent-network.der` and the pairing file `/run/eggos/agent-pairing` when the
view is shown and after every change it writes, through the functions `agent network show`
and `agent pair` use, moved where Settings can call them (`eggos_mcp::pairing`:
`network_state` giving off or on with the port, `certificate_fingerprint`, `pending`,
`begin_pairing(id, name, now)` and `turn_network(on_with_port | off)`, so the CLI and the
pane share one code path as `agent list`'s rows do); never on a tick while the panel is
hidden.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 37 | push button | (240, 64), 208x28 | `Pair an agent...` (three periods: the font has no U+2026) |
| 38 | label | (16, 154) | `Network`; `Network · from EggOS's next start` once the pane has written the file in this Settings run; in `text` |
| 39 | radios, horizontal | right-aligned, ends at x 448, y 148, 28 tall (about 110 wide) | `Off` `On`; the file's state |
| 40 | label | (16, 186) | on: `Agents connect to https://<this machine>:8443/mcp` in `text`; off: `Off: agents on the network are not answered.` in `text.dim` |
| 41 | label | (16, 206) | `Certificate for the client: /var/lib/eggos/agent-network.pem` in `text`; without a certificate `Certificate: made when the network server first starts` in `text.dim` |
| 42 | label | (16, 226) | `SHA-256 ` then the fingerprint's first 16 bytes with their colons, `AB:CD:...:PQ:` (48 characters, about 284 px), in `text`; hidden without a certificate |
| 43 | label | (60, 246) | the fingerprint's last 16 bytes, `RS:TU:...:YZ` (47 characters), in `text`; x 60 is 16 plus the 44 px of `SHA-256 `, so the two halves align; hidden without a certificate |

The fingerprint is `pairing::fingerprint_text`'s 95 characters, split after the sixteenth
byte's colon: a label holds 64 bytes and the pane is 432 wide, so one line cannot hold it,
and two lines of sixteen bytes are how people compare a fingerprint by eye (the client's
own shows it the same way). `<this machine>` is the CLI's placeholder: the pane uses the
same words until the CLI and the pane learn the machine's address together (left out).
The view's lines fit the row grid of the grants view: `Network` sits where `View` does, and
the last line's cell ends at 262, well inside 332; at a taller window nothing moves.

**Status line (widget 7) while idle in this view**, the first that applies:

| State | Pair an agent... | Off / On | Status line |
|-------|------------------|----------|-------------|
| the store cannot be trusted (`StoreState::Unreadable`) | off | on | `Grants file cannot be trusted: <reason>.` |
| no disk (D131) | on | on | `No disk attached: changes last until power-off.` |
| a code is open (`pending().is_open(now)`) | on | on | `Waiting to pair: phone; its code ends 09:19:02.` in `text` (UTC time of `expires`) |
| otherwise | on | on | `Pairing shows a one-time code for an agent on another machine.` in `text.dim` |

The network radios never depend on the store: the file is its own. With no disk the file is
written where `/var/lib/eggos` lives until power-off, as the grants are.

**Choosing `On` or `Off`** writes in the same keystroke or click, instant and silent, as the
class radios do: `On` is `agent network on` with the port the file already holds or `8443`;
`Off` is `agent network off`. Choosing the chosen option writes nothing. After the write the
pane re-reads the file, label 38 gains its note, label 40 changes, and the status line reads
in `text`: `Agents on the network are answered from EggOS's next start.` or `Agents on the
network are not answered from EggOS's next start.` (the CLI's words, 59 and 63 bytes). The
note on label 38 says plainly that the running server is what EggOS started with; the pane
cannot tell whether a server is listening now, and the CLI does not claim to either. Changing
the port stays the CLI's `--port` in 0.3.0 (left out). A failed write keeps the radios on the
file's state and shows `Network file not written: <errno text>.` in `text`.

**Pair an agent...** opens the panel of section 3 in its ask step, unless the last code was
burned and its 30 s have not passed: then nothing opens and the status line reads, in `text`,
the CLI's refusal `The last code was burned by wrong tries: pair again in 20 s.` (the seconds
as of the press; pressing again re-reads). The pane does not tick while idle, so the number
is not counted down in the status line; the panel's countdown is the only tick.

## 3. The pairing panel, drawn as the confirmation panel (section 2.3 of `agents-v0.3.md`)

A modal panel inside the pane, 400 wide, 236 tall, `menu.bg`, `INSET_R6`, 1 px `menu.border`
inside, a 2 px `field.bg` edge outside, centred: x 32; y `(pane height - 236) / 2`, 48 at the
minimum window, 56 at the default. While it shows the pane's widgets ignore pointer, wheel
and Tab and the lists draw unfocused; the status line (widget 7, at y 312, below the panel)
stays visible and keeps saying what matters. Two steps, `ask` and `code`, share the widgets:
a step shows, hides, enables or retitles them and never moves them.

| # | Widget | Place in the panel | Text |
|---|--------|--------------------|------|
| 44 | panel | (0, 0), 400x236 | |
| 45 | label | (16, 16) | ask: `Pair an agent on another machine`; code: `Pairing phone` (`Pairing phone · Claude on my phone` when a name was given or kept; ellipsised at the panel's right padding) in `text` |
| 46 | text field | (16, 48), 176x28 | placeholder `Agent id`, `escape: Clear`; ask step only; the focus when the panel opens |
| 47 | text field | (208, 48), 176x28 | placeholder `Name, optional`, `escape: Clear`; ask step only |
| 48 | label | (16, 88) | ask: the hint (below) in `text.dim`, or the refusal in `text`; code: `Its client sends this code once, within 5 minutes, to` in `text.dim` |
| 49 | label, 2x | (16, 48) | the code `BCDE-FGHJ` (`pairing::code_text`: two groups of four from the alphabet without `0 O 1 I L`, 98 px wide, 32 tall) in `text`; code step only |
| 50 | label | (16, 108) | `https://<this machine>:8443/pair` (the port from the file, `8443` when off) in `text`; code step only |
| 51 | label | (16, 136) | `SHA-256 ` and the fingerprint's first half, as label 42; without a certificate `Certificate: made when the network server first starts` in `text.dim`; code step only |
| 52 | label | (60, 156) | the fingerprint's second half, as label 43; code step only, hidden without a certificate |
| 53 | label | (16, 198) | the countdown (below) in `text`; code step only |
| 54 | push button | (192, 192), 96x28 | `Cancel`; ask step only |
| 55 | push button, primary | (288, 192), 96x28 | ask: `Pair`; code: `Done`; the window's default button while the panel shows |

**The ask step.** `Pair` is enabled only when the id is valid, and label 48 says why when it
is not, the first that applies (the id trimmed; the rules are `AgentId::new`'s and
`AgentName::new`'s, the CLI's):

| The fields | Pair | Label 48 |
|------------|------|----------|
| the id is empty | off | `An id is 1 to 32 letters, digits, ".", "_" or "-".` |
| the id is not a valid id | off | the same line |
| the id is `local` | off | `local is the host's channel, which needs no pairing.` |
| the name is over 64 bytes | off | `A name is at most 64 bytes.` |
| the id is new and the store holds 16 agents | off | `EggOS keeps at most 16 agents: forget one first.` |
| the id is already listed | on | `phone is listed: a new token, and its grants stay.` |
| otherwise | on | `A one-time code for its client follows; it works for 5 minutes.` |

`Pair` (or Enter in either field when `Pair` is enabled) runs `begin_pairing`: the burned
wait is checked again at that moment and refuses with the CLI's line in label 48, in `text`,
the panel staying in the ask step: `The last code was burned by wrong tries: pair again in
20 s.`; otherwise the agent is added to the store with no grants when new (an existing one
keeps its grants and its name unless a name was typed), a new token replaces any old one, the
pairing file is written, the store is written, and the panel goes to the code step. An empty
name is none, as the CLI's missing `--name`: the kept name or the id. Failures keep the ask
step and show in label 48 in `text`: `No randomness for a code: <errno text>.`, `Pairing file
not written: <errno text>.`, `Grants file not written: <errno text>.`, `Grants file cannot
hold this: it holds too many agents.` `Cancel`, Esc or closing the window writes nothing.

**The code step.** The agents list behind the panel re-reads the store and selects the
agent's row. The panel ticks once a second (`timeout_ms` of 1000 while it shows, as the grant
prompt does), re-reads the pairing file and retitles label 53 only:

| Pairing file | Label 53 | What else |
|--------------|----------|-----------|
| open, no wrong tries | `Ends in 4:59.` (minutes and seconds left of `expires`, from `5:00`) | |
| open, wrong tries | `Ends in 4:59; 2 wrong tries, 5 burn it.` | |
| gone (the server removed it: the client sent the right code) | | the panel closes itself, the row stays selected and the status line reads in `text`: `Paired phone: it gets nothing until you grant it.` |
| `burned_at` set | `Burned by 5 wrong tries; a new code may be made in 30 s.` (counting down to `0 s`) | the 2x code stays shown; nothing accepts it any more |
| expired (`now >= expires`) | `The code has ended; pair again for a new one.` | the same |

While the code step shows and the network file is off, the status line below the panel
reads in `text` the CLI's note: `The network server is off: choose On, then start EggOS
again.` (the code is still made, as the CLI makes it; it will be useless until a server
runs, and says so). `Done`, Enter, Esc or closing the window closes the panel and nothing
else: the code stays open until used or ended, exactly as after the CLI's `agent pair`, and
the view's status line then reads `Waiting to pair: phone; its code ends 09:19:02.` There is
no `Cancel` in the code step because the CLI has no way to withdraw a code either; one is
listed in "Left out". After the panel closes the pane re-reads the view and ticks no more.

## 4. Keys, focus and pointer (adding to section 6 of `agents-v0.3.md`)

- Tab from the agents list in `Network`: `Pair an agent...`, then `Off` `On`, then wraps to
  the search field; Shift+Tab the reverse; disabled ones skipped. Left/Right in the network
  group choose and write per step, as the class radios do.
- While the panel shows in the ask step: Tab goes the id field, the name field, `Cancel`,
  `Pair` (when enabled), wrapping; Enter in a field is `Pair` when enabled, else nothing;
  Enter on a button presses it; Esc is `Cancel`. In the code step Tab stays on `Done`
  (the only stop); Enter and Esc are `Done`.
- Pointer: `IBeam` over the two fields, `Arrow` elsewhere; a click outside the panel does
  nothing (the panel is modal, as the confirmation panel is).

## 5. Every device class (D23)

- **One column** (pane narrower than 448): the view radios go under the heading at (16, 56)
  on one line (250 fits a 282 px phone pane); `Pair an agent...` moves under the agents list,
  x 16, full width minus 32; `Network` and its radios take two lines (label, then the group
  on the next, 28 + 28, 8 between); labels 40-43 follow, x 16, 20 apart; the fingerprint's
  halves stay two lines (284 px fits 282 only just: on a pane under 316 the first half
  ellipsises, and the CLI has the whole text). The panel takes the pane's width minus 16 and
  sits at the pane's bottom as a sheet; the two fields stack full width (id, then name, 8
  apart), the 2x code takes its own line, the fingerprint halves ellipsise as above, and the
  buttons share the last row (`Cancel` left, `Pair` right) or, in the code step, `Done` alone
  on the right.
- **Touch**: buttons and radios 40 tall, fields 28; the panel's rows move down by the
  difference and it grows to fit.
- **TV (scale 2)**: everything doubles; the code at 2x of 2x is 196 wide and readable from a
  sofa, which is the point of showing it 2x everywhere.
- **Keyboard-only**: the panel needs Tab, typing and Enter; the view needs arrows and Enter.
- **Headless**: no pane; `agent network` and `agent pair` are the whole capability.

## 6. What an agent sees (D161, `wm show`)

The view radio group `View` gains the `radio` `Network`. In the view: a `button` `Pair an
agent...`; a `radio group` named `Network` with `radio`s `Off` and `On`, the chosen one
`selected`; the five lines `label`s with their drawn text. In the panel: `text field`s named
`Agent id` and `Name, optional`; `button`s `Cancel` and `Pair` or `Done`; the code a `label`
whose text is the code (an agent that reads the screen of the machine it is on is the local
agent, which needs no pairing; the code is for a client elsewhere, and D193's grants store
already lets `local` see nothing it is not granted). Every control named in section 1 is
flagged `person-only`.

## 7. Acceptance (host tests, default 640x440, scale 1x, the fake of `agents-v0.3.md`
section 9 with its clock at 2026-10-31T09:14:02Z, the network file holding `8443`, a
certificate whose fingerprint starts `AB:CD`, and no pairing file)

- The view radios end at x 624, y 18 and read `Grants` `Activity` `Network`; `CAPACITY` is
  139 and the pane's tree holds exactly 55 widgets plus its container.
- `Network` hides widgets 8-31 and shows, in window coordinates: `Pair an agent...` at (416,
  64, 208, 28); `Network` at (192, 154); `Off` `On` ending at x 624, y 148, `On` chosen;
  `Agents connect to https://<this machine>:8443/mcp` at (192, 186); `Certificate for the
  client: /var/lib/eggos/agent-network.pem` at (192, 206); `SHA-256 AB:CD:` ... at (192, 226)
  whose text is 56 bytes and ends in a colon; the second half at (236, 246), 47 bytes; the
  status line `Pairing shows a one-time code for an agent on another machine.` in `text.dim`.
- A fake with no network file shows `Off` chosen, `Off: agents on the network are not
  answered.` in `text.dim`; a fake with no certificate shows `Certificate: made when the
  network server first starts` and hides labels 42 and 43.
- `Off` removes the file, retitles label 38 `Network · from EggOS's next start` and shows
  `Agents on the network are not answered from EggOS's next start.`; `On` writes `8443\n` and
  shows the answered line; `On` again writes nothing; a fake whose write fails with `EROFS`
  keeps `Off` chosen and shows `Network file not written: ...`.
- `Pair an agent...` opens the panel at (208, 56, 400, 236) with `Pair an agent on another
  machine`, the id field (224, 104, 176, 28) focused, the name field (416, 104, 176, 28),
  label 48 reading the id rule in `text.dim`, `Cancel` at (400, 248), `Pair` at (496, 248)
  primary and disabled; typing `x!` keeps it disabled with the rule; `local` shows the local
  line; `phone` enables it with `phone is listed: a new token, and its grants stay.`; a
  65-byte name disables it with the name line; on a store of 16 agents `tv`
  disables it with the 16-agents line and `phone` does not.
- `tv` and Enter write the store with the line `tv\tdeny\tdeny\tdeny\tdeny\ttv` plus its token
  hash (version 2) and the pairing file for `tv` with `expires` 1761902342 (09:14:02 + 300);
  the panel reads `Pairing tv`, a 2x code of 9 characters at (224, 104) matching
  `[A-HJ-KM-NP-Z2-9]{4}-[A-HJ-KM-NP-Z2-9]{4}`, `Its client sends this code once, within 5
  minutes, to` at (224, 144), `https://<this machine>:8443/pair` at (224, 164), the
  fingerprint halves at (224, 192) and (268, 212), `Ends in 5:00.` at (224, 254), `Done`
  alone at (496, 248) primary and focused, `Cancel` hidden; the agents list has `tv` selected.
- After one fake second the line reads `Ends in 4:59.`; a pairing file rewritten with 2 wrong
  tries reads `Ends in 4:58; 2 wrong tries, 5 burn it.`; with `burned_at` = now it reads
  `Burned by 5 wrong tries; a new code may be made in 30 s.` and `29 s` a second later;
  after 300 fake seconds `The code has ended; pair again for a new one.`; removing the file
  closes the panel and shows `Paired tv: it gets nothing until you grant it.` with `tv` still
  selected.
- With the network file absent the code step's status line reads `The network server is
  off: choose On, then start EggOS again.` and the URL still names port 8443.
- `Done` closes the panel, writes nothing, and the status line reads `Waiting to pair: tv; its
  code ends 09:19:02.`; `Pair an agent...` with a pairing file burned 10 fake seconds ago
  opens nothing and shows `The last code was burned by wrong tries: pair again in 20 s.`;
  `Pair` pressed in an open panel on the same fake shows that line in label 48 and stays in
  the ask step.
- `phone` and the name `My phone` and `Pair` keep `phone`'s grants line (`allow`, `ask` with
  its end, `deny`, `deny`), set its name to `My phone` and a fresh token hash, and the panel
  title reads `Pairing phone · My phone`; an empty name keeps `Claude on my phone`.
- Esc in the ask step closes the panel and writes nothing; Esc in the code step closes it and
  the pairing file stays.
- The element tree holds the group `View` with three radios; the group `Network` with `Off`
  and `On` and the button `Pair an agent...` flagged `person-only`; while the panel shows,
  the fields `Agent id` and `Name, optional` and the buttons `Cancel` and `Pair` flagged
  `person-only`; `wm act` `Select` on `Off`, `Invoke` on `Pair an agent...` and `SetText` on
  `Agent id` answer `element_cannot` and change nothing; `Select` on the `Network` view radio
  answers `Done`.
- A pane 400 wide lays `Pair an agent...` under the list and the network radios under their
  label, and the panel as a sheet at the pane's bottom with the fields stacked.
- `agent network show` and the view agree on the state, the port, the fingerprint and the
  waiting agent on every fake above; `agent pair tv` on the fake and the panel's `Pair` leave
  the same store line and the same pairing file fields apart from the code, the token and
  their hashes.

## Left out of 0.3.0

Choosing the port in the pane (the CLI's `--port`); the machine's own address in place of
`<this machine>` (the Network pane knows it; the CLI and the pane will learn it together so
their words stay the same); withdrawing an open code (neither the CLI nor the pane can;
it ends in 5 minutes); showing the token (the client gets it over `/pair`, nobody else);
a copy button for the fingerprint (no clipboard in 0.3.0); telling whether the server is
listening right now; a QR code; renaming an agent outside pairing; pairing more than one
agent at a time (one pairing file, as the CLI); a centred label mode for the code (left at
x 16 like every other line, so the toolkit gains nothing).
