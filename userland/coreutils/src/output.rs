use crate::system::{STANDARD_ERROR, STANDARD_OUTPUT, System};
use core::ffi::CStr;
use eggos_commands::{ErrorDefinition, errors, page};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

pub const OUTPUT_CAPACITY: usize = 4096;

const ERROR_JSON_BYTES: usize = 4096;
const PAGE_JSON_BYTES: usize = page::MAX_CURSOR_BYTES + 128;
const MESSAGE_BYTES_IN_JSON: usize = 512;
pub const LAST_ERROR_BYTES: usize = 200;

pub const FAILURE: i32 = 1;

#[derive(Debug)]
pub struct Output<S: System> {
    pub system: S,
    utility: &'static [u8],
    buffer: [u8; OUTPUT_CAPACITY],
    length: usize,
    write_error: Option<Errno>,
    json: bool,
    last_error: [u8; LAST_ERROR_BYTES],
    last_error_length: usize,
}

impl<S: System> Output<S> {
    pub fn new(system: S, utility: &'static [u8]) -> Self {
        Self {
            system,
            utility,
            buffer: [0; OUTPUT_CAPACITY],
            length: 0,
            write_error: None,
            json: false,
            last_error: [0; LAST_ERROR_BYTES],
            last_error_length: 0,
        }
    }

    pub fn last_error(&self) -> &[u8] {
        &self.last_error[..self.last_error_length]
    }

    pub fn use_json(&mut self) {
        self.json = true;
    }

    pub fn is_json(&self) -> bool {
        self.json
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
        self.coded(&errors::FAILED, parts);
    }

    pub fn coded(&mut self, error: &ErrorDefinition, parts: &[&[u8]]) {
        self.flush();
        self.last_error_length = 0;
        for part in parts {
            let room = LAST_ERROR_BYTES - self.last_error_length;
            let taken = &part[..part.len().min(room)];
            self.last_error[self.last_error_length..][..taken.len()].copy_from_slice(taken);
            self.last_error_length += taken.len();
        }
        if self.json {
            self.json_error(error, parts);
            return;
        }
        let _ = self.system.write(STANDARD_ERROR, self.utility);
        let _ = self.system.write(STANDARD_ERROR, b": ");
        for part in parts {
            write_without_controls(&mut self.system, part);
        }
        let _ = self.system.write(STANDARD_ERROR, b"\n");
        if let Some(command) = error.fix_command {
            let _ = self.system.write(STANDARD_ERROR, b"  try: ");
            let _ = self.system.write(STANDARD_ERROR, command.as_bytes());
            let _ = self.system.write(STANDARD_ERROR, b"\n");
        }
        if self.system.wants_coded_errors() {
            self.json_error(error, parts);
        }
    }

    fn json_error(&mut self, error: &ErrorDefinition, parts: &[&[u8]]) {
        let mut buffer = [0u8; ERROR_JSON_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        out.begin_object();
        out.key("error");
        out.begin_object();
        out.key("code");
        out.string(error.code);
        out.key("category");
        out.string(error.category.name());
        out.key("message");
        out.begin_string();
        let mut room = MESSAGE_BYTES_IN_JSON;
        for part in parts {
            let taken = part.get(..room.min(part.len())).unwrap_or_default();
            out.string_piece_lossy(taken);
            room -= taken.len();
        }
        out.end_string();
        out.key("fix");
        out.string(error.fix);
        if let Some(command) = error.fix_command {
            out.key("fix_command");
            out.string(command);
        }
        out.end_object();
        out.end_object();
        out.end_line();
        if !out.overflowed() {
            let _ = self.system.write(STANDARD_ERROR, out.as_bytes());
        }
    }

    pub fn more(&mut self, last_key: &[u8]) {
        self.flush();
        let mut cursor = [0u8; page::MAX_CURSOR_BYTES];
        let Some(length) = page::encode(last_key, &mut cursor) else {
            return;
        };
        let cursor = &cursor[..length];
        if !self.json {
            let _ = self
                .system
                .write(STANDARD_ERROR, page::MORE_PREFIX.as_bytes());
            let _ = self.system.write(STANDARD_ERROR, cursor);
            let _ = self.system.write(STANDARD_ERROR, b"\n");
            return;
        }
        let mut buffer = [0u8; PAGE_JSON_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        out.begin_object();
        out.key("page");
        eggos_mcp::server::write_page(&mut out, core::str::from_utf8(cursor).ok());
        out.end_object();
        out.end_line();
        if !out.overflowed() {
            let _ = self.system.write(STANDARD_ERROR, out.as_bytes());
        }
    }

    pub fn file_error(&mut self, path: &CStr, errno: Errno) {
        self.coded(error_for(errno), &[path.to_bytes(), b": ", errno.message()]);
    }

    pub fn invalid_option(&mut self, option: &[u8]) -> i32 {
        match option.strip_prefix(b"--") {
            Some(_) => self.coded(&errors::USAGE, &[b"unrecognized option '", option, b"'"]),
            None => self.coded(
                &errors::USAGE,
                &[b"invalid option -- '", &option[1..2], b"'"],
            ),
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

fn write_without_controls<S: System>(system: &mut S, mut text: &[u8]) {
    while let Some(at) = text.iter().position(|&byte| byte < 0x20 || byte == 0x7f) {
        let (plain, rest) = text.split_at(at);
        let _ = system.write(STANDARD_ERROR, plain);
        let byte = rest.first().copied().unwrap_or_default();
        let escaped = match byte {
            b'\n' => *b"\\n  ",
            b'\t' => *b"\\t  ",
            b'\r' => *b"\\r  ",
            _ => [
                b'\\',
                b'x',
                HEX_DIGITS[usize::from(byte >> 4)],
                HEX_DIGITS[usize::from(byte & 0xf)],
            ],
        };
        let length = match byte {
            b'\n' | b'\t' | b'\r' => 2,
            _ => 4,
        };
        let _ = system.write(STANDARD_ERROR, &escaped[..length]);
        text = rest.get(1..).unwrap_or_default();
    }
    let _ = system.write(STANDARD_ERROR, text);
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

pub fn error_for(errno: Errno) -> &'static ErrorDefinition {
    match errno {
        Errno::ENOENT => &errors::NO_SUCH_FILE,
        Errno::EACCES | Errno::EPERM => &errors::PERMISSION_DENIED,
        Errno::EEXIST | Errno::ENOTEMPTY => &errors::ALREADY_EXISTS,
        Errno::EISDIR => &errors::IS_A_FOLDER,
        Errno::ENOTDIR => &errors::NOT_A_FOLDER,
        Errno::EROFS => &errors::READ_ONLY,
        Errno::ENOSPC => &errors::NO_SPACE,
        Errno::EBUSY | Errno::EAGAIN => &errors::BUSY,
        Errno::ETIMEDOUT => &errors::TIMED_OUT,
        Errno::EIO => &errors::DAMAGED,
        Errno::ENOMEM => &errors::OUT_OF_MEMORY,
        _ => &errors::FAILED,
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

    #[test]
    fn errors_are_coded_json_lines_once_json_is_asked_and_text_gains_a_fix_command() {
        let mut output = Output::new(Fake::default(), b"wm");
        output.coded(&errors::NO_SUCH_WINDOW, &[b"7", b": no such window"]);
        assert_eq!(
            output.system.errors(),
            "wm: 7: no such window\n  try: wm list\n"
        );
        let mut output = Output::new(Fake::default(), b"ls");
        output.use_json();
        output.file_error(c"/nope", Errno::ENOENT);
        output.coded(&errors::NO_SUCH_WINDOW, &[b"say \"hi\"\n"]);
        assert_eq!(
            output.system.errors(),
            "{\"error\":{\"code\":\"no_such_file\",\"category\":\"not_found\",\"message\":\"/nope: No such file or directory\",\"fix\":\"Check the path: ls shows what a folder holds.\"}}\n\
             {\"error\":{\"code\":\"no_such_window\",\"category\":\"not_found\",\"message\":\"say \\\"hi\\\"\\n\",\"fix\":\"Use an id that wm list shows.\",\"fix_command\":\"wm list\"}}\n"
        );
        let mut output = Output::new(Fake::default(), b"rm");
        output.coded(
            &errors::NO_SUCH_FILE,
            &[b"cannot remove '", b"a\nmore: --after x\t\x1b[2J\x7f\\", b"'"],
        );
        assert_eq!(
            output.system.errors(),
            "rm: cannot remove 'a\\nmore: --after x\\t\\x1b[2J\\x7f\\'\n"
        );
        assert_eq!(error_for(Errno::EROFS).code, "read_only");
        assert_eq!(error_for(Errno::EINVAL).code, "failed");
    }

    #[test]
    fn a_twin_asked_for_coded_errors_adds_the_json_line_after_the_text_lines() {
        let mut fake = Fake::default();
        fake.coded_errors = true;
        let mut output = Output::new(fake, b"wm");
        output.coded(&errors::NO_SUCH_WINDOW, &[b"7", b": no such\nwindow"]);
        assert_eq!(
            output.system.errors(),
            "wm: 7: no such\\nwindow\n  try: wm list\n\
             {\"error\":{\"code\":\"no_such_window\",\"category\":\"not_found\",\"message\":\"7: no such\\nwindow\",\"fix\":\"Use an id that wm list shows.\",\"fix_command\":\"wm list\"}}\n"
        );
        let mut fake = Fake::default();
        fake.coded_errors = true;
        let mut output = Output::new(fake, b"ls");
        output.use_json();
        output.file_error(c"/nope", Errno::ENOENT);
        assert_eq!(output.system.errors().lines().count(), 1);
    }

    #[test]
    fn more_names_the_next_page_s_cursor_as_text_or_json_after_the_page() {
        let mut output = Output::new(Fake::default(), b"ls");
        output.write(b"a b\n");
        output.more(b"a b");
        assert_eq!(output.system.output(), "a b\n");
        assert_eq!(output.system.errors(), "more: --after a%20b\n");
        let mut output = Output::new(Fake::default(), b"ls");
        output.use_json();
        output.more(b"-x");
        assert_eq!(
            output.system.errors(),
            "{\"page\":{\"truncated\":true,\"truncated_by\":\"limit\",\"next_cursor\":\"%2Dx\"}}\n"
        );
    }
}
