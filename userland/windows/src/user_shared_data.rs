//! `KUSER_SHARED_DATA`, the read-only page Windows maps into every process at `0x7ffe0000`
//! (D151), as far as programs read it directly: `ProcessorFeatures`, the table that
//! `IsProcessorFeaturePresent` answers from. MSVC's ARM64 atomics helpers read its
//! `PF_ARM_V81_ATOMIC_INSTRUCTIONS_AVAILABLE` byte to choose between the ARMv8.1 atomic
//! instructions and exclusive load and store pairs.

/// Where the page lies in every process.
pub const ADDRESS: usize = 0x7ffe_0000;
/// The page's size.
pub const SIZE: usize = 4096;
/// Where `ProcessorFeatures`, one byte per feature, starts in the page.
pub const PROCESSOR_FEATURES: usize = 0x274;
/// How many features the table has.
pub const PROCESSOR_FEATURE_COUNT: usize = 64;

#[cfg(target_arch = "x86_64")]
pub const PF_XMMI_INSTRUCTIONS_AVAILABLE: u32 = 6;
#[cfg(target_arch = "x86_64")]
pub const PF_RDTSC_INSTRUCTION_AVAILABLE: u32 = 8;
#[cfg(target_arch = "x86_64")]
pub const PF_XMMI64_INSTRUCTIONS_AVAILABLE: u32 = 10;
pub const PF_NX_ENABLED: u32 = 12;
#[cfg(not(target_arch = "x86_64"))]
pub const PF_ARM_NEON_INSTRUCTIONS_AVAILABLE: u32 = 19;
#[cfg(not(target_arch = "x86_64"))]
pub const PF_ARM_V8_INSTRUCTIONS_AVAILABLE: u32 = 29;
pub const PF_ARM_V81_ATOMIC_INSTRUCTIONS_AVAILABLE: u32 = 34;

/// Linux's `HWCAP_ATOMICS` on aarch64: the ARMv8.1 atomic instructions.
pub const HWCAP_ATOMICS: u64 = 1 << 8;

#[cfg(target_arch = "x86_64")]
const BASELINE: &[u32] = &[
    PF_XMMI_INSTRUCTIONS_AVAILABLE,
    PF_RDTSC_INSTRUCTION_AVAILABLE,
    PF_XMMI64_INSTRUCTIONS_AVAILABLE,
    PF_NX_ENABLED,
];
#[cfg(not(target_arch = "x86_64"))]
const BASELINE: &[u32] = &[
    PF_NX_ENABLED,
    PF_ARM_NEON_INSTRUCTIONS_AVAILABLE,
    PF_ARM_V8_INSTRUCTIONS_AVAILABLE,
];

/// The features present, one bit per feature number, from the hardware capabilities the
/// kernel gave the layer (`AT_HWCAP`): every processor's baseline, plus the ARMv8.1 atomics on
/// aarch64 when the kernel reports them.
pub fn processor_features(hardware_capabilities: u64) -> u64 {
    let baseline = BASELINE.iter().fold(0, |set, feature| set | 1 << feature);
    let atomics = cfg!(target_arch = "aarch64") && hardware_capabilities & HWCAP_ATOMICS != 0;
    baseline | u64::from(atomics) << PF_ARM_V81_ATOMIC_INSTRUCTIONS_AVAILABLE
}

/// Whether `feature` is in `features`, as `IsProcessorFeaturePresent` answers.
pub fn present(features: u64, feature: u32) -> bool {
    (feature as usize) < PROCESSOR_FEATURE_COUNT && features >> feature & 1 != 0
}

/// Writes `features` into the page as Windows lays them out: a byte per feature, 1 if present.
pub fn write_processor_features(page: &mut [u8; SIZE], features: u64) {
    let table = &mut page[PROCESSOR_FEATURES..PROCESSOR_FEATURES + PROCESSOR_FEATURE_COUNT];
    for (feature, byte) in table.iter_mut().enumerate() {
        *byte = u8::from(present(features, feature as u32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_baseline_is_present_whatever_the_kernel_reports() {
        for capabilities in [0, u64::MAX] {
            let features = processor_features(capabilities);
            assert!(present(features, PF_NX_ENABLED));
            for feature in BASELINE {
                assert!(present(features, *feature));
            }
        }
        assert!(!present(u64::MAX, 64));
        assert!(!present(u64::MAX, u32::MAX));
    }

    #[test]
    fn the_armv8_1_atomics_follow_the_kernel_s_hwcap_on_aarch64_only() {
        let without = processor_features(0);
        let with = processor_features(HWCAP_ATOMICS);
        assert!(!present(without, PF_ARM_V81_ATOMIC_INSTRUCTIONS_AVAILABLE));
        assert_eq!(
            present(with, PF_ARM_V81_ATOMIC_INSTRUCTIONS_AVAILABLE),
            cfg!(target_arch = "aarch64")
        );
    }

    #[test]
    fn msvc_s_atomics_helper_reads_its_byte_at_0x7ffe0296() {
        let atomics_byte = 0x7ffe_0296 - ADDRESS;
        assert_eq!(
            atomics_byte,
            PROCESSOR_FEATURES + PF_ARM_V81_ATOMIC_INSTRUCTIONS_AVAILABLE as usize
        );
        let mut page = [0xaa; SIZE];
        write_processor_features(&mut page, 1 << PF_ARM_V81_ATOMIC_INSTRUCTIONS_AVAILABLE);
        assert_eq!(page[atomics_byte], 1);
        assert_eq!(page[PROCESSOR_FEATURES + PF_NX_ENABLED as usize], 0);
        assert_eq!(page[PROCESSOR_FEATURES - 1], 0xaa);
        assert_eq!(page[PROCESSOR_FEATURES + PROCESSOR_FEATURE_COUNT], 0xaa);
        write_processor_features(&mut page, 0);
        assert_eq!(page[atomics_byte], 0);
    }
}
