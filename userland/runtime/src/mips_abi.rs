//! MIPS64's Linux values where they differ from the generic ones every other port uses (D315):
//! the runtime and EggOS's programs work in the generic values, and [`arguments_out`] gives the
//! calls that take flags MIPS's before they are made, [`result_in`] takes `F_GETFL`'s back, and
//! [`generic_errno`] gives an error its generic number. The kernel does the reverse.

use crate::syscall::number;

type FlagPairs = &'static [(usize, usize)];

const OPEN_FLAGS: FlagPairs = &[
    (0x0008, 0o2000),
    (0x0010, 0o10000),
    (0x0080, 0o4000),
    (0x0100, 0o100),
    (0x0200, 0o1000),
    (0x0400, 0o200),
    (0x0800, 0o400),
    (0x1000, 0o20000),
    (0x2000, 0o100000),
    (0x4000, 0o4010000),
    (0x8000, 0o40000),
];

const MMAP_FLAGS: FlagPairs = &[
    (0x0400, 0x4000),
    (0x0800, 0x20),
    (0x1000, 0x100),
    (0x2000, 0x800),
    (0x4000, 0x1000),
    (0x8000, 0x2000),
    (0x1_0000, 0x8000),
    (0x2_0000, 0x1_0000),
    (0x4_0000, 0x2_0000),
    (0x8_0000, 0x4_0000),
];

const FCNTL_COMMANDS: FlagPairs = &[(14, 5), (24, 8), (23, 9)];
const SOL_SOCKET: (usize, usize) = (0xffff, 1);
const SOCKET_OPTIONS: FlagPairs = &[
    (0x0004, 2),
    (0x0010, 5),
    (0x0200, 15),
    (0x0008, 9),
    (0x0020, 6),
    (0x0080, 13),
    (0x0100, 10),
    (0x1001, 7),
    (0x1002, 8),
    (0x1005, 21),
    (0x1006, 20),
    (0x1007, 4),
    (0x1008, 3),
    (0x1009, 30),
    (0x1028, 38),
    (0x1029, 39),
    (17, 16),
    (18, 17),
];
const NONBLOCK: (usize, usize) = (0x80, 0o4000);
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;

const MIPS_ERRORS: [u16; 99] = [
    45, 78, 46, 89, 93, 90, 0, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 50, 51, 52, 53, 54, 55, 0,
    59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 74, 73, 77, 79, 80, 81, 82, 83, 84, 85, 86,
    87, 88, 91, 92, 94, 95, 96, 97, 98, 99, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130,
    131, 132, 133, 134, 143, 144, 145, 146, 147, 148, 149, 150, 151, 135, 137, 138, 139, 140, 1133,
    159, 160, 158, 161, 162, 163, 164, 165, 166, 167, 168,
];

/// The generic number of MIPS error `mips`.
pub fn generic_errno(mips: usize) -> usize {
    MIPS_ERRORS
        .iter()
        .position(|&number| usize::from(number) == mips && number != 0)
        .map_or(mips, |index| index + 35)
}

fn to_mips(value: usize, pairs: FlagPairs) -> usize {
    let known = pairs.iter().fold(0, |known, &(_, generic)| known | generic);
    pairs
        .iter()
        .filter(|&&(_, generic)| value & generic == generic)
        .fold(value & !known, |out, &(mips, _)| out | mips)
}

fn to_generic(value: usize, pairs: FlagPairs) -> usize {
    let known = pairs.iter().fold(0, |known, &(mips, _)| known | mips);
    pairs
        .iter()
        .filter(|&&(mips, _)| value & mips == mips)
        .fold(value & !known, |out, &(_, generic)| out | generic)
}

fn value_to_mips(value: usize, pairs: FlagPairs) -> usize {
    pairs
        .iter()
        .find(|&&(_, generic)| generic == value)
        .map_or(value, |&(mips, _)| mips)
}

fn nonblock(value: usize) -> usize {
    match value & NONBLOCK.1 {
        0 => value,
        _ => value & !NONBLOCK.1 | NONBLOCK.0,
    }
}

fn socket_type(value: usize) -> usize {
    let base = match value & 0xf {
        1 => 2,
        2 => 1,
        other => other,
    };
    nonblock(value & !0xf) | base
}

/// Call `number`'s arguments with MIPS's values for the generic ones.
pub fn arguments_out(number: usize, mut arguments: [usize; 6]) -> [usize; 6] {
    match number {
        number::OPENAT => arguments[2] = to_mips(arguments[2], OPEN_FLAGS),
        number::MMAP => arguments[3] = to_mips(arguments[3], MMAP_FLAGS),
        number::FCNTL => {
            if arguments[1] == F_SETFL {
                arguments[2] = to_mips(arguments[2], OPEN_FLAGS);
            }
            arguments[1] = value_to_mips(arguments[1], FCNTL_COMMANDS);
        }
        number::PIPE2 => arguments[1] = nonblock(arguments[1]),
        number::DUP3 => arguments[2] = nonblock(arguments[2]),
        number::ACCEPT4 => arguments[3] = nonblock(arguments[3]),
        number::SOCKET => arguments[1] = socket_type(arguments[1]),
        number::SETSOCKOPT if arguments[1] == SOL_SOCKET.1 => {
            arguments[1] = SOL_SOCKET.0;
            arguments[2] = value_to_mips(arguments[2], SOCKET_OPTIONS);
        }
        _ => {}
    }
    arguments
}

/// Call `number`'s result in the generic values: `F_GETFL`'s flags.
pub fn result_in(number: usize, arguments: [usize; 6], result: isize) -> isize {
    match number {
        number::FCNTL if arguments[1] == F_GETFL && result >= 0 => {
            to_generic(result as usize, OPEN_FLAGS) as isize
        }
        _ => result,
    }
}
