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
    fails_only: Cell<Option<u32>>,
    device: RefCell<Option<DeviceSide>>,
    notifies: Cell<usize>,
    log: RefCell<Vec<Command>>,
    resource: RefCell<Option<(u32, u32, u32)>>,
    backing: Cell<u64>,
    image: RefCell<Vec<u32>>,
    transfers: RefCell<Vec<(Rect, u64)>>,
    flushes: RefCell<Vec<Rect>>,
    scanout: RefCell<Option<(Rect, u32)>>,
    no_cursor_queue: bool,
    cursor_device: RefCell<Option<DeviceSide>>,
    cursor_resource: RefCell<Option<(u32, u32)>>,
    cursor_backing: Cell<u64>,
    cursor_image: RefCell<Vec<u32>>,
    cursor_log: RefCell<Vec<Vec<u32>>>,
    three_d: bool,
    holds_streams: Cell<bool>,
    held_streams: RefCell<Vec<(u16, u64, u32)>>,
    streams: RefCell<Vec<(u32, Vec<u8>)>>,
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
            let request = request.get();
            let kind = read32(request);
            if kind == three_d::command::SUBMIT_3D {
                self.take_stream(device, head, request, next);
                continue;
            }
            let (response, response_len, flags, _) = device.descriptor(next);
            assert_ne!(flags & crate::virtio::queue::DESCRIPTOR_WRITE, 0);
            let fenced = read32(request + 4) & FLAG_FENCE != 0;
            self.log.borrow_mut().push(Command {
                kind,
                fenced,
                fence_id: read64(request + 8),
            });
            let answer = self.answer(kind, request, request_len, response.get());
            let failing = self.fails_only.get().is_none_or(|only| only == kind);
            let answer = self.fails.get().filter(|_| failing).unwrap_or(answer);
            write32(response.get(), answer);
            write32(response.get() + 4, if fenced { FLAG_FENCE } else { 0 });
            device.put_used(head, response_len.min(DISPLAY_INFO_BYTES as u32));
        }
    }

    fn take_stream(&self, device: &mut DeviceSide, head: u16, request: u64, next: u16) {
        let (stream, length, _, answer_next) = device.descriptor(next);
        let (answer, answer_len, flags, _) = device.descriptor(answer_next);
        assert_ne!(flags & crate::virtio::queue::DESCRIPTOR_WRITE, 0);
        assert_eq!(read32(request + 24), length);
        assert_ne!(read32(request + 4) & FLAG_FENCE, 0);
        // SAFETY: the stream lies in the test's HostDma, whose device address is its host
        // address, and the driver does not write it while it is in flight.
        let bytes =
            unsafe { std::slice::from_raw_parts(stream.get() as *const u8, length as usize) };
        self.streams
            .borrow_mut()
            .push((read32(request + 16), bytes.to_vec()));
        match self.holds_streams.get() {
            true => self
                .held_streams
                .borrow_mut()
                .push((head, answer.get(), answer_len)),
            false => {
                write32(answer.get(), three_d::answer::OK_NODATA);
                device.put_used(head, answer_len);
            }
        }
    }

    fn answer_held_stream(&self, index: usize) {
        let (head, answer, length) = self.held_streams.borrow_mut().remove(index);
        write32(answer, three_d::answer::OK_NODATA);
        self.device
            .borrow_mut()
            .as_mut()
            .unwrap()
            .put_used(head, length);
    }

    fn process_cursor(&self) {
        let mut device = self.cursor_device.borrow_mut();
        let device = device.as_mut().unwrap();
        while let Some(head) = device.next_available() {
            let (request, len, flags, _) = device.descriptor(head);
            assert_eq!(flags & crate::virtio::queue::DESCRIPTOR_WRITE, 0);
            assert_eq!(len as usize, UPDATE_CURSOR_BYTES);
            let words = (0..14).map(|word| read32(request.get() + 4 * word));
            self.cursor_log.borrow_mut().push(words.collect());
            device.put_used(head, 0);
        }
    }

    fn answer_cursor(&self, kind: u32, request: u64, len: u32) -> u32 {
        match kind {
            command::RESOURCE_CREATE_2D => {
                assert_eq!(read32(request + 28), FORMAT_B8G8R8A8_UNORM);
                let (width, height) = (read32(request + 32), read32(request + 36));
                *self.cursor_resource.borrow_mut() = Some((width, height));
                *self.cursor_image.borrow_mut() = vec![0; (width * height) as usize];
            }
            command::RESOURCE_ATTACH_BACKING => {
                assert_eq!(read32(request + 40) as usize, CURSOR_BYTES);
                self.cursor_backing.set(read64(request + 32));
            }
            command::TRANSFER_TO_HOST_2D => {
                assert_eq!(len as usize, TRANSFER_TO_HOST_2D_BYTES);
                assert_eq!(read_rect(request + 24), Rect::new(0, 0, 64, 64));
                let mut image = self.cursor_image.borrow_mut();
                for (index, pixel) in image.iter_mut().enumerate() {
                    *pixel = read32(self.cursor_backing.get() + 4 * index as u64);
                }
            }
            _ => return ERR_INVALID_PARAMETER,
        }
        response::OK_NODATA
    }

    fn answer(&self, kind: u32, request: u64, len: u32, response: u64) -> u32 {
        let resource = match kind {
            command::RESOURCE_CREATE_2D | command::RESOURCE_ATTACH_BACKING => read32(request + 24),
            command::TRANSFER_TO_HOST_2D => read32(request + 48),
            _ => 0,
        };
        if resource == CURSOR_RESOURCE_ID {
            return self.answer_cursor(kind, request, len);
        }
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

const FEATURE_EDID: u64 = 1 << 1;

impl FakeGpu {
    fn three_d_features(&self) -> u64 {
        match self.three_d {
            true => three_d::FEATURE_VIRGL,
            false => 0,
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
        feature::VERSION_1 | FEATURE_EDID | self.three_d_features()
    }
    fn set_driver_features(&self, features: u64) {
        assert_eq!(features, feature::VERSION_1 | self.three_d_features());
    }
    fn max_queue_size(&self, queue: u16) -> u16 {
        match queue {
            CONTROL_QUEUE => self.max_queue_size,
            CURSOR_QUEUE if !self.no_cursor_queue => self.max_queue_size,
            _ => 0,
        }
    }
    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        let memory = addresses.descriptors.get() as *mut u8;
        // SAFETY: the queue lives in the test's HostDma, whose device address is its host
        // address, for as long as the fake device is used.
        let device = Some(unsafe { DeviceSide::new(memory, size) });
        match queue {
            CONTROL_QUEUE => *self.device.borrow_mut() = device,
            CURSOR_QUEUE => *self.cursor_device.borrow_mut() = device,
            _ => panic!("no queue {queue}"),
        }
        Ok(())
    }
    fn notify(&self, queue: u16) {
        if queue == CURSOR_QUEUE {
            self.process_cursor();
            return;
        }
        assert_eq!(queue, CONTROL_QUEUE);
        self.notifies.set(self.notifies.get() + 1);
        self.process();
    }
    fn read_config8(&self, _offset: usize) -> u8 {
        0
    }
    fn write_config8(&self, _offset: usize, _value: u8) {}
    fn read_config32(&self, offset: usize) -> u32 {
        match offset {
            CONFIG_NUM_SCANOUTS_OFFSET => self.scanouts,
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
fn a_framebuffer_whose_scanout_failed_is_unseen_until_a_reset_hands_it_back() {
    let mut gpu = gpu(FakeGpu::new(Some((64, 48))));
    let mode = gpu.display_mode();
    gpu.transport().fails.set(Some(ERR_INVALID_PARAMETER));
    gpu.transport()
        .fails_only
        .set(Some(command::RESOURCE_ATTACH_BACKING));
    let (error, memory) = gpu
        .attach_framebuffer(framebuffer(mode), mode, &mut polls(10))
        .unwrap_err();
    assert_eq!(error, Error::CommandFailed(ERR_INVALID_PARAMETER));
    assert!(memory.is_none());
    assert_eq!(gpu.mode(), None);
    assert!(gpu.pixels().is_none());
    assert_eq!(
        gpu.flush(&[Rect::new(0, 0, 1, 1)], &mut polls(10)),
        Err(Error::NoScanout)
    );
    let (error, memory) = gpu
        .attach_framebuffer(framebuffer(mode), mode, &mut polls(10))
        .unwrap_err();
    assert_eq!((error, memory.is_some()), (Error::ScanoutInUse, true));
    assert!(gpu.reset(&mut polls(10)).unwrap().framebuffer.is_some());
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

#[test]
fn a_cursor_is_attached_shown_moved_and_hidden_through_the_cursor_queue() {
    let (mut gpu, _) = shown(FakeGpu::new(Some((64, 48))));
    assert!(gpu.has_cursor_queue());
    assert!(gpu.cursor_pixels().is_none());
    assert_eq!(
        gpu.show_cursor((0, 0), (1, 1), &mut polls(10)),
        Err(Error::NoCursor)
    );
    gpu.attach_cursor(HostDma::zeroed(4), &mut polls(10))
        .unwrap();
    let pixels = gpu.cursor_pixels().unwrap();
    assert_eq!(pixels.len(), 64 * 64);
    pixels[0] = 0xff00_0000;
    pixels[65] = 0xffff_ffff;
    gpu.show_cursor((4, 8), (100, 50), &mut polls(10)).unwrap();
    gpu.move_cursor((120, 60), &mut polls(10)).unwrap();
    gpu.hide_cursor(&mut polls(10)).unwrap();
    let fake = gpu.transport();
    assert_eq!(*fake.cursor_resource.borrow(), Some((64, 64)));
    assert_eq!(fake.cursor_image.borrow()[0], 0xff00_0000);
    assert_eq!(fake.cursor_image.borrow()[65], 0xffff_ffff);
    assert_eq!(
        fake.commands()[4..],
        [
            command::RESOURCE_CREATE_2D,
            command::RESOURCE_ATTACH_BACKING,
            command::TRANSFER_TO_HOST_2D
        ]
    );
    let log = fake.cursor_log.borrow();
    let fields = |words: &Vec<u32>| {
        (
            words[0], words[7], words[8], words[10], words[11], words[12],
        )
    };
    assert_eq!(log.len(), 3);
    assert_eq!(
        fields(&log[0]),
        (command::UPDATE_CURSOR, 100, 50, CURSOR_RESOURCE_ID, 4, 8)
    );
    assert_eq!(
        fields(&log[1]),
        (command::MOVE_CURSOR, 120, 60, CURSOR_RESOURCE_ID, 0, 0)
    );
    assert_eq!(
        fields(&log[2]),
        (command::UPDATE_CURSOR, 0, 0, NO_RESOURCE, 0, 0)
    );
    drop(log);
    let released = gpu.reset(&mut polls(10)).unwrap();
    assert!(released.cursor.is_some() && released.framebuffer.is_some());
}

#[test]
fn a_cursor_needs_the_cursor_queue_and_enough_memory() {
    let mut without = FakeGpu::new(Some((64, 48)));
    without.no_cursor_queue = true;
    let (mut gpu, _) = shown(without);
    assert!(!gpu.has_cursor_queue());
    let (error, memory) = gpu
        .attach_cursor(HostDma::zeroed(4), &mut polls(10))
        .unwrap_err();
    assert_eq!((error, memory.is_some()), (Error::NoCursor, true));
    assert_eq!(
        gpu.move_cursor((1, 1), &mut polls(10)),
        Err(Error::NoCursor)
    );
    let (mut gpu, _) = shown(FakeGpu::new(Some((64, 48))));
    let (error, memory) = gpu
        .attach_cursor(HostDma::zeroed(1), &mut polls(10))
        .unwrap_err();
    assert_eq!((error, memory.is_some()), (Error::MemoryTooSmall, true));
    gpu.attach_cursor(HostDma::zeroed(4), &mut polls(10))
        .unwrap();
    let (error, memory) = gpu
        .attach_cursor(HostDma::zeroed(4), &mut polls(10))
        .unwrap_err();
    assert_eq!((error, memory.is_some()), (Error::NoCursor, true));
}

fn three_d_gpu() -> VirtioGpu<FakeGpu, HostDma> {
    let fake = FakeGpu {
        three_d: true,
        ..FakeGpu::new(Some((64, 48)))
    };
    let mut gpu = gpu(fake);
    gpu.attach_three_d(HostDma::zeroed(THREE_D_BYTES.div_ceil(4096)))
        .unwrap();
    gpu
}

fn post_stream(gpu: &mut VirtioGpu<FakeGpu, HostDma>, context: u32, bytes: &[u8]) -> u64 {
    let slot = gpu.free_stream().unwrap();
    gpu.stream(slot).unwrap()[..bytes.len()].copy_from_slice(bytes);
    gpu.start_stream(slot, context, bytes.len() as u32).unwrap()
}

#[test]
fn several_streams_are_in_flight_and_fences_pass_by_the_oldest() {
    let mut gpu = three_d_gpu();
    gpu.transport().holds_streams.set(true);
    let first = post_stream(&mut gpu, 1, &[1, 2, 3, 4]);
    let second = post_stream(&mut gpu, 2, &[5, 6, 7, 8, 9, 10, 11, 12]);
    assert!(second > first);
    assert_eq!(gpu.free_stream(), None);
    assert_eq!(gpu.oldest_stream(), Some(first));
    gpu.transport().answer_held_stream(1);
    assert_eq!(gpu.oldest_stream(), Some(first));
    assert!(gpu.free_stream().is_some());
    gpu.transport().answer_held_stream(0);
    assert_eq!(gpu.oldest_stream(), None);
    assert!(!gpu.streams_in_flight());
    assert_eq!(
        *gpu.transport().streams.borrow(),
        vec![(1, vec![1, 2, 3, 4]), (2, vec![5, 6, 7, 8, 9, 10, 11, 12])]
    );
    assert_eq!(gpu.refused_streams(), 0);
}

#[test]
fn a_slot_in_flight_is_neither_written_nor_posted_again() {
    let mut gpu = three_d_gpu();
    gpu.transport().holds_streams.set(true);
    post_stream(&mut gpu, 1, &[1; 8]);
    assert!(gpu.stream(0).is_none());
    assert_eq!(gpu.start_stream(0, 1, 8), Err(Error::QueueFull));
    assert_eq!(
        gpu.start_stream(STREAM_SLOTS, 1, 8),
        Err(Error::QueueUnavailable)
    );
}

#[test]
fn a_flush_and_other_commands_run_while_streams_are_in_flight() {
    let mut gpu = three_d_gpu();
    let mode = gpu.display_mode();
    gpu.attach_framebuffer(framebuffer(mode), mode, &mut polls(10))
        .unwrap();
    gpu.transport().holds_streams.set(true);
    let fence = post_stream(&mut gpu, 1, &[7; 16]);
    assert_eq!(gpu.start_flush(&[Rect::new(0, 0, 4, 4)]), Ok(1));
    assert_eq!(gpu.finish_flush(), Ok(Some(1)));
    assert_eq!(gpu.flush(&[Rect::new(4, 4, 4, 4)], &mut polls(10)), Ok(1));
    assert_eq!(gpu.oldest_stream(), Some(fence));
    gpu.transport().answer_held_stream(0);
    assert_eq!(gpu.oldest_stream(), None);
}

#[test]
fn a_stream_goes_through_the_stream_slots_only() {
    let mut gpu = three_d_gpu();
    let submit = ThreeDCommand::Submit {
        context: 1,
        length: 4,
    };
    assert_eq!(gpu.start_three_d(&submit), Err(Error::QueueUnavailable));
}

#[test]
fn streams_left_unanswered_wedge_the_device_when_given_up() {
    let mut gpu = three_d_gpu();
    gpu.transport().holds_streams.set(true);
    post_stream(&mut gpu, 1, &[1; 4]);
    gpu.abandon_streams();
    assert!(!gpu.streams_in_flight());
    assert_eq!(gpu.start_stream(0, 1, 4), Err(Error::TimedOut));
}
