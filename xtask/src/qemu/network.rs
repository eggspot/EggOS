//! The network boot test (D157, D158), on every architecture: the release image with
//! `/bin/linux-udp` and `/bin/linux-tcp` added, booted with QEMU's user networking ([`network_args`]), where QEMU's
//! DHCP server gives EggOS 10.0.2.15 and the host is 10.0.2.2. At the shell's prompts the test
//! types `ip addr` (the lease), `cat /etc/resolv.conf` and `ping 10.0.2.2` (QEMU answers),
//! `nslookup` against a DNS stand-in the test serves on the host, and `linux-udp`, a static
//! musl program using `std::net`: over the loopback network, then with a UDP echo server the
//! test serves on the host; `fetch` downloads a file from an HTTP server the test serves on the
//! host, and `linux-tcp`, `std::net`'s TCP over the loopback network and with a TCP echo server
//! on the host. `exit` then powers the machine off.

use super::*;
use eggos_net::Ipv4Address;
use eggos_net::dns;
use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where the test adds the `std::net` test program.
pub const PROGRAM_PATH: &str = "bin/linux-udp";
/// Where the test adds the `std::net` TCP test program.
pub const TCP_PROGRAM_PATH: &str = "bin/linux-tcp";
/// The file the HTTP stand-in serves.
pub const SERVED_PATH: &str = "/eggos.txt";
/// What it holds.
pub const SERVED_TEXT: &str = "EGGOS-FETCH served by the host\n";
/// The name the DNS stand-in answers.
pub const KNOWN_NAME: &str = "eggos.test";
/// The address it answers with.
pub const KNOWN_ADDRESS: Ipv4Address = Ipv4Address::new(192, 0, 2, 7);

const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);
const SERVER_POLL: Duration = Duration::from_millis(200);
const BOOT_LINE: &str = "net: eth0 52:54:00:12:34:56: asking for an address by DHCP";

/// The host ports the test's stand-ins listen on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    /// The DNS stand-in (UDP).
    pub dns: u16,
    /// The UDP echo server.
    pub echo: u16,
    /// The TCP echo server.
    pub tcp_echo: u16,
    /// The HTTP stand-in.
    pub http: u16,
}

fn commands(ports: &Ports) -> Vec<String> {
    let Ports {
        dns,
        echo,
        tcp_echo,
        http,
    } = *ports;
    vec![
        "sleep 2; ip addr; echo EGGOS-NET ip $?\r".into(),
        "cat /etc/resolv.conf; ping -c 2 10.0.2.2; echo EGGOS-NET ping $?\r".into(),
        format!(
            "nslookup -port={dns} {KNOWN_NAME} 10.0.2.2; echo EGGOS-NET nslookup $?\r"
        ),
        format!("linux-udp; linux-udp 10.0.2.2 {echo}; echo EGGOS-NET udp $?\r"),
        format!(
            "fetch -o /tmp/fetched http://10.0.2.2:{http}{SERVED_PATH}; cat /tmp/fetched; \
             echo EGGOS-NET fetch $?\r"
        ),
        format!("linux-tcp; linux-tcp 10.0.2.2 {tcp_echo}; echo EGGOS-NET tcp $?\r"),
        "exit\r".into(),
    ]
}

fn wanted() -> Vec<String> {
    vec![
        BOOT_LINE.into(),
        "inet 10.0.2.15/24 brd 10.0.2.255 scope global dynamic eth0".into(),
        "EGGOS-NET ip 0".into(),
        "nameserver 10.0.2.3".into(),
        "bytes from 10.0.2.2: icmp_seq=1".into(),
        "EGGOS-NET ping 0".into(),
        format!("Address: {KNOWN_ADDRESS}"),
        "EGGOS-NET nslookup 0".into(),
        "LINUX-UDP loopback ok from 127.0.0.1 then EAGAIN".into(),
        "LINUX-UDP localhost 127.0.0.1".into(),
        "LINUX-UDP echo EGGOS-UDP hello".into(),
        "EGGOS-NET udp 0".into(),
        format!("Saved {} bytes to /tmp/fetched (HTTP 200 OK)", SERVED_TEXT.len()),
        SERVED_TEXT.trim_end().into(),
        "EGGOS-NET fetch 0".into(),
        "LINUX-TCP loopback 300000 bytes and back, accept EAGAIN, closed ECONNREFUSED".into(),
        "LINUX-TCP echo EGGOS-TCP hello".into(),
        "EGGOS-NET tcp 0".into(),
    ]
}

fn command_due(console: &str, typed: usize, count: usize) -> bool {
    typed < count && console.matches(SHELL_PROMPT).count() > typed
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    for wanted in wanted() {
        if !console.contains(&wanted) {
            return Err(format!("{arch}: the network boot test's console has no `{wanted}`"));
        }
    }
    if !clean_exit {
        return Err(format!("{arch}: QEMU did not exit cleanly after `exit`"));
    }
    Ok(())
}

/// The answer the DNS stand-in gives `query`: [`KNOWN_ADDRESS`] for [`KNOWN_NAME`], no such
/// name for any other.
pub fn dns_answer(query: &[u8]) -> Option<Vec<u8>> {
    let name = dns::query_name(query)?;
    let addresses: &[Ipv4Address] = match name.as_bytes() == KNOWN_NAME.as_bytes() {
        true => &[KNOWN_ADDRESS],
        false => &[],
    };
    let mut answer = [0u8; dns::MAX_MESSAGE_BYTES];
    let length = dns::write_answer(&mut answer, query, addresses, 60)?;
    Some(answer[..length].to_vec())
}

/// The answer the HTTP stand-in gives `request`: [`SERVED_TEXT`] for a `GET` of
/// [`SERVED_PATH`], 404 for anything else.
pub fn http_answer(request: &[u8]) -> Vec<u8> {
    let wanted = format!("GET {SERVED_PATH} HTTP/1.1\r\n");
    match request.starts_with(wanted.as_bytes()) {
        true => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{SERVED_TEXT}",
            SERVED_TEXT.len()
        )
        .into_bytes(),
        false => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
    }
}

fn serve_tcp(
    stop: Arc<AtomicBool>,
    answer: fn(&[u8]) -> Option<Vec<u8>>,
) -> Result<(u16, std::thread::JoinHandle<()>), String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("a host TCP socket: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let server = std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(SERVER_POLL);
                continue;
            };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(SERVER_POLL * 10));
            let mut received = Vec::new();
            let mut buffer = [0u8; 2048];
            while let Ok(count @ 1..) = stream.read(&mut buffer) {
                received.extend_from_slice(&buffer[..count]);
                if let Some(reply) = answer(&received) {
                    let _ = stream.write_all(&reply);
                    break;
                }
            }
        }
    });
    Ok((port, server))
}

fn serve(
    stop: Arc<AtomicBool>,
    answer: fn(&[u8]) -> Option<Vec<u8>>,
) -> Result<(u16, std::thread::JoinHandle<()>), String> {
    let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("a host UDP socket: {e}"))?;
    socket
        .set_read_timeout(Some(SERVER_POLL))
        .map_err(|e| e.to_string())?;
    let port = socket.local_addr().map_err(|e| e.to_string())?.port();
    let server = std::thread::spawn(move || {
        let mut buffer = [0u8; 2048];
        while !stop.load(Ordering::Relaxed) {
            let Ok((count, from)) = socket.recv_from(&mut buffer) else {
                continue;
            };
            if let Some(reply) = answer(&buffer[..count]) {
                let _ = socket.send_to(&reply, from);
            }
        }
    });
    Ok((port, server))
}

/// Runs the network boot test (see the module documentation) on `kernel`, the release image
/// for `arch` with `/bin/linux-udp` and `/bin/linux-tcp`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    eprintln!("xtask: booting {} in {} (network)", arch.name, arch.qemu);
    let stop = Arc::new(AtomicBool::new(false));
    let (dns_port, dns_server) = serve(Arc::clone(&stop), dns_answer)?;
    let (echo_port, echo_server) = serve(Arc::clone(&stop), |datagram| Some(datagram.to_vec()))?;
    let (tcp_echo_port, tcp_echo_server) =
        serve_tcp(Arc::clone(&stop), |received| Some(received.to_vec()))?;
    let (http_port, http_server) = serve_tcp(Arc::clone(&stop), |request| {
        request
            .windows(4)
            .any(|end| end == b"\r\n\r\n")
            .then(|| http_answer(request))
    })?;
    let ports = Ports {
        dns: dns_port,
        echo: echo_port,
        tcp_echo: tcp_echo_port,
        http: http_port,
    };
    let result = boot(arch, kernel, &commands(&ports));
    stop.store(true, Ordering::Relaxed);
    for server in [dns_server, echo_server, tcp_echo_server, http_server] {
        let _ = server.join();
    }
    result?;
    eprintln!("xtask: {}: network boot test passed", arch.name);
    Ok(())
}

fn boot(arch: &Arch, kernel: &Path, commands: &[String]) -> Result<(), String> {
    let mut child = Command::new(arch.qemu)
        .args(interactive_args(arch, false))
        .args(network_args(arch))
        .arg("-kernel")
        .arg(kernel)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;
    let output = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
    let mut keyboard = child.stdin.take().expect("stdin is piped");
    let started = Instant::now();
    let mut console = Vec::new();
    let mut typed = 0;
    let status = loop {
        console.extend(output.try_iter().flatten());
        if command_due(&String::from_utf8_lossy(&console), typed, commands.len()) {
            keyboard
                .write_all(commands[typed].as_bytes())
                .and_then(|()| keyboard.flush())
                .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
            typed += 1;
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > NETWORK_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: the network boot test did not power off within {}s",
                arch.name,
                NETWORK_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    console.extend(output.iter().flatten());
    verdict(
        arch.name,
        &String::from_utf8_lossy(&console),
        status.success(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_command_waits_for_a_prompt_of_its_own() {
        let prompt = format!("{SHELL_PROMPT}# ");
        assert!(!command_due("", 0, 5));
        assert!(command_due(&prompt, 0, 5));
        assert!(!command_due(&prompt, 1, 5));
        assert!(command_due(&prompt.repeat(2), 1, 5));
        assert!(!command_due(&prompt.repeat(9), 5, 5));
    }

    #[test]
    fn the_verdict_wants_every_line_and_a_clean_exit() {
        let all = wanted().join("\n");
        assert_eq!(verdict("x86_64", &all, true), Ok(()));
        assert!(verdict("x86_64", &all, false).is_err());
        let without_echo = all.replace("LINUX-UDP echo EGGOS-UDP hello", "");
        assert!(verdict("x86_64", &without_echo, true).is_err());
    }

    #[test]
    fn the_http_stand_in_serves_its_file_and_answers_404_to_the_rest() {
        let served = http_answer(b"GET /eggos.txt HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(served.ends_with(SERVED_TEXT.as_bytes()));
        let end = eggos_net::http::head_end(&served).unwrap();
        let head = eggos_net::http::parse_head(&served[..end]).unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(
            head.framing,
            eggos_net::http::Framing::Length(SERVED_TEXT.len() as u64)
        );
        assert!(http_answer(b"GET /other HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 404"));
    }

    #[test]
    fn the_dns_stand_in_answers_its_name_and_refuses_others() {
        let mut query = [0u8; dns::MAX_MESSAGE_BYTES];
        let length = dns::write_query(&mut query, 9, KNOWN_NAME.as_bytes(), dns::record_type::A)
            .unwrap();
        let answer = dns_answer(&query[..length]).unwrap();
        assert_eq!(
            dns::parse_response(&answer, 9).unwrap().addresses(),
            &[KNOWN_ADDRESS]
        );
        let length = dns::write_query(&mut query, 9, b"other.test", dns::record_type::A).unwrap();
        let answer = dns_answer(&query[..length]).unwrap();
        assert_eq!(
            dns::parse_response(&answer, 9),
            Err(dns::ResponseError::NameNotFound)
        );
        let ports = Ports {
            dns: 53,
            echo: 7,
            tcp_echo: 8,
            http: 8080,
        };
        assert!(commands(&ports)[2].contains("-port=53 eggos.test 10.0.2.2"));
        assert!(commands(&ports)[4].contains("http://10.0.2.2:8080/eggos.txt"));
        assert!(commands(&ports)[5].contains("linux-tcp 10.0.2.2 8;"));
    }
}
