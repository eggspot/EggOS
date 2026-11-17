//! The policy file (D654; editions-policy v0.1 section 4): `/etc/eggos/policy`, one
//! `key=value` a line, written by an administrator or a management tool. Each setting line
//! gives a setting its value and locks it; `policy.manager` and `policy.contact` say who
//! manages the computer; `agents.grants` caps what any agent may be granted and `agents.deny`
//! names tools no agent may call. Whether the file can be trusted is decided from its owner,
//! mode, folder, size and text. Which keys and values a setting takes is the Settings
//! program's to check; this module reads the lines, without allocating, so the MCP server and
//! Settings share it.

pub const PATH: &str = "/etc/eggos/policy";
pub const FOLDER: &str = "/etc/eggos";
pub const LARGEST: usize = 16 * 1024;
pub const MOST_LINES: usize = 256;
pub const MANAGER_BYTES: usize = 32;
pub const CONTACT_BYTES: usize = 64;
pub const MANAGER_KEY: &str = "policy.manager";
pub const CONTACT_KEY: &str = "policy.contact";
pub const AGENT_GRANTS_KEY: &str = "agents.grants";
pub const AGENTS_DENY_KEY: &str = "agents.deny";
pub const NO_MANAGER: &str = "your administrator";

const ROOT: u32 = 0;
const GROUP_OR_OTHERS_WRITE: u32 = 0o022;

/// What the file system says of the policy file or its folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ownership {
    /// A regular file for the policy file, a directory for its folder.
    pub right_kind: bool,
    pub owner: u32,
    pub mode: u32,
}

/// Why the policy file cannot be trusted: then it locks nothing and holds every agent to
/// reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distrust {
    NotRegular,
    NotOwnedByRoot,
    OthersCanWrite,
    FolderNotTrusted,
    TooLarge,
    TooManyLines,
    NotUtf8,
}

impl Distrust {
    /// The Management pane's sentence for it.
    pub fn sentence(self) -> &'static str {
        match self {
            Self::NotRegular => "It is not a regular file.",
            Self::NotOwnedByRoot => "It is not owned by root.",
            Self::OthersCanWrite => "Others can write it.",
            Self::FolderNotTrusted => "Its folder is not root's alone.",
            Self::TooLarge => "It is larger than 16 KB.",
            Self::TooManyLines => "It has more than 256 lines.",
            Self::NotUtf8 => "It is not UTF-8 text.",
        }
    }
}

fn root_alone(ownership: Ownership) -> bool {
    ownership.right_kind && ownership.owner == ROOT && ownership.mode & GROUP_OR_OTHERS_WRITE == 0
}

/// Why a policy file with these facts and this text cannot be trusted; `None` when it can.
pub fn distrust(file: Ownership, folder: Ownership, text: &[u8]) -> Option<Distrust> {
    if !file.right_kind {
        return Some(Distrust::NotRegular);
    }
    if file.owner != ROOT {
        return Some(Distrust::NotOwnedByRoot);
    }
    if file.mode & GROUP_OR_OTHERS_WRITE != 0 {
        return Some(Distrust::OthersCanWrite);
    }
    if !root_alone(folder) {
        return Some(Distrust::FolderNotTrusted);
    }
    if text.len() > LARGEST {
        return Some(Distrust::TooLarge);
    }
    let Ok(text) = core::str::from_utf8(text) else {
        return Some(Distrust::NotUtf8);
    };
    (text.lines().count() > MOST_LINES).then_some(Distrust::TooManyLines)
}

/// What one line of the file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
    /// An empty line, or the administrator's note starting with `#`.
    Note,
    Manager(&'a str),
    Contact(&'a str),
    AgentGrants(&'a str),
    AgentsDeny(&'a str),
    /// A setting's value, which the line locks.
    Setting { key: &'a str, value: &'a str },
}

/// Why a line is ignored, before its setting is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineFault {
    NotKeyValue,
    ManagerTooLong,
    ContactTooLong,
}

impl LineFault {
    /// The reason `settings policy` prints for it.
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotKeyValue => "not key=value",
            Self::ManagerTooLong => "longer than 32 bytes",
            Self::ContactTooLong => "longer than 64 bytes",
        }
    }
}

/// What `line` says.
pub fn line_of(line: &str) -> Result<Line<'_>, LineFault> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(Line::Note);
    }
    let (key, value) = line.split_once('=').ok_or(LineFault::NotKeyValue)?;
    let (key, value) = (key.trim(), value.trim());
    if key.is_empty() {
        return Err(LineFault::NotKeyValue);
    }
    Ok(match key {
        MANAGER_KEY if value.len() > MANAGER_BYTES => return Err(LineFault::ManagerTooLong),
        MANAGER_KEY => Line::Manager(value),
        CONTACT_KEY if value.len() > CONTACT_BYTES => return Err(LineFault::ContactTooLong),
        CONTACT_KEY => Line::Contact(value),
        AGENT_GRANTS_KEY => Line::AgentGrants(value),
        AGENTS_DENY_KEY => Line::AgentsDeny(value),
        _ => Line::Setting { key, value },
    })
}

/// Every line with its number from 1, its text and what it says.
pub fn lines(text: &str) -> impl Iterator<Item = (usize, &str, Result<Line<'_>, LineFault>)> {
    text.lines()
        .take(MOST_LINES)
        .enumerate()
        .map(|(at, line)| (at + 1, line, line_of(line)))
}

fn key_of<'a>(line: &Line<'a>) -> Option<&'a str> {
    match *line {
        Line::Note => None,
        Line::Manager(_) => Some(MANAGER_KEY),
        Line::Contact(_) => Some(CONTACT_KEY),
        Line::AgentGrants(_) => Some(AGENT_GRANTS_KEY),
        Line::AgentsDeny(_) => Some(AGENTS_DENY_KEY),
        Line::Setting { key, .. } => Some(key),
    }
}

/// The number of the last good line that gives `key`, which is the one that counts.
pub fn last_line_for(text: &str, key: &str) -> Option<usize> {
    lines(text)
        .filter(|(_, _, said)| said.as_ref().is_ok_and(|line| key_of(line) == Some(key)))
        .map(|(number, _, _)| number)
        .last()
}

/// The later line that gives line `number`'s key again, so line `number` is ignored.
pub fn given_again(text: &str, number: usize) -> Option<usize> {
    let (_, _, said) = lines(text).find(|(at, _, _)| *at == number)?;
    let key = key_of(said.as_ref().ok()?)?;
    last_line_for(text, key).filter(|last| *last != number)
}

fn last_value<'a>(text: &'a str, wanted: impl Fn(&Line<'a>) -> Option<&'a str>) -> Option<&'a str> {
    lines(text)
        .filter_map(|(_, _, said)| said.ok())
        .filter_map(|line| wanted(&line))
        .last()
}

/// Who manages the computer, by `policy.manager`; `None` without it.
pub fn manager(text: &str) -> Option<&str> {
    last_value(text, |line| match *line {
        Line::Manager(name) => Some(name),
        _ => None,
    })
}

/// Where to ask, by `policy.contact`.
pub fn contact(text: &str) -> Option<&str> {
    last_value(text, |line| match *line {
        Line::Contact(contact) => Some(contact),
        _ => None,
    })
}

/// The value the file locks `key` to, its last line for the key.
pub fn locked_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    last_value(text, |line| match *line {
        Line::Setting { key: own, value } if own == key => Some(value),
        _ => None,
    })
}

/// The classes `agents.grants` allows, space-separated as written; `None` without the line.
pub fn agent_ceiling(text: &str) -> Option<&str> {
    last_value(text, |line| match *line {
        Line::AgentGrants(classes) => Some(classes),
        _ => None,
    })
}

/// The tools `agents.deny` names, from its last line.
pub fn denied_tools(text: &str) -> impl Iterator<Item = &str> {
    let named = last_value(text, |line| match *line {
        Line::AgentsDeny(tools) => Some(tools),
        _ => None,
    });
    named.unwrap_or("").split_ascii_whitespace()
}

/// Whether no agent may call `tool`.
pub fn denies(text: &str, tool: &str) -> bool {
    denied_tools(text).any(|denied| denied == tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTOSO: &str = "policy.manager=Contoso IT\npolicy.contact=helpdesk@contoso.example\n\
                           update.policy=managed\nappearance.theme=dark\n# a note\n\n\
                           agents.grants=view write\nagents.deny=disk_format disk_install\n\
                           appearance.theme=light\n";

    fn trusted() -> Ownership {
        Ownership {
            right_kind: true,
            owner: 0,
            mode: 0o644,
        }
    }

    #[test]
    fn a_policy_names_its_manager_its_locks_and_the_agents_ceiling() {
        assert_eq!(manager(CONTOSO), Some("Contoso IT"));
        assert_eq!(contact(CONTOSO), Some("helpdesk@contoso.example"));
        assert_eq!(locked_value(CONTOSO, "update.policy"), Some("managed"));
        assert_eq!(locked_value(CONTOSO, "appearance.theme"), Some("light"));
        assert_eq!(locked_value(CONTOSO, "time.automatic"), None);
        assert_eq!(given_again(CONTOSO, 4), Some(9));
        assert_eq!(given_again(CONTOSO, 9), None);
        assert_eq!(agent_ceiling(CONTOSO), Some("view write"));
        assert!(denies(CONTOSO, "disk_install"));
        assert!(!denies(CONTOSO, "component_remove"));
        assert_eq!(manager("update.policy=notify"), None);
    }

    #[test]
    fn a_wrong_line_is_ignored_with_its_reason() {
        let text = "just words\npolicy.manager=A name much longer than thirty-two bytes\n=x\n";
        let faults: Vec<_> = lines(text).map(|(number, _, said)| (number, said.err())).collect();
        assert_eq!(
            faults,
            [
                (1, Some(LineFault::NotKeyValue)),
                (2, Some(LineFault::ManagerTooLong)),
                (3, Some(LineFault::NotKeyValue)),
            ]
        );
        assert_eq!(manager(text), None);
        assert_eq!(LineFault::ContactTooLong.reason(), "longer than 64 bytes");
    }

    #[test]
    fn only_a_file_root_alone_can_write_is_trusted() {
        let folder = Ownership {
            mode: 0o755,
            ..trusted()
        };
        assert_eq!(distrust(trusted(), folder, CONTOSO.as_bytes()), None);
        let writable = Ownership {
            mode: 0o666,
            ..trusted()
        };
        assert_eq!(distrust(writable, folder, b""), Some(Distrust::OthersCanWrite));
        let theirs = Ownership {
            owner: 1000,
            ..trusted()
        };
        assert_eq!(distrust(theirs, folder, b""), Some(Distrust::NotOwnedByRoot));
        assert_eq!(distrust(trusted(), theirs, b""), Some(Distrust::FolderNotTrusted));
        let folder_as_file = Ownership {
            right_kind: false,
            ..trusted()
        };
        assert_eq!(distrust(folder_as_file, folder, b""), Some(Distrust::NotRegular));
        assert_eq!(distrust(trusted(), folder, &[0xff, 0xfe]), Some(Distrust::NotUtf8));
        assert_eq!(distrust(trusted(), folder, &[b'\n'; LARGEST + 1]), Some(Distrust::TooLarge));
        assert_eq!(distrust(trusted(), folder, &[b'\n'; 300]), Some(Distrust::TooManyLines));
        assert_eq!(Distrust::OthersCanWrite.sentence(), "Others can write it.");
    }
}
