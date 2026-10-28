use crate::architecture::Architecture::{self, Aarch64, Riscv64, X86_64};
use crate::fake::{FakeFiles, Node, Operation};
use crate::installer::*;
use crate::program::NotRunnable;
use crate::program::samples::{linux_program, mach_o, windows_program};
use eggos_app_manifest::Source;

fn host_with(path: &str, bytes: &[u8]) -> FakeFiles {
    let mut files = FakeFiles::new();
    files.file(path, bytes);
    files
}

fn paint() -> Vec<u8> {
    let mut program = windows_program(0x8664);
    program.extend((0..70_000u32).map(|index| index as u8));
    program
}

fn planned(files: &mut FakeFiles, path: &str, machine: Architecture) -> Plan {
    plan(files, path.as_bytes(), None, machine).unwrap()
}

fn refused(files: &mut FakeFiles, path: &str, machine: Architecture) -> Problem {
    plan(files, path.as_bytes(), None, machine)
        .unwrap_err()
        .problem
}

fn manifest_of(files: &FakeFiles, name: &str) -> String {
    match files.find(&format!("/Applications/{name}/app.manifest")) {
        Some(Node::File(bytes, _)) => String::from_utf8(bytes.clone()).unwrap(),
        other => panic!("{other:?}"),
    }
}

fn installed_names(files: &mut FakeFiles) -> Vec<String> {
    let mut names = Vec::new();
    for_each_installed(files, |app| names.push(app.name.to_owned())).unwrap();
    names
}

#[test]
fn a_windows_program_installs_into_its_folder_with_its_manifest() {
    let program = paint();
    let mut files = host_with("/media/host/win-paint.exe", &program);
    let plan = planned(&mut files, "/media/host/win-paint.exe", X86_64);
    assert_eq!(plan.name(), "win-paint");
    assert_eq!(plan.run(), b"win-paint.exe");
    assert_eq!(plan.total_bytes(), program.len() as u64);
    assert_eq!(plan.folder().as_bytes(), b"/Applications/win-paint");
    install(&mut files, plan, &mut [0u8; 4096]).unwrap();
    assert_eq!(
        files.find("/Applications/win-paint/win-paint.exe"),
        Some(&Node::File(program, 0o755))
    );
    assert_eq!(
        manifest_of(&files, "win-paint"),
        "eggos-app 1\nname=win-paint\nsource=windows\nrun=win-paint.exe\narchitecture=x86_64\n"
    );
    assert_eq!(
        files.paths_under("/Applications/"),
        [
            "/Applications/win-paint",
            "/Applications/win-paint/win-paint.exe",
            "/Applications/win-paint/app.manifest"
        ]
    );
    assert_eq!(installed_names(&mut files), ["win-paint"]);
    assert_eq!(files.open_count(), 0);
    assert_eq!(
        refused(&mut files, "/media/host/win-paint.exe", X86_64),
        Problem::AlreadyInstalled
    );
}

#[test]
fn the_manifest_is_written_last_through_a_temporary_file() {
    let mut files = host_with("/media/host/tool", &linux_program(183));
    let plan = planned(&mut files, "/media/host/tool", Aarch64);
    install(&mut files, plan, &mut [0u8; 16]).unwrap();
    assert_eq!(
        files.log,
        [
            Operation::MakeDirectory("/Applications/tool".into()),
            Operation::Create("/Applications/tool/tool".into()),
            Operation::Create("/Applications/tool/app.manifest.tmp".into()),
            Operation::Rename(
                "/Applications/tool/app.manifest.tmp".into(),
                "/Applications/tool/app.manifest".into()
            ),
            Operation::Sync,
        ]
    );
    assert!(manifest_of(&files, "tool").contains("source=linux\nrun=tool\narchitecture=aarch64\n"));
}

#[test]
fn an_app_bundle_is_copied_whole_and_runs_its_contents_macos_program() {
    let mut files = FakeFiles::new();
    files.directory("/media/host/Paint.app");
    files.directory("/media/host/Paint.app/Contents");
    files.file("/media/host/Paint.app/Contents/Info.plist", b"<plist/>");
    files.directory("/media/host/Paint.app/Contents/MacOS");
    files.file(
        "/media/host/Paint.app/Contents/MacOS/Paint",
        &mach_o(0x0100_000c, 2),
    );
    files.directory("/media/host/Paint.app/Contents/Resources");
    files.file(
        "/media/host/Paint.app/Contents/Resources/icon.png",
        &[7; 300],
    );
    let typed_with_a_slash = planned(&mut files, "/media/host/Paint.app/", Aarch64);
    assert_eq!(typed_with_a_slash.source(), b"/media/host/Paint.app");
    assert_eq!(typed_with_a_slash.run(), b"Paint.app/Contents/MacOS/Paint");
    let plan = planned(&mut files, "/media/host/Paint.app", Aarch64);
    assert_eq!(plan.name(), "Paint");
    assert_eq!(plan.run(), b"Paint.app/Contents/MacOS/Paint");
    assert_eq!(plan.total_bytes(), 8 + 64 + 300);
    assert_eq!(plan.program().source, Source::Macos);
    install(&mut files, plan, &mut [0u8; 100]).unwrap();
    assert_eq!(
        files.paths_under("/Applications/"),
        [
            "/Applications/Paint",
            "/Applications/Paint/Paint.app",
            "/Applications/Paint/Paint.app/Contents",
            "/Applications/Paint/Paint.app/Contents/Info.plist",
            "/Applications/Paint/Paint.app/Contents/MacOS",
            "/Applications/Paint/Paint.app/Contents/MacOS/Paint",
            "/Applications/Paint/Paint.app/Contents/Resources",
            "/Applications/Paint/Paint.app/Contents/Resources/icon.png",
            "/Applications/Paint/app.manifest",
        ]
    );
    assert_eq!(
        files.find("/Applications/Paint/Paint.app/Contents/Resources/icon.png"),
        Some(&Node::File(vec![7; 300], 0o755))
    );
    assert!(manifest_of(&files, "Paint").contains("run=Paint.app/Contents/MacOS/Paint\n"));
}

#[test]
fn a_bundle_s_one_program_is_found_whatever_its_name_and_two_are_refused() {
    let mut files = FakeFiles::new();
    files.directory("/media/host/Big Tool.app");
    files.directory("/media/host/Big Tool.app/Contents");
    files.directory("/media/host/Big Tool.app/Contents/MacOS");
    files.file(
        "/media/host/Big Tool.app/Contents/MacOS/bigtool",
        &mach_o(0x0100_0007, 2),
    );
    let plan = planned(&mut files, "/media/host/Big Tool.app", X86_64);
    assert_eq!(plan.name(), "Big Tool");
    assert_eq!(plan.run(), b"Big Tool.app/Contents/MacOS/bigtool");
    files.file("/media/host/Big Tool.app/Contents/MacOS/helper", b"x");
    assert_eq!(
        refused(&mut files, "/media/host/Big Tool.app", X86_64),
        Problem::BundleWithoutProgram
    );
    files.directory("/media/host/Empty.app");
    assert_eq!(
        refused(&mut files, "/media/host/Empty.app", X86_64),
        Problem::BundleWithoutProgram
    );
}

#[test]
fn what_this_machine_cannot_run_is_refused_before_anything_is_written() {
    let mut files = host_with("/media/host/win-paint.exe", &paint());
    files.file("/media/host/arm.exe", &windows_program(0xaa64));
    files.file("/media/host/notes.txt", b"hello");
    files.directory("/media/host/Folder");
    assert_eq!(
        refused(&mut files, "/media/host/win-paint.exe", Riscv64),
        Problem::NotRunnable(NotRunnable::LayerNotOnThisCpu(Source::Windows))
    );
    assert_eq!(
        refused(&mut files, "/media/host/arm.exe", X86_64),
        Problem::NotRunnable(NotRunnable::OtherCpu(Aarch64))
    );
    assert_eq!(
        refused(&mut files, "/media/host/notes.txt", X86_64),
        Problem::NotRunnable(NotRunnable::NotAProgram)
    );
    assert_eq!(
        refused(&mut files, "/media/host/Folder", X86_64),
        Problem::FolderNotApp
    );
    assert_eq!(
        refused(&mut files, "/media/host/missing", X86_64),
        Problem::Unreadable(eggos_runtime::errno::Errno::ENOENT)
    );
    files
        .nodes
        .retain(|(path, _)| path.as_slice() != b"/usr/lib/eggos/windows");
    assert_eq!(
        refused(&mut files, "/media/host/win-paint.exe", X86_64),
        Problem::NotRunnable(NotRunnable::LayerMissing(Source::Windows))
    );
    assert!(files.log.is_empty());
}

#[test]
fn names_come_from_the_file_name_or_from_the_caller_and_must_be_valid() {
    let program = linux_program(62);
    let mut files = host_with("/media/host/it's.bin", &program);
    files.file(&format!("/media/host/{}.bin", "x".repeat(33)), &program);
    files.file("/media/host/fine", &program);
    let problem = plan(&mut files, b"/media/host/it's.bin", None, X86_64).unwrap_err();
    assert_eq!(problem.problem, Problem::InvalidName);
    assert_eq!(problem.name.as_str(), "it's");
    assert_eq!(
        refused(
            &mut files,
            &format!("/media/host/{}.bin", "x".repeat(33)),
            X86_64
        ),
        Problem::InvalidName
    );
    assert_eq!(
        plan(&mut files, b"/media/host/it's.bin", Some(b"Its"), X86_64)
            .unwrap_err()
            .problem,
        Problem::InvalidProgramPath
    );
    let plan = plan(&mut files, b"/media/host/fine", Some(b"Fine Tool"), X86_64).unwrap();
    assert_eq!(plan.name(), "Fine Tool");
    assert_eq!(
        derived_name(b"/a/b/hello.world.exe").as_str(),
        "hello.world"
    );
    assert_eq!(derived_name(b"/a/.hidden").as_str(), ".hidden");
    assert_eq!(derived_name(b"Paint.app/").as_str(), "Paint");
    let long = [b'x'; Name::CAPACITY + 1];
    assert!(Name::whole(&long).is_none());
    assert_eq!(Name::new(&long).as_bytes().len(), Name::CAPACITY);
    assert_eq!(
        Name::whole(&long[..Name::CAPACITY]).unwrap().as_bytes(),
        &long[..Name::CAPACITY]
    );
}

#[test]
fn without_a_disk_the_install_fails_and_leaves_nothing() {
    let mut files = host_with("/media/host/tool", &linux_program(62));
    files.read_only.push(b"/Applications".to_vec());
    let plan = planned(&mut files, "/media/host/tool", X86_64);
    assert_eq!(
        install(&mut files, plan, &mut [0u8; 64]),
        Err(Problem::ReadOnly)
    );
    assert!(files.paths_under("/Applications/").is_empty());
}

#[test]
fn a_full_disk_fails_the_install_and_removes_what_was_copied() {
    let program = paint();
    let mut files = host_with("/media/host/win-paint.exe", &program);
    files.space = Some(program.len() + 2 * 10 + 40_000);
    let plan = planned(&mut files, "/media/host/win-paint.exe", X86_64);
    assert_eq!(
        install(&mut files, plan, &mut [0u8; 4096]),
        Err(Problem::DiskFull)
    );
    assert!(files.paths_under("/Applications/").is_empty());
    assert_eq!(files.open_count(), 0);
}

#[test]
fn progress_follows_the_bytes_and_cancelling_removes_everything() {
    let program = paint();
    let mut files = host_with("/media/host/win-paint.exe", &program);
    let plan = planned(&mut files, "/media/host/win-paint.exe", X86_64);
    let mut installation = Installation::new(plan);
    let mut buffer = [0u8; 8192];
    let mut last = 0;
    for _ in 0..4 {
        match installation.step(&mut files, &mut buffer) {
            Progress::Copying(done) => {
                assert!(done >= last);
                last = done;
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(last > 0 && last < program.len() as u64);
    assert!(installation.cancel(&mut files));
    assert!(files.paths_under("/Applications/").is_empty());
    assert_eq!(files.open_count(), 0);

    let mut installation =
        Installation::new(planned(&mut files, "/media/host/win-paint.exe", X86_64));
    let mut seen_registering = false;
    loop {
        match installation.step(&mut files, &mut buffer) {
            Progress::Copying(done) => assert!(done <= program.len() as u64),
            Progress::Registering => seen_registering = true,
            Progress::Installed => break,
            Progress::Failed(problem) => panic!("{problem:?}"),
        }
    }
    assert!(seen_registering);
    assert!(!installation.cancel(&mut files));
    assert_eq!(installed_names(&mut files), ["win-paint"]);
}

#[test]
fn a_folder_left_by_an_interrupted_install_is_replaced() {
    let mut files = host_with("/media/host/tool", &linux_program(62));
    files.directory("/Applications/tool");
    files.file("/Applications/tool/tool", b"half");
    files.file("/Applications/tool/app.manifest.tmp", b"eggos-app 1\n");
    let plan = planned(&mut files, "/media/host/tool", X86_64);
    install(&mut files, plan, &mut [0u8; 64]).unwrap();
    assert_eq!(installed_names(&mut files), ["tool"]);
    assert_eq!(
        files.paths_under("/Applications/"),
        [
            "/Applications/tool",
            "/Applications/tool/tool",
            "/Applications/tool/app.manifest"
        ]
    );
}

#[test]
fn uninstall_removes_the_manifest_first_then_everything_else() {
    let mut files = FakeFiles::new();
    files.directory("/media/host/Paint.app");
    files.directory("/media/host/Paint.app/Contents");
    files.directory("/media/host/Paint.app/Contents/MacOS");
    files.file(
        "/media/host/Paint.app/Contents/MacOS/Paint",
        &mach_o(0x0100_0007, 2),
    );
    let plan = planned(&mut files, "/media/host/Paint.app", X86_64);
    install(&mut files, plan, &mut [0u8; 64]).unwrap();
    files.log.clear();
    uninstall(&mut files, b"Paint").unwrap();
    assert_eq!(
        files.log.first(),
        Some(&Operation::RemoveFile(
            "/Applications/Paint/app.manifest".into()
        ))
    );
    assert_eq!(files.log.last(), Some(&Operation::Sync));
    assert!(files.paths_under("/Applications/").is_empty());
    assert!(files.find("/Applications").is_some());
    assert!(installed_names(&mut files).is_empty());
    assert_eq!(uninstall(&mut files, b"Paint"), Err(Problem::NotInstalled));
    assert_eq!(uninstall(&mut files, b"../etc"), Err(Problem::NotInstalled));
    files.directory("/Applications/Stray");
    assert_eq!(uninstall(&mut files, b"Stray"), Err(Problem::NotInstalled));
}

#[test]
fn a_bundle_with_a_link_or_too_deep_is_refused() {
    let mut files = FakeFiles::new();
    files.directory("/media/host/Link.app");
    files.directory("/media/host/Link.app/Contents");
    files.directory("/media/host/Link.app/Contents/MacOS");
    files.file(
        "/media/host/Link.app/Contents/MacOS/Link",
        &mach_o(0x0100_0007, 2),
    );
    files.other("/media/host/Link.app/Contents/Current");
    assert_eq!(
        refused(&mut files, "/media/host/Link.app", X86_64),
        Problem::UnsupportedEntry
    );
    let mut files = FakeFiles::new();
    files.directory("/media/host/Deep.app");
    files.directory("/media/host/Deep.app/Contents");
    files.directory("/media/host/Deep.app/Contents/MacOS");
    files.file(
        "/media/host/Deep.app/Contents/MacOS/Deep",
        &mach_o(0x0100_0007, 2),
    );
    let mut path = String::from("/media/host/Deep.app/Contents");
    for level in 0..MAX_DEPTH {
        path.push_str(&format!("/d{level}"));
        files.directory(&path);
    }
    assert_eq!(
        refused(&mut files, "/media/host/Deep.app", X86_64),
        Problem::TooDeep
    );
}

#[test]
fn listing_skips_folders_without_a_valid_manifest() {
    let mut files = FakeFiles::new();
    files.directory("/Applications/Good");
    files.file(
        "/Applications/Good/app.manifest",
        b"eggos-app 1\nname=Good\nsource=linux\nrun=good\n",
    );
    files.directory("/Applications/Bad");
    files.file("/Applications/Bad/app.manifest", b"garbage");
    files.directory("/Applications/Other");
    files.file(
        "/Applications/Other/app.manifest",
        b"eggos-app 1\nname=Good\nsource=linux\nrun=good\n",
    );
    files.file("/Applications/loose", b"x");
    assert_eq!(installed_names(&mut files), ["Good"]);
    files
        .nodes
        .retain(|(path, _)| !path.starts_with(b"/Applications"));
    assert!(installed_names(&mut files).is_empty());
}

#[test]
fn remove_tree_removes_nested_folders_and_single_files() {
    let mut files = FakeFiles::new();
    files.directory("/tmp");
    files.directory("/tmp/a");
    files.directory("/tmp/a/b");
    files.file("/tmp/a/b/c", b"1");
    files.file("/tmp/a/d", b"2");
    files.file("/tmp/e", b"3");
    remove_tree(&mut files, b"/tmp/a").unwrap();
    remove_tree(&mut files, b"/tmp/e").unwrap();
    assert!(files.paths_under("/tmp/").is_empty());
}
