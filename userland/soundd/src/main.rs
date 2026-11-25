//! `soundd`, EggOS's sound server (D774), which init starts only with a sound output, and
//! `sound`, its command, the same program under that name.

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(eggos_user)]
eggos_runtime::main!(eggos_soundd::linux::run);

#[cfg(not(eggos_user))]
fn main() {}
