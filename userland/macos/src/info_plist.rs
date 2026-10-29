//! An `.app` bundle's place and its `Contents/Info.plist` (Apple's XML property list) as far as
//! the layer reads them: the bundle a program inside `X.app/Contents/MacOS/` belongs to, and the
//! string values of the property list's keys (`CFBundleName`, `CFBundleIdentifier`, ...).

const KEY_OPEN: &[u8] = b"<key>";
const KEY_CLOSE: &[u8] = b"</key>";
const STRING_OPEN: &[u8] = b"<string>";
const STRING_CLOSE: &[u8] = b"</string>";
const MACOS_FOLDER: &[u8] = b"/Contents/MacOS/";
const BUNDLE_SUFFIX: &[u8] = b".app";
const ENTITIES: [(&[u8], u8); 5] = [
    (b"&amp;", b'&'),
    (b"&lt;", b'<'),
    (b"&gt;", b'>'),
    (b"&quot;", b'"'),
    (b"&apos;", b'\''),
];

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

/// The bundle folder of the program at `executable`, if it is `<bundle>.app/Contents/MacOS/<name>`.
pub fn bundle_path(executable: &[u8]) -> Option<&[u8]> {
    let mut found = None;
    let mut from = 0;
    while let Some(at) = find(executable, MACOS_FOLDER, from) {
        found = Some(at);
        from = at + 1;
    }
    let at = found?;
    let name = &executable[at + MACOS_FOLDER.len()..];
    let bundle = &executable[..at];
    (!name.is_empty() && !name.contains(&b'/') && bundle.ends_with(BUNDLE_SUFFIX)).then_some(bundle)
}

fn decode<'a>(raw: &[u8], out: &'a mut [u8]) -> Option<&'a [u8]> {
    let mut length = 0;
    let mut at = 0;
    while at < raw.len() {
        let (byte, used) = match raw[at] {
            b'&' => ENTITIES
                .iter()
                .find(|(entity, _)| raw[at..].starts_with(entity))
                .map_or((b'&', 1), |(entity, byte)| (*byte, entity.len())),
            byte => (byte, 1),
        };
        *out.get_mut(length)? = byte;
        length += 1;
        at += used;
    }
    Some(&out[..length])
}

/// Calls `each` with every key whose value is a string, and that value with its XML entities
/// decoded (values longer than `scratch` are skipped).
pub fn for_each_string(xml: &[u8], scratch: &mut [u8], mut each: impl FnMut(&[u8], &[u8])) {
    let mut at = 0;
    while let Some(open) = find(xml, KEY_OPEN, at) {
        let key_start = open + KEY_OPEN.len();
        let Some(key_end) = find(xml, KEY_CLOSE, key_start) else {
            return;
        };
        let key = &xml[key_start..key_end];
        at = key_end + KEY_CLOSE.len();
        let mut value_at = at;
        while xml.get(value_at).is_some_and(u8::is_ascii_whitespace) {
            value_at += 1;
        }
        if !xml[value_at..].starts_with(STRING_OPEN) {
            continue;
        }
        let value_start = value_at + STRING_OPEN.len();
        let Some(value_end) = find(xml, STRING_CLOSE, value_start) else {
            return;
        };
        if let Some(value) = decode(&xml[value_start..value_end], scratch) {
            each(key, value);
        }
        at = value_end + STRING_CLOSE.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLIST: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key>
	<string>mac-paint</string>
	<key>CFBundleName</key>
	<string>Paint &amp; Draw</string>
	<key>LSRequiresIPhoneOS</key>
	<false/>
	<key>CFBundleIdentifier</key>
	<string>app.eggos.paint</string>
</dict>
</plist>"#;

    #[test]
    fn string_values_are_read_with_their_entities_decoded() {
        let mut scratch = [0u8; 64];
        let mut pairs = Vec::new();
        for_each_string(PLIST, &mut scratch, |key, value| {
            pairs.push((key.to_vec(), value.to_vec()));
        });
        assert_eq!(
            pairs,
            [
                (b"CFBundleExecutable".to_vec(), b"mac-paint".to_vec()),
                (b"CFBundleName".to_vec(), b"Paint & Draw".to_vec()),
                (b"CFBundleIdentifier".to_vec(), b"app.eggos.paint".to_vec()),
            ]
        );
        let mut small = [0u8; 4];
        let mut count = 0;
        for_each_string(PLIST, &mut small, |_, _| count += 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn a_program_in_contents_macos_belongs_to_its_bundle() {
        assert_eq!(
            bundle_path(b"/Applications/mac-paint/mac-paint.app/Contents/MacOS/mac-paint"),
            Some(&b"/Applications/mac-paint/mac-paint.app"[..])
        );
        assert_eq!(bundle_path(b"/usr/bin/hello"), None);
        assert_eq!(bundle_path(b"/tmp/Contents/MacOS/tool"), None);
        assert_eq!(bundle_path(b"/A.app/Contents/MacOS/"), None);
        assert_eq!(bundle_path(b"/A.app/Contents/MacOS/x/y"), None);
    }
}
