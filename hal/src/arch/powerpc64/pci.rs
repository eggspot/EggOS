//! The PCI host bridges of a POWER9 (PHB4, D312), which have no ECAM: configuration space is
//! read and written through OPAL calls, every function is put in one partitionable endpoint
//! (PE 1) whose DMA bypasses translation at bus address `1 << 59` plus the physical address,
//! and MSIs are the PHB's XIVE sources (`ibm,opal-msi-ranges`).

use crate::atomic::AtomicU64;
use super::opal;
use core::sync::atomic::{AtomicBool, Ordering};
use eggos_drivers::pci::{Address, ConfigSpace};

const PHB_COMPATIBLE: [&[u8]; 2] = [b"ibm,ioda3-phb", b"ibm,ioda2-phb"];
/// The PE every function joins.
const PE: u64 = 1;
/// Where PHB4's bypass window starts in bus addresses.
pub(super) const BYPASS_BASE: u64 = 1 << 59;
/// The physical memory the bypass window covers.
const BYPASS_SIZE: u64 = 1 << 40;

static PHB: AtomicU64 = AtomicU64::new(u64::MAX);
static READY: AtomicBool = AtomicBool::new(false);

fn phb_node(blob: &[u8]) -> Option<crate::fdt::Node<'_>> {
    let tree = crate::fdt::Fdt::new(blob).ok()?;
    tree.nodes()
        .map_while(Result::ok)
        .find(|node| node.enabled && PHB_COMPATIBLE.iter().any(|name| node.is_compatible(name)))
}

fn be64(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_be_bytes(bytes.get(..8)?.try_into().ok()?))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at * 4..at * 4 + 4)?.try_into().ok()?))
}

/// The first PHB's MSI range (`ibm,opal-msi-ranges`: first source, count).
pub(super) fn first_phb_msis(blob: &[u8]) -> Option<(u64, u64)> {
    let ranges = phb_node(blob)?.opal_msi_ranges;
    Some((u64::from(be32(ranges, 0)?), u64::from(be32(ranges, 1)?)))
}

/// Configuration space through OPAL for the first PHB.
#[derive(Debug)]
pub(super) struct OpalConfig;

pub(super) static CONFIG: OpalConfig = OpalConfig;

fn rid(address: Address) -> u64 {
    u64::from(address.bus) << 8 | u64::from(address.device) << 3 | u64::from(address.function)
}

impl ConfigSpace for OpalConfig {
    fn read32(&self, address: Address, offset: u16) -> u32 {
        opal::pci_config_read32(PHB.load(Ordering::Relaxed), rid(address), u64::from(offset)).unwrap_or(u32::MAX)
    }

    fn write32(&self, address: Address, offset: u16, value: u32) {
        opal::pci_config_write32(PHB.load(Ordering::Relaxed), rid(address), u64::from(offset), value);
    }
}

/// [`crate::Platform::firmware_pci`]: the first PHB's configuration space, once its functions
/// were put in PE 1 and the PE's DMA set to bypass; `None` without a PHB.
pub(super) fn firmware_pci(blob: Option<&[u8]>) -> Option<&'static dyn ConfigSpace> {
    if READY.load(Ordering::Acquire) {
        return Some(&CONFIG);
    }
    let id = be64(phb_node(blob?)?.opal_phb_id)?;
    PHB.store(id, Ordering::Relaxed);
    if !opal::pci_set_pe_all(id, PE) {
        return None;
    }
    opal::pci_set_mve(id, PE);
    if !opal::pci_map_bypass(id, PE, BYPASS_BASE, BYPASS_SIZE) {
        return None;
    }
    READY.store(true, Ordering::Release);
    Some(&CONFIG)
}

/// The first PHB's windows for the kernel's BAR assignment: its node, whose `ranges` give
/// them (D291).
pub(super) fn phb_windows(blob: &[u8]) -> Option<crate::fdt::Node<'_>> {
    phb_node(blob)
}

/// The address and data OPAL gives MSI `index` of the PHB's range, bound to PE 1.
pub(super) fn msi_message(index: u64) -> Option<(u64, u32)> {
    let phb = PHB.load(Ordering::Relaxed);
    opal::pci_set_xive_pe(phb, PE, index);
    opal::get_msi_64(phb, PE, index)
}

/// What a device must add to a physical address to reach it by DMA: the bypass window's base
/// once it is set.
pub(super) fn dma_offset() -> u64 {
    match READY.load(Ordering::Acquire) {
        true => BYPASS_BASE,
        false => 0,
    }
}
