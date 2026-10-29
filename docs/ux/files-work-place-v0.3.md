# Files: the work disk place (v0.3)

Addendum by Claude Fable (D16) to `desktop-v0.3-gui.md` section 8.1, for the repository as a
disk (D166). Everything in 8.1 stands unless this says otherwise. Numbers are at 1x and double
at 2x.

1. **The row.** Label `Work disk` in every profile (macOS, Windows and Linux have no word for
   it; Finder, Explorer and GNOME Files would show the volume's name, which EggOS does not
   know, and `Work` alone reads like a folder next to `Documents`). Path `/work`. It sits right
   after `Host share`, so the two things the host lends the machine stay together: `Home`,
   `Documents`, `Downloads`, `Desktop`, `tmp`, `Host share`, `Work disk`, `Apps`, `System`,
   `Trash`. Text at x 24, `>` at x 8, no icon, like every other row. Code: `Place::WorkDisk`,
   `WORK: &[u8] = b"/work"`, `MAX_PLACES = 10`.

2. **Shown only while a work disk is attached.** `work_disk_attached` is the share's test
   applied to `/work`: `status("/work")` and `status("/")` both succeed and their `device`
   fields differ; any error means not attached. `set_places` skips the row otherwise, exactly
   as it skips `Host share`. The row is checked on every reload, as the share is.

3. **Ten rows at the minimum height.** The minimum content height rises from 300 to 324:
   `MINIMUM_SIZE = (480, 324)`, in every profile and whatever is mounted (a constant, not a
   rule that changes with mounts, so the window never jumps when a disk appears). Ten rows from
   y 60 end at y 300, the status bar's top at 324 - 24 = 300: the same "last row the status
   bar allows" rule as the nine rows at 300. The default 720x480 is unchanged. Rejected:
   scrolling the sidebar (it would hide `Trash`, which 8.1 says is always the last row and
   always shown, behind an 8 px bar in a 160 px column for the sake of one row); hiding `tmp`
   or `System` when both mounts are present (rows that come and go for reasons the user cannot
   see).

4. **Gone while shown.** The row drops at the next reload and a window inside `/work` lists
   nothing but the line `The work disk is not attached` (item 14's `text.dim` line; "attached"
   is D166's word: the host attaches the disk at boot and takes it back at power-off, the user
   never mounts it). Code: the `paint_empty` arm for `HOST_SHARE` repeated for `WORK` and
   `Place::WorkDisk`.

   Inside `/work`, which is writable, everything works as under `/home` (new folder, rename,
   cut, copy, paste, Open, Install...), except the Trash. The Trash is on the home file system,
   so trashing a build tree would copy it there and then delete it, and the host would not get
   the files back from the Trash's side. So Delete in `/work` is permanent, like `/tmp`: the
   menu reads `Delete permanently` (it already does, since `trashable()` is "within `/home`"),
   Delete and Shift+Delete both ask, `Delete <name> permanently?` / `/work is the host's
   folder, so it has no Trash. This cannot be undone.`, `Cancel`, `Delete` (primary). Nothing
   else differs: no read-only notices, no dimmed items.

5. **Anything else the user sees:** none. The status bar stays `N items`; Properties says
   `Directory`, `File`, ... with `Where: /work/...` as anywhere; the path field accepts `/work`
   typed. A desktop icon or launcher entry for the work disk is not part of this addendum.

Checks: ten rows end at y 300 at the minimum height; the row is absent when `/work` shares
`/`'s device and present when it differs; Delete in `/work` asks the permanent dialog.
