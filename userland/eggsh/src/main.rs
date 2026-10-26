//! eggsh, the EggOS shell (D107): init's login program, at `/bin/eggsh`. Interactive on a
//! terminal, it writes a prompt, reads a line, and runs its commands, each program in a
//! process group of its own that has the terminal while it runs; `^C` reaches only that
//! program, and at the prompt gives a fresh one. `eggsh file [arguments]` runs a file of
//! commands, `eggsh -c text [name [arguments]]` the commands of `text`. The language is in
//! `syntax.rs`, the shell in `shell.rs`, both pure logic with host tests; `system.rs` makes
//! the system calls.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(any(test, target_env = "musl"))]
mod path;
#[cfg(any(test, target_env = "musl"))]
mod shell;
#[cfg(any(test, target_env = "musl"))]
mod status;
#[cfg(any(test, target_env = "musl"))]
mod syntax;
#[cfg(target_env = "musl")]
mod system;
#[cfg(any(test, target_env = "musl"))]
mod variables;

#[cfg(target_env = "musl")]
eggos_runtime::main!(system::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
