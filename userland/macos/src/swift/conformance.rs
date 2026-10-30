//! Protocol witness tables a program asks the runtime for (D180): `swift_getWitnessTable`
//! returns a conformance's static table, or instantiates a generic or resilient one once per
//! conforming type: the pattern's words, the resilient witnesses placed in the slots of the
//! requirements they name, the protocol's default implementations in the slots left empty, the
//! base protocols' tables resolved through their accessors, then the conformance's instantiator.
//! Associated types and conformances stay mangled names (low bit set) until
//! `swift_getAssociatedTypeWitness` or `swift_getAssociatedConformanceWitness` first asks for
//! them, and are kept resolved in their slot.

use super::conformance_layout::{self as layout, Memory, requirement};
use super::mangling;
use super::metadata::{self, Metadata, context, kind, set_word, word, word32};
use super::type_lookup::{self, Response};
use crate::libsystem::{Exclusive, heap};
use crate::objc::buffer::Buffer;

struct Mapped;

impl Memory for Mapped {
    fn word32(&self, address: usize) -> u32 {
        word32(address)
    }

    fn word(&self, address: usize) -> usize {
        word(address)
    }
}

#[derive(Debug, Clone, Copy)]
struct Instantiated {
    conformance: usize,
    conforming: Metadata,
    table: usize,
}

static TABLES: Exclusive<Buffer<Instantiated>> = Exclusive::new(Buffer::new());

fn find(tables: &Buffer<Instantiated>, conformance: usize, conforming: Metadata) -> Option<usize> {
    tables
        .as_slice()
        .iter()
        .find(|found| found.conformance == conformance && found.conforming == conforming)
        .map(|found| found.table)
}

fn out_of_memory() -> ! {
    super::fatal_error(&[b"out of memory for a Swift witness table"])
}

extern "C" fn missing_witness() -> ! {
    super::unimplemented(&[b"a protocol requirement its conformance does not implement"])
}

type Instantiator = extern "C" fn(usize, Metadata, *const usize);
type ConformanceAccessor = extern "C" fn(Metadata, Metadata, usize) -> usize;

fn instantiate(
    conformance: &layout::Conformance,
    generic: layout::GenericTable,
    conforming: Metadata,
    arguments: *const usize,
) -> usize {
    let memory = Mapped;
    let protocol = layout::indirectable(&memory, conformance.protocol_field);
    let count = layout::requirement_count(&memory, protocol);
    let block = heap::allocate((generic.private_words + 1 + count) * 8, true);
    if block.is_null() {
        out_of_memory();
    }
    let table = block as usize + generic.private_words * 8;
    if let Some(pattern) = conformance.pattern {
        for index in 0..generic.pattern_words.min(1 + count) {
            set_word(table + 8 * index, word(pattern + 8 * index));
        }
    }
    set_word(table, conformance.address);
    for index in 0..conformance.witness_count {
        let (requirement_field, implementation_field) = conformance.witness_fields(index);
        let named = layout::indirectable(&memory, requirement_field);
        if let Some(slot) = layout::slot_of(&memory, protocol, named) {
            let implementation = layout::relative(&memory, implementation_field);
            set_word(table + 8 * slot, implementation);
        }
    }
    let start = layout::requirements_start(&memory, protocol);
    for index in 0..count {
        let descriptor = start + index * layout::REQUIREMENT_SIZE;
        let slot = table + 8 * (index + 1);
        if word(slot) == 0 {
            let fallback = missing_witness as *const () as usize;
            let default = layout::optional_relative(&memory, descriptor + 4).unwrap_or(fallback);
            set_word(slot, default);
        }
        if word32(descriptor) & requirement::KIND_MASK == requirement::BASE_PROTOCOL {
            conformance_in_slot(table, index + 1, conforming, conforming);
        }
    }
    if let Some(instantiator) = generic.instantiator {
        // SAFETY: a conformance's witness table instantiator: it finishes the new table for
        // the conforming type from the arguments the program passed.
        let instantiator: Instantiator = unsafe { core::mem::transmute(instantiator) };
        instantiator(table, conforming, arguments);
    }
    table
}

extern "C" fn get_witness_table(
    conformance: usize,
    conforming: Metadata,
    arguments: *const usize,
) -> usize {
    let parsed = layout::parse(&Mapped, conformance);
    let Some(generic) = parsed.generic else {
        return parsed
            .pattern
            .unwrap_or_else(|| super::fatal_error(&[b"a conformance without a witness table"]));
    };
    if let Some(found) = TABLES.with(|tables| find(tables, conformance, conforming)) {
        return found;
    }
    let made = instantiate(&parsed, generic, conforming, arguments);
    TABLES.with(|tables| {
        if let Some(found) = find(tables, conformance, conforming) {
            return found;
        }
        let entry = Instantiated {
            conformance,
            conforming,
            table: made,
        };
        if !tables.push(entry) {
            out_of_memory();
        }
        made
    })
}

fn byte(address: usize) -> u8 {
    // SAFETY: a mangled name the program emitted, at least as long as the bytes read.
    unsafe { (address as *const u8).read() }
}

fn conformance_in_slot(
    table: usize,
    slot: usize,
    associated: Metadata,
    conforming: Metadata,
) -> usize {
    let value = word(table + 8 * slot);
    let Some(name) = layout::unresolved_name(value) else {
        return value;
    };
    if byte(name) != layout::ACCESSOR_MARKER || byte(name + 1) != layout::ACCESSOR_REFERENCE {
        super::unimplemented(&[b"an associated conformance named by its mangling"]);
    }
    let accessor = layout::relative(&Mapped, name + 2);
    // SAFETY: an associated conformance accessor the program emitted, which takes the
    // associated type, the conforming type and the witness table, and returns a table.
    let accessor: ConformanceAccessor = unsafe { core::mem::transmute(accessor) };
    let found = accessor(associated, conforming, table);
    set_word(table + 8 * slot, found);
    found
}

fn generic_arguments(table: usize, conforming: Metadata) -> *const usize {
    let conformance = layout::parse(&Mapped, word(table));
    let Some(descriptor) = conformance.type_descriptor(&Mapped) else {
        return core::ptr::null();
    };
    if word32(descriptor) & context::GENERIC == 0 {
        return core::ptr::null();
    }
    if metadata::kind_of(conforming) == kind::CLASS {
        super::unimplemented(&[b"associated types of generic classes' conformances"]);
    }
    (conforming + 16) as *const usize
}

fn slot_from(base: usize, descriptor: usize) -> usize {
    layout::slot_from_base(base, descriptor)
        .unwrap_or_else(|| super::fatal_error(&[b"a requirement outside its protocol"]))
}

extern "C" fn get_associated_type_witness(
    _request: usize,
    table: usize,
    conforming: Metadata,
    base: usize,
    associated_type: usize,
) -> Response {
    let slot = slot_from(base, associated_type);
    let value = word(table + 8 * slot);
    let found = match layout::unresolved_name(value) {
        None => value,
        Some(name) => {
            let length = mangling::name_length(|at| byte(name + at));
            let arguments = generic_arguments(table, conforming);
            let found =
                type_lookup::get_type_by_mangled_name(name as *const u8, length, 0, arguments);
            set_word(table + 8 * slot, found);
            found
        }
    };
    Response {
        metadata: found,
        state: 0,
    }
}

extern "C" fn get_associated_conformance_witness(
    table: usize,
    conforming: Metadata,
    associated: Metadata,
    base: usize,
    associated_conformance: usize,
) -> usize {
    let slot = slot_from(base, associated_conformance);
    conformance_in_slot(table, slot, associated, conforming)
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_swift_getWitnessTable" => get_witness_table as *const () as usize,
        b"_swift_getAssociatedTypeWitness" => get_associated_type_witness as *const () as usize,
        b"_swift_getAssociatedConformanceWitness" => {
            get_associated_conformance_witness as *const () as usize
        }
        _ => return None,
    };
    Some(address as u64)
}
