//! Shell arithmetic (POSIX 2.6.4, D572): `$((expression))` and `let`. Signed 64-bit integers
//! written in decimal, octal (`0` first) or hexadecimal (`0x`); variables by name, their
//! values read as such integers (unset or empty is 0); C's operators with C's precedence:
//! unary `+ - ! ~`, `* / %`, `+ -`, `<< >>`, `< <= > >=`, `== !=`, `&`, `^`, `|`, `&&`, `||`,
//! `?:`, the assignments `= *= /= %= += -= <<= >>= &= ^= |=`, and `,`. Overflow wraps, as in
//! the shells; division by zero is an error.

/// Where an expression reads and writes its variables.
pub trait Variables {
    /// The value of variable `name`, as text; `None` when unset.
    fn get(&self, name: &[u8]) -> Option<&[u8]>;
    /// Sets variable `name` to `value` (decimal text): whether it could.
    fn set(&mut self, name: &[u8], value: &[u8]) -> bool;
}

/// Why an expression could not be worked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Something that is no expression, or an unknown operator.
    Syntax,
    /// A division or remainder by zero.
    DivisionByZero,
    /// A variable's value is not a number.
    NotANumber,
    /// A variable could not be set.
    Assignment,
    /// Nested too deeply.
    TooDeep,
}

impl Error {
    /// What the shell says about it.
    pub fn message(self) -> &'static [u8] {
        match self {
            Error::Syntax => b"arithmetic syntax error",
            Error::DivisionByZero => b"division by zero",
            Error::NotANumber => b"a variable's value is not a number",
            Error::Assignment => b"a variable could not be set",
            Error::TooDeep => b"expression nested too deeply",
        }
    }
}

const MAX_DEPTH: usize = 64;
const MAX_NAME: usize = 64;

/// Works `expression` out with `variables`.
pub fn evaluate(expression: &[u8], variables: &mut dyn Variables) -> Result<i64, Error> {
    let mut parser = Parser {
        input: expression,
        at: 0,
        variables,
        depth: 0,
        skipping: 0,
    };
    let value = parser.comma()?;
    parser.space();
    match parser.at == parser.input.len() {
        true => Ok(value),
        false => Err(Error::Syntax),
    }
}

/// Reads `text` as a shell integer: optional sign, decimal, `0` octal or `0x` hexadecimal;
/// empty is 0.
pub fn parse_number(text: &[u8]) -> Option<i64> {
    let text = trim(text);
    if text.is_empty() {
        return Some(0);
    }
    let (negative, digits) = match text[0] {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let (radix, digits) = match digits {
        [b'0', b'x' | b'X', rest @ ..] => (16, rest),
        [b'0', rest @ ..] if !rest.is_empty() => (8, rest),
        _ => (10, digits),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: i64 = 0;
    for &digit in digits {
        let number = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            b'A'..=b'F' => digit - b'A' + 10,
            _ => return None,
        };
        if u32::from(number) >= radix {
            return None;
        }
        value = value.wrapping_mul(i64::from(radix)).wrapping_add(i64::from(number));
    }
    Some(if negative { value.wrapping_neg() } else { value })
}

fn trim(text: &[u8]) -> &[u8] {
    let start = text.iter().position(|byte| !byte.is_ascii_whitespace()).unwrap_or(text.len());
    let end = text.iter().rposition(|byte| !byte.is_ascii_whitespace()).map_or(start, |at| at + 1);
    &text[start..end]
}

/// Writes `value` in decimal into `out`: the digits used.
pub fn decimal(value: i64, out: &mut [u8; 20]) -> &[u8] {
    let mut at = out.len();
    let mut magnitude = value.unsigned_abs();
    loop {
        at -= 1;
        out[at] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if value < 0 {
        at -= 1;
        out[at] = b'-';
    }
    &out[at..]
}

struct Parser<'a, 'v> {
    input: &'a [u8],
    at: usize,
    variables: &'v mut dyn Variables,
    depth: usize,
    skipping: usize,
}

const ASSIGNMENTS: [&[u8]; 11] = [
    b"<<=", b">>=", b"*=", b"/=", b"%=", b"+=", b"-=", b"&=", b"^=", b"|=", b"=",
];

impl Parser<'_, '_> {
    fn space(&mut self) {
        while self.input.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn peek(&mut self, token: &[u8]) -> bool {
        self.space();
        self.input[self.at..].starts_with(token)
    }

    fn take(&mut self, token: &[u8]) -> bool {
        if self.peek(token) {
            self.at += token.len();
            return true;
        }
        false
    }

    fn deeper(&mut self) -> Result<(), Error> {
        self.depth += 1;
        match self.depth > MAX_DEPTH {
            true => Err(Error::TooDeep),
            false => Ok(()),
        }
    }

    fn comma(&mut self) -> Result<i64, Error> {
        let mut value = self.assignment()?;
        while self.take(b",") {
            value = self.assignment()?;
        }
        Ok(value)
    }

    fn assignment(&mut self) -> Result<i64, Error> {
        self.space();
        let start = self.at;
        if let Some(name) = self.name() {
            let mut buffer = [0u8; MAX_NAME];
            let length = name.len().min(MAX_NAME);
            buffer[..length].copy_from_slice(&name[..length]);
            self.space();
            let operator = ASSIGNMENTS
                .iter()
                .find(|operator| {
                    self.input[self.at..].starts_with(operator)
                        && !(operator == &&b"="[..] && self.input[self.at..].starts_with(b"=="))
                })
                .copied();
            if let Some(operator) = operator {
                self.at += operator.len();
                let right = self.assignment()?;
                let name = &buffer[..length];
                let value = match operator {
                    b"=" => right,
                    _ => {
                        let left = self.variable(name)?;
                        apply(&operator[..operator.len() - 1], left, right)?
                    }
                };
                if self.skipping == 0 {
                    let mut digits = [0u8; 20];
                    if !self.variables.set(name, decimal(value, &mut digits)) {
                        return Err(Error::Assignment);
                    }
                }
                return Ok(value);
            }
            self.at = start;
        }
        self.conditional()
    }

    fn conditional(&mut self) -> Result<i64, Error> {
        let condition = self.binary(0)?;
        if !self.take(b"?") {
            return Ok(condition);
        }
        self.deeper()?;
        let skip_first = condition == 0;
        self.skipping += usize::from(skip_first);
        let first = self.comma()?;
        self.skipping -= usize::from(skip_first);
        if !self.take(b":") {
            return Err(Error::Syntax);
        }
        self.skipping += usize::from(!skip_first);
        let second = self.conditional()?;
        self.skipping -= usize::from(!skip_first);
        self.depth -= 1;
        Ok(if skip_first { second } else { first })
    }

    fn binary(&mut self, level: usize) -> Result<i64, Error> {
        const LEVELS: [&[&[u8]]; 10] = [
            &[b"||"],
            &[b"&&"],
            &[b"|"],
            &[b"^"],
            &[b"&"],
            &[b"==", b"!="],
            &[b"<=", b">=", b"<", b">"],
            &[b"<<", b">>"],
            &[b"+", b"-"],
            &[b"*", b"/", b"%"],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        loop {
            self.space();
            let rest = &self.input[self.at..];
            let operator = LEVELS[level].iter().copied().find(|operator| {
                rest.starts_with(operator)
                    && !rest[operator.len()..].starts_with(b"=")
                    && !(operator == &&b"|"[..] && rest.starts_with(b"||"))
                    && !(operator == &&b"&"[..] && rest.starts_with(b"&&"))
                    && !(operator.len() == 1 && (rest.starts_with(b"<<") || rest.starts_with(b">>")) && level == 6)
            });
            let Some(operator) = operator else {
                return Ok(left);
            };
            self.at += operator.len();
            let short = match operator {
                b"&&" => left == 0,
                b"||" => left != 0,
                _ => false,
            };
            self.skipping += usize::from(short);
            let right = self.binary(level + 1);
            self.skipping -= usize::from(short);
            let right = right?;
            left = match operator {
                b"&&" => i64::from(left != 0 && right != 0),
                b"||" => i64::from(left != 0 || right != 0),
                _ => match self.skipping > 0 {
                    true => apply(operator, left, right).unwrap_or(0),
                    false => apply(operator, left, right)?,
                },
            };
        }
    }

    fn unary(&mut self) -> Result<i64, Error> {
        self.space();
        let value = match self.input.get(self.at) {
            Some(b'+') if !self.input[self.at..].starts_with(b"++") => {
                self.at += 1;
                self.deeper()?;
                let value = self.unary()?;
                self.depth -= 1;
                value
            }
            Some(b'-') if !self.input[self.at..].starts_with(b"--") => {
                self.at += 1;
                self.deeper()?;
                let value = self.unary()?.wrapping_neg();
                self.depth -= 1;
                value
            }
            Some(b'!') => {
                self.at += 1;
                self.deeper()?;
                let value = i64::from(self.unary()? == 0);
                self.depth -= 1;
                value
            }
            Some(b'~') => {
                self.at += 1;
                self.deeper()?;
                let value = !self.unary()?;
                self.depth -= 1;
                value
            }
            _ => self.primary()?,
        };
        Ok(value)
    }

    fn primary(&mut self) -> Result<i64, Error> {
        self.space();
        if self.take(b"(") {
            self.deeper()?;
            let value = self.comma()?;
            self.depth -= 1;
            return match self.take(b")") {
                true => Ok(value),
                false => Err(Error::Syntax),
            };
        }
        if let Some(name) = self.name() {
            let mut buffer = [0u8; MAX_NAME];
            let length = name.len().min(MAX_NAME);
            buffer[..length].copy_from_slice(&name[..length]);
            return self.variable(&buffer[..length]);
        }
        let start = self.at;
        while self
            .input
            .get(self.at)
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            self.at += 1;
        }
        if start == self.at {
            return Err(Error::Syntax);
        }
        parse_number(&self.input[start..self.at]).ok_or(Error::Syntax)
    }

    fn name(&mut self) -> Option<&[u8]> {
        self.space();
        let start = self.at;
        let first = *self.input.get(start)?;
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return None;
        }
        let length = self.input[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
            .count();
        self.at = start + length;
        Some(&self.input[start..start + length])
    }

    fn variable(&mut self, name: &[u8]) -> Result<i64, Error> {
        match self.variables.get(name) {
            None => Ok(0),
            Some(value) => parse_number(value).ok_or(Error::NotANumber),
        }
    }
}

fn apply(operator: &[u8], left: i64, right: i64) -> Result<i64, Error> {
    Ok(match operator {
        b"*" => left.wrapping_mul(right),
        b"/" if right == 0 => return Err(Error::DivisionByZero),
        b"/" => left.wrapping_div(right),
        b"%" if right == 0 => return Err(Error::DivisionByZero),
        b"%" => left.wrapping_rem(right),
        b"+" => left.wrapping_add(right),
        b"-" => left.wrapping_sub(right),
        b"<<" => left.wrapping_shl(right as u32),
        b">>" => left.wrapping_shr(right as u32),
        b"<" => i64::from(left < right),
        b"<=" => i64::from(left <= right),
        b">" => i64::from(left > right),
        b">=" => i64::from(left >= right),
        b"==" => i64::from(left == right),
        b"!=" => i64::from(left != right),
        b"&" => left & right,
        b"^" => left ^ right,
        b"|" => left | right,
        _ => return Err(Error::Syntax),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Map(BTreeMap<Vec<u8>, Vec<u8>>);

    impl Variables for Map {
        fn get(&self, name: &[u8]) -> Option<&[u8]> {
            self.0.get(name).map(Vec::as_slice)
        }
        fn set(&mut self, name: &[u8], value: &[u8]) -> bool {
            self.0.insert(name.to_vec(), value.to_vec());
            true
        }
    }

    #[test]
    fn expressions_follow_c_precedence() {
        let mut map = Map::default();
        assert_eq!(evaluate(b"1 + 2 * 3", &mut map), Ok(7));
        assert_eq!(evaluate(b"(1 + 2) * 3", &mut map), Ok(9));
        assert_eq!(evaluate(b"-2 ** 0", &mut map), Err(Error::Syntax));
        assert_eq!(evaluate(b"0x10 + 010", &mut map), Ok(24));
        assert_eq!(evaluate(b"1 < 2 && 3 > 4 || 5", &mut map), Ok(1));
        assert_eq!(evaluate(b"7 % 0", &mut map), Err(Error::DivisionByZero));
        assert_eq!(evaluate(b"1 ? 2 : 3 / 0", &mut map), Ok(2));
        assert_eq!(evaluate(b"1 << 4 >> 2", &mut map), Ok(4));
    }

    #[test]
    fn assignments_set_variables() {
        let mut map = Map::default();
        assert_eq!(evaluate(b"x = 5, x += 2, x * 2", &mut map), Ok(14));
        assert_eq!(map.get(b"x"), Some(&b"7"[..]));
        assert_eq!(evaluate(b"y", &mut map), Ok(0));
        assert_eq!(evaluate(b"x == 7", &mut map), Ok(1));
    }
}
