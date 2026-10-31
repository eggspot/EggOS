//! `settings`: with no arguments, opens the Settings window; `settings get <key>`, `settings
//! set <key> <value>` and `settings list` get and set the same settings from the command line
//! (D22, D132). Started as `/usr/libexec/app-window`, a hard link to the same program, it is
//! the installer's window (D134), as `/bin/files` the Files window (D135), and as
//! `/bin/text-editor` the Text Editor with `/bin/edit` its command (D149), as `/bin/terminal`
//! the terminal program (D187), and as `/bin/disk` the `disk` command (D192), and as `/bin/task-manager` the Task Manager
//! (D200), so the toolkit
//! is in the image once.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(target_env = "musl")]
extern crate alloc;

#[cfg(target_env = "musl")]
mod disk_system;
#[cfg(target_env = "musl")]
mod program;

#[cfg(target_env = "musl")]
eggos_runtime::main!(program::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
