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
pub const USAGE: &str = "usage: component list [--json] [--download]\n       component cancel\n       component install [--dry-run] \
                         [--json] <name>\n       component remove [--yes] [--dry-run] [--json] \
                         <name>\n       component edition [--dry-run] [<name>]\n       \
                         component update [--dry-run] [--json] [<name>]\n       \
                         component update --daily on|off\n";
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
    /// Downloads bytes `first` to `last` (both included) of `url` into the file `path`, when the
    /// system can: the store's index and the files of the component asked for (D271).
    pub download_range: Option<fn(&str, &str, u64, u64) -> bool>,
    /// The wall clock's seconds, for the stored update check (D299).
    pub now_s: u64,
}

/// A parsed command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    /// `component list`, or with `download` the release's store fetched first (D299).
    List { json: bool, download: bool },
    /// `component cancel`: asks the running job to stop (D299).
    Cancel,
    /// `component install`.
    Install {
        name: &'a str,
        dry_run: bool,
        json: bool,
    },
    /// `component edition`: the editions, or the components of `name` installed (D258).
    Edition { name: Option<&'a str>, dry_run: bool },
    /// `component update`: the installed components, or `name`, brought to the store's copy,
    /// only their changed files fetched (D293).
    Update {
        name: Option<&'a str>,
        dry_run: bool,
        json: bool,
        /// `--daily on|off`: turns the daily check on or off instead (D299).
        daily: Option<bool>,
    },
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
    let (mut download, mut daily) = (false, None);
    let mut operands_only = false;
    let mut words = rest.iter();
    while let Some(&word) = words.next() {
        match word {
            "--download" if !operands_only && verb == "list" => download = true,
            "--daily" if !operands_only && verb == "update" => {
                daily = Some(match *words.next()? {
                    "on" => true,
                    "off" => false,
                    _ => return None,
                });
            }
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
        "list" => Some(Command::List { json, download }),
        "cancel" if name.is_none() => Some(Command::Cancel),
        "update" => Some(Command::Update { name, dry_run, json, daily }),
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
    if let (None, Command::Install { name, dry_run: false, .. }, Some(download_range)) =
        (&store, command, context.download_range)
    {
        store = partial_store(files, context.arch, name, download_range);
    }
    if let (None, Command::Update { name, .. }, Some(download_range)) = (&store, command, context.download_range) {
        store = update_store(files, context.arch, name, download_range);
    }
    let wants_store = wants_store || matches!(command, Command::List { download: true, .. });
    if store.is_none() && wants_store && let Some(download) = context.download {
        crate::state::record_job(
            files,
            &crate::state::Job {
                pid: context.pid,
                kind: "downloading".into(),
                name: "store".into(),
                phase: "downloading".into(),
                done: 0,
                total: 0,
            },
        );
        let path = manager::downloaded_store_path(manager::VERSION, context.arch);
        if download(&manager::store_url(manager::VERSION, context.arch), &path) {
            store = manager::read_store(files, &path, manager::Source::Downloaded, context.arch).ok();
        }
        crate::state::end_job(files);
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
        Command::List { json, .. } => list(files, &mut out, store.as_ref(), json),
        Command::Cancel => cancel(files, &mut out),
        Command::Edition { name, dry_run } => edition(files, &mut out, store.as_ref(), context.pid, name, dry_run),
        Command::Update { daily: Some(on), json, .. } => set_daily(files, &mut out, on, json),
        Command::Update { name, dry_run, json, .. } => {
            update(files, &mut out, store.as_ref(), context, name, dry_run, json)
        }
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

fn partial_store<F: Files>(
    files: &mut F,
    arch: &str,
    name: &str,
    download_range: fn(&str, &str, u64, u64) -> bool,
) -> Option<Store> {
    let url = manager::store_url(manager::VERSION, arch);
    let head = manager::partial_piece_path(manager::VERSION, arch, "index");
    let index_end = index::SECTOR + u64::from(index::MAX_INDEX_BYTES) - 1;
    if !download_range(&url, &head, 0, index_end) {
        return None;
    }
    let bytes = read_whole(files, &head)?;
    let header = index::Header::parse(&bytes).ok()?;
    let text = bytes.get(index::SECTOR as usize..)?.get(..header.index_bytes as usize)?;
    let whole = index::parse_index(&header, text, u64::MAX).ok()?;
    if whole.arch != arch {
        return None;
    }
    let installed = manager::installed(files);
    let plan = manager::plan_install(&whole, &installed, name).ok()?;
    let mut packed = Vec::new();
    for component in plan {
        let mut contents = Vec::new();
        for (number, file) in component.files.iter().enumerate() {
            if file.length == 0 {
                contents.push(Vec::new());
                continue;
            }
            let piece = manager::partial_piece_path(manager::VERSION, arch, &format!("{}-{number}", component.name));
            let last = file.offset + file.length - 1;
            if !download_range(&url, &piece, file.offset, last) {
                return None;
            }
            let bytes = read_whole(files, &piece)?;
            if bytes.len() as u64 != file.length || index::sha256(&bytes) != file.sha256 {
                return None;
            }
            contents.push(bytes);
        }
        packed.push(index::Packed {
            component: component.clone(),
            contents,
        });
    }
    let image = index::store_image(&packed, &whole.version, arch);
    let path = manager::partial_store_path(manager::VERSION, arch, name);
    write_whole(files, &path, &image)?;
    manager::read_store(files, &path, manager::Source::Downloaded, arch).ok()
}

/// A store holding only what updating the installed components (or `only`) needs (D293): the
/// release's index, then the changed files' bytes, each fetched with a range request and
/// checked against its digest, packed with the index's records. `None` when the index cannot be
/// fetched or a file does not match.
fn update_store<F: Files>(
    files: &mut F,
    arch: &str,
    only: Option<&str>,
    download_range: fn(&str, &str, u64, u64) -> bool,
) -> Option<Store> {
    let url = manager::store_url(manager::VERSION, arch);
    let head = manager::partial_piece_path(manager::VERSION, arch, "index");
    let index_end = index::SECTOR + u64::from(index::MAX_INDEX_BYTES) - 1;
    if !download_range(&url, &head, 0, index_end) {
        return None;
    }
    let bytes = read_whole(files, &head)?;
    let header = index::Header::parse(&bytes).ok()?;
    let text = bytes.get(index::SECTOR as usize..)?.get(..header.index_bytes as usize)?;
    let whole = index::parse_index(&header, text, u64::MAX).ok()?;
    if whole.arch != arch {
        return None;
    }
    let installed = manager::installed(files);
    let mut packed = Vec::new();
    for old in installed.iter().filter(|old| only.is_none_or(|name| name == old.name)) {
        let Some(available) = whole.component(&old.name) else {
            continue;
        };
        let Some(plan) = manager::plan_update(old, available) else {
            continue;
        };
        let mut contents = Vec::new();
        for (number, file) in available.files.iter().enumerate() {
            let changed = plan.changed.iter().any(|changed| core::ptr::eq(*changed, file));
            if !changed || file.length == 0 {
                contents.push(Vec::new());
                continue;
            }
            let piece = manager::partial_piece_path(manager::VERSION, arch, &format!("{}-update-{number}", available.name));
            if !download_range(&url, &piece, file.offset, file.offset + file.length - 1) {
                return None;
            }
            let bytes = read_whole(files, &piece)?;
            if bytes.len() as u64 != file.length || index::sha256(&bytes) != file.sha256 {
                return None;
            }
            contents.push(bytes);
        }
        packed.push(index::Packed {
            component: available.clone(),
            contents,
        });
    }
    let image = index::store_image(&packed, &whole.version, arch);
    let path = manager::partial_store_path(manager::VERSION, arch, "update");
    write_whole(files, &path, &image)?;
    manager::read_store(files, &path, manager::Source::Downloaded, arch).ok()
}

fn set_daily<F: Files, C: Console>(files: &mut F, out: &mut Out<'_, C>, on: bool, json: bool) -> i32 {
    if !crate::state::set_daily(files, on) {
        return out.fail(json, &eggos_commands::errors::FAILED, "could not save the daily check's setting");
    }
    match json {
        true => out.json(|writer| {
            writer.begin_object();
            writer.key("updates_daily");
            writer.bool(on);
            writer.end_object();
        }),
        false => out.say(if on {
            "Components are checked for updates every day."
        } else {
            "Components are no longer checked for updates every day."
        }),
    }
    0
}

fn cancel<F: Files, C: Console>(files: &mut F, out: &mut Out<'_, C>) -> i32 {
    match crate::state::job(files) {
        Some(job) => {
            crate::state::request_cancel(files);
            out.say(&format!("Cancelling {} {}...", job.kind, job.name));
            0
        }
        None => {
            out.say("Nothing is running.");
            0
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn update<F: Files, C: Console>(
    files: &mut F,
    out: &mut Out<'_, C>,
    store: Option<&Store>,
    context: Context<'_>,
    only: Option<&str>,
    dry_run: bool,
    json: bool,
) -> i32 {
    let pid = context.pid;
    let installed = manager::installed(files);
    if let Some(name) = only
        && !installed.iter().any(|component| component.name == name)
    {
        return out.refuse(json, &Refusal::NoSuchComponent(name.into()));
    }
    let Some(store) = store else {
        return out.refuse(json, &Refusal::NoStore);
    };
    let plans: Vec<manager::UpdatePlan<'_>> = installed
        .iter()
        .filter(|old| only.is_none_or(|name| name == old.name))
        .filter_map(|old| manager::plan_update(old, store.index.component(&old.name)?))
        .collect();
    let fetched: u64 = plans.iter().map(manager::UpdatePlan::changed_bytes).sum();
    if dry_run && only.is_none() {
        let source = match store.source {
            manager::Source::Disk(_) | manager::Source::Share => "disk",
            _ => "release",
        };
        crate::state::store_check(files, &crate::state::Checked::of(&plans, context.now_s, source));
    }
    let write_summary = |writer: &mut JsonWriter<'_>, key: &str| {
        writer.begin_object();
        writer.key(key);
        writer.begin_array();
        for plan in &plans {
            writer.string(&plan.component.name);
        }
        writer.end_array();
        writer.key("fetched_bytes");
        writer.u64(fetched);
        writer.end_object();
    };
    if plans.is_empty() {
        match json {
            true => out.json(|writer| write_summary(writer, "updated")),
            false => out.say("Every installed component is up to date."),
        }
        return 0;
    }
    if dry_run {
        match json {
            true => out.json(|writer| write_summary(writer, "would_update")),
            false => {
                for plan in &plans {
                    out.say(&format!(
                        "would update {}: {} changed, {} kept, {} gone, {} to fetch",
                        messages::titled(plan.component),
                        plan.changed.len(),
                        plan.kept,
                        plan.dropped.len(),
                        messages::size_text(plan.changed_bytes())
                    ));
                }
            }
        }
        return 0;
    }
    if let Err(refusal) = manager::lock(files, "update", false, pid) {
        return out.refuse(json, &refusal);
    }
    crate::state::clear_failure(files);
    for plan in &plans {
        if !json {
            out.say(&format!("Updating {}...", messages::titled(plan.component)));
        }
        let name = plan.component.name.clone();
        let updated = manager::update(files, Some(store), plan, &mut |files, done, total| {
            if crate::state::cancel_requested(files) {
                return false;
            }
            crate::state::record_job(
                files,
                &crate::state::Job {
                    pid,
                    kind: "updating".into(),
                    name: name.clone(),
                    phase: "copying".into(),
                    done,
                    total,
                },
            );
            true
        });
        crate::state::end_job(files);
        if let Err(failure) = updated {
            crate::state::record_failure(files, &name, &failure);
            manager::unlock(files);
            let line = messages::install_failure_line(&failure, plan.component);
            let (_, _, error) = messages::install_failure(&failure, plan.component);
            return out.fail(json, error, &line);
        }
    }
    manager::unlock(files);
    match json {
        true => out.json(|writer| write_summary(writer, "updated")),
        false => out.say(&format!(
            "Updated {} component{}, {} fetched.",
            plans.len(),
            if plans.len() == 1 { "" } else { "s" },
            messages::size_text(fetched)
        )),
    }
    0
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
    /// The update the last check found for it, if it is installed and one was found (D299).
    pub update: Option<crate::state::CheckedItem>,
    /// The running job about it, if any (D299).
    pub job: Option<crate::state::Job>,
}

/// The rows to show: the store's components in its order, then installed ones it lacks.
pub fn rows<F: Files>(files: &mut F, store: Option<&Store>) -> Vec<Row> {
    let installed = manager::installed(files);
    let busy = busy_with(files);
    let checked = crate::state::last_check(files);
    let job = crate::state::job(files);
    let job_of = |name: &str| job.clone().filter(|job| job.name == name);
    let update_of = |name: &str, is_installed: bool| {
        checked.as_ref().filter(|_| is_installed).and_then(|checked| checked.item(name).cloned())
    };
    let state = |component: &Component, is_installed: bool| match &busy {
        Some((title, true)) if *title == component.title => "removing",
        _ if job.as_ref().is_some_and(|job| job.name == component.name && job.kind == "updating") => "updating",
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
                update: update_of(&component.name, is_installed),
                job: job_of(&component.name),
                component: component.clone(),
            });
        }
    }
    for component in &installed {
        if rows.iter().all(|row| row.component.name != component.name) {
            rows.push(Row {
                state: state(component, true),
                needed_by: needed_by(&component.name),
                update: update_of(&component.name, true),
                job: job_of(&component.name),
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
    match &row.job {
        Some(job) => writer.u64(job.percent()),
        None => writer.null(),
    }
    writer.key("phase");
    match &row.job {
        Some(job) => writer.string(&job.phase),
        None => writer.null(),
    }
    writer.key("update");
    match &row.update {
        Some(item) => {
            writer.begin_object();
            writer.key("fetch_bytes");
            writer.u64(item.fetch_bytes);
            writer.key("changed_bytes");
            writer.u64(item.changed_bytes);
            writer.key("changed_files");
            writer.u64(item.changed_files as u64);
            writer.end_object();
        }
        None => writer.null(),
    }
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

fn write_store<F: Files>(writer: &mut JsonWriter<'_>, files: &mut F, store: Option<&Store>) {
    writer.begin_object();
    let checked = crate::state::last_check(files);
    writer.key("updates_checked");
    match &checked {
        Some(checked) => writer.u64(checked.at),
        None => writer.null(),
    }
    writer.key("updates_source");
    match &checked {
        Some(checked) => writer.string(&checked.source),
        None => writer.null(),
    }
    writer.key("updates_daily");
    writer.bool(crate::state::daily(files));
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
        write_store(&mut writer, files, store);
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
