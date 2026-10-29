use crate::network::{DNS_TIMEOUT_MS, LookupError, Shown, lookup, nameservers, parse_seconds_ms};
use crate::options::parse_count;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_net::Ipv4Address;
use eggos_net::dns::{self, ResponseError};

const USAGE: &[u8] = b"usage: nslookup [-port=<port>] [-timeout=<seconds>] <name> [<server>]";

pub fn nslookup<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut port = dns::PORT;
    let mut timeout_ms = DNS_TIMEOUT_MS;
    let mut operands: [&[u8]; 2] = [b"", b""];
    let mut count = 0;
    for argument in arguments.iter().map(|argument| argument.to_bytes()) {
        if let Some(value) = argument.strip_prefix(b"-port=") {
            let given = parse_count(value)
                .and_then(|port| u16::try_from(port).ok())
                .filter(|&port| port > 0);
            match given {
                Some(given) => port = given,
                None => {
                    output.error(&[b"invalid port '", value, b"'"]);
                    return FAILURE;
                }
            }
            continue;
        }
        if let Some(value) = argument.strip_prefix(b"-timeout=") {
            let given = parse_seconds_ms(value)
                .and_then(|ms| u32::try_from(ms).ok())
                .filter(|&ms| ms > 0);
            match given {
                Some(given) => timeout_ms = given,
                None => {
                    output.error(&[b"invalid timeout '", value, b"'"]);
                    return FAILURE;
                }
            }
            continue;
        }
        match argument {
            b"--" => {}
            option if option.starts_with(b"-") && option.len() > 1 => {
                return output.invalid_option(option);
            }
            operand if count < operands.len() => {
                operands[count] = operand;
                count += 1;
            }
            _ => {
                output.error(&[USAGE]);
                return FAILURE;
            }
        }
    }
    let (name, server) = match operands[..count] {
        [name] => (name, None),
        [name, server] => (name, Some(server)),
        _ => {
            output.error(&[USAGE]);
            return FAILURE;
        }
    };
    if Ipv4Address::parse(name).is_some() {
        output.error(&[name, b": looking up the name of an address is not supported yet"]);
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
            let mut shown_name = name;
            if let Some(canonical) = answers.canonical.as_ref() {
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
