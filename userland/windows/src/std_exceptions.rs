//! The C++ standard library's exception classes the Windows layer throws itself (D230):
//! `std::bad_cast` from a failing reference `dynamic_cast`, `std::bad_alloc` and
//! `std::length_error` from msvcp140's `_X*` functions, `std::system_error` from its threads'
//! errors. A program catches them by the decorated names of its own types, so the layer lays
//! out its own `ThrowInfo`, catchable types, type descriptors and run-time type information
//! as MSVC does for a 64-bit image, every address relative to this table's own first byte,
//! which a thrown record names as the throw's image base. Every class derives singly from
//! `std::exception` at offset 0, holds `std::exception`'s data (`__std_exception_data`) after
//! its virtual function table pointer and has no destructor of its own to run (its message
//! is never owned), so its objects are copied byte for byte.

use crate::unwind::Image;
use core::mem::{offset_of, size_of};

const NAME_LENGTH: usize = 32;
const MAX_BASES: usize = 4;
const COUNT: usize = StdException::ALL.len();
const IMAGE_RELATIVE: u32 = 1;
const NO_VIRTUAL_BASE: u32 = u32::MAX;

/// The size of `std::exception` and of every class here that adds no member.
pub const EXCEPTION_SIZE: u32 = 24;
/// The size of `std::system_error`: `std::exception` and an `std::error_code`.
pub const SYSTEM_ERROR_SIZE: u32 = 40;

/// A standard exception class the layer throws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdException {
    Exception,
    BadAlloc,
    BadArrayNewLength,
    BadCast,
    BadTypeid,
    NonRttiObject,
    BadFunctionCall,
    BadException,
    LogicError,
    LengthError,
    OutOfRange,
    InvalidArgument,
    RuntimeError,
    OverflowError,
    SystemErrorBase,
    SystemError,
}

impl StdException {
    /// Every class, in table order.
    pub const ALL: [StdException; 16] = [
        StdException::Exception,
        StdException::BadAlloc,
        StdException::BadArrayNewLength,
        StdException::BadCast,
        StdException::BadTypeid,
        StdException::NonRttiObject,
        StdException::BadFunctionCall,
        StdException::BadException,
        StdException::LogicError,
        StdException::LengthError,
        StdException::OutOfRange,
        StdException::InvalidArgument,
        StdException::RuntimeError,
        StdException::OverflowError,
        StdException::SystemErrorBase,
        StdException::SystemError,
    ];

    /// Its place in [`StdException::ALL`] and in the table.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The class and its bases, most derived first.
    pub const fn bases(self) -> &'static [StdException] {
        use StdException::*;
        match self {
            Exception => &[Exception],
            BadAlloc => &[BadAlloc, Exception],
            BadArrayNewLength => &[BadArrayNewLength, BadAlloc, Exception],
            BadCast => &[BadCast, Exception],
            BadTypeid => &[BadTypeid, Exception],
            NonRttiObject => &[NonRttiObject, BadTypeid, Exception],
            BadFunctionCall => &[BadFunctionCall, Exception],
            BadException => &[BadException, Exception],
            LogicError => &[LogicError, Exception],
            LengthError => &[LengthError, LogicError, Exception],
            OutOfRange => &[OutOfRange, LogicError, Exception],
            InvalidArgument => &[InvalidArgument, LogicError, Exception],
            RuntimeError => &[RuntimeError, Exception],
            OverflowError => &[OverflowError, RuntimeError, Exception],
            SystemErrorBase => &[SystemErrorBase, RuntimeError, Exception],
            SystemError => &[SystemError, SystemErrorBase, RuntimeError, Exception],
        }
    }

    /// Its decorated name, as a type descriptor holds it.
    pub const fn decorated_name(self) -> &'static [u8] {
        use StdException::*;
        match self {
            Exception => b".?AVexception@std@@",
            BadAlloc => b".?AVbad_alloc@std@@",
            BadArrayNewLength => b".?AVbad_array_new_length@std@@",
            BadCast => b".?AVbad_cast@std@@",
            BadTypeid => b".?AVbad_typeid@std@@",
            NonRttiObject => b".?AV__non_rtti_object@std@@",
            BadFunctionCall => b".?AVbad_function_call@std@@",
            BadException => b".?AVbad_exception@std@@",
            LogicError => b".?AVlogic_error@std@@",
            LengthError => b".?AVlength_error@std@@",
            OutOfRange => b".?AVout_of_range@std@@",
            InvalidArgument => b".?AVinvalid_argument@std@@",
            RuntimeError => b".?AVruntime_error@std@@",
            OverflowError => b".?AVoverflow_error@std@@",
            SystemErrorBase => b".?AV_System_error@std@@",
            SystemError => b".?AVsystem_error@std@@",
        }
    }

    /// Its name as `type_info::name()` gives it, NUL-terminated.
    pub const fn undecorated_name(self) -> &'static [u8] {
        use StdException::*;
        match self {
            Exception => b"class std::exception\0",
            BadAlloc => b"class std::bad_alloc\0",
            BadArrayNewLength => b"class std::bad_array_new_length\0",
            BadCast => b"class std::bad_cast\0",
            BadTypeid => b"class std::bad_typeid\0",
            NonRttiObject => b"class std::__non_rtti_object\0",
            BadFunctionCall => b"class std::bad_function_call\0",
            BadException => b"class std::bad_exception\0",
            LogicError => b"class std::logic_error\0",
            LengthError => b"class std::length_error\0",
            OutOfRange => b"class std::out_of_range\0",
            InvalidArgument => b"class std::invalid_argument\0",
            RuntimeError => b"class std::runtime_error\0",
            OverflowError => b"class std::overflow_error\0",
            SystemErrorBase => b"class std::_System_error\0",
            SystemError => b"class std::system_error\0",
        }
    }

    /// What `what()` says when the class is thrown without a message of its own, as MSVC's
    /// runtime words it; NUL-terminated.
    pub const fn default_message(self) -> &'static [u8] {
        use StdException::*;
        match self {
            BadAlloc => b"bad allocation\0",
            BadArrayNewLength => b"bad array new length\0",
            BadCast => b"bad cast\0",
            BadTypeid => b"bad typeid\0",
            NonRttiObject => b"Access violation - no RTTI data!\0",
            BadFunctionCall => b"bad function call\0",
            BadException => b"bad exception\0",
            _ => b"Unknown exception\0",
        }
    }

    /// What `what()` says when the C++ runtime's own casts and `typeid` throw the class, as
    /// MSVC's runtime words it; NUL-terminated.
    pub const fn runtime_message(self) -> &'static [u8] {
        match self {
            StdException::BadCast => b"Bad dynamic_cast!\0",
            StdException::BadTypeid => b"Attempted a typeid of nullptr pointer!\0",
            _ => self.default_message(),
        }
    }

    /// The size of its objects.
    pub const fn size(self) -> u32 {
        match self {
            StdException::SystemErrorBase | StdException::SystemError => SYSTEM_ERROR_SIZE,
            _ => EXCEPTION_SIZE,
        }
    }
}

/// A type descriptor (`TypeDescriptor`, which a program reads as a `type_info`): the address of
/// `type_info`'s virtual function table, the undecorated name `type_info::name()` keeps (filled
/// in here, so the layer's table stays read-only) and the decorated name.
#[repr(C)]
pub struct TypeDescriptor {
    pub table: *const (),
    pub undecorated: *const u8,
    pub name: [u8; NAME_LENGTH],
}

/// The layer's C++ data for its exception classes, laid out as in a 64-bit image: one type
/// descriptor, `ThrowInfo`, catchable type array, catchable type, complete object locator,
/// class hierarchy descriptor, base class array and base class descriptor per class. Its first
/// eight bytes hold nothing, since MSVC's data reads an RVA of 0 as no address.
#[repr(C)]
pub struct TypeTable {
    rva_zero_means_none: u64,
    descriptors: [TypeDescriptor; COUNT],
    throw_infos: [[u32; 4]; COUNT],
    catchable_arrays: [[u32; 1 + MAX_BASES]; COUNT],
    catchable_types: [[u32; 7]; COUNT],
    locators: [[u32; 6]; COUNT],
    hierarchies: [[u32; 4]; COUNT],
    base_arrays: [[u32; MAX_BASES]; COUNT],
    base_descriptors: [[u32; 7]; COUNT],
}

const _: () = assert!(
    size_of::<TypeTable>()
        == size_of::<u64>()
            + COUNT
                * (size_of::<TypeDescriptor>()
                    + 4 * (4 + 1 + MAX_BASES + 7 + 6 + 4 + MAX_BASES + 7))
);
const _: () = assert!(size_of::<TypeDescriptor>() == 16 + NAME_LENGTH);

// SAFETY: the table is never written after it is made; its pointers name immutable statics.
unsafe impl Sync for TypeTable {}

const fn rva(offset: usize) -> u32 {
    offset as u32
}

const fn descriptor_rva(index: usize) -> u32 {
    rva(offset_of!(TypeTable, descriptors) + index * size_of::<TypeDescriptor>())
}

const fn catchable_type_rva(index: usize) -> u32 {
    rva(offset_of!(TypeTable, catchable_types) + index * 28)
}

const fn hierarchy_rva(index: usize) -> u32 {
    rva(offset_of!(TypeTable, hierarchies) + index * 16)
}

const fn base_descriptor_rva(index: usize) -> u32 {
    rva(offset_of!(TypeTable, base_descriptors) + index * 28)
}

impl TypeTable {
    /// The table, with `type_info_table` as every type descriptor's virtual function table.
    pub const fn new(type_info_table: *const ()) -> Self {
        let mut table = Self {
            rva_zero_means_none: 0,
            descriptors: [const {
                TypeDescriptor {
                    table: core::ptr::null(),
                    undecorated: core::ptr::null(),
                    name: [0; NAME_LENGTH],
                }
            }; COUNT],
            throw_infos: [[0; 4]; COUNT],
            catchable_arrays: [[0; 1 + MAX_BASES]; COUNT],
            catchable_types: [[0; 7]; COUNT],
            locators: [[0; 6]; COUNT],
            hierarchies: [[0; 4]; COUNT],
            base_arrays: [[0; MAX_BASES]; COUNT],
            base_descriptors: [[0; 7]; COUNT],
        };
        let mut index = 0;
        while index < COUNT {
            let kind = StdException::ALL[index];
            let bases = kind.bases();
            let decorated = kind.decorated_name();
            let descriptor = &mut table.descriptors[index];
            descriptor.table = type_info_table;
            descriptor.undecorated = kind.undecorated_name().as_ptr();
            let mut at = 0;
            while at < decorated.len() {
                descriptor.name[at] = decorated[at];
                at += 1;
            }
            table.throw_infos[index] = [
                0,
                0,
                0,
                rva(offset_of!(TypeTable, catchable_arrays) + index * 4 * (1 + MAX_BASES)),
            ];
            table.catchable_arrays[index][0] = bases.len() as u32;
            table.locators[index] = [
                IMAGE_RELATIVE,
                0,
                0,
                descriptor_rva(index),
                hierarchy_rva(index),
                rva(offset_of!(TypeTable, locators) + index * 24),
            ];
            table.hierarchies[index] = [
                0,
                0,
                bases.len() as u32,
                rva(offset_of!(TypeTable, base_arrays) + index * 4 * MAX_BASES),
            ];
            let mut base = 0;
            while base < bases.len() {
                let of = bases[base].index();
                table.catchable_arrays[index][1 + base] = catchable_type_rva(of);
                table.base_arrays[index][base] = base_descriptor_rva(of);
                base += 1;
            }
            table.catchable_types[index] = [
                0,
                descriptor_rva(index),
                0,
                NO_VIRTUAL_BASE,
                0,
                kind.size(),
                0,
            ];
            table.base_descriptors[index] = [
                descriptor_rva(index),
                bases.len() as u32 - 1,
                0,
                NO_VIRTUAL_BASE,
                0,
                0,
                hierarchy_rva(index),
            ];
            index += 1;
        }
        table
    }

    /// The table as an image whose base is its first byte.
    pub fn image(&self) -> Image<'_> {
        Image {
            base: self as *const Self as u64,
            // SAFETY: the table is a `repr(C)` value without padding (its size is the sum of
            // its fields', checked above), so all its bytes are initialized; it is not written
            // while borrowed.
            bytes: unsafe {
                core::slice::from_raw_parts((self as *const Self).cast::<u8>(), size_of::<Self>())
            },
        }
    }

    /// The address of `kind`'s `ThrowInfo`.
    pub fn throw_info(&self, kind: StdException) -> u64 {
        &raw const self.throw_infos[kind.index()] as u64
    }

    /// The address of `kind`'s complete object locator, which its virtual function table's
    /// word before the first slot names.
    pub const fn locator(&self, kind: StdException) -> *const () {
        (&raw const self.locators[kind.index()]).cast()
    }

    /// Whether `address` lies in the table.
    pub fn holds(&self, address: u64) -> bool {
        self.image().rva_of(address).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cxx_data::{self, Handler, Images, flags};
    use crate::cxx_walk::ObjectMemory;
    use crate::rtti;

    struct Real;

    impl ObjectMemory for Real {
        fn u64_at(&self, address: u64) -> u64 {
            // SAFETY: the tests read only the table and the objects they made.
            unsafe { (address as *const u64).read_unaligned() }
        }

        fn i32_at(&self, address: u64) -> i32 {
            // SAFETY: as above.
            unsafe { (address as *const i32).read_unaligned() }
        }

        fn u8_at(&self, address: u64) -> u8 {
            // SAFETY: as above.
            unsafe { (address as *const u8).read() }
        }
    }

    #[test]
    fn a_length_error_is_catchable_as_itself_a_logic_error_and_an_exception() {
        let table = Box::new(TypeTable::new(core::ptr::null()));
        let image = table.image();
        let info =
            cxx_data::throw_info(&image, table.throw_info(StdException::LengthError)).unwrap();
        assert_eq!((info.attributes, info.destructor), (0, 0));
        assert_eq!(info.catchable_count, 3);
        let names: Vec<&[u8]> = (0..3)
            .map(|index| {
                let catchable = cxx_data::catchable_type(&image, &info, index).unwrap();
                assert_eq!(catchable.size, EXCEPTION_SIZE);
                assert_eq!(catchable.copy_function, 0);
                assert_eq!(catchable.virtual_base_pointer, -1);
                cxx_data::type_name(&image, catchable.type_descriptor)
            })
            .collect();
        assert_eq!(
            names,
            [
                &b".?AVlength_error@std@@"[..],
                b".?AVlogic_error@std@@",
                b".?AVexception@std@@"
            ]
        );
        let system = cxx_data::throw_info(&image, table.throw_info(StdException::SystemError));
        assert_eq!(system.map(|info| info.catchable_count), Some(4));
    }

    #[test]
    fn a_program_handler_catches_the_layer_s_object_by_its_base_class_name() {
        let table = Box::new(TypeTable::new(core::ptr::null()));
        let thrown = table.image();
        let mut program = vec![0u8; 0x40];
        program[0x10..0x10 + 21].copy_from_slice(b".?AVlogic_error@std@@");
        let handler_image = Image {
            base: 0x1_4000_0000,
            bytes: &program,
        };
        let images = Images {
            handler: handler_image,
            thrown,
        };
        let handler = Handler {
            adjectives: flags::HANDLER_REFERENCE | flags::HANDLER_CONST,
            type_descriptor: 0x1_4000_0000,
            catch_object: 0,
            funclet: 0,
            continuations: [0; 2],
            continuation_count: 0,
        };
        let info =
            cxx_data::throw_info(&thrown, table.throw_info(StdException::OutOfRange)).unwrap();
        let caught: Vec<bool> = (0..info.catchable_count)
            .map(|index| {
                let catchable = cxx_data::catchable_type(&thrown, &info, index).unwrap();
                cxx_data::handler_catches(&images, &handler, &catchable, info.attributes)
            })
            .collect();
        assert_eq!(caught, [false, true, false]);
        let info = cxx_data::throw_info(&thrown, table.throw_info(StdException::BadCast)).unwrap();
        assert!((0..info.catchable_count).all(|index| {
            let catchable = cxx_data::catchable_type(&thrown, &info, index).unwrap();
            !cxx_data::handler_catches(&images, &handler, &catchable, info.attributes)
        }));
    }

    fn descriptor(table: &TypeTable, kind: StdException) -> u64 {
        &raw const table.descriptors[kind.index()] as u64
    }

    #[test]
    fn the_layer_s_objects_carry_rtti_for_typeid_and_dynamic_cast() {
        let table = Box::new(TypeTable::new(core::ptr::null()));
        let virtual_table: Box<[u64; 3]> =
            Box::new([table.locator(StdException::BadArrayNewLength) as u64, 0, 0]);
        let object: Box<[u64; 3]> = Box::new([&raw const virtual_table[1] as u64, 0, 0]);
        let address = object.as_ptr() as u64;
        let locator = rtti::locator(&Real, address).unwrap();
        let image = table.image();
        assert_eq!(
            cxx_data::type_name(&image, locator.type_descriptor),
            b".?AVbad_array_new_length@std@@"
        );
        let bases = rtti::base_classes(&Real, &locator);
        assert_eq!(bases.len(), 3);
        assert_eq!(bases[0].contained, 2);
        assert_eq!(bases[2].contained, 0);
        let bad_alloc = descriptor(&table, StdException::BadAlloc);
        let source = descriptor(&table, StdException::Exception);
        assert_eq!(
            rtti::dynamic_cast(&Real, address, 0, source, bad_alloc),
            Some(address)
        );
        let bad_cast = descriptor(&table, StdException::BadCast);
        assert_eq!(
            rtti::dynamic_cast(&Real, address, 0, source, bad_cast),
            None
        );
    }

    #[test]
    fn names_fit_their_slots_and_end_in_nul() {
        for kind in StdException::ALL {
            assert!(kind.decorated_name().len() < NAME_LENGTH, "{kind:?}");
            assert_eq!(kind.undecorated_name().last(), Some(&0));
            assert_eq!(kind.default_message().last(), Some(&0));
            assert_eq!(kind.runtime_message().last(), Some(&0));
            assert!(kind.bases().len() <= MAX_BASES);
            assert_eq!(kind.bases()[0], kind);
            assert_eq!(kind.bases().last(), Some(&StdException::Exception));
        }
    }
}
