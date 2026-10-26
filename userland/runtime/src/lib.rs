//! The runtime of EggOS's own Linux programs: their entry point, which relocates the program
//! (a static PIE) and reads its start-up stack, Linux system calls, and the memory functions
//! the compiler calls. Programs name their main function with [`main!`].

#![cfg_attr(not(test), no_std)]
#![no_builtins]

#[cfg(target_env = "musl")]
mod entry;
#[cfg(target_env = "musl")]
mod memory;
pub mod process;
pub mod relocation;
pub mod startup;
pub mod syscall;

/// Names the program's main function, `fn(&Startup) -> i32`: the runtime calls it once the
/// program is relocated and ends the process with the status it returns.
#[macro_export]
macro_rules! main {
    ($main:path) => {
        #[unsafe(no_mangle)]
        fn eggos_main(startup: &$crate::startup::Startup) -> i32 {
            $main(startup)
        }
    };
}
