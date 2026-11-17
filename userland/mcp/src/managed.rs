//! What the policy file allows agents (D656; editions-policy v0.1 sections 4, 7 and 8): the
//! classes any agent may be granted (`agents.grants`), the tools no agent may call
//! (`agents.deny`) and who manages the computer, read again before every call. A file that
//! cannot be trusted holds every agent to `view`; without the `management` component, or
//! without a file, nothing is limited.

use crate::grants::Grant;
use eggos_settings_file::policy;

pub const MOST_DENIED: usize = 32;
pub const TOOL_NAME_BYTES: usize = 48;
pub const COMPONENT_RECORD: &str = "/var/lib/eggos/components/management/component";

/// What the machine found of the policy file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyRead {
    /// No `management` component, or no file: nothing is limited.
    NotManaged,
    Untrusted,
    /// A trusted file of this many bytes, read into the buffer given.
    Trusted(usize),
}

/// The classes an agent may be granted at most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ceiling {
    view: bool,
    write: bool,
    install: bool,
    run: bool,
}

impl Ceiling {
    pub const EVERY: Ceiling = Ceiling {
        view: true,
        write: true,
        install: true,
        run: true,
    };
    pub const VIEW_ONLY: Ceiling = Ceiling {
        view: true,
        write: false,
        install: false,
        run: false,
    };
    pub const READING_ONLY: Ceiling = Ceiling {
        view: false,
        write: false,
        install: false,
        run: false,
    };

    /// The ceiling `agents.grants` names, its unknown words left out.
    pub fn of_classes(classes: &str) -> Self {
        let mut ceiling = Self::READING_ONLY;
        for grant in classes
            .split_ascii_whitespace()
            .filter_map(|word| Grant::named(word.as_bytes()))
        {
            match grant {
                Grant::View => ceiling.view = true,
                Grant::Write => ceiling.write = true,
                Grant::Install => ceiling.install = true,
                Grant::Run => ceiling.run = true,
            }
        }
        ceiling
    }

    /// Whether an agent may hold `grant`.
    pub fn allows(self, grant: Grant) -> bool {
        match grant {
            Grant::View => self.view,
            Grant::Write => self.write,
            Grant::Install => self.install,
            Grant::Run => self.run,
        }
    }
}

impl Default for Ceiling {
    fn default() -> Self {
        Self::EVERY
    }
}

/// The policy's limits on agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub ceiling: Ceiling,
    denied: [[u8; TOOL_NAME_BYTES]; MOST_DENIED],
    denied_lengths: [u8; MOST_DENIED],
    denied_count: usize,
    manager: [u8; policy::MANAGER_BYTES],
    manager_length: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            ceiling: Ceiling::EVERY,
            denied: [[0; TOOL_NAME_BYTES]; MOST_DENIED],
            denied_lengths: [0; MOST_DENIED],
            denied_count: 0,
            manager: [0; policy::MANAGER_BYTES],
            manager_length: 0,
        }
    }
}

impl Limits {
    /// The limits of what the machine read; `text` holds a trusted file's bytes.
    pub fn of(read: PolicyRead, text: &[u8]) -> Self {
        let mut limits = Self::default();
        let length = match read {
            PolicyRead::NotManaged => return limits,
            PolicyRead::Untrusted => {
                limits.ceiling = Ceiling::VIEW_ONLY;
                return limits;
            }
            PolicyRead::Trusted(length) => length,
        };
        let Ok(text) = core::str::from_utf8(text.get(..length).unwrap_or_default()) else {
            limits.ceiling = Ceiling::VIEW_ONLY;
            return limits;
        };
        if let Some(classes) = policy::agent_ceiling(text) {
            limits.ceiling = Ceiling::of_classes(classes);
        }
        for tool in policy::denied_tools(text).take(MOST_DENIED) {
            if tool.len() > TOOL_NAME_BYTES {
                continue;
            }
            limits.denied[limits.denied_count][..tool.len()].copy_from_slice(tool.as_bytes());
            limits.denied_lengths[limits.denied_count] = tool.len() as u8;
            limits.denied_count += 1;
        }
        if let Some(manager) = policy::manager(text) {
            limits.manager[..manager.len()].copy_from_slice(manager.as_bytes());
            limits.manager_length = manager.len();
        }
        limits
    }

    /// Whether no agent may call the tool `name`.
    pub fn denies(&self, name: &[u8]) -> bool {
        (0..self.denied_count)
            .any(|at| &self.denied[at][..usize::from(self.denied_lengths[at])] == name)
    }

    /// Who manages the computer: `policy.manager`, else `your administrator`.
    pub fn manager(&self) -> &str {
        match self.manager_length {
            0 => policy::NO_MANAGER,
            length => core::str::from_utf8(&self.manager[..length]).unwrap_or(policy::NO_MANAGER),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trusted_policy_caps_the_grants_and_denies_tools_by_name() {
        let text = b"policy.manager=Contoso IT\nagents.grants=view write\nagents.deny=disk_format disk_install\n";
        let limits = Limits::of(PolicyRead::Trusted(text.len()), text);
        assert!(limits.ceiling.allows(Grant::Write));
        assert!(!limits.ceiling.allows(Grant::Run));
        assert!(limits.denies(b"disk_install"));
        assert!(!limits.denies(b"disk_list"));
        assert_eq!(limits.manager(), "Contoso IT");
    }

    #[test]
    fn an_untrusted_policy_holds_agents_to_view_and_none_limits_nothing() {
        let untrusted = Limits::of(PolicyRead::Untrusted, b"");
        assert!(untrusted.ceiling.allows(Grant::View));
        assert!(!untrusted.ceiling.allows(Grant::Write));
        assert_eq!(untrusted.manager(), "your administrator");
        let free = Limits::of(PolicyRead::NotManaged, b"");
        assert!(free.ceiling.allows(Grant::Run));
        let empty = b"agents.grants=\n";
        assert!(!Limits::of(PolicyRead::Trusted(empty.len()), empty).ceiling.allows(Grant::View));
    }
}
