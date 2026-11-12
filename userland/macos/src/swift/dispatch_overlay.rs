//! The Swift overlays' entry points a Swift app calls beside `libswiftCore` (D411), as read
//! from a real app's imports (Minesweeper 1.0.0, D389): `libswiftDispatch`'s `DispatchQoS` and
//! `DispatchWorkItemFlags` (their metadata, `DispatchQoS.unspecified`, and the flags' `SetAlgebra`
//! conformance, which generic code builds `[]` through), `DispatchQueue.main` and
//! `async(group:qos:flags:execute:)` over the layer's libdispatch (D388);
//! `libswiftCoreGraphics`' `CGContext.move(to:)`, `addLine(to:)` and
//! `addArc(center:radius:startAngle:endAngle:clockwise:)` over the layer's CoreGraphics (D386);
//! `libswiftCore`'s `SetAlgebra` with its two initializers' dispatch thunks and
//! `_CocoaArrayWrapper`'s `endIndex` and subscript; and every overlay's `_swift_FORCE_LOAD_$`
//! marker. `self` and an indirect result reach Rust through the entries of the Swift calling
//! convention (`entry.rs`'s rule: x20 or r13 for `self`, x8 or rax for the result).

use super::conformance_layout::requirement::{
    ASSOCIATED_TYPE, BASE_PROTOCOL, GETTER, INIT, INSTANCE, METHOD,
};
use super::metadata::{self, Metadata, address_of, word};
use super::protocols::{ConformanceDescriptor, ProtocolDescriptor, requirement};
use super::standard_protocols::{EQUATABLE, EquatableTable};
use crate::objc::abi::Id;
use crate::objc::{send, send1, sel};

const QOS_UNSPECIFIED: usize = 5;
const SET_ALGEBRA_WITNESSES: usize = 18;

macro_rules! entry {
    ($name:literal, $arm:literal, $x86:literal, $target:path) => {
        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 2",
            concat!(".globl ", $name),
            concat!(".hidden ", $name),
            concat!($name, ":"),
            $arm,
            "b {target}",
            target = sym $target,
        );
        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 4",
            concat!(".globl ", $name),
            concat!(".hidden ", $name),
            concat!($name, ":"),
            $x86,
            "jmp {target}",
            target = sym $target,
        );
    };
}

macro_rules! dispatch {
    ($name:literal, $arm:literal, $x86:literal) => {
        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 2",
            concat!(".globl ", $name),
            concat!(".hidden ", $name),
            concat!($name, ":"),
            $arm,
            "br x9",
        );
        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 4",
            concat!(".globl ", $name),
            concat!(".hidden ", $name),
            concat!($name, ":"),
            $x86,
        );
    };
}

entry!(
    "eggos_swift_queue_async",
    "mov x5, x20",
    "mov r9, r13",
    queue_async
);
entry!(
    "eggos_swift_context_move",
    "mov x0, x20",
    "mov rdi, r13",
    context_move
);
entry!(
    "eggos_swift_context_line",
    "mov x0, x20",
    "mov rdi, r13",
    context_line
);
entry!(
    "eggos_swift_context_arc",
    "mov x1, x0\nmov x0, x20",
    "mov rsi, rdi\nmov rdi, r13",
    context_arc
);
entry!(
    "eggos_swift_flags_empty",
    "mov x0, x8",
    "mov rdi, rax",
    flags_empty
);
entry!(
    "eggos_swift_flags_from_sequence",
    "mov x5, x8",
    "mov r9, rax",
    flags_from_sequence
);
entry!(
    "eggos_swift_flags_contains",
    "mov x1, x20",
    "mov rsi, r13",
    flags_contains
);
entry!(
    "eggos_swift_flags_union",
    "mov x1, x20\nmov x2, x8",
    "mov rsi, r13\nmov rdx, rax",
    flags_union
);
entry!(
    "eggos_swift_flags_intersection",
    "mov x1, x20\nmov x2, x8",
    "mov rsi, r13\nmov rdx, rax",
    flags_intersection
);
entry!(
    "eggos_swift_flags_symmetric_difference",
    "mov x1, x20\nmov x2, x8",
    "mov rsi, r13\nmov rdx, rax",
    flags_symmetric_difference
);
entry!(
    "eggos_swift_flags_subtracting",
    "mov x1, x20\nmov x2, x8",
    "mov rsi, r13\nmov rdx, rax",
    flags_subtracting
);
entry!(
    "eggos_swift_flags_form_union",
    "mov x1, x20",
    "mov rsi, r13",
    flags_form_union
);
entry!(
    "eggos_swift_flags_form_intersection",
    "mov x1, x20",
    "mov rsi, r13",
    flags_form_intersection
);
entry!(
    "eggos_swift_flags_form_symmetric_difference",
    "mov x1, x20",
    "mov rsi, r13",
    flags_form_symmetric_difference
);
entry!(
    "eggos_swift_flags_subtract",
    "mov x1, x20",
    "mov rsi, r13",
    flags_subtract
);
entry!(
    "eggos_swift_flags_is_subset",
    "mov x1, x20",
    "mov rsi, r13",
    flags_is_subset
);
entry!(
    "eggos_swift_flags_is_disjoint",
    "mov x1, x20",
    "mov rsi, r13",
    flags_is_disjoint
);
entry!(
    "eggos_swift_flags_is_superset",
    "mov x1, x20",
    "mov rsi, r13",
    flags_is_superset
);
entry!(
    "eggos_swift_flags_is_empty",
    "mov x0, x20",
    "mov rdi, r13",
    flags_is_empty
);
dispatch!(
    "eggos_swift_set_algebra_init_dispatch",
    "ldr x9, [x1, #32]",
    "jmp qword ptr [rsi + 32]"
);
dispatch!(
    "eggos_swift_set_algebra_init_sequence_dispatch",
    "ldr x9, [x4, #160]",
    "jmp qword ptr [r8 + 160]"
);

unsafe extern "C" {
    fn eggos_swift_queue_async();
    fn eggos_swift_context_move();
    fn eggos_swift_context_line();
    fn eggos_swift_context_arc();
    fn eggos_swift_flags_empty();
    fn eggos_swift_flags_from_sequence();
    fn eggos_swift_flags_contains();
    fn eggos_swift_flags_union();
    fn eggos_swift_flags_intersection();
    fn eggos_swift_flags_symmetric_difference();
    fn eggos_swift_flags_subtracting();
    fn eggos_swift_flags_form_union();
    fn eggos_swift_flags_form_intersection();
    fn eggos_swift_flags_form_symmetric_difference();
    fn eggos_swift_flags_subtract();
    fn eggos_swift_flags_is_subset();
    fn eggos_swift_flags_is_disjoint();
    fn eggos_swift_flags_is_superset();
    fn eggos_swift_flags_is_empty();
    fn eggos_swift_set_algebra_init_dispatch();
    fn eggos_swift_set_algebra_init_sequence_dispatch();
}

fn address(function: unsafe extern "C" fn()) -> u64 {
    function as *const () as u64
}

/// `SetAlgebra`: `Equatable`, `ExpressibleByArrayLiteral`, `Element`, then its members in the
/// order the standard library declares them; requirement `i` fills slot `i + 1`.
pub static SET_ALGEBRA: ProtocolDescriptor<21> = ProtocolDescriptor::new([
    requirement(BASE_PROTOCOL),
    requirement(BASE_PROTOCOL),
    requirement(ASSOCIATED_TYPE),
    requirement(INIT),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(METHOD | INSTANCE),
    requirement(GETTER | INSTANCE),
    requirement(INIT),
    requirement(METHOD | INSTANCE),
]);

/// A `SetAlgebra` witness table: the conformance, the base protocols' tables, `Element`'s
/// metadata, then the members' witnesses.
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct SetAlgebraTable {
    conformance: &'static ConformanceDescriptor,
    equatable: &'static EquatableTable<u64>,
    array_literal: usize,
    element: &'static metadata::StructMetadata,
    witnesses: [unsafe extern "C" fn(); SET_ALGEBRA_WITNESSES],
}

extern "C" fn flags_equal(left: *const u64, right: *const u64) -> bool {
    // SAFETY: the two values `==` takes by address.
    unsafe { left.read_unaligned() == right.read_unaligned() }
}

extern "C" fn missing_witness() {
    super::fatal_error(&[b"a DispatchWorkItemFlags SetAlgebra member EggOS does not have yet"]);
}

static FLAGS_EQUATABLE_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
static FLAGS_EQUATABLE: EquatableTable<u64> = EquatableTable {
    conformance: &FLAGS_EQUATABLE_CONFORMANCE,
    equal: flags_equal,
};
static FLAGS_SET_ALGEBRA_CONFORMANCE: ConformanceDescriptor = ConformanceDescriptor::new();
/// `DispatchWorkItemFlags: SetAlgebra`.
pub static FLAGS_SET_ALGEBRA: SetAlgebraTable = SetAlgebraTable {
    conformance: &FLAGS_SET_ALGEBRA_CONFORMANCE,
    equatable: &FLAGS_EQUATABLE,
    array_literal: 0,
    element: &metadata::DISPATCH_WORK_ITEM_FLAGS,
    witnesses: [
        eggos_swift_flags_empty,
        eggos_swift_flags_contains,
        eggos_swift_flags_union,
        eggos_swift_flags_intersection,
        eggos_swift_flags_symmetric_difference,
        missing_witness,
        missing_witness,
        missing_witness,
        eggos_swift_flags_form_union,
        eggos_swift_flags_form_intersection,
        eggos_swift_flags_form_symmetric_difference,
        eggos_swift_flags_subtracting,
        eggos_swift_flags_is_subset,
        eggos_swift_flags_is_disjoint,
        eggos_swift_flags_is_superset,
        eggos_swift_flags_is_empty,
        eggos_swift_flags_from_sequence,
        eggos_swift_flags_subtract,
    ],
};

fn read(value: *const u64) -> u64 {
    // SAFETY: a `DispatchWorkItemFlags` (one `UInt`) by address, as the witnesses take it.
    unsafe { value.read_unaligned() }
}

fn write(out: *mut u64, value: u64) {
    // SAFETY: the indirect result's or the `inout` self's room for one `UInt`.
    unsafe { out.write_unaligned(value) }
}

extern "C" fn flags_empty(out: *mut u64) {
    write(out, 0);
}

extern "C" fn flags_from_sequence(
    sequence: *const usize,
    sequence_type: Metadata,
    _sequence_table: usize,
    _self_type: Metadata,
    _self_table: usize,
    out: *mut u64,
) {
    let mut bits = 0;
    let is_array = word(sequence_type + 8) == metadata::ARRAY_DESCRIPTOR.address();
    if is_array {
        // SAFETY: an `Array` by address: its one word is its buffer.
        let array = unsafe { sequence.read() };
        let storage = super::storage::array_storage(array);
        let start = super::storage::elements(storage, &metadata::INT64_WITNESSES);
        for index in 0..super::storage::count(storage) {
            bits |= word(start + index * 8) as u64;
        }
        super::heap_object::swift_release(storage as *mut u8);
    }
    write(out, bits);
}

extern "C" fn flags_contains(member: *const u64, this: *const u64) -> bool {
    let member = read(member);
    read(this) & member == member
}

extern "C" fn flags_union(other: *const u64, this: *const u64, out: *mut u64) {
    write(out, read(this) | read(other));
}

extern "C" fn flags_intersection(other: *const u64, this: *const u64, out: *mut u64) {
    write(out, read(this) & read(other));
}

extern "C" fn flags_symmetric_difference(other: *const u64, this: *const u64, out: *mut u64) {
    write(out, read(this) ^ read(other));
}

extern "C" fn flags_subtracting(other: *const u64, this: *const u64, out: *mut u64) {
    write(out, read(this) & !read(other));
}

extern "C" fn flags_form_union(other: *const u64, this: *mut u64) {
    write(this, read(this) | read(other));
}

extern "C" fn flags_form_intersection(other: *const u64, this: *mut u64) {
    write(this, read(this) & read(other));
}

extern "C" fn flags_form_symmetric_difference(other: *const u64, this: *mut u64) {
    write(this, read(this) ^ read(other));
}

extern "C" fn flags_subtract(other: *const u64, this: *mut u64) {
    write(this, read(this) & !read(other));
}

extern "C" fn flags_is_subset(other: *const u64, this: *const u64) -> bool {
    read(this) & !read(other) == 0
}

extern "C" fn flags_is_disjoint(other: *const u64, this: *const u64) -> bool {
    read(this) & read(other) == 0
}

extern "C" fn flags_is_superset(other: *const u64, this: *const u64) -> bool {
    let other = read(other);
    read(this) & other == other
}

extern "C" fn flags_is_empty(this: *const u64) -> bool {
    read(this) == 0
}

/// A metadata accessor's answer: the metadata and its state (complete).
#[repr(C)]
pub struct MetadataResponse {
    metadata: Metadata,
    state: usize,
}

extern "C" fn qos_metadata(_request: usize) -> MetadataResponse {
    MetadataResponse {
        metadata: address_of(&metadata::DISPATCH_QOS),
        state: 0,
    }
}

extern "C" fn flags_metadata(_request: usize) -> MetadataResponse {
    MetadataResponse {
        metadata: address_of(&metadata::DISPATCH_WORK_ITEM_FLAGS),
        state: 0,
    }
}

/// `DispatchQoS` as Swift returns it: its class (an enum's case) and relative priority.
#[repr(C)]
pub struct QualityOfService {
    class: usize,
    relative_priority: isize,
}

extern "C" fn qos_unspecified() -> QualityOfService {
    QualityOfService {
        class: QOS_UNSPECIFIED,
        relative_priority: 0,
    }
}

fn libdispatch(name: &[u8]) -> usize {
    crate::libsystem::dispatch::resolve(name).unwrap_or(0)
}

extern "C" fn main_queue() -> usize {
    libdispatch(b"__dispatch_main_q")
}

extern "C" fn queue_async(
    group: usize,
    _class: usize,
    _relative_priority: isize,
    _flags: usize,
    block: usize,
    queue: usize,
) {
    let (name, first) = match group {
        0 => (&b"_dispatch_async"[..], queue),
        group => (&b"_dispatch_group_async"[..], group),
    };
    let function = libdispatch(name);
    if function == 0 {
        return;
    }
    match group {
        0 => {
            // SAFETY: libdispatch's `dispatch_async(queue, block)`.
            let call: extern "C" fn(usize, usize) = unsafe { core::mem::transmute(function) };
            call(first, block);
        }
        _ => {
            // SAFETY: libdispatch's `dispatch_group_async(group, queue, block)`.
            let call: extern "C" fn(usize, usize, usize) =
                unsafe { core::mem::transmute(function) };
            call(first, queue, block);
        }
    }
}

fn graphics(name: &[u8]) -> usize {
    crate::appkit::resolve(name).unwrap_or(0) as usize
}

extern "C" fn context_move(context: usize, x: f64, y: f64) {
    let function = graphics(b"_CGContextMoveToPoint");
    if function != 0 {
        // SAFETY: the layer's `CGContextMoveToPoint`.
        let call: extern "C" fn(usize, f64, f64) = unsafe { core::mem::transmute(function) };
        call(context, x, y);
    }
}

extern "C" fn context_line(context: usize, x: f64, y: f64) {
    let function = graphics(b"_CGContextAddLineToPoint");
    if function != 0 {
        // SAFETY: the layer's `CGContextAddLineToPoint`.
        let call: extern "C" fn(usize, f64, f64) = unsafe { core::mem::transmute(function) };
        call(context, x, y);
    }
}

extern "C" fn context_arc(
    context: usize,
    clockwise: usize,
    x: f64,
    y: f64,
    radius: f64,
    start: f64,
    end: f64,
) {
    let function = graphics(b"_CGContextAddArc");
    if function != 0 {
        type Arc = extern "C" fn(usize, f64, f64, f64, f64, f64, i32);
        // SAFETY: the layer's `CGContextAddArc`.
        let call: Arc = unsafe { core::mem::transmute(function) };
        call(context, x, y, radius, start, end, i32::from(clockwise & 1 != 0));
    }
}

extern "C" fn cocoa_array_end_index(array: Id) -> isize {
    send(array, sel!("count")) as isize
}

extern "C" fn cocoa_array_element(index: isize, array: Id) -> Id {
    let element = send1(array, sel!("objectAtIndex:"), index as usize);
    crate::objc::reference_count::objc_retain(element)
}

static FORCE_LOAD: u8 = 0;

/// Fills the conformances' relative pointers; once, before the program runs.
pub fn set_up() {
    FLAGS_EQUATABLE_CONFORMANCE.fill(
        &raw const EQUATABLE as usize,
        &metadata::DISPATCH_WORK_ITEM_FLAGS_DESCRIPTOR,
        &raw const FLAGS_EQUATABLE as usize,
    );
    FLAGS_SET_ALGEBRA_CONFORMANCE.fill(
        &raw const SET_ALGEBRA as usize,
        &metadata::DISPATCH_WORK_ITEM_FLAGS_DESCRIPTOR,
        &raw const FLAGS_SET_ALGEBRA as usize,
    );
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    if name.starts_with(b"__swift_FORCE_LOAD_$_swift") {
        return Some(&raw const FORCE_LOAD as u64);
    }
    Some(match name {
        b"_$s8Dispatch0A3QoSVMa" => qos_metadata as *const () as u64,
        b"_$s8Dispatch0A3QoSVMn" => metadata::DISPATCH_QOS_DESCRIPTOR.address() as u64,
        b"_$s8Dispatch0A3QoSVN" => address_of(&metadata::DISPATCH_QOS) as u64,
        b"_$s8Dispatch0A3QoSV11unspecifiedACvgZ" => qos_unspecified as *const () as u64,
        b"_$s8Dispatch0A13WorkItemFlagsVMa" => flags_metadata as *const () as u64,
        b"_$s8Dispatch0A13WorkItemFlagsVMn" => {
            metadata::DISPATCH_WORK_ITEM_FLAGS_DESCRIPTOR.address() as u64
        }
        b"_$s8Dispatch0A13WorkItemFlagsVN" => {
            address_of(&metadata::DISPATCH_WORK_ITEM_FLAGS) as u64
        }
        b"_$s8Dispatch0A13WorkItemFlagsVs10SetAlgebraAAMc" => {
            &raw const FLAGS_SET_ALGEBRA_CONFORMANCE as u64
        }
        b"_$s8Dispatch0A13WorkItemFlagsVs10SetAlgebraAAWP" => {
            &raw const FLAGS_SET_ALGEBRA as u64
        }
        b"_$sSo17OS_dispatch_queueC8DispatchE4mainABvgZ" => main_queue as *const () as u64,
        b"_$sSo17OS_dispatch_queueC8DispatchE5async5group3qos5flags7executeySo0a1_b1_F0CSg_AC0D3QoSVAC0D13WorkItemFlagsVyyXBtF" => {
            address(eggos_swift_queue_async)
        }
        b"_$sSo12CGContextRefa12CoreGraphicsE4move2toySo7CGPointV_tF" => {
            address(eggos_swift_context_move)
        }
        b"_$sSo12CGContextRefa12CoreGraphicsE7addLine2toySo7CGPointV_tF" => {
            address(eggos_swift_context_line)
        }
        b"_$sSo12CGContextRefa12CoreGraphicsE6addArc6center6radius10startAngle03endJ09clockwiseySo7CGPointV_AC7CGFloatVA2MSbtF" => {
            address(eggos_swift_context_arc)
        }
        b"_$ss10SetAlgebraMp" => &raw const SET_ALGEBRA as u64,
        b"_$ss10SetAlgebraPxycfCTj" => address(eggos_swift_set_algebra_init_dispatch),
        b"_$ss10SetAlgebraPyxqd__ncSTRd__7ElementQyd__ACRtzlufCTj" => {
            address(eggos_swift_set_algebra_init_sequence_dispatch)
        }
        b"_$ss10SetAlgebraPxycfCTq" => SET_ALGEBRA.requirement(3),
        b"_$ss10SetAlgebraPyxqd__ncSTRd__7ElementQyd__ACRtzlufCTq" => SET_ALGEBRA.requirement(19),
        b"_$ss18_CocoaArrayWrapperV8endIndexSivg" => cocoa_array_end_index as *const () as u64,
        b"_$ss18_CocoaArrayWrapperVyyXlSicig" => cocoa_array_element as *const () as u64,
        _ => return None,
    })
}
