//! Objective-C type encodings (D741), as method lists and ivars carry them: `q24@0:8d16`
//! names the return type, then each argument with its stack offset. Pure logic with host
//! tests: one type's extent (structures, unions and arrays nested, pointers, quoted class
//! names, blocks, bit fields), the type of a given argument, and what kind of value it is.

const QUALIFIERS: &[u8] = b"rnNoORVA";

/// What a value of an encoded type is, as key-value coding boxes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Object,
    Bool,
    Signed,
    Unsigned,
    Float,
    Double,
    Void,
    Other,
}

fn skip_qualifiers(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && QUALIFIERS.contains(&bytes[at]) {
        at += 1;
    }
    at
}

fn is_offset(byte: u8) -> bool {
    byte.is_ascii_digit() || byte == b'-' || byte == b'+'
}

fn skip_digits(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && is_offset(bytes[at]) {
        at += 1;
    }
    at
}

fn closing(bytes: &[u8], start: usize, open: u8, close: u8) -> usize {
    let mut depth = 0usize;
    let mut at = start;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => {
                at += 1;
                while at < bytes.len() && bytes[at] != b'"' {
                    at += 1;
                }
            }
            byte if byte == open => depth += 1,
            byte if byte == close => {
                depth -= 1;
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
        at += 1;
    }
    bytes.len()
}

/// The end (exclusive) of the one type that starts at `start`, after its qualifiers.
pub fn type_end(bytes: &[u8], start: usize) -> usize {
    let start = skip_qualifiers(bytes, start);
    let Some(&first) = bytes.get(start) else {
        return start;
    };
    match first {
        b'{' => closing(bytes, start, b'{', b'}'),
        b'(' => closing(bytes, start, b'(', b')'),
        b'[' => closing(bytes, start, b'[', b']'),
        b'^' => type_end(bytes, start + 1),
        b'b' => skip_digits(bytes, start + 1),
        b'@' => match bytes.get(start + 1) {
            Some(b'"') => {
                let mut at = start + 2;
                while at < bytes.len() && bytes[at] != b'"' {
                    at += 1;
                }
                (at + 1).min(bytes.len())
            }
            Some(b'?') => match bytes.get(start + 2) {
                Some(b'<') => closing(bytes, start + 2, b'<', b'>'),
                _ => start + 2,
            },
            _ => start + 1,
        },
        _ => start + 1,
    }
}

/// The type of argument `index` of a method's encoding: 0 is the return type, 1 the
/// receiver, 2 the selector, 3 the first declared argument. Qualifiers and offsets are left
/// out.
pub fn argument(types: &[u8], index: usize) -> Option<&[u8]> {
    let mut at = 0;
    for position in 0..=index {
        if at >= types.len() {
            return None;
        }
        let start = skip_qualifiers(types, at);
        let end = type_end(types, start);
        if position == index {
            return Some(&types[start..end]);
        }
        at = skip_digits(types, end);
    }
    None
}

/// What a value of the encoded type `encoding` is.
pub fn kind_of(encoding: &[u8]) -> Kind {
    let start = skip_qualifiers(encoding, 0);
    match encoding.get(start) {
        Some(b'@' | b'#') => Kind::Object,
        Some(b'B') => Kind::Bool,
        Some(b'c' | b's' | b'i' | b'l' | b'q') => Kind::Signed,
        Some(b'C' | b'S' | b'I' | b'L' | b'Q') => Kind::Unsigned,
        Some(b'f') => Kind::Float,
        Some(b'd') => Kind::Double,
        Some(b'v') => Kind::Void,
        _ => Kind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_come_in_order_without_offsets() {
        let types = b"v24@0:8q16";
        assert_eq!(argument(types, 0), Some(&b"v"[..]));
        assert_eq!(argument(types, 1), Some(&b"@"[..]));
        assert_eq!(argument(types, 2), Some(&b":"[..]));
        assert_eq!(argument(types, 3), Some(&b"q"[..]));
        assert_eq!(argument(types, 4), None);
    }

    #[test]
    fn structures_class_names_and_blocks_are_one_type_each() {
        let types = b"{CGRect={CGPoint=dd}{CGSize=dd}}16@0:8";
        assert_eq!(argument(types, 0), Some(&b"{CGRect={CGPoint=dd}{CGSize=dd}}"[..]));
        let named = b"@\"NSString\"16@0:8";
        assert_eq!(argument(named, 0), Some(&b"@\"NSString\""[..]));
        assert_eq!(argument(b"v24@0:8@?16", 3), Some(&b"@?"[..]));
        assert_eq!(argument(b"r^v16@0:8", 0), Some(&b"^v"[..]));
    }

    #[test]
    fn kinds_follow_the_first_letter() {
        assert_eq!(kind_of(b"q"), Kind::Signed);
        assert_eq!(kind_of(b"Q"), Kind::Unsigned);
        assert_eq!(kind_of(b"B"), Kind::Bool);
        assert_eq!(kind_of(b"d"), Kind::Double);
        assert_eq!(kind_of(b"@\"NSString\""), Kind::Object);
        assert_eq!(kind_of(b"{CGPoint=dd}"), Kind::Other);
        assert_eq!(kind_of(b""), Kind::Other);
    }
}
