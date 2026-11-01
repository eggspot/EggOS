//! eggsh, the EggOS shell (D107): init's login program, at `/bin/eggsh`. Interactive on a
//! terminal, it writes a prompt, reads a line, and runs its commands, each program in a
//! process group of its own that has the terminal while it runs; `^C` reaches only that
//! program, `^Z` stops it as a job, and at the prompt `^C` gives a fresh one (D181).
//! `eggsh file [arguments]` runs a file of commands, `eggsh -c text [name [arguments]]` the
//! commands of `text`. The words are read in `syntax.rs`, the grammar and the shell in
//! `shell.rs` and `shell/`, patterns, file name expansion, `test`, the positional parameters
//! and the functions in `pattern.rs`, `glob.rs`, `test.rs`, `arguments.rs` and
//! `functions.rs` (D182), its jobs in `jobs.rs`, line editing and history at the prompt in
//! `editor.rs`, all pure logic with host tests; `system.rs` makes the system calls.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(any(test, target_env = "musl"))]
mod arguments;
#[cfg(any(test, target_env = "musl"))]
mod editor;
#[cfg(any(test, target_env = "musl"))]
mod functions;
#[cfg(any(test, target_env = "musl"))]
mod glob;
#[cfg(any(test, target_env = "musl"))]
mod jobs;
#[cfg(any(test, target_env = "musl"))]
mod path;
#[cfg(any(test, target_env = "musl"))]
mod pattern;
#[cfg(any(test, target_env = "musl"))]
mod shell;
#[cfg(any(test, target_env = "musl"))]
mod status;
#[cfg(any(test, target_env = "musl"))]
mod syntax;
#[cfg(target_env = "musl")]
mod system;
#[cfg(any(test, target_env = "musl"))]
mod test;
#[cfg(any(test, target_env = "musl"))]
mod variables;

#[cfg(target_env = "musl")]
eggos_runtime::main!(system::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
