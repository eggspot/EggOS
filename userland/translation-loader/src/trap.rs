//! What a trap in translated code means, where in the guest a host offset of translated code
//! is, and the code generation mode for the CPU.

use eggos_translate::codegen::{CodegenMode, trap};
use eggos_translate::image::BackMapEntry;

const HWCAP_ATOMICS: u64 = 1 << 8;
const HWCAP_LRCPC: u64 = 1 << 15;
const HWCAP_FLAGM: u64 = 1 << 35;
const BRK_MASK: u32 = 0xFFE0_001F;
const BRK: u32 = 0xD420_0000;

/// The code generation mode for a CPU whose `AT_HWCAP` is `capabilities`.
pub fn codegen_mode(capabilities: u64) -> CodegenMode {
    CodegenMode {
        hardware_tso: false,
        lse: capabilities & HWCAP_ATOMICS != 0,
        lrcpc: capabilities & HWCAP_LRCPC != 0,
        flagm: capabilities & HWCAP_FLAGM != 0,
    }
}

/// Why translated code trapped, from its `brk` immediate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapCause {
    /// `int3`.
    Breakpoint,
    /// `ud2`, or bytes that are no x86-64 instruction.
    InvalidOpcode,
    /// An instruction EggOS does not translate.
    Refused,
    /// `div` or `idiv` by zero or with a quotient too large.
    DivideError,
    /// Windows' fast fail (`int 0x29`).
    FastFail,
    /// Any other `int n`.
    GeneralProtection,
    /// A jump to code discovery never found.
    Undiscovered,
}

impl TrapCause {
    /// The cause of the `brk` that `instruction` is, if it is one of translated code's.
    pub fn of(instruction: u32) -> Option<TrapCause> {
        if instruction & BRK_MASK != BRK {
            return None;
        }
        Some(match (instruction >> 5) as u16 {
            trap::BREAKPOINT => TrapCause::Breakpoint,
            trap::INVALID_OPCODE => TrapCause::InvalidOpcode,
            trap::REFUSED => TrapCause::Refused,
            trap::DIVIDE_ERROR => TrapCause::DivideError,
            trap::FAST_FAIL => TrapCause::FastFail,
            trap::GENERAL_PROTECTION => TrapCause::GeneralProtection,
            trap::UNDISCOVERED => TrapCause::Undiscovered,
            _ => return None,
        })
    }

    /// Whether the guest address is in `x16` (the stub loaded it) rather than found through
    /// the back-map.
    pub fn address_in_x16(self) -> bool {
        matches!(self, TrapCause::Refused | TrapCause::Undiscovered)
    }
}

/// The guest relative address of the instruction whose translation holds code offset
/// `host`: the last back-map entry at or before it.
pub fn guest_rva(back_map: &[BackMapEntry], host: u32) -> Option<u32> {
    let after = back_map.partition_point(|entry| entry.host <= host);
    after.checked_sub(1).map(|index| back_map[index].rva)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mode_follows_the_cpu_s_capabilities() {
        assert_eq!(codegen_mode(0), CodegenMode::default());
        let all = codegen_mode(HWCAP_ATOMICS | HWCAP_LRCPC | HWCAP_FLAGM);
        assert!(all.lse && all.lrcpc && all.flagm && !all.hardware_tso);
    }

    #[test]
    fn translated_code_s_traps_are_read_from_their_brk() {
        let brk = |immediate: u16| BRK | (u32::from(immediate) << 5);
        assert_eq!(TrapCause::of(brk(trap::REFUSED)), Some(TrapCause::Refused));
        assert_eq!(
            TrapCause::of(brk(trap::UNDISCOVERED)),
            Some(TrapCause::Undiscovered)
        );
        assert_eq!(
            TrapCause::of(brk(trap::DIVIDE_ERROR)),
            Some(TrapCause::DivideError)
        );
        assert_eq!(TrapCause::of(brk(0xF000)), None);
        assert_eq!(TrapCause::of(0xD503_201F), None);
        assert!(TrapCause::Refused.address_in_x16());
        assert!(!TrapCause::DivideError.address_in_x16());
    }

    #[test]
    fn the_back_map_gives_the_guest_instruction_of_a_host_offset() {
        let map = [
            BackMapEntry {
                host: 64,
                rva: 0x1000,
            },
            BackMapEntry {
                host: 72,
                rva: 0x1003,
            },
            BackMapEntry {
                host: 96,
                rva: 0x1007,
            },
        ];
        assert_eq!(guest_rva(&map, 10), None);
        assert_eq!(guest_rva(&map, 64), Some(0x1000));
        assert_eq!(guest_rva(&map, 80), Some(0x1003));
        assert_eq!(guest_rva(&map, 400), Some(0x1007));
    }
}
