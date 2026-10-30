//! The mangled type names a Swift program asks the runtime to turn into metadata (D180), from
//! the public mangling grammar: symbolic references to context descriptors (relative 32-bit
//! offsets, direct or through a pointer), the standard library's `S` substitutions,
//! `s<length><name><kind>` types of the Swift module, bound generic arguments `y ... G`, `Sg`
//! optionals, `p` existentials of a protocol list (`yp` is `Any`), `x`/`q_` generic
//! parameters, `yt` and `Xp`. The parser is a stack machine over the postfix grammar; what each
//! name means is the [`Resolver`]'s to say. Pure logic with host tests.

/// A standard library type with a one-letter substitution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standard {
    Array,
    Bool,
    Character,
    Dictionary,
    Double,
    Float,
    Int,
    Optional,
    Set,
    String,
    Substring,
    UInt,
    UnicodeScalar,
    UnsafeMutableRawPointer,
    UnsafeRawPointer,
}

impl Standard {
    fn from_letter(letter: u8) -> Option<Self> {
        Some(match letter {
            b'a' => Self::Array,
            b'b' => Self::Bool,
            b'J' => Self::Character,
            b'D' => Self::Dictionary,
            b'd' => Self::Double,
            b'f' => Self::Float,
            b'i' => Self::Int,
            b'q' => Self::Optional,
            b'h' => Self::Set,
            b'S' => Self::String,
            b's' => Self::Substring,
            b'u' => Self::UInt,
            b'c' => Self::UnicodeScalar,
            b'v' => Self::UnsafeMutableRawPointer,
            b'V' => Self::UnsafeRawPointer,
            _ => return None,
        })
    }

    /// Whether the type takes generic arguments.
    pub fn is_generic(self) -> bool {
        matches!(
            self,
            Self::Array | Self::Dictionary | Self::Optional | Self::Set
        )
    }
}

/// A generic type before its arguments are bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generic {
    Descriptor(usize),
    Standard(Standard),
}

/// What a symbolic reference or name stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item<T> {
    Type(T),
    Generic(Generic),
    Protocol(usize),
}

/// The meaning of names, given by the runtime (or a test).
pub trait Resolver {
    type Type: Copy;

    /// The context descriptor at `address`: `kind` 1 is a direct reference, 2 a reference to a
    /// pointer holding the descriptor's address.
    fn symbolic(&mut self, kind: u8, address: usize) -> Option<Item<Self::Type>>;
    /// A non-generic standard library type.
    fn standard(&mut self, standard: Standard) -> Option<Self::Type>;
    /// A type of the Swift module by its name and kind letter (`V`, `C` or `O`).
    fn swift_type(&mut self, name: &[u8], kind: u8) -> Option<Self::Type>;
    fn bound(&mut self, generic: Generic, arguments: &[Self::Type]) -> Option<Self::Type>;
    /// The existential of `protocols` (none: `Any`).
    fn existential(&mut self, protocols: &[usize]) -> Option<Self::Type>;
    fn metatype(&mut self, instance: Self::Type) -> Option<Self::Type>;
    fn empty_tuple(&mut self) -> Option<Self::Type>;
    /// The generic parameter `index` at `depth` of the name's context.
    fn generic_parameter(&mut self, depth: usize, index: usize) -> Option<Self::Type>;
}

const STACK: usize = 24;

#[derive(Debug, Clone, Copy)]
enum Entry<T> {
    Item(Item<T>),
    Arguments,
}

struct Stack<T: Copy> {
    entries: [Option<Entry<T>>; STACK],
    length: usize,
}

impl<T: Copy> Stack<T> {
    fn push(&mut self, entry: Entry<T>) -> Option<()> {
        let slot = self.entries.get_mut(self.length)?;
        *slot = Some(entry);
        self.length += 1;
        Some(())
    }

    fn pop(&mut self) -> Option<Entry<T>> {
        self.length = self.length.checked_sub(1)?;
        self.entries[self.length].take()
    }

    fn top(&self) -> Option<Entry<T>> {
        self.entries[..self.length].last().copied().flatten()
    }

    fn pop_type(&mut self) -> Option<T> {
        match self.pop()? {
            Entry::Item(Item::Type(found)) => Some(found),
            _ => None,
        }
    }
}

fn relative(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn number(bytes: &[u8], at: &mut usize) -> Option<usize> {
    let start = *at;
    while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
        *at += 1;
    }
    if *at == start {
        return None;
    }
    core::str::from_utf8(&bytes[start..*at]).ok()?.parse().ok()
}

fn parameter_index(bytes: &[u8], at: &mut usize) -> Option<usize> {
    if bytes.get(*at) == Some(&b'_') {
        *at += 1;
        return Some(0);
    }
    let value = number(bytes, at)?;
    (bytes.get(*at) == Some(&b'_')).then_some(())?;
    *at += 1;
    Some(value + 1)
}

/// The type `bytes` names, the mangled name at `base` in memory (symbolic references are
/// relative to their own address); `None` if the name is malformed or uses grammar the parser
/// lacks, or the resolver refuses a part.
pub fn parse<R: Resolver>(bytes: &[u8], base: usize, resolver: &mut R) -> Option<R::Type> {
    let mut stack = Stack::<R::Type> {
        entries: [None; STACK],
        length: 0,
    };
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        at += 1;
        match byte {
            0 => break,
            0x01..=0x17 => {
                let offset = relative(bytes, at)?;
                let target = base.wrapping_add(at).wrapping_add_signed(offset as isize);
                at += 4;
                stack.push(Entry::Item(resolver.symbolic(byte, target)?))?;
            }
            0x18..=0x1f => return None,
            b'S' => {
                let letter = *bytes.get(at)?;
                at += 1;
                if letter == b'g' {
                    let wrapped = stack.pop_type()?;
                    let optional = Generic::Standard(Standard::Optional);
                    stack.push(Entry::Item(Item::Type(
                        resolver.bound(optional, &[wrapped])?,
                    )))?;
                    continue;
                }
                let standard = Standard::from_letter(letter)?;
                let item = if standard.is_generic() {
                    Item::Generic(Generic::Standard(standard))
                } else {
                    Item::Type(resolver.standard(standard)?)
                };
                stack.push(Entry::Item(item))?;
            }
            b's' => {
                let length = number(bytes, &mut at)?;
                let name = bytes.get(at..at + length)?;
                at += length;
                let kind = *bytes.get(at)?;
                at += 1;
                stack.push(Entry::Item(Item::Type(resolver.swift_type(name, kind)?)))?;
            }
            b'y' => match bytes.get(at) {
                Some(b'p') => {
                    at += 1;
                    stack.push(Entry::Item(Item::Type(resolver.existential(&[])?)))?;
                }
                Some(b't') => {
                    at += 1;
                    stack.push(Entry::Item(Item::Type(resolver.empty_tuple()?)))?;
                }
                _ => stack.push(Entry::Arguments)?,
            },
            b'_' => {}
            b'p' => {
                let mut protocols = [0usize; 4];
                let mut count = 0;
                while let Some(Entry::Item(Item::Protocol(protocol))) = stack.top() {
                    *protocols.get_mut(count)? = protocol;
                    count += 1;
                    stack.pop()?;
                }
                if count == 0 {
                    return None;
                }
                protocols[..count].reverse();
                let existential = resolver.existential(&protocols[..count])?;
                stack.push(Entry::Item(Item::Type(existential)))?;
            }
            b'G' => {
                let mut popped = [None; STACK];
                let mut count = 0;
                loop {
                    match stack.pop()? {
                        Entry::Arguments => break,
                        Entry::Item(Item::Type(argument)) => {
                            *popped.get_mut(count)? = Some(argument);
                            count += 1;
                        }
                        Entry::Item(_) => return None,
                    }
                }
                let Entry::Item(Item::Generic(generic)) = stack.pop()? else {
                    return None;
                };
                let first = popped[..count].last().copied().flatten()?;
                let mut list = [first; STACK];
                for (slot, argument) in list.iter_mut().zip(popped[..count].iter().rev()) {
                    *slot = (*argument)?;
                }
                let bound = resolver.bound(generic, &list[..count])?;
                stack.push(Entry::Item(Item::Type(bound)))?;
            }
            b'x' => {
                stack.push(Entry::Item(Item::Type(resolver.generic_parameter(0, 0)?)))?;
            }
            b'q' => {
                let (depth, index) = if bytes.get(at) == Some(&b'd') {
                    at += 1;
                    let depth = parameter_index(bytes, &mut at)? + 1;
                    (depth, parameter_index(bytes, &mut at)?)
                } else {
                    (0, parameter_index(bytes, &mut at)? + 1)
                };
                let parameter = resolver.generic_parameter(depth, index)?;
                stack.push(Entry::Item(Item::Type(parameter)))?;
            }
            b'X' => {
                if *bytes.get(at)? != b'p' {
                    return None;
                }
                at += 1;
                let instance = stack.pop_type()?;
                stack.push(Entry::Item(Item::Type(resolver.metatype(instance)?)))?;
            }
            _ => return None,
        }
    }
    let result = stack.pop_type()?;
    (stack.length == 0).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Names {
        made: Vec<String>,
        protocols: Vec<usize>,
    }

    impl Names {
        fn new(protocols: &[usize]) -> Self {
            Self {
                made: Vec::new(),
                protocols: protocols.to_vec(),
            }
        }

        fn make(&mut self, name: String) -> Option<usize> {
            self.made.push(name);
            Some(self.made.len() - 1)
        }
    }

    impl Resolver for Names {
        type Type = usize;

        fn symbolic(&mut self, kind: u8, address: usize) -> Option<Item<usize>> {
            if self.protocols.contains(&address) {
                return Some(Item::Protocol(address));
            }
            if kind == 2 {
                return Some(Item::Generic(Generic::Descriptor(address)));
            }
            self.make(format!("type@{address:x}")).map(Item::Type)
        }

        fn standard(&mut self, standard: Standard) -> Option<usize> {
            self.make(format!("{standard:?}"))
        }

        fn swift_type(&mut self, name: &[u8], kind: u8) -> Option<usize> {
            self.make(format!(
                "Swift.{}{}",
                String::from_utf8_lossy(name),
                kind as char
            ))
        }

        fn bound(&mut self, generic: Generic, arguments: &[usize]) -> Option<usize> {
            let names: Vec<&str> = arguments.iter().map(|&a| self.made[a].as_str()).collect();
            let base = match generic {
                Generic::Descriptor(address) => format!("generic@{address:x}"),
                Generic::Standard(standard) => format!("{standard:?}"),
            };
            let name = format!("{base}<{}>", names.join(", "));
            self.make(name)
        }

        fn existential(&mut self, protocols: &[usize]) -> Option<usize> {
            let names: Vec<String> = protocols.iter().map(|p| format!("{p:x}")).collect();
            self.make(format!("any({})", names.join("&")))
        }

        fn metatype(&mut self, instance: usize) -> Option<usize> {
            let name = format!("{}.Type", self.made[instance]);
            self.make(name)
        }

        fn empty_tuple(&mut self) -> Option<usize> {
            self.make("()".to_owned())
        }

        fn generic_parameter(&mut self, depth: usize, index: usize) -> Option<usize> {
            self.make(format!("param{depth}.{index}"))
        }
    }

    fn named(bytes: &[u8], base: usize, protocols: &[usize]) -> Option<String> {
        let mut names = Names::new(protocols);
        let found = parse(bytes, base, &mut names)?;
        Some(names.made[found].clone())
    }

    #[test]
    fn standard_types_and_their_generic_forms() {
        assert_eq!(named(b"Si", 0, &[]).as_deref(), Some("Int"));
        assert_eq!(named(b"SaySSG", 0, &[]).as_deref(), Some("Array<String>"));
        assert_eq!(
            named(b"SDySSSdG", 0, &[]).as_deref(),
            Some("Dictionary<String, Double>")
        );
        assert_eq!(named(b"SiSg", 0, &[]).as_deref(), Some("Optional<Int>"));
        assert_eq!(named(b"s5UInt8V", 0, &[]).as_deref(), Some("Swift.UInt8V"));
        assert_eq!(named(b"yp", 0, &[]).as_deref(), Some("any()"));
        assert_eq!(named(b"ypXp", 0, &[]).as_deref(), Some("any().Type"));
        assert_eq!(named(b"yt", 0, &[]).as_deref(), Some("()"));
        assert_eq!(
            named(b"SaySiGSg", 0, &[]).as_deref(),
            Some("Optional<Array<Int>>")
        );
    }

    #[test]
    fn symbolic_references_are_relative_to_their_offset_field() {
        let mut bytes = vec![0x02];
        bytes.extend_from_slice(&0x2085i32.to_le_bytes());
        bytes.push(b'y');
        bytes.push(0x01);
        bytes.extend_from_slice(&(-0x10i32).to_le_bytes());
        bytes.push(b'G');
        bytes.push(0);
        assert_eq!(
            named(&bytes, 0x1000, &[]).as_deref(),
            Some("generic@3086<type@ff7>")
        );
    }

    #[test]
    fn protocol_lists_make_existentials() {
        let mut bytes = vec![0x01];
        bytes.extend_from_slice(&0x100i32.to_le_bytes());
        bytes.extend_from_slice(b"_p");
        assert_eq!(named(&bytes, 0, &[0x101]).as_deref(), Some("any(101)"));
        let mut array = b"Say".to_vec();
        array.extend_from_slice(&bytes);
        array.push(b'G');
        assert_eq!(
            named(&array, 0, &[0x104]).as_deref(),
            Some("Array<any(104)>")
        );
    }

    #[test]
    fn generic_parameters_by_depth_and_index() {
        assert_eq!(named(b"x", 0, &[]).as_deref(), Some("param0.0"));
        assert_eq!(named(b"q_", 0, &[]).as_deref(), Some("param0.1"));
        assert_eq!(named(b"q0_", 0, &[]).as_deref(), Some("param0.2"));
        assert_eq!(named(b"qd__", 0, &[]).as_deref(), Some("param1.0"));
        assert_eq!(
            named(b"SDyxq_G", 0, &[]).as_deref(),
            Some("Dictionary<param0.0, param0.1>")
        );
    }

    #[test]
    fn malformed_names_are_refused() {
        assert_eq!(named(b"", 0, &[]), None);
        assert_eq!(named(b"SiSi", 0, &[]), None);
        assert_eq!(named(b"SaySS", 0, &[]), None);
        assert_eq!(named(b"G", 0, &[]), None);
        assert_eq!(named(b"Sz", 0, &[]), None);
        assert_eq!(named(b"s99X", 0, &[]), None);
    }
}
