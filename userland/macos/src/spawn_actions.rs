//! `posix_spawn`'s attributes and file actions as plain data, with Darwin's flag values, the
//! ordered steps a new child takes before it runs the program, and the paths `execvp` and
//! `posix_spawnp` try. Pure logic, host-tested; `libsystem::children` takes the steps (D190).

/// Darwin's `posix_spawnattr_setflags` bits.
pub mod flag {
    pub const RESETIDS: i16 = 0x0001;
    pub const SETPGROUP: i16 = 0x0002;
    pub const SETSIGDEF: i16 = 0x0004;
    pub const SETSIGMASK: i16 = 0x0008;
    pub const SETEXEC: i16 = 0x0040;
    pub const START_SUSPENDED: i16 = 0x0080;
    pub const SETSID: i16 = 0x0400;
    pub const CLOEXEC_DEFAULT: i16 = 0x4000;
}

/// The highest descriptor a file action may name, plus one: Darwin's `OPEN_MAX`.
pub const OPEN_MAX: i32 = 10240;
/// The longest path, with its NUL, `execvp` builds: Darwin's `PATH_MAX`.
pub const PATH_MAX: usize = 1024;
/// The directories `execvp` searches when `PATH` is not set: Darwin's `_PATH_DEFPATH`.
pub const DEFAULT_SEARCH_PATH: &[u8] = b"/usr/bin:/bin";

const ENOTSUP: i32 = 45;
const ENAMETOOLONG: i32 = 63;

/// A `posix_spawnattr_t`'s settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attributes {
    pub flags: i16,
    pub process_group: i32,
    pub default_signals: u32,
    pub mask: u32,
}

impl Attributes {
    /// `Ok` if the layer can start a child with these settings, else Darwin's `errno`:
    /// `ENOTSUP` for `POSIX_SPAWN_START_SUSPENDED`. Other flags it has no use for (Darwin's
    /// private ones) are ignored, as Darwin's kernel ignores those it does not know.
    pub fn check(&self) -> Result<(), i32> {
        if self.flags & flag::START_SUSPENDED != 0 {
            return Err(ENOTSUP);
        }
        Ok(())
    }

    /// Whether the calling process becomes the program (`POSIX_SPAWN_SETEXEC`) instead of a
    /// new child.
    pub fn replaces_caller(&self) -> bool {
        self.flags & flag::SETEXEC != 0
    }

    fn steps<P>(&self) -> [Option<Step<P>>; 6] {
        let on = |bit: i16| self.flags & bit != 0;
        [
            on(flag::CLOEXEC_DEFAULT).then_some(Step::CloseAllOnExec),
            on(flag::SETSID).then_some(Step::NewSession),
            on(flag::SETPGROUP).then_some(Step::SetProcessGroup(self.process_group)),
            on(flag::RESETIDS).then_some(Step::ResetIds),
            on(flag::SETSIGDEF).then_some(Step::DefaultSignals(self.default_signals)),
            on(flag::SETSIGMASK).then_some(Step::SetMask(self.mask)),
        ]
    }
}

/// One `posix_spawn_file_actions_add*` entry; `path` is the action's own copy of the path, and
/// `flags` are Linux's open flags, translated when the action was added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction<P> {
    Open {
        fd: i32,
        path: P,
        flags: u32,
        mode: u32,
    },
    Close {
        fd: i32,
    },
    Duplicate {
        from: i32,
        to: i32,
    },
    Inherit {
        fd: i32,
    },
    ChangeDirectory {
        path: P,
    },
    ChangeDirectoryTo {
        fd: i32,
    },
}

impl<P: Copy> FileAction<P> {
    fn step(&self) -> Step<P> {
        match *self {
            Self::Open {
                fd,
                path,
                flags,
                mode,
            } => Step::Open {
                fd,
                path,
                flags,
                mode,
            },
            Self::Close { fd } => Step::Close(fd),
            Self::Duplicate { from, to } if from == to => Step::KeepAcrossExec(to),
            Self::Duplicate { from, to } => Step::Duplicate { from, to },
            Self::Inherit { fd } => Step::KeepAcrossExec(fd),
            Self::ChangeDirectory { path } => Step::ChangeDirectory(path),
            Self::ChangeDirectoryTo { fd } => Step::ChangeDirectoryTo(fd),
        }
    }
}

/// What a new child does before it runs the program, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step<P> {
    /// Marks every open descriptor close-on-exec, so only those a later step names stay open.
    CloseAllOnExec,
    NewSession,
    SetProcessGroup(i32),
    ResetIds,
    /// Gives the signals of this Darwin set their default action.
    DefaultSignals(u32),
    /// Blocks exactly the signals of this Darwin set.
    SetMask(u32),
    /// Opens `path` as descriptor `fd`.
    Open {
        fd: i32,
        path: P,
        flags: u32,
        mode: u32,
    },
    Close(i32),
    Duplicate {
        from: i32,
        to: i32,
    },
    /// Clears the descriptor's close-on-exec flag.
    KeepAcrossExec(i32),
    ChangeDirectory(P),
    ChangeDirectoryTo(i32),
}

/// The steps a child started with `attributes` and `actions` takes, in order: the attributes'
/// (descriptors first, then session, group, ids and signals), then each file action as added.
pub fn steps<P: Copy>(
    attributes: Attributes,
    actions: &[FileAction<P>],
) -> impl Iterator<Item = Step<P>> {
    attributes
        .steps()
        .into_iter()
        .flatten()
        .chain(actions.iter().map(FileAction::step))
}

/// Whether `fd` is a descriptor a file action may name.
pub fn valid_descriptor(fd: i32) -> bool {
    (0..OPEN_MAX).contains(&fd)
}

/// The NUL-terminated path `execvp` tries for `file` in `directory` (an empty directory is the
/// working directory), written into `buffer`; `ENAMETOOLONG` if it does not fit.
pub fn candidate<'a>(
    directory: &[u8],
    file: &[u8],
    buffer: &'a mut [u8; PATH_MAX],
) -> Result<&'a [u8], i32> {
    let directory: &[u8] = if directory.is_empty() {
        b"."
    } else {
        directory
    };
    let length = directory.len() + 1 + file.len();
    if length + 1 > PATH_MAX {
        return Err(ENAMETOOLONG);
    }
    buffer[..directory.len()].copy_from_slice(directory);
    buffer[directory.len()] = b'/';
    buffer[directory.len() + 1..length].copy_from_slice(file);
    buffer[length] = 0;
    Ok(&buffer[..=length])
}

/// Whether `execvp` runs `file` as given rather than searching `PATH`: when it names a
/// directory.
pub fn is_path(file: &[u8]) -> bool {
    file.contains(&b'/')
}
