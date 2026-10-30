//! Input (D113): the keyboards and pointers drivers install ([`install`], or
//! [`install_interrupting`] for one whose interrupts raise [`DEVICE_EVENTS`], D184). A keyboard's
//! keys become bytes through the US keymap
//! ([`eggos_drivers::input::keymap`]) and enter the console's line discipline as the UART's do
//! ([`console::receive_from_keyboard`]); every key, pointer motion, button and wheel notch also
//! goes into the input event queue ([`next_event`]) once a reader subscribed ([`subscribe`]),
//! and the scrollback keys and the wheel move the graphical terminal's view
//! ([`crate::terminal`]).
//!
//! Nothing runs without an input device. With them, the release kernel's input thread
//! ([`start`]) takes their events: when every device interrupts, it sleeps until one does or the
//! held key's next repeat ([`REPEAT_DELAY_MS`], then every [`REPEAT_INTERVAL_MS`]), so an idle
//! machine never wakes it; otherwise it polls every [`ACTIVE_POLL_MS`] while a key or button is
//! held and for [`ACTIVE_WINDOW_MS`] after the last event, every [`IDLE_POLL_MS`] otherwise, and
//! at each repeat, an idle device costing one load of its used ring's index per poll.

use crate::console;
use crate::devices::interrupts::DeviceEvents;
use crate::sync::LeafSpinLock;
use crate::terminal::{self, ViewMove};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use eggos_drivers::input::key;
use eggos_drivers::input::keymap::{self, CursorKeys, KeyBytes, KeyChange, KeyRepeat, Keyboard};
use eggos_drivers::input::{
    Capabilities, RawEvent, SYN_REPORT, absolute, button, event_type, relative, scale_to_screen,
};
use eggos_drivers::terminal::WHEEL_LINES;
use eggos_hal::Platform;
use eggos_hal::time::mul_div_ceil;

pub use eggos_drivers::input::keymap::Modifiers;

/// Input devices the kernel takes, at most.
pub const MAX_DEVICES: usize = 4;
/// Events the queue holds; more are dropped and counted ([`dropped_events`]).
pub const EVENT_QUEUE_CAPACITY: usize = 256;
/// How long a key is held before it repeats.
pub const REPEAT_DELAY_MS: u64 = 400;
/// How often a held key repeats.
pub const REPEAT_INTERVAL_MS: u64 = 40;
/// The poll interval while a key or button is held, or an event came lately: USB HID's 125 Hz.
pub const ACTIVE_POLL_MS: u64 = 8;
/// The poll interval otherwise.
pub const IDLE_POLL_MS: u64 = 32;
/// How long after the last event polling stays at [`ACTIVE_POLL_MS`].
pub const ACTIVE_WINDOW_MS: u64 = 1000;

/// How many rounds one [`poll`] runs at most, so a device that never runs dry cannot keep the
/// poller from sleeping: 128 events from each device.
pub const MAX_POLL_ROUNDS: usize = 8;

const POLL_BUDGET_BYTES: usize = 128;

/// What a driver lends the kernel: an input device it polls.
pub trait InputDevice: Send {
    /// What the device reports.
    fn capabilities(&self) -> Capabilities;
    /// Hands `event` the device's new events, oldest first, at most `budget`; returns how many
    /// it took from the device.
    fn poll(&mut self, budget: usize, event: &mut dyn FnMut(RawEvent)) -> usize;
}

/// A pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

const BUTTONS: [(Button, u16); 3] = [
    (Button::Left, button::LEFT),
    (Button::Right, button::RIGHT),
    (Button::Middle, button::MIDDLE),
];

/// What happened on an input device, as the desktop reads it ([`next_event`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// Key `code` (a Linux input key code) was pressed or released; `modifiers` are those in
    /// effect after it.
    Key {
        code: u16,
        pressed: bool,
        modifiers: Modifiers,
    },
    /// The pointer moved to (`x`, `y`) on the display, in pixels from its top-left corner.
    PointerMove { x: u32, y: u32 },
    /// A pointer button was pressed or released.
    Button { button: Button, pressed: bool },
    /// The wheel turned by `delta` notches, up when positive.
    Wheel { delta: i32 },
}

const KEY: u64 = 1;
const POINTER_MOVE: u64 = 2;
const BUTTON: u64 = 3;
const WHEEL: u64 = 4;
const COORDINATE_MASK: u32 = 0x00ff_ffff;

const fn field(word: u64, shift: u32, mask: u64) -> u64 {
    (word >> shift) & mask
}

impl InputEvent {
    /// The event in one word, for the lock-free queue.
    pub const fn pack(self) -> u64 {
        match self {
            InputEvent::Key {
                code,
                pressed,
                modifiers,
            } => {
                KEY | (code as u64) << 8 | (pressed as u64) << 24 | (modifiers.bits() as u64) << 32
            }
            InputEvent::PointerMove { x, y } => {
                let x = if x > COORDINATE_MASK {
                    COORDINATE_MASK
                } else {
                    x
                };
                let y = if y > COORDINATE_MASK {
                    COORDINATE_MASK
                } else {
                    y
                };
                POINTER_MOVE | (x as u64) << 8 | (y as u64) << 32
            }
            InputEvent::Button { button, pressed } => {
                BUTTON | (button as u64) << 8 | (pressed as u64) << 16
            }
            InputEvent::Wheel { delta } => WHEEL | (delta as u32 as u64) << 32,
        }
    }

    /// The event [`Self::pack`] made `word` from; `None` for a word it never makes.
    pub const fn unpack(word: u64) -> Option<Self> {
        Some(match word & 0xff {
            KEY => InputEvent::Key {
                code: field(word, 8, 0xffff) as u16,
                pressed: field(word, 24, 1) != 0,
                modifiers: Modifiers::from_bits(field(word, 32, 0xff) as u8),
            },
            POINTER_MOVE => InputEvent::PointerMove {
                x: field(word, 8, COORDINATE_MASK as u64) as u32,
                y: field(word, 32, COORDINATE_MASK as u64) as u32,
            },
            BUTTON => InputEvent::Button {
                button: match field(word, 8, 0xff) {
                    0 => Button::Left,
                    1 => Button::Right,
                    2 => Button::Middle,
                    _ => return None,
                },
                pressed: field(word, 16, 1) != 0,
            },
            WHEEL => InputEvent::Wheel {
                delta: field(word, 32, 0xffff_ffff) as u32 as i32,
            },
            _ => return None,
        })
    }
}

/// A bounded queue of input events: one producer at a time (the input poller), any number of
/// consumers, lock-free, allocating nothing. A full queue drops its oldest event to take a new
/// one and counts it. `N` must be a power of two.
#[derive(Debug)]
pub struct EventQueue<const N: usize> {
    slots: [AtomicU64; N],
    head: AtomicU64,
    tail: AtomicU64,
    dropped: AtomicU64,
}

impl<const N: usize> Default for EventQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> EventQueue<N> {
    const POWER_OF_TWO: () = assert!(N.is_power_of_two());

    /// An empty queue.
    pub const fn new() -> Self {
        let () = Self::POWER_OF_TWO;
        Self {
            slots: [const { AtomicU64::new(0) }; N],
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Adds `event` at the back; `false` when the queue was full, so its oldest event was dropped
    /// and counted to make room. Only one producer may push at a time.
    pub fn push(&self, event: InputEvent) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let mut head = self.head.load(Ordering::Acquire);
        let mut kept_all = true;
        while tail.wrapping_sub(head) >= N as u64 {
            match self.head.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    kept_all = false;
                    break;
                }
                Err(current) => head = current,
            }
        }
        self.slots[tail as usize & (N - 1)].store(event.pack(), Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        kept_all
    }

    /// Takes the event at the front, if any.
    pub fn pop(&self) -> Option<InputEvent> {
        let mut head = self.head.load(Ordering::Acquire);
        loop {
            if head == self.tail.load(Ordering::Acquire) {
                return None;
            }
            let word = self.slots[head as usize & (N - 1)].load(Ordering::Relaxed);
            match self.head.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return InputEvent::unpack(word),
                Err(current) => head = current,
            }
        }
    }

    /// How many events wait.
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        self.tail.load(Ordering::Acquire).wrapping_sub(head) as usize
    }

    /// Whether no event waits.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many old events were dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// The kernel's input event queue.
pub type KernelEventQueue = EventQueue<EVENT_QUEUE_CAPACITY>;

static EVENTS: AtomicPtr<KernelEventQueue> = AtomicPtr::new(core::ptr::null_mut());

fn events() -> Option<&'static KernelEventQueue> {
    let queue = EVENTS.load(Ordering::Acquire);
    // SAFETY: `EVENTS` is null or a leaked `Box` that `install` stored once, with release
    // ordering after making it, and that nothing frees.
    unsafe { queue.as_ref() }
}

static SUBSCRIBED: AtomicBool = AtomicBool::new(false);

static PERSON_HOLDS_INPUT: AtomicBool = AtomicBool::new(false);

/// Whether a key or a pointer button is held on any input device, as the last poll saw it: a
/// person is using the machine, so injected input waits (D141). Reads one atomic, so callers
/// holding the screen's or the display's lock never wait for the devices' lock.
pub fn person_holds_input() -> bool {
    PERSON_HOLDS_INPUT.load(Ordering::Relaxed)
}

/// Whether input events wait in the queue for the desktop to take them.
pub fn has_pending_events() -> bool {
    subscribed_events().is_some_and(|queue| !queue.is_empty())
}

/// Has the input layer keep its events for [`next_event`] from now on. Until a reader
/// subscribes none are kept, so no keystroke waits in memory for a reader that does not exist.
pub fn subscribe() {
    SUBSCRIBED.store(true, Ordering::Release);
}

fn subscribed_events() -> Option<&'static KernelEventQueue> {
    events().filter(|_| SUBSCRIBED.load(Ordering::Acquire))
}

/// The oldest input event not read yet, if any.
pub fn next_event() -> Option<InputEvent> {
    events()?.pop()
}

/// How many input events were dropped because nobody read them in time.
pub fn dropped_events() -> u64 {
    events().map_or(0, EventQueue::dropped)
}

/// What a device reported since the end of its last report, applied at the end of the next.
#[derive(Debug, Default)]
pub struct PendingReport {
    dx: i32,
    dy: i32,
    wheel: i32,
    x: Option<i32>,
    y: Option<i32>,
    presses: u8,
    releases: u8,
    pending: bool,
}

struct Source {
    device: Box<dyn InputDevice>,
    capabilities: Capabilities,
    report: PendingReport,
    interrupting: bool,
}

/// What the input layer keeps across polls, apart from its devices: the keyboard, the key
/// repeat, the pointer, and when the last event came.
#[derive(Debug)]
pub struct InputState {
    keyboard: Keyboard,
    repeat: KeyRepeat,
    cursor_keys: CursorKeys,
    pointer: (u32, u32),
    buttons_held: u8,
    last_event: u64,
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

/// A clock's ticks for the times the input layer keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// Now.
    pub now: u64,
    /// [`REPEAT_DELAY_MS`] in ticks.
    pub repeat_delay: u64,
    /// [`REPEAT_INTERVAL_MS`] in ticks.
    pub repeat_interval: u64,
}

impl Timing {
    /// The timing on platform `P`'s clock, now.
    pub fn now<P: Platform>() -> Self {
        let frequency = P::clock_frequency();
        Self {
            now: P::clock_ticks(),
            repeat_delay: mul_div_ceil(REPEAT_DELAY_MS, frequency, 1000),
            repeat_interval: mul_div_ceil(REPEAT_INTERVAL_MS, frequency, 1000),
        }
    }
}

/// Where translated input goes: bytes for the console, events for the queue, moves of the
/// terminal's scrollback view.
pub trait InputSink {
    /// Bytes a key sent.
    fn bytes(&mut self, bytes: &[u8]);
    /// How many more bytes [`Self::bytes`] takes whole.
    fn room(&self) -> usize;
    /// An event for the queue.
    fn event(&mut self, event: InputEvent);
    /// A move of the terminal's scrollback view: Shift with Page Up, Page Down, Home or End,
    /// or the wheel.
    fn view(&mut self, change: ViewMove);
}

#[cold]
#[inline(never)]
fn move_view_with_key(code: u16, modifiers: Modifiers, sink: &mut dyn InputSink) {
    if !modifiers.contains(Modifiers::SHIFT) {
        return;
    }
    let change = match code {
        key::PAGE_UP => ViewMove::PageBack,
        key::PAGE_DOWN => ViewMove::PageForward,
        key::HOME => ViewMove::Oldest,
        key::END => ViewMove::Live,
        _ => return,
    };
    sink.view(change);
}

impl InputState {
    /// No key or button held, the pointer at the top-left corner.
    pub const fn new() -> Self {
        Self {
            keyboard: Keyboard::new(),
            repeat: KeyRepeat::new(),
            cursor_keys: CursorKeys::Normal,
            pointer: (0, 0),
            buttons_held: 0,
            last_event: 0,
        }
    }

    /// Whether a key or a pointer button is held.
    pub fn anything_held(&self) -> bool {
        self.buttons_held != 0 || self.keyboard.any_held()
    }

    /// The tick of the last event.
    pub fn last_event(&self) -> u64 {
        self.last_event
    }

    /// When the held key repeats next, if one does.
    pub fn next_repeat(&self) -> Option<u64> {
        self.repeat.next()
    }

    /// Has the arrow keys send `mode`'s forms, as the terminal's program asked.
    pub fn set_cursor_keys(&mut self, mode: CursorKeys) {
        self.cursor_keys = mode;
    }

    fn key(
        &mut self,
        event: RawEvent,
        report: &mut PendingReport,
        timing: Timing,
        sink: &mut dyn InputSink,
    ) {
        if let Some(index) = BUTTONS.iter().position(|(_, code)| *code == event.code) {
            let bit = 1 << index;
            let held = self.buttons_held | report.presses;
            report.pending = true;
            match event.value {
                1 if report.releases & bit != 0 => report.releases &= !bit,
                1 if held & bit == 0 => report.presses |= bit,
                0 if held & bit != 0 => report.releases |= bit,
                _ => {}
            }
            return;
        }
        if event.code >= button::FIRST {
            return;
        }
        let Some(change) = self.keyboard.key(event.code, event.value) else {
            return;
        };
        let modifiers = self.keyboard.modifiers();
        let pressed = change == KeyChange::Pressed;
        if pressed && terminal::takes_from_terminal(event.code, modifiers) {
            self.repeat
                .pressed(event.code, false, timing.now, timing.repeat_delay);
        } else if pressed {
            let bytes = match self.cursor_keys {
                CursorKeys::Normal => keymap::encode(event.code, modifiers, CursorKeys::Normal),
                CursorKeys::Application => {
                    keymap::encode(event.code, modifiers, CursorKeys::Application)
                }
            };
            if bytes.is_empty() {
                move_view_with_key(event.code, modifiers, sink);
            }
            sink.bytes(bytes.as_bytes());
            self.repeat.pressed(
                event.code,
                !bytes.is_empty(),
                timing.now,
                timing.repeat_delay,
            );
        } else {
            self.repeat.released(event.code);
        }
        sink.event(InputEvent::Key {
            code: event.code,
            pressed,
            modifiers,
        });
    }

    fn end_report(
        &mut self,
        capabilities: &Capabilities,
        report: &mut PendingReport,
        screen: Option<(u32, u32)>,
        sink: &mut dyn InputSink,
    ) {
        let report = core::mem::take(report);
        if let Some((width, height)) = screen.filter(|&(width, height)| width > 0 && height > 0) {
            let (mut x, mut y) = self.pointer;
            if let Some((x_range, y_range)) = capabilities.absolute {
                if let Some(value) = report.x {
                    x = scale_to_screen(value, x_range, width);
                }
                if let Some(value) = report.y {
                    y = scale_to_screen(value, y_range, height);
                }
            }
            let moved =
                |at: u32, delta: i32, size: u32| at.saturating_add_signed(delta).min(size - 1);
            x = moved(x, report.dx, width);
            y = moved(y, report.dy, height);
            if (x, y) != self.pointer {
                self.pointer = (x, y);
                sink.event(InputEvent::PointerMove { x, y });
            }
        }
        for (index, (button, _)) in BUTTONS.iter().enumerate() {
            let bit = 1 << index;
            if report.presses & bit != 0 {
                self.buttons_held |= bit;
                sink.event(InputEvent::Button {
                    button: *button,
                    pressed: true,
                });
            }
            if report.releases & bit != 0 {
                self.buttons_held &= !bit;
                sink.event(InputEvent::Button {
                    button: *button,
                    pressed: false,
                });
            }
        }
        if report.wheel != 0 {
            let lines = report.wheel.unsigned_abs() as usize * WHEEL_LINES;
            sink.view(if report.wheel > 0 {
                ViewMove::Back(lines)
            } else {
                ViewMove::Forward(lines)
            });
            sink.event(InputEvent::Wheel {
                delta: report.wheel,
            });
        }
    }

    /// Translates `event` from a device with `capabilities` whose report so far is `report`,
    /// on a `screen` of that size if there is one, into bytes and queued events for `sink`.
    pub fn translate(
        &mut self,
        event: RawEvent,
        capabilities: &Capabilities,
        report: &mut PendingReport,
        screen: Option<(u32, u32)>,
        timing: Timing,
        sink: &mut dyn InputSink,
    ) {
        self.last_event = timing.now;
        match (event.kind, event.code) {
            (event_type::KEY, _) => self.key(event, report, timing, sink),
            (event_type::REL, relative::X) => {
                report.dx = report.dx.saturating_add(event.value);
                report.pending = true;
            }
            (event_type::REL, relative::Y) => {
                report.dy = report.dy.saturating_add(event.value);
                report.pending = true;
            }
            (event_type::REL, relative::WHEEL) => {
                report.wheel = report.wheel.saturating_add(event.value);
                report.pending = true;
            }
            (event_type::ABS, absolute::X) => {
                report.x = Some(event.value);
                report.pending = true;
            }
            (event_type::ABS, absolute::Y) => {
                report.y = Some(event.value);
                report.pending = true;
            }
            (event_type::SYN, SYN_REPORT) if report.pending => {
                self.end_report(capabilities, report, screen, sink)
            }
            _ => {}
        }
    }

    /// Sends the held key's bytes again if its repeat is due and `sink` has room for them
    /// whole; without room the repeat stays due for the next poll.
    pub fn repeat_key(&mut self, timing: Timing, sink: &mut dyn InputSink) {
        if sink.room() < KeyBytes::CAPACITY {
            return;
        }
        if let Some(code) = self.repeat.due(timing.now, timing.repeat_interval) {
            let modifiers = self.keyboard.modifiers();
            sink.bytes(keymap::encode(code, modifiers, self.cursor_keys).as_bytes());
        }
    }
}

struct Input {
    sources: [Option<Source>; MAX_DEVICES],
    state: InputState,
}

static INPUT: LeafSpinLock<Option<Box<Input>>> = LeafSpinLock::new_leaf(None);

/// What the input devices' interrupts raise: the input thread waits on it ([`start`]).
pub static DEVICE_EVENTS: DeviceEvents = DeviceEvents::new();

/// Makes `device`, which the input thread polls, one of the kernel's input devices. Refused, and
/// handed back, when [`MAX_DEVICES`] are installed already.
pub fn install(device: Box<dyn InputDevice>) -> Result<(), Box<dyn InputDevice>> {
    install_source(device, false)
}

/// Makes `device`, whose interrupts raise [`DEVICE_EVENTS`], one of the kernel's input devices.
/// Refused, and handed back, when [`MAX_DEVICES`] are installed already.
pub fn install_interrupting(device: Box<dyn InputDevice>) -> Result<(), Box<dyn InputDevice>> {
    install_source(device, true)
}

fn install_source(
    device: Box<dyn InputDevice>,
    interrupting: bool,
) -> Result<(), Box<dyn InputDevice>> {
    let capabilities = device.capabilities();
    let fresh = INPUT.lock().is_none().then(|| {
        Box::new(Input {
            sources: [const { None }; MAX_DEVICES],
            state: InputState::new(),
        })
    });
    let queue = events()
        .is_none()
        .then(|| Box::into_raw(Box::new(KernelEventQueue::new())));
    let mut input = INPUT.lock();
    if input.is_none() {
        *input = fresh;
    }
    if let Some(queue) = queue {
        if EVENTS.load(Ordering::Relaxed).is_null() {
            EVENTS.store(queue, Ordering::Release);
        } else {
            // SAFETY: made just above by `Box::into_raw` and never shared.
            drop(unsafe { Box::from_raw(queue) });
        }
    }
    let Some(slot) = input
        .as_mut()
        .and_then(|input| input.sources.iter_mut().find(|slot| slot.is_none()))
    else {
        return Err(device);
    };
    *slot = Some(Source {
        device,
        capabilities,
        report: PendingReport::default(),
        interrupting,
    });
    Ok(())
}

/// Whether [`install`] would take another device.
pub fn has_room() -> bool {
    INPUT
        .lock()
        .as_ref()
        .is_none_or(|input| input.sources.iter().any(Option::is_none))
}

/// The input devices the kernel has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    /// Devices with keyboard keys.
    pub keyboards: usize,
    /// Devices that move a pointer.
    pub pointers: usize,
}

/// What input devices the kernel has.
pub fn summary() -> Summary {
    let input = INPUT.lock();
    let sources = input
        .iter()
        .flat_map(|input| input.sources.iter().flatten());
    sources.fold(Summary::default(), |summary, source| Summary {
        keyboards: summary.keyboards + usize::from(source.capabilities.keyboard),
        pointers: summary.pointers + usize::from(source.capabilities.is_pointer()),
    })
}

static SCREEN: AtomicU64 = AtomicU64::new(0);

fn screen() -> Option<(u32, u32)> {
    let known = SCREEN.load(Ordering::Relaxed);
    if known != 0 {
        return Some(((known >> 32) as u32, known as u32));
    }
    let mode = crate::display::mode()?;
    SCREEN.store(
        u64::from(mode.width) << 32 | u64::from(mode.height),
        Ordering::Relaxed,
    );
    Some((mode.width, mode.height))
}

const POLL_VIEW_MOVES: usize = 4;

struct PollSink {
    bytes: [u8; POLL_BUDGET_BYTES],
    length: usize,
    queue: Option<&'static KernelEventQueue>,
    views: [Option<ViewMove>; POLL_VIEW_MOVES],
    queued: bool,
}

impl InputSink for PollSink {
    fn bytes(&mut self, bytes: &[u8]) {
        if bytes.len() <= self.room() {
            self.bytes[self.length..self.length + bytes.len()].copy_from_slice(bytes);
            self.length += bytes.len();
        }
    }

    fn room(&self) -> usize {
        POLL_BUDGET_BYTES - self.length
    }

    fn event(&mut self, event: InputEvent) {
        if let Some(queue) = self.queue {
            queue.push(event);
            self.queued = true;
        }
    }

    fn view(&mut self, change: ViewMove) {
        if let Some(slot) = self.views.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(change);
        }
    }
}

/// Polls every input device: in rounds of at most 16 events each, the console fed between
/// rounds, until the devices have nothing more or [`MAX_POLL_ROUNDS`] rounds ran; then sends the
/// held key's repeat if it is due, and returns the clock tick to poll again at. `None` without
/// input devices.
pub fn poll<P: Platform>() -> Option<u64> {
    poll_devices::<P>(&INPUT, subscribed_events(), screen(), Timing::now::<P>())
}

fn hand_over<P: Platform>(sink: &PollSink) {
    if terminal::keys_go_to_console() {
        for change in sink.views.into_iter().flatten() {
            terminal::move_view::<P>(change);
        }
        if sink.length > 0 {
            terminal::move_view::<P>(ViewMove::Live);
            console::receive_from_keyboard::<P>(&sink.bytes[..sink.length]);
        }
    }
    terminal::tick::<P>(sink.queued);
}

fn poll_devices<P: Platform>(
    devices: &LeafSpinLock<Option<Box<Input>>>,
    queue: Option<&'static KernelEventQueue>,
    screen: Option<(u32, u32)>,
    timing: Timing,
) -> Option<u64> {
    let mut round = 1;
    loop {
        let mut sink = PollSink {
            bytes: [0; POLL_BUDGET_BYTES],
            length: 0,
            queue,
            views: [None; POLL_VIEW_MOVES],
            queued: false,
        };
        let (last, next) = {
            let mut input = devices.lock();
            let input = input.as_mut()?;
            input
                .state
                .set_cursor_keys(if terminal::application_cursor_keys() {
                    CursorKeys::Application
                } else {
                    CursorKeys::Normal
                });
            let mut more = false;
            for source in input.sources.iter_mut().flatten() {
                let budget = (POLL_BUDGET_BYTES - sink.length) / KeyBytes::CAPACITY;
                let Source {
                    device,
                    capabilities,
                    report,
                    ..
                } = source;
                let state = &mut input.state;
                let taken = device.poll(budget, &mut |event| {
                    state.translate(event, capabilities, report, screen, timing, &mut sink)
                });
                more |= taken == budget;
            }
            let last = !more || round == MAX_POLL_ROUNDS;
            if last {
                input.state.repeat_key(timing, &mut sink);
            }
            PERSON_HOLDS_INPUT.store(input.state.anything_held(), Ordering::Relaxed);
            let next = next_poll(&input.state, timing.now, P::clock_frequency());
            (last, next)
        };
        hand_over::<P>(&sink);
        if last {
            return Some(next);
        }
        round += 1;
    }
}

/// When to poll next after a poll at `now`, on a clock of `frequency` ticks a second: soon while
/// `state` is active, later when idle, and no later than the held key's next repeat.
pub fn next_poll(state: &InputState, now: u64, frequency: u64) -> u64 {
    let ticks = |milliseconds| mul_div_ceil(milliseconds, frequency, 1000);
    let active =
        state.anything_held() || now.saturating_sub(state.last_event()) < ticks(ACTIVE_WINDOW_MS);
    let interval = if active {
        ticks(ACTIVE_POLL_MS)
    } else {
        ticks(IDLE_POLL_MS)
    };
    let next = now.saturating_add(interval);
    state.next_repeat().map_or(next, |repeat| next.min(repeat))
}

/// How the input thread hears from the devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watching {
    /// Every device interrupts: the thread sleeps until one does or a held key repeats.
    Interrupts,
    /// A device must be polled: the thread looks at them all at intervals.
    Polling,
}

/// How the input thread would hear from the devices installed now; `None` without any.
pub fn watching() -> Option<Watching> {
    let input = INPUT.lock();
    let mut sources = input.as_ref()?.sources.iter().flatten().peekable();
    sources.peek()?;
    Some(if sources.all(|source| source.interrupting) {
        Watching::Interrupts
    } else {
        Watching::Polling
    })
}

/// When the held key repeats next, if one is held.
fn next_repeat() -> Option<u64> {
    INPUT.lock().as_ref()?.state.next_repeat()
}

/// Starts the input thread if there are input devices, and says how it hears from them;
/// `Ok(None)` without any, so nothing runs.
pub fn start<P: Platform>() -> Result<Option<Watching>, crate::thread::SpawnError> {
    let Some(watching) = watching() else {
        return Ok(None);
    };
    crate::thread::spawn::<P, _>(move || {
        let mut seen = DEVICE_EVENTS.raised();
        while let Some(next) = poll::<P>() {
            match watching {
                Watching::Polling => crate::thread::sleep_until::<P>(next),
                Watching::Interrupts => {
                    seen = DEVICE_EVENTS.wait::<P>(seen, next_repeat().unwrap_or(u64::MAX));
                }
            }
        }
    })
    .map(|_| Some(watching))
}

#[cfg(test)]
mod tests;
