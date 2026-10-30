use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::{Duration, System};
use core::ffi::CStr;
use eggos_commands::{CommandDefinition, commands, errors};
use eggos_runtime::errno::Errno;

pub const POWER_OFF: u32 = 0x4321_fedc;
pub const RESTART: u32 = 0x0123_4567;
pub const HALT: u32 = 0xcdef_0123;

const SIGRTMIN: u32 = 34;

const INIT_HALTS: u32 = SIGRTMIN + 3;
const INIT_POWERS_OFF: u32 = SIGRTMIN + 4;
const INIT_RESTARTS: u32 = SIGRTMIN + 5;

const INIT_ANSWER_WAIT: Duration = Duration {
    seconds: 2,
    nanoseconds: 0,
};

fn init_signal(command: u32) -> u32 {
    match command {
        HALT => INIT_HALTS,
        RESTART => INIT_RESTARTS,
        _ => INIT_POWERS_OFF,
    }
}

fn going_down_now(command: u32) -> &'static [u8] {
    match command {
        HALT => b"init did not answer; halting now",
        RESTART => b"init did not answer; restarting now",
        _ => b"init did not answer; powering off now",
    }
}

fn parse<S: System>(
    output: &mut Output<S>,
    arguments: &[&CStr],
    definition: &'static CommandDefinition,
) -> Option<bool> {
    let mut force = false;
    let parsed = parse_defined(output, arguments, definition, |_, given| {
        force |= given.option.short == Some(b'f');
        Ok(())
    });
    let operands = parsed.ok()?;
    match operands.all().first() {
        Some(extra) => {
            output.coded(
                &errors::USAGE,
                &[b"extra operand '", extra.to_bytes(), b"'"],
            );
            None
        }
        None => Some(force),
    }
}

pub fn sync<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    if parse(output, arguments, &commands::SYNC).is_none() {
        return FAILURE;
    }
    output.system.sync();
    0
}

pub fn power<S: System>(output: &mut Output<S>, arguments: &[&CStr], command: u32) -> i32 {
    let definition = match command {
        HALT => &commands::HALT,
        RESTART => &commands::REBOOT,
        _ => &commands::POWEROFF,
    };
    let Some(force) = parse(output, arguments, definition) else {
        return FAILURE;
    };
    output.flush();
    if !force {
        match output.system.signal_init(init_signal(command)) {
            Ok(()) => {
                let _ = output.system.sleep(INIT_ANSWER_WAIT);
                output.error(&[going_down_now(command)]);
                output.flush();
            }
            Err(Errno::EPERM) => {
                output.error(&[Errno::EPERM.message()]);
                return FAILURE;
            }
            Err(_) => {}
        }
    }
    output.system.sync();
    match output.system.reboot(command) {
        Ok(()) => 0,
        Err(errno) => {
            output.error(&[errno.message()]);
            FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run_with(fake: Fake, arguments: &[&CStr], command: u32) -> (i32, Fake) {
        let mut output = Output::new(fake, b"poweroff");
        let status = power(&mut output, arguments, command);
        let status = output.finish(status);
        (status, output.system)
    }

    fn run(arguments: &[&CStr], command: u32, allowed: bool) -> (i32, Fake) {
        let mut fake = Fake::default();
        if allowed {
            fake.reboot_error = None;
        }
        run_with(fake, arguments, command)
    }

    #[test]
    fn each_name_asks_init_with_systemd_s_signal_for_it() {
        for (command, signal) in [(POWER_OFF, 38), (RESTART, 39), (HALT, 37)] {
            let mut fake = Fake::default();
            fake.reboot_error = None;
            fake.init_signal_error = Some(Errno::ESRCH);
            let (status, fake) = run_with(fake, &[], command);
            assert_eq!(fake.init_signals, vec![signal]);
            assert_eq!((status, fake.rebooted), (0, vec![command]));
            assert!(fake.slept.is_empty(), "no init: nothing to wait for");
            assert_eq!(fake.errors(), "");
        }
    }

    #[test]
    fn force_goes_straight_to_the_kernel() {
        for arguments in [&[c"-f"][..], &[c"--force"][..], &[c"-ff"][..]] {
            let (status, fake) = run(arguments, POWER_OFF, true);
            assert_eq!((status, fake.rebooted), (0, vec![POWER_OFF]));
            assert!(fake.init_signals.is_empty());
        }
    }

    #[test]
    fn an_init_that_does_not_answer_in_time_is_gone_around() {
        let (status, fake) = run(&[], RESTART, true);
        assert_eq!(fake.init_signals, vec![INIT_RESTARTS]);
        assert_eq!(fake.slept, vec![INIT_ANSWER_WAIT]);
        assert_eq!(
            fake.errors(),
            "poweroff: init did not answer; restarting now\n"
        );
        assert_eq!((status, fake.rebooted, fake.synced), (0, vec![RESTART], 1));
    }

    #[test]
    fn sync_puts_changes_on_the_disk_and_takes_no_operands() {
        let mut output = Output::new(Fake::default(), b"sync");
        let status = sync(&mut output, &[]);
        assert_eq!((output.finish(status), output.system.synced), (0, 1));
        let mut output = Output::new(Fake::default(), b"sync");
        let status = sync(&mut output, &[c"file"]);
        assert_eq!(output.finish(status), 1);
        assert_eq!(output.system.synced, 0);
        assert_eq!(output.system.errors(), "sync: extra operand 'file'\n");
    }

    #[test]
    fn a_refusal_is_reported() {
        let mut fake = Fake::default();
        fake.init_signal_error = Some(Errno::EPERM);
        let (status, fake) = run_with(fake, &[], POWER_OFF);
        assert_eq!(status, 1);
        assert_eq!(fake.errors(), "poweroff: Operation not permitted\n");
        assert!(fake.rebooted.is_empty());
        let (status, fake) = run(&[c"-f"], POWER_OFF, false);
        assert_eq!(status, 1);
        assert_eq!(fake.errors(), "poweroff: Operation not permitted\n");
    }

    #[test]
    fn options_other_than_force_and_operands_are_refused_before_asking() {
        for (arguments, error) in [
            (&[c"-p"][..], "poweroff: invalid option -- 'p'\n"),
            (&[c"-fx"][..], "poweroff: invalid option -- 'x'\n"),
            (&[c"--halt"][..], "poweroff: unrecognized option '--halt'\n"),
            (&[c"now"][..], "poweroff: extra operand 'now'\n"),
            (&[c"-"][..], "poweroff: extra operand '-'\n"),
        ] {
            let (status, fake) = run(arguments, POWER_OFF, true);
            assert_eq!((status, fake.errors().as_str()), (1, error));
            assert!(fake.rebooted.is_empty());
            assert!(fake.init_signals.is_empty());
        }
    }
}
