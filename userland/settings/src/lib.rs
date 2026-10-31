//! Settings (D132, docs/ux/desktop-v0.3-gui.md section 10, docs/ux/toolkit-v0.3.md section
//! 11): the window built with the toolkit and the settings the `settings` command gets and
//! sets, kept apart from the system calls so host tests, and the QEMU test's expected frames,
//! draw the same window the program shows.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod activity;
pub mod agents_pane;
pub mod art;
pub mod command;
pub mod disk;
pub mod facts;
pub mod keys;
pub mod settings_window;
pub mod storage_pane;
