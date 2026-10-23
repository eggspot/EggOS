//! Who a process acts as (M6, D75): its user and group ids, read with `getuid`, `geteuid`,
//! `getgid`, `getegid`, `getresuid`, `getresgid` and `getgroups`, and changed with `setuid`,
//! `setgid`, `setreuid`, `setregid`, `setresuid`, `setresgid`, `setfsuid`, `setfsgid` and
//! `setgroups`, with Linux's rules.
//!
//! A process starts as root, as Linux starts `init`: every id 0 and no supplementary groups.
//! It is privileged (Linux's `CAP_SETUID`, `CAP_SETGID` and `CAP_SYS_RESOURCE`) while its
//! effective user id is 0: without file capabilities, `capset` or `PR_SET_KEEPCAPS`, that is
//! exactly when Linux leaves those capabilities in its effective set. A privileged process may
//! set any ids; any other may only move its ids among the values it already has. A call that
//! fails changes nothing.

use super::Errno;
use crate::process::Process;
use alloc::boxed::Box;
use alloc::vec::Vec;
use eggos_hal::Platform;

/// The id that leaves an id unchanged in `setreuid`, `setresuid` and their group versions, and
/// that no user or group may have: `(uid_t) -1`.
pub const UNCHANGED: u32 = u32::MAX;

/// The most supplementary groups a process may have (Linux's `NGROUPS_MAX`).
pub const MAX_GROUPS: u32 = 65_536;

/// One kind of id's four values: the real one (who the process is), the effective one (whose
/// privileges it uses), the saved one (an effective id it may return to) and the file system
/// one (whose access files are checked with), which follows the effective one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ids {
    pub real: u32,
    pub effective: u32,
    pub saved: u32,
    pub filesystem: u32,
}

impl Ids {
    /// Root's: every id 0.
    pub const ROOT: Self = Self::all(0);

    /// Every id `id`.
    pub const fn all(id: u32) -> Self {
        Self {
            real: id,
            effective: id,
            saved: id,
            filesystem: id,
        }
    }

    /// Whether `id` is the real, effective or saved id: one an unprivileged process may move
    /// its ids to.
    fn holds(self, id: u32) -> bool {
        id == self.real || id == self.effective || id == self.saved
    }

    /// `setuid(id)`: every id `id` when `privileged`; otherwise the effective and file system
    /// ids, if `id` is the real or saved id. `EINVAL` for [`UNCHANGED`], then `EPERM`.
    pub fn set(self, id: u32, privileged: bool) -> Result<Self, Errno> {
        if id == UNCHANGED {
            return Err(Errno::EINVAL);
        }
        if privileged {
            Ok(Self::all(id))
        } else if id == self.real || id == self.saved {
            Ok(Self {
                effective: id,
                filesystem: id,
                ..self
            })
        } else {
            Err(Errno::EPERM)
        }
    }

    /// `setreuid(real, effective)`, each [`UNCHANGED`] to keep it. Unless `privileged`, the
    /// real id may only become the real or effective id, and the effective id the real,
    /// effective or saved id: `EPERM`. The saved id becomes the new effective id when the real
    /// id is given, or the effective id is given and differs from the old real id. The file
    /// system id follows the effective id.
    pub fn set_real_and_effective(
        self,
        real: u32,
        effective: u32,
        privileged: bool,
    ) -> Result<Self, Errno> {
        if !privileged
            && ((real != UNCHANGED && real != self.real && real != self.effective)
                || (effective != UNCHANGED && !self.holds(effective)))
        {
            return Err(Errno::EPERM);
        }
        let new_real = given_or(real, self.real);
        let new_effective = given_or(effective, self.effective);
        let saved = if real != UNCHANGED || (effective != UNCHANGED && effective != self.real) {
            new_effective
        } else {
            self.saved
        };
        Ok(Self {
            real: new_real,
            effective: new_effective,
            saved,
            filesystem: new_effective,
        })
    }

    /// `setresuid(real, effective, saved)`, each [`UNCHANGED`] to keep it. A call that changes
    /// nothing (each id unchanged or the one it has, the effective one also the file system
    /// id) leaves every id as it is, as Linux has since 6.4 and 6.1.y. Otherwise, unless
    /// `privileged`, each given id must be the real, effective or saved id: `EPERM`; and the
    /// file system id follows the effective id.
    pub fn set_real_effective_and_saved(
        self,
        real: u32,
        effective: u32,
        saved: u32,
        privileged: bool,
    ) -> Result<Self, Errno> {
        let keeps = |given: u32, current: u32| given == UNCHANGED || given == current;
        if keeps(real, self.real)
            && (effective == UNCHANGED
                || (effective == self.effective && effective == self.filesystem))
            && keeps(saved, self.saved)
        {
            return Ok(self);
        }
        if !privileged
            && [real, effective, saved]
                .into_iter()
                .any(|id| id != UNCHANGED && !self.holds(id))
        {
            return Err(Errno::EPERM);
        }
        let effective = given_or(effective, self.effective);
        Ok(Self {
            real: given_or(real, self.real),
            effective,
            saved: given_or(saved, self.saved),
            filesystem: effective,
        })
    }

    /// `setfsuid(id)`: the file system id becomes `id` when `privileged` or `id` is the real,
    /// effective, saved or file system id, and never [`UNCHANGED`].
    pub fn set_filesystem(self, id: u32, privileged: bool) -> Self {
        if id != UNCHANGED && (privileged || self.holds(id) || id == self.filesystem) {
            Self {
                filesystem: id,
                ..self
            }
        } else {
            self
        }
    }

    /// What `execve` makes of them: the saved and file system ids become the effective id.
    pub fn after_exec(self) -> Self {
        Self {
            saved: self.effective,
            filesystem: self.effective,
            ..self
        }
    }
}

fn given_or(id: u32, current: u32) -> u32 {
    if id == UNCHANGED { current } else { id }
}

/// Which ids a call reads or changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    User,
    Group,
}

/// A process's user and group ids and supplementary groups, once any differs from root's.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    user: Ids,
    group: Ids,
    /// Sorted, as Linux keeps them; duplicates kept.
    groups: Vec<u32>,
}

static ROOT: Record = Record {
    user: Ids::ROOT,
    group: Ids::ROOT,
    groups: Vec::new(),
};

/// A process's credentials: root's, until it first changes them, then a record of its own on
/// the heap. Most processes never change theirs, so a process costs a word for them and
/// starting one allocates nothing.
#[derive(Debug, Default)]
pub struct Credentials(Option<Box<[Record; 1]>>);

impl Credentials {
    /// Root's.
    pub const fn new() -> Self {
        Self(None)
    }

    fn record(&self) -> &Record {
        self.0.as_deref().map_or(&ROOT, |[record]| record)
    }

    /// The user or group ids.
    pub fn ids(&self, kind: Kind) -> Ids {
        match kind {
            Kind::User => self.record().user,
            Kind::Group => self.record().group,
        }
    }

    /// The supplementary groups, sorted.
    pub fn groups(&self) -> &[u32] {
        &self.record().groups
    }

    /// Whether the process is privileged: its effective user id is 0 (see the module
    /// documentation).
    pub fn is_privileged(&self) -> bool {
        self.ids(Kind::User).effective == 0
    }

    /// The process's own record, made on its first change: `ENOMEM` if there is no memory.
    fn own(&mut self) -> Result<&mut Record, Errno> {
        if self.0.is_none() {
            let mut block = Vec::new();
            block.try_reserve_exact(1).map_err(|_| Errno::ENOMEM)?;
            block.push(ROOT.clone());
            self.0 = Some(block.into_boxed_slice().try_into().unwrap());
        }
        let Some([record]) = self.0.as_deref_mut() else {
            unreachable!("made above")
        };
        Ok(record)
    }

    /// Sets the user or group ids to `ids`: `ENOMEM` if they change and there is no memory for
    /// the process's own record.
    pub fn set_ids(&mut self, kind: Kind, ids: Ids) -> Result<(), Errno> {
        if self.ids(kind) == ids {
            return Ok(());
        }
        let record = self.own()?;
        match kind {
            Kind::User => record.user = ids,
            Kind::Group => record.group = ids,
        }
        Ok(())
    }

    /// Sets the supplementary groups to `groups`, sorted: `ENOMEM` if there is no memory for
    /// the process's own record.
    fn set_groups(&mut self, mut groups: Vec<u32>) -> Result<(), Errno> {
        groups.sort_unstable();
        if self.groups() != groups {
            self.own()?.groups = groups;
        }
        Ok(())
    }

    /// What `execve` makes of them ([`Ids::after_exec`]); the real ids and groups are kept.
    /// Returns the user and group ids then.
    #[inline]
    pub fn after_exec(&mut self) -> Result<(Ids, Ids), Errno> {
        if self.0.is_none() {
            return Ok((Ids::ROOT, Ids::ROOT));
        }
        self.changed_after_exec()
    }

    #[inline(never)]
    fn changed_after_exec(&mut self) -> Result<(Ids, Ids), Errno> {
        for kind in [Kind::User, Kind::Group] {
            self.set_ids(kind, self.ids(kind).after_exec())?;
        }
        Ok((self.ids(Kind::User), self.ids(Kind::Group)))
    }
}

/// Whether a program started with `user` and `group` ids must be treated as privileged by its
/// C library (`AT_SECURE`): its effective user or group id is not its real one.
pub fn is_secure_execution(user: Ids, group: Ids) -> bool {
    user.effective != user.real || group.effective != group.real
}

fn ids<P: Platform>(process: &Process<P>, kind: Kind) -> Ids {
    process
        .credentials_and_limits()
        .lock()
        .credentials
        .ids(kind)
}

/// `getuid()` with [`Kind::User`], `getgid()` with [`Kind::Group`]: the real id.
pub fn get_real<P: Platform>(process: &Process<P>, kind: Kind) -> u64 {
    u64::from(ids(process, kind).real)
}

/// `geteuid()` with [`Kind::User`], `getegid()` with [`Kind::Group`]: the effective id.
pub fn get_effective<P: Platform>(process: &Process<P>, kind: Kind) -> u64 {
    u64::from(ids(process, kind).effective)
}

/// `getresuid(real, effective, saved)` with [`Kind::User`], `getresgid` with [`Kind::Group`]:
/// writes the three ids, in that order, each a 32-bit word, `EFAULT` at the first that is not
/// the program's writable memory (those before it written).
pub fn get_real_effective_and_saved<P: Platform>(
    process: &Process<P>,
    kind: Kind,
    real: u64,
    effective: u64,
    saved: u64,
) -> Result<u64, Errno> {
    let ids = ids(process, kind);
    for (address, id) in [
        (real, ids.real),
        (effective, ids.effective),
        (saved, ids.saved),
    ] {
        process
            .write_user_memory(address, &id.to_ne_bytes())
            .map_err(|_| Errno::EFAULT)?;
    }
    Ok(0)
}

/// Changes the user or group ids with `change`, given them and whether the process is
/// privileged, and returns 0.
fn change<P: Platform>(
    process: &Process<P>,
    kind: Kind,
    change: impl FnOnce(Ids, bool) -> Result<Ids, Errno>,
) -> Result<u64, Errno> {
    let mut state = process.credentials_and_limits().lock();
    let credentials = &mut state.credentials;
    let ids = change(credentials.ids(kind), credentials.is_privileged())?;
    credentials.set_ids(kind, ids)?;
    Ok(0)
}

/// `setuid(id)` with [`Kind::User`], `setgid(id)` with [`Kind::Group`]: see [`Ids::set`]. The
/// id is 32-bit: the register's upper half is ignored.
pub fn set<P: Platform>(process: &Process<P>, kind: Kind, id: u64) -> Result<u64, Errno> {
    change(process, kind, |ids, privileged| {
        ids.set(id as u32, privileged)
    })
}

/// `setreuid(real, effective)` with [`Kind::User`], `setregid` with [`Kind::Group`]: see
/// [`Ids::set_real_and_effective`].
pub fn set_real_and_effective<P: Platform>(
    process: &Process<P>,
    kind: Kind,
    real: u64,
    effective: u64,
) -> Result<u64, Errno> {
    change(process, kind, |ids, privileged| {
        ids.set_real_and_effective(real as u32, effective as u32, privileged)
    })
}

/// `setresuid(real, effective, saved)` with [`Kind::User`], `setresgid` with [`Kind::Group`]:
/// see [`Ids::set_real_effective_and_saved`].
pub fn set_real_effective_and_saved<P: Platform>(
    process: &Process<P>,
    kind: Kind,
    real: u64,
    effective: u64,
    saved: u64,
) -> Result<u64, Errno> {
    change(process, kind, |ids, privileged| {
        ids.set_real_effective_and_saved(real as u32, effective as u32, saved as u32, privileged)
    })
}

/// `setfsuid(id)` with [`Kind::User`], `setfsgid(id)` with [`Kind::Group`]: see
/// [`Ids::set_filesystem`]. Returns the file system id from before, whether it changed or not;
/// if there is no memory for the change, nothing changes.
pub fn set_filesystem<P: Platform>(process: &Process<P>, kind: Kind, id: u64) -> u64 {
    let mut state = process.credentials_and_limits().lock();
    let credentials = &mut state.credentials;
    let ids = credentials.ids(kind);
    let _ = credentials.set_ids(
        kind,
        ids.set_filesystem(id as u32, credentials.is_privileged()),
    );
    u64::from(ids.filesystem)
}

/// Supplementary groups copied between the program's memory and the kernel in chunks of this
/// many, on the stack, so the lock is never held across a copy.
const GROUPS_PER_COPY: usize = 64;

/// `getgroups(size, list)`: the number of supplementary groups, after writing them, sorted, to
/// `list` as 32-bit words. As Linux: `EINVAL` for a negative `size` (an `int`) or one smaller
/// than the number, unless 0, which writes nothing; then `EFAULT` if `list` is not the
/// program's writable memory.
pub fn getgroups<P: Platform>(process: &Process<P>, size: u64, list: u64) -> Result<u64, Errno> {
    let size = size as u32 as i32;
    let count = process
        .credentials_and_limits()
        .lock()
        .credentials
        .groups()
        .len();
    if size < 0 || (size != 0 && (size as usize) < count) {
        return Err(Errno::EINVAL);
    }
    if size == 0 {
        return Ok(count as u64);
    }
    let mut copied = 0;
    while copied < count {
        let mut bytes = [0u8; GROUPS_PER_COPY * 4];
        let chunk = {
            let state = process.credentials_and_limits().lock();
            let groups = state.credentials.groups().get(copied..count).unwrap_or(&[]);
            let chunk = &groups[..groups.len().min(GROUPS_PER_COPY)];
            for (word, group) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(chunk) {
                *word = group.to_ne_bytes();
            }
            chunk.len()
        };
        if chunk == 0 {
            break;
        }
        process
            .write_user_memory(list.wrapping_add(copied as u64 * 4), &bytes[..chunk * 4])
            .map_err(|_| Errno::EFAULT)?;
        copied += chunk;
    }
    Ok(count as u64)
}

/// `setgroups(size, list)`: sets the supplementary groups to the `size` 32-bit words at
/// `list`, kept sorted. As Linux: `EPERM` unless the process is privileged; `EINVAL` for a
/// `size` above [`MAX_GROUPS`] (an `int`: negative ones too); `ENOMEM`; then, word by word in
/// order, `EFAULT` for one that is not the program's memory and `EINVAL` for [`UNCHANGED`],
/// which is no group. A `size` of 0 empties the list without reading `list`.
pub fn setgroups<P: Platform>(process: &Process<P>, size: u64, list: u64) -> Result<u64, Errno> {
    let is_privileged = || {
        process
            .credentials_and_limits()
            .lock()
            .credentials
            .is_privileged()
    };
    if !is_privileged() {
        return Err(Errno::EPERM);
    }
    let size = size as u32;
    if size > MAX_GROUPS {
        return Err(Errno::EINVAL);
    }
    let mut groups = Vec::new();
    groups
        .try_reserve_exact(size as usize)
        .map_err(|_| Errno::ENOMEM)?;
    while groups.len() < size as usize {
        let chunk = GROUPS_PER_COPY.min(size as usize - groups.len());
        let mut bytes = [0u8; GROUPS_PER_COPY * 4];
        let at = list.wrapping_add(groups.len() as u64 * 4);
        let readable = match process.read_user_memory(at, &mut bytes[..chunk * 4]) {
            Ok(()) => chunk,
            Err(_) => readable_groups(process, at, &mut bytes[..chunk * 4]),
        };
        let read = bytes[..readable * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_ne_bytes(*word));
        if read.clone().any(|group| group == UNCHANGED) {
            return Err(Errno::EINVAL);
        }
        if readable < chunk {
            return Err(Errno::EFAULT);
        }
        groups.extend(read);
    }
    let mut state = process.credentials_and_limits().lock();
    if !state.credentials.is_privileged() {
        return Err(Errno::EPERM);
    }
    state.credentials.set_groups(groups)?;
    Ok(0)
}

/// Reads the 32-bit words at `address` into `bytes` one at a time, after a copy of them all
/// failed, and returns how many were read before the first that is not the program's memory.
#[cold]
fn readable_groups<P: Platform>(process: &Process<P>, address: u64, bytes: &mut [u8]) -> usize {
    let mut read = 0;
    for word in bytes.as_chunks_mut::<4>().0 {
        let at = address.wrapping_add(read as u64 * 4);
        if process.read_user_memory(at, word).is_err() {
            break;
        }
        read += 1;
    }
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    const OTHER: u32 = 1000;

    fn ids(real: u32, effective: u32, saved: u32, filesystem: u32) -> Ids {
        Ids {
            real,
            effective,
            saved,
            filesystem,
        }
    }

    #[test]
    fn setuid_sets_every_id_when_privileged_and_only_the_effective_one_otherwise() {
        assert_eq!(Ids::ROOT.set(OTHER, true), Ok(Ids::all(OTHER)));
        assert_eq!(Ids::ROOT.set(UNCHANGED, true), Err(Errno::EINVAL));
        let swapped = ids(OTHER, 0, 0, 0);
        assert_eq!(swapped.set(OTHER, false), Ok(ids(OTHER, OTHER, 0, OTHER)));
        let dropped = ids(OTHER, OTHER, 0, OTHER);
        assert_eq!(dropped.set(0, false), Ok(ids(OTHER, 0, 0, 0)));
        assert_eq!(Ids::all(OTHER).set(0, false), Err(Errno::EPERM));
        assert_eq!(Ids::all(OTHER).set(UNCHANGED, false), Err(Errno::EINVAL));
    }

    #[test]
    fn setreuid_moves_the_saved_id_with_the_effective_one_as_linux_does() {
        let user = ids(1, 2, 3, 2);
        assert_eq!(
            user.set_real_and_effective(UNCHANGED, UNCHANGED, false),
            Ok(user)
        );
        assert_eq!(
            user.set_real_and_effective(UNCHANGED, 1, false),
            Ok(ids(1, 1, 3, 1))
        );
        assert_eq!(
            user.set_real_and_effective(UNCHANGED, 3, false),
            Ok(ids(1, 3, 3, 3))
        );
        assert_eq!(
            user.set_real_and_effective(2, UNCHANGED, false),
            Ok(ids(2, 2, 2, 2))
        );
        assert_eq!(
            user.set_real_and_effective(1, UNCHANGED, false),
            Ok(ids(1, 2, 2, 2))
        );
        assert_eq!(
            user.set_real_and_effective(2, 1, false),
            Ok(ids(2, 1, 1, 1))
        );
        assert_eq!(
            user.set_real_and_effective(3, UNCHANGED, false),
            Err(Errno::EPERM)
        );
        assert_eq!(
            user.set_real_and_effective(UNCHANGED, 4, false),
            Err(Errno::EPERM)
        );
        assert_eq!(user.set_real_and_effective(7, 8, true), Ok(ids(7, 8, 8, 8)));
    }

    #[test]
    fn setresuid_moves_ids_among_the_held_ones_unless_privileged() {
        let user = ids(1, 2, 3, 9);
        for (real, effective, saved) in [(UNCHANGED, UNCHANGED, UNCHANGED), (1, UNCHANGED, 3)] {
            assert_eq!(
                user.set_real_effective_and_saved(real, effective, saved, false),
                Ok(user)
            );
        }
        assert_eq!(
            user.set_real_effective_and_saved(UNCHANGED, 2, UNCHANGED, false),
            Ok(ids(1, 2, 3, 2))
        );
        assert_eq!(
            user.set_real_effective_and_saved(1, 2, 3, false),
            Ok(ids(1, 2, 3, 2))
        );
        let aligned = ids(1, 2, 3, 2);
        assert_eq!(
            aligned.set_real_effective_and_saved(1, 2, 3, false),
            Ok(aligned)
        );
        assert_eq!(
            user.set_real_effective_and_saved(3, 1, 2, false),
            Ok(ids(3, 1, 2, 1))
        );
        assert_eq!(
            user.set_real_effective_and_saved(UNCHANGED, 9, UNCHANGED, false),
            Err(Errno::EPERM)
        );
        assert_eq!(
            user.set_real_effective_and_saved(4, 5, 6, true),
            Ok(ids(4, 5, 6, 5))
        );
    }

    #[test]
    fn setfsuid_changes_only_to_a_held_id_unless_privileged() {
        let user = ids(1, 2, 3, 2);
        assert_eq!(user.set_filesystem(3, false), ids(1, 2, 3, 3));
        assert_eq!(user.set_filesystem(4, false), user);
        assert_eq!(user.set_filesystem(4, true), ids(1, 2, 3, 4));
        assert_eq!(user.set_filesystem(UNCHANGED, true), user);
        let moved = ids(1, 2, 3, 7);
        assert_eq!(moved.set_filesystem(7, false), moved);
    }

    #[test]
    fn credentials_start_as_root_and_are_copied_on_the_first_change() {
        let mut credentials = Credentials::new();
        assert!(credentials.is_privileged());
        assert_eq!(credentials.ids(Kind::User), Ids::ROOT);
        assert_eq!(credentials.ids(Kind::Group), Ids::ROOT);
        assert!(credentials.groups().is_empty());
        credentials.set_ids(Kind::User, Ids::ROOT).unwrap();
        credentials.set_groups(Vec::new()).unwrap();
        assert_eq!(credentials.after_exec(), Ok((Ids::ROOT, Ids::ROOT)));
        assert!(credentials.0.is_none());
        assert!(!is_secure_execution(Ids::ROOT, Ids::ROOT));

        credentials.set_groups([30, 10, 20, 10].into()).unwrap();
        assert_eq!(credentials.groups(), [10, 10, 20, 30]);
        credentials.set_ids(Kind::Group, ids(5, 6, 7, 6)).unwrap();
        assert!(credentials.is_privileged());
        assert!(is_secure_execution(
            credentials.ids(Kind::User),
            credentials.ids(Kind::Group)
        ));
        credentials
            .set_ids(Kind::User, ids(OTHER, 0, 0, 0))
            .unwrap();
        assert!(credentials.is_privileged());
        credentials
            .set_ids(Kind::User, ids(0, OTHER, 0, OTHER))
            .unwrap();
        assert!(!credentials.is_privileged());
        assert_eq!(
            credentials.after_exec(),
            Ok((ids(0, OTHER, OTHER, OTHER), ids(5, 6, 6, 6)))
        );
        assert_eq!(credentials.ids(Kind::User), ids(0, OTHER, OTHER, OTHER));
        assert_eq!(credentials.groups(), [10, 10, 20, 30]);
    }
}
