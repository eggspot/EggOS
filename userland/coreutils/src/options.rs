use crate::output::Output;
use crate::system::System;
use crate::utilities::MAX_ARGUMENTS;
use core::ffi::CStr;
use eggos_commands::{CommandDefinition, OptionDefinition, OptionValue, ValueType, errors};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Given<'a> {
    pub option: &'static OptionDefinition,
    pub value: Option<&'a [u8]>,
    terminated: Option<&'a CStr>,
    second: Option<&'a CStr>,
}

impl<'a> Given<'a> {
    fn of(option: &'static OptionDefinition, terminated: Option<&'a CStr>) -> Self {
        Self {
            option,
            value: terminated.map(CStr::to_bytes),
            terminated,
            second: None,
        }
    }

    fn with_second(self, second: Option<&'a CStr>) -> Self {
        Self { second, ..self }
    }

    pub fn is_long(&self, name: &str) -> bool {
        self.option.long == Some(name)
    }

    pub fn value_as_c_string(&self) -> Option<&'a CStr> {
        self.terminated
    }

    pub fn second_value(&self) -> Option<&'a [u8]> {
        self.second.map(CStr::to_bytes)
    }
}

fn second_word<'a>(
    kind: Option<OptionValue>,
    first: Option<&CStr>,
    arguments: &[&'a CStr],
    index: &mut usize,
) -> Result<Option<&'a CStr>, Refused> {
    match kind {
        Some(kind) if first.is_some() && kind.name.contains(' ') => {
            let next = *arguments.get(*index).ok_or(Refused::Reported)?;
            *index += 1;
            Ok(Some(next))
        }
        _ => Ok(None),
    }
}

fn takes_negative_numbers(definition: &CommandDefinition) -> bool {
    let negative =
        |value: ValueType| matches!(value, ValueType::Integer { minimum, .. } if minimum < 0);
    definition
        .operands
        .iter()
        .any(|operand| negative(operand.value))
        && !definition
            .options
            .iter()
            .any(|option| option.short.is_some_and(|letter| letter.is_ascii_digit()))
}

fn tail(argument: &CStr, from: usize) -> &CStr {
    CStr::from_bytes_with_nul(&argument.to_bytes_with_nul()[from..]).unwrap_or(c"")
}

pub fn wants_json(definition: &CommandDefinition, arguments: &[&CStr]) -> bool {
    definition.has_json()
        && arguments
            .iter()
            .map(|argument| argument.to_bytes())
            .take_while(|&bytes| bytes != b"--")
            .any(|bytes| bytes == b"--json")
}

pub fn parse_defined<'a, S: System>(
    output: &mut Output<S>,
    arguments: &[&'a CStr],
    definition: &'static CommandDefinition,
    mut on_option: impl FnMut(&mut Output<S>, Given<'a>) -> Result<(), Refused>,
) -> Result<Operands<'a>, Refused> {
    if wants_json(definition, arguments) {
        output.use_json();
    }
    let mut operands = Operands {
        operands: [c""; MAX_ARGUMENTS],
        count: 0,
    };
    let negative_numbers = takes_negative_numbers(definition);
    let mut options_end = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index];
        index += 1;
        let bytes = argument.to_bytes();
        if options_end
            || bytes.len() < 2
            || bytes[0] != b'-'
            || (negative_numbers && bytes[1..].iter().all(u8::is_ascii_digit))
        {
            if operands.count == MAX_ARGUMENTS {
                output.coded(&errors::USAGE, &[b"too many operands"]);
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
        if let Some(long) = bytes.strip_prefix(b"--") {
            let (name, attached) = match long.iter().position(|&byte| byte == b'=') {
                Some(at) => (&long[..at], Some(tail(argument, at + 3))),
                None => (long, None),
            };
            let Some(option) = definition.long_option(name) else {
                output.invalid_option(bytes);
                return Err(Refused::Reported);
            };
            let value = match (option.value, attached) {
                (None, None) => None,
                (None, Some(_)) => {
                    output.coded(
                        &errors::USAGE,
                        &[b"option '--", name, b"' doesn't allow an argument"],
                    );
                    return Err(Refused::Reported);
                }
                (Some(_), Some(value)) => Some(value),
                (Some(kind), None) if kind.optional => None,
                (Some(_), None) => match arguments.get(index) {
                    Some(&next) => {
                        index += 1;
                        Some(next)
                    }
                    None => {
                        output.coded(
                            &errors::USAGE,
                            &[b"option '--", name, b"' requires an argument"],
                        );
                        return Err(Refused::Reported);
                    }
                },
            };
            let Ok(second) = second_word(option.value, value, arguments, &mut index) else {
                output.coded(
                    &errors::USAGE,
                    &[b"option '--", name, b"' requires an argument"],
                );
                return Err(Refused::Reported);
            };
            on_option(output, Given::of(option, value).with_second(second))?;
            continue;
        }
        let mut at = 1;
        while at < bytes.len() {
            let letter = bytes[at];
            at += 1;
            let Some(option) = definition.short_option(letter) else {
                output.invalid_option(&[b'-', letter]);
                return Err(Refused::Reported);
            };
            let Some(kind) = option.value else {
                on_option(output, Given::of(option, None))?;
                continue;
            };
            let value = if at < bytes.len() {
                Some(tail(argument, at))
            } else if kind.optional {
                None
            } else if let Some(&next) = arguments.get(index) {
                index += 1;
                Some(next)
            } else {
                output.coded(
                    &errors::USAGE,
                    &[b"option requires an argument -- '", &[letter], b"'"],
                );
                return Err(Refused::Reported);
            };
            let Ok(second) = second_word(option.value, value, arguments, &mut index) else {
                output.coded(
                    &errors::USAGE,
                    &[b"option requires an argument -- '", &[letter], b"'"],
                );
                return Err(Refused::Reported);
            };
            on_option(output, Given::of(option, value).with_second(second))?;
            break;
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
    use eggos_commands::commands;
    use std::vec::Vec;

    fn given_to(
        definition: &'static CommandDefinition,
        arguments: &[&'static CStr],
    ) -> (Result<Vec<String>, Refused>, Vec<String>, String) {
        let mut output = Output::new(Fake::default(), b"t");
        let mut seen = Vec::new();
        let result = parse_defined(&mut output, arguments, definition, |_, given| {
            let name = match (given.option.short, given.option.long) {
                (Some(letter), _) => (letter as char).to_string(),
                (None, Some(long)) => long.to_owned(),
                (None, None) => String::new(),
            };
            let value = given
                .value
                .map(|value| String::from_utf8_lossy(value).into_owned())
                .unwrap_or_default();
            seen.push(format!("{name}={value}"));
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

    fn given(arguments: &[&'static CStr]) -> (Result<Vec<String>, Refused>, Vec<String>, String) {
        given_to(&commands::AGENTLOG, arguments)
    }

    #[test]
    fn options_come_anywhere_combine_and_take_values_attached_or_next() {
        let (operands, seen, _) = given_to(
            &commands::HEAD,
            &[c"a", c"-qv", c"-n5", c"b", c"-n", c"7", c"--", c"-v"],
        );
        assert_eq!(
            operands,
            Ok(vec!["a".to_owned(), "b".to_owned(), "-v".to_owned()])
        );
        assert_eq!(seen, ["q=", "v=", "n=5", "n=7"]);
        let (operands, _, _) = given_to(&commands::HEAD, &[c"-"]);
        assert_eq!(operands, Ok(vec!["-".to_owned()]));
    }

    #[test]
    fn unknown_options_and_missing_values_are_refused() {
        let (result, _, errors) = given_to(&commands::HEAD, &[c"-x"]);
        assert_eq!(result, Err(Refused::Reported));
        assert_eq!(errors, "t: invalid option -- 'x'\n");
        let (_, _, errors) = given_to(&commands::HEAD, &[c"-n"]);
        assert_eq!(errors, "t: option requires an argument -- 'n'\n");
        let (_, _, errors) = given_to(&commands::HEAD, &[c"--long"]);
        assert_eq!(errors, "t: unrecognized option '--long'\n");
    }

    #[test]
    fn an_optional_value_is_taken_only_attached() {
        let (operands, seen, errors) =
            given_to(&commands::DATE, &[c"--sync", c"+%s", c"--sync=10.0.2.2:123"]);
        assert_eq!(operands, Ok(vec!["+%s".to_owned()]));
        assert_eq!(seen, ["sync=", "sync=10.0.2.2:123"]);
        assert_eq!(errors, "");
    }

    #[test]
    fn negative_numbers_are_operands_only_where_the_registry_takes_them() {
        let (operands, _, _) = given_to(&commands::WM_MOVE, &[c"1", c"-20", c"-3"]);
        assert_eq!(
            operands,
            Ok(vec!["1".to_owned(), "-20".to_owned(), "-3".to_owned()])
        );
        let (_, _, errors) = given_to(&commands::WM_MOVE, &[c"1", c"-2x", c"3"]);
        assert_eq!(errors, "t: invalid option -- '2'\n");
        let (result, _, errors) = given_to(&commands::HEAD, &[c"-5"]);
        assert_eq!(result, Err(Refused::Reported));
        assert_eq!(errors, "t: invalid option -- '5'\n");
    }

    #[test]
    fn a_value_named_with_two_words_takes_two() {
        let mut output = Output::new(Fake::default(), b"t");
        let mut points = Vec::new();
        let arguments = [c"129", c"--at", c"10", c"20", c"0", c"-3"];
        let parsed = parse_defined(&mut output, &arguments, &commands::WM_SCROLL, |_, given| {
            points.push((given.value, given.second_value()));
            Ok(())
        });
        let operands: Vec<&CStr> = parsed.unwrap().all().to_vec();
        assert_eq!(operands, [c"129", c"0", c"-3"]);
        assert_eq!(points, [(Some(&b"10"[..]), Some(&b"20"[..]))]);
        let (result, _, errors) =
            given_to(&commands::WM_SCROLL, &[c"1", c"0", c"0", c"--at", c"5"]);
        assert_eq!(result, Err(Refused::Reported));
        assert_eq!(errors, "t: option '--at' requires an argument\n");
    }

    #[test]
    fn values_keep_their_terminating_nul() {
        let mut output = Output::new(Fake::default(), b"t");
        let mut paths = Vec::new();
        let arguments = [c"--reference=/a", c"-r", c"/b", c"-r/c"];
        let parsed = parse_defined(&mut output, &arguments, &commands::DATE, |_, given| {
            paths.push(given.value_as_c_string().map(CStr::to_owned));
            Ok(())
        });
        assert!(parsed.is_ok());
        assert_eq!(
            paths,
            [
                Some(c"/a".to_owned()),
                Some(c"/b".to_owned()),
                Some(c"/c".to_owned())
            ]
        );
    }

    #[test]
    fn counts_are_decimal_digits_only() {
        assert_eq!(parse_count(b"10"), Some(10));
        assert_eq!(parse_count(b"0"), Some(0));
        for bad in [&b""[..], b"-1", b"1x", b"99999999999999999999"] {
            assert_eq!(parse_count(bad), None);
        }
    }

    #[test]
    fn defined_options_take_long_names_with_values_attached_or_next() {
        let (operands, seen, errors) =
            given(&[c"-n3", c"--raw", c"x", c"-n", c"4", c"--", c"--raw"]);
        assert_eq!(operands, Ok(vec!["x".to_owned(), "--raw".to_owned()]));
        assert_eq!(seen, ["n=3", "raw=", "n=4"]);
        assert_eq!(errors, "");
        let (_, seen, _) = given(&[c"--json"]);
        assert_eq!(seen, ["json="]);
    }

    #[test]
    fn defined_options_refuse_unknown_names_and_values_where_none_belong() {
        let (result, _, errors) = given(&[c"--long"]);
        assert_eq!(result, Err(Refused::Reported));
        assert_eq!(errors, "t: unrecognized option '--long'\n");
        let (_, _, errors) = given(&[c"--raw=yes"]);
        assert_eq!(errors, "t: option '--raw' doesn't allow an argument\n");
        let (_, _, errors) = given(&[c"-n"]);
        assert_eq!(errors, "t: option requires an argument -- 'n'\n");
        let (_, _, errors) = given(&[c"--json", c"-x"]);
        assert!(errors.starts_with("{\"error\":{\"code\":\"usage\",\"category\":\"usage\""));
        assert!(wants_json(&commands::LS, &[c"-l", c"--json"]));
        assert!(!wants_json(&commands::LS, &[c"--", c"--json"]));
        assert!(!wants_json(&commands::CAT, &[c"--json"]));
    }
}
