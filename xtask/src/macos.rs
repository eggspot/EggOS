//! macOS console programs (D120): Mach-O executables, thin or fat, that the macOS layer at
//! `/usr/lib/eggos/macos` runs on aarch64 and x86_64. An image gets the layer only when an added
//! file is a Mach-O or fat file. The boot test's programs are built from `tests/macos-console`
//! by Apple's linker, so only on a macOS host; CI builds them on its macOS runner and hands them
//! to the Linux job through `EGGOS_MACOS_PROGRAMS`. A fat file of both slices is written here.
//! The Objective-C program, `tests/macos-objc/main.m`, is test data our own source and Apple's
//! clang build the same way (D142); it is never shipped. So is the AppKit test app `mac-paint`,
//! `tests/macos-gui/main.m` (D147), which the macOS GUI boot test installs from
//! `mac-paint.zip` (D148), the `mac-paint.app` bundle `assemble_app` writes with the icon
//! `test_icns` draws, archived by `archive_app`.

use crate::qmp::Picture;
use std::path::{Path, PathBuf};

pub const LAYER_PACKAGE: &str = "eggos-macos";
pub const LAYER_PATH: &str = "usr/lib/eggos/macos";
pub const CONSOLE_PACKAGE: &str = "eggos-macos-console";
pub const PROGRAMS_VARIABLE: &str = "EGGOS_MACOS_PROGRAMS";
pub const DEPLOYMENT_TARGET: &str = "13.0";
pub const PROGRAMS: [&str; 4] = [
    "macos-console",
    "macos-unsupported",
    "macos-tools",
    "macos-std",
];

/// A program built from Objective-C source with Apple's clang: `tests/<folder>/main.m` linked
/// with `frameworks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjcProgram {
    pub name: &'static str,
    pub folder: &'static str,
    pub frameworks: &'static [&'static str],
}

/// The AppKit test app's name: its program, and its bundle's folder without `.app`.
pub const GUI_APP: &str = "mac-paint";

/// The programs built from Objective-C sources with Apple's clang.
pub const OBJC_PROGRAMS: [ObjcProgram; 2] = [
    ObjcProgram {
        name: "macos-objc",
        folder: "macos-objc",
        frameworks: &["Foundation"],
    },
    ObjcProgram {
        name: GUI_APP,
        folder: "macos-gui",
        frameworks: &["AppKit", "Foundation"],
    },
];

const BUNDLE_SUFFIX: &str = ".app";
/// The bundle's property list, beside an app's `main.m` and in its bundle's `Contents`.
pub const INFO_PLIST: &str = "Info.plist";
/// The test app's icon file in `Contents/Resources`, which its `Info.plist` names.
pub const APP_ICON: &str = "AppIcon.icns";
/// What the test app's archive is called after its name.
pub const ARCHIVE_SUFFIX: &str = ".zip";

const ICON_DESIGN_SIDE: u32 = 128;
const ICON_SIDES: [(&[u8; 4], u32); 2] = [(b"ic07", 128), (b"icp5", 32)];
const ICON_BACKGROUND: u32 = 0x0020_60c0;
const ICON_DISC: u32 = 0x00f0_a020;
const ICON_STRIPE: u32 = 0x00ff_ffff;
const ICON_DISC_RADIUS: i64 = 40;
const ICON_STRIPE_HALF_WIDTH: u32 = 10;

fn test_icon_pixel(x: u32, y: u32) -> u32 {
    let centre = i64::from(ICON_DESIGN_SIDE / 2);
    let (dx, dy) = (i64::from(x) - centre, i64::from(y) - centre);
    let in_disc = dx * dx + dy * dy < ICON_DISC_RADIUS * ICON_DISC_RADIUS;
    match (x.abs_diff(y) < ICON_STRIPE_HALF_WIDTH, in_disc) {
        (true, _) => ICON_STRIPE,
        (false, true) => ICON_DISC,
        (false, false) => ICON_BACKGROUND,
    }
}

/// The test app's icon at `side` pixels as a PNG: a blue square, an amber disc and a white
/// diagonal, drawn on a 128-pixel grid.
pub fn test_icon_png(side: u32) -> Result<Vec<u8>, String> {
    let pixels = (0..side * side)
        .map(|index| {
            let (x, y) = (index % side, index / side);
            test_icon_pixel(x * ICON_DESIGN_SIDE / side, y * ICON_DESIGN_SIDE / side)
        })
        .collect();
    crate::release::encode_png(&Picture {
        width: side,
        height: side,
        pixels,
    })
}

/// The test app's `AppIcon.icns`: a 128-pixel `ic07` and a 32-pixel `icp5`, both PNG.
pub fn test_icns() -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    for (kind, side) in ICON_SIDES {
        let png = test_icon_png(side)?;
        body.extend_from_slice(kind);
        body.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(&png);
    }
    let mut file = b"icns".to_vec();
    file.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    file.extend_from_slice(&body);
    Ok(file)
}

/// Writes `test_icns` into the bundle at `bundle` as `Contents/Resources/AppIcon.icns`.
pub fn add_test_icon(bundle: &Path) -> Result<(), String> {
    let resources = bundle.join("Contents").join("Resources");
    std::fs::create_dir_all(&resources).map_err(|e| format!("{}: {e}", resources.display()))?;
    let path = resources.join(APP_ICON);
    std::fs::write(&path, test_icns()?).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes the bundle at `bundle` (`<name>.app`) into `<name>.zip` beside it, every file stored;
/// returns the archive's path.
pub fn archive_app(bundle: &Path) -> Result<PathBuf, String> {
    let file_name = bundle
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} has no name", bundle.display()))?;
    let stem = file_name.strip_suffix(BUNDLE_SUFFIX).unwrap_or(&file_name);
    let archive = bundle.with_file_name(format!("{stem}{ARCHIVE_SUFFIX}"));
    let members = crate::zip::folder_members(bundle, &file_name)?;
    std::fs::write(&archive, crate::zip::stored(&members)?)
        .map_err(|e| format!("{}: {e}", archive.display()))?;
    Ok(archive)
}

const MH_MAGIC_64: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const MAX_FAT_ARCHITECTURES: u32 = 32;
const FAT_ALIGNMENT_SHIFT: u32 = 14;

/// The macOS CPU name (`arm64`, `x86_64`) and Rust target of the architecture named `arch`, if
/// macOS has one.
pub fn target(arch: &str) -> Option<(&'static str, &'static str)> {
    match arch {
        "aarch64" => Some(("arm64", "aarch64-apple-darwin")),
        "x86_64" => Some(("x86_64", "x86_64-apple-darwin")),
        _ => None,
    }
}

/// The QEMU arguments the macOS programs boot test adds on `arch`: arm64 macOS programs are
/// compiled for Apple's M1 and later (ARMv8.4-A and more: LSE atomics, RCPC loads), which the
/// other boot tests' Cortex-A72 (ARMv8.0-A) lacks, so aarch64 boots on QEMU's `max` CPU; a
/// later `-cpu` replaces the machine's.
pub fn qemu_cpu_args(arch: &str) -> &'static [&'static str] {
    match arch {
        "aarch64" => &["-cpu", "max"],
        _ => &[],
    }
}

/// Whether the layer runs on the architecture named `arch`.
pub fn layer_runs_on(arch: &str) -> bool {
    target(arch).is_some()
}

/// Whether `bytes` start as a 64-bit Mach-O file or a fat file (not a Java class file, whose
/// second word is a version of at least 45).
pub fn is_mach_o(bytes: &[u8]) -> bool {
    let word = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(
            bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    bytes.starts_with(&MH_MAGIC_64)
        || (matches!(word(0), Some(FAT_MAGIC | FAT_MAGIC_64))
            && word(4).is_some_and(|count| (1..=MAX_FAT_ARCHITECTURES).contains(&count)))
}

pub fn is_program(path: &str) -> bool {
    let mut start = [0u8; 8];
    std::fs::File::open(path)
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut start))
        .is_ok_and(|()| is_mach_o(&start))
}

fn cputype(thin: &[u8]) -> Option<u32> {
    if !thin.starts_with(&MH_MAGIC_64) {
        return None;
    }
    Some(u32::from_le_bytes(thin.get(4..8)?.try_into().ok()?))
}

/// A fat file holding each of `slices` (thin 64-bit Mach-O files), each aligned to 16 KiB as
/// `lipo` aligns arm64 and x86_64 slices.
pub fn fat(slices: &[&[u8]]) -> Result<Vec<u8>, String> {
    let alignment = 1usize << FAT_ALIGNMENT_SHIFT;
    let mut header = Vec::new();
    header.extend_from_slice(&FAT_MAGIC.to_be_bytes());
    header.extend_from_slice(&(slices.len() as u32).to_be_bytes());
    let mut offset = (8 + 20 * slices.len()).next_multiple_of(alignment);
    let mut body = Vec::new();
    for slice in slices {
        let cputype = cputype(slice).ok_or("a fat slice that is not a 64-bit Mach-O file")?;
        let fields = [
            cputype,
            0,
            u32::try_from(offset).map_err(|_| "a fat file over 4 GiB")?,
            u32::try_from(slice.len()).map_err(|_| "a fat slice over 4 GiB")?,
            FAT_ALIGNMENT_SHIFT,
        ];
        for field in fields {
            header.extend_from_slice(&field.to_be_bytes());
        }
        body.resize(
            offset - (8 + 20 * slices.len()).next_multiple_of(alignment),
            0,
        );
        body.extend_from_slice(slice);
        offset = (offset + slice.len()).next_multiple_of(alignment);
    }
    header.resize((8 + 20 * slices.len()).next_multiple_of(alignment), 0);
    header.extend(body);
    Ok(header)
}

/// Where the boot test's programs are: `<dir>/<program>-<cpu>` for each of [`PROGRAMS`] and
/// both CPUs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programs {
    pub directory: PathBuf,
}

impl Programs {
    pub fn path(&self, program: &str, cpu: &str) -> PathBuf {
        self.directory.join(format!("{program}-{cpu}"))
    }
}

/// Whether `EGGOS_MACOS_PROGRAMS` is set but empty: CI built no programs for this run (only
/// pull requests that change code build them), so the boot test is skipped rather than failed.
pub fn not_built_for_this_run(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value.is_empty())
}

/// The boot test's programs: those in `EGGOS_MACOS_PROGRAMS` if it is set, else built here on a
/// macOS host with Apple's linker; `None` on other hosts.
pub fn find_or_build_programs(target_directory: &Path) -> Result<Option<Programs>, String> {
    if let Some(directory) = std::env::var_os(PROGRAMS_VARIABLE) {
        let programs = Programs {
            directory: PathBuf::from(directory),
        };
        let objc_names = OBJC_PROGRAMS.into_iter().map(|program| program.name);
        for program in PROGRAMS.into_iter().chain(objc_names) {
            for cpu in ["arm64", "x86_64"] {
                let path = programs.path(program, cpu);
                if !path.is_file() {
                    return Err(format!(
                        "{PROGRAMS_VARIABLE}: {} is missing",
                        path.display()
                    ));
                }
            }
        }
        return Ok(Some(programs));
    }
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let directory = target_directory.join("macos-console").join("programs");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    for arch in ["aarch64", "x86_64"] {
        let (cpu, target) = target(arch).ok_or("no macOS target")?;
        build_programs(target_directory, target)?;
        let release = target_directory
            .join("macos-console")
            .join(target)
            .join("release");
        for program in PROGRAMS {
            let to = directory.join(format!("{program}-{cpu}"));
            std::fs::copy(release.join(program), &to)
                .map_err(|e| format!("{}: {e}", to.display()))?;
        }
        for program in OBJC_PROGRAMS {
            let output = directory.join(format!("{}-{cpu}", program.name));
            build_objc_program(program, cpu, &output)?;
        }
    }
    Ok(Some(Programs { directory }))
}

/// The `clang` arguments that build `source` for `cpu` into `output` linked with `frameworks`:
/// ARC, chained fixups (macOS 13 and later), optimized.
pub fn objc_arguments(source: &Path, frameworks: &[&str], cpu: &str, output: &Path) -> Vec<String> {
    let mut arguments = vec!["clang".to_owned(), "-fobjc-arc".to_owned()];
    for framework in frameworks {
        arguments.push("-framework".to_owned());
        arguments.push((*framework).to_owned());
    }
    arguments.push(format!("-mmacosx-version-min={DEPLOYMENT_TARGET}"));
    arguments.extend(
        ["-arch", cpu, "-O2", "-Wall", "-Werror", "-o"]
            .iter()
            .map(|argument| argument.to_string()),
    );
    arguments.push(output.to_string_lossy().into_owned());
    arguments.push(source.to_string_lossy().into_owned());
    arguments
}

/// The folder holding `program`'s `main.m` (and an app's `Info.plist`).
pub fn source_folder(program: &ObjcProgram) -> PathBuf {
    crate::workspace_root().join("tests").join(program.folder)
}

/// The Objective-C program named `name`.
pub fn objc_program(name: &str) -> Option<ObjcProgram> {
    OBJC_PROGRAMS
        .into_iter()
        .find(|program| program.name == name)
}

/// Writes the `.app` bundle of the app `name` in `folder`: `<name>.app/Contents/Info.plist`
/// copied from `info_plist` and `<name>.app/Contents/MacOS/<name>` from `program`, replacing
/// what an earlier run wrote there. Returns the bundle's path.
pub fn assemble_app(
    folder: &Path,
    name: &str,
    info_plist: &Path,
    program: &Path,
) -> Result<PathBuf, String> {
    let bundle = folder.join(format!("{name}{BUNDLE_SUFFIX}"));
    let contents = bundle.join("Contents");
    let programs = contents.join("MacOS");
    std::fs::create_dir_all(&programs).map_err(|e| format!("{}: {e}", programs.display()))?;
    for (from, to) in [
        (info_plist, contents.join(INFO_PLIST)),
        (program, programs.join(name)),
    ] {
        std::fs::copy(from, &to)
            .map_err(|e| format!("{} to {}: {e}", from.display(), to.display()))?;
    }
    Ok(bundle)
}

fn build_objc_program(program: ObjcProgram, cpu: &str, output: &Path) -> Result<(), String> {
    let source = source_folder(&program).join("main.m");
    let arguments = objc_arguments(&source, program.frameworks, cpu, output);
    let status = std::process::Command::new("xcrun")
        .args(&arguments)
        .status()
        .map_err(|e| format!("xcrun clang: {e}"))?;
    if !status.success() {
        return Err(format!("xcrun clang {}: {status}", source.display()));
    }
    Ok(())
}

/// The environment `cargo build --target <target> -p eggos-macos-console --release` needs to
/// link programs with chained fixups (macOS 13 and later) into `target/macos-console`.
pub fn build_environment(target_directory: &Path) -> [(&'static str, String); 3] {
    [
        ("MACOSX_DEPLOYMENT_TARGET", DEPLOYMENT_TARGET.to_owned()),
        ("CARGO_ENCODED_RUSTFLAGS", String::new()),
        (
            "CARGO_TARGET_DIR",
            target_directory
                .join("macos-console")
                .to_string_lossy()
                .into_owned(),
        ),
    ]
}

fn build_programs(target_directory: &Path, target: &str) -> Result<(), String> {
    let environment = build_environment(target_directory);
    let environment: Vec<(&str, &str)> = environment
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    crate::cargo_with_env(
        &[
            "build",
            "-p",
            CONSOLE_PACKAGE,
            "--target",
            target,
            "--release",
        ],
        &environment,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CPU_TYPE_X86_64: u32 = 0x0100_0007;
    const CPU_TYPE_ARM64: u32 = 0x0100_000c;

    fn thin(cputype: u32, length: usize) -> Vec<u8> {
        let mut bytes = MH_MAGIC_64.to_vec();
        bytes.extend_from_slice(&cputype.to_le_bytes());
        bytes.resize(length, 0x5a);
        bytes
    }

    #[test]
    fn mach_o_and_fat_files_are_recognised_but_not_java_classes() {
        assert!(is_mach_o(&thin(CPU_TYPE_ARM64, 32)));
        assert!(is_mach_o(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]));
        assert!(!is_mach_o(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0x41]));
        assert!(!is_mach_o(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0]));
        assert!(!is_mach_o(b"\x7fELF\x02\x01\x01\0"));
        assert!(!is_mach_o(b"MZ"));
        assert!(!is_mach_o(&[0xca, 0xfe]));
    }

    #[test]
    fn a_fat_file_lists_each_slice_at_a_16_kib_boundary() {
        let arm = thin(CPU_TYPE_ARM64, 100);
        let intel = thin(CPU_TYPE_X86_64, 0x4001);
        let file = fat(&[&arm, &intel]).unwrap();
        let word = |at: usize| u32::from_be_bytes(file[at..at + 4].try_into().unwrap());
        assert_eq!((word(0), word(4)), (FAT_MAGIC, 2));
        assert_eq!(
            [word(8), word(16), word(20), word(24)],
            [CPU_TYPE_ARM64, 0x4000, 100, 14]
        );
        assert_eq!(
            [word(28), word(36), word(40)],
            [CPU_TYPE_X86_64, 0x8000, 0x4001]
        );
        assert_eq!(&file[0x4000..0x4000 + 100], &arm[..]);
        assert_eq!(&file[0x8000..], &intel[..]);
        assert!(is_mach_o(&file));
        assert!(fat(&[b"not mach-o"]).is_err());
    }

    #[test]
    fn programs_have_one_file_per_program_and_cpu() {
        let programs = Programs {
            directory: PathBuf::from("out"),
        };
        assert_eq!(
            programs.path("macos-console", "arm64"),
            Path::new("out").join("macos-console-arm64")
        );
        assert_eq!(target("riscv64"), None);
        assert!(layer_runs_on("aarch64") && layer_runs_on("x86_64") && !layer_runs_on("riscv64"));
        assert!(not_built_for_this_run(Some(std::ffi::OsStr::new(""))));
        assert!(!not_built_for_this_run(Some(std::ffi::OsStr::new("out"))));
        assert!(!not_built_for_this_run(None));
    }

    #[test]
    fn objective_c_programs_build_with_arc_their_frameworks_and_chained_fixups() {
        let arguments = objc_arguments(
            Path::new("main.m"),
            &["AppKit", "Foundation"],
            "arm64",
            Path::new("out"),
        );
        assert_eq!(arguments[0], "clang");
        assert!(arguments.contains(&"-fobjc-arc".to_owned()));
        assert!(arguments.contains(&"-mmacosx-version-min=13.0".to_owned()));
        assert!(arguments.windows(2).any(|pair| pair == ["-arch", "arm64"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-framework", "AppKit"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["-framework", "Foundation"])
        );
        assert!(arguments.contains(&"-Werror".to_owned()));
        assert_eq!(&arguments[arguments.len() - 2..], ["out", "main.m"]);
        let names: Vec<&str> = OBJC_PROGRAMS.iter().map(|program| program.name).collect();
        assert_eq!(names, ["macos-objc", "mac-paint"]);
        assert_eq!(
            objc_program("macos-objc").map(|program| program.frameworks),
            Some(&["Foundation"][..])
        );
        let app = objc_program(GUI_APP).expect("mac-paint is an Objective-C program");
        assert_eq!(app.frameworks, ["AppKit", "Foundation"]);
        assert!(source_folder(&app).ends_with(Path::new("tests").join("macos-gui")));
        assert!(source_folder(&app).join("main.m").is_file());
        assert!(source_folder(&app).join(INFO_PLIST).is_file());
    }

    #[test]
    fn an_app_bundle_holds_its_info_plist_and_its_program_under_contents() {
        let dir =
            std::env::temp_dir().join(format!("eggos-app-bundle-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plist = dir.join("source.plist");
        let program = dir.join("program");
        std::fs::write(&plist, b"<plist/>").unwrap();
        std::fs::write(&program, thin(CPU_TYPE_ARM64, 64)).unwrap();
        let folder = dir.join("share");
        let bundle = assemble_app(&folder, GUI_APP, &plist, &program).unwrap();
        assert_eq!(bundle, folder.join("mac-paint.app"));
        let again = assemble_app(&folder, GUI_APP, &plist, &program).unwrap();
        assert_eq!(again, bundle);
        let contents = bundle.join("Contents");
        assert_eq!(
            std::fs::read(contents.join("Info.plist")).unwrap(),
            b"<plist/>"
        );
        let installed = std::fs::read(contents.join("MacOS").join("mac-paint")).unwrap();
        assert!(is_mach_o(&installed));
        let mut entries: Vec<String> = std::fs::read_dir(&contents)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        assert_eq!(entries, ["Info.plist", "MacOS"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
