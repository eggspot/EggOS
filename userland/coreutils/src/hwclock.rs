use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_calendar::format;
use eggos_runtime::errno::Errno;

const CLOCK: &CStr = c"/proc/driver/rtc";
const CLOCK_TEXT_CAPACITY: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Function {
    Show,
    SystemToHardware,
    HardwareToSystem,
}

struct Line {
    bytes: [u8; 32],
    length: usize,
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        let room = self
            .bytes
            .get_mut(self.length..end)
            .ok_or(core::fmt::Error)?;
        room.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

pub fn hwclock<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut function = Function::Show;
    for argument in arguments {
        match argument.to_bytes() {
            b"-r" | b"--show" | b"--get" => function = Function::Show,
            b"-w" | b"--systohc" => function = Function::SystemToHardware,
            b"-s" | b"--hctosys" => function = Function::HardwareToSystem,
            b"-u" | b"--utc" => {}
            other if other.len() > 1 && other[0] == b'-' => return output.invalid_option(other),
            other => {
                output.error(&[b"extra operand '", other, b"'"]);
                return FAILURE;
            }
        }
    }
    let hardware = match read_clock(output) {
        Ok(seconds) => seconds,
        Err(status) => return status,
    };
    let set = match function {
        Function::Show => {
            let mut line = Line {
                bytes: [0; 32],
                length: 0,
            };
            if format::iso(&mut line, hardware).is_err() {
                output.error(&[b"the hardware clock's time cannot be shown"]);
                return FAILURE;
            }
            output.write(&line.bytes[..line.length]);
            output.write(b"+00:00\n");
            return 0;
        }
        Function::SystemToHardware => output.system.now(),
        Function::HardwareToSystem => hardware.max(0) as u64,
    };
    match output.system.set_time(set) {
        Ok(()) => 0,
        Err(errno) => {
            output.error(&[b"cannot set the clock: ", errno.message()]);
            FAILURE
        }
    }
}

fn read_clock<S: System>(output: &mut Output<S>) -> Result<i64, i32> {
    let descriptor = match output.system.open(CLOCK) {
        Ok(descriptor) => descriptor,
        Err(Errno::ENOENT) => {
            output.error(&[b"no hardware clock: ", CLOCK.to_bytes(), b" is missing"]);
            return Err(FAILURE);
        }
        Err(errno) => {
            output.file_error(CLOCK, errno);
            return Err(FAILURE);
        }
    };
    let mut text = [0u8; CLOCK_TEXT_CAPACITY];
    let mut length = 0;
    let read = loop {
        match output.system.read(descriptor, &mut text[length..]) {
            Ok(0) => break Ok(()),
            Ok(count) => {
                length += count;
                if length == text.len() {
                    break Ok(());
                }
            }
            Err(Errno::EINTR) => {}
            Err(errno) => break Err(errno),
        }
    };
    output.system.close(descriptor);
    if let Err(errno) = read {
        output.file_error(CLOCK, errno);
        return Err(FAILURE);
    }
    eggos_calendar::parse_proc_driver_rtc(&text[..length]).ok_or_else(|| {
        output.error(&[b"cannot read the hardware clock"]);
        FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    const RTC: &[u8] = b"rtc_time\t: 14:05:09\nrtc_date\t: 2026-10-29\n24hr\t\t: yes\n";

    fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String, Fake) {
        let mut output = Output::new(fake, b"hwclock");
        let status = hwclock(&mut output, arguments);
        let status = output.finish(status);
        let fake = output.system;
        assert_eq!(fake.open_count(), 0);
        let (out, errors) = (fake.output(), fake.errors());
        (status, out, errors, fake)
    }

    fn with_clock() -> Fake {
        Fake::default().with_file("/proc/driver/rtc", 0o444, RTC, 0)
    }

    #[test]
    fn it_shows_the_hardware_clock_in_utc() {
        for arguments in [&[][..], &[c"-r"][..], &[c"--show", c"-u"][..]] {
            let (status, out, _, _) = run(with_clock(), arguments);
            assert_eq!((status, out.as_str()), (0, "2026-10-29 14:05:09+00:00\n"));
        }
    }

    #[test]
    fn it_copies_the_system_clock_to_the_hardware_one_and_back() {
        let (status, _, _, fake) = run(with_clock(), &[c"-w"]);
        assert_eq!(status, 0);
        assert_eq!(fake.set_times, [1_700_000_000]);
        let (status, _, _, fake) = run(with_clock(), &[c"--hctosys"]);
        assert_eq!(status, 0);
        assert_eq!(fake.set_times, [1_793_282_709]);
    }

    #[test]
    fn it_says_why_it_cannot() {
        let (status, _, errors, _) = run(Fake::default(), &[]);
        assert_eq!(status, 1);
        assert_eq!(
            errors,
            "hwclock: no hardware clock: /proc/driver/rtc is missing\n"
        );
        let garbled = Fake::default().with_file("/proc/driver/rtc", 0o444, b"rtc_time : x\n", 0);
        let (_, _, errors, _) = run(garbled, &[]);
        assert_eq!(errors, "hwclock: cannot read the hardware clock\n");
        let mut refused = with_clock();
        refused.set_time_error = Some(Errno::EPERM);
        let (status, _, errors, _) = run(refused, &[c"-s"]);
        assert_eq!(status, 1);
        assert_eq!(
            errors,
            "hwclock: cannot set the clock: Operation not permitted\n"
        );
        let (_, _, errors, _) = run(with_clock(), &[c"--adjust"]);
        assert_eq!(errors, "hwclock: unrecognized option '--adjust'\n");
    }
}
