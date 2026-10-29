//! An installed app's own icon (D148, `docs/ux/app-icons-v0.3.md`): found in what was
//! installed (a Windows program's icon resources, a macOS bundle's `.icns` named by its
//! `Info.plist`, or a PNG beside the program), decoded once, scaled to the two sizes the shell
//! draws and kept as an [`OwnIcon`], which the installer writes as `app.icon`
//! ([`eggos_app_manifest::icon`]). Each source holds candidate images of several sizes; for
//! each size the exact one of the best kind is taken, else the smallest larger, else the
//! largest smaller, and one that fails to decode gives way to the next. Every step is "try,
//! else next": an app whose icon cannot be read has none, and its install goes on.

mod bitmap;
mod icns;
mod pe;
mod scale;
#[cfg(test)]
pub(crate) mod tests;

use crate::files::{self, Files, Kind, Path};
use crate::reader::{self, RangeBytes, Reader};
use core::cmp::Ordering;
use eggos_app_manifest::icon::{self as file, LARGE, LARGE_PIXELS, PIXELS, SMALL};
use eggos_png::SIGNATURE;
use eggos_png::rows::{RowDecoder, RowWorkspace};
use scale::Scaler;

/// The largest side of an image an icon is made from.
pub const MAX_SIDE: u32 = 1024;
/// The most candidate images one source offers.
pub const MAX_CANDIDATES: usize = 32;

/// The most bytes of an `Info.plist` read.
pub const PLIST_BYTES: usize = 16 * 1024;

const ICON_KEY: &[u8] = b"<key>CFBundleIconFile</key>";
const EXECUTABLE_KEY: &[u8] = b"<key>CFBundleExecutable</key>";
const STRING_START: &[u8] = b"<string>";
const STRING_END: &[u8] = b"</string>";
const ICNS_SUFFIX: &[u8] = b".icns";
const PNG_SUFFIX: &[u8] = b".png";
const BUNDLE_PROGRAMS: &[u8] = b"/Contents/MacOS/";
const RESOURCES: &[u8] = b"Contents/Resources";
const INFO_PLIST: &[u8] = b"Contents/Info.plist";
const APP_ICON: &[u8] = b"AppIcon.icns";
const BUNDLE_SUFFIX: &[u8] = b".app";

/// How a candidate image is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A PNG.
    Png,
    /// A Windows icon's bitmap entry.
    Bitmap,
    /// An `.icns` entry of red, green and blue planes, with its alpha mask's place.
    Planes { mask: Option<(u64, u64)> },
    /// An `.icns` entry of alpha, red, green and blue planes.
    Argb,
}

/// An image a source offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// Its width and height.
    pub side: u32,
    /// Its kind, best first: 0 for PNG and 32-bit, 1 for planes with alpha, 2 for colour
    /// planes with a mask, 3 for palettes and 1-bit masks.
    pub rank: u8,
    /// Where its data starts in the source.
    pub offset: u64,
    /// How many bytes its data has.
    pub length: u64,
    /// How it is stored.
    pub format: Format,
}

const NO_CANDIDATE: Candidate = Candidate {
    side: 0,
    rank: 0,
    offset: 0,
    length: 0,
    format: Format::Png,
};

/// The candidates of one source, at most [`MAX_CANDIDATES`].
#[derive(Debug, Clone, Copy)]
pub struct Candidates {
    items: [Candidate; MAX_CANDIDATES],
    count: usize,
}

impl Default for Candidates {
    fn default() -> Self {
        Self::new()
    }
}

impl Candidates {
    /// None yet.
    pub const fn new() -> Self {
        Self {
            items: [NO_CANDIDATE; MAX_CANDIDATES],
            count: 0,
        }
    }

    /// Adds `candidate`, square and from 1 to [`MAX_SIDE`]; later ones past the limit are left
    /// out.
    pub fn push(&mut self, candidate: Candidate) {
        if self.count < MAX_CANDIDATES && (1..=MAX_SIDE).contains(&candidate.side) {
            self.items[self.count] = candidate;
            self.count += 1;
        }
    }

    /// How many there are.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Each, in the order they were added.
    pub fn iter(&self) -> impl Iterator<Item = &Candidate> {
        self.items[..self.count].iter()
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = &mut Candidate> {
        self.items[..self.count].iter_mut()
    }
}

/// An app's own icon: the [`LARGE`] image then the [`SMALL`] one, premultiplied
/// `0xAARRGGBB`.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnIcon {
    pixels: [u32; PIXELS],
}

impl core::fmt::Debug for OwnIcon {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("OwnIcon").finish_non_exhaustive()
    }
}

impl Default for OwnIcon {
    fn default() -> Self {
        Self::new()
    }
}

impl OwnIcon {
    /// Fully transparent.
    pub const fn new() -> Self {
        Self {
            pixels: [0; PIXELS],
        }
    }

    /// The large image, row by row.
    pub fn large(&self) -> &[u32] {
        &self.pixels[..LARGE_PIXELS]
    }

    /// The small image, row by row.
    pub fn small(&self) -> &[u32] {
        &self.pixels[LARGE_PIXELS..]
    }

    /// The icon file's bytes ([`eggos_app_manifest::icon`]).
    pub fn file(&self) -> [u8; file::FILE_BYTES] {
        let mut bytes = [0u8; file::FILE_BYTES];
        file::write(&self.pixels, &mut bytes);
        bytes
    }
}

/// The memory making an icon takes: a PNG decoder's rows, the scaler's sums, a bitmap row and
/// its mask, and an `.icns` entry's packed and unpacked planes. All zero is its starting
/// state.
pub struct Workspace {
    rows: RowWorkspace,
    sums: [u32; LARGE_PIXELS * 4],
    line: [u8; bitmap::MAX_LINE],
    mask: [u8; bitmap::MAX_MASK_LINE],
    packed: [u8; icns::MAX_PACKED],
    planes: [u8; icns::MAX_PLANES],
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for Workspace {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Workspace").finish_non_exhaustive()
    }
}

impl Workspace {
    /// All zero.
    pub const fn new() -> Self {
        Self {
            rows: RowWorkspace::new(),
            sums: [0; LARGE_PIXELS * 4],
            line: [0; bitmap::MAX_LINE],
            mask: [0; bitmap::MAX_MASK_LINE],
            packed: [0; icns::MAX_PACKED],
            planes: [0; icns::MAX_PLANES],
        }
    }
}

/// The side of the square PNG at `offset`, from its header; `None` when there is no PNG there
/// or it is not square or larger than [`MAX_SIDE`].
pub(crate) fn png_side(reader: &mut impl Reader, offset: u64) -> Option<u32> {
    let mut head = [0u8; 24];
    if !reader.read_exact_at(offset, &mut head)
        || head[..8] != SIGNATURE
        || &head[12..16] != b"IHDR"
    {
        return None;
    }
    let width = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
    let height = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
    (width == height && (1..=MAX_SIDE).contains(&width)).then_some(width)
}

fn order_key(candidate: &Candidate, target: u32) -> (u8, u32, u8) {
    match candidate.side.cmp(&target) {
        Ordering::Equal => (0, 0, candidate.rank),
        Ordering::Greater => (1, candidate.side, candidate.rank),
        Ordering::Less => (2, u32::MAX - candidate.side, candidate.rank),
    }
}

/// The order `list`'s candidates are tried in for a `target` by `target` image: indexes into
/// [`Candidates::iter`], how many.
pub fn order(list: &Candidates, target: u32) -> ([usize; MAX_CANDIDATES], usize) {
    let mut order = [0usize; MAX_CANDIDATES];
    for (index, slot) in order.iter_mut().enumerate().take(list.len()) {
        *slot = index;
    }
    order[..list.len()]
        .sort_unstable_by_key(|&index| (order_key(&list.items[index], target), index));
    (order, list.len())
}

fn decode_png(
    reader: &mut impl Reader,
    candidate: &Candidate,
    target: usize,
    work: &mut Workspace,
    out: &mut [u32],
) -> bool {
    let end = candidate.offset + candidate.length;
    let mut source = RangeBytes::new(reader, candidate.offset, end);
    let Ok(decoder) = RowDecoder::open(&mut source) else {
        return false;
    };
    let header = decoder.header();
    if header.width != candidate.side || header.height != candidate.side {
        return false;
    }
    let mut scaler = Scaler::new(target, header.width as usize, &mut work.sums, out);
    if decoder
        .read_rows(&mut work.rows, |y, row| scaler.row(y as usize, row))
        .is_err()
    {
        return false;
    }
    scaler.finish();
    true
}

fn decode(
    reader: &mut impl Reader,
    candidate: &Candidate,
    target: usize,
    work: &mut Workspace,
    out: &mut [u32],
) -> bool {
    let side = candidate.side as usize;
    match candidate.format {
        Format::Png => decode_png(reader, candidate, target, work, out),
        Format::Bitmap => {
            let mut scaler = Scaler::new(target, side, &mut work.sums, out);
            let decoded = bitmap::decode(
                reader,
                candidate.offset,
                candidate.length,
                &mut work.line,
                &mut work.mask,
                |y, row| scaler.row(y, row),
            );
            scaler.finish();
            decoded
        }
        Format::Planes { .. } | Format::Argb => {
            let mut scaler = Scaler::new(target, side, &mut work.sums, out);
            let decoded = icns::decode(
                reader,
                candidate,
                &mut work.packed,
                &mut work.planes,
                |y, row| scaler.row(y, row),
            );
            scaler.finish();
            decoded
        }
    }
}

fn image(
    reader: &mut impl Reader,
    list: &Candidates,
    target: usize,
    work: &mut Workspace,
    out: &mut [u32],
) -> bool {
    let (order, count) = order(list, target as u32);
    order[..count]
        .iter()
        .any(|&index| decode(reader, &list.items[index], target, work, out))
}

/// Makes `icon` from `list`'s candidates in `reader`; false when no candidate decodes.
pub fn make(
    reader: &mut impl Reader,
    list: &Candidates,
    work: &mut Workspace,
    icon: &mut OwnIcon,
) -> bool {
    let (large, small) = icon.pixels.split_at_mut(LARGE_PIXELS);
    !list.is_empty()
        && image(reader, list, LARGE, work, large)
        && image(reader, list, SMALL, work, small)
}

/// Makes `icon` from the Windows program `reader` holds: its first icon group's images.
pub fn from_exe(reader: &mut impl Reader, work: &mut Workspace, icon: &mut OwnIcon) -> bool {
    let mut list = Candidates::new();
    pe::candidates(reader, &mut list);
    make(reader, &list, work, icon)
}

/// Makes `icon` from the `.icns` file `reader` holds.
pub fn from_icns(reader: &mut impl Reader, work: &mut Workspace, icon: &mut OwnIcon) -> bool {
    let mut list = Candidates::new();
    icns::candidates(reader, &mut list);
    make(reader, &list, work, icon)
}

/// Makes `icon` from the square PNG `reader` holds.
pub fn from_png(reader: &mut impl Reader, work: &mut Workspace, icon: &mut OwnIcon) -> bool {
    let mut list = Candidates::new();
    if let Some(side) = png_side(reader, 0) {
        list.push(Candidate {
            side,
            rank: 0,
            offset: 0,
            length: reader.size(),
            format: Format::Png,
        });
    }
    make(reader, &list, work, icon)
}

fn ends_with_ignoring_case(name: &[u8], suffix: &[u8]) -> bool {
    name.len() > suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

/// The `.app` bundle a program at `program` is the macOS program of, if it is one:
/// `<bundle>.app/Contents/MacOS/<file>`.
pub fn bundle_of(program: &[u8]) -> Option<&[u8]> {
    let at = program
        .windows(BUNDLE_PROGRAMS.len())
        .rposition(|window| window == BUNDLE_PROGRAMS)?;
    let bundle = &program[..at];
    let rest = &program[at + BUNDLE_PROGRAMS.len()..];
    (ends_with_ignoring_case(files::file_name(bundle), BUNDLE_SUFFIX) && !rest.contains(&b'/'))
        .then_some(bundle)
}

/// The value of `CFBundleIconFile` in an XML property list, if it is a plain file name.
pub fn plist_icon_file(plist: &[u8]) -> Option<&[u8]> {
    plist_file_name(plist, ICON_KEY)
}

/// The value of `CFBundleExecutable` in an XML property list, if it is a plain file name.
pub fn plist_executable(plist: &[u8]) -> Option<&[u8]> {
    plist_file_name(plist, EXECUTABLE_KEY)
}

fn plist_file_name<'p>(plist: &'p [u8], key_element: &[u8]) -> Option<&'p [u8]> {
    let key = plist
        .windows(key_element.len())
        .position(|window| window == key_element)?;
    let after = &plist[key + key_element.len()..];
    let start = after.iter().position(|byte| !byte.is_ascii_whitespace())?;
    let value = after[start..].strip_prefix(STRING_START)?;
    let end = value
        .windows(STRING_END.len())
        .position(|window| window == STRING_END)?;
    let name = value[..end].trim_ascii();
    let plain = !name.is_empty() && name != b".." && !name.contains(&b'/') && !name.contains(&0);
    plain.then_some(name)
}

fn is_file(files: &mut impl Files, path: &Path) -> bool {
    files
        .status(path.as_bytes())
        .is_ok_and(|status| status.kind == Kind::File)
}

fn read_start(files: &mut impl Files, path: &[u8], buffer: &mut [u8]) -> usize {
    reader::read_file(files, path, |reader| {
        let room = buffer.len().min(reader.size() as usize);
        match reader.read_exact_at(0, &mut buffer[..room]) {
            true => room,
            false => 0,
        }
    })
    .unwrap_or(0)
}

/// The `.icns` file of the bundle at `bundle`: the one its `Info.plist` names in
/// `Contents/Resources` (`.icns` added when the name lacks it), else `AppIcon.icns`, else
/// `<bundle name>.icns`, else the only `.icns` there.
pub fn bundle_icns(files: &mut impl Files, bundle: &[u8]) -> Option<Path> {
    let bundle = Path::from(bundle)?;
    let resources = bundle.join(RESOURCES)?;
    let mut plist = [0u8; PLIST_BYTES];
    let length = read_start(files, bundle.join(INFO_PLIST)?.as_bytes(), &mut plist);
    if let Some(name) = plist_icon_file(&plist[..length]) {
        let mut path = resources.join(name)?;
        if !ends_with_ignoring_case(name, ICNS_SUFFIX) && !path.append(ICNS_SUFFIX) {
            return None;
        }
        if is_file(files, &path) {
            return Some(path);
        }
    }
    let app_icon = resources.join(APP_ICON)?;
    if is_file(files, &app_icon) {
        return Some(app_icon);
    }
    let file_name = bundle.file_name();
    let stem = &file_name[..file_name.len().saturating_sub(BUNDLE_SUFFIX.len())];
    let mut named = resources.join(stem)?;
    if named.append(ICNS_SUFFIX) && is_file(files, &named) {
        return Some(named);
    }
    let mut name = [0u8; 256];
    let mut only = None;
    let mut index = 0;
    while let Ok(Some(length)) = files.entry(resources.as_bytes(), index, &mut name) {
        index += 1;
        if ends_with_ignoring_case(&name[..length], ICNS_SUFFIX) {
            if only.is_some() {
                return None;
            }
            only = resources.join(&name[..length]);
        }
    }
    only.filter(|path| is_file(files, path))
}

/// `<stem>.png` beside the program or bundle at `path`.
pub fn png_beside(path: &[u8]) -> Option<Path> {
    let file_name = files::file_name(path);
    let stem = match file_name.iter().rposition(|&byte| byte == b'.') {
        Some(dot) if dot > 0 => &file_name[..dot],
        _ => file_name,
    };
    let mut beside = Path::from(&path[..path.len() - file_name.len()])?;
    (beside.append(stem) && beside.append(PNG_SUFFIX)).then_some(beside)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IconFile {
    Program,
    Icns,
    Png,
}

fn from_file(
    files: &mut impl Files,
    path: &[u8],
    kind: IconFile,
    work: &mut Workspace,
    icon: &mut OwnIcon,
) -> bool {
    let made = reader::read_file(files, path, |reader| match kind {
        IconFile::Program => from_exe(reader, work, icon),
        IconFile::Icns => from_icns(reader, work, icon),
        IconFile::Png => from_png(reader, work, icon),
    });
    made == Some(true)
}

/// Makes `icon` for the program installed at `program` (absolute): a bundle's `.icns`
/// ([`bundle_icns`]) or a Windows program's icon resources, else `<stem>.png` beside the bundle
/// or the program, else `<stem>.png` beside `source` (the program or bundle installed from);
/// false when none gives an icon.
pub fn find(
    files: &mut impl Files,
    program: &[u8],
    source: Option<&[u8]>,
    work: &mut Workspace,
    icon: &mut OwnIcon,
) -> bool {
    let (own, beside) = match bundle_of(program) {
        Some(bundle) => (
            bundle_icns(files, bundle).map(|path| (path, IconFile::Icns)),
            bundle,
        ),
        None => (
            Path::from(program).map(|path| (path, IconFile::Program)),
            program,
        ),
    };
    if let Some((path, kind)) = own
        && from_file(files, path.as_bytes(), kind, work, icon)
    {
        return true;
    }
    [Some(beside), source]
        .into_iter()
        .flatten()
        .filter_map(png_beside)
        .any(|path| from_file(files, path.as_bytes(), IconFile::Png, work, icon))
}
