//! Files (D135, docs/ux/desktop-v0.3-gui.md sections 8 and 8.1): browsing folders and the host
//! share, opening what is in them, copying, moving, renaming, making folders and the Trash.
//! The logic (names, times, what an entry is, the listing, the Trash, the stepwise copy, move
//! and delete) is host-tested against [`memory::MemoryFiles`] and shared by the window and the
//! core utilities' `cp -r`, `mv` and `trash` (D22); with the `window` feature, the Files window,
//! which `/bin/settings` also serves as `/bin/files` so the toolkit is in the image once.

#![cfg_attr(not(any(test, feature = "memory")), no_std)]

pub mod clock;
#[cfg(feature = "window")]
pub mod files_window;
pub mod job;
pub mod kind;
pub mod listing;
#[cfg(any(test, feature = "memory"))]
pub mod memory;
pub mod names;
#[cfg(all(feature = "window", target_env = "musl"))]
pub mod program;
pub mod trash;
