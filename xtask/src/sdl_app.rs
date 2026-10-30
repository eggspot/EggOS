//! `cargo xtask sdl3` and `cargo xtask sdl-app` (D172, the Real apps track's first app): a real
//! SDL application, Brogue CE, installed and played inside EggOS through `eggway`.
//!
//! Everything here is test data, downloaded and never shipped or committed (decided by
//! research). Brogue CE and the libraries it loads come from Alpine Linux's packages of a
//! pinned branch, each checked against its pinned SHA-256: the game, `sdl2-compat` (SDL 2's
//! interface over SDL 3), `sdl2_image`, Wayland's client, cursor and EGL stub libraries,
//! `libxkbcommon`, `libffi` and musl's loader. No released SDL 3 draws on Wayland without EGL,
//! and EggOS has no GPU stack yet, so `libSDL3.so.0` is built from SDL's pinned commit that
//! added the Wayland driver's `wl_shm` framebuffer, configured with its Wayland driver alone and
//! no OpenGL, Vulkan, X11, D-Bus or audio back ends, by Alpine's own compilers in an
//! `alpine:3.24` Docker container (`cargo xtask sdl3`, which CI runs on a runner of the
//! architecture's own CPU and caches). The pieces are packed into `brogue.zip`: the game at
//! `brogue`, musl's loader and every library at `lib/`, the game's data at `share/brogue`,
//! which the installer recognises as a dynamically linked program carrying its loader
//! ([`crate::qemu::sdl_app`] boots it).

use crate::dynamic_linking::download;
use crate::rustc_version::sha256_of;
use crate::zip::{self, Member};
use crate::{Arch, build_kernel};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The app's name once installed, and its program's file name in the archive.
pub const APP: &str = "brogue";
/// The archive the host folder holds.
pub const ARCHIVE: &str = "brogue.zip";
/// Brogue CE's version, as Alpine packages it.
pub const BROGUE_VERSION: &str = "1.15.1";
/// Where the game's data is inside the app's folder.
pub const DATA_FOLDER: &str = "share/brogue";
/// Where musl's loader and the libraries are inside the app's folder.
pub const LIBRARY_FOLDER: &str = "lib";
/// SDL's commit `libSDL3.so.0` is built from: `wayland: Add SHM framebuffer support` and its
/// follow-ups, on SDL's main branch (3.5.0 in development).
pub const SDL_COMMIT: &str = "328edfc180a4b455098cc877f2b12e4aa94523b9";
/// The library's name, its `DT_SONAME`, which `sdl2-compat` loads.
pub const SDL3_LIBRARY: &str = "libSDL3.so.0";

const SDL_SHA256: &str = "10975a4a09a7272f080d2123b7e626db77329405536f4fab5fdaf9f9e6da6237";
const ALPINE: &str = "https://dl-cdn.alpinelinux.org/alpine/v3.24";
const ALPINE_IMAGE: &str = "alpine:3.24";
const BUILD_PACKAGES: &[&str] = &[
    "build-base",
    "cmake",
    "pkgconf",
    "linux-headers",
    "wayland-dev",
    "libxkbcommon-dev",
];
const SDL_OPTIONS: &[&str] = &[
    "-DCMAKE_BUILD_TYPE=Release",
    "-DSDL_SHARED=ON",
    "-DSDL_STATIC=OFF",
    "-DSDL_WAYLAND=ON",
    "-DSDL_WAYLAND_SHARED=ON",
    "-DSDL_WAYLAND_LIBDECOR=OFF",
    "-DSDL_X11=OFF",
    "-DSDL_KMSDRM=OFF",
    "-DSDL_OPENGL=OFF",
    "-DSDL_OPENGLES=OFF",
    "-DSDL_VULKAN=OFF",
    "-DSDL_DBUS=OFF",
    "-DSDL_LIBUDEV=OFF",
    "-DSDL_IBUS=OFF",
    "-DSDL_ALSA=OFF",
    "-DSDL_PULSEAUDIO=OFF",
    "-DSDL_PIPEWIRE=OFF",
    "-DSDL_JACK=OFF",
    "-DSDL_SNDIO=OFF",
    "-DSDL_TESTS=OFF",
    "-DSDL_EXAMPLES=OFF",
];
const STAMP: &str = "commit";
const DIRECTORY_MODE: u32 = 0o040_755;
const FILE_MODE: u32 = 0o100_644;
const PROGRAM_MODE: u32 = 0o100_755;

/// One Alpine package: its repository, name, version and SHA-256 on each architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Package {
    pub repository: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub x86_64: &'static str,
    pub aarch64: &'static str,
}

impl Package {
    /// Its file name.
    pub fn file(&self) -> String {
        format!("{}-{}.apk", self.name, self.version)
    }

    /// Where Alpine serves it for `arch`.
    pub fn url(&self, arch: &str) -> String {
        format!("{ALPINE}/{}/{arch}/{}", self.repository, self.file())
    }

    /// Its pinned SHA-256 for `arch`.
    pub fn sha256(&self, arch: &str) -> Option<&'static str> {
        match arch {
            "x86_64" => Some(self.x86_64),
            "aarch64" => Some(self.aarch64),
            _ => None,
        }
    }
}

/// The packages, pinned by research (D172).
pub const PACKAGES: [Package; 9] = [
    Package {
        repository: "community",
        name: "brogue",
        version: "1.15.1-r0",
        x86_64: "2f4d3052636abef1eb20b4bcd0f4cc082a9a5f8f5537a981cd3b89d87adf5f7e",
        aarch64: "017c8104d688d0cdc2a020b67a42a31c0d0bf0693723fa0d10e5b1ae3ec19bad",
    },
    Package {
        repository: "community",
        name: "sdl2-compat",
        version: "2.32.68-r0",
        x86_64: "0c4c1279446cba3c2065a3d3c2cacfa9eb4aeff507757c5a37193aaf028c5cf7",
        aarch64: "571417d7f412517258890dcbc1524ebbb874e68503d798907366f1812d8dc7a1",
    },
    Package {
        repository: "community",
        name: "sdl2_image",
        version: "2.8.12-r0",
        x86_64: "9d9f337de7fe25c363700701b8dd76ff4026de9c3d977e321ada08058b57eeda",
        aarch64: "374a159b4006083310a0f0cf5ce022feb7aced3a5647dfdd454a7ac7dbb34dc9",
    },
    Package {
        repository: "main",
        name: "wayland-libs-client",
        version: "1.25.0-r0",
        x86_64: "a728f7103217cafdc83ff6ad0d679c197dbbe280bd5788c8336259f7a39cb303",
        aarch64: "409b4ad8de59d7b69f7c8add50cc76ef0fe3fe8c80e7854d6fe93c6b1dccc005",
    },
    Package {
        repository: "main",
        name: "wayland-libs-cursor",
        version: "1.25.0-r0",
        x86_64: "16ec369226507abaeb84b496ce2bb1a4ef29987ea91ba6bedc40bb0e0af274e7",
        aarch64: "22129b3f85ecf905a00436040c4ca3d07a071e700997aebf9bb7a2430cb42946",
    },
    Package {
        repository: "main",
        name: "wayland-libs-egl",
        version: "1.25.0-r0",
        x86_64: "c944a59c91ce89972b4cf0ecbb46dfcd43f95c4b86ee63b5522b9a843a3d6af9",
        aarch64: "c06c148aa8fe57848f1a30ac46b95736b04fb1bf73324ccf29c0bae82ee4a7e7",
    },
    Package {
        repository: "main",
        name: "libxkbcommon",
        version: "1.13.1-r0",
        x86_64: "f52efd26a9069b1c3edc89cab399f7f81bdc47c9e88a54e017c470bd9734b003",
        aarch64: "484370ec7cb91bd8035a7f947620885557daf0d4b10d8dbe50f207875828988e",
    },
    Package {
        repository: "main",
        name: "libffi",
        version: "3.5.2-r1",
        x86_64: "6da1a19313a40c56cac756fb11e2c4d5267831e1c22fc8323f24c943c94da6e0",
        aarch64: "9a67280b0c7c62db658c2d4d32ed1199903bef859c32dd4245e1a7e35c89ab4b",
    },
    Package {
        repository: "main",
        name: "musl",
        version: "1.2.6-r3",
        x86_64: "e142255d0fa397f14318996bc10378093dcc069dc3dd1b6818c8f28a789c21da",
        aarch64: "2e17864102278d9049c3a56f1bceed9e8cf287065617e579325123443daed8a5",
    },
];

/// The libraries the app carries, by the names they are loaded by, and where the unpacked
/// packages hold them (links that the copy follows).
pub const LIBRARIES: [&str; 7] = [
    "libSDL2-2.0.so.0",
    "libSDL2_image-2.0.so.0",
    "libwayland-client.so.0",
    "libwayland-cursor.so.0",
    "libwayland-egl.so.1",
    "libxkbcommon.so.0",
    "libffi.so.8",
];

const PROGRAM_IN_PACKAGE: &str = "usr/libexec/brogue";
const DATA_IN_PACKAGE: &str = "usr/share/brogue";
const LIBRARIES_IN_PACKAGE: &str = "usr/lib";

fn musl_loader(arch: &str) -> String {
    format!("ld-musl-{arch}.so.1")
}

fn docker_platform(arch: &str) -> Result<&'static str, String> {
    match arch {
        "x86_64" => Ok("linux/amd64"),
        "aarch64" => Ok("linux/arm64"),
        other => Err(format!(
            "{other}: the SDL app runs on x86_64 and aarch64, where Alpine builds Brogue CE"
        )),
    }
}

fn root() -> PathBuf {
    crate::target_directory().join("sdl-app")
}

pub(crate) fn create(folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))
}

pub(crate) fn fresh(folder: &Path) -> Result<(), String> {
    if folder.exists() {
        std::fs::remove_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    create(folder)
}

pub(crate) fn fetch_pinned(url: &str, file: &Path, sha256: &str) -> Result<(), String> {
    if !file.is_file() || sha256_of(file)? != sha256 {
        download(url, file)?;
    }
    let found = sha256_of(file)?;
    match found == sha256 {
        true => Ok(()),
        false => Err(format!(
            "{}: SHA-256 {found}, not the pinned {sha256}: Alpine's mirror keeps only each \
             package's latest build, so pin the new one in its xtask module after checking it",
            file.display()
        )),
    }
}

pub(crate) fn untar(archive: &Path, into: &Path) -> Result<(), String> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .map_err(|e| format!("could not run tar: {e}"))?;
    match status.success()
        || archive
            .extension()
            .is_some_and(|extension| extension == "apk")
    {
        true => Ok(()),
        false => Err(format!("tar -xzf {}: {status}", archive.display())),
    }
}

fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|e| format!("could not run {what}: {e} (is Docker installed?)"))?;
    match status.success() {
        true => Ok(()),
        false => Err(format!("{what}: {status}")),
    }
}

#[cfg(unix)]
fn owner_of(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some(format!("{}:{}", metadata.uid(), metadata.gid()))
}

#[cfg(not(unix))]
fn owner_of(_path: &Path) -> Option<String> {
    None
}

fn build_in_container(container: &str, source: &str, owner: Option<&str>) -> Result<(), String> {
    let exec = |arguments: &[&str]| {
        let mut command = Command::new("docker");
        command.arg("exec").arg(container).args(arguments);
        command
    };
    let mut install = vec!["apk", "add", "--no-cache"];
    install.extend(BUILD_PACKAGES);
    run(&mut exec(&install), "apk add in the container")?;
    let mut configure = vec!["cmake", "-S", source, "-B", "/sdl/build"];
    configure.extend(SDL_OPTIONS);
    run(&mut exec(&configure), "cmake's configuration of SDL")?;
    run(
        &mut exec(&["cmake", "--build", "/sdl/build", "--parallel"]),
        "cmake's build of SDL",
    )?;
    if let Some(owner) = owner {
        run(
            &mut exec(&["chown", "-R", owner, "/sdl"]),
            "chown of the build",
        )?;
    }
    Ok(())
}

/// Builds `libSDL3.so.0` for `arch` from [`SDL_COMMIT`] in an `alpine:3.24` container (see the
/// module documentation), unless the folder already holds that commit's; its path.
pub fn build_sdl3(arch: &Arch) -> Result<PathBuf, String> {
    let platform = docker_platform(arch.name)?;
    let root = root();
    let output = root.join(format!("sdl3-{}", arch.name));
    let library = output.join(SDL3_LIBRARY);
    let stamp = output.join(STAMP);
    let built = std::fs::read_to_string(&stamp).is_ok_and(|commit| commit.trim() == SDL_COMMIT);
    if library.is_file() && built {
        return Ok(library);
    }
    let downloads = root.join("downloads");
    create(&downloads)?;
    let archive = downloads.join(format!("SDL-{SDL_COMMIT}.tar.gz"));
    let url = format!("https://github.com/libsdl-org/SDL/archive/{SDL_COMMIT}.tar.gz");
    fetch_pinned(&url, &archive, SDL_SHA256)?;
    let work = root.join(format!("sdl3-build-{}", arch.name));
    fresh(&work)?;
    untar(&archive, &work)?;
    let work = work
        .canonicalize()
        .map_err(|e| format!("{}: {e}", work.display()))?;
    let container = format!("eggos-sdl3-{}-{}", arch.name, std::process::id());
    let volume = format!("{}:/sdl", work.display());
    run(
        Command::new("docker").args([
            "create",
            "--name",
            &container,
            "--platform",
            platform,
            "-v",
            &volume,
            ALPINE_IMAGE,
            "sleep",
            "7200",
        ]),
        "docker create",
    )?;
    let source = format!("/sdl/SDL-{SDL_COMMIT}");
    let owner = owner_of(&work);
    let built = run(
        Command::new("docker").args(["start", &container]),
        "docker start",
    )
    .and_then(|()| build_in_container(&container, &source, owner.as_deref()));
    let removed = run(
        Command::new("docker").args(["rm", "--force", &container]),
        "docker rm",
    );
    built?;
    removed?;
    fresh(&output)?;
    let made = work.join("build").join(SDL3_LIBRARY);
    std::fs::copy(&made, &library)
        .map_err(|e| format!("{} to {}: {e}", made.display(), library.display()))?;
    std::fs::write(&stamp, SDL_COMMIT).map_err(|e| format!("{}: {e}", stamp.display()))?;
    Ok(library)
}

fn sdl3_library(arch: &Arch) -> Result<PathBuf, String> {
    let output = root().join(format!("sdl3-{}", arch.name));
    let library = output.join(SDL3_LIBRARY);
    let built =
        std::fs::read_to_string(output.join(STAMP)).is_ok_and(|commit| commit.trim() == SDL_COMMIT);
    match library.is_file() && built {
        true => Ok(library),
        false => build_sdl3(arch),
    }
}

fn fetch_packages(arch: &Arch) -> Result<PathBuf, String> {
    let root = root();
    let downloads = root.join(format!("downloads-{}", arch.name));
    create(&downloads)?;
    let unpacked = root.join(format!("packages-{}", arch.name));
    fresh(&unpacked)?;
    for package in PACKAGES {
        let sha256 = package
            .sha256(arch.name)
            .ok_or_else(|| format!("{}: no pinned {}", arch.name, package.name))?;
        let file = downloads.join(package.file());
        fetch_pinned(&package.url(arch.name), &file, sha256)?;
        untar(&file, &unpacked)?;
    }
    Ok(unpacked)
}

pub(crate) fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| {
        format!(
            "{}: {e}: the pinned packages no longer hold what the app needs",
            path.display()
        )
    })
}

/// The archive's members for `arch`: the game, musl's loader, the libraries (`libSDL3.so.0`
/// from `sdl3`) and the game's data, from the packages unpacked at `packages`.
pub fn members(arch: &str, packages: &Path, sdl3: &Path) -> Result<Vec<Member>, String> {
    let mut members = vec![
        Member {
            name: APP.to_owned(),
            data: read(&packages.join(PROGRAM_IN_PACKAGE))?,
            mode: PROGRAM_MODE,
        },
        Member {
            name: format!("{LIBRARY_FOLDER}/"),
            data: Vec::new(),
            mode: DIRECTORY_MODE,
        },
        Member {
            name: format!("{LIBRARY_FOLDER}/{}", musl_loader(arch)),
            data: read(&packages.join("lib").join(musl_loader(arch)))?,
            mode: PROGRAM_MODE,
        },
        Member {
            name: format!("{LIBRARY_FOLDER}/{SDL3_LIBRARY}"),
            data: read(sdl3)?,
            mode: FILE_MODE,
        },
    ];
    for library in LIBRARIES {
        members.push(Member {
            name: format!("{LIBRARY_FOLDER}/{library}"),
            data: read(&packages.join(LIBRARIES_IN_PACKAGE).join(library))?,
            mode: FILE_MODE,
        });
    }
    members.push(Member {
        name: "share/".to_owned(),
        data: Vec::new(),
        mode: DIRECTORY_MODE,
    });
    members.extend(zip::folder_members(
        &packages.join(DATA_IN_PACKAGE),
        DATA_FOLDER,
    )?);
    Ok(members)
}

/// What the app is installed with, as `app install --arguments` takes it: a new game, skipping
/// the menu, with the game's data where the archive puts it.
pub fn install_arguments() -> String {
    format!("-n --data-dir /Applications/{APP}/{DATA_FOLDER}")
}

fn pack(arch: &Arch, packages: &Path, sdl3: &Path) -> Result<PathBuf, String> {
    let folder = root().join(arch.name).join("host");
    fresh(&folder)?;
    let archive = zip::stored(&members(arch.name, packages, sdl3)?)?;
    let path = folder.join(ARCHIVE);
    std::fs::write(&path, archive).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(folder)
}

/// Builds or finds `libSDL3.so.0`, downloads the packages, packs `brogue.zip` into a host
/// folder and runs the boot test ([`crate::qemu::sdl_app::check`]) on the release kernel.
pub fn check(arch: &Arch) -> Result<(), String> {
    docker_platform(arch.name)?;
    let sdl3 = sdl3_library(arch)?;
    let packages = fetch_packages(arch)?;
    let folder = pack(arch, &packages, &sdl3)?;
    let kernel = build_kernel(arch, None)?;
    crate::qemu::sdl_app::check(arch, &kernel, &folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_package_is_pinned_on_both_architectures() {
        for package in PACKAGES {
            for arch in ["x86_64", "aarch64"] {
                let sha256 = package.sha256(arch).unwrap();
                assert_eq!(sha256.len(), 64, "{}", package.name);
                assert!(sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            }
            assert_eq!(package.sha256("riscv64"), None);
        }
        assert_eq!(
            PACKAGES[0].url("aarch64"),
            "https://dl-cdn.alpinelinux.org/alpine/v3.24/community/aarch64/brogue-1.15.1-r0.apk"
        );
        assert!(PACKAGES[0].version.starts_with(BROGUE_VERSION));
        assert_eq!(SDL_SHA256.len(), 64);
    }

    #[test]
    fn the_archive_holds_the_game_its_loader_libraries_and_data() {
        let scratch = std::env::temp_dir().join(format!("eggos-sdl-app-{}", std::process::id()));
        let packages = scratch.join("packages");
        for folder in [
            packages.join("usr/libexec"),
            packages.join("lib"),
            packages.join(LIBRARIES_IN_PACKAGE),
            packages.join(DATA_IN_PACKAGE).join("assets"),
        ] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(packages.join(PROGRAM_IN_PACKAGE), b"\x7fELFgame").unwrap();
        std::fs::write(packages.join("lib/ld-musl-x86_64.so.1"), b"loader").unwrap();
        for library in LIBRARIES {
            std::fs::write(packages.join(LIBRARIES_IN_PACKAGE).join(library), library).unwrap();
        }
        std::fs::write(
            packages.join(DATA_IN_PACKAGE).join("assets/tiles.png"),
            b"png",
        )
        .unwrap();
        let sdl3 = scratch.join(SDL3_LIBRARY);
        std::fs::write(&sdl3, b"sdl3").unwrap();
        let names: Vec<String> = members("x86_64", &packages, &sdl3)
            .unwrap()
            .into_iter()
            .map(|member| member.name)
            .collect();
        std::fs::remove_dir_all(&scratch).unwrap();
        assert_eq!(names[0], "brogue");
        assert!(names.contains(&"lib/ld-musl-x86_64.so.1".to_owned()));
        assert!(names.contains(&"lib/libSDL3.so.0".to_owned()));
        assert!(names.contains(&"lib/libffi.so.8".to_owned()));
        assert!(names.contains(&"share/brogue/assets/tiles.png".to_owned()));
        assert_eq!(
            install_arguments(),
            "-n --data-dir /Applications/brogue/share/brogue"
        );
    }
}
