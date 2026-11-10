use alloc::string::String;
use alloc::vec::Vec;

use eggos_commands::COMMANDS;

use crate::command::{self, Console};
use crate::hive::{Hive, Key, Value};
use crate::path::{KeyPath, Root, utf16};
use crate::registry::{HiveFile, Registry, Store, StoreError, user_sid};
use crate::value::{
    REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_QWORD, REG_SZ, data_text, parse_data,
    string_data, strings_data, type_from_name,
};
use crate::{Error, MAX_DEPTH, MAX_HIVE_BYTES};

#[derive(Debug, Default)]
struct FakeStore {
    files: [Option<Vec<u8>>; 2],
    temporaries: [Option<Vec<u8>>; 2],
    damaged: [Option<Vec<u8>>; 2],
    stamps: [u64; 2],
    user_id: u32,
    clock: u64,
    reads: usize,
    fail_rename: bool,
    disk_full: bool,
}

fn slot(file: HiveFile) -> usize {
    match file {
        HiveFile::User => 0,
        HiveFile::Machine => 1,
    }
}

impl Store for FakeStore {
    fn read(&mut self, file: HiveFile) -> Result<Option<Vec<u8>>, StoreError> {
        self.reads += 1;
        Ok(self.files[slot(file)].clone())
    }

    fn stamp(&mut self, file: HiveFile) -> Option<u64> {
        self.files[slot(file)]
            .as_ref()
            .map(|_| self.stamps[slot(file)])
    }

    fn write_temporary(&mut self, file: HiveFile, bytes: &[u8]) -> Result<(), StoreError> {
        if self.disk_full {
            return Err(StoreError::DiskFull);
        }
        self.temporaries[slot(file)] = Some(bytes.to_vec());
        Ok(())
    }

    fn replace_with_temporary(&mut self, file: HiveFile) -> Result<(), StoreError> {
        if self.fail_rename {
            return Err(StoreError::Failed);
        }
        let index = slot(file);
        self.files[index] = self.temporaries[index].take();
        self.stamps[index] += 1;
        Ok(())
    }

    fn set_aside(&mut self, file: HiveFile) -> Result<(), StoreError> {
        let index = slot(file);
        if let Some(bytes) = self.files[index].take() {
            self.damaged[index] = Some(bytes);
            self.stamps[index] += 1;
        }
        Ok(())
    }

    fn user_id(&mut self) -> u32 {
        self.user_id
    }

    fn now(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

#[derive(Debug, Default)]
struct FakeConsole {
    out: Vec<u8>,
    err: Vec<u8>,
    answer: Option<bool>,
    asked: Vec<String>,
    coded: bool,
}

impl Console for FakeConsole {
    fn out(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    fn err(&mut self, bytes: &[u8]) {
        self.err.extend_from_slice(bytes);
    }

    fn confirm(&mut self, question: &str) -> Option<bool> {
        self.asked.push(String::from(question));
        self.answer
    }

    fn wants_coded_errors(&self) -> bool {
        self.coded
    }
}

fn key(text: &str) -> KeyPath {
    KeyPath::parse(text).unwrap()
}

fn sample() -> Registry<FakeStore> {
    let mut registry = Registry::new(FakeStore::default());
    let autohotkey = key("HKCU\\Software\\AutoHotkey");
    registry
        .create_key(&autohotkey.join(&utf16("Settings")).unwrap())
        .unwrap();
    registry
        .set_value(
            &autohotkey,
            &utf16("Version"),
            REG_SZ,
            string_data("2.0.19"),
        )
        .unwrap();
    registry
        .set_value(
            &autohotkey,
            &utf16("Count"),
            REG_DWORD,
            42u32.to_le_bytes().to_vec(),
        )
        .unwrap();
    registry
}

fn run(
    registry: &mut Registry<FakeStore>,
    line: &str,
    answer: Option<bool>,
) -> (i32, String, String) {
    let words: Vec<&str> = line.split(' ').collect();
    let mut console = FakeConsole {
        answer,
        ..FakeConsole::default()
    };
    let status = command::run(registry, &mut console, &words);
    (
        status,
        String::from_utf8(console.out).unwrap(),
        String::from_utf8(console.err).unwrap(),
    )
}

#[test]
fn a_hive_survives_its_file_and_keeps_subkeys_sorted_ignoring_case() {
    let mut hive = Hive::default();
    for name in ["zeta", "Alpha", "beta"] {
        let index = hive.root.subkey_index(&utf16(name)).unwrap_err();
        hive.root.subkeys.insert(index, Key::named(&utf16(name), 7));
    }
    hive.root.subkeys[1].values.push(Value {
        name: utf16("Count"),
        kind: REG_DWORD,
        data: 5u32.to_le_bytes().to_vec(),
    });
    hive.root.subkeys[1]
        .subkeys
        .push(Key::named(&utf16("Inner"), 9));
    let names: Vec<String> = hive
        .root
        .subkeys
        .iter()
        .map(|key| String::from_utf16(&key.name).unwrap())
        .collect();
    assert_eq!(names, ["Alpha", "beta", "zeta"]);
    let bytes = hive.encode().unwrap();
    assert_eq!(Hive::decode(&bytes), Ok(hive.clone()));
    assert!(hive.root.subkey(&utf16("ALPHA")).is_some());
    assert_eq!(hive.root.count_below(), (4, 1));
}

#[test]
fn a_damaged_hive_file_is_an_error_at_every_length_and_byte() {
    let mut registry = sample();
    let bytes = registry.store().files[0].clone().unwrap();
    for length in 0..bytes.len() {
        assert_eq!(
            Hive::decode(&bytes[..length]),
            Err(Error::HiveDamaged),
            "{length}"
        );
    }
    for index in 0..bytes.len() {
        let mut damaged = bytes.clone();
        damaged[index] ^= 0x41;
        assert!(Hive::decode(&damaged).is_err(), "{index}");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(Hive::decode(&longer), Err(Error::HiveDamaged));
}

fn with_payload(payload: &[u8]) -> Vec<u8> {
    let mut file = b"EGGREGv1".to_vec();
    file.extend_from_slice(&1u32.to_le_bytes());
    file.extend_from_slice(&crate::hive::crc32(payload).to_le_bytes());
    file.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    file.extend_from_slice(payload);
    file
}

fn key_record(depth: u16, name: &str) -> Vec<u8> {
    let mut record = depth.to_le_bytes().to_vec();
    let units = utf16(name);
    record.extend_from_slice(&(units.len() as u16).to_le_bytes());
    for unit in units {
        record.extend_from_slice(&unit.to_le_bytes());
    }
    record.extend_from_slice(&0u64.to_le_bytes());
    record.extend_from_slice(&0u32.to_le_bytes());
    record
}

#[test]
fn checked_files_with_bad_structure_are_refused() {
    let tree = |records: &[(u16, &str)]| {
        let payload: Vec<u8> = records
            .iter()
            .flat_map(|(depth, name)| key_record(*depth, name))
            .collect();
        Hive::decode(&with_payload(&payload))
    };
    assert!(tree(&[(0, ""), (1, "a"), (2, "b"), (1, "c")]).is_ok());
    assert_eq!(
        tree(&[(0, ""), (1, "b"), (1, "a")]),
        Err(Error::HiveDamaged)
    );
    assert_eq!(
        tree(&[(0, ""), (1, "a"), (1, "A")]),
        Err(Error::HiveDamaged)
    );
    assert_eq!(tree(&[(0, ""), (2, "a")]), Err(Error::HiveDamaged));
    assert_eq!(tree(&[(1, "a")]), Err(Error::HiveDamaged));
    assert_eq!(tree(&[(0, "named")]), Err(Error::HiveDamaged));
    assert_eq!(tree(&[(0, ""), (0, "")]), Err(Error::HiveDamaged));
    assert_eq!(tree(&[(0, ""), (1, "a\\b")]), Err(Error::HiveDamaged));
    assert_eq!(tree(&[(0, ""), (1, "")]), Err(Error::HiveDamaged));
    let deep: Vec<(u16, &str)> = (0..=MAX_DEPTH as u16 + 1)
        .map(|depth| (depth, if depth == 0 { "" } else { "k" }))
        .collect();
    assert_eq!(tree(&deep), Err(Error::HiveDamaged));
    assert!(tree(&deep[..=MAX_DEPTH]).is_ok());
}

#[test]
fn writes_go_through_a_temporary_file_so_a_failed_rename_keeps_the_old_hive() {
    let mut registry = sample();
    let before = registry.store().files[0].clone();
    registry.store().fail_rename = true;
    let path = key("HKCU\\Software\\AutoHotkey");
    assert_eq!(
        registry.set_value(&path, &utf16("Version"), REG_SZ, string_data("3")),
        Err(Error::WriteFailed)
    );
    assert_eq!(registry.store().files[0], before);
    let value = registry.value(&path, &utf16("version")).unwrap();
    assert_eq!(data_text(value.kind, &value.data), "2.0.19");
    registry.store().fail_rename = false;
    registry.store().disk_full = true;
    assert_eq!(registry.create_key(&key("HKCU\\New")), Err(Error::DiskFull));
    assert!(!registry.key_exists(&key("HKCU\\New")).unwrap());
}

#[test]
fn another_process_s_write_is_read_when_the_stamp_changes() {
    let mut registry = sample();
    let mut other = Registry::new(FakeStore {
        files: registry.store().files.clone(),
        stamps: registry.store().stamps,
        ..FakeStore::default()
    });
    other.create_key(&key("HKCU\\Software\\Other")).unwrap();
    let reads = registry.store().reads;
    assert!(!registry.key_exists(&key("HKCU\\Software\\Other")).unwrap());
    assert_eq!(registry.store().reads, reads);
    registry.store().files = other.store().files.clone();
    registry.store().stamps[0] += 10;
    assert!(registry.key_exists(&key("HKCU\\Software\\Other")).unwrap());
}

#[test]
fn the_machine_s_hive_is_read_only_but_to_root() {
    let mut registry = Registry::new(FakeStore {
        user_id: 1000,
        ..FakeStore::default()
    });
    assert_eq!(
        registry.create_key(&key("HKLM\\Software\\Vendor")),
        Err(Error::ReadOnlyKey)
    );
    assert_eq!(registry.reset(HiveFile::Machine), Err(Error::ReadOnlyKey));
    registry.store().user_id = 0;
    assert_eq!(
        registry.create_key(&key("HKLM\\Software\\Vendor")),
        Ok(true)
    );
    assert_eq!(
        registry.create_key(&key("HKLM\\Software\\vendor")),
        Ok(false)
    );
    registry.store().user_id = 1000;
    assert!(registry.key_exists(&key("HKLM\\Software\\Vendor")).unwrap());
    assert_eq!(
        registry.delete_key(&key("HKLM\\Software\\Vendor"), true),
        Err(Error::ReadOnlyKey)
    );
}

#[test]
fn classes_root_merges_the_user_s_classes_over_the_machine_s_and_writes_the_user_s() {
    let mut registry = Registry::new(FakeStore::default());
    let machine = key("HKLM\\Software\\Classes\\.txt");
    registry.create_key(&machine).unwrap();
    registry
        .set_value(&machine, &[], REG_SZ, string_data("txtfile"))
        .unwrap();
    registry
        .create_key(&key("HKLM\\Software\\Classes\\CLSID"))
        .unwrap();
    registry.store().user_id = 1000;
    let merged = key("HKCR\\.txt");
    assert_eq!(
        data_text(REG_SZ, &registry.value(&merged, &[]).unwrap().data),
        "txtfile"
    );
    registry
        .set_value(&merged, &[], REG_SZ, string_data("mine"))
        .unwrap();
    assert_eq!(
        data_text(REG_SZ, &registry.value(&merged, &[]).unwrap().data),
        "mine"
    );
    assert_eq!(
        data_text(REG_SZ, &registry.value(&machine, &[]).unwrap().data),
        "txtfile"
    );
    registry
        .create_key(&key("HKCU\\Software\\Classes\\.egg"))
        .unwrap();
    let names: Vec<String> = registry
        .subkeys(&KeyPath::root(Root::ClassesRoot))
        .unwrap()
        .iter()
        .map(|name| String::from_utf16(name).unwrap())
        .collect();
    assert_eq!(names, [".egg", ".txt", "CLSID"]);
}

#[test]
fn users_holds_the_user_s_hive_under_its_sid() {
    let mut registry = sample();
    let sid = user_sid(0);
    assert_eq!(sid, "S-1-5-21-0-0-0-500");
    assert_eq!(
        registry.subkeys(&KeyPath::root(Root::Users)).unwrap(),
        [utf16(&sid)]
    );
    let through_users = key(&alloc::format!("HKU\\{sid}\\Software\\AutoHotkey"));
    assert!(registry.value(&through_users, &utf16("Count")).is_ok());
    assert!(
        !registry
            .key_exists(&key("HKU\\S-1-5-18\\Software"))
            .unwrap()
    );
    assert_eq!(
        registry.delete_key(&key(&alloc::format!("HKU\\{sid}")), true),
        Err(Error::RootKey)
    );
}

#[test]
fn limits_on_names_depth_and_size_hold() {
    let mut registry = Registry::new(FakeStore::default());
    let mut deep = KeyPath::root(Root::CurrentUser);
    deep.names = (0..MAX_DEPTH).map(|_| utf16("k")).collect();
    assert_eq!(registry.create_key(&deep), Ok(true));
    deep.names.push(utf16("k"));
    assert_eq!(registry.create_key(&deep), Err(Error::KeyTooDeep));
    assert_eq!(KeyPath::parse(&"x".repeat(256)), Err(Error::NoSuchKey));
    assert_eq!(
        KeyPath::parse(&alloc::format!("HKCU\\{}", "x".repeat(256))),
        Err(Error::BadName)
    );
    let root = KeyPath::root(Root::CurrentUser);
    assert_eq!(
        registry.set_value(
            &root,
            &[],
            REG_BINARY,
            alloc::vec![0; crate::MAX_DATA_BYTES + 1]
        ),
        Err(Error::ValueTooLarge)
    );
    for index in 0..(MAX_HIVE_BYTES / crate::MAX_DATA_BYTES) {
        let name = utf16(&alloc::format!("blob{index}"));
        let result = registry.set_value(
            &root,
            &name,
            REG_BINARY,
            alloc::vec![7; crate::MAX_DATA_BYTES],
        );
        if index + 1 < MAX_HIVE_BYTES / crate::MAX_DATA_BYTES {
            assert_eq!(result, Ok(()), "{index}");
        } else {
            assert_eq!(result, Err(Error::HiveFull));
        }
    }
}

#[test]
fn deleting_a_key_with_subkeys_needs_the_tree() {
    let mut registry = sample();
    let path = key("HKCU\\Software\\AutoHotkey");
    assert_eq!(registry.delete_key(&path, false), Err(Error::HasSubkeys));
    assert_eq!(registry.count_below(&path), Ok((1, 0)));
    registry.clear_key(&path).unwrap();
    assert!(registry.values(&path).unwrap().is_empty());
    assert_eq!(registry.delete_key(&path, false), Ok(()));
    assert_eq!(registry.delete_key(&path, true), Err(Error::NoSuchKey));
    assert_eq!(
        registry.delete_key(&KeyPath::root(Root::CurrentUser), true),
        Err(Error::RootKey)
    );
}

#[test]
fn data_is_shown_and_typed_in_regedit_s_words() {
    assert_eq!(
        data_text(REG_DWORD, &42u32.to_le_bytes()),
        "0x0000002a (42)"
    );
    assert_eq!(
        data_text(REG_QWORD, &42u64.to_le_bytes()),
        "0x000000000000002a (42)"
    );
    assert_eq!(data_text(REG_BINARY, &[0x4a, 0, 0xff, 0x1e]), "4a 00 ff 1e");
    assert_eq!(data_text(REG_SZ, &[]), "(zero-length value)");
    assert_eq!(
        data_text(REG_MULTI_SZ, &strings_data(&["first", "second"])),
        "first \u{b7} second"
    );
    assert_eq!(data_text(REG_EXPAND_SZ, &string_data("%HOME%")), "%HOME%");
    assert_eq!(data_text(99, &[1, 2]), "01 02");
    assert_eq!(
        parse_data(REG_DWORD, &["0x2A"]),
        Ok(42u32.to_le_bytes().to_vec())
    );
    assert_eq!(parse_data(REG_DWORD, &["4294967296"]), Err(Error::BadData));
    assert_eq!(parse_data(REG_DWORD, &["-1"]), Err(Error::BadData));
    assert_eq!(
        parse_data(REG_BINARY, &["4a 00", "ff"]),
        Ok([0x4a, 0, 0xff].to_vec())
    );
    assert_eq!(parse_data(REG_BINARY, &["4"]), Err(Error::BadData));
    assert_eq!(parse_data(REG_MULTI_SZ, &["a", ""]), Err(Error::BadData));
    assert_eq!(type_from_name("dword"), Some(REG_DWORD));
    assert_eq!(type_from_name("REG_MULTI_SZ"), Some(REG_MULTI_SZ));
    assert_eq!(type_from_name("REG_LINK"), None);
}

#[test]
fn the_command_reads_writes_and_deletes_with_confirmation() {
    let mut registry = sample();
    assert_eq!(
        run(
            &mut registry,
            "registry get HKCU\\Software\\AutoHotkey Count",
            None
        ),
        (
            0,
            String::from("Count  REG_DWORD  0x0000002a (42)\n"),
            String::new()
        )
    );
    assert_eq!(
        run(
            &mut registry,
            "registry set HKCU\\Software\\AutoHotkey @ --type multi_sz one two",
            None
        )
        .0,
        0
    );
    let line = "registry get --json HKCU\\Software\\AutoHotkey";
    let (status, out, _) = run(&mut registry, line, None);
    assert_eq!(status, 0);
    assert_eq!(
        out,
        "{\"key\":\"HKEY_CURRENT_USER\\\\Software\\\\AutoHotkey\",\"subkey\":\"Settings\"}\n\
         {\"key\":\"HKEY_CURRENT_USER\\\\Software\\\\AutoHotkey\",\"name\":\"\",\"type\":\"REG_MULTI_SZ\",\"data\":[\"one\",\"two\"]}\n\
         {\"key\":\"HKEY_CURRENT_USER\\\\Software\\\\AutoHotkey\",\"name\":\"Version\",\"type\":\"REG_SZ\",\"data\":\"2.0.19\"}\n\
         {\"key\":\"HKEY_CURRENT_USER\\\\Software\\\\AutoHotkey\",\"name\":\"Count\",\"type\":\"REG_DWORD\",\"data\":42}\n"
    );
    let line = "registry delete HKCU\\Software\\AutoHotkey";
    let (status, _, err) = run(&mut registry, line, None);
    assert_eq!(status, 1);
    assert!(err.contains("--yes"), "{err}");
    let mut console = FakeConsole {
        answer: Some(false),
        ..FakeConsole::default()
    };
    let words = ["registry", "delete", "HKCU\\Software\\AutoHotkey"];
    assert_eq!(command::run(&mut registry, &mut console, &words), 1);
    assert_eq!(
        console.asked,
        ["Delete HKEY_CURRENT_USER\\Software\\AutoHotkey, 1 subkey and 3 values? [y/N] "]
    );
    assert!(
        registry
            .key_exists(&key("HKCU\\Software\\AutoHotkey"))
            .unwrap()
    );
    assert_eq!(
        run(
            &mut registry,
            "registry delete HKCU\\Software\\AutoHotkey",
            Some(true)
        )
        .0,
        0
    );
    assert!(
        !registry
            .key_exists(&key("HKCU\\Software\\AutoHotkey"))
            .unwrap()
    );
    let (status, _, err) = run(
        &mut registry,
        "registry get HKCU\\Software\\AutoHotkey",
        None,
    );
    assert_eq!(status, 1);
    assert_eq!(
        err,
        "windows: No such key: HKEY_CURRENT_USER\\Software\\AutoHotkey.\n"
    );
}

#[test]
fn errors_carry_their_code_as_json_when_asked() {
    let mut registry = sample();
    let mut console = FakeConsole {
        coded: true,
        ..FakeConsole::default()
    };
    let words = ["registry", "get", "HKCU\\Software\\AutoHotkey", "Missing"];
    assert_eq!(command::run(&mut registry, &mut console, &words), 1);
    let err = String::from_utf8(console.err).unwrap();
    assert!(
        err.starts_with("windows: No such value: Missing in HKEY_CURRENT_USER"),
        "{err}"
    );
    assert!(
        err.contains("{\"error\":{\"code\":\"no_such_value\",\"category\":\"not_found\""),
        "{err}"
    );
    assert_eq!(run(&mut registry, "registry get", None).0, 2);
    assert_eq!(run(&mut registry, "registry get --nope HKCU", None).0, 2);
    let line = "registry set HKCU\\X v --type dword many";
    assert_eq!(run(&mut registry, line, None).0, 2);
}

#[test]
fn a_damaged_hive_is_reported_and_reset_keeps_it_beside_the_new_one() {
    let mut registry = Registry::new(FakeStore {
        files: [Some(b"not a hive".to_vec()), None],
        ..FakeStore::default()
    });
    let (status, _, err) = run(&mut registry, "registry get HKCU", None);
    assert_eq!(status, 1);
    assert!(err.contains("try: windows registry reset"), "{err}");
    assert_eq!(
        run(&mut registry, "registry reset --yes", None),
        (
            0,
            String::from("Replaced the registry file. It is empty.\n"),
            String::new()
        )
    );
    assert_eq!(
        registry.store().damaged[0].as_deref(),
        Some(&b"not a hive"[..])
    );
    assert_eq!(run(&mut registry, "registry get HKCU", None).0, 0);
}

#[test]
fn every_windows_registry_example_writes_what_the_registry_says() {
    let mut ran = 0;
    for definition in COMMANDS
        .iter()
        .filter(|definition| definition.words.starts_with(&["windows", "registry"]))
    {
        for example in definition.examples {
            let mut registry = sample();
            let line = example.line.strip_prefix("windows ").unwrap();
            let (status, out, err) = run(&mut registry, line, None);
            assert_eq!(
                (status, out.as_str()),
                (example.status, example.output),
                "{}: {err}",
                example.line
            );
            ran += 1;
        }
    }
    assert!(ran >= 6, "{ran}");
}

#[test]
fn set_asks_before_replacing_a_value_or_changing_the_machine_s_hive() {
    let mut registry = sample();
    let mut console = FakeConsole {
        answer: Some(false),
        ..FakeConsole::default()
    };
    let words = [
        "registry",
        "set",
        "HKCU\\Software\\AutoHotkey",
        "Count",
        "--type",
        "dword",
        "7",
    ];
    assert_eq!(command::run(&mut registry, &mut console, &words), 1);
    assert_eq!(
        console.asked,
        ["Replace the value Count in HKEY_CURRENT_USER\\Software\\AutoHotkey? [y/N] "]
    );
    let autohotkey = key("HKCU\\Software\\AutoHotkey");
    assert_eq!(
        registry.value(&autohotkey, &utf16("Count")).unwrap().data,
        42u32.to_le_bytes()
    );
    let line = "registry set --yes HKCU\\Software\\AutoHotkey Count --type dword 7";
    assert_eq!(run(&mut registry, line, Some(false)).0, 0);
    let line = "registry set HKCU\\Software\\AutoHotkey Fresh new";
    let mut console = FakeConsole {
        answer: Some(false),
        ..FakeConsole::default()
    };
    let words: Vec<&str> = line.split(' ').collect();
    assert_eq!(command::run(&mut registry, &mut console, &words), 0);
    assert!(console.asked.is_empty());
    let line = "registry set HKCU\\Software\\AutoHotkey Fresh newer";
    assert_eq!(run(&mut registry, line, None).0, 0);
    assert_eq!(
        registry.value(&autohotkey, &utf16("Fresh")).unwrap().data,
        string_data("newer")
    );
    registry.store().user_id = 0;
    let machine = key("HKLM\\Software\\AutoHotkey");
    assert_eq!(
        command::set_question(&mut registry, &machine, None).as_deref(),
        Some(
            "Make the key HKEY_LOCAL_MACHINE\\Software\\AutoHotkey? It applies to every user \
             of this machine. [y/N] "
        )
    );
    assert_eq!(
        command::set_question(&mut registry, &machine, Some(&utf16("InstallDir"))).as_deref(),
        Some(
            "Save the value InstallDir in HKEY_LOCAL_MACHINE\\Software\\AutoHotkey? It applies \
             to every user of this machine. [y/N] "
        )
    );
    registry.store().user_id = 1000;
    assert_eq!(command::set_question(&mut registry, &machine, None), None);
    assert_eq!(
        command::set_question(&mut registry, &key("HKCU\\Software\\New"), None),
        None
    );
}
