//! `eggway`, EggOS's Wayland server (D140): init starts it when a display exists;
//! `eggway --status` says whether one is listening.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(target_env = "musl")]
eggos_runtime::main!(eggos_eggway::linux::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
