use super::mmio::{MAGIC, MmioTransport};
use super::pci::{self, PciTransport, VirtioPciCapability};
use super::queue::{self, Buffer, DESCRIPTOR_NEXT, DESCRIPTOR_WRITE, DeviceSide, SplitQueue, Used};
use super::rng::{self, VirtioRng};
use super::*;
use crate::dma::fake::HostDma;
use crate::dma::{DmaAddr, DmaMemory};
use crate::io::RegisterIo;
use crate::io::fake::FakeRegs;
use crate::pci::{Address, ConfigSpace};
use std::cell::{Cell, RefCell};

struct Shared<'a>(&'a FakeRegs);

impl RegisterIo for Shared<'_> {
    fn read8(&self, offset: usize) -> u8 {
        self.0.read8(offset)
    }
    fn write8(&self, offset: usize, value: u8) {
        self.0.write8(offset, value)
    }
    fn read16(&self, offset: usize) -> u16 {
        self.0.read16(offset)
    }
    fn write16(&self, offset: usize, value: u16) {
        self.0.write16(offset, value)
    }
    fn read32(&self, offset: usize) -> u32 {
        self.0.read32(offset)
    }
    fn write32(&self, offset: usize, value: u32) {
        self.0.write32(offset, value)
    }
}

impl std::fmt::Debug for Shared<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Shared")
    }
}

#[derive(Debug, Default)]
struct FakeDevice {
    device_type: u32,
    offered: u64,
    max_queue_size: u16,
    status: Cell<u8>,
    status_writes: RefCell<Vec<u8>>,
    driver_features: Cell<u64>,
    refuses_features: bool,
    reset_polls_before_zero: Cell<u32>,
    reset_never_completes: Cell<bool>,
    answers: Cell<bool>,
    queue: RefCell<Option<(u16, QueueAddresses)>>,
    device: RefCell<Option<DeviceSide>>,
    notifies: Cell<usize>,
}

impl FakeDevice {
    fn entropy() -> Self {
        Self {
            device_type: device_type::ENTROPY,
            offered: feature::VERSION_1,
            max_queue_size: 64,
            answers: Cell::new(true),
            ..Self::default()
        }
    }

    fn process(&self) {
        let mut device = self.device.borrow_mut();
        let device = device.as_mut().unwrap();
        while let Some(head) = device.next_available() {
            let (addr, len, _, _) = device.descriptor(head);
            for at in 0..len as usize {
                let byte = (addr.get() as usize + at) as *mut u8;
                // SAFETY: a buffer in the test's HostDma, whose device address is its host
                // address.
                unsafe { byte.write_volatile(0xa5) };
            }
            device.put_used(head, len);
        }
    }
}

impl Transport for FakeDevice {
    fn device_type(&self) -> u32 {
        self.device_type
    }
    fn status(&self) -> u8 {
        let status = self.status.get();
        if status == 0 && self.reset_never_completes.get() {
            return 1;
        }
        if status == 0 && self.reset_polls_before_zero.get() > 0 {
            self.reset_polls_before_zero
                .set(self.reset_polls_before_zero.get() - 1);
            return 1;
        }
        status
    }
    fn set_status(&self, status: u8) {
        self.status_writes.borrow_mut().push(status);
        let refused = self.refuses_features && status & status::FEATURES_OK != 0;
        self.status.set(if refused {
            status & !status::FEATURES_OK
        } else {
            status
        });
    }
    fn device_features(&self) -> u64 {
        self.offered
    }
    fn set_driver_features(&self, features: u64) {
        self.driver_features.set(features);
    }
    fn max_queue_size(&self, queue: u16) -> u16 {
        if queue == 0 { self.max_queue_size } else { 0 }
    }
    fn set_up_queue(
        &self,
        _queue: u16,
        size: u16,
        addresses: &QueueAddresses,
    ) -> Result<(), Error> {
        *self.queue.borrow_mut() = Some((size, *addresses));
        let memory = addresses.descriptors.get() as *mut u8;
        // SAFETY: the queue lives in the test's HostDma, whose device address is its host
        // address, for as long as the fake device is used.
        *self.device.borrow_mut() = Some(unsafe { DeviceSide::new(memory, size) });
        Ok(())
    }
    fn notify(&self, _queue: u16) {
        self.notifies.set(self.notifies.get() + 1);
        if self.answers.get() {
            self.process();
        }
    }
    fn read_config8(&self, _offset: usize) -> u8 {
        0
    }
    fn read_config32(&self, _offset: usize) -> u32 {
        0
    }
}

fn queue_in(memory: &HostDma, size: u16) -> (SplitQueue, DeviceSide) {
    // SAFETY: the HostDma is zeroed, page-aligned and large enough for the sizes the tests use,
    // and outlives the queue in each test; its device address is its host address.
    let queue = unsafe { SplitQueue::new(memory.as_mut_ptr(), memory.dma_addr(), size) }.unwrap();
    // SAFETY: as above.
    let device = unsafe { DeviceSide::new(memory.as_mut_ptr(), size) };
    (queue, device)
}

fn buffer(addr: u64, len: u32, device_writes: bool) -> Buffer {
    Buffer {
        addr: DmaAddr::new(addr),
        len,
        device_writes,
    }
}

fn never() -> impl FnMut() -> bool {
    || false
}

fn times(mut count: u32) -> impl FnMut() -> bool {
    move || {
        count = count.saturating_sub(1);
        count > 0
    }
}

#[test]
fn queue_indices_wrap_around_and_descriptors_are_reused() {
    let memory = HostDma::zeroed(1);
    let (mut queue, mut device) = queue_in(&memory, 4);
    for round in 0..70_000u32 {
        let head = queue.submit(&[buffer(0x1000, 8, true)]).unwrap();
        assert!(head < 4);
        assert_eq!(device.next_available(), Some(head));
        assert_eq!(device.next_available(), None);
        device.put_used(head, round % 9);
        assert_eq!(
            queue.pop_used(),
            Some(Used {
                head,
                len: round % 9
            })
        );
        assert_eq!(queue.pop_used(), None);
    }
    assert_eq!(queue.free_descriptors(), 4);
}

#[test]
fn several_chains_in_flight_wrap_around_in_order() {
    let memory = HostDma::zeroed(1);
    let (mut queue, mut device) = queue_in(&memory, 4);
    for _ in 0..20_000 {
        let heads: Vec<u16> = (0..4)
            .map(|_| queue.submit(&[buffer(0x2000, 4, false)]).unwrap())
            .collect();
        assert_eq!(queue.free_descriptors(), 0);
        let available: Vec<u16> = std::iter::from_fn(|| device.next_available()).collect();
        assert_eq!(available, heads);
        for &head in heads.iter().rev() {
            device.put_used(head, 0);
        }
        let used: Vec<u16> = std::iter::from_fn(|| queue.pop_used().map(|u| u.head)).collect();
        assert_eq!(used, heads.iter().rev().copied().collect::<Vec<_>>());
    }
}

#[test]
fn a_chain_reaches_the_device_in_order_and_is_freed_whole() {
    let memory = HostDma::zeroed(1);
    let (mut queue, mut device) = queue_in(&memory, 4);
    let chain = [
        buffer(0x1_0000_1000, 16, false),
        buffer(0x2000, 512, true),
        buffer(0x3000, 1, true),
    ];
    let head = queue.submit(&chain).unwrap();
    assert_eq!(queue.free_descriptors(), 1);
    assert_eq!(device.next_available(), Some(head));
    let mut index = head;
    for (position, expected) in chain.iter().enumerate() {
        let (addr, len, flags, next) = device.descriptor(index);
        assert_eq!((addr, len), (expected.addr, expected.len));
        let last = position == chain.len() - 1;
        assert_eq!(flags & DESCRIPTOR_NEXT != 0, !last);
        assert_eq!(flags & DESCRIPTOR_WRITE != 0, expected.device_writes);
        index = next;
    }
    device.put_used(head, 513);
    assert_eq!(queue.pop_used(), Some(Used { head, len: 513 }));
    assert_eq!(queue.free_descriptors(), 4);
    let again = queue.submit(&chain).unwrap();
    assert_eq!(device.next_available(), Some(again));
}

#[test]
fn a_full_queue_refuses_a_chain() {
    let memory = HostDma::zeroed(1);
    let (mut queue, _) = queue_in(&memory, 4);
    queue
        .submit(&[
            buffer(0, 1, false),
            buffer(0, 1, false),
            buffer(0, 1, false),
        ])
        .unwrap();
    assert_eq!(
        queue.submit(&[buffer(0, 1, false), buffer(0, 1, false)]),
        Err(Error::QueueFull)
    );
    assert_eq!(queue.submit(&[]), Err(Error::QueueFull));
    assert!(queue.submit(&[buffer(0, 1, false)]).is_ok());
    assert_eq!(queue.submit(&[buffer(0, 1, false)]), Err(Error::QueueFull));
}

#[test]
fn a_used_element_naming_no_descriptor_frees_nothing() {
    let memory = HostDma::zeroed(1);
    let (mut queue, mut device) = queue_in(&memory, 4);
    queue.submit(&[buffer(0, 1, false)]).unwrap();
    device.put_used(9, 0);
    assert_eq!(queue.pop_used(), Some(Used { head: 9, len: 0 }));
    assert_eq!(queue.free_descriptors(), 3);
}

#[test]
fn queue_sizes_must_be_powers_of_two_up_to_32768() {
    let memory = HostDma::zeroed(1);
    for size in [0, 3, 6, 65535] {
        // SAFETY: refused before anything is written.
        let queue = unsafe { SplitQueue::new(memory.as_mut_ptr(), memory.dma_addr(), size) };
        assert_eq!(queue.err(), Some(Error::QueueTooSmall));
    }
    let large = HostDma::zeroed(queue::memory_size(32768).div_ceil(4096));
    // SAFETY: large enough for 32768 entries.
    assert!(unsafe { SplitQueue::new(large.as_mut_ptr(), large.dma_addr(), 32768) }.is_ok());
}

#[test]
fn memory_size_lays_out_aligned_areas() {
    assert_eq!(queue::memory_size(1), 16 + 8 + 14);
    assert_eq!(queue::memory_size(8), 152 + 70);
    assert_eq!(queue::memory_size(32768), 589_832 + 262_150);
    let memory = HostDma::zeroed(1);
    let (queue, _) = queue_in(&memory, 8);
    let base = memory.dma_addr().get();
    let addresses = queue.addresses();
    assert_eq!(addresses.descriptors.get(), base);
    assert_eq!(addresses.driver.get(), base + 128);
    assert_eq!(addresses.device.get(), base + 152);
}

#[test]
fn the_driver_notifies_unless_the_device_says_not_to() {
    let memory = HostDma::zeroed(1);
    let (queue, mut device) = queue_in(&memory, 4);
    assert!(queue.should_notify());
    device.set_no_notify(true);
    assert!(!queue.should_notify());
    device.set_no_notify(false);
    assert!(queue.should_notify());
}

#[test]
fn reset_writes_zero_and_polls_until_it_reads_zero() {
    let device = FakeDevice::entropy();
    device.status.set(status::DRIVER_OK);
    device.reset_polls_before_zero.set(3);
    let mut polls = 0;
    assert!(
        reset(&device, &mut || {
            polls += 1;
            true
        })
        .is_ok()
    );
    assert_eq!(polls, 3);
    assert_eq!(*device.status_writes.borrow(), [0]);
}

#[test]
fn a_reset_that_never_reads_zero_times_out() {
    let device = FakeDevice::entropy();
    device.reset_never_completes.set(true);
    let mut polls = 0;
    let result = reset(&device, &mut || {
        polls += 1;
        polls < 5
    });
    assert_eq!(result.err(), Some(Error::ResetTimedOut));
    assert_eq!(polls, 5);
}

#[test]
fn initialization_accepts_offered_features_in_the_spec_s_order() {
    let wanted_bit = 1 << 5;
    let device = FakeDevice {
        offered: feature::VERSION_1
            | feature::ACCESS_PLATFORM
            | feature::ORDER_PLATFORM
            | wanted_bit
            | 1 << 6,
        ..FakeDevice::entropy()
    };
    let accepted = initialize(&device, wanted_bit | 1 << 7, &mut never()).unwrap();
    let expected = feature::VERSION_1 | feature::ACCESS_PLATFORM | wanted_bit;
    assert_eq!(accepted, expected);
    assert_eq!(device.driver_features.get(), expected);
    driver_ok(&device);
    use status::*;
    assert_eq!(
        *device.status_writes.borrow(),
        [
            0,
            ACKNOWLEDGE,
            ACKNOWLEDGE | DRIVER,
            ACKNOWLEDGE | DRIVER | FEATURES_OK,
            ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK
        ]
    );
}

#[test]
fn a_device_without_version_1_is_refused_and_failed() {
    let device = FakeDevice {
        offered: 1 << 5,
        ..FakeDevice::entropy()
    };
    assert_eq!(
        initialize(&device, 1 << 5, &mut never()),
        Err(Error::MissingFeatures)
    );
    assert_ne!(device.status.get() & status::FAILED, 0);
    assert_eq!(device.driver_features.get(), 0);
}

#[test]
fn features_the_device_refuses_fail_it() {
    let device = FakeDevice {
        refuses_features: true,
        ..FakeDevice::entropy()
    };
    assert_eq!(
        initialize(&device, 0, &mut never()),
        Err(Error::FeaturesRefused)
    );
    assert_ne!(device.status.get() & status::FAILED, 0);
}

fn mmio_regs(version: u32, device_id: u32) -> FakeRegs {
    let regs = FakeRegs::default();
    regs.script_reads(0x000, &[MAGIC]);
    regs.script_reads(0x004, &[version]);
    regs.script_reads(0x008, &[device_id]);
    regs
}

#[test]
fn mmio_probing_accepts_only_version_2_devices() {
    let regs = mmio_regs(2, 4);
    assert_eq!(
        MmioTransport::new(Shared(&regs)).unwrap().device_type(),
        device_type::ENTROPY
    );
    let legacy = MmioTransport::new(Shared(&mmio_regs(1, 4))).err();
    assert_eq!(legacy, Some(Error::LegacyVersion));
    assert!(Error::LegacyVersion.description().contains("version 2"));
    assert!(
        Error::LegacyVersion
            .description()
            .contains("force-legacy=false")
    );
    assert_eq!(
        MmioTransport::new(Shared(&mmio_regs(3, 4))).err(),
        Some(Error::UnsupportedVersion(3))
    );
    assert_eq!(
        MmioTransport::new(Shared(&mmio_regs(2, 0))).err(),
        Some(Error::NoDevice)
    );
    let not_virtio = FakeRegs::default();
    not_virtio.script_reads(0x000, &[0x1234]);
    assert_eq!(
        MmioTransport::new(Shared(&not_virtio)).err(),
        Some(Error::NotVirtio)
    );
}

#[test]
fn mmio_registers_are_written_at_the_spec_s_offsets() {
    let regs = mmio_regs(2, 4);
    let transport = MmioTransport::new(Shared(&regs)).unwrap();
    regs.log.borrow_mut().clear();
    regs.script_reads(0x010, &[0x20, 0x3]);
    assert_eq!(transport.device_features(), 0x3_0000_0020);
    transport.set_driver_features(0x1_0000_0020);
    regs.script_reads(0x044, &[0]);
    regs.script_reads(0x034, &[256]);
    assert_eq!(transport.max_queue_size(1), 256);
    regs.script_reads(0x044, &[1]);
    assert_eq!(transport.max_queue_size(1), 0);
    let addresses = QueueAddresses {
        descriptors: DmaAddr::new(0x1_2345_6000),
        driver: DmaAddr::new(0x2_0000_0080),
        device: DmaAddr::new(0x98),
    };
    transport.set_up_queue(1, 8, &addresses).unwrap();
    transport.notify(1);
    transport.set_status(status::ACKNOWLEDGE);
    assert_eq!(
        regs.writes(),
        [
            (0x014, 0),
            (0x014, 1),
            (0x024, 0),
            (0x020, 0x20),
            (0x024, 1),
            (0x020, 1),
            (0x030, 1),
            (0x030, 1),
            (0x030, 1),
            (0x038, 8),
            (0x080, 0x2345_6000),
            (0x084, 1),
            (0x090, 0x80),
            (0x094, 2),
            (0x0a0, 0x98),
            (0x0a4, 0),
            (0x044, 1),
            (0x050, 1),
            (0x070, 1),
        ]
    );
}

struct FakeConfig {
    bytes: RefCell<[u8; 256]>,
}

impl FakeConfig {
    fn put(&self, offset: usize, bytes: &[u8]) {
        self.bytes.borrow_mut()[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    fn virtio_cap(&self, at: usize, next: u8, cfg_type: u8, bar: u8, offset: u32, length: u32) {
        self.put(at, &[0x09, next, 16, cfg_type, bar, 0, 0, 0]);
        self.put(at + 8, &offset.to_le_bytes());
        self.put(at + 12, &length.to_le_bytes());
    }
}

impl ConfigSpace for FakeConfig {
    fn read32(&self, _address: Address, offset: u16) -> u32 {
        let at = usize::from(offset);
        u32::from_le_bytes(self.bytes.borrow()[at..at + 4].try_into().unwrap())
    }
    fn write32(&self, _address: Address, offset: u16, value: u32) {
        self.put(usize::from(offset), &value.to_le_bytes());
    }
}

#[test]
fn virtio_capabilities_are_found_in_list_order() {
    let config = FakeConfig {
        bytes: RefCell::new([0; 256]),
    };
    config.put(0x06, &[0x10, 0]);
    config.put(0x34, &[0x40]);
    config.virtio_cap(0x40, 0x54, pci::COMMON, 4, 0, 0x1000);
    config.virtio_cap(0x54, 0x68, pci::ISR, 4, 0x1000, 0x1000);
    config.virtio_cap(0x68, 0x7c, pci::DEVICE, 4, 0x2000, 0x1000);
    config.virtio_cap(0x7c, 0x94, pci::NOTIFY, 4, 0x3000, 0x1000);
    config.put(0x7c + 16, &4u32.to_le_bytes());
    config.virtio_cap(0x94, 0xa8, pci::PCI, 0, 0, 4);
    config.virtio_cap(0xa8, 0xbc, pci::COMMON, 7, 0, 0x1000);
    config.put(0xbc, &[0x11, 0]);
    let address = Address::new(0, 3, 0).unwrap();
    let found: Vec<VirtioPciCapability> = pci::virtio_capabilities(&config, address).collect();
    let cap = |cfg_type, bar, offset, length, notify_off_multiplier| VirtioPciCapability {
        cfg_type,
        bar,
        offset,
        length,
        notify_off_multiplier,
    };
    assert_eq!(
        found,
        [
            cap(pci::COMMON, 4, 0, 0x1000, 0),
            cap(pci::ISR, 4, 0x1000, 0x1000, 0),
            cap(pci::DEVICE, 4, 0x2000, 0x1000, 0),
            cap(pci::NOTIFY, 4, 0x3000, 0x1000, 4),
            cap(pci::PCI, 0, 0, 4, 0),
        ]
    );
}

#[test]
fn only_modern_virtio_pci_device_ids_are_driven() {
    assert_eq!(pci::modern_device_type(0x1040), Some(0));
    assert_eq!(pci::modern_device_type(0x1044), Some(device_type::ENTROPY));
    assert_eq!(pci::modern_device_type(0x107f), Some(0x3f));
    assert_eq!(pci::modern_device_type(0x1005), None);
    assert_eq!(pci::modern_device_type(0x1080), None);
}

#[test]
fn pci_common_registers_take_their_widths_and_notify_its_offset() {
    let common = FakeRegs::default();
    let notify = FakeRegs::default();
    let transport = PciTransport::new(
        device_type::ENTROPY,
        Shared(&common),
        Shared(&notify),
        0x1000,
        4,
        None,
    );
    common.script_reads(0x12, &[1, 1]);
    common.script_reads(0x1c, &[0]);
    common.script_reads(0x18, &[256]);
    assert_eq!(transport.max_queue_size(0), 256);
    assert_eq!(transport.max_queue_size(1), 0);
    common.script_reads(0x1e, &[3]);
    let addresses = QueueAddresses {
        descriptors: DmaAddr::new(0x1_0000_2000),
        driver: DmaAddr::new(0x2080),
        device: DmaAddr::new(0x2098),
    };
    transport.set_up_queue(0, 8, &addresses).unwrap();
    transport.notify(0);
    common.script_reads(0x14, &[status::DRIVER.into()]);
    assert_eq!(transport.status(), status::DRIVER);
    assert_eq!(
        common.writes(),
        [
            (0x16, 0),
            (0x16, 0),
            (0x16, 0),
            (0x18, 8),
            (0x20, 0x2000),
            (0x24, 1),
            (0x28, 0x2080),
            (0x2c, 0),
            (0x30, 0x2098),
            (0x34, 0),
            (0x1c, 1),
        ]
    );
    assert_eq!(notify.writes(), [(12, 0)]);
}

#[test]
fn a_notify_offset_outside_the_mapped_region_is_refused() {
    let common = FakeRegs::default();
    let notify = FakeRegs::default();
    let transport = PciTransport::new(4, Shared(&common), Shared(&notify), 0x1000, 4, None);
    common.script_reads(0x1e, &[0x400]);
    let addresses = QueueAddresses {
        descriptors: DmaAddr::new(0),
        driver: DmaAddr::new(0),
        device: DmaAddr::new(0),
    };
    assert_eq!(
        transport.set_up_queue(0, 8, &addresses),
        Err(Error::BarOutOfRange)
    );
    assert!(!common.writes().contains(&(0x1c, 1)));
}

#[derive(Debug)]
struct HalfPage(HostDma);

// SAFETY: a part of a HostDma, which meets every requirement.
unsafe impl DmaMemory for HalfPage {
    fn as_mut_ptr(&self) -> *mut u8 {
        self.0.as_mut_ptr()
    }
    fn dma_addr(&self) -> DmaAddr {
        self.0.dma_addr()
    }
    fn len(&self) -> usize {
        2048
    }
}

fn rng_with(device: &FakeDevice) -> VirtioRng<&FakeDevice, HostDma> {
    VirtioRng::new(device, HostDma::zeroed(1), &mut never())
        .map_err(|(error, _, _)| error)
        .unwrap()
}

impl Transport for &FakeDevice {
    fn device_type(&self) -> u32 {
        (**self).device_type()
    }
    fn status(&self) -> u8 {
        (**self).status()
    }
    fn set_status(&self, status: u8) {
        (**self).set_status(status)
    }
    fn device_features(&self) -> u64 {
        (**self).device_features()
    }
    fn set_driver_features(&self, features: u64) {
        (**self).set_driver_features(features)
    }
    fn max_queue_size(&self, queue: u16) -> u16 {
        (**self).max_queue_size(queue)
    }
    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        (**self).set_up_queue(queue, size, addresses)
    }
    fn notify(&self, queue: u16) {
        (**self).notify(queue)
    }
    fn read_config8(&self, offset: usize) -> u8 {
        (**self).read_config8(offset)
    }
    fn read_config32(&self, offset: usize) -> u32 {
        (**self).read_config32(offset)
    }
}

#[test]
fn the_rng_returns_the_device_s_bytes() {
    let device = FakeDevice::entropy();
    let mut rng = rng_with(&device);
    assert_eq!(device.queue.borrow().unwrap().0, rng::QUEUE_SIZE);
    assert_eq!(
        device.status_writes.borrow().last(),
        Some(&(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK))
    );
    let mut bytes = [0u8; 32];
    assert_eq!(rng.read(&mut bytes, &mut never()), Ok(32));
    assert_eq!(bytes, [0xa5; 32]);
    let mut long = vec![0u8; 3000];
    assert_eq!(rng.read(&mut long, &mut never()), Ok(rng::BUFFER_LEN));
    assert!(long[..rng::BUFFER_LEN].iter().all(|&b| b == 0xa5));
    assert!(long[rng::BUFFER_LEN..].iter().all(|&b| b == 0));
    assert_eq!(rng.read(&mut [], &mut never()), Ok(0));
    assert_eq!(device.notifies.get(), 2);
}

#[test]
fn the_rng_takes_a_smaller_queue_rounded_down_to_a_power_of_two() {
    let device = FakeDevice {
        max_queue_size: 6,
        ..FakeDevice::entropy()
    };
    let _rng = rng_with(&device);
    assert_eq!(device.queue.borrow().unwrap().0, 4);
}

#[test]
fn a_device_that_never_answers_times_out_and_its_request_is_reclaimed_later() {
    let device = FakeDevice::entropy();
    device.answers.set(false);
    let mut rng = rng_with(&device);
    let mut bytes = [0u8; 16];
    assert_eq!(rng.read(&mut bytes, &mut times(10)), Err(Error::TimedOut));
    assert_eq!(rng.read(&mut bytes, &mut times(10)), Err(Error::TimedOut));
    assert_eq!(device.notifies.get(), 1);
    device.process();
    device.answers.set(true);
    assert_eq!(rng.read(&mut bytes, &mut never()), Ok(16));
    assert_eq!(bytes, [0xa5; 16]);
}

#[test]
fn reset_hands_the_memory_back_with_its_proof() {
    let device = FakeDevice::entropy();
    let rng = rng_with(&device);
    let (_, memory, _done) = rng.reset(&mut never()).unwrap();
    assert_eq!(memory.len(), 4096);
    assert_eq!(device.status.get(), 0);
}

#[test]
fn a_reset_that_does_not_complete_keeps_the_memory_from_the_caller() {
    let device = FakeDevice::entropy();
    let rng = rng_with(&device);
    device.reset_never_completes.set(true);
    assert_eq!(rng.reset(&mut times(3)).err(), Some(Error::ResetTimedOut));
}

#[test]
fn the_rng_refuses_other_devices_small_memory_and_missing_queues() {
    let block = FakeDevice {
        device_type: device_type::BLOCK,
        ..FakeDevice::entropy()
    };
    let (error, _, memory) = VirtioRng::new(&block, HostDma::zeroed(1), &mut never()).unwrap_err();
    assert_eq!((error, memory.is_some()), (Error::WrongDeviceType, true));
    let device = FakeDevice::entropy();
    let tiny = HalfPage(HostDma::zeroed(1));
    let (error, _, _) = VirtioRng::new(&device, tiny, &mut never()).unwrap_err();
    assert_eq!(error, Error::MemoryTooSmall);
    let queueless = FakeDevice {
        max_queue_size: 0,
        ..FakeDevice::entropy()
    };
    let (error, _, memory) =
        VirtioRng::new(&queueless, HostDma::zeroed(1), &mut never()).unwrap_err();
    assert_eq!((error, memory.is_some()), (Error::QueueUnavailable, true));
    assert_ne!(queueless.status.get() & status::FAILED, 0);
}

#[test]
fn every_device_type_has_a_name() {
    assert_eq!(device_type_name(device_type::ENTROPY), "entropy");
    assert_eq!(device_type_name(device_type::GPU), "gpu");
    assert_eq!(device_type_name(99), "unknown");
}
