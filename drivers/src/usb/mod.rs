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
const SUBCLASS_BOOT: u8 = 1;

/// A HID boot device's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootDevice {
    Keyboard,
    Mouse,
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
}

/// The first HID boot keyboard or mouse interface in a configuration descriptor and all that
/// follows it (`wTotalLength` bytes).
pub fn boot_interface(configuration: &[u8]) -> Option<BootInterface> {
    let value = *configuration.get(5)?;
    let mut at = 0;
    let mut current: Option<(u8, BootDevice)> = None;
    while at + 2 <= configuration.len() {
        let length = usize::from(configuration[at]);
        if length < 2 || at + length > configuration.len() {
            return None;
        }
        let descriptor = &configuration[at..at + length];
        match descriptor[1] {
            DESCRIPTOR_INTERFACE if length >= 9 => {
                current = (descriptor[5] == CLASS_HID && descriptor[6] == SUBCLASS_BOOT)
                    .then_some(descriptor[7])
                    .and_then(|protocol| match protocol {
                        1 => Some((descriptor[2], BootDevice::Keyboard)),
                        2 => Some((descriptor[2], BootDevice::Mouse)),
                        _ => None,
                    });
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
            9, 2, 34, 0, 1, 1, 0, 0xa0, 50,
            9, 4, 0, 0, 1, 3, 1, 1, 0,
            9, 0x21, 0x11, 1, 0, 1, 0x22, 63, 0,
            7, 5, 0x81, 3, 8, 0, 10,
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
        let configuration = [9, 2, 25, 0, 1, 1, 0, 0xa0, 50, 9, 4, 0, 0, 1, 8, 6, 80, 0, 7, 5, 0x81, 2, 0, 2, 0];
        assert_eq!(boot_interface(&configuration), None);
    }
}
