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
//! machine down (`shutdown.rs`, D183). On a never-installed image's first boot with a disk it
//! also starts `settings --first-run` (`first_run.rs`, D224). Its own `init:` lines go to
//! `/dev/kmsg`, the serial console only, so the Terminal's first text is the banner and the
//! shell's.

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(any(test, eggos_user))]
mod agent_server;
#[cfg(any(test, eggos_user))]
mod backoff;
#[cfg(any(test, eggos_user))]
mod config;
#[cfg(any(test, eggos_user))]
mod display_server;
#[cfg(any(test, eggos_user))]
mod first_run;
#[cfg(any(test, eggos_user))]
mod network_clock;
#[cfg(any(test, eggos_user))]
mod shutdown;
#[cfg(eggos_user)]
mod system;

#[cfg(eggos_user)]
eggos_runtime::main!(system::run);

#[cfg(not(eggos_user))]
fn main() {}
