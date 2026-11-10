//! The HTTPS boot test (D225, D159's last check), on every architecture: one boot of the
//! release kernel on QEMU's user network with a host folder holding only `roots.pem`. The
//! test runs two TLS 1.3 servers on the host with our own `eggos_tls::server`, each with a
//! self-signed certificate made for this run that names 10.0.2.2 (the host as EggOS sees it);
//! the folder's roots hold the first one's certificate. `fetch https://` saves a file of
//! several records from the first server, which `wc` and `grep` check, and is refused by the
//! second with fetch's words for an untrusted issuer; `download list --json` then shows the
//! first download done and the second failed. `poweroff` ends the boot.

use super::settings::Boot;
use super::*;
use eggos_tls::CertificateError;
use eggos_tls::certificate::{self, MAX_CERTIFICATE_BYTES, Name, Request};
use eggos_tls::client::{Buffers, Stopped, Transport};
use eggos_tls::server::{self as tls_server, Identity};
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const HTTPS_TIMEOUT: Duration = Duration::from_secs(150);
const SERVER_POLL: Duration = Duration::from_millis(100);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const HOST_ADDRESS: [u8; 4] = [10, 0, 2, 2];
const HOST_TEXT: &str = "10.0.2.2";
const VALIDITY_MARGIN_SECONDS: i64 = 24 * 60 * 60;
const PROTOCOL: &[u8] = b"http/1.1";
const ROOTS_FILE: &str = "roots.pem";
const SERVED_PATH: &str = "/eggos-https.txt";
const SERVED_LINES: usize = 2400;
const FIRST_LINE: &str = "EGGOS-HTTPS line 00000";
const LAST_LINE: &str = "EGGOS-HTTPS end";
const SAVED_TO: &str = "/tmp/eggos-https.txt";
const REFUSED_TO: &str = "/tmp/eggos-untrusted.txt";
const LEASE_COMMAND: &[u8] = b"sleep 1; ip addr\r";
const LEASED_ADDRESS: &str = "inet 10.0.2.15/24";
const LEASE_CHECKS: usize = 30;
const POWER_OFF: &[u8] = b"poweroff\r";
const NOT_FOUND: &[u8] =
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ports {
    trusted: u16,
    untrusted: u16,
}

#[derive(Debug, Clone)]
struct Credential {
    private_key: [u8; 32],
    certificate: Vec<u8>,
}

#[derive(Debug)]
struct Stream(TcpStream);

impl Transport for Stream {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped> {
        self.0.write_all(bytes).map_err(|_| Stopped)
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped> {
        self.0.read(buffer).map_err(|_| Stopped)
    }
}

#[derive(Debug, Default)]
struct StandIns {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl StandIns {
    fn serve(&mut self, credential: Credential) -> Result<u16, String> {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|e| format!("a host TCP socket: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::clone(&self.stop);
        self.threads.push(std::thread::spawn(move || {
            let mut buffers = Box::new(Buffers::new());
            while !stop.load(Ordering::Relaxed) {
                let Ok((socket, _)) = listener.accept() else {
                    std::thread::sleep(SERVER_POLL);
                    continue;
                };
                if let Err(error) = answer_connection(socket, &credential, &mut buffers) {
                    eprintln!("xtask: the HTTPS stand-in on port {port}: {error}");
                }
            }
        }));
        Ok(port)
    }
}

impl Drop for StandIns {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    for (index, chunk) in bytes.chunks_mut(8).enumerate() {
        let value = RandomState::new().hash_one(index);
        chunk.copy_from_slice(&value.to_le_bytes()[..chunk.len()]);
    }
    bytes
}

fn unix_now() -> Result<i64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .map_err(|e| format!("the host's clock: {e}"))
}

fn credential(now: i64) -> Result<Credential, String> {
    let mut private_key: [u8; 32] = random_bytes();
    private_key[0] &= 0x7f;
    let names = [Name::Ipv4(HOST_ADDRESS)];
    let request = Request {
        private_key: &private_key,
        names: &names,
        serial: random_bytes(),
        not_before: now - VALIDITY_MARGIN_SECONDS,
        not_after: now + VALIDITY_MARGIN_SECONDS,
    };
    let mut der = [0u8; MAX_CERTIFICATE_BYTES];
    let length = certificate::self_signed(&request, &mut der)
        .map_err(|error| format!("the test certificate: {error:?}"))?;
    Ok(Credential {
        private_key,
        certificate: der[..length].to_vec(),
    })
}

fn pem(der: &[u8]) -> Result<Vec<u8>, String> {
    let mut text = vec![0u8; certificate::pem_bytes(der.len())];
    let length = certificate::pem(der, &mut text)
        .ok_or_else(|| "the test certificate does not fit its PEM".to_owned())?;
    text.truncate(length);
    Ok(text)
}

fn served_body() -> String {
    let mut body: String = (0..SERVED_LINES)
        .map(|line| format!("EGGOS-HTTPS line {line:05}\n"))
        .collect();
    body.push_str(LAST_LINE);
    body.push('\n');
    body
}

fn http_answer(request: &[u8]) -> Vec<u8> {
    let wanted = format!("GET {SERVED_PATH} HTTP/1.1\r\n");
    match request.starts_with(wanted.as_bytes()) {
        true => {
            let body = served_body();
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes()
        }
        false => NOT_FOUND.to_vec(),
    }
}

fn answer_connection(
    socket: TcpStream,
    credential: &Credential,
    buffers: &mut Buffers,
) -> Result<(), String> {
    socket.set_nonblocking(false).map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(CONNECTION_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let mut stream = Stream(socket);
    let identity = Identity {
        certificate: &credential.certificate,
        private_key: &credential.private_key,
        protocol: PROTOCOL,
    };
    let randomness: [u8; tls_server::RANDOMNESS_BYTES] = random_bytes();
    let mut session = tls_server::accept(&mut stream, buffers, &identity, &randomness)
        .map_err(|error| format!("handshake: {}", error.description()))?;
    let mut request: Vec<u8> = Vec::new();
    let mut piece = [0u8; 2048];
    while !request.windows(4).any(|end| end == b"\r\n\r\n") {
        match session.read(&mut stream, &mut piece) {
            Ok(0) => return Err("the client closed before its request".into()),
            Ok(count) => request.extend_from_slice(&piece[..count]),
            Err(error) => return Err(format!("reading: {}", error.description())),
        }
    }
    session
        .write(&mut stream, &http_answer(&request))
        .map_err(|error| format!("answering: {}", error.description()))?;
    session
        .close(&mut stream)
        .map_err(|error| format!("closing: {}", error.description()))
}

fn roots_folder(arch: &Arch, roots: &[u8]) -> Result<String, String> {
    let folder = crate::target_directory().join(format!("https-folder-{}", arch.name));
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let file = folder.join(ROOTS_FILE);
    std::fs::write(&file, roots).map_err(|e| format!("{}: {e}", file.display()))?;
    crate::host_folder(&folder.to_string_lossy())
}

fn commands(ports: &Ports) -> Vec<String> {
    let Ports { trusted, untrusted } = *ports;
    vec![
        format!(
            "fetch -o {SAVED_TO} https://{HOST_TEXT}:{trusted}{SERVED_PATH}; \
             echo EGGOS-HTTPS fetch $?\r"
        ),
        format!(
            "wc -c < {SAVED_TO}; wc -l < {SAVED_TO}; grep '{FIRST_LINE}' {SAVED_TO}; \
             grep '{LAST_LINE}' {SAVED_TO}; echo EGGOS-HTTPS content $?\r"
        ),
        format!(
            "fetch -o {REFUSED_TO} https://{HOST_TEXT}:{untrusted}{SERVED_PATH}; \
             echo EGGOS-HTTPS untrusted $?\r"
        ),
        "download list --json; echo EGGOS-HTTPS list $?\r".into(),
    ]
}

fn wanted_lines(ports: &Ports) -> Vec<String> {
    let body = served_body();
    vec![
        format!("Saved {} bytes to {SAVED_TO} (HTTP 200 OK)", body.len()),
        "EGGOS-HTTPS fetch 0".into(),
        body.len().to_string(),
        body.lines().count().to_string(),
        FIRST_LINE.into(),
        LAST_LINE.into(),
        "EGGOS-HTTPS content 0".into(),
        format!(
            "fetch: {HOST_TEXT}:{}: {}",
            ports.untrusted,
            CertificateError::UntrustedIssuer.description()
        ),
        "EGGOS-HTTPS untrusted 1".into(),
        "EGGOS-HTTPS list 0".into(),
        POWER_DOWN_LINE.into(),
    ]
}

fn wanted_records() -> [Vec<String>; 2] {
    [
        vec![
            format!("\"path\":\"{SAVED_TO}\""),
            "\"state\":\"done\"".into(),
            format!("\"received_bytes\":{}", served_body().len()),
        ],
        vec![
            format!("\"path\":\"{REFUSED_TO}\""),
            "\"state\":\"failed\"".into(),
        ],
    ]
}

fn verdict(arch: &str, console: &str, ports: &Ports, clean_exit: bool) -> Result<(), String> {
    for wanted in wanted_lines(ports) {
        if !has_line(console, &wanted) {
            return Err(format!(
                "{arch}: the HTTPS boot test's console has no `{wanted}` line"
            ));
        }
    }
    for parts in wanted_records() {
        let listed = console
            .lines()
            .any(|line| parts.iter().all(|part| line.contains(part.as_str())));
        if !listed {
            return Err(format!(
                "{arch}: `download list --json` shows no record with {}",
                parts.join(", ")
            ));
        }
    }
    if !clean_exit {
        return Err(format!("{arch}: QEMU did not exit cleanly after `poweroff`"));
    }
    Ok(())
}

fn type_line(boot: &mut Boot<'_>, line: &[u8]) -> Result<(), String> {
    let typed = boot
        .keyboard
        .write_all(line)
        .and_then(|()| boot.keyboard.flush());
    typed.map_err(|e| {
        let message = format!(
            "{}: could not type on the console: {e}",
            boot.arch.name
        );
        boot.fail(message)
    })
}

fn wait_for_lease(boot: &mut Boot<'_>, prompts: usize) -> Result<usize, String> {
    let mut prompts = prompts;
    for _ in 0..LEASE_CHECKS {
        type_line(boot, LEASE_COMMAND)?;
        prompts += 1;
        boot.wait_for_prompts(prompts)?;
        if String::from_utf8_lossy(&boot.console).contains(LEASED_ADDRESS) {
            return Ok(prompts);
        }
    }
    let message = format!(
        "{}: the HTTPS boot test never saw the DHCP lease",
        boot.arch.name
    );
    Err(boot.fail(message))
}

fn boot_session(arch: &Arch, kernel: &Path, folder: &str, ports: &Ports) -> Result<(), String> {
    let mut args = network_args(arch);
    args.extend(host_folder_args(arch, folder));
    let mut boot = Boot::start(arch, kernel, &args)?.with_timeout(HTTPS_TIMEOUT);
    boot.wait_for_prompts(1)?;
    let mut prompts = wait_for_lease(&mut boot, 1)?;
    for line in commands(ports) {
        type_line(&mut boot, line.as_bytes())?;
        prompts += 1;
        boot.wait_for_prompts(prompts)?;
    }
    type_line(&mut boot, POWER_OFF)?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, ports, clean_exit)
}

/// Runs the HTTPS boot test (see the module documentation) on `kernel`, the release kernel
/// for `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    eprintln!("xtask: booting {} in {} (HTTPS)", arch.name, arch.qemu);
    let now = unix_now()?;
    let trusted = credential(now)?;
    let untrusted = credential(now)?;
    let folder = roots_folder(arch, &pem(&trusted.certificate)?)?;
    let mut stand_ins = StandIns::default();
    let ports = Ports {
        trusted: stand_ins.serve(trusted)?,
        untrusted: stand_ins.serve(untrusted)?,
    };
    let result = boot_session(arch, kernel, &folder, &ports);
    drop(stand_ins);
    result?;
    eprintln!("xtask: {}: HTTPS boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_tls::chain::{NoRoots, TrustAnchors};
    use eggos_tls::client::{self, Settings};
    use eggos_tls::pem::Certificates;

    struct Roots(Vec<u8>);

    impl TrustAnchors for Roots {
        fn each(&mut self, visit: &mut dyn FnMut(&[u8]) -> bool) -> Result<bool, NoRoots> {
            let mut der = vec![0u8; 8192];
            let mut decoder = Certificates::new(&mut der);
            let found = decoder.feed(&self.0, visit) || decoder.finish(visit);
            Ok(found)
        }
    }

    fn get(port: u16, roots: &[u8], now: i64) -> Result<Vec<u8>, client::Error> {
        let socket = TcpStream::connect(("127.0.0.1", port)).expect("the stand-in listens");
        socket
            .set_read_timeout(Some(CONNECTION_TIMEOUT))
            .expect("a read timeout");
        let mut stream = Stream(socket);
        let mut buffers = Box::new(Buffers::new());
        let settings = Settings {
            host: HOST_TEXT.as_bytes(),
            now,
        };
        let randomness: [u8; client::RANDOMNESS_BYTES] = random_bytes();
        let mut session = client::connect(
            &mut stream,
            &mut buffers,
            &settings,
            &randomness,
            &mut Roots(roots.to_vec()),
        )?;
        let request = format!("GET {SERVED_PATH} HTTP/1.1\r\nHost: {HOST_TEXT}\r\n\r\n");
        session.write(&mut stream, request.as_bytes())?;
        let mut received = Vec::new();
        let mut piece = [0u8; 4096];
        loop {
            match session.read(&mut stream, &mut piece)? {
                0 => return Ok(received),
                count => received.extend_from_slice(&piece[..count]),
            }
        }
    }

    #[test]
    fn the_stand_ins_serve_a_client_that_trusts_the_root_and_are_refused_by_others() {
        let now = unix_now().unwrap();
        let trusted = credential(now).unwrap();
        let untrusted = credential(now).unwrap();
        let roots = pem(&trusted.certificate).unwrap();
        let mut stand_ins = StandIns::default();
        let trusted_port = stand_ins.serve(trusted).unwrap();
        let untrusted_port = stand_ins.serve(untrusted).unwrap();
        let request = format!("GET {SERVED_PATH} HTTP/1.1\r\n");
        let wanted = http_answer(request.as_bytes());
        assert_eq!(get(trusted_port, &roots, now), Ok(wanted));
        assert_eq!(
            get(untrusted_port, &roots, now),
            Err(client::Error::Certificate(CertificateError::UntrustedIssuer))
        );
        assert_eq!(
            get(trusted_port, &roots, now + 2 * VALIDITY_MARGIN_SECONDS),
            Err(client::Error::Certificate(CertificateError::Expired))
        );
    }

    #[test]
    fn the_served_file_takes_several_records_and_is_framed_by_its_length() {
        let body = served_body();
        assert!(body.len() > 3 * client::MAX_PLAINTEXT);
        assert!(body.starts_with(&format!("{FIRST_LINE}\n")));
        assert!(body.ends_with(&format!("\n{LAST_LINE}\n")));
        assert_eq!(body.lines().count(), SERVED_LINES + 1);
        let request = format!("GET {SERVED_PATH} HTTP/1.1\r\nHost: x\r\n\r\n");
        let answer = http_answer(request.as_bytes());
        let end = eggos_net::http::head_end(&answer).unwrap();
        let head = eggos_net::http::parse_head(&answer[..end]).unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(
            head.framing,
            eggos_net::http::Framing::Length(body.len() as u64)
        );
        assert!(http_answer(b"GET /other HTTP/1.1\r\n\r\n").starts_with(b"HTTP/1.1 404"));
    }

    #[test]
    fn the_session_fetches_from_both_ports_and_the_verdict_wants_every_line() {
        let ports = Ports {
            trusted: 40001,
            untrusted: 40002,
        };
        let typed = commands(&ports);
        assert!(typed[0].contains("https://10.0.2.2:40001/eggos-https.txt;"));
        assert!(typed[2].contains("https://10.0.2.2:40002/eggos-https.txt;"));
        let mut lines = wanted_lines(&ports);
        lines.push(format!(
            r#"{{"id":31,"path":"{REFUSED_TO}","state":"failed","reason":"untrusted"}}"#
        ));
        lines.push(format!(
            r#"{{"id":30,"path":"{SAVED_TO}","state":"done","received_bytes":{}}}"#,
            served_body().len()
        ));
        let console = format!("{}\r\n", lines.join("\r\n"));
        assert_eq!(verdict("x86_64", &console, &ports, true), Ok(()));
        assert!(verdict("x86_64", &console, &ports, false).is_err());
        let refused = console.replace("not issued by a trusted root", "for another host");
        assert!(verdict("x86_64", &refused, &ports, true).is_err());
        let unfinished = console.replace(r#""state":"done""#, r#""state":"downloading""#);
        assert!(verdict("x86_64", &unfinished, &ports, true).is_err());
        assert!(console.contains(
            "fetch: 10.0.2.2:40002: the server's certificate is not issued by a trusted root"
        ));
    }
}
