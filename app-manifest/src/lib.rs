//! The app catalogue's entries (D133): an installed app is a folder `/Applications/<name>/`
//! holding its program and the file `app.manifest`, a small text file: the line `eggos-app 1`,
//! then `key=value` lines. `name` (the folder's name), `source` (`windows`, `macos` or
//! `linux`) and `run` (the program, absolute or relative to the folder) are required;
//! `architecture`, `synonyms` (comma-separated words the launcher also matches), `icon`
//! (the app's own icon file, relative to the folder, D148: see [`icon`]), and for a
//! dynamically linked Linux program `loader` (its dynamic loader, relative to the folder),
//! `libraries` (the folders its libraries are in, relative to the folder, separated by `:`)
//! and `arguments` (words given to the program when the app opens; D172), and `environment`
//! (`NAME=value` words set in the program's environment when the app opens; D176) are
//! optional. Reading
//! is tolerant: a file of another format, longer than [`LARGEST`], missing a required key, or
//! naming another folder is no app; lines that are no entry are skipped; an invalid optional
//! value is ignored; the last line of a key wins. Nothing here allocates or panics: the
//! kernel's desktop reads manifests with it and the installer writes them.

#![cfg_attr(not(test), no_std)]

pub mod icon;

/// The folder installed apps live in, one folder each.
pub const DIRECTORY: &str = "/Applications";

/// The manifest's file name inside an app's folder.
pub const FILE_NAME: &str = "app.manifest";

/// The file's first line: its format and version.
pub const HEADER: &[u8] = b"eggos-app 1";

/// The longest manifest that is read; a longer one is no app.
pub const LARGEST: usize = 1024;

/// The longest app name, in bytes.
pub const NAME_CAPACITY: usize = 32;

/// The longest `run` value, in bytes.
pub const RUN_CAPACITY: usize = 192;

/// The longest `synonyms` value, in bytes; a longer one gives the app no synonyms.
pub const SYNONYMS_CAPACITY: usize = 128;

/// The most synonyms an app has; later ones are ignored.
pub const MAX_SYNONYMS: usize = 8;

/// The longest `architecture` value, in bytes; a longer one is ignored.
pub const ARCHITECTURE_CAPACITY: usize = 16;

/// The longest `libraries` value, in bytes; a longer one is ignored.
pub const LIBRARIES_CAPACITY: usize = 128;

/// The longest `arguments` value, in bytes; a longer one is ignored.
pub const ARGUMENTS_CAPACITY: usize = 128;

/// The bytes besides letters and digits an `arguments` value may hold: none that a shell
/// reads as anything but part of a word or the space between words.
pub const ARGUMENT_PUNCTUATION: &[u8] = b" -_./:=+,@%";

/// The longest `environment` value, in bytes; a longer one is ignored.
pub const ENVIRONMENT_CAPACITY: usize = 128;

/// The system an app was built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A Windows program (`.exe`).
    Windows,
    /// A macOS program (Mach-O or an `.app` bundle).
    Macos,
    /// A Linux program (ELF).
    Linux,
}

impl Source {
    /// Every source.
    pub const ALL: [Self; 3] = [Self::Windows, Self::Macos, Self::Linux];

    /// The value of `source=` that names it.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
        }
    }

    /// How the launcher shows it: `Windows`, `macOS` or `Linux`.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Macos => "macOS",
            Self::Linux => "Linux",
        }
    }

    fn from_key(key: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|source| source.key().as_bytes() == key)
    }
}

/// An app's manifest, borrowing the file it was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manifest<'a> {
    /// Its name, also its folder's: 1 to [`NAME_CAPACITY`] bytes of printable ASCII, no `/`
    /// or `'`, not `.` or `..`.
    pub name: &'a str,
    /// The system it was built for.
    pub source: Source,
    /// The program to run: absolute, or relative to the app's folder, with no `..` part.
    pub run: &'a str,
    /// The machine it was built for, if the manifest says.
    pub architecture: Option<&'a str>,
    /// Words the launcher matches besides the name, separated by commas.
    pub synonyms: &'a str,
    /// The app's own icon file ([`icon`]), relative to its folder, if it has one.
    pub icon: Option<&'a str>,
    /// The dynamic loader a Linux program runs through, relative to the app's folder with
    /// `run`'s rules, when the app carries its own (D172).
    pub loader: Option<&'a str>,
    /// The folders the loader finds the program's libraries in, relative to the app's folder,
    /// separated by `:`.
    pub libraries: Option<&'a str>,
    /// Words given to the program after its path when the app opens, separated by spaces.
    pub arguments: Option<&'a str>,
    /// `NAME=value` words, separated by spaces, set in the program's environment when the app
    /// opens (D176): where a toolkit finds the data the app carries, say.
    pub environment: Option<&'a str>,
}

impl<'a> Manifest<'a> {
    /// The synonyms, trimmed, empty ones skipped, at most [`MAX_SYNONYMS`].
    pub fn synonyms(&self) -> impl Iterator<Item = &'a str> + use<'a> {
        self.synonyms
            .split(',')
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .take(MAX_SYNONYMS)
    }

    /// Whether [`Self::run`] is relative to the app's folder.
    pub fn run_is_relative(&self) -> bool {
        !self.run.starts_with('/')
    }
}

/// Why a manifest could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The name is not one [`is_name`] accepts.
    InvalidName,
    /// The program is not one [`is_run`] accepts.
    InvalidRun,
    /// The architecture or the synonyms hold something a manifest cannot.
    InvalidValue,
    /// The file would not fit the buffer or [`LARGEST`].
    TooLarge,
}

fn is_printable(text: &[u8]) -> bool {
    text.iter().all(|byte| matches!(byte, b' '..=b'~'))
}

/// Whether `name` may name an app (and its folder).
pub fn is_name(name: &[u8]) -> bool {
    (1..=NAME_CAPACITY).contains(&name.len())
        && is_printable(name)
        && !name.contains(&b'/')
        && !name.contains(&b'\'')
        && name != b"."
        && name != b".."
        && name.first() != Some(&b' ')
        && name.last() != Some(&b' ')
}

/// Whether `run` may be an app's program.
pub fn is_run(run: &[u8]) -> bool {
    (1..=RUN_CAPACITY).contains(&run.len())
        && is_printable(run)
        && !run.contains(&b'\'')
        && !run.ends_with(b"/")
        && run.split(|&byte| byte == b'/').all(|part| part != b"..")
}

/// Whether `icon` may be an app's icon file: a path [`is_run`] accepts that is relative.
pub fn is_icon(icon: &[u8]) -> bool {
    is_run(icon) && !icon.starts_with(b"/")
}

/// Whether `libraries` may be an app's library folders: at most [`LIBRARIES_CAPACITY`]
/// bytes of relative paths [`is_icon`] accepts, separated by `:`.
pub fn is_libraries(libraries: &[u8]) -> bool {
    libraries.len() <= LIBRARIES_CAPACITY
        && libraries
            .split(|&byte| byte == b':')
            .all(|folder| is_icon(folder) && !folder.contains(&b' '))
}

/// Whether `arguments` may be an app's arguments: 1 to [`ARGUMENTS_CAPACITY`] bytes of
/// letters, digits and [`ARGUMENT_PUNCTUATION`], not starting with a space.
pub fn is_arguments(arguments: &[u8]) -> bool {
    (1..=ARGUMENTS_CAPACITY).contains(&arguments.len())
        && arguments.first() != Some(&b' ')
        && arguments
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || ARGUMENT_PUNCTUATION.contains(byte))
}

fn is_variable(word: &[u8]) -> bool {
    let Some(equals) = word.iter().position(|&byte| byte == b'=') else {
        return false;
    };
    let (name, value) = (&word[..equals], &word[equals + 1..]);
    let starts_well = name
        .first()
        .is_some_and(|&byte| byte.is_ascii_alphabetic() || byte == b'_');
    starts_well
        && name
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && value.iter().all(|&byte| {
            byte.is_ascii_alphanumeric() || (byte != b' ' && ARGUMENT_PUNCTUATION.contains(&byte))
        })
}

/// Whether `environment` may be an app's environment: 1 to [`ENVIRONMENT_CAPACITY`] bytes of
/// `NAME=value` words separated by single spaces, each name a letter or `_` then letters,
/// digits and `_`, each value letters, digits and [`ARGUMENT_PUNCTUATION`] but the space.
pub fn is_environment(environment: &[u8]) -> bool {
    (1..=ENVIRONMENT_CAPACITY).contains(&environment.len())
        && environment.split(|&byte| byte == b' ').all(is_variable)
}

fn is_key(key: &[u8]) -> bool {
    !key.is_empty()
        && key
            .iter()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_'))
}

fn without_carriage_return(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn entries(file: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let body = match file.len() <= LARGEST {
        true => {
            let (first, rest) = match file.iter().position(|&byte| byte == b'\n') {
                Some(end) => (&file[..end], &file[end + 1..]),
                None => (file, &[][..]),
            };
            match without_carriage_return(first) == HEADER {
                true => rest,
                false => &[][..],
            }
        }
        false => &[][..],
    };
    body.split(|&byte| byte == b'\n').filter_map(|line| {
        let line = without_carriage_return(line);
        let equals = line.iter().position(|&byte| byte == b'=')?;
        let (key, value) = (&line[..equals], &line[equals + 1..]);
        (is_key(key) && is_printable(value)).then_some((key, value))
    })
}

fn last<'a>(file: &'a [u8], key: &str) -> Option<&'a [u8]> {
    entries(file)
        .filter(|(name, _)| *name == key.as_bytes())
        .last()
        .map(|(_, value)| value)
}

fn text(bytes: &[u8]) -> Option<&str> {
    core::str::from_utf8(bytes).ok()
}

/// The manifest `file` holds for the app in folder `folder` (its name, not its path); `None`
/// when it is no app's: not this format, too long, a required key missing or invalid, or a
/// name that is not the folder's.
pub fn parse<'a>(file: &'a [u8], folder: &[u8]) -> Option<Manifest<'a>> {
    let name = last(file, "name").filter(|name| is_name(name) && *name == folder)?;
    let source = last(file, "source").and_then(Source::from_key)?;
    let run = last(file, "run").filter(|run| is_run(run))?;
    let architecture = last(file, "architecture")
        .filter(|value| !value.is_empty() && value.len() <= ARCHITECTURE_CAPACITY)
        .and_then(text);
    let synonyms = last(file, "synonyms")
        .filter(|value| value.len() <= SYNONYMS_CAPACITY)
        .and_then(text)
        .unwrap_or("");
    let icon = last(file, "icon")
        .filter(|value| is_icon(value))
        .and_then(text);
    let loader = last(file, "loader")
        .filter(|value| is_icon(value))
        .and_then(text);
    let libraries = last(file, "libraries")
        .filter(|value| is_libraries(value))
        .and_then(text);
    let arguments = last(file, "arguments")
        .filter(|value| is_arguments(value))
        .and_then(text);
    let environment = last(file, "environment")
        .filter(|value| is_environment(value))
        .and_then(text);
    Some(Manifest {
        name: text(name)?,
        source,
        run: text(run)?,
        architecture,
        synonyms,
        icon,
        loader,
        libraries,
        arguments,
        environment,
    })
}

struct Writer<'a> {
    out: &'a mut [u8],
    length: usize,
}

impl Writer<'_> {
    fn put(&mut self, parts: &[&[u8]]) -> Result<(), Refused> {
        let needed: usize = parts.iter().map(|part| part.len()).sum();
        if self.length + needed > self.out.len().min(LARGEST) {
            return Err(Refused::TooLarge);
        }
        for part in parts {
            self.out[self.length..self.length + part.len()].copy_from_slice(part);
            self.length += part.len();
        }
        Ok(())
    }
}

/// Writes `manifest` into `out` as [`parse`] reads it; returns the file's length.
pub fn write(manifest: &Manifest<'_>, out: &mut [u8]) -> Result<usize, Refused> {
    if !is_name(manifest.name.as_bytes()) {
        return Err(Refused::InvalidName);
    }
    if !is_run(manifest.run.as_bytes()) {
        return Err(Refused::InvalidRun);
    }
    let architecture = manifest.architecture.unwrap_or("");
    let valid_value =
        |value: &str, capacity: usize| value.len() <= capacity && is_printable(value.as_bytes());
    if !valid_value(architecture, ARCHITECTURE_CAPACITY)
        || !valid_value(manifest.synonyms, SYNONYMS_CAPACITY)
    {
        return Err(Refused::InvalidValue);
    }
    let invalid = manifest.icon.is_some_and(|icon| !is_icon(icon.as_bytes()))
        || manifest
            .loader
            .is_some_and(|loader| !is_icon(loader.as_bytes()))
        || manifest
            .libraries
            .is_some_and(|libraries| !is_libraries(libraries.as_bytes()))
        || manifest
            .arguments
            .is_some_and(|arguments| !is_arguments(arguments.as_bytes()))
        || manifest
            .environment
            .is_some_and(|environment| !is_environment(environment.as_bytes()));
    if invalid {
        return Err(Refused::InvalidValue);
    }
    let mut writer = Writer { out, length: 0 };
    writer.put(&[HEADER, b"\n"])?;
    writer.put(&[b"name=", manifest.name.as_bytes(), b"\n"])?;
    writer.put(&[b"source=", manifest.source.key().as_bytes(), b"\n"])?;
    writer.put(&[b"run=", manifest.run.as_bytes(), b"\n"])?;
    if !architecture.is_empty() {
        writer.put(&[b"architecture=", architecture.as_bytes(), b"\n"])?;
    }
    if !manifest.synonyms.is_empty() {
        writer.put(&[b"synonyms=", manifest.synonyms.as_bytes(), b"\n"])?;
    }
    for (key, value) in [
        (&b"icon="[..], manifest.icon),
        (b"loader=", manifest.loader),
        (b"libraries=", manifest.libraries),
        (b"arguments=", manifest.arguments),
        (b"environment=", manifest.environment),
    ] {
        if let Some(value) = value {
            writer.put(&[key, value.as_bytes(), b"\n"])?;
        }
    }
    Ok(writer.length)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &[u8] =
        b"eggos-app 1\nname=Hello\nsource=linux\nrun=bin/hello\nsynonyms=greeter, hi ,,wave\n";

    #[test]
    fn a_manifest_reads_its_keys() {
        let manifest = parse(HELLO, b"Hello").unwrap();
        assert_eq!(manifest.name, "Hello");
        assert_eq!(manifest.source, Source::Linux);
        assert_eq!(manifest.run, "bin/hello");
        assert!(manifest.run_is_relative());
        assert_eq!(manifest.architecture, None);
        let synonyms: Vec<_> = manifest.synonyms().collect();
        assert_eq!(synonyms, ["greeter", "hi", "wave"]);
    }

    #[test]
    fn a_manifest_naming_another_folder_is_no_app() {
        assert_eq!(parse(HELLO, b"hello"), None);
        assert_eq!(parse(HELLO, b"Other"), None);
    }

    #[test]
    fn a_missing_or_invalid_required_key_is_no_app() {
        for file in [
            &b"eggos-app 1\nsource=linux\nrun=/bin/x\n"[..],
            b"eggos-app 1\nname=Hello\nrun=/bin/x\n",
            b"eggos-app 1\nname=Hello\nsource=linux\n",
            b"eggos-app 1\nname=Hello\nsource=amiga\nrun=/bin/x\n",
            b"eggos-app 1\nname=Hello\nsource=linux\nrun=../escape\n",
            b"eggos-app 1\nname=Hello\nsource=linux\nrun=a/../../b\n",
            b"eggos-app 1\nname=Hello\nsource=linux\nrun=dir/\n",
            b"eggos-app 1\nname=Hello\nsource=linux\nrun=it's\n",
            b"eggos-app 2\nname=Hello\nsource=linux\nrun=/bin/x\n",
            b"name=Hello\nsource=linux\nrun=/bin/x\n",
            b"",
            b"\xff\xfe\x00garbage",
        ] {
            assert_eq!(
                parse(file, b"Hello"),
                None,
                "{:?}",
                String::from_utf8_lossy(file)
            );
        }
    }

    #[test]
    fn names_are_checked() {
        assert!(is_name(b"Hello World"));
        assert!(is_name(&[b'a'; NAME_CAPACITY]));
        for name in [
            &b""[..],
            b".",
            b"..",
            b"a/b",
            b"it's",
            b" lead",
            b"trail ",
            b"tab\t",
            &[b'a'; NAME_CAPACITY + 1],
        ] {
            assert!(!is_name(name), "{name:?}");
        }
    }

    #[test]
    fn the_last_line_of_a_key_wins_and_junk_lines_are_skipped() {
        let file = b"eggos-app 1\r\nname=Old\n\njunk\n=x\nName=Bad\nname=Tool\r\nsource=windows\nsource=macos\nrun=/x/tool\narchitecture=aarch64\nicon=ignored\nunknown=kept\n";
        let manifest = parse(file, b"Tool").unwrap();
        assert_eq!(manifest.source, Source::Macos);
        assert_eq!(manifest.run, "/x/tool");
        assert!(!manifest.run_is_relative());
        assert_eq!(manifest.architecture, Some("aarch64"));
        assert_eq!(manifest.synonyms().count(), 0);
        assert_eq!(manifest.icon, Some("ignored"));
    }

    #[test]
    fn an_icon_is_a_relative_path_and_an_invalid_one_is_ignored() {
        let with = |icon: &str| format!("eggos-app 1\nname=A\nsource=linux\nrun=/a\nicon={icon}\n");
        assert_eq!(
            parse(with("app.icon").as_bytes(), b"A").unwrap().icon,
            Some("app.icon")
        );
        assert_eq!(
            parse(with("art/big.icon").as_bytes(), b"A").unwrap().icon,
            Some("art/big.icon")
        );
        for bad in ["/etc/icon", "../other/app.icon", "a/../../b", "", "dir/"] {
            let file = with(bad);
            let manifest = parse(file.as_bytes(), b"A").unwrap();
            assert_eq!(manifest.icon, None, "{bad}");
        }
        assert_eq!(parse(HELLO, b"Hello").unwrap().icon, None);
    }

    #[test]
    fn a_file_too_long_is_no_app_and_long_optional_values_are_dropped() {
        let mut long = HELLO.to_vec();
        assert!(parse(&long, b"Hello").is_some());
        long.resize(LARGEST + 1, b'\n');
        assert_eq!(parse(&long, b"Hello"), None);
        let mut file = b"eggos-app 1\nname=A\nsource=linux\nrun=/a\nsynonyms=".to_vec();
        file.extend([b'x'; SYNONYMS_CAPACITY + 1]);
        file.extend(b"\narchitecture=");
        file.extend([b'y'; ARCHITECTURE_CAPACITY + 1]);
        let manifest = parse(&file, b"A").unwrap();
        assert_eq!(manifest.synonyms().count(), 0);
        assert_eq!(manifest.architecture, None);
        let many = b"eggos-app 1\nname=A\nsource=linux\nrun=/a\nsynonyms=1,2,3,4,5,6,7,8,9,10\n";
        assert_eq!(parse(many, b"A").unwrap().synonyms().count(), MAX_SYNONYMS);
    }

    #[test]
    fn what_is_written_reads_back() {
        let manifest = Manifest {
            name: "Paint",
            source: Source::Windows,
            run: "paint.exe",
            architecture: Some("x86_64"),
            synonyms: "draw,mspaint",
            icon: Some("app.icon"),
            loader: None,
            libraries: None,
            arguments: None,
            environment: None,
        };
        let mut out = [0u8; LARGEST];
        let length = write(&manifest, &mut out).unwrap();
        assert_eq!(
            &out[..length],
            b"eggos-app 1\nname=Paint\nsource=windows\nrun=paint.exe\narchitecture=x86_64\nsynonyms=draw,mspaint\nicon=app.icon\n"
        );
        assert_eq!(parse(&out[..length], b"Paint"), Some(manifest));
    }

    #[test]
    fn writing_refuses_what_reading_would_not_take() {
        let good = Manifest {
            name: "A",
            source: Source::Linux,
            run: "/a",
            architecture: None,
            synonyms: "",
            icon: None,
            loader: None,
            libraries: None,
            arguments: None,
            environment: None,
        };
        let mut out = [0u8; LARGEST];
        let refused = |manifest: Manifest<'_>, out: &mut [u8]| write(&manifest, out);
        assert_eq!(
            refused(
                Manifest {
                    icon: Some("/abs.icon"),
                    ..good
                },
                &mut out
            ),
            Err(Refused::InvalidValue)
        );
        assert_eq!(
            refused(
                Manifest {
                    name: "a/b",
                    ..good
                },
                &mut out
            ),
            Err(Refused::InvalidName)
        );
        assert_eq!(
            refused(
                Manifest {
                    run: "../x",
                    ..good
                },
                &mut out
            ),
            Err(Refused::InvalidRun)
        );
        assert_eq!(
            refused(
                Manifest {
                    synonyms: "a\nb",
                    ..good
                },
                &mut out
            ),
            Err(Refused::InvalidValue)
        );
        assert_eq!(refused(good, &mut [0u8; 8]), Err(Refused::TooLarge));
        for (loader, libraries, arguments) in [
            (Some("/lib/ld.so"), None, None),
            (None, Some("lib:/usr/lib"), None),
            (None, Some("lib:../x"), None),
            (None, None, Some("-n; reboot")),
            (None, None, Some(" -n")),
            (None, None, Some("--name 'x'")),
        ] {
            assert_eq!(
                refused(
                    Manifest {
                        loader,
                        libraries,
                        arguments,
                        ..good
                    },
                    &mut out
                ),
                Err(Refused::InvalidValue),
                "{loader:?} {libraries:?} {arguments:?}"
            );
        }
    }

    #[test]
    fn a_dynamically_linked_program_s_loader_libraries_and_arguments_read_back() {
        let manifest = Manifest {
            name: "brogue",
            source: Source::Linux,
            run: "brogue",
            architecture: Some("x86_64"),
            synonyms: "",
            icon: None,
            loader: Some("lib/ld-musl-x86_64.so.1"),
            libraries: Some("lib:usr/lib"),
            arguments: Some("--data-dir /Applications/brogue/share/brogue"),
            environment: None,
        };
        let mut out = [0u8; LARGEST];
        let length = write(&manifest, &mut out).unwrap();
        assert!(out[..length].ends_with(
            b"loader=lib/ld-musl-x86_64.so.1\nlibraries=lib:usr/lib\n\
                  arguments=--data-dir /Applications/brogue/share/brogue\n"
        ));
        assert_eq!(parse(&out[..length], b"brogue"), Some(manifest));
        let ignored = b"eggos-app 1\nname=A\nsource=linux\nrun=a\nloader=/abs\nlibraries=a::b\n\
                        arguments=$(reboot)\n";
        let read = parse(ignored, b"A").unwrap();
        assert_eq!(
            (read.loader, read.libraries, read.arguments),
            (None, None, None)
        );
    }

    #[test]
    fn an_environment_of_plain_variables_reads_back_and_anything_else_is_refused() {
        let environment = "XDG_DATA_DIRS=/Applications/mousepad/share NO_AT_BRIDGE=1 LANG=";
        assert!(is_environment(environment.as_bytes()));
        let manifest = Manifest {
            name: "mousepad",
            source: Source::Linux,
            run: "mousepad",
            architecture: Some("aarch64"),
            synonyms: "",
            icon: None,
            loader: Some("lib/ld-musl-aarch64.so.1"),
            libraries: Some("lib"),
            arguments: Some("--disable-server"),
            environment: Some(environment),
        };
        let mut out = [0u8; LARGEST];
        let length = write(&manifest, &mut out).unwrap();
        assert!(out[..length].ends_with(
            b"environment=XDG_DATA_DIRS=/Applications/mousepad/share NO_AT_BRIDGE=1 LANG=\n"
        ));
        assert_eq!(parse(&out[..length], b"mousepad"), Some(manifest));
        for bad in [
            "",
            " A=1",
            "A=1  B=2",
            "A=1 ",
            "1A=x",
            "A-B=x",
            "=x",
            "A",
            "A=$(reboot)",
            "A=x;reboot",
            "A='x'",
        ] {
            assert!(!is_environment(bad.as_bytes()), "{bad:?}");
            assert_eq!(
                write(
                    &Manifest {
                        environment: Some(bad),
                        ..manifest
                    },
                    &mut out
                ),
                Err(Refused::InvalidValue),
                "{bad:?}"
            );
        }
        let long = "A=".to_owned() + &"x".repeat(ENVIRONMENT_CAPACITY - 1);
        assert!(!is_environment(long.as_bytes()));
        let ignored = b"eggos-app 1\nname=A\nsource=linux\nrun=a\nenvironment=PATH=$HOME\n";
        assert_eq!(parse(ignored, b"A").unwrap().environment, None);
    }

    #[test]
    fn every_byte_string_reads_without_panicking() {
        let mut state = 0x9e37_79b9_u32;
        for length in 0..400 {
            let mut file = b"eggos-app 1\nname=A\nsource=linux\nrun=/a\n".to_vec();
            for _ in 0..length {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                file.push(b"=\n\rnamesourcerun/.,' \xff"[(state % 22) as usize]);
            }
            if let Some(manifest) = parse(&file, b"A") {
                let _ = manifest.synonyms().count();
            }
        }
    }
}
