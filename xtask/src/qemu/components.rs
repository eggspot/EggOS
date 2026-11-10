//! The component boot test (D235): two boots of the release image, whose base holds the Windows
//! layer only as a link into its component, on a blank disk, with the host folder holding
//! `win-paint.exe` and the component store attached read-only, on each architecture whose
//! store offers `windows-apps`.
//!
//! 1. `app install /media/host/win-paint.exe` is refused with the offer to run
//!    `component install windows-apps`; that command installs the layer from the store, and
//!    `app install` then succeeds; `component remove --yes windows-apps` removes it, leaving
//!    `/var/lib/eggos/components` empty; installing it again succeeds.
//! 2. The same disk boots again and `/var/lib/eggos/components` still holds `windows-apps`.
//! 3. A blank disk of its own, through Settings (D235, D231): `settings --pane components`
//!    opens in the background, `wm show` lists the row's `Install Windows apps`, which
//!    `wm act` invokes, until the row's button is `Remove Windows apps` and the sidebar lists
//!    `Windows`; with Settings closed, `win-paint.exe` is installed and opened, its window
//!    found with `wm list` and closed; Settings opened again on Components, `Remove Windows
//!    apps` invoked shows the confirmation, whose `Remove` is person-only: `wm act` on it is
//!    refused as `element_cannot` and the layer stays; `settings --pane windows` shows the
//!    `Keys` table.

use super::install::WINDOWS_PROGRAM;
use super::mcp::{element_line, element_of};
use super::settings::Boot;
use super::*;
use eggos_components::index::{self, Component, Header, Index, SECTOR};
use eggos_components::messages;
use eggos_settings::settings_window::{COMPONENTS, PANES, TITLE as SETTINGS_TITLE, WINDOWS};
use std::path::PathBuf;

const TEST_DISK_BYTES: u64 = 64 << 20;
const WINDOWS_APPS: &str = "windows-apps";
const COMPONENTS_FOLDER: &str = "/var/lib/eggos/components";
const LISTED_MARK: &str = "EGGOS-COMPONENT listed";
const STEP_PREFIX: &str = "EGGOS-SETTINGS-PANE ";
const SETTINGS_BOOT_TIMEOUT: Duration = Duration::from_secs(1500);
const STEP_TIMEOUT: Duration = Duration::from_secs(120);
const POLLS: usize = 60;
const POLL_PAUSE: Duration = Duration::from_secs(2);
const CONFIRM_BUTTON: &str = "Remove";
const KEYS_TABLE: &str = "Keys";
const PERSON_ONLY: &str = "person-only";
const DISABLED: &str = "disabled";
const ELEMENT_CANNOT: &str = "that element does not take that action";
const ELEMENT_CANNOT_STATUS: i32 = 1;

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
        format!("ls -A {COMPONENTS_FOLDER}; echo {LISTED_MARK} $?\r"),
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
        format!("ls -A {COMPONENTS_FOLDER}; echo {LISTED_MARK} $?\r"),
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

fn boot(arch: &Arch, kernel: &Path, args: &[String], session: &[String]) -> Result<String, String> {
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

fn console_text(boot: &mut Boot<'_>) -> String {
    boot.console.extend(boot.output.try_iter().flatten());
    String::from_utf8_lossy(&boot.console).into_owned()
}

fn type_text(boot: &mut Boot<'_>, text: &str) -> Result<(), String> {
    boot.keyboard
        .write_all(text.as_bytes())
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", boot.arch.name))
}

fn step_result(console: &str, step: &str) -> Option<(String, i32)> {
    let marker = format!("{STEP_PREFIX}{step} ");
    let echo = format!("{STEP_PREFIX}{step} $?");
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    let (end, status) = lines.iter().enumerate().rev().find_map(|(at, line)| {
        let status = line.strip_prefix(&marker)?.parse().ok()?;
        Some((at, status))
    })?;
    let start = lines[..end]
        .iter()
        .rposition(|line| line.ends_with(&echo))
        .map_or(0, |at| at + 1);
    Some((lines[start..end].join("\n"), status))
}

fn run_step(boot: &mut Boot<'_>, command: &str, step: &str) -> Result<(String, i32), String> {
    console_text(boot);
    let prompts = boot.prompts();
    let from = boot.console.len();
    type_text(boot, &format!("{command}; echo {STEP_PREFIX}{step} $?\r"))?;
    let started = Instant::now();
    loop {
        console_text(boot);
        let typed_since = String::from_utf8_lossy(&boot.console[from..]).into_owned();
        if let Some(result) = step_result(&typed_since, step) {
            boot.wait_for_prompts(prompts + 1)?;
            return Ok(result);
        }
        if started.elapsed() > STEP_TIMEOUT {
            return Err(format!(
                "{}: `{command}` did not finish within {}s",
                boot.arch.name,
                STEP_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn succeeds(boot: &mut Boot<'_>, command: &str, step: &str) -> Result<String, String> {
    match run_step(boot, command, step)? {
        (output, 0) => Ok(output),
        (output, status) => Err(format!(
            "{}: `{command}` ended with status {status}: {output}",
            boot.arch.name
        )),
    }
}

fn start_in_background(boot: &mut Boot<'_>, command: &str) -> Result<(), String> {
    console_text(boot);
    let prompts = boot.prompts();
    type_text(boot, &format!("{command} &\r"))?;
    boot.wait_for_prompts(prompts + 1)
}

fn poll<T>(
    boot: &mut Boot<'_>,
    command: &str,
    step: &str,
    found: impl Fn(&str) -> Option<T>,
) -> Result<T, String> {
    let mut last = String::new();
    for attempt in 0..POLLS {
        last = succeeds(boot, command, &format!("{step}-{attempt}"))?;
        if let Some(found) = found(&last) {
            return Ok(found);
        }
        std::thread::sleep(POLL_PAUSE);
    }
    Err(format!(
        "{}: `{command}` never showed what {step} waits for; last: {last}",
        boot.arch.name
    ))
}

fn window_titled(listed: &str, title: &str) -> Option<u32> {
    listed.lines().find_map(|line| {
        let fields: Vec<&str> = line.trim().splitn(8, ' ').collect();
        match fields.get(7) {
            Some(shown) if shown.trim() == title => fields[0].parse().ok(),
            _ => None,
        }
    })
}

fn person_only_button(tree: &str, name: &str) -> Option<u32> {
    element_of(tree, "button", name).filter(|&id| element_line(tree, id).contains(PERSON_ONLY))
}

fn enabled_button(tree: &str, name: &str) -> Option<u32> {
    element_of(tree, "button", name).filter(|&id| !element_line(tree, id).contains(DISABLED))
}

fn open_window(boot: &mut Boot<'_>, command: &str, title: &str, step: &str) -> Result<u32, String> {
    start_in_background(boot, command)?;
    poll(boot, "wm list", step, |listed| window_titled(listed, title))
}

fn close_window(boot: &mut Boot<'_>, window: u32, title: &str, step: &str) -> Result<(), String> {
    succeeds(
        boot,
        &format!("wm close {window}"),
        &format!("{step}-close"),
    )?;
    poll(boot, "wm list", step, |listed| {
        window_titled(listed, title).is_none().then_some(())
    })?;
    run_step(boot, "wait", &format!("{step}-wait")).map(drop)
}

fn open_settings(boot: &mut Boot<'_>, pane: usize) -> Result<u32, String> {
    let name = PANES[pane].to_lowercase();
    open_window(
        boot,
        &format!("settings --pane {name}"),
        SETTINGS_TITLE,
        &format!("settings-{name}"),
    )
}

fn install_from_settings(boot: &mut Boot<'_>, component: &Component) -> Result<(), String> {
    let install = format!("Install {}", component.title);
    let remove = format!("Remove {}", component.title);
    let window = open_settings(boot, COMPONENTS)?;
    let button = poll(boot, &format!("wm show {window}"), "installable", |tree| {
        enabled_button(tree, &install)
    })?;
    succeeds(boot, &format!("wm act {window} {button} invoke"), "install")?;
    poll(boot, &format!("wm show {window}"), "installed", |tree| {
        let removable = element_of(tree, "button", &remove).is_some();
        let row = element_of(tree, "list-item", PANES[WINDOWS]).is_some();
        (removable && row).then_some(())
    })?;
    close_window(boot, window, SETTINGS_TITLE, "components-closed")
}

fn run_windows_program(boot: &mut Boot<'_>) -> Result<(), String> {
    let paint = eggos_windows_gui_tests::TITLE;
    succeeds(
        boot,
        &format!("app install /media/host/{WINDOWS_PROGRAM}"),
        "paint-installed",
    )?;
    let window = open_window(boot, &format!("app open {paint}"), paint, "paint-opened")?;
    close_window(boot, window, paint, "paint-closed")
}

fn remove_is_the_person_s(boot: &mut Boot<'_>, component: &Component) -> Result<(), String> {
    let arch = boot.arch.name;
    let remove = format!("Remove {}", component.title);
    let window = open_settings(boot, COMPONENTS)?;
    let button = poll(boot, &format!("wm show {window}"), "removable", |tree| {
        enabled_button(tree, &remove)
    })?;
    succeeds(boot, &format!("wm act {window} {button} invoke"), "remove")?;
    let confirm = poll(boot, &format!("wm show {window}"), "confirming", |tree| {
        person_only_button(tree, CONFIRM_BUTTON)
    })?;
    let act = format!("wm act {window} {confirm} invoke");
    let (said, status) = run_step(boot, &act, "confirm")?;
    let refusal = format!("{window} {confirm}: {ELEMENT_CANNOT}");
    if status != ELEMENT_CANNOT_STATUS || !said.lines().any(|line| line.ends_with(&refusal)) {
        return Err(format!(
            "{arch}: `{act}` was not refused as element_cannot (status {status}): {said}"
        ));
    }
    let kept = succeeds(boot, &format!("ls -A {COMPONENTS_FOLDER}"), "kept")?;
    if !kept.lines().any(|line| line == component.name) {
        return Err(format!(
            "{arch}: {COMPONENTS_FOLDER} lost {} after the refused confirmation: {kept}",
            component.name
        ));
    }
    close_window(boot, window, SETTINGS_TITLE, "removable-closed")
}

fn windows_pane(boot: &mut Boot<'_>) -> Result<(), String> {
    let window = open_settings(boot, WINDOWS)?;
    poll(boot, &format!("wm show {window}"), "windows", |tree| {
        element_of(tree, "table", KEYS_TABLE)
    })?;
    close_window(boot, window, SETTINGS_TITLE, "windows-closed")
}

fn through_settings(boot: &mut Boot<'_>, component: &Component) -> Result<(), String> {
    boot.wait_for_prompts(1)?;
    install_from_settings(boot, component)?;
    run_windows_program(boot)?;
    remove_is_the_person_s(boot, component)?;
    windows_pane(boot)
}

fn settings_boot(
    arch: &Arch,
    kernel: &Path,
    args: &[String],
    component: &Component,
) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, args)?.with_timeout(SETTINGS_BOOT_TIMEOUT);
    if let Err(error) = through_settings(&mut boot, component) {
        return Err(boot.fail(error));
    }
    if let Err(error) = type_text(&mut boot, "poweroff\r") {
        return Err(boot.fail(error));
    }
    let (console, clean_exit) = boot.finish()?;
    match clean_exit && has_line(&console, POWER_DOWN_LINE) {
        true => Ok(()),
        false => Err(format!(
            "{}: QEMU did not power off cleanly after the Settings component steps",
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
    let settings_image =
        directory.join(format!("eggos-{}-components-settings-disk.img", arch.name));
    reset_disk_image(&settings_image, TEST_DISK_BYTES)?;
    let mut settings_args = disk_args(arch, &settings_image.to_string_lossy());
    settings_args.extend(host_folder_args(arch, &folder.to_string_lossy()));
    settings_args.extend(store.args(arch));
    settings_boot(arch, kernel, &settings_args, component)?;
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
        assert_eq!(
            first[4],
            "ls -A /var/lib/eggos/components; echo EGGOS-COMPONENT listed $?\r"
        );
        let console = "eggsh:/# ls -A /var/lib/eggos/components; echo EGGOS-COMPONENT listed $?\r\n\
                       EGGOS-COMPONENT listed 0\r\n";
        let arch = &crate::ARCHES[0];
        assert!(listed_nothing(arch, console).is_ok());
        let half_removed = console.replace("$?\r\n", "$?\r\n.removing-windows-apps\r\n");
        assert!(listed_nothing(arch, &half_removed).is_err());
        let left = console.replace("$?\r\n", "$?\r\nwindows-apps\r\n");
        assert!(listed_nothing(arch, &left).is_err());
        assert!(lines_in_order(arch, &left, &second_output()[..2]).is_ok());
        assert!(lines_in_order(arch, console, &second_output()[..2]).is_err());
    }

    #[test]
    fn a_step_s_output_and_status_and_the_windows_and_buttons_in_it_are_read_back() {
        let console = "eggsh:/# wm list; echo EGGOS-SETTINGS-PANE opened-0 $?\r\n\
                       ID STATE X Y WIDTH HEIGHT CLIENT TITLE\r\n\
                       64 normal 0 0 640 400 65535 Terminal\r\n\
                       129 normal 40 40 640 480 3 Settings\r\n\
                       EGGOS-SETTINGS-PANE opened-0 0\r\n";
        let (listed, status) = step_result(console, "opened-0").unwrap();
        assert_eq!(status, 0);
        assert_eq!(listed.lines().count(), 3);
        assert_eq!(window_titled(&listed, SETTINGS_TITLE), Some(129));
        assert_eq!(window_titled(&listed, "win-paint"), None);
        assert_eq!(step_result(console, "opened-1"), None);
        let tree = "1 window 0,0 640x480 \"Settings\"\n  \
                    612 button 336,220 128x28 [focusable,default,person-only] \"Remove\"\n  \
                    540 button 336,134 96x28 [disabled] \"Install Windows apps\"\n";
        assert_eq!(person_only_button(tree, CONFIRM_BUTTON), Some(612));
        assert_eq!(enabled_button(tree, CONFIRM_BUTTON), Some(612));
        assert_eq!(enabled_button(tree, "Install Windows apps"), None);
        assert_eq!(person_only_button(tree, "Install Windows apps"), None);
    }
}
