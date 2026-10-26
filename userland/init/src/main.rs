//! EggOS's init (D106): the program the release kernel starts as process 1, from `/init`.
//! It writes the banner `EggOS <version>`, ignores the signals a terminal or another process
//! could end it with, and starts the login program its configuration names ([`config`]) in
//! a session of its own that has the console as its controlling terminal and foreground
//! group. As long as it runs, init reaps every child that ends, its orphans among them. When
//! it ends, init starts it again after the [`backoff`], or takes the machine down with
//! `reboot` if the configuration says so. Without a login program init says so and only reaps
//! children, for good.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(any(test, target_env = "musl"))]
mod backoff;
#[cfg(any(test, target_env = "musl"))]
mod config;
#[cfg(target_env = "musl")]
mod system;

#[cfg(target_env = "musl")]
eggos_runtime::main!(system::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
