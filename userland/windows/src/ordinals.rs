//! Functions programs import by ordinal rather than by name (D189): Winsock 1.1's, which old
//! programs import from `wsock32` and programs linked with the SDK's `ws2_32.lib` from
//! `ws2_32`. The two libraries number them alike except `inet_addr`, `inet_ntoa` and
//! `ioctlsocket`, which `ws2_32` puts in alphabetical order. kernel32, user32, ntdll and the C
//! runtimes have no stable ordinals, and every linker imports them by name.

/// The library a Winsock ordinal is numbered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winsock {
    Wsock32,
    Ws2_32,
}

const WINSOCK_1_1: &[(u16, &[u8])] = &[
    (1, b"accept"),
    (2, b"bind"),
    (3, b"closesocket"),
    (4, b"connect"),
    (5, b"getpeername"),
    (6, b"getsockname"),
    (7, b"getsockopt"),
    (8, b"htonl"),
    (9, b"htons"),
    (10, b"inet_addr"),
    (11, b"inet_ntoa"),
    (12, b"ioctlsocket"),
    (13, b"listen"),
    (14, b"ntohl"),
    (15, b"ntohs"),
    (16, b"recv"),
    (17, b"recvfrom"),
    (18, b"select"),
    (19, b"send"),
    (20, b"sendto"),
    (21, b"setsockopt"),
    (22, b"shutdown"),
    (23, b"socket"),
    (51, b"gethostbyaddr"),
    (52, b"gethostbyname"),
    (53, b"getprotobyname"),
    (54, b"getprotobynumber"),
    (55, b"getservbyname"),
    (56, b"getservbyport"),
    (57, b"gethostname"),
    (101, b"WSAAsyncSelect"),
    (111, b"WSAGetLastError"),
    (112, b"WSASetLastError"),
    (115, b"WSAStartup"),
    (116, b"WSACleanup"),
    (151, b"__WSAFDIsSet"),
];

const WS2_32_ALPHABETICAL: &[(u16, &[u8])] =
    &[(10, b"ioctlsocket"), (11, b"inet_addr"), (12, b"inet_ntoa")];

/// The name of the function `library` exports as `ordinal`, if Winsock 1.1 numbers one there.
pub fn winsock_function(library: Winsock, ordinal: u16) -> Option<&'static [u8]> {
    let own = match library {
        Winsock::Wsock32 => &[][..],
        Winsock::Ws2_32 => WS2_32_ALPHABETICAL,
    };
    own.iter()
        .chain(WINSOCK_1_1)
        .find(|(number, _)| *number == ordinal)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winsock_ordinals_differ_only_where_ws2_32_is_alphabetical() {
        for library in [Winsock::Wsock32, Winsock::Ws2_32] {
            assert_eq!(winsock_function(library, 3), Some(&b"closesocket"[..]));
            assert_eq!(winsock_function(library, 23), Some(&b"socket"[..]));
            assert_eq!(winsock_function(library, 115), Some(&b"WSAStartup"[..]));
            assert_eq!(winsock_function(library, 151), Some(&b"__WSAFDIsSet"[..]));
            assert_eq!(winsock_function(library, 0), None);
            assert_eq!(winsock_function(library, 24), None);
        }
        assert_eq!(
            winsock_function(Winsock::Wsock32, 10),
            Some(&b"inet_addr"[..])
        );
        assert_eq!(
            winsock_function(Winsock::Wsock32, 12),
            Some(&b"ioctlsocket"[..])
        );
        assert_eq!(
            winsock_function(Winsock::Ws2_32, 10),
            Some(&b"ioctlsocket"[..])
        );
        assert_eq!(
            winsock_function(Winsock::Ws2_32, 11),
            Some(&b"inet_addr"[..])
        );
        assert_eq!(
            winsock_function(Winsock::Ws2_32, 12),
            Some(&b"inet_ntoa"[..])
        );
    }

    #[test]
    fn every_ordinal_names_one_function() {
        for (index, (number, name)) in WINSOCK_1_1.iter().enumerate() {
            assert!(
                WINSOCK_1_1[index + 1..]
                    .iter()
                    .all(|(other, _)| other != number)
            );
            assert!(
                WINSOCK_1_1[index + 1..]
                    .iter()
                    .all(|(_, other)| other != name)
            );
        }
    }
}
