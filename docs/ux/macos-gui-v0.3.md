# EggOS v0.3.0 macOS GUI apps: design brief

Design by Claude Fable (D16) for v0.3.0 items 41 and 44, "macOS GUI apps": how the macOS
layer's own AppKit (`userland/macos`) draws what a programmatic Objective-C app shows. It
extends `desktop-v0.3-gui.md`, `toolkit-v0.3.md` and `windows-gui-v0.3.md`: every token,
metric, primitive and rule there stays. The server draws every window's chrome and title; the
layer draws only content, with the toolkit crate (`userland/toolkit`: Egg Sans 16, the push
button, the text field, the dialog panel, the focus ring). The top bar and the menus under it
are the server's, in its font, Egg Mono 8x16. Every number is in pixels at scale 1 and doubles
at 2x. An AppKit app on EggOS is a macOS app in the user's appearance: its semantic colours
resolve to the desktop's tokens, dark by default; section 20 says how they resolve under the
light and the two high-contrast palettes (sections 1 to 8 name the dark values).

## 0. Coordinates

AppKit's origin is the bottom-left and y grows upward; the window buffer's origin is the
top-left. For a window whose content is `H` tall, a view frame or rect `(x, y, w, h)` covers
content columns `x .. x + w - 1` and rows `H - y - h .. H - y - 1`; a point `(x, y)` is the
content pixel `(x, H - 1 - y)`. Views are not flipped (`isFlipped` NO). Only direct subviews
of the content view exist in 0.3.0, drawn back to front in the order they were added; a custom
`NSView` subclass draws in `drawRect:` with `NSRectFill`, `NSFrameRect`, `-[NSColor set]` and
`NSBezierPath`'s `fillRect:`, `strokeRect:` (1 px) and `moveToPoint:`/`lineToPoint:` (1 px,
no anti-aliasing, no alpha), each clipped to the view's frame; `setNeedsDisplay:` redraws and
commits the view's frame only. Section 10 adds one more level: a scroll view's document view
and that view's own subviews.

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
`setTitle:` redraws the button's rect. Checkboxes, radios and the other `NSButtonType`s:
section 9.

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
  and sets the text (the caret goes to the end). Selection and the clipboard: section 14.
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
(288, 68, 96, 28) primary and focused, `Cancel` at (184, 68, 96, 28). What agents see of the
alert: section 16.

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
  responder as `keyDown:`. The pointer is `Arrow` over everything but text fields; `NSCursor`
  and cursor rects: section 13.

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

## 8. The standard About panel

`-[NSApplication orderFrontStandardAboutPanel:]` and `orderFrontStandardAboutPanelWithOptions:`
show the layer's own window, drawn by the layer like section 4's alert, never by the app. The
app menu's first item (`About CocoaGame`, section 5) is its usual caller.

- **Window.** A `Normal` `FIXED_SIZE` window (Apple's panel blocks nothing, so not a `Dialog`),
  content **288 wide**, `140 + 20n` tall for `n` body lines (160 with the version line, 180
  with version and copyright), placed by `SetPosition` at the centre of the work area
  (`GetWorkArea`), as the Windows message box without an owner, not cascaded. Title `About
  <name>` (the name the panel shows; cut at 64 bytes on a character). The server draws its
  three controls with the maximise circle `button.inactive`; minimise works; it has its own
  window-list entry, matched to the app as its other windows are. It gets the app's main menu
  (`SET_MENU`, as `setMainMenu:` sends every window of the app), so the titles stay in the bar
  while it is focused; an item chosen then sends its `action` to its `target`, a `nil` target
  to `NSApp` and the app's delegate (the panel has no responders). The close control, the
  server's close shortcut (Cmd+W) and **Esc** destroy it; the app's delegate is not asked
  (`windowShouldClose:` is for the app's windows; the panel is not in `NSApp.windows`). Enter,
  Space, Tab and every other key do nothing; nothing in it takes focus, no ring is drawn, a
  click does nothing beyond the server's raise, the pointer is `Arrow`. **One per app**:
  `About` chosen while it is open raises and focuses it (section 15.3; this replaces the first
  build's destroy-and-recreate, which stood while a client could not raise a window). It does
  not keep the app running (the app still exits when its last window closes, section 6, and
  the panel goes with the descriptor), and closing it never exits the app; section 15.4 says
  how it nevertheless appears in `NSApp.windows`. Every profile shows the same window.
- **Content.** The alert's surface: `menu.bg`, a 1 px `menu.border` outline inside the
  content's edge, `INSET_R6` corners over the `window.bg` the content is filled with first
  (`window.bg` carries no text, so it is not the surface). Everything is centred on the
  content's width, `x = (288 - width) / 2` rounded down, `width` the drawn (cut) width;
  Apple centres everything too. Body lines from row 128 at pitch 20, the version line first
  when there is one, then the copyright line; a missing line moves the next up.

| Element | Rule |
|---|---|
| icon | 48x48 at (120, 16): the installed app's own icon's 48 image (`app-icons-v0.3.md` section 3), `app.icon` in the folder holding the `.app` (its bundle's parent; a bare executable's own folder), read once when the panel opens, exactly 13 312 bytes or it is no icon; channels clamped to the alpha, blended over `menu.bg` with section 3's formula; never scaled (the dock's pixels at the dock's size) |
| no icon | the generic tile of `desktop-v0.3-gui.md` section 7 with the macOS badge, at (120, 16): r6 `0,0,48,48` `icon.tile`, 1 px `icon.tile.border`, the name's first ASCII letter or digit upper-cased (else `?`) at 2x at (8, 6) in `text`, the badge r4 `30,26,16,20` `icon.mac` with `M` at 1x at (34, 28) in `icon.dark` |
| name | the app's name at 2x (the heading), cell rows 80..111, `text`, end-ellipsised at 256 (`fit` at 128, then doubled) |
| version | `Version <short> (<build>)` at 1x in `text.dim`, cell rows 128..143, end-ellipsised at 256; `Version <short>` when the two are equal or only one exists (then that one); no line when neither |
| copyright | the copyright up to its first `\n` or `\r`, at 1x in `text.dim`, the next line (rows 148..163 after a version line, 128..143 without), end-ellipsised at 256; no line when there is none |

- **Where the parts come from.** The name is `CFBundleName`, else the executable's file name
  (section 1's rule); `<short>` is `CFBundleShortVersionString`, `<build>` `CFBundleVersion`,
  the copyright `NSHumanReadableCopyright`, all from the bundle's XML `Info.plist` (no bundle:
  none of them). The options dictionary overrides them: `ApplicationName` the name, `Version`
  `<short>`, `ApplicationVersion` `<build>`, `Copyright` the copyright; a string value
  replaces, an empty string removes the line (the name falls back), a value that is not a
  string and an unknown key are ignored. `ApplicationIcon` and `Credits`: section 15.1 and
  15.2 (the first build ignored them). `orderFrontStandardAboutPanel:` is the call with no
  options.
- **What agents see** (D161, `wm show`): the layer answers for this window, its first element
  tree (the NSView tree stays a listed gap). Ids in drawing order, so a missing line gives its
  id to the next; no flags; no element takes an action (`element_cannot`).

| Id | Parent | Role | Rect | Name | Value |
|---|---|---|---|---|---|
| 1 | 0 | `window` | `0,0,288,h` | the title | |
| 2 | 1 | `graphic` | `120,16,48,48` | the app's name | `own` or `generic` |
| 3 | 1 | `label` | the name's drawn cell (`x,80,width,32`) | the full name, uncut | |
| 4 | 1 | `label` | the version's drawn cell (`x,128,width,16`) | the full version line | |
| 5 | 1 | `label` | the copyright's drawn cell (`x,y,width,16`) | the full copyright line | |

- **Worked example A, CocoaGame** (`CFBundleName` `CocoaGame`, `1.0.0`, `1`, no copyright):
  content 288x160, outside 290x190, title `About CocoaGame`; icon (120, 16, 48, 48);
  `CocoaGame` at 2x, 112 wide, (88, 80, 112, 32); `Version 1.0.0 (1)`, 81 wide,
  (103, 128, 81, 16). Tree: 1 `window` `About CocoaGame`; 2 `graphic` `CocoaGame` = `own`
  (`generic` when run from outside `/Applications`: then the tile's `C` at (128, 22), its `M`
  badge at (150, 42)); 3 `label` `CocoaGame` at 88,80 112x32; 4 `label` `Version 1.0.0 (1)`
  at 103,128 81x16.
- **Worked example B, mac-paint** (section 7's app, its `Info.plist` gaining
  `CFBundleShortVersionString` `0.3.0`, `CFBundleVersion` `0.3.0` and `NSHumanReadableCopyright`
  `© 2026 EggOS`): content 288x180, outside 290x210, title `About mac-paint`; icon
  (120, 16, 48, 48); `mac-paint` at 2x, 96 wide, (96, 80, 96, 32); `Version 0.3.0`, 65 wide,
  (111, 128, 65, 16); `© 2026 EggOS`, 71 wide, (108, 148, 71, 16). Tree ids 1 to 5, the labels
  `mac-paint`, `Version 0.3.0`, `© 2026 EggOS`.
- **Scale 2x**: every number doubles and the 48 icon is pixel-doubled, as the dock's is.
- **Tests.** Host tests (`about.rs` beside `alert.rs`): the version rule (`1.0.0`/`1` gives
  `Version 1.0.0 (1)`; `0.3.0`/`0.3.0` `Version 0.3.0`; `1.0`/none `Version 1.0`; none/`7`
  `Version 7`; none/none no line and a 140 content, 160 with a copyright); the name fallback
  to the executable's file name; each options key overriding its part, an empty `Copyright`
  removing the line, a number ignored; the copyright cut at its first newline; the sizes and
  every rect of A and B; a 30-letter name cut with `...` at 256 (`fit` at 128); the render of
  A with the generic tile and of B with `app-icons-v0.3.md` test 2's synthetic 48 icon, hashed,
  with the pixels (0, 0) `window.bg`, (4, 0) `menu.border`, (144, 70) `menu.bg`, (121, 40)
  `icon.tile` and (152, 44) `icon.mac` in A; an `app.icon` of 13 311 bytes giving the generic tile
  and a channel above its alpha drawn as the alpha; the tree of A and B (ids, roles, rects,
  names, values) and `element_cannot` for every action; Esc and `CloseRequest` closing, Enter,
  Space and Tab doing nothing, a second `About` making one window, not two. The QEMU boot test
  (section 7) gains, after the `File` menu's Esc: the `mac-paint` title clicked and `About
  mac-paint` chosen: the panel at the work area's centre, focused, titled `About mac-paint`,
  the three menu titles still `text`, the frame compared with the host's render of B (its icon
  the installed `app.icon`'s 48 image); `wm show` on it listing the five elements; Esc: the
  panel gone, the main window focused; then Cmd+Q as before. The CocoaGame check chooses
  `About CocoaGame`, compares the frame with A (its own icon), and closes it with Cmd+W.
- **Later** (as first built; every item below is now designed in section 15): `ApplicationIcon`
  and `Credits`, raising the existing panel instead of re-creating it, `CFBundleGetInfoString`,
  the panel in `NSApp.windows`.

## 9. NSButton: checkboxes, radios and toggles

Design by Claude Fable for the macOS layer's second GUI slice (sections 9 to 21). Every
`NSButtonType` draws as one of three toolkit controls; `setButtonType:` after creation changes
the drawing, `initWithFrame:` starts as `NSButtonTypeMomentaryPushIn`. `bezelStyle`, images and
`imagePosition` change nothing in this slice (every bezel is the push button). All of them
keep section 2's rules where this section says nothing else: the control is **28 tall,
centred vertically in the app's frame** (`top = frame_top + (frame_h - 28) / 2`, rounded
towards minus infinity, so an 18 px frame, macOS's default for a checkbox, draws 5 px past
its top and bottom), takes no keyboard focus, shows no ring, ignores Space and Tab, and sends
`action` to `target` on a click (press and release inside, the toolkit's capture rule), after
its state has changed. `setEnabled: NO` is the toolkit's disabled look and ignores the pointer.

### 9.1 The three families

| `NSButtonType` | Drawn as | `state` |
|---|---|---|
| `MomentaryPushIn` (default), `MomentaryLight`, `MomentaryChange`, `Accelerator`, `MultiLevelAccelerator` | section 2's push button | stored and returned, never drawn; `MomentaryChange` shows `alternateTitle` while pressed when it is not empty |
| `PushOnPushOff`, `OnOff`, `Toggle` | the push button with an **on** look (9.4) | flips on every click; `Toggle` shows `alternateTitle` while on when it is not empty |
| `Switch` (`checkboxWithTitle:target:action:`) | the toolkit's checkbox (9.2) | off, on, or mixed with `allowsMixedState` |
| `Radio` (`radioButtonWithTitle:target:action:`) | the toolkit's radio (9.3) | on or off; one on per group |

`state` is `NSControlStateValueOff` 0, `On` 1, `Mixed` -1; `setState:` stores the value
(clamped to on when mixed is given without `allowsMixedState`), redraws the control's drawn
rect and sends nothing. `setNextState` and a click move a checkbox **on, off, mixed, on** with
three states (Apple's `nextState` order) and on, off, on with two. `keyEquivalent` `"\r"` on a
checkbox, radio or toggle is stored and never makes it the default button; `integerValue`,
`intValue` and `objectValue` read and write `state`.

### 9.2 Checkbox (`NSButtonTypeSwitch`)

The mark is the toolkit's 16x16 box (`toolkit-v0.3.md` section 5) at the control's left edge,
its top at `top + 6`; the title's cell starts 24 px right of the frame's left (`frame_x +
24`), cell top `top + 6`, end-ellipsised at the frame's right edge (`frame_w - 24` of room; a
frame narrower than 24 draws the box alone). Hit target: `(frame_x, top, max(frame_w, 16),
28)`; a click on the title is a click on the box. Look, from the toolkit, plus the mixed row:

| State | Box | Mark | Title |
|---|---|---|---|
| off | `field.bg`, 1 px `button.border` | none | `text` |
| off, hover | `field.bg`, 1 px `text.dim` | none | `text` |
| off, pressed | `button.pressed`, 1 px `button.border` | none | `text` |
| on | `button.primary`, no border | the tick: nine 2x2 `icon.dark` squares at (3,8) (4,9) (5,10) (6,9) (7,8) (8,7) (9,6) (10,5) (11,4) of the box | `text` |
| on, hover / pressed | `button.primary.hover` / `button.primary.pressed` | the tick | `text` |
| mixed | `button.primary`, no border | a 10x2 `icon.dark` bar at (3, 7) of the box | `text` |
| mixed, hover / pressed | as on | the bar | `text` |
| disabled | off: `field.bg`, 1 px `text.dim`; on or mixed: `button.bg`, no border, mark `text.dim` | | `text.dim` |

Worked example (window content 360 tall, section 18's `Show grid`): frame (16, 316, 160, 28)
gives `top = 360 - 316 - 28 = 16`: the control's rect (16, 16, 160, 28), the box (16, 22, 16,
16), the title's cell at (40, 22), 49 px wide (`Show grid`), so its ink ends at column 88.
After a click: box filled `button.primary`, the tick's first square at (19, 30), `state` 1,
`action` sent once.

### 9.3 Radio (`NSButtonTypeRadio`)

The mark is the toolkit's 16 px circle (`INSET_R8` outline, `field.bg` fill; selected:
`button.primary` fill with the 6 px `icon.dark` dot at (5, 5)), placed as the checkbox's box;
title, hit target, hover, pressed and disabled as the checkbox (there is no mixed radio: a
mixed value is stored as on). **Groups**: radios with the same superview and the same
non-null `action` selector form one group (AppKit's rule since 10.8; `NSMatrix` is later).
Turning one on, by a click or `setState:`, turns every other radio of its group off and
redraws each changed radio's rect; a click on the radio that is already on changes nothing
and still sends `action` (as AppKit). A radio with a null action is a group of one. `setState:
Off` on the on radio leaves the group with none on. Nothing is selected by default; the app
sets the first `state`.

Worked example (section 18): `Small` frame (16, 284, 160, 28) is the rect (16, 48, 160, 28),
circle (16, 54, 16, 16), title cell at (40, 54), 24 wide; `Large` frame (16, 252, 160, 28) is
(16, 80, 160, 28), circle (16, 86, 16, 16), title at (40, 86), 28 wide. Both have the action
`sizeChosen:` and the content view as superview: one group. A click at (24, 94) turns `Large`
on (its dot at (21, 91, 6, 6)) and `Small` off (outline and `field.bg` again).

### 9.4 Toggles (`PushOnPushOff`, `OnOff`, `Toggle`)

Section 2's push button whose **on** state borrows the card picker's selected look, the
toolkit's word for "chosen" (`toolkit-v0.3.md` section 5): fill `selection`, a 1 px `accent`
border inside the edge with `INSET_R4` corners, title `text`. Hover leaves an on button as it
is (as a selected card); pressed while on fills `button.pressed` and keeps the `accent`
border; off states are the push button's. Disabled on: `button.bg` fill, 1 px `text.dim`
border, `text.dim` title; disabled off: the push button's. A toggle that is also the default
button (`"\r"`) is not primary: the on look wins, and Enter still flips it. Worked example
(section 18's `Bold`, frame (16, 212, 96, 28)): rect (16, 120, 96, 28), title 20 wide centred
at (54, 126); after a click `state` 1, fill `selection`, border `accent` at the rect's edge
(rows 120 and 147, columns 16 and 111, corners cut by `INSET_R4`).

### 9.5 Tests

Host tests (`control.rs` beside the button's): the centring rule for frames of 18, 22, 28 and
32 (tops 16 - 5, 16 - 3, 16, 16 + 2 for a frame top of 16); the box and title rects of 9.2's
example and the hashed render of each row of its table (one window per state, pixels (19, 30)
`icon.dark` when on, (20, 29) `icon.dark` when mixed, (16, 22) `button.border` when off);
`nextState` through on, off, mixed, on with `allowsMixedState` and on, off without it;
`setState: -1` without `allowsMixedState` reading 1; the group rule: two radios with the
same action and superview, a third with another action, a click on the second turning the
first off and leaving the third alone, `setState:` doing the same, a click on the on radio
sending the action and changing nothing; the toggle's on render (pixel (16, 133) `accent`,
(60, 133) `selection`) and `Toggle`'s `alternateTitle` while on; `MomentaryChange` showing
`alternateTitle` only while pressed; Tab passing over all of them; Space and Enter doing
nothing to a checkbox; a disabled checkbox ignoring a click and keeping `state`. The QEMU
steps are section 18's.

## 10. NSScrollView and NSClipView

An `NSScrollView` (`initWithFrame:`) owns an `NSClipView` (`contentView`) that shows part of
one `documentView` (`setDocumentView:`, any `NSView`), scrolled by the clip view's bounds
origin and clipped to the clip view's rect. It is the toolkit's scroll view
(`toolkit-v0.3.md` section 8, `toolkit-v0.5.md` section 6) with AppKit's names; the document
view's own subviews are drawn (one more level than section 0 allowed), back to front in the
order added, each clipped to the clip view, and hit-tested through the clip view first. The
scroll view itself has no title and is never the first responder.

### 10.1 Geometry

- **Border** (`borderType`, default `NSNoBorder`): `NSNoBorder` draws nothing and the clip
  view is the frame; `NSLineBorder`, `NSBezelBorder` and `NSGrooveBorder` all draw **one 1 px
  `button.border` rectangle inside the frame's edge, square corners** (the clip view is a
  rectangle, so a rounded corner would show the document through the cut), and the clip view
  is the frame inset by 1. Apple draws three different bevels; one line is EggOS's.
- **Scrollers**: `hasVerticalScroller` and `hasHorizontalScroller` default NO (AppKit's
  default; apps set them). A vertical scroller is **8 px wide** at the clip view's right edge,
  full clip height; a horizontal one 8 px tall at the bottom, full clip width; when both show
  the vertical stops 8 above the bottom, the horizontal 8 before the right edge, and the 8x8
  corner is the scroll view's background. `+[NSScroller scrollerWidth]` and
  `scrollerWidthForControlSize:scrollerStyle:` answer 8; `scrollerStyle` is
  `NSScrollerStyleLegacy` (a shown scroller is always drawn, never an overlay that fades).
  `autohidesScrollers` (default YES): a scroller shows only while the document is larger than
  the clip view on that axis; with NO, a scroller that has nothing to scroll draws its track
  (`progress.track`) and no thumb. `contentSize` is the clip view's size less the scrollers
  shown; `contentSize` changes when a scroller appears, and the document is re-clipped, not
  resized (`documentView.frame` is the app's).
- **The scroller**: track `progress.track` the full strip; thumb `bar.pressed`, `INSET_R4`
  corners, length `track_len * view_len / content_len` rounded down, **at least 24**, at most
  the track; position `(track_len - thumb_len) * offset / (content_len - view_len)` rounded
  down; hovered or dragged `button.inactive` (high contrast: section 20). No arrow buttons
  (macOS has none either).
- **Background**: `drawsBackground` YES fills the clip view `backgroundColor`
  (`controlBackgroundColor`, `field.bg`) before the document draws; NO fills nothing (the
  window's `window.bg` shows). `contentView.backgroundColor` is the same colour.
- **Document origin and the flipped question.** The clip view's `bounds.origin` is the scroll
  position in the document's coordinates; `documentVisibleRect` is `(origin, contentSize)`.
  AppKit shows a non-flipped document's **bottom** first; every real app then scrolls to the
  top itself in `awakeFromNib`, and the views people scroll (`NSTableView`, `NSTextView`,
  `NSCollectionView`) are flipped. **EggOS shows the document's top first for every
  document**: for a flipped document the origin starts at (0, 0); for a non-flipped one at
  `(0, max(0, document_h - content_h))`, which is its top. A person opening a window must see
  its beginning, and no app is written to depend on seeing the end. Horizontal: the left,
  always.
- **Pixels**: with origin `(ox, oy)` and a flipped document, document point `(dx, dy)` is
  clip-view pixel `(dx - ox, dy - oy)`; for a non-flipped document, as section 0 with the
  document's height, shifted the same way. The offset clamps to `0 .. document - content` on
  each axis (0 when the document fits).

### 10.2 Scrolling

| Input | Effect |
|---|---|
| wheel over the scroll view (notches `n`) | the vertical offset by `72 n` (`WHEEL_STEP`, the Mouse pane's 3 lines of 24), clamped; the **innermost** scroll view, table or text area under the pointer whose content overflows on that axis takes it, as the toolkit's rule; none: nothing |
| Shift+wheel, sideways wheel | the horizontal offset by `72 n` |
| thumb press and drag | the offset follows the pointer keeping the grab point; release ends it |
| track press above or left of the thumb / below or right | the offset by `-(view_len - 24)` / `+(view_len - 24)`, once per press |
| Page Up / Page Down, Home / End | the offset by `-(view_h - 24)` / `+(view_h - 24)`, to 0 / to the end, **only when the key reaches the scroll view unhandled**: the first responder is the document view or one of its subviews and did not consume the key (`NSView`'s default `keyDown:`), as AppKit's `pageUp:` responder chain; a table handles its own keys (section 11), a text field never passes these on |
| arrows | nothing (AppKit's plain views do not scroll on arrows) |
| `-[NSClipView scrollToPoint:]`, `-[NSView scrollPoint:]`, `scrollRectToVisible:` (the least scroll that shows the rect), `-[NSScrollView reflectScrolledClipView:]` | set the offset, clamped, and redraw the scroll view's frame; `tile` re-lays the scrollers after a size change |

Every scroll damages the scroll view's whole frame (one commit). The pointer over a scroller
is `Arrow`; over the document, the document's (section 13). `verticalScroller` and
`horizontalScroller` are `NSScroller` objects whose `floatValue` (0 to 1) and `knobProportion`
read the state; setting them scrolls; `scrollerKnobStyle`, `scrollerInsets`, rulers, magnify
and elasticity are stored and ignored.

### 10.3 Worked example

Section 18's table: frame (192, 56, 272, 288) in a 360-tall content, `NSBezelBorder`,
`hasVerticalScroller` YES, `autohidesScrollers` YES, a 20-row table (section 11) as the
document, 508 px tall (28 header + 480 rows). Rect (192, 16, 272, 288); the border at columns
192 and 463 and rows 16 and 303; clip view (193, 17, 270, 286); the document overflows (508 >
286), so the vertical scroller shows at columns 455..462; `contentSize` (262, 286). Because the
document is a table, its header stays put and the scroller runs beside the body only (11.1):
rows 45..302, 258 tall; thumb `258 x 258 / 480 = 138` rows, at 45 at offset 0. After 3 wheel
notches: offset 216 (of a maximum 222), the thumb's top at `45 + 120 x 216 / 222 = 45 + 116 =
161`, rows 161..298.

### 10.4 Tests

Host tests (`scroll_view.rs`): the border's four rects; `contentSize` with no, one and two
scrollers; the thumb's length and position for (view 258, content 480, offset 0 and 216) and
the 24 minimum (view 100, content 10 000); the first-shown origin for a flipped and a
non-flipped 1000-tall document in a 300-tall clip view (0 and 700); `scrollToPoint:` clamping;
the wheel by 72 and the clamp at the end; a track press paging by `view - 24`; a thumb drag by
40 px moving the offset by `40 x (content - view) / (track - thumb)`; Page Down reaching the
scroll view only when the document view is the first responder; a document view's subview
drawn clipped at the clip view's edge (a 40x40 `systemBlueColor` square half outside: 20
columns drawn); `autohidesScrollers NO` drawing a track without a thumb; the render of 10.3
hashed at offsets 0 and 216.

## 11. NSTableView

A cell-based `NSTableView` with a data source, inside an `NSScrollView`, drawn as the
toolkit's table (`toolkit-v0.5.md` section 4: `TABLE_HEADER_HEIGHT` 28, `TABLE_ROW_HEIGHT` 24,
cell padding 8, the first column's mark column). The table is the scroll view's document
view; its frame's width is the clip view's `contentSize` width (the table stretches to it, as
AppKit's `columnAutoresizingStyle` does), its height `28 + 24 x rows` with a header and `24 x
rows` without.

### 11.1 Parts and geometry (clip view at `(cx, cy)`, offset `oy`)

- **Header** (`headerView`, an `NSTableHeaderView` by default; `setHeaderView: nil` removes
  it): the clip view's top 28 rows, **not scrolled** with the body (AppKit keeps it put, and
  the scroll view's vertical scroller runs from the header's bottom to the clip view's
  bottom, the 8x28 beside the header being the header's own surface). Fill `menu.bg`, a 1 px
  `menu.border` line along its bottom (row `cy + 27`); column `c` spans `x_c .. x_c + w_c - 1`
  with `x_0 = cx` and `x_{c+1} = x_c + w_c`; a 1 px `menu.border` separator at `x_{c+1} - 1`,
  header rows 6..21 (`cy + 6 .. cy + 21`); the title (`NSTableColumn.title`, the
  `headerCell`'s `stringValue`) in `text.dim`, cell top `cy + 6`, at `x_c + 8`, end-ellipsised
  at `x_c + w_c - 8`; a right-aligned column (`headerCell.alignment` or the column's
  `dataCell.alignment` `NSTextAlignmentRight`) right-aligns its title and cells to `x_c + w_c -
  8`. No sorting in this slice: header cells have no hover or pressed look and a click on one
  does nothing (`sortDescriptorPrototype` is stored).
- **Rows**: 24 tall from the header's bottom (`cy + 28`, or `cy` without a header), row `r` at
  `y_r = body_top + 24 r - oy`, clipped to the body (the clip view below the header, less the
  scrollers). `rowHeight` answers 24 and `intercellSpacing` `(0, 0)`; `setRowHeight:`,
  `setIntercellSpacing:` and `tableView:heightOfRow:` are stored or called and **ignored**
  (one row height, the toolkit's; a per-row height is later). A row's fill spans the table's
  full width. Cell text: the data source's object's `description` (an `NSString` as it is, an
  `NSNumber`'s decimal, `nil` empty), cell top `y_r + 4`, at `x_c + 8`, end-ellipsised at `x_c
  + w_c - 8`; **the first column's** text at `x_0 + 24` (`TABLE_FIRST_TEXT_X`) and the selected
  row's `>` in `accent` at `x_0 + 8`, the non-colour cue every EggOS list has.
- **Columns** (`NSTableColumn`): `identifier`, `title`, `width` (default 100), `minWidth`
  (default 10, raised to the toolkit's 40 when drawn), `maxWidth` (default 1000), `resizingMask`
  (default user and auto resizing), `headerCell`, `isHidden` (a hidden column takes no space).
  `addTableColumn:`, `removeTableColumn:`, `tableColumns`, `tableColumnWithIdentifier:`,
  `columnWithIdentifier:`, `moveColumn:toColumn:`. The last column does **not** stretch to
  fill the table (AppKit's `columnAutoresizingStyle` is `NSTableViewLastColumnOnlyAutoresizingStyle`
  by default; this slice stores the style and keeps the columns' widths, listed in section
  21): the surface shows past the last column, and a selected row's fill still spans the
  table. When the columns' widths sum past the table's width, the scroll view's horizontal
  scroller scrolls header and body together (`toolkit-v0.5.md` section 6); an app sets
  `hasHorizontalScroller`.
- **Column resize**: the 7 px handle centred on each separator (`x_{c+1} - 4 .. x_{c+1} + 3`,
  header rows only, the last column's right edge included), pointer `ResizeEastWest` (server
  shape 3); a press starts a drag that sets `w_c = clamp(start + dx, max(minWidth, 40),
  maxWidth)` live when the column's `resizingMask` has `NSTableColumnUserResizingMask`, re-laying
  header and rows in the same frame; release sends `tableViewColumnDidResize:` to the delegate
  (an `NSNotification` named `NSTableViewColumnDidResizeNotification`); Esc during the drag
  restores the start width. While hovered or dragging the separator is 1 px `text.dim` the full
  header height.
- **Alternating rows** (`usesAlternatingRowBackgroundColors`, default NO): odd rows (1, 3,
  ...) are filled `menu.bg` (`alternatingContentBackgroundColors` is `@[controlBackgroundColor,
  menu.bg]`; the second is a new semantic colour, `alternatingContentBackgroundColor`); even
  rows the table's `backgroundColor` (`controlBackgroundColor`, `field.bg`). In the two
  high-contrast palettes both are the surface, so the alternation disappears there and the
  grid and the selection carry the row (section 20).
- **Grid** (`gridStyleMask`, default none): `NSTableViewSolidHorizontalGridLineMask` draws a
  1 px `gridColor` (`menu.border`) line on each row's last row (`y_r + 23`) across the table's
  width, and one below the last row; `NSTableViewSolidVerticalGridLineMask` draws one at each
  `x_{c+1} - 1` through the body; the dashed masks draw the same solid lines. Grid lines go
  over the row fills and under the text and the `>`.
- **Empty table** (`rows` 0): the header and the surface, nothing else (AppKit shows nothing;
  the app has no empty text to give).

### 11.2 States

| Part | State | Fill | Text / mark |
|---|---|---|---|
| row | normal | the surface (or the alternate) | `text` |
| row | hover | `menu.hover` | `text` |
| row | selected, table not first responder | `selection`, the table's full width | `text`, `>` `accent` at `x_0 + 8` |
| row | selected, table first responder | `selection` | `text`, `>`, plus the focus ring at inset 0 of the row's rect (2 px `focus.ring`, `INSET_R4`), the row's full width less the scrollers |
| whole table | disabled (`setEnabled: NO`) | unchanged | `text.dim` everywhere, no `>`, pointer and keys ignored, not a Tab stop |

EggOS's lists show hover and macOS's do not; the table is EggOS's (one code path, D22).
Hover on the selected row changes nothing. AppKit's blue-when-key, grey-when-not selection
becomes: the ring says the table is the first responder, the fill says which row is selected,
in every state.

### 11.3 Selection, actions, data

- **Single selection** (`allowsMultipleSelection` stored, ignored). `selectedRow` is -1 at
  first (`allowsEmptySelection` YES, AppKit's default; with NO, `reloadData` selects row 0 when
  nothing is selected and the table has rows). A press on a row selects it, makes the table
  the first responder (the table **is** focusable: lists take focus on macOS without Full
  Keyboard Access, and it joins section 3's Tab loop in the order added), redraws the two rows
  and then sends `action` to `target` (`clickedRow` and `clickedColumn` set); a second press on
  the same row within 400 ms (`DOUBLE_CLICK_MS`) sends `doubleAction` instead (and `action`
  again when there is no `doubleAction`); a press on the selected row re-sends `action` and
  changes nothing; a press on the surface below the last row deselects when
  `allowsEmptySelection` (AppKit does), sends nothing. `tableView:shouldSelectRow:` NO refuses a
  selection (no change, no action). Every change of `selectedRow`, by pointer, keys or
  `selectRowIndexes:byExtendingSelection:` (an `NSIndexSet`; its `firstIndex` is the row; the
  layer's `NSIndexSet` knows `indexSetWithIndex:`, `firstIndex`, `count`,
  `containsIndex:`) and `deselectAll:`, sends `tableViewSelectionDidChange:` to the delegate
  with an `NSNotification` named `NSTableViewSelectionDidChangeNotification`, object the
  table (programmatic changes too, as AppKit), and scrolls the row into view when it was not.
- **Keys** (first responder): Up/Down one row (Down with -1 selects row 0, Up with -1 the last
  row), Home/End the first and last, Page Up/Page Down by the visible rows minus one, the body
  scrolling to keep the selection fully visible; Enter sends `doubleAction` (else `action`);
  Space nothing; Left/Right scroll the horizontal offset by 48 when the content is wider; Tab
  leaves. Cmd+A and type-to-select: later.
- **Wheel**: 72 px per notch rounded to whole rows (3 rows), the header included in the hit
  area (a wheel over the header scrolls the body).
- **Data source** (`dataSource`, `NSTableViewDataSource`): `numberOfRowsInTableView:` is asked at
  `reloadData` and at the first draw and cached as `rows`; `tableView:objectValueForTableColumn:row:`
  is asked **for every drawn cell at every paint** (the toolkit's `cell` callback: the tree
  keeps no cell text; a table of a million rows costs its state). `reloadData` re-asks the
  count, keeps `selectedRow` when it is still in range (else -1, with the delegate's
  notification), and damages the scroll view's frame. `numberOfRows` reads the cache. The
  view-based `tableView:viewForTableColumn:row:` and `NSTableCellView`: later (a table whose
  delegate answers it and whose data source gives no objects draws empty cells).
- **Pointer**: `Arrow` over rows and header cells, `ResizeEastWest` over a handle, `Arrow`
  over the scrollers.

### 11.4 Worked example (section 18)

The scroll view of 10.3: clip view (193, 17, 270, 286), scroller at 455..462, table width
262; columns `Name` 160 and `Size` 100 (sum 260 ≤ 262: no horizontal scroller);
`usesAlternatingRowBackgroundColors` YES; 20 rows, `Item <r + 1>` and `<r + 1> KB`. Header
(193, 17, 270, 28): `Name` at (201, 23), 26 wide; separator column 352, rows 23..38; `Size` at
(361, 23), 20 wide; bottom line row 44. Column 1 ends at column 452; 453..454 is the surface,
the scroller 455..462. Row `r` at `y_r = 45 + 24 r` at offset 0: row 0 rows 45..68 (`field.bg`),
row 1 rows 69..92 (`menu.bg`), ... row 10 rows 285..308 clipped at 302 (18 rows of it). Cells:
`Item 1` at (217, 49), `1 KB` at (361, 49). A press at (320, 150): row `(150 - 45) / 24 = 4`,
`Item 5`, rows 141..164: fill `selection` over columns 193..454, `>` at (201, 145), ring at
inset 0 of (193, 141, 262, 24); the table first responder (the text field's caret gone);
`selectedRow` 4; `action` sent; the delegate notified. Three wheel notches: offset 216 (the
maximum is 222): rows 9..19 shown, `Item 10` at row 9's `y = 45 + 216 - 216 = 45`, row 19 at
285..308 clipped; the selected row 4 is off the top and stays selected; the thumb at 161..298.

### 11.5 Tests

Host tests (`table_view.rs`): the header's rects and titles of 11.4, the separator and
handle zones (press at column 350 is a handle, at 346 a header cell); the row rect of each
visible row at offsets 0 and 216 and which rows are shown; alternating fills by row parity
and the grid lines by mask; the first-shown selection -1, Down selecting row 0, Up from -1
the last row, End scrolling to the end (offset 222) with row 19 fully visible (rows 278..301);
the press at (320, 150) selecting row 4, sending `action` with `clickedRow` 4 and notifying the
delegate once, a second press within 400 ms sending `doubleAction`; `shouldSelectRow:` NO
refusing; `reloadData` with 3 rows while row 4 is selected giving -1 and one notification; a
resize drag of column 0 by +20 giving 180 and re-placing `Size`'s title at (381, 23), clamped
at `minWidth` 40 and `maxWidth`; the disabled look; the render hashed at offsets 0 and 216
with row 4 selected and the table focused (pixels (201, 145) `accent`, (300, 150)
`selection`, (193, 141) `focus.ring`, (300, 80) `menu.bg`); the data source asked for exactly
the drawn cells (22 at offset 0: 11 rows x 2) per paint. The QEMU steps are section 18's.

## 12. NSImage and NSImageView

### 12.1 NSImage

- **Sources.** `imageNamed:` looks in the bundle's `Contents/Resources` for the name as given,
  then with `.png`, `.icns`, `.jpg`, `.jpeg`, `.gif`, `.bmp`, `.ico`, `.tif`, `.tiff` appended,
  first hit wins, decoded once and kept by name (`setName:` registers an image under a name;
  the system images `NSImageName...` answer nil in this slice). `initWithContentsOfFile:`,
  `initByReferencingFile:`, `initWithContentsOfURL:` (file URLs) and `initWithData:` decode the
  bytes. The decoder is ours, `eggos-image` (`decode`, the format sniffed: PNG, JPEG, GIF's
  first frame, TIFF, BMP, `.ico`'s largest entry) plus the installer's `.icns` reader for the
  largest image at most 1024 a side, every image converted once to **premultiplied
  `0xAARRGGBB`** (the installer's `premultiplied`). Limits: a side at most 4096, at most 16 M
  pixels, else the image is nil (`isValid` NO); a damaged file is nil, never a crash. `size`
  is the pixel size in points (one image, no representations: `representations` has one
  `NSBitmapImageRep` whose `pixelsWide`/`pixelsHigh` are the size). The pixels are pixel-
  doubled at 2x like every bitmap.
- **Drawing from `drawRect:`.** `drawAtPoint:fromRect:operation:fraction:` blits the image
  with its bottom-left at the point (section 0's coordinates), unscaled, clipped to the view;
  `drawInRect:fromRect:operation:fraction:` scales it to the rect (12.3); a zero `fromRect` is
  the whole image, a non-zero one crops before scaling; `operation` is ignored except
  `NSCompositingOperationCopy`, which writes the pixels without blending; `fraction` `f`
  scales every pixel's alpha by `f` (`a' = a f`, the colours already premultiplied are scaled
  too). `drawInRect:` and `drawAtPoint:` are these with the whole image, source-over, 1.0.
  Blending over the buffer is `app-icons-v0.3.md` section 3's: `out = s + (d x (255 - a) +
  127) / 255` per channel. `lockFocus`, `TIFFRepresentation`, `CGImage`: later.

### 12.2 NSImageView

The view's `image` drawn in the view's frame; no hit target, never the first responder, no
action on a click (AppKit sends one only when `editable`, which is later); the pointer is
`Arrow` unless a cursor rect says otherwise (section 13). A nil image draws only the frame
style. `imageScaling` and `imageAlignment` place an image of `iw x ih` in the **image area**
`(ax, ay, aw, ah)`: the frame, inset by 2 on each side when `imageFrameStyle` is not none.

| `imageScaling` | Drawn size `(w, h)` |
|---|---|
| `NSImageScaleProportionallyDown` (default) | the image's own size when `iw <= aw` and `ih <= ah`; else the proportional fit below |
| `NSImageScaleProportionallyUpOrDown` | the proportional fit: `if iw x ah <= ih x aw { h = ah; w = max(1, iw x ah / ih) } else { w = aw; h = max(1, ih x aw / iw) }`, integer division |
| `NSImageScaleAxesIndependently` | `(aw, ah)` |
| `NSImageScaleNone` | `(iw, ih)`, clipped to the image area |

| `imageAlignment` | Position in the area |
|---|---|
| `NSImageAlignCenter` (default) | `(ax + (aw - w) / 2, ay + (ah - h) / 2)`, rounded down (negative when `None` overflows: the image is clipped evenly) |
| `Top`, `Bottom`, `Left`, `Right` | centred on the other axis, flush on the named side (content coordinates: `Top` is the area's first row) |
| `TopLeft`, `TopRight`, `BottomLeft`, `BottomRight` | flush on both |

| `imageFrameStyle` | Drawn |
|---|---|
| `NSImageFrameNone` (default) | nothing; the image area is the frame |
| `NSImageFramePhoto`, `NSImageFrameGroove` | a 1 px `menu.border` rectangle inside the frame's edge, square corners; the image area inset by 2 |
| `NSImageFrameGrayBezel`, `NSImageFrameButton` | the same line in `button.border`, the frame filled `button.bg` inside it; the image area inset by 2 |

`setImage:` redraws the view's frame; `animates`, `allowsCutCopyPaste`, `editable`,
`contentTintColor` are stored and ignored.

### 12.3 The scaling filter: nearest neighbour, scaled once

Drawing scales with **nearest neighbour** (`src_x = dx x iw / w`, `src_y = dy x ih / h`,
integer division; the STATIC picture's rule in `windows-controls-v0.3.md` section 28), and an
image view keeps the scaled pixels until its image or its drawn size changes, so a repaint
is a blit and a blend, nothing per pixel beyond the blend. Why: performance first (CLAUDE.md,
D24): nearest reads one source pixel per destination pixel, has no multiplies and is exact
on host tests, and an image drawn at its own size, the common case for icons and the
toolbar-sized pictures a programmatic app shows, is not filtered at all. Area average, which
the installer uses for `app.icon` (`app-icons-v0.3.md` section 4), stays for things scaled
**once and kept**: section 15.1's About icon. Bilinear for photographs: later, as an
`imageScaling` quality the app does not control.

### 12.4 What agents see

Role `graphic`, bounds the view's frame, name the view's `accessibilityLabel` when set, else
the image's `name` (the name given to `imageNamed:` or `setName:`), else the file's name
without its folder and extension (`initWithContentsOfFile:`), else empty; value `<iw>x<ih>`
(`48x48`) or `none` without an image. No actions (`NotSupported`). An image drawn by a custom
view's `drawRect:` is part of that view's `graphic`, not an element of its own.

### 12.5 Tests

Host tests (`image_view.rs`, with `eggos-image`'s test PNGs): `imageNamed:` finding `egg`,
`egg.png` and `AppIcon` (the `.icns`), nil for a missing name and for a file of 5000 x 10;
`size`; the drawn size and position for a 48x48 image in a 96x96 frame under each scaling (48
at (24, 24); `UpOrDown` 96 at (0, 0); `AxesIndependently` 96x96) and for a 200x100 image in
96x96 (`ProportionallyDown` 96x48 at (0, 24); `None` the middle 96x96 of it); a 2 px inset
with `NSImageFramePhoto`; nearest-neighbour pixels of a 2x2 checker scaled to 96 (each cell
48); alpha: a half-transparent red pixel over `window.bg` gives the formula's value; `fraction`
0.5; a cursor rect over the view (section 13); the element's role, name from each source and
value; the render of section 18's image view hashed.

## 13. NSCursor

### 13.1 Shapes: AppKit's cursors on the server's ten

The server has ten shapes (`PointerShape`, `docs/windows.md` `SET_POINTER`); this slice adds
none. Each `NSCursor` class cursor maps to the closest one; the right column names what a
later server shape would be.

| `NSCursor` | Server shape | Later shape |
|---|---|---|
| `arrowCursor` | 0 `Arrow` | |
| `IBeamCursor`, `IBeamCursorForVerticalLayout` | 1 `IBeam` | a vertical I-beam |
| `pointingHandCursor` | 7 `Hand` (D186's table) | |
| `resizeLeftRightCursor`, `resizeLeftCursor`, `resizeRightCursor`, `columnResizeCursor` | 3 `ResizeEastWest` | one-sided arrows |
| `resizeUpDownCursor`, `resizeUpCursor`, `resizeDownCursor`, `rowResizeCursor` | 2 `ResizeNorthSouth` | one-sided arrows |
| `crosshairCursor` | 0 `Arrow` | `Crosshair` |
| `openHandCursor`, `closedHandCursor` | 0 `Arrow` (D186's choice stands: a pointing hand says "click", which a pan does not) | `OpenHand`, `ClosedHand` |
| `operationNotAllowedCursor` | 0 `Arrow` | `NotAllowed` |
| `dragCopyCursor`, `dragLinkCursor` | 0 `Arrow` | the arrow with a `+` or a link badge |
| `disappearingItemCursor`, `contextualMenuCursor` | 0 `Arrow` | |
| `currentCursor` | what 13.2 resolved last | |
| `currentSystemCursor` | the same | |
| the spinning wait cursor | no public `NSCursor` makes it, and none is added: it is the window server's verdict on macOS and on EggOS alike, 8 `Busy` over a window whose app has left events unread for 2 s (D222) | |

An `NSCursor` made from an image (`initWithImage:hotSpot:`) is `Arrow` (an app's own cursor
bitmaps: later). `hotSpot` answers the server shape's hotspot; `image` an `NSImage` of the
shape's bitmap at the server's size (`desktop-v0.3-gui.md` section 2, `desktop-polish-v0.3.md`
sections 1 and 4.1).

### 13.2 Which shape the window shows

The layer resolves one shape per window **on every `PointerMove`, `Enter`, cursor-rect change
and `set`**, and sends `SET_POINTER` only when it differs from the last sent (one request per
change, none per move). In order, the first that applies:

1. **Hidden**: `+[NSCursor hide]` counts up, `unhide` counts down (never below 0); while the
   count is above 0 every window of the app shows 6 `Hidden`. `setHiddenUntilMouseMoves: YES`
   shows `Hidden` until the next `PointerMove` in any of the app's windows.
2. **The server's own `Busy`** (D222) and its `Working` (D222) are the server's and outrank
   everything the app asks; nothing to do here.
3. **A cursor rect**: the topmost view under the pointer (a clip view's document and its
   subviews count, clipped), its rects in the order added, the first containing the point.
   AppKit's own views have implicit rects: an editable text field's whole drawn rect is
   `IBeam`; a table's handles are `ResizeEastWest` (11.1); everything else of theirs is
   `Arrow`. A custom view starts with none.
4. **The current cursor**: the last `set` (or `push`). `push` pushes the current onto a stack
   of at most 8 and sets; `pop` sets the stack's top and removes it (nothing on an empty
   stack); `set` replaces the current without touching the stack. Leaving a cursor rect
   sets `arrowCursor` as `set` would (AppKit's `cursorUpdate:` on exit), so a view that
   `set` an I-beam in `mouseEntered:` and relies on a rect to end it behaves as on macOS.
5. `Arrow`.

The pointer over the server's title bar and borders is the server's; the layer's shape covers
the content only. `-[NSView addCursorRect:cursor:]` records a rect in the view's coordinates
(at most 8 per view; a ninth is ignored), clipped to the view's frame and its clip view;
`removeCursorRect:cursor:` and `discardCursorRects` remove them; `resetCursorRects` is sent to
a view when the window becomes key, when the view's frame or superview changes, and after
`-[NSWindow invalidateCursorRectsForView:]` (the layer discards the view's rects first, then
calls it, as AppKit does). `NSTrackingArea` with `NSTrackingCursorUpdate`, `mouseEntered:`,
`mouseExited:` and `cursorUpdate:` on custom views: later (section 21).

Worked example (section 18): `ImageHost`'s `resetCursorRects` adds `[self addCursorRect:
self.bounds cursor:[NSCursor pointingHandCursor]]`: the pointer at content (64, 212), inside
the view's rect (16, 164, 96, 96), shows `Hand`; moving to (64, 290), over the text field,
`IBeam`; to (64, 134), over `Bold`, `Arrow`. One `SET_POINTER` per change: 7, then 1, then 0.

### 13.3 Tests

Host tests (`cursor.rs`): every class cursor's shape per 13.1; `hide` twice and `unhide` once
still `Hidden`, `unhide` again `Arrow`, a third `unhide` staying at 0; `setHiddenUntilMouseMoves:`
cleared by a move; a rect of 8 added, the ninth ignored; the resolution order with a rect
over a text field (the rect wins over the field's I-beam), with `set` and no rect, and after
leaving a rect (`Arrow`); `push`/`pop` with a stack of 2; `invalidateCursorRectsForView:`
calling `resetCursorRects` after discarding; the three `SET_POINTER` records of 13.2's example
and no record for a move inside one zone.

## 14. NSPasteboard and the Edit menu

### 14.1 The general pasteboard is the shared clipboard

`+[NSPasteboard generalPasteboard]` (and `pasteboardWithName: NSPasteboardNameGeneral`) is the
window server's one text (D197: `SET_CLIPBOARD`, `GET_CLIPBOARD`, a UTF-8 text of at most 1 MiB
and a serial). No visible UI.

| Call | Effect |
|---|---|
| `changeCount` | the server's serial (`GET_CLIPBOARD` with a 0-length buffer: a cheap read) |
| `clearContents` | `SET_CLIPBOARD` of 0 bytes; answers the new serial |
| `declareTypes:owner:` | `clearContents`; the owner's `pasteboard:provideDataForType:` is never called (lazy data: later); answers the serial |
| `setString:forType:` | for `NSPasteboardTypeString` (`public.utf8-plain-text`) and the legacy `NSStringPboardType`: `SET_CLIPBOARD` of the string's UTF-8; YES; a string past 1 MiB or another type: NO, nothing written |
| `setData:forType:` | the same when the bytes are valid UTF-8; NO otherwise |
| `writeObjects:` | the first `NSString` in the array as `setString:`; other classes are skipped; YES when one was written |
| `stringForType:` | for the string types, `GET_CLIPBOARD` whole (length first, then the bytes), an `NSString`, nil when the text is empty; other types nil |
| `dataForType:` | the bytes as `NSData`, nil when empty |
| `types` | `@[NSPasteboardTypeString]` when the text is not empty, else `@[]` |
| `availableTypeFromArray:` | `NSPasteboardTypeString` when it is in the array and the text is not empty, else nil |
| `readObjectsForClasses:options:` | `@[text]` when `NSString` (or `NSAttributedString`, as a plain string) is among the classes and the text is not empty, else `@[]` |
| `canReadObjectForClasses:options:`, `canReadItemWithDataConformingToTypes:` | YES under the same condition |
| `pasteboardItems` | one `NSPasteboardItem` with the string type, or none |
| `pasteboardWithName:` (`Find`, `Drag`, `Font`, `Ruler`, a unique name) | a private pasteboard of the process: one text slot and its own count, never shared |

Every read goes to the server, so what another app copied a moment ago is what `stringForType:`
returns: no syncing by serial as the Text Editor does.

### 14.2 Text fields: selection, Cmd+C, Cmd+X, Cmd+V, Cmd+A

An editable text field becomes the toolkit's field with selection (`toolkit-v0.4.md` section
4): `selection`-filled cells, Shift+arrows, Shift+Home/End, Shift+click, drag, double-click
word, triple-click all, Tab into the field selecting all, printable keys replacing the
selection. The Cmd shortcuts, in `keyDown:` order (section 5: the menu's key equivalents are
matched first; a menu item whose action is one of 14.3's selectors reaches the same code, so
one keystroke does one thing):

| Keys | Effect |
|---|---|
| Cmd+C | `copy:`: the selection to the general pasteboard (`setString:`); nothing without a selection |
| Cmd+X | `cut:`: `copy:` and the selection removed (the field's `action` is not sent; `controlTextDidChange:` is) |
| Cmd+V | `paste:`: the pasteboard's text replaces the selection or goes in at the caret, whole characters up to the field's 255 bytes, line breaks and tabs made one space, other control characters dropped (the toolkit's `paste`); nothing when the clipboard is empty |
| Cmd+A | `selectAll:` |
| Cmd+Z | later |

`stringValue` reads the text as before; `currentEditor` answers nil (no field editor in this
slice; `NSText`'s `selectedRange` on the field: `selectedRange` and `setSelectedRange:`
directly on `NSTextField` work on the field's anchor and caret as a convenience, in UTF-16
units). The pointer stays `IBeam`.

### 14.3 The Edit menu

An `NSMenuItem` with a nil target and `copy:`, `cut:`, `paste:`, `selectAll:` or `delete:`
sends the action down the responder chain (section 5): an editable text field as first
responder performs 14.2's; a table performs none of them (`copy:` of a row is the app's own,
as in section 18's `Copy` button); the window and the delegate get it when they implement it;
otherwise nothing. Items are always drawn enabled (`validateMenuItem:` and
`autoenablesItems`: later). The usual menu, section 18's: `Edit` with `Cut Cmd+X`, `Copy
Cmd+C`, `Paste Cmd+V`, separator, `Select All Cmd+A`, 32-px items in the server's popup, key
labels at the right.

### 14.4 Tests

Host tests (`pasteboard.rs`, against the server fake): `setString:` then `clip get` giving the
text and the serial counting up; `clearContents` emptying; `stringForType:` nil when empty,
the text otherwise, after another client's `clip set`; `types` and `availableTypeFromArray:`;
`writeObjects:` with a number and a string writing the string; `setData:` with invalid UTF-8
refused; a 1 MiB + 1 string refused; a `Find` pasteboard not touching the server; the field:
Cmd+A, Cmd+C copying `Egg`, End, Cmd+V giving `EggEgg`, Cmd+X leaving the empty field and the
clipboard `EggEgg`, a paste of `a\nb\tc` giving `a b c`, a paste that would pass 255 bytes cut
on a character; the menu item `copy:` with a nil target reaching the focused field.

## 15. The standard About panel, the rest

Section 8 stands except where this section changes it.

### 15.1 `ApplicationIcon` and `setApplicationIconImage:`

The 48x48 at (120, 16) is chosen in this order: the options' `ApplicationIcon` (an `NSImage`;
another class is ignored), then `NSApp.applicationIconImage` (set by `setApplicationIconImage:`,
any time; a later set repaints an open panel's icon rect), then the installed `app.icon`'s 48
image (section 8), then the generic tile. An `NSImage` is scaled to fit 48x48 **once, when it
is set or the panel opens, with the installer's area-average scaler** (`app-icons-v0.3.md`
section 4: a larger image by area average, a smaller one by nearest at the largest whole
factor, centred, the rest transparent; this is drawn once and kept, so quality is free: 12.3),
proportionally, centred in the 48x48 (a 200x100 image becomes 48x24 at (120, 28)), then
blended over `menu.bg` with section 8's formula. `applicationIconImage` answers what was set,
else an `NSImage` of `app.icon`'s 48 pixels, else nil. **Nothing else uses it**: the dock, the
launcher, the switcher, the window list and the catalogue draw the installed `app.icon` (D148:
the shell's icon is the installer's, and a process cannot change the catalogue); alerts have
no icon (section 4). The tree's `graphic` value gains `image` for an `NSImage` icon beside
`own` and `generic`.

### 15.2 `Credits`

The options' `Credits` is an `NSAttributedString` or `NSString` (another class is ignored);
without the option the bundle's `Contents/Resources/Credits.txt` (UTF-8) is read; `Credits.rtf`,
`.rtfd` and `.html` are **not** shown (an RTF read as text would show control words; parsing
RTF is later, section 21). **Plain text only**: an attributed string's attributes are dropped
(`string`). The text is cut at 4096 bytes on a character boundary; an empty text draws no
region.

- **Region**: `(16, T, 256, 108)`, `T = 132 + 20 n` for `n` body lines (8 below the last body
  line's cell, which ends at `124 + 20 n`): 172 with version and copyright, 152 with one, 132
  with none. Surface `field.bg`, a 1 px `button.border` rectangle inside its edge, square
  corners; inner rect `(17, T + 1, 254, 106)`. Text in `text` at 1x, from column **24**, wrapped
  at words to **232 px** (columns 24..255; the text area's rule, `toolkit-v0.5.md` section 5.3:
  break at spaces, a word wider than 232 broken at a glyph, `\n` and `\r\n` start a line, an
  empty line counts), line `k` with its cell top at `T + 6 + 20 k - offset`, clipped to the
  inner rect. Content height `12 + 20 x lines`; when it exceeds 106 the scroller of 10.1
  shows at columns 263..270, rows `T + 1 .. T + 106` (track 106; thumb `106 x 106 /
  content_h`, at least 24), and the offset ranges over `0 .. content_h - 106`. The 232 is
  reserved whether or not the scroller shows, so text never reflows.
- **Scrolling**: the wheel by **3 lines, 60 px**, per notch (the toolkit's 72 is 3 rows of
  24; here a line is 20), clamped; thumb drag and track click (by `106 - 24 = 82`) as 10.2.
  No keys (section 8: nothing in the panel takes focus; Esc still closes it), no selection,
  no links; the pointer over the region is `Arrow`.
- **Panel height**: `T + 108 + 16 = 256 + 20 n` (296, 276, 256). Without credits, section
  8's `140 + 20 n`.

### 15.3 Raised, not re-created

`About` chosen, or `orderFrontStandardAboutPanel:` called, while the panel exists raises and
focuses the existing window (restoring it when minimised) and leaves it where the person put
it. The server has no request a client may send for this (`MANAGE_WINDOW` is root's; section
8 said so). **The engineer adds one**: **`RAISE_WINDOW`** (W, 4 bytes: 0 the window), honoured
for the sender's own windows only (`EPERM` for another client's, `ENOENT` for none), doing
what `ManageWindow Focus` does, **and only while the sender owns the focused window** or no
window is focused (so an app in the background cannot steal the focus; then `EBUSY` and nothing
moves, which is what `makeKeyAndOrderFront:` from a background app does on macOS too); it
joins `docs/windows.md`'s table. `makeKeyAndOrderFront:` and `orderFront:` on an app's own
existing window send it too (a second `makeKeyAndOrderFront:` on a shown window is a raise, not
a second window). When the call brings **different options** (a new `Credits`, icon, name or
version that changes the content), the panel is destroyed and created again at its current
position, as the first build did always.

### 15.4 In `NSApp.windows`

The panel is an `NSPanel` object the layer owns: `title` `About <name>`, `isVisible` YES while
shown, `styleMask` titled, closable and miniaturizable, `contentView` an `NSView` with no
subviews (the drawing is the layer's), `isKeyWindow` while focused, `delegate` nil unless the
app sets one. It is in `NSApp.windows` while shown and is `NSApp.keyWindow` when focused, so
`performClose:` and the app's Cmd+W handler find it; `close`, `orderOut:` and `performClose:`
on it close it (its delegate's `windowShouldClose:` is asked when the app set one; NO keeps
it); closing removes it from `windows`. It **still does not keep the app alive**: section 6's
"last window closed" counts the app's own windows only, and the panel goes with them. The
`NSApp.windows` order is creation order, the panel at its place.

### 15.5 `CFBundleGetInfoString`

Used as the copyright line when `NSHumanReadableCopyright` is absent and the options give no
`Copyright` (AppKit's own fallback), with the same cut at the first newline. Nothing else
reads it.

### 15.6 The element tree, amended

Section 8's table gains, after the copyright (ids in drawing order; a missing line still gives
its id to the next):

| Id | Parent | Role | Rect | Name | Value |
|---|---|---|---|---|---|
| 2 | 1 | `graphic` | `120,16,48,48` | the app's name | `own`, `generic` or `image` |
| next | 1 | `group` | `16,T,256,108` | `Credits` | `line <k + 1> of <lines>`, `k` the first line with a row inside the inner rect |
| following | the group | `label` | the line's cell `24,y,width,16` | the line's text | |

One `label` per line whose **whole** cell is inside the inner rect and whose text is not
empty, in order (a line cut by the border is counted in the value, not listed). Actions: `set-text "<n>"` on the group scrolls so line `n` (from 1) is the first
shown (clamped; `NotSupported` for a text that is not an integer); everything else
`element_cannot`, as before.

### 15.7 Worked example C, mac-views

Section 18's app: `CFBundleName` `mac-views`, `CFBundleShortVersionString` and
`CFBundleVersion` `0.3.0`, `NSHumanReadableCopyright` `© 2026 EggOS`, options `Credits` the
string of 18.1 and `ApplicationIcon` the bundled `egg.png` (48x48). Content 288x296, outside
290x326, title `About mac-views`; icon: the PNG at (120, 16, 48, 48), unscaled; `mac-views` at
2x, 104 wide, (92, 80, 104, 32); `Version 0.3.0`, 65 wide, (111, 128, 65, 16); `© 2026 EggOS`,
71 wide, (108, 148, 71, 16); credits region (16, 172, 256, 108), border rows 172 and 279,
columns 16 and 271. The text wraps to six lines: `Design by Claude Fable.` (116), `Every
control here is drawn by the EggOS` (210), `toolkit, not by AppKit.` (105), `No third-party
code, ever.` (131), an empty line, `Thanks to everyone who runs it.` (164): content height 132
> 106, so the scroller shows at (263, 173, 8, 106), thumb `106 x 106 / 132 = 85` tall at 173,
and the offset ranges over 0..26. Cells at offset 0: tops 178, 198, 218, 238 (258 is the empty
line; 278 is line 5, one row of it inside). After one wheel notch: offset 26 (60 clamped), line
1's cell at 172 (its first row under the border, rows 173..187 visible), line 5's at 252..267
fully visible, the thumb at `173 + 21 x 26 / 26 = 194`. Tree at offset 0: 1 `window` `About
mac-views` 0,0,288,296; 2 `graphic` `mac-views` = `image`; 3 `label` `mac-views` 92,80,104,32;
4 `label` `Version 0.3.0` 111,128,65,16; 5 `label` `© 2026 EggOS` 108,148,71,16; 6 `group`
`Credits` 16,172,256,108 = `line 1 of 6`; 7 `label` `Design by Claude Fable.` 24,178,116,16; 8
`label` 24,198,210,16; 9 `label` 24,218,105,16; 10 `label` 24,238,131,16. After the notch: 6 =
`line 2 of 6` (line 1 has rows inside, cut by the border, so it counts and is not listed),
and the labels are the lines whose whole cell is inside and has text: 7 `label` `Every
control here is drawn by the EggOS` 24,192,210,16; 8 `label` `toolkit, not by AppKit.`
24,212,105,16; 9 `label` `Thanks to everyone who runs it.` 24,252,164,16 (line 4 is empty).
Scale 2x doubles everything and pixel-doubles the icon.

### 15.8 Tests

Host tests (`about.rs`): the icon order (options, `applicationIconImage`, `app.icon`, tile)
and a 200x100 `NSImage` becoming 48x24 at (120, 28); `setApplicationIconImage:` repainting
an open panel's icon rect only; `Credits` from an `NSAttributedString` losing its attributes;
`Credits.txt` read when the option is absent, `Credits.rtf` ignored; the wrap of C's text into
its six lines with the widths above; `T` and the height for 0, 1 and 2 body lines with and
without credits; the scroller's presence at 5 and 6 lines (5 lines: content 112 > 106, shows;
4 lines: 92, does not); a notch scrolling 60 and clamping at 26; the render of C hashed at
offsets 0 and 26 (pixels (16, 172) `button.border`, (20, 180) `field.bg`, (24, 181) `text`,
(265, 180) `bar.pressed`, (265, 270) `progress.track`); the tree of C at both offsets;
`set-text "4"` on the group scrolling to line 4 (offset 26 clamped) and `set-text "x"`
`NotSupported`; `RAISE_WINDOW` sent, not `CreateWindow`, on a second `About` with the same
options, and destroy-plus-create with different options; `EBUSY` from the fake when another
client has the focus leaving the panel as it is; the panel in `NSApp.windows` with its title,
`isVisible`, gone after `close`, and `applicationShouldTerminateAfterLastWindowClosed` not
counting it; `CFBundleGetInfoString` used only without a copyright.

## 16. The alert's element tree

`NSAlert runModal`'s dialog (section 4) answers `wm show` like the About panel: the layer
answers for the dialog window, ids in drawing order, no NSView tree. Flags on the buttons:
`focusable`; `focused` on the one with the ring; `default` on the default button.

| Id | Parent | Role | Rect | Name | Flags |
|---|---|---|---|---|---|
| 1 | 0 | `window` | `0,0,400,h` | the title (the app's name) | |
| 2 | 1 | `label` | `16,16,width,16` | `messageText`, uncut | |
| 3 | 1 | `label` | `16,36,width,16` | `informativeText`, uncut; absent when empty (the buttons then start at 3) | |
| then | 1 | `button` | each button's drawn rect, **left to right** | its title | `focusable`, `focused`, `default` |

Actions: `invoke` on a button presses it exactly as a click does (`runModal` returns its
code; the dialog closes); `focus` on a button moves the ring to it; `select` and `set-text`
`NotSupported`; the labels and the window `NotSupported`. While the alert runs, the parent
window's tree is still answered and its actions refused (`Disabled`), as `elements.rs` does.
Consistent with D161: `wm show <dialog>` then `wm act <dialog> <id> invoke`.

Worked example (section 4's alert of `mac-paint`): 1 `window` `mac-paint` 0,0,400,112; 2
`label` `mac-paint` 16,16,48,16; 3 `label` `A programmatic AppKit app on EggOS.` 16,36,186,16;
4 `button` `Cancel` 184,68,96,28 `focusable`; 5 `button` `OK` 288,68,96,28 `focusable`,
`focused`, `default`. `wm act <dialog> 5 invoke` returns 1000 from `runModal`.

Tests (`alert.rs`): the tree of the example (ids, roles, rects, names, flags) and of a
one-line alert with one `OK` (ids 1, 2, 3); `invoke` on 4 returning 1001 and closing; `focus`
on 4 then the ring on `Cancel` and `invoke` on 5 still returning 1000; `set-text` on 5
`NotSupported`; the parent's `invoke` answered `Disabled` while the alert runs. The QEMU
step: during mac-paint's `Info` alert (section 7), `wm show` lists the five elements.

## 17. What agents see for the new views (D161, `wm show`)

`elements.rs`'s rule stands: the window is element 1, named by its title; views follow in
pre-order from the content view, numbered from 2, hidden views left out; bounds in content
pixels, a control's its drawn 28 px rect. New rows, with the roles `eggos_element_tree::Role`
already has (no new role):

| View | Role | Name | Value | Flags | Actions |
|---|---|---|---|---|---|
| checkbox | `checkbox` | the title | `mixed` when mixed, else empty | `checked` when on, `disabled` | `invoke` toggles as a click (9.1's order); `select`, `focus`, `set-text` `NotSupported` (not focusable) |
| radio | `radio` | the title | empty | `selected` when on, `disabled` | `invoke` or `select` turns it on as a click (the group follows); no synthetic group element: a radio's parent is its superview's element, as any view's |
| toggle button | `button` | the title (`alternateTitle` while shown) | `on` or `off` | `checked` when on, `default`, `disabled` | `invoke` flips it |
| scroll view | `group` | `accessibilityLabel` or empty | `<offset_x>,<offset_y>` in document pixels (`0,216`) | | `set-text "<x>,<y>"` scrolls there, clamped (`NotSupported` for other text); its scrollers have no elements; the document view and its subviews are its children, bounds clipped to the clip view; a child wholly outside gets its width and **height 0** at the clip view's top or bottom edge (the Windows layer's rule, `windows-controls-v0.3.md` section 29), so it is present, ordered and recognisably not visible |
| table | `table` | `accessibilityLabel` or empty | `row <selected + 1> of <rows>`, `no row of <rows>` with -1, `0 rows` when empty | `focusable`, `focused`, `disabled` | `set-text "<n>"` selects row `n` (from 1) and scrolls it into view (`action` sent, the delegate notified; `Done` with no change for `n` out of range); `focus` |
| table column | `columnheader` | the title | empty | | `set-text "<width>"` resizes as a drag (clamped; `NotSupported` when the column refuses resizing or the text is not an integer); `invoke` `NotSupported` (no sorting) |
| table row, **shown** | `row` | the first column's text | every cell joined by tabs (U+0009) | `selected` | `select` selects as a click (`action`); `invoke` as a double-click (`doubleAction`) |
| image view | `graphic` | 12.4 | `<w>x<h>` or `none` | | none |
| text field | `text-field` | as before | its text | as before | as before; the selection is not exposed |

Rows not shown are not listed (the tree is at most 64 KiB; the toolkit's cap of 91 shown rows
holds). The table's header cells come first, then the shown rows, under the table's element,
which sits under the scroll view's `group`. Worked example: section 18.4.

## 18. The test app `mac-views.app`

Built on CI's macOS runner from `tests/macos-views/main.m`, our own Objective-C source, as
`mac-paint` is (`xtask/src/macos.rs`'s `OBJC_PROGRAMS` gains it; `assemble_app` writes its
bundle with `Info.plist`, `Contents/Resources/egg.png`, a 48x48 PNG xtask draws, the `egg`
of the boot screen at 48 over transparency, and `AppIcon.icns` as mac-paint's), installed by
the boot test from `mac-views.zip`. `Info.plist`: `CFBundleName` `mac-views`,
`CFBundleShortVersionString` `0.3.0`, `CFBundleVersion` `0.3.0`, `NSHumanReadableCopyright`
`© 2026 EggOS`. mac-paint is unchanged (its About step keeps section 8's example B; mac-views
is the app that shows `Credits` and `ApplicationIcon`).

### 18.1 The window

One `FIXED_SIZE` window `mac-views`, content **480x360**. Views in the order added (AppKit
frames, bottom-left origin; the content rect each one draws, top-left origin):

| # | View | Frame | Content rect | Details |
|---|---|---|---|---|
| 1 | checkbox `Show grid` (`checkboxWithTitle:`), action `gridToggled:` | (16, 316, 160, 28) | (16, 16, 160, 28); box (16, 22, 16, 16); title at (40, 22), 49 wide | off; on toggles the table's `gridStyleMask` to both solid masks |
| 2 | radio `Small`, action `sizeChosen:` | (16, 284, 160, 28) | (16, 48, 160, 28); circle (16, 54, 16, 16); title at (40, 54), 24 wide | on at start |
| 3 | radio `Large`, action `sizeChosen:` | (16, 252, 160, 28) | (16, 80, 160, 28); circle (16, 86, 16, 16); title at (40, 86), 28 wide | off |
| 4 | toggle `Bold` (`NSButtonTypePushOnPushOff`), action `boldToggled:` | (16, 212, 96, 28) | (16, 120, 96, 28); title at (54, 126), 20 wide | off |
| 5 | `ImageHost`, an `NSImageView` subclass whose `resetCursorRects` adds its bounds with `pointingHandCursor`; image `[NSImage imageNamed:@"egg"]`, default scaling and alignment, no frame style | (16, 100, 96, 96) | (16, 164, 96, 96); the 48x48 drawn at (40, 188) | |
| 6 | text field, placeholder `Type here` | (16, 56, 160, 28) | (16, 276, 160, 28); placeholder at (24, 282), 51 wide | the first editable field: the first responder at start, caret at (24, 282) |
| 7 | scroll view, `NSBezelBorder`, `hasVerticalScroller` YES; document: the table below | (192, 56, 272, 288) | (192, 16, 272, 288); clip (193, 17, 270, 286); scroller columns 455..462, rows 45..302 | |
| 7a | table `Items`: columns `name` `Name` 160 and `size` `Size` 100; `usesAlternatingRowBackgroundColors` YES; data source 20 rows `Item <r + 1>` / `<r + 1> KB`; `action` `rowClicked:`, `doubleAction` `rowOpened:`; delegate's `tableViewSelectionDidChange:` sets the result label to `Selected: Item <n>` | the clip view | header (193, 17, 270, 28), rows from 45 as 11.4 | `selectedRow` -1 at start |
| 8 | button `Copy`, action `copySelected:` | (16, 16, 76, 28) | (16, 316, 76, 28); title at (42, 322), 24 wide | copies the selected row's `Name` with `clearContents` and `setString:forType:`; nothing with no selection |
| 9 | button `Paste`, action `pasteToLabel:` | (100, 16, 76, 28) | (100, 316, 76, 28); title at (124, 322), 28 wide | sets the result label to `Pasted: <stringForType:>`, `Pasted:` for nil |
| 10 | label (result), `Pasted:` at start | (192, 16, 272, 28) | (192, 316, 272, 28); text at (192, 322), 37 wide | left-aligned, end-ellipsised at 464 |

Main menu: `mac-views` (`About mac-views` → `showAbout:`, separator, `Quit mac-views` Cmd+Q),
`Edit` (`Cut` Cmd+X `cut:`, `Copy` Cmd+C `copy:`, `Paste` Cmd+V `paste:`, separator, `Select
All` Cmd+A `selectAll:`, all nil-targeted); titles at x 104..191 and 192..239. `showAbout:`
calls `orderFrontStandardAboutPanelWithOptions:` with `ApplicationIcon` `[NSImage
imageNamed:@"egg"]` and `Credits` the string
`Design by Claude Fable.\nEvery control here is drawn by the EggOS toolkit, not by
AppKit.\nNo third-party code, ever.\n\nThanks to everyone who runs it.` (15.7's example C).

### 18.2 Host tests

`mac_views.rs` renders the window at 480x360 and hashes: the start frame (checkbox off,
`Small` on, `Bold` off, the egg at (40, 188), the placeholder and caret, the header and rows
0..10 with alternating fills, no selection, `Pasted:`), with pixels (16, 22) `button.border`,
(21, 61) `icon.dark` (the dot), (300, 56) `field.bg`, (300, 80) `menu.bg`, (459, 100)
`bar.pressed`, (459, 250) `progress.track`; then each step of 18.3 as a host render after the
same input, so the QEMU test compares screendumps with these renders.

### 18.3 QEMU steps (aarch64 and x86_64, after the mac-paint steps of sections 7 and 8)

1. `mac-views` started from the launcher: the window opened and focused, the two menu titles
   in the bar; frame compared with the start render.
2. A click at content (24, 30): the checkbox on (`button.primary` box, the tick); the table
   gains horizontal and vertical grid lines (`menu.border` at rows 68, 92, ... and column 352).
3. A click at (24, 94): `Large` on, `Small` off.
4. A click at (64, 134): `Bold` on (`selection` fill, `accent` border).
5. A click at (320, 150): row 4 `Item 5` selected (fill 193..454, `>` at (201, 145), the ring),
   the table the first responder (the field's caret gone), the label `Selected: Item 5`
   (81 wide at (192, 322)).
6. The wheel 3 notches down at (320, 150): offset 216, rows 9..19 shown (`Item 10` at
   (217, 49)), the thumb at rows 161..298, the selection kept off-screen.
7. A click at (54, 330), `Copy`: `clip get` prints `Item 5`.
8. `clip set Hello world` from the test; a click at (138, 330), `Paste`: the label `Pasted:
   Hello world` (94 wide at (192, 322)).
9. A click at (96, 290) on the field (caret, the table's ring gone); `Egg` typed; Cmd+A (the
   three cells `selection`); Cmd+C: `clip get` prints `Egg`; End; Cmd+V: the field reads
   `EggEgg` (34 wide from 24).
10. The pointer moved to (64, 212), over the image: the screendump shows the `Hand` at its
    hotspot (D186: dumps include the pointer); moved to (64, 290): the `IBeam`.
11. `mac-views` clicked in the bar, `About mac-views` chosen: the panel of 15.7 at the work
    area's centre, focused, compared with the host's render at offset 0; `wm show` lists its
    ten elements.
12. The wheel one notch down over the credits: offset 26, compared with the render at 26;
    `wm show` gives `line 2 of 6` and three labels.
13. A click on the main window's title bar (focus back), then `About mac-views` again: the
    panel raised and focused, **the same window id in `wm list`**, at the same place, still at
    offset 26.
14. Esc: the panel gone, the main window focused. Cmd+Q: the window gone, the dock without
    the app, the process ended with status 0.

### 18.4 The tree at step 6

`wm show` on the window: 1 `window` `mac-views` 0,0,480,360; 2 `checkbox` `Show grid`
16,16,160,28 `checked`; 3 `radio` `Small` 16,48,160,28; 4 `radio` `Large` 16,80,160,28
`selected`; 5 `button` `Bold` 16,120,96,28 = `on`, `checked`; 6 `graphic` `egg` 16,164,96,96 =
`48x48`; 7 `text-field` 16,276,160,28 (name `Type here`, the placeholder; value empty); 8
`group` 192,16,272,288 = `0,216`; 9 `table` `Items` 193,17,270,286 = `row 5 of 20`,
`focusable`, `focused`; 10 `columnheader` `Name` 193,17,160,28; 11 `columnheader` `Size`
353,17,100,28; 12..22 `row`s for rows 9..19: 12 `Item 10` = `Item 10<tab>10 KB` 193,45,262,24,
... 22 `Item 20` 193,285,262,18 (clipped); none `selected` (row 4 is not shown); 23 `button`
`Copy` 16,316,76,28; 24 `button` `Paste` 100,316,76,28; 25 `label` `Selected: Item 5`
192,316,272,28. `wm act <w> 9 set-text 1` selects `Item 1`, scrolls to offset 0 and the label
reads `Selected: Item 1`.

## 19. Device classes (D23)

Built now: the desktop (pointer and keyboard, 1x and 2x). The rules below are what the same
code does elsewhere, and they bind the engineer in one way today: **no control height, row
height or hit size is a literal in the layer**; they come from the toolkit's one
`form_factor()` (`toolkit-v0.5.md` section 11), so the phone rows arrive by changing one
function.

- **Phone** (touch, a 360 px wide maximised window): controls 40 tall centred in the app's
  frame as 28 is today (a 28 px frame draws 6 px past each edge), the checkbox's and radio's
  mark 16 stays, its top at +12; table rows and header 40, text at +12; every hit target at
  least 40x40 (a checkbox's whole 40-tall control; a scroller thumb's hit zone 24 wide while
  its drawn width stays 8); a vertical pan over a scroll view or table scrolls it (the server
  sends the pan as `Wheel` notches; a horizontal pan as `SidewaysWheel`); a long press on a
  row is the double-click's `doubleAction`; the About panel is a maximised `Normal` window
  (`desktop-v0.3-gui.md` section 12) whose content keeps section 8's layout centred on the
  window's width (`x = (W - width) / 2`) and whose credits region is `W - 32` wide, wrapping
  at `W - 56`; cursor rects and `NSCursor` do nothing (no pointer). The app's frames are
  AppKit points, so a 480x360 window becomes 360 wide and the server clamps: an app that lays
  out for one size is cut, as on an iPad running a Mac app; a resizable window gets
  `windowDidResize:` and may re-lay out.
- **TV** (D-pad, scale 2, no pointer): Full Keyboard Access is on: push buttons, checkboxes,
  radios and toggles **take focus** and show the ring (section 2's "no focus" is a desktop
  rule), Up/Down/Left/Right move the focus to the nearest control in that direction (the
  toolkit's two-dimensional order), Select is Space (toggle, select, press), `Back` is Esc; a
  focused table takes Up/Down for rows and Left/Right for its horizontal offset, and leaves
  with Left at offset 0 or Right at its end; the About panel's credits scroll with Up/Down
  while the panel is focused (3 lines each), `Back` closes it; column handles do not exist;
  hover looks are never drawn.
- **Wearable** (a 200 px screen): a scroll view fills the window and scrolls by pan; a table
  is drawn as a list of its first column with no header (the toolkit's wearable rule: `Kind::
  Table` becomes a list); checkboxes and radios keep 16 px marks in 40 px rows; the About
  panel shows the icon, the name and the version only (no copyright, no credits; a long
  press on the name shows the rest later); image views scale `ProportionallyDown` into what
  the window gives; cursor rects do nothing.
- **2x** (built now): every number doubles, bitmaps pixel-double, the fonts as before.

## 20. Palettes: AppKit follows the user's appearance

**Decision**: an AppKit app follows `appearance.theme` and `accessibility.high_contrast`, as
the toolkit's own windows do (`toolkit-v0.5.md` section 8, D219), and unlike the Windows
layer's program content, which stays light (`windows-controls-v0.3.md` section 0) because
Win32's colours are constants programs hard-code against. AppKit's semantic colours are
dynamic by design: a macOS app already draws in Light and Dark Mode by reading them, so the
same app on EggOS reading them in the user's palette is what its author built for. Fixed
colours (`blackColor`, `colorWithRed:...`) stay their exact values in every palette, as on
macOS; an app that paints a white view in `whiteColor` gets white on a dark desktop, as it
would in Dark Mode.

- **How**: the layer asks `GET_PALETTE` at start and takes `PaletteChanged` (kind 16): it
  re-fills every window `window.bg` (or the app's `backgroundColor`), re-draws every view
  (`drawRect:` again), re-lays nothing, and commits each window once, in the same frame as
  the shell; `viewDidChangeEffectiveAppearance` is sent to every view and
  `NSApplicationDidChangeEffectiveAppearanceNotification`-style observers get
  `effectiveAppearance` changed. `NSApp.effectiveAppearance.name` is `NSAppearanceNameDarkAqua`
  under the dark palettes, `NSAppearanceNameAqua` under the light ones, with
  `NSAppearanceNameAccessibilityHighContrastDarkAqua` / `...HighContrastAqua` in high contrast;
  `bestMatchFromAppearancesWithNames:` picks by that; `NSApp.appearance = [NSAppearance
  appearanceNamed: NSAppearanceNameAqua]` (an app forcing an appearance) draws the app in
  that theme's palette with the user's high-contrast setting (index `theme + 2 x
  high_contrast`), the one honest reading of "the app insists on light".
- **The semantic colours** of section 1 resolve to the same tokens in every palette; the token
  takes the palette's value. Two colours are added by this slice:
  `alternatingContentBackgroundColor` `menu.bg` (11.1) and `scrubberTexturedBackgroundColor`
  `button.bg` (for completeness; an unknown semantic colour selector answers `labelColor`).
  The `system*Color`s keep **one value per theme**: dark, section 1's; light, the light
  terminal's ANSI normal row (`systemBlueColor` `#1F5FBF`, `systemRedColor` `#B3261E`,
  `systemGreenColor` `#2E7D32`, `systemYellowColor` `#8A5A00`, `systemPurpleColor` `#8E2EA0`,
  `systemTealColor` `#0B7285`, `systemOrangeColor` the light `accent` `#9A6200`,
  `systemGrayColor` the light `text.dim`); high contrast does not change them (they are the
  app's own choice of a colour, not a pair EggOS can promise a ratio for, and macOS's
  Increase Contrast leaves them too).
- **Every token this brief draws with**, in the four palettes (values from `toolkit-v0.5.md`
  8.2 and `first-run-accessibility-v0.3.md` 2.4; no new token):

| Token | Dark | Light | Dark, high contrast | Light, high contrast | Drawn here as |
|---|---|---|---|---|---|
| `window.bg` | `#12161D` | `#FFFFFF` | `#000000` | `#FFFFFF` | the content before views draw |
| `field.bg` | `#0B0E13` | `#FFFFFF` | `#000000` | `#FFFFFF` | checkbox and radio off fill, table surface, scroll view background, credits region |
| `menu.bg` | `#1C2430` | `#F4F6F9` | `#000000` | `#FFFFFF` | table header, alternate rows, the alert's and About panel's surface |
| `menu.border` | `#2A3340` | `#C9D0DA` | `#FFFFFF` | `#000000` | header bottom line, column separators, grid lines, photo frame |
| `menu.hover` | `#2A3440` | `#E3E8EF` | `#262626` | `#E6E6E6` | row hover |
| `text` | `#E6EAF0` | `#161B22` | `#FFFFFF` | `#000000` | titles, cells, credits |
| `text.dim` | `#8A94A6` | `#5B6675` | `#B8C0CC` | `#3D4650` | header titles, placeholders, disabled, hovered handle |
| `button.bg` | `#2A3440` | `#FFFFFF` | `#1F1F1F` | `#FFFFFF` | push and toggle off fill, bezel frame fill, disabled marks' fill |
| `button.border` | `#3A4454` | `#B8C1CC` | `#FFFFFF` | `#000000` | box and circle outline, scroll view border, credits border, button border |
| `button.hover` / `button.pressed` | `#344050` / `#202833` | `#F0F3F7` / `#DCE2EA` | `#333333` / `#4D4D4D` | `#E6E6E6` / `#D9D9D9` | push and toggle hover and press, box pressed |
| `button.primary` (+ hover, pressed) | `#F2B33D` (`#F6C25C`, `#D99E2E`) | the same | `#FFD24D` (`#FFE080`, `#E6B800`) | `#F2B33D` (`#F6C25C`, `#D99E2E`) | the checked box and selected radio |
| `icon.dark` | `#0B0E13` | `#0B0E13` | `#000000` | `#000000` | the tick, the bar, the dot |
| `selection` | `#2E4A6B` | `#CFE0F5` | `#153A6B` | `#CCE0FF` | selected row, toggle on, text selection |
| `accent` | `#F2B33D` | `#9A6200` | `#FFD24D` | `#7A4E00` | the `>` mark, the toggle's on border |
| `focus.ring` | `#F2B33D` | `#9A6200` | `#FFD24D` | `#7A4E00` | the ring on the selected row of a focused table |
| `progress.track` | `#26303F` | `#DCE2EA` | `#4D4D4D` | `#D9D9D9` | scroller track |
| `bar.pressed` | `#334052` | `#D3DAE3` | `#404040` | `#D9D9D9` | scroller thumb at rest, **standard palettes only** |
| `button.inactive` | `#4A5466` | `#B0B9C6` | `#8A94A6` | `#6B6B6B` | scroller thumb hovered or dragged; **at rest in high contrast** |

- **High contrast changes** (the standard palettes' 1 px edges become white on black or black
  on white by the token values alone; these are the three places the token swap is not
  enough): the **scroller thumb** rests in `button.inactive` and goes `text` when hovered or
  dragged (`bar.pressed` on `progress.track` is 1.1:1 in dark high contrast; `button.inactive`
  is 3.0:1 there and 3.5:1 in light); **alternating rows** vanish (11.1: both fills are the
  surface), the grid and the `>` carry the row; the **toggle's on state** keeps its `accent`
  border (`#FFD24D` on `#153A6B`, 7.9:1), which is the non-colour cue, and the checked box's
  tick, the radio's dot and the selected row's `>` are the others. No stroke widens, no layout
  moves (2.4's rule).
- **Tests** (`palette.rs`): every semantic colour's token; each token's four values against
  the toolkit's `Palette` constants (one assertion per row above, so a palette change is
  caught here too); `effectiveAppearance.name` per index; a forced `NSAppearanceNameAqua`
  under dark high contrast giving index 3; `PaletteChanged` repainting a window whose
  `drawRect:` counts its calls (one more call, one commit); the renders of 9.2's rows, 11.4
  and 15.7 hashed in all four palettes, with the thumb's `button.inactive` pixel at rest in
  the high-contrast ones.

## 21. Later

- `NSButton` images, `imagePosition`, bezel styles, `NSMatrix`; focus on buttons, checkboxes
  and radios with Full Keyboard Access (the TV rule, on the desktop as a setting).
- Per-row heights, `intercellSpacing`, sorting (`sortDescriptors`, header click), multiple
  selection, column reordering by drag, `NSTableViewLastColumnOnlyAutoresizingStyle`, type-to-select,
  Cmd+A in a table, view-based tables (`NSTableCellView`), `NSOutlineView`, drag and drop of
  rows.
- A document view scrolled by arrows, overlay scrollers, `scrollerInsets`, rulers, magnify,
  `NSTrackingArea` and `mouseEntered:`/`mouseExited:`/`cursorUpdate:`.
- Bilinear filtering for photographs, animated GIFs, multiple representations and DPI,
  `lockFocus`, `TIFFRepresentation`, `NSImageName` system images, an editable image view.
- New server pointer shapes: a crosshair, open and closed hands, not-allowed, drag copy and
  link badges, a vertical I-beam; an app's own cursor bitmaps.
- Lazy pasteboard data (`pasteboard:provideDataForType:`), types beyond text (images, files,
  RTF), `NSPasteboardNameDrag` across apps, Cmd+Z, `validateMenuItem:` and `autoenablesItems`,
  the field editor (`currentEditor`).
- Credits in RTF, RTFD and HTML with styles, links in the credits, a selectable credits text.
- Phone, TV and wearable builds of section 19.
