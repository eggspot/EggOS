//! The `component` command (D235): `component list [--json]`, `component install [--dry-run]
//! [--json] <name>` and `component remove [--yes] [--dry-run] [--json] <name>`, over any
//! [`Files`] and [`Console`], so the host tests run it whole.

use crate::index::Component;
use crate::manager::{self, Refusal, RemoveError, Store};
use crate::messages;
use crate::{firmware, index};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use eggos_app::files::Files;
use eggos_commands::{ErrorDefinition, errors};
use eggos_mcp::json::JsonWriter;

/// The command's usage.
pub const USAGE: &str = "usage: component list [--json]\n       component install [--dry-run] \
                         [--json] <name>\n       component remove [--yes] [--dry-run] [--json] \
                         <name>\n       component edition [--dry-run] [<name>]\n";
/// The exit status of a command line written wrongly.
pub const USAGE_STATUS: i32 = 2;
/// The exit status of a refusal or a failure.
pub const FAILED: i32 = 1;

const RECORD_BYTES: usize = 4096;
const ANSWER_BYTES: usize = 16;

/// The command's standard input, output and error.
pub trait Console {
    /// Writes to standard output.
    fn write_output(&mut self, bytes: &[u8]);
    /// Writes to standard error.
    fn write_error(&mut self, bytes: &[u8]);
    /// Whether a person is at a terminal to answer a question.
    fn is_terminal(&mut self) -> bool;
    /// Reads the person's answer into `buffer`; how many bytes.
    fn read_answer(&mut self, buffer: &mut [u8]) -> usize;
}

/// What the command runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context<'a> {
    /// The machine's architecture word, `x86_64`.
    pub arch: &'a str,
    /// The command's process id, kept in the lock.
    pub pid: u32,
    /// Whether every error also writes its coded JSON line (the MCP server's twins).
    pub coded: bool,
    /// Downloads `url` into the file `path`, when the system can (D257): the release's store
    /// when no store is attached.
    pub download: Option<fn(&str, &str) -> bool>,
}

/// A parsed command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    /// `component list`.
    List { json: bool },
    /// `component install`.
    Install {
        name: &'a str,
        dry_run: bool,
        json: bool,
    },
    /// `component edition`: the editions, or the components of `name` installed (D258).
    Edition { name: Option<&'a str>, dry_run: bool },
    /// `component remove`.
    Remove {
        name: &'a str,
        yes: bool,
        dry_run: bool,
        json: bool,
    },
}

/// Parses the words after `component`.
pub fn parse<'a>(arguments: &[&'a str]) -> Option<Command<'a>> {
    let (&verb, rest) = arguments.split_first()?;
    let (mut json, mut dry_run, mut yes, mut name) = (false, false, false, None);
    let mut operands_only = false;
    for &word in rest {
        match word {
            "--json" if !operands_only => json = true,
            "--dry-run" if !operands_only && verb != "list" => dry_run = true,
            "--yes" if !operands_only && verb == "remove" => yes = true,
            "--" if !operands_only => operands_only = true,
            _ if word.starts_with('-') && !operands_only => return None,
            _ if name.is_none() && verb != "list" => name = Some(word),
            _ if verb == "edition" => return None,
            _ => return None,
        }
    }
    match verb {
        "list" => Some(Command::List { json }),
        "edition" if !json => Some(Command::Edition { name, dry_run }),
        "install" => Some(Command::Install {
            name: name?,
            dry_run,
            json,
        }),
        "remove" => Some(Command::Remove {
            name: name?,
            yes,
            dry_run,
            json,
        }),
        _ => None,
    }
}

/// Runs `component` with `arguments` (the words after it); the exit status.
pub fn run<F: Files, C: Console>(
    files: &mut F,
    console: &mut C,
    context: Context<'_>,
    arguments: &[&str],
) -> i32 {
    let Some(command) = parse(arguments) else {
        console.write_error(USAGE.as_bytes());
        return USAGE_STATUS;
    };
    let mut store = manager::find_store(files, context.arch);
    let wants_store = matches!(
        command,
        Command::Install { dry_run: false, .. } | Command::Edition { name: Some(_), dry_run: false }
    );
    if store.is_none() && wants_store && let Some(download) = context.download {
        let path = manager::downloaded_store_path(manager::VERSION, context.arch);
        if download(&manager::store_url(manager::VERSION, context.arch), &path) {
            store = manager::read_store(files, &path, manager::Source::Downloaded, context.arch).ok();
        }
    }
    if let (Command::Install { name, dry_run: false, .. }, Some(download)) = (command, context.download)
        && store.as_ref().is_none_or(|store| store.index.component(name).is_none())
        && let Some(component) = firmware::find(name).filter(|component| component.arches.contains(&context.arch))
    {
        store = upstream_store(files, component, context.arch, download).or(store);
    }
    let mut out = Out {
        console,
        coded: context.coded,
        file_name: manager::store_file_name(manager::VERSION, context.arch),
    };
    match command {
        Command::List { json } => list(files, &mut out, store.as_ref(), json),
        Command::Edition { name, dry_run } => edition(files, &mut out, store.as_ref(), context.pid, name, dry_run),
        Command::Install {
            name,
            dry_run,
            json,
        } => install(
            files,
            &mut out,
            store.as_ref(),
            context.pid,
            name,
            dry_run,
            json,
        ),
        Command::Remove {
            name,
            yes,
            dry_run,
            json,
        } => remove(
            files,
            &mut out,
            store.as_ref(),
            context.pid,
            name,
            yes,
            dry_run,
            json,
        ),
    }
}

fn read_whole<F: Files>(files: &mut F, path: &str) -> Option<Vec<u8>> {
    let length = files.status_following(path.as_bytes()).ok()?.size;
    let handle = files.open(path.as_bytes()).ok()?;
    let mut bytes = vec![0u8; usize::try_from(length).ok()?];
    let mut done = 0;
    let mut read = Ok(());
    while done < bytes.len() && read.is_ok() {
        read = match files.read_at(handle, done as u64, &mut bytes[done..]) {
            Ok(0) | Err(_) => Err(()),
            Ok(count) => {
                done += count;
                Ok(())
            }
        };
    }
    files.close(handle);
    read.ok().map(|()| bytes)
}

fn write_whole<F: Files>(files: &mut F, path: &str, bytes: &[u8]) -> Option<()> {
    let handle = files.create(path.as_bytes(), 0o644).ok()?;
    let written = files.write(handle, bytes).and_then(|()| files.sync_file(handle));
    files.close(handle);
    written.ok()
}

fn cached_name(file: &firmware::Pinned) -> String {
    let mut path = String::from(manager::DOWNLOAD_FOLDER);
    path.push_str("/linux-firmware-");
    path.push_str(firmware::LINUX_FIRMWARE_TAG);
    path.push('-');
    path.push_str(&file.path.replace('/', "-"));
    path
}

fn upstream_store<F: Files>(
    files: &mut F,
    component: &firmware::FirmwareComponent,
    arch: &str,
    download: fn(&str, &str) -> bool,
) -> Option<Store> {
    let gathered = firmware::gather(component, &mut |file| {
        let path = cached_name(file);
        let cached = read_whole(files, &path).filter(|bytes| firmware::matches_pin(file, bytes));
        cached.or_else(|| {
            download(&firmware::upstream_url(file), &path).then_some(())?;
            read_whole(files, &path)
        })
    })
    .ok()?;
    let packed = index::Packed {
        component: firmware::record(component, &gathered),
        contents: gathered.into_iter().map(|(_, bytes)| bytes).collect(),
    };
    let image = index::store_image(&[packed], manager::VERSION, arch);
    let path = manager::upstream_store_path(manager::VERSION, arch, component.name);
    write_whole(files, &path, &image)?;
    manager::read_store(files, &path, manager::Source::Upstream, arch).ok()
}

struct Out<'c, C: Console> {
    console: &'c mut C,
    coded: bool,
    file_name: String,
}

impl<C: Console> Out<'_, C> {
    fn say(&mut self, line: &str) {
        self.console.write_output(line.as_bytes());
        self.console.write_output(b"\n");
    }

    fn fail(&mut self, json: bool, error: &ErrorDefinition, line: &str) -> i32 {
        if !json {
            self.console.write_error(line.as_bytes());
            self.console.write_error(b"\n");
            if let Some(command) = error.fix_command {
                self.console.write_error(b"  try: ");
                self.console.write_error(command.as_bytes());
                self.console.write_error(b"\n");
            }
        }
        if json || self.coded {
            let mut record = [0u8; RECORD_BYTES];
            let mut writer = JsonWriter::new(&mut record);
            eggos_mcp::server::write_error(&mut writer, error, line);
            writer.end_line();
            self.console.write_error(writer.as_bytes());
        }
        FAILED
    }

    fn refuse(&mut self, json: bool, refusal: &Refusal) -> i32 {
        let (line, error) = messages::refusal(refusal, &self.file_name);
        self.fail(json, error, &line)
    }

    fn json(&mut self, write: impl FnOnce(&mut JsonWriter<'_>)) {
        let mut record = [0u8; RECORD_BYTES];
        let mut writer = JsonWriter::new(&mut record);
        write(&mut writer);
        writer.end_line();
        self.console.write_output(writer.as_bytes());
    }
}

/// One row of `component list`, of `component_list` and of the Components list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The component.
    pub component: Component,
    /// `installed`, `not-installed`, `installing` or `removing`.
    pub state: &'static str,
    /// The titles of the installed components that need it.
    pub needed_by: Vec<String>,
}

/// The rows to show: the store's components in its order, then installed ones it lacks.
pub fn rows<F: Files>(files: &mut F, store: Option<&Store>) -> Vec<Row> {
    let installed = manager::installed(files);
    let busy = busy_with(files);
    let state = |component: &Component, is_installed: bool| match &busy {
        Some((title, true)) if *title == component.title => "removing",
        Some((title, false)) if *title == component.title => "installing",
        _ if is_installed => "installed",
        _ => "not-installed",
    };
    let needed_by = |name: &str| -> Vec<String> {
        installed
            .iter()
            .filter(|other| other.needs(name))
            .map(|other| other.title.clone())
            .collect()
    };
    let mut rows = Vec::new();
    if let Some(store) = store {
        for component in &store.index.components {
            let is_installed = installed.iter().any(|known| known.name == component.name);
            rows.push(Row {
                state: state(component, is_installed),
                needed_by: needed_by(&component.name),
                component: component.clone(),
            });
        }
    }
    for component in &installed {
        if rows.iter().all(|row| row.component.name != component.name) {
            rows.push(Row {
                state: state(component, true),
                needed_by: needed_by(&component.name),
                component: component.clone(),
            });
        }
    }
    rows
}

fn busy_with<F: Files>(files: &mut F) -> Option<(String, bool)> {
    let (pid, title, removing) = manager::lock_holder(files)?;
    let mut proc = String::from("/proc/");
    proc.push_str(&pid.to_string());
    files.status(proc.as_bytes()).ok()?;
    Some((title, removing))
}

/// Writes one row as `component list --json`'s record.
pub fn write_row(writer: &mut JsonWriter<'_>, row: &Row) {
    let component = &row.component;
    writer.begin_object();
    writer.key("name");
    writer.string(&component.name);
    writer.key("title");
    writer.string(&component.title);
    writer.key("summary");
    writer.string(&component.summary);
    writer.key("state");
    writer.string(row.state);
    writer.key("percent");
    writer.null();
    writer.key("size_bytes");
    writer.u64(component.size_bytes());
    writer.key("size_text");
    writer.string(&messages::size_text(component.size_bytes()));
    writer.key("needs");
    writer.begin_array();
    for need in &component.needs {
        writer.string(need);
    }
    writer.end_array();
    writer.key("needed_by");
    writer.begin_array();
    for title in &row.needed_by {
        writer.string(title);
    }
    writer.end_array();
    writer.key("file");
    writer.string(&format!("{}.component", component.name));
    writer.end_object();
}

fn write_store(writer: &mut JsonWriter<'_>, store: Option<&Store>) {
    writer.begin_object();
    writer.key("store");
    match store {
        None => writer.null(),
        Some(store) => {
            writer.begin_object();
            writer.key("file");
            writer.string(&store.file_name());
            writer.key("version");
            writer.string(&store.index.version);
            writer.key("arch");
            writer.string(&store.index.arch);
            writer.key("source");
            match store.source {
                manager::Source::Disk(disk) => {
                    writer.string("disk");
                    writer.key("device");
                    writer.string(disk);
                }
                manager::Source::Share => writer.string("share"),
                manager::Source::Downloaded => writer.string("downloaded"),
                manager::Source::Upstream => writer.string("linux-firmware"),
            }
            writer.end_object();
        }
    }
    writer.end_object();
}

fn list<F: Files, C: Console>(
    files: &mut F,
    out: &mut Out<'_, C>,
    store: Option<&Store>,
    json: bool,
) -> i32 {
    let rows = rows(files, store);
    if json {
        let mut record = [0u8; RECORD_BYTES];
        let mut writer = JsonWriter::new(&mut record);
        write_store(&mut writer, store);
        writer.end_line();
        out.console.write_error(writer.as_bytes());
        for row in &rows {
            out.json(|writer| write_row(writer, row));
        }
        return 0;
    }
    let line = messages::store_line(store, &out.file_name);
    out.console.write_error(line.as_bytes());
    out.console.write_error(b"\n");
    let width = |pick: &dyn Fn(&Row) -> usize, header: usize| {
        rows.iter().map(pick).fold(header, usize::max)
    };
    let name_width = width(&|row| row.component.name.len(), 4);
    let state_width = width(&|row| row.state.len(), 5);
    let size_width = width(
        &|row| messages::size_text(row.component.size_bytes()).len(),
        4,
    );
    out.say(&format!(
        "{:name_width$}  {:state_width$}  {:size_width$}  TITLE",
        "NAME", "STATE", "SIZE"
    ));
    for row in &rows {
        out.say(&format!(
            "{:name_width$}  {:state_width$}  {:size_width$}  {}",
            row.component.name,
            row.state,
            messages::size_text(row.component.size_bytes()),
            row.component.title
        ));
    }
    0
}

#[allow(clippy::too_many_arguments)]
fn edition<F: Files, C: Console>(
    files: &mut F,
    out: &mut Out<'_, C>,
    store: Option<&Store>,
    pid: u32,
    name: Option<&str>,
    dry_run: bool,
) -> i32 {
    let installed = manager::installed(files);
    let names = installed.iter().map(|component| component.name.as_str());
    let recorded = recorded_edition(files);
    let Some(name) = name else {
        let current = crate::editions::current_preferring(names, recorded.as_deref());
        for edition in crate::editions::EDITIONS {
            let mark = if current.is_some_and(|current| current.name == edition.name) { "*" } else { " " };
            out.say(&alloc::format!("{mark} {}\t{}", edition.name, edition.summary));
        }
        return 0;
    };
    let Some(edition) = crate::editions::edition(name) else {
        out.say(&alloc::format!("No edition named \"{name}\": component edition names them."));
        return FAILED;
    };
    let missing: alloc::vec::Vec<&str> = edition
        .components
        .iter()
        .copied()
        .filter(|wanted| !installed.iter().any(|component| component.name == *wanted))
        .filter(|wanted| store.is_none_or(|store| store.index.components.iter().any(|offered| offered.name == *wanted)))
        .collect();
    if missing.is_empty() {
        out.say(&alloc::format!("{} is installed: nothing to add.", edition.title));
        return 0;
    }
    if dry_run {
        out.say(&alloc::format!("Would install for {}: {}", edition.title, missing.join(", ")));
        return 0;
    }
    for component in missing {
        let status = install(files, out, store, pid, component, false, false);
        if status != 0 {
            return status;
        }
    }
    record_edition(files, edition.name);
    0
}

/// Records `name` as the edition chosen (D263), best effort.
pub fn record_edition<F: Files>(files: &mut F, name: &str) {
    if let Ok(handle) = files.create(crate::editions::RECORD.as_bytes(), 0o644) {
        let _ = files.write(handle, name.as_bytes());
        files.close(handle);
    }
}

/// The edition recorded as chosen, if any.
pub fn recorded_edition<F: Files>(files: &mut F) -> Option<String> {
    let handle = files.open(crate::editions::RECORD.as_bytes()).ok()?;
    let mut bytes = [0u8; 32];
    let read = files.read(handle, &mut bytes).unwrap_or(0);
    files.close(handle);
    core::str::from_utf8(&bytes[..read]).ok().map(|text| String::from(text.trim()))
}

fn install<F: Files, C: Console>(
    files: &mut F,
    out: &mut Out<'_, C>,
    store: Option<&Store>,
    pid: u32,
    name: &str,
    dry_run: bool,
    json: bool,
) -> i32 {
    let installed = manager::installed(files);
    if let Some(component) = installed.iter().find(|component| component.name == name) {
        let line = messages::already_installed(&component.title);
        match json {
            true => out.json(|writer| write_installed(writer, &[], 0)),
            false => out.say(&line),
        }
        return 0;
    }
    let Some(store) = store else {
        return out.refuse(json, &Refusal::NoStore);
    };
    let plan = match manager::plan_install(&store.index, &installed, name) {
        Ok(plan) => plan,
        Err(refusal) => return out.refuse(json, &refusal),
    };
    let total: u64 = plan.iter().map(|component| component.size_bytes()).sum();
    if dry_run {
        match json {
            true => out.json(|writer| {
                writer.begin_object();
                writer.key("would_install");
                writer.begin_array();
                for component in &plan {
                    writer.string(&component.name);
                }
                writer.end_array();
                writer.key("size_bytes");
                writer.u64(total);
                writer.end_object();
            }),
            false => out.say(&messages::would_install(&plan)),
        }
        return 0;
    }
    let Some(wanted) = plan.last() else {
        return 0;
    };
    if let Err(refusal) = manager::lock(files, &wanted.title, false, pid) {
        return out.refuse(json, &refusal);
    }
    if let (false, Some(also)) = (json, messages::also_installs(&plan)) {
        out.say(&format!("{also} {}", messages::in_all(&plan)));
    }
    let terminal = !json && out.console.is_terminal();
    let mut done: Vec<&str> = Vec::new();
    for component in &plan {
        if !json {
            out.console
                .write_output(messages::installing(component).as_bytes());
        }
        let mut shown = u64::MAX;
        let installing = messages::installing(component);
        let installed = manager::install(files, store, component, &mut |copied, total| {
            let percent = copied.saturating_mul(100) / total.max(1);
            if terminal && percent != shown {
                shown = percent;
                let line = format!(
                    "\r{installing} {percent} %, {}",
                    messages::size_text(copied)
                );
                out.console.write_output(line.as_bytes());
            }
            true
        });
        if !json {
            out.console.write_output(b"\n");
        }
        if let Err(failure) = installed {
            manager::unlock(files);
            let line = messages::install_failure_line(&failure, component);
            let (_, _, error) = messages::install_failure(&failure, component);
            return out.fail(json, error, &line);
        }
        done.push(&component.name);
        if !json {
            out.say(&messages::installed(component));
        }
    }
    manager::unlock(files);
    if json {
        out.json(|writer| write_installed(writer, &done, total));
    }
    0
}

fn write_installed(writer: &mut JsonWriter<'_>, names: &[&str], total: u64) {
    writer.begin_object();
    writer.key("installed");
    writer.begin_array();
    for name in names {
        writer.string(name);
    }
    writer.end_array();
    writer.key("size_bytes");
    writer.u64(total);
    writer.end_object();
}

#[allow(clippy::too_many_arguments)]
fn remove<F: Files, C: Console>(
    files: &mut F,
    out: &mut Out<'_, C>,
    store: Option<&Store>,
    pid: u32,
    name: &str,
    yes: bool,
    dry_run: bool,
    json: bool,
) -> i32 {
    let installed = manager::installed(files);
    let component = match manager::check_remove(files, &installed, name) {
        Ok(Some(component)) => component,
        Ok(None) => {
            let Some(known) = store.and_then(|store| store.index.component(name)) else {
                return out.refuse(json, &Refusal::NoSuchComponent(name.into()));
            };
            match json {
                true => out.json(|writer| write_removed(writer, None, 0)),
                false => out.say(&messages::not_installed(&known.title)),
            }
            return 0;
        }
        Err(refusal) => return out.refuse(json, &refusal),
    };
    if dry_run {
        match json {
            true => out.json(|writer| {
                writer.begin_object();
                writer.key("would_remove");
                writer.string(&component.name);
                writer.key("freed_bytes");
                writer.u64(component.size_bytes());
                writer.end_object();
            }),
            false => out.say(&messages::would_remove(&component)),
        }
        return 0;
    }
    if !yes {
        if json || !out.console.is_terminal() {
            return out.fail(
                json,
                &errors::NEEDS_CONFIRMATION,
                &messages::needs_yes(&component),
            );
        }
        let question = format!(
            "{} {} [y/N] ",
            messages::remove_question(&component),
            component.on_remove
        );
        out.console.write_error(question.as_bytes());
        let mut answer = [0u8; ANSWER_BYTES];
        let length = out.console.read_answer(&mut answer);
        let answer = core::str::from_utf8(&answer[..length]).unwrap_or("").trim();
        if !matches!(answer, "y" | "Y" | "yes" | "Yes" | "YES") {
            return out.fail(json, &errors::DECLINED_BY_USER, messages::DECLINED);
        }
    }
    let component = match manager::remove_checked(files, &component.title, &component.name, pid) {
        Ok(Some(removed)) => removed,
        Ok(None) => {
            match json {
                true => out.json(|writer| write_removed(writer, None, 0)),
                false => out.say(&messages::not_installed(&component.title)),
            }
            return 0;
        }
        Err(RemoveError::Refused(refusal)) => return out.refuse(json, &refusal),
        Err(RemoveError::Failed(failure)) => {
            return out.fail(
                json,
                &errors::REMOVE_FAILED,
                &messages::remove_failure_line(&failure, &component),
            );
        }
    };
    match json {
        true => {
            out.json(|writer| write_removed(writer, Some(&component.name), component.size_bytes()))
        }
        false => out.say(&messages::removed(&component)),
    }
    0
}

fn write_removed(writer: &mut JsonWriter<'_>, name: Option<&str>, freed: u64) {
    writer.begin_object();
    writer.key("removed");
    match name {
        Some(name) => writer.string(name),
        None => writer.null(),
    }
    writer.key("freed_bytes");
    writer.u64(freed);
    writer.end_object();
}
