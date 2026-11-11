# EggOS v0.5 Components: updates, the store download and live progress: design brief

Status: design for v0.5; D293 (updates), D263's and D235's open deviations.

Design by Claude Fable (D16) for the next version of Settings > Components. It extends
`components-v0.3.md` and `editions-v0.4.md`, and everything there stands unless a line here
changes it: the five states, the store lines, the rows, the note's forms, one action at a time,
the refusals and failures, the tokens and the contrast table. It adds an **Updates band**, an
`Update` button on a row, the store **download** from the pane, the edition confirmation where
v0.4 drew it, the touch and TV layouts, and progress that moves while a component is copied.
Nothing here is GUI-only (D22): section 9 names the command and the MCP tool behind every button.
Numbers are px at 1x in pane coordinates (origin window x 176, y 0; 464 wide, right edge x 448);
the only motion is the progress bar following bytes.

## 1. Layout: what moves, and the new widgets

Widgets 1 to 49 keep their numbers (40-43 the Edition band, 44-49 its plan lines, which the code
does not have yet). The Updates band takes the Edition band's place and everything under it
moves down 56:

| Block | v0.4 | v0.5 |
|-------|------|------|
| heading, store lines (1-4) | y 16, 56, 76, 96 | the same |
| Updates band (50) | none | (16, 124, 432, 48) |
| Edition band (40) | (16, 124, 432, 64) | (16, 180, 432, 64) |
| `ROWS_TOP` | 196 | **252**; row `r` at `252 + 48 r` |

Two rows end at 348 and three at 396, inside the default 412; four scroll in the window's scroll
view, as the pane already does for a note.

| # | Widget | Place and size | Text and colour | Shown |
|---|--------|----------------|-----------------|-------|
| 50 | panel | (16, 124), 432x48, `menu.bg`, no border | holds 51-53 | always |
| 51 | label, 296 wide | (24, 6) in the panel | the updates line (section 2) in `text` | always |
| 52 | checkbox | (24, 24) in the panel | `Check every day` | always |
| 53 | button, 96x28 | (336, 10) in the panel | `Check now` or `Update all` | always |
| 54 + r | button, 96x28, `r` 0..5 | (232, 10) in row `r`'s panel | `Update`; `Cancel` while that row updates | the row has an update |
| 60 | button, 96x28 | (352, 66) on the pane | `Download` | no store, section 5 |

`WIDGETS` becomes **60** (v0.4's 49 + 11); `CAPACITY` grows with it (17 over today's code, which
lacks the six plan lines), and the installer's `dialog::CAPACITY` stays equal.

**The note's anchor.** The note (widgets 35-39, 44-49) opens under one of four anchors, and
everything below the anchor moves down by the note's height through `set_place`:

| Anchor | `note` (the anchor's bottom) | Used by |
|--------|------------------------------|---------|
| the store lines | 116 | the store download and its messages (section 5) |
| the Updates band | 172 | a failed check (section 2) |
| the Edition band | 244 | the edition confirmation and the band's messages (section 4) |
| row `r` | `252 + 48 (r + 1)` | install, update and remove, as v0.3 |

**A row with an update** has two buttons, so its texts end at panel x 216 instead of 320: the
title is ellipsised there, widget 7 (the state) is hidden, and widget 8 (the summary's line)
carries the row's state in `text`: `Update: 0.4 MB to download`. The summary returns when the
row is up to date; `component list --json` always has it.

## 2. The Updates band

The band shows the **last check's result**, which the capability stores (ask 2) and `component
list --json` returns, so showing the pane never touches the network. A check is `component
update --dry-run --json`: one range request for the release store's index when no store is on a
disk (D293), or a read of the attached store.

| State | Widget 51 | Widget 53 |
|-------|-----------|-----------|
| never checked | `Not checked yet` | `Check now` |
| checking | `Checking for updates...` | `Check now`, disabled |
| nothing to update | `Up to date · checked today at 09:14` | `Check now` |
| updates | `2 updates · 0.7 MB to download` (`1 update · ...`) | `Update all` |
| updates, store on a disk | `2 updates · 0.7 MB to copy` | `Update all` |
| updating all | `Updating 1 of 2: Windows apps` | `Update all`, disabled |
| nothing installed | `Nothing installed, so nothing to update` | `Check now`, disabled |

- **When**: `today at 09:14`, `yesterday at 09:14`, `3 days ago`; with no wall clock (the bar
  shows `up ...`), `since this start`. The bytes are the sum of the rows' `fetch_bytes`.
- **`Check now`** runs the check in the background (section 6's job); the rows and the band
  change when it ends. A check that fails opens a two-line message note under the band and
  keeps the last good result in widget 51: `Could not check for updates.` over `The network is
  offline.` (or `The release did not answer.`, `<reason>.`).
- **`Check every day`** (checked by default) is the daily check's switch: section 7.
- While any component installs, updates or is removed, widget 53 is disabled with the tooltip
  `Another component is installing` (`is updating`, `is being removed`).

## 3. Updating

**`Update` on a row** (no confirmation: an update is `change`, the old copy stays until the new
one is whole, and the row already says the size):

- The row's line reads `Updating 0 %`, `Update` becomes `Cancel`, `Remove` and every other
  button are disabled (tooltip `Another component is updating`), and the progress note opens
  under the row: the bar, and widget 35 naming the phase and its bytes.

  | Phase | Widget 35 | `Cancel` |
  |-------|-----------|----------|
  | fetching the changed files | `Downloading: 0.2 MB of 0.4 MB` | enabled |
  | staging (changed files written, unchanged ones copied and checked) | `Copying: 1.1 MB of 1.8 MB` | enabled |
  | the swap | `Swapping in...` | disabled |

  One bar for both phases, so it never goes back: permille `(fetched + staged) x 1000 /
  (fetch_bytes + size_bytes)`; the row's percent is the same number over ten. With a store on a
  disk there is no first phase.
- **Done**: the note closes, the row reads `Installed · 1.8 MB` with its summary and one button,
  the band counts one update fewer (or `Up to date · checked today at 09:20`: an update is a
  check). When programs that use the component are running (`in_use`), a one-line note stays
  under the row: `Updated. Running programs use it after a restart.`
- **`Cancel`** (or Delete on it): the staged copy is deleted; `Cancelled.` over `The installed
  copy stays.`; the row keeps `Update`.
- **Failure**: the row stays installed and keeps `Update`; nothing reads `Failed`, because
  nothing on the machine changed. A two-line note under the row:

  | Failure | Widget 35 | Widget 36 |
  |---------|-----------|-----------|
  | offline, or the release did not answer | `Could not download the update: <reason>.` | `The installed copy stays.` |
  | a fetched file fails its digest | `The downloaded update of Windows apps is damaged.` | the same |
  | an installed file fails its digest while copied | `The installed copy of Windows apps is damaged.` | `Remove it and install it again.` |
  | disk full | `Not enough space: 2.2 MB needed, 0.6 MB free.` | `The installed copy stays.` |
  | the store disk was detached | `The store was detached before the copy finished.` | the same |
  | anything else | `Could not update Windows apps: <errno text>.` | the same |
  | not root | `Not allowed: only root installs or removes components.` | hidden |

  A crash or power cut mid-swap needs no words: `tidy` finishes or undoes it (D293), and the
  next read shows what is there.

**`Update all`** runs `component update` with no name: the rows with an update, in the index's
order, each with its own note and bar as above. Widget 51 reads `Updating 1 of 2: Windows apps`;
the dropdown is disabled (tooltip `A component is updating`). `Cancel` on the updating row stops
it and the rest; a failure stops the rest too, as the command does. The rows not reached keep
`Update`, and the band counts them.

## 4. The edition confirmation, under the band (D263's first deviation)

As `editions-v0.4.md` sections 3 and 5 drew it, with `note` = 244: widget 35 `Would install for
Personal:`, one plan line per component (widgets 44.., `Windows apps · 1.8 MB`), widget 36 `3.2
MB in all.`, `Cancel` and `Install`; `84 + 20 n` tall. The band's messages (`Personal is
installed: nothing to add.`, the offline lines, a failed download) open as message notes under
the band; widget 43 goes back to being the summary and nothing else, except `Installing
Personal: 1 of 2` while a plan runs. The code's `Note` gains the anchor of section 1.

## 5. No store: the pane downloads it (D263's second deviation, D257)

With no store on a disk, in the host share or in `/var/cache/eggos/`:

| Network | Widget 2 (`text`) | Widget 3 (`text.dim`) | Widget 4 (`text.dim`) | Widget 60 |
|---------|-------------------|-----------------------|-----------------------|-----------|
| up | `No component store on this computer.` | `Download it to see what you can install.` | `From the EggOS 0.5.0 release.` | `Download` |
| down | the same | `Connect, or attach the store disk.` | `run-eggos.sh attaches it automatically.` | `Download` |

- **`Download`** opens the download note under the store lines (`note` 116; the bands and rows
  move down 84): the bar at +8, widget 35 at +24 `Downloading the component store: 12.4 MB of
  31.0 MB`, `Cancel` (38) at +48. With no length from the release the bar is hidden and the line
  ends `12.4 MB`. The download also shows in the Downloads window. When it ends the note closes,
  the store lines read `Component store: EggOS 0.5.0, x86_64` over `... downloaded from the
  release`, widget 60 hides and the rows appear with `Install`.
- **Pressed offline**: a message note, `The network is offline.` over `Connect or attach the
  store disk, then try again.` The button stays enabled: the press answers.
- **Choosing an edition with no store** is v0.4's section 4 (the confirmation by name, then the
  same download note, here under the Edition band, then the plan with no second question).
- **Outcomes**: `Cancel` gives `Cancelled.` over `Nothing was downloaded.` and deletes the
  partial file; a failure `Could not download the component store: <reason>.` over the same; a
  damaged file `The downloaded component store is damaged.` and the file is deleted.
- Updates need no store download: the check and the fetch use range requests (D293).

## 6. Live progress and cancelling one install (D235's "Not yet")

Today the copy runs inside one tick of the window, so the bar jumps from nothing to done and one
install cannot be stopped. From v0.5 every install, update, remove and store download is a
**job** outside the window's program (ask 1), and the pane only reads it:

- The pane commits the frame with the note open at `0 B of 1.8 MB` before the job starts, then
  reads the job's progress every **100 ms** while one runs (once a second otherwise, as v0.3),
  damaging only the bar, widget 35 and the row's state label. A copy shorter than one read shows
  the bar at 0 and then the result; nothing flashes, since the note closes with the result.
- **`Cancel`** on the installing row stops that one job: the staged files are deleted, the row
  reads `Not installed · 1.8 MB`, the note `Cancelled.` over `Nothing was installed.` In a plan
  (a dependency, an edition), the components already installed stay and the rest are not
  attempted. While the checked copy is moved into place the note reads `Finishing...` and
  `Cancel` is disabled: from there the job ends in under a second, one way or the other.
- Leaving the pane or closing Settings does **not** stop a job. The pane shown again finds it in
  `component list` (`installing 42 %`, `updating 42 %`) and reopens its note; `Cancel` works on a
  job any front-end started.
- Disabled row buttons carry their tooltips (v0.3 section 6), which the code lacks.

## 7. The daily check, the pane's notice and the bar's chip

- **The check** is `component update --dry-run --json`, run by the system once a day: ten
  minutes after the network is up when the last check is over 24 hours old, then every 24 hours.
  Offline, it tries again each hour. It fetches the index only, installs nothing, and says
  nothing when it fails (the band keeps the last good result and its age). `Check every day`
  unchecked stops it; `Check now` still works.
- **In the pane** the notice is the band: `2 updates · 0.7 MB to download` and the rows'
  `Update` buttons. No dot on the sidebar's row and no dialog.
- **On the bar: an `Updates` chip**, and nothing else. Decided for, because a person who never
  opens Settings would never learn of an update; and against a popup or a sound, because an
  update is never urgent enough to interrupt. The chip is drawn only while the stored result has
  at least one update: not when up to date, not when a check failed.
  - Geometry as the Agent chip's (`desktop-v0.3-gui.md` 14.1): label `Updates`, 72x20,
    `INSET_R4`, text at x + 8, fill `BAR_HOVER`, text `TEXT_DIM`, hover and pressed
    `BAR_PRESSED`. Top bar: right edge at the Agent chip's left - 8, or at `power_button().x -
    8` when there is no Agent chip: at 1280, (1100, 4, 72, 20) or (1164, 4, 72, 20). Taskbar:
    right edge at the Agent chip's left - 8, or at `clock_rect().x - 16`: (992, 770, 72, 20) or
    (1056, 770, 72, 20); the taskbar items stop 16 px before the leftmost chip.
  - Tooltip after 600 ms: `2 component updates, 0.7 MB. Click to open Components.`
  - A click opens Settings on Components with the focus on `Update all`. It updates nothing.
  - An agent sees a `button` named `Updates` with the value `2`.
  - Phone, tablet, TV and wearable bars have no chip (no room, or no pointer); the band is the
    notice there.

## 8. Keys, focus and what an agent sees

- **Tab** from the sidebar: `Download` when shown, `Check now` / `Update all`, `Check every
  day`, the dropdown, then per row `Update` and the row's own button, then an open note's
  buttons; disabled ones are skipped. Showing the pane does not move the focus.
- **Up/Down** on a row button move to the previous or next row, staying on `Update` when that
  row has one; **Left/Right** move between a row's two buttons. Up from row 0 goes to the
  dropdown, Up from the dropdown to the band's button.
- **Enter and Space** press; Space toggles the checkbox; **Delete** on a `Cancel` presses it;
  **Esc** closes an open confirmation. None of the new buttons is a default button.
- **Accessible names**: `Update Windows apps`, `Cancel updating Windows apps`, `Update all
  components`, `Check for updates now`, `Download the component store`. The Updates band is a
  `group` named `Updates` with the value the count (`2`, `0`, or `unchecked`); a row's group
  value gains `update 0.4 MB` (`windows-apps installed 1.8 MB update 0.4 MB`), or `updating
  42`; the bars are `progress` named `Updating Windows apps` and `Downloading the component
  store`.
- Nothing new is person-only: an update and a download add or replace and never destroy, like
  `Install`. `Remove`'s confirming button stays person-only.

## 9. One capability, three front-ends (D22)

| Button | Command | MCP tool |
|--------|---------|----------|
| showing the pane | `component list --json` | `component_list` |
| `Check now`, and the daily check | `component update --dry-run --json` | `component_update`, `dry_run` |
| `Check every day` | `component update --daily on` / `off` | `component_update`, `daily` |
| `Update` on a row | `component update <name>` | `component_update`, `name` |
| `Update all` | `component update` | `component_update` |
| `Download` | `component list --download` | `component_list`, `download` (the `install` grant) |
| `Install`, `Install both` | `component install <name>` | `component_install` |
| `Remove`, confirmed | `component remove --yes <name>` | `component_remove` |
| the dropdown, then `Install` | `component edition <name>` | `component_edition` |
| `Cancel` on a running job | Ctrl+C in the command, or `component cancel` (ask 1) | the client's cancel of the call |

Every sentence in sections 2 to 6 comes from one function in `eggos_components::messages`,
shared by the pane, the command and the tool; the command joins a note's two lines with `; ` and
lower-cases the second, as v0.3 section 7 does (`...; the installed copy stays.`). `component
list` prints `update 0.4 MB` after the size of a row that has one, and `updating 42 %` as a
state. `component_update` asks with the grant prompt when `install` is `ask`: `component_update
· Update components`, `updates: Windows apps, Mac apps`, `download: 0.7 MB`.

## 10. Every device class (D263's third deviation, D235's phone layout)

- **Phone** (pane 328 wide, 48 px targets): one column, each block 8 under the last. Store
  lines as v0.3; `Download` full width, 48 tall, at y 120 (the blocks below start 56 lower while
  it shows). Updates band at y 124, 132 tall: the line at (8, 8), the checkbox's row at (8, 28),
  40 tall, the button at (8, 76), the band's width minus 16, 48 tall. Edition band at 264, 116
  tall (v0.4). Rows from **388**, 128 tall (v0.3); a row with an update is 184 tall: `Update` at
  (8, 72) and the row's own button at (8, 128), both full width, 48 tall. Notes as v0.3: lines
  at x 24, the bar the width minus 48, buttons stacked full width with the primary one last. A
  long-press on a disabled button shows its tooltip.
- **Tablet**: the desktop layout with 40 px controls: Updates band 56 tall at 124 (the line at
  +8, the checkbox at +28, the button at +8), Edition band 72 tall at 188, rows from **268**, 56
  tall, both row buttons 40 tall at +8, the notes' buttons 40 tall.
- **TV** (scale 2, D-pad): everything doubles and the ring is always drawn. Right from the
  sidebar lands on `Download` when shown, else on the band's button. Down goes button,
  checkbox, dropdown, rows; Left/Right move between a row's two buttons, and Left from the
  leftmost goes back to the sidebar. OK presses, Back closes a confirmation. While a job runs
  the ring sits on its `Cancel`. Nothing needs a pointer or hover: a disabled button's reason
  shows as the focused widget's tooltip.
- **Wearable**: still read-only: a first line `2 updates` when there are any, and the row's
  state word `Update ready`; the last line stays `Install or remove from your phone or the CLI.`
- **Headless**: no pane and no chip; the commands and the tools are the whole capability.

## 11. Acceptance (host tests, 640x440, 1x, window coordinates; fake A: a store on `/dev/vdc`
offering `windows-apps` 1.8 MB and `mac-apps` 1.4 MB, nothing installed; fake B: no store on a
disk, both installed, a stored check from today 09:14 with 0.4 MB and 0.3 MB to fetch)

- The pane holds 60 widgets. Fake A: the Updates band (192, 124, 432, 48) with `Nothing
  installed, so nothing to update` at (216, 130), `Check every day` at (216, 148) checked, and
  `Check now` (528, 134, 96, 28) disabled; the Edition band (192, 180, 432, 64) with the
  dropdown (464, 184, 160, 28); row 0 at y 252, row 1 at 300; widgets 54-60 hidden.
- Fake A, choosing `Personal`: `Would install for Personal:` at (216, 248), `Windows apps · 1.8
  MB` at cell top 268, `Mac apps · 1.4 MB` at 288, `3.2 MB in all.` at 308, `Cancel` (392, 332,
  96, 28), `Install` (496, 332, 128, 28) focused, row 0 at y 376. With both installed, choosing
  `Server` shows its two lines at 248 and 268 and widget 43 still holds the summary.
- Fake A, `Install` on row 0 with a job that reports 0, 42 % and done on three reads 100 ms
  apart: the bar is at 0, then 420 permille with `0.8 MB of 1.8 MB`, damaging three rects only;
  then `Installed · 1.8 MB`. `Cancel` at 42 % leaves no file in the components folder and shows
  `Cancelled.` over `Nothing was installed.`; in the `Finishing...` phase `Cancel` is disabled.
  A pane rebuilt while the job runs shows `Installing 42 %` and the note again.
- Fake B: widget 51 `2 updates · 0.7 MB to download`, widget 53 `Update all`; row 0's title at
  (216, 258), `Update: 0.4 MB to download` at (216, 278) in `text`, `Update` (424, 262, 96, 28),
  `Remove` (528, 262, 96, 28), no state label; row 1 the same at y 300 with 0.3 MB.
- Fake B, `Update` on row 0: the line reads `Updating 0 %`, the button `Cancel`, `Remove` and
  row 1's buttons disabled, the bar (216, 308, 408, 8), `Downloading: 0 B of 0.4 MB` at cell
  top 324, row 1 at y 340. A read with half the fetch done: `Downloading: 0.2 MB of 0.4 MB`,
  the bar at 90 permille, `Updating 9 %`. A read in staging: `Copying: ...`. Done: `Installed ·
  1.8 MB` ending at 512, the summary back, one button, widget 51 `1 update · 0.3 MB to
  download`.
- Fake B, failures: a digest mismatch shows `The downloaded update of Windows apps is damaged.`
  over `The installed copy stays.`, the row still installed with `Update`, and `component list`
  says `installed`; offline shows the download line; `Cancel` mid-fetch shows `Cancelled.` over
  `The installed copy stays.` and no `.staging-windows-apps` folder.
- Fake B, `Update all`: widget 51 `Updating 1 of 2: Windows apps`, then `2 of 2: Mac apps`,
  then `Up to date · checked today at` the fake clock's time, and `Check now`. A failure on the
  first leaves row 1 with `Update` and the band at `2 updates`.
- `Check now` on a fake with nothing changed: `Checking for updates...` with the button
  disabled, then `Up to date · checked today at 09:14`. Offline: the last result stays and the
  note under the band reads `Could not check for updates.` over `The network is offline.`
  Unchecking `Check every day` writes the setting `component update --daily off` writes.
- No store anywhere, network up: the three lines of section 5 and `Download` at (528, 66, 96,
  28). Pressing it: the bar (216, 124, 408, 8), `Downloading the component store: 12.4 MB of
  31.0 MB` at cell top 140 with the bar at 400 permille, `Cancel` (392, 164, 96, 28), the
  Updates band at y 208; the end shows the rows with `Install` and hides widget 60. Network
  down: the two offline lines and no download.
- The desktop with two stored updates draws the chip at (1100, 4, 72, 20) beside the Agent chip
  and at (1164, 4, 72, 20) without it; (992, 770) and (1056, 770) on the taskbar; no chip with
  none. Its click makes Settings' `pane()` `COMPONENTS` with the focus on widget 53.
- Tab order is section 8's; Right on `Update` focuses `Remove`; Down from row 0's `Update`
  focuses row 1's.
- A pane 328 wide lays the Updates band 132 tall at y 124 and rows from 388, an update row 184
  tall; a tablet lays rows from 268. The element tree holds the `group` `Updates` with the value
  `2` and a `button` named `Update Windows apps`.
- `component update --dry-run --json`, `component_update` with `dry_run`, `component list
  --json` and the pane agree on which components have updates and on every byte count. Every
  line fits a label's 64 bytes; a test pins each.

## 12. Backend asks

For the capability (`eggos-components`, `component`, the MCP tools):

1. **Jobs outside the caller.** Install, update, remove and the store download run where a
   window's tick does not wait for them, report `phase`, `done_bytes` and `total_bytes` at least
   every 100 ms, and can be cancelled by any front-end (`component cancel`, the MCP call's
   cancellation) up to the swap. `component list --json` shows a running job as `installing` or
   `updating` with `percent`, `phase` and the bytes.
2. **The check's result is stored** (for example `/var/lib/eggos/updates`) by every `component
   update --dry-run`, and `component list --json` returns it: per item `update`: `null`, or
   `fetch_bytes`, `changed_bytes` and `changed_files`; once, `updates_checked` (a time, or
   `null`) and `updates_source` (`release` or `disk`).
3. **The daily check**: a system timer that runs the dry run as section 7 says, and `component
   update --daily on|off` with its state in `component list --json`.
4. **`component update` with no name stops at the first failure** and says which were updated
   and which were not. If the command carries on instead, say so and section 3 follows it.
5. **What a running program sees after a swap** (the desktop updating itself included): if it
   keeps the old files until it restarts, section 3's `Updated. Running programs use it after a
   restart.` stands; if an update of a component in use must be refused, the pane shows v0.3's
   `In use` line instead.
6. **`component list --download`** (and `download` on `component_list`): fetch the release's
   store into `/var/cache/eggos/` with progress and no install, so the pane's `Download` equals
   a command. The store's size before the download, if the release can give it cheaply, would
   let widget 4 say it.
7. **Failure codes** for update: `update_offline`, `update_damaged`, `installed_damaged`,
   `no_space`, `store_detached`, `update_failed`, `cancelled`, each with section 3's sentence.

For the desktop:

8. The `Updates` chip: `Desktop::set_updates(count, bytes)`, told on each stored result, and a
   click that opens Settings on a named pane.

For the toolkit:

9. Table rows with button cells (`wifi-v0.4.md`, ask 11) are still not in `toolkit-v0.5.md`;
   this design keeps the row panels. With them, the rows become one table and `MAX_COMPONENTS`
   goes away.

## Left out

Installing updates automatically; an update's notes (what changed); updating EggOS itself (the
base image); a schedule other than daily; a metered-network rule; several jobs at once and a
queue that carries on after a failure; binary deltas; a signed store index; rolling an update
back from the pane; the summary shown beside an offered update.
