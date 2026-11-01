//! `app`: installs, uninstalls and lists apps (D134, docs/apps.md); see [`eggos_app::command`].

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(eggos_user)]
eggos_runtime::main!(eggos_app::command::run);

#[cfg(not(eggos_user))]
fn main() {}
