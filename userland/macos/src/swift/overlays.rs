//! The Swift overlays a program links beside `libswiftCore` (D180), as far as they are
//! runtime: `libswiftObjectiveC`'s `Selector ==` (selectors are unique, so equal names are
//! equal addresses) and `libswiftCoreFoundation`'s `CGFloat` (`metadata.rs`, a `Double` in the
//! `CoreGraphics` module). The overlays a program links weakly and imports nothing from load as
//! empty libraries.

extern "C" fn selector_equal(left: usize, right: usize) -> bool {
    left == right
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    (name == b"_$s10ObjectiveC8SelectorV2eeoiySbAC_ACtFZ")
        .then_some(selector_equal as *const () as u64)
}
