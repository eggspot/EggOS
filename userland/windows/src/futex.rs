//! How many sleepers a futex wake wakes. Linux reads the count as a C `int`, so waking every
//! sleeper takes `i32::MAX`: `u32::MAX` would read as -1, which wakes one.

/// The sleepers a wake wakes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeCount {
    One,
    All,
}

impl WakeCount {
    /// The count `futex(FUTEX_WAKE)` takes.
    pub const fn count(self) -> u32 {
        match self {
            WakeCount::One => 1,
            WakeCount::All => i32::MAX as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waking_all_is_the_largest_positive_int_and_never_reads_as_one_or_less() {
        assert_eq!(WakeCount::All.count() as i32, i32::MAX);
        assert_eq!(WakeCount::One.count() as i32, 1);
        assert!(WakeCount::All.count() as i32 > WakeCount::One.count() as i32);
    }
}
