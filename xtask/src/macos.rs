//! macOS console programs (D120): Mach-O executables, thin or fat, that the macOS layer at
//! `/usr/lib/eggos/macos` runs on aarch64 and x86_64. An image gets the layer only when an added
//! file is a Mach-O or fat file. The boot test's programs are built from `tests/macos-console`
//! by Apple's linker, so only on a macOS host; CI builds them on its macOS runner and hands them
//! to the Linux job through `EGGOS_MACOS_PROGRAMS`. A fat file of both slices is written here.

use std::path::{Path, PathBuf};

pub const LAYER_PACKAGE: &str = "eggos-macos";
pub const LAYER_PATH: &str = "usr/lib/eggos/macos";
pub const CONSOLE_PACKAGE: &str = "eggos-macos-console";
pub const PROGRAMS_VARIABLE: &str = "EGGOS_MACOS_PROGRAMS";
pub const DEPLOYMENT_TARGET: &str = "13.0";
pub const PROGRAMS: [&str; 2] = ["macos-console", "macos-unsupported"];

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

/// The boot test's programs: those in `EGGOS_MACOS_PROGRAMS` if it is set, else built here on a
/// macOS host with Apple's linker; `None` on other hosts.
pub fn find_or_build_programs(target_directory: &Path) -> Result<Option<Programs>, String> {
    if let Some(directory) = std::env::var_os(PROGRAMS_VARIABLE) {
        let programs = Programs {
            directory: PathBuf::from(directory),
        };
        for program in PROGRAMS {
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
    }
    Ok(Some(Programs { directory }))
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
    }
}
