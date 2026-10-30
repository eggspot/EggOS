//! Bytes `PeekNamedPipe` looked at (D189): Linux cannot read a pipe without taking the bytes
//! out of it, so the layer takes them and keeps them here, per descriptor, until `ReadFile`
//! takes them first, before what is still in the pipe.

/// How many bytes one pipe keeps.
pub const CAPACITY: usize = 4096;
/// How many pipes can hold peeked bytes at once.
pub const PIPES: usize = 8;

#[derive(Debug, Clone, Copy)]
struct Peeked {
    fd: usize,
    length: usize,
    bytes: [u8; CAPACITY],
}

/// The peeked bytes of up to [`PIPES`] pipes.
#[derive(Debug)]
pub struct PeekedPipes {
    pipes: [Peeked; PIPES],
}

impl PeekedPipes {
    /// No bytes kept.
    pub const fn new() -> Self {
        Self {
            pipes: [Peeked {
                fd: 0,
                length: 0,
                bytes: [0; CAPACITY],
            }; PIPES],
        }
    }

    fn find(&self, fd: usize) -> Option<usize> {
        self.pipes
            .iter()
            .position(|pipe| pipe.length != 0 && pipe.fd == fd)
    }

    /// Whether no pipe holds peeked bytes.
    pub fn is_empty(&self) -> bool {
        self.pipes.iter().all(|pipe| pipe.length == 0)
    }

    /// The bytes kept for `fd`, oldest first.
    pub fn kept(&self, fd: usize) -> &[u8] {
        match self.find(fd) {
            Some(index) => &self.pipes[index].bytes[..self.pipes[index].length],
            None => &[],
        }
    }

    /// How many more bytes `fd` can keep.
    pub fn room(&self, fd: usize) -> usize {
        match self.find(fd) {
            Some(index) => CAPACITY - self.pipes[index].length,
            None if self.pipes.iter().any(|pipe| pipe.length == 0) => CAPACITY,
            None => 0,
        }
    }

    /// Keeps `bytes` after those `fd` already keeps, as many as there is room for; how many.
    pub fn keep(&mut self, fd: usize, bytes: &[u8]) -> usize {
        let index = match self.find(fd) {
            Some(index) => index,
            None => match self.pipes.iter().position(|pipe| pipe.length == 0) {
                Some(index) => {
                    self.pipes[index].fd = fd;
                    index
                }
                None => return 0,
            },
        };
        let pipe = &mut self.pipes[index];
        let count = bytes.len().min(CAPACITY - pipe.length);
        pipe.bytes[pipe.length..pipe.length + count].copy_from_slice(&bytes[..count]);
        pipe.length += count;
        count
    }

    /// Moves the oldest bytes `fd` keeps into `out`, as many as fit; how many.
    pub fn take(&mut self, fd: usize, out: &mut [u8]) -> usize {
        let Some(index) = self.find(fd) else {
            return 0;
        };
        let pipe = &mut self.pipes[index];
        let count = out.len().min(pipe.length);
        out[..count].copy_from_slice(&pipe.bytes[..count]);
        pipe.bytes.copy_within(count..pipe.length, 0);
        pipe.length -= count;
        count
    }

    /// Drops the bytes `fd` keeps, as when its handle closes.
    pub fn forget(&mut self, fd: usize) {
        if let Some(index) = self.find(fd) {
            self.pipes[index].length = 0;
        }
    }
}

impl Default for PeekedPipes {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peeked_bytes_come_back_first_and_in_order() {
        let mut pipes = Box::new(PeekedPipes::new());
        assert!(pipes.is_empty());
        assert_eq!(pipes.keep(7, b"hello"), 5);
        assert_eq!(pipes.keep(7, b" there"), 6);
        assert_eq!(pipes.kept(7), b"hello there");
        assert_eq!(pipes.kept(8), b"");
        let mut out = [0u8; 4];
        assert_eq!(pipes.take(7, &mut out), 4);
        assert_eq!(&out, b"hell");
        assert_eq!(pipes.kept(7), b"o there");
        let mut rest = [0u8; 32];
        assert_eq!(pipes.take(7, &mut rest), 7);
        assert_eq!(&rest[..7], b"o there");
        assert_eq!(pipes.take(7, &mut rest), 0);
        assert!(pipes.is_empty());
    }

    #[test]
    fn each_pipe_keeps_its_own_bytes_up_to_its_capacity() {
        let mut pipes = Box::new(PeekedPipes::new());
        let full = [1u8; CAPACITY + 10];
        assert_eq!(pipes.keep(3, &full), CAPACITY);
        assert_eq!(pipes.room(3), 0);
        assert_eq!(pipes.keep(3, b"x"), 0);
        for fd in 4..4 + PIPES - 1 {
            assert_eq!(pipes.keep(fd, b"y"), 1);
        }
        assert_eq!(pipes.room(100), 0);
        assert_eq!(pipes.keep(100, b"z"), 0);
        pipes.forget(3);
        assert_eq!(pipes.kept(3), b"");
        assert_eq!(pipes.room(100), CAPACITY);
        assert_eq!(pipes.keep(100, b"z"), 1);
        assert_eq!(pipes.kept(4), b"y");
    }
}
