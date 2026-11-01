use crate::options::{Given, Refused, parse_count};
use crate::output::Output;
use crate::system::System;
use eggos_commands::page::{self, MAX_KEY_BYTES};
use eggos_commands::{ValueType, errors};

#[derive(Debug, Clone, Copy)]
pub struct Page {
    pub limit: Option<u64>,
    after: [u8; MAX_KEY_BYTES],
    after_length: Option<usize>,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: None,
            after: [0; MAX_KEY_BYTES],
            after_length: None,
        }
    }
}

impl Page {
    pub fn after(&self) -> Option<&[u8]> {
        self.after_length.map(|length| &self.after[..length])
    }

    pub fn is_paged(&self) -> bool {
        self.limit.is_some() || self.after_length.is_some()
    }

    pub fn after_number<S: System>(&self, output: &mut Output<S>) -> Result<Option<u64>, Refused> {
        let Some(after) = self.after() else {
            return Ok(None);
        };
        match parse_count(after) {
            Some(number) => Ok(Some(number)),
            None => {
                output.coded(&errors::BAD_CURSOR, &[b"invalid cursor '", after, b"'"]);
                Err(Refused::Reported)
            }
        }
    }

    pub fn take<S: System>(
        &mut self,
        output: &mut Output<S>,
        given: Given<'_>,
    ) -> Result<bool, Refused> {
        let value = given.value.unwrap_or_default();
        if given.is_long("limit") {
            let most = match given.option.value.map(|value| value.value) {
                Some(ValueType::Integer { maximum, .. }) => maximum as u64,
                _ => 0,
            };
            match parse_count(value) {
                Some(limit) if (1..=most).contains(&limit) => self.limit = Some(limit),
                _ => {
                    let mut digits = [0u8; 20];
                    output.coded(
                        &errors::USAGE,
                        &[
                            b"invalid limit '",
                            value,
                            b"': a page holds 1 to ",
                            crate::output::decimal(most, &mut digits),
                            b" items",
                        ],
                    );
                    return Err(Refused::Reported);
                }
            }
            return Ok(true);
        }
        if given.is_long("after") {
            match page::decode(value, &mut self.after) {
                Some(length) => self.after_length = Some(length),
                None => {
                    output.coded(
                        &errors::BAD_CURSOR,
                        &[b"invalid cursor '", value, b"': not one a page ended with"],
                    );
                    return Err(Refused::Reported);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use crate::options::parse_defined;
    use core::ffi::CStr;
    use eggos_commands::commands;

    fn parsed(arguments: &[&CStr]) -> (Result<Page, Refused>, String) {
        let mut output = Output::new(Fake::default(), b"ps");
        let mut page = Page::default();
        let result = parse_defined(&mut output, arguments, &commands::PS, |output, given| {
            page.take(output, given).map(|_| ())
        })
        .map(|_| page);
        output.finish(0);
        (result, output.system.errors())
    }

    #[test]
    fn a_page_takes_a_limit_in_its_range_and_a_cursor() {
        let (page, _) = parsed(&[c"--limit", c"2", c"--after=my%20x"]);
        let page = page.unwrap();
        assert_eq!(page.limit, Some(2));
        assert_eq!(page.after(), Some(&b"my x"[..]));
        assert!(page.is_paged());
        assert!(!Page::default().is_paged());
        let (_, errors) = parsed(&[c"--limit", c"101"]);
        assert_eq!(
            errors,
            "ps: invalid limit '101': a page holds 1 to 100 items\n"
        );
        let (_, errors) = parsed(&[c"--limit=0"]);
        assert!(errors.starts_with("ps: invalid limit '0'"), "{errors}");
        let (result, errors) = parsed(&[c"--after", c"-x"]);
        assert_eq!(result.unwrap_err(), Refused::Reported);
        assert_eq!(
            errors,
            "ps: invalid cursor '-x': not one a page ended with\n"
        );
        let (_, errors) = parsed(&[c"--json", c"--after", c"a b"]);
        assert!(errors.contains("\"code\":\"bad_cursor\""), "{errors}");
    }

    #[test]
    fn a_numeric_cursor_is_read_as_a_number() {
        let (page, _) = parsed(&[c"--after", c"129"]);
        let mut output = Output::new(Fake::default(), b"ps");
        assert_eq!(page.unwrap().after_number(&mut output), Ok(Some(129)));
        let (page, _) = parsed(&[c"--after", c"x"]);
        assert_eq!(
            page.unwrap().after_number(&mut output),
            Err(Refused::Reported)
        );
        output.finish(0);
        assert_eq!(output.system.errors(), "ps: invalid cursor 'x'\n");
    }
}
