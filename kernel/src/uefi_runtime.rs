//! UEFI runtime services (D781): the firmware's clock (`GetTime`, `SetTime`) and its variables
//! (`GetVariable`, `GetNextVariableName`, `SetVariable`), which a machine booted by EggOS's
//! loader keeps after boot. The loader moved them to the kernel's direct map with
//! `SetVirtualAddressMap` and the kernel maps their ranges there ([`crate::paging`]), so they
//! are called in place from any thread and any address space, one call at a time, with the
//! CPU's FP/SIMD registers lent to the firmware and given back ([`crate::thread::with_fp_lent`]).
//!
//! The clock becomes the machine's real-time clock when no driver registered one (Arm machines
//! without a PL031, D150); the variables are what `/sys/firmware/efi/efivars` shows
//! ([`crate::vfs::efivarfs`]).

use crate::sync::SpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_void;
use eggos_calendar::DateTime;
use eggos_drivers::rtc::{NANOSECONDS_PER_SECOND, RealTimeClock, RtcError};
use eggos_hal::Platform;

const SYSTEM_TABLE_SIGNATURE: u64 = u64::from_le_bytes(*b"IBI SYST");
const RUNTIME_SIGNATURE: u64 = u64::from_le_bytes(*b"RUNTSERV");
const RUNTIME_SERVICES_AT: usize = 88;
const GET_TIME_AT: usize = 24;
const SET_TIME_AT: usize = 32;
const GET_VARIABLE_AT: usize = 72;
const GET_NEXT_VARIABLE_NAME_AT: usize = 80;
const SET_VARIABLE_AT: usize = 88;
const UNSPECIFIED_TIME_ZONE: i16 = 0x07ff;
const ERROR_BIT: usize = 1 << (usize::BITS - 1);
/// The firmware's answer that a buffer is too small for what it holds.
pub const BUFFER_TOO_SMALL: usize = ERROR_BIT | 5;
/// The firmware's answer that no such variable exists, or no more names follow.
pub const NOT_FOUND: usize = ERROR_BIT | 14;
const LARGEST_VARIABLE: usize = 64 * 1024;
const LARGEST_NAME_UNITS: usize = 512;

/// A variable's vendor GUID, in the byte order UEFI stores it.
pub type Guid = [u8; 16];

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct EfiTime {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    pad1: u8,
    nanosecond: u32,
    time_zone: i16,
    daylight: u8,
    pad2: u8,
}

type GetTime = unsafe extern "efiapi" fn(*mut EfiTime, *mut c_void) -> usize;
type SetTime = unsafe extern "efiapi" fn(*const EfiTime) -> usize;
type GetVariable =
    unsafe extern "efiapi" fn(*const u16, *const Guid, *mut u32, *mut usize, *mut u8) -> usize;
type GetNextVariableName = unsafe extern "efiapi" fn(*mut usize, *mut u16, *mut Guid) -> usize;
type SetVariable =
    unsafe extern "efiapi" fn(*const u16, *const Guid, u32, usize, *const u8) -> usize;

#[derive(Clone, Copy)]
struct Services {
    get_time: GetTime,
    set_time: SetTime,
    get_variable: GetVariable,
    get_next_variable_name: GetNextVariableName,
    set_variable: SetVariable,
    lend_fp: fn(&mut dyn FnMut()),
}

static SERVICES: SpinLock<Option<Services>> = SpinLock::new(None);

fn lend_fp<P: Platform>(call: &mut dyn FnMut()) {
    crate::thread::with_fp_lent::<P, ()>(call);
}

fn read_u64(address: usize) -> u64 {
    // SAFETY: callers read the firmware's runtime tables, mapped for good by `crate::paging`.
    unsafe { core::ptr::read_unaligned(address as *const u64) }
}

/// Finds the runtime services the loader handed over; whether there are some to call.
pub fn start<P: Platform>() -> bool {
    let Some(handoff) = eggos_hal::handoff::handoff::<P>() else {
        return false;
    };
    if handoff.uefi_runtime_mapped != 1 || handoff.uefi_system_table == 0 {
        return false;
    }
    let offset = P::direct_map_offset();
    let system = (handoff.uefi_system_table as usize).wrapping_add(offset);
    if read_u64(system) != SYSTEM_TABLE_SIGNATURE {
        return false;
    }
    let runtime = read_u64(system + RUNTIME_SERVICES_AT) as usize;
    if runtime < offset || read_u64(runtime) != RUNTIME_SIGNATURE {
        return false;
    }
    let table_bytes = (read_u64(runtime + 8) >> 32) as usize;
    if table_bytes < SET_VARIABLE_AT + 8 {
        return false;
    }
    let pointer = |at: usize| read_u64(runtime + at) as usize;
    let pointers = [
        pointer(GET_TIME_AT),
        pointer(SET_TIME_AT),
        pointer(GET_VARIABLE_AT),
        pointer(GET_NEXT_VARIABLE_NAME_AT),
        pointer(SET_VARIABLE_AT),
    ];
    if pointers.iter().any(|&address| address < offset) {
        return false;
    }
    // SAFETY: the signed table's slots, moved into the mapped kernel half, of these types.
    let services = unsafe {
        Services {
            get_time: core::mem::transmute::<usize, GetTime>(pointers[0]),
            set_time: core::mem::transmute::<usize, SetTime>(pointers[1]),
            get_variable: core::mem::transmute::<usize, GetVariable>(pointers[2]),
            get_next_variable_name: core::mem::transmute::<usize, GetNextVariableName>(
                pointers[3],
            ),
            set_variable: core::mem::transmute::<usize, SetVariable>(pointers[4]),
            lend_fp: lend_fp::<P>,
        }
    };
    *SERVICES.lock() = Some(services);
    true
}

/// Whether runtime services can be called.
pub fn is_present() -> bool {
    SERVICES.lock().is_some()
}

fn call<R>(work: impl FnOnce(&Services) -> R) -> Option<R> {
    let services = SERVICES.lock();
    let found = (*services)?;
    let mut work = Some(work);
    let mut result = None;
    (found.lend_fp)(&mut || {
        if let Some(work) = work.take() {
            result = Some(work(&found));
        }
    });
    drop(services);
    result
}

fn is_error(status: usize) -> bool {
    status & ERROR_BIT != 0
}

fn time_to_unix(time: &EfiTime) -> Option<u64> {
    let moment = DateTime {
        year: i64::from(time.year),
        month: u32::from(time.month),
        day: u32::from(time.day),
        hour: u32::from(time.hour),
        minute: u32::from(time.minute),
        second: u32::from(time.second),
    };
    let mut seconds = moment.to_unix()?;
    if time.time_zone != UNSPECIFIED_TIME_ZONE {
        seconds = seconds.checked_sub(i64::from(time.time_zone) * 60)?;
    }
    let seconds = u64::try_from(seconds).ok()?;
    seconds
        .checked_mul(NANOSECONDS_PER_SECOND)?
        .checked_add(u64::from(time.nanosecond).min(NANOSECONDS_PER_SECOND - 1))
}

fn unix_to_time(nanoseconds: u64) -> Option<EfiTime> {
    let seconds = i64::try_from(nanoseconds / NANOSECONDS_PER_SECOND).ok()?;
    let moment = DateTime::from_unix(seconds);
    if !(1900..=9999).contains(&moment.year) {
        return None;
    }
    Some(EfiTime {
        year: moment.year as u16,
        month: moment.month as u8,
        day: moment.day as u8,
        hour: moment.hour as u8,
        minute: moment.minute as u8,
        second: moment.second as u8,
        nanosecond: (nanoseconds % NANOSECONDS_PER_SECOND) as u32,
        time_zone: 0,
        ..EfiTime::default()
    })
}

/// The firmware's clock as a real-time clock.
#[derive(Debug, Clone, Copy)]
pub struct FirmwareClock;

impl RealTimeClock for FirmwareClock {
    fn name(&self) -> &'static str {
        "uefi"
    }

    fn read(&self) -> Result<u64, RtcError> {
        let mut time = EfiTime::default();
        // SAFETY: `time` is a writable `EFI_TIME`; the capabilities pointer may be null.
        let status = call(|services| unsafe {
            (services.get_time)(&mut time, core::ptr::null_mut())
        })
        .ok_or(RtcError::Invalid)?;
        if is_error(status) {
            return Err(RtcError::Busy);
        }
        time_to_unix(&time).ok_or(RtcError::Invalid)
    }

    fn write(&self, nanoseconds: u64) -> Result<(), RtcError> {
        let time = unix_to_time(nanoseconds).ok_or(RtcError::OutOfRange)?;
        // SAFETY: `time` is a valid `EFI_TIME` the firmware only reads.
        let status = call(|services| unsafe { (services.set_time)(&time) })
            .ok_or(RtcError::Invalid)?;
        match is_error(status) {
            true => Err(RtcError::OutOfRange),
            false => Ok(()),
        }
    }
}

/// Registers the firmware's clock as the real-time clock, when there are runtime services and
/// no driver registered another; whether it did.
pub fn register_clock() -> bool {
    is_present() && crate::wall_clock::register(Box::new(FirmwareClock))
}

fn name_units(name: &[u16]) -> Vec<u16> {
    let mut units: Vec<u16> = name.iter().copied().take_while(|&unit| unit != 0).collect();
    units.push(0);
    units
}

/// Variable `name` (UTF-16, without its NUL) of vendor `guid`: its attributes and data;
/// `Err` with the firmware's status, [`NOT_FOUND`] when it does not exist.
pub fn get_variable(name: &[u16], guid: &Guid) -> Result<(u32, Vec<u8>), usize> {
    let name = name_units(name);
    let mut data = alloc::vec![0u8; 1024];
    loop {
        let mut attributes = 0u32;
        let mut size = data.len();
        // SAFETY: `name` is NUL-ended and `data` writable for `size` bytes; the rest are live.
        let status = call(|services| unsafe {
            (services.get_variable)(
                name.as_ptr(),
                guid,
                &mut attributes,
                &mut size,
                data.as_mut_ptr(),
            )
        })
        .ok_or(NOT_FOUND)?;
        match status {
            BUFFER_TOO_SMALL if size > data.len() && size <= LARGEST_VARIABLE => {
                data.resize(size, 0);
            }
            status if is_error(status) => return Err(status),
            _ => {
                data.truncate(size);
                return Ok((attributes, data));
            }
        }
    }
}

/// Sets variable `name` of vendor `guid` to `data` with `attributes`; empty `data` with no
/// append bit deletes it. `Err` with the firmware's status.
pub fn set_variable(name: &[u16], guid: &Guid, attributes: u32, data: &[u8]) -> Result<(), usize> {
    let name = name_units(name);
    // SAFETY: `name` is NUL-ended, `guid` is valid, and `data` is readable for its length.
    let status = call(|services| unsafe {
        (services.set_variable)(name.as_ptr(), guid, attributes, data.len(), data.as_ptr())
    })
    .ok_or(NOT_FOUND)?;
    match is_error(status) {
        true => Err(status),
        false => Ok(()),
    }
}

/// Every variable's name (without its NUL) and vendor GUID, in the firmware's order.
pub fn variable_names() -> Vec<(Vec<u16>, Guid)> {
    let mut names = Vec::new();
    let mut name = alloc::vec![0u16; LARGEST_NAME_UNITS];
    let mut guid: Guid = [0; 16];
    loop {
        let mut size = name.len() * 2;
        // SAFETY: `name` (NUL-ended) is writable for `size` bytes and `guid` is live.
        let Some(status) = call(|services| unsafe {
            (services.get_next_variable_name)(&mut size, name.as_mut_ptr(), &mut guid)
        }) else {
            return names;
        };
        if is_error(status) {
            return names;
        }
        let units: Vec<u16> = name.iter().copied().take_while(|&unit| unit != 0).collect();
        names.push((units, guid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_times_become_unix_nanoseconds_and_back() {
        let time = EfiTime {
            year: 2026,
            month: 11,
            day: 26,
            hour: 9,
            minute: 30,
            second: 15,
            nanosecond: 500,
            time_zone: UNSPECIFIED_TIME_ZONE,
            ..EfiTime::default()
        };
        let nanoseconds = time_to_unix(&time).unwrap();
        let back = unix_to_time(nanoseconds).unwrap();
        assert_eq!(
            (back.year, back.month, back.day, back.hour, back.second),
            (2026, 11, 26, 9, 15)
        );
        assert_eq!(back.nanosecond, 500);
        let east = EfiTime {
            time_zone: 60,
            ..time
        };
        assert_eq!(
            time_to_unix(&east).unwrap() + 3600 * NANOSECONDS_PER_SECOND,
            nanoseconds
        );
        assert!(time_to_unix(&EfiTime::default()).is_none());
    }
}
