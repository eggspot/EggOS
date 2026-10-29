use crate::disk::{ABSOLUTE_CAPACITY, absolute_path};
use crate::options::parse_defined;
use crate::output::{FAILURE, Output, error_for};
use crate::system::{MAX_LINK_HOPS, Status, SymbolicLinks, System};
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

const PATH_CAPACITY: usize = 4096;
const NAME_CAPACITY: usize = 256;
const RECORD_BYTES: usize = 2 * PATH_CAPACITY + 256;

pub struct PathText {
    bytes: [u8; PATH_CAPACITY],
    length: usize,
}

impl PathText {
    fn new() -> Self {
        Self {
            bytes: [0; PATH_CAPACITY],
            length: 0,
        }
    }

    pub fn of(bytes: &[u8]) -> Result<Self, Errno> {
        let mut text = Self::new();
        text.push(bytes)?;
        Ok(text)
    }

    pub fn push(&mut self, part: &[u8]) -> Result<(), Errno> {
        let end = self.length + part.len();
        if end >= PATH_CAPACITY || part.contains(&0) {
            return Err(Errno::ENAMETOOLONG);
        }
        self.bytes[self.length..end].copy_from_slice(part);
        self.length = end;
        self.bytes[end] = 0;
        Ok(())
    }

    fn truncate(&mut self, length: usize) {
        self.length = length.min(self.length);
        self.bytes[self.length] = 0;
    }

    fn pop_component(&mut self) {
        let parent = self.as_bytes().iter().rposition(|&byte| byte == b'/');
        self.truncate(parent.unwrap_or(0));
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    pub fn as_c_str(&self) -> &CStr {
        CStr::from_bytes_with_nul(&self.bytes[..=self.length]).unwrap_or(c"")
    }
}

struct Pending {
    bytes: [u8; PATH_CAPACITY],
    start: usize,
    end: usize,
}

impl Pending {
    fn of(path: &[u8]) -> Result<Self, Errno> {
        let mut pending = Self {
            bytes: [0; PATH_CAPACITY],
            start: 0,
            end: path.len(),
        };
        pending
            .bytes
            .get_mut(..path.len())
            .ok_or(Errno::ENAMETOOLONG)?
            .copy_from_slice(path);
        Ok(pending)
    }

    fn skip_slashes(&mut self) {
        while self.start < self.end && self.bytes[self.start] == b'/' {
            self.start += 1;
        }
    }

    fn next(&mut self, name: &mut [u8; NAME_CAPACITY]) -> Result<Option<usize>, Errno> {
        self.skip_slashes();
        if self.start == self.end {
            return Ok(None);
        }
        let length = self.bytes[self.start..self.end]
            .iter()
            .position(|&byte| byte == b'/')
            .unwrap_or(self.end - self.start);
        name.get_mut(..length)
            .ok_or(Errno::ENAMETOOLONG)?
            .copy_from_slice(&self.bytes[self.start..self.start + length]);
        self.start += length;
        self.skip_slashes();
        Ok(Some(length))
    }

    fn is_done(&self) -> bool {
        self.start == self.end
    }

    fn prepend(&mut self, text: &[u8]) -> Result<(), Errno> {
        let rest = self.end - self.start;
        let needed = text.len() + 1 + rest;
        if needed > PATH_CAPACITY {
            return Err(Errno::ENAMETOOLONG);
        }
        let rest_at = text.len() + 1;
        self.bytes.copy_within(self.start..self.end, rest_at);
        self.bytes[..text.len()].copy_from_slice(text);
        self.bytes[text.len()] = b'/';
        self.start = 0;
        self.end = needed;
        Ok(())
    }
}

fn read_link_whole<S: System>(
    system: &mut S,
    path: &CStr,
    target: &mut [u8; PATH_CAPACITY],
) -> Result<usize, Errno> {
    match system.read_link(path, target)? {
        PATH_CAPACITY => Err(Errno::ENAMETOOLONG),
        length => Ok(length),
    }
}

fn canonical<S: System>(system: &mut S, path: &[u8]) -> Result<PathText, Errno> {
    let mut absolute = [0u8; ABSOLUTE_CAPACITY];
    let length = absolute_path(system, path, &mut absolute).ok_or(Errno::ENAMETOOLONG)?;
    let mut pending = Pending::of(&absolute[..length])?;
    let mut resolved = PathText::new();
    let mut name = [0u8; NAME_CAPACITY];
    let mut target = [0u8; PATH_CAPACITY];
    let mut hops = 0;
    while let Some(length) = pending.next(&mut name)? {
        let name = &name[..length];
        match name {
            b"." => continue,
            b".." => {
                resolved.pop_component();
                continue;
            }
            _ => {}
        }
        let last = pending.is_done();
        let before = resolved.length;
        resolved.push(b"/")?;
        resolved.push(name)?;
        match system.status(None, resolved.as_c_str(), SymbolicLinks::DoNotFollow) {
            Ok(status) if status.is_link() => {
                hops += 1;
                if hops > MAX_LINK_HOPS {
                    return Err(Errno::ELOOP);
                }
                let length = read_link_whole(system, resolved.as_c_str(), &mut target)?;
                let target = &target[..length];
                resolved.truncate(match target.first() {
                    Some(b'/') => 0,
                    _ => before,
                });
                pending.prepend(target)?;
            }
            Ok(status) if !last && !status.is_directory() => return Err(Errno::ENOTDIR),
            Ok(_) => {}
            Err(Errno::ENOENT) if last => {}
            Err(errno) => return Err(errno),
        }
    }
    if resolved.length == 0 {
        resolved.push(b"/")?;
    }
    Ok(resolved)
}

fn write_record<S: System>(output: &mut Output<S>, path: &[u8], target: &[u8]) {
    let mut buffer = [0u8; RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("path");
    out.string_lossy(path);
    out.key("target");
    out.string_lossy(target);
    out.end_object();
    out.end_line();
    match out.overflowed() {
        false => output.write(out.as_bytes()),
        true => output.error(&[path, b": the record is too long to write"]),
    }
}

pub fn readlink<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let (mut canonicalize, mut no_newline, mut verbose) = (false, false, false);
    let Ok(operands) = parse_defined(output, arguments, &commands::READLINK, |_, given| {
        match given.option.short {
            Some(b'f') => canonicalize = true,
            Some(b'n') => no_newline = true,
            Some(b'v') => verbose = true,
            _ => {}
        }
        Ok(())
    }) else {
        return FAILURE;
    };
    let paths = operands.all();
    if paths.is_empty() {
        output.coded(&errors::USAGE, &[b"missing operand"]);
        return FAILURE;
    }
    if no_newline && paths.len() > 1 {
        output.error(&[b"ignoring --no-newline with multiple arguments"]);
        no_newline = false;
    }
    let reports = verbose || output.is_json();
    let mut status = 0;
    let mut target = [0u8; PATH_CAPACITY];
    for &path in paths {
        let found = match canonicalize {
            true => canonical(&mut output.system, path.to_bytes()),
            false => read_link_whole(&mut output.system, path, &mut target)
                .and_then(|length| PathText::of(&target[..length])),
        };
        match found {
            Ok(text) if output.is_json() => write_record(output, path.to_bytes(), text.as_bytes()),
            Ok(text) => {
                output.write(text.as_bytes());
                if !no_newline {
                    output.write(b"\n");
                }
            }
            Err(errno) => {
                status = FAILURE;
                if !reports {
                    continue;
                }
                match errno {
                    Errno::EINVAL => output.coded(
                        &errors::NOT_A_LINK,
                        &[path.to_bytes(), b": not a symbolic link"],
                    ),
                    errno => output.file_error(path, errno),
                }
            }
        }
    }
    status
}

#[derive(Debug, Clone, Copy, Default)]
struct LinkOptions {
    symbolic: bool,
    force: bool,
    no_dereference: bool,
    no_target_directory: bool,
    verbose: bool,
}

fn base_name(path: &[u8]) -> &[u8] {
    let end = path
        .iter()
        .rposition(|&byte| byte != b'/')
        .map_or(0, |last| last + 1);
    let start = path[..end]
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |slash| slash + 1);
    &path[start..end]
}

fn folder_part(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) => b"/",
        Some(slash) => &path[..slash],
        None => b".",
    }
}

fn quoted<S: System>(output: &mut Output<S>, text: &[u8]) {
    output.write(b"'");
    output.write(text);
    output.write(b"'");
}

fn same_file<S: System>(
    output: &mut Output<S>,
    options: LinkOptions,
    target: &CStr,
    link: &CStr,
    existing: &Status,
) -> bool {
    let named = match (options.symbolic, target.to_bytes().first()) {
        (false, _) | (true, Some(b'/')) => PathText::of(target.to_bytes()),
        (true, _) => PathText::of(folder_part(link.to_bytes())).and_then(|mut path| {
            path.push(b"/")?;
            path.push(target.to_bytes())?;
            Ok(path)
        }),
    };
    let links = match options.symbolic {
        true => SymbolicLinks::Follow,
        false => SymbolicLinks::DoNotFollow,
    };
    named
        .and_then(|named| output.system.status(None, named.as_c_str(), links))
        .is_ok_and(|status| status.is_same_file(existing))
}

fn make_one<S: System>(
    output: &mut Output<S>,
    options: LinkOptions,
    target: &CStr,
    link: &CStr,
) -> bool {
    let (target_bytes, link_bytes) = (target.to_bytes(), link.to_bytes());
    if !options.symbolic {
        match output
            .system
            .status(None, target, SymbolicLinks::DoNotFollow)
        {
            Ok(status) if status.is_directory() => {
                output.coded(
                    &errors::IS_A_FOLDER,
                    &[
                        b"'",
                        target_bytes,
                        b"': hard link not allowed for directory",
                    ],
                );
                return false;
            }
            Ok(_) => {}
            Err(errno) => {
                output.coded(
                    error_for(errno),
                    &[b"failed to access '", target_bytes, b"': ", errno.message()],
                );
                return false;
            }
        }
    }
    if options.force
        && let Ok(existing) = output.system.status(None, link, SymbolicLinks::DoNotFollow)
    {
        if existing.is_directory() {
            output.coded(
                &errors::IS_A_FOLDER,
                &[b"'", link_bytes, b"': cannot overwrite directory"],
            );
            return false;
        }
        if same_file(output, options, target, link, &existing) {
            output.error(&[
                b"'",
                target_bytes,
                b"' and '",
                link_bytes,
                b"' are the same file",
            ]);
            return false;
        }
        if let Err(errno) = output.system.remove(link, false) {
            output.coded(
                error_for(errno),
                &[b"cannot remove '", link_bytes, b"': ", errno.message()],
            );
            return false;
        }
    }
    let made = match options.symbolic {
        true => output.system.make_link(target, link),
        false => output.system.make_hard_link(target, link),
    };
    let arrow: &[u8] = if options.symbolic { b" -> " } else { b" => " };
    if let Err(errno) = made {
        let error = error_for(errno);
        match options.symbolic {
            true => output.coded(
                error,
                &[
                    b"failed to create symbolic link '",
                    link_bytes,
                    b"': ",
                    errno.message(),
                ],
            ),
            false => output.coded(
                error,
                &[
                    b"failed to create hard link '",
                    link_bytes,
                    b"' => '",
                    target_bytes,
                    b"': ",
                    errno.message(),
                ],
            ),
        }
        return false;
    }
    if options.verbose {
        quoted(output, link_bytes);
        output.write(arrow);
        quoted(output, target_bytes);
        output.write(b"\n");
    }
    true
}

fn is_folder<S: System>(output: &mut Output<S>, path: &CStr, follow: bool) -> bool {
    let links = match follow {
        true => SymbolicLinks::Follow,
        false => SymbolicLinks::DoNotFollow,
    };
    output
        .system
        .status(None, path, links)
        .is_ok_and(|status| status.is_directory())
}

pub fn ln<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut options = LinkOptions::default();
    let Ok(operands) = parse_defined(output, arguments, &commands::LN, |_, given| {
        match given.option.short {
            Some(b's') => options.symbolic = true,
            Some(b'f') => options.force = true,
            Some(b'n') => options.no_dereference = true,
            Some(b'T') => options.no_target_directory = true,
            _ => options.verbose = true,
        }
        Ok(())
    }) else {
        return FAILURE;
    };
    let operands = operands.all();
    let (targets, folder): (&[&CStr], Option<&CStr>) = match operands {
        [] => {
            output.coded(&errors::USAGE, &[b"missing file operand"]);
            return FAILURE;
        }
        [target] if options.no_target_directory => {
            output.coded(
                &errors::USAGE,
                &[
                    b"missing destination file operand after '",
                    target.to_bytes(),
                    b"'",
                ],
            );
            return FAILURE;
        }
        [_] => (operands, Some(c".")),
        [_, _] if options.no_target_directory => (&operands[..1], None),
        [_, _, extra, ..] if options.no_target_directory => {
            output.coded(
                &errors::USAGE,
                &[b"extra operand '", extra.to_bytes(), b"'"],
            );
            return FAILURE;
        }
        [targets @ .., last] if is_folder(output, last, !options.no_dereference) => {
            (targets, Some(*last))
        }
        [_, _] => (&operands[..1], None),
        [.., last] => {
            output.coded(
                &errors::NOT_A_FOLDER,
                &[b"target '", last.to_bytes(), b"' is not a directory"],
            );
            return FAILURE;
        }
    };
    let mut status = 0;
    for &target in targets {
        let link = match folder {
            None => PathText::of(operands[operands.len() - 1].to_bytes()),
            Some(folder) => PathText::of(folder.to_bytes()).and_then(|mut link| {
                if !link.as_bytes().ends_with(b"/") {
                    link.push(b"/")?;
                }
                link.push(base_name(target.to_bytes()))?;
                Ok(link)
            }),
        };
        let made = match link {
            Ok(link) => make_one(output, options, target, link.as_c_str()),
            Err(errno) => {
                output.coded(
                    error_for(errno),
                    &[target.to_bytes(), b": ", errno.message()],
                );
                false
            }
        };
        if !made {
            status = FAILURE;
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    type Utility = fn(&mut Output<Fake>, &[&CStr]) -> i32;

    fn home() -> Fake {
        Fake::default()
            .with_directory("/", 0)
            .with_directory("/home", 0)
            .with_directory("/home/docs", 0)
            .with_file("/home/note.txt", 0o644, b"text\n", 0)
    }

    fn run(fake: Fake, utility: Utility, arguments: &[&CStr]) -> (i32, Fake) {
        let mut output = Output::new(fake, b"t");
        let status = utility(&mut output, arguments);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0, "a descriptor was left open");
        (status, output.system)
    }

    fn ln_in(fake: Fake, arguments: &[&CStr]) -> (i32, Fake) {
        run(fake, ln, arguments)
    }

    fn readlink_in(fake: Fake, arguments: &[&CStr]) -> (i32, Fake) {
        run(fake, readlink, arguments)
    }

    fn target_of(fake: &Fake, path: &str) -> String {
        String::from_utf8(fake.link_target(path).expect(path)).unwrap()
    }

    fn working_in(mut fake: Fake, folder: &str) -> Fake {
        fake.working_directory = folder.to_owned();
        fake
    }

    fn status_of(fake: &mut Fake, path: &CStr) -> Status {
        let found = fake.status(None, path, SymbolicLinks::DoNotFollow);
        found.unwrap()
    }

    #[test]
    fn a_symbolic_link_keeps_its_target_s_text_as_given() {
        let (status, fake) = ln_in(home(), &[c"-s", c"note.txt", c"/home/latest"]);
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(target_of(&fake, "/home/latest"), "note.txt");
        let arguments = [c"-sv", c"../nowhere", c"/home/dangling"];
        let (status, fake) = ln_in(home(), &arguments);
        assert_eq!(status, 0);
        assert_eq!(fake.output(), "'/home/dangling' -> '../nowhere'\n");
        assert_eq!(target_of(&fake, "/home/dangling"), "../nowhere");
    }

    #[test]
    fn a_hard_link_is_another_name_for_the_same_file() {
        let arguments = [c"-v", c"/home/note.txt", c"/home/same.txt"];
        let (status, mut fake) = ln_in(home(), &arguments);
        assert_eq!(status, 0);
        assert_eq!(fake.output(), "'/home/same.txt' => '/home/note.txt'\n");
        assert_eq!(fake.contents("/home/same.txt"), b"text\n");
        let original = status_of(&mut fake, c"/home/note.txt");
        let other = status_of(&mut fake, c"/home/same.txt");
        assert!(original.is_same_file(&other));
        assert_eq!((original.links, other.links), (2, 2));
        let (status, fake) = ln_in(home(), &[c"/home/docs", c"/home/again"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: '/home/docs': hard link not allowed for directory\n"
        );
        let (status, fake) = ln_in(home(), &[c"/home/none", c"/home/x"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: failed to access '/home/none': No such file or directory\n"
        );
    }

    #[test]
    fn a_name_already_there_is_kept_unless_forced_and_never_a_folder_or_the_same_file() {
        let fake = home().with_file("/home/old.txt", 0o644, b"old\n", 0);
        let (status, fake) = ln_in(fake, &[c"-s", c"note.txt", c"/home/old.txt"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: failed to create symbolic link '/home/old.txt': File exists\n"
        );
        assert_eq!(fake.contents("/home/old.txt"), b"old\n");
        let (status, fake) = ln_in(fake, &[c"-sf", c"note.txt", c"/home/old.txt"]);
        assert_eq!(status, 0);
        assert_eq!(target_of(&fake, "/home/old.txt"), "note.txt");
        let (status, fake) = ln_in(home(), &[c"-fT", c"/home/note.txt", c"/home/docs"]);
        assert_eq!(status, 1);
        assert!(fake.errors().contains("cannot overwrite directory"));
        let same = [c"-f", c"/home/note.txt", c"/home/note.txt"];
        let (status, fake) = ln_in(home(), &same);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: '/home/note.txt' and '/home/note.txt' are the same file\n"
        );
        assert_eq!(fake.contents("/home/note.txt"), b"text\n");
        let (status, fake) = ln_in(home(), &[c"-sf", c"note.txt", c"/home/note.txt"]);
        assert_eq!(status, 1);
        assert!(fake.errors().contains("are the same file"));
        assert_eq!(fake.contents("/home/note.txt"), b"text\n");
    }

    #[test]
    fn links_go_into_a_folder_under_their_targets_names() {
        let fake = home().with_file("/home/b.txt", 0o644, b"b", 0);
        let arguments = [c"-s", c"/home/note.txt", c"/home/b.txt", c"/home/docs"];
        let (status, fake) = ln_in(fake, &arguments);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(target_of(&fake, "/home/docs/note.txt"), "/home/note.txt");
        assert_eq!(target_of(&fake, "/home/docs/b.txt"), "/home/b.txt");
        let (status, fake) = ln_in(home(), &[c"-s", c"a", c"b", c"/home/note.txt"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: target '/home/note.txt' is not a directory\n"
        );
        let (status, fake) = ln_in(home(), &[c"-sT", c"x", c"/home/docs"]);
        assert_eq!(status, 1);
        assert!(fake.errors().contains("File exists"));
        let to_docs = || working_in(home().with_link("/home/to-docs", "docs"), "/home");
        let (status, fake) = ln_in(to_docs(), &[c"-s", c"note.txt", c"to-docs"]);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(target_of(&fake, "/home/docs/note.txt"), "note.txt");
        let (status, fake) = ln_in(to_docs(), &[c"-sfn", c"note.txt", c"to-docs"]);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(target_of(&fake, "/home/to-docs"), "note.txt");
        let in_docs = working_in(home(), "/home/docs");
        let (status, fake) = ln_in(in_docs, &[c"-s", c"/home/note.txt"]);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(target_of(&fake, "/home/docs/note.txt"), "/home/note.txt");
    }

    #[test]
    fn missing_and_extra_operands_are_usage_errors() {
        for (arguments, message) in [
            (&[][..], "t: missing file operand\n"),
            (
                &[c"-T", c"a"][..],
                "t: missing destination file operand after 'a'\n",
            ),
            (&[c"-T", c"a", c"b", c"c"][..], "t: extra operand 'c'\n"),
            (&[c"-x", c"a"][..], "t: invalid option -- 'x'\n"),
        ] {
            let (status, fake) = ln_in(home(), arguments);
            assert_eq!((status, fake.errors().as_str()), (1, message));
        }
    }

    fn linked() -> Fake {
        home()
            .with_link("/home/latest", "note.txt")
            .with_link("/home/up", "../home/latest")
            .with_link("/home/absolute", "/home/docs")
            .with_link("/home/loop", "loop")
            .with_link("/home/dangling", "gone/x")
    }

    #[test]
    fn readlink_prints_a_link_s_target_and_fails_quietly_on_anything_else() {
        let (status, fake) = readlink_in(linked(), &[c"/home/latest", c"/home/up"]);
        assert_eq!(status, 0);
        assert_eq!(fake.output(), "note.txt\n../home/latest\n");
        let (status, fake) = readlink_in(linked(), &[c"-n", c"/home/latest"]);
        assert_eq!((status, fake.output().as_str()), (0, "note.txt"));
        let (status, fake) = readlink_in(linked(), &[c"/home/note.txt", c"/home/none"]);
        assert_eq!((status, fake.output().as_str()), (1, ""));
        assert_eq!(fake.errors(), "");
        let arguments = [c"-v", c"/home/note.txt", c"/home/none"];
        let (status, fake) = readlink_in(linked(), &arguments);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "t: /home/note.txt: not a symbolic link\n\
             t: /home/none: No such file or directory\n"
        );
        let arguments = [c"-n", c"/home/latest", c"/home/up"];
        let (status, fake) = readlink_in(linked(), &arguments);
        assert_eq!(status, 0);
        assert_eq!(fake.output(), "note.txt\n../home/latest\n");
        assert_eq!(
            fake.errors(),
            "t: ignoring --no-newline with multiple arguments\n"
        );
        let (status, fake) = readlink_in(linked(), &[]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (1, "t: missing operand\n")
        );
    }

    #[test]
    fn readlink_json_writes_a_record_per_link_and_coded_errors() {
        let arguments = [c"--json", c"/home/latest", c"/home/note.txt"];
        let (status, fake) = readlink_in(linked(), &arguments);
        assert_eq!(status, 1);
        assert_eq!(
            fake.output(),
            "{\"path\":\"/home/latest\",\"target\":\"note.txt\"}\n"
        );
        assert!(
            fake.errors()
                .starts_with("{\"error\":{\"code\":\"not_a_link\"")
        );
    }

    #[test]
    fn canonicalizing_follows_every_link_and_needs_all_but_the_last_part() {
        let arguments = [
            c"-f",
            c"up",
            c"/home/absolute/../note.txt",
            c"/home/docs/new",
            c"/home/./docs/../docs",
            c"/",
        ];
        let (status, fake) = readlink_in(working_in(linked(), "/home"), &arguments);
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(
            fake.output(),
            "/home/note.txt\n/home/note.txt\n/home/docs/new\n/home/docs\n/\n"
        );
        for (path, why) in [
            (c"/home/loop", "Too many levels of symbolic links"),
            (c"/home/missing/x", "No such file or directory"),
            (c"/home/dangling", "No such file or directory"),
            (c"/home/note.txt/x", "Not a directory"),
        ] {
            let (status, fake) = readlink_in(linked(), &[c"-fv", path]);
            assert_eq!((status, fake.output().as_str()), (1, ""), "{path:?}");
            let shown = path.to_str().unwrap();
            assert_eq!(fake.errors(), format!("t: {shown}: {why}\n"));
        }
    }

    #[test]
    fn a_pending_path_takes_a_link_s_target_in_front_of_what_is_left() {
        let mut pending = Pending::of(b"/a//b/c").unwrap();
        let mut name = [0u8; NAME_CAPACITY];
        assert_eq!(pending.next(&mut name), Ok(Some(1)));
        assert_eq!(&name[..1], b"a");
        assert_eq!(pending.next(&mut name), Ok(Some(1)));
        assert!(!pending.is_done());
        pending.prepend(b"../x/yz").unwrap();
        let mut seen = Vec::new();
        while let Some(length) = pending.next(&mut name).unwrap() {
            seen.push(String::from_utf8(name[..length].to_vec()).unwrap());
        }
        assert_eq!(seen, ["..", "x", "yz", "c"]);
        assert!(pending.is_done());
        assert_eq!(base_name(b"/a/b/"), b"b");
        assert_eq!(base_name(b"c"), b"c");
        assert_eq!(folder_part(b"/a/b"), b"/a");
        assert_eq!(folder_part(b"/b"), b"/");
        assert_eq!(folder_part(b"b"), b".");
    }
}
