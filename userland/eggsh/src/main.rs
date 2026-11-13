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

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(any(test, eggos_user))]
mod arguments;
#[cfg(any(test, eggos_user))]
mod arithmetic;
#[cfg(any(test, eggos_user))]
mod editor;
#[cfg(any(test, eggos_user))]
mod functions;
#[cfg(any(test, eggos_user))]
mod glob;
#[cfg(any(test, eggos_user))]
mod jobs;
#[cfg(any(test, eggos_user))]
mod path;
#[cfg(any(test, eggos_user))]
mod pattern;
#[cfg(any(test, eggos_user))]
mod shell;
#[cfg(any(test, eggos_user))]
mod status;
#[cfg(any(test, eggos_user))]
mod syntax;
#[cfg(eggos_user)]
mod system;
#[cfg(any(test, eggos_user))]
mod test;
#[cfg(any(test, eggos_user))]
mod variables;

#[cfg(eggos_user)]
eggos_runtime::main!(system::run);

#[cfg(not(eggos_user))]
fn main() {}
