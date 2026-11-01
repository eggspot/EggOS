//! Whole-image translation (slice 39d1 of D191): code discovery over a loaded program image
//! ([`discover`]), flag liveness across its blocks, and the image builder
//! ([`translate_image`]) that lowers every block, lays the code out, links the exits into
//! branches, and builds the entries table, the back-map, the refused-site list and the runtime
//! routines (the indirect dispatch and the entry trampoline of [`runtime`]). The [`bridge`]
//! module generates the calling-convention bridges between translated and native code.
//!
//! Everything here is pure: the caller (the Windows or macOS layer) hands in the image's
//! sections and its known entry points; nothing reads files or maps memory.

pub mod bridge;
mod discover;
mod entries;
mod link;
mod liveness;
pub mod runtime;
mod seeds;
#[cfg(test)]
mod tests;

pub use discover::{Discovery, JumpTable, MAX_JUMP_TABLE_ENTRIES, discover};
pub use entries::{EMPTY_KEY, EntriesTable, HASH_MULTIPLIER, MAX_BITS, MIN_BITS};
pub use link::{
    BackMapEntry, ImageError, RefusedSite, SiteReason, TranslatedImage, TranslationStats,
    translate_image,
};
pub use liveness::live_outs;
pub use seeds::{function_starts, pdata_function_starts};

/// One mapped section of the guest image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    /// Its address relative to the image base.
    pub rva: u32,
    /// Its contents as mapped (zero-filled to the mapped size by the caller where it matters).
    pub bytes: &'a [u8],
    /// Whether the program may execute it; only executable sections are translated, every
    /// section is read for jump tables.
    pub executable: bool,
}

/// Where a known entry point came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SeedOrigin {
    /// The program's entry point.
    EntryPoint,
    /// An exported function.
    Export,
    /// A TLS callback.
    TlsCallback,
    /// A function start from the exception table (PE `.pdata` `RUNTIME_FUNCTION`).
    ExceptionTable,
    /// A function start from Mach-O `LC_FUNCTION_STARTS`.
    FunctionStart,
    /// A symbol table entry for a function.
    Symbol,
    /// The target of a relocation that points into an executable section; it may be a
    /// pointer to data kept in code, so it is only used when the bytes there look like code.
    Relocation,
}

impl SeedOrigin {
    /// Whether the address is certainly code; otherwise it must pass the plausibility check.
    pub const fn is_certain(self) -> bool {
        !matches!(self, SeedOrigin::Relocation)
    }
}

/// A known entry point of the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Seed {
    /// Its address relative to the image base.
    pub rva: u32,
    /// Where it came from.
    pub origin: SeedOrigin,
}

/// A guest image as its loader would map it at its preferred base.
#[derive(Clone, Copy, Debug)]
pub struct ImageInput<'a> {
    /// The preferred base address; translated code holds guest addresses relative to it, and
    /// the loader moves them by the difference to the real base.
    pub image_base: u64,
    /// The mapped sections.
    pub sections: &'a [Section<'a>],
    /// The known entry points.
    pub seeds: &'a [Seed],
}

/// The relative address the entries table maps to the entry trampoline's return path: the
/// image's first byte (its headers), which is never code. A guest `ret` to the image base
/// returns to the native caller of [`runtime`]'s entry trampoline.
pub const RETURN_TO_NATIVE_RVA: u32 = 0;

impl<'a> ImageInput<'a> {
    fn section_at(&self, address: u64) -> Option<(Section<'a>, usize)> {
        let rva = address.checked_sub(self.image_base)?;
        self.sections.iter().find_map(|&section| {
            let offset = rva.checked_sub(u64::from(section.rva))?;
            let offset = usize::try_from(offset).ok()?;
            (offset < section.bytes.len()).then_some((section, offset))
        })
    }

    /// Whether `address` lies in an executable section (the image base itself never does).
    pub fn is_code(&self, address: u64) -> bool {
        address
            != self
                .image_base
                .wrapping_add(u64::from(RETURN_TO_NATIVE_RVA))
            && self
                .section_at(address)
                .is_some_and(|(section, _)| section.executable)
    }

    /// The executable bytes from `address` to the end of its section; empty outside code.
    pub fn code_from(&self, address: u64) -> &'a [u8] {
        match self.section_at(address) {
            Some((section, offset)) if section.executable && self.is_code(address) => {
                &section.bytes[offset..]
            }
            _ => &[],
        }
    }

    /// `length` bytes at `address` from any one section.
    pub fn read(&self, address: u64, length: usize) -> Option<&'a [u8]> {
        let (section, offset) = self.section_at(address)?;
        section.bytes.get(offset..offset.checked_add(length)?)
    }

    /// The relative address of `address`, when it is inside the image's 4 GiB.
    pub fn rva(&self, address: u64) -> Option<u32> {
        u32::try_from(address.checked_sub(self.image_base)?).ok()
    }

    /// The guest address of `rva`.
    pub fn address(&self, rva: u32) -> u64 {
        self.image_base.wrapping_add(u64::from(rva))
    }
}
