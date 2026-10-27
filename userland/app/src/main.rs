//! `app`: installs, uninstalls and lists apps (D134, docs/apps.md); see [`eggos_app::command`].

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(target_env = "musl")]
eggos_runtime::main!(eggos_app::command::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
