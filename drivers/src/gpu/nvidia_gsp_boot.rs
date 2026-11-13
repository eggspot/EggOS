//! What GSP-RM needs in memory before SEC2's booter starts it (D522), for GSP firmware
//! 535.113.01 on Ampere and Ada. The protected region WPR2 is laid out from the top of video
//! memory down: the VGA workspace, FRTS (which FWSEC already wrote, D513), the boot binary,
//! GSP-RM's image, its heap, the booter's metadata and a heap outside WPR2. The WPR metadata
//! tells the booter where each lies and where the image (through a three-level page table,
//! radix3), the boot binary and the image's signature sit in system memory. GSP-RM itself is
//! told through libos's memory regions (three log buffers and its arguments), whose arguments
//! name the shared memory holding the command and status queues. The booter, a heavy-secure
//! Falcon program, comes with signatures for several fuse versions; the one for the chip's
//! fuse is patched into its data. Layouts are NVIDIA's (the r535 ABI of the MIT-licensed open
//! GPU kernel modules) and the firmware file headers those nouveau's MIT-licensed `nvfw`
//! headers describe (`NVIDIA-NOTICE.md`).

use alloc::vec::Vec;

/// The GSP firmware release whose ABI this module writes.
pub const FIRMWARE_VERSION: &str = "535.113.01";
/// GSP's page: the unit of radix3 and of the queues' page table.
pub const PAGE_BYTES: u64 = 4096;
/// The bytes FWSEC's FRTS region takes.
pub const FRTS_BYTES: u64 = 1 << 20;
/// The register saying where the VGA workspace is.
pub const VGA_WORKSPACE_REGISTER: usize = 0x62_5f04;
/// The bytes of the WPR metadata.
pub const WPR_META_BYTES: usize = 256;
/// The bytes of each libos log buffer.
pub const LOG_BYTES: usize = 64 << 10;
/// The bytes of the command queue and of the status queue.
pub const QUEUE_BYTES: u64 = 0x4_0000;
/// The bytes of the RM arguments libos hands GSP-RM.
pub const RM_ARGUMENTS_BYTES: usize = 80;
/// The libos memory regions GSP-RM is given, in order.
pub const LIBOS_REGIONS: [&str; 4] = ["LOGINIT", "LOGINTR", "LOGRM", "RMARGS"];

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
const WPR_ALIGNMENT: u64 = 128 << 10;
const ELF_ALIGNMENT: u64 = 64 << 10;
const NON_WPR_HEAP_BYTES: u64 = MIB;
const HEAP_OS_BYTES: u64 = 20 << 20;
const HEAP_BASE_BYTES: u64 = 8 << 20;
const HEAP_PER_GIB: u64 = 96 << 10;
const HEAP_CLIENT_BYTES: u64 = (48 << 10) * 2048;
const HEAP_LEAST_BYTES: u64 = 84 << 20;
const VGA_WORKSPACE_ENABLED: u32 = 1 << 3;
const VGA_WORKSPACE_ADDRESS_MASK: u32 = 0xffff_ff00;
const VGA_WORKSPACE_BYTES: u64 = 0x2_0000;
const WPR_META_MAGIC: u64 = 0xdc3a_ae21_371a_60b3;
const WPR_META_REVISION: u64 = 1;
const LIBOS_ENTRY_BYTES: usize = 32;
const LIBOS_CONTIGUOUS: u8 = 1;
const LIBOS_SYSTEM_MEMORY: u8 = 1;
const QUEUE_HEADER_VERSION: u32 = 0;
const QUEUE_SWAPS_RECEIVE: u32 = 1;
const QUEUE_RECEIVE_HEADER: u32 = 32;
const PAGE_ENTRY_BYTES: u64 = 8;
const RADIX3_FIRST_LEVELS: u64 = 2 * PAGE_BYTES;

/// Where the VGA workspace starts in video memory of `fb_bytes`, from the register saying
/// where it is (`workspace`): 1 MiB below the top when it says nothing, 128 KiB below when
/// it points lower than that.
pub fn bios_start(fb_bytes: u64, workspace: u32) -> u64 {
    let base = fb_bytes.saturating_sub(MIB);
    if workspace & VGA_WORKSPACE_ENABLED == 0 {
        return base;
    }
    let address = u64::from(workspace & VGA_WORKSPACE_ADDRESS_MASK) << 8;
    if address < base {
        fb_bytes - VGA_WORKSPACE_BYTES
    } else {
        address
    }
}

/// Where FWSEC's FRTS region goes for a VGA workspace at `bios_start`: right below WPR2's
/// 128 KiB aligned end.
pub fn frts_offset(bios_start: u64) -> Option<u64> {
    align_down(bios_start, WPR_ALIGNMENT).checked_sub(FRTS_BYTES)
}

fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

/// The heap GSP-RM gets inside WPR2 on a board with `fb_bytes` of video memory.
pub fn heap_bytes(fb_bytes: u64) -> u64 {
    let gib = fb_bytes.div_ceil(GIB);
    let wanted = HEAP_OS_BYTES
        + HEAP_BASE_BYTES
        + (HEAP_PER_GIB * gib).next_multiple_of(MIB)
        + HEAP_CLIENT_BYTES.next_multiple_of(MIB);
    wanted.max(HEAP_LEAST_BYTES)
}

/// WPR2's layout: offsets in video memory, from the top down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WprLayout {
    pub fb_bytes: u64,
    pub bios_start: u64,
    pub wpr_end: u64,
    pub frts: u64,
    pub boot: u64,
    pub boot_bytes: u64,
    pub elf: u64,
    pub elf_bytes: u64,
    pub heap: u64,
    pub heap_bytes: u64,
    pub wpr_start: u64,
    pub non_wpr_heap: u64,
}

impl WprLayout {
    /// The layout for `fb_bytes` of video memory with the VGA workspace at `bios_start`, a
    /// boot binary of `boot_bytes` and an image of `elf_bytes`; `None` when they do not fit.
    pub fn new(fb_bytes: u64, bios_start: u64, boot_bytes: u64, elf_bytes: u64) -> Option<Self> {
        let wpr_end = align_down(bios_start, WPR_ALIGNMENT);
        let frts = frts_offset(bios_start)?;
        let boot = align_down(frts.checked_sub(boot_bytes)?, PAGE_BYTES);
        let elf = align_down(boot.checked_sub(elf_bytes)?, ELF_ALIGNMENT);
        let heap = align_down(elf.checked_sub(heap_bytes(fb_bytes))?, MIB);
        let wpr_start = heap.checked_sub(MIB)?;
        let non_wpr_heap = wpr_start.checked_sub(NON_WPR_HEAP_BYTES)?;
        Some(Self {
            fb_bytes,
            bios_start,
            wpr_end,
            frts,
            boot,
            boot_bytes,
            elf,
            elf_bytes,
            heap,
            heap_bytes: align_down(elf - heap, MIB),
            wpr_start,
            non_wpr_heap,
        })
    }
}

/// Where the boot binary's monitor code, data and manifest are inside it, and its version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bootloader {
    pub data: (usize, usize),
    pub code_offset: u32,
    pub data_offset: u32,
    pub manifest_offset: u32,
    pub app_version: u32,
}

/// Where the parts GSP-RM boots from sit in system memory (device addresses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemImages {
    pub radix3: u64,
    pub bootloader: u64,
    pub signature: (u64, u64),
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn file_header(file: &[u8]) -> Option<(usize, usize, usize)> {
    let header = word(file, 12)? as usize;
    let data = word(file, 16)? as usize;
    let data_bytes = word(file, 20)? as usize;
    data.checked_add(data_bytes)
        .filter(|&end| end <= file.len())?;
    Some((header, data, data_bytes))
}

/// The boot binary file `file` (`bootloader-<version>.bin`): its data and its descriptor's
/// offsets; `None` when its headers point outside it.
pub fn bootloader(file: &[u8]) -> Option<Bootloader> {
    let (header, data, data_bytes) = file_header(file)?;
    Some(Bootloader {
        data: (data, data_bytes),
        app_version: word(file, header.checked_add(28)?)?,
        manifest_offset: word(file, header.checked_add(32)?)?,
        data_offset: word(file, header.checked_add(40)?)?,
        code_offset: word(file, header.checked_add(48)?)?,
    })
}

/// The WPR metadata for `layout`, with the parts in system memory at `images` and the boot
/// binary `boot`.
pub fn wpr_meta(
    layout: &WprLayout,
    images: &SystemImages,
    boot: &Bootloader,
) -> [u8; WPR_META_BYTES] {
    let fields: [u64; 26] = [
        WPR_META_MAGIC,
        WPR_META_REVISION,
        images.radix3,
        layout.elf_bytes,
        images.bootloader,
        layout.boot_bytes,
        u64::from(boot.code_offset),
        u64::from(boot.data_offset),
        u64::from(boot.manifest_offset),
        images.signature.0,
        images.signature.1,
        layout.non_wpr_heap,
        layout.non_wpr_heap,
        NON_WPR_HEAP_BYTES,
        layout.wpr_start,
        layout.heap,
        layout.heap_bytes,
        layout.elf,
        layout.boot,
        layout.frts,
        FRTS_BYTES,
        layout.wpr_end,
        layout.fb_bytes,
        layout.bios_start,
        layout.fb_bytes - layout.bios_start,
        0,
    ];
    let mut meta = [0u8; WPR_META_BYTES];
    for (slot, value) in meta.as_chunks_mut::<8>().0.iter_mut().zip(fields) {
        slot.copy_from_slice(&value.to_le_bytes());
    }
    meta
}

/// The bytes radix3 needs for an image of `image_bytes`: the root page, the page of level 1
/// entries and the level 2 entries, one per image page; `None` past what one level 1 page
/// reaches.
pub fn radix3_bytes(image_bytes: u64) -> Option<u64> {
    let entries = image_bytes.div_ceil(PAGE_BYTES);
    let level_2 = (entries * PAGE_ENTRY_BYTES).next_multiple_of(PAGE_BYTES);
    let fits = level_2 / PAGE_BYTES <= PAGE_BYTES / PAGE_ENTRY_BYTES;
    fits.then_some(RADIX3_FIRST_LEVELS + level_2)
}

/// Writes radix3 into `table`, which sits at device address `table_address` and holds
/// [`radix3_bytes`], for the image whose pages are at `pages`, in order.
pub fn write_radix3(table: &mut [u8], table_address: u64, pages: &[u64]) -> Option<()> {
    let needed = radix3_bytes(pages.len() as u64 * PAGE_BYTES)? as usize;
    let table = table.get_mut(..needed)?;
    let mut put = |at: usize, value: u64| table[at..at + 8].copy_from_slice(&value.to_le_bytes());
    put(0, table_address + PAGE_BYTES);
    let level_2_pages = (needed as u64 - RADIX3_FIRST_LEVELS) / PAGE_BYTES;
    for page in 0..level_2_pages {
        let address = table_address + RADIX3_FIRST_LEVELS + page * PAGE_BYTES;
        put((PAGE_BYTES + page * PAGE_ENTRY_BYTES) as usize, address);
    }
    for (index, &page) in pages.iter().enumerate() {
        put(
            RADIX3_FIRST_LEVELS as usize + index * PAGE_ENTRY_BYTES as usize,
            page,
        );
    }
    Some(())
}

/// The tag libos knows a memory region by: its name's first eight letters, the first the
/// most significant.
pub fn libos_id(name: &str) -> u64 {
    name.bytes()
        .take(8)
        .fold(0, |id, letter| id << 8 | u64::from(letter))
}

/// libos's arguments page: one contiguous system memory region per name in
/// [`LIBOS_REGIONS`], each at its device address and of its bytes in `regions`.
pub fn libos_arguments(regions: &[(u64, u64); 4]) -> [u8; PAGE_BYTES as usize] {
    let mut page = [0u8; PAGE_BYTES as usize];
    for (index, (name, &(address, bytes))) in LIBOS_REGIONS.iter().zip(regions).enumerate() {
        let entry = &mut page[index * LIBOS_ENTRY_BYTES..(index + 1) * LIBOS_ENTRY_BYTES];
        entry[0..8].copy_from_slice(&libos_id(name).to_le_bytes());
        entry[8..16].copy_from_slice(&address.to_le_bytes());
        entry[16..24].copy_from_slice(&bytes.to_le_bytes());
        entry[24] = LIBOS_CONTIGUOUS;
        entry[25] = LIBOS_SYSTEM_MEMORY;
    }
    page
}

/// Writes into the log buffer `log`, at device address `address`, the page entries GSP-RM
/// reads after its first word.
pub fn write_log_pages(log: &mut [u8], address: u64) {
    let pages = log.len() as u64 / PAGE_BYTES;
    for (page, slot) in (0..pages).zip(log[8..].chunks_exact_mut(8)) {
        slot.copy_from_slice(&(address + page * PAGE_BYTES).to_le_bytes());
    }
}

/// The shared memory's layout: its page table's entries and bytes, then the command queue
/// and the status queue, [`QUEUE_BYTES`] each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedMemory {
    pub page_entries: u64,
    pub command_queue: u64,
    pub status_queue: u64,
    pub bytes: u64,
}

impl SharedMemory {
    /// The layout every boot uses.
    pub fn new() -> Self {
        let queue_pages = 2 * QUEUE_BYTES / PAGE_BYTES;
        let page_entries = queue_pages + (queue_pages * PAGE_ENTRY_BYTES).div_ceil(PAGE_BYTES);
        let table_bytes = (page_entries * PAGE_ENTRY_BYTES).next_multiple_of(PAGE_BYTES);
        Self {
            page_entries,
            command_queue: table_bytes,
            status_queue: table_bytes + QUEUE_BYTES,
            bytes: table_bytes + 2 * QUEUE_BYTES,
        }
    }

    /// Writes the page table and the command queue's header into `memory` (its
    /// [`Self::bytes`], zeroed, at device address `address`).
    pub fn initialize(&self, memory: &mut [u8], address: u64) {
        for page in 0..self.page_entries {
            let at = (page * PAGE_ENTRY_BYTES) as usize;
            memory[at..at + 8].copy_from_slice(&(address + page * PAGE_BYTES).to_le_bytes());
        }
        let header = [
            QUEUE_HEADER_VERSION,
            QUEUE_BYTES as u32,
            PAGE_BYTES as u32,
            ((QUEUE_BYTES - PAGE_BYTES) / PAGE_BYTES) as u32,
            0,
            QUEUE_SWAPS_RECEIVE,
            QUEUE_RECEIVE_HEADER,
            PAGE_BYTES as u32,
        ];
        let start = self.command_queue as usize;
        for (index, value) in header.iter().enumerate() {
            let at = start + 4 * index;
            memory[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    /// The RM arguments naming this shared memory at device address `address`, for a cold
    /// boot.
    pub fn rm_arguments(&self, address: u64) -> [u8; RM_ARGUMENTS_BYTES] {
        let mut arguments = [0u8; RM_ARGUMENTS_BYTES];
        arguments[0..8].copy_from_slice(&address.to_le_bytes());
        arguments[8..12].copy_from_slice(&(self.page_entries as u32).to_le_bytes());
        arguments[16..24].copy_from_slice(&self.command_queue.to_le_bytes());
        arguments[24..32].copy_from_slice(&self.status_queue.to_le_bytes());
        arguments
    }
}

impl Default for SharedMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// SEC2's booter, ready for [`super::nvidia_falcon::Falcon::run_secure`]: its code then its
/// data (the fuse's signature patched in), and where they go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Booter {
    pub image: Vec<u8>,
    pub code_virtual: u32,
    pub code_bytes: u32,
    pub data_bytes: u32,
    pub signature_place: u32,
    pub engine_mask: u16,
    pub ucode_id: u8,
}

/// The fuse register holding the version of ucode `ucode_id` for the engines in
/// `engine_mask` (SEC2, NVDEC or GSP), whose highest set bit the booter's signature must
/// match.
pub fn booter_fuse(engine_mask: u16, ucode_id: u8) -> Option<usize> {
    let base = match engine_mask {
        mask if mask & 1 != 0 => 0x82_4140,
        mask if mask & 1 << 2 != 0 => 0x82_4100,
        mask if mask & 1 << 10 != 0 => 0x82_41c0,
        _ => return None,
    };
    Some(base + 4 * usize::from(ucode_id.checked_sub(1)?))
}

/// The booter's engines and ucode id, read before the fuse can be.
pub fn booter_identity(file: &[u8]) -> Option<(u16, u8)> {
    let (header, _, _) = file_header(file)?;
    let meta = word(file, header.checked_add(16)?)? as usize;
    Some((
        word(file, meta.checked_add(4)?)? as u16,
        word(file, meta.checked_add(8)?)? as u8,
    ))
}

/// The booter file `file` (`booter_load-<version>.bin`), signed for the fuse value `fuse`;
/// `None` when its headers point outside it or no signature fits the fuse.
pub fn booter(file: &[u8], fuse: u32) -> Option<Booter> {
    let (header, data, data_bytes) = file_header(file)?;
    let field = |at: usize| word(file, header.checked_add(at)?);
    let (signatures, signatures_bytes) = (field(0)? as usize, field(4)? as usize);
    let place = word(file, field(8)? as usize)? as usize;
    let first_signature = word(file, field(12)? as usize)? as usize;
    let meta = field(16)? as usize;
    let count = word(file, field(24)? as usize)? as usize;
    let load = field(28)? as usize;
    let signature_bytes = signatures_bytes
        .checked_div(count)
        .filter(|&bytes| bytes > 0)?;
    let fuse_version = word(file, meta)?;
    let index = match fuse {
        0 => count - 1,
        value => fuse_version.checked_sub(32 - value.leading_zeros())? as usize,
    };
    if index >= count {
        return None;
    }
    let source = signatures
        .checked_add(first_signature)?
        .checked_add(index * signature_bytes)?;
    let signature = file.get(source..source.checked_add(signature_bytes)?)?;
    let mut body = file.get(data..data + data_bytes)?.to_vec();
    body.get_mut(place..place.checked_add(signature_bytes)?)?
        .copy_from_slice(signature);
    let load_field = |at: usize| word(file, load.checked_add(at)?).map(|value| value as usize);
    let (data_start, data_size) = (load_field(8)?, load_field(12)?);
    let (code_start, code_size) = (load_field(20)?, load_field(24)?);
    let code = body.get(code_start..code_start.checked_add(code_size)?)?;
    let data_part = body.get(data_start..data_start.checked_add(data_size)?)?;
    let mut image = Vec::with_capacity(code_size + data_size);
    image.extend_from_slice(code);
    image.extend_from_slice(data_part);
    Some(Booter {
        image,
        code_virtual: code_start as u32,
        code_bytes: code_size as u32,
        data_bytes: data_size as u32,
        signature_place: place.checked_sub(data_start)? as u32,
        engine_mask: word(file, meta + 4)? as u16,
        ucode_id: word(file, meta + 8)? as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_layout_stacks_down_from_the_vga_workspace() {
        let fb = 8 * GIB;
        let bios = bios_start(fb, 0);
        assert_eq!(bios, fb - MIB);
        let layout = WprLayout::new(fb, bios, 0x3000, 30 * MIB).unwrap();
        assert_eq!(layout.wpr_end, fb - MIB);
        assert_eq!(layout.frts, fb - 2 * MIB);
        assert_eq!(layout.boot, fb - 2 * MIB - 0x3000);
        assert_eq!(layout.elf % ELF_ALIGNMENT, 0);
        assert!(layout.heap_bytes >= HEAP_LEAST_BYTES);
        assert_eq!(layout.wpr_start, layout.heap - MIB);
        assert_eq!(layout.non_wpr_heap, layout.wpr_start - MIB);
        assert_eq!(bios_start(fb, 0x0000_1008), fb - VGA_WORKSPACE_BYTES);
        assert_eq!(WprLayout::new(64 * MIB, 63 * MIB, 0x3000, 30 * MIB), None);
    }

    #[test]
    fn radix3_points_the_root_at_level_1_and_level_2_at_the_pages() {
        let pages = [0x10_0000, 0x20_0000, 0x30_0000];
        let mut table = vec![0u8; radix3_bytes(3 * PAGE_BYTES).unwrap() as usize];
        write_radix3(&mut table, 0x8000, &pages).unwrap();
        let read = |at: usize| u64::from_le_bytes(table[at..at + 8].try_into().unwrap());
        assert_eq!(read(0), 0x9000);
        assert_eq!(read(4096), 0xa000);
        assert_eq!(read(8192 + 16), 0x30_0000);
    }

    #[test]
    fn libos_names_its_regions_by_their_letters() {
        assert_eq!(libos_id("RMARGS"), 0x524d_4152_4753);
        let page = libos_arguments(&[(0x1000, 0x10000); 4]);
        assert_eq!(&page[32..40], &libos_id("LOGINTR").to_le_bytes());
        assert_eq!(page[24..26], [LIBOS_CONTIGUOUS, LIBOS_SYSTEM_MEMORY]);
    }

    #[test]
    fn the_shared_memory_holds_its_page_table_then_two_queues() {
        let shared = SharedMemory::new();
        assert_eq!(shared.page_entries, 129);
        assert_eq!(shared.command_queue, PAGE_BYTES);
        let mut memory = vec![0u8; shared.bytes as usize];
        shared.initialize(&mut memory, 0x4000_0000);
        assert_eq!(memory[8..16], (0x4000_1000u64).to_le_bytes());
        let header = |index: usize| word(&memory, 4096 + 4 * index).unwrap();
        assert_eq!((header(1), header(2), header(3)), (0x4_0000, 4096, 63));
        let arguments = shared.rm_arguments(0x4000_0000);
        assert_eq!(word(&arguments, 8), Some(129));
    }

    fn booter_file() -> Vec<u8> {
        let mut file = vec![0u8; 0x400];
        let put = |file: &mut Vec<u8>, at: usize, value: u32| {
            file[at..at + 4].copy_from_slice(&value.to_le_bytes())
        };
        put(&mut file, 12, 0x40);
        put(&mut file, 16, 0x200);
        put(&mut file, 20, 0x200);
        put(&mut file, 0x40, 0x100);
        put(&mut file, 0x44, 0x20);
        put(&mut file, 0x48, 0x80);
        put(&mut file, 0x4c, 0x84);
        put(&mut file, 0x50, 0x90);
        put(&mut file, 0x58, 0x88);
        put(&mut file, 0x5c, 0xa0);
        put(&mut file, 0x80, 0x110);
        put(&mut file, 0x84, 0);
        put(&mut file, 0x88, 2);
        put(&mut file, 0x90, 3);
        put(&mut file, 0x94, 1);
        put(&mut file, 0x98, 4);
        put(&mut file, 0xa8, 0x100);
        put(&mut file, 0xac, 0x100);
        put(&mut file, 0xb4, 0);
        put(&mut file, 0xb8, 0x100);
        file[0x100..0x110].fill(0xaa);
        file[0x110..0x120].fill(0xbb);
        file
    }

    #[test]
    fn the_booter_takes_the_signature_its_fuse_names() {
        let file = booter_file();
        assert_eq!(booter_identity(&file), Some((1, 4)));
        assert_eq!(booter_fuse(1, 4), Some(0x82_414c));
        let signed = booter(&file, 0b11).unwrap();
        assert_eq!(signed.signature_place, 0x10);
        assert_eq!(signed.code_bytes, 0x100);
        assert_eq!(&signed.image[0x110..0x120], &[0xbb; 16]);
        let newest = booter(&file, 0).unwrap();
        assert_eq!(&newest.image[0x110..0x120], &[0xbb; 16]);
        assert_eq!(booter(&file, 0b1111), None);
    }
}
