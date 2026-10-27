//! What the Windows layer draws and how its message box behaves (D129), as pure logic over a
//! window's pixels, shared by the layer and by the host's render of the Windows GUI test: GDI's
//! shapes and text with Win32's rules for which pixels each one covers, the Win32 system colours,
//! and the message box laid out, painted and driven as `docs/ux/windows-gui-v0.3.md` says.

#![cfg_attr(not(test), no_std)]

pub mod colour;
pub mod message_box;
pub mod shapes;
pub mod surface;
pub mod text;
