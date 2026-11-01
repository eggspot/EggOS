# EggOS v0.3.0 Windows GUI programs: design brief

Design by Claude Fable (D16) for v0.3.0 item 32, "Windows GUI apps, part 1": the three things a
Windows program shows that `desktop-v0.3-gui.md` does not define. It extends
`desktop-v0.3-gui.md`: every token, metric, primitive and rule there and in `desktop-v0.3.md`
stays; this brief only adds the message box, the Win32 system colours and the pointer mapping.
The server draws every window's chrome as section 3 of that brief says; the Windows layer draws
only content. Every number is in pixels at scale 1 and doubles at 2x; the font is Egg Mono 8x16
(`font-8x16.txt`) for every glyph, with no second font and no anti-aliasing.

## 0. One rule added for every dialog

The content surface of a `Dialog` window is `menu.bg` (`#1C2430`; the popup and launcher
surface), its body text `text`, its secondary text `text.dim` (5.1:1 on it). `desktop-v0.3-gui.md`
section 9 did not name the surface; this is it, for the message box and the install dialog alike.

## 1. The message box

`MessageBoxW(owner, text, caption, type)` is drawn entirely by the Windows layer as the content of
one EggOS window. It is modal in the Win32 sense (the call returns only when a button closes the
box) and in the server's (its parent gets no input while it is open).

### 1.1 Window

| | With an owner | Without an owner (`NULL`, or an owner that is not shown) |
|---|---|---|
| Role | `Dialog`, parent = the owner's top-level EggOS window (`GetAncestor(owner, GA_ROOT)`) | `Normal` with `FIXED_SIZE` |
| Placement | centred on the parent, clamped (section 3.4 of the GUI brief) | `SetPosition` to the centre of the work area; the server clamps |
| Chrome | close control only; no resize, no maximise | three controls; the maximise circle is `button.inactive` (`FIXED_SIZE`); minimise works like any window's |
| Window list | its parent's entry | its own entry, titled as below |

A minimised owner is restored first (`SetState Normal`), then the box opens on it. Title: the
caption, UTF-16 converted to UTF-8 (a codepoint the font lacks draws `?`), cut at 64 bytes on a
codepoint boundary; `Error` when the caption is `NULL`. The server cuts a long title with `...`.
Flags without effect in part 1: `MB_RIGHT`, `MB_RTLREADING`, `MB_SETFOREGROUND`, `MB_TOPMOST`,
`MB_SYSTEMMODAL`, `MB_TASKMODAL`, `MB_SERVICE_NOTIFICATION`, `MB_DEFAULT_DESKTOP_ONLY`. When the
server refuses the window (`ENOSPC`, `ENOMEM`) the call returns 0, as Win32 does.

### 1.2 Content layout

Content coordinates, (0, 0) the content's top-left; dialog padding 16 everywhere.

| Element | Rule |
|---|---|
| Icon | 32x32 at (16, 16) when `type & MB_ICONMASK` is not 0 (section 1.3) |
| Text column | x = 64 with an icon (16 + 32 + 16), x = 16 without |
| Text lines | pitch 20 (16 glyph rows + 4), `text` on `menu.bg`; `n` lines are `20n - 4` tall |
| Text top | y = 16; with an icon and exactly one line, y = 24 (the line is centred on the icon) |
| Body height `body_h` | `max(20n - 4, 32)` with an icon, `20n - 4` without |
| Wrap width `wrap_w` | `min(480, work_w - 128)`, a whole number of glyphs (`wrap_w / 8` glyphs; 60 at 1280 wide) |
| Content width `w` | `max(256, text_x + widest_line_px + 16, buttons_px + 32)` |
| Buttons row | y = `16 + body_h + 16`, 28 tall; the row's right edge at `w - 16` |
| Content height `h` | `body_h + 76` (16 + body_h + 16 + 28 + 16) |
| Outside size | `w + 2` by `h + 30` |

Text rules, from the UTF-16 `text`: `\r\n`, `\n` and a lone `\r` each end a line (a trailing one
makes an empty last line); a tab is one space; other codepoints below 0x20 are dropped; a
codepoint the font lacks is `?`; `&` is a plain `&` (no mnemonics in the body). A line is wrapped
greedily at `wrap_w`: it breaks at the last space that fits, every space at the break is
dropped, and a word longer than `wrap_w` is cut at `wrap_w` and continues on the next line.
Empty text is one empty line. At most 24 lines: when the text has more, lines 1-23 are shown and
line 24 is `...` (three `.` glyphs) in `text.dim`. The widest line sets the width; a single `OK`
box with short text is the 256 minimum.

### 1.3 Icons

Four 32x32 pictures, each a rounded rect r6 (`INSET_R6`) filling the 32x32 in the tile colour,
then one glyph at 2x (a 16x32 cell drawn with `glyph_2x`, foreground `icon.dark` `#0B0E13`,
background the tile colour) at (8, 0) inside the tile, so its ink is centred and the corner insets
are never touched. Selected by `type & 0xF0`:

| `type & 0xF0` | Flags | Tile | Glyph | Ink at 2x (tile coordinates) |
|---|---|---|---|---|
| `0x10` | `MB_ICONERROR`, `MB_ICONHAND`, `MB_ICONSTOP` | `button.close` `#E5534B` | `X` | columns 10..21, rows 4..25 |
| `0x20` | `MB_ICONQUESTION` | `icon.windows` `#4F8FE0` | `?` | columns 10..21, rows 4..25 |
| `0x30` | `MB_ICONWARNING`, `MB_ICONEXCLAMATION` | `button.minimise` `#E8B33C` | `!` | columns 14..15, rows 4..25 |
| `0x40` | `MB_ICONINFORMATION`, `MB_ICONASTERISK` | `icon.windows` `#4F8FE0` | `i` | columns 12..17, rows 4..25 |

Any other value in the nibble draws no icon. The glyph shapes are the font's (`X` rows 2-12,
`?` rows 2-12, `!` rows 2-12, `i` rows 2-12, doubled). No sound in part 1.

### 1.4 Buttons

| `type & 0xF` | Buttons, left to right | Result of each (`IDxxx`) |
|---|---|---|
| 0 `MB_OK` (and 7-15) | `OK` | 1 |
| 1 `MB_OKCANCEL` | `OK`, `Cancel` | 1, 2 |
| 2 `MB_ABORTRETRYIGNORE` | `Abort`, `Retry`, `Ignore` | 3, 4, 5 |
| 3 `MB_YESNOCANCEL` | `Yes`, `No`, `Cancel` | 6, 7, 2 |
| 4 `MB_YESNO` | `Yes`, `No` | 6, 7 |
| 5 `MB_RETRYCANCEL` | `Retry`, `Cancel` | 4, 2 |
| 6 `MB_CANCELTRYCONTINUE` | `Cancel`, `Try Again`, `Continue` | 2, 10, 11 |

`MB_HELP` appends `Help` (result: none; it sends `WM_HELP` to the owner, when there is one, and
does not close the box). The labels are Windows' with the `&` removed; the letter after it is the
button's mnemonic (`A`, `R`, `I`, `Y`, `N`, `T`, `C` for Continue, `H`; `OK` and `Cancel` have
none) and is underlined: the glyph's row 14 in the text colour, as `desktop-v0.3.md` defines
underline.

Geometry: push buttons of section 1 of the GUI brief, 28 tall, width `max(96, 8 * glyphs + 32)`
(every label is 96 except `Try Again`, 104), radius 4 (`INSET_R4`), 8 px apart, right-aligned as
a group to `w - 16`, in the table's order (so the last button's right edge is at `w - 17`). The
label is centred: x = `bx + (bw - 8 * glyphs) / 2` (integer division), glyph top at `by + 6`.

States, one rule for every button:

| State | Fill | Border (1 px, `INSET_R4`) | Text |
|---|---|---|---|
| plain | `button.bg` | `button.border` | `text` |
| focused (= the default) | `button.primary` | `button.primary` | `icon.dark` |
| hovered (pointer inside, either of the above) | unchanged | `text.dim` | unchanged |
| pressed (button held inside, or Space held on the focused one) | `quarter(fill)` (`#202730` plain, `#B6872E` focused) | as hovered | unchanged |

Exactly one button is focused at any time; it is the default button, so the primary look and the
focus ring always follow the focus (Windows moves the default to the focused push button the same
way). The focus ring is the 2 px `INSET_R4` outline drawn 2 px inside the button's edge, in
`icon.dark` (on `button.primary` the token `focus.ring` would vanish: same colour). Initial focus:
`type & 0xF00` picks button 1 (`0x000`), 2 (`0x100`), 3 (`0x200`) or 4 (`0x300`); an index past
the last button means button 1. Focus cannot leave the buttons; the ring is drawn whether or not
the window is focused.

### 1.5 Keyboard and mouse

Keys arrive as the server's `Key` events (Linux key codes; repeats, value 2, count as presses for
focus movement only):

| Key | Does |
|---|---|
| Enter (28), keypad Enter (96), on press | activates the focused button |
| Space (57) | press: the focused button shows pressed; release: activates it (nothing if the press was not seen) |
| Tab (15), Right (106), Down (108) | focus the next button, wrapping |
| Shift+Tab, Left (105), Up (103) | focus the previous button, wrapping |
| Esc (1) | activates `Cancel`; `OK` in an `MB_OK` box; ignored by `MB_YESNO` and `MB_ABORTRETRYIGNORE` |
| a mnemonic letter, plain or with Alt (codepoint compared case-insensitively) | activates that button |
| anything else | ignored (never forwarded to the owner) |

`CloseRequest` (the close control, Alt+F4, the profile's Ctrl+W or Cmd+W) does what Esc does;
in the two types without a cancel path it is ignored and the box stays. The server still draws
the close circle normally for those: drawing it `button.inactive` needs a create flag the server
does not have (item 42 adds `NO_CLOSE`; the console terminal already draws that look).

Mouse: a left button-down inside a button focuses it and starts a press; while held, the button
is pressed when the pointer is inside it and plain when outside (the server's implicit grab
delivers the motion); a release inside the same button activates it, a release anywhere else
cancels. Button-down elsewhere in the content does nothing. The pointer over the content is
`Arrow`. Activating closes the window (`Destroy`) and returns the button's result; after a
`Help` press the box stays.

### 1.6 Worked example

`MessageBoxW(hwnd, L"You pressed the space bar.", L"win-paint", MB_OKCANCEL | MB_ICONINFORMATION)`
with an owner: a `Dialog` titled `win-paint`, content **288x108**, outside **290x138**, centred
on the owner's window (outside top-left at `parent.x + (parent.w - 290) / 2`,
`parent.y + (parent.h - 138) / 2`, then clamped into the work area). Text: 26 glyphs, 208 px,
one line; `body_h` 32; `w = max(256, 64 + 208 + 16, 200 + 32) = 288`; `h = 32 + 76 = 108`.

| Element | Rectangle (x, y, w, h) | Pixels |
|---|---|---|
| surface | 0, 0, 288, 108 | `menu.bg` |
| icon tile | 16, 16, 32, 32 | rounded r6, `icon.windows` |
| icon glyph `i` at 2x | 24, 16, 16, 32 | `icon.dark` on `icon.windows`; ink x 28..33, y 20..41 |
| text `You pressed the space bar.` | 64, 24, 208, 16 | `text` on `menu.bg` |
| `OK` button (focused, default) | 72, 64, 96, 28 | fill and border `button.primary` |
| `OK` focus ring | 74, 66, 92, 24 | 2 px `INSET_R4` outline, `icon.dark` |
| `OK` label | 112, 70, 16, 16 | `icon.dark` |
| `Cancel` button | 176, 64, 96, 28 | fill `button.bg`, border `button.border` |
| `Cancel` label | 200, 70, 48, 16 | `text` |
| bottom padding | 0, 92, 288, 16 | `menu.bg` |

Enter or a click on `OK` returns 1; Esc, the close control, Alt+F4 or `Cancel` returns 2; Tab
moves the yellow fill and the ring to `Cancel` (then Enter returns 2). Hovering `Cancel` turns its
border `text.dim`; holding the button on it fills it `#202730`.

## 2. Win32 system colours inside a program's client area

**Decision: the system colours keep Windows' default light values.** Programs were designed
against them (black text on a white window, bitmaps and custom drawing mixed with
`GetSysColor`), Windows itself leaves classic Win32 programs light in its dark mode, and a dark
`COLOR_WINDOW` under a program's hard-coded black text would make the program unreadable. A light
client area inside EggOS's dark chrome is exactly what a Windows user sees in dark mode today.
A themed look for programs that opt in comes with Windows controls (item 42), not before.

`GetSysColor(i)` returns a `COLORREF`, packed `0x00BBGGRR`; the framebuffer packs `0x00RRGGBB`,
so the layer swaps the red and blue bytes when it paints. The table is in RGB:

| Index | Name | RGB | Index | Name | RGB |
|---|---|---|---|---|---|
| 0 | `COLOR_SCROLLBAR` | `C8C8C8` | 16 | `COLOR_BTNSHADOW` / `3DSHADOW` | `A0A0A0` |
| 1 | `COLOR_BACKGROUND` / `DESKTOP` | `000000` | 17 | `COLOR_GRAYTEXT` | `6D6D6D` |
| 2 | `COLOR_ACTIVECAPTION` | `99B4D1` | 18 | `COLOR_BTNTEXT` | `000000` |
| 3 | `COLOR_INACTIVECAPTION` | `BFCDDB` | 19 | `COLOR_INACTIVECAPTIONTEXT` | `000000` |
| 4 | `COLOR_MENU` | `F0F0F0` | 20 | `COLOR_BTNHIGHLIGHT` / `3DHIGHLIGHT` | `FFFFFF` |
| 5 | `COLOR_WINDOW` | `FFFFFF` | 21 | `COLOR_3DDKSHADOW` | `696969` |
| 6 | `COLOR_WINDOWFRAME` | `646464` | 22 | `COLOR_3DLIGHT` | `E3E3E3` |
| 7 | `COLOR_MENUTEXT` | `000000` | 23 | `COLOR_INFOTEXT` | `000000` |
| 8 | `COLOR_WINDOWTEXT` | `000000` | 24 | `COLOR_INFOBK` | `FFFFE1` |
| 9 | `COLOR_CAPTIONTEXT` | `000000` | 25 | (unused) | `000000` |
| 10 | `COLOR_ACTIVEBORDER` | `B4B4B4` | 26 | `COLOR_HOTLIGHT` | `0066CC` |
| 11 | `COLOR_INACTIVEBORDER` | `F4F7FC` | 27 | `COLOR_GRADIENTACTIVECAPTION` | `B9D1EA` |
| 12 | `COLOR_APPWORKSPACE` | `ABABAB` | 28 | `COLOR_GRADIENTINACTIVECAPTION` | `D7E4F2` |
| 13 | `COLOR_HIGHLIGHT` | `0078D7` | 29 | `COLOR_MENUHILIGHT` | `0078D7` |
| 14 | `COLOR_HIGHLIGHTTEXT` | `FFFFFF` | 30 | `COLOR_MENUBAR` | `F0F0F0` |
| 15 | `COLOR_BTNFACE` / `3DFACE` | `F0F0F0` | 31+ | out of range | `GetSysColor` returns 0 |

`GetSysColorBrush(i)` and a class background of `(HBRUSH)(i + 1)` paint the same values.
`SetSysColors` is accepted and ignored in part 1 (it would be system-wide on Windows).

Win32 semantics stay Win32's: `GetStockObject` gives `WHITE_BRUSH` `FFFFFF`, `LTGRAY_BRUSH`
`C0C0C0`, `GRAY_BRUSH` `808080`, `DKGRAY_BRUSH` `404040`, `BLACK_BRUSH` `000000`, `NULL_BRUSH`
(paints nothing), `WHITE_PEN` and `BLACK_PEN` 1 px solid, `NULL_PEN` (draws nothing), `DC_BRUSH`
white and `DC_PEN` black until `SetDCBrushColor`/`SetDCPenColor`; a fresh device context has text
`000000`, background `FFFFFF`, `OPAQUE` mode, `BLACK_PEN`, `WHITE_BRUSH` and `SYSTEM_FONT`.

Before the first paint: the layer fills a window's buffer with `window.bg` (`#12161D`) when it
creates the buffer and again after every `SetSize`, and makes the first `Commit` only when the
first `WM_PAINT` has ended (`EndPaint`, or `DefWindowProc` validating it), so the user sees the
window appear once, painted. A class with no background brush that paints nothing therefore
shows `window.bg`, the same neutral surface the server shows during a resize; a class with a
brush is erased with it by `WM_ERASEBKGND` before that commit, so its first frame is already its
colour. On Windows such a window would show stale pixels; `window.bg` is the deliberate
replacement.

Text: Egg Mono 8x16 is every font in part 1 (`SYSTEM_FONT`, `DEFAULT_GUI_FONT`,
`ANSI_FIXED_FONT`, `ANSI_VAR_FONT`, `SYSTEM_FIXED_FONT`, `DEVICE_DEFAULT_FONT`, and anything
`CreateFontW`/`CreateFontIndirectW` returns: face, height, weight and italic are ignored).
`GetTextMetricsW` reports height 16, ascent 13 (baseline row 12), descent 3, internal and
external leading 0, average and maximum width 8, a fixed-pitch, non-TrueType face named
`Egg Mono`. `DrawTextW` and `TextOutW` follow Win32: `DT_CENTER` puts the line at
`left + (width - 8n) / 2`, `DT_RIGHT` at `right - 8n`, `DT_VCENTER | DT_SINGLELINE` at
`top + (height - 16) / 2` (integer division; `DT_BOTTOM | DT_SINGLELINE` at `bottom - 16`),
multi-line text advances 16 per line (Win32's line height is the font's; the message box's
pitch 20 is the dialog's, not `DrawText`'s), `DT_WORDBREAK` wraps with the same greedy rule as
section 1.2 (the one implementation), a prefix `&` underlines the next glyph at row 14 unless
`DT_NOPREFIX`, `&&` is `&`, a tab is one space unless `DT_EXPANDTABS` (then to the next multiple
of 8 cells), `DT_END_ELLIPSIS` ends a cut line with `...` (three `.` glyphs), a codepoint the
font lacks draws `?`, and `OPAQUE` text fills its 8x16 cells with the background colour. Nothing
here needs a design call beyond those; underline and `...` reuse the font's one way of doing them.

## 3. Pointer

`LoadCursorW(NULL, id)` returns a handle for every standard id; `SetCursor` (and `DefWindowProc`'s
`WM_SETCURSOR`, which sets the class cursor) maps it to the server's `SetPointer` shape:

| Win32 id | Server shape |
|---|---|
| `IDC_ARROW` | `Arrow` (0) |
| `IDC_IBEAM` | `IBeam` (1) |
| `IDC_SIZENS` | `ResizeNS` (2) |
| `IDC_SIZEWE` | `ResizeEW` (3) |
| `IDC_SIZENWSE` | `ResizeNWSE` (4) |
| `IDC_SIZENESW` | `ResizeNESW` (5) |
| `IDC_SIZEALL`, `IDC_SIZE` | `Arrow` (the GUI brief's `Move` is the arrow) |
| `IDC_WAIT` | `Busy` (8, D186, `desktop-polish-v0.3.md` section 1) |
| `IDC_APPSTARTING` | `Arrow` (the composite arrow-and-hourglass is a follow-up) |
| `IDC_HAND` | `Hand` (7, D186) |
| `IDC_CROSS`, `IDC_NO`, `IDC_HELP`, `IDC_UPARROW`, `IDC_ICON`, `IDC_PERSON`, `IDC_PIN` | `Arrow` |
| a class with `hCursor` `NULL`, or `SetCursor(NULL)` | `Arrow` (the server cannot hide the pointer over one window in 0.3.0; listed gap) |

`LoadCursorW` with a module handle (a program's own cursor resource), `CreateCursor` and
`LoadImageW` cursors give a handle that maps to `Arrow`. The `Busy` and `Hand` shapes are drawn
since D186 (`desktop-polish-v0.3.md`). Over the message box the pointer is always `Arrow`.

## 4. Tests

Host tests hash the worked example's content at 288x108 and check its table pixel by pixel
(the tile's corner insets, the glyph's ink rows, the ring's 2 px, the label positions), the
layout for each of the seven button sets with and without `MB_HELP` (widths, the 104 of
`Try Again`), the wrap rules (a space break, a `\r\n`, a 70-glyph word, 30 lines becoming 23
plus `...`), the focus order and Esc per type, the mnemonics, the system colour table against
`GetSysColor` with the byte swap, `GetTextMetricsW`, and the pointer mapping. The QEMU boot test's
Windows GUI program opens the worked example's box and the `screendump` is compared with the
host's render of it centred on its owner.
