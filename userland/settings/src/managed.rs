//! What the policy file locks, as Settings sees it (D655; editions-policy v0.1 sections 4
//! and 6): which setting lines hold a value the setting takes, why the other lines are
//! ignored, and `settings policy`'s text. The file's lines and trust are
//! `eggos_settings_file::policy`'s; the keys and values are [`crate::keys`]'.

use crate::facts::Facts;
use crate::keys::{self, Key, Refusal};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_mcp::grants::Grant;
use eggos_settings_file::policy::{self, Distrust, Line, LineFault};
use eggos_toolkit::text::Text;

/// The component whose install makes the policy file count.
pub const COMPONENT: &str = "management";

/// The policy file as the machine found it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PolicyFile {
    /// The `management` component is not installed, so no file is read.
    #[default]
    NotInstalled,
    Absent,
    Untrusted(Distrust),
    Trusted(String),
}

impl PolicyFile {
    /// The trusted file's text.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Trusted(text) => Some(text),
            _ => None,
        }
    }

    /// Who manages the computer: `policy.manager`, else `your administrator`.
    pub fn manager(&self) -> &str {
        self.text()
            .and_then(policy::manager)
            .unwrap_or(policy::NO_MANAGER)
    }

    /// The value the file locks `name` to, when its last line for the key holds a value the
    /// setting takes.
    pub fn locked(&self, name: &str) -> Option<&str> {
        let value = policy::locked_value(self.text()?, name)?;
        keys::change(name, value).is_ok().then_some(value)
    }

    /// Every locked setting and its value, by key.
    pub fn locks(&self) -> Vec<(&str, &str)> {
        let Some(text) = self.text() else {
            return Vec::new();
        };
        let mut locks: Vec<(&str, &str)> = policy::lines(text)
            .filter(|(number, _, _)| policy::given_again(text, *number).is_none())
            .filter_map(|(_, _, said)| match said {
                Ok(Line::Setting { key, value }) if keys::change(key, value).is_ok() => {
                    Some((key, value))
                }
                _ => None,
            })
            .collect();
        locks.sort_unstable();
        locks
    }
}

fn setting_refusal(key: &str, value: &str) -> Option<String> {
    match keys::change(key, value) {
        Ok(_) => None,
        Err(Refusal::NoSuchSetting) => Some(format!("{key}: no such setting")),
        Err(Refusal::ReadOnly) => Some(format!("{key}: read-only")),
        Err(Refusal::InvalidValue) => {
            let choices = Key::named(key).map_or_else(Text::new, Key::choices);
            Some(format!("{key}: invalid value '{value}' ({})", choices.as_str()))
        }
        Err(Refusal::InvalidLayouts(refusal)) => {
            let (quoted, reason) = keys::layouts_refusal(value, refusal);
            Some(format!("{key}: invalid value '{quoted}' ({})", reason.as_str()))
        }
    }
}

/// Why line `number` of `text`, which says `said`, is ignored; `None` when it counts.
pub fn ignored_reason(text: &str, number: usize, said: Result<Line<'_>, LineFault>) -> Option<String> {
    let line = match said {
        Err(fault) => return Some(String::from(fault.reason())),
        Ok(Line::Note) => return None,
        Ok(line) => line,
    };
    if let Some(later) = policy::given_again(text, number) {
        return Some(format!("given again on line {later}"));
    }
    match line {
        Line::Setting { key, value } => setting_refusal(key, value),
        Line::AgentGrants(classes) => classes
            .split_ascii_whitespace()
            .find(|class| Grant::named(class.as_bytes()).is_none())
            .map(|class| format!("{}: no class '{class}' (view, write, install or run)", policy::AGENT_GRANTS_KEY)),
        _ => None,
    }
}

fn lower_first(sentence: &str) -> String {
    let mut characters = sentence.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(characters).collect()
    })
}

fn agents_line(text: &str) -> Option<String> {
    let ceiling = policy::agent_ceiling(text);
    let denied: Vec<&str> = policy::denied_tools(text).collect();
    let mut parts = Vec::new();
    match ceiling {
        Some(classes) if classes.trim().is_empty() => parts.push(String::from("reading only")),
        Some(classes) => parts.push(format!("at most {}", classes.split_ascii_whitespace().collect::<Vec<_>>().join(" "))),
        None => {}
    }
    if !denied.is_empty() {
        parts.push(format!("denied {}", denied.join(" ")));
    }
    (!parts.is_empty()).then(|| format!("agents: {}", parts.join("; ")))
}

/// `settings policy`'s text for `file`, with `(pending)` after a locked value `facts` do not
/// have yet.
pub fn report(file: &PolicyFile, facts: &Facts) -> String {
    let text = match file {
        PolicyFile::NotInstalled => {
            return format!("Not managed: the {COMPONENT} component is not installed.\n");
        }
        PolicyFile::Absent => return format!("Not managed: no policy file at {}.\n", policy::PATH),
        PolicyFile::Untrusted(why) => {
            return format!(
                "The policy file cannot be trusted: {} Nothing is locked; agents can only read.\n",
                lower_first(why.sentence())
            );
        }
        PolicyFile::Trusted(text) => text,
    };
    let mut out = format!("Managed by {}", file.manager());
    if let Some(contact) = policy::contact(text) {
        out.push_str(&format!(" ({contact})"));
    }
    out.push('\n');
    let locks = file.locks();
    let noun = if locks.len() == 1 { "setting" } else { "settings" };
    out.push_str(&format!("{}: {} {noun} locked\n", policy::PATH, locks.len()));
    for (key, value) in &locks {
        let running = Key::named(key).map(|own| own.value(facts));
        let pending = running.is_some_and(|running| running.as_str() != *value);
        out.push_str(&format!("{key}={value}{}\n", if pending { " (pending)" } else { "" }));
    }
    if let Some(line) = agents_line(text) {
        out.push_str(&line);
        out.push('\n');
    }
    for (number, _, said) in policy::lines(text) {
        if let Some(reason) = ignored_reason(text, number, said) {
            out.push_str(&format!("ignored line {number}: {reason}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contoso() -> PolicyFile {
        PolicyFile::Trusted(String::from(
            "policy.manager=Contoso IT\npolicy.contact=helpdesk@contoso.example\n\
             shell.profile=linux\nmouse.buttons=sideways\napperance.theme=dark\n\
             agents.grants=view write\nagents.deny=disk_format disk_install\n",
        ))
    }

    #[test]
    fn a_trusted_file_locks_only_values_the_setting_takes() {
        let file = contoso();
        assert_eq!(file.manager(), "Contoso IT");
        assert_eq!(file.locked("shell.profile"), Some("linux"));
        assert_eq!(file.locked("mouse.buttons"), None);
        assert_eq!(file.locks(), [("shell.profile", "linux")]);
        assert_eq!(PolicyFile::Absent.locked("shell.profile"), None);
        assert_eq!(PolicyFile::Absent.manager(), "your administrator");
    }

    #[test]
    fn settings_policy_lists_the_locks_the_agents_and_the_ignored_lines() {
        let text = report(&contoso(), &Facts::example());
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("Managed by Contoso IT (helpdesk@contoso.example)"));
        assert_eq!(lines.next(), Some("/etc/eggos/policy: 1 setting locked"));
        assert!(lines.next().is_some_and(|line| line.starts_with("shell.profile=linux")));
        assert_eq!(lines.next(), Some("agents: at most view write; denied disk_format disk_install"));
        assert!(lines.next().is_some_and(|line| line.starts_with("ignored line 4: mouse.buttons: invalid value 'sideways'")));
        assert_eq!(lines.next(), Some("ignored line 5: apperance.theme: no such setting"));
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn without_a_trusted_file_it_says_why_nothing_is_locked() {
        assert_eq!(
            report(&PolicyFile::Untrusted(Distrust::OthersCanWrite), &Facts::example()),
            "The policy file cannot be trusted: others can write it. Nothing is locked; agents can only read.\n"
        );
        assert_eq!(
            report(&PolicyFile::NotInstalled, &Facts::example()),
            "Not managed: the management component is not installed.\n"
        );
    }
}
