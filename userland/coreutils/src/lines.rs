use crate::output::{FAILURE, Output};
use crate::system::{STANDARD_INPUT, System};
use core::ffi::CStr;
use core::ops::ControlFlow;
use eggos_runtime::errno::Errno;

pub const LINE_CAPACITY: usize = 16 * 1024;

pub const STANDARD_INPUT_NAME: &CStr = c"-";

pub fn is_standard_input(path: &CStr) -> bool {
    path.to_bytes() == b"-"
}

pub fn each_line<S: System>(
    output: &mut Output<S>,
    path: &CStr,
    mut on_line: impl FnMut(&mut Output<S>, &[u8]) -> ControlFlow<()>,
) -> i32 {
    let descriptor = if is_standard_input(path) {
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
    let mut buffer = [0u8; LINE_CAPACITY];
    let mut filled = 0;
    let mut status = 0;
    'reading: loop {
        output.flush();
        let read = match output.system.read(descriptor, &mut buffer[filled..]) {
            Ok(read) => read,
            Err(Errno::EINTR) => continue,
            Err(errno) => {
                output.file_error(path, errno);
                status = FAILURE;
                break;
            }
        };
        if read == 0 {
            if filled > 0 {
                let _ = on_line(output, &buffer[..filled]);
            }
            break;
        }
        let end = filled + read;
        let mut start = 0;
        while let Some(newline) = buffer[start..end].iter().position(|&byte| byte == b'\n') {
            let line_end = start + newline + 1;
            if on_line(output, &buffer[start..line_end]).is_break() {
                break 'reading;
            }
            start = line_end;
        }
        if start == 0 && end == LINE_CAPACITY {
            if on_line(output, &buffer[..end]).is_break() {
                break;
            }
            start = end;
        }
        buffer.copy_within(start..end, 0);
        filled = end - start;
    }
    if descriptor != STANDARD_INPUT {
        output.system.close(descriptor);
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use std::vec::Vec;

    fn lines_of(contents: &[u8]) -> Vec<Vec<u8>> {
        let fake = Fake::default().with_file("/f", 0o644, contents, 0);
        let mut output = Output::new(fake, b"test");
        let mut lines = Vec::new();
        let status = each_line(&mut output, c"/f", |_, line| {
            lines.push(line.to_vec());
            ControlFlow::Continue(())
        });
        assert_eq!(status, 0);
        assert_eq!(output.system.open_count(), 0);
        lines
    }

    #[test]
    fn lines_keep_their_newline_and_the_last_may_have_none() {
        assert_eq!(lines_of(b"a\nbb\n\nc"), [&b"a\n"[..], b"bb\n", b"\n", b"c"]);
        assert!(lines_of(b"").is_empty());
    }

    #[test]
    fn a_line_longer_than_the_buffer_comes_in_pieces() {
        let mut long = vec![b'x'; LINE_CAPACITY + 10];
        long.push(b'\n');
        let lines = lines_of(&long);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), LINE_CAPACITY);
        assert_eq!(lines[1].len(), 11);
    }

    #[test]
    fn a_missing_file_is_reported() {
        let mut output = Output::new(Fake::default(), b"test");
        assert_eq!(
            each_line(&mut output, c"/nope", |_, _| ControlFlow::Continue(())),
            1
        );
        output.finish(1);
        assert_eq!(
            output.system.errors(),
            "test: /nope: No such file or directory\n"
        );
    }
}
