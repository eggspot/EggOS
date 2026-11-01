use super::policy::{
    CachedFile, MAX_CAP_BYTES, STALE_TEMPORARY_SECONDS, cap_bytes, evictions, file_name, key_of,
    removals_for_app, temporary_name,
};
use super::*;
use crate::image::{ImageInput, Section, Seed, SeedOrigin, translate_image};

const BASE: u64 = 0x1_4000_0000;

const MODE: CodegenMode = CodegenMode {
    hardware_tso: false,
    lse: true,
    lrcpc: false,
    flagm: true,
};

fn sample() -> CacheFile {
    let code = [
        0x31, 0xC0, 0xFF, 0xC0, 0x83, 0xF8, 0x0A, 0x75, 0xF9, 0xE8, 0x01, 0x00, 0x00, 0x00, 0xC3,
        0xC5, 0xF8, 0x77, 0xC3,
    ];
    let sections = [Section {
        rva: 0x1000,
        bytes: &code,
        executable: true,
    }];
    let seeds = [Seed {
        rva: 0x1000,
        origin: SeedOrigin::EntryPoint,
    }];
    let input = ImageInput {
        image_base: BASE,
        sections: &sections,
        seeds: &seeds,
    };
    CacheFile {
        input_digest: input_digest(&code),
        mode: MODE,
        image_base: BASE,
        image: translate_image(&input, MODE).unwrap(),
    }
}

fn reseal(bytes: &mut [u8]) {
    let body = bytes.len() - 32;
    let mut hash = Sha256::new();
    hash.update(&bytes[..body]);
    let checksum = hash.finish();
    bytes[body..].copy_from_slice(&checksum);
}

#[test]
fn a_cache_file_round_trips() {
    let file = sample();
    assert!(!file.image.refused_sites.is_empty());
    assert!(!file.image.guest_address_literals.is_empty());
    let bytes = serialize(&file);
    assert_eq!(&bytes[..8], b"EGGOSTX\0");
    assert!(bytes[CODE_OFFSET..].starts_with(&file.image.code));
    assert_eq!(parse(&bytes), Ok(file));
}

#[test]
fn every_truncation_is_rejected() {
    let bytes = serialize(&sample());
    for length in 0..bytes.len() {
        assert_eq!(
            parse(&bytes[..length]),
            Err(CacheError::Truncated),
            "{length}"
        );
    }
}

#[test]
fn another_version_or_format_is_rejected_before_the_checksum() {
    let bytes = serialize(&sample());
    let mut translator = bytes.clone();
    translator[12..16].copy_from_slice(&(TRANSLATOR_VERSION + 1).to_le_bytes());
    assert_eq!(parse(&translator), Err(CacheError::WrongTranslator));
    let mut format = bytes.clone();
    format[8..10].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert_eq!(parse(&format), Err(CacheError::WrongFormat));
    let mut magic = bytes;
    magic[0] = b'X';
    assert_eq!(parse(&magic), Err(CacheError::BadMagic));
}

#[test]
fn damage_is_caught() {
    let bytes = serialize(&sample());
    let mut flipped = bytes.clone();
    flipped[CODE_OFFSET + 4] ^= 1;
    assert_eq!(parse(&flipped), Err(CacheError::Checksum));
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(parse(&longer), Err(CacheError::Malformed));
    let mut other_mode = bytes.clone();
    other_mode[10] ^= 1;
    reseal(&mut other_mode);
    assert_eq!(parse(&other_mode), Err(CacheError::KeyMismatch));
    let mut unknown_mode = bytes.clone();
    unknown_mode[10] = 0x10;
    reseal(&mut unknown_mode);
    assert_eq!(parse(&unknown_mode), Err(CacheError::Malformed));
    let mut padding = bytes;
    padding[HEADER_BYTES] = 1;
    reseal(&mut padding);
    assert_eq!(parse(&padding), Err(CacheError::Malformed));
}

#[test]
fn the_key_depends_on_input_mode_and_translator() {
    let digest = input_digest(b"program");
    let key = cache_key(&digest, MODE);
    assert_eq!(key, cache_key(&digest, MODE));
    assert_ne!(key, cache_key(&input_digest(b"programs"), MODE));
    let other = CodegenMode {
        lrcpc: true,
        ..MODE
    };
    assert_ne!(key, cache_key(&digest, other));
    for bits in 0..16 {
        assert_eq!(mode_from_bits(bits).map(mode_bits), Some(bits));
    }
    assert_eq!(mode_from_bits(0x10), None);
}

#[test]
fn names_come_from_keys_and_back() {
    let key = [0xA5u8; 32];
    let name = file_name(&key);
    assert_eq!(name.len(), 64 + 3);
    assert!(name.starts_with("a5a5"));
    assert_eq!(key_of(&name), Some(key));
    assert_eq!(key_of("notes.tx"), None);
    assert_eq!(key_of(&name.to_uppercase()), None);
    let temporary = temporary_name(&key, 0x42);
    assert_eq!(key_of(&temporary), None);
    assert!(temporary.ends_with(".42.tmp"));
}

#[test]
fn the_cap_is_two_gibibytes_or_five_percent() {
    assert_eq!(cap_bytes(100 << 30), MAX_CAP_BYTES);
    assert_eq!(cap_bytes(10 << 30), 512 << 20);
}

fn file(key: u8, bytes: u64, last_launch: u64, in_use: bool) -> CachedFile {
    CachedFile {
        name: file_name(&[key; 32]),
        bytes,
        last_launch,
        in_use,
    }
}

#[test]
fn eviction_removes_the_least_recently_launched_files_not_in_use() {
    let now = 100_000;
    let files = [
        file(1, 400, 50, false),
        file(2, 400, 10, true),
        file(3, 400, 30, false),
        file(4, 400, 90, false),
        CachedFile {
            name: "readme.txt".into(),
            bytes: 10_000,
            last_launch: 0,
            in_use: false,
        },
        CachedFile {
            name: temporary_name(&[9; 32], 7),
            bytes: 100,
            last_launch: now - STALE_TEMPORARY_SECONDS,
            in_use: false,
        },
        CachedFile {
            name: temporary_name(&[8; 32], 7),
            bytes: 100,
            last_launch: now - 1,
            in_use: false,
        },
    ];
    assert_eq!(
        evictions(&files, 1000, 200, now),
        [
            temporary_name(&[9; 32], 7),
            file_name(&[3; 32]),
            file_name(&[1; 32]),
        ]
    );
    assert_eq!(
        evictions(&files, 1600, 0, now),
        [temporary_name(&[9; 32], 7)]
    );
    assert_eq!(
        evictions(&files, 0, 0, now).len(),
        4,
        "everything not in use and ours"
    );
}

#[test]
fn removing_an_app_removes_its_own_files() {
    let files = [
        file(1, 1, 1, false),
        file(2, 1, 1, false),
        file(3, 1, 1, true),
    ];
    assert_eq!(
        removals_for_app(&files, &[[1; 32], [3; 32]]),
        [file_name(&[1; 32])]
    );
}
