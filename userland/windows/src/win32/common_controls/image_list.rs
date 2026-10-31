//! comctl32's image lists (D178, `docs/ux/windows-controls-v0.3.md` section 27): each list is a
//! handle in a small table of the layer's naming a heap block, made on the first image, that
//! holds every image's `cx` by `cy` pixels and its one-bit mask, cut from the program's bitmaps
//! and icons when they are added, so a bitmap deleted afterwards changes nothing. Images draw by
//! the design's rule (`eggos_windows_gui::controls::image`) into a device context
//! (`ImageList_Draw`, `ImageList_DrawEx`) or, for the common controls, through [`LayerImages`].
//! The table's lock is only ever taken alone or inside the GUI state's.

use super::super::controls::picture_of;
use super::super::gui::{Gui, with};
use super::super::{Bool, FALSE, SpinLock, TRUE, error, fail, heap, paint};
use crate::gui::slots::Slots;
use crate::library::Export;
use eggos_windows_gui::controls::common::Images;
use eggos_windows_gui::controls::image::{self, MaskSource, Picture};
use eggos_windows_gui::surface::{Rect, Surface};

const MAX_IMAGE_LISTS: usize = 32;
const HANDLE_BASE: usize = 0x0150_0000;
const MOST_IMAGES: usize = 4096;
const LARGEST_SIDE: i32 = 256;
const SMALLEST_GROWTH: usize = 4;
const LAST: i32 = -1;

#[derive(Debug, Clone, Copy)]
struct ImageList {
    size: (i32, i32),
    count: usize,
    capacity: usize,
    grow: usize,
    block: usize,
}

impl ImageList {
    fn pixels_per_image(&self) -> usize {
        self.size.0 as usize * self.size.1 as usize
    }

    fn mask_words(&self) -> usize {
        image::mask_bytes(self.size.0, self.size.1).div_ceil(4)
    }

    fn stride(&self) -> usize {
        (self.pixels_per_image() + self.mask_words()) * 4
    }

    fn image(&self, index: usize) -> Option<(&[u32], &[u8])> {
        if index >= self.count || self.block == 0 {
            return None;
        }
        let start = self.block + index * self.stride();
        let pixels = self.pixels_per_image();
        let mask_bytes = image::mask_bytes(self.size.0, self.size.1);
        // SAFETY: the block holds `capacity` images of `stride` bytes, `index` is below `count`
        // and `count <= capacity`; the table's lock is held while the borrow of `self` lasts.
        let colours = unsafe { core::slice::from_raw_parts(start as *const u32, pixels) };
        // SAFETY: as above, for the mask after the image's pixels.
        let mask =
            unsafe { core::slice::from_raw_parts((start + pixels * 4) as *const u8, mask_bytes) };
        Some((colours, mask))
    }

    fn image_mut(&mut self, index: usize) -> Option<(&mut [u32], &mut [u8])> {
        if index >= self.capacity || self.block == 0 {
            return None;
        }
        let start = self.block + index * self.stride();
        let pixels = self.pixels_per_image();
        let mask_bytes = image::mask_bytes(self.size.0, self.size.1);
        // SAFETY: as `image`, for a slot below `capacity`; `&mut self` makes this the only
        // reference to the block.
        let colours = unsafe { core::slice::from_raw_parts_mut(start as *mut u32, pixels) };
        // SAFETY: as above, for the mask after the image's pixels, apart from them.
        let mask =
            unsafe { core::slice::from_raw_parts_mut((start + pixels * 4) as *mut u8, mask_bytes) };
        Some((colours, mask))
    }

    fn reserve(&mut self, count: usize) -> bool {
        if count <= self.capacity {
            return true;
        }
        if count > MOST_IMAGES {
            return false;
        }
        let capacity = count.max(self.capacity + self.grow).min(MOST_IMAGES);
        let Some(bytes) = capacity.checked_mul(self.stride()) else {
            return false;
        };
        let block = heap::allocate(bytes);
        if block.is_null() {
            return false;
        }
        if self.block != 0 {
            // SAFETY: both blocks are the layer's, the old one `capacity * stride` bytes long and
            // the new one longer; they do not overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.block as *const u8,
                    block,
                    self.capacity * self.stride(),
                )
            };
            // SAFETY: the old block, the layer's, used by nothing once copied.
            unsafe { heap::free(self.block as *mut u8) };
        }
        self.block = block as usize;
        self.capacity = capacity;
        true
    }

    fn store(&mut self, index: usize, source: &Picture<'_>, masks: MaskSource<'_>, place: i32) {
        let size = self.size;
        if let Some((pixels, mask)) = self.image_mut(index) {
            image::cut(source, masks, place * size.0, size, pixels, mask);
        }
    }

    fn clear_image(&mut self, index: usize) {
        if let Some((pixels, mask)) = self.image_mut(index) {
            pixels.fill(0);
            mask.fill(0xff);
        }
    }
}

type ListTable = Slots<ImageList, MAX_IMAGE_LISTS>;

static LISTS: SpinLock<ListTable> = SpinLock::new(Slots::new(HANDLE_BASE));

/// The images of the layer's image lists, for a common control's painting under the GUI
/// state's lock: a list is named by its handle, and a handle that names no list has no images.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct LayerImages;

impl Images for LayerImages {
    fn size(&self, list: usize) -> Option<(i32, i32)> {
        LISTS.with(|lists| lists.get(list).map(|list| list.size))
    }

    fn draw(
        &self,
        surface: &mut Surface<'_>,
        list: usize,
        index: i32,
        x: i32,
        y: i32,
        greyed: bool,
    ) {
        LISTS.with(|lists| {
            let Some(list) = lists.get(list) else {
                return;
            };
            let Some((pixels, mask)) = usize::try_from(index).ok().and_then(|at| list.image(at))
            else {
                return;
            };
            image::draw_cell(surface, pixels, mask, list.size, (x, y), greyed);
        });
    }
}

/// The size of list `list`'s images, if it is one.
pub(super) fn image_size(list: usize) -> Option<(i32, i32)> {
    LayerImages.size(list)
}

extern "efiapi" fn image_list_create(
    cx: i32,
    cy: i32,
    _flags: u32,
    initial: i32,
    grow: i32,
) -> usize {
    if !(1..=LARGEST_SIDE).contains(&cx) || !(1..=LARGEST_SIDE).contains(&cy) {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let mut list = ImageList {
        size: (cx, cy),
        count: 0,
        capacity: 0,
        grow: usize::try_from(grow).unwrap_or(0).max(SMALLEST_GROWTH),
        block: 0,
    };
    let initial = usize::try_from(initial).unwrap_or(0).min(MOST_IMAGES);
    if initial > 0 && !list.reserve(initial) {
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    }
    match LISTS.with(|lists| lists.insert(list)) {
        Some(handle) => handle,
        None => {
            free_block(list.block);
            fail(error::NOT_ENOUGH_MEMORY, 0)
        }
    }
}

fn free_block(block: usize) {
    if block != 0 {
        // SAFETY: the block is a list's, which left the table, so nothing reaches it any more.
        unsafe { heap::free(block as *mut u8) };
    }
}

extern "efiapi" fn image_list_destroy(list: usize) -> Bool {
    match LISTS.with(|lists| lists.remove(list)) {
        Some(removed) => {
            free_block(removed.block);
            TRUE
        }
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

extern "efiapi" fn image_list_get_image_count(list: usize) -> i32 {
    match LISTS.with(|lists| lists.get(list).map(|list| list.count)) {
        Some(count) => count as i32,
        None => fail(error::INVALID_HANDLE, 0),
    }
}

extern "efiapi" fn image_list_set_image_count(list: usize, count: u32) -> Bool {
    let done = LISTS.with(|lists| {
        let list = lists.get_mut(list).ok_or(error::INVALID_HANDLE)?;
        let count = count as usize;
        if !list.reserve(count) {
            return Err(error::NOT_ENOUGH_MEMORY);
        }
        for index in list.count..count {
            list.clear_image(index);
        }
        list.count = count;
        Ok(())
    });
    match done {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn image_list_get_icon_size(
    list: usize,
    cx: *mut i32,
    cy: *mut i32,
) -> Bool {
    let Some((width, height)) = image_size(list) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    if cx.is_null() || cy.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `ImageList_GetIconSize`'s contract: two writable `int`s, checked not null.
    unsafe { cx.write(width) };
    // SAFETY: as above.
    unsafe { cy.write(height) };
    TRUE
}

fn put(list: usize, at: Option<usize>, bitmap: usize, mask: Mask) -> Result<usize, u32> {
    with(|gui| put_locked(gui, list, at, bitmap, mask)).unwrap_or(Err(error::NOT_ENOUGH_MEMORY))
}

#[derive(Debug, Clone, Copy)]
enum Mask {
    Bitmap(usize),
    Colour(u32),
}

fn put_locked(
    gui: &Gui,
    list: usize,
    at: Option<usize>,
    bitmap: usize,
    mask: Mask,
) -> Result<usize, u32> {
    let source = picture_of(gui, bitmap).ok_or(error::INVALID_HANDLE)?;
    let masks = match mask {
        Mask::Bitmap(0) => MaskSource::None,
        Mask::Bitmap(handle) => {
            MaskSource::Bitmap(picture_of(gui, handle).ok_or(error::INVALID_HANDLE)?)
        }
        Mask::Colour(colour_ref) => MaskSource::Colour(colour_ref),
    };
    LISTS.with(|lists| {
        let list = lists.get_mut(list).ok_or(error::INVALID_HANDLE)?;
        if let Some(index) = at {
            if index >= list.count {
                return Err(error::INVALID_INDEX);
            }
            list.store(index, &source, masks, 0);
            return Ok(index);
        }
        let added = usize::try_from(source.width / list.size.0)
            .unwrap_or(0)
            .max(1);
        let first = list.count;
        if !list.reserve(first + added) {
            return Err(error::NOT_ENOUGH_MEMORY);
        }
        for place in 0..added {
            list.store(first + place, &source, masks, place as i32);
        }
        list.count = first + added;
        Ok(first)
    })
}

fn index_or_minus_one(result: Result<usize, u32>) -> i32 {
    match result {
        Ok(index) => index as i32,
        Err(code) => fail(code, LAST),
    }
}

extern "efiapi" fn image_list_add(list: usize, bitmap: usize, mask: usize) -> i32 {
    index_or_minus_one(put(list, None, bitmap, Mask::Bitmap(mask)))
}

extern "efiapi" fn image_list_add_masked(list: usize, bitmap: usize, colour_ref: u32) -> i32 {
    index_or_minus_one(put(list, None, bitmap, Mask::Colour(colour_ref)))
}

extern "efiapi" fn image_list_replace(list: usize, index: i32, bitmap: usize, mask: usize) -> Bool {
    let Ok(index) = usize::try_from(index) else {
        return fail(error::INVALID_INDEX, FALSE);
    };
    match put(list, Some(index), bitmap, Mask::Bitmap(mask)) {
        Ok(_) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn image_list_replace_icon(list: usize, index: i32, icon: usize) -> i32 {
    let Some((colour, mask)) = super::super::icon::icon_bitmaps(icon) else {
        return fail(error::INVALID_HANDLE, LAST);
    };
    let at = match index {
        LAST => None,
        index => match usize::try_from(index) {
            Ok(index) => Some(index),
            Err(_) => return fail(error::INVALID_INDEX, LAST),
        },
    };
    index_or_minus_one(put(list, at, colour, Mask::Bitmap(mask)))
}

extern "efiapi" fn image_list_remove(list: usize, index: i32) -> Bool {
    let done = LISTS.with(|lists| {
        let list = lists.get_mut(list).ok_or(error::INVALID_HANDLE)?;
        if index == LAST {
            list.count = 0;
            return Ok(());
        }
        let index = usize::try_from(index)
            .ok()
            .filter(|&index| index < list.count)
            .ok_or(error::INVALID_INDEX)?;
        let stride = list.stride();
        // SAFETY: images `index + 1..count` lie inside the block, which `&mut` makes this
        // function's alone; `copy` allows the overlap.
        unsafe {
            core::ptr::copy(
                (list.block + (index + 1) * stride) as *const u8,
                (list.block + index * stride) as *mut u8,
                (list.count - index - 1) * stride,
            );
        }
        list.count -= 1;
        Ok(())
    });
    match done {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

fn draw_into(hdc: usize, list: usize, index: i32, at: (i32, i32), size: (i32, i32)) -> Bool {
    let Some(whole) = image_size(list) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    let (width, height) = match size {
        (0, 0) => whole,
        (width, 0) => (width, whole.1),
        (0, height) => (whole.0, height),
        size => size,
    };
    let drawn = paint::draw(hdc, |surface, _, _| {
        if let Some(surface) = surface {
            let saved = surface.clip();
            surface.clip_to(Rect::sized(at.0, at.1, width, height));
            LayerImages.draw(surface, list, index, at.0, at.1, false);
            surface.set_clip(saved);
        }
    });
    match drawn {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn image_list_draw(
    list: usize,
    index: i32,
    hdc: usize,
    x: i32,
    y: i32,
    _style: u32,
) -> Bool {
    draw_into(hdc, list, index, (x, y), (0, 0))
}

#[allow(clippy::too_many_arguments)]
extern "efiapi" fn image_list_draw_ex(
    list: usize,
    index: i32,
    hdc: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    _background: u32,
    _foreground: u32,
    _style: u32,
) -> Bool {
    draw_into(hdc, list, index, (x, y), (width, height))
}

/// comctl32's image list functions.
pub(super) const EXPORTS: &[Export] = exports![
    b"ImageList_Create" => image_list_create,
    b"ImageList_Destroy" => image_list_destroy,
    b"ImageList_Add" => image_list_add,
    b"ImageList_AddMasked" => image_list_add_masked,
    b"ImageList_Replace" => image_list_replace,
    b"ImageList_ReplaceIcon" => image_list_replace_icon,
    b"ImageList_Remove" => image_list_remove,
    b"ImageList_GetImageCount" => image_list_get_image_count,
    b"ImageList_SetImageCount" => image_list_set_image_count,
    b"ImageList_GetIconSize" => image_list_get_icon_size,
    b"ImageList_Draw" => image_list_draw,
    b"ImageList_DrawEx" => image_list_draw_ex [two_stack_arguments],
];
