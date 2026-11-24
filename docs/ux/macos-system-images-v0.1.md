# EggOS v0.1 macOS layer: the named system images, drawn by us: design brief

Design by Claude Fable (D16) for the `NSImageName...` images `imageNamed:` answered nil for
in `macos-gui-v0.3.md` 12.1. Each is **our drawing**, a few filled shapes on a 16 px or 32 px
grid that the layer builds as CoreGraphics paths (`core_graphics.rs`: rects, ellipses, arcs,
polygons, fill and even-odd fill) and rasterises once per size and palette into the
premultiplied `0xAARRGGBB` bitmap every `NSImage` is (12.1), so `NSImageView`, `NSButton`
images and `drawAtPoint:` need nothing new. Not Apple's art: the shapes are the ones the
toolkit's own icons use (`settings-forms-v0.5.md` 1.1: 2 px strokes, no gradients, no
shading).

## 0. Rules for every image

- **Grid and coordinates.** `(x, y)` is a pixel corner, y down, `(0, 0)` the top-left of the
  image; `rect (x, y, w, h)` covers pixels `x .. x + w - 1`, `y .. y + h - 1`; `disc (cx, cy,
  r)` is the circle of radius `r` about the pixel-corner `(cx, cy)`, filled with the toolkit's
  inset tables where one exists (`INSET_C14`, `INSET_C12`, `INSET_R3`, section 0 of
  `windows-controls-v0.3.md`) and otherwise by the path's rasteriser with no anti-aliasing;
  `ring (cx, cy, r_out, r_in)` is a disc minus a disc; `polygon` lists corners clockwise.
  Strokes are drawn as filled shapes, **2 px** wide at 1x.
- **Sizes.** Templates (`...Template` names, `MenuOnState`, `Bookmarks`, `Share`,
  `Lock`/`Unlock`) are **16x16**; the status dots 16x16; the icons (`Folder`, `FolderSmart`,
  `Computer`, `Network`, `Caution`, `Info`, `TrashEmpty`, `TrashFull`, `User`,
  `UserGroup`, `Application`) **32x32**. `size` answers that. **At 2x the paths are drawn
  at twice the coordinates**, 4 px strokes, crisp: these are the one kind of `NSImage` that
  is not pixel-doubled.
- **Colour.** A template (`isTemplate` YES) is one colour, decided where it is drawn: the
  control's text colour (`text`, `text.dim` disabled, `icon.dark` on a primary button, the
  menu item's text colour), or `contentTintColor` when the image view or button sets one
  (12.2's "ignored" ends for templates). Non-templates have fixed colours per theme (light /
  dark), the one-value-per-theme rule of section 20; high contrast changes the token-named
  ones only (`text`, `text.dim`, `button.bg`, `button.border`, `menu.border`, `field.bg`).
- **Scaling.** 12.2's rules apply when a view is larger or smaller than the image; a view of
  exactly 16 or 32 draws the paths as given. `imageNamed:` returns the same object each time;
  `setName:` under a system name replaces it for the app, as AppKit.
- **Agents.** The element name of a view showing one is the system name without `NSImageName`
  and `Template` (`Add`, `GoBack`, `StatusAvailable`), unless `accessibilityLabel` is set.

Token hex, light / dark (section 20): `text` `#161B22` / `#E6EAF0`, `text.dim` `#5B6675` /
`#8A94A6`, `icon.dark` `#0B0E13`, `button.bg` `#FFFFFF` / `#2A3440`, `button.border`
`#B8C1CC` / `#3A4454`, `menu.border` `#C9D0DA` / `#2A3340`, `field.bg` `#FFFFFF` / `#0B0E13`.
System colours, light / dark: blue `#1F5FBF` / `#4F8FE0`, green `#2E7D32` / `#5FB865`, yellow
`#8A5A00` / `#E2B341`, red `#B3261E` / `#E0565B`; `folder.front` (new, this brief only)
`#5A8FD8` / `#7FB0F0`.

## 1. Templates, 16x16, one colour

| Name | Shapes |
|---|---|
| `NSImageNameAddTemplate` | rect (7, 3, 2, 10); rect (3, 7, 10, 2) |
| `NSImageNameRemoveTemplate` | rect (3, 7, 10, 2) |
| `NSImageNameActionTemplate` (gear) | ring (8, 8, 6, 3); teeth: rect (7, 0, 2, 3), rect (7, 13, 2, 3), rect (0, 7, 3, 2), rect (13, 7, 3, 2), and the four diagonal teeth as 2x2 squares at (2, 2), (12, 2), (2, 12), (12, 12) |
| `NSImageNameGoBackTemplate` | polygon (10, 2), (12, 4), (8, 8), (12, 12), (10, 14), (4, 8) |
| `NSImageNameGoForwardTemplate` | the mirror: polygon (6, 2), (12, 8), (6, 14), (4, 12), (8, 8), (4, 4) |
| `NSImageNameGoLeftTemplate`, `GoRightTemplate` | the same two |
| `NSImageNameRefreshTemplate` | ring (8, 8, 7, 5) minus the quadrant rect (8, 1, 8, 7) (the top-right open); arrowhead polygon (9, 1), (16, 4), (9, 7) pointing right at the ring's top end |
| `NSImageNameStopProgressTemplate` | polygon (3, 4), (4, 3), (13, 12), (12, 13); polygon (12, 3), (13, 4), (4, 13), (3, 12) (an `x` of 2 px diagonals) |
| `NSImageNameStopProgressFreestandingTemplate` | ring (8, 8, 8, 6) plus the `x` above scaled to (5, 5)..(11, 11): polygons (5, 6), (6, 5), (11, 10), (10, 11) and (10, 5), (11, 6), (6, 11), (5, 10) |
| `NSImageNameLockLockedTemplate` | body rect (3, 7, 10, 8); shackle ring (8, 5, 4, 2) clipped to rows 1..4, legs rect (4, 5, 2, 2) and rect (10, 5, 2, 2) |
| `NSImageNameLockUnlockedTemplate` | body rect (3, 7, 10, 8); shackle ring (8, 3, 4, 2) clipped to rows 0..2, left leg rect (4, 3, 2, 4) reaching the body, no right leg |
| `NSImageNameMenuOnStateTemplate` (tick) | polygon (2, 8), (4, 6), (6, 8), (12, 2), (14, 4), (6, 12) |
| `NSImageNameMenuMixedStateTemplate` | rect (3, 7, 10, 2) (as Remove) |
| `NSImageNameShareTemplate` | tray: rect (3, 7, 2, 7), rect (11, 7, 2, 7), rect (3, 13, 10, 2); arrow shaft rect (7, 2, 2, 8); head polygon (4, 5), (8, 1), (12, 5), (11, 6), (8, 3), (5, 6) |
| `NSImageNameBookmarksTemplate` | polygon (4, 1), (12, 1), (12, 15), (8, 11), (4, 15) minus polygon (6, 3), (10, 3), (10, 11), (8, 9), (6, 11) (a 2 px ribbon outline) |
| `NSImageNameListViewTemplate` | rects (2, 3, 12, 2), (2, 7, 12, 2), (2, 11, 12, 2) |
| `NSImageNameIconViewTemplate` | rects (2, 2, 5, 5), (9, 2, 5, 5), (2, 9, 5, 5), (9, 9, 5, 5) |
| `NSImageNameRevealFreestandingTemplate` | magnifier: ring (6, 6, 5, 3); handle polygon (9, 10), (10, 9), (15, 14), (14, 15) |
| `NSImageNameInvalidDataFreestandingTemplate` | ring (8, 8, 8, 6); rect (7, 4, 2, 5); rect (7, 10, 2, 2) |

The status template `NSImageNameStatusNone` is in section 2 (it is a ring, template).

## 2. The status dots, 16x16

| Name | Shapes | Colour (light / dark) |
|---|---|---|
| `NSImageNameStatusAvailable` | disc (8, 8, 5) | green `#2E7D32` / `#5FB865` |
| `NSImageNameStatusPartiallyAvailable` | disc (8, 8, 5) | yellow `#8A5A00` / `#E2B341` |
| `NSImageNameStatusUnavailable` | disc (8, 8, 5) | red `#B3261E` / `#E0565B` |
| `NSImageNameStatusNone` | ring (8, 8, 5, 3) | template (the control's text colour) |

The three colours alone would fail a colour-blind reader, so the dots are not the only
cue where the layer places them itself: a menu item showing one still has its text, and an
app's status row is its own. Contrast on `window.bg`: green 4.8:1 light, 7.9:1 dark; yellow
6.2:1 light, 9.3:1 dark; red 6.3:1 light, 4.6:1 dark.

## 3. The icons, 32x32

| Name | Shapes and colours (light / dark) |
|---|---|
| `NSImageNameFolder` | back and tab: polygon (2, 6), (12, 6), (15, 9), (30, 9), (30, 27), (2, 27) in blue `#1F5FBF` / `#4F8FE0`; front: rect (2, 12, 28, 15) in `folder.front` `#5A8FD8` / `#7FB0F0` |
| `NSImageNameFolderSmart` | `Folder`, plus the gear of section 1 scaled to 12 px (its coordinates x 0.75, rounded) at (10, 14) in `#FFFFFF` |
| `NSImageNameComputer` | monitor: rect (3, 4, 26, 18) in `text` with rect (5, 6, 22, 14) in `field.bg` inside (a 2 px frame); stand rect (14, 22, 4, 4) and base rect (9, 26, 14, 2) in `text` |
| `NSImageNameNetwork` | globe: ring (16, 16, 13, 11) in `text`; equator rect (3, 15, 26, 2); meridian rect (15, 3, 2, 26); the inner meridian an ellipse ring centred (16, 16), radii 6 by 13 outer and 4 by 11 inner; all `text` |
| `NSImageNameCaution` | triangle polygon (16, 2), (30, 28), (2, 28) in yellow `#8A5A00` / `#E2B341`; `!` in `icon.dark`: rect (14, 11, 4, 10), rect (14, 23, 4, 3) |
| `NSImageNameInfo` | disc (16, 16, 14) in blue `#1F5FBF` / `#4F8FE0`; `i` in `#FFFFFF`: rect (14, 8, 4, 4), rect (14, 14, 4, 10) |
| `NSImageNameTrashEmpty` | lid rect (6, 5, 20, 2) and handle rect (13, 2, 6, 3) in `text`; can: polygon (7, 9), (25, 9), (23, 29), (9, 29) in `text` with the inner polygon (9, 11), (23, 11), (21, 27), (11, 27) in `button.bg` (a 2 px outline); two ribs rect (13, 13, 2, 12), rect (17, 13, 2, 12) in `text.dim` |
| `NSImageNameTrashFull` | `TrashEmpty` with the lid raised and tilted: lid polygon (5, 3), (25, 1), (25, 3), (5, 5) and handle rect (13, 0, 6, 2); a crumpled sheet: disc (16, 9, 5) in `text.dim` behind the can's rim |
| `NSImageNameUser` | head disc (16, 10, 6); shoulders: disc (16, 30, 11) clipped to rows 19..29; both `text` |
| `NSImageNameUserGroup` | back figure in `text.dim`: head disc (11, 9, 5), shoulders disc (11, 28, 9) clipped to rows 19..27; front figure in `text`: head disc (21, 11, 6), shoulders disc (21, 30, 11) clipped to rows 19..29, each with a 2 px gap carved in the back figure around it (even-odd with the front shapes grown by 2) |
| `NSImageNameApplicationIcon` (the placeholder for an app without an icon, 15.1) | rounded square rect (2, 2, 28, 28) with `INSET_R6` corners: fill `button.bg`, 2 px border `button.border`; a window inside: rect (8, 9, 16, 14) in `menu.border`, its title bar rect (8, 9, 16, 3) in `text.dim` carrying three 1x1 dots in `button.bg` at (9, 10), (12, 10), (15, 10) |
| `NSImageNameMultipleDocuments` | two sheets: rect (10, 2, 16, 22) in `text` with rect (12, 4, 12, 18) in `field.bg`; rect (6, 8, 16, 22) in `text` with rect (8, 10, 12, 18) in `field.bg` over the first |
| `NSImageNamePreferencesGeneral` | the gear of section 1 scaled x 2 (coordinates doubled), template |
| `NSImageNameAdvanced` | the gear scaled x 2 plus a second ring (8, 24, 6, 3) and its four straight teeth, template |

The Application placeholder's two colours and the Trash's rely on tokens, so they follow the
palette; the folders, Caution and Info keep their two fixed values, as the system colours do
(section 20).

## 4. Where the layer uses them itself

- `NSAlert`'s warning and informational styles (section 4) draw `Caution` and `Info` at
  32x32 where the alert's icon goes once this lands; the critical style `Caution`.
- `NSMenuItem` `state` on draws `MenuOnStateTemplate` as the tick (the server draws the
  menus; this is the glyph the layer hands it, `toolkit-v0.6.md` 2.1's tick is the same shape
  at 8 px), mixed `MenuMixedStateTemplate`.
- The About panel's icon with no `AppIcon` is `ApplicationIcon` (15.1).
- `NSOpenPanel` rows (`open_panel.rs`) draw `Folder` and `MultipleDocuments` at 16 px beside
  names: the paths at half their coordinates (1 px strokes), since paths scale, not 12.2's
  nearest-neighbour shrink of the 32 px bitmap.

## 5. Tests

Host tests (`system_images.rs`): every name in sections 1 to 3 answers a non-nil image of
the stated `size` and `isTemplate`; pixel checks per image (one or two per shape: `Add`'s
(8, 4) and (4, 8) set and (4, 4) clear; `GoBack`'s (5, 8) set and (11, 8) clear; `Refresh`'s
(14, 4) set and (12, 4) clear; `LockLocked`'s (4, 6) set, `LockUnlocked`'s (10, 6) clear;
`StatusAvailable`'s (8, 8) green and (8, 2) clear; `Folder`'s (4, 8) blue and (4, 20)
`folder.front`; `Caution`'s (16, 15) `icon.dark`; `Info`'s (16, 20) white; `User`'s (16, 10)
and (16, 25) set, (16, 17) clear; `Application`'s (3, 3) clear at the corner and (10, 10)
`button.bg`); a template drawn in an `NSImageView` with `contentTintColor` red giving red
pixels, in a disabled `NSButton` `text.dim`; the 2x bitmaps 32 and 64 a side with the
stroke 4 px (the `Add` bar rows 14..17 set, 13 clear); the four palettes' renders of `Folder`,
`Caution`, `Trash` and `Application` hashed; the element names.

## Decided by design (the owner can overrule any of these)

1. Flat shapes with 2 px strokes, the toolkit's icon idiom, in place of Apple's art.
2. Templates 16 px and the icons 32 px; paths redrawn at 2x rather than pixel-doubled.
3. A template's colour is the control's text colour or `contentTintColor`.
4. Status dots use the theme's ANSI green, yellow and red; `StatusNone` is a template ring.
5. Folders are blue with a lighter front (`folder.front`, two fixed values); Caution yellow,
   Info blue, both with `icon.dark` or white marks.
6. The Application placeholder is a rounded square with a window drawn inside, in tokens.

## Later

`NSImageNameColorPanel`, `FontPanel`, `Everyone`, `HomeTemplate`, `PathTemplate`,
`SlideshowTemplate`, `QuickLookTemplate`, `EnterFullScreenTemplate`, `ExitFullScreenTemplate`,
`FlowViewTemplate`, `ColumnViewTemplate`, `SmartBadgeTemplate`, `Touch Bar` names, `SF
Symbols` (`imageWithSystemSymbolName:`), and app-set `template` images recoloured.

## Decisions for the engineer

1. `system_images.rs`: one function per name building a `Path` list in the 16 or 32 grid
   with (shape, colour role) pairs; rasterised on first `imageNamed:` per size and palette
   into the `NSImage` bitmap; `PaletteChanged` drops the cache.
2. Colour roles: `Template` (resolved at draw time), token names, system colours, and
   `folder.front` with its two values.
3. `isTemplate` YES for section 1 and `StatusNone`; `contentTintColor` honoured for
   templates on `NSImageView` and `NSButton`.
4. 2x: paths at doubled coordinates, never the 1x bitmap doubled.
5. Element names: the name without `NSImageName` and `Template`.
