//! Architecture modules. Exactly one is compiled in, chosen by the target, and
//! its items are re-exported here so the kernel image can say `arch::init`
//! whatever it is built for.

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::*;

#[cfg(target_arch = "riscv64")]
mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64::*;

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::*;
