//! The `disk` command (D192, docs/ux/storage-v0.3.md): `disk list`, `disk check`, `disk repair`
//! and `disk format`, defined in the command model's registry (`eggos_commands::commands::
//! DISK_LIST`, `DISK_CHECK`, `DISK_REPAIR`, `DISK_FORMAT`), over the shared capability
//! `eggos_files::storage`. `/bin/disk` is a hard link to `/bin/settings`. The disks and the
//! terminal are a [`WholeDisks`] and a [`Console`], so host tests run the registry's examples.

use alloc::format;
use alloc::string::String;
use eggos_commands::{ErrorDefinition, errors};
use eggos_eggfs::{CheckMode, DiskCheck};
use eggos_files::storage::{
    self, DiskRecord, StorageError, WholeDisks, erased_text, headline, outcome_word,
};
use eggos_mcp::json::JsonWriter;
use eggos_mcp::server::write_error;

/// The command's usage, printed for a usage error.
pub const USAGE: &str = "usage: disk list [--json] | disk check [--json] <device> | disk repair \
                         [--json] [--dry-run] <device> | disk format [--label NAME] [--yes] \
                         [--dry-run] <device>\n";
/// The exit status of a usage error.
pub const USAGE_STATUS: i32 = 2;
/// The exit status of every other error.
pub const FAILURE: i32 = 1;

const ANSWER_BYTES: usize = 64;
const RECORD_BYTES: usize = 1024;
const ERROR_BYTES: usize = 1024;
const MAX_WORDS: usize = 2;

/// The terminal the command talks to.
pub trait Console {
    /// Writes to standard output.
    fn write_output(&mut self, bytes: &[u8]);
    /// Writes to standard error.
    fn write_error(&mut self, bytes: &[u8]);
    /// Whether standard input and standard error are a terminal a person answers on.
    fn is_terminal(&mut self) -> bool;
    /// Reads one line of the person's answer into `buffer`; its length.
    fn read_answer(&mut self, buffer: &mut [u8]) -> usize;
}

#[derive(Debug, Default)]
struct Options<'a> {
    json: bool,
    dry_run: bool,
    yes: bool,
    label: Option<&'a str>,
}

/// Runs `disk` with `arguments`, the words after its name; the exit status.
pub fn run(disks: &mut impl WholeDisks, console: &mut impl Console, arguments: &[&str]) -> i32 {
    let mut options = Options::default();
    let mut words = [""; MAX_WORDS];
    let mut count = 0;
    let mut options_end = false;
    let mut rest = arguments.iter();
    while let Some(&argument) = rest.next() {
        match argument {
            "--" if !options_end => options_end = true,
            "--json" if !options_end => options.json = true,
            "--dry-run" if !options_end => options.dry_run = true,
            "--yes" | "-y" if !options_end => options.yes = true,
            "--label" if !options_end => match rest.next() {
                Some(&label) => options.label = Some(label),
                None => return usage(console, options.json),
            },
            _ if !options_end && argument.len() > 1 && argument.starts_with('-') => {
                let message = format!("unrecognized option '{argument}'");
                report(console, options.json, &errors::USAGE, &message);
                return USAGE_STATUS;
            }
            _ if count == MAX_WORDS => return usage(console, options.json),
            _ => {
                words[count] = argument;
                count += 1;
            }
        }
    }
    let plain = !options.dry_run && !options.yes && options.label.is_none();
    match words[..count] {
        ["list"] if plain => list(disks, console, options.json),
        ["check", device] if plain => {
            check(disks, console, device, CheckMode::ReadOnly, options.json)
        }
        ["repair", device] if !options.yes && options.label.is_none() => {
            let mode = match options.dry_run {
                true => CheckMode::ReadOnly,
                false => CheckMode::Repair,
            };
            check(disks, console, device, mode, options.json)
        }
        ["format", device] if !options.json => format_disk(disks, console, device, &options),
        _ => usage(console, options.json),
    }
}

fn usage(console: &mut impl Console, json: bool) -> i32 {
    match json {
        true => report(console, true, &errors::USAGE, USAGE.trim_end()),
        false => console.write_error(USAGE.as_bytes()),
    }
    USAGE_STATUS
}

fn report(console: &mut impl Console, json: bool, error: &ErrorDefinition, message: &str) {
    if json {
        let mut buffer = [0u8; ERROR_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        write_error(&mut out, error, message);
        out.end_line();
        if !out.overflowed() {
            console.write_error(out.as_bytes());
        }
        return;
    }
    console.write_error(b"disk: ");
    console.write_error(message.as_bytes());
    console.write_error(b"\n");
    if let Some(command) = error.fix_command {
        console.write_error(b"  try: ");
        console.write_error(command.as_bytes());
        console.write_error(b"\n");
    }
}

/// The registry's error for a storage error.
pub fn error_of(error: &StorageError) -> &'static ErrorDefinition {
    match error {
        StorageError::NoSuchDisk => &errors::NO_SUCH_DISK,
        StorageError::InUse(_) | StorageError::Busy => &errors::DISK_IN_USE,
        StorageError::ReadOnly => &errors::READ_ONLY,
        StorageError::NotAllowed => &errors::PERMISSION_DENIED,
        StorageError::NotEggFs => &errors::NOT_EGGFS,
        StorageError::LabelTooLong => &errors::LABEL_TOO_LONG,
        StorageError::TooSmall | StorageError::Failed => &errors::FAILED,
    }
}

fn refuse(console: &mut impl Console, json: bool, device: &str, error: &StorageError) -> i32 {
    let message = format!("{device}: {}", error.sentence());
    report(console, json, error_of(error), &message);
    match error {
        StorageError::LabelTooLong => USAGE_STATUS,
        _ => FAILURE,
    }
}

fn dash(text: Option<String>) -> String {
    text.unwrap_or_else(|| String::from("-"))
}

fn list_line(record: &DiskRecord) -> String {
    let file_system = match record.file_system {
        storage::FileSystem::EggFs => "EggFS",
        storage::FileSystem::Fat => "FAT",
        other => other.word(),
    };
    let label = record
        .label
        .as_ref()
        .filter(|label| !label.is_empty())
        .map(|label| format!("\"{label}\""));
    format!(
        "{}  {}  {}  {file_system}  {}  {}  {}  {}\n",
        record.device,
        storage_size(record.size_bytes),
        if record.read_only { "ro" } else { "rw" },
        dash(label),
        dash(record.mounted_on.clone()),
        dash(record.used_bytes.map(storage_size)),
        dash(record.available_bytes.map(storage_size)),
    )
}

fn storage_size(bytes: u64) -> String {
    String::from(eggos_files::listing::size_text(bytes).as_str())
}

fn write_record(console: &mut impl Console, record: &DiskRecord) {
    let mut buffer = [0u8; RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("device");
    out.string(record.device);
    out.key("size_bytes");
    out.u64(record.size_bytes);
    out.key("read_only");
    out.bool(record.read_only);
    out.key("file_system");
    out.string(record.file_system.word());
    out.key("label");
    match &record.label {
        Some(label) => out.string(label),
        None => out.null(),
    }
    out.key("mounted_on");
    match &record.mounted_on {
        Some(place) => out.string(place),
        None => out.null(),
    }
    for (key, value) in [
        ("used_bytes", record.used_bytes),
        ("available_bytes", record.available_bytes),
    ] {
        out.key(key);
        match value {
            Some(value) => out.u64(value),
            None => out.null(),
        }
    }
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        console.write_output(out.as_bytes());
    }
}

fn list(disks: &mut impl WholeDisks, console: &mut impl Console, json: bool) -> i32 {
    let records = match storage::list(disks) {
        Ok(records) => records,
        Err(error) => return refuse(console, json, storage::DISKS_FILE, &error),
    };
    if !json {
        console.write_output(b"DEVICE  SIZE  RW  FS  LABEL  MOUNTED  USED  FREE\n");
    }
    for record in &records {
        match json {
            true => write_record(console, record),
            false => console.write_output(list_line(record).as_bytes()),
        }
    }
    0
}

fn write_check(console: &mut impl Console, device: &str, check: &DiskCheck) {
    let mut buffer = alloc::vec![0u8; RECORD_BYTES + check.findings.len() * 256];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("device");
    out.string(device);
    out.key("clean");
    out.bool(check.is_clean());
    out.key("repairable");
    out.bool(check.is_repairable());
    out.key("blocks");
    out.u64(check.blocks);
    out.key("free_blocks");
    out.u64(check.free_blocks);
    out.key("findings");
    out.begin_array();
    for finding in &check.findings {
        out.begin_object();
        out.key("code");
        out.string(finding.problem.code());
        out.key("description");
        out.string(finding.problem.description());
        out.key("inode");
        out.u64(u64::from(finding.inode));
        out.key("block");
        out.u64(u64::from(finding.block));
        out.key("outcome");
        out.string(outcome_word(finding.outcome));
        out.end_object();
    }
    out.end_array();
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        console.write_output(out.as_bytes());
    }
}

fn check(
    disks: &mut impl WholeDisks,
    console: &mut impl Console,
    device: &str,
    mode: CheckMode,
    json: bool,
) -> i32 {
    let shown = storage::device_named(device).unwrap_or(device);
    let result = match storage::check(disks, device, mode) {
        Ok(result) => result,
        Err(error) => return refuse(console, json, shown, &error),
    };
    match json {
        true => write_check(console, shown, &result),
        false => {
            let mut text = headline(&result);
            text.push('\n');
            for finding in &result.findings {
                text.push_str(&format!(
                    "{}  inode {}  block {}  {}  {}\n",
                    finding.problem.code(),
                    finding.inode,
                    finding.block,
                    outcome_word(finding.outcome),
                    finding.problem.description()
                ));
            }
            console.write_output(text.as_bytes());
        }
    }
    if mode == CheckMode::Repair && !result.is_repairable() {
        let message = format!("{shown}: problems are left that cannot be repaired safely");
        report(console, json, &errors::NOT_REPAIRABLE, &message);
        return FAILURE;
    }
    0
}

fn format_disk(
    disks: &mut impl WholeDisks,
    console: &mut impl Console,
    device: &str,
    options: &Options<'_>,
) -> i32 {
    let label = options.label.unwrap_or("");
    if !storage::label_fits(label) {
        return refuse(console, false, device, &StorageError::LabelTooLong);
    }
    let record = match storage::find(disks, device) {
        Ok(record) => record,
        Err(error) => return refuse(console, false, device, &error),
    };
    if let Some(place) = record.mounted_on.clone() {
        return refuse(console, false, record.device, &StorageError::InUse(place));
    }
    if record.read_only {
        return refuse(console, false, record.device, &StorageError::ReadOnly);
    }
    let labelled = match label.is_empty() {
        true => String::from("with no label"),
        false => format!("labelled \"{label}\""),
    };
    if options.dry_run {
        let line = format!(
            "would erase {} ({}) and make an empty EggFS file system {labelled}\n",
            record.device,
            erased_text(&record)
        );
        console.write_output(line.as_bytes());
        return 0;
    }
    if !options.yes {
        if !console.is_terminal() {
            let message = format!(
                "{}: formatting erases the disk: add --yes, or run it on a terminal to be asked",
                record.device
            );
            report(console, false, &errors::NEEDS_CONFIRMATION, &message);
            return FAILURE;
        }
        let name = record.short_name();
        let question = format!(
            "Type {name} to format {} (erases {}): ",
            record.device,
            erased_text(&record)
        );
        console.write_error(question.as_bytes());
        let mut answer = [0u8; ANSWER_BYTES];
        let length = console.read_answer(&mut answer);
        let answer = core::str::from_utf8(answer.get(..length).unwrap_or_default()).unwrap_or("");
        if answer.trim() != name {
            let message = format!("{}: not formatted", record.device);
            report(console, false, &errors::DECLINED_BY_USER, &message);
            return FAILURE;
        }
    }
    if let Err(error) = storage::format(disks, record.device, label) {
        return refuse(console, false, record.device, &error);
    }
    let mut line = storage::formatted_text(record.device, label);
    line.push('\n');
    console.write_output(line.as_bytes());
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_commands::COMMANDS;
    use eggos_files::storage::Usage;
    use eggos_files::storage::memory::{MemoryDisks, MemoryWholeDisk};

    const BLOCKS: usize = 512;

    #[derive(Debug, Default)]
    struct Fake {
        output: Vec<u8>,
        errors: Vec<u8>,
        terminal: bool,
        answer: &'static str,
    }

    impl Console for Fake {
        fn write_output(&mut self, bytes: &[u8]) {
            self.output.extend_from_slice(bytes);
        }

        fn write_error(&mut self, bytes: &[u8]) {
            self.errors.extend_from_slice(bytes);
        }

        fn is_terminal(&mut self) -> bool {
            self.terminal
        }

        fn read_answer(&mut self, buffer: &mut [u8]) -> usize {
            let length = self.answer.len().min(buffer.len());
            buffer[..length].copy_from_slice(&self.answer.as_bytes()[..length]);
            length
        }
    }

    fn example_disks() -> MemoryDisks {
        let mut home = MemoryWholeDisk::eggfs("vda", BLOCKS, "Home");
        home.mounted_on = Some("/home");
        home.usage = Some(Usage {
            size_bytes: 2 << 20,
            used_bytes: 1 << 20,
            available_bytes: 1 << 20,
        });
        MemoryDisks::of(vec![home, MemoryWholeDisk::blank("vdb", BLOCKS)])
    }

    fn disk(disks: &mut MemoryDisks, fake: &mut Fake, words: &[&str]) -> (i32, String, String) {
        let status = run(disks, fake, words);
        let output = String::from_utf8_lossy(&core::mem::take(&mut fake.output)).into_owned();
        let errors = String::from_utf8_lossy(&core::mem::take(&mut fake.errors)).into_owned();
        (status, output, errors)
    }

    #[test]
    fn every_disk_example_writes_what_the_registry_says() {
        let mut ran = 0;
        for command in COMMANDS.iter().filter(|command| command.words[0] == "disk") {
            for example in command.examples {
                let words: Vec<&str> = example.line.split(' ').skip(1).collect();
                let mut disks = example_disks();
                let (status, output, errors) = disk(&mut disks, &mut Fake::default(), &words);
                assert_eq!(
                    (status, output.as_str()),
                    (example.status, example.output),
                    "{}: {errors}",
                    example.line
                );
                ran += 1;
            }
        }
        assert!(ran >= 10, "{ran}");
    }

    #[test]
    fn check_reports_clean_and_repair_fixes_a_damaged_disk_then_check_is_clean() {
        let mut disks = example_disks();
        let mut fake = Fake::default();
        assert_eq!(
            disk(&mut disks, &mut fake, &["format", "--yes", "/dev/vdb"]).0,
            0
        );
        let (status, output, _) = disk(&mut disks, &mut fake, &["check", "/dev/vdb"]);
        assert_eq!(status, 0);
        assert!(output.starts_with("Clean. 512 blocks, "), "{output}");
        disks.disks[1].image[4096..8192].fill(0);
        let (status, output, _) = disk(&mut disks, &mut fake, &["check", "--json", "vdb"]);
        assert_eq!(status, 0);
        assert!(
            output.starts_with("{\"device\":\"/dev/vdb\",\"clean\":false,\"repairable\":true,"),
            "{output}"
        );
        assert!(output.contains("\"outcome\":\"repairable\""), "{output}");
        let (status, output, _) = disk(&mut disks, &mut fake, &["repair", "--dry-run", "vdb"]);
        assert_eq!(status, 0);
        assert!(output.starts_with("Found "), "{output}");
        let (status, output, _) = disk(&mut disks, &mut fake, &["repair", "/dev/vdb"]);
        assert_eq!(status, 0);
        assert!(output.starts_with("Repaired "), "{output}");
        assert!(output.contains("  repaired  "), "{output}");
        let (_, output, _) = disk(&mut disks, &mut fake, &["check", "/dev/vdb"]);
        assert!(output.starts_with("Clean. "), "{output}");
    }

    #[test]
    fn format_asks_on_a_terminal_and_refuses_a_wrong_answer_or_a_long_label() {
        let mut disks = example_disks();
        let mut fake = Fake {
            terminal: true,
            answer: "vda\n",
            ..Fake::default()
        };
        let (status, _, errors) = disk(&mut disks, &mut fake, &["format", "/dev/vdb"]);
        assert_eq!(status, 1);
        assert!(
            errors.starts_with("Type vdb to format /dev/vdb (erases Blank, 2.0 MB): disk: "),
            "{errors}"
        );
        assert!(errors.contains("not formatted"), "{errors}");
        assert_eq!(disks.disk("vdb").writes, 0);
        fake.answer = " vdb \n";
        let (status, output, _) = disk(
            &mut disks,
            &mut fake,
            &["format", "--label", "Photos", "/dev/vdb"],
        );
        assert_eq!(status, 0);
        assert_eq!(output, "Formatted /dev/vdb as EggFS \"Photos\".\n");
        let long = "x".repeat(65);
        let (status, _, errors) = disk(
            &mut disks,
            &mut fake,
            &["format", "--yes", "--label", &long, "vdb"],
        );
        assert_eq!(status, 2);
        assert!(
            errors.contains("The label is too long: 64 bytes at most."),
            "{errors}"
        );
        let (status, output, _) = disk(&mut disks, &mut fake, &["list"]);
        assert_eq!(status, 0);
        assert!(
            output.contains("\n/dev/vdb  2.0 MB  rw  EggFS  \"Photos\"  -  "),
            "{output}"
        );
    }

    #[test]
    fn errors_are_coded_and_usage_is_refused() {
        let mut disks = example_disks();
        let mut fake = Fake::default();
        let (status, _, errors) = disk(&mut disks, &mut fake, &["check", "--json", "/dev/vdc"]);
        assert_eq!(status, 1);
        assert!(errors.contains("\"code\":\"no_such_disk\""), "{errors}");
        let (status, _, errors) = disk(&mut disks, &mut fake, &["check", "/dev/vda"]);
        assert_eq!(status, 1);
        assert_eq!(
            errors,
            "disk: /dev/vda: In use at /home: nothing was done.\n"
        );
        for words in [
            &["frobnicate"][..],
            &["list", "--yes"],
            &["check", "--dry-run", "vdb"],
            &["format", "--json", "vdb"],
            &["repair", "--label", "x", "vdb"],
            &["format", "--label"],
            &["check"],
        ] {
            assert_eq!(disk(&mut disks, &mut fake, words).0, 2, "{words:?}");
        }
        assert!(disks.disks.iter().all(|disk| disk.writes == 0));
    }
}
