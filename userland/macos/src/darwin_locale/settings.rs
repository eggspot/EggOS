//! `setlocale`'s names and categories as a Mac answers them: C, POSIX, C.UTF-8 and en_US.UTF-8
//! for every category and UTF-8 for character types only; an empty name takes the environment's
//! (`LC_ALL`, then the category's own variable, then `LANG`); and `LC_ALL` is one name when the
//! six categories share it, else the six joined by `/` in category order, which `LC_ALL` also
//! accepts back.

use core::ffi::CStr;

/// `LC_ALL`: every category at once.
pub const ALL: i32 = 0;
/// `LC_COLLATE`, the first category.
pub const COLLATE: i32 = 1;
/// `LC_CTYPE`: character classes and the multibyte encoding.
pub const CTYPE: i32 = 2;
/// `LC_MONETARY`.
pub const MONETARY: i32 = 3;
/// `LC_NUMERIC`.
pub const NUMERIC: i32 = 4;
/// `LC_TIME`.
pub const TIME: i32 = 5;
/// `LC_MESSAGES`, the last category.
pub const MESSAGES: i32 = 6;

const CATEGORY_COUNT: usize = 6;
const LOCALE_BITS: u32 = 3;
const LOCALE_MASK: u32 = (1 << LOCALE_BITS) - 1;
const VARIABLES: [&[u8]; CATEGORY_COUNT] = [
    b"LC_COLLATE",
    b"LC_CTYPE",
    b"LC_MONETARY",
    b"LC_NUMERIC",
    b"LC_TIME",
    b"LC_MESSAGES",
];
const LONGEST_NAME: usize = 11;

/// The room [`Settings::composite`] needs: six of the longest name, each followed by a `/` or
/// the NUL.
pub const NAME_CAPACITY: usize = CATEGORY_COUNT * (LONGEST_NAME + 1);

/// A locale the layer has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    C,
    Posix,
    CUtf8,
    UnitedStates,
    Utf8,
}

const LOCALES: [Locale; 5] = [
    Locale::C,
    Locale::Posix,
    Locale::CUtf8,
    Locale::UnitedStates,
    Locale::Utf8,
];

impl Locale {
    /// The name `setlocale` takes and answers.
    pub fn name(self) -> &'static CStr {
        match self {
            Self::C => c"C",
            Self::Posix => c"POSIX",
            Self::CUtf8 => c"C.UTF-8",
            Self::UnitedStates => c"en_US.UTF-8",
            Self::Utf8 => c"UTF-8",
        }
    }

    fn named(name: &[u8], category: i32) -> Option<Self> {
        LOCALES.into_iter().find(|locale| {
            locale.name().to_bytes() == name && (*locale != Self::Utf8 || category == CTYPE)
        })
    }

    /// Whether characters are UTF-8 sequences when this is the `LC_CTYPE` locale; otherwise each
    /// byte is a character.
    pub fn is_utf8(self) -> bool {
        matches!(self, Self::CUtf8 | Self::UnitedStates | Self::Utf8)
    }

    /// Whether this is `en_US.UTF-8`, whose time, number, money and message conventions are the
    /// United States' rather than C's.
    pub fn is_united_states(self) -> bool {
        self == Self::UnitedStates
    }
}

/// The locale of each of the six categories, three bits each, so that one atomic word holds
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings(u32);

impl Settings {
    /// Every category in the C locale, as a program starts.
    pub const C: Self = Self(0);

    pub fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    fn shift(category: i32) -> u32 {
        (category.clamp(COLLATE, MESSAGES) - COLLATE) as u32 * LOCALE_BITS
    }

    /// The locale of `category`, one of [`COLLATE`] to [`MESSAGES`].
    pub fn get(self, category: i32) -> Locale {
        let index = (self.0 >> Self::shift(category)) & LOCALE_MASK;
        LOCALES.get(index as usize).copied().unwrap_or(Locale::C)
    }

    fn with(self, category: i32, locale: Locale) -> Self {
        let shift = Self::shift(category);
        Self((self.0 & !(LOCALE_MASK << shift)) | ((locale as u32) << shift))
    }

    /// The settings after `setlocale(category, name)`, or `None` when it fails and changes
    /// nothing: `name` is a locale's name, for [`ALL`] also six names joined by `/`, or empty
    /// for the environment's, which `environment` looks up by variable name.
    pub fn set<'e>(
        self,
        category: i32,
        name: &[u8],
        environment: impl Fn(&[u8]) -> Option<&'e [u8]>,
    ) -> Option<Self> {
        let categories = match category {
            ALL => COLLATE..=MESSAGES,
            COLLATE..=MESSAGES => category..=category,
            _ => return None,
        };
        let composite = category == ALL && name.contains(&b'/');
        let mut parts = name.split(|&byte| byte == b'/');
        let mut settings = self;
        for each in categories {
            let wanted: &[u8] = if composite {
                parts.next()?
            } else if name.is_empty() {
                from_environment(each, &environment)
            } else {
                name
            };
            settings = settings.with(each, Locale::named(wanted, each)?);
        }
        if composite && parts.next().is_some() {
            return None;
        }
        Some(settings)
    }

    /// The one locale `setlocale(category, NULL)` names: `category`'s, or for [`ALL`] the one
    /// all six share; `None` when they differ and the name is [`Settings::composite`].
    pub fn single(self, category: i32) -> Option<Locale> {
        if category != ALL {
            return Some(self.get(category));
        }
        let first = self.get(COLLATE);
        (COLLATE..=MESSAGES)
            .all(|each| self.get(each) == first)
            .then_some(first)
    }

    /// The six categories' names joined by `/`, written into `out` with a NUL; returns the
    /// length without the NUL.
    pub fn composite(self, out: &mut [u8; NAME_CAPACITY]) -> usize {
        let mut length = 0;
        for each in COLLATE..=MESSAGES {
            let name = self.get(each).name().to_bytes();
            out[length..length + name.len()].copy_from_slice(name);
            length += name.len();
            out[length] = if each == MESSAGES { 0 } else { b'/' };
            length += 1;
        }
        length - 1
    }
}

fn from_environment<'e>(
    category: i32,
    environment: &impl Fn(&[u8]) -> Option<&'e [u8]>,
) -> &'e [u8] {
    let own = VARIABLES[(category - COLLATE) as usize];
    [b"LC_ALL".as_slice(), own, b"LANG"]
        .into_iter()
        .find_map(|variable| environment(variable).filter(|value| !value.is_empty()))
        .unwrap_or(b"C")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &[u8]) -> Option<&'static [u8]> {
        None
    }

    fn name(settings: Settings) -> String {
        match settings.single(ALL) {
            Some(locale) => locale.name().to_str().unwrap().to_owned(),
            None => {
                let mut out = [0; NAME_CAPACITY];
                let length = settings.composite(&mut out);
                assert_eq!(out[length], 0);
                String::from_utf8(out[..length].to_vec()).unwrap()
            }
        }
    }

    #[test]
    fn names_are_accepted_per_category_as_on_a_mac() {
        let posix = Settings::C.set(ALL, b"POSIX", none).unwrap();
        assert_eq!(name(posix), "POSIX");
        let mixed = Settings::C.set(CTYPE, b"UTF-8", none).unwrap();
        assert_eq!(name(mixed), "C/UTF-8/C/C/C/C");
        assert_eq!(mixed.set(ALL, b"UTF-8", none), None);
        assert_eq!(mixed.set(TIME, b"UTF-8", none), None);
        assert_eq!(mixed.set(ALL, b"en_US.utf8", none), None);
        assert_eq!(mixed.set(ALL, b"xx_YY.UTF-8", none), None);
        assert_eq!(mixed.set(7, b"C", none), None);
        let united = mixed.set(ALL, b"en_US.UTF-8", none).unwrap();
        assert_eq!(name(united), "en_US.UTF-8");
        assert!(united.get(CTYPE).is_utf8() && united.get(TIME).is_united_states());
        let c_utf8 = Settings::C.set(ALL, b"C.UTF-8", none).unwrap();
        assert_eq!(name(c_utf8), "C.UTF-8");
        assert!(c_utf8.get(CTYPE).is_utf8() && !c_utf8.get(NUMERIC).is_united_states());
        assert!(!Settings::C.get(CTYPE).is_utf8());
    }

    #[test]
    fn a_composite_name_restores_the_six_categories() {
        let mixed = Settings::C
            .set(CTYPE, b"en_US.UTF-8", none)
            .unwrap()
            .set(TIME, b"C.UTF-8", none)
            .unwrap();
        let written = name(mixed);
        assert_eq!(written, "C/en_US.UTF-8/C/C/C.UTF-8/C");
        assert_eq!(Settings::C.set(ALL, written.as_bytes(), none), Some(mixed));
        assert_eq!(Settings::C.set(ALL, b"C/C/C/C/C", none), None);
        assert_eq!(Settings::C.set(ALL, b"C/C/C/C/C/C/C", none), None);
        assert_eq!(Settings::C.set(CTYPE, b"C/C", none), None);
        let longest = Settings::C
            .set(ALL, b"en_US.UTF-8", none)
            .unwrap()
            .set(MESSAGES, b"C", none)
            .unwrap();
        let mut out = [0; NAME_CAPACITY];
        assert_eq!(longest.composite(&mut out), 5 * 12 + 1);
    }

    fn environment(
        values: &'static [(&'static [u8], &'static [u8])],
    ) -> impl Fn(&[u8]) -> Option<&'static [u8]> {
        move |variable: &[u8]| {
            values
                .iter()
                .find(|(name, _)| *name == variable)
                .map(|(_, value)| *value)
        }
    }

    #[test]
    fn an_empty_name_reads_lc_all_then_the_category_then_lang() {
        let lang = environment(&[(b"LANG", b"en_US.UTF-8"), (b"LC_TIME", b"C")]);
        assert_eq!(
            name(Settings::C.set(ALL, b"", lang).unwrap()),
            "en_US.UTF-8/en_US.UTF-8/en_US.UTF-8/en_US.UTF-8/C/en_US.UTF-8"
        );
        let all = environment(&[(b"LC_ALL", b"C.UTF-8"), (b"LC_CTYPE", b"en_US.UTF-8")]);
        assert_eq!(name(Settings::C.set(ALL, b"", all).unwrap()), "C.UTF-8");
        let empty_all = environment(&[(b"LC_ALL", b""), (b"LANG", b"en_US.UTF-8")]);
        assert_eq!(
            name(Settings::C.set(ALL, b"", empty_all).unwrap()),
            "en_US.UTF-8"
        );
        let ctype_only = environment(&[(b"LANG", b"UTF-8")]);
        assert_eq!(Settings::C.set(ALL, b"", ctype_only), None);
        let ctype_only = environment(&[(b"LANG", b"UTF-8")]);
        assert_eq!(
            Settings::C
                .set(CTYPE, b"", ctype_only)
                .map(|settings| settings.get(CTYPE)),
            Some(Locale::Utf8)
        );
        assert_eq!(Settings::C.set(ALL, b"", none), Some(Settings::C));
    }
}
