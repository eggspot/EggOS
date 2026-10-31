use crate::network::{Shown, milliseconds, parse_seconds_ms, resolve};
use crate::options::{self, Refused, parse_count};
use crate::output::{FAILURE, Output};
use crate::system::{DatagramKind, Duration, System};
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_net::icmp;

pub const DEFAULT_COUNT: u64 = 4;
pub const DEFAULT_DATA_BYTES: usize = 56;
pub const MAX_DATA_BYTES: usize = 1472 - icmp::HEADER_BYTES;
pub const DEFAULT_INTERVAL_MS: u64 = 1000;
pub const MIN_INTERVAL_MS: u64 = 200;
pub const DEFAULT_TIMEOUT_MS: u64 = 1000;
const TROUBLE: i32 = 2;
const PATTERN_START: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Settings {
    count: u64,
    interval_ms: u64,
    timeout_ms: u64,
    data_bytes: usize,
    quiet: bool,
}

pub fn ping<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut settings = Settings {
        count: DEFAULT_COUNT,
        interval_ms: DEFAULT_INTERVAL_MS,
        timeout_ms: DEFAULT_TIMEOUT_MS,
        data_bytes: DEFAULT_DATA_BYTES,
        quiet: false,
    };
    let parsed = options::parse_defined(output, arguments, &commands::PING, |output, given| {
        let bad = |output: &mut Output<S>, what: &[u8], value: &[u8]| {
            output.coded(&errors::USAGE, &[b"invalid ", what, b": '", value, b"'"]);
            Err(Refused::Reported)
        };
        match (given.option.short, given.value) {
            (Some(b'q'), _) => settings.quiet = true,
            (Some(b'c'), Some(value)) => match parse_count(value).filter(|&count| count > 0) {
                Some(count) => settings.count = count,
                None => return bad(output, b"count", value),
            },
            (Some(b'i'), Some(value)) => {
                match parse_seconds_ms(value).filter(|&ms| ms >= MIN_INTERVAL_MS) {
                    Some(ms) => settings.interval_ms = ms,
                    None => return bad(output, b"interval (at least 0.2 seconds)", value),
                }
            }
            (Some(b'W'), Some(value)) => match parse_seconds_ms(value).filter(|&ms| ms > 0) {
                Some(ms) => settings.timeout_ms = ms,
                None => return bad(output, b"timeout", value),
            },
            (Some(b's'), Some(value)) => {
                match parse_count(value).filter(|&bytes| bytes as usize <= MAX_DATA_BYTES) {
                    Some(bytes) => settings.data_bytes = bytes as usize,
                    None => return bad(output, b"packet size", value),
                }
            }
            _ => {}
        }
        Ok(())
    });
    let operands = match parsed {
        Ok(operands) => operands,
        Err(Refused::Reported) => return TROUBLE,
    };
    let [host] = operands.all() else {
        output.coded(
            &errors::USAGE,
            &[b"usage: ping [-c count] [-i interval] [-W timeout] [-s size] [-q] host"],
        );
        return TROUBLE;
    };
    let host = host.to_bytes();
    let address = match resolve(output, host) {
        Ok(address) => address,
        Err(_) => return TROUBLE,
    };
    let descriptor = match output.system.open_datagram_socket(DatagramKind::Ping) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.error(&[b"socket: ", errno.message()]);
            return TROUBLE;
        }
    };
    let status = run(output, descriptor, host, address, settings);
    output.system.close(descriptor);
    status
}

#[derive(Debug, Default)]
struct Tally {
    sent: u64,
    received: u64,
    errors: u64,
    fastest_us: u64,
    slowest_us: u64,
    total_us: u64,
}

fn run<S: System>(
    output: &mut Output<S>,
    descriptor: usize,
    host: &[u8],
    address: eggos_net::Ipv4Address,
    settings: Settings,
) -> i32 {
    let shown = Shown::<15>::of(address);
    output.write(b"PING ");
    output.write(host);
    output.write(b" (");
    output.write(shown.as_bytes());
    output.write(b") ");
    output.write_number(settings.data_bytes as u64, 0);
    output.write(b"(");
    output.write_number((settings.data_bytes + icmp::HEADER_BYTES + 20) as u64, 0);
    output.write(b") bytes of data.\n");
    output.flush();
    let mut request = [0u8; icmp::HEADER_BYTES + MAX_DATA_BYTES];
    request[0] = icmp::kind::ECHO_REQUEST;
    for (index, byte) in request[icmp::HEADER_BYTES..].iter_mut().enumerate() {
        *byte = PATTERN_START.wrapping_add(index as u8);
    }
    let request_bytes = icmp::HEADER_BYTES + settings.data_bytes;
    let mut tally = Tally::default();
    let started = output.system.monotonic_us();
    for sequence in 1..=settings.count {
        let sequence = sequence as u16;
        request[6..8].copy_from_slice(&sequence.to_be_bytes());
        let sent_at = output.system.monotonic_us();
        tally.sent += 1;
        if let Err(errno) = output
            .system
            .send_to(descriptor, &request[..request_bytes], address, 0)
        {
            tally.errors += 1;
            output.error(&[b"sending to ", shown.as_bytes(), b": ", errno.message()]);
        } else if let Some((bytes, round_trip_us)) =
            wait_for_reply(output, descriptor, sequence, sent_at, settings.timeout_ms)
        {
            tally.received += 1;
            tally.total_us += round_trip_us;
            tally.fastest_us = match tally.received {
                1 => round_trip_us,
                _ => tally.fastest_us.min(round_trip_us),
            };
            tally.slowest_us = tally.slowest_us.max(round_trip_us);
            if !settings.quiet {
                output.write_number(bytes as u64, 0);
                output.write(b" bytes from ");
                output.write(shown.as_bytes());
                output.write(b": icmp_seq=");
                output.write_number(u64::from(sequence), 0);
                output.write(b" time=");
                output.write(milliseconds(round_trip_us).as_bytes());
                output.write(b" ms\n");
                output.flush();
            }
        } else if !settings.quiet {
            output.write(b"no answer from ");
            output.write(shown.as_bytes());
            output.write(b": icmp_seq=");
            output.write_number(u64::from(sequence), 0);
            output.write(b"\n");
            output.flush();
        }
        if u64::from(sequence) < settings.count {
            let elapsed_ms = output.system.monotonic_us().saturating_sub(sent_at) / 1000;
            let rest_ms = settings.interval_ms.saturating_sub(elapsed_ms);
            if rest_ms > 0 {
                let _ = output.system.sleep(Duration {
                    seconds: rest_ms / 1000,
                    nanoseconds: (rest_ms % 1000) as u32 * 1_000_000,
                });
            }
        }
    }
    let elapsed_ms = output.system.monotonic_us().saturating_sub(started) / 1000;
    write_summary(output, host, &tally, elapsed_ms);
    match tally.received {
        0 => FAILURE,
        _ => 0,
    }
}

fn wait_for_reply<S: System>(
    output: &mut Output<S>,
    descriptor: usize,
    sequence: u16,
    sent_at: u64,
    timeout_ms: u64,
) -> Option<(usize, u64)> {
    let mut reply = [0u8; icmp::HEADER_BYTES + MAX_DATA_BYTES];
    loop {
        let waited_ms = output.system.monotonic_us().saturating_sub(sent_at) / 1000;
        let left = timeout_ms.checked_sub(waited_ms).filter(|&left| left > 0)?;
        let received = output
            .system
            .receive_from(
                descriptor,
                &mut reply,
                u32::try_from(left).unwrap_or(u32::MAX),
            )
            .ok()??;
        let arrived = output.system.monotonic_us();
        let Some(echo) = icmp::parse_echo(&reply[..received.0]) else {
            continue;
        };
        if echo.kind == icmp::kind::ECHO_REPLY && echo.sequence == sequence {
            return Some((received.0, arrived.saturating_sub(sent_at)));
        }
    }
}

fn write_summary<S: System>(output: &mut Output<S>, host: &[u8], tally: &Tally, elapsed_ms: u64) {
    output.write(b"\n--- ");
    output.write(host);
    output.write(b" ping statistics ---\n");
    output.write_number(tally.sent, 0);
    output.write(b" packets transmitted, ");
    output.write_number(tally.received, 0);
    output.write(b" received, ");
    if tally.errors > 0 {
        output.write(b"+");
        output.write_number(tally.errors, 0);
        output.write(b" errors, ");
    }
    let lost = tally.sent - tally.received;
    output.write_number(lost * 100 / tally.sent.max(1), 0);
    output.write(b"% packet loss, time ");
    output.write_number(elapsed_ms, 0);
    output.write(b"ms\n");
    if let Some(average_us) = tally.total_us.checked_div(tally.received) {
        output.write(b"rtt min/avg/max = ");
        output.write(milliseconds(tally.fastest_us).as_bytes());
        output.write(b"/");
        output.write(milliseconds(average_us).as_bytes());
        output.write(b"/");
        output.write(milliseconds(tally.slowest_us).as_bytes());
        output.write(b" ms\n");
    }
}

#[cfg(test)]
mod tests;
