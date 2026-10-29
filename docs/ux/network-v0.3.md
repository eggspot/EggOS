# EggOS v0.3.0 Network: design brief (networking part 1)

Design by Claude Fable (D16) for networking part 1 (D157): a polled virtio-net driver, an IPv4
stack in the kernel (Ethernet, ARP, IPv4, ICMP echo, UDP), a DHCP client that configures the
one wired adapter at boot, and the DNS server DHCP gives (served as `/etc/resolv.conf`); `lo`
(127.0.0.1/8) always exists. The front-ends of this one capability (D22) are the CLI (`ip`,
`ping`, `nslookup`), the MCP tool `net_status`, and the two GUI parts this brief settles: the
**Network pane in Settings** and the **network indicator on the desktop's bar**. Everything in
`toolkit-v0.3.md` (section 11 for Settings), `desktop-v0.3-gui.md` sections 6, 10, 11, 14.1 and
`date-time-v0.3.md` stands; numbers are px at 1x and double at 2x; no motion is added.

The pane is read-only in this part: there are no network actions yet (no manual addresses, no
renew, no Wi-Fi, no IPv6, no proxies), so it draws no controls and no placeholders for them.
Only the wired adapter `eth0` is shown; `lo` is not a thing a user configures and stays in the
CLI and MCP output. What the GUI can show, it reads from `/proc/net/interfaces` (the kernel's
text file; the program parses it, as it parses `/proc/uptime` and `/proc/driver/rtc`).

## 0. The five states, named once

One vocabulary for the three front-ends. The GUI never invents a fourth word for a state.

| State | When | Pane value (section 2) | Bar (section 4) | `net_status` `state` | `ip` |
|-------|------|------------------------|-----------------|----------------------|------|
| no adapter | the machine has no virtio-net device (`--no-network`) | `No network adapter.` | not shown | `"none"` (no `eth0` object) | only `lo` is listed |
| cable unplugged | the adapter's link is down (virtio-net's link status bit) | `Cable unplugged` | the broken cable | `"cable-unplugged"` | `state DOWN` |
| getting an address | link up, DHCP `DISCOVER`/`REQUEST` sent and retrying with backoff | `Getting an address...` | three dots | `"getting-address"` | `state UP`, no `inet` line, `dhcp asking` |
| no address | link up, no DHCP server answered; the kernel retries every 60 s in the background | `No address` | `!` | `"no-address"` | `state UP`, no `inet` line, `dhcp no-address` |
| connected | link up and an address from DHCP | `Connected` | the full glyph | `"connected"` | `state UP`, `inet 10.0.2.15/24 ... dynamic`, `dhcp configured` |

`state` is one field of `net_status` beside the raw facts (`link: "up" | "down"`, `dhcp:
"asking" | "configured" | "no-address"`), so an agent gets the same summary a person sees. `ip
addr`'s lines keep Linux's shape (people and programs parse it; `state UP`/`DOWN` and `dynamic`
are Linux's own words) and carry the DHCP word at the end of the interface line as `dhcp
asking|configured|no-address`. A link that goes down after an address was given shows **cable
unplugged** everywhere: the address is not usable, and the DHCP state is kept behind it.

## 1. The pane in the sidebar

- **Name** `Network`. It is the **second** row, between `Appearance` and `Display`:
  `Appearance`, `Network`, `Display`, `Keyboard`, `Mouse`, `Date & time`, `Components`,
  `About`. Appearance stays first because it is the profile picker the first boot opens on;
  Network comes right after it because every desktop a switcher knows puts it near the top
  (Windows 11 third, macOS third, GNOME first) and above displays, keyboard and mouse; the
  hardware panes stay together after it and the two about the system itself stay last. Its row
  is y 76..107 of the sidebar (44 + 1 x 32), text at x 16. Every row after it moves down 32:
  `Display` 108, `Keyboard` 140, `Mouse` 172, `Date & time` 204, `Components` 236, `About` 268
  (the `Date & time` acceptance in `date-time-v0.3.md` section 7 reads 204 from now on, and
  `DATE_AND_TIME` is index 5, `ABOUT` 7).
- **Search words**: `Ethernet`, `wired`, `eth0`, `Wi-Fi`, `WiFi`, `wireless`, `internet`,
  `online`, `connection`, `LAN`, `cable`, `adapter`, `IP address`, `IPv4`, `DNS`, `DHCP`,
  `gateway`, `router`, `MAC address`, `hardware address`, `MTU`, `lease`, plus the pane's row
  names `IPv4 address`, `Router`, `DNS servers`, `DHCP server`, `Hardware address`, `Traffic`.
  `Wi-Fi` and `wireless` are there on purpose (README principle 3): someone looking for Wi-Fi
  must find the pane that tells them this machine has a wired adapter only. Note `mac` also
  matches Appearance (`macOS`); nothing else collides.
- **Budget**: the window's tree holds 48 widgets and all 48 are used (`date-time-v0.3.md`
  section 1). This pane is its container plus exactly 15 widgets (section 2), so `CAPACITY`
  becomes **64**. The installer's `eggos_app::dialog::CAPACITY` is asserted equal to Settings'
  (one tree type for both windows): raise both to 64.

## 2. Layout (pane coordinates: the pane's origin is window x 176, y 0; it is 464 wide at the
default size, so the right-aligned edge is x 448)

Rows 0..4 of the 40 px grid from y 64, text cell top at row + 12, as every pane; then three
body lines 20 apart, as About's. Every widget has one fixed place; a state shows or hides
widgets and changes their texts, it never moves them, so the layout fills in place as the
machine connects (section 3).

| # | Widget | Place | Text and colour | Shown in |
|---|--------|-------|-----------------|----------|
| 1 | heading | (16, 16), 2x | `Network` | always |
| 2 | label | (16, 76) | `Ethernet` in `text`; with no adapter, `No network adapter.` | always |
| 3 | label, right-aligned | ends at x 448, cell top 76 | the state value (section 0) in `text` | an adapter exists |
| 4 | label | (16, 116) | the explanation (below) in `text.dim` | every state but connected |
| 5 | label | (16, 116) | `IPv4 address` in `text` | connected |
| 6 | label, right-aligned | ends at 448, cell top 116 | `10.0.2.15/24 · mask 255.255.255.0` in `text` | connected |
| 7 | label | (16, 156) | `Router` | connected |
| 8 | label, right-aligned | ends at 448, cell top 156 | `10.0.2.2` | connected |
| 9 | label | (16, 196) | `DNS servers` | connected |
| 10 | label, right-aligned | ends at 448, cell top 196 | `10.0.2.3` (up to three, `, ` between, in DHCP's order) | connected |
| 11 | label | (16, 236) | `DHCP server` | connected |
| 12 | label, right-aligned | ends at 448, cell top 236 | `10.0.2.2 · 23 h 59 min of lease left` | connected |
| 13 | label | (16, 276) | `Hardware address 52:54:00:12:34:56 · MTU 1500` in `text` | an adapter exists |
| 14 | label | (16, 296) | `Received 3.4 MB (2,905 packets)` in `text` | an adapter exists |
| 15 | label | (16, 316) | `Sent 1.2 MB (812 packets)` in `text` | an adapter exists |

Widgets 4 and 5 share y 116 and are never shown together. The content ends at y 332, the pane's
height at the minimum window size, so nothing scrolls. The `·` is U+00B7, as About's.

**The explanation line (widget 4)**, one sentence a switcher understands, `text.dim` because it
is a hint under the value, not a fact of its own:

| State | Widget 2 | Widget 3 | Widget 4 |
|-------|----------|----------|----------|
| no adapter | `No network adapter.` | hidden | `EggOS found none at boot: there is nothing to set up.` |
| cable unplugged | `Ethernet` | `Cable unplugged` | `Plug in a network cable. EggOS connects by itself.` |
| getting an address | `Ethernet` | `Getting an address...` | `Asking the network for an address (DHCP).` |
| no address | `Ethernet` | `No address` | `No DHCP server answered. EggOS keeps trying.` |
| connected | `Ethernet` | `Connected` | hidden (rows 1-4 show instead) |

Three periods, not U+2026 (not in Egg Sans). The widest value, `Getting an address...`, is
104 px; the widest line, the no-adapter one, about 300 px: nothing meets anything.

**How the numbers are written** (one function each, shared with the CLI where the CLI prints
the same thing, D22):

- IPv4 addresses in dotted decimal without leading zeros; the prefix as `/24`; the subnet mask
  in dotted decimal after `· mask `. Hardware address: six lower-case hex pairs with `:`, as
  `ip` prints it. `MTU 1500` after `· `.
- Lease left, from the seconds `/proc/net/interfaces` gives, every unit floored: 1 day or more
  `<d> d <h> h` (`1 d 0 h`); 1 hour or more `<h> h <m> min` (`23 h 59 min` for 86342 s); 1
  minute or more `<m> min`; else `<s> s`. The DHCP row reads `<server> · <left> of lease left`;
  an infinite lease (0xFFFFFFFF, RFC 2132) reads `<server> · lease does not end`.
- Bytes as Files' sizes (`size_text` in `userland/files/src/listing.rs`: under 1024 `512 B`,
  then `KB`, `MB`, `GB` with one decimal below 10 and none from 10, 1024-based); move that
  function to the shared crate rather than copying it. Packets with thousands separators as
  Files' `bytes_text` (`2,905`), the word `packets` even for 1 (`1 packets` never happens in
  practice: the first DHCP exchange is several).
- Receive errors and drops, summed, are shown only when there are any, after the packets:
  `Received 3.4 MB (2,905 packets, 3 errors)`. Zero is the normal case and says nothing.

The widest realistic lines fit: three DNS servers of `255.255.255.255` are 257 px against the
66 px `DNS servers`; `Received 999.9 GB (99,999,999 packets, 9,999 errors)` is about 275 px of
the 432 available.

## 3. Live updates

The pane re-reads `/proc/net/interfaces` once a second while it is shown, in the tick that
refreshes About's uptime and Date & time's clock (`show_clock`), and when it is shown. Each
tick compares the new facts with the shown ones and sets only what changed: a state change
sets widgets 2-4 and shows or hides 4-12; a changed address, router, DNS or DHCP server its
value; the lease and the two traffic lines change almost every tick and damage their own rects
only (as About's uptime line does). Nothing is read or redrawn while another pane shows.

So a user who opens Settings on a fresh boot sees `Getting an address...` with its hint, then,
within a few seconds, `Connected` and rows 1-4 filling in below it in the same frame, the
hint gone. Pulling the cable (`set_link eth0 off` in QEMU's monitor) turns the value into
`Cable unplugged` and clears rows 1-4 in one tick; plugging it back brings them back once DHCP
has answered again. The hardware and traffic lines never move; the received line counting up
is how a person sees the network is alive.

## 4. The bar indicator

A 16x16 glyph in a hit target on each bar, next to the power button on the top bar and next
to the clock on the taskbar, where the network icon sits on GNOME's top bar and Windows'
taskbar; the Agent chip (14.1) moves left of it. Whether there is an adapter is known at boot
and never changes, so the indicator never appears or disappears while the desktop runs and
nothing beside it ever shifts. Token names are the code's `colour::` constants.

### 4.1 Place

- **Top bar (macOS and Linux profiles)**: a 28x28 hit target directly left of the power
  button, `Rect::new(power_button().x - 28, bar.y, 28, 28)`; at 1280 that is (1216, 0, 28,
  28). The glyph at the target's (6, 6): (1222, 6). The Agent chip's right edge is 8 px left of
  the target when the indicator shows, `power_button().x - 8` as today when it does not: with
  both, the chip is (1152, 4, 56, 20), text at (1160, 6). The clock, the Linux chips and the
  macOS menu titles are unchanged (they stop at the clock's left - 16).
- **Taskbar (Windows profile)**: a 40x40 item whose right edge is 8 px left of the clock's
  rect, `Rect::new(clock_rect().x - 8 - 40, bar.y, 40, 40)`; at 1280 (1096, 760, 40, 40), the
  glyph at the item's (12, 12): (1108, 772), the same row as the clock's text. The Agent chip's
  right edge is 8 px left of the item when the indicator shows: (1032, 770, 56, 20), text at
  (1040, 772); without it, `clock_rect().x - 16` as today. The taskbar items' room (6.4 item
  13) ends 16 px before the leftmost of the chip, the indicator and the clock.
- Any width (D23): the place is computed from the power button or the clock, so a 360 px
  phone screen has it; at scale 2 the target and the glyph pixel-double (32x32 glyph).

### 4.2 The glyph

Two outlined 9x6 boxes, the machine (top-left) and the network (bottom-right), joined by a
1 px cable, drawn with `fill` and `row` from a table of rectangles (no bitmap): the outline of
box A is rows 0 and 5 across x 0..8 and columns 0 and 8 across rows 1..4; box B the same at
x 7..15, rows 10..15; the cable is (4, 6), (4, 7), row 8 across x 4..11, (11, 9). Pixels
relative to the glyph's 16x16 box, `X` = `TEXT`, `d` = `TEXT_DIM`, `a` = `ACCENT`, `.` =
the bar.

**Connected**, everything `TEXT`:

```
XXXXXXXXX.......
X.......X.......
X.......X.......
X.......X.......
X.......X.......
XXXXXXXXX.......
....X...........
....X...........
....XXXXXXXX....
...........X....
.......XXXXXXXXX
.......X.......X
.......X.......X
.......X.......X
.......X.......X
.......XXXXXXXXX
```

**Getting an address**: box A and the cable `TEXT`, box B `TEXT_DIM` (not reached yet), three
`ACCENT` dots inside it at (9, 12), (11, 12), (13, 12), the pane's `...`:

```
XXXXXXXXX.......
X.......X.......
X.......X.......
X.......X.......
X.......X.......
XXXXXXXXX.......
....X...........
....X...........
....XXXXXXXX....
...........X....
.......ddddddddd
.......d.......d
.......d.a.a.a.d
.......d.......d
.......d.......d
.......ddddddddd
```

**No address**: as getting an address, but inside box B an `ACCENT` `!`: (11, 11), (11, 12),
then (11, 14):

```
.......ddddddddd
.......d...a...d
.......d...a...d
.......d.......d
.......d...a...d
.......ddddddddd
```

**Cable unplugged**: box A `TEXT`, the cable broken off after its stub, (4, 6), (4, 7),
(4, 8), (5, 8) in `TEXT`, nothing on rows 8-9 beyond x 5, box B `TEXT_DIM` and empty:

```
XXXXXXXXX.......
X.......X.......
X.......X.......
X.......X.......
X.......X.......
XXXXXXXXX.......
....X...........
....X...........
....XX..........
................
.......ddddddddd
.......d.......d
.......d.......d
.......d.......d
.......d.......d
.......ddddddddd
```

No state is told by colour alone: the shape differs in each (a full cable, dots, `!`, a cut
cable), and the dim box only adds to it. Contrast: `TEXT` on `BAR` 14.3:1, `TEXT_DIM` on `BAR`
5.7:1, `ACCENT` on `BAR` 9.4:1.

### 4.3 States, hover, press

- **No adapter**: nothing is drawn, no hit target, no `PanelItem`; the chip and the taskbar
  items are where they are today.
- **Hover and pressed** follow each bar's own items: on the top bar no hover fill and
  `BAR_PRESSED` while pressed, over the target's rows 0..26 (the edge line stays), as the power
  button; on the taskbar `BAR_HOVER` on hover and `BAR_PRESSED` while pressed over rows 1..39,
  as every taskbar item. Pointer `Arrow`.
- **Updates**: the kernel tells the desktop on each change (`Desktop::set_network(state)`,
  with the address for the tooltip), which damages the glyph's rect and, if a tooltip shows,
  re-places and redraws it with the new text (old and new rects damaged). No tick is needed:
  the indicator has nothing timed.

### 4.4 Tooltip

After 600 ms of rest, the panel tooltip's chrome (6.4 item 3): below the top bar at y 32,
above the taskbar at y 732, centred on the target and clamped 8 px inside the screen. The bar
font is Egg Mono (8 px per glyph), so the width is glyphs x 8 + 16.

| State | Text | Glyphs | Top bar rect at 1280 | Taskbar rect at 1280 |
|-------|------|--------|----------------------|----------------------|
| cable unplugged | `Ethernet: cable unplugged` | 25 | (1056, 32, 216, 24) | (1008, 732, 216, 24) |
| getting an address | `Ethernet: getting an address...` | 31 | (1008, 32, 264, 24) | (984, 732, 264, 24) |
| no address | `Ethernet: no address. EggOS keeps trying.` | 41 | (928, 32, 344, 24) | (928, 732, 344, 24) |
| connected | `Ethernet: connected, 10.0.2.15` | 30 | (1016, 32, 256, 24) | (988, 732, 256, 24) |

The connected tooltip carries the address because that is what people hover a network icon
for. The text is also the item's **accessible name** (the role is a button; the screen reader
of README's baseline reads the tooltip text, so the name and the tip never disagree).

### 4.5 Click

A click (`PanelItem::Network`) opens Settings on the Network pane. The clock has no click yet
(`date-time-v0.3.md` section 6 left it for later), so this is the first bar item that opens a
pane, and it does it through the CLI so nothing is GUI-only (D22): a new `settings open
<pane>` form of the `settings` command opens the window with that pane current (`open
network`; the other pane names in lower case work too, `open date-and-time` with the `&`
written out as `and`); with no arguments `settings` still opens on Appearance. The desktop
returns `Action::OpenSettingsPane(SettingsPane::Network)` and the kernel starts
`/bin/settings` with the arguments `open` and `network` (`Command::program`), under the same
2 s `starting` guard as `Action::Start(AppId::Settings)`. When a Settings window already
exists, the click focuses it (restoring it if minimised), as the dock's Settings icon does,
and the pane stays what it was: there is no channel to tell a running program to switch panes
in 0.3.0, so this is listed as a limit, not hidden. The keyboard way to the same pane is the
launcher (Super, `settings`, Enter) and the sidebar search (`network`); the bar's items take no
keys (6.1), and that is unchanged.

## 5. The CLI and MCP mirror (D22)

- `ip addr` (and `ip link`, `ip route`, `ip neigh`) print Linux's shapes with the state words
  of section 0; `ip -s link` prints the counters the pane's traffic lines show, from the same
  `/proc/net/interfaces` numbers. `ping` and `nslookup` are unchanged by this brief.
- `net_status` returns, per adapter, `name`, `state` (section 0), `link`, `dhcp`, `mac`,
  `mtu`, `address` (with `prefix` and `mask`), `router`, `dns` (an array), `dhcp_server`,
  `lease_left_s`, `lease_s`, and the counters `rx_bytes`, `rx_packets`, `rx_errors`,
  `tx_bytes`, `tx_packets`; with no adapter, `adapters` holds only `lo`.
- `settings list` gains read-only keys `network.state` (section 0's `net_status` words),
  `network.address`, `network.router`, `network.dns`, `network.mac`, so the GUI's rows and the
  CLI's values come from one parse of the same file; `set` on them answers `read-only`.
- One sentence table (section 0 and section 2) is the source of every word; the pane, the
  tooltip, `net_status` and `ip` are tested against it.

## 6. Acceptance (host tests, default 640x440, scale 1x; the bar at 1280x800)

The example facts: link up, DHCP configured, hardware address `52:54:00:12:34:56`, MTU 1500,
`10.0.2.15/24`, router `10.0.2.2`, one DNS server `10.0.2.3`, DHCP server `10.0.2.2`, lease
86342 of 86400 s left, received 3,600,000 bytes in 2,905 packets with 0 errors, sent 1,300,000
bytes in 812 packets.

- `PANES` is `Appearance, Network, Display, Keyboard, Mouse, Date & time, Components, About`;
  the sidebar row of `Network` is (0, 76, 176, 32) and of `Date & time` (0, 204, 176, 32);
  `ethernet`, `wi-fi`, `dns`, `dhcp`, `ip address`, `router`, `mtu` and `lease` each match
  Network and nothing else; `mac` matches Network and Appearance.
- The tree holds 64 widgets. In window coordinates with the example facts: heading at
  (192, 16); `Ethernet` at (192, 76); the value's right edge at 624, cell top 76, text
  `Connected`; `IPv4 address` at (192, 116) with `10.0.2.15/24 · mask 255.255.255.0` ending at
  624; `Router` / `10.0.2.2` at 156; `DNS servers` / `10.0.2.3` at 196; `DHCP server` /
  `10.0.2.2 · 23 h 59 min of lease left` at 236; `Hardware address 52:54:00:12:34:56 · MTU
  1500` at (192, 276); `Received 3.4 MB (2,905 packets)` at (192, 296); `Sent 1.2 MB (812
  packets)` at (192, 316); widget 4 hidden.
- Link down with the same facts: the value reads `Cable unplugged`, widget 4 at (192, 116)
  reads `Plug in a network cable. EggOS connects by itself.`, widgets 5-12 are hidden, 13-15
  shown. DHCP asking: `Getting an address...` and `Asking the network for an address (DHCP).`.
  DHCP no-address: `No address` and `No DHCP server answered. EggOS keeps trying.`. No adapter:
  widget 2 reads `No network adapter.`, widget 3 hidden, widget 4 reads `EggOS found none at
  boot: there is nothing to set up.`, widgets 5-15 hidden.
- Three DNS servers read `10.0.2.3, 10.0.2.4, 10.0.2.5`; 3 receive errors read `Received 3.4
  MB (2,905 packets, 3 errors)`; lease left 90061 reads `1 d 1 h`, 3599 `59 min`, 59 `59 s`,
  0xFFFFFFFF of 0xFFFFFFFF `lease does not end`.
- Showing the pane with the asking facts, then ticking with the connected facts: the value,
  widget 4's visibility and rows 1-4 change in that tick; ticking again with only the received
  counter changed damages widget 14's rect alone.
- The bar, macOS and Linux profiles: with an adapter the indicator's target is (1216, 0, 28,
  28), the glyph's pixels at (1222, 6) match the state's map (hash all four), the Agent chip
  is (1152, 4, 56, 20) while an MCP client is connected; without an adapter `panel_item_at`
  finds nothing there and the chip is (1180, 4, 56, 20). Windows profile: the item (1096, 760,
  40, 40), glyph at (1108, 772), the chip (1032, 770, 56, 20), the taskbar items' limit 1080
  with no chip; without an adapter the chip is (1072, 770, 56, 20).
- Resting the pointer on (1230, 14) for 700 ms in the connected state shows the tooltip
  `Ethernet: connected, 10.0.2.15` at (1016, 32, 256, 24); the state changing to cable
  unplugged while it shows re-places it at (1056, 32, 216, 24) with the new text; on the
  taskbar, resting on (1116, 780) gives (988, 732, 256, 24).
- A button-down on the target returns `Action::OpenSettingsPane(SettingsPane::Network)` when
  no Settings window exists and `Action::None` with the Settings window focused when one does;
  `/bin/settings open network` opens the window on the Network pane (`pane()` is `NETWORK`),
  `settings open nowhere` prints the usage line and exits 2.
- QEMU, all three architectures, in the boot test: the first prompt's screendump shows the
  indicator in the getting-an-address or connected state; a screendump 5 s later shows it
  connected and `ip addr` prints `inet 10.0.2.15/24`; a boot with `--no-network` shows no
  indicator and the chip at (1180, 4). `net_status` and `settings get network.state` print
  `connected`.
