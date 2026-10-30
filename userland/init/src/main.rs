//! EggOS's init (D106): the program the release kernel starts as process 1, from `/init`.
//! It writes the banner `EggOS <version>`, ignores the signals a terminal or another process
//! could end it with, and starts the login program its configuration names (`config.rs`) in
//! a session of its own that has the console as its controlling terminal and foreground
//! group. As long as it runs, init reaps every child that ends, its orphans among them. When
//! it ends, init starts it again after the back-off (`backoff.rs`), or takes the machine down with
//! `reboot` if the configuration says so. Without a login program init says so and only reaps
//! children, for good. When the agent channel exists, init also starts the built-in MCP server
//! and starts it again when it ends (`agent_server.rs`, D136). When a display exists, it also
//! starts the Wayland server, `eggway`, and starts it again when it ends (`display_server.rs`,
//! D140); every program it starts finds the Wayland socket through `XDG_RUNTIME_DIR` and
//! `WAYLAND_DISPLAY`. Asked to power off, restart or halt (`poweroff`, `reboot`, `halt`, the
//! desktop's power menu), or when the login program ends and the configuration says to, init
//! ends every other process, `SIGTERM` first and `SIGKILL` for those left, then takes the
//! machine down (`shutdown.rs`, D183).

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(any(test, target_env = "musl"))]
mod agent_server;
#[cfg(any(test, target_env = "musl"))]
mod backoff;
#[cfg(any(test, target_env = "musl"))]
mod config;
#[cfg(any(test, target_env = "musl"))]
mod display_server;
#[cfg(any(test, target_env = "musl"))]
mod shutdown;
#[cfg(target_env = "musl")]
mod system;

#[cfg(target_env = "musl")]
eggos_runtime::main!(system::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
