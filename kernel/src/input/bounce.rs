//! Bounce keys (D602, docs/ux/accessibility-v0.4.md section 3.2): a press of a key within the
//! delay of the same key's last accepted release is dropped, with its repeats and its release,
//! as if it never happened. Checked before slow keys and sticky keys; another key is never
//! affected.

const KEYS: usize = 256;
const WORD_BITS: usize = 64;

/// Each key's last accepted release, and the keys whose press was dropped.
#[derive(Debug)]
pub(super) struct BounceKeys {
    released_at: [u64; KEYS],
    dropped: [u64; KEYS / WORD_BITS],
}

impl BounceKeys {
    pub(super) const fn new() -> Self {
        Self {
            released_at: [0; KEYS],
            dropped: [0; KEYS / WORD_BITS],
        }
    }

    /// Whether key `code`'s change `value` (1 pressed, 2 repeated, 0 released) at `now` is
    /// dropped, a press within `within` ticks of its last accepted release being a bounce.
    pub(super) fn drops(&mut self, code: u16, value: i32, now: u64, within: u64) -> bool {
        let index = usize::from(code);
        if index >= KEYS {
            return false;
        }
        let (word, bit) = (index / WORD_BITS, 1u64 << (index % WORD_BITS));
        match value {
            1 => {
                let last = self.released_at[index];
                let bounced = last != 0 && now.saturating_sub(last) < within;
                if bounced {
                    self.dropped[word] |= bit;
                }
                bounced
            }
            2 => self.dropped[word] & bit != 0,
            0 if self.dropped[word] & bit != 0 => {
                self.dropped[word] &= !bit;
                true
            }
            0 => {
                self.released_at[index] = now.max(1);
                false
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_press_soon_after_the_release_is_dropped_with_its_release() {
        let mut bounce = BounceKeys::new();
        assert!(!bounce.drops(30, 1, 1000, 300));
        assert!(!bounce.drops(30, 0, 1100, 300));
        assert!(bounce.drops(30, 1, 1200, 300));
        assert!(bounce.drops(30, 2, 1250, 300));
        assert!(bounce.drops(30, 0, 1300, 300));
        assert!(!bounce.drops(31, 1, 1310, 300));
        assert!(!bounce.drops(30, 1, 1500, 300));
    }
}
