use super::*;
use crate::dma::fake::HostDma;
use crate::virtio::queue::DeviceSide;
use crate::virtio::{QueueAddresses, feature, status};
use std::cell::{Cell, RefCell};

const ERR_INVALID_PARAMETER: u32 = 0x1205;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Command {
    kind: u32,
    fenced: bool,
    fence_id: u64,
}

#[derive(Debug, Default)]
struct FakeGpu {
    display: Option<(u32, u32)>,
    scanouts: u32,
    max_queue_size: u16,
    status: Cell<u8>,
    answers_left: Cell<Option<usize>>,
    fails: Cell<Option<u32>>,
    device: RefCell<Option<DeviceSide>>,
    notifies: Cell<usize>,
    log: RefCell<Vec<Command>>,
    resource: RefCell<Option<(u32, u32, u32)>>,
    backing: Cell<u64>,
    image: RefCell<Vec<u32>>,
    transfers: RefCell<Vec<(Rect, u64)>>,
    flushes: RefCell<Vec<Rect>>,
    scanout: RefCell<Option<(Rect, u32)>>,
}

fn read32(address: u64) -> u32 {
    // SAFETY: inside a buffer of the test's HostDma, whose device address is its host address.
    u32::from_le(unsafe { (address as *const u32).read_unaligned() })
}

fn read64(address: u64) -> u64 {
    // SAFETY: as in `read32`.
    u64::from_le(unsafe { (address as *const u64).read_unaligned() })
}

fn write32(address: u64, value: u32) {
    // SAFETY: inside a response buffer of the test's HostDma.
    unsafe { (address as *mut u32).write_unaligned(value.to_le()) }
}

fn read_rect(address: u64) -> Rect {
    Rect::new(
        read32(address),
        read32(address + 4),
        read32(address + 8),
        read32(address + 12),
    )
}

impl FakeGpu {
    fn new(display: Option<(u32, u32)>) -> Self {
        Self {
            display,
            scanouts: 1,
            max_queue_size: 64,
            ..Self::default()
        }
    }

    fn commands(&self) -> Vec<u32> {
        self.log
            .borrow()
            .iter()
            .map(|command| command.kind)
            .collect()
    }

    fn process(&self) {
        let mut device = self.device.borrow_mut();
        let device = device.as_mut().unwrap();
        while self.answers_left.get() != Some(0) {
            let Some(head) = device.next_available() else {
                break;
            };
            self.answers_left
                .set(self.answers_left.get().map(|left| left - 1));
            let (request, request_len, _, next) = device.descriptor(head);
            let (response, response_len, flags, _) = device.descriptor(next);
            assert_ne!(flags & crate::virtio::queue::DESCRIPTOR_WRITE, 0);
            let request = request.get();
            let kind = read32(request);
            let fenced = read32(request + 4) & FLAG_FENCE != 0;
            self.log.borrow_mut().push(Command {
                kind,
                fenced,
                fence_id: read64(request + 8),
            });
            let answer = self.answer(kind, request, request_len, response.get());
            let answer = self.fails.get().unwrap_or(answer);
            write32(response.get(), answer);
            write32(response.get() + 4, if fenced { FLAG_FENCE } else { 0 });
            device.put_used(head, response_len.min(DISPLAY_INFO_BYTES as u32));
        }
    }

    fn answer(&self, kind: u32, request: u64, len: u32, response: u64) -> u32 {
        match kind {
            command::GET_DISPLAY_INFO => {
                assert_eq!(len as usize, HEADER_BYTES);
                if let Some((width, height)) = self.display {
                    write32(response + 24 + 8, width);
                    write32(response + 24 + 12, height);
                    write32(response + 24 + 16, 1);
                }
                response::OK_DISPLAY_INFO
            }
            command::RESOURCE_CREATE_2D => {
                assert_eq!(len as usize, CREATE_2D_BYTES);
                let (id, format) = (read32(request + 24), read32(request + 28));
                let (width, height) = (read32(request + 32), read32(request + 36));
                assert_eq!(format, FORMAT_B8G8R8X8_UNORM);
                *self.resource.borrow_mut() = Some((id, width, height));
                *self.image.borrow_mut() = vec![0; (width * height) as usize];
                response::OK_NODATA
            }
            command::RESOURCE_ATTACH_BACKING => {
                assert_eq!(len as usize, ATTACH_BACKING_BYTES);
                assert_eq!(read32(request + 28), 1);
                let (_, width, height) = self.resource.borrow().unwrap();
                assert_eq!(read32(request + 40), width * height * 4);
                self.backing.set(read64(request + 32));
                response::OK_NODATA
            }
            command::SET_SCANOUT => {
                assert_eq!(len as usize, SET_SCANOUT_BYTES);
                assert_eq!(read32(request + 40), SCANOUT_ID);
                *self.scanout.borrow_mut() = Some((read_rect(request + 24), read32(request + 44)));
                response::OK_NODATA
            }
            command::TRANSFER_TO_HOST_2D => {
                assert_eq!(len as usize, TRANSFER_TO_HOST_2D_BYTES);
                let rect = read_rect(request + 24);
                let offset = read64(request + 40);
                let (id, width, _) = self.resource.borrow().unwrap();
                assert_eq!(read32(request + 48), id);
                let mut image = self.image.borrow_mut();
                for row in 0..rect.height {
                    for column in 0..rect.width {
                        let source = self.backing.get()
                            + offset
                            + u64::from(row) * u64::from(width) * 4
                            + u64::from(column) * 4;
                        let target = (rect.y + row) * width + rect.x + column;
                        image[target as usize] = read32(source);
                    }
                }
                self.transfers.borrow_mut().push((rect, offset));
                response::OK_NODATA
            }
            command::RESOURCE_FLUSH => {
                assert_eq!(len as usize, RESOURCE_FLUSH_BYTES);
                self.flushes.borrow_mut().push(read_rect(request + 24));
                response::OK_NODATA
            }
            _ => ERR_INVALID_PARAMETER,
        }
    }
}

impl Transport for FakeGpu {
    fn device_type(&self) -> u32 {
        device_type::GPU
    }
    fn status(&self) -> u8 {
        self.status.get()
    }
    fn set_status(&self, status: u8) {
        self.status.set(status);
    }
    fn device_features(&self) -> u64 {
        feature::VERSION_1 | 1
    }
    fn set_driver_features(&self, features: u64) {
        assert_eq!(features, feature::VERSION_1);
    }
    fn max_queue_size(&self, queue: u16) -> u16 {
        if queue < 2 { self.max_queue_size } else { 0 }
    }
    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        assert_eq!(queue, CONTROL_QUEUE);
        let memory = addresses.descriptors.get() as *mut u8;
        // SAFETY: the queue lives in the test's HostDma, whose device address is its host
        // address, for as long as the fake device is used.
        *self.device.borrow_mut() = Some(unsafe { DeviceSide::new(memory, size) });
        Ok(())
    }
    fn notify(&self, queue: u16) {
        assert_eq!(queue, CONTROL_QUEUE);
        self.notifies.set(self.notifies.get() + 1);
        self.process();
    }
    fn read_config8(&self, _offset: usize) -> u8 {
        0
    }
    fn read_config32(&self, offset: usize) -> u32 {
        match offset {
            NUM_SCANOUTS => self.scanouts,
            _ => 0,
        }
    }
}

fn polls(mut count: u32) -> impl FnMut() -> bool {
    move || {
        count = count.saturating_sub(1);
        count > 0
    }
}

fn gpu(fake: FakeGpu) -> VirtioGpu<FakeGpu, HostDma> {
    VirtioGpu::new(fake, HostDma::zeroed(1), &mut polls(10)).unwrap()
}

fn framebuffer(mode: Mode) -> HostDma {
    HostDma::zeroed(mode.bytes().div_ceil(4096))
}

fn shown(fake: FakeGpu) -> (VirtioGpu<FakeGpu, HostDma>, Mode) {
    let mut gpu = gpu(fake);
    let mode = gpu.display_mode();
    gpu.attach_framebuffer(framebuffer(mode), mode, &mut polls(10))
        .unwrap();
    (gpu, mode)
}

#[test]
fn the_display_mode_is_scanout_0_s_or_the_fallback() {
    assert_eq!(
        gpu(FakeGpu::new(Some((64, 48)))).display_mode(),
        Mode::new(64, 48)
    );
    assert_eq!(gpu(FakeGpu::new(None)).display_mode(), FALLBACK_MODE);
    let refusing = FakeGpu::new(Some((64, 48)));
    refusing.fails.set(Some(ERR_INVALID_PARAMETER));
    let gpu = gpu(refusing);
    assert_eq!(gpu.display_mode(), FALLBACK_MODE);
    assert_eq!(
        gpu.transport().status.get(),
        status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK
    );
}

#[test]
fn a_framebuffer_is_created_attached_and_shown_with_fenced_commands() {
    let (mut gpu, mode) = shown(FakeGpu::new(Some((64, 48))));
    assert_eq!(gpu.mode(), Some(mode));
    let fake = gpu.transport();
    assert_eq!(
        fake.commands(),
        [
            command::GET_DISPLAY_INFO,
            command::RESOURCE_CREATE_2D,
            command::RESOURCE_ATTACH_BACKING,
            command::SET_SCANOUT
        ]
    );
    let log = fake.log.borrow();
    assert!(log.iter().all(|command| command.fenced));
    assert!(
        log.windows(2)
            .all(|pair| pair[0].fence_id < pair[1].fence_id)
    );
    assert_eq!(*fake.resource.borrow(), Some((RESOURCE_ID, 64, 48)));
    assert_eq!(
        *fake.scanout.borrow(),
        Some((Rect::new(0, 0, 64, 48), RESOURCE_ID))
    );
    drop(log);
    let backing = fake.backing.get();
    let pixels = gpu.pixels().unwrap();
    assert_eq!(pixels.len(), 64 * 48);
    assert_eq!(pixels.as_ptr() as u64, backing);
}

#[test]
fn a_flush_transfers_exactly_the_damaged_pixels_then_flushes_them() {
    let (mut gpu, mode) = shown(FakeGpu::new(Some((64, 48))));
    let pixels = gpu.pixels().unwrap();
    for (index, pixel) in pixels.iter_mut().enumerate() {
        *pixel = index as u32 | 0x00ab_0000;
    }
    let rect = Rect::new(5, 7, 10, 3);
    assert_eq!(gpu.flush(&[rect], &mut polls(10)), Ok(1));
    let fake = gpu.transport();
    assert_eq!(
        *fake.transfers.borrow(),
        [(rect, (7 * mode.stride() + 5 * 4) as u64)]
    );
    assert_eq!(*fake.flushes.borrow(), [rect]);
    let image = fake.image.borrow();
    for y in 0..mode.height {
        for x in 0..mode.width {
            let index = (y * mode.width + x) as usize;
            let inside = (5..15).contains(&x) && (7..10).contains(&y);
            let expected = if inside {
                index as u32 | 0x00ab_0000
            } else {
                0
            };
            assert_eq!(image[index], expected, "pixel ({x}, {y})");
        }
    }
}

#[test]
fn every_transfer_is_answered_before_any_flush_is_sent() {
    let (mut gpu, _) = shown(FakeGpu::new(Some((64, 48))));
    let notifies = gpu.transport().notifies.get();
    let rects = [
        Rect::new(0, 0, 1, 1),
        Rect::new(10, 10, 2, 2),
        Rect::new(20, 0, 3, 3),
    ];
    assert_eq!(gpu.flush(&rects, &mut polls(10)), Ok(3));
    let fake = gpu.transport();
    assert_eq!(fake.notifies.get() - notifies, 2);
    assert_eq!(
        fake.commands()[4..],
        [
            command::TRANSFER_TO_HOST_2D,
            command::TRANSFER_TO_HOST_2D,
            command::TRANSFER_TO_HOST_2D,
            command::RESOURCE_FLUSH,
            command::RESOURCE_FLUSH,
            command::RESOURCE_FLUSH
        ]
    );
    assert_eq!(*fake.flushes.borrow(), rects);
}

#[test]
fn rects_beyond_one_batch_go_in_more_rounds() {
    let mut small = FakeGpu::new(Some((64, 48)));
    small.max_queue_size = 4;
    let (mut gpu, _) = shown(small);
    let rects: Vec<Rect> = (0..5).map(|index| Rect::new(index * 4, 0, 2, 2)).collect();
    assert_eq!(gpu.flush(&rects, &mut polls(10)), Ok(5));
    use command::{RESOURCE_FLUSH as F, TRANSFER_TO_HOST_2D as T};
    assert_eq!(
        gpu.transport().commands()[4..],
        [T, T, F, F, T, T, F, F, T, F]
    );
    assert_eq!(*gpu.transport().flushes.borrow(), rects);
}

#[test]
fn rects_are_clipped_to_the_framebuffer_and_empty_ones_skipped() {
    let (mut gpu, _) = shown(FakeGpu::new(Some((64, 48))));
    let rects = [
        Rect::new(60, 40, 10, 10),
        Rect::new(64, 0, 5, 5),
        Rect::new(0, 0, 0, 5),
    ];
    assert_eq!(gpu.flush(&rects, &mut polls(10)), Ok(1));
    assert_eq!(*gpu.transport().flushes.borrow(), [Rect::new(60, 40, 4, 8)]);
    let commands = gpu.transport().log.borrow().len();
    assert_eq!(gpu.flush(&rects[1..], &mut polls(10)), Ok(0));
    assert_eq!(gpu.transport().log.borrow().len(), commands);
}

#[test]
fn nothing_is_flushed_or_drawn_before_a_framebuffer_is_attached() {
    let mut gpu = gpu(FakeGpu::new(Some((64, 48))));
    assert!(gpu.pixels().is_none());
    assert_eq!(gpu.mode(), None);
    assert_eq!(
        gpu.flush(&[Rect::new(0, 0, 1, 1)], &mut polls(10)),
        Err(Error::NoScanout)
    );
}

#[test]
fn unusable_framebuffers_and_modes_are_handed_back() {
    let mut gpu = gpu(FakeGpu::new(Some((64, 48))));
    let mode = gpu.display_mode();
    let small = HostDma::zeroed(1);
    let (error, memory) = gpu
        .attach_framebuffer(small, mode, &mut polls(10))
        .unwrap_err();
    assert_eq!((error, memory.is_some()), (Error::MemoryTooSmall, true));
    for bad in [Mode::new(0, 48), Mode::new(64, 0), Mode::new(65536, 65536)] {
        let (error, memory) = gpu
            .attach_framebuffer(HostDma::zeroed(1), bad, &mut polls(10))
            .unwrap_err();
        assert_eq!((error, memory.is_some()), (Error::UnsupportedMode, true));
    }
    gpu.attach_framebuffer(framebuffer(mode), mode, &mut polls(10))
        .unwrap();
    let (error, memory) = gpu
        .attach_framebuffer(framebuffer(mode), mode, &mut polls(10))
        .unwrap_err();
    assert_eq!((error, memory.is_some()), (Error::ScanoutInUse, true));
}

#[test]
fn a_refused_resource_hands_the_framebuffer_back() {
    let mut gpu = gpu(FakeGpu::new(Some((64, 48))));
    let mode = gpu.display_mode();
    gpu.transport().fails.set(Some(ERR_INVALID_PARAMETER));
    let (error, memory) = gpu
        .attach_framebuffer(framebuffer(mode), mode, &mut polls(10))
        .unwrap_err();
    assert_eq!(error, Error::CommandFailed(ERR_INVALID_PARAMETER));
    assert!(memory.is_some());
    assert_eq!(gpu.mode(), None);
}

#[test]
fn a_framebuffer_the_device_was_given_comes_back_only_from_a_reset() {
    let mut gpu = gpu(FakeGpu::new(Some((64, 48))));
    let mode = gpu.display_mode();
    gpu.transport().answers_left.set(Some(1));
    let (error, memory) = gpu
        .attach_framebuffer(framebuffer(mode), mode, &mut polls(1))
        .unwrap_err();
    assert_eq!((error, memory.is_none()), (Error::TimedOut, true));
    assert_eq!(
        gpu.transport().commands(),
        [command::GET_DISPLAY_INFO, command::RESOURCE_CREATE_2D]
    );
    let released = gpu.reset(&mut polls(10)).unwrap();
    assert!(released.framebuffer.is_some());
    released.transport.answers_left.set(None);
    let mut gpu = VirtioGpu::new(released.transport, released.control, &mut polls(10)).unwrap();
    gpu.attach_framebuffer(framebuffer(mode), mode, &mut polls(10))
        .unwrap();
    let released = gpu.reset(&mut polls(10)).unwrap();
    assert!(released.framebuffer.is_some());
    assert_eq!(released.transport.status.get(), 0);
}

#[test]
fn a_device_that_stops_answering_fails_every_later_command() {
    let (mut gpu, _) = shown(FakeGpu::new(Some((64, 48))));
    gpu.transport().answers_left.set(Some(0));
    let rect = [Rect::new(0, 0, 4, 4)];
    assert_eq!(gpu.flush(&rect, &mut polls(3)), Err(Error::TimedOut));
    gpu.transport().answers_left.set(None);
    let commands = gpu.transport().log.borrow().len();
    assert_eq!(gpu.flush(&rect, &mut polls(3)), Err(Error::TimedOut));
    assert_eq!(gpu.transport().log.borrow().len(), commands);
}

#[test]
fn devices_the_driver_cannot_drive_are_refused() {
    let mut headless = FakeGpu::new(None);
    headless.scanouts = 0;
    let refused = VirtioGpu::new(headless, HostDma::zeroed(1), &mut polls(10)).unwrap_err();
    assert_eq!((refused.0, refused.2.is_some()), (Error::NoScanout, true));
    let mut no_queue = FakeGpu::new(None);
    no_queue.max_queue_size = 1;
    let refused = VirtioGpu::new(no_queue, HostDma::zeroed(1), &mut polls(10)).unwrap_err();
    assert_eq!(refused.0, Error::QueueUnavailable);
    let silent = FakeGpu::new(None);
    silent.answers_left.set(Some(0));
    let refused = VirtioGpu::new(silent, HostDma::zeroed(1), &mut polls(3)).unwrap_err();
    assert_eq!((refused.0, refused.2.is_some()), (Error::TimedOut, true));
    assert_eq!(refused.1.status.get(), 0);
}
