//! `device list` (D184): the machine's devices, read from `/proc/driver/devices`, with the rows,
//! order and words Settings' About pane and the MCP tool `device_list` show
//! (docs/ux/devices-v0.3.md): by default the devices a person recognises that are no other
//! device's parent, and every refused one; with `-a` every device. Sorted by kind, then by id.

use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

const DEVICES: &CStr = c"/proc/driver/devices";
const TEXT_CAPACITY: usize = 32 * 1024;
const RECORD_BYTES: usize = 1024;
const FIELDS: usize = 10;

/// The most devices a list shows; the file's lines past it are left out, and said so.
pub const MAX_DEVICES: usize = 256;

/// Every class in the order lists show them, with the kind a person reads.
pub const KINDS: [(&str, &str); 11] = [
    ("display", "Display"),
    ("input", "Input"),
    ("disk", "Disk"),
    ("network", "Network"),
    ("console", "Console"),
    ("serial", "Serial port"),
    ("clock", "Clock"),
    ("entropy", "Random numbers"),
    ("bus", "Bus"),
    ("interrupt-controller", "Interrupt controller"),
    ("other", "Other"),
];

const RECOGNISED: [&str; 8] = [
    "display", "input", "disk", "network", "console", "serial", "clock", "entropy",
];

const ID_WIDTH: usize = 4;
const PARENT_WIDTH: usize = 8;
const KIND_WIDTH: usize = 22;
const DELIVERY_WIDTH: usize = 12;
const DEVICE_WIDTH: usize = 42;

/// One line of `/proc/driver/devices`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device<'a> {
    pub id: u64,
    pub parent: Option<u64>,
    pub bus: &'a [u8],
    pub class: &'a [u8],
    pub name: &'a [u8],
    pub description: &'a [u8],
    pub driver: Option<&'a [u8]>,
    pub state: &'a [u8],
    pub delivery: Option<&'a [u8]>,
    pub reason: Option<&'a [u8]>,
}

fn present(field: &[u8]) -> Option<&[u8]> {
    (field != b"-").then_some(field)
}

fn number(field: &[u8]) -> Option<u64> {
    if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
        return None;
    }
    field.iter().try_fold(0u64, |value, &digit| {
        value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
    })
}

impl<'a> Device<'a> {
    /// No device: what a list's unused slots hold.
    pub const EMPTY: Device<'static> = Device {
        id: 0,
        parent: None,
        bus: b"",
        class: b"",
        name: b"",
        description: b"",
        driver: None,
        state: b"",
        delivery: None,
        reason: None,
    };

    /// The device a line of the file describes; `None` for a line that is not one.
    pub fn parse(line: &'a [u8]) -> Option<Self> {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let mut fields = [&[][..]; FIELDS];
        let mut parts = line.split(|&byte| byte == b'\t');
        for field in &mut fields {
            *field = parts.next()?;
        }
        if parts.next().is_some() {
            return None;
        }
        let [
            id,
            parent,
            bus,
            class,
            name,
            description,
            driver,
            state,
            delivery,
            reason,
        ] = fields;
        Some(Self {
            id: number(id)?,
            parent: match present(parent) {
                Some(parent) => Some(number(parent)?),
                None => None,
            },
            bus,
            class,
            name,
            description,
            driver: present(driver),
            state,
            delivery: present(delivery),
            reason: present(reason),
        })
    }

    /// Where its class comes in [`KINDS`]; unknown classes go with `other`.
    pub fn kind_order(&self) -> usize {
        KINDS
            .iter()
            .position(|(class, _)| class.as_bytes() == self.class)
            .unwrap_or(KINDS.len() - 1)
    }

    /// The kind a person reads: `Display`, `Serial port`, ...
    pub fn kind(&self) -> &'static str {
        KINDS[self.kind_order()].1
    }

    fn is_refused(&self) -> bool {
        self.state == b"refused"
    }

    /// The `Device` column: a virtio device's description (`virtio gpu`), else its name and
    /// description joined by ` · `.
    pub fn write_device(&self, out: &mut dyn FnMut(&[u8])) {
        if self.bus == b"virtio" {
            out(self.description);
            return;
        }
        out(self.name);
        out(" · ".as_bytes());
        out(self.description);
    }

    /// The `Driver` column: the driver's name, `No driver`, or `Refused: <reason>`.
    pub fn write_driver(&self, out: &mut dyn FnMut(&[u8])) {
        match (self.is_refused(), self.driver) {
            (true, _) => {
                out(b"Refused: ");
                out(self.reason.unwrap_or(b"no reason given"));
            }
            (false, Some(driver)) if self.state == b"bound" => out(driver),
            _ => out(b"No driver"),
        }
    }
}

/// Whether the default list shows `device` of `devices`: a class a person recognises and no
/// device's parent, or refused.
pub fn shown_by_default(device: &Device<'_>, devices: &[Device<'_>]) -> bool {
    if device.is_refused() {
        return true;
    }
    let recognised = RECOGNISED
        .iter()
        .any(|class| class.as_bytes() == device.class);
    recognised && !devices.iter().any(|other| other.parent == Some(device.id))
}

/// The rows a list shows, as indices into `devices`, in its order: by kind, then by id.
pub fn rows(devices: &[Device<'_>], all: bool, rows: &mut [usize; MAX_DEVICES]) -> usize {
    let mut count = 0;
    for (index, device) in devices.iter().enumerate() {
        if all || shown_by_default(device, devices) {
            rows[count] = index;
            count += 1;
        }
    }
    rows[..count].sort_unstable_by_key(|&index| (devices[index].kind_order(), devices[index].id));
    count
}

/// The devices `text` describes, at most [`MAX_DEVICES`] of them, and whether some were left
/// out.
pub fn parse_all<'a>(text: &'a [u8], devices: &mut [Device<'a>; MAX_DEVICES]) -> (usize, bool) {
    let mut count = 0;
    for line in text.split_inclusive(|&byte| byte == b'\n') {
        let Some(device) = Device::parse(line) else {
            continue;
        };
        if count == MAX_DEVICES {
            return (count, true);
        }
        devices[count] = device;
        count += 1;
    }
    (count, false)
}

pub fn device<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some((verb, rest)) = arguments.split_first() else {
        output.coded(&errors::USAGE, &[b"usage: device list [-a] [--json]"]);
        return FAILURE;
    };
    if verb.to_bytes() != b"list" {
        output.coded(
            &errors::USAGE,
            &[
                b"unknown verb '",
                verb.to_bytes(),
                b"': device list [-a] [--json]",
            ],
        );
        return FAILURE;
    }
    list(output, rest)
}

fn list<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut all = false;
    let mut json = false;
    let parsed = parse_defined(output, arguments, &commands::DEVICE_LIST, |_, given| {
        match (given.option.short, given.option.long) {
            (Some(b'a'), _) => all = true,
            (_, Some("json")) => json = true,
            _ => {}
        }
        Ok(())
    });
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    if let Some(extra) = operands.all().first() {
        output.coded(
            &errors::USAGE,
            &[b"extra operand '", extra.to_bytes(), b"'"],
        );
        return FAILURE;
    }
    let mut text = [0u8; TEXT_CAPACITY];
    let length = match read_devices(&mut output.system, &mut text) {
        Ok(length) => length,
        Err(errno) => {
            output.file_error(DEVICES, errno);
            return FAILURE;
        }
    };
    let mut parsed = [Device::EMPTY; MAX_DEVICES];
    let (count, cut) = parse_all(&text[..length], &mut parsed);
    let devices = &parsed[..count];
    let mut order = [0usize; MAX_DEVICES];
    let shown = rows(devices, all, &mut order);
    if !json {
        write_header(output);
    }
    for &index in &order[..shown] {
        match json {
            true => write_record(output, &devices[index]),
            false => write_row(output, &devices[index]),
        }
    }
    if cut {
        output.error(&[b"more devices than a list holds: the rest are left out"]);
        return FAILURE;
    }
    0
}

fn read_devices<S: System>(system: &mut S, text: &mut [u8]) -> Result<usize, Errno> {
    let descriptor = system.open(DEVICES)?;
    let mut length = 0;
    let read = loop {
        if length == text.len() {
            break Ok(());
        }
        match system.read(descriptor, &mut text[length..]) {
            Ok(0) => break Ok(()),
            Ok(count) => length += count,
            Err(Errno::EINTR) => {}
            Err(errno) => break Err(errno),
        }
    };
    system.close(descriptor);
    read.map(|()| length)
}

fn columns(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| byte & 0xc0 != 0x80).count()
}

fn write_padded<S: System>(output: &mut Output<S>, cell: &[u8], width: usize) {
    output.write(cell);
    for _ in columns(cell)..width.max(columns(cell) + 1) {
        output.write(b" ");
    }
}

fn write_header<S: System>(output: &mut Output<S>) {
    write_padded(output, b"ID", ID_WIDTH);
    write_padded(output, b"PARENT", PARENT_WIDTH);
    write_padded(output, b"KIND", KIND_WIDTH);
    write_padded(output, b"DELIVERY", DELIVERY_WIDTH);
    write_padded(output, b"DEVICE", DEVICE_WIDTH);
    output.write(b"DRIVER\n");
}

fn write_row<S: System>(output: &mut Output<S>, device: &Device<'_>) {
    let mut digits = [0u8; 20];
    write_padded(
        output,
        crate::output::decimal(device.id, &mut digits),
        ID_WIDTH,
    );
    let mut digits = [0u8; 20];
    let parent = match device.parent {
        Some(parent) => crate::output::decimal(parent, &mut digits),
        None => b"-",
    };
    write_padded(output, parent, PARENT_WIDTH);
    write_padded(output, device.kind().as_bytes(), KIND_WIDTH);
    write_padded(output, device.delivery.unwrap_or(b"-"), DELIVERY_WIDTH);
    let mut cell = [0u8; RECORD_BYTES];
    let mut length = 0;
    device.write_device(&mut |bytes| {
        let end = (length + bytes.len()).min(cell.len());
        cell[length..end].copy_from_slice(&bytes[..end - length]);
        length = end;
    });
    write_padded(output, &cell[..length], DEVICE_WIDTH);
    device.write_driver(&mut |bytes| output.write(bytes));
    output.write(b"\n");
}

fn write_record<S: System>(output: &mut Output<S>, device: &Device<'_>) {
    let mut buffer = [0u8; RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    let text_or_null = |out: &mut JsonWriter<'_>, value: Option<&[u8]>| match value {
        Some(value) => out.string_lossy(value),
        None => out.null(),
    };
    out.begin_object();
    out.key("id");
    out.u64(device.id);
    out.key("parent");
    match device.parent {
        Some(parent) => out.u64(parent),
        None => out.null(),
    }
    for (key, value) in [
        ("bus", device.bus),
        ("class", device.class),
        ("name", device.name),
        ("description", device.description),
    ] {
        out.key(key);
        out.string_lossy(value);
    }
    out.key("driver");
    text_or_null(&mut out, device.driver);
    out.key("state");
    out.string_lossy(device.state);
    out.key("delivery");
    text_or_null(&mut out, device.delivery);
    out.key("reason");
    text_or_null(&mut out, device.reason);
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        output.write(out.as_bytes());
    }
}

#[cfg(test)]
pub(crate) mod tests;
