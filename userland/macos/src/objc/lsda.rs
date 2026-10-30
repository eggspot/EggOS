//! A function's language-specific data area (D175), the `__gcc_except_tab` table clang writes
//! for each function with `@try`, `@catch`, `@finally` or cleanups: its call sites, each with
//! the landing pad that handles an exception thrown from it and a chain of actions (a catch of
//! one type, or a cleanup). [`search`] reads the call site of a program counter and says what an
//! exception passing through it does there. Pure logic with host tests.

const OMIT: u8 = 0xff;
const FORMAT_MASK: u8 = 0x0f;
const APPLICATION_MASK: u8 = 0x70;
const INDIRECT: u8 = 0x80;
const ABSOLUTE_POINTER: u8 = 0x00;
const ULEB128: u8 = 0x01;
const UDATA2: u8 = 0x02;
const UDATA4: u8 = 0x03;
const UDATA8: u8 = 0x04;
const SLEB128: u8 = 0x09;
const SDATA2: u8 = 0x0a;
const SDATA4: u8 = 0x0b;
const SDATA8: u8 = 0x0c;
const ABSOLUTE: u8 = 0x00;
const PC_RELATIVE: u8 = 0x10;

/// What an exception does in a frame at one call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// Nothing: it passes on to the caller.
    Nothing,
    /// A cleanup (`@finally`, ARC releases) runs at this landing pad, then the exception goes
    /// on.
    Cleanup(u64),
    /// A `@catch` takes it: the landing pad and the selector the pad's code compares.
    Handler { landing_pad: u64, selector: i64 },
}

struct Reader<'r> {
    at: u64,
    read: &'r dyn Fn(u64) -> Option<u8>,
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let byte = (self.read)(self.at)?;
        self.at = self.at.wrapping_add(1);
        Some(byte)
    }

    fn fixed(&mut self, bytes: u32) -> Option<u64> {
        let mut value = 0u64;
        for index in 0..bytes {
            value |= u64::from(self.byte()?) << (8 * index);
        }
        Some(value)
    }

    fn unsigned(&mut self) -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = self.byte()?;
            if shift < 64 {
                value |= u64::from(byte & 0x7f) << shift;
            }
            shift += 7;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
    }

    fn signed(&mut self) -> Option<i64> {
        let mut value = 0i64;
        let mut shift = 0;
        loop {
            let byte = self.byte()?;
            if shift < 64 {
                value |= i64::from(byte & 0x7f) << shift;
            }
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    value |= -1i64 << shift;
                }
                return Some(value);
            }
        }
    }

    fn encoded(&mut self, encoding: u8) -> Option<u64> {
        if encoding == OMIT {
            return None;
        }
        let position = self.at;
        let value = match encoding & FORMAT_MASK {
            ABSOLUTE_POINTER | UDATA8 | SDATA8 => self.fixed(8)?,
            ULEB128 => self.unsigned()?,
            UDATA2 => self.fixed(2)?,
            UDATA4 => self.fixed(4)?,
            SLEB128 => self.signed()? as u64,
            SDATA2 => self.fixed(2)? as u16 as i16 as i64 as u64,
            SDATA4 => self.fixed(4)? as u32 as i32 as i64 as u64,
            _ => return None,
        };
        if value == 0 {
            return Some(0);
        }
        let value = match encoding & APPLICATION_MASK {
            ABSOLUTE => value,
            PC_RELATIVE => position.wrapping_add(value),
            _ => return None,
        };
        match encoding & INDIRECT {
            0 => Some(value),
            _ => Reader {
                at: value,
                read: self.read,
            }
            .fixed(8),
        }
    }
}

fn entry_size(encoding: u8) -> Option<u64> {
    match encoding & FORMAT_MASK {
        ABSOLUTE_POINTER | UDATA8 | SDATA8 => Some(8),
        UDATA4 | SDATA4 => Some(4),
        UDATA2 | SDATA2 => Some(2),
        _ => None,
    }
}

/// What an exception thrown at `pc` (one before the call's return address) in the function
/// starting at `function_start` does there, from its LSDA at `lsda`. `read` reads a byte of
/// the image; `catches` says whether a `@catch` whose type information is at the given
/// address (0 for a catch-all) takes the exception. `None` for a malformed table.
pub fn search(
    lsda: u64,
    function_start: u64,
    pc: u64,
    read: &dyn Fn(u64) -> Option<u8>,
    catches: &dyn Fn(u64) -> bool,
) -> Option<Found> {
    let mut reader = Reader { at: lsda, read };
    let start_encoding = reader.byte()?;
    let landing_base = match start_encoding {
        OMIT => function_start,
        encoding => reader.encoded(encoding)?,
    };
    let type_encoding = reader.byte()?;
    let type_table = match type_encoding {
        OMIT => None,
        _ => {
            let offset = reader.unsigned()?;
            Some(reader.at.checked_add(offset)?)
        }
    };
    let call_site_encoding = reader.byte()?;
    let table_length = reader.unsigned()?;
    let actions = reader.at.checked_add(table_length)?;
    let offset = pc.checked_sub(function_start)?;
    while reader.at < actions {
        let start = reader.encoded(call_site_encoding)?;
        let length = reader.encoded(call_site_encoding)?;
        let pad = reader.encoded(call_site_encoding)?;
        let action = reader.unsigned()?;
        if offset < start {
            break;
        }
        if offset >= start.checked_add(length)? {
            continue;
        }
        if pad == 0 {
            return Some(Found::Nothing);
        }
        let landing_pad = landing_base.wrapping_add(pad);
        if action == 0 {
            return Some(Found::Cleanup(landing_pad));
        }
        return action_chain(
            actions.checked_add(action - 1)?,
            landing_pad,
            type_table,
            type_encoding,
            read,
            catches,
        );
    }
    Some(Found::Nothing)
}

fn action_chain(
    first: u64,
    landing_pad: u64,
    type_table: Option<u64>,
    type_encoding: u8,
    read: &dyn Fn(u64) -> Option<u8>,
    catches: &dyn Fn(u64) -> bool,
) -> Option<Found> {
    let mut record = first;
    let mut cleanup = false;
    for _ in 0..MAX_ACTIONS {
        let mut reader = Reader { at: record, read };
        let filter = reader.signed()?;
        let next = reader.at;
        let displacement = reader.signed()?;
        if filter > 0 {
            let table = type_table?;
            let entry =
                table.checked_sub((filter as u64).checked_mul(entry_size(type_encoding)?)?)?;
            let type_info = Reader { at: entry, read }.encoded(type_encoding)?;
            if catches(type_info) {
                return Some(Found::Handler {
                    landing_pad,
                    selector: filter,
                });
            }
        } else if filter == 0 {
            cleanup = true;
        }
        if displacement == 0 {
            return Some(match cleanup {
                true => Found::Cleanup(landing_pad),
                false => Found::Nothing,
            });
        }
        record = next.wrapping_add(displacement as u64);
    }
    None
}

const MAX_ACTIONS: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;

    const LSDA: u64 = 0x5000;
    const FUNCTION: u64 = 0x1000;
    const CATCH_TYPE: u64 = 0x7777;

    fn table() -> Vec<u8> {
        let mut bytes = vec![OMIT, 0x9b];
        let call_sites: Vec<u8> = vec![
            0x04, 0x08, 0x00, 0x00, 0x10, 0x08, 0x40, 0x00, 0x20, 0x08, 0x50, 0x01, 0x30, 0x08,
            0x60, 0x05,
        ];
        let actions: Vec<u8> = vec![0x01, 0x00, 0x00, 0x00, 0x02, 0x7d];
        let type_entries_before_base = 8;
        let after_offset = 2 + call_sites.len() + actions.len() + type_entries_before_base;
        bytes.push(after_offset as u8);
        bytes.push(ULEB128);
        bytes.push(call_sites.len() as u8);
        bytes.extend_from_slice(&call_sites);
        bytes.extend_from_slice(&actions);
        let entry_two = LSDA + bytes.len() as u64;
        let pointer_slot = 0x6000u64;
        bytes.extend_from_slice(&((pointer_slot.wrapping_sub(entry_two)) as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes
    }

    fn reader(bytes: &[u8]) -> impl Fn(u64) -> Option<u8> + '_ {
        move |address: u64| {
            if (0x6000..0x6008).contains(&address) {
                return Some(CATCH_TYPE.to_le_bytes()[(address - 0x6000) as usize]);
            }
            let at = address.checked_sub(LSDA)? as usize;
            bytes.get(at).copied()
        }
    }

    #[test]
    fn a_call_site_without_a_landing_pad_passes_the_exception_on() {
        let bytes = table();
        let read = reader(&bytes);
        let found = search(LSDA, FUNCTION, FUNCTION + 0x05, &read, &|_| true);
        assert_eq!(found, Some(Found::Nothing));
        let outside = search(LSDA, FUNCTION, FUNCTION + 0x80, &read, &|_| true);
        assert_eq!(outside, Some(Found::Nothing));
    }

    #[test]
    fn a_call_site_with_action_zero_is_a_cleanup() {
        let bytes = table();
        let read = reader(&bytes);
        let found = search(LSDA, FUNCTION, FUNCTION + 0x12, &read, &|_| false);
        assert_eq!(found, Some(Found::Cleanup(FUNCTION + 0x40)));
    }

    #[test]
    fn a_catch_all_takes_any_exception_with_its_selector() {
        let bytes = table();
        let read = reader(&bytes);
        let found = search(LSDA, FUNCTION, FUNCTION + 0x22, &read, &|address| {
            address == 0
        });
        assert_eq!(
            found,
            Some(Found::Handler {
                landing_pad: FUNCTION + 0x50,
                selector: 1,
            })
        );
    }

    #[test]
    fn a_typed_catch_is_tried_before_the_cleanup_it_chains_to() {
        let bytes = table();
        let read = reader(&bytes);
        let taken = search(LSDA, FUNCTION, FUNCTION + 0x31, &read, &|address| {
            address == CATCH_TYPE
        });
        assert_eq!(
            taken,
            Some(Found::Handler {
                landing_pad: FUNCTION + 0x60,
                selector: 2,
            })
        );
        let refused = search(LSDA, FUNCTION, FUNCTION + 0x31, &read, &|_| false);
        assert_eq!(refused, Some(Found::Cleanup(FUNCTION + 0x60)));
    }

    #[test]
    fn a_truncated_table_is_malformed() {
        let bytes = table();
        let short = &bytes[..6];
        let read = reader(short);
        assert_eq!(
            search(LSDA, FUNCTION, FUNCTION + 0x22, &read, &|_| true),
            None
        );
    }

    #[test]
    fn signed_and_unsigned_numbers_read_as_leb128() {
        let bytes = [0xe5, 0x8e, 0x26, 0x7f, 0x80, 0x7f];
        let read = |address: u64| bytes.get(address as usize).copied();
        let mut reader = Reader { at: 0, read: &read };
        assert_eq!(reader.unsigned(), Some(624_485));
        assert_eq!(reader.signed(), Some(-1));
        assert_eq!(reader.signed(), Some(-128));
    }
}
