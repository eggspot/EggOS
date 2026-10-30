//! Installing, opening and uninstalling apps (D134, docs/apps.md): what a program is and
//! whether this machine runs it, the copy into `/Applications/<name>/` with its manifest
//! written last, installing from a `.zip` (D148), the app's own icon (D148), the uninstall
//! that leaves nothing behind, the `app` command, and, with the
//! `window` feature, the installer's window that `/bin/settings` also serves as
//! `/usr/libexec/app-window`. Everything but the system calls is host-tested against a fake
//! file system.

#![cfg_attr(not(test), no_std)]

pub mod architecture;
pub mod archive;
pub mod command;
#[cfg(feature = "window")]
pub mod dialog;
pub mod files;
pub mod icon;
#[allow(clippy::result_large_err)]
pub mod installer;
#[cfg(target_env = "musl")]
pub mod linux;
pub mod messages;
pub mod program;
pub mod reader;
pub mod translation;
#[cfg(all(feature = "window", target_env = "musl"))]
pub mod window_program;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod installer_tests;
