//! The old C runtime's (`msvcrt.dll`, D189) structures that MinGW programs reach into: the
//! `FILE` array `__iob_func` returns, whose entries MinGW's own `printf` locks through `_lock`
//! by their place in the array, and the critical section MinGW enters right after any other
//! `FILE` (the old runtime's `_FILEX`).

/// The bytes of one `FILE` (`struct _iobuf`) on 64-bit Windows.
pub const FILE_BYTES: usize = 48;
/// The bytes of the critical section `_FILEX` keeps right after a `FILE` the program opened.
pub const FILE_LOCK_BYTES: usize = 40;
/// The 8-byte words of a `FILE` with its critical section.
pub const FILE_WITH_LOCK_WORDS: usize = (FILE_BYTES + FILE_LOCK_BYTES) / 8;
/// The `FILE`s in the array `__iob_func` returns (`_IOB_ENTRIES`).
pub const IOB_ENTRIES: usize = 20;
/// The 8-byte words of that array.
pub const IOB_WORDS: usize = IOB_ENTRIES * FILE_BYTES / 8;
/// The first `_lock` number that belongs to the array's streams (`_STREAM_LOCKS`).
pub const FIRST_STREAM_LOCK: usize = 16;
/// The locks `_lock` and `_unlock` take: the runtime's own, then one per array stream.
pub const LOCKS: usize = FIRST_STREAM_LOCK + IOB_ENTRIES;

/// The place of the `FILE` at `file` in the array that starts at `array`, if it is one of the
/// array's entries.
pub fn iob_index(array: usize, file: usize) -> Option<usize> {
    let offset = file.checked_sub(array)?;
    let index = offset / FILE_BYTES;
    (offset % FILE_BYTES == 0 && index < IOB_ENTRIES).then_some(index)
}

/// The slot of the lock `_lock` and `_unlock` name `number`, if the runtime has it.
pub fn lock_slot(number: i32) -> Option<usize> {
    usize::try_from(number).ok().filter(|slot| *slot < LOCKS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_its_lock_is_whole_words_and_the_array_is_twenty_files() {
        assert_eq!(FILE_WITH_LOCK_WORDS * 8, 88);
        assert_eq!(IOB_WORDS * 8, 960);
        assert_eq!(LOCKS, 36);
    }

    #[test]
    fn only_the_start_of_an_entry_of_the_array_is_one_of_its_files() {
        let array = 0x1000;
        assert_eq!(iob_index(array, array), Some(0));
        assert_eq!(iob_index(array, array + 48), Some(1));
        assert_eq!(iob_index(array, array + 2 * 48), Some(2));
        assert_eq!(iob_index(array, array + 19 * 48), Some(19));
        assert_eq!(iob_index(array, array + 20 * 48), None);
        assert_eq!(iob_index(array, array + 8), None);
        assert_eq!(iob_index(array, array - 48), None);
        assert_eq!(iob_index(array, 0), None);
    }

    #[test]
    fn locks_are_numbered_from_zero_up_to_the_last_stream_s() {
        assert_eq!(lock_slot(0), Some(0));
        assert_eq!(lock_slot(17), Some(17));
        assert_eq!(lock_slot(35), Some(35));
        assert_eq!(lock_slot(36), None);
        assert_eq!(lock_slot(-1), None);
    }
}
