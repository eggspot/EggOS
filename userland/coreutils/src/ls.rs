use crate::options::{Refused, parse_defined};
use crate::output::{FAILURE, Output, error_for};
use crate::page::Page;
use crate::system::{DIRECTORY, FILE_TYPE, STANDARD_OUTPUT, Status, SymbolicLinks, System};
use crate::utilities::MAX_ARGUMENTS;
use core::ffi::CStr;
use eggos_commands::page::MAX_KEY_BYTES;
use eggos_commands::{commands, errors};
use eggos_mcp::json::JsonWriter;

pub const NAMES_CAPACITY: usize = 32 * 1024;
pub const MAX_ENTRIES: usize = 1024;

const SERIOUS_TROUBLE: i32 = 2;
const DIRECTORY_CHUNK: usize = 4096;
const RECORD_LENGTH_OFFSET: usize = 16;
const RECORD_NAME_OFFSET: usize = 19;
const SECONDS_PER_DAY: i64 = 86_400;
const EARLIEST_TIME_SHOWN: i64 = -62_167_219_200;
const LATEST_TIME_SHOWN: i64 = 253_402_300_799;
const RECORD_BYTES: usize = 8 * 1024;
const RECORD_PATH_BYTES: usize = 1024;
const COLUMN_GAP: usize = 2;
const DEFAULT_LINE_WIDTH: usize = 80;
const SPACES: [u8; 32] = [b' '; 32];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Layout {
    #[default]
    ColumnsOnATerminal,
    Lines,
    Columns,
}

#[derive(Debug, Clone, Copy, Default)]
struct Options {
    all: bool,
    almost_all: bool,
    long: bool,
    directories_as_files: bool,
    json: bool,
    layout: Layout,
    line_width: Option<usize>,
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
    threshold: Option<Entry>,
    dropped: bool,
}

impl Listing {
    pub fn new() -> Self {
        Self {
            names: [0; NAMES_CAPACITY],
            used: 0,
            entries: [Entry::default(); MAX_ENTRIES],
            count: 0,
            threshold: None,
            dropped: false,
        }
    }

    fn clear(&mut self) {
        self.used = 0;
        self.count = 0;
        self.threshold = None;
        self.dropped = false;
    }

    fn rejects(&self, name: &[u8]) -> bool {
        self.threshold.is_some_and(|entry| name > self.name(&entry))
    }

    fn keep_smallest(&mut self, keep: usize) {
        self.sort();
        let keep = keep.min(self.count);
        let names = &mut self.names;
        let kept = &mut self.entries[..keep];
        kept.sort_unstable_by_key(|entry| entry.start);
        let mut used = 0;
        for entry in kept.iter_mut() {
            let start = entry.start as usize;
            let length = usize::from(entry.length) + 1;
            names.copy_within(start..start + length, used);
            entry.start = used as u32;
            used += length;
        }
        self.dropped |= keep < self.count;
        self.used = used;
        self.count = keep;
        self.sort();
        self.threshold = self.entries[..self.count].last().copied();
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

fn line_width<S: System>(output: &mut Output<S>, options: Options) -> Option<usize> {
    if options.long || options.json || options.layout == Layout::Lines {
        return None;
    }
    let terminal = output.system.terminal_columns(STANDARD_OUTPUT);
    if terminal.is_none() && options.layout == Layout::ColumnsOnATerminal {
        return None;
    }
    match terminal {
        Some(columns @ 1..) => Some(usize::from(columns)),
        _ => Some(DEFAULT_LINE_WIDTH),
    }
}

fn display_width(name: &[u8]) -> usize {
    name.iter()
        .filter(|&&byte| !(0x80..0xc0).contains(&byte))
        .count()
}

fn column_width(
    width_of: &impl Fn(usize) -> usize,
    count: usize,
    rows: usize,
    column: usize,
) -> usize {
    (column * rows..count.min((column + 1) * rows))
        .map(width_of)
        .max()
        .unwrap_or(0)
}

fn rows_for(count: usize, line_width: usize, width_of: &impl Fn(usize) -> usize) -> usize {
    let most_columns = (line_width + COLUMN_GAP) / (1 + COLUMN_GAP);
    let mut rows = count.div_ceil(most_columns.max(1)).max(1);
    while rows < count {
        let columns = count.div_ceil(rows);
        let names: usize = (0..columns)
            .map(|column| column_width(width_of, count, rows, column))
            .sum();
        if names + COLUMN_GAP * (columns - 1) < line_width {
            return rows;
        }
        rows += 1;
    }
    count
}

fn write_spaces<S: System>(output: &mut Output<S>, mut count: usize) {
    while count > 0 {
        let piece = count.min(SPACES.len());
        output.write(&SPACES[..piece]);
        count -= piece;
    }
}

fn write_columns<'a, S: System>(
    output: &mut Output<S>,
    line_width: usize,
    count: usize,
    name: impl Fn(usize) -> &'a [u8],
) {
    let width_of = |index: usize| display_width(name(index));
    let rows = rows_for(count, line_width, &width_of);
    for row in 0..rows.min(count) {
        let mut index = row;
        let mut column = 0;
        while index < count {
            output.write(name(index));
            if index + rows < count {
                let width = column_width(&width_of, count, rows, column);
                write_spaces(output, width - width_of(index) + COLUMN_GAP);
            }
            index += rows;
            column += 1;
        }
        output.write(b"\n");
    }
}

pub fn ls<S: System>(output: &mut Output<S>, listing: &mut Listing, arguments: &[&CStr]) -> i32 {
    let mut options = Options::default();
    let mut page = Page::default();
    let parsed = parse_defined(output, arguments, &commands::LS, |output, given| {
        if page.take(output, given)? {
            return Ok::<(), Refused>(());
        }
        match given.option.short {
            Some(b'a') => options.all = true,
            Some(b'A') => options.almost_all = true,
            Some(b'l') => options.long = true,
            Some(b'd') => options.directories_as_files = true,
            Some(b'1') => options.layout = Layout::Lines,
            Some(b'C') => options.layout = Layout::Columns,
            _ => options.json |= given.is_long("json"),
        }
        Ok(())
    });
    let Ok(operands) = parsed else {
        return SERIOUS_TROUBLE;
    };
    options.line_width = line_width(output, options);
    let operands = match operands.all() {
        [] => &[c"."][..],
        given => given,
    };
    if page.is_paged() && (operands.len() > 1 || options.directories_as_files) {
        output.coded(
            &errors::USAGE,
            &[b"--limit and --after page the entries of one folder"],
        );
        return SERIOUS_TROUBLE;
    }
    let headers = operands.len() > 1 && !options.json;
    list_operands(
        output,
        listing,
        options,
        &page,
        operands.iter().copied(),
        headers,
    )
}

fn list_operands<'a, S: System>(
    output: &mut Output<S>,
    listing: &mut Listing,
    options: Options,
    page: &Page,
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
                output.coded(
                    error_for(errno),
                    &[b"cannot access '", path.to_bytes(), b"': ", errno.message()],
                );
                status = SERIOUS_TROUBLE;
            }
        }
    }
    let files = &mut files[..file_count];
    files.sort_unstable_by(|a, b| a.path.cmp(b.path));
    match options.line_width {
        Some(line_width) => {
            write_columns(output, line_width, files.len(), |index| {
                files[index].path.to_bytes()
            });
        }
        None => {
            let widths = Widths::of(files.iter().map(|file| &file.status));
            for file in files.iter() {
                let path = file.path.to_bytes();
                if !write_entry(output, options, None, path, &file.status, widths) {
                    status = status.max(FAILURE);
                }
            }
        }
    }
    let mut printed = !files.is_empty();
    let directories = &mut directories[..directory_count];
    directories.sort_unstable_by(|a, b| a.path.cmp(b.path));
    for directory in directories.iter() {
        if printed && !options.json {
            output.write(b"\n");
        }
        if headers {
            output.write(directory.path.to_bytes());
            output.write(b":\n");
        }
        printed = true;
        status = status.max(list_directory(
            output,
            listing,
            options,
            page,
            directory.path,
        ));
    }
    status
}

fn list_directory<S: System>(
    output: &mut Output<S>,
    listing: &mut Listing,
    options: Options,
    page: &Page,
    path: &CStr,
) -> i32 {
    let descriptor = match output.system.open(path) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.coded(
                error_for(errno),
                &[
                    b"cannot open directory '",
                    path.to_bytes(),
                    b"': ",
                    errno.message(),
                ],
            );
            return SERIOUS_TROUBLE;
        }
    };
    let mut status = read_directory(output, listing, options, page, path, descriptor);
    listing.sort();
    let mut last = [0u8; MAX_KEY_BYTES];
    let mut last_length = None;
    if let Some(limit) = page.limit {
        let shown = listing.count.min(limit as usize);
        let more = listing.dropped || listing.count > shown;
        if more && let Some(entry) = shown.checked_sub(1).map(|at| listing.entries[at]) {
            let name = listing.name(&entry);
            if let Some(place) = last.get_mut(..name.len()) {
                place.copy_from_slice(name);
                last_length = Some(name.len());
            }
        }
        listing.count = shown;
    }
    if options.long || options.json {
        status = status.max(read_statuses(output, listing, path, descriptor));
    }
    if options.long && !options.json {
        let total = listing.entries[..listing.count]
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.status.blocks));
        output.write(b"total ");
        output.write_number(total.div_ceil(2), 0);
        output.write(b"\n");
    }
    output.system.close(descriptor);
    let listing: &Listing = listing;
    let entries = &listing.entries[..listing.count];
    if let Some(line_width) = options.line_width {
        write_columns(output, line_width, entries.len(), |index| {
            listing.name(&entries[index])
        });
    } else {
        let widths = Widths::of(entries.iter().map(|entry| &entry.status));
        for entry in entries {
            let name = listing.name(entry);
            if !write_entry(
                output,
                options,
                Some(path.to_bytes()),
                name,
                &entry.status,
                widths,
            ) {
                status = status.max(FAILURE);
            }
        }
    }
    if let Some(length) = last_length {
        output.more(&last[..length]);
    }
    status
}

fn read_directory<S: System>(
    output: &mut Output<S>,
    listing: &mut Listing,
    options: Options,
    page: &Page,
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
                output.coded(
                    error_for(errno),
                    &[
                        b"reading directory '",
                        path.to_bytes(),
                        b"': ",
                        errno.message(),
                    ],
                );
                return SERIOUS_TROUBLE;
            }
        };
        for name in record_names(&chunk[..length]) {
            let before = page.after().is_some_and(|after| name <= after);
            if !options.shows(name) || before || listing.rejects(name) || listing.push(name) {
                continue;
            }
            if let Some(limit) = page.limit {
                let keep = (limit as usize + 1).min(listing.count / 2).max(1);
                listing.keep_smallest(keep);
                if listing.rejects(name) || listing.push(name) {
                    continue;
                }
            }
            output.coded(
                &errors::TOO_MANY_ENTRIES,
                &[
                    b"cannot list all of '",
                    path.to_bytes(),
                    b"': too many entries; list it a page at a time with --limit",
                ],
            );
            return FAILURE;
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
                output.coded(
                    error_for(errno),
                    &[
                        b"cannot access '",
                        path.to_bytes(),
                        b"/",
                        name.to_bytes(),
                        b"': ",
                        errno.message(),
                    ],
                );
                status = FAILURE;
            }
        }
    }
    listing.count = kept;
    status
}

pub fn record_names(records: &[u8]) -> impl Iterator<Item = &[u8]> {
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
    folder: Option<&[u8]>,
    name: &[u8],
    status: &Status,
    widths: Widths,
) -> bool {
    if options.json {
        return write_record(output, folder, name, status);
    }
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
    true
}

fn type_name(mode: u32) -> &'static str {
    match mode & FILE_TYPE {
        DIRECTORY => "folder",
        0o100_000 => "file",
        0o120_000 => "link",
        0o020_000 => "character_device",
        0o060_000 => "block_device",
        0o010_000 => "fifo",
        0o140_000 => "socket",
        _ => "unknown",
    }
}

fn write_record<S: System>(
    output: &mut Output<S>,
    folder: Option<&[u8]>,
    name: &[u8],
    status: &Status,
) -> bool {
    let mut buffer = [0u8; RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("name");
    out.string_lossy(name);
    out.key("path");
    out.begin_string();
    if let Some(folder) = folder {
        out.string_piece_lossy(folder.get(..RECORD_PATH_BYTES).unwrap_or(folder));
        if !folder.ends_with(b"/") {
            out.string_piece("/");
        }
    }
    out.string_piece_lossy(name.get(..RECORD_PATH_BYTES).unwrap_or(name));
    out.end_string();
    out.key("type");
    out.string(type_name(status.mode));
    out.key("permissions");
    out.string_lossy(&mode_string(status.mode)[1..]);
    for (key, value) in [
        ("links", u64::from(status.links)),
        ("owner", u64::from(status.owner)),
        ("group", u64::from(status.group)),
        ("size_bytes", status.size),
    ] {
        out.key(key);
        out.u64(value);
    }
    out.key("modified_at");
    match eggos_calendar::rfc3339(status.modified) {
        Some(text) => out.string_lossy(&text),
        None => out.null(),
    }
    out.end_object();
    out.end_line();
    if out.overflowed() {
        output.error(&[b"cannot write '", name, b"' as JSON: its path is too long"]);
        return false;
    }
    output.write(out.as_bytes());
    true
}

pub fn mode_string(mode: u32) -> [u8; 10] {
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
            "ls: cannot list all of '/many': too many entries; list it a page at a time with \
             --limit\n"
        );
        assert_eq!(fake.output().lines().count(), MAX_ENTRIES);
    }

    fn many(count: usize) -> Fake {
        let mut fake = tree();
        for index in (0..count).rev() {
            fake = fake.with_file(&format!("/many/{index:05}"), 0o644, b"", 0);
        }
        fake.with_directory("/many", 0)
    }

    #[test]
    fn limit_and_after_page_through_a_folder_larger_than_the_listing() {
        let total = MAX_ENTRIES * 2 + 7;
        let mut seen = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut arguments = vec![c"--limit".to_owned(), c"500".to_owned()];
            if let Some(cursor) = &after {
                arguments.push(c"--after".to_owned());
                arguments.push(std::ffi::CString::new(cursor.as_str()).unwrap());
            }
            arguments.push(c"/many".to_owned());
            let borrowed: Vec<&CStr> = arguments.iter().map(|word| word.as_c_str()).collect();
            let (status, fake) = run(many(total), &borrowed);
            assert_eq!(status, 0, "{}", fake.errors());
            let page: Vec<String> = fake.output().lines().map(str::to_owned).collect();
            assert!(!page.is_empty() && page.len() <= 500);
            seen.extend(page);
            match fake.errors().strip_prefix("more: --after ") {
                Some(cursor) => after = Some(cursor.trim_end().to_owned()),
                None => {
                    assert_eq!(fake.errors(), "");
                    break;
                }
            }
        }
        let expected: Vec<String> = (0..total).map(|index| format!("{index:05}")).collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn a_page_is_sorted_cut_at_its_limit_and_names_its_last_entry() {
        let (status, fake) = run(tree(), &[c"--limit", c"1", c"-a", c"/etc"]);
        assert_eq!(status, 0);
        assert_eq!(fake.output(), ".\n");
        assert_eq!(fake.errors(), "more: --after .\n");
        let (_, fake) = run(tree(), &[c"--limit=2", c"--after", c".", c"-a", c"/etc"]);
        assert_eq!(fake.output(), "..\n.hidden\n");
        assert_eq!(fake.errors(), "more: --after .hidden\n");
        let (_, fake) = run(tree(), &[c"--after", c"hostname", c"/etc"]);
        assert_eq!(
            (fake.output().as_str(), fake.errors().as_str()),
            ("motd\n", "")
        );
        let (_, fake) = run(tree(), &[c"--limit", c"2", c"/etc"]);
        assert_eq!(
            (fake.output().as_str(), fake.errors().as_str()),
            ("hostname\nmotd\n", "")
        );
        let (status, fake) = run(tree(), &[c"--limit", c"1", c"-l", c"--json", c"/etc"]);
        assert_eq!(status, 0);
        assert_eq!(fake.output().lines().count(), 1);
        assert!(fake.output().starts_with("{\"name\":\"hostname\""));
        assert_eq!(
            fake.errors(),
            "{\"page\":{\"truncated\":true,\"truncated_by\":\"limit\",\"next_cursor\":\"hostname\"}}\n"
        );
        let fake = tree().with_file("/etc/my notes", 0o644, b"", 0);
        let (_, fake) = run(fake, &[c"--limit", c"1", c"/etc"]);
        assert_eq!(fake.errors(), "more: --after hostname\n");
        let fake = tree().with_file("/etc/a b", 0o644, b"", 0);
        let (_, fake) = run(fake, &[c"--limit", c"1", c"/etc"]);
        assert_eq!(
            (fake.output().as_str(), fake.errors().as_str()),
            ("a b\n", "more: --after a%20b\n")
        );
        let fake = tree().with_file("/etc/a b", 0o644, b"", 0);
        let (_, fake) = run(fake, &[c"--after", c"a%20b", c"--limit", c"1", c"/etc"]);
        assert_eq!(fake.output(), "hostname\n");
    }

    #[test]
    fn a_page_lists_one_folder_and_a_bad_cursor_is_refused() {
        let (status, fake) = run(tree(), &[c"--limit", c"1", c"/etc", c"/bin"]);
        assert_eq!(status, 2);
        assert_eq!(
            fake.errors(),
            "ls: --limit and --after page the entries of one folder\n"
        );
        let (status, _) = run(tree(), &[c"--after", c"x", c"-d", c"/etc"]);
        assert_eq!(status, 2);
        let (status, fake) = run(tree(), &[c"--after", c"-x", c"/etc"]);
        assert_eq!(status, 2);
        assert_eq!(
            fake.errors(),
            "ls: invalid cursor '-x': not one a page ended with\n"
        );
        let (status, fake) = run(tree(), &[c"--limit", c"501", c"/etc"]);
        assert_eq!(status, 2);
        assert!(fake.errors().contains("1 to 500"), "{}", fake.errors());
    }

    fn columns_tree(terminal_width: Option<u16>) -> Fake {
        let mut fake = tree();
        for name in ["a", "bb", "ccc", "dddd", "eeeee", "f", "gg"] {
            fake = fake.with_file(&format!("/c/{name}"), 0o644, b"", 0);
        }
        fake = fake.with_directory("/c", 0);
        fake.terminal_width = terminal_width;
        fake
    }

    #[test]
    fn a_terminal_gets_the_names_in_as_few_rows_as_fit_sorted_down_each_column() {
        let (status, fake) = run(columns_tree(Some(80)), &[c"/c"]);
        assert_eq!(status, 0);
        assert_eq!(fake.output(), "a  bb  ccc  dddd  eeeee  f  gg\n");
        let (_, fake) = run(columns_tree(Some(30)), &[c"/c"]);
        assert_eq!(fake.output(), "a   ccc   eeeee  gg\nbb  dddd  f\n");
        let (_, fake) = run(columns_tree(Some(10)), &[c"/c"]);
        assert_eq!(fake.output(), "a      f\nbb     gg\nccc\ndddd\neeeee\n");
        let (_, fake) = run(columns_tree(Some(1)), &[c"/c"]);
        assert_eq!(fake.output(), "a\nbb\nccc\ndddd\neeeee\nf\ngg\n");
    }

    #[test]
    fn a_line_exactly_as_wide_as_the_terminal_wraps_into_more_rows() {
        let (_, fake) = run(columns_tree(Some(31)), &[c"/c"]);
        assert_eq!(fake.output(), "a  bb  ccc  dddd  eeeee  f  gg\n");
        let (_, fake) = run(columns_tree(Some(30)), &[c"/c"]);
        assert_eq!(fake.output().lines().count(), 2);
    }

    #[test]
    fn a_pipe_gets_one_name_a_line_and_dash_one_and_dash_capital_c_choose() {
        let (_, fake) = run(columns_tree(None), &[c"/c"]);
        assert_eq!(fake.output(), "a\nbb\nccc\ndddd\neeeee\nf\ngg\n");
        let (_, fake) = run(columns_tree(Some(80)), &[c"-1", c"/c"]);
        assert_eq!(fake.output(), "a\nbb\nccc\ndddd\neeeee\nf\ngg\n");
        let (_, fake) = run(columns_tree(None), &[c"-C", c"/c"]);
        assert_eq!(fake.output(), "a  bb  ccc  dddd  eeeee  f  gg\n");
        let (_, fake) = run(columns_tree(Some(0)), &[c"/c"]);
        assert_eq!(fake.output(), "a  bb  ccc  dddd  eeeee  f  gg\n");
        let (_, fake) = run(columns_tree(Some(80)), &[c"-C", c"-1", c"/c"]);
        assert_eq!(fake.output().lines().count(), 7);
    }

    #[test]
    fn the_long_format_and_json_stay_one_entry_a_line_on_a_terminal() {
        let (_, fake) = run(columns_tree(Some(80)), &[c"-l", c"/bin"]);
        assert_eq!(
            fake.output(),
            "total 2\n-rwxr-xr-x 1 0 0 2000 1971-01-01 00:00 ls\n"
        );
        let (_, fake) = run(columns_tree(Some(80)), &[c"--json", c"/c"]);
        assert_eq!(fake.output().lines().count(), 7);
    }

    #[test]
    fn file_operands_and_each_folder_get_columns_of_their_own() {
        let (status, fake) = run(
            columns_tree(Some(80)),
            &[c"/etc/motd", c"/bin/ls", c"/etc", c"/c"],
        );
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "/bin/ls  /etc/motd\n\n/c:\na  bb  ccc  dddd  eeeee  f  gg\n\n/etc:\nhostname  motd\n"
        );
    }

    #[test]
    fn a_name_s_width_counts_characters_not_bytes() {
        assert_eq!(display_width(b"plain"), 5);
        assert_eq!(display_width("caf\u{e9}".as_bytes()), 4);
        assert_eq!(display_width("\u{1f95a}".as_bytes()), 1);
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

    #[test]
    fn json_writes_a_record_per_entry_with_every_field() {
        let (status, fake) = run(tree(), &[c"--json", c"/bin", c"/etc/motd"]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "{\"name\":\"/etc/motd\",\"path\":\"/etc/motd\",\"type\":\"file\",\
             \"permissions\":\"rw-r--r--\",\"links\":1,\"owner\":0,\"group\":0,\
             \"size_bytes\":18,\"modified_at\":\"2023-11-14T22:13:20Z\"}\n\
             {\"name\":\"ls\",\"path\":\"/bin/ls\",\"type\":\"file\",\
             \"permissions\":\"rwxr-xr-x\",\"links\":1,\"owner\":0,\"group\":0,\
             \"size_bytes\":2000,\"modified_at\":\"1971-01-01T00:00:00Z\"}\n"
        );
        let (status, fake) = run(tree(), &[c"/", c"--json"]);
        assert_eq!(status, 0);
        assert!(
            fake.output()
                .starts_with("{\"name\":\"bin\",\"path\":\"/bin\",\"type\":\"folder\"")
        );
        assert_eq!(fake.output().lines().count(), 2);
    }

    #[test]
    fn json_errors_are_coded_records_on_standard_error() {
        let (status, fake) = run(tree(), &[c"--json", c"/nope"]);
        assert_eq!(status, 2);
        assert_eq!(fake.output(), "");
        assert!(
            fake.errors().starts_with(
                "{\"error\":{\"code\":\"no_such_file\",\"category\":\"not_found\",\
                 \"message\":\"cannot access '/nope': No such file or directory\""
            ),
            "{}",
            fake.errors()
        );
        let (status, fake) = run(tree(), &[c"--jsn"]);
        assert_eq!(status, 2);
        assert_eq!(fake.errors(), "ls: unrecognized option '--jsn'\n");
    }
}
