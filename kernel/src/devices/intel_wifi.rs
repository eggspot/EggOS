//! Intel's Wi-Fi 6 and 6E adapters on PCI (D274; the driver is `eggos_drivers::wifi::iwx`,
//! D272): the probe finds the adapter and keeps its registers; once the file systems are mounted
//! a kernel thread waits for the `wifi-firmware` component's file, powers the adapter up and
//! hands it the firmware. What it reached shows in `/proc/driver/wifi`.

use super::dma::DmaRegion;
use super::{Deadline, DeviceId, DmaCoherence, DriverDescriptor, Identity, Match, Probe, ProbeError, ProbePass};
use crate::linux::exec::ProgramBytes;
use crate::sync::SpinLock;
use crate::vfs::{self, NodeKind};
use alloc::vec::Vec;
use core::time::Duration;
use eggos_drivers::dma::DmaAllocator;
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_drivers::wifi::iwx::devices::{self, Adapter, VENDOR_INTEL};
use eggos_drivers::wifi::iwx::firmware::Firmware;
use eggos_drivers::wifi::iwx::nvm;
use eggos_drivers::wifi::iwx::scan;
use eggos_drivers::wifi::iwx::station::{self, Link, Target};
use eggos_wifi::client;
use eggos_wifi::frames::{self as wifi_frames, Network, Security};
use eggos_wifi::handshake::{self, Keys, Supplicant};
use eggos_drivers::wifi::iwx::transport::{self, Transport, Wait};
use eggos_hal::Platform;

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
const SCAN_INTERVAL: Duration = Duration::from_secs(30);
const ANSWER_TIMEOUT_MS: u64 = 2000;
const HANDSHAKE_TIMEOUT_MS: u64 = 5000;
/// The file that names the network to join (D278): `ssid=<name>` and `passphrase=<words>` lines.
pub const SETTINGS_PATH: &str = "/var/lib/eggos/wifi.conf";
const LARGEST_SETTINGS: usize = 4096;

static CONNECTED: SpinLock<Option<alloc::string::String>> = SpinLock::new(None);

struct Connection {
    link: Link<DmaRegion>,
    keys: Option<Keys>,
}

const ETHERNET_HEADER_BYTES: usize = 14;
const WIFI_MTU: usize = 1500;
const MAX_PENDING_FRAMES: usize = 64;
const KEY_ID_SHIFT: u32 = 6;

/// The joined network as the network stack's adapter (D279): Ethernet frames out become data
/// frames to the access point, protected by CCMP on the host when the network is WPA2; data
/// frames in are checked, decrypted and handed up as Ethernet frames.
struct WifiDevice {
    transport: Transport<Mmio, DmaRegion>,
    link: Link<DmaRegion>,
    keys: Option<Keys>,
    ax210: bool,
    packet_number: u64,
    last_unicast: u64,
    last_group: u64,
    pending: alloc::collections::VecDeque<Vec<u8>>,
}

// SAFETY: as `Found`: the adapter's registers and DMA memory belong to this device alone once
// the bring-up thread hands it to the network stack.
unsafe impl Send for WifiDevice {}

impl WifiDevice {
    fn take_frames(&mut self) {
        for packet in self.transport.take_notifications() {
            if packet.code != scan::RX_MPDU || self.pending.len() >= MAX_PENDING_FRAMES {
                continue;
            }
            let Some(heard) = scan::received_frame(&packet.payload, self.ax210) else {
                continue;
            };
            if let Some(ethernet) = self.ethernet_of(&heard.frame) {
                self.pending.push_back(ethernet);
            }
        }
    }

    fn ethernet_of(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        let header = client::data_header_bytes(frame)?;
        let bssid = self.link.target().bssid;
        let mut body = alloc::vec![0u8; frame.len()];
        let body = match (client::is_protected_data(frame), &self.keys) {
            (true, Some(keys)) => {
                let group = frame.get(4).is_some_and(|first| first & 1 != 0);
                let key_id = frame.get(header + 3).map_or(0, |byte| byte >> KEY_ID_SHIFT);
                let key: [u8; 16] = match group {
                    true if key_id == keys.gtk_index && keys.gtk_bytes >= 16 => keys.gtk[..16].try_into().ok()?,
                    true => return None,
                    false => keys.tk,
                };
                let (length, number) = eggos_wifi::protection::decrypt(&key, frame, &mut body).ok()?;
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
            return None;
        }
        let mut ethernet = Vec::with_capacity(ETHERNET_HEADER_BYTES + payload.len());
        ethernet.extend_from_slice(&destination);
        ethernet.extend_from_slice(&source);
        ethernet.extend_from_slice(&ethertype.to_be_bytes());
        ethernet.extend_from_slice(&payload);
        Some(ethernet)
    }
}

impl crate::net::NetDevice for WifiDevice {
    fn mac(&self) -> eggos_net::ethernet::MacAddress {
        eggos_net::ethernet::MacAddress(self.link.address())
    }

    fn mtu(&self) -> usize {
        WIFI_MTU
    }

    fn is_link_up(&self) -> bool {
        !self.transport.radio_killed()
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        if self.pending.is_empty() {
            self.take_frames();
        }
        let next = self.pending.pop_front()?;
        let length = next.len().min(frame.len());
        frame[..length].copy_from_slice(&next[..length]);
        Some(length)
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        if frame.len() < ETHERNET_HEADER_BYTES {
            return Err("a frame shorter than its Ethernet header");
        }
        let destination: [u8; 6] = frame[..6].try_into().map_err(|_| "a damaged Ethernet frame")?;
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
        let own = self.link.address();
        let bssid = self.link.target().bssid;
        let plain = client::data_frame(own, bssid, destination, ethertype, &frame[ETHERNET_HEADER_BYTES..]);
        let wire = match &self.keys {
            None => plain,
            Some(keys) => {
                self.packet_number += 1;
                let header = client::data_header_bytes(&plain).ok_or("not a data frame")?;
                let mut out = alloc::vec![0u8; plain.len() + eggos_wifi::protection::CCMP_HEADER_BYTES + 8];
                let length = eggos_wifi::protection::encrypt(&keys.tk, &plain[..header], self.packet_number, 0, &plain[header..], &mut out)
                    .map_err(|_| "the frame could not be protected")?;
                out.truncate(length);
                out
            }
        };
        self.link.send(&self.transport, &wire).map_err(station::JoinError::description)
    }
}

/// The network the station is connected to, if any.
pub fn connected_network() -> Option<alloc::string::String> {
    CONNECTED.lock().clone()
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

fn wait_frame<P: Platform, T>(
    transport: &mut Transport<Mmio, DmaRegion>,
    ax210: bool,
    milliseconds: u64,
    mut wanted: impl FnMut(&[u8]) -> Option<T>,
) -> Result<T, &'static str> {
    let mut wait = Pauses::<P>::within(milliseconds);
    loop {
        for packet in transport.take_notifications() {
            if packet.code != scan::RX_MPDU {
                continue;
            }
            let found = scan::received_frame(&packet.payload, ax210).and_then(|heard| wanted(&heard.frame));
            if let Some(found) = found {
                return Ok(found);
            }
        }
        if !wait.keep_waiting() {
            return Err("the network did not answer");
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
) -> Result<Connection, &'static str> {
    let network = seen.network;
    let channel = nvm
        .channels
        .iter()
        .find(|channel| Some(channel.number) == network.channel)
        .copied()
        .ok_or("the network's channel is not one the adapter may use")?;
    let protected = match network.security {
        Security::Open => false,
        Security::Wpa2Personal => true,
        _ => return Err("the network's security is not WPA2-Personal or open"),
    };
    let (own, bssid) = (nvm.address, network.bssid);
    let ax210 = adapter.family == devices::Family::Ax210;
    let target = Target {
        bssid,
        channel,
        beacon_interval: seen.beacon_interval,
    };
    let mut link = station::join(transport, firmware, adapter, own, target, &mut Regions, &mut Pauses::<P>::within(INIT_TIMEOUT_MS))
        .map_err(station::JoinError::description)?;
    link.send(transport, &client::authentication_request(own, bssid)).map_err(station::JoinError::description)?;
    let status = wait_frame::<P, _>(transport, ax210, ANSWER_TIMEOUT_MS, |frame| client::authentication_status(frame, bssid))?;
    if status != 0 {
        return Err("the network refused authentication");
    }
    let request = client::association_request(own, bssid, network.ssid(), protected);
    link.send(transport, &request).map_err(station::JoinError::description)?;
    let (status, association) = wait_frame::<P, _>(transport, ax210, ANSWER_TIMEOUT_MS, |frame| client::association_answer(frame, bssid))?;
    if status != 0 {
        return Err("the network refused the association");
    }
    station::associated(transport, &link, association, &mut Pauses::<P>::within(INIT_TIMEOUT_MS)).map_err(station::JoinError::description)?;
    if !protected {
        return Ok(Connection { link, keys: None });
    }
    let pmk = handshake::pmk_from_passphrase(passphrase, network.ssid()).ok_or("the passphrase is not 8 to 63 characters")?;
    let mut nonce = [0u8; handshake::NONCE_BYTES];
    crate::random::fill::<P>(&mut nonce);
    let mut supplicant = Supplicant::new(pmk, own, bssid, nonce);
    loop {
        let message = wait_frame::<P, _>(transport, ax210, HANDSHAKE_TIMEOUT_MS, |frame| eapol_from(frame, bssid))?;
        let answer = supplicant.receive(&message).map_err(|_| "the WPA2 handshake failed: is the passphrase right?")?;
        let frame = client::data_frame(own, bssid, bssid, client::ETHERTYPE_EAPOL, &answer.frame[..answer.length]);
        link.send(transport, &frame).map_err(station::JoinError::description)?;
        if let Some(keys) = answer.keys {
            return Ok(Connection { link, keys: Some(keys) });
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
/// read the adapter's MAC address, how many channels it may use and the networks the last
/// scan heard, one a line (`<BSSID> <channel> <signal> dBm <security> <SSID>`).
pub fn status() -> alloc::string::String {
    let state = *STATUS.lock();
    let Some((address, channels)) = RUNNING.lock().as_ref().map(|running| (running.address, running.channels)) else {
        return state.into();
    };
    let mut text = alloc::format!("{state}; address {}; {channels} channels", address_text(address));
    if let Some(network) = connected_network() {
        text.push_str(&alloc::format!("; network {network}"));
    }
    for seen in networks() {
        let network = seen.network;
        text.push_str(&alloc::format!(
            "\n{} {} {} dBm {:?} {}",
            address_text(network.bssid),
            network.channel.unwrap_or(0),
            seen.signal,
            network.security,
            alloc::string::String::from_utf8_lossy(network.ssid())
        ));
    }
    text
}

fn set_status(words: &'static str) {
    *STATUS.lock() = words;
}

fn probe_intel_wifi(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe.table().get(id).ok_or(ProbeError::Refused("no such device"))?;
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
        return Err(ProbeError::Refused("an Intel Wi-Fi adapter is already in use"));
    }
    let region = node.regions[0];
    if region.is_empty() {
        return Err(ProbeError::Refused("the adapter's registers are not in memory space"));
    }
    let length = usize::try_from(region.len()).map_err(|_| ProbeError::Refused("the register block is too large"))?;
    let config = probe.pci_config().ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let subdevice = (config.read32(function.address, SUBSYSTEM_IDS) >> 16) as u16;
    pci::enable_memory_and_bus_master(config, function.address);
    let base = probe.map_window(region)?;
    // SAFETY: BAR 0 of an Intel Wi-Fi adapter is its control and status register block, mapped
    // as device memory by `map_window` for as long as the kernel runs; only this driver drives
    // the adapter.
    let regs = unsafe { Mmio::new(base, length) };
    let hw_rev = transport::older_hw_rev(eggos_drivers::io::RegisterIo::read32(&regs, HW_REV));
    let adapter = devices::identify(function.device_id, subdevice, hw_rev)
        .ok_or(ProbeError::Refused("an Intel Wi-Fi adapter this driver does not run"))?;
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

fn read_firmware(path: &str) -> Option<Vec<u8>> {
    let root = vfs::root();
    let node = vfs::resolve(root, root, path.as_bytes()).ok()?;
    let size = node.attributes().size;
    if node.kind() != NodeKind::Regular || size > LARGEST_FIRMWARE as u64 {
        return None;
    }
    let bytes = ProgramBytes::read(node, size, size).ok()?;
    Some(bytes.bytes().to_vec())
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

fn bring_up<P: Platform>(found: Found) {
    let Found { regs, adapter } = found;
    let path = devices::firmware_path(&adapter);
    let bytes = loop {
        if let Some(bytes) = read_firmware(&path) {
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
    if let Err(error) = transport.load_firmware(&firmware, &bytes, &mut Regions, &mut Pauses::<P>::within(LOAD_TIMEOUT_MS)) {
        return set_status(error.description());
    }
    let nvm = match nvm::initialize(&mut transport, &firmware, &mut Pauses::<P>::within(INIT_TIMEOUT_MS)) {
        Ok(nvm) => nvm,
        Err(error) => return set_status(error.description()),
    };
    if let Err(error) = scan::configure(&mut transport, &firmware, &adapter, &nvm, &mut Pauses::<P>::within(INIT_TIMEOUT_MS)) {
        return set_status(error.description());
    }
    *RUNNING.lock() = Some(Running {
        firmware: firmware.version,
        address: nvm.address,
        channels: nvm.channels.len(),
    });
    loop {
        if transport.radio_killed() {
            set_status("ready; the radio is switched off");
        } else {
            set_status("scanning");
            match scan::scan(&mut transport, &firmware, &adapter, &nvm, &mut Pauses::<P>::within(SCAN_TIMEOUT_MS)) {
                Ok(heard) => {
                    publish(&heard);
                    set_status("ready");
                }
                Err(error) => set_status(error.description()),
            }
            let chosen = wanted().and_then(|(ssid, passphrase)| {
                networks().into_iter().find(|seen| seen.network.ssid() == ssid.as_slice()).map(|seen| (seen, passphrase))
            });
            if let Some((seen, passphrase)) = chosen {
                set_status("connecting");
                match connect::<P>(&mut transport, &firmware, &adapter, &nvm, &seen, &passphrase) {
                    Ok(joined) => {
                        *CONNECTED.lock() = Some(alloc::string::String::from_utf8_lossy(seen.network.ssid()).into());
                        let device = WifiDevice {
                            transport,
                            link: joined.link,
                            keys: joined.keys,
                            ax210: adapter.family == devices::Family::Ax210,
                            packet_number: 0,
                            last_unicast: 0,
                            last_group: 0,
                            pending: alloc::collections::VecDeque::new(),
                        };
                        return set_status(match crate::net::attach_late::<P>(alloc::boxed::Box::new(device)) {
                            Ok(()) => "connected",
                            Err(reason) => reason,
                        });
                    }
                    Err(reason) => set_status(reason),
                }
            }
        }
        crate::thread::sleep::<P>(SCAN_INTERVAL);
    }
}

fn publish(heard: &[scan::Heard]) {
    let mut seen: Vec<Seen> = Vec::new();
    for frame in heard {
        let Some(mut network) = wifi_frames::network(&frame.frame) else {
            continue;
        };
        network.channel = network.channel.or(Some(frame.channel));
        match seen.iter_mut().find(|known| known.network.bssid == network.bssid) {
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
