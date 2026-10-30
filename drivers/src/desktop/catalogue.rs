//! The apps the shell offers (docs/ux/desktop-v0.3-gui.md, sections 6.2 and 6.3, D133): the
//! built-in Terminal, Files, Settings and the Text Editor (D149, launcher only), the host's
//! shared folder while it is mounted, and the
//! apps installed under `/Applications`, read from their manifests by the kernel when the
//! desktop starts, when the launcher opens and when the installer says the apps changed
//! (D134); and, while the share is mounted, the launcher's `Install from host share...` row.
//! The launcher's search matches a name or one of its synonyms, case-insensitively.

use super::icons::{Icon, IconPixels, Source};
use alloc::vec::Vec;
use eggos_app_manifest::{self as manifest, NAME_CAPACITY, SYNONYMS_CAPACITY};

/// The most installed apps the shell lists; more are left out.
pub const MAX_INSTALLED: usize = 32;
/// The longest command a launch hands the kernel, in bytes.
pub const COMMAND_CAPACITY: usize = 256;

const TERMINAL_SYNONYMS: [&str; 5] = ["shell", "console", "cmd", "iTerm", "bash"];
const FILES_SYNONYMS: [&str; 5] = ["Explorer", "Finder", "Dolphin", "Nautilus", "folders"];
const SETTINGS_SYNONYMS: [&str; 4] = ["Control Panel", "System Settings", "Preferences", "options"];
const TEXT_EDITOR_SYNONYMS: [&str; 6] = ["Notepad", "TextEdit", "gedit", "Kate", "editor", "notes"];
const SETTINGS_PROGRAM: &str = "/bin/settings";
const FILES_PROGRAM: &str = "/bin/files";
const TEXT_EDITOR_PROGRAM: &str = "/bin/text-editor";
const HOST_SHARE_LISTING: &str = "ls /media/host";
const HOST_SHARE: &str = "/media/host";
const PLAIN_PATH_BYTES: &[u8] = b"/._-+";
const KILL_LINE: &[u8] = b"\x15";
const INSTALL_FROM_HOST_SHARE: &str = "Install from host share...";

/// The installer's window (D134): `app-window install <path>` and `app-window uninstall
/// <name>`.
pub const INSTALLER_PROGRAM: &str = "/usr/libexec/app-window";
/// What separates a started program's path and its arguments in a [`Command`].
pub const ARGUMENT_SEPARATOR: u8 = 0;

const BUILT_INS: [AppId; 4] = [
    AppId::Terminal,
    AppId::Files,
    AppId::Settings,
    AppId::TextEditor,
];

/// An app the shell can launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppId {
    /// The console terminal's window.
    Terminal,
    /// The Files app (`/bin/files`), when the build has it.
    Files,
    /// The Settings app (`/bin/settings`).
    Settings,
    /// The Text Editor (`/bin/text-editor`, D149), when the build has it.
    TextEditor,
    /// The host's shared folder at `/media/host`.
    HostShare,
    /// The launcher's row that opens the installer on the host's shared folder.
    InstallFromHostShare,
    /// The installed app at this index of the catalogue.
    Installed(u8),
}

/// What launching an app asks of the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// Start the program as a process: its path, then each argument, separated by
    /// [`ARGUMENT_SEPARATOR`].
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

    fn program(path: &str, arguments: &[&[u8]]) -> Option<Self> {
        let mut command = Self::from(path);
        arguments
            .iter()
            .all(|argument| {
                !argument.contains(&ARGUMENT_SEPARATOR)
                    && command.push(&[ARGUMENT_SEPARATOR])
                    && command.push(argument)
            })
            .then_some(command)
    }

    /// A started program's path and arguments, in order.
    pub fn words(&self) -> impl Iterator<Item = &[u8]> {
        self.as_bytes().split(|&byte| byte == ARGUMENT_SEPARATOR)
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstalledApp {
    name: [u8; NAME_CAPACITY],
    name_length: u8,
    source: Source,
    program: Command,
    line: Command,
    synonyms: [u8; SYNONYMS_CAPACITY],
    synonyms_length: u8,
    icon: Option<IconPixels>,
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

fn push_word(line: &mut Command, path: &[u8]) -> bool {
    let plain = path
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || PLAIN_PATH_BYTES.contains(byte));
    match plain {
        true => line.push(path),
        false => line.push(b"'") && line.push(path) && line.push(b"'"),
    }
}

fn push_in_folder(line: &mut Command, name: &str, relative: &str) -> bool {
    [
        manifest::DIRECTORY.as_bytes(),
        b"/",
        name.as_bytes(),
        b"/",
        relative.as_bytes(),
    ]
    .iter()
    .all(|part| line.push(part))
}

fn launch_line(app: &manifest::Manifest<'_>, program: &Command) -> Option<Command> {
    let mut line = Command::new();
    if let Some(loader) = app.loader {
        let mut path = Command::new();
        if !push_in_folder(&mut path, app.name, loader)
            || !push_word(&mut line, path.as_bytes())
            || !line.push(b" ")
        {
            return None;
        }
        if let Some(libraries) = app.libraries {
            let mut folders = Command::new();
            for (at, folder) in libraries.split(':').enumerate() {
                if (at > 0 && !folders.push(b":"))
                    || !push_in_folder(&mut folders, app.name, folder)
                {
                    return None;
                }
            }
            if !line.push(b"--library-path ")
                || !push_word(&mut line, folders.as_bytes())
                || !line.push(b" ")
            {
                return None;
            }
        }
    }
    if !push_word(&mut line, program.as_bytes()) {
        return None;
    }
    if let Some(arguments) = app.arguments
        && (!line.push(b" ") || !line.push(arguments.as_bytes()))
    {
        return None;
    }
    Some(line)
}

/// The catalogue. See the module documentation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Catalogue {
    installed: Vec<InstalledApp>,
    host_share: bool,
    files_present: bool,
    text_editor_present: bool,
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
    /// The built-ins alone: no host share, no Files, no Text Editor, nothing installed.
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
        let line = launch_line(&app, &program);
        let Some(line) = line.filter(|_| resolved) else {
            return false;
        };
        if self.installed.try_reserve(1).is_err() {
            return false;
        }
        let mut entry = InstalledApp {
            name: [0; NAME_CAPACITY],
            name_length: app.name.len() as u8,
            source: app.source,
            program,
            line,
            synonyms: [0; SYNONYMS_CAPACITY],
            synonyms_length: app.synonyms.len() as u8,
            icon: None,
        };
        entry.name[..app.name.len()].copy_from_slice(app.name.as_bytes());
        entry.synonyms[..app.synonyms.len()].copy_from_slice(app.synonyms.as_bytes());
        self.installed.insert(at, entry);
        true
    }

    /// Gives the installed app whose folder is `folder` its own icon, from its `app.icon` file
    /// (D148); returns whether it took it (an app of that name, a file of the right length).
    pub fn set_icon(&mut self, folder: &[u8], file: &[u8]) -> bool {
        let Some(app) = self
            .installed
            .iter_mut()
            .find(|app| app.name().as_bytes() == folder)
        else {
            return false;
        };
        app.icon = IconPixels::from_file(file);
        app.icon.is_some()
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

    /// Records whether the build has the Text Editor.
    pub fn set_text_editor_present(&mut self, present: bool) {
        self.text_editor_present = present;
    }

    /// Whether the build has the Text Editor.
    pub fn text_editor_present(&self) -> bool {
        self.text_editor_present
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
            AppId::HostShare | AppId::InstallFromHostShare => self.host_share,
            AppId::Installed(index) => self.installed(index).is_some(),
            AppId::Terminal | AppId::Files | AppId::Settings | AppId::TextEditor => true,
        }
    }

    /// The installed app called `name`, if there is one.
    pub fn installed_named(&self, name: &str) -> Option<AppId> {
        self.installed
            .iter()
            .position(|app| app.name() == name)
            .map(|index| AppId::Installed(index as u8))
    }

    /// The app called `name`: the built-ins Terminal, Files, Settings and Text Editor first, so
    /// an installed app of the same name never stands in for one, then the installed apps.
    pub fn app_named(&self, name: &str) -> Option<AppId> {
        BUILT_INS
            .into_iter()
            .find(|&app| self.name(app) == name)
            .or_else(|| self.installed_named(name))
    }

    /// `app`'s name as the shell shows it.
    pub fn name(&self, app: AppId) -> &str {
        match app {
            AppId::Terminal => "Terminal",
            AppId::Files => "Files",
            AppId::Settings => "Settings",
            AppId::TextEditor => "Text Editor",
            AppId::HostShare => "Host share",
            AppId::InstallFromHostShare => INSTALL_FROM_HOST_SHARE,
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
    pub fn icon(&self, app: AppId) -> Icon<'_> {
        match app {
            AppId::Terminal => Icon::Terminal,
            AppId::Files => Icon::Files,
            AppId::Settings => Icon::Settings,
            AppId::TextEditor => Icon::TextEditor,
            AppId::HostShare | AppId::InstallFromHostShare => Icon::HostShare,
            AppId::Installed(index) => {
                match self.installed(index).and_then(|app| app.icon.as_ref()) {
                    Some(pixels) => Icon::Own(pixels),
                    None => Icon::named(self.name(app), self.source(app)),
                }
            }
        }
    }

    /// The apps the launcher lists, in its order: the built-ins, then the installed apps, each
    /// alphabetically, then `Install from host share...` while the share is mounted.
    pub fn launcher_apps(&self) -> impl Iterator<Item = AppId> + '_ {
        [
            AppId::Files,
            AppId::Settings,
            AppId::Terminal,
            AppId::TextEditor,
        ]
        .into_iter()
        .chain((0..self.installed.len()).map(|index| AppId::Installed(index as u8)))
        .chain(self.host_share.then_some(AppId::InstallFromHostShare))
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
            AppId::TextEditor => &TEXT_EDITOR_SYNONYMS,
            AppId::HostShare | AppId::InstallFromHostShare | AppId::Installed(_) => &[],
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
    /// programs, `Install from host share...` the installer on the share, and the host share
    /// Files on it (D135); without Files the share's listing and installed apps are typed in
    /// the terminal (the program's path, single-quoted when it holds more than letters, digits
    /// and `/._-+`; for a Linux program carrying its own dynamic loader, the loader first with
    /// `--library-path` and its library folders; then the app's arguments, D172), after Ctrl-U
    /// so a half-typed line does not run with it; the terminal is only focused, which the
    /// desktop does itself.
    pub fn launch(&self, app: AppId) -> Option<Launch> {
        match app {
            AppId::Terminal => None,
            AppId::Files => Some(Launch::Start(Command::from(FILES_PROGRAM))),
            AppId::Settings => Some(Launch::Start(Command::from(SETTINGS_PROGRAM))),
            AppId::TextEditor => Some(Launch::Start(Command::from(TEXT_EDITOR_PROGRAM))),
            AppId::InstallFromHostShare => {
                Command::program(INSTALLER_PROGRAM, &[b"install", HOST_SHARE.as_bytes()])
                    .map(Launch::Start)
            }
            AppId::HostShare if self.files_present => {
                Command::program(FILES_PROGRAM, &[HOST_SHARE.as_bytes()]).map(Launch::Start)
            }
            AppId::HostShare => {
                let mut line = Command::new();
                let typed = line.push(KILL_LINE)
                    && line.push(HOST_SHARE_LISTING.as_bytes())
                    && line.push(b"\r");
                typed.then_some(Launch::Type(line))
            }
            AppId::Installed(index) => {
                let typed = self.installed(index)?.line;
                let mut line = Command::new();
                let fits = line.push(KILL_LINE) && line.push(typed.as_bytes()) && line.push(b"\r");
                fits.then_some(Launch::Type(line))
            }
        }
    }

    /// What uninstalling installed app `app` starts: the installer's window asking first; `None`
    /// for an app that is not installed.
    pub fn uninstall(&self, app: AppId) -> Option<Command> {
        let AppId::Installed(index) = app else {
            return None;
        };
        let name = self.installed(index)?.name();
        Command::program(INSTALLER_PROGRAM, &[b"uninstall", name.as_bytes()])
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
                "Text Editor",
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
        assert_eq!(found("").len(), 7);
        assert_eq!(
            found("s"),
            ["Files", "Settings", "Terminal", "Text Editor", "Paint"]
        );
        assert_eq!(found("notepad"), ["Text Editor"]);
        assert_eq!(found("gedit"), ["Text Editor"]);
    }

    #[test]
    fn apps_are_found_by_name_the_built_ins_first() {
        let mut catalogue = catalogue();
        assert!(
            catalogue.add_installed(b"Settings", &manifest("Settings", "linux", "/bin/fake", ""))
        );
        assert_eq!(catalogue.app_named("Settings"), Some(AppId::Settings));
        assert_eq!(catalogue.app_named("Files"), Some(AppId::Files));
        assert_eq!(catalogue.app_named("Terminal"), Some(AppId::Terminal));
        assert_eq!(catalogue.app_named("Text Editor"), Some(AppId::TextEditor));
        assert_eq!(
            catalogue.app_named("Paint"),
            catalogue.installed_named("Paint")
        );
        assert!(catalogue.app_named("Paint").is_some());
        assert_eq!(catalogue.app_named("settings"), None);
        assert_eq!(catalogue.app_named("Host share"), None);
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
        assert_eq!(start(AppId::TextEditor), b"/bin/text-editor");
        assert_eq!(catalogue.icon(AppId::TextEditor), Icon::TextEditor);
        assert_eq!(catalogue.launch(AppId::Terminal), None);
        assert_eq!(typed(AppId::HostShare), b"\x15ls /media/host\r");
        let paint = catalogue.installed_named("Paint").unwrap();
        assert_eq!(typed(paint), b"\x15/Applications/Paint/paint.exe\r");
        let zebra = catalogue.installed_named("zebra").unwrap();
        assert_eq!(typed(zebra), b"\x15/usr/bin/zebra\r");
        let hello = catalogue.installed_named("Hello World").unwrap();
        assert_eq!(
            typed(hello),
            b"\x15'/Applications/Hello World/Hello.app/Contents/MacOS/hello'\r"
        );
        assert_eq!(catalogue.launch(AppId::Installed(9)), None);
        let installer = start(AppId::InstallFromHostShare);
        assert_eq!(installer, b"/usr/libexec/app-window\0install\0/media/host");
        catalogue.set_files_present(true);
        match catalogue.launch(AppId::HostShare) {
            Some(Launch::Start(command)) => {
                assert_eq!(command.as_bytes(), b"/bin/files\0/media/host")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_program_with_its_own_loader_opens_through_it_with_its_arguments() {
        let mut catalogue = Catalogue::new();
        let file = b"eggos-app 1\nname=brogue\nsource=linux\nrun=brogue\n\
                     loader=lib/ld-musl-x86_64.so.1\nlibraries=lib:usr/lib\n\
                     arguments=--data-dir /Applications/brogue/share/brogue\n";
        assert!(catalogue.add_installed(b"brogue", file));
        let brogue = catalogue.installed_named("brogue").unwrap();
        match catalogue.launch(brogue) {
            Some(Launch::Type(line)) => assert_eq!(
                line.as_bytes(),
                b"\x15/Applications/brogue/lib/ld-musl-x86_64.so.1 --library-path \
                  /Applications/brogue/lib:/Applications/brogue/usr/lib \
                  /Applications/brogue/brogue --data-dir /Applications/brogue/share/brogue\r"
            ),
            other => panic!("{other:?}"),
        }
        let long = format!(
            "eggos-app 1\nname=long\nsource=linux\nrun={}\nloader={}\narguments={}\n",
            "p".repeat(150),
            "l".repeat(100),
            "a".repeat(100)
        );
        assert!(!catalogue.add_installed(b"long", long.as_bytes()));
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

    #[test]
    fn an_app_with_its_own_icon_shows_it_and_a_wrong_file_keeps_the_tile() {
        use eggos_app_manifest::icon as icon_file;
        let mut catalogue = catalogue();
        let file = vec![0x7f; icon_file::FILE_BYTES];
        assert!(catalogue.set_icon(b"Paint", &file));
        let paint = catalogue.installed_named("Paint").unwrap();
        let expected = IconPixels::from_file(&file).unwrap();
        assert_eq!(catalogue.icon(paint), Icon::Own(&expected));
        assert!(!catalogue.set_icon(b"Nobody", &file));
        assert!(!catalogue.set_icon(b"zebra", &file[1..]));
        let zebra = catalogue.installed_named("zebra").unwrap();
        assert_eq!(
            catalogue.icon(zebra),
            Icon::App {
                letter: b'Z',
                badge: Some(Source::Linux)
            }
        );
        catalogue.clear_installed();
        assert!(!catalogue.set_icon(b"Paint", &file));
    }

    #[test]
    fn the_installer_row_follows_the_share_and_uninstall_starts_the_installer_s_window() {
        let mut catalogue = catalogue();
        assert!(!catalogue.has(AppId::InstallFromHostShare));
        assert!(
            !catalogue
                .launcher_apps()
                .any(|app| app == AppId::InstallFromHostShare)
        );
        catalogue.set_host_share(true);
        assert_eq!(
            catalogue.launcher_apps().last(),
            Some(AppId::InstallFromHostShare)
        );
        assert!(
            !catalogue
                .desktop_apps()
                .any(|app| app == AppId::InstallFromHostShare)
        );
        assert_eq!(
            catalogue.name(AppId::InstallFromHostShare),
            "Install from host share..."
        );
        assert!(catalogue.matches(AppId::InstallFromHostShare, "install"));
        assert_eq!(catalogue.icon(AppId::InstallFromHostShare), Icon::HostShare);
        let hello = catalogue.installed_named("Hello World").unwrap();
        let command = catalogue.uninstall(hello).unwrap();
        let words: Vec<&[u8]> = command.words().collect();
        assert_eq!(
            words,
            [
                &b"/usr/libexec/app-window"[..],
                b"uninstall",
                b"Hello World"
            ]
        );
        assert_eq!(catalogue.uninstall(AppId::Settings), None);
        assert_eq!(catalogue.uninstall(AppId::Installed(9)), None);
        let settings = match catalogue.launch(AppId::Settings) {
            Some(Launch::Start(command)) => command,
            other => panic!("{other:?}"),
        };
        assert_eq!(settings.words().count(), 1);
    }
}
