//! The kernel's boot self-tests run this program as a Linux program: it makes the calls a
//! C library's and Rust's standard library's start-up make, checks each answer is Linux's,
//! writes `EGGOS-USER: a program built from Rust source passed its startup checks` if all
//! are, and exits with 0; otherwise it names each check that failed and exits with 1.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(target_env = "musl")]
mod checks;

#[cfg(target_env = "musl")]
eggos_runtime::main!(checks::run);

#[cfg(not(target_env = "musl"))]
fn main() {}
