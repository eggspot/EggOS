use crate::output::{FAILURE, Output, error_for};
use crate::system::System;
use core::ffi::CStr;
use eggos_android_properties::{self as format, AREA_PATH, SET_REQUEST};
use eggos_commands::errors;
use eggos_runtime::errno::Errno;

const LARGEST_AREA: usize = 64 * 1024;
const MOST_OPERANDS: usize = 3;

struct Operands<'a> {
    words: [&'a [u8]; MOST_OPERANDS],
    count: usize,
}

impl<'a> Operands<'a> {
    fn read<S: System>(output: &mut Output<S>, arguments: &[&'a CStr]) -> Result<Self, i32> {
        let mut operands = Operands {
            words: [&[][..]; MOST_OPERANDS],
            count: 0,
        };
        let mut options_end = false;
        for argument in arguments {
            let bytes = argument.to_bytes();
            if !options_end && bytes == b"--" {
                options_end = true;
                continue;
            }
            if !options_end && bytes.len() > 1 && bytes[0] == b'-' {
                return Err(output.invalid_option(bytes));
            }
            let Some(slot) = operands.words.get_mut(operands.count) else {
                output.coded(&errors::USAGE, &[b"extra operand '", bytes, b"'"]);
                return Err(FAILURE);
            };
            *slot = bytes;
            operands.count += 1;
        }
        Ok(operands)
    }

    fn get(&self) -> &[&'a [u8]] {
        &self.words[..self.count]
    }
}

pub fn getprop<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let operands = match Operands::read(output, arguments) {
        Ok(operands) => operands,
        Err(status) => return status,
    };
    if operands.count > 2 {
        output.coded(&errors::USAGE, &[b"usage: getprop [name [default]]"]);
        return FAILURE;
    }
    let mut area = [0u8; LARGEST_AREA];
    let length = match read_area(output, &mut area) {
        Ok(length) => length,
        Err(status) => return status,
    };
    let area = &area[..length];
    if !format::is_area(area) {
        output.coded(
            &errors::DAMAGED,
            &[AREA_PATH.to_bytes(), b": not a property area"],
        );
        return FAILURE;
    }
    match operands.get() {
        [] => {
            let listed = format::for_each(area, |name, value| {
                output.write(b"[");
                output.write(name);
                output.write(b"]: [");
                output.write(value);
                output.write(b"]\n");
            });
            if listed.is_err() {
                output.coded(&errors::DAMAGED, &[AREA_PATH.to_bytes(), b": damaged"]);
                return FAILURE;
            }
        }
        [name, default @ ..] => {
            let value = format::get(area, name)
                .or(default.first().copied())
                .unwrap_or_default();
            output.write(value);
            output.write(b"\n");
        }
    }
    0
}

pub fn setprop<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let operands = match Operands::read(output, arguments) {
        Ok(operands) => operands,
        Err(status) => return status,
    };
    let [name, value] = operands.get() else {
        output.coded(&errors::USAGE, &[b"usage: setprop name value"]);
        return FAILURE;
    };
    if !format::is_valid_name(name) {
        output.coded(
            &errors::USAGE,
            &[
                b"'",
                name,
                b"' is not a property name: parts of letters, digits, _ - : @ between dots",
            ],
        );
        return FAILURE;
    }
    let Some(mut request) =
        format::encode_request(name, value).filter(|_| format::is_valid_value(value))
    else {
        output.coded(
            &errors::INVALID_VALUE,
            &[b"a property's value is at most 91 bytes, without NUL"],
        );
        return FAILURE;
    };
    let descriptor = match output.system.open(AREA_PATH) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.file_error(AREA_PATH, errno);
            return FAILURE;
        }
    };
    let result = output
        .system
        .device_control(descriptor, SET_REQUEST, &mut request);
    output.system.close(descriptor);
    match result {
        Ok(()) => 0,
        Err(Errno::EROFS) => {
            output.coded(
                &errors::READ_ONLY,
                &[name, b": a read-only property keeps its first value"],
            );
            FAILURE
        }
        Err(errno) => {
            output.coded(
                error_for(errno),
                &[b"cannot set ", name, b": ", errno.message()],
            );
            FAILURE
        }
    }
}

fn read_area<S: System>(output: &mut Output<S>, area: &mut [u8]) -> Result<usize, i32> {
    let descriptor = match output.system.open(AREA_PATH) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.file_error(AREA_PATH, errno);
            return Err(FAILURE);
        }
    };
    let mut length = 0;
    let result = loop {
        let Some(room) = area.get_mut(length..).filter(|room| !room.is_empty()) else {
            break Ok(length);
        };
        match output.system.read(descriptor, room) {
            Ok(0) => break Ok(length),
            Ok(read) => length += read,
            Err(errno) => break Err(errno),
        }
    };
    output.system.close(descriptor);
    result.map_err(|errno| {
        output.file_error(AREA_PATH, errno);
        FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn system() -> Fake {
        let mut area = vec![0; 4096];
        format::initialize(&mut area).unwrap();
        format::set(&mut area, b"ro.build.version.sdk", b"29").unwrap();
        format::set(&mut area, b"ro.hardware", b"eggos").unwrap();
        Fake::default().with_file("/dev/__properties__", 0o444, &area, 0)
    }

    fn getprop_output(arguments: &[&CStr]) -> (i32, String, String) {
        let mut output = Output::new(system(), b"getprop");
        let status = getprop(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system.output(), output.system.errors())
    }

    #[test]
    fn getprop_prints_a_value_its_default_or_every_property() {
        assert_eq!(
            getprop_output(&[c"ro.build.version.sdk"]),
            (0, "29\n".into(), String::new())
        );
        assert_eq!(getprop_output(&[c"ro.unset"]).1, "\n");
        assert_eq!(getprop_output(&[c"--", c"ro.unset", c"-1"]).1, "-1\n");
        assert_eq!(
            getprop_output(&[]).1,
            "[ro.build.version.sdk]: [29]\n[ro.hardware]: [eggos]\n"
        );
        let (status, _, errors) = getprop_output(&[c"a", c"b", c"c"]);
        assert_eq!(status, 1);
        assert!(errors.contains("usage: getprop"), "{errors}");
        assert_eq!(getprop_output(&[c"-x"]).0, 1);
    }

    #[test]
    fn setprop_sets_a_property_that_getprop_then_reads() {
        let mut output = Output::new(system(), b"setprop");
        assert_eq!(setprop(&mut output, &[c"eggos.greeting", c"hello"]), 0);
        assert_eq!(setprop(&mut output, &[c"--", c"eggos.level", c"-1"]), 0);
        assert_eq!(getprop(&mut output, &[c"eggos.greeting"]), 0);
        assert_eq!(getprop(&mut output, &[c"eggos.level"]), 0);
        output.flush();
        assert_eq!(output.system.output(), "hello\n-1\n");
    }

    #[test]
    fn setprop_refuses_read_only_properties_bad_names_and_long_values() {
        let mut output = Output::new(system(), b"setprop");
        assert_eq!(setprop(&mut output, &[c"ro.hardware", c"other"]), FAILURE);
        assert_eq!(setprop(&mut output, &[c"bad..name", c"1"]), FAILURE);
        let long = [b'v'; 92];
        let long = CStr::from_bytes_with_nul(&[&long[..], b"\0"].concat())
            .unwrap()
            .to_owned();
        assert_eq!(setprop(&mut output, &[c"eggos.long", &long]), FAILURE);
        assert_eq!(setprop(&mut output, &[c"eggos.only"]), FAILURE);
        output.flush();
        let errors = output.system.errors();
        assert!(errors.contains("read-only property keeps"), "{errors}");
        assert!(
            errors.contains("'bad..name' is not a property name"),
            "{errors}"
        );
        assert!(errors.contains("at most 91 bytes"), "{errors}");
        assert!(errors.contains("usage: setprop name value"), "{errors}");
    }
}
