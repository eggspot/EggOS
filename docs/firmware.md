# Vendor firmware

Some devices run firmware their vendor writes, which the driver loads at start-up: Intel's Wi-Fi
adapters, and the display microcontroller (DMC) on Intel graphics. None of it is open source, and
none of it is in this repository or in the EggOS image (D265). eggspot mirrors the files whose
licence allows redistribution in its own component store, as optional components the user installs
on request:

```
component install wifi-firmware
component install display-firmware
```

The store is the default source. When it cannot be reached, or does not offer the component,
`component install` downloads the same files from linux-firmware's tagged release instead. Either
way every file is checked against the SHA-256 pinned in `userland/components/src/firmware.rs`, so
both sources give the same bytes.

## Where the files come from

All files come from linux-firmware's release `20261019`
(`https://gitlab.com/kernel-firmware/linux-firmware/-/raw/20261019/<path>`), pinned by SHA-256.
`cargo xtask` fetches them into `target/firmware/20261019/` when it builds the component store.

Each component installs its folder under `/usr/lib/firmware/`, which the base links into the
component. The folder also holds the licence text and a `WHENCE` file: the entries for these files
from linux-firmware's `WHENCE` (the driver's header, each file's entry, the licence lines).

### `wifi-firmware`: Intel Wi-Fi (`/usr/lib/firmware/intel/iwlwifi/`)

| File | Adapter |
| --- | --- |
| `iwlwifi-cc-a0-77.ucode` | Wi-Fi 6 AX200 |
| `iwlwifi-QuZ-a0-hr-b0-77.ucode` | Wi-Fi 6 AX201 |
| `iwlwifi-ty-a0-gf-a0-89.ucode`, `iwlwifi-ty-a0-gf-a0.pnvm` | Wi-Fi 6E AX210 |
| `iwlwifi-so-a0-gf-a0-89.ucode`, `iwlwifi-so-a0-gf-a0.pnvm` | Wi-Fi 6E AX211 |

Licence: `LICENCE.iwlwifi_firmware` (Intel; redistribution in binary form allowed, with the
copyright notice and licence).

### `display-firmware`: Intel display (`/usr/lib/firmware/i915/`)

| File | Graphics |
| --- | --- |
| `tgl_dmc_ver2_12.bin` | Tiger Lake |
| `adls_dmc_ver2_01.bin` | Alder Lake S |
| `adlp_dmc.bin` | Alder Lake P |
| `mtl_dmc.bin` | Meteor Lake |

Licence: `LICENSE.i915` (Intel; redistribution in binary form allowed, with the copyright notice
and licence).

## Files the store does not carry

linux-firmware carries only files that may be redistributed, so every file EggOS pins today is in
the store. Firmware that may not be redistributed is never mirrored: a driver that needs such a
file downloads it from the vendor on the user's request only, and it is listed here with the
reason. None of the drivers EggOS has needs one yet. A known example a later driver would meet:

- Broadcom's `b43` Wi-Fi firmware: it exists only inside Broadcom's own drivers, from which
  `b43-fwcutter` extracts it, and Broadcom's licence does not allow passing it on.

## Updating the pins

To move to a newer linux-firmware release, change `LINUX_FIRMWARE_TAG` and each file's SHA-256
(and `WHENCE`'s) in `userland/components/src/firmware.rs`, after checking each file's licence in
the new `WHENCE`. A file whose bytes do not match its pin fails the store build and the fallback
install alike.
