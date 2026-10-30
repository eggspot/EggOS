//! Native functions that call translated code (D191, slice 39e2c): a page of thunks, each an
//! AAPCS64 function that moves its first six integer arguments one register up, loads the guest
//! address of its slot in the data page that follows into `x0` and branches to the layer's
//! entry (whose address is the data page's word [`ENTRY_SLOT`]), which calls the guest
//! function through the entry trampoline. A slot is written once, before its thunk is handed
//! out, so the code page never changes after it is made executable.

use alloc::vec::Vec;

/// The bytes of one thunk.
pub const THUNK_BYTES: usize = 64;
/// The bytes of the code page, and of the data page after it.
pub const PAGE_BYTES: usize = 4096;
/// The thunks of one code page.
pub const THUNKS_PER_PAGE: usize = PAGE_BYTES / THUNK_BYTES;
/// The data page's word holding the layer's entry.
pub const ENTRY_SLOT: usize = THUNKS_PER_PAGE;

const MOVE_REGISTER: u32 = 0xAA00_03E0;
const LOAD_LITERAL: u32 = 0x5800_0000;
const BRANCH_X16: u32 = 0xD61F_0200;
const BREAKPOINT: u32 = 0xD420_0000;
const ARGUMENT_REGISTERS: u32 = 6;
const LOAD_GUEST_WORD: usize = 6;
const LOAD_ENTRY_WORD: usize = 7;
const X16: u32 = 16;

fn move_register(destination: u32, source: u32) -> u32 {
    MOVE_REGISTER | (source << 16) | destination
}

fn load_literal(register: u32, from: usize, to: usize) -> u32 {
    let words = ((to - from) / 4) as u32;
    LOAD_LITERAL | ((words & 0x7_FFFF) << 5) | register
}

/// Where the data word of thunk `index` lies, from the code page's start.
pub const fn guest_slot_offset(index: usize) -> usize {
    PAGE_BYTES + 8 * index
}

/// The words of thunk `index`, which lies `THUNK_BYTES * index` bytes into the code page.
pub fn thunk_words(index: usize) -> [u32; THUNK_BYTES / 4] {
    let start = index * THUNK_BYTES;
    let mut words = [BREAKPOINT; THUNK_BYTES / 4];
    for (word, register) in words.iter_mut().zip((1..=ARGUMENT_REGISTERS).rev()) {
        *word = move_register(register, register - 1);
    }
    words[LOAD_GUEST_WORD] = load_literal(0, start + 4 * LOAD_GUEST_WORD, guest_slot_offset(index));
    words[LOAD_ENTRY_WORD] = load_literal(
        X16,
        start + 4 * LOAD_ENTRY_WORD,
        guest_slot_offset(ENTRY_SLOT),
    );
    words[LOAD_ENTRY_WORD + 1] = BRANCH_X16;
    words
}

/// The whole code page: every thunk in order.
pub fn page_words() -> Vec<u32> {
    (0..THUNKS_PER_PAGE).flat_map(thunk_words).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thunk_shifts_the_arguments_then_loads_its_slot_and_the_entry() {
        let words = thunk_words(0);
        assert_eq!(
            words[..6],
            [
                0xAA05_03E6,
                0xAA04_03E5,
                0xAA03_03E4,
                0xAA02_03E3,
                0xAA01_03E2,
                0xAA00_03E1
            ]
        );
        assert_eq!(words[6], LOAD_LITERAL | (((4096 - 24) / 4) << 5));
        assert_eq!(words[7], LOAD_LITERAL | (((4096 + 512 - 28) / 4) << 5) | 16);
        assert_eq!(words[8], 0xD61F_0200);
        assert!(words[9..].iter().all(|&word| word == BREAKPOINT));
    }

    #[test]
    fn each_thunk_reaches_its_own_slot() {
        for index in [1, 17, THUNKS_PER_PAGE - 1] {
            let words = thunk_words(index);
            let at = index * THUNK_BYTES + 4 * LOAD_GUEST_WORD;
            let reach = ((words[LOAD_GUEST_WORD] >> 5) & 0x7_FFFF) as usize * 4;
            assert_eq!(at + reach, guest_slot_offset(index));
            let entry_at = index * THUNK_BYTES + 4 * LOAD_ENTRY_WORD;
            let entry_reach = ((words[LOAD_ENTRY_WORD] >> 5) & 0x7_FFFF) as usize * 4;
            assert_eq!(entry_at + entry_reach, guest_slot_offset(ENTRY_SLOT));
        }
    }

    #[test]
    fn the_page_holds_every_thunk_and_its_data_fits_the_next_page() {
        assert_eq!(page_words().len() * 4, PAGE_BYTES);
        assert!(guest_slot_offset(ENTRY_SLOT) + 8 <= 2 * PAGE_BYTES);
    }
}
