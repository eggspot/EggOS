use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

const PREFIX: &str = "LINUX-TCP";
const ECHO_REQUEST: &[u8] = b"EGGOS-TCP hello";
const TIMEOUT: Duration = Duration::from_secs(10);
const LARGE_BYTES: usize = 300_000;

type Checked<T> = Result<T, String>;

fn failed(what: &str, error: std::io::Error) -> String {
    format!("{what}: {error}")
}

fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 251) as u8).collect()
}

fn loopback() -> Checked<()> {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|error| failed("bind", error))?;
    let address = listener
        .local_addr()
        .map_err(|error| failed("local_addr", error))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| failed("set_nonblocking", error))?;
    let empty = match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => "EAGAIN",
        Err(_) => "another error",
        Ok(_) => "a connection",
    };
    listener
        .set_nonblocking(false)
        .map_err(|error| failed("set_blocking", error))?;
    let client = std::thread::spawn(move || -> Checked<Vec<u8>> {
        let mut stream = TcpStream::connect(address).map_err(|error| failed("connect", error))?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .map_err(|error| failed("set_read_timeout", error))?;
        stream
            .write_all(&pattern(LARGE_BYTES))
            .map_err(|error| failed("write_all", error))?;
        stream
            .shutdown(Shutdown::Write)
            .map_err(|error| failed("shutdown", error))?;
        let mut answer = Vec::new();
        stream
            .read_to_end(&mut answer)
            .map_err(|error| failed("client read_to_end", error))?;
        Ok(answer)
    });
    let (mut server, peer) = listener.accept().map_err(|error| failed("accept", error))?;
    server
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|error| failed("set_read_timeout", error))?;
    let mut received = Vec::new();
    server
        .read_to_end(&mut received)
        .map_err(|error| failed("server read_to_end", error))?;
    if received != pattern(LARGE_BYTES) {
        return Err(format!(
            "the server got {} bytes, not the pattern",
            received.len()
        ));
    }
    server
        .write_all(b"done")
        .map_err(|error| failed("server write_all", error))?;
    drop(server);
    let answer = client
        .join()
        .map_err(|_| "the client thread panicked".to_owned())??;
    if answer != b"done" || !peer.ip().is_loopback() {
        return Err(format!("the client got {answer:?} from {peer}"));
    }
    drop(listener);
    let closed = match TcpStream::connect(address) {
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => "ECONNREFUSED",
        Err(_) => "another error",
        Ok(_) => "a connection",
    };
    println!("{PREFIX} loopback {LARGE_BYTES} bytes and back, accept {empty}, closed {closed}");
    Ok(())
}

fn echo(host: &str, port: &str) -> Checked<()> {
    let port: u16 = port.parse().map_err(|_| format!("bad port {port}"))?;
    let host: Ipv4Addr = host.parse().map_err(|_| format!("bad host {host}"))?;
    let mut stream = TcpStream::connect_timeout(&(host, port).into(), TIMEOUT)
        .map_err(|error| failed("connect", error))?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|error| failed("set_read_timeout", error))?;
    stream
        .write_all(ECHO_REQUEST)
        .map_err(|error| failed("write_all", error))?;
    let mut reply = vec![0u8; ECHO_REQUEST.len()];
    stream
        .read_exact(&mut reply)
        .map_err(|error| failed("read_exact", error))?;
    println!("{PREFIX} echo {}", String::from_utf8_lossy(&reply));
    Ok(())
}

pub fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let checked = match arguments.as_slice() {
        [] => loopback(),
        [host, port] => echo(host, port),
        _ => Err("usage: linux-tcp [host port]".to_owned()),
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
