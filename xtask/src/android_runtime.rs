//! Android's runtime module as test data (D188): the dynamic linker (`linker64`) and bionic's
//! shared libraries (`libc.so`, `libm.so`, `libdl.so` and the rest of the module's `lib64`)
//! that a dynamically linked Android program needs, laid out as Android lays them out under
//! `/system`, for the dynamic Android programs boot test and for `cargo xtask image --add` of
//! such a program.
//!
//! The NDK ships neither the linker nor runtime libraries (only stubs to link against), so
//! they come from AOSP's prebuilt `com.android.runtime` APEX in `platform/prebuilts/runtime`
//! at a pinned commit (decided by research): downloaded from android.googlesource.com,
//! checked against the git blob id that commit records ([`crate::gitiles`]), never shipped in
//! a release and never committed. An APEX is a `.zip` whose stored `apex_payload.img` is an
//! ext4 image ([`crate::ext4`]); the linker goes to `/system/bin/linker64`, every library to
//! `/system/lib64`, the folder the linker searches, and a linker configuration of ours to
//! `/system/etc/ld.config.<abi>.txt`, the first file the linker looks for, mapping the folders
//! programs run from to one namespace that searches `/system/lib64`.

use crate::ext4::{Image, Kind};
use crate::gitiles;
use crate::initramfs::Content;
use eggos_app::archive::{self, Archive};
use eggos_app::reader::Bytes;
use eggos_png::inflate::{Sink, Window};
use std::path::{Path, PathBuf};

/// Android's dynamic linker for 64-bit programs: the `PT_INTERP` of every dynamically linked
/// one.
pub const LINKER_PATH: &str = "/system/bin/linker64";

const REPOSITORY: &str = "https://android.googlesource.com/platform/prebuilts/runtime";
const COMMIT: &str = "070571b455076f77a01c7b07154a15e545d2b428";
const PAYLOAD: &[u8] = b"apex_payload.img";
const LINKER_IN_PAYLOAD: &str = "bin/linker64";
const LIBRARY_FOLDERS_IN_PAYLOAD: [&str; 2] = ["lib64/bionic", "lib64"];
const LIBRARY_FOLDER: &str = "system/lib64";
const PROGRAM_FOLDERS: [&str; 6] = [
    "/system/bin/",
    "/bin/",
    "/usr/bin/",
    "/usr/local/bin/",
    "/home/",
    "/tmp/",
];

/// The runtime module of one Android ABI at the pinned commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Module {
    /// Android's name for the ABI (`arm64`, `x86_64`), in the APEX's file name and the
    /// linker configuration's.
    pub abi: &'static str,
    /// The git blob id of the APEX at the pinned commit.
    pub blob: &'static str,
}

/// The runtime module for the architecture named `arch`. `None` on riscv64, whose Android
/// programs need the vector extension the kernel does not run yet (D188), and elsewhere.
pub fn module(arch: &str) -> Option<Module> {
    match arch {
        "aarch64" => Some(Module {
            abi: "arm64",
            blob: "26a7749e9b232184112897144c97128073776f2a",
        }),
        "x86_64" => Some(Module {
            abi: "x86_64",
            blob: "8763cb7be08e9490ea38865a282b7ec643390ce5",
        }),
        _ => None,
    }
}

fn url(module: Module) -> String {
    gitiles::file_url(
        REPOSITORY,
        COMMIT,
        &format!("mainline/runtime/apex/{}", file_name(module)),
    )
}

fn file_name(module: Module) -> String {
    format!("com.android.runtime-{}.apex", module.abi)
}

/// The linker configuration's path in the image for `module`'s ABI.
pub fn linker_config_path(module: Module) -> String {
    format!("system/etc/ld.config.{}.txt", module.abi)
}

/// Our linker configuration: the folders programs run from, all in one section whose default
/// namespace is not isolated and searches `/system/lib64` (`${LIB}`).
pub fn linker_config() -> String {
    let mut text: String = PROGRAM_FOLDERS
        .iter()
        .map(|folder| format!("dir.system = {folder}\n"))
        .collect();
    text.push_str(
        "\n[system]\nnamespace.default.isolated = false\n\
         namespace.default.search.paths = /system/${LIB}\n",
    );
    text
}

struct Collect(Vec<u8>);

impl Sink for Collect {
    fn write(&mut self, bytes: &[u8]) -> bool {
        self.0.extend_from_slice(bytes);
        true
    }
}

fn payload(apex: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = Bytes(apex);
    let opened = Archive::open(&mut reader)
        .map_err(|e| format!("the runtime module is not a readable .zip: {e:?}"))?;
    let mut cursor = opened.first();
    while let Some(entry) = opened
        .next(&mut reader, &mut cursor)
        .map_err(|e| format!("the runtime module's .zip: {e:?}"))?
    {
        if entry.name() != PAYLOAD {
            continue;
        }
        let mut window = Box::new(Window::new());
        let mut out = Collect(Vec::new());
        archive::extract(&mut reader, &entry, &mut window, &mut out)
            .map_err(|e| format!("the runtime module's apex_payload.img: {e:?}"))?;
        return Ok(out.0);
    }
    Err("the runtime module has no apex_payload.img".into())
}

/// The linker and the libraries in the runtime module's payload (an ext4 image), each with
/// its path in our image: `linker64` in `system/bin`, every `.so` of `lib64/bionic` and
/// `lib64` in `system/lib64` (the first of a name wins; a symbolic link that leads out of the
/// payload is left out). An error without `libc.so`.
pub fn files(payload: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let image = Image::open(payload)?;
    let linker = image.read(LINKER_IN_PAYLOAD)?;
    let mut files = vec![(LINKER_PATH.trim_start_matches('/').to_owned(), linker)];
    for folder in LIBRARY_FOLDERS_IN_PAYLOAD {
        let Ok(names) = image.list(folder) else {
            continue;
        };
        for (name, kind) in names {
            let path = format!("{LIBRARY_FOLDER}/{name}");
            if !name.ends_with(".so")
                || !matches!(kind, Kind::File | Kind::Link)
                || files.iter().any(|(taken, _)| *taken == path)
            {
                continue;
            }
            match image.read(&format!("{folder}/{name}")) {
                Ok(bytes) => files.push((path, bytes)),
                Err(_) if kind == Kind::Link => {}
                Err(error) => return Err(error),
            }
        }
    }
    if !files.iter().any(|(path, _)| path.ends_with("/libc.so")) {
        return Err("the runtime module has no libc.so in lib64/bionic or lib64".into());
    }
    Ok(files)
}

/// The runtime an image with a dynamically linked Android program gets: the files
/// [`files`] finds, extracted on the host, and the linker configuration.
#[derive(Debug)]
pub struct Runtime {
    files: Vec<(String, String)>,
    config_path: String,
    config: String,
}

impl Runtime {
    /// Fetches the pinned runtime module for `arch` into `target/android-runtime/<abi>` (once;
    /// later calls reuse it) and extracts its files there.
    pub fn fetch(arch: &str) -> Result<Self, String> {
        let module = module(arch).ok_or_else(|| {
            format!(
                "{arch}: no Android runtime module for it: dynamically linked Android programs \
                 run on aarch64 and x86_64 (D188)"
            )
        })?;
        let folder = crate::target_directory()
            .join("android-runtime")
            .join(module.abi);
        create_folder(&folder)?;
        let apex = gitiles::fetch(&url(module), module.blob, &folder.join(file_name(module)))?;
        let payload = payload(&apex).map_err(|e| format!("{}: {e}", file_name(module)))?;
        let extracted = folder.join("files");
        let mut files = Vec::new();
        for (path, bytes) in self::files(&payload)? {
            let host: PathBuf = extracted.join(&path);
            if let Some(parent) = host.parent() {
                create_folder(parent)?;
            }
            std::fs::write(&host, &bytes).map_err(|e| format!("{}: {e}", host.display()))?;
            files.push((path, host.to_string_lossy().into_owned()));
        }
        Ok(Self {
            files,
            config_path: linker_config_path(module),
            config: linker_config(),
        })
    }

    /// The initramfs members: the linker and libraries as programs, then the configuration.
    pub fn members(&self) -> Vec<(&str, Content<'_>)> {
        self.files
            .iter()
            .map(|(path, host)| (path.as_str(), Content::Program(host)))
            .chain([(self.config_path.as_str(), Content::Text(&self.config))])
            .collect()
    }
}

fn create_folder(folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::{Member, stored};

    #[test]
    fn aarch64_and_x86_64_have_a_pinned_module_and_riscv64_none() {
        for (arch, abi) in [("aarch64", "arm64"), ("x86_64", "x86_64")] {
            let module = module(arch).unwrap();
            assert_eq!(module.abi, abi);
            assert_eq!(module.blob.len(), 40);
            assert!(module.blob.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(url(module).starts_with(&format!("{REPOSITORY}/+/{COMMIT}/mainline/")));
            assert!(url(module).ends_with(&format!("runtime-{abi}.apex?format=TEXT")));
        }
        assert_eq!(module("riscv64"), None);
        assert_eq!(module("s390x"), None);
    }

    #[test]
    fn the_linker_config_puts_every_program_folder_in_one_open_namespace() {
        let config = linker_config();
        for folder in PROGRAM_FOLDERS {
            assert!(config.contains(&format!("dir.system = {folder}\n")));
        }
        assert!(config.contains("\n[system]\n"));
        assert!(config.contains("namespace.default.isolated = false\n"));
        assert!(config.ends_with("namespace.default.search.paths = /system/${LIB}\n"));
        let arm64 = module("aarch64").unwrap();
        assert_eq!(linker_config_path(arm64), "system/etc/ld.config.arm64.txt");
        assert_eq!(LINKER_PATH.trim_start_matches('/'), "system/bin/linker64");
    }

    #[test]
    fn the_payload_is_the_apex_s_stored_image() {
        let member = |name: &str, data: &[u8]| Member {
            name: name.to_owned(),
            data: data.to_vec(),
            mode: 0o100_644,
        };
        let apex = stored(&[
            member("apex_manifest.pb", b"manifest"),
            member("apex_payload.img", b"ext4 bytes"),
        ])
        .unwrap();
        assert_eq!(payload(&apex).unwrap(), b"ext4 bytes");
        let without = stored(&[member("apex_manifest.pb", b"manifest")]).unwrap();
        assert!(payload(&without).is_err());
        assert!(payload(b"not a zip").is_err());
        assert!(files(b"not ext4").is_err());
    }
}
