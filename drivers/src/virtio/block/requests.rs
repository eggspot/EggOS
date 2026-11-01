//! Which of the block driver's requests are in flight (D219): each has a header and a status
//! byte of its own, by its number, and a run of whole data slots of the driver's DMA memory,
//! so up to [`MAX_IN_FLIGHT`] requests are posted at once and complete in any order.

/// The most requests in flight at once.
pub const MAX_IN_FLIGHT: usize = 15;
/// Data slots of the driver's DMA memory, each [`SLOT_BYTES`] long.
pub const DATA_SLOTS: usize = 15;
/// The bytes of one data slot: an EggFS block.
pub const SLOT_BYTES: usize = 4096;

const _: () = assert!(DATA_SLOTS <= u16::BITS as usize);

/// One request in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InFlight {
    /// The first descriptor of its chain, as the queue's used ring names it.
    pub head: u16,
    /// Its first data slot.
    pub first_slot: usize,
    /// How many data slots it holds (0 for a flush).
    pub slots: usize,
    /// Whether the device answered it.
    pub answered: bool,
}

/// The requests in flight and the data slots they hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTable {
    requests: [Option<InFlight>; MAX_IN_FLIGHT],
    free_slots: u16,
}

impl Default for RequestTable {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestTable {
    /// No request in flight, every data slot free.
    pub const fn new() -> Self {
        Self {
            requests: [None; MAX_IN_FLIGHT],
            free_slots: ((1u32 << DATA_SLOTS) - 1) as u16,
        }
    }

    /// The data slots `bytes` take.
    pub const fn slots_for(bytes: usize) -> usize {
        bytes.div_ceil(SLOT_BYTES)
    }

    /// Reserves a request number and a run of `slots` free data slots in a row, the lowest
    /// that fits: the request's number and its first slot, or `None` while none fits.
    pub fn reserve(&mut self, slots: usize) -> Option<(usize, usize)> {
        let request = self.requests.iter().position(Option::is_none)?;
        let first_slot = match slots {
            0 => 0,
            _ => (0..=DATA_SLOTS.checked_sub(slots)?).find(|&first| {
                let run = run_mask(first, slots);
                self.free_slots & run == run
            })?,
        };
        self.free_slots &= !run_mask(first_slot, slots);
        self.requests[request] = Some(InFlight {
            head: u16::MAX,
            first_slot,
            slots,
            answered: false,
        });
        Some((request, first_slot))
    }

    /// Records that `request` was posted as the chain starting at `head`.
    pub fn posted(&mut self, request: usize, head: u16) {
        if let Some(Some(in_flight)) = self.requests.get_mut(request) {
            in_flight.head = head;
        }
    }

    /// Marks the request whose chain starts at `head` answered: its number, or `None` for a
    /// chain that is not one of them.
    pub fn answered(&mut self, head: u16) -> Option<usize> {
        let request = self.requests.iter().position(|in_flight| {
            in_flight.is_some_and(|in_flight| in_flight.head == head && !in_flight.answered)
        })?;
        if let Some(in_flight) = &mut self.requests[request] {
            in_flight.answered = true;
        }
        Some(request)
    }

    /// Request `request`, if in flight.
    pub fn get(&self, request: usize) -> Option<InFlight> {
        self.requests.get(request).copied().flatten()
    }

    /// Frees `request` and its data slots.
    pub fn release(&mut self, request: usize) {
        if let Some(slot) = self.requests.get_mut(request)
            && let Some(in_flight) = slot.take()
        {
            self.free_slots |= run_mask(in_flight.first_slot, in_flight.slots);
        }
    }

    /// How many requests are in flight, answered or not.
    pub fn in_flight(&self) -> usize {
        self.requests.iter().filter(|slot| slot.is_some()).count()
    }

    /// Whether every data slot is free.
    pub fn all_slots_free(&self) -> bool {
        self.free_slots == ((1u32 << DATA_SLOTS) - 1) as u16
    }
}

fn run_mask(first: usize, slots: usize) -> u16 {
    match slots {
        0 => 0,
        _ => ((((1u32 << slots) - 1) << first) & 0xffff) as u16,
    }
}
