//! Interrupts the kernel handles, described the same way on every architecture.
//!
//! Each architecture module acknowledges an interrupt at its interrupt controller, quiets the
//! source when needed (a fired one-shot timer), signals the end of the interrupt, and then
//! passes an [`Interrupt`] to the kernel's `kernel_interrupt` function; a device's interrupt
//! ([`Interrupt::ConsoleInput`], [`Interrupt::Device`]) is ended after the kernel has quieted the
//! device instead. When that returns, the interrupted code resumes. Spurious interrupts are dropped in the HAL; an
//! interrupt from a source the kernel has not asked for is reported as an
//! [`crate::Exception`] of kind [`crate::ExceptionKind::Interrupt`].

/// An interrupt delivered to the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    /// This CPU's timer reached the deadline set with [`crate::Platform::set_timer_deadline`].
    /// The timer is disarmed until the next `set_timer_deadline`.
    Timer,
    /// Another CPU called [`crate::Platform::wake_cpu`] for this one. It carries nothing: the
    /// kernel looks at its own shared state to see why (D41). Wakes a CPU waiting in
    /// [`crate::Platform::wait_for_interrupt`]. Wakes sent while one is pending may merge into
    /// one.
    Wakeup,
    /// The console's UART received bytes, once [`crate::Platform::enable_console_input`] turned
    /// its interrupt on. The kernel takes them with [`crate::Platform::console_read`] before it
    /// returns: the HAL ends the interrupt at its controller only then, so the UART stops
    /// asserting it first.
    ConsoleInput,
    /// The machine's power button was pressed, once [`crate::Platform::enable_power_button`]
    /// turned its interrupt on (D202).
    PowerButton,
    /// A device interrupt the kernel routed with [`crate::Platform::route_device_interrupt`] or
    /// [`crate::Platform::message_interrupt`], by the line the call gave. The kernel quiets the
    /// device before it returns (a level-triggered line stays asserted until then); the HAL
    /// ends the interrupt at its controller after.
    Device(DeviceLine),
}

/// One of the device interrupt lines the HAL hands out ([`crate::Platform::route_device_interrupt`],
/// [`crate::Platform::message_interrupt`]): a small index the kernel keeps its handler under,
/// whatever the controller's own numbering (a GIC interrupt id, a PLIC source, an x86 vector).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceLine(u8);

impl DeviceLine {
    /// How many lines the HAL hands out, on every architecture.
    pub const COUNT: usize = 32;

    /// Line `index`, if below [`Self::COUNT`].
    pub const fn from_index(index: usize) -> Option<Self> {
        if index < Self::COUNT {
            Some(Self(index as u8))
        } else {
            None
        }
    }

    /// The line's index, below [`Self::COUNT`].
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A message-signalled interrupt (PCI MSI-X, PCI Local Bus 3.0, 6.8.2): the device raises it by
/// writing `data` to `address`, which the platform's interrupt controller decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptMessage {
    pub address: u64,
    pub data: u32,
}

/// The device lines of an interrupt controller with numbered sources (a GIC's interrupt ids, a
/// PLIC's sources), handed out in order: line `n` is the `n`th source routed. Source 0 is never
/// one (GIC ids of devices start at 32, PLIC sources at 1), so 0 marks a free line.
#[cfg(any(
    test,
    all(
        target_os = "none",
        any(target_arch = "aarch64", target_arch = "riscv64")
    )
))]
#[derive(Debug)]
pub(crate) struct SourceLines {
    sources: [core::sync::atomic::AtomicU32; DeviceLine::COUNT],
}

#[cfg(any(
    test,
    all(
        target_os = "none",
        any(target_arch = "aarch64", target_arch = "riscv64")
    )
))]
impl SourceLines {
    pub(crate) const fn new() -> Self {
        Self {
            sources: [const { core::sync::atomic::AtomicU32::new(0) }; DeviceLine::COUNT],
        }
    }

    /// The line of `source`: the one it had if it was routed before, else the next free line;
    /// `None` when every line is taken, or for source 0. Says whether the line is new.
    pub(crate) fn claim(&self, source: u32) -> Option<(DeviceLine, bool)> {
        use core::sync::atomic::Ordering;
        if source == 0 {
            return None;
        }
        for (index, slot) in self.sources.iter().enumerate() {
            match slot.compare_exchange(0, source, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some((DeviceLine(index as u8), true)),
                Err(held) if held == source => return Some((DeviceLine(index as u8), false)),
                Err(_) => {}
            }
        }
        None
    }

    /// The line `source` was routed to, if it was. Lines are taken in order, so the search
    /// stops at the first free one.
    #[inline]
    pub(crate) fn line_of(&self, source: u32) -> Option<DeviceLine> {
        use core::sync::atomic::Ordering;
        for (index, slot) in self.sources.iter().enumerate() {
            match slot.load(Ordering::Acquire) {
                0 => return None,
                held if held == source => return Some(DeviceLine(index as u8)),
                _ => {}
            }
        }
        None
    }

    /// The source routed to `line`, if any.
    #[cfg(test)]
    pub(crate) fn source_of(&self, line: DeviceLine) -> Option<u32> {
        use core::sync::atomic::Ordering;
        let source = self.sources.get(line.index())?.load(Ordering::Acquire);
        (source != 0).then_some(source)
    }
}

#[cfg(target_os = "none")]
unsafe extern "Rust" {
    /// Defined by the kernel binary with exactly this signature (Rust ABI), which makes declaring
    /// it `safe` sound. Returns to resume the interrupted code.
    safe fn kernel_interrupt(interrupt: Interrupt);
}

/// Hands `interrupt` to the kernel.
#[cfg(target_os = "none")]
pub(crate) fn deliver(interrupt: Interrupt) {
    kernel_interrupt(interrupt);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_get_lines_in_order_and_keep_them() {
        let lines = SourceLines::new();
        assert_eq!(lines.claim(48), Some((DeviceLine(0), true)));
        assert_eq!(lines.claim(49), Some((DeviceLine(1), true)));
        assert_eq!(lines.claim(48), Some((DeviceLine(0), false)));
        assert_eq!(lines.line_of(49), Some(DeviceLine(1)));
        assert_eq!(lines.line_of(50), None);
        assert_eq!(lines.source_of(DeviceLine(1)), Some(49));
        assert_eq!(lines.source_of(DeviceLine(2)), None);
    }

    #[test]
    fn source_zero_and_a_full_table_get_no_line() {
        let lines = SourceLines::new();
        assert_eq!(lines.claim(0), None);
        for source in 1..=DeviceLine::COUNT as u32 {
            assert!(lines.claim(source).is_some());
        }
        assert_eq!(lines.claim(1000), None);
        assert_eq!(lines.line_of(1000), None);
        assert_eq!(
            lines.line_of(DeviceLine::COUNT as u32),
            DeviceLine::from_index(DeviceLine::COUNT - 1)
        );
    }
}
