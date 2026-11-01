//! C++ run-time type information for Windows programs (D229): `dynamic_cast`
//! (`__RTDynamicCast`), `dynamic_cast<void *>` (`__RTCastToVoid`) and `typeid` of a polymorphic
//! object (`__RTtypeid`) over the program's RTTI (`rtti.rs`), and `type_info::name()`
//! (`__std_type_info_name`), undecorated once per type (`undecorate.rs`) and kept in the
//! `type_info`'s own slot. A failing reference cast throws `std::bad_cast`, `typeid` of a null
//! pointer `std::bad_typeid` and of an object without RTTI `std::__non_rtti_object`, from the
//! program's call (`thrown.rs`).

use super::super::heap;
use super::cxx::ProgramMemory;
use super::thrown::{self, throwing_entry};
use crate::library::Export;
use crate::std_exceptions::StdException;
use crate::{rtti, undecorate};
use core::sync::atomic::{AtomicU64, Ordering};

extern "efiapi" fn dynamic_cast(
    object: u64,
    vfptr_offset: i32,
    source: u64,
    target: u64,
    is_reference: i32,
) -> u64 {
    if object == 0 {
        return 0;
    }
    match rtti::dynamic_cast(
        &ProgramMemory,
        object,
        i64::from(vfptr_offset),
        source,
        target,
    ) {
        Some(result) => result,
        None if is_reference != 0 => {
            thrown::owe(StdException::BadCast);
            0
        }
        None => 0,
    }
}

extern "efiapi" fn type_id(object: u64) -> u64 {
    if object == 0 {
        thrown::owe(StdException::BadTypeid);
        return 0;
    }
    match rtti::locator(&ProgramMemory, object) {
        Some(locator) => locator.type_descriptor,
        None => {
            thrown::owe(StdException::NonRttiObject);
            0
        }
    }
}

extern "efiapi" fn cast_to_void(object: u64) -> u64 {
    if object == 0 {
        return 0;
    }
    match rtti::locator(&ProgramMemory, object) {
        Some(locator) => rtti::complete_object(&ProgramMemory, object, &locator),
        None => {
            thrown::owe(StdException::NonRttiObject);
            0
        }
    }
}

/// `__std_type_info_name(data, root)`: `data` is the `type_info`'s `__std_type_info_data`, a
/// slot for the undecorated name followed by the decorated name. The name is made once and
/// kept in the slot for the program's life (no node goes on `root`'s list, so
/// `__std_type_info_destroy_list` has nothing to free); a name this undecorator does not read
/// is given decorated, past its `.`.
unsafe extern "efiapi" fn type_info_name(data: *mut u64, _root: u64) -> *const u8 {
    // SAFETY: the slot is the first word of the `type_info`'s data, which every thread naming
    // the type reads and writes only through this function, atomically.
    let slot = unsafe { AtomicU64::from_ptr(data) };
    let kept = slot.load(Ordering::Acquire);
    if kept != 0 {
        return kept as *const u8;
    }
    // SAFETY: the decorated name follows the slot, NUL-terminated.
    let decorated = unsafe { core::ffi::CStr::from_ptr(data.wrapping_add(1).cast()) }.to_bytes();
    let name = undecorate::type_name(decorated)
        .unwrap_or_else(|| decorated.get(1..).unwrap_or(&[]).to_vec());
    let copy = heap::allocate(name.len() + 1);
    if copy.is_null() {
        return decorated.as_ptr().wrapping_add(1);
    }
    // SAFETY: `copy` holds the name and its NUL.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), copy, name.len());
        copy.add(name.len()).write(0);
    }
    match slot.compare_exchange(0, copy as u64, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => copy,
        Err(winner) => {
            // SAFETY: the copy came from this heap and no one else saw it.
            unsafe { heap::free(copy) };
            winner as *const u8
        }
    }
}

extern "efiapi" fn type_info_destroy_list(_root: u64) {}

pub(super) const EXPORTS: &[Export] = &[
    Export {
        name: b"__RTDynamicCast",
        address: throwing_entry!(dynamic_cast),
    },
    Export {
        name: b"__RTtypeid",
        address: throwing_entry!(type_id),
    },
    Export {
        name: b"__RTCastToVoid",
        address: throwing_entry!(cast_to_void),
    },
    Export {
        name: b"__std_type_info_name",
        address: export_entry!(type_info_name),
    },
    Export {
        name: b"__std_type_info_destroy_list",
        address: export_entry!(type_info_destroy_list),
    },
];
