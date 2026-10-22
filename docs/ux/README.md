# UX track: designing the desktop for switchers

EggOS should feel familiar on day one to someone coming from macOS, Windows or a Linux desktop
(GNOME, KDE Plasma), and then feel like an upgrade. This folder collects what those users expect
and how EggOS maps each habit, so the desktop (M12) is designed before it is built. Designs are made by Claude Fable (D16).
Sources: each platform's public user documentation and human interface guidelines, and observed
behaviour. A UX regression is treated like a performance regression.

## Principles
1. **Habits just work.** A user's muscle memory from their previous OS keeps working: shortcuts,
   window controls, where files live, how settings are found.
2. **Choose a "home" convention at first run, not a compromise.** First-run setup asks
   "Where are you coming from?" (macOS / Windows / GNOME / KDE) and sets shortcut layout, window
   control side, and dock/taskbar style accordingly. Every choice stays changeable in Settings.
3. **One action, many names.** Search in Settings and the launcher accepts every platform's term
   ("Control Panel", "System Settings", "Preferences"; "Trash", "Recycle Bin"; "Finder",
   "Explorer", "Files", "Dolphin").
4. **Accessibility is not a later feature.** Screen reader, keyboard-only use, scaling and contrast
   are part of the first desktop milestone.

## Keyboard shortcuts
The primary modifier follows the chosen profile: **Cmd** (macOS profile, `Super`/Win key on PC
keyboards acts as Cmd) or **Ctrl** (Windows, GNOME, KDE profiles).

| Action | macOS | Windows | GNOME | KDE | EggOS plan |
|--------|-------|---------|-------|-----|------------|
| Copy / paste / cut / undo | Cmd+C/V/X/Z | Ctrl+C/V/X/Z | Ctrl+… | Ctrl+… | primary modifier + C/V/X/Z, everywhere including terminals (terminal copy without breaking Ctrl+C = SIGINT: copy when there is a selection) |
| App launcher / search | Cmd+Space | Win | Super | Alt+F1 / Super | Super, and Cmd+Space in macOS profile; both always work |
| Switch apps | Cmd+Tab | Alt+Tab | Alt+Tab / Super+Tab | Alt+Tab | profile's key; the other also works |
| Switch windows of one app | Cmd+` | — | Alt+` | — | Alt+` / Cmd+` |
| Close window / quit app | Cmd+W / Cmd+Q | Alt+F4 / Ctrl+W | Ctrl+W / Ctrl+Q | Ctrl+W / Ctrl+Q | all honoured; quit asks only if unsaved work |
| Show desktop | F11 / Cmd+F3 | Win+D | unbound by default | Meta+D (Plasma 6) | Super+D in every profile, plus F11 in macOS profile |
| Lock screen | Ctrl+Cmd+Q | Win+L | Super+L | Super+L | Super+L in every profile, plus Ctrl+Cmd+Q in macOS profile |
| Screenshot | Cmd+Shift+3/4/5 | Win+Shift+S, PrtScn | PrtScn | PrtScn / Meta+Shift+S | all honoured |
| Snap window left/right | (Sequoia) Fn+Ctrl+arrows | Win+←/→ | Super+←/→ | Meta+←/→ | Super+←/→, plus drag-to-edge snapping |
| Maximise / restore | double-click title / green button | Win+↑/↓ | Super+↑/↓ | Meta+PgUp | Super+↑/↓ |
| Virtual desktops | Ctrl+←/→ | Win+Ctrl+←/→ | Super+PgUp/PgDn | Ctrl+F1..F4 | profile's keys; Ctrl+Super+←/→ always |
| Task manager | Cmd+Opt+Esc | Ctrl+Shift+Esc | (System Monitor) | Ctrl+Esc | Ctrl+Shift+Esc and Cmd+Opt+Esc |
| Emoji picker | Ctrl+Cmd+Space | Win+. | Super+. | Meta+. | Super+. and Ctrl+Cmd+Space |

## Window management
| Habit | macOS | Windows | GNOME | KDE | EggOS plan |
|-------|-------|---------|-------|-----|------------|
| Window controls | left, close/min/zoom (traffic lights) | right, min/max/close | right, close only by default | right, min/max/close | side and set follow the profile |
| Menus | global menu bar | in-window | in-window / header-bar | in-window (global optional) | in-window by default; global menu bar in macOS profile |
| Taskbar / dock | Dock, bottom | taskbar, bottom, centred (11) | dash in Activities | panel, bottom | one shell component, styled per profile |
| Overview | Mission Control (F3, 3-finger up) | Task View (Win+Tab) | Activities (Super) | Overview (Meta+W) | one overview, all gestures and keys |
| Touchpad gestures | 3/4-finger swipes, pinch | 3/4-finger swipes | 3-finger swipes | 3/4-finger swipes | same gesture set as user's profile |

## File locations
| Concept | macOS | Windows | Linux (XDG) | EggOS plan |
|---------|-------|---------|-------------|------------|
| Home | `/Users/<name>` | `C:\Users\<name>` | `/home/<name>` | `/home/<name>`; `/Users` → `/home` alias; compatibility layers map `C:\Users` and `/Users` |
| Documents, Downloads, Desktop, Pictures, Music, Videos | `~/Documents` … | `%USERPROFILE%\Documents` … | XDG user dirs | XDG user dirs, same English names on every profile |
| Apps | `/Applications` | `C:\Program Files` | `/usr/share/applications` | one app catalogue; compatibility layers see their native path |
| Trash | `~/.Trash` | Recycle Bin | `~/.local/share/Trash` (freedesktop spec) | freedesktop Trash, shown as "Trash" or "Recycle Bin" by profile |
| Removable drives | `/Volumes` | drive letters | `/run/media/<user>` | `/run/media/<user>`; shown as drives in the file manager |

## Settings
Users search for the setting rather than navigate. EggOS Settings must have: instant search with
synonyms across platforms (principle 3), the same top-level areas users know (Network, Bluetooth,
Displays, Sound, Notifications, Appearance, Privacy & Security, Users, Apps, Keyboard, Mouse &
Touchpad, Accessibility, Updates, About), and every setting reachable by an AI agent through the
AI-native interface (D3) with the same permissions.

## Accessibility (first desktop milestone, not later)
Screen reader with the familiar toggles (Cmd+F5 VoiceOver, Win+Ctrl+Enter Narrator,
Super+Alt+S Orca), full keyboard navigation, display scaling per monitor, high contrast, reduced
motion, sticky/slow keys, live captions later.

## Open questions (for the owner, when the desktop milestone gets close)
- Default profile when the user skips first-run setup.
- Whether to ship a global menu bar outside the macOS profile.
