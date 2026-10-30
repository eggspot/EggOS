//! `__weak` references (D175): ARC's `objc_storeWeak` and its kin keep a table of every weak
//! variable and the object it names, behind one lock. A weakly referenced object carries a mark
//! in its reference-count word, and when it is freed every variable naming it becomes nil; a
//! load retains the object only if it is not already being deallocated, so a weak variable
//! never yields a freed object. Objects the runtime never frees (constants, classes) are stored
//! without an entry.

use super::abi::Id;
use super::buffer::Buffer;
use super::reference_count::{self, objc_autorelease, objc_release};
use crate::libsystem::Exclusive;

static TABLE: Exclusive<Buffer<(usize, usize)>> = Exclusive::new(Buffer::new());

fn forget(table: &mut Buffer<(usize, usize)>, location: *mut Id) {
    let entries = table.as_mut_slice();
    if let Some(at) = entries
        .iter()
        .position(|&(_, slot)| slot == location as usize)
    {
        let last = entries.len() - 1;
        entries.swap(at, last);
        table.pop();
    }
}

fn store(location: *mut Id, object: Id, known: bool) -> Id {
    TABLE.with(|table| {
        if known {
            forget(table, location);
        }
        let stored = match object.is_null() {
            true => object,
            false if !reference_count::is_counted(object) => object,
            false if reference_count::is_deallocating(object) => core::ptr::null_mut(),
            false => {
                if table.push((object as usize, location as usize)) {
                    reference_count::mark_weakly_referenced(object);
                    object
                } else {
                    core::ptr::null_mut()
                }
            }
        };
        // SAFETY: the program passes the address of its weak variable.
        unsafe { location.write(stored) };
        stored
    })
}

/// `objc_initWeak`: makes the new weak variable at `location` name `object`.
///
/// # Safety
/// `location` must be the address of a weak variable not yet initialized.
pub unsafe extern "C" fn objc_init_weak(location: *mut Id, object: Id) -> Id {
    store(location, object, false)
}

/// `objc_storeWeak`: makes the weak variable at `location` name `object` instead.
///
/// # Safety
/// `location` must be the address of a weak variable the runtime initialized.
pub unsafe extern "C" fn objc_store_weak(location: *mut Id, object: Id) -> Id {
    store(location, object, true)
}

/// `objc_loadWeakRetained`: the object the weak variable names, retained, or nil.
///
/// # Safety
/// `location` must be the address of a weak variable the runtime initialized.
pub unsafe extern "C" fn objc_load_weak_retained(location: *mut Id) -> Id {
    let (object, retained) = TABLE.with(|_| {
        // SAFETY: the program passes the address of its weak variable.
        let object = unsafe { location.read() };
        if object.is_null() || !reference_count::is_counted(object) {
            return (object, true);
        }
        match reference_count::try_retain(object) {
            Some(retained) => (object, retained),
            None => (core::ptr::null_mut(), true),
        }
    });
    match retained {
        true => object,
        false => reference_count::objc_retain(object),
    }
}

unsafe extern "C" fn objc_load_weak(location: *mut Id) -> Id {
    // SAFETY: the caller's promise.
    objc_autorelease(unsafe { objc_load_weak_retained(location) })
}

/// `objc_destroyWeak`: forgets the weak variable at `location`.
///
/// # Safety
/// `location` must be the address of a weak variable the runtime initialized.
pub unsafe extern "C" fn objc_destroy_weak(location: *mut Id) {
    store(location, core::ptr::null_mut(), true);
}

unsafe extern "C" fn objc_copy_weak(to: *mut Id, from: *mut Id) {
    // SAFETY: the caller's promise.
    let object = unsafe { objc_load_weak_retained(from) };
    store(to, object, false);
    objc_release(object);
}

unsafe extern "C" fn objc_move_weak(to: *mut Id, from: *mut Id) {
    // SAFETY: the caller's promise.
    unsafe { objc_copy_weak(to, from) };
    store(from, core::ptr::null_mut(), true);
}

/// Makes every weak variable naming `object`, which is being freed, nil.
pub fn clear(object: Id) {
    TABLE.with(|table| {
        let mut index = 0;
        while let Some(&(named, slot)) = table.as_slice().get(index) {
            if named != object as usize {
                index += 1;
                continue;
            }
            // SAFETY: a weak variable the program registered and has not destroyed.
            unsafe { (slot as *mut Id).write(core::ptr::null_mut()) };
            let entries = table.as_mut_slice();
            let last = entries.len() - 1;
            entries.swap(index, last);
            table.pop();
        }
    });
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_objc_initWeak" => objc_init_weak as *const () as usize,
        b"_objc_storeWeak" => objc_store_weak as *const () as usize,
        b"_objc_loadWeakRetained" => objc_load_weak_retained as *const () as usize,
        b"_objc_loadWeak" => objc_load_weak as *const () as usize,
        b"_objc_destroyWeak" => objc_destroy_weak as *const () as usize,
        b"_objc_copyWeak" => objc_copy_weak as *const () as usize,
        b"_objc_moveWeak" => objc_move_weak as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}
