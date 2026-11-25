//! What init's daily update loop does under each update policy (D661; editions-policy v0.1
//! section 2): `automatic` checks every day, whatever the daily setting says, and installs
//! what the check finds; `notify` checks only when the daily check is on, as before; `managed`
//! neither checks nor installs. The policy is the component manager's record
//! (`/var/lib/eggos/update-policy`, `policy=<name>`), which only Settings writes.

/// The record's path, with its terminating NUL.
pub const PATH: &[u8] = b"/var/lib/eggos/update-policy\0";

/// The component manager's arguments that install every update the store has, recording how
/// it went for the Components pane (D772).
pub const INSTALL_ARGUMENTS: [&[u8]; 2] = [b"update\0", b"--automatic\0"];

/// What one turn of the daily loop does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// Wait and look again later.
    Wait,
    /// Check, recording what the store has.
    Check,
    /// Check, then install what it found.
    CheckAndInstall,
}

/// The turn the record `text` and the daily setting ask for; a missing or unknown policy is
/// `notify`.
pub fn turn(text: &[u8], daily_check_on: bool) -> Turn {
    let policy = text
        .split(|&byte| byte == b'\n')
        .filter_map(|line| line.strip_prefix(b"policy="))
        .map(<[u8]>::trim_ascii)
        .last()
        .unwrap_or(b"notify");
    match policy {
        b"automatic" => Turn::CheckAndInstall,
        b"managed" => Turn::Wait,
        _ if daily_check_on => Turn::Check,
        _ => Turn::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_policy_decides_whether_the_day_checks_and_installs() {
        assert_eq!(turn(b"policy=automatic\n", false), Turn::CheckAndInstall);
        assert_eq!(turn(b"policy=managed\nlocked_by=Contoso IT\n", true), Turn::Wait);
        assert_eq!(turn(b"policy=notify\n", true), Turn::Check);
        assert_eq!(turn(b"policy=notify\n", false), Turn::Wait);
        assert_eq!(turn(b"", true), Turn::Check);
    }
}
