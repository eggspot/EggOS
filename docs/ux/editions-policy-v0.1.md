# EggOS editions' own settings v0.1: the update policy and managed policies: design brief

Status: design for D258's leftover ("editions' own settings (updates, policies) for
professional and enterprise") and `editions-v0.4.md`'s ("each edition's own settings").

Design by Claude Fable (D16). It extends `editions-v0.4.md` and `components-v0.5.md`, and
everything there stands: the Edition band, the Updates band's line, checkbox and button, the
daily check, the `Updates` chip, the rows, the notes and every sentence. This brief adds what
the Professional and Enterprise editions bring of their own: an **update policy**
(`Automatic`, `Notify only`, `Managed`), **managed policies** read from `/etc/eggos/policy`,
the look of a setting a policy locks, the `settings` command's and the MCP tools' view of
locked keys, and what an agent may never change. A machine without them is byte for byte
`components-v0.5.md`'s. One capability, three front-ends (D22): sections 6 and 7 name the
command and the tool behind everything a pane shows. Numbers are px at 1x in pane coordinates
(origin window x 176, y 0; 464 wide, right edge x 448) and double at 2x. Nothing moves.

## 0. What a person gets, and how that is checked

- With Professional or Enterprise, **`Install updates`** in Settings > Components chooses
  between `Automatic` (EggOS installs component updates by itself), `Notify only` (it checks
  and tells you, as today) and `Managed` (it does neither; whoever manages the computer runs
  the updates).
- An administrator, or a management tool, writes **one text file**, `/etc/eggos/policy`. Each
  line sets a setting and **locks** it.
- A locked setting shows its value, greyed, with **a lock** beside it, and the pane's first
  line says **who manages it**: `Managed by Contoso IT`. A new pane, **Management**, lists
  every locked setting in one place.
- `settings policy` prints the same list; `settings set` on a locked key says who manages it;
  agents get the same refusal and can never lift a lock.
- Host tests drive the panes, the command and the tools against a fake policy file (section
  11).

## 1. Where these settings come from: the `management` component

Editions are lists of components (D258) and nothing is installed beyond the base (D18), so the
editions' own settings are a component too:

- **`management`**, title `Management`, summary `Policies an administrator sets, and how
  updates are installed.` It is in **Professional's and Enterprise's** lists and not in
  Personal's or Server's. `component install management` installs it on any edition: nothing
  here is closed to anyone.
- **Not installed** (Personal, Server, or removed): no `update.policy` key, no `Install
  updates` row, no Management pane, and `/etc/eggos/policy` is **not read**: nothing is locked
  and the cost is nothing.
- **Installed**: the key, the row, the pane, and the policy file is honoured. `update.policy`
  starts at `notify`, so installing the component changes nothing until someone chooses.
- The edition confirmation (`editions-v0.4.md` section 3) lists it like any component:
  `Management · 0.2 MB`.
- **Removing it** is v0.3's remove with one more line in its confirmation: `Policies in
  /etc/eggos/policy stop applying.` It needs root, like every remove.

A policy is the administrator's word to people without root and to agents. Root can edit the
file or remove the component, as on every system; a lock is not a defence against root.

## 2. The update policy

One key, **`update.policy`**, with three values. It governs component updates, the only
updates there are (`components-v0.5.md`).

| | `automatic` (`Automatic`) | `notify` (`Notify only`) | `managed` (`Managed`) |
|---|---|---|---|
| the daily check | on, and cannot be turned off | as v0.5: `Check every day` | off |
| when the check finds updates | EggOS installs them, one after another, in the background | the band and the chip say so | |
| the `Updates` chip on the bar | only while an automatic update has failed | as v0.5 | never |
| `Check now` | checks, then installs | checks | disabled |
| `Update`, `Update all` | work | work | disabled, tooltip `Updates are managed` |
| `component update` | works | works | refused (section 6) unless `--as-manager` |
| a restart | never forced: `Updated. Running programs use it after a restart.` | | |

**The Updates band's line** (widget 51) gains these states; the rest are v0.5's:

| State | Widget 51 |
|-------|-----------|
| automatic, just updated | `Updated 2 components · today at 09:16` (`1 component`) |
| automatic, updating | `Updating 1 of 2: Windows apps` (v0.5's) |
| automatic, the last attempt failed | `2 updates · 0.7 MB to download` (v0.5's), and a note under the band: `Could not update automatically: <reason>.` over `It tries again with the next check.` |
| managed, chosen here | `Managed: EggOS installs no updates by itself` |
| managed, locked by a policy | `Managed by Contoso IT` |

`Check every day` (widget 52) is checked and disabled under `automatic` (tooltip `Automatic
updates check every day`), unchecked and disabled under `managed` (tooltip `Updates are
managed`), and v0.5's under `notify`. An automatic update is v0.5's `Update all`, started by
the system instead of a press: the same jobs, the same notes if the pane is open, the same
swap, the same failures; the first failure stops the rest until the next check.

## 3. `Install updates` in the Components pane

Shown only while `management` is installed. The Updates band grows by one row and everything
under it moves down 36:

| Block | v0.5 | With `management` |
|-------|------|-------------------|
| Updates band (50) | (16, 124, 432, 48) | (16, 124, 432, **84**) |
| Edition band (40) | (16, 180, 432, 64) | (16, **216**, 432, 64) |
| `ROWS_TOP` | 252 | **288**; row `r` at `288 + 48 r` |
| the notes' anchors | 116, 172, 244, `252 + 48 (r + 1)` | 116, **208**, **280**, `288 + 48 (r + 1)` |

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 61 | label | (24, 54) in the band | `Install updates` in `text` |
| 62 | dropdown, name `Install updates` | (272, 48) in the band, 160x28 | `Automatic`, `Notify only`, `Managed`; ends at pane x 448 |

`WIDGETS` becomes 62. Tab order: `Check now` / `Update all`, `Check every day`, **`Install
updates`**, the Edition dropdown, the rows. The dropdown is the toolkit's
(`toolkit-v0.4.md` 3.1). Choosing a value saves `update.policy` and takes effect in the same
frame: the band's line, the checkbox and the buttons change as section 2 says. **No
question is asked**: the choice is undone by choosing again, and the band's line says plainly
what `Managed` means. While a component installs, updates or is removed the dropdown is
disabled with v0.5's tooltips.

Search words added to `Components`: `automatic updates`, `auto update`, `update policy`,
`notify`, `managed`, `Windows Update`, `Software Update`.

## 4. The policy file

**`/etc/eggos/policy`**: UTF-8 text, one `key=value` a line, the settings file's own grammar.

```
policy.manager=Contoso IT
policy.contact=helpdesk@contoso.example
update.policy=managed
appearance.theme=dark
time.automatic=on
accessibility.high_contrast=false
agents.grants=view write
agents.deny=disk_format disk_install component_remove
```

| Line | Meaning |
|------|---------|
| `policy.manager=<name>` | who manages the computer, 32 bytes at most; shown as `Managed by <name>`. Without it: `Managed by your administrator` |
| `policy.contact=<text>` | where to ask, 64 bytes at most; shown beside the name |
| `<setting>=<value>` | any key `settings list` marks settable, with a value `settings set` would take: the setting **has that value and is locked** |
| `agents.grants=<classes>` | the most any agent may be granted: some of `view write install run`, or empty for none |
| `agents.deny=<tools>` | MCP tools no agent may call, by name |
| a line starting with `#`, an empty line | the administrator's note; skipped |

- **Trusted only when** the file is a regular file owned by root, not writable by group or
  others, in a folder with the same properties, 16 KiB and 256 lines at most, and UTF-8.
- **A line that is wrong** (no such setting, a read-only key, a value the setting does not
  take, a name too long, a key given twice: the last wins) is **ignored and reported**; the
  other lines apply. The reason is `settings set`'s own (`invalid value 'blue' (dark or
  light)`).
- **A file that cannot be trusted** locks nothing, so a person keeps a working computer, and
  holds **every agent to `view`** until it is fixed, the careful side for automation (D193's
  stance on a grants file it cannot trust). The Management pane and `settings policy` say
  why.
- **When it is read**: before every `settings set`, `settings_set` and agent call (a lock
  holds from the next call, as grants do, D193), each time a Settings pane is shown, and at
  boot, after the settings file, so the first frame already has the locked values.
- **When its values are applied** to the running system: at boot, when Settings is shown, and
  by **`settings policy --apply`** (root), which a management tool runs after writing the
  file. Until then a changed value is shown as pending (sections 5.3 and 6); the lock itself
  never waits.
- EggOS never writes the file.

## 5. How a locked setting looks

### 5.1 The pane's banner: who manages it

A pane that holds at least one locked setting gets a **banner** as its first 28 px, and the
pane's own content moves down 28 (every pane coordinate + 28; at the minimum window the pane
scrolls 28 more in the window's scroll view).

| Part | Place in the pane | Look |
|------|-------------------|------|
| banner | (0, 0, 464, 28) | `menu.hover` fill; its bottom row (y 27) 1 px `menu.border` |
| lock | cell (16, 6), 16x16 | section 5.2's glyph in `text` |
| name | (40, 6) | `Managed by Contoso IT` in `text` |
| contact | right-aligned, ending at x 448, cell top 6 | `helpdesk@contoso.example` in `text`; hidden when it would come within 16 px of the name |

Four window-level widgets (`CAPACITY` + 4), placed by the window, not by each pane. The
banner takes no focus and no input. Panes with a banner: any pane with a locked key
(`Appearance`, `Display`, `Keyboard`, `Mouse`, `Accessibility`, `Power`, `Date & time`),
`Components` when `update.policy` is locked, and `Agents` when the policy caps or denies.

### 5.2 The lock on the control

`WidgetTree::set_locked(id, bool)`: a locked widget is **disabled** (the toolkit's disabled
look, showing the policy's value, skipped by Tab, ignoring the pointer) and the tree draws a
**lock** beside it; no widget is added to any pane.

- **The glyph**, in a 16x16 cell, `text.dim`, 1 px strokes, doubled at 2x: the shackle's top
  is row 2, columns 5 to 10; its sides are rows 3 to 5 at columns 4 and 11; the body is rows
  6 to 12, columns 3 to 12, filled; the keyhole is rows 8 and 9, columns 7 and 8, left in the
  surface's colour.

  ```
  ................
  ................
  .....######.....
  ....#......#....
  ....#......#....
  ....#......#....
  ...##########...
  ...##########...
  ...####..####...
  ...####..####...
  ...##########...
  ...##########...
  ...##########...
  ................
  ................
  ................
  ```
- **Its place**: the cell's left edge 24 px left of the control's left edge, centred on the
  control's height (`control.y + (height - 16) / 2`). When the control has less than 24 px
  to its left inside its container, the cell goes 8 px right of the control instead. A
  radio group, a slider with its readout and a dropdown are each one control.
- **Its tooltip**: resting on a locked control or its lock for 600 ms shows `Managed by
  Contoso IT` (the tree's one lock owner text, `set_lock_owner`), as a disabled widget's
  tooltip shows today.
- A setting a pane shows as a fact (a read-only line) gets no lock: it could never be changed
  there.
- **In the Agents pane**, a permission above `agents.grants` shows `Deny`, locked; the status
  line reads `Managed: no agent gets install or run.` A tool in `agents.deny` is listed in the
  Management pane only.
- **In Components**, a locked `update.policy` locks widget 62 (the lock at (248, 54) in the
  band) and, as section 2 says, widgets 52 and 53.

### 5.3 The Management pane

A pane named **`Management`**, right after `Components` and before `About`, shown only while
the component is installed. It is read-only: the file is the administrator's.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Management` |
| 2 | label | (16, 64) | `Managed by Contoso IT` in `text` |
| 3 | label | (16, 84) | the contact, `text.dim`; hidden when none |
| 4 | label | (16, 104) | `/etc/eggos/policy · 4 settings locked` in `text.dim` |
| 5 | table, name `Locked settings` | (16, 132), 432x148 | columns `Setting` 216, `Value` 104, `Pane` 112; five rows show; empty text `No setting is locked.` |
| 6 | label | (16, 288) | `Agents: at most view write. Denied: 3 tools.` in `text.dim`; hidden when the policy says nothing about agents |
| 7 | label | (16, 308) | the status line, `text` (below); hidden when there is nothing to say |

- **Rows**: one a locked setting, sorted by key; `Setting` its key, `Value` its value (`dark`,
  and `dark (pending)` while the running value differs), `Pane` the pane that shows it or
  empty. Enter or a double-click on a row shows that pane. Tab reaches the table alone.
- **No file**: widget 2 `Not managed`, widget 4 `No policy file at /etc/eggos/policy.`, the
  table's empty text, widgets 6 and 7 hidden.
- **Not trusted**: widget 2 `The policy file cannot be trusted`, widget 4 the reason and the
  effect: `Others can write it. Nothing is locked; agents can only read.` (`It is not owned by
  root.`, `It is larger than 16 KB.`, `It is not UTF-8 text.`, `It is not a regular file.`).
- **Ignored lines**: the status line `2 lines ignored: settings policy lists them.` (`1 line
  ignored`).
- **Pending values**: the status line `Some values are not applied yet: settings policy
  --apply.`; ignored lines win the line when both are true.
- Search words: `managed`, `management`, `policy`, `policies`, `organization`,
  `organisation`, `administrator`, `admin`, `locked`, `lock`, `MDM`, `Group Policy`, `work`,
  `school`.

## 6. The `settings` command

- **`settings policy [--json]`** (new verb; the registry's `SETTINGS_POLICY`, effect `read`):

  ```
  Managed by Contoso IT (helpdesk@contoso.example)
  /etc/eggos/policy: 4 settings locked
  accessibility.high_contrast=false
  appearance.theme=dark
  time.automatic=on
  update.policy=managed
  agents: at most view write; denied disk_format disk_install component_remove
  ```

  then one line an ignored line, `ignored line 9: apperance.theme: no such setting`, and
  `(pending)` after a value not applied yet. No file: `Not managed: no policy file at
  /etc/eggos/policy.` Not trusted: `The policy file cannot be trusted: others can write it.
  Nothing is locked; agents can only read.` Without the component: `Not managed: the
  management component is not installed.` Status 0 in every case: the view succeeded.
  `--json`: `managed`, `trusted`, `reason`, `manager`, `contact`, `path`, `locked` (`key`,
  `value`, `pane`, `pending`), `agents` (`grants`, `deny`), `ignored` (`line`, `text`,
  `reason`).
- **`settings policy --apply`** (root; effect `change`): applies every locked value the
  running system does not have yet, and prints `applied 2 settings` (`nothing to apply`).
- **`settings list --json`, `settings get --json`**: a locked key's record is
  `{"key":"appearance.theme","value":"dark","settable":false,"locked":true,"managed_by":
  "Contoso IT"}`. A key that is not locked keeps today's record, byte for byte. The plain
  forms are unchanged (`key=value`; the value alone).
- **`settings set`** on a locked key, `--dry-run` too: status 1, `settings: appearance.theme:
  managed by Contoso IT`, then `  try: settings policy`; the coded error is **`managed`**
  (new; refused; hint `Ask whoever manages this computer. settings policy lists what is
  locked.`). Root gets the same answer: the file is the one place a locked value changes.
- **`update.policy`**: `settings get update.policy` prints `automatic`, `notify` or `managed`;
  `settings set update.policy automatic`; another value: `invalid value 'x' (automatic, notify
  or managed)`. Without the component: `no such setting`.
- **`component update`** under `managed`: `component: updates are managed by Contoso IT`
  (`managed`), also for `--dry-run`; **`--as-manager`** (root) runs it, for the tool that
  manages the machine. **`component update --daily off`** under `automatic`: `component: the
  daily check is part of automatic updates.`
- **`agent grant`** above `agents.grants`: `agent: run: managed by Contoso IT` (`managed`).

## 7. The MCP tools

| Tool | What changes |
|------|--------------|
| `settings_list`, `settings_get` | a locked key's record carries `locked` and `managed_by`, and `settable` is false |
| `settings_set` | a locked key answers `managed`; `update.policy` joins the keys it takes |
| `settings_policy` (new, the view grant) | the record `settings policy --json` prints; no `apply` argument |
| `component_update` | under `managed` it answers `managed`; it has no `as_manager` argument |
| `component_list` | the record gains `update_policy` while the component is installed |
| every tool in `agents.deny` | answers `managed` before anything else is checked |

The grant prompt (`agents-v0.3.md`) never offers a class above `agents.grants`: the choice is
not drawn, and its line reads `Managed: no agent gets run.`

## 8. What an agent may not change

1. **A locked setting.** `settings_set` answers `managed`; `wm act` on its control answers
   `Disabled`. No grant, no confirmation and no `run` grant changes that.
2. **The policy file.** Every file tool refuses a path in `/etc/eggos/` with
   `path_not_allowed`, and a program an agent starts with the `run` grant has no write access
   to that folder. There is no tool that writes or applies a policy.
3. **Its own ceiling.** No agent is granted a class outside `agents.grants`, by the pane, by
   `agent grant` or by the grant prompt; a grant an agent already holds above the ceiling
   stops counting from the next call.
4. **A denied tool.** A tool in `agents.deny` answers `managed` for every agent, the local
   one included.
5. **Who runs the updates.** An agent may set `update.policy` to `automatic` or `notify` with
   the write grant. Setting it to **`managed`** needs the person's confirmation (`confirm`,
   asked by elicitation, as a destroy tool's), because it stops updates. Under `managed` an
   agent cannot update, and cannot pass `--as-manager`.
6. **The component itself.** `component_remove` of `management` answers `managed` while a
   policy file exists, and otherwise needs the person's confirmation like every remove.
7. Every refusal is written to the agent log with the outcome `managed`, where the person and
   the administrator can read it.

What an agent **may** do: read everything (`settings_policy`, `settings_list`, the element
tree), so it can tell a person why something is greyed and who to ask.

## 9. Keys, focus and what an agent sees (D161)

- **Focus**: a locked control is disabled, so Tab and the arrows skip it; the banner is not a
  stop; nothing else in a pane's order changes. In Management the table is the one stop: Up,
  Down, Home, End, Enter (`toolkit-v0.5.md` section 4).
- **The banner** is a `group` named **`Managed`** with the value the manager (`Contoso IT`),
  holding the `label`s `Managed by Contoso IT` and the contact.
- **A locked control** keeps its role, name and value and is flagged `disabled`; under it a
  `tooltip` named `Managed by Contoso IT` is listed whether or not it shows, so an agent and a
  screen reader learn why without hovering.
- **`Install updates`** is a `combobox` named `Install updates` with its title as value and
  its three options.
- **Management**: the `label`s with their drawn text; a `table` named `Locked settings`,
  value `row 1 of 4`, its rows valued `appearance.theme\tdark\tAppearance`.
- The records come from `settings_policy`, not from the tree (D22: the tree is for operating
  the window).

## 10. Every device class (D23)

The reflow rule of `settings-forms-v0.5.md` applies; these are the parts it does not decide.

- **Phone** (pages): the banner is the pane page's first row under its top bar, the page's
  width, **48 tall**, the lock at (16, 16), the name at (48, 16), no contact; a tap on it
  opens the Management page. A locked control keeps its full-width row and its lock sits at
  the row's right end, 16 from the edge. `Install updates`: the Updates band grows from 132 to
  **208**, the label at (8, 132) and the dropdown at (8, 152), the band's width minus 16, 48
  tall; the Edition band starts at 340 and the rows at 464. Management: the three lines, then
  the table the page's width with 40 px rows, scrolling sideways, then the two lines.
- **Tablet**: the desktop layout with 40 px controls: the banner 40 tall (cell tops + 12);
  the Updates band 104 tall with the dropdown at + 56, 40 tall; the Edition band at 236; rows
  from 316.
- **TV** (scale 2, D-pad): everything doubles; the ring skips locked controls; the banner is
  always in view at the pane's top, which is how a remote's user reads who manages it, there
  being no hover. In Management the D-pad moves in the table and OK shows the row's pane.
- **Wearable**: read-only as it is; the list's first line is `Managed by Contoso IT` when a
  policy is trusted.
- **Headless**: `settings policy`, `settings set` and the tools are the whole capability; a
  server's administrator installs `management` by name.

## 11. Tokens

| Part | Token on token | D / DH / L / LH |
|------|----------------|-----------------|
| the banner's lock, name and contact | `text` on `menu.hover` | 10.5 / 15.1 / 14.0 / 16.8 |
| the banner's edge | `menu.border` | an edge; white and black in DH and LH |
| a control's lock | `text.dim` on `menu.bg` | 5.1 / 11.4 / 5.4 / 9.6 |
| a locked control | the toolkit's disabled look (`text.dim`) | 5.1 / 11.4 / 5.4 / 9.6 on `menu.bg` |
| Management's facts and the agents line | `text.dim` on `menu.bg` | 5.1 / 11.4 / 5.4 / 9.6 |
| Management's name and status line, `Install updates` | `text` on `menu.bg` | 12.9 / 21.0 / 16.0 / 21.0 |

The lock is a shape, the control is greyed and the banner is words: none of the three cues
is colour alone. No new token.

## 12. Acceptance (host tests, 640x440, 1x, window coordinates; `components-v0.5.md`'s fake B
with `management` installed; policy P is section 4's example, trusted)

- Without `management`: every case of `components-v0.5.md` section 11 passes unchanged;
  `settings get update.policy` answers `no such setting`; policy P on the disk draws no banner
  and locks nothing; `settings policy` prints the not-installed line.
- With it and no policy file: the Updates band is (192, 124, 432, 84), `Install updates` at
  (216, 178), the dropdown (464, 172, 160, 28) reading `Notify only`; the Edition band (192,
  216, 432, 64) with its dropdown (464, 220, 160, 28); row 0 at y 288. Tab from `Check every
  day` reaches `Install updates`, then the Edition dropdown.
- Choosing `Automatic`: the settings file holds `update.policy=automatic`; `Check every day`
  is checked and disabled; a daily check that finds two updates runs both jobs with no press,
  widget 51 reads `Updating 1 of 2: Windows apps`, then `Updated 2 components · today at
  09:16`, and no chip is drawn. A first job that fails with a digest mismatch leaves widget 51
  at `2 updates · 0.7 MB to download`, draws the chip, and opens the automatic note under
  the band at y 208.
- Choosing `Managed`: widget 51 `Managed: EggOS installs no updates by itself`; `Check now`
  and the rows' `Update` disabled; `Check every day` unchecked and disabled; no daily check
  runs and no chip is drawn; `component update` answers `managed` and `component update
  --as-manager` runs.
- With policy P, Components: the banner (176, 0, 464, 28) with (180, 10) `menu.hover` and row
  27 `menu.border`; the lock's body at (195, 12) `text` and its keyhole (199, 14)
  `menu.hover`; `Managed by Contoso IT` at (216, 6); the contact ending at x 624; the heading
  at (192, 44); the Updates band at (192, 152, 432, 84); the dropdown (464, 200, 160, 28)
  disabled reading `Managed`, its lock cell at (440, 206); widget 51 `Managed by Contoso IT`.
- With policy P, Appearance: the banner; the `Theme` radios at y 300, disabled, `Dark`
  selected; the lock cell at (483, 306) with (486, 312) `text.dim` and (490, 314) `menu.bg`;
  a click on `Light` returns `Nothing`; resting on the radios 600 ms shows the tooltip
  `Managed by Contoso IT`. The Display pane, with nothing locked, has no banner and its
  heading at (192, 16).
- Management: `PANES` holds it between `Components` and `About`; `Managed by Contoso IT` at
  (192, 64), the contact at (192, 84), `/etc/eggos/policy · 4 settings locked` at (192, 104);
  the table (192, 132, 432, 148) with rows `accessibility.high_contrast` / `false` /
  `Accessibility`, `appearance.theme` / `dark` / `Appearance`, `time.automatic` / `on` / `Date
  & time`, `update.policy` / `managed` / `Components`, the first row's cells at x 216, 416 and
  520, cell top 164; `Agents: at most view write. Denied: 3 tools.` at (192, 288); Enter on
  the second row shows Appearance. `policy`, `managed` and `group policy` find the pane.
- The file: a line `appearance.theme=blue` is ignored and listed with `invalid value 'blue'
  (dark or light)`, the other three settings stay locked, and the pane's status line reads `1
  line ignored: settings policy lists them.`; `system.cpus=8` is ignored as `read-only`; a
  33-byte manager is ignored and the banner reads `Managed by your administrator`. A file
  with mode 0666 locks nothing, draws no banner, makes Management read `The policy file
  cannot be trusted` over `Others can write it. Nothing is locked; agents can only read.`,
  and an agent with the write grant gets `needs_grant` from `settings_set`.
- The command: `settings policy` prints section 6's seven lines; `--json` has four `locked`
  records; `settings get --json appearance.theme` is the record of section 6; `settings get
  --json display.scale` is today's record; `settings set appearance.theme light` and its
  `--dry-run` exit 1 with `settings: appearance.theme: managed by Contoso IT`; after the file
  changes `appearance.theme` to `light`, `settings policy` shows `light (pending)` and
  `settings policy --apply` sends `SET_PALETTE` 1 and prints `applied 1 setting`.
- Agents: `settings_set` on a locked key answers `managed` with any grant; `disk_format`
  answers `managed`; a file write to `/etc/eggos/policy` answers `path_not_allowed`; `agent
  grant phone run` answers `managed` and the Agents pane shows `Run` at `Deny` with a lock;
  `settings_set update.policy managed` without `confirm` answers `needs_confirmation`;
  `component_remove` of `management` answers `managed`; each refusal is in the agent log with
  the outcome `managed`.
- A pane 328 wide draws the banner 48 tall and the Updates band 208 tall; the element tree
  holds the `group` `Managed` valued `Contoso IT`, a `tooltip` `Managed by Contoso IT` under
  the `Theme` radios, the `combobox` `Install updates` and the `table` `Locked settings`.
- The computed contrast of every pair in section 11 meets its floor in the four palettes.

## Decided by design (the owner can overrule any of these)

1. The editions' own settings are a component, `management`, listed by Professional and
   Enterprise and installable on any edition, rather than code switched on by an edition's
   name. If the owner wants them in the base instead, only section 1 changes.
2. `update.policy` starts at `notify` on every edition: nothing changes until someone
   chooses. Personal keeps `components-v0.5.md` as it is.
3. `Managed` is a value a person can choose, not only a state a policy puts the machine in;
   choosing it asks nothing, because choosing again undoes it.
4. Automatic updates install at once after the daily check and never restart anything.
5. The policy file uses the settings' own keys: a line both sets and locks. There is no
   second vocabulary to learn.
6. A wrong line is skipped and reported; an untrusted file locks nothing for people and
   holds agents to `view`.
7. `settings set` refuses a locked key for root too; the file is the one place it changes.
8. Who manages is said once a pane, in a banner; each locked control carries only the lock.
9. The lock is drawn by the toolkit for a locked widget, so no pane spends a widget on it.
10. One error code, `managed`, for every policy refusal.
11. Setting `managed` by an agent needs the person's confirmation; lifting any lock is never
    an agent's.
12. A policy is not a defence against root.

## Left out of v0.1

Updating EggOS itself (the base image) and a policy for it; deferring updates by days, update
hours, a metered-network rule; signed policy files and policies fetched from a server; a
policy per user; locking a control that is not a `settings` key (a pane's button, an app's
action); a time limit on a policy; telling the manager about refusals; first run choosing an
edition and its policy; editing the policy file from a pane.

## Decisions for the engineer

1. Component `management` (title, summary) in `editions.rs`'s Professional and Enterprise
   lists; installed is one cheap question that every part below asks first.
2. Key `update.policy` (`automatic`, `notify`, `managed`; default `notify`), listed only while
   the component is installed; the behaviours of section 2's table; `component update
   --as-manager`; the automatic run after the daily check; the chip only for a failed
   automatic update.
3. Widgets 61 and 62 with their places; the Updates band 84 tall, the Edition band at 216,
   `ROWS_TOP` 288, the notes' anchors 208 and 280; `WIDGETS` 62; widget 51's new lines and
   widget 52's and 53's states.
4. The policy file: path, grammar, the reserved keys (`policy.manager` 32 bytes,
   `policy.contact` 64, `agents.grants`, `agents.deny`), the trust rule, ignored lines with
   their reasons, the 16 KiB and 256-line limits, read before every set and call, applied at
   boot, on Settings shown and by `settings policy --apply`.
5. The banner: (0, 0, 464, 28), `menu.hover`, `menu.border` bottom row, lock at (16, 6), name
   at (40, 6), contact right-aligned at 448, the pane's content + 28; four window-level
   widgets.
6. `WidgetTree::set_locked(id, bool)` and `set_lock_owner(&str)`: disabled, the 16x16 glyph of
   5.2 in `text.dim` 24 px left of the control (or 8 px right), the tooltip.
7. The Management pane: widgets 1 to 7 with their places, the table's columns 216, 104, 112,
   its states and search words; after `Components`, before `About`, only while installed.
8. `settings policy [--json]`, `settings policy --apply`, the locked record's `locked` and
   `managed_by` with `settable` false, the refusal's words, the error `managed`.
9. MCP: `settings_policy`; `managed` from `settings_set`, `component_update`, denied tools
   and capped grants; `confirm` for setting `managed`; `update_policy` in `component_list`.
10. Agents: the ceiling applied to the pane, `agent grant` and the grant prompt; `/etc/eggos/`
    closed to the file tools and to programs started with the `run` grant; `component_remove`
    of `management` refused while a policy file exists; the agent log's outcome `managed`.
11. The phone, tablet, TV and wearable forms of section 10; the element names of section 9.
