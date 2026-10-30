# EggOS v0.3.0 Devices in About: design brief

Design by Claude Fable (D16) for the GUI front-end of the `devices` capability (D22): the
list of the machine's devices that `device list` prints and the MCP tool `device_list`
returns, shown in Settings' `About` pane. Everything in `toolkit-v0.3.md` (section 11 for
Settings), `desktop-v0.3-gui.md` section 10 and `network-v0.3.md` stands; numbers are px at 1x
and double at 2x; no motion is added. The data is the kernel's `/proc/driver/devices`, one
line a device (id, parent, bus, class, name, description, driver, state, delivery, reason);
the program parses it as it parses `/proc/net/interfaces`. Read-only: no action touches a
device in this slice.

## 0. Where it goes: a `Devices` section at the bottom of About, with `Show all devices`

About stays the one place "about this machine", as About This Mac, Windows' System > About
and GNOME's About are; a person who wants to know what hardware EggOS found looks there
first. The facts (egg, name, architecture, CPUs, memory, uptime, licence) end at y 160 and
the rest of the pane is empty, 172 px at the smallest window and more at any other, which is
room for a table that scrolls; a table carries 8 or 50 rows equally well. The toggle keeps the
default view short (what a person recognises) and gives a technical user every device in one
click, as Device Manager's `Show hidden devices` does. Not chosen: a separate drill-in
(`System Report...`), because Settings has no second-level navigation and no back control,
and inventing one for a read-only list is a new concept for one pane; a `Devices` sidebar
pane, because devices are not settings and the sidebar lists what a user changes.

## 1. The rows: one rule for the pane, the CLI and the MCP tool

- **Default** (`device list`, `device_list` with `all` false, the pane with the box
  unticked): every device whose class is one a person recognises (display, input, disk,
  network, console, serial, clock, entropy) and that no other device names as its `parent`,
  plus every `refused` device whatever its class. So the virtio children show and their
  transports do not (a transport is plumbing: its child is the device), empty virtio-mmio
  slots (class bus, unbound) and interrupt controllers do not, an undriven SATA controller
  does (`No driver` is what someone came to find out), and a refusal is never hidden.
- **All** (`device list -a`, `all` true, the box ticked): every line of the file.
- **Order**, both views: by kind in the order of the table below, then by id (discovery
  order). No tree, no group header rows: the `Kind` column is the grouping. The parent is in
  `device list -a` and `device_list`.
- One predicate and one sort, in the shared crate the CLI and the MCP tool use (D22); the pane
  shows the same rows in the same order with the same words. The CLI prints the technical
  columns too (id, parent, bus, name, delivery); the pane leaves those to it.

| class | Kind | class | Kind |
|-------|------|-------|------|
| display | `Display` | clock | `Clock` |
| input | `Input` | entropy | `Random numbers` |
| disk | `Disk` | bus | `Bus` |
| network | `Network` | interrupt-controller | `Interrupt controller` |
| console | `Console` | other | `Other` |
| serial | `Serial port` | | |

**The three columns' words** (one function each, shared with the CLI where it prints the
same): `Kind` from the table; `Device` is the description for a virtio device (`virtio gpu`:
its name `gpu` would say it twice) and `<name> · <description>` for the others
(`00:1f.2 · 8086:2922 class 0106`, `pl031@9010000 · arm,pl031`; the `·` is U+00B7);
`Driver` is the driver's name when bound (`virtio-gpu`), `No driver` when unbound, and
`Refused: <reason>` when refused, ellipsised at the column's end (the whole reason is in
the row's element value, section 5, and in `device list`). No colour tells a state apart: the
word does (toolkit section 2: no third text colour).

## 2. Layout (pane coordinates: origin window x 176, y 0; 464 wide at the default size, so
the right-aligned edge is x 448; the pane's bottom edge is the window's content height)

| # | Widget | Place | Text and colour |
|---|--------|-------|-----------------|
| 1-5 | the facts as today | egg (16, 64); heading (80, 64); lines at y 104, 124, 144, x 80 | unchanged |
| 6 | label | (16, 182) | `Devices` in `text.dim` (a section heading inside a pane, as Files' `Places`) |
| 7 | checkbox, right-aligned | its right edge at 448, y 176, 28 tall | `Show all <n> devices` (`<n>` the file's line count); unticked when the pane opens; disabled when the default view already shows every device |
| 8 | table (section 3) | (16, 212) to (448, bottom - 16) | the rows of section 1 |

The facts end at y 160 and the section starts 16 below, its heading's cell top level with
the checkbox label's (control y + 6). The table starts 8 px under the checkbox row and takes
the rest of the pane: 212 tall at the default 440 window (the header and 7 rows, 20 px
spare), 132 at the 360 minimum (the header and 4 rows). It grows with the window; nothing
else in About moves. The checkbox's label is the widest text here, `Show all 50 devices`,
about 120 px, well right of `Devices`.

## 3. The table

A `Table` is a list of section 6 of the toolkit with a header row and fixed columns, rows
supplied by the program (the toolkit's `List` holds 32 static strings, which cannot show 50
devices read at boot; "Implementation notes"). Same surface, states, mark, selection,
scrolling and keys as a list; only what follows is new.

- **Header**: the first 24 rows, not scrolled, not selectable: the column names in `text.dim`
  at the columns' x with the cell top at +4, and a 1 px `bar.edge` line on its row 23, as
  Files' header. No sorting by clicking (later).
- **Rows**: 24 tall from y 24, cell top +4, `>` at x + 8 on the selected row, text from
  x + 24. Columns, with `w` the table's width minus the 8 px the scroll bar reserves (424 at
  the default size): `Kind` at x + 24, 104 wide; `Device` at x + 136, `w - 136 - 120` wide
  (168 at the default size), it takes every extra pixel of a wider window; `Driver` at
  x + w - 112, 112 wide, so it ends 8 px before the bar. 8 px between columns is the gap
  the ellipsis rule leaves. Every cell is one line in `text`, ellipsised at its column's end
  (`Interrupt controller`, 112 px, is the widest `Kind` and the only one that touches its
  edge). Digits are tabular, so PCI addresses align.
- **States**: `menu.hover` on hover, `selection` with the `>` in `accent` on the selected
  row, the ring at inset 0 while focused, exactly as a list; one row is always selected when
  there are rows (the first, by default). Selection is a reading position: it does nothing,
  nothing opens.
- **Scrolling**: the bar of toolkit section 8 along the rows' right edge (not the header's),
  wheel 72 px = 3 rows, the view scrolls to keep the selection visible.
- **Empty text** at x + 16 of the first row in `text.dim`: `No devices to show (tick Show
  all devices).` in the default view, `EggOS found no devices.` in the all view, and `The
  device list could not be read.` when the file cannot be read (then the checkbox is
  disabled and its count reads `Show all devices`).
- **Narrow rows** (D23): when the table is narrower than 400 px, or the input is touch, the
  header goes, rows are 40 tall with two lines, `Device` at cell top +4 in `text` and `<Kind>
  · <Driver>` at +20 in `text.dim` (`Display · virtio-gpu`, `Disk · No driver`), the `>` at
  x + 8 with its cell top at +12, and the wheel scrolls 120 px (3 rows). A phone shows the
  same rows one under the other, the way its own settings lists do; 40 px is the touch target
  of section 12 of `desktop-v0.3-gui.md`. On a TV (scale 2) everything doubles and the ring
  on the selected row is the cue; arrows move it. Keyboard-only on every form factor: the
  table is one Tab stop and never needs a pointer.

**Example, aarch64 virt, default view** (the Driver column in its place):

```
Kind            Device                                     Driver
Display         virtio gpu                                 virtio-gpu
Input           virtio input                               virtio-input
Input           virtio input                               virtio-input
Disk            virtio block                               virtio-blk
Network         virtio network                             virtio-net
Console         virtio console                             virtio-console
Serial port     pl011@9000000 · arm,pl011                  No driver
Clock           pl031@9010000 · arm,pl031                  pl031
Random numbers  virtio entropy                             virtio-rng
```

The two `Input` rows read the same because the file does not say which is the keyboard and
which the tablet; a limitation to list, not to paper over in the GUI.

## 4. Keys and focus

- Tab from the sidebar list enters the pane at the checkbox, then the table, then wraps to
  the search field; Shift+Tab the reverse. Showing the pane does not move the focus.
- On the checkbox: Space or a click toggles; the rows are re-filtered in the same frame; the
  selected device stays selected if it is still shown (the view scrolls to it), else the
  first row is selected and the view is at the top. The state is not saved: Settings opens
  with the box unticked.
- In the table: Up/Down one row, Home/End the first and last, PageUp/PageDown the visible
  rows minus one; Enter, Space and a double-click do nothing (there is no open action); Esc
  goes to the window, which ignores it. Arrows in the header do nothing because the header
  is not a row.
- Pointer: `Arrow` everywhere; a click selects and focuses the table; a right-click does
  nothing (no context menu in this slice).

## 5. What an agent sees (D161, `wm show`)

The toolkit describes the pane as it does today: the table is a `list` named `Devices`, its
shown rows `list item`s in the shown order, each named by its `Device` text with the value
`<class> <state> <driver>` from the file's own words, the reason appended after `: ` when
refused (`display bound virtio-gpu`, `disk unbound -`, `bus refused virtio-mmio: version 1
device`), the selected one flagged `selected`; the checkbox a `checkbox` named `Show all 41
devices` with `checked` when ticked; the section heading a `label` `Devices`; the facts the
labels and graphic they are now. `wm act <id> <checkbox> invoke` toggles the view as Space
does and `select` on a row selects it. An agent that wants the records uses `device_list`,
not the tree (D22: the tree is for operating the window).

## 6. Search, data and parity (D22)

- **Search words** added to About's: `devices`, `hardware`, `driver`, `drivers`, `Device
  Manager`, `System Information`, `System Report`, `PCI`, `virtio`, `GPU`, `graphics`,
  `disk`, `storage` (README principle 3: each platform's name for the place). `hardware` also
  matches Date & time (`hardware clock`) and `disk` nothing else; no other collision.
- **When it is read**: once each time the pane is shown, not on a tick; the device table is
  made at boot and does not change while the system runs (no hot-plug in 0.3.0). The checkbox
  re-filters what was read; it does not re-read.
- **Parity**: `device list` prints the default rows, `device list -a` every row, `--json` the
  records (`id`, `parent`, `bus`, `class`, `name`, `description`, `driver`, `state`,
  `delivery`, `reason`, with `null` for `-`); `device_list` (effect `read`, no grant, as `net_status`: D184)
  takes `all` and returns the same records with the same filter and order. One predicate, one
  sort and one set of words serve the three; the host test renders the pane from the same
  example file the CLI test prints and checks row for row.
- **No settings keys**: devices are facts, not settings; `settings list` is unchanged.

## 7. Acceptance (host tests, default 640x440, scale 1x, an example file of 41 lines in which
9 pass the default filter)

- `about` matches `device manager`, `system information`, `driver` and `virtio` and nothing
  else does; `hardware` matches About and Date & time.
- In window coordinates: `Devices` at (192, 182) in `text.dim`; the checkbox's rect ends at
  624, y 176, 28 tall, label `Show all 41 devices`, unticked, enabled; the table at
  (192, 212, 432, 212): header texts `Kind` at x 216, `Device` at 328, `Driver` at 504, the
  `bar.edge` line at y 235; the first row's `Kind` at (216, 240), 9 rows, the first selected
  with the `>` at x 200; row texts as the example of section 3 in that order.
- Ticking the box shows 41 rows, the bar appears, the row that was selected stays selected,
  the label is unchanged; unticking returns to 9 with the first row selected when the
  selected device is hidden.
- At 640x360 the table is (192, 212, 432, 132) and shows the header and 4 rows; PageDown
  moves the selection 3 rows; End scrolls to the last row.
- The element tree holds a list `Devices` with 9 list items, the first `virtio gpu` with value
  `display bound virtio-gpu` and `selected`, a row `00:1f.2 · 8086:2922 class 0106` with value
  `disk unbound -` on the x86_64 example, and a checkbox `Show all 41 devices`.
- A table 360 wide draws no header, 40 px rows, `Display · virtio-gpu` at +20 in `text.dim`.
- A file that cannot be read shows `The device list could not be read.`, the checkbox
  disabled and labelled `Show all devices`.
- `device list`, `device_list` and the pane agree on the 9 rows and their order from the
  same example file; `-a` and `all: true` on the 41.

## Implementation notes

- **One new toolkit widget, `Table`** (`Kind::Table`): a focusable list with a header row,
  fixed columns (`&'static [(name, width)]`, the flexible one marked), rows beyond 32 whose
  texts the program supplies (a cell function `fn(data: u32, row, column, &mut Text<64>)` and
  a row count, the way `Custom` and `Cards` take a painter), a selected row, an offset, the
  narrow mode of section 3, and the list's states, keys, bar and empty text. Its element
  description is `list` and `list item`, the item's name the flexible column's text and its
  value the program's (`value` function alongside `cell`). Column resizing and sorting by
  header stay in the toolkit's "Later".
- **Settings**: +3 widgets (label, checkbox, table) on what the tree holds when this lands
  (48 today, 64 with Network's pane), and the installer's `CAPACITY` stays equal to it. The
  About pane's panel takes the window's height (`Length::Fill`) instead of `PANE_HEIGHT`, so
  the table reaches `bottom - 16`; the other panes are unchanged. The records (at most 64,
  the file's lines past that dropped and the count still reported) live in `Facts`, read when
  About is shown.
- **Shared**: the filter predicate, the kind order and the three column words in the crate
  `device list` and `device_list` use, so the pane cannot drift from them.
- **Limits to list** (`docs/limitations.md`): two virtio input devices read alike; the
  console UART and the interrupt controller set up before the driver model read `No driver`
  until they join it; no hot-plug, so the list is as of boot; no tree view.
