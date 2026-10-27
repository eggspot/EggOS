//! A stdio stream's buffer and who owns it: a block from the layer's heap, which the stream
//! frees when it closes or gets another buffer, or memory it only borrows (the standard
//! streams' static buffers, or one the program hands `setvbuf`), which it never frees.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Heap,
    Borrowed,
}

/// The memory a stream buffers through.
#[derive(Debug)]
pub struct StreamBuffer {
    pub start: *mut u8,
    pub capacity: usize,
    owner: Owner,
}

impl StreamBuffer {
    /// No buffer.
    pub const fn none() -> Self {
        Self::borrowed(core::ptr::null_mut(), 0)
    }

    /// `capacity` bytes at `start` that someone else owns.
    pub const fn borrowed(start: *mut u8, capacity: usize) -> Self {
        Self {
            start,
            capacity,
            owner: Owner::Borrowed,
        }
    }

    /// A block of the layer's heap of `capacity` bytes, which the stream owns.
    pub const fn heap(start: *mut u8, capacity: usize) -> Self {
        Self {
            start,
            capacity,
            owner: Owner::Heap,
        }
    }

    /// Buffers through `capacity` bytes at `start` that the program owns from now on, and
    /// returns the heap block to free if the stream owned its old buffer.
    pub fn replace_with_borrowed(&mut self, start: *mut u8, capacity: usize) -> Option<*mut u8> {
        let old = core::mem::replace(self, Self::borrowed(start, capacity));
        old.into_owned()
    }

    /// Leaves the stream without a buffer and returns the heap block to free, if it owned one.
    pub fn take_owned(&mut self) -> Option<*mut u8> {
        core::mem::replace(self, Self::none()).into_owned()
    }

    fn into_owned(self) -> Option<*mut u8> {
        (self.owner == Owner::Heap && !self.start.is_null()).then_some(self.start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(bytes: &mut [u8]) -> *mut u8 {
        bytes.as_mut_ptr()
    }

    #[test]
    fn the_layer_s_own_block_is_freed_once_when_replaced_or_closed() {
        let (mut ours, mut theirs) = ([0u8; 16], [0u8; 8]);
        let mut buffer = StreamBuffer::heap(block(&mut ours), 16);
        assert_eq!(
            buffer.replace_with_borrowed(block(&mut theirs), 8),
            Some(block(&mut ours))
        );
        assert_eq!((buffer.start, buffer.capacity), (block(&mut theirs), 8));
        assert_eq!(buffer.take_owned(), None);
        assert_eq!(buffer.take_owned(), None);
        let mut closed = StreamBuffer::heap(block(&mut ours), 16);
        assert_eq!(closed.take_owned(), Some(block(&mut ours)));
        assert_eq!(closed.take_owned(), None);
        assert_eq!((closed.start, closed.capacity), (core::ptr::null_mut(), 0));
    }

    #[test]
    fn borrowed_buffers_are_never_freed() {
        let (mut first, mut second) = ([0u8; 16], [0u8; 8]);
        let mut buffer = StreamBuffer::borrowed(block(&mut first), 16);
        assert_eq!(buffer.replace_with_borrowed(block(&mut second), 8), None);
        assert_eq!(buffer.take_owned(), None);
        assert_eq!(StreamBuffer::none().take_owned(), None);
        assert_eq!(
            StreamBuffer::heap(core::ptr::null_mut(), 0).take_owned(),
            None
        );
    }
}
