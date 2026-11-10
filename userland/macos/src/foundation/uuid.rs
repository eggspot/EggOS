//! NSUUID (D232): sixteen bytes, made random as version 4 (RFC 9562, the variant bits `10`)
//! from the layer's `arc4random_buf`, or read from the 36-character text form
//! (`8-4-4-4-12` hexadecimal digits, either case), and written as macOS writes it, in capitals.

/// The length of a UUID's text form.
pub const TEXT_LENGTH: usize = 36;

const DASHES: [usize; 4] = [8, 13, 18, 23];
const DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Turns sixteen random bytes into a version 4 UUID.
pub fn version_4(mut bytes: [u8; 16]) -> [u8; 16] {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

/// The text form of `bytes`, in capitals.
pub fn format(bytes: &[u8; 16]) -> [u8; TEXT_LENGTH] {
    let mut text = [b'-'; TEXT_LENGTH];
    let mut at = 0;
    for byte in bytes {
        if DASHES.contains(&at) {
            at += 1;
        }
        text[at] = DIGITS[usize::from(byte >> 4)];
        text[at + 1] = DIGITS[usize::from(byte & 0x0f)];
        at += 2;
    }
    text
}

fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The bytes of the text form `text`, or `None` if it is not exactly one.
pub fn parse(text: &[u8]) -> Option<[u8; 16]> {
    if text.len() != TEXT_LENGTH {
        return None;
    }
    let mut bytes = [0u8; 16];
    let mut at = 0;
    for byte in &mut bytes {
        if DASHES.contains(&at) {
            if text[at] != b'-' {
                return None;
            }
            at += 1;
        }
        *byte = (digit(text[at])? << 4) | digit(text[at + 1])?;
        at += 2;
    }
    Some(bytes)
}

#[cfg(eggos_user)]
pub use class::define;

#[cfg(eggos_user)]
mod class {
    use super::super::{
        Definition, NSUUID, NSUUID_META, autoreleased, class_pointer, define as define_class,
        is_string, new_instance, string_from_utf8, with_utf8,
    };
    use super::{TEXT_LENGTH, format, parse, version_4};
    use crate::libsystem::process::arc4random_buf;
    use crate::objc::abi::{Id, Sel};
    use crate::objc::reference_count::{objc_release, objc_retain};
    use crate::objc::root::{self, NSOBJECT};
    use crate::objc::{Runtime, methods};

    #[repr(C)]
    #[derive(Debug)]
    struct UuidObject {
        isa: usize,
        bytes: [u8; 16],
    }

    fn bytes_of(object: Id) -> [u8; 16] {
        // SAFETY: the callers pass NSUUID instances, whose memory starts with these fields.
        unsafe { (*object.cast::<UuidObject>()).bytes }
    }

    fn set_bytes(object: Id, bytes: [u8; 16]) {
        // SAFETY: as above; a UUID being made, which only this thread holds.
        unsafe { (*object.cast::<UuidObject>()).bytes = bytes };
    }

    fn random() -> [u8; 16] {
        let mut bytes = [0u8; 16];
        // SAFETY: the buffer holds sixteen bytes.
        unsafe { arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len()) };
        version_4(bytes)
    }

    fn is_uuid(object: Id) -> bool {
        root::is_kind_of(object, &NSUUID)
    }

    extern "C" fn uuid(_: Id, _: Sel) -> Id {
        let made = new_instance(class_pointer(&NSUUID));
        if !made.is_null() {
            set_bytes(made, random());
        }
        autoreleased(made)
    }

    extern "C" fn init(this: Id, _: Sel) -> Id {
        set_bytes(this, random());
        this
    }

    extern "C" fn init_with_uuid_string(this: Id, _: Sel, text: Id) -> Id {
        let parsed = match text.is_null() || !is_string(text) {
            true => None,
            false => with_utf8(text, parse),
        };
        match parsed {
            Some(bytes) => {
                set_bytes(this, bytes);
                this
            }
            None => {
                objc_release(this);
                core::ptr::null_mut()
            }
        }
    }

    unsafe extern "C" fn init_with_uuid_bytes(this: Id, _: Sel, bytes: *const [u8; 16]) -> Id {
        if !bytes.is_null() {
            // SAFETY: the caller passes sixteen bytes.
            set_bytes(this, unsafe { bytes.read_unaligned() });
        }
        this
    }

    unsafe extern "C" fn get_uuid_bytes(this: Id, _: Sel, out: *mut [u8; 16]) {
        if !out.is_null() {
            // SAFETY: the caller's sixteen-byte buffer.
            unsafe { out.write_unaligned(bytes_of(this)) };
        }
    }

    extern "C" fn uuid_string(this: Id, _: Sel) -> Id {
        let text: [u8; TEXT_LENGTH] = format(&bytes_of(this));
        string_from_utf8(&text)
    }

    extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
        u8::from(this == other || (is_uuid(other) && bytes_of(this) == bytes_of(other)))
    }

    extern "C" fn hash(this: Id, _: Sel) -> usize {
        let bytes = bytes_of(this);
        let mut word = [0u8; 8];
        word.copy_from_slice(&bytes[8..]);
        u64::from_le_bytes(word) as usize
    }

    extern "C" fn compare(this: Id, _: Sel, other: Id) -> isize {
        if !is_uuid(other) {
            return 1;
        }
        match bytes_of(this).cmp(&bytes_of(other)) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        }
    }

    extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
        objc_retain(this)
    }

    pub fn define(runtime: &mut Runtime) -> bool {
        let instance = methods![
            "init" => init,
            "initWithUUIDString:" => init_with_uuid_string,
            "initWithUUIDBytes:" => init_with_uuid_bytes,
            "getUUIDBytes:" => get_uuid_bytes,
            "UUIDString" => uuid_string,
            "description" => uuid_string,
            "isEqual:" => is_equal,
            "hash" => hash,
            "compare:" => compare,
            "copyWithZone:" => copy_with_zone,
        ];
        let class = methods!["UUID" => uuid];
        define_class(
            runtime,
            &Definition {
                name: b"NSUUID\0",
                class: &NSUUID,
                meta: &NSUUID_META,
                superclass: &NSOBJECT,
                instance_size: core::mem::size_of::<UuidObject>(),
                flags: 0,
                instance_methods: &instance,
                class_methods: &class,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips_in_capitals() {
        let bytes = [
            0xe6, 0x21, 0xe1, 0xf8, 0xc3, 0x6c, 0x49, 0x5a, 0x93, 0xfc, 0x0c, 0x24, 0x7a, 0x3e,
            0x6e, 0x5f,
        ];
        let text = format(&bytes);
        assert_eq!(&text, b"E621E1F8-C36C-495A-93FC-0C247A3E6E5F");
        assert_eq!(parse(&text), Some(bytes));
        let lower = parse(b"e621e1f8-c36c-495a-93fc-0c247a3e6e5f");
        assert_eq!(lower, Some(bytes));
    }

    #[test]
    fn malformed_text_is_refused() {
        let good = *b"E621E1F8-C36C-495A-93FC-0C247A3E6E5F";
        assert_eq!(parse(&good[..35]), None);
        assert_eq!(parse(b"E621E1F8-C36C-495A-93FC-0C247A3E6E5F0"), None);
        assert_eq!(parse(b"E621E1F8xC36C-495A-93FC-0C247A3E6E5F"), None);
        assert_eq!(parse(b"G621E1F8-C36C-495A-93FC-0C247A3E6E5F"), None);
        assert_eq!(parse(b"E621E1F8-C36C-495A-93FC0-C247A3E6E5F"), None);
        assert_eq!(parse(b""), None);
        for at in 0..good.len() {
            let mut damaged = good;
            damaged[at] = b' ';
            assert_eq!(parse(&damaged), None, "space at {at}");
        }
    }

    #[test]
    fn version_4_sets_the_version_and_variant() {
        let made = version_4([0xff; 16]);
        assert_eq!(made[6], 0x4f);
        assert_eq!(made[8], 0xbf);
        let made = version_4([0; 16]);
        assert_eq!(made[6], 0x40);
        assert_eq!(made[8], 0x80);
        assert_eq!(format(&made)[14], b'4');
    }
}
