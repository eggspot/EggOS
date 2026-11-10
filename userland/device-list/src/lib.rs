//! The machine's devices as people read them (D184, D220, docs/ux/devices-v0.3.md): the lines
//! of `/proc/driver/devices`, which rows a list shows, in which order, and the words of its
//! columns. `device list`, the MCP tool `device_list` and Settings' About pane all use it, so
//! the three front-ends show the same rows the same way (D22). No allocator.

#![cfg_attr(not(test), no_std)]

/// Where the kernel lists the devices.
pub const DEVICES_FILE: &str = "/proc/driver/devices";

/// The most devices a list holds; the file's lines past it are left out, and said so.
pub const MAX_DEVICES: usize = 256;

const FIELDS: usize = 10;

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

/// An example of the file on an aarch64 `virt` machine, five devices of which the default
/// list shows three: what the front-ends' tests read.
pub const EXAMPLE: &[u8] =
    b"0\t-\tdevicetree\tserial\tpl011@9000000\tarm,pl011\t-\tunbound\t-\t-\n\
1\t-\tdevicetree\tclock\tpl031@9010000\tarm,pl031\tpl031\tbound\tpolled\t-\n\
2\t-\tdevicetree\tbus\tvirtio_mmio@a000000\tvirtio,mmio\tvirtio-mmio\tbound\t-\t-\n\
3\t-\tdevicetree\tbus\tvirtio_mmio@a000200\tvirtio,mmio\t-\tunbound\t-\t-\n\
4\t2\tvirtio\tinput\tinput\tvirtio input\tvirtio-input\tbound\tinterrupts\t-\n";

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

    /// Whether a driver refused it.
    pub fn is_refused(&self) -> bool {
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

/// The rows a list shows, as indices into `devices`, written to the start of `rows` in the
/// list's order (by kind, then by id): every device with `all`, else those
/// [`shown_by_default`]. Returns how many; at most `rows.len()`.
pub fn rows(devices: &[Device<'_>], all: bool, rows: &mut [usize]) -> usize {
    let mut count = 0;
    for (index, device) in devices.iter().enumerate() {
        if count == rows.len() {
            break;
        }
        if all || shown_by_default(device, devices) {
            rows[count] = index;
            count += 1;
        }
    }
    rows[..count].sort_unstable_by_key(|&index| (devices[index].kind_order(), devices[index].id));
    count
}

/// The devices `text` describes, written to the start of `devices` (at most its length), how
/// many, and whether some were left out.
pub fn parse_all<'a>(text: &'a [u8], devices: &mut [Device<'a>]) -> (usize, bool) {
    let mut count = 0;
    for line in text.split_inclusive(|&byte| byte == b'\n') {
        let Some(device) = Device::parse(line) else {
            continue;
        };
        if count == devices.len() {
            return (count, true);
        }
        devices[count] = device;
        count += 1;
    }
    (count, false)
}

#[cfg(test)]
mod tests;
