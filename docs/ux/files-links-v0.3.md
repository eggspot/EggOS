# Files: links (v0.3)

Addendum by Claude Fable (D16) to `desktop-v0.3-gui.md` section 8.1, for symbolic and hard
links (D170). Everything in 8.1 stands unless this says otherwise. Numbers are at 1x and double
at 2x. One rule runs through it: **a link row stands for its target when you open it, and is
the link itself for everything else** (Cut, Copy, Rename, Trash, Delete, Properties), as in
Finder, Explorer and GNOME Files. Hard links are invisible: several names of one file are
several ordinary rows.

1. **The link row.** The type mark is the target's (the 12x10 folder for a folder, the 10x12
   page for anything else, including a target that is missing) with a link badge over its
   bottom-left corner: a 6x6 plate in `menu.bg` at (x + 7, y + 12) of the row, cutting the
   mark's corner as Explorer's overlay does, and inside it a 4x4 arrow pointing up and right in
   `text` (12.9:1 on `menu.bg`), rows from y + 13: `.###`, `..##`, `.#.#`, `#...` with column
   0 at x + 8. The selected row's `>` replaces the mark and its badge, as today; the cut row
   keeps both. A link to a folder sorts with the folders (Explorer, GNOME, Dolphin; Finder's
   "folders on top" is the odd one out) and is browsed as one. Size: the target's size for a
   link to a file (the link's own byte count is the length of its target text, which nobody
   wants in that column), `-` for a link to a folder or a broken link. Modified: the link's own
   time (when it was made or last re-pointed; Explorer and Finder do the same, and the target's
   time is one `Show Original` away). Programs reached through a link keep their 6x6 source
   badge after the name. Code: `files::Kind::Link` from `status` (which does not follow);
   `Entry.link: Link` with `enum Link { None, Sound, Broken }`; `Entry.kind` is the target's
   kind as if it were listed where it really is (`kind::of_target`: `Folder`, `Text`,
   `Program(_)`, `Archive`, `File`, and `App(_)`/`Bundle` for a link to an installed app or an
   `.app`), `Kind::Other` when broken; `Files::status_following` for the target's status;
   `paint::link_badge`, `LINK_PLATE: Rect = Rect::new(7, 12, 6, 6)`.

2. **A broken link** (its target is missing, or it loops back to itself) draws its name, size
   (`-`) and time in `text.dim`, like the cut row, with the page mark and the badge in their
   normal colours. Open shows a notice: `<name> points to <target>, which does not exist.`
   (`ENOENT`, `ENOTDIR`), `<name> points back to itself.` (`ELOOP`), otherwise `Could not open
   <name> (<errno name>).`; the `<target>` is the stored text and the notice ellipsises as any.

3. **Opening a link** (double-click, Enter, `Open`, Cmd+Down, Cmd+O) acts on the target as
   item 11 would open it where it really is: a folder is browsed into, an app opens, a Windows
   or macOS program opens the installer's prompt, a Linux program runs, a text file opens in the
   editor or `cat`. Files passes the link's path; the kernel walks through it. A folder opened
   through a link shows the path **through the link** in the path field (`/home/root/Desktop/
   Projects link`), as Explorer and GNOME Files do and as `cd` keeps `$PWD`; Finder's jump to
   the real path is rejected because `^` would then land somewhere the user never saw. `^` and
   Backspace go to the link's folder with the link selected (today's `up` rule, unchanged);
   `<` goes where you came from. Code: no change to `go`, `up` or `back`; `open` matches on
   `entry.kind` as today, since the kind is the target's.

4. **Going to the target: `Show Original`** (`Open file location` in the Windows profile:
   Explorer's words; `Show Original` is Finder's, and GNOME's `Open Item Location` means the
   same). On a link row only, right after `Open` (and `Install...`), before the first
   separator. It goes to the folder the stored target names (relative text joined to the link's
   folder and normalised, absolute text as is; one level, what `readlink` prints, never a full
   resolution) and selects the target's name there, pushing history like any navigation; a
   folder that cannot be opened shows the usual `Could not open <folder> (<errno name>).`.
   `[dim]` on a broken link. No key: Finder's Cmd+R is Files' reload synonym in every profile
   and stays so. Not in the Trash (a trashed row's menu is its own). Code:
   `item::SHOW_ORIGINAL = 16`, `Files::read_link(path, &mut [u8; PATH_CAPACITY])`,
   `names::link_target_path(folder, target)`, then `enter(files, parent, Some(name))`.

5. **Making a link: `Make Link`** (`Create shortcut` in the Windows profile: Explorer's words.
   Finder's `Make Alias` is rejected: an alias follows its original when it moves, a symbolic
   link does not, and the words must not promise that; GNOME's `Make Link` is plain and true).
   On every row that has `Cut` and `Copy` (file, folder, program, `.app` bundle, and a link,
   which gets a link to the link, as `ln -s` does), first in the rename group: `Open`
   [`Install...`] [`Show Original`] ─ `Cut` `Copy` [`Paste`] ─ `Make Link` `Rename` `Move to
   Trash` ─ `Properties`. `[dim]` on the host share; a folder that refuses (the system tree)
   gives the notice `Could not make a link to <name> (<errno name>).`. Not on an app folder
   row in `/Applications` (its menu stays `Open` `Uninstall...` ─ `Properties`: a link made
   there would sit in the catalogue; app shortcuts on the Desktop are a later addendum, with
   the launcher's view of links), not in the Trash, not on empty space. The link is made in
   the same folder, named `<stem> link<.ext>` (`report link.txt`, `Projects link`), then
   `<stem> link 2<.ext>`, ... when taken: the shape and clash rule of `<stem> copy<.ext>`, so
   it sorts next to its original and keeps the extension at the end; one name in every profile
   (a file name is data, not wording). It is selected and renaming in place at once, like `New
   folder`, with item 10's caret rule. The stored target is the **absolute** path of the row
   (`entry_path`, the path through any link you came by): a link is made to be cut and pasted
   onto the Desktop, and a relative name would break the moment it left the folder (Nautilus
   stores absolute paths too). No key (Finder's Cmd+L is Files' path field in every profile;
   GNOME's Ctrl+M is minimise, Cmd+M, in the macOS profile); Shift+F10 reaches it. No `Paste
   link` item in this addendum. Code: `item::MAKE_LINK = 15`, `Files::make_link(target,
   path)`, `names::link_name(name, folder, number)` with `LINK: &[u8] = b" link"` beside
   `copy_name`, `free_path` as today.

6. **Everything else acts on the link, never its target.** Rename renames the link (its target
   text is untouched). Cut, Copy and Paste move or copy the link: a pasted copy is a new link
   with the same target text (`cp -P`; Finder and Explorer copy the alias or `.lnk`), named by
   item 8's rule, so a copied link to `/home/root/Projects` works wherever it lands; Paste on a
   folder-link row pastes into the target (Explorer's folder symlinks), `[dim]` when broken, and
   `Cannot paste a folder into itself.` is checked against the resolved target. Move to Trash
   moves the link file; the trashed row keeps its badge, Restore puts the link back, Delete
   permanently removes the link alone; `/tmp` and `/work` ask their permanent dialogs as for
   any row. A folder being copied, moved across file systems or deleted carries its links as
   links and never descends into one. `Uninstall...` never appears on a link, even one to an
   installed app: Delete there trashes the link. Code: `job.rs` copies a `Kind::Link` entry by
   `read_link` + `make_link` (today's `EINVAL` arm); `Walk::next` already treats a
   non-following `status` of a link as not a directory.

7. **Properties of a link**, five lines as today: the name; `Kind: Link to <Directory | Text |
   File | Zip archive | Windows program, x86_64 | ... | App, Windows>` (the target's `describe`
   text after `Link to `), or `Kind: Broken link`; `Target: <stored text>` exactly as stored
   (`/home/root/Projects`, `../notes.txt`); `Modified: YYYY-MM-DD HH:MM` (the link's own);
   `Where: <folder path>`. No `Size:` line: the row shows the target's size and `Show Original`
   the rest. A trashed link gets `Deleted:` and `From:` as item 4. A file with several
   hard-linked names says nothing (no `Names: 2`): Finder, Explorer and GNOME Files show
   nothing either, and the line would need a sixth row for every file. Code: `kind::describe`
   prefixes `Link to `; `PROPERTIES_LINES` stays 5.

8. **Places and the path field: nothing special.** A typed path through a link opens (the
   kernel walks it); a typed path to a link to a file fails with today's `Could not open <path>
   (ENOTDIR).`, a broken one with `(ENOENT)`. The sidebar highlights a place by the path text
   alone, so a place reached through a link highlights no row, as any non-place path today.
   Links in the read-only system tree (`System`) list, open and show Properties like any; only
   `Make Link`, `Rename`, `Cut` and `Move to Trash` fail there with their errno notices.

9. **Anything else the user sees:** none. The status bar stays `N items` (a link is one item);
   type-ahead, hidden `.` names, the scrollbar and the progress panel are unchanged. Not part
   of this addendum: the link badge on desktop icons and in the launcher, app shortcuts on the
   Desktop made from `/Applications`, `Paste link`, a `Size:` or hard-link count in
   Properties, and a way to re-point a broken link. The CLI and MCP front-ends of the same
   capability are `ln -s <target> <link>`, `readlink` and the `ln` tool (D22); the GUI's `Make
   Link` is exactly `ln -s <absolute row path> "<stem> link<.ext>"`.

Checks: host tests hash a folder-link row and a file-link row (the plate at (x + 7, y + 12),
the arrow's 10 pixels in `text`) and a broken one (`text.dim` name, `-` size); a link to a
folder sorts among folders and shows `-`, a link to a file the target's size and the link's own
time; `Make Link` on `report.txt` makes `report link.txt` with the absolute target, selected
and renaming, then `report link 2.txt`; `Create shortcut`/`Open file location` in the Windows
wording, `Make Link`/`Show Original` otherwise; `Show Original` enters the target's folder with
it selected and is dim on a broken link; opening a folder link shows the path through the link
and `^` returns to the link's folder with the link selected; Copy and Paste of a link makes a
link with the same target text; Move to Trash moves the link and its target stays; Open on a
broken link shows `<name> points to <target>, which does not exist.`; Properties reads `Kind:
Link to Directory` and `Target: /home/root/Projects`. QEMU screenshots: `/home/root` with
`Projects link`, `notes link.txt` and a broken link, a link row's context menu in both
wordings, and a link's Properties panel.
