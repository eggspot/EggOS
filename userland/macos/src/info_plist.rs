//! An `.app` bundle's place and its `Contents/Info.plist` (an XML or binary property list,
//! read by `eggos-plist`) as far as the layer reads them: the bundle a program inside
//! `X.app/Contents/MacOS/` belongs to, and the string values of the property list's keys
//! (`CFBundleName`, `CFBundleIdentifier`, ...).

const MACOS_FOLDER: &[u8] = b"/Contents/MacOS/";
const BUNDLE_SUFFIX: &[u8] = b".app";
const LONGEST_KEY: usize = 256;

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

/// Calls `each` with every key of the top-level dictionary whose value is a string, and that
/// value as UTF-8, from an XML or a binary property list (values longer than `scratch` are
/// skipped).
pub fn for_each_string(plist: &[u8], scratch: &mut [u8], mut each: impl FnMut(&[u8], &[u8])) {
    let Ok(document) = eggos_plist::Document::parse(plist) else {
        return;
    };
    let Some(entries) = document.top().entries() else {
        return;
    };
    let mut key_buffer = [0u8; LONGEST_KEY];
    for (key, value) in entries {
        let (Some(key), Some(value)) = (key.text(), value.text()) else {
            continue;
        };
        let (Some(key), Some(value)) = (key.write_utf8(&mut key_buffer), value.write_utf8(scratch))
        else {
            continue;
        };
        each(key, value);
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
