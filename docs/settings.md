# Settings

Settings is EggOS's settings app (D132), designed by Claude Fable
([desktop-v0.3-gui.md](ux/desktop-v0.3-gui.md) section 10,
[toolkit-v0.3.md](ux/toolkit-v0.3.md) section 11). One program, `/bin/settings`, is all of its
front-ends (D22): run with no arguments it opens the Settings window; with arguments it is the
`settings` command. Its MCP tools (`settings_get`, `settings_set`, `settings_list`) come with
the MCP server (D21).

## The window

Type `settings` at the prompt. The window has a sidebar with a search field and the panes
`Appearance`, `Display`, `Keyboard`, `Mouse`, `Components` and `About`; typing in the search
field keeps the panes whose name, synonyms (`Control Panel`, `Preferences`, `theme`, `HiDPI`,
...) or settings match. Tab moves between the search, the sidebar and the pane's controls;
Down in the search goes to the sidebar.

**Appearance** asks "Where are you coming from?" with three cards, `Windows`, `macOS` and
`Linux`: click one, or move to it with the arrows and press Enter or Space. The desktop changes
at once: the window controls move to the right (Windows, Linux) or the left (macOS) on every
window, and the profile's shortcuts apply (Cmd+W and Cmd+M in the macOS profile, Ctrl+W in the
others, Super+H to minimise in the Linux profile; docs/windows.md). Until the shell (item 35)
there are no panels per profile, so Windows and Linux look the same. The pane shows the
profile's primary modifier and control side. The choice is saved and the desktop starts in it
at the next boot; without a disk the pane says `No disk attached: this choice lasts until
power-off.`

**Display** shows the resolution, the scale (1x) and how the pointer is drawn; **Keyboard** the
layout (US); **About** the version, the architecture, the CPUs, the memory and the uptime;
**Mouse** and **Components** come in a later version. The scale is 1x in 0.3.0 (the `2x`
choice is shown disabled).

## The command

| Command | Does | Status |
|---------|------|--------|
| `settings` | Opens the window and serves it until it is closed. | 0; 1 without a display |
| `settings list` | Writes every setting as `key=value`, one per line. | 0 |
| `settings get <key>` | Writes the setting's value. | 0; 1 for an unknown key |
| `settings set <key> <value>` | Sets it: saved in the settings file, then applied at once on the display (with no display it applies at the next boot). | 0; 1 for an unknown or read-only key, or when saving or applying failed (`settings: <key>: <why>`); 2 for an invalid value or a bad command |

| Key | Value | Settable |
|-----|-------|----------|
| `shell.profile` | `windows`, `macos` (the default) or `linux` | yes |
| `display.scale` | `1` | no (2x comes later) |
| `display.resolution` | the screen's size, `1280x800` | no |
| `display.pointer` | `software` | no |
| `keyboard.layout` | `us` | no |
| `system.version`, `system.arch`, `system.cpus`, `system.memory_mb`, `system.uptime_s` | facts about the machine | no |

## The settings file

`/home/root/.config/eggos/settings`: under `/home`, so on the disk when one is attached (D131)
and in memory for the session otherwise; in `root`'s home until there are accounts. A small
text file: the line `eggos-settings 1`, then `key=value` lines. A file of another version, a
longer one than 4 KiB or garbage holds nothing, so every setting takes its default; unknown
keys are kept when the file is written, and the last line of a key wins. A change is written to
`settings.tmp` beside it, synced, and renamed over it, so a failure leaves the old file whole.
The kernel reads `shell.profile` from it after mounting the disks and before the desktop starts,
so the first frame is already in the saved profile. The format is the `eggos-settings-file`
crate (`settings-file/`), shared by the kernel and the program.

Not yet: the boot parameter `eggos.profile=` (the kernel has no command line yet), per-user
settings (there are no accounts), 2x, key repeat, the mouse settings, the components list.
