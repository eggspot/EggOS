//! The runtime of EggOS's own Linux programs: their entry point, which relocates the program
//! (a static PIE) and reads its start-up stack, Linux system calls and their error numbers, and
//! the memory functions the compiler calls. Programs name their main function with [`main!`].

#![cfg_attr(not(test), no_std)]
#![no_builtins]

#[cfg(all(eggos_kernel, eggos_user))]
compile_error!(
    "a build is the kernel's (`eggos_kernel`) or a program's (`eggos_user`), never both"
);
#[cfg(all(target_env = "musl", not(eggos_user)))]
compile_error!("a musl build of the runtime needs `--cfg eggos_user` (.cargo/config.toml)");

#[cfg(eggos_user)]
mod entry;
pub mod errno;
pub mod futex;
#[cfg(eggos_user)]
mod memory;
#[cfg(target_arch = "mips64")]
mod mips_abi;
pub mod process;
pub mod relocation;
pub mod signal;
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
