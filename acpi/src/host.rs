//! What the interpreter asks of the machine: operation regions' reads and writes, time, and the
//! notifications AML sends.

use crate::name::Path;
use crate::object::{Object, PciFunction, RegionSpace};

/// Why the machine refused an access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    /// The machine does not reach that address space.
    UnsupportedSpace,
    /// The address lies outside what the machine lets AML reach.
    OutOfRange,
    /// The device behind the space did not answer.
    NoResponse,
}

/// The machine the interpreter runs on. Widths are in bits: 8, 16, 32 or 64.
pub trait Host {
    /// Reads `width` bits at `address` of `space` (system memory, system I/O, the embedded
    /// controller, CMOS, ...).
    fn read(&mut self, space: RegionSpace, address: u64, width: u32) -> Result<u64, HostError>;

    /// Writes `value`'s low `width` bits at `address` of `space`.
    fn write(
        &mut self,
        space: RegionSpace,
        address: u64,
        width: u32,
        value: u64,
    ) -> Result<(), HostError>;

    /// Reads `width` bits at `offset` of `function`'s configuration space.
    fn read_pci(&mut self, function: PciFunction, offset: u16, width: u32)
    -> Result<u64, HostError>;

    /// Writes `value`'s low `width` bits at `offset` of `function`'s configuration space.
    fn write_pci(
        &mut self,
        function: PciFunction,
        offset: u16,
        width: u32,
        value: u64,
    ) -> Result<(), HostError>;

    /// Lets other work run for at least `milliseconds` (`Sleep`).
    fn sleep(&mut self, milliseconds: u64);

    /// Waits `microseconds` without giving the CPU up (`Stall`).
    fn stall(&mut self, microseconds: u64);

    /// A clock in 100-nanosecond units that only goes forward (`Timer`).
    fn timer(&mut self) -> u64;

    /// AML told the operating system about the object at `path` (`Notify`).
    fn notify(&mut self, path: &Path, value: u64);

    /// AML stored `object` to `Debug`. Nothing by default.
    fn debug(&mut self, _object: &Object) {}

    /// The address and length of the loaded table with `signature`, `oem_id` and
    /// `oem_table_id`, for a `DataTableRegion`. None by default.
    fn table_place(
        &mut self,
        _signature: &[u8],
        _oem_id: &[u8],
        _oem_table_id: &[u8],
    ) -> Option<(u64, u64)> {
        None
    }
}
