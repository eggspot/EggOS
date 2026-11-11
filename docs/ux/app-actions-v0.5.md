# EggOS v0.5 apps' own actions in the GUI: design brief

Design by Claude Fable (D16) for the GUI front-end D292 left as "Not yet". An installed app
declares actions in `app.actions` (`app-manifest/src/actions.rs`): a title, a description, an
effect (`read`, `change`, `destroy`) and up to eight arguments (`text`, `number`, `boolean`,
required or optional). Today they are `app actions`, `app do` and MCP tools; D22 asks for a GUI.
A person finds an app's actions **in the app's menu** (launcher row, desktop icon, Files), and
everything after the choice happens in **one small window of the `app` program**, started as
`app do --window <app> [<action>]`, the way `app uninstall --window` asks its question (D134).
The window runs the action through the same code as `app do`, so grants, the audit log and the
errors are the CLI's (D22: nothing here is GUI-only). Tokens, type and voice are
`desktop-v0.3-gui.md`'s and the toolkit's; numbers are px at 1x; no motion and no colour token
is added: a `destroy` action is told by words and by its place, never by colour.

## 1. Where a person finds them: the app's menu

The context menu of an installed app (`desktop-v0.3-gui.md` 6.4 item 4: right-click, Shift+F10
or the Menu key on a launcher row or a desktop icon) gains the app's actions, by title:

```
Open
---------------
New note...          actions whose effect is read or change, the file's order
Count notes
All actions...
---------------
Empty trash...       actions whose effect is destroy, then the app's own removal
Uninstall...
```

- **Which actions**: the first four the file offers. With no valid action the menu is today's
  (`Open`, `Uninstall...`, 160 wide) and nothing below applies.
- **Two groups.** `read` and `change` actions sit under `Open`; `destroy` actions sit in the
  last group with `Uninstall...`, so everything that deletes is at the bottom, below a
  separator. Each group keeps the file's order.
- **Text**: the title, or the action's name when the title is empty. `...` is added when
  choosing it asks something first: it takes arguments, or its effect is `destroy` (so every
  item of the last group ends in `...`). Longer than 24 glyphs with the `...`: cut to 21 and `...`.
- **`All actions...`** is always there when the app has an action: it opens the list (section
  3), where descriptions show and actions past the fourth are reached.
- **Geometry**: the menu is **216 wide** (text at x + 12, 24 glyphs), items 32 tall, separators
  9 tall with a 1 px `menu.border` line at their row 4, inset 8 (the toolkit menu's separator),
  1 px border. The example is 212 tall: items at y 1, 42, 74, 106, 147, 179. The tallest (four
  actions) is 244, inside the launcher's 400. Placement and clamping are unchanged.
- **Keys**: Up/Down step over items and skip separators, Enter chooses, Esc closes; typing a
  letter moves to the next item starting with it.
- **Choosing an action** closes the menu (and the launcher) and starts `app do --window <app>
  <action>`; `All actions...` starts `app do --window <app>`.
- **Files**: an app folder's menu in `/Applications` becomes `Open` `Actions...` `Uninstall...`
  ─ `Properties`; `Actions...` shows when the folder's `app.actions` offers an action and
  starts `app do --window <name>`. Files does not list titles: its menu is already long.
- Built-in apps (Terminal, Files, Settings) have no folder and so no actions yet (ask 7).

## 2. What choosing an action does

| The action | What happens |
|------------|--------------|
| no arguments, `read` or `change` | runs at once, no window; the result as section 6 |
| arguments, `read` or `change` | the form (section 4); `Run` runs it |
| no arguments, `destroy` | the confirmation (section 5) |
| arguments, `destroy` | the form with `Continue`, then the confirmation listing the values |

A `destroy` action never runs without the confirmation's button: the window passes
`--confirmed` only from that press. Choosing the same action twice opens two windows; each is
its own process and its own run.

## 3. The window, and the list of all actions

One window, titled with the app's name, 480 wide, not resizable, placed as the installer's
window; its height `H` is per screen. Each screen is a fresh widget tree
(`WidgetTree<40, 1>`). Every screen but the list shares:

| # | Widget | Place | Text and colour |
|---|--------|-------|-----------------|
| 1 | icon, 48 x 48 | (16, 16) | the app's icon (D196), else the generic tile |
| 2 | label, 384 wide | (80, 16) | the action's title in `text`, cut at 48 glyphs |
| 3 | label, 384 wide, two lines | (80, 36) and (80, 56) | the description in `text.dim`, wrapped at words, cut with `...` |
| 4 | label | (80, 76) | the effect: `Only reads.`, `Changes something in <App>.` in `text.dim`; `Deletes or overwrites something.` in `text` |
| 5 | label, 240 wide | (16, B + 6) | the status (`Running...`) in `text.dim` |
| 6 | button, 96 x 28 | (264, B) | the second button |
| 7 | primary button, 96 x 28 | (368, B) | the main button |

`B = H - 44`. The body starts at y 104.

**The list** (`All actions...`, `Actions...`), `H` 400: the icon; the app's name at (80, 16) in
`text`; `1 action` or `<n> actions` at (80, 36) in `text.dim`; widget 8, a **table** at (16, 80,
448, 260) with columns `Action` 160, `What it does` 208, `Effect` 72 (header 28, rows 24, so
nine rows show and the rest scroll). `Effect` reads `Reads`, `Changes` or `Deletes`; the
file's order, not sortable. Row 0 is selected and the table focused when it opens. `Close` is
widget 6; widget 7 reads `Run` for an action that runs at once and `Run...` for one that asks
first, following the selected row. Enter, a double-click or widget 7 chooses the row as
section 2 says: the form or confirmation replaces the list in this window, and a run that
needs no window shows `Running...` in widget 5 and then the result here. A file with no valid
action (it changed since the menu was drawn): no table, `<App> has no actions.` at (80, 36).

## 4. The form

One row per argument, in the file's order. `T(i) = 104 + 60 x i`:

| # | Widget | Place | Text and colour |
|---|--------|-------|-----------------|
| 9 + 3i | label, 336 wide | (16, T(i)) | the argument's description in `text`, cut at 42 glyphs; its name when there is none |
| 10 + 3i | label, right-aligned | ends at 464, top T(i) | `Required` or `Optional` in `text.dim`; a problem in `text` |
| 11 + 3i | the control | (16, T(i) + 20) | by type, below |

- **text**: a text field 448 x 28. **number**: a text field 160 x 28. Both are empty at first;
  an empty field is "no value" (so the form cannot pass an empty text; the CLI can).
- **boolean**: a dropdown 160 x 28, the same for both kinds so the control never changes
  shape: required `No`, `Yes` (`No` chosen); optional `Not set`, `No`, `Yes` (`Not set` chosen).
  A dropdown, not a checkbox, because an optional boolean has three answers.
- With one to four arguments `H = max(240, 156 + 60 x n)` (240, 276, 336, 396) and nothing
  scrolls. With five to eight `H` is 400 and the rows live in a scroll view at (0, 104, 480,
  244), content height `60 x n`; the header and the buttons stay.
- Widget 6 is `Cancel`; widget 7 is `Run`, or `Continue` for a `destroy` action.
- **Checking**, on `Run`, `Continue` or Enter in a field, with `ArgumentType::accepts` (what
  `app do` uses): a required field that is empty turns its word to `Needs a value`; a number
  field `accepts` refuses turns it to `Not a number`. The word is in `text`, the first such
  field takes the focus with its text selected (scrolled into view), and nothing runs. A
  word returns to `Required` or `Optional` at the next key in its field.
- **Running**: the controls and widget 7 are disabled, widget 5 reads `Running...`, widget 6
  becomes `Stop` (it ends the program; the result then reads `Stopped.`). The window stays
  responsive; closing it while the action runs is `Stop`.
- `Cancel`, Esc or closing the window before a run does nothing else. The values are kept
  only while the window is open.

## 5. The confirmation (`destroy`)

`H = max(240, 164 + 20 x v)`, `v` the arguments that have a value. Widgets 1 to 4, widget 4
in `text`. From (80, 104), one line per value at pitch 20 in `text.dim`: `<description>:
<value>`, cut at 48 glyphs; booleans read `Yes` or `No`. The buttons:

- the **action button**, its right edge at 464, top `B`: the action's title cut at 20 glyphs,
  `glyphs x 8 + 24` wide and at least 96. It names what will happen (`Empty trash`), as
  `Uninstall` does. It is **not** drawn as primary;
- `Cancel`, 96 x 28, 8 px to its left. **The focus starts on `Cancel`**, so Enter or Space
  right after the window opens cancels;
- `Back` at (16, B, 96, 28) when the confirmation came from the form: it returns there with
  the values kept.

The action button is person-only (D193): `wm show` lists it and `wm act` refuses to invoke
it, so an agent uses `app_do_confirmed`, which asks the user itself. Pressing it runs the
action with `--confirmed`: the buttons are replaced by a disabled `Running...` label in widget
5 and `Stop` in widget 6's place.

## 6. Where the output and the exit status show

- **A run that began without a window** (no arguments, not `destroy`, chosen from a menu):
  - ended with status 0 and no output: the desktop's notice (`desktop-v0.3-gui.md` 6.4 item
    2), `<title>: done`, cut at 36 glyphs, for 3 s. Nothing to close;
  - any output, or another status: the result screen opens as a window;
  - still running after 1000 ms: the window opens with widgets 1 to 4, `Running...` and
    `Stop`, so a slow action is seen and can be ended; the result then replaces it.
- **A run that began in a window** (form, confirmation, list) always ends in that window's
  **result screen**, so the person who filled a form sees its outcome where they were looking.

The result screen: widgets 1 and 2; widget 3 is the **status sentence** in `text`; widget 4 is
hidden. With output, `H` 360: widget 8, a read-only **text area** at (16, 84, 448, 216) holding
the program's output and errors in the order they arrived (the caret, selection and Mod+C work;
typing does nothing). Without output, `H` 240 and no area. Buttons: `Copy output` at (16, B,
112, 28), only with output; `Run again` (widget 6), which returns to the form with its values,
or to the confirmation for a `destroy` action, or runs at once; `Close` (widget 7, primary,
focused).

| What ended the run | Status sentence |
|--------------------|-----------------|
| status 0, with output | `Done.` |
| status 0, no output | `Done. The action printed nothing.` |
| another status | `Failed with status <n>.` |
| `Stop` | `Stopped.` |
| the program did not start | `Could not start: <the error's words>.` |
| `no_such_action` (the file changed, or the app was uninstalled) | `<App> no longer has this action.`, or `<App> is not installed.`; no `Run again` |
| `app do` refused the values (the file changed under the form) | `Could not run: <the error's words>.` |

Output past the text area's 64 KiB is cut, and the area's last line reads `Output cut. See all
of it in the Terminal: app do <app> <action>`. Bytes that are not text draw as `?`. The error's
words are never reworded: they are what `app do` prints.

## 7. Focus, keys and accessible names

- **Form**: the focus starts in the first control. Tab order: the controls top to bottom, then
  widget 6, widget 7, wrapping. Enter in a field is widget 7; Esc is `Cancel` (in a field with
  text, the toolkit's `Escape::Clear` empties it first). A control scrolled out of view scrolls
  in when it takes the focus.
- **Confirmation**: `Cancel`, the action button, `Back`; Enter and Space press the focused
  button only; Esc is `Cancel`. No key runs the action without the focus being moved to it.
- **Result**: `Close`, `Copy output`, the text area, `Run again`; Enter and Esc close.
- **List**: the table (Up/Down, Home/End, Enter), `Close`, widget 7; Esc closes.
- Mnemonics and shortcuts: none; the ring is the toolkit's.
- Names: a menu item is `<title>, action of <App>`, and `<title>, deletes, action of <App>`
  for `destroy`; a control is `<description>, required` or `, optional`, plus the problem
  word when shown; the action button is `<title>, deletes or overwrites`; the text area is
  `Output of <title>`; a table row reads its three cells.

## 8. Phone, tablet, TV (D23)

- **Phone**: a long-press on a launcher row or home icon opens the menu as the bottom sheet
  (items 48 tall, the two groups kept, the `destroy` group last). The window is a bottom sheet,
  full width: controls full width and 40 tall, the buttons 40 tall and side by side in the
  same order; the form scrolls above them. The notice is the same, at the top.
- **Tablet**: the desktop design, with long-press for the menu and 40 px controls.
- **TV**: the remote's `Menu` key on a focused launcher row opens the menu; the D-pad moves, OK
  chooses, `Back` is Esc. The confirmation's focus still starts on `Cancel`. Text and number
  arguments need the on-screen keyboard; until it exists an action with such an argument shows
  disabled in the menu and the list.
- **Wearable**: no menu and no window; actions are reached by an agent.

## 9. Acceptance (host tests, scale 1x)

App `notes` with `new-note` (`New note`, change, `title:text:required:Title`,
`pinned:boolean:optional:Pin it`), `count` (`Count notes`, read), `empty-trash` (`Empty
trash`, destroy), in that order.

- The launcher row's and the desktop icon's menu is 216 x 212 with `Open`, `New note...`,
  `Count notes`, `All actions...`, `Empty trash...`, `Uninstall...` at y 1, 42, 74, 106, 147,
  179; Down from `All actions...` lands on `Empty trash...`. An app without `app.actions` has
  the 160 x 66 menu. Six actions show four and `All actions...`. An action titled with 30
  glyphs shows 21 and `...`.
- `Count notes` starts `app do --window notes count`: no window; status 0 with no output shows
  the notice `Count notes: done`; output `12 notes` opens a 480 x 360 window with `Done.` at
  (80, 36), the text area at (16, 84, 448, 216) holding `12 notes`, `Close` focused at (368,
  316); `Copy output` puts `12 notes` on the clipboard. A program that sleeps shows the 480 x
  240 window with `Running...` after 1000 ms, and `Stop` gives `Stopped.`.
- `New note...` opens a 480 x 276 window: `Title` at (16, 104), `Required` ending at 464, a
  field at (16, 124, 448, 28) with the focus; `Pin it` at (16, 164), `Optional`, a dropdown at
  (16, 184, 160, 28) showing `Not set`; `Cancel` at (264, 232), `Run` at (368, 232). Enter with
  the field empty runs nothing and shows `Needs a value` in `text`; typing `Milk` and Enter
  runs `new-note` with `title=Milk` and no `pinned`; choosing `Yes` adds `pinned=true`. A
  number argument holding `1e3` shows `Not a number`. Eight arguments give a 480 x 400 window
  whose rows scroll and whose buttons stay at y 356.
- `Empty trash...` opens a 480 x 240 window with `Deletes or overwrites something.` in `text`
  at (80, 76), `Cancel` at (248, 196) focused and `Empty trash` at (352, 196, 112, 28). Enter
  closes it and nothing runs. Tab then Enter runs `app do notes empty-trash --confirmed`.
  `wm act` on the action button is refused. A `destroy` action with arguments shows `Continue`
  on the form, then its values as lines, and `Back` returns with the values kept.
- Exit status 3 with `disk full` on its errors shows `Failed with status 3.` and the text.
  Removing the action from the file before the choice shows `Notes no longer has this
  action.` with `Close` only. No path prints `needs_confirmation`.
- `All actions...` opens a 480 x 400 window with three rows (`New note`, `Count notes`, `Empty
  trash`; `Changes`, `Reads`, `Deletes`); widget 7 reads `Run...`, `Run`, `Run...` as the
  selection moves; Enter on `Count notes` shows its result in the same window.
- Files' menu on `/Applications/notes` has `Actions...`; on an app without actions it has not.
- Every run through the window writes the audit record `app do` writes.

## 10. Backend asks

For the kernel's desktop:

1. The catalogue reads each installed app's `app.actions` with the manifest (and again on
   `APPS_CHANGED`) and keeps, per app, the first four actions' name, title (24 bytes),
   whether it asks first and whether it is `destroy`, and whether there are any.
2. `AppMenu` with a variable item list, separators and the 216 width; a new `Action` that
   starts `/bin/app do --window <app> [<action>]`, as `Action::Uninstall` starts the installer.
3. A notice with text from a program (a window request, at most 38 glyphs, rate-limited), so
   `app` can show `<title>: done`. Until then that case opens the 240-tall result screen.

For `app`:

4. `app do --window <app> [<action>]`: the screens above over the same run as `app do`, with
   the output and errors captured in arrival order, the exit status, and a way to end the
   program (`Stop`).
5. `app do` runs the program with `NO_COLOR=1`, so output shown in the text area carries no
   escape sequences.

For the toolkit:

6. A read-only text area (`TextArea` gains `read_only`: selection, copy and scrolling work,
   typing, cut and paste do nothing), and a person-only mark on a button that `wm act` honours.

Later:

7. Actions for the built-in apps (an `app.actions` beside their programs), actions on the
   dock's and the taskbar's icon menus, an optional title per argument and a default value in
   the file format, and pinning one action to the launcher as its own row.
