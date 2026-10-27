//! `settings`: with no arguments, opens the Settings window; `settings get <key>`, `settings
//! set <key> <value>` and `settings list` get and set the same settings from the command line
//! (D22, D132). Started as `/usr/libexec/app-window`, a hard link to the same program, it is
//! the installer's window (D134), so the toolkit is in the image once.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(target_env = "musl")]
mod program;

#[cfg(target_env = "musl")]
eggos_runtime::main!(program::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
