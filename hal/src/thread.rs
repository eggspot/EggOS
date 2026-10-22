//! Kernel thread contexts, the same way on every architecture (D35).
//!
//! A thread that is not running is described by one word, its saved context: the stack pointer
//! at which [`crate::Platform::switch_thread`] left it. The switch pushes the registers the
//! architecture's calling convention says a function must preserve onto the running thread's
//! stack, stores the stack pointer, loads the other thread's and pops that thread's registers,
//! then returns into it. Everything else the compiler already saved around the call, so a switch
//! costs a few stores and loads and no allocation.
//!
//! A new thread gets an initial frame laid out exactly as a switch would have left it: its
//! "return address" is the architecture's thread trampoline, and two of the restored registers
//! hold the entry function and its argument, which the trampoline passes on.

/// The most bytes any architecture's initial frame takes below the stack top; a thread's stack
/// must be larger.
pub const INITIAL_FRAME_MAX: usize = 128;

/// Writes `frame` (lowest address first) at the top of the stack that ends at `stack_top`,
/// rounded down to 16 bytes, and returns its address: the new thread's saved context. The frame
/// ends exactly at the rounded top, so once the switch has popped it, the trampoline runs with
/// the stack pointer 16-byte aligned, as every architecture's calling convention requires.
///
/// # Safety
/// The `frame.len()` words below `stack_top` rounded down to 16 must be writable memory that
/// nothing else uses.
#[cfg_attr(not(target_os = "none"), allow(dead_code))]
pub(crate) unsafe fn write_initial_frame(stack_top: usize, frame: &[usize]) -> usize {
    debug_assert!(size_of_val(frame) <= INITIAL_FRAME_MAX);
    let start = (stack_top & !15) - size_of_val(frame);
    // SAFETY: the `frame.len()` words from `start` are the caller's stack (see above), aligned
    // as `start` is a multiple of 16 minus whole words, and nothing else uses them, so they
    // cannot overlap `frame`.
    unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(), start as *mut usize, frame.len()) };
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_ends_at_the_aligned_top() {
        let mut stack = [0usize; 32];
        let base = stack.as_mut_ptr() as usize;
        // A top that is not 16-byte aligned: the frame ends at the aligned address below it.
        let top = ((base + 31 * size_of::<usize>()) & !15) + 8;
        // SAFETY: the three words below `top` rounded down are inside `stack`.
        let context = unsafe { write_initial_frame(top, &[1, 2, 3]) };
        assert_eq!(context, (top & !15) - 24);
        let index = (context - base) / size_of::<usize>();
        assert_eq!(stack[index..index + 3], [1, 2, 3]);
        assert_eq!(stack[index + 3..], vec![0; 32 - index - 3][..]);
    }
}
