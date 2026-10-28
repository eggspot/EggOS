use crate::disk::SystemFiles;
use crate::ls::record_names;
use crate::options::{Item, parse};
use crate::output::{FAILURE, Output};
use crate::system::{SymbolicLinks, System};
use core::ffi::CStr;
use eggos_app::files::Path as FilesPath;
use eggos_files::job::{Job, STEP_BYTES, Step};
use eggos_runtime::errno::Errno;

const PATH_CAPACITY: usize = 4096;

const COPY_CHUNK: usize = 16 * 1024;

const ENTRIES_CHUNK: usize = 2048;

const NEW_DIRECTORY_MODE: u32 = 0o777;

const NEW_FILE_MODE: u32 = 0o666;

struct Path {
    bytes: [u8; PATH_CAPACITY],
    length: usize,
}

impl Path {
    fn of(path: &[u8]) -> Option<Self> {
        let mut built = Self {
            bytes: [0; PATH_CAPACITY],
            length: 0,
        };
        built.push(path)?;
        Some(built)
    }

    fn push(&mut self, part: &[u8]) -> Option<()> {
        let end = self.length + part.len();
        if end >= PATH_CAPACITY || part.contains(&0) {
            return None;
        }
        self.bytes[self.length..end].copy_from_slice(part);
        self.length = end;
        self.bytes[end] = 0;
        Some(())
    }

    fn join(directory: &[u8], name: &[u8]) -> Option<Self> {
        let mut path = Self::of(directory)?;
        if !directory.ends_with(b"/") {
            path.push(b"/")?;
        }
        path.push(name)?;
        Some(path)
    }

    fn as_c_str(&self) -> &CStr {
        CStr::from_bytes_with_nul(&self.bytes[..=self.length]).unwrap_or(c"")
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

fn base_name(path: &[u8]) -> &[u8] {
    let trimmed = match path.iter().rposition(|&byte| byte != b'/') {
        Some(end) => &path[..=end],
        None => return b"/",
    };
    trimmed
        .rsplit(|&byte| byte == b'/')
        .next()
        .unwrap_or(trimmed)
}

fn is_directory<S: System>(output: &mut Output<S>, path: &CStr) -> bool {
    output
        .system
        .status(None, path, SymbolicLinks::Follow)
        .is_ok_and(|status| status.is_directory())
}

fn failed<S: System>(output: &mut Output<S>, what: &[u8], path: &[u8], errno: Errno) -> i32 {
    output.error(&[what, b" '", path, b"': ", errno.message()]);
    FAILURE
}

pub fn mkdir<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut parents = false;
    let Ok(operands) = parse(output, arguments, b"p", b"", |_, _| {
        parents = true;
        Ok(())
    }) else {
        return FAILURE;
    };
    if operands.all().is_empty() {
        output.error(&[b"missing operand"]);
        return FAILURE;
    }
    let mut status = 0;
    for &operand in operands.all() {
        let result = match parents {
            true => make_with_parents(output, operand.to_bytes()),
            false => output.system.make_directory(operand, NEW_DIRECTORY_MODE),
        };
        if let Err(errno) = result {
            status = failed(
                output,
                b"cannot create directory",
                operand.to_bytes(),
                errno,
            );
        }
    }
    status
}

fn make_with_parents<S: System>(output: &mut Output<S>, path: &[u8]) -> Result<(), Errno> {
    let mut end = 0;
    while end < path.len() {
        end = path[end + 1..]
            .iter()
            .position(|&byte| byte == b'/')
            .map_or(path.len(), |at| end + 1 + at);
        let prefix = Path::of(&path[..end]).ok_or(Errno::ENAMETOOLONG)?;
        match output
            .system
            .make_directory(prefix.as_c_str(), NEW_DIRECTORY_MODE)
        {
            Ok(()) => {}
            Err(Errno::EEXIST) if is_directory(output, prefix.as_c_str()) => {}
            Err(errno) => return Err(errno),
        }
    }
    Ok(())
}

pub fn touch<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut create = true;
    let Ok(operands) = parse(output, arguments, b"c", b"", |_, _| {
        create = false;
        Ok(())
    }) else {
        return FAILURE;
    };
    if operands.all().is_empty() {
        output.error(&[b"missing file operand"]);
        return FAILURE;
    }
    let mut status = 0;
    for &operand in operands.all() {
        let result = match output.system.set_modified_now(operand) {
            Err(Errno::ENOENT) if create => output
                .system
                .create(operand, NEW_FILE_MODE, false)
                .map(|descriptor| output.system.close(descriptor)),
            Err(Errno::ENOENT) => Ok(()),
            result => result,
        };
        if let Err(errno) = result {
            status = failed(output, b"cannot touch", operand.to_bytes(), errno);
        }
    }
    status
}

pub fn rm<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let (mut recursive, mut force) = (false, false);
    let Ok(operands) = parse(output, arguments, b"rRf", b"", |_, item| {
        match item {
            Item::Flag(b'f') => force = true,
            _ => recursive = true,
        }
        Ok(())
    }) else {
        return FAILURE;
    };
    if operands.all().is_empty() && !force {
        output.error(&[b"missing operand"]);
        return FAILURE;
    }
    let mut status = 0;
    for &operand in operands.all() {
        let path = operand.to_bytes();
        if matches!(base_name(path), b"." | b"..") {
            output.error(&[
                b"refusing to remove '.' or '..' directory: skipping '",
                path,
                b"'",
            ]);
            status = FAILURE;
            continue;
        }
        let result = match output.system.remove(operand, false) {
            Err(Errno::EISDIR) if recursive => remove_tree(output, path),
            result => result,
        };
        match result {
            Ok(()) => {}
            Err(Errno::ENOENT) if force => {}
            Err(errno) => status = failed(output, b"cannot remove", path, errno),
        }
    }
    status
}

fn remove_tree<S: System>(output: &mut Output<S>, directory: &[u8]) -> Result<(), Errno> {
    let path = Path::of(directory).ok_or(Errno::ENAMETOOLONG)?;
    loop {
        let descriptor = output.system.open(path.as_c_str())?;
        let mut chunk = [0u8; ENTRIES_CHUNK];
        let read = output.system.directory_entries(descriptor, &mut chunk);
        output.system.close(descriptor);
        let length = read?;
        let mut removed_any = false;
        for name in record_names(&chunk[..length]) {
            if matches!(name, b"." | b"..") {
                continue;
            }
            let child = Path::join(directory, name).ok_or(Errno::ENAMETOOLONG)?;
            match output.system.remove(child.as_c_str(), false) {
                Err(Errno::EISDIR) => remove_tree(output, child.as_bytes()),
                result => result,
            }?;
            removed_any = true;
        }
        if !removed_any {
            return output.system.remove(path.as_c_str(), true);
        }
    }
}

fn target_for(source: &[u8], target: &CStr, into_directory: bool) -> Option<Path> {
    match into_directory {
        true => Path::join(target.to_bytes(), base_name(source)),
        false => Path::of(target.to_bytes()),
    }
}

fn sources_and_target<'a, S: System>(
    output: &mut Output<S>,
    operands: &'a [&'a CStr],
) -> Option<(&'a [&'a CStr], &'a CStr, bool)> {
    let Some((&target, sources)) = operands.split_last() else {
        output.error(&[b"missing file operand"]);
        return None;
    };
    if sources.is_empty() {
        output.error(&[
            b"missing destination file operand after '",
            target.to_bytes(),
            b"'",
        ]);
        return None;
    }
    let into_directory = is_directory(output, target);
    if sources.len() > 1 && !into_directory {
        output.error(&[b"target '", target.to_bytes(), b"' is not a directory"]);
        return None;
    }
    Some((sources, target, into_directory))
}

fn run_job<S: System>(output: &mut Output<S>, mut job: Job, what: &[u8]) -> Result<(), ()> {
    let mut buffer = [0u8; STEP_BYTES];
    match job.run(&mut SystemFiles::new(&mut output.system), &mut buffer) {
        Step::Failed(errno, path) => {
            failed(output, what, path.as_bytes(), errno);
            Err(())
        }
        Step::Working | Step::Done => Ok(()),
    }
}

fn job_paths(from: &[u8], to: &[u8]) -> Option<(FilesPath, FilesPath)> {
    Some((FilesPath::from(from)?, FilesPath::from(to)?))
}

pub fn cp<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut recursive = false;
    let Ok(operands) = parse(output, arguments, b"rR", b"", |_, _| {
        recursive = true;
        Ok(())
    }) else {
        return FAILURE;
    };
    let Some((sources, target, into_directory)) = sources_and_target(output, operands.all()) else {
        return FAILURE;
    };
    let mut status = 0;
    for &source in sources {
        if recursive && is_directory(output, source) {
            let Some(to) = target_for(source.to_bytes(), target, into_directory) else {
                status = failed(output, b"cannot copy", source.to_bytes(), Errno::ENAMETOOLONG);
                continue;
            };
            let Some((from, to)) = job_paths(source.to_bytes(), to.as_bytes()) else {
                status = failed(output, b"cannot copy", source.to_bytes(), Errno::ENAMETOOLONG);
                continue;
            };
            if run_job(output, Job::copy(&from, &to), b"cannot copy").is_err() {
                status = FAILURE;
            }
            continue;
        }
        if is_directory(output, source) {
            output.error(&[
                b"-r not specified; omitting directory '",
                source.to_bytes(),
                b"'",
            ]);
            status = FAILURE;
            continue;
        }
        let Some(to) = target_for(source.to_bytes(), target, into_directory) else {
            status = failed(
                output,
                b"cannot create regular file",
                target.to_bytes(),
                Errno::ENAMETOOLONG,
            );
            continue;
        };
        if let Err((what, path, errno)) = copy_file(output, source, to.as_c_str()) {
            status = failed(output, what, path.to_bytes(), errno);
        }
    }
    status
}

fn copy_file<'p, S: System>(
    output: &mut Output<S>,
    from: &'p CStr,
    to: &'p CStr,
) -> Result<(), (&'static [u8], &'p CStr, Errno)> {
    let mode = output
        .system
        .status(None, from, SymbolicLinks::Follow)
        .map_err(|errno| (&b"cannot stat"[..], from, errno))?
        .mode
        & 0o777;
    let source = output
        .system
        .open(from)
        .map_err(|errno| (&b"cannot open"[..], from, errno))?;
    let target = match output.system.create(to, mode, true) {
        Ok(target) => target,
        Err(errno) => {
            output.system.close(source);
            return Err((b"cannot create regular file", to, errno));
        }
    };
    let mut chunk = [0u8; COPY_CHUNK];
    let result = loop {
        match output.system.read(source, &mut chunk) {
            Ok(0) => break Ok(()),
            Ok(length) => {
                if let Err(errno) = output.system.write(target, &chunk[..length]) {
                    break Err((&b"error writing"[..], to, errno));
                }
            }
            Err(Errno::EINTR) => {}
            Err(errno) => break Err((b"error reading", from, errno)),
        }
    };
    output.system.close(source);
    output.system.close(target);
    result
}

pub fn mv<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Ok(operands) = parse(output, arguments, b"f", b"", |_, _| Ok(())) else {
        return FAILURE;
    };
    let Some((sources, target, into_directory)) = sources_and_target(output, operands.all()) else {
        return FAILURE;
    };
    let mut status = 0;
    for &source in sources {
        let Some(to) = target_for(source.to_bytes(), target, into_directory) else {
            status = failed(
                output,
                b"cannot move",
                source.to_bytes(),
                Errno::ENAMETOOLONG,
            );
            continue;
        };
        let result = match output.system.rename(source, to.as_c_str()) {
            Err(Errno::EXDEV) if is_directory(output, source) => {
                let Some((from, to)) = job_paths(source.to_bytes(), to.as_bytes()) else {
                    status = failed(output, b"cannot move", source.to_bytes(), Errno::ENAMETOOLONG);
                    continue;
                };
                if run_job(output, Job::moving(&from, &to), b"cannot move").is_err() {
                    status = FAILURE;
                }
                continue;
            }
            Err(Errno::EXDEV) => copy_file(output, source, to.as_c_str())
                .map_err(|(_, _, errno)| errno)
                .and_then(|()| output.system.remove(source, false)),
            result => result,
        };
        if let Err(errno) = result {
            output.error(&[
                b"cannot move '",
                source.to_bytes(),
                b"' to '",
                to.as_bytes(),
                b"': ",
                errno.message(),
            ]);
            status = FAILURE;
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Fake, Node};

    fn files() -> Fake {
        Fake::default()
            .with_directory("/tmp", 0)
            .with_directory("/home", 0)
            .with_file("/tmp/x", 0o640, b"a\nb\n", 0)
            .with_directory("/etc", 0)
            .with_file("/etc/motd", 0o644, b"hi\n", 0)
    }

    fn run(
        utility: fn(&mut Output<Fake>, &[&CStr]) -> i32,
        fake: Fake,
        arguments: &[&CStr],
    ) -> (i32, Fake) {
        let mut fake = fake;
        fake.standard_error.clear();
        let mut output = Output::new(fake, b"t");
        let status = utility(&mut output, arguments);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0, "a descriptor was left open");
        (status, output.system)
    }

    fn contents(fake: &Fake, path: &str) -> Option<Vec<u8>> {
        match fake.nodes.get(path.as_bytes()) {
            Some((Node::File(content), _)) => Some(content.clone()),
            _ => None,
        }
    }

    #[test]
    fn mkdir_makes_directories_and_with_p_their_parents() {
        let (status, fake) = run(mkdir, files(), &[c"/home/u"]);
        assert_eq!(status, 0);
        assert!(matches!(
            fake.nodes.get(&b"/home/u"[..]),
            Some((Node::Directory, _))
        ));
        let (status, fake) = run(mkdir, fake, &[c"/home/u", c"/no/such"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: cannot create directory '/home/u': File exists\n\
             t: cannot create directory '/no/such': No such file or directory\n"
        );
        let (status, fake) = run(mkdir, fake, &[c"-p", c"/home/u/a/b", c"/home"]);
        assert_eq!(status, 0);
        assert!(fake.nodes.contains_key(&b"/home/u/a/b"[..]));
        let (status, _) = run(mkdir, files(), &[c"-p", c"/tmp/x/y"]);
        assert_eq!(status, 1);
    }

    #[test]
    fn touch_creates_missing_files_and_dates_existing_ones() {
        let (status, fake) = run(touch, files(), &[c"/home/f", c"/tmp/x"]);
        assert_eq!(status, 0);
        assert_eq!(contents(&fake, "/home/f"), Some(Vec::new()));
        assert_eq!(contents(&fake, "/tmp/x"), Some(b"a\nb\n".to_vec()));
        assert_eq!(fake.nodes[&b"/tmp/x"[..]].1.modified, 1_700_000_000);
        let (status, fake) = run(touch, files(), &[c"-c", c"/home/none", c"/no/f"]);
        assert_eq!(status, 0);
        assert!(!fake.nodes.contains_key(&b"/home/none"[..]));
        assert_eq!(fake.errors(), "");
        let (status, fake) = run(touch, files(), &[c"/no/f"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: cannot touch '/no/f': No such file or directory\n"
        );
    }

    #[test]
    fn cp_copies_files_into_names_or_directories() {
        let (status, fake) = run(cp, files(), &[c"/tmp/x", c"/home/y"]);
        assert_eq!(status, 0);
        assert_eq!(contents(&fake, "/home/y"), Some(b"a\nb\n".to_vec()));
        assert_eq!(fake.nodes[&b"/home/y"[..]].1.mode & 0o777, 0o640);
        let (status, fake) = run(cp, fake, &[c"/tmp/x", c"/etc/motd", c"/home"]);
        assert_eq!(status, 0);
        assert_eq!(contents(&fake, "/home/x"), Some(b"a\nb\n".to_vec()));
        assert_eq!(contents(&fake, "/home/motd"), Some(b"hi\n".to_vec()));
        let (status, fake) = run(cp, fake, &[c"/etc/motd", c"/home/y"]);
        assert_eq!(status, 0);
        assert_eq!(contents(&fake, "/home/y"), Some(b"hi\n".to_vec()));
        let (status, fake) = run(cp, files(), &[c"/tmp", c"/home/d"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: -r not specified; omitting directory '/tmp'\n"
        );
        let (status, fake) = run(cp, files(), &[c"-r", c"/etc", c"/home"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(contents(&fake, "/home/etc/motd"), Some(b"hi\n".to_vec()));
        assert_eq!(contents(&fake, "/etc/motd"), Some(b"hi\n".to_vec()));
        let (status, fake) = run(cp, fake, &[c"-R", c"/etc", c"/home/etc2"]);
        assert_eq!(status, 0);
        assert_eq!(contents(&fake, "/home/etc2/motd"), Some(b"hi\n".to_vec()));
        let (status, fake) = run(cp, fake, &[c"-r", c"/etc", c"/home"]);
        assert_eq!(status, 1);
        assert_eq!(fake.errors(), "t: cannot copy '/home/etc': File exists\n");
        let (status, fake) = run(cp, files(), &[c"/tmp/x", c"/etc/motd", c"/tmp/x"]);
        assert_eq!(status, 1);
        assert_eq!(fake.errors(), "t: target '/tmp/x' is not a directory\n");
        let (status, fake) = run(cp, files(), &[c"/nope", c"/home/z"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: cannot stat '/nope': No such file or directory\n"
        );
        let (_, fake) = run(cp, files(), &[c"/tmp/x"]);
        assert_eq!(
            fake.errors(),
            "t: missing destination file operand after '/tmp/x'\n"
        );
    }

    #[test]
    fn mv_renames_and_copies_across_file_systems() {
        let (status, fake) = run(mv, files(), &[c"/tmp/x", c"/tmp/z"]);
        assert_eq!(status, 0);
        assert_eq!(contents(&fake, "/tmp/z"), Some(b"a\nb\n".to_vec()));
        assert_eq!(contents(&fake, "/tmp/x"), None);
        let (status, fake) = run(mv, fake, &[c"/tmp/z", c"/tmp/"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        let mut across = files();
        across.cross_device = vec![b"/home".to_vec()];
        let (status, fake) = run(mv, across, &[c"/tmp/x", c"/home"]);
        assert_eq!(status, 0);
        assert_eq!(contents(&fake, "/home/x"), Some(b"a\nb\n".to_vec()));
        assert_eq!(contents(&fake, "/tmp/x"), None);
        let mut across = files().with_directory("/tmp/d", 0).with_file("/tmp/d/f", 0o600, b"f", 0);
        across.cross_device = vec![b"/home".to_vec()];
        let (status, fake) = run(mv, across, &[c"/tmp/d", c"/home"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(contents(&fake, "/home/d/f"), Some(b"f".to_vec()));
        assert!(!fake.nodes.contains_key(&b"/tmp/d"[..]));
        let (status, fake) = run(mv, files(), &[c"/nope", c"/home/a"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: cannot move '/nope' to '/home/a': No such file or directory\n"
        );
    }

    #[test]
    fn rm_removes_files_and_with_r_whole_trees() {
        let tree = files()
            .with_directory("/home/u", 0)
            .with_directory("/home/u/d", 0)
            .with_file("/home/u/d/f", 0o644, b"", 0)
            .with_file("/home/u/g", 0o644, b"", 0);
        let (status, fake) = run(rm, tree, &[c"/tmp/x", c"/home/u"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: cannot remove '/home/u': Is a directory\n"
        );
        assert_eq!(contents(&fake, "/tmp/x"), None);
        let (status, fake) = run(rm, fake, &[c"-r", c"/home/u"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert!(!fake.nodes.keys().any(|path| path.starts_with(b"/home/u")));
        let (status, fake) = run(rm, fake, &[c"/missing"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: cannot remove '/missing': No such file or directory\n"
        );
        let (status, fake) = run(rm, files(), &[c"-f", c"/missing"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(run(rm, files(), &[c"-f"]).0, 0);
        let (status, fake) = run(rm, files(), &[c"-r", c"/tmp/.."]);
        assert_eq!(status, 1);
        assert!(
            fake.errors()
                .starts_with("t: refusing to remove '.' or '..'")
        );
    }
}
