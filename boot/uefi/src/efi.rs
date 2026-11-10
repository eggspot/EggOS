#![allow(dead_code)]

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Status = usize;
pub type PhysicalAddress = u64;

pub const SUCCESS: Status = 0;
const ERROR_BIT: Status = 1 << (usize::BITS - 1);
pub const BUFFER_TOO_SMALL: Status = ERROR_BIT | 5;
pub const INVALID_PARAMETER: Status = ERROR_BIT | 2;
pub const NOT_FOUND: Status = ERROR_BIT | 14;
pub const LOAD_ERROR: Status = ERROR_BIT | 1;
pub const OUT_OF_RESOURCES: Status = ERROR_BIT | 9;
pub const UNSUPPORTED: Status = ERROR_BIT | 3;

pub fn is_error(status: Status) -> bool {
    status & ERROR_BIT != 0
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub first: u32,
    pub second: u16,
    pub third: u16,
    pub rest: [u8; 8],
}

impl Guid {
    pub const fn new(first: u32, second: u16, third: u16, rest: [u8; 8]) -> Self {
        Self {
            first,
            second,
            third,
            rest,
        }
    }
}

pub const LOADED_IMAGE_PROTOCOL: Guid = Guid::new(
    0x5b1b_31a1,
    0x9562,
    0x11d2,
    [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
);
pub const SIMPLE_FILE_SYSTEM_PROTOCOL: Guid = Guid::new(
    0x964e_5b22,
    0x6459,
    0x11d2,
    [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
);
pub const DEVICE_PATH_PROTOCOL: Guid = Guid::new(
    0x0957_6e91,
    0x6d3f,
    0x11d2,
    [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
);
pub const FILE_INFO: Guid = Guid::new(
    0x0957_6e92,
    0x6d3f,
    0x11d2,
    [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
);
pub const GRAPHICS_OUTPUT_PROTOCOL: Guid = Guid::new(
    0x9042_a9de,
    0x23dc,
    0x4a38,
    [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
);
pub const ACPI_20_TABLE: Guid = Guid::new(
    0x8868_e871,
    0xe4f1,
    0x11d3,
    [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81],
);
pub const ACPI_10_TABLE: Guid = Guid::new(
    0xeb9d_2d30,
    0x2d88,
    0x11d3,
    [0x9a, 0x16, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d],
);
pub const DEVICE_TREE_TABLE: Guid = Guid::new(
    0xb1b6_21d5,
    0xf19c,
    0x41a5,
    [0x83, 0x0b, 0xd9, 0x15, 0x2c, 0x69, 0xaa, 0xe0],
);

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: unsafe extern "efiapi" fn(*mut SimpleTextOutput, bool) -> Status,
    pub output_string: unsafe extern "efiapi" fn(*mut SimpleTextOutput, *const u16) -> Status,
}

#[repr(C)]
pub struct ConfigurationTable {
    pub vendor_guid: Guid,
    pub vendor_table: *mut c_void,
}

#[repr(C)]
pub struct SystemTable {
    pub header: TableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub console_in: *mut c_void,
    pub console_out_handle: Handle,
    pub console_out: *mut SimpleTextOutput,
    pub standard_error_handle: Handle,
    pub standard_error: *mut SimpleTextOutput,
    pub runtime_services: *mut c_void,
    pub boot_services: *mut BootServices,
    pub configuration_table_entries: usize,
    pub configuration_table: *mut ConfigurationTable,
}

pub const ALLOCATE_ANY_PAGES: u32 = 0;
pub const ALLOCATE_MAX_ADDRESS: u32 = 1;
pub const ALLOCATE_ADDRESS: u32 = 2;

type Unused = usize;

#[repr(C)]
pub struct BootServices {
    pub header: TableHeader,
    pub raise_tpl: Unused,
    pub restore_tpl: Unused,
    pub allocate_pages:
        unsafe extern "efiapi" fn(u32, u32, usize, *mut PhysicalAddress) -> Status,
    pub free_pages: unsafe extern "efiapi" fn(PhysicalAddress, usize) -> Status,
    pub get_memory_map: unsafe extern "efiapi" fn(
        *mut usize,
        *mut u8,
        *mut usize,
        *mut usize,
        *mut u32,
    ) -> Status,
    pub allocate_pool: Unused,
    pub free_pool: Unused,
    pub create_event: Unused,
    pub set_timer: Unused,
    pub wait_for_event: Unused,
    pub signal_event: Unused,
    pub close_event: Unused,
    pub check_event: Unused,
    pub install_protocol_interface: Unused,
    pub reinstall_protocol_interface: Unused,
    pub uninstall_protocol_interface: Unused,
    pub handle_protocol: unsafe extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status,
    pub reserved: Unused,
    pub register_protocol_notify: Unused,
    pub locate_handle: Unused,
    pub locate_device_path: Unused,
    pub install_configuration_table: Unused,
    pub load_image: Unused,
    pub start_image: Unused,
    pub exit: Unused,
    pub unload_image: Unused,
    pub exit_boot_services: unsafe extern "efiapi" fn(Handle, usize) -> Status,
    pub get_next_monotonic_count: Unused,
    pub stall: unsafe extern "efiapi" fn(usize) -> Status,
    pub set_watchdog_timer: unsafe extern "efiapi" fn(usize, u64, usize, *const u16) -> Status,
    pub connect_controller: Unused,
    pub disconnect_controller: Unused,
    pub open_protocol: Unused,
    pub close_protocol: Unused,
    pub open_protocol_information: Unused,
    pub protocols_per_handle: Unused,
    pub locate_handle_buffer: Unused,
    pub locate_protocol:
        unsafe extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status,
}

#[repr(C)]
pub struct LoadedImage {
    pub revision: u32,
    pub parent_handle: Handle,
    pub system_table: *mut SystemTable,
    pub device_handle: Handle,
    pub file_path: *mut DevicePath,
    pub reserved: *mut c_void,
    pub load_options_size: u32,
    pub load_options: *mut c_void,
    pub image_base: *mut c_void,
    pub image_size: u64,
    pub image_code_type: u32,
    pub image_data_type: u32,
    pub unload: Unused,
}

#[repr(C)]
pub struct DevicePath {
    pub kind: u8,
    pub subtype: u8,
    pub length: [u8; 2],
}

pub const DEVICE_PATH_MEDIA: u8 = 4;
pub const DEVICE_PATH_MEDIA_HARD_DRIVE: u8 = 1;
pub const DEVICE_PATH_END: u8 = 0x7f;
pub const HARD_DRIVE_SIGNATURE_GUID: u8 = 2;
pub const HARD_DRIVE_SIGNATURE_OFFSET: usize = 24;
pub const HARD_DRIVE_SIGNATURE_TYPE_OFFSET: usize = 41;

#[repr(C)]
pub struct SimpleFileSystem {
    pub revision: u64,
    pub open_volume: unsafe extern "efiapi" fn(*mut SimpleFileSystem, *mut *mut File) -> Status,
}

pub const FILE_MODE_READ: u64 = 1;
pub const FILE_INFO_FILE_SIZE: usize = 8;

#[repr(C)]
pub struct File {
    pub revision: u64,
    pub open: unsafe extern "efiapi" fn(*mut File, *mut *mut File, *const u16, u64, u64) -> Status,
    pub close: unsafe extern "efiapi" fn(*mut File) -> Status,
    pub delete: Unused,
    pub read: unsafe extern "efiapi" fn(*mut File, *mut usize, *mut c_void) -> Status,
    pub write: Unused,
    pub get_position: Unused,
    pub set_position: Unused,
    pub get_info: unsafe extern "efiapi" fn(*mut File, *const Guid, *mut usize, *mut c_void) -> Status,
}

pub const PIXEL_RGB_RESERVED: u32 = 0;
pub const PIXEL_BGR_RESERVED: u32 = 1;
pub const PIXEL_BIT_MASK: u32 = 2;
pub const PIXEL_BLT_ONLY: u32 = 3;

#[repr(C)]
pub struct PixelBitmask {
    pub red: u32,
    pub green: u32,
    pub blue: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct GraphicsModeInformation {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    pub pixel_format: u32,
    pub pixel_information: PixelBitmask,
    pub pixels_per_scan_line: u32,
}

#[repr(C)]
pub struct GraphicsMode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *mut GraphicsModeInformation,
    pub size_of_info: usize,
    pub frame_buffer_base: PhysicalAddress,
    pub frame_buffer_size: usize,
}

#[repr(C)]
pub struct GraphicsOutput {
    pub query_mode: unsafe extern "efiapi" fn(
        *mut GraphicsOutput,
        u32,
        *mut usize,
        *mut *mut GraphicsModeInformation,
    ) -> Status,
    pub set_mode: unsafe extern "efiapi" fn(*mut GraphicsOutput, u32) -> Status,
    pub blt: Unused,
    pub mode: *mut GraphicsMode,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryDescriptor {
    pub kind: u32,
    pub padding: u32,
    pub physical_start: PhysicalAddress,
    pub virtual_start: u64,
    pub pages: u64,
    pub attribute: u64,
}

pub const PAGE_SIZE: u64 = 4096;
