use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::{CommandDefinition, commands, errors};

pub const POWER_OFF: u32 = 0x4321_fedc;
pub const RESTART: u32 = 0x0123_4567;
pub const HALT: u32 = 0xcdef_0123;

fn no_operands<S: System>(
    output: &mut Output<S>,
    arguments: &[&CStr],
    definition: &'static CommandDefinition,
) -> bool {
    let Ok(operands) = parse_defined(output, arguments, definition, |_, _| Ok(())) else {
        return false;
    };
    match operands.all().first() {
        Some(extra) => {
            output.coded(
                &errors::USAGE,
                &[b"extra operand '", extra.to_bytes(), b"'"],
            );
            false
        }
        None => true,
    }
}

pub fn sync<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    if !no_operands(output, arguments, &commands::SYNC) {
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
    if !no_operands(output, arguments, definition) {
        return FAILURE;
    }
    output.flush();
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

    fn run(arguments: &[&CStr], command: u32, allowed: bool) -> (i32, Fake) {
        let mut fake = Fake::default();
        if allowed {
            fake.reboot_error = None;
        }
        let mut output = Output::new(fake, b"poweroff");
        let status = power(&mut output, arguments, command);
        let status = output.finish(status);
        (status, output.system)
    }

    #[test]
    fn each_name_asks_the_kernel_for_its_command() {
        for command in [POWER_OFF, RESTART, HALT] {
            let (status, fake) = run(&[], command, true);
            assert_eq!((status, fake.rebooted), (0, vec![command]));
        }
        let (status, fake) = run(&[c"-f", c"--force", c"-ff"], POWER_OFF, true);
        assert_eq!((status, fake.rebooted), (0, vec![POWER_OFF]));
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
        let (status, fake) = run(&[], POWER_OFF, false);
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
        }
    }
}
