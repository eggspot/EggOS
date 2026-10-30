//! Device interrupts (issue #97's slice 3, D184): the handler of each device line the HAL hands
//! out ([`DeviceLine`]), and [`DeviceEvents`], what a driver's thread waits on instead of
//! polling.
//!
//! A line's handler runs in the interrupt, which the kernel takes only at a preemption point, in
//! idle or in user mode (D37), so the CPU holds no lock: it quiets the device
//! ([`Handler::acknowledge`], so a level-triggered line drops before the HAL ends the interrupt)
//! and raises the device's events, which wakes the thread waiting on them. Everything else
//! happens in that thread.

use crate::thread::Parker;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
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

static HANDLERS: [AtomicPtr<Handler>; DeviceLine::COUNT] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; DeviceLine::COUNT];

/// Makes `handler` the handler of `line`, for good. Refused when the line has one already.
pub fn connect(line: DeviceLine, handler: Handler) -> Result<(), &'static str> {
    let slot = HANDLERS
        .get(line.index())
        .ok_or("no such device interrupt line")?;
    let handler = Box::into_raw(Box::new(handler));
    match slot.compare_exchange(
        core::ptr::null_mut(),
        handler,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Ok(()),
        Err(_) => {
            // SAFETY: made just above by `Box::into_raw` and never shared.
            drop(unsafe { Box::from_raw(handler) });
            Err("the device interrupt line is in use")
        }
    }
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
    }

    /// Waits until the events were raised since the count `seen` ([`Self::raised`]) or the clock
    /// reaches `deadline` ([`Platform::clock_ticks`]; `u64::MAX` for none), and returns the
    /// count then. One thread waits at a time. Call it as [`crate::thread::yield_now`] is called:
    /// with interrupts masked and no lock held.
    pub fn wait<P: Platform>(&self, seen: u64, deadline: u64) -> u64 {
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
    fn a_line_without_a_handler_is_ignored() {
        handle::<P>(DeviceLine::from_index(DeviceLine::COUNT - 2).unwrap());
    }
}
