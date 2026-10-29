//! The network boot test (D157), on every architecture: the release image with
//! `/bin/linux-udp` added, booted with QEMU's user networking ([`network_args`]), where QEMU's
//! DHCP server gives EggOS 10.0.2.15 and the host is 10.0.2.2. At the shell's prompts the test
//! types `ip addr` (the lease), `cat /etc/resolv.conf` and `ping 10.0.2.2` (QEMU answers),
//! `nslookup` against a DNS stand-in the test serves on the host, and `linux-udp`, a static
//! musl program using `std::net`: over the loopback network, then with a UDP echo server the
//! test serves on the host. `exit` then powers the machine off.

use super::*;
use eggos_net::Ipv4Address;
use eggos_net::dns;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where the test adds the `std::net` test program.
pub const PROGRAM_PATH: &str = "bin/linux-udp";
/// The name the DNS stand-in answers.
pub const KNOWN_NAME: &str = "eggos.test";
/// The address it answers with.
pub const KNOWN_ADDRESS: Ipv4Address = Ipv4Address::new(192, 0, 2, 7);

const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);
const SERVER_POLL: Duration = Duration::from_millis(200);
const BOOT_LINE: &str = "net: eth0 52:54:00:12:34:56: asking for an address by DHCP";

fn commands(dns_port: u16, echo_port: u16) -> Vec<String> {
    vec![
        "sleep 2; ip addr; echo EGGOS-NET ip $?\r".into(),
        "cat /etc/resolv.conf; ping -c 2 10.0.2.2; echo EGGOS-NET ping $?\r".into(),
        format!(
            "nslookup -port={dns_port} {KNOWN_NAME} 10.0.2.2; echo EGGOS-NET nslookup $?\r"
        ),
        format!("linux-udp; linux-udp 10.0.2.2 {echo_port}; echo EGGOS-NET udp $?\r"),
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
/// for `arch` with `/bin/linux-udp`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    eprintln!("xtask: booting {} in {} (network)", arch.name, arch.qemu);
    let stop = Arc::new(AtomicBool::new(false));
    let (dns_port, dns_server) = serve(Arc::clone(&stop), dns_answer)?;
    let (echo_port, echo_server) = serve(Arc::clone(&stop), |datagram| Some(datagram.to_vec()))?;
    let result = boot(arch, kernel, &commands(dns_port, echo_port));
    stop.store(true, Ordering::Relaxed);
    let _ = dns_server.join();
    let _ = echo_server.join();
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
        assert!(commands(53, 7)[2].contains("-port=53 eggos.test 10.0.2.2"));
    }
}
