# Real apps on EggOS

The Real apps track (roadmap, D169 onwards) checks EggOS against real applications that people
use, not only programs we wrote ourselves. Each app here was chosen by research for being
freely redistributable and a fair test of one kind of app; CI downloads it as test data on
every run (never shipped in an image, never committed), installs it, opens it, drives its main
feature and closes it. This page lists each app, the version tested, where it runs and what
works.

| App | Version | License | Platform | Architectures | What works | CI job |
|-----|---------|---------|----------|---------------|------------|--------|
| [Brogue: Community Edition](https://github.com/tmewett/BrogueCE), a roguelike game | 1.15.1 (Alpine `v3.24`'s `brogue-1.15.1-r0`), on `sdl2-compat` 2.32.68 and SDL `main` at `328edfc1` | AGPL-3.0-or-later (code), CC BY-SA 4.0 (tiles) | Linux, SDL 2 on Wayland, musl | x86_64, aarch64 | installs from a `.zip` with its loader and libraries, opens from its icon or `app open` in its own window, plays turns from the keyboard, saves and quits | `sdl3`, `sdl-app` (D172) |
| [Pegs](https://www.chiark.greenend.org.uk/~sgtatham/puzzles/doc/pegs.html) from Simon Tatham's Portable Puzzle Collection, a peg solitaire game | `20260923.616da16` (the collection's official `pegs.exe`) | MIT | Windows, classic Win32 (ANSI, MSVC's `/MD` C runtime) | x86_64 (the collection publishes x64 Windows builds only) | installs from its `.exe`, opens from its icon or `app open`, plays by dragging pegs, undoes and redoes, changes its board from its `Type` menu (the window resizes, the tick moves), opens its About box and its Custom dialog with a drop-down list, quits | `win32-app` (D174) |

## Brogue CE (Linux, SDL)

- **Where it comes from.** `cargo xtask sdl-app` downloads Alpine Linux `v3.24`'s packages
  `brogue`, `sdl2-compat`, `sdl2_image`, `wayland-libs-client`, `wayland-libs-cursor`,
  `wayland-libs-egl`, `libxkbcommon`, `libffi` and `musl`, each checked against a SHA-256 pinned
  in `xtask/src/sdl_app.rs` (Alpine's mirror keeps only a package's latest build, so a rebuilt
  package is pinned again by hand). No released SDL 3 draws on Wayland without EGL and EggOS
  has no GPU stack yet, so `libSDL3.so.0` is built from SDL's commit `328edfc1`, which added
  the Wayland driver's `wl_shm` framebuffer, with its Wayland driver alone (`cargo xtask
  sdl3`, Docker, `alpine:3.24`). The pieces go into `brogue.zip`: `brogue`, `lib/` (musl's
  loader, SDL 3, `sdl2-compat`, `sdl2_image`, Wayland's libraries, `libxkbcommon`, `libffi`)
  and `share/brogue` (its tiles and key map).
- **Installing it.** `app install --arguments '-n --data-dir /Applications/brogue/share/brogue'
  /media/host/brogue.zip` (or the MCP tool `app_install` with `arguments`): the installer
  finds the one program in the archive, sees from its `PT_INTERP` that it needs
  `ld-musl-<arch>.so.1`, finds that loader in the archive and the folder of its libraries, and
  writes them into the manifest (`loader=lib/ld-musl-<arch>.so.1`, `libraries=lib`,
  `arguments=...`). Brogue looks for its data where Alpine installs it unless told, hence
  `--data-dir`; `-n` starts a new game without the menu.
- **Opening it.** Its icon, `app open brogue` or the MCP tool `app_open` types
  `/Applications/brogue/lib/ld-musl-<arch>.so.1 --library-path /Applications/brogue/lib
  /Applications/brogue/brogue -n --data-dir ...` in the terminal, which runs it until it
  ends. SDL 3 connects to `eggway`, asks for server-side decorations, draws with its software
  renderer into `wl_shm` buffers, sets its pointer through `wp_cursor_shape_manager_v1` and
  gets keys through `wl_keyboard`; the window wears EggOS's own title bar and controls.
- **What CI does** (`sdl-app`, x86_64 and aarch64): installs it on a blank disk, opens it from
  `/tmp`, waits for its window, clicks its title bar, plays nine turns (`z` five times, then
  `l`, `j`, `h`, `k`), checks the window changed, saves with `S` and Enter (Brogue quits after
  saving), and wants the saved game in `/tmp`, no Brogue window in `wm list`, and the
  manifest's loader, library folder and arguments on the disk after the power-off.
- **Known limits.** It saves into the folder it was started from: from the launcher that is
  the terminal's current folder, so start it from a writable one (`/tmp`, `/home`); no sound
  (EggOS has no audio device yet; Brogue starts only SDL's video, and SDL is built without
  audio back ends); the pointer shows the system's
  arrow where SDL asks for a shape EggOS does not draw (a hand, a busy pointer); the window is
  shown opaque at scale 1. What EggOS still lacks for it is in D172's "Not yet".

## Pegs (Windows, classic Win32)

- **Where it comes from.** `cargo xtask win32-app` downloads the collection's official x64
  `pegs.exe` from <https://www.chiark.greenend.org.uk/~sgtatham/puzzles/pegs.exe>, checked
  against the SHA-256 pinned in `xtask/src/win32_app.rs`. The collection publishes each puzzle
  at one address that every new build replaces (the versioned installer,
  `puzzles-20260923.616da16-installer.msi`, holds the same file), so a new build is pinned again
  by hand. It is test data: never shipped in an image, never committed.
- **Why it.** Research chose it as the first classic Win32 app: a real, popular, MIT-licensed
  program written against the plain Win32 API in C, built by MSVC on its `/MD` C runtime,
  with a menu bar built in code (`CreateMenu`, `AppendMenuA`, `CheckMenuItem`,
  `EnableMenuItem`), dialogs it builds itself on `DefDlgProcA` (static text, buttons, check
  boxes, edits and a drop-down list combo box), a board drawn off screen into a bitmap and
  copied with `BitBlt`, and a window it sizes to its board with `SetWindowPos`. Pegs needs no
  status bar (other puzzles of the collection use comctl32's, which the layer has not yet), and
  its Cross board is the same at every start, so its frames can be compared.
- **Installing it.** `app install /media/host/pegs.exe` (or the MCP tool `app_install`, or Files)
  installs the program as the app `pegs`.
- **Opening it.** Its icon, `app open pegs` or the MCP tool `app_open` runs it through the
  Windows layer. It starts a Cross 7x7 board; drag a peg over another into a hole to remove it,
  `u` undoes, `r` redoes, `n` starts a new game, `q` quits. Agents read its window as an element
  tree with `wm show` (its dialogs' controls, the combo box's chosen board type as its value)
  and operate it with `wm click`, `drag`, `type` and `press`.
- **What CI does** (`win32-app`, x86_64): installs it on a blank disk, lists it, opens it from
  `/tmp`, finds its window where the screen changed and its content by its menu bar, checks the
  three pegs of the first jump, drags the peg over the next into the hole and checks the pegs
  again, presses `u` and wants the starting board back pixel for pixel, `r` and wants the
  jumped board back, opens `Type` and wants `Cross 7x7` ticked, chooses `Cross 5x7` and wants
  the window narrower and the tick on `Cross 5x7`, opens `Help > About` and closes it with
  Enter, opens `Type > Custom...`, opens its board type list with Tab, Tab and Alt+Down and
  closes the list and the dialog with Esc, wanting the board unchanged after each dialog,
  quits with `q`, and wants no Pegs window in `wm list` and a clean power-off.
- **Known limits.** x86_64 only: the collection ships no ARM64 Windows build, and x64 programs
  do not run on Arm yet (part 39). Help needs `puzzles.chm` and Windows' HTML Help, which EggOS
  does not have, so the Help menu has only About; printing finds no printer; Copy puts the
  board's text on the program's own clipboard, as there is no clipboard shared between apps yet;
  every font is Egg Mono, so the dialogs are laid out from its metrics. What the layer still
  lacks for other puzzles of the collection (comctl32's status bar, more of the math library) is
  in D174's "Not yet".
