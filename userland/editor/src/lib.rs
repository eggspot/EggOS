//! The Text Editor (D149, docs/ux/text-editor-v0.3.md): a document of UTF-8 text in memory the
//! program gives it, its lines, the caret and the selection, undo and redo, finding, words and
//! columns, reading and saving files through `eggos-app`'s [`eggos_app::files::Files`], all
//! host-tested; with the `window` feature, the editor's window, which `/bin/settings` also
//! serves as `/bin/text-editor` so the toolkit is in the image once, and the `edit` command.

#![cfg_attr(not(test), no_std)]

pub mod columns;
pub mod command;
pub mod document;
pub mod editor;
#[cfg(feature = "window")]
pub mod editor_window;
pub mod file;
pub mod find;
pub mod history;
#[cfg(all(feature = "window", target_env = "musl"))]
pub mod program;
pub mod words;
