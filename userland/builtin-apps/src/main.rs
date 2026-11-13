//! The built-in apps (D396): Files, the Text Editor with its `edit` command, and Downloads, one
//! program installed as the `builtin-apps` component (D18, docs/ux/components-v0.3.md) at
//! `/usr/libexec/builtin-apps`, started by the links `/bin/files`, `/bin/text-editor`,
//! `/bin/edit` and `/bin/downloads`, which the base keeps dangling until the component is
//! installed. A machine without the component has none of them and pays nothing for them.

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(eggos_user)]
extern crate alloc;

#[cfg(eggos_user)]
eggos_runtime::main!(run);

#[cfg(eggos_user)]
#[global_allocator]
static MEMORY: eggos_runtime::pages::Pages = eggos_runtime::pages::Pages;

#[cfg(eggos_user)]
const USAGE: &[u8] = b"builtin-apps: start it as files, text-editor, edit or downloads\n";

#[cfg(eggos_user)]
fn run(startup: &eggos_runtime::startup::Startup) -> i32 {
    let started_as = startup
        .arguments()
        .next()
        .map_or(&b""[..], core::ffi::CStr::to_bytes);
    match eggos_app::files::file_name(started_as) {
        b"files" => eggos_files::program::run(startup),
        b"text-editor" => eggos_editor::program::run(startup),
        b"edit" => eggos_editor::command::run(startup),
        b"downloads" => eggos_downloads::program::run(startup),
        _ => {
            eggos_app::linux::write_all(eggos_app::linux::STANDARD_ERROR, USAGE);
            2
        }
    }
}

/// The CPU features Linux's auxiliary vector reports, which compiler-builtins' aarch64 atomics
/// ask for: none, so they use load-exclusive and store-exclusive pairs, which every Armv8 CPU
/// has.
#[cfg(all(eggos_user, target_arch = "aarch64"))]
#[unsafe(no_mangle)]
pub extern "C" fn getauxval(_: core::ffi::c_ulong) -> core::ffi::c_ulong {
    0
}

#[cfg(not(eggos_user))]
fn main() {}
