//! The console as a terminal (M6, D61): `ioctl` on descriptors 0 to 2. C libraries ask
//! whether a descriptor is a terminal before they choose how to buffer it (musl's stdout and
//! `isatty` with `TIOCGWINSZ`, glibc's `isatty` and `tcgetattr` with `TCGETS`), so a program
//! that writes to the console line-buffers as it would on a Linux serial console.
//!
//! The requests answered are the ones whose answer is true of the console today:
//! - `TCGETS` and `TCGETS2`: the settings of a Linux serial console at 115200 baud
//!   ([`TERMIOS`]); output already turns `\n` into `\r\n` (`OPOST | ONLCR`) in the UART driver.
//! - `TIOCGWINSZ` and `TIOCSWINSZ`: the window size, zero until a program sets one, as on a
//!   serial line, whose size the kernel cannot know. Nobody is told of a change (`SIGWINCH`)
//!   until signals are delivered.
//! - `FIONREAD`: no bytes waiting, as there is no console input yet (M8).
//!
//! Every other request is `ENOTTY`, as Linux answers a request a terminal does not know:
//! changing the settings (`TCSETS` and its variants) waits for a line discipline with console
//! input, job control (`TIOCGPGRP`, `TIOCSPGRP`) for sessions, and the descriptor flags
//! (`FIONBIO`, `FIOCLEX`, `FIONCLEX`) for a descriptor table (M7).

use super::{Errno, is_console};
use crate::process::Process;
use crate::sync::SpinLock;
use eggos_hal::Platform;

/// The requests answered, the generic numbers, which AArch64, RISC-V and x86_64 share.
pub mod request {
    /// Read the settings (`struct termios`).
    pub const TCGETS: u32 = 0x5401;
    /// Read the window size (`struct winsize`).
    pub const TIOCGWINSZ: u32 = 0x5413;
    /// Set the window size.
    pub const TIOCSWINSZ: u32 = 0x5414;
    /// How many bytes wait to be read (an `int`).
    pub const FIONREAD: u32 = 0x541b;
    /// Read the settings with their speeds (`struct termios2`): `_IOR('T', 0x2a, 44 bytes)`.
    pub const TCGETS2: u32 = 0x802c_542a;

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    compile_error!("check Linux's terminal ioctl numbers for this architecture");
}

/// The size of the kernel's `struct termios`: four 32-bit flag words, the line discipline
/// (a byte) and 19 control characters.
const TERMIOS_SIZE: usize = 36;

/// The size of `struct termios2`: `struct termios`, then the input and output speeds.
const TERMIOS2_SIZE: usize = TERMIOS_SIZE + 8;

/// The console's speed, in baud: what QEMU's and most boards' serial consoles run at.
const BAUD: u32 = 115_200;

/// The console's settings as `TCGETS` reports them: a Linux serial console's after boot
/// (`stty -a`), at `BAUD`.
pub const TERMIOS: [u8; TERMIOS_SIZE] = {
    // Input: carriage return read as newline; ^S and ^Q stop and start output.
    const ICRNL: u32 = 0o400;
    const IXON: u32 = 0o2000;
    // Output: processed, newline written as carriage return and newline.
    const OPOST: u32 = 0o1;
    const ONLCR: u32 = 0o4;
    // Control: the speed, 8-bit characters, the receiver on, hang up on last close, no
    // modem control lines.
    const B115200: u32 = 0o10002;
    const CS8: u32 = 0o60;
    const CREAD: u32 = 0o200;
    const HUPCL: u32 = 0o2000;
    const CLOCAL: u32 = 0o4000;
    // Local: signals from ^C, ^\ and ^Z, line editing, echo with its usual variants.
    const ISIG: u32 = 0o1;
    const ICANON: u32 = 0o2;
    const ECHO: u32 = 0o10;
    const ECHOE: u32 = 0o20;
    const ECHOK: u32 = 0o40;
    const ECHOCTL: u32 = 0o1000;
    const ECHOKE: u32 = 0o4000;
    const IEXTEN: u32 = 0o100000;
    let flags = [
        ICRNL | IXON,
        OPOST | ONLCR,
        B115200 | CS8 | CREAD | HUPCL | CLOCAL,
        ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN,
    ];
    // VINTR ^C, VQUIT ^\, VERASE DEL, VKILL ^U, VEOF ^D, VTIME 0, VMIN 1, VSWTC, VSTART ^Q,
    // VSTOP ^S, VSUSP ^Z, VEOL, VREPRINT ^R, VDISCARD ^O, VWERASE ^W, VLNEXT ^V, VEOL2.
    let control: [u8; 17] = [
        0x03, 0x1c, 0x7f, 0x15, 0x04, 0, 1, 0, 0x11, 0x13, 0x1a, 0, 0x12, 0x0f, 0x17, 0x16, 0,
    ];
    let mut bytes = [0u8; TERMIOS_SIZE];
    let mut i = 0;
    while i < flags.len() {
        let word = flags[i].to_ne_bytes();
        let mut j = 0;
        while j < 4 {
            bytes[4 * i + j] = word[j];
            j += 1;
        }
        i += 1;
    }
    // `c_line` (byte 16) is 0, the normal line discipline; `c_cc` follows.
    let mut k = 0;
    while k < control.len() {
        bytes[17 + k] = control[k];
        k += 1;
    }
    bytes
};

/// The size of a `struct winsize`: rows, columns, then the width and height in pixels, each
/// an `unsigned short`.
const WINSIZE_SIZE: usize = 8;

/// The console's window size, as `TIOCSWINSZ` last set it: the console is one for the whole
/// machine. Never held while user memory is copied.
static WINDOW_SIZE: SpinLock<[u8; WINSIZE_SIZE]> = SpinLock::new([0; WINSIZE_SIZE]);

/// `ioctl(fd, request, argument)` on the console: see the module documentation. As Linux:
/// `EBADF` for a descriptor that is not open, before the request is looked at; `ENOTTY` for a
/// request the console does not answer; `EFAULT` if `argument` is not the program's memory
/// (writable, for a request that reports something). `fd` and `request` are 32-bit: the
/// registers' upper halves are ignored.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    fd: u64,
    request: u64,
    argument: u64,
) -> Result<u64, Errno> {
    if !is_console(fd) {
        return Err(Errno::EBADF);
    }
    let write = |bytes: &[u8]| {
        process
            .write_user_memory(argument, bytes)
            .map(|()| 0)
            .map_err(|_| Errno::EFAULT)
    };
    match request as u32 {
        request::TCGETS => write(&TERMIOS),
        request::TCGETS2 => {
            let mut bytes = [0; TERMIOS2_SIZE];
            bytes[..TERMIOS_SIZE].copy_from_slice(&TERMIOS);
            bytes[TERMIOS_SIZE..TERMIOS_SIZE + 4].copy_from_slice(&BAUD.to_ne_bytes());
            bytes[TERMIOS_SIZE + 4..].copy_from_slice(&BAUD.to_ne_bytes());
            write(&bytes)
        }
        request::TIOCGWINSZ => {
            let size = *WINDOW_SIZE.lock();
            write(&size)
        }
        request::TIOCSWINSZ => {
            let mut size = [0; WINSIZE_SIZE];
            process
                .read_user_memory(argument, &mut size)
                .map_err(|_| Errno::EFAULT)?;
            *WINDOW_SIZE.lock() = size;
            Ok(0)
        }
        request::FIONREAD => write(&0i32.to_ne_bytes()),
        _ => Err(Errno::ENOTTY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_console_s_settings_are_a_serial_console_s() {
        let word = |i: usize| u32::from_ne_bytes(TERMIOS[4 * i..4 * i + 4].try_into().unwrap());
        // `stty -a` on a Linux serial console: icrnl ixon; opost onlcr; 115200 cs8 cread
        // hupcl clocal; isig icanon iexten echo echoe echok echoctl echoke.
        assert_eq!(word(0), 0x500);
        assert_eq!(word(1), 0x5);
        assert_eq!(word(2), 0x1cb2);
        assert_eq!(word(3), 0x8a3b);
        assert_eq!(TERMIOS[16], 0);
        assert_eq!(&TERMIOS[17..22], b"\x03\x1c\x7f\x15\x04");
        // VMIN 1, VTIME 0: a read returns as soon as a byte arrives.
        assert_eq!((TERMIOS[17 + 6], TERMIOS[17 + 5]), (1, 0));
        assert_eq!(&TERMIOS[34..], &[0, 0]);
    }
}
