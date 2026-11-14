//! A zone's past (D579): the transitions of a TZif file (RFC 8536, versions 1 to 4), so a time
//! before the zone's last change of rules is shown with the rule of its day, and the file's
//! footer rule after its last transition. Leap-second records are skipped: EggOS's clock
//! counts POSIX seconds.

use super::{Abbreviation, Local, Rule, UTC, parse};

/// The most transitions kept; a longer file keeps its latest ones.
pub const MAX_TRANSITIONS: usize = 512;
/// The most local time types kept.
pub const MAX_TYPES: usize = 32;
/// The most bytes of a file's abbreviations kept.
pub const MAX_ABBREVIATION_TEXT: usize = 64;
/// Where the TZif file of the IANA zone `Area/City` is, under this folder.
pub const ZONEINFO_FOLDER: &str = "/usr/share/zoneinfo";

const HEADER_BYTES: usize = 44;
const MAGIC: &[u8; 4] = b"TZif";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalType {
    offset: i64,
    daylight: bool,
    abbreviation: Abbreviation,
}

/// The transitions of a TZif file. See the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    times: [i64; MAX_TRANSITIONS],
    kinds: [u8; MAX_TRANSITIONS],
    count: usize,
    types: [LocalType; MAX_TYPES],
    type_count: usize,
    footer: Option<Rule>,
}

struct Counts {
    utc_indicators: usize,
    standard_indicators: usize,
    leaps: usize,
    times: usize,
    types: usize,
    characters: usize,
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn header(bytes: &[u8]) -> Option<(u8, Counts)> {
    if bytes.get(..4)? != MAGIC {
        return None;
    }
    let version = *bytes.get(4)?;
    let count = |index: usize| u32_at(bytes, 20 + 4 * index).map(|value| value as usize);
    Some((
        version,
        Counts {
            utc_indicators: count(0)?,
            standard_indicators: count(1)?,
            leaps: count(2)?,
            times: count(3)?,
            types: count(4)?,
            characters: count(5)?,
        },
    ))
}

impl Counts {
    fn data_bytes(&self, time_bytes: usize) -> usize {
        self.times * time_bytes
            + self.times
            + self.types * 6
            + self.characters
            + self.leaps * (time_bytes + 4)
            + self.standard_indicators
            + self.utc_indicators
    }
}

fn abbreviation_at(text: &[u8], index: usize) -> Abbreviation {
    let rest = text.get(index..).unwrap_or_default();
    let end = rest.iter().position(|&byte| byte == 0).unwrap_or(rest.len());
    Abbreviation::new(&rest[..end]).unwrap_or(UTC.abbreviation)
}

impl History {
    /// The history in the TZif file `bytes`; `None` when it is not one.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let (version, first) = header(bytes)?;
        let (data_start, counts, time_bytes) = match version {
            0 => (HEADER_BYTES, first, 4),
            _ => {
                let second_header = HEADER_BYTES + first.data_bytes(4);
                let (_, second) = header(bytes.get(second_header..)?)?;
                (second_header + HEADER_BYTES, second, 8)
            }
        };
        if counts.types == 0 || counts.types > 256 || counts.characters > 1024 {
            return None;
        }
        let data = bytes.get(data_start..data_start + counts.data_bytes(time_bytes))?;
        let times_end = counts.times * time_bytes;
        let kinds_end = times_end + counts.times;
        let types_end = kinds_end + counts.types * 6;
        let text = &data[types_end..types_end + counts.characters];
        let mut history = Self {
            times: [0; MAX_TRANSITIONS],
            kinds: [0; MAX_TRANSITIONS],
            count: 0,
            types: [LocalType {
                offset: 0,
                daylight: false,
                abbreviation: UTC.abbreviation,
            }; MAX_TYPES],
            type_count: counts.types.min(MAX_TYPES),
            footer: None,
        };
        for index in 0..history.type_count {
            let at = kinds_end + index * 6;
            let offset = i32::from_be_bytes(data[at..at + 4].try_into().ok()?);
            history.types[index] = LocalType {
                offset: i64::from(offset),
                daylight: data[at + 4] != 0,
                abbreviation: abbreviation_at(text, usize::from(data[at + 5])),
            };
        }
        let skipped = counts.times.saturating_sub(MAX_TRANSITIONS);
        for index in skipped..counts.times {
            let time = match time_bytes {
                4 => i64::from(i32::from_be_bytes(
                    data[index * 4..index * 4 + 4].try_into().ok()?,
                )),
                _ => i64::from_be_bytes(data[index * 8..index * 8 + 8].try_into().ok()?),
            };
            let kind = data[times_end + index];
            if usize::from(kind) >= history.type_count {
                return None;
            }
            if history.count > 0 && time <= history.times[history.count - 1] {
                return None;
            }
            history.times[history.count] = time;
            history.kinds[history.count] = kind;
            history.count += 1;
        }
        if version != 0 {
            let footer = bytes.get(data_start + counts.data_bytes(time_bytes)..)?;
            let footer = footer.strip_prefix(b"\n")?;
            let end = footer.iter().position(|&byte| byte == b'\n')?;
            history.footer = core::str::from_utf8(&footer[..end])
                .ok()
                .filter(|rule| !rule.is_empty())
                .and_then(parse);
        }
        Some(history)
    }

    fn local_of(&self, kind: u8) -> Local {
        let local = self.types[usize::from(kind)];
        Local {
            offset: local.offset,
            abbreviation: local.abbreviation,
            daylight: local.daylight,
        }
    }

    /// The rule for times after the last transition: the footer's, or the last type's.
    pub fn later_rule(&self) -> Rule {
        if let Some(footer) = self.footer {
            return footer;
        }
        let kind = match self.count {
            0 => 0,
            count => self.kinds[count - 1],
        };
        let local = self.types[usize::from(kind)];
        Rule {
            abbreviation: local.abbreviation,
            west: -local.offset,
            daylight: None,
        }
    }

    /// The local time at `utc`.
    pub fn at(&self, utc: i64) -> Local {
        let times = &self.times[..self.count];
        let after = times.partition_point(|&time| time <= utc);
        match after {
            0 => self.local_of(0),
            after if after == self.count && self.footer.is_some() => self.later_rule().at(utc),
            after => self.local_of(self.kinds[after - 1]),
        }
    }
}

/// A zone as EggOS shows times in it: its rule, and its past when its TZif file was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    rule: Rule,
    history: Option<History>,
}

impl Zone {
    /// A zone known by its rule alone.
    pub const fn of_rule(rule: Rule) -> Self {
        Self {
            rule,
            history: None,
        }
    }

    /// The zone whose TZif file is `tzif`, with `rule` for what the file does not say.
    pub fn with_history(rule: Rule, tzif: &[u8]) -> Self {
        let history = History::parse(tzif);
        let rule = history.as_ref().map_or(rule, History::later_rule);
        Self { rule, history }
    }

    /// Its current rule.
    pub fn rule(&self) -> &Rule {
        &self.rule
    }

    /// Whether its past was read.
    pub fn has_history(&self) -> bool {
        self.history.is_some()
    }

    /// The local time at `utc`.
    pub fn at(&self, utc: i64) -> Local {
        match &self.history {
            Some(history) => history.at(utc),
            None => self.rule.at(utc),
        }
    }

    /// The instant a local time names (seconds since the epoch, counted as if local were UTC),
    /// as [`Rule::to_utc`] chooses it.
    pub fn to_utc(&self, local: i64) -> i64 {
        if self.history.is_none() {
            return self.rule.to_utc(local);
        }
        const DAY: i64 = 86_400;
        let candidates = [
            local.saturating_sub(self.at(local.saturating_sub(DAY)).offset),
            local.saturating_sub(self.at(local).offset),
            local.saturating_sub(self.at(local.saturating_add(DAY)).offset),
        ];
        let names_local = |utc: &i64| utc.saturating_add(self.at(*utc).offset) == local;
        candidates
            .iter()
            .copied()
            .filter(names_local)
            .min()
            .unwrap_or_else(|| candidates.iter().copied().max().unwrap_or(local))
    }
}

/// The path of the TZif file of the IANA zone `name` (as `TZ` or the `time.zone` setting give
/// it, a leading `:` allowed) written into `out` with its NUL; `None` for a POSIX rule or a
/// name that is not one of [`super::ZONES`].
pub fn tzif_path<'o>(name: &str, out: &'o mut [u8; 64]) -> Option<&'o [u8]> {
    let name = name.strip_prefix(':').unwrap_or(name);
    super::find(name)?;
    let folder = ZONEINFO_FOLDER.as_bytes();
    let length = folder.len() + 1 + name.len() + 1;
    let place = out.get_mut(..length)?;
    place[..folder.len()].copy_from_slice(folder);
    place[folder.len()] = b'/';
    place[folder.len() + 1..length - 1].copy_from_slice(name.as_bytes());
    place[length - 1] = 0;
    Some(&out[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tzif(version: u8, transitions: &[(i64, u8)], types: &[(i32, bool, u8)], text: &[u8], footer: &str) -> Vec<u8> {
        let counts = |time_bytes: usize, out: &mut Vec<u8>| {
            out.extend_from_slice(MAGIC);
            out.push(version);
            out.extend_from_slice(&[0; 15]);
            for count in [0, 0, 0, transitions.len(), types.len(), text.len()] {
                out.extend_from_slice(&(count as u32).to_be_bytes());
            }
            for &(time, _) in transitions {
                match time_bytes {
                    4 => out.extend_from_slice(&(time as i32).to_be_bytes()),
                    _ => out.extend_from_slice(&time.to_be_bytes()),
                }
            }
            for &(_, kind) in transitions {
                out.push(kind);
            }
            for &(offset, daylight, index) in types {
                out.extend_from_slice(&offset.to_be_bytes());
                out.push(u8::from(daylight));
                out.push(index);
            }
            out.extend_from_slice(text);
        };
        let mut out = Vec::new();
        counts(4, &mut out);
        if version != 0 {
            counts(8, &mut out);
            out.push(b'\n');
            out.extend_from_slice(footer.as_bytes());
            out.push(b'\n');
        }
        out
    }

    #[test]
    fn a_past_offset_is_used_before_the_change_and_the_footer_after() {
        let file = tzif(
            b'2',
            &[(1_000_000_000, 1)],
            &[(10800, false, 0), (3600, false, 4)],
            b"MSK\0CET\0",
            "CET-1CEST,M3.5.0,M10.5.0/3",
        );
        let zone = Zone::with_history(UTC, &file);
        assert!(zone.has_history());
        let before = zone.at(999_999_999);
        assert_eq!((before.offset, before.abbreviation.as_str()), (10800, "MSK"));
        assert_eq!(zone.at(1_010_000_000).abbreviation.as_str(), "CET");
        let summer = zone.at(1_750_000_000);
        assert_eq!((summer.offset, summer.daylight), (7200, true));
        assert_eq!(zone.to_utc(999_999_999 + 10800), 999_999_999);
    }

    #[test]
    fn a_version_one_file_keeps_its_last_type_and_bad_files_are_refused() {
        let file = tzif(0, &[(-100, 1)], &[(0, false, 0), (-18000, false, 4)], b"LMT\0EST\0", "");
        let history = History::parse(&file).unwrap();
        assert_eq!(history.at(0).offset, -18000);
        assert_eq!(history.later_rule().standard_offset(), -18000);
        assert!(History::parse(b"TZiX").is_none());
        let unsorted = tzif(0, &[(5, 0), (1, 0)], &[(0, false, 0)], b"UTC\0", "");
        assert!(History::parse(&unsorted).is_none());
    }

    #[test]
    fn only_iana_names_have_a_file() {
        let mut out = [0u8; 64];
        assert_eq!(
            tzif_path(":Europe/Paris", &mut out),
            Some(&b"/usr/share/zoneinfo/Europe/Paris\0"[..])
        );
        assert_eq!(tzif_path("CET-1CEST", &mut out), None);
    }
}
