# EggOS v0.1 Windows layer: the menu bar selected by Alt, and keyboard cues: design brief

Design by Claude Fable (D16) for the two deviations D338 and D339 record: a Windows program's
menu bar (`windows-controls-v0.3.md` section 6, `userland/windows/src/win32/menu.rs`) is
**selected without a drop-down** when Alt is pressed and released alone or F10 is pressed,
as on Windows, and the mnemonic **underlines show only once Alt is pressed** (Windows' keyboard
cues, `UISF_HIDEACCEL`). Everything in section 6, section 12's menu additions and D335 to
D337 stays: the 24 px bar, the entries `8n + 16` wide with text at `entry_x + 8` and cell top
4, the drop-downs, the keys inside them, submenus. Section 6's own table already names "bar
mode: the first enabled entry highlighted, no drop-down"; this brief gives it its look and
its exact keys, so D339's "the drop-down opens at once" and "the underlines stay always
drawn" are replaced. Numbers are px at 1x in the window's content coordinates and double at
2x; the font is Egg Mono 8x16 (underline row 14 of the cell); the palette is section 0's Light
inside a program, with the Dark column for an EggOS dialog that carries a menu later.

## 0. What a person gets, and how that is checked

- Tap Alt: the first menu title lights up and shows its underlines, nothing drops down; Left
  and Right walk the titles, Down or Enter opens the one lit, Esc or Alt again puts it back.
- Hold Alt and the underlines appear under every title's letter, exactly as on Windows; let
  go without choosing and they go away.
- The pointer keeps working through it: hovering another title moves the light, a click opens.
- Host tests pin the pixels of the three title states, the underline rows and the state
  machine (section 7); a QEMU step on the test window (section 18 of the controls brief).

## 1. The bar title's states

A title's rect is `(entry_x, 0, 8n + 16, 24)` (`n` cells of label); its text at `(entry_x +
8, 4)`; the mnemonic glyph's underline is row **18** (cell row 14) across that glyph's 8
columns, 1 px in the title's text colour.

| State | Fill | Text | Also |
|---|---|---|---|
| plain | `menu.bar` (`F0F0F0`) | `menu.text` (`000000`) | |
| hovered (pointer in, nothing selected or open) | `menu.highlight` (`91C9F7`) | `menu.text` | |
| **selected, closed** (section 2) | `menu.highlight` | `menu.text` | the Light **focus indicator** of section 1 of the controls brief: the 1 px dotted outline of `(entry_x + 2, 2, 8n + 12, 20)` (rows 2 and 21, columns `entry_x + 2` and `entry_x + 8n + 13`, the pixels where `x + y` is even) in `focus` (`000000`) |
| open (a drop-down under it) | `menu.highlight` | `menu.text` | no dotted outline: the drop-down is the indication |
| disabled (`MF_GRAYED`, `MF_DISABLED`) | `menu.bar` | `menu.text.disabled` (`6D6D6D`) | never hovered, selected or opened |

The dotted outline is what tells "selected, closed" from "hovered" in a screendump, where the
pointer is not drawn, and it is the same indicator every focused control of the program shows,
so it needs no new rule. It never meets the underline (row 18 is inside; rows 2 and 21 are
the outline) or the text (rows 4..19, columns from `+ 8`). In the Dark column: fill
`menu.hover`, text `text`, and the toolkit's 2 px `focus.ring` ring (`INSET_R4`) at inset 2
of the title's rect, as every focused control in an EggOS dialog.

Underlines are drawn in a title only while the window's **keyboard cues** are on (section
3); the states above do not change that.

## 2. The session: `Bar::Idle`, `Bar::Selected(entry)`, `Bar::Open(entry)`

One state per top-level window with a menu bar (`Idle` is section 6's "no menu open";
`Open` is today's `Tracking`). `Selected` is new: a title lit as section 1's third row, no
drop-down, every key consumed as section 6 says ("while a menu is open or the bar is in bar
mode, no key reaches the program and pointer events over the client are consumed").

| In | Input | Goes to | And |
|---|---|---|---|
| `Idle` | Alt down (`VK_MENU`, `WM_SYSKEYDOWN`) | `Idle` | remembered (`alt_alone`); cues on; Alt still reaches the program |
| `Idle` | any other key or button down while remembered | `Idle` | forgotten (Alt+letter, Alt+F4 and Alt+click stay what they were) |
| `Idle` | Alt up while remembered | `Selected(first)` | `first` = the first enabled title (one with or without a drop-down); none enabled: stays `Idle` |
| `Idle` | F10 down (window enabled, no combo list open) | `Selected(first)` | cues on |
| `Idle` | Alt + a title's mnemonic | `Open(that)` as D335 | cues on |
| `Idle` | Alt up with nothing remembered, or a button press anywhere in the window, or `FocusOut` | `Idle` | cues off |
| `Selected(e)` | Left / Right | `Selected(previous / next enabled)` | wrapping; the outline moves |
| `Selected(e)` | Down, Enter, Space | `Open(e)` with the first enabled item highlighted | a title with no drop-down (`MF_STRING` on the bar): `WM_COMMAND` posted, `Idle`, cues off |
| `Selected(e)` | Up | `Open(e)` with the **last** enabled item highlighted | as Windows; the same for a title without a drop-down as Enter |
| `Selected(e)` | a title's mnemonic (with or without Alt) | `Open(that)`, first item highlighted | or its command and `Idle` when it has no drop-down |
| `Selected(e)` | Esc, Alt down, F10 | `Idle` | cues off; the Alt up that follows starts nothing |
| `Selected(e)` | any other key | `Selected(e)` | consumed, nothing |
| `Selected(e)` | pointer moves onto another enabled title | `Selected(that)` | closed; onto a disabled title or off the bar: no change |
| `Selected(e)` | button down on a title | `Open(that)` | as a click always did |
| `Selected(e)` | button down elsewhere in the content | `Idle` | the press and its release are not delivered (section 6); cues off |
| `Selected(e)` | `FocusOut` not followed by `FocusIn` in the same read | `Idle` | cues off |
| `Open(e)` | Esc (no submenu open) | `Selected(e)` | the title stays lit, now with the outline; cues stay |
| `Open(e)` | Alt down, or F10 | `Idle` | everything closes, cues off, as Windows |
| `Open(e)` | an item chosen, a click outside, `FocusOut` | `Idle` | cues off |
| `Open(e)` | Left / Right, Up / Down, mnemonics, Enter, Space | as D335 and D337 | unchanged |

Alt+letter that is no title's mnemonic stays the program's, with Alt forgotten; Alt+Tab is a
`FocusOut`. A disabled window (`EnableWindow`, a modal) and a window with a combo list open
take no session. `DrawMenuBar` or a bar change during `Selected` returns to `Idle`.

## 3. Keyboard cues: when the underlines show

One flag per top-level window, `keyboard_cues`, off at creation:

- **On** at Alt down (every Alt, alone or with another key), at F10 down, and when a session
  is entered by any key. It stays on through the whole session, `Selected` and `Open`,
  whatever the pointer does meanwhile.
- **Off** when the session ends (`Idle` is entered, by a choice, Esc, Alt, F10, a click
  outside or `FocusOut`), at an Alt up that started no session (Alt+letter that was the
  program's, a released modifier), and at a button press in the window while `Idle`.
- **Where drawn** while on: in every bar title (row 18, the mnemonic glyph's columns, in the
  title's text colour, `menu.text.disabled` on a disabled title) and in every item of the
  open drop-downs and submenus (row `iy + 18`, the glyph's columns from `dx + 28`, in the
  item's text colour). While off, no underline anywhere in the bar or its menus.
- **A drop-down opened by the pointer** shows no underlines (cues off), as Windows; its
  mnemonics still work (D335). Pressing Alt or F10 while it is open closes it rather than
  showing them (the table above), as Windows.
- **Repaint**: a cue change redraws the bar's rect `(0, 0, content_w, 24)` and the open
  drop-downs in one commit; nothing else moves. `&&` is `&` and takes no underline; a label
  without `&` has none.

The controls' own labels (push buttons, check boxes, radios, statics) keep section 1's rule of
the controls brief, **always underlined**: Windows ties those to the same flag, but that is a
change to every dialog's look and is left out of this brief (section "Later").

## 4. The pointer during a session

- `Selected`: the bar tracks the pointer as a modal loop would: hover over an enabled title
  selects it (the outline follows; no drop-down); the pointer leaving the bar leaves the
  selection where it is; a press on a title opens it; a press on the client is swallowed and
  ends the session; the pointer shape stays `Arrow`.
- `Open`: unchanged (section 6: hovering another enabled title opens it; a release on an item
  chooses).
- No session: the hover look is section 1's second row, as today, without underlines.

## 5. High contrast and the palettes

Inside a program the Light values are fixed (section 0 of the controls brief, part 1), so the
high-contrast setting changes none of section 1's pixels; the look holds because its cues are
not colours: the dotted outline, the underline and the drop-down itself. `menu.text` on
`menu.highlight` is 12.3:1 and the dotted `focus` on it the same. When the Windows layer
starts answering `SPI_GETHIGHCONTRAST` with system colours from the palette
(`first-run-accessibility-v0.3.md` 2.4, "later"), the bar's fills go through
`COLOR_MENUBAR`, `COLOR_MENUHILIGHT` and `COLOR_MENUTEXT` and the outline and underline stay
1 px in the text colour: no third rule. The Dark column follows the tokens' high-contrast
values as every EggOS dialog (`menu.hover` `#262626` / `#E6E6E6`, `focus.ring` `#FFD24D` /
`#7A4E00`).

## 6. What an agent sees (D161, `wm show`)

The `menubar` element's `menuitem` children (section 29 of the controls brief) gain the
toolkit's flags: `selected` on the title that is `Selected` or `Open`, `expanded` on the
`Open` one; `invoke` on a title opens it as a click does (unchanged). Cues are not an element
property: an agent does not need underlines, it reads the name. A screen reader hearing the
session start reads `<title> menu, selected` and then each title as Left and Right move.

## 7. Worked example and tests

The test window's bar `&File`, `&Edit`, `&Help`: `File` at (0, 0, 48, 24), text (8, 4);
`Edit` at (48, 0, 48, 24), text (56, 4); `Help` at (96, 0, 48, 24), text (104, 4).

- Alt down: the bar repaints with underlines at row 18, columns 8..15 (`F`), 56..63 (`E`),
  104..111 (`H`), fills unchanged. Alt up: `Selected(0)`: `File`'s rect filled `91C9F7`, the
  dotted outline of (2, 2, 44, 20): rows 2 and 21 from column 2 to 45 and columns 2 and 45
  from row 2 to 21, the even `x + y` pixels in `000000`; the other titles plain with their
  underlines. Right: the fill and outline on `Edit` (48..95, outline (50, 2, 44, 20)). Down:
  `Edit`'s drop-down open, first enabled item highlighted, the outline gone, the items
  underlined at `iy + 18`. Esc: back to `Selected(1)` with the outline. Esc: `Idle`, no
  underlines, the bar plain.
- Alt down, `F`: `Open(0)` at once (D335), underlines shown. Alt down, `X` (no mnemonic):
  the program gets Alt+X; Alt up: no session, underlines off.
- A click on `Help`: `Open(2)`, no underlines; Alt: `Idle`, bar plain.
- `Selected(0)`, the pointer moved to (60, 12): `Selected(1)`; a press at (200, 100): `Idle`,
  the program gets no button messages; underlines off.
- A bar whose only enabled title is `Help`: Alt alone selects `Help`; Left and Right stay on
  it.

Host tests (`menu.rs`, `drawn.rs`): the state table of section 2 row by row (each transition
and what the program receives, `alt_alone` kept as today); the cue flag's on and off cases of
section 3; the pixels above hashed for the three title states with cues on and off (pixel
(8, 18) `000000` with cues, `F0F0F0` without; (2, 2) and (45, 21) `000000` when `File` is
selected, (3, 2) `91C9F7`; (8, 18) under `Open` with cues `000000`); the drop-down's item
underline row with cues on and its absence after a pointer open; the element flags of
section 6; and the D339 test that expected a drop-down after Alt alone now expecting
`Selected(0)`. QEMU (x86_64 and aarch64, the controls brief's steps): a screendump after Alt
is pressed and released shows `File` lit with the outline and the three underlines, and
`wm show` flags `File` `selected`.

## Decided by design (the owner can overrule any of these)

1. The selected-closed title uses the program's own focus indicator (the Light dotted
   outline, inset 2) on the highlight fill, so a screendump tells it from hover; Windows
   draws no outline, but Windows also draws its pointer in the picture.
2. Up opens the drop-down with its last enabled item highlighted; Down, Enter and Space with
   the first.
3. Alt down (not up) ends a session; the matching Alt up starts nothing.
4. Cues are per window, on at any Alt or F10, off when the session ends or an unused Alt is
   released or the pointer presses in an idle window; a pointer-opened menu shows none.
5. Controls' labels stay always underlined; only the bar and its menus take the cues.
6. Hover moves the selection in `Selected` without opening, as Windows' modal loop does.

## Later

Keyboard cues on the controls' labels (`WM_UPDATEUISTATE`, `WM_QUERYUISTATE`,
`UISF_HIDEACCEL` and `UISF_HIDEFOCUS` answered to programs that ask); `SPI_GETHIGHCONTRAST`
system colours; a menu in an EggOS dialog using the Dark column; `MF_RIGHTJUSTIFY`; the
system menu by Alt+Space.

## Decisions for the engineer

1. `Gui` gains per window `bar: Bar` (`Idle`, `Selected(entry)`, `Open` = `Tracking`) and
   `keyboard_cues: bool`; `alt_alone` stays as it is.
2. Alt up alone and F10 go to `Selected(first_enabled_entry)`, no `open`; `first_open_entry`
   is used only by Down, Enter, Space, Up and the mnemonics.
3. Drawing: the selected title as section 1's third row (`menu.highlight` fill, dotted
   `focus` outline of `(entry_x + 2, 2, 8n + 12, 20)`); underlines at row 18 (`iy + 18` in
   items) only while `keyboard_cues`.
4. The transition table of section 2 and the cue rules of section 3, including the pointer
   rows; everything consumed while not `Idle`.
5. Repaint the bar and open menus on every state or cue change, one commit.
6. Element flags `selected` and `expanded` on the titles.
