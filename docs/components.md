# Components (D18, D235)

EggOS installs a small base. Everything else is an optional component that you install and
remove yourself. A component that is not installed costs nothing: no disk, no memory, no
running service.

## The component store

Components come from a store. A store is one file per architecture,
`eggos-<version>-<arch>-components.img`, which `cargo xtask release` writes beside the kernel
images and lists in `SHA256SUMS`. EggOS reads the store from either of two places:

- a disk attached read-only (`/dev/vda` to `/dev/vdd`);
- the same file in the host share (`/media/host`).

This version's stores offer two components, on x86_64 and aarch64:

- `windows-apps`, the Windows layer;
- `mac-apps`, the macOS layer.

riscv64's store is empty.

`run-eggos.sh` and `run-eggos.ps1` attach the store as a read-only disk when it sits beside them
and matches its line in `SHA256SUMS`; `--no-components` (`-NoComponents`) leaves it out.
`cargo xtask run` builds the store and attaches it the same way, unless `--no-components`.

The base holds a symbolic link at each layer's path (`/usr/lib/eggos/windows`,
`/usr/lib/eggos/macos`, and `/bin/windows`) into the folder the component installs to. The link
leads nowhere until you install the component, so Windows and Mac programs do not start until
then, and `app install` tells you which component to install.

## Commands

| Command | MCP tool | What it does |
|---------|----------|--------------|
| `component list [--json]` | `component_list` | Lists the store's components and the installed ones. The store goes on standard error. |
| `component install [--dry-run] <name>` | `component_install` | Installs a component and what it needs. The MCP tool needs the `install` grant. |
| `component remove [--yes] [--dry-run] <name>` | `component_remove` | Removes a component. The MCP tool needs the `install` grant and always asks the user first. |

`app install` of a Windows or Mac program whose layer is missing tells you which component to
install, for example `component install windows-apps`.

## How an install stays safe

Installing runs as root, so the manager treats the store as untrusted input:

- **Index.** The store's index is read under hard limits: 64 KiB, 6 components and 64 files
  each. The header carries the index's SHA-256.
- **Paths.** Every path must be relative and must sit under `usr/lib/eggos/`. A path with
  `..`, `.` or an empty part is refused, and so is one that is absolute, duplicated, or nested
  inside another file's path.
- **Modes and offsets.** A file's mode must be `644` or `755`. Its bytes must be
  sector-aligned and lie inside the store.
- **No links.** The format has no symbolic links.

A component installs under `/var/lib/eggos/components/<name>/`:

- **Copy and check.** Its files are copied into a staging folder. Each file is checked against
  its SHA-256 as it is copied.
- **Commit.** The record is written, then the staging folder is renamed into place. A failure
  at any step leaves nothing installed.
- **Folders.** The folders above it must be real folders, not links.
- **One at a time.** One install or remove runs at a time, under `/run/eggos/components.lock`.

A remove is refused in two cases: while another installed component needs the component, and
while a running program runs one of its files: its command's first word, the link that word
names (`/bin/windows registry ...`), or its executable (`/proc/<pid>/exe`) is one of the
component's files, at its fixed path or where it is installed. Otherwise the remove renames the component's
folder away first, then deletes it. If a staging or removing folder is left behind, the next
action deletes it.

Without a persistent disk, `/var` is in memory, so installed components last until the
machine powers off.
