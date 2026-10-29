use super::*;
use crate::linux::handle;
use crate::memory::FRAMES;
use crate::paging::PageTables;
use crate::tests::TestPlatform as P;
use alloc::vec;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;
use eggos_hal::user::UserExit;

const DATA: u64 = 0x50_0000;
const PAGES: u64 = 2;
const ADDRESS: u64 = DATA;
const LENGTH: u64 = DATA + 0x40;
const BUFFER: u64 = DATA + 0x100;
const HEADER: u64 = DATA + 0x800;
const IOVEC: u64 = DATA + 0x900;

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

fn store_address(process: &Process<P>, address: Ipv4Address, port: u16) {
    store(process, ADDRESS, &encode_address(address, port));
    store(process, LENGTH, &(SOCKADDR_IN_SIZE as u32).to_ne_bytes());
}

fn loaded_address(process: &Process<P>) -> Option<(Ipv4Address, u16)> {
    parse_address(&load(process, ADDRESS, SOCKADDR_IN_SIZE)).unwrap()
}

struct Test {
    thread: LinuxThread,
    process: Arc<Process<P>>,
    _kernel: PageTables<<P as Platform>::PageTableFormat>,
}

impl Test {
    fn new() -> Self {
        let kernel = kernel_tables();
        let process = process(&kernel);
        let thread = LinuxThread::first(&process);
        Self {
            thread,
            process,
            _kernel: kernel,
        }
    }

    fn call(&mut self, number: u64, arguments: [u64; 6]) -> u64 {
        call(&self.process, &mut self.thread, number, arguments)
    }

    fn socket(&mut self, protocol: u32) -> u64 {
        let kind = u64::from(SOCK_DGRAM | open_flag::NONBLOCK);
        let fd = self.call(
            number::SOCKET,
            [AF_INET.into(), kind, protocol.into(), 0, 0, 0],
        );
        assert!(fd < 1024, "socket failed: {fd:#x}");
        fd
    }

    fn bind(&mut self, fd: u64, address: Ipv4Address, port: u16) -> u64 {
        store_address(&self.process, address, port);
        self.call(
            number::BIND,
            [fd, ADDRESS, SOCKADDR_IN_SIZE as u64, 0, 0, 0],
        )
    }

    fn send_to(&mut self, fd: u64, data: &[u8], to: Option<(Ipv4Address, u16)>, flags: u32) -> u64 {
        store(&self.process, BUFFER, data);
        let (address, length) = match to {
            Some((address, port)) => {
                store_address(&self.process, address, port);
                (ADDRESS, SOCKADDR_IN_SIZE as u64)
            }
            None => (0, 0),
        };
        let arguments = [fd, BUFFER, data.len() as u64, flags.into(), address, length];
        self.call(number::SENDTO, arguments)
    }

    fn receive_from(&mut self, fd: u64, room: u64, flags: u32) -> u64 {
        store(
            &self.process,
            LENGTH,
            &(SOCKADDR_IN_SIZE as u32).to_ne_bytes(),
        );
        let arguments = [fd, BUFFER, room, flags.into(), ADDRESS, LENGTH];
        self.call(number::RECVFROM, arguments)
    }

    fn local(&mut self, fd: u64) -> (Ipv4Address, u16) {
        store(
            &self.process,
            LENGTH,
            &(SOCKADDR_IN_SIZE as u32).to_ne_bytes(),
        );
        assert_eq!(
            self.call(number::GETSOCKNAME, [fd, ADDRESS, LENGTH, 0, 0, 0]),
            0
        );
        loaded_address(&self.process).unwrap()
    }
}

#[test]
fn a_udp_datagram_goes_through_the_loopback_network_with_its_sender_s_address() {
    let mut test = Test::new();
    let receiver = test.socket(0);
    assert_eq!(test.bind(receiver, Ipv4Address::LOOPBACK, 5301), 0);
    assert_eq!(test.local(receiver), (Ipv4Address::LOOPBACK, 5301));
    let sender = test.socket(IPPROTO_UDP);
    assert_eq!(
        test.send_to(sender, b"hello", Some((Ipv4Address::LOOPBACK, 5301)), 0),
        5
    );
    let (_, port) = test.local(sender);
    assert!(
        endpoints::EPHEMERAL_PORTS.contains(&port),
        "the sender was bound by sending"
    );
    assert_eq!(test.receive_from(receiver, 64, 0), 5);
    assert_eq!(load(&test.process, BUFFER, 5), b"hello");
    assert_eq!(
        loaded_address(&test.process),
        Some((Ipv4Address::LOOPBACK, port))
    );
    assert_eq!(test.receive_from(receiver, 64, 0), Errno::EAGAIN.result());
}

#[test]
fn a_short_buffer_cuts_the_datagram_and_msg_trunc_says_its_length() {
    let mut test = Test::new();
    let receiver = test.socket(0);
    assert_eq!(test.bind(receiver, Ipv4Address::UNSPECIFIED, 5302), 0);
    let sender = test.socket(0);
    for _ in 0..3 {
        assert_eq!(
            test.send_to(
                sender,
                b"0123456789",
                Some((Ipv4Address::LOOPBACK, 5302)),
                0
            ),
            10
        );
    }
    assert_eq!(test.receive_from(receiver, 4, message::PEEK), 4);
    assert_eq!(test.receive_from(receiver, 4, 0), 4);
    assert_eq!(load(&test.process, BUFFER, 4), b"0123");
    assert_eq!(test.receive_from(receiver, 4, MSG_TRUNC), 10);
    store(&test.process, IOVEC, &BUFFER.to_ne_bytes());
    store(&test.process, IOVEC + 8, &3u64.to_ne_bytes());
    let mut header = [0u8; MSGHDR_SIZE];
    header[16..24].copy_from_slice(&IOVEC.to_ne_bytes());
    header[24..32].copy_from_slice(&1u64.to_ne_bytes());
    store(&test.process, HEADER, &header);
    assert_eq!(
        test.call(number::RECVMSG, [receiver, HEADER, 0, 0, 0, 0]),
        3
    );
    let flags = u32::from_ne_bytes(load(&test.process, HEADER + 48, 4).try_into().unwrap());
    assert_eq!(flags, MSG_TRUNC);
}

#[test]
fn sends_without_a_destination_route_or_broadcast_permission_fail_as_linux() {
    let mut test = Test::new();
    let fd = test.socket(0);
    assert_eq!(
        test.send_to(fd, b"x", None, 0),
        Errno::EDESTADDRREQ.result()
    );
    assert_eq!(
        test.send_to(fd, b"x", Some((Ipv4Address::new(10, 0, 2, 2), 7)), 0),
        Errno::ENETUNREACH.result()
    );
    assert_eq!(
        test.send_to(fd, b"x", Some((Ipv4Address::BROADCAST, 7)), 0),
        Errno::EACCES.result()
    );
    assert_eq!(
        test.bind(fd, Ipv4Address::new(10, 9, 9, 9), 5303),
        Errno::EADDRNOTAVAIL.result()
    );
    let taken = test.socket(0);
    assert_eq!(test.bind(taken, Ipv4Address::UNSPECIFIED, 5304), 0);
    let other = test.socket(0);
    assert_eq!(
        test.bind(other, Ipv4Address::LOOPBACK, 5304),
        Errno::EADDRINUSE.result()
    );
    assert_eq!(
        test.bind(taken, Ipv4Address::UNSPECIFIED, 5305),
        Errno::EINVAL.result()
    );
    assert_eq!(
        test.call(number::GETPEERNAME, [taken, ADDRESS, LENGTH, 0, 0, 0]),
        Errno::ENOTCONN.result()
    );
    assert_eq!(
        test.call(number::LISTEN, [taken, 1, 0, 0, 0, 0]),
        Errno::EOPNOTSUPP.result()
    );
}

#[test]
fn a_connected_socket_reads_and_writes_its_peer_only() {
    let mut test = Test::new();
    let server = test.socket(0);
    assert_eq!(test.bind(server, Ipv4Address::LOOPBACK, 5306), 0);
    let client = test.socket(0);
    store_address(&test.process, Ipv4Address::LOOPBACK, 5306);
    assert_eq!(
        test.call(
            number::CONNECT,
            [client, ADDRESS, SOCKADDR_IN_SIZE as u64, 0, 0, 0]
        ),
        0
    );
    store(
        &test.process,
        LENGTH,
        &(SOCKADDR_IN_SIZE as u32).to_ne_bytes(),
    );
    assert_eq!(
        test.call(number::GETPEERNAME, [client, ADDRESS, LENGTH, 0, 0, 0]),
        0
    );
    assert_eq!(
        loaded_address(&test.process),
        Some((Ipv4Address::LOOPBACK, 5306))
    );
    assert_eq!(test.send_to(client, b"ping", None, 0), 4);
    assert_eq!(test.receive_from(server, 16, 0), 4);
    let client_address = loaded_address(&test.process).unwrap();
    assert_eq!(test.send_to(server, b"pong", Some(client_address), 0), 4);
    let stranger = test.socket(0);
    assert_eq!(test.send_to(stranger, b"spam", Some(client_address), 0), 4);
    assert_eq!(test.receive_from(client, 16, 0), 4);
    assert_eq!(load(&test.process, BUFFER, 4), b"pong");
    assert_eq!(
        test.receive_from(client, 16, 0),
        Errno::EAGAIN.result(),
        "the stranger's is dropped"
    );
    assert_eq!(test.call(number::SHUTDOWN, [client, 0, 0, 0, 0, 0]), 0);
    assert_eq!(
        test.receive_from(client, 16, 0),
        0,
        "a read shut down ends the stream"
    );
}

#[test]
fn a_ping_socket_sends_echo_requests_and_reads_the_replies() {
    let mut test = Test::new();
    let fd = test.socket(IPPROTO_ICMP);
    let mut request = [0u8; icmp::HEADER_BYTES + 4];
    request[0] = icmp::kind::ECHO_REQUEST;
    request[6..8].copy_from_slice(&7u16.to_be_bytes());
    request[icmp::HEADER_BYTES..].copy_from_slice(b"egg!");
    assert_eq!(
        test.send_to(fd, &request, Some((Ipv4Address::LOOPBACK, 0)), 0),
        12
    );
    let (_, identifier) = test.local(fd);
    assert_eq!(test.receive_from(fd, 64, 0), 12);
    let reply = load(&test.process, BUFFER, 12);
    let echo = icmp::parse_echo(&reply).unwrap();
    assert_eq!(
        (echo.kind, echo.identifier, echo.sequence, echo.data),
        (icmp::kind::ECHO_REPLY, identifier, 7, &b"egg!"[..])
    );
    assert_eq!(
        loaded_address(&test.process),
        Some((Ipv4Address::LOOPBACK, 0))
    );
    let mut not_a_request = request;
    not_a_request[0] = 13;
    assert_eq!(
        test.send_to(fd, &not_a_request, Some((Ipv4Address::LOOPBACK, 0)), 0),
        Errno::EINVAL.result()
    );
}

#[test]
fn options_report_a_datagram_socket_and_keep_what_was_set() {
    let mut test = Test::new();
    let fd = test.socket(0);
    let get = |test: &mut Test, level: u32, name: u32| {
        store(&test.process, LENGTH, &4u32.to_ne_bytes());
        let result = test.call(
            number::GETSOCKOPT,
            [fd, level.into(), name.into(), BUFFER, LENGTH, 0],
        );
        assert_eq!(result, 0);
        u32::from_ne_bytes(load(&test.process, BUFFER, 4).try_into().unwrap())
    };
    assert_eq!(get(&mut test, SOL_SOCKET, option::TYPE), SOCK_DGRAM);
    assert_eq!(get(&mut test, SOL_SOCKET, option::DOMAIN), AF_INET);
    assert_eq!(get(&mut test, SOL_SOCKET, option::PROTOCOL), IPPROTO_UDP);
    assert_eq!(get(&mut test, SOL_IP, IP_TTL), 64);
    store(&test.process, DATA + 0xf00, &1i32.to_ne_bytes());
    let set = |test: &mut Test, level: u32, name: u32| {
        test.call(
            number::SETSOCKOPT,
            [fd, level.into(), name.into(), DATA + 0xf00, 4, 0],
        )
    };
    assert_eq!(set(&mut test, SOL_SOCKET, SO_BROADCAST), 0);
    assert_eq!(get(&mut test, SOL_SOCKET, SO_BROADCAST), 1);
    assert_eq!(set(&mut test, SOL_IP, IP_TTL), 0);
    assert_eq!(get(&mut test, SOL_IP, IP_TTL), 1);
    assert_eq!(set(&mut test, SOL_IP, 99), Errno::ENOPROTOOPT.result());
}
