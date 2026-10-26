use crate::output::{FAILURE, Output};
use crate::system::{DIRECTORY, FILE_TYPE, Status, SymbolicLinks, System};
use crate::utilities::MAX_ARGUMENTS;
use core::ffi::CStr;

pub const NAMES_CAPACITY: usize = 32 * 1024;
pub const MAX_ENTRIES: usize = 1024;

const SERIOUS_TROUBLE: i32 = 2;
const DIRECTORY_CHUNK: usize = 4096;
const RECORD_LENGTH_OFFSET: usize = 16;
const RECORD_NAME_OFFSET: usize = 19;
const SECONDS_PER_DAY: i64 = 86_400;
const EARLIEST_TIME_SHOWN: i64 = -62_167_219_200;
const LATEST_TIME_SHOWN: i64 = 253_402_300_799;

#[derive(Debug, Clone, Copy, Default)]
struct Options {
    all: bool,
    almost_all: bool,
    long: bool,
    directories_as_files: bool,
}

impl Options {
    fn shows(&self, name: &[u8]) -> bool {
        match name {
            b"." | b".." => self.all,
            [b'.', ..] => self.all || self.almost_all,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Operand<'a> {
    path: &'a CStr,
    status: Status,
}

#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    start: u32,
    length: u16,
    status: Status,
}

#[derive(Debug)]
pub struct Listing {
    names: [u8; NAMES_CAPACITY],
    used: usize,
    entries: [Entry; MAX_ENTRIES],
    count: usize,
}

impl Listing {
    pub fn new() -> Self {
        Self {
            names: [0; NAMES_CAPACITY],
            used: 0,
            entries: [Entry::default(); MAX_ENTRIES],
            count: 0,
        }
    }

    fn clear(&mut self) {
        self.used = 0;
        self.count = 0;
    }

    fn push(&mut self, name: &[u8]) -> bool {
        let end = self.used + name.len() + 1;
        if end > NAMES_CAPACITY || self.count == MAX_ENTRIES {
            return false;
        }
        self.names[self.used..end - 1].copy_from_slice(name);
        self.names[end - 1] = 0;
        self.entries[self.count] = Entry {
            start: self.used as u32,
            length: name.len() as u16,
            status: Status::default(),
        };
        self.used = end;
        self.count += 1;
        true
    }

    fn name(&self, entry: &Entry) -> &[u8] {
        &self.names[entry.start as usize..][..usize::from(entry.length)]
    }

    fn name_with_nul(&self, entry: &Entry) -> &CStr {
        let bytes = &self.names[entry.start as usize..][..usize::from(entry.length) + 1];
        CStr::from_bytes_with_nul(bytes).unwrap_or(c"")
    }

    fn sort(&mut self) {
        let names = &self.names;
        let name = |entry: &Entry| &names[entry.start as usize..][..usize::from(entry.length)];
        self.entries[..self.count].sort_unstable_by(|a, b| name(a).cmp(name(b)));
    }
}

impl Default for Listing {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Widths {
    links: usize,
    owner: usize,
    group: usize,
    size: usize,
}

impl Widths {
    fn of<'a>(statuses: impl Iterator<Item = &'a Status>) -> Self {
        statuses.fold(Self::default(), |widths, status| Self {
            links: widths.links.max(digits(u64::from(status.links))),
            owner: widths.owner.max(digits(u64::from(status.owner))),
            group: widths.group.max(digits(u64::from(status.group))),
            size: widths.size.max(digits(status.size)),
        })
    }
}

fn digits(value: u64) -> usize {
    value.checked_ilog10().map_or(1, |log| log as usize + 1)
}

pub fn ls<S: System>(output: &mut Output<S>, listing: &mut Listing, arguments: &[&CStr]) -> i32 {
    let mut options = Options::default();
    let mut operands = [None::<&CStr>; MAX_ARGUMENTS];
    let mut operand_count = 0;
    let mut options_end = false;
    for &argument in arguments {
        let bytes = argument.to_bytes();
        if options_end || bytes.len() < 2 || bytes[0] != b'-' {
            if operand_count == MAX_ARGUMENTS {
                output.error(&[b"too many operands"]);
                return SERIOUS_TROUBLE;
            }
            operands[operand_count] = Some(argument);
            operand_count += 1;
            continue;
        }
        if bytes == b"--" {
            options_end = true;
            continue;
        }
        if bytes.starts_with(b"--") {
            output.invalid_option(bytes);
            return SERIOUS_TROUBLE;
        }
        for &letter in &bytes[1..] {
            match letter {
                b'a' => options.all = true,
                b'A' => options.almost_all = true,
                b'l' => options.long = true,
                b'd' => options.directories_as_files = true,
                b'1' => {}
                _ => {
                    output.invalid_option(&[b'-', letter]);
                    return SERIOUS_TROUBLE;
                }
            }
        }
    }
    if operand_count == 0 {
        operands[0] = Some(c".");
        operand_count = 1;
    }
    let operands = operands[..operand_count].iter().flatten().copied();
    list_operands(output, listing, options, operands, operand_count > 1)
}

fn list_operands<'a, S: System>(
    output: &mut Output<S>,
    listing: &mut Listing,
    options: Options,
    operands: impl Iterator<Item = &'a CStr>,
    headers: bool,
) -> i32 {
    let mut status = 0;
    let empty = Operand {
        path: c"",
        status: Status::default(),
    };
    let mut files = [empty; MAX_ARGUMENTS];
    let mut file_count = 0;
    let mut directories = [empty; MAX_ARGUMENTS];
    let mut directory_count = 0;
    for path in operands {
        match output.system.status(None, path, SymbolicLinks::Follow) {
            Ok(found) if found.is_directory() && !options.directories_as_files => {
                directories[directory_count] = Operand {
                    path,
                    status: found,
                };
                directory_count += 1;
            }
            Ok(found) => {
                files[file_count] = Operand {
                    path,
                    status: found,
                };
                file_count += 1;
            }
            Err(errno) => {
                output.error(&[b"cannot access '", path.to_bytes(), b"': ", errno.message()]);
                status = SERIOUS_TROUBLE;
            }
        }
    }
    let files = &mut files[..file_count];
    files.sort_unstable_by(|a, b| a.path.cmp(b.path));
    let widths = Widths::of(files.iter().map(|file| &file.status));
    for file in files.iter() {
        write_entry(output, options, file.path.to_bytes(), &file.status, widths);
    }
    let mut printed = !files.is_empty();
    let directories = &mut directories[..directory_count];
    directories.sort_unstable_by(|a, b| a.path.cmp(b.path));
    for directory in directories.iter() {
        if printed {
            output.write(b"\n");
        }
        if headers {
            output.write(directory.path.to_bytes());
            output.write(b":\n");
        }
        printed = true;
        status = status.max(list_directory(output, listing, options, directory.path));
    }
    status
}

fn list_directory<S: System>(
    output: &mut Output<S>,
    listing: &mut Listing,
    options: Options,
    path: &CStr,
) -> i32 {
    let descriptor = match output.system.open(path) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.error(&[
                b"cannot open directory '",
                path.to_bytes(),
                b"': ",
                errno.message(),
            ]);
            return SERIOUS_TROUBLE;
        }
    };
    let mut status = read_directory(output, listing, options, path, descriptor);
    listing.sort();
    if options.long {
        status = status.max(read_statuses(output, listing, path, descriptor));
        let total = listing.entries[..listing.count]
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.status.blocks));
        output.write(b"total ");
        output.write_number(total.div_ceil(2), 0);
        output.write(b"\n");
    }
    output.system.close(descriptor);
    let entries = &listing.entries[..listing.count];
    let widths = Widths::of(entries.iter().map(|entry| &entry.status));
    for entry in entries {
        write_entry(output, options, listing.name(entry), &entry.status, widths);
    }
    status
}

fn read_directory<S: System>(
    output: &mut Output<S>,
    listing: &mut Listing,
    options: Options,
    path: &CStr,
    descriptor: usize,
) -> i32 {
    listing.clear();
    let mut chunk = [0u8; DIRECTORY_CHUNK];
    loop {
        let length = match output.system.directory_entries(descriptor, &mut chunk) {
            Ok(0) => return 0,
            Ok(length) => length,
            Err(errno) => {
                output.error(&[
                    b"reading directory '",
                    path.to_bytes(),
                    b"': ",
                    errno.message(),
                ]);
                return SERIOUS_TROUBLE;
            }
        };
        for name in record_names(&chunk[..length]) {
            if options.shows(name) && !listing.push(name) {
                output.error(&[
                    b"cannot list all of '",
                    path.to_bytes(),
                    b"': too many entries",
                ]);
                return FAILURE;
            }
        }
    }
}

fn read_statuses<S: System>(
    output: &mut Output<S>,
    listing: &mut Listing,
    path: &CStr,
    descriptor: usize,
) -> i32 {
    let mut status = 0;
    let mut kept = 0;
    for index in 0..listing.count {
        let entry = listing.entries[index];
        let name = listing.name_with_nul(&entry);
        match output
            .system
            .status(Some(descriptor), name, SymbolicLinks::DoNotFollow)
        {
            Ok(found) => {
                listing.entries[kept] = Entry {
                    status: found,
                    ..entry
                };
                kept += 1;
            }
            Err(errno) => {
                output.error(&[
                    b"cannot access '",
                    path.to_bytes(),
                    b"/",
                    name.to_bytes(),
                    b"': ",
                    errno.message(),
                ]);
                status = FAILURE;
            }
        }
    }
    listing.count = kept;
    status
}

fn record_names(records: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = records;
    core::iter::from_fn(move || {
        let length = rest.get(RECORD_LENGTH_OFFSET..RECORD_LENGTH_OFFSET + 2)?;
        let length = usize::from(u16::from_ne_bytes([length[0], length[1]]));
        let record = rest.get(..length).filter(|_| length > RECORD_NAME_OFFSET)?;
        rest = &rest[length..];
        let name = &record[RECORD_NAME_OFFSET..];
        let end = name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(name.len());
        Some(&name[..end])
    })
}

fn write_entry<S: System>(
    output: &mut Output<S>,
    options: Options,
    name: &[u8],
    status: &Status,
    widths: Widths,
) {
    if options.long {
        output.write(&mode_string(status.mode));
        output.write(b" ");
        output.write_number(u64::from(status.links), widths.links);
        output.write(b" ");
        output.write_number(u64::from(status.owner), widths.owner);
        output.write(b" ");
        output.write_number(u64::from(status.group), widths.group);
        output.write(b" ");
        output.write_number(status.size, widths.size);
        output.write(b" ");
        output.write(&time_string(status.modified));
        output.write(b" ");
    }
    output.write(name);
    output.write(b"\n");
}

fn mode_string(mode: u32) -> [u8; 10] {
    let kind = match mode & FILE_TYPE {
        DIRECTORY => b'd',
        0o100_000 => b'-',
        0o120_000 => b'l',
        0o020_000 => b'c',
        0o060_000 => b'b',
        0o010_000 => b'p',
        0o140_000 => b's',
        _ => b'?',
    };
    let bit = |mask: u32, letter: u8| if mode & mask != 0 { letter } else { b'-' };
    let execute =
        |mask: u32, special: u32, set: u8, unset: u8| match (mode & mask != 0, mode & special != 0)
        {
            (true, true) => set,
            (false, true) => unset,
            (true, false) => b'x',
            (false, false) => b'-',
        };
    [
        kind,
        bit(0o400, b'r'),
        bit(0o200, b'w'),
        execute(0o100, 0o4000, b's', b'S'),
        bit(0o040, b'r'),
        bit(0o020, b'w'),
        execute(0o010, 0o2000, b's', b'S'),
        bit(0o004, b'r'),
        bit(0o002, b'w'),
        execute(0o001, 0o1000, b't', b'T'),
    ]
}

fn time_string(seconds: i64) -> [u8; 16] {
    let seconds = seconds.clamp(EARLIEST_TIME_SHOWN, LATEST_TIME_SHOWN);
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_date(days);
    let mut text = *b"0000-00-00 00:00";
    let year = year as u32;
    for (start, value, width) in [
        (0, year, 4),
        (5, month, 2),
        (8, day, 2),
        (11, (of_day / 3600) as u32, 2),
        (14, (of_day % 3600 / 60) as u32, 2),
    ] {
        let mut value = value;
        for index in (start..start + width).rev() {
            text[index] = b'0' + (value % 10) as u8;
            value /= 10;
        }
    }
    text
}

fn civil_date(days_since_epoch: i64) -> (i64, u32, u32) {
    let shifted = days_since_epoch + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_from_march + 2) / 5 + 1) as u32;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(fake: Fake, arguments: &[&CStr]) -> (i32, Fake) {
        let mut output = Output::new(fake, b"ls");
        let mut listing = Box::new(Listing::new());
        let status = ls(&mut output, &mut listing, arguments);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0, "a descriptor was left open");
        (status, output.system)
    }

    fn tree() -> Fake {
        Fake::default()
            .with_directory(".", 0)
            .with_directory("/", 0)
            .with_directory("/etc", 1_700_000_000)
            .with_file("/etc/motd", 0o644, b"Welcome to EggOS.\n", 1_700_000_000)
            .with_file("/etc/hostname", 0o644, b"eggos\n", 0)
            .with_file("/etc/.hidden", 0o600, b"", 0)
            .with_directory("/bin", 0)
            .with_file("/bin/ls", 0o755, &[0; 2000], 86_400 * 365)
    }

    #[test]
    fn a_directory_s_entries_are_listed_sorted_without_hidden_ones() {
        let (status, fake) = run(tree(), &[c"/etc"]);
        assert_eq!(status, 0);
        assert_eq!(fake.output(), "hostname\nmotd\n");
        let (_, fake) = run(tree(), &[c"/"]);
        assert_eq!(fake.output(), "bin\netc\n");
    }

    #[test]
    fn dash_a_shows_every_entry_and_dash_capital_a_all_but_dot_and_dot_dot() {
        let (_, fake) = run(tree(), &[c"-a", c"/etc"]);
        assert_eq!(fake.output(), ".\n..\n.hidden\nhostname\nmotd\n");
        let (_, fake) = run(tree(), &[c"/etc", c"-A"]);
        assert_eq!(fake.output(), ".hidden\nhostname\nmotd\n");
    }

    #[test]
    fn no_operand_lists_the_working_directory() {
        let fake = tree().with_file("./file", 0o644, b"", 0);
        let (status, fake) = run(fake, &[]);
        assert_eq!(status, 0);
        assert_eq!(fake.output(), "file\n");
    }

    #[test]
    fn files_come_first_then_each_directory_under_its_name() {
        let (status, fake) = run(tree(), &[c"/etc", c"/etc/motd", c"/bin", c"/bin/ls"]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "/bin/ls\n/etc/motd\n\n/bin:\nls\n\n/etc:\nhostname\nmotd\n"
        );
    }

    #[test]
    fn a_missing_operand_is_reported_with_status_two_and_the_rest_listed() {
        let (status, fake) = run(tree(), &[c"/nope", c"/bin"]);
        assert_eq!(status, 2);
        assert_eq!(
            fake.errors(),
            "ls: cannot access '/nope': No such file or directory\n"
        );
        assert_eq!(fake.output(), "/bin:\nls\n");
    }

    #[test]
    fn the_long_format_has_mode_links_owner_group_size_time_and_a_total() {
        let (status, fake) = run(tree(), &[c"-l", c"/bin"]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "total 2\n-rwxr-xr-x 1 0 0 2000 1971-01-01 00:00 ls\n"
        );
        let (_, fake) = run(tree(), &[c"-la", c"/etc"]);
        assert_eq!(
            fake.output(),
            "total 1\n\
             drwxr-xr-x 2 0 0  0 2023-11-14 22:13 .\n\
             drwxr-xr-x 2 0 0  0 1970-01-01 00:00 ..\n\
             -rw------- 1 0 0  0 1970-01-01 00:00 .hidden\n\
             -rw-r--r-- 1 0 0  6 1970-01-01 00:00 hostname\n\
             -rw-r--r-- 1 0 0 18 2023-11-14 22:13 motd\n"
        );
    }

    #[test]
    fn dash_d_lists_directories_themselves() {
        let (_, fake) = run(tree(), &[c"-d", c"/etc", c"/bin"]);
        assert_eq!(fake.output(), "/bin\n/etc\n");
        let (_, fake) = run(tree(), &[c"-ld", c"/etc"]);
        assert_eq!(fake.output(), "drwxr-xr-x 2 0 0 0 2023-11-14 22:13 /etc\n");
    }

    #[test]
    fn an_entry_that_cannot_be_examined_is_reported_with_status_one() {
        let mut fake = tree();
        fake.status_error = Some((b"/etc/motd".to_vec(), eggos_runtime::errno::Errno::EACCES));
        let (status, fake) = run(fake, &[c"-l", c"/etc"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "ls: cannot access '/etc/motd': Permission denied\n"
        );
        assert!(fake.output().ends_with(" hostname\n"));
    }

    #[test]
    fn bad_options_and_a_file_where_a_directory_cannot_be_read_are_errors() {
        let (status, fake) = run(tree(), &[c"-z"]);
        assert_eq!(
            (status, fake.errors().as_str()),
            (2, "ls: invalid option -- 'z'\n")
        );
        let (status, _) = run(tree(), &[c"--color"]);
        assert_eq!(status, 2);
        let (status, fake) = run(tree(), &[c"--", c"-z"]);
        assert_eq!(status, 2);
        assert_eq!(
            fake.errors(),
            "ls: cannot access '-z': No such file or directory\n"
        );
    }

    #[test]
    fn a_directory_with_more_entries_than_the_listing_holds_is_cut_short() {
        let mut fake = tree();
        for index in 0..MAX_ENTRIES + 1 {
            fake = fake.with_file(&format!("/many/{index:05}"), 0o644, b"", 0);
        }
        let fake = fake.with_directory("/many", 0);
        let (status, fake) = run(fake, &[c"/many"]);
        assert_eq!(status, 1);
        assert_eq!(
            fake.errors(),
            "ls: cannot list all of '/many': too many entries\n"
        );
        assert_eq!(fake.output().lines().count(), MAX_ENTRIES);
    }

    #[test]
    fn records_are_read_up_to_their_length_and_a_bad_one_ends_the_walk() {
        let mut records = [0u8; 48];
        records[16..18].copy_from_slice(&24u16.to_ne_bytes());
        records[19..22].copy_from_slice(b"ab\0");
        records[24 + 16..24 + 18].copy_from_slice(&24u16.to_ne_bytes());
        records[24 + 19..24 + 24].copy_from_slice(b"cdefg");
        let names: Vec<&[u8]> = record_names(&records).collect();
        assert_eq!(names, [&b"ab"[..], b"cdefg"]);
        records[16..18].copy_from_slice(&0u16.to_ne_bytes());
        assert_eq!(record_names(&records).count(), 0);
        assert_eq!(record_names(&records[..10]).count(), 0);
    }

    #[test]
    fn modes_show_the_type_and_special_bits() {
        assert_eq!(&mode_string(0o104_755), b"-rwsr-xr-x");
        assert_eq!(&mode_string(0o102_644), b"-rw-r-Sr--");
        assert_eq!(&mode_string(0o041_777), b"drwxrwxrwt");
        assert_eq!(&mode_string(0o041_776), b"drwxrwxrwT");
        assert_eq!(&mode_string(0o120_777), b"lrwxrwxrwx");
        assert_eq!(&mode_string(0o020_620), b"crw--w----");
        assert_eq!(&mode_string(0o000_000), b"?---------");
    }

    #[test]
    fn times_are_utc_dates_and_minutes() {
        assert_eq!(&time_string(0), b"1970-01-01 00:00");
        assert_eq!(&time_string(951_782_400), b"2000-02-29 00:00");
        assert_eq!(&time_string(1_700_000_000), b"2023-11-14 22:13");
        assert_eq!(&time_string(-60), b"1969-12-31 23:59");
        assert_eq!(&time_string(i64::MAX), b"9999-12-31 23:59");
        assert_eq!(&time_string(i64::MIN), b"0000-01-01 00:00");
    }
}
