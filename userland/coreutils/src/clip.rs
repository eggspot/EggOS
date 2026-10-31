//! `clip` (D197): the clipboard every app shares, kept by the window server. `clip get` writes
//! its text to standard output as it is; `clip set <words>...` makes the words, joined by
//! spaces, the clipboard's text, and `clip set` alone what standard input holds, at most
//! [`SET_LIMIT`] bytes of UTF-8.

use crate::output::{FAILURE, Output};
use crate::system::{STANDARD_INPUT, System};
use crate::wm::open_device;
use core::ffi::CStr;
use eggos_commands::errors;
use eggos_runtime::errno::Errno;

/// `SET_CLIPBOARD`, the window server's request that replaces the clipboard's text.
pub const SET_CLIPBOARD: u32 = 0x4010_7720;
/// `GET_CLIPBOARD`, the window server's request that reads it.
pub const GET_CLIPBOARD: u32 = 0xc020_7721;
/// The most bytes `clip set` takes.
pub const SET_LIMIT: usize = 32 * 1024;

const PIECE: usize = 4096;
const READ_ATTEMPTS: usize = 3;
const USAGE: &[u8] = b"usage: clip get | clip set [--] [words...]";
const USAGE_STATUS: i32 = 2;

fn usage<S: System>(output: &mut Output<S>) -> i32 {
    output.coded(&errors::USAGE, &[USAGE]);
    USAGE_STATUS
}

fn get<S: System>(output: &mut Output<S>, device: usize) -> i32 {
    let mut piece = [0u8; PIECE];
    'attempt: for _ in 0..READ_ATTEMPTS {
        let mut offset = 0;
        let mut first_serial = None;
        loop {
            let (length, serial) = match output.system.clipboard(device, offset, &mut piece) {
                Ok(read) => read,
                Err(errno) => {
                    output.coded(
                        &errors::FAILED,
                        &[b"cannot read the clipboard: ", errno.message()],
                    );
                    return FAILURE;
                }
            };
            if *first_serial.get_or_insert(serial) != serial {
                continue 'attempt;
            }
            let copied = length.saturating_sub(offset).min(PIECE);
            output.write(&piece[..copied]);
            offset += copied;
            if offset >= length {
                return 0;
            }
        }
    }
    output.coded(
        &errors::FAILED,
        &[b"the clipboard kept changing while it was read"],
    );
    FAILURE
}

fn set<S: System>(output: &mut Output<S>, device: usize, words: &[&CStr]) -> i32 {
    let mut text = [0u8; SET_LIMIT];
    let mut length = 0;
    let mut too_long = false;
    if words.is_empty() {
        loop {
            if length == SET_LIMIT {
                let mut probe = [0u8; 1];
                too_long = output
                    .system
                    .read(STANDARD_INPUT, &mut probe)
                    .is_ok_and(|read| read > 0);
                break;
            }
            match output.system.read(STANDARD_INPUT, &mut text[length..]) {
                Ok(0) => break,
                Ok(read) => length += read,
                Err(Errno::EINTR) => continue,
                Err(errno) => {
                    output.coded(
                        &errors::FAILED,
                        &[b"cannot read standard input: ", errno.message()],
                    );
                    return FAILURE;
                }
            }
        }
    } else {
        for (index, word) in words.iter().enumerate() {
            let separator: &[u8] = if index == 0 { b"" } else { b" " };
            for part in [separator, word.to_bytes()] {
                match text.get_mut(length..length + part.len()) {
                    Some(place) => {
                        place.copy_from_slice(part);
                        length += part.len();
                    }
                    None => too_long = true,
                }
            }
        }
    }
    if too_long {
        output.coded(&errors::FAILED, &[b"the text is longer than 32768 bytes"]);
        return FAILURE;
    }
    match output.system.set_clipboard(device, &text[..length]) {
        Ok(()) => 0,
        Err(Errno::EINVAL) => {
            output.coded(&errors::FAILED, &[b"the text is not UTF-8"]);
            FAILURE
        }
        Err(errno) => {
            output.coded(
                &errors::FAILED,
                &[b"cannot set the clipboard: ", errno.message()],
            );
            FAILURE
        }
    }
}

pub fn clip<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some((verb, rest)) = arguments.split_first() else {
        return usage(output);
    };
    let (rest, options_ended) = match rest.split_first() {
        Some((first, after)) if first.to_bytes() == b"--" => (after, true),
        _ => (rest, false),
    };
    let verb = verb.to_bytes();
    let options_given = !options_ended
        && rest.iter().any(|word| {
            let word = word.to_bytes();
            word.len() > 1 && word.starts_with(b"-")
        });
    match verb {
        b"get" if rest.is_empty() => match open_device(output) {
            Some(device) => get(output, device),
            None => FAILURE,
        },
        b"set" if !options_given => match open_device(output) {
            Some(device) => set(output, device, rest),
            None => FAILURE,
        },
        _ => usage(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(fake: &mut Fake, arguments: &[&CStr]) -> i32 {
        let mut output = Output::new(&mut *fake, b"clip");
        let status = clip(&mut output, arguments);
        output.finish(status)
    }

    #[test]
    fn words_set_the_clipboard_and_get_writes_it_back() {
        let mut fake = Fake::default();
        fake.window_server = true;
        assert_eq!(run(&mut fake, &[c"set", c"hello", c"world"]), 0);
        assert_eq!(fake.clipboard, b"hello world");
        assert_eq!(run(&mut fake, &[c"get"]), 0);
        assert_eq!(fake.standard_output, b"hello world");
    }

    #[test]
    fn standard_input_sets_it_when_no_words_are_given() {
        let mut fake = Fake::default();
        fake.window_server = true;
        let long: Vec<u8> = (0..10_000).map(|index| b'a' + (index % 26) as u8).collect();
        fake.standard_input = long.clone();
        assert_eq!(run(&mut fake, &[c"set"]), 0);
        assert_eq!(fake.clipboard, long);
        assert_eq!(run(&mut fake, &[c"get"]), 0);
        assert_eq!(fake.standard_output, long);
    }

    #[test]
    fn text_past_the_limit_or_not_utf8_is_refused() {
        let mut fake = Fake::default();
        fake.window_server = true;
        fake.standard_input = vec![b'x'; SET_LIMIT + 1];
        assert_eq!(run(&mut fake, &[c"set"]), FAILURE);
        assert!(fake.clipboard.is_empty());
        let mut fake = Fake::default();
        fake.window_server = true;
        fake.standard_input = vec![0xff, 0xfe];
        assert_eq!(run(&mut fake, &[c"set"]), FAILURE);
        assert!(String::from_utf8_lossy(&fake.standard_error).contains("not UTF-8"));
        assert_eq!(run(&mut Fake::default(), &[c"paste"]), USAGE_STATUS);
        assert_eq!(run(&mut Fake::default(), &[c"get", c"x"]), USAGE_STATUS);
    }
}
