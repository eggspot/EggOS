//! Autorelease pools: each thread has one stack of autoreleased objects, made on first use; a
//! push returns the stack's height as its token and a pop releases, newest first, everything
//! above it. A release may run a `dealloc` that autoreleases more, so a pop takes one object at a
//! time and reads the stack again after each. Objects autoreleased with no pool pushed stay on
//! the stack: a created thread's are released when it ends, the main thread's are left to the
//! end of the process, as on macOS without a pool.

use super::abi::Id;
use super::buffer::Buffer;
use super::reference_count::objc_release;
use crate::libsystem::{heap, thread};

fn stack() -> Option<*mut Buffer<Id>> {
    let thread = thread::current();
    // SAFETY: the calling thread's own control block; only this thread uses its pool stack.
    let slot = unsafe { &mut (*thread).autorelease_pools };
    if *slot == 0 {
        let memory = heap::allocate(core::mem::size_of::<Buffer<Id>>(), true).cast::<Buffer<Id>>();
        if memory.is_null() {
            return None;
        }
        // SAFETY: a new block for the stack, which only this thread uses.
        unsafe { memory.write(Buffer::new()) };
        *slot = memory as usize;
    }
    Some(*slot as *mut Buffer<Id>)
}

fn with_stack<R>(work: impl FnOnce(&mut Buffer<Id>) -> R) -> Option<R> {
    let stack = stack()?;
    // SAFETY: the calling thread's own stack; no reference to it outlives `work`, which never
    // runs program code.
    Some(work(unsafe { &mut *stack }))
}

/// Puts `object` in the calling thread's innermost pool.
pub fn add(object: Id) -> Id {
    with_stack(|stack| stack.push(object));
    object
}

/// Pushes a pool; the token is for [`pop`].
pub fn push() -> usize {
    with_stack(|stack| stack.len()).unwrap_or(0)
}

/// Releases every object autoreleased since the push that returned `token`.
pub fn pop(token: usize) {
    loop {
        let next = with_stack(|stack| {
            if stack.len() > token {
                stack.pop()
            } else {
                None
            }
        })
        .flatten();
        match next {
            Some(object) => objc_release(object),
            None => return,
        }
    }
}

/// Releases a created thread's autoreleased objects when it ends (the thread-exit hook).
pub fn drain_thread() {
    // SAFETY: the calling thread's own control block.
    if unsafe { (*thread::current()).autorelease_pools } == 0 {
        return;
    }
    pop(0);
    with_stack(|stack| stack.free());
}

extern "C" fn objc_autorelease_pool_push() -> usize {
    push()
}

extern "C" fn objc_autorelease_pool_pop(token: usize) {
    pop(token);
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_objc_autoreleasePoolPush" => objc_autorelease_pool_push as *const () as usize,
        b"_objc_autoreleasePoolPop" => objc_autorelease_pool_pop as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}
