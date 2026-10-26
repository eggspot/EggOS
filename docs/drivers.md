# Drivers and the driver model

How EggOS finds devices and binds drivers to them (M5, D110), how the virtio core works (M8,
D111), the display (D112), input (D113), and how to add a driver.

## The pieces

| Where | What |
|---|---|
| `drivers/` (`eggos-drivers`) | Register-level drivers, architecture-independent, host-tested against fakes: `io` (`RegisterIo`), `dma` (`DmaAddr`, `DmaMemory`), `pci` (configuration space, ECAM, MCFG, BARs, capabilities), `display` (`Mode`, `Rect`, `Damage`), `input` (the input event codes, `Capabilities`, the US keymap and key repeat, pointer scaling), `virtio` (transports, the split virtqueue, the entropy device, the 2D GPU, the input device) |
| `hal/src/fdt/nodes.rs` | `Fdt::nodes`: every device-tree node with what a bus needs, allocating nothing |
| `hal/src/lib.rs` | What the firmware gives: `Platform::device_tree`, `acpi_table`, `device_apertures`, the device-tree DMA default, and `make_kernel_mappings_visible` |
| `kernel/src/devices.rs` | The model: descriptors, matching, the device table, probe passes, the `devices` capability |
| `kernel/src/devices/fdt_bus.rs`, `pci_bus.rs` | Discovery: the device-tree platform bus, PCI Express through the MCFG |
| `kernel/src/devices/window.rs` | Device windows: device registers mapped into the kernel |
| `kernel/src/devices/dma.rs` | DMA regions |
| `kernel/src/devices/virtio.rs` | virtio-mmio and virtio-pci as bus drivers, virtio-rng and virtio-gpu on top |
| `kernel/src/display.rs` | The display: the framebuffer a display driver installs, damage, flushes, the first frame |
| `kernel/src/input.rs` | Input: the devices drivers install, polling, keys to the console's line discipline, the input event queue |

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
children after their parents. `devices::for_each` is the `devices` capability: the GUI, CLI
and MCP front-ends (D22) come with the MCP server (M10). The boot log prints the count and
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
  (`virtio`, `virtio-gpu`, `virtio-input`, `virtio-rng`): a driver not enabled is not linked
  and costs nothing (D18). The release kernel enables `virtio-gpu` (D112) and `virtio-input`
  (D113) by default, which bring the virtio core; `--no-default-features` leaves every driver
  out. Test and benchmark kernels
  (`qemu-test`) enable them all.

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

## Input

`kernel/src/input.rs` (D113) is what keyboards and pointers feed:

- A driver implements `input::InputDevice` (its `Capabilities`, a `poll` with a budget) and
  calls `input::install` (at most `MAX_DEVICES`, 4).
- `input::poll::<P>()` polls every device in rounds of at most 16 events, so a round's key bytes
  (8 at most per key) fit 128 bytes, which go to the console's line discipline between rounds
  (`console::receive_from_keyboard`, the UART's path), and sends the held key's repeat when it
  is due. It returns when to poll next. The release kernel's poller thread
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
  ring of 256, one producer (the poller) and any consumers; a full queue drops new events and
  counts them. It is made with the first input device and costs nothing without one.

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
cursor queue, display events (a mode change), modes whose buffer is larger than one DMA
region (4 MiB), and a display lock that sleeps instead of spinning, DMA cache
maintenance for non-coherent devices, `ranges` translation in the device tree, PCI on Arm and
RISC-V (a BAR allocator), ACPI devices beyond the `MCFG` (AML, M13), unmapping windows and
unbinding drivers (with cross-CPU TLB shootdown for kernel mappings), an IOMMU, and the
`devices` capability's CLI and MCP front-ends (M10).
