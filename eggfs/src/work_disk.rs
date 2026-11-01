use crate::{Disk, Error, Kind, Volume, kind_of};

/// The regular file in a volume's root directory that makes the volume a work disk (D166): a
/// host's folder carried into EggOS, mounted at `/work`, rather than the disk that holds
/// `/home`. Nothing inside EggOS can reach a volume's root, so only the host makes one.
pub const WORK_DISK_MARKER: &[u8] = b".eggos-work-disk";

/// The top-level directory of a work disk, which EggOS mounts at `/work`.
pub const WORK_FOLDER: &[u8] = b"work";

/// Whether `volume` is a work disk: its root holds a regular file named [`WORK_DISK_MARKER`].
pub fn is_work_disk<D: Disk>(volume: &mut Volume<D>) -> Result<bool, Error> {
    let root = volume.root();
    Ok(volume
        .lookup(root, WORK_DISK_MARKER)?
        .is_some_and(|node| kind_of(node) == Kind::Regular))
}

/// Makes `volume` a work disk: writes the empty [`WORK_DISK_MARKER`] (permissions 0444) and
/// makes [`WORK_FOLDER`] (0755), both at time `now`, and returns the folder's node. Refuses
/// with [`Error::Exists`] a volume that already has either name in its root.
pub fn make_work_disk<D: Disk>(volume: &mut Volume<D>, now: u64) -> Result<u64, Error> {
    let root = volume.root();
    volume.create(root, WORK_DISK_MARKER, Kind::Regular, 0o444, now)?;
    volume.create(root, WORK_FOLDER, Kind::Directory, 0o755, now)
}

/// The node of `volume`'s [`WORK_FOLDER`]: [`Error::NotFound`] if the volume is not a work
/// disk or lacks the folder, [`Error::NotDirectory`] if the name is a file.
pub fn work_folder<D: Disk>(volume: &mut Volume<D>) -> Result<u64, Error> {
    if !is_work_disk(volume)? {
        return Err(Error::NotFound);
    }
    let root = volume.root();
    match volume.lookup(root, WORK_FOLDER)? {
        Some(node) if kind_of(node) == Kind::Directory => Ok(node),
        Some(_) => Err(Error::NotDirectory),
        None => Err(Error::NotFound),
    }
}
