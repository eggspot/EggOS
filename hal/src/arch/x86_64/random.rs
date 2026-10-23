//! Seed material from the CPU's random number instructions (D56): `RDSEED`, which reads the
//! hardware entropy source's conditioned output, or `RDRAND`, the CPU's own generator seeded
//! from it, where the CPU has no `RDSEED` (Intel's Digital Random Number Generator software
//! implementation guide; AMD's equivalents behave the same).

use core::arch::x86_64::{__cpuid, __cpuid_count, _rdrand64_step, _rdseed64_step};

/// Bytes of seed material taken: 256 bits, what seeds the kernel's generator.
pub(super) const SEED_SIZE: usize = 32;

/// Tries per `RDSEED` word: it fails, briefly, when the entropy source cannot keep up.
const RDSEED_TRIES: usize = 100;
/// Tries per `RDRAND` word: Intel's guide treats ten failures in a row as a broken CPU.
const RDRAND_TRIES: usize = 10;

/// Fills `seed` from `RDSEED`, else `RDRAND`; false if the CPU has neither or the one it has
/// does not deliver. A sample of all ones, or equal to the one before, is thrown away and
/// counts as a failed try: some CPUs have reported success with stuck values (AMD's RDRAND
/// after resume on some Zen 2 parts, all ones). The caller hashes the bytes with its other
/// inputs, so a CPU source that is weak, or worse, cannot undo them.
pub(super) fn fill(seed: &mut [u8; SEED_SIZE]) -> bool {
    let max_leaf = __cpuid(0).eax;
    let has_rdseed = max_leaf >= 7 && __cpuid_count(7, 0).ebx & 1 << 18 != 0;
    let has_rdrand = __cpuid(1).ecx & 1 << 30 != 0;
    (has_rdseed
        // SAFETY: CPUID says the CPU has `RDSEED`.
        && take(seed, RDSEED_TRIES, || unsafe { rdseed() }))
        || (has_rdrand
            // SAFETY: CPUID says the CPU has `RDRAND`.
            && take(seed, RDRAND_TRIES, || unsafe { rdrand() }))
}

/// Fills `seed` from `sample`, a word at a time, giving each word `tries` tries.
fn take(seed: &mut [u8; SEED_SIZE], tries: usize, mut sample: impl FnMut() -> Option<u64>) -> bool {
    let mut previous = None;
    for word in seed.as_chunks_mut::<8>().0 {
        let good = (0..tries).find_map(|_| {
            let value = sample().filter(|&value| value != u64::MAX && Some(value) != previous);
            if value.is_none() {
                core::hint::spin_loop();
            }
            value
        });
        let Some(value) = good else {
            return false;
        };
        *word = value.to_le_bytes();
        previous = Some(value);
    }
    true
}

/// One `RDSEED`: `None` if it had nothing ready (carry clear).
///
/// # Safety
/// The CPU must have `RDSEED` (`CPUID.(EAX=7,ECX=0):EBX[18]`).
#[target_feature(enable = "rdseed")]
unsafe fn rdseed() -> Option<u64> {
    let mut value = 0;
    (_rdseed64_step(&mut value) == 1).then_some(value)
}

/// One `RDRAND`: `None` if it had nothing ready (carry clear).
///
/// # Safety
/// The CPU must have `RDRAND` (`CPUID.1:ECX[30]`).
#[target_feature(enable = "rdrand")]
unsafe fn rdrand() -> Option<u64> {
    let mut value = 0;
    (_rdrand64_step(&mut value) == 1).then_some(value)
}
