//! The Swift calling convention's two differences from C that the runtime meets (D180): `self`
//! of a method that takes it by address (and a class's or metatype's `self`) comes in a register
//! of its own, x20 on arm64 and r13 on x86_64, and an indirect result's address comes in x8 on
//! arm64 and rax on x86_64. Each entry here moves that register into the next C argument
//! register and jumps to the Rust function; [`call_with_context`] is the other direction, for
//! the runtime calling a program's destructor or witness. The protocol dispatch thunks jump
//! through the witness table at the requirement's slot. A function that returns four words
//! (a `Substring`) returns them in registers to Swift but through memory from Rust, so its entry
//! passes a stack buffer and loads the words from it.

use super::metadata::{self, Metadata};
use super::{cast, characters, protocols, standard_protocols, storage, string};

macro_rules! thunk {
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

thunk!(
    "eggos_swift_hasher_finalize",
    "mov x0, x20",
    "mov rdi, r13",
    string::hasher_finalize
);
thunk!(
    "eggos_swift_hasher_combine",
    "mov x1, x20",
    "mov rsi, r13",
    string::hasher_combine
);
thunk!(
    "eggos_swift_hasher_combine8",
    "mov x1, x20",
    "mov rsi, r13",
    string::hasher_combine8
);
thunk!(
    "eggos_swift_hasher_combine16",
    "mov x1, x20",
    "mov rsi, r13",
    string::hasher_combine16
);
thunk!(
    "eggos_swift_hasher_combine32",
    "mov x1, x20",
    "mov rsi, r13",
    string::hasher_combine32
);
thunk!(
    "eggos_swift_hasher_init",
    "mov x1, x0\nmov x0, x8",
    "mov rsi, rdi\nmov rdi, rax",
    string::hasher_init
);
thunk!(
    "eggos_swift_string_append",
    "mov x2, x20",
    "mov rdx, r13",
    string::string_append
);
thunk!(
    "eggos_swift_guts_grow",
    "mov x1, x20",
    "mov rsi, r13",
    string::guts_grow
);
thunk!(
    "eggos_swift_write_ascii",
    "mov x2, x20",
    "mov rdx, r13",
    string::write_ascii
);
thunk!(
    "eggos_swift_string_storage_destroy",
    "mov x0, x20",
    "mov rdi, r13",
    string::destroy_storage
);
thunk!(
    "eggos_swift_array_storage_destroy",
    "mov x0, x20",
    "mov rdi, r13",
    storage::destroy_array
);
thunk!(
    "eggos_swift_dictionary_storage_destroy",
    "mov x0, x20",
    "mov rdi, r13",
    storage::destroy_dictionary
);
thunk!(
    "eggos_swift_dictionary_allocate",
    "mov x1, x20",
    "mov rsi, r13",
    storage::dictionary_allocate
);
thunk!(
    "eggos_swift_dictionary_copy",
    "mov x1, x20",
    "mov rsi, r13",
    storage::dictionary_copy
);
thunk!(
    "eggos_swift_dictionary_resize",
    "mov x3, x20",
    "mov rcx, r13",
    storage::dictionary_resize
);
thunk!(
    "eggos_swift_array_subscript",
    "mov x3, x8",
    "mov rcx, rax",
    storage::array_subscript
);
thunk!(
    "eggos_swift_int_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::int_description
);
thunk!(
    "eggos_swift_int8_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::int8_description
);
thunk!(
    "eggos_swift_int16_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::int16_description
);
thunk!(
    "eggos_swift_int32_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::int32_description
);
thunk!(
    "eggos_swift_uint_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::uint_description
);
thunk!(
    "eggos_swift_uint8_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::uint8_description
);
thunk!(
    "eggos_swift_uint16_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::uint16_description
);
thunk!(
    "eggos_swift_uint32_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::uint32_description
);
thunk!(
    "eggos_swift_double_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::double_description
);
thunk!(
    "eggos_swift_bool_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::bool_description
);
thunk!(
    "eggos_swift_string_description",
    "mov x0, x20",
    "mov rdi, r13",
    protocols::string_description
);

thunk!(
    "eggos_swift_string_hash_value",
    "mov x0, x20",
    "mov rdi, r13",
    standard_protocols::string_hash_value
);
thunk!(
    "eggos_swift_string_hash_into",
    "mov x1, x20",
    "mov rsi, r13",
    standard_protocols::string_hash_into
);
thunk!(
    "eggos_swift_string_raw_hash",
    "mov x1, x20",
    "mov rsi, r13",
    standard_protocols::string_raw_hash
);
thunk!(
    "eggos_swift_int_hash_value",
    "mov x0, x20",
    "mov rdi, r13",
    standard_protocols::int_hash_value
);
thunk!(
    "eggos_swift_int_hash_into",
    "mov x1, x20",
    "mov rsi, r13",
    standard_protocols::int_hash_into
);
thunk!(
    "eggos_swift_int_raw_hash",
    "mov x1, x20",
    "mov rsi, r13",
    standard_protocols::int_raw_hash
);
thunk!(
    "eggos_swift_default_hash_value",
    "mov x2, x20",
    "mov rdx, r13",
    standard_protocols::default_hash_value
);
thunk!(
    "eggos_swift_default_raw_hash_value",
    "mov x3, x20",
    "mov rcx, r13",
    standard_protocols::default_raw_hash_value
);
thunk!(
    "eggos_swift_iterator_next",
    "mov x0, x20",
    "mov rdi, r13",
    characters::iterator_next
);
thunk!(
    "eggos_swift_bridge_any_object",
    "mov x1, x0\nmov x0, x8",
    "mov rsi, rdi\nmov rdi, rax",
    cast::bridge_any_object_to_any
);

macro_rules! four_words {
    ($name:literal, $target:path) => {
        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 2",
            concat!(".globl ", $name),
            concat!(".hidden ", $name),
            concat!($name, ":"),
            "stp x29, x30, [sp, #-48]!",
            "mov x29, sp",
            "mov x4, x3",
            "mov x3, x2",
            "mov x2, x1",
            "mov x1, x0",
            "add x0, sp, #16",
            "bl {target}",
            "ldp x0, x1, [sp, #16]",
            "ldp x2, x3, [sp, #32]",
            "ldp x29, x30, [sp], #48",
            "ret",
            target = sym $target,
        );
        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 4",
            concat!(".globl ", $name),
            concat!(".hidden ", $name),
            concat!($name, ":"),
            "push rbp",
            "mov rbp, rsp",
            "sub rsp, 32",
            "mov r8, rcx",
            "mov rcx, rdx",
            "mov rdx, rsi",
            "mov rsi, rdi",
            "mov rdi, rsp",
            "call {target}",
            "mov rax, qword ptr [rsp]",
            "mov rdx, qword ptr [rsp + 8]",
            "mov rcx, qword ptr [rsp + 16]",
            "mov r8, qword ptr [rsp + 24]",
            "mov rsp, rbp",
            "pop rbp",
            "ret",
            target = sym $target,
        );
    };
}

four_words!("eggos_swift_substring", characters::substring);

dispatch!(
    "eggos_swift_description_dispatch",
    "ldr x9, [x1, #8]",
    "jmp qword ptr [rsi + 8]"
);
dispatch!(
    "eggos_swift_lock_dispatch",
    "ldr x9, [x1, #8]",
    "jmp qword ptr [rsi + 8]"
);
dispatch!(
    "eggos_swift_unlock_dispatch",
    "ldr x9, [x1, #16]",
    "jmp qword ptr [rsi + 16]"
);
dispatch!(
    "eggos_swift_write_dispatch",
    "ldr x9, [x3, #24]",
    "jmp qword ptr [rcx + 24]"
);
dispatch!(
    "eggos_swift_write_ascii_dispatch",
    "ldr x9, [x3, #32]",
    "jmp qword ptr [rcx + 32]"
);
dispatch!(
    "eggos_swift_equal_dispatch",
    "ldr x9, [x3, #8]",
    "jmp qword ptr [rcx + 8]"
);
dispatch!(
    "eggos_swift_hash_value_dispatch",
    "ldr x9, [x1, #16]",
    "jmp qword ptr [rsi + 16]"
);
dispatch!(
    "eggos_swift_hash_into_dispatch",
    "ldr x9, [x2, #24]",
    "jmp qword ptr [rdx + 24]"
);
dispatch!(
    "eggos_swift_raw_hash_dispatch",
    "ldr x9, [x2, #32]",
    "jmp qword ptr [rdx + 32]"
);
dispatch!(
    "eggos_swift_raw_value_init_dispatch",
    "ldr x9, [x2, #16]",
    "jmp qword ptr [rdx + 16]"
);
dispatch!(
    "eggos_swift_raw_value_dispatch",
    "ldr x9, [x1, #24]",
    "jmp qword ptr [rsi + 24]"
);
dispatch!(
    "eggos_swift_all_cases_dispatch",
    "ldr x9, [x1, #24]",
    "jmp qword ptr [rsi + 24]"
);

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".text",
    ".p2align 2",
    ".globl eggos_swift_call_context",
    ".hidden eggos_swift_call_context",
    "eggos_swift_call_context:",
    "stp x29, x30, [sp, #-32]!",
    "mov x29, sp",
    "str x20, [sp, #16]",
    "mov x9, x0",
    "mov x20, x1",
    "mov x0, x2",
    "mov x1, x3",
    "mov x2, x4",
    "mov x3, x5",
    "blr x9",
    "ldr x20, [sp, #16]",
    "ldp x29, x30, [sp], #32",
    "ret",
);

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".text",
    ".p2align 4",
    ".globl eggos_swift_call_context",
    ".hidden eggos_swift_call_context",
    "eggos_swift_call_context:",
    "push rbp",
    "mov rbp, rsp",
    "push r13",
    "sub rsp, 8",
    "mov rax, rdi",
    "mov r13, rsi",
    "mov rdi, rdx",
    "mov rsi, rcx",
    "mov rdx, r8",
    "mov rcx, r9",
    "call rax",
    "add rsp, 8",
    "pop r13",
    "pop rbp",
    "ret",
);

unsafe extern "C" {
    pub fn eggos_swift_hasher_finalize();
    pub fn eggos_swift_hasher_combine();
    pub fn eggos_swift_hasher_combine8();
    pub fn eggos_swift_hasher_combine16();
    pub fn eggos_swift_hasher_combine32();
    pub fn eggos_swift_hasher_init();
    pub fn eggos_swift_string_append();
    pub fn eggos_swift_guts_grow();
    pub fn eggos_swift_write_ascii();
    pub fn eggos_swift_string_storage_destroy();
    pub fn eggos_swift_array_storage_destroy();
    pub fn eggos_swift_dictionary_storage_destroy();
    pub fn eggos_swift_dictionary_allocate();
    pub fn eggos_swift_dictionary_copy();
    pub fn eggos_swift_dictionary_resize();
    pub fn eggos_swift_array_subscript();
    pub fn eggos_swift_int_description();
    pub fn eggos_swift_int8_description();
    pub fn eggos_swift_int16_description();
    pub fn eggos_swift_int32_description();
    pub fn eggos_swift_uint_description();
    pub fn eggos_swift_uint8_description();
    pub fn eggos_swift_uint16_description();
    pub fn eggos_swift_uint32_description();
    pub fn eggos_swift_double_description();
    pub fn eggos_swift_bool_description();
    pub fn eggos_swift_string_description();
    pub fn eggos_swift_description_dispatch();
    pub fn eggos_swift_lock_dispatch();
    pub fn eggos_swift_unlock_dispatch();
    pub fn eggos_swift_write_dispatch();
    pub fn eggos_swift_write_ascii_dispatch();
    pub fn eggos_swift_string_hash_value();
    pub fn eggos_swift_string_hash_into();
    pub fn eggos_swift_string_raw_hash();
    pub fn eggos_swift_int_hash_value();
    pub fn eggos_swift_int_hash_into();
    pub fn eggos_swift_int_raw_hash();
    pub fn eggos_swift_default_hash_value();
    pub fn eggos_swift_default_raw_hash_value();
    pub fn eggos_swift_iterator_next();
    pub fn eggos_swift_bridge_any_object();
    pub fn eggos_swift_substring();
    pub fn eggos_swift_equal_dispatch();
    pub fn eggos_swift_hash_value_dispatch();
    pub fn eggos_swift_hash_into_dispatch();
    pub fn eggos_swift_raw_hash_dispatch();
    pub fn eggos_swift_raw_value_init_dispatch();
    pub fn eggos_swift_raw_value_dispatch();
    pub fn eggos_swift_all_cases_dispatch();
    fn eggos_swift_call_context(
        function: usize,
        context: usize,
        first: usize,
        second: usize,
        third: usize,
        fourth: usize,
    ) -> usize;
}

/// Calls the Swift function at `function` with `context` as its `self` register and up to four
/// word arguments, returning its first result register (meaningless for a function that
/// returns nothing).
///
/// # Safety
/// `function` must be a Swift function that takes a context and at most those four arguments
/// (more are ignored).
pub unsafe fn call_with_context(function: usize, context: usize, arguments: [usize; 4]) -> usize {
    // SAFETY: the caller's promise; the entry saves and restores the context register.
    unsafe {
        eggos_swift_call_context(
            function,
            context,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )
    }
}

/// The address of a function or entry point, for the export table and witness tables.
pub fn address(function: unsafe extern "C" fn()) -> u64 {
    function as usize as u64
}

/// Runs the destructor of the heap object `object` whose metadata is `class`: the function
/// in the word two before the metadata, which takes the object as its context.
pub fn destroy(object: usize, class: Metadata) {
    let destructor = metadata::word(class.wrapping_sub(16));
    // SAFETY: every heap metadata keeps its destructor there, a Swift function taking the
    // object as its context and returning nothing.
    unsafe { call_with_context(destructor, object, [0; 4]) };
}
