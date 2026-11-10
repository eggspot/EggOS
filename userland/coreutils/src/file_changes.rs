use crate::disk::{ABSOLUTE_CAPACITY, SystemFiles, absolute_path};
use crate::ls::record_names;
use crate::options::parse_defined;
use crate::output::{FAILURE, Output, error_for};
use crate::system::{SymbolicLinks, System, folder_and_leaf};
use core::ffi::CStr;
use eggos_app::files::Path as FilesPath;
use eggos_commands::{commands, errors};
use eggos_files::job::{Job, Preview, STEP_BYTES, Step};
use eggos_files::names::{is_within, normalized};
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
    output.coded(
        error_for(errno),
        &[what, b" '", path, b"': ", errno.message()],
    );
    FAILURE
}

pub fn mkdir<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut parents = false;
    let Ok(operands) = parse_defined(output, arguments, &commands::MKDIR, |_, _| {
        parents = true;
        Ok(())
    }) else {
        return FAILURE;
    };
    if operands.all().is_empty() {
        output.coded(&errors::USAGE, &[b"missing operand"]);
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
    let Ok(operands) = parse_defined(output, arguments, &commands::TOUCH, |_, _| {
        create = false;
        Ok(())
    }) else {
        return FAILURE;
    };
    if operands.all().is_empty() {
        output.coded(&errors::USAGE, &[b"missing file operand"]);
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
    let (mut recursive, mut force, mut dry_run) = (false, false, false);
    let Ok(operands) = parse_defined(output, arguments, &commands::RM, |_, given| {
        match given.option.short {
            Some(b'f') => force = true,
            Some(_) => recursive = true,
            None => dry_run = true,
        }
        Ok(())
    }) else {
        return FAILURE;
    };
    if operands.all().is_empty() && !force {
        output.coded(&errors::USAGE, &[b"missing operand"]);
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
        if dry_run {
            if preview_removal(output, operand, recursive, force).is_err() {
                status = FAILURE;
            }
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
        output.coded(&errors::USAGE, &[b"missing file operand"]);
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
        output.coded(
            &errors::NOT_A_FOLDER,
            &[b"target '", target.to_bytes(), b"' is not a directory"],
        );
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

fn job_paths<S: System>(system: &mut S, from: &[u8], to: &[u8]) -> Option<(FilesPath, FilesPath)> {
    let mut absolute = [0u8; ABSOLUTE_CAPACITY];
    let length = absolute_path(system, from, &mut absolute)?;
    let from = normalized(&absolute[..length])?;
    let length = absolute_path(system, to, &mut absolute)?;
    Some((from, normalized(&absolute[..length])?))
}

fn into_itself<S: System>(
    output: &mut Output<S>,
    from: &FilesPath,
    to: &FilesPath,
    source: &[u8],
    target: &[u8],
) -> bool {
    let inside = is_within(to.as_bytes(), from.as_bytes());
    if inside {
        output.error(&[
            b"cannot copy a directory, '",
            source,
            b"', into itself, '",
            target,
            b"'",
        ]);
    }
    inside
}

pub fn cp<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let (mut recursive, mut dry_run) = (false, false);
    let Ok(operands) = parse_defined(output, arguments, &commands::CP, |_, given| {
        match given.option.short {
            Some(_) => recursive = true,
            None => dry_run = true,
        }
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
                status = failed(
                    output,
                    b"cannot copy",
                    source.to_bytes(),
                    Errno::ENAMETOOLONG,
                );
                continue;
            };
            let target_path = to;
            let Some((from, to)) = job_paths(
                &mut output.system,
                source.to_bytes(),
                target_path.as_bytes(),
            ) else {
                status = failed(
                    output,
                    b"cannot copy",
                    source.to_bytes(),
                    Errno::ENAMETOOLONG,
                );
                continue;
            };
            if into_itself(
                output,
                &from,
                &to,
                source.to_bytes(),
                target_path.as_bytes(),
            ) {
                status = FAILURE;
                continue;
            }
            let job = Job::copy(&from, &to);
            let copied = match dry_run {
                true => preview_tree(output, &job, source, target_path.as_bytes(), COPYING),
                false => run_job(output, job, COPYING.failure),
            };
            if copied.is_err() {
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
        if is_same_file(output, source, to.as_c_str(), SymbolicLinks::Follow) {
            same_file(output, source.to_bytes(), to.as_bytes());
            status = FAILURE;
            continue;
        }
        let copied = match dry_run {
            true => preview_file_copy(output, source, to.as_c_str()),
            false => copy_file(output, source, to.as_c_str()),
        };
        if let Err((what, path, errno)) = copied {
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
    let mut dry_run = false;
    let Ok(operands) = parse_defined(output, arguments, &commands::MV, |_, given| {
        dry_run |= given.is_long("dry-run");
        Ok(())
    }) else {
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
        if dry_run {
            if preview_move(output, source, &to).is_err() {
                status = FAILURE;
            }
            continue;
        }
        let result = match output.system.rename(source, to.as_c_str()) {
            Err(Errno::EXDEV) if is_directory(output, source) => {
                let Some((from, to)) =
                    job_paths(&mut output.system, source.to_bytes(), to.as_bytes())
                else {
                    status = failed(
                        output,
                        b"cannot move",
                        source.to_bytes(),
                        Errno::ENAMETOOLONG,
                    );
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
            status = move_failed(output, source.to_bytes(), to.as_bytes(), errno);
        }
    }
    status
}

fn move_failed<S: System>(
    output: &mut Output<S>,
    source: &[u8],
    target: &[u8],
    errno: Errno,
) -> i32 {
    output.error(&[
        b"cannot move '",
        source,
        b"' to '",
        target,
        b"': ",
        errno.message(),
    ]);
    FAILURE
}

struct TreeWords {
    failure: &'static [u8],
    doing: &'static [u8],
    takes_the_source: bool,
}

const COPYING: TreeWords = TreeWords {
    failure: b"cannot copy",
    doing: b"would copy ",
    takes_the_source: false,
};

const MOVING: TreeWords = TreeWords {
    failure: b"cannot move",
    doing: b"would move ",
    takes_the_source: true,
};

type Refusal<'p> = (&'static [u8], &'p CStr, Errno);

fn is_regular(status: &crate::system::Status) -> bool {
    !status.is_directory() && !status.is_link()
}

fn is_same_file<S: System>(
    output: &mut Output<S>,
    first: &CStr,
    second: &CStr,
    links: SymbolicLinks,
) -> bool {
    match (
        output.system.status(None, first, links),
        output.system.status(None, second, links),
    ) {
        (Ok(first), Ok(second)) => first.is_same_file(&second),
        _ => false,
    }
}

fn same_file<S: System>(output: &mut Output<S>, source: &[u8], target: &[u8]) {
    output.error(&[b"'", source, b"' and '", target, b"' are the same file"]);
}

fn write_quoted<S: System>(output: &mut Output<S>, path: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.write(b"'");
    for &byte in path {
        match byte {
            b'\\' => output.write(b"\\\\"),
            byte if byte < b' ' || byte == 0x7f => output.write(&[
                b'\\',
                b'x',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0xf)],
            ]),
            byte => output.write(&[byte]),
        }
    }
    output.write(b"'");
}

fn write_count<S: System>(output: &mut Output<S>, count: u64, one: &[u8], many: &[u8]) {
    output.write_number(count, 0);
    output.write(b" ");
    output.write(if count == 1 { one } else { many });
}

fn write_size<S: System>(output: &mut Output<S>, bytes: u64) {
    output.write(b" (");
    write_count(output, bytes, b"byte", b"bytes");
    output.write(b")");
}

fn write_tree<S: System>(output: &mut Output<S>, tree: &Preview) {
    output.write(b" (");
    write_count(output, tree.files, b"file", b"files");
    output.write(b", ");
    write_count(output, tree.folders, b"folder", b"folders");
    output.write(b", ");
    write_count(output, tree.bytes, b"byte", b"bytes");
    output.write(b")");
}

fn write_replaced<S: System>(output: &mut Output<S>, replaced: Option<u64>) {
    if let Some(size) = replaced {
        output.write(b", replacing the file there");
        write_size(output, size);
    }
}

fn folder_for<S: System>(output: &mut Output<S>, path: &[u8]) -> Result<(), Errno> {
    let folder = Path::of(folder_and_leaf(path).0).ok_or(Errno::ENAMETOOLONG)?;
    match output
        .system
        .status(None, folder.as_c_str(), SymbolicLinks::Follow)?
        .is_directory()
    {
        true => output.system.may_write(folder.as_c_str()),
        false => Err(Errno::ENOTDIR),
    }
}

fn may_write_in<S: System>(output: &mut Output<S>, path: &[u8]) -> Result<(), Errno> {
    let folder = Path::of(folder_and_leaf(path).0).ok_or(Errno::ENAMETOOLONG)?;
    output.system.may_write(folder.as_c_str())
}

fn may_change_in<S: System>(output: &mut Output<S>, path: &[u8], needs: u64) -> Result<(), Errno> {
    may_write_in(output, path)?;
    has_room(output, path, needs)
}

fn can_write<S: System>(output: &mut Output<S>, path: &[u8], needs: u64) -> Result<(), Errno> {
    let folder = Path::of(folder_and_leaf(path).0).ok_or(Errno::ENAMETOOLONG)?;
    match output.system.file_system_usage(folder.as_c_str()) {
        Ok(usage) if usage.read_only => Err(Errno::EROFS),
        Ok(usage) if needs > usage.available_bytes() => Err(Errno::ENOSPC),
        _ => Ok(()),
    }
}

fn file_to_copy<'p, S: System>(output: &mut Output<S>, from: &'p CStr) -> Result<u64, Refusal<'p>> {
    let size = output
        .system
        .status(None, from, SymbolicLinks::Follow)
        .map_err(|errno| (&b"cannot stat"[..], from, errno))?
        .size;
    let descriptor = output
        .system
        .open(from)
        .map_err(|errno| (&b"cannot open"[..], from, errno))?;
    output.system.close(descriptor);
    Ok(size)
}

fn file_to_replace<S: System>(output: &mut Output<S>, to: &CStr) -> Result<Option<u64>, Errno> {
    match output.system.status(None, to, SymbolicLinks::Follow) {
        Ok(status) if status.is_directory() => Err(Errno::EISDIR),
        Ok(status) => output.system.may_write(to).map(|()| Some(status.size)),
        Err(Errno::ENOENT) => folder_for(output, to.to_bytes()).map(|()| None),
        Err(errno) => Err(errno),
    }
}

fn preview_file_copy<'p, S: System>(
    output: &mut Output<S>,
    from: &'p CStr,
    to: &'p CStr,
) -> Result<(), Refusal<'p>> {
    let size = file_to_copy(output, from)?;
    let replaced = file_to_replace(output, to)
        .map_err(|errno| (&b"cannot create regular file"[..], to, errno))?;
    let needs = size.saturating_sub(replaced.unwrap_or(0));
    has_room(output, to.to_bytes(), needs).map_err(|errno| match errno {
        Errno::ENOSPC => (&b"error writing"[..], to, errno),
        errno => (&b"cannot create regular file"[..], to, errno),
    })?;
    output.write(b"would copy ");
    write_quoted(output, from.to_bytes());
    output.write(b" to ");
    write_quoted(output, to.to_bytes());
    write_size(output, size);
    write_replaced(output, replaced);
    output.write(b"\n");
    Ok(())
}

fn preview_tree<S: System>(
    output: &mut Output<S>,
    job: &Job,
    source: &CStr,
    target: &[u8],
    words: TreeWords,
) -> Result<(), ()> {
    let tree = match job.preview(&mut SystemFiles::new(&mut output.system)) {
        Ok(tree) => tree,
        Err((errno, path)) => {
            failed(output, words.failure, path.as_bytes(), errno);
            return Err(());
        }
    };
    let mut checked = may_change_in(output, job.target().as_bytes(), tree.bytes)
        .map_err(|errno| (errno, *job.target()));
    if checked.is_ok() && words.takes_the_source {
        checked = may_change_in(output, job.source().as_bytes(), 0)
            .map_err(|errno| (errno, *job.source()));
    }
    if let Err((errno, path)) = checked {
        failed(output, words.failure, path.as_bytes(), errno);
        return Err(());
    }
    let folder = output
        .system
        .status(None, source, SymbolicLinks::DoNotFollow)
        .is_ok_and(|status| status.is_directory());
    output.write(words.doing);
    if folder {
        output.write(b"the folder ");
    }
    write_quoted(output, source.to_bytes());
    output.write(b" to ");
    write_quoted(output, target);
    write_tree(output, &tree);
    output.write(b"\n");
    Ok(())
}

fn holds_entries<S: System>(output: &mut Output<S>, folder: &CStr) -> Result<bool, Errno> {
    let descriptor = output.system.open(folder)?;
    let mut chunk = [0u8; ENTRIES_CHUNK];
    let found = loop {
        match output.system.directory_entries(descriptor, &mut chunk) {
            Ok(0) => break Ok(false),
            Ok(length) => {
                if record_names(&chunk[..length]).any(|name| !matches!(name, b"." | b"..")) {
                    break Ok(true);
                }
            }
            Err(errno) => break Err(errno),
        }
    };
    output.system.close(descriptor);
    found
}

fn preview_move<S: System>(output: &mut Output<S>, source: &CStr, to: &Path) -> Result<(), ()> {
    let refuse = |output: &mut Output<S>, errno: Errno| {
        move_failed(output, source.to_bytes(), to.as_bytes(), errno);
        Err(())
    };
    let moved = match output
        .system
        .status(None, source, SymbolicLinks::DoNotFollow)
    {
        Ok(status) => status,
        Err(errno) => return refuse(output, errno),
    };
    if is_same_file(output, source, to.as_c_str(), SymbolicLinks::DoNotFollow) {
        output.write(b"would leave ");
        write_quoted(output, source.to_bytes());
        output.write(b" as it is: ");
        write_quoted(output, to.as_bytes());
        output.write(b" is the same file\n");
        return Ok(());
    }
    let Some((from, to_path)) = job_paths(&mut output.system, source.to_bytes(), to.as_bytes())
    else {
        return refuse(output, Errno::ENAMETOOLONG);
    };
    if moved.is_directory() && is_within(to_path.as_bytes(), from.as_bytes()) {
        return refuse(output, Errno::EINVAL);
    }
    let Some(folder) = Path::of(folder_and_leaf(to.as_bytes()).0) else {
        return refuse(output, Errno::ENAMETOOLONG);
    };
    let into = match output
        .system
        .status(None, folder.as_c_str(), SymbolicLinks::Follow)
    {
        Ok(status) if status.is_directory() => status,
        Ok(_) => return refuse(output, Errno::ENOTDIR),
        Err(errno) => return refuse(output, errno),
    };
    if into.device != moved.device {
        return preview_move_across(output, source, to, &from, &to_path);
    }
    let replaced = match output
        .system
        .status(None, to.as_c_str(), SymbolicLinks::DoNotFollow)
    {
        Err(Errno::ENOENT) => None,
        Err(errno) => return refuse(output, errno),
        Ok(there) => match (moved.is_directory(), there.is_directory()) {
            (true, true) => match holds_entries(output, to.as_c_str()) {
                Ok(false) => Some(None),
                Ok(true) => return refuse(output, Errno::ENOTEMPTY),
                Err(errno) => return refuse(output, errno),
            },
            (true, false) => return refuse(output, Errno::ENOTDIR),
            (false, true) => return refuse(output, Errno::EISDIR),
            (false, false) => Some(is_regular(&there).then_some(there.size)),
        },
    };
    for path in [source.to_bytes(), to.as_bytes()] {
        if let Err(errno) = may_change_in(output, path, 0) {
            return refuse(output, errno);
        }
    }
    output.write(b"would move ");
    if moved.is_directory() {
        output.write(b"the folder ");
    }
    write_quoted(output, source.to_bytes());
    output.write(b" to ");
    write_quoted(output, to.as_bytes());
    if is_regular(&moved) {
        write_size(output, moved.size);
    }
    match replaced {
        Some(Some(size)) => write_replaced(output, Some(size)),
        Some(None) if moved.is_directory() => output.write(b", replacing the empty folder there"),
        Some(None) => output.write(b", replacing the link there"),
        None => {}
    }
    output.write(b"\n");
    Ok(())
}

fn preview_move_across<S: System>(
    output: &mut Output<S>,
    source: &CStr,
    to: &Path,
    from: &FilesPath,
    to_path: &FilesPath,
) -> Result<(), ()> {
    if is_directory(output, source) {
        let job = Job::moving(from, to_path);
        return preview_tree(output, &job, source, to.as_bytes(), MOVING);
    }
    let refuse = |output: &mut Output<S>, errno: Errno| {
        move_failed(output, source.to_bytes(), to.as_bytes(), errno);
        Err(())
    };
    let size = match file_to_copy(output, source) {
        Ok(size) => size,
        Err((_, _, errno)) => return refuse(output, errno),
    };
    let replaced = match file_to_replace(output, to.as_c_str()) {
        Ok(replaced) => replaced,
        Err(errno) => return refuse(output, errno),
    };
    let needs = size.saturating_sub(replaced.unwrap_or(0));
    let checked = has_room(output, to.as_bytes(), needs)
        .and_then(|()| may_change_in(output, source.to_bytes(), 0));
    if let Err(errno) = checked {
        return refuse(output, errno);
    }
    output.write(b"would move ");
    write_quoted(output, source.to_bytes());
    output.write(b" to ");
    write_quoted(output, to.as_bytes());
    write_size(output, size);
    write_replaced(output, replaced);
    output.write(b"\n");
    Ok(())
}

fn measure_tree<S: System>(
    output: &mut Output<S>,
    directory: &[u8],
    tree: &mut Preview,
) -> Result<(), Errno> {
    let path = Path::of(directory).ok_or(Errno::ENAMETOOLONG)?;
    output.system.may_write(path.as_c_str())?;
    let descriptor = output.system.open(path.as_c_str())?;
    let measured = measure_entries(output, descriptor, directory, tree);
    output.system.close(descriptor);
    measured
}

fn measure_entries<S: System>(
    output: &mut Output<S>,
    descriptor: usize,
    directory: &[u8],
    tree: &mut Preview,
) -> Result<(), Errno> {
    let mut chunk = [0u8; ENTRIES_CHUNK];
    loop {
        let length = output.system.directory_entries(descriptor, &mut chunk)?;
        if length == 0 {
            return Ok(());
        }
        for name in record_names(&chunk[..length]) {
            if matches!(name, b"." | b"..") {
                continue;
            }
            let child = Path::join(directory, name).ok_or(Errno::ENAMETOOLONG)?;
            let status =
                output
                    .system
                    .status(None, child.as_c_str(), SymbolicLinks::DoNotFollow)?;
            if status.is_directory() {
                tree.folders += 1;
                measure_tree(output, child.as_bytes(), tree)?;
                continue;
            }
            tree.files += 1;
            if is_regular(&status) {
                tree.bytes = tree.bytes.saturating_add(status.size);
            }
        }
    }
}

fn preview_removal<S: System>(
    output: &mut Output<S>,
    operand: &CStr,
    recursive: bool,
    force: bool,
) -> Result<(), ()> {
    let path = operand.to_bytes();
    let refuse = |output: &mut Output<S>, errno: Errno| {
        failed(output, b"cannot remove", path, errno);
        Err(())
    };
    let status = match output
        .system
        .status(None, operand, SymbolicLinks::DoNotFollow)
    {
        Ok(status) => status,
        Err(Errno::ENOENT) if force => return Ok(()),
        Err(errno) => return refuse(output, errno),
    };
    if status.is_directory() && !recursive {
        return refuse(output, Errno::EISDIR);
    }
    if let Err(errno) = may_change_in(output, path, 0) {
        return refuse(output, errno);
    }
    if !status.is_directory() {
        output.write(b"would delete ");
        write_quoted(output, path);
        if is_regular(&status) {
            write_size(output, status.size);
        }
        output.write(b" for good\n");
        return Ok(());
    }
    let mut tree = Preview::default();
    if let Err(errno) = measure_tree(output, path, &mut tree) {
        return refuse(output, errno);
    }
    output.write(b"would delete the folder ");
    write_quoted(output, path);
    output.write(b" and everything in it for good");
    write_tree(output, &tree);
    output.write(b"\n");
    Ok(())
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
        let mut inside = files()
            .with_directory("/home/root", 0)
            .with_directory("/home/root/Work", 0)
            .with_file("/home/root/Work/a", 0o644, b"a", 0);
        inside.working_directory = "/home/root".to_owned();
        let (status, fake) = run(cp, inside, &[c"-r", c"Work", c"/home/root/Work/x"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: cannot copy a directory, 'Work', into itself, '/home/root/Work/x'\n"
        );
        assert!(
            !fake
                .nodes
                .keys()
                .any(|path| path.starts_with(b"/home/root/Work/x"))
        );
        let (status, fake) = run(cp, fake, &[c"-r", c"Work", c"./Work/./y/"]);
        assert_eq!(status, 1);
        assert!(fake.errors().contains("into itself"));
        let (status, fake) = run(cp, fake, &[c"-r", c"Work", c"Copy"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(contents(&fake, "/home/root/Copy/a"), Some(b"a".to_vec()));
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
        let mut across = files()
            .with_directory("/tmp/d", 0)
            .with_file("/tmp/d/f", 0o600, b"f", 0);
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

    fn dry(
        utility: fn(&mut Output<Fake>, &[&CStr]) -> i32,
        fake: Fake,
        arguments: &[&CStr],
    ) -> (i32, Fake) {
        let mut fake = fake;
        fake.standard_output.clear();
        let before = snapshot(&fake);
        let (status, fake) = run(utility, fake, arguments);
        assert_eq!(snapshot(&fake), before, "a dry run changed something");
        (status, fake)
    }

    fn snapshot(fake: &Fake) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
        fake.nodes
            .iter()
            .map(|(path, (node, _))| match node {
                Node::File(content) => (path.clone(), Some(content.clone())),
                _ => (path.clone(), None),
            })
            .collect()
    }

    fn sized(fake: Fake, folder: &str, available_blocks: u64, read_only: bool) -> Fake {
        let mut fake = fake;
        fake.file_systems = vec![(
            folder.as_bytes().to_vec(),
            crate::system::FileSystemUsage {
                block_size: 1,
                blocks: 1_000_000,
                free: available_blocks,
                available: available_blocks,
                read_only,
            },
        )];
        fake
    }

    #[test]
    fn cp_dry_run_says_what_it_would_copy_and_replace_and_copies_nothing() {
        let (status, fake) = dry(cp, files(), &[c"--dry-run", c"/tmp/x", c"/home/y"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(
            fake.output(),
            "would copy '/tmp/x' to '/home/y' (4 bytes)\n"
        );
        let (status, fake) = dry(cp, files(), &[c"--dry-run", c"/tmp/x", c"/etc/motd"]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "would copy '/tmp/x' to '/etc/motd' (4 bytes), replacing the file there (3 bytes)\n"
        );
        let (status, fake) = dry(
            cp,
            files(),
            &[c"--dry-run", c"/tmp/x", c"/etc/motd", c"/home"],
        );
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "would copy '/tmp/x' to '/home/x' (4 bytes)\n\
             would copy '/etc/motd' to '/home/motd' (3 bytes)\n"
        );
        let tree = files()
            .with_directory("/etc/skel", 0)
            .with_file("/etc/skel/profile", 0o644, b"export A=1\n", 0)
            .with_link("/etc/latest", "motd");
        let (status, fake) = dry(cp, tree, &[c"--dry-run", c"-r", c"/etc", c"/home"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(
            fake.output(),
            "would copy the folder '/etc' to '/home/etc' (3 files, 1 folder, 14 bytes)\n"
        );
        let named = files().with_file("/tmp/a\nb", 0o644, b"", 0);
        let (_, fake) = dry(cp, named, &[c"--dry-run", c"/tmp/a\nb", c"/home/c\\d"]);
        assert_eq!(
            fake.output(),
            "would copy '/tmp/a\\x0ab' to '/home/c\\\\d' (0 bytes)\n"
        );
    }

    #[test]
    fn cp_dry_run_refuses_what_the_copy_would_refuse() {
        let refusals: [(&[&CStr], &str); 6] = [
            (
                &[c"/tmp", c"/home/d"],
                "t: -r not specified; omitting directory '/tmp'\n",
            ),
            (
                &[c"/nope", c"/home/z"],
                "t: cannot stat '/nope': No such file or directory\n",
            ),
            (
                &[c"/tmp/x", c"/no/such/y"],
                "t: cannot create regular file '/no/such/y': No such file or directory\n",
            ),
            (
                &[c"/tmp/x", c"/tmp/"],
                "t: '/tmp/x' and '/tmp/x' are the same file\n",
            ),
            (
                &[c"/tmp/x", c"/etc/motd", c"/tmp/x"],
                "t: target '/tmp/x' is not a directory\n",
            ),
            (
                &[c"-r", c"/etc", c"/etc/inside"],
                "t: cannot copy a directory, '/etc', into itself, '/etc/inside'\n",
            ),
        ];
        for (arguments, error) in refusals {
            let (real, fake) = run(cp, files(), arguments);
            assert_eq!((real, fake.errors().as_str()), (1, error));
            let mut previewed = vec![c"--dry-run"];
            previewed.extend_from_slice(arguments);
            let (status, fake) = dry(cp, files(), &previewed);
            assert_eq!((status, fake.errors().as_str()), (1, error));
            assert_eq!(fake.output(), "");
        }
        let (_, kept) = run(cp, files(), &[c"/tmp/x", c"/tmp/"]);
        assert_eq!(contents(&kept, "/tmp/x"), Some(b"a\nb\n".to_vec()));
        let copied = files().with_directory("/home/etc", 0);
        let (status, fake) = dry(cp, copied, &[c"--dry-run", c"-r", c"/etc", c"/home"]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (1, "t: cannot copy '/home/etc': File exists\n")
        );
        let read_only = sized(files(), "/home", 1_000_000, true);
        let (status, fake) = dry(cp, read_only, &[c"--dry-run", c"/tmp/x", c"/home/y"]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (
                1,
                "t: cannot create regular file '/home/y': Read-only file system\n"
            )
        );
        let full = sized(files(), "/home", 3, false);
        let (status, fake) = dry(cp, full, &[c"--dry-run", c"/tmp/x", c"/home/y"]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (1, "t: error writing '/home/y': No space left on device\n")
        );
        let full = sized(files(), "/home", 3, false).with_directory("/home/u", 0);
        let (status, fake) = dry(cp, full, &[c"--dry-run", c"-r", c"/etc", c"/home/u/etc"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(
            fake.output(),
            "would copy the folder '/etc' to '/home/u/etc' (1 file, 0 folders, 3 bytes)\n"
        );
    }

    #[test]
    fn mv_dry_run_says_what_it_would_move_and_refuses_what_the_move_would() {
        let (status, fake) = dry(mv, files(), &[c"--dry-run", c"/tmp/x", c"/tmp/z"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(fake.output(), "would move '/tmp/x' to '/tmp/z' (4 bytes)\n");
        let (_, fake) = dry(mv, files(), &[c"--dry-run", c"/tmp/x", c"/etc/motd"]);
        assert_eq!(
            fake.output(),
            "would move '/tmp/x' to '/etc/motd' (4 bytes), replacing the file there (3 bytes)\n"
        );
        let (status, fake) = dry(mv, files(), &[c"--dry-run", c"/tmp/x", c"/tmp/"]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "would leave '/tmp/x' as it is: '/tmp/x' is the same file\n"
        );
        let empty = files().with_directory("/home/etc", 0);
        let (status, fake) = dry(mv, empty, &[c"--dry-run", c"/etc", c"/home"]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "would move the folder '/etc' to '/home/etc', replacing the empty folder there\n"
        );
        let held = || {
            files()
                .with_directory("/home/etc", 0)
                .with_file("/home/etc/a", 0o644, b"a", 0)
                .with_directory("/home/x", 0)
        };
        let refusals: [(&[&CStr], &str); 4] = [
            (
                &[c"/etc", c"/home"],
                "t: cannot move '/etc' to '/home/etc': Directory not empty\n",
            ),
            (
                &[c"/tmp/x", c"/home"],
                "t: cannot move '/tmp/x' to '/home/x': Is a directory\n",
            ),
            (
                &[c"/etc", c"/etc/inside"],
                "t: cannot move '/etc' to '/etc/inside': Invalid argument\n",
            ),
            (
                &[c"/nope", c"/home/a"],
                "t: cannot move '/nope' to '/home/a': No such file or directory\n",
            ),
        ];
        for (arguments, error) in refusals {
            let mut previewed = vec![c"--dry-run"];
            previewed.extend_from_slice(arguments);
            let (status, fake) = dry(mv, held(), &previewed);
            assert_eq!((status, fake.errors().as_str()), (1, error));
            assert_eq!(fake.output(), "");
        }
        let (real, fake) = run(mv, held(), &[c"/nope", c"/home/a"]);
        assert_eq!(
            (real, fake.errors().as_str()),
            (
                1,
                "t: cannot move '/nope' to '/home/a': No such file or directory\n"
            )
        );
        let read_only = sized(files(), "/tmp", 1_000_000, true);
        let (status, fake) = dry(mv, read_only, &[c"--dry-run", c"/tmp/x", c"/home/x"]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (
                1,
                "t: cannot move '/tmp/x' to '/home/x': Read-only file system\n"
            )
        );
    }

    #[test]
    fn mv_dry_run_between_file_systems_checks_the_copy_it_becomes() {
        let mut across = files()
            .with_directory("/tmp/d", 0)
            .with_file("/tmp/d/f", 0o600, b"f", 0);
        across.cross_device = vec![b"/home".to_vec()];
        if let Some((_, status)) = across.nodes.get_mut(&b"/home"[..]) {
            status.device = 7;
        }
        let (status, fake) = dry(mv, across, &[c"--dry-run", c"/tmp/x", c"/tmp/d", c"/home"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(
            fake.output(),
            "would move '/tmp/x' to '/home/x' (4 bytes)\n\
             would move the folder '/tmp/d' to '/home/d' (1 file, 0 folders, 1 byte)\n"
        );
        let (status, fake) = run(mv, fake, &[c"/tmp/x", c"/tmp/d", c"/home"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(contents(&fake, "/home/d/f"), Some(b"f".to_vec()));
    }

    #[test]
    fn rm_dry_run_says_what_it_would_delete_and_deletes_nothing() {
        let tree = || {
            files()
                .with_directory("/home/u", 0)
                .with_directory("/home/u/d", 0)
                .with_file("/home/u/d/f", 0o644, b"ff", 0)
                .with_file("/home/u/g", 0o644, b"", 0)
                .with_link("/home/u/l", "g")
        };
        let (status, fake) = dry(rm, tree(), &[c"--dry-run", c"/tmp/x", c"/home/u"]);
        assert_eq!(status, 1);
        assert_eq!(fake.output(), "would delete '/tmp/x' (4 bytes) for good\n");
        assert_eq!(
            fake.errors(),
            "t: cannot remove '/home/u': Is a directory\n"
        );
        let (status, fake) = dry(rm, tree(), &[c"--dry-run", c"-r", c"/home/u"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(
            fake.output(),
            "would delete the folder '/home/u' and everything in it for good \
             (3 files, 1 folder, 2 bytes)\n"
        );
        let (status, fake) = dry(rm, tree(), &[c"--dry-run", c"/home/u/l"]);
        assert_eq!(status, 0);
        assert_eq!(fake.output(), "would delete '/home/u/l' for good\n");
        let (status, fake) = dry(rm, files(), &[c"--dry-run", c"/missing"]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (
                1,
                "t: cannot remove '/missing': No such file or directory\n"
            )
        );
        let (status, fake) = dry(rm, files(), &[c"--dry-run", c"-f", c"/missing"]);
        assert_eq!((status, fake.output().as_str()), (0, ""));
        let (status, fake) = dry(rm, files(), &[c"--dry-run", c"-r", c"/tmp/.."]);
        assert_eq!(status, 1);
        assert!(
            fake.errors()
                .starts_with("t: refusing to remove '.' or '..'")
        );
        let read_only = sized(files(), "/tmp", 1_000_000, true);
        let (status, fake) = dry(rm, read_only, &[c"--dry-run", c"/tmp/x"]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (1, "t: cannot remove '/tmp/x': Read-only file system\n")
        );
    }
}
