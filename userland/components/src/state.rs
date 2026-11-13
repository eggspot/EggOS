//! What the component manager keeps between runs for its front-ends (D299,
//! docs/ux/components-v0.5.md section 12): the last update check's result, whether the daily
//! check is on, the running job's progress, and a request to cancel it. Each is a small text
//! file, written whole and read whole, so the pane, the command and the MCP tools read the same.

use crate::manager::UpdatePlan;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_app::files::Files;

/// The last check's result.
pub const UPDATES_PATH: &str = "/var/lib/eggos/updates";
/// `off` when the daily check is off; on otherwise.
pub const DAILY_PATH: &str = "/var/lib/eggos/updates-daily";
/// The running job's progress.
pub const JOB_PATH: &str = "/run/eggos/component-job";
/// Present while a front-end asks the running job to stop.
pub const CANCEL_PATH: &str = "/run/eggos/component-cancel";
/// Why the last update job failed, for the front-ends that see only its exit status (D421).
pub const FAILURE_PATH: &str = "/run/eggos/component-failure";
const LARGEST: usize = 8192;

/// One component the check found an update for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedItem {
    pub name: String,
    pub fetch_bytes: u64,
    pub changed_bytes: u64,
    pub changed_files: usize,
}

/// A check's result: when, from where (`release` or `disk`), and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub at: u64,
    pub source: String,
    pub items: Vec<CheckedItem>,
}

impl Checked {
    /// The result of the plans a dry run made at `at` from `source`.
    pub fn of(plans: &[UpdatePlan<'_>], at: u64, source: &str) -> Self {
        Self {
            at,
            source: source.into(),
            items: plans
                .iter()
                .map(|plan| CheckedItem {
                    name: plan.component.name.clone(),
                    fetch_bytes: plan.changed_bytes(),
                    changed_bytes: plan.changed_bytes(),
                    changed_files: plan.changed.len(),
                })
                .collect(),
        }
    }

    /// The bytes every update would fetch.
    pub fn fetch_bytes(&self) -> u64 {
        self.items.iter().map(|item| item.fetch_bytes).sum()
    }

    /// The item for component `name`.
    pub fn item(&self, name: &str) -> Option<&CheckedItem> {
        self.items.iter().find(|item| item.name == name)
    }

    fn text(&self) -> String {
        let mut text = format!("checked {} {}\n", self.at, self.source);
        for item in &self.items {
            text.push_str(&format!(
                "update {} {} {} {}\n",
                item.name, item.fetch_bytes, item.changed_bytes, item.changed_files
            ));
        }
        text
    }

    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let mut first = lines.next()?.split(' ');
        if first.next()? != "checked" {
            return None;
        }
        let at = first.next()?.parse().ok()?;
        let source = String::from(first.next()?);
        let items = lines
            .filter_map(|line| {
                let mut words = line.split(' ');
                (words.next()? == "update").then_some(())?;
                Some(CheckedItem {
                    name: words.next()?.into(),
                    fetch_bytes: words.next()?.parse().ok()?,
                    changed_bytes: words.next()?.parse().ok()?,
                    changed_files: words.next()?.parse().ok()?,
                })
            })
            .collect();
        Some(Self { at, source, items })
    }
}

/// A job's progress: what it does, to what, its phase and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub pid: u32,
    /// `installing`, `updating`, `removing` or `downloading`.
    pub kind: String,
    pub name: String,
    /// `downloading`, `copying`, `finishing` or `swapping`.
    pub phase: String,
    pub done: u64,
    pub total: u64,
}

impl Job {
    /// Its whole percent.
    pub fn percent(&self) -> u64 {
        self.done.saturating_mul(100) / self.total.max(1)
    }
}

fn read_text<F: Files>(files: &mut F, path: &str) -> Option<String> {
    let handle = files.open(path.as_bytes()).ok()?;
    let mut bytes = alloc::vec![0u8; LARGEST];
    let mut length = 0;
    while length < bytes.len() {
        match files.read(handle, &mut bytes[length..]) {
            Ok(0) | Err(_) => break,
            Ok(count) => length += count,
        }
    }
    files.close(handle);
    bytes.truncate(length);
    String::from_utf8(bytes).ok()
}

fn write_text<F: Files>(files: &mut F, path: &str, text: &str) -> bool {
    let staged = format!("{path}.new");
    let _ = files.remove_file(staged.as_bytes());
    let Ok(handle) = files.create(staged.as_bytes(), 0o644) else {
        return false;
    };
    let written = files
        .write(handle, text.as_bytes())
        .and_then(|()| files.sync_file(handle));
    files.close(handle);
    written.is_ok() && files.rename(staged.as_bytes(), path.as_bytes()).is_ok()
}

/// Keeps `checked` as the last check's result.
pub fn store_check<F: Files>(files: &mut F, checked: &Checked) -> bool {
    write_text(files, UPDATES_PATH, &checked.text())
}

/// The last check's result, if one was kept.
pub fn last_check<F: Files>(files: &mut F) -> Option<Checked> {
    Checked::parse(&read_text(files, UPDATES_PATH)?)
}

/// Whether the daily check is on: unless it was turned off.
pub fn daily<F: Files>(files: &mut F) -> bool {
    read_text(files, DAILY_PATH).is_none_or(|text| text.trim() != "off")
}

/// Turns the daily check on or off.
pub fn set_daily<F: Files>(files: &mut F, on: bool) -> bool {
    write_text(files, DAILY_PATH, if on { "on\n" } else { "off\n" })
}

/// Records `job`'s progress.
pub fn record_job<F: Files>(files: &mut F, job: &Job) {
    let text = format!(
        "{} {} {} {} {} {}\n",
        job.pid, job.kind, job.name, job.phase, job.done, job.total
    );
    let _ = write_text(files, JOB_PATH, &text);
}

/// The running job's progress, if one is recorded.
pub fn job<F: Files>(files: &mut F) -> Option<Job> {
    let text = read_text(files, JOB_PATH)?;
    let mut words = text.split_whitespace();
    Some(Job {
        pid: words.next()?.parse().ok()?,
        kind: words.next()?.into(),
        name: words.next()?.into(),
        phase: words.next()?.into(),
        done: words.next()?.parse().ok()?,
        total: words.next()?.parse().ok()?,
    })
}

/// Forgets the job and any request to cancel it.
pub fn end_job<F: Files>(files: &mut F) {
    let _ = files.remove_file(JOB_PATH.as_bytes());
    let _ = files.remove_file(CANCEL_PATH.as_bytes());
}

/// Records that updating component `name` failed with `failure`.
pub fn record_failure<F: Files>(files: &mut F, name: &str, failure: &crate::manager::Failure) {
    let _ = write_text(
        files,
        FAILURE_PATH,
        &format!("{name} {}\n", failure_text(failure)),
    );
}

/// Forgets the last failure, as a new job starts.
pub fn clear_failure<F: Files>(files: &mut F) {
    let _ = files.remove_file(FAILURE_PATH.as_bytes());
}

/// The component the last update job failed on and why, if one is recorded.
pub fn last_failure<F: Files>(files: &mut F) -> Option<(String, crate::manager::Failure)> {
    let text = read_text(files, FAILURE_PATH)?;
    let (name, rest) = text.trim_end().split_once(' ')?;
    Some((String::from(name), parse_failure(rest)?))
}

/// `failure` as one line of words: its kind, then its values.
pub fn failure_text(failure: &crate::manager::Failure) -> String {
    use crate::manager::Failure;
    match failure {
        Failure::Damaged(file) => format!("damaged {file}"),
        Failure::Detached => String::from("detached"),
        Failure::NoSpace { needed, free } => format!("no_space {needed} {free}"),
        Failure::Cancelled => String::from("cancelled"),
        Failure::NotAllowed => String::from("not_allowed"),
        Failure::UnsafeFolder => String::from("unsafe_folder"),
        Failure::FilesLeft(errno) => format!("files_left {}", errno.0),
        Failure::Errno(errno) => format!("errno {}", errno.0),
    }
}

/// The failure [`failure_text`] wrote.
pub fn parse_failure(text: &str) -> Option<crate::manager::Failure> {
    use crate::manager::Failure;
    use eggos_runtime::errno::Errno;
    let (kind, rest) = text.split_once(' ').unwrap_or((text, ""));
    let number = |word: Option<&str>| word?.parse::<u64>().ok();
    let mut words = rest.split_whitespace();
    Some(match kind {
        "damaged" => Failure::Damaged(String::from(rest)),
        "detached" => Failure::Detached,
        "no_space" => Failure::NoSpace {
            needed: number(words.next())?,
            free: number(words.next())?,
        },
        "cancelled" => Failure::Cancelled,
        "not_allowed" => Failure::NotAllowed,
        "unsafe_folder" => Failure::UnsafeFolder,
        "files_left" => Failure::FilesLeft(Errno(u16::try_from(number(words.next())?).ok()?)),
        "errno" => Failure::Errno(Errno(u16::try_from(number(words.next())?).ok()?)),
        _ => return None,
    })
}

/// Asks the running job to stop at its next step.
pub fn request_cancel<F: Files>(files: &mut F) -> bool {
    write_text(files, CANCEL_PATH, "cancel\n")
}

/// Whether a front-end asked the running job to stop.
pub fn cancel_requested<F: Files>(files: &mut F) -> bool {
    files.status(CANCEL_PATH.as_bytes()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_reads_back_as_written() {
        let checked = Checked {
            at: 1_793_282_709,
            source: "release".into(),
            items: alloc::vec![CheckedItem {
                name: "windows-apps".into(),
                fetch_bytes: 400_000,
                changed_bytes: 400_000,
                changed_files: 3,
            }],
        };
        assert_eq!(Checked::parse(&checked.text()), Some(checked.clone()));
        assert_eq!(checked.fetch_bytes(), 400_000);
    }

    #[test]
    fn every_failure_reads_back_as_written() {
        use crate::manager::Failure;
        use eggos_runtime::errno::Errno;
        for failure in [
            Failure::Damaged("windows-apps.component payload".into()),
            Failure::Detached,
            Failure::NoSpace {
                needed: 2_306_867,
                free: 629_145,
            },
            Failure::Cancelled,
            Failure::NotAllowed,
            Failure::UnsafeFolder,
            Failure::FilesLeft(Errno(16)),
            Failure::Errno(Errno(5)),
        ] {
            assert_eq!(parse_failure(&failure_text(&failure)), Some(failure));
        }
        assert_eq!(parse_failure("no_space 12"), None);
        assert_eq!(parse_failure("melted"), None);
    }
}
