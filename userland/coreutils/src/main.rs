//! The EggOS core utilities (D108): one program, `/bin/coreutils`, whose other names in `/bin`
//! (hard links to the same file) are its utilities: `agentlog`, `cat`, `chmod` (D170), `cp`, `date` (D150), `device` (D184), `df` (D171),
//! `echo`, `false`, `fetch` (D158), `free`, `grep`, `halt`, `head`, `help` (D160), `hwclock` (D150), `ip` (D157), `ln` (D170), `ls`,
//! `mkdir`, `mv`, `nslookup` (D157), `ping` (D157), `poweroff`, `ps`, `readlink` (D170), `reboot`, `rm`, `sleep`,
//! `sync`, `tail`, `tee`, `touch`, `trash`, `true`, `uname`, `uptime`, `wc` and `wm`. It runs
//! the utility named by the last component of the name it was started by; started as
//! `coreutils`, the one its first argument names (`coreutils --list` lists them). Each utility is pure logic over a `System` trait, with
//! host tests against a fake; `linux.rs` makes the system calls. docs/coreutils.md describes
//! what each utility does.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(any(test, target_env = "musl"))]
mod agentlog;
#[cfg(any(test, target_env = "musl"))]
mod cat;
#[cfg(any(test, target_env = "musl"))]
mod chmod;
#[cfg(any(test, target_env = "musl"))]
mod date;
#[cfg(any(test, target_env = "musl"))]
mod device;
#[cfg(any(test, target_env = "musl"))]
mod df;
#[cfg(any(test, target_env = "musl"))]
mod disk;
#[cfg(any(test, target_env = "musl"))]
mod echo;
#[cfg(test)]
mod examples;
#[cfg(test)]
mod fake;
#[cfg(any(test, target_env = "musl"))]
mod fetch;
#[cfg(any(test, target_env = "musl"))]
mod file_changes;
#[cfg(any(test, target_env = "musl"))]
mod grep;
#[cfg(any(test, target_env = "musl"))]
mod head;
#[cfg(any(test, target_env = "musl"))]
mod help;
#[cfg(any(test, target_env = "musl"))]
mod hwclock;
#[cfg(any(test, target_env = "musl"))]
mod ip;
#[cfg(any(test, target_env = "musl"))]
mod lines;
#[cfg(any(test, target_env = "musl"))]
mod links;
#[cfg(target_env = "musl")]
mod linux;
#[cfg(any(test, target_env = "musl"))]
mod ls;
#[cfg(any(test, target_env = "musl"))]
mod network;
#[cfg(any(test, target_env = "musl"))]
mod nslookup;
#[cfg(any(test, target_env = "musl"))]
mod options;
#[cfg(any(test, target_env = "musl"))]
mod output;
#[cfg(any(test, target_env = "musl"))]
mod page;
#[cfg(any(test, target_env = "musl"))]
mod ping;
#[cfg(any(test, target_env = "musl"))]
mod power;
#[cfg(any(test, target_env = "musl"))]
mod processes;
#[cfg(any(test, target_env = "musl"))]
mod sleep;
#[cfg(any(test, target_env = "musl"))]
mod system;
#[cfg(any(test, target_env = "musl"))]
mod tee;
#[cfg(any(test, target_env = "musl"))]
mod trash;
#[cfg(any(test, target_env = "musl"))]
mod uname;
#[cfg(any(test, target_env = "musl"))]
mod utilities;
#[cfg(any(test, target_env = "musl"))]
mod wc;
#[cfg(any(test, target_env = "musl"))]
mod window_capture;
#[cfg(any(test, target_env = "musl"))]
mod window_input;
#[cfg(any(test, target_env = "musl"))]
mod window_menu;
mod window_tree;
#[cfg(any(test, target_env = "musl"))]
mod wm;

#[cfg(target_env = "musl")]
eggos_runtime::main!(linux::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
