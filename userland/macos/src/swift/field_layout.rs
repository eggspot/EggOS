//! A Swift class's stored properties laid out after its superclass (D387), as
//! `swift_initClassMetadata2` does when a class's superclass, such as an AppKit class of the
//! layer's, has a size only known at run time: each field at the next offset its alignment
//! allows, from the superclass's instance size.

/// The offsets of fields of `(size, alignment)` placed from `start`, written into `offsets`;
/// the instance size after the last and the instance's alignment mask (at least `minimum_mask`).
pub fn lay_out(start: usize, fields: &[(usize, usize)], offsets: &mut [usize], minimum_mask: usize) -> (usize, usize) {
    let mut at = start;
    let mut mask = minimum_mask;
    for (slot, &(size, alignment)) in offsets.iter_mut().zip(fields) {
        let alignment = alignment.max(1);
        at = at.next_multiple_of(alignment);
        *slot = at;
        at += size;
        mask = mask.max(alignment - 1);
    }
    (at, mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_follow_the_superclass_at_their_alignment() {
        let mut offsets = [0; 3];
        let (size, mask) = lay_out(84, &[(1, 1), (8, 8), (4, 4)], &mut offsets, 7);
        assert_eq!(offsets, [84, 88, 96]);
        assert_eq!(size, 100);
        assert_eq!(mask, 7);
    }

    #[test]
    fn a_class_with_no_fields_keeps_its_superclass_size() {
        assert_eq!(lay_out(48, &[], &mut [], 15), (48, 15));
    }
}
