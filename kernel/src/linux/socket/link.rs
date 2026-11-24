//! A network interface's link settings through a socket's `ioctl`s, as Linux's `ifconfig`,
//! `ip` and `ethtool` use them (D708): `SIOCGIFMTU` and `SIOCSIFMTU` read and set its MTU,
//! `SIOCETHTOOL` with `ETHTOOL_GWOL` and `ETHTOOL_SWOL` its Wake-on-LAN. Each takes a
//! `struct ifreq`: the interface's name in 16 bytes, then the MTU as an `int`, or a pointer to
//! the `struct ethtool_wolinfo`. Setting needs a privileged process (`EPERM`).

use crate::linux::Errno;
use crate::linux::time::is_privileged;
use crate::net::{self, LinkError};
use crate::process::Process;
use eggos_hal::Platform;

/// The requests answered, the same number on every architecture (`linux/sockios.h`).
pub mod request {
    pub const SIOCGIFMTU: u32 = 0x8921;
    pub const SIOCSIFMTU: u32 = 0x8922;
    pub const SIOCETHTOOL: u32 = 0x8946;
}

/// `struct ethtool_wolinfo`'s commands and its only wake kind EggOS has.
mod ethtool {
    pub const GET_WAKE: u32 = 5;
    pub const SET_WAKE: u32 = 6;
    pub const WAKE_MAGIC: u32 = 1 << 5;
}

const NAME_BYTES: usize = 16;
const IFREQ_BYTES: usize = NAME_BYTES + 8;
const WOLINFO_BYTES: usize = 20;

fn interface_name(ifreq: &[u8; IFREQ_BYTES]) -> &[u8] {
    let name = &ifreq[..NAME_BYTES];
    let end = name.iter().position(|&byte| byte == 0).unwrap_or(NAME_BYTES);
    &name[..end]
}

fn refused(error: LinkError) -> Errno {
    match error {
        LinkError::NoSuchInterface => Errno::ENODEV,
        LinkError::Refused(_) => Errno::EINVAL,
    }
}

/// The interface `ioctl`s on a socket, `None` for another request.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    request: u64,
    argument: u64,
) -> Option<Result<u64, Errno>> {
    let request = request as u32;
    if !matches!(
        request,
        request::SIOCGIFMTU | request::SIOCSIFMTU | request::SIOCETHTOOL
    ) {
        return None;
    }
    Some(answer(process, request, argument))
}

fn answer<P: Platform>(process: &Process<P>, request: u32, argument: u64) -> Result<u64, Errno> {
    let mut ifreq = [0u8; IFREQ_BYTES];
    process
        .read_user_memory(argument, &mut ifreq)
        .map_err(|_| Errno::EFAULT)?;
    let name = interface_name(&ifreq);
    let status = net::interfaces::<P>()
        .into_iter()
        .find(|status| status.name.as_bytes() == name)
        .ok_or(Errno::ENODEV)?;
    match request {
        request::SIOCGIFMTU => {
            let mtu = status.mtu as i32;
            process
                .write_user_memory(argument + NAME_BYTES as u64, &mtu.to_ne_bytes())
                .map_err(|_| Errno::EFAULT)?;
            Ok(0)
        }
        request::SIOCSIFMTU => {
            if !is_privileged(process) {
                return Err(Errno::EPERM);
            }
            let mtu = i32::from_ne_bytes(ifreq[NAME_BYTES..NAME_BYTES + 4].try_into().unwrap());
            let mtu = usize::try_from(mtu).map_err(|_| Errno::EINVAL)?;
            net::set_mtu(name, mtu).map_err(refused)?;
            Ok(0)
        }
        _ => {
            let field = &ifreq[NAME_BYTES..];
            let pointer = match crate::linux::abi::WORD_BYTES {
                8 => u64::from_ne_bytes(field[..8].try_into().unwrap()),
                _ => u64::from(u32::from_ne_bytes(field[..4].try_into().unwrap())),
            };
            let mut info = [0u8; WOLINFO_BYTES];
            process
                .read_user_memory(pointer, &mut info)
                .map_err(|_| Errno::EFAULT)?;
            let command = u32::from_ne_bytes(info[..4].try_into().unwrap());
            match command {
                ethtool::GET_WAKE => {
                    let supported = match status.wake_on_lan {
                        Some(_) => ethtool::WAKE_MAGIC,
                        None => 0,
                    };
                    let on = match status.wake_on_lan {
                        Some(true) => ethtool::WAKE_MAGIC,
                        _ => 0,
                    };
                    info[4..8].copy_from_slice(&supported.to_ne_bytes());
                    info[8..12].copy_from_slice(&on.to_ne_bytes());
                    process
                        .write_user_memory(pointer, &info)
                        .map_err(|_| Errno::EFAULT)?;
                    Ok(0)
                }
                ethtool::SET_WAKE => {
                    if !is_privileged(process) {
                        return Err(Errno::EPERM);
                    }
                    let wanted = u32::from_ne_bytes(info[8..12].try_into().unwrap());
                    if wanted & !ethtool::WAKE_MAGIC != 0 || status.wake_on_lan.is_none() {
                        return Err(Errno::EOPNOTSUPP);
                    }
                    net::set_wake_on_lan(name, wanted == ethtool::WAKE_MAGIC).map_err(refused)?;
                    Ok(0)
                }
                _ => Err(Errno::EOPNOTSUPP),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_ends_at_its_first_zero_or_after_sixteen_bytes() {
        let mut ifreq = [0u8; IFREQ_BYTES];
        ifreq[..4].copy_from_slice(b"eth0");
        assert_eq!(interface_name(&ifreq), b"eth0");
        ifreq[..NAME_BYTES].copy_from_slice(&[b'x'; NAME_BYTES]);
        assert_eq!(interface_name(&ifreq).len(), NAME_BYTES);
    }
}
