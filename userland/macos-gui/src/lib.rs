//! What the macOS layer's AppKit draws and how it reads input (D147,
//! `docs/ux/macos-gui-v0.3.md`), as pure logic over a window's pixels, shared by the layer and by
//! the host's render of the macOS GUI test: AppKit's bottom-left coordinates as buffer pixels,
//! its colours, its primitives, controls and alert drawn with the toolkit, the main menu as the
//! window server's `SET_MENU` pool, the server's key records as `NSEvent`'s key codes,
//! characters and modifier flags, and the standard About panel (D175).

#![cfg_attr(not(test), no_std)]

pub mod about;
pub mod alert;
pub mod clicks;
pub mod colour;
pub mod geometry;
pub mod keys;
pub mod menu;
pub mod paint;
