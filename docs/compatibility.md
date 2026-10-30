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
