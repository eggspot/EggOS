# Windows from programs: `/dev/window`

Programs open their own windows on the EggOS desktop through `/dev/window`, the window server's
device (D127). The design is Claude Fable's (`docs/ux/desktop-v0.3-gui.md`, sections 3 and 4,
D16). In v0.3.0 the server is part of the kernel (D126); this device is the contract that stays
when it moves to user space, so a program written against this page keeps working.

`userland/hello-window` is a complete example: it opens a window, draws a checkerboard into
it, shows it and waits for its close request.

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
   unmapped, even after the window is destroyed or resized; it cannot be moved (`mremap`) or
   discarded (`madvise`), and a process with one cannot `fork` (it can `vfork` and `execve`).
4. Draw, then `ioctl(fd, COMMIT, &commit)`: the first commit shows the window on top, focused,
   cascaded below the window under it (or centred when it is the first). Each commit copies the
   rectangles it names (or the whole content) to the screen during the call, so the program may
   draw again as soon as it returns.
5. `read(fd, buffer, n * 32)` takes event records, waiting for one unless the descriptor is
   `O_NONBLOCK` (`EAGAIN`); `poll` reports `POLLIN` while one waits. A read shorter than one
   record is `EINVAL`. React to `CloseRequest` (destroy the window, or ask the user).

`write` is `EINVAL`, `lseek` `ESPIPE`; `fstat` reports a character device (`S_IFCHR | 0666`,
device 240:0, inode 6 on the device files' file system 0:5).

## Requests

Linux's `_IOC` encoding: type `'w'` (0x77), the request's number, the argument's size, and the
direction (`W` the kernel reads the argument, `R` it writes it back). Every field is a 32-bit
integer in the machine's byte order unless marked 64; offsets are in bytes. Window ids are the
`u32` the kernel returned (they fit 16 bits; events carry them as `u16`).

| Request | Number | Size | Argument (offset: field) | Behaviour |
|---------|--------|------|--------------------------|-----------|
| `CREATE` | `0xc0207701` (RW) | 32 | 0: role, 4: parent (dialogs), 8: content width (in/out), 12: content height (in/out), 16: flags, 20: window id (out), 24: buffer offset (64, out) | Roles: 0 `Normal`, 1 `Dialog` (its parent must be a normal window of the client's). `Panel` 2, `Popup` 3 and `Background` 4 are the server's (`EPERM`). Flags: 1 `FIXED_SIZE`, 2 `TAKES_CTRL_KEYS` (used from item 31). The size is clamped to at least 158x62 and at most the work area less 32 each way (less 2 and 30 for the chrome). |
| `SET_TITLE` | `0x40487702` (W) | 72 | 0: window, 4: length, 8: title (64 bytes) | UTF-8, at most 64 bytes; drawn centred, cut with `...` when it does not fit. |
| `SET_SIZE` | `0xc0187703` (RW) | 24 | 0: window, 4: width (in/out), 8: height (in/out), 16: new buffer offset (64, out) | A new, zeroed buffer at the new size (clamped as `CREATE`, and to the minimum size); the window keeps its top-left corner; its content shows the window background until the next commit. Map the new offset; the old mapping stays valid until unmapped. |
| `SET_MINIMUM_SIZE` | `0x400c7704` (W) | 12 | 0: window, 4: width, 8: height | The smallest content `SET_SIZE` (and from item 31 the user) may give it. |
| `SET_POSITION` | `0x400c7705` (W) | 12 | 0: window, 4: x, 8: y (signed, work-area coordinates of the window's outside top-left) | Clamped so the whole window is in the work area; applied when it is first shown if it is not yet. |
| `SET_STATE` | `0x40087706` (W) | 8 | 0: window, 4: state | 0 normal (restores), 1 minimised (to a chip in the top bar); 2 maximised is `EOPNOTSUPP` until item 31. Dialogs cannot be minimised (`EINVAL`). |
| `SET_POINTER` | `0x40087707` (W) | 8 | 0: window, 4: shape | 0 arrow, 1 I-beam, 2-5 the resize shapes; recorded now, shown from item 31. |
| `COMMIT` | `0x40887708` (W) | 136 | 0: window, 4: count (at most 8), 8: rects, each 16 bytes: x, y, width, height in content coordinates | Shows the window on its first commit; copies the rects, clipped to the content, or all of it when count is 0 (or after `SET_SIZE`). |
| `DESTROY` | `0x40047709` (W) | 4 | 0: window | Unshows and frees it (its buffer once no mapping holds it), and its dialogs. |
| `GET_WORK_AREA` | `0x8014770a` (R) | 20 | 0: x, 4: y, 8: width, 12: height, 16: scale | Where windows go (the screen below the top bar) and the scale, 1 in 0.3.0. |

Errors: a window that is not the client's, an unknown role, state or shape, unknown flags, a
title too long or not UTF-8, more than 8 rects: `EINVAL`; a role the client may not make:
`EPERM`; 16 windows for the client or 64 in all (the terminal's included): `ENOSPC`; no memory
for a buffer (at most 4 MiB in 0.3.0): `ENOMEM`; an argument outside the program's memory:
`EFAULT`; any other request: `ENOTTY`.

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
| `FocusIn` / `FocusOut` | 7 / 8 | | | | | focus changes |
| `CloseRequest` | 9 | | | | | the close control was clicked; the server closes nothing itself |
| `Resize` | 10 | width | height | | | the server changed the size (from item 31) |
| `StateChanged` | 11 | state | | | | after a minimise (1) or restore (0) |
| `Scale` | 12 | scale | | | | reserved |
| `Overflow` | 13 | records dropped | | | | the queue overflowed |

Each client's queue holds 256 records. When it is full, a `PointerMove` replaces the newest
record if that is a `PointerMove` of the same window; otherwise the oldest record is dropped,
and the next record read is `Overflow` with the count. A record whose copy to the program fails
(`EFAULT`) is lost, as on Linux's input devices.

## What the user does with windows

Click anywhere on a window to raise and focus it (a click on its parent focuses an open
dialog instead); drag the title bar to move it (it stays inside the work area); the yellow
control minimises it to a chip in the top bar, which restores it; the red control, shown on
the focused window, sends `CloseRequest`. The terminal is one of the windows, works as before,
and declines closing. Keys go to the focused window: the terminal's to the shell, a program's
as `Key` events.

## Not yet

Item 31: resizing from the edges, maximise and restore, snap, the switcher (Alt+Tab), Alt+F4
and Cmd+W, the window menu, drawn pointer shapes, keyboard operation, windows partly off
screen, the `wm` command and its MCP tools, and the 2 s rule for a program that does not answer
a close request. Later: key repeat as `Key` records with value 2, scales other than 1, buffers
over 4 MiB, the server in user space.
