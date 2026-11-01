//! The Windows libraries the layer provides and how an import's library name finds one: names
//! compare ignoring ASCII case, with or without `.dll`, and the API sets
//! (`api-ms-win-core-*`) resolve to kernel32's functions, where Windows forwards them too;
//! shell32 has the command line, folders, files and programs (D178); user32 and gdi32 have
//! what GUI programs' windows,
//! messages and painting need (D129), comdlg32 the file picker (D145) and comctl32
//! `InitCommonControls` (D174) and the common controls (D178); the C runtime of MSVC's `/MD`
//! programs is `vcruntime140` and the Universal C Runtime, `ucrtbase`, which programs import
//! through the API sets `api-ms-win-crt-*` (D146); GDI+, COM (`ole32`, `oleaut32`) and the
//! smaller system libraries a real app imports (D178) are the layer's too; programs built
//! on Rust's standard library also import `bcryptprimitives` and `userenv`, MinGW's programs
//! the old C runtime, `msvcrt`, and programs linked with the SDK's `ws2_32.lib` import Winsock
//! from `ws2_32` by ordinal (D189).

use crate::text::equal_ignoring_ascii_case;

/// A library whose functions the layer implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Library {
    Kernel32,
    Ntdll,
    Shell32,
    User32,
    Gdi32,
    Comdlg32,
    Comctl32,
    VcRuntime,
    Ucrt,
    GdiPlus,
    Ole32,
    OleAut32,
    Advapi32,
    Wsock32,
    Winmm,
    Version,
    Psapi,
    Wininet,
    Shlwapi,
    UxTheme,
    Dwmapi,
    BcryptPrimitives,
    Userenv,
    Msvcrt,
    Ws2_32,
}

impl Library {
    /// Every library, in a fixed order (a module handle is its place in it).
    pub const ALL: [Library; 25] = [
        Library::Kernel32,
        Library::Ntdll,
        Library::Shell32,
        Library::User32,
        Library::Gdi32,
        Library::Comdlg32,
        Library::Comctl32,
        Library::VcRuntime,
        Library::Ucrt,
        Library::GdiPlus,
        Library::Ole32,
        Library::OleAut32,
        Library::Advapi32,
        Library::Wsock32,
        Library::Winmm,
        Library::Version,
        Library::Psapi,
        Library::Wininet,
        Library::Shlwapi,
        Library::UxTheme,
        Library::Dwmapi,
        Library::BcryptPrimitives,
        Library::Userenv,
        Library::Msvcrt,
        Library::Ws2_32,
    ];

    /// Its name, as [`library`] finds it.
    pub const fn name(self) -> &'static [u8] {
        match self {
            Library::Kernel32 => b"kernel32",
            Library::Ntdll => b"ntdll",
            Library::Shell32 => b"shell32",
            Library::User32 => b"user32",
            Library::Gdi32 => b"gdi32",
            Library::Comdlg32 => b"comdlg32",
            Library::Comctl32 => b"comctl32",
            Library::VcRuntime => b"vcruntime140",
            Library::Ucrt => b"ucrtbase",
            Library::GdiPlus => b"gdiplus",
            Library::Ole32 => b"ole32",
            Library::OleAut32 => b"oleaut32",
            Library::Advapi32 => b"advapi32",
            Library::Wsock32 => b"wsock32",
            Library::Winmm => b"winmm",
            Library::Version => b"version",
            Library::Psapi => b"psapi",
            Library::Wininet => b"wininet",
            Library::Shlwapi => b"shlwapi",
            Library::UxTheme => b"uxtheme",
            Library::Dwmapi => b"dwmapi",
            Library::BcryptPrimitives => b"bcryptprimitives",
            Library::Userenv => b"userenv",
            Library::Msvcrt => b"msvcrt",
            Library::Ws2_32 => b"ws2_32",
        }
    }
}

const SMALLER_LIBRARIES: usize = 16;

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
    } else if is(b"comdlg32") {
        Some(Library::Comdlg32)
    } else if is(b"comctl32") {
        Some(Library::Comctl32)
    } else if is(b"vcruntime140") || is(b"vcruntime140_1") {
        Some(Library::VcRuntime)
    } else if is(b"ucrtbase") || starts_with_ignoring_case(name, b"api-ms-win-crt-") {
        Some(Library::Ucrt)
    } else {
        Library::ALL[Library::ALL.len() - SMALLER_LIBRARIES..]
            .iter()
            .copied()
            .find(|library| is(library.name()))
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
        assert_eq!(library(b"COMDLG32.dll"), Some(Library::Comdlg32));
        assert_eq!(library(b"VCRUNTIME140.dll"), Some(Library::VcRuntime));
        assert_eq!(library(b"vcruntime140_1.dll"), Some(Library::VcRuntime));
        assert_eq!(library(b"ucrtbase.dll"), Some(Library::Ucrt));
        assert_eq!(
            library(b"api-ms-win-crt-runtime-l1-1-0.dll"),
            Some(Library::Ucrt)
        );
        assert_eq!(library(b"API-MS-WIN-CRT-STDIO-L1-1-0"), Some(Library::Ucrt));
        assert_eq!(library(b"GDIPLUS.DLL"), Some(Library::GdiPlus));
        assert_eq!(library(b"ole32.dll"), Some(Library::Ole32));
        assert_eq!(library(b"OLEAUT32.dll"), Some(Library::OleAut32));
        assert_eq!(library(b"WSOCK32.dll"), Some(Library::Wsock32));
        assert_eq!(library(b"UxTheme.dll"), Some(Library::UxTheme));
        assert_eq!(library(b"PSAPI.DLL"), Some(Library::Psapi));
        assert_eq!(
            library(b"bcryptprimitives.dll"),
            Some(Library::BcryptPrimitives)
        );
        assert_eq!(library(b"USERENV.dll"), Some(Library::Userenv));
        assert_eq!(library(b"MSVCRT.dll"), Some(Library::Msvcrt));
        assert_eq!(library(b"msvcrt"), Some(Library::Msvcrt));
        assert_eq!(library(b"WS2_32.dll"), Some(Library::Ws2_32));
        assert_eq!(
            library(b"api-ms-win-core-synch-l1-2-0.dll"),
            Some(Library::Kernel32)
        );
        for library_of_all in Library::ALL {
            assert_eq!(library(library_of_all.name()), Some(library_of_all));
        }
        for name in [
            &b"kernel32.dll.dll"[..],
            b"",
            b".dll",
            b"api-ms-win-crt",
            b"vcruntime",
            b"msvcr100.dll",
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
