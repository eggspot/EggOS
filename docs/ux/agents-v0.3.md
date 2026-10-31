# EggOS v0.3.0 Agents: design brief (slice 41c)

Design by Claude Fable (D16) for the GUI front-end of the `agent` capability (D22, D193): the
Settings `Agents` pane that lists the agents EggOS knows, sets each one's grants, revokes and
forgets them and browses the audit log, with the same data and words as `agent list`, `agent
show <id>`, `agent grant [--ask] [--for 15m|2h|7d] [--yes] <id> <view|write|install|run>`,
`agent revoke <id> [<grant>]`, `agent forget <id>`, `agentlog [--agent] [--tool] [--outcome]
[--since]` and the MCP tools `agent_list` and `agent_show`; and the grant prompt, the dialog
EggOS itself draws when an agent with an `ask` grant calls a tool or a call needs the user's
yes. Everything in `toolkit-v0.3.md` (section 11 for Settings), `desktop-v0.3-gui.md` sections
3, 10 and 12, `keyboard-v0.3.md` and `storage-v0.3.md` (the list-and-facts panes this one is
shaped like) stands; numbers are px at 1x and double at 2x; the only motion is the caret's blink
and the prompt's one-a-second countdown. The store holds at most 16 agents (D193), so the pane
is built for `local` plus 0 to 16 rows. Every text below fits a label's 64 bytes with an id of
up to 16 characters; a longer id (ids go to 32) ends the line in `...` as Storage's `shortened`
does, and the CLI has the whole text.

## 1. The pane in the sidebar

- **Name** `Agents`: the CLI's noun (`agent`), the bar's `Agent` chip and its tooltip (`An AI
  agent is connected`), and what people call Claude Code, Copilot and the rest. Not `AI` (too
  wide), not `Privacy & security` (macOS's and Windows's panes hold far more than this). Its
  row comes right after `Date & time`, so the hardware panes stay together, the clock stays
  with them, and the two about the system itself stay last: `Appearance`, (`Network`,)
  `Display`, `Keyboard`, `Mouse`, `Storage`, `Date & time`, `Agents`, `Components`, `About`.
  Without Network its row is y 236..267 (44 + 6 x 32) and `Components` and `About` move down
  32 (268, 300; `AGENTS` is index 6, `COMPONENTS` 7, `ABOUT` 8); with Network's row too,
  everything is 32 lower again and the sidebar's list scrolls at the 360 minimum (toolkit
  section 8), as any list does.
- **Search words**: `agent`, `agents`, `AI`, `assistant`, `MCP`, `Claude`, `Copilot`, `grant`,
  `grants`, `permission`, `permissions`, `privacy`, `security`, `allow`, `deny`, `ask`,
  `revoke`, `forget`, `audit`, `log`, `activity`, `view`, `write`, `install`, `run`, `Privacy &
  Security`, plus its button names `Add`, `Revoke all`, `Forget`. `install` also matches
  Components; nothing else collides.
- **Budget**: the tree holds 96 widgets and 77 are used (`storage-v0.3.md` section 1). This
  pane is its container plus exactly 36 widgets (7 shared by both views, 14 in the grants
  view, 10 in the activity view, 5 in the confirmation panel), so `CAPACITY` becomes **114**
  (the installer's `eggos_app::dialog::CAPACITY` stays equal to it; the grant prompt's own
  tree, section 5, is 11 widgets of the same type). Network's 16 and About's 3 take it to 133
  when they land. The count is this small because dynamic rows are labels drawn over a list of
  empty static rows (section 2.1), not one button per row.

## 2. Layout (pane coordinates: origin window x 176, y 0; 464 wide at the default size, so the
right-aligned edge is x 448; usable height 332, the pane's height at the minimum window)

Every widget has one fixed place; a state shows, hides, enables or retitles widgets and never
moves them. The pane has two views, `Grants` and `Activity`, chosen by a horizontal radio
group beside the heading; the agents list at the top left is shared, and the area under it
holds one view's widgets at a time (the other view's are hidden). The status line at the
bottom is shared too.

### 2.1 Shared widgets and the grants view

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | heading | (16, 16), 2x | `Agents` |
| 2 | radios, horizontal | right-aligned, ends at x 448, y 18, 28 tall (about 170 wide) | `Grants` `Activity`; `Grants` when the pane is shown |
| 3 | list, 3 rows of 24, mark x 8, text x 24 | (16, 64), 208x72 | 32 static rows whose text is empty; `set_shown_items` to the agents' count; empty text never shows (`local` is always a row) |
| 4-6 | labels | (40, 68), (40, 92), (40, 116) | the three visible rows' texts, section 3, in `text`; retitled on every scroll and selection change |
| 7 | label | (16, 312) | the status line (section 4) or, in `Activity`, the selected record's detail line (section 2.2), `text.dim` or `text` |
| 8 | text field | (240, 64), 104x28 | placeholder `Agent id`, `escape: Clear` |
| 9 | push button | (344, 64), 96x28 | `Add` |
| 10 | push button | (240, 100), 96x28 | `Revoke all` |
| 11 | push button | (344, 100), 96x28 | `Forget` |
| 12, 14, 16, 18 | labels | (16, 154), (16, 186), (16, 218), (16, 250) | the class lines `View`, `Write`, `Install`, `Run` with their note (section 3) in `text` |
| 13, 15, 17, 19 | radios, horizontal | right-aligned, end at x 448; y 148, 180, 212, 244; 28 tall (about 206 wide, so from x 242) | `Deny` `Ask` `Allow`; the class's own permission from the store |
| 20 | label | (16, 282) | `Grants last` in `text` |
| 21 | radios, horizontal | right-aligned, ends at x 448, y 276 (about 290 wide, from x 158) | `No end` `15 min` `2 h` `7 d`; `No end` when the pane is shown |

The list draws selection, hover, the `>` mark and the focus ring; labels 4-6 are declared
after it so they draw over its rows, and labels take no pointer input, so a click on a row's
text reaches the list. The list scrolls in whole rows (toolkit section 8), so the three labels
always sit on three whole rows: after a scroll the program retitles them with rows `offset ..
offset + 3`. A machine with `local` and two agents (the usual case) never scrolls. The right
column at x 240 is 208 wide: the id field and `Add` fill it, and the two buttons under them
end at x 440. Class rows are 32 apart (a 28 px control and 4 px), their label cell at the
control's +6; a class line has up to 226 px before its radios (`Install · until 2026-10-31
11:14` is about 200). `Grants last` ends at y 304 and the status line's cell at 328, inside
332: nothing scrolls at the minimum window, and at a taller window nothing moves.

### 2.2 The activity view (shown with widgets 1-7 when `Activity` is chosen)

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 22 | checkbox | (240, 64), 208x28 | `This agent only`; unchecked when the pane is shown |
| 23 | text field | (240, 100), 208x28 | placeholder `Tool name`, `escape: Clear` |
| 24 | radios, horizontal | (16, 148), 28 tall (about 316 wide) | `Any result` `ok` `error` `refused`; `Any result` when the pane is shown |
| 25 | label, right-aligned | ends at x 448, cell top 154 | the count `32 of 418` in `text.dim`; hidden when nothing matches |
| 26 | radios, horizontal | (16, 180), 28 tall (about 406 wide) | `Last hour` `Last day` `Last week` `All time`; `All time` when the pane is shown |
| 27 | list, 4 rows of 24, mark x 8, text x 24 | (16, 216), 432x96 | 32 static empty rows, `set_shown_items` to the records shown; empty text `No records match.` |
| 28-31 | labels | (40, 220), (40, 244), (40, 268), (40, 292) | the four visible records' rows (section 3) in `text`; retitled on scroll and selection as labels 4-6 are |

The list holds the newest 32 records that match the filters, newest first, four visible and
the rest by wheel, arrows or the bar; label 25 says how many of the matching records they
are. The status line (widget 7) is the selected record's detail line in this view. Switching
views hides one set and shows the other in the same frame; the agents list keeps its
selection and the filters keep their values while the pane is shown, and all reset when it is
shown again.

### 2.3 The confirmation panel (section 4), drawn as Storage's format panel

A modal panel inside the pane, 400 wide, `menu.bg`, `INSET_R6`, 1 px `menu.border` inside, a
2 px `field.bg` edge outside, centred in the pane (x 32; y `(pane height - 132) / 2`, 100 at
the minimum window, 108 at the default), 132 tall. While it shows the pane's widgets ignore
pointer, wheel and Tab, and the lists draw unfocused.

| # | Widget | Place in the panel | Text |
|---|--------|--------------------|------|
| 32 | panel | (0, 0), 400x132 | |
| 33 | label | (16, 16) | `Grant run to phone?` (`Grant install to phone?`) in `text` |
| 34 | label | (16, 36) | `Run gives it full control of EggOS.` or `Install lets it add programs.` in `text.dim`; with `Ask`: `Each call asks you. Run gives it full control of EggOS.` |
| 35 | push button | (192, 88), 96x28 | `Cancel` |
| 36 | push button, primary | (288, 88), 96x28 | `Grant`; the window's default button while the panel shows |

## 3. The rows, the lines and the records: one set of words for the pane, the CLI and MCP

The program reads the store and the boot grants each time the pane is shown and again after
every change it writes (never on a tick), through the functions `agent list` uses, moved
where Settings can call them (`eggos_mcp::grants` for the store's reading and writing with
`agent-grants.new`, `sync`, `rename`; `grants::from_command_line` for the boot grants). The
rows are `agent list`'s rows: `local` first (its boot grants when the store has no entry for
it), then the store's agents in the store's order. Every stamp is UTC, as `date` and
`agentlog` print it.

| Fact | Pane | `agent list` / `agentlog` |
|------|------|---------------------------|
| an agent's row (labels 4-6) | `phone · Claude on my phone` (id, `·`, name); `local` or `phone` alone when the name is the id; ellipsised at the row's end | `phone  Claude on my phone  ...` |
| a class line (labels 12-18) | `Write`; `Write · until 2026-10-31 11:14`; `Write · ended 2026-10-31 09:00`; `Write · run brings it`; `Write · started without it`; `Write · file untrusted` | `allow until 2026-10-31T11:14:00Z`; `note: phone keeps run, which brings write`; `note: EggOS was started without write` |
| a class's radios | the class's own permission (`deny`, `ask`, `allow`) as `Deny` `Ask` `Allow`; `Deny` when its time has ended | `deny`, `ask`, `allow` |
| a record's row (labels 28-31) | `10-31 09:14:02 · phone · tee · ok` (month-day and time, the id, the tool, the row's result; an id past 12 characters is its first 9 and `...`) | `TIME  AGENT  CLIENT  TOOL  RESULT  ...` |
| a record's detail line (widget 7) | `phone · exit status 0, 5 bytes · {"path":"/home/a"}` (the id, the DETAIL field, the ARGUMENTS field), cut at 64 bytes with `...` | the row's `ARGUMENTS` and `DETAIL` |

**A class line's note** is the first that applies: `started without it` when the boot grants
lack the class (every agent is capped by them, 41b1; the radios still show and write the
store's choice, which takes effect only once EggOS starts with `--mcp-allow <class>`); `file
untrusted` when the store cannot be read (section 4); `run brings it` when the agent's `run`
permission is stronger than the class's own (D193: a class's decision is the stronger of its
own and `run`'s; the `Run` row right below shows which); `until <stamp>` when the grant has an
end still ahead; `ended <stamp>` when it has passed (the radios then show `Deny`: that is the
decision; the store keeps the old line until the user chooses something, as `agent list`
shows it). The `·` is U+00B7.

**The records** are `agentlog`'s rows, read through the function it uses (moved beside it):
one row per call, `interrupted` for a call still open, built on `audit::query`'s
oldest-first records and shown newest first. The filters are `Filter`'s: `This agent only`
is `--agent <the selected row's id>`; the tool field is `--tool` (exact name, trimmed; empty
is no filter); the result radios are `--outcome ok|error|refused` by how the call ended
(`Any result` is none; `asked`, `started`, `initialize`, `disconnected`, `server-start` and
`server-restart` show under `Any result`, as they do without `--outcome`); `Last hour`, `Last
day`, `Last week` are `--since 1h|24h|7d` and `All time` none. The log is read when the view
is shown and whenever a filter or, with the checkbox ticked, the selection changes; not on a
tick.

## 4. The actions, their rules and the status line

**Which controls are enabled**, decided from the selected agent and the store's state, with
the status line (widget 7, `text.dim`) giving the reason whenever something is disabled:

| State | Class radios | Revoke all | Forget | Add | Status line while idle |
|-------|--------------|------------|--------|-----|------------------------|
| `local` selected, store fine | on | on | off | on | `local is EggOS's own channel: revoke it, never forget it.` |
| another agent selected, store fine | on | on | on | on | `Started with: view write. No agent gets more.` (the boot grants' words from `Grants::describe`; `Started with: none. Agents can only read.`) |
| no disk (D131) | on | on | as above | on | `No disk attached: changes last until power-off.` |
| the store cannot be trusted (`StoreState::Unreadable`) | off | off | off | off | `Grants file cannot be trusted: <reason>.` (`StoreError::reason()`, at most 32 bytes, a test pins it) |
| 16 agents in the store | on | on | on | off | `EggOS keeps at most 16 agents: forget one first.` |
| the id field holds an invalid id | | | | off | `An id is 1 to 32 letters, digits, ".", "_" or "-".` |
| the id field holds `local` | | | | off | `local is always listed.` |

The status line's priority is the table's order from the top; the no-disk line replaces the
started-with line only (the local line and the untrusted line win over it). A damaged store
lists `local` alone with its boot grants and everything else disabled: the CLI's `agent list`
refuses the same file with `damaged` and says to remove it; the pane does not delete files
(listed in "Left out"), and an agent with `write` cannot reach `/var/lib/eggos` either way.

**Choosing a permission** (the radios of widgets 13-19) writes the store in the same
keystroke or click, instant and silent as the Keyboard pane's list: `Deny` is `agent revoke
<id> <class>`; `Ask` and `Allow` are `agent grant [--ask] <id> <class>` with `--for` from
`Grants last` (`No end` none, else `15m`, `2h`, `7d`, the end being now plus that). Choosing
the option already chosen writes nothing; so to put an end on a grant that has none, choose
`Deny` and then the permission again (or use the CLI's `--for`). `Grants last` by itself
writes nothing: it shapes the next `Ask` or `Allow`. An agent not yet in the store (`local`
without an entry) gets its entry on the first write exactly as the CLI makes it: `local`'s
from its boot grants, so taking one grant back keeps the others. After the write the pane
re-reads the store and the status line reads, in `text`: `Gave write to phone, from its next
call.`; `Gave write to phone until 2026-10-31 11:14.`; `Gave write to phone, asking each
time.`; `Took write back from phone, from its next call.`; `Took write back from phone; run
still brings it.`; `Gave write to phone; EggOS was started without it.` (the CLI's note: no
effect until EggOS starts with `--mcp-allow write`). The server reads the store before every
call (D193), so "from its next call" is exact.

**Confirmation: `Ask` or `Allow` on `Run` or `Install`.** `agent grant <id> run|install`
needs `--yes` (41b1: run is full control, install adds programs), so the pane asks the same
way: the radios stay on their old choice and the panel of section 2.3 opens with the chosen
permission remembered; `Grant` (Enter) writes it and closes the panel; `Cancel`, Esc or
closing the window writes nothing and returns the focus to the radios. `Deny` on those rows
asks nothing: taking back never asks. The confirmation is not destructive (nothing is lost),
so it is a button, not Storage's typed name.

**Revoke all** is `agent revoke <id>`: every class to `Deny`, no end; no confirmation (effect
`change`; the safe direction). Status: `Took every grant back from phone.` **Forget** is
`agent forget <id>`: the agent leaves the store, the row above it (or `local`) is selected and
the status line reads `Forgot phone: it gets nothing until granted again.`; no confirmation
either, as the CLI asks none (effect `change`): an agent forgotten by a slip loses grants, never
data, and gets them back by a few clicks. `Forget` is disabled for `local` (D193: the local
agent is always there; revoking it is the way). **Add** takes the id field (trimmed): a valid
id not in the store is added with every class `Deny` and no name (the name is the id, as the
CLI's fallback), its row selected, the field cleared and the status `Added phone with no
grants: choose what it may do.`; an id already listed only selects its row. Enter in the field
presses `Add` when it is enabled, else nothing. Naming (`--name`) stays the CLI's in 0.3.0.

**Failures** keep the pane as it was and show in `text`: `Grants file not written: <errno
text>.` (the write to `agent-grants.new` or the rename failed); `Grants file cannot hold
this: it holds too many agents.` (the store's `too_many_agents`, when a 17th agent raced the
pane); after a failure the store is re-read.

**The activity view's status line** is the selected record's detail line (section 3) in
`text`; with no records it reads `No records match.` in `text.dim` (the list's empty text says
the same); when the log cannot be read: `The log could not be read: <errno text>.` When more
records match than the 32 shown, label 25 says `32 of 418`: older ones are reached by
narrowing the filters or with `agentlog` (paging is listed in "Left out").

## 5. The grant prompt: EggOS's own dialog for `ask` grants and the user's yes

**When.** The server has to ask the user (D193's `Decision::AskUser`, or a tool whose
`Confirmation` needs a yes for this call) and the desktop is running (the server can open
`/dev/window` and `/bin/settings` exists: the desktop is an optional component, D18). Then it
asks on EggOS's own screen, for every agent including `local`, instead of through the client:
the screen is EggOS's, the client's elicitation is only as strong as the client is honest
(D193's accepted risk, which this closes). Without the desktop the server asks through the
client's elicitation exactly as today, and refuses as today when the client cannot ask
(`confirm: true` never counts). Decided here: the screen wins when both are possible; the
trade is that a host-side `local` client in a QEMU run must look at the QEMU window; the owner
can overrule.

**The program.** `settings --grant-prompt`, started by the server the way it runs a twin: it
writes the question as one JSON line on the prompt's stdin (`agent`, `name`, `tool`, `title`
from `ToolDefinition.title`, `class`, `destructive` (the call needs a yes by itself),
`arguments` as the audit record's `ARGUMENTS` object, `timeout_s`), and reads one word on its
stdout: `once`, `session`, `always`, `deny` or `timeout`; exit status 0. The window is a
`Dialog` with no parent (a new case the window server allows for this program: centred in the
work area, at the top of layer 1 and kept there while shown, so a click on another window does
not cover it), `FIXED_SIZE`, title `An agent asks`, content 480x200, taking focus when first
shown (section 3.2 of `desktop-v0.3-gui.md`). One prompt at a time: the server asks one
question at a time already (`Pending` is one).

| # | Widget | Place and size | Text and colour |
|---|--------|----------------|-----------------|
| 1 | label | (16, 16) | the agent's name `Claude on my phone` in `text` (`local` for the local agent) |
| 2 | label, right-aligned | ends at x 464, cell top 16 | `id phone` in `text.dim`; hidden when the name is the id |
| 3 | label | (16, 40) | `tee · Write a file` (the tool's name, `·`, its title) in `text` |
| 4-6 | labels | (16, 68), (16, 88), (16, 108) | the arguments, one `key: value` a line in `text`, the path first (`path: /home/mea/notes.txt`, `content: 40 bytes`, `append: false`); each cut at 64 bytes and the line's width with `...`; unused lines hidden |
| 7 | label | (16, 132) | `Denied in 1:30 if you do nothing.` in `text.dim`, counting down once a second (damaging its rect only) |
| 8 | push button | (36, 156), 96x28 | `Always`; hidden when `class` is `run` or `install` or `destructive` is true |
| 9 | push button | (140, 156), 116x28 | `This session` |
| 10 | push button | (264, 156), 96x28 | `Once` |
| 11 | push button, primary | (368, 156), 96x28 | `Deny`; the default button, focused when the window opens |

The buttons are a dialog row: 8 apart, right-aligned, the default one last (toolkit section
4), so the most careful answer sits where every profile's default button sits, and hiding
`Always` leaves its place empty rather than moving the others. The question itself is the
same one the server words for elicitation (`An agent asks to replace /home/mea/notes.txt (12
bytes) with 40 bytes. Allow it?`): the prompt shows its facts as lines because a label holds
64 bytes and a path can be longer; the whole sentence is in the audit log's record.

**What each answer means for the server.**

| Answer | The call | The store | Later calls |
|--------|----------|-----------|-------------|
| `once` | runs (`started`, detail `allowed once by the user`) | unchanged | ask again |
| `session` | runs (`allowed for this session`) | unchanged | the server keeps, per connection, the classes the user allowed for the session; while the store says `ask` for that class it runs them without asking; `disconnected` clears them; a store that says `deny` wins, since the store is read before every call |
| `always` | runs (`allowed always`) | the prompt writes the agent's class to `allow`, no end, as `agent grant <id> <class>` does, before answering; if the write fails the answer becomes `once` | `allow`: no question |
| `deny` | refused (`needs_grant`, detail `denied by the user`) | unchanged | ask again |
| `timeout` | refused (`no answer in 90 s`) | unchanged | ask again |

`Always` is offered only for `view` and `write` calls that need no yes of their own: `run` and
`install` are never granted from a prompt (the CLI needs `--yes` for them, and a prompt is
the wrong place to read what that means), and a destructive call is answered for itself.

**Timeout.** The prompt denies after **90 s** (`GRANT_PROMPT_TIMEOUT_S`), shown as `1:30`
down to `0:00`; the server keeps `ELICITATION_TIMEOUT_MS` (120 s) as its own backstop, so the
prompt's answer always comes first and the server's timeout only covers a prompt that died.
Ninety seconds is long enough to walk to the screen and short enough that a waiting agent is
not stuck for minutes.

**Keys.** Tab goes `Always`, `This session`, `Once`, `Deny` (hidden ones skipped), wrapping;
Shift+Tab the reverse; focus opens on `Deny`. Space presses the focused button; Enter is the
default button, `Deny`, from anywhere; Esc is `Deny` (a `Dialog` closes as its cancel button,
toolkit section 3); the title bar's close control and a `CloseRequest` are `Deny`. No
mnemonics: the toolkit has none in 0.3.0. Pointer `Arrow` everywhere.

**Agents and the prompt.** Its four buttons are person-only (section 8): `wm act` on them
answers `element_cannot`, and closing it with `wm close` only denies the asking agent its own
call. The honest limit: `wm click` and `wm key` inject input the window cannot tell from the
user's, so an agent holding `run` could press `Once`; but every input tool needs `run`, which
is full control of EggOS already (D137), and an agent whose `run` is `ask` is asked before
the click, which it cannot answer without another click. The prompt therefore adds no input
filtering; it relies on the grant model.

## 6. Keys, focus and pointer

- Tab from the sidebar list enters the pane at the view radios, then the agents list, then
  the shown view's widgets in reading order: grants: the id field, `Add`, `Revoke all`,
  `Forget`, the four class groups, `Grants last`; activity: `This agent only`, the tool field,
  the result radios, the time radios, the records list; disabled ones skipped; then wraps to
  the search field; Shift+Tab the reverse. Showing the pane does not move the focus.
- In the agents list: Up/Down, Home/End move the selection (one row is always selected); a
  change updates the class lines, the radios, the buttons and the status line in the same
  frame and, in `Activity` with the checkbox ticked, re-reads the log. Enter, Space and a
  double-click do nothing: the row has no open action. The records list is the same, a change
  updating the detail line only.
- In a radio group: Left/Right and Up/Down move and choose at once (toolkit section 3), so
  arrows across `Deny` `Ask` `Allow` write the store per step, as the Keyboard pane's arrows
  apply layouts per step; stepping onto `Ask` or `Allow` of `Run` or `Install` opens the panel
  as a click does.
- Space or Enter on a focused button presses it; Enter in the id field presses `Add` when it
  is enabled; Enter elsewhere does nothing (the pane has no default button while the panel is
  hidden). Esc clears a field, else goes to the window, which ignores it. While the panel
  shows, Tab goes `Cancel`, `Grant`, wrapping; Enter is `Grant`; Esc is `Cancel`.
- Pointer: `Arrow` everywhere but the two fields (`IBeam`); a click on a row selects and
  focuses its list; a right-click does nothing.

## 7. Every device class (D23)

- **One column** when the pane is narrower than 448 (a phone's maximised window): the view
  radios go under the heading at (16, 56); the agents list keeps x 16 and takes the width
  minus 32; the right column's widgets (id field and `Add`, then `Revoke all` and `Forget`, or
  the checkbox and the tool field) move under it, x 16, 8 apart, full width minus 32, the
  pairs sharing a row when two 96 px buttons fit and stacking otherwise; the class rows follow
  with the label on one line and the radios on the next (28 + 28, 8 between rows), `Grants
  last` the same way; the time radios wrap to two lines of two; the records list takes the
  width minus 32; then the status line. The pane's panel grows to its content and scrolls in
  the window's existing scroll view (wheel 72 px; the focused widget is kept in view). The
  confirmation panel takes the pane's width minus 16 and sits at the pane's bottom, as a
  sheet, its two buttons sharing the last row.
- **Touch**: list rows 40 tall (text cell top +12, `>` at x 8), buttons and radios 40 tall,
  fields 28 as everywhere; both lists then show their rows at 40 and everything below moves
  down by the difference.
- **The prompt on a phone** is a bottom sheet (`desktop-v0.3-gui.md` section 12): full width,
  the lines as above with 16 px padding, the buttons stacked full width, 40 tall, 8 apart,
  `Always` first and `Deny` last (nearest the thumb, as the default is last); `Back` is
  `Deny`. On a wearable the sheet is the whole screen and scrolls to the buttons; the
  countdown line stays visible above them.
- **TV (scale 2)**: everything doubles; the ring on the selected row, the chosen radio and
  the focused button is the cue; the pane and the prompt work by arrows, Tab, Enter and Esc;
  `Back` on the remote is `Deny`.
- **Keyboard-only** on every form factor: each list is one Tab stop; the panel and the
  prompt need only Tab and Enter.
- **Headless** (no desktop component): no pane and no prompt; the CLI and MCP tools are the
  whole capability, and the server asks through the client's elicitation (section 5).

## 8. What an agent sees (D161, `wm show`), and what it may not touch

The agents list is a `list` named `Agents`, its rows `list item`s named by their row text
(`phone · Claude on my phone`) with the value the id (`phone`); the view radios a `radio group`
named `View` with `radio`s `Grants` and `Activity`; the class groups `radio group`s named
`View`, `Write`, `Install`, `Run` with `radio`s `Deny`, `Ask`, `Allow`, the chosen one
`selected`; `Grants last` a `radio group` of that name; `Add`, `Revoke all`, `Forget`, `Cancel`
and `Grant` `button`s, disabled ones flagged; the id field a `text field` named `Agent id`;
the class lines and the status line `label`s with their drawn text; the activity view's
checkbox a `checkbox` named `This agent only`, its field a `text field` named `Tool name`, its
groups `radio group`s named `Result` and `Since`, its list a `list` named `Activity` whose
items are named by their row text with the value `<time> <agent> <tool> <result>`
(`agentlog`'s four columns, the full stamp and id).

**Person-only widgets (SECURITY).** D193's store is one only the user changes, so no agent
operates the controls that write it or shape a write: the four class groups and each of their
options, `Grants last`, `Add`, `Revoke all`, `Forget`, the panel's `Cancel` and `Grant`, and
the prompt's four buttons. The toolkit gains a per-widget flag, `WidgetTree::set_person_only
(id, true)`, that `run_view`'s `act` checks before anything else and answers
`Outcome::NotSupported` for every `Action` (`Invoke`, `Select`, `SetText` and `Focus` alike),
which `wm act` reports as `element_cannot`; `wm show` lists the flag as `person-only` so an
agent knows why. Navigation stays actable: the sidebar and its search, the view radios, the
agents list (`Select`, `Focus`), the activity filters (`Invoke` on the checkbox, `SetText` on
the field, `Select` on the groups) and the records list. An agent that wants the records calls
`agent_list` and `agent_show` (its own grants; no tool reads another agent's), not the tree
(D22); no MCP tool reads the log (41b2: agents read it through `run`, which shows every
agent's calls).

## 9. Acceptance (host tests, default 640x440, scale 1x, a fake with the boot grants `view
write`, a store holding `phone` named `Claude on my phone` (`view allow`, `write ask` until
2026-10-31T11:14:00Z, `install deny`, `run deny`) and `laptop` (`run allow`, the rest `deny`),
a clock at 2026-10-31T09:14:02Z, and `agentlog`'s test log)

- `PANES` holds `Agents` right after `Date & time`, row (0, 236, 176, 32); `Components` at
  y 268 and `About` at 300; `agent`, `grant`, `mcp`, `ai`, `permissions`, `privacy`, `audit`,
  `revoke` and `activity` match it and nothing else; `install` matches it and Components.
- In window coordinates: heading (192, 16); the view radios end at x 624, y 18, `Grants`
  chosen; the agents list (192, 64, 208, 72) with rows `local`, `phone · Claude on my phone`,
  `laptop`, the first selected; the id field (416, 64, 104, 28), `Add` (520, 64), `Revoke all`
  (416, 100), `Forget` (520, 100) disabled; class lines at (192, 154), (192, 186), (192, 218),
  (192, 250) reading `View`, `Write`, `Install · started without it`, `Run · started without
  it`; the radios' right edges at 624 and their choices `Allow`, `Allow`, `Deny`, `Deny`;
  `Grants last` at (192, 282) with `No end` chosen; the status line at (192, 312) reading the
  local line in `text.dim`.
- Down selects `phone`: lines `View`, `Write · until 2026-10-31 11:14`, `Install · started
  without it`, `Run · started without it`; radios `Allow`, `Ask`, `Deny`, `Deny`; `Forget`
  enabled; the status line `Started with: view write. No agent gets more.`
- `Allow` on `Write` writes the store with the line `phone\tallow\tallow\tdeny\tdeny\tClaude on
  my phone` (through `agent-grants.new`, `sync`, `rename`) and shows `Gave write to phone, from
  its next call.`; `2 h` then `Ask` on `View` writes `ask@1761909242` (09:14:02 + 7200) and the
  line reads `View · until 2026-10-31 11:14`; `Allow` again on `Write` writes nothing; `Deny`
  on `Write` writes `deny` with no end.
- `Allow` on `Run` leaves the radios on `Deny` and shows the panel at (208, 108, 400, 132)
  with `Grant run to phone?` and `Run gives it full control of EggOS.`, `Grant` primary and
  focused on `Cancel`; Esc closes it and writes nothing; `Grant` writes `run allow`, the
  `Install` line then reads `Install · run brings it` and `View` and `Write` carry no note.
- Down to `laptop`: the lines read `View · run brings it`, `Write · run brings it`, `Install ·
  started without it`, `Run · started without it`; `Revoke all` writes `laptop\tdeny\tdeny\tdeny\tdeny\tlaptop` and
  shows `Took every grant back from laptop.`; `Forget` removes the line, selects `phone` and
  shows `Forgot laptop: it gets nothing until granted again.`
- Typing `tv` in the id field and Enter adds `tv\tdeny\tdeny\tdeny\tdeny\ttv`, selects its
  row and clears the field; `local` and `x!` disable `Add` with their status lines; a store of
  16 agents disables `Add` with the 16-agents line; a fake whose write fails with `EROFS`
  shows `Grants file not written: ...` and the radios keep the store's choice.
- A fake whose store is damaged lists `local` alone, disables the radios and buttons and
  reads `Grants file cannot be trusted: <reason>.`; a fake with no disk reads the no-disk line
  for `phone`; a `local` store entry above the boot grants (`install allow`) shows `Allow`
  with `Install · started without it`.
- `Activity` hides widgets 8-21 and shows the checkbox at (416, 64), the field (416, 100, 208,
  28), the result radios at (192, 148), the time radios at (192, 180), the count `7 of 7` ending
  at x 624, and the list (192, 216, 432, 96) whose first row reads `01-01 00:00:07 · local · ls ·
  interrupted` and the detail line `local · /home · {}`; typing `cat`
  leaves two rows; `refused` one; `This agent only` with `phone` selected leaves none and the
  list reads `No records match.`; `Last hour` with the fake clock leaves none; `All time` and
  an empty field bring the seven back; a fake whose log read fails shows `The log could not be
  read: ...`.
- The element tree holds the list `Agents` with three items, the first `local` with value
  `local`; the groups `View`, `Write`, `Install`, `Run`, `Grants last`, the buttons `Add`,
  `Revoke all`, `Forget` flagged `person-only`; `wm act` `Invoke` on `Forget` and `Select` on
  an `Allow` radio answer `element_cannot` and change nothing; `Select` on the agents list's
  second item and `SetText` on `Tool name` answer `Done`.
- The prompt: `settings --grant-prompt` with `{"agent":"phone","name":"Claude on my phone",
  "tool":"tee","title":"Write a file","class":"write","destructive":false,"arguments":
  {"path":"/home/a","content":"40 bytes"},"timeout_s":90}` on stdin draws 480x200 with the name
  at (16, 16), `id phone` ending at 464, `tee · Write a file` at (16, 40), `path: /home/a` at
  (16, 68), `content: 40 bytes` at (16, 88), the countdown `Denied in 1:30 if you do nothing.`
  at (16, 132) and buttons at (36, 156), (140, 156), (264, 156), (368, 156), `Deny` primary
  and focused; Enter answers `deny`, Esc `deny`, Space on `Once` `once`; `Always` writes
  `phone`'s `write` to `allow` and answers `always`; with `"class":"run"` or
  `"destructive":true` `Always` is hidden and Tab skips its place; after 90 fake seconds the
  answer is `timeout`; the line reads `1:29` after one second; `wm act` on any button answers
  `element_cannot`.
- A pane 400 wide lays the right column's widgets under the list and the class radios under
  their lines; the prompt at 360 wide is a sheet with four stacked 40 px buttons, `Deny` last.
- `agent list --json`, `agent_list` and the pane agree row for row and class for class on the
  fake; the pane's record rows and `agentlog --json`'s records agree record for record.

## Left out of 0.3.0

Naming and renaming an agent in the pane (`--name` stays the CLI's); ends other than 15 min,
2 h and 7 d, and seconds (`--for 30s` is the CLI's); re-choosing a chosen permission to change
its end; agents seen in the log but not yet granted offered as rows; deleting a damaged store
from the pane; paging past the newest 32 records and live updates of the log on a tick; a
`--until` filter; a tool picker instead of a typed name; the full stamp and arguments in a
row (a label holds 64 bytes); mnemonics in the prompt; remembering `once` answers; the prompt
on a locked screen; per-tool grants; pairing network agents from the pane (41d); a middle
ellipsis for long paths in the prompt; a toolkit list with dynamic row text (the `Table` of
`devices-v0.3.md`), which would replace the labels-over-a-list of section 2.1.
