# EggOS v0.3.0 app icons: design brief (item 45)

Design by Claude Fable (D16) for roadmap item 45 (decision D148): an installed foreign app
shows its own icon, the one its vendor drew, wherever the shell shows an app, instead of the
generic letter tile of `desktop-v0.3-gui.md` section 7. The installer decodes the vendor's
icon once, at install time, and writes it pre-scaled into the app's folder at the two sizes
the shell draws; the shell blends those pixels and never decodes anything. Numbers are at 1x
and double at 2x. Everything in `desktop-v0.3-gui.md` stands unless this brief says otherwise.

## 1. Where an own icon shows

The own icon replaces the generic tile in every place that calls the icon function with a
catalogue app, at the size that place already uses, at the same position, with nothing else
moved:

- 48x48 (`IconSize::Large`): the dock (pinned apps are built-ins, so only the running and
  minimised entries of installed apps), the desktop icons, and the installer's window:
  the prompt (once the program, `.app` or archive has been read), the progress and result
  screens, and the uninstall confirmation (`Uninstall <name>?`).
- 32x32 (`IconSize::Small`): the taskbar's items, the launcher's rows and the switcher's rows.
- Not in 0.3.0: the window title bar (section 3.3: it carries no icon, and stays that way;
  the taskbar, dock and switcher already identify the window), the Linux dash (it holds the
  pinned built-ins only), the Files app (its rows use the 12x10 type mark and the 6x6 source
  square of section 8, and no 16 px icon is made), the installer's chooser and its failure
  message (the `?` tile and the badgeless tile, unchanged), and the launcher's `Minimised:`
  chips (text).

A window is matched to its app as today (the catalogue entry it was launched from, or its
title); a window the catalogue cannot name keeps the badgeless letter tile of section 6.4.1.

## 2. No badge on an own icon

An own icon carries no source badge, at 48 or at 32. The icon is the mark the user already
knows from Windows or macOS, drawn as the vendor drew it; a 16x20 `W` over its corner would be
the tint we never apply (section 6). The source stays readable where it is text: the launcher
row's `Windows` / `macOS` / `Linux` at the right, the prompt's kind line, `app list`. The
generic tile keeps its badge: it has nothing else to say about the app.

## 3. The icon file and how the shell draws it

The installer writes `/Applications/<name>/app.icon` and the manifest line `icon=app.icon`
(the `icon` key stops being reserved: a path relative to the app's folder, with the rules of
`run`: at most 192 bytes, no `..`, no `'`; a hand-written manifest may name its own file).
Without the key, or when the file cannot be read, the app draws the generic tile.

`app.icon` is raw pixels, exactly 13 312 bytes, no header: the 48x48 image (9 216 bytes), then
the 32x32 image (4 096 bytes); each row-major, top row first, one little-endian `u32` per pixel
as `0xAARRGGBB`, premultiplied alpha (`R`, `G`, `B` already scaled by `A`), so a fully
transparent pixel is `0x00000000`. A file of any other length is not an icon (generic tile).

The shell (the window server's catalogue) reads the file when it reads the catalogue (desktop
start, launcher open, `APPS_CHANGED`), into one static pool of 32 slots of 13 312 bytes (the
catalogue's `MAX_INSTALLED`), 416 KiB in `.bss`, one slot per installed app, cleared when the
app leaves the catalogue. On load every pixel is sanitised once: each colour channel is clamped
to at most the pixel's alpha (`min(c, a)`), so a hostile file can never overflow the blend.
No allocation, no decoding, no scaling in the kernel.

Drawing an own icon is one loop over its 48x48 or 32x32 pixels, clipped to the canvas like
every icon: for each channel, `out = s + (d * (255 - a) + 127) / 255`, `s` the icon's
premultiplied channel, `d` the pixel already on the canvas (wallpaper, the dock tile, a
selection or hover fill), `a` the icon's alpha. `a = 255` writes `s`; `a = 0` leaves `d`.
The canvas's top byte is written as 0 (the framebuffer is XRGB). No threshold: a 50 % edge
blends to 50 %. The generic tile is drawn by the shape table as
before; the only change in `icons.rs` is a new `Icon::Own(slot)` variant drawn by this loop,
with the slot's pixels borrowed from the pool.

## 4. The installer: choosing the source image and scaling it

The installer finds the icon while it plans the install (so the prompt already shows it), and
writes `app.icon` after the copy and before the manifest. Every step here is "try, else
next"; a source whose icon cannot be read installs exactly as before, with no icon file, no
`icon=` line, and nothing said in the prompt, the result or the CLI's output. The source,
in order, for each kind of program:

- A Windows `.exe`: the first `RT_GROUP_ICON` resource in the resource directory's own order
  (named entries, then numbered, as the file stores them), its first language; its `RT_ICON`
  entries are the candidates. Then `<stem>.png` beside the program.
- A macOS `.app`: `Contents/Resources/<CFBundleIconFile>.icns` from an XML `Info.plist`
  (`.icns` appended when the value lacks it); a binary plist is not parsed, so then
  `Contents/Resources/AppIcon.icns`, then `<bundle name>.icns`, then the only `.icns` in
  `Contents/Resources`. The `.icns` entries are the candidates (asset catalogues,
  `Assets.car`, are not read in 0.3.0). Then `<stem>.png` beside the `.app`. A bare Mach-O
  tool: `<stem>.png` beside it.
- A Linux program: `<stem>.png` beside it.

`<stem>` is the program's file name without its extension (`win-paint.exe` gives
`win-paint.png`); the PNG is one candidate of its own size, square only.

**Candidates** are square images with a size and a quality rank: PNG and 32-bit entries rank
first, `ARGB` (`ic04`, `ic05`) second, 24-bit with an 8-bit mask (`il32` + `l8mk` and
kin) third, palettised and 1-bit-mask entries last. For each target `T` (48, then 32) pick,
in this order: the exact `T x T` entry of the best rank; else the smallest entry larger than
`T` (best rank at that size); else the largest entry smaller than `T`. An entry that fails to
decode is skipped and the next in the same order is tried. Both targets are chosen from the
same candidate list independently (a `.icns` with `ic11` 32 and `ic07` 128 gives 32 from
`ic11` and 48 from `ic07`; an `.exe` with 16, 32 and 256 gives 32 exact and 48 from 256).

**Decoding** to straight RGBA, then premultiplying with `c = (c * a + 127) / 255`:

- BMP entries (`BITMAPINFOHEADER`, height doubled, rows bottom-up, 4-byte padded): 32 bpp uses
  its alpha byte, except that an entry whose alpha is 0 on every pixel is opaque and takes its
  transparency from the AND mask (the XP-era icons); 24, 8, 4 and 1 bpp take alpha from the
  AND mask alone. A mask bit of 1 is fully transparent (`0x00000000`), 0 is opaque.
- PNG (an `.exe`'s 256 entries, `.icns` `ic07` to `ic14`, the file beside the program): colour
  types 0, 2, 3, 4 and 6 at bit depth 8; 16-bit takes the high byte; `tRNS` applies to 0, 2 and
  3; `Adam7` interlacing and JPEG 2000 entries (`jP` or `ff 4f ff 51` signature) are "fails
  to decode" (next candidate). Inflate is our own, as everything is.
- `.icns` legacy types: `is32`/`il32`/`ih32`/`it32` are R, G, B planes, each raw or run-length
  (a byte `b >= 0x80` repeats the next byte `b - 125` times, else `b + 1` literal bytes;
  `it32` starts with 4 zero bytes); their mask `s8mk`/`l8mk`/`h8mk`/`t8mk` is the alpha,
  missing mask means opaque. `ic04`/`ic05` are `ARGB` + run-length planes A, R, G, B.
  `TOC `, `icnV`, `name`, `info` and unknown types are skipped.

**Scaling**, on premultiplied pixels, integers only:

- Larger than `T` (side `S`): area average. Destination pixel `i` covers the source interval
  `[i * S / D, (i + 1) * S / D)`; in units of `1 / D` of a source pixel the overlap of source
  pixel `j` with it is an integer `w`, and the weights of one destination pixel sum to `S`
  per axis, `S * S` in all. Each channel is `(sum(src * wx * wy) + S * S / 2) / (S * S)`
  (`S = 1024` keeps the sum under `2^28`). An integer ratio (64 to 32, 256 to 32) reduces to a
  plain mean of `k x k` blocks.
- Smaller than `T` (side `S`): nearest neighbour by the largest integer factor
  `k = T / S` (16 to 48: 3; 16 to 32: 2; 32 to 48 and 24 to 32: 1), the result centred at
  offset `(T - k * S) / 2`, the rest transparent. Pixel art stays crisp; a 32 inside 48 sits
  with 8 px of clear margin rather than blurred to 1.5x.
- Exact: copied.
- Never any added padding, trimming or shadow: a macOS icon keeps the margin Apple's canvas
  gives it and looks as it does in the macOS dock; a Windows icon fills its square as in the
  Windows taskbar.

The worst case the installer decodes is a lone 1024 PNG (4 MiB of pixels, once); the prompt
appears after it, under the 2 s install budget of section 9.

## 5. Archives: installing from a `.zip`

`app install <path>.zip` (and Files' `Install...` on a `.zip`, the chooser on the host share)
reads the archive's central directory, stored and deflate entries only; anything else is
`cannot read it: not a zip archive EggOS can open`. The programs in it are the entries that
the install rules already accept (`.exe` by its bytes, an `X.app/Contents/MacOS/X` bundle, a
static ELF), excluding anything inside another `.app`'s `Contents`. The whole archive is
extracted into `/Applications/<name>/` (the program's DLLs and files beside it come along),
`run` is the chosen entry's path inside the folder, and the manifest is written last as
always; the icon rules of section 4 then apply to the extracted program.

- One program: the prompt as today, with the archive on `From:` and the entry on a new line
  under it: `From: /media/host/Paint-1.2.zip`, `Inside: Paint.app` (or `Inside:
  bin/win-paint.exe`), then `To:` and `Adds:` (six lines, within the dialog's eight). The
  app's name is the entry's stem (`Paint`, `win-paint`), or `--name`.
- Several programs: the chooser of section 9 as for a folder, headed `Install a program from
  /media/host/tools.zip`, one button per entry path, four at most, `and N more` as today;
  choosing one shows its prompt. From the CLI: `app: could not install tools.zip: it holds
  several programs (bin/a.exe, bin/b.exe): choose one with --entry <path>`, status 1, and
  `app install --entry bin/a.exe -- tools.zip` installs that one.
- None: the chooser's `It holds no program EggOS can run.`; from the CLI `app: could not
  install notes.zip: it holds no program EggOS can run`.

## 6. States and 2x

An own icon is treated exactly like the generic tile: the same backgrounds behind it (dock
hover `bar.hover` and pressed `bar.pressed` r6 52x52 tiles, taskbar `bar.hover` / `bar.pressed`,
the desktop cell's `selection` fill and focus ring, the launcher's and switcher's selected
rows), the same running mark (the 4x4 `text` square under a dock icon), the same minimised
treatment (no taskbar underline, a `text.dim` title in the switcher). No tint, no dimming, no
outline, no drop shadow, ever: the icon's own alpha is what blends it onto each background.
At scale 2 the 48 and 32 images are pixel-doubled with the rest of the icon set (section 7);
the file holds no 96 or 64 image in 0.3.0, and when 2x gets native icons they will be a second
file beside this one, not a change to it.

## 7. CLI and MCP (D22)

- `app list` gains a fifth tab-separated column: the icon file's path inside the folder
  (`app.icon`) or `-` for an app drawn as the generic tile.
- `app icon [--size 48|32] [--] <name> <file.png>` writes the app's icon as a PNG (RGBA 8-bit,
  straight alpha, un-premultiplied as `c = (c * 255 + a / 2) / a`, 0 where `a = 0`; 48 by
  default), using the encoder `wm capture` already has. It prints `Wrote <file>.`; `app:
  <name> has no icon` (status 1) for a generic-tile app or a built-in; `app: <name> is not
  installed` otherwise. Nothing is read but the shell-ready file: no decoding here either.
- MCP: `app_list` entries gain `"icon"`, the icon file's absolute path
  (`/Applications/Paint/app.icon`) or absent, and the tool `app_icon` (`name`; `size`, 48 or
  32) returns the PNG image as `wm_capture` does, no grant needed (it reads what the desktop
  shows). It is `app icon`'s twin.
- The GUI face of the capability is the icon itself in the shell; export is the CLI's and
  MCP's view of the same pixels.

## 8. Host tests that pin the look

In `icons.rs` (kernel side, no allocator in the drawing code):

1. A hand-built 4x4 own icon with the four telling pixels, opaque red `0xffff0000`,
   half green premultiplied `0x80008000`, transparent `0x00000000` and opaque white
   `0xffffffff`, blended over the tests' `BACKGROUND` `0x00ff00ff` and over `dock.bg`,
   asserting the exact `u32` of each from the section 3 formula (half green over
   `0x00ff00ff` is `0x007f807f`; the opaque and transparent ones are `0x00ff0000`,
   `0x00ff00ff` and `0x00ffffff`).
2. Two synthetic icons (a 48 and a 32 filled by a formula: alpha rising left to right, colour
   top to bottom) drawn at both sizes and added to the pinned hash table beside the eight
   icons there.
3. An own icon draws only inside its square and respects a clip that cuts it.
4. A slot filled from a file with a channel above its alpha draws as if the channel equalled
   the alpha; a file of 13 311 or 13 313 bytes leaves the app on the generic tile (same hash
   as today's `windows app`).

In the installer (userland):

5. Candidate choice: `[16, 32, 256]` gives 32 exact and 48 from 256; `[16, 32]` gives 48
   from 32 at `k = 1`, offset 8; `[64]` gives both from 64; `[16]` gives 48 at `k = 3` and 32
   at `k = 2`; ranks: `ic11` beats `ic05` beats `il32` at 32, `ih32` beats `ic07` at 48, a PNG
   256 beats a 32-bit BMP 256.
6. Area average: a 4x4 to 2x2 with known premultiplied means; 6 to 4 with the non-integer
   weights (destination 0 takes source 0 at 4 and source 1 at 2, in units of 1/4).
7. Alpha: a 1-bit mask makes transparent pixels; a 32-bpp entry with all-zero alpha is opaque
   under its mask; a PNG `tRNS` palette entry is transparent; premultiply rounding of
   `(255, 128)` gives 128.
8. Decoding a stored (uncompressed-deflate) 2x2 RGBA PNG; an interlaced PNG and a JPEG 2000
   entry are skipped and the next candidate is taken; a broken icon still yields a plan, and
   the written manifest has no `icon=` line.
9. The written `app.icon` is exactly 13 312 bytes with the 48 image first; `app icon` on it
   decodes back to the same pixels after premultiplying.
10. Zip: a central directory with two `.exe` entries gives the chooser with both paths and the
    CLI's `several programs` message; one gives the prompt with `From:` and `Inside:`; none
    gives `It holds no program EggOS can run.`; an entry inside `X.app/Contents` is not a
    second program; `Info.plist` with `CFBundleIconFile` `AppIcon` and `AppIcon.icns` resolve
    to the same file.

In QEMU (expected frames): `mac-paint.app` gains a small `.icns` (an `ic11` 32 and an `ic07`
128 drawn by the test's builder) and `win-paint.exe` an `RT_GROUP_ICON` with 16, 32 and 48
entries; after `app install` the dock, the desktop, the launcher row and the taskbar frames
are compared pixel for pixel, and `app list` shows `app.icon` for both.

## 9. Left out of 0.3.0

Title-bar icons; 16 px icons for Files; native 2x images; binary plists and asset
catalogues; `.dmg`, `.pkg` and `.ipa` sources (D17's installer work); replacing an app's icon
from Settings; icons for native EggOS apps from their manifest (the three built-ins keep the
drawn set).

## 10. As built (D148)

Built from this brief for item 45; where the build differs, D148 records why:

- The shell keeps an own icon in the catalogue's entry for the app, on the kernel's heap,
  allocated only for an app that has one (13 312 bytes each), instead of a 416 KiB static pool:
  a machine with no own icons pays nothing (CLAUDE.md note 3). The pixels, the sanitising and
  the blend are as section 3 says.
- The installer's dialogs (prompt, progress, result, uninstall) still show the generic tile:
  the icon is made after the copy, from what was installed, so the prompt has none yet. The
  `.zip` prompt has its `Inside:` line.
- `app icon` and the MCP tool `app_icon` (section 7) are built (D196; `app_icon` always
  answers the 48-pixel image); `app list` and `app_list` name the icon file.
- The installer's `Installed` screen and the uninstall confirmation show the own icon (D196);
  binary `Info.plist` files are read (D196).
- The QEMU test installs `mac-paint.zip`, whose bundle's `AppIcon.icns` holds a 128-pixel `ic07`
  and a 32-pixel `icp5` PNG drawn by xtask; `win-paint.exe` gets no icon resources (it is
  linked without a resource compiler), so PE icons are covered by host tests on hand-built
  resource trees.
