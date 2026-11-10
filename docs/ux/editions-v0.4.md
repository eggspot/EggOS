# EggOS v0.4.0 Editions: design brief (the GUI front-end of `component edition`)

Status: design for v0.4, D258.

Design by Claude Fable (D16) for the GUI front-end of the edition capability (D18, D22, D258): an
`Edition` band at the top of Settings' `Components` pane, with the same data and words as
`component edition [--dry-run] [<name>]` and the MCP tool `component_edition`. It extends
`components-v0.3.md`, and everything there stands: the five states, the store lines, the rows, the
note's three forms, the one-action-at-a-time rule, the refusals and failures, the tokens and the
contrast table. Widgets 1 to 39 keep their numbers; the rows and the note move down 72 px (section
1). Numbers are px at 1x and double at 2x; the only motion is the progress bar following bytes.

**The data.** `eggos_components::editions::EDITIONS`, smallest first: `server` `Server`, `personal`
`Personal`, `professional` `Professional`, `enterprise` `Enterprise`, each with a summary and a list
of components. The current edition is `editions::current(installed)`: the edition whose list is
exactly what is installed, or none. An edition only adds (D258): it never removes a component.

## 1. The band in the pane (pane coordinates, as `components-v0.3.md` section 2)

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 40 | panel | (16, 124), 432x64, fill `menu.bg`, no border | holds 41-43; always shown, store or not |
| 41 | label | (24, 10) in the panel | `Edition` in `text` |
| 42 | dropdown, name `Edition` | (272, 4) in the panel, 160x28 | the editions' titles; ends at pane x 448 |
| 43 | label | (24, 38) in the panel, 408 wide | the shown edition's summary in `text.dim` |
| 44 + i | label, `i` 0..5 | (40, note + 24 + 20 i), 408 wide | the plan's line `i` in `text`; shown only in the edition confirmation |

- `ROWS_TOP` becomes **196** (the band ends at 188, 8 px of surface, then row 0): row `r` is at
  `196 + 48 r`. Two rows (today's store) end at 292 and four at 388, inside the default 412; at the
  minimum window the pane scrolls in the window's scroll view, as it already does for a note.
- `WIDGETS` becomes **49** (39 + the band's 4 + 6 plan lines), so `CAPACITY` grows by 10.
- The band is created right after widget 4, so Tab reaches the dropdown before row 0's button.
- The note (widgets 35-39) opens under the band exactly as under a row: `note` is 188, and every
  row moves down by its height through `set_place`. The band is "the acting row" for an edition.

## 2. The current edition

The dropdown shows `current()`'s title, and widget 43 its summary from `EDITIONS`, re-read on every
read of the pane (so installing or removing a component in its row changes it in the same frame).

- An edition matches: options `Server`, `Personal`, `Professional`, `Enterprise`; the selected one
  is the edition `component edition` stars.
- None matches (say `mac-apps` alone): the options become `Custom` and the four titles
  (`tree.change` swaps the option slice), `Custom` selected, widget 43 `Your own set of components:
  no edition matches it exactly.` The CLI prints no star then. Choosing `Custom` does nothing.
- Summaries are ellipsised at 64 bytes by `shortened`, as every line is; a test pins each under 64.

## 3. Choosing another edition

`Response::Chosen(42, option)`. The current option: nothing happens. Another edition: the pane
builds the plan with the function `component edition <name> --dry-run` uses (the edition's
components that are not installed and that the store offers for this machine, each preceded by
what it needs, in install order), and:

- **Empty plan**: a one-line message note under the band, the CLI's sentence, `Personal is
  installed: nothing to add.`; the dropdown goes back to the current edition. When that leaves the
  dropdown on another title than the one chosen, widget 36 adds `An edition only adds: remove a
  component in its row below.` (`text.dim`, two-line note, 44 tall).
- **A plan**: the **confirmation** opens under the band. Widget 35 `Would install for Personal:`
  (the CLI's dry-run sentence up to its colon); widgets 44.. one line a component as its row shows
  it, `Windows apps · 1.8 MB`, `Mac apps · 1.4 MB` (the title and size from the store's index); widget
  36 at `note + 24 + 20 n`, `3.2 MB in all.` (`messages::in_all`); `Cancel` (38) and `Install` (39,
  primary, the default button) at `note + 48 + 20 n`. Height `84 + 20 n` for `n` lines: 124 for two.
  The dropdown shows the chosen title while the confirmation is open; every row's button is
  disabled (tooltip `Choosing an edition`). The focus opens on `Install`.
- **`Cancel`**, Esc, or leaving the pane: nothing is installed, the dropdown returns to the current
  edition and takes the focus.
- **`Install`**: the note closes and the plan runs component by component with the pane's existing
  behaviour (`components-v0.3.md` section 4): the note moves to each component's row with its bar and
  `0.8 MB of 1.8 MB`, the row reads `Installing 42 %` with `Cancel`, every other button is disabled.
  Meanwhile the dropdown is disabled and keeps the chosen title, and widget 43 reads `Installing
  Personal: 1 of 2` (the component being installed, of the plan's length).

## 4. No store attached (D257)

The band still shows (the editions are in the program, not in the store). Choosing an edition:

- **A store is in `/var/cache/eggos/`** (downloaded earlier): it is a store like any other; section 3.
- **No store, the network up**: the confirmation lists the components by **name** (no index, so no
  titles and no sizes: `windows-apps`, `mac-apps`), and widget 36 reads `Downloads the component
  store from the release first.` `Install` turns the note into the **download** form, under the
  band: the bar at +8, widget 35 at +24 `Downloading the component store: 12.4 MB of 31.0 MB`,
  `Cancel` (38) at +48; 84 tall. The bar is hidden and the line ends `12.4 MB` when the release sends
  no length. The download also shows in the Downloads window, as D257 says. When it ends, the store
  lines read `Component store: EggOS 0.4.0, x86_64` over `... downloaded from the release`, the rows
  appear, and the plan is rebuilt from the index (a component the store does not offer for this
  machine drops out) and runs as in section 3 with no second question.
- **No store, the network down**: no confirmation. A two-line message under the band: `No component
  store, and the network is offline.` over `Attach the store disk or connect, then choose again.`
  The dropdown stays enabled: the press answers, as `Remove` does, and a greyed choice says nothing.

## 5. Done, refused, failed

| Outcome | What shows |
|---------|------------|
| done | the last row's note closes; the rows read `Installed · 1.8 MB`; the count line updates; the dropdown is enabled and shows `current()`, widget 43 its summary. No message: the band is the result |
| cancelled while downloading | under the band `Cancelled.` over `Nothing was installed.`; the partial file is deleted |
| cancelled while installing | under that row, v0.3's `Cancelled.` note; components already installed stay; the rest are not attempted; the dropdown shows `current()` (often `Custom`) |
| a component fails | v0.3's failure note under that row and its state `Failed`; the rest are not attempted, as the CLI stops at the first failure |
| the download fails | under the band `Could not download the component store: <reason>.` over `Nothing was installed.` |
| the downloaded store is damaged | `The downloaded component store is damaged.` over `Nothing was installed.`; the file is deleted |
| not root | `Not allowed: only root installs or removes components.` |
| busy | while a row installs or removes, the dropdown is disabled with the tooltip `Another component is installing` (`is being removed`) |

A message under the band stays until the next action, a new choice, or the pane being shown again.

## 6. Keys, focus and pointer

- **Tab** from the sidebar enters the pane at the dropdown, then the rows' buttons, then an open
  confirmation's `Cancel` and `Install`; a disabled dropdown is skipped. Showing the pane does not
  move the focus.
- **The dropdown** is the toolkit's (`toolkit-v0.4.md` 3.1): Space, Enter or Down opens the popup,
  Up/Down move, Home/End, a letter jumps to a title, Enter chooses, Esc closes with no choice.
  **Up** on row 0's button moves the focus to the dropdown.
- **Enter** in the confirmation presses `Install` (the default button while it shows); **Esc**
  cancels it. So Personal from Server is: Enter, Down, Enter, Enter.
- **Pointer**: a click opens the popup, a click on a title chooses it, a click outside closes it.

## 7. Every device class (D23)

- **Phone**: the band is the pane's width minus 32 and 116 tall: `Edition` at (8, 8), the dropdown at
  (8, 32), the band's width minus 16, **48** tall, the summary at (8, 88), ellipsised. Rows start at
  248. The confirmation's lines keep x 24; its buttons stack full width, 48 tall, `Install` last;
  it is `164 + 20 n` tall. The popup's rows are 48 tall.
- **Tablet**: the desktop layout with a 40 px dropdown: the band is 72 tall (summary at +46), rows
  start at 204, the confirmation's buttons are 40 tall.
- **TV**: everything doubles; the ring sits on the dropdown when the pane is entered (Right from the
  sidebar); OK opens the popup, Up/Down move in it, OK chooses, Back closes it; Down from the
  dropdown goes to row 0's button, or to `Install` while a confirmation shows (Left/Right between
  its two buttons). Nothing needs a pointer.
- **Wearable**: read-only: a line `Edition: Personal` (`Edition: Custom`) above the list.
- **Headless**: no pane; `component edition` and `component_edition` are the whole capability.

## 8. What an agent sees (D161), and the three front-ends

- The band is a `group` named `Edition` with the value the CLI's name: `personal`, or `none`. In it,
  the `label` `Edition`, the `dropdown` named `Edition` with its title as value and its options, and
  the summary `label`. The plan lines are `label`s with the value the component's name
  (`windows-apps`), so a title maps back to what `--dry-run` prints. The download bar is a
  `progress` named `Downloading the component store`.
- Nothing is person-only: an edition adds and never destroys, so `Install` and the dropdown stay
  actable (`choose`, `invoke`), like v0.3's `Install both`.
- **Words, one function each**: `Would install for <Title>: ...`, `<Title> is installed: nothing to
  add.`, the failures and `in all` come from `eggos_components::messages` for the pane, the command
  and the tool. `component_edition` asks with the grant prompt when `install` is `ask`: lines
  `component_edition · Install an edition`, `name: personal`, `installs: Windows apps, Mac apps`,
  `size: 3.2 MB`.

## 9. Asks of the capability (the same for all three front-ends)

- **Remember the choice.** Personal, Professional and Enterprise list the same components today, and
  `current()` answers the largest match, so a person who chooses `Personal` sees `Enterprise`. The
  edition action should record its name (`/var/lib/eggos/edition`) and `current()` should prefer the
  recorded edition while its list still matches. The pane shows whatever `current()` says.
- **Server's summary** is 73 bytes, over a label's 64: shorten it in `editions.rs`, for example `The
  base alone: no desktop and no foreign apps.`
- The pane needs a request to download the store (D257's fetch, with its bytes for the bar).

## 10. Acceptance (host tests, default 640x440, 1x, a fake store on `/dev/vdc` offering `windows-apps`
1.8 MB and `mac-apps` 1.4 MB, nothing installed)

- The pane holds 49 widgets. In window coordinates: the band (192, 124, 432, 64), `Edition` at (216,
  134), the dropdown (464, 128, 160, 28) reading `Server`, Server's summary at (216, 162); row 0 at
  y 196 and row 1 at 244. Tab from the sidebar focuses the dropdown, then row 0's `Install`.
- Choosing `Personal` opens the confirmation: `Would install for Personal:` at (216, 192), `Windows
  apps · 1.8 MB` at cell top 212, `Mac apps · 1.4 MB` at 232, `3.2 MB in all.` at 252, `Cancel` (392,
  276, 96, 28), `Install` (496, 276, 128, 28) focused; row 0 at y 320; both rows' buttons disabled.
  Esc closes it, installs nothing, shows `Server` and focuses the dropdown.
- `Install`: row 0 reads `Installing 0 %` with the bar under it, the dropdown is disabled, widget 43
  reads `Installing Personal: 1 of 2`; then row 1; then both read `Installed`, the count line `2 of
  2 installed · 3.2 MB on disk`, and the dropdown shows what `component edition` stars.
- With both installed, choosing `Server` shows `Server is installed: nothing to add.` over `An
  edition only adds: remove a component in its row below.` and the dropdown returns.
- `mac-apps` alone installed: the dropdown reads `Custom` with five options, and `component edition`
  prints no star. A fake whose first install fails with a checksum mismatch shows v0.3's lines
  under row 0, row 1 stays `Not installed`, nothing else is attempted.
- No store, network up: the confirmation lists `windows-apps` and `mac-apps` over `Downloads the
  component store from the release first.`; `Install` shows the download note with `Cancel`; a tick
  with 12.4 of 31.0 MB shows that line and the bar at 400 permille; the end shows the rows and
  starts row 0. No store, network down: the two offline lines and no confirmation.
- A pane 328 wide lays the band 116 tall with a 48 px dropdown. The element tree holds the `group`
  `Edition` with the value `server` and a `dropdown` named `Edition`; `wm act invoke` on `Install`
  presses it. `component edition personal --dry-run`, `component_edition` with `dry_run` and the
  confirmation name the same components in the same order. Every line fits 64 bytes.

## Left out

Editions that remove what they do not include (so `Server` on a full machine really is the base
alone); the edition choice in first run; each edition's own settings (updates, policies); a
component's `Adds:` line in the confirmation; downloading only the plan's components instead of the
whole store; a queue that carries on after a failure; an edition's page; the wearable's own choice.
