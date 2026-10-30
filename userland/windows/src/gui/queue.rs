//! The message queue `GetMessageW` and `PeekMessageW` read: messages posted with
//! `PostMessageW`, then input from the window server, then `WM_QUIT` once `PostQuitMessage` asked
//! for it, as Windows orders them (`WM_PAINT` comes from the windows' update regions, after all
//! of these). Mouse moves coalesce: a move replaces the newest input when that is a move of the
//! same window. Every message belongs to a thread, the one that made its window or posted it to
//! no window, and only that thread reads it, as each Windows thread reads only its own queue.

use super::message;

/// The most posted messages waiting at once (Windows' quota is larger).
pub const POSTED_CAPACITY: usize = 256;
/// The most input messages waiting at once; past it the oldest is dropped.
pub const INPUT_CAPACITY: usize = 256;
/// The most threads with a `WM_QUIT` waiting at once.
pub const QUIT_CAPACITY: usize = 8;

/// One message, as `MSG` holds it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Message {
    pub window: usize,
    pub message: u32,
    pub wparam: usize,
    pub lparam: usize,
    pub time: u32,
    pub point: (i32, i32),
    pub thread: u32,
}

/// Which messages a read takes: those of `thread` (any when 0) and of `window` (any when 0,
/// only those posted to no window when [`THREAD_MESSAGES`]) numbered `first..=last` (any when
/// both are 0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Filter {
    pub window: usize,
    pub first: u32,
    pub last: u32,
    pub thread: u32,
}

/// The window filter `-1`: messages posted to the thread, not to a window.
pub const THREAD_MESSAGES: usize = usize::MAX;

/// `GetQueueStatus`'s `QS_` bits for the kinds of message waiting.
pub mod status {
    pub const KEY: u32 = 0x1;
    pub const MOUSE_MOVE: u32 = 0x2;
    pub const MOUSE_BUTTON: u32 = 0x4;
    pub const POST_MESSAGE: u32 = 0x8;
    pub const TIMER: u32 = 0x10;
    pub const PAINT: u32 = 0x20;
    pub const HOT_KEY: u32 = 0x80;
}

impl Filter {
    pub fn accepts_number(&self, number: u32) -> bool {
        (self.first == 0 && self.last == 0) || (self.first..=self.last).contains(&number)
    }

    pub fn accepts(&self, message: &Message) -> bool {
        let window = match self.window {
            0 => true,
            THREAD_MESSAGES => message.window == 0,
            window => window == message.window,
        };
        let thread = self.thread == 0 || self.thread == message.thread;
        window && thread && self.accepts_number(message.message)
    }
}

#[derive(Debug)]
struct Ring<const N: usize> {
    entries: [Message; N],
    start: usize,
    length: usize,
}

impl<const N: usize> Ring<N> {
    const fn new() -> Self {
        Self {
            entries: [Message {
                window: 0,
                message: 0,
                wparam: 0,
                lparam: 0,
                time: 0,
                point: (0, 0),
                thread: 0,
            }; N],
            start: 0,
            length: 0,
        }
    }

    fn at(&self, index: usize) -> &Message {
        &self.entries[(self.start + index) % N]
    }

    fn push(&mut self, message: Message) -> bool {
        if self.length == N {
            return false;
        }
        self.entries[(self.start + self.length) % N] = message;
        self.length += 1;
        true
    }

    fn newest_mut(&mut self) -> Option<&mut Message> {
        let index = self.length.checked_sub(1)?;
        Some(&mut self.entries[(self.start + index) % N])
    }

    fn drop_oldest(&mut self) {
        if self.length > 0 {
            self.start = (self.start + 1) % N;
            self.length -= 1;
        }
    }

    fn find(&self, filter: &Filter) -> Option<usize> {
        (0..self.length).find(|&index| filter.accepts(self.at(index)))
    }

    fn remove(&mut self, index: usize) -> Message {
        let message = *self.at(index);
        for at in index..self.length - 1 {
            self.entries[(self.start + at) % N] = *self.at(at + 1);
        }
        self.length -= 1;
        message
    }
}

/// The process's queue.
#[derive(Debug)]
pub struct Queue {
    posted: Ring<POSTED_CAPACITY>,
    input: Ring<INPUT_CAPACITY>,
    quits: [Option<(u32, usize)>; QUIT_CAPACITY],
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    pub const fn new() -> Self {
        Self {
            posted: Ring::new(),
            input: Ring::new(),
            quits: [None; QUIT_CAPACITY],
        }
    }

    /// Posts `message`; `false` when the queue is full.
    pub fn post(&mut self, message: Message) -> bool {
        self.posted.push(message)
    }

    /// Queues input from the window server.
    pub fn post_input(&mut self, message: Message) {
        if message.message == message::MOUSE_MOVE
            && let Some(newest) = self.input.newest_mut()
            && newest.message == message::MOUSE_MOVE
            && newest.window == message.window
        {
            *newest = message;
            return;
        }
        if !self.input.push(message) {
            self.input.drop_oldest();
            self.input.push(message);
        }
    }

    /// `PostQuitMessage(code)` on `thread`: its `WM_QUIT` comes once nothing posted or input
    /// is left for it. A thread asking again replaces its code; past [`QUIT_CAPACITY`] threads
    /// the oldest request is replaced.
    pub fn quit(&mut self, thread: u32, code: usize) {
        let slot = self
            .quits
            .iter()
            .position(|quit| quit.is_some_and(|(asking, _)| asking == thread))
            .or_else(|| self.quits.iter().position(Option::is_none))
            .unwrap_or(0);
        self.quits[slot] = Some((thread, code));
    }

    /// The first message `filter` accepts, taken out of the queue when `remove` says so.
    pub fn take(&mut self, filter: &Filter, remove: bool) -> Option<Message> {
        if let Some(index) = self.posted.find(filter) {
            return Some(match remove {
                true => self.posted.remove(index),
                false => *self.posted.at(index),
            });
        }
        if let Some(index) = self.input.find(filter) {
            return Some(match remove {
                true => self.input.remove(index),
                false => *self.input.at(index),
            });
        }
        if !filter.accepts_number(message::QUIT) {
            return None;
        }
        let slot = self.quits.iter().position(|quit| {
            quit.is_some_and(|(thread, _)| filter.thread == 0 || filter.thread == thread)
        })?;
        let (thread, code) = self.quits[slot]?;
        if remove {
            self.quits[slot] = None;
        }
        Some(Message {
            message: message::QUIT,
            wparam: code,
            thread,
            ..Message::default()
        })
    }

    /// The kinds of message waiting for `thread` as [`status`] bits: posted messages (a posted
    /// `WM_HOTKEY` as a hot key, a `WM_QUIT` as a posted message), keys, mouse moves and mouse
    /// buttons.
    pub fn status(&self, thread: u32) -> u32 {
        let mine = Filter {
            thread,
            ..Filter::default()
        };
        let mut bits = 0;
        for index in 0..self.posted.length {
            let posted = self.posted.at(index);
            if mine.accepts(posted) {
                bits |= match posted.message {
                    message::HOT_KEY => status::HOT_KEY,
                    _ => status::POST_MESSAGE,
                };
            }
        }
        for index in 0..self.input.length {
            let input = self.input.at(index);
            if mine.accepts(input) {
                bits |= match input.message {
                    message::MOUSE_MOVE => status::MOUSE_MOVE,
                    message::FIRST_MOUSE..=message::LAST_MOUSE => status::MOUSE_BUTTON,
                    _ => status::KEY,
                };
            }
        }
        if self
            .quits
            .iter()
            .flatten()
            .any(|(asking, _)| *asking == thread)
        {
            bits |= status::POST_MESSAGE;
        }
        bits
    }

    /// Drops everything waiting for `window`, which is gone.
    pub fn forget(&mut self, window: usize) {
        let filter = Filter {
            window,
            ..Filter::default()
        };
        while let Some(index) = self.posted.find(&filter) {
            self.posted.remove(index);
        }
        while let Some(index) = self.input.find(&filter) {
            self.input.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(window: usize, number: u32, wparam: usize) -> Message {
        Message {
            window,
            message: number,
            wparam,
            ..Message::default()
        }
    }

    const ANY: Filter = Filter {
        window: 0,
        first: 0,
        last: 0,
        thread: 0,
    };

    #[test]
    fn posted_messages_come_before_input_and_quit_comes_last() {
        let mut queue = Box::new(Queue::new());
        queue.quit(0, 3);
        queue.post_input(message(1, message::KEY_DOWN, 65));
        queue.post(message(1, 0x400, 1));
        queue.post(message(2, 0x401, 2));
        assert_eq!(queue.take(&ANY, false).unwrap().wparam, 1);
        assert_eq!(queue.take(&ANY, true).unwrap().wparam, 1);
        assert_eq!(queue.take(&ANY, true).unwrap().wparam, 2);
        assert_eq!(queue.take(&ANY, true).unwrap().message, message::KEY_DOWN);
        let quit = queue.take(&ANY, true).unwrap();
        assert_eq!((quit.message, quit.wparam), (message::QUIT, 3));
        assert_eq!(queue.take(&ANY, true), None);
    }

    #[test]
    fn filters_pick_by_window_and_number_range() {
        let mut queue = Box::new(Queue::new());
        queue.post(message(1, 0x400, 1));
        queue.post(message(2, 0x401, 2));
        queue.post(message(2, 0x500, 3));
        let second = Filter { window: 2, ..ANY };
        assert_eq!(queue.take(&second, true).unwrap().wparam, 2);
        queue.post(message(0, 0x402, 4));
        let thread = Filter {
            window: THREAD_MESSAGES,
            ..ANY
        };
        assert_eq!(queue.take(&thread, true).unwrap().wparam, 4);
        assert_eq!(queue.take(&thread, true), None);
        let range = Filter {
            window: 0,
            first: 0x500,
            last: 0x5ff,
            thread: 0,
        };
        assert_eq!(queue.take(&range, true).unwrap().wparam, 3);
        assert_eq!(queue.take(&range, true), None);
        queue.quit(0, 0);
        assert_eq!(queue.take(&range, true), None);
        assert_eq!(queue.take(&ANY, true).unwrap().wparam, 1);
        assert_eq!(queue.take(&ANY, true).unwrap().message, message::QUIT);
    }

    #[test]
    fn each_thread_reads_only_its_own_messages_and_quit() {
        let mut queue = Box::new(Queue::new());
        let mine = |thread| Filter { thread, ..ANY };
        queue.post(Message {
            thread: 5,
            ..message(1, 0x400, 1)
        });
        queue.post_input(Message {
            thread: 6,
            ..message(2, message::KEY_DOWN, 2)
        });
        queue.quit(6, 9);
        queue.quit(6, 7);
        assert_eq!(queue.take(&mine(6), true).unwrap().wparam, 2);
        let quit = queue.take(&mine(6), true).unwrap();
        assert_eq!(
            (quit.message, quit.wparam, quit.thread),
            (message::QUIT, 7, 6)
        );
        assert_eq!(queue.take(&mine(6), true), None);
        assert_eq!(queue.take(&mine(5), true).unwrap().wparam, 1);
        for thread in 1..=QUIT_CAPACITY as u32 + 1 {
            queue.quit(thread, thread as usize);
        }
        assert_eq!(
            queue.take(&mine(1), true),
            None,
            "the oldest request was replaced"
        );
        assert_eq!(queue.take(&mine(9), true).unwrap().wparam, 9);
    }

    #[test]
    fn the_status_names_the_kinds_of_message_waiting_for_a_thread() {
        let mut queue = Box::new(Queue::new());
        assert_eq!(queue.status(5), 0);
        let of = |thread: u32, number: u32| Message {
            thread,
            ..message(1, number, 0)
        };
        queue.post(of(5, 0x400));
        queue.post(of(6, message::HOT_KEY));
        queue.post_input(of(5, message::KEY_DOWN));
        queue.post_input(of(5, message::MOUSE_MOVE));
        queue.post_input(of(6, message::LEFT_BUTTON_DOWN));
        assert_eq!(
            queue.status(5),
            status::POST_MESSAGE | status::KEY | status::MOUSE_MOVE
        );
        assert_eq!(queue.status(6), status::HOT_KEY | status::MOUSE_BUTTON);
        queue.quit(7, 0);
        assert_eq!(queue.status(7), status::POST_MESSAGE);
    }

    #[test]
    fn mouse_moves_coalesce_and_full_queues_hold_their_limits() {
        let mut queue = Box::new(Queue::new());
        queue.post_input(message(1, message::MOUSE_MOVE, 1));
        queue.post_input(message(1, message::MOUSE_MOVE, 2));
        queue.post_input(message(2, message::MOUSE_MOVE, 3));
        assert_eq!(queue.take(&ANY, true).unwrap().wparam, 2);
        assert_eq!(queue.take(&ANY, true).unwrap().wparam, 3);
        for index in 0..POSTED_CAPACITY {
            assert!(queue.post(message(1, 0x400, index)));
        }
        assert!(!queue.post(message(1, 0x400, 999)));
        for index in 0..INPUT_CAPACITY + 2 {
            queue.post_input(message(1, message::KEY_DOWN, index));
        }
        queue.forget(1);
        assert_eq!(queue.take(&ANY, false), None);
        for index in 0..INPUT_CAPACITY + 2 {
            queue.post_input(message(1, message::KEY_DOWN, index));
        }
        assert_eq!(queue.take(&ANY, true).unwrap().wparam, 2);
    }
}
