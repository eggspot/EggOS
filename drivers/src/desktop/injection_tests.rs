use super::protocol::{
    DiscardEvents, EventRecord, MAX_WHEEL_NOTCHES, QUEUE_CAPACITY, Request, input_kind, modifier,
    request, size,
};
use super::*;
use crate::input::keymap::{Modifiers, key_for_ascii};

struct Recorded {
    records: Vec<(u16, EventRecord)>,
    room: usize,
}

impl Default for Recorded {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            room: QUEUE_CAPACITY,
        }
    }
}

impl EventSink for Recorded {
    fn send(&mut self, client: u16, record: EventRecord) {
        self.records.push((client, record));
    }

    fn room(&self, _client: u16) -> usize {
        self.room
    }
}

impl Recorded {
    fn take(&mut self) -> Vec<EventRecord> {
        core::mem::take(&mut self.records)
            .into_iter()
            .map(|(_, record)| record)
            .collect()
    }

    fn kinds(&mut self) -> Vec<u16> {
        self.take().into_iter().map(|record| record.kind).collect()
    }
}

const CLIENT: u16 = 3;
const QUIET_MS: u32 = 10_000;

fn shown(desktop: &mut Desktop, width: u32, height: u32) -> WindowId {
    let created = desktop.create(CLIENT, 0, 0, width, height, 0).unwrap();
    desktop
        .commit(
            CLIENT,
            u32::from(created.window),
            &[],
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .unwrap();
    created.window
}

fn desktop_with_two_windows() -> (Desktop, WindowId, WindowId) {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let first = shown(&mut desktop, 300, 200);
    let second = shown(&mut desktop, 300, 200);
    desktop.set_time(QUIET_MS);
    (desktop, first, second)
}

fn send(
    desktop: &mut Desktop,
    window: WindowId,
    input: InjectedInput,
    events: &mut Recorded,
) -> Result<Injected, WindowError> {
    desktop.send_input(u32::from(window), input, events)
}

#[test]
fn the_requests_have_linux_s_encoding_and_parse_their_fields() {
    assert_eq!(request::CAPTURE_WINDOW, 0xc028_7713);
    assert_eq!(request::SEND_INPUT, 0x4018_7714);
    let mut capture = [0u8; size::CAPTURE_WINDOW];
    capture[..4].copy_from_slice(&7u32.to_ne_bytes());
    capture[4..8].copy_from_slice(&3u32.to_ne_bytes());
    capture[8..12].copy_from_slice(&16u32.to_ne_bytes());
    capture[24..32].copy_from_slice(&0x1000u64.to_ne_bytes());
    capture[32..40].copy_from_slice(&4096u64.to_ne_bytes());
    assert_eq!(
        Request::parse(request::CAPTURE_WINDOW, &capture),
        Some(Request::CaptureWindow {
            window: 7,
            first_row: 3,
            row_count: 16,
            buffer: 0x1000,
            length: 4096
        })
    );
    let mut input = [0u8; size::SEND_INPUT];
    for (index, word) in [7u32, input_kind::CLICK, 5, -1i32 as u32, 0x110, 9]
        .into_iter()
        .enumerate()
    {
        input[index * 4..index * 4 + 4].copy_from_slice(&word.to_ne_bytes());
    }
    assert_eq!(
        Request::parse(request::SEND_INPUT, &input),
        Some(Request::SendInput {
            window: 7,
            kind: input_kind::CLICK,
            x: 5,
            y: -1,
            code: 0x110,
            value: 9
        })
    );
    assert_eq!(Request::parse(request::SEND_INPUT, &input[..20]), None);
}

#[test]
fn hostile_input_arguments_are_invalid() {
    use crate::input::button;
    let invalid = Err(WindowError::Invalid);
    for (kind, x, y, code, value) in [
        (99, 0, 0, 0, 0),
        (input_kind::POINTER_MOVE, -1, 0, 0, 0),
        (input_kind::CLICK, 0, i32::MIN, u32::from(button::LEFT), 0),
        (input_kind::CLICK, 0, 0, 0x113, 0),
        (input_kind::CLICK, 0, 0, u32::from(key::A), 0),
        (input_kind::KEY, 0, 0, 0, 0),
        (input_kind::KEY, 0, 0, u32::from(button::LEFT), 0),
        (input_kind::KEY, 0, 0, u32::from(button::FIRST), 0),
        (input_kind::KEY, 0, 0, u32::MAX, 0),
        (
            input_kind::KEY,
            0,
            0,
            u32::from(key::A),
            modifier::CAPS_LOCK,
        ),
        (input_kind::KEY, 0, 0, u32::from(key::A), 1 << 31),
        (input_kind::WHEEL, 0, 0, 0, 0),
        (input_kind::CHARACTER, 0, 0, 0x0d, 0),
        (input_kind::CHARACTER, 0, 0, 0x7f, 0),
        (input_kind::CHARACTER, 0, 0, 0xe9, 0),
        (input_kind::CHARACTER, 0, 0, 0x141, 0),
    ] {
        assert_eq!(
            InjectedInput::from_request(kind, x, y, code, value),
            invalid,
            "{kind} {x} {y} {code:#x} {value:#x}"
        );
    }
    assert_eq!(
        InjectedInput::from_request(input_kind::WHEEL, 1, 2, 0, i32::MIN as u32),
        Ok(InjectedInput::Wheel {
            x: 1,
            y: 2,
            notches: -MAX_WHEEL_NOTCHES
        })
    );
    assert_eq!(
        InjectedInput::from_request(input_kind::KEY, 0, 0, 31, modifier::CTRL | modifier::SHIFT),
        Ok(InjectedInput::Key {
            code: 31,
            modifiers: Modifiers::CTRL.with(Modifiers::SHIFT)
        })
    );
    assert_eq!(
        InjectedInput::from_request(input_kind::CHARACTER, 0, 0, u32::from(b'~'), 0),
        Ok(InjectedInput::Character(b'~'))
    );
}

#[test]
fn a_click_goes_straight_to_its_window_bracketed_and_changes_nothing_else() {
    let (mut desktop, first, second) = desktop_with_two_windows();
    let mut events = Recorded::default();
    desktop.pointer_moved(5, 5, &mut Damage::new(), &mut events);
    desktop.set_time(QUIET_MS * 2);
    events.take();
    let focused = desktop.focused_window();
    assert_eq!(focused, Some(second));
    let stacking: Vec<WindowId> = desktop.stacking().collect();
    let click = InjectedInput::Click {
        x: 10,
        y: 20,
        button: crate::input::button::LEFT,
    };
    assert_eq!(
        send(&mut desktop, first, click, &mut events),
        Ok(Injected::Queued)
    );
    let records = events.take();
    let kinds: Vec<u16> = records.iter().map(|record| record.kind).collect();
    assert_eq!(
        kinds,
        [
            event::FOCUS_IN,
            event::POINTER_ENTER,
            event::POINTER_MOVE,
            event::BUTTON,
            event::BUTTON,
            event::POINTER_LEAVE,
            event::FOCUS_OUT
        ]
    );
    assert!(records.iter().all(|record| record.window == first));
    assert_eq!(
        (records[3].a, records[3].b, records[3].c, records[3].d),
        (10, 20, 0x110, 1)
    );
    assert_eq!(records[4].d, 0);
    assert_eq!(desktop.focused_window(), focused);
    assert_eq!(desktop.stacking().collect::<Vec<_>>(), stacking);
    assert_eq!(desktop.pointer, (5, 5));
    assert!(desktop.hover.is_none());
    assert_eq!(
        send(&mut desktop, second, click, &mut events),
        Ok(Injected::Queued)
    );
    assert_eq!(
        events.kinds(),
        [
            event::POINTER_ENTER,
            event::POINTER_MOVE,
            event::BUTTON,
            event::BUTTON,
            event::POINTER_LEAVE
        ],
        "no focus bracket for the focused window"
    );
}

#[test]
fn a_key_brings_its_modifiers_down_and_up_around_it() {
    let (mut desktop, _, second) = desktop_with_two_windows();
    let mut events = Recorded::default();
    let held = Modifiers::CTRL.with(Modifiers::SHIFT);
    let input = InjectedInput::Key {
        code: 20,
        modifiers: held,
    };
    send(&mut desktop, second, input, &mut events).unwrap();
    let keys: Vec<(i32, i32, u32)> = events
        .take()
        .into_iter()
        .map(|record| (record.a, record.b, record.c))
        .collect();
    let (shift, ctrl) = (i32::from(key::LEFT_SHIFT), i32::from(key::LEFT_CTRL));
    let both = modifier::SHIFT | modifier::CTRL;
    assert_eq!(
        keys,
        [
            (shift, 1, modifier::SHIFT),
            (ctrl, 1, both),
            (20, 1, both),
            (20, 0, both),
            (ctrl, 0, modifier::SHIFT),
            (shift, 0, 0)
        ]
    );
    send(
        &mut desktop,
        second,
        InjectedInput::Character(b'A'),
        &mut events,
    )
    .unwrap();
    let records = events.take();
    assert_eq!(records.len(), 4);
    assert_eq!(
        (records[1].a, records[1].d),
        (i32::from(key::A), u32::from(b'A'))
    );
    send(
        &mut desktop,
        second,
        InjectedInput::Key {
            code: key::LEFT_SHIFT,
            modifiers: Modifiers::NONE,
        },
        &mut events,
    )
    .unwrap();
    assert_eq!(events.kinds(), [event::KEY, event::KEY]);
}

#[test]
fn minimised_and_covered_windows_still_get_input() {
    let (mut desktop, first, second) = desktop_with_two_windows();
    let mut events = Recorded::default();
    desktop
        .set_state(
            CLIENT,
            u32::from(second),
            1,
            &mut Damage::new(),
            &mut events,
        )
        .unwrap();
    events.take();
    for window in [first, second] {
        assert_eq!(
            send(
                &mut desktop,
                window,
                InjectedInput::PointerMove { x: 1, y: 1 },
                &mut events
            ),
            Ok(Injected::Queued)
        );
        assert!(events.take().iter().all(|record| record.window == window));
    }
}

#[test]
fn the_terminal_s_input_comes_back_for_its_own_handlers() {
    let (mut desktop, _, _) = desktop_with_two_windows();
    let terminal = desktop.terminal_window();
    let mut events = Recorded::default();
    let key = InjectedInput::Key {
        code: key::LEFT_SHIFT,
        modifiers: Modifiers::NONE,
    };
    assert_eq!(
        send(&mut desktop, terminal, key, &mut events),
        Ok(Injected::ForTerminal(TerminalInput::Key {
            code: key::LEFT_SHIFT,
            modifiers: Modifiers::NONE
        }))
    );
    assert_eq!(
        send(
            &mut desktop,
            terminal,
            InjectedInput::Character(b'x'),
            &mut events
        ),
        Ok(Injected::ForTerminal(TerminalInput::Character(b'x')))
    );
    let wheel = InjectedInput::Wheel {
        x: 0,
        y: 0,
        notches: -2,
    };
    assert_eq!(
        send(&mut desktop, terminal, wheel, &mut events),
        Ok(Injected::ForTerminal(TerminalInput::Scroll { notches: -2 }))
    );
    let click = InjectedInput::Click {
        x: 1,
        y: 1,
        button: crate::input::button::LEFT,
    };
    assert_eq!(
        send(&mut desktop, terminal, click, &mut events),
        Err(WindowError::Unsupported)
    );
    assert!(events.records.is_empty());
    assert_eq!(
        desktop.content_layout(u32::from(terminal)),
        Err(WindowError::Unsupported)
    );
}

#[test]
fn points_outside_the_content_and_unknown_windows_are_refused() {
    let (mut desktop, first, _) = desktop_with_two_windows();
    let content = desktop.content_rect(first).unwrap();
    let mut events = Recorded::default();
    for (x, y) in [(content.width, 0), (0, content.height)] {
        assert_eq!(
            send(
                &mut desktop,
                first,
                InjectedInput::PointerMove { x, y },
                &mut events
            ),
            Err(WindowError::Invalid)
        );
    }
    let corner = InjectedInput::PointerMove {
        x: content.width - 1,
        y: content.height - 1,
    };
    assert!(send(&mut desktop, first, corner, &mut events).is_ok());
    assert_eq!(
        desktop.send_input(u32::from(first) + (1 << 6), corner, &mut events),
        Err(WindowError::NoSuchWindow)
    );
    assert_eq!(
        desktop.send_input(u32::MAX, corner, &mut events),
        Err(WindowError::NoSuchWindow)
    );
}

#[test]
fn a_person_wins_while_holding_a_button_and_for_half_a_second_after() {
    let (mut desktop, first, _) = desktop_with_two_windows();
    let mut events = Recorded::default();
    let input = InjectedInput::Character(b'a');
    let at = QUIET_MS;
    desktop.set_time(at);
    desktop.key(
        key::A,
        true,
        Modifiers::NONE,
        &mut Damage::new(),
        &mut events,
    );
    desktop.key(
        key::A,
        false,
        Modifiers::NONE,
        &mut Damage::new(),
        &mut events,
    );
    for (now, busy) in [(at, true), (at + 499, true), (at + 500, false)] {
        desktop.set_time(now);
        assert_eq!(
            send(&mut desktop, first, input, &mut events) == Err(WindowError::Busy),
            busy,
            "{now}"
        );
    }
    desktop.pointer_moved(700, 400, &mut Damage::new(), &mut events);
    desktop.set_time(at + 1100);
    desktop.pointer_moved(703, 397, &mut Damage::new(), &mut events);
    assert!(!desktop.person_is_active(), "jitter under the threshold");
    desktop.pointer_moved(705, 400, &mut Damage::new(), &mut events);
    assert!(desktop.person_is_active());
    desktop.set_time(at + 2000);
    desktop.button(
        crate::input::button::LEFT,
        true,
        &mut Damage::new(),
        &mut events,
    );
    desktop.set_time(at + 2600);
    assert_eq!(
        send(&mut desktop, first, input, &mut events),
        Err(WindowError::Busy),
        "the button is still down"
    );
    desktop.button(
        crate::input::button::LEFT,
        false,
        &mut Damage::new(),
        &mut events,
    );
    desktop.set_time(at + 3000);
    assert!(desktop.person_is_active(), "the release counts as input");
    desktop.set_time(at + 3200);
    assert!(!desktop.person_is_active());
    events.take();
    desktop.set_time(10);
    desktop.wheel(1, &mut events);
    desktop.set_time(u32::MAX - 100);
    desktop.wheel(1, &mut events);
    desktop.set_time(200);
    assert!(desktop.person_is_active(), "across the clock's wrap");
    desktop.set_time(500);
    assert!(!desktop.person_is_active());
}

#[test]
fn a_step_that_does_not_fit_the_queue_sends_nothing() {
    let (mut desktop, first, _) = desktop_with_two_windows();
    let mut events = Recorded {
        room: 6,
        ..Recorded::default()
    };
    let click = InjectedInput::Click {
        x: 1,
        y: 1,
        button: crate::input::button::LEFT,
    };
    assert_eq!(
        send(&mut desktop, first, click, &mut events),
        Err(WindowError::QueueFull)
    );
    assert!(events.records.is_empty());
    events.room = 7;
    assert_eq!(
        send(&mut desktop, first, click, &mut events),
        Ok(Injected::Queued)
    );
    assert_eq!(events.records.len(), 7);
    let mut queue = EventQueue::new().unwrap();
    for _ in 0..QUEUE_CAPACITY - 2 {
        queue.push(EventRecord::new(event::KEY, 1, 0, 0, 0, 0));
    }
    assert_eq!(queue.room(), 2);
}

#[test]
fn a_capture_reads_the_content_at_its_size_with_the_fill_around_the_buffer() {
    let (mut desktop, first, _) = desktop_with_two_windows();
    let layout = desktop.content_layout(u32::from(first)).unwrap();
    let content = desktop.content_rect(first).unwrap();
    assert_eq!(
        (layout.width, layout.height),
        (content.width, content.height)
    );
    desktop
        .manage(
            u32::from(first),
            6,
            400,
            250,
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .unwrap();
    let layout = desktop.content_layout(u32::from(first)).unwrap();
    let content = desktop.content_rect(first).unwrap();
    assert_eq!(
        (layout.width, layout.height),
        (content.width, content.height)
    );
    assert_eq!(
        desktop.content_layout(u32::MAX),
        Err(WindowError::NoSuchWindow)
    );
    let partial = ContentLayout {
        width: 10,
        height: 4,
        buffer: Rect::new(2, 1, 5, 2),
        fill: 0x00aa_aaaa,
    };
    let mut row = [0u32; 10];
    let mut asked = Vec::new();
    partial.copy_row(0, 1, &mut row, &mut |x, y, out| {
        asked.push((x, y, out.len()));
        out.fill(0x0011_1111);
    });
    assert_eq!(asked, [(0, 0, 5)]);
    let fill = 0x00aa_aaaa;
    let buffer = 0x0011_1111;
    assert_eq!(
        row,
        [
            fill, fill, buffer, buffer, buffer, buffer, buffer, fill, fill, fill
        ]
    );
    partial.copy_row(0, 3, &mut row, &mut |_, _, _| panic!("outside the buffer"));
    assert_eq!(row, [fill; 10]);
    let mut tail = [0u32; 4];
    partial.copy_row(5, 2, &mut tail, &mut |x, y, out| {
        assert_eq!((x, y, out.len()), (3, 1, 2));
        out.fill(buffer);
    });
    assert_eq!(tail, [buffer, buffer, fill, fill]);
}

#[test]
fn every_printable_character_has_a_key_that_types_it() {
    for character in b' '..=b'~' {
        let (code, modifiers) = key_for_ascii(character).unwrap();
        assert_eq!(
            codepoint(code, modifiers),
            u32::from(character),
            "{character}"
        );
    }
    assert_eq!(key_for_ascii(b'\r'), None);
    assert_eq!(key_for_ascii(0x80), None);
    assert_eq!(key_for_ascii(b'1'), Some((2, Modifiers::NONE)));
}
