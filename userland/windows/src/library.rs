//! The Windows libraries the layer provides and how an import's library name finds one: names
//! compare ignoring ASCII case, with or without `.dll`, and the API sets
//! (`api-ms-win-core-*`) resolve to kernel32's functions, where Windows forwards them too;
//! shell32 has only `CommandLineToArgvW`; user32 and gdi32 have what GUI programs' windows,
//! messages and painting need (D129); the C runtime of MSVC's `/MD` programs is
//! `vcruntime140` and the Universal C Runtime, `ucrtbase`, which programs import through the
//! API sets `api-ms-win-crt-*` (D145).

use crate::text::equal_ignoring_ascii_case;

/// A library whose functions the layer implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Library {
    Kernel32,
    Ntdll,
    Shell32,
    User32,
    Gdi32,
    VcRuntime,
    Ucrt,
}

/// One function a library exports: its name and its address in the layer.
#[derive(Debug, Clone, Copy)]
pub struct Export {
    pub name: &'static [u8],
    pub address: *const (),
}

// SAFETY: an export is a name and a function's address, both immutable for the program's life.
unsafe impl Sync for Export {}

/// The library `name` (an import's or `LoadLibrary`'s) means, if the layer has it.
pub fn library(name: &[u8]) -> Option<Library> {
    let name = strip_suffix_ignoring_case(name, b".dll");
    let is =
        |wanted: &[u8]| equal_ignoring_ascii_case(name.iter().copied(), wanted.iter().copied());
    if is(b"kernel32") || is(b"kernelbase") || starts_with_ignoring_case(name, b"api-ms-win-core-")
    {
        Some(Library::Kernel32)
    } else if is(b"ntdll") {
        Some(Library::Ntdll)
    } else if is(b"shell32") {
        Some(Library::Shell32)
    } else if is(b"user32") {
        Some(Library::User32)
    } else if is(b"gdi32") {
        Some(Library::Gdi32)
    } else if is(b"vcruntime140") || is(b"vcruntime140_1") {
        Some(Library::VcRuntime)
    } else if is(b"ucrtbase") || starts_with_ignoring_case(name, b"api-ms-win-crt-") {
        Some(Library::Ucrt)
    } else {
        None
    }
}

/// The address of `function` among `exports`, if it is there.
pub fn find(exports: &[&[Export]], function: &[u8]) -> Option<*const ()> {
    exports
        .iter()
        .flat_map(|list| list.iter())
        .find(|export| export.name == function)
        .map(|export| export.address)
}

fn strip_suffix_ignoring_case<'a>(name: &'a [u8], suffix: &[u8]) -> &'a [u8] {
    match name.len().checked_sub(suffix.len()) {
        Some(at) if name[at..].eq_ignore_ascii_case(suffix) => &name[..at],
        _ => name,
    }
}

fn starts_with_ignoring_case(name: &[u8], prefix: &[u8]) -> bool {
    name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_names_match_ignoring_case_and_the_dll_suffix() {
        for name in [&b"KERNEL32.dll"[..], b"kernel32", b"KernelBase.DLL"] {
            assert_eq!(library(name), Some(Library::Kernel32));
        }
        assert_eq!(
            library(b"api-ms-win-core-synch-l1-2-0.dll"),
            Some(Library::Kernel32)
        );
        assert_eq!(library(b"ntdll.dll"), Some(Library::Ntdll));
        assert_eq!(library(b"SHELL32.dll"), Some(Library::Shell32));
        assert_eq!(library(b"user32.dll"), Some(Library::User32));
        assert_eq!(library(b"GDI32.dll"), Some(Library::Gdi32));
        assert_eq!(library(b"VCRUNTIME140.dll"), Some(Library::VcRuntime));
        assert_eq!(library(b"vcruntime140_1.dll"), Some(Library::VcRuntime));
        assert_eq!(library(b"ucrtbase.dll"), Some(Library::Ucrt));
        assert_eq!(
            library(b"api-ms-win-crt-runtime-l1-1-0.dll"),
            Some(Library::Ucrt)
        );
        assert_eq!(library(b"API-MS-WIN-CRT-STDIO-L1-1-0"), Some(Library::Ucrt));
        for name in [
            &b"ws2_32.dll"[..],
            b"kernel32.dll.dll",
            b"",
            b".dll",
            b"api-ms-win-crt",
            b"vcruntime",
            b"msvcrt.dll",
        ] {
            assert_eq!(library(name), None);
        }
    }

    #[test]
    fn functions_are_found_by_exact_name_across_lists() {
        static FIRST: [Export; 1] = [Export {
            name: b"GetStdHandle",
            address: 0x10 as *const (),
        }];
        static SECOND: [Export; 1] = [Export {
            name: b"WriteFile",
            address: 0x20 as *const (),
        }];
        let lists: [&[Export]; 2] = [&FIRST, &SECOND];
        assert_eq!(find(&lists, b"WriteFile"), Some(0x20 as *const ()));
        assert_eq!(find(&lists, b"writefile"), None);
        assert_eq!(find(&lists, b"GetStdHandleA"), None);
    }
}
