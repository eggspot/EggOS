//! NSPasteboard's rules that need no server (`docs/ux/macos-gui-v0.3.md` section 14.1, D232):
//! which type names are plain text, the longest text one copy holds, and the names of private
//! pasteboards made by `pasteboardWithUniqueName`.

/// The most text one copy holds, as the window server keeps it (D197).
pub const MAX_TEXT_BYTES: usize = 1 << 20;

const UNIQUE_PREFIX: &[u8; 17] = b"EggOS pasteboard ";
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// The length of a unique pasteboard's name.
pub const UNIQUE_NAME_BYTES: usize = UNIQUE_PREFIX.len() + 16;

/// Whether `kind` names plain text: `NSPasteboardTypeString` (`public.utf8-plain-text`) or the
/// name it had before macOS 10.6, `NSStringPboardType`.
pub fn is_text_type(kind: &str) -> bool {
    kind == "public.utf8-plain-text" || kind == "NSStringPboardType"
}

/// The name of the `number`th pasteboard `pasteboardWithUniqueName` makes: a fixed prefix and
/// the number in 16 hexadecimal digits.
pub fn unique_name(number: u64) -> [u8; UNIQUE_NAME_BYTES] {
    let mut name = [0u8; UNIQUE_NAME_BYTES];
    name[..UNIQUE_PREFIX.len()].copy_from_slice(UNIQUE_PREFIX);
    for (index, byte) in name[UNIQUE_PREFIX.len()..].iter_mut().enumerate() {
        let nibble = (number >> (60 - 4 * index)) & 0xf;
        *byte = HEX_DIGITS[nibble as usize];
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_has_two_type_names() {
        assert!(is_text_type("public.utf8-plain-text"));
        assert!(is_text_type("NSStringPboardType"));
        assert!(!is_text_type("public.png"));
        assert!(!is_text_type("public.plain-text"));
        assert!(!is_text_type(""));
    }

    #[test]
    fn unique_names_differ_and_are_readable() {
        assert_eq!(&unique_name(0), b"EggOS pasteboard 0000000000000000");
        assert_eq!(&unique_name(0x1f), b"EggOS pasteboard 000000000000001f");
        assert_ne!(unique_name(1), unique_name(2));
    }

    #[test]
    fn a_copy_holds_one_mebibyte() {
        assert_eq!(MAX_TEXT_BYTES, 1_048_576);
    }
}
