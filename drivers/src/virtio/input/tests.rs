use super::*;
use crate::dma::fake::HostDma;
use crate::virtio::queue::DeviceSide;
use crate::virtio::{QueueAddresses, feature, status};
use std::cell::{Cell, RefCell};

#[derive(Debug, Default)]
struct FakeInput {
    keys: Vec<u16>,
    relatives: Vec<u16>,
    absolutes: Vec<(u16, i32, i32)>,
    max_queue_size: u16,
    status: Cell<u8>,
    selected: Cell<(u8, u8)>,
    device: RefCell<Option<DeviceSide>>,
    posted: RefCell<Vec<u16>>,
    notifies: Cell<usize>,
    notified_before_driver_ok: Cell<bool>,
    not_input: bool,
}

fn write_event(address: u64, event: RawEvent) {
    let bytes = [
        event.kind.to_le_bytes().as_slice(),
        &event.code.to_le_bytes(),
        &event.value.to_le_bytes(),
    ]
    .concat();
    // SAFETY: an event buffer the driver posted, inside the test's HostDma, whose device
    // address is its host address.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, 8) };
}

impl FakeInput {
    fn keyboard() -> Self {
        Self {
            keys: (1..=88)
                .chain([key::KEYPAD_ENTER, key::LEFT_SUPER])
                .collect(),
            max_queue_size: 64,
            ..Self::default()
        }
    }

    fn mouse() -> Self {
        Self {
            keys: vec![button::LEFT, button::RIGHT, button::MIDDLE],
            relatives: vec![relative::X, relative::Y, relative::WHEEL],
            max_queue_size: 64,
            ..Self::default()
        }
    }

    fn tablet() -> Self {
        Self {
            keys: vec![button::LEFT, button::RIGHT, button::MIDDLE],
            relatives: vec![relative::WHEEL],
            absolutes: vec![(absolute::X, 0, 32767), (absolute::Y, -100, 100)],
            max_queue_size: 64,
            ..Self::default()
        }
    }

    fn bitmap(codes: &[u16]) -> Vec<u8> {
        let size = codes.iter().map(|code| code / 8 + 1).max().unwrap_or(0);
        let mut bitmap = vec![0u8; usize::from(size)];
        for code in codes {
            bitmap[usize::from(code / 8)] |= 1 << (code % 8);
        }
        bitmap
    }

    fn answer(&self) -> Vec<u8> {
        let (select, subselect) = self.selected.get();
        let codes = |codes: &[u16]| Self::bitmap(codes);
        match (select, u16::from(subselect)) {
            (config::EVENT_BITS, event_type::KEY) => codes(&self.keys),
            (config::EVENT_BITS, event_type::REL) => codes(&self.relatives),
            (config::EVENT_BITS, event_type::ABS) => {
                codes(&self.absolutes.iter().map(|axis| axis.0).collect::<Vec<_>>())
            }
            (config::ABSOLUTE_INFO, axis) => self
                .absolutes
                .iter()
                .find(|absolute| absolute.0 == axis)
                .map(|&(_, min, max)| {
                    [min.to_le_bytes(), max.to_le_bytes(), [0; 4], [0; 4], [0; 4]].concat()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn send(&self, events: &[RawEvent]) -> usize {
        let mut device = self.device.borrow_mut();
        let device = device.as_mut().unwrap();
        let mut sent = 0;
        for &event in events {
            let Some(head) = device.next_available() else {
                break;
            };
            let (address, len, flags, _) = device.descriptor(head);
            assert_eq!(len as usize, EVENT_BYTES);
            assert_ne!(flags & crate::virtio::queue::DESCRIPTOR_WRITE, 0);
            write_event(address.get(), event);
            device.put_used(head, EVENT_BYTES as u32);
            sent += 1;
        }
        sent
    }

    fn available(&self) -> usize {
        let mut device = self.device.borrow_mut();
        let device = device.as_mut().unwrap();
        let mut heads = Vec::new();
        while let Some(head) = device.next_available() {
            heads.push(head);
        }
        let count = heads.len();
        self.posted.borrow_mut().extend(heads);
        count
    }
}

impl Transport for FakeInput {
    fn device_type(&self) -> u32 {
        match self.not_input {
            true => device_type::GPU,
            false => device_type::INPUT,
        }
    }
    fn status(&self) -> u8 {
        self.status.get()
    }
    fn set_status(&self, status: u8) {
        self.status.set(status);
    }
    fn device_features(&self) -> u64 {
        feature::VERSION_1
    }
    fn set_driver_features(&self, features: u64) {
        assert_eq!(features, feature::VERSION_1);
    }
    fn max_queue_size(&self, queue: u16) -> u16 {
        if queue < 2 { self.max_queue_size } else { 0 }
    }
    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        assert_eq!(queue, EVENT_QUEUE);
        let memory = addresses.descriptors.get() as *mut u8;
        // SAFETY: the queue lives in the test's HostDma, whose device address is its host
        // address, for as long as the fake device is used.
        *self.device.borrow_mut() = Some(unsafe { DeviceSide::new(memory, size) });
        Ok(())
    }
    fn notify(&self, queue: u16) {
        assert_eq!(queue, EVENT_QUEUE);
        if self.status.get() & status::DRIVER_OK == 0 {
            self.notified_before_driver_ok.set(true);
        }
        self.notifies.set(self.notifies.get() + 1);
    }
    fn read_config8(&self, offset: usize) -> u8 {
        let answer = self.answer();
        match offset {
            config::SIZE_OFFSET => answer.len() as u8,
            _ => offset
                .checked_sub(config::DATA_OFFSET)
                .and_then(|index| answer.get(index).copied())
                .unwrap_or(0),
        }
    }
    fn write_config8(&self, offset: usize, value: u8) {
        let (select, subselect) = self.selected.get();
        match offset {
            config::SELECT_OFFSET => self.selected.set((value, subselect)),
            config::SUBSELECT_OFFSET => self.selected.set((select, value)),
            _ => panic!("the driver wrote configuration byte {offset}"),
        }
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

fn input(fake: FakeInput) -> VirtioInput<FakeInput, HostDma> {
    VirtioInput::new(fake, HostDma::zeroed(1), &mut polls(10)).unwrap()
}

fn drain(input: &mut VirtioInput<FakeInput, HostDma>) -> Vec<RawEvent> {
    let mut events = Vec::new();
    input.poll(usize::MAX, &mut |event| events.push(event));
    events
}

fn key_event(code: u16, value: i32) -> RawEvent {
    RawEvent::new(event_type::KEY, code, value)
}

const SYN: RawEvent = RawEvent::new(event_type::SYN, 0, 0);

#[test]
fn a_keyboard_mouse_and_tablet_report_what_they_are() {
    let keyboard = input(FakeInput::keyboard()).capabilities();
    assert!(keyboard.keyboard && !keyboard.buttons && !keyboard.is_pointer());
    let mouse = input(FakeInput::mouse()).capabilities();
    assert!(!mouse.keyboard && mouse.buttons && mouse.relative && mouse.wheel);
    assert_eq!(mouse.absolute, None);
    let tablet = input(FakeInput::tablet()).capabilities();
    assert!(!tablet.keyboard && tablet.buttons && !tablet.relative && tablet.wheel);
    assert_eq!(
        tablet.absolute,
        Some((
            AxisRange { min: 0, max: 32767 },
            AxisRange {
                min: -100,
                max: 100
            }
        ))
    );
    let nothing = input(FakeInput {
        max_queue_size: 8,
        ..FakeInput::default()
    })
    .capabilities();
    assert_eq!(nothing, Capabilities::default());
}

#[test]
fn every_buffer_is_posted_before_driver_ok_then_the_device_is_told() {
    let input = input(FakeInput::keyboard());
    let fake = input.transport();
    assert_eq!(
        fake.status.get(),
        status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK
    );
    assert_eq!(fake.available(), usize::from(QUEUE_SIZE));
    assert_eq!(fake.notifies.get(), 1);
    assert!(!fake.notified_before_driver_ok.get());
    assert!(
        fake.device
            .borrow()
            .as_ref()
            .unwrap()
            .used_notifications_disabled()
    );
}

#[test]
fn a_smaller_queue_is_filled_whole() {
    let input = input(FakeInput {
        max_queue_size: 12,
        ..FakeInput::keyboard()
    });
    assert_eq!(input.transport().available(), 8);
}

#[test]
fn events_come_back_in_order_and_a_full_queue_takes_more_once_polled() {
    let mut input = input(FakeInput::keyboard());
    let burst: Vec<RawEvent> = (0..100).map(|code| key_event(code, 1)).collect();
    assert_eq!(input.transport().send(&burst), usize::from(QUEUE_SIZE));
    let mut received = drain(&mut input);
    assert_eq!(received, burst[..usize::from(QUEUE_SIZE)]);
    assert_eq!(
        input.transport().send(&burst[usize::from(QUEUE_SIZE)..]),
        100 - usize::from(QUEUE_SIZE)
    );
    received.extend(drain(&mut input));
    assert_eq!(received, burst);
}

#[test]
fn the_device_never_runs_out_of_buffers_over_many_reports() {
    let mut input = input(FakeInput::mouse());
    let mut received = Vec::new();
    for round in 0..5000 {
        let report = [
            RawEvent::new(event_type::REL, relative::X, round),
            RawEvent::new(event_type::REL, relative::Y, -round),
            SYN,
        ];
        assert_eq!(input.transport().send(&report), 3, "round {round}");
        received.extend(drain(&mut input));
    }
    assert_eq!(received.len(), 15000);
    assert_eq!(
        received[3 * 4321],
        RawEvent::new(event_type::REL, relative::X, 4321)
    );
    assert_eq!(
        received[3 * 4999 + 1],
        RawEvent::new(event_type::REL, relative::Y, -4999)
    );
}

#[test]
fn a_poll_takes_at_most_its_budget_and_an_idle_poll_tells_the_device_nothing() {
    let mut input = input(FakeInput::keyboard());
    let notifies = input.transport().notifies.get();
    let mut events = Vec::new();
    assert_eq!(input.poll(16, &mut |event| events.push(event)), 0);
    assert_eq!(input.transport().notifies.get(), notifies);
    let burst: Vec<RawEvent> = (0..40).map(|code| key_event(code, 1)).collect();
    assert_eq!(input.transport().send(&burst), 40);
    assert_eq!(input.poll(16, &mut |event| events.push(event)), 16);
    assert_eq!(input.poll(16, &mut |event| events.push(event)), 16);
    assert_eq!(input.poll(16, &mut |event| events.push(event)), 8);
    assert_eq!(events, burst);
    assert_eq!(input.transport().notifies.get(), notifies + 3);
}

#[test]
fn a_short_event_is_skipped_and_its_buffer_posted_again() {
    let mut input = input(FakeInput::keyboard());
    {
        let fake = input.transport();
        let mut device = fake.device.borrow_mut();
        let device = device.as_mut().unwrap();
        let head = device.next_available().unwrap();
        device.put_used(head, 4);
    }
    assert!(drain(&mut input).is_empty());
    assert_eq!(input.transport().available(), usize::from(QUEUE_SIZE));
}

#[test]
fn values_are_read_signed_and_little_endian() {
    let mut input = input(FakeInput::tablet());
    let events = [
        RawEvent::new(event_type::ABS, absolute::X, 32767),
        RawEvent::new(event_type::REL, relative::WHEEL, -1),
        RawEvent::new(event_type::KEY, button::LEFT, 1),
        SYN,
    ];
    input.transport().send(&events);
    assert_eq!(drain(&mut input), events);
}

#[test]
fn the_wrong_device_too_little_memory_or_no_queue_is_refused_with_the_memory_back() {
    let wrong = VirtioInput::new(
        FakeInput {
            not_input: true,
            ..FakeInput::keyboard()
        },
        HostDma::zeroed(1),
        &mut polls(10),
    );
    assert!(matches!(wrong, Err((Error::WrongDeviceType, _, Some(_)))));
    let small = VirtioInput::new(FakeInput::keyboard(), HostDma::zeroed(0), &mut polls(10));
    assert!(matches!(small, Err((Error::MemoryTooSmall, _, Some(_)))));
    let no_queue = VirtioInput::new(
        FakeInput {
            max_queue_size: 0,
            ..FakeInput::keyboard()
        },
        HostDma::zeroed(1),
        &mut polls(10),
    );
    let Err((Error::QueueUnavailable, transport, Some(_))) = no_queue else {
        panic!("a device without an event queue was not refused");
    };
    assert_ne!(transport.status.get() & status::FAILED, 0);
}

#[test]
fn a_reset_hands_the_memory_back_once_the_status_reads_0() {
    let input = input(FakeInput::keyboard());
    let (transport, memory, _) = input.reset(&mut polls(10)).unwrap();
    assert_eq!(transport.status.get(), 0);
    assert_eq!(memory.len(), 4096);
}

#[test]
fn a_used_head_outside_the_queue_is_skipped_and_nothing_is_posted_twice() {
    let mut input = input(FakeInput {
        max_queue_size: 8,
        ..FakeInput::keyboard()
    });
    assert_eq!(input.transport().available(), 8);
    {
        let fake = input.transport();
        let mut device = fake.device.borrow_mut();
        device.as_mut().unwrap().put_used(40, EVENT_BYTES as u32);
    }
    let mut events = Vec::new();
    assert_eq!(input.poll(16, &mut |event| events.push(event)), 1);
    assert!(events.is_empty());
    assert_eq!(input.transport().available(), 0);
}
