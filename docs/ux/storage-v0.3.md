# EggOS v0.3.0 Storage: design brief (slice 40b)

Design by Claude Fable (D16) for the GUI front-end of the `disk` capability (D22, D192): the
Settings `Storage` pane that lists the machine's disks and checks, repairs and formats an
unmounted EggFS disk, with the same data and words as `disk list`, `disk check [--repair]
<device>`, `disk format [--label NAME] [--dry-run] [--yes] <device>` and the MCP tools
`disk_list`, `disk_check`, `disk_format`. Everything in `toolkit-v0.3.md` (section 11 for
Settings), `desktop-v0.3-gui.md` section 10 and `keyboard-v0.3.md` (the list-and-facts pane this
one is shaped like) stands; numbers are px at 1x and double at 2x; the only motion is the
caret's blink. The kernel attaches at most 4 disks, so the pane is built for 1 to 4 rows.

## 1. The pane in the sidebar

- **Name** `Storage` (Windows' and macOS's word; `Disks` is GNOME's app). Its row comes right
  after `Mouse`, so the hardware panes stay together and the two about the system itself stay
  last: `Appearance`, (`Network`,) `Display`, `Keyboard`, `Mouse`, `Storage`, `Date & time`,
  `Components`, `About`. Without Network its row is y 172..203 (44 + 4 x 32) and `Date &
  time`, `Components` and `About` each move down 32 (204, 236, 268; `DATE_AND_TIME` is index 5,
  `ABOUT` 7); with Network's row too, everything is 32 lower again.
- **Search words**: `disk`, `disks`, `drive`, `drives`, `volume`, `storage`, `space`, `free
  space`, `format`, `erase`, `check`, `repair`, `fsck`, `chkdsk`, `Disk Utility`, `Disk
  Management`, `Disks`, `EggFS`, `label`, `partition`, plus its button names `Check`, `Repair`,
  `Format`. `disk` and `storage` also match About (devices); nothing else collides.
- **Budget**: the tree holds 52 widgets and all 52 are used. This pane is its container plus
  exactly 24 widgets (16 in the pane, section 2, and 8 in the format panel, section 5), so
  `CAPACITY` becomes **77** (the installer's `eggos_app::dialog::CAPACITY` stays equal to it).
  Once Network's 16 and About's 3 (`devices-v0.3.md`) land as well, 96; raise it once to 96 now
  rather than three times.

## 2. Layout (pane coordinates: origin window x 176, y 0; 464 wide at the default size, so the
right-aligned edge is x 448; usable height 332, the pane's height at the minimum window)

Every widget has one fixed place; a state shows, hides, enables or retitles widgets and never
moves them. The list of disks is on the left and the selected disk's facts on the right, as the
Keyboard pane's list and facts are; the actions and the report run the full width under them.

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Storage` |
| 2 | list, 4 rows of 24, mark x 8, text x 24 | (16, 64), 208x96 | the device names, section 3; empty text `No disks attached.` |
| 3 | label | (240, 64) | contents line: `EggFS "Photos"` in `text` (section 3) |
| 4 | label | (240, 84) | size line: `1.0 GB · 312 MB used · 712 MB free` in `text` |
| 5 | label | (240, 104) | place line: `Mounted at /home` or `Not mounted` in `text` |
| 6 | label | (240, 124) | `Read-only` in `text`; hidden when the disk accepts writes |
| 7 | progress | (240, 148), 208x8 | the used share, `used x 1000 / size` permille; hidden when used is unknown |
| 8 | push button | (16, 172), 96x28 | `Check` |
| 9 | push button | (120, 172), 96x28 | `Repair` |
| 10 | push button | (224, 172), 96x28 | `Format...` (three periods) |
| 11 | label | (16, 212) | the status line, section 4, `text.dim` or `text` |
| 12-16 | labels | (16, 232), (16, 252), (16, 272), (16, 292), (16, 312) | report lines, section 4, `text`; hidden until a report exists |

The list's four rows are the static strings `/dev/vda`, `/dev/vdb`, `/dev/vdc`, `/dev/vdd`
(the kernel's names, `vda` first); the rows of disks that are not attached are not shown
(`set_shown_items`), so a machine with one disk shows one row and no empty space is drawn under
it (the list's surface is the pane's). The facts at x 240 are 208 wide; the widest realistic
size line, `999.9 GB · 999.9 GB used · 999.9 GB free`, is about 230 px, so it is ellipsised at
448 and the CLI has the exact numbers; the usual `1.0 GB · 312 MB used · 712 MB free` is
about 190. Buttons are 8 apart; the last ends at x 320. The report's five lines end at y 328,
inside 332: nothing scrolls at the minimum window, and at a taller window nothing moves.

## 3. The rows and the facts: one set of words for the pane, the CLI and MCP

The program reads the disks once each time the pane is shown and again after every action
(never on a tick: no hot-plug in 0.3.0), through the same function `disk list` prints and
`disk_list` returns: for each disk its `device`, `size_bytes`, `read_only`, `file_system`
(`eggfs`, `fat`, `blank`, `unknown`), `label` (empty when none), `mounted_on` (`/home`,
`/work`, `/media/host` or `null`), `used_bytes` and `available_bytes` (`null` when unknown:
an unmounted disk that is not EggFS). `disk list` prints one line a disk, `DEVICE  SIZE  RW
FS  LABEL  MOUNTED  USED  FREE`, with the words below; `--json` the record. Sizes everywhere are
Files' `size_text` (`1.0 GB`, `312 MB`; move it to the shared crate, as `network-v0.3.md`
asked); `-` where a value is unknown.

| Fact | Pane | `disk list` |
|------|------|-------------|
| device | `/dev/vdb` (the row) | `/dev/vdb` |
| file system and label | `EggFS "Photos"`; `EggFS, no label`; `FAT (the host's shared folder)`; `Blank`; `Unknown contents` | `EggFS` `"Photos"`; `EggFS` `-`; `FAT` `-`; `blank` `-`; `unknown` `-` |
| sizes | `1.0 GB · 312 MB used · 712 MB free`; unknown: `1.0 GB · space unknown` | `1.0 GB` ... `312 MB` `712 MB`; `-` `-` |
| place | `Mounted at /home`; `Not mounted` | `/home`; `-` |
| writes | `Read-only` (widget 6 shown) or nothing | `ro` or `rw` |

The label is shown between straight quotes exactly as stored (UTF-8, up to 64 bytes; glyphs
Egg Sans lacks draw `?`), ellipsised at the column's end. `free` is `available_bytes`, the same
number Files' status bar and `df` show (`files-free-space-v0.3.md`). The `·` is U+00B7. The
first attached disk is selected when the pane opens; selection is kept while the pane is
shown and reset when it is shown again. The sidebar's `Storage` row appears whether or not a
disk is attached: with none, the list shows its empty text, widgets 3 to 10 are hidden and the
status line reads `No disk attached: nothing to check or format.`

## 4. The actions, their rules and the status line

**Which buttons are enabled**, decided from the selected disk alone, with the status line
(widget 11, `text.dim`) giving the reason whenever any button is disabled:

| Selected disk | Check | Repair | Format... | Status line while idle |
|---------------|-------|--------|-----------|------------------------|
| mounted (`/home`, `/work`, `/media/host`) | off | off | off | `In use at /home: a mounted disk cannot be checked, repaired or formatted in this version.` |
| unmounted EggFS, writable, no report yet | on | off | on | `Check reads the disk and changes nothing. Repair is enabled when a check finds something to repair.` |
| unmounted EggFS, writable, last check found repairable problems | on | on | on | the report's headline (below) |
| unmounted EggFS, read-only | on | off | off | `Read-only disk: it can be checked, not repaired or formatted.` |
| unmounted, blank, FAT or unknown, writable | off | off | on | `No EggFS file system on the disk: nothing to check. Format... makes it one.` |
| unmounted, blank, FAT or unknown, read-only | off | off | off | `Read-only disk with no EggFS file system: nothing can be done to it.` |

**Repair without a dialog, after a check.** `disk check --repair` has effect `change`
(`command-model.md`: a change never asks), so the pane asks nothing either; but a repair writes
to a disk that may be damaged, so the pane sequences it: `Repair` is enabled only after `Check`
on the same disk has listed at least one `repairable` finding. The read-only check runs the
same code against an in-memory overlay and predicts the repair exactly (D192), so the check's
report is the preview a confirmation dialog would show, and the user has read it before
`Repair` can be pressed. The CLI and MCP keep their own rule (no prior check required): one
capability, and the GUI orders its two steps. `Format...` always opens the panel of section 5.

**Running.** On `Check` or `Repair` the status line shows `Checking /dev/vdb...` or
`Repairing /dev/vdb...` in `text`, the report lines are hidden, the list and the three buttons
are disabled, and the frame is committed before the work starts; the sidebar still works.
A 1 GiB disk takes well under a second on QEMU; no progress bar, because the checker reports
no progress (listed in "Left out").

**The report** (the headline in widget 11, `text`, then up to five lines in widgets 12-16):

| Result | Headline |
|--------|----------|
| clean | `Clean. 262,144 blocks, 182,000 free.` |
| check, problems | `Found 3 problems: 2 repairable, 1 not. 262,144 blocks, 182,000 free.` (`1 problem`; `, 0 not` omitted) |
| repair | `Repaired 2 problems; 1 not repairable. 262,144 blocks, 182,000 free.` (`; 0 not repairable` omitted) |
| the disk is not EggFS | `No EggFS file system on the disk.` |
| the disk failed | `The disk could not be read or written.` |
| in use (raced a mount) | `In use at /home: nothing was done.` |
| not root | `Not allowed: only root may open disks.` |

Block counts use Files' `bytes_text` thousands separators. Each finding is one line,
`<Outcome> · <where> · <description> (<code>)`: the outcome word first (`Repairable`,
`Repaired`, `Not repairable`), then `inode 17`, `block 4,096`, `inode 17, block 4,096` or
nothing when both are 0, then the checker's own description and its stable kebab-case code
from `Problem::description()` and `Problem::code()`, for example `Repairable · inode 17 · a
file counts its names wrongly (link-count)`; end-ellipsised at 448 (the code is in `disk
check`'s full output). Findings are shown in the checker's order; when there are more than
five, the fifth line reads `and 4 more: disk check /dev/vdb lists them all.` `disk check`
prints the same headline, then one finding a line as `<code>  inode <n>  block <n>
<outcome>  <description>`, and `--json` the records (`code`, `description`, `inode`, `block`,
`outcome` of `repairable|repaired|not-repairable`, plus `blocks`, `free_blocks`, `clean`);
`disk_check` returns them. The report stays until the next action on any disk, a change of
selection, or the pane being shown again (a stale result is never kept); selecting another
disk puts that disk's idle line back.

## 5. Format...: the confirmation panel

A modal panel inside the pane, drawn as Files' dialog panel (`desktop-v0.3-gui.md` 8.3): 400
wide, `menu.bg`, `INSET_R6`, 1 px `menu.border` inside, a 2 px `field.bg` edge outside,
centred in the pane (x 32; y `(pane height - 204) / 2`, 64 at the minimum window), 204 tall.
While it shows the pane's widgets ignore pointer, wheel and Tab, and the list draws unfocused.

| # | Widget | Place in the panel | Text |
|---|--------|--------------------|------|
| 17 | panel | (0, 0), 400x204 | |
| 18 | label | (16, 16) | `Format /dev/vdb?` in `text` |
| 19 | label | (16, 36) | `Erases everything on it: EggFS "Photos", 1.0 GB.` in `text.dim` (`Blank, 1.0 GB`, `FAT, 1.0 GB`, `Unknown contents, 1.0 GB`) |
| 20 | label | (16, 56) | `This cannot be undone. Type vdb to confirm.` in `text.dim` |
| 21 | text field | (16, 84), 368x28 | placeholder `Label (optional)`, `escape: Clear` |
| 22 | text field | (16, 120), 368x28 | placeholder `Type vdb to confirm`, `escape: Clear` |
| 23 | push button | (176, 160), 96x28 | `Cancel` |
| 24 | push button, primary | (280, 160), 96x28 | `Format`; the window's default button while the panel shows |

- **Format** is disabled until field 22 holds exactly the device's short name (`vdb`,
  case-sensitive, spaces trimmed); typing it enables the button in the same keystroke. Enter in
  either field goes to `Format` when it is enabled, else nothing. Esc clears the focused field;
  Esc in an empty field, `Cancel` or closing the window cancels: the panel hides, nothing is
  written, the focus returns to `Format...`. Focus opens on the label field; Tab goes label
  field, confirm field, `Cancel`, `Format`, wrapping; `Set`'s default-button role of the Date &
  time pane is not affected (one default button at a time: the pane's buttons are none).
- **The label** is `--label`: trimmed, up to 64 bytes of UTF-8, no NUL (the field cannot type
  one). Over 64 bytes, `Format` is disabled too and line 20 reads `The label is too long: 64
  bytes at most.` in `text` until it fits; the CLI's `disk format` refuses the same label with
  `label-too-long` and a NUL with `label-invalid`.
- **On Format** the panel's controls are disabled and line 20 reads `Formatting /dev/vdb...`;
  when it is done the panel hides, the disks are re-read, the same row stays selected and now
  reads `EggFS "Photos"` / `1.0 GB · 1.1 MB used · 1,022 MB free` / `Not mounted`, the status
  line reads `Formatted /dev/vdb as EggFS "Photos".` (`as EggFS, no label`) in `text`, and the
  report lines are hidden. A failure keeps the panel open with line 20 showing the error in
  `text`: `The disk could not be written.`, `In use at /home: nothing was done.`, `Read-only
  disk: nothing was done.`, `Not allowed: only root may open disks.`
- **The CLI and MCP** confirm the same way: `disk format` is effect `destroy`, so it asks `Type
  vdb to format /dev/vdb (erases EggFS "Photos", 1.0 GB):` on a terminal and refuses with
  `needs_confirmation` without one; `--yes` answers it, `--dry-run` prints what would be erased
  and writes nothing. `disk_format` takes `device`, `label`, `dry_run` and the `confirm`
  argument the model gives every `destroy` tool (elicitation where the client supports it).
  The erased-contents sentence is the same function in all three.

## 6. Keys, focus and pointer

- Tab from the sidebar list enters the pane at the disk list, then `Check`, `Repair`,
  `Format...` (disabled ones skipped), then wraps to the search field; Shift+Tab the reverse.
  Showing the pane does not move the focus.
- In the list: Up/Down, Home/End move the selection (one row is always selected when there are
  rows); a change of selection updates widgets 3 to 11 in the same frame and hides the report.
  Enter, Space and a double-click do nothing: the row has no open action.
- Space or Enter on a focused button presses it; Enter elsewhere in the pane does nothing (no
  default button while the panel is hidden, so Enter in the search field never runs a check).
  Esc goes to the window, which ignores it. No new shortcuts: the panes have none in 0.3.0.
- Pointer: `Arrow` everywhere but the two fields (`IBeam`); a click selects and focuses the
  list; a right-click does nothing.

## 7. Every device class (D23)

- **One column** when the pane is narrower than 448 (a phone's maximised window, where the
  work area clamps the 640 minimum): the list keeps (16, 64) and takes the pane's width minus
  32; the facts move under it, x 16, from 20 px below the list's end (lines at pitch 20, the
  progress bar 8 under the last line, full width minus 32); the buttons are stacked full
  width, 8 apart, under the bar; then the status line and the report. The pane's panel grows to
  its content and scrolls in the window's existing scroll view (wheel 72 px; the focused
  widget is kept in view). The format panel takes the pane's width minus 16 (8 each side) and
  sits at the pane's bottom, as a sheet; its fields and buttons keep their 16 px padding, the
  two buttons sharing the last row.
- **Touch**: list rows 40 tall (text cell top +12, `>` at x 8), buttons 40 tall, fields 28 as
  everywhere; the list then is 160 tall at four rows and everything below moves down 64.
- **TV (scale 2)**: everything doubles; the ring on the selected row and the focused button is
  the cue; the whole pane, the panel included, works by arrows, Tab, Enter and Esc.
- **Keyboard-only** on every form factor: the list is one Tab stop; the confirm panel needs
  only typing and Enter.

## 8. What an agent sees (D161, `wm show`)

The list is a `list` named `Disks`, its rows `list item`s named `/dev/vdb` with the value
`eggfs "Photos" 1.0 GB not-mounted rw` (the record's words: `fat`, `blank`, `unknown`, `/home`,
`ro`); the facts are `label`s with their drawn text; `Check`, `Repair` and `Format...` are
`button`s, disabled ones flagged; the status and report lines `label`s; the panel's fields
`text field`s named `Label (optional)` and `Type vdb to confirm` and its buttons `Cancel` and
`Format`. An agent that wants the records calls `disk_list`, `disk_check` and `disk_format`,
not the tree (D22: the tree is for operating the window).

## 9. Acceptance (host tests, default 640x440, scale 1x, a fake with `vda` EggFS "Home"
mounted at `/home`, `vdb` EggFS "Photos" unmounted, `vdc` blank)

- `PANES` holds `Storage` right after `Mouse`; `storage`, `format`, `fsck`, `chkdsk`, `disk
  utility` and `repair` match it; `disk` matches it and About.
- In window coordinates: heading (192, 16); the list (192, 64, 208, 96) with rows `/dev/vda`,
  `/dev/vdb`, `/dev/vdc`, the first selected; the facts at x 416, y 64, 84, 104: `EggFS
  "Home"`, `1.0 GB · 312 MB used · 712 MB free`, `Mounted at /home`; the bar (416, 148, 208, 8)
  at 305 permille; the three buttons at (192, 172), (296, 172), (400, 172), all disabled; the
  status line at (192, 212) reads the in-use line in `text.dim`.
- Down selects `/dev/vdb`: `Check` and `Format...` enabled, `Repair` disabled, the status line
  the "Check reads the disk" line. `Check` on a clean fake shows `Clean. 262,144 blocks,
  182,000 free.`; on a fake with three findings (two repairable) the headline `Found 3
  problems: 2 repairable, 1 not.` and three lines starting `Repairable · inode 17 ·`, and
  `Repair` becomes enabled; `Repair` shows `Repaired 2 problems; 1 not repairable.`
- `/dev/vdc`: `Check` and `Repair` disabled, `Format...` enabled, the "No EggFS file system"
  line. `Format...` shows the panel at (208, 64, 400, 204) with `Format /dev/vdc?`, `Erases
  everything on it: Blank, 1.0 GB.`, `Format` disabled; typing `vdc` in the confirm field
  enables it; Enter formats and the row then reads `EggFS, no label`; a 65-byte label disables
  `Format` and shows the too-long line; Esc in an empty field closes the panel and writes
  nothing.
- A read-only fake disables `Repair` and `Format...` with the read-only line; a fake whose
  check fails with `Io` shows `The disk could not be read or written.`; `EPERM` shows the
  not-allowed line; a fake with no disks shows `No disks attached.` and the no-disk status line.
- `disk list`, `disk_list` and the pane agree row for row on the fake; the pane's report lines
  and `disk check --json`'s records agree finding for finding.
- A pane 400 wide lays the facts under the list and stacks the buttons; the element tree holds
  the list `Disks` with three items, the first `/dev/vda` with value `eggfs "Home" 1.0 GB /home
  rw`.

## Left out of 0.3.0

Checking, repairing or formatting a mounted disk (unmounting first); a progress bar for long
checks and formats; a cancel; partitions and formatting part of a disk from the pane (`--blocks`
stays the crate's); `lost+found` browsing (Files opens it once the disk is mounted); formatting
as FAT; renaming a label without formatting; mounting and unmounting; SMART and health;
hot-plug and a refresh on a tick; more than 4 disks; more than five finding lines in the pane.
