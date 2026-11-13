//! Which Intel adapter a PCI function is, and the firmware files it runs from the
//! `wifi-firmware` component (D265).

/// Intel's PCI vendor id.
pub const VENDOR_INTEL: u16 = 0x8086;
/// Where the drivers load the adapters' firmware from.
pub const FIRMWARE_FOLDER: &str = "/usr/lib/firmware/intel/iwlwifi";

const HW_REV_TYPE_MASK: u32 = 0x000f_fff0;
const HW_REV_STEP_MASK: u32 = 0x0000_000c;
const MAC_TYPE_QU: u32 = 0x0000_0330;
const MAC_TYPE_QUZ: u32 = 0x0000_0350;
const STEP_C0: u32 = 0x0000_0008;
const SUBDEVICE_SO: u16 = 0x0090;
const KILLER_GF4: [u16; 2] = [0x1691, 0x1692];

/// The generation of the adapter's host interface: how its firmware is handed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// AX200 and AX201: one context-info block names the firmware's sections.
    Ax200,
    /// AX210 and AX211: a context info that points at a scratch area and the image loader.
    Ax210,
}

/// How long the adapter's power management may delay its latency tolerance report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LtrDelay {
    None,
    Us200,
    Us1820,
    Us2500,
}

/// One adapter and how to run it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adapter {
    /// Its product name.
    pub name: &'static str,
    pub family: Family,
    /// Whether it is part of the platform's chipset (CNVi) rather than a card of its own.
    pub integrated: bool,
    pub ltr_delay: LtrDelay,
    /// The crystal's start-up latency in microseconds, 0 when the firmware's default is right.
    pub xtal_latency: u32,
    pub low_latency_xtal: bool,
    /// Whether the 6 GHz band may be used.
    pub ultra_high_band: bool,
    /// The firmware file's name in [`FIRMWARE_FOLDER`].
    pub firmware: &'static str,
    /// The platform NVM file the AX210 family loads after the firmware, if any.
    pub pnvm: Option<&'static str>,
    /// Whether the firmware's debug memory region is set up (integrated AX211s).
    pub imr: bool,
}

const CC_A0: &str = "iwlwifi-cc-a0-77.ucode";
const QU_B0_HR_B0: &str = "iwlwifi-Qu-b0-hr-b0-77.ucode";
const QU_C0_HR_B0: &str = "iwlwifi-Qu-c0-hr-b0-77.ucode";
const QUZ_A0_HR_B0: &str = "iwlwifi-QuZ-a0-hr-b0-77.ucode";
const TY_A0_GF_A0: &str = "iwlwifi-ty-a0-gf-a0-77.ucode";
const TY_A0_GF_A0_PNVM: &str = "iwlwifi-ty-a0-gf-a0.pnvm";
const SO_A0_GF_A0: &str = "iwlwifi-so-a0-gf-a0-77.ucode";
const SO_A0_GF_A0_PNVM: &str = "iwlwifi-so-a0-gf-a0.pnvm";

const AX201_IDS: [(u16, LtrDelay, u32, bool); 7] = [
    (0x02f0, LtrDelay::Us200, 500, false),
    (0x06f0, LtrDelay::Us200, 500, false),
    (0xa0f0, LtrDelay::Us200, 500, false),
    (0x34f0, LtrDelay::Us1820, 1820, false),
    (0x3df0, LtrDelay::Us1820, 1820, false),
    (0x4df0, LtrDelay::Us1820, 1820, false),
    (0x43f0, LtrDelay::Us2500, 12000, true),
];

const AX211_IDS: [(u16, bool); 7] = [
    (0x2726, false),
    (0x51f0, false),
    (0x51f1, true),
    (0x54f0, false),
    (0x7a70, true),
    (0x7af0, false),
    (0x7f70, false),
];

/// The adapter a PCI function with `device` and `subdevice` ids is, given its `CSR_HW_REV`
/// register `hw_rev`; `None` for one this driver does not run, or one whose firmware the
/// `wifi-firmware` component does not carry.
pub fn identify(device: u16, subdevice: u16, hw_rev: u32) -> Option<Adapter> {
    let mac = hw_rev & HW_REV_TYPE_MASK;
    let discrete = Adapter {
        name: "Wi-Fi 6 AX200",
        family: Family::Ax200,
        integrated: false,
        ltr_delay: LtrDelay::None,
        xtal_latency: 0,
        low_latency_xtal: false,
        ultra_high_band: false,
        firmware: CC_A0,
        pnvm: None,
        imr: false,
    };
    if device == 0x2723 {
        return Some(discrete);
    }
    if let Some(&(_, ltr_delay, xtal_latency, low_latency_xtal)) =
        AX201_IDS.iter().find(|(id, ..)| *id == device)
    {
        let firmware = match (mac, hw_rev & HW_REV_STEP_MASK) {
            (MAC_TYPE_QUZ, _) => QUZ_A0_HR_B0,
            (MAC_TYPE_QU, STEP_C0) => QU_C0_HR_B0,
            _ if matches!(device, 0x02f0 | 0x06f0) => return None,
            _ => QU_B0_HR_B0,
        };
        return Some(Adapter {
            name: "Wi-Fi 6 AX201",
            integrated: true,
            ltr_delay,
            xtal_latency,
            low_latency_xtal,
            firmware,
            ..discrete
        });
    }
    let six_e = Adapter {
        family: Family::Ax210,
        ultra_high_band: true,
        ..discrete
    };
    if device == 0x2725 {
        let (name, firmware, pnvm) = match subdevice {
            SUBDEVICE_SO => ("Wi-Fi 6E AX211", SO_A0_GF_A0, SO_A0_GF_A0_PNVM),
            _ => ("Wi-Fi 6E AX210", TY_A0_GF_A0, TY_A0_GF_A0_PNVM),
        };
        return Some(Adapter {
            name,
            firmware,
            pnvm: Some(pnvm),
            ultra_high_band: subdevice == SUBDEVICE_SO,
            ..six_e
        });
    }
    let &(_, integrated) = AX211_IDS.iter().find(|(id, _)| *id == device)?;
    if KILLER_GF4.contains(&subdevice) {
        return None;
    }
    let long_crystal = device == 0x7a70 || integrated;
    Some(Adapter {
        name: "Wi-Fi 6E AX211",
        integrated,
        imr: integrated,
        ltr_delay: if integrated {
            LtrDelay::Us2500
        } else {
            LtrDelay::None
        },
        xtal_latency: if long_crystal { 12000 } else { 0 },
        low_latency_xtal: long_crystal,
        firmware: SO_A0_GF_A0,
        pnvm: Some(SO_A0_GF_A0_PNVM),
        ..six_e
    })
}

/// Where the adapter's firmware file is.
pub fn firmware_path(adapter: &Adapter) -> alloc::string::String {
    alloc::format!("{FIRMWARE_FOLDER}/{}", adapter.firmware)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HW_REV_TYPE_QU_C0: u32 = 0x0000_0338;
    const HW_REV_TYPE_QUZ: u32 = 0x0000_0354;

    #[test]
    fn each_adapter_runs_the_firmware_its_silicon_needs() {
        assert_eq!(identify(0x2723, 0x0084, 0).unwrap().firmware, CC_A0);
        let quz = identify(0xa0f0, 0x0244, HW_REV_TYPE_QUZ).unwrap();
        assert_eq!(
            (quz.firmware, quz.integrated, quz.ltr_delay),
            (QUZ_A0_HR_B0, true, LtrDelay::Us200)
        );
        assert_eq!(
            identify(0x34f0, 0x0070, HW_REV_TYPE_QU_C0)
                .unwrap()
                .firmware,
            QU_C0_HR_B0
        );
        assert_eq!(
            identify(0x34f0, 0x0070, 0x0000_0330).unwrap().firmware,
            QU_B0_HR_B0
        );
        assert_eq!(identify(0x02f0, 0x0070, 0x0000_0330), None);
        let ty = identify(0x2725, 0x0020, 0).unwrap();
        assert_eq!(
            (ty.firmware, ty.pnvm, ty.family),
            (TY_A0_GF_A0, Some(TY_A0_GF_A0_PNVM), Family::Ax210)
        );
        assert_eq!(
            identify(0x2725, SUBDEVICE_SO, 0).unwrap().firmware,
            SO_A0_GF_A0
        );
        let so = identify(0x51f0, 0x0090, 0).unwrap();
        assert_eq!(
            (so.firmware, so.integrated, so.xtal_latency),
            (SO_A0_GF_A0, false, 0)
        );
        let integrated = identify(0x7a70, 0x0090, 0).unwrap();
        assert_eq!(
            (
                integrated.integrated,
                integrated.imr,
                integrated.xtal_latency
            ),
            (true, true, 12000)
        );
        assert!(!ty.ultra_high_band && so.ultra_high_band);
        assert_eq!(identify(0x51f0, 0x1691, 0), None);
        assert_eq!(identify(0x1234, 0, 0), None);
        assert_eq!(
            firmware_path(&ty),
            "/usr/lib/firmware/intel/iwlwifi/iwlwifi-ty-a0-gf-a0-77.ucode"
        );
    }
}
