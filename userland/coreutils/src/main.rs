//! The EggOS core utilities (D108): one program, `/bin/coreutils`, whose other names in `/bin`
//! (hard links to the same file) are its utilities: `cat`, `cp`, `echo`, `false`, `free`,
//! `grep`, `halt`, `head`, `ls`, `mkdir`, `mv`, `poweroff`, `ps`, `reboot`, `rm`, `sleep`,
//! `tail`, `touch`, `true`, `uname`, `uptime` and `wc`. It runs the utility named by the last
//! component of the name it was started by; started as `coreutils`, the one its first argument
//! names (`coreutils --list` lists them). Each utility is pure logic over a `System` trait, with
//! host tests against a fake; `linux.rs` makes the system calls. docs/coreutils.md describes
//! what each utility does.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(any(test, target_env = "musl"))]
mod cat;
#[cfg(any(test, target_env = "musl"))]
mod echo;
#[cfg(test)]
mod fake;
#[cfg(any(test, target_env = "musl"))]
mod file_changes;
#[cfg(any(test, target_env = "musl"))]
mod grep;
#[cfg(any(test, target_env = "musl"))]
mod head;
#[cfg(any(test, target_env = "musl"))]
mod lines;
#[cfg(target_env = "musl")]
mod linux;
#[cfg(any(test, target_env = "musl"))]
mod ls;
#[cfg(any(test, target_env = "musl"))]
mod options;
#[cfg(any(test, target_env = "musl"))]
mod output;
#[cfg(any(test, target_env = "musl"))]
mod power;
#[cfg(any(test, target_env = "musl"))]
mod processes;
#[cfg(any(test, target_env = "musl"))]
mod sleep;
#[cfg(any(test, target_env = "musl"))]
mod system;
#[cfg(any(test, target_env = "musl"))]
mod uname;
#[cfg(any(test, target_env = "musl"))]
mod utilities;
#[cfg(any(test, target_env = "musl"))]
mod wc;

#[cfg(target_env = "musl")]
eggos_runtime::main!(linux::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
