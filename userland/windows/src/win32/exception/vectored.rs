//! Vectored exception and continue handlers (D177): `AddVectoredExceptionHandler` puts a
//! handler first or last in the process's list; each exception goes to every handler in order
//! before any frame's, and one that answers `EXCEPTION_CONTINUE_EXECUTION` resumes the thread.
//! Continue handlers run, in order, whenever an exception is about to be continued. The lists
//! are copied out of their lock before the handlers run, so a handler may add or remove one.

use super::super::SpinLock;
use super::context::Context;
use super::frames::{self, Bridge};
use super::{ExceptionPointers, filter};

const MAX_HANDLERS: usize = 32;

#[derive(Clone, Copy)]
struct Entry {
    id: u64,
    handler: u64,
}

struct List {
    entries: [Entry; MAX_HANDLERS],
    count: usize,
    next_id: u64,
}

impl List {
    const fn new() -> Self {
        Self {
            entries: [Entry { id: 0, handler: 0 }; MAX_HANDLERS],
            count: 0,
            next_id: 0,
        }
    }

    fn add(&mut self, first: bool, handler: u64) -> u64 {
        if self.count == MAX_HANDLERS || handler == 0 {
            return 0;
        }
        self.next_id += 1;
        let entry = Entry {
            id: self.next_id,
            handler,
        };
        let at = if first { 0 } else { self.count };
        self.entries.copy_within(at..self.count, at + 1);
        self.entries[at] = entry;
        self.count += 1;
        entry.id
    }

    fn remove(&mut self, id: u64) -> bool {
        let Some(at) = self.entries[..self.count]
            .iter()
            .position(|entry| entry.id == id)
        else {
            return false;
        };
        self.entries.copy_within(at + 1..self.count, at);
        self.count -= 1;
        true
    }
}

static EXCEPTION_HANDLERS: SpinLock<List> = SpinLock::new(List::new());
static CONTINUE_HANDLERS: SpinLock<List> = SpinLock::new(List::new());

fn snapshot(list: &SpinLock<List>) -> ([Entry; MAX_HANDLERS], usize) {
    list.with(|list| (list.entries, list.count))
}

fn call(list: &SpinLock<List>, pointers: &mut ExceptionPointers, context: *mut Context) -> bool {
    let (entries, count) = snapshot(list);
    for entry in &entries[..count] {
        // SAFETY: the dispatch's `CONTEXT`.
        let resume = unsafe { *context };
        let mut bridge = Bridge::new(Some(resume), None);
        // SAFETY: the program registered `handler` as a vectored handler, which takes the
        // exception's pointers.
        let answer = unsafe {
            frames::call_program(
                &mut bridge,
                entry.handler,
                [(&raw mut *pointers) as u64, 0, 0, 0],
            )
        } as i32;
        if answer == filter::CONTINUE_EXECUTION {
            return true;
        }
    }
    false
}

/// Gives an exception to the vectored exception handlers; true when one resumes the thread.
pub fn call_exception_handlers(pointers: &mut ExceptionPointers, context: *mut Context) -> bool {
    call(&EXCEPTION_HANDLERS, pointers, context)
}

/// Tells the vectored continue handlers an exception is about to be continued.
pub fn call_continue_handlers(pointers: &mut ExceptionPointers, context: *mut Context) {
    call(&CONTINUE_HANDLERS, pointers, context);
}

pub(super) extern "efiapi" fn add_exception_handler(first: u32, handler: u64) -> u64 {
    EXCEPTION_HANDLERS.with(|list| list.add(first != 0, handler))
}

pub(super) extern "efiapi" fn remove_exception_handler(id: u64) -> u32 {
    u32::from(EXCEPTION_HANDLERS.with(|list| list.remove(id)))
}

pub(super) extern "efiapi" fn add_continue_handler(first: u32, handler: u64) -> u64 {
    CONTINUE_HANDLERS.with(|list| list.add(first != 0, handler))
}

pub(super) extern "efiapi" fn remove_continue_handler(id: u64) -> u32 {
    u32::from(CONTINUE_HANDLERS.with(|list| list.remove(id)))
}
