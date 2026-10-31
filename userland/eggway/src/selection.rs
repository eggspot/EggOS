//! Copy and paste between Wayland clients (D176): `wl_data_device_manager` (the clipboard) and
//! `zwp_primary_selection_device_manager_v1` (the primary selection, what is selected, pasted
//! with the middle button). A client offers its data as a source with the MIME types it can
//! give and sets it as the selection; the client with the keyboard focus is offered it, with
//! every type, before its focus comes and whenever it changes; a client that reads it asks for
//! one type with the write end of a pipe, which goes to the source's client, who writes the data
//! into it and closes it. The compositor never reads the data itself. Drag and drop is not
//! served yet: a drag's source is cancelled at once.

use crate::client::{ClientSlot, MAX_OUT_DESCRIPTORS};
use crate::compositor::{Message, Shared, each_of, flush, new_object, object_index};
use crate::system::{Fd, System};
use eggos_wayland::protocol::{
    Event, Interface, data_device, data_offer, data_source, display, primary_selection_device,
    primary_selection_offer, primary_selection_source,
};
use eggos_wayland::wire::Value;

/// The most MIME types one source offers; the ones past it are not offered.
pub const MAX_MIME_TYPES: usize = 12;
/// The longest MIME type kept, in bytes; a longer one is not offered.
pub const MIME_TYPE_CAPACITY: usize = 64;
/// Sources one client holds at once.
pub const MAX_SOURCES: usize = 8;
/// Deliveries to other clients waiting at once; a paste that finds no room gets no data.
pub const MAX_DELIVERIES: usize = 8;

/// Which selection: the clipboard (`wl_data_device`) or the primary selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Clipboard,
    Primary,
}

impl Kind {
    /// The interface of this selection's sources.
    pub const fn source_interface(self) -> Interface {
        match self {
            Self::Clipboard => Interface::DataSource,
            Self::Primary => Interface::PrimarySelectionSource,
        }
    }

    const fn device_interface(self) -> Interface {
        match self {
            Self::Clipboard => Interface::DataDevice,
            Self::Primary => Interface::PrimarySelectionDevice,
        }
    }

    const fn offer_interface(self) -> Interface {
        match self {
            Self::Clipboard => Interface::DataOffer,
            Self::Primary => Interface::PrimarySelectionOffer,
        }
    }

    const fn data_offer_event(self) -> Event {
        match self {
            Self::Clipboard => data_device::DATA_OFFER,
            Self::Primary => primary_selection_device::DATA_OFFER,
        }
    }

    const fn selection_event(self) -> Event {
        match self {
            Self::Clipboard => data_device::SELECTION,
            Self::Primary => primary_selection_device::SELECTION,
        }
    }

    const fn offer_event(self) -> Event {
        match self {
            Self::Clipboard => data_offer::OFFER,
            Self::Primary => primary_selection_offer::OFFER,
        }
    }

    const fn send_event(self) -> Event {
        match self {
            Self::Clipboard => data_source::SEND,
            Self::Primary => primary_selection_source::SEND,
        }
    }

    const fn cancelled_event(self) -> Event {
        match self {
            Self::Clipboard => data_source::CANCELLED,
            Self::Primary => primary_selection_source::CANCELLED,
        }
    }
}

/// One MIME type, such as `text/plain;charset=utf-8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MimeType {
    bytes: [u8; MIME_TYPE_CAPACITY],
    length: u8,
}

impl MimeType {
    /// `text` as a MIME type; `None` when it is empty or longer than [`MIME_TYPE_CAPACITY`].
    pub fn new(text: &[u8]) -> Option<Self> {
        if text.is_empty() || text.len() > MIME_TYPE_CAPACITY {
            return None;
        }
        let mut bytes = [0; MIME_TYPE_CAPACITY];
        bytes[..text.len()].copy_from_slice(text);
        Some(Self {
            bytes,
            length: text.len() as u8,
        })
    }

    /// Its bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

/// The MIME types a source offers, in the order it offered them, each once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MimeTypes {
    types: [Option<MimeType>; MAX_MIME_TYPES],
    count: usize,
}

impl MimeTypes {
    /// None.
    pub const fn new() -> Self {
        Self {
            types: [None; MAX_MIME_TYPES],
            count: 0,
        }
    }

    /// Adds `text`, unless it is there already, too long, or there is no room.
    pub fn add(&mut self, text: &[u8]) {
        if self.iter().any(|known| known.as_bytes() == text) || self.count == MAX_MIME_TYPES {
            return;
        }
        if let Some(mime_type) = MimeType::new(text) {
            self.types[self.count] = Some(mime_type);
            self.count += 1;
        }
    }

    /// Every type, in order.
    pub fn iter(&self) -> impl Iterator<Item = &MimeType> {
        self.types[..self.count].iter().flatten()
    }
}

impl Default for MimeTypes {
    fn default() -> Self {
        Self::new()
    }
}

/// A source a client made, by the id of its `wl_data_source` or
/// `zwp_primary_selection_source_v1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataSource {
    pub id: u32,
    pub kind: Kind,
    pub mime_types: MimeTypes,
    /// It was set as a selection, or a drag started with it: it may not be set again.
    pub used: bool,
}

/// What a selection is now: whose source, what it offers, and its generation, which the
/// offers made for it carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// The index of the source's client.
    pub owner: u8,
    pub source: u32,
    pub mime_types: MimeTypes,
    pub generation: u16,
}

/// Work for a client other than the one whose request made it, done once that request is
/// handled ([`crate::compositor::Compositor`] delivers it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The source is no longer the selection.
    Cancel { owner: u8, source: u32, kind: Kind },
    /// A client asked for the data as `mime_type`: the source's client writes it into
    /// `descriptor`, a pipe's write end.
    Send {
        owner: u8,
        source: u32,
        kind: Kind,
        mime_type: MimeType,
        descriptor: Fd,
    },
    /// The selection changed: offer it to the client with the keyboard focus.
    Announce { kind: Kind },
}

/// Both selections and the deliveries waiting.
#[derive(Debug, Default)]
pub struct Selections {
    pub clipboard: Option<Selection>,
    pub primary: Option<Selection>,
    generation: u16,
    deliveries: [Option<Delivery>; MAX_DELIVERIES],
}

impl Selections {
    /// The selection `kind`.
    pub fn of(&self, kind: Kind) -> Option<Selection> {
        match kind {
            Kind::Clipboard => self.clipboard,
            Kind::Primary => self.primary,
        }
    }

    fn slot(&mut self, kind: Kind) -> &mut Option<Selection> {
        match kind {
            Kind::Clipboard => &mut self.clipboard,
            Kind::Primary => &mut self.primary,
        }
    }

    fn next_generation(&mut self) -> u16 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Queues `delivery`; gives it back when the queue is full.
    pub fn queue(&mut self, delivery: Delivery) -> Result<(), Delivery> {
        match self.deliveries.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => {
                *slot = Some(delivery);
                Ok(())
            }
            None => Err(delivery),
        }
    }

    /// The oldest delivery waiting.
    pub fn take_delivery(&mut self) -> Option<Delivery> {
        let first = self.deliveries[0].take();
        self.deliveries.rotate_left(1);
        first
    }

    fn queue_or_drop<S: System>(&mut self, system: &mut S, delivery: Delivery) {
        if let Err(Delivery::Send { descriptor, .. }) = self.queue(delivery) {
            system.close(descriptor);
        }
    }

    fn announce_change(&mut self, kind: Kind) {
        let waiting = Delivery::Announce { kind };
        if !self.deliveries.contains(&Some(waiting)) {
            let _ = self.queue(waiting);
        }
    }

    /// Client `owner` went: its selections end, and what waits for it is dropped, its
    /// descriptors closed.
    pub fn forget_client<S: System>(&mut self, system: &mut S, owner: u8) {
        for kind in [Kind::Clipboard, Kind::Primary] {
            if self
                .of(kind)
                .is_some_and(|selection| selection.owner == owner)
            {
                *self.slot(kind) = None;
                self.announce_change(kind);
            }
        }
        for slot in self.deliveries.iter_mut() {
            match *slot {
                Some(Delivery::Send {
                    owner: waiting,
                    descriptor,
                    ..
                }) if waiting == owner => {
                    system.close(descriptor);
                    *slot = None;
                }
                Some(Delivery::Cancel { owner: waiting, .. }) if waiting == owner => *slot = None,
                _ => {}
            }
        }
        let mut kept = [None; MAX_DELIVERIES];
        for (to, delivery) in kept.iter_mut().zip(self.deliveries.iter().flatten()) {
            *to = Some(*delivery);
        }
        self.deliveries = kept;
    }
}

fn kind_of(interface: Interface) -> Kind {
    match interface {
        Interface::PrimarySelectionDeviceManager
        | Interface::PrimarySelectionDevice
        | Interface::PrimarySelectionOffer
        | Interface::PrimarySelectionSource => Kind::Primary,
        _ => Kind::Clipboard,
    }
}

/// `wl_data_device_manager`'s and `zwp_primary_selection_device_manager_v1`'s requests: a new
/// source, or a device for the seat (offered the selection at once if its client has the
/// keyboard focus).
pub(crate) fn manager_request(client: &mut ClientSlot, shared: &mut Shared, message: &Message) {
    let kind = kind_of(message.interface);
    let id = message.uint(0);
    match message.opcode {
        0 => {
            let Some(free) = client.sources.iter().position(Option::is_none) else {
                client.protocol_error(id, display::NO_MEMORY, b"too many sources");
                return;
            };
            if new_object(
                client,
                id,
                kind.source_interface(),
                message.version,
                free as u16,
            ) {
                client.sources[free] = Some(DataSource {
                    id,
                    kind,
                    mime_types: MimeTypes::new(),
                    used: false,
                });
            }
        }
        1 => {
            if object_index(client, message.uint(1), Interface::Seat).is_none()
                || !new_object(client, id, kind.device_interface(), message.version, 0)
            {
                return;
            }
            if shared.keyboard_client == Some(client.index) {
                let selection = shared.selections.of(kind);
                offer_to(client, kind, selection, id, message.version);
            }
        }
        _ => {}
    }
}

/// A source's requests: `offer` adds a MIME type; `destroy` ends the selection it is.
pub(crate) fn source_request(client: &mut ClientSlot, shared: &mut Shared, message: &Message) {
    let index = usize::from(message.index);
    let Some(source) = client.sources.get(index).copied().flatten() else {
        return;
    };
    if message.opcode == 1 {
        client.sources[index] = None;
        let kind = source.kind;
        let current = shared.selections.of(kind);
        if current.is_some_and(|selection| {
            selection.owner == client.index && selection.source == source.id
        }) {
            *shared.selections.slot(kind) = None;
            shared.selections.announce_change(kind);
        }
        return;
    }
    if message.opcode == 0
        && let Some(Some(source)) = client.sources.get_mut(index)
    {
        source.mime_types.add(message.values[0].bytes());
    }
}

/// A device's requests: `set_selection` makes a source (or none) the selection; a drag's
/// source is cancelled at once, as drag and drop is not served yet.
pub(crate) fn device_request<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    message: &Message,
) {
    let kind = kind_of(message.interface);
    let (set_selection, start_drag) = match kind {
        Kind::Clipboard => (1, Some(0)),
        Kind::Primary => (0, None),
    };
    if Some(message.opcode) == start_drag {
        let source = message.uint(0);
        if source != 0
            && let Some(index) = take_source(client, message.object, source, kind)
        {
            let version = client.object(source).map_or(1, |object| object.version);
            client.post(source, version, kind.cancelled_event(), &[]);
            if let Some(Some(taken)) = client.sources.get_mut(usize::from(index)) {
                taken.used = true;
            }
        }
        return;
    }
    if message.opcode != set_selection {
        return;
    }
    let source = message.uint(0);
    let chosen = match source {
        0 => None,
        id => {
            let Some(index) = take_source(client, message.object, id, kind) else {
                return;
            };
            let slot = &mut client.sources[usize::from(index)];
            if let Some(taken) = slot.as_mut() {
                taken.used = true;
            }
            *slot
        }
    };
    let generation = shared.selections.next_generation();
    let owner = client.index;
    let new = chosen.map(|source| Selection {
        owner,
        source: source.id,
        mime_types: source.mime_types,
        generation,
    });
    let old = core::mem::replace(shared.selections.slot(kind), new);
    if let Some(old) = old {
        shared.selections.queue_or_drop(
            system,
            Delivery::Cancel {
                owner: old.owner,
                source: old.source,
                kind,
            },
        );
    }
    shared.selections.announce_change(kind);
}

fn take_source(client: &mut ClientSlot, device: u32, source: u32, kind: Kind) -> Option<u16> {
    let index = object_index(client, source, kind.source_interface())?;
    match client.sources.get(usize::from(index)).copied().flatten() {
        Some(found) if !found.used => Some(index),
        _ => {
            client.protocol_error(device, data_device::USED_SOURCE, b"source already used");
            None
        }
    }
}

/// An offer's requests: `receive` passes the pipe to the source's client when the offer is
/// still the selection's (else the pipe is closed, so the reader gets nothing); `finish` is a
/// drag's, an error on a selection's offer; `accept` and `set_actions` are a drag's and do
/// nothing here.
pub(crate) fn offer_request<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    message: &Message,
) {
    let kind = kind_of(message.interface);
    let receive = match kind {
        Kind::Clipboard => 1,
        Kind::Primary => 0,
    };
    if kind == Kind::Clipboard && message.opcode == 3 {
        client.protocol_error(
            message.object,
            data_offer::INVALID_FINISH,
            b"not a drag's offer",
        );
        return;
    }
    if message.opcode != receive {
        return;
    }
    let descriptor = message.descriptors[0];
    let current = shared
        .selections
        .of(kind)
        .filter(|selection| selection.generation == message.index);
    let mime_type = MimeType::new(message.values[0].bytes());
    let (Some(selection), Some(mime_type)) = (current, mime_type) else {
        system.close(descriptor);
        return;
    };
    shared.selections.queue_or_drop(
        system,
        Delivery::Send {
            owner: selection.owner,
            source: selection.source,
            kind,
            mime_type,
            descriptor,
        },
    );
}

/// Offers the selection `kind` (or its absence) to every device of that kind `client` has,
/// as it gets the keyboard focus or the selection changes while it has it.
pub fn announce(client: &mut ClientSlot, selections: &Selections, kind: Kind) {
    let selection = selections.of(kind);
    each_of(
        client,
        kind.device_interface(),
        |client, device, version| {
            offer_to(client, kind, selection, device, version);
        },
    );
}

fn offer_to(
    client: &mut ClientSlot,
    kind: Kind,
    selection: Option<Selection>,
    device: u32,
    version: u32,
) {
    let offer = selection.and_then(|selection| {
        client.create_server_object(kind.offer_interface(), version, selection.generation)
    });
    let (Some(selection), Some(offer)) = (selection, offer) else {
        client.post(device, version, kind.selection_event(), &[Value::Object(0)]);
        return;
    };
    client.post(
        device,
        version,
        kind.data_offer_event(),
        &[Value::NewId(offer)],
    );
    for mime_type in selection.mime_types.iter() {
        client.post(
            offer,
            version,
            kind.offer_event(),
            &[Value::String(mime_type.as_bytes())],
        );
    }
    client.post(
        device,
        version,
        kind.selection_event(),
        &[Value::Object(offer)],
    );
}

/// Tells the source's client its source is no longer the selection.
pub fn cancel(client: &mut ClientSlot, source: u32, kind: Kind) {
    let Some(object) = client
        .object(source)
        .filter(|object| object.interface == Some(kind.source_interface()))
    else {
        return;
    };
    client.post(source, object.version, kind.cancelled_event(), &[]);
}

/// Asks the source's client to write its data as `mime_type` into `descriptor`, which goes
/// with the event; the descriptor is closed instead when the source is gone or the client's
/// descriptors are not read.
pub fn send<S: System>(
    client: &mut ClientSlot,
    system: &mut S,
    source: u32,
    kind: Kind,
    mime_type: &MimeType,
    descriptor: Fd,
) {
    let object = client
        .object(source)
        .filter(|object| object.interface == Some(kind.source_interface()));
    if client.descriptors_out_count == MAX_OUT_DESCRIPTORS {
        flush(client, system);
    }
    let Some(object) = object.filter(|_| client.descriptors_out_count < MAX_OUT_DESCRIPTORS) else {
        system.close(descriptor);
        return;
    };
    client.descriptors_out[client.descriptors_out_count] = descriptor;
    client.descriptors_out_count += 1;
    client.post(
        source,
        object.version,
        kind.send_event(),
        &[Value::String(mime_type.as_bytes()), Value::Fd],
    );
}
