use super::*;
use crate::linux::handle;
use crate::linux::socket::SOCK_STREAM;
use crate::linux::socket::inet::{AF_INET, SOCKADDR_IN_SIZE, encode_address, parse_address};
use crate::memory::FRAMES;
use crate::paging::PageTables;
use crate::tests::TestPlatform as P;
use alloc::vec;
use alloc::vec::Vec;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;
use eggos_hal::user::UserExit;

const DATA: u64 = 0x50_0000;
const PAGES: u64 = 2;
const ADDRESS: u64 = DATA;
const LENGTH: u64 = DATA + 0x40;
const VALUE: u64 = DATA + 0x80;
const BUFFER: u64 = DATA + 0x100;

struct Test {
    thread: LinuxThread,
    process: Arc<Process<P>>,
    _kernel: PageTables<<P as Platform>::PageTableFormat>,
}

impl Test {
    fn new() -> Self {
        let kernel = {
            let mut frames = FRAMES.lock();
            let mut tables = PageTables::new(frames.as_mut().unwrap()).unwrap();
            tables.mark_active();
            tables
        };
        // SAFETY: test tables, never active; the test platform's switch only records roots.
        let mut process = unsafe { Process::<P>::new(&kernel) }.unwrap();
        for page in 0..PAGES {
            process
                .map_new_page(DATA + page * FRAME_SIZE, Attributes::USER_READ_WRITE, &[])
                .unwrap();
        }
        let process = Arc::new(process);
        let thread = LinuxThread::first(&process);
        Self {
            thread,
            process,
            _kernel: kernel,
        }
    }

    fn call(&mut self, number: u64, arguments: [u64; 6]) -> u64 {
        let mut context = UserContext::new(0x40_0000, 0);
        context.simulate_syscall(number, arguments, 0x40_0004);
        assert_eq!(
            handle(
                &self.process,
                &mut self.thread,
                &mut context,
                UserExit::Syscall
            ),
            ControlFlow::Continue(())
        );
        context.syscall_result()
    }

    fn store(&self, address: u64, bytes: &[u8]) {
        self.process.write_user_memory(address, bytes).unwrap();
    }

    fn load(&self, address: u64, length: usize) -> Vec<u8> {
        let mut bytes = vec![0; length];
        self.process.read_user_memory(address, &mut bytes).unwrap();
        bytes
    }

    fn socket(&mut self) -> u64 {
        let kind = u64::from(SOCK_STREAM | open_flag::NONBLOCK);
        let fd = self.call(number::SOCKET, [AF_INET.into(), kind, 0, 0, 0, 0]);
        assert!(fd < 1024, "socket failed: {fd:#x}");
        fd
    }

    fn with_address(&mut self, number: u64, fd: u64, address: (Ipv4Address, u16)) -> u64 {
        self.store(ADDRESS, &encode_address(address.0, address.1));
        self.call(number, [fd, ADDRESS, SOCKADDR_IN_SIZE as u64, 0, 0, 0])
    }

    fn name(&mut self, number: u64, fd: u64) -> u64 {
        self.store(LENGTH, &(SOCKADDR_IN_SIZE as u32).to_ne_bytes());
        self.call(number, [fd, ADDRESS, LENGTH, 0, 0, 0])
    }

    fn loaded_address(&self) -> Option<(Ipv4Address, u16)> {
        parse_address(&self.load(ADDRESS, SOCKADDR_IN_SIZE)).unwrap()
    }

    fn send(&mut self, fd: u64, data: &[u8]) -> u64 {
        self.store(BUFFER, data);
        self.call(number::SENDTO, [fd, BUFFER, data.len() as u64, 0, 0, 0])
    }

    fn receive(&mut self, fd: u64, room: u64, flags: u32) -> u64 {
        self.call(number::RECVFROM, [fd, BUFFER, room, flags.into(), 0, 0])
    }

    fn option(&mut self, fd: u64, level: u32, name: u32) -> (u64, u32) {
        self.store(LENGTH, &4u32.to_ne_bytes());
        let result = self.call(
            number::GETSOCKOPT,
            [fd, level.into(), name.into(), VALUE, LENGTH, 0],
        );
        let value = u32::from_ne_bytes(self.load(VALUE, 4).try_into().unwrap());
        (result, value)
    }
}

#[test]
fn a_stream_connects_over_loopback_and_carries_bytes_both_ways() {
    let mut test = Test::new();
    let server = test.socket();
    assert_eq!(
        test.with_address(number::BIND, server, (Ipv4Address::LOOPBACK, 47_101)),
        0
    );
    assert_eq!(test.call(number::LISTEN, [server, 8, 0, 0, 0, 0]), 0);
    assert_eq!(test.receive(server, 4, 0), Errno::ENOTCONN.result());
    assert_eq!(
        test.call(number::ACCEPT, [server, 0, 0, 0, 0, 0]),
        Errno::EAGAIN.result()
    );
    assert_eq!(test.option(server, SOL_SOCKET, option::ACCEPTCONN), (0, 1));
    assert_eq!(
        test.option(server, SOL_SOCKET, option::TYPE),
        (0, SOCK_STREAM)
    );
    let client = test.socket();
    let connected = test.with_address(number::CONNECT, client, (Ipv4Address::LOOPBACK, 47_101));
    assert!(
        connected == 0 || connected == Errno::EINPROGRESS.result(),
        "{connected:#x}"
    );
    assert_eq!(test.name(number::GETPEERNAME, client), 0);
    assert_eq!(test.loaded_address(), Some((Ipv4Address::LOOPBACK, 47_101)));
    assert_eq!(test.name(number::GETSOCKNAME, client), 0);
    let (_, client_port) = test.loaded_address().unwrap();
    test.store(LENGTH, &(SOCKADDR_IN_SIZE as u32).to_ne_bytes());
    let accepted = test.call(number::ACCEPT4, [server, ADDRESS, LENGTH, 0, 0, 0]);
    assert!(accepted < 1024, "accept failed: {accepted:#x}");
    assert_eq!(
        test.loaded_address(),
        Some((Ipv4Address::LOOPBACK, client_port))
    );
    assert_eq!(test.send(client, b"hello"), 5);
    store_ioctl_room(&mut test);
    assert_eq!(
        test.call(
            number::IOCTL,
            [
                accepted,
                u64::from(crate::linux::terminal::request::FIONREAD),
                VALUE,
                0,
                0,
                0
            ]
        ),
        0
    );
    assert_eq!(
        u32::from_ne_bytes(test.load(VALUE, 4).try_into().unwrap()),
        5
    );
    assert_eq!(test.receive(accepted, 64, message::PEEK), 5);
    assert_eq!(test.receive(accepted, 3, 0), 3);
    assert_eq!(test.load(BUFFER, 3), b"hel");
    assert_eq!(test.receive(accepted, 64, 0), 2);
    assert_eq!(test.receive(accepted, 64, 0), Errno::EAGAIN.result());
    assert_eq!(test.send(accepted, b"world"), 5);
    assert_eq!(test.receive(client, 64, 0), 5);
    assert_eq!(test.load(BUFFER, 5), b"world");
    assert_eq!(test.call(number::SHUTDOWN, [client, 1, 0, 0, 0, 0]), 0);
    assert_eq!(
        test.receive(accepted, 64, 0),
        0,
        "the client's FIN reads as the end"
    );
    test.store(BUFFER, b"late");
    let flags = u64::from(message::NOSIGNAL);
    assert_eq!(
        test.call(number::SENDTO, [client, BUFFER, 4, flags, 0, 0]),
        Errno::EPIPE.result()
    );
}

fn store_ioctl_room(test: &mut Test) {
    test.store(VALUE, &[0xff; 4]);
}

#[test]
fn a_connection_no_one_listens_for_is_refused_and_so_error_is_clear_after() {
    let mut test = Test::new();
    let client = test.socket();
    assert_eq!(
        test.with_address(number::CONNECT, client, (Ipv4Address::LOOPBACK, 47_111)),
        Errno::ECONNREFUSED.result()
    );
    assert_eq!(test.option(client, SOL_SOCKET, option::ERROR), (0, 0));
    let unroutable = test.socket();
    assert_eq!(
        test.with_address(
            number::CONNECT,
            unroutable,
            (Ipv4Address::new(10, 0, 2, 2), 80)
        ),
        Errno::ENETUNREACH.result(),
        "without an adapter only this machine is reachable"
    );
    let listening = test.socket();
    assert_eq!(test.call(number::LISTEN, [listening, 1, 0, 0, 0, 0]), 0);
    assert_eq!(test.name(number::GETSOCKNAME, listening), 0);
    let (_, port) = test.loaded_address().unwrap();
    assert!(
        crate::net::endpoints::EPHEMERAL_PORTS.contains(&port),
        "listen bound it"
    );
    assert_eq!(
        test.with_address(number::CONNECT, listening, (Ipv4Address::LOOPBACK, 47_112)),
        Errno::EINVAL.result()
    );
    assert_eq!(test.option(listening, IPPROTO_TCP, TCP_NODELAY), (0, 1));
}
