# Drivers and the driver model

How EggOS finds devices and binds drivers to them (M5, D110), how the virtio core works (M8,
D111), the display (D112), input (D113), disks and the host folder (D130), and how to add a
driver.

## The pieces

| Where | What |
|---|---|
| `drivers/` (`eggos-drivers`) | Register-level drivers, architecture-independent, host-tested against fakes: `io` (`RegisterIo`), `dma` (`DmaAddr`, `DmaMemory`), `pci` (configuration space, ECAM, MCFG, BARs, capabilities), `display` (`Mode`, `Rect`, `Damage`), `font` (Egg Mono 8x16, glyph ids, `draw_glyph`: the `eggos-font` crate in `font/`, re-exported, D129), `terminal` (the terminal emulator: cells, scrollback, the ANSI subset, rendering of damaged cells), `input` (the input event codes, `Capabilities`, the US keymap and key repeat, pointer scaling), `virtio` (transports, the split virtqueue, the entropy device, the 2D GPU, the input device, the block device) |
| `hal/src/fdt/nodes.rs` | `Fdt::nodes`: every device-tree node with what a bus needs, allocating nothing |
| `hal/src/lib.rs` | What the firmware gives: `Platform::device_tree`, `acpi_table`, `device_apertures`, the device-tree DMA default, and `make_kernel_mappings_visible` |
| `kernel/src/devices.rs` | The model: descriptors, matching, the device table, probe passes, the `devices` capability |
| `kernel/src/devices/fdt_bus.rs`, `pci_bus.rs` | Discovery: the device-tree platform bus, PCI Express through the MCFG |
| `kernel/src/devices/window.rs` | Device windows: device registers mapped into the kernel |
| `kernel/src/devices/dma.rs` | DMA regions |
| `kernel/src/devices/virtio.rs` | virtio-mmio and virtio-pci as bus drivers, virtio-rng, virtio-gpu, virtio-input and virtio-blk on top |
| `kernel/src/block.rs` | Disks: the `BlockDevice` drivers install and file systems take |
| `kernel/src/vfs/fat.rs` | The read-only FAT file system the host folder is mounted with |
| `eggfs/` (`eggos-eggfs`), `kernel/src/vfs/eggfs.rs` | EggFS, the journaled file system of the persistent disk (D131, docs/eggfs.md), and its folders in the VFS |
| `kernel/src/display.rs` | The display: the framebuffer a display driver installs, damage, flushes, the first frame |
| `kernel/src/input.rs` | Input: the devices drivers install, polling, keys to the console's line discipline, the input event queue |
| `kernel/src/terminal.rs` | The graphical terminal: the console's session mirrored on the display, the drawing thread |

## Discovery

`devices::discover_and_probe` runs once at boot on the boot CPU, after paging and before any
other CPU starts.

- **aarch64, riscv64.** Every enabled device-tree node with a `compatible` (the root and the
  CPUs aside) becomes a device: its name and `compatible` strings, its `reg` decoded with the
  parent's cells (only where every ancestor's `ranges` is the identity), its first two
  interrupts decoded with their controller's `#interrupt-cells` (`interrupts` through the
  inherited `interrupt-parent`, or `interrupts-extended`), and its DMA coherence
  (`dma-coherent`, `dma-noncoherent`, else the architecture's default: coherent on RISC-V,
  not on Arm).
- **x86_64.** The ACPI `MCFG` gives PCI segment 0's ECAM window, which is mapped in a device
  window (and kept out of the direct map, `Platform::device_apertures`). Every function on the
  buses it covers becomes a device, bridges followed to their secondary buses, with its memory
  BARs sized (decoding off meanwhile) and recorded by BAR number. Firmware's BAR addresses are
  kept; nothing is reassigned.

The table (`DeviceTable`) is on the heap, as large as what was found, in discovery order,
children after their parents. `devices::for_each` is the `devices` capability: its GUI, CLI
and MCP front-ends (D22) are not built yet (the MCP server exists, D136, but has no tool for
it). The boot log prints the count and
each device a bus found or a driver refused.

## Drivers

A driver is a `DriverDescriptor`, a `static` with a name, a probe pass, a match table and a
probe function:

```rust
pub static RNG_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "virtio-rng",
    pass: ProbePass::Device,
    matches: &[Match::VirtioId(virtio::device_type::ENTROPY)],
    probe: probe_rng,
};
```

- **Passes.** Interrupt controllers, then buses, then devices. Each pass offers every unbound
  device, children added by earlier drivers included, to the driver of that pass whose match
  table ranks it best.
- **Ranks.** A `compatible` match ranks by the position of the string in the device's list (its
  most specific string first), so a board-specific driver beats a generic one whatever the
  order of `BUILTIN`. On PCI an exact id ranks 0, an id range 1, a class 2. Equal ranks go to
  the driver listed first.
- **Outcomes.** `Ok` binds the device. `Err(ProbeError::Absent)` says nothing is there (an
  empty virtio-mmio slot) and leaves it unbound, which is no failure.
  `Err(ProbeError::Refused(reason))` records the refusal, which the boot log shows.
- **Registration.** `devices::BUILTIN` lists the drivers the kernel's cargo features build in
  (`virtio`, `virtio-gpu`, `virtio-input`, `virtio-rng`, `virtio-blk`): a driver not enabled
  is not linked and costs nothing (D18). The release kernel enables `virtio-gpu` (D112),
  `virtio-input` (D113) and `virtio-blk` (D130) by default, which bring the virtio core;
  `--no-default-features` leaves every driver out. Test and benchmark kernels (`qemu-test`)
  enable them all.

## What a probe function gets

`Probe`, the context every probe function takes:

- `table()`: the device table, to read the device and add children (a transport adds the
  virtio device it finds, with the parent's DMA coherence and interrupts).
- `map_window(region)`: maps registers into the kernel's device window as device memory and
  returns the virtual address. Windows stay mapped; a region inside pages already mapped
  reuses their window.
- `pci_config()`: PCI configuration space, if discovery found one.
- `deadline(ms)`: a point on the platform clock to poll a device until.

For DMA, a driver takes a `DmaRegion` (`DmaRegion::allocate(bytes)`): zeroed, physically
contiguous whole frames, reached through the direct map, implementing `DmaMemory`. It goes back
with `free_after_reset`, which wants the `virtio::ResetDone` proof that the device's status read
back 0 after a reset; a region dropped without it is leaked, since the device may still write
it. Only devices with coherent DMA are driven: a probe refuses the others.

## virtio

- **Transports.** `Transport` is the one trait device drivers use. virtio-mmio version 2 (QEMU
  must be told `-global virtio-mmio.force-legacy=false`; a version 1 device is refused with a
  message that says so) and modern virtio-pci (capabilities 1 to 5 read from the capability
  list, the BARs they name mapped, memory decoding and bus mastering turned on; legacy and
  transitional device ids are not matched).
- **Initialization.** Reset (status 0, read back until 0), `ACKNOWLEDGE`, `DRIVER`, features
  (`VIRTIO_F_VERSION_1` required, `ACCESS_PLATFORM` accepted when offered), `FEATURES_OK` read
  back, queues, `DRIVER_OK`; `FAILED` on a refusal.
- **The split virtqueue.** Descriptors, the available ring and the used ring in one DMA region,
  reached only through raw volatile or atomic accesses. A release fence before the available
  index is published, a full fence before the used ring's notification flag is read, an
  acquire load of the used index. The free list costs nothing to build (a never-used counter
  plus freed descriptors). Polled for now: interrupts are issue #97's slice 3.
- **virtio-rng.** One request queue of 8 entries and a 1 KiB request buffer in one 4 KiB
  region; `devices::virtio::read_random` fills a buffer, polling until a deadline.
- **Polled queues.** Every polled driver sets the available ring's `VIRTQ_AVAIL_F_NO_INTERRUPT`
  (`SplitQueue::disable_used_notifications`, D113), so the device raises no interrupt nobody
  takes (on virtio-mmio its line would otherwise stay asserted until slice 3 enables it).
- **Doorbells.** The kernel's transports run `eggos_hal::memory::doorbell_barrier` before every
  notification (`dmb oshst` on AArch64, `fence w, o` on RISC-V, a compiler fence on x86), so a
  device sees the ring's writes before the doorbell (D112).
- **virtio-gpu** (D112). The control queue (up to 32 entries) and its requests and responses in
  one 4 KiB region, the framebuffer in another (4 MiB at 1280x800). At probe: the display's
  mode from `GET_DISPLAY_INFO` (1024x768 when the device reports none, or when the mode's
  buffer cannot be had), `RESOURCE_CREATE_2D` in `B8G8R8X8_UNORM`, `RESOURCE_ATTACH_BACKING`,
  `SET_SCANOUT`; then the driver is the kernel's display. Every command is fenced, and a flush
  sends each damaged rectangle's `TRANSFER_TO_HOST_2D`, waits for their answers, then sends
  their `RESOURCE_FLUSH`: the spec promises neither order nor completion of unfenced commands.

- **virtio-input** (D113). One event queue of up to 64 entries and 64 event buffers of 8 bytes
  (type, code, value, little-endian) in one 4 KiB region, every buffer always posted: `poll`
  takes the events the device wrote, oldest first, up to a budget, posts their buffers again and
  notifies once. What the device reports comes from its configuration (`select` 0x11 with an
  event type for the codes it sends, 0x12 with an axis for its range): keyboard keys, buttons,
  relative X and Y, the wheel, absolute X and Y with their ranges. A device that is neither a
  keyboard nor a pointer is refused. The status queue (LEDs) is not set up.
- **virtio-blk** (D130). One request queue of up to 8 entries, the 16-byte request header, the
  status byte and a 60 KiB data area in one 64 KiB region; one request at a time, three
  descriptors (header, data, status), a transfer over 60 KiB split into several. The disk is
  addressed in 512-byte sectors whatever its block size; `VIRTIO_BLK_F_RO` is accepted and a
  read-only disk refuses writes before any request; the capacity comes from the
  configuration. `VIRTIO_BLK_F_FLUSH` is accepted when offered, and `flush` then sends a flush
  request (header and status only) so writes before it reach stable storage (D131); a device
  without it has no volatile cache. The data is copied between the caller and the region once the request is
  answered. A request not answered by its deadline stays posted and the next waits for it.
  The driver installs the disk with `block::install`.

## Input

`kernel/src/input.rs` (D113) is what keyboards and pointers feed:

- A driver implements `input::InputDevice` (its `Capabilities`, a `poll` with a budget) and
  calls `input::install` (at most `MAX_DEVICES`, 4).
- `input::poll::<P>()` polls every device in rounds of at most 16 events, so a round's key bytes
  (8 at most per key) fit 128 bytes, which go to the console's line discipline between rounds
  (`console::receive_from_keyboard`, the UART's path), at most 8 rounds (`MAX_POLL_ROUNDS`, so
  a device that never runs dry cannot keep the poller awake), and sends the held key's repeat
  when it is due. It returns when to poll next. The release kernel's poller thread
  (`input::start_polling`, started by init's boot only with an input device) sleeps until then:
  8 ms while a key or button is held or for 1 s after the last event, 32 ms otherwise, never
  later than the next repeat. An idle device costs one load of its used ring's index.
- Keys: the US keymap (`eggos_drivers::input::keymap`, the terminal design's table): characters
  with Shift, Caps Lock (letters only), Ctrl (control characters) and Alt (an `ESC` before
  them), Enter, Backspace (`DEL`; Ctrl, `BS`), Tab and Shift+Tab, Esc, the cursor, editing and
  function keys in xterm's form with its modifier parameter. A press of a key already held (a
  host's autorepeat) and value 2 are ignored; the last key pressed that sends bytes repeats
  after 400 ms, then every 40 ms, until released or another key is pressed.
- Pointers: at the end of each report (`SYN_REPORT`), the motion (a tablet's position scaled
  to the display, rounded so QEMU's truncated scaling comes back to its pixel; a mouse's motion
  added and clamped to it), then the buttons (left, right, middle), then the wheel. Without a
  display there is no position, only buttons and the wheel.
- The input event queue (`input::next_event`, `dropped_events`): `Key` (code, pressed,
  modifiers), `PointerMove` (x, y), `Button`, `Wheel`, each packed into one word in a lock-free
  ring of 256, one producer (the poller) and any consumers; a full queue drops its oldest event
  and counts it. Events are kept only once a reader called `input::subscribe` (the desktop, item
  14; the input self-test), so no keystroke waits in memory for a reader that does not exist
  (D114). It is made with the first input device and costs nothing without one.
- The graphical terminal (D114): the arrow keys follow its application cursor mode (`CSI ? 1
  h`), Shift with Page Up, Page Down, Home and End and the wheel (3 lines a notch) move its
  scrollback view and send nothing, and any key that sends bytes brings the view back to the
  live screen. A key repeat waits for a poll whose buffer has room for its whole sequence.

## Disks and the host folder

`kernel/src/block.rs` (D130) is where disks go:

- A driver implements `block::BlockDevice` (`sectors`, `is_read_only`, `read`, `write` of
  whole 512-byte sectors and `flush`, polling until a `Deadline`) and calls `block::install`
  (at most `MAX_DISKS`, 4). At boot `block::take_all` hands every disk to the mounting code,
  and a file system owns the one it takes, so nothing else reaches the device; the others go
  back with `install`. With no disk nothing is allocated.
- At boot, before `/tmp` and `/home` get their memory file systems, the kernel reads what
  each disk holds, never relying on their order (D131): EggFS (or a blank writable disk, which
  it formats) is mounted at `/home` and `/Applications` (`vfs::eggfs`), printing `vfs: EggFS
  disk mounted at /home, /Applications and /var (...)`; a FAT volume on the whole disk or its first
  MBR partition read-only at `/media/host` (`vfs::fat::Fat::mount`), printing `vfs: FAT16
  disk "QEMU VVFAT" mounted read-only at /media/host`; anything else is left untouched and
  reported. Both file systems hold their disk under a `SleepLock`, so a thread waiting for a
  transfer sleeps.
- The persistent disk: `cargo xtask run` and the run scripts attach `eggos-<arch>-disk.img`
  (`qemu::disk_args`: `-blockdev driver=file,node-name=eggfs-file,filename=<image>
  -blockdev driver=raw,node-name=eggfs,file=eggfs-file -device <the virtio-blk device>,
  drive=eggfs`).
- The host folder: `cargo xtask run --apps <folder>` and the run scripts' `--apps` attach a
  host folder with QEMU's `vvfat` driver on the virtio block device (`-blockdev
  driver=vvfat,node-name=host,dir=<folder>,read-only=on -device virtio-blk-device,drive=host`
  on the `virt` machines, `virtio-blk-pci,disable-legacy=on` on q35; `qemu::host_folder_args`).
  `vvfat` presents a 504 MiB disk with an MBR and a FAT16 volume with long names; its limits
  are in docs/limitations.md.

## The agent channel

`eggos_drivers::virtio::console` (D136) drives a virtio console device with multiple ports
(virtio 1.2, 5.3, `VIRTIO_CONSOLE_F_MULTIPORT` required), polled like the others: the control
queues (2 and 3) and port 1's receive and transmit queues (4 and 5), four 4 KiB receive
buffers and one 8 KiB transmit buffer in one 32 KiB DMA region allocated only when the device
exists. At probe the driver says it is ready, readies port 1 (refusing any other port the
device adds), waits for its name and opens it; the kernel's `CONSOLE_DRIVER` (feature
`virtio-console`, default) installs a port named `org.eggos.mcp` as the agent channel,
`kernel/src/agent_channel.rs`, which `/dev/agent` (240:1) hands to one privileged opener at a
time. A read waits by polling the device (every 4 ms while bytes moved within the last second,
every 32 ms otherwise) until device interrupts arrive (issue #97); `poll` reports `POLLIN` while
bytes or the host's end wait and `POLLOUT` always, and a `poll` waiting for input looks at the
device again at the same intervals (D137); a write waits while QEMU holds the transmit buffer
because the host has not read it. QEMU's arguments are
`qemu::agent_channel_args`: `-device virtio-serial-device,id=agent` (`virtio-serial-pci,
disable-legacy=on` on q35) and `-device virtserialport,bus=agent.0,nr=1,chardev=<chardev>,
name=org.eggos.mcp`. On riscv64 `virt`, which has 8 virtio-mmio slots, a boot with the host
folder, the disk and the channel uses all 8.

## The display

`kernel/src/display.rs` (D112) is what the graphical terminal and the desktop draw through:

- `display::draw(|framebuffer| ...)` lends the `Framebuffer` under the display's lock: its
  `mode()`, its `pixels()` (row after row, `mode().width` per row, each `0x00RRGGBB`), `fill`,
  and `damage(rect)` to record what changed.
- `display::flush::<P>()` shows the damage (at most 8 rectangles, overlapping ones merged; a
  ninth merges where the union grows least) and nothing else; nothing reaches the device while
  nothing is damaged. A failed flush keeps the damage for the next.
- At boot the kernel draws the first frame (the whole screen in `wallpaper.top`, `#0E1726`)
  and prints `boot: display 1280x800`, or `boot: no display` without one.
- A display driver implements `display::Scanout` and calls `display::install`.

## The graphical terminal

`kernel/src/terminal.rs` (D114) shows the console's session on the display, with the engine of
`eggos_drivers::terminal` (the design: docs/ux/terminal-v0.3.md):

- `terminal::start::<P>()` makes a terminal over the whole display (an 8 px padding, the grid
  `(w - 16) / 8` by `(h - 16) / 16` cells: 158x49 at 1280x800, 126x47 at 1024x768) and fills
  the screen with `term.bg`, which the drawing thread shows; `TIOCGWINSZ` then reports the grid. The release kernel
  starts it in init's boot, before `/init`, and prints `boot: terminal 158x49`; from the next
  line on the serial console and the terminal receive the same bytes. Without a display
  nothing is made.
- `console::write::<P>(bytes)` is the session's output: what programs write and the input's
  echo. With the terminal shown it writes to the UART and feeds the terminal under the
  terminal's lock, so both see the same bytes in the same order, with the UART's `\n` to
  `\r\n` (the engine's `set_onlcr`); otherwise it is the UART's write after one load. The
  kernel's own messages (`PlatformConsole`, panics) stay on the serial console.
- Feeding only updates cells. The drawing thread (`terminal::start_drawing`) renders the cells
  that changed into the framebuffer under the display's lock, then the terminal's, and flushes
  the damage after letting the terminal's lock go, at most once every 16 ms and at once after an
  idle frame; a scroll moves pixels and draws only the new line. Replies programs asked for
  (`CSI 6 n`, `CSI c`) go to the console's input from that thread.

## Adding a driver

1. Write the register-level driver in `drivers/`, generic over `RegisterIo` (or `Transport`
   for a virtio device) and `DmaMemory`, with host tests against `io::fake::FakeRegs`,
   `pci::fake::FakeConfig` or a fake virtio device (`virtio::queue::DeviceSide`). It must not
   assume an architecture or a bus.
2. Add its `DriverDescriptor` and probe function in `kernel/src/devices/`, next to its bus:
   match as specifically as the hardware allows, map registers with `map_window`, refuse
   non-coherent DMA, give memory back only after a reset, and keep the bound driver's state
   behind a lock (boxed at probe time, so nothing of it sits in the kernel image).
3. Give it a cargo feature in `kernel/Cargo.toml` and list it in `devices::BUILTIN` under
   that feature.
4. Add a self-test that the device works on every architecture QEMU offers it on, and the
   QEMU arguments in `xtask` (`ARCHES`). A hot path gets a gated benchmark (D27).
5. Record the decision in `docs/decisions.md` and the driver in the roadmap's support matrix.

## Not yet

Device interrupts for drivers (issue #97, slice 3: GICv2 SPIs, the PLIC, MSI-X; then input
needs no poller), keyboard layouts other than US and the keyboard's LEDs, the virtio GPU's
cursor queue, display events (a mode change), several requests in flight and interrupts on
virtio-blk, modes whose buffer is larger than one DMA
region (4 MiB), and a display lock that sleeps instead of spinning, DMA cache
maintenance for non-coherent devices, `ranges` translation in the device tree, PCI on Arm and
RISC-V (a BAR allocator), ACPI devices beyond the `MCFG` (AML, M13), unmapping windows and
unbinding drivers (with cross-CPU TLB shootdown for kernel mappings), an IOMMU, and the
`devices` capability's CLI and MCP front-ends (M10).
