use crate::date::{ask_time, time_server};
use crate::fetch::{RootsFile, tls_failure};
use crate::network::{INTERFACES, MAX_INTERFACES, Shown, read_text, resolve};
use crate::output::{FAILURE, Output};
use crate::system::{DatagramKind, Duration, System};
use core::cell::RefCell;
use core::ffi::CStr;
use eggos_calendar::{MONTHS, days_from_civil, days_in_month, rfc3339};
use eggos_net::ethernet::MacAddress;
use eggos_net::status::parse_interfaces;
use eggos_net::{Ipv4Address, http, sntp};
use eggos_runtime::errno::Errno;
use eggos_settings_file::clock_sync::{
    BUDGET_LARGEST, BUDGET_TEMPORARY_PREFIX, LARGEST, Outcome, Record, TEMPORARY_PREFIX,
    parse_budget, write_budget,
};
use eggos_tls::chain::TrustAnchors;
use eggos_tls::client::{self, Buffers, RANDOMNESS_BYTES, Session, Settings, Stopped, Transport};

pub const STEP_LIMIT_MS: u64 = 3_600_000;
pub const UNPROVEN_BUDGET_MS: u64 = 3_600_000;
pub const IGNORE_BELOW_MS: u64 = 1_000;
pub const AGREE_WINDOW_MS: i64 = 120_000;
pub const PROOF_HOSTS: [&[u8]; 3] = [b"www.google.com", b"www.cloudflare.com", b"www.apple.com"];
pub const PROOF_QUORUM: usize = 2;
pub const PROOF_HOST_TIMEOUT_MS: u32 = 10_000;
pub const FLOOR: i64 = 1_767_225_600;
pub const RECORD: &CStr = c"/run/eggos/clock-sync";
pub const ROOTS_REMEDY: &[u8] = b"install roots at /etc/ssl/roots.pem or run date --sync";
const RECORD_FOLDER: &CStr = c"/run/eggos";
pub const BUDGET: &CStr = c"/var/lib/eggos/clock-budget";
const BUDGET_FOLDERS: [&CStr; 2] = [c"/var/lib", c"/var/lib/eggos"];
const TEMPORARY_NAME_BYTES: usize = 64;
const PROOF_HEAD_BYTES: usize = 8192;
const REQUEST_BYTES: usize = 384;
const USER_AGENT: &[u8] = b"EggOS-date";
const NANOSECONDS: i64 = 1_000_000_000;
const NANOSECONDS_PER_MS: i64 = 1_000_000;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asker {
    Person,
    Automatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Testimony {
    Dated {
        date: i64,
        at_us: u64,
        valid_from: i64,
        valid_until: i64,
    },
    Unreachable,
}

pub trait Witness<S: System> {
    fn testify(&mut self, output: &mut Output<S>, host: &[u8], at: i64) -> Testimony;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    AlreadyRight,
    Implausible,
    Step { unproven_ms: u64 },
    NeedsProof,
    Unproven,
}

fn judge(
    asker: Asker,
    offset_ms: i64,
    server_time: i64,
    now: i64,
    unproven_ms: u64,
    has_roots: bool,
) -> Verdict {
    if server_time < FLOOR {
        return Verdict::Implausible;
    }
    let size_ms = offset_ms.unsigned_abs();
    if size_ms < IGNORE_BELOW_MS {
        return Verdict::AlreadyRight;
    }
    if asker == Asker::Person {
        return Verdict::Step { unproven_ms };
    }
    let unset = now < FLOOR;
    let spent = unproven_ms.saturating_add(size_ms);
    match (unset, has_roots) {
        (false, _) if size_ms <= STEP_LIMIT_MS && spent <= UNPROVEN_BUDGET_MS => {
            Verdict::Step { unproven_ms: spent }
        }
        (true, false) => Verdict::Step { unproven_ms },
        (false, false) => Verdict::Unproven,
        (_, true) => Verdict::NeedsProof,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agreement {
    Agrees,
    Disagrees { https_minus_server_ms: i64 },
    Silent,
}

fn agreement(testimony: Testimony, server_time_ns: i64, reply_us: u64) -> Agreement {
    let Testimony::Dated {
        date,
        at_us,
        valid_from,
        valid_until,
    } = testimony
    else {
        return Agreement::Silent;
    };
    let elapsed_ns = (at_us.saturating_sub(reply_us) as i64).saturating_mul(1000);
    let expected_ns = server_time_ns.saturating_add(elapsed_ns);
    let difference_ns = date.saturating_mul(NANOSECONDS).saturating_sub(expected_ns);
    let within = difference_ns.unsigned_abs() <= AGREE_WINDOW_MS.unsigned_abs() * 1_000_000;
    match within && (valid_from..=valid_until).contains(&date) {
        true => Agreement::Agrees,
        false => Agreement::Disagrees {
            https_minus_server_ms: difference_ns / NANOSECONDS_PER_MS,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proof {
    Proven([usize; PROOF_QUORUM]),
    Unreachable,
    Disagrees { https_offset_ms: i64 },
}

fn prove<S: System, W: Witness<S>>(
    output: &mut Output<S>,
    witness: &mut W,
    server_time_ns: i64,
    reply_us: u64,
    offset_ms: i64,
) -> Proof {
    let mut passed = [0; PROOF_QUORUM];
    let mut count = 0;
    let mut disagreement = None;
    for (index, host) in PROOF_HOSTS.iter().enumerate() {
        let testimony = witness.testify(output, host, server_time_ns.div_euclid(NANOSECONDS));
        match agreement(testimony, server_time_ns, reply_us) {
            Agreement::Agrees => {
                passed[count] = index;
                count += 1;
                if count == PROOF_QUORUM {
                    return Proof::Proven(passed);
                }
            }
            Agreement::Disagrees {
                https_minus_server_ms,
            } => {
                disagreement.get_or_insert(offset_ms.saturating_add(https_minus_server_ms));
            }
            Agreement::Silent => {}
        }
    }
    match disagreement {
        Some(https_offset_ms) => Proof::Disagrees { https_offset_ms },
        None => Proof::Unreachable,
    }
}

struct Synced {
    outcome: Outcome,
    seconds: i64,
    moved_ms: i64,
    unproven_ms: u64,
}

struct Failed {
    outcome: Outcome,
    offset_ms: i64,
}

impl Failed {
    const fn of(outcome: Outcome) -> Self {
        Self {
            outcome,
            offset_ms: 0,
        }
    }
}

pub fn sync<S: System, W: Witness<S>>(
    output: &mut Output<S>,
    server: &[u8],
    asker: Asker,
    witness: &mut W,
) -> Result<i64, i32> {
    let Some((host, port)) = time_server(server) else {
        output.error(&[
            b"invalid time server '",
            server,
            b"': give host or host:port",
        ]);
        return Err(FAILURE);
    };
    let started = output.system.now() as i64;
    let unproven_ms = unproven_budget(&mut output.system);
    let mut record = Record::new(Outcome::Syncing, started, server);
    record.unproven_ms = unproven_ms;
    write_record(&mut output.system, &record);
    let synced = synchronize(output, host, port, server, asker, unproven_ms, witness);
    record.finished = Some(output.system.now() as i64);
    let result = match synced {
        Ok(synced) => {
            record.outcome = synced.outcome;
            record.moved_ms = synced.moved_ms;
            record.unproven_ms = synced.unproven_ms;
            if synced.unproven_ms != unproven_ms {
                keep_budget_or_say(output, synced.unproven_ms);
            }
            Ok(synced.seconds)
        }
        Err(failed) => {
            record.outcome = failed.outcome;
            record.moved_ms = failed.offset_ms;
            record.set_reason(output.last_error());
            Err(FAILURE)
        }
    };
    write_record(&mut output.system, &record);
    result
}

fn synchronize<S: System, W: Witness<S>>(
    output: &mut Output<S>,
    host: &[u8],
    port: u16,
    server: &[u8],
    asker: Asker,
    unproven_ms: u64,
    witness: &mut W,
) -> Result<Synced, Failed> {
    if has_no_address(&mut output.system) {
        output.error(&[server, b": the network adapter has no address"]);
        return Err(Failed::of(Outcome::NoAddress));
    }
    let no_answer = |_| Failed::of(Outcome::NoAnswer);
    let address: Ipv4Address = resolve(output, host).map_err(no_answer)?;
    let socket = match output.system.open_datagram_socket(DatagramKind::Udp) {
        Ok(socket) => socket,
        Err(errno) => {
            output.error(&[b"socket: ", errno.message()]);
            return Err(Failed::of(Outcome::NoAnswer));
        }
    };
    let answer = ask_time(output, socket, address, port, server);
    output.system.close(socket);
    let answer = answer.map_err(no_answer)?;
    let reply_us = answer.arrived_us;
    let server_time_ns = sntp::arrival_time(&answer.reply, answer.round_trip_ns);
    let local_at_reply_ns = local_ns(&mut output.system) - elapsed_ns(output, reply_us);
    let offset_ms = (server_time_ns - local_at_reply_ns) / NANOSECONDS_PER_MS;
    let roots = asker == Asker::Automatic && has_roots(output);
    let verdict = judge(
        asker,
        offset_ms,
        server_time_ns.div_euclid(NANOSECONDS),
        local_at_reply_ns.div_euclid(NANOSECONDS),
        unproven_ms,
        roots,
    );
    let stratum = Shown::<4>::of(answer.reply.stratum);
    let synced = match verdict {
        Verdict::Implausible => {
            let shown = rfc3339(server_time_ns.div_euclid(NANOSECONDS)).unwrap_or([b'?'; 20]);
            output.error(&[
                server,
                b": implausible: the time server says ",
                &shown,
                b", before 2026: not used",
            ]);
            return Err(Failed::of(Outcome::NoAnswer));
        }
        Verdict::AlreadyRight => {
            output.write(b"Synchronized with ");
            output.write(server);
            output.write(b" (stratum ");
            output.write(stratum.as_bytes());
            output.write(b"): the clock was already right (off by ");
            write_seconds(output, offset_ms);
            output.write(b")\n");
            return Ok(Synced {
                outcome: Outcome::Set,
                seconds: output.system.now() as i64,
                moved_ms: 0,
                unproven_ms,
            });
        }
        Verdict::Step { unproven_ms } => {
            step_clock(output, server_time_ns, reply_us, Outcome::Set, unproven_ms)?
        }
        Verdict::Unproven => {
            output.error(&[
                server,
                b": unproven: the clock is off by ",
                seconds_text(offset_ms).as_bytes(),
                b", more than the automatic clock moves without an HTTPS check, and there are \
                  no roots to check with: ",
                ROOTS_REMEDY,
            ]);
            return Err(Failed {
                outcome: Outcome::Unconfirmed,
                offset_ms,
            });
        }
        Verdict::NeedsProof => match prove(output, witness, server_time_ns, reply_us, offset_ms) {
            Proof::Proven(hosts) => {
                let synced = step_clock(output, server_time_ns, reply_us, Outcome::Confirmed, 0)?;
                write_moved(output, server, &stratum, synced.moved_ms);
                output.write(b" (confirmed over HTTPS by ");
                for (index, &host) in hosts.iter().enumerate() {
                    if index > 0 {
                        output.write(b" and ");
                    }
                    output.write(PROOF_HOSTS[host]);
                }
                output.write(b")\n");
                return Ok(synced);
            }
            Proof::Unreachable => {
                output.error(&[
                    server,
                    b": proof unreachable: the clock is off by ",
                    seconds_text(offset_ms).as_bytes(),
                    b", and fewer than 2 of www.google.com, www.cloudflare.com and \
                      www.apple.com confirmed it over HTTPS: not applied",
                ]);
                return Err(Failed {
                    outcome: Outcome::Unconfirmed,
                    offset_ms,
                });
            }
            Proof::Disagrees { https_offset_ms } => {
                output.error(&[
                    server,
                    b": proof disagrees: the time server says the clock is off by ",
                    seconds_text(offset_ms).as_bytes(),
                    b", HTTPS says by ",
                    seconds_text(https_offset_ms).as_bytes(),
                    b": not applied",
                ]);
                return Err(Failed {
                    outcome: Outcome::Unconfirmed,
                    offset_ms,
                });
            }
        },
    };
    write_moved(output, server, &stratum, synced.moved_ms);
    if asker == Asker::Person && offset_ms.unsigned_abs() > STEP_LIMIT_MS {
        output.write(b" (not verified over HTTPS)");
    }
    output.write(b"\n");
    Ok(synced)
}

fn local_ns<S: System>(system: &mut S) -> i64 {
    let now = system.now_with_nanoseconds();
    now.seconds as i64 * NANOSECONDS + i64::from(now.nanoseconds)
}

fn elapsed_ns<S: System>(output: &mut Output<S>, since_us: u64) -> i64 {
    (output.system.monotonic_us().saturating_sub(since_us) as i64).saturating_mul(1000)
}

fn step_clock<S: System>(
    output: &mut Output<S>,
    server_time_ns: i64,
    reply_us: u64,
    outcome: Outcome,
    unproven_ms: u64,
) -> Result<Synced, Failed> {
    let time_ns = server_time_ns + elapsed_ns(output, reply_us);
    let before_ns = local_ns(&mut output.system);
    let since_epoch = Duration {
        seconds: time_ns.div_euclid(NANOSECONDS).max(0) as u64,
        nanoseconds: time_ns.rem_euclid(NANOSECONDS) as u32,
    };
    if let Err(errno) = output.system.set_time(since_epoch) {
        output.error(&[b"cannot set date: ", errno.message()]);
        let outcome = match errno {
            Errno::EPERM | Errno::EACCES => Outcome::NotAllowed,
            _ => Outcome::NoAnswer,
        };
        return Err(Failed::of(outcome));
    }
    Ok(Synced {
        outcome,
        seconds: since_epoch.seconds as i64,
        moved_ms: (time_ns - before_ns) / NANOSECONDS_PER_MS,
        unproven_ms,
    })
}

fn write_moved<S: System>(
    output: &mut Output<S>,
    server: &[u8],
    stratum: &Shown<4>,
    moved_ms: i64,
) {
    output.write(b"Synchronized with ");
    output.write(server);
    output.write(b" (stratum ");
    output.write(stratum.as_bytes());
    output.write(b"): the clock moved by ");
    write_seconds(output, moved_ms);
}

fn write_seconds<S: System>(output: &mut Output<S>, milliseconds: i64) {
    output.write(seconds_text(milliseconds).as_bytes());
}

fn seconds_text(milliseconds: i64) -> Shown<32> {
    let size = milliseconds.unsigned_abs();
    Shown::of(format_args!(
        "{}{}.{:03} s",
        if milliseconds < 0 { "-" } else { "+" },
        size / 1000,
        size % 1000
    ))
}

fn has_no_address<S: System>(system: &mut S) -> bool {
    let Ok(text) = read_text(system, INTERFACES) else {
        return false;
    };
    let (list, count) = parse_interfaces::<MAX_INTERFACES>(text.as_bytes());
    !list[..count]
        .iter()
        .flatten()
        .any(|status| status.mac != MacAddress::ZERO && status.subnet.is_some())
}

fn has_roots<S: System>(output: &mut Output<S>) -> bool {
    let cell = RefCell::new(output);
    let mut roots = RootsFile { output: &cell };
    matches!(roots.each(&mut |_| true), Ok(true))
}

fn read_record<S: System>(system: &mut S) -> Option<Record> {
    let text = read_text(system, RECORD).ok()?;
    Record::parse(text.as_bytes())
}

fn write_record<S: System>(system: &mut S, record: &Record) {
    let mut file = [0u8; LARGEST];
    let Some(length) = record.write(&mut file) else {
        return;
    };
    let _ = system.make_directory(RECORD_FOLDER, 0o755);
    let _ = replace_file(system, TEMPORARY_PREFIX, RECORD, &file[..length]);
}

fn kept_budget<S: System>(system: &mut S) -> Option<u64> {
    match read_text(system, BUDGET) {
        Err(Errno::ENOENT) => None,
        Err(_) => Some(UNPROVEN_BUDGET_MS),
        Ok(text) => Some(parse_budget(text.as_bytes()).unwrap_or(UNPROVEN_BUDGET_MS)),
    }
}

fn unproven_budget<S: System>(system: &mut S) -> u64 {
    let this_boot = read_record(system).map_or(0, |record| record.unproven_ms);
    kept_budget(system).unwrap_or(0).max(this_boot)
}

fn keep_budget<S: System>(system: &mut S, unproven_ms: u64) -> Result<(), Errno> {
    let mut file = [0u8; BUDGET_LARGEST];
    let length = write_budget(unproven_ms, &mut file).ok_or(Errno::EINVAL)?;
    for folder in BUDGET_FOLDERS {
        let _ = system.make_directory(folder, 0o755);
    }
    replace_file(system, BUDGET_TEMPORARY_PREFIX, BUDGET, &file[..length])
}

fn keep_budget_or_say<S: System>(output: &mut Output<S>, unproven_ms: u64) {
    if let Err(errno) = keep_budget(&mut output.system, unproven_ms) {
        output.error(&[
            b"cannot keep the unproven clock budget in ",
            BUDGET.to_bytes(),
            b": ",
            errno.message(),
        ]);
    }
}

fn replace_file<S: System>(
    system: &mut S,
    temporary_prefix: &str,
    target: &CStr,
    bytes: &[u8],
) -> Result<(), Errno> {
    let mut random = [0u8; 4];
    let _ = system.random(&mut random);
    let mut name = [0u8; TEMPORARY_NAME_BYTES];
    let prefix = temporary_prefix.as_bytes();
    let Some(room) = name.get_mut(..prefix.len() + 2 * random.len()) else {
        return Err(Errno::ENAMETOOLONG);
    };
    room[..prefix.len()].copy_from_slice(prefix);
    for (index, byte) in random.iter().enumerate() {
        room[prefix.len() + 2 * index] = HEX_DIGITS[usize::from(byte >> 4)];
        room[prefix.len() + 2 * index + 1] = HEX_DIGITS[usize::from(byte & 0xf)];
    }
    let Ok(temporary) = CStr::from_bytes_until_nul(&name) else {
        return Err(Errno::ENAMETOOLONG);
    };
    let descriptor = system.create(temporary, 0o644, true)?;
    let written = system
        .write(descriptor, bytes)
        .and_then(|()| system.sync_file(descriptor));
    system.close(descriptor);
    let replaced = written.and_then(|()| system.rename(temporary, target));
    if replaced.is_err() {
        let _ = system.remove(temporary, false);
    }
    replaced
}

pub fn forget_unproven_steps<S: System>(output: &mut Output<S>) {
    let system = &mut output.system;
    if let Some(mut record) = read_record(system).filter(|record| record.unproven_ms != 0) {
        record.unproven_ms = 0;
        write_record(system, &record);
    }
    if kept_budget(system).is_some_and(|unproven_ms| unproven_ms != 0) {
        keep_budget_or_say(output, 0);
    }
}

pub fn imf_fixdate(text: &[u8]) -> Option<i64> {
    let &[
        _,
        _,
        _,
        b',',
        b' ',
        day_tens,
        day_ones,
        b' ',
        month_0,
        month_1,
        month_2,
        b' ',
        century_tens,
        century_ones,
        year_tens,
        year_ones,
        b' ',
        hour_tens,
        hour_ones,
        b':',
        minute_tens,
        minute_ones,
        b':',
        second_tens,
        second_ones,
        b' ',
        b'G',
        b'M',
        b'T',
    ] = text
    else {
        return None;
    };
    let two = |tens: u8, ones: u8| {
        (tens.is_ascii_digit() && ones.is_ascii_digit())
            .then(|| u32::from(tens - b'0') * 10 + u32::from(ones - b'0'))
    };
    let month = MONTHS
        .iter()
        .position(|name| name.as_bytes()[..3] == [month_0, month_1, month_2])?
        as u32
        + 1;
    let year = i64::from(two(century_tens, century_ones)? * 100 + two(year_tens, year_ones)?);
    let day = two(day_tens, day_ones)?;
    let (hour, minute, second) = (
        two(hour_tens, hour_ones)?,
        two(minute_tens, minute_ones)?,
        two(second_tens, second_ones)?,
    );
    if day == 0 || day > days_in_month(year, month) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let seconds = hour * 3600 + minute * 60 + second;
    Some(days_from_civil(year, month, day) * 86_400 + i64::from(seconds))
}

fn age_seconds(text: &[u8]) -> Option<i64> {
    if text.is_empty() || text.len() > 9 || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        text.iter()
            .fold(0i64, |value, digit| value * 10 + i64::from(digit - b'0')),
    )
}

pub struct Https;

impl<S: System> Witness<S> for Https {
    fn testify(&mut self, output: &mut Output<S>, host: &[u8], at: i64) -> Testimony {
        let deadline_us = output.system.monotonic_us() + u64::from(PROOF_HOST_TIMEOUT_MS) * 1000;
        let Ok(address) = resolve(output, host) else {
            return Testimony::Unreachable;
        };
        let descriptor = match output.system.open_stream_socket() {
            Ok(descriptor) => descriptor,
            Err(errno) => {
                output.error(&[b"socket: ", errno.message()]);
                return Testimony::Unreachable;
            }
        };
        let testimony = ask_date(output, descriptor, address, host, at, deadline_us);
        output.system.close(descriptor);
        testimony
    }
}

struct Timed<'r, 'o, S: System> {
    output: &'r RefCell<&'o mut Output<S>>,
    descriptor: usize,
    deadline_us: u64,
    shown: &'r Shown<264>,
}

fn remaining_ms<S: System>(system: &mut S, deadline_us: u64) -> Option<u32> {
    let left_us = deadline_us.saturating_sub(system.monotonic_us());
    match left_us / 1000 {
        0 => None,
        left_ms => Some(left_ms.min(u64::from(u32::MAX)) as u32),
    }
}

impl<S: System> Transport for Timed<'_, '_, S> {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped> {
        let mut output = self.output.borrow_mut();
        output.system.send(self.descriptor, bytes).map_err(|errno| {
            output.error(&[self.shown.as_bytes(), b": ", errno.message()]);
            Stopped
        })
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped> {
        let mut output = self.output.borrow_mut();
        loop {
            let Some(wait_ms) = remaining_ms(&mut output.system, self.deadline_us) else {
                output.error(&[self.shown.as_bytes(), b": no answer within 10 seconds"]);
                return Err(Stopped);
            };
            match output.system.receive(self.descriptor, buffer, wait_ms) {
                Ok(Some(count)) => return Ok(count),
                Ok(None) | Err(Errno::EINTR) => {}
                Err(errno) => {
                    output.error(&[self.shown.as_bytes(), b": ", errno.message()]);
                    return Err(Stopped);
                }
            }
        }
    }
}

fn ask_date<S: System>(
    output: &mut Output<S>,
    descriptor: usize,
    address: Ipv4Address,
    host: &[u8],
    at: i64,
    deadline_us: u64,
) -> Testimony {
    let shown = Shown::<264>::of(format_args!(
        "{}:{}",
        core::str::from_utf8(host).unwrap_or("?"),
        http::DEFAULT_SECURE_PORT
    ));
    let Some(wait_ms) = remaining_ms(&mut output.system, deadline_us) else {
        return Testimony::Unreachable;
    };
    let connected = output
        .system
        .connect(descriptor, address, http::DEFAULT_SECURE_PORT, wait_ms);
    if let Err(errno) = connected {
        output.error(&[shown.as_bytes(), b": ", errno.message()]);
        return Testimony::Unreachable;
    }
    let mut randomness = [0u8; RANDOMNESS_BYTES];
    if let Err(errno) = output.system.random(&mut randomness) {
        output.error(&[b"getrandom: ", errno.message()]);
        return Testimony::Unreachable;
    }
    let settings = Settings { host, now: at };
    let mut buffers = Buffers::new();
    let cell = RefCell::new(output);
    let mut socket = Timed {
        output: &cell,
        descriptor,
        deadline_us,
        shown: &shown,
    };
    let mut roots = RootsFile { output: &cell };
    let connected = client::connect(
        &mut socket,
        &mut buffers,
        &settings,
        &randomness,
        &mut roots,
    );
    match connected {
        Ok(mut session) => read_date(&mut session, &mut socket, host),
        Err(error) => {
            tls_failure(&mut cell.borrow_mut(), &shown, error);
            Testimony::Unreachable
        }
    }
}

fn failed<S: System>(socket: &Timed<'_, '_, S>, error: client::Error) -> Testimony {
    tls_failure(&mut socket.output.borrow_mut(), socket.shown, error);
    Testimony::Unreachable
}

fn head_request(host: &[u8], out: &mut [u8; REQUEST_BYTES]) -> Option<usize> {
    let mut length = 0;
    for part in [
        &b"HEAD / HTTP/1.1\r\nHost: "[..],
        host,
        b"\r\nUser-Agent: ",
        USER_AGENT,
        b"\r\nAccept: */*\r\nConnection: close\r\n\r\n",
    ] {
        let end = length + part.len();
        out.get_mut(length..end)?.copy_from_slice(part);
        length = end;
    }
    Some(length)
}

fn read_date<S: System>(
    session: &mut Session<'_>,
    socket: &mut Timed<'_, '_, S>,
    host: &[u8],
) -> Testimony {
    let shown = socket.shown;
    let mut request = [0u8; REQUEST_BYTES];
    let Some(length) = head_request(host, &mut request) else {
        return Testimony::Unreachable;
    };
    if let Err(error) = session.write(socket, &request[..length]) {
        return failed(socket, error);
    }
    let mut head = [0u8; PROOF_HEAD_BYTES];
    let mut filled = 0;
    let end = loop {
        if let Some(end) = http::head_end(&head[..filled]) {
            break end;
        }
        let problem: &[u8] = match filled == head.len() {
            true => b": the server's answer has a head over 8 KiB",
            false => match session.read(socket, &mut head[filled..]) {
                Ok(0) => b": the server closed the connection without answering",
                Ok(count) => {
                    filled += count;
                    continue;
                }
                Err(error) => return failed(socket, error),
            },
        };
        socket
            .output
            .borrow_mut()
            .error(&[shown.as_bytes(), problem]);
        return Testimony::Unreachable;
    };
    let at_us = socket.output.borrow_mut().system.monotonic_us();
    let validity = session.certificate_validity();
    let _ = session.close(socket);
    let head = &head[..end];
    let dated = http::parse_head(head)
        .ok()
        .and_then(|_| http::header(head, b"date"))
        .and_then(imf_fixdate);
    let (Some(date), Some((valid_from, valid_until))) = (dated, validity) else {
        socket
            .output
            .borrow_mut()
            .error(&[shown.as_bytes(), b": the answer has no Date header"]);
        return Testimony::Unreachable;
    };
    let age = http::header(head, b"age")
        .and_then(age_seconds)
        .unwrap_or(0);
    Testimony::Dated {
        date: date + age,
        at_us,
        valid_from,
        valid_until,
    }
}

#[cfg(test)]
mod tests;
