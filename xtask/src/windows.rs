//! Windows programs built with Microsoft's C compiler (D146, D177): `tests/windows-crt/eggpad.c`,
//! our own C notepad, and `tests/windows-crt/eggcpp.cpp`, our own C++ program that throws and
//! catches C++ exceptions and handles faults with `__try`, are test data that MSVC's `cl.exe`
//! builds as `/MD` programs (on the C runtime's DLLs) for x64 and ARM64; `eggcpp-fh3` is
//! `eggcpp.cpp` built for `__CxxFrameHandler3` on x64 (`/d2FH4-`), whose default is
//! `__CxxFrameHandler4`; `tests/windows-crt/eggspawn.c`, our own C program that starts copies of
//! itself through `CreateProcessW` and `CreateProcessA` and reads one's output through a pipe
//! (D189), and `tests/windows-crt/eggsync.c`, our own C program for APCs, abandoned mutexes,
//! ordinal imports and DOS wildcards (D189), build as console programs for both.
//! `tests/windows-rust`, our own Rust program on Rust's standard library (`eggrust`, D189),
//! builds with `cargo` for `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`, linked by
//! MSVC's linker; and `tests/windows-mingw/eggmingw.c`, our own C program on MinGW's `msvcrt.dll` (D189), builds
//! with llvm-mingw's msvcrt toolchain (`x86_64-w64-mingw32-clang`, `aarch64-w64-mingw32-clang`,
//! on `PATH` or in `EGGOS_LLVM_MINGW`). Only a
//! Windows host with Visual Studio's C++ tools can build them, found through `vswhere`; CI
//! builds them on its Windows runner and hands them to the Linux job through
//! `EGGOS_WINDOWS_PROGRAMS`. They are never shipped.

use std::path::{Path, PathBuf};

pub const PROGRAMS_VARIABLE: &str = "EGGOS_WINDOWS_PROGRAMS";
pub const PROGRAMS: [&str; 5] = ["eggpad", "eggcpp", "eggcpp-fh3", "eggspawn", "eggsync"];
/// The test programs built from our own Rust source on Rust's standard library (D189).
pub const RUST_PROGRAMS: [&str; 1] = ["eggrust"];
/// The test programs built from our own C source by llvm-mingw against MinGW's `msvcrt.dll`
/// (D189).
pub const MINGW_PROGRAMS: [&str; 1] = ["eggmingw"];
/// The folder of the llvm-mingw toolchain that builds [`MINGW_PROGRAMS`], when its compilers are
/// not on `PATH`.
pub const LLVM_MINGW_VARIABLE: &str = "EGGOS_LLVM_MINGW";
const MINGW_SOURCE_FOLDER: &str = "windows-mingw";
const RUST_PACKAGE: &str = "windows-rust";
const TARGETS: [&str; 2] = ["x64", "arm64"];
const VISUAL_STUDIO_COMPONENT: &str = "Microsoft.VisualStudio.Component.VC.Tools.x86.x64";

/// MSVC's name (`x64`, `arm64`) for the architecture named `arch`, if the Windows layer runs
/// programs there.
pub fn msvc_target(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("x64"),
        "aarch64" => Some("arm64"),
        _ => None,
    }
}

/// Where the boot test's programs are: `<dir>/<program>-<target>.exe` for each of
/// [`PROGRAMS`] and both targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programs {
    pub directory: PathBuf,
}

impl Programs {
    pub fn path(&self, program: &str, target: &str) -> PathBuf {
        self.directory.join(format!("{program}-{target}.exe"))
    }
}

/// Whether `EGGOS_WINDOWS_PROGRAMS` is set but empty: CI built no programs for this run (only
/// pull requests that change code build them), so the boot test is skipped rather than failed.
pub fn not_built_for_this_run(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value.is_empty())
}

/// The boot test's programs: those in `EGGOS_WINDOWS_PROGRAMS` if it is set, else built here on
/// a Windows host with MSVC; `None` on other hosts.
pub fn find_or_build_programs(target_directory: &Path) -> Result<Option<Programs>, String> {
    if let Some(directory) = std::env::var_os(PROGRAMS_VARIABLE) {
        let programs = Programs {
            directory: PathBuf::from(directory),
        };
        crate::ci_artifact::fetch_when_built("windows-programs", &programs.directory)?;
        for program in PROGRAMS.iter().chain(&RUST_PROGRAMS).chain(&MINGW_PROGRAMS) {
            for target in TARGETS {
                let path = programs.path(program, target);
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
    build_programs(target_directory)
}

fn build_programs(target_directory: &Path) -> Result<Option<Programs>, String> {
    if !cfg!(windows) {
        return Ok(None);
    }
    let directory = target_directory.join("windows-crt").join("programs");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let installation = visual_studio()?;
    for target in TARGETS {
        let environment = developer_environment(&installation, target)?;
        let compiler = find_in_path(&environment, "cl.exe")
            .ok_or_else(|| format!("MSVC's cl.exe is not on the {target} tools' PATH"))?;
        for program in PROGRAMS {
            let objects = target_directory
                .join("windows-crt")
                .join(format!("objects-{target}"));
            std::fs::create_dir_all(&objects).map_err(|e| format!("{}: {e}", objects.display()))?;
            let source = crate::workspace_root()
                .join("tests")
                .join("windows-crt")
                .join(source_file(program));
            let output = Programs {
                directory: directory.clone(),
            }
            .path(program, target);
            let status = std::process::Command::new(&compiler)
                .args(compiler_arguments(
                    program, target, &source, &output, &objects,
                ))
                .env_clear()
                .envs(environment.iter().map(|(name, value)| (name, value)))
                .status()
                .map_err(|e| format!("{}: {e}", compiler.display()))?;
            if !status.success() {
                return Err(format!("cl.exe {}: {status}", source.display()));
            }
        }
        build_rust_programs(target, target_directory, &directory)?;
        build_mingw_programs(target, &directory)?;
    }
    Ok(Some(Programs { directory }))
}

fn build_rust_programs(
    target: &str,
    target_directory: &Path,
    directory: &Path,
) -> Result<(), String> {
    let manifest = crate::workspace_root()
        .join("tests")
        .join(RUST_PACKAGE)
        .join("Cargo.toml");
    let build_directory = target_directory.join(RUST_PACKAGE);
    let arguments = rust_build_arguments(target, &manifest, &build_directory);
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    crate::cargo_with_env(&arguments, &[])?;
    for program in RUST_PROGRAMS {
        let built = build_directory
            .join(rust_target(target))
            .join("release")
            .join(format!("{program}.exe"));
        let output = Programs {
            directory: directory.to_owned(),
        }
        .path(program, target);
        std::fs::copy(&built, &output)
            .map_err(|e| format!("{} -> {}: {e}", built.display(), output.display()))?;
    }
    Ok(())
}

fn build_mingw_programs(target: &str, directory: &Path) -> Result<(), String> {
    let toolchain = std::env::var_os(LLVM_MINGW_VARIABLE).map(PathBuf::from);
    let compiler = mingw_compiler(target, toolchain.as_deref());
    for program in MINGW_PROGRAMS {
        let source = crate::workspace_root()
            .join("tests")
            .join(MINGW_SOURCE_FOLDER)
            .join(format!("{program}.c"));
        let output = Programs {
            directory: directory.to_owned(),
        }
        .path(program, target);
        let status = std::process::Command::new(&compiler)
            .args(mingw_compiler_arguments(&source, &output))
            .status()
            .map_err(|e| {
                format!(
                    "{}: {e}: install llvm-mingw's msvcrt toolchain and put its bin folder on \
                     PATH, or set {LLVM_MINGW_VARIABLE} to its folder",
                    compiler.display()
                )
            })?;
        if !status.success() {
            return Err(format!(
                "{}: {}: {status}",
                compiler.display(),
                source.display()
            ));
        }
    }
    Ok(())
}

/// llvm-mingw's C compiler for MSVC's `target` (`x64`, `arm64`): in `toolchain`'s `bin` folder
/// when one is given, else found on `PATH`.
pub fn mingw_compiler(target: &str, toolchain: Option<&Path>) -> PathBuf {
    let triple = match target {
        "arm64" => "aarch64-w64-mingw32",
        _ => "x86_64-w64-mingw32",
    };
    let name = format!("{triple}-clang{}", std::env::consts::EXE_SUFFIX);
    match toolchain {
        Some(folder) => folder.join("bin").join(name),
        None => PathBuf::from(name),
    }
}

/// The arguments that build the console program at `source` into `output`, optimized, with
/// warnings as errors, against MinGW's default C runtime, `msvcrt.dll`.
pub fn mingw_compiler_arguments(source: &Path, output: &Path) -> Vec<String> {
    vec![
        "-O2".to_owned(),
        "-Wall".to_owned(),
        "-Werror".to_owned(),
        "-o".to_owned(),
        output.display().to_string(),
        source.display().to_string(),
    ]
}

/// Rust's target for MSVC's `target` (`x64`, `arm64`).
pub fn rust_target(target: &str) -> &'static str {
    match target {
        "arm64" => "aarch64-pc-windows-msvc",
        _ => "x86_64-pc-windows-msvc",
    }
}

/// The `cargo` arguments that build the package at `manifest` optimized for MSVC's `target`,
/// into `target_directory`.
pub fn rust_build_arguments(target: &str, manifest: &Path, target_directory: &Path) -> Vec<String> {
    [
        "build",
        "--release",
        "--manifest-path",
        &manifest.display().to_string(),
        "--target",
        rust_target(target),
        "--target-dir",
        &target_directory.display().to_string(),
    ]
    .iter()
    .map(|argument| argument.to_string())
    .collect()
}

fn visual_studio() -> Result<PathBuf, String> {
    let program_files = std::env::var_os("ProgramFiles(x86)")
        .ok_or("ProgramFiles(x86) is not set, so vswhere cannot be found")?;
    let vswhere = PathBuf::from(program_files)
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    let output = std::process::Command::new(&vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            VISUAL_STUDIO_COMPONENT,
            "-property",
            "installationPath",
        ])
        .output()
        .map_err(|e| format!("{}: {e}", vswhere.display()))?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || path.is_empty() {
        return Err("no Visual Studio with the C++ tools (vswhere found none)".into());
    }
    Ok(PathBuf::from(path))
}

fn developer_environment(
    installation: &Path,
    target: &str,
) -> Result<Vec<(String, String)>, String> {
    let script = installation
        .join("VC")
        .join("Auxiliary")
        .join("Build")
        .join("vcvarsall.bat");
    let argument = vcvars_argument(target, cfg!(target_arch = "aarch64"));
    let output = command_prompt(format!(
        "/d /s /c \"\"{}\" {argument} >nul && set\"",
        script.display()
    ))
    .map_err(|e| format!("{}: {e}", script.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} {argument}: {}",
            script.display(),
            output.status
        ));
    }
    Ok(environment_lines(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(windows)]
fn command_prompt(line: String) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd").raw_arg(line).output()
}

#[cfg(not(windows))]
fn command_prompt(_line: String) -> std::io::Result<std::process::Output> {
    Err(std::io::Error::other(
        "the command prompt runs on Windows hosts only",
    ))
}

fn find_in_path(environment: &[(String, String)], program: &str) -> Option<PathBuf> {
    let path = environment
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))?;
    std::env::split_paths(&path.1)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

/// `vcvarsall.bat`'s argument that sets up MSVC to build `target` programs on an x64 host, or
/// an ARM64 one when `arm64_host`.
pub fn vcvars_argument(target: &str, arm64_host: bool) -> String {
    let host = if arm64_host { "arm64" } else { "x64" };
    if host == target {
        target.to_owned()
    } else {
        format!("{host}_{target}")
    }
}

/// The environment `set` prints, one `NAME=value` per line.
pub fn environment_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| !name.is_empty())
        .map(|(name, value)| (name.to_owned(), value.trim_end_matches('\r').to_owned()))
        .collect()
}

/// The source file `program` is built from.
pub fn source_file(program: &str) -> &'static str {
    match program {
        "eggpad" => "eggpad.c",
        "eggspawn" => "eggspawn.c",
        "eggsync" => "eggsync.c",
        _ => "eggcpp.cpp",
    }
}

/// The `cl.exe` arguments that build `program` from `source` into `output` for MSVC's
/// `target` on the C runtime's DLLs (`/MD`), optimized, with warnings as errors, its objects
/// in `objects`: eggpad as a GUI program; the C++ programs as console programs with C++
/// exceptions (`/EHsc`), `eggcpp-fh3` on x64 with `__CxxFrameHandler3`'s data (`/d2FH4-`).
/// `eggspawn` and `eggsync` are C console programs, `eggsync` with user32's waits.
pub fn compiler_arguments(
    program: &str,
    target: &str,
    source: &Path,
    output: &Path,
    objects: &Path,
) -> Vec<String> {
    let common = [
        "/nologo",
        "/O2",
        "/MD",
        "/W3",
        "/WX",
        "/utf-8",
        "/D_CRT_SECURE_NO_WARNINGS",
    ];
    let language: &[&str] = match (program, target) {
        ("eggpad", _) => &["/DUNICODE", "/D_UNICODE"],
        ("eggspawn" | "eggsync", _) => &[],
        ("eggcpp-fh3", "x64") => &["/EHsc", "/d2FH4-"],
        _ => &["/EHsc"],
    };
    let link: &[&str] = match program {
        "eggpad" => &["/SUBSYSTEM:WINDOWS", "user32.lib", "gdi32.lib"],
        "eggsync" => &["/SUBSYSTEM:CONSOLE", "user32.lib"],
        _ => &["/SUBSYSTEM:CONSOLE"],
    };
    common
        .iter()
        .chain(language)
        .map(|argument| argument.to_string())
        .chain([
            format!("/Fo{}\\", objects.display()),
            format!("/Fe{}", output.display()),
            source.display().to_string(),
            "/link".to_owned(),
        ])
        .chain(link.iter().map(|argument| argument.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rust_programs_build_optimized_for_each_msvc_target() {
        let manifest = Path::new("tests").join("windows-rust").join("Cargo.toml");
        let arguments = rust_build_arguments("arm64", &manifest, Path::new("out"));
        assert_eq!(arguments[..2], ["build", "--release"]);
        let target = arguments.iter().position(|argument| argument == "--target");
        assert_eq!(
            target.map(|at| arguments[at + 1].as_str()),
            Some("aarch64-pc-windows-msvc")
        );
        assert!(arguments.contains(&manifest.display().to_string()));
        assert_eq!(rust_target("x64"), "x86_64-pc-windows-msvc");
        assert_eq!(RUST_PROGRAMS, ["eggrust"]);
    }

    #[test]
    fn the_mingw_programs_build_with_llvm_mingw_for_each_target() {
        assert_eq!(MINGW_PROGRAMS, ["eggmingw"]);
        let suffix = std::env::consts::EXE_SUFFIX;
        assert_eq!(
            mingw_compiler("x64", None),
            PathBuf::from(format!("x86_64-w64-mingw32-clang{suffix}"))
        );
        let toolchain = Path::new("llvm-mingw");
        assert_eq!(
            mingw_compiler("arm64", Some(toolchain)),
            toolchain
                .join("bin")
                .join(format!("aarch64-w64-mingw32-clang{suffix}"))
        );
        let arguments =
            mingw_compiler_arguments(Path::new("eggmingw.c"), Path::new("eggmingw-x64.exe"));
        assert!(arguments.contains(&"-O2".to_owned()));
        assert!(arguments.contains(&"-Werror".to_owned()));
        let output = arguments.iter().position(|argument| argument == "-o");
        assert_eq!(
            output.map(|at| arguments[at + 1].as_str()),
            Some("eggmingw-x64.exe")
        );
        assert_eq!(arguments.last().map(String::as_str), Some("eggmingw.c"));
    }

    #[test]
    fn programs_have_one_file_per_program_and_target() {
        let programs = Programs {
            directory: PathBuf::from("out"),
        };
        assert_eq!(
            programs.path("eggpad", "arm64"),
            Path::new("out").join("eggpad-arm64.exe")
        );
        assert_eq!(msvc_target("x86_64"), Some("x64"));
        assert_eq!(msvc_target("aarch64"), Some("arm64"));
        assert_eq!(msvc_target("riscv64"), None);
        assert!(not_built_for_this_run(Some(std::ffi::OsStr::new(""))));
        assert!(!not_built_for_this_run(Some(std::ffi::OsStr::new("out"))));
        assert!(!not_built_for_this_run(None));
    }

    #[test]
    fn msvc_is_set_up_for_each_target_from_the_host_s_tools() {
        assert_eq!(vcvars_argument("x64", false), "x64");
        assert_eq!(vcvars_argument("arm64", false), "x64_arm64");
        assert_eq!(vcvars_argument("arm64", true), "arm64");
        assert_eq!(vcvars_argument("x64", true), "arm64_x64");
    }

    #[test]
    fn the_environment_is_read_from_set_s_lines() {
        let lines = environment_lines("Path=C:\\a;C:\\b\r\nINCLUDE=x=y\r\n=C:=C:\\\r\nnoise\r\n");
        assert_eq!(
            lines,
            [
                ("Path".to_owned(), "C:\\a;C:\\b".to_owned()),
                ("INCLUDE".to_owned(), "x=y".to_owned()),
            ]
        );
    }

    #[test]
    fn eggpad_builds_with_the_c_runtime_s_dlls_as_a_gui_program() {
        let arguments = compiler_arguments(
            "eggpad",
            "x64",
            Path::new("eggpad.c"),
            Path::new("eggpad-x64.exe"),
            Path::new("objects"),
        );
        for wanted in [
            "/MD",
            "/WX",
            "/SUBSYSTEM:WINDOWS",
            "user32.lib",
            "gdi32.lib",
        ] {
            assert!(arguments.contains(&wanted.to_owned()), "{wanted}");
        }
        assert!(arguments.contains(&"/Feeggpad-x64.exe".to_owned()));
        assert!(arguments.contains(&"/Foobjects\\".to_owned()));
        let link = arguments.iter().position(|argument| argument == "/link");
        let source = arguments.iter().position(|argument| argument == "eggpad.c");
        assert!(source < link);
        assert!(!arguments.contains(&"/EHsc".to_owned()));
        assert_eq!(
            PROGRAMS,
            ["eggpad", "eggcpp", "eggcpp-fh3", "eggspawn", "eggsync"]
        );
    }

    #[test]
    fn the_cpp_programs_build_as_console_programs_with_cpp_exceptions() {
        let objects = Path::new("objects");
        for (program, target) in [
            ("eggcpp", "x64"),
            ("eggcpp", "arm64"),
            ("eggcpp-fh3", "arm64"),
        ] {
            let arguments = compiler_arguments(
                program,
                target,
                Path::new("eggcpp.cpp"),
                Path::new("out.exe"),
                objects,
            );
            for wanted in ["/MD", "/EHsc", "/WX", "/SUBSYSTEM:CONSOLE", "eggcpp.cpp"] {
                assert!(arguments.contains(&wanted.to_owned()), "{program} {wanted}");
            }
            assert!(
                !arguments.contains(&"/d2FH4-".to_owned()),
                "{program} {target}"
            );
            assert!(!arguments.contains(&"user32.lib".to_owned()));
        }
        let frame_handler_3 = compiler_arguments(
            "eggcpp-fh3",
            "x64",
            Path::new("eggcpp.cpp"),
            Path::new("out.exe"),
            objects,
        );
        assert!(frame_handler_3.contains(&"/d2FH4-".to_owned()));
        assert_eq!(source_file("eggpad"), "eggpad.c");
        assert_eq!(source_file("eggcpp-fh3"), "eggcpp.cpp");
    }

    #[test]
    fn eggspawn_builds_as_a_c_console_program() {
        for target in TARGETS {
            let arguments = compiler_arguments(
                "eggspawn",
                target,
                Path::new("eggspawn.c"),
                Path::new("out.exe"),
                Path::new("objects"),
            );
            for wanted in ["/MD", "/WX", "/SUBSYSTEM:CONSOLE", "eggspawn.c"] {
                assert!(arguments.contains(&wanted.to_owned()), "{target} {wanted}");
            }
            assert!(!arguments.contains(&"/EHsc".to_owned()), "{target}");
        }
        assert_eq!(source_file("eggspawn"), "eggspawn.c");
    }

    #[test]
    fn eggsync_builds_as_a_c_console_program_with_user32() {
        for target in TARGETS {
            let arguments = compiler_arguments(
                "eggsync",
                target,
                Path::new("eggsync.c"),
                Path::new("out.exe"),
                Path::new("objects"),
            );
            for wanted in ["/WX", "/SUBSYSTEM:CONSOLE", "user32.lib", "eggsync.c"] {
                assert!(arguments.contains(&wanted.to_owned()), "{target} {wanted}");
            }
            assert!(!arguments.contains(&"/EHsc".to_owned()), "{target}");
        }
        assert_eq!(source_file("eggsync"), "eggsync.c");
    }
}
