use crate::network::{Shown as ShownText, resolve};
use crate::output::{FAILURE, Output};
use crate::system::{DatagramKind, Duration, SymbolicLinks, System};
use crate::zone::{local_seconds, local_zone};
use core::ffi::CStr;
use eggos_calendar::zone::{Rule, UTC};
use eggos_calendar::{DateTime, MONTHS, ParseError, WEEKDAYS, parse_setting};
use eggos_net::Ipv4Address;
use eggos_net::sntp::{self, ReplyError, Timestamp};
use eggos_runtime::errno::Errno;

pub const DEFAULT_TIME_SERVER: &[u8] = b"pool.ntp.org";
pub const SYNC_TIMEOUT_MS: u32 = 3000;
pub const SYNC_ATTEMPTS: u32 = 3;
const NANOSECONDS: i64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown<'a> {
    Now,
    File(&'a CStr),
}

pub fn date<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut set: Option<&CStr> = None;
    let mut sync: Option<&[u8]> = None;
    let mut shown = Shown::Now;
    let mut format: Option<&[u8]> = None;
    let mut universal = false;
    let mut options_end = false;
    let mut index = 0;
    while let Some(&argument) = arguments.get(index) {
        index += 1;
        let bytes = argument.to_bytes();
        if !options_end && bytes == b"--" {
            options_end = true;
            continue;
        }
        if !options_end && bytes.len() > 1 && bytes[0] == b'-' {
            match bytes {
                b"-u" | b"--utc" | b"--universal" => universal = true,
                b"--sync" => sync = Some(DEFAULT_TIME_SERVER),
                _ if bytes.starts_with(b"--sync=") => sync = Some(&bytes[b"--sync=".len()..]),
                b"-s" | b"--set" | b"-r" | b"--reference" => {
                    let Some(&value) = arguments.get(index) else {
                        output.error(&[b"option requires an argument -- '", &bytes[1..], b"'"]);
                        return FAILURE;
                    };
                    index += 1;
                    match bytes {
                        b"-s" | b"--set" => set = Some(value),
                        _ => shown = Shown::File(value),
                    }
                }
                _ => match (
                    bytes.strip_prefix(b"--set="),
                    bytes.strip_prefix(b"--reference="),
                ) {
                    (Some(value), _) => set = Some(cstr_tail(argument, bytes.len() - value.len())),
                    (_, Some(value)) => {
                        shown = Shown::File(cstr_tail(argument, bytes.len() - value.len()));
                    }
                    _ => return output.invalid_option(bytes),
                },
            }
            continue;
        }
        match (bytes.strip_prefix(b"+"), format) {
            (Some(wanted), None) => format = Some(wanted),
            (Some(_), Some(_)) => {
                output.error(&[b"extra operand '", bytes, b"'"]);
                return FAILURE;
            }
            (None, _) => {
                output.error(&[
                    b"invalid date '",
                    bytes,
                    b"': set the clock with -s 'YYYY-MM-DD HH:MM[:SS]' (UTC)",
                ]);
                return FAILURE;
            }
        }
    }
    let seconds = match (sync, set, shown) {
        (Some(_), Some(_), _) | (Some(_), _, Shown::File(_)) | (None, Some(_), Shown::File(_)) => {
            output.error(&[b"the options to set the time and to show a file's time are exclusive"]);
            return FAILURE;
        }
        (Some(server), None, Shown::Now) => match sync_clock(output, server) {
            Ok(seconds) => seconds,
            Err(status) => return status,
        },
        (None, Some(text), Shown::Now) => match set_clock(output, text) {
            Ok(seconds) => seconds,
            Err(status) => return status,
        },
        (None, None, Shown::File(path)) => {
            match output.system.status(None, path, SymbolicLinks::Follow) {
                Ok(status) => status.modified,
                Err(errno) => {
                    output.file_error(path, errno);
                    return FAILURE;
                }
            }
        }
        (None, None, Shown::Now) => output.system.now() as i64,
    };
    let zone = match universal {
        true => UTC,
        false => local_zone(&mut output.system),
    };
    write_formatted(output, format.unwrap_or(DEFAULT_FORMAT), seconds, &zone);
    output.write(b"\n");
    0
}

const DEFAULT_FORMAT: &[u8] = b"%a %b %e %H:%M:%S %Z %Y";

fn cstr_tail(argument: &CStr, skip: usize) -> &CStr {
    CStr::from_bytes_with_nul(&argument.to_bytes_with_nul()[skip..]).unwrap_or(c"")
}

fn set_clock<S: System>(output: &mut Output<S>, text: &CStr) -> Result<i64, i32> {
    let typed = core::str::from_utf8(text.to_bytes()).unwrap_or("");
    let now = output.system.now() as i64;
    let seconds = match parse_setting(typed, now) {
        Ok(seconds) => seconds,
        Err(ParseError::NotADateAndTime) => {
            output.error(&[
                b"invalid date '",
                text.to_bytes(),
                b"': type YYYY-MM-DD HH:MM[:SS] or HH:MM[:SS], in UTC",
            ]);
            return Err(FAILURE);
        }
        Err(ParseError::NeedsTheDate) => {
            output.error(&[b"the clock is not set: give the date too, YYYY-MM-DD HH:MM"]);
            return Err(FAILURE);
        }
        Err(ParseError::YearOutOfRange) => {
            output.error(&[
                b"invalid date '",
                text.to_bytes(),
                b"': the year must be 2000 to 2099",
            ]);
            return Err(FAILURE);
        }
    };
    let since_epoch = Duration {
        seconds: seconds as u64,
        nanoseconds: 0,
    };
    match output.system.set_time(since_epoch) {
        Ok(()) => Ok(seconds),
        Err(errno) => {
            output.error(&[b"cannot set date: ", errno.message()]);
            Err(FAILURE)
        }
    }
}

fn time_server(server: &[u8]) -> Option<(&[u8], u16)> {
    let (host, port) = match server.iter().rposition(|&byte| byte == b':') {
        Some(colon) => {
            let digits = &server[colon + 1..];
            if digits.is_empty() || digits.len() > 5 || !digits.iter().all(u8::is_ascii_digit) {
                return None;
            }
            let port = digits
                .iter()
                .fold(0u32, |value, digit| value * 10 + u32::from(digit - b'0'));
            (
                &server[..colon],
                u16::try_from(port).ok().filter(|&port| port > 0)?,
            )
        }
        None => (server, sntp::PORT),
    };
    match host.is_empty() {
        true => None,
        false => Some((host, port)),
    }
}

struct Answer {
    reply: sntp::Reply,
    round_trip_ns: i64,
    arrived_us: u64,
}

fn ask_time<S: System>(
    output: &mut Output<S>,
    socket: usize,
    address: Ipv4Address,
    port: u16,
    server: &[u8],
) -> Result<Answer, i32> {
    let mut packet = [0u8; sntp::PACKET_BYTES];
    let mut reply = [0u8; 2 * sntp::PACKET_BYTES];
    for _ in 0..SYNC_ATTEMPTS {
        let mut cookie = [0u8; 8];
        if output.system.random(&mut cookie).is_err() {
            cookie = output.system.monotonic_us().to_be_bytes();
        }
        let sent = Timestamp {
            seconds: u32::from_be_bytes([cookie[0], cookie[1], cookie[2], cookie[3]]),
            fraction: u32::from_be_bytes([cookie[4], cookie[5], cookie[6], cookie[7]]),
        };
        sntp::write_request(&mut packet, sent);
        let start = output.system.monotonic_us();
        if let Err(errno) = output.system.send_to(socket, &packet, address, port) {
            output.error(&[server, b": ", errno.message()]);
            return Err(FAILURE);
        }
        loop {
            let waited_ms = (output.system.monotonic_us().saturating_sub(start) / 1000) as u32;
            if waited_ms >= SYNC_TIMEOUT_MS {
                break;
            }
            match output
                .system
                .receive_from(socket, &mut reply, SYNC_TIMEOUT_MS - waited_ms)
            {
                Ok(Some((count, sender))) if sender.address == address && sender.port == port => {
                    let arrived_us = output.system.monotonic_us();
                    match sntp::parse_reply(&reply[..count], sent) {
                        Ok(parsed) => {
                            return Ok(Answer {
                                reply: parsed,
                                round_trip_ns: (arrived_us.saturating_sub(start) * 1000) as i64,
                                arrived_us,
                            });
                        }
                        Err(ReplyError::NotOurs) => {}
                        Err(error) => {
                            output.error(&[server, b": ", error.description().as_bytes()]);
                            return Err(FAILURE);
                        }
                    }
                }
                Ok(Some(_)) | Err(Errno::EINTR) => {}
                Ok(None) => break,
                Err(errno) => {
                    output.error(&[server, b": ", errno.message()]);
                    return Err(FAILURE);
                }
            }
        }
    }
    output.error(&[server, b": no answer from the time server"]);
    Err(FAILURE)
}

fn sync_clock<S: System>(output: &mut Output<S>, server: &[u8]) -> Result<i64, i32> {
    let Some((host, port)) = time_server(server) else {
        output.error(&[
            b"invalid time server '",
            server,
            b"': give host or host:port",
        ]);
        return Err(FAILURE);
    };
    let address = resolve(output, host)?;
    let socket = match output.system.open_datagram_socket(DatagramKind::Udp) {
        Ok(socket) => socket,
        Err(errno) => {
            output.error(&[b"socket: ", errno.message()]);
            return Err(FAILURE);
        }
    };
    let answer = ask_time(output, socket, address, port, server);
    output.system.close(socket);
    let answer = answer?;
    let elapsed_ns = (output
        .system
        .monotonic_us()
        .saturating_sub(answer.arrived_us)
        * 1000) as i64;
    let time_ns = sntp::arrival_time(&answer.reply, answer.round_trip_ns) + elapsed_ns;
    let before = output.system.now_with_nanoseconds();
    let before_ns = before.seconds as i64 * NANOSECONDS + i64::from(before.nanoseconds);
    let since_epoch = Duration {
        seconds: time_ns.div_euclid(NANOSECONDS).max(0) as u64,
        nanoseconds: time_ns.rem_euclid(NANOSECONDS) as u32,
    };
    if let Err(errno) = output.system.set_time(since_epoch) {
        output.error(&[b"cannot set date: ", errno.message()]);
        return Err(FAILURE);
    }
    let moved = time_ns - before_ns;
    let stratum = ShownText::<4>::of(answer.reply.stratum);
    output.write(b"Synchronized with ");
    output.write(server);
    output.write(b" (stratum ");
    output.write(stratum.as_bytes());
    output.write(b"): the clock moved by ");
    output.write(if moved < 0 { b"-" } else { b"+" });
    write_padded(
        output,
        (moved.unsigned_abs() / 1_000_000_000) as i64,
        1,
        b'0',
    );
    output.write(b".");
    write_padded(
        output,
        (moved.unsigned_abs() % 1_000_000_000 / 1_000_000) as i64,
        3,
        b'0',
    );
    output.write(b" s\n");
    Ok(since_epoch.seconds as i64)
}

fn write_padded<S: System>(output: &mut Output<S>, value: i64, width: usize, pad: u8) {
    if value < 0 {
        output.write(b"-");
    }
    let mut digits = [0u8; 20];
    let text = crate::output::decimal(value.unsigned_abs(), &mut digits);
    for _ in text.len()..width {
        output.write(&[pad]);
    }
    output.write(text);
}

fn write_formatted<S: System>(output: &mut Output<S>, format: &[u8], seconds: i64, zone: &Rule) {
    let local = zone.at(seconds);
    let moment = DateTime::from_unix(local_seconds(zone, seconds));
    let weekday = WEEKDAYS[moment.weekday()].as_bytes();
    let month = MONTHS[moment.month as usize - 1].as_bytes();
    let hour_12 = match moment.hour % 12 {
        0 => 12,
        hour => hour,
    };
    let mut rest = format;
    while let Some((&byte, after)) = rest.split_first() {
        rest = after;
        if byte != b'%' {
            output.write(&[byte]);
            continue;
        }
        let Some((&conversion, after)) = rest.split_first() else {
            output.write(b"%");
            break;
        };
        rest = after;
        match conversion {
            b'a' => output.write(&weekday[..3]),
            b'A' => output.write(weekday),
            b'b' | b'h' => output.write(&month[..3]),
            b'B' => output.write(month),
            b'c' => write_formatted(output, b"%a %b %e %H:%M:%S %Y", seconds, zone),
            b'C' => write_padded(output, moment.year.div_euclid(100), 2, b'0'),
            b'd' => write_padded(output, i64::from(moment.day), 2, b'0'),
            b'D' => write_formatted(output, b"%m/%d/%y", seconds, zone),
            b'e' => write_padded(output, i64::from(moment.day), 2, b' '),
            b'F' => write_formatted(output, b"%Y-%m-%d", seconds, zone),
            b'H' => write_padded(output, i64::from(moment.hour), 2, b'0'),
            b'I' => write_padded(output, i64::from(hour_12), 2, b'0'),
            b'j' => write_padded(output, i64::from(moment.day_of_year()), 3, b'0'),
            b'k' => write_padded(output, i64::from(moment.hour), 2, b' '),
            b'l' => write_padded(output, i64::from(hour_12), 2, b' '),
            b'm' => write_padded(output, i64::from(moment.month), 2, b'0'),
            b'M' => write_padded(output, i64::from(moment.minute), 2, b'0'),
            b'n' => output.write(b"\n"),
            b'p' => output.write(if moment.hour < 12 { b"AM" } else { b"PM" }),
            b'r' => write_formatted(output, b"%I:%M:%S %p", seconds, zone),
            b'R' => write_formatted(output, b"%H:%M", seconds, zone),
            b's' => write_padded(output, seconds, 1, b'0'),
            b'S' => write_padded(output, i64::from(moment.second), 2, b'0'),
            b't' => output.write(b"\t"),
            b'T' => write_formatted(output, b"%H:%M:%S", seconds, zone),
            b'u' => write_padded(output, (moment.weekday() as i64 + 6) % 7 + 1, 1, b'0'),
            b'w' => write_padded(output, moment.weekday() as i64, 1, b'0'),
            b'y' => write_padded(output, moment.year.rem_euclid(100), 2, b'0'),
            b'Y' => write_padded(output, moment.year, 4, b'0'),
            b'z' => output.write(&local.numeric_offset()),
            b'Z' => output.write(local.abbreviation.as_str().as_bytes()),
            b'%' => output.write(b"%"),
            other => output.write(&[b'%', other]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use eggos_runtime::errno::Errno;

    const NOW: i64 = 1_793_282_709;

    fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String, Fake) {
        let mut output = Output::new(fake, b"date");
        let status = date(&mut output, arguments);
        let status = output.finish(status);
        let fake = output.system;
        let (out, errors) = (fake.output(), fake.errors());
        (status, out, errors, fake)
    }

    fn at_now() -> Fake {
        let mut fake = Fake::default();
        fake.now = NOW;
        fake
    }

    #[test]
    fn with_no_format_it_shows_the_time_as_date_does_in_the_c_locale() {
        let (status, out, _, _) = run(at_now(), &[]);
        assert_eq!(
            (status, out.as_str()),
            (0, "Thu Oct 29 14:05:09 UTC 2026\n")
        );
        let (_, out, _, _) = run(at_now(), &[c"-u"]);
        assert_eq!(out, "Thu Oct 29 14:05:09 UTC 2026\n");
    }

    #[test]
    fn a_format_takes_strftime_s_conversions() {
        let (status, out, _, _) = run(
            at_now(),
            &[c"+%s %F %T %Y%m%d %H%M%S %a %A %b %B %e %j %y %C %I %p %u %w %Z %z %% %q %D %R"],
        );
        assert_eq!(status, 0);
        assert_eq!(
            out,
            "1793282709 2026-10-29 14:05:09 20261029 140509 Thu Thursday Oct October 29 302 26 \
             20 02 PM 4 4 UTC +0000 % %q 10/29/26 14:05\n"
        );
        let (_, out, _, _) = run(at_now(), &[c"+EGGOS-DATE %s"]);
        assert_eq!(out, "EGGOS-DATE 1793282709\n");
        let (_, out, _, _) = run(Fake::default(), &[c"+%e|%k|%l|%I|%p"]);
        assert_eq!(out, "14|22|10|10|PM\n");
    }

    #[test]
    fn reference_shows_a_file_s_modification_time() {
        let file = || at_now().with_file("/tmp/x", 0o644, b"", 951_782_400);
        let (status, out, _, _) = run(file(), &[c"-r", c"/tmp/x", c"+%F %T"]);
        assert_eq!((status, out.as_str()), (0, "2000-02-29 00:00:00\n"));
        let (_, out, _, _) = run(file(), &[c"--reference=/tmp/x", c"+%s"]);
        assert_eq!(out, "951782400\n");
        let (status, _, errors, _) = run(at_now(), &[c"-r", c"/nope"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "date: /nope: No such file or directory\n");
    }

    fn whole_seconds(seconds: u64) -> Duration {
        Duration {
            seconds,
            nanoseconds: 0,
        }
    }

    #[test]
    fn set_parses_the_moment_sets_the_clock_and_shows_it() {
        let (status, out, _, fake) = run(at_now(), &[c"-s", c"2030-01-02 03:04:05"]);
        assert_eq!(
            (status, out.as_str()),
            (0, "Wed Jan  2 03:04:05 UTC 2030\n")
        );
        assert_eq!(fake.set_times, [whole_seconds(1_893_553_445)]);
        let (_, _, _, fake) = run(at_now(), &[c"--set=15:00", c"+%s"]);
        assert_eq!(fake.set_times, [whole_seconds(1_793_286_000)]);
        let (_, _, _, fake) = run(at_now(), &[c"-u", c"--set", c"2026-10-29T14:05:09Z"]);
        assert_eq!(fake.set_times, [whole_seconds(NOW as u64)]);
    }

    #[test]
    fn set_refuses_what_it_cannot_set_with_the_reason() {
        for (arguments, error) in [
            (
                &[c"-s", c"tomorrow"][..],
                "date: invalid date 'tomorrow': type YYYY-MM-DD HH:MM[:SS] or HH:MM[:SS], in UTC\n",
            ),
            (
                &[c"-s", c"1999-12-31 23:59"][..],
                "date: invalid date '1999-12-31 23:59': the year must be 2000 to 2099\n",
            ),
            (&[c"-s"][..], "date: option requires an argument -- 's'\n"),
            (
                &[c"-s", c"2030-01-02 03:04", c"-r", c"/tmp/x"][..],
                "date: the options to set the time and to show a file's time are exclusive\n",
            ),
            (
                &[c"0102030430"][..],
                "date: invalid date '0102030430': set the clock with -s 'YYYY-MM-DD HH:MM[:SS]' \
                 (UTC)\n",
            ),
            (&[c"+%s", c"+%F"][..], "date: extra operand '+%F'\n"),
            (&[c"-x"][..], "date: invalid option -- 'x'\n"),
        ] {
            let (status, out, errors, fake) = run(at_now(), arguments);
            assert_eq!((status, out.as_str()), (1, ""), "{arguments:?}");
            assert_eq!(errors, error);
            assert!(fake.set_times.is_empty());
        }
        let mut unset = Fake::default();
        unset.now = 12;
        let (_, _, errors, _) = run(unset, &[c"-s", c"14:05"]);
        assert_eq!(
            errors,
            "date: the clock is not set: give the date too, YYYY-MM-DD HH:MM\n"
        );
        let mut refused = at_now();
        refused.set_time_error = Some(Errno::EPERM);
        let (status, _, errors, _) = run(refused, &[c"-s", c"2030-01-02 03:04"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "date: cannot set date: Operation not permitted\n");
    }

    const TIME_SERVER: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

    fn with_time_server(port: u16) -> Fake {
        let mut fake = at_now();
        fake.now -= 100;
        fake.network.ntp_servers.push((TIME_SERVER, port));
        fake.network.ntp_time_ns = NOW * 1_000_000_000 + 500_000_000;
        fake.network.round_trip_us = 20_000;
        fake
    }

    #[test]
    fn sync_sets_the_clock_from_a_time_server_and_says_how_far_it_moved() {
        let (status, out, errors, fake) = run(with_time_server(123), &[c"--sync=10.0.2.2"]);
        assert_eq!((status, errors.as_str()), (0, ""));
        assert_eq!(
            out,
            "Synchronized with 10.0.2.2 (stratum 1): the clock moved by +100.510 s\n\
             Thu Oct 29 14:05:09 UTC 2026\n"
        );
        assert_eq!(
            fake.set_times,
            [Duration {
                seconds: NOW as u64,
                nanoseconds: 510_000_000
            }]
        );
        let sent = &fake.network.sent[0];
        assert_eq!((sent.1, sent.2, sent.3[0]), (TIME_SERVER, 123, 0x23));
        let (status, _, _, fake) = run(with_time_server(1123), &[c"--sync=10.0.2.2:1123", c"+%s"]);
        assert_eq!(status, 0);
        assert_eq!(fake.set_times.len(), 1);
    }

    #[test]
    fn sync_fails_with_the_reason_and_leaves_the_clock() {
        for (arguments, error) in [
            (
                &[c"--sync=10.0.2.9"][..],
                "date: 10.0.2.9: no answer from the time server\n",
            ),
            (
                &[c"--sync=h:99999"][..],
                "date: invalid time server 'h:99999': give host or host:port\n",
            ),
            (
                &[c"--sync", c"-s", c"2030-01-02 03:04"][..],
                "date: the options to set the time and to show a file's time are exclusive\n",
            ),
            (
                &[c"--sync"][..],
                "date: pool.ntp.org: no DNS server: /etc/resolv.conf names none\n",
            ),
        ] {
            let (status, out, errors, fake) = run(with_time_server(123), arguments);
            assert_eq!((status, out.as_str()), (1, ""), "{arguments:?}");
            assert_eq!(errors, error);
            assert!(fake.set_times.is_empty());
        }
        let (_, _, _, fake) = run(with_time_server(123), &[c"--sync=10.0.2.9"]);
        assert_eq!(fake.network.sent.len(), SYNC_ATTEMPTS as usize);
    }

    #[test]
    fn the_time_shows_in_tz_then_the_settings_zone_then_utc() {
        let mut fake = Fake::default();
        fake.now = 1_774_746_000;
        fake.time_zone_variable = Some(b"Europe/Berlin".to_vec());
        let (status, out, _, _) = run(fake, &[c"+%F %H:%M %Z %z %s"]);
        assert_eq!(status, 0);
        assert_eq!(out, "2026-03-29 03:00 CEST +0200 1774746000\n");
        let mut settings = Fake::default();
        settings.now = 1_774_746_000;
        let settings = settings
            .with_directory("/home", 0)
            .with_directory("/home/root", 0)
            .with_directory("/home/root/.config", 0)
            .with_directory("/home/root/.config/eggos", 0)
            .with_file(
                "/home/root/.config/eggos/settings",
                0o644,
                b"eggos-settings 1\ntime.zone=Asia/Kolkata\n",
                0,
            );
        let (_, out, _, mut settings) = run(settings, &[c"+%H:%M %Z"]);
        assert_eq!(out, "06:30 IST\n");
        settings.standard_output.clear();
        let (_, out, _, _) = run(settings, &[c"-u", c"+%H:%M %Z %z"]);
        assert_eq!(out, "01:00 UTC +0000\n");
        let mut unknown = Fake::default();
        unknown.now = 1_774_746_000;
        unknown.time_zone_variable = Some(b"Nowhere/Land".to_vec());
        let (_, out, _, _) = run(unknown, &[c"+%H %Z"]);
        assert_eq!(out, "01 UTC\n");
    }
}
