//! User programs (M4, D47).
//!
//! The kernel runs a program in user mode with [`Platform::run_user`] inside its address
//! space ([`AddressSpace::with_active`]), handles whatever took the CPU back (a system call, an
//! interrupt, an exception) and runs it again. Processes that own their address space and
//! threads, loading programs from files, and the Linux system calls come next (roadmap M4, M6);
//! for now the only program is the HAL's test program, for the boot self-tests and benchmarks.

use crate::address_space::AddressSpace;
use crate::memory;
use crate::paging::PageTables;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;
use eggos_hal::user::{self, UserContext};

/// Where [`LoadedTestProgram`] maps the HAL's test program: its first page.
pub const TEST_PROGRAM_ADDRESS: u64 = 0x40_0000;

/// The HAL's test program ([`Platform::user_test_program`]) in an address space of its own: a
/// copy of its code in one frame, mapped [`Attributes::USER_TEXT`] at
/// [`TEST_PROGRAM_ADDRESS`]. Dropping it frees the frame and the address space's tables.
pub struct LoadedTestProgram<P: Platform> {
    /// Always `Some` until dropped: taken first in `drop`, so the tables go before the frame
    /// they map.
    space: Option<AddressSpace<P>>,
    frame: u64,
    program: user::TestProgram,
}

impl<P: Platform> LoadedTestProgram<P> {
    /// Copies the HAL's test program into a new frame, the rest of it zero, and maps it in a new
    /// address space sharing the kernel half of `kernel`.
    ///
    /// # Safety
    /// As [`AddressSpace::new`]: `kernel` must be the kernel's own page tables, active on every
    /// CPU that runs the program.
    pub unsafe fn new(kernel: &PageTables<P::PageTableFormat>) -> Result<Self, &'static str> {
        let program = P::user_test_program();
        if program.code.len() > FRAME_SIZE as usize {
            return Err("the test program is larger than a page");
        }
        let frame = {
            let mut frames = memory::FRAMES.lock();
            let frames = frames.as_mut().ok_or("there is no frame allocator")?;
            let frame = frames.allocate().ok_or("no frame for the test program")?;
            let page = (frame as usize).wrapping_add(frames.physical_memory_offset()) as *mut u8;
            // SAFETY: the frame was just allocated, so this code owns it, and the allocator
            // reaches it through its offset. Zeroed first, so no stale kernel data is left in a
            // page the program can read.
            unsafe { core::ptr::write_bytes(page, 0, FRAME_SIZE as usize) };
            // SAFETY: as above; the code fits in the frame (checked above) and is in the kernel
            // image, so the two do not overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(program.code.as_ptr(), page, program.code.len())
            };
            P::make_instructions_visible(page as usize, program.code.len());
            frame
        };
        // SAFETY: `kernel` is the kernel's own tables, active on every CPU (the caller's duty).
        let mapped = unsafe { AddressSpace::new(kernel) }.and_then(|mut space| {
            space
                .map(
                    TEST_PROGRAM_ADDRESS,
                    frame,
                    FRAME_SIZE,
                    Attributes::USER_TEXT,
                )
                .map(|()| space)
        });
        match mapped {
            Ok(space) => Ok(Self {
                space: Some(space),
                frame,
                program,
            }),
            Err(error) => {
                free_frame(frame);
                Err(error.description())
            }
        }
    }

    /// A context that starts the program at `entry`, an offset from the HAL's
    /// [`user::TestProgram`] (say, its `checks`), with no stack.
    pub fn context(&self, entry: impl FnOnce(&user::TestProgram) -> usize) -> UserContext {
        UserContext::new(TEST_PROGRAM_ADDRESS + entry(&self.program) as u64, 0)
    }

    /// Runs `active` with the program's address space active on this CPU, as
    /// [`AddressSpace::with_active`]: [`Platform::run_user`] runs the program there.
    pub fn with_active<R>(&self, active: impl FnOnce() -> R) -> R {
        self.space
            .as_ref()
            .expect("the address space lives until drop")
            .with_active(active)
    }
}

impl<P: Platform> core::fmt::Debug for LoadedTestProgram<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LoadedTestProgram")
            .field("frame", &self.frame)
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}

impl<P: Platform> Drop for LoadedTestProgram<P> {
    fn drop(&mut self) {
        drop(self.space.take());
        free_frame(self.frame);
    }
}

/// Gives the test program's frame back to the kernel's frame allocator.
fn free_frame(frame: u64) {
    let mut frames = memory::FRAMES.lock();
    let frames = frames
        .as_mut()
        .expect("the frame came from the frame allocator");
    // SAFETY: the frame came from this allocator in `LoadedTestProgram::new`, and nothing maps it any
    // more: the address space that did is gone (or was never made).
    unsafe { frames.free(frame) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TestPlatform as P;
    use crate::thread::host::frames_for_test;

    /// Kernel tables, active (frozen) as the kernel's are.
    fn kernel_tables() -> PageTables<<P as Platform>::PageTableFormat> {
        let mut frames = memory::FRAMES.lock();
        let mut tables = PageTables::new(frames.as_mut().unwrap()).unwrap();
        tables.mark_active();
        tables
    }

    #[test]
    fn the_program_is_mapped_as_user_code_and_everything_is_freed() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        let free = || memory::FRAMES.lock().as_ref().unwrap().free_frames();
        let before = free();
        {
            // SAFETY: test tables, never active; the test platform's switch only records roots.
            let program = unsafe { LoadedTestProgram::<P>::new(&kernel) }.unwrap();
            let space = program.space.as_ref().unwrap();
            let (phys, attributes) = space.translate(TEST_PROGRAM_ADDRESS).unwrap();
            assert_eq!((phys, attributes), (program.frame, Attributes::USER_TEXT));
            let code = P::user_test_program().code;
            let offset = memory::FRAMES
                .lock()
                .as_ref()
                .unwrap()
                .physical_memory_offset();
            // SAFETY: the program's frame, through the test allocator's offset.
            let page = unsafe {
                core::slice::from_raw_parts(
                    (phys as usize).wrapping_add(offset) as *const u8,
                    FRAME_SIZE as usize,
                )
            };
            assert_eq!(&page[..code.len()], code);
            assert!(page[code.len()..].iter().all(|&byte| byte == 0));
            let context = program.context(|program| program.spin);
            assert_eq!(
                context.pc(),
                TEST_PROGRAM_ADDRESS + P::user_test_program().spin as u64
            );
            assert!(free() < before);
        }
        assert_eq!(free(), before);
    }
}
