use crate::output::Output;
use crate::system::System;
use crate::utilities::MAX_ARGUMENTS;
use core::ffi::CStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item<'a> {
    Flag(u8),
    Valued(u8, &'a [u8]),
}

#[derive(Debug)]
pub struct Operands<'a> {
    operands: [&'a CStr; MAX_ARGUMENTS],
    count: usize,
}

impl<'a> Operands<'a> {
    pub fn all(&self) -> &[&'a CStr] {
        &self.operands[..self.count]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Reported,
}

pub fn parse<'a, S: System>(
    output: &mut Output<S>,
    arguments: &[&'a CStr],
    flags: &[u8],
    valued: &[u8],
    mut on_option: impl FnMut(&mut Output<S>, Item<'a>) -> Result<(), Refused>,
) -> Result<Operands<'a>, Refused> {
    let mut operands = Operands {
        operands: [c""; MAX_ARGUMENTS],
        count: 0,
    };
    let mut options_end = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index];
        index += 1;
        let bytes = argument.to_bytes();
        if options_end || bytes.len() < 2 || bytes[0] != b'-' {
            if operands.count == MAX_ARGUMENTS {
                output.error(&[b"too many operands"]);
                return Err(Refused::Reported);
            }
            operands.operands[operands.count] = argument;
            operands.count += 1;
            continue;
        }
        if bytes == b"--" {
            options_end = true;
            continue;
        }
        if bytes.starts_with(b"--") {
            output.invalid_option(bytes);
            return Err(Refused::Reported);
        }
        let mut at = 1;
        while at < bytes.len() {
            let letter = bytes[at];
            at += 1;
            if flags.contains(&letter) {
                on_option(output, Item::Flag(letter))?;
            } else if valued.contains(&letter) {
                let value = if at < bytes.len() {
                    &bytes[at..]
                } else if let Some(next) = arguments.get(index) {
                    index += 1;
                    next.to_bytes()
                } else {
                    output.error(&[b"option requires an argument -- '", &[letter], b"'"]);
                    return Err(Refused::Reported);
                };
                on_option(output, Item::Valued(letter, value))?;
                break;
            } else {
                output.invalid_option(&[b'-', letter]);
                return Err(Refused::Reported);
            }
        }
    }
    Ok(operands)
}

pub fn parse_count(text: &[u8]) -> Option<u64> {
    if text.is_empty() || text.len() > 19 {
        return None;
    }
    text.iter().try_fold(0u64, |value, &byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + u64::from(byte - b'0'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use std::vec::Vec;

    fn items(
        arguments: &[&'static CStr],
    ) -> (Result<Vec<String>, Refused>, Vec<Item<'static>>, String) {
        let mut output = Output::new(Fake::default(), b"t");
        let mut seen = Vec::new();
        let result = parse(&mut output, arguments, b"vc", b"n", |_, item| {
            seen.push(item);
            Ok(())
        })
        .map(|operands| {
            operands
                .all()
                .iter()
                .map(|operand| operand.to_string_lossy().into_owned())
                .collect()
        });
        output.finish(0);
        (result, seen, output.system.errors())
    }

    #[test]
    fn options_come_anywhere_combine_and_take_values_attached_or_next() {
        let (operands, seen, _) = items(&[c"a", c"-vc", c"-n5", c"b", c"-n", c"7", c"--", c"-v"]);
        assert_eq!(
            operands,
            Ok(vec!["a".to_owned(), "b".to_owned(), "-v".to_owned()])
        );
        assert_eq!(
            seen,
            [
                Item::Flag(b'v'),
                Item::Flag(b'c'),
                Item::Valued(b'n', b"5"),
                Item::Valued(b'n', b"7"),
            ]
        );
        let (operands, _, _) = items(&[c"-"]);
        assert_eq!(operands, Ok(vec!["-".to_owned()]));
    }

    #[test]
    fn unknown_options_and_missing_values_are_refused() {
        let (result, _, errors) = items(&[c"-x"]);
        assert_eq!(result, Err(Refused::Reported));
        assert_eq!(errors, "t: invalid option -- 'x'\n");
        let (_, _, errors) = items(&[c"-n"]);
        assert_eq!(errors, "t: option requires an argument -- 'n'\n");
        let (_, _, errors) = items(&[c"--long"]);
        assert_eq!(errors, "t: unrecognized option '--long'\n");
    }

    #[test]
    fn counts_are_decimal_digits_only() {
        assert_eq!(parse_count(b"10"), Some(10));
        assert_eq!(parse_count(b"0"), Some(0));
        for bad in [&b""[..], b"-1", b"1x", b"99999999999999999999"] {
            assert_eq!(parse_count(bad), None);
        }
    }
}
