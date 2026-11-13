//! Address-space tags (D552): x86_64's PCIDs, AArch64's ASIDs and RISC-V's ASIDs let a CPU keep
//! the translations of a few user address spaces at once, so switching back to one it ran
//! recently drops nothing. Each CPU keeps [`TAG_SLOTS`] slots; slot `i` is hardware tag `i + 1`
//! (tag 0 stays the kernel's, whose low half is empty). A slot remembers which address space it
//! holds ([`AddressSpaceTag::id`], never reused) and the generation of that space's mappings it
//! last saw: a space whose mappings were removed or changed since ([`AddressSpaceTag::generation`]
//! moved on) has its slot's translations dropped when it is switched to.

use core::cell::UnsafeCell;

/// Which user address space the kernel switches to, and how far its mappings have changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AddressSpaceTag {
    /// Unique for the kernel's lifetime; 0 for none.
    pub id: u64,
    /// Moves on every time a mapping of the space is removed or loses access.
    pub generation: u64,
}

/// Slots per CPU: Linux keeps 6 PCIDs per CPU; 8 fits every architecture's smallest tag field.
pub const TAG_SLOTS: usize = 8;

/// A CPU's slots. Only the CPU itself uses them, with interrupts masked.
#[derive(Debug)]
pub struct AddressSpaceTags(UnsafeCell<TagSlots>);

#[derive(Debug)]
struct TagSlots {
    slots: [AddressSpaceTag; TAG_SLOTS],
    next: usize,
}

// SAFETY: each CPU's block is touched only by that CPU, with interrupts masked (the `choose`
// contract), so the cell is never reached from two places at once.
unsafe impl Sync for AddressSpaceTags {}

/// The hardware tag to switch with, and whether its old translations must be dropped first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagChoice {
    /// 1 to [`TAG_SLOTS`].
    pub hardware_tag: u16,
    pub drop_old: bool,
}

impl AddressSpaceTags {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(TagSlots {
            slots: [AddressSpaceTag {
                id: 0,
                generation: 0,
            }; TAG_SLOTS],
            next: 0,
        }))
    }

    /// The slot for `tag`: the one already holding its address space, whose translations stay
    /// unless its generation moved on, else the oldest slot taken over, its translations dropped.
    ///
    /// # Safety
    /// Only the CPU these slots belong to may call it, with interrupts masked.
    pub unsafe fn choose(&self, tag: AddressSpaceTag) -> TagChoice {
        // SAFETY: the caller's promise: nothing else reaches the slots meanwhile.
        let slots = unsafe { &mut *self.0.get() };
        if let Some(index) = slots.slots.iter().position(|slot| slot.id == tag.id) {
            let drop_old = slots.slots[index].generation != tag.generation;
            slots.slots[index].generation = tag.generation;
            return TagChoice {
                hardware_tag: index as u16 + 1,
                drop_old,
            };
        }
        let index = slots.next;
        slots.next = (index + 1) % TAG_SLOTS;
        slots.slots[index] = tag;
        TagChoice {
            hardware_tag: index as u16 + 1,
            drop_old: true,
        }
    }
}

impl Default for AddressSpaceTags {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(id: u64, generation: u64) -> AddressSpaceTag {
        AddressSpaceTag { id, generation }
    }

    #[test]
    fn a_space_keeps_its_slot_until_its_mappings_change_or_it_is_evicted() {
        let tags = AddressSpaceTags::new();
        // SAFETY: a host test: one thread, no interrupts.
        let choose = |t| unsafe { tags.choose(t) };
        let first = choose(tag(1, 0));
        assert!(first.drop_old);
        assert_eq!(choose(tag(1, 0)), TagChoice { drop_old: false, ..first });
        assert!(choose(tag(1, 1)).drop_old);
        for id in 2..=TAG_SLOTS as u64 {
            assert!(choose(tag(id, 0)).drop_old);
        }
        assert!(!choose(tag(1, 1)).drop_old);
        assert!(choose(tag(100, 0)).drop_old);
        assert!(choose(tag(1, 1)).drop_old);
    }
}
