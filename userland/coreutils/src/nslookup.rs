use crate::network::{DNS_TIMEOUT_MS, LookupError, Shown, lookup, nameservers, parse_seconds_ms};
use crate::options::{Refused, parse_count, parse_defined};
use crate::output::{FAILURE, Output};
use crate::system::System;
use crate::utilities::MAX_ARGUMENTS;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_net::Ipv4Address;
use eggos_net::dns::{self, ResponseError};

const USAGE: &[u8] = b"usage: nslookup [-port=<port>] [-timeout=<seconds>] <name> [<server>]";

fn bind_style(argument: &CStr) -> Option<(&'static CStr, &CStr)> {
    let bytes = argument.to_bytes();
    let (long, prefix): (&'static CStr, &[u8]) = if bytes.starts_with(b"-port=") {
        (c"--port", b"-port=")
    } else if bytes.starts_with(b"-timeout=") {
        (c"--timeout", b"-timeout=")
    } else {
        return None;
    };
    let value = CStr::from_bytes_with_nul(&argument.to_bytes_with_nul()[prefix.len()..]).ok()?;
    Some((long, value))
}

pub fn nslookup<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut words = [c""; MAX_ARGUMENTS];
    let mut count = 0;
    let mut options_end = false;
    for &argument in arguments {
        let (first, second) = match bind_style(argument).filter(|_| !options_end) {
            Some((long, value)) => (long, Some(value)),
            None => (argument, None),
        };
        options_end |= argument.to_bytes() == b"--";
        for word in [Some(first), second].into_iter().flatten() {
            let Some(slot) = words.get_mut(count) else {
                output.coded(&errors::USAGE, &[USAGE]);
                return FAILURE;
            };
            *slot = word;
            count += 1;
        }
    }
    let mut port = dns::PORT;
    let mut timeout_ms = DNS_TIMEOUT_MS;
    let parsed = parse_defined(
        output,
        &words[..count],
        &commands::NSLOOKUP,
        |output, given| {
            let value = given.value.unwrap_or_default();
            if given.is_long("port") {
                let given = parse_count(value)
                    .and_then(|port| u16::try_from(port).ok())
                    .filter(|&port| port > 0);
                let Some(given) = given else {
                    output.coded(&errors::USAGE, &[b"invalid port '", value, b"'"]);
                    return Err(Refused::Reported);
                };
                port = given;
            } else if given.is_long("timeout") {
                let given = parse_seconds_ms(value)
                    .and_then(|ms| u32::try_from(ms).ok())
                    .filter(|&ms| ms > 0);
                let Some(given) = given else {
                    output.coded(&errors::USAGE, &[b"invalid timeout '", value, b"'"]);
                    return Err(Refused::Reported);
                };
                timeout_ms = given;
            }
            Ok(())
        },
    );
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    let (name, server) = match operands.all() {
        &[name] => (name.to_bytes(), None),
        &[name, server] => (name.to_bytes(), Some(server.to_bytes())),
        _ => {
            output.coded(&errors::USAGE, &[USAGE]);
            return FAILURE;
        }
    };
    if Ipv4Address::parse(name).is_some() {
        output.error(&[
            name,
            b": looking up the name of an address is not supported yet",
        ]);
        return FAILURE;
    }
    let server = match server {
        Some(server) => match Ipv4Address::parse(server) {
            Some(address) => address,
            None => {
                output.error(&[b"the server must be an IPv4 address: '", server, b"'"]);
                return FAILURE;
            }
        },
        None => match nameservers(&mut output.system).into_iter().flatten().next() {
            Some(address) => address,
            None => {
                output.error(&[b"no DNS server: /etc/resolv.conf names none"]);
                return FAILURE;
            }
        },
    };
    let shown = Shown::<15>::of(server);
    output.write(b"Server:\t\t");
    output.write(shown.as_bytes());
    output.write(b"\nAddress:\t");
    output.write(shown.as_bytes());
    output.write(b"#");
    output.write_number(u64::from(port), 0);
    output.write(b"\n\n");
    let name = name.strip_suffix(b".").unwrap_or(name);
    match lookup(&mut output.system, name, server, port, timeout_ms) {
        Ok(answers) if answers.addresses().is_empty() => {
            output.write(b"*** Can't find ");
            output.write(name);
            output.write(b": No answer\n");
            FAILURE
        }
        Ok(answers) => {
            output.write(b"Non-authoritative answer:\n");
            let canonical = answers
                .canonical
                .as_ref()
                .map(|canonical| Shown::<256>::printable(canonical.as_bytes()));
            let mut shown_name = name;
            if let Some(canonical) = canonical.as_ref() {
                output.write(name);
                output.write(b"\tcanonical name = ");
                output.write(canonical.as_bytes());
                output.write(b".\n");
                shown_name = canonical.as_bytes();
            }
            for address in answers.addresses() {
                output.write(b"Name:\t");
                output.write(shown_name);
                output.write(b"\nAddress: ");
                output.write(Shown::<15>::of(address).as_bytes());
                output.write(b"\n");
            }
            0
        }
        Err(LookupError::Answer(error)) => {
            let code: &[u8] = match error {
                ResponseError::NameNotFound => b"NXDOMAIN",
                ResponseError::ServerFailure => b"SERVFAIL",
                ResponseError::Refused => b"REFUSED",
                _ => error.description().as_bytes(),
            };
            output.write(b"** server can't find ");
            output.write(name);
            output.write(b": ");
            output.write(code);
            output.write(b"\n");
            FAILURE
        }
        Err(LookupError::TimedOut) => {
            output.write(b";; connection timed out; no servers could be reached\n");
            FAILURE
        }
        Err(error) => {
            output.error(&[name, b": ", error.message()]);
            FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
