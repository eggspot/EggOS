//! The Downloads window (D224, docs/ux/downloads-v0.3.md): every download `fetch` recorded
//! since boot, newest first, read again once a second while the window shows; a web address
//! field that starts `fetch` into `/home/root/Downloads`, `Cancel` and `Clear`, which do what
//! `download cancel` and `download clear` do through the shared `eggos-download-list`. An
//! optional component (D18): `/bin/settings` serves it as `/bin/downloads` when the image has
//! that name, so the toolkit is in the image once. Everything but the system calls is
//! host-tested against a fake system.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod address;
mod cells;
pub mod downloads_window;
#[cfg(eggos_user)]
pub mod program;
