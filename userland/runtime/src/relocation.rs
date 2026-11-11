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
#[cfg(target_arch = "loongarch64")]
pub const RELATIVE: u32 = 3;
#[cfg(target_arch = "powerpc64")]
pub const RELATIVE: u32 = 22;
#[cfg(target_arch = "mips64")]
pub const RELATIVE: u32 = 3;
#[cfg(target_arch = "s390x")]
pub const RELATIVE: u32 = 12;

/// MIPS64's dynamic tags for its `DT_REL` table and its local GOT entries (D315).
#[cfg(target_arch = "mips64")]
mod mips {
    pub const PLTGOT: usize = 3;
    pub const RELSZ: usize = 18;
    pub const RELENT: usize = 19;
    pub const LOCAL_GOTNO: usize = 0x7000_000a;
    /// `R_MIPS_64`, the second type of a 64-bit `R_MIPS_REL32`.
    pub const TYPE_64: u64 = 18;
    /// `GOT[1]`'s mark that it is the GNU module pointer, not a local entry.
    pub const GOT1_MODULE: usize = 1 << 63;
}

/// Whether n64's `r_info` (the symbol in its low 32 bits, then `r_ssym`, `r_type3`, `r_type2`
/// and `r_type` one byte each) is a relative relocation: no symbol, `R_MIPS_REL32`, then
/// `R_MIPS_64` or nothing.
#[cfg(target_arch = "mips64")]
fn is_mips_relative(info: u64) -> bool {
    let (symbol, third, second, first) = (info as u32, (info >> 40) as u8, (info >> 48) as u8, (info >> 56) as u8);
    symbol == 0 && third == 0 && u32::from(first) == RELATIVE && matches!(u64::from(second), 0 | mips::TYPE_64)
}

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
    /// A `DT_RELASZ` that is not a whole number of entries, or one with no `DT_RELA`.
    TableSize,
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
    #[cfg(target_arch = "mips64")]
    {
        // SAFETY: the caller's guarantees are `relocate_mips`'s.
        return unsafe { relocate_mips(bias, dynamic) };
    }
    #[cfg(not(target_arch = "mips64"))]
    // SAFETY: the caller's guarantees are `relocate_rela`'s.
    unsafe {
        relocate_rela(bias, dynamic)
    }
}

/// MIPS64's static PIE (D315): a `DT_REL` table of relative relocations, whose addends are the
/// words they name, then the local GOT entries, which no relocation names, each moved by
/// `bias`, from the first after the reserved ones.
///
/// # Safety
///
/// As for [`relocate`]; the GOT `DT_PLTGOT` names is the program's, writable.
#[cfg(target_arch = "mips64")]
unsafe fn relocate_mips(bias: usize, dynamic: *const [usize; 2]) -> Result<(), RelocationError> {
    let (mut table, mut size, mut entry_size, mut got, mut local) = (0, 0, 16, 0, 0);
    let mut entry = dynamic;
    loop {
        // SAFETY: the section ends with `DT_NULL`, which stops the walk.
        let [kind, value] = unsafe { entry.read() };
        match kind {
            tag::NULL => break,
            tag::REL => table = value,
            mips::RELSZ => size = value,
            mips::RELENT => entry_size = value,
            mips::PLTGOT => got = value,
            mips::LOCAL_GOTNO => local = value,
            tag::RELA | tag::RELR => return Err(RelocationError::UnsupportedTable),
            _ => {}
        }
        entry = entry.wrapping_add(1);
    }
    if entry_size != 16 {
        return Err(RelocationError::EntrySize);
    }
    if size % 16 != 0 || (size != 0 && table == 0) {
        return Err(RelocationError::TableSize);
    }
    let relocations = bias.wrapping_add(table) as *const [u64; 2];
    for index in 0..size / 16 {
        // SAFETY: the caller's `DT_REL` table, `DT_RELSZ` bytes long, at `bias` plus its address.
        let [offset, info] = unsafe { relocations.wrapping_add(index).read_unaligned() };
        if !is_mips_relative(info) {
            return Err(RelocationError::UnsupportedType);
        }
        let target = bias.wrapping_add(offset as usize) as *mut usize;
        // SAFETY: the caller promised each relocation names a writable word of the program.
        unsafe { target.write_unaligned(target.read_unaligned().wrapping_add(bias)) };
    }
    if got != 0 {
        let got = bias.wrapping_add(got) as *mut usize;
        // SAFETY: `GOT[1]`, a word of the program's GOT.
        let first = match unsafe { got.wrapping_add(1).read() } & mips::GOT1_MODULE {
            0 => 1,
            _ => 2,
        };
        for index in first..local {
            let slot = got.wrapping_add(index);
            // SAFETY: a local GOT entry of the program, writable, which nothing used yet.
            unsafe { slot.write(slot.read().wrapping_add(bias)) };
        }
    }
    Ok(())
}

/// The `DT_RELA` table every other architecture's static PIE has.
///
/// # Safety
///
/// As for [`relocate`].
#[cfg_attr(target_arch = "mips64", allow(dead_code))]
unsafe fn relocate_rela(bias: usize, dynamic: *const [usize; 2]) -> Result<(), RelocationError> {
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
    if size % size_of::<Rela>() != 0 || (size != 0 && table == 0) {
        return Err(RelocationError::TableSize);
    }
    let relocations = bias.wrapping_add(table) as *const Rela;
    for index in 0..size / size_of::<Rela>() {
        // SAFETY: the caller's `DT_RELA` table, `DT_RELASZ` bytes long, at `bias` plus its
        // address; read unaligned, so no alignment is assumed.
        let relocation = unsafe { relocations.wrapping_add(index).read_unaligned() };
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
            (tag::RELASZ, RelocationError::TableSize),
        ] {
            let dynamic = [[kind, 16], [tag::NULL, 0]];
            // SAFETY: there is nothing to relocate.
            assert_eq!(unsafe { relocate(0, dynamic.as_ptr()) }, Err(error));
        }
        assert_eq!(word, 0);
    }

    #[test]
    fn refuses_a_size_that_is_not_whole_entries_or_has_no_table() {
        let mut word = 0usize;
        let bias = &raw mut word as usize;
        let relocations = [Rela {
            offset: 0,
            info: u64::from(RELATIVE),
            addend: 0x40,
        }];
        let table = (relocations.as_ptr() as usize).wrapping_sub(bias);
        let dynamic = [[tag::RELA, table], [tag::RELASZ, 25], [tag::NULL, 0]];
        // SAFETY: the table is refused before anything is written.
        let result = unsafe { relocate(bias, dynamic.as_ptr()) };
        assert_eq!(result, Err(RelocationError::TableSize));
        let without_table = [[tag::RELASZ, size_of::<Rela>()], [tag::NULL, 0]];
        // SAFETY: the size without a table is refused before anything is read.
        let result = unsafe { relocate(bias, without_table.as_ptr()) };
        assert_eq!(result, Err(RelocationError::TableSize));
        assert_eq!(word, 0);
    }
}
