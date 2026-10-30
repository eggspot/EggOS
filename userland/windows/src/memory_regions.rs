//! What `VirtualQuery` (D189) reports for an address, from the process's memory as Linux lists
//! it (`/proc/self/maps`, one mapping per line, lowest first): the run of pages from the
//! address's page that share its state, protection and kind, merging adjacent mappings that
//! agree; pages mapped with no access are reserved; an address no mapping holds is free up to
//! the next mapping.

const PAGE_SIZE: usize = 4096;

/// `MEM_COMMIT`.
pub const MEM_COMMIT: u32 = 0x1000;
/// `MEM_RESERVE`.
pub const MEM_RESERVE: u32 = 0x2000;
/// `MEM_FREE`.
pub const MEM_FREE: u32 = 0x1_0000;
/// `MEM_PRIVATE`.
pub const MEM_PRIVATE: u32 = 0x2_0000;
/// `MEM_MAPPED`.
pub const MEM_MAPPED: u32 = 0x4_0000;
/// `PAGE_NOACCESS`.
pub const PAGE_NOACCESS: u32 = 0x01;
const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_EXECUTE: u32 = 0x10;
const PAGE_EXECUTE_READ: u32 = 0x20;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;

/// One `MEMORY_BASIC_INFORMATION`'s values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub base: usize,
    pub allocation_base: usize,
    pub allocation_protect: u32,
    pub size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

#[derive(Debug, Clone, Copy)]
struct Mapping {
    start: usize,
    end: usize,
    protect: u32,
    kind: u32,
}

fn hexadecimal(digits: &[u8]) -> Option<usize> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0usize, |value, digit| {
        let digit = (*digit as char).to_digit(16)?;
        value.checked_mul(16)?.checked_add(digit as usize)
    })
}

fn protection(permissions: &[u8]) -> Option<u32> {
    let flag = |at: usize, letter: u8| permissions.get(at).map(|found| *found == letter);
    Some(match (flag(0, b'r')?, flag(1, b'w')?, flag(2, b'x')?) {
        (false, false, false) => PAGE_NOACCESS,
        (true, false, false) => PAGE_READONLY,
        (_, true, false) => PAGE_READWRITE,
        (false, false, true) => PAGE_EXECUTE,
        (true, false, true) => PAGE_EXECUTE_READ,
        (_, true, true) => PAGE_EXECUTE_READWRITE,
    })
}

fn mapping(line: &[u8]) -> Option<Mapping> {
    let mut fields = line
        .split(|byte| *byte == b' ')
        .filter(|field| !field.is_empty());
    let range = fields.next()?;
    let dash = range.iter().position(|byte| *byte == b'-')?;
    let start = hexadecimal(&range[..dash])?;
    let end = hexadecimal(&range[dash + 1..])?;
    let protect = protection(fields.next()?)?;
    let inode = fields.nth(2)?;
    let kind = match inode.iter().all(|digit| *digit == b'0') {
        true => MEM_PRIVATE,
        false => MEM_MAPPED,
    };
    (start < end).then_some(Mapping {
        start,
        end,
        protect,
        kind,
    })
}

fn state_and_protect(protect: u32) -> (u32, u32) {
    match protect {
        PAGE_NOACCESS => (MEM_RESERVE, 0),
        protect => (MEM_COMMIT, protect),
    }
}

/// The region `VirtualQuery` reports for `address`, from the text of `/proc/self/maps`;
/// `end_of_user_space` is where a free region past the last mapping ends.
pub fn region_at(maps: &[u8], address: usize, end_of_user_space: usize) -> Region {
    let page = address & !(PAGE_SIZE - 1);
    let mut found: Option<(Region, Mapping)> = None;
    let mut next_start = end_of_user_space;
    for mapping in maps.split(|byte| *byte == b'\n').filter_map(mapping) {
        if let Some((region, last)) = found.as_mut() {
            let adjacent = mapping.start == last.end;
            if !adjacent || mapping.protect != last.protect || mapping.kind != last.kind {
                break;
            }
            region.size = mapping.end - region.base;
            *last = mapping;
        } else if mapping.start <= page && page < mapping.end {
            let (state, protect) = state_and_protect(mapping.protect);
            let region = Region {
                base: page,
                allocation_base: mapping.start,
                allocation_protect: mapping.protect,
                size: mapping.end - page,
                state,
                protect,
                kind: mapping.kind,
            };
            found = Some((region, mapping));
        } else if mapping.start > page {
            next_start = next_start.min(mapping.start);
        }
    }
    match found {
        Some((region, _)) => region,
        None => Region {
            base: page,
            allocation_base: 0,
            allocation_protect: 0,
            size: next_start.saturating_sub(page),
            state: MEM_FREE,
            protect: PAGE_NOACCESS,
            kind: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAPS: &[u8] = b"00400000-00401000 r--p 00000000 00:02 17 /tmp/a.exe\n\
00401000-00403000 r-xp 00001000 00:02 17 /tmp/a.exe\n\
00403000-00404000 rw-p 00000000 00:00 0\n\
00404000-00406000 rw-p 00000000 00:00 0\n\
00406000-00408000 ---p 00000000 00:00 0\n\
00500000-00501000 rw-p 00000000 00:00 0 [heap]\n";
    const END: usize = 0x8000_0000_0000;

    #[test]
    fn an_address_in_a_mapping_reports_from_its_page_to_the_mapping_s_end() {
        let region = region_at(MAPS, 0x0040_1234, END);
        assert_eq!(region.base, 0x0040_1000);
        assert_eq!(region.allocation_base, 0x0040_1000);
        assert_eq!(region.size, 0x2000);
        assert_eq!(region.state, MEM_COMMIT);
        assert_eq!(region.protect, PAGE_EXECUTE_READ);
        assert_eq!(region.kind, MEM_MAPPED);
        let region = region_at(MAPS, 0x0040_2fff, END);
        assert_eq!((region.base, region.size), (0x0040_2000, 0x1000));
        assert_eq!(region_at(MAPS, 0x0040_0000, END).protect, PAGE_READONLY);
    }

    #[test]
    fn adjacent_mappings_that_agree_are_one_region() {
        let region = region_at(MAPS, 0x0040_3010, END);
        assert_eq!(region.base, 0x0040_3000);
        assert_eq!(region.size, 0x3000);
        assert_eq!(region.protect, PAGE_READWRITE);
        assert_eq!(region.kind, MEM_PRIVATE);
    }

    #[test]
    fn pages_with_no_access_are_reserved() {
        let region = region_at(MAPS, 0x0040_7000, END);
        assert_eq!(region.state, MEM_RESERVE);
        assert_eq!(region.protect, 0);
        assert_eq!(region.allocation_protect, PAGE_NOACCESS);
        assert_eq!((region.base, region.size), (0x0040_7000, 0x1000));
    }

    #[test]
    fn an_address_no_mapping_holds_is_free_up_to_the_next_mapping() {
        let region = region_at(MAPS, 0x0040_9123, END);
        assert_eq!(region.state, MEM_FREE);
        assert_eq!(region.protect, PAGE_NOACCESS);
        assert_eq!(
            (region.base, region.size),
            (0x0040_9000, 0x0050_0000 - 0x0040_9000)
        );
        assert_eq!(region.allocation_base, 0);
        let past = region_at(MAPS, 0x0060_0000, END);
        assert_eq!((past.base, past.size), (0x0060_0000, END - 0x0060_0000));
        assert_eq!(region_at(b"", 0x1000, END).size, END - 0x1000);
    }

    #[test]
    fn lines_that_are_not_mappings_are_skipped() {
        let maps = b"garbage\n00400000-00401000 rw-p 00000000 00:00 0\nzz-1 r--p\n";
        assert_eq!(region_at(maps, 0x0040_0000, END).protect, PAGE_READWRITE);
        assert_eq!(region_at(maps, 0x0040_1000, END).state, MEM_FREE);
    }
}
