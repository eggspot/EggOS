//! How the C runtime's counted `printf` functions (D146) end the caller's buffer and what they
//! return, from the options word `__stdio_common_vs(w)printf` receives: C99's `snprintf`
//! (`_CRT_INTERNAL_PRINTF_STANDARD_SNPRINTF_BEHAVIOR`), Microsoft's `_snprintf`, `_vsnwprintf`
//! and `sprintf` (`_CRT_INTERNAL_PRINTF_LEGACY_VSPRINTF_NULL_TERMINATION`), and ISO's
//! `swprintf` and `vswprintf`, which pass neither.

/// `_CRT_INTERNAL_PRINTF_LEGACY_VSPRINTF_NULL_TERMINATION`.
pub const LEGACY_VSPRINTF_NULL_TERMINATION: u64 = 0x1;
/// `_CRT_INTERNAL_PRINTF_STANDARD_SNPRINTF_BEHAVIOR`.
pub const STANDARD_SNPRINTF_BEHAVIOR: u64 = 0x2;

/// Which documented rules a counted `printf` call follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convention {
    /// C99's `snprintf`: always ends the buffer, at `count - 1` at most, and returns the length
    /// the whole output needs.
    Standard,
    /// `_snprintf`: ends the buffer only when the output is shorter than `count`; an output of
    /// exactly `count` returns `count` unended, a longer one -1.
    Legacy,
    /// ISO `swprintf`: always ends the buffer, at `count - 1` at most, and returns -1 when the
    /// output takes `count` units or more.
    Iso,
}

impl Convention {
    /// The rules the options word names; the standard bit wins over the legacy one.
    pub fn of_options(options: u64) -> Self {
        if options & STANDARD_SNPRINTF_BEHAVIOR != 0 {
            Self::Standard
        } else if options & LEGACY_VSPRINTF_NULL_TERMINATION != 0 {
            Self::Legacy
        } else {
            Self::Iso
        }
    }
}

/// The `errno` a counted `printf` call sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// `EINVAL`: a null buffer with a count.
    InvalidParameter,
    /// `ERANGE`: `_snprintf` with a buffer and a count of zero.
    Range,
}

/// What a counted `printf` call does once its output is formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ending {
    /// Where the terminating NUL goes, if anywhere.
    pub terminator_at: Option<usize>,
    pub result: i32,
    pub fault: Option<Fault>,
}

/// The ending of a call whose whole output is `produced` units, into a buffer of `count` units
/// (`has_buffer` false for a null buffer, which only measures when `count` is 0).
pub fn ending(convention: Convention, produced: usize, count: usize, has_buffer: bool) -> Ending {
    let length = produced as i32;
    let end = |terminator_at, result, fault| Ending {
        terminator_at,
        result,
        fault,
    };
    if !has_buffer {
        return match count {
            0 => end(None, length, None),
            _ => end(None, -1, Some(Fault::InvalidParameter)),
        };
    }
    if count == 0 {
        return match convention {
            Convention::Standard => end(None, length, None),
            Convention::Legacy => end(None, -1, Some(Fault::Range)),
            Convention::Iso => end(None, -1, None),
        };
    }
    let fits = produced < count;
    match convention {
        Convention::Standard => end(Some(produced.min(count - 1)), length, None),
        Convention::Legacy if fits => end(Some(produced), length, None),
        Convention::Legacy if produced == count => end(None, length, None),
        Convention::Legacy => end(None, -1, None),
        Convention::Iso => end(
            Some(produced.min(count - 1)),
            if fits { length } else { -1 },
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(terminator_at: Option<usize>, result: i32, fault: Option<Fault>) -> Ending {
        Ending {
            terminator_at,
            result,
            fault,
        }
    }

    #[test]
    fn the_options_word_names_the_convention() {
        assert_eq!(Convention::of_options(0), Convention::Iso);
        assert_eq!(Convention::of_options(0x1), Convention::Legacy);
        assert_eq!(Convention::of_options(0x2), Convention::Standard);
        assert_eq!(Convention::of_options(0x3), Convention::Standard);
        assert_eq!(Convention::of_options(0x4), Convention::Iso);
        assert_eq!(Convention::of_options(0x5), Convention::Legacy);
    }

    #[test]
    fn standard_snprintf_always_ends_the_buffer_and_returns_the_whole_length() {
        let standard = |produced, count| ending(Convention::Standard, produced, count, true);
        assert_eq!(standard(5, 0), at(None, 5, None));
        assert_eq!(standard(3, 8), at(Some(3), 3, None));
        assert_eq!(standard(8, 8), at(Some(7), 8, None));
        assert_eq!(standard(9, 8), at(Some(7), 9, None));
        assert_eq!(standard(100, 8), at(Some(7), 100, None));
    }

    #[test]
    fn legacy_snprintf_ends_the_buffer_only_when_the_output_is_shorter() {
        let legacy = |produced, count| ending(Convention::Legacy, produced, count, true);
        assert_eq!(legacy(5, 0), at(None, -1, Some(Fault::Range)));
        assert_eq!(legacy(3, 8), at(Some(3), 3, None));
        assert_eq!(legacy(8, 8), at(None, 8, None));
        assert_eq!(legacy(9, 8), at(None, -1, None));
        assert_eq!(legacy(100, 8), at(None, -1, None));
    }

    #[test]
    fn iso_swprintf_always_ends_the_buffer_and_fails_when_the_output_does_not_fit() {
        let iso = |produced, count| ending(Convention::Iso, produced, count, true);
        assert_eq!(iso(5, 0), at(None, -1, None));
        assert_eq!(iso(3, 8), at(Some(3), 3, None));
        assert_eq!(iso(8, 8), at(Some(7), -1, None));
        assert_eq!(iso(9, 8), at(Some(7), -1, None));
        assert_eq!(iso(100, 64), at(Some(63), -1, None));
    }

    #[test]
    fn a_null_buffer_measures_with_a_count_of_zero_and_is_refused_with_one() {
        for convention in [Convention::Standard, Convention::Legacy, Convention::Iso] {
            assert_eq!(ending(convention, 12, 0, false), at(None, 12, None));
            assert_eq!(
                ending(convention, 12, 4, false),
                at(None, -1, Some(Fault::InvalidParameter))
            );
        }
    }
}
