# Apps on the desktop: the catalogue and `app.manifest`

The desktop shell (D133, `docs/ux/desktop-v0.3-gui.md` section 6) lists the built-in apps
(Terminal, Files, Settings) and every installed app in its launcher, on the dock or taskbar
when they have windows, and as desktop icons. An installed app is a folder under
`/Applications` that holds its program and a manifest. `/Applications` lives on the disk when
one is attached (D131), so installed apps survive a reboot.

## The folder

```
/Applications/<name>/
    app.manifest        the app's entry in the catalogue
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
app install [--name <name>] [--window] <path>
app uninstall [--window] <name>        (app remove is the same)
app list
```

- **`app install <path>`** installs a Windows program (a PE32+ `.exe`, x86_64 or ARM64), a
  macOS program (a 64-bit Mach-O or fat file, or an `.app` folder whose `Contents/MacOS` holds
  its program: the one named like the bundle, else its only file) or a static 64-bit Linux
  program (ELF), from anywhere, typically the host share at `/media/host`
  (D130). What a file is comes from its first bytes, not its
  name. The app's name is the file's name without its extension (`win-paint.exe` gives
  `win-paint`, `Paint.app` gives `Paint`), or `--name`; it must follow the manifest's rules
  below. The program, or the `.app` folder whole, is copied into `/Applications/<name>/`, then
  the manifest is written (`name`, `source`, `run`, `architecture`) and everything synced.
  It prints `Installed <name> in /Applications/<name>.`
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
- **`app list`** prints one line per installed app, in `/Applications`' order: its name,
  source, architecture and program path, separated by tabs.
- Status: 0 done, 1 refused or failed, 2 a usage error. With a desktop running, `install`
  and `uninstall` tell it (`APPS_CHANGED`, [Windows](windows.md)), so its launcher and icons
  follow at once.
- **`--window`** shows the installer's window instead of printing: the prompt (`Install
  <name>?`, the kind, `From:`, `To:`, `Adds:`, with `Cancel` and `Install`), the progress (a
  bar that follows the bytes, `Cancel` until the copy ends), the result (`Installed
  <name>.`, with `Close` and `Open`, which opens the app as its icon does) or the reason it
  cannot, and for uninstall `Uninstall <name>?` with what it removes, `Cancel` and
  `Uninstall`. Given a folder that is not an `.app`, the window lists the programs in it
  (four at most) to choose from. The window is `/usr/libexec/app-window`, another name of
  the Settings program, so the toolkit is in the image once.
- The desktop starts the same window: the launcher's `Install from host share...` row (while
  the share is mounted) opens it on `/media/host`, and `Uninstall...` in an installed app's
  menu (its desktop icon's or its launcher row's) asks to uninstall it.
- Files (D135) starts the same window: a double-click on a Windows or macOS program or an
  `.app` outside `/Applications`, or `Install...` in a program's menu, opens the prompt; in the
  `Apps` place (`/Applications`) a double-click on an app opens it as its icon does and Delete
  or `Uninstall...` asks to uninstall it.
- Not yet: the MCP tools `app_install`, `app_uninstall` and `app_list` come with the MCP
  server (D21, after the release); installing a missing layer (the component manager, D18);
  `--force` to replace an installed app (uninstall it first).

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
| `icon` | no | Reserved: icons are drawn from the name's first letter and the source's badge in 0.3.0. |

A manifest that is not this format, is longer than 1024 bytes, lacks a required key, has an
invalid value for one, or names another folder is not an app: the shell skips it. At most 32
apps are listed, sorted by name ignoring case; past 32, the first 32 found in `/Applications`
are kept.

## What launching does

| App | Launch |
|-----|--------|
| Terminal | Focuses the terminal's window (restoring it if it is minimised). |
| Settings, Files | Focuses its window if it has one, else starts `/bin/settings` or `/bin/files` as a process. Without `/bin/files` (D18), Files says `Files is not installed.` |
| Host share | Starts Files on `/media/host` (`/bin/files /media/host`, D135); without Files it focuses the terminal and runs `ls /media/host` in it. |
| An installed app | Focuses the terminal and runs its program there, as if its path had been typed at the prompt (single-quoted when it holds a space or other punctuation), after Ctrl-U, so a half-typed line is erased first. The program runs through its layer as at the prompt: a Windows GUI program opens its window, a console program writes in the terminal. |
| `Install from host share...` | Starts the installer's window on `/media/host` (above). |

The kernel reads the catalogue when the desktop starts, each time the launcher opens and when
`app` says the apps changed, never otherwise (D18). A command is typed into the terminal as a
keyboard would type it, after Ctrl-U, which erases a half-typed line at the prompt; if a
program is running in the terminal, that program receives it.
