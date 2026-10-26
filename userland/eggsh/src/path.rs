//! Finding the program a command names: a name with a `/` is a path; any other is looked for
//! in each directory of `PATH`, in order (an empty entry is the working directory).

/// The `PATH` the shell searches when the variable is not set.
pub const DEFAULT_PATH: &[u8] = b"/bin:/usr/bin:/sbin";

/// The most bytes a program's path takes, its NUL included.
pub const PATH_CAPACITY: usize = 4096;

/// What a path names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Nothing.
    Missing,
    /// A directory.
    Directory,
    /// A file someone may execute.
    Executable,
    /// A file nobody may execute.
    NotExecutable,
}

/// What paths name: the file system, or a test's.
pub trait Files {
    /// What `path` (with its NUL) names.
    fn kind(&mut self, path: &[u8]) -> Kind;
}

/// Where a command's program is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    /// This many bytes of the buffer, its NUL included.
    Program(usize),
    /// A name without `/` that no directory of `PATH` has.
    NotFound,
    /// A path with `/` that names nothing.
    Missing,
    /// A path that names a directory.
    Directory,
    /// A file that cannot be executed (the first such, when nothing executable was found).
    NotExecutable,
    /// Too long for [`PATH_CAPACITY`].
    TooLong,
}

/// Finds the program `name` runs, searching `path` if it has no `/`, writing its path in
/// `buffer`.
pub fn find<F: Files>(
    name: &[u8],
    path: &[u8],
    files: &mut F,
    buffer: &mut [u8; PATH_CAPACITY],
) -> Lookup {
    if name.contains(&b'/') {
        if name.len() + 1 > PATH_CAPACITY {
            return Lookup::TooLong;
        }
        buffer[..name.len()].copy_from_slice(name);
        buffer[name.len()] = 0;
        return match files.kind(&buffer[..name.len() + 1]) {
            Kind::Executable => Lookup::Program(name.len() + 1),
            Kind::Missing => Lookup::Missing,
            Kind::Directory => Lookup::Directory,
            Kind::NotExecutable => Lookup::NotExecutable,
        };
    }
    if name.is_empty() {
        return Lookup::NotFound;
    }
    let mut result = Lookup::NotFound;
    for directory in path.split(|&byte| byte == b':') {
        let directory = if directory.is_empty() {
            &b"."[..]
        } else {
            directory
        };
        let length = directory.len() + 1 + name.len() + 1;
        if length > PATH_CAPACITY {
            continue;
        }
        buffer[..directory.len()].copy_from_slice(directory);
        buffer[directory.len()] = b'/';
        buffer[directory.len() + 1..length - 1].copy_from_slice(name);
        buffer[length - 1] = 0;
        match files.kind(&buffer[..length]) {
            Kind::Executable => return Lookup::Program(length),
            Kind::NotExecutable if result == Lookup::NotFound => result = Lookup::NotExecutable,
            _ => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeFiles {
        files: Vec<(&'static str, Kind)>,
        asked: Vec<String>,
    }

    impl Files for FakeFiles {
        fn kind(&mut self, path: &[u8]) -> Kind {
            let path = std::str::from_utf8(path.strip_suffix(b"\0").unwrap()).unwrap();
            self.asked.push(path.to_owned());
            self.files
                .iter()
                .find(|(name, _)| *name == path)
                .map_or(Kind::Missing, |(_, kind)| *kind)
        }
    }

    fn files() -> FakeFiles {
        FakeFiles {
            files: vec![
                ("/bin/ls", Kind::Executable),
                ("/usr/bin/ls", Kind::Executable),
                ("/bin/notes", Kind::NotExecutable),
                ("/usr/bin/notes", Kind::Executable),
                ("/bin/data", Kind::NotExecutable),
                ("/bin/dir", Kind::Directory),
                ("/sbin/dir", Kind::Executable),
                ("./here", Kind::Executable),
            ],
            asked: Vec::new(),
        }
    }

    fn found(name: &str, path: &str) -> (Lookup, String, Vec<String>) {
        let mut files = files();
        let mut buffer = [0; PATH_CAPACITY];
        let result = find(name.as_bytes(), path.as_bytes(), &mut files, &mut buffer);
        let program = match result {
            Lookup::Program(length) => {
                assert_eq!(buffer[length - 1], 0);
                String::from_utf8(buffer[..length - 1].to_vec()).unwrap()
            }
            _ => String::new(),
        };
        (result, program, files.asked)
    }

    #[test]
    fn the_first_executable_in_path_order_wins() {
        let (result, program, asked) = found("ls", "/bin:/usr/bin");
        assert_eq!(result, Lookup::Program(8));
        assert_eq!(program, "/bin/ls");
        assert_eq!(asked, ["/bin/ls"]);
        let (_, program, _) = found("ls", "/usr/bin:/bin");
        assert_eq!(program, "/usr/bin/ls");
        let (_, program, _) = found("notes", "/bin:/usr/bin");
        assert_eq!(program, "/usr/bin/notes");
        let (_, program, _) = found("dir", std::str::from_utf8(DEFAULT_PATH).unwrap());
        assert_eq!(program, "/sbin/dir");
    }

    #[test]
    fn a_missing_command_or_one_that_cannot_run_is_told_apart() {
        assert_eq!(found("nothing", "/bin:/usr/bin").0, Lookup::NotFound);
        assert_eq!(found("data", "/bin:/usr/bin").0, Lookup::NotExecutable);
        assert_eq!(found("", "/bin").0, Lookup::NotFound);
        assert_eq!(found("ls", "").1, "");
    }

    #[test]
    fn an_empty_path_entry_is_the_working_directory() {
        let (_, program, asked) = found("here", "/bin::/usr/bin");
        assert_eq!(program, "./here");
        assert_eq!(asked, ["/bin/here", "./here"]);
    }

    #[test]
    fn a_name_with_a_slash_is_not_searched() {
        let (result, program, asked) = found("/bin/ls", "/usr/bin");
        assert_eq!((result, program.as_str()), (Lookup::Program(8), "/bin/ls"));
        assert_eq!(asked, ["/bin/ls"]);
        assert_eq!(found("./here", "/bin").1, "./here");
        assert_eq!(found("/bin/none", "/bin").0, Lookup::Missing);
        assert_eq!(found("/bin/dir", "/bin").0, Lookup::Directory);
        assert_eq!(found("/bin/data", "/bin").0, Lookup::NotExecutable);
        let long = "/".repeat(PATH_CAPACITY);
        assert_eq!(found(&long, "/bin").0, Lookup::TooLong);
    }
}
