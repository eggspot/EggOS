//! ARP for IPv4 over Ethernet (RFC 826): the packets that ask which hardware address has an
//! IPv4 address and answer, and the cache of the answers.

use crate::ethernet::MacAddress;
use crate::ipv4::Ipv4Address;

/// Bytes of an ARP packet for IPv4 over Ethernet.
pub const PACKET_BYTES: usize = 28;
/// How many answers the cache keeps; a new one replaces the oldest.
pub const CACHE_ENTRIES: usize = 16;
/// How long an answer is believed, as Linux's default `base_reachable_time` rounds it.
pub const ENTRY_LIFETIME_MS: u64 = 60_000;

/// The operations.
pub mod operation {
    /// Who has this address?
    pub const REQUEST: u16 = 1;
    /// This station has it.
    pub const REPLY: u16 = 2;
}

const HARDWARE_ETHERNET: u16 = 1;
const PROTOCOL_IPV4: u16 = 0x0800;

/// An ARP packet for IPv4 over Ethernet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    /// [`operation::REQUEST`] or [`operation::REPLY`].
    pub operation: u16,
    /// The sender's hardware address.
    pub sender_mac: MacAddress,
    /// The sender's IPv4 address.
    pub sender_ip: Ipv4Address,
    /// The target's hardware address (zero in a request).
    pub target_mac: MacAddress,
    /// The target's IPv4 address.
    pub target_ip: Ipv4Address,
}

/// The packet at the start of `bytes`; `None` unless it is IPv4 over Ethernet.
pub fn parse(bytes: &[u8]) -> Option<Packet> {
    let bytes = bytes.first_chunk::<PACKET_BYTES>()?;
    let word = |at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]);
    if word(0) != HARDWARE_ETHERNET || word(2) != PROTOCOL_IPV4 || bytes[4] != 6 || bytes[5] != 4
    {
        return None;
    }
    let mac = |at: usize| {
        let mut address = [0u8; 6];
        address.copy_from_slice(&bytes[at..at + 6]);
        MacAddress(address)
    };
    let ip = |at: usize| Ipv4Address([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Some(Packet {
        operation: word(6),
        sender_mac: mac(8),
        sender_ip: ip(14),
        target_mac: mac(18),
        target_ip: ip(24),
    })
}

/// Writes `packet` to the start of `out`: [`PACKET_BYTES`], or 0 when `out` is shorter.
pub fn write(out: &mut [u8], packet: &Packet) -> usize {
    let Some(bytes) = out.first_chunk_mut::<PACKET_BYTES>() else {
        return 0;
    };
    bytes[0..2].copy_from_slice(&HARDWARE_ETHERNET.to_be_bytes());
    bytes[2..4].copy_from_slice(&PROTOCOL_IPV4.to_be_bytes());
    bytes[4] = 6;
    bytes[5] = 4;
    bytes[6..8].copy_from_slice(&packet.operation.to_be_bytes());
    bytes[8..14].copy_from_slice(&packet.sender_mac.0);
    bytes[14..18].copy_from_slice(&packet.sender_ip.0);
    bytes[18..24].copy_from_slice(&packet.target_mac.0);
    bytes[24..28].copy_from_slice(&packet.target_ip.0);
    PACKET_BYTES
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    address: Ipv4Address,
    mac: MacAddress,
    updated_ms: u64,
}

/// The answers heard, each believed for [`ENTRY_LIFETIME_MS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    entries: [Option<Entry>; CACHE_ENTRIES],
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

impl Cache {
    /// An empty cache.
    pub const fn new() -> Self {
        Self {
            entries: [None; CACHE_ENTRIES],
        }
    }

    fn is_live(entry: &Entry, now_ms: u64) -> bool {
        now_ms.saturating_sub(entry.updated_ms) < ENTRY_LIFETIME_MS
    }

    /// The hardware address `address` has, if an answer for it is still believed at `now_ms`.
    pub fn lookup(&self, address: Ipv4Address, now_ms: u64) -> Option<MacAddress> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.address == address && Self::is_live(entry, now_ms))
            .map(|entry| entry.mac)
    }

    /// Records that `address` has `mac` at `now_ms`, in its own entry, a free or stale one, or
    /// the oldest.
    pub fn update(&mut self, address: Ipv4Address, mac: MacAddress, now_ms: u64) {
        let entry = Entry {
            address,
            mac,
            updated_ms: now_ms,
        };
        let slot = match self
            .entries
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.address == address))
        {
            Some(index) => index,
            None => self
                .entries
                .iter()
                .position(|slot| slot.is_none_or(|entry| !Self::is_live(&entry, now_ms)))
                .unwrap_or_else(|| self.oldest()),
        };
        self.entries[slot] = Some(entry);
    }

    fn oldest(&self) -> usize {
        self.entries
            .iter()
            .enumerate()
            .min_by_key(|(_, slot)| slot.map_or(0, |entry| entry.updated_ms))
            .map_or(0, |(index, _)| index)
    }

    /// Updates `address`'s entry if there is one (RFC 826's merge); whether there was.
    pub fn refresh(&mut self, address: Ipv4Address, mac: MacAddress, now_ms: u64) -> bool {
        let known = self
            .entries
            .iter()
            .any(|slot| slot.is_some_and(|entry| entry.address == address));
        if known {
            self.update(address, mac, now_ms);
        }
        known
    }

    /// The answers still believed at `now_ms`.
    pub fn entries(&self, now_ms: u64) -> impl Iterator<Item = (Ipv4Address, MacAddress)> + '_ {
        self.entries
            .iter()
            .flatten()
            .filter(move |entry| Self::is_live(entry, now_ms))
            .map(|entry| (entry.address, entry.mac))
    }

    /// Forgets every answer.
    pub fn clear(&mut self) {
        self.entries = [None; CACHE_ENTRIES];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(last: u8) -> Ipv4Address {
        Ipv4Address::new(10, 0, 2, last)
    }

    fn mac(last: u8) -> MacAddress {
        MacAddress([0x52, 0x55, 0x0a, 0, 2, last])
    }

    #[test]
    fn a_packet_round_trips_and_other_hardware_is_refused() {
        let packet = Packet {
            operation: operation::REQUEST,
            sender_mac: mac(15),
            sender_ip: host(15),
            target_mac: MacAddress::ZERO,
            target_ip: host(2),
        };
        let mut bytes = [0u8; PACKET_BYTES];
        assert_eq!(write(&mut bytes, &packet), PACKET_BYTES);
        assert_eq!(parse(&bytes), Some(packet));
        let mut token_ring = bytes;
        token_ring[1] = 6;
        assert_eq!(parse(&token_ring), None);
        assert_eq!(parse(&bytes[..27]), None);
        assert_eq!(write(&mut [0u8; 27], &packet), 0);
    }

    #[test]
    fn answers_expire_refresh_and_the_oldest_makes_room() {
        let mut cache = Cache::new();
        cache.update(host(2), mac(2), 1000);
        assert_eq!(cache.lookup(host(2), 1000 + ENTRY_LIFETIME_MS - 1), Some(mac(2)));
        assert_eq!(cache.lookup(host(2), 1000 + ENTRY_LIFETIME_MS), None);
        assert!(!cache.refresh(host(3), mac(3), 2000));
        assert_eq!(cache.lookup(host(3), 2000), None);
        assert!(cache.refresh(host(2), mac(9), 2000));
        assert_eq!(cache.lookup(host(2), 2000), Some(mac(9)));
        for last in 10..10 + CACHE_ENTRIES as u8 {
            cache.update(host(last), mac(last), 3000 + u64::from(last));
        }
        assert_eq!(cache.lookup(host(2), 3100), None);
        assert_eq!(cache.entries(3100).count(), CACHE_ENTRIES);
        cache.update(host(100), mac(100), 4000);
        assert_eq!(cache.lookup(host(10), 4000), None);
        assert_eq!(cache.lookup(host(100), 4000), Some(mac(100)));
        cache.clear();
        assert_eq!(cache.entries(4000).count(), 0);
    }
}
