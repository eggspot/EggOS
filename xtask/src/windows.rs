//! Windows programs built with Microsoft's C compiler (D146): `tests/windows-crt/eggpad.c`, our
//! own C notepad, is test data that MSVC's `cl.exe` builds as a `/MD` program (on the C
//! runtime's DLLs) for x64 and ARM64. Only a Windows host with Visual Studio's C++ tools can
//! build it, found through `vswhere`; CI builds it on its Windows runner and hands it to the
//! Linux job through `EGGOS_WINDOWS_PROGRAMS`. It is never shipped.

use std::path::{Path, PathBuf};

pub const PROGRAMS_VARIABLE: &str = "EGGOS_WINDOWS_PROGRAMS";
pub const PROGRAMS: [&str; 1] = ["eggpad"];
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
        for program in PROGRAMS {
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
                .join(format!("{program}.c"));
            let output = Programs {
                directory: directory.clone(),
            }
            .path(program, target);
            let status = std::process::Command::new(&compiler)
                .args(compiler_arguments(&source, &output, &objects))
                .env_clear()
                .envs(environment.iter().map(|(name, value)| (name, value)))
                .status()
                .map_err(|e| format!("{}: {e}", compiler.display()))?;
            if !status.success() {
                return Err(format!("cl.exe {}: {status}", source.display()));
            }
        }
    }
    Ok(Some(Programs { directory }))
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

/// The `cl.exe` arguments that build `source` into `output` as a GUI program on the C
/// runtime's DLLs (`/MD`), optimized, with warnings as errors, its objects in `objects`.
pub fn compiler_arguments(source: &Path, output: &Path, objects: &Path) -> Vec<String> {
    [
        "/nologo",
        "/O2",
        "/MD",
        "/W3",
        "/WX",
        "/utf-8",
        "/D_CRT_SECURE_NO_WARNINGS",
        "/DUNICODE",
        "/D_UNICODE",
    ]
    .iter()
    .map(|argument| argument.to_string())
    .chain([
        format!("/Fo{}\\", objects.display()),
        format!("/Fe{}", output.display()),
        source.display().to_string(),
        "/link".to_owned(),
        "/SUBSYSTEM:WINDOWS".to_owned(),
        "user32.lib".to_owned(),
        "gdi32.lib".to_owned(),
    ])
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(PROGRAMS, ["eggpad"]);
    }
}
