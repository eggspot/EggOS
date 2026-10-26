use crate::system::{STANDARD_ERROR, STANDARD_OUTPUT, System};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;

pub const OUTPUT_CAPACITY: usize = 4096;

pub const FAILURE: i32 = 1;

#[derive(Debug)]
pub struct Output<S: System> {
    pub system: S,
    utility: &'static [u8],
    buffer: [u8; OUTPUT_CAPACITY],
    length: usize,
    write_error: Option<Errno>,
}

impl<S: System> Output<S> {
    pub fn new(system: S, utility: &'static [u8]) -> Self {
        Self {
            system,
            utility,
            buffer: [0; OUTPUT_CAPACITY],
            length: 0,
            write_error: None,
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if self.length + bytes.len() > OUTPUT_CAPACITY {
            self.flush();
            if bytes.len() > OUTPUT_CAPACITY {
                self.write_through(bytes);
                return;
            }
        }
        self.buffer[self.length..self.length + bytes.len()].copy_from_slice(bytes);
        self.length += bytes.len();
    }

    pub fn write_number(&mut self, value: u64, width: usize) {
        let mut digits = [0u8; 20];
        let text = decimal(value, &mut digits);
        for _ in text.len()..width {
            self.write(b" ");
        }
        self.write(text);
    }

    pub fn flush(&mut self) {
        let length = core::mem::take(&mut self.length);
        write_standard_output(
            &mut self.system,
            &mut self.write_error,
            &self.buffer[..length],
        );
    }

    fn write_through(&mut self, bytes: &[u8]) {
        write_standard_output(&mut self.system, &mut self.write_error, bytes);
    }

    pub fn error(&mut self, parts: &[&[u8]]) {
        self.flush();
        let _ = self.system.write(STANDARD_ERROR, self.utility);
        let _ = self.system.write(STANDARD_ERROR, b": ");
        for part in parts {
            let _ = self.system.write(STANDARD_ERROR, part);
        }
        let _ = self.system.write(STANDARD_ERROR, b"\n");
    }

    pub fn file_error(&mut self, path: &CStr, errno: Errno) {
        self.error(&[path.to_bytes(), b": ", errno.message()]);
    }

    pub fn invalid_option(&mut self, option: &[u8]) -> i32 {
        match option.strip_prefix(b"--") {
            Some(_) => self.error(&[b"unrecognized option '", option, b"'"]),
            None => self.error(&[b"invalid option -- '", &option[1..2], b"'"]),
        }
        FAILURE
    }

    pub fn finish(&mut self, status: i32) -> i32 {
        self.flush();
        match self.write_error {
            Some(errno) => {
                self.write_error = None;
                self.error(&[b"write error: ", errno.message()]);
                status.max(FAILURE)
            }
            None => status,
        }
    }
}

fn write_standard_output<S: System>(system: &mut S, write_error: &mut Option<Errno>, bytes: &[u8]) {
    if write_error.is_some() || bytes.is_empty() {
        return;
    }
    if let Err(errno) = system.write(STANDARD_OUTPUT, bytes) {
        *write_error = Some(errno);
    }
}

pub fn decimal(mut value: u64, digits: &mut [u8; 20]) -> &[u8] {
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &digits[start..];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    #[test]
    fn output_is_buffered_until_flushed_and_errors_flush_it_first() {
        let mut output = Output::new(Fake::default(), b"ls");
        output.write(b"a\n");
        assert!(output.system.standard_output.is_empty());
        output.error(&[b"oops"]);
        assert_eq!(output.system.standard_output, b"a\n");
        assert_eq!(output.system.standard_error, b"ls: oops\n");
        output.write(&[b'x'; OUTPUT_CAPACITY + 1]);
        assert_eq!(output.system.standard_output.len(), OUTPUT_CAPACITY + 3);
    }

    #[test]
    fn a_failed_write_is_reported_once_with_a_failure_status() {
        let mut output = Output::new(Fake::default(), b"echo");
        output.system.write_error = Some(Errno(28));
        output.write(b"text");
        assert_eq!(output.finish(0), FAILURE);
        assert_eq!(
            output.system.standard_error,
            b"echo: write error: No space left on device\n"
        );
        assert_eq!(output.finish(2), 2);
    }

    #[test]
    fn numbers_are_right_aligned_in_their_width() {
        let mut output = Output::new(Fake::default(), b"ls");
        output.write_number(42, 5);
        output.write_number(0, 0);
        output.write_number(u64::MAX, 1);
        output.flush();
        assert_eq!(output.system.standard_output, b"   42018446744073709551615");
    }

    #[test]
    fn invalid_options_are_named_as_getopt_does() {
        let mut output = Output::new(Fake::default(), b"cat");
        assert_eq!(output.invalid_option(b"-x"), FAILURE);
        assert_eq!(output.invalid_option(b"--long"), FAILURE);
        assert_eq!(
            output.system.standard_error,
            b"cat: invalid option -- 'x'\ncat: unrecognized option '--long'\n"
        );
    }
}
