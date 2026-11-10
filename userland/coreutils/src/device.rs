//! `device list` (D184): the machine's devices, read from `/proc/driver/devices`, with the rows,
//! order and words Settings' About pane and the MCP tool `device_list` show
//! (docs/ux/devices-v0.3.md): by default the devices a person recognises that are no other
//! device's parent, and every refused one; with `-a` every device. Sorted by kind, then by id.

use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
pub use eggos_device_list::{Device, MAX_DEVICES, parse_all, rows};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

const DEVICES: &CStr = c"/proc/driver/devices";
const TEXT_CAPACITY: usize = 32 * 1024;
const RECORD_BYTES: usize = 1024;

const ID_WIDTH: usize = 4;
const PARENT_WIDTH: usize = 8;
const KIND_WIDTH: usize = 22;
const DELIVERY_WIDTH: usize = 12;
const DEVICE_WIDTH: usize = 42;

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
