//! Pages of a list (D164, docs/ux/command-model.md "Pages"): a list command's `--limit <count>`
//! and `--after <cursor>`, and a list tool's `limit` and `after`. A cursor is the key of the
//! last item a page showed (a name, a process or window id), with every byte outside
//! [`is_cursor_byte`]'s set, and a leading `-`, written as `%XX`, so that any key is one word
//! that never reads as an option. When more items follow a page, the command's last line on
//! standard error is [`MORE_PREFIX`] and the cursor; with `--json` it is
//! `{"page":{"truncated":true,"truncated_by":"limit","next_cursor":"<cursor>"}}`.

/// The longest key a cursor holds: a file name.
pub const MAX_KEY_BYTES: usize = 255;
/// The longest cursor: a key of [`MAX_KEY_BYTES`] with every byte encoded.
pub const MAX_CURSOR_BYTES: usize = 3 * MAX_KEY_BYTES;
/// The last line of a command's standard error when more items follow its page: this, then
/// the cursor.
pub const MORE_PREFIX: &str = "more: --after ";
/// The JSON Schema pattern of a cursor, for the tools' `after` argument.
pub const CURSOR_PATTERN: &str = "^[A-Za-z0-9._~@+,=:%][A-Za-z0-9._~@+,=:%-]*$";

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Whether `byte` stands for itself in a cursor (`-` only after the first byte).
pub fn is_cursor_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"._~@+,=:-".contains(&byte)
}

/// Writes `key` as a cursor into `out`; the cursor's length, or `None` when `out` is too small.
pub fn encode(key: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut length = 0;
    for (index, &byte) in key.iter().enumerate() {
        if is_cursor_byte(byte) && !(index == 0 && byte == b'-') {
            *out.get_mut(length)? = byte;
            length += 1;
            continue;
        }
        let high = HEX.get(usize::from(byte >> 4)).copied()?;
        let low = HEX.get(usize::from(byte & 0xf)).copied()?;
        out.get_mut(length..length + 3)?
            .copy_from_slice(&[b'%', high, low]);
        length += 3;
    }
    Some(length)
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// Decodes `cursor` into `out`; the key's length, or `None` when it is not a cursor: empty,
/// starting with `-`, holding a byte outside the set or a `%` without two hex digits, or longer
/// than `out`.
pub fn decode(cursor: &[u8], out: &mut [u8]) -> Option<usize> {
    if cursor.is_empty() || cursor.first() == Some(&b'-') {
        return None;
    }
    let mut length = 0;
    let mut rest = cursor;
    while let Some((&byte, after)) = rest.split_first() {
        let (value, next) = match byte {
            b'%' => match after {
                [high, low, next @ ..] => ((hex_value(*high)? << 4) | hex_value(*low)?, next),
                _ => return None,
            },
            byte if is_cursor_byte(byte) => (byte, after),
            _ => return None,
        };
        *out.get_mut(length)? = value;
        length += 1;
        rest = next;
    }
    Some(length)
}

/// Whether `cursor` is one a command writes: 1 to [`MAX_CURSOR_BYTES`] bytes of the set and
/// `%`, not starting with `-`.
pub fn is_cursor(cursor: &[u8]) -> bool {
    !cursor.is_empty()
        && cursor.len() <= MAX_CURSOR_BYTES
        && cursor.first() != Some(&b'-')
        && cursor
            .iter()
            .all(|&byte| byte == b'%' || is_cursor_byte(byte))
}

/// The next page's cursor, when the last line of `errors` (a command's standard error) says
/// that more items follow; never anything else a program wrote there.
pub fn next_cursor(errors: &[u8]) -> Option<&[u8]> {
    let errors = errors.strip_suffix(b"\n")?;
    let line = match errors.iter().rposition(|&byte| byte == b'\n') {
        Some(end) => errors.get(end + 1..)?,
        None => errors,
    };
    let cursor = line.strip_prefix(MORE_PREFIX.as_bytes())?;
    is_cursor(cursor).then_some(cursor)
}

/// Writes item `index`'s key into the buffer and returns its length (at most
/// [`MAX_KEY_BYTES`]): the key a fixed list pages by.
pub type ItemKey<'k> = &'k dyn Fn(usize, &mut [u8; MAX_KEY_BYTES]) -> usize;

/// Where item `index` of a fixed list of `count` items stands among the items whose keys come
/// after `after` (every item when `None`), by key order: `Some(0)` for the smallest, `None` when
/// its key does not come after the cursor. A list that shows its items in its own order pages by
/// key with it: a page of `limit` holds the items ranked below `limit`.
pub fn rank(count: usize, index: usize, after: Option<&[u8]>, key: ItemKey<'_>) -> Option<usize> {
    let mut own = [0u8; MAX_KEY_BYTES];
    let own_length = key(index, &mut own);
    let own = own.get(..own_length)?;
    if after.is_some_and(|after| own <= after) {
        return None;
    }
    let mut other = [0u8; MAX_KEY_BYTES];
    let mut rank = 0;
    for item in 0..count {
        let length = key(item, &mut other);
        let other = other.get(..length).unwrap_or_default();
        if other < own && after.is_none_or(|after| other > after) {
            rank += 1;
        }
    }
    Some(rank)
}

/// The item that ends a page of `limit` items of a fixed list (see [`rank`]) when more items
/// than `limit` come after `after`: its key is the next page's cursor. `None` on the last page.
pub fn page_end(
    count: usize,
    limit: usize,
    after: Option<&[u8]>,
    key: ItemKey<'_>,
) -> Option<usize> {
    let following = (0..count)
        .filter(|&index| rank(count, index, after, key).is_some())
        .count();
    if following <= limit {
        return None;
    }
    let last = limit.checked_sub(1)?;
    (0..count).find(|&index| rank(count, index, after, key) == Some(last))
}

/// The `limit` smallest keys of those offered in any order (each at most `K` bytes), so that a
/// command whose items come unsorted can show one page in key order with a fixed amount of
/// memory: `N` must be more than the largest limit.
#[derive(Debug, Clone)]
pub struct Smallest<const K: usize, const N: usize> {
    keys: [[u8; K]; N],
    lengths: [usize; N],
    count: usize,
    limit: usize,
    more: bool,
}

impl<const K: usize, const N: usize> Smallest<K, N> {
    /// Keeps the `limit` smallest keys (at least 1, at most `N - 1`).
    pub fn new(limit: usize) -> Self {
        Self {
            keys: [[0; K]; N],
            lengths: [0; N],
            count: 0,
            limit: limit.clamp(1, N.saturating_sub(1).max(1)),
            more: false,
        }
    }

    fn key(&self, index: usize) -> &[u8] {
        match (self.keys.get(index), self.lengths.get(index)) {
            (Some(key), Some(&length)) => key.get(..length).unwrap_or_default(),
            _ => &[],
        }
    }

    /// Offers `key`; a key longer than `K` bytes is never kept, and counts as one that follows.
    pub fn offer(&mut self, key: &[u8]) {
        if key.len() > K {
            self.more = true;
            return;
        }
        let at = (0..self.count)
            .position(|index| key < self.key(index))
            .unwrap_or(self.count);
        if at >= self.limit {
            self.more = true;
            return;
        }
        if self.count == self.limit {
            self.more = true;
            self.count -= 1;
        }
        let mut index = self.count;
        while index > at {
            if let Some(previous) = index.checked_sub(1) {
                self.keys.swap(index, previous);
                self.lengths.swap(index, previous);
            }
            index -= 1;
        }
        if let (Some(place), Some(length)) = (self.keys.get_mut(at), self.lengths.get_mut(at)) {
            if let Some(room) = place.get_mut(..key.len()) {
                room.copy_from_slice(key);
            }
            *length = key.len();
        }
        self.count += 1;
    }

    /// Whether more keys were offered than the page holds.
    pub fn more(&self) -> bool {
        self.more
    }

    /// The page's last key: the next page's cursor when [`Self::more`].
    pub fn last(&self) -> Option<&[u8]> {
        self.count.checked_sub(1).map(|index| self.key(index))
    }

    /// Whether `key` is on the page.
    pub fn holds(&self, key: &[u8]) -> bool {
        (0..self.count).any(|index| self.key(index) == key)
    }

    /// The page's keys, smallest first.
    pub fn keys(&self) -> impl Iterator<Item = &[u8]> {
        (0..self.count).map(|index| self.key(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(key: &[u8]) -> String {
        let mut out = [0u8; MAX_CURSOR_BYTES];
        let length = encode(key, &mut out).unwrap();
        String::from_utf8(out[..length].to_vec()).unwrap()
    }

    fn decoded(cursor: &str) -> Option<Vec<u8>> {
        let mut out = [0u8; MAX_KEY_BYTES];
        decode(cursor.as_bytes(), &mut out).map(|length| out[..length].to_vec())
    }

    #[test]
    fn plain_keys_stay_readable_and_odd_ones_become_one_safe_word() {
        assert_eq!(encoded(b"notes.txt"), "notes.txt");
        assert_eq!(encoded(b"129"), "129");
        assert_eq!(encoded(b"my notes"), "my%20notes");
        assert_eq!(encoded(b"-rf"), "%2Drf");
        assert_eq!(encoded(b"a-b"), "a-b");
        assert_eq!(encoded(b"50%\n"), "50%25%0A");
        assert_eq!(encoded("caf\u{e9}".as_bytes()), "caf%C3%A9");
        let mut small = [0u8; 2];
        assert_eq!(encode(b"abc", &mut small), None);
        assert_eq!(encode(b" ", &mut small), None);
    }

    #[test]
    fn every_key_comes_back_from_its_cursor() {
        for key in [
            &b"notes.txt"[..],
            b"my notes",
            b"-rf",
            b"%",
            b"\x00\xff",
            b"a\nmore: x",
        ] {
            let cursor = encoded(key);
            assert!(is_cursor(cursor.as_bytes()), "{cursor}");
            assert_eq!(decoded(&cursor).as_deref(), Some(key));
        }
        assert_eq!(decoded("%c3%a9").unwrap(), "\u{e9}".as_bytes());
        for bad in ["", "-x", "a b", "%", "%4", "%zz", "a/b", "a\"b"] {
            assert_eq!(decoded(bad), None, "{bad:?}");
        }
        let mut short = [0u8; 2];
        assert_eq!(decode(b"abc", &mut short), None);
        let longest = vec![0xffu8; MAX_KEY_BYTES];
        assert_eq!(encoded(&longest).len(), MAX_CURSOR_BYTES);
    }

    #[test]
    fn the_next_cursor_is_read_only_from_the_last_line_of_standard_error() {
        assert_eq!(
            next_cursor(b"more: --after notes.txt\n"),
            Some(&b"notes.txt"[..])
        );
        assert_eq!(
            next_cursor(b"ls: cannot access 'x'\nmore: --after a%20b\n"),
            Some(&b"a%20b"[..])
        );
        for not in [
            &b""[..],
            b"more: --after notes.txt",
            b"more: --after notes.txt\nls: oops\n",
            b"more: --after \n",
            b"more: --after a b\n",
            b"more: --after -x\n",
            b"  more: --after x\n",
        ] {
            assert_eq!(next_cursor(not), None, "{not:?}");
        }
        let long = format!("{MORE_PREFIX}{}\n", "a".repeat(MAX_CURSOR_BYTES + 1));
        assert_eq!(next_cursor(long.as_bytes()), None);
    }

    #[test]
    fn a_fixed_list_pages_by_key_and_keeps_its_own_order_within_a_page() {
        const NAMES: [&str; 5] = ["pear", "apple", "fig", "kiwi", "banana"];
        let key = |index: usize, out: &mut [u8; MAX_KEY_BYTES]| {
            let name = NAMES[index].as_bytes();
            out[..name.len()].copy_from_slice(name);
            name.len()
        };
        let ranks: Vec<Option<usize>> = (0..NAMES.len())
            .map(|index| rank(NAMES.len(), index, None, &key))
            .collect();
        assert_eq!(ranks, [Some(4), Some(0), Some(2), Some(3), Some(1)]);
        assert_eq!(page_end(NAMES.len(), 2, None, &key), Some(4));
        let after = Some(&b"banana"[..]);
        let ranks: Vec<Option<usize>> = (0..NAMES.len())
            .map(|index| rank(NAMES.len(), index, after, &key))
            .collect();
        assert_eq!(ranks, [Some(2), None, Some(0), Some(1), None]);
        assert_eq!(page_end(NAMES.len(), 2, after, &key), Some(3));
        assert_eq!(page_end(NAMES.len(), 3, after, &key), None);
        assert_eq!(page_end(NAMES.len(), 9, None, &key), None);
        assert_eq!(rank(NAMES.len(), 0, Some(&b"zebra"[..]), &key), None);
    }

    #[test]
    fn the_smallest_keys_are_kept_in_order_whatever_order_they_come_in() {
        let mut page = Smallest::<8, 4>::new(3);
        for key in [&b"pear"[..], b"apple", b"fig", b"kiwi", b"banana", b"date"] {
            page.offer(key);
        }
        let keys: Vec<&[u8]> = page.keys().collect();
        assert_eq!(keys, [&b"apple"[..], b"banana", b"date"]);
        assert!(page.more());
        assert_eq!(page.last(), Some(&b"date"[..]));
        assert!(page.holds(b"banana") && !page.holds(b"fig"));
        let mut all = Smallest::<8, 4>::new(3);
        all.offer(b"b");
        all.offer(b"a");
        assert!(!all.more());
        assert_eq!(all.keys().count(), 2);
        assert_eq!(all.last(), Some(&b"b"[..]));
        let mut long = Smallest::<2, 4>::new(3);
        long.offer(b"abc");
        assert!(long.more() && long.last().is_none());
        let mut ids = Smallest::<4, 3>::new(9);
        for id in [129u32, 3, 70, 1000] {
            ids.offer(&id.to_be_bytes());
        }
        let ids: Vec<&[u8]> = ids.keys().collect();
        assert_eq!(ids, [&3u32.to_be_bytes()[..], &70u32.to_be_bytes()[..]]);
    }
}
