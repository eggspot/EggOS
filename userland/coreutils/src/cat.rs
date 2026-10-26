use crate::output::{FAILURE, Output};
use crate::system::{STANDARD_INPUT, System};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;

const CHUNK: usize = 16 * 1024;

pub fn cat<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut options_end = false;
    for argument in arguments {
        let bytes = argument.to_bytes();
        if options_end || bytes.len() < 2 || bytes[0] != b'-' {
            continue;
        }
        match bytes {
            b"--" => options_end = true,
            b"-u" => {}
            _ if bytes.starts_with(b"--") => return output.invalid_option(bytes),
            _ => {
                if let Some(&letter) = bytes[1..].iter().find(|&&letter| letter != b'u') {
                    return output.invalid_option(&[b'-', letter]);
                }
            }
        }
    }
    let mut status = 0;
    let mut copied_any = false;
    let mut options_end = false;
    for &argument in arguments {
        let bytes = argument.to_bytes();
        if !options_end && bytes.len() > 1 && bytes[0] == b'-' {
            options_end = bytes == b"--";
            continue;
        }
        copied_any = true;
        status = status.max(copy(output, argument));
    }
    if !copied_any {
        status = copy(output, c"-");
    }
    status
}

fn copy<S: System>(output: &mut Output<S>, path: &CStr) -> i32 {
    let standard_input = path.to_bytes() == b"-";
    let descriptor = if standard_input {
        STANDARD_INPUT
    } else {
        match output.system.open(path) {
            Ok(descriptor) => descriptor,
            Err(errno) => {
                output.file_error(path, errno);
                return FAILURE;
            }
        }
    };
    let mut chunk = [0u8; CHUNK];
    let mut status = 0;
    loop {
        match output.system.read(descriptor, &mut chunk) {
            Ok(0) => break,
            Ok(length) => {
                output.write(&chunk[..length]);
                output.flush();
            }
            Err(Errno::EINTR) => {}
            Err(errno) => {
                output.file_error(path, errno);
                status = FAILURE;
                break;
            }
        }
    }
    if !standard_input {
        output.system.close(descriptor);
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(fake: Fake, arguments: &[&CStr]) -> (i32, Fake) {
        let mut output = Output::new(fake, b"cat");
        let status = cat(&mut output, arguments);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0, "a descriptor was left open");
        (status, output.system)
    }

    fn files() -> Fake {
        let mut fake = Fake::default()
            .with_directory("/etc", 0)
            .with_file("/etc/motd", 0o644, b"Welcome to EggOS.\n", 0)
            .with_file("/big", 0o644, &[b'x'; CHUNK * 2 + 5], 0);
        fake.standard_input = b"typed\nlines\n".to_vec();
        fake
    }

    #[test]
    fn files_are_copied_in_order_whole() {
        let (status, fake) = run(files(), &[c"/etc/motd", c"/big", c"/etc/motd"]);
        assert_eq!(status, 0);
        let mut expected = b"Welcome to EggOS.\n".to_vec();
        expected.extend_from_slice(&[b'x'; CHUNK * 2 + 5]);
        expected.extend_from_slice(b"Welcome to EggOS.\n");
        assert_eq!(fake.standard_output, expected);
    }

    #[test]
    fn no_operand_or_a_dash_copies_standard_input() {
        let (status, fake) = run(files(), &[]);
        assert_eq!((status, fake.output().as_str()), (0, "typed\nlines\n"));
        let (_, fake) = run(files(), &[c"-u", c"/etc/motd", c"-"]);
        assert_eq!(fake.output(), "Welcome to EggOS.\ntyped\nlines\n");
    }

    #[test]
    fn a_missing_file_or_a_directory_is_reported_and_the_rest_copied() {
        let (status, fake) = run(files(), &[c"/nope", c"/etc", c"/etc/motd"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "cat: /nope: No such file or directory\ncat: /etc: Is a directory\n"
        );
        assert_eq!(fake.output(), "Welcome to EggOS.\n");
    }

    #[test]
    fn options_are_checked_before_anything_is_copied() {
        let (status, fake) = run(files(), &[c"/etc/motd", c"-n"]);
        assert_eq!(status, 1);
        assert_eq!(fake.errors(), "cat: invalid option -- 'n'\n");
        assert_eq!(fake.output(), "");
        let (status, _) = run(files(), &[c"--number"]);
        assert_eq!(status, 1);
        let (status, fake) = run(files(), &[c"--", c"-n"]);
        assert_eq!(status, 1);
        assert_eq!(fake.errors(), "cat: -n: No such file or directory\n");
    }
}
