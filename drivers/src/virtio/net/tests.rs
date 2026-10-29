use super::*;
use crate::dma::fake::HostDma;
use crate::virtio::queue::{DESCRIPTOR_WRITE, DeviceSide};
use crate::virtio::{QueueAddresses, feature as common, status};
use std::cell::{Cell, RefCell};

const MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];

#[derive(Debug)]
struct FakeNet {
    features: u64,
    link: Cell<u16>,
    mtu: u16,
    max_queue_size: u16,
    status: Cell<u8>,
    accepted: Cell<u64>,
    queues: RefCell<[Option<DeviceSide>; 2]>,
    notified: RefCell<Vec<u16>>,
}

impl FakeNet {
    fn new(features: u64) -> Self {
        Self {
            features,
            link: Cell::new(config::LINK_UP),
            mtu: 1500,
            max_queue_size: 256,
            status: Cell::new(0),
            accepted: Cell::new(0),
            queues: RefCell::new([None, None]),
            notified: RefCell::new(Vec::new()),
        }
    }

    fn deliver(&self, frame: &[u8]) -> bool {
        let mut queues = self.queues.borrow_mut();
        let device = queues[0].as_mut().unwrap();
        let Some(head) = device.next_available() else {
            return false;
        };
        let (address, length, flags, _) = device.descriptor(head);
        assert_ne!(flags & DESCRIPTOR_WRITE, 0);
        assert_eq!(length as usize, BUFFER_BYTES);
        let mut bytes = vec![0u8; HEADER_BYTES];
        bytes.extend_from_slice(frame);
        // SAFETY: a receive buffer the driver posted, inside the test's HostDma, whose device
        // address is its host address, `BUFFER_BYTES` long, which the frames fit.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), address.get() as *mut u8, bytes.len())
        };
        device.put_used(head, bytes.len() as u32);
        true
    }

    fn sent(&self) -> Vec<Vec<u8>> {
        let mut queues = self.queues.borrow_mut();
        let device = queues[1].as_mut().unwrap();
        let mut frames = Vec::new();
        while let Some(head) = device.next_available() {
            let (address, length, flags, _) = device.descriptor(head);
            assert_eq!(flags & DESCRIPTOR_WRITE, 0);
            // SAFETY: a transmit buffer the driver handed over, inside the test's HostDma, whose
            // device address is its host address, `length` bytes long.
            let bytes =
                unsafe { std::slice::from_raw_parts(address.get() as *const u8, length as usize) };
            assert!(bytes[..HEADER_BYTES].iter().all(|&byte| byte == 0));
            frames.push(bytes[HEADER_BYTES..].to_vec());
            device.put_used(head, 0);
        }
        frames
    }
}

impl Transport for FakeNet {
    fn device_type(&self) -> u32 {
        device_type::NETWORK
    }
    fn status(&self) -> u8 {
        self.status.get()
    }
    fn set_status(&self, status: u8) {
        self.status.set(status);
    }
    fn device_features(&self) -> u64 {
        self.features | common::VERSION_1
    }
    fn set_driver_features(&self, features: u64) {
        self.accepted.set(features);
    }
    fn max_queue_size(&self, queue: u16) -> u16 {
        if queue < 2 { self.max_queue_size } else { 0 }
    }
    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        let memory = addresses.descriptors.get() as *mut u8;
        // SAFETY: the queue lives in the test's HostDma, whose device address is its host
        // address, for as long as the fake device is used.
        self.queues.borrow_mut()[usize::from(queue)] = Some(unsafe { DeviceSide::new(memory, size) });
        Ok(())
    }
    fn notify(&self, queue: u16) {
        self.notified.borrow_mut().push(queue);
    }
    fn read_config8(&self, offset: usize) -> u8 {
        let link = self.link.get().to_le_bytes();
        let mtu = self.mtu.to_le_bytes();
        match offset {
            0..6 => MAC[offset],
            6 | 7 => link[offset - 6],
            10 | 11 => mtu[offset - 10],
            _ => 0,
        }
    }
    fn write_config8(&self, offset: usize, _value: u8) {
        panic!("the driver wrote configuration byte {offset}");
    }
    fn read_config32(&self, offset: usize) -> u32 {
        u32::from_le_bytes(std::array::from_fn(|index| self.read_config8(offset + index)))
    }
}

fn polls(mut count: u32) -> impl FnMut() -> bool {
    move || {
        count = count.saturating_sub(1);
        count > 0
    }
}

fn driver(fake: FakeNet) -> VirtioNet<FakeNet, HostDma> {
    VirtioNet::new(fake, HostDma::zeroed(MEMORY_BYTES / 4096), &mut polls(10)).unwrap()
}

fn frame(fill: u8, length: usize) -> Vec<u8> {
    (0..length).map(|index| fill.wrapping_add(index as u8)).collect()
}

#[test]
fn the_device_is_set_up_with_its_address_link_and_every_receive_buffer_posted() {
    let net = driver(FakeNet::new(feature::MAC | feature::STATUS));
    let fake = net.transport();
    assert_eq!(
        fake.status.get(),
        status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK
    );
    assert_eq!(fake.accepted.get(), common::VERSION_1 | feature::MAC | feature::STATUS);
    assert_eq!(net.mac(), MAC);
    assert_eq!(net.mtu(), DEFAULT_MTU);
    assert!(net.is_link_up());
    fake.link.set(0);
    assert!(!net.is_link_up());
    assert_eq!(*fake.notified.borrow(), [RECEIVE_QUEUE]);
    for posted in 0..QUEUE_SIZE {
        assert!(fake.deliver(&frame(posted as u8, 60)), "buffer {posted}");
    }
    assert!(!fake.deliver(&frame(0, 60)), "only QUEUE_SIZE buffers are posted");
}

#[test]
fn a_device_that_says_its_mtu_or_not_its_link_is_believed_within_limits() {
    let mut fake = FakeNet::new(feature::MAC | feature::MTU);
    fake.mtu = 1400;
    fake.link.set(0);
    let net = driver(fake);
    assert_eq!(net.mtu(), 1400);
    assert!(net.is_link_up(), "a device that does not report its link is up");
    let mut huge = FakeNet::new(feature::MAC | feature::MTU);
    huge.mtu = 9000;
    assert_eq!(driver(huge).mtu(), DEFAULT_MTU);
}

#[test]
fn received_frames_come_without_the_header_and_their_buffers_go_back() {
    let mut net = driver(FakeNet::new(feature::MAC));
    let mut buffer = [0u8; MAX_FRAME_BYTES];
    assert_eq!(net.receive(&mut buffer), None);
    for round in 0..3u8 {
        for index in 0..QUEUE_SIZE as u8 {
            assert!(net.transport().deliver(&frame(round ^ index, 60 + usize::from(index))));
        }
        for index in 0..QUEUE_SIZE as u8 {
            let length = 60 + usize::from(index);
            assert_eq!(net.receive(&mut buffer), Some(length));
            assert_eq!(&buffer[..length], &frame(round ^ index, length)[..]);
        }
        assert_eq!(net.receive(&mut buffer), None);
    }
    assert!(net.transport().deliver(&frame(9, MAX_FRAME_BYTES)));
    let mut short = [0u8; 100];
    assert_eq!(net.receive(&mut short), Some(100), "a frame is cut to the caller's buffer");
    assert_eq!(&short[..], &frame(9, 100)[..]);
}

#[test]
fn sent_frames_go_with_a_zero_header_and_a_full_queue_waits_for_the_device() {
    let mut net = driver(FakeNet::new(feature::MAC));
    for index in 0..QUEUE_SIZE as u8 {
        net.transmit(&frame(index, 60)).unwrap();
    }
    assert_eq!(net.transmit(&frame(0, 60)), Err(Error::QueueFull));
    let sent = net.transport().sent();
    assert_eq!(sent.len(), usize::from(QUEUE_SIZE));
    assert!(sent.iter().enumerate().all(|(index, sent)| *sent == frame(index as u8, 60)));
    assert!(net.transport().notified.borrow().contains(&TRANSMIT_QUEUE));
    net.transmit(&frame(7, 1514)).unwrap();
    assert_eq!(net.transport().sent(), [frame(7, 1514)]);
    assert_eq!(net.transmit(&frame(0, MAX_FRAME_BYTES + 1)), Err(Error::FrameTooLong));
}

#[test]
fn a_device_without_a_hardware_address_or_memory_enough_is_refused() {
    let refused = VirtioNet::new(
        FakeNet::new(feature::STATUS),
        HostDma::zeroed(MEMORY_BYTES / 4096),
        &mut polls(10),
    );
    let Err((error, fake, memory)) = refused else {
        panic!("a device without a MAC was driven");
    };
    assert_eq!(error, Error::NoHardwareAddress);
    assert!(memory.is_some());
    assert_ne!(fake.status.get() & status::FAILED, 0);
    let small = VirtioNet::new(FakeNet::new(feature::MAC), HostDma::zeroed(1), &mut polls(10));
    assert!(matches!(small, Err((Error::MemoryTooSmall, _, Some(_)))));
}

#[test]
fn reset_hands_back_the_memory_with_its_proof() {
    let net = driver(FakeNet::new(feature::MAC));
    let (fake, memory, _) = net.reset(&mut polls(10)).unwrap();
    assert_eq!(fake.status.get(), 0);
    assert_eq!(memory.len(), MEMORY_BYTES);
}
