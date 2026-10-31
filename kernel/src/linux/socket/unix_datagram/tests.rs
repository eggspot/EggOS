use super::*;
use crate::linux::descriptors::open_flag;
use crate::linux::handle;
use crate::linux::socket::{AF_UNIX, SOL_SOCKET, option};
use crate::memory::FRAMES;
use crate::paging::PageTables;
use crate::tests::TestPlatform as P;
use crate::thread::host::frames_for_test;
use alloc::vec;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;
use eggos_hal::user::UserExit;

const DATA: u64 = 0x58_0000;
const PAGES: u64 = 2;
const SEND_DATA: u64 = DATA + 0x100;
const RECEIVE_DATA: u64 = DATA + 0x200;
const ADDRESS: u64 = DATA + 0x400;
const ADDRESS_LENGTH: u64 = DATA + 0x500;
const PAIR: u64 = DATA + 0x600;
const UNIX: u64 = AF_UNIX as u64;
const DGRAM: u64 = SOCK_DGRAM as u64;

fn kernel_tables() -> PageTables<<P as Platform>::PageTableFormat> {
    let mut frames = FRAMES.lock();
    let mut tables = PageTables::new(frames.as_mut().unwrap()).unwrap();
    tables.mark_active();
    tables
}

fn process(kernel: &PageTables<<P as Platform>::PageTableFormat>) -> Arc<Process<P>> {
    // SAFETY: test tables, never active; the test platform's switch only records roots.
    let mut process = unsafe { Process::<P>::new(kernel) }.unwrap();
    for page in 0..PAGES {
        process
            .map_new_page(DATA + page * FRAME_SIZE, Attributes::USER_READ_WRITE, &[])
            .unwrap();
    }
    Arc::new(process)
}

fn call(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    number: u64,
    arguments: [u64; 6],
) -> u64 {
    let mut context = UserContext::new(0x40_0000, 0);
    context.simulate_syscall(number, arguments, 0x40_0004);
    assert_eq!(
        handle(process, thread, &mut context, UserExit::Syscall),
        ControlFlow::Continue(())
    );
    context.syscall_result()
}

fn store(process: &Process<P>, address: u64, bytes: &[u8]) {
    process.write_user_memory(address, bytes).unwrap();
}

fn load(process: &Process<P>, address: u64, length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    process.read_user_memory(address, &mut bytes).unwrap();
    bytes
}

fn abstract_address(name: &[u8]) -> Vec<u8> {
    let mut bytes = (AF_UNIX as u16).to_ne_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(name);
    bytes
}

#[test]
fn a_datagram_pair_keeps_each_datagram_whole_and_cuts_a_short_read() {
    let _serial = unix_socket::tests_lock();
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    let nonblocking = u64::from(open_flag::NONBLOCK);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::SOCKETPAIR,
            [UNIX, DGRAM | nonblocking, 0, PAIR, 0, 0]
        ),
        0
    );
    let words = load(&process, PAIR, 8);
    let first = u64::from(u32::from_ne_bytes(words[..4].try_into().unwrap()));
    let second = u64::from(u32::from_ne_bytes(words[4..].try_into().unwrap()));
    store(&process, SEND_DATA, b"hello world");
    for length in [5, 11] {
        assert_eq!(
            call(
                &process,
                &mut thread,
                number::WRITE,
                [first, SEND_DATA, length, 0, 0, 0]
            ),
            length
        );
    }
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::READ,
            [second, RECEIVE_DATA, 64, 0, 0, 0]
        ),
        5
    );
    assert_eq!(load(&process, RECEIVE_DATA, 5), b"hello");
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::RECVFROM,
            [second, RECEIVE_DATA, 3, MSG_TRUNC as u64, 0, 0]
        ),
        11
    );
    assert_eq!(load(&process, RECEIVE_DATA, 3), b"hel");
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::READ,
            [second, RECEIVE_DATA, 64, 0, 0, 0]
        ),
        Errno::EAGAIN.result()
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::LISTEN,
            [first, 1, 0, 0, 0, 0]
        ),
        Errno::EOPNOTSUPP.result()
    );
    store(&process, ADDRESS_LENGTH, &4u32.to_ne_bytes());
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::GETSOCKOPT,
            [
                first,
                u64::from(SOL_SOCKET),
                u64::from(option::TYPE),
                ADDRESS,
                ADDRESS_LENGTH,
                0
            ]
        ),
        0
    );
    assert_eq!(load(&process, ADDRESS, 4), 2u32.to_ne_bytes());
}

#[test]
fn a_bound_socket_receives_from_any_sender_with_its_address() {
    let _serial = unix_socket::tests_lock();
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    let nonblocking = u64::from(open_flag::NONBLOCK);
    let socket = |thread: &mut LinuxThread| {
        call(
            &process,
            thread,
            number::SOCKET,
            [UNIX, DGRAM | nonblocking, 0, 0, 0, 0],
        )
    };
    let receiver = socket(&mut thread);
    let sender = socket(&mut thread);
    let receiver_address = abstract_address(b"eggos-datagram-receiver");
    store(&process, ADDRESS, &receiver_address);
    let length = receiver_address.len() as u64;
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::BIND,
            [receiver, ADDRESS, length, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::WRITE,
            [sender, SEND_DATA, 1, 0, 0, 0]
        ),
        Errno::ENOTCONN.result()
    );
    store(&process, SEND_DATA, b"ping");
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::SENDTO,
            [sender, SEND_DATA, 4, 0, ADDRESS, length]
        ),
        4
    );
    let sender_address = abstract_address(b"eggos-datagram-sender");
    store(&process, ADDRESS, &sender_address);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::BIND,
            [sender, ADDRESS, sender_address.len() as u64, 0, 0, 0]
        ),
        0
    );
    store(&process, ADDRESS, &receiver_address);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::CONNECT,
            [sender, ADDRESS, length, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::WRITE,
            [sender, SEND_DATA, 2, 0, 0, 0]
        ),
        2
    );
    store(&process, ADDRESS_LENGTH, &110u32.to_ne_bytes());
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::RECVFROM,
            [receiver, RECEIVE_DATA, 64, 0, ADDRESS, ADDRESS_LENGTH]
        ),
        4
    );
    assert_eq!(load(&process, RECEIVE_DATA, 4), b"ping");
    assert_eq!(
        load(&process, ADDRESS_LENGTH, 4),
        2u32.to_ne_bytes(),
        "an unbound sender has no name"
    );
    store(&process, ADDRESS_LENGTH, &110u32.to_ne_bytes());
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::RECVFROM,
            [receiver, RECEIVE_DATA, 64, 0, ADDRESS, ADDRESS_LENGTH]
        ),
        2
    );
    let named = sender_address.len();
    assert_eq!(
        load(&process, ADDRESS_LENGTH, 4),
        (named as u32).to_ne_bytes()
    );
    assert_eq!(load(&process, ADDRESS, named), sender_address);
    let stream = call(
        &process,
        &mut thread,
        number::SOCKET,
        [UNIX, u64::from(super::super::SOCK_STREAM), 0, 0, 0, 0],
    );
    let stream_address = abstract_address(b"eggos-datagram-stream");
    store(&process, ADDRESS, &stream_address);
    let stream_length = stream_address.len() as u64;
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::BIND,
            [stream, ADDRESS, stream_length, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::SENDTO,
            [sender, SEND_DATA, 4, 0, ADDRESS, stream_length]
        ),
        Errno::EPROTOTYPE.result()
    );
}

struct Counting(core::sync::atomic::AtomicU32);

impl poll::Watcher for Counting {
    fn woken(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_sender_waiting_for_room_is_woken_when_its_receiver_drains_or_closes() {
    let _serial = unix_socket::tests_lock();
    let (sender, receiver) = pair();
    let counting = Arc::new(Counting(core::sync::atomic::AtomicU32::new(0)));
    let watcher: Arc<dyn poll::Watcher> = counting.clone();
    sender.wake_list().watch(&watcher).unwrap();
    let fill = |receiver: &Arc<UnixSocket>, count: usize| {
        let receiving = receiver.datagrams().unwrap();
        for _ in 0..count {
            let datagram = Datagram {
                sender: Address::Unnamed,
                bytes: vec![1],
            };
            let pushed = receiving.push(Arc::as_ptr(&sender), datagram, receive_limit(receiver));
            assert!(pushed.is_ok());
        }
    };
    fill(&receiver, MAX_QUEUED);
    assert_eq!(sender.events(event::OUT), 0);
    let woken = counting.0.load(Ordering::Relaxed);
    let taken = receiver.datagrams().unwrap().take(false).unwrap();
    assert!(taken.is_some());
    assert!(counting.0.load(Ordering::Relaxed) > woken);
    assert_eq!(sender.events(event::OUT), event::OUT);
    fill(&receiver, 1);
    assert_eq!(sender.events(event::OUT), 0);
    let woken = counting.0.load(Ordering::Relaxed);
    drop(receiver);
    assert!(counting.0.load(Ordering::Relaxed) > woken);
    assert_eq!(sender.events(event::OUT), event::OUT);
    drop(sender.wake_list().unwatch(&watcher));
}
