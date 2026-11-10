//! `cargo xtask usb-image --arch <arch>` (D241): a disk image to write to a USB stick, which a
//! PC or an AArch64 UEFI machine boots from: a GUID partition table with one EFI system partition
//! holding EggOS's UEFI loader, the release kernel and an empty command line. EggOS runs from it
//! with its home in memory, and `disk install` puts it on the machine's own disk.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

const SECTOR_BYTES: u64 = 512;
const SYSTEM_PARTITION_BYTES: u64 = 256 << 20;
const TABLE_ROOM_BYTES: u64 = 2 << 20;

/// The UEFI target and the loader's name on the EFI system partition for `arch`.
fn uefi_target(arch: &str) -> Option<(&'static str, &'static str)> {
    match arch {
        "x86_64" => Some(("x86_64-unknown-uefi", "BOOTX64.EFI")),
        "aarch64" => Some(("aarch64-unknown-uefi", "BOOTAA64.EFI")),
        _ => None,
    }
}

fn build_loader(target: &str) -> Result<PathBuf, String> {
    crate::build_executable(
        &["build", "-p", "eggos-uefi-boot", "--target", target],
        &[],
        &format!("UEFI loader for {target}"),
    )
}

fn unique(seed: &[u8], salt: u8) -> [u8; 16] {
    let digest = crate::sha256::digest(&[seed, &[salt]].concat());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    eggos_gpt::random_guid(bytes)
}

/// Builds the image for `arch` and returns its path.
pub fn build(arch: &crate::Arch) -> Result<PathBuf, String> {
    let (target, loader_name) = uefi_target(arch.name)
        .ok_or_else(|| format!("{} machines do not boot through UEFI from a USB stick yet", arch.name))?;
    let kernel_path = crate::build_kernel(arch, None)?;
    let loader_path = build_loader(target)?;
    let kernel = std::fs::read(&kernel_path).map_err(|e| format!("{}: {e}", kernel_path.display()))?;
    let loader = std::fs::read(&loader_path).map_err(|e| format!("{}: {e}", loader_path.display()))?;
    let version = env!("CARGO_PKG_VERSION");
    let output = crate::target_directory()
        .join("image")
        .join(format!("eggos-{version}-{}-usb.img", arch.name));
    std::fs::create_dir_all(output.parent().unwrap_or(&output)).map_err(|e| e.to_string())?;
    let total_bytes = SYSTEM_PARTITION_BYTES + 2 * TABLE_ROOM_BYTES;
    let sectors = total_bytes / SECTOR_BYTES;
    let layout = eggos_gpt::Layout::new(sectors, SECTOR_BYTES).ok_or("the image is too small for a partition table")?;
    let first = layout.aligned(layout.first_usable);
    let last = (first + SYSTEM_PARTITION_BYTES / SECTOR_BYTES - 1).min(layout.last_usable);
    let seed = [version.as_bytes(), arch.name.as_bytes(), &(kernel.len() as u64).to_le_bytes()].concat();
    let partitions = [eggos_gpt::NewPartition {
        kind: eggos_gpt::EFI_SYSTEM_PARTITION,
        unique: unique(&seed, 1),
        first,
        last,
        name: "EggOS",
    }];
    let table = eggos_gpt::NewTable::new(layout, unique(&seed, 0), &partitions).ok_or("the partition does not fit")?;
    let mut file = File::create(&output).map_err(|e| format!("{}: {e}", output.display()))?;
    file.set_len(total_bytes).map_err(|e| e.to_string())?;
    let mut put = |offset: u64, bytes: &[u8]| -> std::io::Result<()> {
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)
    };
    let [primary_header, primary_entries, backup_entries, backup_header] = table.places();
    let mut header = [0u8; 512];
    put(0, &table.mbr).map_err(|e| e.to_string())?;
    header[..eggos_gpt::HEADER_BYTES].copy_from_slice(&table.primary_header);
    put(primary_header * SECTOR_BYTES, &header).map_err(|e| e.to_string())?;
    put(primary_entries * SECTOR_BYTES, &table.entries).map_err(|e| e.to_string())?;
    put(backup_entries * SECTOR_BYTES, &table.entries).map_err(|e| e.to_string())?;
    header[..eggos_gpt::HEADER_BYTES].copy_from_slice(&table.backup_header);
    put(backup_header * SECTOR_BYTES, &header).map_err(|e| e.to_string())?;
    let loader_file = format!("EFI/BOOT/{loader_name}");
    let files = [
        eggos_fat32::File {
            path: &loader_file,
            bytes: &loader,
        },
        eggos_fat32::File {
            path: "EGGOS/KERNEL.ELF",
            bytes: &kernel,
        },
        eggos_fat32::File {
            path: "EGGOS/CMDLINE.TXT",
            bytes: b"\n",
        },
    ];
    let volume_sectors = u32::try_from(last - first + 1).map_err(|_| "the partition is too large")?;
    let volume_id = u32::from_le_bytes(unique(&seed, 2)[..4].try_into().unwrap_or_default());
    let base = first * SECTOR_BYTES;
    eggos_fat32::write_volume(volume_sectors, first as u32, volume_id, &files, &mut |offset, bytes| {
        put(base + offset, bytes)
    })
    .map_err(|error| format!("writing the EFI system partition failed: {error:?}"))?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x86_64_and_aarch64_have_uefi_loaders_and_riscv64_does_not_yet() {
        assert_eq!(uefi_target("x86_64").map(|(_, name)| name), Some("BOOTX64.EFI"));
        assert_eq!(uefi_target("aarch64").map(|(_, name)| name), Some("BOOTAA64.EFI"));
        assert_eq!(uefi_target("riscv64"), None);
    }
}
