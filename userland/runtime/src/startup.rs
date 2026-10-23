//! What Linux hands a program on its stack at entry: `argc`, the `argv` and `envp` pointer
//! arrays, each ended by a null pointer, then the auxiliary vector's (type, value) pairs ended
//! by `AT_NULL`.

use core::ffi::{CStr, c_char};

/// The auxiliary vector's entry types.
pub mod auxv {
    pub const NULL: usize = 0;
    pub const PHDR: usize = 3;
    pub const PHENT: usize = 4;
    pub const PHNUM: usize = 5;
    pub const PAGESZ: usize = 6;
    pub const BASE: usize = 7;
    pub const ENTRY: usize = 9;
    pub const UID: usize = 11;
    pub const EUID: usize = 12;
    pub const GID: usize = 13;
    pub const EGID: usize = 14;
    pub const CLKTCK: usize = 17;
    pub const SECURE: usize = 23;
    pub const RANDOM: usize = 25;
}

/// ELF program header types the runtime looks for.
pub mod segment {
    pub const DYNAMIC: u32 = 2;
    pub const GNU_RELRO: u32 = 0x6474_e552;
}

/// An ELF-64 program header, as the auxiliary vector's `AT_PHDR` points to them.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ProgramHeader {
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub virt: u64,
    pub phys: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub align: u64,
}

/// The program's start-up stack, read in place.
#[derive(Debug, Clone, Copy)]
pub struct Startup {
    argc: usize,
    argv: *const *const c_char,
    envp: *const *const c_char,
    auxiliary: *const [usize; 2],
}

impl Startup {
    /// Reads the start-up stack at `stack`.
    ///
    /// # Safety
    ///
    /// `stack` points to `argc`, laid out as Linux lays out a program's stack, and that memory
    /// stays in place, unchanged, for as long as the program runs.
    pub unsafe fn from_stack(stack: *const usize) -> Self {
        // SAFETY: the caller's stack starts with `argc`.
        let argc = unsafe { stack.read() };
        let argv = stack.wrapping_add(1).cast::<*const c_char>();
        let envp = argv.wrapping_add(argc + 1);
        let mut end = envp;
        // SAFETY: the environment's pointers follow `argv`'s end marker and end with a null
        // one, which stops the walk.
        while !unsafe { end.read() }.is_null() {
            end = end.wrapping_add(1);
        }
        Self {
            argc,
            argv,
            envp,
            auxiliary: end.wrapping_add(1).cast(),
        }
    }

    /// `argc`.
    pub fn argument_count(&self) -> usize {
        self.argc
    }

    /// The arguments, `argv[0]` first.
    pub fn arguments(&self) -> impl Iterator<Item = &'static CStr> {
        // SAFETY: `from_stack`'s caller promised `argc` pointers to strings that stay.
        unsafe { strings(self.argv) }
    }

    /// The environment's `NAME=value` strings.
    pub fn environment(&self) -> impl Iterator<Item = &'static CStr> {
        // SAFETY: `from_stack`'s caller promised a null-ended array of strings that stay.
        unsafe { strings(self.envp) }
    }

    /// The `argv` array's end marker: a null pointer if the stack is laid out as Linux does.
    pub fn argument_end_marker(&self) -> *const c_char {
        // SAFETY: `from_stack` found `argc` pointers and their end marker here.
        unsafe { self.argv.wrapping_add(self.argc).read() }
    }

    /// The value of the first auxiliary vector entry of type `kind`.
    pub fn auxiliary_value(&self, kind: usize) -> Option<usize> {
        let mut entry = self.auxiliary;
        loop {
            // SAFETY: the vector ends with an `AT_NULL` entry, which stops the walk.
            let [entry_kind, value] = unsafe { entry.read() };
            if entry_kind == auxv::NULL {
                return None;
            }
            if entry_kind == kind {
                return Some(value);
            }
            entry = entry.wrapping_add(1);
        }
    }

    /// The program's headers, as `AT_PHDR` and `AT_PHNUM` name them (none if either is
    /// missing or the entry size is not ELF-64's).
    pub fn program_headers(&self) -> &'static [ProgramHeader] {
        let (Some(address), Some(count)) = (
            self.auxiliary_value(auxv::PHDR),
            self.auxiliary_value(auxv::PHNUM),
        ) else {
            return &[];
        };
        if address == 0
            || self.auxiliary_value(auxv::PHENT) != Some(size_of::<ProgramHeader>())
            || address % align_of::<ProgramHeader>() != 0
        {
            return &[];
        }
        // SAFETY: Linux maps the program headers `AT_PHDR` names, `AT_PHNUM` of them, for as
        // long as the program runs, and they were checked aligned with ELF-64's entry size.
        unsafe { core::slice::from_raw_parts(address as *const ProgramHeader, count) }
    }

    /// How far the program was moved from the addresses it was linked at: where its
    /// dynamic section is, `dynamic`, less where its `PT_DYNAMIC` header says it is.
    pub fn load_bias(&self, dynamic: usize) -> Option<usize> {
        self.program_headers()
            .iter()
            .find(|header| header.kind == segment::DYNAMIC)
            .map(|header| dynamic.wrapping_sub(header.virt as usize))
    }
}

/// The strings of the null-ended pointer array at `array`.
///
/// # Safety
///
/// `array` is null-ended and its strings are null-ended, and both stay in place.
unsafe fn strings(array: *const *const c_char) -> impl Iterator<Item = &'static CStr> {
    let mut next = array;
    core::iter::from_fn(move || {
        // SAFETY: the caller's null-ended array: `next` is at most its end marker.
        let string = unsafe { next.read() };
        if string.is_null() {
            return None;
        }
        next = next.wrapping_add(1);
        // SAFETY: not the end marker, so a null-ended string that stays.
        Some(unsafe { CStr::from_ptr(string) })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_arguments_environment_and_auxiliary_vector() {
        let program = c"/eggos-selftest";
        let flag = c"--check";
        let term = c"TERM=linux";
        let headers = [ProgramHeader {
            kind: segment::DYNAMIC,
            flags: 0,
            offset: 0,
            virt: 0x2000,
            phys: 0,
            file_size: 0,
            memory_size: 0,
            align: 8,
        }];
        let stack = [
            2,
            program.as_ptr() as usize,
            flag.as_ptr() as usize,
            0,
            term.as_ptr() as usize,
            0,
            auxv::PAGESZ,
            4096,
            auxv::PHDR,
            headers.as_ptr() as usize,
            auxv::PHENT,
            size_of::<ProgramHeader>(),
            auxv::PHNUM,
            1,
            auxv::NULL,
            0,
        ];
        // SAFETY: `stack` is laid out as Linux lays out a program's stack and outlives
        // `startup`.
        let startup = unsafe { Startup::from_stack(stack.as_ptr()) };
        assert_eq!(startup.argument_count(), 2);
        assert!(startup.arguments().eq([program, flag]));
        assert!(startup.argument_end_marker().is_null());
        assert!(startup.environment().eq([term]));
        assert_eq!(startup.auxiliary_value(auxv::PAGESZ), Some(4096));
        assert_eq!(startup.auxiliary_value(auxv::RANDOM), None);
        assert_eq!(startup.program_headers().len(), 1);
        assert_eq!(startup.load_bias(0x5_2000), Some(0x5_0000));
    }

    #[test]
    fn has_no_program_headers_without_elf_64_s_entry_size() {
        let stack = [
            0,
            0,
            0,
            auxv::PHDR,
            64,
            auxv::PHENT,
            32,
            auxv::PHNUM,
            1,
            0,
            0,
        ];
        // SAFETY: as above.
        let startup = unsafe { Startup::from_stack(stack.as_ptr()) };
        assert!(startup.program_headers().is_empty());
        assert_eq!(startup.load_bias(0x1000), None);
    }
}
