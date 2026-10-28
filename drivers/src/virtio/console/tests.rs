use super::*;
use crate::dma::fake::HostDma;
use crate::virtio::queue::DeviceSide;
use crate::virtio::{QueueAddresses, feature as common_feature};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

#[derive(Debug)]
struct FakeConsole {
    status: Cell<u8>,
    accepted: Cell<u64>,
    offers_multiport: bool,
    ports: Vec<(u32, &'static [u8])>,
    host_open: Cell<bool>,
    queues: RefCell<[Option<DeviceSide>; 6]>,
    sent_control: RefCell<Vec<ControlMessage>>,
    to_driver_control: RefCell<VecDeque<Vec<u8>>>,
    output: RefCell<Vec<u8>>,
    held: RefCell<Option<(u16, u32)>>,
    throttled: Cell<bool>,
    guest_open: Cell<bool>,
}

impl FakeConsole {
    fn new() -> Self {
        Self {
            status: Cell::new(0),
            accepted: Cell::new(0),
            offers_multiport: true,
            ports: vec![(1, b"org.eggos.mcp")],
            host_open: Cell::new(true),
            queues: RefCell::new([const { None }; 6]),
            sent_control: RefCell::new(Vec::new()),
            to_driver_control: RefCell::new(VecDeque::new()),
            output: RefCell::new(Vec::new()),
            held: RefCell::new(None),
            throttled: Cell::new(false),
            guest_open: Cell::new(false),
        }
    }

    fn queue_control(&self, message: ControlMessage, extra: &[u8]) {
        let mut bytes = message.to_bytes().to_vec();
        bytes.extend_from_slice(extra);
        self.to_driver_control.borrow_mut().push_back(bytes);
        self.deliver_control();
    }

    fn deliver_control(&self) {
        let mut queues = self.queues.borrow_mut();
        let Some(device) = queues[usize::from(CONTROL_RECEIVE_QUEUE)].as_mut() else {
            return;
        };
        let mut pending = self.to_driver_control.borrow_mut();
        while !pending.is_empty() {
            let Some(head) = device.next_available() else {
                return;
            };
            let message = pending.pop_front().unwrap();
            let (addr, len, _, _) = device.descriptor(head);
            let count = message.len().min(len as usize);
            // SAFETY: a buffer the driver posted for the device to write, in the test's HostDma.
            unsafe {
                std::ptr::copy_nonoverlapping(message.as_ptr(), addr.get() as *mut u8, count)
            };
            device.put_used(head, count as u32);
        }
    }

    fn take_driver_control(&self) {
        let messages: Vec<ControlMessage> = {
            let mut queues = self.queues.borrow_mut();
            let device = queues[usize::from(CONTROL_TRANSMIT_QUEUE)]
                .as_mut()
                .unwrap();
            let mut messages = Vec::new();
            while let Some(head) = device.next_available() {
                let (addr, len, _, _) = device.descriptor(head);
                // SAFETY: a buffer the driver posted for the device to read.
                let bytes =
                    unsafe { std::slice::from_raw_parts(addr.get() as *const u8, len as usize) };
                messages.push(ControlMessage::from_bytes(bytes).unwrap());
                device.put_used(head, 0);
            }
            messages
        };
        for message in messages {
            self.sent_control.borrow_mut().push(message);
            match message.event {
                event::DEVICE_READY => {
                    for &(id, _) in &self.ports {
                        self.queue_control(
                            ControlMessage {
                                id,
                                event: event::PORT_ADD,
                                value: 0,
                            },
                            &[],
                        );
                    }
                }
                event::PORT_READY if message.value == 1 => {
                    let name = self
                        .ports
                        .iter()
                        .find(|(id, _)| *id == message.id)
                        .map(|(_, name)| *name)
                        .unwrap();
                    if !name.is_empty() {
                        self.queue_control(
                            ControlMessage {
                                id: message.id,
                                event: event::PORT_NAME,
                                value: 1,
                            },
                            name,
                        );
                    }
                    if self.host_open.get() {
                        self.queue_control(
                            ControlMessage {
                                id: message.id,
                                event: event::PORT_OPEN,
                                value: 1,
                            },
                            &[],
                        );
                    }
                }
                event::PORT_OPEN => self.guest_open.set(message.value == 1),
                _ => {}
            }
        }
    }

    fn host_sends(&self, bytes: &[u8]) -> usize {
        let mut queues = self.queues.borrow_mut();
        let device = queues[usize::from(PORT_RECEIVE_QUEUE)].as_mut().unwrap();
        let mut sent = 0;
        while sent < bytes.len() {
            let Some(head) = device.next_available() else {
                break;
            };
            let (addr, len, _, _) = device.descriptor(head);
            let count = (bytes.len() - sent).min(len as usize);
            // SAFETY: a receive buffer the driver posted for the device to write.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes[sent..].as_ptr(), addr.get() as *mut u8, count)
            };
            device.put_used(head, count as u32);
            sent += count;
        }
        sent
    }

    fn take_output(&self) {
        let mut queues = self.queues.borrow_mut();
        let device = queues[usize::from(PORT_TRANSMIT_QUEUE)].as_mut().unwrap();
        if let Some((head, _)) = *self.held.borrow() {
            if self.throttled.get() {
                return;
            }
            device.put_used(head, 0);
        }
        *self.held.borrow_mut() = None;
        while let Some(head) = device.next_available() {
            let (addr, len, _, _) = device.descriptor(head);
            // SAFETY: a buffer the driver posted for the device to read.
            let bytes =
                unsafe { std::slice::from_raw_parts(addr.get() as *const u8, len as usize) };
            self.output.borrow_mut().extend_from_slice(bytes);
            if self.throttled.get() {
                *self.held.borrow_mut() = Some((head, len));
                return;
            }
            device.put_used(head, 0);
        }
    }
}

impl Transport for &FakeConsole {
    fn device_type(&self) -> u32 {
        device_type::CONSOLE
    }
    fn status(&self) -> u8 {
        self.status.get()
    }
    fn set_status(&self, status: u8) {
        self.status.set(status);
    }
    fn device_features(&self) -> u64 {
        let multiport = match self.offers_multiport {
            true => feature::MULTIPORT,
            false => 0,
        };
        common_feature::VERSION_1 | multiport
    }
    fn set_driver_features(&self, features: u64) {
        self.accepted.set(features);
    }
    fn max_queue_size(&self, queue: u16) -> u16 {
        if queue < 6 { 32 } else { 0 }
    }
    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        let memory = addresses.descriptors.get() as *mut u8;
        // SAFETY: the queue lives in the test's HostDma, whose device address is its host
        // address, for as long as the fake device is used.
        self.queues.borrow_mut()[usize::from(queue)] =
            Some(unsafe { DeviceSide::new(memory, size) });
        Ok(())
    }
    fn notify(&self, queue: u16) {
        match queue {
            CONTROL_RECEIVE_QUEUE => self.deliver_control(),
            CONTROL_TRANSMIT_QUEUE => self.take_driver_control(),
            PORT_TRANSMIT_QUEUE => self.take_output(),
            _ => {}
        }
    }
    fn read_config8(&self, _offset: usize) -> u8 {
        0
    }
    fn write_config8(&self, offset: usize, _value: u8) {
        panic!("the driver wrote configuration byte {offset}");
    }
    fn read_config32(&self, _offset: usize) -> u32 {
        0
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

fn console(fake: &FakeConsole) -> VirtioConsole<&FakeConsole, HostDma> {
    VirtioConsole::new(fake, memory(), &mut polls(20)).unwrap()
}

fn read_all(console: &mut VirtioConsole<&FakeConsole, HostDma>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = [0u8; 1000];
    loop {
        let count = console.receive(&mut buffer);
        if count == 0 {
            return out;
        }
        out.extend_from_slice(&buffer[..count]);
    }
}

fn send(console: &mut VirtioConsole<&FakeConsole, HostDma>, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let count = console.stage(bytes);
        bytes = &bytes[count..];
        console.send_staged().unwrap();
    }
}

#[test]
fn the_handshake_readies_and_opens_the_named_data_port() {
    let fake = FakeConsole::new();
    let console = console(&fake);
    assert_eq!(console.name(), b"org.eggos.mcp");
    assert!(console.host_connected());
    assert!(fake.guest_open.get());
    assert_ne!(fake.accepted.get() & feature::MULTIPORT, 0);
    let sent: Vec<(u32, u16, u16)> = fake
        .sent_control
        .borrow()
        .iter()
        .map(|message| (message.id, message.event, message.value))
        .collect();
    assert_eq!(
        sent,
        [
            (0, event::DEVICE_READY, 1),
            (1, event::PORT_READY, 1),
            (1, event::PORT_OPEN, 1)
        ]
    );
}

#[test]
fn other_ports_are_refused_and_the_data_port_still_opens() {
    let mut fake = FakeConsole::new();
    fake.ports = vec![(1, b"org.eggos.mcp"), (2, b"other")];
    let console = console(&fake);
    assert_eq!(console.name(), b"org.eggos.mcp");
    assert!(fake.sent_control.borrow().contains(&ControlMessage {
        id: 2,
        event: event::PORT_READY,
        value: 0
    }));
}

#[test]
fn a_device_without_multiport_or_a_named_port_is_refused() {
    let mut fake = FakeConsole::new();
    fake.offers_multiport = false;
    let error = VirtioConsole::new(&fake, memory(), &mut polls(20)).unwrap_err();
    assert_eq!(error.0, Error::NoPort);
    let mut fake = FakeConsole::new();
    fake.ports = vec![(1, b"")];
    let error = VirtioConsole::new(&fake, memory(), &mut polls(20)).unwrap_err();
    assert_eq!(error.0, Error::NoPort);
    assert!(error.2.is_some());
    let mut fake = FakeConsole::new();
    fake.ports = vec![(2, b"org.eggos.mcp")];
    assert_eq!(
        VirtioConsole::new(&fake, memory(), &mut polls(20))
            .unwrap_err()
            .0,
        Error::NoPort
    );
}

#[test]
fn small_memory_and_other_devices_are_refused() {
    let fake = FakeConsole::new();
    let error = VirtioConsole::new(&fake, HostDma::zeroed(1), &mut polls(5)).unwrap_err();
    assert_eq!(error.0, Error::MemoryTooSmall);
}

#[test]
fn bytes_from_the_host_are_read_across_buffers_and_buffers_go_back() {
    let fake = FakeConsole::new();
    let mut console = console(&fake);
    assert!(!console.has_input());
    let message: Vec<u8> = (0..RECEIVE_BUFFER_BYTES * 3 + 17)
        .map(|i| i as u8)
        .collect();
    assert_eq!(fake.host_sends(&message), message.len());
    assert!(console.has_input());
    let mut first = [0u8; 10];
    assert_eq!(console.receive(&mut first), 10);
    assert_eq!(&first, &message[..10]);
    let rest = read_all(&mut console);
    assert_eq!(rest, &message[10..]);
    for round in 0..20u8 {
        let chunk = vec![round; RECEIVE_BUFFER_BYTES * 2];
        assert_eq!(fake.host_sends(&chunk), chunk.len());
        assert_eq!(read_all(&mut console), chunk);
    }
}

#[test]
fn more_than_the_posted_buffers_waits_for_the_driver_to_read() {
    let fake = FakeConsole::new();
    let mut console = console(&fake);
    let message = vec![7u8; RECEIVE_BUFFER_BYTES * RECEIVE_BUFFERS + 100];
    let first = fake.host_sends(&message);
    assert_eq!(first, RECEIVE_BUFFER_BYTES * RECEIVE_BUFFERS);
    let mut buffer = vec![0u8; RECEIVE_BUFFER_BYTES];
    assert_eq!(console.receive(&mut buffer), RECEIVE_BUFFER_BYTES);
    assert_eq!(fake.host_sends(&message[first..]), 100);
}

#[test]
fn sends_are_split_at_the_transmit_buffer_and_arrive_in_order() {
    let fake = FakeConsole::new();
    let mut console = console(&fake);
    let message: Vec<u8> = (0..TRANSMIT_BYTES * 2 + 5)
        .map(|i| (i % 251) as u8)
        .collect();
    send(&mut console, &message);
    assert!(!console.is_sending());
    assert_eq!(*fake.output.borrow(), message);
}

#[test]
fn a_throttled_transmission_holds_further_bytes_until_the_host_drains() {
    let fake = FakeConsole::new();
    let mut console = console(&fake);
    fake.throttled.set(true);
    assert_eq!(console.stage(b"first\n"), 6);
    assert_eq!(console.staged(), 6);
    console.send_staged().unwrap();
    assert!(console.is_sending());
    assert_eq!(console.stage(b"second"), 0);
    console.send_staged().unwrap();
    fake.throttled.set(false);
    fake.take_output();
    assert!(!console.is_sending());
    assert_eq!(console.stage(b"second\n"), 7);
    console.send_staged().unwrap();
    assert_eq!(*fake.output.borrow(), b"first\nsecond\n");
}

#[test]
fn the_host_side_closing_and_reopening_is_seen() {
    let fake = FakeConsole::new();
    let mut console = console(&fake);
    assert!(!console.poll_control());
    fake.queue_control(
        ControlMessage {
            id: DATA_PORT,
            event: event::PORT_OPEN,
            value: 0,
        },
        &[],
    );
    assert!(console.poll_control());
    assert!(!console.host_connected());
    fake.queue_control(
        ControlMessage {
            id: DATA_PORT,
            event: event::PORT_OPEN,
            value: 1,
        },
        &[],
    );
    assert!(console.poll_control());
    assert!(console.host_connected());
}

#[test]
fn a_port_added_later_is_refused_and_short_control_messages_are_skipped() {
    let fake = FakeConsole::new();
    let mut console = console(&fake);
    fake.to_driver_control.borrow_mut().push_back(vec![1, 2, 3]);
    fake.deliver_control();
    fake.queue_control(
        ControlMessage {
            id: 5,
            event: event::PORT_ADD,
            value: 0,
        },
        &[],
    );
    assert!(!console.poll_control());
    assert_eq!(
        fake.sent_control.borrow().last(),
        Some(&ControlMessage {
            id: 5,
            event: event::PORT_READY,
            value: 0
        })
    );
    for _ in 0..20 {
        fake.queue_control(
            ControlMessage {
                id: DATA_PORT,
                event: event::CONSOLE_PORT,
                value: 1,
            },
            &[],
        );
        console.poll_control();
    }
    assert!(console.host_connected());
}

#[test]
fn a_long_port_name_is_cut_and_control_messages_round_trip() {
    let mut fake = FakeConsole::new();
    let long: &'static [u8] = &[b'n'; 100];
    fake.ports = vec![(1, long)];
    let console = console(&fake);
    assert_eq!(console.name(), &[b'n'; MAX_NAME_BYTES][..]);
    let message = ControlMessage {
        id: 0x0102_0304,
        event: 0x0506,
        value: 0x0708,
    };
    assert_eq!(
        message.to_bytes(),
        [0x04, 0x03, 0x02, 0x01, 0x06, 0x05, 0x08, 0x07]
    );
    assert_eq!(
        ControlMessage::from_bytes(&message.to_bytes()),
        Some(message)
    );
    assert_eq!(ControlMessage::from_bytes(&[0; 7]), None);
}
