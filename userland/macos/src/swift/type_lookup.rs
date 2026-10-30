//! `swift_getTypeByMangledNameInContext` (D180): a program asks for the metadata of a type it
//! names by a mangled name (`mangling.rs`), usually through a symbolic reference to a context
//! descriptor bound to generic arguments: the runtime's own descriptors and the standard
//! library's substitutions give its own metadata, a program's non-generic type its metadata
//! accessor's answer. Also `Optional`'s metadata accessor.

use super::generic;
use super::mangling::{self, Generic, Item, Resolver, Standard};
use super::metadata::{self, Metadata, context, relative, word, word32};
use super::storage;

/// What a metadata accessor returns: the metadata and its state.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct Response {
    pub metadata: Metadata,
    pub state: usize,
}

struct Lookup {
    arguments: *const usize,
}

fn builtin_generic(descriptor: usize) -> Option<Generic> {
    let ours = [
        (&metadata::OPTIONAL_DESCRIPTOR, Standard::Optional),
        (&metadata::ARRAY_DESCRIPTOR, Standard::Array),
        (&metadata::DICTIONARY_DESCRIPTOR, Standard::Dictionary),
        (&metadata::SET_DESCRIPTOR, Standard::Set),
    ];
    if let Some((_, standard)) = ours.iter().find(|(found, _)| found.address() == descriptor) {
        return Some(Generic::Standard(*standard));
    }
    let storages = [
        metadata::CONTIGUOUS_ARRAY_STORAGE_DESCRIPTOR.address(),
        metadata::DICTIONARY_STORAGE_DESCRIPTOR.address(),
    ];
    storages
        .contains(&descriptor)
        .then_some(Generic::Descriptor(descriptor))
}

fn call_accessor(descriptor: usize) -> Option<Metadata> {
    let accessor = relative(descriptor + 12);
    if accessor == descriptor + 12 {
        return None;
    }
    // SAFETY: a type descriptor's access function: Swift's metadata accessor, which takes the
    // request and returns the metadata and its state.
    let function: extern "C" fn(usize) -> Response = unsafe { core::mem::transmute(accessor) };
    let found = function(0).metadata;
    (found != 0).then_some(found)
}

fn context_item(descriptor: usize) -> Option<Item<Metadata>> {
    if let Some(generic) = builtin_generic(descriptor) {
        return Some(Item::Generic(generic));
    }
    if let Some(found) = metadata::standard_struct(descriptor) {
        return Some(Item::Type(found));
    }
    let flags = word32(descriptor);
    match flags & context::KIND_MASK {
        context::PROTOCOL => Some(Item::Protocol(descriptor)),
        context::CLASS | context::STRUCT | context::ENUM => {
            if flags & context::GENERIC != 0 {
                Some(Item::Generic(Generic::Descriptor(descriptor)))
            } else {
                call_accessor(descriptor).map(Item::Type)
            }
        }
        _ => None,
    }
}

impl Resolver for Lookup {
    type Type = Metadata;

    fn symbolic(&mut self, kind: u8, address: usize) -> Option<Item<Metadata>> {
        let descriptor = match kind {
            1 => address,
            2 => word(address),
            _ => return None,
        };
        context_item(descriptor)
    }

    fn standard(&mut self, standard: Standard) -> Option<Metadata> {
        metadata::standard(standard)
    }

    fn swift_type(&mut self, name: &[u8], kind: u8) -> Option<Metadata> {
        metadata::swift_type(name, kind)
    }

    fn bound(&mut self, generic: Generic, arguments: &[Metadata]) -> Option<Metadata> {
        match (generic, arguments) {
            (Generic::Standard(Standard::Optional), &[wrapped]) => generic::optional(wrapped),
            (Generic::Standard(Standard::Array), &[_]) => {
                generic::collection(&metadata::ARRAY_DESCRIPTOR, arguments)
            }
            (Generic::Standard(Standard::Set), &[_]) => {
                generic::collection(&metadata::SET_DESCRIPTOR, arguments)
            }
            (Generic::Standard(Standard::Dictionary), &[_, _]) => {
                generic::collection(&metadata::DICTIONARY_DESCRIPTOR, arguments)
            }
            (Generic::Descriptor(descriptor), &[element])
                if descriptor == metadata::CONTIGUOUS_ARRAY_STORAGE_DESCRIPTOR.address() =>
            {
                storage::contiguous_array_storage(element)
            }
            (Generic::Descriptor(descriptor), &[key, value])
                if descriptor == metadata::DICTIONARY_STORAGE_DESCRIPTOR.address() =>
            {
                storage::dictionary_storage(key, value)
            }
            _ => None,
        }
    }

    fn existential(&mut self, protocols: &[usize]) -> Option<Metadata> {
        let class_bound = protocols
            .iter()
            .any(|&protocol| word32(protocol) & (1 << 16) == 0);
        if class_bound {
            return None;
        }
        generic::existential(protocols)
    }

    fn metatype(&mut self, _instance: Metadata) -> Option<Metadata> {
        None
    }

    fn empty_tuple(&mut self) -> Option<Metadata> {
        Some(metadata::address_of(&metadata::EMPTY_TUPLE))
    }

    fn generic_parameter(&mut self, depth: usize, index: usize) -> Option<Metadata> {
        if depth != 0 || self.arguments.is_null() {
            return None;
        }
        Some(word(self.arguments as usize + 8 * index))
    }
}

fn report(name: &[u8]) -> ! {
    let mut printable = [0u8; 96];
    let mut length = 0;
    for &byte in name.iter().take(printable.len()) {
        printable[length] = if byte.is_ascii_graphic() { byte } else { b'?' };
        length += 1;
    }
    super::unimplemented(&[b"the type named ", &printable[..length]])
}

extern "C" fn get_type_by_mangled_name(
    name: *const u8,
    length: usize,
    _context: usize,
    arguments: *const usize,
) -> Metadata {
    // SAFETY: the program's mangled name of `length` bytes.
    let bytes = unsafe { core::slice::from_raw_parts(name, length) };
    let mut lookup = Lookup { arguments };
    mangling::parse(bytes, name as usize, &mut lookup).unwrap_or_else(|| report(bytes))
}

extern "C" fn get_type_by_mangled_name_in_state(
    _state: usize,
    name: *const u8,
    length: usize,
    context: usize,
    arguments: *const usize,
) -> Metadata {
    get_type_by_mangled_name(name, length, context, arguments)
}

extern "C" fn optional_accessor(_request: usize, wrapped: Metadata) -> Response {
    let found = generic::optional(wrapped)
        .unwrap_or_else(|| super::fatal_error(&[b"out of memory for Swift metadata"]));
    Response {
        metadata: found,
        state: 0,
    }
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_swift_getTypeByMangledNameInContext" => get_type_by_mangled_name as *const () as usize,
        b"_swift_getTypeByMangledNameInContextInMetadataState" => {
            get_type_by_mangled_name_in_state as *const () as usize
        }
        b"_$sSqMa" => optional_accessor as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}
