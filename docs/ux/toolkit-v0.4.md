# EggOS toolkit v0.4, slice 1: slider, dropdown, text selection and the clipboard

Design by Claude Fable (D16) for the first slice of the toolkit's next widgets. It extends
`toolkit-v0.3.md`: every token, metric, state rule and the focus and pointer rules of its
sections 1 to 9 stay, and this brief only adds the three things section 10 listed first:
a **slider** (`Kind::Slider`), a **dropdown** (`Kind::Dropdown`) and **selection and the
clipboard in the single-line text field**. Every number is in px at scale 1x and doubles at
2x; colours are the existing tokens (no new token: section 1). Everything is drawn by our own
Rust code on the software framebuffer: 1 px strokes, no anti-aliasing, no alpha, no motion
except the caret's blink. The widgets fit the code's model: a `Kind` variant held by value in
the retained tree (no heap), parts numbered from 0 for the element tree, a `Response` the
program acts on, and `Look` for the state the tree draws.

## 0. What a person gets, and how that is checked

- A **slider** replaces the stepper of `displays-mouse-v0.3.md` section 5 (`Pointer speed`,
  `Repeat delay`, `Repeat rate`): drag, click, arrows, Home/End and PageUp/PageDown set a
  number in a range, the value is drawn next to the track, and the setting applies while the
  thumb moves (section 2.5), so a person feels the pointer speed before letting go.
- A **dropdown** shows the chosen option in a 28 px button and opens a list of up to eight
  rows to choose another; it is what a choice of five or more options needs (a radio group
  holds four; a list of fourteen keyboard layouts is 336 px tall).
- In a **text field** Shift+arrows, a drag, a double-click and Ctrl/Cmd+A select text, the
  selection is drawn in `selection`, typing replaces it, and Ctrl/Cmd+X, C and V go through
  the clipboard every app shares (D197), so a path copied in the Text Editor pastes into
  Files' path field and a word typed in Settings' search can be copied out.

Host tests (section 8) pin the pixels and the responses; `wm show` (section 6) lists each new
widget with its value so an agent uses it without the screen.

## 1. Tokens and constants

**No new colour token.** The toolkit has one theme (the light theme is a later slice; when it
comes it gives every token here its light value, as it will every other token). Used, by
name: `accent` `#F2B33D`, `button.primary` and its `.hover` `#F6C25C` and `.pressed`
`#D99E2E`, `progress.track` `#26303F`, `button.bg` `#2A3440`, `button.hover` `#344050`,
`button.pressed` `#202833`, `button.border` `#3A4454`, `menu.bg` `#1C2430`, `menu.border`
`#2A3340`, `menu.hover` `#2A3440`, `selection` `#2E4A6B`, `focus.ring` `#F2B33D`, `text`
`#E6EAF0`, `text.dim` `#8A94A6`, `icon.dark` `#0B0E13`, `bar.pressed` and `button.inactive`
(the scroll bar's thumb). Contrast of every pair used is already fixed in `toolkit-v0.3.md`
section 1 (`text` on `selection` 7.5:1 is the selected text; `text` on `menu.hover` 10.5:1
the highlighted row).

One new shape constant in `canvas`: `INSET_R2 = [1, 0]` (a 4 px tall bar with rounded ends,
mirrored like `INSET_R4`), for the slider's track. New metric constants in `widget`, all at
1x: `SLIDER_TRACK_HEIGHT 4`, `SLIDER_THUMB 16`, `SLIDER_MIN_TRACK 128`, `SLIDER_READOUT 48`,
`SLIDER_GAP 8`, `SLIDER_PAGE_STEPS 10`; `DROPDOWN_TEXT_X 12`, `DROPDOWN_ARROW_ROOM 32`,
`POPUP_ROW_HEIGHT 32` (equal to `menu::ITEM_HEIGHT`), `POPUP_ROWS 8`, `POPUP_GAP 4`,
`POPUP_OUTSET 12`, `POPUP_MARK_X 8`, `POPUP_TEXT_X 24`, `POPUP_WHEEL_ROWS 2`. Key codes added
to `input::key`: `A 30`, `X 45`, `C 46`, `V 47` (Linux codes; the shortcuts are by key
position, as the window server's Ctrl+W and Cmd+W are).

## 2. Slider

### 2.1 Kind

```
Kind::Slider(Slider)
pub struct Slider {
    pub name: &'static str,
    pub value: i32,
    pub min: i32,
    pub max: i32,
    pub step: i32,
    pub unit: &'static str,
    pub readout: bool,
}
Kind::slider(name, min, max, step, value, unit) -> Kind   (readout: true)
```

`step` is at least 1; the reachable values are `min + k * step`; the constructor lowers `max`
to the last reachable value (`min + ((max - min) / step) * step`) and clamps and snaps `value`
to one, so the widget never shows a number the range cannot reach. `unit` is drawn after the
digits as given (`""`, `" ms"`, `"/s"`, `" %"`); `readout: false` draws no number (a slider
whose effect is seen or heard at once, with no setting that must show exactly). `name` is the
element's name for an agent (section 6): the words the label before it shows (`Pointer
speed`); the widget draws no name. `WidgetTree::value(id) -> Option<i32>` and
`set_value(id, value)` (snapped and clamped, damaging the control, no response) are how the
program shows a value set from the CLI, as the stepper's `-`/`+` panes do when focused.

### 2.2 Geometry at 1x

- The control is **28 tall** (`CONTROL_HEIGHT`), so it sits in the rows the stepper and the
  buttons use; `Length::Content` is `(SLIDER_MIN_TRACK + 56, 28)` with a readout and
  `(SLIDER_MIN_TRACK, 28)` without. The program usually gives it a width (`Length::Px(216)`
  in Settings, `Fill` in a one-column pane).
- **Readout** (when `readout`): the rightmost 48 px of the control, text right-aligned to the
  control's right edge, cell top at +6, `text`; the digits are tabular so values of equal
  length align, and `2000 ms` (40 px) fits. An 8 px gap separates it from the track.
- **Track rect**: the control minus the readout and its gap: `x .. right - 56` (or the whole
  width without a readout). The **track bar** is 4 tall at `y + 12 .. y + 16`, `INSET_R2`
  ends, spanning the track rect's full width: `progress.track`, and from the track's left
  edge to the thumb's centre `accent` (the filled part, as the progress bar's done part).
- **Thumb**: a 16 px circle (`INSET_R8`), top at `y + 6`, centred at
  `thumb_x = track.x + 8 + (value - min) * travel / (max - min)` with
  `travel = track.width - 16` (integer division, so `min` lands at `track.x + 8` and `max` at
  `track.right - 8` exactly). A range of one value (`min == max`) draws the thumb at the left
  and the program disables the control.
- **Hit areas**: the thumb's is `thumb_x - 8 .. thumb_x + 8` over the control's full height;
  everything else in the control is the track's. Pointer `Arrow` everywhere.

In Settings' Mouse pane the stepper (widgets 7 to 9, `-` at (328, 150), the value in
(364, 156, 48, 16), `+` at (420, 150)) becomes one slider at (232, 150), 216x28: track
x 232..392 (travel 144, so 1 to 10 is 16 px a step), readout 400..448, right edge 448 as
before; the Keyboard pane's two steppers likewise (`Repeat delay` 100 to 2000 by 50 is 38 steps
over 144 px, so a click snaps and the keys are exact). Three widgets become one per stepper.

### 2.3 States

| State | Track (rest / filled) | Thumb | Readout |
|-------|------------------------|-------|---------|
| normal | `progress.track` / `accent` | `button.primary`, no outline | `text` |
| hover (pointer in the thumb's hit area) | unchanged | `button.primary.hover` | `text` |
| pressed or dragging | unchanged | `button.primary.pressed` | `text` |
| focused | as its other state | as its other state, plus the ring | `text` |
| disabled | `progress.track` / `progress.track` | `button.bg` fill, 1 px `text.dim` outline (`INSET_R8`) | `text.dim` |

The **ring** is the checkbox's: a 26x26 rect centred on the thumb (`thumb_x - 13 ..
thumb_x + 13`, `y + 1 .. y + 27`), the 2 px `focus.ring` `INSET_R4` outline 2 px inside it,
so it sits 1 px outside the circle; it moves with the thumb. Hover over the track (not the
thumb) changes nothing: the track is a target to click, not a control to light up.

### 2.4 Pointer, keys, wheel

- **A press on the track jumps** the value to the pressed point and starts a drag from there
  (macOS, GTK, Android and the web's range input do this; Windows pages towards the click,
  the one habit left out: tapping where you want the value is what a touch screen needs, D23).
  **A press on the thumb** starts a drag keeping the grab offset (`grab = pointer x -
  thumb_x`), so the thumb does not jump under the finger.
- **Dragging**: on every `PointerMove` while held, `value = snap(pointer x - grab)`, where
  `snap(px)` is `min + round((px - track.x - 8) * (max - min) / travel / step) * step`
  clamped to `min ..= max` (round half up, in integers:
  `(dx * (max - min) * 2 + travel) / (2 * travel)` steps before snapping to `step`); the
  pointer may leave the control and the window (the server holds the pointer while a button
  is down). The thumb shows the pressed look throughout. Release ends the drag wherever it
  happens; nothing more is sent.
- **Keys** (focused): Right and Up `+step`, Left and Down `-step`, Home `min`, End `max`,
  PageUp `+10 * step`, PageDown `-10 * step` (`SLIDER_PAGE_STEPS`; for 1 to 10 the page is the
  end, for 100 to 2000 by 50 it is 500); all clamped. Enter goes to the default button, Space
  does nothing, Tab leaves. No key repeat in the toolkit (as the stepper), so holding a key
  steps once.
- **Wheel: nothing.** The stepper ignored it and the slider does too: panes scroll in a scroll
  view on a phone, and a wheel that happens to pass over a slider must not change a setting.
  The wheel goes on to the scroll view beneath.
- **Disabled**: ignores the pointer and keys, skipped by Tab.

### 2.5 Response: `Response::Adjusted(WidgetId, i32)` on every change, dragging included

Whenever the value changes, by a key, a click, a drag step or `wm act`, the tree returns
`Response::Adjusted(id, value)`, once per new value (a move within the same step returns
`Nothing`); no release event. Reasons: the settings these sliders set apply live (pointer
speed is felt while dragging; macOS applies tracking speed and volume live, so does GNOME),
and a slider that only speaks on release feels dead. The cost is bounded by the steps, not the
pixels: a drag across `Pointer speed` is at most 9 writes, across `Repeat delay` 38, and the
stepper already wrote once a press. A program whose apply is expensive coalesces with its own
`deadline`/`tick` (apply at once, save after a quiet 250 ms); the toolkit does not do that for
it. `set_value` sends nothing (the program set it).

## 3. Dropdown

### 3.1 Kind

```
Kind::Dropdown(Dropdown)
pub struct Dropdown {
    pub name: &'static str,
    pub options: &'static [&'static str],
    pub selected: usize,
    pub open: bool,
    pub highlighted: usize,
    pub offset: i32,
}
Kind::dropdown(name, options, selected) -> Kind
```

At most `MAX_ROWS` (32) options, as a list; `selected` is always an option (a dropdown is
never empty: a choice with nothing to choose is a label). `highlighted` is the row the keys
and the pointer move in the open popup; `offset` the popup's scroll in px. `WidgetTree::
selected(id)` and `set_selected(id, index)` work on it as on a radio group (`set_selected`
damages the button, sends nothing, and closes the popup if open). `name` is the element's
name (section 6), the words of the label before it (`Layout`).

### 3.2 Geometry at 1x

**The closed button**: 28 tall, `INSET_R4` corners, 1 px `button.border` inside the edge,
the chosen option's text at `x + 12` (`DROPDOWN_TEXT_X`), cell top +6, in the space
`x + 12 .. right - 32`, end-ellipsised; the **chevron** in the rightmost 32 px: nine 2x2
squares at `(i, |i - 4|)` for `i` in 0..9, a down-pointing `v` 10 wide and 6 tall, its
top-left at `(right - 22, y + 11)`, drawn in the text's colour (it does not flip while open).
`Length::Content` is `(12 + the widest option + 32, 28)`, at least 96 wide
(`BUTTON_MIN_WIDTH`).

**The popup**: `width = button.width + 24`, `x = button.x - 12` (`POPUP_OUTSET`), so a row's
text at `popup.x + 24` (`POPUP_TEXT_X`) is exactly over the button's text at `button.x + 12`:
the chosen word does not shift when the list opens (macOS aligns its pop-up menus the same
way). `rows = min(options, 8)` (`POPUP_ROWS`); `height = 2 + rows * 32`. Placement, in
window content coordinates (the tree draws in one window, so the popup never leaves it):
**below** at `y = button.bottom + 4` when `y + height <= window height`; else **above** at
`y = button.y - 4 - height` when that is `>= 0`; else below with `rows` cut to what fits
(`max(1, (window height - button.bottom - 6) / 32)`), scrolling. Horizontally `x` is clamped
to `0 .. window width - width`, and `width` to the window's. A dropdown inside a scroll view
places its popup from its on-screen rect.

Drawn: `menu.bg` fill, 1 px `menu.border` outline, `INSET_R6` corners, exactly as the
context menu (`menu.rs`) draws itself; rows inside the border, each `32` tall, `width - 2`
wide (`width - 10` when the scroll bar shows), the text at `popup.x + 24`, cell top `row.y +
8`, ellipsised at `popup.right - 9` (or `- 17` with the bar); the chosen option's `>` in
`accent` at `popup.x + 8` (`POPUP_MARK_X`), the non-colour cue of every selectable list
(toolkit section 6); the highlighted row filled `menu.hover` under its text, as the menu
fills its hovered item (the two compose: the chosen row highlighted is `menu.hover` with the
`>`). With more than 8 options the scroll bar of toolkit section 8 sits in the inner rect's
right 8 px; rows reserve the 8 px from the first row on. The popup is **drawn last**, after
every widget, so nothing covers it; its rect is clipped to the window content. No shadow (no
alpha in the framebuffer), no animation.

### 3.3 States

| State | Button fill | Border | Text and chevron |
|-------|-------------|--------|------------------|
| closed, normal | `button.bg` | `button.border` | `text` |
| closed, hover | `button.hover` | `button.border` | `text` |
| pressed (held, popup not yet open or just opened) and **open** | `button.pressed` | `button.border` | `text` |
| focused (closed or open) | as its other state | as its other state | `text`, plus the ring (2 px inside the button's rect) |
| disabled | `button.bg` | `button.border` | `text.dim` |

The button stays `button.pressed` for as long as the popup is open (macOS highlights the
button while its menu is up), and the ring stays on the button: the focus never moves into
the popup, which is part of the same widget. Rows: the highlighted one `menu.hover`, the
rest `menu.bg`; text `text`; no disabled rows in this slice (the program leaves an option
out rather than greying it).

### 3.4 Pointer, keys, wheel

- **Open**: a press on the closed button opens the popup at once with `highlighted =
  selected`, scrolled so the selected row is visible (the first page when it is on it). The
  release that follows, over the button, leaves it open; a release **over a row** chooses that
  row (press, slide, let go: the macOS habit, free because the tree already tracks the press).
- **In the popup**: the pointer over a row highlights it (hover and highlight are one thing,
  one row at a time); a click on a row (press and release inside it) chooses it; the scroll bar
  works as section 8; a wheel over the popup scrolls it 2 rows a notch (`POPUP_WHEEL_ROWS`,
  the whole rows nearest `WHEEL_STEP`).
- **Close without choosing**: Esc; Tab or Shift+Tab (the focus moves on); a press outside
  the popup and the button (consumed: it reaches no widget beneath, as on macOS, so a
  mis-click closes the list and does nothing else); a wheel outside the popup (consumed too,
  so the popup never floats away from its button while a pane scrolls); the window losing
  focus; the dropdown being disabled, hidden or re-laid out by a resize.
- **Keys, open**: Up/Down move the highlight one row, not wrapping; Home/End to the first and
  last; PageUp/PageDown by `rows - 1`; the view scrolls to keep the highlight visible; Enter or
  Space chooses the highlight and closes; **a letter or digit** jumps to the next option
  starting with it, case-insensitively, searching from the row after the highlight and
  cycling (as every desktop's lists do); Esc closes.
- **Keys, closed and focused**: Space, Enter, Alt+Down and Alt+Up open; Up/Down choose the
  previous or next option at once without opening, not wrapping (the toolkit's radio group
  already chooses on arrows, and Windows combos do the same); Home/End choose the first and
  last; a letter chooses the next option starting with it, cycling. Enter on a closed dropdown
  opens it rather than reaching the default button (a dialog's dropdown would otherwise be
  unopenable by keyboard without Space).
- **Disabled**: ignores the pointer and keys, skipped by Tab.

### 3.5 Response

Choosing an option that differs from `selected` returns `Response::Chosen(id, index)` (the
existing variant: a dropdown is a radio group drawn small) and closes the popup, the button
retitled in the same frame; choosing the chosen option closes with `Nothing`. Opening and
closing return `Nothing`. The program never needs to know the popup exists.

## 4. Text selection and the clipboard in the single-line text field

### 4.1 State

`Field` gains `pub anchor: usize`: the selection is the bytes between `anchor` and `caret`,
in either order, and there is none when they are equal. Every existing caret move
(Left, Right, Home, End, a click) sets `anchor = caret` (collapses). Shift with any of them
moves the caret and leaves the anchor (extends). The selection is kept while the field is
unfocused and shown again on focus, but **drawn only while the field is focused and its
window has focus**, like the caret.

### 4.2 Drawing

The selected glyphs' cells are filled `selection`: the rect from the x of the selection's
first boundary to the x of its last (advance widths, so the last glyph's spacing column is
included, as toolkit section 2 measures text), the cell's full 16 rows at the text's top
(`y + 6`), clipped to the inner rect (`x + 8 .. right - 8`) and shifted by the scroll offset;
the text over it stays `text` (7.5:1); the caret is drawn on top at its end. The placeholder
is never selected (it is not text).

### 4.3 Keys and pointer

| Input | Effect |
|-------|--------|
| Shift+Left, Shift+Right, Shift+Home, Shift+End | move the caret, keep the anchor: extend or shrink the selection |
| Left or Right with a selection, no Shift | collapse to the selection's start or end (no move past it), as on every desktop |
| Mod+A | select all (`anchor 0`, `caret len`) |
| a printable character | replaces the selection, then inserts, caret after it |
| Backspace, Delete | with a selection, remove it (one step); without, as before |
| Esc `Clear` | empties text and selection; `Revert` restores the text, caret at its end, no selection |
| Tab into the field (keyboard focus) | selects all, caret at the end (what a Tab into a field does on macOS and Windows; typing replaces) |
| click | focuses, caret at the nearest boundary, collapses |
| Shift+click | extends from the anchor to the nearest boundary |
| drag from a press | the caret follows the pointer, the press point is the anchor; while the pointer is past the inner rect's left or right edge, each further `PointerMove` moves the caret one more character beyond the edge and the field scrolls to show it |
| double-click | selects the word under the pointer: a maximal run of ASCII letters, digits and `_`, or of characters beyond ASCII; any other non-space character is a word of its own; a run of spaces is what a double-click on a space selects (the Text Editor's rule); anchor at its start, caret at its end |
| triple-click | selects all |
| Mod+C | `Response::Copied(id)` when there is a selection, else `Nothing` |
| Mod+X | removes the selection and returns `Response::Cut(id)`; `Nothing` without one |
| Mod+V | `Response::PasteWanted(id)` |

Clicks count against the same field within `DOUBLE_CLICK_MS` (400 ms) of the previous one:
`last_click` gains a count (1, 2, 3; a fourth starts over at 1). The pointer stays `IBeam`.

**`Mod` is Ctrl or Super, in every profile**, as the Text Editor and Files decided (`Mod is
Ctrl or Cmd (the Super key); both are accepted in every profile`): a macOS-profile user presses
Cmd, a Windows- or Linux-profile user Ctrl, and both work everywhere, so the tree needs no
profile and the application passes none (`WindowServer::desktop_profile()` exists for a
program that wants to show `Cmd` or `Ctrl` in a hint; the toolkit does not read it). Alt
alone is not `Mod`: Alt+A is unused and goes to the window. For this the tree hands the
field the `Modifiers` rather than today's one `command` flag; a letter with Ctrl or Super
held is a shortcut or unused, never typed, as today.

### 4.4 How the clipboard is reached: the tree asks, the application layer answers

The tree has no window server, so it never copies or pastes itself; it hands text out and asks
for text in, through three new responses and two methods:

- `Response::Copied(WidgetId)`: the field put its selection in `WidgetTree::copied() ->
  &str` (a `Text<255>` the tree keeps, the last copy); the program gives it to the shared
  clipboard: `server.set_clipboard(tree.copied())`.
- `Response::Cut(WidgetId)`: the same, and the text changed (the program treats it as
  `Edited` too).
- `Response::PasteWanted(WidgetId)`: the program fetches the clipboard
  (`server.clipboard(0, &mut [u8; 255])`, the longest valid UTF-8 prefix of what fits) and
  calls `WidgetTree::paste(id, text, now_ms) -> Response`, which replaces the selection with
  `text` at the caret, keeping whole characters up to the 255-byte limit (as typing beyond it
  is ignored), with line breaks and tabs turned into one space each and other control
  characters dropped (a single-line field), and returns `Edited(id)` or `Nothing` (empty
  clipboard, nothing fits). The program feeds that into its own `respond`.

`application::run` (which owns the `Window`) answers all three itself before the program's
`respond` sees anything: `Copied` is answered and swallowed, `Cut` is answered and passed on
as `Edited(id)`, `PasteWanted` is answered and the `paste` result passed on. So an
`Application` keeps seeing only `Edited`; a program that drives a tree itself (a `View`) does
the same three things. Decided this way rather than giving the tree a server handle because
the tree is host-tested without one and because the Text Editor and the Windows layer already
treat the server's text as the thing every app syncs with (D197), not something a widget owns.

## 5. Focus, pointer and the tree

- Section 3's focusable list gains the slider and the dropdown: Tab stops on each, in
  declaration order; a click focuses either and shows the ring; disabled ones are skipped.
- `Kind::is_focusable` and `is_interactive` include both. Parts: a dropdown's rows are its
  parts 0..options (their element ids as a list's rows), its scroll bar thumb `THUMB`; a
  slider has no parts.
- The tree's hit test checks an open popup first (it is on top of everything): a point in it
  targets the dropdown's row or thumb; a press anywhere else while a popup is open closes it
  and is consumed.
- The tree's `Drag` learns an x (`grab_x`) for the slider; `pressed`/`pressed_inside` work as
  for a button on the dropdown's closed button and rows.
- The field's key path takes `Modifiers`; `field_key` grows the selection cases; `FieldKey`
  gains `Copy`, `Cut` and `Paste` so `tree::field_key` maps them to the three responses.

## 6. What an agent sees (D161, `wm show`)

Two roles join `eggos_element_tree::Role` (and docs/windows.md's list): **13 `slider`** and
**14 `combo-box`** (the name agents know from UIA and ARIA, where a `<select>` is a combobox;
the toolkit's `Kind` says `Dropdown` because that is what the thing looks like, and the role
says what an agent expects to drive).

- A **slider**: role `slider`, name its `name` (`Pointer speed`), value
  `<value> <min>..<max> by <step>` (`5 1..10 by 1`; `400 100..2000 by 50`: the first token is
  the number, without the unit, which is presentation), flags `focusable`, `focused`,
  `disabled`. Actions: `set-text` with a decimal number sets it (snapped and clamped) and
  answers `Done` with `Adjusted(id, value)` (or `Nothing` when unchanged); a text that is not
  an integer answers `NotSupported`; `invoke` and `select` answer `NotSupported`; `focus` as
  every focusable. An agent reads the range and writes the exact value, never presses an
  arrow thirty times (D22).
- A **dropdown**: role `combo-box`, name its `name` (`Layout`), value the chosen option's
  text, flag `expanded` while the popup is open; below it every option as a `list-item` named
  by its text, the chosen one `selected`, listed whether or not the popup is open (so an agent
  chooses without opening, as it chooses a radio). Actions: `select` on an option chooses it
  (`Chosen` when it differs, the popup closed if open); `invoke` on an option the same;
  `invoke` on the box opens or closes the popup (`Done`, `Nothing`); `set-text` with an
  option's exact text chooses it, with other text `NotSupported`.
- A **text field** is unchanged: its value is its text; the selection is not exposed (later).

## 7. Damage and 2x

A slider change damages the whole control (thumb, filled track, readout and ring move
together). A dropdown's open and close damage the button and the popup's rect; a highlight
move damages the two rows; a popup scroll the popup. A selection change damages the field.
The popup's rect is damage like any other: the widgets under it repaint and the popup is
drawn over them last. Scale 2x doubles every number here: an 8 px track, a 32 px thumb, a
96 px readout, 64 px rows, 4x4 chevron squares, a 32-row selection fill; the fonts
pixel-double as before.

## 8. Acceptance (host tests, scale 1x; window content 320x300; "press" and "release" are
the left button; `s`, `d`, `f` are the widget ids)

Fixture A: slider `s` at (16, 16), `Length::Px(216)` x 28, name `Speed`, 1 to 10 by 1,
value 5, unit `""`. Track x 16..176, travel 144; readout 184..232; track bar y 28..32;
thumb for 5 centred at x 88, rect (80, 22, 16, 16); filled bar x 16..88 `accent`, 88..176
`progress.track`.

1. Paint: pixel (20, 30) `accent`, (150, 30) `progress.track`, (88, 30) `button.primary`,
   (88, 23) `button.primary`, (88, 21) not; `5` right-aligned ending at x 232, cell top 22;
   the ring absent until focus; after `set_focus(s)`, pixel (76, 22) `focus.ring` (the ring's
   outer edge at `thumb_x - 13 + 2 = 77`... exact: outer edge x 77, so (77, 24) `focus.ring`
   and (76, 24) not).
2. Press (150, 30) → `Adjusted(s, 9)`, thumb centre 152, pressed look at (152, 30); release
   (150, 30) → `Nothing`, hover look at (152, 30), then `PointerLeave` → normal.
3. Press (20, 30) → `Adjusted(s, 1)`; press at the thumb (24, 30) and move to (300, 30) →
   `Adjusted(s, 10)` (clamped); release at (300, 30) → `Nothing`, value 10.
4. `set_value(s, 5)`; press (88, 30) (grab 0), move (104, 30) → `Adjusted(s, 6)`; move
   (108, 30) → `Nothing` (5.25 rounds to 5 steps: 6); move (112, 30) → `Adjusted(s, 7)` (5.5
   rounds up); release → `Nothing`.
5. Press (92, 30) (grab +4), move (108, 30) → `Adjusted(s, 6)` (pointer 108 − grab 4 = 104).
6. Keys at value 5, focused: Right → `Adjusted(s, 6)`; Up → 7; Left → 6; Down → 5; End → 10;
   Right → `Nothing`; Home → 1; PageUp → 10 (1 + 10 clamped); PageDown → `Adjusted(s, 1)`
   (10 − 10 = 0 clamped to 1); Space → `Nothing`; Enter with a default button `b` →
   `Activated(b)`.
7. Wheel at (88, 30), 1 notch → `Nothing`, value unchanged; a scroll view containing the
   slider scrolls as if the wheel were over the view.
8. `set_enabled(s, false)`: press (150, 30) → `Nothing`; Tab from a button before it skips
   it; paint: (88, 30) `button.bg`, (88, 22) `text.dim` (the outline's top), readout
   `text.dim`, (20, 30) `progress.track`.
9. `Kind::slider("Delay", 100, 2000, 50, 2000, " ms")`, 216 wide at (16, 60): readout
   `2000 ms` ending at 232; `set-text "1234"` through `act` → value 1250 (snapped up: 1234 is
   22.68 steps, 23 → 1250), `Adjusted`; `set-text "x"` → `NotSupported`;
   `Kind::slider("Odd", 0, 95, 10, 95, "")` has `max` 90 and value 90.
10. `describe`: `slider` named `Speed`, value `5 1..10 by 1`, bounds (16, 16, 216, 28),
    `focusable`.

Fixture B: dropdown `d` at (16, 60), `Length::Px(160)` x 28, name `Layout`, options
`US`, `UK`, `German`, `Swedish`, selected 0. Button x 16..176, y 60..88; text `US` at
(28, 66); chevron top-left (154, 71). Popup below: (4, 92, 184, 130); rows y 93..125,
125..157, 157..189, 189..221; text at x 28; `>` at x 12.

11. Paint closed: (17, 74) `button.border`, (20, 74) `button.bg`, `US` from x 28 in `text`,
    chevron pixel (154, 71) `text`, (162, 71) `text`, (158, 75) `text`, (158, 71) `button.bg`.
    `describe`: `combo-box` named `Layout`, value `US`, four `list-item`s, the first
    `selected`, no `expanded`.
12. Press (100, 74) → `Nothing`, open, highlighted 0, button fill (20, 74) `button.pressed`;
    paint: (4, 100) `menu.border`, (10, 100) `menu.bg`, row 0 (28, 100) `menu.hover`, `>` at
    (12, 101) `accent`, `UK` at (28, 133); `describe` has `expanded`. Release (100, 74) →
    `Nothing`, still open.
13. Move (100, 170) → row 2 `menu.hover`, row 0 `menu.bg`; press and release (100, 170) →
    `Chosen(d, 2)`, closed, button text `German`, button fill normal (pointer over it: hover).
14. Press (100, 74), move (100, 140), release (100, 140) → `Chosen(d, 1)`, closed.
15. Open, then press (250, 250) (outside) → `Nothing`, closed, no widget beneath pressed
    (a button `b` at (200, 240) placed there gets no `Activated` on the release). Open, then
    wheel at (250, 250) → closed, `Nothing`.
16. Keys, focused, closed: Down → `Chosen(d, 1)`; Down, Down → 3; Down → `Nothing`; Home →
    `Chosen(d, 0)`; End → 3; `u` (code of U, character `u`) → `Chosen(d, 0)` (US, cycling
    from after Swedish); `u` → 1 (UK); `U` with Shift → 0.
17. Keys, focused, closed, selected 0: Space → open, highlighted 0, `Nothing`; Down → 1;
    PageDown → 3 (`rows - 1` = 3); Up → 2; Enter → `Chosen(d, 2)`, closed. Alt+Down → open;
    Esc → closed, `Nothing`, selected still 2. Open, `g` → highlighted 2 (German) when
    starting from 3; Tab → closed, focus on the next widget.
18. Placement: the same dropdown at (16, 240) opens above: popup (4, 106, 184, 130). In a
    window 320x150 with the dropdown at (16, 60): below does not fit (92 + 130 > 150), above
    does not (60 − 134 < 0), so below with `rows = (150 − 88 − 6) / 32 = 1`: popup
    (4, 92, 184, 34), scrolling, the scroll bar at x 171..179 inside it.
19. Twelve options, selected 10: opening shows rows 5..12 (offset 160, the selected row
    visible, last page), bar thumb present; wheel 1 notch up over the popup → offset 96;
    Home → highlighted 0, offset 0.
20. `set_enabled(d, false)` while open → closed; press → `Nothing`; paint text and chevron
    `text.dim`. `act(select, option 3)` → `Chosen(d, 3)`, `Done`; `act(set-text "German")`
    → `Chosen(d, 2)`; `act(set-text "French")` → `NotSupported`; `act(invoke, box)` → opens,
    `Done`, `Nothing`.

Fixture C: text field `f` at (16, 16), 200x28, `escape: Clear`, text `12345 6789` (digits
6 px, space 4: boundaries at x 24, 30, 36, 42, 48, 54, 58, 64, 70, 76, 82), focused by a click
at (120, 30) (caret 10, anchor 10). A second field `g` after it and a button `b` before it.

21. Shift+Left → caret 9, anchor 10, fill (76, 22, 6, 16) `selection`, `9` still `text` on
    it, caret at x 76; Shift+Home → fill 24..82; Left → caret 0, no fill; Shift+End → all
    selected; Right → caret 10, no selection. Mod+A (Ctrl, then Super) → anchor 0, caret 10.
    Alt+A → `Unused { code: A, .. }`, nothing selected.
22. Click (44, 30) → caret 3, anchor 3; Shift+click (70, 30) → caret 8, fill (42, 22, 28, 16).
    Typing `x` → text `123x89`, caret 4, `Edited(f)`, no fill. Mod+A, Backspace → empty,
    `Edited(f)`; placeholder shown, no fill.
23. Text `12345 6789`, click (44, 30), move (70, 30) with the button held → caret 8, fill
    42..70, `Nothing`; release → `Nothing`, selection kept. Move further to (260, 30) held in
    a field whose text is wider than the inner rect: each move past x 208 advances the caret
    one more character and scrolls.
24. Double-click (66, 30) (two presses within 400 ms) → anchor 6, caret 10, fill 58..82;
    triple-click → anchor 0, caret 10; double-click (56, 30) on the space → anchor 5,
    caret 6, fill 54..58. Double-click on `a_b` selects all three; on `a-b` at the `-` selects
    the `-` alone; on `ÀÉ` the two.
25. Selection 3..8: Mod+C → `Copied(f)`, `copied()` = `45 67`, text unchanged; Mod+X →
    `Cut(f)`, text `12389`, caret 3, `copied()` = `45 67`; Mod+C with no selection →
    `Nothing`; Mod+V → `PasteWanted(f)`; `paste(f, "ab\ncd", now)` at caret 3 →
    `123ab cd89`, caret 8, `Edited(f)`; `paste(f, "", now)` → `Nothing`; a paste of 300
    bytes into an empty field keeps the longest prefix of whole characters within 255;
    `paste` with a selection replaces it.
26. Tab from `b` into `f` → anchor 0, caret 10, fill 24..82; Tab on to `g` → `f` keeps anchor
    and caret but draws no fill; Shift+Tab back → fill 24..82 again (the kept selection,
    shown; a Tab-in does not reselect what is already all selected). `FocusOut` → no fill,
    no caret; `FocusIn` → both back.
27. Esc in `f` with text → text empty, selection gone; `Revert` field: text back, caret at its
    end, anchor equal.
28. `describe` for `f` is unchanged: `text-field` valued by its text.
29. `application::run` with a fake server: on `Copied` it calls `set_clipboard` with
    `copied()` and the program's `respond` is not called; on `Cut` it calls `set_clipboard`
    and `respond` gets `Edited(f)`; on `PasteWanted` it reads the clipboard and `respond`
    gets `Edited(f)` with the text pasted, or nothing when the clipboard is empty.

Pixel cases compare the painted framebuffer against the stated colours at the stated points;
response cases drive `handle` with `Input` records and compare the returned `Response`.

## 9. Later slices

Tooltips; menus and context menus as tree widgets (the `menu` module stays the shell's and
the apps' for now); tables with resizable columns; multi-line text and wrapping; drag and
drop; the light theme; word moves (Mod+arrows) and Shift+Mod+arrows in the field; a middle
ellipsis; the selection in the element tree; disabled rows in a dropdown; an editable combo
box (a field with a dropdown); a vertical slider and a range slider; key repeat in the
toolkit; horizontal scrolling and inertia; IME and dead keys in the field; a `Spinner` for
exact numbers (the stepper's buttons with a typed value); focus rings hidden in "mouse
mode"; the selection and copy in the terminal (its own brief).
