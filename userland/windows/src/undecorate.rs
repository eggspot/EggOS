//! The names of C++ types as `type_info::name()` gives them on Windows (D230), from the
//! decorated names MSVC writes into type descriptors (`.?AVWidget@@` is `class Widget`, `.H` is
//! `int`, `.PEBD` is `char const * __ptr64`), following Microsoft's public description of its
//! name decoration: built-in types by letter, pointers and references with their qualifiers,
//! classes, structures, unions and enumerations by their qualified names (innermost first, each
//! part ending in `@`, the list in another `@`), templates (`?$name@arguments@`) with type and
//! integer arguments, function pointers, the anonymous namespace, and back references (`0` to
//! `9`) to the names and argument types already read, each template argument list counting its
//! own. Pure logic with host tests.

use alloc::vec::Vec;

const MAX_BACK_REFERENCES: usize = 10;
const MAX_DEPTH: usize = 32;
const MAX_PARTS: usize = 64;
const ANONYMOUS_NAMESPACE: &[u8] = b"`anonymous namespace'";

/// The name of the type whose decorated name (a type descriptor's, with its leading `.`) is
/// `decorated`, as MSVC's `type_info::name()` gives it, or none for an encoding this
/// undecorator does not read.
pub fn type_name(decorated: &[u8]) -> Option<Vec<u8>> {
    let mut encoded = decorated.strip_prefix(b".")?;
    if let Some(rest) = encoded.strip_prefix(b"?A") {
        encoded = rest;
    }
    let mut parser = Parser {
        input: encoded,
        at: 0,
        names: Vec::new(),
        types: Vec::new(),
        depth: 0,
    };
    let name = parser.type_name()?;
    (parser.at == encoded.len()).then_some(name)
}

struct Parser<'a> {
    input: &'a [u8],
    at: usize,
    names: Vec<Vec<u8>>,
    types: Vec<Vec<u8>>,
    depth: usize,
}

fn back_reference(code: u8) -> Option<usize> {
    code.is_ascii_digit().then(|| usize::from(code - b'0'))
}

fn push_decimal(out: &mut Vec<u8>, value: i64) {
    if value < 0 {
        out.push(b'-');
    }
    let mut digits = [0u8; 20];
    let mut magnitude = value.unsigned_abs();
    let mut count = 0;
    loop {
        digits[count] = b'0' + (magnitude % 10) as u8;
        count += 1;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    out.extend(digits[..count].iter().rev());
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn eat(&mut self, prefix: &[u8]) -> bool {
        let found = self.input[self.at..].starts_with(prefix);
        if found {
            self.at += prefix.len();
        }
        found
    }

    fn remember_name(&mut self, name: &[u8]) {
        if self.names.len() < MAX_BACK_REFERENCES {
            self.names.push(name.to_vec());
        }
    }

    fn type_name(&mut self) -> Option<Vec<u8>> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let name = self.type_name_at_depth();
        self.depth -= 1;
        name
    }

    fn type_name_at_depth(&mut self) -> Option<Vec<u8>> {
        let code = self.next()?;
        let simple: &[u8] = match code {
            b'C' => b"signed char",
            b'D' => b"char",
            b'E' => b"unsigned char",
            b'F' => b"short",
            b'G' => b"unsigned short",
            b'H' => b"int",
            b'I' => b"unsigned int",
            b'J' => b"long",
            b'K' => b"unsigned long",
            b'M' => b"float",
            b'N' => b"double",
            b'O' => b"long double",
            b'X' => b"void",
            b'_' => match self.next()? {
                b'N' => b"bool",
                b'J' => b"__int64",
                b'K' => b"unsigned __int64",
                b'W' => b"wchar_t",
                b'S' => b"char16_t",
                b'U' => b"char32_t",
                b'Q' => b"char8_t",
                _ => return None,
            },
            b'V' => return self.class(b"class "),
            b'U' => return self.class(b"struct "),
            b'T' => return self.class(b"union "),
            b'W' => {
                return match self.next()? {
                    b'4' => self.class(b"enum "),
                    _ => None,
                };
            }
            b'P' => return self.pointer(b"*", b""),
            b'Q' => return self.pointer(b"*", b" const"),
            b'R' => return self.pointer(b"*", b" volatile"),
            b'S' => return self.pointer(b"*", b" const volatile"),
            b'A' => return self.pointer(b"&", b""),
            b'$' if self.eat(b"$Q") => return self.pointer(b"&&", b""),
            b'$' if self.eat(b"$T") => b"std::nullptr_t",
            _ => return back_reference(code).and_then(|index| self.types.get(index).cloned()),
        };
        Some(simple.to_vec())
    }

    fn pointer(&mut self, symbol: &[u8], own_qualifiers: &[u8]) -> Option<Vec<u8>> {
        if self.eat(b"6") {
            return self.function_pointer(symbol, own_qualifiers);
        }
        let mut storage = Vec::new();
        loop {
            match self.peek()? {
                b'E' => storage.extend_from_slice(b" __ptr64"),
                b'I' => storage.extend_from_slice(b" __restrict"),
                _ => break,
            }
            self.at += 1;
        }
        let pointee_qualifiers: &[u8] = match self.next()? {
            b'A' => b"",
            b'B' => b" const",
            b'C' => b" volatile",
            b'D' => b" const volatile",
            _ => return None,
        };
        let mut out = self.type_name()?;
        out.extend_from_slice(pointee_qualifiers);
        out.push(b' ');
        out.extend_from_slice(symbol);
        out.extend_from_slice(&storage);
        out.extend_from_slice(own_qualifiers);
        Some(out)
    }

    fn function_pointer(&mut self, symbol: &[u8], own_qualifiers: &[u8]) -> Option<Vec<u8>> {
        let convention: &[u8] = match self.next()? {
            b'A' | b'B' => b"__cdecl",
            b'C' | b'D' => b"__pascal",
            b'E' | b'F' => b"__thiscall",
            b'G' | b'H' => b"__stdcall",
            b'I' | b'J' => b"__fastcall",
            b'Q' => b"__vectorcall",
            _ => return None,
        };
        let returned = match self.eat(b"?A") {
            true => self.type_name()?,
            false => match self.eat(b"?B") {
                true => {
                    let mut constant = self.type_name()?;
                    constant.extend_from_slice(b" const");
                    constant
                }
                false => self.type_name()?,
            },
        };
        let mut arguments: Vec<Vec<u8>> = Vec::new();
        if self.eat(b"X") {
            arguments.push(b"void".to_vec());
        } else {
            loop {
                if self.eat(b"@") {
                    break;
                }
                if self.eat(b"Z") {
                    arguments.push(b"...".to_vec());
                    self.at -= 1;
                    break;
                }
                if arguments.len() >= MAX_PARTS {
                    return None;
                }
                arguments.push(self.argument()?);
            }
        }
        if !self.eat(b"Z") {
            return None;
        }
        let mut out = returned;
        out.extend_from_slice(b" (");
        out.extend_from_slice(convention);
        out.extend_from_slice(symbol);
        out.extend_from_slice(own_qualifiers);
        out.extend_from_slice(b")(");
        out.extend_from_slice(&arguments.join(&b","[..]));
        out.push(b')');
        Some(out)
    }

    fn argument(&mut self) -> Option<Vec<u8>> {
        let start = self.at;
        let argument = self.type_name()?;
        if self.at - start > 1 && self.types.len() < MAX_BACK_REFERENCES {
            self.types.push(argument.clone());
        }
        Some(argument)
    }

    fn class(&mut self, keyword: &[u8]) -> Option<Vec<u8>> {
        let mut parts: Vec<Vec<u8>> = Vec::new();
        while !self.eat(b"@") {
            if parts.len() >= MAX_PARTS {
                return None;
            }
            parts.push(self.name_part()?);
        }
        if parts.is_empty() {
            return None;
        }
        parts.reverse();
        let mut out = keyword.to_vec();
        out.extend_from_slice(&parts.join(&b"::"[..]));
        Some(out)
    }

    fn simple_name(&mut self) -> Option<Vec<u8>> {
        let length = self.input[self.at..].iter().position(|byte| *byte == b'@')?;
        let name = &self.input[self.at..self.at + length];
        if name.is_empty()
            || !name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
        {
            return None;
        }
        self.at += length + 1;
        Some(name.to_vec())
    }

    fn name_part(&mut self) -> Option<Vec<u8>> {
        if let Some(index) = self.peek().and_then(back_reference) {
            self.at += 1;
            return self.names.get(index).cloned();
        }
        let part = if self.eat(b"?$") {
            self.template()?
        } else if self.eat(b"?A") {
            let length = self.input[self.at..].iter().position(|byte| *byte == b'@')?;
            self.at += length + 1;
            ANONYMOUS_NAMESPACE.to_vec()
        } else {
            self.simple_name()?
        };
        self.remember_name(&part);
        Some(part)
    }

    fn template(&mut self) -> Option<Vec<u8>> {
        let names = core::mem::take(&mut self.names);
        let types = core::mem::take(&mut self.types);
        let template = self.template_with_own_references();
        self.names = names;
        self.types = types;
        template
    }

    fn template_with_own_references(&mut self) -> Option<Vec<u8>> {
        let mut out = self.simple_name()?;
        self.remember_name(&out);
        let mut arguments: Vec<Vec<u8>> = Vec::new();
        while !self.eat(b"@") {
            if arguments.len() >= MAX_PARTS {
                return None;
            }
            let argument = match self.eat(b"$0") {
                true => {
                    let mut number = Vec::new();
                    push_decimal(&mut number, self.number()?);
                    number
                }
                false => self.argument()?,
            };
            arguments.push(argument);
        }
        out.push(b'<');
        out.extend_from_slice(&arguments.join(&b","[..]));
        if out.last() == Some(&b'>') {
            out.push(b' ');
        }
        out.push(b'>');
        Some(out)
    }

    fn number(&mut self) -> Option<i64> {
        let negative = self.eat(b"?");
        let first = self.next()?;
        let magnitude = match first {
            b'0'..=b'9' => i64::from(first - b'0') + 1,
            _ => {
                let mut value: i64 = 0;
                let mut digit = first;
                while digit != b'@' {
                    if !(b'A'..=b'P').contains(&digit) {
                        return None;
                    }
                    value = value.checked_mul(16)?.checked_add(i64::from(digit - b'A'))?;
                    digit = self.next()?;
                }
                value
            }
        };
        Some(match negative {
            true => -magnitude,
            false => magnitude,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::type_name;

    fn name(decorated: &str) -> Option<String> {
        type_name(decorated.as_bytes()).map(|name| String::from_utf8(name).unwrap())
    }

    #[test]
    fn built_in_types_are_named_by_letter() {
        assert_eq!(name(".H").as_deref(), Some("int"));
        assert_eq!(name(".N").as_deref(), Some("double"));
        assert_eq!(name("._N").as_deref(), Some("bool"));
        assert_eq!(name("._K").as_deref(), Some("unsigned __int64"));
        assert_eq!(name("._W").as_deref(), Some("wchar_t"));
        assert_eq!(name(".$$T").as_deref(), Some("std::nullptr_t"));
    }

    #[test]
    fn classes_structures_unions_and_enumerations_keep_their_keyword_and_scopes() {
        assert_eq!(name(".?AVWidget@@").as_deref(), Some("class Widget"));
        assert_eq!(
            name(".?AUPoint@geometry@@").as_deref(),
            Some("struct geometry::Point")
        );
        assert_eq!(name(".?ATValue@@").as_deref(), Some("union Value"));
        assert_eq!(name(".?AW4Color@ui@@").as_deref(), Some("enum ui::Color"));
        assert_eq!(
            name(".?AVinner@?A0x1a2b3c4d@@").as_deref(),
            Some("class `anonymous namespace'::inner")
        );
        assert_eq!(
            name(".?AVbad_cast@std@@").as_deref(),
            Some("class std::bad_cast")
        );
    }

    #[test]
    fn pointers_and_references_carry_their_qualifiers_and_ptr64() {
        assert_eq!(name(".PEAH").as_deref(), Some("int * __ptr64"));
        assert_eq!(name(".PEBD").as_deref(), Some("char const * __ptr64"));
        assert_eq!(
            name(".PEAVWidget@@").as_deref(),
            Some("class Widget * __ptr64")
        );
        assert_eq!(
            name(".PEAPEBD").as_deref(),
            Some("char const * __ptr64 * __ptr64")
        );
        assert_eq!(
            name(".QEBD").as_deref(),
            Some("char const * __ptr64 const")
        );
        assert_eq!(name(".AEAH").as_deref(), Some("int & __ptr64"));
        assert_eq!(name(".$$QEAH").as_deref(), Some("int && __ptr64"));
    }

    #[test]
    fn templates_list_their_arguments_with_their_own_back_references() {
        assert_eq!(
            name(".?AV?$vector@HV?$allocator@H@std@@@std@@").as_deref(),
            Some("class std::vector<int,class std::allocator<int> >")
        );
        assert_eq!(
            name(".?AV?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@").as_deref(),
            Some(
                "class std::basic_string<char,struct std::char_traits<char>,class std::allocator<char> >"
            )
        );
        assert_eq!(
            name(".?AV?$pair@PEAVWidget@@PEAV1@@@").as_deref(),
            Some("class pair<class Widget * __ptr64,class Widget * __ptr64>")
        );
        assert_eq!(
            name(".?AV?$pair@PEAVWidget@@0@@").as_deref(),
            Some("class pair<class Widget * __ptr64,class Widget * __ptr64>")
        );
    }

    #[test]
    fn integer_template_arguments_are_decimal() {
        assert_eq!(
            name(".?AV?$array@H$04@std@@").as_deref(),
            Some("class std::array<int,5>")
        );
        assert_eq!(name(".?AV?$Box@$0BA@@@").as_deref(), Some("class Box<16>"));
        assert_eq!(name(".?AV?$Box@$0A@@@").as_deref(), Some("class Box<0>"));
        assert_eq!(name(".?AV?$Box@$0?0@@").as_deref(), Some("class Box<-1>"));
    }

    #[test]
    fn function_pointers_name_their_convention_and_arguments() {
        assert_eq!(name(".P6AHH@Z").as_deref(), Some("int (__cdecl*)(int)"));
        assert_eq!(name(".P6AXXZ").as_deref(), Some("void (__cdecl*)(void)"));
        assert_eq!(
            name(".P6AXPEBDZZ").as_deref(),
            Some("void (__cdecl*)(char const * __ptr64,...)")
        );
    }

    #[test]
    fn encodings_it_does_not_read_give_nothing() {
        assert_eq!(name("H"), None);
        assert_eq!(name(".?AVunterminated"), None);
        assert_eq!(name(".?AVWidget@@trailing"), None);
        assert_eq!(name(".PEAY01H"), None);
        assert_eq!(name(".9"), None);
        let deep = format!(".{}H", "PEA".repeat(40));
        assert_eq!(name(&deep), None);
    }
}
