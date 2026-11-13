//! The component store (D235): `eggos-<version>-<arch>-components.img`, built outside the base
//! image, in the format `eggos_components::index` reads, with each optional component the
//! architecture runs and the vendors' firmware its machines need (D265), and the links the base
//! keeps at each component file's fixed path.

use crate::{Arch, release};
use eggos_components::firmware::{self, Pinned};
use eggos_components::index::{self, Component, File, Packed};
use eggos_components::manager;
use std::path::PathBuf;

/// One component the store offers, and the programs its files are built from.
#[derive(Debug)]
pub struct Offered {
    pub name: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub on_remove: &'static str,
    pub needs: &'static [&'static str],
    pub files: &'static [(&'static str, &'static str)],
    pub runs_on: fn(&str) -> bool,
}

/// Every component a store may offer, each after those it needs.
pub const OFFERED: [Offered; 5] = [
    Offered {
        name: "desktop",
        title: "Desktop",
        summary: "The window server, the shell and its windows",
        on_remove: "EggOS starts to its text console; apps with windows need it to run.",
        needs: &[],
        files: &[],
        runs_on: |_| true,
    },
    Offered {
        name: "builtin-apps",
        title: "Built-in apps",
        summary: "Files, Text Editor, Downloads",
        on_remove: "They leave the launcher; your files stay.",
        needs: &["desktop"],
        files: &[(crate::BUILTIN_APPS_PATH, crate::BUILTIN_APPS_PACKAGE)],
        runs_on: |_| true,
    },
    Offered {
        name: "windows-apps",
        title: "Windows apps",
        summary: "Runs Windows programs natively, no emulation",
        on_remove: "Installed Windows programs stay in Apps and need it to run.",
        needs: &[],
        files: &[(crate::WINDOWS_LAYER_PATH, crate::WINDOWS_LAYER_PACKAGE)],
        runs_on: |arch| crate::WINDOWS_LAYER_ARCHES.contains(&arch),
    },
    Offered {
        name: "mac-apps",
        title: "Mac apps",
        summary: "Runs Mac programs natively, no emulation",
        on_remove: "Installed Mac programs stay in Apps and need it to run.",
        needs: &[],
        files: &[(crate::macos::LAYER_PATH, crate::macos::LAYER_PACKAGE)],
        runs_on: crate::macos::layer_runs_on,
    },
    Offered {
        name: "android-apps",
        title: "Android apps",
        summary: "Runs Android apps natively, compiled ahead of time, no emulation",
        on_remove: "Installed Android apps stay in Apps and need it to run.",
        needs: &["desktop"],
        files: &[(crate::ANDROID_LAYER_PATH, crate::ANDROID_LAYER_PACKAGE)],
        runs_on: |arch| crate::ANDROID_LAYER_ARCHES.contains(&arch),
    },
];

/// The components the store of `arch` offers.
pub fn offered(arch: &str) -> impl Iterator<Item = &'static Offered> {
    OFFERED
        .iter()
        .filter(move |offered| (offered.runs_on)(arch))
}

/// The links the base image of `arch` holds: each offered file's fixed path and each firmware
/// component's folder, pointing into where the component manager installs it, dangling until
/// then.
pub fn base_links(arch: &str) -> Vec<(String, String)> {
    let layers = offered(arch).flat_map(|offered| {
        offered.files.iter().map(|(path, _)| {
            (
                (*path).to_string(),
                manager::installed_path(offered.name, path),
            )
        })
    });
    let firmware = firmware::for_arch(arch).map(|component| {
        let folder = firmware::folder_path(component);
        let target = manager::installed_path(component.name, &folder);
        (folder, target)
    });
    layers.chain(firmware).collect()
}

/// The store's file name for `arch`.
pub fn store_name(arch: &Arch) -> String {
    manager::store_file_name(release::VERSION, arch.name)
}

fn packed(offered: &Offered, contents: Vec<Vec<u8>>) -> Packed {
    Packed {
        component: Component {
            name: offered.name.into(),
            title: offered.title.into(),
            summary: offered.summary.into(),
            on_remove: offered.on_remove.into(),
            needs: offered.needs.iter().map(|need| (*need).into()).collect(),
            files: offered
                .files
                .iter()
                .map(|(path, _)| File {
                    path: (*path).into(),
                    mode: 0o755,
                    offset: 0,
                    length: 0,
                    sha256: [0; 32],
                })
                .collect(),
        },
        contents,
    }
}

/// The store image holding `parts` (each offered component with its files' bytes) for `arch`.
pub fn store_image(parts: &[(&Offered, Vec<Vec<u8>>)], arch: &str) -> Vec<u8> {
    let packed: Vec<Packed> = parts
        .iter()
        .map(|(offered, contents)| packed(offered, contents.clone()))
        .collect();
    index::store_image(&packed, release::VERSION, arch)
}

fn firmware_cache() -> PathBuf {
    crate::target_directory()
        .join("firmware")
        .join(firmware::LINUX_FIRMWARE_TAG)
}

fn fetch_firmware(file: &Pinned) -> Result<Vec<u8>, String> {
    let path = firmware_cache().join(file.path);
    let cached = std::fs::read(&path)
        .ok()
        .filter(|bytes| firmware::matches_pin(file, bytes));
    if let Some(bytes) = cached {
        return Ok(bytes);
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    crate::dynamic_linking::download(&firmware::upstream_url(file), &path)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    match firmware::matches_pin(file, &bytes) {
        true => Ok(bytes),
        false => Err(format!(
            "{}: not the SHA-256 pinned in userland/components/src/firmware.rs for linux-firmware {}",
            file.path,
            firmware::LINUX_FIRMWARE_TAG
        )),
    }
}

/// The firmware components of `arch` (D265), each with its pinned files fetched from
/// linux-firmware into `target/firmware/<tag>/` and checked.
pub fn firmware_parts(arch: &str) -> Result<Vec<Packed>, String> {
    let mut parts = Vec::new();
    for component in firmware::for_arch(arch) {
        let mut failed = None;
        let gathered = firmware::gather(component, &mut |file| match fetch_firmware(file) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                failed = Some(error);
                None
            }
        });
        let gathered = gathered
            .map_err(|gathering| failed.take().unwrap_or_else(|| format!("{gathering:?}")))?;
        parts.push(Packed {
            component: firmware::record(component, &gathered),
            contents: gathered.into_iter().map(|(_, bytes)| bytes).collect(),
        });
    }
    Ok(parts)
}

/// Builds the store of `arch` into `target/components/` and returns its path.
pub fn build_store(arch: &Arch) -> Result<PathBuf, String> {
    let mut packed_parts = Vec::new();
    for offered in offered(arch.name) {
        let mut contents = Vec::new();
        for (_, package) in offered.files {
            let program = crate::build_runtime_program(arch, package)?;
            contents.push(std::fs::read(&program).map_err(|e| format!("{program}: {e}"))?);
        }
        packed_parts.push(packed(offered, contents));
    }
    packed_parts.extend(firmware_parts(arch.name)?);
    let image = index::store_image(&packed_parts, release::VERSION, arch.name);
    let dir = crate::target_directory().join("components");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(store_name(arch));
    std::fs::write(&path, &image).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_xtask_writes_is_the_one_the_component_manager_reads() {
        let parts: Vec<(&Offered, Vec<Vec<u8>>)> = offered("x86_64")
            .map(|offered| (offered, vec![vec![0x7f; 700]; offered.files.len()]))
            .collect();
        let image = store_image(&parts, "x86_64");
        assert_eq!(image.len() % index::STORE_BLOCK, 0);
        assert_eq!(&image[..16], b"EGGOS-COMPONENTS");
        let header = index::Header::parse(&image).unwrap();
        let text = &image[index::SECTOR as usize..][..header.index_bytes as usize];
        let read = index::parse_index(&header, text, image.len() as u64).unwrap();
        let names: Vec<&str> = read.components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "desktop",
                "builtin-apps",
                "windows-apps",
                "mac-apps",
                "android-apps"
            ]
        );
        for file in read
            .components
            .iter()
            .flat_map(|component| &component.files)
        {
            let bytes = &image[file.offset as usize..][..file.length as usize];
            assert_eq!(index::sha256(bytes), file.sha256);
            assert!(index::is_component_path(&file.path));
        }
        assert!(read.components[0].files.is_empty());
    }

    #[test]
    fn the_base_links_each_layer_s_fixed_path_into_its_component_where_the_layer_runs() {
        assert_eq!(
            base_links("x86_64"),
            [
                (
                    "usr/libexec/builtin-apps".to_string(),
                    "/var/lib/eggos/components/builtin-apps/root/usr/libexec/builtin-apps"
                        .to_string()
                ),
                (
                    "usr/lib/eggos/windows".to_string(),
                    "/var/lib/eggos/components/windows-apps/root/usr/lib/eggos/windows".to_string()
                ),
                (
                    "usr/lib/eggos/macos".to_string(),
                    "/var/lib/eggos/components/mac-apps/root/usr/lib/eggos/macos".to_string()
                ),
                (
                    "usr/lib/eggos/android".to_string(),
                    "/var/lib/eggos/components/android-apps/root/usr/lib/eggos/android".to_string()
                ),
                (
                    "usr/lib/firmware/intel/iwlwifi".to_string(),
                    "/var/lib/eggos/components/wifi-firmware/root/usr/lib/firmware/intel/iwlwifi"
                        .to_string()
                ),
                (
                    "usr/lib/firmware/i915".to_string(),
                    "/var/lib/eggos/components/display-firmware/root/usr/lib/firmware/i915"
                        .to_string()
                ),
            ]
        );
        assert_eq!(
            base_links("riscv64"),
            [(
                "usr/libexec/builtin-apps".to_string(),
                "/var/lib/eggos/components/builtin-apps/root/usr/libexec/builtin-apps".to_string()
            )]
        );
        assert_eq!(
            offered("riscv64")
                .map(|offered| offered.name)
                .collect::<Vec<_>>(),
            ["desktop", "builtin-apps"]
        );
    }
}
