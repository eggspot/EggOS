//! Installing, uninstalling and listing apps (D134, docs/apps.md): an app is the folder
//! `/Applications/<name>/` holding its program (an `.app` bundle whole) and `app.manifest`,
//! which is written last, through a temporary file renamed over it, so a half-installed app has
//! no manifest and is not listed (D133); uninstall removes the manifest first, then the folder.
//! An installation is a sequence of small steps so the installer's window stays responsive and
//! can cancel between them. A `.zip` (D148) is told by its first bytes; its one program (a
//! `.exe`, the program of an `.app` bundle in it, or a static ELF), or the one `--entry` names,
//! is the app's, and the whole archive, `__MACOSX` metadata left out, is extracted into the
//! app's folder, one entry a step. An archive holding a file the installer writes itself
//! ([`RESERVED_NAMES`]) or more than [`MAX_EXTRACTED_BYTES`] once extracted is refused. After
//! the copy the app's own icon (D148) is made from what was installed and written as
//! `app.icon`, named by the manifest; an app without one installs all the same.

use crate::architecture::Architecture;
use crate::archive::{self, Archive, ArchiveError, Cursor, Entry};
use crate::files::{self, Files, Handle, Kind, PATH_CAPACITY, Path};
use crate::icon::{self, OwnIcon, Workspace};
use crate::program::{self, NotRunnable, Program};
use crate::reader::{self, FileReader, Reader};
use core::cell::RefCell;
use eggos_app_manifest::icon as icon_file;
use eggos_app_manifest::{self as manifest, Manifest, Source};
use eggos_commands::page::Smallest;
use eggos_png::inflate::{Sink, Window};
use eggos_runtime::errno::Errno;

/// Where apps are installed.
pub const APPLICATIONS: &[u8] = b"/Applications";
/// The Windows layer's program (D118).
pub const WINDOWS_LAYER: &[u8] = b"/usr/lib/eggos/windows";
/// The macOS layer's program (D120).
pub const MACOS_LAYER: &[u8] = b"/usr/lib/eggos/macos";
/// The manifest is written here first, then renamed over `app.manifest`.
pub const TEMPORARY_MANIFEST: &[u8] = b"app.manifest.tmp";
/// How deep folders inside an `.app` bundle may nest.
pub const MAX_DEPTH: usize = 16;
/// The files the installer writes in an app's folder, which no program and no entry of a
/// `.zip` may take.
pub const RESERVED_NAMES: [&[u8]; 3] = [
    manifest::FILE_NAME.as_bytes(),
    TEMPORARY_MANIFEST,
    icon_file::FILE_NAME.as_bytes(),
];
/// The most bytes a `.zip`'s files may take once extracted.
pub const MAX_EXTRACTED_BYTES: u64 = 4 << 30;

const MAX_BUNDLES: usize = 4;
const INFO_PLIST: &[u8] = b"Contents/Info.plist";
const INFO_PLIST_INSIDE: &[u8] = b"/Contents/Info.plist";
const BUNDLE_SUFFIX: &[u8] = b".app";
const BUNDLE_PROGRAMS: &[u8] = b"Contents/MacOS";
const MANIFEST_PERMISSIONS: u32 = 0o644;
const FILE_PERMISSIONS: u32 = 0o644;
const PROGRAM_PERMISSIONS: u32 = 0o755;
const WINDOWS_PROGRAM_SUFFIX: &[u8] = b".exe";
const BUNDLE_PROGRAMS_INSIDE: &[u8] = b"/Contents/MacOS/";
const ELF_MAGIC: &[u8] = b"\x7fELF";

/// Why an app could not be installed or uninstalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// The file is no program this machine runs.
    NotRunnable(NotRunnable),
    /// The file could not be read.
    Unreadable(Errno),
    /// A folder that is not an `.app` bundle.
    FolderNotApp,
    /// An `.app` bundle whose `Contents/MacOS` holds no program.
    BundleWithoutProgram,
    /// The app's name breaks the manifest's rules.
    InvalidName,
    /// The program's path inside the app's folder cannot be a manifest's `run=`.
    InvalidProgramPath,
    /// A path is longer than [`PATH_CAPACITY`].
    PathTooLong,
    /// An app of that name is installed.
    AlreadyInstalled,
    /// No app of that name is installed.
    NotInstalled,
    /// `/Applications` is read-only: EggOS started without a disk (D131).
    ReadOnly,
    /// The disk is full.
    DiskFull,
    /// Folders nest deeper than [`MAX_DEPTH`].
    TooDeep,
    /// A symbolic link, a device or anything else that is neither a file nor a folder.
    UnsupportedEntry,
    /// A `.zip` that cannot be read or extracted.
    Archive(ArchiveError),
    /// A `.zip` holding no program this installer recognises.
    NoProgramInArchive,
    /// A `.zip` holding several programs, listed, and no `--entry` choosing one.
    SeveralPrograms(Listing),
    /// No program in the `.zip` is at the path `--entry` gives.
    NoSuchEntry,
    /// A program, or a file in a `.zip`, named like a file the installer writes itself
    /// ([`RESERVED_NAMES`]).
    ReservedName,
    /// A `.zip` whose files take more than [`MAX_EXTRACTED_BYTES`] once extracted.
    TooLarge,
    /// Another error of the file system.
    Failed(Errno),
}

/// The programs a `.zip` holds, named in a short line for a message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Listing {
    bytes: [u8; Listing::CAPACITY],
    length: usize,
    cut: bool,
}

impl core::fmt::Debug for Listing {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl Listing {
    /// The longest listing, in bytes.
    pub const CAPACITY: usize = 120;
    const SEPARATOR: &[u8] = b", ";
    const ELLIPSIS: &[u8] = b"...";

    const fn new() -> Self {
        Self {
            bytes: [0; Self::CAPACITY],
            length: 0,
            cut: false,
        }
    }

    fn append(&mut self, bytes: &[u8]) {
        let end = (self.length + bytes.len()).min(Self::CAPACITY);
        self.bytes[self.length..end].copy_from_slice(&bytes[..end - self.length]);
        self.length = end;
    }

    fn add(&mut self, name: &[u8]) {
        if self.cut {
            return;
        }
        let separator = match self.length {
            0 => &b""[..],
            _ => Self::SEPARATOR,
        };
        let printable = name.iter().all(|&byte| matches!(byte, b' '..=b'~'));
        let needed = separator.len() + name.len() + Self::SEPARATOR.len() + Self::ELLIPSIS.len();
        self.append(separator);
        match printable && self.length + needed - separator.len() <= Self::CAPACITY {
            true => self.append(name),
            false => {
                self.append(Self::ELLIPSIS);
                self.cut = true;
            }
        }
    }

    /// The names, separated by commas.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

impl Problem {
    fn from_errno(errno: Errno) -> Self {
        match errno {
            Errno::EROFS => Self::ReadOnly,
            Errno::ENOSPC => Self::DiskFull,
            other => Self::Failed(other),
        }
    }
}

/// The layer `source`'s programs run through, if any.
pub fn layer_path(source: Source) -> Option<&'static [u8]> {
    match source {
        Source::Windows => Some(WINDOWS_LAYER),
        Source::Macos => Some(MACOS_LAYER),
        Source::Linux => None,
    }
}

/// A name as typed or derived, kept for messages even when it is no valid app name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; Name::CAPACITY],
    length: usize,
}

impl core::fmt::Debug for Name {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.as_bytes().escape_ascii(), formatter)
    }
}

impl Name {
    /// The most bytes a name keeps.
    pub const CAPACITY: usize = 64;

    /// `bytes`, its first [`Self::CAPACITY`] bytes kept: for messages.
    pub fn new(bytes: &[u8]) -> Self {
        let length = bytes.len().min(Self::CAPACITY);
        let mut name = Self {
            bytes: [0; Self::CAPACITY],
            length,
        };
        name.bytes[..length].copy_from_slice(&bytes[..length]);
        name
    }

    /// `bytes` whole; `None` when they do not fit, so a name that names a file is never cut.
    pub fn whole(bytes: &[u8]) -> Option<Self> {
        (bytes.len() <= Self::CAPACITY).then(|| Self::new(bytes))
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// The name as text: the bytes when they are UTF-8, else `?`.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("?")
    }

    /// Whether it may name an app ([`manifest::is_name`]).
    pub fn is_valid(&self) -> bool {
        self.length <= manifest::NAME_CAPACITY && manifest::is_name(self.as_bytes())
    }
}

/// The app name a program or bundle at `path` gets: its file name without its extension
/// (`win-paint.exe` gives `win-paint`, `Paint.app` gives `Paint`).
pub fn derived_name(path: &[u8]) -> Name {
    let file = files::file_name(path);
    let stem = match file.iter().rposition(|&byte| byte == b'.') {
        Some(dot) if dot > 0 => &file[..dot],
        _ => file,
    };
    Name::new(stem)
}

/// Whether `file_name` names an `.app` bundle: it ends with `.app`, in any case, after
/// something.
pub fn is_bundle_name(file_name: &[u8]) -> bool {
    file_name.len() > BUNDLE_SUFFIX.len()
        && file_name[file_name.len() - BUNDLE_SUFFIX.len()..].eq_ignore_ascii_case(BUNDLE_SUFFIX)
}

/// Why installing failed, with the name it was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused {
    pub name: Name,
    pub problem: Problem,
}

/// What installing a program will do, checked before anything is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    name: Name,
    program: Program,
    source: Path,
    bundle: bool,
    archive: Option<Archive>,
    run: Path,
    leftover: bool,
    total_bytes: u64,
}

impl Plan {
    /// The app's name.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// The program: its system and CPU.
    pub fn program(&self) -> Program {
        self.program
    }

    /// The file, `.app` folder or `.zip` installed.
    pub fn source(&self) -> &[u8] {
        self.source.as_bytes()
    }

    /// Whether it installs a `.zip`'s contents.
    pub fn is_archive(&self) -> bool {
        self.archive.is_some()
    }

    /// The program inside the app's folder: the manifest's `run=`.
    pub fn run(&self) -> &[u8] {
        self.run.as_bytes()
    }

    /// The app's folder.
    pub fn folder(&self) -> Path {
        app_folder(self.name.as_bytes()).unwrap_or_default()
    }

    /// The bytes the copy writes.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

fn app_folder(name: &[u8]) -> Option<Path> {
    Path::from(APPLICATIONS)?.join(name)
}

fn read_head(files: &mut impl Files, path: &[u8], head: &mut [u8]) -> Result<usize, Errno> {
    let file = files.open(path)?;
    let mut length = 0;
    let result = loop {
        if length == head.len() {
            break Ok(length);
        }
        match files.read(file, &mut head[length..]) {
            Ok(0) => break Ok(length),
            Ok(read) => length += read,
            Err(error) => break Err(error),
        }
    };
    files.close(file);
    result
}

/// The manifest of the app in folder `name` under `/Applications`, read into `file`; its
/// length when it is a valid manifest of that folder.
pub fn read_manifest(
    files: &mut impl Files,
    name: &[u8],
    file: &mut [u8; manifest::LARGEST + 1],
) -> Option<usize> {
    let path = app_folder(name)?.join(manifest::FILE_NAME.as_bytes())?;
    let length = read_head(files, path.as_bytes(), file).ok()?;
    manifest::parse(&file[..length], name).map(|_| length)
}

fn is_file(files: &mut impl Files, path: &Path) -> bool {
    files
        .status(path.as_bytes())
        .is_ok_and(|status| status.kind == Kind::File)
}

fn declared_program(files: &mut impl Files, bundle: &Path, programs: &Path) -> Option<Path> {
    let plist_path = bundle.join(INFO_PLIST)?;
    let mut plist = [0u8; icon::PLIST_BYTES];
    let length = icon::read_start(files, plist_path.as_bytes(), &mut plist);
    let mut executable = [0u8; icon::PLIST_NAME_BYTES];
    let program = programs.join(icon::plist_executable(&plist[..length], &mut executable)?)?;
    is_file(files, &program).then_some(program)
}

fn bundle_program(files: &mut impl Files, bundle: &Path, stem: &[u8]) -> Result<Path, Problem> {
    let programs = bundle.join(BUNDLE_PROGRAMS).ok_or(Problem::PathTooLong)?;
    if let Some(declared) = declared_program(files, bundle, &programs) {
        return Ok(declared);
    }
    let named = programs.join(stem).ok_or(Problem::PathTooLong)?;
    if is_file(files, &named) {
        return Ok(named);
    }
    let mut name = [0u8; 256];
    let mut found = None;
    let mut index = 0;
    while let Some(length) = files
        .entry(programs.as_bytes(), index, &mut name)
        .map_err(|_| Problem::BundleWithoutProgram)?
    {
        index += 1;
        let candidate = programs.join(&name[..length]).ok_or(Problem::PathTooLong)?;
        if is_file(files, &candidate) {
            if found.is_some() {
                return Err(Problem::BundleWithoutProgram);
            }
            found = Some(candidate);
        }
    }
    found.ok_or(Problem::BundleWithoutProgram)
}

fn measure(files: &mut impl Files, root: &Path) -> Result<u64, Problem> {
    let mut walk = Walk::new();
    let mut total = 0u64;
    while let Some((_, status)) = walk.next(files, root)? {
        match status.kind {
            Kind::File => total = total.saturating_add(status.size),
            Kind::Directory => {}
            Kind::Other => return Err(Problem::UnsupportedEntry),
        }
    }
    Ok(total)
}

fn without_trailing_slashes(path: &[u8]) -> &[u8] {
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    &path[..end]
}

/// Checks that the file, `.app` folder or `.zip` at `source` can be installed on `machine` as
/// `name` (else the name its file name, or in a `.zip` its program's, gives) and works out how.
pub fn plan(
    files: &mut impl Files,
    source: &[u8],
    name: Option<&[u8]>,
    machine: Architecture,
) -> Result<Plan, Refused> {
    plan_entry(files, source, name, None, machine)
}

/// [`plan`], with the program a `.zip` holds chosen by `entry`, its path inside the archive (an
/// `.app` bundle's own path also names its program); `entry` is ignored for anything else.
pub fn plan_entry(
    files: &mut impl Files,
    source: &[u8],
    name: Option<&[u8]>,
    entry: Option<&[u8]>,
    machine: Architecture,
) -> Result<Plan, Refused> {
    let source = without_trailing_slashes(source);
    let file_name = files::file_name(source);
    let is_bundle_name = is_bundle_name(file_name);
    let given = name;
    let name = name.map_or_else(|| derived_name(source), Name::new);
    let refused = |problem| Refused { name, problem };
    let source = Path::from(source).ok_or(refused(Problem::PathTooLong))?;
    let status = files
        .status(source.as_bytes())
        .map_err(|error| refused(Problem::Unreadable(error)))?;
    let (bundle, program_path) = match status.kind {
        Kind::Directory if is_bundle_name => {
            let stem = &file_name[..file_name.len() - BUNDLE_SUFFIX.len()];
            (true, bundle_program(files, &source, stem).map_err(refused)?)
        }
        Kind::Directory => return Err(refused(Problem::FolderNotApp)),
        Kind::File => (false, source),
        Kind::Other => return Err(refused(Problem::NotRunnable(NotRunnable::NotAProgram))),
    };
    let mut head = [0u8; program::HEAD_BYTES];
    let length = read_head(files, program_path.as_bytes(), &mut head)
        .map_err(|error| refused(Problem::Unreadable(error)))?;
    if !bundle && archive::is_archive(&head[..length]) {
        return plan_archive(files, source, given, entry, machine);
    }
    let program = program::recognise(&head[..length], machine)
        .map_err(|reason| refused(Problem::NotRunnable(reason)))?;
    if bundle && program.source != Source::Macos {
        return Err(refused(Problem::NotRunnable(NotRunnable::NotAProgram)));
    }
    let run = match bundle {
        true => {
            let inside = &program_path.as_bytes()[source.as_bytes().len()..];
            let mut run = Path::new();
            if !run.append(file_name) || !run.append(inside) {
                return Err(refused(Problem::PathTooLong));
            }
            run
        }
        false => Path::from(file_name).ok_or(refused(Problem::PathTooLong))?,
    };
    let leftover = settle(files, program, &name, &run).map_err(refused)?;
    let total_bytes = match bundle {
        true => measure(files, &source).map_err(refused)?,
        false => status.size,
    };
    Ok(Plan {
        name,
        program,
        source,
        bundle,
        archive: None,
        run,
        leftover,
        total_bytes,
    })
}

fn settle(
    files: &mut impl Files,
    program: Program,
    name: &Name,
    run: &Path,
) -> Result<bool, Problem> {
    if let Some(layer) = layer_path(program.source) {
        let installed = files
            .status(layer)
            .is_ok_and(|status| status.kind == Kind::File);
        if !installed {
            return Err(Problem::NotRunnable(NotRunnable::LayerMissing(
                program.source,
            )));
        }
    }
    if !name.is_valid() {
        return Err(Problem::InvalidName);
    }
    if !manifest::is_run(run.as_bytes()) {
        return Err(Problem::InvalidProgramPath);
    }
    if is_reserved(run.as_bytes()) {
        return Err(Problem::ReservedName);
    }
    let folder = app_folder(name.as_bytes()).ok_or(Problem::PathTooLong)?;
    if folder.join(run.as_bytes()).is_none() {
        return Err(Problem::PathTooLong);
    }
    match files.status(folder.as_bytes()) {
        Ok(_) => {
            let mut file = [0u8; manifest::LARGEST + 1];
            match read_manifest(files, name.as_bytes(), &mut file) {
                Some(_) => Err(Problem::AlreadyInstalled),
                None => Ok(true),
            }
        }
        Err(_) => Ok(false),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    BundleProgram { bundle_end: usize },
    WindowsProgram,
    MaybeLinuxProgram,
    Other,
}

/// Whether `name`, a path inside an app's folder, would take a file the installer writes: its
/// first part is one of [`RESERVED_NAMES`], in any case.
pub fn is_reserved(name: &[u8]) -> bool {
    let first = name.split(|&byte| byte == b'/').next().unwrap_or(name);
    RESERVED_NAMES
        .iter()
        .any(|reserved| first.eq_ignore_ascii_case(reserved))
}

fn bundle_end(name: &[u8]) -> Option<usize> {
    let mut start = 0;
    for part in name.split(|&byte| byte == b'/') {
        let end = start + part.len();
        if is_bundle_name(part) {
            return Some(end);
        }
        start = end + 1;
    }
    None
}

fn bundle_stem(bundle: &[u8]) -> &[u8] {
    let part = files::file_name(bundle);
    &part[..part.len() - BUNDLE_SUFFIX.len()]
}

#[derive(Clone, Copy)]
struct ZippedBundle {
    bundle: Path,
    declared: Option<Path>,
    declared_found: bool,
    stem: Option<Path>,
    first: Option<Path>,
    files: usize,
}

impl ZippedBundle {
    const EMPTY: Self = Self {
        bundle: Path::new(),
        declared: None,
        declared_found: false,
        stem: None,
        first: None,
        files: 0,
    };

    fn program(&self) -> Option<&[u8]> {
        let declared = self.declared.as_ref().filter(|_| self.declared_found);
        let only = self.first.as_ref().filter(|_| self.files == 1);
        declared.or(self.stem.as_ref()).or(only).map(Path::as_bytes)
    }
}

struct ZippedBundles {
    bundles: [ZippedBundle; MAX_BUNDLES],
    count: usize,
}

impl ZippedBundles {
    fn new() -> Self {
        Self {
            bundles: [ZippedBundle::EMPTY; MAX_BUNDLES],
            count: 0,
        }
    }

    fn of(&self, bundle: &[u8]) -> Option<&ZippedBundle> {
        self.bundles[..self.count]
            .iter()
            .find(|found| found.bundle.as_bytes() == bundle)
    }

    fn find_or_add(&mut self, bundle: &[u8]) -> Option<&mut ZippedBundle> {
        let at = match self.bundles[..self.count]
            .iter()
            .position(|found| found.bundle.as_bytes() == bundle)
        {
            Some(at) => at,
            None if self.count < MAX_BUNDLES => {
                self.bundles[self.count] = ZippedBundle {
                    bundle: Path::from(bundle)?,
                    ..ZippedBundle::EMPTY
                };
                self.count += 1;
                self.count - 1
            }
            None => return None,
        };
        Some(&mut self.bundles[at])
    }

    fn any_declared(&self) -> bool {
        self.bundles[..self.count]
            .iter()
            .any(|found| found.declared.is_some())
    }

    fn mark_declared_found(&mut self, name: &[u8]) {
        for found in &mut self.bundles[..self.count] {
            if found
                .declared
                .as_ref()
                .is_some_and(|declared| declared.as_bytes() == name)
            {
                found.declared_found = true;
            }
        }
    }
}

fn declared_in(plist: &[u8], bundle: &[u8]) -> Option<Path> {
    let mut executable = [0u8; icon::PLIST_NAME_BYTES];
    let executable = icon::plist_executable(plist, &mut executable)?;
    let mut program = Path::from(bundle)?;
    (program.append(BUNDLE_PROGRAMS_INSIDE) && program.append(executable)).then_some(program)
}

fn zipped_bundles(
    zip: &mut impl Reader,
    archive: &Archive,
    window: &mut Window,
) -> Result<ZippedBundles, Problem> {
    let mut bundles = ZippedBundles::new();
    let mut plist = [0u8; icon::PLIST_BYTES];
    let mut cursor = archive.first();
    while let Some(entry) = archive.next(zip, &mut cursor).map_err(Problem::Archive)? {
        let name = entry.name();
        let Some(end) = bundle_end(name) else {
            continue;
        };
        if entry.is_metadata() || entry.is_directory() {
            continue;
        }
        let (bundle, rest) = name.split_at(end);
        let Some(found) = bundles.find_or_add(bundle) else {
            continue;
        };
        if rest == INFO_PLIST_INSIDE {
            if let Ok(length) = archive::head(zip, &entry, window, &mut plist) {
                found.declared = declared_in(&plist[..length], bundle);
            }
            continue;
        }
        let Some(file) = rest.strip_prefix(BUNDLE_PROGRAMS_INSIDE) else {
            continue;
        };
        if file.contains(&b'/') {
            continue;
        }
        found.files += 1;
        if found.first.is_none() {
            found.first = Path::from(name);
        }
        if file == bundle_stem(bundle) {
            found.stem = Path::from(name);
        }
    }
    if bundles.any_declared() {
        let mut cursor = archive.first();
        while let Some(entry) = archive.next(zip, &mut cursor).map_err(Problem::Archive)? {
            if !entry.is_metadata() && !entry.is_directory() {
                bundles.mark_declared_found(entry.name());
            }
        }
    }
    Ok(bundles)
}

fn role(name: &[u8], bundles: &ZippedBundles) -> Role {
    if let Some(end) = bundle_end(name) {
        let bundle = &name[..end];
        let is_program = match bundles.of(bundle) {
            Some(found) => found.program() == Some(name),
            None => name[end..].strip_prefix(BUNDLE_PROGRAMS_INSIDE) == Some(bundle_stem(bundle)),
        };
        return match is_program {
            true => Role::BundleProgram { bundle_end: end },
            false => Role::Other,
        };
    }
    let file_name = files::file_name(name);
    let windows = file_name.len() > WINDOWS_PROGRAM_SUFFIX.len()
        && file_name[file_name.len() - WINDOWS_PROGRAM_SUFFIX.len()..]
            .eq_ignore_ascii_case(WINDOWS_PROGRAM_SUFFIX);
    match (windows, file_name.contains(&b'.')) {
        (true, _) => Role::WindowsProgram,
        (false, false) => Role::MaybeLinuxProgram,
        (false, true) => Role::Other,
    }
}

struct Scanned {
    archive: Archive,
    run: Path,
    named_after: Path,
    program: Program,
    total: u64,
}

fn program_role(
    zip: &mut impl Reader,
    entry: &Entry,
    window: &mut Window,
    bundles: &ZippedBundles,
) -> Option<Role> {
    if entry.is_metadata() || entry.is_directory() {
        return None;
    }
    let role = role(entry.name(), bundles);
    let is_program = match role {
        Role::BundleProgram { .. } | Role::WindowsProgram => true,
        Role::MaybeLinuxProgram => {
            let mut head = [0u8; 4];
            archive::head(zip, entry, window, &mut head) == Ok(head.len()) && head[..] == *ELF_MAGIC
        }
        Role::Other => false,
    };
    is_program.then_some(role)
}

fn shown_name(name: &[u8], role: Role) -> &[u8] {
    match role {
        Role::BundleProgram { bundle_end } => &name[..bundle_end],
        _ => name,
    }
}

fn scan(
    zip: &mut impl Reader,
    wanted: Option<&[u8]>,
    window: &mut Window,
    machine: Architecture,
) -> Result<Scanned, Problem> {
    let archive = Archive::open(zip).map_err(Problem::Archive)?;
    let bundles = zipped_bundles(zip, &archive, window)?;
    let mut cursor = archive.first();
    let mut chosen: Option<(Entry, Role)> = None;
    let mut found = 0;
    let mut listing = Listing::new();
    let mut total = 0u64;
    while let Some(entry) = archive.next(zip, &mut cursor).map_err(Problem::Archive)? {
        if is_reserved(entry.name()) {
            return Err(Problem::ReservedName);
        }
        if entry.is_metadata() || entry.is_directory() {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED_BYTES {
            return Err(Problem::TooLarge);
        }
        let Some(role) = program_role(zip, &entry, window, &bundles) else {
            continue;
        };
        let name = entry.name();
        let shown = shown_name(name, role);
        if wanted.is_some_and(|wanted| wanted != name && wanted != shown) {
            continue;
        }
        found += 1;
        listing.add(shown);
        if chosen.is_none() {
            chosen = Some((entry, role));
        }
    }
    let (entry, role) = match (found, chosen) {
        (1, Some(chosen)) => chosen,
        (0, _) if wanted.is_some() => return Err(Problem::NoSuchEntry),
        (0, _) => return Err(Problem::NoProgramInArchive),
        _ => return Err(Problem::SeveralPrograms(listing)),
    };
    let mut head = [0u8; program::HEAD_BYTES];
    let length = archive::head(zip, &entry, window, &mut head).map_err(Problem::Archive)?;
    let program = program::recognise(&head[..length], machine).map_err(Problem::NotRunnable)?;
    let named_after = match role {
        Role::BundleProgram { bundle_end } if program.source == Source::Macos => {
            &entry.name()[..bundle_end]
        }
        Role::BundleProgram { .. } => {
            return Err(Problem::NotRunnable(NotRunnable::NotAProgram));
        }
        _ => entry.name(),
    };
    Ok(Scanned {
        archive,
        run: Path::from(entry.name()).ok_or(Problem::PathTooLong)?,
        named_after: Path::from(named_after).ok_or(Problem::PathTooLong)?,
        program,
        total,
    })
}

fn plan_archive(
    files: &mut impl Files,
    source: Path,
    name: Option<&[u8]>,
    entry: Option<&[u8]>,
    machine: Architecture,
) -> Result<Plan, Refused> {
    let fallback = name.map_or_else(|| derived_name(source.as_bytes()), Name::new);
    let mut window = Window::new();
    let scanned = reader::read_file(files, source.as_bytes(), |zip| {
        scan(zip, entry, &mut window, machine)
    })
    .unwrap_or(Err(Problem::Unreadable(Errno::EIO)))
    .map_err(|problem| Refused {
        name: fallback,
        problem,
    })?;
    let name = name.map_or_else(|| derived_name(scanned.named_after.as_bytes()), Name::new);
    let leftover = settle(files, scanned.program, &name, &scanned.run)
        .map_err(|problem| Refused { name, problem })?;
    Ok(Plan {
        name,
        program: scanned.program,
        source,
        bundle: false,
        archive: Some(scanned.archive),
        run: scanned.run,
        leftover,
        total_bytes: scanned.total,
    })
}

/// The programs the `.zip` at `source` holds, each as `--entry` takes it (a bundle by its own
/// path), in the archive's order: the first `entries.len()` are put in `entries`, whole; how
/// many were put and how many others there are. Nothing for what is no `.zip`.
pub fn archive_programs(
    files: &mut impl Files,
    source: &[u8],
    entries: &mut [Path],
) -> (usize, usize) {
    let mut window = Window::new();
    reader::read_file(files, source, |zip| {
        let Ok(archive) = Archive::open(zip) else {
            return (0, 0);
        };
        let bundles =
            zipped_bundles(zip, &archive, &mut window).unwrap_or_else(|_| ZippedBundles::new());
        let mut cursor = archive.first();
        let (mut count, mut more) = (0, 0);
        while let Ok(Some(entry)) = archive.next(zip, &mut cursor) {
            let Some(role) = program_role(zip, &entry, &mut window, &bundles) else {
                continue;
            };
            let path = Path::from(shown_name(entry.name(), role));
            match (entries.get_mut(count), path) {
                (Some(slot), Some(path)) => {
                    *slot = path;
                    count += 1;
                }
                _ => more += 1,
            }
        }
        (count, more)
    })
    .unwrap_or((0, 0))
}

#[derive(Debug, Clone, Copy)]
struct Walk {
    relative: Path,
    indexes: [usize; MAX_DEPTH],
    depth: usize,
}

impl Walk {
    fn new() -> Self {
        Self {
            relative: Path::new(),
            indexes: [0; MAX_DEPTH],
            depth: 0,
        }
    }

    fn under(root: &Path, relative: &Path) -> Result<Path, Problem> {
        match relative.is_empty() {
            true => Ok(*root),
            false => root.join(relative.as_bytes()).ok_or(Problem::PathTooLong),
        }
    }

    fn next(
        &mut self,
        files: &mut impl Files,
        root: &Path,
    ) -> Result<Option<(Path, files::Status)>, Problem> {
        let mut name = [0u8; 256];
        loop {
            let directory = Self::under(root, &self.relative)?;
            let entry = files
                .entry(directory.as_bytes(), self.indexes[self.depth], &mut name)
                .map_err(Problem::Unreadable)?;
            let Some(length) = entry else {
                if self.depth == 0 {
                    return Ok(None);
                }
                self.relative.pop();
                self.depth -= 1;
                continue;
            };
            self.indexes[self.depth] += 1;
            let mut relative = self.relative;
            if !relative.push(&name[..length]) {
                return Err(Problem::PathTooLong);
            }
            let status = files
                .status(Self::under(root, &relative)?.as_bytes())
                .map_err(Problem::Unreadable)?;
            if status.kind == Kind::Directory {
                if self.depth + 1 >= MAX_DEPTH {
                    return Err(Problem::TooDeep);
                }
                self.depth += 1;
                self.indexes[self.depth] = 0;
                self.relative = relative;
            }
            return Ok(Some((relative, status)));
        }
    }
}

/// Removes `path` and, when it is a folder, everything in it, one entry at a time from the
/// first, so a file system whose listing moves under removals still loses every entry.
pub fn remove_tree(files: &mut impl Files, path: &[u8]) -> Result<(), Errno> {
    let root = Path::from(path).ok_or(Errno::ENAMETOOLONG)?;
    if files.status(root.as_bytes())?.kind != Kind::Directory {
        return files.remove_file(root.as_bytes());
    }
    let mut current = root;
    let mut name = [0u8; 256];
    loop {
        match files.entry(current.as_bytes(), 0, &mut name)? {
            Some(length) => {
                let child = current.join(&name[..length]).ok_or(Errno::ENAMETOOLONG)?;
                match files.status(child.as_bytes())?.kind {
                    Kind::Directory => current = child,
                    _ => files.remove_file(child.as_bytes())?,
                }
            }
            None => {
                files.remove_directory(current.as_bytes())?;
                if current == root {
                    return Ok(());
                }
                current.pop();
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Preparing,
    Copying,
    Icon,
    Registering,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenCopy {
    from: Handle,
    to: Handle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenArchive {
    file: Handle,
    cursor: Cursor,
}

/// Where an installation is after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Copying the program: this many of the plan's bytes are written.
    Copying(u64),
    /// Making the app's icon and writing the manifest next.
    Registering,
    /// Done: the app is installed.
    Installed,
    /// It failed; nothing of it is left.
    Failed(Problem),
}

/// An installation under way. See the module documentation.
#[derive(Debug)]
pub struct Installation {
    plan: Plan,
    stage: Stage,
    walk: Walk,
    copy: Option<OpenCopy>,
    extracting: Option<OpenArchive>,
    single_copied: bool,
    done_bytes: u64,
    icon: bool,
}

struct SharedFile<'a, 'f, F: Files> {
    files: &'a RefCell<&'f mut F>,
    file: Handle,
    size: u64,
    error: Option<Errno>,
}

impl<F: Files> Reader for SharedFile<'_, '_, F> {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> usize {
        if offset >= self.size {
            return 0;
        }
        let room = (self.size - offset).min(buffer.len() as u64) as usize;
        self.files
            .borrow_mut()
            .read_at(self.file, offset, &mut buffer[..room])
            .unwrap_or(0)
    }
}

impl<F: Files> Sink for SharedFile<'_, '_, F> {
    fn write(&mut self, bytes: &[u8]) -> bool {
        match self.files.borrow_mut().write(self.file, bytes) {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }
}

fn extract_to<F: Files>(
    files: &mut F,
    zip: Handle,
    size: u64,
    entry: &Entry,
    target: Handle,
) -> Result<(), Problem> {
    let shared = RefCell::new(files);
    let mut reader = SharedFile {
        files: &shared,
        file: zip,
        size,
        error: None,
    };
    let mut writer = SharedFile {
        files: &shared,
        file: target,
        size: 0,
        error: None,
    };
    let mut window = Window::new();
    archive::extract(&mut reader, entry, &mut window, &mut writer).map_err(|error| {
        match (error, writer.error) {
            (ArchiveError::Stopped, Some(errno)) => Problem::from_errno(errno),
            (error, _) => Problem::Archive(error),
        }
    })
}

fn make_directory_once(files: &mut impl Files, path: &[u8]) -> Result<(), Problem> {
    match files.make_directory(path) {
        Ok(()) | Err(Errno::EEXIST) => Ok(()),
        Err(error) => Err(Problem::from_errno(error)),
    }
}

fn make_parents(files: &mut impl Files, folder: &Path, name: &[u8]) -> Result<(), Problem> {
    for (at, _) in name.iter().enumerate().filter(|&(_, &byte)| byte == b'/') {
        let parent = folder.join(&name[..at]).ok_or(Problem::PathTooLong)?;
        make_directory_once(files, parent.as_bytes())?;
    }
    Ok(())
}

impl Installation {
    /// An installation of `plan`, nothing done yet.
    pub fn new(plan: Plan) -> Self {
        Self {
            plan,
            stage: Stage::Preparing,
            walk: Walk::new(),
            copy: None,
            extracting: None,
            single_copied: false,
            done_bytes: 0,
            icon: false,
        }
    }

    /// What is installed.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Whether the copy is over, so cancelling no longer applies.
    pub fn is_past_copying(&self) -> bool {
        matches!(
            self.stage,
            Stage::Icon | Stage::Registering | Stage::Finished
        )
    }

    /// Does the next small piece of the work: one folder or file made, one `buffer` of data
    /// copied or one archive entry extracted, the app's icon made, or the manifest written.
    pub fn step(&mut self, files: &mut impl Files, buffer: &mut [u8]) -> Progress {
        let result = match self.stage {
            Stage::Preparing => self.prepare(files),
            Stage::Copying => self.copy_some(files, buffer),
            Stage::Icon => self.write_icon(files),
            Stage::Registering => self.register(files),
            Stage::Finished => Ok(Progress::Installed),
        };
        match result {
            Ok(progress) => progress,
            Err(problem) => {
                self.undo(files);
                Progress::Failed(problem)
            }
        }
    }

    /// Runs every step; what the last one reports.
    pub fn run(&mut self, files: &mut impl Files, buffer: &mut [u8]) -> Progress {
        loop {
            if let done @ (Progress::Installed | Progress::Failed(_)) = self.step(files, buffer) {
                return done;
            }
        }
    }

    /// Stops the installation and removes what it wrote; `false` once the copy is over.
    pub fn cancel(&mut self, files: &mut impl Files) -> bool {
        if self.is_past_copying() {
            return false;
        }
        self.undo(files);
        true
    }

    fn folder(&self) -> Result<Path, Problem> {
        app_folder(self.plan.name.as_bytes()).ok_or(Problem::PathTooLong)
    }

    fn undo(&mut self, files: &mut impl Files) {
        if let Some(copy) = self.copy.take() {
            files.close(copy.from);
            files.close(copy.to);
        }
        if let Some(open) = self.extracting.take() {
            files.close(open.file);
        }
        if self.stage != Stage::Preparing
            && let Ok(folder) = self.folder()
        {
            let _ = remove_tree(files, folder.as_bytes());
        }
        self.stage = Stage::Finished;
    }

    fn prepare(&mut self, files: &mut impl Files) -> Result<Progress, Problem> {
        let folder = self.folder()?;
        if self.plan.leftover {
            remove_tree(files, folder.as_bytes()).map_err(Problem::from_errno)?;
        }
        files
            .make_directory(folder.as_bytes())
            .map_err(|error| match error {
                Errno::EEXIST => Problem::AlreadyInstalled,
                other => Problem::from_errno(other),
            })?;
        self.stage = Stage::Copying;
        if self.plan.bundle {
            let bundle = folder
                .join(self.plan.source.file_name())
                .ok_or(Problem::PathTooLong)?;
            files
                .make_directory(bundle.as_bytes())
                .map_err(Problem::from_errno)?;
        }
        Ok(Progress::Copying(0))
    }

    fn start_copy(
        &mut self,
        files: &mut impl Files,
        from: &[u8],
        to: &[u8],
        permissions: u32,
    ) -> Result<(), Problem> {
        let from = files.open(from).map_err(Problem::Unreadable)?;
        match files.create(to, permissions) {
            Ok(to) => {
                self.copy = Some(OpenCopy { from, to });
                Ok(())
            }
            Err(error) => {
                files.close(from);
                Err(Problem::from_errno(error))
            }
        }
    }

    fn copy_some(
        &mut self,
        files: &mut impl Files,
        buffer: &mut [u8],
    ) -> Result<Progress, Problem> {
        if let Some(copy) = self.copy {
            let read = files.read(copy.from, buffer).map_err(Problem::Unreadable)?;
            if read == 0 {
                self.copy = None;
                files.close(copy.from);
                let synced = files.sync_file(copy.to);
                files.close(copy.to);
                synced.map_err(Problem::from_errno)?;
            } else {
                files
                    .write(copy.to, &buffer[..read])
                    .map_err(Problem::from_errno)?;
                self.done_bytes = self.done_bytes.saturating_add(read as u64);
            }
            return Ok(Progress::Copying(self.done_bytes));
        }
        if let Some(archive) = self.plan.archive {
            return self.extract_next(files, archive);
        }
        let folder = self.folder()?;
        if !self.plan.bundle {
            if self.single_copied {
                self.stage = Stage::Icon;
                return Ok(Progress::Registering);
            }
            self.single_copied = true;
            let to = folder
                .join(self.plan.run.as_bytes())
                .ok_or(Problem::PathTooLong)?;
            let permissions = files
                .status(self.plan.source.as_bytes())
                .map_err(Problem::Unreadable)?
                .permissions;
            let source = self.plan.source;
            self.start_copy(files, source.as_bytes(), to.as_bytes(), permissions)?;
            return Ok(Progress::Copying(self.done_bytes));
        }
        let source = self.plan.source;
        let Some((relative, status)) = self.walk.next(files, &source)? else {
            self.stage = Stage::Icon;
            return Ok(Progress::Registering);
        };
        let to = folder
            .join(source.file_name())
            .and_then(|bundle| bundle.join(relative.as_bytes()))
            .ok_or(Problem::PathTooLong)?;
        match status.kind {
            Kind::Directory => files
                .make_directory(to.as_bytes())
                .map_err(Problem::from_errno)?,
            Kind::File => {
                let from = source
                    .join(relative.as_bytes())
                    .ok_or(Problem::PathTooLong)?;
                self.start_copy(files, from.as_bytes(), to.as_bytes(), status.permissions)?;
            }
            Kind::Other => return Err(Problem::UnsupportedEntry),
        }
        Ok(Progress::Copying(self.done_bytes))
    }

    fn extract_next(
        &mut self,
        files: &mut impl Files,
        archive: Archive,
    ) -> Result<Progress, Problem> {
        let open = match self.extracting {
            Some(open) => open,
            None => {
                let file = files
                    .open(self.plan.source.as_bytes())
                    .map_err(Problem::Unreadable)?;
                let open = OpenArchive {
                    file,
                    cursor: archive.first(),
                };
                self.extracting = Some(open);
                open
            }
        };
        let mut cursor = open.cursor;
        let next = archive.next(
            &mut FileReader::new(files, open.file, archive.size()),
            &mut cursor,
        );
        self.extracting = Some(OpenArchive { cursor, ..open });
        let Some(entry) = next.map_err(Problem::Archive)? else {
            self.extracting = None;
            files.close(open.file);
            self.stage = Stage::Icon;
            return Ok(Progress::Registering);
        };
        if is_reserved(entry.name()) {
            return Err(Problem::ReservedName);
        }
        if entry.is_metadata() {
            return Ok(Progress::Copying(self.done_bytes));
        }
        if !entry.is_directory()
            && self.done_bytes.saturating_add(entry.size()) > MAX_EXTRACTED_BYTES
        {
            return Err(Problem::TooLarge);
        }
        let folder = self.folder()?;
        make_parents(files, &folder, entry.name())?;
        let to = folder.join(entry.name()).ok_or(Problem::PathTooLong)?;
        if entry.is_directory() {
            make_directory_once(files, to.as_bytes())?;
            return Ok(Progress::Copying(self.done_bytes));
        }
        let permissions = match (
            entry.permissions(),
            entry.name() == self.plan.run.as_bytes(),
        ) {
            (_, true) => PROGRAM_PERMISSIONS,
            (Some(permissions), false) => permissions,
            (None, false) => FILE_PERMISSIONS,
        };
        let target = files
            .create(to.as_bytes(), permissions)
            .map_err(Problem::from_errno)?;
        let extracted = extract_to(files, open.file, archive.size(), &entry, target);
        let synced = files.sync_file(target);
        files.close(target);
        extracted?;
        synced.map_err(Problem::from_errno)?;
        self.done_bytes = self.done_bytes.saturating_add(entry.size());
        Ok(Progress::Copying(self.done_bytes))
    }

    fn write_icon(&mut self, files: &mut impl Files) -> Result<Progress, Problem> {
        self.stage = Stage::Registering;
        let folder = self.folder()?;
        let program = folder
            .join(self.plan.run.as_bytes())
            .ok_or(Problem::PathTooLong)?;
        let source = match self.plan.archive {
            Some(_) => None,
            None => Some(self.plan.source),
        };
        let mut work = Workspace::new();
        let mut own = OwnIcon::new();
        let source = source.as_ref().map(Path::as_bytes);
        if !icon::find(files, program.as_bytes(), source, &mut work, &mut own) {
            return Ok(Progress::Registering);
        }
        let path = folder
            .join(icon_file::FILE_NAME.as_bytes())
            .ok_or(Problem::PathTooLong)?;
        let Ok(handle) = files.create(path.as_bytes(), MANIFEST_PERMISSIONS) else {
            return Ok(Progress::Registering);
        };
        let written = files
            .write(handle, &own.file())
            .and_then(|()| files.sync_file(handle));
        files.close(handle);
        match written {
            Ok(()) => self.icon = true,
            Err(_) => {
                let _ = files.remove_file(path.as_bytes());
            }
        }
        Ok(Progress::Registering)
    }

    fn register(&mut self, files: &mut impl Files) -> Result<Progress, Problem> {
        let folder = self.folder()?;
        let run = core::str::from_utf8(self.plan.run.as_bytes())
            .map_err(|_| Problem::InvalidProgramPath)?;
        let entry = Manifest {
            name: self.plan.name.as_str(),
            source: self.plan.program.source,
            run,
            architecture: Some(self.plan.program.architecture.name()),
            synonyms: "",
            icon: self.icon.then_some(icon_file::FILE_NAME),
        };
        let mut file = [0u8; manifest::LARGEST];
        let length = manifest::write(&entry, &mut file).map_err(|refused| match refused {
            manifest::Refused::InvalidName => Problem::InvalidName,
            _ => Problem::InvalidProgramPath,
        })?;
        let temporary = folder
            .join(TEMPORARY_MANIFEST)
            .ok_or(Problem::PathTooLong)?;
        let target = folder
            .join(manifest::FILE_NAME.as_bytes())
            .ok_or(Problem::PathTooLong)?;
        let handle = files
            .create(temporary.as_bytes(), MANIFEST_PERMISSIONS)
            .map_err(Problem::from_errno)?;
        let written = files
            .write(handle, &file[..length])
            .and_then(|()| files.sync_file(handle));
        files.close(handle);
        written.map_err(Problem::from_errno)?;
        files
            .rename(temporary.as_bytes(), target.as_bytes())
            .map_err(Problem::from_errno)?;
        files.sync();
        self.stage = Stage::Finished;
        Ok(Progress::Installed)
    }
}

/// Installs `plan` in one go.
pub fn install(files: &mut impl Files, plan: Plan, buffer: &mut [u8]) -> Result<(), Problem> {
    match Installation::new(plan).run(files, buffer) {
        Progress::Failed(problem) => Err(problem),
        _ => Ok(()),
    }
}

/// Uninstalls the app called `name`: its manifest first, so it leaves the catalogue at once,
/// then its folder and everything in it (D18: nothing is left behind).
pub fn uninstall(files: &mut impl Files, name: &[u8]) -> Result<(), Problem> {
    if !Name::new(name).is_valid() {
        return Err(Problem::NotInstalled);
    }
    let mut file = [0u8; manifest::LARGEST + 1];
    if read_manifest(files, name, &mut file).is_none() {
        return Err(Problem::NotInstalled);
    }
    let folder = app_folder(name).ok_or(Problem::PathTooLong)?;
    let manifest_path = folder
        .join(manifest::FILE_NAME.as_bytes())
        .ok_or(Problem::PathTooLong)?;
    files
        .remove_file(manifest_path.as_bytes())
        .map_err(Problem::from_errno)?;
    let removed = remove_tree(files, folder.as_bytes()).map_err(Problem::from_errno);
    files.sync();
    removed
}

/// Calls `each` with the manifest of every installed app, in `/Applications`' order; nothing
/// when there is no `/Applications`.
pub fn for_each_installed(
    files: &mut impl Files,
    mut each: impl FnMut(&Manifest<'_>),
) -> Result<(), Errno> {
    let mut name = [0u8; 256];
    let mut file = [0u8; manifest::LARGEST + 1];
    let mut index = 0;
    loop {
        let entry = match files.entry(APPLICATIONS, index, &mut name) {
            Ok(entry) => entry,
            Err(Errno::ENOENT) => return Ok(()),
            Err(error) => return Err(error),
        };
        let Some(length) = entry else {
            return Ok(());
        };
        index += 1;
        let folder = &name[..length];
        if let Some(read) = read_manifest(files, folder, &mut file)
            && let Some(app) = manifest::parse(&file[..read], folder)
        {
            each(&app);
        }
    }
}

/// The most apps a page of `app list` holds (D164).
pub const PAGE_LIMIT: usize = eggos_commands::commands::LIST_PAGE_LIMIT as usize;

const PAGE_SLOTS: usize = PAGE_LIMIT + 1;
const NAME_KEY_BYTES: usize = manifest::NAME_CAPACITY;

/// Calls `each` with the installed apps whose names come after `after`, in `/Applications`'
/// order: with `limit`, only the next `limit` of them by name, and then the name the page ended
/// with when more follow, the next page's cursor (D164).
pub fn for_each_installed_page(
    files: &mut impl Files,
    limit: Option<usize>,
    after: Option<&[u8]>,
    mut each: impl FnMut(&Manifest<'_>),
) -> Result<Option<Name>, Errno> {
    let comes_after = |name: &str| after.is_none_or(|after| name.as_bytes() > after);
    let Some(limit) = limit else {
        for_each_installed(files, |app| {
            if comes_after(app.name) {
                each(app);
            }
        })?;
        return Ok(None);
    };
    let mut page = Smallest::<NAME_KEY_BYTES, PAGE_SLOTS>::new(limit);
    for_each_installed(files, |app| {
        if comes_after(app.name) {
            page.offer(app.name.as_bytes());
        }
    })?;
    for_each_installed(files, |app| {
        if page.holds(app.name.as_bytes()) {
            each(app);
        }
    })?;
    Ok(page.last().filter(|_| page.more()).map(Name::new))
}

const _: () = assert!(
    PATH_CAPACITY >= APPLICATIONS.len() + 1 + manifest::NAME_CAPACITY + 1 + manifest::RUN_CAPACITY
);
