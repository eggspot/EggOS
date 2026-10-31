//! Icons, cursors and images of the program's own (D178): `CreateIconIndirect`,
//! `CreateIconFromResourceEx` (an icon or cursor image: a DIB with its AND mask, or a PNG),
//! `GetIconInfo`, `DestroyIcon`, `DrawIconEx` (alpha blended when the image has alpha, AND
//! and XOR with the mask otherwise, scaled to the nearest pixel), `CopyImage` and `LoadImageW`
//! (bitmaps, icons and cursors from the program's resources, or from `.bmp`, `.ico` and `.cur`
//! files). An icon is an entry of the layer's icon table holding a colour bitmap (alpha in its
//! top byte when the image has any) and a mask bitmap ([`dib::MASKED`] where it is
//! transparent), both in gdi's object table. The shared system icons and cursors of `LoadIconW`
//! and `LoadCursorW` have no image: drawing one or asking for its `ICONINFO` fails with
//! `ERROR_NOT_SUPPORTED`, and it copies and destroys as itself.

use super::dib::{self, Packed};
use super::gdi;
use super::paint;
use super::resource::{self, Name, kind};
use super::{Bool, FALSE, SpinLock, TRUE, error, error_for, fail, heap};
use crate::gui::objects::{self, Object};
use crate::gui::slots::Slots;
use crate::library::Export;
use crate::linux;
use crate::text;
use eggos_png::rows::RowWorkspace;
use eggos_windows_gui::surface::Rect;

const MAX_ICONS: usize = 64;
const HANDLE_BASE: usize = 0x0130_0000;
const DEFAULT_SIDE: u32 = 32;
const IMAGE_BITMAP: u32 = 0;
const IMAGE_ICON: u32 = 1;
const IMAGE_CURSOR: u32 = 2;
const LR_COPYRETURNORG: u32 = 0x4;
const LR_COPYDELETEORG: u32 = 0x8;
const LR_LOADFROMFILE: u32 = 0x10;
const LR_DEFAULTSIZE: u32 = 0x40;
const LR_CREATEDIBSECTION: u32 = 0x2000;
const DI_MASK: u32 = 0x1;
const DI_IMAGE: u32 = 0x2;
const DI_DEFAULTSIZE: u32 = 0x8;
const DIRECTORY_HEADER: usize = 6;
const GROUP_ENTRY: usize = 14;
const FILE_ENTRY: usize = 16;
const CURSOR_FILE: u16 = 2;
const HOTSPOT_BYTES: usize = 4;
const LARGEST_FILE: usize = 64 << 20;
const IDS: usize = 0x1_0000;
const WHITE: u32 = 0x00ff_ffff;

#[derive(Debug, Clone, Copy)]
struct Icon {
    colour: usize,
    mask: usize,
    width: u32,
    height: u32,
    hotspot: (u32, u32),
    icon: Bool,
}

static ICONS: SpinLock<Slots<Icon, MAX_ICONS>> = SpinLock::new(Slots::new(0));

fn find_icon(handle: usize) -> Option<Icon> {
    let local = handle.checked_sub(HANDLE_BASE)?;
    ICONS.with(|icons| icons.get(local).copied())
}

/// The colour and mask bitmaps (handles in gdi's object table, which stay the icon's) of icon
/// or cursor `icon`; `None` for a system icon of `LoadIconW`'s, which has no image, and for
/// what is no icon.
pub(super) fn icon_bitmaps(icon: usize) -> Option<(usize, usize)> {
    let entry = find_icon(icon)?;
    Some((entry.colour, entry.mask))
}

/// A zeroed heap block of the layer's, freed when dropped.
#[derive(Debug)]
pub(super) struct HeapBlock {
    address: *mut u8,
    length: usize,
}

impl HeapBlock {
    /// A zeroed block of `length` bytes.
    pub(super) fn new(length: usize) -> Result<HeapBlock, u32> {
        let address = heap::allocate(length.max(1));
        match address.is_null() {
            true => Err(error::NOT_ENOUGH_MEMORY),
            false => Ok(HeapBlock { address, length }),
        }
    }

    pub(super) fn bytes(&self) -> &[u8] {
        // SAFETY: the block holds `length` bytes for as long as `self` lives.
        unsafe { core::slice::from_raw_parts(self.address, self.length) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `bytes`, borrowed uniquely.
        unsafe { core::slice::from_raw_parts_mut(self.address, self.length) }
    }

    /// Its bytes as 32-bit pixels (heap blocks are 16-byte aligned).
    pub(super) fn pixels(&self) -> &[u32] {
        // SAFETY: as in `bytes`; the block is aligned for `u32`.
        unsafe { core::slice::from_raw_parts(self.address.cast(), self.length / 4) }
    }

    pub(super) fn pixels_mut(&mut self) -> &mut [u32] {
        // SAFETY: as in `pixels`, borrowed uniquely.
        unsafe { core::slice::from_raw_parts_mut(self.address.cast(), self.length / 4) }
    }

    /// Hands the block over to the caller, who frees it with `heap::free`.
    pub(super) fn into_raw(self) -> *mut u8 {
        let address = self.address;
        core::mem::forget(self);
        address
    }
}

impl Drop for HeapBlock {
    fn drop(&mut self) {
        // SAFETY: a block `new` allocated, which nothing uses after its owner is gone.
        unsafe { heap::free(self.address) };
    }
}

/// An image in the layer's pixels: `width * height` of them in a heap block, top row first.
#[derive(Debug)]
pub(super) struct Decoded {
    pub(super) pixels: HeapBlock,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl Decoded {
    /// A zeroed image of `width` by `height` pixels.
    pub(super) fn new(width: u32, height: u32) -> Result<Decoded, u32> {
        let side = 1..=dib::LARGEST_SIDE;
        if !side.contains(&width) || !side.contains(&height) {
            return Err(error::INVALID_DATA);
        }
        Ok(Decoded {
            pixels: HeapBlock::new(width as usize * height as usize * 4)?,
            width,
            height,
        })
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn rows(&self, first: u32, count: u32) -> Result<Decoded, u32> {
        let mut part = Decoded::new(self.width, count)?;
        let width = self.width as usize;
        let start = first as usize * width;
        let length = count as usize * width;
        part.pixels
            .pixels_mut()
            .copy_from_slice(&self.pixels.pixels()[start..start + length]);
        Ok(part)
    }
}

/// Decodes the PNG in `bytes` (straight alpha in each pixel's top byte).
pub(super) fn decode_png(bytes: &[u8]) -> Result<Decoded, u32> {
    let (width, height) = dib::png_size(bytes).ok_or(error::INVALID_DATA)?;
    let mut image = Decoded::new(width, height)?;
    let workspace = HeapBlock::new(core::mem::size_of::<RowWorkspace>())?;
    // SAFETY: a zeroed, 16-byte aligned block of a `RowWorkspace`'s size, used only here: all
    // zero is a workspace's starting state, and its arrays take any bytes.
    let work = unsafe { &mut *workspace.address.cast::<RowWorkspace>() };
    match dib::decode_png(bytes, work, image.pixels.pixels_mut()) {
        true => Ok(image),
        false => Err(error::INVALID_DATA),
    }
}

/// Decodes a DIB into the layer's pixels.
pub(super) fn decode_packed(packed: &Packed<'_>) -> Result<Decoded, u32> {
    let mut image = Decoded::new(packed.layout.width, packed.layout.height)?;
    match packed.decode(image.pixels.pixels_mut()) {
        true => Ok(image),
        false => Err(error::INVALID_DATA),
    }
}

/// The whole file the Windows path `name` names, read into a heap block.
pub(super) fn read_file(name: &[u16]) -> Result<HeapBlock, u32> {
    let mut path = [0u8; text::PATH_CAPACITY];
    let length =
        text::eggos_path(text::utf16_chars(name), &mut path).ok_or(error::INVALID_PARAMETER)?;
    let path = &path[..length];
    let fd = linux::open(path, linux::O_RDONLY)
        .map_err(|errno| super::directory::path_error(path, errno))?;
    let read = read_all(fd);
    let _ = linux::close(fd);
    read
}

fn read_all(fd: usize) -> Result<HeapBlock, u32> {
    let size = linux::fstat(fd).map_err(error_for)?[linux::STAT_SIZE_WORD] as usize;
    if size > LARGEST_FILE {
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    let mut block = HeapBlock::new(size)?;
    let mut filled = 0;
    while filled < size {
        match linux::read(fd, &mut block.bytes_mut()[filled..]).map_err(error_for)? {
            0 => break,
            read => filled += read,
        }
    }
    block.length = filled;
    Ok(block)
}

fn copy_pixels(handle: usize) -> Result<Decoded, u32> {
    gdi::read_bitmap(handle, |bitmap, pixels| -> Result<Decoded, u32> {
        let mut copy = Decoded::new(bitmap.width, bitmap.height)?;
        let out = copy.pixels.pixels_mut();
        let count = out.len().min(pixels.len());
        out[..count].copy_from_slice(&pixels[..count]);
        Ok(copy)
    })?
}

fn bitmap_from(image: &Decoded, target: (u32, u32), section: bool) -> Result<usize, u32> {
    let (handle, pixels) = gdi::new_bitmap(target.0, target.1, section)?;
    // SAFETY: a fresh bitmap's `target.0 * target.1` pixels, which nothing else uses yet.
    let out =
        unsafe { core::slice::from_raw_parts_mut(pixels, target.0 as usize * target.1 as usize) };
    dib::scale_nearest(image.pixels.pixels(), image.size(), out, target);
    Ok(handle)
}

fn make_icon(
    colour: &Decoded,
    mask: &Decoded,
    target: (u32, u32),
    hotspot: (u32, u32),
    icon: Bool,
) -> Result<usize, u32> {
    let colour_bitmap = bitmap_from(colour, target, false)?;
    let mask_bitmap = match bitmap_from(mask, target, false) {
        Ok(bitmap) => bitmap,
        Err(code) => {
            gdi::delete_bitmap(colour_bitmap);
            return Err(code);
        }
    };
    let entry = Icon {
        colour: colour_bitmap,
        mask: mask_bitmap,
        width: target.0,
        height: target.1,
        hotspot,
        icon,
    };
    match ICONS.with(|icons| icons.insert(entry)) {
        Some(local) => Ok(HANDLE_BASE + local),
        None => {
            gdi::delete_bitmap(colour_bitmap);
            gdi::delete_bitmap(mask_bitmap);
            Err(error::NOT_ENOUGH_MEMORY)
        }
    }
}

fn scaled(point: (u32, u32), from: (u32, u32), to: (u32, u32)) -> (u32, u32) {
    (
        (u64::from(point.0) * u64::from(to.0) / u64::from(from.0.max(1))) as u32,
        (u64::from(point.1) * u64::from(to.1) / u64::from(from.1.max(1))) as u32,
    )
}

fn desired(width: i32, height: i32, flags: u32) -> (u32, u32) {
    let side = |value: i32| match value {
        value if value > 0 => value as u32,
        _ if flags & LR_DEFAULTSIZE != 0 => DEFAULT_SIDE,
        _ => 0,
    };
    (side(width), side(height))
}

fn or_own(desired: (u32, u32), own: (u32, u32)) -> (u32, u32) {
    (
        if desired.0 == 0 { own.0 } else { desired.0 },
        if desired.1 == 0 { own.1 } else { desired.1 },
    )
}

struct IconImage {
    colour: Decoded,
    mask: Decoded,
}

fn decode_icon_image(data: &[u8]) -> Result<IconImage, u32> {
    if dib::png_size(data).is_some() {
        let colour = decode_png(data)?;
        let mut mask = Decoded::new(colour.width, colour.height)?;
        for (masked, pixel) in mask
            .pixels
            .pixels_mut()
            .iter_mut()
            .zip(colour.pixels.pixels())
        {
            if pixel >> 24 == 0 {
                *masked = dib::MASKED;
            }
        }
        return Ok(IconImage { colour, mask });
    }
    let image = dib::icon_dib(data).ok_or(error::INVALID_DATA)?;
    let colour = decode_packed(&image.colour)?;
    let mut mask = Decoded::new(colour.width, colour.height)?;
    image.decode_mask(mask.pixels.pixels_mut());
    Ok(IconImage { colour, mask })
}

fn icon_from_image(
    data: &[u8],
    icon: Bool,
    hotspot: Option<(u32, u32)>,
    desired: (u32, u32),
) -> Result<usize, u32> {
    let image = decode_icon_image(data)?;
    let own = image.colour.size();
    let target = or_own(desired, own);
    let hotspot = match hotspot {
        Some(hotspot) => scaled(hotspot, own, target),
        None => (target.0 / 2, target.1 / 2),
    };
    make_icon(&image.colour, &image.mask, target, hotspot, icon)
}

fn cursor_image(data: &[u8]) -> Result<((u32, u32), &[u8]), u32> {
    let x = dib::u16_at(data, 0).ok_or(error::INVALID_DATA)?;
    let y = dib::u16_at(data, 2).ok_or(error::INVALID_DATA)?;
    Ok(((u32::from(x), u32::from(y)), &data[HOTSPOT_BYTES..]))
}

fn icon_from_resource_data(data: &[u8], icon: Bool, desired: (u32, u32)) -> Result<usize, u32> {
    match icon {
        FALSE => {
            let (hotspot, image) = cursor_image(data)?;
            icon_from_image(image, FALSE, Some(hotspot), desired)
        }
        _ => icon_from_image(data, TRUE, None, desired),
    }
}

#[repr(C)]
struct IconInfo {
    icon: Bool,
    hotspot_x: u32,
    hotspot_y: u32,
    reserved: u32,
    mask: usize,
    colour: usize,
}

fn normalise_mask(mask: &mut Decoded) {
    for pixel in mask.pixels.pixels_mut() {
        *pixel = match *pixel & WHITE == WHITE {
            true => dib::MASKED,
            false => 0,
        };
    }
}

fn icon_from_bitmaps(info: &IconInfo) -> Result<usize, u32> {
    let whole_mask = copy_pixels(info.mask)?;
    let (colour, mut mask) = match info.colour {
        0 => {
            let height = (whole_mask.height / 2).max(1);
            let colour = match whole_mask.height >= 2 {
                true => whole_mask.rows(height, height)?,
                false => Decoded::new(whole_mask.width, 1)?,
            };
            (colour, whole_mask.rows(0, height)?)
        }
        colour => (copy_pixels(colour)?, whole_mask),
    };
    normalise_mask(&mut mask);
    let size = colour.size();
    let hotspot = match info.icon {
        FALSE => (info.hotspot_x, info.hotspot_y),
        _ => (size.0 / 2, size.1 / 2),
    };
    make_icon(
        &colour,
        &mask,
        size,
        hotspot,
        Bool::from(info.icon != FALSE),
    )
}

unsafe extern "efiapi" fn create_icon_indirect(info: *const IconInfo) -> usize {
    if info.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller passes a readable `ICONINFO`.
    let info = unsafe { info.read_unaligned() };
    match icon_from_bitmaps(&info) {
        Ok(handle) => handle,
        Err(code) => fail(code, 0),
    }
}

unsafe extern "efiapi" fn create_icon_from_resource_ex(
    bits: *const u8,
    size: u32,
    icon: Bool,
    _version: u32,
    width: i32,
    height: i32,
    flags: u32,
) -> usize {
    if bits.is_null() || (size as usize) < HOTSPOT_BYTES {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: `CreateIconFromResourceEx`'s contract: `size` readable bytes.
    let data = unsafe { core::slice::from_raw_parts(bits, size as usize) };
    let icon = Bool::from(icon != FALSE);
    match icon_from_resource_data(data, icon, desired(width, height, flags)) {
        Ok(handle) => handle,
        Err(code) => fail(code, 0),
    }
}

fn copy_bitmap(handle: usize, target: Option<(u32, u32)>, section: bool) -> Result<usize, u32> {
    let source = copy_pixels(handle)?;
    bitmap_from(&source, target.unwrap_or(source.size()), section)
}

unsafe extern "efiapi" fn get_icon_info(handle: usize, out: *mut IconInfo) -> Bool {
    if objects::is_system_icon(handle) {
        return fail(error::NOT_SUPPORTED, FALSE);
    }
    let Some(entry) = find_icon(handle) else {
        return fail(error::INVALID_CURSOR_HANDLE, FALSE);
    };
    if out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let colour = match copy_bitmap(entry.colour, None, false) {
        Ok(colour) => colour,
        Err(code) => return fail(code, FALSE),
    };
    let mask = match copy_bitmap(entry.mask, None, false) {
        Ok(mask) => mask,
        Err(code) => {
            gdi::delete_bitmap(colour);
            return fail(code, FALSE);
        }
    };
    // SAFETY: the caller passes a writable `ICONINFO`.
    unsafe {
        out.write_unaligned(IconInfo {
            icon: entry.icon,
            hotspot_x: entry.hotspot.0,
            hotspot_y: entry.hotspot.1,
            reserved: 0,
            mask,
            colour,
        })
    };
    TRUE
}

fn destroy(handle: usize) -> Result<(), u32> {
    if objects::is_system_icon(handle) {
        return Ok(());
    }
    let local = handle
        .checked_sub(HANDLE_BASE)
        .ok_or(error::INVALID_CURSOR_HANDLE)?;
    let entry = ICONS
        .with(|icons| icons.remove(local))
        .ok_or(error::INVALID_CURSOR_HANDLE)?;
    gdi::delete_bitmap(entry.colour);
    gdi::delete_bitmap(entry.mask);
    Ok(())
}

extern "efiapi" fn destroy_icon(handle: usize) -> Bool {
    match destroy(handle) {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

fn icon_pixel(colour: u32, masked: bool, under: u32, alpha: bool, flags: u32) -> u32 {
    let (image, mask) = (flags & DI_IMAGE != 0, flags & DI_MASK != 0);
    if alpha && image {
        return match mask {
            true => dib::blend(colour, under),
            false => (under & !WHITE) | (colour & WHITE),
        };
    }
    let mut result = under;
    if mask && !masked {
        result &= !WHITE;
    }
    if image {
        result ^= colour & WHITE;
    }
    result
}

#[allow(clippy::too_many_arguments)]
extern "efiapi" fn draw_icon_ex(
    hdc: usize,
    x: i32,
    y: i32,
    handle: usize,
    width: i32,
    height: i32,
    _step: u32,
    background: usize,
    flags: u32,
) -> Bool {
    if objects::is_system_icon(handle) {
        return fail(error::NOT_SUPPORTED, FALSE);
    }
    let Some(entry) = find_icon(handle) else {
        return fail(error::INVALID_CURSOR_HANDLE, FALSE);
    };
    let side = |value: i32, own: u32| match value {
        value if value > 0 => value as u32,
        _ if flags & DI_DEFAULTSIZE != 0 => DEFAULT_SIDE,
        _ => own,
    };
    let (target_width, target_height) = (side(width, entry.width), side(height, entry.height));
    let drawn = paint::draw_in(hdc, |surface, _, gui| {
        let Some(surface) = surface else {
            return Ok(());
        };
        let bitmaps = (
            objects::object(&gui.objects, entry.colour),
            objects::object(&gui.objects, entry.mask),
        );
        let (Some(Object::Bitmap(colour)), Some(Object::Bitmap(mask))) = bitmaps else {
            return Err(error::INVALID_CURSOR_HANDLE);
        };
        let count = colour.width as usize * colour.height as usize;
        let same_size = (mask.width, mask.height) == (colour.width, colour.height);
        if colour.address == 0 || mask.address == 0 || !same_size {
            return Err(error::INVALID_CURSOR_HANDLE);
        }
        // SAFETY: the icon's colour bitmap is in the table, under its lock: `count` pixels.
        let colours = unsafe { core::slice::from_raw_parts(colour.address as *const u32, count) };
        // SAFETY: as above, for its mask bitmap, of the same size.
        let masks = unsafe { core::slice::from_raw_parts(mask.address as *const u32, count) };
        let area = Rect::sized(x, y, target_width as i32, target_height as i32);
        if background != 0
            && let Some(fill) = objects::brush_colour(&gui.objects, background)
        {
            surface.fill(area, fill);
        }
        let alpha = dib::has_alpha(colours);
        let visible = area.intersection(&surface.clip());
        for row in visible.top..visible.bottom {
            let source_row = ((row - y) as u32 * colour.height / target_height) as usize;
            for column in visible.left..visible.right {
                let source_column = ((column - x) as u32 * colour.width / target_width) as usize;
                let at = source_row * colour.width as usize + source_column;
                let Some(under) = surface.get(column, row) else {
                    continue;
                };
                let pixel = icon_pixel(colours[at], masks[at] != 0, under, alpha, flags);
                surface.pixel(column, row, pixel);
            }
        }
        Ok(())
    });
    match drawn {
        Ok(Ok(())) => TRUE,
        Ok(Err(code)) | Err(code) => fail(code, FALSE),
    }
}

fn copy_bitmap_image(handle: usize, width: i32, height: i32, flags: u32) -> Result<usize, u32> {
    let own = gdi::read_bitmap(handle, |bitmap, _| (bitmap.width, bitmap.height))?;
    let target = or_own(desired(width, height, 0), own);
    if flags & LR_COPYRETURNORG != 0 && target == own {
        return Ok(handle);
    }
    let copy = copy_bitmap(handle, Some(target), flags & LR_CREATEDIBSECTION != 0)?;
    if flags & LR_COPYDELETEORG != 0 {
        gdi::delete_bitmap(handle);
    }
    Ok(copy)
}

fn copy_icon(handle: usize, width: i32, height: i32, flags: u32) -> Result<usize, u32> {
    if objects::is_system_icon(handle) {
        return Ok(handle);
    }
    let entry = find_icon(handle).ok_or(error::INVALID_CURSOR_HANDLE)?;
    let own = (entry.width, entry.height);
    let target = or_own(desired(width, height, flags), own);
    if flags & LR_COPYRETURNORG != 0 && target == own {
        return Ok(handle);
    }
    let colour = copy_pixels(entry.colour)?;
    let mask = copy_pixels(entry.mask)?;
    let hotspot = scaled(entry.hotspot, own, target);
    let copy = make_icon(&colour, &mask, target, hotspot, entry.icon)?;
    if flags & LR_COPYDELETEORG != 0 {
        destroy(handle)?;
    }
    Ok(copy)
}

extern "efiapi" fn copy_image(
    handle: usize,
    kind: u32,
    width: i32,
    height: i32,
    flags: u32,
) -> usize {
    let copied = match kind {
        IMAGE_BITMAP => copy_bitmap_image(handle, width, height, flags),
        IMAGE_ICON | IMAGE_CURSOR => copy_icon(handle, width, height, flags),
        _ => Err(error::INVALID_PARAMETER),
    };
    match copied {
        Ok(copy) => copy,
        Err(code) => fail(code, 0),
    }
}

fn choose(candidates: impl Iterator<Item = (u32, u16)>, aim: u32) -> Option<usize> {
    candidates
        .enumerate()
        .min_by_key(|&(_, (side, bits))| {
            let class = match side.cmp(&aim) {
                core::cmp::Ordering::Equal => 0,
                core::cmp::Ordering::Greater => 1,
                core::cmp::Ordering::Less => 2,
            };
            (class, side.abs_diff(aim), u16::MAX - bits)
        })
        .map(|(index, _)| index)
}

fn byte_side(byte: u8) -> u32 {
    match byte {
        0 => 256,
        side => u32::from(side),
    }
}

fn entry_count(directory: &[u8]) -> usize {
    usize::from(dib::u16_at(directory, 4).unwrap_or(0))
}

fn group_choice(group: &[u8], cursor: bool, aim: u32) -> Option<u16> {
    let entry = |index: usize| DIRECTORY_HEADER + index * GROUP_ENTRY;
    let count = (0..entry_count(group))
        .take_while(|&index| group.len() >= entry(index) + GROUP_ENTRY)
        .count();
    let candidates = (0..count).map(|index| {
        let at = entry(index);
        let side = match cursor {
            true => u32::from(dib::u16_at(group, at).unwrap_or(0)),
            false => byte_side(group[at]),
        };
        (side, dib::u16_at(group, at + 6).unwrap_or(0))
    });
    let chosen = choose(candidates, aim)?;
    dib::u16_at(group, entry(chosen) + 12)
}

fn file_choice(file: &[u8], aim: u32) -> Option<(core::ops::Range<usize>, (u32, u32))> {
    let cursor = dib::u16_at(file, 2)? == CURSOR_FILE;
    let entry = |index: usize| DIRECTORY_HEADER + index * FILE_ENTRY;
    let count = (0..entry_count(file))
        .take_while(|&index| file.len() >= entry(index) + FILE_ENTRY)
        .count();
    let candidates = (0..count).map(|index| {
        let at = entry(index);
        let bits = match cursor {
            true => 0,
            false => dib::u16_at(file, at + 6).unwrap_or(0),
        };
        (byte_side(file[at]), bits)
    });
    let at = entry(choose(candidates, aim)?);
    let size = dib::u32_at(file, at + 8)? as usize;
    let offset = dib::u32_at(file, at + 12)? as usize;
    let hotspot = (
        u32::from(dib::u16_at(file, at + 4)?),
        u32::from(dib::u16_at(file, at + 6)?),
    );
    let range = offset..offset.checked_add(size)?;
    file.get(range.clone())?;
    Some((range, hotspot))
}

fn aim(desired: (u32, u32)) -> u32 {
    match desired.0 {
        0 => DEFAULT_SIDE,
        side => side,
    }
}

fn bitmap_from_packed(
    packed: &Packed<'_>,
    width: i32,
    height: i32,
    flags: u32,
) -> Result<usize, u32> {
    let image = decode_packed(packed)?;
    let target = or_own(desired(width, height, 0), image.size());
    bitmap_from(&image, target, flags & LR_CREATEDIBSECTION != 0)
}

fn load_file(path: &[u16], kind: u32, width: i32, height: i32, flags: u32) -> Result<usize, u32> {
    let file = read_file(path)?;
    let bytes = file.bytes();
    match kind {
        IMAGE_BITMAP => {
            let packed = dib::bmp_file(bytes).ok_or(error::INVALID_DATA)?;
            bitmap_from_packed(&packed, width, height, flags)
        }
        IMAGE_ICON | IMAGE_CURSOR => {
            let desired = desired(width, height, flags);
            let (range, hotspot) = file_choice(bytes, aim(desired)).ok_or(error::INVALID_DATA)?;
            let cursor = dib::u16_at(bytes, 2) == Some(CURSOR_FILE);
            let icon = Bool::from(kind == IMAGE_ICON);
            icon_from_image(&bytes[range], icon, cursor.then_some(hotspot), desired)
        }
        _ => Err(error::INVALID_PARAMETER),
    }
}

fn resource_bytes(module: usize, kind: u16, name: Name<'_>) -> Result<&'static [u8], u32> {
    let (address, size) =
        resource::find(module, Name::Id(kind), name).ok_or(error::RESOURCE_NAME_NOT_FOUND)?;
    // SAFETY: a resource's data lies in the program's image, mapped and readable for the
    // process's life.
    Ok(unsafe { core::slice::from_raw_parts(address as *const u8, size as usize) })
}

fn load_resource_image(
    module: usize,
    name: Name<'_>,
    image: u32,
    width: i32,
    height: i32,
    flags: u32,
) -> Result<usize, u32> {
    match image {
        IMAGE_BITMAP => {
            let data = resource_bytes(module, kind::BITMAP, name)?;
            let packed = dib::packed(data).ok_or(error::INVALID_DATA)?;
            bitmap_from_packed(&packed, width, height, flags)
        }
        IMAGE_ICON | IMAGE_CURSOR => {
            let cursor = image == IMAGE_CURSOR;
            let (group_kind, image_kind) = match cursor {
                true => (kind::GROUP_CURSOR, kind::CURSOR),
                false => (kind::GROUP_ICON, kind::ICON),
            };
            let group = resource_bytes(module, group_kind, name)?;
            let desired = desired(width, height, flags);
            let id = group_choice(group, cursor, aim(desired)).ok_or(error::INVALID_DATA)?;
            let data = resource_bytes(module, image_kind, Name::Id(id))?;
            icon_from_resource_data(data, Bool::from(!cursor), desired)
        }
        _ => Err(error::INVALID_PARAMETER),
    }
}

fn system_image(name: usize, image: u32) -> Result<usize, u32> {
    match image {
        IMAGE_ICON if name < IDS => Ok(objects::icon_handle(true, name)),
        IMAGE_CURSOR if name < IDS => Ok(objects::cursor_handle(true, name)),
        _ => Err(error::RESOURCE_NAME_NOT_FOUND),
    }
}

unsafe extern "efiapi" fn load_image_w(
    instance: usize,
    name: usize,
    image: u32,
    width: i32,
    height: i32,
    flags: u32,
) -> usize {
    let loaded = match (flags & LR_LOADFROMFILE != 0, instance) {
        (true, _) if name < IDS => Err(error::INVALID_PARAMETER),
        (true, _) => {
            // SAFETY: `LoadImageW`'s contract: with `LR_LOADFROMFILE`, a NUL-terminated path.
            let path = unsafe { super::file::wide_string(name as *const u16) };
            load_file(path, image, width, height, flags)
        }
        (false, 0) => system_image(name, image),
        (false, module) => {
            // SAFETY: `LoadImageW`'s contract: an id or a NUL-terminated resource name.
            let name = unsafe { resource::name_from(name) };
            load_resource_image(module, name, image, width, height, flags)
        }
    };
    match loaded {
        Ok(handle) => handle,
        Err(code) => fail(code, 0),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CreateIconIndirect" => create_icon_indirect,
    b"CreateIconFromResourceEx" => create_icon_from_resource_ex,
    b"GetIconInfo" => get_icon_info,
    b"DestroyIcon" => destroy_icon,
    b"DrawIconEx" => draw_icon_ex [two_stack_arguments],
    b"CopyImage" => copy_image,
    b"LoadImageW" => load_image_w,
];

const _: () = assert!(core::mem::size_of::<IconInfo>() == 32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_draw_by_alpha_or_by_and_and_xor() {
        let normal = DI_MASK | DI_IMAGE;
        assert_eq!(
            icon_pixel(0x0012_3456, false, 0x00ff_ffff, false, normal),
            0x0012_3456
        );
        assert_eq!(icon_pixel(0, true, 0x0065_4321, false, normal), 0x0065_4321);
        assert_eq!(
            icon_pixel(0x00ff_ffff, true, 0x00f0_f0f0, false, normal),
            0x000f_0f0f
        );
        assert_eq!(
            icon_pixel(0x0012_3456, false, 0x0065_4321, false, DI_MASK),
            0
        );
        assert_eq!(icon_pixel(0xff12_3456, false, 0, true, normal), 0x0012_3456);
        assert_eq!(
            icon_pixel(0x0012_3456, true, 0x0065_4321, true, normal),
            0x0065_4321
        );
        assert_eq!(
            icon_pixel(0x0012_3456, true, 0x0065_4321, true, DI_IMAGE),
            0x0012_3456
        );
        assert_eq!(
            icon_pixel(0x0012_3456, false, 0x0065_4321, false, 0),
            0x0065_4321
        );
    }

    #[test]
    fn the_nearest_size_with_the_most_colours_is_chosen() {
        let sizes = [(16, 32), (32, 4), (32, 32), (48, 32)];
        assert_eq!(choose(sizes.into_iter(), 32), Some(2));
        assert_eq!(choose(sizes.into_iter(), 24), Some(2));
        assert_eq!(choose([(32, 4), (32, 8)].into_iter(), 32), Some(1));
        assert_eq!(choose(sizes.into_iter(), 64), Some(3));
        assert_eq!(choose(sizes.into_iter(), 8), Some(0));
        assert_eq!(choose(core::iter::empty(), 32), None);
    }

    fn directory(kind: u16, entries: &[&[u8]]) -> Vec<u8> {
        let mut bytes = vec![0, 0];
        bytes.extend_from_slice(&kind.to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes
    }

    #[test]
    fn group_resources_and_icon_files_name_their_images() {
        let small = [16, 16, 0, 0, 1, 0, 32, 0, 0, 0, 0, 0, 5, 0];
        let large = [0, 0, 0, 0, 1, 0, 32, 0, 0, 0, 0, 0, 6, 0];
        let group = directory(1, &[&small, &large]);
        assert_eq!(group_choice(&group, false, 16), Some(5));
        assert_eq!(group_choice(&group, false, 32), Some(6));
        assert_eq!(group_choice(&group[..20], false, 32), Some(5));
        let cursor_entry = [32, 0, 64, 0, 1, 0, 1, 0, 0, 0, 0, 0, 9, 0];
        assert_eq!(
            group_choice(&directory(2, &[&cursor_entry]), true, 32),
            Some(9)
        );
        let mut entry = vec![32, 32, 0, 0, 3, 0, 4, 0];
        entry.extend_from_slice(&2u32.to_le_bytes());
        entry.extend_from_slice(&22u32.to_le_bytes());
        let mut file = directory(2, &[&entry]);
        file.extend_from_slice(&[0xaa, 0xbb]);
        assert_eq!(file_choice(&file, 32), Some((22..24, (3, 4))));
        assert_eq!(file_choice(&file[..23], 32), None);
    }

    #[test]
    fn sizes_follow_the_program_or_the_image() {
        assert_eq!(desired(0, 0, 0), (0, 0));
        assert_eq!(desired(0, 0, LR_DEFAULTSIZE), (32, 32));
        assert_eq!(desired(48, -1, 0), (48, 0));
        assert_eq!(or_own((0, 20), (16, 16)), (16, 20));
        assert_eq!(scaled((8, 8), (16, 16), (32, 64)), (16, 32));
        assert_eq!(aim((0, 0)), 32);
    }
}
