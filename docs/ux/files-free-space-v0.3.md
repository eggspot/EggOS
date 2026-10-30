# Files: free space (v0.3)

Addendum by Claude Fable (D16) to `desktop-v0.3-gui.md` section 8.1, for the free space the
kernel now answers with `statfs`. Everything in 8.1 stands unless this says otherwise; its item
15 (`X free` as a limitation) and section 8's "there is no `X free`" note are superseded, and
the line in `docs/limitations.md` goes. Numbers are at 1x and double at 2x. The status bar's
right end says one thing: **what this place lets you write**, `12.3 GB free` on a writable file
system, `Read-only` on one that refuses, nothing when there is nothing honest to say. It is the
same number as `df`'s `Avail` column and the MCP tool `df`'s `available_bytes` (the bytes the
user can still write, never a root reserve); "free" is the user's word for it in all three.

1. **The text.** `<size> free`, the size from the Size column's formatter `size_text`, so the
   rounding is the one the user already reads in the list: `512 B free`, `4.2 KB free`,
   `12 MB free`, `1.5 GB free`, `120 GB free`; `0 B free` when the disk is full. No `of
   <total>`: the status bar shows the number the user acts on and nothing to compare it with
   (Finder's `12.3 GB available` and GNOME's `12.3 GB free` show one number; Explorer's `free of`
   is its drive tile, not its status bar); the total is `df`'s. One string in every profile,
   `free` in the macOS profile too: Finder's `available` is a synonym, not a name a switcher
   looks for, and `Trash`/`Recycle Bin` differ because they are names. `Read-only` in every
   profile as well; no other string.
2. **What each place shows**, from the `statfs` answer of the folder shown (the Trash view asks
   `/home/root`, the file system the Trash lives on, so emptying it changes the number where the
   user is looking): read-only flag set → `Read-only`, whatever the sizes; else total bytes
   greater than 0 → `<available bytes> free` (`f_bavail` x `f_frsize`); else nothing. So:
   `/home` (EggFS, or tmpfs without a disk) `X free`; `/tmp` `X free` (its budget out of RAM is
   what you can write there; no `of memory`, the dialogs already say `/tmp is not kept`);
   `/work` `X free`; `/media/host` and `/` (the initramfs) `Read-only`; `/Applications`
   whatever its file system answers; `/proc` and `/dev`, which report no blocks, nothing (or
   `Read-only` if the kernel flags them so: both are true). Nothing when `statfs` fails for any
   reason (an older kernel's `ENOSYS` included), and nothing when the list shows `Cannot read
   this folder`, `The host share is not mounted` or `The work disk is not attached`: the bar is
   `N items` alone, as today. Rule 6 of 8.1 holds: the right end is a fact, never a message.
3. **Placement.** Right-aligned, ending at `width - 12` (the left text starts at 12; the Text
   Editor's status bar ends its right text at `W - 12` too), `text.dim`, cell top +4, on the
   bar's `bar.bg`. The right text is drawn only when it starts at least 16 px after the left
   text's end; otherwise it is dropped whole (never ellipsised: `12.3 GB f...` is worse than
   nothing) and `N items` wins. At the minimum width 480 the widest case, `512 items` (72 px)
   and `1023 TB free` (96 px), leaves 288 px, so the drop never happens at 1x; the rule is
   there so no later wording can make it happen silently.
4. **When it is read.** Once, in `reload`, right after the folder is read: on entering a folder
   (`go`, `^`, `<`, a place, a typed path), F5 and `Reload`, and after Files' own work finishes
   (paste, move, trash, permanent delete, `Empty Trash`, rename, `New folder`, `Make Link`), all
   of which reload today. Never on a timer: `tick` does not call it, so an idle window costs the
   disk nothing, and a change another program makes shows at the next reload (F5), as its new
   files do. One `statfs` per reload beside the four `status` calls `set_places` already makes.
   The value is kept in the window (`free_space: FreeSpace`, `enum FreeSpace { Unknown,
   Bytes(u64), ReadOnly }`) and painted from there; a resize repaints, it does not re-read.
5. **Properties: no line.** A folder's panel keeps its five lines (`PROPERTIES_LINES` 5). Free
   space belongs to the file system, not the folder, and the status bar shows it on every view of
   that file system; Finder's and Explorer's folder panels show none either, and the total is
   `df`'s.
6. **The element tree** (D161). The status bar is not in the tree today (`elements.rs` pushes
   `Places`, `Path` and `Entries` only). It gets two `Text` elements under the root, named with
   their drawn text exactly as the Win32 layer names drawn text: `ITEMS_ELEMENT = 40` (`3 items`),
   its bounds the left text's cell, and `FREE_SPACE_ELEMENT = 41` (`12.3 GB free` or
   `Read-only`), its bounds the right text's cell, left out of the tree whenever nothing is drawn.
   No value: an agent that wants bytes calls the MCP tool `df`.
7. **Code.** `Files::space(&mut self, path: &[u8]) -> Result<Space, Errno>` on the trait, `pub
   struct Space { pub total: u64, pub available: u64, pub read_only: bool }` in bytes (the layer
   multiplies blocks by `f_frsize` once); `MemoryFiles::sized(folder, total, available)` beside
   `read_only(folder)`, which answers `Read-only` for its folders, and a call counter;
   `listing::free_space_text(FreeSpace) -> Option<Stamp>`; `FilesWindow::free_space_rect() ->
   Option<Rect>`, the right text's cell, for the QEMU mask as `time_column()` is;
   `STATUS_END_FROM_RIGHT: i32 = 12`, `STATUS_GAP: i32 = 16`.

Checks: host tests paint `3 items` at x 12 and `12.3 GB free` ending at `width - 12` in
`text.dim` for a folder whose mock space is 16 GiB total and 12.3 GiB available, `Read-only` on
`/media/host` and `/`, nothing for a mock answering zero blocks or `ENOSYS`, home's number in the
Trash view, and nothing while the list says `not mounted`; the counter proves one `space` call
per reload, one after a paste finishes, and none from `tick` over 10 s; the tree lists elements
40 and 41 with the drawn text and leaves 41 out when nothing is drawn; a window 480 wide still
draws both. QEMU screenshots: `/media/host` with `Read-only` (deterministic, in the host mirror
too); `/home/root` with the right text's cell masked by `free_space_rect()`, and element 41 read
through the tree equal to `size_text` of `df --json /home/root`'s `available` plus ` free`.
