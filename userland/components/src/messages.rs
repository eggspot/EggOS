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
        1 => format!("In use: {} is running. Quit it, then remove.", joined(names)),
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
        Failure::Errno(errno) => (
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

/// `Could not remove Windows apps: <reason>. It stays installed.`.
pub fn remove_failure_line(failure: &Failure, component: &Component) -> String {
    let reason = match failure {
        Failure::Errno(errno) => errno_text(*errno),
        Failure::NotAllowed => {
            return String::from("Not allowed: only root installs or removes components.");
        }
        _ => String::from("it could not be moved away"),
    };
    format!(
        "Could not remove {}: {reason}. {STAYS_INSTALLED}",
        component.title
    )
}

fn errno_text(errno: eggos_runtime::errno::Errno) -> String {
    String::from_utf8_lossy(errno.message()).into_owned()
}
