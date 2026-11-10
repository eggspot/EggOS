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

Turn Secure Boot off in the firmware's setup (the loader is not signed yet), then choose the USB
stick in the firmware's boot menu. EggOS starts with its home in memory.

What it drives: the firmware's framebuffer (D236), ACPI tables and, on AArch64, GICv2/v3/v4 and
PSCI (D237), NVMe and AHCI disks (D238), USB keyboards and mice on the root ports and PS/2 ones
(D239), and Intel e1000/e1000e/igc and Realtek RTL8111/8125 wired network adapters (D240).

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
