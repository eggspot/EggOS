//! Translating an architecture's own Linux constants to the generic ones the kernel takes, and
//! back: MIPS's (D315) and SPARC64's (D327) flags, commands and options.

/// The pairs (the architecture's, generic) of one kind of flag word or value; bits or values in
/// neither pass unchanged.
pub type FlagPairs = &'static [(u64, u64)];

/// The flag word `value`, in the architecture's bits, in the generic ones.
pub fn flags_to_generic(value: u64, pairs: FlagPairs) -> u64 {
    let known = pairs.iter().fold(0, |known, &(own, _)| known | own);
    pairs
        .iter()
        .filter(|&&(own, _)| value & own == own)
        .fold(value & !known, |out, &(_, generic)| out | generic)
}

/// The flag word `value`, in the generic bits, in the architecture's.
pub fn flags_to_architecture(value: u64, pairs: FlagPairs) -> u64 {
    let known = pairs.iter().fold(0, |known, &(_, generic)| known | generic);
    pairs
        .iter()
        .filter(|&&(_, generic)| value & generic == generic)
        .fold(value & !known, |out, &(own, _)| out | own)
}

/// The value `value`, the architecture's, as the generic one.
pub fn value_to_generic(value: u64, pairs: FlagPairs) -> u64 {
    pairs
        .iter()
        .find(|&&(own, _)| own == value)
        .map_or(value, |&(_, generic)| generic)
}

/// The value `value`, generic, as the architecture's.
pub fn value_to_architecture(value: u64, pairs: FlagPairs) -> u64 {
    pairs
        .iter()
        .find(|&&(_, generic)| generic == value)
        .map_or(value, |&(own, _)| own)
}
