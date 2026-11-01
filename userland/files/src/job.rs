//! Copying, moving and deleting files and folders in small steps (D135), so a window stays
//! responsive and can cancel between them, and the `cp -r`, `mv` and `trash` commands run the
//! same code to the end. A copy never replaces anything: its target must not exist. A move is
//! a rename when it can be; across file systems it is a copy, then a delete of the source. A
//! delete removes a folder's entries one at a time from the first, so a file system whose
//! listing moves under removals still loses every entry. Cancelling keeps what is done and
//! removes the file half-written. Paths are normalized first, so a target written with `.`,
//! `..` or `//` cannot hide inside its own source. A symbolic link is copied, moved and deleted
//! as a link, with the same target text, and never descended into (D170).

use crate::names::{is_inside, normalized};
use eggos_app::files::{Files, Handle, Kind, PATH_CAPACITY, Path, Status};
use eggos_runtime::errno::Errno;

/// How deep folders may nest under what is copied or deleted.
pub const MAX_DEPTH: usize = 32;
/// The bytes a step copies at most.
pub const STEP_BYTES: usize = 64 * 1024;
const ENTRIES_PER_STEP: usize = 16;
const PERMILLE: u64 = 1000;

/// What a job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    Copy,
    Move,
    Delete,
}

/// Where a job is after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Step {
    /// More to do.
    Working,
    /// All done.
    Done,
    /// It stopped at `path` with `error`.
    Failed(Errno, Path),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Starting,
    Measuring,
    CopyingRoot,
    Copying,
    Deleting,
    Finished,
}

#[derive(Debug, Clone, Copy)]
struct Walk {
    relative: Path,
    indexes: [usize; MAX_DEPTH],
    depth: usize,
}

#[allow(clippy::result_large_err)]
impl Walk {
    fn new() -> Self {
        Self {
            relative: Path::new(),
            indexes: [0; MAX_DEPTH],
            depth: 0,
        }
    }

    fn under(root: &Path, relative: &Path) -> Result<Path, Errno> {
        match relative.is_empty() {
            true => Ok(*root),
            false => root.join(relative.as_bytes()).ok_or(Errno::ENAMETOOLONG),
        }
    }

    fn next(
        &mut self,
        files: &mut impl Files,
        root: &Path,
    ) -> Result<Option<(Path, Status)>, (Errno, Path)> {
        let mut name = [0u8; 256];
        loop {
            let folder = Self::under(root, &self.relative).map_err(|error| (error, *root))?;
            let entry = files
                .entry(folder.as_bytes(), self.indexes[self.depth], &mut name)
                .map_err(|error| (error, folder))?;
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
                return Err((Errno::ENAMETOOLONG, folder));
            }
            let path = Self::under(root, &relative).map_err(|error| (error, folder))?;
            let status = files
                .status(path.as_bytes())
                .map_err(|error| (error, path))?;
            if status.kind == Kind::Directory {
                if self.depth + 1 >= MAX_DEPTH {
                    return Err((Errno::ENAMETOOLONG, path));
                }
                self.depth += 1;
                self.indexes[self.depth] = 0;
                self.relative = relative;
            }
            return Ok(Some((relative, status)));
        }
    }
}

#[allow(clippy::result_large_err)]
fn copy_link(files: &mut impl Files, from: &Path, to: &Path) -> Result<(), (Errno, Path)> {
    let mut target = [0u8; PATH_CAPACITY];
    let length = files
        .read_link(from.as_bytes(), &mut target)
        .map_err(|error| (error, *from))?;
    files
        .make_link(&target[..length], to.as_bytes())
        .map_err(|error| (error, *to))
}

#[derive(Debug, Clone, Copy)]
struct Writing {
    from: Handle,
    to: Handle,
    target: Path,
}

/// A copy, move or delete under way. See the module documentation.
#[derive(Debug)]
pub struct Job {
    work: Work,
    from: Path,
    to: Path,
    stage: Stage,
    walk: Walk,
    writing: Option<Writing>,
    single: bool,
    current: Path,
    total: u64,
    done: u64,
}

#[allow(clippy::result_large_err)]
impl Job {
    /// A copy of `from` to `to`, which must not exist yet.
    pub fn copy(from: &Path, to: &Path) -> Self {
        Self::new(Work::Copy, from, to)
    }

    /// A move of `from` to `to`, which must not exist yet.
    pub fn moving(from: &Path, to: &Path) -> Self {
        Self::new(Work::Move, from, to)
    }

    /// A delete of `path` and everything in it.
    pub fn delete(path: &Path) -> Self {
        Self::new(Work::Delete, path, &Path::new())
    }

    fn new(work: Work, from: &Path, to: &Path) -> Self {
        let exact = |path: &Path| match path.is_empty() {
            true => *path,
            false => normalized(path.as_bytes()).unwrap_or(*path),
        };
        let (from, to) = (&exact(from), &exact(to));
        Self {
            work,
            from: *from,
            to: *to,
            stage: Stage::Starting,
            walk: Walk::new(),
            writing: None,
            single: false,
            current: *from,
            total: 0,
            done: 0,
        }
    }

    /// What it does.
    pub fn work(&self) -> Work {
        self.work
    }

    /// What it copies, moves or deletes.
    pub fn source(&self) -> &Path {
        &self.from
    }

    /// Where it copies or moves to.
    pub fn target(&self) -> &Path {
        &self.to
    }

    /// How far it is, in thousandths: bytes for a copy and a move, entries for a delete.
    pub fn permille(&self) -> u16 {
        match self.total {
            0 => 0,
            total => (self.done.min(total) * PERMILLE / total) as u16,
        }
    }

    /// Does the next small piece of the work with `buffer` for the data.
    pub fn step(&mut self, files: &mut impl Files, buffer: &mut [u8]) -> Step {
        let result = match self.stage {
            Stage::Starting => self.start(files),
            Stage::Measuring => self.measure(files),
            Stage::CopyingRoot | Stage::Copying => self.copy_some(files, buffer),
            Stage::Deleting => self.delete_some(files),
            Stage::Finished => Ok(()),
        };
        match result {
            Ok(()) if self.stage == Stage::Finished => Step::Done,
            Ok(()) => Step::Working,
            Err((error, path)) => {
                self.cancel(files);
                Step::Failed(error, path)
            }
        }
    }

    /// Runs every step: what the last one says.
    pub fn run(&mut self, files: &mut impl Files, buffer: &mut [u8]) -> Step {
        loop {
            match self.step(files, buffer) {
                Step::Working => {}
                done => return done,
            }
        }
    }

    /// Stops: what is done stays, a file half-written is removed.
    pub fn cancel(&mut self, files: &mut impl Files) {
        if let Some(writing) = self.writing.take() {
            files.close(writing.from);
            files.close(writing.to);
            let _ = files.remove_file(writing.target.as_bytes());
        }
        self.stage = Stage::Finished;
    }

    fn start(&mut self, files: &mut impl Files) -> Result<(), (Errno, Path)> {
        let from = self.from;
        files
            .status(from.as_bytes())
            .map_err(|error| (error, from))?;
        if self.work != Work::Delete {
            let to = self.to;
            if is_inside(to.as_bytes(), from.as_bytes()) {
                return Err((Errno::EINVAL, to));
            }
            match files.status(to.as_bytes()) {
                Err(Errno::ENOENT) => {}
                Ok(_) => return Err((Errno::EEXIST, to)),
                Err(error) => return Err((error, to)),
            }
        }
        if self.work == Work::Move {
            match files.rename(from.as_bytes(), self.to.as_bytes()) {
                Ok(()) => {
                    self.stage = Stage::Finished;
                    return Ok(());
                }
                Err(Errno::EXDEV) => {}
                Err(error) => return Err((error, from)),
            }
        }
        self.stage = Stage::Measuring;
        self.measure_root(files)
    }

    fn measure_root(&mut self, files: &mut impl Files) -> Result<(), (Errno, Path)> {
        let from = self.from;
        let status = files
            .status(from.as_bytes())
            .map_err(|error| (error, from))?;
        self.total = match self.work {
            Work::Delete => 1,
            _ => status.size,
        };
        if status.kind != Kind::Directory {
            self.finish_measuring();
        }
        Ok(())
    }

    fn finish_measuring(&mut self) {
        self.walk = Walk::new();
        self.stage = match self.work {
            Work::Delete => Stage::Deleting,
            _ => Stage::CopyingRoot,
        };
        self.current = self.from;
    }

    fn measure(&mut self, files: &mut impl Files) -> Result<(), (Errno, Path)> {
        let from = self.from;
        for _ in 0..ENTRIES_PER_STEP {
            match self.walk.next(files, &from)? {
                Some((_, status)) => {
                    self.total += match self.work {
                        Work::Delete => 1,
                        _ => status.size,
                    };
                }
                None => {
                    self.finish_measuring();
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn begin_file(
        &mut self,
        files: &mut impl Files,
        from: &Path,
        to: &Path,
        permissions: u32,
    ) -> Result<(), (Errno, Path)> {
        let source = files
            .open(from.as_bytes())
            .map_err(|error| (error, *from))?;
        match files.create(to.as_bytes(), permissions) {
            Ok(target) => {
                self.writing = Some(Writing {
                    from: source,
                    to: target,
                    target: *to,
                });
                Ok(())
            }
            Err(error) => {
                files.close(source);
                Err((error, *to))
            }
        }
    }

    fn after_copy(&mut self) {
        match self.work {
            Work::Move => {
                self.stage = Stage::Deleting;
                self.current = self.from;
            }
            _ => self.stage = Stage::Finished,
        }
    }

    fn copy_some(
        &mut self,
        files: &mut impl Files,
        buffer: &mut [u8],
    ) -> Result<(), (Errno, Path)> {
        let chunk = buffer.len().min(STEP_BYTES);
        let buffer = &mut buffer[..chunk];
        if let Some(writing) = self.writing {
            let read = files
                .read(writing.from, buffer)
                .map_err(|error| (error, self.current))?;
            if read > 0 {
                files
                    .write(writing.to, &buffer[..read])
                    .map_err(|error| (error, writing.target))?;
                self.done = self.done.saturating_add(read as u64);
                return Ok(());
            }
            self.writing = None;
            files.close(writing.from);
            let synced = files.sync_file(writing.to);
            files.close(writing.to);
            synced.map_err(|error| (error, writing.target))?;
            if self.single {
                self.after_copy();
            }
            return Ok(());
        }
        let (from, to) = (self.from, self.to);
        if self.stage == Stage::CopyingRoot {
            self.stage = Stage::Copying;
            let status = files
                .status(from.as_bytes())
                .map_err(|error| (error, from))?;
            return match status.kind {
                Kind::Directory => files
                    .make_directory(to.as_bytes())
                    .map_err(|error| (error, to)),
                Kind::File => {
                    self.single = true;
                    self.begin_file(files, &from, &to, status.permissions)
                }
                Kind::Link => {
                    copy_link(files, &from, &to)?;
                    self.done = self.done.saturating_add(status.size);
                    self.after_copy();
                    Ok(())
                }
                Kind::Other => Err((Errno::EINVAL, from)),
            };
        }
        let Some((relative, status)) = self.walk.next(files, &from)? else {
            self.after_copy();
            return Ok(());
        };
        let source = from
            .join(relative.as_bytes())
            .ok_or((Errno::ENAMETOOLONG, from))?;
        let target = to
            .join(relative.as_bytes())
            .ok_or((Errno::ENAMETOOLONG, to))?;
        self.current = source;
        match status.kind {
            Kind::Directory => files
                .make_directory(target.as_bytes())
                .map_err(|error| (error, target)),
            Kind::File => self.begin_file(files, &source, &target, status.permissions),
            Kind::Link => {
                copy_link(files, &source, &target)?;
                self.done = self.done.saturating_add(status.size);
                Ok(())
            }
            Kind::Other => Err((Errno::EINVAL, source)),
        }
    }

    fn delete_some(&mut self, files: &mut impl Files) -> Result<(), (Errno, Path)> {
        let root = self.from;
        let mut name = [0u8; 256];
        for _ in 0..ENTRIES_PER_STEP {
            let current = self.current;
            let status = files
                .status(current.as_bytes())
                .map_err(|error| (error, current))?;
            if status.kind != Kind::Directory {
                files
                    .remove_file(current.as_bytes())
                    .map_err(|error| (error, current))?;
                self.done += 1;
                if current == root {
                    self.stage = Stage::Finished;
                    return Ok(());
                }
                self.current.pop();
                continue;
            }
            match files
                .entry(current.as_bytes(), 0, &mut name)
                .map_err(|error| (error, current))?
            {
                Some(length) => {
                    self.current = current
                        .join(&name[..length])
                        .ok_or((Errno::ENAMETOOLONG, current))?;
                }
                None => {
                    files
                        .remove_directory(current.as_bytes())
                        .map_err(|error| (error, current))?;
                    self.done += 1;
                    if current == root {
                        self.stage = Stage::Finished;
                        return Ok(());
                    }
                    self.current.pop();
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryFiles, Node};

    fn tree() -> MemoryFiles {
        let mut files = MemoryFiles::new();
        files
            .file("/home/root/Work/a.txt", &[7u8; 200_000], 1)
            .file("/home/root/Work/Deep/b.txt", b"b", 1)
            .folder("/home/root/Work/Empty")
            .file("/media/host/notes.txt", b"from the host", 1)
            .folder("/tmp")
            .device("/media/host", 3)
            .read_only("/media/host")
            .device("/tmp", 2);
        files
    }

    fn path(text: &str) -> Path {
        Path::from(text.as_bytes()).unwrap()
    }

    fn run(files: &mut MemoryFiles, mut job: Job) -> Step {
        let mut buffer = vec![0u8; STEP_BYTES];
        let mut steps = 0;
        loop {
            steps += 1;
            assert!(steps < 10_000, "the job never ends");
            match job.step(files, &mut buffer) {
                Step::Working => {}
                done => return done,
            }
        }
    }

    #[test]
    fn a_folder_is_copied_whole_and_its_source_kept() {
        let mut files = tree();
        let job = Job::copy(&path("/home/root/Work"), &path("/tmp/Work"));
        assert_eq!(run(&mut files, job), Step::Done);
        assert_eq!(
            files.contents("/tmp/Work/a.txt").map(<[u8]>::len),
            Some(200_000)
        );
        assert_eq!(files.contents("/tmp/Work/Deep/b.txt"), Some(&b"b"[..]));
        assert_eq!(files.node("/tmp/Work/Empty"), Some(&Node::Folder));
        assert!(files.node("/home/root/Work/a.txt").is_some());
        assert_eq!(files.open_count(), 0);
    }

    #[test]
    fn a_file_is_copied_out_of_the_read_only_share_but_not_into_it() {
        let mut files = tree();
        let job = Job::copy(
            &path("/media/host/notes.txt"),
            &path("/home/root/notes.txt"),
        );
        assert_eq!(run(&mut files, job), Step::Done);
        assert_eq!(
            files.contents("/home/root/notes.txt"),
            Some(&b"from the host"[..])
        );
        let job = Job::copy(&path("/home/root/notes.txt"), &path("/media/host/back.txt"));
        assert_eq!(
            run(&mut files, job),
            Step::Failed(Errno::EROFS, path("/media/host/back.txt"))
        );
        let job = Job::moving(
            &path("/media/host/notes.txt"),
            &path("/home/root/moved.txt"),
        );
        assert!(matches!(
            run(&mut files, job),
            Step::Failed(Errno::EROFS, _)
        ));
        assert_eq!(
            files.contents("/media/host/notes.txt"),
            Some(&b"from the host"[..])
        );
        assert_eq!(files.open_count(), 0);
    }

    #[test]
    fn a_move_renames_on_one_file_system_and_copies_then_deletes_across() {
        let mut files = tree();
        let job = Job::moving(&path("/home/root/Work/Deep"), &path("/home/root/Deep"));
        assert_eq!(run(&mut files, job), Step::Done);
        assert_eq!(files.contents("/home/root/Deep/b.txt"), Some(&b"b"[..]));
        let job = Job::moving(&path("/home/root/Work"), &path("/tmp/Work"));
        assert_eq!(run(&mut files, job), Step::Done);
        assert_eq!(files.node("/home/root/Work"), None);
        assert_eq!(
            files.contents("/tmp/Work/a.txt").map(<[u8]>::len),
            Some(200_000)
        );
        assert_eq!(files.node("/tmp/Work/Empty"), Some(&Node::Folder));
    }

    #[test]
    fn nothing_is_replaced_and_a_folder_never_goes_into_itself() {
        let mut files = tree();
        files.folder("/tmp/Work");
        let job = Job::copy(&path("/home/root/Work"), &path("/tmp/Work"));
        assert_eq!(
            run(&mut files, job),
            Step::Failed(Errno::EEXIST, path("/tmp/Work"))
        );
        let job = Job::moving(&path("/home/root/Work"), &path("/home/root/Work/Deep/x"));
        assert!(matches!(
            run(&mut files, job),
            Step::Failed(Errno::EINVAL, _)
        ));
        let job = Job::copy(&path("/home/root/./Work"), &path("/home/root//Work/./x"));
        assert!(matches!(
            run(&mut files, job),
            Step::Failed(Errno::EINVAL, _)
        ));
        assert_eq!(files.node("/home/root/Work/x"), None);
    }

    #[test]
    fn a_delete_removes_everything_and_counts_entries() {
        let mut files = tree();
        let mut job = Job::delete(&path("/home/root/Work"));
        let mut buffer = vec![0u8; STEP_BYTES];
        let mut seen = Vec::new();
        while job.step(&mut files, &mut buffer) == Step::Working {
            seen.push(job.permille());
        }
        assert_eq!(files.node("/home/root/Work"), None);
        assert!(files.node("/home/root").is_some());
        assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]));
        let job = Job::delete(&path("/home/root/none"));
        assert!(matches!(
            run(&mut files, job),
            Step::Failed(Errno::ENOENT, _)
        ));
    }

    #[test]
    fn a_cancelled_copy_keeps_what_is_done_and_removes_the_half_written_file() {
        let mut files = tree();
        let mut job = Job::copy(&path("/home/root/Work"), &path("/tmp/Work"));
        let mut buffer = vec![0u8; STEP_BYTES];
        while files
            .contents("/tmp/Work/a.txt")
            .is_none_or(|bytes| bytes.is_empty())
        {
            assert_eq!(job.step(&mut files, &mut buffer), Step::Working);
        }
        assert!(job.permille() > 0);
        job.cancel(&mut files);
        assert_eq!(files.node("/tmp/Work/a.txt"), None);
        assert_eq!(files.node("/tmp/Work"), Some(&Node::Folder));
        assert_eq!(files.open_count(), 0);
        assert_eq!(job.step(&mut files, &mut buffer), Step::Done);
    }

    #[test]
    fn links_are_copied_moved_and_deleted_as_links_and_never_descended_into() {
        let mut files = tree();
        files
            .file("/home/root/Keep/k.txt", b"k", 1)
            .link("/home/root/Work/out", "/home/root/Keep", 1)
            .link("/home/root/Work/near", "a.txt", 1);
        let job = Job::copy(&path("/home/root/Work"), &path("/tmp/Work"));
        assert_eq!(run(&mut files, job), Step::Done);
        assert_eq!(
            files.node("/tmp/Work/out"),
            Some(&Node::Link(b"/home/root/Keep".to_vec()))
        );
        assert_eq!(
            files.node("/tmp/Work/near"),
            Some(&Node::Link(b"a.txt".to_vec()))
        );
        assert_eq!(files.node("/tmp/Work/out/k.txt"), None);
        let job = Job::moving(&path("/home/root/Work/near"), &path("/tmp/near"));
        assert_eq!(run(&mut files, job), Step::Done);
        assert_eq!(
            files.node("/tmp/near"),
            Some(&Node::Link(b"a.txt".to_vec()))
        );
        assert_eq!(files.node("/home/root/Work/near"), None);
        let job = Job::delete(&path("/home/root/Work"));
        assert_eq!(run(&mut files, job), Step::Done);
        assert_eq!(files.node("/home/root/Work"), None);
        assert_eq!(files.contents("/home/root/Keep/k.txt"), Some(&b"k"[..]));
    }

    #[test]
    fn a_full_disk_stops_the_copy_and_leaves_no_half_file() {
        let mut files = tree();
        files.space = Some(250_000);
        let job = Job::copy(
            &path("/home/root/Work/a.txt"),
            &path("/home/root/a copy.txt"),
        );
        assert_eq!(
            run(&mut files, job),
            Step::Failed(Errno::ENOSPC, path("/home/root/a copy.txt"))
        );
        assert_eq!(files.node("/home/root/a copy.txt"), None);
    }
}
