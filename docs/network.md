# The network

EggOS's network, part 1 (D157): IPv4 over one wired adapter, with an address from DHCP, UDP
and ping for programs, and DNS through `/etc/resolv.conf`. TCP and downloads come next (part
2), then HTTPS and the network clock (part 3).

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
  tries 4 times over a minute, then every minute.
- **Names.** `/etc/resolv.conf` lists the lease's DNS servers (the kernel serves the file, so
  it follows the lease); `/etc/hosts` names `localhost`. Linux programs resolve names through
  musl as on Linux; `ping` and `nslookup` do it themselves.
- **Programs.** `AF_INET` datagram sockets: UDP, and ping sockets (`SOCK_DGRAM` with
  `IPPROTO_ICMP`, Linux's unprivileged ping), with `bind`, `connect`, `sendto`, `recvfrom`,
  `sendmsg`, `recvmsg`, `poll`, timeouts, `SO_BROADCAST`, `SO_RCVBUF`, `IP_TTL` and Linux's
  errors ([linux-programs.md](linux-programs.md)). A datagram must fit the link's MTU (1472
  bytes of UDP data on Ethernet): EggOS neither fragments nor reassembles yet.

## Seeing it: one capability, three front-ends

| | |
|-|-|
| CLI | `ip addr`, `ip link -s`, `ip route`, `ip neigh` (Linux's shapes, plus the DHCP state); `ping`; `nslookup` ([coreutils.md](coreutils.md)) |
| MCP | `net_status`, the JSON `ip -j addr` writes ([mcp.md](mcp.md)) |
| GUI | Settings' `Network` pane and the bar's indicator: designed ([ux/network-v0.3.md](ux/network-v0.3.md)), not built yet |

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
- `kernel/src/linux/socket/inet.rs`: the Linux system calls on `AF_INET` sockets.

Checks: host tests for each protocol, the DHCP client against a stand-in server, the kernel's
loopback and every socket call; on every architecture, a QEMU boot (`xtask/src/qemu/network.rs`)
gets its lease, pings the host, looks a name up against a DNS stand-in on the host, and runs
`linux-udp` (`std::net` on musl) over `lo` and with a UDP echo server on the host.

## Not yet

TCP; IPv6; fragments; static addresses, a second adapter and their routes; multicast; reverse
lookups; netlink (iproute2 itself and `getifaddrs`); the GUI front-end; MCP tools for `ping` and
`nslookup` (the `run` tool reaches them); interrupts instead of polling. See D157.
