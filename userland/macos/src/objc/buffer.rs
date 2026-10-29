//! A growable array of plain values in the layer's heap, for the runtime's and Foundation's
//! tables and collections.

use crate::libsystem::heap;

/// A growable array of `T`, which must be plain data: nothing is dropped.
#[derive(Debug)]
pub struct Buffer<T: Copy> {
    pointer: *mut T,
    length: usize,
    capacity: usize,
}

// SAFETY: a buffer owns its block; its users reach it through a lock or one thread.
unsafe impl<T: Copy> Send for Buffer<T> {}

#[allow(clippy::len_without_is_empty)]
impl<T: Copy> Buffer<T> {
    pub const fn new() -> Self {
        Self {
            pointer: core::ptr::null_mut(),
            length: 0,
            capacity: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn as_slice(&self) -> &[T] {
        if self.pointer.is_null() {
            return &[];
        }
        // SAFETY: the first `length` values are initialized.
        unsafe { core::slice::from_raw_parts(self.pointer, self.length) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.pointer.is_null() {
            return &mut [];
        }
        // SAFETY: as above, borrowed mutably through `self`.
        unsafe { core::slice::from_raw_parts_mut(self.pointer, self.length) }
    }

    /// Makes room for `more` values; false if there is no memory.
    pub fn reserve(&mut self, more: usize) -> bool {
        let Some(needed) = self.length.checked_add(more) else {
            return false;
        };
        if needed <= self.capacity {
            return true;
        }
        let capacity = needed.max(self.capacity * 2).max(4);
        let Some(bytes) = capacity.checked_mul(core::mem::size_of::<T>()) else {
            return false;
        };
        let grown = heap::reallocate(self.pointer.cast(), bytes).cast::<T>();
        if grown.is_null() {
            return false;
        }
        self.pointer = grown;
        self.capacity = capacity;
        true
    }

    /// Appends `value`; false if there is no memory.
    pub fn push(&mut self, value: T) -> bool {
        if !self.reserve(1) {
            return false;
        }
        // SAFETY: `reserve` made room for one more value.
        unsafe { self.pointer.wrapping_add(self.length).write(value) };
        self.length += 1;
        true
    }

    pub fn pop(&mut self) -> Option<T> {
        let last = self.length.checked_sub(1)?;
        let value = self.as_slice()[last];
        self.length = last;
        Some(value)
    }

    /// Frees the values' block; the buffer is empty afterwards.
    pub fn free(&mut self) {
        heap::release(self.pointer.cast());
        *self = Self::new();
    }
}
