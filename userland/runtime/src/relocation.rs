//! A static PIE relocates itself: Linux maps it at a base of its choosing and applies no
//! relocations, so before the program touches a pointer stored in its data (a vtable, a table
//! of strings), the runtime adds the load bias to each one its `R_*_RELATIVE` relocations name.
//! This runs before relocation, so it must not use such pointers itself.

/// Dynamic section tags the relocation reads or refuses.
pub mod tag {
    pub const NULL: usize = 0;
    pub const RELA: usize = 7;
    pub const RELASZ: usize = 8;
    pub const RELAENT: usize = 9;
    pub const REL: usize = 17;
    pub const RELR: usize = 36;
}

/// The architecture's `R_*_RELATIVE` relocation type.
#[cfg(target_arch = "x86_64")]
pub const RELATIVE: u32 = 8;
#[cfg(target_arch = "aarch64")]
pub const RELATIVE: u32 = 1027;
#[cfg(target_arch = "riscv64")]
pub const RELATIVE: u32 = 3;

/// An ELF-64 relocation with an addend.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Rela {
    pub offset: usize,
    pub info: u64,
    pub addend: isize,
}

/// Why a program cannot relocate itself: a static PIE linked by our build has only
/// `R_*_RELATIVE` relocations in a `DT_RELA` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocationError {
    /// `DT_REL` or `DT_RELR`, which our build does not produce.
    UnsupportedTable,
    /// A `DT_RELAENT` other than [`Rela`]'s size.
    EntrySize,
    /// A relocation other than `R_*_RELATIVE`, or one that names a symbol.
    UnsupportedType,
}

/// Applies the `R_*_RELATIVE` relocations of the dynamic section at `dynamic` for a program
/// moved by `bias`.
///
/// # Safety
///
/// `dynamic` is the program's dynamic section, ended by `DT_NULL`, its `DT_RELA` table is at
/// `bias` plus its address, and every relocation names a writable word of the program,
/// which nothing else uses yet.
pub unsafe fn relocate(bias: usize, dynamic: *const [usize; 2]) -> Result<(), RelocationError> {
    let mut table = 0;
    let mut size = 0;
    let mut entry_size = size_of::<Rela>();
    let mut entry = dynamic;
    loop {
        // SAFETY: the section ends with `DT_NULL`, which stops the walk.
        let [kind, value] = unsafe { entry.read() };
        match kind {
            tag::NULL => break,
            tag::RELA => table = value,
            tag::RELASZ => size = value,
            tag::RELAENT => entry_size = value,
            tag::REL | tag::RELR => return Err(RelocationError::UnsupportedTable),
            _ => {}
        }
        entry = entry.wrapping_add(1);
    }
    if entry_size != size_of::<Rela>() {
        return Err(RelocationError::EntrySize);
    }
    let relocations = bias.wrapping_add(table) as *const Rela;
    for index in 0..size / size_of::<Rela>() {
        // SAFETY: the caller's `DT_RELA` table, `DT_RELASZ` bytes long, at `bias` plus its
        // address.
        let relocation = unsafe { relocations.wrapping_add(index).read() };
        if relocation.info != u64::from(RELATIVE) {
            return Err(RelocationError::UnsupportedType);
        }
        let target = bias.wrapping_add(relocation.offset) as *mut usize;
        // SAFETY: the caller promised each relocation names a writable word of the program.
        unsafe { target.write_unaligned(bias.wrapping_add_signed(relocation.addend)) };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_the_bias_to_every_relative_relocation() {
        let mut words = [0usize; 4];
        let bias = words.as_mut_ptr() as usize;
        let relocations = [
            Rela {
                offset: 8,
                info: u64::from(RELATIVE),
                addend: 0x40,
            },
            Rela {
                offset: 24,
                info: u64::from(RELATIVE),
                addend: -8,
            },
        ];
        let table = (relocations.as_ptr() as usize).wrapping_sub(bias);
        let dynamic = [
            [tag::RELA, table],
            [tag::RELASZ, size_of_val(&relocations)],
            [tag::RELAENT, size_of::<Rela>()],
            [tag::NULL, 0],
        ];
        // SAFETY: both relocations name words of `words`, which the test owns.
        let result = unsafe { relocate(bias, dynamic.as_ptr()) };
        assert_eq!(result, Ok(()));
        assert_eq!(words, [0, bias + 0x40, 0, bias - 8]);
    }

    #[test]
    fn refuses_symbols_other_types_and_other_tables() {
        let mut word = 0usize;
        let bias = &raw mut word as usize;
        let with_symbol = [Rela {
            offset: 0,
            info: 1 << 32 | u64::from(RELATIVE),
            addend: 0,
        }];
        let table = (with_symbol.as_ptr() as usize).wrapping_sub(bias);
        let dynamic = [[tag::RELA, table], [tag::RELASZ, 24], [tag::NULL, 0]];
        // SAFETY: the relocation is refused before anything is written.
        let result = unsafe { relocate(bias, dynamic.as_ptr()) };
        assert_eq!(result, Err(RelocationError::UnsupportedType));
        for (kind, error) in [
            (tag::RELR, RelocationError::UnsupportedTable),
            (tag::REL, RelocationError::UnsupportedTable),
            (tag::RELAENT, RelocationError::EntrySize),
        ] {
            let dynamic = [[kind, 16], [tag::NULL, 0]];
            // SAFETY: there is nothing to relocate.
            assert_eq!(unsafe { relocate(0, dynamic.as_ptr()) }, Err(error));
        }
        assert_eq!(word, 0);
    }
}
