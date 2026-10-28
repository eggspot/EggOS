//! Installing, opening and uninstalling apps (D134, docs/apps.md): what a program is and
//! whether this machine runs it, the copy into `/Applications/<name>/` with its manifest
//! written last, the uninstall that leaves nothing behind, the `app` command, and, with the
//! `window` feature, the installer's window that `/bin/settings` also serves as
//! `/usr/libexec/app-window`. Everything but the system calls is host-tested against a fake
//! file system.

#![cfg_attr(not(test), no_std)]

pub mod architecture;
pub mod command;
#[cfg(feature = "window")]
pub mod dialog;
pub mod files;
pub mod installer;
#[cfg(target_env = "musl")]
pub mod linux;
pub mod messages;
pub mod program;
#[cfg(all(feature = "window", target_env = "musl"))]
pub mod window_program;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod installer_tests;
