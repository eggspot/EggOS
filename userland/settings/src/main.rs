//! `settings`: with no arguments, opens the Settings window; `settings get <key>`, `settings
//! set <key> <value>` and `settings list` get and set the same settings from the command line
//! (D22, D132). Started under the name of one of its hard links it is another program, so the
//! toolkit is in the image once: as `/usr/libexec/app-window` the installer's window and as
//! `/bin/app` the `app` command (D134, D214), as `/bin/files` the Files window (D135), as
//! `/bin/text-editor` the Text Editor with `/bin/edit` its command (D149), as `/bin/terminal`
//! the terminal program (D187), as `/bin/disk` the `disk` command (D192), and as
//! `/bin/task-manager` the Task Manager (D200).

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
