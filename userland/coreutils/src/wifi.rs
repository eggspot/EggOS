//! `wifi list` (D280): the Wi-Fi adapter's state and the networks its last scan heard, from
//! `/proc/driver/wifi`; `wifi connect <ssid> [<passphrase>]` names the network to join in
//! `/var/lib/eggos/wifi.conf`, readable by root alone, which the kernel's Wi-Fi driver reads
//! after its next scan; `wifi forget` removes it.

use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::{commands, errors};

const STATUS: &CStr = c"/proc/driver/wifi";
const SETTINGS: &CStr = c"/var/lib/eggos/wifi.conf";
const SETTINGS_MODE: u32 = 0o600;
const PIECE: usize = 4096;
const SMALLEST_PASSPHRASE: usize = 8;
const LARGEST_PASSPHRASE: usize = 63;
const LARGEST_SSID: usize = 32;
const USAGE: &[u8] = b"usage: wifi list | wifi connect <ssid> [<passphrase>] | wifi forget";

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
    if !passphrase.is_empty() && !(SMALLEST_PASSPHRASE..=LARGEST_PASSPHRASE).contains(&passphrase.len())
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
    let written = [&b"ssid="[..], ssid, b"\npassphrase=", passphrase, b"\n", hidden_line]
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
    let Ok(operands) = parse_defined(output, rest, definition, |_, given| {
        hidden |= given.option.long == Some("hidden");
        Ok(())
    }) else {
        return FAILURE;
    };
    match (verb.to_bytes(), operands.all()) {
        (b"list", []) => show(output),
        (b"forget", []) => forget(output),
        (b"connect", [ssid]) => connect(output, ssid.to_bytes(), b"", hidden),
        (b"connect", [ssid, passphrase]) => connect(output, ssid.to_bytes(), passphrase.to_bytes(), hidden),
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
            .with_file("/proc/driver/wifi", 0o444, b"ready; address 00:11:22:33:44:55; 37 channels\n", 0)
            .with_directory("/var", 0)
            .with_directory("/var/lib", 0)
            .with_directory("/var/lib/eggos", 0)
    }

    #[test]
    fn wifi_shows_the_driver_s_state_and_connect_names_the_network() {
        let (status, out, _, _) = run(machine(), &[c"list"]);
        assert_eq!((status, out.as_str()), (0, "ready; address 00:11:22:33:44:55; 37 channels\n"));
        let (status, _, errors, fake) = run(machine(), &[c"connect", c"home", c"correct horse"]);
        assert_eq!((status, errors.as_str()), (0, ""));
        assert_eq!(fake.contents("/var/lib/eggos/wifi.conf"), b"ssid=home\npassphrase=correct horse\n");
        let (status, _, errors, _) = run(machine(), &[c"connect", c"home", c"short"]);
        assert_eq!(status, FAILURE);
        assert!(errors.contains("8 to 63"));
        let (status, _, _, _) = run(machine(), &[c"dance"]);
        assert_eq!(status, FAILURE);
    }
}
