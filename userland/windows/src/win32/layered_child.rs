//! Layered child windows (D721): a child with `WS_EX_LAYERED` and `SetLayeredWindowAttributes`
//! draws into its top-level window's buffer like any child, so the pixels it is about to cover
//! are kept when a device context of it opens and, when the context is freed, what it drew is
//! blended over them by its constant alpha, and its colour key lets them show through.

use super::SpinLock;
use super::gui::Gui;
use crate::gui::windows;
use core::sync::atomic::{AtomicBool, Ordering};
use eggos_windows_gui::blit;
use eggos_windows_gui::surface::Rect;

const MOST_KEPT: usize = 8;
const COLOUR_BITS: u32 = 0x00ff_ffff;

/// How a layered child blends over what is under it: its colour key as `0x00RRGGBB` and its
/// constant alpha, each when set.
#[derive(Debug, Clone, Copy)]
pub(super) struct ChildBlend {
    pub key: Option<u32>,
    pub alpha: Option<u8>,
}

#[derive(Debug, Clone, Copy)]
struct Kept {
    hdc: usize,
    root: usize,
    buffer: usize,
    area: Rect,
    pixels: usize,
    blend: ChildBlend,
}

static KEPT: SpinLock<[Option<Kept>; MOST_KEPT]> = SpinLock::new([None; MOST_KEPT]);
static ANY_LAYERED_CHILD: AtomicBool = AtomicBool::new(false);

/// A child window became layered: device contexts start keeping what is under such children.
pub(super) fn note_layered_child() {
    ANY_LAYERED_CHILD.store(true, Ordering::Relaxed);
}

/// Context `hdc` of `window` opened over `local` (client coordinates), under the state's lock:
/// when `window` is a layered child, the top-level buffer's pixels there are kept.
pub(super) fn keep_under(gui: &Gui, hdc: usize, window: usize, local: Rect) {
    if !ANY_LAYERED_CHILD.load(Ordering::Relaxed) {
        return;
    }
    if !gui.windows.get(window).is_some_and(|entry| entry.is_child()) {
        return;
    }
    let Some(blend) = super::window_state::child_blend(window) else {
        return;
    };
    let Some(placement) = windows::client_placement(&gui.windows, window) else {
        return;
    };
    let Some(root) = gui.windows.get(placement.root).filter(|root| root.pixels != 0) else {
        return;
    };
    let buffer = Rect::sized(0, 0, root.buffer_width as i32, root.buffer_height as i32);
    let area = local
        .offset(placement.origin.0, placement.origin.1)
        .intersection(&placement.clip)
        .intersection(&buffer);
    if area.is_empty() {
        return;
    }
    let width = area.width() as usize;
    let pixels = super::heap::allocate(width * area.height() as usize * 4) as *mut u32;
    if pixels.is_null() {
        return;
    }
    for row in 0..area.height() as usize {
        let from = (area.top as usize + row) * root.buffer_width as usize + area.left as usize;
        // SAFETY: `area` lies in the root's buffer, held by the caller's lock; `pixels` fits it.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (root.pixels as *const u32).add(from),
                pixels.add(row * width),
                width,
            )
        };
    }
    let kept = Kept {
        hdc,
        root: placement.root,
        buffer: root.pixels,
        area,
        pixels: pixels as usize,
        blend,
    };
    let stored = KEPT.with(|table| {
        let slot = table.iter_mut().find(|slot| slot.is_none())?;
        *slot = Some(kept);
        Some(())
    });
    if stored.is_none() {
        // SAFETY: `pixels` came from `heap::allocate` above and nothing else holds it.
        unsafe { super::heap::free(pixels as *mut u8) };
    }
}

/// Context `hdc` is being freed, under the state's lock: when it kept what was under a layered
/// child, what the child drew is blended over that.
pub(super) fn blend_kept(gui: &Gui, hdc: usize) {
    if !ANY_LAYERED_CHILD.load(Ordering::Relaxed) {
        return;
    }
    let found = KEPT.with(|table| {
        table
            .iter_mut()
            .find(|slot| slot.is_some_and(|kept| kept.hdc == hdc))
            .and_then(Option::take)
    });
    let Some(kept) = found else {
        return;
    };
    let root = gui.windows.get(kept.root).filter(|root| {
        root.pixels == kept.buffer
            && kept.area.right <= root.buffer_width as i32
            && kept.area.bottom <= root.buffer_height as i32
    });
    if let Some(root) = root {
        blend_rows(root.pixels as *mut u32, root.buffer_width as usize, &kept);
    }
    // SAFETY: `kept.pixels` came from `heap::allocate`, and nothing holds it once taken out.
    unsafe { super::heap::free(kept.pixels as *mut u8) };
}

fn blend_rows(buffer: *mut u32, stride: usize, kept: &Kept) {
    let width = kept.area.width() as usize;
    let under = kept.pixels as *const u32;
    for row in 0..kept.area.height() as usize {
        let line = (kept.area.top as usize + row) * stride + kept.area.left as usize;
        for column in 0..width {
            // SAFETY: the caller checked that `kept.area` still lies inside the root's buffer.
            let drawn = unsafe { buffer.add(line + column) };
            // SAFETY: `kept.pixels` holds `width` pixels for each row of `kept.area`.
            let beneath = unsafe { under.add(row * width + column).read() };
            // SAFETY: as above.
            let pixel = unsafe { drawn.read() };
            let keyed = kept.blend.key == Some(pixel & COLOUR_BITS);
            let shown = match (keyed, kept.blend.alpha) {
                (true, _) => beneath,
                (false, Some(alpha)) => blit::blend_over(pixel, beneath, alpha, false),
                (false, None) => pixel,
            };
            // SAFETY: as above.
            unsafe { drawn.write(shown) };
        }
    }
}
