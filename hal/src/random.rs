//! Seed material for the kernel's random number generator (D56): what the machine offers, and
//! where it came from. The HAL only gathers it ([`crate::Platform::random_seeds`]); how much
//! each source is trusted is the kernel's policy.

/// Where a piece of seed material came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedSource {
    /// The device tree's `/chosen/rng-seed`: random bytes from the firmware or boot loader
    /// (QEMU's `virt` machines give 32).
    DeviceTreeRngSeed,
    /// The device tree's `/chosen/kaslr-seed`: a value meant for the kernel's layout
    /// randomization, from the same loaders.
    DeviceTreeKaslrSeed,
    /// The CPU's own random number instruction: x86's `RDSEED`, or `RDRAND` where there is no
    /// `RDSEED`.
    CpuInstruction,
}

impl SeedSource {
    /// A distinct byte per source, which the kernel hashes in before each piece.
    pub const fn tag(self) -> u8 {
        match self {
            Self::DeviceTreeRngSeed => 1,
            Self::DeviceTreeKaslrSeed => 2,
            Self::CpuInstruction => 3,
        }
    }
}

/// Overwrites `bytes` with zeros in a way the compiler keeps, for seed material and keys that
/// must not stay in memory after use.
pub fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` is a valid, aligned, exclusive reference.
        unsafe { core::ptr::write_volatile(byte, 0) };
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
