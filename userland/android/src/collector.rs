//! The Android layer's garbage collector (D759): every object the layer makes is recorded in
//! a heap set; when enough objects were made since the last collection, the objects reachable
//! from the roots are marked (the layer's own roots, and every word of the native stack and
//! the callee-saved registers, conservatively, since compiled code keeps references in
//! registers and stack slots without maps) by following each object's references precisely,
//! and the others are freed.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// The fewest objects made between two collections.
pub const FEWEST_BETWEEN: usize = 4096;

/// The layer's objects and how many were made since the last collection.
#[derive(Debug, Default)]
pub struct Heap {
    pub objects: BTreeSet<u64>,
    made_since: usize,
}

impl Heap {
    /// Records the new object at `address`.
    pub fn note(&mut self, address: u64) {
        self.objects.insert(address);
        self.made_since += 1;
    }

    /// Whether a collection is due: as many objects made since the last as are alive, and at
    /// least [`FEWEST_BETWEEN`].
    pub fn due(&self) -> bool {
        self.made_since >= FEWEST_BETWEEN.max(self.objects.len())
    }

    /// Starts counting again after a collection.
    pub fn collected(&mut self) {
        self.made_since = 0;
    }
}

/// The objects reachable from `roots` among `objects`, each object's references given by
/// `references` (which pushes them onto its second argument). Any root or reference that is
/// not an object is ignored, so roots may be read conservatively.
pub fn trace(
    roots: impl Iterator<Item = u64>,
    objects: &BTreeSet<u64>,
    references: impl Fn(u64, &mut Vec<u64>),
) -> BTreeSet<u64> {
    let mut marked = BTreeSet::new();
    let mut pending: Vec<u64> = roots.filter(|root| objects.contains(root)).collect();
    let mut found = Vec::new();
    while let Some(address) = pending.pop() {
        if !marked.insert(address) {
            continue;
        }
        found.clear();
        references(address, &mut found);
        pending.extend(
            found
                .iter()
                .copied()
                .filter(|reference| objects.contains(reference) && !marked.contains(reference)),
        );
    }
    marked
}

/// The objects of `objects` that `marked` does not hold: what a collection frees.
pub fn unmarked(objects: &BTreeSet<u64>, marked: &BTreeSet<u64>) -> Vec<u64> {
    objects.difference(marked).copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::vec;

    #[test]
    fn what_no_root_reaches_is_unmarked_cycles_included() {
        let edges: BTreeMap<u64, Vec<u64>> = [
            (0x1000, vec![0x2000, 7]),
            (0x2000, vec![0x1000]),
            (0x3000, vec![0x4000]),
            (0x4000, vec![0x3000]),
            (0x5000, vec![]),
        ]
        .into_iter()
        .collect();
        let objects: BTreeSet<u64> = edges.keys().copied().collect();
        let marked = trace([0x1000, 42, 0x5000].into_iter(), &objects, |address, out| {
            out.extend(edges.get(&address).into_iter().flatten().copied())
        });
        assert_eq!(marked, [0x1000, 0x2000, 0x5000].into_iter().collect());
        assert_eq!(unmarked(&objects, &marked), vec![0x3000, 0x4000]);
    }

    #[test]
    fn a_collection_waits_for_as_many_new_objects_as_live_ones() {
        let mut heap = Heap::default();
        for address in 0..FEWEST_BETWEEN as u64 - 1 {
            heap.note(address * 16);
        }
        assert!(!heap.due());
        heap.note(0xffff_0000);
        assert!(heap.due());
        heap.collected();
        assert!(!heap.due());
    }
}
