# EggOS v0.3.0 terminal emulator: design brief

Part of the v0.3.0 desktop design (`desktop-v0.3.md`, D109). The console terminal shows the
same byte stream as the serial console: programs write to descriptors 0-2, the console sends
every byte to the UART and to this emulator, and keys become bytes that enter the console's
line discipline (`console::line_discipline::receive`, D100) exactly as UART bytes do. The
emulator never adds bytes to the output stream; its only replies (DSR) go into the input.

## Text area

- Grid: 80x24 cells of 8x16 (section 5 of the desktop brief; a maximised window changes the
  grid). Origin at the content's top-left plus the 8 px padding. `TIOCGWINSZ` reports
  `ws_row 24, ws_col 80, ws_xpixel 640, ws_ypixel 384` and follows the grid.
- Cell: 4 bytes, `glyph: u8` (ids from the font table), `fg: u8` and `bg: u8` (0-15 ANSI, 16
  = default), `flags: u8` (bit 0 bold, bit 1 underline, bit 2 inverse, bit 3 wide-continuation
  reserved). Bold draws the bright variant of an ANSI 0-7 foreground and the default foreground
  unchanged; inverse swaps the resolved colours; underline draws row 14.
- Screen: 24x80 cells (7.5 KiB) plus an alternate screen of the same size, both fixed arrays.
- Scrollback: a ring of 256 lines of 80 cells (80 KiB), fixed, no allocation; lines that
  scroll off the top of the main screen go in, the oldest are dropped. The alternate screen
  has none. `CSI 3 J` clears it. Viewing: Shift+PageUp / Shift+PageDown move the view by 23
  lines, Shift+Home / Shift+End to the oldest / the live screen, the wheel by 3 lines per
  notch; any key that produces bytes jumps back to the live screen. While scrolled back the
  cursor is not drawn and a 1-cell `▲` in `accent` sits in the top-right cell of the grid.
- Cursor: a block in `term.cursor` covering the cell, the cell's glyph drawn in `term.bg` on
  it; blinks 500 ms on / 500 ms off, restarted "on" by every byte in or out; hidden by
  `CSI ? 25 l`, steady with `CSI ? 12 l`. The cursor is drawn only when the window has focus;
  without focus it is a 1 px `term.cursor` outline of the cell.
- Selection, copy and paste: not in 0.3.0. `term.selection` is reserved.
- Mouse: no reporting to programs in 0.3.0 (`CSI ? 1000 h` and friends are accepted and
  ignored); the wheel scrolls the view.
- Bell (`0x07`): ignored in 0.3.0 (no audio, no flash).
- Tabs: stops every 8 columns; `HT` moves to the next stop or the last column.
- Autowrap on by default (`CSI ? 7 h`): a glyph written in column 80 sets the pending-wrap
  flag and the next glyph starts a new line, as xterm does, so an exactly 80-column line does
  not produce an empty line.
- UTF-8 input to the emulator: decoded to a codepoint, then to a glyph id (ASCII, the extras
  listed in the font section, else `?`); an invalid sequence draws one `?` per bad byte.
  Combining characters and wide characters are not handled (each codepoint takes one cell).

## The ANSI subset

Everything `eggsh`, the core utilities and a nano-like editor need. Unknown sequences are
parsed to completion (so no garbage is printed) and ignored. Parameters are decimal, `;`
separated, missing or 0 meaning the default (1 for movement, 0 for erase modes); at most 16
parameters, longer sequences are dropped.

| Sequence | Name | Behaviour |
|----------|------|-----------|
| `BS` 0x08 | backspace | cursor left one column, not past column 1 |
| `HT` 0x09 | tab | next tab stop |
| `LF` 0x0A, `VT` 0x0B, `FF` 0x0C | line feed | cursor down; at the bottom of the scroll region, scroll up one line |
| `CR` 0x0D | carriage return | column 1 |
| `ESC 7` / `ESC 8` | save / restore cursor | position, SGR attributes and the pending-wrap flag (DECSC/DECRC) |
| `ESC D` / `ESC M` | index / reverse index | down / up one line, scrolling the region at its edge |
| `ESC E` | next line | `CR` + `LF` |
| `ESC c` | reset | main screen, grid cleared, cursor home, SGR reset, modes default, scroll region full |
| `ESC ( B`, `ESC ) 0`, `ESC =`, `ESC >` | charset and keypad selects | accepted, no effect (keypad is always numeric in 0.3.0) |
| `CSI n A` / `B` / `C` / `D` | cursor up / down / right / left | by `n`, clamped to the scroll region (A, B) and the grid (C, D) |
| `CSI n E` / `F` | next / previous line | `n` lines, column 1 |
| `CSI n G` | column | absolute column `n` |
| `CSI r ; c H`, `CSI r ; c f` | cursor position | row `r`, column `c`, 1-based, clamped; with `CSI ? 6 h` relative to the scroll region |
| `CSI n d` | row | absolute row `n` |
| `CSI n J` | erase in display | 0: cursor to end; 1: start to cursor; 2: whole screen; 3: whole screen and scrollback. Erased cells take the current background |
| `CSI n K` | erase in line | 0: cursor to end of line; 1: start to cursor; 2: whole line |
| `CSI n L` / `M` | insert / delete lines | at the cursor, within the scroll region |
| `CSI n @` / `P` / `X` | insert / delete / erase characters | in the cursor's line |
| `CSI n S` / `T` | scroll up / down | the scroll region by `n` lines |
| `CSI t ; b r` | scroll region | rows `t` to `b` (default full), cursor home |
| `CSI s` / `CSI u` | save / restore cursor | position only (SCOSC/SCORC) |
| `CSI 6 n` | device status | replies `ESC [ r ; c R` into the input |
| `CSI c`, `CSI > c` | device attributes | replies `ESC [ ? 1 ; 2 c` / `ESC [ > 0 ; 10 ; 0 c` |
| `CSI ? 25 h` / `l` | cursor visible / hidden | |
| `CSI ? 12 h` / `l` | cursor blinking / steady | |
| `CSI ? 7 h` / `l` | autowrap on / off | |
| `CSI ? 6 h` / `l` | origin mode | cursor positions relative to the scroll region |
| `CSI ? 1 h` / `l` | application cursor keys | arrows send `ESC O A` etc. instead of `ESC [ A` |
| `CSI ? 1049 h` / `l`, `? 1047`, `? 47` | alternate screen | 1049 also saves/restores the cursor; the alternate screen starts cleared and has no scrollback |
| `CSI ? 2004 h` / `l`, `? 1000-1006 h` / `l` | bracketed paste, mouse reporting | accepted, no effect |
| `CSI m` with the parameters below | select graphic rendition | |
| `OSC ... BEL` / `OSC ... ESC \` | operating system commands | parsed and ignored (so a title set by a program is harmless); 1 KiB at most |

SGR parameters: `0` reset; `1` bold; `2` faint (no effect, accepted); `4` underline; `7`
inverse; `22` / `24` / `27` clear bold / underline / inverse; `30-37` foreground 0-7; `39`
default foreground; `40-47` background 0-7; `49` default background; `90-97` foreground
8-15; `100-107` background 8-15; `38 ; 5 ; n` and `48 ; 5 ; n` set colour `n` for `n` in
0-15 (16-255 map to the nearest of the 16 by the usual 6x6x6 cube and grey ramp formula:
cube levels ≥ 128 count as bright, picking the 3-bit colour from the three channels'
thresholds, greys 232-243 black / bright black, 244-255 white / bright white); `38 ; 2 ; r ;
g ; b` and `48 ; 2 ; ...` map to the nearest of the 16 the same way. `TERM` is `xterm` in
init's environment for the shell (this subset is a subset of xterm's; there is no terminfo
database on the system, so the name only tells future ports what to expect).

## Keyboard

virtio-input delivers Linux input event codes (`EV_KEY` with the key code, value 1 down,
0 up). The map is US QWERTY; the layout setting is a follow-up. Modifiers are tracked from
their own key codes: Shift 42/54, Ctrl 29/97, Alt 56/100, Super (Meta) 125/126, Caps Lock 58
toggles on press and inverts the case of letters only. Key repeat: 400 ms after the press,
then every 40 ms, for every key that produces bytes, cancelled by its release or any other
press.

Printable keys (code: unshifted, shifted):

| Codes | Unshifted | Shifted |
|-------|-----------|---------|
| 2-13 | `1234567890-=` | `!@#$%^&*()_+` |
| 16-27 | `qwertyuiop[]` | `QWERTYUIOP{}` |
| 30-40 | `asdfghjkl;'` | `ASDFGHJKL:"` |
| 41 | `` ` `` | `~` |
| 43 | `\` | `\|` |
| 44-53 | `zxcvbnm,./` | `ZXCVBNM<>?` |
| 57 | space | space |
| 71-73, 75-77, 79-83 (keypad, numeric) | `789`, `456`, `1230.` | same |
| 55, 74, 78, 98 | `*`, `-`, `+`, `/` | same |

Bytes into the line discipline:

| Key | Bytes | Note |
|-----|-------|------|
| printable | the character as UTF-8 (ASCII here) | Caps Lock inverts letters |
| Enter 28, keypad Enter 96 | `0D` | `ICRNL` turns it into `0A` in canonical mode |
| Backspace 14 | `7F` | `VERASE`; Ctrl+Backspace sends `08` |
| Tab 15 | `09` | Shift+Tab `ESC [ Z` |
| Esc 1 | `1B` | |
| Ctrl + letter | letter & `1F` | Ctrl+C `03` (`VINTR`, SIGINT to the foreground group), Ctrl+D `04` (`VEOF`), Ctrl+U `15` (`VKILL`), Ctrl+W `17` (`VWERASE`), Ctrl+\ `1C` (`VQUIT`), Ctrl+H `08`, Ctrl+I `09`, Ctrl+M `0D`, Ctrl+[ `1B`, Ctrl+] `1D`, Ctrl+^ `1E`, Ctrl+_ `1F`, Ctrl+Space and Ctrl+@ `00` |
| Alt + key | `1B` then the key's bytes | |
| Up / Down / Right / Left 103 / 108 / 106 / 105 | `ESC [ A` / `B` / `C` / `D` | `ESC O A` ... in application cursor mode |
| Home 102 / End 107 | `ESC [ H` / `ESC [ F` | |
| Insert 110 / Delete 111 | `ESC [ 2 ~` / `ESC [ 3 ~` | |
| PageUp 104 / PageDown 109 | `ESC [ 5 ~` / `ESC [ 6 ~` | with Shift: scrollback view, no bytes |
| F1-F4 59-62 | `ESC O P` / `Q` / `R` / `S` | |
| F5-F12 63-68, 87, 88 | `ESC [ 15 ~`, `17`, `18`, `19`, `20`, `21`, `23`, `24 ~` | |
| modified arrows, Home, End, Insert, Delete, Page keys, F keys | xterm's form: `ESC [ 1 ; m A`, `ESC [ 3 ; m ~`, `ESC [ 15 ; m ~` | `m = 1 + Shift·1 + Alt·2 + Ctrl·4`; Shift+PageUp/Down/Home/End are the view keys and send nothing |
| Super alone or with a key | nothing | reserved for the shell |
| Caps Lock, Num Lock, Scroll Lock, modifiers alone | nothing | |

The bytes go through `line_discipline::receive` with the console's echo, so canonical-mode
editing, `^C` and `^D` behave exactly as on the serial console; a program that sets raw mode
(`TCSETS` without `ICANON`, `ECHO`) sees the bytes as listed. The line discipline's input
queue is shared by the UART and the keyboard; both can be typed on at once.

## Pointer

Events from `virtio-tablet` (`EV_ABS`, `ABS_X`/`ABS_Y` in 0..32767, scaled to the screen
size: `x = v * (w - 1) / 32767`) or `virtio-mouse` (`EV_REL`, deltas accumulated into a
clamped screen position), buttons `BTN_LEFT` 0x110, `BTN_RIGHT` 0x111, `BTN_MIDDLE` 0x112,
`REL_WHEEL` (+1 up). Over the text area: wheel scrolls the view; buttons do nothing in 0.3.0
(selection is a follow-up). Elsewhere the desktop brief's section 4 to 6 apply.

## Tests

Host tests in the drawing crate feed byte sequences to the emulator and check the cell grid
and the cursor: every row of the table above, the 16 colours plus bold and inverse, autowrap
at column 80, scrolling into the scrollback and its ring wrap, the alternate screen round
trip, DSR's reply bytes, the key map (every code, every modifier combination in the table)
and the UTF-8 decoder's error path. The QEMU test of the desktop brief checks the rendered
prompt line's hash on all three architectures.
