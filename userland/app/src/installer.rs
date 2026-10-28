//! Installing, uninstalling and listing apps (D134, docs/apps.md): an app is the folder
//! `/Applications/<name>/` holding its program (an `.app` bundle whole) and `app.manifest`,
//! which is written last, through a temporary file renamed over it, so a half-installed app has
//! no manifest and is not listed (D133); uninstall removes the manifest first, then the folder.
//! An installation is a sequence of small steps so the installer's window stays responsive and
//! can cancel between them.

use crate::architecture::Architecture;
use crate::files::{self, Files, Handle, Kind, PATH_CAPACITY, Path};
use crate::program::{self, NotRunnable, Program};
use eggos_app_manifest::{self as manifest, Manifest, Source};
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

const BUNDLE_SUFFIX: &[u8] = b".app";
const BUNDLE_PROGRAMS: &[u8] = b"Contents/MacOS";
const MANIFEST_PERMISSIONS: u32 = 0o644;

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
    /// Another error of the file system.
    Failed(Errno),
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
    bytes: [u8; 64],
    length: usize,
}

impl core::fmt::Debug for Name {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.as_bytes().escape_ascii(), formatter)
    }
}

impl Name {
    /// `bytes`, its first 64 bytes kept.
    pub fn new(bytes: &[u8]) -> Self {
        let length = bytes.len().min(64);
        let mut name = Self {
            bytes: [0; 64],
            length,
        };
        name.bytes[..length].copy_from_slice(&bytes[..length]);
        name
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

    /// The file or `.app` folder installed.
    pub fn source(&self) -> &[u8] {
        self.source.as_bytes()
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

fn bundle_program(files: &mut impl Files, bundle: &Path, stem: &[u8]) -> Result<Path, Problem> {
    let programs = bundle.join(BUNDLE_PROGRAMS).ok_or(Problem::PathTooLong)?;
    let named = programs.join(stem).ok_or(Problem::PathTooLong)?;
    if files
        .status(named.as_bytes())
        .is_ok_and(|status| status.kind == Kind::File)
    {
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
        let is_file = files
            .status(candidate.as_bytes())
            .is_ok_and(|status| status.kind == Kind::File);
        if is_file {
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

/// Checks that the file or `.app` folder at `source` can be installed on `machine` as `name`
/// (else the name its file name gives) and works out how.
pub fn plan(
    files: &mut impl Files,
    source: &[u8],
    name: Option<&[u8]>,
    machine: Architecture,
) -> Result<Plan, Refused> {
    let file_name = files::file_name(source);
    let is_bundle_name = is_bundle_name(file_name);
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
    let program = program::recognise(&head[..length], machine)
        .map_err(|reason| refused(Problem::NotRunnable(reason)))?;
    if bundle && program.source != Source::Macos {
        return Err(refused(Problem::NotRunnable(NotRunnable::NotAProgram)));
    }
    if let Some(layer) = layer_path(program.source) {
        let installed = files
            .status(layer)
            .is_ok_and(|status| status.kind == Kind::File);
        if !installed {
            return Err(refused(Problem::NotRunnable(NotRunnable::LayerMissing(
                program.source,
            ))));
        }
    }
    if !name.is_valid() {
        return Err(refused(Problem::InvalidName));
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
    if !manifest::is_run(run.as_bytes()) {
        return Err(refused(Problem::InvalidProgramPath));
    }
    let folder = app_folder(name.as_bytes()).ok_or(refused(Problem::PathTooLong))?;
    if folder.join(run.as_bytes()).is_none() {
        return Err(refused(Problem::PathTooLong));
    }
    let leftover = match files.status(folder.as_bytes()) {
        Ok(_) => {
            let mut file = [0u8; manifest::LARGEST + 1];
            if read_manifest(files, name.as_bytes(), &mut file).is_some() {
                return Err(refused(Problem::AlreadyInstalled));
            }
            true
        }
        Err(_) => false,
    };
    let total_bytes = match bundle {
        true => measure(files, &source).map_err(refused)?,
        false => status.size,
    };
    Ok(Plan {
        name,
        program,
        source,
        bundle,
        run,
        leftover,
        total_bytes,
    })
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
    Registering,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenCopy {
    from: Handle,
    to: Handle,
}

/// Where an installation is after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Copying the program: this many of the plan's bytes are written.
    Copying(u64),
    /// Writing the manifest next.
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
    single_copied: bool,
    done_bytes: u64,
}

impl Installation {
    /// An installation of `plan`, nothing done yet.
    pub fn new(plan: Plan) -> Self {
        Self {
            plan,
            stage: Stage::Preparing,
            walk: Walk::new(),
            copy: None,
            single_copied: false,
            done_bytes: 0,
        }
    }

    /// What is installed.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Whether the copy is over, so cancelling no longer applies.
    pub fn is_past_copying(&self) -> bool {
        matches!(self.stage, Stage::Registering | Stage::Finished)
    }

    /// Does the next small piece of the work: one folder or file made, or one `buffer` of
    /// data copied, or the manifest written.
    pub fn step(&mut self, files: &mut impl Files, buffer: &mut [u8]) -> Progress {
        let result = match self.stage {
            Stage::Preparing => self.prepare(files),
            Stage::Copying => self.copy_some(files, buffer),
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
        let folder = self.folder()?;
        if !self.plan.bundle {
            if self.single_copied {
                self.stage = Stage::Registering;
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
            self.stage = Stage::Registering;
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

const _: () = assert!(
    PATH_CAPACITY >= APPLICATIONS.len() + 1 + manifest::NAME_CAPACITY + 1 + manifest::RUN_CAPACITY
);
