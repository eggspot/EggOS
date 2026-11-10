//! `settings`: with no arguments, opens the Settings window; `settings get <key>`, `settings
//! set <key> <value>` and `settings list` get and set the same settings from the command line
//! (D22, D132). Started under the name of one of its hard links it is another program, so the
//! toolkit is in the image once: as `/usr/libexec/app-window` the installer's window and as
//! `/bin/app` the `app` command (D134, D214), as `/bin/files` the Files window (D135), as
//! `/bin/text-editor` the Text Editor with `/bin/edit` its command (D149), as `/bin/terminal`
//! the terminal program (D187), as `/bin/disk` the `disk` command (D192), as
//! `/bin/task-manager` the Task Manager (D200), and as `/bin/component` the `component`
//! command (D238).

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(eggos_user)]
extern crate alloc;

#[cfg(eggos_user)]
mod disk_system;
#[cfg(eggos_user)]
mod program;

#[cfg(eggos_user)]
eggos_runtime::main!(run);

#[cfg(eggos_user)]
fn run(startup: &eggos_runtime::startup::Startup) -> i32 {
    let started_as = startup
        .arguments()
        .next()
        .map_or(&b""[..], core::ffi::CStr::to_bytes);
    match eggos_app::files::file_name(started_as) {
        b"component" => eggos_components::program::run(startup),
        _ => program::run(startup),
    }
}

#[cfg(not(eggos_user))]
fn main() {}
