//! Unwinding a Windows program's stack from the unwind data in its own image (D177): the
//! exception directory (`.pdata`) lists each function, and the records it points at describe
//! how its prolog changed the registers, as Microsoft documents for x64 (`UNWIND_INFO`) and
//! ARM64 (`.xdata` records and packed unwind data). One step undoes one function's frame and
//! returns the caller's registers, the frame's establisher and its language handler, as
//! `RtlVirtualUnwind` does. Pure logic over the image's bytes and a reader of the stack, with
//! host tests; the exception dispatch in `win32/exception` uses it.

#[cfg(any(test, target_arch = "aarch64"))]
pub mod arm64;
#[cfg(any(test, target_arch = "x86_64"))]
pub mod x64;

/// The program's image as it is mapped: its first address and its bytes.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    pub base: u64,
    pub bytes: &'a [u8],
}

impl Image<'_> {
    fn bytes_at<const N: usize>(&self, rva: u64) -> Option<[u8; N]> {
        let start = usize::try_from(rva).ok()?;
        self.bytes
            .get(start..start.checked_add(N)?)?
            .try_into()
            .ok()
    }

    pub fn u8_at(&self, rva: u64) -> Option<u8> {
        self.bytes_at::<1>(rva).map(|[byte]| byte)
    }

    #[cfg(any(test, target_arch = "x86_64"))]
    pub fn u16_at(&self, rva: u64) -> Option<u16> {
        self.bytes_at(rva).map(u16::from_le_bytes)
    }

    pub fn u32_at(&self, rva: u64) -> Option<u32> {
        self.bytes_at(rva).map(u32::from_le_bytes)
    }

    /// The image-relative address of `address`, if it lies inside the image.
    pub fn rva_of(&self, address: u64) -> Option<u64> {
        address
            .checked_sub(self.base)
            .filter(|rva| *rva < self.bytes.len() as u64)
    }
}

/// Reads the memory of the thread whose stack is unwound.
pub trait Memory {
    /// The 64-bit word at `address`, if it can be read.
    fn u64_at(&self, address: u64) -> Option<u64>;
}

/// The part of a thread's stack a walk may read and its frames may lie in: from the stack
/// pointer the walk starts at (never below the stack's limit) up to the stack's base, as
/// Windows bounds a walk by the TEB's `StackLimit` and `StackBase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackRange {
    pub low: u64,
    pub high: u64,
}

impl StackRange {
    /// The range of a stack between `limit` and `base` for a walk that starts at `sp`.
    pub fn new(limit: u64, base: u64, sp: u64) -> Self {
        Self {
            low: limit.max(sp),
            high: base,
        }
    }

    /// Whether `address` lies in the range, the base included.
    pub fn holds(&self, address: u64) -> bool {
        (self.low..=self.high).contains(&address)
    }

    /// Whether the 8-byte word at `address` is aligned and lies wholly in the range.
    pub fn holds_word(&self, address: u64) -> bool {
        address.is_multiple_of(8)
            && address >= self.low
            && address.checked_add(8).is_some_and(|end| end <= self.high)
    }

    /// Whether a step from a frame whose stack pointer is `sp` to a caller whose stack pointer
    /// is `caller_sp`, with establisher frame `establisher`, stays on the stack and moves up it.
    pub fn allows_step(&self, sp: u64, caller_sp: u64, establisher: u64) -> bool {
        self.holds(sp) && caller_sp >= sp && self.holds(caller_sp) && self.holds(establisher)
    }
}

/// Which language handlers a step asks for, as `RtlVirtualUnwind`'s `HandlerType`: the
/// exception handler (`UNW_FLAG_EHANDLER`), the termination handler (`UNW_FLAG_UHANDLER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandlerKinds {
    pub exception: bool,
    pub termination: bool,
}

impl HandlerKinds {
    pub const EXCEPTION_FLAG: u32 = 1;
    pub const TERMINATION_FLAG: u32 = 2;

    /// The kinds a `HandlerType` argument names.
    pub fn from_flags(flags: u32) -> Self {
        Self {
            exception: flags & Self::EXCEPTION_FLAG != 0,
            termination: flags & Self::TERMINATION_FLAG != 0,
        }
    }

    #[cfg(any(test, target_arch = "x86_64"))]
    fn matches(self, flags: u32) -> bool {
        (self.exception && flags & Self::EXCEPTION_FLAG != 0)
            || (self.termination && flags & Self::TERMINATION_FLAG != 0)
    }
}

/// What one step found besides the caller's registers: the frame's language handler (its
/// address) and the address of its data, when the step asked for that kind and the program
/// was in the function's body, and the frame's establisher, which identifies the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unwound {
    pub handler: Option<u64>,
    pub handler_data: u64,
    pub establisher: u64,
}

/// Why a frame cannot be unwound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnwindError {
    /// The unwind data runs past the image.
    Truncated,
    /// An unwind code this unwinder does not know.
    UnknownCode,
    /// A saved register's stack slot cannot be read.
    UnreadableStack,
}

#[cfg(test)]
mod tests {
    use super::StackRange;

    const LIMIT: u64 = 0x7000_0000;
    const BASE: u64 = 0x7010_0000;

    #[test]
    fn only_aligned_words_between_the_start_and_the_base_are_read() {
        let range = StackRange::new(LIMIT, BASE, LIMIT + 0x800);
        assert!(range.holds_word(LIMIT + 0x800));
        assert!(range.holds_word(BASE - 8));
        assert!(!range.holds_word(BASE));
        assert!(!range.holds_word(BASE - 4));
        assert!(!range.holds_word(LIMIT + 0x7f8));
        assert!(!range.holds_word(LIMIT));
        assert!(!range.holds_word(0x4141_4141_4141_4140));
        assert!(!range.holds_word(u64::MAX - 7));
        assert!(!range.holds_word(0));
    }

    #[test]
    fn a_start_below_the_limit_is_bounded_by_the_limit_and_one_above_the_base_holds_nothing() {
        let below = StackRange::new(LIMIT, BASE, 0x1000);
        assert_eq!(below.low, LIMIT);
        assert!(!below.holds_word(0x1000));
        let above = StackRange::new(LIMIT, BASE, 0x4141_4141_4141_4140);
        assert!(!above.holds(0x4141_4141_4141_4140));
        assert!(!above.holds_word(BASE - 8));
    }

    #[test]
    fn a_step_must_stay_on_the_stack_and_never_move_down_it() {
        let range = StackRange::new(LIMIT, BASE, LIMIT + 0x100);
        assert!(range.allows_step(LIMIT + 0x100, LIMIT + 0x180, LIMIT + 0x180));
        assert!(range.allows_step(LIMIT + 0x100, LIMIT + 0x100, LIMIT + 0x100));
        assert!(range.allows_step(LIMIT + 0x100, BASE, BASE));
        assert!(!range.allows_step(LIMIT + 0x180, LIMIT + 0x100, LIMIT + 0x180));
        assert!(!range.allows_step(LIMIT + 0x100, BASE + 8, BASE));
        assert!(!range.allows_step(LIMIT + 0x100, LIMIT + 0x180, 0x4141_4141_4141_4140));
        assert!(!range.allows_step(0x10, LIMIT + 0x180, LIMIT + 0x180));
    }
}

#[cfg(test)]
pub mod test_memory {
    use super::Memory;

    /// A stack for tests: words from `base` upward.
    pub struct Stack {
        pub base: u64,
        pub words: Vec<u64>,
    }

    impl Memory for Stack {
        fn u64_at(&self, address: u64) -> Option<u64> {
            let offset = address.checked_sub(self.base)?;
            if !offset.is_multiple_of(8) {
                return None;
            }
            self.words.get((offset / 8) as usize).copied()
        }
    }
}
