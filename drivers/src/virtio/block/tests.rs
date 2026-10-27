use super::*;
use crate::dma::fake::HostDma;
use crate::virtio::queue::{DESCRIPTOR_NEXT, DESCRIPTOR_WRITE, DeviceSide};
use crate::virtio::{QueueAddresses, feature as common_feature, status as device_status};
use std::cell::{Cell, RefCell};

#[derive(Debug)]
struct FakeDisk {
    contents: RefCell<Vec<u8>>,
    read_only: bool,
    max_queue_size: u16,
    answers: Cell<bool>,
    failure: Cell<Option<u8>>,
    status: Cell<u8>,
    accepted: Cell<u64>,
    device: RefCell<Option<DeviceSide>>,
    requests: RefCell<Vec<(u32, u64, u32)>>,
    not_a_disk: bool,
}

impl FakeDisk {
    fn new(sectors: usize) -> Self {
        let contents = (0..sectors * SECTOR_SIZE)
            .map(|index| (index / SECTOR_SIZE * 7 + index % 251) as u8)
            .collect();
        Self {
            contents: RefCell::new(contents),
            read_only: false,
            max_queue_size: 256,
            answers: Cell::new(true),
            failure: Cell::new(None),
            status: Cell::new(0),
            accepted: Cell::new(0),
            device: RefCell::new(None),
            requests: RefCell::new(Vec::new()),
            not_a_disk: false,
        }
    }

    fn answer_requests(&self) {
        let mut device = self.device.borrow_mut();
        let device = device.as_mut().unwrap();
        while let Some(head) = device.next_available() {
            let (header, header_len, flags, data_index) = device.descriptor(head);
            assert_eq!(header_len as usize, HEADER_BYTES);
            assert_eq!(flags, DESCRIPTOR_NEXT);
            let (data, data_len, data_flags, status_index) = device.descriptor(data_index);
            assert_ne!(data_flags & DESCRIPTOR_NEXT, 0);
            let (status_at, status_len, status_flags, _) = device.descriptor(status_index);
            assert_eq!((status_len, status_flags), (1, DESCRIPTOR_WRITE));
            let header = header.get() as *const u8;
            // SAFETY: the request header the driver posted, inside the test's HostDma.
            let kind = u32::from_le(unsafe { header.cast::<u32>().read() });
            // SAFETY: the header's reserved field.
            let reserved = unsafe { header.wrapping_add(4).cast::<u32>().read() };
            // SAFETY: the header's sector field.
            let sector = u64::from_le(unsafe { header.wrapping_add(8).cast::<u64>().read() });
            assert_eq!(reserved, 0);
            self.requests.borrow_mut().push((kind, sector, data_len));
            let start = sector as usize * SECTOR_SIZE;
            let range = start..start + data_len as usize;
            let mut contents = self.contents.borrow_mut();
            let result = match (kind, self.failure.get()) {
                (_, Some(failure)) => failure,
                (request::IN, None) => {
                    assert_ne!(data_flags & DESCRIPTOR_WRITE, 0);
                    // SAFETY: the data buffer the driver posted for the device to write.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            contents[range].as_ptr(),
                            data.get() as *mut u8,
                            data_len as usize,
                        )
                    };
                    status::OK
                }
                (request::OUT, None) => {
                    assert_eq!(data_flags & DESCRIPTOR_WRITE, 0);
                    // SAFETY: the data buffer the driver posted for the device to read.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            data.get() as *const u8,
                            contents[range].as_mut_ptr(),
                            data_len as usize,
                        )
                    };
                    status::OK
                }
                _ => status::UNSUPPORTED,
            };
            // SAFETY: the status byte the driver posted for the device to write.
            unsafe { (status_at.get() as *mut u8).write(result) };
            let written = match kind {
                request::IN => data_len + 1,
                _ => 1,
            };
            device.put_used(head, written);
        }
    }
}

impl Transport for FakeDisk {
    fn device_type(&self) -> u32 {
        match self.not_a_disk {
            true => device_type::ENTROPY,
            false => device_type::BLOCK,
        }
    }
    fn status(&self) -> u8 {
        self.status.get()
    }
    fn set_status(&self, status: u8) {
        self.status.set(status);
    }
    fn device_features(&self) -> u64 {
        let read_only = match self.read_only {
            true => feature::READ_ONLY,
            false => 0,
        };
        common_feature::VERSION_1 | read_only | 1 << 9
    }
    fn set_driver_features(&self, features: u64) {
        self.accepted.set(features);
    }
    fn max_queue_size(&self, queue: u16) -> u16 {
        if queue == 0 { self.max_queue_size } else { 0 }
    }
    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        assert_eq!(queue, REQUEST_QUEUE);
        let memory = addresses.descriptors.get() as *mut u8;
        // SAFETY: the queue lives in the test's HostDma, whose device address is its host
        // address, for as long as the fake device is used.
        *self.device.borrow_mut() = Some(unsafe { DeviceSide::new(memory, size) });
        Ok(())
    }
    fn notify(&self, queue: u16) {
        assert_eq!(queue, REQUEST_QUEUE);
        if self.answers.get() {
            self.answer_requests();
        }
    }
    fn read_config8(&self, offset: usize) -> u8 {
        let capacity = (self.contents.borrow().len() / SECTOR_SIZE) as u64;
        capacity
            .to_le_bytes()
            .get(offset - config::CAPACITY_OFFSET)
            .copied()
            .unwrap_or(0)
    }
    fn write_config8(&self, offset: usize, _value: u8) {
        panic!("the driver wrote configuration byte {offset}");
    }
    fn read_config32(&self, offset: usize) -> u32 {
        u32::from_le_bytes(std::array::from_fn(|index| {
            self.read_config8(offset + index)
        }))
    }
}

fn polls(mut count: u32) -> impl FnMut() -> bool {
    move || {
        count = count.saturating_sub(1);
        count > 0
    }
}

fn memory() -> HostDma {
    HostDma::zeroed(MEMORY_BYTES / 4096)
}

fn disk(fake: FakeDisk) -> VirtioBlock<FakeDisk, HostDma> {
    VirtioBlock::new(fake, memory(), &mut polls(10)).unwrap()
}

fn sectors_of(fake: &FakeDisk, first: usize, count: usize) -> Vec<u8> {
    fake.contents.borrow()[first * SECTOR_SIZE..(first + count) * SECTOR_SIZE].to_vec()
}

#[test]
fn the_disk_reports_its_capacity_and_is_driven_once_set_up() {
    let block = disk(FakeDisk::new(100));
    assert_eq!(block.sectors(), 100);
    assert!(!block.is_read_only());
    let fake = block.transport();
    assert_eq!(
        fake.status.get(),
        device_status::ACKNOWLEDGE
            | device_status::DRIVER
            | device_status::FEATURES_OK
            | device_status::DRIVER_OK
    );
    assert_eq!(fake.accepted.get(), common_feature::VERSION_1);
    assert!(
        fake.device
            .borrow()
            .as_ref()
            .unwrap()
            .used_notifications_disabled()
    );
}

#[test]
fn a_read_only_disk_says_so_and_refuses_writes_without_a_request() {
    let mut block = disk(FakeDisk {
        read_only: true,
        ..FakeDisk::new(8)
    });
    assert!(block.is_read_only());
    assert_eq!(
        block.transport().accepted.get(),
        common_feature::VERSION_1 | feature::READ_ONLY
    );
    assert_eq!(
        block.write(0, &[0; SECTOR_SIZE], &mut polls(10)),
        Err(Error::ReadOnly)
    );
    assert!(block.transport().requests.borrow().is_empty());
}

#[test]
fn reads_return_the_sectors_asked_for_in_requests_of_at_most_the_largest_transfer() {
    let mut block = disk(FakeDisk::new(400));
    let mut one = [0u8; SECTOR_SIZE];
    block.read(5, &mut one, &mut polls(10)).unwrap();
    assert_eq!(one.to_vec(), sectors_of(block.transport(), 5, 1));
    let mut large = vec![0u8; 300 * SECTOR_SIZE];
    block.read(99, &mut large, &mut polls(10)).unwrap();
    assert_eq!(large, sectors_of(block.transport(), 99, 300));
    let per_request = (MAX_TRANSFER / SECTOR_SIZE) as u64;
    assert_eq!(
        *block.transport().requests.borrow(),
        vec![
            (request::IN, 5, SECTOR_SIZE as u32),
            (request::IN, 99, MAX_TRANSFER as u32),
            (request::IN, 99 + per_request, MAX_TRANSFER as u32),
            (
                request::IN,
                99 + 2 * per_request,
                (300 * SECTOR_SIZE - 2 * MAX_TRANSFER) as u32
            ),
        ]
    );
}

#[test]
fn writes_reach_the_disk_and_read_back() {
    let mut block = disk(FakeDisk::new(300));
    let data: Vec<u8> = (0..200 * SECTOR_SIZE)
        .map(|index| (index % 13) as u8)
        .collect();
    block.write(17, &data, &mut polls(10)).unwrap();
    assert_eq!(sectors_of(block.transport(), 17, 200), data);
    let mut back = vec![0u8; data.len()];
    block.read(17, &mut back, &mut polls(10)).unwrap();
    assert_eq!(back, data);
}

#[test]
fn transfers_past_the_end_or_of_part_of_a_sector_are_refused_without_a_request() {
    let mut block = disk(FakeDisk::new(10));
    let mut buffer = [0u8; 2 * SECTOR_SIZE];
    assert_eq!(
        block.read(9, &mut buffer, &mut polls(10)),
        Err(Error::BeyondCapacity)
    );
    assert_eq!(
        block.read(u64::MAX, &mut buffer, &mut polls(10)),
        Err(Error::BeyondCapacity)
    );
    assert_eq!(
        block.read(0, &mut buffer[..100], &mut polls(10)),
        Err(Error::PartialSector)
    );
    assert_eq!(
        block.write(0, &buffer[..SECTOR_SIZE + 1], &mut polls(10)),
        Err(Error::PartialSector)
    );
    block.read(8, &mut buffer, &mut polls(10)).unwrap();
    assert_eq!(block.transport().requests.borrow().len(), 1);
}

#[test]
fn a_failed_request_reports_the_device_s_status() {
    let mut block = disk(FakeDisk::new(10));
    block.transport().failure.set(Some(status::IO_ERROR));
    let mut buffer = [0u8; SECTOR_SIZE];
    assert_eq!(
        block.read(0, &mut buffer, &mut polls(10)),
        Err(Error::CommandFailed(status::IO_ERROR.into()))
    );
    block.transport().failure.set(None);
    block.read(1, &mut buffer, &mut polls(10)).unwrap();
    assert_eq!(buffer.to_vec(), sectors_of(block.transport(), 1, 1));
}

#[test]
fn an_unanswered_request_times_out_and_the_next_waits_for_it_first() {
    let mut block = disk(FakeDisk::new(10));
    block.transport().answers.set(false);
    let mut buffer = [0u8; SECTOR_SIZE];
    assert_eq!(
        block.read(2, &mut buffer, &mut polls(5)),
        Err(Error::TimedOut)
    );
    assert_eq!(
        block.read(3, &mut buffer, &mut polls(5)),
        Err(Error::TimedOut)
    );
    block.transport().answers.set(true);
    block.transport().answer_requests();
    block.read(4, &mut buffer, &mut polls(5)).unwrap();
    assert_eq!(buffer.to_vec(), sectors_of(block.transport(), 4, 1));
    assert_eq!(
        *block.transport().requests.borrow(),
        vec![
            (request::IN, 2, SECTOR_SIZE as u32),
            (request::IN, 4, SECTOR_SIZE as u32)
        ]
    );
}

#[test]
fn a_device_that_is_not_a_disk_or_has_too_small_a_queue_is_refused() {
    let refused = |fake: FakeDisk| match VirtioBlock::new(fake, memory(), &mut polls(10)) {
        Err((error, _, memory)) => {
            assert!(memory.is_some());
            error
        }
        Ok(_) => panic!("driven"),
    };
    let not_a_disk = FakeDisk {
        not_a_disk: true,
        ..FakeDisk::new(1)
    };
    assert_eq!(refused(not_a_disk), Error::WrongDeviceType);
    let small_queue = FakeDisk {
        max_queue_size: 2,
        ..FakeDisk::new(1)
    };
    assert_eq!(refused(small_queue), Error::QueueUnavailable);
    let small_memory = VirtioBlock::new(FakeDisk::new(1), HostDma::zeroed(1), &mut polls(10));
    assert!(matches!(small_memory, Err((Error::MemoryTooSmall, _, _))));
}

#[test]
fn a_queue_of_three_entries_holds_two_as_a_power_of_two_and_is_refused() {
    let fake = FakeDisk {
        max_queue_size: 3,
        ..FakeDisk::new(1)
    };
    assert!(matches!(
        VirtioBlock::new(fake, memory(), &mut polls(10)),
        Err((Error::QueueUnavailable, _, Some(_)))
    ));
    let fake = FakeDisk {
        max_queue_size: 4,
        ..FakeDisk::new(1)
    };
    assert!(VirtioBlock::new(fake, memory(), &mut polls(10)).is_ok());
}
