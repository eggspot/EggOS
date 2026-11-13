//! The objects the ACPI namespace and AML methods hold (ACPI 6.5, 19.3.5 and 19.6.97).

use crate::name::Path;
use crate::namespace::NodeId;
use alloc::boxed::Box;
use alloc::vec::Vec;

/// One ACPI object, held by a namespace node, a method's local or argument, or a package.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Object {
    #[default]
    Uninitialized,
    Integer(u64),
    String(Vec<u8>),
    Buffer(Vec<u8>),
    Package(Vec<Object>),
    /// A reference made by `RefOf`, `Index` or `CondRefOf`.
    Reference(Target),
    /// A name written in a package, which stays a name: `_PRT`'s sources, `_PSD`'s domains.
    NameReference(Path),
    Method(Method),
    NativeMethod(NativeMethod),
    /// A scope the namespace starts with (`\_GPE`, `\_PR`, `\_SI`).
    Scope,
    Device,
    Processor {
        id: u8,
        block_address: u32,
        block_length: u8,
    },
    PowerResource {
        system_level: u8,
        order: u16,
    },
    ThermalZone,
    Mutex {
        sync_level: u8,
        held: u32,
    },
    Event {
        signals: u64,
    },
    Region(Region),
    Field(Field),
    BufferField(BufferField),
    Debug,
}

/// The kinds `ObjectType` reports (ACPI 6.5, 19.6.97).
pub mod object_type {
    pub const UNINITIALIZED: u64 = 0;
    pub const INTEGER: u64 = 1;
    pub const STRING: u64 = 2;
    pub const BUFFER: u64 = 3;
    pub const PACKAGE: u64 = 4;
    pub const FIELD_UNIT: u64 = 5;
    pub const DEVICE: u64 = 6;
    pub const EVENT: u64 = 7;
    pub const METHOD: u64 = 8;
    pub const MUTEX: u64 = 9;
    pub const OPERATION_REGION: u64 = 10;
    pub const POWER_RESOURCE: u64 = 11;
    pub const PROCESSOR: u64 = 12;
    pub const THERMAL_ZONE: u64 = 13;
    pub const BUFFER_FIELD: u64 = 14;
    pub const DEBUG: u64 = 16;
}

impl Object {
    /// The kind `ObjectType` reports for it.
    pub fn object_type(&self) -> u64 {
        use object_type::*;
        match self {
            Object::Uninitialized | Object::Scope | Object::NameReference(_) => UNINITIALIZED,
            Object::Integer(_) => INTEGER,
            Object::String(_) => STRING,
            Object::Buffer(_) => BUFFER,
            Object::Package(_) => PACKAGE,
            Object::Reference(_) => UNINITIALIZED,
            Object::Method(_) | Object::NativeMethod(_) => METHOD,
            Object::Device => DEVICE,
            Object::Processor { .. } => PROCESSOR,
            Object::PowerResource { .. } => POWER_RESOURCE,
            Object::ThermalZone => THERMAL_ZONE,
            Object::Mutex { .. } => MUTEX,
            Object::Event { .. } => EVENT,
            Object::Region(_) => OPERATION_REGION,
            Object::Field(_) => FIELD_UNIT,
            Object::BufferField(_) => BUFFER_FIELD,
            Object::Debug => DEBUG,
        }
    }

    /// Whether it opens a scope other names live in.
    pub fn is_scope(&self) -> bool {
        matches!(
            self,
            Object::Scope
                | Object::Device
                | Object::Processor { .. }
                | Object::PowerResource { .. }
                | Object::ThermalZone
                | Object::Method(_)
        )
    }

    pub fn as_integer(&self) -> Option<u64> {
        match self {
            Object::Integer(value) => Some(*value),
            _ => None,
        }
    }
}

/// A method AML defines: its body is `start..end` of table `table`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    pub table: usize,
    pub start: usize,
    pub end: usize,
    pub arguments: u8,
    pub serialized: bool,
    pub sync_level: u8,
}

/// A method the interpreter answers itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeMethod {
    /// `\_OSI`: whether the operating system answers to an interface string.
    OperatingSystemInterface,
}

/// Where a reference points.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Nowhere: a result that is thrown away.
    Null,
    Node(NodeId),
    Local(u8),
    Argument(u8),
    Debug,
    /// One element of a package, byte of a buffer or character of a string.
    Element(Box<Target>, usize),
    /// An object with no home, such as a package written in place.
    Temporary(Box<Object>),
}

/// The address spaces an operation region may live in (ACPI 6.5, 19.6.100).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionSpace {
    SystemMemory,
    SystemIo,
    PciConfig,
    EmbeddedControl,
    SmBus,
    SystemCmos,
    PciBarTarget,
    Ipmi,
    GeneralPurposeIo,
    GenericSerialBus,
    PlatformCommunicationChannel,
    Other(u8),
}

impl RegionSpace {
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Self::SystemMemory,
            1 => Self::SystemIo,
            2 => Self::PciConfig,
            3 => Self::EmbeddedControl,
            4 => Self::SmBus,
            5 => Self::SystemCmos,
            6 => Self::PciBarTarget,
            7 => Self::Ipmi,
            8 => Self::GeneralPurposeIo,
            9 => Self::GenericSerialBus,
            10 => Self::PlatformCommunicationChannel,
            other => Self::Other(other),
        }
    }
}

/// A PCI function's configuration space, for a `PCI_Config` region: its segment, bus, device
/// and function, from the `_SEG`, `_BBN` and `_ADR` above the region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciFunction {
    pub segment: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

/// An operation region: `length` bytes from `offset` in `space`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub space: RegionSpace,
    pub offset: u64,
    pub length: u64,
    /// The function a `PCI_Config` region is in, found at its first access.
    pub pci: Option<PciFunction>,
}

/// How a field unit is reached, in units of its width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Any,
    Byte,
    Word,
    DoubleWord,
    QuadWord,
    Buffer,
}

impl Access {
    pub fn from_flags(flags: u8) -> Self {
        match flags & 0x0f {
            1 => Self::Byte,
            2 => Self::Word,
            3 => Self::DoubleWord,
            4 => Self::QuadWord,
            5 => Self::Buffer,
            _ => Self::Any,
        }
    }
}

/// What a write does with the bits of a unit the field does not cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateRule {
    Preserve,
    WriteAsOnes,
    WriteAsZeros,
}

impl UpdateRule {
    pub fn from_flags(flags: u8) -> Self {
        match flags >> 5 & 0x03 {
            1 => Self::WriteAsOnes,
            2 => Self::WriteAsZeros,
            _ => Self::Preserve,
        }
    }
}

/// What a field unit's bits live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// An operation region.
    Region(NodeId),
    /// A data field reached by writing each unit's byte offset to an index field first.
    Indexed { index: NodeId, data: NodeId },
    /// An operation region whose bank field is set to `value` first.
    Banked {
        region: NodeId,
        bank: NodeId,
        value: u64,
    },
}

/// A field unit: `bit_length` bits from `bit_offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub kind: FieldKind,
    pub bit_offset: u64,
    pub bit_length: u64,
    pub access: Access,
    pub update: UpdateRule,
    /// The field list said `Lock`: every access holds the firmware's global lock (D556).
    pub lock: bool,
}

/// Bits of a buffer named by `CreateField` and its kin.
#[derive(Debug, Clone, PartialEq)]
pub struct BufferField {
    pub source: Target,
    pub bit_offset: u64,
    pub bit_length: u64,
}
