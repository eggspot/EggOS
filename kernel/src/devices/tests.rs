use super::*;
use eggos_drivers::pci::{Address, Function};
use std::sync::Mutex;

fn tree_device(compatible: &'static [u8]) -> Identity {
    Identity::DeviceTree {
        name: b"dev@0",
        compatible,
    }
}

fn pci_device(vendor: u16, device: u16, class: u8, subclass: u8) -> Identity {
    Identity::Pci(Function {
        address: Address::new(0, 3, 0).unwrap(),
        vendor_id: vendor,
        device_id: device,
        class,
        subclass,
        prog_if: 0,
        revision: 1,
        header_type: 0,
        interrupt_pin: 1,
        interrupt_line: 0,
    })
}

fn never(_: &mut dyn Probe, _: DeviceId) -> Result<(), ProbeError> {
    Err(ProbeError::Refused("not expected to probe"))
}

static GENERIC: DriverDescriptor = DriverDescriptor {
    name: "generic",
    pass: ProbePass::Device,
    matches: &[Match::Compatible(b"vendor,generic")],
    probe: never,
};

static SPECIFIC: DriverDescriptor = DriverDescriptor {
    name: "specific",
    pass: ProbePass::Device,
    matches: &[
        Match::Compatible(b"vendor,other"),
        Match::Compatible(b"vendor,board-specific"),
    ],
    probe: never,
};

static CLASS: DriverDescriptor = DriverDescriptor {
    name: "class",
    pass: ProbePass::Device,
    matches: &[Match::PciClass {
        class: 1,
        subclass: 8,
    }],
    probe: never,
};

static RANGE: DriverDescriptor = DriverDescriptor {
    name: "range",
    pass: ProbePass::Device,
    matches: &[Match::PciIdRange {
        vendor: 0x1af4,
        first: 0x1040,
        last: 0x107f,
    }],
    probe: never,
};

static EXACT: DriverDescriptor = DriverDescriptor {
    name: "exact",
    pass: ProbePass::Device,
    matches: &[Match::PciId {
        vendor: 0x1af4,
        device: 0x1042,
    }],
    probe: never,
};

static BUS_FOR_GENERIC: DriverDescriptor = DriverDescriptor {
    name: "bus",
    pass: ProbePass::Bus,
    matches: &[Match::Compatible(b"vendor,generic")],
    probe: never,
};

#[test]
fn the_most_specific_compatible_string_wins_whatever_the_driver_order() {
    let identity = tree_device(b"vendor,board-specific\0vendor,generic\0");
    for drivers in [[&GENERIC, &SPECIFIC], [&SPECIFIC, &GENERIC]] {
        let best = best_driver(&drivers, ProbePass::Device, &identity).unwrap();
        assert_eq!(best.name, "specific");
    }
    let generic_only = tree_device(b"vendor,generic\0");
    let best = best_driver(&[&SPECIFIC, &GENERIC], ProbePass::Device, &generic_only);
    assert_eq!(best.unwrap().name, "generic");
}

#[test]
fn a_pci_id_beats_an_id_range_which_beats_a_class() {
    let drivers = [&CLASS, &RANGE, &EXACT];
    let block = pci_device(0x1af4, 0x1042, 1, 8);
    assert_eq!(
        best_driver(&drivers, ProbePass::Device, &block)
            .unwrap()
            .name,
        "exact"
    );
    let rng = pci_device(0x1af4, 0x1044, 0xff, 0);
    assert_eq!(
        best_driver(&drivers, ProbePass::Device, &rng).unwrap().name,
        "range"
    );
    let transitional = pci_device(0x1af4, 0x1005, 0xff, 0);
    assert!(best_driver(&drivers, ProbePass::Device, &transitional).is_none());
    let past_the_range = pci_device(0x1af4, 0x1080, 0xff, 0);
    assert!(best_driver(&drivers, ProbePass::Device, &past_the_range).is_none());
    let nvme = pci_device(0x8086, 0x5845, 1, 8);
    assert_eq!(
        best_driver(&drivers, ProbePass::Device, &nvme)
            .unwrap()
            .name,
        "class"
    );
    assert!(best_driver(&drivers, ProbePass::Device, &pci_device(0x8086, 1, 2, 0)).is_none());
}

#[test]
fn equal_ranks_go_to_the_first_driver_listed_and_passes_are_kept_apart() {
    static TWIN: DriverDescriptor = DriverDescriptor {
        name: "twin",
        pass: ProbePass::Device,
        matches: &[Match::Compatible(b"vendor,generic")],
        probe: never,
    };
    let identity = tree_device(b"vendor,generic\0");
    let best = best_driver(&[&TWIN, &GENERIC], ProbePass::Device, &identity);
    assert_eq!(best.unwrap().name, "twin");
    let best = best_driver(&[&GENERIC, &TWIN], ProbePass::Device, &identity);
    assert_eq!(best.unwrap().name, "generic");
    let best = best_driver(&[&GENERIC, &BUS_FOR_GENERIC], ProbePass::Bus, &identity);
    assert_eq!(best.unwrap().name, "bus");
    assert!(best_driver(&[&GENERIC], ProbePass::InterruptController, &identity).is_none());
}

#[test]
fn matches_never_cross_bus_kinds() {
    let virtio = Identity::Virtio {
        device_type: 4,
        transport: 0,
    };
    assert_eq!(Match::VirtioId(4).rank(&virtio), Some(0));
    assert_eq!(Match::VirtioId(2).rank(&virtio), None);
    assert_eq!(Match::Compatible(b"virtio,mmio").rank(&virtio), None);
    let range = Match::PciIdRange {
        vendor: 0x1af4,
        first: 0,
        last: u16::MAX,
    };
    assert_eq!(range.rank(&tree_device(b"x\0")), None);
    assert_eq!(
        Match::AcpiHid(b"PNP0A08").rank(&pci_device(1, 2, 3, 4)),
        None
    );
    assert_eq!(
        Match::Compatible(b"vendor,generic").rank(&tree_device(b"vendor,generic-x\0")),
        None
    );
}

struct FakeProbe {
    table: DeviceTable,
}

impl Probe for FakeProbe {
    fn table(&mut self) -> &mut DeviceTable {
        &mut self.table
    }
    fn map_window(&mut self, region: PhysicalRange) -> Result<usize, ProbeError> {
        Ok(region.start as usize)
    }
    fn pci_config(&self) -> Option<&dyn ConfigSpace> {
        None
    }
    fn deadline(&self, _: u64) -> Deadline {
        Deadline {
            ticks: || 0,
            end: 1,
        }
    }
}

static PROBED: Mutex<Vec<(&'static str, usize)>> = Mutex::new(Vec::new());

fn bus_adding_a_child(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    PROBED.lock().unwrap().push(("bus", id.index()));
    let mut child = DeviceNode::new(Identity::Virtio {
        device_type: 4,
        transport: 0,
    });
    child.parent = Some(id);
    probe
        .table()
        .add(child)
        .ok_or(ProbeError::Refused("full"))?;
    Ok(())
}

fn device_driver(_: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    PROBED.lock().unwrap().push(("device", id.index()));
    Ok(())
}

fn finding_nothing(_: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    PROBED.lock().unwrap().push(("empty", id.index()));
    Err(ProbeError::Absent)
}

fn refusing(_: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    PROBED.lock().unwrap().push(("refusing", id.index()));
    Err(ProbeError::Refused("a legacy device"))
}

#[test]
fn passes_probe_buses_before_devices_and_the_children_buses_add() {
    static BUS: DriverDescriptor = DriverDescriptor {
        name: "test-bus",
        pass: ProbePass::Bus,
        matches: &[Match::Compatible(b"test,bus")],
        probe: bus_adding_a_child,
    };
    static DEVICE: DriverDescriptor = DriverDescriptor {
        name: "test-device",
        pass: ProbePass::Device,
        matches: &[Match::VirtioId(4)],
        probe: device_driver,
    };
    static REFUSING: DriverDescriptor = DriverDescriptor {
        name: "test-refusing",
        pass: ProbePass::InterruptController,
        matches: &[Match::Compatible(b"test,old")],
        probe: refusing,
    };
    static EMPTY_SLOT: DriverDescriptor = DriverDescriptor {
        name: "test-slot",
        pass: ProbePass::Bus,
        matches: &[Match::Compatible(b"test,slot")],
        probe: finding_nothing,
    };
    PROBED.lock().unwrap().clear();
    let mut probe = FakeProbe {
        table: DeviceTable::new(),
    };
    for compatible in [
        &b"test,bus\0"[..],
        b"test,old\0",
        b"test,nobody\0",
        b"test,slot\0",
    ] {
        probe.table.add(DeviceNode::new(tree_device(compatible)));
    }
    probe_all(&mut probe, &[&DEVICE, &BUS, &REFUSING, &EMPTY_SLOT]);
    assert_eq!(
        *PROBED.lock().unwrap(),
        [("refusing", 1), ("bus", 0), ("empty", 3), ("device", 4)]
    );
    let bindings: Vec<_> = probe.table.iter().map(|(_, node)| node.binding).collect();
    assert_eq!(
        bindings,
        [
            Binding::Bound("test-bus"),
            Binding::Refused("test-refusing", "a legacy device"),
            Binding::Unbound,
            Binding::Unbound,
            Binding::Bound("test-device"),
        ]
    );
    let child = probe.table.get(DeviceId(4)).unwrap();
    assert_eq!(child.parent, Some(DeviceId(0)));
}

#[test]
fn the_table_keeps_discovery_order() {
    let mut table = DeviceTable::new();
    assert!(table.is_empty());
    for index in 0..300 {
        let id = table.add(DeviceNode::new(pci_device(index, 0, 0, 0)));
        assert_eq!(id.map(DeviceId::index), Some(usize::from(index)));
    }
    assert_eq!((table.len(), table.dropped()), (300, 0));
    for (id, node) in table.iter() {
        let Identity::Pci(function) = node.identity else {
            panic!("not PCI");
        };
        assert_eq!(usize::from(function.vendor_id), id.index());
    }
}

#[test]
fn a_device_prints_its_identity_registers_and_driver() {
    let mut node = DeviceNode::new(tree_device(b"virtio,mmio\0"));
    node.regions[0] = PhysicalRange::from_start_length(0xa00_0000, 0x200);
    node.binding = Binding::Bound("virtio-mmio");
    assert_eq!(
        format!("{node}"),
        "dev@0 (virtio,mmio) 0xa000000..0xa000200: virtio-mmio"
    );
    let mut pci = DeviceNode::new(pci_device(0x1af4, 0x1044, 0xff, 0));
    pci.binding = Binding::Refused("virtio-pci", "no capabilities");
    assert_eq!(
        format!("{pci}"),
        "pci 00:03.0 1af4:1044 class ff00: virtio-pci refused: no capabilities"
    );
}

fn line_of(id: u16, node: &DeviceNode) -> String {
    let mut text = String::new();
    write_device_line(&mut text, DeviceId(id), node).unwrap();
    text
}

#[test]
fn devices_have_the_classes_a_person_recognises() {
    assert_eq!(class(&tree_device(b"arm,pl011\0arm,primecell\0")), "serial");
    assert_eq!(class(&tree_device(b"arm,pl031\0arm,primecell\0")), "clock");
    assert_eq!(class(&tree_device(b"google,goldfish-rtc\0")), "clock");
    assert_eq!(class(&tree_device(b"virtio,mmio\0")), "bus");
    assert_eq!(
        class(&tree_device(b"sifive,plic-1.0.0\0riscv,plic0\0")),
        "interrupt-controller"
    );
    assert_eq!(
        class(&tree_device(b"arm,cortex-a15-gic\0")),
        "interrupt-controller"
    );
    assert_eq!(class(&tree_device(b"qemu,fw-cfg-mmio\0")), "other");
    assert_eq!(class(&pci_device(0x8086, 0x2922, 0x01, 0x06)), "disk");
    assert_eq!(class(&pci_device(0x8086, 0x29c0, 0x06, 0x00)), "bus");
    let gpu = Identity::Virtio {
        device_type: eggos_drivers::virtio::device_type::GPU,
        transport: 0,
    };
    assert_eq!(class(&gpu), "display");
}

#[test]
fn a_device_s_line_has_ten_tab_separated_fields() {
    let mut gpu = DeviceNode::new(Identity::Virtio {
        device_type: eggos_drivers::virtio::device_type::GPU,
        transport: 0,
    });
    gpu.parent = Some(DeviceId(3));
    gpu.binding = Binding::Bound("virtio-gpu");
    assert_eq!(
        line_of(4, &gpu),
        "4\t3\tvirtio\tdisplay\tgpu\tvirtio gpu\tvirtio-gpu\tbound\tpolled\t-\n"
    );
    gpu.delivery = Delivery::Interrupts;
    assert!(line_of(4, &gpu).ends_with("\tbound\tinterrupts\t-\n"));
    let disk = DeviceNode::new(pci_device(0x8086, 0x2922, 0x01, 0x06));
    assert_eq!(
        line_of(1, &disk),
        "1\t-\tpci\tdisk\t00:03.0\t8086:2922 class 0106\t-\tunbound\t-\t-\n"
    );
    let mut slot = DeviceNode::new(Identity::DeviceTree {
        name: b"virtio_mmio@a000000",
        compatible: b"virtio,mmio\0",
    });
    slot.binding = Binding::Refused("virtio-mmio", "a legacy\tdevice");
    assert_eq!(
        line_of(0, &slot),
        "0\t-\tdevicetree\tbus\tvirtio_mmio@a000000\tvirtio,mmio\tvirtio-mmio\trefused\t-\ta legacy?device\n"
    );
    slot.binding = Binding::Bound("virtio-mmio");
    assert!(line_of(0, &slot).ends_with("\tvirtio-mmio\tbound\t-\t-\n"));
}
