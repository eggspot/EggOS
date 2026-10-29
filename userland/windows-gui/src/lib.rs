//! What the Windows layer draws and how its own dialogs and the system's controls behave (D129,
//! D145), as pure logic over a window's pixels, shared by the layer and by the host's render of
//! the Windows GUI test: GDI's shapes and text with Win32's rules for which pixels each one
//! covers, the Win32 system colours, the message box, the controls, the menu bar and the file
//! picker laid out, painted and driven as `docs/ux/windows-gui-v0.3.md` and
//! `docs/ux/windows-controls-v0.3.md` say.

#![cfg_attr(not(test), no_std)]

pub mod colour;
pub mod controls;
pub mod file_picker;
pub mod menu;
pub mod message_box;
pub mod shapes;
pub mod surface;
pub mod text;
