//! The component store (D235): `eggos-<version>-<arch>-components.img`, built outside the base
//! image, in the format `eggos_components::index` reads, with each optional component the
//! architecture runs, and the links the base keeps at each component file's fixed path.

use crate::{Arch, release};
use eggos_components::index::{self, Component, File, Header, Index, SECTOR};
use eggos_components::manager;
use std::path::PathBuf;

const KERNEL_BLOCK: usize = 4096;

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
pub const OFFERED: [Offered; 2] = [
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
];

/// The components the store of `arch` offers.
pub fn offered(arch: &str) -> impl Iterator<Item = &'static Offered> {
    OFFERED.iter().filter(move |offered| (offered.runs_on)(arch))
}

/// The links the base image of `arch` holds: each offered file's fixed path, pointing into
/// where the component manager installs it, dangling until then.
pub fn base_links(arch: &str) -> Vec<(&'static str, String)> {
    offered(arch)
        .flat_map(|offered| {
            offered
                .files
                .iter()
                .map(|(path, _)| (*path, manager::installed_path(offered.name, path)))
        })
        .collect()
}

/// The store's file name for `arch`.
pub fn store_name(arch: &Arch) -> String {
    manager::store_file_name(release::VERSION, arch.name)
}

fn index_of(parts: &[(&Offered, Vec<Vec<u8>>)], arch: &str, data_start: u64) -> Index {
    let mut offset = data_start;
    let components = parts
        .iter()
        .map(|(offered, contents)| Component {
            name: offered.name.into(),
            title: offered.title.into(),
            summary: offered.summary.into(),
            on_remove: offered.on_remove.into(),
            needs: offered.needs.iter().map(|need| (*need).into()).collect(),
            files: offered
                .files
                .iter()
                .zip(contents)
                .map(|((path, _), bytes)| {
                    let file = File {
                        path: (*path).into(),
                        mode: 0o755,
                        offset,
                        length: bytes.len() as u64,
                        sha256: index::sha256(bytes),
                    };
                    offset += (bytes.len() as u64).div_ceil(SECTOR).max(1) * SECTOR;
                    file
                })
                .collect(),
        })
        .collect();
    Index {
        version: release::VERSION.into(),
        arch: arch.into(),
        components,
    }
}

/// The store image holding `parts` (each offered component with its files' bytes) for `arch`.
pub fn store_image(parts: &[(&Offered, Vec<Vec<u8>>)], arch: &str) -> Vec<u8> {
    let mut data_start = SECTOR * 2;
    let index = loop {
        let index = index_of(parts, arch, data_start);
        let text = index.write();
        let needed = Header::parse(&Header::of(text.as_bytes()))
            .map(|header| header.data_start())
            .unwrap_or(data_start);
        if needed == data_start {
            break index;
        }
        data_start = needed;
    };
    let text = index.write();
    let mut image = Header::of(text.as_bytes()).to_vec();
    image.extend_from_slice(text.as_bytes());
    for ((_, contents), component) in parts.iter().zip(&index.components) {
        for (bytes, file) in contents.iter().zip(&component.files) {
            image.resize(file.offset as usize, 0);
            image.extend_from_slice(bytes);
        }
    }
    image.resize(image.len().next_multiple_of(KERNEL_BLOCK), 0);
    image
}

/// Builds the store of `arch` into `target/components/` and returns its path.
pub fn build_store(arch: &Arch) -> Result<PathBuf, String> {
    let mut parts = Vec::new();
    for offered in offered(arch.name) {
        let mut contents = Vec::new();
        for (_, package) in offered.files {
            let program = crate::build_runtime_program(arch, package)?;
            contents.push(std::fs::read(&program).map_err(|e| format!("{program}: {e}"))?);
        }
        parts.push((offered, contents));
    }
    let image = store_image(&parts, arch.name);
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
            .map(|offered| (offered, vec![vec![0x7f; 700]]))
            .collect();
        let image = store_image(&parts, "x86_64");
        assert_eq!(image.len() % KERNEL_BLOCK, 0);
        assert_eq!(&image[..16], b"EGGOS-COMPONENTS");
        let header = Header::parse(&image).unwrap();
        let text = &image[SECTOR as usize..][..header.index_bytes as usize];
        let read = index::parse_index(&header, text, image.len() as u64).unwrap();
        let names: Vec<&str> = read.components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["windows-apps", "mac-apps"]);
        for component in &read.components {
            let file = &component.files[0];
            let bytes = &image[file.offset as usize..][..file.length as usize];
            assert_eq!(index::sha256(bytes), file.sha256);
            assert!(index::is_component_path(&file.path));
        }
    }

    #[test]
    fn the_base_links_each_layer_s_fixed_path_into_its_component_where_the_layer_runs() {
        assert_eq!(
            base_links("x86_64"),
            [
                (
                    "usr/lib/eggos/windows",
                    "/var/lib/eggos/components/windows-apps/root/usr/lib/eggos/windows".into()
                ),
                (
                    "usr/lib/eggos/macos",
                    "/var/lib/eggos/components/mac-apps/root/usr/lib/eggos/macos".into()
                ),
            ]
        );
        assert!(base_links("riscv64").is_empty());
        assert!(offered("riscv64").next().is_none());
    }
}
