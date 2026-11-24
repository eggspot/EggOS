use crate::network::{Interfaces, NEIGHBOURS, Shown, interfaces, read_interfaces, read_text};
use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use crate::utilities::MAX_ARGUMENTS;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_net::dhcp::INFINITE_LEASE_S;
use eggos_net::status::{Dhcp, InterfaceStatus, NO_ADAPTER_WORD, parse_neighbour};

const USAGE: &[u8] = b"usage: ip [-s] [-j] [address | link | route | neigh] [show [dev <name>]]\n       ip link set dev <name> [mtu <bytes>] [wake magic | wake off]";
const SIOCSIFMTU: u32 = 0x8922;
const SIOCETHTOOL: u32 = 0x8946;
const ETHTOOL_SET_WAKE: u32 = 6;
const WAKE_MAGIC: u32 = 1 << 5;
const IFREQ_NAME_BYTES: usize = 16;
const IFREQ_BYTES: usize = 40;
const WOLINFO_BYTES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Object {
    Address,
    Link,
    Route,
    Neighbour,
}

fn object(word: &[u8]) -> Option<Object> {
    match word {
        b"a" | b"addr" | b"address" => Some(Object::Address),
        b"l" | b"link" => Some(Object::Link),
        b"r" | b"ro" | b"route" => Some(Object::Route),
        b"n" | b"neigh" | b"neighbor" | b"neighbour" => Some(Object::Neighbour),
        _ => None,
    }
}

fn iproute2_word(argument: &CStr) -> &CStr {
    match argument.to_bytes() {
        b"-json" => c"--json",
        b"-stats" | b"-statistics" => c"-s",
        _ => argument,
    }
}

pub fn ip<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut words = [c""; MAX_ARGUMENTS];
    if arguments.len() > words.len() {
        return usage(output);
    }
    for (word, &argument) in words.iter_mut().zip(arguments) {
        *word = iproute2_word(argument);
    }
    let mut statistics = false;
    let mut help = false;
    let mut link_options: [Option<&[u8]>; 3] = [None; 3];
    let parsed = parse_defined(
        output,
        &words[..arguments.len()],
        &commands::IP,
        |output, given| {
            match (given.option.short, given.option.long) {
                (Some(b's'), _) => statistics = true,
                (Some(b'j'), _) => output.use_json(),
                (Some(b'h'), _) => help = true,
                (None, Some("dev")) => link_options[0] = given.value,
                (None, Some("mtu")) => link_options[1] = given.value,
                (None, Some("wake")) => link_options[2] = given.value,
                _ => {}
            }
            Ok(())
        },
    );
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    if help {
        output.write(USAGE);
        output.write(b"\n");
        return 0;
    }
    let json = output.is_json();
    let all: [&[u8]; MAX_ARGUMENTS] = core::array::from_fn(|index| {
        operands.all().get(index).map_or(&b""[..], |operand| operand.to_bytes())
    });
    let given = &all[..operands.all().len().min(MAX_ARGUMENTS)];
    if let [b"l" | b"link", b"set", rest @ ..] = given {
        let mut words: [&[u8]; MAX_ARGUMENTS + 6] = [b""; MAX_ARGUMENTS + 6];
        let mut count = 0;
        for (keyword, value) in [&b"dev"[..], b"mtu", b"wake"].into_iter().zip(link_options) {
            if let Some(value) = value {
                words[count] = keyword;
                words[count + 1] = value;
                count += 2;
            }
        }
        for word in rest {
            if count < words.len() {
                words[count] = word;
                count += 1;
            }
        }
        return set_link(output, &words[..count]);
    }
    let mut chosen = None;
    let mut device: Option<&[u8]> = None;
    let mut words = operands.all().iter().map(|operand| operand.to_bytes());
    while let Some(word) = words.next() {
        match word {
            b"show" | b"list" | b"ls" if chosen.is_some() => {}
            b"dev" if chosen.is_some() => match words.next() {
                Some(name) => device = Some(name),
                None => return usage(output),
            },
            b"help" => {
                output.write(USAGE);
                output.write(b"\n");
                return 0;
            }
            word if chosen.is_none() => match object(word) {
                Some(found) => chosen = Some(found),
                None => {
                    output.coded(
                        &errors::USAGE,
                        &[b"object \"", word, b"\" is unknown, try \"ip help\""],
                    );
                    return FAILURE;
                }
            },
            _ => return usage(output),
        }
    }
    let read = match read_interfaces(output) {
        Ok(read) => read,
        Err(status) => return status,
    };
    if let Some(name) = device
        && !interfaces(&read).any(|status| status.name.as_bytes() == name)
    {
        output.error(&[b"device \"", name, b"\" does not exist"]);
        return FAILURE;
    }
    let shown =
        |status: &&InterfaceStatus| device.is_none_or(|name| status.name.as_bytes() == name);
    match (chosen.unwrap_or(Object::Address), json) {
        (_, true) => write_json(output, &read),
        (Object::Address, false) => {
            for (index, status) in numbered(&read).filter(|(_, status)| shown(status)) {
                write_link(output, index, status, statistics);
                write_addresses(output, status);
            }
            0
        }
        (Object::Link, false) => {
            for (index, status) in numbered(&read).filter(|(_, status)| shown(status)) {
                write_link(output, index, status, statistics);
            }
            0
        }
        (Object::Route, false) => {
            for status in interfaces(&read).filter(|status| shown(status)) {
                write_routes(output, status);
            }
            0
        }
        (Object::Neighbour, false) => write_neighbours(output, device),
    }
}

/// `ip link set dev <name> [mtu <bytes>] [wake magic|off]` (D708), through `SIOCSIFMTU` and
/// `SIOCETHTOOL` on a socket, as iproute2 and ethtool do.
fn set_link<S: System>(output: &mut Output<S>, words: &[&[u8]]) -> i32 {
    let mut name: Option<&[u8]> = None;
    let mut mtu: Option<u32> = None;
    let mut wake: Option<bool> = None;
    let mut rest = words.iter();
    while let Some(&word) = rest.next() {
        match (word, rest.next().copied()) {
            (b"dev", Some(value)) => name = Some(value),
            (b"mtu", Some(value)) => match crate::wm::number(value) {
                Some(bytes) if (68..=65_535).contains(&bytes) => mtu = Some(bytes.unsigned_abs()),
                _ => {
                    output.error(&[b"invalid MTU \"", value, b"\""]);
                    return FAILURE;
                }
            },
            (b"wake", Some(b"magic" | b"on")) => wake = Some(true),
            (b"wake", Some(b"off")) => wake = Some(false),
            _ => return usage(output),
        }
    }
    let Some(name) = name.filter(|name| !name.is_empty() && name.len() < IFREQ_NAME_BYTES) else {
        return usage(output);
    };
    if mtu.is_none() && wake.is_none() {
        return usage(output);
    }
    let socket = match output
        .system
        .open_datagram_socket(crate::system::DatagramKind::Udp)
    {
        Ok(socket) => socket,
        Err(errno) => {
            output.error(&[b"cannot open a socket: ", errno.message()]);
            return FAILURE;
        }
    };
    let mut ifreq = [0u8; IFREQ_BYTES];
    ifreq[..name.len()].copy_from_slice(name);
    let mut status = 0;
    if let Some(mtu) = mtu {
        ifreq[IFREQ_NAME_BYTES..IFREQ_NAME_BYTES + 4].copy_from_slice(&mtu.to_ne_bytes());
        if let Err(errno) = output.system.device_control(socket, SIOCSIFMTU, &mut ifreq) {
            link_refused(output, name, b"mtu", errno);
            status = FAILURE;
        }
    }
    if let Some(on) = wake {
        let mut wolinfo = [0u8; WOLINFO_BYTES];
        wolinfo[..4].copy_from_slice(&ETHTOOL_SET_WAKE.to_ne_bytes());
        let wanted = if on { WAKE_MAGIC } else { 0 };
        wolinfo[8..12].copy_from_slice(&wanted.to_ne_bytes());
        let pointer = wolinfo.as_mut_ptr() as usize as u64;
        ifreq[IFREQ_NAME_BYTES..IFREQ_NAME_BYTES + 8].copy_from_slice(&pointer.to_ne_bytes());
        if let Err(errno) = output.system.device_control(socket, SIOCETHTOOL, &mut ifreq) {
            link_refused(output, name, b"wake", errno);
            status = FAILURE;
        }
    }
    output.system.close(socket);
    status
}

fn link_refused<S: System>(
    output: &mut Output<S>,
    name: &[u8],
    setting: &[u8],
    errno: eggos_runtime::errno::Errno,
) {
    let why: &[u8] = match errno {
        eggos_runtime::errno::Errno::EPERM => b"only root may change it",
        eggos_runtime::errno::Errno::ENODEV => b"no such device",
        eggos_runtime::errno::Errno::EOPNOTSUPP => b"the adapter cannot do it",
        eggos_runtime::errno::Errno::EINVAL => b"the adapter does not take that value",
        other => other.message(),
    };
    output.error(&[b"cannot set ", setting, b" of \"", name, b"\": ", why]);
}

fn usage<S: System>(output: &mut Output<S>) -> i32 {
    output.coded(&errors::USAGE, &[USAGE]);
    FAILURE
}

fn numbered(read: &Interfaces) -> impl Iterator<Item = (usize, &InterfaceStatus)> {
    interfaces(read)
        .enumerate()
        .map(|(index, status)| (index + 1, status))
}

fn write_link<S: System>(
    output: &mut Output<S>,
    index: usize,
    status: &InterfaceStatus,
    statistics: bool,
) {
    output.write_number(index as u64, 0);
    output.write(b": ");
    output.write(status.name.as_bytes());
    let flags: &[u8] = match (status.is_loopback(), status.link_up) {
        (true, _) => b": <LOOPBACK,UP,LOWER_UP> mtu ",
        (false, true) => b": <BROADCAST,UP,LOWER_UP> mtu ",
        (false, false) => b": <NO-CARRIER,BROADCAST,UP> mtu ",
    };
    output.write(flags);
    output.write_number(u64::from(status.mtu), 0);
    let state: &[u8] = match (status.is_loopback(), status.link_up) {
        (true, _) => b" state UNKNOWN",
        (false, true) => b" state UP",
        (false, false) => b" state DOWN",
    };
    output.write(state);
    if status.dhcp != Dhcp::Off {
        output.write(b" dhcp ");
        output.write(status.dhcp.word().as_bytes());
    }
    if status.max_mtu > status.mtu {
        output.write(b" maxmtu ");
        output.write_number(u64::from(status.max_mtu), 0);
    }
    match status.wake_on_lan {
        Some(true) => output.write(b" wake magic"),
        Some(false) => output.write(b" wake off"),
        None => {}
    }
    output.write(b"\n    ");
    let mac = Shown::<17>::of(status.mac);
    match status.is_loopback() {
        true => {
            output.write(b"link/loopback ");
            output.write(mac.as_bytes());
            output.write(b" brd 00:00:00:00:00:00\n");
        }
        false => {
            output.write(b"link/ether ");
            output.write(mac.as_bytes());
            output.write(b" brd ff:ff:ff:ff:ff:ff\n");
        }
    }
    if statistics {
        let counters = status.counters;
        write_counters(
            output,
            b"    RX:  bytes packets errors dropped\n",
            [
                counters.received_bytes,
                counters.received_packets,
                counters.received_errors,
                counters.received_dropped,
            ],
        );
        write_counters(
            output,
            b"    TX:  bytes packets errors dropped\n",
            [
                counters.sent_bytes,
                counters.sent_packets,
                0,
                counters.sent_dropped,
            ],
        );
    }
}

fn write_counters<S: System>(output: &mut Output<S>, heading: &[u8], values: [u64; 4]) {
    output.write(heading);
    output.write(b"    ");
    for (value, width) in values.into_iter().zip([10, 8, 7, 8]) {
        output.write(b" ");
        output.write_number(value, width - 1);
    }
    output.write(b"\n");
}

fn write_addresses<S: System>(output: &mut Output<S>, status: &InterfaceStatus) {
    let Some(subnet) = status
        .subnet
        .filter(|_| status.link_up || status.is_loopback())
    else {
        return;
    };
    output.write(b"    inet ");
    output.write(Shown::<18>::of(subnet).as_bytes());
    if !status.is_loopback() {
        output.write(b" brd ");
        output.write(Shown::<15>::of(subnet.broadcast()).as_bytes());
    }
    match status.is_loopback() {
        true => output.write(b" scope host "),
        false if status.dhcp == Dhcp::Configured => output.write(b" scope global dynamic "),
        false => output.write(b" scope global "),
    }
    output.write(status.name.as_bytes());
    output.write(b"\n");
    if let Some(left) = status.lease_left_s {
        let lifetime = match left {
            INFINITE_LEASE_S => Shown::<24>::of("forever"),
            seconds => Shown::<24>::of(format_args!("{seconds}sec")),
        };
        output.write(b"       valid_lft ");
        output.write(lifetime.as_bytes());
        output.write(b" preferred_lft ");
        output.write(lifetime.as_bytes());
        output.write(b"\n");
    }
}

fn write_routes<S: System>(output: &mut Output<S>, status: &InterfaceStatus) {
    let Some(subnet) = status
        .subnet
        .filter(|_| !status.is_loopback() && status.link_up)
    else {
        return;
    };
    let source = Shown::<15>::of(subnet.address);
    if let Some(router) = status.router {
        output.write(b"default via ");
        output.write(Shown::<15>::of(router).as_bytes());
        output.write(b" dev ");
        output.write(status.name.as_bytes());
        output.write(b" proto dhcp src ");
        output.write(source.as_bytes());
        output.write(b"\n");
    }
    output
        .write(Shown::<18>::of(format_args!("{}/{}", subnet.network(), subnet.prefix)).as_bytes());
    output.write(b" dev ");
    output.write(status.name.as_bytes());
    output.write(b" proto kernel scope link src ");
    output.write(source.as_bytes());
    output.write(b"\n");
}

fn write_neighbours<S: System>(output: &mut Output<S>, device: Option<&[u8]>) -> i32 {
    let text = match read_text(&mut output.system, NEIGHBOURS) {
        Ok(text) => text,
        Err(errno) => {
            output.file_error(NEIGHBOURS, errno);
            return FAILURE;
        }
    };
    for line in text.as_bytes().split(|&byte| byte == b'\n') {
        let Some(neighbour) = parse_neighbour(line) else {
            continue;
        };
        if device.is_some_and(|name| neighbour.interface.as_bytes() != name) {
            continue;
        }
        output.write(Shown::<15>::of(neighbour.address).as_bytes());
        output.write(b" dev ");
        output.write(neighbour.interface.as_bytes());
        output.write(b" lladdr ");
        output.write(Shown::<17>::of(neighbour.mac).as_bytes());
        output.write(b" REACHABLE\n");
    }
    0
}

fn write_json_string<S: System>(output: &mut Output<S>, text: &[u8]) {
    output.write(b"\"");
    for &byte in text {
        match byte {
            b'"' => output.write(b"\\\""),
            b'\\' => output.write(b"\\\\"),
            byte => output.write(&[byte]),
        }
    }
    output.write(b"\"");
}

fn write_json_optional<S: System>(output: &mut Output<S>, value: Option<impl core::fmt::Display>) {
    match value {
        Some(value) => write_json_string(output, Shown::<32>::of(value).as_bytes()),
        None => output.write(b"null"),
    }
}

fn write_json_number<S: System>(output: &mut Output<S>, value: Option<u64>) {
    match value {
        Some(value) => output.write_number(value, 0),
        None => output.write(b"null"),
    }
}

fn write_json<S: System>(output: &mut Output<S>, read: &Interfaces) -> i32 {
    let ethernet = interfaces(read).find(|status| !status.is_loopback());
    output.write(b"{\"state\":");
    let state = ethernet.map_or(NO_ADAPTER_WORD, |status| status.state().word());
    write_json_string(output, state.as_bytes());
    output.write(b",\"adapters\":[");
    for (index, status) in interfaces(read).enumerate() {
        if index > 0 {
            output.write(b",");
        }
        write_json_adapter(output, status);
    }
    output.write(b"]}\n");
    0
}

fn write_json_adapter<S: System>(output: &mut Output<S>, status: &InterfaceStatus) {
    let field = |output: &mut Output<S>, name: &[u8]| {
        output.write(b",\"");
        output.write(name);
        output.write(b"\":");
    };
    output.write(b"{\"name\":");
    write_json_string(output, status.name.as_bytes());
    field(output, b"state");
    write_json_string(output, status.state().word().as_bytes());
    field(output, b"link");
    let link: &[u8] = if status.link_up { b"up" } else { b"down" };
    write_json_string(output, link);
    field(output, b"dhcp");
    write_json_string(output, status.dhcp.word().as_bytes());
    field(output, b"mac");
    write_json_string(output, Shown::<17>::of(status.mac).as_bytes());
    field(output, b"mtu");
    write_json_number(output, Some(u64::from(status.mtu)));
    field(output, b"address");
    write_json_optional(output, status.subnet.map(|subnet| subnet.address));
    field(output, b"prefix");
    write_json_number(output, status.subnet.map(|subnet| u64::from(subnet.prefix)));
    field(output, b"mask");
    write_json_optional(output, status.subnet.map(|subnet| subnet.mask()));
    field(output, b"router");
    write_json_optional(output, status.router);
    field(output, b"dns");
    output.write(b"[");
    for (index, server) in status.dns_servers().enumerate() {
        if index > 0 {
            output.write(b",");
        }
        write_json_string(output, Shown::<15>::of(server).as_bytes());
    }
    output.write(b"]");
    field(output, b"dhcp_server");
    write_json_optional(output, status.dhcp_server);
    let lease = |seconds: Option<u32>| seconds.filter(|&seconds| seconds != INFINITE_LEASE_S);
    field(output, b"lease_left_s");
    write_json_number(output, lease(status.lease_left_s).map(u64::from));
    field(output, b"lease_s");
    write_json_number(output, lease(status.lease_s).map(u64::from));
    field(output, b"lease_ends");
    let ends: &[u8] = match status.lease_s {
        Some(INFINITE_LEASE_S) => b"false",
        Some(_) => b"true",
        None => b"null",
    };
    output.write(ends);
    let counters = status.counters;
    for (name, value) in [
        (&b"rx_bytes"[..], counters.received_bytes),
        (b"rx_packets", counters.received_packets),
        (b"rx_errors", counters.received_errors),
        (b"rx_dropped", counters.received_dropped),
        (b"tx_bytes", counters.sent_bytes),
        (b"tx_packets", counters.sent_packets),
        (b"tx_dropped", counters.sent_dropped),
    ] {
        field(output, name);
        write_json_number(output, Some(value));
    }
    if status.max_mtu > status.mtu {
        field(output, b"max_mtu");
        write_json_number(output, Some(u64::from(status.max_mtu)));
    }
    if let Some(on) = status.wake_on_lan {
        field(output, b"wake_on_lan");
        output.write(if on { b"\"magic\"" } else { b"\"off\"" });
    }
    output.write(b"}");
}

#[cfg(test)]
mod tests;
