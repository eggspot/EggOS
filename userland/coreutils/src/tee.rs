use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::{STANDARD_INPUT, System};
use core::ffi::CStr;
use eggos_commands::commands;
use eggos_runtime::errno::Errno;

const PIECE: usize = 512;
const MAX_FILES: usize = 32;
const MODE: u32 = 0o644;
const USAGE: &[u8] = b"usage: tee [-a] [--] [file...]";

struct Target<'a> {
    path: &'a CStr,
    descriptor: Option<usize>,
}

pub fn tee<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut append = false;
    let parsed = parse_defined(output, arguments, &commands::TEE, |_, _| {
        append = true;
        Ok(())
    });
    let Ok(operands) = parsed else {
        output.error(&[USAGE]);
        return FAILURE;
    };
    let mut targets: [Option<Target<'_>>; MAX_FILES] = [const { None }; MAX_FILES];
    for (count, &argument) in operands.all().iter().enumerate() {
        if count == MAX_FILES {
            output.error(&[b"too many files"]);
            return FAILURE;
        }
        targets[count] = Some(Target {
            path: argument,
            descriptor: None,
        });
    }
    let mut status = 0;
    for target in targets.iter_mut().flatten() {
        let opened = match append {
            true => output.system.append(target.path, MODE),
            false => output.system.create(target.path, MODE, true),
        };
        match opened {
            Ok(descriptor) => target.descriptor = Some(descriptor),
            Err(errno) => {
                output.file_error(target.path, errno);
                status = FAILURE;
            }
        }
    }
    let mut piece = [0u8; PIECE];
    loop {
        let length = match output.system.read(STANDARD_INPUT, &mut piece) {
            Ok(0) => break,
            Ok(length) => length,
            Err(Errno::EINTR) => continue,
            Err(errno) => {
                output.error(&[b"standard input: ", errno.message()]);
                status = FAILURE;
                break;
            }
        };
        let bytes = &piece[..length];
        output.write(bytes);
        output.flush();
        for target in targets.iter_mut().flatten() {
            let Some(descriptor) = target.descriptor else {
                continue;
            };
            if let Err(errno) = output.system.write(descriptor, bytes) {
                output.file_error(target.path, errno);
                output.system.close(descriptor);
                target.descriptor = None;
                status = FAILURE;
            }
        }
    }
    for target in targets.iter_mut().flatten() {
        if let Some(descriptor) = target.descriptor.take() {
            output.system.close(descriptor);
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(mut fake: Fake, input: &[u8], arguments: &[&CStr]) -> (i32, Fake) {
        fake.standard_input = input.to_vec();
        let mut output = Output::new(fake, b"tee");
        let status = tee(&mut output, arguments);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0, "a descriptor was left open");
        (status, output.system)
    }

    fn home() -> Fake {
        Fake::default().with_directory("/home", 0).with_file(
            "/home/note.txt",
            0o644,
            b"old text\n",
            0,
        )
    }

    #[test]
    fn standard_input_goes_to_standard_output_and_every_file() {
        let input = vec![b'z'; PIECE * 3 + 7];
        let (status, fake) = run(home(), &input, &[c"/home/note.txt", c"/home/new.txt"]);
        assert_eq!(status, 0);
        assert_eq!(fake.standard_output, input);
        assert_eq!(fake.contents("/home/note.txt"), input);
        assert_eq!(fake.contents("/home/new.txt"), input);
    }

    #[test]
    fn append_adds_to_the_end_and_creates_what_is_missing() {
        let (status, fake) = run(home(), b"more\n", &[c"-a", c"--", c"/home/note.txt"]);
        assert_eq!(status, 0);
        assert_eq!(fake.contents("/home/note.txt"), b"old text\nmore\n");
        let (status, fake) = run(home(), b"x", &[c"--append", c"/home/other"]);
        assert_eq!(status, 0);
        assert_eq!(fake.contents("/home/other"), b"x");
    }

    #[test]
    fn a_file_that_cannot_be_written_is_reported_and_the_rest_written() {
        let (status, fake) = run(
            home(),
            b"text",
            &[c"/nowhere/x", c"/home", c"/home/note.txt"],
        );
        assert_eq!(status, 1);
        assert_eq!(fake.contents("/home/note.txt"), b"text");
        assert_eq!(fake.standard_output, b"text");
        assert!(fake.errors().contains("tee: /nowhere/x: "));
        assert!(fake.errors().contains("tee: /home: "));
    }

    #[test]
    fn no_file_copies_standard_input_and_bad_options_are_refused() {
        let (status, fake) = run(home(), b"only out", &[]);
        assert_eq!(
            (status, fake.standard_output.as_slice()),
            (0, &b"only out"[..])
        );
        let (status, fake) = run(home(), b"x", &[c"-i", c"/home/note.txt"]);
        assert_eq!(status, 1);
        assert_eq!(fake.contents("/home/note.txt"), b"old text\n");
        assert!(fake.errors().starts_with("tee: invalid option -- 'i'\n"));
    }
}
