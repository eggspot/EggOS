//! SPARC64's Linux values where they differ from the generic ones (D327), as MIPS64's do
//! (`mips_abi`): the runtime and EggOS's programs work in the generic values, and
//! [`arguments_out`] gives the calls that take flags SPARC's before they are made,
//! [`result_in`] takes `F_GETFL`'s back, and [`generic_errno`] gives an error its generic
//! number. The kernel does the reverse.

use crate::syscall::number;

type FlagPairs = &'static [(usize, usize)];

const OPEN_FLAGS: FlagPairs = &[
    (0x0008, 0o2000),
    (0x0200, 0o100),
    (0x0400, 0o1000),
    (0x0800, 0o200),
    (0x2000, 0o10000),
    (0x4000, 0o4000),
    (0x8000, 0o400),
    (0x1_0000, 0o200_000),
    (0x2_0000, 0o400_000),
    (0x4_0000, 0o100_000),
    (0x10_0000, 0o40_000),
    (0x20_0000, 0o1_000_000),
    (0x40_0000, 0o2_000_000),
    (0x80_2000, 0o4_010_000),
    (0x100_0000, 0o10_000_000),
];

const MMAP_FLAGS: FlagPairs = &[(0x40, 0x4000), (0x100, 0x2000), (0x200, 0x100)];

const FCNTL_COMMANDS: FlagPairs = &[(7, 5), (8, 6), (9, 7), (6, 8), (5, 9)];
const SOL_SOCKET: (usize, usize) = (0xffff, 1);
const SOCKET_OPTIONS: FlagPairs = &[
    (0x0001, 1),
    (0x0004, 2),
    (0x1008, 3),
    (0x1007, 4),
    (0x0010, 5),
    (0x0020, 6),
    (0x1001, 7),
    (0x1002, 8),
    (0x0008, 9),
    (0x0100, 10),
    (0x0080, 13),
    (0x0200, 15),
    (0x0002, 16),
    (0x0040, 17),
    (0x0800, 18),
    (0x1000, 19),
    (0x2000, 20),
    (0x4000, 21),
    (0x8000, 30),
    (0x1028, 38),
    (0x1029, 39),
    (0x0044, 66),
    (0x0045, 67),
];
const DESCRIPTOR_FLAGS: FlagPairs = &[(0x4000, 0o4000), (0x40_0000, 0o2_000_000)];
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;

const SPARC_ERRORS: [u16; 99] = [
    78, 63, 79, 90, 66, 62, 0, 75, 77, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106,
    107, 108, 109, 72, 111, 73, 74, 80, 113, 71, 82, 83, 84, 85, 86, 87, 88, 76, 92, 115, 93, 89,
    114, 112, 124, 123, 110, 122, 116, 91, 68, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50,
    51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 64, 65, 37, 36, 70, 117, 118, 119, 120, 121, 69,
    125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135,
];

/// The generic number of SPARC error `sparc`.
pub fn generic_errno(sparc: usize) -> usize {
    SPARC_ERRORS
        .iter()
        .position(|&number| usize::from(number) == sparc && number != 0)
        .map_or(sparc, |index| index + 35)
}

fn to_sparc(value: usize, pairs: FlagPairs) -> usize {
    let known = pairs.iter().fold(0, |known, &(_, generic)| known | generic);
    pairs
        .iter()
        .filter(|&&(_, generic)| value & generic == generic)
        .fold(value & !known, |out, &(sparc, _)| out | sparc)
}

fn to_generic(value: usize, pairs: FlagPairs) -> usize {
    let known = pairs.iter().fold(0, |known, &(sparc, _)| known | sparc);
    pairs
        .iter()
        .filter(|&&(sparc, _)| value & sparc == sparc)
        .fold(value & !known, |out, &(_, generic)| out | generic)
}

fn value_to_sparc(value: usize, pairs: FlagPairs) -> usize {
    pairs
        .iter()
        .find(|&&(_, generic)| generic == value)
        .map_or(value, |&(sparc, _)| sparc)
}

/// Call `number`'s arguments with SPARC's values for the generic ones; `rt_sigaction` gets
/// its restorer argument (none) before the signal-set size.
pub fn arguments_out(number: usize, mut arguments: [usize; 6]) -> [usize; 6] {
    match number {
        number::OPENAT => arguments[2] = to_sparc(arguments[2], OPEN_FLAGS),
        number::MMAP => arguments[3] = to_sparc(arguments[3], MMAP_FLAGS),
        number::FCNTL => {
            if arguments[1] == F_SETFL {
                arguments[2] = to_sparc(arguments[2], OPEN_FLAGS);
            }
            arguments[1] = value_to_sparc(arguments[1], FCNTL_COMMANDS);
        }
        number::PIPE2 => arguments[1] = to_sparc(arguments[1], DESCRIPTOR_FLAGS),
        number::DUP3 => arguments[2] = to_sparc(arguments[2], DESCRIPTOR_FLAGS),
        number::SOCKET => arguments[1] = to_sparc(arguments[1], DESCRIPTOR_FLAGS),
        number::SETSOCKOPT if arguments[1] == SOL_SOCKET.1 => {
            arguments[1] = SOL_SOCKET.0;
            arguments[2] = value_to_sparc(arguments[2], SOCKET_OPTIONS);
        }
        number::RT_SIGACTION => {
            arguments[4] = arguments[3];
            arguments[3] = 0;
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
