const PT_LOAD: u32 = 1;
const PT_PHDR: u32 = 6;
const PF_X: u32 = 1;
const PROGRAM_HEADER_SIZE: usize = 56;

/// The range of addresses the executable segments of the program whose program headers are
/// `headers`, mapped at `headers_address`, occupy: from the first segment's start to the last
/// one's end. `None` without a `PT_PHDR` header, which gives the load bias.
pub fn executable_range(headers: &[u8], headers_address: u64) -> Option<(u64, u64)> {
    let field32 = |header: &[u8], at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(header.get(at..at + 4)?.try_into().ok()?))
    };
    let field64 = |header: &[u8], at: usize| -> Option<u64> {
        Some(u64::from_le_bytes(header.get(at..at + 8)?.try_into().ok()?))
    };
    let mut bias = None;
    for header in headers.chunks_exact(PROGRAM_HEADER_SIZE) {
        if field32(header, 0)? == PT_PHDR {
            bias = Some(headers_address.wrapping_sub(field64(header, 16)?));
        }
    }
    let bias = bias?;
    let mut range: Option<(u64, u64)> = None;
    for header in headers.chunks_exact(PROGRAM_HEADER_SIZE) {
        if field32(header, 0)? != PT_LOAD || field32(header, 4)? & PF_X == 0 {
            continue;
        }
        let start = bias.wrapping_add(field64(header, 16)?);
        let end = start.wrapping_add(field64(header, 40)?);
        range = Some(match range {
            Some((low, high)) => (low.min(start), high.max(end)),
            None => (start, end),
        });
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layer_s_code_range_comes_from_its_program_headers() {
        let header = |kind: u32, flags: u32, address: u64, size: u64| {
            let mut bytes = vec![0u8; PROGRAM_HEADER_SIZE];
            bytes[0..4].copy_from_slice(&kind.to_le_bytes());
            bytes[4..8].copy_from_slice(&flags.to_le_bytes());
            bytes[16..24].copy_from_slice(&address.to_le_bytes());
            bytes[40..48].copy_from_slice(&size.to_le_bytes());
            bytes
        };
        let mut headers = header(PT_PHDR, 4, 0x40, 0x150);
        headers.extend(header(PT_LOAD, 4, 0, 0x1000));
        headers.extend(header(PT_LOAD, 5, 0x1000, 0x8000));
        headers.extend(header(PT_LOAD, 6, 0x9000, 0x2000));
        assert_eq!(
            executable_range(&headers, 0x5555_0040),
            Some((0x5555_1000, 0x5555_9000))
        );
        assert_eq!(
            executable_range(&headers[PROGRAM_HEADER_SIZE..], 0x40),
            None
        );
    }
}
