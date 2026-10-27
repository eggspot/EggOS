//! `hello-window`: a Linux program that opens a window on the EggOS desktop through
//! `/dev/window` (D127, docs/windows.md), as the window test runs it. `hello-window [-i] [-o]
//! TITLE` checks a few answers of the contract, creates a 400x300 window titled `TITLE`,
//! maps its pixels, draws a checkerboard of 16 px cells in the title's two colours, commits
//! it and writes `hello-window TITLE: shown` to its standard error; then it reads events: a
//! `Resize` it answers with a buffer of the new size, a new checkerboard and a commit, writing
//! `hello-window TITLE: resized WxH`; a `StateChanged` it reports as `state normal`,
//! `state minimised` or `state maximised`; a close request it answers by writing
//! `hello-window TITLE: close request`, destroying the window and exiting with 0. With `-i` it first waits for a line on its standard
//! input, with `-o` it writes one to its standard output once shown, so two of them in a
//! pipeline open their windows in order.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(any(test, target_env = "musl"))]
mod pattern;
#[cfg(target_env = "musl")]
mod window;

#[cfg(target_env = "musl")]
eggos_runtime::main!(window::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
