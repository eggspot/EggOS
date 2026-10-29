# Apps on the desktop: the catalogue and `app.manifest`

The desktop shell (D133, `docs/ux/desktop-v0.3-gui.md` section 6) lists the built-in apps
(Terminal, Files, Settings, and in the launcher only the Text Editor, D149) and every installed
app in its launcher, on the dock or taskbar
when they have windows, and as desktop icons. An installed app is a folder under
`/Applications` that holds its program and a manifest. `/Applications` lives on the disk when
one is attached (D131), so installed apps survive a reboot.

## The folder

```
/Applications/<name>/
    app.manifest        the app's entry in the catalogue
    app.icon            its own icon, when the installer found one (D148)
    ...                 its program and the files beside it
```

The folder's name is the app's name. `app install` copies a program here and writes the
manifest last, through a temporary file (`app.manifest.tmp`) renamed over `app.manifest`, so
a half-installed app has no manifest and is not listed; `app uninstall` removes the manifest
first, then the folder and everything in it (D134). A manifest can also be written by hand:

```
mkdir /Applications/Hello
echo 'eggos-app 1' > /Applications/Hello/app.manifest
echo name=Hello >> /Applications/Hello/app.manifest
echo source=linux >> /Applications/Hello/app.manifest
echo run=/bin/uname >> /Applications/Hello/app.manifest
```

Open the launcher (Super, or Cmd+Space in the macOS profile): it reads the catalogue again and
the app appears there and on the desktop.

## Installing and uninstalling: `app`

`app` is the command; the installer's window is the same capability with dialogs (D22, D134).

```
app install [--name <name>] [--entry <path>] [--window] [--] <path>
app uninstall [--window] [--] <name>   (app remove is the same)
app open [--] <name>
app list
```

`--` ends the options: the word after it is the path or name even if it starts with `-`.

- **`app install <path>`** installs a Windows program (a PE32+ `.exe`, x86_64 or ARM64), a
  macOS program (a 64-bit Mach-O or fat file, or an `.app` folder whose `Contents/MacOS` holds
  its program: the one named like the bundle, else its only file) or a static 64-bit Linux
  program (ELF), from anywhere, typically the host share at `/media/host`
  (D130). What a file is comes from its first bytes, not its
  name. The app's name is the file's name without its extension (`win-paint.exe` gives
  `win-paint`, `Paint.app` gives `Paint`), or `--name`; it must follow the manifest's rules
  below. The program, or the `.app` folder whole, is copied into `/Applications/<name>/`, then
  the app's own icon is written (below), then the manifest (`name`, `source`, `run`,
  `architecture`, `icon`) and everything is synced. It prints `Installed <name> in
  /Applications/<name>.`
- **A `.zip`** (D148, told by its first bytes) installs the one program it holds: a `.exe`,
  the program of an `.app` bundle in it (the file in `X.app/Contents/MacOS/` its `Info.plist`'s
  `CFBundleExecutable` names, else `X.app/Contents/MacOS/X`), or a file without an
  extension that is an ELF; nothing inside another bundle's `Contents` counts. The whole
  archive is extracted into `/Applications/<name>/` (macOS's `__MACOSX` metadata left out), so
  the program's files and DLLs come along, and `run` is the program's path inside it; the name
  comes from the program (`bin/win-paint.exe` gives `win-paint`, `Paint.app` gives `Paint`). An
  archive with several programs is refused with their list (`it holds several programs (a.exe,
  b.exe): choose one with --entry <path>`); `--entry <path>` chooses one (a bundle's own path
  names its program too); the installer's window lists them to choose from instead, and the
  MCP tool `app_install` takes the path as `entry`. An archive holding a file named
  `app.manifest`, `app.manifest.tmp` or `app.icon` at its top (the files the installer writes)
  is refused, and so is one whose files would take more than 4 GiB once extracted. Stored and
  deflated entries are read (our own inflate, shared with
  the PNG decoder); ZIP64, encryption and other methods are refused as `not a zip archive EggOS
  can open`, and an entry whose name would leave the folder (absolute, `..`, a backslash or a
  colon) as `it holds a file whose name points outside its folder`. Each entry's CRC-32 is
  checked; a damaged one fails the install and removes what was extracted.
- **The app's own icon** (D148, `docs/ux/app-icons-v0.3.md`): after the copy, the installer
  looks for it in what it installed: a Windows program's first icon group (`RT_GROUP_ICON`
  and its `RT_ICON` images, PNG or bitmap with its mask), a bundle's `.icns`
  (`Contents/Resources/<CFBundleIconFile>` from an XML `Info.plist`, else `AppIcon.icns`, else
  `<bundle name>.icns`, else the only `.icns`; PNG entries and the `is32`/`il32`/`ih32` and
  `ARGB` planes), else `<stem>.png` beside the program or bundle, installed or installed from.
  For each of the two sizes the shell draws (48 and 32 pixels) it takes the exact size of the
  best kind, else the smallest larger (area average), else the largest smaller (grown by a
  whole factor, centred), and writes `app.icon`. An app without a readable icon installs the
  same, with the generic letter tile.
- It refuses, before writing anything: a file that is no program
  (`not a program EggOS can run`), a library, a folder that is not an `.app`, a name already
  installed (`it is already installed`: uninstall it first), a name or program path the
  manifest cannot hold, and what this machine cannot run, as `EggOS cannot run <file> on
  <arch>: <reason>`: a Windows or macOS program where its layer does not run (riscv64), a
  program built for another CPU (until ahead-of-time translation exists, D17), a 32-bit
  program, a Linux program that needs a dynamic loader. A Windows or macOS program whose
  layer is not installed (`/usr/lib/eggos/windows`, `/usr/lib/eggos/macos`) is refused too
  (`it needs the Windows layer, which is not installed`), since nothing could open it (D18).
- A failure while copying (`/Applications is read-only because EggOS started without a
  disk`, `not enough space on the disk (ENOSPC)`, an I/O error) removes what was copied. A
  folder of that name without a valid manifest (an install or uninstall that was cut short)
  is replaced.
- **`app uninstall <name>`** removes the app: its manifest, so it leaves the launcher and the
  desktop at once, then its folder; nothing of it is left (D18). The layer it ran through
  stays. `app: could not uninstall <name>: it is not installed` otherwise.
- **`app open <name>`** opens an app as a double-click on its icon does (`OPEN_APP`, D137):
  the built-in `Terminal`, `Files`, `Settings` or `Text Editor` (D141, D149; a built-in wins
  over an installed app
  of the same name), or an installed app through its layer, a console app in a terminal. It
  prints `Opened <name>.`; `app: <name> is not installed`, or `app: could not open <name>: <error>` without a
  desktop. It is the MCP tool `app_open`'s twin.
- **`app list`** prints one line per installed app, in `/Applications`' order: its name,
  source, architecture, program path and own icon file (`app.icon`, or `-` for the generic
  tile), separated by tabs; the MCP tool `app_list` answers the same.
- Status: 0 done, 1 refused or failed, 2 a usage error. With a desktop running, `install`
  and `uninstall` tell it (`APPS_CHANGED`, [Windows](windows.md)), so its launcher and icons
  follow at once.
- **`--window`** shows the installer's window instead of printing: the prompt (`Install
  <name>?`, the kind, `From:`, `To:`, `Adds:`, with `Cancel` and `Install`), the progress (a
  bar that follows the bytes, `Cancel` until the copy ends), the result (`Installed
  <name>.`, with `Close` and `Open`, which opens the app as its icon does) or the reason it
  cannot, and for uninstall `Uninstall <name>?` with what it removes, `Cancel` and
  `Uninstall`. Given a folder that is not an `.app`, the window lists the programs in it
  (four at most) to choose from; given a `.zip` holding several programs and no `--entry`, it
  lists them by their paths inside it the same way. The window is
  `/usr/libexec/app-window`, another name of the Settings program, so the toolkit is in the
  image once.
- The desktop starts the same window: the launcher's `Install from host share...` row (while
  the share is mounted) opens it on `/media/host`, and `Uninstall...` in an installed app's
  menu (its desktop icon's or its launcher row's) asks to uninstall it.
- Files (D135) starts the same window: a double-click on a Windows or macOS program, an
  `.app` outside `/Applications` or a `.zip` (D148), or `Install...` in their menu, opens the
  prompt (for a `.zip`, an `Inside:` line names the program in it); in the
  `Apps` place (`/Applications`) a double-click on an app opens it as its icon does and Delete
  or `Uninstall...` asks to uninstall it.
- Not yet: the MCP tools `app_install`, `app_uninstall` and `app_list` come with the MCP
  server (D21, after the release); installing a missing layer (the component manager, D18);
  `--force` to replace an installed app (uninstall it first).

## The Text Editor: `edit`

The Text Editor (D149, `docs/ux/text-editor-v0.3.md`) edits text files of up to 4 MiB of UTF-8
in a window of its own: New, Open, Save, Save As and Find in its toolbar, undo and redo, cut,
copy and paste, line numbers, `Ln, Col`, `Modified` and `LF` or `CRLF` in its status bar, both
Ctrl and Cmd in every profile. A file that is not UTF-8 or holds a NUL is refused; a CRLF
file and a byte-order mark are kept as they were when it is saved.

```
edit [--] [<path>]
```

- **`edit <path>`** opens the file in a Text Editor window and returns at once, printing
  `Opened <path> in Text Editor.`; the window runs on its own (a session of its own). A path
  with nothing at it opens as a new file that the first save makes; with no path, an untitled
  document. A folder, a file that is not text or one over 4 MiB is refused with the editor's
  words (`edit: notes.bin is not a text file.`).
- With no desktop (a terminal-only install, or before the desktop starts) `edit` says there is
  no desktop to open the Text Editor on and fails: `cat` shows a file and `tee` writes one.
  There is no terminal editor yet.
- Files opens a text file in the editor (double-click, Enter or `Open`) when it is installed;
  the launcher's `Text Editor` and `app open "Text Editor"` open an untitled one; the MCP tool
  `edit_open` is `edit`'s twin ([mcp.md](mcp.md)).
- The window is `/bin/text-editor [<path>]`, another name of the Settings program, as `edit`
  is.
- Status: 0 opened, 1 refused or failed, 2 a usage error.

## `app.manifest`

A text file of at most 1024 bytes: the line `eggos-app 1`, then one `key=value` line per
entry. Keys are `a`-`z`, `0`-`9`, `.` and `_`; values are printable ASCII. A line that is no
entry is skipped, an unknown key is ignored, and the last line of a key wins. CRLF line ends
are accepted. The crate `eggos-app-manifest` (`app-manifest/`) reads and writes it; the kernel
and the installer share it.

| Key | Required | Value |
|-----|----------|-------|
| `name` | yes | The app's name, which must be its folder's: 1 to 32 bytes, no `/` or `'`, not `.` or `..`, no space at either end. |
| `source` | yes | `windows`, `macos` or `linux`: the system it was built for. The launcher shows `Windows`, `macOS` or `Linux` and the icon wears that badge. |
| `run` | yes | The program: an absolute path, or a path relative to the app's folder; at most 192 bytes, no `..` part, no `'`, not ending in `/`. |
| `architecture` | no | The machine it was built for (`x86_64`, `aarch64`, ...), at most 16 bytes. |
| `synonyms` | no | Words the launcher's search also matches, separated by commas, at most 8 used, at most 128 bytes in all. The program's file name always matches. |
| `icon` | no | The app's own icon file (D148), relative to the folder, with `run`'s rules: [`app.icon`](../app-manifest/src/icon.rs), exactly 13 312 bytes, the 48x48 then the 32x32 image as premultiplied `0xAARRGGBB`, little-endian. The shell draws it, without a badge, wherever the app's icon shows; without the key, or with a file of another size, the app wears the letter tile and its source's badge. |

A manifest that is not this format, is longer than 1024 bytes, lacks a required key, has an
invalid value for one, or names another folder is not an app: the shell skips it. At most 32
apps are listed, sorted by name ignoring case; past 32, the first 32 found in `/Applications`
are kept.

## What launching does

| App | Launch |
|-----|--------|
| Terminal | Focuses the terminal's window (restoring it if it is minimised). |
| Settings, Files | Focuses its window if it has one, else starts `/bin/settings` or `/bin/files` as a process. Without `/bin/files` (D18), Files says `Files is not installed.` |
| Text Editor | Starts `/bin/text-editor` on an untitled document, a new window each time (D149); the dock's icon of running editors focuses them. Without it, `Text Editor is not installed.` |
| Host share | Starts Files on `/media/host` (`/bin/files /media/host`, D135); without Files it focuses the terminal and runs `ls /media/host` in it. |
| An installed app | Focuses the terminal and runs its program there, as if its path had been typed at the prompt (single-quoted when it holds a space or other punctuation), after Ctrl-U, so a half-typed line is erased first. The program runs through its layer as at the prompt: a Windows GUI program or a macOS app (an `.app`, D147) opens its window, a console program writes in the terminal. |
| `Install from host share...` | Starts the installer's window on `/media/host` (above). |

The kernel reads the catalogue when the desktop starts, each time the launcher opens and when
`app` says the apps changed, never otherwise (D18). A command is typed into the terminal as a
keyboard would type it, after Ctrl-U, which erases a half-typed line at the prompt; if a
program is running in the terminal, that program receives it.
