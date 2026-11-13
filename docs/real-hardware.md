# EggOS on a real machine

Status: code written, unbuilt and untested (code-only mode, owner's order of 2026-11-10). The
steps below are how it is meant to work once the build-and-fix pass has run.

## Make a USB stick

```
cargo xtask usb-image --arch x86_64
```

writes `target/image/eggos-<version>-x86_64-usb.img` (`--arch aarch64` for AArch64 UEFI machines):
a GUID partition table with one EFI system partition holding `\EFI\BOOT\BOOTX64.EFI` (EggOS's
UEFI loader, `boot/uefi`), `\EGGOS\KERNEL.ELF` (the release kernel) and `\EGGOS\CMDLINE.TXT` (the
kernel command line, one line).

Write it to a USB stick, which erases the stick. On macOS, find the stick with `diskutil list`,
then `diskutil unmountDisk /dev/diskN` and `sudo dd if=<image> of=/dev/rdiskN bs=4m`; on Linux,
`sudo dd if=<image> of=/dev/sdX bs=4M conv=fsync`; on Windows, a raw image writer.

## Boot it

Turn Secure Boot off in the firmware's setup (the loader is not signed yet; see "Secure Boot" below),
then choose the USB stick in the firmware's boot menu. EggOS starts with its home in memory.

What it drives: the firmware's framebuffer (D236), or Intel's integrated graphics from Skylake to
Meteor Lake at the firmware's mode with tear-free flips and a hardware pointer (D268); ACPI tables
and, on AArch64, GICv2/v3/v4 and PSCI (D237); NVMe and AHCI disks (D238) and USB flash drives
(D264); USB keyboards and mice, through hubs too, and PS/2 ones (D239, D254); and Intel
e1000/e1000e/igc and Realtek RTL8111/8125 wired network adapters (D240).

## Secure Boot

Nearly every PC ships with Secure Boot on, and its firmware trusts only Microsoft's keys: Windows boots
with one, and every other system (Linux distributions included) boots through Microsoft's third-party
UEFI CA. EggOS's loader is not signed by it yet, so the firmware refuses it until Secure Boot is turned
off. That is normal for an experimental system.

How EggOS gets signed, the way Linux distributions do it:

1. An EggOS signing key signs the loader and the kernel.
2. EggOS ships `shim` (the small open-source first-stage loader, BSD-licensed) with that key's
   certificate inside it.
3. The shim build goes through the public shim review on GitHub (how the key is kept, that nothing
   unsigned is booted, how a bad release is revoked, a reproducible build).
4. Microsoft signs the reviewed shim through its hardware developer portal. The firmware then trusts
   shim, and shim trusts EggOS's key.

Until then, a user can keep Secure Boot on by enrolling EggOS's own certificate in the firmware's key
setup instead of turning it off. Arm boards mostly have no Secure Boot; Apple silicon Macs need their
own reduced-security setting.

Decided (owner, 2026-11-13): the upstream `shim`, then EggOS's own Rust loader. shim is the one
exception to "no C": a third-party binary EggOS ships unchanged, while everything it starts stays EggOS's
own Rust, signed with EggOS's key. Microsoft's rules (in force since 20 October 2025) exempt a reviewed
upstream shim that hands over to an open-source loader from the yearly paid security audit (OCP SAFE)
that any other loader, our own Rust one included, would need. shim costs nothing at run time (it checks
one signature at power-on and is gone once EggOS takes over the machine); the chores it adds are
updating it when it ships a security fix (then review and Microsoft's signature again) and keeping
EggOS's key on a hardware token.

Status: not started. The signing key, a Microsoft Partner Center account and an EV code-signing
certificate are the owner's (CLAUDE.md, "Still the owner's"), and the work is planned for when EggOS is
close to 1.0. Microsoft's 2011 UEFI CA was due to expire in 2026 and new signatures come from its 2023
CA, which older PCs may need a firmware update to trust; check the current state when the work starts.

## Install it

On EggOS, in a terminal:

```
disk list
disk install /dev/vdb
```

`disk install` (also the MCP tool `disk_install`) erases the disk named and writes a GUID
partition table with a 256 MiB EFI system partition (the loader and kernel EggOS was started
from) and an EggOS partition on the rest; the first boot from that disk makes the EggOS partition
an empty EggFS volume and mounts it as `/home`, `/Applications` and `/var` (D241). Remove the stick
and restart.
