//! The translation record a layer prints in its translate-only mode (D191, slice 39e2d): the
//! cache key, the bytes of code translated, the cache file's size, whether it is on the disk
//! and the refused sites, as `eggos_app_manifest::translation` writes them, for the installer
//! to report and keep with the app.

use alloc::vec;
use alloc::vec::Vec;
use eggos_app_manifest::translation::{self, Record, RefusedSite};
use eggos_translate::cache::{self, CacheFile};
use eggos_translate::image::SiteReason;

pub use eggos_app_manifest::translation::LAYER_OPTION;

const NOT_LOWERED: &str = "an instruction the translator does not lower yet";

/// Why a site was refused, in words.
pub fn reason(reason: SiteReason) -> &'static str {
    match reason {
        SiteReason::Refused(refusal) => refusal.description(),
        SiteReason::NotLowered => NOT_LOWERED,
    }
}

/// Where a layer's translation of a program came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The cache file already on the disk.
    Cached,
    /// Made now and stored in a cache file of this many bytes.
    Stored(usize),
    /// Made now; its cache file of this many bytes could not be stored.
    NotStored(usize),
}

/// The record of `file`, from `origin`; its first refused sites are listed by guest address.
pub fn record(file: &CacheFile, origin: Origin) -> Vec<u8> {
    let (cache_bytes, stored) = match origin {
        Origin::Cached => (cache::serialize(file).len(), true),
        Origin::Stored(bytes) => (bytes, true),
        Origin::NotStored(bytes) => (bytes, false),
    };
    let image = &file.image;
    let mut record = Record::new(
        file.key(),
        image.stats.guest_bytes,
        cache_bytes as u64,
        stored,
        image.refused_sites.len() as u64,
    );
    for site in &image.refused_sites {
        let listed = record.push_site(RefusedSite {
            address: file.image_base.wrapping_add(u64::from(site.rva)),
            reason: reason(site.reason).as_bytes(),
        });
        if !listed {
            break;
        }
    }
    let mut out = vec![0u8; translation::LARGEST];
    let length = translation::write(&record, &mut out).unwrap_or(0);
    out.truncate(length);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache_files;
    use eggos_translate::Refusal;
    use eggos_translate::cache::{input_digest, policy};
    use eggos_translate::codegen::CodegenMode;
    use eggos_translate::image::{ImageInput, Section, Seed, SeedOrigin, translate_image};

    const IMAGE_BASE: u64 = 0x1_4000_0000;
    const TEXT: u32 = 0x1000;

    fn translated(code: &[u8]) -> CacheFile {
        let sections = [Section {
            rva: TEXT,
            bytes: code,
            executable: true,
        }];
        let seeds = [Seed {
            rva: TEXT,
            origin: SeedOrigin::EntryPoint,
        }];
        let input = ImageInput {
            image_base: IMAGE_BASE,
            sections: &sections,
            seeds: &seeds,
        };
        let mode = CodegenMode::default();
        CacheFile {
            input_digest: input_digest(code),
            mode,
            image_base: IMAGE_BASE,
            image: translate_image(&input, mode).unwrap(),
        }
    }

    #[test]
    fn the_record_carries_the_key_sizes_and_the_refused_sites_by_guest_address() {
        let file = translated(&[0xc5, 0xf8, 0x77, 0xc3]);
        let bytes = cache::serialize(&file);
        let text = record(&file, Origin::Stored(bytes.len()));
        let parsed = translation::parse(&text).unwrap();
        assert_eq!(parsed.key, file.key());
        assert_eq!(parsed.cache_bytes, bytes.len() as u64);
        assert_eq!(parsed.code_bytes, file.image.stats.guest_bytes);
        assert!(parsed.stored);
        assert_eq!(parsed.refused, file.image.refused_sites.len() as u64);
        assert!(parsed.refused >= 1);
        assert_eq!(parsed.sites()[0].address, IMAGE_BASE + u64::from(TEXT));
        assert_eq!(
            parsed.sites()[0].reason,
            Refusal::Vex.description().as_bytes()
        );
    }

    #[test]
    fn the_record_names_the_cache_file_the_layers_write() {
        let file = translated(&[0xc3]);
        let key = file.key();
        let mut layer_path = cache_files::file_path(&policy::file_name(&key));
        assert_eq!(layer_path.pop(), Some(0));
        assert_eq!(&translation::cache_path(&key)[..], &layer_path[..]);
        assert_eq!(translation::CACHE_DIRECTORY, policy::DIRECTORY);
        assert_eq!(translation::CACHE_EXTENSION, policy::EXTENSION);
    }

    #[test]
    fn every_reason_fits_a_record() {
        assert!(translation::is_reason(NOT_LOWERED.as_bytes()));
        for refusal in [
            Refusal::Vex,
            Refusal::Evex,
            Refusal::Xop,
            Refusal::Mmx,
            Refusal::Sse3,
            Refusal::ThreeByteMap,
            Refusal::X87,
            Refusal::AddressSize,
            Refusal::Branch16,
            Refusal::StateSave,
            Refusal::System,
        ] {
            assert!(translation::is_reason(refusal.description().as_bytes()));
        }
    }
}
