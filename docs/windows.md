# Windows from programs: `/dev/window`

Programs open their own windows on the EggOS desktop through `/dev/window`, the window server's
device (D127, D128). The design is Claude Fable's (`docs/ux/desktop-v0.3-gui.md`, sections 2
to 5, D16). In v0.3.0 the server is part of the kernel (D126); this device is the contract that stays
when it moves to user space, so a program written against this page keeps working.

Windows GUI programs reach it through the Windows layer's user32 and gdi32
([Windows programs](windows-programs.md), D129), one connection per program. Linux Wayland
programs reach it through `eggway`, the Wayland server ([Linux programs](linux-programs.md),
D140), which opens one connection per Wayland client and one window per `xdg_toplevel`.

`userland/hello-window` is a complete example: it opens a window, draws a checkerboard into
it, shows it, redraws it at each new size the user resizes or maximises it to, and waits for
its close request.

## Using it

1. `open("/dev/window", O_RDWR)`: each open is one client. Closing its last descriptor
   destroys its windows. Without a display, `open` fails with `ENXIO`; with 64 clients
   connected, `ENFILE`.
2. `ioctl(fd, CREATE, &create)`: a window. Nothing is shown yet. The reply holds the window's
   id, its content size (the size asked for, clamped as below) and the offset of its pixel
   buffer.
3. `mmap(NULL, width * height * 4, PROT_READ | PROT_WRITE, MAP_SHARED, fd, offset)`: the
   window's pixels, `u32` `0x00RRGGBB`, rows of `width` pixels with no padding, zeroed at first.
   `MAP_PRIVATE` is refused with `EACCES`, an executable or inaccessible mapping with `EPERM`
   (writable and executable at once is `EACCES`, as for any mapping), an offset
   that is not one of the client's buffers with `EINVAL`. The mapping stays valid until it is
   unmapped, even after the window is destroyed or resized; it cannot be moved (`mremap`),
   discarded (`madvise`) or made executable or inaccessible (`mprotect`, `EINVAL`), and a
   process with one cannot `fork` (it can `vfork` and `execve`). Unmap the old buffer after a
   resize: a client's buffers, mapped or shown, hold at most 32 MiB together, every client's
   at most a quarter of memory, and none takes the kernel's reserve (`ENOMEM` past them, D155).
4. Draw, then `ioctl(fd, COMMIT, &commit)`: the first commit shows the window on top, focused,
   cascaded below the window under it (or centred when it is the first). Each commit copies the
   rectangles it names (or the whole content) to the screen during the call, so the program may
   draw again as soon as it returns.
5. `read(fd, buffer, n * 32)` takes event records, waiting for one unless the descriptor is
   `O_NONBLOCK` (`EAGAIN`); `poll` reports `POLLIN` while one waits. A read shorter than one
   record is `EINVAL`. React to `Resize` (call `SET_SIZE` with the size it gives, map the new
   buffer, draw, commit) and `CloseRequest` (destroy the window, or ask the user). A close
   request left unread for 2 s closes the window: the server destroys it (the program keeps
   running), its title saying `(not responding)` from 500 ms on.

`write` is `EINVAL`, `lseek` `ESPIPE`; `fstat` reports a character device (`S_IFCHR | 0666`,
device 240:0, inode 6 on the device files' file system 0:5).

## Requests

Linux's `_IOC` encoding: type `'w'` (0x77), the request's number, the argument's size, and the
direction (`W` the kernel reads the argument, `R` it writes it back). Every field is a 32-bit
integer in the machine's byte order unless marked 64; offsets are in bytes. Window ids are the
`u32` the kernel returned (they fit 16 bits; events carry them as `u16`).

| Request | Number | Size | Argument (offset: field) | Behaviour |
|---------|--------|------|--------------------------|-----------|
| `CREATE` | `0xc0207701` (RW) | 32 | 0: role, 4: parent (dialogs), 8: content width (in/out), 12: content height (in/out), 16: flags, 20: window id (out), 24: buffer offset (64, out) | Roles: 0 `Normal`, 1 `Dialog` (its parent must be a normal window of the client's). `Panel` 2, `Popup` 3 and `Background` 4 are the server's (`EPERM`). Flags: 1 `FIXED_SIZE` (no resize zones, the maximise control inactive, `SET_STATE` 2 refused), 2 `TAKES_CTRL_KEYS` (Ctrl+W reaches the window in the Windows and Linux profiles; Alt+F4 and the Super keys never do). The size is clamped to at least 158x62 and at most the work area less 32 each way (less 2 and 30 for the chrome). |
| `SET_TITLE` | `0x40487702` (W) | 72 | 0: window, 4: length, 8: title (64 bytes) | UTF-8, at most 64 bytes; drawn centred, cut with `...` when it does not fit. |
| `SET_SIZE` | `0xc0187703` (RW) | 24 | 0: window, 4: width (in/out), 8: height (in/out), 16: new buffer offset (64, out) | A new, zeroed buffer at the new size, clamped to the minimum size and the whole work area (less the chrome); its content shows the window background until the next commit. The answer to a `Resize`: while one is unanswered (the user is still dragging), only the buffer changes and the window keeps the size the user gave it, the buffer shown clipped. Otherwise the window takes the size, keeping its top-left corner, and a maximised window becomes normal (`StateChanged` 0). Map the new offset; the old mapping stays valid until unmapped. `EAGAIN` if the window's size kept changing under four attempts. |
| `SET_MINIMUM_SIZE` | `0x400c7704` (W) | 12 | 0: window, 4: width, 8: height | The smallest content `SET_SIZE` and the user's resizing may give it. |
| `SET_POSITION` | `0x400c7705` (W) | 12 | 0: window, 4: x, 8: y (signed, work-area coordinates of the window's outside top-left) | Clamped so the whole window is in the work area; applied when it is first shown if it is not yet. |
| `SET_STATE` | `0x40087706` (W) | 8 | 0: window, 4: state | 0 normal (restores from minimised and from maximised), 1 minimised (to the window list: the taskbar, the dock or a chip in the top bar), 2 maximised (fills the work area; `EINVAL` for a dialog or a `FIXED_SIZE` window). Dialogs cannot be minimised (`EINVAL`). |
| `SET_POINTER` | `0x40087707` (W) | 8 | 0: window, 4: shape | The pointer drawn over the window's content: 0 arrow, 1 I-beam, 2 north-south, 3 east-west, 4 north-west-south-east, 5 north-east-south-west. |
| `COMMIT` | `0x40887708` (W) | 136 | 0: window, 4: count (at most 8), 8: rects, each 16 bytes: x, y, width, height in content coordinates | Shows the window on its first commit; copies the rects, clipped to the content, or all of it when count is 0 (or after `SET_SIZE`). |
| `DESTROY` | `0x40047709` (W) | 4 | 0: window | Unshows and frees it (its buffer once no mapping holds it), and its dialogs. |
| `GET_WORK_AREA` | `0x8014770a` (R) | 20 | 0: x, 4: y, 8: width, 12: height, 16: scale | Where windows go (the screen less the profile's panels: above the taskbar, or between the top bar and the dock, or below the top bar; what a maximised window fills) and the scale, 1 in 0.3.0. |
| `LIST_WINDOW` | `0xc060770b` (RW) | 96 | 0: index (in); 0: window, 4: state, 8: x, 12: y, 16: width, 20: height (the outside rectangle on the screen), 24: client (65535 the kernel's), 28: title length, 32: title (64 bytes) (out) | The window at that index among the shown and minimised windows, oldest first; `ENOENT` past the last. Root only (`EPERM`); what `wm list` reads. |
| `MANAGE_WINDOW` | `0x4010770c` (W) | 16 | 0: window, 4: action, 8: a, 12: b (signed) | Any client's window: 0 focus (restoring it if minimised), 1 minimise, 2 maximise, 3 restore, 4 close (sends `CloseRequest`; `EPERM` for the terminal), 5 move its outside top-left to (a, b) on the screen, 6 give it content a by b (sends `Resize`). Root only (`EPERM`); what `wm`'s other commands make. |
| `GET_PROFILE` | `0x8004770d` (R) | 4 | 0: profile (out) | The desktop's layout profile (D132): 0 Windows habits, 1 macOS habits, 2 Linux habits. Any client. |
| `SET_PROFILE` | `0x4004770e` (W) | 4 | 0: profile | Makes it the desktop's at once: the shell's panels and work area change (D133), every window's controls move to the profile's side (left for macOS, right for the others) and its shortcuts apply; windows are moved into the new work area, maximised ones fill it and resizable ones larger than it shrink to fit (their clients get `Resize`); the others keep their size and buffer. The profile is not saved: Settings and `settings set` save it in the settings file, which the kernel reads at boot (D132). Root only, for now the active user's session (`EPERM`); `EINVAL` for another number. |
| `APPS_CHANGED` | `0x0000770f` | 0 | none | The installed apps changed (D134): the shell reads the catalogue again at once, so its launcher, dock and desktop icons follow; an open icon menu of an app that is gone closes. Any client; what `app install` and `app uninstall` send. |
| `OPEN_APP` | `0x40447710` (W) | 68 | 0: length, 4: name (64 bytes) | Opens the built-in app (`Terminal`, `Files`, `Settings`; one wins over an installed app of the same name, D141) or else the installed app of that name as a double-click on its icon does (its program typed in the terminal, after Ctrl-U), after reading the catalogue again. `ENOENT` when no such app is installed. Root only, for now the active user's session (`EPERM`); what the installer's `Open` sends. |
| `TYPE_IN_TERMINAL` | `0x41047711` (W) | 260 | 0: length, 4: line (256 bytes) | Types the line in the terminal window as a keyboard would (after Ctrl-U, which erases a half-typed line, and with Enter after it) and focuses the terminal (D135); a program running in the terminal receives it. `EINVAL` for a control character or a length past 256; `ENXIO` without an interactive desktop. Root only, for now the active user's session (`EPERM`); what Files sends to run a program or `cat` a text file. |
| `CAPTURE_WINDOW` | `0xc0287713` (RW) | 40 | 0: window, 4: first row, 8: row count (0 asks the size only), 12: content width (out), 16: content height (out), 20: rows copied (out), 24: buffer (64, the program's address), 32: buffer length (64) | Copies rows of any client's window's content, as the desktop shows it (the client's buffer, the window's fill around it; no chrome), into the buffer: packed `0x00RRGGBB` pixels in the machine's byte order, content-width pixels a row, straight from the window's own buffer, so a covered or minimised window is captured too (D141). The row count is cut at the content's height; `EINVAL` when the rows do not fit the length or the first row is past the height, `ENOENT` for no such window (one closed between two calls), `EOPNOTSUPP` for the terminal's window, which the kernel draws, `EFAULT` for a buffer outside the program's memory (the reply is then not written). Root only (`EPERM`); what `wm capture` reads, band by band. |
| `SEND_INPUT` | `0x40187714` (W) | 24 | 0: window, 4: kind, 8: x, 12: y (signed, content pixels), 16: code, 20: value | Sends one step of input straight to any window's queue (D141), never through the shortcuts, the popups or hit-testing, without moving the person's pointer and whether or not the window is on screen: kind 0 a pointer move to (x, y); 1 a click of button `code` (`BTN_LEFT` 0x110, `BTN_RIGHT` 0x111, `BTN_MIDDLE` 0x112) at (x, y), a move, a press and a release; 2 the wheel by `value` notches (signed, up when positive, at most 16 either way) at (x, y); 3 a press and release of key `code` with the modifier bits of `Key` records in `value` (1 Shift, 2 Ctrl, 4 Alt, 8 Super), the modifiers' left keys pressed around it; 4 the printable ASCII character `code`, typed as the US keymap types it. A window not focused gets `FocusIn` before and `FocusOut` after; pointer input to a window the pointer is not over comes between `PointerEnter` and `PointerLeave`; focus, hover and stacking stay as they were. The terminal's window takes keys, characters and the wheel (its own handlers: the console and the scrollback), and refuses pointer input with `EOPNOTSUPP`. `EBUSY` while a person uses the machine (below); `EAGAIN` when the client's queue lacks room for the whole step (nothing is sent; try again); `EINVAL` for an unknown kind, a negative or outside point, another button, a key code that is 0 or a button's, other modifier bits, no notch, or a character that is not printable ASCII; `ENOENT` for no such window. Root only (`EPERM`); what `wm click`, `type`, `press` and `scroll` send. |
| `START_MOVE_OR_RESIZE` | `0x400c7712` (W) | 12 | 0: window, 4: kind, 8: edges | While the window's content holds a left press (the button still down), hands that press to the server's own move (kind 0, edges 0), as a drag of the title bar, or resize (kind 1) by `edges` (1 left, 2 right, 4 top, 8 bottom; no two opposite ones), as a drag of those edges: the window follows the pointer, a maximised one is restored after 8 px, a resize sends `Resize`; the release ends it and reaches no client, and the client gets `PointerLeave`. Any client, on its own normal window or dialog (D140); `EINVAL` without the press, for a fixed-size or maximised window's resize, an unknown kind or bad edges. What `xdg_toplevel.move` and `resize` become. |
| `SET_MENU` | `0x41007715` (W) | 256 | 0: window, 4: pool length (at most 248), 8: pool (248 bytes) | Gives the window its main menu (D147), replacing the one it had; length 0 removes it. The pool is UTF-8 entries, each ending with `\n`: `M<title>` starts a menu, `I<label>\t<key label>` an item, `D<label>\t<key label>` a disabled item (the key label may be empty, the tab is always there), `-` a separator; at most 6 menus of at most 16 entries each (separators count), a menu first, no control characters in a label. The macOS profile's top bar shows the titles while the window, or a dialog of it, is focused (the dialog's dims them); choosing an item sends `MenuChosen`. Any client, on its own normal window; `EINVAL` otherwise or for a pool that breaks these rules. What `-[NSApplication setMainMenu:]` sends. |
| `GET_MENU` | `0xc1007716` (RW) | 256 | 0: window; back: 4: pool length, 8: pool (248 bytes) | Reads any window's main menu (D147): the pool `SET_MENU` last gave it, length 0 for none, so a command or an agent can list its menus and items (`wm menu`). `ENOENT` for no such window. Root only (`EPERM`). |
| `CHOOSE_MENU` | `0x41047717` (W) | 260 | 0: window, 4: menu index, 8: entry index, 12: label length, 16: label (244) | Chooses an enabled item of any window's main menu as a person's choice in the top bar does (D147): the window's menu closes if it is open and the window gets `MenuChosen`; it works on a covered, minimised or unfocused window, in every profile, and moves neither the focus nor the pointer. The label is the entry's as the caller read it with `GET_MENU`, compared byte for byte, so a menu the app set again since then is not chosen from by mistake. `EBUSY` while a person uses the machine (below), as `SEND_INPUT`; `EAGAIN` when the client's queue is full; `EINVAL` for no such menu or item, a separator, a disabled item, another label, or while the window's client has a dialog open (an app-modal alert: the app would only act on the choice once it is answered); `ENOENT` for no such window. Root only (`EPERM`); what `wm menu --choose` sends. |
| `GET_TREE` | `0xc0207718` (RW) | 32 | 0: window, 4: answer (out: 0 a tree, 1 pixels only), 8: tree length (out), 16: buffer (64, the program's address), 24: buffer length (64, at least 65536) | Asks any window's app for its element tree (D161, "Element trees" below): the app gets a `Query` record and answers with `ANSWER_QUERY`; the call waits up to 500 ms, then reports pixels only, as it does for an app that answers none. The tree, at most 64 KiB, is copied into the buffer. `EINVAL` for a buffer under 64 KiB, `ENOENT` for no such window, `EOPNOTSUPP` for the terminal's, `EAGAIN` when the app's queue is full or 8 queries already wait, `EINTR` for a signal, `EFAULT` for a buffer outside the program's memory. Root only (`EPERM`); what `wm show` reads. |
| `ACT_ON_ELEMENT` | `0xc1007719` (RW) | 256 | 0: window, 4: element, 8: action (1 invoke, 2 focus, 3 set-text, 4 select), 12: text length, 16: outcome (out: 0 done, 1 no such element, 2 not supported, 3 disabled), 24: text (232 bytes of UTF-8, for set-text) | Asks any window's app to act on one element of its tree (D161), as a person's click or keys would; waits up to 500 ms for the app's outcome. `EBUSY` while a person uses the machine, as `SEND_INPUT`; `ETIMEDOUT` when the app answers no queries (pixels only); `EINVAL` for an unknown action, element 0, a text past 232 bytes, not UTF-8 or holding a control character; `ENOENT`, `EOPNOTSUPP`, `EAGAIN` and `EINTR` as `GET_TREE`. Root only (`EPERM`); what `wm act` sends. |
| `READ_QUERY` | `0xc100771a` (RW) | 256 | 0: serial (in); 4: kind (0 tree, 1 action), 8: window, 12: element, 16: action, 20: text length, 24: text (232 bytes) (out) | The app reads the query of a `Query` record it got: the text set-text carries. `ENOENT` for no query of that serial waiting for this client (answered, or its asker stopped waiting). Any client, its own queries. |
| `ANSWER_QUERY` | `0x4018771b` (W) | 24 | 0: serial, 4: outcome (for a tree 0, or 1 for none: pixels only; for an action as `ACT_ON_ELEMENT`'s), 8: tree (64, the program's address), 16: tree length (64, at most 65536) | Answers a query once (D161); the asker wakes with it. `ENOENT` for no query of that serial waiting for this client, `EINVAL` for a tree past 64 KiB, `ENOMEM` without memory for it, `EFAULT` for a tree outside the program's memory. Any client, its own queries; what the toolkit and `eggway` send. |

Errors: a window that is not the client's, an unknown role, state, shape or action, unknown
flags, a title too long or not UTF-8, a menu pool `SET_MENU` refuses, more than 8 rects: `EINVAL`; a role the client may not
make, or a management request from a process that is not root: `EPERM`; 16 windows for the
client or 64 in all (the terminal's included): `ENOSPC`; no memory for a buffer (at most 4 MiB
each in 0.3.0, 32 MiB for all of a client's, a quarter of memory for every client's, D155): `ENOMEM`; an argument outside the program's
memory: `EFAULT`; any other request: `ENOTTY`.

## Events

Records of 32 bytes: 0: kind (`u16`), 2: window (`u16`), 4: time in milliseconds on the
monotonic clock (`u32`, wrapping), 8: `a` (`i32`), 12: `b` (`i32`), 16: `c` (`u32`), 20: `d`
(`u32`), 24: zero. Content coordinates have (0, 0) at the content's top-left; the chrome sends
nothing.

| Kind | Number | a | b | c | d | When |
|------|--------|---|---|---|---|------|
| `PointerEnter` | 1 | x | y | | | the pointer enters the content |
| `PointerMove` | 2 | x | y | buttons held (bit 0 left, 1 right, 2 middle) | | motion over the content, or anywhere while a press in it holds the pointer (then x and y may be negative or past the size) |
| `PointerLeave` | 3 | | | | | the pointer leaves the content, or the window is minimised |
| `Button` | 4 | x | y | `BTN_LEFT` 0x110, `BTN_RIGHT` 0x111, `BTN_MIDDLE` 0x112 | 1 pressed, 0 released | a press in the content (which raises and focuses the window and holds the pointer until every button is up) and its release |
| `Wheel` | 5 | x | y | | notches (`i32`, +1 up) | over the content |
| `Key` | 6 | Linux key code | 1 pressed, 0 released | modifiers: bit 0 Shift, 1 Ctrl, 2 Alt, 3 Super, 4 Caps Lock | the character under the US keymap with the modifiers (Ctrl+letter its control character; Enter 13, Tab 9, Backspace 8, Esc 27), or 0 | the window is focused |
| `FocusIn` / `FocusOut` | 7 / 8 | | | | | focus changes; also `FocusOut` when the switcher or the window menu opens over the focused window, and `FocusIn` when it closes |
| `CloseRequest` | 9 | | | | | the close control, Alt+F4, Cmd+W (Ctrl+W in the Windows and Linux profiles), the window menu or `wm close`; the server closes nothing itself, but see the 2 s rule |
| `Resize` | 10 | width | height | | | the server changed the content size: an edge or corner resize (live, as the pointer moves), maximise, restore, snap, the window menu's `Size`, `wm size`; a `Resize` replaces the window's `Resize` still queued, so only the latest size waits |
| `StateChanged` | 11 | state | | | | after a minimise (1), maximise (2) or restore (0, or 2 for a maximised window restored from minimised) |
| `Scale` | 12 | scale | | | | reserved |
| `Overflow` | 13 | records dropped | | | | the queue overflowed |
| `MenuChosen` | 14 | menu index (from 0) | entry index in that menu (from 0, separators counted) | | | an item of the window's main menu (`SET_MENU`) was chosen, by a click or Enter, or by `CHOOSE_MENU` |
| `Query` | 15 | serial | kind: 0 the element tree, 1 an action | element | action | another program asked for the window's element tree (`GET_TREE`) or an action on one element (`ACT_ON_ELEMENT`); answer it with `ANSWER_QUERY` within 500 ms (D161) |

Each client's queue holds 256 records. When it is full, a `PointerMove` replaces the newest
record if that is a `PointerMove` of the same window; otherwise the oldest record is dropped,
and the next record read is `Overflow` with the count. A record whose copy to the program fails
(`EFAULT`) is lost, as on Linux's input devices.

## What the user does with windows

Click anywhere on a window to raise and focus it (a click on its parent focuses an open
dialog instead); drag the title bar to move it (it stays inside the work area; a maximised
window dragged more than 8 px is restored under the pointer). Drag an edge or a corner to
resize it: a ring 2 px outside the border and 4 px inside it (2 px along the top), with 16x16
corners, where the pointer shows the resize shape. The three controls are close (red),
minimise (yellow) and maximise (green), from the left edge in the default macOS-habits profile,
and close, maximise, minimise from the right edge in the Windows and Linux profiles; hovering
one shows its glyph, and a maximised window's green control restores it. The yellow control
minimises the window to the window list (its taskbar item, an icon on the dock after the
separator, or a chip in the Linux top bar), which restores it; the red one, on the focused
window, sends `CloseRequest`. A double-click on the title bar maximises or restores.

Keys the desktop takes before the focused window (D128): Alt+Tab (Cmd+Tab in the macOS
profile) opens the switcher, every shown or minimised window most recently focused first;
each Tab moves on, Shift+Tab back, releasing Alt chooses, Esc keeps the focus; Alt+Esc cycles
without it. Alt+F4, Cmd+W (Ctrl+W in the Windows and Linux profiles, unless the window takes
Ctrl keys) close. Super+Up maximises, Super+Down restores or minimises, Super+Left and
Super+Right snap to a half, Cmd+M (Super+H in the Linux profile) minimises, Super+D minimises
every window and again restores them, Super+X opens the power menu. Alt+Space, or a
right-click on the title bar, opens the window menu: `Restore`, `Move`, `Size`, `Minimise`,
`Maximise`, `Close`; Up and Down choose, Enter acts; after `Move` or `Size` the arrows move the
window or its bottom-right corner by 8 px, Enter keeps it, Esc puts it back.

The terminal is one of the windows, works as before, keeps its 80x24 size (no resizing or
maximising) and declines closing. Other keys go to the focused window: the terminal's to the
shell, a program's as `Key` events.

The layout profile (Windows, macOS or Linux habits: the controls' side and the shortcuts) is
chosen in Settings or with `settings set shell.profile` ([Settings](settings.md), D132), which
use `GET_PROFILE` and `SET_PROFILE`.

From the command line, `wm` (docs/coreutils.md) lists the windows and focuses, minimises,
maximises, restores, closes, moves and sizes them; it uses `LIST_WINDOW` and `MANAGE_WINDOW`,
so it needs root. `wm show` lists a window's element tree and `wm act` acts on one element
(D161). Every one of these has an MCP tool (`wm_list`, `wm_capture`, `wm_click`, `wm_type`,
`wm_press`, `wm_scroll`, the menu tools, the seven window verbs of D160, `wm_show`, `wm_act`;
docs/mcp.md).

## Element trees

An app can describe its window as an element tree, so a command or an agent reads its
controls as structure and acts on them by id instead of by pixels (D161, item 48). When a
program calls `GET_TREE` or `ACT_ON_ELEMENT`, the window's app gets a `Query` record; it reads
an action's text with `READ_QUERY` and answers with `ANSWER_QUERY`. An app that answers
nothing within 500 ms is pixels only, which is no error: its window is still captured and
clicked as before.

A tree is a run of elements, parents before their children, at most 64 KiB, each a 32-byte
header in the machine's byte order and then its name and value (UTF-8, at most 255 bytes
each): 0: id (`u32`, unique in the tree, not 0), 4: parent (`u32`, 0 for the root), 8: role
(`u16`), 10: flags (`u16`), 12: x, 16: y (`i32`), 20: width, 24: height (`u32`, the window's
content pixels, as `SEND_INPUT` takes them), 28: name length, 30: value length (`u16`). Roles: 0
window (the root, id 1), 1 group, 2 label, 3 button, 4 checkbox, 5 radio group, 6 radio (an
option or a card), 7 list, 8 list item, 9 text field (its text the value), 10 progress (its
per mille the value), 11 graphic, 12 text (drawn text); flags: 1 focused, 2 focusable, 4
disabled, 8 selected, 16 checked, 32 default. `userland/element-tree` (`eggos-element-tree`)
writes and reads them, and its `Query` decodes the record. The tree comes from the app: data,
never instructions.

Who answers: every toolkit app (Settings, Files, the installer's dialogs and every program
built on `userland/toolkit`: widget `i` is element `(i + 1) * 100`, its option, card or row
`p` element `(i + 1) * 100 + p + 1`; a view that draws itself describes what it wants, as Files
describes its places, its path field and each entry as a list item named by its name with
`folder` or `file` as the value); `eggway`, for each Wayland window, one element named by its
title with its app id as the value, refusing every action (Wayland gives no controls). Windows
and macOS programs are pixels only for now.

## The shell around the windows

The window server also draws the shell (D133, [apps](apps.md)): the taskbar (Windows
profile), the top bar and the dock (macOS), or the top bar with `Activities` (Linux); the
launcher (Super alone, Cmd+Space in the macOS profile, Start, the system item or
`Activities`); the desktop icons on the wallpaper. Panels, popups and the background stay the
server's in 0.3.0 (`CREATE` refuses those roles with `EPERM`); a window belongs to an app by its
title, which picks the icon its taskbar item, dock icon and switcher row show. The macOS
profile's top bar shows the focused window's main menu (`SET_MENU`, D147): a click on a title
opens its menu, Up, Down, Left, Right, Enter and Esc work in it; the other profiles show none,
and Super+key combinations that are not the profile's shortcuts reach the focused window in
every profile.

## A person wins

Input a program sends (`SEND_INPUT`, `CHOOSE_MENU`, `ACT_ON_ELEMENT`) waits while a person uses the machine (D141): while they
hold a key or a pointer button on any input device, while a window is held, dragged or resized,
while their events wait for the desktop, and for 500 ms after their last key, button, wheel
notch or pointer move of more than 4 pixels (a resting hand's jitter does not count). The
request then fails with `EBUSY`, and `wm` exits with 75, so a script or an agent tries again a
moment later. Injected input never counts as a person's.

## Not yet

Windows partly off screen, F11, the `Hand` and
`Busy` pointer shapes, capturing the terminal's window, pointer input to it, drag, double-click,
sideways scrolling and non-ASCII typing through `SEND_INPUT`, input redirected to a window's
open dialog (it goes to the window named), element trees from Windows and macOS programs (an
HWND tree with drawn text, an NSView tree), key repeat as `Key` records with
value 2, scales other than 1, buffers over 4 MiB, the server in user space.
