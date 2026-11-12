# Files: drag and drop (v0.5)

Addendum by Claude Fable (D16) to `desktop-v0.3-gui.md` section 8.1, on top of toolkit v0.5
section 7 (the drag inside a window) and D393/D394 (the window server carries a drag between
windows). Everything there stands unless this says otherwise. Numbers are at 1x and double at
2x. One rule runs through it: **a drop does what Cut or Copy and Paste would do to the same
items in the same folder** (item 8's names, item 5's progress panel, item 6's notices), so the
drag is a faster hand for an action that already exists, not a second file engine. The
habits are Finder's, Explorer's and GNOME Files', which agree on nearly all of it; where they
differ, the choice and its reason are stated.

## 0. What a person gets, and how that is checked

Pick up a file or a folder in Files and let it go on a folder, a place in the sidebar, the
Trash, another Files window or the Text Editor; it moves, copies, is trashed or opens, with
the same highlight the toolkit already draws, Esc backing out at any moment. Checked by the
host tests of section 11 and the QEMU check there.

## 1. What can be dragged

- **Sources**: the entries table's rows (list view) and the grid's cells (icon view), in every
  folder and in the Trash view. The sidebar's places are not sources (reordering them is
  section 12). Nothing else in Files is a source.
- **The set**: at `Response::DragStarted(table, row)`, the dragged set is the whole selection
  when `row` is in it, else that row alone, which becomes the selection (Finder and Explorer
  both select the pressed row and drag it alone). The set is captured then: `DragSet { paths:
  Vec<PathBuf>, from: PathBuf, from_trash: bool }`, every path absolute, all in one folder.
- **The ghost's words** (`drag_words`): one item, its name; two or more, `<N> items` (`2
  items`). A name longer than 30 glyphs is end-ellipsised by Files to 27 glyphs and `...`
  before the hand-over, since the server draws the words it is given. The ghost is the
  toolkit's tile in both views (the server's is text-only, D393, and the two must match when
  the drag leaves the window; a 32 px icon in the ghost is section 12).
- **Refused**: `DragStarted` on a row that is renaming in place, or while a dialog or the
  progress panel shows, is answered `tree.cancel_drag()`. The host share's rows **can** be
  dragged (a copy out of a read-only share is the share's main use).

## 2. Drop targets in Files

Files declares the table and the grid `set_drag_source(true)`, `set_drag_out(true)` and
`set_drop_target(DropMode::OntoOrWhole)`; the sidebar list `set_drop_target(DropMode::Onto)`.

- **`DropMode::OntoOrWhole`** (new in the toolkit): the row under the pointer is the target
  when it is droppable, else the whole widget is; `Dropped`/`ForeignDropped` carry `at = row`,
  or `at = rows` (one past the last row) for the whole widget. The whole-widget highlight is a
  2 px `accent` outline (`INSET_R4`) at inset 0 of the widget's rect, no fill (Finder's
  outline of the window's content; a fill would hide the rows). The row highlight is 7.2's.
- **`WidgetTree::set_drop_row(id, row, bool)`** (new): a row that is not droppable is skipped
  (the whole-widget target applies there instead). Files sets it after every listing: folder
  rows and links to folders `true`, every other row `false`. The toolkit's own rule stands:
  the source's rows highlight nothing for the window's own drag.
- **Targets**, then: a folder row or cell (Onto: into that folder); the list's empty area, its
  header and any file row (Whole: into the folder shown); a sidebar place (Onto: into its
  folder); `Trash` (Onto: to the Trash). The whole-widget target is set **off**
  (`set_drop_whole(id, false)`, the third new call) while the folder shown is the dragged
  set's own folder, the host share, or the Trash view, and on while it is any other folder.
- **Not targets, so nothing highlights**: a file row; the dragged rows themselves and the
  boundaries of 7.2; the folder the set is already in (whole off, above); `Host share` and
  the share's folders (read-only, as `Paste` is `[dim]` there); the Trash view's rows and its
  empty area (dropping onto the `Trash` place is the way in); `Trash` when the set is on the
  host share (as `Move to Trash` is `[dim]` there) or already in the Trash.
- **A foreign drag** (`Input::DragOver` with a new serial): Files reads the payload once per
  serial with `WindowServer::drag_payload` (the kernel keeps it like the clipboard, so a read
  on the first `DragOver` costs one request) and decides the targets before the first
  highlight: kind 2 (text) sets every target off; kind 1 builds a `DragSet` from the lines
  and applies the rules above, with `set_drop_row(.., false)` on every folder row whose path
  is one of the paths or under one (a folder is never dropped into itself or its own child:
  Finder's and Explorer's rule, and item 6's `Cannot paste a folder into itself.`). A
  `DragLeave` clears nothing of that; the next serial recomputes.

## 3. Move, copy, link, trash

The modifier is read **at the release** (Finder and Explorer: you may press it after picking
up), one map in every profile, as item 12 maps keys; precedence link, copy, move, default:

| Held at release | Action | Habit |
|---|---|---|
| nothing | **move** when source and target are on one file system (`st_dev` equal), else **copy** | all three |
| Ctrl or Alt | **copy** | Explorer and GNOME Ctrl; Finder Option |
| Shift, or Mod alone | **move** (across file systems: copy then delete, item 8) | Explorer and GNOME Shift; Finder Cmd |
| Mod+Shift or Mod+Alt | **link** (`Paste Link`'s rule: a symbolic link per item, `<stem> link<.ext>`) | Explorer and GNOME Ctrl+Shift; Finder Cmd+Option |

Explorer's Alt alone (a shortcut) becomes a copy here; nothing is lost by it, and Option as
copy is the habit of the default profile. A drop on `Trash` ignores the modifiers.

- **Doing it**: the window that gets the drop does the work, through the paste code: a move on
  one file system is a rename per item; a copy, a cross-file-system move and a trashing of
  many run item 5's progress panel (`Moving <name>...`, the current item's name, the bar over
  the total bytes; `Cancel` keeps what is done). Names never overwrite: item 8's `<stem>
  copy<.ext>`. The new items are selected in the target folder, the first one the focus row.
- **`Trash`**: `Move to Trash` on the set, no dialog; a set in `/tmp` asks item 4's `Delete
  <N> items permanently?` first (`Delete <name> permanently?` for one), and the dialog's
  cancel leaves everything in place. The dropped names are not selected (they are gone).
- **Out of the Trash**: a drop from the Trash view into a folder is a move there that drops
  the item's `info` entry (a `Restore` to the folder you chose; Finder and Explorer allow it).
  Copy and link from the Trash are not offered: the modifiers are ignored, it is a move.
- **Errors** are item 6's notices in the target window: `Cannot move a folder into itself.`
  (the drop that got through anyway, say by a path renamed under it), `Could not <copy|move>
  <name> (<errno name>).`, `The host share is read-only.`; a notice stops the batch at that
  item, what is done stays. A set whose items vanished before the drop (another window moved
  them) shows `Could not move <name> (ENOENT).`.

## 4. Between windows

- **Leaving**: the pointer crossing the window's edge gives `Response::DragLeftWindow(table,
  row)`; Files hands over `start_drag(kind 1, paths one per line, drag_words)`. The source's
  rows stay as they were (selected, not dimmed: a drag is not a cut), the status bar unchanged.
- **Into another Files window**: as section 2; the drop's folder is the target window's.
- **Onto the Text Editor**: the editor declares its text view `set_drop_target(DropMode::
  Whole)` (new: the whole widget, outline only, for widgets that draw their own content;
  `at` 0). `ForeignDropped` with kind 1 opens the **first** path as `Open...` does (the
  unsaved-changes question first; a document replaced in this window, classic Notepad's
  habit; TextEdit inserts the file's text, which nobody expects from a drop on a window). Two
  or more paths: the first opens and the notice `Dropped <N> files: opened <name>.` says so
  (opening many is Files' `Open` on the selection). A folder first: the notice `Cannot open a
  folder.`. Kind 2: nothing, and the editor sets its target off for that serial as Files does.
- **Text dropped on Files** (kind 2): nothing, no highlight (a new text file from a drop is
  section 12).
- **The source after `DragEnded`**: `landed: false` restores nothing (nothing changed).
  `landed: true` reloads the listing at once (a rename is done before the event reaches the
  source) and once more when the window next gains focus (`reload_on_focus`, one shot: a
  cross-file-system move may still be running its panel in the other window). The selection
  survives a reload by path, as F5 keeps it. The source never knows what the drop did; it
  shows the folder as it is.
- **Pointer back over the source**: the source hears `DragOver` like any window and applies
  section 2 to its own payload (the whole target is off there, folder rows still take it).

## 5. Highlights, pointer, spring-loaded folders

- The toolkit's: `Hand` throughout, `menu.hover` fill with the 2 px `accent` outline on a row
  or cell (`INSET_R4` on rows, r6 on icon cells), the outline alone for the whole widget,
  the 24 px edge scroll. The ghost is 7.2's tile; no `+` or link badge on it in this version
  (D393 fixes the server's ghost at the hand-over, so a badge that follows the modifier
  needs a protocol change: section 12).
- **Spring-loaded folders, yes** (Finder and GNOME Files; Explorer only in its tree): the
  pointer resting on a droppable folder row, cell or sidebar place (within 4 px) for 1000 ms
  (`DRAG_SPRING_MS`, by `tick`) browses the window into that folder, nothing flashes. For the
  window's own drag Files first hands the drag to the server (`tree.cancel_drag()`, then
  `start_drag` as section 4; the window holds the pointer, so D393 allows it), so after the
  spring there is one kind of drag and the sources' indices no longer matter. `Trash`, the
  host share and `Host share` never spring. Esc after a spring cancels the drag and leaves
  the window in the sprung folder (Finder's list view does the same; `Back` is Alt+Left).

## 6. Keys

Esc cancels (the toolkit's drag, or the server's carried one). Nothing else during a drag;
the modifiers of section 3 are read at the release. The keyboard's way is the one that
exists: `Cut` or `Copy`, go to the folder, `Paste` (Mod+X, Mod+C, Mod+V, item 12), and
`Paste Link` for links. No new menu item and no new key.

## 7. What an agent sees (D161, `wm show`)

No new element and no new MCP tool: a drag has no element (toolkit v0.5 section 10). An agent
moves or copies files with `mv`, `cp`, `ln -s` and `files` (D22), or with `Entries`' `set-text
"2,5"` and the `Cut` and `Paste` `menuitem`s; D162's `wm drag` drives the toolkit's drag inside
one window (a folder row is reached as a drop that way); across windows it is D393's "not yet".
While a drag is carried, `describe` shows the windows unchanged (no ghost element) and the
target window's `Entries` keeps its value. Every move, copy, trash and link lands in the
agent-visible log as the paste it is.

## 8. Device classes (D23)

- **Desktop and laptop**: all of the above.
- **Tablet**: the same; a long press (500 ms, the toolkit's) picks a row up, the ghost at
  `(12, -36)`; the drop between windows needs two windows side by side (the adaptive shell's
  split) and works as section 4. No modifier on touch: the default of section 3's first row
  only (a touch-only copy is section 12). The Text Editor's drop target is the same.
- **Phone**: inside the one Files window only (the shell shows one window; a hand-over to
  the server still happens at the edge, and ends `landed: false`).
- **TV and wearable**: no drag (no pointer); `Cut`, `Copy`, `Paste` from the menu and the
  context menu are the way, as today.

## 9. Profile wording

`Trash` reads `Recycle Bin` in the Windows profile, in the dialog of section 3 and nowhere
else new. All other words are the same in every profile.

## 10. Code names

`files::drag` module: `DragSet`, `DropAction { Move, Copy, Link, Trash }`, `drop_action(mods:
Modifiers, same_fs: bool, onto_trash: bool) -> DropAction`, `drag_words(set) -> String`,
`DRAG_SPRING_MS: u32 = 1000`, `DRAG_WORDS_MAX_GLYPHS: usize = 30`, `reload_on_focus: bool`.
Toolkit: `DropMode::OntoOrWhole`, `DropMode::Whole`, `set_drop_row`, `set_drop_whole`.
Text Editor: `drop_open(paths: &[PathBuf])`, notices `Dropped <N> files: opened <name>.` and
`Cannot open a folder.` added to section 8's table.

## 11. Acceptance (host tests, Files content 720x480, scale 1x, dark palette; a fake window
server records `start_drag`, feeds `DragOver`/`Drop`/`DragEnded`)

Fixture: `/home/root/Documents` holds folders `A`, `B` (`B/A` inside) and files `a.txt`,
`b.txt`; `/tmp` is another file system; `/media/host` mounted read-only.

1. Press `a.txt` at its row, move 7 px → `DragStarted`; ghost tile with `a.txt`; `a.txt` is
   the selection alone. Release over folder `A` → `Dropped { at: row of A }`; `a.txt` is in
   `A`, the listing reloaded, no panel (one rename).
2. Select `a.txt` and `b.txt`, drag `b.txt` → ghost `2 items`; release on `A` → both moved.
3. Drag `a.txt` over `b.txt` → no highlight; release → `DragCancelled`, nothing moved.
4. Drag `A` over `A` → nothing; over the empty area → nothing (whole off in its own folder).
5. Drag `a.txt` onto the sidebar's `tmp` → copied (different `st_dev`), the progress panel
   when the fake file system delays, `Copying a.txt...`; the source keeps `a.txt`. Same with
   Shift held at release → moved (copy then delete), `a.txt` gone from `Documents`.
6. Drag `a.txt` onto `B` with Alt held at release → `a.txt` stays and `B/a.txt` exists; with
   Mod+Shift → `B/a link.txt` a symlink to `/home/root/Documents/a.txt`.
7. Drag `a.txt` onto `Trash` → trashed, no dialog, its row gone; from `/tmp` → the dialog
   `Delete a.txt permanently?`, Esc leaves it. On the share, `Trash` highlights nothing.
8. Hover `B` with a drag for 999 ms → nothing; 1000 ms → the fake server recorded
   `start_drag(kind 1, "/home/root/Documents/a.txt\n", "a.txt")`, the window shows `B`, the
   whole outline shows on `DragOver` at (400, 300), `Drop` → `B/a.txt`, `A` folder row inside
   `B` still takes a drop.
9. Foreign `DragOver` with kind 1 `"/home/root/Documents/A\n"` while the window shows `A` →
   no highlight anywhere (the set's own folder); while it shows `B`, over the row `B/A` → the
   row highlights (another folder of the same name, not the dragged one); over `B`'s empty
   area → the outline; `Drop` there → `Documents/A` moved in as `B/A copy` (item 8's name).
10. Foreign kind 2 → no highlight anywhere; `Drop` → nothing, no notice.
11. Pointer leaves the window with `a.txt` dragged → `DragLeftWindow`, `start_drag` recorded
    with `"a.txt"`; `DragEnded { landed: true }` → one reload now; focus gained → one more;
    `landed: false` → none.
12. Text Editor fixture (`Untitled`, unmodified): `Drop` kind 1 `"/home/root/Documents/a.txt\n
    /home/root/Documents/b.txt\n"` → the document is `a.txt`, notice `Dropped 2 files: opened
    a.txt.`; a modified document first asks `Save changes to Untitled?`; a folder → `Cannot open
    a folder.`; the outline shows at inset 0 of the text view on `DragOver`, no fill.

QEMU check: boot the desktop, open Files on `/home/root/Documents` with the fixture, `wm drag`
`a.txt`'s row onto `A` (D162, in-window), `ls` shows `A/a.txt`; screenshot the window with the
ghost mid-drag (a `wm drag` with a long step) hashed against the host test's frame. The
cross-window check waits for `wm drag` across windows (D393's "not yet"); until then the fake
server in the host tests covers sections 4 and 5.

## 12. Not in this version

A `+` or link badge on the ghost following the modifier (needs a ghost update request in the
server's protocol); the icon in the ghost; dropping on the desktop, the dock and the Terminal
(a path pasted); reordering the sidebar's places (`Between` on the list); a new text file from
dropped text; dragging text out of the Text Editor; a replace-or-keep-both dialog on a name
clash (Files never overwrites); undo of a move; copy on touch (a long-press menu); `wm drag`
across windows; drops that uninstall from `/Applications`.
