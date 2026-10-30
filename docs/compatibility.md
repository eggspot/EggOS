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
| [AutoHotkey](https://www.autohotkey.com/), a scripting language for automating Windows | 2.0.31 (the release's `AutoHotkey_2.0.31.zip`, its `AutoHotkey64.exe`) | GPL-2.0 | Windows, Win32 in C++ on comctl32's common controls, GDI+ and COM (static C runtime) | x86_64 (AutoHotkey publishes x64 and 32-bit x86 builds only) | installs from its `.zip` with the script it runs, opens from its icon or `app open`, runs our script's window: a toolbar, tabs, a list view, a tree view, a progress bar, a picture through GDI+, a status bar, rows from a `Scripting.Dictionary` through COM; keys and clicks drive them, agents read them with `wm show`, Alt+F4 closes it | `autohotkey` (D178) |
| [CocoaGame](https://github.com/reallukee/cocoagame), a small Objective-C board game | 1.0.0 (the author's release `CocoaGame.zip`) | MIT | macOS, Objective-C on AppKit (built in code, no nib), linking the Cocoa umbrella | aarch64, x86_64 (a universal binary) | installs from its `.zip`, opens from its icon or `app open` in its own window, moves its player with `w`, `a`, `s`, `d` and the arrow keys, eats the food, shows its About panel from its app menu, answers agents with its views as an element tree, quits with Cmd+Q | `cocoa-app` (D175) |
| [Mousepad](https://docs.xfce.org/apps/mousepad/start), Xfce's text editor | 0.7.0 (Alpine `v3.24`'s `mousepad-0.7.0-r0`), on GTK 3.24.52 and GtkSourceView 4.8.4 | GPL-2.0-or-later (Mousepad; GTK and its libraries LGPL and others; DejaVu fonts free) | Linux, GTK 3 on Wayland, musl | x86_64, aarch64 | installs from a `.zip` with its loader, libraries, schemas and fonts and its environment, opens from its icon or `app open` in its own window with EggOS's title bar, edits text from the keyboard, copies and pastes inside the app (Ctrl+A, Ctrl+C, Ctrl+V), saves and quits | `gtk-app` (D176) |
| [CorePad](https://gitlab.com/cubocore/coreapps/corepad), the C Suite's text editor | 5.0.0 (Alpine `v3.24`'s `corepad-5.0.0-r0`), on Qt 6.11.1 | GPL-3.0-or-later (CorePad and `libcprime`; Qt LGPL-3.0; ICU, DejaVu fonts and the other libraries free) | Linux, Qt 6 Widgets on Wayland, musl | x86_64, aarch64 | installs from a `.zip` with its loader, libraries, Qt plugins, ICU data, fonts and its environment, opens from its icon or `app open` in its own window with EggOS's title bar, edits text from the keyboard, copies and pastes inside the app (Ctrl+A, Ctrl+C, Ctrl+V), saves as a new file through Qt's file dialog (a second window) and quits | `qt-app` (D179) |

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

## AutoHotkey (Windows, common controls, GDI+ and COM)

- **Where it comes from.** `cargo xtask autohotkey` downloads AutoHotkey's release archive
  <https://github.com/AutoHotkey/AutoHotkey/releases/download/v2.0.31/AutoHotkey_2.0.31.zip>,
  checked against the SHA-256 pinned in `xtask/src/autohotkey.rs` (a release asset can be
  replaced, so a new one is pinned again by hand). It is test data: never shipped in an image,
  never committed; only the binary is used.
- **Why it.** Research chose it as the second Windows app: a real, widely used program written
  in C++ against the Win32 API whose own `Gui` functions create comctl32's list views, tree
  views, tab controls, status bars and progress bars (and any other class as a custom control,
  so the toolbar too), which loads pictures through GDI+ and creates COM objects, and which
  builds its whole window from a script, so the window is the same at every start and its frames
  can be compared. It ships no ARM64 build.
- **Installing it.** `app install --name autohotkey --entry AutoHotkey64.exe --arguments
  controls.ahk /media/host/AutoHotkey_2.0.31.zip` (or the MCP tool `app_install`, or Files)
  installs the archive as the app `autohotkey`, opening `controls.ahk` from the folder it is
  opened in. Any other script works the same way.
- **Opening it.** Its icon, `app open autohotkey` or the MCP tool `app_open` runs it through the
  Windows layer. Our script, `tests/autohotkey/controls.ahk`, builds the window of Claude
  Fable's design (docs/ux/windows-controls-v0.3.md section 30): the toolbar's `Step` adds 20 to
  the progress bar, `Reset` empties it and `Show tree` turns to the tree; arrows and clicks move
  through the list and the tree, and the status bar says what is selected. Agents read the window
  as an element tree with `wm show` (rows, tree items with `expanded`, tabs, status parts, the
  progress bar's per mille, toolbar buttons) and act on them with `wm act`.
- **What CI does** (`autohotkey`, x86_64): installs the archive on a blank disk, lists it, opens
  it in `/tmp` beside the script and its picture, finds the window where the screen changed, and
  checks the design's pixels after each step: the first frame, Down twice in the list, a click
  on `Step`, `wm show`'s rows, progress and tabs, a click on the `Tree` tab, a click on `Fruit`'s
  box and Down; Alt+F4 closes it, and `wm list` has no such window afterwards.
- **Known limits.** x86_64 only: AutoHotkey ships no ARM64 build, and x64 programs do not run on
  Arm yet (part 39). Its hot keys work only while one of its windows has the keyboard (no global
  hooks), there is no tray icon (scripts use `#NoTrayIcon`), the registry is empty and
  read-only, `Download` has no HTTPS, `Run` starts programs but not documents, and every font is
  Egg Mono. What the layer still lacks is in D178's "Not yet".

## CocoaGame (macOS, Objective-C on AppKit)

- **Where it comes from.** `cargo xtask cocoa-app` downloads the author's release archive
  <https://github.com/reallukee/cocoagame/releases/download/v1.0.0/CocoaGame.zip>, checked
  against the SHA-256 pinned in `xtask/src/cocoa_app.rs` (a release asset can be replaced, so a
  new one is pinned again by hand). It is test data: never shipped in an image, never committed.
- **Why it.** Research chose it as the first real Mac app: an MIT-licensed Objective-C app
  published as a `.zip` (not through the App Store), a universal binary (arm64 and x86_64)
  built for a current macOS with chained fixups, its window, view and menu built in code with
  no nib, linking only the Cocoa umbrella with 25 imports, and no updater, helper or
  permission prompt. It seeds its random numbers with the time, so the check reads where the
  player and the food are from the frame instead of expecting fixed cells.
- **Installing it.** `app install /media/host/CocoaGame.zip` (or the MCP tool `app_install`, or
  Files) extracts `CocoaGame.app` and installs it as the app `CocoaGame`, with its own icon made
  from `CocoaGame.icns`.
- **Opening it.** Its icon, `app open CocoaGame` or the MCP tool `app_open` runs
  `CocoaGame.app/Contents/MacOS/CocoaGame` through the macOS layer. `w`, `a`, `s`, `d` and the
  arrow keys move the navy player (wrapping at the edges); landing on the red food moves the
  food elsewhere. Its app menu (in the macOS profile's top bar, or `wm menu`) has `About
  CocoaGame`, which shows the standard About panel, and `Quit CocoaGame` (Cmd+Q). Agents read its
  window as an element tree (`wm show`: the window and its `CanvasView` as a focused graphic)
  and the About panel's too.
- **What CI does** (`cocoa-app`, aarch64 and x86_64): installs it on a blank disk, lists it,
  opens it from `/tmp`, finds its window where the screen changed and the board by its colours,
  reads the player's and the food's cells, and compares every frame with the board the host
  renders: the start, 21 presses of `d` (back where it started), `d`, `w` or `s`, an arrow key,
  and steering onto the food (which must reappear off the player's row and column); `wm show`
  lists the window and the game's view; `wm menu --choose` opens the About panel, found on the
  screen exactly as the host renders it with the icon the installer makes, and `wm show` lists
  its elements; Esc closes it, Cmd+Q quits the game, and `wm list` shows none of its windows.
- **Known limits.** The game has no sound and no saved state, so nothing else is exercised. The
  layer's AppKit is the core of AppKit (`docs/macos-programs.md`): apps with nibs, table and
  scroll views or images, like the runner-up UDCalc, do not run yet (D175's "Not yet").

## Mousepad (Linux, GTK 3)

- **Where it comes from.** `cargo xtask gtk-app` downloads Alpine Linux `v3.24`'s package
  `mousepad` and the 56 packages its program and libraries need (GTK 3, GLib, Pango, cairo,
  HarfBuzz, fontconfig, FreeType, GtkSourceView 4, the accessibility and X11 libraries GTK is
  linked against, Wayland's client libraries, `libxkbcommon`, musl and DejaVu's fonts), each
  checked against a SHA-256 pinned in `xtask/src/gtk_app.rs` (Alpine's mirror keeps only a
  package's latest build, so a rebuilt package is pinned again by hand). It is test data:
  never shipped in an image, never committed. The pieces go into `mousepad.zip`: `mousepad`,
  `libexec/glib-compile-schemas`, `lib/` (musl's loader and 64 libraries, about 30 MB),
  `share/glib-2.0/schemas` (Mousepad's and GTK's GSettings schemas), `share/gtksourceview-4`
  (its languages and colour styles) and `share/fonts` (four DejaVu fonts).
- **Why it.** Research chose it as the first GTK app: a real, maintained, freely licensed
  editor whose interface is built into its program (no files at fixed paths but schemas and
  data that `XDG_DATA_DIRS` moves), that runs on GTK 3, which draws with cairo into `wl_shm`
  buffers (GTK 4 links Vulkan and GStreamer and wants a GPU first), that needs no D-Bus with
  `--disable-server`, and whose main feature, editing a file, can be driven by keys and
  checked by reading the file back. Runner-up: Geany 2.1 (no schemas, but its interface file
  sits at a fixed `/usr/share/geany`).
- **Installing it.** `app install --entry mousepad --environment
  'XDG_DATA_DIRS=/Applications/mousepad/share NO_AT_BRIDGE=1' --arguments '--disable-server
  /tmp/note.txt' /media/host/mousepad.zip` (or the MCP tool `app_install` with `entry`,
  `environment` and `arguments`): the archive holds two programs, so `--entry` names the
  editor; the installer finds its loader and libraries as for Brogue, writes the
  `environment=` and `arguments=` lines, and links the fonts it carries
  (`/Applications/mousepad/share/fonts`) as `/Applications/.fonts/mousepad`, which
  `/etc/fonts/fonts.conf` lists, so fontconfig finds them. GSettings reads compiled schemas,
  which the archive carries as XML: compile them once with the tool it carries,
  `/Applications/mousepad/lib/ld-musl-<arch>.so.1 --library-path /Applications/mousepad/lib
  /Applications/mousepad/libexec/glib-compile-schemas
  /Applications/mousepad/share/glib-2.0/schemas`, as a packager would.
- **Opening it.** Its icon, `app open mousepad` or the MCP tool `app_open` types
  `XDG_DATA_DIRS=... NO_AT_BRIDGE=1 /Applications/mousepad/lib/ld-musl-<arch>.so.1
  --library-path /Applications/mousepad/lib /Applications/mousepad/mousepad --disable-server
  /tmp/note.txt` in the terminal. GTK connects to `eggway`, asks for KDE's server-side
  decorations (so it draws no title bar of its own and the window wears EggOS's), draws with
  cairo into `wl_shm` buffers, its menus as popups, and copies and pastes through
  `wl_data_device_manager` and the primary selection; its text view tells `eggway` about the
  focused text field through `zwp_text_input_v3`, which agents see in `wm show`.
- **What CI does** (`gtk-app`, x86_64 and aarch64): installs it on a blank disk, lists it,
  compiles its schemas inside EggOS with its own `glib-compile-schemas`, writes `alpha` into
  `/tmp/note.txt`, opens it from `/tmp`, finds its window where the screen changed, clicks its
  title bar, presses Ctrl+End and types `beta` (the window must change), presses Ctrl+A,
  Ctrl+C, Ctrl+End and Ctrl+V (it must change again), saves with Ctrl+S and quits with Ctrl+Q,
  then wants the note to hold `alpha` and `beta` twice with the copy pasted right after the
  first `beta`, `mousepad` in `/Applications/.fonts`, no Mousepad window in `wm list`, and the
  manifest's loader, libraries, arguments and environment on the disk after the power-off.
- **Known limits.** Icons and images are blank: Alpine's gdk-pixbuf 2.44 loads PNG and JPEG
  only through glycin, which starts sandboxed loader programs EggOS does not ship; there is no
  D-Bus, so no accessibility bus, no single instance across windows and no portals; files are
  not watched for changes made by other programs (`inotify` is not there, so GLib polls);
  saving keeps the file's owner only when it is the user's (`chown` is not there yet); drag and
  drop is refused; text input methods have nothing to talk to yet. What EggOS still lacks for
  it is in D176's "Not yet".

## CorePad (Linux, Qt 6)

- **Where it comes from.** `cargo xtask qt-app` downloads Alpine Linux `v3.24`'s package
  `corepad` and the 51 packages its program, its libraries and the Qt plugins it carries need
  on either architecture (Qt 6.11.1's Core, Gui, Widgets, Network, DBus, Xml and Wayland
  client, the C Suite's `libcprime`, ICU 78 with its English data, GLib, HarfBuzz, fontconfig,
  FreeType, the network libraries Qt's network module is linked against, the X11 libraries
  x86_64's Qt is linked against, Wayland's client libraries, `libxkbcommon`, musl and DejaVu's
  fonts), each checked against a SHA-256 pinned in `xtask/src/qt_app.rs` (Alpine's mirror
  keeps only a package's latest build, so a rebuilt package is pinned again by hand). It is
  test data: never shipped in an image, never committed. The pieces go into `corepad.zip`:
  `corepad`, `lib/` (musl's loader, 54 libraries on aarch64 and 60 on x86_64, about 57 MB, and
  EggOS's own `libEGL.so.1`, `libGL.so.1` and `libGLESv2.so.2`), `plugins/` (Qt's Wayland
  platform, its `xdg-shell` integration, the compose input context and the client-side title
  bar Qt falls back to), `share/icu` (ICU's data, 3 MB) and `share/fonts` (four DejaVu fonts).
- **Why it.** Research chose it as the first Qt app: a real, maintained, freely licensed Qt
  Widgets editor whose interface and syntax colours are built into its program, that draws with
  Qt's raster engine into `wl_shm` buffers, needs no KDE Frameworks, WebEngine or QML, starts
  with no session restore, single instance or first-run dialog, and whose main feature,
  editing a file, can be driven by keys and checked by reading the saved file back. FeatherPad
  and SpeedCrunch, the first candidates, are not in Alpine `v3.24`; runner-up: qalculate-qt
  5.11 (63 libraries and nothing to read back but the screen).
- **EGL and GL without a GPU.** Alpine's `libQt6Gui` is linked against Mesa's `libEGL.so.1`
  and `libGL.so.1` (x86_64) or `libGLESv2.so.2` (aarch64), which bring Mesa's Gallium driver
  and LLVM (about 235 MB) that EggOS cannot run, and musl's loader refuses a program whose
  libraries name a function nothing defines. So the archive carries EggOS's own three
  libraries, written by `xtask/src/gl_libraries.rs` as small ELF shared objects: every EGL,
  GL and GLX entry point Qt names (and the rest of EGL 1.5's) answers that there is no display,
  configuration, context or surface, as a system without a GPU driver does. Qt Widgets never
  asks for a context, so it draws with its raster engine.
- **Installing it.** `app install --entry corepad --environment 'QT_QPA_PLATFORM=wayland
  QT_PLUGIN_PATH=/Applications/corepad/plugins ICU_DATA=/Applications/corepad/share/icu'
  --arguments '/tmp/note.txt' /media/host/corepad.zip` (or the MCP tool `app_install` with
  `entry`, `environment` and `arguments`): the installer finds its loader and libraries as for
  Mousepad, writes the `environment=` and `arguments=` lines and links the fonts it carries as
  `/Applications/.fonts/corepad`. The environment names Qt's Wayland platform and where Qt
  finds its plugins and ICU its data (the app's own folders); with the loader, the libraries'
  folder, the program and the note, the launch line takes 241 of its 256 bytes. Qt warns that
  musl's `C` locale is not UTF-8 and switches to `C.UTF-8` itself.
- **Opening it.** Its icon, `app open corepad` or the MCP tool `app_open`. Qt connects to
  `eggway`, binds `wl_output` 4 (the output's name), `xdg_wm_base` 5 (each toplevel's bounds
  and the window manager's capabilities: maximise and minimise), `wp_fractional_scale_v1`
  (a preferred scale of 1), `wp_viewporter`, `zxdg_decoration_manager_v1` (so EggOS draws the
  title bar), `wl_data_device_manager` and the primary selection (copy and paste), and
  `zwp_text_input_v3` (the focused field in `wm show`); its file dialog is a second toplevel,
  its completer and menus popups. Qt's file dialog watches the folder it shows through
  `inotify`.
- **What CI does** (`qt-app`, x86_64 and aarch64): installs it on a blank disk with 2 GiB,
  lists it, writes `alpha` into `/tmp/note.txt`, opens it from `/tmp`, finds its window where
  the screen changed, clicks its title bar, presses Ctrl+End and types `beta` (the window must
  change), presses Ctrl+A, Ctrl+C, Ctrl+End and Ctrl+V (it must change again), presses Ctrl+S,
  which opens CorePad's Save As dialog (the screen must change), selects the name with Ctrl+A,
  types `saved.txt` and presses Return, then quits with Ctrl+Q; it wants `/tmp/saved.txt` to
  hold `alpha` and `beta` twice with the copy right after the first `beta`, the note to hold
  `alpha` alone, `corepad` in `/Applications/.fonts`, no CorePad window in `wm list`, and the
  manifest's loader, libraries, arguments and environment on the disk after the power-off.
- **Known limits.** CorePad saves only through its Save As dialog from the keyboard (Ctrl+S is
  wired to it; a plain save is a toolbar button); it picks its desktop layout from the screen's
  physical size, which `eggway` does not report (Qt then assumes 100 dots per inch, so screens
  narrower than about 1100 pixels get its tablet layout); there is no D-Bus, so its
  notifications go nowhere and Qt's accessibility bridge stays off; icons are blank (no icon
  theme is carried); no OpenGL, so Qt Quick and OpenGL widgets do not run. What EggOS still
  lacks for Qt is in D179's "Not yet".

## Chosen, not running yet: Minesweeper (macOS, Swift on AppKit)

- **What it is.** [Minesweeper](https://github.com/codemaki/Minesweeper) by codemaki, a Swift
  port of the classic game, MIT. Version 1.0.0, the release asset
  <https://github.com/codemaki/Minesweeper/releases/download/v1.0.0/Minesweeper-1.0.0.zip>
  (170 998 bytes, SHA-256 `53bbd3cc0f3158adf70f5a843d995438f9f1b2c62aaf0f57fdd09d16fa041858`;
  1.0.1 ships only as a `.dmg`), holding `Minesweeper.app` with
  `Contents/MacOS/Minesweeper` (a universal binary, arm64 and x86_64, for macOS 13) and
  `Contents/Resources/AppIcon.icns`, plus AppleDouble `._*` members.
- **Why it.** Research chose it as the first real Swift app (D180): Swift on AppKit with its
  window, menu and board built in code (no nib, storyboard or SwiftUI), no updater or other
  framework, the system's Swift runtime rather than a copy in the app, and the smallest list of
  imports among the candidates (112 from `libswiftCore`, a few from the Dispatch,
  CoreGraphics, ObjectiveC and CoreFoundation overlays, and Foundation's Swift overlay).
  Runner-up: Mine2000 1.0.1 (arm64 only; its bitmaps are not under its code's licence).
- **Where it stands.** The Swift runtime runs Swift console programs (D180), with the witness
  tables of a program's own conformances, dynamic casts, weak references, `String`'s characters
  and the runtime parts of the overlays the app links; Foundation's Swift overlay, the Dispatch
  and CoreGraphics overlays, Swift subclasses of NSView and the CoreGraphics drawing it uses
  come next, with
  `cargo xtask swift-app` and CI's `swift-app` job, which will install it, open a cell, flag one
  and start a new game. D180's "Not yet" lists what remains.
