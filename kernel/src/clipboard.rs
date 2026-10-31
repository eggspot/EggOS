//! The clipboard every app shares (D197): one UTF-8 text the window server keeps for its
//! clients (`SET_CLIPBOARD`, `GET_CLIPBOARD`), replaced whole by each copy, with a serial that
//! counts the copies so a program can tell a new one. It is on the heap only while it holds
//! something, so a machine where nothing is copied pays nothing for it.

use crate::sync::LeafSpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The most bytes one copy holds.
pub const CAPACITY: usize = 1 << 20;

struct Clipboard {
    text: Option<Arc<Vec<u8>>>,
    serial: u32,
}

static CLIPBOARD: LeafSpinLock<Clipboard> = LeafSpinLock::new_leaf(Clipboard {
    text: None,
    serial: 0,
});

/// Makes `text` the clipboard's, the previous text dropped once no reader holds it; the new
/// serial.
pub fn set(text: Vec<u8>) -> u32 {
    let text = (!text.is_empty()).then(|| Arc::new(text));
    let (previous, serial) = {
        let mut clipboard = CLIPBOARD.lock();
        clipboard.serial = clipboard.serial.wrapping_add(1);
        (
            core::mem::replace(&mut clipboard.text, text),
            clipboard.serial,
        )
    };
    drop(previous);
    serial
}

/// The clipboard's text (empty when nothing was copied) and its serial.
pub fn get() -> (Option<Arc<Vec<u8>>>, u32) {
    let clipboard = CLIPBOARD.lock();
    (clipboard.text.clone(), clipboard.serial)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_replaces_the_text_and_counts_up_the_serial() {
        let first = set(b"hello".to_vec());
        let (text, serial) = get();
        assert_eq!(
            (text.as_deref().map(Vec::as_slice), serial),
            (Some(&b"hello"[..]), first)
        );
        let second = set(Vec::new());
        assert_eq!(second, first.wrapping_add(1));
        assert_eq!(get().0, None);
    }
}
