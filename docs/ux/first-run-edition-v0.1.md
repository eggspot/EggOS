# EggOS v0.1 first run: choosing an edition: design brief

Design by Claude Fable (D16) for the edition choice D258 left for "first run, which needs a
Claude Fable design". It adds a **fourth page** to the first-run window of
`first-run-accessibility-v0.3.md` section 1 (the three pages, the shared widgets S1 to S6,
the save-at-once rule, the keys, the device classes) and reuses the edition capability as
`editions-v0.4.md` and `component edition` define it: the four editions of
`eggos_components::editions::EDITIONS` with their summaries, the plan a dry run builds, the
install path of the Components pane, the words of `eggos_components::messages`. Nothing in
those files changes but the step count (`Step 1 of 4` ... `Step 4 of 4`). Numbers are px at
1x in window content coordinates (640x440, the right-aligned edge x 624; the 40 px grid from
y 64) and double at 2x; the only motion is the Components pane's progress bar, which this
window never shows.

## 0. What a person gets, and how that is checked

- The last question of first run: **`Which edition?`**, four choices with one line each,
  `Personal` already chosen, the size and what it installs under them.
- `Done` records the choice and starts the installs in the background; the person is at
  their desktop at once and sees the downloads in the Downloads window and the Components
  pane. Nothing is removed, ever.
- The same choice is `component edition <name>` on the CLI, `component_edition` for an
  agent, and Settings > Components' `Edition` dropdown later.
- Host tests at 640x440, 390x752, 960x540 at scale 2 (section 7).

## 1. Where it sits

Page **4 of 4**, after `Time zone`: the profile, the keyboard and the clock are what the
person needs to use the machine in the next minute; the edition starts downloads, which
belong at the end so a `Skip` on page 1 still leaves a usable machine and the network has had
its first seconds to come up. `Back` from it returns to `Time zone`; `Next` on page 3 reads
`Next`, and page 4's primary button is `Done`. S2 reads `Step 4 of 4` (so S2 counts four on
every page). Headless installs never see it (first run does not open, 1.1); their edition is
the CLI line they run anyway.

## 2. The page (`Which edition?`)

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 4.1 | radio group, 4 radios | rows at y 64, 120, 176, 232; each radio (16, row + 6), 28 tall, `Length::Content` wide | `Personal`, `Professional`, `Enterprise`, `Server` (the titles of `EDITIONS`, largest first but `Server` last) |
| 4.2-4.5 | labels, right-aligned | end at x 624, cell top row + 12 | the edition's size, `3.2 MB` (`messages::in_all`'s number for its plan), in `text.dim`; `Installed` when the plan is empty |
| 4.6-4.9 | labels, 584 wide | (40, row + 32) | the edition's summary in `text.dim`: `Windows and macOS apps beside EggOS's own.`, `Personal, with every tool for work that the store offers.`, `Professional, ready for managed fleets.`, `The base alone: no desktop and no foreign apps.` |
| 4.10 | label, 608 wide | (16, 300) | the **installs line** (section 3) in `text.dim`, or a refusal in `text` |
| 4.11 | label, 608 wide | (16, 320) | the second line (section 3) in `text.dim`; hidden when there is none |

Rows are 56 apart (a 28 px radio row and a 20 px summary line, 8 px of air), four of them end
at 288; the two lines at 300 and 320 end at 336, 20 px above S3's status line at 356, which
keeps `Every choice can be changed later in Settings.` (true here too: the Components pane's
dropdown). The radio's label is the title alone: the summary is a line under it, not part of
the control, so the group reads as four short names to the eye and the screen reader
(`Personal, radio, selected, 1 of 4`).

**Default**: `Personal` is selected when the page is shown and no edition is recorded
(`/var/lib/eggos/edition`, `editions-v0.4.md` section 9's ask): a machine at first run has a
display and a person, which is Personal's case; Server is offered because the same four names
appear everywhere (D22) and a person installing a server with a display is real. A recorded
edition (a re-run of `settings --first-run`) is the one selected. The order puts Server last
so the default is first and the two managed editions sit between.

## 3. The installs line, and what a choice does

Selecting a radio builds the plan with the dry-run function (`component edition <name>
--dry-run`'s: the edition's components not yet installed that the store offers for this
machine, in install order) and writes the two lines, nothing else yet:

| Case | 4.10 | 4.11 |
|------|------|------|
| a plan, store present | `Installs Desktop, Built-in apps, Windows apps, Mac apps: 3.2 MB.` (titles from the store's index, `messages::in_all`) | `Downloads start when you press Done; EggOS is ready to use meanwhile.` |
| a plan, no store, network up (D257) | `Installs desktop, builtin-apps, windows-apps, mac-apps.` (names, no sizes) | `Downloads the component store from the release first, when you press Done.` |
| a plan, no store, network down | `Installs desktop, builtin-apps, windows-apps, mac-apps.` | `No component store, and the network is offline: the installs wait in Settings > Components.` |
| empty plan | `<Title> is installed: nothing to add.` (the CLI's sentence) | hidden |
| `Server` chosen on a machine with the desktop | `Nothing to add.` | `An edition only adds: remove components in Settings > Components.` (`editions-v0.4.md` section 3's line) |
| not enough disk | `Not enough space: 3.2 MB needed, 0.6 MB free.` (`components-v0.3.md`'s line, in `text`) | `Free some space, then choose the edition in Settings > Components.` |
| a policy locks the edition (`editions-policy-v0.1.md`) | `Managed by Contoso IT.` in `text` | the radios disabled, the managed edition selected |

The free space is the components volume's, read when the page is shown and after each
choice; the line is `text` because it is a refusal of the person's choice, as the first-run
status line's refusals are.

**What is saved, when** (1.4's rule, kept): the **choice is recorded at once** in
`/var/lib/eggos/edition` by `Request::SetEdition(name)` (the same write `component edition`
makes before it installs), so a crash or a close after the choice keeps it and the Components
pane shows it selected. The **installs run at `Done`** (or the close control, or `Skip` on
this page): the window sends `Request::InstallEdition(name)` and closes without waiting; the
component manager runs the plan through the Components pane's path (`components-v0.3.md`
section 4), each download listed in the Downloads window (D257), the pane's row reading
`Installing 42 %` for whoever opens it, and a notice `Personal installed.` (the shell's
notice of `desktop-v0.3-gui.md` 6.4 item 2) when the last component is in, or `Could not
install <Title>: <reason>.` on the first failure (the rest not attempted, as the CLI). With
not enough disk or no store offline, `Done` records the choice and starts nothing: the
Components pane's band shows the recorded edition with its own message when opened. `Back`
writes nothing. A failed record (`EPERM`, `EACCES`, no disk) puts the radio back and the
status line S3 says `Not allowed: this user may not set the edition.` or `No disk attached:
the edition could not be saved.`, as the other pages' refusals.

Why not install while the page shows: a first run should end in seconds, the downloads take
minutes, and the pane already shows progress; one place for progress (D22) beats a second
bar here.

## 4. Keys, focus and pointer

- **Initial focus**: the selected radio. **Tab order**: the radio group (one stop; Up/Down
  move the selection, the toolkit's radios), `Skip`, `Back`, `Done`, wrap.
- **Up/Down** select the previous/next radio (and record it, section 3); Home/End the first
  and last; Space selects the focused one. **Enter** reaches `Done` from the group (radios do
  not use Enter).
- Keyboard-only from page 3: Enter (`Next`), Down (`Professional`), Tab, Tab, Tab, Enter
  (`Done`): the whole first run stays under twenty keys.
- **Pointer**: a click on a radio or its summary line selects it (the line reaches the radio
  through the label, as the zone rows' labels do); `Arrow` everywhere; a right-click nothing.
- **Esc**: the window's, ignored (1.5).

## 5. CLI, MCP and agents (D22)

- No new command and no new tool: the page is a GUI way of `component edition <name>`
  (which records and installs; `--dry-run` prints the page's installs line) and the MCP tool
  `component_edition` (install grant, its prompt lines `component_edition · Install an
  edition`, `name: personal`, `installs: ...`, `size: ...`). An agent setting a machine up
  calls `settings_set` three times and `component_edition` once. `settings --first-run` is
  still a mode, not a capability (1.7).
- **Element tree** (`wm show`): a `radio group` named `Which edition?` with `radio`s
  `Personal`, `Professional`, `Enterprise`, `Server`, the chosen one `selected`, each valued
  by the CLI's name (`personal`); the size and summary `label`s; the installs line `label`s
  with their drawn text; `button`s `Skip`, `Back`, `Done`. `Select` on a radio is the record
  `component edition` makes; `Invoke` on `Done` starts the installs, which an agent may do
  (an edition adds and never destroys, `editions-v0.4.md` section 8).

## 6. Every device class (D23)

- **Phone** (one column, 1.6's layout; `W` 390, `C` 358): the step label at (16, 56); radio
  rows 48 tall at y 88, 156, 224, 292 (a 48 px row whose whole width is the target, the
  summary line under each at `row + 48`... as a 68 px pitch: radio row `(16, y, C, 48)`, the
  summary at cell top `y + 50`, 20 tall); the size right-aligned at `16 + C` on the radio
  row; the installs lines at cell tops 364 and 384, wrapping to a second line each when wider
  than `C` (R9); the page scrolls; the buttons as 1.6's bottom block; the phone's `Back`
  input is `Back`.
- **Tablet**: the desktop layout with 40 px radio rows (rows 64, 124, 184, 244; the lines at
  312 and 332).
- **TV** (scale 2): everything doubles; the ring on the focused radio; the remote's Up/Down
  select, OK selects, Right then OK reaches `Done`; nothing needs a pointer.
- **Headless**: no page; `component edition` is the capability.

## 7. Acceptance (host tests, 640x440, scale 1x, window coordinates; a fake store offering the four components with the sizes of `editions-v0.4.md` section 10 plus `desktop` 0 (installed) and `builtin-apps` 0 (installed), 1.2 GB free)

- Page 4 after three `Next`s: heading `Which edition?` at (16, 16), `Step 4 of 4` ending at
  624; radios at (16, 70), (16, 126), (16, 182), (16, 238) with `Personal` selected; sizes
  `3.2 MB`, `3.4 MB`, `3.4 MB`, `Installed` ending at 624 at cell tops 76, 132, 188, 244;
  the summaries at (40, 96), (40, 152), (40, 208), (40, 264); 4.10 at (16, 300) `Installs
  Windows apps, Mac apps: 3.2 MB.`, 4.11 at (16, 320) the downloads sentence; S3 at (16,
  356); `Done` at (528, 396) primary, `Back` enabled.
- Down: `Professional` selected, `/var/lib/eggos/edition` holds `professional` (the fake
  records it), 4.10 names `Management · 0.2 MB` too and `3.4 MB`; End: `Server`, `Nothing to
  add.` and the only-adds line; Home: `Personal`.
- `Done` sends `InstallEdition("personal")` and closes; the fake manager receives the plan
  `windows-apps`, `mac-apps` in order; `Skip` and the close control do the same; `Back`
  sends nothing and shows page 3.
- 0.6 MB free: 4.10 reads the not-enough-space line in `text`, 4.11 the free-space sentence;
  `Done` records and sends no install. No store and offline: the names line and the offline
  sentence. A policy fake: the radios disabled, `Managed by Contoso IT.`.
- A re-run with `enterprise` recorded: `Enterprise` selected on show.
- `wm show` lists the group, the four radios with `selected` on one and values `personal`...,
  the labels and the three buttons; `select` on `Professional` records it.
- Phone 390x752: the rows and lines of section 6; TV 960x540 at scale 2: the ring on
  `Personal`, OK then Right, Right, Right, OK (or Enter) ending the window with the install
  sent.

## Decided by design (the owner can overrule any of these)

1. The edition is the last page, after the time zone, because it starts downloads.
2. Four radios with a summary line each and the size at the right, not cards: there is no
   picture to show, and four names read in one glance.
3. `Personal` is the default at a display; `Server` is offered, last.
4. The choice is recorded at once; the installs start at `Done` (and `Skip`, and the close),
   in the background, with progress in the Downloads window and the Components pane only.
5. Not enough disk or no store offline refuses in the page's line, records the choice and
   installs nothing; the Components pane picks it up later.
6. No new command or tool: `component edition` and `component_edition` are the capability.

## Later

Editions that remove what they do not include; a per-component tick list under the chosen
edition ("install these now, those later"); the installer asking the edition before the
first boot so the base image is written with it (D69); a picture per edition.

## Decisions for the engineer

1. `FirstRunWindow` page 4, S2 `Step <n> of 4`, page 3's primary `Next`, page 4's `Done`;
   widgets 4.1 to 4.11 at section 2's places; rows 56 apart from y 64.
2. On show: the recorded edition from `/var/lib/eggos/edition` or `personal`; the free space
   of the components volume; the plan per edition by the dry-run function; the lines of
   section 3's table from `messages`.
3. `Request::SetEdition(name)` on every selection (records only); `Request::InstallEdition
   (name)` on `Done`, `Skip` and the close, then close; nothing on `Back`; refusals as 1.4.
4. The manager runs the plan through the Components pane's install path with Downloads
   entries and the end notice `<Title> installed.` / the failure line.
5. Keys of section 4; elements of section 5; the phone, tablet and TV numbers of section 6.
