//! Winsock's data (D178), for wsock32: the `WSAE*` error each Linux error number becomes,
//! `inet_addr`'s forms of an IPv4 address (one to four parts, each decimal, octal with a
//! leading `0` or hexadecimal with `0x`), the dotted form `inet_ntoa` writes, names and
//! addresses in a hosts file, and the well-known services `getservbyname` and `getservbyport`
//! know, from IANA's registry.

/// Winsock's error codes.
pub mod code {
    pub const WSAEINTR: u32 = 10004;
    pub const WSAEACCES: u32 = 10013;
    pub const WSAEFAULT: u32 = 10014;
    pub const WSAEINVAL: u32 = 10022;
    pub const WSAEMFILE: u32 = 10024;
    pub const WSAEWOULDBLOCK: u32 = 10035;
    pub const WSAEINPROGRESS: u32 = 10036;
    pub const WSAEALREADY: u32 = 10037;
    pub const WSAENOTSOCK: u32 = 10038;
    pub const WSAEDESTADDRREQ: u32 = 10039;
    pub const WSAEMSGSIZE: u32 = 10040;
    pub const WSAEPROTOTYPE: u32 = 10041;
    pub const WSAENOPROTOOPT: u32 = 10042;
    pub const WSAEPROTONOSUPPORT: u32 = 10043;
    pub const WSAESOCKTNOSUPPORT: u32 = 10044;
    pub const WSAEOPNOTSUPP: u32 = 10045;
    pub const WSAEAFNOSUPPORT: u32 = 10047;
    pub const WSAEADDRINUSE: u32 = 10048;
    pub const WSAEADDRNOTAVAIL: u32 = 10049;
    pub const WSAENETDOWN: u32 = 10050;
    pub const WSAENETUNREACH: u32 = 10051;
    pub const WSAENETRESET: u32 = 10052;
    pub const WSAECONNABORTED: u32 = 10053;
    pub const WSAECONNRESET: u32 = 10054;
    pub const WSAENOBUFS: u32 = 10055;
    pub const WSAEISCONN: u32 = 10056;
    pub const WSAENOTCONN: u32 = 10057;
    pub const WSAESHUTDOWN: u32 = 10058;
    pub const WSAETIMEDOUT: u32 = 10060;
    pub const WSAECONNREFUSED: u32 = 10061;
    pub const WSAEHOSTDOWN: u32 = 10064;
    pub const WSAEHOSTUNREACH: u32 = 10065;
    pub const WSAVERNOTSUPPORTED: u32 = 10092;
    pub const WSANOTINITIALISED: u32 = 10093;
    pub const WSAHOST_NOT_FOUND: u32 = 11001;
    pub const WSATRY_AGAIN: u32 = 11002;
    pub const WSANO_DATA: u32 = 11004;
}

/// The Winsock error for Linux error number `errno`.
pub fn error_for(errno: u16) -> u32 {
    use code::*;
    match errno {
        4 => WSAEINTR,
        9 => WSAENOTSOCK,
        11 => WSAEWOULDBLOCK,
        12 | 105 => WSAENOBUFS,
        1 | 13 => WSAEACCES,
        14 => WSAEFAULT,
        23 | 24 => WSAEMFILE,
        32 | 108 => WSAESHUTDOWN,
        88 => WSAENOTSOCK,
        89 => WSAEDESTADDRREQ,
        90 => WSAEMSGSIZE,
        91 => WSAEPROTOTYPE,
        92 => WSAENOPROTOOPT,
        93 => WSAEPROTONOSUPPORT,
        94 => WSAESOCKTNOSUPPORT,
        95 => WSAEOPNOTSUPP,
        96 | 97 => WSAEAFNOSUPPORT,
        98 => WSAEADDRINUSE,
        99 => WSAEADDRNOTAVAIL,
        100 => WSAENETDOWN,
        101 => WSAENETUNREACH,
        102 => WSAENETRESET,
        103 => WSAECONNABORTED,
        104 => WSAECONNRESET,
        106 => WSAEISCONN,
        107 => WSAENOTCONN,
        110 => WSAETIMEDOUT,
        111 => WSAECONNREFUSED,
        112 => WSAEHOSTDOWN,
        113 => WSAEHOSTUNREACH,
        114 => WSAEALREADY,
        115 => WSAEINPROGRESS,
        _ => WSAEINVAL,
    }
}

fn part(text: &[u8]) -> Option<u32> {
    let (digits, radix) = match text {
        [b'0', b'x' | b'X', rest @ ..] => (rest, 16),
        [b'0', rest @ ..] if !rest.is_empty() => (rest, 8),
        _ => (text, 10),
    };
    if digits.is_empty() && radix != 16 {
        return None;
    }
    digits.iter().try_fold(0u32, |value, byte| {
        let digit = char::from(*byte).to_digit(radix)?;
        value.checked_mul(radix)?.checked_add(digit)
    })
}

/// The address `text` spells in one of `inet_addr`'s forms, its bytes in network order.
pub fn parse_address(text: &[u8]) -> Option<[u8; 4]> {
    let mut parts = [0u32; 4];
    let mut count = 0;
    for piece in text.split(|byte| *byte == b'.') {
        if count == 4 {
            return None;
        }
        parts[count] = part(piece)?;
        count += 1;
    }
    let value = match count {
        1 => parts[0],
        2 if parts[0] <= 0xff && parts[1] <= 0xff_ffff => parts[0] << 24 | parts[1],
        3 if parts[..2].iter().all(|part| *part <= 0xff) && parts[2] <= 0xffff => {
            parts[0] << 24 | parts[1] << 16 | parts[2]
        }
        4 if parts.iter().all(|part| *part <= 0xff) => {
            parts[0] << 24 | parts[1] << 16 | parts[2] << 8 | parts[3]
        }
        _ => return None,
    };
    Some(value.to_be_bytes())
}

/// Writes `address` in dotted form (`10.0.2.15`) into `out`; its length.
pub fn write_dotted(address: [u8; 4], out: &mut [u8; 16]) -> usize {
    let mut length = 0;
    for (index, byte) in address.into_iter().enumerate() {
        if index > 0 {
            out[length] = b'.';
            length += 1;
        }
        let digits = [byte / 100, byte / 10 % 10, byte % 10];
        let skip = match byte {
            0..=9 => 2,
            10..=99 => 1,
            _ => 0,
        };
        for digit in &digits[skip..] {
            out[length] = b'0' + digit;
            length += 1;
        }
    }
    length
}

fn host_lines(hosts: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    hosts.split(|byte| *byte == b'\n').filter_map(|line| {
        let line = line.split(|byte| *byte == b'#').next().unwrap_or(line);
        let mut words = line
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|word| !word.is_empty());
        let address = words.next()?;
        let address = eggos_net::Ipv4Address::parse(address)?.0;
        Some((address, line))
    })
}

/// The address a hosts file (`/etc/hosts`) gives `name` (ignoring case), if any.
pub fn host_address(hosts: &[u8], name: &[u8]) -> Option<[u8; 4]> {
    host_lines(hosts).find_map(|(address, line)| {
        line.split(|byte| byte.is_ascii_whitespace())
            .filter(|word| !word.is_empty())
            .skip(1)
            .any(|word| word.eq_ignore_ascii_case(name))
            .then_some(address)
    })
}

/// The first name a hosts file gives `address`, if any.
pub fn host_name(hosts: &[u8], address: [u8; 4]) -> Option<&[u8]> {
    host_lines(hosts).find_map(|(found, line)| {
        if found != address {
            return None;
        }
        line.split(|byte| byte.is_ascii_whitespace())
            .filter(|word| !word.is_empty())
            .nth(1)
    })
}

/// A well-known service: its name, port and protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Service {
    pub name: &'static [u8],
    pub port: u16,
    pub protocol: &'static [u8],
}

const fn service(name: &'static [u8], port: u16, protocol: &'static [u8]) -> Service {
    Service {
        name,
        port,
        protocol,
    }
}

const SERVICES: [Service; 26] = [
    service(b"echo", 7, b"tcp"),
    service(b"echo", 7, b"udp"),
    service(b"ftp-data", 20, b"tcp"),
    service(b"ftp", 21, b"tcp"),
    service(b"ssh", 22, b"tcp"),
    service(b"telnet", 23, b"tcp"),
    service(b"smtp", 25, b"tcp"),
    service(b"domain", 53, b"tcp"),
    service(b"domain", 53, b"udp"),
    service(b"bootps", 67, b"udp"),
    service(b"bootpc", 68, b"udp"),
    service(b"tftp", 69, b"udp"),
    service(b"http", 80, b"tcp"),
    service(b"pop3", 110, b"tcp"),
    service(b"nntp", 119, b"tcp"),
    service(b"ntp", 123, b"udp"),
    service(b"imap", 143, b"tcp"),
    service(b"snmp", 161, b"udp"),
    service(b"ldap", 389, b"tcp"),
    service(b"https", 443, b"tcp"),
    service(b"submission", 587, b"tcp"),
    service(b"ldaps", 636, b"tcp"),
    service(b"imaps", 993, b"tcp"),
    service(b"pop3s", 995, b"tcp"),
    service(b"ms-sql-s", 1433, b"tcp"),
    service(b"ms-wbt-server", 3389, b"tcp"),
];

fn protocol_matches(service: &Service, protocol: Option<&[u8]>) -> bool {
    protocol.is_none_or(|protocol| service.protocol.eq_ignore_ascii_case(protocol))
}

/// The service named `name` (ignoring case) over `protocol` (any if `None`).
pub fn service_by_name(name: &[u8], protocol: Option<&[u8]>) -> Option<Service> {
    SERVICES
        .iter()
        .find(|service| {
            service.name.eq_ignore_ascii_case(name) && protocol_matches(service, protocol)
        })
        .copied()
}

/// The service on `port` over `protocol` (any if `None`).
pub fn service_by_port(port: u16, protocol: Option<&[u8]>) -> Option<Service> {
    SERVICES
        .iter()
        .find(|service| service.port == port && protocol_matches(service, protocol))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_errors_become_winsock_errors() {
        assert_eq!(error_for(111), code::WSAECONNREFUSED);
        assert_eq!(error_for(11), code::WSAEWOULDBLOCK);
        assert_eq!(error_for(115), code::WSAEINPROGRESS);
        assert_eq!(error_for(104), code::WSAECONNRESET);
        assert_eq!(error_for(9), code::WSAENOTSOCK);
        assert_eq!(error_for(4095), code::WSAEINVAL);
    }

    #[test]
    fn addresses_parse_in_every_form_inet_addr_takes() {
        assert_eq!(parse_address(b"10.0.2.15"), Some([10, 0, 2, 15]));
        assert_eq!(parse_address(b"127.1"), Some([127, 0, 0, 1]));
        assert_eq!(parse_address(b"10.1.258"), Some([10, 1, 1, 2]));
        assert_eq!(parse_address(b"0x7f.0.0.01"), Some([127, 0, 0, 1]));
        assert_eq!(parse_address(b"012.0.0.1"), Some([10, 0, 0, 1]));
        assert_eq!(parse_address(b"3232235777"), Some([192, 168, 1, 1]));
        for bad in [
            &b""[..],
            b"1.2.3.4.5",
            b"256.1.1.1",
            b"1..2",
            b"a.b.c.d",
            b"08.1.1.1",
        ] {
            assert_eq!(parse_address(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn addresses_are_written_dotted() {
        let mut out = [0u8; 16];
        let length = write_dotted([192, 168, 0, 7], &mut out);
        assert_eq!(&out[..length], b"192.168.0.7");
        let length = write_dotted([255, 255, 255, 255], &mut out);
        assert_eq!(&out[..length], b"255.255.255.255");
    }

    #[test]
    fn hosts_files_name_addresses_both_ways() {
        let hosts =
            b"# comment\n127.0.0.1 localhost loopback\n10.0.2.2   gateway # router\nbad line\n";
        assert_eq!(host_address(hosts, b"LOCALHOST"), Some([127, 0, 0, 1]));
        assert_eq!(host_address(hosts, b"loopback"), Some([127, 0, 0, 1]));
        assert_eq!(host_address(hosts, b"gateway"), Some([10, 0, 2, 2]));
        assert_eq!(host_address(hosts, b"router"), None);
        assert_eq!(host_name(hosts, [10, 0, 2, 2]), Some(&b"gateway"[..]));
        assert_eq!(host_name(hosts, [1, 2, 3, 4]), None);
    }

    #[test]
    fn well_known_services_are_found_by_name_and_port() {
        assert_eq!(
            service_by_name(b"HTTP", Some(b"tcp")).map(|s| s.port),
            Some(80)
        );
        assert_eq!(service_by_name(b"http", Some(b"udp")), None);
        assert_eq!(
            service_by_name(b"domain", Some(b"udp")).map(|s| s.protocol),
            Some(&b"udp"[..])
        );
        assert_eq!(
            service_by_port(443, None).map(|s| s.name),
            Some(&b"https"[..])
        );
        assert_eq!(service_by_port(1, None), None);
    }
}
