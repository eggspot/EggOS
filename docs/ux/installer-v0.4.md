# EggOS v0.4.0 Installer: design brief (the GUI front-end of `disk install`)

Design by Claude Fable (D16) for the GUI front-end of the `disk install` capability (D22, D241):
an `Install EggOS...` action in Settings' `Storage` pane that erases an unmounted disk and writes
EggOS on it, with the same data and words as `disk install [--dry-run] [--yes] <device>` and the
MCP tool `disk_install`. It extends `storage-v0.3.md`, and everything there stands: the pane's
layout, its rows and facts, `Check`, `Repair`, `Format...` and the format panel are unchanged, and
widgets 1 to 24 keep their numbers and places. Numbers are px at 1x and double at 2x; the only
motion is the caret's blink.

The case this is built for is the live system started from the USB stick (`real-hardware.md`): its
home is in memory, the machine's own NVMe and SATA disks are the rows, and the stick itself is
never a row, because the kernel has no USB mass-storage driver (D241). So no row can be the disk
EggOS is running from unless it is mounted, and a mounted disk is refused.

## 1. The action in the pane

Two widgets are added to the pane (pane coordinates, as `storage-v0.3.md` section 2):

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 25 | push button | (328, 172), 120x28 | `Install EggOS...` (three periods) |
| 26 | label | (16, 232), 432 wide | the install line, below, `text.dim` |

- **The button** is the fourth of the action row, 8 after `Format...` (which ends at x 320), and
  ends at x 448, the pane's right-aligned edge. It is 120 wide, not 96, because its title is 16
  characters. It is hidden when no disk is attached, as widgets 3 to 10 are. In the tree it is
  created right after widget 10, so the Tab order is the disk list, `Check`, `Repair`,
  `Format...`, `Install EggOS...` (disabled ones skipped), then the search field.
- **The install line** has report line 1's place (widget 12) and is never shown with it: it is
  shown while the pane has no report and nothing runs, and hidden while a check, repair or format
  report or a running line is shown. The status line (widget 11) keeps its `storage-v0.3.md`
  sentences word for word, about `Check`, `Repair` and `Format...`; the install line gives the
  reason for `Install EggOS...` alone. End-ellipsised at 448.
- **Search words** added to `Storage`: `install`, `installer`, `setup`, `Install EggOS`, `boot`,
  `EFI`, `GPT`, `USB`. `install` also matches the panes that already list it.
- **Budget**: these 2 and the panel's 9 (section 2) are 11 widgets, plus the panel's painter
  child the format panel also has in code: `CAPACITY` grows by 12.

**When the button is enabled**, decided from the selected disk and from whether the kernel kept
the boot files, checked in the CLI's order (`install_on`), so the line names what `disk install`
would refuse with:

| Case, first that applies | Button | Install line while idle (`text.dim`) |
|--------------------------|--------|--------------------------------------|
| mounted | off | `In use at /home: EggOS cannot be installed on a mounted disk.` |
| read-only | off | `Read-only disk: EggOS cannot be installed on it.` |
| smaller than 1 GiB | off | `Smaller than 1 GB: EggOS needs a disk of 1 GB or more.` |
| EggOS not started by its UEFI loader | off | `EggOS was not started from its USB stick or disk, so it has nothing to install.` |
| otherwise | on | `Install EggOS... erases the disk and writes EggOS on it, to start this machine from.` |
| no disk attached, boot files kept | hidden | `EggOS sees no disk to install on. It reads NVMe and SATA disks, not USB ones.` |
| no disk attached, no boot files | hidden | hidden |

- The fourth sentence is `StorageError::NoBootFiles.sentence()` unchanged. `GB` is Files'
  `size_text` unit (a disk of exactly 1 GiB reads `1.0 GB` and is large enough).
- Whether the boot files were kept is read once, when the pane is first shown, from
  `/proc/driver/boot` alone (its `kernel` and `loader` sizes both present and not 0); it cannot
  change while the system runs. The two files themselves are read only when the installation
  starts. The functions belong in `eggos_files::storage` beside `allowed` and `idle_text`
  (`has_boot_files`, `install_allowed`, `install_idle_text`), so the three front-ends share them.
- A system running from an installed disk has that disk mounted (off, the first line) and may
  install on any other disk: `started from its USB stick or disk` covers both.
- While a check, repair, format or installation runs, and while either panel shows, the button is
  disabled with the other three.

## 2. Install EggOS...: the confirmation panel

A modal panel with the format panel's frame exactly (`storage-v0.3.md` section 5, the same
painter): 400x204 at pane (32, `(pane height - 204) / 2`), 64 at the minimum window. Only one of
the two panels shows at a time. While it shows, the pane's widgets ignore pointer, wheel and Tab,
and the list draws unfocused.

| # | Widget | Place in the panel | Text |
|---|--------|--------------------|------|
| 27 | panel | (0, 0), 400x204 | |
| 28 | label | (16, 16) | `Install EggOS on /dev/vdb?` in `text` |
| 29 | label | (16, 36) | `Erases everything on it: EggFS "Photos", 500 GB.` in `text.dim` (`erases_text`, the format panel's sentence) |
| 30 | label | (16, 56) | `Writes an EFI system partition, 256 MB,` in `text.dim` |
| 31 | label | (16, 76) | `and an EggOS partition, 499.7 GB, for your files.` in `text.dim` |
| 32 | label | (16, 96) | `This cannot be undone. Type vdb to confirm.` in `text.dim` (`confirm_text`) |
| 33 | text field | (16, 120), 368x28 | placeholder `Type vdb to confirm`, `escape: Clear` |
| 34 | push button | (176, 160), 96x28 | `Cancel` |
| 35 | push button, primary | (280, 160), 96x28 | `Install`; the window's default button while the panel shows |

- **Lines 30 and 31** give the two partitions `installation_layout` will write, sized with
  `size_text`: the EFI system partition is always `256 MB` (`SYSTEM_PARTITION_BYTES`), the EggOS
  partition is the rest (its last sector minus its first, plus one, times 512). All five lines
  are 368 wide and end-ellipsised; the longest realistic erases line, with a 64-byte label, is
  ellipsised as it is in the format panel.
- **Install** is disabled until field 33 holds exactly the disk's short name (`vdb`,
  case-sensitive, spaces trimmed); typing it enables the button in the same keystroke. Enter in
  the field goes to `Install` when it is enabled, else nothing.
- **Focus** opens on the field; Tab goes field, `Cancel`, `Install` (skipped while disabled),
  wrapping; Shift+Tab the reverse. Space or Enter presses a focused button.
- **Esc** clears the field when it holds text; Esc in the empty field or on a button, `Cancel`,
  or closing the window cancels: the panel hides, nothing is read or written, the focus returns
  to `Install EggOS...`, and the default button goes back to the window's own.
- **The CLI and MCP** confirm the same way: `disk install` asks `Type vdb to install EggOS on
  /dev/vdb (erases EggFS "Photos", 500 GB):` on a terminal and refuses with `needs_confirmation`
  without one; `--yes` answers it; `--dry-run` prints `would erase /dev/vdb (EggFS "Photos",
  500 GB) and install EggOS on it`. The erased-contents words are one function (`erased_text`)
  in all three.

## 3. Running, done and refused

- **Running.** On `Install` the field and both buttons are disabled, line 32 reads `Installing
  EggOS on /dev/vdb... This can take a minute.` in `text`, lines 28 to 31 stay, and the frame is
  committed before the work starts (the program's deferred disk work, as `Format`). The work
  reads the kept loader and kernel, then `storage::install` writes the table, the 256 MiB FAT32
  volume with the two files, and the EggOS partition's first MiB; with a large kernel on a slow
  disk that is tens of seconds. There is no bar and no cancel, because `install` reports no
  progress and a half-written table is worse than a finished one; input that arrives meanwhile,
  a close of the window included, is acted on when the work ends.
- **Done.** The panel hides, the disks are re-read, the same row stays selected, the focus
  returns to `Install EggOS...`, the report lines are hidden, and:
  - the status line (widget 11) reads the CLI's line, in `text`: `Installed EggOS on /dev/vdb.
    Restart and choose it in the firmware's boot menu.`;
  - the install line (widget 26) reads, in `text.dim`: `It reads Unknown contents here until
    EggOS is started from it.`;
  - the row's facts read `Unknown contents` / `500 GB · space unknown` / `Not mounted`, and
    `disk list` says `unknown`. That is the record's truth and stays one set of words: the
    running kernel reaches the whole disk, whose first sector is now a protective MBR, and only
    a kernel started later uses the EggOS partition (D238). After a restart from the disk the
    same row reads `EggFS, no label`, `Mounted at /home`.
  - Both lines stay until the next action, a change of selection or the pane being shown again,
    as a report does. `Format...` and `Install EggOS...` stay enabled on the row.
- **Refused or failed.** The panel stays open, the field and `Cancel` are enabled again,
  `Install` follows the field, and line 32 shows `StorageError::sentence()` in `text`:

| `StorageError` | Line 32 |
|----------------|---------|
| `InUse` (raced a mount) | `In use at /home: nothing was done.` |
| `ReadOnly` | `Read-only disk: nothing was done.` |
| `NotAllowed` | `Not allowed: only root may open disks.` |
| too small (raced; see below) | `The disk is smaller than 1 GB: EggOS needs 1 GB or more.` |
| `NoBootFiles` | `EggOS was not started from its USB stick or disk, so it has nothing to install.` |
| `Busy` | `The disk is busy: try again in a moment.` |
| `Failed` | `The disk could not be read or written.` |
| `NoSuchDisk` | `No such disk is attached.` |

  After `Failed` the disk may hold part of an installation, so the disks are re-read behind the
  panel and line 29 is written again from the new record (usually `Unknown contents`). Typing in
  the field puts line 32's `This cannot be undone.` sentence back.
- **One change the words need in code.** `disk install` on a disk under 1 GiB answers today with
  `StorageError::TooSmall`, whose sentence is `The disk is smaller than 1 MiB.` (format's limit),
  which is false for an installation. The capability gets its own variant, `TooSmallToInstall`,
  with the sentence in the table above, used by the pane, `disk install` and `disk_install`.

## 4. Every device class (D23)

As `storage-v0.3.md` section 7, with these additions:

- **One column** (pane narrower than 448): `Install EggOS...` is the fourth stacked button, full
  width, 8 under `Format...`; the status line, the install line and the report follow, the
  install line still in report line 1's place. The install panel is a sheet as the format panel
  is: the pane's width minus 16, at the pane's bottom, 16 px padding, the field full width, the
  two buttons sharing the last row. Lines 30 and 31 are short enough for a 320 px sheet; the
  erases line is ellipsised there as it is in the format sheet.
- **Touch**: the button is 40 tall with the other three; the field stays 28. The sheet needs
  the typed short name, so it needs a keyboard, as the format sheet does.
- **TV (scale 2)**: everything doubles; the button is reached by Tab or arrows after
  `Format...`; the panel works by typing, Tab, Enter and Esc alone.
- **Keyboard-only** on every form factor: Tab to `Install EggOS...`, Space, type the name, Enter.

## 5. What an agent sees (D161, `wm show`)

`Install EggOS...` is a `button`, flagged disabled when it is; the install line is a `label`
with its drawn text, so the reason is readable from the tree. The panel is a `panel` holding
five `label`s with their drawn text, a `text field` named `Type vdb to confirm`, and the
`button`s `Cancel` and `Install` (`Install` flagged disabled until the name is typed, and the
default button).

The typed name is a person's confirmation, as the format panel's is: an agent does not type it.
An agent that wants to install calls `disk_install` (`device`, `dry_run`, and the `confirm`
argument every `destroy` tool has, asked of the person by elicitation where the client supports
it), which needs the write grant, and reads the disks with `disk_list`, not the tree (D22: the
tree is for operating the window).

## 6. Acceptance (host tests, default 640x440, scale 1x, the fake of `storage-v0.3.md` section 9
with every disk 2 GiB: `vda` EggFS "Home" mounted at `/home`, `vdb` EggFS "Photos" unmounted,
`vdc` blank; boot files kept unless a check says otherwise)

- `install`, `installer` and `setup` match `Storage`. In window coordinates the button is (504,
  172, 120, 28) and the install line (192, 232); Tab from `Format...` reaches `Install EggOS...`,
  then the search field.
- `/dev/vda`: all four buttons disabled; the status line is the 0.3 in-use line unchanged; the
  install line reads `In use at /home: EggOS cannot be installed on a mounted disk.` in
  `text.dim`.
- `/dev/vdb`: the button enabled, the install line the `erases the disk and writes EggOS on it`
  line. `Check` on a fake with findings hides the install line and shows report line 1 in its
  place; selecting another disk brings the install line back.
- A read-only fake, a 512 MiB fake and a fake with no `/proc/driver/boot` each disable the button
  with their line; a mounted, read-only disk shows the mounted line (the CLI's order). A disk of
  exactly 1 GiB enables it. No disks with boot files shows the `sees no disk` line; without boot
  files, no install line.
- `Install EggOS...` on `/dev/vdc` shows the panel at (208, 64, 400, 204): `Install EggOS on
  /dev/vdc?`, `Erases everything on it: Blank, 2.0 GB.`, `Writes an EFI system partition, 256
  MB,`, `and an EggOS partition, 1.7 GB, for your files.`, `This cannot be undone. Type vdc to
  confirm.`; the field focused, `Install` disabled and the default button; the format panel
  hidden. `vdb` typed leaves `Install` disabled; `vdc` enables it; ` vdc ` does too.
- Enter installs: the fake's first sector ends `55 AA`, its table lists an EFI system partition
  of 524,288 sectors from sector 2,048 and an EggOS data partition after it. The panel is hidden,
  the row reads `Unknown contents` / `2.0 GB · space unknown` / `Not mounted`, the status line
  is `Installed EggOS on /dev/vdc. Restart and choose it in the firmware's boot menu.`, the
  install line the `Unknown contents here until` line, and the focus is on `Install EggOS...`.
- Esc with text in the field clears it; Esc again closes the panel; the fake is untouched and
  the focus is on `Install EggOS...`. `Cancel` does the same.
- A fake whose writes fail keeps the panel open with `The disk could not be read or written.`
  and the controls enabled; `EPERM`, `EBUSY` and `EROFS` show their sentences; boot files that
  vanish show the `NoBootFiles` sentence.
- The pane's erases line, `disk install --dry-run` and the CLI's question hold the same
  `erased_text`; the done line is byte for byte the CLI's without its newline.
- A pane 400 wide stacks four buttons and shows the panel as a sheet; the element tree holds the
  `button` `Install EggOS...` and, with the panel open, the `text field` `Type vdc to confirm`
  and the `button`s `Cancel` and `Install`.

## Left out of 0.4.0

A progress bar and a cancel (the installer reports no progress); a `Restart` button in the pane
or the done line (the person restarts as they always do); choosing partition sizes, keeping other
systems' partitions and dual boot; a label for the EggOS partition; a UEFI boot entry, so the
firmware's boot menu is still needed; a new `disk list` word for a disk that holds an
installation not yet started (it reads `unknown`); the two `Writes` lines in `disk install
--dry-run`; installing on the disk EggOS runs from, or on any mounted disk; installing from a
system the loader did not start (QEMU's direct kernel boot); USB disks as targets and the stick
as a row; copying the live system's home and settings to the new disk; Secure Boot.
