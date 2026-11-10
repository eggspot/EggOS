//! The component boot test (D238): two boots of the release image, whose base holds the Windows
//! layer only as a link into its component, on a blank disk, with the host folder holding
//! `win-paint.exe` and the component store attached read-only, on each architecture whose
//! store offers `windows-apps`.
//!
//! 1. `app install /media/host/win-paint.exe` is refused with the offer to run
//!    `component install windows-apps`; that command installs the layer from the store, and
//!    `app install` then succeeds; `component remove --yes windows-apps` removes it, leaving
//!    `/var/lib/eggos/components` empty; installing it again succeeds.
//! 2. The same disk boots again and `/var/lib/eggos/components` still holds `windows-apps`.

use super::install::WINDOWS_PROGRAM;
use super::settings::Boot;
use super::*;
use eggos_components::index::{self, Component, Header, Index, SECTOR};
use eggos_components::messages;
use std::path::PathBuf;

const TEST_DISK_BYTES: u64 = 64 << 20;
const WINDOWS_APPS: &str = "windows-apps";
const COMPONENTS_FOLDER: &str = "/var/lib/eggos/components";
const LISTED_MARK: &str = "EGGOS-COMPONENT listed";

/// The component store a boot test attaches, and the index it was built with.
pub(super) struct StoreDisk {
    path: PathBuf,
    index: Index,
}

impl StoreDisk {
    /// Builds the store of `arch` and reads its index back as the component manager does.
    pub(super) fn build(arch: &Arch) -> Result<Self, String> {
        let path = crate::components::build_store(arch)?;
        let image = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let unreadable = |error| format!("{}: not a component store: {error:?}", path.display());
        let header = Header::parse(&image).map_err(unreadable)?;
        let text = image
            .get(SECTOR as usize..)
            .and_then(|rest| rest.get(..header.index_bytes as usize))
            .ok_or_else(|| format!("{}: the index is cut short", path.display()))?;
        let index = index::parse_index(&header, text, image.len() as u64).map_err(unreadable)?;
        Ok(Self { path, index })
    }

    /// QEMU's arguments that attach it.
    pub(super) fn args(&self, arch: &Arch) -> Vec<String> {
        store_disk_args(arch, &self.path.to_string_lossy())
    }

    /// The component `name` it offers.
    pub(super) fn component(&self, name: &str) -> Result<&Component, String> {
        self.index
            .components
            .iter()
            .find(|component| component.name == name)
            .ok_or_else(|| format!("{}: the store offers no {name}", self.path.display()))
    }
}

/// The console line that installs `name` and echoes its status after `mark`.
pub(super) fn install_line(name: &str, mark: &str) -> String {
    format!("component install {name}; echo EGGOS-COMPONENT {mark} $?\r")
}

/// What [`install_line`] prints when it installs `component`.
pub(super) fn installed_output(component: &Component, mark: &str) -> [String; 2] {
    [
        messages::installed(component),
        format!("EGGOS-COMPONENT {mark} 0"),
    ]
}

fn first_session() -> Vec<String> {
    vec![
        format!("app install /media/host/{WINDOWS_PROGRAM}; echo EGGOS-APP offer $?\r"),
        install_line(WINDOWS_APPS, "installed"),
        format!("app install /media/host/{WINDOWS_PROGRAM}; echo EGGOS-APP status $?\r"),
        format!("component remove --yes {WINDOWS_APPS}; echo EGGOS-COMPONENT removed $?\r"),
        format!("ls {COMPONENTS_FOLDER}; echo {LISTED_MARK} $?\r"),
        install_line(WINDOWS_APPS, "reinstalled"),
        "poweroff\r".to_owned(),
    ]
}

fn first_output(arch: &Arch, component: &Component) -> Vec<String> {
    let mut lines = vec![
        format!(
            "app: EggOS cannot run {WINDOWS_PROGRAM} on {}: it needs the Windows layer, which is \
             not installed: install it with `component install {WINDOWS_APPS}`",
            arch.name
        ),
        "EGGOS-APP offer 1".to_owned(),
    ];
    lines.extend(installed_output(component, "installed"));
    lines.push("Installed win-paint in /Applications/win-paint.".to_owned());
    lines.push("EGGOS-APP status 0".to_owned());
    lines.push(messages::removed(component));
    lines.push("EGGOS-COMPONENT removed 0".to_owned());
    lines.push(format!("{LISTED_MARK} 0"));
    lines.extend(installed_output(component, "reinstalled"));
    lines.push(POWER_DOWN_LINE.to_owned());
    lines
}

fn second_session() -> Vec<String> {
    vec![
        format!("ls {COMPONENTS_FOLDER}; echo {LISTED_MARK} $?\r"),
        "poweroff\r".to_owned(),
    ]
}

fn second_output() -> Vec<String> {
    vec![
        WINDOWS_APPS.to_owned(),
        format!("{LISTED_MARK} 0"),
        POWER_DOWN_LINE.to_owned(),
    ]
}

fn lines_in_order(arch: &Arch, console: &str, wanted: &[String]) -> Result<(), String> {
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    let mut at = 0;
    for line in wanted {
        match lines[at..].iter().position(|candidate| candidate == line) {
            Some(found) => at += found + 1,
            None => {
                return Err(format!(
                    "{}: the component test's console has no `{line}` line where expected",
                    arch.name
                ));
            }
        }
    }
    Ok(())
}

fn listed_nothing(arch: &Arch, console: &str) -> Result<(), String> {
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    let listed = format!("{LISTED_MARK} 0");
    let at = lines
        .iter()
        .position(|line| *line == listed)
        .ok_or_else(|| format!("{}: no `{listed}` line", arch.name))?;
    match at.checked_sub(1).map(|before| lines[before]) {
        Some(before) if before.ends_with(&format!("echo {LISTED_MARK} $?")) => Ok(()),
        before => Err(format!(
            "{}: {COMPONENTS_FOLDER} still held something after the remove: `{}`",
            arch.name,
            before.unwrap_or_default()
        )),
    }
}

fn boot(
    arch: &Arch,
    kernel: &Path,
    args: &[String],
    session: &[String],
) -> Result<String, String> {
    let mut boot = Boot::start(arch, kernel, args)?;
    let session: Vec<&[u8]> = session.iter().map(|line| line.as_bytes()).collect();
    for _ in 0..session.len() {
        boot.type_next(&session)?;
    }
    let (console, clean_exit) = boot.finish()?;
    match clean_exit {
        true => Ok(console),
        false => Err(format!(
            "{}: QEMU did not exit cleanly in the component test",
            arch.name
        )),
    }
}

/// Runs the component boot test (see the module documentation) on `kernel`, the release image
/// for `arch`, with `folder` (the host folder holding [`WINDOWS_PROGRAM`]) attached; on an
/// architecture whose store offers no Windows layer it does nothing.
pub fn check(arch: &Arch, kernel: &Path, folder: &Path) -> Result<(), String> {
    if !crate::components::offered(arch.name).any(|offered| offered.name == WINDOWS_APPS) {
        return Ok(());
    }
    let store = StoreDisk::build(arch)?;
    let component = store.component(WINDOWS_APPS)?;
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}-components-disk.img", arch.name));
    reset_disk_image(&image, TEST_DISK_BYTES)?;
    let mut args = disk_args(arch, &image.to_string_lossy());
    args.extend(host_folder_args(arch, &folder.to_string_lossy()));
    args.extend(store.args(arch));
    let console = boot(arch, kernel, &args, &first_session())?;
    lines_in_order(arch, &console, &first_output(arch, component))?;
    listed_nothing(arch, &console)?;
    let console = boot(arch, kernel, &args, &second_session())?;
    lines_in_order(arch, &console, &second_output())?;
    eprintln!("xtask: {}: component boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_typed_line_has_its_echo_and_the_lines_come_in_order() {
        let first = first_session();
        assert_eq!(first.len(), 7);
        assert!(first.iter().all(|line| line.ends_with('\r')));
        assert_eq!(
            first[1],
            "component install windows-apps; echo EGGOS-COMPONENT installed $?\r"
        );
        let console = "eggsh:/# ls /var/lib/eggos/components; echo EGGOS-COMPONENT listed $?\r\n\
                       EGGOS-COMPONENT listed 0\r\n";
        let arch = &crate::ARCHES[0];
        assert!(listed_nothing(arch, console).is_ok());
        let left = console.replace("$?\r\n", "$?\r\nwindows-apps\r\n");
        assert!(listed_nothing(arch, &left).is_err());
        assert!(lines_in_order(arch, &left, &second_output()[..2]).is_ok());
        assert!(lines_in_order(arch, console, &second_output()[..2]).is_err());
    }
}
