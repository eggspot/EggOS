use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::ExitCode;
use std::time::Duration;

const PREFIX: &str = "LINUX-UDP";
const ECHO_REQUEST: &[u8] = b"EGGOS-UDP hello";
const TIMEOUT: Duration = Duration::from_secs(5);
const ECHO_TRIES: usize = 3;

type Checked<T> = Result<T, String>;

fn failed(what: &str, error: std::io::Error) -> String {
    format!("{what}: {error}")
}

fn loopback() -> Checked<()> {
    let receiver = UdpSocket::bind("127.0.0.1:0").map_err(|error| failed("bind", error))?;
    let address = receiver
        .local_addr()
        .map_err(|error| failed("local_addr", error))?;
    let sender = UdpSocket::bind("0.0.0.0:0").map_err(|error| failed("bind any", error))?;
    sender
        .send_to(b"through lo", address)
        .map_err(|error| failed("send_to", error))?;
    receiver
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|error| failed("set_read_timeout", error))?;
    let mut buffer = [0u8; 64];
    let (count, from) = receiver
        .recv_from(&mut buffer)
        .map_err(|error| failed("recv_from", error))?;
    let sender_port = sender
        .local_addr()
        .map_err(|error| failed("sender's local_addr", error))?
        .port();
    if &buffer[..count] != b"through lo" || from.port() != sender_port {
        return Err(format!("got {:?} from {from}", &buffer[..count]));
    }
    receiver
        .set_nonblocking(true)
        .map_err(|error| failed("set_nonblocking", error))?;
    let empty = match receiver.recv_from(&mut buffer) {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => "EAGAIN",
        Err(_) => "another error",
        Ok(_) => "a datagram",
    };
    println!("{PREFIX} loopback ok from 127.0.0.1 then {empty}");
    Ok(())
}

fn localhost() -> Checked<()> {
    let addresses: Vec<SocketAddr> = ("localhost", 7)
        .to_socket_addrs()
        .map_err(|error| failed("resolve localhost", error))?
        .collect();
    match addresses.first() {
        Some(address) if address.ip() == Ipv4Addr::LOCALHOST => {
            println!("{PREFIX} localhost {}", address.ip());
            Ok(())
        }
        other => Err(format!("localhost resolved to {other:?}")),
    }
}

fn echo(host: &str, port: &str) -> Checked<()> {
    let port: u16 = port.parse().map_err(|_| format!("bad port {port}"))?;
    let host: Ipv4Addr = host.parse().map_err(|_| format!("bad host {host}"))?;
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|error| failed("bind", error))?;
    socket
        .connect((host, port))
        .map_err(|error| failed("connect", error))?;
    socket
        .set_read_timeout(Some(TIMEOUT / ECHO_TRIES as u32))
        .map_err(|error| failed("set_read_timeout", error))?;
    let mut buffer = [0u8; 256];
    for _ in 0..ECHO_TRIES {
        socket
            .send(ECHO_REQUEST)
            .map_err(|error| failed("send", error))?;
        match socket.recv(&mut buffer) {
            Ok(count) => {
                let reply = String::from_utf8_lossy(&buffer[..count]);
                println!("{PREFIX} echo {reply}");
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(failed("recv", error)),
        }
    }
    Err(format!("no echo from {host}:{port}"))
}

pub fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let checked = match arguments.as_slice() {
        [] => loopback().and_then(|()| localhost()),
        [host, port] => echo(host, port),
        _ => Err("usage: linux-udp [host port]".to_owned()),
    };
    match checked {
        Ok(()) => {
            println!("{PREFIX} status 0");
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{PREFIX} failed: {error}");
            ExitCode::FAILURE
        }
    }
}
