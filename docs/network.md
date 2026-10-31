# The network

EggOS's network: IPv4 over one wired adapter, with an address from DHCP, UDP and ping for
programs, and DNS through `/etc/resolv.conf` (part 1, D157); TCP for programs and downloads over
HTTP with `fetch` (part 2, D158); HTTPS and the network clock (part 3, D159).

## Getting on the network

The run scripts and `cargo xtask run` attach QEMU's user networking behind a virtio network
device: QEMU is the machine's router, DHCP server and DNS forwarder, and EggOS reaches the
host and, through it, the internet (QEMU's NAT, so nothing reaches EggOS from outside).

| Address | What it is |
|---------|------------|
| 10.0.2.15 | EggOS (given by DHCP) |
| 10.0.2.2 | the host, and the router; it answers `ping`, and its UDP and TCP ports are the host's `127.0.0.1` ports |
| 10.0.2.3 | the DNS server, which asks the host's resolver |

`--no-network` (`-NoNetwork` in PowerShell, `cargo xtask run --no-network`) starts EggOS
without an adapter: only `lo` exists, and nothing network-related runs.

At boot the console says `net: eth0 52:54:00:12:34:56: asking for an address by DHCP`; the
address usually comes within milliseconds. `ip addr` shows it.

## What works

- **Adapters.** virtio-net (D157), polled, the first one only. The interface is `eth0`; `lo`
  (127.0.0.1/8) always exists, and packets to `127.0.0.0/8` or to the machine's own address
  never leave it.
- **DHCP.** The kernel asks for an address at boot and keeps it: it renews the lease halfway
  through with the server that gave it, asks any server once seven eighths are gone, starts
  again when the lease ends or is refused, and when the cable comes back. With no answer it
  tries 4 times over about a minute, rests a minute, then starts another round.
- **Names.** `/etc/resolv.conf` lists the lease's DNS servers (the kernel serves the file, so
  it follows the lease); `/etc/hosts` names `localhost`. Linux programs resolve names through
  musl as on Linux; `ping` and `nslookup` do it themselves.
- **Programs.** `AF_INET` datagram sockets: UDP, and ping sockets (`SOCK_DGRAM` with
  `IPPROTO_ICMP`, Linux's unprivileged ping), with `bind`, `connect`, `sendto`, `recvfrom`,
  `sendmsg`, `recvmsg`, `poll`, timeouts, `SO_BROADCAST`, `SO_RCVBUF`, `IP_TTL` and Linux's
  errors ([linux-programs.md](linux-programs.md)). A datagram must fit the link's MTU (1472
  bytes of UDP data on Ethernet): EggOS neither fragments nor reassembles yet.
- **TCP** (D158). `AF_INET` stream sockets: `connect` (blocking, or non-blocking with
  `EINPROGRESS` and `poll`), `listen`, `accept`/`accept4`, `send`/`recv` and their message forms,
  `read`/`write`, `shutdown`, `poll`, `SO_ERROR`, `TCP_NODELAY` (always on), timeouts and
  `FIONREAD`, with Linux's errors (`ECONNREFUSED`, `ECONNRESET`, `ETIMEDOUT`, `EPIPE`). The
  connections are EggOS's own (`eggos_net::tcp`): the three-way handshake, the segment size
  option (1460 bytes on Ethernet), sending within the peer's window, retransmission on a timer
  that follows the measured round trip (RFC 6298: from 1 s, at least 200 ms, doubling on each
  retry, giving up after 6 SYNs or 15 retries), fast retransmission after three duplicate
  acknowledgments, probing a closed window, FIN both ways, `TIME-WAIT` (60 s), and resets sent
  and honoured (out-of-sequence ones answered with a challenge acknowledgment). Each connection
  keeps up to 64 KiB to send and 64 KiB received (the largest window without window scaling),
  within the kernel's reserve for programs (D155). Closing a socket with bytes it never read
  resets the connection, as Linux does.
- **Downloads.** `fetch <url> [-o file]` gets an `http://` URL (`GET` over HTTP/1.1, the body by
  its length, in chunks or until the server closes, up to 5 redirects, an error for a 4xx or
  5xx status) to a file or standard output ([coreutils.md](coreutils.md)).

## Seeing it: one capability, three front-ends

| | |
|-|-|
| CLI | `ip addr`, `ip link -s`, `ip route`, `ip neigh` (Linux's shapes, plus the DHCP state); `ping`; `nslookup`; `fetch` ([coreutils.md](coreutils.md)) |
| MCP | `net_status`, the JSON `ip -j addr` writes; `net_ping` (`ping`); `net_lookup` (`nslookup`); `fetch`, with the `write` grant ([mcp.md](mcp.md)) |
| GUI | Settings' `Network` pane and the bar's indicator: designed ([ux/network-v0.3.md](ux/network-v0.3.md)), not built yet; downloads have no window yet |

Every front-end names the state with the same words: `connected`, `getting-address`,
`no-address`, `cable-unplugged`, or `none` when there is no adapter.

What the kernel reports, all read-only:

- `/proc/net/interfaces`: EggOS's own, one line per interface, fields separated by spaces and
  `-` for none: name, link (`up`/`down`), hardware address, MTU, DHCP state (`asking`,
  `configured`, `no-address`, `off`), address/prefix, router, DNS servers (separated by
  commas), DHCP server, lease seconds left and length (`forever` for a lease that never
  ends), then packets, bytes, errors and drops received, and packets, bytes and drops sent.
- `/proc/net/route` and `/proc/net/arp`: Linux's layouts.

## How it is built

- `drivers/src/virtio/net/`: the driver (16 receive and 16 transmit buffers of 2 KiB).
- `net/` (`eggos-net`): the protocols, without allocation, clock or device, shared by the
  kernel, the utilities and the tests: Ethernet, ARP, IPv4, ICMP echo, UDP, the DHCP client,
  DNS messages, one interface's stack and the status text.
- `kernel/src/net.rs`: the adapter, its interface and the thread that polls it (every
  millisecond while frames move, every 20 ms otherwise); `net/endpoints.rs` the UDP ports and
  ping identifiers and their queues (212992 bytes each by default, within the kernel's
  reserve for programs, D155); `net/resolv_conf.rs` the served `/etc/resolv.conf`.
- `net/` also holds TCP (`tcp.rs`: segments and one connection's state machine over its
  caller's buffers) and HTTP/1.1 (`http.rs`: URLs, the request, the response's head and a body
  decoded as it comes).
- `kernel/src/net/tcp.rs`: the connections the kernel runs, their bytes, the listeners, the
  resets for segments no one owns, and the outbox through which segments to this machine are
  delivered in order; it runs after every socket call and from the poll thread.
- `kernel/src/linux/socket/inet.rs`: the Linux system calls on `AF_INET` datagram sockets;
  `socket/tcp.rs` those on stream sockets.

Checks: host tests for each protocol, the DHCP client against a stand-in server, the kernel's
loopback and every socket call; on every architecture, a QEMU boot (`xtask/src/qemu/network.rs`)
gets its lease, pings the host, looks a name up against a DNS stand-in on the host, and runs
`linux-udp` (`std::net` on musl) over `lo` and with a UDP echo server on the host; `fetch`
downloads a file from an HTTP stand-in on the host, and `linux-tcp` sends 300000 bytes each way
over `lo`, sees a closed port refused and talks to a TCP echo server on the host. The MCP boot
calls `net_ping`, `net_lookup` and `fetch`.

## HTTPS and the network clock (D159)

- `fetch https://...` (and MCP `fetch`) speaks TLS 1.3 with a client of our own (`tls/`, on
  `crypto/`, both written from the RFCs and NIST specifications): X25519 key exchange,
  ChaCha20-Poly1305, AES-128-GCM or AES-256-GCM records, and the server's certificates
  checked: the host named in them, valid now, a path through the certificate authorities the
  server sends to a trusted root, ECDSA (P-256, P-384) and RSA (PKCS #1 v1.5, PSS; 2048 to
  4096 bits) signatures.
- **The trusted roots are yours to give.** EggOS ships no root certificates (D159). `fetch`
  reads PEM certificates from `/etc/ssl/roots.pem`, or else from `roots.pem` in the host folder
  (`/media/host/roots.pem`). Copy your host's list there: on macOS `/etc/ssl/cert.pem`, on
  Linux `/etc/ssl/certs/ca-certificates.crt`; on Windows export the trusted roots as Base-64
  certificates into one file. Without roots, `https://` fails and says so; nothing is
  downloaded unchecked.
- `date --sync[=host[:port]]` sets the clock (and the hardware clock) from an NTP server by
  SNTP (RFC 4330), `pool.ntp.org` by default, corrected by half the round trip, and says how far
  the clock moved; MCP `clock_sync` is its twin (grant `run`). Certificates need a right clock:
  a wrong one makes them look expired, and the error says to sync.

## Not yet

IPv6; fragments; static addresses, a second adapter and their routes;
multicast; reverse lookups; netlink (iproute2 itself and `getifaddrs`); the GUI front-end
(Settings' Network pane, the bar's indicator, a window for downloads); TCP window scaling,
selective acknowledgments, keep-alive, urgent data and Nagle's delay (`TCP_NODELAY` is always
on), and keeping segments that arrive out of order (they are dropped and sent again); TCP
timers on a machine without an adapter run only when a program makes a socket call; interrupts
instead of polling; the Date & time pane's "Set automatically" and syncing at boot; TLS
session resumption, client certificates, a second key exchange (X25519 is the only group
offered, so a server that wants another fails), revocation (OCSP, CRLs) and name constraints
(a certificate that makes them critical is refused). See D157 to D159.
