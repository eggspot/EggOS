//! Device interrupts (issue #97's slice 3, D184): the handler of each device line the HAL hands
//! out ([`DeviceLine`]), and [`DeviceEvents`], what a driver's thread waits on instead of
//! polling.
//!
//! A line's handler runs in the interrupt, which the kernel takes only at a preemption point, in
//! idle or in user mode (D37), so the CPU holds no lock: it quiets the device
//! ([`Handler::acknowledge`], so a level-triggered line drops before the HAL ends the interrupt)
//! and raises the device's events, which wakes the thread waiting on them. Everything else
//! happens in that thread.
//!
//! Lines are routed to the boot CPU as devices are found; once the other CPUs are online,
//! [`spread`] hands the connected lines out to the online CPUs in turn, and every line
//! connected after goes to the next CPU (D536). A line at an interrupt controller moves through
//! the HAL; a message-signalled one by writing the HAL's new message into its device's MSI-X
//! entry ([`MessageTable`]). A line that cannot move stays where it is.

use eggos_hal::atomic::AtomicU64;
use crate::thread::Parker;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use eggos_drivers::io::Mmio;
use eggos_drivers::pci::msix::MsixTable;
use eggos_hal::{DeviceLine, Platform};

/// What a device line's interrupt does.
#[derive(Debug, Clone, Copy)]
pub struct Handler {
    /// Quiets the device, given [`Self::context`]: reads and acknowledges a virtio-mmio slot's
    /// interrupt status, nothing for a message-signalled interrupt. Must take no lock.
    pub acknowledge: fn(usize),
    /// What [`Self::acknowledge`] is given: the slot's registers' address.
    pub context: usize,
    /// Raised after the device is quiet.
    pub events: &'static DeviceEvents,
}

/// Where a message-signalled line's device keeps its message: entry `entry` of an MSI-X table
/// of `entries` entries, `bytes` long, mapped as device memory for good at `address` (D536,
/// D540).
#[derive(Debug, Clone, Copy)]
pub struct MessageTable {
    pub address: usize,
    pub bytes: usize,
    pub entries: u16,
    pub entry: u16,
}

static HANDLERS: [AtomicPtr<Handler>; DeviceLine::COUNT] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; DeviceLine::COUNT];

static MESSAGE_TABLES: [AtomicPtr<MessageTable>; DeviceLine::COUNT] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; DeviceLine::COUNT];

static SPREADING: AtomicBool = AtomicBool::new(false);

static NEXT_CPU: AtomicUsize = AtomicUsize::new(1);

/// Makes `handler` the handler of `line`, for good. Refused when the line has one already. The
/// line stays where the HAL routed it until [`follow`].
pub fn connect(line: DeviceLine, handler: Handler) -> Result<(), &'static str> {
    connect_with(line, handler, None)
}

/// [`connect`] for a message-signalled line whose device keeps its message in `table`, so the
/// line can move to another CPU (D536).
pub fn connect_message(
    line: DeviceLine,
    handler: Handler,
    table: MessageTable,
) -> Result<(), &'static str> {
    connect_with(line, handler, Some(table))
}

fn connect_with(
    line: DeviceLine,
    handler: Handler,
    table: Option<MessageTable>,
) -> Result<(), &'static str> {
    let slot = HANDLERS
        .get(line.index())
        .ok_or("no such device interrupt line")?;
    let handler = Box::into_raw(Box::new(handler));
    let taken = slot
        .compare_exchange(core::ptr::null_mut(), handler, Ordering::AcqRel, Ordering::Acquire)
        .is_err();
    if taken {
        // SAFETY: made just above by `Box::into_raw` and never shared.
        drop(unsafe { Box::from_raw(handler) });
        return Err("the device interrupt line is in use");
    }
    let table = table.map_or(core::ptr::null_mut(), |table| Box::into_raw(Box::new(table)));
    MESSAGE_TABLES[line.index()].store(table, Ordering::Release);
    Ok(())
}

/// Sends `line`, just connected, to the next CPU in turn once [`spread`] has run; nothing
/// before, when [`spread`] moves it with the others (D536).
pub fn follow<P: Platform>(line: DeviceLine) {
    if SPREADING.load(Ordering::Acquire) {
        steer_to_next_cpu::<P>(line);
    }
}

/// Disconnects `line` and gives it back to the HAL (D413): when a driver lets its device go, or
/// connecting failed after the line was routed. The handler's few words stay allocated, so an
/// interrupt already in flight on another CPU never reads freed memory.
pub fn disconnect<P: Platform>(line: DeviceLine) {
    if let Some(slot) = HANDLERS.get(line.index()) {
        slot.store(core::ptr::null_mut(), Ordering::Release);
    }
    if let Some(slot) = MESSAGE_TABLES.get(line.index()) {
        slot.store(core::ptr::null_mut(), Ordering::Release);
    }
    P::release_device_interrupt(line);
}

/// Hands every connected line out to the online CPUs in turn, starting with the first after
/// the boot CPU, and has every line connected from now on follow (D536). Called once the other
/// CPUs are online; nothing moves on a machine with one.
pub fn spread<P: Platform>() {
    if SPREADING.swap(true, Ordering::AcqRel) {
        return;
    }
    for index in 0..DeviceLine::COUNT {
        let connected = !HANDLERS[index].load(Ordering::Acquire).is_null();
        if connected && let Some(line) = DeviceLine::from_index(index) {
            steer_to_next_cpu::<P>(line);
        }
    }
}

fn steer_to_next_cpu<P: Platform>(line: DeviceLine) {
    let cpus = crate::cpu::CPUS.all();
    let online = cpus.iter().filter(|cpu| cpu.is_online()).count();
    if online < 2 {
        return;
    }
    let turn = NEXT_CPU.fetch_add(1, Ordering::Relaxed) % online;
    if let Some(cpu) = cpus.iter().filter(|cpu| cpu.is_online()).nth(turn) {
        steer::<P>(line, cpu.hardware_id());
    }
}

/// Moves `line` to the CPU with hardware id `cpu`: at the HAL, or by its device's new message.
/// Its events are raised once after a move, so the driver looks at the device for an interrupt
/// the move may have lost. Whether it moved.
pub fn steer<P: Platform>(line: DeviceLine, cpu: u64) -> bool {
    let table = MESSAGE_TABLES[line.index()].load(Ordering::Acquire);
    // SAFETY: a stored table is a leaked box, never changed or freed.
    let moved = match unsafe { table.as_ref() } {
        Some(table) => P::message_for(line, cpu).is_ok_and(|message| {
            // SAFETY: the table is a function's MSI-X table, mapped as device memory for good
            // by the driver that connected the line, which writes this line's entry only when
            // it sets the line up, before connecting it.
            let registers = unsafe { Mmio::new(table.address, table.bytes) };
            let entries = MsixTable::new(registers, table.entries);
            entries.set(table.entry, message.address, message.data)
        }),
        None => P::steer_device_interrupt(line, cpu).is_ok(),
    };
    let handler = HANDLERS[line.index()].load(Ordering::Acquire);
    // SAFETY: a connected handler is a leaked box, never changed or freed.
    if moved && let Some(handler) = unsafe { handler.as_ref() } {
        handler.events.raise::<P>();
    }
    moved
}

/// Handles the interrupt of `line`, which this CPU took: its handler quiets the device and
/// raises its events. A line with no handler is ignored (the HAL ends it all the same).
pub fn handle<P: Platform>(line: DeviceLine) {
    let Some(slot) = HANDLERS.get(line.index()) else {
        return;
    };
    let handler = slot.load(Ordering::Acquire);
    if handler.is_null() {
        return;
    }
    // SAFETY: a connected handler is a leaked box, never changed or freed.
    let handler = unsafe { &*handler };
    (handler.acknowledge)(handler.context);
    handler.events.raise::<P>();
}

/// Events a device's interrupts raise, and the one thread that waits for them: a count of the
/// raises, which the waiter compares with the one it saw, and the waiter's parker. Costs nothing
/// while nobody waits but the count's increment.
#[derive(Debug)]
pub struct DeviceEvents {
    raised: AtomicU64,
    waiter: AtomicPtr<Parker>,
    wakes_input_waiters: AtomicBool,
    masked_line: AtomicUsize,
}

impl Default for DeviceEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceEvents {
    pub const fn new() -> Self {
        Self {
            raised: AtomicU64::new(0),
            waiter: AtomicPtr::new(core::ptr::null_mut()),
            wakes_input_waiters: AtomicBool::new(false),
            masked_line: AtomicUsize::new(0),
        }
    }

    /// Says the events come from `line`, a level-triggered input the HAL masks when it fires
    /// (a PCI function's INTx, D413): each wait unmasks it again, the driver having looked.
    pub fn set_masked_line(&self, line: DeviceLine) {
        self.masked_line.store(line.index() + 1, Ordering::Release);
    }

    /// Events whose raise also wakes the threads waiting for console or device input
    /// ([`crate::console::wake_input_waiters`]): for a device programs wait on through `read`
    /// and `poll` rather than through a driver's thread, such as the agent channel (D220).
    pub const fn waking_input_waiters() -> Self {
        Self {
            wakes_input_waiters: AtomicBool::new(true),
            ..Self::new()
        }
    }

    /// Has every later raise also wake the threads waiting on the console's input key, as
    /// [`Self::waking_input_waiters`] does: for a device threads wait on through the input key
    /// besides its one waiter, such as the GPU's fence waiters (D460).
    pub fn wake_input_waiters_too(&self) {
        self.wakes_input_waiters.store(true, Ordering::Release);
    }

    /// Says a thread that does not wait through [`Self::wait`] looked at the device: unmasks
    /// the masked line, as each wait does.
    pub fn looked_at<P: Platform>(&self) {
        let masked = self.masked_line.load(Ordering::Acquire);
        if let Some(line) = masked.checked_sub(1).and_then(DeviceLine::from_index) {
            P::unmask_device_interrupt(line);
        }
    }

    /// How many times the events were raised: what a waiter reads before it looks at the device,
    /// and passes to [`Self::wait`].
    pub fn raised(&self) -> u64 {
        self.raised.load(Ordering::SeqCst)
    }

    /// Counts a raise and wakes the waiter, if one waits. From an interrupt handler or a thread.
    pub fn raise<P: Platform>(&self) {
        self.raised.fetch_add(1, Ordering::SeqCst);
        let waiter = self.waiter.swap(core::ptr::null_mut(), Ordering::SeqCst);
        if !waiter.is_null() {
            // SAFETY: the waiter's parker stays in place until it is unparked: `wait` takes it
            // back itself, or waits for this unpark when it finds it taken.
            unsafe { Parker::unpark::<P>(waiter) };
        }
        if self.wakes_input_waiters.load(Ordering::Acquire) {
            crate::console::wake_input_waiters::<P>();
        }
    }

    /// Waits until the events were raised since the count `seen` ([`Self::raised`]) or the clock
    /// reaches `deadline` ([`Platform::clock_ticks`]; `u64::MAX` for none), and returns the
    /// count then. One thread waits at a time. Call it as [`crate::thread::yield_now`] is called:
    /// with interrupts masked and no lock held.
    pub fn wait<P: Platform>(&self, seen: u64, deadline: u64) -> u64 {
        let masked = self.masked_line.load(Ordering::Acquire);
        if let Some(line) = masked.checked_sub(1).and_then(DeviceLine::from_index) {
            P::unmask_device_interrupt(line);
        }
        let parker = Parker::new();
        self.waiter
            .store(core::ptr::from_ref(&parker).cast_mut(), Ordering::SeqCst);
        if self.raised.load(Ordering::SeqCst) == seen {
            parker.park_until::<P>(deadline);
        }
        if self
            .waiter
            .swap(core::ptr::null_mut(), Ordering::SeqCst)
            .is_null()
        {
            parker.park_until::<P>(u64::MAX);
        }
        self.raised()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TestPlatform as P;
    use core::sync::atomic::AtomicUsize;

    static ACKNOWLEDGED: AtomicUsize = AtomicUsize::new(0);
    static EVENTS: DeviceEvents = DeviceEvents::new();

    fn count_acknowledge(context: usize) {
        ACKNOWLEDGED.fetch_add(context, Ordering::Relaxed);
    }

    #[test]
    fn a_connected_line_acknowledges_its_device_then_raises_its_events() {
        let line = DeviceLine::from_index(DeviceLine::COUNT - 1).unwrap();
        let handler = Handler {
            acknowledge: count_acknowledge,
            context: 1,
            events: &EVENTS,
        };
        connect(line, handler).unwrap();
        assert!(connect(line, handler).is_err());
        let before = EVENTS.raised();
        handle::<P>(line);
        assert_eq!(ACKNOWLEDGED.load(Ordering::Relaxed), 1);
        assert_eq!(EVENTS.raised(), before + 1);
        assert_eq!(EVENTS.wait::<P>(before, u64::MAX), before + 1);
    }

    #[test]
    fn a_line_the_machine_cannot_move_stays_and_raises_nothing() {
        static STAYING: DeviceEvents = DeviceEvents::new();
        let line = DeviceLine::from_index(DeviceLine::COUNT - 3).unwrap();
        let handler = Handler {
            acknowledge: count_acknowledge,
            context: 0,
            events: &STAYING,
        };
        let table = MessageTable {
            address: 0,
            bytes: 0,
            entries: 0,
            entry: 0,
        };
        connect_message(line, handler, table).unwrap();
        assert!(!steer::<P>(line, 1));
        assert_eq!(STAYING.raised(), 0);
    }

    #[test]
    fn a_line_without_a_handler_is_ignored() {
        handle::<P>(DeviceLine::from_index(DeviceLine::COUNT - 2).unwrap());
    }
}
