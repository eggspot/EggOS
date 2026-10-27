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

The folder's name is the app's name. Item 36 (install) copies a program here and writes the
manifest last, through a temporary file renamed over `app.manifest`, so a half-installed app
has no manifest and is not listed; uninstall removes the manifest first, then the folder.
Until item 36, a manifest can be written by hand:

```
mkdir /Applications/Hello
echo 'eggos-app 1' > /Applications/Hello/app.manifest
echo name=Hello >> /Applications/Hello/app.manifest
echo source=linux >> /Applications/Hello/app.manifest
echo run=/bin/uname >> /Applications/Hello/app.manifest
```

Open the launcher (Super, or Cmd+Space in the macOS profile): it reads the catalogue again and
the app appears there and on the desktop.

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
| Settings, Files | Focuses its window if it has one, else starts `/bin/settings` or `/bin/files` as a process. Files says `Files is not in this build yet.` until item 37. |
| Host share | Focuses the terminal and runs `ls /media/host` in it. |
| An installed app | Focuses the terminal and runs its program there, as if its path had been typed at the prompt (single-quoted when it holds a space or other punctuation). |

The kernel reads the catalogue when the desktop starts and each time the launcher opens,
never otherwise (D18). A command is typed into the terminal as a keyboard would type it: if a
program is running in the terminal, or a line is half typed at the prompt, that program or line
receives it.
