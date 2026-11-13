//! `session-bus`, EggOS's D-Bus session bus (D575): init starts it with the display server and
//! every program finds it through `DBUS_SESSION_BUS_ADDRESS`.

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(eggos_user)]
extern crate alloc;

#[cfg(eggos_user)]
eggos_runtime::main!(eggos_dbus::linux::run);

#[cfg(eggos_user)]
#[global_allocator]
static MEMORY: eggos_runtime::pages::Pages = eggos_runtime::pages::Pages;

#[cfg(not(eggos_user))]
fn main() {}
