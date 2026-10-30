//! `eggway`, EggOS's Wayland server (D140): Linux programs that speak Wayland open their
//! windows through it. Each `xdg_toplevel` becomes a window on `/dev/window` with the desktop's
//! own chrome: the client's committed pixels are copied into it, and the window server's input
//! comes back as `wl_pointer`, `wl_keyboard` and `xdg_toplevel` events. init starts it when a
//! display exists; it listens on `/run/user/0/wayland-0`. Everything but the system calls is
//! host-tested against a fake machine, and fuzzed.

#![cfg_attr(not(any(test, feature = "fuzz")), no_std)]

pub mod activation;
pub mod client;
pub mod compositor;
#[cfg(any(test, feature = "fuzz"))]
pub mod fake;
#[cfg(any(test, feature = "fuzz"))]
pub mod fuzz;
#[cfg(target_env = "musl")]
pub mod linux;
pub mod selection;
pub mod system;
#[cfg(test)]
mod tests;
pub mod text_input;
