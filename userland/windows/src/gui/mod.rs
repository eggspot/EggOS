//! The pure parts of the layer's Win32 GUI (D129), tested on the host: the handle tables, window
//! classes, windows, the message queue, GDI objects and device contexts, and the window
//! server's input events as Win32 messages, timers and menus (D145). `win32/` puts them to work
//! on `/dev/window`. Accelerator tables, dialog templates, the keyboard layout, registered names
//! and window properties came with AutoHotkey (D178).

pub mod accelerators;
pub mod atoms;
pub mod classes;
pub mod dialog_template;
pub mod drawn;
pub mod input;
pub mod keyboard;
pub mod menus;
pub mod message;
pub mod objects;
pub mod picker;
pub mod properties;
pub mod queue;
pub mod slots;
pub mod timers;
pub mod windows;
