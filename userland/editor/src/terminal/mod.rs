//! The Text Editor in a terminal (D582, docs/ux/text-editor-terminal-v0.1.md): `edit` on a
//! console or terminal without a desktop, or with `-t`. [`keys`] reads what the terminal
//! sends, [`session`] is the editor's state and its frames, host-tested, and [`program`] runs
//! it on the process's terminal.

pub mod keys;
#[cfg(eggos_user)]
pub mod program;
pub mod session;
