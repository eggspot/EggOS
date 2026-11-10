//! Property lists (D196, D232): Apple's XML (`<plist>`) and binary (`bplist00`) formats, read in
//! place from the file's bytes without allocating. [`Document::parse`] checks what it can up
//! front (a binary list's trailer and offset table); everything else is checked as it is read,
//! so a damaged list gives `None` from the accessor that meets the damage, never a panic.
//! Shared by the installer (an `Info.plist`'s icon and program) and the macOS layer (NSBundle,
//! keyed archives and nibs).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod binary;
mod text;
mod xml;

#[cfg(test)]
mod tests;

pub use text::{Bytes, Chars, Data, Text};

/// Seconds from 1970-01-01 to 2001-01-01, the reference date property list dates count from.
pub const REFERENCE_DATE_UNIX_SECONDS: i64 = 978_307_200;

/// Which of the two formats a list is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Xml,
    Binary,
}

/// Why a list could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The bytes start as neither format.
    NotAPropertyList,
    /// The bytes start as a property list but its structure is broken.
    Damaged,
}

/// What an object is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Null,
    Boolean,
    Integer,
    Real,
    Date,
    Data,
    String,
    Uid,
    Array,
    Set,
    Dictionary,
}

#[derive(Debug, Clone, Copy)]
enum Source<'b> {
    Binary(binary::Table<'b>),
    Xml(&'b [u8]),
}

/// An opened property list.
#[derive(Debug, Clone, Copy)]
pub struct Document<'b> {
    source: Source<'b>,
    top: usize,
}

/// One object of a list: a value, or a collection whose members are objects.
#[derive(Debug, Clone, Copy)]
pub struct Object<'b> {
    source: Source<'b>,
    node: usize,
}

impl<'b> Document<'b> {
    /// Opens `bytes` as a binary list (it starts with `bplist00`) or an XML one.
    pub fn parse(bytes: &'b [u8]) -> Result<Self, Error> {
        if bytes.starts_with(binary::MAGIC) {
            let table = binary::Table::open(bytes).ok_or(Error::Damaged)?;
            return Ok(Self {
                top: table.top,
                source: Source::Binary(table),
            });
        }
        match xml::top(bytes) {
            Some(Ok(top)) => Ok(Self {
                source: Source::Xml(bytes),
                top,
            }),
            Some(Err(())) => Err(Error::Damaged),
            None => Err(Error::NotAPropertyList),
        }
    }

    pub fn format(&self) -> Format {
        match self.source {
            Source::Binary(_) => Format::Binary,
            Source::Xml(_) => Format::Xml,
        }
    }

    /// The top-level object.
    pub fn top(&self) -> Object<'b> {
        Object {
            source: self.source,
            node: self.top,
        }
    }

    /// The object numbered `index` in a binary list's object table; `None` in an XML list.
    pub fn object(&self, index: usize) -> Option<Object<'b>> {
        match self.source {
            Source::Binary(table) => (index < table.objects).then_some(Object {
                source: self.source,
                node: index,
            }),
            Source::Xml(_) => None,
        }
    }
}

impl<'b> Object<'b> {
    fn at(&self, node: usize) -> Self {
        Self {
            source: self.source,
            node,
        }
    }

    /// What the object is; `None` when it is damaged or of a kind lists cannot hold.
    pub fn kind(&self) -> Option<Kind> {
        match self.source {
            Source::Binary(table) => table.kind(self.node),
            Source::Xml(bytes) => xml::kind(bytes, self.node),
        }
    }

    /// The number a binary list's object table gives this object, which keyed archives'
    /// references name; `None` in an XML list.
    pub fn index(&self) -> Option<usize> {
        matches!(self.source, Source::Binary(_)).then_some(self.node)
    }

    pub fn boolean(&self) -> Option<bool> {
        match self.source {
            Source::Binary(table) => table.boolean(self.node),
            Source::Xml(bytes) => xml::boolean(bytes, self.node),
        }
    }

    /// An integer's value (binary lists hold up to 128 bits).
    pub fn integer(&self) -> Option<i128> {
        match self.source {
            Source::Binary(table) => table.integer(self.node),
            Source::Xml(bytes) => xml::integer(bytes, self.node),
        }
    }

    pub fn real(&self) -> Option<f64> {
        match self.source {
            Source::Binary(table) => table.real(self.node),
            Source::Xml(bytes) => xml::real(bytes, self.node),
        }
    }

    /// A date, as seconds since 2001-01-01 00:00:00 UTC.
    pub fn date(&self) -> Option<f64> {
        match self.source {
            Source::Binary(table) => table.date(self.node),
            Source::Xml(bytes) => xml::date(bytes, self.node),
        }
    }

    /// A keyed archive's reference: a binary list's UID, or an XML list's one-entry dictionary
    /// `{CF$UID = n}`.
    pub fn uid(&self) -> Option<u64> {
        match self.source {
            Source::Binary(table) => table.uid(self.node),
            Source::Xml(_) => {
                let mut entries = self.entries()?;
                let (key, value) = entries.next()?;
                if entries.next().is_some() || !key.text()?.equals(b"CF$UID") {
                    return None;
                }
                u64::try_from(value.integer()?).ok()
            }
        }
    }

    /// A string's text (an XML `<key>` is one too).
    pub fn text(&self) -> Option<Text<'b>> {
        match self.source {
            Source::Binary(table) => table.text(self.node),
            Source::Xml(bytes) => xml::text(bytes, self.node),
        }
    }

    pub fn data(&self) -> Option<Data<'b>> {
        match self.source {
            Source::Binary(table) => table.data(self.node),
            Source::Xml(bytes) => xml::data(bytes, self.node),
        }
    }

    /// The members of an array or a set, in order.
    pub fn items(&self) -> Option<Items<'b>> {
        let inner = match self.source {
            Source::Binary(table) => {
                let (refs, count) = table.members(self.node)?;
                ItemsInner::Binary {
                    table,
                    refs,
                    count,
                    index: 0,
                }
            }
            Source::Xml(bytes) => {
                if !matches!(xml::kind(bytes, self.node)?, Kind::Array) {
                    return None;
                }
                ItemsInner::Xml(xml::Children::of(bytes, self.node)?)
            }
        };
        Some(Items { inner })
    }

    /// A dictionary's keys and values, in the order the list holds them.
    pub fn entries(&self) -> Option<Entries<'b>> {
        let inner = match self.source {
            Source::Binary(table) => {
                let (keys, count) = table.entries(self.node)?;
                EntriesInner::Binary {
                    table,
                    keys,
                    count,
                    index: 0,
                }
            }
            Source::Xml(bytes) => {
                if !matches!(xml::kind(bytes, self.node)?, Kind::Dictionary) {
                    return None;
                }
                EntriesInner::Xml(xml::Children::of(bytes, self.node)?)
            }
        };
        Some(Entries { inner })
    }

    /// How many members an array, a set or a dictionary has.
    pub fn count(&self) -> Option<usize> {
        match self.source {
            Source::Binary(table) => table
                .members(self.node)
                .or_else(|| table.entries(self.node))
                .map(|(_, count)| count),
            Source::Xml(_) => match self.kind()? {
                Kind::Array => Some(self.items()?.count()),
                Kind::Dictionary => Some(self.entries()?.count()),
                _ => None,
            },
        }
    }

    /// The `index`th member of an array or a set.
    pub fn item(&self, index: usize) -> Option<Object<'b>> {
        match self.source {
            Source::Binary(table) => {
                let (refs, count) = table.members(self.node)?;
                if index >= count {
                    return None;
                }
                Some(self.at(table.reference_at(refs, index)?))
            }
            Source::Xml(_) => self.items()?.nth(index),
        }
    }

    /// The value a dictionary holds under the key whose text is `key` (UTF-8).
    pub fn get(&self, key: &[u8]) -> Option<Object<'b>> {
        self.entries()?.find_map(|(name, value)| {
            name.text()
                .is_some_and(|text| text.equals(key))
                .then_some(value)
        })
    }
}

#[derive(Debug, Clone)]
enum ItemsInner<'b> {
    Binary {
        table: binary::Table<'b>,
        refs: usize,
        count: usize,
        index: usize,
    },
    Xml(xml::Children<'b>),
}

/// The members of an array or a set; it ends early at damage.
#[derive(Debug, Clone)]
pub struct Items<'b> {
    inner: ItemsInner<'b>,
}

impl<'b> Iterator for Items<'b> {
    type Item = Object<'b>;

    fn next(&mut self) -> Option<Object<'b>> {
        match &mut self.inner {
            ItemsInner::Binary {
                table,
                refs,
                count,
                index,
            } => {
                if *index >= *count {
                    return None;
                }
                let node = table.reference_at(*refs, *index)?;
                *index += 1;
                Some(Object {
                    source: Source::Binary(*table),
                    node,
                })
            }
            ItemsInner::Xml(children) => {
                let bytes = children.bytes();
                children.next().map(|node| Object {
                    source: Source::Xml(bytes),
                    node,
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
enum EntriesInner<'b> {
    Binary {
        table: binary::Table<'b>,
        keys: usize,
        count: usize,
        index: usize,
    },
    Xml(xml::Children<'b>),
}

/// A dictionary's keys and values; it ends early at damage.
#[derive(Debug, Clone)]
pub struct Entries<'b> {
    inner: EntriesInner<'b>,
}

impl<'b> Iterator for Entries<'b> {
    type Item = (Object<'b>, Object<'b>);

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            EntriesInner::Binary {
                table,
                keys,
                count,
                index,
            } => {
                if *index >= *count {
                    return None;
                }
                let key = table.reference_at(*keys, *index)?;
                let value = table.reference_at(*keys, count.checked_add(*index)?)?;
                *index += 1;
                let source = Source::Binary(*table);
                Some((
                    Object { source, node: key },
                    Object {
                        source,
                        node: value,
                    },
                ))
            }
            EntriesInner::Xml(children) => {
                let bytes = children.bytes();
                let key = children.next()?;
                if xml::tag_name(bytes, key)? != b"key" {
                    return None;
                }
                let value = children.next()?;
                let source = Source::Xml(bytes);
                Some((
                    Object { source, node: key },
                    Object {
                        source,
                        node: value,
                    },
                ))
            }
        }
    }
}
