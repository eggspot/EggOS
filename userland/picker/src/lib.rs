//! The system file picker, `/bin/pick` (D391, docs/ux/file-picker-v0.3.md): one process an
//! app starts with its standard output on a pipe, which shows a window like Files for the
//! person to open a file, save under a name or choose a folder, prints the chosen paths and
//! exits. The command line ([`options`]), the filters ([`filter`]) and what choosing does
//! ([`state`]) are host-tested logic apart from any window.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod filter;
pub mod options;
pub mod picker_window;
pub mod program;
pub mod state;
