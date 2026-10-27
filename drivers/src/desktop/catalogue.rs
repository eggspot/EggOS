//! The apps the shell offers (docs/ux/desktop-v0.3-gui.md, sections 6.2 and 6.3, D133): the
//! built-in Terminal, Files and Settings, the host's shared folder while it is mounted, and the
//! apps installed under `/Applications`, read from their manifests by the kernel when the
//! desktop starts and when the launcher opens. The launcher's search matches a name or one of
//! its synonyms, case-insensitively.

use super::icons::{Icon, Source};
use alloc::vec::Vec;
use eggos_app_manifest::{self as manifest, NAME_CAPACITY, SYNONYMS_CAPACITY};

/// The most installed apps the shell lists; more are left out.
pub const MAX_INSTALLED: usize = 32;
/// The longest command a launch hands the kernel, in bytes.
pub const COMMAND_CAPACITY: usize = 256;

const TERMINAL_SYNONYMS: [&str; 5] = ["shell", "console", "cmd", "iTerm", "bash"];
const FILES_SYNONYMS: [&str; 5] = ["Explorer", "Finder", "Dolphin", "Nautilus", "folders"];
const SETTINGS_SYNONYMS: [&str; 4] = ["Control Panel", "System Settings", "Preferences", "options"];
const SETTINGS_PROGRAM: &str = "/bin/settings";
const FILES_PROGRAM: &str = "/bin/files";
const HOST_SHARE_LISTING: &str = "ls /media/host";
const PLAIN_PATH_BYTES: &[u8] = b"/._-+";

/// An app the shell can launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppId {
    /// The console terminal's window.
    Terminal,
    /// The Files app (`/bin/files`), when the build has it.
    Files,
    /// The Settings app (`/bin/settings`).
    Settings,
    /// The host's shared folder at `/media/host`.
    HostShare,
    /// The installed app at this index of the catalogue.
    Installed(u8),
}

/// What launching an app asks of the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// Start the program at this path as a process.
    Start(Command),
    /// Type this line in the terminal, as if the user had.
    Type(Command),
}

/// A program's path or a line to type, at most [`COMMAND_CAPACITY`] bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Command {
    bytes: [u8; COMMAND_CAPACITY],
    length: usize,
}

impl core::fmt::Debug for Command {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.as_bytes().escape_ascii(), formatter)
    }
}

impl Command {
    fn new() -> Self {
        Self {
            bytes: [0; COMMAND_CAPACITY],
            length: 0,
        }
    }

    fn push(&mut self, part: &[u8]) -> bool {
        let Some(end) = self
            .length
            .checked_add(part.len())
            .filter(|&end| end <= COMMAND_CAPACITY)
        else {
            return false;
        };
        self.bytes[self.length..end].copy_from_slice(part);
        self.length = end;
        true
    }

    fn from(part: &str) -> Self {
        let mut command = Self::new();
        command.push(part.as_bytes());
        command
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

#[derive(Debug, Clone, Copy)]
struct InstalledApp {
    name: [u8; NAME_CAPACITY],
    name_length: u8,
    source: Source,
    program: Command,
    synonyms: [u8; SYNONYMS_CAPACITY],
    synonyms_length: u8,
}

impl InstalledApp {
    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..usize::from(self.name_length)]).unwrap_or("")
    }

    fn file_name(&self) -> &str {
        let path = self.program.as_bytes();
        let start = path
            .iter()
            .rposition(|&byte| byte == b'/')
            .map_or(0, |slash| slash + 1);
        core::str::from_utf8(&path[start..]).unwrap_or("")
    }

    fn synonyms(&self) -> impl Iterator<Item = &str> {
        core::str::from_utf8(&self.synonyms[..usize::from(self.synonyms_length)])
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .take(manifest::MAX_SYNONYMS)
    }
}

/// The catalogue. See the module documentation.
#[derive(Debug, Default)]
pub struct Catalogue {
    installed: Vec<InstalledApp>,
    host_share: bool,
    files_present: bool,
}

fn contains_ignoring_case(text: &str, query: &str) -> bool {
    let (text, query) = (text.as_bytes(), query.as_bytes());
    query.is_empty()
        || text
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query))
}

fn lower(text: &str) -> impl Iterator<Item = u8> + '_ {
    text.bytes().map(|byte| byte.to_ascii_lowercase())
}

fn ordered(first: &str, second: &str) -> core::cmp::Ordering {
    lower(first)
        .cmp(lower(second))
        .then_with(|| first.cmp(second))
}

impl Catalogue {
    /// The built-ins alone: no host share, no Files, nothing installed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets every installed app, before a new scan adds them again.
    pub fn clear_installed(&mut self) {
        self.installed.clear();
    }

    /// Adds the app whose folder in `/Applications` is `folder` and whose manifest holds
    /// `file`, in name order; returns whether it was added (a valid manifest, room for it, a
    /// name not listed yet).
    pub fn add_installed(&mut self, folder: &[u8], file: &[u8]) -> bool {
        if self.installed.len() >= MAX_INSTALLED {
            return false;
        }
        let Some(app) = manifest::parse(file, folder) else {
            return false;
        };
        let mut program = Command::new();
        let resolved = match app.run_is_relative() {
            true => [
                manifest::DIRECTORY.as_bytes(),
                b"/",
                app.name.as_bytes(),
                b"/",
                app.run.as_bytes(),
            ]
            .iter()
            .all(|part| program.push(part)),
            false => program.push(app.run.as_bytes()),
        };
        let at = match self
            .installed
            .binary_search_by(|listed| ordered(listed.name(), app.name))
        {
            Ok(_) => return false,
            Err(at) => at,
        };
        if !resolved || self.installed.try_reserve(1).is_err() {
            return false;
        }
        let mut entry = InstalledApp {
            name: [0; NAME_CAPACITY],
            name_length: app.name.len() as u8,
            source: app.source,
            program,
            synonyms: [0; SYNONYMS_CAPACITY],
            synonyms_length: app.synonyms.len() as u8,
        };
        entry.name[..app.name.len()].copy_from_slice(app.name.as_bytes());
        entry.synonyms[..app.synonyms.len()].copy_from_slice(app.synonyms.as_bytes());
        self.installed.insert(at, entry);
        true
    }

    /// Records whether the host's shared folder is mounted at `/media/host`.
    pub fn set_host_share(&mut self, mounted: bool) {
        self.host_share = mounted;
    }

    /// Whether the host's shared folder is mounted.
    pub fn host_share(&self) -> bool {
        self.host_share
    }

    /// Records whether the build has the Files app.
    pub fn set_files_present(&mut self, present: bool) {
        self.files_present = present;
    }

    /// Whether the build has the Files app.
    pub fn files_present(&self) -> bool {
        self.files_present
    }

    /// How many apps are installed.
    pub fn installed_count(&self) -> usize {
        self.installed.len()
    }

    fn installed(&self, index: u8) -> Option<&InstalledApp> {
        self.installed.get(usize::from(index))
    }

    /// Whether `app` is in the catalogue now.
    pub fn has(&self, app: AppId) -> bool {
        match app {
            AppId::HostShare => self.host_share,
            AppId::Installed(index) => self.installed(index).is_some(),
            AppId::Terminal | AppId::Files | AppId::Settings => true,
        }
    }

    /// The installed app called `name`, if there is one.
    pub fn installed_named(&self, name: &str) -> Option<AppId> {
        self.installed
            .iter()
            .position(|app| app.name() == name)
            .map(|index| AppId::Installed(index as u8))
    }

    /// `app`'s name as the shell shows it.
    pub fn name(&self, app: AppId) -> &str {
        match app {
            AppId::Terminal => "Terminal",
            AppId::Files => "Files",
            AppId::Settings => "Settings",
            AppId::HostShare => "Host share",
            AppId::Installed(index) => self.installed(index).map_or("", InstalledApp::name),
        }
    }

    /// The system an installed app was built for; `None` for the built-ins.
    pub fn source(&self, app: AppId) -> Option<Source> {
        match app {
            AppId::Installed(index) => self.installed(index).map(|app| app.source),
            _ => None,
        }
    }

    /// `app`'s icon.
    pub fn icon(&self, app: AppId) -> Icon {
        match app {
            AppId::Terminal => Icon::Terminal,
            AppId::Files => Icon::Files,
            AppId::Settings => Icon::Settings,
            AppId::HostShare => Icon::HostShare,
            AppId::Installed(_) => Icon::named(self.name(app), self.source(app)),
        }
    }

    /// The apps the launcher lists, in its order: the built-ins, then the installed apps, each
    /// alphabetically.
    pub fn launcher_apps(&self) -> impl Iterator<Item = AppId> + '_ {
        [AppId::Files, AppId::Settings, AppId::Terminal]
            .into_iter()
            .chain((0..self.installed.len()).map(|index| AppId::Installed(index as u8)))
    }

    /// The apps the desktop shows icons for, in order: the built-ins, the host share while it
    /// is mounted, then the installed apps alphabetically.
    pub fn desktop_apps(&self) -> impl Iterator<Item = AppId> + '_ {
        [AppId::Terminal, AppId::Files, AppId::Settings]
            .into_iter()
            .chain(self.host_share.then_some(AppId::HostShare))
            .chain((0..self.installed.len()).map(|index| AppId::Installed(index as u8)))
    }

    /// Whether `query` is a case-insensitive part of `app`'s name or of one of its synonyms
    /// (an installed app's program's file name is one).
    pub fn matches(&self, app: AppId, query: &str) -> bool {
        if contains_ignoring_case(self.name(app), query) {
            return true;
        }
        let built_in: &[&str] = match app {
            AppId::Terminal => &TERMINAL_SYNONYMS,
            AppId::Files => &FILES_SYNONYMS,
            AppId::Settings => &SETTINGS_SYNONYMS,
            AppId::HostShare | AppId::Installed(_) => &[],
        };
        if built_in
            .iter()
            .any(|synonym| contains_ignoring_case(synonym, query))
        {
            return true;
        }
        let AppId::Installed(index) = app else {
            return false;
        };
        self.installed(index).is_some_and(|installed| {
            contains_ignoring_case(installed.file_name(), query)
                || installed
                    .synonyms()
                    .any(|synonym| contains_ignoring_case(synonym, query))
        })
    }

    /// What launching `app` asks of the kernel, if anything: Settings and Files start their
    /// programs; the host share and installed apps are typed in the terminal (the program's
    /// path, single-quoted when it holds more than letters, digits and `/._-+`); the terminal
    /// is only focused, which the desktop does itself.
    pub fn launch(&self, app: AppId) -> Option<Launch> {
        match app {
            AppId::Terminal => None,
            AppId::Files => Some(Launch::Start(Command::from(FILES_PROGRAM))),
            AppId::Settings => Some(Launch::Start(Command::from(SETTINGS_PROGRAM))),
            AppId::HostShare => {
                let mut line = Command::from(HOST_SHARE_LISTING);
                line.push(b"\r");
                Some(Launch::Type(line))
            }
            AppId::Installed(index) => {
                let path = self.installed(index)?.program;
                let plain = path
                    .as_bytes()
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || PLAIN_PATH_BYTES.contains(byte));
                let mut line = Command::new();
                let typed = match plain {
                    true => line.push(path.as_bytes()) && line.push(b"\r"),
                    false => line.push(b"'") && line.push(path.as_bytes()) && line.push(b"'\r"),
                };
                typed.then_some(Launch::Type(line))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str, source: &str, run: &str, synonyms: &str) -> Vec<u8> {
        format!("eggos-app 1\nname={name}\nsource={source}\nrun={run}\nsynonyms={synonyms}\n")
            .into_bytes()
    }

    fn catalogue() -> Catalogue {
        let mut catalogue = Catalogue::new();
        for (name, source, run, synonyms) in [
            ("zebra", "linux", "/usr/bin/zebra", ""),
            ("Paint", "windows", "paint.exe", "draw, mspaint"),
            ("Hello World", "macos", "Hello.app/Contents/MacOS/hello", ""),
        ] {
            assert!(
                catalogue.add_installed(name.as_bytes(), &manifest(name, source, run, synonyms))
            );
        }
        catalogue
    }

    fn names(catalogue: &Catalogue, apps: impl Iterator<Item = AppId>) -> Vec<String> {
        apps.map(|app| catalogue.name(app).to_owned()).collect()
    }

    #[test]
    fn the_launcher_lists_the_built_ins_then_installed_apps_alphabetically() {
        let catalogue = catalogue();
        assert_eq!(
            names(&catalogue, catalogue.launcher_apps()),
            [
                "Files",
                "Settings",
                "Terminal",
                "Hello World",
                "Paint",
                "zebra"
            ]
        );
    }

    #[test]
    fn the_desktop_lists_the_host_share_only_while_it_is_mounted() {
        let mut catalogue = catalogue();
        assert_eq!(
            names(&catalogue, catalogue.desktop_apps()),
            [
                "Terminal",
                "Files",
                "Settings",
                "Hello World",
                "Paint",
                "zebra"
            ]
        );
        catalogue.set_host_share(true);
        assert_eq!(names(&catalogue, catalogue.desktop_apps())[3], "Host share");
        assert!(catalogue.has(AppId::HostShare));
    }

    #[test]
    fn a_scan_skips_garbage_duplicates_and_apps_past_the_limit() {
        let mut catalogue = Catalogue::new();
        assert!(!catalogue.add_installed(b"x", b"\xff\x00 not a manifest"));
        assert!(!catalogue.add_installed(b"Other", &manifest("A", "linux", "/a", "")));
        assert!(catalogue.add_installed(b"A", &manifest("A", "linux", "/a", "")));
        assert!(!catalogue.add_installed(b"A", &manifest("A", "linux", "/b", "")));
        for index in 1..MAX_INSTALLED {
            let name = format!("app{index:02}");
            assert!(catalogue.add_installed(name.as_bytes(), &manifest(&name, "linux", "/a", "")));
        }
        assert!(!catalogue.add_installed(b"late", &manifest("late", "linux", "/a", "")));
        assert_eq!(catalogue.installed_count(), MAX_INSTALLED);
        catalogue.clear_installed();
        assert_eq!(catalogue.installed_count(), 0);
        assert!(!catalogue.has(AppId::Installed(0)));
    }

    #[test]
    fn the_search_matches_names_and_synonyms_ignoring_case() {
        let catalogue = catalogue();
        let found = |query: &str| {
            names(
                &catalogue,
                catalogue
                    .launcher_apps()
                    .filter(|&app| catalogue.matches(app, query)),
            )
        };
        assert_eq!(found("pref"), ["Settings"]);
        assert_eq!(found("control panel"), ["Settings"]);
        assert_eq!(found("FINDER"), ["Files"]);
        assert_eq!(found("cmd"), ["Terminal"]);
        assert_eq!(found("iterm"), ["Terminal"]);
        assert_eq!(found("draw"), ["Paint"]);
        assert_eq!(found("paint.exe"), ["Paint"]);
        assert_eq!(found("hello"), ["Hello World"]);
        assert_eq!(found("zz"), Vec::<String>::new());
        assert_eq!(found("").len(), 6);
        assert_eq!(found("s"), ["Files", "Settings", "Terminal", "Paint"]);
    }

    #[test]
    fn launching_starts_built_in_programs_and_types_the_others() {
        let mut catalogue = catalogue();
        catalogue.set_host_share(true);
        let start = |app| match catalogue.launch(app) {
            Some(Launch::Start(path)) => path.as_bytes().to_vec(),
            other => panic!("{other:?}"),
        };
        let typed = |app| match catalogue.launch(app) {
            Some(Launch::Type(line)) => line.as_bytes().to_vec(),
            other => panic!("{other:?}"),
        };
        assert_eq!(start(AppId::Settings), b"/bin/settings");
        assert_eq!(start(AppId::Files), b"/bin/files");
        assert_eq!(catalogue.launch(AppId::Terminal), None);
        assert_eq!(typed(AppId::HostShare), b"ls /media/host\r");
        let paint = catalogue.installed_named("Paint").unwrap();
        assert_eq!(typed(paint), b"/Applications/Paint/paint.exe\r");
        let zebra = catalogue.installed_named("zebra").unwrap();
        assert_eq!(typed(zebra), b"/usr/bin/zebra\r");
        let hello = catalogue.installed_named("Hello World").unwrap();
        assert_eq!(
            typed(hello),
            b"'/Applications/Hello World/Hello.app/Contents/MacOS/hello'\r"
        );
        assert_eq!(catalogue.launch(AppId::Installed(9)), None);
    }

    #[test]
    fn installed_apps_wear_their_source_s_badge() {
        let catalogue = catalogue();
        let paint = catalogue.installed_named("Paint").unwrap();
        assert_eq!(
            catalogue.icon(paint),
            Icon::App {
                letter: b'P',
                badge: Some(Source::Windows)
            }
        );
        assert_eq!(catalogue.source(paint).unwrap().label(), "Windows");
        assert_eq!(catalogue.source(AppId::Settings), None);
        assert_eq!(catalogue.icon(AppId::HostShare), Icon::HostShare);
    }
}
