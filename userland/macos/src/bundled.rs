//! An app's own libraries (D409): frameworks and dylibs an app carries in its bundle (Sparkle
//! in `Contents/Frameworks`) and links by `@rpath/`, `@executable_path/` or `@loader_path/`.
//! `start` maps each like the executable (its own fixups, imports and Objective-C metadata)
//! before the executable is bound; this module keeps them, finds the file a linked path
//! names, looks symbols up in their export tries (following re-exports), and runs their
//! initializers, dependencies first, then the executable's, before `main`.

use crate::export_trie::{self, Symbol};
use crate::libsystem::Exclusive;
use crate::macho::{MAX_LIBRARIES, MachO, SectionRange};
use core::ffi::c_char;

/// The most libraries an app may carry.
pub const MOST_IMAGES: usize = 64;
/// The longest path a library is found at.
pub const LONGEST_PATH: usize = 1024;
const MOST_REEXPORT_DEPTH: usize = 8;
const EXECUTABLE_PREFIX: &[u8] = b"@executable_path/";
const LOADER_PREFIX: &[u8] = b"@loader_path/";
const RPATH_PREFIX: &[u8] = b"@rpath/";

/// A library mapped into the program.
#[derive(Debug, Clone, Copy)]
pub struct Image {
    /// The path it was linked by (`@rpath/Sparkle.framework/Versions/B/Sparkle`).
    pub linked: &'static [u8],
    /// Where its Mach-O header is.
    pub base: usize,
    pub macho: MachO<'static>,
    /// For each of its libraries (by ordinal, from 1), the library here, or `None` for one the
    /// layer provides.
    pub dependencies: [Option<usize>; MAX_LIBRARIES],
}

static IMAGES: Exclusive<[Option<Image>; MOST_IMAGES]> = Exclusive::new([None; MOST_IMAGES]);

/// The library linked as `linked`, if one is loaded.
pub fn find(linked: &[u8]) -> Option<usize> {
    IMAGES.with(|images| {
        images
            .iter()
            .position(|image| image.is_some_and(|image| image.linked == linked))
    })
}

/// Keeps `image`; its index, or `None` when the table is full.
pub fn add(image: Image) -> Option<usize> {
    IMAGES.with(|images| {
        let free = images.iter().position(Option::is_none)?;
        images[free] = Some(image);
        Some(free)
    })
}

fn image(index: usize) -> Option<Image> {
    IMAGES.with(|images| images.get(index).copied().flatten())
}

/// Where paths are searched from: the executable's folder, the folder of the image that
/// links the library, and the run paths, already expanded.
#[derive(Debug, Clone, Copy)]
pub struct Search<'s> {
    pub executable_folder: &'s [u8],
    pub loader_folder: &'s [u8],
    pub run_paths: &'s [&'s [u8]],
}

fn joined(parts: &[&[u8]], out: &mut [u8; LONGEST_PATH]) -> Option<usize> {
    let mut length = 0;
    for part in parts {
        let end = length.checked_add(part.len())?;
        if end >= LONGEST_PATH {
            return None;
        }
        out[length..end].copy_from_slice(part);
        length = end;
    }
    out[length] = 0;
    Some(length)
}

/// The folder part of `path` (without its last `/`), or `.`.
pub fn folder_of(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|byte| *byte == b'/') {
        Some(0) => b"/",
        Some(at) => &path[..at],
        None => b".",
    }
}

/// `path` with `@executable_path/` and `@loader_path/` replaced, into `out` (NUL-terminated);
/// its length. A path of another form is copied.
pub fn expand(path: &[u8], search: &Search<'_>, out: &mut [u8; LONGEST_PATH]) -> Option<usize> {
    if let Some(rest) = path.strip_prefix(EXECUTABLE_PREFIX) {
        return joined(&[search.executable_folder, b"/", rest], out);
    }
    if let Some(rest) = path.strip_prefix(LOADER_PREFIX) {
        return joined(&[search.loader_folder, b"/", rest], out);
    }
    joined(&[path], out)
}

/// Each place the linked path `linked` may be, in the order dyld tries them: for `@rpath/`
/// every run path, else the path expanded. `try_path` gets each (NUL-terminated) and says
/// whether it is the one.
pub fn locate(
    linked: &[u8],
    search: &Search<'_>,
    try_path: &mut dyn FnMut(&[u8]) -> bool,
) -> Option<([u8; LONGEST_PATH], usize)> {
    let mut out = [0u8; LONGEST_PATH];
    if let Some(rest) = linked.strip_prefix(RPATH_PREFIX) {
        for run_path in search.run_paths {
            let mut expanded = [0u8; LONGEST_PATH];
            let Some(base) = expand(run_path, search, &mut expanded) else {
                continue;
            };
            if let Some(length) = joined(&[&expanded[..base], b"/", rest], &mut out)
                && try_path(&out[..=length])
            {
                return Some((out, length));
            }
        }
        return None;
    }
    let length = expand(linked, search, &mut out)?;
    try_path(&out[..=length]).then_some((out, length))
}

fn trie_of(image: &Image) -> &'static [u8] {
    image
        .macho
        .exports
        .and_then(|range| image.macho.bytes.get(range.offset..range.offset + range.size))
        .unwrap_or(&[])
}

/// The address of `name` in library `index`, re-exports followed (to another of the app's
/// libraries, or to the layer through `layer`).
pub fn symbol(index: usize, name: &[u8], layer: &dyn Fn(&[u8]) -> Option<u64>) -> Option<u64> {
    symbol_at(index, name, layer, 0)
}

fn symbol_at(
    index: usize,
    name: &[u8],
    layer: &dyn Fn(&[u8]) -> Option<u64>,
    depth: usize,
) -> Option<u64> {
    if depth > MOST_REEXPORT_DEPTH {
        return None;
    }
    let found = image(index)?;
    let header = found.base as u64;
    match export_trie::lookup(trie_of(&found), name)? {
        Symbol::Offset(offset) => Some(header + offset),
        Symbol::Absolute(value) => Some(value),
        Symbol::Resolver { stub, .. } => Some(header + stub),
        Symbol::ThreadLocal(_) => None,
        Symbol::Reexport { ordinal, name: other } => {
            let wanted = if other.is_empty() { name } else { other };
            let slot = (ordinal as usize).checked_sub(1)?;
            match found.dependencies.get(slot).copied().flatten() {
                Some(next) => symbol_at(next, wanted, layer, depth + 1),
                None => layer(wanted),
            }
        }
    }
}

/// `name` in any of the app's libraries, for a flat-namespace import.
pub fn search_all(name: &[u8], layer: &dyn Fn(&[u8]) -> Option<u64>) -> Option<u64> {
    (0..MOST_IMAGES).find_map(|index| symbol(index, name, layer))
}

type Initializer = extern "C" fn(i32, *mut *mut c_char, *mut *mut c_char, *mut *mut c_char);

fn slid(range: SectionRange, macho: &MachO<'_>, base: usize) -> (usize, usize) {
    let start = base + (range.address - macho.base_address()) as usize;
    (start, range.size as usize)
}

/// Runs the initializers of the image `macho` mapped at `base`, as dyld does before `main`:
/// `__mod_init_func`'s pointers (rebased already) and `__init_offsets`' offsets.
///
/// # Safety
/// The image is mapped, bound, rebased and protected, and its initializers may run now.
pub unsafe fn run_initializers_of(
    macho: &MachO<'_>,
    base: usize,
    arguments: (i32, *mut *mut c_char, *mut *mut c_char, *mut *mut c_char),
) {
    let (count, argv, envp, apple) = arguments;
    if let Some(range) = macho.initializers {
        let (start, size) = slid(range, macho, base);
        for index in 0..size / 8 {
            // SAFETY: the section's pointers, in the mapped image, each an initializer.
            let function = unsafe { (start as *const usize).add(index).read() };
            if function != 0 {
                // SAFETY: as above: a function of the image, called as dyld calls it.
                let initializer: Initializer = unsafe { core::mem::transmute(function) };
                initializer(count, argv, envp, apple);
            }
        }
    }
    if let Some(range) = macho.initializer_offsets {
        let (start, size) = slid(range, macho, base);
        for index in 0..size / 4 {
            // SAFETY: the section's 32-bit offsets from the header, in the mapped image.
            let offset = unsafe { (start as *const u32).add(index).read() } as usize;
            // SAFETY: as above: a function of the image.
            let initializer: Initializer = unsafe { core::mem::transmute(base + offset) };
            initializer(count, argv, envp, apple);
        }
    }
}

/// Runs every library's initializers, in the order they were loaded (dependencies first).
///
/// # Safety
/// As [`run_initializers_of`], for every library.
pub unsafe fn run_initializers(
    arguments: (i32, *mut *mut c_char, *mut *mut c_char, *mut *mut c_char),
) {
    for index in 0..MOST_IMAGES {
        if let Some(found) = image(index) {
            // SAFETY: the caller's duty.
            unsafe { run_initializers_of(&found.macho, found.base, arguments) };
        }
    }
}
