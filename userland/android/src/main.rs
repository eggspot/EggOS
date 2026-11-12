//! The Android layer (M15, D470 to D472), at `/usr/lib/eggos/android`: the kernel starts it for
//! an installed app's `classes.dex` (D470). It reads the app's manifest beside it, compiles the
//! app's methods ahead of time for this machine ([`eggos_dex_compiler`], never interpreting
//! them, D17), links each call to the app's compiled code or to the framework the layer
//! provides ([`framework`]), creates the launcher activity, runs its `onCreate`, and shows the
//! view it set in an EggOS window.

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(any(test, eggos_user))]
extern crate alloc;

#[cfg(any(test, eggos_user))]
mod framework;
#[cfg(all(eggos_user, target_arch = "x86_64"))]
mod layer;

#[cfg(all(eggos_user, target_arch = "x86_64"))]
eggos_runtime::main!(layer::run);

#[cfg(all(eggos_user, not(target_arch = "x86_64")))]
eggos_runtime::main!(compiles_for_x86_64_only);

#[cfg(all(eggos_user, not(target_arch = "x86_64")))]
fn compiles_for_x86_64_only(_: &eggos_runtime::startup::Startup) -> i32 {
    eggos_runtime::syscall::write_all(
        2,
        b"android: the Android layer compiles apps for x86_64 only so far\n",
    );
    1
}

#[cfg(not(eggos_user))]
fn main() {}
