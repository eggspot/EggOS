//! `reboot(magic, magic2, cmd, arg)` (D106): powers the machine off, halts it or resets it,
//! for a privileged caller (effective user id 0, standing in for `CAP_SYS_BOOT`). As Linux:
//! `EPERM` for any other caller, checked first; then `EINVAL` unless `magic` is
//! `LINUX_REBOOT_MAGIC1` and `magic2` one of the four `LINUX_REBOOT_MAGIC2` values; then
//! `EINVAL` for a command it does not know or has not built in (`KEXEC`, `SW_SUSPEND`).
//! `CAD_ON` and `CAD_OFF` succeed and change nothing: no key combination resets the machine.
//! `RESTART2` resets as `RESTART` does, its command string unread. Nothing is synced: there is
//! no writable file system yet. Before the machine goes down every other CPU is parked
//! ([`crate::cpu::stop_others`]), and the console says what happens, in Linux's words.

use super::Errno;
use super::credentials::{self, Kind};
use crate::process::Process;
use core::fmt::Write;
use eggos_hal::{ExitStatus, Platform};

/// `LINUX_REBOOT_MAGIC1`.
pub const MAGIC1: u32 = 0xfee1_dead;

/// The `LINUX_REBOOT_MAGIC2` values: `LINUX_REBOOT_MAGIC2`, `2A`, `2B` and `2C`.
pub const MAGIC2: [u32; 4] = [0x2812_1969, 0x0512_1996, 0x1604_1998, 0x2011_2000];

/// The commands (`LINUX_REBOOT_CMD_*`).
pub mod command {
    /// Resets the machine.
    pub const RESTART: u32 = 0x0123_4567;
    /// Stops the machine.
    pub const HALT: u32 = 0xcdef_0123;
    /// Lets the reset key combination reset the machine at once.
    pub const CAD_ON: u32 = 0x89ab_cdef;
    /// Has the reset key combination signal init instead.
    pub const CAD_OFF: u32 = 0;
    /// Powers the machine off.
    pub const POWER_OFF: u32 = 0x4321_fedc;
    /// Resets the machine with a command for the firmware.
    pub const RESTART2: u32 = 0xa1b2_c3d4;
    /// Starts a kernel loaded before.
    pub const KEXEC: u32 = 0x4558_4543;
    /// Suspends the machine to disk.
    pub const SW_SUSPEND: u32 = 0xd000_fce2;
}

/// What a valid `reboot` call does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing: `CAD_ON` and `CAD_OFF`.
    None,
    /// Power the machine off.
    PowerOff,
    /// Stop the machine.
    Halt,
    /// Reset the machine.
    Restart,
}

impl Action {
    /// The line the console shows as the machine goes down, `None` for [`Action::None`].
    pub fn announcement(self) -> Option<&'static str> {
        match self {
            Action::None => None,
            Action::PowerOff => Some("reboot: Power down"),
            Action::Halt => Some("reboot: System halted"),
            Action::Restart => Some("reboot: Restarting system"),
        }
    }
}

/// What `reboot` with these arguments does for a caller that is `privileged` or not, or its
/// error, in Linux's order (see the module documentation).
pub fn decode(privileged: bool, magic: u32, magic2: u32, cmd: u32) -> Result<Action, Errno> {
    if !privileged {
        return Err(Errno::EPERM);
    }
    if magic != MAGIC1 || !MAGIC2.contains(&magic2) {
        return Err(Errno::EINVAL);
    }
    match cmd {
        command::CAD_ON | command::CAD_OFF => Ok(Action::None),
        command::POWER_OFF => Ok(Action::PowerOff),
        command::HALT => Ok(Action::Halt),
        command::RESTART | command::RESTART2 => Ok(Action::Restart),
        _ => Err(Errno::EINVAL),
    }
}

/// `reboot(magic, magic2, cmd, arg)`: see the module documentation. Returns 0 for
/// `CAD_ON` and `CAD_OFF`; does not return for the commands that take the machine down.
pub fn reboot<P: Platform>(
    process: &Process<P>,
    magic: u64,
    magic2: u64,
    cmd: u64,
) -> Result<u64, Errno> {
    let privileged = credentials::get_effective(process, Kind::User) == 0;
    match decode(privileged, magic as u32, magic2 as u32, cmd as u32)? {
        Action::None => Ok(0),
        action => take_machine_down::<P>(action),
    }
}

/// Parks every other CPU, says `action`'s line on the console, then takes the machine down as
/// `action` says.
#[cold]
#[inline(never)]
pub fn take_machine_down<P: Platform>(action: Action) -> ! {
    crate::cpu::stop_others::<P>();
    if let Some(line) = action.announcement() {
        let _ = writeln!(crate::console::PlatformConsole::<P>::new(), "{line}");
    }
    match action {
        Action::PowerOff => P::exit(ExitStatus::Success),
        Action::Restart => P::restart(),
        Action::None | Action::Halt => P::halt(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unprivileged_caller_gets_eperm_before_the_magic_is_checked() {
        assert_eq!(decode(false, 0, 0, command::POWER_OFF), Err(Errno::EPERM));
        assert_eq!(
            decode(false, MAGIC1, MAGIC2[0], command::POWER_OFF),
            Err(Errno::EPERM)
        );
    }

    #[test]
    fn bad_magic_is_einval() {
        assert_eq!(
            decode(true, MAGIC1 + 1, MAGIC2[0], command::POWER_OFF),
            Err(Errno::EINVAL)
        );
        assert_eq!(
            decode(true, MAGIC1, 0x1234, command::POWER_OFF),
            Err(Errno::EINVAL)
        );
        assert_eq!(
            decode(true, MAGIC2[0], MAGIC1, command::HALT),
            Err(Errno::EINVAL)
        );
    }

    #[test]
    fn every_second_magic_is_accepted() {
        for magic2 in MAGIC2 {
            assert_eq!(
                decode(true, MAGIC1, magic2, command::POWER_OFF),
                Ok(Action::PowerOff)
            );
        }
    }

    #[test]
    fn each_command_decodes_to_its_action() {
        let decoded = |cmd| decode(true, MAGIC1, MAGIC2[3], cmd);
        assert_eq!(decoded(command::POWER_OFF), Ok(Action::PowerOff));
        assert_eq!(decoded(command::HALT), Ok(Action::Halt));
        assert_eq!(decoded(command::RESTART), Ok(Action::Restart));
        assert_eq!(decoded(command::RESTART2), Ok(Action::Restart));
        assert_eq!(decoded(command::CAD_ON), Ok(Action::None));
        assert_eq!(decoded(command::CAD_OFF), Ok(Action::None));
        assert_eq!(decoded(command::KEXEC), Err(Errno::EINVAL));
        assert_eq!(decoded(command::SW_SUSPEND), Err(Errno::EINVAL));
        assert_eq!(decoded(0x1234_5678), Err(Errno::EINVAL));
    }

    #[test]
    fn only_the_commands_that_take_the_machine_down_announce_it() {
        assert_eq!(Action::None.announcement(), None);
        assert_eq!(Action::PowerOff.announcement(), Some("reboot: Power down"));
        assert_eq!(Action::Halt.announcement(), Some("reboot: System halted"));
        assert_eq!(
            Action::Restart.announcement(),
            Some("reboot: Restarting system")
        );
    }
}
