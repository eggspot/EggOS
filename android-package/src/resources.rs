//! An APK's resource table, `resources.arsc` (D500): the packages' types and entries, each a
//! resource id (`0xPPTTEEEE`: package, type, entry) with a value per configuration. The
//! Android layer reads the simple string values, which hold both string resources
//! (`R.string.hello`) and the paths of layouts and other files (`R.layout.main` is
//! `res/layout/main.xml`). Every value is kept with the configuration it is for (D764):
//! the value for the device's configuration (its locale, orientation, night mode, density and
//! platform version) is chosen as Android chooses it, with the default configuration's when
//! nothing more specific fits; dimensions, colours, integers and booleans are read as typed
//! values (references followed), and styles as their attribute values over their parents'.

use crate::manifest::{half, strings, word};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

const TABLE: u16 = 0x0002;
const STRING_POOL: u16 = 0x0001;
const PACKAGE: u16 = 0x0200;
const TYPE: u16 = 0x0201;
const TYPE_STRING: u8 = 0x03;
const NO_ENTRY: u32 = u32::MAX;
const NO_SHORT_ENTRY: u16 = u16::MAX;
const FLAG_COMPLEX: u16 = 1;
const FLAG_SPARSE: u8 = 1;
const FLAG_OFFSET_16: u8 = 2;
const PACKAGE_HEADER_ID: usize = 8;
const TYPE_ID: usize = 8;
const TYPE_FLAGS: usize = 9;
const TYPE_ENTRY_COUNT: usize = 12;
const TYPE_ENTRIES_START: usize = 16;
const MOST_ENTRIES: usize = 1 << 16;

/// The string values of a resource table, by resource id, and its colours (D518).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceTable {
    strings: BTreeMap<u32, String>,
    colours: BTreeMap<u32, u32>,
    variants: BTreeMap<u32, Vec<String>>,
    values: BTreeMap<u32, Vec<(Configuration, Value)>>,
    styles: BTreeMap<u32, Style>,
    device: Configuration,
}

/// What a resource value is for (D764): the parts of `ResTable_config` the layer matches on.
/// A zero part means "any".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Configuration {
    pub language: [u8; 2],
    pub country: [u8; 2],
    /// 1 portrait, 2 landscape.
    pub orientation: u8,
    /// 1 not night, 2 night (`uiMode`'s night bits, shifted down).
    pub night: u8,
    pub density: u16,
    pub sdk: u16,
}

impl Configuration {
    fn read(chunk: &[u8], at: usize) -> Self {
        let byte = |offset: usize| chunk.get(at + offset).copied().unwrap_or(0);
        let short = |offset: usize| half(chunk, at + offset).unwrap_or(0);
        Self {
            language: [byte(8), byte(9)],
            country: [byte(10), byte(11)],
            orientation: byte(12),
            night: (byte(29) & 0x30) >> 4,
            density: short(14),
            sdk: short(24),
        }
    }

    /// Whether a value for `self` may serve a device of configuration `device`.
    pub fn fits(&self, device: &Self) -> bool {
        let matches = |wanted: u8, has: u8| wanted == 0 || wanted == has;
        (self.language == [0, 0] || self.language == device.language)
            && (self.country == [0, 0] || self.country == device.country)
            && matches(self.orientation, device.orientation)
            && matches(self.night, device.night)
            && (self.sdk == 0 || self.sdk <= device.sdk.max(1))
    }

    fn specificity(&self) -> (bool, bool, bool, bool, u16) {
        (
            self.language != [0, 0],
            self.country != [0, 0],
            self.orientation != 0,
            self.night != 0,
            self.sdk,
        )
    }
}

/// A simple value: its binary type (`Res_value.dataType`), its data, and its text when it
/// is a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    pub kind: u8,
    pub data: u32,
    pub text: Option<String>,
}

/// A style (a bag entry, D764): its parent style and its attributes' values.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Style {
    pub parent: u32,
    pub items: Vec<(u32, Value)>,
}

const TYPE_REFERENCE: u8 = 0x01;
const TYPE_DIMENSION: u8 = 0x05;
const TYPE_INT_DECIMAL: u8 = 0x10;
const TYPE_INT_HEXADECIMAL: u8 = 0x11;
const TYPE_INT_BOOLEAN: u8 = 0x12;
const MOST_REFERENCES: usize = 8;
const MOST_STYLE_DEPTH: usize = 16;
const MAP_ITEM_BYTES: usize = 12;
const MOST_STYLE_ITEMS: usize = 512;
const CONFIGURATION_AT: usize = 20;

const MOST_VARIANTS: usize = 32;
const DENSITY_QUALIFIERS: [(&str, u32); 8] = [
    ("ldpi", 120),
    ("mdpi", 160),
    ("tvdpi", 213),
    ("hdpi", 240),
    ("xhdpi", 320),
    ("xxhdpi", 480),
    ("xxxhdpi", 640),
    ("nodpi", 0),
];

/// The density a resource file's folder qualifiers name (`res/drawable-xhdpi-v4/a.png` is
/// 320); `None` for a folder without one or for `nodpi` and `anydpi`.
pub fn density_of(path: &str) -> Option<u32> {
    let folder = path.rsplit('/').nth(1)?;
    folder.split('-').skip(1).find_map(|qualifier| {
        DENSITY_QUALIFIERS
            .iter()
            .find(|(name, _)| *name == qualifier)
            .map(|(_, density)| *density)
            .filter(|&density| density > 0)
    })
}
const DENSITIES: [&str; 6] = ["xxxhdpi", "xxhdpi", "xhdpi", "hdpi", "mdpi", "ldpi"];
const ICON_FILES: [&str; 2] = [".png", ".webp"];

const FIRST_COLOUR_TYPE: u8 = 0x1c;
const LAST_COLOUR_TYPE: u8 = 0x1f;

impl ResourceTable {
    /// The table in `bytes`, `resources.arsc` or an App Bundle's `resources.pb` (D758);
    /// `None` when it is neither.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if half(bytes, 0)? != TABLE {
            return Self::parse_bundle(bytes);
        }
        let mut table = Self::default();
        let mut pool = Vec::new();
        let mut at = usize::from(half(bytes, 2)?);
        while at + 8 <= bytes.len() {
            let kind = half(bytes, at)?;
            let size = word(bytes, at + 4)? as usize;
            if size < 8 {
                return None;
            }
            let chunk = bytes.get(at..at + size)?;
            match kind {
                STRING_POOL => pool = strings(chunk)?,
                PACKAGE => table.package(chunk, &pool)?,
                _ => {}
            }
            at += size;
        }
        Some(table)
    }

    fn package(&mut self, chunk: &[u8], pool: &[String]) -> Option<()> {
        let package = word(chunk, PACKAGE_HEADER_ID)? & 0xff;
        let mut at = usize::from(half(chunk, 2)?);
        while at + 8 <= chunk.len() {
            let kind = half(chunk, at)?;
            let size = word(chunk, at + 4)? as usize;
            if size < 8 {
                return None;
            }
            let inner = chunk.get(at..at + size)?;
            if kind == TYPE {
                self.entries(package, inner, pool)?;
            }
            at += size;
        }
        Some(())
    }

    fn entries(&mut self, package: u32, chunk: &[u8], pool: &[String]) -> Option<()> {
        let type_id = u32::from(*chunk.get(TYPE_ID)?);
        let flags = *chunk.get(TYPE_FLAGS)?;
        let count = (word(chunk, TYPE_ENTRY_COUNT)? as usize).min(MOST_ENTRIES);
        let entries_start = word(chunk, TYPE_ENTRIES_START)? as usize;
        let offsets = usize::from(half(chunk, 2)?);
        for slot in 0..count {
            let (entry, offset) = match (flags & FLAG_SPARSE != 0, flags & FLAG_OFFSET_16 != 0) {
                (true, _) => {
                    let index = u32::from(half(chunk, offsets + 4 * slot)?);
                    let offset = usize::from(half(chunk, offsets + 4 * slot + 2)?) * 4;
                    (index, offset)
                }
                (false, true) => match half(chunk, offsets + 2 * slot)? {
                    NO_SHORT_ENTRY => continue,
                    offset => (slot as u32, usize::from(offset) * 4),
                },
                (false, false) => match word(chunk, offsets + 4 * slot)? {
                    NO_ENTRY => continue,
                    offset => (slot as u32, offset as usize),
                },
            };
            let at = entries_start + offset;
            let entry_size = usize::from(half(chunk, at)?);
            let id = package << 24 | type_id << 16 | entry;
            if half(chunk, at + 2)? & FLAG_COMPLEX != 0 {
                self.style(id, chunk, at, entry_size, pool);
                continue;
            }
            let value = at + entry_size;
            let typed = Value {
                kind: *chunk.get(value + 3)?,
                data: word(chunk, value + 4)?,
                text: None,
            };
            let typed = Value {
                text: (typed.kind == TYPE_STRING)
                    .then(|| pool.get(typed.data as usize).cloned())
                    .flatten(),
                ..typed
            };
            let configuration = Configuration::read(chunk, CONFIGURATION_AT);
            self.values
                .entry(id)
                .or_default()
                .push((configuration, typed));
            let id_of = |entry: u32| package << 24 | type_id << 16 | entry;
            let kind = *chunk.get(value + 3)?;
            if (FIRST_COLOUR_TYPE..=LAST_COLOUR_TYPE).contains(&kind) {
                let colour = word(chunk, value + 4)?;
                self.colours.entry(id_of(entry)).or_insert(colour);
                continue;
            }
            if kind != TYPE_STRING {
                continue;
            }
            let Some(text) = pool.get(word(chunk, value + 4)? as usize) else {
                continue;
            };
            let id = package << 24 | type_id << 16 | entry;
            self.strings.entry(id).or_insert_with(|| text.clone());
            let variants = self.variants.entry(id).or_default();
            if variants.len() < MOST_VARIANTS && !variants.contains(text) {
                variants.push(text.clone());
            }
        }
        Some(())
    }

    fn style(&mut self, id: u32, chunk: &[u8], at: usize, entry_size: usize, pool: &[String]) {
        let parent = word(chunk, at + 8).unwrap_or(0);
        let count = (word(chunk, at + 12).unwrap_or(0) as usize).min(MOST_STYLE_ITEMS);
        let mut style = Style {
            parent,
            items: Vec::with_capacity(count),
        };
        for index in 0..count {
            let item = at + entry_size + index * MAP_ITEM_BYTES;
            let (Some(name), Some(&kind), Some(data)) =
                (word(chunk, item), chunk.get(item + 7), word(chunk, item + 8))
            else {
                break;
            };
            let text = (kind == TYPE_STRING)
                .then(|| pool.get(data as usize).cloned())
                .flatten();
            style.items.push((name, Value { kind, data, text }));
        }
        self.styles.entry(id).or_insert(style);
    }

    /// Chooses values for a device of configuration `device` from now on.
    pub fn set_device(&mut self, device: Configuration) {
        self.device = device;
    }

    /// Resource `id`'s value for the device: the most specific configuration that fits it.
    pub fn value(&self, id: u32) -> Option<&Value> {
        let device = self.device;
        self.values
            .get(&id)?
            .iter()
            .filter(|(configuration, _)| configuration.fits(&device))
            .fold(None::<&(Configuration, Value)>, |best, candidate| match best {
                Some(chosen) if chosen.0.specificity() >= candidate.0.specificity() => Some(chosen),
                _ => Some(candidate),
            })
            .map(|(_, value)| value)
    }

    /// Resource `id`'s value with references to other resources followed.
    pub fn resolved(&self, id: u32) -> Option<&Value> {
        let mut current = self.value(id)?;
        for _ in 0..MOST_REFERENCES {
            if current.kind != TYPE_REFERENCE || current.data == 0 {
                return Some(current);
            }
            current = self.value(current.data)?;
        }
        None
    }

    /// Dimension resource `id` (`R.dimen.margin`), as binary XML encodes a dimension.
    pub fn dimension(&self, id: u32) -> Option<u32> {
        self.resolved(id)
            .filter(|value| value.kind == TYPE_DIMENSION)
            .map(|value| value.data)
    }

    /// Integer resource `id` (`R.integer.columns`).
    pub fn integer(&self, id: u32) -> Option<i32> {
        self.resolved(id)
            .filter(|value| matches!(value.kind, TYPE_INT_DECIMAL | TYPE_INT_HEXADECIMAL))
            .map(|value| value.data as i32)
    }

    /// Boolean resource `id` (`R.bool.tablet`).
    pub fn boolean(&self, id: u32) -> Option<bool> {
        self.resolved(id)
            .filter(|value| value.kind == TYPE_INT_BOOLEAN)
            .map(|value| value.data != 0)
    }

    /// Style `id`'s attribute values, its own over its parents', by attribute id.
    pub fn style_items(&self, id: u32) -> BTreeMap<u32, Value> {
        let mut chain = Vec::new();
        let mut current = id;
        for _ in 0..MOST_STYLE_DEPTH {
            let Some(style) = self.styles.get(&current) else {
                break;
            };
            chain.push(style);
            current = style.parent;
        }
        let mut items = BTreeMap::new();
        for style in chain.into_iter().rev() {
            for (name, value) in &style.items {
                items.insert(*name, value.clone());
            }
        }
        items
    }

    fn parse_bundle(bytes: &[u8]) -> Option<Self> {
        let values = crate::proto::table_values(bytes);
        if values.is_empty() {
            return None;
        }
        let mut table = Self::default();
        for each in values {
            let kind = each.value.kind;
            if (FIRST_COLOUR_TYPE..=LAST_COLOUR_TYPE).contains(&kind) {
                table.colours.entry(each.id).or_insert(each.value.data);
                continue;
            }
            let Some(text) = each.value.text else {
                continue;
            };
            let variants = table.variants.entry(each.id).or_default();
            if variants.len() < MOST_VARIANTS && !variants.contains(&text) {
                variants.push(text.clone());
            }
            table.strings.entry(each.id).or_insert(text);
        }
        Some(table)
    }

    /// Resource `id`'s string value for the device: a string resource's text or a file
    /// resource's path.
    pub fn string(&self, id: u32) -> Option<&str> {
        self.resolved(id)
            .and_then(|value| value.text.as_deref())
            .or_else(|| self.strings.get(&id).map(String::as_str))
    }

    /// Every configuration's string value of resource `id`, in the table's order: a file
    /// resource's path for each density, locale and version it has (D757).
    pub fn variants(&self, id: u32) -> &[String] {
        self.variants.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The best picture file for icon resource `id`: a PNG (else WebP) of the highest density
    /// it comes in, never an adaptive icon's XML (D757).
    pub fn icon_file(&self, id: u32) -> Option<&str> {
        let rank = |path: &str| {
            let kind = ICON_FILES
                .iter()
                .position(|ending| path.ends_with(ending))?;
            let density = DENSITIES
                .iter()
                .position(|density| path.contains(density))
                .unwrap_or(DENSITIES.len());
            Some((kind, density))
        };
        self.variants(id)
            .iter()
            .filter_map(|path| rank(path).map(|rank| (rank, path.as_str())))
            .min_by_key(|&(rank, _)| rank)
            .map(|(_, path)| path)
    }

    /// The file of drawable resource `id` best for a screen of `dpi` dots per inch, with the
    /// density it was drawn for (D763): one drawn for exactly that density, else the least
    /// dense above it (scaled down), else the densest below it (scaled up); a `nodpi` or
    /// `anydpi` file counts as drawn for the screen.
    pub fn density_variant(&self, id: u32, dpi: u32) -> Option<(&str, u32)> {
        let densities = self
            .variants(id)
            .iter()
            .map(|path| (path.as_str(), density_of(path).unwrap_or(dpi)));
        let mut best: Option<(&str, u32)> = None;
        for (path, density) in densities {
            let better = match best {
                None => true,
                Some((_, chosen)) if chosen == dpi => false,
                Some(_) if density == dpi => true,
                Some((_, chosen)) if chosen < dpi => density > chosen,
                Some((_, chosen)) => density >= dpi && density < chosen,
            };
            if better {
                best = Some((path, density));
            }
        }
        best
    }

    /// Colour resource `id`'s value for the device, `0xAARRGGBB`.
    pub fn colour(&self, id: u32) -> Option<u32> {
        self.resolved(id)
            .filter(|value| (FIRST_COLOUR_TYPE..=LAST_COLOUR_TYPE).contains(&value.kind))
            .map(|value| value.data)
            .or_else(|| self.colours.get(&id).copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn pool(strings: &[&str]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for text in strings {
            offsets.push(data.len() as u32);
            data.push(text.len() as u8);
            data.push(text.len() as u8);
            data.extend_from_slice(text.as_bytes());
            data.push(0);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let start = 28 + 4 * strings.len();
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&STRING_POOL.to_le_bytes());
        chunk.extend_from_slice(&28u16.to_le_bytes());
        chunk.extend_from_slice(&((start + data.len()) as u32).to_le_bytes());
        chunk.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        chunk.extend_from_slice(&0u32.to_le_bytes());
        chunk.extend_from_slice(&(1u32 << 8).to_le_bytes());
        chunk.extend_from_slice(&(start as u32).to_le_bytes());
        chunk.extend_from_slice(&0u32.to_le_bytes());
        for offset in offsets {
            chunk.extend_from_slice(&offset.to_le_bytes());
        }
        chunk.extend_from_slice(&data);
        chunk
    }

    fn type_chunk(type_id: u8, values: &[Option<u32>]) -> Vec<u8> {
        let header = 20 + 64;
        let offsets_bytes = 4 * values.len();
        let entries_start = header + offsets_bytes;
        let mut entries = Vec::new();
        let mut offsets = Vec::new();
        for value in values {
            match value {
                None => offsets.extend_from_slice(&NO_ENTRY.to_le_bytes()),
                Some(string) => {
                    offsets.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                    entries.extend_from_slice(&8u16.to_le_bytes());
                    entries.extend_from_slice(&0u16.to_le_bytes());
                    entries.extend_from_slice(&0u32.to_le_bytes());
                    entries.extend_from_slice(&[8, 0, 0, TYPE_STRING]);
                    entries.extend_from_slice(&string.to_le_bytes());
                }
            }
        }
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&TYPE.to_le_bytes());
        chunk.extend_from_slice(&(header as u16).to_le_bytes());
        chunk.extend_from_slice(&((entries_start + entries.len()) as u32).to_le_bytes());
        chunk.extend_from_slice(&[type_id, 0, 0, 0]);
        chunk.extend_from_slice(&(values.len() as u32).to_le_bytes());
        chunk.extend_from_slice(&(entries_start as u32).to_le_bytes());
        chunk.extend_from_slice(&[0; 64]);
        chunk.extend(offsets);
        chunk.extend(entries);
        chunk
    }

    #[test]
    fn string_and_layout_values_are_found_by_their_ids() {
        let strings = pool(&["Hello", "res/layout/main.xml"]);
        let mut package = Vec::new();
        package.extend(type_chunk(2, &[Some(1)]));
        package.extend(type_chunk(3, &[None, Some(0)]));
        let header = 288;
        let mut package_chunk = Vec::new();
        package_chunk.extend_from_slice(&PACKAGE.to_le_bytes());
        package_chunk.extend_from_slice(&(header as u16).to_le_bytes());
        package_chunk.extend_from_slice(&((header + package.len()) as u32).to_le_bytes());
        package_chunk.extend_from_slice(&0x7fu32.to_le_bytes());
        package_chunk.extend_from_slice(&vec![0u8; header - 12]);
        package_chunk.extend(package);
        let mut body = strings;
        body.extend(package_chunk);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&TABLE.to_le_bytes());
        bytes.extend_from_slice(&12u16.to_le_bytes());
        bytes.extend_from_slice(&((12 + body.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend(body);
        let table = ResourceTable::parse(&bytes).unwrap();
        assert_eq!(table.string(0x7f02_0000), Some("res/layout/main.xml"));
        assert_eq!(table.string(0x7f03_0001), Some("Hello"));
        assert_eq!(table.string(0x7f03_0000), None);
    }

    #[test]
    fn the_icon_is_the_densest_png_and_never_the_adaptive_xml() {
        let mut table = ResourceTable::default();
        table.variants.insert(
            0x7f0d_0000,
            alloc::vec![
                String::from("res/mipmap-anydpi-v26/ic_launcher.xml"),
                String::from("res/mipmap-hdpi-v4/ic_launcher.png"),
                String::from("res/mipmap-xxxhdpi-v4/ic_launcher.png"),
                String::from("res/mipmap-xhdpi-v4/ic_launcher.webp"),
            ],
        );
        assert_eq!(
            table.icon_file(0x7f0d_0000),
            Some("res/mipmap-xxxhdpi-v4/ic_launcher.png")
        );
        assert_eq!(table.icon_file(0x7f0d_0001), None);
    }

    #[test]
    fn the_density_nearest_above_the_screen_wins() {
        let mut table = ResourceTable::default();
        table.variants.insert(
            0x7f08_0001,
            vec![
                String::from("res/drawable-mdpi-v4/a.png"),
                String::from("res/drawable-xxhdpi-v4/a.png"),
                String::from("res/drawable-xhdpi-v4/a.png"),
            ],
        );
        let chosen = |dpi| table.density_variant(0x7f08_0001, dpi);
        assert_eq!(chosen(320), Some(("res/drawable-xhdpi-v4/a.png", 320)));
        assert_eq!(chosen(240), Some(("res/drawable-xhdpi-v4/a.png", 320)));
        assert_eq!(chosen(160), Some(("res/drawable-mdpi-v4/a.png", 160)));
        assert_eq!(chosen(640), Some(("res/drawable-xxhdpi-v4/a.png", 480)));
        assert_eq!(density_of("res/drawable-anydpi-v24/a.xml"), None);
        assert_eq!(density_of("res/drawable/a.png"), None);
    }

    #[test]
    fn the_most_specific_fitting_configuration_wins() {
        let text = |words: &str| Value {
            kind: TYPE_STRING,
            data: 0,
            text: Some(String::from(words)),
        };
        let mut table = ResourceTable::default();
        let vietnamese = Configuration {
            language: *b"vi",
            ..Configuration::default()
        };
        let night = Configuration {
            night: 2,
            ..Configuration::default()
        };
        table.values.insert(
            0x7f0f_0001,
            vec![
                (Configuration::default(), text("Hello")),
                (vietnamese, text("Xin chào")),
            ],
        );
        table.values.insert(
            0x7f05_0001,
            vec![
                (Configuration::default(), Value { kind: 0x1c, data: 0xffff_ffff, text: None }),
                (night, Value { kind: 0x1c, data: 0xff00_0000, text: None }),
            ],
        );
        assert_eq!(table.string(0x7f0f_0001), Some("Hello"));
        assert_eq!(table.colour(0x7f05_0001), Some(0xffff_ffff));
        table.set_device(Configuration {
            language: *b"vi",
            country: *b"VN",
            night: 2,
            sdk: 34,
            ..Configuration::default()
        });
        assert_eq!(table.string(0x7f0f_0001), Some("Xin chào"));
        assert_eq!(table.colour(0x7f05_0001), Some(0xff00_0000));
        let newer = Configuration {
            sdk: 35,
            ..Configuration::default()
        };
        assert!(!newer.fits(&table.device));
    }

    #[test]
    fn a_style_s_own_values_override_its_parent_s() {
        let mut table = ResourceTable::default();
        let value = |data: u32| Value { kind: TYPE_INT_DECIMAL, data, text: None };
        table.styles.insert(1, Style { parent: 0, items: vec![(10, value(1)), (11, value(2))] });
        table.styles.insert(2, Style { parent: 1, items: vec![(11, value(3))] });
        let items = table.style_items(2);
        assert_eq!(items.get(&10).map(|value| value.data), Some(1));
        assert_eq!(items.get(&11).map(|value| value.data), Some(3));
    }
}
