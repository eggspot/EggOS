//! Every sentence the component manager says (docs/ux/components-v0.3.md sections 3, 4 and 7):
//! one function each, shared by the Components list, `component` and the MCP tools, so the
//! three front-ends say the same words (D22).

use crate::index::Component;
use crate::manager::{Failure, Refusal, Store};
use alloc::format;
use alloc::string::String;
use eggos_commands::{ErrorDefinition, errors};

const KIBIBYTE: u64 = 1024;

/// A size as Files writes it: `0 B`, `712 KB`, `1.8 MB`.
pub fn size_text(size: u64) -> String {
    if size < KIBIBYTE {
        return format!("{size} B");
    }
    let mut unit = KIBIBYTE;
    for name in ["KB", "MB", "GB", "TB"] {
        let next = unit.saturating_mul(KIBIBYTE);
        if size < next || name == "TB" {
            let tenths = size.saturating_mul(10) / unit;
            return match tenths < 100 {
                true => format!("{}.{} {name}", tenths / 10, tenths % 10),
                false => format!("{} {name}", size / unit),
            };
        }
        unit = next;
    }
    String::new()
}

/// `Windows apps (1.8 MB)`.
pub fn titled(component: &Component) -> String {
    format!(
        "{} ({})",
        component.title,
        size_text(component.size_bytes())
    )
}

/// `Installing Windows apps (1.8 MB)...`.
pub fn installing(component: &Component) -> String {
    format!("Installing {}...", titled(component))
}

/// `Installed Windows apps (1.8 MB).`.
pub fn installed(component: &Component) -> String {
    format!("Installed {}.", titled(component))
}

/// `Windows apps is already installed.`.
pub fn already_installed(title: &str) -> String {
    format!("{title} is already installed.")
}

fn joined(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What installing `plan` (dependencies first, the asked one last) installs besides the asked
/// one: `Installs Desktop (2.6 MB) too: Built-in apps need it.`; `None` with nothing besides.
pub fn also_installs(plan: &[&Component]) -> Option<String> {
    let (wanted, needed) = plan.split_last()?;
    if needed.is_empty() {
        return None;
    }
    let names: alloc::vec::Vec<String> = needed.iter().map(|need| titled(need)).collect();
    let pronoun = match needed.len() {
        1 => "it",
        _ => "them",
    };
    Some(format!(
        "Installs {} too: {} need {pronoun}.",
        joined(&names),
        wanted.title
    ))
}

/// `3.7 MB in all.`.
pub fn in_all(plan: &[&Component]) -> String {
    let total: u64 = plan.iter().map(|component| component.size_bytes()).sum();
    format!("{} in all.", size_text(total))
}

/// `would install Desktop (2.6 MB) and Built-in apps (1.1 MB): 3.7 MB in all`.
pub fn would_install(plan: &[&Component]) -> String {
    let names: alloc::vec::Vec<String> = plan.iter().map(|component| titled(component)).collect();
    let total: u64 = plan.iter().map(|component| component.size_bytes()).sum();
    format!(
        "would install {}: {} in all",
        joined(&names),
        size_text(total)
    )
}

/// `Remove Windows apps (1.8 MB)?`.
pub fn remove_question(component: &Component) -> String {
    format!("Remove {}?", titled(component))
}

/// `Removing Windows apps needs a yes: add --yes.`.
pub fn needs_yes(component: &Component) -> String {
    format!("Removing {} needs a yes: add --yes.", component.title)
}

/// `Removed Windows apps; 1.8 MB freed.`.
pub fn removed(component: &Component) -> String {
    format!(
        "Removed {}; {} freed.",
        component.title,
        size_text(component.size_bytes())
    )
}

/// `would remove Windows apps (1.8 MB)`.
pub fn would_remove(component: &Component) -> String {
    format!("would remove {}", titled(component))
}

/// `Windows apps is not installed.`.
pub fn not_installed(title: &str) -> String {
    format!("{title} is not installed.")
}

/// `Nothing removed.`.
pub const DECLINED: &str = "Nothing removed.";
/// `Cancelled.`.
pub const CANCELLED: &str = "Cancelled.";
/// The second line of every failed install.
pub const NOTHING_INSTALLED: &str = "Nothing was installed.";
/// The second line of a failed remove.
pub const STAYS_INSTALLED: &str = "It stays installed.";
/// The second line of a remove that left files behind.
pub const FILES_LEFT: &str = "The next install or remove deletes them.";
/// What a store that cannot be read is told.
pub const STORE_UNREADABLE: &str =
    "The attached component store could not be read: it is damaged or not a store.";

/// The no-store sentence: `No component store is attached: attach the store disk, or put
/// <file> in the host share. run-eggos.sh attaches it automatically.`.
pub fn no_store(file_name: &str) -> String {
    format!(
        "No component store is attached: attach the store disk, or put {file_name} in the host \
         share. run-eggos.sh attaches it automatically."
    )
}

/// The store line `component list` writes on standard error.
pub fn store_line(store: Option<&Store>, file_name: &str) -> String {
    match store {
        Some(store) => format!(
            "store: {} (EggOS {}, {}) {}",
            store.file_name(),
            store.index.version,
            store.index.arch,
            where_text(store)
        ),
        None => format!(
            "store: none. Attach the store disk, or put {file_name} in the host share; \
             run-eggos.sh attaches it automatically."
        ),
    }
}

/// `on /dev/vdc` or `in the host share`.
pub fn where_text(store: &Store) -> String {
    match store.source {
        crate::manager::Source::Disk(disk) => format!("on {disk}"),
        crate::manager::Source::Share => String::from("in the host share"),
        crate::manager::Source::Downloaded => String::from("downloaded from the release"),
        crate::manager::Source::Upstream => String::from("downloaded from linux-firmware"),
    }
}

/// What a refusal says, in one line, and its error code.
pub fn refusal(refusal: &Refusal, file_name: &str) -> (String, &'static ErrorDefinition) {
    match refusal {
        Refusal::NoStore => (no_store(file_name), &errors::NO_STORE),
        Refusal::NoSuchComponent(name) => (
            format!("No component named \"{name}\": component list names them."),
            &errors::NO_SUCH_COMPONENT,
        ),
        Refusal::Busy { title, removing } => (
            format!(
                "Busy: {title} {}. Wait for it, then try again.",
                match removing {
                    true => "is being removed",
                    false => "is installing",
                }
            ),
            &errors::COMPONENT_BUSY,
        ),
        Refusal::NeededBy(titles) => (
            format!(
                "Needed by {}: remove {} first.",
                joined(titles),
                match titles.len() {
                    1 => "that",
                    _ => "those",
                }
            ),
            &errors::NEEDED_BY,
        ),
        Refusal::InUse { names, count } => (in_use(names, *count), &errors::IN_USE),
        Refusal::NotAllowed => (
            String::from("Not allowed: only root installs or removes components."),
            &errors::PERMISSION_DENIED,
        ),
    }
}

/// `In use: Notepad is running. Quit it, then remove.`, or the several programs' form.
pub fn in_use(names: &[String], count: usize) -> String {
    match count {
        1 => format!(
            "In use: {} is running. Quit it, then remove.",
            joined(names)
        ),
        2 if names.len() == 2 => format!(
            "In use: {} are running. Quit them, then remove.",
            joined(names)
        ),
        _ => format!("In use: {count} programs are running. Quit them, then remove."),
    }
}

/// What a failed install says: the first line, the second line, and the error code.
pub fn install_failure(
    failure: &Failure,
    component: &Component,
) -> (String, &'static str, &'static ErrorDefinition) {
    match failure {
        Failure::Damaged(file) => (
            format!("The store's copy of {file} is damaged."),
            NOTHING_INSTALLED,
            &errors::STORE_DAMAGED,
        ),
        Failure::Detached => (
            String::from("The store was detached before the copy finished."),
            NOTHING_INSTALLED,
            &errors::STORE_DETACHED,
        ),
        Failure::NoSpace { needed, free } => (
            match needed {
                0 => String::from("Not enough space."),
                _ => format!(
                    "Not enough space: {} needed, {} free.",
                    size_text(*needed),
                    size_text(*free)
                ),
            },
            NOTHING_INSTALLED,
            &errors::NO_SPACE,
        ),
        Failure::Cancelled => (
            String::from(CANCELLED),
            NOTHING_INSTALLED,
            &errors::DECLINED_BY_USER,
        ),
        Failure::NotAllowed => (
            String::from("Not allowed: only root installs or removes components."),
            "",
            &errors::PERMISSION_DENIED,
        ),
        Failure::UnsafeFolder => (
            format!(
                "Could not install {}: {} is not a real folder.",
                component.title,
                crate::manager::COMPONENTS
            ),
            NOTHING_INSTALLED,
            &errors::INSTALL_FAILED,
        ),
        Failure::Errno(errno) | Failure::FilesLeft(errno) => (
            format!(
                "Could not install {}: {}.",
                component.title,
                errno_text(*errno)
            ),
            NOTHING_INSTALLED,
            &errors::INSTALL_FAILED,
        ),
    }
}

/// A failed install's one line, as `component install` says it: the two lines joined with `; `
/// and the second lowercased at its start.
pub fn install_failure_line(failure: &Failure, component: &Component) -> String {
    let (first, second, _) = install_failure(failure, component);
    match second {
        "" => first,
        second => format!(
            "{}; {}{}",
            first.trim_end_matches('.'),
            second[..1].to_ascii_lowercase(),
            &second[1..]
        ),
    }
}

/// What a failed remove says: the first line and the second (`""` for none), as the
/// Components list shows them.
pub fn remove_failure(failure: &Failure, component: &Component) -> (String, &'static str) {
    let reason = match failure {
        Failure::FilesLeft(errno) => {
            return (
                format!(
                    "Removed {}, but some of its files could not be deleted: {}.",
                    component.title,
                    errno_text(*errno)
                ),
                FILES_LEFT,
            );
        }
        Failure::Errno(errno) => errno_text(*errno),
        Failure::NotAllowed => {
            return (
                String::from("Not allowed: only root installs or removes components."),
                "",
            );
        }
        _ => String::from("it could not be moved away"),
    };
    (
        format!("Could not remove {}: {reason}.", component.title),
        STAYS_INSTALLED,
    )
}

/// `Could not remove Windows apps: <reason>. It stays installed.`.
pub fn remove_failure_line(failure: &Failure, component: &Component) -> String {
    match remove_failure(failure, component) {
        (first, "") => first,
        (first, second) => format!("{first} {second}"),
    }
}

/// What removing the component the desktop runs from says while the desktop runs: from its
/// own window, the first line and the second; from a console, one line.
pub fn desktop_in_use(from_its_window: bool) -> (String, &'static str) {
    match from_its_window {
        true => (
            String::from("In use: the desktop is running, and this window is part of it."),
            BOOT_TO_REMOVE,
        ),
        false => (
            format!("In use: the desktop is running. {BOOT_TO_REMOVE}"),
            "",
        ),
    }
}

/// How the component the desktop runs from is removed.
pub const BOOT_TO_REMOVE: &str = "Boot with run-eggos.sh --serial and remove it from there.";

/// The Components list's three lines while no store is attached.
pub const NO_STORE_LINES: [&str; 3] = [
    "No component store is attached.",
    "Attach the store disk, or put the .img in the host share.",
    "run-eggos.sh and run-eggos.ps1 attach it automatically.",
];

/// `Component store: EggOS 0.3.0, x86_64`.
pub fn store_title(store: &Store) -> String {
    format!(
        "Component store: EggOS {}, {}",
        store.index.version, store.index.arch
    )
}

/// `eggos-0.3.0-x86_64-components.img on /dev/vdc`, or `... in the host share`.
pub fn store_place(store: &Store) -> String {
    format!("{} {}", store.file_name(), where_text(store))
}

/// `2 of 4 installed · 3.7 MB on disk`, or `Nothing installed · 0 B on disk`.
pub fn count_line(installed: usize, shown: usize, installed_bytes: u64) -> String {
    let size = size_text(installed_bytes);
    match installed {
        0 => format!("Nothing installed \u{b7} {size} on disk"),
        _ => format!("{installed} of {shown} installed \u{b7} {size} on disk"),
    }
}

/// `Installed · 1.8 MB`.
pub fn installed_state(size: u64) -> String {
    format!("Installed \u{b7} {}", size_text(size))
}

/// `Not installed · 1.8 MB`.
pub fn not_installed_state(size: u64) -> String {
    format!("Not installed \u{b7} {}", size_text(size))
}

/// `Installing 42 %`, from the bytes copied of the total, floored.
pub fn installing_state(received: u64, total: u64) -> String {
    format!("Installing {} %", percent(received, total))
}

/// The percent copied, floored: `received x 100 / total`.
pub fn percent(received: u64, total: u64) -> u64 {
    received.saturating_mul(100) / total.max(1)
}

/// `0 B of 1.8 MB`.
pub fn copied_of(received: u64, total: u64) -> String {
    format!("{} of {}", size_text(received), size_text(total))
}

/// The state of a component being removed.
pub const REMOVING_STATE: &str = "Removing...";
/// The state of a component whose last install failed.
pub const FAILED_STATE: &str = "Failed";
/// The state of a component another program is installing.
pub const INSTALLING_ELSEWHERE_STATE: &str = "Installing...";

/// Why a row's button is disabled, its tooltip (docs/ux/components-v0.3.md section 6): another
/// component is installing.
pub const OTHER_INSTALLING_REASON: &str = "Another component is installing";
/// Another component is being removed.
pub const OTHER_REMOVING_REASON: &str = "Another component is being removed";
/// Another component is updating.
pub const OTHER_UPDATING_REASON: &str = "Another component is updating";
/// This component is being removed.
pub const BEING_REMOVED_REASON: &str = "This component is being removed";
/// Another program is installing this component.
pub const INSTALLING_REASON: &str = "This component is installing";
/// This component is updating.
pub const UPDATING_REASON: &str = "This component is updating";
/// Its install is being cancelled.
pub const CANCELLING_REASON: &str = "Cancelling";
/// There is no store to install from.
pub const NO_STORE_REASON: &str = "Download the component store first";

/// The Updates band's line before any check (docs/ux/components-v0.5.md section 2).
pub const NOT_CHECKED: &str = "Not checked yet";
/// The Updates band's line while a check runs.
pub const CHECKING: &str = "Checking for updates...";
/// The Updates band's line with nothing installed.
pub const NOTHING_TO_UPDATE: &str = "Nothing installed, so nothing to update";
/// The first line of a failed check's note.
pub const CHECK_FAILED: &str = "Could not check for updates.";
/// The second line of a check that got no answer.
pub const RELEASE_SILENT: &str = "The release did not answer.";
/// The second line of an update that failed or was cancelled.
pub const INSTALLED_COPY_STAYS: &str = "The installed copy stays.";
/// The note under a row updated while programs use it.
pub const UPDATED_RESTART: &str = "Updated. Running programs use it after a restart.";
/// The first line of a failed store download.
pub const STORE_DOWNLOAD_FAILED: &str = "Could not download the component store.";
/// The second line of a failed or cancelled store download.
pub const NOTHING_DOWNLOADED: &str = "Nothing was downloaded.";
/// The no-store lines with the download offered (section 5): network up, then down.
pub const DOWNLOAD_STORE_LINES: [[&str; 3]; 2] = [
    [
        "No component store on this computer.",
        "Download it to see what you can install.",
        "From the EggOS release.",
    ],
    [
        "No component store on this computer.",
        "Connect, or attach the store disk.",
        "run-eggos.sh attaches it automatically.",
    ],
];

const DAY_S: u64 = 86_400;
const HOUR_S: u64 = 3_600;
const MINUTE_S: u64 = 60;

/// When a check ran, at `at` seconds, seen at `now` seconds (local time, or `None` with no
/// wall clock): `today at 09:14`, `yesterday at 09:14`, `3 days ago`, `since this start`.
pub fn checked_when(at: u64, now: Option<u64>) -> String {
    let Some(now) = now.filter(|now| *now >= at && at > 0) else {
        return String::from("since this start");
    };
    let clock = format!("{:02}:{:02}", at % DAY_S / HOUR_S, at % HOUR_S / MINUTE_S);
    match now / DAY_S - at / DAY_S {
        0 => format!("today at {clock}"),
        1 => format!("yesterday at {clock}"),
        days => format!("{days} days ago"),
    }
}

/// `Up to date · checked today at 09:14`.
pub fn up_to_date(when: &str) -> String {
    format!("Up to date \u{b7} checked {when}")
}

/// `2 updates · 0.7 MB to download`, `1 update · 0.4 MB to copy` from a store on a disk.
pub fn updates_line(count: usize, bytes: u64, from_disk: bool) -> String {
    let noun = if count == 1 { "update" } else { "updates" };
    format!(
        "{count} {noun} \u{b7} {} {}",
        size_text(bytes),
        to_fetch(from_disk)
    )
}

/// `Update: 0.4 MB to download`, a row's line while it has an update.
pub fn update_row_line(bytes: u64, from_disk: bool) -> String {
    format!("Update: {} {}", size_text(bytes), to_fetch(from_disk))
}

fn to_fetch(from_disk: bool) -> &'static str {
    match from_disk {
        true => "to copy",
        false => "to download",
    }
}

/// `Updating 1 of 2: Windows apps`.
pub fn updating_all(at: usize, of: usize, title: &str) -> String {
    format!("Updating {at} of {of}: {title}")
}

/// `Updating 42 %`.
pub fn updating_state(percent: u64) -> String {
    format!("Updating {percent} %")
}

/// A running job's phase and bytes: `Downloading: 0.2 MB of 0.4 MB`, `Copying: 1.1 MB of 1.8
/// MB`, `Swapping in...`, `Finishing...`.
pub fn job_phase_line(phase: &str, done: u64, total: u64) -> String {
    match phase {
        "downloading" => format!("Downloading: {}", copied_of(done, total)),
        "swapping" => String::from("Swapping in..."),
        "finishing" => String::from("Finishing..."),
        _ => format!("Copying: {}", copied_of(done, total)),
    }
}

/// `Downloading the component store: 12.4 MB of 31.0 MB`, or `... 12.4 MB` with no length.
pub fn downloading_store(done: u64, total: u64) -> String {
    match total {
        0 => format!("Downloading the component store: {}", size_text(done)),
        _ => format!(
            "Downloading the component store: {}",
            copied_of(done, total)
        ),
    }
}

/// `Could not update Windows apps.`
pub fn update_failed(title: &str) -> String {
    format!("Could not update {title}.")
}

/// A failed update's two lines (docs/ux/components-v0.5.md section 3): what went wrong, then
/// what became of the installed copy; the second is empty when there is nothing to add.
pub fn update_failure(failure: &Failure, title: &str) -> (String, &'static str) {
    match failure {
        Failure::Damaged(_) => (
            format!("The downloaded update of {title} is damaged."),
            INSTALLED_COPY_STAYS,
        ),
        Failure::Detached => (
            String::from("The store was detached before the copy finished."),
            INSTALLED_COPY_STAYS,
        ),
        Failure::NoSpace { needed, free } => (
            match needed {
                0 => String::from("Not enough space."),
                _ => format!(
                    "Not enough space: {} needed, {} free.",
                    size_text(*needed),
                    size_text(*free)
                ),
            },
            INSTALLED_COPY_STAYS,
        ),
        Failure::Cancelled => (String::from(CANCELLED), INSTALLED_COPY_STAYS),
        Failure::NotAllowed => (
            String::from("Not allowed: only root installs or removes components."),
            "",
        ),
        Failure::UnsafeFolder => (
            format!(
                "Could not update {title}: {} is not a real folder.",
                crate::manager::COMPONENTS
            ),
            INSTALLED_COPY_STAYS,
        ),
        Failure::Errno(errno) | Failure::FilesLeft(errno) => (
            format!("Could not update {title}: {}.", errno_text(*errno)),
            INSTALLED_COPY_STAYS,
        ),
    }
}

/// The bar's chip tooltip: `2 component updates, 0.7 MB. Click to open Components.`
pub fn updates_tooltip(count: usize, bytes: u64) -> String {
    let noun = if count == 1 { "update" } else { "updates" };
    format!(
        "{count} component {noun}, {}. Click to open Components.",
        size_text(bytes)
    )
}

fn errno_text(errno: eggos_runtime::errno::Errno) -> String {
    String::from_utf8_lossy(errno.message()).into_owned()
}
