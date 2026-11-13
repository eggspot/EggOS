//! USB (D239): standard requests and descriptors (USB 2.0, chapter 9), the HID class's boot
//! interfaces, and the xHCI host controller driver.

pub mod xhci;

/// A setup packet's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl Setup {
    /// The eight bytes as the xHCI setup TRB's immediate data takes them.
    pub fn as_u64(&self) -> u64 {
        u64::from(self.request_type)
            | (u64::from(self.request) << 8)
            | (u64::from(self.value) << 16)
            | (u64::from(self.index) << 32)
            | (u64::from(self.length) << 48)
    }

    pub fn is_in(&self) -> bool {
        self.request_type & 0x80 != 0
    }

    pub const fn get_descriptor(kind: u8, index: u8, length: u16) -> Self {
        Self {
            request_type: 0x80,
            request: 6,
            value: ((kind as u16) << 8) | index as u16,
            index: 0,
            length,
        }
    }

    pub const fn set_configuration(value: u8) -> Self {
        Self {
            request_type: 0x00,
            request: 9,
            value: value as u16,
            index: 0,
            length: 0,
        }
    }

    pub const fn hid_set_protocol_boot(interface: u8) -> Self {
        Self {
            request_type: 0x21,
            request: 0x0b,
            value: 0,
            index: interface as u16,
            length: 0,
        }
    }

    pub const fn hid_set_idle(interface: u8) -> Self {
        Self {
            request_type: 0x21,
            request: 0x0a,
            value: 0,
            index: interface as u16,
            length: 0,
        }
    }

    pub const fn hid_report_descriptor(interface: u8, length: u16) -> Self {
        Self {
            request_type: 0x81,
            request: 6,
            value: (DESCRIPTOR_REPORT as u16) << 8,
            index: interface as u16,
            length,
        }
    }

    pub const fn hid_set_output_report(interface: u8, length: u16) -> Self {
        Self {
            request_type: 0x21,
            request: 0x09,
            value: 0x0200,
            index: interface as u16,
            length,
        }
    }
}

pub const DESCRIPTOR_DEVICE: u8 = 1;
pub const DESCRIPTOR_CONFIGURATION: u8 = 2;
const DESCRIPTOR_INTERFACE: u8 = 4;
const DESCRIPTOR_ENDPOINT: u8 = 5;
const CLASS_HID: u8 = 3;
const DESCRIPTOR_HID: u8 = 0x21;
const DESCRIPTOR_REPORT: u8 = 0x22;
const SUBCLASS_BOOT: u8 = 1;

/// A HID boot device's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootDevice {
    Keyboard,
    Mouse,
    /// A HID device without the boot protocol, read through its report descriptor (D256).
    Report,
}

/// A HID boot interface with its interrupt IN endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootInterface {
    pub configuration: u8,
    pub interface: u8,
    pub kind: BootDevice,
    pub endpoint: u8,
    pub max_packet: u16,
    pub interval: u8,
    /// The report descriptor's length, from the HID descriptor.
    pub report_length: u16,
}

/// A USB mass-storage interface with the bulk-only transport and SCSI commands (D264): its
/// configuration, interface and bulk endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageInterface {
    pub configuration: u8,
    pub interface: u8,
    pub bulk_in: u8,
    pub bulk_out: u8,
    pub max_packet: u16,
}

/// The first mass-storage interface (class 08h, SCSI transparent 06h, bulk-only 50h) of a
/// configuration descriptor, with its two bulk endpoints.
pub fn storage_interface(configuration: &[u8]) -> Option<StorageInterface> {
    let value = *configuration.get(5)?;
    let mut at = 0;
    let mut found: Option<StorageInterface> = None;
    while at + 2 <= configuration.len() {
        let length = usize::from(configuration[at]);
        if length < 2 || at + length > configuration.len() {
            return None;
        }
        let descriptor = &configuration[at..at + length];
        match descriptor[1] {
            DESCRIPTOR_INTERFACE if length >= 9 => {
                if found.is_some_and(|storage| storage.bulk_in != 0 && storage.bulk_out != 0) {
                    return found;
                }
                found = (descriptor[5] == 0x08 && descriptor[6] == 0x06 && descriptor[7] == 0x50)
                    .then_some(StorageInterface {
                        configuration: value,
                        interface: descriptor[2],
                        bulk_in: 0,
                        bulk_out: 0,
                        max_packet: 0,
                    });
            }
            DESCRIPTOR_ENDPOINT if length >= 7 => {
                if let Some(storage) = found.as_mut() {
                    let address = descriptor[2];
                    if descriptor[3] & 0b11 == 0b10 {
                        storage.max_packet =
                            u16::from_le_bytes([descriptor[4], descriptor[5]]) & 0x7ff;
                        if address & 0x80 != 0 {
                            storage.bulk_in = address & 0x0f;
                        } else {
                            storage.bulk_out = address & 0x0f;
                        }
                    }
                }
            }
            _ => {}
        }
        at += length;
    }
    found.filter(|storage| storage.bulk_in != 0 && storage.bulk_out != 0)
}

/// A bulk-only transport command block wrapper (USB Mass Storage Class BOT 1.0, 5.1).
pub fn command_block(tag: u32, data_bytes: u32, data_in: bool, command: &[u8]) -> [u8; 31] {
    let mut block = [0u8; 31];
    block[..4].copy_from_slice(&0x4342_5355u32.to_le_bytes());
    block[4..8].copy_from_slice(&tag.to_le_bytes());
    block[8..12].copy_from_slice(&data_bytes.to_le_bytes());
    block[12] = if data_in { 0x80 } else { 0 };
    block[14] = command.len().min(16) as u8;
    block[15..15 + command.len().min(16)].copy_from_slice(&command[..command.len().min(16)]);
    block
}

/// Whether a command status wrapper says the command with `tag` passed.
pub fn status_passed(status: &[u8; 13], tag: u32) -> bool {
    status[..4] == 0x5342_5355u32.to_le_bytes()
        && status[4..8] == tag.to_le_bytes()
        && status[12] == 0
}

/// SCSI READ(10) or WRITE(10) of `blocks` blocks from `lba`.
pub fn scsi_read_write_10(write: bool, lba: u32, blocks: u16) -> [u8; 10] {
    let mut command = [0u8; 10];
    command[0] = if write { 0x2a } else { 0x28 };
    command[2..6].copy_from_slice(&lba.to_be_bytes());
    command[7..9].copy_from_slice(&blocks.to_be_bytes());
    command
}

/// The first HID boot keyboard or mouse interface in a configuration descriptor and all that
/// follows it (`wTotalLength` bytes).
pub fn boot_interface(configuration: &[u8]) -> Option<BootInterface> {
    let value = *configuration.get(5)?;
    let mut at = 0;
    let mut current: Option<(u8, BootDevice)> = None;
    let mut report_length = 0u16;
    while at + 2 <= configuration.len() {
        let length = usize::from(configuration[at]);
        if length < 2 || at + length > configuration.len() {
            return None;
        }
        let descriptor = &configuration[at..at + length];
        match descriptor[1] {
            DESCRIPTOR_INTERFACE if length >= 9 => {
                report_length = 0;
                current = (descriptor[5] == CLASS_HID).then(|| {
                    let kind = match (descriptor[6] == SUBCLASS_BOOT, descriptor[7]) {
                        (true, 1) => BootDevice::Keyboard,
                        (true, 2) => BootDevice::Mouse,
                        _ => BootDevice::Report,
                    };
                    (descriptor[2], kind)
                });
            }
            DESCRIPTOR_HID if length >= 9 && descriptor[6] == DESCRIPTOR_REPORT => {
                report_length = u16::from_le_bytes([descriptor[7], descriptor[8]]);
            }
            DESCRIPTOR_ENDPOINT if length >= 7 => {
                if let Some((interface, kind)) = current {
                    let address = descriptor[2];
                    let interrupt = descriptor[3] & 0b11 == 0b11;
                    if address & 0x80 != 0 && interrupt {
                        return Some(BootInterface {
                            configuration: value,
                            interface,
                            kind,
                            endpoint: address & 0x0f,
                            max_packet: u16::from_le_bytes([descriptor[4], descriptor[5]]) & 0x7ff,
                            interval: descriptor[6],
                            report_length,
                        });
                    }
                }
            }
            _ => {}
        }
        at += length;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setup_packet_packs_little_endian() {
        let setup = Setup::get_descriptor(DESCRIPTOR_DEVICE, 0, 18);
        assert_eq!(setup.as_u64(), 0x0012_0000_0100_0680);
        assert!(setup.is_in());
    }

    #[test]
    fn the_boot_keyboard_interface_and_its_endpoint_are_found() {
        let configuration = [
            9, 2, 34, 0, 1, 1, 0, 0xa0, 50, 9, 4, 0, 0, 1, 3, 1, 1, 0, 9, 0x21, 0x11, 1, 0, 1,
            0x22, 63, 0, 7, 5, 0x81, 3, 8, 0, 10,
        ];
        let found = boot_interface(&configuration).unwrap();
        assert_eq!(found.kind, BootDevice::Keyboard);
        assert_eq!(found.configuration, 1);
        assert_eq!(found.endpoint, 1);
        assert_eq!(found.max_packet, 8);
        assert_eq!(found.interval, 10);
    }

    #[test]
    fn a_configuration_without_a_boot_interface_has_none() {
        let configuration = [
            9, 2, 25, 0, 1, 1, 0, 0xa0, 50, 9, 4, 0, 0, 1, 8, 6, 80, 0, 7, 5, 0x81, 2, 0, 2, 0,
        ];
        assert_eq!(boot_interface(&configuration), None);
    }

    #[test]
    fn a_flash_drive_s_bulk_endpoints_are_found() {
        let configuration = [
            9, 2, 32, 0, 1, 1, 0, 0x80, 50, 9, 4, 0, 0, 2, 8, 6, 0x50, 0, 7, 5, 0x81, 2, 0, 2, 0,
            7, 5, 0x02, 2, 0, 2, 0,
        ];
        let storage = storage_interface(&configuration).unwrap();
        assert_eq!(
            (storage.bulk_in, storage.bulk_out, storage.max_packet),
            (1, 2, 512)
        );
    }

    #[test]
    fn a_command_block_carries_its_tag_length_and_command() {
        let block = command_block(7, 512, true, &scsi_read_write_10(false, 0x100, 1));
        assert_eq!(&block[..4], b"USBC");
        assert_eq!(block[4], 7);
        assert_eq!(u32::from_le_bytes(block[8..12].try_into().unwrap()), 512);
        assert_eq!(block[12], 0x80);
        assert_eq!(block[14], 10);
        assert_eq!(block[15], 0x28);
        assert_eq!(&block[17..21], &[0, 0, 1, 0]);
        let mut status = [0u8; 13];
        status[..4].copy_from_slice(b"USBS");
        status[4] = 7;
        assert!(status_passed(&status, 7));
        status[12] = 1;
        assert!(!status_passed(&status, 7));
    }
}
