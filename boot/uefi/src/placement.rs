pub const GIGABYTE: u64 = 1 << 30;

pub const PAGE: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeRange {
    pub start: u64,
    pub end: u64,
}

pub fn page_floor(address: u64) -> u64 {
    address & !(PAGE - 1)
}

pub fn page_ceiling(address: u64) -> Option<u64> {
    address.checked_next_multiple_of(PAGE)
}

pub fn candidates_in_gigabytes(
    linked_start: u64,
    size: u64,
    free: FreeRange,
    below: u64,
) -> impl Iterator<Item = u64> {
    let offset = linked_start % GIGABYTE;
    let first = free.start / GIGABYTE;
    let last = free.end.min(below) / GIGABYTE;
    (first..=last).filter_map(move |gigabyte| {
        let start = gigabyte.checked_mul(GIGABYTE)?.checked_add(offset)?;
        let end = start.checked_add(size)?;
        let fits_gigabyte = (start / GIGABYTE) == ((end - 1) / GIGABYTE);
        (start >= free.start && end <= free.end && end <= below && fits_gigabyte).then_some(start)
    })
}

pub fn trim_command_line(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let end = bytes
        .iter()
        .position(|&byte| byte == b'\n' || byte == b'\r' || byte == 0)
        .unwrap_or(bytes.len());
    let line = &bytes[..end];
    let start = line
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(line.len());
    let finish = line
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |last| last + 1);
    &line[start..finish]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_keeps_its_offset_inside_a_gigabyte() {
        let free = FreeRange {
            start: 0x8000_0000,
            end: 0x1_0000_0000,
        };
        let found: Vec<_> =
            candidates_in_gigabytes(0x4020_0000, 0x40_0000, free, u64::MAX).collect();
        assert_eq!(found, vec![0x8020_0000, 0xc020_0000]);
    }

    #[test]
    fn no_candidate_where_the_range_is_too_small_or_too_high() {
        let free = FreeRange {
            start: 0x8030_0000,
            end: 0x8040_0000,
        };
        assert_eq!(
            candidates_in_gigabytes(0x4020_0000, 0x1000, free, u64::MAX).count(),
            0
        );
        let high = FreeRange {
            start: 0x40_0000_0000,
            end: 0x41_0000_0000,
        };
        assert_eq!(
            candidates_in_gigabytes(0x4020_0000, 0x1000, high, 256 * GIGABYTE).count(),
            0
        );
    }

    #[test]
    fn the_command_line_is_its_first_line_trimmed() {
        assert_eq!(
            trim_command_line(b"\xef\xbb\xbf eggos.display=offscreen \r\nsecond"),
            b"eggos.display=offscreen"
        );
        assert_eq!(trim_command_line(b"   \n"), b"");
    }
}
