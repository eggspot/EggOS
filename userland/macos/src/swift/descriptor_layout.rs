//! Where the trailing parts of a Swift type context descriptor sit (D740), from Swift's ABI:
//! a class's fixed fields end at byte 44 and a struct's or enum's at 28; then, in order, the
//! generic context (its header, one byte per parameter padded to four, twelve bytes per
//! requirement, the pack shapes), a class's resilient superclass, the foreign or singleton
//! metadata initialization, a class's vtable (its header and eight bytes per method), its
//! override table (twelve bytes per entry) and its Objective-C resilient class stub. Pure
//! logic over a reader of the descriptor's 32-bit words, with host tests.

/// Context descriptor kinds.
pub const CLASS: u32 = 16;
pub const STRUCT: u32 = 17;
pub const ENUM: u32 = 18;

const KIND_MASK: u32 = 0x1f;
const GENERIC: u32 = 0x80;
const METADATA_INITIALIZATION_SHIFT: u32 = 16;
const SINGLETON_INITIALIZATION: u32 = 1;
const FOREIGN_INITIALIZATION: u32 = 2;
const IMMEDIATE_MEMBERS_NEGATIVE: u32 = 1 << 28;
const RESILIENT_SUPERCLASS: u32 = 1 << 29;
const OVERRIDE_TABLE: u32 = 1 << 30;
const VTABLE: u32 = 1 << 31;
const RESILIENT_SUPERCLASS_KIND_SHIFT: u32 = 25;
const TYPE_PACKS: u32 = 1;
const CONDITIONAL_INVERTED_PROTOCOLS: u32 = 2;

const CLASS_FIXED_SIZE: usize = 44;
const VALUE_FIXED_SIZE: usize = 28;
const GENERIC_HEADER_SIZE: usize = 16;
const REQUIREMENT_SIZE: usize = 12;
const PACK_HEADER_SIZE: usize = 4;
const PACK_SHAPE_SIZE: usize = 8;
const SINGLETON_SIZE: usize = 12;
const VTABLE_HEADER_SIZE: usize = 8;
const METHOD_SIZE: usize = 8;
const OVERRIDE_HEADER_SIZE: usize = 4;
const OVERRIDE_SIZE: usize = 12;

/// A descriptor's flags word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags(pub u32);

impl Flags {
    pub fn kind(self) -> u32 {
        self.0 & KIND_MASK
    }

    pub fn is_generic(self) -> bool {
        self.0 & GENERIC != 0
    }

    fn metadata_initialization(self) -> u32 {
        (self.0 >> METADATA_INITIALIZATION_SHIFT) & 3
    }

    pub fn has_singleton_initialization(self) -> bool {
        self.metadata_initialization() == SINGLETON_INITIALIZATION
    }

    pub fn has_resilient_superclass(self) -> bool {
        self.kind() == CLASS && self.0 & RESILIENT_SUPERCLASS != 0
    }

    pub fn immediate_members_negative(self) -> bool {
        self.kind() == CLASS && self.0 & IMMEDIATE_MEMBERS_NEGATIVE != 0
    }

    fn has_vtable(self) -> bool {
        self.kind() == CLASS && self.0 & VTABLE != 0
    }

    fn has_override_table(self) -> bool {
        self.kind() == CLASS && self.0 & OVERRIDE_TABLE != 0
    }

    /// How the resilient superclass is named: 0 a descriptor, 1 a pointer to one, 2 an
    /// Objective-C class name, 3 a pointer to an Objective-C class.
    pub fn resilient_superclass_kind(self) -> u32 {
        (self.0 >> RESILIENT_SUPERCLASS_KIND_SHIFT) & 7
    }
}

/// Byte offsets of a descriptor's trailing parts; `None` for parts it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Layout {
    pub generic_header: Option<usize>,
    pub resilient_superclass: Option<usize>,
    pub singleton: Option<usize>,
    pub vtable: Option<usize>,
    pub override_table: Option<usize>,
    pub class_stub: Option<usize>,
}

/// The layout of the descriptor whose 32-bit word at each byte offset `read` gives; `None`
/// for kinds other than classes, structs and enums, and for generic contexts with conditional
/// inverted protocols, whose trailing parts this does not read.
pub fn layout(read: impl Fn(usize) -> u32) -> Option<Layout> {
    let flags = Flags(read(0));
    let mut at = match flags.kind() {
        CLASS => CLASS_FIXED_SIZE,
        STRUCT | ENUM => VALUE_FIXED_SIZE,
        _ => return None,
    };
    let mut found = Layout::default();
    if flags.is_generic() {
        found.generic_header = Some(at);
        let counts = read(at + 8);
        let (parameters, requirements) = ((counts & 0xffff) as usize, (counts >> 16) as usize);
        let generic_flags = read(at + 12) >> 16;
        if generic_flags & CONDITIONAL_INVERTED_PROTOCOLS != 0 {
            return None;
        }
        at += GENERIC_HEADER_SIZE + parameters.next_multiple_of(4);
        at += REQUIREMENT_SIZE * requirements;
        if generic_flags & TYPE_PACKS != 0 {
            let packs = (read(at) & 0xffff) as usize;
            at += PACK_HEADER_SIZE + PACK_SHAPE_SIZE * packs;
        }
    }
    if flags.has_resilient_superclass() {
        found.resilient_superclass = Some(at);
        at += 4;
    }
    match flags.metadata_initialization() {
        FOREIGN_INITIALIZATION => at += 4,
        SINGLETON_INITIALIZATION => {
            found.singleton = Some(at);
            at += SINGLETON_SIZE;
        }
        _ => {}
    }
    if flags.has_vtable() {
        found.vtable = Some(at);
        at += VTABLE_HEADER_SIZE + METHOD_SIZE * read(at + 4) as usize;
    }
    if flags.has_override_table() {
        found.override_table = Some(at);
        at += OVERRIDE_HEADER_SIZE + OVERRIDE_SIZE * read(at) as usize;
    }
    if flags.has_resilient_superclass() && flags.has_singleton_initialization() {
        found.class_stub = Some(at);
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(words: &[(usize, u32)]) -> impl Fn(usize) -> u32 + '_ {
        move |offset| {
            words
                .iter()
                .find(|(at, _)| *at == offset)
                .map_or(0, |&(_, value)| value)
        }
    }

    #[test]
    fn a_plain_class_has_its_vtable_after_its_fields() {
        let words = [(0, CLASS | VTABLE), (48, 3)];
        let found = layout(reader(&words)).unwrap();
        assert_eq!(found.generic_header, None);
        assert_eq!(found.vtable, Some(44));
    }

    #[test]
    fn a_generic_class_skips_its_parameters_and_requirements() {
        let flags = CLASS | GENERIC | VTABLE | OVERRIDE_TABLE;
        let vtable = 44 + 16 + 4 + 12;
        let words = [(0, flags), (52, 1 | (1 << 16)), (vtable + 4, 2)];
        let found = layout(reader(&words)).unwrap();
        assert_eq!(found.generic_header, Some(44));
        assert_eq!(found.vtable, Some(vtable));
        assert_eq!(found.override_table, Some(vtable + 8 + 16));
    }

    #[test]
    fn a_resilient_class_has_its_stub_last() {
        let flags = CLASS | RESILIENT_SUPERCLASS | (SINGLETON_INITIALIZATION << 16) | VTABLE;
        let words = [(0, flags), (64, 1)];
        let found = layout(reader(&words)).unwrap();
        assert_eq!(found.resilient_superclass, Some(44));
        assert_eq!(found.singleton, Some(48));
        assert_eq!(found.vtable, Some(60));
        assert_eq!(found.class_stub, Some(76));
    }

    #[test]
    fn a_generic_struct_has_its_header_after_28_bytes() {
        let words = [(0, STRUCT | GENERIC), (36, 2)];
        let found = layout(reader(&words)).unwrap();
        assert_eq!(found.generic_header, Some(28));
        assert_eq!(found.vtable, None);
        assert_eq!(layout(reader(&[(0, 3)])), None);
    }
}
