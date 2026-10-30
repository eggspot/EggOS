//! The desktop shell's side in the kernel (D133, D134): the app catalogue read from the file
//! systems, with each app's own icon its manifest names (D148, the installer's `app.icon`,
//! taken only when it is exactly the size the format has), and the programs the launcher
//! starts. The catalogue is read when the desktop
//! starts, each time the launcher opens and when the installer says the apps changed, never
//! otherwise; an app starts as a process of its own, beside init, as the user's.

use crate::linux::exec::{self, ProgramBytes};
use crate::linux::{self, process_table};
use crate::process::Process;
use crate::sync::SpinLock;
use crate::vfs::{self, NodeKind};
use alloc::sync::Arc;
use alloc::vec::Vec;
use eggos_app_manifest as manifest;
use eggos_drivers::desktop::Catalogue;
use eggos_hal::Platform;

/// Where the host's shared folder is mounted (D130).
pub const HOST_SHARE: &[u8] = b"/media/host";
/// The Files app's program, when the build has it.
pub const FILES_PROGRAM: &[u8] = b"/bin/files";
/// The Text Editor's program, when the build has it (D149).
pub const TEXT_EDITOR_PROGRAM: &[u8] = b"/bin/text-editor";
/// The terminal program (D187), a name of the Settings program.
pub const TERMINAL_PROGRAM: &[u8] = b"/bin/terminal";

const ENVIRONMENT: [&[u8]; 4] = [
    b"HOME=/",
    b"TERM=xterm",
    b"XDG_RUNTIME_DIR=/run/user/0",
    b"WAYLAND_DISPLAY=wayland-0",
];

type Starter = Arc<dyn Fn(&[u8]) -> Result<(), &'static str> + Send + Sync>;

static STARTER: SpinLock<Option<Starter>> = SpinLock::new(None);

fn read_small_file(node: vfs::Node, largest: usize) -> Option<Vec<u8>> {
    let size = node.attributes().size;
    if node.kind() != NodeKind::Regular || size > largest as u64 {
        return None;
    }
    let bytes = ProgramBytes::read(node, size, size).ok()?;
    Some(bytes.bytes().to_vec())
}

/// The catalogue as the file systems hold it now: every app under `/Applications` whose
/// manifest is valid, whether the host share is mounted and whether the build has Files and
/// the Text Editor.
pub fn catalogue() -> Catalogue {
    let mut catalogue = Catalogue::new();
    let root = vfs::root();
    catalogue.set_host_share(vfs::is_mount_point(HOST_SHARE));
    catalogue.set_files_present(vfs::resolve(root, root, FILES_PROGRAM).is_ok());
    catalogue.set_text_editor_present(vfs::resolve(root, root, TEXT_EDITOR_PROGRAM).is_ok());
    catalogue.set_terminal_present(vfs::resolve(root, root, TERMINAL_PROGRAM).is_ok());
    let Ok(directory) = vfs::resolve(root, root, manifest::DIRECTORY.as_bytes()) else {
        return catalogue;
    };
    if !directory.is_directory() {
        return catalogue;
    }
    let mut position = 0;
    while let Some((name, node, kind)) = directory.entry(position) {
        position += 1;
        if kind != NodeKind::Directory || matches!(name.as_bytes(), b"." | b"..") {
            continue;
        }
        let Some(file) = node
            .lookup(manifest::FILE_NAME.as_bytes())
            .ok()
            .and_then(|file| read_small_file(file, manifest::LARGEST))
        else {
            continue;
        };
        if catalogue.add_installed(name.as_bytes(), &file)
            && let Some(icon) = manifest::parse(&file, name.as_bytes()).and_then(|app| app.icon)
            && let Some(bytes) = vfs::resolve(root, node, icon.as_bytes())
                .ok()
                .and_then(|icon| read_small_file(icon, manifest::icon::FILE_BYTES))
        {
            catalogue.set_icon(name.as_bytes(), &bytes);
        }
        if catalogue.installed_count() == eggos_drivers::desktop::MAX_INSTALLED {
            break;
        }
    }
    catalogue
}

const MAX_ARGUMENTS: usize = 8;

/// Starts the program `command` names (its path, then its arguments, separated by
/// [`eggos_drivers::desktop::ARGUMENT_SEPARATOR`]) as a process of its own, as init's sibling
/// started by the kernel, with its address space made beside `beside`'s.
pub fn start<P: Platform>(beside: &Process<P>, command: &[u8]) -> Result<(), &'static str> {
    let mut arguments: [&[u8]; MAX_ARGUMENTS] = [&[]; MAX_ARGUMENTS];
    let mut count = 0;
    for word in command.split(|&byte| byte == eggos_drivers::desktop::ARGUMENT_SEPARATOR) {
        *arguments.get_mut(count).ok_or("too many arguments")? = word;
        count += 1;
    }
    let arguments = &arguments[..count];
    let path = arguments[0];
    let root = vfs::root();
    let node = vfs::resolve(root, root, path).map_err(|_| "no such program")?;
    let attributes = node.attributes();
    if attributes.kind != NodeKind::Regular {
        return Err("not a program");
    }
    let image = ProgramBytes::read(node, attributes.size, u64::MAX).map_err(|_| "unreadable")?;
    let mut process = beside.new_beside()?;
    let id = process.id().get();
    process_table::TABLE
        .lock()
        .add(id, None)
        .map_err(|_| "no room in the process table")?;
    let Ok(context) = exec::load(&mut process, image.bytes(), path, arguments, &ENVIRONMENT) else {
        process_table::TABLE.lock().remove_unended(id);
        return Err("not a program this machine runs");
    };
    let process = Arc::new(process);
    let file_name = path.rsplit(|&byte| byte == b'/').next().unwrap_or(path);
    process_table::TABLE
        .lock()
        .set_name(id, process_table::name_of(file_name));
    if linux::spawn::<P>(process, context).is_err() {
        process_table::TABLE.lock().remove_unended(id);
        return Err("could not start its thread");
    }
    Ok(())
}

/// Makes `starter` what [`start_program`] calls: init's starting of programs beside itself.
pub fn set_starter(starter: Starter) {
    *STARTER.lock() = Some(starter);
}

/// Starts the program `command` names ([`start`]) through the starter init registered; an
/// error when there is none yet or the program cannot start.
pub fn start_program(command: &[u8]) -> Result<(), &'static str> {
    let starter = STARTER
        .lock()
        .clone()
        .ok_or("nothing starts programs yet")?;
    starter(command)
}
