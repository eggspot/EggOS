# EggOS v0.1 Settings > Network: a wired adapter's MTU and Wake-on-LAN: design brief

Design by Claude Fable (D16) for the two link settings D708 left for "after a Fable design":
the adapter's **MTU** (68 to the adapter's largest, 1500 the standard, jumbo frames up to 9000
on the adapters that take them) and **Wake-on-LAN** (a magic packet wakes the machine from
sleep or power-off, on or off). The capability exists (D22): the CLI `ip link set dev <name>
mtu <bytes> wake magic|off`, the MCP tool `net_link_set` (`interface`, `mtu`, `wake_on_lan`),
and `net_status` / `/proc/net/interfaces` reporting `max_mtu` and `wake_on_lan`. This brief
adds the GUI front-end: two rows and a status line in the pane's wired section. It extends
`network-v0.3.md` (the grid, the voice, widgets 1 to 15 where they are) and `wifi-v0.4.md`,
and follows `settings-forms-v0.5.md`'s row rules (R2, R3, R8, R9). Numbers are px at 1x in
pane coordinates (origin window x 176, y 0; 464 wide, right edge x 448); no motion is added.

## 0. What a person gets, and how that is checked

- Under the wired adapter's traffic lines, an `MTU` row with the number and a `Set` button,
  and a `Wake-on-LAN` row with a `Magic packet` checkbox, greyed when the adapter cannot wake.
- One status line under them that says the adapter's range at rest, the result after a
  change, and plain words when something was refused.
- `ip link set`, `net_link_set`, `settings set network.mtu` and the pane do the same thing
  through the same kernel call and say the same sentences; an agent reads the rows in `wm
  show` and presses `Set` with `wm act`.
- Host tests at 640x440, 390x752, 1024x660, 960x540 at scale 2 and 320x320 (section 7).

## 1. Layout: three rows added to the wired section (widgets 53 to 58)

The wired section's rows 0..4 (text tops 76 to 236) and lines (276, 296, 316) stay. The new
rows take grid rows 7, 8 and 9 (`T = 64 + 40 r`, text at `+ 12`, controls at `+ 6`); the
wired section ends at **452** instead of 332 and the Wi-Fi section of `wifi-v0.4.md` starts
120 lower: `Wi-Fi` at (16, 476), its hint at 496, `Open Components` at 520, rows at `T(r) =
524 + 40 r`. The 440 window scrolls the pane as it already does with Wi-Fi rows.

| # | Widget | Place | Text and colour | Shown |
|---|--------|-------|-----------------|-------|
| 53 | label | (16, 356) | `MTU` in `text`; `text.dim` while 54 is disabled | an adapter exists |
| 54 | text field | (272, 350), 72x28 | the current MTU in bytes (`1500`), digits only; `escape: Clear`, then the pane refills it (section 3) | an adapter exists |
| 55 | push button | (352, 350), 96x28 | `Set`; enabled while 54's text is non-empty and differs from the current MTU | an adapter exists |
| 56 | label | (16, 396) | `Wake-on-LAN` in `text`; `text.dim` while 57 is disabled | an adapter exists |
| 57 | checkbox | (288, 390), 28 tall, `Length::Content` wide (about 108) | `Magic packet`, ticked when `wake`; disabled when the adapter cannot wake (`-`) | an adapter exists |
| 58 | label, 432 wide | (16, 436) | the link status line (section 4), `text.dim` at rest, `text` after an action | an adapter exists |

The field is 72 wide (five digits and the 8 px insets) at 272 so that `Set` keeps R3's place
(352, 96 wide, right edge 448) with the 8 px gap date-time's field and `Set` have; R2's x 288
holds for the checkbox. `Set` is not the window's default button (the pane has no one
action; Enter in the field presses it, section 3). "An adapter exists" is widget 13's rule:
`eth0` is in `/proc/net/interfaces`, wired or the joined Wi-Fi adapter alike; the kernel's
`max_mtu` and `wake` facts decide what each row allows, never the adapter's kind. With no
adapter the six widgets are hidden and the Wi-Fi section is built at its v0.4 place (y 356):
"no adapter" is settled at boot, so the section's origin is chosen once when the pane is
built and never moves afterwards (`network-v0.3.md` section 2's rule holds within a boot).

`NetworkPane::WIDGETS` becomes 58 and Settings' `CAPACITY` rises by 6 (the installer's
`dialog::CAPACITY` with it, as the assertion demands). Search words gained: `jumbo`, `jumbo
frames`, `wake`, `Wake-on-LAN`, `WoL`, `magic packet`, `sleep`.

## 2. The facts, from one parse

Per tick (`network-v0.3.md` section 3) the pane reads `mtu`, `max_mtu` and `wake` (`wake`,
`no-wake` or `-`) from `/proc/net/interfaces`, the same line `ip link` prints as `mtu 1500
... maxmtu 9000 wake magic`. `MIN_MTU` is 68 (D708, IPv4's minimum); `max_mtu` is the
adapter's (1500 on the Realtek chips and Wi-Fi, 9000 on the Intel ones). Each tick sets only
what changed: widget 13's `· MTU <n>` as today; 54's text to the new MTU **only when the
field is not focused and shows the value the pane last put there** (a person's typing is
never overwritten; an agent's `ip link set` while the field rests shows up within a second);
57's tick to `wake` unless a change is in flight (section 3); 58's rest sentence when
`max_mtu` or `wake` changes.

## 3. Changing

- **MTU.** Enter in the field or a press on `Set` applies the field's text. Validation, in
  this order, with the sentence shown in 58 in `text` and nothing applied: trimmed text that
  is not 1 to 5 decimal digits reads `Not a number. Type the MTU in bytes, 68 to <max>.`; a
  number outside `68 ..= max_mtu` reads `The MTU must be 68 to <max> on this adapter.`;
  equal to the current MTU: `Set` is disabled, so nothing happens. The parser and the range
  check are `ip link set`'s (D22). A valid number is applied with `SIOCSIFMTU` on a socket,
  exactly as `ip` does: the field and `Set` are disabled and 58 reads `Setting MTU <n>...` in
  `text` until the next tick shows the new `mtu` (then `MTU set to <n>.` and 54 shows it,
  enabled, `Set` disabled) or the call fails (section 4; the field keeps the text and the
  caret for correction, `Set` enabled again). The call returns at once, so the `Setting...`
  state lasts one tick at most; it exists so a slow adapter reset and an agent's screen look
  the same. An Intel adapter re-initialises its rings on an MTU change, so the link may
  bounce: the state rows show `Cable unplugged` then `Connected` as the facts say, nothing is
  hidden. Esc in the field empties it (the toolkit's `Escape::Clear`); the pane then refills
  it with the current MTU and `Set` goes disabled: one Esc is "put it back". A press on
  `Set` while 58 shows an error replaces the error.
- **Wake-on-LAN.** A press on the checkbox (or Space while focused) applies at once with
  `SIOCETHTOOL` `ETHTOOL_SWOL` (magic packet or nothing), as `ip link set ... wake` does: the
  box shows the new state and is disabled, 58 reads `Turning Wake-on-LAN on...` (`off...`)
  in `text`, until the next tick confirms it (`Wake-on-LAN on: a magic packet wakes this
  computer from sleep or power-off.` / `Wake-on-LAN off.`) or the call fails, when the box
  goes back and 58 says why. No confirmation: the person pressed it, and off undoes on.
- **Disabled wake.** `-` in the facts (the adapter cannot wake): 57 is disabled with its
  label `text.dim`, 56 `text.dim`, and 58's rest sentence says why (section 4); the
  pointer and keys skip it as any disabled control.
- **Focus and keys.** Tab from the sidebar list enters the wired section for the first time:
  54, 55, 57 (when enabled), then the Wi-Fi section's order (`wifi-v0.4.md` 6), then wraps to
  the search field; Shift+Tab the reverse. Enter in 54 presses `Set`; Enter or Space on 55
  presses it; Space on 57 toggles. Showing the pane does not move the focus. The ring is the
  toolkit's (2 px `focus.ring`, inset 2).

## 4. The status line (widget 58)

| When | Text | Colour |
|------|------|--------|
| rest, jumbo frames and wake | `MTU from 68 to 9000 bytes on this adapter; 1500 is the standard.` | `text.dim` |
| rest, `max_mtu` 1500 | `MTU from 68 to 1500 bytes on this adapter, which takes no jumbo frames.` | `text.dim` |
| rest, the adapter cannot wake | the sentence above, then ` It cannot wake the computer.` | `text.dim` |
| applying | `Setting MTU 9000...` / `Turning Wake-on-LAN on...` / `...off...` | `text` |
| done | `MTU set to 9000.` / `Wake-on-LAN on: a magic packet wakes this computer from sleep or power-off.` / `Wake-on-LAN off.` | `text` |
| not digits | `Not a number. Type the MTU in bytes, 68 to 9000.` | `text` |
| out of range | `The MTU must be 68 to 9000 on this adapter.` | `text` |
| `EPERM` | `Not allowed: this user may not change the adapter.` | `text` |
| `EINVAL` (the adapter does not take that value) | `The adapter does not take MTU 9000.` | `text` |
| `EOPNOTSUPP` | `The adapter cannot do it.` (MTU) / `This adapter cannot wake the computer.` (wake) | `text` |
| `ENODEV` | `No such adapter any more.` | `text` |
| any other errno | `Could not set the MTU: <errno words>.` / `Could not change Wake-on-LAN: <errno words>.` | `text` |

The errno rows are `ip`'s `link_refused` reasons in the pane's voice (one table, in the
shared crate, used by `ip`, `settings set` and the pane, so the three say the same thing;
`ip` keeps its `cannot set mtu of "eth0": <why>` shape). A result or error stays until the
next action, a fact change that rewrites the rest sentence, or the pane is shown again
(which puts the rest sentence back: a stale result is never kept, as date-time's). Numbers in
the sentences are the adapter's (`<max>`) and the typed one (`<n>`). No colour beyond
`text` and `text.dim`: the words tell success from failure. The widest line (the wake
sentence, about 410 px) fits the 432 width.

## 5. CLI, MCP and agents (D22)

- `settings list` gains two writable keys beside v0.3's read-only ones: `network.mtu`
  (`settings set network.mtu 9000`, dry run `would set network.mtu from 1500 to 9000`,
  errors in section 4's sentences: `settings: network.mtu: the MTU must be 68 to 9000 on
  this adapter`) and `network.wake_on_lan` (`on` or `off`; `invalid value 'x' (on or off)`),
  plus read-only `network.max_mtu`. They call the same ioctls as `ip link set`.
- `net_link_set` is unchanged: it **asks the user first** (D708's `confirm`), because an agent
  changing the link is not the person pressing `Set`; the pane asks nothing. `net_status`'s
  `mtu`, `max_mtu` and `wake_on_lan` are the pane's facts.
- The element tree (`wm show`): `label` `MTU`; `field` `MTU` with value the text (`set-text
  "9000"`); `button` `Set` (`invoke`); `checkbox` `Wake-on-LAN: magic packet`, checked or
  not, `disabled` when the adapter cannot wake; `label` the status line's text. A screen
  reader reads the status line's new text after `Set` or the checkbox acts, as it reads
  date-time's. The GUI route for an agent (`set-text` then `invoke`) runs under the Settings
  window's grant, not `net_link_set`'s confirmation: the two routes keep their own rules.

## 6. Every device class (D23)

- **Phone** (pane page, `C` = `W` - 32; `settings-forms-v0.5.md` section 2): after the
  traffic lines (R9), `MTU` label cell top `r`; the field (16, `r` + 24, `C`, 48); `Set` (16,
  `r` + 80, `C`, 48) (R2 then R3's stacked button); `Wake-on-LAN: magic packet` as one
  checkbox row (16, `r` + 144, `C`, 48) whose whole width is the target (R8); the status line
  at cell top `r` + 208 (R9, wrapping to two lines when wider than `C`); the Wi-Fi section
  follows 16 under it. The field takes the numeric on-screen keyboard; Enter on it is `Set`.
- **Tablet**: the desktop layout with 40 px targets (the field and `Set` 40 tall at `+ 0`
  of the row, the checkbox's row 40), as every pane; a narrow split side uses the phone's.
- **TV** (scale 2, D-pad): the desktop layout; the field takes digits from the remote's
  number keys and the on-screen keyboard, OK on `Set` applies, OK on the checkbox toggles; the
  focus ring always drawn; the focus tooltip repeats a disabled checkbox's reason.
- **Wearable**: read-only, two facts lines after the traffic lines: `MTU 1500` and
  `Wake-on-LAN off` (or `on`, or `not available`), with the pane's standing line on where to
  change them.
- **2x**: every number doubles.

## 7. Acceptance (host tests, 640x440, scale 1x, pane coordinates; facts as `network-v0.3.md` 6's plus `max_mtu` 9000 and `wake` `no-wake`)

- Widgets 53 to 58 at their places and texts: `MTU` (16, 356); the field (272, 350, 72, 28)
  reading `1500`; `Set` (352, 350, 96, 28) disabled; `Wake-on-LAN` (16, 396); the checkbox at
  (288, 390), unticked, enabled; the line at (16, 436) reading the jumbo-and-wake sentence in
  `text.dim`; `Wi-Fi` at (16, 476) and its first row at y 524. `max_mtu` 1500 gives the
  no-jumbo sentence; `wake` `-` disables the checkbox, greys 56 and appends the cannot-wake
  sentence. No adapter: 53 to 58 hidden, `Wi-Fi` at (16, 356).
- Typing `9000` enables `Set`; Enter sends `SIOCSIFMTU` 9000 for `eth0` (the test's system
  records it), disables 54 and 55 and shows `Setting MTU 9000...` in `text`; the next tick
  with `mtu` 9000 shows `MTU set to 9000.`, the field `9000` enabled, `Set` disabled, widget
  13 `· MTU 9000`. `abc` shows the not-a-number sentence and sends nothing; `9001` the range
  sentence; `1500` leaves `Set` disabled; Esc empties then refills `1500`. `EPERM`, `EINVAL`,
  `EOPNOTSUPP` and `ENODEV` from the ioctl give section 4's sentences with the text kept and
  `Set` enabled; a tick while the field is focused with new text does not overwrite it; a
  tick with `mtu` 1400 while the field rests at `1500` shows `1400`.
- A press on the checkbox sends `ETHTOOL_SWOL` magic, shows the box ticked and disabled with
  `Turning Wake-on-LAN on...`; the tick with `wake` confirms the on sentence and enables the
  box; an `EOPNOTSUPP` answer unticks it and shows the cannot-wake sentence.
- Tab order from the sidebar list: field, `Set`, checkbox, then the Wi-Fi section's; with the
  checkbox disabled it is skipped. `wm show` lists the six elements with the names of section
  5; `set-text "9000"` then `invoke` `Set` applies as Enter does.
- `settings set network.mtu 9000` and `--dry-run` print section 5's lines; `settings set
  network.mtu 70000` exits 1 with the range sentence; `network.wake_on_lan maybe` the choices
  line; `settings get network.max_mtu` prints `9000`.
- Phone 390x752: the field (16, `r` + 24, 358, 48), `Set` under it, the checkbox row, the
  status line two lines at `C` 358; tablet 1024x660 the 40 px targets; TV 960x540 at scale 2
  the ring on the field; wearable 320x320 the two facts lines.
- QEMU (all three architectures, the boot test): `settings set network.mtu 9000` then `ip
  link` prints `mtu 9000` and `net_status` reports `mtu` 9000; a screendump of the pane
  shows `9000` in the field (it followed the facts) and the rest sentence on the status
  line (the change came from outside the pane, so no result is shown).

## Decided by design (the owner can overrule any of these)

1. A field and `Set`, not a stepper or a presets dropdown: the number is typed once, as
   date-time's clock and GNOME's MTU are, and `Set` is what an agent can `invoke`.
2. `Set` is a plain button, not the window's default; Enter in the field presses it.
3. The checkbox applies at once and needs no confirmation; `net_link_set` keeps D708's.
4. The field follows the facts only while it rests and shows what the pane put there.
5. Errors are errno words in the pane's voice, shared with `ip` and `settings` as one table.
6. The rows show whenever `eth0` exists; the kernel's `max_mtu` and `wake` decide what each
   allows, not the adapter's kind.
7. The Wi-Fi section's origin is chosen once per boot (476 with an adapter, 356 without).
8. `settings` gains `network.mtu`, `network.wake_on_lan` and read-only `network.max_mtu`.

## Left out

Wake on other patterns (D708's "not yet"), a per-adapter list when the machine has several
adapters (D708's single `eth0` holds), the kernel's refusal reason (`LinkError::Refused`'s
sentence is lost at the ioctl's errno; a backend ask: carry it in a readable place, such as
`/proc/net/interfaces`'s next line, so the pane can print `the RTL8139 takes no jumbo
frames` instead of `does not take MTU 9000`), and Wi-Fi power settings.

## Decisions for the engineer

1. Widgets 53 to 58 at section 1's places; `NetworkPane::WIDGETS` 58; `CAPACITY` + 6; the
   Wi-Fi section's `T(r)` = `524 + 40 r` with an adapter, v0.4's without.
2. Facts per tick: `mtu`, `max_mtu`, `wake` from the one parse; `MIN_MTU` 68; the field
   follows only when unfocused and unchanged; the checkbox follows unless in flight.
3. Validation order and sentences of sections 3 and 4; `SIOCSIFMTU` and `SIOCETHTOOL`
   `ETHTOOL_SWOL` on a socket as `ip`; the errno table in the shared crate, used by `ip`,
   `settings` and the pane.
4. In-flight states (`Setting...`, `Turning...`): controls disabled until the next tick
   confirms or the call fails.
5. Esc: `Escape::Clear`, then the pane refills the current MTU.
6. Tab order 54, 55, 57, then Wi-Fi's; element names of section 5.
7. `settings` keys `network.mtu`, `network.wake_on_lan`, `network.max_mtu`.
8. Device classes as section 6 through the reflow rules; the wearable's two lines.
