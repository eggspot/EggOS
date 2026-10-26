use super::dma::DmaRegion;
use super::{Binding, DeviceNode, Identity};
use crate::console;
use crate::display::{self, Mode, Rect};
use crate::input::{self, Button, InputEvent, Modifiers};
use crate::memory::{FRAMES, Memory};
use crate::terminal;
use eggos_drivers::dma::DmaMemory;
use eggos_drivers::input::{AxisRange, key, scale_to_screen};
use eggos_drivers::virtio::device_type;
use eggos_hal::paging::{Format, MemoryType};
use eggos_hal::{PhysicalRange, Platform};

fn devices() -> alloc::vec::Vec<DeviceNode> {
    let mut found = alloc::vec::Vec::new();
    super::for_each(&mut |_, node| found.push(*node));
    found
}

fn compatible(node: &DeviceNode, wanted: &[u8]) -> bool {
    match node.identity {
        Identity::DeviceTree { compatible, .. } => compatible
            .split(|&byte| byte == 0)
            .any(|string| string == wanted),
        _ => false,
    }
}

fn virtio_devices(found: &[DeviceNode], wanted: u32) -> usize {
    found
        .iter()
        .filter(|node| {
            matches!(node.identity, Identity::Virtio { device_type, .. } if device_type == wanted)
        })
        .count()
}

const VIRTIO_DEVICES: usize = 5;

const VIRTIO_INPUT_DEVICES: usize = 3;

fn virtio_mmio_slots(found: &[DeviceNode], expected: usize) -> Result<(), &'static str> {
    let slots: alloc::vec::Vec<_> = found
        .iter()
        .filter(|node| compatible(node, b"virtio,mmio"))
        .collect();
    if slots.len() != expected {
        return Err("the device tree did not give the machine's virtio-mmio slots");
    }
    if slots
        .iter()
        .any(|node| node.regions().next().is_none() || node.interrupts[0].is_none())
    {
        return Err("a virtio-mmio slot has no registers or no interrupt");
    }
    if slots
        .iter()
        .any(|node| matches!(node.binding, Binding::Refused(..)))
    {
        return Err("a virtio-mmio slot was refused (a legacy device?)");
    }
    let bound = slots
        .iter()
        .filter(|node| node.binding == Binding::Bound("virtio-mmio"))
        .count();
    if cfg!(feature = "virtio") && bound != VIRTIO_DEVICES {
        return Err("the virtio-mmio driver did not bind exactly the slots with a device");
    }
    Ok(())
}

fn no_normal_alias<P: Platform>(
    memory: &Memory<P>,
    found: &[DeviceNode],
) -> Result<(), &'static str> {
    if !<P::PageTableFormat as Format>::ENCODES_MEMORY_TYPE {
        return Ok(());
    }
    let mut ranges: alloc::vec::Vec<PhysicalRange> =
        found.iter().flat_map(DeviceNode::regions).collect();
    P::device_apertures(&mut |aperture| ranges.push(aperture));
    let offset = P::direct_map_offset() as u64;
    for range in ranges {
        let frames = range.covering_frames();
        let middle = frames.start + (frames.len() / 2) / 4096 * 4096;
        for page in [frames.start, middle, frames.end - 4096] {
            let alias = memory.page_tables.translate(page.wrapping_add(offset));
            if alias.is_some_and(|(_, attributes)| attributes.memory_type == MemoryType::Normal) {
                return Err("device registers are also mapped as normal memory in the direct map");
            }
        }
    }
    Ok(())
}

fn q35_functions(found: &[DeviceNode]) -> Result<(), &'static str> {
    let functions = || {
        found.iter().filter_map(|node| match node.identity {
            Identity::Pci(function) => Some((function, node)),
            _ => None,
        })
    };
    if !functions().any(|(f, _)| (f.class, f.subclass) == (6, 0) && f.address.device == 0) {
        return Err("PCI enumeration found no host bridge at 00:00.0");
    }
    let Some((_, rng)) = functions().find(|(f, _)| (f.vendor_id, f.device_id) == (0x1af4, 0x1044))
    else {
        return Err("PCI enumeration found no modern virtio-rng-pci function");
    };
    if rng.regions().next().is_none() || rng.regions().any(|region| region.start == 0) {
        return Err("the virtio-rng-pci function's BARs were not assigned");
    }
    if cfg!(feature = "virtio") && rng.binding != Binding::Bound("virtio-pci") {
        return Err("the virtio-pci driver did not bind the virtio-rng-pci function");
    }
    let gpu = functions().find(|(f, _)| (f.vendor_id, f.device_id) == (0x1af4, 0x1050));
    if cfg!(feature = "virtio")
        && gpu.is_none_or(|(_, gpu)| gpu.binding != Binding::Bound("virtio-pci"))
    {
        return Err("the virtio-pci driver did not bind a virtio-gpu-pci function");
    }
    let inputs = functions()
        .filter(|(f, node)| {
            (f.vendor_id, f.device_id) == (0x1af4, 0x1052)
                && node.binding == Binding::Bound("virtio-pci")
        })
        .count();
    if cfg!(feature = "virtio") && inputs != VIRTIO_INPUT_DEVICES {
        return Err("the virtio-pci driver did not bind the three virtio-input-pci functions");
    }
    Ok(())
}

pub(crate) fn devices_are_discovered<P: Platform>(
    memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let found = devices();
    no_normal_alias(memory, &found)?;
    let machine = if cfg!(all(target_os = "none", target_arch = "aarch64")) {
        virtio_mmio_slots(&found, 32).and_then(|()| {
            let gic = found
                .iter()
                .any(|node| compatible(node, b"arm,cortex-a15-gic"));
            let uart = found.iter().any(|node| compatible(node, b"arm,pl011"));
            (gic && uart)
                .then_some(())
                .ok_or("the device tree did not give the GIC and the PL011")
        })
    } else if cfg!(all(target_os = "none", target_arch = "riscv64")) {
        virtio_mmio_slots(&found, 8).and_then(|()| {
            found
                .iter()
                .any(|node| compatible(node, b"riscv,plic0"))
                .then_some(())
                .ok_or("the device tree did not give the PLIC")
        })
    } else if cfg!(all(target_os = "none", target_arch = "x86_64")) {
        q35_functions(&found)
    } else {
        return Ok(());
    };
    machine?;
    if cfg!(feature = "virtio") && virtio_devices(&found, device_type::ENTROPY) != 1 {
        return Err("no virtio entropy device behind the transports");
    }
    if cfg!(feature = "virtio") && virtio_devices(&found, device_type::GPU) != 1 {
        return Err("no virtio GPU behind the transports");
    }
    if cfg!(feature = "virtio")
        && virtio_devices(&found, device_type::INPUT) != VIRTIO_INPUT_DEVICES
    {
        return Err("not three virtio input devices behind the transports");
    }
    Ok(())
}

/// The line the input self-test writes when it is ready for the QEMU runner's pointer and key
/// events. Must match the runner's.
pub const INPUT_EVENTS_LINE: &str = "EGGOS-INPUT: send pointer and key events";

/// Where the runner puts the tablet's pointer: its `ABS_X` and `ABS_Y` values, 0 to 32767.
/// Must match the runner's.
pub const TABLET_POSITION: (i32, i32) = (8192, 24576);

/// How far the runner moves the mouse after that. Must match the runner's.
pub const MOUSE_MOTION: (i32, i32) = (7, -5);

const INPUT_EVENTS_TIMEOUT_MS: u64 = 10_000;

fn expected_input_events(width: u32, height: u32) -> [InputEvent; 7] {
    let range = AxisRange { min: 0, max: 32767 };
    let x = scale_to_screen(TABLET_POSITION.0, range, width);
    let y = scale_to_screen(TABLET_POSITION.1, range, height);
    let button = |pressed| InputEvent::Button {
        button: Button::Left,
        pressed,
    };
    let shift = |pressed, modifiers| InputEvent::Key {
        code: key::LEFT_SHIFT,
        pressed,
        modifiers,
    };
    [
        InputEvent::PointerMove { x, y },
        button(true),
        button(false),
        InputEvent::PointerMove {
            x: x.saturating_add_signed(MOUSE_MOTION.0),
            y: y.saturating_add_signed(MOUSE_MOTION.1),
        },
        InputEvent::Wheel { delta: 1 },
        shift(true, Modifiers::SHIFT),
        shift(false, Modifiers::NONE),
    ]
}

pub(crate) fn input_devices_deliver_events<P: Platform>(
    _memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    if !cfg!(all(target_os = "none", feature = "virtio-input")) {
        return Ok(());
    }
    let summary = input::summary();
    if summary == input::Summary::default() {
        return Err("there are no input devices (no virtio keyboard, tablet and mouse?)");
    }
    if summary.keyboards != 1 || summary.pointers != 2 {
        return Err("the input devices are not one keyboard and two pointers");
    }
    if cfg!(feature = "qemu-bench") {
        return Ok(());
    }
    let (width, height) = display::mode()
        .map(|mode| (mode.width, mode.height))
        .ok_or("there is no display to move the pointer on")?;
    input::subscribe();
    while input::next_event().is_some() {}
    P::console_write(INPUT_EVENTS_LINE.as_bytes());
    P::console_write(b"\n");
    let expected = expected_input_events(width, height);
    let mut matched = 0;
    let mut deadline = super::Deadline::after::<P>(INPUT_EVENTS_TIMEOUT_MS);
    while matched < expected.len() {
        if !deadline.keep_waiting() {
            return Err("the pointer and key events QEMU sent did not all arrive within 10 s");
        }
        input::poll::<P>();
        while let Some(event) = input::next_event() {
            if expected.get(matched) == Some(&event) {
                matched += 1;
            }
        }
    }
    if console::available_input() != 0 {
        return Err("Shift alone sent bytes to the console");
    }
    Ok(())
}

#[cfg(any(test, feature = "virtio-rng"))]
pub(crate) fn virtio_rng_returns_bytes<P: Platform>() -> Result<(), &'static str> {
    let mut first = [0u8; 32];
    let mut second = [0u8; 32];
    let mut deadline = super::Deadline::after::<P>(1000);
    let read = super::virtio::read_random(&mut first, &mut deadline);
    if read.is_none() && !cfg!(target_os = "none") {
        return Ok(());
    }
    if read != Some(32) || super::virtio::read_random(&mut second, &mut deadline) != Some(32) {
        return Err("the virtio entropy device did not return 32 bytes");
    }
    if first == [0; 32] || first == second {
        return Err("the virtio entropy device returned zeros or the same bytes twice");
    }
    Ok(())
}

const TEST_PATTERN: [(Rect, u32); 3] = [
    (Rect::new(0, 0, 64, 32), 0x00f2_b33d),
    (Rect::new(1200, 760, 100, 100), 0x00e6_eaf0),
    (Rect::new(600, 0, 1, 800), 0x00e5_534b),
];

pub(crate) fn display_shows_the_first_frame<P: Platform>(
    _memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let Some(mode) = display::mode() else {
        return match cfg!(all(target_os = "none", feature = "virtio-gpu")) {
            true => Err("there is no display (no virtio GPU found?)"),
            false => Ok(()),
        };
    };
    if mode != Mode::new(1280, 800) {
        return Err("the display's mode is not the 1280x800 xtask asks QEMU for");
    }
    let first_frame = || display::draw(|framebuffer| display::pixel_hash(framebuffer.pixels()));
    if display::flushes() == 0 || first_frame() != Some(display::FIRST_FRAME_HASH_1280X800) {
        return Err("the first frame was not drawn and shown");
    }
    let flushes = display::flushes();
    for colour in [None, Some(display::FIRST_FRAME_COLOUR)] {
        display::draw(|framebuffer| {
            for (rect, pattern) in TEST_PATTERN {
                framebuffer.fill(rect, colour.unwrap_or(pattern));
            }
        });
        if display::flush::<P>() != Ok(TEST_PATTERN.len()) {
            return Err("the display did not show a test pattern's three rectangles");
        }
        let pattern = display::draw(|framebuffer| framebuffer.pixels()[1279 + 799 * 1280]);
        if colour.is_none() && pattern != Some(0x00e6_eaf0) {
            return Err("the test pattern was not drawn clipped to the screen");
        }
    }
    if display::flushes() != flushes + 2
        || first_frame() != Some(display::FIRST_FRAME_HASH_1280X800)
    {
        return Err("the first frame did not come back after the test pattern");
    }
    Ok(())
}

pub(crate) fn terminal_draws_on_the_display<P: Platform>(
    _memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    if display::mode().is_none() {
        return match cfg!(all(target_os = "none", feature = "virtio-gpu")) {
            true => Err("there is no display (no virtio GPU found?)"),
            false => Ok(()),
        };
    }
    if terminal::is_active() {
        return Err("the terminal was shown before its self-test");
    }
    let area = terminal::start::<P>().ok_or("no terminal could be made for the display")?;
    let flushes = display::flushes();
    console::write::<P>(terminal::TEST_SCREEN.as_bytes());
    let shown = terminal::show::<P>();
    let hash = display::draw(|framebuffer| display::pixel_hash(framebuffer.pixels()));
    terminal::stop();
    let first_frame = display::show_first_frame::<P>();
    if (area.columns, area.rows) != (158, 49) {
        return Err("the terminal's grid at 1280x800 is not 158x49");
    }
    if shown.is_err() || display::flushes() <= flushes {
        return Err("the terminal's lines were not flushed to the display");
    }
    if hash != Some(terminal::TEST_SCREEN_HASH_1280X800) {
        return Err("the terminal did not draw the lines the host renders");
    }
    if first_frame.is_err() || terminal::is_active() {
        return Err("the terminal did not let go of the display and the console");
    }
    Ok(())
}

fn free_frames() -> Option<u64> {
    FRAMES.lock().as_ref().map(|frames| frames.free_frames())
}

pub(crate) fn dma_regions_are_zeroed_and_freed_after_reset<P: Platform>(
    _memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let before = free_frames().ok_or("there is no frame allocator")?;
    let region = DmaRegion::allocate(3 * 4096).map_err(|_| "no memory for a DMA region")?;
    let size_and_alignment = region.len() == 4 * 4096
        && region.dma_addr().get().is_multiple_of(4 * 4096)
        && (region.as_mut_ptr() as usize).is_multiple_of(4096);
    let zeroed = (0..region.len()).all(|offset| {
        // SAFETY: inside the region, which nothing but this test uses.
        unsafe { region.as_mut_ptr().wrapping_add(offset).read_volatile() == 0 }
    });
    // SAFETY: the region was never given to a device.
    unsafe { region.free() };
    if !size_and_alignment || !zeroed {
        return Err("a DMA region was not whole, aligned frames, zeroed");
    }
    if free_frames() != Some(before) {
        return Err("a DMA region's frames were not all given back");
    }
    #[cfg(any(test, feature = "virtio-rng"))]
    reset_and_reinitialize_the_entropy_device::<P>(before)?;
    Ok(())
}

#[cfg(any(test, feature = "virtio-rng"))]
fn reset_and_reinitialize_the_entropy_device<P: Platform>(before: u64) -> Result<(), &'static str> {
    use eggos_drivers::virtio::rng::{MEMORY_BYTES, VirtioRng};
    let Some(rng) = super::virtio::take_rng() else {
        return match cfg!(target_os = "none") {
            true => Err("there is no virtio entropy device to reset"),
            false => Ok(()),
        };
    };
    let mut deadline = super::Deadline::after::<P>(1000);
    let (transport, memory, reset) = rng
        .reset(&mut || deadline.keep_waiting())
        .map_err(|_| "the virtio entropy device's status did not read back 0 after a reset")?;
    let frames = (memory.len() / 4096) as u64;
    memory.free_after_reset(&reset);
    if free_frames() != Some(before + frames) {
        return Err("the reset device's DMA region was not given back");
    }
    let memory = DmaRegion::allocate(MEMORY_BYTES).map_err(|_| "no memory for a DMA region")?;
    let mut rng = VirtioRng::new(transport, memory, &mut || deadline.keep_waiting())
        .map_err(|_| "the virtio entropy device could not be set up again after its reset")?;
    let mut bytes = [0u8; 32];
    let read = rng.read(&mut bytes, &mut || deadline.keep_waiting());
    super::virtio::put_rng(rng);
    if read != Ok(32) {
        return Err("the virtio entropy device set up again returned no bytes");
    }
    if free_frames() != Some(before) {
        return Err("setting the virtio entropy device up again took more than its region");
    }
    Ok(())
}
