//! The binary XML of `AndroidManifest.xml` (D450): a chunk of chunks, the first a string pool
//! (UTF-16 or UTF-8), then the elements, each with its attributes as a name's string and a
//! typed value. [`Manifest::parse`] reads the package's name and version, its lowest and target
//! SDK, the permissions it asks for, and the activity the launcher starts (the one whose
//! intent filter holds `android.intent.action.MAIN` and `android.intent.category.LAUNCHER`).

use alloc::string::String;
use alloc::vec::Vec;

const XML_CHUNK: u16 = 0x0003;
const STRING_POOL: u16 = 0x0001;
const START_ELEMENT: u16 = 0x0102;
const END_ELEMENT: u16 = 0x0103;
const UTF8_FLAG: u32 = 1 << 8;
const NO_STRING: u32 = u32::MAX;
const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;
const ATTRIBUTE_BYTES: usize = 20;
const MAIN_ACTION: &str = "android.intent.action.MAIN";
const LAUNCHER_CATEGORY: &str = "android.intent.category.LAUNCHER";

/// What a manifest says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    pub package: String,
    pub version_name: String,
    pub version_code: u32,
    pub min_sdk: u32,
    pub target_sdk: u32,
    pub permissions: Vec<String>,
    /// The activity the launcher starts, as its manifest names it (`.MainActivity` or a full
    /// class name).
    pub launcher_activity: Option<String>,
}

pub(crate) fn half(bytes: &[u8], at: usize) -> Option<u16> {
    let field = bytes.get(at..at + 2)?;
    Some(u16::from_le_bytes([field[0], field[1]]))
}

pub(crate) fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

pub(crate) fn strings(chunk: &[u8]) -> Option<Vec<String>> {
    let count = word(chunk, 8)? as usize;
    let flags = word(chunk, 16)?;
    let data = word(chunk, 20)? as usize;
    let header = usize::from(half(chunk, 2)?);
    let mut found = Vec::with_capacity(count.min(4096));
    for index in 0..count {
        let at = data + word(chunk, header + 4 * index)? as usize;
        let text = match flags & UTF8_FLAG != 0 {
            true => {
                let skip = if chunk.get(at)? & 0x80 != 0 { 2 } else { 1 };
                let first = *chunk.get(at + skip)?;
                let (length, start) = match first & 0x80 != 0 {
                    true => (usize::from(first & 0x7f) << 8 | usize::from(*chunk.get(at + skip + 1)?), at + skip + 2),
                    false => (usize::from(first), at + skip + 1),
                };
                String::from_utf8_lossy(chunk.get(start..start + length)?).into_owned()
            }
            false => {
                let first = half(chunk, at)?;
                let (length, start) = match first & 0x8000 != 0 {
                    true => (usize::from(first & 0x7fff) << 16 | usize::from(half(chunk, at + 2)?), at + 4),
                    false => (usize::from(first), at + 2),
                };
                let units: Vec<u16> = (0..length).map(|unit| half(chunk, start + 2 * unit)).collect::<Option<_>>()?;
                String::from_utf16_lossy(&units)
            }
        };
        found.push(text);
    }
    Some(found)
}

#[derive(Default)]
struct Walk {
    in_activity: Option<String>,
    filter_main: bool,
    filter_launcher: bool,
}

impl Manifest {
    /// The manifest in binary XML `xml`; `None` when it is not binary XML.
    pub fn parse(xml: &[u8]) -> Option<Self> {
        if half(xml, 0)? != XML_CHUNK {
            return None;
        }
        let mut at = usize::from(half(xml, 2)?);
        let mut pool = Vec::new();
        let mut manifest = Self::default();
        let mut walk = Walk::default();
        while at + 8 <= xml.len() {
            let kind = half(xml, at)?;
            let size = word(xml, at + 4)? as usize;
            if size < 8 {
                return None;
            }
            let chunk = xml.get(at..at + size)?;
            match kind {
                STRING_POOL => pool = strings(chunk)?,
                START_ELEMENT => manifest.start(chunk, &pool, &mut walk)?,
                END_ELEMENT => manifest.end(chunk, &pool, &mut walk),
                _ => {}
            }
            at += size;
        }
        Some(manifest)
    }

    fn start(&mut self, chunk: &[u8], pool: &[String], walk: &mut Walk) -> Option<()> {
        let name = pool.get(word(chunk, 20)? as usize)?.as_str();
        let start = usize::from(half(chunk, 24)?);
        let size = usize::from(half(chunk, 26)?).max(ATTRIBUTE_BYTES);
        let count = usize::from(half(chunk, 28)?);
        let mut attributes = Vec::with_capacity(count);
        for index in 0..count {
            let at = 16 + start + index * size;
            let attribute = pool.get(word(chunk, at + 4)? as usize).map_or("", String::as_str);
            let raw = word(chunk, at + 8)?;
            let kind = *chunk.get(at + 15)?;
            let data = word(chunk, at + 16)?;
            let text = match (raw, kind) {
                (raw, _) if raw != NO_STRING => pool.get(raw as usize).cloned(),
                (_, TYPE_STRING) => pool.get(data as usize).cloned(),
                _ => None,
            };
            attributes.push((attribute, text, (kind, data)));
        }
        let value = |wanted: &str| attributes.iter().find(|(name, _, _)| *name == wanted);
        let text = |wanted: &str| value(wanted).and_then(|(_, text, _)| text.clone());
        let number = |wanted: &str| {
            value(wanted).and_then(|(_, text, (kind, data))| match kind {
                &TYPE_INT_DEC | 0x11 => Some(*data),
                _ => text.as_deref().and_then(|text| text.parse().ok()),
            })
        };
        match name {
            "manifest" => {
                self.package = text("package").unwrap_or_default();
                self.version_name = text("versionName").unwrap_or_default();
                self.version_code = number("versionCode").unwrap_or(0);
            }
            "uses-sdk" => {
                self.min_sdk = number("minSdkVersion").unwrap_or(1);
                self.target_sdk = number("targetSdkVersion").unwrap_or(self.min_sdk);
            }
            "uses-permission" => {
                if let Some(permission) = text("name") {
                    self.permissions.push(permission);
                }
            }
            "activity" | "activity-alias" => walk.in_activity = text("name"),
            "action" if text("name").as_deref() == Some(MAIN_ACTION) => walk.filter_main = true,
            "category" if text("name").as_deref() == Some(LAUNCHER_CATEGORY) => walk.filter_launcher = true,
            _ => {}
        }
        Some(())
    }

    fn end(&mut self, chunk: &[u8], pool: &[String], walk: &mut Walk) {
        let name = word(chunk, 20).and_then(|index| pool.get(index as usize));
        match name.map(String::as_str) {
            Some("intent-filter") => {
                if walk.filter_main && walk.filter_launcher && self.launcher_activity.is_none() {
                    self.launcher_activity = walk.in_activity.clone();
                }
                walk.filter_main = false;
                walk.filter_launcher = false;
            }
            Some("activity" | "activity-alias") => walk.in_activity = None,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Builder {
        strings: Vec<&'static str>,
        elements: Vec<u8>,
    }

    impl Builder {
        fn index(&mut self, text: &'static str) -> u32 {
            match self.strings.iter().position(|known| *known == text) {
                Some(at) => at as u32,
                None => {
                    self.strings.push(text);
                    (self.strings.len() - 1) as u32
                }
            }
        }

        fn start(&mut self, name: &'static str, attributes: &[(&'static str, Result<&'static str, u32>)]) {
            let name = self.index(name);
            let mut chunk = Vec::new();
            chunk.extend_from_slice(&START_ELEMENT.to_le_bytes());
            chunk.extend_from_slice(&16u16.to_le_bytes());
            let size = 36 + ATTRIBUTE_BYTES * attributes.len();
            chunk.extend_from_slice(&(size as u32).to_le_bytes());
            chunk.extend_from_slice(&[0; 8]);
            chunk.extend_from_slice(&NO_STRING.to_le_bytes());
            chunk.extend_from_slice(&name.to_le_bytes());
            chunk.extend_from_slice(&20u16.to_le_bytes());
            chunk.extend_from_slice(&20u16.to_le_bytes());
            chunk.extend_from_slice(&(attributes.len() as u16).to_le_bytes());
            chunk.extend_from_slice(&[0; 6]);
            for (attribute, value) in attributes {
                let attribute = self.index(attribute);
                chunk.extend_from_slice(&NO_STRING.to_le_bytes());
                chunk.extend_from_slice(&attribute.to_le_bytes());
                match value {
                    Ok(text) => {
                        let text = self.index(text);
                        chunk.extend_from_slice(&text.to_le_bytes());
                        chunk.extend_from_slice(&[8, 0, 0, TYPE_STRING]);
                        chunk.extend_from_slice(&text.to_le_bytes());
                    }
                    Err(number) => {
                        chunk.extend_from_slice(&NO_STRING.to_le_bytes());
                        chunk.extend_from_slice(&[8, 0, 0, TYPE_INT_DEC]);
                        chunk.extend_from_slice(&number.to_le_bytes());
                    }
                }
            }
            self.elements.extend_from_slice(&chunk);
        }

        fn end(&mut self, name: &'static str) {
            let name = self.index(name);
            self.elements.extend_from_slice(&END_ELEMENT.to_le_bytes());
            self.elements.extend_from_slice(&16u16.to_le_bytes());
            self.elements.extend_from_slice(&24u32.to_le_bytes());
            self.elements.extend_from_slice(&[0; 8]);
            self.elements.extend_from_slice(&NO_STRING.to_le_bytes());
            self.elements.extend_from_slice(&name.to_le_bytes());
        }

        fn finish(self) -> Vec<u8> {
            let mut data = Vec::new();
            let mut offsets = Vec::new();
            for text in &self.strings {
                offsets.push(data.len() as u32);
                data.push(text.len() as u8);
                data.push(text.len() as u8);
                data.extend_from_slice(text.as_bytes());
                data.push(0);
            }
            while data.len() % 4 != 0 {
                data.push(0);
            }
            let header = 28;
            let start = header + 4 * offsets.len();
            let mut pool = Vec::new();
            pool.extend_from_slice(&STRING_POOL.to_le_bytes());
            pool.extend_from_slice(&(header as u16).to_le_bytes());
            pool.extend_from_slice(&((start + data.len()) as u32).to_le_bytes());
            pool.extend_from_slice(&(offsets.len() as u32).to_le_bytes());
            pool.extend_from_slice(&0u32.to_le_bytes());
            pool.extend_from_slice(&UTF8_FLAG.to_le_bytes());
            pool.extend_from_slice(&(start as u32).to_le_bytes());
            pool.extend_from_slice(&0u32.to_le_bytes());
            for offset in offsets {
                pool.extend_from_slice(&offset.to_le_bytes());
            }
            pool.extend_from_slice(&data);
            let mut xml = Vec::new();
            xml.extend_from_slice(&XML_CHUNK.to_le_bytes());
            xml.extend_from_slice(&8u16.to_le_bytes());
            xml.extend_from_slice(&((8 + pool.len() + self.elements.len()) as u32).to_le_bytes());
            xml.extend_from_slice(&pool);
            xml.extend_from_slice(&self.elements);
            xml
        }
    }

    #[test]
    fn a_manifest_names_its_package_sdk_permissions_and_launcher() {
        let mut xml = Builder {
            strings: Vec::new(),
            elements: Vec::new(),
        };
        xml.start(
            "manifest",
            &[("package", Ok("org.example.notes")), ("versionName", Ok("1.2")), ("versionCode", Err(12))],
        );
        xml.start("uses-sdk", &[("minSdkVersion", Err(24)), ("targetSdkVersion", Err(34))]);
        xml.end("uses-sdk");
        xml.start("uses-permission", &[("name", Ok("android.permission.INTERNET"))]);
        xml.end("uses-permission");
        xml.start("application", &[]);
        xml.start("activity", &[("name", Ok(".SettingsActivity"))]);
        xml.end("activity");
        xml.start("activity", &[("name", Ok(".MainActivity"))]);
        xml.start("intent-filter", &[]);
        xml.start("action", &[("name", Ok(MAIN_ACTION))]);
        xml.end("action");
        xml.start("category", &[("name", Ok(LAUNCHER_CATEGORY))]);
        xml.end("category");
        xml.end("intent-filter");
        xml.end("activity");
        xml.end("application");
        xml.end("manifest");
        let manifest = Manifest::parse(&xml.finish()).unwrap();
        assert_eq!(manifest.package, "org.example.notes");
        assert_eq!((manifest.version_name.as_str(), manifest.version_code), ("1.2", 12));
        assert_eq!((manifest.min_sdk, manifest.target_sdk), (24, 34));
        assert_eq!(manifest.permissions, ["android.permission.INTERNET"]);
        assert_eq!(manifest.launcher_activity.as_deref(), Some(".MainActivity"));
    }

    #[test]
    fn text_xml_is_not_binary_xml() {
        assert_eq!(Manifest::parse(b"<manifest/>"), None);
    }
}
