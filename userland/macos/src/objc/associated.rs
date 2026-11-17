//! Associated objects (D673): `objc_setAssociatedObject` and its kin keep, behind one lock, a
//! table of every (object, key) pair and the value it holds, retained or copied as its policy
//! says. Setting nil removes a pair; an object's pairs go when it is freed, or when
//! `objc_removeAssociatedObjects` asks. Values are retained, copied and released outside the
//! lock, since those messages may run the program's own code.

use super::abi::Id;
use super::buffer::Buffer;
use super::reference_count::{objc_release, objc_retain};
use crate::libsystem::Exclusive;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

const RETAIN_NONATOMIC: usize = 1;
const COPY_NONATOMIC: usize = 3;
const RETAIN: usize = 0x301;
const COPY: usize = 0x303;

#[derive(Debug, Clone, Copy)]
struct Association {
    object: usize,
    key: usize,
    value: usize,
    owned: bool,
}

static TABLE: Exclusive<Buffer<Association>> = Exclusive::new(Buffer::new());
static COUNT: AtomicUsize = AtomicUsize::new(0);

fn kept(value: Id, policy: usize) -> (Id, bool) {
    match policy {
        RETAIN | RETAIN_NONATOMIC => (objc_retain(value), true),
        COPY | COPY_NONATOMIC => (super::send(value, sel!("copy")), true),
        _ => (value, false),
    }
}

/// `objc_setAssociatedObject`: makes `object` hold `value` under `key`, kept by `policy`;
/// nil removes it.
pub extern "C" fn objc_set_associated_object(object: Id, key: *const c_void, value: Id, policy: usize) {
    if object.is_null() || key.is_null() {
        return;
    }
    let (stored, owned) = match value.is_null() {
        true => (value, false),
        false => kept(value, policy),
    };
    let fresh = Association {
        object: object as usize,
        key: key as usize,
        value: stored as usize,
        owned,
    };
    let replaced = TABLE.with(|table| {
        let entries = table.as_mut_slice();
        let at = entries
            .iter()
            .position(|entry| entry.object == fresh.object && entry.key == fresh.key);
        match (at, stored.is_null()) {
            (Some(at), true) => {
                let old = entries[at];
                let last = entries.len() - 1;
                entries.swap(at, last);
                table.pop();
                COUNT.fetch_sub(1, Ordering::Relaxed);
                Some(old)
            }
            (Some(at), false) => {
                let old = entries[at];
                entries[at] = fresh;
                Some(old)
            }
            (None, true) => None,
            (None, false) => {
                if table.push(fresh) {
                    COUNT.fetch_add(1, Ordering::Relaxed);
                    None
                } else {
                    Some(fresh)
                }
            }
        }
    });
    if let Some(old) = replaced.filter(|old| old.owned) {
        objc_release(old.value as Id);
    }
}

/// `objc_getAssociatedObject`: what `object` holds under `key`, or nil.
pub extern "C" fn objc_get_associated_object(object: Id, key: *const c_void) -> Id {
    if object.is_null() || COUNT.load(Ordering::Relaxed) == 0 {
        return core::ptr::null_mut();
    }
    TABLE.with(|table| {
        table
            .as_slice()
            .iter()
            .find(|entry| entry.object == object as usize && entry.key == key as usize)
            .map_or(core::ptr::null_mut(), |entry| entry.value as Id)
    })
}

/// `objc_removeAssociatedObjects`, and an object being freed: drops every pair of `object`.
pub extern "C" fn objc_remove_associated_objects(object: Id) {
    if object.is_null() || COUNT.load(Ordering::Relaxed) == 0 {
        return;
    }
    let mut released = Buffer::<usize>::new();
    TABLE.with(|table| {
        let mut at = 0;
        while at < table.len() {
            let entry = table.as_slice()[at];
            if entry.object != object as usize {
                at += 1;
                continue;
            }
            if entry.owned {
                released.push(entry.value);
            }
            let entries = table.as_mut_slice();
            let last = entries.len() - 1;
            entries.swap(at, last);
            table.pop();
            COUNT.fetch_sub(1, Ordering::Relaxed);
        }
    });
    for &value in released.as_slice() {
        objc_release(value as Id);
    }
    released.free();
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_objc_setAssociatedObject" => objc_set_associated_object as *const () as usize,
        b"_objc_getAssociatedObject" => objc_get_associated_object as *const () as usize,
        b"_objc_removeAssociatedObjects" => objc_remove_associated_objects as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}
