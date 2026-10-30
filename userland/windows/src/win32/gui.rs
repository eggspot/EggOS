//! The state behind the layer's user32 and gdi32 (D129) and the path every message takes. The
//! state is one heap block per process, made when the program first uses a window, a class, a
//! message or a GDI object, so a console program never pays for it, behind one lock that is
//! never held while the program's code runs. Messages come from `PostMessageW`, from the window
//! server's events (read without blocking whenever the program asks for a message) and from
//! the windows' regions waiting to be painted, and each thread reads only its own windows'
//! messages. One thread at a time reads the server's records and acts on them under the lock;
//! what it must call in the program (a window procedure for a resize, a close request or focus)
//! it calls after letting go, on its own windows, and posts to the window's thread otherwise. A
//! thread with nothing to read sleeps: one of them in `ppoll` on the server's descriptor and a
//! wake-up pipe, the others on a futex word that every change bumps, so no wake-up is lost
//! whichever thread is asleep where. Child windows (D145) share their top-level window's server
//! window: its pointer input goes to the child under the pointer (or the one holding the
//! capture) and its keys to the focused one, while the menu bar and an open drop-down take what
//! falls on them; timers fire after the paints, and a thread with a timer sleeps until it is
//! due. A system control a program subclassed gets its messages through the program's
//! procedure, and the hot keys the program registered are taken from the keys its windows get
//! (D178); the tables only some programs use are one more heap block, made at first use.

use super::window_device::{self, Buffer, Device};
use super::{SpinLock, call_program, error, error_for, heap, thread_environment};
use crate::gui::accelerators::{self, AcceleratorTables};
use crate::gui::atoms::AtomTable;
use crate::gui::classes::ClassTable;
use crate::gui::drawn::DrawnTexts;
use crate::gui::input::{Event, InputState, KEY_STATES, RECORD_SIZE, Record};
use crate::gui::message::{self, point_parameter};
use crate::gui::objects::{self, ContextTable, ObjectTable};
use crate::gui::properties::PropertyTable;
use crate::gui::queue::{Filter, Message, Queue};
use crate::gui::timers::Timers;
use crate::gui::windows::{self, Pixels, Procedure, WindowTable};
use crate::linux;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use eggos_element_tree::QUERY_EVENT;
use eggos_runtime::futex::WakeCount;

const PENDING_CHARACTERS: usize = 8;
const RECORDS_PER_READ: usize = 16;
const NO_DESCRIPTOR: usize = usize::MAX;
const NANOSECONDS_PER_MILLISECOND: u64 = 1_000_000;
const CS_DBLCLKS: u32 = 0x8;
const DOUBLE_CLICK_MS: u32 = 400;
const DOUBLE_CLICK_DISTANCE: i32 = 4;

#[derive(Debug, Clone, Copy, Default)]
struct PendingCharacter {
    key: Message,
    character: u32,
}

/// The most hot keys a program registers.
pub const MAX_HOT_KEYS: usize = 16;
const MOD_KEYS: u32 = 0xf;

/// A hot key `RegisterHotKey` made: the window (0 for the thread) and thread its `WM_HOTKEY`
/// goes to, its id, its `MOD_` modifiers and its virtual key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotKey {
    pub window: usize,
    pub thread: u32,
    pub id: i32,
    pub modifiers: u32,
    pub key: u32,
}

/// The tables only some programs use (D178): registered names, window properties, accelerator
/// tables and hot keys.
#[derive(Debug)]
pub struct Extras {
    pub atoms: AtomTable,
    pub properties: PropertyTable,
    pub accelerators: AcceleratorTables,
    pub hot_keys: [Option<HotKey>; MAX_HOT_KEYS],
}

/// Everything the GUI functions share.
#[derive(Debug)]
pub struct Gui {
    pub classes: ClassTable,
    pub windows: WindowTable,
    pub queue: Queue,
    pub objects: ObjectTable,
    pub contexts: ContextTable,
    pub device: Option<Device>,
    pub timers: Timers,
    pub menus: usize,
    pub tracking: Option<super::menu::Tracking>,
    pub combo: Option<super::combo_box::Tracking>,
    pub active_root: usize,
    pub drawn: DrawnTexts,
    /// The item a context menu (`TrackPopupMenuEx`) closed with, 0 for none, until taken.
    pub menu_choice: Option<usize>,
    /// The tick count of the last input from the window server or the program itself.
    pub last_input: u32,
    /// The `QS_` bits `GetQueueStatus` last reported.
    pub queue_reported: u32,
    pub input: InputState,
    last_click: Option<(usize, u32, (i32, i32))>,
    characters: [PendingCharacter; PENDING_CHARACTERS],
    next_character: usize,
    wake: Option<(usize, usize)>,
    extras: usize,
}

impl Gui {
    fn new() -> Self {
        Self {
            classes: ClassTable::new(),
            windows: windows::new_table(),
            queue: Queue::new(),
            objects: objects::new_objects(),
            contexts: objects::new_contexts(),
            device: None,
            timers: Timers::new(),
            menus: 0,
            tracking: None,
            combo: None,
            active_root: 0,
            drawn: DrawnTexts::new(),
            menu_choice: None,
            last_input: super::user32::now_ms(),
            queue_reported: 0,
            input: InputState::default(),
            last_click: None,
            characters: [PendingCharacter::default(); PENDING_CHARACTERS],
            next_character: 0,
            wake: None,
            extras: 0,
        }
    }

    /// The tables only some programs use, made now if they were not.
    pub fn extras(&mut self) -> Option<&mut Extras> {
        if self.extras == 0 {
            let block = heap::allocate(core::mem::size_of::<Extras>()).cast::<Extras>();
            if block.is_null() {
                return None;
            }
            // SAFETY: a new, 16-byte aligned heap block the size of the tables.
            unsafe {
                block.write(Extras {
                    atoms: AtomTable::new(),
                    properties: PropertyTable::new(),
                    accelerators: accelerators::new_tables(),
                    hot_keys: [None; MAX_HOT_KEYS],
                })
            };
            self.extras = block as usize;
        }
        self.made_extras_mut()
    }

    /// The tables only some programs use, if they were made.
    pub fn made_extras(&self) -> Option<&Extras> {
        // SAFETY: a published block is the one `extras` made and never frees, reached only
        // under the state's lock, which the caller holds through `self`.
        (self.extras != 0).then(|| unsafe { &*(self.extras as *const Extras) })
    }

    /// As [`Self::made_extras`], to change them.
    pub fn made_extras_mut(&mut self) -> Option<&mut Extras> {
        // SAFETY: as `made_extras`, and `&mut self` makes this the only reference.
        (self.extras != 0).then(|| unsafe { &mut *(self.extras as *mut Extras) })
    }

    /// Posts a message for `window` (0 for the thread itself) to `thread`'s queue;
    /// `ERROR_NOT_ENOUGH_QUOTA` when it is full. The caller wakes the threads after the lock.
    pub fn post(
        &mut self,
        window: usize,
        thread: u32,
        number: u32,
        wparam: usize,
        lparam: usize,
    ) -> Result<(), u32> {
        let message = Message {
            window,
            message: number,
            wparam,
            lparam,
            time: super::user32::now_ms(),
            point: (0, 0),
            thread,
        };
        match self.queue.post(message) {
            true => Ok(()),
            false => Err(error::NOT_ENOUGH_QUOTA),
        }
    }

    fn hot_key_for(&self, number: u32, wparam: usize) -> Option<HotKey> {
        if !matches!(number, message::KEY_DOWN | message::SYS_KEY_DOWN) {
            return None;
        }
        let modifiers = self.input.hot_key_modifiers();
        self.made_extras()?
            .hot_keys
            .iter()
            .flatten()
            .find(|hot_key| {
                hot_key.key as usize == wparam && hot_key.modifiers & MOD_KEYS == modifiers
            })
            .copied()
    }

    /// The character `TranslateMessage` makes of key press `key`, once: the one remembered for
    /// that very message, else for the same window and virtual key (D174: programs pass
    /// `TranslateMessage` a `MSG` of their own making, with a changed `lParam`).
    pub fn take_character(&mut self, key: &Message) -> Option<u32> {
        let same = |pending: &PendingCharacter| {
            let other = &pending.key;
            pending.character != 0
                && (
                    other.window,
                    other.message,
                    other.wparam,
                    other.lparam,
                    other.time,
                ) == (key.window, key.message, key.wparam, key.lparam, key.time)
        };
        let same_key = |pending: &PendingCharacter| {
            let other = &pending.key;
            pending.character != 0
                && (other.window, other.message, other.wparam)
                    == (key.window, key.message, key.wparam)
        };
        let index = self
            .characters
            .iter()
            .position(same)
            .or_else(|| self.characters.iter().position(same_key))?;
        Some(core::mem::take(&mut self.characters[index].character))
    }

    fn remember_character(&mut self, key: Message, character: u32) {
        self.characters[self.next_character] = PendingCharacter { key, character };
        self.next_character = (self.next_character + 1) % PENDING_CHARACTERS;
    }

    fn wake_pipe(&mut self) -> Option<(usize, usize)> {
        if self.wake.is_none() {
            let (read, write) = linux::pipe().ok()?;
            WAKE_WRITE.store(write, Ordering::SeqCst);
            self.wake = Some((read, write));
        }
        self.wake
    }

    /// The window `handle` names; `ERROR_INVALID_WINDOW_HANDLE` otherwise.
    pub fn window(&self, handle: usize) -> Result<&windows::Window, u32> {
        self.windows.get(handle).ok_or(error::INVALID_WINDOW_HANDLE)
    }

    pub fn window_mut(&mut self, handle: usize) -> Result<&mut windows::Window, u32> {
        self.windows
            .get_mut(handle)
            .ok_or(error::INVALID_WINDOW_HANDLE)
    }

    /// Whether Shift is held, by the last input the server sent.
    pub fn shift_held(&self) -> bool {
        self.input.shift_held()
    }

    /// The keys and buttons held, by the last input the server sent, as `GetKeyboardState`
    /// reports them (D174).
    pub fn keyboard_state(&self) -> [u8; KEY_STATES] {
        self.input.keyboard_state()
    }

    /// A left button press as `WM_LBUTTONDBLCLK` when it is the second within 400 ms and 4
    /// pixels on the same window of a program's class with `CS_DBLCLKS` (D174, design section
    /// 17); every other press is kept as it is and remembered as a first one.
    pub fn double_click(&mut self, press: Message) -> Message {
        let takes_double_clicks = self.windows.get(press.window).is_some_and(|window| {
            window.class_style & CS_DBLCLKS != 0
                && matches!(window.procedure, Procedure::Program(_))
        });
        let second = self.last_click.is_some_and(|(window, time, (x, y))| {
            window == press.window
                && press.time.wrapping_sub(time) <= DOUBLE_CLICK_MS
                && (x - press.point.0).abs() <= DOUBLE_CLICK_DISTANCE
                && (y - press.point.1).abs() <= DOUBLE_CLICK_DISTANCE
        });
        match takes_double_clicks && second {
            true => {
                self.last_click = None;
                Message {
                    message: message::LEFT_BUTTON_DOUBLE_CLICK,
                    ..press
                }
            }
            false => {
                self.last_click = Some((press.window, press.time, press.point));
                press
            }
        }
    }

    /// The window keys of `root`'s tree go to: its focused window while that is still in the
    /// tree, else `root`.
    pub fn key_target(&self, root: usize) -> usize {
        let focus = self.windows.get(root).map_or(0, |window| window.focus);
        match focus != 0 && windows::is_in_tree(&self.windows, root, focus) {
            true => focus,
            false => root,
        }
    }
}

static GUI: AtomicPtr<SpinLock<Gui>> = AtomicPtr::new(core::ptr::null_mut());
static POLLING: AtomicBool = AtomicBool::new(false);
static PUMPING: AtomicBool = AtomicBool::new(false);
static WAKE_SEQUENCE: AtomicU32 = AtomicU32::new(0);
static FUTEX_WAITERS: AtomicU32 = AtomicU32::new(0);
static WAKE_WRITE: AtomicUsize = AtomicUsize::new(NO_DESCRIPTOR);

fn state() -> Option<&'static SpinLock<Gui>> {
    let current = GUI.load(Ordering::Acquire);
    if !current.is_null() {
        // SAFETY: a published state is never freed or moved.
        return Some(unsafe { &*current });
    }
    let fresh = heap::allocate(core::mem::size_of::<SpinLock<Gui>>()).cast::<SpinLock<Gui>>();
    if fresh.is_null() {
        return None;
    }
    // SAFETY: `fresh` is a new, 16-byte aligned heap block the size of the state.
    unsafe { fresh.write(SpinLock::new(Gui::new())) };
    match GUI.compare_exchange(
        core::ptr::null_mut(),
        fresh,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        // SAFETY: just published, never freed.
        Ok(_) => Some(unsafe { &*fresh }),
        Err(winner) => {
            // SAFETY: the block lost the race, so nothing else saw it.
            unsafe { heap::free(fresh.cast()) };
            // SAFETY: the winner's state is published and never freed.
            Some(unsafe { &*winner })
        }
    }
}

/// Runs `work` on the state under its lock; `None` when there is no memory for it. `work`
/// must not call the program.
pub fn with<R>(work: impl FnOnce(&mut Gui) -> R) -> Option<R> {
    state().map(|state| state.with(work))
}

/// As [`with`], for work that fails with a Win32 error code.
pub fn try_with<R>(work: impl FnOnce(&mut Gui) -> Result<R, u32>) -> Result<R, u32> {
    with(work).unwrap_or(Err(error::NOT_ENOUGH_MEMORY))
}

/// The connection to the window server, opened on first use.
pub fn device() -> Result<Device, u32> {
    if let Some(device) = try_with(|gui| Ok(gui.device))? {
        return Ok(device);
    }
    let opened = Device::open().map_err(error_for)?;
    let kept = try_with(|gui| Ok(*gui.device.get_or_insert(opened)))?;
    if kept != opened {
        let _ = linux::close(opened.fd);
    }
    Ok(kept)
}

/// Wakes the threads sleeping for a message, after something one of them could read was
/// added: the futex word's sleepers, and the thread in `ppoll` through the pipe.
pub fn wake() {
    WAKE_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    if FUTEX_WAITERS.load(Ordering::SeqCst) > 0 {
        linux::futex_wake(WAKE_SEQUENCE.as_ptr() as usize, WakeCount::All);
    }
    if POLLING.load(Ordering::SeqCst) {
        let fd = WAKE_WRITE.load(Ordering::SeqCst);
        if fd != NO_DESCRIPTOR {
            let _ = linux::write(fd, &[1]);
        }
    }
}

/// Calls the window procedure of `window` with a message and returns its result (the program's
/// procedure that subclassed a system control first); 0 for a window that is gone or answered
/// by the layer.
pub fn send(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    let found = with(|gui| {
        gui.windows
            .get(window)
            .map(|window| (window.procedure, window.ansi, window.subclass))
    })
    .flatten();
    let (procedure, ansi) = match found {
        Some((_, _, subclass)) if subclass != 0 => {
            // SAFETY: the address is the procedure the program put in place of the control's
            // with `SetWindowLongPtrW`, which Windows calls with these four arguments.
            return unsafe { call_program(subclass, [window, number as usize, wparam, lparam]) };
        }
        Some((procedure, ansi, _)) => (Some(procedure), ansi),
        None => (None, false),
    };
    match procedure {
        Some(Procedure::Program(address)) if address != 0 && ansi => {
            super::ansi::call_procedure(address, window, number, wparam, lparam)
        }
        Some(Procedure::Program(address)) if address != 0 => {
            // SAFETY: the address is the window procedure the program gave for this window,
            // which Windows calls with these four arguments.
            unsafe { call_program(address, [window, number as usize, wparam, lparam]) }
        }
        Some(Procedure::Control(kind, _)) => {
            super::controls::procedure(window, kind, number, wparam, lparam)
        }
        Some(Procedure::FilePicker(_)) => {
            super::file_picker::procedure(window, number, wparam, lparam)
        }
        _ => 0,
    }
}

/// A message the thread reading the server's records hands on: sent when it is for one of that
/// thread's windows, posted to the window's thread otherwise.
#[derive(Debug, Clone, Copy)]
pub struct Delivery {
    pub window: usize,
    pub thread: u32,
    pub message: u32,
    pub wparam: usize,
    pub lparam: usize,
}

const NO_DELIVERY: Delivery = Delivery {
    window: 0,
    thread: 0,
    message: 0,
    wparam: 0,
    lparam: 0,
};

/// Calls `delivery`'s window procedure when thread `thread` owns the window, else posts it to
/// the window's thread.
pub fn deliver(delivery: &Delivery, thread: u32) {
    if delivery.thread == thread {
        send(
            delivery.window,
            delivery.message,
            delivery.wparam,
            delivery.lparam,
        );
        return;
    }
    let posted = with(|gui| {
        gui.queue.post(Message {
            window: delivery.window,
            message: delivery.message,
            wparam: delivery.wparam,
            lparam: delivery.lparam,
            time: super::user32::now_ms(),
            point: (0, 0),
            thread: delivery.thread,
        })
    });
    if posted == Some(true) {
        wake();
    }
}

/// Posts a message to `window`'s thread (`PostMessageW`), or to the calling thread's own queue
/// when `window` is 0.
pub fn post(window: usize, number: u32, wparam: usize, lparam: usize) -> Result<(), u32> {
    try_with(|gui| {
        let thread = match window {
            0 => thread_environment::thread_id(),
            handle => gui.window(handle)?.thread,
        };
        gui.post(window, thread, number, wparam, lparam)
    })?;
    wake();
    Ok(())
}

fn read_events(thread: u32) {
    let Some(Some(device)) = with(|gui| gui.device) else {
        return;
    };
    let mut records = [0u8; RECORD_SIZE * RECORDS_PER_READ];
    loop {
        if PUMPING
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let read = device.read_events(&mut records).unwrap_or(0);
        let mut deliveries = [NO_DELIVERY; RECORDS_PER_READ];
        let mut count = 0;
        let mut queries = [None; RECORDS_PER_READ];
        let mut asked = 0;
        for record in records[..read].as_chunks::<RECORD_SIZE>().0 {
            let record = Record::parse(record);
            if record.kind == QUERY_EVENT {
                queries[asked] = Some(record);
                asked += 1;
            } else if let Some(delivery) = handle_event(&device, &record) {
                deliveries[count] = delivery;
                count += 1;
            }
        }
        PUMPING.store(false, Ordering::Release);
        if read > 0 {
            wake();
        }
        for delivery in &deliveries[..count] {
            deliver(delivery, thread);
        }
        for query in queries[..asked].iter().flatten() {
            super::elements::answer(&device, query, thread);
        }
        if read < records.len() {
            return;
        }
    }
}

fn handle_event(device: &Device, record: &Record) -> Option<Delivery> {
    let (handle, procedure, thread, event) = with(|gui| {
        let event = gui.input.translate(record);
        let handle = windows::by_server(&gui.windows, u32::from(record.window))?;
        let window = gui.windows.get(handle)?;
        Some((handle, window.procedure, window.thread, event))
    })
    .flatten()?;
    if let Procedure::MessageBox(_) = procedure {
        super::message_box::handle_event(handle, record);
        return None;
    }
    let to_window = |message, wparam, lparam| Delivery {
        window: handle,
        thread,
        message,
        wparam,
        lparam,
    };
    match event {
        Event::Input {
            message: number,
            wparam,
            lparam,
            point,
            character,
        } => {
            let input = Input {
                number,
                wparam,
                lparam,
                point,
                character,
                time: record.time,
            };
            input_event(device, handle, thread, &input)
        }
        Event::Resize { width, height } => resize(device, handle, width, height)
            .map(|(size_kind, size)| to_window(message::SIZE, size_kind, size)),
        Event::StateChanged(state) => state_changed(handle, state)
            .map(|(size_kind, size)| to_window(message::SIZE, size_kind, size)),
        Event::CloseRequest => Some(to_window(message::SYS_COMMAND, message::SC_CLOSE, 0)),
        Event::FocusIn => {
            with(|gui| gui.active_root = handle);
            Some(to_window(message::SET_FOCUS, 0, 0))
        }
        Event::FocusOut => {
            with(|gui| {
                if gui.active_root == handle {
                    gui.active_root = 0;
                }
            });
            Some(to_window(message::KILL_FOCUS, 0, 0))
        }
        Event::PointerLeft | Event::Ignored => None,
    }
}

enum Routed {
    Menu(super::menu::Effect),
    Combo(super::combo_box::Effect),
    Queued,
    Dropped,
}

/// One key or pointer input for a top-level window's tree, the point in its content.
#[derive(Debug, Clone, Copy)]
pub struct Input {
    pub number: u32,
    pub wparam: usize,
    pub lparam: usize,
    pub point: (i32, i32),
    pub character: Option<u32>,
    pub time: u32,
}

fn input_event(device: &Device, root: usize, thread: u32, input: &Input) -> Option<Delivery> {
    let routed = with(|gui| {
        gui.last_input = super::user32::now_ms();
        let (number, wparam, point) = (input.number, input.wparam, input.point);
        if let Some(hot_key) = gui.hot_key_for(number, wparam) {
            let lparam = message::word_pair(hot_key.modifiers as usize, hot_key.key as usize);
            let posted = gui.post(
                hot_key.window,
                hot_key.thread,
                message::HOT_KEY,
                hot_key.id as isize as usize,
                lparam,
            );
            return match posted {
                Ok(()) => Routed::Queued,
                Err(_) => Routed::Dropped,
            };
        }
        let combo = super::combo_box::input(gui, root, number, wparam, point, input.character);
        if let Some(effect) = combo {
            return Routed::Combo(effect);
        }
        if let Some(effect) = super::menu::input(gui, root, number, wparam, point) {
            return Routed::Menu(effect);
        }
        match route_input(gui, root, number, wparam, input.lparam, point, input.time) {
            Some(message) => {
                let message = match message.message {
                    message::LEFT_BUTTON_DOWN => gui.double_click(message),
                    _ => message,
                };
                gui.queue.post_input(message);
                if let Some(character) = input.character {
                    gui.remember_character(message, character);
                }
                Routed::Queued
            }
            None => Routed::Dropped,
        }
    });
    match routed {
        Some(Routed::Menu(effect)) => super::menu::finish_input(device, root, thread, effect),
        Some(Routed::Combo(effect)) => super::combo_box::finish_input(device, root, effect),
        _ => None,
    }
}

/// Input the program sends itself (`SendInput`, `keybd_event`, `mouse_event`, D178) for
/// top-level window `root`'s tree: through its hot keys, combo list and menus to the window
/// the window server's input would reach, as if it had come from the server.
pub fn inject(root: usize, input: &Input) {
    let Ok(device) = device() else {
        return;
    };
    let Some(owner) = with(|gui| gui.windows.get(root).map(|window| window.thread)).flatten()
    else {
        return;
    };
    if let Some(delivery) = input_event(&device, root, owner, input) {
        deliver(&delivery, thread_environment::thread_id());
    }
    wake();
}

fn is_mouse(number: u32) -> bool {
    (message::FIRST_MOUSE..=message::LAST_MOUSE).contains(&number)
}

fn route_input(
    gui: &Gui,
    root: usize,
    number: u32,
    wparam: usize,
    lparam: usize,
    point: (i32, i32),
    time: u32,
) -> Option<Message> {
    let top = gui.windows.get(root)?;
    if !top.enabled || top.modal_disabled {
        return None;
    }
    let message = |window: usize, lparam: usize, point: (i32, i32)| {
        let thread = gui.windows.get(window).map_or(0, |window| window.thread);
        Message {
            window,
            message: number,
            wparam,
            lparam,
            time,
            point,
            thread,
        }
    };
    if !is_mouse(number) {
        return Some(message(gui.key_target(root), lparam, point));
    }
    let capture = top.capture;
    let target = match capture != 0 && windows::is_in_tree(&gui.windows, root, capture) {
        true => capture,
        false => {
            if point.1 < top.menu_height {
                return None;
            }
            windows::child_at(&gui.windows, root, point.0, point.1 - top.menu_height).0
        }
    };
    if !windows::enabled_in_tree(&gui.windows, target) {
        return None;
    }
    let origin = windows::client_placement(&gui.windows, target)?.origin;
    let local = (
        point.0.saturating_sub(origin.0),
        point.1.saturating_sub(origin.1),
    );
    let lparam = match number {
        message::MOUSE_WHEEL | message::MOUSE_HORIZONTAL_WHEEL => lparam,
        _ => point_parameter(local.0, local.1),
    };
    Some(message(target, lparam, local))
}

pub(super) fn resize(
    device: &Device,
    handle: usize,
    width: u32,
    height: u32,
) -> Option<(usize, usize)> {
    let (server, size_kind) = with(|gui| {
        let window = gui.windows.get(handle)?;
        Some((window.server, window.size_kind))
    })
    .flatten()?;
    let new = device.resize(server, width, height).ok()?;
    let new = Pixels {
        address: new.pixels,
        width: new.width,
        height: new.height,
    };
    let unused = with(|gui| {
        let unused = windows::replace_pixels(&mut gui.windows, handle, new);
        super::menu::paint_bar(gui, handle);
        unused
    })
    .unwrap_or(new);
    // SAFETY: `replace_pixels` gave back the buffer no table entry holds any more (the
    // window's old one, or the new one if the window went meanwhile), and drawing reaches a
    // buffer only through the table under its lock.
    unsafe {
        window_device::unmap(&Buffer {
            window: server,
            width: unused.width,
            height: unused.height,
            pixels: unused.address,
        })
    };
    let client = with(|gui| {
        gui.windows
            .get(handle)
            .map(|window| point_parameter(window.width as i32, window.height as i32))
    })
    .flatten()?;
    (unused != new).then_some((size_kind, client))
}

fn state_changed(handle: usize, state: u32) -> Option<(usize, usize)> {
    let size_kind = match state {
        1 => message::SIZE_MINIMIZED,
        2 => message::SIZE_MAXIMIZED,
        _ => message::SIZE_RESTORED,
    };
    let size = with(|gui| {
        let window = gui.windows.get_mut(handle)?;
        window.size_kind = size_kind;
        Some(point_parameter(window.width as i32, window.height as i32))
    })
    .flatten()?;
    Some(match size_kind {
        message::SIZE_MINIMIZED => (size_kind, 0),
        _ => (size_kind, size),
    })
}

fn find(gui: &mut Gui, filter: &Filter, remove: bool) -> Option<Message> {
    if let Some(message) = gui.queue.take(filter, remove) {
        return Some(message);
    }
    if filter.accepts_number(message::PAINT)
        && let Some(window) = windows::needing_paint(&gui.windows, filter.window, filter.thread)
    {
        if remove && let Some(entry) = gui.windows.get_mut(window) {
            entry.paint_sent = true;
        }
        return Some(Message {
            window,
            message: message::PAINT,
            thread: filter.thread,
            ..Message::default()
        });
    }
    gui.timers.take_due(filter, super::user32::now_ms(), remove)
}

/// What [`wait_for`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Message(Message),
    Done,
    Nothing,
}

/// The calling thread's next message `filter` accepts (taken out of the queue when `remove`
/// says so), the server's records read first; with `wait`, the thread sleeps until one comes
/// or `done` holds for the state, checked under the lock.
pub fn wait_for(filter: Filter, remove: bool, wait: bool, done: &dyn Fn(&Gui) -> bool) -> Next {
    let thread = thread_environment::thread_id();
    let filter = Filter { thread, ..filter };
    loop {
        read_events(thread);
        let found = with(|gui| match done(gui) {
            true => Next::Done,
            false => find(gui, &filter, remove).map_or(Next::Nothing, Next::Message),
        });
        match found {
            Some(Next::Nothing) if wait => sleep(&filter, done),
            Some(found) => return found,
            None => return Next::Nothing,
        }
    }
}

/// The calling thread's next message `filter` accepts: [`wait_for`] with nothing else to wait
/// for.
pub fn next_message(filter: Filter, remove: bool, wait: bool) -> Option<Message> {
    match wait_for(filter, remove, wait, &|_| false) {
        Next::Message(message) => Some(message),
        Next::Done | Next::Nothing => None,
    }
}

fn sleep(filter: &Filter, done: &dyn Fn(&Gui) -> bool) {
    let sequence = WAKE_SEQUENCE.load(Ordering::SeqCst);
    let descriptors = with(|gui| {
        let (read, _) = gui.wake_pipe()?;
        let device = gui.device.map_or(-1, |device| device.fd as i32);
        Some([device, read as i32])
    })
    .flatten();
    let Some(descriptors) = descriptors else {
        linux::yield_now();
        return;
    };
    let timeout = with(|gui| gui.timers.next_due(filter, super::user32::now_ms()))
        .flatten()
        .map(|milliseconds| u64::from(milliseconds) * NANOSECONDS_PER_MILLISECOND);
    if timeout == Some(0) {
        return;
    }
    let polling = POLLING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    let idle = with(|gui| !done(gui) && find(gui, filter, false).is_none()).unwrap_or(false);
    if !polling {
        if idle {
            FUTEX_WAITERS.fetch_add(1, Ordering::SeqCst);
            let _ = linux::futex_wait(&WAKE_SEQUENCE, sequence, timeout);
            FUTEX_WAITERS.fetch_sub(1, Ordering::SeqCst);
        }
        return;
    }
    if idle {
        let _ = linux::wait_readable(descriptors, timeout);
    }
    let mut drained = [0u8; 64];
    while matches!(linux::read(descriptors[1] as usize, &mut drained), Ok(1..)) {}
    POLLING.store(false, Ordering::SeqCst);
    wake();
}
