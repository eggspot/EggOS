# EggOS v0.3.0 macOS GUI apps: design brief

Design by Claude Fable (D16) for v0.3.0 items 41 and 44, "macOS GUI apps": how the macOS
layer's own AppKit (`userland/macos`) draws what a programmatic Objective-C app shows. It
extends `desktop-v0.3-gui.md`, `toolkit-v0.3.md` and `windows-gui-v0.3.md`: every token,
metric, primitive and rule there stays. The server draws every window's chrome and title; the
layer draws only content, with the toolkit crate (`userland/toolkit`: Egg Sans 16, the push
button, the text field, the dialog panel, the focus ring). The top bar and the menus under it
are the server's, in its font, Egg Mono 8x16. Every number is in pixels at scale 1 and doubles
at 2x. An AppKit app on EggOS is a macOS app in Dark Mode: its semantic colours resolve to the
desktop's dark tokens, and nothing is drawn light.

## 0. Coordinates

AppKit's origin is the bottom-left and y grows upward; the window buffer's origin is the
top-left. For a window whose content is `H` tall, a view frame or rect `(x, y, w, h)` covers
content columns `x .. x + w - 1` and rows `H - y - h .. H - y - 1`; a point `(x, y)` is the
content pixel `(x, H - 1 - y)`. Views are not flipped (`isFlipped` NO). Only direct subviews
of the content view exist in 0.3.0, drawn back to front in the order they were added; a custom
`NSView` subclass draws in `drawRect:` with `NSRectFill`, `NSFrameRect`, `-[NSColor set]` and
`NSBezierPath`'s `fillRect:`, `strokeRect:` (1 px) and `moveToPoint:`/`lineToPoint:` (1 px,
no anti-aliasing, no alpha), each clipped to the view's frame; `setNeedsDisplay:` redraws and
commits the view's frame only.

## 1. Window content defaults

- **Window.** `NSWindow`'s `contentRect` size is the content size (the server clamps); its
  origin is ignored (the server cascades). `NSWindowStyleMaskResizable` set: resizable, else
  `FIXED_SIZE`. `contentMinSize` is `SET_MINIMUM_SIZE`. `title` is `SET_TITLE`; a window with
  no title shows the app's name (`CFBundleName`, else the executable's name), so the dock,
  the switcher and the catalogue know it. `makeKeyAndOrderFront:` is the first commit. The
  content is filled `window.bg` (`#12161D`, `WINDOW_BACKGROUND`) before the first view draws,
  and again after every `Resize`; `backgroundColor` replaces it.
- **Semantic colours** (`NSColor`), as tokens: `windowBackgroundColor` `window.bg`;
  `controlBackgroundColor`, `textBackgroundColor` `field.bg` `#0B0E13`; `labelColor`,
  `textColor`, `controlTextColor`, `headerTextColor`, `selectedTextColor`,
  `selectedMenuItemTextColor`, `alternateSelectedControlTextColor` `text` `#E6EAF0`;
  `secondaryLabelColor`, `tertiaryLabelColor`, `quaternaryLabelColor`,
  `placeholderTextColor`, `disabledControlTextColor` `text.dim` `#8A94A6`; `controlColor`
  `button.bg` `#2A3440`; `selectedControlColor`, `selectedContentBackgroundColor`,
  `selectedTextBackgroundColor` `selection` `#2E4A6B`; `controlAccentColor`,
  `keyboardFocusIndicatorColor` `accent` `#F2B33D`; `separatorColor`, `gridColor`
  `menu.border` `#2A3340`; `linkColor`, `systemBlueColor` `#4F8FE0`; `systemRedColor`
  `#E0565B`, `systemGreenColor` `#5FB865`, `systemYellowColor` `#E2B341`,
  `systemPurpleColor` `#C46BD2`, `systemTealColor` `#44B9C9` (the ANSI normal row of
  `desktop-v0.3.md`), `systemOrangeColor` `accent`, `systemGrayColor` `text.dim`.
  `blackColor`, `whiteColor`, `redColor`, ... and `colorWithRed:green:blue:alpha:` are their
  exact values, alpha ignored; `clearColor` draws nothing.
- **Text.** `-[NSString drawAtPoint:withAttributes:]` draws Egg Sans 16 with the point at the
  bottom-left of the text's 16-row line box, as Apple does: the cell's top row is content row
  `H - y - 16`, its first glyph at column `x`; `drawInRect:` is the same with the rect's
  bottom-left and clipping. `sizeWithAttributes:` is `(sum of advances, 16)`. **The default
  foreground is `labelColor` (`text`), not Apple's black**: EggOS's window surface is dark, so
  black text with no colour attribute would vanish; an explicit `blackColor` stays black.
- **Fonts.** One face in 0.3.0: every `NSFont` (`systemFontOfSize:`, `boldSystemFontOfSize:`,
  `userFontOfSize:`, `fontWithName:size:`, `menuFontOfSize:`, the fixed-pitch ones too) is
  Egg Sans at 1x for sizes below 28 and at 2x (the toolkit's heading, a 32-row cell) from 28
  up. `pointSize` reports what was asked; `ascender` 12, `descender` -4, `leading` 0,
  `xHeight` 7, `capHeight` 10 (24, -8, 14, 20 at 2x). Bold and a second face: later.

## 2. NSButton

A push button (`buttonWithTitle:target:action:`, `initWithFrame:` with the default type and
bezel) is the toolkit's push button (`toolkit-v0.3.md` section 4: `INSET_R4`, 1 px
`button.border`, `button.bg`, hover `button.hover`, pressed `button.pressed`, text centred
with its cell top at +6) drawn in the frame the app gives: **the frame's full width, 28 tall,
centred vertically in the frame** (top at `frame_top + (frame_h - 28) / 2`, rounded down), so
a 21 or 22 px frame, macOS's usual, draws 3 px past its top and bottom, and a 32 px frame
leaves 2 px above and below. The toolkit's 96 minimum does not apply to an app's frame: a
narrower frame gets a narrower button, its title end-ellipsised at `width - 16`. Hit target:
the drawn 28 px rect. States: the toolkit's. `keyEquivalent` `"\r"` makes it the window's
default button, drawn **primary** (`button.primary`, `icon.dark` title, no border, hover
`button.primary.hover`, pressed `button.primary.pressed`); Enter anywhere in the window
activates it, with no pressed look. At most one default per window (the last set wins).
`setEnabled: NO`: the toolkit's disabled look (`text.dim` title, no hover, no press).
Buttons take no keyboard focus and show no ring (macOS without Full Keyboard Access); Tab
skips them, Space does nothing. A click (press and release inside, the toolkit's capture rule)
sends `action` to `target` (`nil` target: the first responder, then the window's delegate).
`setTitle:` redraws the button's rect. Checkboxes, radios and other `NSButtonType`s: part 2.

## 3. NSTextField

- **Editable, bezeled** (`initWithFrame:`, `textFieldWithString:`): the toolkit's single-line
  text field (section 7 of `toolkit-v0.3.md`: `field.bg`, 1 px `button.border`, `INSET_R4`,
  text inset 8, hover border `text.dim`, 255 bytes) drawn like the button: the frame's width,
  28 tall centred in the frame. The pointer over it is `IBeam`. A click makes it the first
  responder (focus ring, caret at the nearest glyph boundary); typing, Backspace, Delete,
  Left, Right, Home and End as the toolkit; Enter sends `action` to `target` and keeps the
  focus; Esc does nothing; Tab moves the first responder to the next editable field in the
  order added, Shift+Tab back, wrapping (buttons are not in the loop). When the window first
  becomes key, the first editable field added is the first responder (`initialFirstResponder`
  and `makeFirstResponder:` override). `placeholderString` in `text.dim`; `stringValue` reads
  and sets the text (the caret goes to the end). No selection, no clipboard in 0.3.0.
- **Label** (`labelWithString:`, or `editable` and `bezeled` NO with no background): plain
  text in `labelColor` (`textColor` overrides), its cell vertically centred in the frame
  (`frame_top + (frame_h - 16) / 2`, 2x fonts centre their 32), `alignment` left, centre or
  right within the frame's width, end-ellipsised at the frame's width. No border, no fill, no
  hit target, never focused. Wrapping and `maximumNumberOfLines`: later.

## 4. NSAlert runModal

A `Dialog` window (role 1) whose parent is the app's key window (else its most recently
shown window; with no window at all a `Normal` `FIXED_SIZE` window, as the Windows message box
without an owner). The server centres it on the parent, gives it the close control only, and
blocks the parent. Its title bar shows **the app's name** (macOS alerts have no title; the
server always draws one, and the name says who is asking). Content: the toolkit's dialog panel
(`panel.rs`) filling the content exactly, 400 wide, `menu.bg` (`windows-gui-v0.3.md` section 0:
every dialog's surface), its 1 px `menu.border` outline inside the content's edge, its 2 px
outer edge and corner cut-outs off the content and invisible:

| Element | Rule |
|---|---|
| line 1 | `messageText` in `text` at (16, 16); empty text draws an empty line |
| line 2 | `informativeText` in `text.dim` at (16, 36), absent when empty (then one line) |
| lines | each one line, end-ellipsised at 368; no wrap, no icon, `alertStyle` ignored |
| buttons | `addButtonWithTitle:` in macOS order: the first added is the default, rightmost; drawn right-aligned from `right - 16`, 8 apart, 28 tall, top at `20n + 28`; the toolkit's minimum 96 applies (the panel sizes its own buttons) |
| default | the first button, primary; also any button whose `keyEquivalent` is `"\r"` (the last set wins) |
| cancel | the button titled `Cancel` or whose `keyEquivalent` is Escape; none: Esc and the close control do nothing |
| size | content 400 x `20n + 72` (112 for two lines, 92 for one); outside 402 x `20n + 102` |
| limit | 3 buttons (the panel's); a fourth is ignored (listed gap); no button added means `OK` |

Enter activates the default, Esc the cancel, Tab and Shift+Tab move the focus ring between
the buttons, Space presses the focused one (the panel's rules); focus starts on the default.
`runModal` runs the dialog's own event loop and returns `NSAlertFirstButtonReturn` (1000),
1001 or 1002 for the button pressed. Worked example (section 7's app): `mac-paint` /
`A programmatic AppKit app on EggOS.` / `OK`, `Cancel`: content 400x112, `OK` at
(288, 68, 96, 28) primary and focused, `Cancel` at (184, 68, 96, 28).

## 5. The app's main menu in the top bar

`-[NSApplication setMainMenu:]` sends the menu to the server (`SET_MENU`, below) for every
window of the app; the server draws it in the **macOS profile's top bar while one of the
app's windows is focused**, in Egg Mono 8x16, the bar's font (codepoints the font lacks draw
`?`). The menu belongs to the window: a focused window without one shows the bar as today.

- **Titles**: from x 104 (where the Linux profile's chips start; the macOS profile has none),
  each in `text` with its glyph top at row 6 (as `EggOS`), 8 px of padding each side, so a
  title of `n` glyphs has a hit target `8n + 16` wide and the full 28 px height, titles side by
  side with no gap. The first menu is the app's: its title is the app's name whatever
  `NSMenu`'s title says (as AppKit does); it is **not otherwise distinguished** (bold is a
  second weight, later; it is first, and it is the app's name). Titles whose right edge would
  pass the clock's left - 16 (x 596 at 1280 wide) are dropped with those after them. Hover
  `bar.hover`, open `bar.pressed`, on the whole hit target above the edge line.
- **The open menu**: a click on a title opens it; while one is open, hovering another title
  opens that one instead; Left and Right do the same from the keyboard. It is the popup of
  `desktop-v0.3-gui.md` 6.4 item 4 and `menu.rs`: `menu.bg`, 1 px `menu.border`, `INSET_R6`,
  the window shadow, top at y 32 (the power menu's), left edge at the title's hit-target left
  (clamped 8 px inside the screen's right edge), items 32 tall, height `2 + 32 x items + 9 x
  separators`; width `max(160, round up to 8 of (12 + label width + 32 + key label width + 12))`
  over its items, widths in the bar's 8 px glyphs. An item's label at x + 12 in `text`
  (`text.dim` when disabled, skipped by Up/Down, ignored by clicks); its key equivalent at the
  right, ending at `right - 12`, in `text.dim`. A separator is a 9 px band: 4, a 1 px
  `menu.border` line from x 8 to `right - 8`, 4. Hover `menu.hover` over the item's full
  width; Up/Down move it, Enter chooses, Esc closes, a click chooses, a button-down outside
  closes and is delivered normally (a click on the open title only closes). Submenus: later
  (an item with one is drawn disabled).
- **Key labels**: the font has no `⌘`, so modifiers are spelled, in macOS's order, joined by
  `+`: `Ctrl+Alt+Shift+Cmd+X`; a letter upper-cased, `Cmd+,` and `Cmd+.` as they are, and
  `Enter`, `Esc`, `Tab`, `Space`, `Del`, `Up`, `Down`, `Left`, `Right`, `F1` .. `F12`. A
  key equivalent whose mask has no Cmd keeps its other modifiers (`Shift+F5`); an item with
  none shows nothing.
- **Choosing** sends the window a `MenuChosen` event: `a` the menu index, `b` the item index
  (both from 0). The layer sends the item's `action` to its `target` (`nil`: the first
  responder, the window, its delegate, then `NSApp`), and `Cmd`+key events it receives
  (section 6) are matched against the menu's key equivalents the same way, so a shortcut and a
  click do one thing. Key equivalents the server takes first (Cmd+W, Cmd+M, Cmd+Tab,
  Cmd+Space, Super+arrows, Super+D, Super+X) never reach the app; their items are drawn and
  choosing them by click still works.
- **Limits**, so the request stays one small record: at most 6 menus of at most 16 items each
  (separators count), every label and key label UTF-8 in one pool of at most 248 bytes; a
  menu past the limit is cut (`EINVAL` for a label pool that does not fit). The `SET_MENU`
  request: 0: window, then the counts, the per-item flags (enabled, separator) and the pool,
  the layout the engineer's; a count of 0 removes the menu. `setMainMenu:` again replaces it;
  `NSMenuItem`'s `setEnabled:` and `setTitle:` resend it.
- **During a dialog**: while a `Dialog` of the window is focused the titles are drawn in
  `text.dim` and ignore the pointer and keys (macOS disables the menu during `runModal`).
- **Other profiles, 0.3.0**: the Windows profile has no top bar and the Linux profile's top
  bar holds the window list from x 104, so **neither shows the menu**; the app's key
  equivalents still work there with Super as Cmd (the server passes Super+key to the focused
  window in every profile), and Super+Q quits. A menu bar inside the window for those profiles
  is part 2's listed gap.

## 6. Cmd+Q and Cmd+W

- **Cmd+Q** (the Super key, modifier bit 3, with `KEY_Q`, let through to the focused window
  in every profile) quits the app, menu or no menu: the layer asks the delegate
  (`applicationShouldTerminate:`; `NSTerminateCancel` keeps it running) and exits; the server
  destroys the app's windows with its descriptor. `Quit <app>` in the app menu is the same
  path (`terminate:`).
- **Cmd+W** stays the server's close shortcut: the window gets `CloseRequest`, the layer runs
  `performClose:` (`windowShouldClose:` may refuse), and `close` destroys the window. When the
  last window closes the app exits (`applicationShouldTerminateAfterLastWindowClosed` is
  taken as YES in 0.3.0: a windowless app is reachable from nothing yet; listed gap).
- Other Cmd+key presses go to the menu's key equivalents (section 5), then to the first
  responder as `keyDown:`. The pointer is `Arrow` over everything but text fields (`NSCursor`
  shapes: later).

## 7. Tests

`mac-paint.app`, built on CI's macOS runner from our own source like the Objective-C test
program: a `FIXED_SIZE` window `mac-paint`, content 400x300; a `PaintView` frame
(0, 48, 400, 252) filled `textBackgroundColor` whose click fills an 8x8 `systemBlueColor`
square centred on the point and whose `c` key clears it; a text field (8, 10, 184, 28) with
the placeholder `Title` whose Enter sets the window's title; a `Clear` button (200, 10, 96, 28);
an `Info` button (304, 8, 88, 32), `keyEquivalent` `"\r"`, opening section 4's worked alert;
the main menu `mac-paint` (`About mac-paint`, separator, `Quit mac-paint` `Cmd+Q`), `File`
(`Clear` `Cmd+K`, `Close` `Cmd+W`), `Help` (`mac-paint Help`), titles at x 104..191,
192..239 and 240..287.

Host tests render the window at 400x300 and hash it (the field at rows 262..289 with the
caret, both buttons at rows 262..289 with `Info` primary, the view's rows 0..251), the alert
at 400x112 against section 4's table, the key-label spelling, the coordinate rule of
section 0 against `drawAtPoint:` and a frame, and the menu's width rule. The QEMU boot test
on aarch64 and x86_64 compares screendumps with the host's renders after each step, in order:
the window opened and focused (placeholder, caret, the three menu titles in the bar); a click
at content (100, 100) (the blue square at 96..103); `Hello` typed and Enter (the title bar
reads `Hello`); `Clear` clicked (the view empty); `Info` pressed (the alert centred on the
window, `OK` focused, the titles `text.dim`); Esc (the alert gone); the `File` menu opened by
a click on its title (the popup at (192, 32), `Cmd+K` and `Cmd+W` at its right); Esc; then
Cmd+Q: the window gone, the dock without the app, the process ended with status 0.
