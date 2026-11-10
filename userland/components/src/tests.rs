use crate::command::{self, Console, Context};
use crate::fake::FakeFiles;
use crate::index::{self, Component, File, Header, Index, IndexError, SECTOR};
use crate::manager::{self, COMPONENTS, Failure, Refusal, Source};
use crate::messages;

const STORE_DISK: &str = "/dev/vdc";
const WINDOWS_LAYER: &str = "usr/lib/eggos/windows";

struct Part<'a> {
    name: &'a str,
    title: &'a str,
    needs: &'a [&'a str],
    files: &'a [(&'a str, &'a [u8])],
}

const WINDOWS: Part<'static> = Part {
    name: "windows-apps",
    title: "Windows apps",
    needs: &[],
    files: &[(WINDOWS_LAYER, b"\x7fELF windows layer")],
};

const DESKTOP: Part<'static> = Part {
    name: "desktop",
    title: "Desktop",
    needs: &[],
    files: &[("usr/lib/eggos/desktop/shell", b"\x7fELF shell")],
};

const APPS: Part<'static> = Part {
    name: "builtin-apps",
    title: "Built-in apps",
    needs: &["desktop"],
    files: &[
        ("usr/lib/eggos/apps/files", b"\x7fELF files"),
        ("usr/lib/eggos/apps/editor", b"\x7fELF editor"),
    ],
};

fn component(part: &Part<'_>, offsets: &mut u64) -> Component {
    Component {
        name: part.name.into(),
        title: part.title.into(),
        summary: "Runs them natively".into(),
        on_remove: "Programs stay in Apps.".into(),
        needs: part.needs.iter().map(|need| (*need).into()).collect(),
        files: part
            .files
            .iter()
            .map(|(path, bytes)| {
                let offset = *offsets;
                *offsets += (bytes.len() as u64).div_ceil(SECTOR).max(1) * SECTOR;
                File {
                    path: (*path).into(),
                    mode: 0o755,
                    offset,
                    length: bytes.len() as u64,
                    sha256: index::sha256(bytes),
                }
            })
            .collect(),
    }
}

fn index_for(parts: &[Part<'_>], data_start: u64) -> Index {
    let mut offsets = data_start;
    Index {
        version: manager::VERSION.into(),
        arch: "x86_64".into(),
        components: parts
            .iter()
            .map(|part| component(part, &mut offsets))
            .collect(),
    }
}

fn store_image(parts: &[Part<'_>]) -> Vec<u8> {
    let mut start = SECTOR * 2;
    loop {
        let text = index_for(parts, start).write();
        let header = Header::parse(&Header::of(text.as_bytes())).unwrap();
        if header.data_start() == start {
            break;
        }
        start = header.data_start();
    }
    let index = index_for(parts, start);
    let text = index.write();
    let mut image = Header::of(text.as_bytes()).to_vec();
    image.extend_from_slice(text.as_bytes());
    for (part, component) in parts.iter().zip(&index.components) {
        for ((_, bytes), file) in part.files.iter().zip(&component.files) {
            image.resize(file.offset as usize, 0);
            image.extend_from_slice(bytes);
        }
    }
    image.resize(image.len().div_ceil(SECTOR as usize) * SECTOR as usize, 0);
    image
}

fn machine(parts: &[Part<'_>]) -> FakeFiles {
    let mut files = FakeFiles::new();
    files.folder("/var").folder("/run").folder("/proc");
    files.file(STORE_DISK, &store_image(parts));
    files
}

fn store(files: &mut FakeFiles) -> manager::Store {
    manager::find_store(files, "x86_64").unwrap()
}

fn parse(image: &[u8]) -> Result<Index, IndexError> {
    let header = Header::parse(image)?;
    let text = &image[SECTOR as usize..SECTOR as usize + header.index_bytes as usize];
    index::parse_index(&header, text, image.len() as u64)
}

fn with_index_text(text: &str, length: u64) -> Result<Index, IndexError> {
    let header = Header::parse(&Header::of(text.as_bytes())).unwrap();
    index::parse_index(&header, text.as_bytes(), length)
}

fn digest_text(bytes: &[u8]) -> String {
    index::sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn one_file_index(line: &str) -> String {
    format!(
        "eggos-components 1 0.3.0 x86_64\ncomponent windows-apps\ntitle Windows apps\nsummary \
         S\non-remove R\n{line}\nend\n"
    )
}

#[test]
fn a_store_reads_back_as_written() {
    let image = store_image(&[DESKTOP, APPS, WINDOWS]);
    let index = parse(&image).unwrap();
    assert_eq!(index.arch, "x86_64");
    let names: Vec<&str> = index.components.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["desktop", "builtin-apps", "windows-apps"]);
    assert_eq!(index.components[1].needs, ["desktop"]);
    assert_eq!(index.components[2].size_bytes(), 18);
}

#[test]
fn a_store_is_refused_for_bad_magic_format_size_or_a_changed_index() {
    let image = store_image(&[WINDOWS]);
    let mut bad = image.clone();
    bad[0] = b'X';
    assert_eq!(parse(&bad), Err(IndexError::NotAStore));
    let mut bad = image.clone();
    bad[16] = 2;
    assert_eq!(parse(&bad), Err(IndexError::UnknownFormat));
    let mut bad = image.clone();
    bad[20..24].copy_from_slice(&(index::MAX_INDEX_BYTES + 1).to_le_bytes());
    assert_eq!(parse(&bad), Err(IndexError::IndexSize));
    let mut bad = image.clone();
    bad[SECTOR as usize + 20] ^= 1;
    assert_eq!(parse(&bad), Err(IndexError::IndexDamaged));
    assert_eq!(Header::parse(&image[..100]), Err(IndexError::NotAStore));
}

#[test]
fn paths_outside_the_allowed_prefixes_are_refused() {
    for path in [
        "../etc/passwd",
        "/usr/lib/eggos/windows",
        "usr/lib/eggos/../../../etc/shadow",
        "usr/lib/eggos/./windows",
        "usr/lib/eggos//windows",
        "usr/bin/sh",
        "etc/eggos/init.conf",
        "usr/lib/eggos/",
        "usr/lib/eggos/win dows",
        "usr/lib/eggosx/windows",
    ] {
        assert!(!index::is_component_path(path), "{path}");
        let line = format!("file 755 1024 1 {} {path}", digest_text(b"x"));
        assert!(
            matches!(
                with_index_text(&one_file_index(&line), 4096),
                Err(IndexError::Line(_))
            ),
            "{path}"
        );
    }
    assert!(index::is_component_path(WINDOWS_LAYER));
    assert!(index::is_component_path(
        "usr/lib/eggos/macos.d/Foundation-1.0+x"
    ));
}

#[test]
fn a_file_line_needs_a_known_mode_an_aligned_offset_and_bytes_inside_the_store() {
    let digest = digest_text(b"x");
    let refused = |line: String| with_index_text(&one_file_index(&line), 4096);
    assert!(matches!(
        refused(format!("file 777 1024 1 {digest} {WINDOWS_LAYER}")),
        Err(IndexError::Line(_))
    ));
    assert!(matches!(
        refused(format!("file 4755 1024 1 {digest} {WINDOWS_LAYER}")),
        Err(IndexError::Line(_))
    ));
    assert!(matches!(
        refused(format!("file 755 1000 1 {digest} {WINDOWS_LAYER}")),
        Err(IndexError::Line(_))
    ));
    assert_eq!(
        refused(format!("file 755 4096 1 {digest} {WINDOWS_LAYER}")),
        Err(IndexError::OutsideStore)
    );
    assert_eq!(
        refused(format!("file 755 0 1 {digest} {WINDOWS_LAYER}")),
        Err(IndexError::OutsideStore)
    );
    assert_eq!(
        refused(format!(
            "file 755 18446744073709551104 1024 {digest} {WINDOWS_LAYER}"
        )),
        Err(IndexError::OutsideStore)
    );
    assert!(matches!(
        refused(format!("file 755 1024 1 {} {WINDOWS_LAYER}", &digest[1..])),
        Err(IndexError::Line(_))
    ));
    assert!(matches!(
        refused(format!("link 755 1024 1 {digest} {WINDOWS_LAYER}")),
        Err(IndexError::Line(_))
    ));
    assert!(refused(format!("file 755 1024 1 {digest} {WINDOWS_LAYER}")).is_ok());
}

#[test]
fn names_texts_counts_duplicates_and_needs_are_checked() {
    let digest = digest_text(b"x");
    let file = format!("file 755 1024 1 {digest} {WINDOWS_LAYER}");
    let block = |name: &str, extra: &str| {
        format!("component {name}\ntitle T\nsummary S\non-remove R\n{extra}{file}\nend\n")
    };
    let head = "eggos-components 1 0.3.0 x86_64\n";
    let text = format!("{head}{}", block("Windows", ""));
    assert!(with_index_text(&text, 4096).is_err());
    let text = format!("{head}{}{}", block("a", ""), block("a", ""));
    assert_eq!(with_index_text(&text, 4096), Err(IndexError::Duplicate));
    let text = format!("{head}{}{}", block("a", "needs b\n"), block("b", ""));
    assert_eq!(with_index_text(&text, 4096), Err(IndexError::UnknownNeed));
    let text = format!("{head}{}", block("a", "needs a\n"));
    assert!(with_index_text(&text, 4096).is_err());
    let many: String = (0..=index::MAX_COMPONENTS)
        .map(|at| block(&format!("c{at}"), ""))
        .collect();
    assert_eq!(
        with_index_text(&format!("{head}{many}"), 4096),
        Err(IndexError::TooMany)
    );
    let clash = format!("file 755 1024 1 {digest} {WINDOWS_LAYER}/inner\n");
    let text = format!("{head}{}", block("a", &clash));
    assert_eq!(with_index_text(&text, 4096), Err(IndexError::Duplicate));
    let text = format!("{head}component a\ntitle \u{1b}[2J\nsummary S\non-remove R\n{file}\nend\n");
    assert!(with_index_text(&text, 4096).is_err());
    let text = format!("{head}component a\ntitle T\nsummary S\n{file}\nend\n");
    assert!(with_index_text(&text, 4096).is_err());
    let text = format!("{head}component a\ntitle T\nsummary S\non-remove R\n{file}\n");
    assert_eq!(with_index_text(&text, 4096), Err(IndexError::Truncated));
    let text = "eggos-components 1 0.3.0 x86_64 extra\n";
    assert_eq!(with_index_text(text, 4096), Err(IndexError::Line(1)));
}

#[test]
fn the_store_is_found_on_a_disk_or_in_the_host_share_for_this_machine_only() {
    let mut files = machine(&[WINDOWS]);
    let found = store(&mut files);
    assert_eq!(found.source, Source::Disk(STORE_DISK));
    assert_eq!(
        found.file_name(),
        format!("eggos-{}-x86_64-components.img", manager::VERSION)
    );
    assert!(manager::find_store(&mut files, "aarch64").is_none());
    let image = files.contents(STORE_DISK).unwrap().to_vec();
    files.nodes.remove(STORE_DISK.as_bytes());
    assert!(manager::find_store(&mut files, "x86_64").is_none());
    let share = format!(
        "/media/host/{}",
        manager::store_file_name(manager::VERSION, "x86_64")
    );
    files.file(&share, &image);
    assert_eq!(store(&mut files).source, Source::Share);
}

#[test]
fn an_install_copies_checks_records_and_renames_into_place() {
    let mut files = machine(&[WINDOWS]);
    let found = store(&mut files);
    let component = found.index.component("windows-apps").unwrap().clone();
    let mut seen = Vec::new();
    manager::install(&mut files, &found, &component, &mut |done, total| {
        seen.push((done, total));
        true
    })
    .unwrap();
    assert_eq!(seen, [(18, 18)]);
    let installed = manager::installed_path("windows-apps", WINDOWS_LAYER);
    assert_eq!(
        files.contents(&installed),
        Some(&b"\x7fELF windows layer"[..])
    );
    let listed = manager::installed(&mut files);
    assert_eq!(listed, [component]);
    assert!(
        files
            .paths_under(COMPONENTS)
            .iter()
            .all(|path| !path.contains(".staging-"))
    );
}

#[test]
fn a_damaged_file_detached_store_full_disk_or_cancel_leaves_nothing_installed() {
    let mut files = machine(&[WINDOWS]);
    let found = store(&mut files);
    let component = found.index.component("windows-apps").unwrap().clone();
    let offset = component.files[0].offset as usize;
    let mut damaged = files.contents(STORE_DISK).unwrap().to_vec();
    damaged[offset + 3] ^= 0x20;
    files.file(STORE_DISK, &damaged);
    let result = manager::install(&mut files, &found, &component, &mut |_, _| true);
    assert_eq!(
        result,
        Err(Failure::Damaged("windows-apps.component".into()))
    );
    assert!(files.paths_under(COMPONENTS).is_empty());
    assert!(manager::installed(&mut files).is_empty());
    let (first, second, error) = messages::install_failure(&result.unwrap_err(), &component);
    assert_eq!(
        first,
        "The store's copy of windows-apps.component is damaged."
    );
    assert_eq!(second, "Nothing was installed.");
    assert_eq!(error.code, "store_damaged");

    let mut files = machine(&[WINDOWS]);
    files.fail_reads_of = Some(STORE_DISK.as_bytes().to_vec());
    let result = manager::install(&mut files, &found, &component, &mut |_, _| true);
    assert_eq!(result, Err(Failure::Detached));
    assert!(files.paths_under(COMPONENTS).is_empty());

    let mut files = machine(&[WINDOWS]);
    files.available = Some(4);
    let result = manager::install(&mut files, &found, &component, &mut |_, _| true);
    assert_eq!(
        result,
        Err(Failure::NoSpace {
            needed: 18,
            free: 4
        })
    );
    assert_eq!(
        messages::install_failure_line(&result.unwrap_err(), &component),
        "Not enough space: 18 B needed, 4 B free; nothing was installed."
    );
    assert!(files.paths_under(COMPONENTS).is_empty());

    let mut files = machine(&[WINDOWS]);
    let result = manager::install(&mut files, &found, &component, &mut |_, _| false);
    assert_eq!(result, Err(Failure::Cancelled));
    assert!(files.paths_under(COMPONENTS).is_empty());
}

#[test]
fn an_install_refuses_a_components_folder_that_is_a_link_and_a_user_who_is_not_root() {
    let mut files = machine(&[WINDOWS]);
    let found = store(&mut files);
    let component = found.index.component("windows-apps").unwrap().clone();
    files.folder("/var/lib/eggos").link(COMPONENTS, "/etc");
    let result = manager::install(&mut files, &found, &component, &mut |_, _| true);
    assert_eq!(result, Err(Failure::UnsafeFolder));
    assert!(files.paths_under("/etc").is_empty());

    let mut files = machine(&[WINDOWS]);
    files.read_only.push(b"/var/lib".to_vec());
    let result = manager::install(&mut files, &found, &component, &mut |_, _| true);
    assert_eq!(result, Err(Failure::NotAllowed));
}

#[test]
fn installing_one_that_needs_another_plans_the_needed_one_first() {
    let mut files = machine(&[DESKTOP, APPS, WINDOWS]);
    let found = store(&mut files);
    let plan = manager::plan_install(&found.index, &[], "builtin-apps").unwrap();
    let names: Vec<&str> = plan.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["desktop", "builtin-apps"]);
    assert_eq!(
        messages::also_installs(&plan).unwrap(),
        "Installs Desktop (10 B) too: Built-in apps need it."
    );
    let installed = vec![found.index.component("desktop").unwrap().clone()];
    let plan = manager::plan_install(&found.index, &installed, "builtin-apps").unwrap();
    assert_eq!(plan.len(), 1);
    assert!(
        manager::plan_install(&found.index, &installed, "desktop")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        manager::plan_install(&found.index, &[], "winapps"),
        Err(Refusal::NoSuchComponent("winapps".into()))
    );
}

#[test]
fn removing_one_that_is_needed_or_in_use_is_refused_and_otherwise_leaves_nothing() {
    let mut files = machine(&[DESKTOP, APPS, WINDOWS]);
    let found = store(&mut files);
    for name in ["desktop", "builtin-apps", "windows-apps"] {
        let component = found.index.component(name).unwrap().clone();
        manager::install(&mut files, &found, &component, &mut |_, _| true).unwrap();
    }
    let installed = manager::installed(&mut files);
    assert_eq!(
        manager::check_remove(&mut files, &installed, "desktop"),
        Err(Refusal::NeededBy(vec!["Built-in apps".into()]))
    );
    files.file(
        "/proc/12/cmdline",
        b"/usr/lib/eggos/windows\0/home/root/Notepad.exe\0",
    );
    let refusal = manager::check_remove(&mut files, &installed, "windows-apps").unwrap_err();
    assert_eq!(
        refusal,
        Refusal::InUse {
            names: vec!["Notepad".into()],
            count: 1
        }
    );
    assert_eq!(
        messages::refusal(&refusal, "store.img").0,
        "In use: Notepad is running. Quit it, then remove."
    );
    files.nodes.remove(&b"/proc/12/cmdline"[..]);
    let checked = manager::check_remove(&mut files, &installed, "windows-apps").unwrap();
    assert!(checked.is_some());
    manager::remove(&mut files, "windows-apps").unwrap();
    assert!(
        files
            .paths_under(COMPONENTS)
            .iter()
            .all(|path| !path.contains("windows"))
    );
    assert_eq!(
        {
            let installed = manager::installed(&mut files);
            manager::check_remove(&mut files, &installed, "windows-apps")
        },
        Ok(None)
    );
}

#[test]
fn a_program_run_through_a_link_or_seen_by_its_executable_keeps_the_component_in_use() {
    let mut files = machine(&[WINDOWS]);
    let found = store(&mut files);
    let component = found.index.component("windows-apps").unwrap().clone();
    manager::install(&mut files, &found, &component, &mut |_, _| true).unwrap();
    let installed_layer = manager::installed_path("windows-apps", WINDOWS_LAYER);
    files.link("/bin/windows", &installed_layer);
    files.file("/proc/20/cmdline", b"/bin/windows\0registry\0list\0");
    assert_eq!(
        manager::in_use(&mut files, &component),
        (vec!["windows".into()], 1)
    );
    files.nodes.remove(&b"/proc/20/cmdline"[..]);
    files.file("/proc/21/cmdline", b"windows\0/home/root/Paint.exe\0");
    files.link("/proc/21/exe", "/bin/windows");
    assert_eq!(
        manager::in_use(&mut files, &component),
        (vec!["Paint".into()], 1)
    );
    files.nodes.remove(&b"/proc/21/exe"[..]);
    files.link("/proc/21/exe", &installed_layer);
    assert_eq!(manager::in_use(&mut files, &component).1, 1);
    files.nodes.remove(&b"/proc/21/exe"[..]);
    files.link("/proc/21/exe", "/bin/ls");
    files.file("/proc/22/cmdline", b"/bin/ls\0/usr/lib/eggos/windows\0");
    assert_eq!(manager::in_use(&mut files, &component), (Vec::new(), 0));
    assert_eq!(
        {
            let installed = manager::installed(&mut files);
            manager::check_remove(&mut files, &installed, "windows-apps")
                .map(|found| found.is_some())
        },
        Ok(true)
    );
}

#[test]
fn leftovers_of_an_interrupted_install_or_remove_are_tidied_and_never_listed() {
    let mut files = machine(&[WINDOWS]);
    files
        .file(
            &format!("{COMPONENTS}/.staging-windows-apps/root/x"),
            b"half",
        )
        .file(&format!("{COMPONENTS}/.removing-desktop/component"), b"x")
        .file(&format!("{COMPONENTS}/stray/component"), b"not a record");
    assert!(manager::installed(&mut files).is_empty());
    manager::tidy(&mut files);
    assert_eq!(
        files.paths_under(COMPONENTS),
        [
            format!("{COMPONENTS}/stray"),
            format!("{COMPONENTS}/stray/component")
        ]
    );
}

#[test]
fn a_second_action_is_refused_while_one_runs_and_a_dead_holder_s_lock_is_taken_over() {
    let mut files = machine(&[WINDOWS]);
    files.folder("/run/eggos").folder("/proc/40");
    files.file(manager::LOCK, b"40 install Desktop");
    assert_eq!(
        manager::lock(&mut files, "Windows apps", false, 41),
        Err(Refusal::Busy {
            title: "Desktop".into(),
            removing: false
        })
    );
    files.nodes.remove(&b"/proc/40"[..]);
    assert_eq!(manager::lock(&mut files, "Windows apps", false, 41), Ok(()));
    assert_eq!(
        files.contents(manager::LOCK),
        Some(&b"41 install Windows apps"[..])
    );
}

#[derive(Default)]
struct FakeConsole {
    output: String,
    error: String,
    terminal: bool,
    answer: &'static str,
}

impl Console for FakeConsole {
    fn write_output(&mut self, bytes: &[u8]) {
        self.output.push_str(core::str::from_utf8(bytes).unwrap());
    }

    fn write_error(&mut self, bytes: &[u8]) {
        self.error.push_str(core::str::from_utf8(bytes).unwrap());
    }

    fn is_terminal(&mut self) -> bool {
        self.terminal
    }

    fn read_answer(&mut self, buffer: &mut [u8]) -> usize {
        buffer[..self.answer.len()].copy_from_slice(self.answer.as_bytes());
        self.answer.len()
    }
}

const CONTEXT: Context<'static> = Context {
    arch: "x86_64",
    pid: 7,
    coded: false,
};

fn run(files: &mut FakeFiles, console: &mut FakeConsole, line: &str) -> i32 {
    let words: Vec<&str> = line.split(' ').collect();
    command::run(files, console, CONTEXT, &words)
}

#[test]
fn the_command_lists_installs_and_removes_in_the_design_s_words() {
    let mut files = machine(&[WINDOWS]);
    let mut console = FakeConsole::default();
    assert_eq!(run(&mut files, &mut console, "list"), 0);
    assert_eq!(
        console.output,
        "NAME          STATE          SIZE  TITLE\nwindows-apps  not-installed  18 B  Windows apps\n"
    );
    assert!(console.error.starts_with("store: eggos-"));
    assert!(console.error.trim_end().ends_with("x86_64) on /dev/vdc"));

    let mut console = FakeConsole::default();
    assert_eq!(
        run(&mut files, &mut console, "install --dry-run windows-apps"),
        0
    );
    assert_eq!(
        console.output,
        "would install Windows apps (18 B): 18 B in all\n"
    );
    let mut console = FakeConsole::default();
    assert_eq!(run(&mut files, &mut console, "install windows-apps"), 0);
    assert_eq!(
        console.output,
        "Installing Windows apps (18 B)...\nInstalled Windows apps (18 B).\n"
    );
    assert!(files.contents(manager::LOCK).is_none());
    let mut console = FakeConsole::default();
    assert_eq!(run(&mut files, &mut console, "install windows-apps"), 0);
    assert_eq!(console.output, "Windows apps is already installed.\n");

    let mut console = FakeConsole::default();
    assert_eq!(run(&mut files, &mut console, "remove windows-apps"), 1);
    assert_eq!(
        console.error,
        "Removing Windows apps needs a yes: add --yes.\n"
    );
    let mut console = FakeConsole {
        terminal: true,
        answer: "n\n",
        ..FakeConsole::default()
    };
    assert_eq!(run(&mut files, &mut console, "remove windows-apps"), 1);
    assert!(
        console
            .error
            .starts_with("Remove Windows apps (18 B)? Programs stay in Apps. [y/N] ")
    );
    assert!(console.error.ends_with("Nothing removed.\n"));
    let mut console = FakeConsole::default();
    assert_eq!(
        run(&mut files, &mut console, "remove --yes windows-apps"),
        0
    );
    assert_eq!(console.output, "Removed Windows apps; 18 B freed.\n");
    let mut console = FakeConsole::default();
    assert_eq!(
        run(&mut files, &mut console, "remove --yes windows-apps"),
        0
    );
    assert_eq!(console.output, "Windows apps is not installed.\n");
    assert!(manager::installed(&mut files).is_empty());
}

#[test]
fn the_command_refuses_without_a_store_an_unknown_name_and_a_wrong_command_line() {
    let mut files = FakeFiles::new();
    files.folder("/var").folder("/run").folder("/proc");
    let mut console = FakeConsole::default();
    assert_eq!(run(&mut files, &mut console, "install windows-apps"), 1);
    assert!(
        console
            .error
            .starts_with("No component store is attached: attach the store disk")
    );
    let mut files = machine(&[WINDOWS]);
    let mut console = FakeConsole::default();
    assert_eq!(run(&mut files, &mut console, "install winapps"), 1);
    assert_eq!(
        console.error,
        "No component named \"winapps\": component list names them.\n  try: component list\n"
    );
    let mut console = FakeConsole::default();
    assert_eq!(run(&mut files, &mut console, "install --json winapps"), 1);
    assert!(console.error.contains("\"no_such_component\""));
    let lines = [
        "",
        "list extra",
        "install",
        "install a b",
        "remove --force a",
        "upgrade a",
    ];
    for line in lines {
        let mut console = FakeConsole::default();
        let words: Vec<&str> = line.split(' ').filter(|word| !word.is_empty()).collect();
        assert_eq!(
            command::run(&mut files, &mut console, CONTEXT, &words),
            command::USAGE_STATUS,
            "{line}"
        );
    }
}

#[test]
fn the_json_records_carry_the_design_s_fields() {
    let mut files = machine(&[DESKTOP, APPS]);
    let mut console = FakeConsole::default();
    assert_eq!(
        run(&mut files, &mut console, "install --json builtin-apps"),
        0
    );
    assert_eq!(
        console.output,
        "{\"installed\":[\"desktop\",\"builtin-apps\"],\"size_bytes\":31}\n"
    );
    let mut console = FakeConsole::default();
    assert_eq!(run(&mut files, &mut console, "list --json"), 0);
    let first = console.output.lines().next().unwrap();
    assert!(first.starts_with("{\"name\":\"desktop\",\"title\":\"Desktop\""));
    assert!(first.contains("\"state\":\"installed\""));
    assert!(first.contains("\"needed_by\":[\"Built-in apps\"]"));
    assert!(console.error.starts_with("{\"store\":{\"file\":\"eggos-"));
}

#[test]
fn the_registry_s_examples_run_as_written() {
    let layer = vec![0x7f; 1_900_000];
    let windows = Part {
        files: &[(WINDOWS_LAYER, layer.as_slice())],
        ..WINDOWS
    };
    let mut ran = 0;
    for command in eggos_commands::commands::COMMANDS.iter() {
        if command.words[0] != "component" {
            continue;
        }
        for example in command.examples {
            let mut files = machine(core::slice::from_ref(&windows));
            let mut console = FakeConsole::default();
            let words: Vec<&str> = example.line.split(' ').skip(1).collect();
            let status = command::run(&mut files, &mut console, CONTEXT, &words);
            assert_eq!(
                (status, console.output.as_str()),
                (example.status, example.output),
                "{}: {}",
                example.line,
                console.error
            );
            ran += 1;
        }
    }
    assert_eq!(ran, 4);
}
