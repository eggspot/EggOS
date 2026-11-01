//! Translating an app ahead of its first launch (D191, slice 39e2d): an x64 Windows or x86_64
//! macOS program on an Arm computer is translated right after it is installed, by its layer
//! in its translate-only mode (`<layer> --translate <program>`), which stores the cache file
//! and prints the translation record (`eggos_app_manifest::translation`). The installer keeps
//! the record as `app.translation` in the app's folder: `app show` lists its refused sites, and
//! uninstalling removes the cache file it names. The installer never links the translator;
//! when the layer is missing or the translation fails, the first launch translates as before.

use crate::architecture::Architecture;
use crate::files::{Files, Path};
use crate::installer::{self, APPLICATIONS, Plan};
use crate::program;
use eggos_app_manifest::translation::{self, Record};
use eggos_runtime::errno::Errno;

const RECORD_PERMISSIONS: u32 = 0o644;

/// The bytes a record is read into: one more than the largest, to tell a longer file.
pub type RecordBuffer = [u8; translation::LARGEST + 1];

/// What translating an installed program ahead of its first launch runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadOfLaunch {
    /// The layer, started as `<layer> --translate <program>`.
    pub layer: &'static [u8],
    /// The installed program.
    pub program: Path,
}

/// What translating `plan`'s program on `machine` runs; `None` when the program runs natively.
pub fn ahead_of_launch(plan: &Plan, machine: Architecture) -> Option<AheadOfLaunch> {
    let built = plan.program();
    if !program::runs_translated(built.source, built.architecture, machine) {
        return None;
    }
    let layer = installer::layer_path(built.source)?;
    let run = plan.run();
    let program = match run.starts_with(b"/") {
        true => Path::from(run)?,
        false => plan.folder().join(run)?,
    };
    Some(AheadOfLaunch { layer, program })
}

fn record_path(name: &[u8]) -> Option<Path> {
    Path::from(APPLICATIONS)?
        .join(name)?
        .join(translation::FILE_NAME.as_bytes())
}

/// Keeps `printed`, what the layer printed, as the app `name`'s record; refused with `EINVAL`
/// when it is no record.
pub fn keep_record(files: &mut impl Files, name: &[u8], printed: &[u8]) -> Result<(), Errno> {
    if translation::parse(printed).is_none() {
        return Err(Errno::EINVAL);
    }
    let path = record_path(name).ok_or(Errno::ENAMETOOLONG)?;
    let file = files.create(path.as_bytes(), RECORD_PERMISSIONS)?;
    let written = files
        .write(file, printed)
        .and_then(|()| files.sync_file(file));
    files.close(file);
    written
}

/// The record kept for the app `name`, read into `buffer`; its length when it is a record.
pub fn read_record(
    files: &mut impl Files,
    name: &[u8],
    buffer: &mut RecordBuffer,
) -> Option<usize> {
    let path = record_path(name)?;
    let file = files.open(path.as_bytes()).ok()?;
    let mut filled = 0;
    let read = loop {
        match files.read(file, &mut buffer[filled..]) {
            Ok(0) => break Some(filled),
            Ok(count) => {
                filled += count;
                if filled == buffer.len() {
                    break None;
                }
            }
            Err(_) => break None,
        }
    };
    files.close(file);
    let length = read?;
    translation::parse(&buffer[..length]).map(|_| length)
}

/// Removes the cache file `record` names; one already gone is no error.
pub fn remove_cache(files: &mut impl Files, record: &Record<'_>) {
    let _ = files.remove_file(&record.cache_path());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFiles;
    use eggos_app_manifest::translation::RefusedSite;

    fn printed() -> Vec<u8> {
        let mut record = Record::new([7; 32], 100, 9000, true, 1);
        assert!(record.push_site(RefusedSite {
            address: 0x1000,
            reason: b"an SSE3 instruction",
        }));
        let mut out = [0u8; translation::LARGEST];
        let length = translation::write(&record, &mut out).unwrap();
        out[..length].to_vec()
    }

    #[test]
    fn a_kept_record_reads_back_and_names_its_cache_file() {
        let mut files = FakeFiles::new();
        files.directory("/Applications/Paint");
        keep_record(&mut files, b"Paint", &printed()).unwrap();
        let mut buffer: RecordBuffer = [0; translation::LARGEST + 1];
        let length = read_record(&mut files, b"Paint", &mut buffer).unwrap();
        let record = translation::parse(&buffer[..length]).unwrap();
        assert_eq!(record.refused, 1);
        assert_eq!(record.sites()[0].reason, b"an SSE3 instruction");
        let cache = record.cache_path();
        let cache = core::str::from_utf8(&cache).unwrap();
        files.file(cache, b"cached code");
        remove_cache(&mut files, &record);
        assert!(files.find(cache).is_none());
        remove_cache(&mut files, &record);
    }

    #[test]
    fn uninstalling_removes_the_cache_file_the_record_names() {
        let mut files = FakeFiles::new();
        files.directory("/Applications/Paint");
        files.file(
            "/Applications/Paint/app.manifest",
            b"eggos-app 1\nname=Paint\nsource=windows\nrun=paint.exe\narchitecture=x86_64\n",
        );
        files.file("/Applications/Paint/paint.exe", b"MZ");
        keep_record(&mut files, b"Paint", &printed()).unwrap();
        let cache = translation::cache_path(&[7; 32]);
        let cache = core::str::from_utf8(&cache).unwrap();
        files.file(cache, b"cached code");
        installer::uninstall(&mut files, b"Paint").unwrap();
        assert!(files.find(cache).is_none());
        assert!(files.find("/Applications/Paint").is_none());
    }

    #[test]
    fn what_is_no_record_is_not_kept() {
        let mut files = FakeFiles::new();
        files.directory("/Applications/Paint");
        assert_eq!(
            keep_record(&mut files, b"Paint", b"windows: x.exe: out of memory\n"),
            Err(Errno::EINVAL)
        );
        let mut buffer: RecordBuffer = [0; translation::LARGEST + 1];
        assert_eq!(read_record(&mut files, b"Paint", &mut buffer), None);
    }
}
