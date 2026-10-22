# Third-party dependencies

None, and there never will be. EggOS is written entirely by us: no third-party crates, with no
exceptions (CLAUDE.md, Rules; D14). When we need something a crate would provide, we write it
ourselves. `Cargo.lock` lists only EggOS's own crates.

Outside EggOS, and so not dependencies: the firmware a machine provides (UEFI, OpenSBI, QEMU's
loaders), and the GitHub-provided actions CI runs on (`actions/checkout`, `actions/cache`).
