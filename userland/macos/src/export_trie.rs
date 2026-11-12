//! A library's exported symbols (D409): the export trie of `LC_DYLD_EXPORTS_TRIE` (or of
//! `LC_DYLD_INFO`'s export range), a tree of nodes whose edges spell symbol names and whose
//! terminal nodes say where a symbol is: an offset from the image's header, an absolute
//! value, another library's symbol it re-exports, or a stub with a resolver.

const KIND_MASK: u64 = 0x03;
const KIND_THREAD_LOCAL: u64 = 0x01;
const KIND_ABSOLUTE: u64 = 0x02;
const REEXPORT: u64 = 0x08;
const STUB_AND_RESOLVER: u64 = 0x10;
const MOST_STEPS: usize = 512;

/// Where an exported symbol is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol<'a> {
    /// At this offset from the image's Mach-O header.
    Offset(u64),
    /// This value, wherever the image is.
    Absolute(u64),
    /// A thread-local variable's descriptor at this offset.
    ThreadLocal(u64),
    /// Another library's symbol: the 1-based ordinal of one of this image's libraries and the
    /// name there (empty for the same name).
    Reexport { ordinal: u64, name: &'a [u8] },
    /// A stub at this offset whose resolver, at the other, finds the real function.
    Resolver { stub: u64, resolver: u64 },
}

fn uleb(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(*at)?;
        *at += 1;
        if shift >= 64 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
}

fn c_string<'a>(bytes: &'a [u8], at: &mut usize) -> Option<&'a [u8]> {
    let rest = bytes.get(*at..)?;
    let length = rest.iter().position(|byte| *byte == 0)?;
    *at += length + 1;
    Some(&rest[..length])
}

fn terminal<'a>(trie: &'a [u8], at: usize) -> Option<Symbol<'a>> {
    let mut at = at;
    let flags = uleb(trie, &mut at)?;
    if flags & REEXPORT != 0 {
        let ordinal = uleb(trie, &mut at)?;
        let name = c_string(trie, &mut at)?;
        return Some(Symbol::Reexport { ordinal, name });
    }
    if flags & STUB_AND_RESOLVER != 0 {
        let stub = uleb(trie, &mut at)?;
        let resolver = uleb(trie, &mut at)?;
        return Some(Symbol::Resolver { stub, resolver });
    }
    let value = uleb(trie, &mut at)?;
    Some(match flags & KIND_MASK {
        KIND_ABSOLUTE => Symbol::Absolute(value),
        KIND_THREAD_LOCAL => Symbol::ThreadLocal(value),
        _ => Symbol::Offset(value),
    })
}

/// The symbol `name` exports in `trie`, if it exports it.
pub fn lookup<'a>(trie: &'a [u8], name: &[u8]) -> Option<Symbol<'a>> {
    let mut node = 0usize;
    let mut rest = name;
    for _ in 0..MOST_STEPS {
        let mut at = node;
        let terminal_size = uleb(trie, &mut at)? as usize;
        if rest.is_empty() {
            return match terminal_size {
                0 => None,
                _ => terminal(trie, at),
            };
        }
        at = at.checked_add(terminal_size)?;
        let children = *trie.get(at)?;
        at += 1;
        let mut next = None;
        for _ in 0..children {
            let label = c_string(trie, &mut at)?;
            let child = uleb(trie, &mut at)? as usize;
            if !label.is_empty() && rest.starts_with(label) {
                next = Some((child, label.len()));
                break;
            }
        }
        let (child, used) = next?;
        if child == node || child >= trie.len() {
            return None;
        }
        node = child;
        rest = &rest[used..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut trie = Vec::new();
        trie.extend_from_slice(&[0, 1]);
        trie.extend_from_slice(b"_\0");
        trie.push(5);
        trie.extend_from_slice(&[0, 2]);
        trie.extend_from_slice(b"foo\0");
        trie.push(17);
        trie.extend_from_slice(b"bar\0");
        trie.push(22);
        trie.extend_from_slice(&[3, 0, 0x80, 0x20, 0]);
        trie.extend_from_slice(&[7, 0x08, 2]);
        trie.extend_from_slice(b"_baz\0");
        trie.push(0);
        trie
    }

    #[test]
    fn a_symbol_is_found_by_its_edges() {
        let trie = sample();
        assert_eq!(trie[17], 3);
        assert_eq!(trie[22], 7);
        assert_eq!(lookup(&trie, b"_foo"), Some(Symbol::Offset(0x1000)));
        assert_eq!(
            lookup(&trie, b"_bar"),
            Some(Symbol::Reexport {
                ordinal: 2,
                name: b"_baz",
            })
        );
    }

    #[test]
    fn names_it_does_not_export_are_not_found() {
        let trie = sample();
        assert_eq!(lookup(&trie, b"_qux"), None);
        assert_eq!(lookup(&trie, b"_"), None);
        assert_eq!(lookup(&trie, b"_foobar"), None);
        assert_eq!(lookup(&[], b"_foo"), None);
        assert_eq!(lookup(&[0, 1, b'_', 0, 0], b"_x"), None);
    }
}
