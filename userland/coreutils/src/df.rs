use crate::disk::{ABSOLUTE_CAPACITY, absolute_path};
use crate::lines::each_line;
use crate::links::PathText;
use crate::options::parse_defined;
use crate::output::{FAILURE, Output, decimal};
use crate::system::{FileSystemUsage, System};
use core::ffi::CStr;
use core::ops::ControlFlow;
use eggos_commands::{commands, errors};
use eggos_mcp::json::JsonWriter;

const MOUNTS: &CStr = c"/proc/mounts";

const MAX_MOUNTS: usize = 32;

const NAME_CAPACITY: usize = 32;

const POINT_CAPACITY: usize = 256;

const RECORD_BYTES: usize = 2 * POINT_CAPACITY + 256;

const NAME_WIDTH: usize = 14;

const TYPE_WIDTH: usize = 8;

const NUMBER_WIDTH: usize = 10;

const PERCENT_WIDTH: usize = 4;

const KIBIBYTE: u64 = 1024;

const HUMAN_UNITS: [u8; 6] = *b"KMGTPE";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Chosen {
    human: bool,
    types: bool,
    all: bool,
    json: bool,
}

#[derive(Debug, Clone, Copy)]
struct Mount {
    source: [u8; NAME_CAPACITY],
    source_length: usize,
    kind: [u8; NAME_CAPACITY],
    kind_length: usize,
    point: [u8; POINT_CAPACITY],
    point_length: usize,
}

impl Mount {
    const EMPTY: Self = Self {
        source: [0; NAME_CAPACITY],
        source_length: 0,
        kind: [0; NAME_CAPACITY],
        kind_length: 0,
        point: [0; POINT_CAPACITY],
        point_length: 0,
    };

    fn parse(line: &[u8]) -> Option<Self> {
        let mut fields = line
            .split(|&byte| byte == b' ' || byte == b'\n')
            .filter(|field| !field.is_empty());
        let source = fields.next()?;
        let point = fields.next()?;
        let kind = fields.next()?;
        if source.len() > NAME_CAPACITY
            || kind.len() > NAME_CAPACITY
            || point.len() > POINT_CAPACITY
            || point.first() != Some(&b'/')
        {
            return None;
        }
        let mut mount = Self::EMPTY;
        mount.source[..source.len()].copy_from_slice(source);
        mount.source_length = source.len();
        mount.kind[..kind.len()].copy_from_slice(kind);
        mount.kind_length = kind.len();
        mount.point[..point.len()].copy_from_slice(point);
        mount.point_length = point.len();
        Some(mount)
    }

    fn source(&self) -> &[u8] {
        &self.source[..self.source_length]
    }

    fn kind(&self) -> &[u8] {
        &self.kind[..self.kind_length]
    }

    fn point(&self) -> &[u8] {
        &self.point[..self.point_length]
    }

    fn holds(&self, path: &[u8]) -> bool {
        let point = self.point();
        point == b"/"
            || path == point
            || path
                .strip_prefix(point)
                .is_some_and(|rest| rest.starts_with(b"/"))
    }
}

struct Mounts {
    mounts: [Mount; MAX_MOUNTS],
    count: usize,
}

impl Mounts {
    fn new() -> Self {
        Self {
            mounts: [Mount::EMPTY; MAX_MOUNTS],
            count: 0,
        }
    }

    fn add(&mut self, line: &[u8]) {
        if let (Some(mount), Some(slot)) = (Mount::parse(line), self.mounts.get_mut(self.count)) {
            *slot = mount;
            self.count += 1;
        }
    }

    fn all(&self) -> &[Mount] {
        &self.mounts[..self.count]
    }

    fn holding(&self, path: &[u8]) -> Option<&Mount> {
        self.all()
            .iter()
            .filter(|mount| mount.holds(path))
            .max_by_key(|mount| mount.point_length)
    }
}

pub fn df<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut chosen = Chosen::default();
    let Ok(operands) = parse_defined(output, arguments, &commands::DF, |_, given| {
        match (given.option.short, given.option.long) {
            (Some(b'h'), _) => chosen.human = true,
            (Some(b'k'), _) => chosen.human = false,
            (Some(b'T'), _) => chosen.types = true,
            (Some(b'a'), _) => chosen.all = true,
            (_, Some("json")) => chosen.json = true,
            _ => {}
        }
        Ok(())
    }) else {
        return FAILURE;
    };
    let mut mounts = Mounts::new();
    let status = each_line(output, MOUNTS, |_, line| {
        mounts.add(line);
        ControlFlow::Continue(())
    });
    if status != 0 {
        return status;
    }
    if !chosen.json {
        write_header(output, chosen);
    }
    let mut status = 0;
    if operands.all().is_empty() {
        for mount in mounts.all() {
            let Ok(path) = PathText::of(mount.point()) else {
                continue;
            };
            match output.system.file_system_usage(path.as_c_str()) {
                Ok(usage) if usage.blocks > 0 || chosen.all => {
                    write_usage(output, chosen, Some(mount), &usage);
                }
                Ok(_) => {}
                Err(errno) => {
                    output.file_error(path.as_c_str(), errno);
                    status = FAILURE;
                }
            }
        }
        return status;
    }
    for &path in operands.all() {
        let usage = match output.system.file_system_usage(path) {
            Ok(usage) => usage,
            Err(errno) => {
                output.file_error(path, errno);
                status = FAILURE;
                continue;
            }
        };
        let mut absolute = [0u8; ABSOLUTE_CAPACITY];
        let mount = absolute_path(&mut output.system, path.to_bytes(), &mut absolute)
            .and_then(|length| mounts.holding(&absolute[..length]));
        write_usage(output, chosen, mount, &usage);
    }
    status
}

fn write_padded<S: System>(output: &mut Output<S>, text: &[u8], width: usize) {
    output.write(text);
    for _ in text.len()..width {
        output.write(b" ");
    }
}

fn write_right<S: System>(output: &mut Output<S>, text: &[u8], width: usize) {
    output.write(b" ");
    for _ in text.len()..width {
        output.write(b" ");
    }
    output.write(text);
}

fn write_header<S: System>(output: &mut Output<S>, chosen: Chosen) {
    write_padded(output, b"Filesystem", NAME_WIDTH);
    if chosen.types {
        output.write(b" ");
        write_padded(output, b"Type", TYPE_WIDTH);
    }
    let names: [&[u8]; 3] = match chosen.human {
        true => [b"Size", b"Used", b"Avail"],
        false => [b"1K-blocks", b"Used", b"Available"],
    };
    for name in names {
        write_right(output, name, NUMBER_WIDTH);
    }
    output.write(b" Use% Mounted on\n");
}

fn write_usage<S: System>(
    output: &mut Output<S>,
    chosen: Chosen,
    mount: Option<&Mount>,
    usage: &FileSystemUsage,
) {
    let (source, kind, point) = match mount {
        Some(mount) => (mount.source(), mount.kind(), mount.point()),
        None => (&b"-"[..], &b"-"[..], &b"-"[..]),
    };
    if chosen.json {
        write_record(output, source, kind, point, usage);
        return;
    }
    write_padded(output, source, NAME_WIDTH);
    if chosen.types {
        output.write(b" ");
        write_padded(output, kind, TYPE_WIDTH);
    }
    for bytes in [
        usage.size_bytes(),
        usage.used_bytes(),
        usage.available_bytes(),
    ] {
        let mut text = [0u8; 24];
        let shown = match chosen.human {
            true => human_size(bytes, &mut text),
            false => kibibytes(bytes, &mut text),
        };
        write_right(output, shown, NUMBER_WIDTH);
    }
    let mut percent = [0u8; 24];
    let shown = used_percent(usage.used_bytes(), usage.available_bytes(), &mut percent);
    write_right(output, shown, PERCENT_WIDTH);
    output.write(b" ");
    output.write(point);
    output.write(b"\n");
}

fn write_record<S: System>(
    output: &mut Output<S>,
    source: &[u8],
    kind: &[u8],
    point: &[u8],
    usage: &FileSystemUsage,
) {
    let mut buffer = [0u8; RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("file_system");
    out.string_lossy(source);
    out.key("type");
    out.string_lossy(kind);
    out.key("mounted_on");
    out.string_lossy(point);
    out.key("size_bytes");
    out.u64(usage.size_bytes());
    out.key("used_bytes");
    out.u64(usage.used_bytes());
    out.key("available_bytes");
    out.u64(usage.available_bytes());
    out.key("read_only");
    out.bool(usage.read_only);
    out.end_object();
    out.end_line();
    match out.overflowed() {
        true => output.coded(&errors::FAILED, &[b"a record too long for JSON"]),
        false => output.write(out.as_bytes()),
    }
}

fn kibibytes(bytes: u64, text: &mut [u8; 24]) -> &[u8] {
    let mut digits = [0u8; 20];
    let shown = decimal(bytes.div_ceil(KIBIBYTE), &mut digits);
    text[..shown.len()].copy_from_slice(shown);
    &text[..shown.len()]
}

fn used_percent(used: u64, available: u64, text: &mut [u8; 24]) -> &[u8] {
    let total = u128::from(used) + u128::from(available);
    if total == 0 {
        text[0] = b'-';
        return &text[..1];
    }
    let percent = (u128::from(used) * 100).div_ceil(total) as u64;
    let mut digits = [0u8; 20];
    let shown = decimal(percent, &mut digits);
    text[..shown.len()].copy_from_slice(shown);
    text[shown.len()] = b'%';
    &text[..=shown.len()]
}

pub fn human_size(bytes: u64, text: &mut [u8; 24]) -> &[u8] {
    let mut digits = [0u8; 20];
    if bytes < KIBIBYTE {
        let shown = decimal(bytes, &mut digits);
        text[..shown.len()].copy_from_slice(shown);
        return &text[..shown.len()];
    }
    let bytes = u128::from(bytes);
    let mut unit = 0;
    let mut divisor = u128::from(KIBIBYTE);
    while unit + 1 < HUMAN_UNITS.len() && bytes.div_ceil(divisor) >= u128::from(KIBIBYTE) {
        unit += 1;
        divisor *= u128::from(KIBIBYTE);
    }
    let tenths = (bytes * 10).div_ceil(divisor);
    let mut length = 0;
    let mut put = |part: &[u8]| {
        text[length..length + part.len()].copy_from_slice(part);
        length += part.len();
    };
    if tenths < 100 {
        put(decimal((tenths / 10) as u64, &mut digits));
        put(b".");
        put(decimal((tenths % 10) as u64, &mut digits));
    } else {
        put(decimal(bytes.div_ceil(divisor) as u64, &mut digits));
    }
    put(&HUMAN_UNITS[unit..=unit]);
    &text[..length]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use crate::utilities::run;

    const GIBIBYTE: u64 = 1 << 30;

    fn machine() -> Fake {
        let mut fake = Fake::default()
            .with_directory("/home", 0)
            .with_directory("/home/root", 0)
            .with_directory("/proc", 0)
            .with_directory("/tmp", 0)
            .with_file(
                "/proc/mounts",
                0o444,
                b"rootfs / rootfs ro 0 0\nproc /proc proc ro 0 0\neggfs /home eggfs rw 0 0\n\
                  tmpfs /tmp tmpfs rw 0 0\n",
                0,
            );
        fake.file_systems = vec![
            (
                b"/".to_vec(),
                FileSystemUsage {
                    block_size: 4096,
                    blocks: 1024,
                    read_only: true,
                    ..FileSystemUsage::default()
                },
            ),
            (
                b"/proc".to_vec(),
                FileSystemUsage {
                    block_size: 4096,
                    read_only: true,
                    ..FileSystemUsage::default()
                },
            ),
            (
                b"/home".to_vec(),
                FileSystemUsage {
                    block_size: 4096,
                    blocks: 262_144,
                    free: 196_608,
                    available: 196_608,
                    read_only: false,
                },
            ),
            (
                b"/tmp".to_vec(),
                FileSystemUsage {
                    block_size: 4096,
                    blocks: 8192,
                    free: 8192,
                    available: 8192,
                    read_only: false,
                },
            ),
        ];
        fake
    }

    fn df_output(arguments: &[&CStr]) -> (i32, String, String) {
        let mut fake = machine();
        let words: Vec<&CStr> = [c"df"]
            .into_iter()
            .chain(arguments.iter().copied())
            .collect();
        let status = run(&mut fake, &words);
        (status, fake.output(), fake.errors())
    }

    #[test]
    fn sizes_are_shown_as_gnu_shows_them_with_h() {
        let mut text = [0u8; 24];
        for (bytes, shown) in [
            (0, "0"),
            (1023, "1023"),
            (1024, "1.0K"),
            (1536, "1.5K"),
            (10 * 1024, "10K"),
            (268_435_456, "256M"),
            (GIBIBYTE, "1.0G"),
            (GIBIBYTE + 1, "1.1G"),
            (1023 * 1024 + 1, "1.0M"),
            (12 * GIBIBYTE, "12G"),
        ] {
            assert_eq!(human_size(bytes, &mut text), shown.as_bytes(), "{bytes}");
        }
    }

    #[test]
    fn without_paths_df_lists_every_file_system_with_blocks_and_all_with_a() {
        let (status, output, _) = df_output(&[]);
        assert_eq!(status, 0);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 4, "{output}");
        assert!(lines[0].starts_with("Filesystem      1K-blocks"));
        assert!(lines[1].starts_with("rootfs ") && lines[1].ends_with(" /"));
        assert!(lines[2].ends_with(" 25% /home"), "{output}");
        assert!(lines[3].ends_with("  0% /tmp"), "{output}");
        let (_, all, _) = df_output(&[c"-a"]);
        assert!(
            all.lines().any(|line| line.ends_with("   - /proc")),
            "{all}"
        );
    }

    #[test]
    fn a_path_shows_the_file_system_holding_it_and_a_missing_one_fails() {
        let (status, output, _) = df_output(&[c"-T", c"/home/root"]);
        assert_eq!(status, 0);
        assert!(output.starts_with("Filesystem     Type    "), "{output}");
        assert!(output.contains("\neggfs          eggfs   "), "{output}");
        let (status, _, errors) = df_output(&[c"/nowhere"]);
        assert_eq!(status, FAILURE);
        assert!(
            errors.contains("/nowhere: No such file or directory"),
            "{errors}"
        );
    }

    #[test]
    fn json_writes_a_record_per_file_system_in_bytes() {
        let (status, output, _) = df_output(&[c"--json", c"/tmp"]);
        assert_eq!(status, 0);
        assert_eq!(
            output,
            "{\"file_system\":\"tmpfs\",\"type\":\"tmpfs\",\"mounted_on\":\"/tmp\",\
             \"size_bytes\":33554432,\"used_bytes\":0,\"available_bytes\":33554432,\
             \"read_only\":false}\n"
        );
    }
}
