//! The EggOS core utilities (D108): one program, `/bin/coreutils`, whose other names in `/bin`
//! (hard links to the same file) are its utilities: `agent` (D193), `agentlog`, `cat`, `chmod` (D170), `clip` (D197), `cp`, `date` (D150), `device` (D184), `df` (D171),
//! `echo`, `false`, `fetch` (D158), `free`, `getprop` (D188), `grep`, `halt`, `head`, `help` (D160), `hwclock` (D150), `ip` (D157), `kill` (D200), `ln` (D170), `logcat` (D188), `ls`,
//! `mkdir`, `mv`, `nslookup` (D157), `ping` (D157), `poweroff`, `ps`, `readlink` (D170), `reboot`, `rm`, `setprop` (D188), `sleep`,
//! `sync`, `tail`, `taskset` (D198), `tee`, `touch`, `trash`, `true`, `uname`, `uptime`, `wc` and `wm`. It runs
//! the utility named by the last component of the name it was started by; started as
//! `coreutils`, the one its first argument names (`coreutils --list` lists them). Each utility is pure logic over a `System` trait, with
//! host tests against a fake; `linux.rs` makes the system calls. docs/coreutils.md describes
//! what each utility does.

#![cfg_attr(eggos_user, no_std, no_main)]

#[cfg(any(test, eggos_user))]
mod agent;
#[cfg(any(test, eggos_user))]
mod agentlog;
#[cfg(any(test, eggos_user))]
mod cat;
#[cfg(any(test, eggos_user))]
mod chmod;
#[cfg(any(test, eggos_user))]
mod clip;
#[cfg(any(test, eggos_user))]
mod date;
#[cfg(any(test, eggos_user))]
mod device;
#[cfg(any(test, eggos_user))]
mod df;
#[cfg(any(test, eggos_user))]
mod disk;
#[cfg(any(test, eggos_user))]
mod echo;
#[cfg(test)]
mod examples;
#[cfg(test)]
mod fake;
#[cfg(any(test, eggos_user))]
mod fetch;
#[cfg(any(test, eggos_user))]
mod file_changes;
#[cfg(any(test, eggos_user))]
mod grep;
#[cfg(any(test, eggos_user))]
mod head;
#[cfg(any(test, eggos_user))]
mod help;
#[cfg(any(test, eggos_user))]
mod hwclock;
#[cfg(any(test, eggos_user))]
mod ip;
#[cfg(any(test, eggos_user))]
mod kill;
#[cfg(any(test, eggos_user))]
mod lines;
#[cfg(any(test, eggos_user))]
mod links;
#[cfg(eggos_user)]
mod linux;
#[cfg(any(test, eggos_user))]
mod logcat;
#[cfg(any(test, eggos_user))]
mod ls;
#[cfg(any(test, eggos_user))]
mod network;
#[cfg(any(test, eggos_user))]
mod nslookup;
#[cfg(any(test, eggos_user))]
mod options;
#[cfg(any(test, eggos_user))]
mod output;
#[cfg(any(test, eggos_user))]
mod page;
#[cfg(any(test, eggos_user))]
mod ping;
#[cfg(any(test, eggos_user))]
mod power;
#[cfg(any(test, eggos_user))]
mod processes;
#[cfg(any(test, eggos_user))]
mod properties;
#[cfg(any(test, eggos_user))]
mod sleep;
#[cfg(any(test, eggos_user))]
mod system;
#[cfg(any(test, eggos_user))]
mod taskset;
#[cfg(any(test, eggos_user))]
mod tee;
#[cfg(any(test, eggos_user))]
mod trash;
#[cfg(any(test, eggos_user))]
mod uname;
#[cfg(any(test, eggos_user))]
mod utilities;
#[cfg(any(test, eggos_user))]
mod wc;
#[cfg(any(test, eggos_user))]
mod window_capture;
#[cfg(any(test, eggos_user))]
mod window_desktop;
#[cfg(any(test, eggos_user))]
mod window_input;
#[cfg(any(test, eggos_user))]
mod window_menu;
#[cfg(any(test, eggos_user))]
mod window_tree;
#[cfg(any(test, eggos_user))]
mod wm;
#[cfg(any(test, eggos_user))]
mod zone;

#[cfg(eggos_user)]
eggos_runtime::main!(linux::run);

#[cfg(not(eggos_user))]
fn main() {}
