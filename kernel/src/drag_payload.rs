//! What a drag between windows carries (D393): one payload the window server keeps while the
//! drag is on (`START_DRAG`), its kind (paths one per line, or plain text) and a serial that
//! counts the drags, read by the window it is dropped on (`GET_DRAG`). Like the clipboard
//! (D197), it is on the heap only while it holds something.

use crate::sync::LeafSpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The most bytes one drag carries.
pub const CAPACITY: usize = 1 << 20;

struct Payload {
    bytes: Option<Arc<Vec<u8>>>,
    kind: u32,
    serial: u32,
}

static PAYLOAD: LeafSpinLock<Payload> = LeafSpinLock::new_leaf(Payload {
    bytes: None,
    kind: 0,
    serial: 0,
});

/// Makes `bytes` of `kind` the drag's payload, the previous one dropped once no reader holds
/// it; the new serial.
pub fn set(kind: u32, bytes: Vec<u8>) -> u32 {
    let bytes = (!bytes.is_empty()).then(|| Arc::new(bytes));
    let (previous, serial) = {
        let mut payload = PAYLOAD.lock();
        payload.serial = payload.serial.wrapping_add(1);
        payload.kind = kind;
        (core::mem::replace(&mut payload.bytes, bytes), payload.serial)
    };
    drop(previous);
    serial
}

/// The payload, its kind and its serial.
pub fn get() -> (Option<Arc<Vec<u8>>>, u32, u32) {
    let payload = PAYLOAD.lock();
    (payload.bytes.clone(), payload.kind, payload.serial)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_replaces_the_payload_and_counts_up_the_serial() {
        let first = set(1, b"/home/root/a.txt".to_vec());
        let (bytes, kind, serial) = get();
        assert_eq!(bytes.as_deref().map(Vec::as_slice), Some(&b"/home/root/a.txt"[..]));
        assert_eq!((kind, serial), (1, first));
        assert_eq!(set(2, Vec::new()), first.wrapping_add(1));
        assert_eq!(get().0, None);
    }
}
