# EggOS v0.3.0 Date & time: design brief (item 29)

Design by Claude Fable (D16) for roadmap item 29 (D150): the kernel reads a real-time clock at
boot (x86_64 CMOS, aarch64 PL031, riscv64 Goldfish), the system clock is UTC and there are no
time zones in this release (local time is UTC, a listed limitation), and setting the clock
(`date -s`, the MCP tool `clock_set`, Settings) writes the hardware clock when there is one.
This brief adds the Settings pane that is the GUI front-end of that one capability (D22) and
settles the top bar's clock. Everything in `toolkit-v0.3.md` (section 11 for Settings) and in
`desktop-v0.3-gui.md` section 10 stands; numbers are px at 1x and double at 2x; the only
motion is the caret's blink and the ticking seconds.

## 1. The pane in the sidebar

- **Name** `Date & time` (the `&` is in the font). It is the fifth row, between `Mouse` and
  `Components`: `Appearance`, `Display`, `Keyboard`, `Mouse`, `Date & time`, `Components`,
  `About`, so the panes about the machine's hardware stay together and the two about the
  system itself stay last. Its row is y 172..203 of the sidebar (44 + 4 x 32), text at x 16.
- **Search words**: `clock`, `time`, `date`, `time zone`, `timezone`, `UTC`, `RTC`,
  `hwclock`, `hardware clock`, `calendar`, plus its setting names `Now` and `Set to`.
- **Budget**: the window's tree holds 48 widgets and 40 are used; this pane is its container
  plus exactly 7 widgets (below), so the tree is full. Nothing else may be added to Settings
  without raising `CAPACITY`.

## 2. Layout (pane coordinates: the pane's origin is window x 176, y 0; it is 464 wide at the
default size, so the right-aligned edge is x 448)

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Date & time` |
| 2 | label | (16, 76) | `Now` in `text` |
| 3 | label, right-aligned | ends at x 448, cell top 76 | the current time (section 3) in `text` |
| 4 | label | (16, 116) | `Set to` in `text` |
| 5 | text field | (144, 110), 200x28 | placeholder `YYYY-MM-DD HH:MM`, `escape: Clear` |
| 6 | push button, primary | (352, 110), 96x28 | `Set`; the window's default button |
| 7 | label | (16, 156) | the status line (section 5), `text.dim` or `text` |

Rows 0..2 of the 40 px grid from y 64: text cell top at row + 12, controls at row + 6; the
field ends 8 px before the button. The pane's content ends at y 172, well inside 332.

## 3. The time shown (`Now`)

`<Weekday> <D> <Month> <YYYY>, <HH>:<MM>:<SS> UTC`, English names, the day without a leading
zero, 24-hour time: `Thursday 29 October 2026, 14:05:09 UTC`. A sentence rather than ISO
because the field is where ISO belongs and a person checks a clock by reading it; the weekday
is the fastest way to notice a wrongly set date. The same format when the clock is unset (it
then reads `Thursday 1 January 1970, 00:00:12 UTC` and the status line explains). The widest
case, `Wednesday 30 September 2026, 23:59:59 UTC`, is about 240 px, so it never meets `Now`.
Right-aligned, the ticking end stays put (tabular digits). Refreshed once a second while the
pane is shown, damaging only this label's rect (as About's uptime line); not refreshed while
hidden. It is refreshed in the same frame as a successful `Set`.

## 4. Setting the clock

- **Accepted**, after trimming spaces: `YYYY-MM-DD HH:MM`, `YYYY-MM-DD HH:MM:SS`, `HH:MM`,
  `HH:MM:SS`. Fixed digit counts; a `T` may stand in for the space; a trailing `Z` or ` UTC`
  is accepted and ignored (so a line from `date` can be pasted back later, when there is a
  clipboard). Seconds default to 00; the time-only forms keep the current UTC date. Month
  1-12, day valid for that month (Gregorian leap years), hour 0-23, minute and second 0-59.
  The year must be 2000 to 2099: every hardware clock we write stores it (CMOS's two digits
  with the century, PL031's 32-bit seconds) and nothing below 1970-01-02 can be mistaken for
  "unset" (section 6). The parser and the range check are the same code as `date -s` and
  `clock_set` (D22: one capability).
- **Set** is the window's default button and primary. It is disabled (`text.dim` on
  `button.bg`, section 4 of the toolkit) while the field is empty, so Enter in the sidebar's
  search field never sets anything; it is enabled on the first character. Enter in the field
  or a click on the button sets the clock to the typed moment, subsecond 0, then writes the
  hardware clock when there is one.
- **After a successful set**: the field is emptied (placeholder back, `Set` disabled), keeps
  the focus and the caret, the status line shows the result and `Now` shows the new time.
- **After an error**: the text and the caret stay for correction; the status line shows the
  error until the next `Set` or until the pane is shown again. Esc clears the field (its
  `escape: Clear`); a second Esc goes to the window, which ignores it.
- **Focus**: Tab from the sidebar list enters the pane at the field, then `Set`, then wraps to
  the search field. Showing the pane (from the sidebar) does not move the focus.

## 5. The status line (widget 7)

One line at (16, 156); it changes only when the pane is shown and when `Set` acts.

| When | Text | Colour |
|------|------|--------|
| shown, a hardware clock was read at boot | `From the hardware clock, read at boot.` | `text.dim` |
| shown, no hardware clock | `No hardware clock: the time started at 1970 at boot.` | `text.dim` |
| set, and the hardware clock took it | `Set, and saved to the hardware clock.` | `text` |
| set, no hardware clock (or its write failed) | `Set. No hardware clock: this time lasts until power-off.` | `text` |
| the text is not a date and time | `Not a date and time. Type YYYY-MM-DD HH:MM or HH:MM.` | `text` |
| a time-only form while the clock is unset | `Type the date too: YYYY-MM-DD HH:MM.` | `text` |
| a year outside 2000 to 2099 | `The year must be 2000 to 2099.` | `text` |
| the kernel answered `EPERM` | `Not allowed: this user may not set the clock.` | `text` |

Showing the pane again empties the field and puts the matching "shown" line back; a stale
result is never kept. No colour beyond `text` and `text.dim` (toolkit section 2: no third text
colour): the wording tells success from failure. The widest line is about 320 px.

## 6. The top bar and taskbar clock

The rule of `desktop-v0.3.md` section 4 stands: realtime seconds >= 86400 means the wall clock
is set and the clock shows `HH:MM` (24 h, UTC) in `text`; below that it shows the uptime forms
in `text.dim`. The threshold is now crossed at boot on every machine with a hardware clock, and
on one without it the moment Settings, `date -s` or `clock_set` sets the clock (the next tick
switches; a CLI set back below 1970-01-02 switches back, the rule is symmetric). The shell
ticks once a second as today; `HH:MM` is redrawn only when the minute changes.

- **Placement**: the rectangle stays 10 glyph cells (80x16) of the bar font, centred on the bar
  (x `(screen_w - 80) / 2`, y 6; on the taskbar, its right edge 56 px from the screen's right,
  vertically centred). The text is **pixel-centred** in it: it starts at
  `rect.x + (80 - 8 x glyphs) / 2`, so `HH:MM` (5 glyphs) starts 20 px in, exactly on the
  bar's midpoint as section 4 asks; `up m:ss` and `up 100d` (7) 12 px in, `up 4d 03h` (9) 4 px
  in, `up h:mm:ss` and `up 10h 05m` (10) at 0. This replaces the cell-centring the
  implementation does today (16 px and 8 px for the odd lengths). The whole 80x16 rect is
  flushed on every change, whichever text is shown.
- **Not a button in 0.3.0**: no hover fill, pointer `Arrow`, no tooltip (the top bar has none;
  the taskbar's tooltips belong to its items), a click is a click on the bar (nothing). Later:
  a click opens the `Date & time` pane.

## 7. Acceptance (host tests, default 640x440, scale 1x)

- `PANES` is `Appearance, Display, Keyboard, Mouse, Date & time, Components, About`; the
  sidebar row of `Date & time` is (0, 172, 176, 32); `clock`, `UTC`, `rtc`, `hwclock`,
  `time zone` and `set to` each match it and nothing else; `time` also matches About
  (`uptime`).
- The tree holds 48 widgets. In window coordinates: heading at (192, 16); `Now` at (192, 76);
  the time label's right edge at 624, cell top 76; `Set to` at (192, 116); the field
  (320, 110, 200, 28) with placeholder `YYYY-MM-DD HH:MM`; `Set` (528, 110, 96, 28), primary,
  disabled while the field is empty; the status line at (192, 156).
- With realtime 1_793_282_709 the time label reads `Thursday 29 October 2026, 14:05:09 UTC`;
  with 12, `Thursday 1 January 1970, 00:00:12 UTC`; a second later each shows the next second.
- Typing `2026-10-29 14:05` and Enter requests a set to 1_793_282_700; `14:05:30` on that day
  requests 1_793_282_730; `2026-10-29T14:05:09Z` requests 1_793_282_709. `2026-02-30 10:00`,
  `29/10/2026` and `14:5` show the "Not a date and time" line and keep their text;
  `1999-12-31 23:59` shows the year line; `14:05` with realtime 12 shows the "Type the date
  too" line; a set answered `EPERM` shows the "Not allowed" line.
- A successful set answered "saved" shows `Set, and saved to the hardware clock.`, empties
  the field, disables `Set` and leaves the focus in the field; one answered "no hardware
  clock" shows the power-off line. Showing another pane and coming back restores the "shown"
  line and an empty field.
- The bar: with uptime 5 the clock rect is (600, 6, 80, 16), text `up 0:05` at x 612 in
  `text.dim`; with realtime 1_793_282_709 the text is `14:05` at x 620 in `text`; it is
  damaged once when the minute changes and not between.
