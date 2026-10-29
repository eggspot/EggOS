//! SNTP, the simple network time protocol (RFC 4330, over NTP version 4's packet, RFC 5905):
//! the client's request, the server's reply checked, and the time it gives corrected by half
//! the network's round trip.

/// The UDP port NTP servers listen on.
pub const PORT: u16 = 123;
/// The bytes of a packet without extensions.
pub const PACKET_BYTES: usize = 48;
/// The seconds from 1900 (NTP's epoch) to 1970.
pub const UNIX_EPOCH_IN_NTP_SECONDS: i64 = 2_208_988_800;

const NANOSECONDS: i64 = 1_000_000_000;
const ERA_SECONDS: i64 = 1 << 32;
const VERSION: u8 = 4;
const CLIENT: u8 = 3;
const SERVER: u8 = 4;
const UNSYNCHRONIZED: u8 = 3;

/// An NTP timestamp: seconds since 1900 (in the era that holds 1968 to 2104) and a binary
/// fraction of a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timestamp {
    /// The seconds.
    pub seconds: u32,
    /// The fraction of a second, in units of 2^-32 s.
    pub fraction: u32,
}

impl Timestamp {
    /// The timestamp of a moment `nanoseconds` after 1970 (UTC).
    pub fn from_unix_nanoseconds(nanoseconds: i64) -> Self {
        let seconds = nanoseconds.div_euclid(NANOSECONDS);
        let rest = nanoseconds.rem_euclid(NANOSECONDS);
        Self {
            seconds: (seconds + UNIX_EPOCH_IN_NTP_SECONDS).rem_euclid(ERA_SECONDS) as u32,
            fraction: ((rest << 32) / NANOSECONDS) as u32,
        }
    }

    /// The moment in nanoseconds after 1970: seconds with the top bit clear are read in the
    /// next era (2036 to 2104), as RFC 4330 says.
    pub fn to_unix_nanoseconds(self) -> i64 {
        let mut seconds = i64::from(self.seconds);
        if self.seconds & 0x8000_0000 == 0 {
            seconds += ERA_SECONDS;
        }
        let fraction = (i64::from(self.fraction) * NANOSECONDS) >> 32;
        (seconds - UNIX_EPOCH_IN_NTP_SECONDS) * NANOSECONDS + fraction
    }

    fn read(bytes: &[u8]) -> Self {
        Self {
            seconds: u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            fraction: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }

    fn write(self, out: &mut [u8]) {
        out[..4].copy_from_slice(&self.seconds.to_be_bytes());
        out[4..8].copy_from_slice(&self.fraction.to_be_bytes());
    }

    fn is_zero(self) -> bool {
        self.seconds == 0 && self.fraction == 0
    }
}

/// Writes a client's request whose transmit timestamp is `transmit`, which the server's reply
/// must carry back.
pub fn write_request(out: &mut [u8; PACKET_BYTES], transmit: Timestamp) {
    out.fill(0);
    out[0] = (VERSION << 3) | CLIENT;
    transmit.write(&mut out[40..]);
}

/// A server's reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reply {
    /// How far the server is from a reference clock: 1 for a server with one of its own.
    pub stratum: u8,
    /// When the request arrived, by the server's clock.
    pub receive: Timestamp,
    /// When the reply left, by the server's clock.
    pub transmit: Timestamp,
}

/// Why a reply is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyError {
    /// It is shorter than an NTP packet.
    Short,
    /// It is not a server's reply.
    NotAServer,
    /// The server says its own clock is not synchronized.
    Unsynchronized,
    /// The server asks to be left alone (a kiss-o'-death, stratum 0).
    Refused,
    /// It does not answer the request sent.
    NotOurs,
    /// It carries no time.
    NoTime,
}

impl ReplyError {
    /// What is wrong, for people.
    pub fn description(self) -> &'static str {
        match self {
            Self::Short => "the reply is too short for NTP",
            Self::NotAServer => "the reply is not an NTP server's",
            Self::Unsynchronized => "the server's own clock is not synchronized",
            Self::Refused => "the server refused to give the time (kiss-o'-death)",
            Self::NotOurs => "the reply does not answer the request sent",
            Self::NoTime => "the reply carries no time",
        }
    }
}

/// Checks `packet` as the reply to a request sent with the transmit timestamp `sent`.
pub fn parse_reply(packet: &[u8], sent: Timestamp) -> Result<Reply, ReplyError> {
    if packet.len() < PACKET_BYTES {
        return Err(ReplyError::Short);
    }
    let leap = packet[0] >> 6;
    let version = (packet[0] >> 3) & 7;
    let mode = packet[0] & 7;
    if mode != SERVER || !(3..=4).contains(&version) {
        return Err(ReplyError::NotAServer);
    }
    let stratum = packet[1];
    if stratum == 0 {
        return Err(ReplyError::Refused);
    }
    if leap == UNSYNCHRONIZED || stratum > 15 {
        return Err(ReplyError::Unsynchronized);
    }
    if Timestamp::read(&packet[24..32]) != sent {
        return Err(ReplyError::NotOurs);
    }
    let receive = Timestamp::read(&packet[32..40]);
    let transmit = Timestamp::read(&packet[40..48]);
    if transmit.is_zero() || receive.is_zero() {
        return Err(ReplyError::NoTime);
    }
    Ok(Reply {
        stratum,
        receive,
        transmit,
    })
}

/// The time when `reply` arrived, in nanoseconds after 1970: the server's transmit time plus
/// half the network's delay, the `round_trip_ns` the client measured less the time the server
/// held the request.
pub fn arrival_time(reply: &Reply, round_trip_ns: i64) -> i64 {
    let transmit = reply.transmit.to_unix_nanoseconds();
    let held = transmit - reply.receive.to_unix_nanoseconds();
    let delay = (round_trip_ns - held.clamp(0, round_trip_ns.max(0))).max(0);
    transmit + delay / 2
}

/// Writes a server's reply to `request` with the given times, as a stratum 1 server: for test
/// servers. `false` when `request` is not a client's request.
pub fn write_reply(
    request: &[u8],
    receive: Timestamp,
    transmit: Timestamp,
    out: &mut [u8; PACKET_BYTES],
) -> bool {
    if request.len() < PACKET_BYTES || request[0] & 7 != CLIENT {
        return false;
    }
    out.fill(0);
    out[0] = (VERSION << 3) | SERVER;
    out[1] = 1;
    out[12..16].copy_from_slice(b"EGG\0");
    out[24..32].copy_from_slice(&request[40..48]);
    receive.write(&mut out[32..]);
    transmit.write(&mut out[40..]);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOMENT: i64 = 1_793_282_709_250_000_000;

    #[test]
    fn timestamps_convert_both_ways_across_the_2036_era_change() {
        let stamp = Timestamp::from_unix_nanoseconds(MOMENT);
        assert_eq!(
            stamp.seconds as i64,
            1_793_282_709 + UNIX_EPOCH_IN_NTP_SECONDS
        );
        assert_eq!(stamp.fraction, 1 << 30);
        assert_eq!(stamp.to_unix_nanoseconds(), MOMENT);
        let epoch = Timestamp::from_unix_nanoseconds(0);
        assert_eq!(epoch.seconds as i64, UNIX_EPOCH_IN_NTP_SECONDS);
        assert_eq!(epoch.to_unix_nanoseconds(), 0);
    }

    #[test]
    fn a_reply_answers_its_request_and_gives_the_time_at_arrival() {
        let sent = Timestamp {
            seconds: 0xe000_0000,
            fraction: 77,
        };
        let mut request = [0u8; PACKET_BYTES];
        write_request(&mut request, sent);
        assert_eq!(request[0], 0x23);
        let server_time = Timestamp::from_unix_nanoseconds(MOMENT);
        let mut reply = [0u8; PACKET_BYTES];
        assert!(write_reply(&request, server_time, server_time, &mut reply));
        let parsed = parse_reply(&reply, sent).unwrap();
        assert_eq!(parsed.stratum, 1);
        assert_eq!(arrival_time(&parsed, 40_000_000), MOMENT + 20_000_000);
        assert_eq!(
            parse_reply(&reply, Timestamp::default()),
            Err(ReplyError::NotOurs)
        );
        assert_eq!(parse_reply(&reply[..47], sent), Err(ReplyError::Short));
        let mut refused = reply;
        refused[1] = 0;
        assert_eq!(parse_reply(&refused, sent), Err(ReplyError::Refused));
        let mut unsynchronized = reply;
        unsynchronized[0] |= 0xc0;
        assert_eq!(
            parse_reply(&unsynchronized, sent),
            Err(ReplyError::Unsynchronized)
        );
        assert_eq!(parse_reply(&request, sent), Err(ReplyError::NotAServer));
        assert!(!write_reply(
            &reply,
            server_time,
            server_time,
            &mut [0; PACKET_BYTES]
        ));
    }

    #[test]
    fn the_time_the_server_held_the_request_is_not_network_delay() {
        let receive = Timestamp::from_unix_nanoseconds(MOMENT);
        let transmit = Timestamp::from_unix_nanoseconds(MOMENT + 10_000_000);
        let reply = Reply {
            stratum: 2,
            receive,
            transmit,
        };
        assert_eq!(
            arrival_time(&reply, 30_000_000),
            MOMENT + 10_000_000 + 10_000_000
        );
        assert_eq!(arrival_time(&reply, 5_000_000), MOMENT + 10_000_000);
    }
}
