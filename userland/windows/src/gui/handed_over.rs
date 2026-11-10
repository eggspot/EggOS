//! Agents' actions on a window of another thread (D227): the thread answering the window
//! server's query hands `focus` and `set-text` to the window's thread as messages of the
//! layer's own, which that thread does inside its next `GetMessageW` or `PeekMessageW`, as
//! Windows delivers a message sent from another thread; the program never sees them. They are
//! posted to [`HANDED_OVER_WINDOW`], a handle no window has, so a program's own
//! `PostThreadMessageW` or `PostMessageW` of the same numbers is never taken for one.

use super::queue::{Filter, Message};

/// Gives the window in `wParam` the focus.
pub const AGENT_FOCUS: u32 = 0x03f8;
/// Sets the text of the edit control in `wParam`; `lParam` is a heap block of NUL-terminated
/// UTF-16 that the receiving thread frees.
pub const AGENT_SET_TEXT: u32 = 0x03f9;

/// The window a handed-over action is posted to: no window has this handle, so only the layer
/// posts to it (`PostMessageW` refuses a handle that is not a window's).
pub const HANDED_OVER_WINDOW: usize = usize::MAX - 1;

/// Whether `message` is an agent's action handed over from another thread.
pub fn is_handed_over(message: &Message) -> bool {
    message.window == HANDED_OVER_WINDOW && matches!(message.message, AGENT_FOCUS | AGENT_SET_TEXT)
}

/// The filter that takes `message`, a handed-over action a read left in its thread's queue,
/// back out: the first of its kind for its thread, which is `message` itself.
pub fn taking(message: &Message) -> Filter {
    Filter {
        window: HANDED_OVER_WINDOW,
        first: message.message,
        last: message.message,
        thread: message.thread,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::queue::Queue;

    const THREAD: u32 = 7;

    fn posted(window: usize, number: u32, wparam: usize, thread: u32) -> Message {
        Message {
            window,
            message: number,
            wparam,
            thread,
            ..Message::default()
        }
    }

    #[test]
    fn only_the_layer_s_own_messages_are_handed_over() {
        assert!(is_handed_over(&posted(
            HANDED_OVER_WINDOW,
            AGENT_FOCUS,
            5,
            THREAD
        )));
        assert!(is_handed_over(&posted(
            HANDED_OVER_WINDOW,
            AGENT_SET_TEXT,
            5,
            THREAD
        )));
        assert!(!is_handed_over(&posted(5, AGENT_FOCUS, 0, THREAD)));
        assert!(!is_handed_over(&posted(
            HANDED_OVER_WINDOW,
            0x0400,
            5,
            THREAD
        )));
    }

    #[test]
    fn a_program_s_thread_message_with_a_handed_over_number_is_its_own() {
        let forged = posted(0, AGENT_SET_TEXT, 5, THREAD);
        assert!(!is_handed_over(&forged));
        let mut queue = Queue::new();
        assert!(queue.post(forged));
        let any = Filter {
            thread: THREAD,
            ..Filter::default()
        };
        let read = queue.take(&any, false).unwrap();
        assert!(!is_handed_over(&read));
        assert_eq!(
            queue.take(
                &taking(&posted(HANDED_OVER_WINDOW, AGENT_SET_TEXT, 0, THREAD)),
                true
            ),
            None
        );
        assert_eq!(queue.take(&any, true), Some(forged));
    }

    #[test]
    fn a_handed_over_action_a_peek_left_queued_is_taken_out_alone() {
        let mut queue = Queue::new();
        let focus = posted(HANDED_OVER_WINDOW, AGENT_FOCUS, 5, THREAD);
        let later_focus = posted(HANDED_OVER_WINDOW, AGENT_FOCUS, 6, THREAD);
        let other_thread = posted(HANDED_OVER_WINDOW, AGENT_FOCUS, 9, THREAD + 1);
        let user = posted(0, 0x0400, 1, THREAD);
        for message in [other_thread, focus, user, later_focus] {
            assert!(queue.post(message));
        }
        let any = Filter {
            thread: THREAD,
            ..Filter::default()
        };
        let peeked = queue.take(&any, false).unwrap();
        assert_eq!(peeked, focus);
        assert!(is_handed_over(&peeked));
        assert_eq!(queue.take(&taking(&peeked), true), Some(focus));
        assert_eq!(queue.take(&any, true), Some(user));
        assert_eq!(queue.take(&any, true), Some(later_focus));
        assert_eq!(queue.take(&any, true), None);
        let theirs = Filter {
            thread: THREAD + 1,
            ..Filter::default()
        };
        assert_eq!(queue.take(&theirs, true), Some(other_thread));
    }
}
