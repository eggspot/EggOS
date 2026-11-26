# EggOS Installer, dual boot v0.1: installing beside Windows, macOS or Linux: design brief

Status: design for the leftover of `installer-v0.4.md` ("keeping other systems' partitions and
dual boot") and of D241 ("keeping other operating systems' partitions", "a UEFI boot entry").

Design by Claude Fable (D16). It extends `installer-v0.4.md` and `storage-v0.3.md`, and
everything there stands: the Storage pane, its widgets 1 to 35 with their numbers and places,
the install line, the confirmation panel's frame, the typed short name, the running, done and
refused words, and **`disk install <device>` with no new option still erases the whole disk
after the same question**. This brief adds one panel before the confirmation (the plan), the
words the confirmation takes when a system is kept, the shared EFI system partition, EggOS's
boot entry and boot menu, and the refusals. One capability, three front-ends (D22): section 7
gives the command and the MCP tool for everything the pane does. Numbers are px at 1x and
double at 2x; sizes are Files' `size_text` (`36 GB`, `1.0 GB`, `256 MB`); the only motion is
the caret's blink and the boot menu's countdown.

## 0. What a person gets, and how that is checked

- `Install EggOS...` on a disk that already holds a system asks **how**: `Install alongside
  Windows` (chosen when it is possible) or `Use the whole disk`.
- Alongside: a **slider** says how much EggOS gets, a **map** of the disk shows what stays and
  what is new, and one line says what the other system keeps. Nothing of it is erased.
- EggOS goes into the **EFI system partition that is already there**, beside the other
  system's files, and writes none of them.
- At power-on a **boot menu** offers EggOS and the others, with a default and a wait; a
  machine with one system never sees it and loses no time to it.
- What cannot be done safely is **refused with the reason and the way out**: an MBR disk, a
  BitLocker volume, no room, a hibernated Windows, a volume EggOS cannot shrink.
- Host tests drive the pane, the command and the tool against fake disks and a fake firmware
  (section 12).

## 1. What EggOS finds on a disk

The capability gains one read, **`disk partitions <device>`** (the MCP tool
`disk_partitions`), and the pane, the command and the tool all plan from its record:

- `table`: `gpt`, `mbr`, `none` or `damaged` (a GPT whose primary header or entries fail their
  CRC).
- `partitions`, in table order, numbered from 1: `number`, `first_sector`, `sectors`,
  `size_bytes`, `type` (`efi-system`, `microsoft-reserved`, `microsoft-data`,
  `windows-recovery`, `apple-apfs`, `apple-hfs`, `linux-data`, `linux-swap`, `linux-lvm`,
  `eggos-data`, `unknown`), `file_system` (`fat`, `ntfs`, `bitlocker`, `apfs`, `hfs+`, `ext4`,
  `btrfs`, `xfs`, `luks`, `swap`, `eggfs`, `unknown`), `label`, `system` and `system_name`
  (below, or `null`), `used_bytes` and `free_bytes` (`null` where EggOS cannot read them),
  **`shrinkable_bytes`** (how much can be taken from its end without moving a file; `null`
  when it cannot be shrunk) and `shrink_blocked` (`null`, or the reason: `encrypted`,
  `hibernated`, `needs-check`, `unsupported`).
- `unallocated`: each run of free sectors of 1 MiB or more, `first_sector`, `sectors`,
  `size_bytes`.
- `systems`: each system found, `system`, `name`, `partition`.

**How a system is recognised** (read-only probes; nothing is mounted or written):

| System | Found when | Its partition | Name shown |
|--------|-----------|---------------|------------|
| Windows | an EFI system partition holds `\EFI\Microsoft\Boot\bootmgfw.efi`, or the disk has a Microsoft reserved partition | the largest `microsoft-data` partition that is NTFS or BitLocker | `Windows` |
| macOS | a partition of type Apple APFS or Apple HFS+ | that partition | `macOS` |
| Linux | a partition of type Linux data, swap or LVM, or an EFI system partition folder of a known distribution | the largest Linux data partition | the folder's name from a table of ten (`ubuntu` `Ubuntu`, `fedora` `Fedora`, `debian` `Debian`, `opensuse` `openSUSE`, `arch` `Arch Linux`, ...), else `Linux` |
| EggOS | a partition of type EggOS data (D238) | that partition | `EggOS` |

Names are joined as people say them: `Windows`, `Windows and Ubuntu`, `Windows, Ubuntu and
macOS`. A disk with partitions and no system EggOS knows reads `3 partitions`, and they can be
kept all the same.

**What can be shrunk in v0.1**: an NTFS volume that is not BitLocker, was shut down fully and
is not marked for a check; an ext4 volume that is clean. EggOS only takes the **free tail**:
it moves no file, so `shrinkable_bytes` is the space after the last used cluster, less a
reserve it leaves the other system (`min(8 GiB, half its free space)`). Never APFS, HFS+,
Btrfs, XFS, LVM, LUKS, FAT, swap or EggFS: those are shrunk in their own system first, and
EggOS then uses the unallocated space.

**Where EggOS goes** (one rule for the pane, the command and the tool; no chooser in v0.1):

1. the largest unallocated run, when it is 16 GiB or more;
2. else the tail of the partition with the most `shrinkable_bytes`, joined with an
   unallocated run that directly follows it;
3. else the largest unallocated run, when it is 1 GiB or more;
4. else alongside is refused (section 8).

The **room** is that run's size in whole GiB. EggOS's partition is one run, aligned to 1 MiB,
and needs 1 GiB at least (the limit `disk install` already has).

**In the Storage pane** a partitioned disk's contents line (widget 3) reads the systems and the
count instead of `Unknown contents`: `Windows · 4 partitions`, `Windows and Ubuntu · 7
partitions`, `3 partitions`. `disk list` keeps its `unknown` word for such a disk; the new
facts are `disk partitions`'s. The install line (widget 26), when the button is enabled on
such a disk, reads `Install EggOS... can keep Windows on this disk, or use all of it.` (`can
keep what is on this disk`, with no system named).

## 2. The plan panel

`Install EggOS...` on a disk whose `table` is `gpt` or `mbr` and holds at least one partition
opens the **plan panel** first. A blank disk, a disk with no table and a disk whose table is
`damaged` and empty go straight to v0.4's confirmation, as today.

The frame is the format panel's (`storage-v0.3.md` section 5): 400 wide, **276 tall**, at pane
(32, `(pane height - 276) / 2`), 28 at the minimum window. Only one panel shows at a time;
while it shows, the pane's widgets ignore pointer, wheel and Tab.

| # | Widget | Place in the panel | Text and colour |
|---|--------|--------------------|-----------------|
| 36 | panel | (0, 0), 400x276 | |
| 37 | label | (16, 16) | `Install EggOS on /dev/vdb` in `text` |
| 38 | label, 368 wide | (16, 36) | the found line, `text.dim` (below) |
| 39 | radios, vertical, 2 rows of 28 | (16, 60), 368x56 | `Install alongside Windows`; `Use the whole disk` |
| 40 | custom, the map | (16, 124), 368x16 | section 2.2 |
| 41 | label, 368 wide | (16, 146) | the legend, `text.dim`: `Windows 218 GB · EggOS 36 GB` |
| 42 | label | (16, 176) | `EggOS gets` in `text` |
| 43 | slider, readout | (112, 170), 272x28 | name `EggOS size`, whole GB, unit ` GB` |
| 44 | label, 368 wide | (16, 206) | the keeps line (below) |
| 45 | push button | (176, 232), 96x28 | `Cancel` |
| 46 | push button, primary | (280, 232), 96x28 | `Next`; the window's default button while the panel shows |
| 47 | push button | (16, 160) in the confirmation panel, 96x28 | `Back` (section 3) |
| 48 | label, 368 wide | (16, 166) | refusal line 1, `text.dim`; in the slider row's place |
| 49 | label, 368 wide | (16, 186) | refusal line 2, `text.dim` |

`CAPACITY` grows by 15: these 14 and the map's painter child.

**The found line** (widget 38): `Found: Windows, 254 GB, 100 GB free.`; with a second system
`Found: Windows and Ubuntu.`; with unallocated space that will be used `Found: Windows, 222 GB
· 32 GB unallocated.`; BitLocker adds `(BitLocker)` after the name; no system: `Found: 3
partitions, 120 GB.`

### 2.1 The two choices

| State | Radio 1 | Radio 2 | Slider row (42, 43) | Lines 48, 49 | Keeps line (44) |
|-------|---------|---------|---------------------|--------------|-----------------|
| alongside possible, chosen (the opening state) | selected | | shown | hidden | `text.dim`: `Windows keeps 218 GB, 64 GB of it free.` |
| the same, from unallocated space | selected | | shown | hidden | `text.dim`: `Uses unallocated space. Windows is not touched.` |
| alongside possible, whole disk chosen | | selected | hidden | hidden | `text`: `Erases everything: Windows and 3 more partitions.` |
| alongside refused | disabled | selected | hidden | the reason and the way out (section 8) | `text`: the erases line |

- Radio 1's title names what is kept: `Install alongside Windows`, `Install alongside Windows
  and Ubuntu`, `Install alongside what is on it` (no system named; three or more systems).
- **The slider** is the toolkit's (`toolkit-v0.4.md` section 2): 1 to the room, step 1, the
  readout `36 GB`. It opens at **half the room, or 16 when half is less and the room allows
  16, or all the room when it is under 16**; from unallocated space alone it opens at all of
  it. Every change rewrites the map, the legend and the keeps line in the same frame. With a
  room of 1 it is shown disabled at 1.
- **The keeps line** subtracts the slider's value from the other system's size and from its
  free space: `Windows keeps 218 GB, 64 GB of it free.`
- **`Next`** opens the confirmation (section 3); Enter presses it. **`Cancel`**, Esc or
  closing the window closes the panel, changes nothing and returns the focus to `Install
  EggOS...`.

### 2.2 The map

A bar of the whole disk, 368x16, one **segment** per partition and per unallocated run of 1
MiB or more, in disk order, with a 1 px gap of the panel's surface between segments.

- Widths are in proportion to bytes, but a segment is **at least 4 px**: the small ones take
  their 4 first, the rest share what is left in proportion, and the largest takes the
  rounding.
- **Kept** partitions are hollow: `menu.bg` with a 1 px `text.dim` outline. **EggOS's** new
  partition (and its own EFI system partition, when it writes one) is solid `accent`.
  **Unallocated** space that stays is not drawn: the surface shows. Solid, hollow and empty
  differ in shape, not only in colour.
- With `Use the whole disk` the map is two solid segments: the EFI system partition (4 px)
  and EggOS.
- The legend under it names the two that matter, in words: the other system's new size and
  EggOS's; with the whole disk, `EFI 256 MB · EggOS 255 GB`.

## 3. The confirmation

v0.4's panel and widgets 27 to 35, in the same places, with these lines when a system is
kept, and **`Back`** (widget 47) at its left, shown whenever the plan panel came first:

| # | Shrinking | From unallocated space |
|---|-----------|------------------------|
| 28 | `Install EggOS beside Windows on /dev/vdb?` | the same |
| 29 | `Shrinks Windows from 254 GB to 218 GB. Nothing is erased.` | `Uses 32 GB of unallocated space. Nothing is erased.` |
| 30 | `Writes an EggOS partition, 36 GB, for your files,` | the same, with its size |
| 31 | `and adds EggOS to the EFI system partition.` | the same |
| 32 | `Back up first: a resize can fail. Type vdb to confirm.` | `Changes the partition table. Type vdb to confirm.` |

- When EggOS writes its own EFI system partition (section 5), line 30 gives the EggOS
  partition 256 MB less and line 31 reads `and its own EFI system partition, 256 MB.`
- When a kept system is BitLocker, line 32 reads `Have your BitLocker recovery key ready. Type
  vdb to confirm.`
- With `Use the whole disk`, the lines are v0.4's; line 29's contents are `Windows and 3 more
  partitions, 256 GB`.
- The typed short name, `Install`'s enabling, Enter and Esc are v0.4's. The same deliberate
  gesture guards both choices: a partition table that someone's system lives in is about to
  change.
- **`Back`** returns to the plan panel as it was left (choice and slider kept) and clears the
  field. Tab order: the field, `Back`, `Cancel`, `Install`.

## 4. Running, done, refused

**Running.** As v0.4: the controls are disabled, lines 28 to 31 stay, and line 32 names the
step in `text`. No bar and no cancel: a step half done is worse than a step finished.

| Step, in this order | Line 32 | If it fails |
|---------------------|---------|-------------|
| 1. check again everything section 8 checks | `Checking /dev/vdb...` | the refusal's sentence; nothing was written |
| 2. write EggOS's files to the EFI system partition | `Adding EggOS to the EFI system partition...` | `Could not write the EFI system partition: nothing was changed.` (its new files are removed) |
| 3. shrink the file system, its new size written last | `Shrinking Windows...` | `Could not shrink Windows: nothing was changed.` (step 2's files are removed) |
| 4. write the partition table, backup first, then primary | `Writing the partition table...` | `Could not write the partition table: the old one was put back.` (the file system stays smaller than its partition, which harms nothing; step 2's files are removed) |
| 5. zero the EggOS partition's first MiB | `Preparing the EggOS partition...` | `The disk could not be read or written.` |
| 6. add the firmware's boot entry | `Adding EggOS to the boot menu...` | not a failure: the second done line below |

The order is what makes each failure safe: EggOS's boot files are in place before the table
changes, and a file system is never larger than its partition.

**Done.** The panel hides, the disks are read again, the row stays selected and its contents
line reads `Windows and EggOS · 5 partitions`. The status line (widget 11), the command's
line, in `text`: `Installed EggOS beside Windows on /dev/vdb. Restart: the boot menu offers
both.` When the firmware took no boot entry: `Installed EggOS beside Windows on /dev/vdb.
Choose EggOS in the firmware's boot menu.` The install line (widget 26), `text.dim`: `Windows
keeps 218 GB. EggOS has 36 GB.`

**Refused or failed.** The panel stays open with the sentence in line 32, the field and
`Cancel` enabled again; v0.4's table of `StorageError` sentences holds, with the step
failures above.

## 5. The EFI system partition, shared

- EggOS uses **the disk's existing EFI system partition** and writes only its own folder,
  **`\EFI\EggOS\`**: `EGGOSX64.EFI` (`EGGOSAA64.EFI` on AArch64; the kept loader),
  `KERNEL.ELF`, `CMDLINE.TXT` and `BOOT.TXT` (the boot menu's settings, section 6). It never
  writes, renames or deletes a file outside that folder, and never formats the partition.
- `\EFI\BOOT\BOOT<ARCH>.EFI`, the firmware's fallback path, is written **only when no file is
  there**: on a shared partition it is usually another system's.
- It needs the two boot files' size plus 1 MiB free. **When the partition has less, is not
  FAT, or the disk has none**, EggOS writes its **own** 256 MB EFI system partition at the
  start of its room, as a whole-disk install does; two on one disk are allowed by UEFI. The
  confirmation's line 31 says which will happen before anything is written.
- `CMDLINE.TXT` names EggOS's partition by its GUID, so a second EggOS partition on the
  machine is never mistaken for it.

## 6. The boot entry and the boot menu

**The entry.** The installation adds a firmware boot entry named **`EggOS`** for `\EFI\EggOS\
EGGOS<ARCH>.EFI` and puts it **first** in the boot order; the other systems' entries stay,
after it, untouched. A whole-disk install adds the same entry (D241's "Not yet").

**The menu** is EggOS's loader's, shown before the kernel starts:

- **Only when there is a choice**: the firmware lists another entry that starts a file on a
  disk partition (network and removable-device entries do not count), or the EFI system
  partition holds another system's loader. A machine with EggOS alone starts at once, as
  today (D69).
- **Entries**: `EggOS` first, then the others under the names the firmware gives them
  (`Windows Boot Manager`, `ubuntu`), cut at 40 characters, then `Firmware settings` when the
  firmware can restart into its setup.
- **Look** (the boot screen's style and colours, `desktop-v0.3.md` section 1; Egg Mono 8x16;
  on a screen `W` x `H`): background `#0E1726`; `EggOS` at 2x, centred, top at `H / 2 - 160`;
  rows **400 wide, 40 tall**, at x `(W - 400) / 2`, the first at y `H / 2 - 96`, one under
  another; a row's name at row x + 40, cell top + 12, in `#E6EAF0`; ` (default)` after the
  default's name in `#8A94A6`; the selected row filled `#2E4A6B` with `>` in `#F2B33D` at row
  x + 16; the hint centred 16 under the last row in `#8A94A6`. At 1280x800 with three
  entries: rows at (440, 304), (440, 344), (440, 384), the hint's cell top at 440. A
  framebuffer 2560 wide or more doubles everything.
- **The wait**: the hint reads `Starts EggOS in 5 s`, counting down once a second; at 0 the
  default starts. Any key stops the countdown for good, and the hint becomes `Enter: start
  D: default   T: wait 5 s`.
- **Keys**: Up and Down move (wrapping), Home and End, Enter starts the selected entry, 1 to 9
  start that entry, **D** makes the selected entry the default, **T** steps the wait through
  0, 3, 5, 10 and 30 s. Esc only stops the countdown.
- **Starting another system**: the loader asks the firmware to start that entry next and
  restarts the machine (`Restarting into Windows...` for that moment). It does not load the
  other system's loader itself, so the other system starts exactly as its firmware entry
  always started it, and BitLocker and Secure Boot see nothing new. A loader found on the
  partition with no firmware entry is started directly.
- **High contrast**: when `accessibility.high_contrast` is on, `BOOT.TXT` says so and the
  menu is `#FFFFFF` on `#000000`, the selected row `#153A6B` with a 1 px `#FFFFFF` outline,
  `>` `#FFD24D`, the dim words `#B8C0CC`. The light theme does not reach it: the boot screen
  has one background.
- **No screen** (a server on a serial console): the same as lines, `1) EggOS (default)`, `2)
  Windows Boot Manager`, `3) Firmware settings`, then `Starts 1 in 5 s. Press a number.`

**Its two settings**, kept in `BOOT.TXT` and reached three ways (D22): the menu's own D and T
keys, `settings get|set`, and `settings_get|settings_set`.

| Key | Values | Default |
|-----|--------|---------|
| `boot.default` | `eggos`, `last` (the one started last), or an entry's id | `eggos` |
| `boot.menu_wait_s` | 0 to 60; 0 hides the menu unless Shift, Space or Esc is held or tapped while EggOS starts, or the last start did not finish | 5 |
| `boot.entries` (read-only) | the menu's ids in order: `eggos,windows,firmware` | |

An entry's id is its system in lower case (`windows`, `ubuntu`, `macos`, `firmware`), with
`-2` and so on for a second of the same. On a machine with no menu the keys are listed and
`boot.entries` reads `eggos`.

## 7. One capability, three front-ends

| In the pane | Command | MCP tool |
|-------------|---------|----------|
| the contents line, the plan panel's facts | `disk partitions [--json] <device>` | `disk_partitions`: `device` |
| `Use the whole disk`, then `Install` | `disk install [--dry-run] [--yes] <device>` (unchanged) | `disk_install`: `device`, `dry_run`, `confirm` (unchanged) |
| `Install alongside ...`, the slider, then `Install` | `disk install --alongside [--size <size>] [--dry-run] [--yes] <device>` | `disk_install` with `alongside: true` and `size_bytes` |
| the boot menu's D and T | `settings set boot.default <id>`, `settings set boot.menu_wait_s <n>` | `settings_set` |

- **`disk partitions`** prints one line a partition and a run, `NUMBER  START  SIZE  TYPE  FS
  LABEL  SYSTEM  FREE  SHRINK`, with `-` for a run's number and for what is unknown, and
  `--json` the record of section 1. It needs root, as `disk list`; the tool needs the view
  grant.
- **`--size`** is whole GiB with a unit (`36G`, `1T`); without it the size is the slider's
  opening value. Out of range: `--size must be 1G to 72G on /dev/vdb` (`size_out_of_range`).
- **Without `--alongside`** nothing changes, except that the question names the systems
  (`Type vdb to install EggOS on /dev/vdb (erases Windows and 3 more partitions, 256 GB):`)
  and a terminal first gets `Windows is on this disk. To keep it: disk install --alongside
  /dev/vdb`.
- **The question** with `--alongside`: `Type vdb to install EggOS beside Windows on /dev/vdb
  (shrinks Windows to 218 GB, erases nothing):`; `--yes` answers it; without a terminal,
  `needs_confirmation`.
- **`--dry-run`**: `would shrink Windows (partition 3) from 254 GB to 218 GB, write an EggOS
  partition of 36 GB, add EggOS to the EFI system partition (partition 1) and add a boot
  entry`.
- **The tool** needs the write grant and `confirm`, asked of the person by elicitation with
  the dry run's sentence. `alongside` defaults to false, so a caller written for v0.4 does
  what it did. An agent never types the short name in the pane (v0.4 section 5).
- The sentences of sections 2 to 4 and 8 are one function each in `eggos_files::storage`, for
  the pane, the command and the tool; a refusal's two lines are joined with a space in the
  command's message.

## 8. Refusals

Alongside is refused, **and `Use the whole disk` stays possible**, in the cases below. The
pane shows the two lines in widgets 48 and 49; the command and the tool answer with the code.

**The table**, checked first:

| Case | Code | Line 1 | Line 2 |
|------|------|--------|--------|
| the table is MBR | `table_is_mbr` | `MBR partition table: EggOS installs alongside on GPT disks only.` | `Convert it to GPT in the other system, or use the whole disk.` |
| the table is damaged | `table_damaged` | `The partition table is damaged: EggOS will not change it.` | `Repair it in the other system, or use the whole disk.` |
| the table has no free entry | `table_full` | `The partition table is full: 128 partitions.` | `Remove one in the other system, or use the whole disk.` |

**No room**: when section 1's rule finds no place of 1 GB, the reason is the first of these
that applies to the largest system's partition:

| Case | Code | Line 1 | Line 2 |
|------|------|--------|--------|
| BitLocker | `partition_encrypted` | `Windows is encrypted with BitLocker: EggOS cannot shrink it.` | `In Windows, shrink the volume by 16 GB or more, then come back.` |
| Windows hibernated or using Fast Startup | `system_hibernated` | `Windows is hibernated or uses Fast Startup: its disk is in use.` | `Shut Windows down fully (shutdown /s /t 0), then try again.` |
| the file system is marked for a check | `needs_check` | `Windows' file system needs a check: EggOS will not shrink it.` | `Run chkdsk in Windows (fsck in Linux), then try again.` |
| a file system EggOS cannot shrink (APFS, HFS+, Btrfs, XFS, LVM, LUKS) | `cannot_shrink` | `EggOS cannot shrink a macOS (APFS) volume.` | `Shrink it in macOS's Disk Utility, then install in the space.` |
| less than 1 GB can be freed | `no_room_alongside` | `No room beside Windows: 0.6 GB can be freed, EggOS needs 1 GB.` | `Free space in Windows, or use the whole disk.` |

v0.4's refusals come before all of these and refuse both choices: mounted, read-only, smaller
than 1 GB, not started by EggOS's UEFI loader. BitLocker with unallocated room is **not** a
refusal: EggOS installs in that room and the confirmation warns about the recovery key.

## 9. Keys, focus, pointer

- The plan panel opens with the focus on the radios. **Tab**: radios (one stop), the slider
  (when shown), `Cancel`, `Next`, wrapping; Shift+Tab the reverse; disabled ones are skipped.
- **Radios**: Up and Down choose (a disabled radio is skipped); Space chooses the focused
  one. **Slider**: Left and Right 1 GB, PageUp and PageDown 10, Home and End the ends; the
  wheel does nothing (v0.4). **Enter** anywhere in the panel presses `Next`; **Esc** cancels.
- **Pointer**: a click on a radio's row chooses it; the slider drags; the map takes no input
  and shows no tooltip (its facts are the legend's).

## 10. What an agent sees (D161)

- The plan panel is a `panel` holding: the `label`s with their drawn text; a `radiogroup`
  named **`How to install`** with `radio`s `Install alongside Windows` (`selected`) and `Use
  the whole disk` (`disabled` when refused); a `graphic` named **`Disk layout`** whose value
  lists the segments in order (`efi 100 MB, reserved 16 MB, Windows 218 GB, EggOS 36 GB,
  recovery 1.0 GB`); a `slider` named **`EggOS size`** valued `36 1..72 by 1`; `button`s
  `Cancel` and `Next` (`default`).
- The confirmation is v0.4's, with the `button` `Back`.
- An agent may read the panel, choose, set the slider and press `Next`; the typed name stays
  the person's. An agent that wants to install calls `disk_partitions` and `disk_install`.

## 11. Every device class (D23)

- **One column** (a pane narrower than 448; a phone): the plan is a sheet as the format panel
  is: the pane's width minus 16, at the bottom, 16 px padding; the radios' rows 48 tall; the
  map and the lines the sheet's width minus 32; `EggOS gets` above a full-width slider; the
  two buttons share the last row, 48 tall. The found, keeps and refusal lines are ellipsised;
  the sheet's legend carries the two sizes.
- **Tablet**: the desktop panel with 40 px rows and buttons: radios at 60 and 100, the map at
  148, the legend 170, the slider row 194, the keeps line 242, the buttons at 268; 324 tall.
- **TV** (scale 2, D-pad): everything doubles; Up and Down move radios, slider and buttons;
  Left and Right move the slider or between the two buttons; OK presses; Back cancels. The
  confirmation needs typing, as v0.4.
- **Wearable**: Storage is read-only there; nothing installs from it.
- **Headless**: the commands and the tools are the whole capability; the boot menu is the
  serial one.
- **The boot menu on touch devices**: where the firmware reports the volume keys as Up and
  Down and the power key as Enter, they move and start; with no keys at all, the wait starts
  the default.

## 12. Tokens

| Part | Token on token | D / DH / L / LH |
|------|----------------|-----------------|
| kept segment's outline, the found, legend, keeps and refusal lines | `text.dim` on `menu.bg` | 5.1 / 11.4 / 5.4 / 9.6 |
| EggOS's segment | `accent` on `menu.bg` | 8.4 / 14.6 / 4.7 / 7.2 |
| the erases line, the titles, `EggOS gets` | `text` on `menu.bg` | 12.9 / 21.0 / 16.0 / 21.0 |
| the radios, the slider, the buttons | the toolkit's own | |
| boot menu: names; dim words; selected name; `>` | fixed: `#E6EAF0`, `#8A94A6` on `#0E1726`; `#E6EAF0`, `#F2B33D` on `#2E4A6B` | 14.9; 5.9; 7.5; 4.9 |
| boot menu, high contrast | `#FFFFFF`, `#B8C0CC` on `#000000`; `#FFFFFF`, `#FFD24D` on `#153A6B` | 21.0; 11.4; 11.4; 7.9 |

## 13. Acceptance (host tests, 640x440, 1x, the fake of `storage-v0.3.md` section 9 plus fake
`W`: `/dev/vdb`, 256 GiB, GPT: 1 an EFI system partition, 100 MiB, FAT, 68 MB free, holding
`\EFI\Microsoft\Boot\bootmgfw.efi`; 2 Microsoft reserved, 16 MiB; 3 NTFS `Windows`, all the
rest (254.9 GiB, shown as `254 GB`), 100 GiB free, 72 GiB shrinkable, shut down fully; 4
Windows recovery, 1 GiB, at the disk's end; no unallocated space; a fake firmware with the
entry `Windows Boot Manager`; boot files kept)

- `disk partitions /dev/vdb` lists four partitions with `Windows` on 3, `FREE` `100 GB`,
  `SHRINK` `72 GB`; `--json` and `disk_partitions` give the same record. The pane's contents
  line reads `Windows · 4 partitions`, the install line the `can keep Windows` line.
- `Install EggOS...` opens the plan panel at window (208, 28, 400, 276): `Install EggOS on
  /dev/vdb` at (224, 44), `Found: Windows, 254 GB, 100 GB free.` at (224, 64), the radios at
  (224, 88) with `Install alongside Windows` selected and focused, the map at (224, 152, 368,
  16), the legend `Windows 218 GB · EggOS 36 GB` at (224, 174), the slider at (320, 198, 272,
  28) reading `36 GB` with the range 1 to 72, `Windows keeps 218 GB, 64 GB of it free.` at
  (224, 234), `Cancel` (384, 260, 96, 28), `Next` (488, 260, 96, 28) the default button.
- The map's segments, in panel x: 16..20, 21..25, 26..328, 329..379, 380..384; pixel (350,
  132) is `accent`, (100, 132) `menu.bg`, (26, 132) `text.dim`, (328, 132) the panel's
  surface.
- End on the slider → 72: `Windows keeps 182 GB, 28 GB of it free.`, the legend `Windows 182
  GB · EggOS 72 GB`. Down on the radios → `Use the whole disk`: the slider row hidden, the
  keeps line `Erases everything: Windows and 3 more partitions.` in `text`, the map two solid
  segments. Up → the slider back at 72.
- `Next` at 36: the confirmation with `Install EggOS beside Windows on /dev/vdb?`, `Shrinks
  Windows from 254 GB to 218 GB. Nothing is erased.`, `Writes an EggOS partition, 36 GB, for
  your files,`, `and adds EggOS to the EFI system partition.`, `Back up first: a resize can
  fail. Type vdb to confirm.`, and `Back` at (224, 224, 96, 28) in window coordinates. `Back`
  shows the plan panel with 36 still chosen.
- `vdb`, Enter: the fake's partition 3 is 218 GiB and its file system says the same; a fifth
  entry of type EggOS data, 36 GiB, starts where partition 3 ends, aligned to 1 MiB;
  partitions 1, 2 and 4 are byte for byte what they were except partition 1's new
  `\EFI\EggOS\` folder with its four files; `bootmgfw.efi` is unchanged; no `\EFI\BOOT\` file
  was replaced; the fake firmware's order is `EggOS`, `Windows Boot Manager`. The status line
  is the done line and the contents line `Windows and EggOS · 5 partitions`.
- A fake whose table write fails ends with the old table on the disk and `Could not write the
  partition table: the old one was put back.`; one whose shrink fails leaves the file system
  and the table as they were and no `\EFI\EggOS\` folder; a firmware that refuses the entry
  gives the second done line.
- An EFI system partition with 2 MB free: line 31 reads `and its own EFI system partition, 256
  MB.`, line 30 `35 GB` (35.7), and the installation writes a second EFI system partition at
  the start of the room.
- Refusals: an MBR fake opens the plan panel with radio 1 disabled, radio 2 selected, the two
  MBR lines at (224, 194) and (224, 214) and the erases line; `disk install --alongside`
  answers `table_is_mbr`. A BitLocker fake (partition 3 starts `-FVE-FS-`) shows the BitLocker
  lines; with 32 GiB unallocated instead it offers alongside with the slider at 32 of 1 to
  32, `Uses unallocated space. Windows is not touched.`, and the recovery-key line in the
  confirmation. A hibernated fake, a fake marked for a check, an APFS fake and a fake with
  0.6 GiB shrinkable each show their two lines and their code.
- `disk install --alongside --dry-run /dev/vdb` prints section 7's sentence; `--size 80G`
  answers `size_out_of_range`; `disk install /dev/vdb` asks v0.4's question naming `Windows
  and 3 more partitions`; `disk_install` with `alongside: true` and no `confirm` answers
  `needs_confirmation`.
- The loader's menu, rendered at 1280x800 with the fake firmware: rows at (440, 304), (440,
  344), (440, 384) reading `EggOS (default)`, `Windows Boot Manager`, `Firmware settings`; row
  1 `#2E4A6B` with `>` at x 456; the hint `Starts EggOS in 5 s`. Five seconds start EggOS.
  Down, Enter set the firmware's next entry to Windows and restart. D on row 2 writes
  `boot.default=windows`; T makes the wait 10. With no other entry the menu is not drawn and
  no wait passes. `settings get boot.entries` prints `eggos,windows,firmware`.
- A pane 400 wide shows the plan as a sheet with 48 px radios; the element tree holds the
  `radiogroup` `How to install`, the `graphic` `Disk layout` and the `slider` `EggOS size`
  valued `36 1..72 by 1`.

## Decided by design (the owner can overrule any of these)

1. The plan is its own panel before v0.4's confirmation, so the confirmation and the
   whole-disk path stay as they are.
2. `Install alongside` is the opening choice whenever it is possible; the command's default
   stays the whole disk, so existing scripts do what they did.
3. EggOS shrinks only NTFS and ext4, and only by their free tail: no file is ever moved.
   Everything else is shrunk in its own system.
4. One rule picks where EggOS goes; there is no chooser between several free places in v0.1.
5. The typed short name guards both choices.
6. The EFI system partition is shared when it has room, and EggOS writes its own when it has
   not, rather than refusing.
7. EggOS's entry goes first in the firmware's order, and the menu waits 5 s.
8. Another system is started by the firmware after a restart, not loaded by EggOS's loader:
   slower by one firmware start, and BitLocker and Secure Boot stay undisturbed.
9. The boot menu itself is the GUI of `boot.default` and `boot.menu_wait_s` (D and T); a
   Settings row for them is later.
10. The map tells kept from new by hollow and solid, not by colour alone.
11. The minimum for EggOS alongside is the whole-disk minimum, 1 GB.

## Left out of v0.1

Moving files to shrink further; shrinking APFS, Btrfs, XFS, LVM or an encrypted volume;
choosing among several free places, or a partition to replace; MBR disks alongside, and
converting MBR to GPT; installing beside an EggOS already on the disk as an upgrade or a
repair; removing EggOS and giving the space back; a progress bar and a cancel; `Restart into
Windows` from the power menu and a `Startup` row in Settings; a pointer in the boot menu; the
light theme in the boot menu; Secure Boot signing (`real-hardware.md`); Apple silicon Macs.

## Decisions for the engineer

1. `disk partitions <device>` and `disk_partitions`, with the record of section 1 (`table`,
   `partitions`, `unallocated`, `systems`, `shrinkable_bytes`, `shrink_blocked`); read-only
   probes for FAT, NTFS, BitLocker, APFS, HFS+, ext4, Btrfs, XFS, LUKS and swap.
2. The recognition table of section 1 and the names' joining; the placement rule (16 GiB of
   unallocated space first, else the largest shrinkable tail with a following run, else 1
   GiB of unallocated space); the reserve `min(8 GiB, free / 2)`.
3. Widgets 36 to 49 with their places; the plan panel 400x276; `CAPACITY` + 15; the states
   of 2.1; the slider's opening value; the map's 4 px minimum and its hollow, solid and
   empty segments.
4. The confirmation's lines of section 3 and `Back`; the step order, sentences and
   failures of section 4; both done lines.
5. `\EFI\EggOS\` with the loader, `KERNEL.ELF`, `CMDLINE.TXT` (EggOS's partition by GUID)
   and `BOOT.TXT`; the fallback path written only when empty; EggOS's own 256 MB partition
   when the shared one has no room. This needs `eggos-fat32` to add files to an existing
   volume, and the loader to read its files from the folder it was started from.
6. The firmware entry `EggOS`, first in the order, through the runtime variable services
   (GitHub issues), for alongside and whole-disk installs.
7. The loader's menu: when it shows, its entries, places, colours, keys, the wait, the
   high-contrast form, the serial form, and starting another entry by the firmware's
   next-boot variable and a restart.
8. `boot.default`, `boot.menu_wait_s` and `boot.entries` in `settings`, stored in `BOOT.TXT`.
9. `disk install --alongside [--size <size>]`, `disk_install`'s `alongside` and
   `size_bytes`, the question, the dry-run sentence and the terminal's hint; codes
   `table_is_mbr`, `table_damaged`, `table_full`, `partition_encrypted`, `system_hibernated`,
   `needs_check`, `cannot_shrink`, `no_room_alongside`, `size_out_of_range`.
10. Shrinking NTFS and ext4 by the free tail, the new size written last; refusing a
    hibernated, Fast Startup or unclean volume.
11. The contents line `Windows · 4 partitions` and the install line's `can keep` sentence.
12. The element names of section 10 and the layouts of section 11.
