//! Windows handle flags on Linux descriptors (D189): a file or pipe handle is inherited by the
//! programs `CreateProcess` starts when its descriptor does not close on `execve`, so
//! `HANDLE_FLAG_INHERIT` is the absence of `FD_CLOEXEC`; `HANDLE_FLAG_PROTECT_FROM_CLOSE`
//! has no Linux counterpart and is kept by the layer in [`ProtectedDescriptors`].

/// `HANDLE_FLAG_INHERIT`: children started with inherited handles get this handle.
pub const INHERIT: u32 = 1;
/// `HANDLE_FLAG_PROTECT_FROM_CLOSE`: `CloseHandle` refuses to close this handle.
pub const PROTECT_FROM_CLOSE: u32 = 2;
const KNOWN: u32 = INHERIT | PROTECT_FROM_CLOSE;
/// Linux's descriptor flag that closes a descriptor on `execve`.
pub const CLOSE_ON_EXEC: usize = 1;

/// The flags `SetHandleInformation(handle, mask, flags)` leaves on a handle that had
/// `current`: those in `mask` taken from `flags`, the others kept; unknown bits are dropped.
pub fn updated(current: u32, mask: u32, flags: u32) -> u32 {
    ((current & !mask) | (flags & mask)) & KNOWN
}

/// The handle flags of a descriptor with the Linux descriptor flags `descriptor_flags`,
/// protected from closing or not.
pub fn of_descriptor(descriptor_flags: usize, protected: bool) -> u32 {
    let inherit = match descriptor_flags & CLOSE_ON_EXEC {
        0 => INHERIT,
        _ => 0,
    };
    let protect = match protected {
        true => PROTECT_FROM_CLOSE,
        false => 0,
    };
    inherit | protect
}

/// The Linux descriptor flags for a descriptor that had `descriptor_flags` and now has the
/// handle flags `flags`.
pub fn descriptor_flags(descriptor_flags: usize, flags: u32) -> usize {
    match flags & INHERIT {
        0 => descriptor_flags | CLOSE_ON_EXEC,
        _ => descriptor_flags & !CLOSE_ON_EXEC,
    }
}

const PROTECTED_WORDS: usize = 64;

/// The descriptors protected from closing, one bit each for the first 4096 descriptors.
#[derive(Debug)]
pub struct ProtectedDescriptors {
    words: [u64; PROTECTED_WORDS],
}

impl ProtectedDescriptors {
    /// None protected.
    pub const fn new() -> Self {
        Self {
            words: [0; PROTECTED_WORDS],
        }
    }

    /// The highest descriptor this set can protect, plus one.
    pub const CAPACITY: usize = PROTECTED_WORDS * 64;

    /// Whether `fd` is protected from closing.
    pub fn contains(&self, fd: usize) -> bool {
        fd < Self::CAPACITY && self.words[fd / 64] & (1 << (fd % 64)) != 0
    }

    /// Protects `fd` from closing, or stops protecting it; `false` when `fd` is beyond the
    /// set's capacity and protection was asked for.
    pub fn set(&mut self, fd: usize, protected: bool) -> bool {
        if fd >= Self::CAPACITY {
            return !protected;
        }
        let bit = 1 << (fd % 64);
        match protected {
            true => self.words[fd / 64] |= bit,
            false => self.words[fd / 64] &= !bit,
        }
        true
    }
}

impl Default for ProtectedDescriptors {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_handle_information_changes_only_the_masked_flags() {
        assert_eq!(updated(INHERIT, INHERIT, 0), 0);
        assert_eq!(updated(0, INHERIT, INHERIT), INHERIT);
        assert_eq!(
            updated(INHERIT, PROTECT_FROM_CLOSE, PROTECT_FROM_CLOSE),
            INHERIT | PROTECT_FROM_CLOSE
        );
        assert_eq!(updated(INHERIT, 0, 0), INHERIT);
        assert_eq!(updated(0, u32::MAX, u32::MAX), KNOWN);
    }

    #[test]
    fn inheritance_is_the_absence_of_close_on_exec() {
        assert_eq!(of_descriptor(0, false), INHERIT);
        assert_eq!(of_descriptor(CLOSE_ON_EXEC, false), 0);
        assert_eq!(of_descriptor(CLOSE_ON_EXEC, true), PROTECT_FROM_CLOSE);
        assert_eq!(descriptor_flags(CLOSE_ON_EXEC, INHERIT), 0);
        assert_eq!(descriptor_flags(0, 0), CLOSE_ON_EXEC);
        assert_eq!(descriptor_flags(4, PROTECT_FROM_CLOSE), 4 | CLOSE_ON_EXEC);
    }

    #[test]
    fn protected_descriptors_are_remembered_until_released() {
        let mut protected = ProtectedDescriptors::new();
        assert!(!protected.contains(5));
        assert!(protected.set(5, true));
        assert!(protected.set(4095, true));
        assert!(protected.contains(5) && protected.contains(4095));
        assert!(!protected.contains(4) && !protected.contains(69));
        assert!(protected.set(5, false));
        assert!(!protected.contains(5));
        assert!(!protected.set(4096, true));
        assert!(protected.set(4096, false));
        assert!(!protected.contains(4096));
    }
}
