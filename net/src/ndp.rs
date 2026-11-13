//! Neighbour discovery's state for one interface (RFC 4861, RFC 4862, D565): the neighbour
//! cache that maps IPv6 addresses to link-layer ones, and the addresses the interface makes
//! itself: its link-local address from its MAC address, and a global one in the prefix a
//! router advertises for autoconfiguration (SLAAC), with that router as the default route.

use crate::ethernet::MacAddress;
use crate::icmpv6::RouterAdvertisement;
use crate::ipv6::Ipv6Address;

/// How many neighbours the cache holds; a new one replaces the oldest.
pub const CACHE_ENTRIES: usize = 16;
/// How long an entry is trusted, as ARP's.
pub const ENTRY_LIFETIME_MS: u64 = 60_000;
/// How many router solicitations the interface sends at start (RFC 4861's
/// `MAX_RTR_SOLICITATIONS`).
pub const ROUTER_SOLICITATIONS: u8 = 3;
/// How long between them (`RTR_SOLICITATION_INTERVAL`).
pub const ROUTER_SOLICITATION_INTERVAL_MS: u64 = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    address: Ipv6Address,
    link: MacAddress,
    learned_ms: u64,
}

/// The neighbour cache.
#[derive(Debug, Clone, Default)]
pub struct Cache {
    entries: [Option<Entry>; CACHE_ENTRIES],
}

impl Cache {
    /// The link-layer address of `address`, while its entry is fresh at `now_ms`.
    pub fn lookup(&self, address: Ipv6Address, now_ms: u64) -> Option<MacAddress> {
        self.entries.iter().flatten().find_map(|entry| {
            (entry.address == address
                && now_ms.saturating_sub(entry.learned_ms) < ENTRY_LIFETIME_MS)
                .then_some(entry.link)
        })
    }

    /// Learns that `address` is at `link`.
    pub fn learn(&mut self, address: Ipv6Address, link: MacAddress, now_ms: u64) {
        let entry = Entry {
            address,
            link,
            learned_ms: now_ms,
        };
        let slot = self
            .entries
            .iter()
            .position(|slot| slot.is_some_and(|known| known.address == address))
            .or_else(|| self.entries.iter().position(Option::is_none))
            .unwrap_or_else(|| {
                self.entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, slot)| slot.map_or(0, |entry| entry.learned_ms))
                    .map_or(0, |(index, _)| index)
            });
        self.entries[slot] = Some(entry);
    }

    /// The fresh entries at `now_ms`.
    pub fn entries(&self, now_ms: u64) -> impl Iterator<Item = (Ipv6Address, MacAddress)> + '_ {
        self.entries.iter().flatten().filter_map(move |entry| {
            (now_ms.saturating_sub(entry.learned_ms) < ENTRY_LIFETIME_MS)
                .then_some((entry.address, entry.link))
        })
    }
}

/// The interface's IPv6 addresses and router.
#[derive(Debug, Clone)]
pub struct Addresses {
    /// The link-local address.
    pub link_local: Ipv6Address,
    /// The global address made in the advertised prefix, with its prefix length and when it
    /// stops being valid.
    pub global: Option<(Ipv6Address, u8, u64)>,
    /// The default router and when it stops being one.
    pub router: Option<(Ipv6Address, u64)>,
    /// The MTU the router advertised.
    pub mtu: Option<u32>,
    solicitations_sent: u8,
    next_solicitation_ms: u64,
}

impl Addresses {
    /// The addresses of an interface with `mac`: its link-local one, and none other until a
    /// router advertises.
    pub fn new(mac: MacAddress) -> Self {
        Self {
            link_local: Ipv6Address::link_local_of(mac),
            global: None,
            router: None,
            mtu: None,
            solicitations_sent: 0,
            next_solicitation_ms: 0,
        }
    }

    /// Whether `address` is one of its own, or a multicast group it listens to (all nodes and
    /// its solicited-node groups).
    pub fn is_ours(&self, address: Ipv6Address) -> bool {
        address == self.link_local
            || self.global.is_some_and(|(global, _, _)| global == address)
            || address == Ipv6Address::ALL_NODES
            || address == self.link_local.solicited_node()
            || self
                .global
                .is_some_and(|(global, _, _)| global.solicited_node() == address)
    }

    /// The address to send to `destination` from: the global one off the link-local scope,
    /// the link-local one otherwise.
    pub fn source_for(&self, destination: Ipv6Address, now_ms: u64) -> Ipv6Address {
        match self.global {
            Some((global, _, until)) if !destination.is_link_local() && now_ms < until => global,
            _ => self.link_local,
        }
    }

    /// Whether `destination` is on the link: link-local, multicast, or in the global prefix.
    pub fn is_on_link(&self, destination: Ipv6Address) -> bool {
        destination.is_link_local()
            || destination.is_multicast()
            || self
                .global
                .is_some_and(|(global, prefix, _)| destination.in_prefix(global, prefix))
    }

    /// The default router, while it is one at `now_ms`.
    pub fn router(&self, now_ms: u64) -> Option<Ipv6Address> {
        self.router
            .filter(|&(_, until)| now_ms < until)
            .map(|(router, _)| router)
    }

    /// Takes in `advertisement`, which came from `router` at `now_ms`, for an interface with
    /// `mac`: the router becomes the default one for its lifetime, and an autonomous prefix of
    /// 64 bits gives the interface its global address.
    pub fn take_advertisement(
        &mut self,
        router: Ipv6Address,
        advertisement: &RouterAdvertisement,
        mac: MacAddress,
        now_ms: u64,
    ) {
        if !router.is_link_local() {
            return;
        }
        self.router = match advertisement.lifetime_seconds {
            0 => None,
            seconds => Some((router, now_ms.saturating_add(u64::from(seconds) * 1000))),
        };
        if let Some(mtu) = advertisement.mtu.filter(|&mtu| mtu >= crate::ipv6::MIN_MTU as u32) {
            self.mtu = Some(mtu);
        }
        if let Some(prefix) = advertisement.prefix
            && prefix.autonomous
            && prefix.length == 64
            && !prefix.prefix.is_link_local()
        {
            let address = Ipv6Address::with_interface_of(prefix.prefix, mac);
            let until = now_ms.saturating_add(u64::from(prefix.valid_seconds) * 1000);
            self.global = (prefix.valid_seconds > 0).then_some((address, 64, until));
        }
        self.solicitations_sent = ROUTER_SOLICITATIONS;
    }

    /// Whether a router solicitation is due at `now_ms`; counts it as sent.
    pub fn solicitation_due(&mut self, now_ms: u64) -> bool {
        if self.solicitations_sent >= ROUTER_SOLICITATIONS || now_ms < self.next_solicitation_ms {
            return false;
        }
        self.solicitations_sent += 1;
        self.next_solicitation_ms = now_ms.saturating_add(ROUTER_SOLICITATION_INTERVAL_MS);
        true
    }

    /// When the next router solicitation is due, `u64::MAX` for none.
    pub fn next_solicitation_ms(&self) -> u64 {
        match self.solicitations_sent < ROUTER_SOLICITATIONS {
            true => self.next_solicitation_ms,
            false => u64::MAX,
        }
    }

    /// Starts asking for routers again: the link came up.
    pub fn restart(&mut self) {
        self.solicitations_sent = 0;
        self.next_solicitation_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icmpv6::Prefix;

    #[test]
    fn an_advertised_prefix_gives_a_global_address_and_a_default_router() {
        let mac = MacAddress([0x52, 0x54, 0, 0x12, 0x34, 0x56]);
        let mut addresses = Addresses::new(mac);
        assert!(addresses.solicitation_due(0));
        assert!(!addresses.solicitation_due(10));
        let router = Ipv6Address::parse(b"fe80::2").unwrap();
        let advertisement = RouterAdvertisement {
            lifetime_seconds: 1800,
            link: None,
            mtu: Some(1500),
            prefix: Some(Prefix {
                prefix: Ipv6Address::parse(b"fec0::").unwrap(),
                length: 64,
                on_link: true,
                autonomous: true,
                valid_seconds: 86_400,
            }),
        };
        addresses.take_advertisement(router, &advertisement, mac, 100);
        let global = Ipv6Address::parse(b"fec0::5054:ff:fe12:3456").unwrap();
        assert_eq!(addresses.global.map(|(address, _, _)| address), Some(global));
        assert_eq!(addresses.router(200), Some(router));
        let outside = Ipv6Address::parse(b"2001:db8::1").unwrap();
        assert_eq!(addresses.source_for(outside, 200), global);
        assert!(!addresses.is_on_link(outside));
        assert!(addresses.is_ours(global.solicited_node()));
    }
}
