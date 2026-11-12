//! The user view (KPTI, D502): on CPUs that let a program read kernel memory speculatively
//! (Meltdown, [`Platform::isolates_user_view`]), each CPU gets page tables whose kernel half maps
//! only what entering and leaving the kernel needs ([`Platform::user_view_ranges`]), at the same
//! addresses and with the same frames as the kernel's tables, never reachable from user mode.
//! The HAL runs programs on them and switches to the kernel's on every way in; it fills their
//! user half from the program's tables at every entry.

use crate::memory::FRAMES;
use crate::paging::{MapError, PageTables};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;

/// Gives the calling CPU its user view when the CPU needs one; why not, if it could not (the
/// CPU then runs programs without it, as on CPUs that need none). Called by each CPU once its
/// block is set and the frame allocator works, before it runs a program.
pub fn set_up<P: Platform>() -> Result<(), &'static str> {
    if !P::isolates_user_view() {
        return Ok(());
    }
    let image = P::kernel_image();
    let image_text = image.text_start.wrapping_add(image.virtual_offset)
        ..image.rodata_start.wrapping_add(image.virtual_offset);
    let image_range = image.start.wrapping_add(image.virtual_offset)
        ..image.end.wrapping_add(image.virtual_offset);
    let physical = |virt: u64| match image_range.contains(&virt) {
        true => virt.wrapping_sub(image.virtual_offset),
        false => virt.wrapping_sub(P::physical_memory_offset() as u64),
    };
    let mut guard = FRAMES.lock();
    let frames = guard.as_mut().ok_or("no frame allocator")?;
    let mut tables =
        PageTables::<P::PageTableFormat>::new(frames).map_err(MapError::description)?;
    let mut failed = None;
    P::user_view_ranges(&mut |start, length| {
        let first = start & !(FRAME_SIZE - 1);
        let end = start.saturating_add(length).next_multiple_of(FRAME_SIZE);
        for page in (first..end).step_by(FRAME_SIZE as usize) {
            if failed.is_some() || tables.translate(page).is_some() {
                continue;
            }
            let attributes = match image_text.contains(&page) {
                true => Attributes::TEXT,
                false => Attributes::READ_WRITE,
            };
            if let Err(error) = tables.map(page, physical(page), FRAME_SIZE, attributes, frames) {
                failed = Some(error);
            }
        }
    });
    if let Some(error) = failed {
        return Err(error.description());
    }
    let root = tables.roots()[0];
    drop(guard);
    // SAFETY: `root` is the root of tables mapping only kernel ranges the HAL named, at their
    // own addresses and frames, which nothing frees; this CPU's block is set (the caller's duty)
    // and the kernel runs with interrupts masked.
    unsafe { P::set_user_view(root) };
    Ok(())
}
