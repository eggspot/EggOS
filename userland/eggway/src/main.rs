//! `eggway`, EggOS's Wayland server (D140): init starts it when a display exists;
//! `eggway --status` says whether one is listening.

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(eggos_user)]
eggos_runtime::main!(eggos_eggway::linux::run);

#[cfg(not(eggos_user))]
fn main() {}
