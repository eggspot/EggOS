//! shell32's `CommandLineToArgvW`: the command line split as Windows splits it (an empty one is
//! the program's path, one argument), in one block
//! (the pointer array, then the arguments) that the caller frees with `LocalFree`.

use super::{error, fail, heap};
use crate::command_line::{self, Piece};
use crate::library::Export;

unsafe extern "efiapi" fn command_line_to_argv_w(line: *const u16, count: *mut i32) -> *mut usize {
    if count.is_null() {
        return fail(error::INVALID_PARAMETER, core::ptr::null_mut());
    }
    let line = match line.is_null() {
        true => &[][..],
        // SAFETY: `CommandLineToArgvW`'s contract: a NUL-terminated string.
        false => unsafe { super::file::wide_string(line) },
    };
    let program = super::process::process().program_windows;
    let (mut arguments, mut units) = (0usize, 0usize);
    split(line, program, &mut |piece| match piece {
        Piece::Unit(_) => units += 1,
        Piece::End => {
            arguments += 1;
            units += 1;
        }
    });
    let table = (arguments + 1) * 8;
    let Some(size) = units
        .checked_mul(2)
        .and_then(|text| text.checked_add(table))
    else {
        return fail(error::NOT_ENOUGH_MEMORY, core::ptr::null_mut());
    };
    let block = heap::allocate(size);
    if block.is_null() {
        return fail(error::NOT_ENOUGH_MEMORY, core::ptr::null_mut());
    }
    let pointers = block.cast::<usize>();
    let text = block.wrapping_add(table).cast::<u16>();
    let (mut argument, mut at) = (0usize, 0usize);
    let mut start = true;
    split(line, program, &mut |piece| {
        if start {
            // SAFETY: the block holds `arguments + 1` pointers first; `argument` is below
            // `arguments`.
            unsafe {
                pointers
                    .wrapping_add(argument)
                    .write(text.wrapping_add(at) as usize)
            };
            start = false;
        }
        let unit = match piece {
            Piece::Unit(unit) => unit,
            Piece::End => {
                argument += 1;
                start = true;
                0
            }
        };
        // SAFETY: the text after the pointers holds `units` units, and `at` is below it.
        unsafe { text.wrapping_add(at).write(unit) };
        at += 1;
    });
    // SAFETY: as above: the array ends with a null pointer, as `argv` does.
    unsafe { pointers.wrapping_add(arguments).write(0) };
    // SAFETY: the caller passes a writable `int` for the count.
    unsafe { count.write(arguments as i32) };
    pointers
}

fn split(line: &[u16], program: &[u16], out: &mut impl FnMut(Piece)) {
    if !line.is_empty() {
        return command_line::split(line, out);
    }
    program.iter().for_each(|unit| out(Piece::Unit(*unit)));
    out(Piece::End);
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CommandLineToArgvW" => command_line_to_argv_w,
];
