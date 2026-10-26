use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;

pub const POWER_OFF: u32 = 0x4321_fedc;
pub const RESTART: u32 = 0x0123_4567;
pub const HALT: u32 = 0xcdef_0123;

pub fn power<S: System>(output: &mut Output<S>, arguments: &[&CStr], command: u32) -> i32 {
    for argument in arguments {
        match argument.to_bytes() {
            b"-f" | b"--force" => {}
            option @ [b'-', _, ..] if option.starts_with(b"--") => {
                return output.invalid_option(option);
            }
            [b'-', letters @ ..] if !letters.is_empty() => {
                if let Some(&letter) = letters.iter().find(|&&letter| letter != b'f') {
                    return output.invalid_option(&[b'-', letter]);
                }
            }
            operand => {
                output.error(&[b"extra operand '", operand, b"'"]);
                return FAILURE;
            }
        }
    }
    output.flush();
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
