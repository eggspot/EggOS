//! What a trap in translated code means, where in the guest a host offset of translated code
//! is, the code generation mode for the CPU, and how an ordered access of translated code that
//! faulted on a misaligned address completes.

use core::sync::atomic::{Ordering, fence};
use eggos_translate::codegen::{CodegenMode, trap};
use eggos_translate::image::BackMapEntry;

const HWCAP_ATOMICS: u64 = 1 << 8;
const HWCAP_LRCPC: u64 = 1 << 15;
const HWCAP_FLAGM: u64 = 1 << 35;
const BRK_MASK: u32 = 0xFFE0_001F;
const BRK: u32 = 0xD420_0000;
const ORDERED_ACCESS_MASK: u32 = 0x3FFF_FC00;
const LDAR: u32 = 0x08DF_FC00;
const STLR: u32 = 0x089F_FC00;
const LDAPR: u32 = 0x38BF_C000;
const STACK_POINTER_OR_ZERO: u8 = 31;

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

/// An acquire load (`ldar`, `ldapr`) or release store (`stlr`) that translated code makes for
/// a guest access. Arm faults on these when the address is not a multiple of the access size,
/// where x86-64 allows any address, so the layers complete them in the fault handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderedAccess {
    /// A load, else a store.
    pub load: bool,
    /// The access size: 1, 2, 4 or 8 bytes.
    pub bytes: u8,
    /// The register loaded or stored (`Rt`; 31 is the zero register).
    pub data: u8,
    /// The register holding the address (`Rn`; 31 is the stack pointer).
    pub base: u8,
}

impl OrderedAccess {
    /// The ordered access that `instruction` is, if it is one.
    pub fn of(instruction: u32) -> Option<OrderedAccess> {
        let load = match instruction & ORDERED_ACCESS_MASK {
            LDAR | LDAPR => true,
            STLR => false,
            _ => return None,
        };
        Some(OrderedAccess {
            load,
            bytes: 1u8 << (instruction >> 30),
            data: (instruction & 0x1F) as u8,
            base: (instruction >> 5 & 0x1F) as u8,
        })
    }

    /// Makes the access with a full barrier on each side, which keeps the order the ordered
    /// instruction gives, at any alignment. `registers` are `x0`-`x30` then `sp`, as a signal's
    /// context holds them; a load writes its zero-extended value back into them.
    ///
    /// # Safety
    /// The address in the base register must be valid for the access.
    pub unsafe fn complete(self, registers: &mut [u64; 32]) {
        let address = registers[usize::from(self.base)] as usize;
        fence(Ordering::SeqCst);
        if self.load {
            let value = match self.bytes {
                // SAFETY: the caller's contract.
                1 => u64::from(unsafe { (address as *const u8).read() }),
                // SAFETY: the caller's contract; the read takes any alignment.
                2 => u64::from(unsafe { (address as *const u16).read_unaligned() }),
                // SAFETY: as above.
                4 => u64::from(unsafe { (address as *const u32).read_unaligned() }),
                // SAFETY: as above.
                _ => unsafe { (address as *const u64).read_unaligned() },
            };
            if self.data != STACK_POINTER_OR_ZERO {
                registers[usize::from(self.data)] = value;
            }
        } else {
            let value = match self.data {
                STACK_POINTER_OR_ZERO => 0,
                data => registers[usize::from(data)],
            };
            match self.bytes {
                // SAFETY: the caller's contract.
                1 => unsafe { (address as *mut u8).write(value as u8) },
                // SAFETY: the caller's contract; the write takes any alignment.
                2 => unsafe { (address as *mut u16).write_unaligned(value as u16) },
                // SAFETY: as above.
                4 => unsafe { (address as *mut u32).write_unaligned(value as u32) },
                // SAFETY: as above.
                _ => unsafe { (address as *mut u64).write_unaligned(value) },
            }
        }
        fence(Ordering::SeqCst);
    }
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

    #[test]
    fn ordered_accesses_decode_from_their_encodings() {
        assert_eq!(
            OrderedAccess::of(0xC8DF_FE00),
            Some(OrderedAccess {
                load: true,
                bytes: 8,
                data: 0,
                base: 16
            })
        );
        assert_eq!(
            OrderedAccess::of(0x889F_FE01),
            Some(OrderedAccess {
                load: false,
                bytes: 4,
                data: 1,
                base: 16
            })
        );
        assert_eq!(
            OrderedAccess::of(0xF8BF_C220),
            Some(OrderedAccess {
                load: true,
                bytes: 8,
                data: 0,
                base: 17
            })
        );
        assert_eq!(
            OrderedAccess::of(0x489F_FE3F),
            Some(OrderedAccess {
                load: false,
                bytes: 2,
                data: 31,
                base: 17
            })
        );
        assert_eq!(OrderedAccess::of(0xC85F_FE00), None);
        assert_eq!(OrderedAccess::of(0xF940_0200), None);
        assert_eq!(OrderedAccess::of(0xF8E1_0200), None);
    }

    #[test]
    fn a_misaligned_ordered_store_and_load_complete() {
        let mut memory = [0u8; 16];
        let mut registers = [0u64; 32];
        registers[16] = memory.as_mut_ptr().wrapping_add(4) as u64;
        registers[0] = 0x1122_3344_5566_7788;
        let store = OrderedAccess::of(0xC89F_FE00).unwrap();
        // SAFETY: the address is inside `memory`, with 12 bytes after it.
        unsafe { store.complete(&mut registers) };
        assert_eq!(
            memory[4..12],
            [0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
        );
        registers[1] = u64::MAX;
        let load = OrderedAccess::of(0x88DF_FE01).unwrap();
        // SAFETY: as above.
        unsafe { load.complete(&mut registers) };
        assert_eq!(registers[1], 0x5566_7788);
        let zero = OrderedAccess::of(0x889F_FE1F).unwrap();
        // SAFETY: as above.
        unsafe { zero.complete(&mut registers) };
        assert_eq!(memory[4..12], [0, 0, 0, 0, 0x44, 0x33, 0x22, 0x11]);
    }
}
