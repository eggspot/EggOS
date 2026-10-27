//! The message queue `GetMessageW` and `PeekMessageW` read: messages posted with
//! `PostMessageW`, then input from the window server, then `WM_QUIT` once `PostQuitMessage` asked
//! for it, as Windows orders them (`WM_PAINT` comes from the windows' update regions, after all
//! of these). Mouse moves coalesce: a move replaces the newest input when that is a move of the
//! same window.

use super::message;

/// The most posted messages waiting at once (Windows' quota is larger).
pub const POSTED_CAPACITY: usize = 256;
/// The most input messages waiting at once; past it the oldest is dropped.
pub const INPUT_CAPACITY: usize = 256;

/// One message, as `MSG` holds it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Message {
    pub window: usize,
    pub message: u32,
    pub wparam: usize,
    pub lparam: usize,
    pub time: u32,
    pub point: (i32, i32),
}

/// Which messages a read takes: those of `window` (any when 0, only those posted to no window
/// when [`THREAD_MESSAGES`]) numbered `first..=last` (any when both are 0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Filter {
    pub window: usize,
    pub first: u32,
    pub last: u32,
}

/// The window filter `-1`: messages posted to the thread, not to a window.
pub const THREAD_MESSAGES: usize = usize::MAX;

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
        window && self.accepts_number(message.message)
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
    quit: Option<usize>,
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
            quit: None,
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

    /// `PostQuitMessage(code)`: `WM_QUIT` comes once nothing posted or input is left.
    pub fn quit(&mut self, code: usize) {
        self.quit = Some(code);
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
        let code = self.quit.filter(|_| filter.accepts_number(message::QUIT))?;
        if remove {
            self.quit = None;
        }
        Some(Message {
            message: message::QUIT,
            wparam: code,
            ..Message::default()
        })
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
    };

    #[test]
    fn posted_messages_come_before_input_and_quit_comes_last() {
        let mut queue = Box::new(Queue::new());
        queue.quit(3);
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
        };
        assert_eq!(queue.take(&range, true).unwrap().wparam, 3);
        assert_eq!(queue.take(&range, true), None);
        queue.quit(0);
        assert_eq!(queue.take(&range, true), None);
        assert_eq!(queue.take(&ANY, true).unwrap().wparam, 1);
        assert_eq!(queue.take(&ANY, true).unwrap().message, message::QUIT);
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
