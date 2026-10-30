//! GDI's objects and device contexts: brushes, pens and fonts by handle (the stock objects at
//! fixed handles, system-colour brushes too), and each device context's selected objects,
//! colours, background mode, pen position and clip.

use super::slots::Slots;
use eggos_windows_gui::colour;
use eggos_windows_gui::shapes::Pen;
use eggos_windows_gui::surface::Rect;

/// The most brushes, pens and fonts a program holds at once.
pub const MAX_OBJECTS: usize = 256;
/// The most device contexts a program holds at once.
pub const MAX_CONTEXTS: usize = 32;
const OBJECT_BASE: usize = 0x0200_0000;
const CONTEXT_BASE: usize = 0x0100_0000;
const STOCK_BASE: usize = 0x0180_0000;
const SYSTEM_BRUSH_BASE: usize = 0x0190_0000;
const CURSOR_BASE: usize = 0x0170_0000;
const ICON_BASE: usize = 0x0160_0000;
const RESOURCE_IDS: usize = 0x1_0000;
/// The default bitmap's handle, what the first `SelectObject` of a bitmap into a memory device
/// context gives back.
pub const DEFAULT_BITMAP: usize = STOCK_BASE + 0x40;
/// A memory device context's clip before the program narrows it: no limit but its bitmap.
pub const UNLIMITED: Rect = Rect::new(-(1 << 30), -(1 << 30), 1 << 30, 1 << 30);

/// `LoadCursorW`'s system cursor ids this layer maps to the server's pointer shapes.
pub mod cursor {
    pub const ARROW: usize = 32512;
    pub const I_BEAM: usize = 32513;
    pub const SIZE_NORTH_WEST_SOUTH_EAST: usize = 32642;
    pub const SIZE_NORTH_EAST_SOUTH_WEST: usize = 32643;
    pub const SIZE_WEST_EAST: usize = 32644;
    pub const SIZE_NORTH_SOUTH: usize = 32645;
    pub const WAIT: usize = 32514;
    pub const HAND: usize = 32649;
    pub const APP_STARTING: usize = 32650;
}

/// The handle `LoadCursorW` gives for resource `id`: system cursors keep their id, and a
/// program's own cursors are the arrow (docs/ux/windows-gui-v0.3.md section 3).
pub fn cursor_handle(system: bool, id: usize) -> usize {
    match system && id < RESOURCE_IDS {
        true => CURSOR_BASE + id,
        false => CURSOR_BASE + cursor::ARROW,
    }
}

/// The handle `LoadIconW` gives for resource `id`; icons are not shown in part 1.
pub fn icon_handle(system: bool, id: usize) -> usize {
    match system && id < RESOURCE_IDS {
        true => ICON_BASE + id,
        false => ICON_BASE,
    }
}

/// Whether `handle` is one of [`icon_handle`]'s or [`cursor_handle`]'s: a shared system icon or
/// cursor, which has no image of its own (D178).
pub fn is_system_icon(handle: usize) -> bool {
    (ICON_BASE..ICON_BASE + RESOURCE_IDS).contains(&handle)
        || (CURSOR_BASE..CURSOR_BASE + RESOURCE_IDS).contains(&handle)
}

/// The server's pointer shape for cursor `handle`: the arrow for every cursor the server has
/// no shape for, and for none.
pub fn pointer_shape(handle: usize) -> u32 {
    match handle.wrapping_sub(CURSOR_BASE) {
        cursor::I_BEAM => 1,
        cursor::SIZE_NORTH_SOUTH => 2,
        cursor::SIZE_WEST_EAST => 3,
        cursor::SIZE_NORTH_WEST_SOUTH_EAST => 4,
        cursor::SIZE_NORTH_EAST_SOUTH_WEST => 5,
        cursor::HAND => 7,
        cursor::WAIT => 8,
        cursor::APP_STARTING => 0,
        _ => 0,
    }
}

/// `GetStockObject`'s indices.
pub mod stock {
    pub const WHITE_BRUSH: u32 = 0;
    pub const LIGHT_GRAY_BRUSH: u32 = 1;
    pub const GRAY_BRUSH: u32 = 2;
    pub const DARK_GRAY_BRUSH: u32 = 3;
    pub const BLACK_BRUSH: u32 = 4;
    pub const NULL_BRUSH: u32 = 5;
    pub const WHITE_PEN: u32 = 6;
    pub const BLACK_PEN: u32 = 7;
    pub const NULL_PEN: u32 = 8;
    pub const OEM_FIXED_FONT: u32 = 10;
    pub const SYSTEM_FONT: u32 = 13;
    pub const DEVICE_DEFAULT_FONT: u32 = 14;
    pub const SYSTEM_FIXED_FONT: u32 = 16;
    pub const DEFAULT_GUI_FONT: u32 = 17;
    pub const DC_BRUSH: u32 = 18;
    pub const DC_PEN: u32 = 19;
}

/// A bitmap's pixels (`0x00RRGGBB`, rows of `width`): a heap block of the layer's, or none for
/// the default bitmap a memory device context starts with (D174).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bitmap {
    pub address: usize,
    pub width: u32,
    pub height: u32,
}

impl Bitmap {
    /// The 1 by 1 bitmap a memory device context starts with, which draws nowhere.
    pub const EMPTY: Bitmap = Bitmap {
        address: 0,
        width: 1,
        height: 1,
    };

    /// Its rectangle, from (0, 0).
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }
}

/// A drawing object: a brush (`None` paints nothing), a pen (`None` draws nothing), a font
/// (every font is Egg Mono), a bitmap or a rectangular region (D174).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Object {
    Brush(Option<u32>),
    Pen(Option<Pen>),
    Font,
    Bitmap(Bitmap),
    Region(Rect),
}

fn stock_object(index: u32) -> Option<Object> {
    let solid = |pixel| Some(Object::Brush(Some(pixel)));
    let pen = |colour| Some(Object::Pen(Some(Pen { colour, width: 1 })));
    match index {
        stock::WHITE_BRUSH | stock::DC_BRUSH => solid(0x00ff_ffff),
        stock::LIGHT_GRAY_BRUSH => solid(0x00c0_c0c0),
        stock::GRAY_BRUSH => solid(0x0080_8080),
        stock::DARK_GRAY_BRUSH => solid(0x0040_4040),
        stock::BLACK_BRUSH => solid(0),
        stock::NULL_BRUSH => Some(Object::Brush(None)),
        stock::WHITE_PEN => pen(0x00ff_ffff),
        stock::BLACK_PEN | stock::DC_PEN => pen(0),
        stock::NULL_PEN => Some(Object::Pen(None)),
        stock::OEM_FIXED_FONT..=stock::DEVICE_DEFAULT_FONT
        | stock::SYSTEM_FIXED_FONT
        | stock::DEFAULT_GUI_FONT => Some(Object::Font),
        _ => None,
    }
}

/// The handle `GetStockObject(index)` returns, if `index` is a stock object.
pub fn stock_handle(index: u32) -> Option<usize> {
    stock_object(index).map(|_| STOCK_BASE + index as usize)
}

/// The handle `GetSysColorBrush(index)` returns.
pub fn system_brush_handle(index: u32) -> Option<usize> {
    colour::system_colour(index).map(|_| SYSTEM_BRUSH_BASE + index as usize)
}

/// The program's brushes, pens and fonts.
pub type ObjectTable = Slots<Object, MAX_OBJECTS>;

pub const fn new_objects() -> ObjectTable {
    Slots::new(OBJECT_BASE)
}

/// The object `handle` names: a stock object, a system-colour brush or one of `table`'s.
pub fn object(table: &ObjectTable, handle: usize) -> Option<Object> {
    if handle == DEFAULT_BITMAP {
        return Some(Object::Bitmap(Bitmap::EMPTY));
    }
    if let Some(index) = handle.checked_sub(STOCK_BASE).filter(|index| *index < 32) {
        return stock_object(index as u32);
    }
    if let Some(index) = handle
        .checked_sub(SYSTEM_BRUSH_BASE)
        .filter(|index| *index < 32)
    {
        return colour::system_colour(index as u32).map(|pixel| Object::Brush(Some(pixel)));
    }
    table.get(handle).copied()
}

/// Whether `handle` is a stock object or a system-colour brush, which deleting leaves alone.
pub fn is_permanent(handle: usize) -> bool {
    handle == DEFAULT_BITMAP
        || (STOCK_BASE..STOCK_BASE + 32).contains(&handle)
        || (SYSTEM_BRUSH_BASE..SYSTEM_BRUSH_BASE + 32).contains(&handle)
}

/// The pixel a brush `handle` paints, or `None` for a null brush or no brush: as a class's
/// background or `FillRect`'s brush, 1 to 31 also name system colour `handle - 1`.
pub fn brush_colour(table: &ObjectTable, handle: usize) -> Option<u32> {
    if (1..=31).contains(&handle) {
        return colour::system_colour(handle as u32 - 1);
    }
    match object(table, handle)? {
        Object::Brush(pixel) => pixel,
        _ => None,
    }
}

/// `SetBkMode`'s modes.
pub const TRANSPARENT: u32 = 1;
pub const OPAQUE: u32 = 2;

/// What `SelectObject` gives back for a region: the clip is one rectangle.
pub const SIMPLE_REGION: usize = 2;

/// A device context: the window it draws on (0 for one that draws nowhere) or, for a memory
/// context, the bitmap selected into it (D174), where it may draw (`clip`, inside
/// `base_clip`, which `SelectClipRgn(NULL)` returns to), what it drew, its selected objects
/// and text state, and the point pattern brushes are tiled from (D178).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceContext {
    pub window: usize,
    pub painting: bool,
    pub memory: bool,
    pub bitmap: usize,
    pub base_clip: Rect,
    pub clip: Rect,
    pub drawn: Rect,
    pub pen: usize,
    pub brush: usize,
    pub font: usize,
    pub text_colour: u32,
    pub background_colour: u32,
    pub background_mode: u32,
    pub position: (i32, i32),
    pub brush_origin: (i32, i32),
}

impl DeviceContext {
    /// A context with Windows' defaults (black pen, white brush, the system font, black text on
    /// an opaque white background) drawing on `window` inside `clip`.
    pub fn new(window: usize, painting: bool, clip: Rect) -> Self {
        let stock = |index| stock_handle(index).unwrap_or(0);
        DeviceContext {
            window,
            painting,
            memory: false,
            bitmap: 0,
            base_clip: clip,
            clip,
            drawn: Rect::default(),
            pen: stock(stock::BLACK_PEN),
            brush: stock(stock::WHITE_BRUSH),
            font: stock(stock::SYSTEM_FONT),
            text_colour: 0,
            background_colour: 0x00ff_ffff,
            background_mode: OPAQUE,
            position: (0, 0),
            brush_origin: (0, 0),
        }
    }

    /// `CreateCompatibleDC`'s context: Windows' defaults, drawing on its bitmap, at first the
    /// default one.
    pub fn memory() -> Self {
        DeviceContext {
            memory: true,
            bitmap: DEFAULT_BITMAP,
            ..DeviceContext::new(0, false, UNLIMITED)
        }
    }

    /// Selects `handle`, which is `object`, and returns the handle it replaces; a region
    /// becomes the clip (inside the base clip) and gives [`SIMPLE_REGION`]; a bitmap goes only
    /// into a memory context (0 otherwise).
    pub fn select(&mut self, handle: usize, object: Object) -> usize {
        let slot = match object {
            Object::Brush(_) => &mut self.brush,
            Object::Pen(_) => &mut self.pen,
            Object::Font => &mut self.font,
            Object::Bitmap(_) if self.memory => &mut self.bitmap,
            Object::Bitmap(_) => return 0,
            Object::Region(rect) => {
                self.clip = self.base_clip.intersection(&rect.normalised());
                return SIMPLE_REGION;
            }
        };
        core::mem::replace(slot, handle)
    }

    /// The bitmap a memory context draws on.
    pub fn selected_bitmap(&self, table: &ObjectTable) -> Option<Bitmap> {
        match object(table, self.bitmap)? {
            Object::Bitmap(bitmap) if self.memory => Some(bitmap),
            _ => None,
        }
    }

    /// The pen it draws with.
    pub fn pen(&self, table: &ObjectTable) -> Option<Pen> {
        match object(table, self.pen)? {
            Object::Pen(pen) => pen,
            _ => None,
        }
    }

    /// The brush it fills with.
    pub fn brush(&self, table: &ObjectTable) -> Option<u32> {
        brush_colour(table, self.brush)
    }

    /// The background text is drawn on: none when the mode is transparent.
    pub fn text_background(&self) -> Option<u32> {
        (self.background_mode == OPAQUE).then_some(colour::pixel(self.background_colour))
    }
}

/// The program's device contexts.
pub type ContextTable = Slots<DeviceContext, MAX_CONTEXTS>;

pub const fn new_contexts() -> ContextTable {
    Slots::new(CONTEXT_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_objects_and_system_brushes_have_fixed_handles() {
        let table = new_objects();
        let white = stock_handle(stock::WHITE_BRUSH).unwrap();
        assert_eq!(
            object(&table, white),
            Some(Object::Brush(Some(0x00ff_ffff)))
        );
        let null_pen = stock_handle(stock::NULL_PEN).unwrap();
        assert_eq!(object(&table, null_pen), Some(Object::Pen(None)));
        assert_eq!(
            object(&table, stock_handle(stock::DEFAULT_GUI_FONT).unwrap()),
            Some(Object::Font)
        );
        assert_eq!(stock_handle(9), None);
        assert_eq!(stock_handle(15), None);
        assert!(is_permanent(white));
        let window_brush = system_brush_handle(5).unwrap();
        assert_eq!(brush_colour(&table, window_brush), Some(0x00ff_ffff));
        assert_eq!(brush_colour(&table, 6), Some(0x00ff_ffff));
        assert_eq!(brush_colour(&table, 16), Some(0x00f0_f0f0));
        assert_eq!(brush_colour(&table, 0), None);
        assert_eq!(system_brush_handle(31), None);
    }

    #[test]
    fn cursors_map_to_the_servers_pointer_shapes() {
        assert_eq!(pointer_shape(cursor_handle(true, cursor::ARROW)), 0);
        assert_eq!(pointer_shape(cursor_handle(true, cursor::I_BEAM)), 1);
        assert_eq!(
            pointer_shape(cursor_handle(true, cursor::SIZE_NORTH_SOUTH)),
            2
        );
        assert_eq!(
            pointer_shape(cursor_handle(true, cursor::SIZE_WEST_EAST)),
            3
        );
        assert_eq!(pointer_shape(cursor_handle(true, 32642)), 4);
        assert_eq!(pointer_shape(cursor_handle(true, 32643)), 5);
        assert_eq!(pointer_shape(cursor_handle(true, 32649)), 7);
        assert_eq!(pointer_shape(cursor_handle(true, cursor::WAIT)), 8);
        assert_eq!(pointer_shape(cursor_handle(true, cursor::APP_STARTING)), 0);
        assert_eq!(pointer_shape(cursor_handle(false, cursor::I_BEAM)), 0);
        assert_eq!(pointer_shape(0), 0);
        assert_ne!(icon_handle(true, 32512), 0);
        assert_ne!(icon_handle(false, 0x4_0000), 0);
        assert!(is_system_icon(icon_handle(true, 32512)));
        assert!(is_system_icon(cursor_handle(true, cursor::I_BEAM)));
        assert!(!is_system_icon(0x0130_0000));
    }

    #[test]
    fn created_objects_are_found_selected_and_deleted() {
        let mut table = new_objects();
        let red = table.insert(Object::Brush(Some(0x00ff_0000))).unwrap();
        let pen = table
            .insert(Object::Pen(Some(Pen {
                colour: 0x0000_00ff,
                width: 3,
            })))
            .unwrap();
        let mut context = DeviceContext::new(0x1_0010, true, Rect::new(0, 0, 10, 10));
        assert_eq!(context.brush(&table), Some(0x00ff_ffff));
        assert_eq!(context.pen(&table).map(|pen| pen.colour), Some(0));
        let previous = context.select(red, Object::Brush(Some(0x00ff_0000)));
        assert_eq!(previous, stock_handle(stock::WHITE_BRUSH).unwrap());
        context.select(pen, Object::Pen(None));
        assert_eq!(context.brush(&table), Some(0x00ff_0000));
        assert_eq!(context.pen(&table).map(|pen| pen.width), Some(3));
        assert!(!is_permanent(red));
        table.remove(red);
        assert_eq!(context.brush(&table), None);
        assert_eq!(context.text_background(), Some(0x00ff_ffff));
        context.background_mode = TRANSPARENT;
        assert_eq!(context.text_background(), None);
        context.background_colour = 0x0000_00ff;
        context.background_mode = OPAQUE;
        assert_eq!(context.text_background(), Some(0x00ff_0000));
    }

    #[test]
    fn a_memory_context_draws_on_its_selected_bitmap_and_a_region_clips() {
        let mut table = new_objects();
        let bitmap = Bitmap {
            address: 0x5000,
            width: 33,
            height: 33,
        };
        let handle = table.insert(Object::Bitmap(bitmap)).unwrap();
        let mut memory = DeviceContext::memory();
        assert_eq!(memory.selected_bitmap(&table), Some(Bitmap::EMPTY));
        assert_eq!(
            memory.select(handle, Object::Bitmap(bitmap)),
            DEFAULT_BITMAP
        );
        assert_eq!(memory.selected_bitmap(&table), Some(bitmap));
        assert_eq!(bitmap.bounds(), Rect::new(0, 0, 33, 33));
        assert!(is_permanent(DEFAULT_BITMAP));
        let mut window = DeviceContext::new(0x1_0010, false, Rect::new(0, 0, 100, 100));
        assert_eq!(window.select(handle, Object::Bitmap(bitmap)), 0);
        assert_eq!(window.selected_bitmap(&table), None);
        let region = Object::Region(Rect::new(50, 50, 150, 150));
        assert_eq!(window.select(0, region), SIMPLE_REGION);
        assert_eq!(window.clip, Rect::new(50, 50, 100, 100));
        assert_eq!(window.base_clip, Rect::new(0, 0, 100, 100));
    }
}
