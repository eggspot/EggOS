//! EggOS's UEFI loader (D236): `\EFI\BOOT\BOOTX64.EFI` or `BOOTAA64.EFI` on the EFI system
//! partition of a USB stick or an installed disk. It reads `\EGGOS\KERNEL.ELF` and
//! `\EGGOS\CMDLINE.TXT` from the partition it was loaded from, places the kernel, finds the ACPI
//! root, the device tree and the framebuffer, leaves boot services and enters the kernel with an
//! [`eggos_boot_handoff::Handoff`]. On the development host it is only a stub, so `cargo build`
//! works everywhere.

#![cfg_attr(eggos_uefi, no_std, no_main)]

#[cfg(eggos_uefi)]
mod efi;
#[cfg_attr(not(eggos_uefi), allow(dead_code))]
mod elf;
#[cfg(eggos_uefi)]
mod loader;
#[cfg_attr(not(eggos_uefi), allow(dead_code))]
mod placement;

#[cfg(not(eggos_uefi))]
fn main() {}
