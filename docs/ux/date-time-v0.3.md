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
  with the century, PL031's 32-bit seconds). A time-only form keeps the current date only when
  its year is in that range; otherwise (an unset clock reads 1970) the date is needed. The parser and the range check are the same code as `date -s` and
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

The rule of `desktop-v0.3.md` section 4, as review round 2 of D150 made it: once the wall clock
is set the clock shows `HH:MM` (24 h, UTC) in `text`; until then it shows the uptime forms in
`text.dim`. The clock is set at boot on every machine with a readable hardware clock, and on one
without it the moment Settings, `date -s` or `clock_set` sets the clock (the next tick
switches). The first rule, realtime seconds >= 86400, showed `1970-01-02` after a day's uptime
on a machine without a readable clock, so the kernel now keeps whether the clock was set. The shell
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

## 8. Time zone (D194, slice 42d): the picker in the pane

Design by Claude Fable for the GUI front-end of `time.zone` (D22): the setting the CLI sets
with `settings set time.zone Europe/Paris` and MCP with `settings_set`, one of the 419 zones
of `eggos_calendar::zone::ZONES` (every zone of tzdata 2026e's `zone.tab` plus `UTC`, sorted
by name, each with its current rule). The clock stays UTC; the zone only changes how times
are shown and read. Sections 1 to 7 stand except where this section says otherwise. The
pattern is macOS's `Closest city` field and GNOME's zone search: **one search field and the
list of matches**, not a region-then-city two-step (two lists cost twice the widgets and
twice the clicks, and nobody knows whether Istanbul is `Europe` or `Asia`). The list is the
Agents pane's pattern (`agents-v0.3.md` section 2.1): 32 empty static rows under three
labels, so 419 zones cost 4 widgets. The pane shows the setting's zone, never `TZ` (a
program's override), and `UTC` when the setting is missing or names no zone.

### 8.1 How the zone is shown

- **`Now` (widget 3) is local time**: `<Weekday> <D> <Month> <YYYY>, <HH>:<MM>:<SS> <ABBR>`,
  the zone's abbreviation where `UTC` stood: `Thursday 29 October 2026, 15:05:09 CET`. When
  the abbreviation is numeric (it starts with `+` or `-`: `-03`, `+0530`, 167 zones) the
  offset stands in for it, `UTC-03:00`, so the line never ends in a bare `-03`. With the zone
  `UTC` the line is exactly section 3's. Widest: `Wednesday 30 September 2026, 23:59:59
  UTC-03:00`, 48 glyphs, 384 px, starting at x 64 of the pane: never meets `Now`.
- **The zone row** (widgets 8 to 10, under the status line): `Time zone` at the left and, right-
  aligned, two lines 20 apart as About's: the name in `text`, `Europe/Paris` (the IANA name
  as `settings get time.zone` prints it, `_` kept: it is the value, and a person who typed it
  on the CLI must recognise it), and the facts in `text.dim`:

  | Zone | Facts line |
  |------|------------|
  | has daylight saving time, in force | `UTC+11:00 AEDT, daylight time` |
  | has it, not in force | `UTC+01:00 CET, standard time` |
  | has none | `UTC+05:30 IST` |
  | numeric abbreviation | `UTC-03:00` (`UTC-02:00, standard time` for Nuuk) |
  | `UTC` | `UTC+00:00` |

  `daylight time` and `standard time` are the words of the zone data (`Local::daylight`) and
  of Windows (`Adjust for daylight saving time`); `summer time` is British usage, and for
  Sydney or Santiago the daylight months straddle the new year, so a reader would ask which
  summer. The offset is always `UTC±HH:MM`, with the sign,
  the form Windows lists zones by and `date +%z` without the colon. Widest facts line:
  `UTC-03:30 NDT, daylight time`, 28 glyphs; an 8-byte abbreviation gives 33, 264 px,
  starting at x 184; the name's 30 glyphs start at 208; `Time zone` ends at 88.

### 8.2 Layout (pane coordinates, added to section 2's table; nothing of 1 to 7 moves)

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 8 | label | (16, 180) | `Time zone` in `text` |
| 9 | label, right-aligned | ends at x 448, cell top 180 | the zone's name, `Europe/Paris`, in `text` |
| 10 | label, right-aligned | ends at x 448, cell top 200 | the facts line (8.1) in `text.dim` |
| 11 | text field | (16, 224), 304x28 | placeholder `City, region or abbreviation`, `escape: Clear`; empty when the pane is shown |
| 12 | label, right-aligned | ends at x 448, cell top 230 | the match count (8.3) in `text.dim`; hidden while the field is empty or nothing matches |
| 13 | list, 3 rows of 24, mark x 8, text x 24 | (16, 260), 432x72 | 32 static rows whose text is empty; `set_shown_items` to the rows shown; empty text `No zone matches` |
| 14-16 | labels | (40, 264), (40, 288), (40, 312) | the three visible rows' texts (8.3) in `text`; retitled on every scroll and filter change |

The section starts 8 px under the status line's cell (172), the field 8 px under the facts
line's cell (216), the list 8 px under the field (252), and the list ends at 332, the pane's
height at the minimum window, as the Appearance pane's no-disk line does: nothing scrolls at
the minimum window and nothing moves at a taller one. The count label has at least 56 px of
room (field end 320, widest count `32 of 419` 72 px). Labels 14-16 are declared after the
list so they draw over its rows and take no pointer input, as the Agents pane's.

### 8.3 The list and the search

- **Row text**: `<name> · UTC±HH:MM <ABBR>` at the current instant, the abbreviation dropped
  when numeric: `Europe/Paris · UTC+01:00 CET`, `America/Argentina/Buenos_Aires · UTC-03:00`,
  `UTC · UTC+00:00`. Widest: 48 glyphs, 384 px, from x 40 to 424, inside the bar's 440. The
  offset is computed for the 32 rows shown, from each zone's rule at the current second, when
  the rows are filled (a rule parses in a few hundred cycles; the list is not re-ticked).
- **The first row is always the chosen zone**, selected (the `>` mark, toolkit section 6), so
  the mark never lies and the list reads as a closed dropdown while the field is empty: one
  row, the zone in force. The matches follow it in `ZONES` order (alphabetical), the chosen
  zone skipped among them, at most 31 of them (the mask's 32). When a query matches nothing
  the list is empty and shows `No zone matches`; the setting is untouched.
- **Matching**, after trimming: case-insensitive substring of the name with `_` and `/` read
  as spaces (`buenos aires`, `new york`, `argentina buenos` match), or the query equal to a
  zone's current abbreviation, case-insensitive (`ist` finds Kolkata and Jerusalem by their
  abbreviation and Boa_Vista, Istanbul and Christmas by name; `cest` finds nothing on 29
  October, when Paris is on CET). Not by offset or country in 0.3.0 (`zone.tab`'s country
  column is not in the table).
- **Count** (widget 12): `1 zone`, `2 zones`, or `32 of 144` when more match than the 31
  rows after the chosen one hold (`<shown> of <matching>`, the Activity view's form). Hidden
  with an empty field or no match. The user types one more letter; that is the whole
  remedy, as in every zone search.
- **Filtering** happens on every edit (`Response::Edited`), in the same frame: rows refilled,
  mask set, offset 0, labels retitled, count retitled. Esc clears the field (its `escape`),
  which puts the chosen zone's single row back.

### 8.4 Choosing

The list's selection is the choice, as the Keyboard pane's layouts list and the sidebar: a
click on a row, or an arrow step onto it, writes `time.zone` at once (`Response::Chosen`
with the slot, mapped to the zone's index, then `save_time_zone`), and in the same frame
`Now` (widget 3), the name (9) and the facts (10) show the new zone. A double-click or
Enter on a row (`Opened`) does nothing more. The rows stay as they are until the next edit
or the next showing of the pane, when the chosen zone is pinned first again: a row that was
chosen does not jump. Writing per arrow step is one small file each (the settings file,
through its temporary file) and is undone by one step back; the `Now` line changing under
the arrows is the feedback macOS gives when a city is clicked.

- **A failed write** chooses nothing: the selection goes back to the chosen zone's slot
  (always shown, 8.3), the labels stay, and the status line (widget 7) says why: `EPERM` or
  `EACCES` → `Not allowed: this user may not set the time zone.`; any other errno (`ENOENT`,
  `EROFS`, `ENOSPC`: no disk) → `No disk attached: the time zone could not be saved.`, both
  in `text`. A successful choice leaves the status line as it is: the three lines that
  changed are the result. Showing the pane again restores the "shown" line as section 5 says.
- **Other programs** read the zone from the settings file when they show a time (`date`,
  `ls -l`); the top bar's clock (section 6) now shows the zone's local `HH:MM`, and the shell
  re-reads the zone at the minute tick that redraws it (a one-line file), so the bar follows
  within a minute without any new notification.
- **CLI and MCP**: `settings set time.zone <name>` and `settings_set` are the same write;
  `settings get time.zone` the same name (D22). Nothing new is added to them.

### 8.5 Setting the clock in the chosen zone (recommended: yes)

`Set to` takes the typed time **in the chosen zone's local time**, because a person sets a
clock from their watch, the wall clock or a phone, all of which show local time, and the pane's
own `Now` line is local now; `date -s` does the same on Linux and macOS. The parser is still
the one of section 4, then the local moment goes through `Rule::to_utc` (the earlier instant
when the clocks went back, the time after the gap when they went forward). The time-only
forms keep the current **local** date. A trailing `Z` or ` UTC`, accepted and ignored until
now, now means UTC, so a line from `date -u` still pastes back exactly. The year rule (2000 to
2099) applies to the UTC instant, the one the hardware clocks store: `2099-12-31 23:30` in a
UTC-05:00 zone is refused with section 5's year line. `date -s` and `clock_set` take the same
local reading from the same code (D22; `clock_set` keeps a `utc` boolean, default false, for
agents that have an instant). The placeholder stays `YYYY-MM-DD HH:MM`; the status lines of
section 5 are unchanged, and `Now` shows the set time in the zone in the same frame.

### 8.6 Keys, focus and pointer

- Tab from the sidebar list: the `Set to` field, `Set`, the zone field, the list, then wraps
  to the search field; Shift+Tab the reverse. Showing the pane does not move the focus.
- In the zone field: typing filters; **Enter and Down move the focus to the list** (as Down in
  the sidebar's search does) and never reach `Set`, the window's default button, so Enter in
  the zone field can never set the clock; Esc clears the field (a second Esc goes to the
  window, which ignores it).
- In the list: Up/Down, Home/End move the selection and choose per step (8.4); the wheel and
  the bar scroll in whole rows and retitle labels 14-16 with rows `offset .. offset + 3`;
  Enter and Space do nothing more. Up on the first row and Down on the last stay (no wrap).
- Pointer: `IBeam` over the two fields, `Arrow` elsewhere; a click on a row's text reaches
  the list through the label; a right-click does nothing.

### 8.7 Every device class (D23)

- **One column** when the pane is narrower than 448: the name and the facts go under `Time
  zone` at x 16 (cells 200 and 220, left-aligned), the field at (16, 244), the width minus 32,
  the count label hidden (the list's own rows say what matched; the count is a desktop
  nicety), the list at (16, 280), the width minus 32, three rows; the pane grows past the
  minimum height and scrolls in the window's scroll view, the focused widget kept in view.
- **Touch**: list rows 40 tall (text cell top +12), the list 120 tall; the field 28 as
  everywhere; a tap on a row chooses it.
- **TV (scale 2)**: everything doubles; the pane works by arrows, Tab and Enter: Tab to the
  field, type with the remote's keyboard, Down into the list, Down to the zone.
- **Keyboard-only** on every form factor: the field and the list are two Tab stops; a zone
  is reached by typing three letters and one or two Downs.
- **Headless**: no pane; `settings set time.zone` and `settings_set` are the whole capability.

### 8.8 What an agent sees (D161, `wm show`)

The list is a `list` named `Time zones`, its rows `list item`s named by their row text
(`Europe/Paris · UTC+01:00 CET`) with the value the IANA name (`Europe/Paris`), the chosen one
`selected`; the field a `text field` named `City, region or abbreviation`; `Time zone`, the
name, the facts and the count `label`s with their drawn text. None is person-only: an agent
with `write` may set `time.zone` through `settings_set` already, and the pane is the same
write (`SetText` on the field, then `Select` on a row). Agents that want the zone call
`settings_get`, not the tree.

### 8.9 Search words and budget

- **Search words** added to section 1's: `zone`, `daylight`, `DST`, `summer time`, `offset`,
  `city`, `IANA`, `Europe/Paris`, plus the setting name `Time zone`. `city` collides with
  nothing; `zone` matches only this pane.
- **Budget**: `CAPACITY` is 140 today. This section adds exactly **9 widgets** (8 to 16: five
  labels, one field, one list, three row labels), so `CAPACITY` becomes **149**, and the
  installer's `eggos_app::dialog::CAPACITY` stays equal to it. The pane is its container plus
  16 widgets.

### 8.10 Acceptance (host tests, default 640x440, scale 1x; facts as section 7's with
`time_zone` set per case; realtime 1_793_282_709 unless said)

- `CAPACITY` is 149 and the tree holds 149. In window coordinates: `Time zone` at (192, 180);
  the name's right edge at 624, cell top 180; the facts' right edge at 624, cell top 200; the
  field (192, 224, 304, 28) with placeholder `City, region or abbreviation`, empty when the
  pane is shown; the count's right edge at 624, cell top 230, hidden; the list
  (192, 260, 432, 72) with one row shown, selected, its label at (216, 264) reading
  `UTC · UTC+00:00` with the zone `UTC`.
- With `Europe/Paris`: `Now` reads `Thursday 29 October 2026, 15:05:09 CET`, the name
  `Europe/Paris`, the facts `UTC+01:00 CET, standard time`. With `Australia/Sydney`: `Friday
  30 October 2026, 01:05:09 AEDT`, facts `UTC+11:00 AEDT, daylight time`. With `Asia/Kolkata`:
  `..., 19:35:09 IST`, facts `UTC+05:30 IST`. With `America/Argentina/Buenos_Aires`: `...,
  11:05:09 UTC-03:00`, facts `UTC-03:00`, the row `America/Argentina/Buenos_Aires ·
  UTC-03:00`. With `America/Nuuk`: facts `UTC-02:00, standard time`. With `UTC`: section 7's
  lines unchanged.
- Typing `par` with `UTC` chosen shows three rows: `UTC · UTC+00:00` (selected),
  `America/Paramaribo · UTC-03:00`, `Europe/Paris · UTC+01:00 CET`, and the count `2 zones`;
  `buenos aires` shows `UTC` then `America/Argentina/Buenos_Aires · UTC-03:00`, count
  `1 zone`; `ist` shows `UTC` then Boa_Vista, Jerusalem, Kolkata, Istanbul, Christmas in that
  order, count `5 zones`; `america` shows `UTC` and 31 rows from `America/Adak`, count
  `32 of 144`; `xyz` shows no row, `No zone matches`, no count; Esc empties the field and
  shows the `UTC` row alone.
- With `par` typed, Down from the field focuses the list on `UTC`; Down writes
  `time.zone=America/Paramaribo` (one `save_time_zone(index)` with Paramaribo's index) and
  `Now` reads `..., 11:05:09 UTC-03:00`; Down again writes `Europe/Paris` and `Now` reads
  `..., 15:05:09 CET`; the rows do not move; Up writes Paramaribo again. A click on the third
  row's label at (216, 312) writes `Europe/Paris`. Enter in the zone field with `2026-10-29
  14:05` in `Set to` sets nothing. Showing another pane and coming back shows the empty field
  and the chosen zone's single row.
- A write answered `EPERM` leaves the setting, puts the selection back on the chosen zone's
  row and shows `Not allowed: this user may not set the time zone.`; one answered `ENOENT`
  shows `No disk attached: the time zone could not be saved.`; a successful write leaves the
  status line as it was.
- With `Europe/Paris` chosen, typing `2026-10-29 15:05` and Enter in `Set to` requests
  1_793_282_700 (14:05 UTC); `2026-10-29 15:05 UTC` requests 1_793_286_300; `15:05:30`
  requests 1_793_282_730; `2026-03-29 02:30` (in the gap) requests the instant of 03:30 CEST,
  1_774_747_800; `2099-12-31 23:30` in `America/New_York` shows the year line.
- `settings set time.zone Europe/Paris` on the CLI, then showing the pane, shows the Paris
  lines above; `settings get time.zone` after a click on the Paris row prints `Europe/Paris`.
- The bar: with `Europe/Paris` and realtime 1_793_282_709 the clock reads `15:05`.

### Left out of 0.3.0

Search by country or by offset (`zone.tab`'s country column, and `UTC+5:30` as a query);
`Set automatically` from the network (NTP and location, with the Network pane); the next
daylight change (`Clocks go back on 25 October`); a map; the zone's full English name
(`Central European Time`: not in the data, and the abbreviation plus the offset says it).
