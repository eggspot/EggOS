//! What the interpreter asks of the machine: operation regions' reads and writes, time, and the
//! notifications AML sends.

use alloc::vec::Vec;
use crate::name::Path;
use crate::interpreter::Machine;
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
    fn read_pci(
        &mut self,
        function: PciFunction,
        offset: u16,
        width: u32,
    ) -> Result<u64, HostError>;

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

    /// The AML thread running now, which owns the mutexes it acquires (D595). One thread, 0,
    /// by default.
    fn thread_id(&mut self) -> u64 {
        0
    }

    /// Lends `machine` to the other AML threads (methods the machine runs at the same time,
    /// such as an event's while another call waits) until `until` holds of it or `timeout`
    /// milliseconds pass (0xffff: for ever), then gives it back with whether `until` held
    /// (D595). By default there are no other threads: it waits out the timeout (unless it is
    /// for ever) and looks once more.
    fn wait_for(
        &mut self,
        machine: Machine,
        until: &dyn Fn(&Machine) -> bool,
        timeout: u64,
    ) -> (Machine, bool) {
        if timeout < 0xffff {
            self.sleep(timeout);
        }
        let held = until(&machine);
        (machine, held)
    }

    /// Reads `pins` pins from the `offset`th pin on of the GPIO connection `connection` (a
    /// resource template holding a `GpioIo` or `GpioInt` descriptor), lowest pin in bit 0
    /// (D595). Refused by default.
    fn gpio_read(&mut self, _connection: &[u8], _offset: u32, _pins: u32) -> Result<u64, HostError> {
        Err(HostError::UnsupportedSpace)
    }

    /// Writes `value`'s low `pins` bits to the pins of `connection` from the `offset`th on
    /// (D595). Refused by default.
    fn gpio_write(
        &mut self,
        _connection: &[u8],
        _offset: u32,
        _pins: u32,
        _value: u64,
    ) -> Result<(), HostError> {
        Err(HostError::UnsupportedSpace)
    }

    /// One transfer with the serial-bus device of `connection` (an `I2cSerialBusV2`,
    /// `SpiSerialBusV2` or `UartSerialBusV2` descriptor): `command` is the field's byte offset
    /// in its region, `protocol` the `AccessAs` attribute and `length` its byte count, `write`
    /// the bytes to send. Returns the data buffer the protocol gives back: a status byte, a
    /// length byte, then the data (ACPI 6.5, 5.5.2.4.5.3). Refused by default (D595).
    fn serial_bus(
        &mut self,
        _connection: &[u8],
        _command: u64,
        _protocol: u8,
        _length: u8,
        _write: Option<&[u8]>,
    ) -> Result<Vec<u8>, HostError> {
        Err(HostError::UnsupportedSpace)
    }

    /// AML stored `object` to `Debug`. Nothing by default.
    fn debug(&mut self, _object: &Object) {}

    /// The bytes of the firmware's table with `signature`, `oem_id` and `oem_table_id` (blank
    /// IDs match any), for `LoadTable` (D556). None by default.
    fn table_bytes(
        &mut self,
        _signature: &[u8],
        _oem_id: &[u8],
        _oem_table_id: &[u8],
    ) -> Option<Vec<u8>> {
        None
    }

    /// Takes the firmware's global lock (the FACS's lock word, ACPI 6.5, 5.2.10.1), waiting up to
    /// `timeout` milliseconds (0xffff for ever) while the firmware holds it; whether it did (D556).
    /// Granted at once by default, for a machine with no FACS.
    fn acquire_global_lock(&mut self, _timeout: u64) -> bool {
        true
    }

    /// Gives the firmware's global lock back, telling the firmware if it waits for it (D556).
    /// Nothing by default.
    fn release_global_lock(&mut self) {}

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
