//! The kernel's random number generator (D56): `AT_RANDOM`, `getrandom`, and later layout
//! randomization and `/dev/urandom`, all draw from it.
//!
//! - **Input.** Seed material ([`eggos_hal::Platform::random_seeds`], and the clock) is hashed
//!   into one BLAKE2s state as it arrives, each piece after its source's tag and its length,
//!   with a count of the bits it is credited with. Hashing, not XOR: a weak or hostile source
//!   cannot cancel the others.
//! - **Seeding** ([`reseed`]) extracts the input's hash into the *base key*:
//!   `key = BLAKE2s(key: extracted, message: old key)`, and moves the base key's generation on.
//!   The generator counts as seeded ([`is_seeded`]) once at least [`SEEDED_BITS`] bits were
//!   credited, as on Linux.
//! - **Output.** Every CPU has its own key ([`CpuKey`], in its per-CPU block), derived from
//!   the base key whenever the base key's generation moves on: one ChaCha20 block of the base
//!   key gives the base key's next value and the CPU's key, so the base key's lock is taken
//!   only then. Each draw is fast key erasure (Bernstein, "Fast-key-erasure random-number
//!   generators", 2017): one ChaCha20 block of the CPU's key, whose first half replaces the
//!   key and whose second half is the output, so no state left in memory reveals output
//!   already handed out. A draw of more than 32 bytes takes those 32 as a key of its own and
//!   streams ChaCha20 from it outside any lock ([`fill_with`]).

pub mod blake2s;
pub mod chacha20;

use crate::cpu;
use crate::sync::SpinLock;
use crate::thread::WaitQueue;
use blake2s::Blake2s;
use chacha20::{BLOCK_SIZE, KEY_SIZE};
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering, compiler_fence};
use eggos_hal::Platform;
use eggos_hal::random::SeedSource;

/// Credited bits that make the generator seeded: a 256-bit key's worth, as on Linux.
pub const SEEDED_BITS: u32 = 256;

/// The nonce of every block: each key is used for one stream only.
const NONCE: [u8; 12] = [0; 12];

/// The bytes one draw hands out from its block: the half that does not become the next key.
const DRAW_SIZE: usize = BLOCK_SIZE - KEY_SIZE;

/// The input not extracted yet.
struct Input {
    hash: Blake2s,
    /// Bits credited since boot (they are not used up: seeded stays seeded).
    credited_bits: u32,
}

static INPUT: SpinLock<Input> = SpinLock::new(Input {
    hash: Blake2s::new(),
    credited_bits: 0,
});

/// The base key every CPU's key is derived from.
static BASE_KEY: SpinLock<[u8; KEY_SIZE]> = SpinLock::new([0; KEY_SIZE]);

/// The base key's generation: moved on by every [`reseed`], under [`BASE_KEY`]'s lock. Starts
/// at 1, so a CPU key's 0 means it was never derived.
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// Whether [`SEEDED_BITS`] bits were credited and extracted ([`is_seeded`]).
static SEEDED: AtomicBool = AtomicBool::new(false);

/// Threads waiting for the generator to be seeded ([`wait_until_seeded`]).
static SEEDED_WAITERS: WaitQueue = WaitQueue::new();

/// A CPU's key, in its per-CPU block. Only the CPU itself takes its lock, for one ChaCha20
/// block; interrupt handlers never draw, so the lock never spins.
pub struct CpuKey {
    key: [u8; KEY_SIZE],
    /// The base key's [`GENERATION`] this key was derived from; 0 if never.
    generation: u64,
}

impl CpuKey {
    pub const fn new() -> Self {
        Self {
            key: [0; KEY_SIZE],
            generation: 0,
        }
    }
}

/// Shows the generation only: the key is secret.
impl core::fmt::Debug for CpuKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CpuKey")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Default for CpuKey {
    fn default() -> Self {
        Self::new()
    }
}

/// How many bits a piece of seed material from `source` is credited with (D56): a loader's
/// `rng-seed` and the CPU's instruction fully, as Linux does by default (`random.trust_bootloader`,
/// `random.trust_cpu`); `kaslr-seed`, whose bytes may have gone into a layout, not at all.
fn credit(source: SeedSource, bytes: &[u8]) -> u32 {
    match source {
        SeedSource::DeviceTreeRngSeed | SeedSource::CpuInstruction => {
            u32::try_from(bytes.len()).map_or(u32::MAX, |length| length.saturating_mul(8))
        }
        SeedSource::DeviceTreeKaslrSeed => 0,
    }
}

/// Hashes `bytes` from `source` into the input, credited as [`credit`] says. Takes effect at
/// the next [`reseed`].
pub fn add_seed(source: SeedSource, bytes: &[u8]) {
    add_input(source.tag(), bytes, credit(source, bytes));
}

/// Hashes `bytes`, after `tag` and their length, into the input, crediting `bits`.
fn add_input(tag: u8, bytes: &[u8], bits: u32) {
    let mut input = INPUT.lock();
    input.hash.update(&[tag]);
    input.hash.update(&(bytes.len() as u64).to_le_bytes());
    input.hash.update(bytes);
    input.credited_bits = input.credited_bits.saturating_add(bits);
}

/// The tag of the clock reads hashed in at each [`reseed`]: after every [`SeedSource`]'s.
const CLOCK_TAG: u8 = 0x80;

/// Extracts the input into the base key, and moves its generation on so that every CPU
/// derives a new key at its next draw. The clock is hashed in first, uncredited. Returns
/// whether the generator is seeded; if this made it so, wakes the threads waiting for that.
pub fn reseed<P: Platform>() -> bool {
    add_input(CLOCK_TAG, &P::clock_ticks().to_le_bytes(), 0);
    let extracted;
    let credited_bits;
    {
        let mut input = INPUT.lock();
        extracted = Secret(core::mem::take(&mut input.hash).finalize());
        // The next extraction depends on this one too.
        input.hash.update(&extracted.0);
        credited_bits = input.credited_bits;
    }
    {
        let mut base = BASE_KEY.lock();
        let mut hash = Blake2s::keyed(&extracted.0);
        hash.update(&*base);
        let key = Secret(hash.finalize());
        base.copy_from_slice(&key.0);
        GENERATION.fetch_add(1, Ordering::Release);
    }
    if credited_bits >= SEEDED_BITS && !SEEDED.swap(true, Ordering::AcqRel) {
        SEEDED_WAITERS.wake_all::<P>();
    }
    is_seeded()
}

/// What seeded the generator at boot ([`seed_at_boot`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootSeeds {
    /// Pieces of seed material the machine offered.
    pub seeds: usize,
    /// Bits they were credited with.
    pub credited_bits: u32,
    /// Whether that seeded the generator.
    pub seeded: bool,
}

/// Seeds the generator from what the machine offers ([`Platform::random_seeds`]) and the
/// clock. Called once at boot, on the boot CPU, before any other CPU starts and before
/// anything draws.
pub fn seed_at_boot<P: Platform>() -> BootSeeds {
    let mut seeds = 0;
    let mut credited_bits = 0u32;
    P::random_seeds(&mut |source, bytes| {
        add_seed(source, bytes);
        seeds += 1;
        credited_bits = credited_bits.saturating_add(credit(source, bytes));
    });
    let seeded = reseed::<P>();
    BootSeeds {
        seeds,
        credited_bits,
        seeded,
    }
}

/// Whether the generator was seeded with [`SEEDED_BITS`] credited bits. Until it is, its
/// output depends only on what was hashed in, which may be guessable: `getrandom` without
/// `GRND_INSECURE` waits (D56).
pub fn is_seeded() -> bool {
    SEEDED.load(Ordering::Acquire)
}

/// Returns once the generator is seeded, running other threads meanwhile. As
/// [`WaitQueue::wait_until`]: with interrupts masked and no lock held.
pub fn wait_until_seeded<P: Platform>() {
    SEEDED_WAITERS.wait_until::<P>(is_seeded);
}

/// One draw from this CPU's key: [`DRAW_SIZE`] bytes, the key replaced (see the module
/// documentation).
fn draw<P: Platform>() -> Secret<DRAW_SIZE> {
    let mut key = cpu::current::<P>().random.lock();
    let generation = GENERATION.load(Ordering::Acquire);
    if key.generation != generation {
        derive_cpu_key(&mut key, generation);
    }
    let block = Secret(chacha20::block(&key.key, 0, &NONCE));
    let (next, output) = block.0.split_at(KEY_SIZE);
    key.key.copy_from_slice(next);
    let mut drawn = Secret([0; DRAW_SIZE]);
    drawn.0.copy_from_slice(output);
    drawn
}

/// Derives `key` from the base key: out of line, once per CPU per generation.
#[cold]
#[inline(never)]
fn derive_cpu_key(key: &mut CpuKey, generation: u64) {
    let mut base = BASE_KEY.lock();
    let block = Secret(chacha20::block(&base, 0, &NONCE));
    let (next, cpu_key) = block.0.split_at(KEY_SIZE);
    base.copy_from_slice(next);
    key.key.copy_from_slice(cpu_key);
    // The generation read before the lock: if a reseed came in between, the next draw
    // derives again.
    key.generation = generation;
}

/// Fills `bytes` with the generator's output, seeded or not (as `GRND_INSECURE`): one draw
/// for up to 32 bytes, so `AT_RANDOM`'s 16 cost one ChaCha20 block.
pub fn fill<P: Platform>(bytes: &mut [u8]) {
    let mut offset = 0;
    let _ = fill_with::<P>(bytes.len(), |chunk| {
        bytes[offset..][..chunk.len()].copy_from_slice(chunk);
        offset += chunk.len();
        ControlFlow::Continue(())
    });
}

/// Hands `length` bytes of the generator's output to `sink`, in chunks of up to 64 bytes,
/// seeded or not, until `sink` breaks. Up to 32 bytes come from one draw; more come from a
/// ChaCha20 stream keyed with one draw, outside any lock, so `sink` may block or pass a
/// preemption point. Every chunk is wiped once `sink` has it, and the stream's key at the end.
pub fn fill_with<P: Platform>(
    length: usize,
    mut sink: impl FnMut(&[u8]) -> ControlFlow<()>,
) -> ControlFlow<()> {
    if length == 0 {
        return ControlFlow::Continue(());
    }
    let drawn = draw::<P>();
    if length <= DRAW_SIZE {
        return sink(&drawn.0[..length]);
    }
    let mut flow = ControlFlow::Continue(());
    let mut left = length;
    let mut counter = 0u32;
    while left > 0 && flow.is_continue() {
        let block = Secret(chacha20::block(&drawn.0, counter, &NONCE));
        let take = left.min(BLOCK_SIZE);
        flow = sink(&block.0[..take]);
        left -= take;
        // At most `MAX_RW_COUNT` bytes per call: far fewer than 2^32 blocks.
        counter = counter.wrapping_add(1);
    }
    flow
}

/// Secret bytes (keys, output not handed out yet), wiped when dropped: 8-byte aligned, so the
/// wipe is one store per 8 bytes, not one per byte. Best effort, as everywhere in Rust and C:
/// a copy the compiler makes when it moves one (a return, a spill) is not wiped.
#[repr(C, align(8))]
struct Secret<const N: usize>([u8; N]);

impl<const N: usize> Drop for Secret<N> {
    fn drop(&mut self) {
        const { assert!(N.is_multiple_of(8), "a secret is whole words") };
        let words = self.0.as_mut_ptr().cast::<u64>();
        for word in 0..N / 8 {
            // SAFETY: `Secret` is 8-byte aligned and `N` a multiple of 8, so word `word` is
            // inside it and aligned. Volatile, so the compiler keeps stores to memory about
            // to be dropped.
            unsafe { words.wrapping_add(word).write_volatile(0) };
        }
        compiler_fence(Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TestPlatform as P;

    #[test]
    fn draws_differ_and_never_repeat_a_key() {
        let mut first = [0u8; 16];
        let mut second = [0u8; 16];
        fill::<P>(&mut first);
        fill::<P>(&mut second);
        assert_ne!(first, second);
        assert_ne!(first, [0; 16]);
    }

    #[test]
    fn long_fills_are_filled_to_the_end() {
        for length in [1, 31, 32, 33, 64, 65, 200, 4096] {
            let mut bytes = vec![0u8; length];
            fill::<P>(&mut bytes);
            // No 16-byte stretch of zeros, as an unfilled tail would leave.
            assert!(
                bytes
                    .chunks(16)
                    .all(|chunk| chunk.iter().any(|&byte| byte != 0)),
                "a fill of {length} bytes"
            );
        }
    }

    #[test]
    fn a_sink_that_breaks_gets_no_more() {
        let mut chunks = 0;
        let flow = fill_with::<P>(1000, |chunk| {
            assert_eq!(chunk.len(), BLOCK_SIZE);
            chunks += 1;
            if chunks == 3 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert_eq!(flow, ControlFlow::Break(()));
        assert_eq!(chunks, 3);
    }

    #[test]
    fn a_reseed_gives_this_cpu_a_new_key() {
        let before = GENERATION.load(Ordering::Acquire);
        reseed::<P>();
        assert!(GENERATION.load(Ordering::Acquire) > before);
        let mut bytes = [0u8; 8];
        fill::<P>(&mut bytes);
        let key = cpu::current::<P>().random.lock();
        assert!(key.generation > before);
    }

    #[test]
    fn credits_follow_the_policy() {
        assert_eq!(credit(SeedSource::DeviceTreeRngSeed, &[0; 32]), 256);
        assert_eq!(credit(SeedSource::CpuInstruction, &[0; 32]), 256);
        assert_eq!(credit(SeedSource::DeviceTreeKaslrSeed, &[0; 8]), 0);
    }
}
