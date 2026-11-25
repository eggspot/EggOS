//! `wifi list` (D280): the Wi-Fi adapter's state and the networks its last scan heard, from
//! `/proc/driver/wifi`; `wifi connect <ssid> [<passphrase>]` names the network to join in
//! `/var/lib/eggos/wifi.conf`, readable by root alone, which the kernel's Wi-Fi driver reads
//! after its next scan; `wifi forget` removes it. `wifi connect <ssid> --eap peap|tls ...` names
//! an Enterprise network (D780): the PEM files are checked by `eggos-tls`'s parser, the one
//! Settings uses, and copied into `/var/lib/eggos/wifi/`.

use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_tls::{chain, pem, private_key};

const STATUS: &CStr = c"/proc/driver/wifi";
const SETTINGS: &CStr = c"/var/lib/eggos/wifi.conf";
const SETTINGS_MODE: u32 = 0o600;
const PIECE: usize = 4096;
const SMALLEST_PASSPHRASE: usize = 8;
const LARGEST_PASSPHRASE: usize = 63;
const LARGEST_SSID: usize = 32;
const USAGE: &[u8] = b"usage: wifi list | wifi connect <ssid> [<passphrase>] | wifi connect <ssid> \
--eap peap|tls --identity <user> [--password <pw>] [--anonymous <id>] [--cert <file> --key <file>] \
--ca <file> --domain <domain> | wifi forget";
const FILES: &CStr = c"/var/lib/eggos/wifi";
const CA_COPY: &CStr = c"/var/lib/eggos/wifi/ca.pem";
const CERTIFICATE_COPY: &CStr = c"/var/lib/eggos/wifi/client.pem";
const KEY_COPY: &CStr = c"/var/lib/eggos/wifi/key.pem";
const FILES_MODE: u32 = 0o700;
const LARGEST_PEM: usize = 64 * 1024;
const LARGEST_IDENTITY: usize = 253;
const LARGEST_PASSWORD: usize = 255;

#[derive(Debug, Default)]
struct Enterprise<'a> {
    eap: Option<&'a [u8]>,
    identity: Option<&'a [u8]>,
    password: Option<&'a [u8]>,
    anonymous: Option<&'a [u8]>,
    certificate: Option<&'a CStr>,
    key: Option<&'a CStr>,
    ca: Option<&'a CStr>,
    domain: Option<&'a [u8]>,
}

struct PemFile {
    bytes: [u8; LARGEST_PEM],
    length: usize,
}

fn read_pem<S: System>(output: &mut Output<S>, path: &CStr, file: &mut PemFile) -> bool {
    let descriptor = match output.system.open(path) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.file_error(path, errno);
            return false;
        }
    };
    file.length = 0;
    let read = loop {
        match output.system.read(descriptor, &mut file.bytes[file.length..]) {
            Ok(0) => break Ok(()),
            Ok(count) => {
                file.length += count;
                if file.length == LARGEST_PEM {
                    break Ok(());
                }
            }
            Err(errno) => break Err(errno),
        }
    };
    output.system.close(descriptor);
    if let Err(errno) = read {
        output.file_error(path, errno);
        return false;
    }
    true
}

fn copy_pem<S: System>(output: &mut Output<S>, path: &CStr, bytes: &[u8]) -> bool {
    let descriptor = match output.system.create(path, SETTINGS_MODE, true) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.file_error(path, errno);
            return false;
        }
    };
    let written = output
        .system
        .write(descriptor, bytes)
        .and_then(|()| output.system.sync_file(descriptor));
    output.system.close(descriptor);
    if let Err(errno) = written {
        output.file_error(path, errno);
        return false;
    }
    true
}

fn one_line(value: &[u8], largest: usize) -> bool {
    value.len() <= largest && !value.contains(&b'\n')
}

fn connect_enterprise<S: System>(
    output: &mut Output<S>,
    ssid: &[u8],
    given: &Enterprise<'_>,
    hidden: bool,
) -> i32 {
    let certificate_method = match given.eap {
        Some(b"peap") => false,
        Some(b"tls") => true,
        _ => {
            output.coded(&errors::USAGE, &[b"--eap is peap or tls"]);
            return FAILURE;
        }
    };
    let identity = given.identity.unwrap_or_default();
    if identity.is_empty() || !one_line(identity, LARGEST_IDENTITY) {
        output.coded(&errors::USAGE, &[b"--identity is 1 to 253 characters on one line"]);
        return FAILURE;
    }
    let password = given.password.unwrap_or_default();
    let anonymous = given.anonymous.unwrap_or_default();
    let peap_complete = !password.is_empty()
        && one_line(password, LARGEST_PASSWORD)
        && one_line(anonymous, LARGEST_IDENTITY);
    if !certificate_method && !peap_complete {
        output.coded(
            &errors::USAGE,
            &[b"peap takes --password (1 to 255 characters) and at most 253 for --anonymous"],
        );
        return FAILURE;
    }
    let (Some(ca), Some(domain)) = (given.ca, given.domain) else {
        output.coded(&errors::USAGE, &[b"an Enterprise network needs --ca and --domain"]);
        return FAILURE;
    };
    if !chain::is_host_name(domain) {
        output.coded(&errors::BAD_DOMAIN, &[b"not a domain name: ", domain]);
        return FAILURE;
    }
    let files = match certificate_method {
        true => match (given.certificate, given.key) {
            (Some(certificate), Some(key)) => Some((certificate, key)),
            _ => {
                output.coded(&errors::USAGE, &[b"tls takes --cert and --key"]);
                return FAILURE;
            }
        },
        false => None,
    };
    let mut file = PemFile {
        bytes: [0; LARGEST_PEM],
        length: 0,
    };
    match output.system.make_directory(FILES, FILES_MODE) {
        Ok(()) | Err(Errno::EEXIST) => {}
        Err(errno) => {
            output.file_error(FILES, errno);
            return FAILURE;
        }
    }
    if !read_pem(output, ca, &mut file) {
        return FAILURE;
    }
    if !pem::holds_certificate(&file.bytes[..file.length]) {
        output.coded(&errors::NOT_A_CERTIFICATE, &[ca.to_bytes()]);
        return FAILURE;
    }
    if !copy_pem(output, CA_COPY, &file.bytes[..file.length]) {
        return FAILURE;
    }
    if let Some((certificate, key)) = files {
        if !read_pem(output, certificate, &mut file) {
            return FAILURE;
        }
        if !pem::holds_certificate(&file.bytes[..file.length]) {
            output.coded(&errors::NOT_A_CERTIFICATE, &[certificate.to_bytes()]);
            return FAILURE;
        }
        if !copy_pem(output, CERTIFICATE_COPY, &file.bytes[..file.length]) {
            return FAILURE;
        }
        if !read_pem(output, key, &mut file) {
            return FAILURE;
        }
        if let Err(problem) = private_key::p256_from_pem(&file.bytes[..file.length]) {
            let error = match problem {
                private_key::KeyError::Unsupported => &errors::KEY_UNSUPPORTED,
                private_key::KeyError::Encrypted => &errors::KEY_ENCRYPTED,
                _ => &errors::NOT_A_KEY,
            };
            output.coded(error, &[key.to_bytes(), b": ", problem.description().as_bytes()]);
            return FAILURE;
        }
        let copied = copy_pem(output, KEY_COPY, &file.bytes[..file.length]);
        file.bytes.fill(0);
        if !copied {
            return FAILURE;
        }
    }
    let descriptor = match output.system.create(SETTINGS, SETTINGS_MODE, true) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.file_error(SETTINGS, errno);
            return FAILURE;
        }
    };
    let hidden_line: &[u8] = if hidden { b"hidden=yes\n" } else { b"" };
    let method_lines: [&[u8]; 6] = match certificate_method {
        false => [
            b"eap=peap\nidentity=",
            identity,
            b"\npassword=",
            password,
            b"\nanonymous=",
            anonymous,
        ],
        true => [
            b"eap=tls\nidentity=",
            identity,
            b"\ncert=",
            CERTIFICATE_COPY.to_bytes(),
            b"\nkey=",
            KEY_COPY.to_bytes(),
        ],
    };
    let written = [&b"ssid="[..], ssid, b"\nsecurity=enterprise\n"]
        .iter()
        .chain(method_lines.iter())
        .chain(
            [
                &b"\nca="[..],
                CA_COPY.to_bytes(),
                b"\ndomain=",
                domain,
                b"\n",
                hidden_line,
            ]
            .iter(),
        )
        .try_for_each(|part| output.system.write(descriptor, part))
        .and_then(|()| output.system.sync_file(descriptor));
    output.system.close(descriptor);
    if let Err(errno) = written {
        output.file_error(SETTINGS, errno);
        return FAILURE;
    }
    output.write(b"Joining ");
    output.write(ssid);
    output.write(b" after the next scan; wifi list shows when it is connected.\n");
    0
}

fn show<S: System>(output: &mut Output<S>) -> i32 {
    let descriptor = match output.system.open(STATUS) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.file_error(STATUS, errno);
            return FAILURE;
        }
    };
    let mut piece = [0u8; PIECE];
    let status = loop {
        match output.system.read(descriptor, &mut piece) {
            Ok(0) => break 0,
            Ok(read) => output.write(&piece[..read]),
            Err(errno) => {
                output.file_error(STATUS, errno);
                break FAILURE;
            }
        }
    };
    output.system.close(descriptor);
    status
}

fn connect<S: System>(output: &mut Output<S>, ssid: &[u8], passphrase: &[u8], hidden: bool) -> i32 {
    if ssid.is_empty() || ssid.len() > LARGEST_SSID || ssid.contains(&b'\n') {
        output.error(&[b"a network's name is 1 to 32 bytes on one line"]);
        return FAILURE;
    }
    if !passphrase.is_empty()
        && !(SMALLEST_PASSPHRASE..=LARGEST_PASSPHRASE).contains(&passphrase.len())
        || passphrase.contains(&b'\n')
    {
        output.error(&[b"a WPA2 passphrase is 8 to 63 characters on one line"]);
        return FAILURE;
    }
    let descriptor = match output.system.create(SETTINGS, SETTINGS_MODE, true) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.file_error(SETTINGS, errno);
            return FAILURE;
        }
    };
    let hidden_line: &[u8] = if hidden { b"hidden=yes\n" } else { b"" };
    let written = [
        &b"ssid="[..],
        ssid,
        b"\npassphrase=",
        passphrase,
        b"\n",
        hidden_line,
    ]
    .iter()
    .try_for_each(|part| output.system.write(descriptor, part))
    .and_then(|()| output.system.sync_file(descriptor));
    output.system.close(descriptor);
    if let Err(errno) = written {
        output.file_error(SETTINGS, errno);
        return FAILURE;
    }
    output.write(b"Joining ");
    output.write(ssid);
    output.write(b" after the next scan; wifi list shows when it is connected.\n");
    0
}

fn forget<S: System>(output: &mut Output<S>) -> i32 {
    for copy in [CA_COPY, CERTIFICATE_COPY, KEY_COPY] {
        let _ = output.system.remove(copy, false);
    }
    match output.system.remove(SETTINGS, false) {
        Ok(()) => {
            output.write(b"No network to join.\n");
            0
        }
        Err(errno) => {
            output.file_error(SETTINGS, errno);
            FAILURE
        }
    }
}

pub fn wifi<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some((verb, rest)) = arguments.split_first() else {
        return show(output);
    };
    let definition = match verb.to_bytes() {
        b"list" => &commands::WIFI_LIST,
        b"connect" => &commands::WIFI_CONNECT,
        b"forget" => &commands::WIFI_FORGET,
        other => {
            output.coded(&errors::USAGE, &[b"unknown verb '", other, b"': ", USAGE]);
            return FAILURE;
        }
    };
    let mut hidden = false;
    let mut enterprise = Enterprise::default();
    let Ok(operands) = parse_defined(output, rest, definition, |_, given| {
        match given.option.long {
            Some("hidden") => hidden = true,
            Some("eap") => enterprise.eap = given.value,
            Some("identity") => enterprise.identity = given.value,
            Some("password") => enterprise.password = given.value,
            Some("anonymous") => enterprise.anonymous = given.value,
            Some("cert") => enterprise.certificate = given.value_as_c_string(),
            Some("key") => enterprise.key = given.value_as_c_string(),
            Some("ca") => enterprise.ca = given.value_as_c_string(),
            Some("domain") => enterprise.domain = given.value,
            _ => {}
        }
        Ok(())
    }) else {
        return FAILURE;
    };
    match (verb.to_bytes(), operands.all()) {
        (b"list", []) => show(output),
        (b"forget", []) => forget(output),
        (b"connect", [ssid]) if enterprise.eap.is_some() => {
            connect_enterprise(output, ssid.to_bytes(), &enterprise, hidden)
        }
        (b"connect", [ssid]) => connect(output, ssid.to_bytes(), b"", hidden),
        (b"connect", [_, _]) if enterprise.eap.is_some() => {
            output.coded(
                &errors::USAGE,
                &[b"an Enterprise network takes --password, not a passphrase"],
            );
            FAILURE
        }
        (b"connect", [ssid, passphrase]) => {
            connect(output, ssid.to_bytes(), passphrase.to_bytes(), hidden)
        }
        _ => {
            output.coded(&errors::USAGE, &[USAGE]);
            FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String, Fake) {
        let mut output = Output::new(fake, b"wifi");
        let status = wifi(&mut output, arguments);
        let status = output.finish(status);
        let fake = output.system;
        let (out, errors) = (fake.output(), fake.errors());
        (status, out, errors, fake)
    }

    fn machine() -> Fake {
        Fake::default()
            .with_directory("/proc", 0)
            .with_directory("/proc/driver", 0)
            .with_file(
                "/proc/driver/wifi",
                0o444,
                b"ready; address 00:11:22:33:44:55; 37 channels\n",
                0,
            )
            .with_directory("/var", 0)
            .with_directory("/var/lib", 0)
            .with_directory("/var/lib/eggos", 0)
    }

    #[test]
    fn wifi_shows_the_driver_s_state_and_connect_names_the_network() {
        let (status, out, _, _) = run(machine(), &[c"list"]);
        assert_eq!(
            (status, out.as_str()),
            (0, "ready; address 00:11:22:33:44:55; 37 channels\n")
        );
        let (status, _, errors, fake) = run(machine(), &[c"connect", c"home", c"correct horse"]);
        assert_eq!((status, errors.as_str()), (0, ""));
        assert_eq!(
            fake.contents("/var/lib/eggos/wifi.conf"),
            b"ssid=home\npassphrase=correct horse\n"
        );
        let (status, _, errors, _) = run(machine(), &[c"connect", c"home", c"short"]);
        assert_eq!(status, FAILURE);
        assert!(errors.contains("8 to 63"));
        let (status, _, _, _) = run(machine(), &[c"dance"]);
        assert_eq!(status, FAILURE);
    }

    const CA: &[u8] = b"-----BEGIN CERTIFICATE-----\nMAA=\n-----END CERTIFICATE-----\n";

    #[test]
    fn an_enterprise_network_copies_its_ca_and_names_the_method() {
        let fake = machine()
            .with_directory("/home", 0)
            .with_file("/home/ca.pem", 0o644, CA, 0);
        let (status, _, errors, fake) = run(
            fake,
            &[
                c"connect",
                c"corp",
                c"--eap",
                c"peap",
                c"--identity",
                c"kim",
                c"--password",
                c"secret",
                c"--ca",
                c"/home/ca.pem",
                c"--domain",
                c"radius.example.com",
            ],
        );
        assert_eq!((status, errors.as_str()), (0, ""));
        assert_eq!(
            fake.contents("/var/lib/eggos/wifi.conf"),
            b"ssid=corp\nsecurity=enterprise\neap=peap\nidentity=kim\npassword=secret\n\
              anonymous=\nca=/var/lib/eggos/wifi/ca.pem\ndomain=radius.example.com\n"
        );
        assert_eq!(fake.contents("/var/lib/eggos/wifi/ca.pem"), CA);
    }

    #[test]
    fn enterprise_refusals_name_the_domain_and_the_files() {
        let fake = || {
            machine()
                .with_directory("/home", 0)
                .with_file("/home/ca.pem", 0o644, CA, 0)
                .with_file("/home/notes.txt", 0o644, b"hello", 0)
        };
        let base = [
            c"connect",
            c"corp",
            c"--eap",
            c"peap",
            c"--identity",
            c"kim",
            c"--password",
            c"secret",
        ];
        let mut bad_domain = base.to_vec();
        bad_domain.extend([c"--ca", c"/home/ca.pem", c"--domain", c"radius example"]);
        let (status, _, errors, _) = run(fake(), &bad_domain);
        assert_eq!(status, FAILURE);
        assert!(errors.contains("not a domain name"));
        let mut not_pem = base.to_vec();
        not_pem.extend([c"--ca", c"/home/notes.txt", c"--domain", c"radius.example.com"]);
        let (status, _, errors, _) = run(fake(), &not_pem);
        assert_eq!(status, FAILURE);
        assert!(errors.contains("notes.txt"));
    }
}
