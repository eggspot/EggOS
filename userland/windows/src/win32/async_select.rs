//! `WSAAsyncSelect` (D348): a socket's network events as window messages. The thread that
//! sleeps on the window server's descriptor watches the registered sockets too, and every
//! thread checks them when it looks for a message. Each event is posted once to the window,
//! with the socket as `wParam` and the event and its error as `lParam`, until the call that
//! re-enables it, as on Windows: `FD_READ` and `FD_OOB` after a `recv`, `FD_WRITE` after a
//! `send` that would have blocked; `FD_CONNECT` and `FD_CLOSE` once.

use super::{SpinLock, gui};
use crate::linux::{self, POLL_ERROR, POLL_HANG_UP, POLL_IN, POLL_OUT, Watched};
use crate::sockets::code;
use core::sync::atomic::{AtomicUsize, Ordering};

/// The most sockets registered at once.
pub const MOST_SOCKETS: usize = 64;
const MSG_PEEK: usize = 0x2;
const MSG_DONTWAIT: usize = 0x40;
const EAGAIN: u16 = 11;

/// `WSAAsyncSelect`'s network events (`FD_*`).
pub mod event {
    pub const READ: u32 = 0x01;
    pub const WRITE: u32 = 0x02;
    pub const OOB: u32 = 0x04;
    pub const ACCEPT: u32 = 0x08;
    pub const CONNECT: u32 = 0x10;
    pub const CLOSE: u32 = 0x20;
}

const ONCE: u32 = event::CONNECT | event::CLOSE;

#[derive(Debug, Clone, Copy)]
struct Registered {
    socket: usize,
    fd: usize,
    window: usize,
    message: u32,
    asked: u32,
    armed: u32,
}

static SOCKETS: SpinLock<[Option<Registered>; MOST_SOCKETS]> = SpinLock::new([None; MOST_SOCKETS]);
static REGISTERED: AtomicUsize = AtomicUsize::new(0);

fn remove(table: &mut [Option<Registered>; MOST_SOCKETS], socket: usize) {
    for slot in table.iter_mut() {
        if slot.is_some_and(|registered| registered.socket == socket) {
            *slot = None;
            REGISTERED.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Has `socket` (descriptor `fd`) post `message` to `window` for `events`, replacing what it
/// asked before; no window or no events only forgets it. `WSAENOBUFS` when the table is full.
pub fn select(
    socket: usize,
    fd: usize,
    window: usize,
    message: u32,
    events: u32,
) -> Result<(), u32> {
    let result = SOCKETS.with(|table| {
        remove(table, socket);
        if window == 0 || events == 0 {
            return Ok(());
        }
        let slot = table
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(code::WSAENOBUFS)?;
        *slot = Some(Registered {
            socket,
            fd,
            window,
            message,
            asked: events,
            armed: events,
        });
        REGISTERED.fetch_add(1, Ordering::AcqRel);
        Ok(())
    });
    gui::wake();
    result
}

/// Re-enables `events` of `socket` after the call that re-enables them on Windows.
pub fn rearm(socket: usize, events: u32) {
    if REGISTERED.load(Ordering::Acquire) == 0 {
        return;
    }
    SOCKETS.with(|table| {
        for registered in table.iter_mut().flatten() {
            if registered.socket == socket {
                registered.armed |= events & registered.asked & !ONCE;
            }
        }
    });
    gui::wake();
}

/// Forgets `socket`, which the program closed.
pub fn forget(socket: usize) {
    if REGISTERED.load(Ordering::Acquire) != 0 {
        SOCKETS.with(|table| remove(table, socket));
    }
}

fn poll_events(armed: u32) -> u16 {
    let mut asked = 0;
    if armed & (event::READ | event::OOB | event::CLOSE) != 0 {
        asked |= POLL_IN;
    }
    if armed & (event::WRITE | event::CONNECT) != 0 {
        asked |= POLL_OUT;
    }
    asked
}

/// The registered sockets' descriptors to watch for their armed events, into `out`; how many.
pub fn watched(out: &mut [Watched]) -> usize {
    if REGISTERED.load(Ordering::Acquire) == 0 {
        return 0;
    }
    SOCKETS.with(|table| {
        let wanted = table
            .iter()
            .flatten()
            .map(|registered| (registered.fd, poll_events(registered.armed)))
            .filter(|&(_, asked)| asked != 0);
        let mut count = 0;
        for ((fd, asked), slot) in wanted.zip(out.iter_mut()) {
            *slot = Watched {
                fd: fd as i32,
                asked,
                reported: 0,
            };
            count += 1;
        }
        count
    })
}

fn incoming(fd: usize) -> Option<(u32, u32)> {
    let mut byte = [0u8; 1];
    match linux::receive(fd, &mut byte, MSG_PEEK | MSG_DONTWAIT) {
        Ok(0) => Some((event::CLOSE, 0)),
        Ok(_) => Some((event::READ, 0)),
        Err(errno) if errno.0 == EAGAIN => None,
        Err(errno) => Some((event::CLOSE, crate::sockets::error_for(errno.0))),
    }
}

fn happened(registered: &Registered, reported: u16) -> [Option<(u32, u32)>; 2] {
    let armed = registered.armed;
    let failed = reported & (POLL_ERROR | POLL_HANG_UP) != 0;
    let outgoing = match () {
        () if armed & event::CONNECT != 0 && (reported & POLL_OUT != 0 || failed) => {
            let error = if failed { code::WSAECONNREFUSED } else { 0 };
            Some((event::CONNECT, error))
        }
        () if armed & event::WRITE != 0 && reported & POLL_OUT != 0 => Some((event::WRITE, 0)),
        () => None,
    };
    let inbound = match reported & POLL_IN != 0 {
        true => incoming(registered.fd),
        false if reported & POLL_HANG_UP != 0 => Some((event::CLOSE, code::WSAECONNRESET)),
        false => None,
    };
    let inbound = inbound.filter(|&(kind, _)| armed & kind != 0);
    [outgoing, inbound]
}

/// Checks the registered sockets' armed events now and posts each that happened.
pub fn post_ready() {
    if REGISTERED.load(Ordering::Acquire) == 0 {
        return;
    }
    let mut list = [Watched::default(); MOST_SOCKETS];
    let count = watched(&mut list);
    if count == 0 || linux::wait_for_events(&mut list[..count], Some(0)).unwrap_or(0) == 0 {
        return;
    }
    let mut posts = [(0usize, 0u32, 0usize, 0usize); 2 * MOST_SOCKETS];
    let mut posted = 0;
    SOCKETS.with(|table| {
        for one in list[..count].iter().filter(|one| one.reported != 0) {
            let Some(registered) = table
                .iter_mut()
                .flatten()
                .find(|registered| registered.fd as i32 == one.fd)
            else {
                continue;
            };
            for (kind, error) in happened(registered, one.reported).into_iter().flatten() {
                registered.armed &= !kind;
                let reply = (error as usize) << 16 | kind as usize;
                posts[posted] = (
                    registered.window,
                    registered.message,
                    registered.socket,
                    reply,
                );
                posted += 1;
            }
        }
    });
    for &(window, message, socket, reply) in &posts[..posted] {
        let _ = gui::post(window, message, socket, reply);
    }
}
