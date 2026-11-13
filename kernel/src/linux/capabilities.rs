//! POSIX capabilities as Linux reports them (D557): `capget` and `capset` with the version 3
//! header, and `prctl`'s bounding-set calls. A process starts with every capability when its
//! effective user id is root's and none otherwise; `capset` may only lower them, and a
//! capability dropped from the bounding set never comes back. The kernel's checks still decide
//! by user id (root or not): the sets are kept and reported, so programs that drop privileges by
//! capabilities (and check that they did) work, but holding one does not yet grant anything.

use super::Errno;
use crate::process::Process;
use eggos_hal::Platform;

/// `_LINUX_CAPABILITY_VERSION_3`.
const VERSION_3: u32 = 0x2008_0522;
/// Capabilities 0 to 40 (`CAP_CHECKPOINT_RESTORE`).
const LAST_CAPABILITY: u32 = 40;
const ALL: u64 = (1 << (LAST_CAPABILITY + 1)) - 1;

/// A process's capability sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub effective: u64,
    pub permitted: u64,
    pub inheritable: u64,
    pub bounding: u64,
}

impl Capabilities {
    /// What a process whose effective user id is `user` starts with.
    pub const fn for_user(user: u32) -> Self {
        let held = if user == 0 { ALL } else { 0 };
        Self {
            effective: held,
            permitted: held,
            inheritable: 0,
            bounding: ALL,
        }
    }
}

fn current<P: Platform>(process: &Process<P>) -> Capabilities {
    let state = process.credentials_and_limits().lock();
    state.capabilities.unwrap_or_else(|| {
        Capabilities::for_user(
            state
                .credentials
                .ids(super::credentials::Kind::User)
                .effective,
        )
    })
}

fn read_header<P: Platform>(process: &Process<P>, header: u64) -> Result<(u32, i32), Errno> {
    let mut bytes = [0u8; 8];
    process
        .read_user_memory(header, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    let version = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let pid = i32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    Ok((version, pid))
}

/// `capget(header, data)`: the caller's sets (pid 0 or its own) as two `cap_user_data_t`
/// records (low and high 32 bits). As Linux: a header with another version gets version 3
/// written back and `EINVAL`, `ESRCH` for another process.
pub fn capget<P: Platform>(process: &Process<P>, header: u64, data: u64) -> Result<u64, Errno> {
    let (version, pid) = read_header(process, header)?;
    if version != VERSION_3 {
        let _ = process.write_user_memory(header, &VERSION_3.to_ne_bytes());
        return Err(Errno::EINVAL);
    }
    if pid != 0 && pid as u32 != process.id().get() {
        return Err(Errno::ESRCH);
    }
    if data == 0 {
        return Ok(0);
    }
    let held = current(process);
    let mut bytes = [0u8; 24];
    for (half, shift) in [(0, 0), (1, 32)] {
        let at = half * 12;
        bytes[at..at + 4].copy_from_slice(&((held.effective >> shift) as u32).to_ne_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&((held.permitted >> shift) as u32).to_ne_bytes());
        bytes[at + 8..at + 12].copy_from_slice(&((held.inheritable >> shift) as u32).to_ne_bytes());
    }
    process
        .write_user_memory(data, &bytes)
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

/// `capset(header, data)`: sets the caller's sets. As Linux: `EPERM` when the new permitted set
/// is not inside the old one, or the effective set not inside the new permitted one, or the
/// inheritable set grows past the permitted and bounding sets.
pub fn capset<P: Platform>(process: &Process<P>, header: u64, data: u64) -> Result<u64, Errno> {
    let (version, pid) = read_header(process, header)?;
    if version != VERSION_3 {
        return Err(Errno::EINVAL);
    }
    if pid != 0 && pid as u32 != process.id().get() {
        return Err(Errno::EPERM);
    }
    let mut bytes = [0u8; 24];
    process
        .read_user_memory(data, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    let word = |at: usize| u64::from(u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]));
    let effective = word(0) | word(12) << 32;
    let permitted = word(4) | word(16) << 32;
    let inheritable = word(8) | word(20) << 32;
    let old = current(process);
    if permitted & !old.permitted != 0
        || effective & !permitted != 0
        || inheritable & !(old.inheritable | old.permitted) != 0
        || inheritable & !(old.inheritable | old.bounding) != 0
    {
        return Err(Errno::EPERM);
    }
    process.credentials_and_limits().lock().capabilities = Some(Capabilities {
        effective,
        permitted,
        inheritable,
        bounding: old.bounding,
    });
    Ok(0)
}

/// `PR_CAPBSET_READ`: whether capability `capability` is in the bounding set.
pub fn bounding_holds<P: Platform>(process: &Process<P>, capability: u64) -> Result<u64, Errno> {
    if capability > u64::from(LAST_CAPABILITY) {
        return Err(Errno::EINVAL);
    }
    Ok(current(process).bounding >> capability & 1)
}

/// `PR_CAPBSET_DROP`: takes `capability` out of the bounding set for good; `EPERM` for a
/// caller without `CAP_SETPCAP` (8) in its effective set.
pub fn drop_from_bounding<P: Platform>(process: &Process<P>, capability: u64) -> Result<u64, Errno> {
    const CAP_SETPCAP: u64 = 8;
    if capability > u64::from(LAST_CAPABILITY) {
        return Err(Errno::EINVAL);
    }
    let mut held = current(process);
    if held.effective >> CAP_SETPCAP & 1 == 0 {
        return Err(Errno::EPERM);
    }
    held.bounding &= !(1 << capability);
    process.credentials_and_limits().lock().capabilities = Some(held);
    Ok(0)
}
