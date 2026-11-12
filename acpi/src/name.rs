//! Names in the ACPI namespace (ACPI 6.5, 5.3 and 20.2.2): four-character segments, absolute
//! paths from the root, and names as AML writes them before they are resolved.

use alloc::vec::Vec;

/// One name segment: four characters, padded with `_`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NameSeg(pub [u8; 4]);

impl NameSeg {
    pub const fn new(name: &[u8; 4]) -> Self {
        Self(*name)
    }

    /// The segment `text` names, padded with `_` when shorter than four characters; `None` for
    /// an empty or longer one.
    pub fn from_text(text: &[u8]) -> Option<Self> {
        if text.is_empty() || text.len() > 4 {
            return None;
        }
        let mut name = [b'_'; 4];
        name[..text.len()].copy_from_slice(text);
        Some(Self(name))
    }

    pub fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }
}

/// An absolute path: the segments from the root, the root itself being empty.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Path(pub Vec<NameSeg>);

impl Path {
    pub const fn root() -> Self {
        Self(Vec::new())
    }

    /// The absolute path `text` writes (`\_SB.PCI0._PRT`, segments of up to four characters);
    /// `None` for a relative or malformed one.
    pub fn parse(text: &[u8]) -> Option<Self> {
        let rest = text.strip_prefix(b"\\")?;
        if rest.is_empty() {
            return Some(Self::root());
        }
        rest.split(|&byte| byte == b'.')
            .map(NameSeg::from_text)
            .collect::<Option<Vec<_>>>()
            .map(Self)
    }

    pub fn child(&self, segment: NameSeg) -> Self {
        let mut segments = self.0.clone();
        segments.push(segment);
        Self(segments)
    }

    pub fn parent(&self) -> Option<Self> {
        let (_, parent) = self.0.split_last()?;
        Some(Self(parent.to_vec()))
    }

    pub fn last(&self) -> Option<NameSeg> {
        self.0.last().copied()
    }

    pub fn depth(&self) -> usize {
        self.0.len()
    }

    /// Whether `self` lies below `ancestor`, or is it.
    pub fn starts_with(&self, ancestor: &Path) -> bool {
        self.0.starts_with(&ancestor.0)
    }

    /// Appends the path as ACPI writes it: `\`, then the segments joined by `.`.
    pub fn write(&self, out: &mut Vec<u8>) {
        out.push(b'\\');
        for (index, segment) in self.0.iter().enumerate() {
            if index > 0 {
                out.push(b'.');
            }
            out.extend_from_slice(segment.as_bytes());
        }
    }
}

/// A name as AML writes it (`NameString`), before it is resolved against a scope.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NameString {
    pub root: bool,
    pub parents: u8,
    pub segments: Vec<NameSeg>,
}

impl NameString {
    /// Whether it is the null name, which names nothing.
    pub fn is_null(&self) -> bool {
        !self.root && self.parents == 0 && self.segments.is_empty()
    }

    /// Whether the namespace's search rules apply to it: a single segment with no prefix.
    pub fn searches(&self) -> bool {
        !self.root && self.parents == 0 && self.segments.len() == 1
    }

    /// The path it names from `scope`, without searching; `None` when its parent prefixes climb
    /// past the root.
    pub fn resolve(&self, scope: &Path) -> Option<Path> {
        let mut segments = match self.root {
            true => Vec::new(),
            false => {
                let kept = scope.0.len().checked_sub(usize::from(self.parents))?;
                scope.0[..kept].to_vec()
            }
        };
        segments.extend_from_slice(&self.segments);
        Some(Path(segments))
    }
}
