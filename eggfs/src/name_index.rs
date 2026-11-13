use crate::layout::{BLOCK_SIZE, put_u32, u32_at};

pub const ROOT_MAGIC: [u8; 8] = *b"EGGFSNIX";
pub const MAX_BUCKETS: u32 = 512;
pub const PAIRS_PER_BUCKET: u32 = ((BLOCK_SIZE - PAIRS_AT) / PAIR_BYTES) as u32;
const BUCKETS_AT: usize = 8;
const POINTERS_AT: usize = 16;
const PAIRS_AT: usize = 8;
const PAIR_BYTES: usize = 8;
const FILL_DIVISOR: u32 = 2;

pub fn buckets_for(entries: u32) -> u32 {
    (entries.max(1) * FILL_DIVISOR)
        .div_ceil(PAIRS_PER_BUCKET)
        .next_power_of_two()
}

pub fn bucket_of(hash: u32, buckets: u32) -> u32 {
    hash.rotate_right(16) & (buckets - 1)
}

pub fn root_buckets(root: &[u8]) -> Option<u32> {
    if root[..8] != ROOT_MAGIC {
        return None;
    }
    let buckets = u32_at(root, BUCKETS_AT);
    (buckets.is_power_of_two() && buckets <= MAX_BUCKETS).then_some(buckets)
}

pub fn bucket_pointer(root: &[u8], bucket: u32) -> u32 {
    u32_at(root, POINTERS_AT + 4 * bucket as usize)
}

pub fn write_root(root: &mut [u8], pointers: &[u32]) {
    root.fill(0);
    root[..8].copy_from_slice(&ROOT_MAGIC);
    put_u32(root, BUCKETS_AT, pointers.len() as u32);
    for (bucket, &pointer) in pointers.iter().enumerate() {
        put_u32(root, POINTERS_AT + 4 * bucket, pointer);
    }
}

pub fn bucket_len(bucket: &[u8]) -> Option<u32> {
    let length = u32_at(bucket, 0);
    (length <= PAIRS_PER_BUCKET).then_some(length)
}

pub fn pair(bucket: &[u8], position: u32) -> (u32, u32) {
    let at = PAIRS_AT + PAIR_BYTES * position as usize;
    (u32_at(bucket, at), u32_at(bucket, at + 4))
}

pub fn holds(bucket: &[u8], hash: u32, block_index: u32) -> bool {
    let length = bucket_len(bucket).unwrap_or(0);
    (0..length).any(|position| pair(bucket, position) == (hash, block_index))
}

pub fn push_pair(bucket: &mut [u8], hash: u32, block_index: u32) -> bool {
    let Some(length) = bucket_len(bucket) else {
        return false;
    };
    if holds(bucket, hash, block_index) {
        return true;
    }
    if length >= PAIRS_PER_BUCKET {
        return false;
    }
    let at = PAIRS_AT + PAIR_BYTES * length as usize;
    put_u32(bucket, at, hash);
    put_u32(bucket, at + 4, block_index);
    put_u32(bucket, 0, length + 1);
    true
}
