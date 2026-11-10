//! advapi32 (D178): the registry's functions are `registry.rs`'s (D231), and here are
//! `RegConnectRegistryW` (this computer only), the user's name (the account's: `USER` or
//! `USERNAME`, else `root`, D132) and a process's access token, a handle of its own that holds
//! no privileges, since EggOS has none (`ERROR_NOT_ALL_ASSIGNED`); there is no service manager
//! and no logon of other accounts.

use super::sync::{self, Kind};
use super::{Bool, FALSE, TRUE, error, fail};
use crate::library::Export;
use crate::text;

const ERROR_SUCCESS: u32 = 0;
const PREDEFINED_KEYS: core::ops::RangeInclusive<u32> = 0x8000_0000..=0x8000_0007;
const HKEY_LOCAL_MACHINE: u32 = 0x8000_0002;
const HKEY_USERS: u32 = 0x8000_0003;
const SE_PRIVILEGE_ENABLED: u32 = 0x2;
const PRIVILEGE_SIZE: usize = 12;
const PRIVILEGES: [&str; 35] = [
    "SeCreateTokenPrivilege",
    "SeAssignPrimaryTokenPrivilege",
    "SeLockMemoryPrivilege",
    "SeIncreaseQuotaPrivilege",
    "SeMachineAccountPrivilege",
    "SeTcbPrivilege",
    "SeSecurityPrivilege",
    "SeTakeOwnershipPrivilege",
    "SeLoadDriverPrivilege",
    "SeSystemProfilePrivilege",
    "SeSystemtimePrivilege",
    "SeProfileSingleProcessPrivilege",
    "SeIncreaseBasePriorityPrivilege",
    "SeCreatePagefilePrivilege",
    "SeCreatePermanentPrivilege",
    "SeBackupPrivilege",
    "SeRestorePrivilege",
    "SeShutdownPrivilege",
    "SeDebugPrivilege",
    "SeAuditPrivilege",
    "SeSystemEnvironmentPrivilege",
    "SeChangeNotifyPrivilege",
    "SeRemoteShutdownPrivilege",
    "SeUndockPrivilege",
    "SeSyncAgentPrivilege",
    "SeEnableDelegationPrivilege",
    "SeManageVolumePrivilege",
    "SeImpersonatePrivilege",
    "SeCreateGlobalPrivilege",
    "SeTrustedCredManAccessPrivilege",
    "SeRelabelPrivilege",
    "SeIncreaseWorkingSetPrivilege",
    "SeTimeZonePrivilege",
    "SeCreateSymbolicLinkPrivilege",
    "SeDelegateSessionUserImpersonatePrivilege",
];
const FIRST_PRIVILEGE: u32 = 2;
const USER_VARIABLES: [&[u8]; 2] = [b"USER", b"USERNAME"];
const DEFAULT_USER: &[u8] = b"root";

fn predefined(key: usize) -> Option<u32> {
    let high = key >> 32;
    let low = key as u32;
    ((high == 0 || high == 0xffff_ffff) && PREDEFINED_KEYS.contains(&low)).then_some(low)
}

fn is_this_computer(name: &[u16]) -> bool {
    let name = name
        .strip_prefix(&[u16::from(b'\\'), u16::from(b'\\')])
        .unwrap_or(name);
    let mut host = [0u8; super::system::HOST_NAME_CAPACITY];
    let length = super::system::host_name(&mut host);
    name.is_empty()
        || (name.len() == length
            && name
                .iter()
                .zip(&host[..length])
                .all(|(unit, byte)| text::upper(*unit) == u16::from(byte.to_ascii_uppercase())))
}

unsafe extern "efiapi" fn reg_connect_registry_w(
    machine: *const u16,
    key: usize,
    result: *mut usize,
) -> u32 {
    if result.is_null() {
        return error::INVALID_PARAMETER;
    }
    let local = match machine.is_null() {
        true => true,
        // SAFETY: `RegConnectRegistryW`'s contract: a NUL-terminated computer name.
        false => is_this_computer(unsafe { super::file::wide_string(machine) }),
    };
    if !local {
        return error::BAD_NETPATH;
    }
    match predefined(key) {
        Some(HKEY_LOCAL_MACHINE | HKEY_USERS) => {
            // SAFETY: the caller passes a writable `HKEY`.
            unsafe { result.write(key) };
            ERROR_SUCCESS
        }
        _ => error::INVALID_HANDLE,
    }
}

unsafe extern "efiapi" fn get_user_name_w(buffer: *mut u16, size: *mut u32) -> Bool {
    if size.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let mut name = [0u8; 256];
    let length = USER_VARIABLES
        .iter()
        .find_map(|variable| super::environment::variable(variable, &mut name))
        .filter(|length| (1..=name.len()).contains(length));
    let name: &[u8] = match length {
        Some(length) => &name[..length],
        None => DEFAULT_USER,
    };
    let mut units = [0u16; 256];
    let written = text::write_utf16(name, &mut units);
    // SAFETY: the caller passes a readable and writable `DWORD` for the size.
    let capacity = unsafe { size.read() };
    // SAFETY: `GetUserNameW`'s contract: `buffer` holds that many writable units.
    let out = unsafe { super::file::wide_buffer(buffer, capacity as usize) };
    let (result, count) = match super::file::copy_terminated(&units[..written], out) {
        Ok(copied) => (TRUE, copied + 1),
        Err(needed) => (fail(error::INSUFFICIENT_BUFFER, FALSE), needed),
    };
    // SAFETY: as above.
    unsafe { size.write(count as u32) };
    result
}

unsafe extern "efiapi" fn open_process_token(
    process: usize,
    _access: u32,
    token: *mut usize,
) -> Bool {
    if token.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    if super::processes::process_id_of(process).is_none() {
        return fail(error::INVALID_HANDLE, FALSE);
    }
    let (handle, _) = sync::insert(Kind::Token, 0);
    if handle == 0 {
        return FALSE;
    }
    // SAFETY: the caller passes a writable `HANDLE`.
    unsafe { token.write(handle) };
    TRUE
}

unsafe extern "efiapi" fn lookup_privilege_value_w(
    system: *const u16,
    name: *const u16,
    luid: *mut u64,
) -> Bool {
    if name.is_null() || luid.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `LookupPrivilegeValueW`'s contract: a NUL-terminated computer name, or null.
    if !system.is_null() && !is_this_computer(unsafe { super::file::wide_string(system) }) {
        return fail(error::BAD_NETPATH, FALSE);
    }
    // SAFETY: as above: a NUL-terminated privilege name.
    let name = unsafe { super::file::wide_string(name) };
    let found = PRIVILEGES.iter().position(|privilege| {
        privilege.len() == name.len()
            && privilege
                .bytes()
                .zip(name)
                .all(|(byte, unit)| u16::from(byte.to_ascii_uppercase()) == text::upper(*unit))
    });
    match found {
        Some(index) => {
            // SAFETY: the caller passes a writable `LUID`.
            unsafe { luid.write_unaligned(u64::from(FIRST_PRIVILEGE + index as u32)) };
            TRUE
        }
        None => fail(error::NO_SUCH_PRIVILEGE, FALSE),
    }
}

fn is_token(handle: usize) -> bool {
    sync::change(handle, |kind| (matches!(kind, Kind::Token), false)) == Some(true)
}

unsafe extern "efiapi" fn adjust_token_privileges(
    token: usize,
    disable_all: Bool,
    new_state: *const u8,
    buffer_length: u32,
    previous: *mut u8,
    returned: *mut u32,
) -> Bool {
    if !is_token(token) {
        return fail(error::INVALID_HANDLE, FALSE);
    }
    if !previous.is_null() {
        if (buffer_length as usize) < core::mem::size_of::<u32>() {
            if !returned.is_null() {
                // SAFETY: the caller passes a writable `DWORD`, or null.
                unsafe { returned.write(core::mem::size_of::<u32>() as u32) };
            }
            return fail(error::INSUFFICIENT_BUFFER, FALSE);
        }
        // SAFETY: the caller passes `buffer_length` writable bytes: no privilege changed.
        unsafe { previous.cast::<u32>().write_unaligned(0) };
        if !returned.is_null() {
            // SAFETY: as above.
            unsafe { returned.write(core::mem::size_of::<u32>() as u32) };
        }
    }
    let mut enabling = false;
    if disable_all == FALSE {
        if new_state.is_null() {
            return fail(error::INVALID_PARAMETER, FALSE);
        }
        // SAFETY: `AdjustTokenPrivileges`' contract: a readable `TOKEN_PRIVILEGES`, its count
        // first.
        let count = unsafe { new_state.cast::<u32>().read_unaligned() } as usize;
        for index in 0..count {
            let attributes = new_state.wrapping_add(4 + index * PRIVILEGE_SIZE + 8);
            // SAFETY: as above: `count` entries of a `LUID` and its attributes follow.
            if unsafe { attributes.cast::<u32>().read_unaligned() } & SE_PRIVILEGE_ENABLED != 0 {
                enabling = true;
            }
        }
    }
    let code = if enabling {
        error::NOT_ALL_ASSIGNED
    } else {
        error::SUCCESS
    };
    super::thread_environment::set_last_error(code);
    TRUE
}

extern "efiapi" fn create_process_with_logon_w() -> Bool {
    fail(error::NOT_SUPPORTED, FALSE)
}

extern "efiapi" fn open_sc_manager_w(
    _machine: *const u16,
    _database: *const u16,
    _access: u32,
) -> usize {
    fail(error::CALL_NOT_IMPLEMENTED, 0)
}

extern "efiapi" fn lock_service_database(_manager: usize) -> usize {
    fail(error::INVALID_HANDLE, 0)
}

extern "efiapi" fn unlock_service_database(_lock: usize) -> Bool {
    fail(error::INVALID_SERVICE_LOCK, FALSE)
}

extern "efiapi" fn close_service_handle(_handle: usize) -> Bool {
    fail(error::INVALID_HANDLE, FALSE)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"RegConnectRegistryW" => reg_connect_registry_w,
    b"GetUserNameW" => get_user_name_w,
    b"OpenProcessToken" => open_process_token,
    b"LookupPrivilegeValueW" => lookup_privilege_value_w,
    b"AdjustTokenPrivileges" => adjust_token_privileges,
    b"CreateProcessWithLogonW" => create_process_with_logon_w,
    b"OpenSCManagerW" => open_sc_manager_w,
    b"LockServiceDatabase" => lock_service_database,
    b"UnlockServiceDatabase" => unlock_service_database,
    b"CloseServiceHandle" => close_service_handle,
];
