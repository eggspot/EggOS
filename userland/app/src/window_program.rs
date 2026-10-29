//! The installer's window, `/usr/libexec/app-window` (D134): `app-window install [--name
//! <name>] [--entry <path>] <path>` shows the prompt for the program, `.app` or `.zip` at
//! `path` (for another folder, the chooser of the programs in it; for a `.zip` holding several
//! and no `--entry`, the chooser of its programs, D148), then the progress and the result;
//! `app-window uninstall <name>` asks before uninstalling. `app ... --window` and the desktop's
//! launcher and icon menus start it. It is another name of the toolkit program `/bin/settings`,
//! so the toolkit and its font are in the image once.

use crate::architecture::Architecture;
use crate::archive;
use crate::command::{self, Command};
use crate::dialog::{self, AppIcon, Dialog, Request, SIZE};
use crate::files::{self, Files, Kind, PATH_CAPACITY, Path};
use crate::installer::{self, Installation, Name, Plan, Problem, Progress};
use crate::linux::{self, LinuxFiles, STANDARD_ERROR, write_all};
use crate::messages::{self, Line};
use core::fmt::Write;
use eggos_app_manifest as manifest;
use eggos_runtime::startup::Startup;
use eggos_toolkit::application::{self, Application};
use eggos_toolkit::tree::{Response, WidgetTree};
use eggos_toolkit::window::Window;

const COPY_BUFFER: usize = 64 * 1024;
const STEPS_PER_TICK: usize = 16;
const MAX_ARGUMENTS: usize = 8;
const SIGNATURE_BYTES: usize = 8;

static mut BUFFER: [u8; COPY_BUFFER] = [0; COPY_BUFFER];

#[allow(clippy::large_enum_variant)]
enum Stage {
    Choosing {
        folder: Path,
        names: [Name; dialog::MAX_CHOICES],
    },
    ChoosingEntry {
        archive: Path,
        entries: [Path; dialog::MAX_CHOICES],
    },
    Prompting(Plan),
    Showing,
    Installing(Installation),
    Installed(Plan),
    Confirming(Name),
}

struct Installer {
    dialog: Dialog,
    stage: Stage,
    files: LinuxFiles,
    machine: Architecture,
    name: Option<Name>,
    entry: Option<Path>,
    buffer: &'static mut [u8],
}

const _: () = assert!(archive::NAME_CAPACITY < PATH_CAPACITY);

fn entry_as_given(entry: &[u8]) -> Path {
    Path::from(&entry[..entry.len().min(PATH_CAPACITY)]).unwrap_or_default()
}

fn close() -> ! {
    eggos_runtime::syscall::exit(0)
}

fn is_program_file(files: &mut LinuxFiles, path: &[u8]) -> bool {
    let Ok(file) = files.open(path) else {
        return false;
    };
    let mut head = [0u8; SIGNATURE_BYTES];
    let read = files.read(file, &mut head).unwrap_or(0);
    files.close(file);
    let head = &head[..read];
    head.starts_with(b"MZ")
        || head.starts_with(b"\x7fELF")
        || head.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || head.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
}

fn programs_in(
    files: &mut LinuxFiles,
    folder: &Path,
) -> ([Name; dialog::MAX_CHOICES], usize, usize) {
    let mut names = [Name::new(b""); dialog::MAX_CHOICES];
    let (mut count, mut more) = (0, 0);
    let mut name = [0u8; 256];
    let mut index = 0;
    while let Ok(Some(length)) = files.entry(folder.as_bytes(), index, &mut name) {
        index += 1;
        let entry = &name[..length];
        let Some(path) = folder.join(entry) else {
            continue;
        };
        let wanted = match files
            .status_following(path.as_bytes())
            .map(|status| status.kind)
        {
            Ok(Kind::Directory) => installer::is_bundle_name(entry),
            Ok(Kind::File) => is_program_file(files, path.as_bytes()),
            _ => false,
        };
        if !wanted {
            continue;
        }
        match (count < dialog::MAX_CHOICES, Name::whole(entry)) {
            (true, Some(whole)) => {
                names[count] = whole;
                count += 1;
            }
            _ => more += 1,
        }
    }
    (names, count, more)
}

impl Installer {
    fn show(&mut self, dialog: Dialog, stage: Stage) {
        self.dialog = dialog;
        self.dialog.tree().damage_all();
        self.stage = stage;
    }

    fn icon_of(plan: &Plan) -> AppIcon {
        AppIcon::of(plan.name(), Some(plan.program().source))
    }

    fn prompt_for(&mut self, path: &[u8]) {
        let name = self.name.as_ref().map(Name::as_bytes);
        let entry = self.entry.as_ref().map(Path::as_bytes);
        let (dialog, plan) =
            dialog::install_prompt_entry(&mut self.files, path, name, entry, self.machine);
        match plan {
            Ok(plan) => self.show(dialog, Stage::Prompting(plan)),
            Err(refused)
                if self.entry.is_none()
                    && matches!(refused.problem, Problem::SeveralPrograms(_)) =>
            {
                self.choose_entry(path)
            }
            Err(_) => self.show(dialog, Stage::Showing),
        }
    }

    fn choose_entry(&mut self, path: &[u8]) {
        let archive = Path::from(path).unwrap_or_default();
        let mut entries = [Path::new(); dialog::MAX_CHOICES];
        let (count, more) =
            installer::archive_programs(&mut self.files, archive.as_bytes(), &mut entries);
        let mut shown = [""; dialog::MAX_CHOICES];
        for (slot, entry) in shown.iter_mut().zip(&entries[..count]) {
            *slot = core::str::from_utf8(entry.as_bytes()).unwrap_or("?");
        }
        let mut place = Line::new();
        place.push_bytes(archive.as_bytes());
        let dialog = Dialog::choose_entry(place.as_str(), &shown[..count], more);
        self.show(dialog, Stage::ChoosingEntry { archive, entries });
    }

    fn start_install(&mut self, plan: Plan) {
        let mut status = Line::new();
        let _ = write!(status, "Copying {}...", plan.name());
        let dialog = Dialog::progress(Self::icon_of(&plan), plan.name(), status.as_str());
        self.show(dialog, Stage::Installing(Installation::new(plan)));
    }

    fn uninstall(&mut self, name: Name) {
        match installer::uninstall(&mut self.files, name.as_bytes()) {
            Ok(()) => {
                linux::apps_changed();
                close()
            }
            Err(problem) => {
                let line = messages::uninstall_failed(name.as_str(), problem);
                let dialog = Dialog::message(AppIcon::of(name.as_str(), None), line.as_str());
                self.show(dialog, Stage::Showing);
            }
        }
    }

    fn work(&mut self) {
        let Stage::Installing(installation) = &mut self.stage else {
            return;
        };
        let mut progress = Progress::Copying(0);
        for _ in 0..STEPS_PER_TICK {
            progress = installation.step(&mut self.files, &mut *self.buffer);
            if !matches!(progress, Progress::Copying(_)) {
                break;
            }
        }
        let plan = *installation.plan();
        match progress {
            Progress::Copying(done) => {
                let mut status = Line::new();
                let _ = write!(status, "Copying {}...", plan.name());
                self.dialog
                    .show_progress(done, plan.total_bytes(), status.as_str());
            }
            Progress::Registering => {
                self.dialog.copy_over();
                self.dialog
                    .show_progress(plan.total_bytes(), plan.total_bytes(), "Registering...");
            }
            Progress::Installed => {
                linux::apps_changed();
                let dialog = Dialog::installed(Self::icon_of(&plan), plan.name());
                self.show(dialog, Stage::Installed(plan));
            }
            Progress::Failed(problem) => {
                let line = messages::install_failed(plan.name(), "this", self.machine, problem);
                let dialog = Dialog::message(Self::icon_of(&plan), line.as_str());
                self.show(dialog, Stage::Showing);
            }
        }
    }
}

impl Application<{ dialog::CAPACITY }> for Installer {
    fn tree(&mut self) -> &mut WidgetTree<{ dialog::CAPACITY }> {
        self.dialog.tree()
    }

    fn respond(&mut self, response: Response, _now_ms: u32) {
        let Some(request) = self.dialog.respond(response) else {
            return;
        };
        match request {
            Request::Choose(index) => {
                let (path, entry) = match &self.stage {
                    Stage::Choosing { folder, names } => (
                        names
                            .get(index)
                            .and_then(|name| folder.join(name.as_bytes())),
                        None,
                    ),
                    Stage::ChoosingEntry { archive, entries } => {
                        let entry = entries
                            .get(index)
                            .copied()
                            .filter(|entry| !entry.is_empty());
                        (entry.map(|_| *archive), entry)
                    }
                    _ => (None, None),
                };
                if let Some(path) = path {
                    if entry.is_some() {
                        self.entry = entry;
                    }
                    self.prompt_for(path.as_bytes());
                }
            }
            Request::Install => {
                if let Stage::Prompting(plan) = &self.stage {
                    let plan = *plan;
                    self.start_install(plan);
                }
            }
            Request::Cancel => {
                if let Stage::Installing(installation) = &mut self.stage
                    && !installation.cancel(&mut self.files)
                {
                    return;
                }
                close();
            }
            Request::Open => {
                if let Stage::Installed(plan) = &self.stage {
                    let _ = linux::open_app(plan.name().as_bytes());
                }
                close();
            }
            Request::Uninstall => {
                if let Stage::Confirming(name) = &self.stage {
                    let name = *name;
                    self.uninstall(name);
                }
            }
            Request::Close => close(),
        }
    }

    fn deadline(&self, now_ms: u32) -> Option<u32> {
        matches!(self.stage, Stage::Installing(_)).then_some(now_ms)
    }

    fn tick(&mut self, _now_ms: u32) {
        self.work();
    }
}

fn complain(parts: &[&[u8]]) {
    write_all(STANDARD_ERROR, b"app-window: ");
    for part in parts {
        write_all(STANDARD_ERROR, part);
    }
    write_all(STANDARD_ERROR, b"\n");
}

fn first_screen(installer: &mut Installer, command: Command<'_>) {
    match command {
        Command::Install { path, .. } => {
            let folder = Path::from(path).unwrap_or_default();
            let is_plain_folder = !installer::is_bundle_name(files::file_name(path))
                && installer
                    .files
                    .status_following(path)
                    .is_ok_and(|status| status.kind == Kind::Directory);
            if !is_plain_folder {
                installer.prompt_for(path);
                return;
            }
            let (names, count, more) = programs_in(&mut installer.files, &folder);
            let mut shown = [""; dialog::MAX_CHOICES];
            for (slot, name) in shown.iter_mut().zip(&names[..count]) {
                *slot = name.as_str();
            }
            let mut place = Line::new();
            place.push_bytes(folder.as_bytes());
            let dialog = Dialog::choose(place.as_str(), &shown[..count], more);
            installer.show(dialog, Stage::Choosing { folder, names });
        }
        Command::Uninstall { name, .. } => {
            let shown = Name::new(name);
            let mut file = [0u8; manifest::LARGEST + 1];
            let source = installer::read_manifest(&mut installer.files, name, &mut file)
                .and_then(|length| manifest::parse(&file[..length], name))
                .map(|app| app.source);
            let icon = AppIcon::of(shown.as_str(), source);
            match source {
                Some(_) => {
                    let dialog = Dialog::confirm_uninstall(icon, shown.as_str());
                    installer.show(dialog, Stage::Confirming(shown));
                }
                None => {
                    let line = messages::uninstall_failed(
                        shown.as_str(),
                        installer::Problem::NotInstalled,
                    );
                    installer.show(Dialog::message(icon, line.as_str()), Stage::Showing);
                }
            }
        }
        Command::Open { .. } | Command::List { .. } | Command::Help => {}
    }
}

fn title(command: &Command<'_>) -> &'static str {
    match command {
        Command::Uninstall { .. } => dialog::UNINSTALL_TITLE,
        _ => dialog::INSTALL_TITLE,
    }
}

/// The program: see the module documentation.
pub fn run(startup: &Startup) -> i32 {
    let mut arguments: [&[u8]; MAX_ARGUMENTS] = [b""; MAX_ARGUMENTS];
    let mut count = 0;
    for argument in startup.arguments().skip(1) {
        if count == MAX_ARGUMENTS {
            write_all(STANDARD_ERROR, command::USAGE.as_bytes());
            return command::USAGE_ERROR;
        }
        arguments[count] = argument.to_bytes();
        count += 1;
    }
    let command = match command::parse(&arguments[..count]) {
        Some(command @ (Command::Install { .. } | Command::Uninstall { .. })) => command,
        _ => {
            write_all(STANDARD_ERROR, command::USAGE.as_bytes());
            return command::USAGE_ERROR;
        }
    };
    let (name, entry) = match command {
        Command::Install { name, entry, .. } => (name.map(Name::new), entry.map(entry_as_given)),
        _ => (None, None),
    };
    let (width, height) = SIZE;
    let mut window = match Window::create(title(&command), width, height, SIZE) {
        Ok(window) => window,
        Err(error) => {
            complain(&[b"cannot open its window: ", error.message()]);
            return command::FAILED;
        }
    };
    // SAFETY: this program has one thread and takes the buffer once, here, so no other
    // reference to it exists while the installer holds it.
    let buffer =
        unsafe { core::slice::from_raw_parts_mut((&raw mut BUFFER).cast::<u8>(), COPY_BUFFER) };
    let mut installer = Installer {
        dialog: Dialog::message(AppIcon::of("?", None), ""),
        stage: Stage::Showing,
        files: LinuxFiles::new(),
        machine: linux::machine(),
        name,
        entry,
        buffer,
    };
    first_screen(&mut installer, command);
    match application::run(&mut window, &mut installer) {
        Ok(()) => {
            if let Stage::Installing(installation) = &mut installer.stage {
                installation.cancel(&mut installer.files);
            }
            0
        }
        Err(error) => {
            complain(&[b"its window: ", error.message()]);
            command::FAILED
        }
    }
}
