use super::*;
use crate::linux::descriptors::{FD_CLOEXEC, command};
use crate::linux::{handle, number};
use crate::memory::FRAMES;
use crate::paging::PageTables;
use crate::tests::TestPlatform as P;
use crate::thread::host::frames_for_test;
use alloc::vec;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;
use eggos_hal::user::UserExit;

const DATA: u64 = 0x50_0000;
const PAGES: u64 = 4;

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

fn int_at(process: &Process<P>, address: u64) -> u32 {
    u32::from_ne_bytes(load(process, address, 4).try_into().unwrap())
}

const STREAM: u64 = SOCK_STREAM as u64;
const UNIX: u64 = AF_UNIX as u64;

fn pair(process: &Arc<Process<P>>, thread: &mut LinuxThread) -> (u64, u64) {
    assert_eq!(
        call(
            process,
            thread,
            number::SOCKETPAIR,
            [UNIX, STREAM, 0, DATA, 0, 0]
        ),
        0
    );
    (
        u64::from(int_at(process, DATA)),
        u64::from(int_at(process, DATA + 4)),
    )
}

fn abstract_address(name: &[u8]) -> Vec<u8> {
    let mut bytes = (AF_UNIX as u16).to_ne_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(name);
    bytes
}

fn header(
    name: u64,
    name_length: u32,
    iov: u64,
    count: u64,
    control: u64,
    control_length: u64,
) -> Vec<u8> {
    let mut bytes = vec![0u8; MSGHDR_SIZE];
    bytes[..8].copy_from_slice(&name.to_ne_bytes());
    bytes[8..12].copy_from_slice(&name_length.to_ne_bytes());
    bytes[16..24].copy_from_slice(&iov.to_ne_bytes());
    bytes[24..32].copy_from_slice(&count.to_ne_bytes());
    bytes[32..40].copy_from_slice(&control.to_ne_bytes());
    bytes[40..48].copy_from_slice(&control_length.to_ne_bytes());
    bytes
}

fn iov(base: u64, length: u64) -> Vec<u8> {
    [base.to_ne_bytes(), length.to_ne_bytes()].concat()
}

const SEND_HEADER: u64 = DATA + 0x100;
const SEND_IOV: u64 = DATA + 0x200;
const SEND_DATA: u64 = DATA + 0x300;
const SEND_CONTROL: u64 = DATA + 0x400;
const RECEIVE_HEADER: u64 = DATA + 0x1000;
const RECEIVE_IOV: u64 = DATA + 0x1100;
const RECEIVE_DATA: u64 = DATA + 0x1200;
const RECEIVE_CONTROL: u64 = DATA + 0x1300;

fn send_rights(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    fd: u64,
    descriptors: &[u32],
) -> u64 {
    store(process, SEND_DATA, b"x");
    store(process, SEND_IOV, &iov(SEND_DATA, 1));
    store(process, SEND_CONTROL, &rights_message(descriptors));
    let control_length = (CMSGHDR_SIZE + 4 * descriptors.len()) as u64;
    store(
        process,
        SEND_HEADER,
        &header(0, 0, SEND_IOV, 1, SEND_CONTROL, control_length),
    );
    call(
        process,
        thread,
        number::SENDMSG,
        [fd, SEND_HEADER, 0, 0, 0, 0],
    )
}

fn receive_rights(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    fd: u64,
    room: u64,
    flags: u32,
) -> u64 {
    store(process, RECEIVE_IOV, &iov(RECEIVE_DATA, 8));
    store(
        process,
        RECEIVE_HEADER,
        &header(0, 0, RECEIVE_IOV, 1, RECEIVE_CONTROL, room),
    );
    call(
        process,
        thread,
        number::RECVMSG,
        [fd, RECEIVE_HEADER, u64::from(flags), 0, 0, 0],
    )
}

#[test]
fn socket_s_errors_come_in_linux_s_order_and_its_flags_are_kept() {
    let _serial = unix_socket::tests_lock();
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    let socket = |thread: &mut LinuxThread, domain, kind, protocol| {
        call(
            &process,
            thread,
            number::SOCKET,
            [domain, kind, protocol, 0, 0, 0],
        )
    };
    assert_eq!(
        socket(&mut thread, 2, STREAM, 0),
        Errno::EAFNOSUPPORT.result()
    );
    assert_eq!(
        socket(&mut thread, UNIX, 2, 0),
        Errno::ESOCKTNOSUPPORT.result()
    );
    assert_eq!(
        socket(&mut thread, UNIX, 5, 0),
        Errno::ESOCKTNOSUPPORT.result()
    );
    assert_eq!(
        socket(&mut thread, UNIX, STREAM, 6),
        Errno::EPROTONOSUPPORT.result()
    );
    assert_eq!(
        socket(&mut thread, UNIX, STREAM | 0x100, 0),
        Errno::EINVAL.result()
    );
    assert_eq!(socket(&mut thread, UNIX, 12, 0), Errno::EINVAL.result());
    let flags = u64::from(open_flag::NONBLOCK | open_flag::CLOEXEC);
    let fd = socket(&mut thread, UNIX, STREAM | flags, 1);
    assert_eq!(fd, 3);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::FCNTL,
            [fd, u64::from(command::GETFL), 0, 0, 0, 0]
        ),
        u64::from(open_flag::RDWR | open_flag::NONBLOCK)
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::FCNTL,
            [fd, u64::from(command::GETFD), 0, 0, 0, 0]
        ),
        u64::from(FD_CLOEXEC)
    );
    assert_eq!(
        call(&process, &mut thread, number::LISTEN, [fd, 5, 0, 0, 0, 0]),
        Errno::EINVAL.result()
    );
    assert_eq!(
        call(&process, &mut thread, number::BIND, [0, DATA, 2, 0, 0, 0]),
        Errno::ENOTSOCK.result()
    );
    assert_eq!(
        call(&process, &mut thread, number::ACCEPT4, [fd, 0, 0, 0, 0, 0]),
        Errno::EINVAL.result()
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::GETPEERNAME,
            [fd, DATA, DATA + 0x80, 0, 0, 0]
        ),
        Errno::ENOTCONN.result()
    );
    assert_eq!(
        call(&process, &mut thread, number::LSEEK, [fd, 0, 0, 0, 0, 0]),
        Errno::ESPIPE.result()
    );
}

#[test]
fn a_pair_moves_bytes_both_ways_and_reports_epipe_once_one_end_closes() {
    let _serial = unix_socket::tests_lock();
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    let (a, b) = pair(&process, &mut thread);
    store(&process, SEND_DATA, b"abc");
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::WRITE,
            [a, SEND_DATA, 3, 0, 0, 0]
        ),
        3
    );
    store(&process, DATA + 0x80, &100i32.to_ne_bytes());
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::IOCTL,
            [
                b,
                u64::from(crate::linux::terminal::request::FIONREAD),
                DATA + 0x80,
                0,
                0,
                0
            ]
        ),
        0
    );
    assert_eq!(int_at(&process, DATA + 0x80), 3);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::READ,
            [b, RECEIVE_DATA, 8, 0, 0, 0]
        ),
        3
    );
    assert_eq!(load(&process, RECEIVE_DATA, 3), b"abc");
    let nonblocking = u64::from(message::DONTWAIT);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::RECVFROM,
            [b, RECEIVE_DATA, 8, nonblocking, 0, 0]
        ),
        Errno::EAGAIN.result()
    );
    let quiet = u64::from(message::NOSIGNAL);
    assert_eq!(
        call(&process, &mut thread, number::SHUTDOWN, [a, 1, 0, 0, 0, 0]),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::SENDTO,
            [a, SEND_DATA, 0, quiet, 0, 0]
        ),
        Errno::EPIPE.result()
    );
    assert_eq!(
        call(&process, &mut thread, number::CLOSE, [b, 0, 0, 0, 0, 0]),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::SENDTO,
            [a, SEND_DATA, 3, u64::from(message::NOSIGNAL), 0, 0]
        ),
        Errno::EPIPE.result()
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::RECVFROM,
            [a, RECEIVE_DATA, 8, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(&process, &mut thread, number::SHUTDOWN, [a, 3, 0, 0, 0, 0]),
        Errno::EINVAL.result()
    );
}

#[test]
fn scm_rights_installs_new_descriptors_on_the_same_open_files() {
    let _serial = unix_socket::tests_lock();
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    let (a, b) = pair(&process, &mut thread);
    assert_eq!(send_rights(&process, &mut thread, a, &[1]), 1);
    assert_eq!(unix_socket::in_flight(), 1);
    let room = 24;
    assert_eq!(
        receive_rights(&process, &mut thread, b, room, message::CMSG_CLOEXEC),
        1
    );
    let written = load(&process, RECEIVE_HEADER, MSGHDR_SIZE);
    assert_eq!(u64::from_ne_bytes(written[40..48].try_into().unwrap()), 24);
    assert_eq!(u32::from_ne_bytes(written[48..52].try_into().unwrap()), 0);
    let message = load(&process, RECEIVE_CONTROL, 20);
    assert_eq!(u64::from_ne_bytes(message[..8].try_into().unwrap()), 20);
    assert_eq!(
        u32::from_ne_bytes(message[8..12].try_into().unwrap()),
        SOL_SOCKET
    );
    assert_eq!(
        u32::from_ne_bytes(message[12..16].try_into().unwrap()),
        SCM_RIGHTS
    );
    let received = u32::from_ne_bytes(message[16..20].try_into().unwrap());
    assert_eq!(received, 5);
    let original = thread.files().file(1).unwrap();
    let passed = thread.files().file(u64::from(received)).unwrap();
    assert!(Arc::ptr_eq(&original, &passed));
    assert!(thread.files().lock().get(received).unwrap().close_on_exec);
    assert_eq!(unix_socket::in_flight(), 0);

    assert_eq!(send_rights(&process, &mut thread, a, &[1, 2]), 1);
    assert_eq!(receive_rights(&process, &mut thread, b, 20, 0), 1);
    let written = load(&process, RECEIVE_HEADER, MSGHDR_SIZE);
    assert_eq!(u64::from_ne_bytes(written[40..48].try_into().unwrap()), 20);
    assert_eq!(
        u32::from_ne_bytes(written[48..52].try_into().unwrap()),
        message::CTRUNC
    );
    assert!(!thread.files().lock().get(6).unwrap().close_on_exec);
    assert!(thread.files().lock().get(7).is_none());

    assert_eq!(send_rights(&process, &mut thread, a, &[1]), 1);
    assert_eq!(receive_rights(&process, &mut thread, b, 0, 0), 1);
    let written = load(&process, RECEIVE_HEADER, MSGHDR_SIZE);
    assert_eq!(
        u32::from_ne_bytes(written[48..52].try_into().unwrap()),
        message::CTRUNC
    );
    assert_eq!(unix_socket::in_flight(), 0);

    assert_eq!(
        send_rights(&process, &mut thread, a, &[40]),
        Errno::EBADF.result()
    );
    let own = u32::try_from(a).unwrap();
    assert_eq!(
        send_rights(&process, &mut thread, a, &[own]),
        Errno::ETOOMANYREFS.result()
    );
    assert_eq!(unix_socket::in_flight(), 0);
    store(&process, SEND_HEADER, &header(0, 0, SEND_IOV, 1025, 0, 0));
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::SENDMSG,
            [a, SEND_HEADER, 0, 0, 0, 0]
        ),
        Errno::EMSGSIZE.result()
    );
}

#[test]
fn an_abstract_listener_accepts_a_connection_with_its_peer_s_credentials() {
    let _serial = unix_socket::tests_lock();
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    let nonblocking = u64::from(open_flag::NONBLOCK);
    let listener = call(
        &process,
        &mut thread,
        number::SOCKET,
        [UNIX, STREAM | nonblocking, 0, 0, 0, 0],
    );
    let address = abstract_address(b"eggos-test");
    let length = address.len() as u64;
    store(&process, DATA, &address);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::BIND,
            [listener, DATA, length, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::BIND,
            [listener, DATA, length, 0, 0, 0]
        ),
        Errno::EINVAL.result()
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::LISTEN,
            [listener, u64::MAX, 0, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::ACCEPT4,
            [listener, 0, 0, 0, 0, 0]
        ),
        Errno::EAGAIN.result()
    );
    let client = call(
        &process,
        &mut thread,
        number::SOCKET,
        [UNIX, STREAM, 0, 0, 0, 0],
    );
    let taken = call(
        &process,
        &mut thread,
        number::SOCKET,
        [UNIX, STREAM, 0, 0, 0, 0],
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::BIND,
            [taken, DATA, length, 0, 0, 0]
        ),
        Errno::EADDRINUSE.result()
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::CONNECT,
            [client, DATA, 2, 0, 0, 0]
        ),
        Errno::EINVAL.result()
    );
    store(&process, DATA + 0x200, &abstract_address(b"nobody"));
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::CONNECT,
            [client, DATA + 0x200, 9, 0, 0, 0]
        ),
        Errno::ECONNREFUSED.result()
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::CONNECT,
            [client, DATA, length, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::CONNECT,
            [client, DATA, length, 0, 0, 0]
        ),
        Errno::EISCONN.result()
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::CONNECT,
            [client, DATA + 0x200, 9, 0, 0, 0]
        ),
        Errno::ECONNREFUSED.result()
    );
    store(&process, SEND_DATA, b"hi");
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::WRITE,
            [client, SEND_DATA, 2, 0, 0, 0]
        ),
        2
    );
    let peer_at = DATA + 0x800;
    let length_at = DATA + 0x900;
    store(
        &process,
        length_at,
        &(SOCKADDR_UN_SIZE as i32).to_ne_bytes(),
    );
    let cloexec = u64::from(open_flag::CLOEXEC);
    let accepted = call(
        &process,
        &mut thread,
        number::ACCEPT4,
        [listener, peer_at, length_at, cloexec, 0, 0],
    );
    assert!(accepted < 64, "accepted {accepted:#x}");
    assert_eq!(int_at(&process, length_at), 2);
    assert!(
        thread
            .files()
            .lock()
            .get(accepted as u32)
            .unwrap()
            .close_on_exec
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::READ,
            [accepted, RECEIVE_DATA, 8, 0, 0, 0]
        ),
        2
    );
    assert_eq!(load(&process, RECEIVE_DATA, 2), b"hi");
    store(&process, length_at, &(UCRED_SIZE as i32).to_ne_bytes());
    let level = u64::from(SOL_SOCKET);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::GETSOCKOPT,
            [
                accepted,
                level,
                u64::from(option::PEERCRED),
                peer_at,
                length_at,
                0
            ]
        ),
        0
    );
    assert_eq!(int_at(&process, peer_at), process.id().get());
    store(
        &process,
        length_at,
        &(SOCKADDR_UN_SIZE as i32).to_ne_bytes(),
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::GETPEERNAME,
            [client, peer_at, length_at, 0, 0, 0]
        ),
        0
    );
    assert_eq!(int_at(&process, length_at), length as u32);
    assert_eq!(load(&process, peer_at, address.len()), address);
    store(&process, length_at, &4i32.to_ne_bytes());
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::GETSOCKOPT,
            [
                listener,
                level,
                u64::from(option::ACCEPTCONN),
                peer_at,
                length_at,
                0
            ]
        ),
        0
    );
    assert_eq!(int_at(&process, peer_at), 1);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::GETSOCKOPT,
            [listener, level, 1000, peer_at, length_at, 0]
        ),
        Errno::ENOPROTOOPT.result()
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::GETSOCKOPT,
            [listener, 6, u64::from(option::TYPE), peer_at, length_at, 0]
        ),
        Errno::EOPNOTSUPP.result()
    );
    for fd in [listener, client, taken, accepted] {
        assert_eq!(
            call(&process, &mut thread, number::CLOSE, [fd, 0, 0, 0, 0, 0]),
            0
        );
    }
    assert!(!unix_socket::has_bindings());
}

#[test]
fn addresses_parse_and_encode_as_linux_counts_them() {
    let family = (AF_UNIX as u16).to_ne_bytes();
    let with = |rest: &[u8]| [&family[..], rest].concat();
    assert_eq!(parse_address(&family), Ok(Address::Unnamed));
    let path = Address::Path(Arc::from(&b"/run/wayland-0"[..]));
    assert_eq!(parse_address(&with(b"/run/wayland-0")), Ok(path.clone()));
    assert_eq!(parse_address(&with(b"/run/wayland-0\0")), Ok(path.clone()));
    assert_eq!(
        parse_address(&with(b"/run/wayland-0\0junk")),
        Ok(path.clone())
    );
    assert_eq!(
        parse_address(&with(b"\0name")),
        Ok(Address::Abstract(Arc::from(&b"name"[..])))
    );
    assert_eq!(parse_address(&family[..1]), Err(Errno::EINVAL));
    assert_eq!(
        parse_address(&[0u8; SOCKADDR_UN_SIZE + 1]),
        Err(Errno::EINVAL)
    );
    assert_eq!(parse_address(&[2, 0, b'x']), Err(Errno::EINVAL));
    let (bytes, length) = encode_address(&path);
    assert_eq!(length, 2 + 14 + 1);
    assert_eq!(&bytes[2..length], b"/run/wayland-0\0");
    let (bytes, length) = encode_address(&Address::Abstract(Arc::from(&b"name"[..])));
    assert_eq!(length, 2 + 1 + 4);
    assert_eq!(&bytes[2..length], b"\0name");
    assert_eq!(encode_address(&Address::Unnamed).1, 2);
}

#[test]
fn control_messages_are_read_as_linux_reads_them() {
    let message = |level: u32, kind: u32, descriptors: &[i32]| {
        let length = CMSGHDR_SIZE + 4 * descriptors.len();
        let mut bytes = (length as u64).to_ne_bytes().to_vec();
        bytes.extend_from_slice(&level.to_ne_bytes());
        bytes.extend_from_slice(&kind.to_ne_bytes());
        for fd in descriptors {
            bytes.extend_from_slice(&fd.to_ne_bytes());
        }
        bytes.resize(length.next_multiple_of(8), 0);
        bytes
    };
    let two = [
        message(SOL_SOCKET, SCM_RIGHTS, &[3]),
        message(SOL_SOCKET, SCM_RIGHTS, &[4, 5]),
    ]
    .concat();
    assert_eq!(parse_rights(&two), Ok(vec![3, 4, 5]));
    let other_level = [message(6, 1, &[9]), message(SOL_SOCKET, SCM_RIGHTS, &[7])].concat();
    assert_eq!(parse_rights(&other_level), Ok(vec![7]));
    assert_eq!(
        parse_rights(&message(SOL_SOCKET, 2, &[1, 2, 3])),
        Err(Errno::EINVAL)
    );
    let mut short = message(SOL_SOCKET, SCM_RIGHTS, &[3]);
    short[..8].copy_from_slice(&8u64.to_ne_bytes());
    assert_eq!(parse_rights(&short), Err(Errno::EINVAL));
    let mut long = message(SOL_SOCKET, SCM_RIGHTS, &[3]);
    long[..8].copy_from_slice(&64u64.to_ne_bytes());
    assert_eq!(parse_rights(&long), Err(Errno::EINVAL));
    let too_many: Vec<i32> = (0..254).collect();
    assert_eq!(
        parse_rights(&message(SOL_SOCKET, SCM_RIGHTS, &too_many)),
        Err(Errno::EINVAL)
    );
    let most: Vec<i32> = (0..253).collect();
    assert_eq!(
        parse_rights(&message(SOL_SOCKET, SCM_RIGHTS, &most)).map(|fds| fds.len()),
        Ok(253)
    );
    assert_eq!(parse_rights(&[0u8; 8]), Ok(vec![]));
}

#[test]
fn the_backlog_is_read_unsigned_and_capped_and_timeouts_are_checked() {
    assert_eq!(backlog_limit(-1i64 as u64), unix_socket::MAX_BACKLOG);
    assert_eq!(backlog_limit(5), 5);
    assert_eq!(backlog_limit(1 << 32 | 3), 3);
    assert_eq!(backlog_limit(0), 0);
    let timeval = |seconds: i64, microseconds: i64| {
        let mut bytes = [0u8; TIMEVAL_SIZE];
        bytes[..8].copy_from_slice(&seconds.to_ne_bytes());
        bytes[8..].copy_from_slice(&microseconds.to_ne_bytes());
        bytes
    };
    assert_eq!(timeout_nanoseconds(timeval(1, 500_000)), Ok(1_500_000_000));
    assert_eq!(timeout_nanoseconds(timeval(0, 0)), Ok(0));
    assert_eq!(timeout_nanoseconds(timeval(-1, 0)), Ok(0));
    assert_eq!(timeout_nanoseconds(timeval(1, 1_000_000)), Err(Errno::EDOM));
    assert_eq!(timeout_nanoseconds(timeval(1, -1)), Err(Errno::EDOM));
}

#[test]
fn a_connect_that_ends_with_its_process_leaves_the_socket_open() {
    let _serial = unix_socket::tests_lock();
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    let listener = call(
        &process,
        &mut thread,
        number::SOCKET,
        [UNIX, STREAM, 0, 0, 0, 0],
    );
    let address = abstract_address(b"eggos-full");
    let length = address.len() as u64;
    store(&process, DATA, &address);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::BIND,
            [listener, DATA, length, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::LISTEN,
            [listener, 0, 0, 0, 0, 0]
        ),
        0
    );
    let first = call(
        &process,
        &mut thread,
        number::SOCKET,
        [UNIX, STREAM, 0, 0, 0, 0],
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::CONNECT,
            [first, DATA, length, 0, 0, 0]
        ),
        0
    );
    let waiting = call(
        &process,
        &mut thread,
        number::SOCKET,
        [UNIX, STREAM, 0, 0, 0, 0],
    );
    let socket = match thread.files().file(waiting).unwrap().kind() {
        FileKind::Socket(socket) => Arc::clone(socket),
        _ => panic!("not a socket"),
    };
    assert!(process.exit(crate::process::ProcessExit::Exited(0)));
    let mut context = UserContext::new(0x40_0000, 0);
    context.simulate_syscall(number::CONNECT, [waiting, DATA, length, 0, 0, 0], 0x40_0004);
    assert_eq!(
        handle(&process, &mut thread, &mut context, UserExit::Syscall),
        ControlFlow::Break(())
    );
    assert_eq!(socket.begin_connect(), Ok(Address::Unnamed));
    socket.abandon_connect();
}
