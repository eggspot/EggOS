//! The named system images (D752, docs/ux/macos-system-images-v0.1.md): each one our own
//! drawing of a few filled shapes on a 16 or 32 pixel grid, rasterised at any whole scale by
//! sampling every pixel's centre in the 1x grid (so 2x redraws the shapes, never doubles the
//! pixels), each shape in a colour role resolved for the palette. Templates are drawn in one
//! colour the caller chooses at draw time.

use alloc::vec::Vec;

/// A shape on the 1x grid; `(x, y)` is a pixel corner, y down.
#[derive(Debug, Clone, Copy)]
pub enum Shape {
    Rect(i32, i32, i32, i32),
    Disc(i32, i32, i32),
    Ring(i32, i32, i32, i32),
    EllipseRing((i32, i32), (i32, i32), (i32, i32)),
    Polygon(&'static [(i32, i32)]),
    RoundRect(i32, i32, i32, i32, &'static [i32]),
    Rows(&'static Shape, i32, i32),
}

/// The colour a shape is filled with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Template,
    Text,
    TextDim,
    IconDark,
    ButtonBackground,
    ButtonBorder,
    MenuBorder,
    FieldBackground,
    Blue,
    Green,
    Yellow,
    Red,
    FolderFront,
    White,
    Clear,
}

/// One shape of an image: drawn in `role`, at `scale` times its coordinates plus `offset`.
#[derive(Debug, Clone, Copy)]
pub struct Layer {
    pub shape: Shape,
    pub role: Role,
    pub scale: f64,
    pub offset: (f64, f64),
}

const fn layer(shape: Shape, role: Role) -> Layer {
    Layer {
        shape,
        role,
        scale: 1.0,
        offset: (0.0, 0.0),
    }
}

const fn placed(shape: Shape, role: Role, scale: f64, offset: (f64, f64)) -> Layer {
    Layer {
        shape,
        role,
        scale,
        offset,
    }
}

/// A system image: its name as `NSImageName...`'s string, its side, whether it is a template,
/// and its layers drawn in order (a `Clear` layer cuts what is under it).
#[derive(Debug, Clone, Copy)]
pub struct SystemImage {
    pub name: &'static str,
    pub side: u32,
    pub template: bool,
    pub layers: &'static [Layer],
}

const INSET_R4: [i32; 4] = [2, 1, 0, 0];
const INSET_R6: [i32; 6] = [4, 2, 1, 1, 0, 0];

const GEAR: [Shape; 9] = [
    Shape::Ring(8, 8, 6, 3),
    Shape::Rect(7, 0, 2, 3),
    Shape::Rect(7, 13, 2, 3),
    Shape::Rect(0, 7, 3, 2),
    Shape::Rect(13, 7, 3, 2),
    Shape::Rect(2, 2, 2, 2),
    Shape::Rect(12, 2, 2, 2),
    Shape::Rect(2, 12, 2, 2),
    Shape::Rect(12, 12, 2, 2),
];

const fn gear(role: Role, scale: f64, offset: (f64, f64)) -> [Layer; 9] {
    [
        placed(GEAR[0], role, scale, offset),
        placed(GEAR[1], role, scale, offset),
        placed(GEAR[2], role, scale, offset),
        placed(GEAR[3], role, scale, offset),
        placed(GEAR[4], role, scale, offset),
        placed(GEAR[5], role, scale, offset),
        placed(GEAR[6], role, scale, offset),
        placed(GEAR[7], role, scale, offset),
        placed(GEAR[8], role, scale, offset),
    ]
}

const T: Role = Role::Template;
const BACK: &[(i32, i32)] = &[(10, 2), (12, 4), (8, 8), (12, 12), (10, 14), (4, 8)];
const FORWARD: &[(i32, i32)] = &[(6, 2), (12, 8), (6, 14), (4, 12), (8, 8), (4, 4)];
const CROSS_A: &[(i32, i32)] = &[(3, 4), (4, 3), (13, 12), (12, 13)];
const CROSS_B: &[(i32, i32)] = &[(12, 3), (13, 4), (4, 13), (3, 12)];
const SMALL_CROSS_A: &[(i32, i32)] = &[(5, 6), (6, 5), (11, 10), (10, 11)];
const SMALL_CROSS_B: &[(i32, i32)] = &[(10, 5), (11, 6), (6, 11), (5, 10)];
const TICK: &[(i32, i32)] = &[(2, 8), (4, 6), (6, 8), (12, 2), (14, 4), (6, 12)];
const SHARE_HEAD: &[(i32, i32)] = &[(4, 5), (8, 1), (12, 5), (11, 6), (8, 3), (5, 6)];
const RIBBON: &[(i32, i32)] = &[(4, 1), (12, 1), (12, 15), (8, 11), (4, 15)];
const RIBBON_INSIDE: &[(i32, i32)] = &[(6, 3), (10, 3), (10, 11), (8, 9), (6, 11)];
const HANDLE: &[(i32, i32)] = &[(9, 10), (10, 9), (15, 14), (14, 15)];
const ARROWHEAD: &[(i32, i32)] = &[(9, 1), (16, 4), (9, 7)];
const FOLDER_BACK: &[(i32, i32)] = &[(2, 6), (12, 6), (15, 9), (30, 9), (30, 27), (2, 27)];
const TRIANGLE: &[(i32, i32)] = &[(16, 2), (30, 28), (2, 28)];
const CAN: &[(i32, i32)] = &[(7, 9), (25, 9), (23, 29), (9, 29)];
const CAN_INSIDE: &[(i32, i32)] = &[(9, 11), (23, 11), (21, 27), (11, 27)];
const TILTED_LID: &[(i32, i32)] = &[(5, 3), (25, 1), (25, 3), (5, 5)];
const LOCKED_SHACKLE: Shape = Shape::Ring(8, 5, 4, 2);
const UNLOCKED_SHACKLE: Shape = Shape::Ring(8, 3, 4, 2);
const USER_SHOULDERS: Shape = Shape::Disc(16, 30, 11);
const BACK_SHOULDERS: Shape = Shape::Disc(11, 28, 9);
const FRONT_SHOULDERS: Shape = Shape::Disc(21, 30, 11);
const FRONT_SHOULDERS_GROWN: Shape = Shape::Disc(21, 30, 13);

const ADD: [Layer; 2] = [
    layer(Shape::Rect(7, 3, 2, 10), T),
    layer(Shape::Rect(3, 7, 10, 2), T),
];
const REMOVE: [Layer; 1] = [layer(Shape::Rect(3, 7, 10, 2), T)];
const ACTION: [Layer; 9] = gear(T, 1.0, (0.0, 0.0));
const GO_BACK: [Layer; 1] = [layer(Shape::Polygon(BACK), T)];
const GO_FORWARD: [Layer; 1] = [layer(Shape::Polygon(FORWARD), T)];
const REFRESH: [Layer; 3] = [
    layer(Shape::Ring(8, 8, 7, 5), T),
    layer(Shape::Rect(8, 1, 8, 7), Role::Clear),
    layer(Shape::Polygon(ARROWHEAD), T),
];
const STOP: [Layer; 2] = [
    layer(Shape::Polygon(CROSS_A), T),
    layer(Shape::Polygon(CROSS_B), T),
];
const STOP_FREESTANDING: [Layer; 3] = [
    layer(Shape::Ring(8, 8, 8, 6), T),
    layer(Shape::Polygon(SMALL_CROSS_A), T),
    layer(Shape::Polygon(SMALL_CROSS_B), T),
];
const LOCKED: [Layer; 4] = [
    layer(Shape::Rect(3, 7, 10, 8), T),
    layer(Shape::Rows(&LOCKED_SHACKLE, 1, 4), T),
    layer(Shape::Rect(4, 5, 2, 2), T),
    layer(Shape::Rect(10, 5, 2, 2), T),
];
const UNLOCKED: [Layer; 3] = [
    layer(Shape::Rect(3, 7, 10, 8), T),
    layer(Shape::Rows(&UNLOCKED_SHACKLE, 0, 2), T),
    layer(Shape::Rect(4, 3, 2, 4), T),
];
const MENU_ON: [Layer; 1] = [layer(Shape::Polygon(TICK), T)];
const SHARE: [Layer; 5] = [
    layer(Shape::Rect(3, 7, 2, 7), T),
    layer(Shape::Rect(11, 7, 2, 7), T),
    layer(Shape::Rect(3, 13, 10, 2), T),
    layer(Shape::Rect(7, 2, 2, 8), T),
    layer(Shape::Polygon(SHARE_HEAD), T),
];
const BOOKMARKS: [Layer; 2] = [
    layer(Shape::Polygon(RIBBON), T),
    layer(Shape::Polygon(RIBBON_INSIDE), Role::Clear),
];
const LIST_VIEW: [Layer; 3] = [
    layer(Shape::Rect(2, 3, 12, 2), T),
    layer(Shape::Rect(2, 7, 12, 2), T),
    layer(Shape::Rect(2, 11, 12, 2), T),
];
const ICON_VIEW: [Layer; 4] = [
    layer(Shape::Rect(2, 2, 5, 5), T),
    layer(Shape::Rect(9, 2, 5, 5), T),
    layer(Shape::Rect(2, 9, 5, 5), T),
    layer(Shape::Rect(9, 9, 5, 5), T),
];
const REVEAL: [Layer; 2] = [
    layer(Shape::Ring(6, 6, 5, 3), T),
    layer(Shape::Polygon(HANDLE), T),
];
const INVALID: [Layer; 3] = [
    layer(Shape::Ring(8, 8, 8, 6), T),
    layer(Shape::Rect(7, 4, 2, 5), T),
    layer(Shape::Rect(7, 10, 2, 2), T),
];
const STATUS_AVAILABLE: [Layer; 1] = [layer(Shape::Disc(8, 8, 5), Role::Green)];
const STATUS_PARTIAL: [Layer; 1] = [layer(Shape::Disc(8, 8, 5), Role::Yellow)];
const STATUS_UNAVAILABLE: [Layer; 1] = [layer(Shape::Disc(8, 8, 5), Role::Red)];
const STATUS_NONE: [Layer; 1] = [layer(Shape::Ring(8, 8, 5, 3), T)];
const FOLDER: [Layer; 2] = [
    layer(Shape::Polygon(FOLDER_BACK), Role::Blue),
    layer(Shape::Rect(2, 12, 28, 15), Role::FolderFront),
];
const SMART_GEAR: [Layer; 9] = gear(Role::White, 0.75, (10.0, 14.0));
const FOLDER_SMART: [Layer; 11] = [
    FOLDER[0],
    FOLDER[1],
    SMART_GEAR[0],
    SMART_GEAR[1],
    SMART_GEAR[2],
    SMART_GEAR[3],
    SMART_GEAR[4],
    SMART_GEAR[5],
    SMART_GEAR[6],
    SMART_GEAR[7],
    SMART_GEAR[8],
];
const COMPUTER: [Layer; 4] = [
    layer(Shape::Rect(3, 4, 26, 18), Role::Text),
    layer(Shape::Rect(5, 6, 22, 14), Role::FieldBackground),
    layer(Shape::Rect(14, 22, 4, 4), Role::Text),
    layer(Shape::Rect(9, 26, 14, 2), Role::Text),
];
const NETWORK: [Layer; 4] = [
    layer(Shape::Ring(16, 16, 13, 11), Role::Text),
    layer(Shape::Rect(3, 15, 26, 2), Role::Text),
    layer(Shape::Rect(15, 3, 2, 26), Role::Text),
    layer(Shape::EllipseRing((16, 16), (6, 13), (4, 11)), Role::Text),
];
const CAUTION: [Layer; 3] = [
    layer(Shape::Polygon(TRIANGLE), Role::Yellow),
    layer(Shape::Rect(14, 11, 4, 10), Role::IconDark),
    layer(Shape::Rect(14, 23, 4, 3), Role::IconDark),
];
const INFO: [Layer; 3] = [
    layer(Shape::Disc(16, 16, 14), Role::Blue),
    layer(Shape::Rect(14, 8, 4, 4), Role::White),
    layer(Shape::Rect(14, 14, 4, 10), Role::White),
];
const TRASH_EMPTY: [Layer; 6] = [
    layer(Shape::Rect(6, 5, 20, 2), Role::Text),
    layer(Shape::Rect(13, 2, 6, 3), Role::Text),
    layer(Shape::Polygon(CAN), Role::Text),
    layer(Shape::Polygon(CAN_INSIDE), Role::ButtonBackground),
    layer(Shape::Rect(13, 13, 2, 12), Role::TextDim),
    layer(Shape::Rect(17, 13, 2, 12), Role::TextDim),
];
const TRASH_FULL: [Layer; 7] = [
    layer(Shape::Disc(16, 9, 5), Role::TextDim),
    layer(Shape::Polygon(TILTED_LID), Role::Text),
    layer(Shape::Rect(13, 0, 6, 2), Role::Text),
    layer(Shape::Polygon(CAN), Role::Text),
    layer(Shape::Polygon(CAN_INSIDE), Role::ButtonBackground),
    layer(Shape::Rect(13, 13, 2, 12), Role::TextDim),
    layer(Shape::Rect(17, 13, 2, 12), Role::TextDim),
];
const USER: [Layer; 2] = [
    layer(Shape::Disc(16, 10, 6), Role::Text),
    layer(Shape::Rows(&USER_SHOULDERS, 19, 29), Role::Text),
];
const USER_GROUP: [Layer; 6] = [
    layer(Shape::Disc(11, 9, 5), Role::TextDim),
    layer(Shape::Rows(&BACK_SHOULDERS, 19, 27), Role::TextDim),
    layer(Shape::Disc(21, 11, 8), Role::Clear),
    layer(Shape::Rows(&FRONT_SHOULDERS_GROWN, 17, 29), Role::Clear),
    layer(Shape::Disc(21, 11, 6), Role::Text),
    layer(Shape::Rows(&FRONT_SHOULDERS, 19, 29), Role::Text),
];
const APPLICATION: [Layer; 7] = [
    layer(Shape::RoundRect(2, 2, 28, 28, &INSET_R6), Role::ButtonBorder),
    layer(Shape::RoundRect(4, 4, 24, 24, &INSET_R4), Role::ButtonBackground),
    layer(Shape::Rect(8, 9, 16, 14), Role::MenuBorder),
    layer(Shape::Rect(8, 9, 16, 3), Role::TextDim),
    layer(Shape::Rect(9, 10, 1, 1), Role::ButtonBackground),
    layer(Shape::Rect(12, 10, 1, 1), Role::ButtonBackground),
    layer(Shape::Rect(15, 10, 1, 1), Role::ButtonBackground),
];
const MULTIPLE_DOCUMENTS: [Layer; 4] = [
    layer(Shape::Rect(10, 2, 16, 22), Role::Text),
    layer(Shape::Rect(12, 4, 12, 18), Role::FieldBackground),
    layer(Shape::Rect(6, 8, 16, 22), Role::Text),
    layer(Shape::Rect(8, 10, 12, 18), Role::FieldBackground),
];
const PREFERENCES: [Layer; 9] = gear(T, 2.0, (0.0, 0.0));
const SECOND_GEAR: [Layer; 5] = [
    placed(Shape::Ring(8, 24, 6, 3), T, 1.0, (0.0, 0.0)),
    placed(Shape::Rect(7, 16, 2, 3), T, 1.0, (0.0, 0.0)),
    placed(Shape::Rect(7, 29, 2, 3), T, 1.0, (0.0, 0.0)),
    placed(Shape::Rect(0, 23, 3, 2), T, 1.0, (0.0, 0.0)),
    placed(Shape::Rect(13, 23, 3, 2), T, 1.0, (0.0, 0.0)),
];
const ADVANCED: [Layer; 14] = [
    PREFERENCES[0],
    PREFERENCES[1],
    PREFERENCES[2],
    PREFERENCES[3],
    PREFERENCES[4],
    PREFERENCES[5],
    PREFERENCES[6],
    PREFERENCES[7],
    PREFERENCES[8],
    SECOND_GEAR[0],
    SECOND_GEAR[1],
    SECOND_GEAR[2],
    SECOND_GEAR[3],
    SECOND_GEAR[4],
];

const fn template(name: &'static str, layers: &'static [Layer]) -> SystemImage {
    SystemImage {
        name,
        side: 16,
        template: true,
        layers,
    }
}

const fn icon(name: &'static str, side: u32, layers: &'static [Layer]) -> SystemImage {
    SystemImage {
        name,
        side,
        template: false,
        layers,
    }
}

/// Every system image, by the string its `NSImageName...` constant holds.
pub const SYSTEM_IMAGES: [SystemImage; 38] = [
    template("NSAddTemplate", &ADD),
    template("NSRemoveTemplate", &REMOVE),
    template("NSActionTemplate", &ACTION),
    template("NSGoBackTemplate", &GO_BACK),
    template("NSGoForwardTemplate", &GO_FORWARD),
    template("NSGoLeftTemplate", &GO_BACK),
    template("NSGoRightTemplate", &GO_FORWARD),
    template("NSRefreshTemplate", &REFRESH),
    template("NSStopProgressTemplate", &STOP),
    template("NSStopProgressFreestandingTemplate", &STOP_FREESTANDING),
    template("NSLockLockedTemplate", &LOCKED),
    template("NSLockUnlockedTemplate", &UNLOCKED),
    template("NSMenuOnStateTemplate", &MENU_ON),
    template("NSMenuMixedStateTemplate", &REMOVE),
    template("NSShareTemplate", &SHARE),
    template("NSBookmarksTemplate", &BOOKMARKS),
    template("NSListViewTemplate", &LIST_VIEW),
    template("NSIconViewTemplate", &ICON_VIEW),
    template("NSRevealFreestandingTemplate", &REVEAL),
    template("NSInvalidDataFreestandingTemplate", &INVALID),
    icon("NSStatusAvailable", 16, &STATUS_AVAILABLE),
    icon("NSStatusPartiallyAvailable", 16, &STATUS_PARTIAL),
    icon("NSStatusUnavailable", 16, &STATUS_UNAVAILABLE),
    template("NSStatusNone", &STATUS_NONE),
    icon("NSFolder", 32, &FOLDER),
    icon("NSFolderSmart", 32, &FOLDER_SMART),
    icon("NSComputer", 32, &COMPUTER),
    icon("NSNetwork", 32, &NETWORK),
    icon("NSCaution", 32, &CAUTION),
    icon("NSInfo", 32, &INFO),
    icon("NSTrashEmpty", 32, &TRASH_EMPTY),
    icon("NSTrashFull", 32, &TRASH_FULL),
    icon("NSUser", 32, &USER),
    icon("NSUserGroup", 32, &USER_GROUP),
    icon("NSApplicationIcon", 32, &APPLICATION),
    icon("NSMultipleDocuments", 32, &MULTIPLE_DOCUMENTS),
    SystemImage {
        name: "NSPreferencesGeneral",
        side: 32,
        template: true,
        layers: &PREFERENCES,
    },
    SystemImage {
        name: "NSAdvanced",
        side: 32,
        template: true,
        layers: &ADVANCED,
    },
];

/// The system image whose `NSImageName...` string is `name`.
pub fn named(name: &str) -> Option<&'static SystemImage> {
    SYSTEM_IMAGES.iter().find(|image| image.name == name)
}

/// The `NSImageName...` constant's symbol suffix for `name` (`NSAddTemplate` is
/// `NSImageNameAddTemplate`).
pub fn constant_suffix(name: &str) -> &str {
    name.strip_prefix("NS").unwrap_or(name)
}

/// A role's colour in the dark palette, or for `light`, the light one (`0x00RRGGBB`).
pub fn colour_of(role: Role, light: bool) -> Option<u32> {
    let (light_value, dark_value) = match role {
        Role::Template | Role::Text => (0x0016_1b22, 0x00e6_eaf0),
        Role::TextDim => (0x005b_6675, 0x008a_94a6),
        Role::IconDark => (0x000b_0e13, 0x000b_0e13),
        Role::ButtonBackground => (0x00ff_ffff, 0x002a_3440),
        Role::ButtonBorder => (0x00b8_c1cc, 0x003a_4454),
        Role::MenuBorder => (0x00c9_d0da, 0x002a_3340),
        Role::FieldBackground => (0x00ff_ffff, 0x000b_0e13),
        Role::Blue => (0x001f_5fbf, 0x004f_8fe0),
        Role::Green => (0x002e_7d32, 0x005f_b865),
        Role::Yellow => (0x008a_5a00, 0x00e2_b341),
        Role::Red => (0x00b3_261e, 0x00e0_565b),
        Role::FolderFront => (0x005a_8fd8, 0x007f_b0f0),
        Role::White => (0x00ff_ffff, 0x00ff_ffff),
        Role::Clear => return None,
    };
    Some(match light {
        true => light_value,
        false => dark_value,
    })
}

fn inside_polygon(points: &[(i32, i32)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut previous = match points.last() {
        Some(&last) => last,
        None => return false,
    };
    for &point in points {
        let (x1, y1) = (f64::from(previous.0), f64::from(previous.1));
        let (x2, y2) = (f64::from(point.0), f64::from(point.1));
        if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
        previous = point;
    }
    inside
}

fn inside_disc(cx: i32, cy: i32, radius: i32, x: f64, y: f64) -> bool {
    let (dx, dy) = (x - f64::from(cx), y - f64::from(cy));
    dx * dx + dy * dy <= f64::from(radius * radius)
}

fn inside_ellipse(centre: (i32, i32), radii: (i32, i32), x: f64, y: f64) -> bool {
    let dx = (x - f64::from(centre.0)) / f64::from(radii.0.max(1));
    let dy = (y - f64::from(centre.1)) / f64::from(radii.1.max(1));
    dx * dx + dy * dy <= 1.0
}

impl Shape {
    /// Whether the 1x-grid point `(x, y)` is inside.
    pub fn holds(&self, x: f64, y: f64) -> bool {
        match *self {
            Shape::Rect(left, top, width, height) => {
                x >= f64::from(left)
                    && x < f64::from(left + width)
                    && y >= f64::from(top)
                    && y < f64::from(top + height)
            }
            Shape::Disc(cx, cy, radius) => inside_disc(cx, cy, radius, x, y),
            Shape::Ring(cx, cy, outer, inner) => {
                inside_disc(cx, cy, outer, x, y) && !inside_disc(cx, cy, inner, x, y)
            }
            Shape::EllipseRing(centre, outer, inner) => {
                inside_ellipse(centre, outer, x, y) && !inside_ellipse(centre, inner, x, y)
            }
            Shape::Polygon(points) => inside_polygon(points, x, y),
            Shape::RoundRect(left, top, width, height, insets) => {
                if !Shape::Rect(left, top, width, height).holds(x, y) {
                    return false;
                }
                let row = y.floor() as i32 - top;
                let from_edge = row.min(height - 1 - row);
                let inset = insets.get(from_edge as usize).copied().unwrap_or(0);
                let column = x.floor() as i32 - left;
                column >= inset && column < width - inset
            }
            Shape::Rows(shape, first, last) => {
                let row = y.floor() as i32;
                row >= first && row <= last && shape.holds(x, y)
            }
        }
    }
}

/// `image` rasterised at `scale` (1 or 2): `0xAARRGGBB` pixels, premultiplied (every pixel
/// opaque or clear), a template's in `template_colour`.
pub fn render(image: &SystemImage, scale: u32, light: bool, template_colour: u32) -> Vec<u32> {
    let side = image.side * scale.max(1);
    let step = 1.0 / f64::from(scale.max(1));
    let mut pixels = alloc::vec![0u32; (side * side) as usize];
    for row in 0..side {
        for column in 0..side {
            let x = (f64::from(column) + 0.5) * step;
            let y = (f64::from(row) + 0.5) * step;
            let pixel = &mut pixels[(row * side + column) as usize];
            for each in image.layers {
                let local_x = (x - each.offset.0) / each.scale;
                let local_y = (y - each.offset.1) / each.scale;
                if !each.shape.holds(local_x, local_y) {
                    continue;
                }
                *pixel = match (each.role, colour_of(each.role, light)) {
                    (_, None) => 0,
                    (Role::Template, Some(_)) => 0xff00_0000 | template_colour,
                    (_, Some(colour)) => 0xff00_0000 | colour,
                };
            }
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(pixels: &[u32], side: u32, x: u32, y: u32) -> u32 {
        pixels[(y * side + x) as usize]
    }

    fn drawn(name: &str) -> (Vec<u32>, u32) {
        let image = named(name).unwrap();
        (render(image, 1, false, 0x00ff_ffff), image.side)
    }

    #[test]
    fn every_image_has_its_size_and_kind() {
        for image in &SYSTEM_IMAGES {
            assert!(image.side == 16 || image.side == 32, "{}", image.name);
            assert!(!image.layers.is_empty());
        }
        assert!(named("NSAddTemplate").unwrap().template);
        assert!(!named("NSFolder").unwrap().template);
        assert!(named("NSStatusNone").unwrap().template);
        assert_eq!(constant_suffix("NSAddTemplate"), "AddTemplate");
    }

    #[test]
    fn the_designs_sample_pixels_hold() {
        let (add, side) = drawn("NSAddTemplate");
        assert_ne!(at(&add, side, 8, 4), 0);
        assert_ne!(at(&add, side, 4, 8), 0);
        assert_eq!(at(&add, side, 4, 4), 0);
        let (back, side) = drawn("NSGoBackTemplate");
        assert_ne!(at(&back, side, 5, 8), 0);
        assert_eq!(at(&back, side, 11, 8), 0);
        let (refresh, side) = drawn("NSRefreshTemplate");
        assert_ne!(at(&refresh, side, 14, 4), 0);
        assert_ne!(at(&refresh, side, 12, 12), 0);
        assert_eq!(at(&refresh, side, 8, 8), 0);
        let (status, side) = drawn("NSStatusAvailable");
        assert_eq!(at(&status, side, 8, 8), 0xff5f_b865);
        assert_eq!(at(&status, side, 8, 2), 0);
        let (folder, side) = drawn("NSFolder");
        assert_eq!(at(&folder, side, 4, 8), 0xff4f_8fe0);
        assert_eq!(at(&folder, side, 4, 20), 0xff7f_b0f0);
        let (caution, side) = drawn("NSCaution");
        assert_eq!(at(&caution, side, 16, 15), 0xff0b_0e13);
        let (info, side) = drawn("NSInfo");
        assert_eq!(at(&info, side, 16, 20), 0xffff_ffff);
        let (user, side) = drawn("NSUser");
        assert_ne!(at(&user, side, 16, 10), 0);
        assert_ne!(at(&user, side, 16, 25), 0);
        assert_eq!(at(&user, side, 16, 17), 0);
        let (application, side) = drawn("NSApplicationIcon");
        assert_eq!(at(&application, side, 2, 2), 0);
        assert_eq!(at(&application, side, 5, 5), 0xff2a_3440);
        assert_eq!(at(&application, side, 10, 14), 0xff2a_3340);
    }

    #[test]
    fn double_scale_redraws_the_shapes() {
        let image = named("NSAddTemplate").unwrap();
        let pixels = render(image, 2, false, 0x00ff_ffff);
        assert_eq!(pixels.len(), 32 * 32);
        assert_ne!(at(&pixels, 32, 8, 14), 0);
        assert_ne!(at(&pixels, 32, 8, 17), 0);
        assert_eq!(at(&pixels, 32, 8, 13), 0);
    }
}
