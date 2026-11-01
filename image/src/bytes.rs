use crate::Error;

pub(crate) fn byte(bytes: &[u8], at: usize) -> Result<u8, Error> {
    bytes.get(at).copied().ok_or(Error::Truncated)
}

pub(crate) fn le16(bytes: &[u8], at: usize) -> Result<u16, Error> {
    let pair = bytes
        .get(at..at.checked_add(2).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)?;
    Ok(u16::from_le_bytes([pair[0], pair[1]]))
}

pub(crate) fn le32(bytes: &[u8], at: usize) -> Result<u32, Error> {
    let word = bytes
        .get(at..at.checked_add(4).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)?;
    Ok(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

pub(crate) fn be16(bytes: &[u8], at: usize) -> Result<u16, Error> {
    le16(bytes, at).map(u16::swap_bytes)
}

pub(crate) fn be32(bytes: &[u8], at: usize) -> Result<u32, Error> {
    le32(bytes, at).map(u32::swap_bytes)
}
