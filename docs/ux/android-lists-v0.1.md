# EggOS v0.1 Android layer: lists with the app's own rows: design brief

Design by Claude Fable (D16) for the Android layer's `ListView` and `RecyclerView`
(`userland/android`, D508, D511, D516) once rows are shown **as themselves**: each row is the
app's row view (text views, buttons, image views, nested linear layouts), laid out and drawn
as the layer already draws a content view (D500, D509), inside a scroll area of the list's
own. Today's text list (one string a row, 32 rows, `Kind::list`) is replaced; nothing an app
does changes. The look is the toolkit's (`toolkit-v0.3.md` sections 6 and 8, `toolkit-v0.5.md`
section 11, `toolkit-v0.6.md` section 7) in the four palettes of `macos-gui-v0.3.md` section
20: no new token. Numbers are px at 1x in the app window's content coordinates (the window is
360x640, `WINDOW_SIZE`; a dp is a px at 1x) and double at 2x; the one motion is a touch fling
(section 3), off under `accessibility.reduce_motion`.

## 0. What a person gets, and how that is checked

- An Android app's list looks like the app drew it: its own rows, images and buttons, in a
  scrolling area with EggOS's scroll bar.
- A tap or click on a row does what the app's row click does; a tap on a button inside a row
  presses that button only.
- With a keyboard: Up/Down walk the rows, Enter opens one, Tab reaches the buttons inside it.
- A thousand rows cost what the screen shows: rows are made for what is visible and reused.
- Host tests pin the layout, the states, the clicks, the keys, the recycling counts and the
  element tree (section 9).

## 1. The list's area

- **Frame**: the list view's rect from the layout as any view's (D500): a `match_parent`
  height is what remains of the window under the views above it; `wrap_content` is the sum
  of its rows' heights, capped at what remains. Fill: the list's `android:background` when
  set, else the window's surface (`field.bg`, `#0B0E13` dark / `#FFFFFF` light).
- **Scroll bar**: the toolkit's (`toolkit-v0.3.md` section 8): 8 px inside the right edge,
  full height, `progress.track` with a `bar.pressed` thumb (`button.inactive` hovered or
  dragged; at rest too in high contrast), thumb length `view_h x view_h / content_h`, at
  least 24, drawn only when the content is taller than the view; rows keep the full width
  less 8 from the first row on when it shows. `content_h` is section 4's estimate.
- **Rows** are laid out top to bottom at `y_r = y_0 + sum(h_0 .. h_{r-1}) + sum(offsets) -
  offset_y`, each `h_r` tall (section 4), the list's full width (less the bar), clipped to
  the list's frame.
- **Horizontal lists** (`LinearLayoutManager` with `HORIZONTAL`): the same with columns side
  by side, the bar along the bottom 8 px, Left/Right in place of Up/Down.

## 2. The row's states, and a click

| State | Fill under the row view | Also |
|---|---|---|
| normal | the list's surface | the row's own views draw over it; a row's own `background` colour replaces the fill (and hides the states below, as on Android) |
| hover (pointer, desktop) | `menu.hover` | none on touch or TV |
| pressed (button down or finger on the row, not on a clickable child) | `button.pressed` | the toolkit's capture: pressed while inside, nothing while outside |
| current, list focused | `selection` | the toolkit's 2 px `focus.ring` at inset 0 of the row, full width less the bar |
| current, list unfocused | the surface | nothing: Android keeps no visible selection outside focus |
| disabled (`isEnabled()` false on the row view, or `areAllItemsEnabled`/`isEnabled(position)` false) | the surface | the row takes no press, no hover; Up/Down skip it |

No `>` mark column: the row is the app's, from edge to edge; the ring and the `selection` fill
are the keyboard cues (D22's non-colour rule is met by the ring). The fill is drawn under the
row view, so a row whose views leave pixels bare shows it, as the macOS cell views do
(`macos-tables-v0.4.md` section 3).

**Which click is whose.** A press starts on the deepest view under the point:

- a **clickable child** (`Button`, `ImageButton`, `CheckBox`, `Switch`, `RadioButton`, or any
  view with a click listener or `android:onClick`): the press is the child's: its own pressed
  look (the toolkit's for buttons), its own `onClick` on release inside it; the row shows no
  pressed fill and gets no click;
- anything else (a `TextView`, an `ImageView`, the row's layout, bare surface): the press is
  the **row's**: the row's pressed fill, and on release inside the row, `ListView`:
  `onItemClick(list, rowView, position, id)` with `id` from `getItemId(position)`; `RecyclerView`:
  the item view's click listener (`itemView.setOnClickListener` in `onBindViewHolder`), or
  nothing when the item view has none;
- a **long press** (500 ms, `LONG_PRESS_MS`, no move past 6 px) on the row: `onItemLongClick`
  (`ListView`, when a listener is set; a `true` answer swallows the click) or the item view's
  long-click listener; on a child, the child's.

A release outside what was pressed does nothing. A drag past 6 px (`DRAG_THRESHOLD`) on touch
turns the press into a scroll and nothing is clicked. Pointer shapes: `Arrow` everywhere
(`Hand` over a clickable child, as the toolkit's buttons).

## 3. Scrolling

- **Wheel**: 72 px a notch (the toolkit's), in pixels, clamped (rows vary in height, so no
  snapping to rows). **Scroll bar** drag and track clicks as the toolkit. **Keys**: section 5.
- **Touch drag**: 1:1 with the finger from `DRAG_THRESHOLD`; the press under it is cancelled.
- **Fling**: at release, the velocity over the last 100 ms (`FLING_SAMPLE_MS`); while above
  50 px/s (`FLING_STOP`) the offset moves by `v x 16 / 1000` per frame and `v` is multiplied
  by 0.95 (`FLING_DECAY`), stopping at the ends with no overscroll glow; a new press stops it.
  Under `accessibility.reduce_motion` there is no fling: the list stops where the finger
  lifts.
- `smoothScrollToPosition`, `scrollToPosition`, `setSelection(position)` and
  `scrollToPositionWithOffset` set the offset so that row's top is at the list's top (clamped);
  no animation.

## 4. Rows made, reused, measured

- **Made rows**: the rows whose rects touch the list's frame plus `OVERSCAN` 2 above and 2
  below. When a row leaves that window its view goes to a pool per view type (`RECYCLE_POOL`
  8 a type; past that it is dropped); a row entering it takes a pooled view of its type when
  one exists.
  - `RecyclerView`: `getItemViewType(position)` first; `onCreateViewHolder(list, type)` only
    when the pool for that type is empty; `onBindViewHolder(holder, position)` every time a
    row enters the window. `notifyDataSetChanged` rebinds the made rows and re-asks the count;
    `notifyItemChanged(p)` rebinds that row; `notifyItemInserted`, `notifyItemRemoved` and
    `...Range...` shift the rows below and make or drop rows as needed; no animation.
  - `ListView` with an app adapter: `getItemViewType` and `getViewTypeCount` (D511's adapters
    without them are one type); `getView(position, convertView, list)` with a pooled view as
    `convertView` or `null`; `notifyDataSetChanged` as above.
  - `ArrayAdapter` of strings (D508): the layer's own one-text row, 40 tall, the text at
    (16, +12) in `text`.
- **Heights**: a row is measured when bound, as its layout says (`wrap_content` is its
  children's stack; a fixed `layout_height` is taken; `match_parent` is the list's height),
  at least `LIST_ROW_MIN` **40** (a touch target; Android's 48 dp rows are taller than that
  anyway). The list keeps the heights of the made rows and, for the rest, the **last measured
  height of their type** (`getItemViewType` is asked for the estimate), so `content_h`, the
  bar and `scrollToPosition` are estimates that settle as rows are made, Android's own rule.
  A row whose bound height changes (text set after binding) re-lays the rows below it in the
  same frame.
- **Cost**: at most `(visible rows + 4) x` the row view's widgets live in the tree; a
  million-row adapter is asked `getItemCount` once and bound for what shows.

## 5. Keys and focus

The list is a Tab stop (the toolkit's lists are), in the content view's order.

| Key | Does |
|---|---|
| Up / Down | the current row moves one, skipping disabled rows, scrolling to keep it fully visible (a row taller than the list shows its top) |
| Home / End | the first / last row |
| PageUp / PageDown | by the rows fully visible less one |
| Enter, Space | the row click of section 2 (`onItemClick` / the item view's click); Space while a child is focused presses the child |
| Tab | into the current row's focusable children, first to last (buttons, check boxes, editable texts, image buttons), then out of the list to the next widget; Shift+Tab the reverse |
| Up / Down while a child is focused | leave the child: the row moves and takes the focus |
| Esc | nothing (the window's) |
| Left / Right | in a grid (section 7) or a horizontal list, the neighbour cell; else nothing |

A focused child shows its own ring; the row keeps the `selection` fill while any of its
children is focused. The current row is row 0 when the list is first focused, kept across
rebinds while in range (else clamped). A pointer or touch press makes the pressed row current
only while the list is focused by it (the fill shows from the next focus).

## 6. Item decorations and dividers

- **`DividerItemDecoration`** (vertical): a 1 px `menu.border` line across the row's width
  (less the bar) on the row's last pixel row, every row but the last (Android's default); with
  `setDrawable(ColorDrawable)` the drawable's colour and its intrinsic height (at least 1, at
  most 8) replace them. Horizontal orientation: a 1 px column after each column.
- **`ListView`'s `android:divider` / `dividerHeight`**: the same line, `dividerHeight` tall
  (0 draws none); a colour drawable's colour; any other drawable: `menu.border`.
- **Offsets** (`ItemDecoration.getItemOffsets` through `outRect`; a spacing decoration): each
  side's offset is empty surface around the row (the row view is inset by them; the row's
  states fill the inset rect only), added to the row's pitch; the common `top = 8` style
  gives 8 px of surface between rows.
- **`onDraw` / `onDrawOver`** of a custom decoration: not run (the `Canvas` is not the
  layer's); listed in "Later".
- Dividers and offsets are drawn over the surface and under the rows' views, after the state
  fills.

## 7. Grids

- **`GridLayoutManager(context, spanCount)`**, vertical: cells `cw = (list width - bar) /
  spanCount` wide (the last column takes the remainder), a cell row as tall as its tallest
  bound cell (`LIST_ROW_MIN` at least); cells are laid out row-major, left to right; gaps come
  only from decorations' offsets (section 6). `spanSizeLookup` is honoured: a cell of span
  `s` is `s x cw` wide, and a row breaks when a cell does not fit, as Android's.
- **States** per cell as section 2 (the fill and the ring on the cell's rect); the click rules
  are per cell.
- **Keys**: Left/Right move one cell, wrapping to the previous/next row's end/start;
  Up/Down move by one cell row keeping the column (clamped to the row's last cell); Home/End
  the first/last cell; PageUp/PageDown by the cell rows fully visible less one.
- **Made cells**: section 4 by cell rows; the pool is shared.
- **`StaggeredGridLayoutManager`**: **deferred**. It is drawn as a `GridLayoutManager` of the
  same span count with uniform cell rows (every cell of a row as tall as the tallest), listed
  as a deviation in "Later"; a `GridLayoutManager` with `HORIZONTAL` is a grid of columns with
  the bar along the bottom.

## 8. Empty lists, and what an agent sees

- **Empty** (`getCount()`/`getItemCount()` 0): `ListView.setEmptyView(view)` shows that view
  in the list's frame instead (hidden once rows exist), as Android; otherwise the surface and
  nothing else. The layer invents no empty text: the words are the app's.
- **Elements** (D161, `wm show`): the list is a `list` named by its `contentDescription`,
  else its id's name (`recycler_view`), value `<n> items`; each **made** row a `list-item`
  child, name the row's text views' texts joined by `, ` (the first text alone as the value's
  head), value `<position + 1> of <n>` with `current` on the current row and `disabled` on
  a disabled one; a row's clickable children are its children elements (`button`, `checkbox`,
  `switch`, `text field`) with their own names and actions. Rows not made are not listed, so
  an agent scrolls first: `set-text "<position>"` on the list is `scrollToPosition`;
  `select <i>` makes row `i` current and scrolls to it; `invoke` on a row is the row click;
  `invoke` on a child presses it. A grid lists its cells the same way, value `<cell> of <n>`.
  A screen reader reads `<row's texts>, <i> of <n>` as the current row moves.

## 9. Acceptance (host tests, the 360x640 window, scale 1x, dark palette unless said; a fake adapter of 1000 rows, each a `LinearLayout` with a `TextView` `Row <i>` and a `Button` `Open`, 56 tall)

- The list at (0, 0, 360, 640) with `match_parent`: rows at `y = 56 r`, 352 wide (the bar at
  352..359, thumb length 24 at the top); 12 rows made at offset 0 (11 visible + overscan 2
  below, clamped above); `onCreateViewHolder` called 12 times, `onBindViewHolder` 12; one
  wheel notch (offset 72): row 0 leaves the made window (its bottom 56 is above the overscan's
  start), row 13 enters, `onCreateViewHolder` is not called again (the pool answers),
  `onBindViewHolder` once more; 200 notches of scrolling create at most 8 + 12 holders in all.
- A press at (100, 28) and release: the row's `button.pressed` fill while down, then
  `onItemClick`/the item view's listener with position 0 and no button press; a press on
  `Open` of row 1 at its rect: the button's pressed look, its `onClick`, no row click; a
  long press at (100, 28) for 500 ms: `onItemLongClick`; a touch drag of 20 px: no click,
  offset 20; a fling at 1000 px/s: the offset after 16, 32, 48 ms per the decay, stopped
  under 50 px/s; with `reduce_motion` the offset stays at the lift.
- Focused by Tab: row 0 `selection` with the ring at (0, 0, 352, 56); Down 15 times: row
  15 current, the offset so its bottom is at 640; PageDown: row 25; End: row 999 with the
  bar's thumb at the bottom; Enter: the row click of 999; Tab: `Open` of row 999 ringed;
  Shift+Tab back to the row; Tab from the row's last child leaves the list.
- Mixed types: rows of type 1 (every fifth) 96 tall: `y_r` by the prefix sums, `content_h`'s
  estimate before and after row 5 is made, the thumb's length changing then and not again.
- `DividerItemDecoration`: pixel (100, 55) `menu.border`, (100, 54) the surface; a spacing
  decoration of top 8: row 1 at y 64 with rows 56..63 the surface; `dividerHeight` 0 draws
  nothing.
- A grid of span 3: cells 117, 117 and 118 wide at x 0, 117, 234; Right from cell 2 goes to
  cell 3 on the next row; Down from cell 1 to cell 4; a span-3 lookup on row 0 makes it full
  width.
- `setEmptyView` showing its text at 0 items and hidden at 1; `wm show` listing the `list`
  `1000 items`, 12 `list-item`s named `Row 0, Open` ... with `1 of 1000`, each with a
  `button` `Open`; `invoke` on row 3 giving the row click; `set-text "500"` making rows 500..
  the made ones.
- The four palettes' pixels of a pressed, hovered and current row hashed; the TV form (no
  hover, the ring always on the current row, OK the click); the phone form (no hover, a tap
  the click, the 40 px minimum).

## Decided by design (the owner can overrule any of these)

1. No mark column: the app's row spans the width; the ring and `selection` are the keyboard
   cues, and nothing persists after a tap, as on Android.
2. A press on a clickable child is the child's alone; everything else in the row is the row's.
3. Rows are made for the visible range plus two each side, pooled per type, eight a type.
4. Unmade rows are estimated at the last measured height of their type.
5. The minimum row height is 40; dividers are 1 px `menu.border` by default.
6. Touch flings with a 0.95-per-frame decay, none under reduce motion; the wheel scrolls 72
   px with no row snapping.
7. Tab enters the current row's children; Up/Down always move rows.
8. `StaggeredGridLayoutManager` is drawn as a uniform grid for now; custom decoration drawing
   is not run; no empty text is invented.

## Later

Staggered grids laid out as such; custom `ItemDecoration.onDraw`; item animations; swipe and
drag helpers (`ItemTouchHelper`); sticky headers; `ExpandableListView`; section indexers and
fast scroll; nested scrolling (a list inside a scroll view); `setSelector` drawables;
choice modes (`CHOICE_MODE_SINGLE`/`MULTIPLE` with checked rows); `smoothScroll` motion.

## Decisions for the engineer

1. A list is a toolkit scroll area of its own (`Kind::scroll` with a child container per made
   row), bar 8 px, `OVERSCAN` 2, `RECYCLE_POOL` 8 a type, `LIST_ROW_MIN` 40, `LONG_PRESS_MS`
   500, `DRAG_THRESHOLD` 6, `FLING_SAMPLE_MS` 100, `FLING_DECAY` 0.95 a frame, `FLING_STOP` 50.
2. Row rects by prefix sums of made heights plus the type estimate; decorations' offsets added
   to the pitch; dividers 1 px `menu.border` or the colour drawable's colour and height (1..8).
3. States of section 2 drawn under the row's container; hit test deepest-first, clickable
   children first; `onItemClick(list, rowView, position, getItemId)` / the item view's
   listener; long press.
4. Keys of section 5; the current row with the ring; Tab into children; the row's fill kept
   while a child is focused.
5. Grids: `cw = (width - bar) / span`, remainder to the last column, `spanSizeLookup`, keys of
   section 7; staggered as uniform.
6. Elements of section 8: `list` `<n> items`, `list-item` `<i> of <n>`, children; `set-text
   "<position>"`, `select`, `invoke`.
7. No fling under `accessibility.reduce_motion`; no hover on touch and TV.
