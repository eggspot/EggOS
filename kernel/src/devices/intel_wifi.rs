//! Intel's Wi-Fi 6 and 6E adapters on PCI (D274; the driver is `eggos_drivers::wifi::iwx`,
//! D272): the probe finds the adapter and keeps its registers; once the file systems are mounted
//! a kernel thread waits for the `wifi-firmware` component's file, powers the adapter up and
//! hands it the firmware. What it reached shows in `/proc/driver/wifi`.

use super::dma::DmaRegion;
use super::interrupts::DeviceEvents;
use super::{
    Deadline, DeviceId, DmaCoherence, DriverDescriptor, Identity, Match, Probe, ProbeError,
    ProbePass,
};
use crate::linux::exec::ProgramBytes;
use crate::sync::{LeafSpinLock, SpinLock};
use crate::vfs::{self, NodeKind};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use core::time::Duration;
use eggos_drivers::dma::DmaAllocator;
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_drivers::wifi::iwx::devices::{self, Adapter, VENDOR_INTEL};
use eggos_drivers::wifi::iwx::firmware::Firmware;
use eggos_drivers::wifi::iwx::nvm;
use eggos_drivers::wifi::iwx::scan;
use eggos_drivers::wifi::iwx::station::{self, Link, Target};
use eggos_drivers::wifi::iwx::transport::{self, Transport, Wait};
use eggos_hal::Platform;
use eggos_wifi::client;
use eggos_wifi::frames::{self as wifi_frames, Network, Security};
use eggos_wifi::handshake::{self, Akm, Keys, Supplicant};
use eggos_wifi::sae::{self, Sae};

const NETWORK_CONTROLLER: u8 = 0x02;
const OTHER_NETWORK: u8 = 0x80;
const SUBSYSTEM_IDS: u16 = 0x2c;
const HW_REV: usize = 0x28;
const LARGEST_FIRMWARE: usize = 4 << 20;
const FIRMWARE_RETRY: Duration = Duration::from_secs(10);
const LOAD_TIMEOUT_MS: u64 = 1000;
const START_TIMEOUT_MS: u64 = 2000;
const INIT_TIMEOUT_MS: u64 = 4000;
const SCAN_TIMEOUT_MS: u64 = 15_000;
const SCAN_INTERVAL_MS: u64 = 30_000;
const ANSWER_TIMEOUT_MS: u64 = 2000;
const HANDSHAKE_TIMEOUT_MS: u64 = 5000;
const SAE_COMMIT_TRIES: usize = 3;
/// The file that names the network to join (D278): `ssid=<name>` and `passphrase=<words>` lines.
pub const SETTINGS_PATH: &str = "/var/lib/eggos/wifi.conf";
const LARGEST_SETTINGS: usize = 4096;

static CONNECTED: SpinLock<Option<alloc::string::String>> = SpinLock::new(None);
static JOINING: SpinLock<Option<alloc::string::String>> = SpinLock::new(None);
static FAILURE: SpinLock<Option<Failure>> = SpinLock::new(None);
static SIGNAL: AtomicI32 = AtomicI32::new(0);
static LINK_UP: AtomicBool = AtomicBool::new(false);
static TRAFFIC: LeafSpinLock<Traffic> = LeafSpinLock::new_leaf(Traffic {
    incoming: VecDeque::new(),
    outgoing: VecDeque::new(),
});
static WIFI_EVENTS: DeviceEvents = DeviceEvents::new();

const ETHERNET_HEADER_BYTES: usize = 14;
const WIFI_MTU: usize = 1500;
const MAX_PENDING_FRAMES: usize = 64;
const KEY_ID_SHIFT: u32 = 6;
const LEAVING: u16 = 3;
const SETTINGS_CHECK_MS: u64 = 1000;
const CONNECTED_PUMP_MS: u64 = 2;
const RETRY_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Failure {
    code: &'static str,
    sentence: &'static str,
}

impl Failure {
    const fn of(code: &'static str, sentence: &'static str) -> Self {
        Self { code, sentence }
    }

    fn adapter(sentence: &'static str) -> Self {
        Self::of("adapter", sentence)
    }
}

const NOT_IN_RANGE: Failure = Failure::of("not-in-range", "the network is not in range");
const NO_ANSWER: Failure = Failure::of("no-answer", "the network did not answer");
const WRONG_PASSPHRASE: Failure = Failure::of(
    "wrong-passphrase",
    "the network did not accept the passphrase",
);
const REFUSED: Failure = Failure::of("refused", "the network refused the station");
const UNSUPPORTED: Failure = Failure::of(
    "unsupported-security",
    "the network's security is not WPA3-Personal, WPA2-Personal or open",
);
const DROPPED: Failure = Failure::of("dropped", "the network ended the connection");

struct Traffic {
    incoming: VecDeque<Vec<u8>>,
    outgoing: VecDeque<Vec<u8>>,
}

struct Connection {
    link: Link<DmaRegion>,
    keys: Option<Keys>,
    management_protected: bool,
    supplicant: Option<Supplicant>,
}

struct WifiDevice<P: Platform> {
    address: [u8; 6],
    platform: core::marker::PhantomData<fn() -> P>,
}

impl<P: Platform> crate::net::NetDevice for WifiDevice<P> {
    fn mac(&self) -> eggos_net::ethernet::MacAddress {
        eggos_net::ethernet::MacAddress(self.address)
    }

    fn mtu(&self) -> usize {
        WIFI_MTU
    }

    fn is_link_up(&self) -> bool {
        LINK_UP.load(Ordering::Acquire)
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        let next = TRAFFIC.lock().incoming.pop_front()?;
        let length = next.len().min(frame.len());
        frame[..length].copy_from_slice(&next[..length]);
        Some(length)
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        if frame.len() < ETHERNET_HEADER_BYTES {
            return Err("a frame shorter than its Ethernet header");
        }
        if !LINK_UP.load(Ordering::Acquire) {
            return Err("the Wi-Fi station has joined no network");
        }
        {
            let mut traffic = TRAFFIC.lock();
            if traffic.outgoing.len() >= MAX_PENDING_FRAMES {
                return Err("the Wi-Fi transmit queue is full");
            }
            traffic.outgoing.push_back(frame.to_vec());
        }
        WIFI_EVENTS.raise::<P>();
        Ok(())
    }
}

struct Joined {
    link: Link<DmaRegion>,
    keys: Option<Keys>,
    ssid: Vec<u8>,
    passphrase: Vec<u8>,
    packet_number: u64,
    last_unicast: u64,
    last_group: u64,
    management_protected: bool,
    last_management: u64,
    supplicant: Option<Supplicant>,
}

enum Received {
    Ethernet(Vec<u8>),
    Eapol(Vec<u8>),
}

impl Joined {
    fn rekey(&mut self, eapol: &[u8]) -> Option<Vec<u8>> {
        let answer = self.supplicant.as_mut()?.receive_group(eapol).ok()?;
        let fresh = answer.keys?;
        let keys = self.keys.as_mut()?;
        keys.gtk = fresh.gtk;
        keys.gtk_bytes = fresh.gtk_bytes;
        keys.gtk_index = fresh.gtk_index;
        if fresh.igtk.is_some() {
            keys.igtk = fresh.igtk;
        }
        self.last_group = 0;
        let (own, bssid) = (self.link.address(), self.link.target().bssid);
        let mut ethernet = Vec::with_capacity(ETHERNET_HEADER_BYTES + answer.length);
        ethernet.extend_from_slice(&bssid);
        ethernet.extend_from_slice(&own);
        ethernet.extend_from_slice(&client::ETHERTYPE_EAPOL.to_be_bytes());
        ethernet.extend_from_slice(&answer.frame[..answer.length]);
        self.wire_frame(&ethernet).ok()
    }

    fn ended(&mut self, frame: &[u8]) -> bool {
        let (own, bssid) = (self.link.address(), self.link.target().bssid);
        if !self.management_protected {
            return client::ended_by(frame, own, bssid).is_some();
        }
        let Some(keys) = &self.keys else {
            return false;
        };
        if !client::is_protected_ending(frame)
            || frame.get(4..10) != Some(&own[..])
            || frame.get(10..16) != Some(&bssid[..])
        {
            return false;
        }
        let mut body = [0u8; 64];
        let Ok((length, number)) = eggos_wifi::protection::decrypt(&keys.tk, frame, &mut body)
        else {
            return false;
        };
        if length < 2 || (number <= self.last_management && self.last_management != 0) {
            return false;
        }
        self.last_management = number;
        true
    }

    fn received(&mut self, frame: &[u8]) -> Option<Received> {
        let header = client::data_header_bytes(frame)?;
        let bssid = self.link.target().bssid;
        let mut body = alloc::vec![0u8; frame.len()];
        let body = match (client::is_protected_data(frame), &self.keys) {
            (true, Some(keys)) => {
                let group = frame.get(4).is_some_and(|first| first & 1 != 0);
                let key_id = frame.get(header + 3).map_or(0, |byte| byte >> KEY_ID_SHIFT);
                let key: [u8; 16] = match group {
                    true if key_id == keys.gtk_index && keys.gtk_bytes >= 16 => {
                        keys.gtk[..16].try_into().ok()?
                    }
                    true => return None,
                    false => keys.tk,
                };
                let (length, number) =
                    eggos_wifi::protection::decrypt(&key, frame, &mut body).ok()?;
                let last = match group {
                    true => &mut self.last_group,
                    false => &mut self.last_unicast,
                };
                if number <= *last && *last != 0 {
                    return None;
                }
                *last = number;
                &body[..length]
            }
            (true, None) => return None,
            (false, Some(_)) => return None,
            (false, None) => frame.get(header..)?,
        };
        let (destination, source, ethertype, payload) = client::ethernet_from(frame, body, bssid)?;
        if ethertype == client::ETHERTYPE_EAPOL {
            return Some(Received::Eapol(payload));
        }
        let mut ethernet = Vec::with_capacity(ETHERNET_HEADER_BYTES + payload.len());
        ethernet.extend_from_slice(&destination);
        ethernet.extend_from_slice(&source);
        ethernet.extend_from_slice(&ethertype.to_be_bytes());
        ethernet.extend_from_slice(&payload);
        Some(Received::Ethernet(ethernet))
    }

    fn wire_frame(&mut self, frame: &[u8]) -> Result<Vec<u8>, &'static str> {
        let destination: [u8; 6] = frame[..6]
            .try_into()
            .map_err(|_| "a damaged Ethernet frame")?;
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
        let own = self.link.address();
        let bssid = self.link.target().bssid;
        let plain = client::data_frame(
            own,
            bssid,
            destination,
            ethertype,
            &frame[ETHERNET_HEADER_BYTES..],
        );
        let Some(keys) = &self.keys else {
            return Ok(plain);
        };
        self.packet_number += 1;
        let header = client::data_header_bytes(&plain).ok_or("not a data frame")?;
        let mut out = alloc::vec![0u8; plain.len() + eggos_wifi::protection::CCMP_HEADER_BYTES + 8];
        let length = eggos_wifi::protection::encrypt(
            &keys.tk,
            &plain[..header],
            self.packet_number,
            0,
            &plain[header..],
            &mut out,
        )
        .map_err(|_| "the frame could not be protected")?;
        out.truncate(length);
        Ok(out)
    }
}

/// The network the station is connected to, if any.
pub fn connected_network() -> Option<alloc::string::String> {
    CONNECTED.lock().clone()
}

fn wanted_hidden() -> bool {
    let root = vfs::root();
    let Ok(node) = vfs::resolve(root, root, SETTINGS_PATH.as_bytes()) else {
        return false;
    };
    let size = node.attributes().size;
    if node.kind() != NodeKind::Regular || size > LARGEST_SETTINGS as u64 {
        return false;
    }
    let Ok(bytes) = ProgramBytes::read(node, size, size) else {
        return false;
    };
    bytes
        .bytes()
        .split(|&byte| byte == b'\n')
        .any(|line| line == b"hidden=yes")
}

/// `name` as `/proc/driver/wifi` writes it (D298): printable ASCII as it is but `\`, the
/// rest, and a space at either end, as `\xHH`, so a name with any bytes reads back the same.
pub fn escaped_name(name: &[u8]) -> alloc::string::String {
    let mut text = alloc::string::String::new();
    for (index, &byte) in name.iter().enumerate() {
        let edge_space = byte == b' ' && (index == 0 || index + 1 == name.len());
        match byte {
            b'\\' => text.push_str("\\\\"),
            0x21..=0x7e => text.push(byte as char),
            b' ' if !edge_space => text.push(' '),
            _ => text.push_str(&alloc::format!("\\x{byte:02x}")),
        }
    }
    text
}

fn wanted() -> Option<(Vec<u8>, Vec<u8>)> {
    let root = vfs::root();
    let node = vfs::resolve(root, root, SETTINGS_PATH.as_bytes()).ok()?;
    let size = node.attributes().size;
    if node.kind() != NodeKind::Regular || size > LARGEST_SETTINGS as u64 {
        return None;
    }
    let bytes = ProgramBytes::read(node, size, size).ok()?;
    let (mut ssid, mut passphrase) = (None, Vec::new());
    for line in bytes.bytes().split(|&byte| byte == b'\n') {
        if let Some(value) = line.strip_prefix(b"ssid=") {
            ssid = Some(value.to_vec());
        } else if let Some(value) = line.strip_prefix(b"passphrase=") {
            passphrase = value.to_vec();
        }
    }
    Some((ssid?, passphrase))
}

struct Awaited {
    own: [u8; 6],
    bssid: [u8; 6],
    ax210: bool,
    milliseconds: u64,
    timed_out: Failure,
    sent_away: Failure,
}

fn wait_frame<P: Platform, T>(
    transport: &mut Transport<Mmio, DmaRegion>,
    awaited: Awaited,
    mut wanted: impl FnMut(&[u8]) -> Option<T>,
) -> Result<T, Failure> {
    let Awaited {
        own,
        bssid,
        ax210,
        milliseconds,
        timed_out,
        sent_away,
    } = awaited;
    let mut wait = Pauses::<P>::within(milliseconds);
    loop {
        for packet in transport.take_notifications() {
            if packet.code != scan::RX_MPDU {
                continue;
            }
            let Some(heard) = scan::received_frame(&packet.payload, ax210) else {
                continue;
            };
            if client::ended_by(&heard.frame, own, bssid).is_some() {
                return Err(sent_away);
            }
            if let Some(found) = wanted(&heard.frame) {
                return Ok(found);
            }
        }
        if !wait.keep_waiting() {
            return Err(timed_out);
        }
        wait.pause(1000);
    }
}

fn eapol_from(frame: &[u8], bssid: [u8; 6]) -> Option<Vec<u8>> {
    let header = client::data_header_bytes(frame)?;
    if client::is_protected_data(frame) {
        return None;
    }
    let (_, _, ethertype, payload) = client::ethernet_from(frame, frame.get(header..)?, bssid)?;
    (ethertype == client::ETHERTYPE_EAPOL).then_some(payload)
}

fn connect<P: Platform>(
    transport: &mut Transport<Mmio, DmaRegion>,
    firmware: &Firmware,
    adapter: &Adapter,
    nvm: &nvm::Nvm,
    seen: &Seen,
    passphrase: &[u8],
) -> Result<Connection, Failure> {
    let network = seen.network;
    let channel = nvm
        .channels
        .iter()
        .find(|channel| Some(channel.number) == network.channel)
        .copied()
        .ok_or(Failure::of(
            "channel",
            "the network's channel is not one the adapter may use",
        ))?;
    let akm = match network.security {
        Security::Open => None,
        Security::Wpa2Personal => Some(Akm::Psk),
        Security::Wpa3Personal => Some(Akm::Sae),
        _ => return Err(UNSUPPORTED),
    };
    let rsn: Option<&[u8]> = match akm {
        None => None,
        Some(Akm::Psk) => Some(&wifi_frames::WPA2_PERSONAL_RSN),
        Some(Akm::Sae) => Some(&wifi_frames::WPA3_PERSONAL_RSN),
    };
    let (own, bssid) = (nvm.address, network.bssid);
    let ax210 = adapter.family == devices::Family::Ax210;
    let answer = |timed_out: Failure, sent_away: Failure, milliseconds: u64| Awaited {
        own,
        bssid,
        ax210,
        milliseconds,
        timed_out,
        sent_away,
    };
    let target = Target {
        bssid,
        channel,
        beacon_interval: seen.beacon_interval,
    };
    let joining_failed = |error: station::JoinError| Failure::adapter(error.description());
    let mut link = station::join(
        transport,
        firmware,
        adapter,
        own,
        target,
        &mut Regions,
        &mut Pauses::<P>::within(INIT_TIMEOUT_MS),
    )
    .map_err(joining_failed)?;
    let mut attempt = || -> Result<(Option<Keys>, Option<Supplicant>), Failure> {
        let mut sae_pmk = None;
        if akm == Some(Akm::Sae) {
            let mut random = |out: &mut [u8]| crate::random::fill::<P>(out);
            let mut exchange = Sae::new(passphrase, own, bssid, &mut random)
                .map_err(|_| Failure::adapter("SAE could not start"))?;
            let mut token: Vec<u8> = Vec::new();
            let mut committed = false;
            for _ in 0..SAE_COMMIT_TRIES {
                link.send(
                    transport,
                    &client::sae_authentication(own, bssid, 1, 0, &exchange.commit_body(&token)),
                )
                .map_err(joining_failed)?;
                let (_, status, body) = wait_frame::<P, _>(
                    transport,
                    answer(NO_ANSWER, REFUSED, ANSWER_TIMEOUT_MS),
                    |frame| {
                        client::sae_answer(frame, bssid).filter(|(sequence, _, _)| *sequence == 1)
                    },
                )?;
                if status == sae::ANTI_CLOGGING_TOKEN_REQUIRED {
                    token = body.get(2..).unwrap_or_default().to_vec();
                    continue;
                }
                if status != 0 {
                    return Err(REFUSED);
                }
                exchange.receive_commit(&body).map_err(|_| REFUSED)?;
                committed = true;
                break;
            }
            if !committed {
                return Err(REFUSED);
            }
            let confirm = exchange.confirm_body(1).ok_or(REFUSED)?;
            link.send(
                transport,
                &client::sae_authentication(own, bssid, 2, 0, &confirm),
            )
            .map_err(joining_failed)?;
            let (_, status, body) = wait_frame::<P, _>(
                transport,
                answer(WRONG_PASSPHRASE, WRONG_PASSPHRASE, ANSWER_TIMEOUT_MS),
                |frame| client::sae_answer(frame, bssid).filter(|(sequence, _, _)| *sequence == 2),
            )?;
            if status != 0 {
                return Err(WRONG_PASSPHRASE);
            }
            let (pmk, _) = exchange
                .receive_confirm(&body)
                .map_err(|_| WRONG_PASSPHRASE)?;
            sae_pmk = Some(pmk);
        } else {
            link.send(transport, &client::authentication_request(own, bssid))
                .map_err(joining_failed)?;
            let status = wait_frame::<P, _>(
                transport,
                answer(NO_ANSWER, REFUSED, ANSWER_TIMEOUT_MS),
                |frame| client::authentication_status(frame, bssid),
            )?;
            if status != 0 {
                return Err(REFUSED);
            }
        }
        let request = client::association_request(own, bssid, network.ssid(), rsn);
        link.send(transport, &request).map_err(joining_failed)?;
        let (status, association) = wait_frame::<P, _>(
            transport,
            answer(NO_ANSWER, REFUSED, ANSWER_TIMEOUT_MS),
            |frame| client::association_answer(frame, bssid),
        )?;
        if status != 0 {
            return Err(REFUSED);
        }
        station::associated(
            transport,
            &link,
            association,
            &mut Pauses::<P>::within(INIT_TIMEOUT_MS),
        )
        .map_err(joining_failed)?;
        let Some(akm) = akm else {
            return Ok((None, None));
        };
        let pmk = match sae_pmk {
            Some(pmk) => pmk,
            None => handshake::pmk_from_passphrase(passphrase, network.ssid())
                .ok_or(WRONG_PASSPHRASE)?,
        };
        let mut nonce = [0u8; handshake::NONCE_BYTES];
        crate::random::fill::<P>(&mut nonce);
        let mut supplicant = Supplicant::for_akm(akm, pmk, own, bssid, nonce);
        let mut answered = false;
        loop {
            let timed_out = if answered {
                WRONG_PASSPHRASE
            } else {
                NO_ANSWER
            };
            let message = wait_frame::<P, _>(
                transport,
                answer(timed_out, WRONG_PASSPHRASE, HANDSHAKE_TIMEOUT_MS),
                |frame| eapol_from(frame, bssid),
            )?;
            let reply = match supplicant.receive(&message) {
                Ok(reply) => reply,
                Err(handshake::HandshakeError::BadMic) => return Err(WRONG_PASSPHRASE),
                Err(_) if answered => return Err(WRONG_PASSPHRASE),
                Err(_) => continue,
            };
            let frame = client::data_frame(
                own,
                bssid,
                bssid,
                client::ETHERTYPE_EAPOL,
                &reply.frame[..reply.length],
            );
            link.send(transport, &frame).map_err(joining_failed)?;
            answered = true;
            if let Some(keys) = reply.keys {
                return Ok((Some(keys), Some(supplicant)));
            }
        }
    };
    match attempt() {
        Ok((keys, supplicant)) => Ok(Connection {
            link,
            keys,
            management_protected: akm == Some(Akm::Sae),
            supplicant,
        }),
        Err(failure) => {
            let _ = link.send(transport, &client::deauthentication(own, bssid, LEAVING));
            let _ = station::leave(
                transport,
                firmware,
                link,
                &mut Pauses::<P>::within(INIT_TIMEOUT_MS),
            );
            Err(failure)
        }
    }
}

/// Intel's Wi-Fi adapters (PCI class 02h, subclass 80h, vendor 8086h).
pub static INTEL_WIFI_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "intel-wifi",
    pass: ProbePass::Device,
    matches: &[Match::PciClass {
        class: NETWORK_CONTROLLER,
        subclass: OTHER_NETWORK,
    }],
    probe: probe_intel_wifi,
};

struct Found {
    regs: Mmio,
    adapter: Adapter,
}

// SAFETY: the registers are the adapter's alone, mapped for good by the probe, and only the
// bring-up thread uses them once it takes them.
unsafe impl Send for Found {}

static FOUND: SpinLock<Option<Found>> = SpinLock::new(None);
static STATUS: SpinLock<&'static str> = SpinLock::new("no Intel Wi-Fi adapter");
static RUNNING: SpinLock<Option<Running>> = SpinLock::new(None);
static NETWORKS: SpinLock<Vec<Seen>> = SpinLock::new(Vec::new());

struct Running {
    firmware: (u8, u8, u8),
    address: [u8; 6],
    channels: usize,
}

/// A network the last scan heard: what its beacon or probe response said, and how strongly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    pub network: Network,
    pub signal: i8,
    /// Its beacon interval, in time units.
    pub beacon_interval: u16,
}

/// The networks the last scan heard, strongest first.
pub fn networks() -> Vec<Seen> {
    NETWORKS.lock().clone()
}

fn address_text(address: [u8; 6]) -> alloc::string::String {
    let [a, b, c, d, e, f] = address;
    alloc::format!("{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}")
}

/// What the Wi-Fi adapter reached, for `/proc/driver/wifi`: the state, and once the NVM is
/// read the adapter's MAC address and how many channels it may use; then the chosen network
/// while it is not joined (`joining <name>`), the joined one and its signal (`network <name>`,
/// `signal <n> dBm`), and the last failure until the next try ends (`failed <code>: <words>`,
/// D284); then the networks the last scan heard, one a line (`<BSSID> <channel> <signal> dBm
/// <security> <SSID>`).
pub fn status() -> alloc::string::String {
    let state = *STATUS.lock();
    let Some((address, channels)) = RUNNING
        .lock()
        .as_ref()
        .map(|running| (running.address, running.channels))
    else {
        return state.into();
    };
    let mut text = alloc::format!(
        "{state}; address {}; {channels} channels",
        address_text(address)
    );
    match connected_network() {
        Some(network) => {
            let signal = SIGNAL.load(Ordering::Relaxed);
            text.push_str(&alloc::format!("; network {network}; signal {signal} dBm"));
        }
        None => {
            if let Some(name) = JOINING.lock().as_ref() {
                text.push_str(&alloc::format!("; joining {name}"));
            }
        }
    }
    if let Some(failure) = *FAILURE.lock() {
        text.push_str(&alloc::format!(
            "; failed {}: {}",
            failure.code,
            failure.sentence
        ));
    }
    for seen in networks() {
        let network = seen.network;
        text.push_str(&alloc::format!(
            "\n{} {} {} dBm {:?} {}",
            address_text(network.bssid),
            network.channel.unwrap_or(0),
            seen.signal,
            network.security,
            escaped_name(network.ssid())
        ));
    }
    text
}

fn set_status(words: &'static str) {
    *STATUS.lock() = words;
}

fn probe_intel_wifi(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    if function.vendor_id != VENDOR_INTEL {
        return Err(ProbeError::Refused("not an Intel adapter"));
    }
    if node.dma != DmaCoherence::Coherent {
        return Err(ProbeError::Refused("the device's DMA is not coherent"));
    }
    if FOUND.lock().is_some() {
        return Err(ProbeError::Refused(
            "an Intel Wi-Fi adapter is already in use",
        ));
    }
    let region = node.regions[0];
    if region.is_empty() {
        return Err(ProbeError::Refused(
            "the adapter's registers are not in memory space",
        ));
    }
    let length = usize::try_from(region.len())
        .map_err(|_| ProbeError::Refused("the register block is too large"))?;
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let subdevice = (config.read32(function.address, SUBSYSTEM_IDS) >> 16) as u16;
    pci::enable_memory_and_bus_master(config, function.address);
    let base = probe.map_window(region)?;
    // SAFETY: BAR 0 of an Intel Wi-Fi adapter is its control and status register block, mapped
    // as device memory by `map_window` for as long as the kernel runs; only this driver drives
    // the adapter.
    let regs = unsafe { Mmio::new(base, length) };
    let hw_rev = transport::older_hw_rev(eggos_drivers::io::RegisterIo::read32(&regs, HW_REV));
    let adapter = devices::identify(function.device_id, subdevice, hw_rev).ok_or(
        ProbeError::Refused("an Intel Wi-Fi adapter this driver does not run"),
    )?;
    set_status("waiting for the file systems");
    *FOUND.lock() = Some(Found { regs, adapter });
    Ok(())
}

struct Regions;

impl DmaAllocator for Regions {
    type Memory = DmaRegion;

    fn allocate(&mut self, bytes: usize) -> Option<DmaRegion> {
        DmaRegion::allocate(bytes).ok()
    }
}

struct Pauses<P: Platform> {
    deadline: Deadline,
    platform: core::marker::PhantomData<P>,
}

impl<P: Platform> Pauses<P> {
    fn within(milliseconds: u64) -> Self {
        Self {
            deadline: Deadline::after::<P>(milliseconds),
            platform: core::marker::PhantomData,
        }
    }
}

impl<P: Platform> Wait for Pauses<P> {
    fn pause(&mut self, microseconds: u32) {
        let mut pause = Deadline::after::<P>(u64::from(microseconds).div_ceil(1000));
        while pause.keep_waiting() {
            core::hint::spin_loop();
        }
    }

    fn keep_waiting(&mut self) -> bool {
        self.deadline.keep_waiting()
    }
}

/// Starts the thread that brings the adapter the probe found up, once the file systems are
/// mounted; whether there is one.
pub fn start<P: Platform>() -> bool {
    let Some(found) = FOUND.lock().take() else {
        return false;
    };
    set_status("waiting for the wifi-firmware component");
    if crate::thread::spawn::<P, _>(move || bring_up::<P>(found)).is_err() {
        set_status("no thread to start the adapter");
    }
    true
}

struct Station {
    transport: Transport<Mmio, DmaRegion>,
    firmware: Firmware,
    adapter: Adapter,
    nvm: nvm::Nvm,
    joined: Option<Joined>,
    attached: bool,
}

impl Station {
    fn ax210(&self) -> bool {
        self.adapter.family == devices::Family::Ax210
    }

    fn leave<P: Platform>(&mut self, tell_the_network: bool) {
        let Some(joined) = self.joined.take() else {
            return;
        };
        LINK_UP.store(false, Ordering::Release);
        *CONNECTED.lock() = None;
        TRAFFIC.lock().outgoing.clear();
        let mut link = joined.link;
        if tell_the_network {
            let _ = link.send(
                &self.transport,
                &client::deauthentication(link.address(), link.target().bssid, LEAVING),
            );
        }
        if let Err(error) = station::leave(
            &mut self.transport,
            &self.firmware,
            link,
            &mut Pauses::<P>::within(INIT_TIMEOUT_MS),
        ) {
            *FAILURE.lock() = Some(Failure::adapter(error.description()));
        }
    }

    fn scan<P: Platform>(&mut self) {
        if self.joined.is_none() {
            set_status("scanning");
        }
        let hidden = match (self.joined.is_none(), wanted_hidden(), wanted()) {
            (true, true, Some((ssid, _))) => Some(ssid),
            _ => None,
        };
        let result = scan::scan(
            &mut self.transport,
            &self.firmware,
            &self.adapter,
            &self.nvm,
            hidden.as_deref(),
            &mut Pauses::<P>::within(SCAN_TIMEOUT_MS),
        );
        match result {
            Ok(heard) => publish(&heard),
            Err(error) => *FAILURE.lock() = Some(Failure::adapter(error.description())),
        }
        if let Some(joined) = &self.joined {
            let bssid = joined.link.target().bssid;
            if let Some(seen) = networks().iter().find(|seen| seen.network.bssid == bssid) {
                SIGNAL.store(i32::from(seen.signal), Ordering::Relaxed);
            }
        } else {
            set_status("ready");
        }
    }

    fn join<P: Platform>(&mut self, ssid: &[u8], passphrase: &[u8]) -> Result<(), Failure> {
        let seen = networks()
            .into_iter()
            .find(|seen| seen.network.ssid() == ssid)
            .ok_or(NOT_IN_RANGE)?;
        set_status("connecting");
        let joined = connect::<P>(
            &mut self.transport,
            &self.firmware,
            &self.adapter,
            &self.nvm,
            &seen,
            passphrase,
        );
        let joined = match joined {
            Ok(joined) => joined,
            Err(failure) => {
                set_status("ready");
                return Err(failure);
            }
        };
        let address = joined.link.address();
        self.joined = Some(Joined {
            link: joined.link,
            keys: joined.keys,
            ssid: ssid.to_vec(),
            passphrase: passphrase.to_vec(),
            packet_number: 0,
            last_unicast: 0,
            last_group: 0,
            management_protected: joined.management_protected,
            last_management: 0,
            supplicant: joined.supplicant,
        });
        if !self.attached {
            let device = WifiDevice::<P> {
                address,
                platform: core::marker::PhantomData,
            };
            if let Err(reason) = crate::net::attach_wireless::<P>(alloc::boxed::Box::new(device)) {
                self.leave::<P>(true);
                set_status("ready");
                return Err(Failure::adapter(reason));
            }
            self.attached = true;
        }
        SIGNAL.store(i32::from(seen.signal), Ordering::Relaxed);
        *CONNECTED.lock() = Some(escaped_name(ssid));
        LINK_UP.store(true, Ordering::Release);
        set_status("connected");
        Ok(())
    }

    fn pump<P: Platform>(&mut self) -> Result<(), Failure> {
        let ax210 = self.ax210();
        let Some(joined) = self.joined.as_mut() else {
            return Ok(());
        };
        let outgoing: Vec<Vec<u8>> = TRAFFIC.lock().outgoing.drain(..).collect();
        for frame in outgoing {
            if let Ok(wire) = joined.wire_frame(&frame) {
                let _ = joined.link.send(&self.transport, &wire);
            }
        }
        let mut arrived = false;
        for packet in self.transport.take_notifications() {
            if packet.code != scan::RX_MPDU {
                continue;
            }
            let Some(heard) = scan::received_frame(&packet.payload, ax210) else {
                continue;
            };
            if joined.ended(&heard.frame) {
                return Err(DROPPED);
            }
            match joined.received(&heard.frame) {
                Some(Received::Ethernet(ethernet)) => {
                    let mut traffic = TRAFFIC.lock();
                    if traffic.incoming.len() < MAX_PENDING_FRAMES {
                        traffic.incoming.push_back(ethernet);
                        arrived = true;
                    }
                }
                Some(Received::Eapol(eapol)) => {
                    if let Some(answer) = joined.rekey(&eapol) {
                        let _ = joined.link.send(&self.transport, &answer);
                    }
                }
                None => {}
            }
        }
        if arrived {
            crate::net::DEVICE_EVENTS.raise::<P>();
        }
        Ok(())
    }

    fn joined_matches(&self, chosen: &Option<(Vec<u8>, Vec<u8>)>) -> bool {
        match (&self.joined, chosen) {
            (Some(joined), Some((ssid, passphrase))) => {
                &joined.ssid == ssid && &joined.passphrase == passphrase
            }
            _ => false,
        }
    }
}

fn bring_up<P: Platform>(found: Found) {
    let Found { regs, adapter } = found;
    let path = devices::firmware_path(&adapter);
    let bytes = loop {
        if let Some(bytes) = super::read_firmware(&path, LARGEST_FIRMWARE as u64) {
            break bytes;
        }
        crate::thread::sleep::<P>(FIRMWARE_RETRY);
    };
    let firmware = match Firmware::parse(&bytes) {
        Ok(firmware) => firmware,
        Err(_) => return set_status("the firmware file is damaged"),
    };
    let (Ok(rings), Ok(buffers)) = (
        DmaRegion::allocate(transport::RING_BYTES),
        DmaRegion::allocate(transport::RX_BUFFER_AREA_BYTES),
    ) else {
        return set_status("no memory for the adapter's rings");
    };
    let mut transport = Transport::new(regs, adapter, rings, buffers);
    set_status("starting the adapter");
    if let Err(error) = transport.start_hardware(&mut Pauses::<P>::within(START_TIMEOUT_MS)) {
        return set_status(error.description());
    }
    if let Err(error) = transport.load_firmware(
        &firmware,
        &bytes,
        &mut Regions,
        &mut Pauses::<P>::within(LOAD_TIMEOUT_MS),
    ) {
        return set_status(error.description());
    }
    let nvm = match nvm::initialize(
        &mut transport,
        &firmware,
        &mut Pauses::<P>::within(INIT_TIMEOUT_MS),
    ) {
        Ok(nvm) => nvm,
        Err(error) => return set_status(error.description()),
    };
    if let Err(error) = scan::configure(
        &mut transport,
        &firmware,
        &adapter,
        &nvm,
        &mut Pauses::<P>::within(INIT_TIMEOUT_MS),
    ) {
        return set_status(error.description());
    }
    *RUNNING.lock() = Some(Running {
        firmware: firmware.version,
        address: nvm.address,
        channels: nvm.channels.len(),
    });
    set_status("ready");
    run::<P>(Station {
        transport,
        firmware,
        adapter,
        nvm,
        joined: None,
        attached: false,
    });
}

fn run<P: Platform>(mut station: Station) {
    let mut chosen: Option<(Vec<u8>, Vec<u8>)> = None;
    let mut last_scan_ms: Option<u64> = None;
    let mut next_try_ms = 0u64;
    let mut last_settings_ms: Option<u64> = None;
    loop {
        let seen = WIFI_EVENTS.raised();
        let now = crate::net::now_ms::<P>();
        if last_settings_ms.is_none_or(|at| now.saturating_sub(at) >= SETTINGS_CHECK_MS) {
            last_settings_ms = Some(now);
            let settings = wanted();
            if settings != chosen {
                chosen = settings;
                *JOINING.lock() = chosen.as_ref().map(|(ssid, _)| escaped_name(ssid));
                if station.joined.is_some() && !station.joined_matches(&chosen) {
                    station.leave::<P>(true);
                    set_status("ready");
                }
                if chosen.is_some() {
                    last_scan_ms = None;
                    next_try_ms = 0;
                }
            }
        }
        if station.transport.radio_killed() {
            station.leave::<P>(false);
            set_status("ready; the radio is switched off");
        } else {
            if last_scan_ms.is_none_or(|at| now.saturating_sub(at) >= SCAN_INTERVAL_MS) {
                station.scan::<P>();
                last_scan_ms = Some(crate::net::now_ms::<P>());
            }
            if station.joined.is_none()
                && let Some((ssid, passphrase)) = chosen.clone()
                && crate::net::now_ms::<P>() >= next_try_ms
            {
                let outcome = station.join::<P>(&ssid, &passphrase);
                *FAILURE.lock() = outcome.err();
                if outcome.is_err() {
                    next_try_ms = last_scan_ms.unwrap_or(0).saturating_add(SCAN_INTERVAL_MS);
                }
            }
            if let Err(failure) = station.pump::<P>() {
                station.leave::<P>(false);
                set_status("ready");
                *FAILURE.lock() = Some(failure);
                next_try_ms = 0;
            }
        }
        let wait_ms = match station.joined.is_some() {
            true => CONNECTED_PUMP_MS,
            false => SETTINGS_CHECK_MS,
        };
        WIFI_EVENTS.wait::<P>(seen, crate::net::ticks_after::<P>(wait_ms));
    }
}

fn publish(heard: &[scan::Heard]) {
    let mut seen: Vec<Seen> = Vec::new();
    for frame in heard {
        let Some(mut network) = wifi_frames::network(&frame.frame) else {
            continue;
        };
        network.channel = network.channel.or(Some(frame.channel));
        match seen
            .iter_mut()
            .find(|known| known.network.bssid == network.bssid)
        {
            Some(known) => known.signal = known.signal.max(frame.signal),
            None => seen.push(Seen {
                network,
                signal: frame.signal,
                beacon_interval: client::beacon_interval(&frame.frame).unwrap_or(100),
            }),
        }
    }
    seen.sort_by_key(|known| core::cmp::Reverse(known.signal));
    *NETWORKS.lock() = seen;
}

/// The running firmware's major, minor and API version.
pub fn firmware_version() -> Option<(u8, u8, u8)> {
    RUNNING.lock().as_ref().map(|running| running.firmware)
}
