use super::*;
use crate::fake::Fake;
use eggos_runtime::errno::Errno;
use eggos_settings_file::clock_sync;

const NOW: i64 = 1_793_282_709;
const SERVER: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
const HALF_ROUND_TRIP_NS: i64 = 10_000_000;
const REPLY_US: u64 = 20_000;
const ROOTS_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";

fn clock(local: i64, server_at_reply_ns: i64) -> Fake {
    let mut fake = Fake::default().with_directory("/run", 0);
    fake.now = local;
    fake.network.ntp_servers.push((SERVER, 123));
    fake.network.ntp_time_ns = server_at_reply_ns - HALF_ROUND_TRIP_NS;
    fake.network.round_trip_us = 2 * HALF_ROUND_TRIP_NS as u64 / 1000;
    fake
}

fn seconds(seconds: i64) -> i64 {
    seconds * NANOSECONDS
}

fn with_roots(fake: Fake) -> Fake {
    fake.with_directory("/etc", 0)
        .with_directory("/etc/ssl", 0)
        .with_file("/etc/ssl/roots.pem", 0o644, ROOTS_PEM, 0)
}

fn with_unproven(fake: Fake, unproven_ms: u64) -> Fake {
    let mut record = Record::new(Outcome::Set, 1, b"pool.ntp.org");
    record.finished = Some(2);
    record.moved_ms = 5;
    record.unproven_ms = unproven_ms;
    let mut file = [0u8; LARGEST];
    let length = record.write(&mut file).unwrap();
    fake.with_directory("/run/eggos", 0).with_file(
        "/run/eggos/clock-sync",
        0o644,
        &file[..length],
        0,
    )
}

#[derive(Default)]
struct Witnesses {
    answers: Vec<Testimony>,
    asked: Vec<(Vec<u8>, i64)>,
}

impl Witness<Fake> for Witnesses {
    fn testify(&mut self, _output: &mut Output<Fake>, host: &[u8], at: i64) -> Testimony {
        self.asked.push((host.to_vec(), at));
        let answer = self.answers.get(self.asked.len() - 1).copied();
        answer.unwrap_or(Testimony::Unreachable)
    }
}

fn witnesses(answers: &[Testimony]) -> Witnesses {
    Witnesses {
        answers: answers.to_vec(),
        asked: Vec::new(),
    }
}

fn dated(date: i64) -> Testimony {
    Testimony::Dated {
        date,
        at_us: REPLY_US,
        valid_from: NOW - 30 * 86_400,
        valid_until: NOW + 30 * 86_400,
    }
}

struct Run {
    result: Result<i64, i32>,
    out: String,
    errors: String,
    fake: Fake,
}

impl Run {
    fn record(&self) -> Record {
        Record::parse(&self.fake.contents(clock_sync::PATH)).unwrap()
    }
}

fn sync_with(fake: Fake, asker: Asker, witness: &mut Witnesses) -> Run {
    let mut output = Output::new(fake, b"date");
    let result = sync(&mut output, b"10.0.2.2", asker, witness);
    output.flush();
    let fake = output.system;
    Run {
        result,
        out: fake.output(),
        errors: fake.errors(),
        fake,
    }
}

#[test]
fn the_record_s_paths_are_the_settings_file_crate_s() {
    assert_eq!(RECORD.to_str(), Ok(clock_sync::PATH));
    assert_eq!(RECORD_FOLDER.to_str(), Ok(clock_sync::FOLDER));
}

#[test]
fn judge_follows_the_automatic_clock_s_rule() {
    let automatic = |offset_ms, now, unproven_ms, roots| {
        judge(Asker::Automatic, offset_ms, NOW, now, unproven_ms, roots)
    };
    assert_eq!(
        judge(Asker::Person, 9_000_000, FLOOR - 1, NOW, 0, false),
        Verdict::Implausible
    );
    assert_eq!(automatic(999, NOW, 0, false), Verdict::AlreadyRight);
    assert_eq!(automatic(-999, NOW, 0, true), Verdict::AlreadyRight);
    assert_eq!(
        automatic(-1000, NOW, 7, false),
        Verdict::Step { unproven_ms: 1007 }
    );
    assert_eq!(
        automatic(3_600_000, NOW, 0, true),
        Verdict::Step {
            unproven_ms: 3_600_000
        }
    );
    assert_eq!(automatic(3_600_001, NOW, 0, false), Verdict::Unproven);
    assert_eq!(automatic(3_600_001, NOW, 0, true), Verdict::NeedsProof);
    assert_eq!(automatic(2_000, NOW, 3_599_000, false), Verdict::Unproven);
    assert_eq!(automatic(2_000, NOW, 3_599_000, true), Verdict::NeedsProof);
    assert_eq!(
        automatic(1_000, NOW, 3_599_000, false),
        Verdict::Step {
            unproven_ms: 3_600_000
        }
    );
    assert_eq!(
        automatic(1_000, FLOOR - 1, 0, false),
        Verdict::Step { unproven_ms: 0 }
    );
    assert_eq!(automatic(1_000, FLOOR - 1, 0, true), Verdict::NeedsProof);
    assert_eq!(
        automatic(900_000_000, 12, 5, false),
        Verdict::Step { unproven_ms: 5 }
    );
    assert_eq!(
        judge(Asker::Person, 900_000_000, NOW, NOW - 9_000, 5, true),
        Verdict::Step { unproven_ms: 5 }
    );
}

#[test]
fn https_agrees_within_two_minutes_of_the_time_server_and_inside_its_certificate() {
    let server_ns = seconds(NOW);
    let later = |date| Testimony::Dated {
        date,
        at_us: REPLY_US + 30_000_000,
        valid_from: NOW - 86_400,
        valid_until: NOW + 86_400,
    };
    assert_eq!(
        agreement(later(NOW + 30 + 120), server_ns, REPLY_US),
        Agreement::Agrees
    );
    assert_eq!(
        agreement(later(NOW + 30 - 120), server_ns, REPLY_US),
        Agreement::Agrees
    );
    assert_eq!(
        agreement(later(NOW + 30 + 121), server_ns, REPLY_US),
        Agreement::Disagrees {
            https_minus_server_ms: 121_000
        }
    );
    let expired = Testimony::Dated {
        date: NOW,
        at_us: REPLY_US,
        valid_from: NOW - 86_400,
        valid_until: NOW - 1,
    };
    assert_eq!(
        agreement(expired, server_ns, REPLY_US),
        Agreement::Disagrees {
            https_minus_server_ms: 0
        }
    );
    assert_eq!(
        agreement(Testimony::Unreachable, server_ns, REPLY_US),
        Agreement::Silent
    );
}

#[test]
fn a_person_s_sync_steps_any_amount_says_it_was_not_checked_and_keeps_the_budget() {
    let fake = with_unproven(clock(NOW - 7_200, seconds(NOW)), 3_000_000);
    let synced = sync_with(fake, Asker::Person, &mut Witnesses::default());
    assert_eq!((synced.result, synced.errors.as_str()), (Ok(NOW), ""));
    assert_eq!(
        synced.out,
        "Synchronized with 10.0.2.2 (stratum 1): the clock moved by +7200.000 s (not verified \
         over HTTPS)\n"
    );
    let record = synced.record();
    assert_eq!(
        (
            record.outcome,
            record.moved_ms,
            record.unproven_ms,
            record.finished
        ),
        (Outcome::Set, 7_200_000, 3_000_000, Some(NOW))
    );
    assert_eq!(record.started, NOW - 7_200);
    let synced = sync_with(
        clock(NOW - 60, seconds(NOW)),
        Asker::Person,
        &mut Witnesses::default(),
    );
    assert_eq!(
        synced.out,
        "Synchronized with 10.0.2.2 (stratum 1): the clock moved by +60.000 s\n"
    );
}

#[test]
fn under_a_second_off_the_clock_is_left_alone() {
    let synced = sync_with(
        clock(NOW, seconds(NOW) - 400_000_000),
        Asker::Automatic,
        &mut Witnesses::default(),
    );
    assert_eq!(synced.result, Ok(NOW));
    assert_eq!(
        synced.out,
        "Synchronized with 10.0.2.2 (stratum 1): the clock was already right (off by -0.400 s)\n"
    );
    assert!(synced.fake.set_times.is_empty());
    let record = synced.record();
    assert_eq!((record.outcome, record.moved_ms), (Outcome::Set, 0));
}

#[test]
fn a_time_before_2026_is_implausible_and_not_used() {
    let synced = sync_with(
        clock(NOW, seconds(FLOOR - 1)),
        Asker::Person,
        &mut Witnesses::default(),
    );
    assert_eq!(synced.result, Err(FAILURE));
    assert_eq!(
        synced.errors,
        "date: 10.0.2.2: implausible: the time server says 2025-12-31T23:59:59Z, before 2026: \
         not used\n"
    );
    assert!(synced.fake.set_times.is_empty());
    let record = synced.record();
    assert_eq!(record.outcome, Outcome::NoAnswer);
    assert!(record.reason().starts_with(b"10.0.2.2: implausible"));
}

#[test]
fn the_automatic_clock_steps_up_to_an_hour_while_its_budget_lasts() {
    let synced = sync_with(
        clock(NOW - 3_600, seconds(NOW)),
        Asker::Automatic,
        &mut Witnesses::default(),
    );
    assert_eq!(synced.result, Ok(NOW));
    assert_eq!(synced.record().unproven_ms, 3_600_000);
    let fake = with_unproven(clock(NOW - 100, seconds(NOW)), 3_500_000);
    let synced = sync_with(fake, Asker::Automatic, &mut Witnesses::default());
    assert_eq!(synced.result, Ok(NOW));
    assert_eq!(synced.record().unproven_ms, 3_600_000);
    let fake = with_unproven(clock(NOW - 101, seconds(NOW)), 3_500_000);
    let synced = sync_with(fake, Asker::Automatic, &mut Witnesses::default());
    assert_eq!(synced.result, Err(FAILURE));
    assert!(synced.fake.set_times.is_empty());
    assert_eq!(
        synced.errors,
        "date: 10.0.2.2: unproven: the clock is off by +101.000 s, more than the automatic \
         clock moves without an HTTPS check, and there are no roots to check with: install \
         roots at /etc/ssl/roots.pem or run date --sync\n"
    );
    let record = synced.record();
    assert_eq!(
        (record.outcome, record.moved_ms, record.unproven_ms),
        (Outcome::Unconfirmed, 101_000, 3_500_000)
    );
    let synced = sync_with(
        clock(NOW - 3_601, seconds(NOW)),
        Asker::Automatic,
        &mut Witnesses::default(),
    );
    assert_eq!(synced.record().outcome, Outcome::Unconfirmed);
}

#[test]
fn an_unset_clock_without_roots_takes_any_step() {
    let mut witness = Witnesses::default();
    let synced = sync_with(clock(5_000, seconds(NOW)), Asker::Automatic, &mut witness);
    assert_eq!(synced.result, Ok(NOW));
    assert!(witness.asked.is_empty());
    let record = synced.record();
    assert_eq!((record.outcome, record.unproven_ms), (Outcome::Set, 0));
}

#[test]
fn with_roots_even_an_unset_clock_needs_two_https_dates_to_agree() {
    let mut witness = witnesses(&[dated(NOW), dated(NOW + 5)]);
    let fake = with_roots(clock(5_000, seconds(NOW)));
    let synced = sync_with(fake, Asker::Automatic, &mut witness);
    assert_eq!((synced.result, synced.errors.as_str()), (Ok(NOW), ""));
    assert_eq!(
        witness.asked,
        [
            (b"www.google.com".to_vec(), NOW),
            (b"www.cloudflare.com".to_vec(), NOW)
        ]
    );
    assert!(
        synced
            .out
            .ends_with("s (confirmed over HTTPS by www.google.com and www.cloudflare.com)\n")
    );
    let record = synced.record();
    assert_eq!(
        (record.outcome, record.unproven_ms),
        (Outcome::Confirmed, 0)
    );
}

#[test]
fn a_proven_step_is_applied_and_resets_the_budget() {
    let mut witness = witnesses(&[Testimony::Unreachable, dated(NOW), dated(NOW + 60)]);
    let fake = with_unproven(with_roots(clock(NOW - 7_200, seconds(NOW))), 3_000_000);
    let synced = sync_with(fake, Asker::Automatic, &mut witness);
    assert_eq!(synced.result, Ok(NOW));
    assert_eq!(witness.asked.len(), 3);
    assert_eq!(
        synced.out,
        "Synchronized with 10.0.2.2 (stratum 1): the clock moved by +7200.000 s (confirmed \
         over HTTPS by www.cloudflare.com and www.apple.com)\n"
    );
    let record = synced.record();
    assert_eq!(
        (record.outcome, record.moved_ms, record.unproven_ms),
        (Outcome::Confirmed, 7_200_000, 0)
    );
    let mut witness = witnesses(&[dated(NOW), dated(NOW), dated(NOW)]);
    let fake = with_roots(clock(NOW - 7_200, seconds(NOW)));
    sync_with(fake, Asker::Automatic, &mut witness);
    assert_eq!(witness.asked.len(), 2);
}

#[test]
fn fewer_than_two_agreeing_hosts_leave_the_clock_and_say_why() {
    let mut witness = witnesses(&[dated(NOW), Testimony::Unreachable]);
    let fake = with_roots(clock(NOW - 7_200, seconds(NOW)));
    let synced = sync_with(fake, Asker::Automatic, &mut witness);
    assert_eq!(synced.result, Err(FAILURE));
    assert!(synced.fake.set_times.is_empty());
    assert_eq!(
        synced.errors,
        "date: 10.0.2.2: proof unreachable: the clock is off by +7200.000 s, and fewer than 2 of \
         www.google.com, www.cloudflare.com and www.apple.com confirmed it over HTTPS: not \
         applied\n"
    );
    let record = synced.record();
    assert_eq!(
        (record.outcome, record.moved_ms),
        (Outcome::Unconfirmed, 7_200_000)
    );
    let mut witness = witnesses(&[dated(NOW + 3_600), dated(NOW), Testimony::Unreachable]);
    let fake = with_roots(clock(NOW - 7_200, seconds(NOW)));
    let synced = sync_with(fake, Asker::Automatic, &mut witness);
    assert_eq!(synced.result, Err(FAILURE));
    assert_eq!(
        synced.errors,
        "date: 10.0.2.2: proof disagrees: the time server says the clock is off by +7200.000 s, \
         HTTPS says by +10800.000 s: not applied\n"
    );
    assert_eq!(synced.record().outcome, Outcome::Unconfirmed);
}

#[test]
fn a_set_clock_without_roots_never_asks_https() {
    let mut witness = witnesses(&[dated(NOW), dated(NOW)]);
    let synced = sync_with(
        clock(NOW - 7_200, seconds(NOW)),
        Asker::Automatic,
        &mut witness,
    );
    assert_eq!(synced.result, Err(FAILURE));
    assert!(witness.asked.is_empty());
    assert!(synced.errors.ends_with("run date --sync\n"));
}

#[test]
fn a_refused_step_is_not_allowed_and_no_address_is_said() {
    let mut refused = clock(NOW - 60, seconds(NOW));
    refused.set_time_error = Some(Errno::EPERM);
    let synced = sync_with(refused, Asker::Automatic, &mut Witnesses::default());
    assert_eq!(synced.result, Err(FAILURE));
    assert_eq!(
        synced.errors,
        "date: cannot set date: Operation not permitted\n"
    );
    assert_eq!(synced.record().outcome, Outcome::NotAllowed);
    let waiting = clock(NOW - 60, seconds(NOW)).with_file(
        "/proc/net/interfaces",
        0o444,
        b"lo up 00:00:00:00:00:00 65536 off 127.0.0.1/8 - - - - - 12 1008 0 0 12 1008 0\n\
          eth0 up 52:54:00:12:34:56 1500 asking - - - - - - 0 0 0 0 0 0 0\n",
        0,
    );
    let synced = sync_with(waiting, Asker::Automatic, &mut Witnesses::default());
    assert_eq!(
        synced.errors,
        "date: 10.0.2.2: the network adapter has no address\n"
    );
    assert!(synced.fake.network.sent.is_empty());
    let record = synced.record();
    assert_eq!(record.outcome, Outcome::NoAddress);
    assert_eq!(
        record.reason(),
        b"10.0.2.2: the network adapter has no address"
    );
    let silent = clock(NOW - 60, seconds(NOW));
    let mut output = Output::new(silent, b"date");
    assert_eq!(
        sync(
            &mut output,
            b"10.0.2.9",
            Asker::Automatic,
            &mut Witnesses::default()
        ),
        Err(FAILURE)
    );
    let record = Record::parse(&output.system.contents(clock_sync::PATH)).unwrap();
    assert_eq!(
        (record.outcome, record.server(), record.reason()),
        (
            Outcome::NoAnswer,
            &b"10.0.2.9"[..],
            &b"10.0.2.9: no answer from the time server"[..]
        )
    );
}

#[test]
fn date_takes_automatic_with_sync_only_and_set_forgets_the_unproven_steps() {
    let run = |fake: Fake, arguments: &[&CStr]| {
        let mut output = Output::new(fake, b"date");
        let status = crate::date::date(&mut output, arguments);
        let status = output.finish(status);
        let errors = output.system.errors();
        (status, errors, output.system)
    };
    let (status, errors, _) = run(clock(NOW, seconds(NOW)), &[c"--automatic"]);
    assert_eq!(
        (status, errors.as_str()),
        (1, "date: --automatic needs --sync\n")
    );
    let fake = with_unproven(clock(NOW, seconds(NOW)), 3_000_000);
    let (status, _, fake) = run(fake, &[c"-u", c"-s", c"2026-10-29 15:00"]);
    assert_eq!(status, 0);
    let record = Record::parse(&fake.contents(clock_sync::PATH)).unwrap();
    assert_eq!((record.outcome, record.unproven_ms), (Outcome::Set, 0));
    let fake = with_roots(clock(NOW - 7_200, seconds(NOW)));
    let (status, errors, fake) = run(fake, &[c"--sync=10.0.2.2", c"--automatic"]);
    assert_eq!(status, 1);
    assert!(
        errors.starts_with(
            "date: www.google.com: no DNS server: /etc/resolv.conf names none\n\
             date: www.cloudflare.com: no DNS server"
        ),
        "{errors}"
    );
    assert!(errors.ends_with("over HTTPS: not applied\n"), "{errors}");
    assert!(fake.set_times.is_empty());
}

#[test]
fn an_imf_fixdate_reads_as_seconds_since_1970() {
    assert_eq!(imf_fixdate(b"Thu, 29 Oct 2026 14:05:09 GMT"), Some(NOW));
    assert_eq!(
        imf_fixdate(b"Sun, 06 Nov 1994 08:49:37 GMT"),
        Some(784_111_777)
    );
    for wrong in [
        &b"Thu, 29 Oct 2026 14:05:09 UTC"[..],
        b"Thu, 30 Feb 2026 14:05:09 GMT",
        b"Thu, 29 oct 2026 14:05:09 GMT",
        b"Thu, 29 Oct 2026 24:05:09 GMT",
        b"Thursday, 29-Oct-26 14:05:09 GMT",
        b"Thu, 29 Oct 2026 14:05:09 GMT ",
    ] {
        assert_eq!(imf_fixdate(wrong), None, "{wrong:?}");
    }
    assert_eq!(age_seconds(b"42"), Some(42));
    assert_eq!(age_seconds(b"-1"), None);
    let mut request = [0u8; REQUEST_BYTES];
    let length = head_request(b"www.apple.com", &mut request).unwrap();
    assert_eq!(
        &request[..length],
        b"HEAD / HTTP/1.1\r\nHost: www.apple.com\r\nUser-Agent: EggOS-date\r\nAccept: */*\r\n\
          Connection: close\r\n\r\n"
    );
}
