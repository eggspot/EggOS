//! The terminal program (D187, docs/ux/terminal-windows-v0.3.md): a shell on a pseudo-terminal
//! in a window of its own, drawn by the console terminal's engine ([`eggos_terminal`]). Users
//! open as many as they like, resize, maximise and close them, and agents read and type into
//! them as into any window. Its window's state ([`window`]), what a key sends the shell
//! ([`keys`]) and the window's title ([`title`]) are host-tested; the program itself, which
//! `/bin/settings` also serves as `/bin/terminal` so the toolkit is in the image once, opens
//! the pseudo-terminal, starts the shell and serves the window.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod keys;
#[cfg(target_env = "musl")]
pub mod program;
pub mod title;
pub mod window;

#[cfg(test)]
mod tests;
