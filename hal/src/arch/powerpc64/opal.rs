//! OPAL, skiboot's runtime firmware interface (D304): calls in hypervisor real mode, big-endian,
//! with the token in `r0`, the arguments in `r3` on, `r2` OPAL's base, and physical addresses
//! for every pointer. The call switches the CPU to that mode and back; what it passes by
//! pointer lives in this module's statics, which are in the kernel image, so their physical
//! address is their virtual one less the kernel's offset.

use super::KERNEL_VIRTUAL_OFFSET;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

unsafe extern "C" {
    /// Calls OPAL `token` with up to five arguments; its return value.
    fn ppc64_opal_call(token: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64;
    /// The same with up to seven arguments.
    #[link_name = "ppc64_opal_call"]
    fn ppc64_opal_call7(token: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64) -> i64;
}

/// OPAL's base and entry point, from `r8` and `r9` at boot.
pub(super) static OPAL_BASE: AtomicU64 = AtomicU64::new(0);
pub(super) static OPAL_ENTRY: AtomicU64 = AtomicU64::new(0);

const CONSOLE_WRITE: u64 = 1;
const CONSOLE_READ: u64 = 2;
const CEC_POWER_DOWN: u64 = 5;
const CEC_REBOOT: u64 = 6;
const POLL_EVENTS: u64 = 10;
const START_CPU: u64 = 41;
const SUCCESS: i64 = 0;
const BUSY_EVENT: i64 = -12;
const CONSOLE_TERMINAL: u64 = 0;
const BUFFER_BYTES: usize = 256;

/// The console's buffer and the length word OPAL reads and writes, big-endian.
static mut BUFFER: [u8; BUFFER_BYTES] = [0; BUFFER_BYTES];
static mut LENGTH: u64 = 0;
static CONSOLE_HELD: AtomicBool = AtomicBool::new(false);

fn physical<T>(pointer: *const T) -> u64 {
    (pointer as u64).wrapping_sub(KERNEL_VIRTUAL_OFFSET)
}

fn call(token: u64, arguments: [u64; 5]) -> i64 {
    if OPAL_ENTRY.load(Ordering::Relaxed) == 0 {
        return -1;
    }
    let [a, b, c, d, e] = arguments;
    // SAFETY: OPAL was handed to the kernel at boot and is reserved in the memory map; the call
    // runs it in the mode it expects and comes back with every register the ABI preserves.
    unsafe { ppc64_opal_call(token, a, b, c, d, e) }
}

/// OPAL's base, 0 before boot set it.
pub(super) fn base() -> u64 {
    OPAL_BASE.load(Ordering::Relaxed)
}

fn hold_console() {
    while CONSOLE_HELD
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

fn release_console() {
    CONSOLE_HELD.store(false, Ordering::Release);
}

/// Writes `bytes` to OPAL's console 0, polling OPAL while it is busy.
pub(super) fn console_write(bytes: &[u8]) {
    hold_console();
    for chunk in bytes.chunks(BUFFER_BYTES) {
        let mut written = 0;
        while written < chunk.len() {
            let rest = &chunk[written..];
            // SAFETY: the console lock is held, so only this CPU uses the buffer and the length.
            let (buffer, length) = unsafe {
                let buffer = &raw mut BUFFER;
                (*buffer)[..rest.len()].copy_from_slice(rest);
                let length = &raw mut LENGTH;
                *length = (rest.len() as u64).to_be();
                (buffer as *const u8, length as *const u64)
            };
            let status = call(CONSOLE_WRITE, [CONSOLE_TERMINAL, physical(length), physical(buffer), 0, 0]);
            // SAFETY: as above; OPAL wrote the count it took back.
            let taken = u64::from_be(unsafe { *(&raw const LENGTH) }) as usize;
            if taken > 0 {
                written += taken.min(rest.len());
            } else if status == SUCCESS || status == BUSY_EVENT {
                call(POLL_EVENTS, [0; 5]);
            } else {
                break;
            }
        }
    }
    release_console();
}

/// Reads what OPAL's console 0 has into `buffer`: how many bytes.
pub(super) fn console_read(buffer: &mut [u8]) -> usize {
    hold_console();
    let wanted = buffer.len().min(BUFFER_BYTES);
    // SAFETY: the console lock is held.
    let (source, length) = unsafe {
        let length = &raw mut LENGTH;
        *length = (wanted as u64).to_be();
        (&raw const BUFFER as *const u8, length as *const u64)
    };
    let status = call(CONSOLE_READ, [CONSOLE_TERMINAL, physical(length), physical(source), 0, 0]);
    // SAFETY: as above; OPAL wrote the count it read back.
    let read = match status {
        SUCCESS => (u64::from_be(unsafe { *(&raw const LENGTH) }) as usize).min(wanted),
        _ => 0,
    };
    // SAFETY: as above.
    buffer[..read].copy_from_slice(unsafe { &(*(&raw const BUFFER))[..read] });
    release_console();
    read
}

const PCI_CONFIG_READ_WORD: u64 = 15;
const PCI_CONFIG_WRITE_WORD: u64 = 18;
const PCI_SET_PE: u64 = 31;
const PCI_SET_MVE: u64 = 33;
const PCI_SET_MVE_ENABLE: u64 = 34;
const PCI_SET_XIVE_PE: u64 = 37;
const GET_MSI_64: u64 = 40;
const PCI_MAP_PE_DMA_WINDOW_REAL: u64 = 45;
const XIVE_RESET: u64 = 128;
const XIVE_GET_IRQ_INFO: u64 = 129;
const XIVE_SET_IRQ_CONFIG: u64 = 131;
const XIVE_SET_QUEUE_INFO: u64 = 133;
const XIVE_MODE_EXPLOITATION: u64 = 1;
const MAP_PE: u64 = 1;

/// The words OPAL writes results into, big-endian, behind the console's lock.
static mut RESULTS: [u64; 6] = [0; 6];

fn with_results<R>(body: impl FnOnce(&[u64; 6], [u64; 6]) -> R) -> R {
    hold_console();
    // SAFETY: the console's lock is held, so only this CPU uses the result words.
    let results = unsafe { &mut *(&raw mut RESULTS) };
    *results = [0; 6];
    let addresses: [u64; 6] = core::array::from_fn(|index| physical(&raw const results[index]));
    let value = body(results, addresses);
    release_console();
    value
}

/// The 32-bit configuration word of function `rid` of PHB `phb` at `offset`.
pub(super) fn pci_config_read32(phb: u64, rid: u64, offset: u64) -> Option<u32> {
    with_results(|results, addresses| {
        let status = call(PCI_CONFIG_READ_WORD, [phb, rid, offset, addresses[0], 0]);
        // SAFETY: OPAL wrote the word into the first result.
        let word = unsafe { core::ptr::read_volatile(&raw const results[0] as *const u32) };
        (status == SUCCESS).then(|| u32::from_be(word))
    })
}

/// Writes the 32-bit configuration word of function `rid` of PHB `phb` at `offset`.
pub(super) fn pci_config_write32(phb: u64, rid: u64, offset: u64, value: u32) {
    call(PCI_CONFIG_WRITE_WORD, [phb, rid, offset, u64::from(value), 0]);
}

/// Puts every function of PHB `phb` in PE `pe`; whether OPAL took it.
pub(super) fn pci_set_pe_all(phb: u64, pe: u64) -> bool {
    hold_console();
    let status = call_seven(PCI_SET_PE, [phb, pe, 0, 0, 0, 0, MAP_PE]);
    release_console();
    status == SUCCESS
}

/// Gives PE `pe` of PHB `phb` its MSI validation entry, enabled.
pub(super) fn pci_set_mve(phb: u64, pe: u64) {
    call(PCI_SET_MVE, [phb, pe, pe, 0, 0]);
    call(PCI_SET_MVE_ENABLE, [phb, pe, 1, 0, 0]);
}

/// Sets PE `pe`'s second DMA window to bypass translation from bus address `base` for `size`
/// bytes; whether OPAL took it.
pub(super) fn pci_map_bypass(phb: u64, pe: u64, base: u64, size: u64) -> bool {
    call(PCI_MAP_PE_DMA_WINDOW_REAL, [phb, pe, (pe << 1) | 1, base, size]) == SUCCESS
}

/// Binds MSI `index` of PHB `phb` to PE `pe`.
pub(super) fn pci_set_xive_pe(phb: u64, pe: u64, index: u64) {
    call(PCI_SET_XIVE_PE, [phb, pe, index, 0, 0]);
}

/// The 64-bit MSI address and data of MSI `index` of PHB `phb` in PE `pe`.
pub(super) fn get_msi_64(phb: u64, pe: u64, index: u64) -> Option<(u64, u32)> {
    with_results(|results, addresses| {
        let status = call_seven(GET_MSI_64, [phb, pe, index, 1, addresses[0], addresses[1], 0]);
        // SAFETY: OPAL wrote the address and the data into the first two results.
        let (address, data) = unsafe {
            (
                core::ptr::read_volatile(&raw const results[0]),
                core::ptr::read_volatile(&raw const results[1] as *const u32),
            )
        };
        (status == SUCCESS).then(|| (u64::from_be(address), u32::from_be(data)))
    })
}

/// Resets XIVE into exploitation mode; whether OPAL took it.
pub(super) fn xive_reset() -> bool {
    call(XIVE_RESET, [XIVE_MODE_EXPLOITATION, 0, 0, 0, 0]) == SUCCESS
}

/// Gives virtual processor `vp`'s priority `priority` the event queue at physical `page`, of
/// `2^order` bytes, with `flags`.
pub(super) fn xive_set_queue(vp: u64, priority: u64, page: u64, order: u64, flags: u64) -> bool {
    call(XIVE_SET_QUEUE_INFO, [vp, priority, page, order, flags]) == SUCCESS
}

/// A source's ESB page, its size's shift and its flags.
pub(super) struct IrqInfo {
    pub(super) flags: u64,
    pub(super) eoi_page: u64,
    pub(super) esb_shift: u64,
}

/// What OPAL says of XIVE source `source`.
pub(super) fn xive_irq_info(source: u64) -> Option<IrqInfo> {
    with_results(|results, addresses| {
        let status = call_seven(
            XIVE_GET_IRQ_INFO,
            [source, addresses[0], addresses[1], addresses[2], addresses[3], addresses[4], 0],
        );
        // SAFETY: OPAL wrote the flags, the EOI and trigger pages and the shift into the
        // first four results.
        let read = |index: usize| u64::from_be(unsafe { core::ptr::read_volatile(&raw const results[index]) });
        let shift = u32::from_be(unsafe { core::ptr::read_volatile(&raw const results[3] as *const u32) });
        (status == SUCCESS).then(|| IrqInfo {
            flags: read(0),
            eoi_page: read(1),
            esb_shift: u64::from(shift),
        })
    })
}

/// Routes XIVE source `source` to virtual processor `vp` at `priority`, its queue data `data`.
pub(super) fn xive_set_irq(source: u64, vp: u64, priority: u64, data: u64) -> bool {
    call(XIVE_SET_IRQ_CONFIG, [source, vp, priority, data, 0]) == SUCCESS
}

fn call_seven(token: u64, arguments: [u64; 7]) -> i64 {
    if OPAL_ENTRY.load(Ordering::Relaxed) == 0 {
        return -1;
    }
    let [a, b, c, d, e, f, g] = arguments;
    // SAFETY: as in `call`; the seven-argument form passes `f` and `g` in r8 and r9.
    unsafe { ppc64_opal_call7(token, a, b, c, d, e, f, g) }
}

/// Asks OPAL to power the machine off.
pub(super) fn power_down() {
    call(CEC_POWER_DOWN, [0; 5]);
}

/// Asks OPAL to reset the machine.
pub(super) fn reboot() {
    call(CEC_REBOOT, [0; 5]);
}

/// Asks OPAL to start the thread whose server number is `cpu` at physical `entry`; whether it
/// took the request.
pub(super) fn start_cpu(cpu: u64, entry: u64) -> bool {
    call(START_CPU, [cpu, entry, 0, 0, 0]) == SUCCESS
}
