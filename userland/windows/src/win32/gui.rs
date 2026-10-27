//! The state behind the layer's user32 and gdi32 (D129) and the path every message takes. The
//! state is one heap block per process, made when the program first uses a window, a class, a
//! message or a GDI object, so a console program never pays for it, behind one lock that is
//! never held while the program's code runs. Messages come from `PostMessageW`, from the window
//! server's events (read without blocking whenever the program asks for a message) and from
//! the windows' regions waiting to be painted; a thread with nothing to read sleeps in `ppoll`
//! on the server's descriptor and a pipe that a post from another thread writes to.

use super::window_device::{Buffer, Device};
use super::{SpinLock, call_program, error, error_for, heap};
use crate::gui::classes::ClassTable;
use crate::gui::input::{Event, InputState, RECORD_SIZE, Record};
use crate::gui::message::{self, point_parameter};
use crate::gui::objects::{self, ContextTable, ObjectTable};
use crate::gui::queue::{Filter, Message, Queue};
use crate::gui::windows::{self, Procedure, WindowTable};
use crate::linux;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use eggos_windows_gui::surface::Rect;

const PENDING_CHARACTERS: usize = 8;
const RECORDS_PER_READ: usize = 16;
const NO_DESCRIPTOR: usize = usize::MAX;

#[derive(Debug, Clone, Copy, Default)]
struct PendingCharacter {
    key: Message,
    character: u32,
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
    input: InputState,
    characters: [PendingCharacter; PENDING_CHARACTERS],
    next_character: usize,
    wake: Option<(usize, usize)>,
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
            input: InputState::default(),
            characters: [PendingCharacter::default(); PENDING_CHARACTERS],
            next_character: 0,
            wake: None,
        }
    }

    /// The character `TranslateMessage` makes of key press `key`, once.
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
        let pending = self.characters.iter_mut().find(|pending| same(pending))?;
        Some(core::mem::take(&mut pending.character))
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
}

static GUI: AtomicPtr<SpinLock<Gui>> = AtomicPtr::new(core::ptr::null_mut());
static SLEEPERS: AtomicUsize = AtomicUsize::new(0);
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

/// Wakes a thread sleeping for a message, after something it could read was added.
pub fn wake() {
    if SLEEPERS.load(Ordering::SeqCst) > 0 {
        let fd = WAKE_WRITE.load(Ordering::SeqCst);
        if fd != NO_DESCRIPTOR {
            let _ = linux::write(fd, &[1]);
        }
    }
}

/// Calls the window procedure of `window` with a message and returns its result; 0 for a
/// window that is gone or answered by the layer.
pub fn send(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    let procedure = with(|gui| gui.windows.get(window).map(|window| window.procedure)).flatten();
    match procedure {
        Some(Procedure::Program(address)) if address != 0 => {
            // SAFETY: the address is the window procedure the program gave for this window,
            // which Windows calls with these four arguments.
            unsafe { call_program(address, [window, number as usize, wparam, lparam]) }
        }
        _ => 0,
    }
}

fn read_events() {
    let Some(Some(device)) = with(|gui| gui.device) else {
        return;
    };
    let mut records = [0u8; RECORD_SIZE * RECORDS_PER_READ];
    loop {
        let read = match device.read_events(&mut records) {
            Ok(read) => read,
            Err(_) => return,
        };
        for record in records[..read].as_chunks::<RECORD_SIZE>().0 {
            handle_event(&Record::parse(record));
        }
        if read < records.len() {
            return;
        }
    }
}

fn handle_event(record: &Record) {
    let target = with(|gui| {
        let event = gui.input.translate(record);
        let handle = windows::by_server(&gui.windows, u32::from(record.window))?;
        let procedure = gui.windows.get(handle)?.procedure;
        Some((handle, procedure, event))
    })
    .flatten();
    let Some((handle, procedure, event)) = target else {
        return;
    };
    if let Procedure::MessageBox(state) = procedure {
        super::message_box::handle_event(state, record);
        return;
    }
    match event {
        Event::Input {
            message: number,
            wparam,
            lparam,
            point,
            character,
        } => {
            let message = Message {
                window: handle,
                message: number,
                wparam,
                lparam,
                time: record.time,
                point,
            };
            with(|gui| {
                gui.queue.post_input(message);
                if let Some(character) = character {
                    gui.remember_character(message, character);
                }
            });
        }
        Event::Resize { width, height } => resize(handle, width, height),
        Event::StateChanged(state) => state_changed(handle, state),
        Event::CloseRequest => {
            send(handle, message::SYS_COMMAND, message::SC_CLOSE, 0);
        }
        Event::FocusIn => {
            send(handle, message::SET_FOCUS, 0, 0);
        }
        Event::FocusOut => {
            send(handle, message::KILL_FOCUS, 0, 0);
        }
        Event::PointerLeft | Event::Ignored => {}
    }
}

fn resize(handle: usize, width: u32, height: u32) {
    let found = with(|gui| {
        let device = gui.device?;
        let window = gui.windows.get(handle)?;
        let buffer = Buffer {
            window: window.server,
            width: window.width,
            height: window.height,
            pixels: window.pixels,
        };
        Some((device, buffer, window.size_kind))
    })
    .flatten();
    let Some((device, mut buffer, size_kind)) = found else {
        return;
    };
    if device.resize(&mut buffer, width, height).is_err() {
        return;
    }
    with(|gui| {
        if let Some(window) = gui.windows.get_mut(handle) {
            window.pixels = buffer.pixels;
            window.width = buffer.width;
            window.height = buffer.height;
            window.update = Rect::default();
            window.invalidate(None, true);
        }
    });
    let size = point_parameter(buffer.width as i32, buffer.height as i32);
    send(handle, message::SIZE, size_kind, size);
}

fn state_changed(handle: usize, state: u32) {
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
    .flatten();
    if let Some(size) = size {
        let size = if size_kind == message::SIZE_MINIMIZED {
            0
        } else {
            size
        };
        send(handle, message::SIZE, size_kind, size);
    }
}

fn find(gui: &mut Gui, filter: &Filter, remove: bool) -> Option<Message> {
    if let Some(message) = gui.queue.take(filter, remove) {
        return Some(message);
    }
    if !filter.accepts_number(message::PAINT) {
        return None;
    }
    let window = windows::needing_paint(&gui.windows, filter.window)?;
    Some(Message {
        window,
        message: message::PAINT,
        ..Message::default()
    })
}

/// The next message `filter` accepts (taken out of the queue when `remove` says so): the
/// server's events are read first; with `wait`, the thread sleeps until one comes.
pub fn next_message(filter: Filter, remove: bool, wait: bool) -> Option<Message> {
    loop {
        read_events();
        let found = with(|gui| find(gui, &filter, remove)).flatten();
        if found.is_some() || !wait {
            return found;
        }
        sleep(&filter);
    }
}

fn sleep(filter: &Filter) {
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
    SLEEPERS.fetch_add(1, Ordering::SeqCst);
    let nothing_yet = with(|gui| find(gui, filter, false).is_none()).unwrap_or(false);
    if nothing_yet {
        let _ = linux::wait_readable(descriptors);
    }
    SLEEPERS.fetch_sub(1, Ordering::SeqCst);
    let mut drained = [0u8; 64];
    while matches!(linux::read(descriptors[1] as usize, &mut drained), Ok(1..)) {}
}
