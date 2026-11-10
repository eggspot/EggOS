use crate::efi::{self, BootServices, Guid, Handle, Status, SystemTable};
use crate::elf::{self, Image};
use crate::placement::{self, FreeRange, GIGABYTE, PAGE};
use core::ffi::c_void;
use core::ptr::{null, null_mut};
use eggos_boot_handoff::{
    self as handoff, Framebuffer, Handoff, MAX_COMMAND_LINE, MAX_MEMORY_RANGES, MEMORY_RANGE_BYTES,
    MemoryMapWriter, MemoryRange, PvhStartInfo, memory_kind, pixel_format,
};

#[cfg(target_arch = "x86_64")]
const MACHINE: u16 = elf::MACHINE_X86_64;
#[cfg(target_arch = "aarch64")]
const MACHINE: u16 = elf::MACHINE_AARCH64;

const KERNEL_PATH: [u16; 32] = utf16("\\EGGOS\\KERNEL.ELF");
const COMMAND_LINE_PATH: [u16; 32] = utf16("\\EGGOS\\CMDLINE.TXT");

const BOOT_DATA_BELOW: u64 = if cfg!(target_arch = "x86_64") {
    0xffff_ffff
} else {
    256 * GIGABYTE - 1
};
const KERNEL_BELOW: u64 = 256 * GIGABYTE;

const HANDOFF_PAGE_BYTES: usize = PAGE as usize;
const COMMAND_LINE_BYTES: usize = MAX_COMMAND_LINE + 1;
const MEMORY_MAP_BYTES: usize = MAX_MEMORY_RANGES * MEMORY_RANGE_BYTES;
const BOOT_DATA_BYTES: usize = HANDOFF_PAGE_BYTES + COMMAND_LINE_BYTES + 1 + MEMORY_MAP_BYTES;
const EXIT_ATTEMPTS: usize = 4;
const EXTRA_DESCRIPTORS: usize = 32;
const FAILURE_PAUSE_US: usize = 10_000_000;
const MAX_FRAMEBUFFER_PIXELS: u64 = 3840 * 2160;

const fn utf16(text: &str) -> [u16; 32] {
    let bytes = text.as_bytes();
    let mut out = [0u16; 32];
    let mut index = 0;
    while index < bytes.len() && index < 31 {
        out[index] = bytes[index] as u16;
        index += 1;
    }
    out
}

struct Firmware {
    image: Handle,
    system: *mut SystemTable,
    boot: *mut BootServices,
}

static mut SYSTEM: *mut SystemTable = null_mut();

fn print(text: &str) {
    // SAFETY: `SYSTEM` is written once at entry, before anything prints, and read only while
    // boot services run; the console protocol is the firmware's, valid until `ExitBootServices`.
    let system = unsafe { SYSTEM };
    if system.is_null() {
        return;
    }
    // SAFETY: the firmware's system table, valid while boot services run.
    let console = unsafe { (*system).console_out };
    if console.is_null() {
        return;
    }
    let mut buffer = [0u16; 129];
    let mut used = 0;
    let flush = |buffer: &mut [u16; 129], used: &mut usize| {
        buffer[*used] = 0;
        // SAFETY: `buffer` is a NUL-ended UTF-16 string and `console` the firmware's text output.
        unsafe { ((*console).output_string)(console, buffer.as_ptr()) };
        *used = 0;
    };
    for character in text.chars() {
        if character == '\n' {
            buffer[used] = u16::from(b'\r');
            used += 1;
        }
        let mut units = [0u16; 2];
        for unit in character.encode_utf16(&mut units) {
            buffer[used] = *unit;
            used += 1;
        }
        if used >= 124 {
            flush(&mut buffer, &mut used);
        }
    }
    if used > 0 {
        flush(&mut buffer, &mut used);
    }
}

#[unsafe(export_name = "efi_main")]
extern "efiapi" fn efi_main(image: Handle, system: *mut SystemTable) -> Status {
    // SAFETY: the entry point runs once, before anything reads `SYSTEM`.
    unsafe { SYSTEM = system };
    // SAFETY: the firmware passes its system table, valid while boot services run.
    let boot = unsafe { (*system).boot_services };
    let firmware = Firmware {
        image,
        system,
        boot,
    };
    print("EggOS: loading the kernel\n");
    let Err(message) = start(&firmware);
    print("EggOS cannot start: ");
    print(message);
    print("\n");
    // SAFETY: boot services are still running on every path that returns an error.
    unsafe { ((*boot).stall)(FAILURE_PAUSE_US) };
    efi::LOAD_ERROR
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    print("EggOS: the loader stopped on an internal error\n");
    loop {
        core::hint::spin_loop();
    }
}

impl Firmware {
    fn services(&self) -> &BootServices {
        // SAFETY: the boot services table is valid while boot services run, which every caller
        // is before `exit`.
        unsafe { &*self.boot }
    }

    fn protocol<T>(&self, handle: Handle, guid: &Guid) -> Result<*mut T, &'static str> {
        let mut interface: *mut c_void = null_mut();
        // SAFETY: a firmware call with a valid handle, GUID and out pointer.
        let status = unsafe { (self.services().handle_protocol)(handle, guid, &mut interface) };
        if efi::is_error(status) || interface.is_null() {
            return Err("a firmware protocol the loader needs is missing");
        }
        Ok(interface.cast())
    }

    fn locate<T>(&self, guid: &Guid) -> Option<*mut T> {
        let mut interface: *mut c_void = null_mut();
        // SAFETY: a firmware call with a valid GUID and out pointer.
        let status = unsafe { (self.services().locate_protocol)(guid, null_mut(), &mut interface) };
        (!efi::is_error(status) && !interface.is_null()).then_some(interface.cast())
    }

    fn allocate(&self, kind: u32, pages: usize, address: &mut u64) -> Result<(), Status> {
        // SAFETY: a firmware call with an out pointer to a live `u64`.
        let status = unsafe {
            (self.services().allocate_pages)(kind, handoff::uefi_memory_type::LOADER_DATA, pages, address)
        };
        if efi::is_error(status) {
            Err(status)
        } else {
            Ok(())
        }
    }

    fn allocate_bytes(&self, bytes: usize, below: Option<u64>) -> Result<&'static mut [u8], &'static str> {
        let pages = bytes.div_ceil(PAGE as usize).max(1);
        let mut address = below.unwrap_or(0);
        let kind = if below.is_some() {
            efi::ALLOCATE_MAX_ADDRESS
        } else {
            efi::ALLOCATE_ANY_PAGES
        };
        self.allocate(kind, pages, &mut address)
            .map_err(|_| "the firmware has no memory for the loader")?;
        // SAFETY: the firmware just gave these pages to the loader, identity-mapped as UEFI
        // requires, and nothing else uses them.
        let memory = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, pages * PAGE as usize) };
        memory.fill(0);
        Ok(memory)
    }

    fn table(&self, guid: &Guid) -> Option<u64> {
        // SAFETY: the system table is the firmware's, valid while boot services run.
        let system = unsafe { &*self.system };
        // SAFETY: the firmware's configuration table holds this many entries.
        let tables = unsafe {
            core::slice::from_raw_parts(system.configuration_table, system.configuration_table_entries)
        };
        tables
            .iter()
            .find(|table| table.vendor_guid == *guid)
            .map(|table| table.vendor_table as u64)
    }
}

fn read_file(
    firmware: &Firmware,
    root: *mut efi::File,
    path: &[u16; 32],
) -> Result<&'static [u8], &'static str> {
    let mut file: *mut efi::File = null_mut();
    // SAFETY: `root` is the volume's open root directory and `path` a NUL-ended UTF-16 path.
    let status = unsafe { ((*root).open)(root, &mut file, path.as_ptr(), efi::FILE_MODE_READ, 0) };
    if efi::is_error(status) || file.is_null() {
        return Err("a file the loader needs is not on the boot partition");
    }
    let mut info = [0u64; 128];
    let mut info_bytes = core::mem::size_of_val(&info);
    // SAFETY: `file` is open and `info` is writable for `info_bytes`.
    let status = unsafe {
        ((*file).get_info)(file, &efi::FILE_INFO, &mut info_bytes, info.as_mut_ptr().cast())
    };
    if efi::is_error(status) {
        // SAFETY: `file` is open.
        unsafe { ((*file).close)(file) };
        return Err("the firmware could not say how large a file is");
    }
    let size = usize::try_from(info[efi::FILE_INFO_FILE_SIZE / 8])
        .map_err(|_| "a file on the boot partition is too large")?;
    let buffer = firmware.allocate_bytes(size, None)?;
    let mut read = 0;
    while read < size {
        let mut chunk = size - read;
        // SAFETY: `file` is open and `buffer[read..]` is writable for `chunk` bytes.
        let status = unsafe { ((*file).read)(file, &mut chunk, buffer[read..].as_mut_ptr().cast()) };
        if efi::is_error(status) || chunk == 0 {
            // SAFETY: `file` is open.
            unsafe { ((*file).close)(file) };
            return Err("the firmware could not read a file");
        }
        read += chunk;
    }
    // SAFETY: `file` is open.
    unsafe { ((*file).close)(file) };
    Ok(&buffer[..size])
}

fn boot_partition(firmware: &Firmware, device: Handle) -> [u8; 16] {
    let Ok(mut node) = firmware.protocol::<efi::DevicePath>(device, &efi::DEVICE_PATH_PROTOCOL) else {
        return [0; 16];
    };
    for _ in 0..64 {
        // SAFETY: `node` points at a device path node of the firmware's, which ends with an end
        // node; each node's length says where the next one starts.
        let (kind, subtype, length) = unsafe {
            let header = &*node;
            (header.kind, header.subtype, usize::from(u16::from_le_bytes(header.length)))
        };
        if kind == efi::DEVICE_PATH_END && subtype == 0xff || length < 4 {
            break;
        }
        if kind == efi::DEVICE_PATH_MEDIA && subtype == efi::DEVICE_PATH_MEDIA_HARD_DRIVE && length >= 42 {
            // SAFETY: a hard drive node is `length` bytes long, at least 42.
            let bytes = unsafe { core::slice::from_raw_parts(node.cast::<u8>(), length) };
            if bytes[efi::HARD_DRIVE_SIGNATURE_TYPE_OFFSET] == efi::HARD_DRIVE_SIGNATURE_GUID {
                let mut guid = [0u8; 16];
                guid.copy_from_slice(
                    &bytes[efi::HARD_DRIVE_SIGNATURE_OFFSET..efi::HARD_DRIVE_SIGNATURE_OFFSET + 16],
                );
                return guid;
            }
        }
        // SAFETY: the next node starts `length` bytes on, inside the firmware's device path.
        node = unsafe { node.cast::<u8>().add(length).cast() };
    }
    [0; 16]
}

fn is_bgrx(info: &efi::GraphicsModeInformation) -> bool {
    match info.pixel_format {
        efi::PIXEL_BGR_RESERVED => true,
        efi::PIXEL_BIT_MASK => {
            let masks = &info.pixel_information;
            masks.red == 0x00ff_0000 && masks.green == 0x0000_ff00 && masks.blue == 0x0000_00ff
        }
        _ => false,
    }
}

fn mode_score(info: &efi::GraphicsModeInformation) -> Option<u64> {
    let pixels = u64::from(info.horizontal_resolution) * u64::from(info.vertical_resolution);
    let packed = info.pixels_per_scan_line == info.horizontal_resolution;
    (is_bgrx(info) && pixels <= MAX_FRAMEBUFFER_PIXELS).then_some(pixels * 2 + u64::from(packed))
}

fn framebuffer(firmware: &Firmware) -> Framebuffer {
    let Some(output) = firmware.locate::<efi::GraphicsOutput>(&efi::GRAPHICS_OUTPUT_PROTOCOL) else {
        return Framebuffer::default();
    };
    // SAFETY: the firmware's graphics output protocol and its mode, valid while boot services run.
    let mode = unsafe { &*(*output).mode };
    // SAFETY: the current mode's information, valid while boot services run.
    let current = unsafe { &*mode.info };
    let keep_current = is_bgrx(current) && current.pixels_per_scan_line == current.horizontal_resolution;
    if !keep_current {
        let mut best: Option<(u64, u32)> = None;
        for number in 0..mode.max_mode {
            let mut size = 0usize;
            let mut info: *mut efi::GraphicsModeInformation = null_mut();
            // SAFETY: a firmware call with out pointers to live values.
            let status = unsafe { ((*output).query_mode)(output, number, &mut size, &mut info) };
            if efi::is_error(status) || info.is_null() {
                continue;
            }
            // SAFETY: the firmware filled `info` for this mode.
            if let Some(score) = mode_score(unsafe { &*info }) {
                if best.is_none_or(|(best_score, _)| score > best_score) {
                    best = Some((score, number));
                }
            }
        }
        if let Some((_, number)) = best {
            // SAFETY: a firmware call setting a mode it listed.
            unsafe { ((*output).set_mode)(output, number) };
        }
    }
    // SAFETY: as above, re-read after a possible mode change.
    let mode = unsafe { &*(*output).mode };
    // SAFETY: as above.
    let info = unsafe { &*mode.info };
    let format = match info.pixel_format {
        efi::PIXEL_RGB_RESERVED => pixel_format::RGBX,
        _ if is_bgrx(info) => pixel_format::BGRX,
        _ => pixel_format::NONE,
    };
    Framebuffer {
        address: mode.frame_buffer_base,
        size: mode.frame_buffer_size as u64,
        width: info.horizontal_resolution,
        height: info.vertical_resolution,
        stride_pixels: info.pixels_per_scan_line,
        format,
    }
}

struct RawMemoryMap {
    buffer: &'static mut [u8],
    size: usize,
    key: usize,
    descriptor_size: usize,
}

impl RawMemoryMap {
    fn allocate(firmware: &Firmware) -> Result<Self, &'static str> {
        let mut size = 0usize;
        let mut key = 0usize;
        let mut descriptor_size = 0usize;
        let mut version = 0u32;
        // SAFETY: asking for the size only: a null buffer of size 0.
        unsafe {
            (firmware.services().get_memory_map)(&mut size, null_mut(), &mut key, &mut descriptor_size, &mut version)
        };
        if descriptor_size == 0 {
            descriptor_size = core::mem::size_of::<efi::MemoryDescriptor>();
        }
        let capacity = size + EXTRA_DESCRIPTORS * descriptor_size;
        let buffer = firmware.allocate_bytes(capacity, None)?;
        Ok(Self {
            buffer,
            size: 0,
            key: 0,
            descriptor_size,
        })
    }

    fn refresh(&mut self, firmware: &Firmware) -> Result<(), &'static str> {
        let mut size = self.buffer.len();
        let mut version = 0u32;
        // SAFETY: `buffer` is writable for `size` bytes and the other pointers are live.
        let status = unsafe {
            (firmware.services().get_memory_map)(
                &mut size,
                self.buffer.as_mut_ptr(),
                &mut self.key,
                &mut self.descriptor_size,
                &mut version,
            )
        };
        if efi::is_error(status) || self.descriptor_size < core::mem::size_of::<efi::MemoryDescriptor>() {
            return Err("the firmware could not give its memory map");
        }
        self.size = size;
        Ok(())
    }

    fn descriptors(&self) -> impl Iterator<Item = efi::MemoryDescriptor> + '_ {
        self.buffer[..self.size]
            .chunks_exact(self.descriptor_size)
            .map(|bytes| {
                // SAFETY: each chunk is at least a descriptor's size (checked in `refresh`), and
                // `read_unaligned` takes any alignment.
                unsafe { bytes.as_ptr().cast::<efi::MemoryDescriptor>().read_unaligned() }
            })
    }
}

struct Placement {
    delta: u64,
    start: u64,
    end: u64,
}

fn place_kernel(firmware: &Firmware, image: &Image<'_>) -> Result<Placement, &'static str> {
    let (low, high) = image
        .physical_extent()
        .ok_or("the kernel file has no loadable segments")?;
    let start = placement::page_floor(low);
    let end = placement::page_ceiling(high).ok_or("the kernel image is out of reach")?;
    let pages = ((end - start) / PAGE) as usize;
    let mut address = start;
    if firmware.allocate(efi::ALLOCATE_ADDRESS, pages, &mut address).is_ok() {
        return Ok(Placement { delta: 0, start, end });
    }
    if cfg!(target_arch = "x86_64") {
        return Err("the firmware uses the memory the kernel loads at (1 MiB)");
    }
    let mut map = RawMemoryMap::allocate(firmware)?;
    map.refresh(firmware)?;
    for descriptor in map.descriptors() {
        if descriptor.kind != handoff::uefi_memory_type::CONVENTIONAL {
            continue;
        }
        let free = FreeRange {
            start: descriptor.physical_start,
            end: descriptor.physical_start + descriptor.pages * PAGE,
        };
        for candidate in placement::candidates_in_gigabytes(start, end - start, free, KERNEL_BELOW) {
            let mut address = candidate;
            if firmware.allocate(efi::ALLOCATE_ADDRESS, pages, &mut address).is_ok() {
                return Ok(Placement {
                    delta: candidate - start,
                    start: candidate,
                    end: candidate + (end - start),
                });
            }
        }
    }
    Err("no free memory where the kernel can run")
}

fn copy_kernel(image: &Image<'_>, placement: &Placement) -> Result<(), &'static str> {
    // SAFETY: the loader owns the kernel's pages, which the firmware identity-maps.
    let target = unsafe {
        core::slice::from_raw_parts_mut(placement.start as *mut u8, (placement.end - placement.start) as usize)
    };
    target.fill(0);
    for segment in image.loads() {
        let bytes = image
            .file_bytes(&segment)
            .ok_or("a kernel segment runs past the file")?;
        let offset = (segment.physical + placement.delta - placement.start) as usize;
        target
            .get_mut(offset..offset + bytes.len())
            .ok_or("a kernel segment lies outside the image")?
            .copy_from_slice(bytes);
    }
    Ok(())
}

struct BootData {
    handoff: &'static mut Handoff,
    pvh: &'static mut PvhStartInfo,
    command_line: &'static mut [u8],
    ranges: &'static mut [MemoryRange],
    start: u64,
    end: u64,
}

fn boot_data(firmware: &Firmware) -> Result<BootData, &'static str> {
    let memory = firmware.allocate_bytes(BOOT_DATA_BYTES, Some(BOOT_DATA_BELOW))?;
    let start = memory.as_ptr() as u64;
    let end = start + memory.len() as u64;
    let (first_page, rest) = memory.split_at_mut(HANDOFF_PAGE_BYTES);
    let (command_line, rest) = rest.split_at_mut(COMMAND_LINE_BYTES);
    let ranges_start = rest.as_mut_ptr().align_offset(8);
    let rest = &mut rest[ranges_start..];
    // SAFETY: the first page is page-aligned and large enough for both structures one after the
    // other, which are plain integers that any zeroed bytes make valid.
    let (handoff, pvh) = unsafe {
        let base = first_page.as_mut_ptr();
        (
            &mut *base.cast::<Handoff>(),
            &mut *base.add(handoff::HANDOFF_BYTES.next_multiple_of(8)).cast::<PvhStartInfo>(),
        )
    };
    // SAFETY: `rest` starts 8-aligned and holds at least `MAX_MEMORY_RANGES` ranges' bytes,
    // zeroed, which is a valid `MemoryRange` each.
    let ranges = unsafe {
        core::slice::from_raw_parts_mut(rest.as_mut_ptr().cast::<MemoryRange>(), MAX_MEMORY_RANGES)
    };
    Ok(BootData {
        handoff,
        pvh,
        command_line,
        ranges,
        start,
        end,
    })
}

fn start(firmware: &Firmware) -> Result<core::convert::Infallible, &'static str> {
    // SAFETY: disabling the watchdog, a firmware call with no pointers of ours.
    unsafe { (firmware.services().set_watchdog_timer)(0, 0, 0, null()) };
    check_paging()?;
    let loaded = firmware.protocol::<efi::LoadedImage>(firmware.image, &efi::LOADED_IMAGE_PROTOCOL)?;
    // SAFETY: the firmware's loaded image protocol for this image.
    let device = unsafe { (*loaded).device_handle };
    let file_system = firmware.protocol::<efi::SimpleFileSystem>(device, &efi::SIMPLE_FILE_SYSTEM_PROTOCOL)?;
    let mut root: *mut efi::File = null_mut();
    // SAFETY: the firmware's file system protocol on the boot device.
    let status = unsafe { ((*file_system).open_volume)(file_system, &mut root) };
    if efi::is_error(status) || root.is_null() {
        return Err("the boot partition could not be opened");
    }
    let kernel_file = read_file(firmware, root, &KERNEL_PATH)?;
    let command_line = read_file(firmware, root, &COMMAND_LINE_PATH).unwrap_or_default();
    let image = Image::parse(kernel_file)?;
    if image.machine != MACHINE {
        return Err("the kernel on the boot partition is for another kind of CPU");
    }
    let entry = if cfg!(target_arch = "x86_64") {
        image.uefi_entry().ok_or("the kernel has no UEFI entry point")?
    } else {
        image.entry
    };
    let placement = place_kernel(firmware, &image)?;
    copy_kernel(&image, &placement)?;
    let entry = entry + placement.delta;

    let mut data = boot_data(firmware)?;
    let line = placement::trim_command_line(command_line);
    let line = &line[..line.len().min(MAX_COMMAND_LINE)];
    data.command_line[..line.len()].copy_from_slice(line);
    *data.handoff = Handoff::new();
    data.handoff.command_line = data.command_line.as_ptr() as u64;
    data.handoff.command_line_length = line.len() as u32;
    data.handoff.rsdp = firmware
        .table(&efi::ACPI_20_TABLE)
        .or_else(|| firmware.table(&efi::ACPI_10_TABLE))
        .unwrap_or(0);
    data.handoff.device_tree = firmware.table(&efi::DEVICE_TREE_TABLE).unwrap_or(0);
    data.handoff.framebuffer = framebuffer(firmware);
    data.handoff.boot_partition = boot_partition(firmware, device);
    data.handoff.kernel_start = placement.start;
    data.handoff.kernel_end = placement.end;
    data.handoff.boot_data_start = data.start;
    data.handoff.boot_data_end = data.end;
    data.handoff.uefi_system_table = firmware.system as u64;
    data.handoff.memory_map = data.ranges.as_ptr() as u64;

    print("EggOS: starting the kernel\n");
    let mut map = RawMemoryMap::allocate(firmware)?;
    exit_boot_services(firmware, &mut map)?;
    // SAFETY: boot services are gone: nothing may print any more.
    unsafe { SYSTEM = null_mut() };

    let mut writer = MemoryMapWriter::new(&mut *data.ranges);
    for descriptor in map.descriptors() {
        if let Some(kind) = handoff::kind_of_uefi_type(descriptor.kind) {
            writer.add(MemoryRange {
                start: descriptor.physical_start,
                length: descriptor.pages.saturating_mul(PAGE),
                kind,
                reserved: 0,
            });
        }
    }
    writer.mark(data.start, data.end, memory_kind::BOOT_DATA);
    data.handoff.memory_map_entries = writer.len() as u32;
    *data.pvh = PvhStartInfo::for_handoff(data.handoff, data.handoff.command_line);

    let handoff_address = core::ptr::from_ref(data.handoff) as u64;
    let pvh_address = core::ptr::from_ref(data.pvh) as u64;
    // SAFETY: boot services are gone, the kernel is in place at `placement` and its entry point
    // is `entry`; the boot data the handoff points at stays where it is.
    unsafe { enter_kernel(entry, handoff_address, pvh_address, &placement, &data) }
}

fn exit_boot_services(firmware: &Firmware, map: &mut RawMemoryMap) -> Result<(), &'static str> {
    for _ in 0..EXIT_ATTEMPTS {
        map.refresh(firmware)?;
        // SAFETY: the image handle the firmware started this loader with, and the key of the
        // memory map just read.
        let status = unsafe { (firmware.services().exit_boot_services)(firmware.image, map.key) };
        if !efi::is_error(status) {
            return Ok(());
        }
    }
    Err("the firmware would not let the kernel take over")
}

#[cfg(target_arch = "x86_64")]
fn check_paging() -> Result<(), &'static str> {
    const CR4_LA57: u64 = 1 << 12;
    let cr4: u64;
    // SAFETY: reading CR4 has no side effects.
    unsafe { core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags)) };
    if cr4 & CR4_LA57 != 0 {
        return Err("the firmware runs with 5-level paging, which the kernel does not take yet");
    }
    Ok(())
}

#[cfg(target_arch = "aarch64")]
fn check_paging() -> Result<(), &'static str> {
    Ok(())
}

/// # Safety
/// Boot services must be gone and the kernel placed at `placement` with `entry` inside it.
#[cfg(target_arch = "x86_64")]
unsafe fn enter_kernel(
    entry: u64,
    handoff: u64,
    pvh: u64,
    _placement: &Placement,
    _data: &BootData,
) -> ! {
    // SAFETY: the kernel's 64-bit UEFI entry takes the PVH start info in `rdi` and the handoff in
    // `rsi`, with the firmware's identity mapping, and never returns.
    unsafe {
        core::arch::asm!(
            "cli",
            "jmp {entry}",
            entry = in(reg) entry,
            in("rdi") pvh,
            in("rsi") handoff,
            options(noreturn),
        )
    }
}

/// # Safety
/// As for the x86_64 version.
#[cfg(target_arch = "aarch64")]
unsafe fn enter_kernel(
    entry: u64,
    handoff: u64,
    _pvh: u64,
    placement: &Placement,
    data: &BootData,
) -> ! {
    // SAFETY: the sequence cleans the kernel and the boot data to the point of coherency, turns
    // the MMU and caches off (dropping from EL2 to EL1 where the firmware ran at EL2) and enters
    // the kernel at its physical entry with the handoff in `x0`, as its entry code expects; it
    // runs from the firmware's identity mapping and never returns.
    unsafe {
        core::arch::asm!(
            "msr daifset, #0xf",
            "mrs x9, ctr_el0",
            "ubfx x9, x9, #16, #4",
            "mov x10, #4",
            "lsl x10, x10, x9",
            "sub x11, x10, #1",
            "bic x2, x2, x11",
            "1:",
            "dc civac, x2",
            "add x2, x2, x10",
            "cmp x2, x3",
            "b.lo 1b",
            "bic x4, x4, x11",
            "2:",
            "dc civac, x4",
            "add x4, x4, x10",
            "cmp x4, x5",
            "b.lo 2b",
            "dsb sy",
            "ic iallu",
            "dsb sy",
            "isb",
            "mrs x9, CurrentEL",
            "lsr x9, x9, #2",
            "cmp x9, #2",
            "b.eq 3f",
            "mrs x9, sctlr_el1",
            "bic x9, x9, #1",
            "bic x9, x9, #4",
            "bic x9, x9, #0x1000",
            "msr sctlr_el1, x9",
            "isb",
            "ic iallu",
            "dsb nsh",
            "isb",
            "br x1",
            "3:",
            "mrs x9, sctlr_el2",
            "bic x9, x9, #1",
            "bic x9, x9, #4",
            "bic x9, x9, #0x1000",
            "msr sctlr_el2, x9",
            "isb",
            "ic iallu",
            "dsb nsh",
            "isb",
            "mov x9, #0x80000000",
            "msr hcr_el2, x9",
            "mrs x9, cnthctl_el2",
            "orr x9, x9, #3",
            "msr cnthctl_el2, x9",
            "msr cntvoff_el2, xzr",
            "mrs x9, id_aa64pfr0_el1",
            "ubfx x9, x9, #24, #4",
            "cbz x9, 4f",
            "mrs x9, S3_4_C12_C9_5",
            "orr x9, x9, #1",
            "orr x9, x9, #8",
            "msr S3_4_C12_C9_5, x9",
            "isb",
            "4:",
            "mov x9, #0x0800",
            "movk x9, #0x30d0, lsl #16",
            "msr sctlr_el1, x9",
            "msr vttbr_el2, xzr",
            "mov x9, #0x33ff",
            "msr cptr_el2, x9",
            "msr hstr_el2, xzr",
            "mov x9, #0x3c5",
            "msr spsr_el2, x9",
            "msr elr_el2, x1",
            "isb",
            "eret",
            in("x0") handoff,
            in("x1") entry,
            in("x2") placement.start,
            in("x3") placement.end,
            in("x4") data.start,
            in("x5") data.end,
            options(noreturn),
        )
    }
}
