# EggOS v0.1 macOS layer: NSTabView, NSSplitView, their controllers, and links in text: design brief

Design by Claude Fable (D16) for the macOS layer's two container views and the two
controllers that wrap them, plus the look of `NSLinkAttributeName` runs. It extends
`macos-gui-v0.3.md` (section 0's coordinates, the tokens of sections 1 and 20, the toolkit's
button and focus ring, section 19's device classes) and `macos-tables-v0.4.md`; nothing
there changes. Every control is **ours**, drawn from the tokens the toolkit draws with, not
Apple's art: a tab strip is the toolkit's segmented row, a divider the toolkit's separator.
Numbers are px at 1x in window content coordinates (a view's frame converted by section 0;
`(x, y, w, h)` below is the converted, top-left rect) and double at 2x; no motion.

Light values quoted inline are section 20's; the dark value follows in brackets. Every token
has its two high-contrast values there too, and nothing below branches on the palette.

| Token | Light | Dark | Drawn here as |
|---|---|---|---|
| `window.bg` | `#FFFFFF` | `#12161D` | the content around a tab view, a split view's panes |
| `menu.bg` | `#F4F6F9` | `#1C2430` | the content box, a thick divider, the sidebar pane |
| `menu.border` | `#C9D0DA` | `#2A3340` | the box's line, dividers, the strip's inner separators |
| `button.bg` / `button.border` | `#FFFFFF` / `#B8C1CC` | `#2A3440` / `#3A4454` | a tab at rest, the strip's outline |
| `button.hover` / `button.pressed` | `#F0F3F7` / `#DCE2EA` | `#344050` / `#202833` | a tab hovered, pressed |
| `selection` | `#CFE0F5` | `#2E4A6B` | the selected tab |
| `text` / `text.dim` | `#161B22` / `#5B6675` | `#E6EAF0` / `#8A94A6` | tab labels; dim when disabled, and the hovered divider |
| `focus.ring` | `#9A6200` | `#F2B33D` | the ring on the selected tab of a focused strip |
| `linkColor` | `#1F5FBF` | `#4F8FE0` | link runs (section 4) |

## 1. NSTabView

### 1.1 Parts and geometry

- **Items**: `NSTabViewItem` with `label`, `view`, `identifier`, `tabView`; `addTabViewItem:`,
  `insertTabViewItem:atIndex:`, `removeTabViewItem:`, `tabViewItems`, `numberOfTabViewItems`,
  `indexOfTabViewItem:`, `tabViewItemAtIndex:`, `selectedTabViewItem`, `selectTabViewItem:`,
  `selectTabViewItemAtIndex:`, `selectFirstTabViewItem:`, `selectNextTabViewItem:`,
  `selectPreviousTabViewItem:`, `delegate`, `tabViewType`, `controlSize` (stored), `font`
  (stored; Egg Sans 16 always), `drawsBackground`.
- **The strip** (`NSTopTabsBezelBorder`, the default): a segmented row **28 tall**
  (`TAB_STRIP_HEIGHT`), its segments side by side with no gap, the row's outer edge a 1 px
  `button.border` line with `INSET_R4` corners (the toolkit's button bezel), a 1 px
  `menu.border` separator between segments (column `seg_right`, rows 6..21, as a table
  header's). Segment `i` is `max(48, label width + 24)` wide (`TAB_MIN_WIDTH` 48, 12 px each
  side of the label); when the sum passes `w - 16` every segment is cut to `(w - 16) /
  items` and its label end-ellipsised at 12 from its right edge (a label narrower than 4
  cells keeps its first glyphs, no `...`). The strip is **centred** on the frame's width:
  `strip = (x + (w - sw) / 2, y, sw, 28)`. Label cell top `+ 6`, centred in the segment.
- **The content box**: a 1 px `menu.border` rectangle with `INSET_R4` corners, filled
  `menu.bg` (`drawsBackground` NO: no fill, the line only), its rect `(x, y + 14, w, h - 14)`
  so the strip straddles its top line, Apple's arrangement. `contentRect` (the selected
  item's `view` frame) is the box inset 8 on the left, right and bottom and **`y + 36`** at
  the top (28 of strip + 8); the view is sized to it at every layout (`setFrame:` by the app
  is overwritten, as AppKit does) and clipped to it; only the selected item's view draws and
  takes hits. `NSTopTabsBezelBorder` is the default `tabViewType`.
- **Other types**:

  | `tabViewType` | Strip | Box | Content inset |
  |---|---|---|---|
  | `NSBottomTabsBezelBorder` | `(x + (w - sw) / 2, y + h - 28, sw, 28)` | `(x, y, w, h - 14)` | 8, 8, top 8, bottom 36 |
  | `NSLeftTabsBezelBorder` | vertical: segments stacked, each 28 tall, all `sw = max(48, widest label + 24)` wide (at most 160), the strip `(x, y + (h - sh) / 2, sw, sh)` with `sh = 28 x items`; separators are rows between segments, columns 6..`sw - 7` | `(x + sw / 2, y, w - sw / 2, h)` | left `sw / 2 + 8`, others 8 |
  | `NSRightTabsBezelBorder` | the mirror: strip at `x + w - sw` | `(x, y, w - sw / 2, h)` | right `sw / 2 + 8` |
  | `NSNoTabsBezelBorder` | none | `(x, y, w, h)`, filled `menu.bg` | 8 all round |
  | `NSNoTabsLineBorder` | none | the line only, no fill | 8 all round |
  | `NSNoTabsNoBorder` | none | nothing drawn | 0: the content is the frame |

  A left or right strip's labels are horizontal, end-ellipsised at `sw - 12`. A strip taller
  than `h - 16` cuts each segment to `(h - 16) / items` (at least 20) and clips labels.
- **Empty**: no items draw the box alone (and no strip).

### 1.2 Segment states

| State | Fill | Label | Also |
|---|---|---|---|
| rest | `button.bg` | `text` | |
| hover (pointer in the segment, no button down) | `button.hover` | `text` | not on the selected one |
| pressed (button down in the segment, pointer inside) | `button.pressed` | `text` | |
| **selected** | `selection` | `text` | no separator on its two sides (its fill reaches the neighbours) |
| selected, strip focused | `selection` | `text` | the 2 px `focus.ring` at inset 2 of the segment's rect (`INSET_R4`), the toolkit's ring |
| disabled (the whole tab view, 1.4) | `button.bg` | `text.dim` | the selected one `selection` with `text.dim`; no hover, no press |

The selected tab is told by fill and by being the one whose content shows; `selection` is
the same cue a selected row uses, 7.5:1 and 12.9:1 under `text` (`toolkit-v0.6.md` section
1). The pointer is `Arrow` over the strip and the box.

### 1.3 Selecting

- **Pointer**: a click (press and release in one segment) selects it; the toolkit's capture
  rules. The delegate's `tabView:shouldSelectTabViewItem:` NO refuses (pressed look ends,
  nothing else); else `tabView:willSelectTabViewItem:`, the selection changes (the old item's
  view hidden, the new one's placed and drawn), `tabView:didSelectTabViewItem:`. The same
  messages for every programmatic `select...` call (AppKit sends them). A click on the
  selected segment does nothing. `tabViewDidChangeNumberOfTabViewItems:` after add and
  remove; removing the selected item selects the nearest earlier one (the next when it was
  first), as AppKit.
- **Ctrl+Tab / Ctrl+Shift+Tab**, from anywhere in the window: the tab view that contains the
  first responder (else the first tab view added to the window) selects its next / previous
  item, wrapping, with the delegate messages; the first responder stays where it was unless
  it was in the hidden view, then the tab view takes it. Ctrl+Tab never reaches the
  program's `keyDown:`.
- **Focus** (`acceptsFirstResponder`): on the desktop the strip is **not** a Tab stop,
  macOS's rule without Full Keyboard Access (section 2's rule for buttons); on the TV, and
  on the desktop once Full Keyboard Access is a setting (section 21), it is, after the
  controls added before it: the ring shows on the selected segment, Left/Right (Up/Down for
  a left or right strip) select the previous/next item wrapping, Home/End the first/last,
  Space nothing, Tab leaves. A tab view that cannot be selected into never takes the ring.
- **Disabled**: `NSTabView` has no `enabled`; the look of 1.2's last row applies when the
  window is disabled by a modal sheet or alert, and is ready for an enabling flag later.

### 1.4 Worked example

A tab view at frame (16, 16, 448, 328) in a 480x360 window with `General` (56 px wide),
`Advanced` (72 px) and `About` (46 px): segments 80, 96 and 70 wide, `sw` 246; strip at
(117, 16, 246, 28): `General` segment columns 117..196, `Advanced` 197..292, `About`
293..362; separators at columns 196 and 292 (rows 22..37) except beside the selected one;
labels at (129, 22), (209, 22), (305, 22). The box (16, 30, 448, 314) with its line at rows 30
and 343, columns 16 and 463; `General`'s view at (24, 52, 432, 284). With `Advanced`
selected: fill `selection` over 197..292, rows 16..43, no separator at 196 or 292; a
focused strip rings (199, 18, 92, 24). Ctrl+Tab: `About`.

## 2. NSSplitView

### 2.1 Parts and geometry

- `vertical` (YES: panes side by side, dividers vertical lines; NO, the default: panes
  stacked), `dividerStyle`, `arrangedSubviews` / `addArrangedSubview:` / `insertArrangedSubview:atIndex:`
  / `removeArrangedSubview:` (and plain `addSubview:` as AppKit's arranged-by-default),
  `dividerThickness`, `adjustSubviews`, `setPosition:ofDividerAtIndex:`, `minPossiblePositionOfDividerAtIndex:`,
  `maxPossiblePositionOfDividerAtIndex:`, `isSubviewCollapsed:`, `holdingPriorityForSubviewAtIndex:`
  (stored), `autosaveName` (stored; saving: later), `delegate`.
- **Dividers**, drawn by the split view between pane `i` and `i + 1`, across the full other
  dimension:

  | `dividerStyle` | `dividerThickness` | Drawn (vertical split: columns; horizontal: rows) |
  |---|---|---|
  | `NSSplitViewDividerStyleThin` (default) | 1 | one `menu.border` line |
  | `NSSplitViewDividerStyleThick` | 9 | `menu.bg` with a `menu.border` line on each outer edge, and a grip: three 2x2 `text.dim` dots centred, 4 apart along the divider (dot centres at `mid - 4`, `mid`, `mid + 4`) |
  | `NSSplitViewDividerStylePaneSplitter` | 9 | as thick |

  Pane `i`'s frame is what is left between dividers; panes are laid out along the split
  axis in order and fill the other axis. The layer never draws a pane's background: a pane
  is the app's view (or a controller's sidebar, section 3).
- **The pointer area** is **9 px** centred on the divider (`SPLIT_HANDLE`: `mid - 4 .. mid +
  4`), whatever is drawn, so a 1 px line is as easy to grab as a thick one (the table
  handle's 7 was for a column; a divider is the only thing in its strip). Pointer
  `ResizeEastWest` for a vertical split, `ResizeNorthSouth` for a horizontal one (section
  13's shapes), from hover; `Arrow` elsewhere. While hovered or dragged a thin divider is
  drawn `text.dim` (the hovered column separator's rule) and a thick one's grip `text`.
- **Minimum pane**: 0 unless the delegate's `splitView:constrainMinCoordinate:ofSubviewAt:` /
  `constrainMaxCoordinate:` say otherwise; `splitView:constrainSplitPosition:ofSubviewAt:`
  rounds a proposed position.

### 2.2 Dragging, resizing, collapsing

- A press in the pointer area starts a drag: every `PointerMove` sets the divider's position
  `clamp(start + d, min, max)` live, re-laying the two panes in the same frame
  (`splitViewWillResizeSubviews:` once at the start, `splitViewDidResizeSubviews:` at every
  change, both `NSNotification`s named as AppKit's); release ends it; Esc during it restores
  the start position and notifies once more. Only the two neighbouring panes change (AppKit's
  behaviour for a drag).
- **Resize of the split view** (the window resized): panes share the change by
  `splitView:resizeSubviewsWithOldSize:` when the delegate implements it, else
  proportionally by `adjustSubviews`, the last pane taking the rounding; a pane under its
  minimum is clamped and the next ones take the rest.
- **Collapsing**: a pane whose `splitView:canCollapseSubview:` answers YES collapses when a
  drag moves its divider past **half its minimum** (its size becomes 0, `isSubviewCollapsed:`
  YES, its view hidden; the divider stays drawn at the edge) and expands back when dragged
  past its minimum again, to the minimum; `splitView:shouldCollapseSubview:forDoubleClickOnDividerAtIndex:`
  YES lets a **double-click** on the divider (two presses within `DOUBLE_CLICK_MS`) toggle
  it, restoring the size it had. A collapsed pane's divider shows its hover look so it can be
  found. `setPosition:ofDividerAtIndex:` collapses the same way when the position is 0 or the
  end.
- **Keys**: none, as on macOS (a split view is not a control and takes no focus; the panes'
  controls keep the Tab loop in the order added). On the TV, dividers do not move (no pointer;
  the positions are the app's defaults); on a phone a pan over a thin divider's 9 px area
  drags it (the toolkit's touch rule: a hit target at least 40 along its length, which a
  divider is).
- **Agents** (D161): each divider is a `splitter` element, name `Divider <i>` (or the
  neighbouring panes' `accessibilityLabel`s joined by ` / ` when set), value its position in
  px, `set-text "<position>"` moves it as a drag does (clamped, collapsing by the same
  rules), `invoke` toggles collapse when the delegate allows a double-click to.

### 2.3 Worked example

A vertical split view at (0, 0, 480, 360), thin, two panes, the delegate's minimum 120 for
the first and 200 for the second: pane 0 (0, 0, 200, 360), the divider column 200, pane 1
(201, 0, 279, 360); the pointer area columns 196..204. A drag from (200, 100) to (150, 100):
pane 0 (0, 0, 150, 360), the divider at 150, pane 1 (151, 0, 329, 360); on to (50, 100): the
divider clamped at 120; on to (40, 100) with `canCollapseSubview:` YES for pane 0: past half
its minimum (60), pane 0 collapses: its view hidden, the divider at column 0 (its pointer
area columns 0..4, drawn in the hover look), pane 1 (1, 0, 479, 360). A double-click on
column 2, with the delegate allowing it, expands pane 0 to 150, its last size.

## 3. NSTabViewController and NSSplitViewController

- **NSTabViewController**: owns an `NSTabView` filling its `view`; `tabViewItems`,
  `addChildViewController:` (wraps the child's view in an item labelled by the child's
  `title`), `selectedTabViewItemIndex`, `tabStyle`:

  | `tabStyle` | Drawn as |
  |---|---|
  | `NSTabViewControllerTabStyleSegmentedControlOnTop` (default) | `NSTopTabsBezelBorder`, section 1 |
  | `NSTabViewControllerTabStyleToolbar` | the server draws every window's chrome and the layer has no toolbar (section 21), so: the same top strip, drawn **above** the box in a 36 px band filled `menu.bg` with a 1 px `menu.border` bottom line, the strip centred in it at `+ 4`, and the box without its top line (`y + 36`, content at `y + 44`); listed as a deviation |
  | `NSTabViewControllerTabStyleUnspecified` | `NSNoTabsNoBorder` |

  `transitionOptions` are stored (no motion). The `NSTabView` delegate messages go to the
  controller, which calls `viewWillAppear`/`viewDidAppear` on the shown child and
  `viewWillDisappear`/`viewDidDisappear` on the hidden one.
- **NSSplitViewController**: owns an `NSSplitView` filling its `view`, `vertical` YES,
  thin dividers; `splitViewItems`, `addSplitViewItem:`, `insertSplitViewItem:atIndex:`,
  `removeSplitViewItem:`; an `NSSplitViewItem` has `viewController`, `behavior` (`Sidebar`,
  `ContentList`, `Default`), `collapsed`, `canCollapse`, `minimumThickness`,
  `maximumThickness`, `preferredThicknessFraction`, `holdingPriority`,
  `collapseBehavior` (stored), `allowsFullHeightLayout` (stored). The controller is the
  split view's delegate and answers from the items.
- **Defaults**: `sidebarWithViewController:` gives `canCollapse` YES, `minimumThickness`
  **160**, `maximumThickness` 320, a first-layout width of **200** (`SIDEBAR_WIDTH`;
  `preferredThicknessFraction` when set, clamped) and fills its pane `menu.bg` behind the
  child's view (the one pane the layer fills: a sidebar reads as a sidebar in every app);
  `contentListWithViewController:` `minimumThickness` 200, no fill;
  `splitViewItemWithViewController:` `minimumThickness` 0, `canCollapse` NO. Sizes are
  pixels at 1x and double at 2x. `toggleSidebar:` (the `View` menu's item, Cmd+Ctrl+S in the
  app's main menu when it has one) collapses or expands the first `Sidebar` item.

## 4. Links: `NSLinkAttributeName` in text fields and text views

The first attribute the layer keeps from an attributed string (section 14.1's "plain text
only" stands for the rest): a run with `NSLinkAttributeName` (an `NSURL` or an `NSString`).

- **Look**: the run's glyphs in `linkColor` (`#1F5FBF` light, `#4F8FE0` dark; section 20's
  one-value-per-theme system colours) with a 1 px underline at the cell's **row 14** (Egg
  Mono's underline row, so links and mnemonics line up), from the run's first glyph's left
  edge to its last glyph's right edge, in the same colour. An `NSForegroundColorAttributeName`
  on the run replaces the colour, the underline stays; `NSUnderlineStyleAttributeName`
  `None` on the run removes the underline (the app's choice, as AppKit). No visited state:
  AppKit keeps no history, and neither does the layer; an app that wants one sets the colour.
- **Hover**: pointer `PointingHand` (section 13's `pointingHandCursor`, the server's `Hand`)
  over the run's glyph rects; no colour or fill change (macOS shows none; the pointer and
  the underline are the cues). **Pressed**: none. A **click** (press and release in the
  run's rects) activates: an `NSTextView` asks `textView:clickedOnLink:atIndex:`, and when
  the delegate answers NO or is absent opens the URL with `NSWorkspace`'s `openURL:`, which
  is the desktop's `open` (an `http` or `https` URL goes to the browser component when
  installed, else the `No app opens this` alert of `files-links-v0.3.md`; a `file` URL to
  Files); an `NSTextField` activates links only when `selectable` or `editable` (AppKit's
  rule) and sends nothing to a delegate. Selecting across a link with the pointer (a drag
  starting in the run) selects, never activates.
- **Keyboard**: none in a text field; in a text view, when the caret is inside a link's run,
  Cmd+Enter activates it (AppKit has no key for this; one is given so the capability is not
  pointer-only, section 19's TV rule). **Agents**: each run is a `link` element under the
  field's or view's element, name the run's text, value the URL; `invoke` activates.
- **Palettes**: `linkColor` on `window.bg` is 5.1:1 light and 6.0:1 dark; on `selection`
  the run keeps its colour (4.0:1 light, 3.6:1 dark, with the underline as the second cue);
  in high contrast the theme's value stands (section 20's rule for system colours) and the
  underline carries the link.

## 5. Device classes (D23)

- **Phone and tablet**: tab segments 40 tall (the strip 40, the box from `y + 20`, content at
  `y + 48`), labels at `+ 12`; a horizontal pan on the strip never scrolls (segments cut to
  fit); dividers' 9 px area holds, thick dividers 12 (grip the same); links take a tap.
- **TV**: the strip takes focus and the arrows (1.3); dividers fixed; the ring always drawn.
- **Wearable**: a tab view shows the selected item's view only, no strip, the next item by a
  swipe (listed); a split view stacks its panes in a scroll.
- **2x**: every number doubles; the grip's dots 4x4.

## 6. Tests

Host tests (`tab_view.rs`, `split_view.rs`, `attributed_string.rs`, `palette.rs`):

- the geometry of 1.4 for each `tabViewType` (strip, box, content rect, separators), the
  cut-to-fit widths for eight 100-px labels in a 448 frame (54 each, ellipsised), the
  segment states hashed (rest, hover, pressed, selected, focused on the TV form factor,
  disabled) in the four palettes; the click and `select...` messages in order,
  `shouldSelectTabViewItem:` NO refusing, removal selecting the neighbour, Ctrl+Tab and
  Ctrl+Shift+Tab wrapping with the first responder kept or moved; only the selected view
  drawing and hit.
- the split view of 2.3: frames, the pointer area and shape at (198, 100), the drag, clamps,
  collapse past half the minimum, the double-click toggle with the size restored, Esc
  restoring, the notifications' count; thick dividers' pixels (lines, grip dots);
  `resizeSubviewsWithOldSize:` versus proportional `adjustSubviews` on a window resize;
  `splitter` elements and `set-text`.
- controllers: the sidebar item's 200 start, 160 minimum, `menu.bg` fill, `toggleSidebar:`;
  the toolbar style's band; the child controllers' appear/disappear calls.
- links: a field with `Egg<OS>` where `OS` carries a link: `OS`'s pixels `linkColor`, the
  underline at row 14 under its columns, the pointer `Hand` at a point in it and `IBeam`
  beside it, a click calling `clickedOnLink:` then `openURL:` when the delegate answers NO,
  a drag selecting without opening, `NSForegroundColorAttributeName` winning, the `link`
  element's name and value; `selectable` NO in a field doing nothing.
- QEMU (`mac-views.app`, a tab view added to section 18's window): a screendump with
  `Advanced` selected after Ctrl+Tab and `wm show` listing the tab view's items.

## Decided by design (the owner can overrule any of these)

1. Tabs are a centred 28 px segmented row straddling the box's top line, the selected one in
   `selection`, not Apple's raised white segment; the box is `menu.bg` with a 1 px line.
2. Segment width is the label + 24, at least 48, cut evenly to fit.
3. The strip is a Tab stop only with Full Keyboard Access (TV); Ctrl+Tab works everywhere.
4. The pointer area of every divider is 9 px; thin dividers turn `text.dim` on hover.
5. Collapse past half the minimum; a double-click toggles when the delegate allows.
6. The toolbar tab style is drawn as a `menu.bg` band over the box (no toolbar exists).
7. The sidebar item starts at 200, minimum 160, maximum 320, and is the one pane filled
   `menu.bg`.
8. Links: `linkColor`, underline at row 14, pointer hand, no hover or visited look, Cmd+Enter
   in a text view, `link` elements for agents.

## Later

Per-tab enabling, tab images and tooltips, `controlSize` small and mini, a toolbar,
`autosaveName` positions kept across runs, `NSSplitViewItem` animations and `collapseBehavior`,
full-height sidebars, the wearable's swipe between tabs, link tooltips showing the URL,
links in the credits of the About panel, visited colours.

## Decisions for the engineer

1. `TAB_STRIP_HEIGHT` 28 (40 touch), `TAB_MIN_WIDTH` 48, segment `max(48, label + 24)`, cut
   to `(w - 16) / items`; strip centred; box at `y + 14`; content inset 8 and top `y + 36`;
   the table of 1.1 for the other types.
2. States of 1.2 with the toolkit's bezel (`button.border`, `INSET_R4`), separators as a
   table header's, ring at inset 2 of the selected segment.
3. Messages in 1.3's order for clicks and every `select...`; Ctrl+Tab in the window's key
   path before `keyDown:`; focus only under Full Keyboard Access.
4. `SPLIT_HANDLE` 9; thin 1 px `menu.border`, thick 9 px `menu.bg` with edge lines and three
   2x2 `text.dim` dots 4 apart; hover `text.dim` / grip `text`; shapes `ResizeEastWest` /
   `ResizeNorthSouth`.
5. Drag clamps from the delegate, live layout, Esc restores, notifications as AppKit;
   collapse at half the minimum, double-click toggles, `set-text` and `invoke` on `splitter`
   elements.
6. `SIDEBAR_WIDTH` 200, minimum 160, maximum 320, `menu.bg` fill; content list minimum 200;
   toolbar style band 36 tall.
7. Links: keep `NSLinkAttributeName` runs; `linkColor`; underline row 14; `Hand`; click →
   `clickedOnLink:` → `openURL:`; fields only when `selectable`/`editable`; Cmd+Enter in text
   views; `link` elements.
