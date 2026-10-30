use crate::architecture::Architecture::{self, Aarch64, Riscv64, X86_64};
use crate::archive::samples::{Member, deflate_fixed_literals, deflated, stored, zip};
use crate::fake::{FakeFiles, Node, Operation};
use crate::files::{Files, Kind, Path};
use crate::installer::*;
use crate::program::NotRunnable;
use crate::program::samples::{dynamic_program, linux_program, mach_o, windows_program};
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

fn framework_bundle() -> FakeFiles {
    let mut files = FakeFiles::new();
    let framework = "/media/host/Tool.app/Contents/Frameworks/Kit.framework";
    let versions = format!("{framework}/Versions");
    let version = format!("{framework}/Versions/A");
    for folder in [
        "/media/host/Tool.app",
        "/media/host/Tool.app/Contents",
        "/media/host/Tool.app/Contents/MacOS",
        "/media/host/Tool.app/Contents/Frameworks",
        framework,
        versions.as_str(),
        version.as_str(),
    ] {
        files.directory(folder);
    }
    files.file(
        "/media/host/Tool.app/Contents/MacOS/Tool",
        &mach_o(0x0100_0007, 2),
    );
    files.file(&format!("{framework}/Versions/A/Kit"), &[9; 40]);
    files.link(&format!("{framework}/Versions/Current"), "A");
    files.link(&format!("{framework}/Kit"), "Versions/Current/Kit");
    files.link(
        "/media/host/Tool.app/Contents/MacOS/Kit",
        "../Frameworks/Kit.framework/Kit",
    );
    files
}

#[test]
fn a_bundle_s_symbolic_links_are_made_again_as_links() {
    let mut files = framework_bundle();
    let plan = planned(&mut files, "/media/host/Tool.app", X86_64);
    assert_eq!(plan.total_bytes(), 64 + 40);
    install(&mut files, plan, &mut [0u8; 64]).unwrap();
    let framework = "/Applications/Tool/Tool.app/Contents/Frameworks/Kit.framework";
    assert_eq!(
        files.find(&format!("{framework}/Versions/Current")),
        Some(&Node::Link(b"A".to_vec()))
    );
    assert_eq!(
        files.find(&format!("{framework}/Kit")),
        Some(&Node::Link(b"Versions/Current/Kit".to_vec()))
    );
    assert_eq!(
        files.find("/Applications/Tool/Tool.app/Contents/MacOS/Kit"),
        Some(&Node::Link(b"../Frameworks/Kit.framework/Kit".to_vec()))
    );
    assert!(files.log.contains(&Operation::MakeLink(
        "A".into(),
        format!("{framework}/Versions/Current")
    )));
    let mut target = [0u8; 64];
    let through = files
        .status_following(format!("{framework}/Kit").as_bytes())
        .unwrap();
    assert_eq!((through.kind, through.size), (Kind::File, 40));
    assert_eq!(
        files.read_link(format!("{framework}/Kit").as_bytes(), &mut target),
        Ok(20)
    );
    assert!(manifest_of(&files, "Tool").contains("run=Tool.app/Contents/MacOS/Tool\n"));
    uninstall(&mut files, b"Tool").unwrap();
    assert!(files.paths_under("/Applications/").is_empty());
}

#[test]
fn a_bundle_s_link_that_could_lead_outside_it_is_refused() {
    for target in [
        "/etc/passwd",
        "../../../../../etc",
        "../../../..",
        "Versions/../../..",
        "A/../A",
        "",
    ] {
        let mut files = framework_bundle();
        files.link(
            "/media/host/Tool.app/Contents/Frameworks/Kit.framework/Escape",
            target,
        );
        assert_eq!(
            refused(&mut files, "/media/host/Tool.app", X86_64),
            Problem::UnsafeLink,
            "{target}"
        );
        assert!(files.paths_under("/Applications/").is_empty());
    }
}

#[test]
fn a_link_stays_inside_when_its_climbs_come_first_and_stop_at_its_folder() {
    for (link, target, inside) in [
        ("Current", "A", true),
        ("Kit.framework/Kit", "Versions/Current/Kit", true),
        ("Contents/MacOS/Kit", "../Frameworks/Kit", true),
        ("Contents/MacOS/Kit", "../../Contents", true),
        ("Contents/MacOS/Kit", "../../..", false),
        ("Kit", "..", false),
        ("Kit", ".", true),
        ("Kit", "./A//B/", true),
        ("a/b/c", "../../x/../y", false),
        ("a/b", "/a", false),
        ("a/b", "", false),
        ("a/b", "x\0y", false),
    ] {
        assert_eq!(
            link_stays_inside(link.as_bytes(), target.as_bytes()),
            inside,
            "{link} -> {target}"
        );
    }
}

#[test]
fn a_bundle_with_a_device_or_too_deep_is_refused() {
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

fn icns_holding(png: &[u8]) -> Vec<u8> {
    let mut file = b"icns".to_vec();
    file.extend_from_slice(&(png.len() as u32 + 16).to_be_bytes());
    file.extend_from_slice(b"ic07");
    file.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
    file.extend_from_slice(png);
    file
}

fn icon_of(files: &FakeFiles, name: &str) -> Vec<u8> {
    match files.find(&format!("/Applications/{name}/app.icon")) {
        Some(Node::File(bytes, _)) => bytes.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_zip_s_one_program_installs_with_everything_beside_it_and_no_mac_metadata() {
    let program = windows_program(0x8664);
    let readme = b"read me, deflated by hand";
    let zip = zip(&[
        stored("bin/win-paint.exe", &program),
        deflated("bin/README.txt", readme, deflate_fixed_literals(readme)),
        stored("__MACOSX/bin/._win-paint.exe", b"resource fork"),
    ]);
    let mut files = host_with("/media/host/Paint-1.2.zip", &zip);
    let plan = planned(&mut files, "/media/host/Paint-1.2.zip", X86_64);
    assert!(plan.is_archive());
    assert_eq!(plan.name(), "win-paint");
    assert_eq!(plan.run(), b"bin/win-paint.exe");
    assert_eq!(plan.source(), b"/media/host/Paint-1.2.zip");
    assert_eq!(plan.total_bytes(), (program.len() + readme.len()) as u64);
    install(&mut files, plan, &mut [0u8; 64]).unwrap();
    assert_eq!(
        files.paths_under("/Applications/"),
        [
            "/Applications/win-paint",
            "/Applications/win-paint/bin",
            "/Applications/win-paint/bin/win-paint.exe",
            "/Applications/win-paint/bin/README.txt",
            "/Applications/win-paint/app.manifest",
        ]
    );
    assert_eq!(
        files.find("/Applications/win-paint/bin/win-paint.exe"),
        Some(&Node::File(program, 0o755))
    );
    assert_eq!(
        files.find("/Applications/win-paint/bin/README.txt"),
        Some(&Node::File(readme.to_vec(), 0o644))
    );
    assert!(manifest_of(&files, "win-paint").contains("run=bin/win-paint.exe\n"));
    assert_eq!(files.open_count(), 0);
}

fn link_member(name: &str, target: &str) -> Member<'static> {
    Member {
        mode: Some(0o120_777),
        ..stored(name, target.as_bytes())
    }
}

#[test]
fn a_zip_s_symbolic_links_are_made_as_links_when_they_stay_inside() {
    let zip = zip(&[
        stored(
            "mac-tool.app/Contents/MacOS/mac-tool",
            &mach_o(0x0100_000c, 2),
        ),
        stored(
            "mac-tool.app/Contents/Frameworks/Kit.framework/Versions/A/Kit",
            b"kit",
        ),
        link_member(
            "mac-tool.app/Contents/Frameworks/Kit.framework/Versions/Current",
            "A",
        ),
        link_member(
            "mac-tool.app/Contents/Frameworks/Kit.framework/Kit",
            "Versions/Current/Kit",
        ),
    ]);
    let mut files = host_with("/media/host/mac-tool.zip", &zip);
    let plan = planned(&mut files, "/media/host/mac-tool.zip", Aarch64);
    assert_eq!(plan.run(), b"mac-tool.app/Contents/MacOS/mac-tool");
    install(&mut files, plan, &mut [0u8; 64]).unwrap();
    let framework = "/Applications/mac-tool/mac-tool.app/Contents/Frameworks/Kit.framework";
    assert_eq!(
        files.find(&format!("{framework}/Versions/Current")),
        Some(&Node::Link(b"A".to_vec()))
    );
    assert_eq!(
        files.find(&format!("{framework}/Kit")),
        Some(&Node::Link(b"Versions/Current/Kit".to_vec()))
    );
    uninstall(&mut files, b"mac-tool").unwrap();
    assert!(files.paths_under("/Applications/").is_empty());
}

#[test]
fn a_zip_s_link_that_leads_out_or_that_an_entry_goes_through_is_refused() {
    let program = windows_program(0x8664);
    for (members, problem) in [
        (
            vec![stored("tool.exe", &program), link_member("up", "..")],
            Problem::UnsafeLink,
        ),
        (
            vec![stored("tool.exe", &program), link_member("etc", "/etc")],
            Problem::UnsafeLink,
        ),
        (
            vec![
                stored("tool.exe", &program),
                link_member("here", "."),
                stored("here/inner/x", b"x"),
            ],
            Problem::Archive(crate::archive::ArchiveError::UnsafeName),
        ),
    ] {
        let mut files = host_with("/media/host/tool.zip", &zip(&members));
        let plan = planned(&mut files, "/media/host/tool.zip", X86_64);
        assert_eq!(install(&mut files, plan, &mut [0u8; 64]), Err(problem));
        assert!(files.paths_under("/Applications/").is_empty());
    }
}

#[test]
fn a_zipped_bundle_installs_with_its_own_icon() {
    let png = crate::icon::tests::solid_png(64, 0x0012_3456);
    let plist = b"<dict><key>CFBundleIconFile</key><string>AppIcon</string></dict>";
    let zip = zip(&[
        stored("mac-paint.app/", b""),
        stored("mac-paint.app/Contents/Info.plist", plist),
        stored(
            "mac-paint.app/Contents/MacOS/mac-paint",
            &mach_o(0x0100_000c, 2),
        ),
        stored(
            "mac-paint.app/Contents/Resources/AppIcon.icns",
            &icns_holding(&png),
        ),
    ]);
    let mut files = host_with("/media/host/mac-paint.zip", &zip);
    let plan = planned(&mut files, "/media/host/mac-paint.zip", Aarch64);
    assert_eq!(plan.name(), "mac-paint");
    assert_eq!(plan.run(), b"mac-paint.app/Contents/MacOS/mac-paint");
    assert_eq!(plan.program().source, Source::Macos);
    install(&mut files, plan, &mut [0u8; 64]).unwrap();
    let manifest = manifest_of(&files, "mac-paint");
    assert!(manifest.ends_with(
        "run=mac-paint.app/Contents/MacOS/mac-paint\narchitecture=aarch64\nicon=app.icon\n"
    ));
    let icon = icon_of(&files, "mac-paint");
    assert_eq!(icon.len(), 13_312);
    assert!(
        icon.chunks(4)
            .all(|pixel| pixel == [0x56, 0x34, 0x12, 0xff])
    );
    let mut listed = Vec::new();
    for_each_installed(&mut files, |app| listed.push(app.icon.map(str::to_owned))).unwrap();
    assert_eq!(listed, [Some("app.icon".to_owned())]);
    uninstall(&mut files, b"mac-paint").unwrap();
    assert!(files.paths_under("/Applications/").is_empty());
}

#[test]
fn a_program_with_a_png_beside_it_gets_that_icon() {
    let mut files = host_with("/media/host/tool", &linux_program(62));
    files.file(
        "/media/host/tool.png",
        &crate::icon::tests::solid_png(48, 0x00ab_cdef),
    );
    let plan = planned(&mut files, "/media/host/tool", X86_64);
    install(&mut files, plan, &mut [0u8; 64]).unwrap();
    let icon = icon_of(&files, "tool");
    assert_eq!(icon[..4], [0xef, 0xcd, 0xab, 0xff]);
    assert!(manifest_of(&files, "tool").ends_with("icon=app.icon\n"));
    let mut broken = host_with("/media/host/tool", &linux_program(62));
    broken.file("/media/host/tool.png", b"\x89PNG\r\n\x1a\n not really");
    let plan = planned(&mut broken, "/media/host/tool", X86_64);
    install(&mut broken, plan, &mut [0u8; 64]).unwrap();
    assert!(broken.find("/Applications/tool/app.icon").is_none());
    assert!(!manifest_of(&broken, "tool").contains("icon="));
}

#[test]
fn a_zip_with_several_programs_needs_an_entry_and_one_with_none_is_refused() {
    let program = windows_program(0x8664);
    let two = zip(&[stored("bin/a.exe", &program), stored("bin/b.exe", &program)]);
    let mut files = host_with("/media/host/tools.zip", &two);
    let problem = refused(&mut files, "/media/host/tools.zip", X86_64);
    let Problem::SeveralPrograms(listing) = problem else {
        panic!("{problem:?}");
    };
    assert_eq!(listing.as_str(), "bin/a.exe, bin/b.exe");
    assert_eq!(
        crate::messages::reason(problem).as_str(),
        "it holds several programs (bin/a.exe, bin/b.exe): choose one with --entry <path>"
    );
    let entry_plan = plan_entry(
        &mut files,
        b"/media/host/tools.zip",
        None,
        Some(b"bin/b.exe"),
        X86_64,
    )
    .unwrap();
    assert_eq!(entry_plan.name(), "b");
    assert_eq!(
        plan_entry(
            &mut files,
            b"/media/host/tools.zip",
            None,
            Some(b"c.exe"),
            X86_64
        )
        .unwrap_err()
        .problem,
        Problem::NoSuchEntry
    );
    let notes = zip(&[stored("notes.txt", b"just text"), stored("LICENSE", b"MIT")]);
    let mut files = host_with("/media/host/notes.zip", &notes);
    let refusal = plan(&mut files, b"/media/host/notes.zip", None, X86_64).unwrap_err();
    assert_eq!(refusal.problem, Problem::NoProgramInArchive);
    assert_eq!(refusal.name.as_str(), "notes");
    let escaping = zip(&[stored("../evil.exe", &program)]);
    let mut files = host_with("/media/host/evil.zip", &escaping);
    assert_eq!(
        refused(&mut files, "/media/host/evil.zip", X86_64),
        Problem::Archive(crate::archive::ArchiveError::UnsafeName)
    );
    let inside_bundle = zip(&[stored("X.app/Contents/Resources/helper.exe", &program)]);
    let mut files = host_with("/media/host/x.zip", &inside_bundle);
    assert_eq!(
        refused(&mut files, "/media/host/x.zip", X86_64),
        Problem::NoProgramInArchive
    );
}

#[test]
fn a_zip_holding_a_file_the_installer_writes_is_refused() {
    let program = windows_program(0x8664);
    for planted in [
        "app.manifest",
        "app.manifest.tmp",
        "APP.ICON",
        "app.icon/x",
        "App.Manifest",
    ] {
        let archive = zip(&[stored("Tool.exe", &program), stored(planted, b"x")]);
        let mut files = host_with("/media/host/tool.zip", &archive);
        assert_eq!(
            refused(&mut files, "/media/host/tool.zip", X86_64),
            Problem::ReservedName,
            "{planted}"
        );
        assert!(files.log.is_empty(), "{planted}");
    }
    let directory_first = zip(&[stored("app.manifest/", b""), stored("Tool.exe", &program)]);
    let mut files = host_with("/media/host/tool.zip", &directory_first);
    assert_eq!(
        refused(&mut files, "/media/host/tool.zip", X86_64),
        Problem::ReservedName
    );
    assert!(is_reserved(b"app.manifest") && is_reserved(b"App.Icon"));
    assert!(!is_reserved(b"bin/app.manifest") && !is_reserved(b"app.manifests"));
    let nested = zip(&[
        stored("Tool.exe", &program),
        stored("docs/app.manifest", b"x"),
    ]);
    let mut files = host_with("/media/host/tool.zip", &nested);
    let plan = planned(&mut files, "/media/host/tool.zip", X86_64);
    install(&mut files, plan, &mut [0u8; 64]).unwrap();
    assert!(manifest_of(&files, "Tool").contains("run=Tool.exe\n"));
    assert!(files.find("/Applications/Tool/docs/app.manifest").is_some());
}

#[test]
fn a_zip_larger_than_the_limit_once_extracted_is_refused() {
    let program = windows_program(0x8664);
    let mut archive = zip(&[
        stored("tool.exe", &program),
        stored("a.bin", b"a"),
        stored("b.bin", b"b"),
    ]);
    let central: Vec<usize> = archive
        .windows(4)
        .enumerate()
        .filter(|(_, window)| *window == [0x50, 0x4b, 0x01, 0x02])
        .map(|(at, _)| at)
        .collect();
    assert_eq!(central.len(), 3);
    for &at in &central[1..] {
        archive[at + 24..at + 28].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
    }
    let mut files = host_with("/media/host/big.zip", &archive);
    assert_eq!(
        refused(&mut files, "/media/host/big.zip", X86_64),
        Problem::TooLarge
    );
}

#[test]
fn a_zipped_bundle_s_program_is_the_one_its_info_plist_names() {
    let plist = b"<dict><key>CFBundleExecutable</key>\n<string>firefox</string></dict>";
    let archive = zip(&[
        stored("Firefox.app/Contents/Info.plist", plist),
        stored(
            "Firefox.app/Contents/MacOS/firefox",
            &mach_o(0x0100_000c, 2),
        ),
        stored(
            "Firefox.app/Contents/MacOS/plugin-container",
            &mach_o(0x0100_000c, 2),
        ),
        stored("Firefox.app/Contents/MacOS/libxul.dylib", b"library"),
    ]);
    let mut files = host_with("/media/host/Firefox.zip", &archive);
    let plan = planned(&mut files, "/media/host/Firefox.zip", Aarch64);
    assert_eq!(plan.name(), "Firefox");
    assert_eq!(plan.run(), b"Firefox.app/Contents/MacOS/firefox");
    let chosen = plan_entry(
        &mut files,
        b"/media/host/Firefox.zip",
        None,
        Some(b"Firefox.app"),
        Aarch64,
    )
    .unwrap();
    assert_eq!(chosen.run(), b"Firefox.app/Contents/MacOS/firefox");
    let mut entries = [Path::new(); 4];
    assert_eq!(
        archive_programs(&mut files, b"/media/host/Firefox.zip", &mut entries),
        (1, 0)
    );
    assert_eq!(entries[0].as_bytes(), b"Firefox.app");
}

#[test]
fn the_programs_of_a_zip_are_listed_for_the_window_s_chooser() {
    let program = windows_program(0x8664);
    let archive = zip(&[
        stored("bin/a.exe", &program),
        stored("bin/notes.txt", b"text"),
        stored("bin/b.exe", &program),
        stored(&format!("{}.exe", "x".repeat(70)), &program),
        stored("c.exe", &program),
    ]);
    let mut files = host_with("/media/host/tools.zip", &archive);
    let mut two = [Path::new(); 2];
    assert_eq!(
        archive_programs(&mut files, b"/media/host/tools.zip", &mut two),
        (2, 2)
    );
    assert_eq!(two[0].as_bytes(), b"bin/a.exe");
    assert_eq!(two[1].as_bytes(), b"bin/b.exe");
    let mut four = [Path::new(); 4];
    assert_eq!(
        archive_programs(&mut files, b"/media/host/tools.zip", &mut four),
        (4, 0)
    );
    assert_eq!(
        four[2].as_bytes(),
        format!("{}.exe", "x".repeat(70)).as_bytes()
    );
    assert_eq!(four[3].as_bytes(), b"c.exe");
    let mut none = [Path::new(); 2];
    assert_eq!(
        archive_programs(&mut files, b"/media/host/missing.zip", &mut none),
        (0, 0)
    );
}

#[test]
fn a_zip_s_programs_with_paths_longer_than_a_name_are_listed_whole_and_install() {
    let program = windows_program(0x8664);
    let folder = "SomeVendorTool-12.3.4-windows-x64-portable-build-2026/bin";
    let tool = format!("{folder}/Tool.exe");
    let updater = format!("{folder}/Updater.exe");
    assert!(tool.len() > Name::CAPACITY);
    let archive = zip(&[stored(&tool, &program), stored(&updater, &program)]);
    let mut files = host_with("/media/host/tool.zip", &archive);
    assert!(matches!(
        refused(&mut files, "/media/host/tool.zip", X86_64),
        Problem::SeveralPrograms(_)
    ));
    let mut entries = [Path::new(); 4];
    assert_eq!(
        archive_programs(&mut files, b"/media/host/tool.zip", &mut entries),
        (2, 0)
    );
    assert_eq!(entries[0].as_bytes(), tool.as_bytes());
    assert_eq!(entries[1].as_bytes(), updater.as_bytes());
    let plan = plan_entry(
        &mut files,
        b"/media/host/tool.zip",
        None,
        Some(entries[1].as_bytes()),
        X86_64,
    )
    .unwrap();
    assert_eq!(plan.name(), "Updater");
    assert_eq!(plan.run(), updater.as_bytes());
}

#[test]
fn a_program_named_like_a_file_the_installer_writes_is_refused() {
    for name in [
        "app.manifest",
        "app.manifest.tmp",
        "app.icon",
        "App.Manifest",
    ] {
        let path = format!("/media/host/{name}");
        let mut files = host_with(&path, &linux_program(62));
        assert_eq!(
            refused(&mut files, &path, X86_64),
            Problem::ReservedName,
            "{name}"
        );
        assert!(files.log.is_empty(), "{name}");
    }
    let mut files = host_with("/media/host/app.manifest", &linux_program(62));
    let named = plan(
        &mut files,
        b"/media/host/app.manifest",
        Some(b"Tool"),
        X86_64,
    )
    .unwrap_err();
    assert_eq!(named.problem, Problem::ReservedName);
    assert!(files.paths_under("/Applications/").is_empty());
}

#[test]
fn a_bundle_s_program_is_found_alike_in_a_folder_and_in_a_zip() {
    let declares = |executable: &str| {
        format!("<dict><key>CFBundleExecutable</key><string>{executable}</string></dict>")
    };
    let arm = mach_o(0x0100_000c, 2);
    let mut files = FakeFiles::new();
    files.directory("/media/host/Firefox.app");
    files.directory("/media/host/Firefox.app/Contents");
    files.file(
        "/media/host/Firefox.app/Contents/Info.plist",
        declares("firefox").as_bytes(),
    );
    files.directory("/media/host/Firefox.app/Contents/MacOS");
    files.file("/media/host/Firefox.app/Contents/MacOS/firefox", &arm);
    files.file(
        "/media/host/Firefox.app/Contents/MacOS/plugin-container",
        &arm,
    );
    assert_eq!(
        planned(&mut files, "/media/host/Firefox.app", Aarch64).run(),
        b"Firefox.app/Contents/MacOS/firefox"
    );
    files.directory("/media/host/Paint.app");
    files.directory("/media/host/Paint.app/Contents");
    files.file(
        "/media/host/Paint.app/Contents/Info.plist",
        declares("gone").as_bytes(),
    );
    files.directory("/media/host/Paint.app/Contents/MacOS");
    files.file("/media/host/Paint.app/Contents/MacOS/Paint", &arm);
    files.file("/media/host/Paint.app/Contents/MacOS/helper", &arm);
    assert_eq!(
        planned(&mut files, "/media/host/Paint.app", Aarch64).run(),
        b"Paint.app/Contents/MacOS/Paint"
    );

    let zipped_paint = zip(&[
        stored("Paint.app/Contents/Info.plist", declares("gone").as_bytes()),
        stored("Paint.app/Contents/MacOS/Paint", &arm),
        stored("Paint.app/Contents/MacOS/helper", &arm),
    ]);
    let mut files = host_with("/media/host/Paint.zip", &zipped_paint);
    assert_eq!(
        planned(&mut files, "/media/host/Paint.zip", Aarch64).run(),
        b"Paint.app/Contents/MacOS/Paint"
    );
    let zipped_only = zip(&[stored("Big Tool.app/Contents/MacOS/bigtool", &arm)]);
    let mut files = host_with("/media/host/big.zip", &zipped_only);
    let plan = planned(&mut files, "/media/host/big.zip", Aarch64);
    assert_eq!(plan.name(), "Big Tool");
    assert_eq!(plan.run(), b"Big Tool.app/Contents/MacOS/bigtool");
    let zipped_two = zip(&[
        stored("X.app/Contents/MacOS/a", &arm),
        stored("X.app/Contents/MacOS/b", &arm),
    ]);
    let mut files = host_with("/media/host/x.zip", &zipped_two);
    assert_eq!(
        refused(&mut files, "/media/host/x.zip", Aarch64),
        Problem::NoProgramInArchive
    );
}

#[test]
fn a_damaged_entry_fails_the_install_and_leaves_nothing() {
    let program = windows_program(0x8664);
    let mut zip = zip(&[
        stored("paint.exe", &program),
        stored("data.bin", b"0123456789"),
    ]);
    let data = zip
        .windows(10)
        .position(|window| window == b"0123456789")
        .unwrap();
    zip[data] ^= 1;
    let mut files = host_with("/media/host/paint.zip", &zip);
    let plan = planned(&mut files, "/media/host/paint.zip", X86_64);
    assert_eq!(
        install(&mut files, plan, &mut [0u8; 64]),
        Err(Problem::Archive(crate::archive::ArchiveError::Corrupt))
    );
    assert!(files.paths_under("/Applications/").is_empty());
    assert_eq!(files.open_count(), 0);
}

fn replace_file(files: &mut FakeFiles, path: &str, bytes: &[u8]) {
    files
        .nodes
        .retain(|(at, _)| at.as_slice() != path.as_bytes());
    files.file(path, bytes);
}

#[test]
fn extracting_checks_reserved_names_and_the_size_again() {
    let program = windows_program(0x8664);
    assert!(program.len() > 16);
    let clean = zip(&[stored("tool.exe", &program), stored("readme.x", b"x")]);
    let planted = zip(&[stored("tool.exe", &program), stored("app.icon", b"x")]);
    assert_eq!(clean.len(), planted.len());
    let mut files = host_with("/media/host/tool.zip", &clean);
    let plan = planned(&mut files, "/media/host/tool.zip", X86_64);
    replace_file(&mut files, "/media/host/tool.zip", &planted);
    assert_eq!(
        install(&mut files, plan, &mut [0u8; 64]),
        Err(Problem::ReservedName)
    );
    assert!(files.paths_under("/Applications/").is_empty());
    assert_eq!(files.open_count(), 0);

    let mut grown = clean.clone();
    let central = grown
        .windows(4)
        .rposition(|window| window == [0x50, 0x4b, 0x01, 0x02])
        .unwrap();
    grown[central + 24..central + 28].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
    let mut files = host_with("/media/host/tool.zip", &clean);
    let plan = planned(&mut files, "/media/host/tool.zip", X86_64);
    replace_file(&mut files, "/media/host/tool.zip", &grown);
    assert_eq!(
        install(&mut files, plan, &mut [0u8; 64]),
        Err(Problem::TooLarge)
    );
    assert!(files.paths_under("/Applications/").is_empty());
    assert_eq!(files.open_count(), 0);
}

fn with_apps(names: &[&str]) -> FakeFiles {
    let mut files = FakeFiles::new();
    for name in names {
        files.directory(&format!("/Applications/{name}"));
        files.file(
            &format!("/Applications/{name}/app.manifest"),
            format!("eggos-app 1\nname={name}\nsource=linux\nrun={name}\n").as_bytes(),
        );
    }
    files
}

fn page_of(
    files: &mut FakeFiles,
    limit: Option<usize>,
    after: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut names = Vec::new();
    let next = for_each_installed_page(files, limit, after.map(str::as_bytes), |app| {
        names.push(app.name.to_owned())
    })
    .unwrap();
    (names, next.map(|name| name.as_str().to_owned()))
}

#[test]
fn installed_apps_page_by_name_in_the_folder_s_order() {
    let mut files = with_apps(&["pear", "apple", "fig", "kiwi"]);
    assert_eq!(
        page_of(&mut files, Some(2), None),
        (
            vec!["apple".to_owned(), "fig".to_owned()],
            Some("fig".to_owned())
        )
    );
    assert_eq!(
        page_of(&mut files, Some(2), Some("fig")),
        (vec!["pear".to_owned(), "kiwi".to_owned()], None)
    );
    assert_eq!(
        page_of(&mut files, None, Some("kiwi")),
        (vec!["pear".to_owned()], None)
    );
    assert_eq!(
        page_of(&mut files, Some(9), Some("pear")),
        (Vec::new(), None)
    );
    assert_eq!(page_of(&mut files, None, None).0.len(), 4);
}

#[test]
fn a_dynamically_linked_program_installs_with_the_loader_and_libraries_its_zip_holds() {
    let program = dynamic_program(62, b"/lib/ld-musl-x86_64.so.1");
    let archive = zip(&[
        stored("brogue", &program),
        stored("lib/ld-musl-x86_64.so.1", &linux_program(62)),
        stored("lib/libSDL3.so.0", b"library"),
        stored("share/brogue/keymap.txt", b"keys"),
    ]);
    let mut files = host_with("/media/host/brogue.zip", &archive);
    let plan = planned(&mut files, "/media/host/brogue.zip", X86_64)
        .with_arguments(b"-n --data-dir /Applications/brogue/share/brogue")
        .unwrap();
    assert_eq!(plan.name(), "brogue");
    assert_eq!(
        plan.loader().map(|loader| loader.path.as_bytes()),
        Some(&b"lib/ld-musl-x86_64.so.1"[..])
    );
    install(&mut files, plan, &mut [0u8; 64]).unwrap();
    let manifest = manifest_of(&files, "brogue");
    assert!(
        manifest.contains(
            "run=brogue\narchitecture=x86_64\nloader=lib/ld-musl-x86_64.so.1\nlibraries=lib\n\
             arguments=-n --data-dir /Applications/brogue/share/brogue\n"
        ),
        "{manifest}"
    );
    let alone = zip(&[
        stored("brogue", &program),
        stored("lib/libSDL3.so.0", b"library"),
    ]);
    let mut files = host_with("/media/host/brogue.zip", &alone);
    assert_eq!(
        refused(&mut files, "/media/host/brogue.zip", X86_64),
        Problem::NotRunnable(NotRunnable::NeedsDynamicLoader)
    );
    let mut files = host_with("/media/host/brogue.zip", &archive);
    let unsafe_words = planned(&mut files, "/media/host/brogue.zip", X86_64)
        .with_arguments(b"-n; reboot")
        .unwrap_err();
    assert_eq!(unsafe_words.problem, Problem::InvalidArguments);
    let mut files = host_with("/media/host/brogue", &program);
    assert_eq!(
        refused(&mut files, "/media/host/brogue", X86_64),
        Problem::NotRunnable(NotRunnable::NeedsDynamicLoader)
    );
}
