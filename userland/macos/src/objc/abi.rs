//! The Objective-C ABI as Apple's toolchain lays it out in arm64 and x86_64 executables (D142):
//! classes (`class_t` and `class_ro_t`), method lists in their pointer and relative forms, ivar
//! lists, categories and `objc_super`. The runtime reads these through raw pointers into the
//! mapped image; the readers here check entry sizes and counts so a malformed list is skipped
//! instead of read out of bounds.

use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};

/// A selector: the address of its canonical NUL-terminated name.
pub type Sel = *const u8;

/// A method's implementation: the address of its code.
pub type Imp = usize;

/// The first word of every object: its class.
#[repr(C)]
#[derive(Debug)]
pub struct Object {
    pub isa: AtomicPtr<ClassObject>,
}

/// An object reference (`id`).
pub type Id = *mut Object;

/// `class_t`: a class or metaclass object. The `record` word is the ABI's `vtable`, which
/// clang leaves 0; the runtime keeps its record of the class there.
#[repr(C)]
#[derive(Debug)]
pub struct ClassObject {
    pub isa: AtomicPtr<ClassObject>,
    pub superclass: AtomicPtr<ClassObject>,
    pub cache: AtomicPtr<crate::objc::cache::Cache>,
    pub record: AtomicUsize,
    pub data: AtomicUsize,
}

impl ClassObject {
    #[allow(clippy::declare_interior_mutable_const)]
    pub const EMPTY: Self = Self {
        isa: AtomicPtr::new(core::ptr::null_mut()),
        superclass: AtomicPtr::new(core::ptr::null_mut()),
        cache: AtomicPtr::new(core::ptr::null_mut()),
        record: AtomicUsize::new(0),
        data: AtomicUsize::new(0),
    };

    /// The `class_ro_t` the `data` word points at, its low three flag bits cleared.
    pub fn read_only(&self) -> *const ClassReadOnly {
        (self.data.load(Ordering::Relaxed) & !7) as *const ClassReadOnly
    }
}

/// `class_ro_t`.
#[repr(C)]
#[derive(Debug)]
pub struct ClassReadOnly {
    pub flags: u32,
    pub instance_start: u32,
    pub instance_size: u32,
    pub reserved: u32,
    pub ivar_layout: usize,
    pub name: *const u8,
    pub base_methods: *const ListHeader,
    pub base_protocols: usize,
    pub ivars: *const ListHeader,
    pub weak_ivar_layout: usize,
    pub base_properties: usize,
}

/// The header of a method or ivar list: the entry size (with flags, for methods) and count.
#[repr(C)]
#[derive(Debug)]
pub struct ListHeader {
    pub entry_size_and_flags: u32,
    pub count: u32,
}

const RELATIVE_METHODS: u32 = 0x8000_0000;
const ENTRY_SIZE_MASK: u32 = 0xfffc;
const POINTER_METHOD_SIZE: usize = 24;
const RELATIVE_METHOD_SIZE: usize = 12;
const IVAR_SIZE: usize = 32;
/// Lists longer than this are treated as malformed and skipped.
pub const MAX_LIST_ENTRIES: u32 = 1 << 16;

/// One method of a list: where its name is (a C string) and its implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodEntry {
    pub name: *const u8,
    pub imp: Imp,
}

/// The methods of the list at `list` (null for none), in list order.
///
/// # Safety
/// `list` must be null or point to a method list in mapped memory whose entries, and the
/// selector references and code its relative entries point at, are mapped too.
pub unsafe fn methods(list: *const ListHeader) -> MethodIter {
    if list.is_null() {
        return MethodIter::EMPTY;
    }
    // SAFETY: the caller's promise: the header is mapped.
    let header = unsafe { &*list };
    let relative = header.entry_size_and_flags & RELATIVE_METHODS != 0;
    let size = (header.entry_size_and_flags & ENTRY_SIZE_MASK) as usize;
    let expected = if relative {
        RELATIVE_METHOD_SIZE
    } else {
        POINTER_METHOD_SIZE
    };
    if size != expected || header.count >= MAX_LIST_ENTRIES {
        return MethodIter::EMPTY;
    }
    MethodIter {
        next: list as usize + core::mem::size_of::<ListHeader>(),
        left: header.count as usize,
        size,
        relative,
    }
}

/// The entries of a method list, decoded.
#[derive(Debug)]
pub struct MethodIter {
    next: usize,
    left: usize,
    size: usize,
    relative: bool,
}

impl MethodIter {
    const EMPTY: Self = Self {
        next: 0,
        left: 0,
        size: 0,
        relative: false,
    };
}

fn relative_target(field: usize) -> usize {
    // SAFETY: `methods`' caller promised the entry, so its 32-bit field, is mapped.
    let offset = unsafe { (field as *const i32).read_unaligned() };
    field.wrapping_add_signed(offset as isize)
}

impl Iterator for MethodIter {
    type Item = MethodEntry;

    fn next(&mut self) -> Option<MethodEntry> {
        if self.left == 0 {
            return None;
        }
        let at = self.next;
        self.next += self.size;
        self.left -= 1;
        Some(if self.relative {
            let reference = relative_target(at) as *const *const u8;
            MethodEntry {
                // SAFETY: a relative entry's name is the selector reference it points at, which
                // `methods`' caller promised is mapped.
                name: unsafe { reference.read() },
                imp: relative_target(at + 8),
            }
        } else {
            let entry = at as *const [usize; 3];
            // SAFETY: `methods`' caller promised the 24-byte entry is mapped.
            let [name, _types, imp] = unsafe { entry.read() };
            MethodEntry {
                name: name as *const u8,
                imp,
            }
        })
    }
}

/// `ivar_t`.
#[repr(C)]
#[derive(Debug)]
pub struct Ivar {
    pub offset: *mut AtomicU32,
    pub name: *const u8,
    pub kind: *const u8,
    pub alignment_shift: u32,
    pub size: u32,
}

/// The ivars of the list at `list` (null for none).
///
/// # Safety
/// `list` must be null or point to a mapped ivar list.
pub unsafe fn ivars<'a>(list: *const ListHeader) -> &'a [Ivar] {
    if list.is_null() {
        return &[];
    }
    // SAFETY: the caller's promise.
    let header = unsafe { &*list };
    if header.entry_size_and_flags as usize != IVAR_SIZE || header.count >= MAX_LIST_ENTRIES {
        return &[];
    }
    let first = (list as usize + core::mem::size_of::<ListHeader>()) as *const Ivar;
    // SAFETY: the caller's promise: `count` entries of 32 bytes follow the header.
    unsafe { core::slice::from_raw_parts(first, header.count as usize) }
}

/// How far a class's ivars move when its superclass's instances are `superclass_size` bytes
/// but the class was compiled for `compiled_start` (the non-fragile ivars): the new start is the
/// superclass size rounded up to the largest ivar alignment; ivars never move down.
pub fn ivar_slide(compiled_start: u32, superclass_size: u32, alignment: u32) -> u32 {
    let alignment = alignment.max(1);
    let start = superclass_size.next_multiple_of(alignment);
    start.saturating_sub(compiled_start)
}

/// `category_t`.
#[repr(C)]
#[derive(Debug)]
pub struct Category {
    pub name: *const u8,
    pub class: *mut ClassObject,
    pub instance_methods: *const ListHeader,
    pub class_methods: *const ListHeader,
    pub protocols: usize,
    pub instance_properties: usize,
}

/// `struct objc_super`, which `objc_msgSendSuper2` takes: the receiver and the class whose
/// method is calling, whose superclass the search starts at.
#[cfg(test)]
#[repr(C)]
#[derive(Debug)]
pub struct ObjcSuper {
    pub receiver: Id,
    pub current_class: *mut ClassObject,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_method_lists_decode_in_order() {
        #[repr(C)]
        struct List {
            header: ListHeader,
            entries: [[usize; 3]; 2],
        }
        let names = [
            c"first".as_ptr().cast::<u8>(),
            c"second".as_ptr().cast::<u8>(),
        ];
        let list = List {
            header: ListHeader {
                entry_size_and_flags: 24,
                count: 2,
            },
            entries: [
                [names[0] as usize, 0, 0x1000],
                [names[1] as usize, 0, 0x2000],
            ],
        };
        // SAFETY: the list and its names live on this stack frame.
        let decoded: Vec<_> = unsafe { methods(&list.header) }.collect();
        assert_eq!(
            decoded,
            [
                MethodEntry {
                    name: names[0],
                    imp: 0x1000
                },
                MethodEntry {
                    name: names[1],
                    imp: 0x2000
                }
            ]
        );
    }

    #[test]
    fn relative_method_lists_name_selector_references_and_code_by_offset() {
        #[repr(C)]
        struct Image {
            reference: *const u8,
            code: [u8; 8],
            header: ListHeader,
            entry: [i32; 3],
        }
        let name = c"count".as_ptr().cast::<u8>();
        let mut image = Image {
            reference: name,
            code: [0; 8],
            header: ListHeader {
                entry_size_and_flags: RELATIVE_METHODS | 12,
                count: 1,
            },
            entry: [0; 3],
        };
        let entry = &raw const image.entry as usize;
        let reference = &raw const image.reference as usize;
        let code = &raw const image.code as usize;
        image.entry = [
            (reference as isize - entry as isize) as i32,
            0,
            (code as isize - (entry + 8) as isize) as i32,
        ];
        // SAFETY: as above.
        let decoded: Vec<_> = unsafe { methods(&image.header) }.collect();
        assert_eq!(decoded, [MethodEntry { name, imp: code }]);
    }

    #[test]
    fn malformed_lists_are_skipped() {
        let header = ListHeader {
            entry_size_and_flags: 16,
            count: 4,
        };
        // SAFETY: the header is only read: a wrong entry size ends the list at once.
        assert_eq!(unsafe { methods(&header) }.count(), 0);
        let huge = ListHeader {
            entry_size_and_flags: 24,
            count: MAX_LIST_ENTRIES,
        };
        // SAFETY: as above, the count is refused before any entry is read.
        assert_eq!(unsafe { methods(&huge) }.count(), 0);
        // SAFETY: a null list has no methods.
        assert_eq!(unsafe { methods(core::ptr::null()) }.count(), 0);
        // SAFETY: as above, for ivars.
        assert!(unsafe { ivars(&header) }.is_empty());
    }

    #[test]
    fn ivars_slide_past_a_larger_superclass_only() {
        assert_eq!(ivar_slide(8, 8, 8), 0);
        assert_eq!(ivar_slide(8, 4, 8), 0);
        assert_eq!(ivar_slide(8, 24, 8), 16);
        assert_eq!(ivar_slide(8, 20, 8), 16);
        assert_eq!(ivar_slide(8, 20, 1), 12);
        assert_eq!(ivar_slide(16, 20, 0), 4);
    }

    #[test]
    fn layouts_match_the_abi() {
        assert_eq!(core::mem::size_of::<ClassObject>(), 40);
        assert_eq!(core::mem::size_of::<ClassReadOnly>(), 72);
        assert_eq!(core::mem::size_of::<Ivar>(), 32);
        assert_eq!(core::mem::size_of::<ObjcSuper>(), 16);
        assert_eq!(core::mem::offset_of!(ObjcSuper, current_class), 8);
        assert_eq!(core::mem::offset_of!(ClassObject, cache), 16);
        assert_eq!(core::mem::offset_of!(ClassObject, record), 24);
        assert_eq!(core::mem::offset_of!(ClassReadOnly, name), 24);
        assert_eq!(core::mem::offset_of!(ClassReadOnly, ivars), 48);
        assert_eq!(core::mem::size_of::<Category>(), 48);
    }
}
