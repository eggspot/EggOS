use super::*;
use crate::tests::TestPlatform;
use eggos_drivers::input::{AxisRange, key};
use std::sync::Arc;

#[derive(Debug, Default)]
struct Recorded {
    bytes: Vec<u8>,
    events: Vec<InputEvent>,
    views: Vec<ViewMove>,
    full: bool,
}

impl InputSink for Recorded {
    fn bytes(&mut self, bytes: &[u8]) {
        assert!(!self.full, "bytes were sent to a full sink");
        self.bytes.extend_from_slice(bytes);
    }
    fn room(&self) -> usize {
        if self.full {
            KeyBytes::CAPACITY - 1
        } else {
            usize::MAX
        }
    }
    fn event(&mut self, event: InputEvent) {
        self.events.push(event);
    }
    fn view(&mut self, change: ViewMove) {
        self.views.push(change);
    }
}

const SCREEN: Option<(u32, u32)> = Some((1280, 800));

struct Device {
    capabilities: Capabilities,
    report: PendingReport,
}

impl Device {
    fn keyboard() -> Self {
        Self::with(Capabilities {
            keyboard: true,
            ..Capabilities::default()
        })
    }

    fn mouse() -> Self {
        Self::with(Capabilities {
            buttons: true,
            relative: true,
            wheel: true,
            ..Capabilities::default()
        })
    }

    fn tablet() -> Self {
        let range = AxisRange { min: 0, max: 32767 };
        Self::with(Capabilities {
            buttons: true,
            wheel: true,
            absolute: Some((range, range)),
            ..Capabilities::default()
        })
    }

    fn with(capabilities: Capabilities) -> Self {
        Self {
            capabilities,
            report: PendingReport::default(),
        }
    }
}

fn timing(now: u64) -> Timing {
    Timing {
        now,
        repeat_delay: 400,
        repeat_interval: 40,
    }
}

fn send(
    state: &mut InputState,
    device: &mut Device,
    events: &[RawEvent],
    screen: Option<(u32, u32)>,
    now: u64,
) -> Recorded {
    let mut recorded = Recorded::default();
    for &event in events {
        state.translate(
            event,
            &device.capabilities,
            &mut device.report,
            screen,
            timing(now),
            &mut recorded,
        );
    }
    recorded
}

fn press(code: u16) -> [RawEvent; 2] {
    [RawEvent::new(event_type::KEY, code, 1), SYN]
}

fn release(code: u16) -> [RawEvent; 2] {
    [RawEvent::new(event_type::KEY, code, 0), SYN]
}

fn tap(code: u16) -> Vec<RawEvent> {
    [press(code), release(code)].concat()
}

const SYN: RawEvent = RawEvent::new(event_type::SYN, SYN_REPORT, 0);

fn key_event(code: u16, pressed: bool, modifiers: Modifiers) -> InputEvent {
    InputEvent::Key {
        code,
        pressed,
        modifiers,
    }
}

#[test]
fn typing_a_line_sends_its_bytes_and_queues_each_key() {
    let mut state = InputState::new();
    let mut keyboard = Device::keyboard();
    let typed: Vec<RawEvent> = [35, 23, key::ENTER].into_iter().flat_map(tap).collect();
    let recorded = send(&mut state, &mut keyboard, &typed, SCREEN, 0);
    assert_eq!(recorded.bytes, b"hi\r");
    assert_eq!(
        recorded.events,
        [
            key_event(35, true, Modifiers::NONE),
            key_event(35, false, Modifiers::NONE),
            key_event(23, true, Modifiers::NONE),
            key_event(23, false, Modifiers::NONE),
            key_event(key::ENTER, true, Modifiers::NONE),
            key_event(key::ENTER, false, Modifiers::NONE),
        ]
    );
    assert!(!state.anything_held());
}

#[test]
fn modifiers_change_the_bytes_and_ride_along_in_key_events() {
    let mut state = InputState::new();
    let mut keyboard = Device::keyboard();
    let mut events = press(key::LEFT_SHIFT).to_vec();
    events.extend(tap(key::A));
    events.extend(release(key::LEFT_SHIFT));
    events.extend(press(key::RIGHT_CTRL));
    events.extend(tap(46));
    events.extend(release(key::RIGHT_CTRL));
    events.extend(tap(key::UP));
    let recorded = send(&mut state, &mut keyboard, &events, SCREEN, 0);
    assert_eq!(recorded.bytes, b"A\x03\x1b[A");
    assert_eq!(
        recorded.events[0],
        key_event(key::LEFT_SHIFT, true, Modifiers::SHIFT)
    );
    assert_eq!(
        recorded.events[1],
        key_event(key::A, true, Modifiers::SHIFT)
    );
    assert_eq!(
        recorded.events[3],
        key_event(key::LEFT_SHIFT, false, Modifiers::NONE)
    );
    assert_eq!(recorded.events[5], key_event(46, true, Modifiers::CTRL));
}

#[test]
fn a_held_key_repeats_after_400_ms_then_every_40_ms_until_released() {
    let mut state = InputState::new();
    let mut keyboard = Device::keyboard();
    let pressed = send(&mut state, &mut keyboard, &press(key::A), SCREEN, 1000);
    assert_eq!(pressed.bytes, b"a");
    assert_eq!(state.next_repeat(), Some(1400));
    let mut repeated = Recorded::default();
    for now in [1399, 1400, 1439, 1440, 1480] {
        state.repeat_key(timing(now), &mut repeated);
    }
    assert_eq!(repeated.bytes, b"aaa");
    assert!(repeated.events.is_empty());
    send(&mut state, &mut keyboard, &release(key::A), SCREEN, 1500);
    assert_eq!(state.next_repeat(), None);
    state.repeat_key(timing(5000), &mut repeated);
    assert_eq!(repeated.bytes, b"aaa");
}

#[test]
fn modifiers_do_not_repeat_and_another_press_stops_a_repeat() {
    let mut state = InputState::new();
    let mut keyboard = Device::keyboard();
    send(&mut state, &mut keyboard, &press(key::A), SCREEN, 0);
    send(
        &mut state,
        &mut keyboard,
        &press(key::LEFT_SHIFT),
        SCREEN,
        100,
    );
    assert_eq!(state.next_repeat(), None);
    assert!(state.anything_held());
    let mut repeated = Recorded::default();
    state.repeat_key(timing(1000), &mut repeated);
    assert!(repeated.bytes.is_empty());
}

#[test]
fn a_host_s_autorepeat_presses_are_ignored() {
    let mut state = InputState::new();
    let mut keyboard = Device::keyboard();
    let mut events = press(key::A).to_vec();
    events.extend(press(key::A));
    events.push(RawEvent::new(event_type::KEY, key::A, 2));
    events.extend(release(key::A));
    let recorded = send(&mut state, &mut keyboard, &events, SCREEN, 0);
    assert_eq!(recorded.bytes, b"a");
    assert_eq!(recorded.events.len(), 2);
}

#[test]
fn a_tablet_moves_the_pointer_to_the_scaled_position_at_the_end_of_its_report() {
    let mut state = InputState::new();
    let mut tablet = Device::tablet();
    let report = [
        RawEvent::new(event_type::ABS, absolute::X, 32767),
        RawEvent::new(event_type::ABS, absolute::Y, 16384),
    ];
    let partial = send(&mut state, &mut tablet, &report, SCREEN, 0);
    assert!(partial.events.is_empty());
    let ended = send(&mut state, &mut tablet, &[SYN], SCREEN, 0);
    assert_eq!(ended.events, [InputEvent::PointerMove { x: 1279, y: 400 }]);
    let only_x = [RawEvent::new(event_type::ABS, absolute::X, 0), SYN];
    let moved = send(&mut state, &mut tablet, &only_x, SCREEN, 0);
    assert_eq!(moved.events, [InputEvent::PointerMove { x: 0, y: 400 }]);
    let same = send(&mut state, &mut tablet, &only_x, SCREEN, 0);
    assert!(same.events.is_empty());
}

#[test]
fn a_mouse_moves_the_pointer_by_its_motion_clamped_to_the_screen() {
    let mut state = InputState::new();
    let mut mouse = Device::mouse();
    let motion = |dx, dy| {
        [
            RawEvent::new(event_type::REL, relative::X, dx),
            RawEvent::new(event_type::REL, relative::Y, dy),
            SYN,
        ]
    };
    let moved = send(&mut state, &mut mouse, &motion(30, 20), SCREEN, 0);
    assert_eq!(moved.events, [InputEvent::PointerMove { x: 30, y: 20 }]);
    let clamped = send(&mut state, &mut mouse, &motion(-100, 5000), SCREEN, 0);
    assert_eq!(clamped.events, [InputEvent::PointerMove { x: 0, y: 799 }]);
    let far = send(
        &mut state,
        &mut mouse,
        &motion(i32::MAX, i32::MIN),
        SCREEN,
        0,
    );
    assert_eq!(far.events, [InputEvent::PointerMove { x: 1279, y: 0 }]);
}

#[test]
fn buttons_follow_their_report_s_motion_and_the_wheel_comes_last() {
    let mut state = InputState::new();
    let mut mouse = Device::mouse();
    let report = [
        RawEvent::new(event_type::KEY, button::LEFT, 1),
        RawEvent::new(event_type::REL, relative::WHEEL, 1),
        RawEvent::new(event_type::REL, relative::X, 5),
        RawEvent::new(event_type::KEY, button::RIGHT, 1),
        SYN,
    ];
    let recorded = send(&mut state, &mut mouse, &report, SCREEN, 0);
    assert_eq!(
        recorded.events,
        [
            InputEvent::PointerMove { x: 5, y: 0 },
            InputEvent::Button {
                button: Button::Left,
                pressed: true
            },
            InputEvent::Button {
                button: Button::Right,
                pressed: true
            },
            InputEvent::Wheel { delta: 1 },
        ]
    );
    assert!(state.anything_held());
    let again = [RawEvent::new(event_type::KEY, button::LEFT, 1), SYN];
    assert!(
        send(&mut state, &mut mouse, &again, SCREEN, 0)
            .events
            .is_empty()
    );
    let up = [
        RawEvent::new(event_type::KEY, button::LEFT, 0),
        RawEvent::new(event_type::KEY, button::RIGHT, 0),
        RawEvent::new(event_type::KEY, 0x150, 0),
        RawEvent::new(event_type::REL, relative::WHEEL, -2),
        SYN,
    ];
    assert_eq!(
        send(&mut state, &mut mouse, &up, SCREEN, 0).events,
        [
            InputEvent::Button {
                button: Button::Left,
                pressed: false
            },
            InputEvent::Button {
                button: Button::Right,
                pressed: false
            },
            InputEvent::Wheel { delta: -2 },
        ]
    );
    assert!(!state.anything_held());
}

fn middle(pressed: bool) -> InputEvent {
    InputEvent::Button {
        button: Button::Middle,
        pressed,
    }
}

#[test]
fn a_click_in_one_report_or_in_two_arrives_as_a_press_then_a_release() {
    let mut state = InputState::new();
    let mut tablet = Device::tablet();
    let down = RawEvent::new(event_type::KEY, button::MIDDLE, 1);
    let up = RawEvent::new(event_type::KEY, button::MIDDLE, 0);
    for click in [&[down, SYN, up, SYN][..], &[down, up, SYN]] {
        let recorded = send(&mut state, &mut tablet, click, SCREEN, 0);
        assert_eq!(recorded.events, [middle(true), middle(false)]);
        assert!(recorded.bytes.is_empty() && !state.anything_held());
    }
    let held = send(
        &mut state,
        &mut tablet,
        &[down, SYN, up, down, SYN],
        SCREEN,
        0,
    );
    assert_eq!(held.events, [middle(true)]);
    assert!(state.anything_held());
}

#[test]
fn without_a_screen_buttons_and_the_wheel_arrive_but_no_motion() {
    let mut state = InputState::new();
    let mut mouse = Device::mouse();
    let report = [
        RawEvent::new(event_type::REL, relative::X, 5),
        RawEvent::new(event_type::KEY, button::LEFT, 1),
        RawEvent::new(event_type::REL, relative::WHEEL, 1),
        SYN,
    ];
    let recorded = send(&mut state, &mut mouse, &report, None, 0);
    assert_eq!(
        recorded.events,
        [
            InputEvent::Button {
                button: Button::Left,
                pressed: true
            },
            InputEvent::Wheel { delta: 1 },
        ]
    );
}

#[test]
fn polling_is_fast_while_active_slow_when_idle_and_never_later_than_a_repeat() {
    let frequency = 1000;
    let mut state = InputState::new();
    assert_eq!(next_poll(&state, 5000, frequency), 5000 + IDLE_POLL_MS);
    let mut keyboard = Device::keyboard();
    send(
        &mut state,
        &mut keyboard,
        &tap(key::LEFT_SHIFT),
        SCREEN,
        5000,
    );
    assert_eq!(next_poll(&state, 5000, frequency), 5000 + ACTIVE_POLL_MS);
    let later = 5000 + ACTIVE_WINDOW_MS;
    assert_eq!(next_poll(&state, later, frequency), later + IDLE_POLL_MS);
    send(&mut state, &mut keyboard, &press(key::A), SCREEN, 10_000);
    let held = 10_000 + 5 * ACTIVE_WINDOW_MS;
    assert_eq!(next_poll(&state, held, frequency), 10_400);
    assert_eq!(next_poll(&state, 10_399, frequency), 10_400);
    assert_eq!(next_poll(&state, 10_390, frequency), 10_398);
}

#[test]
fn every_event_packs_into_a_word_and_back() {
    let events = [
        key_event(0, false, Modifiers::NONE),
        key_event(key::MAX, true, Modifiers::from_bits(31)),
        InputEvent::PointerMove { x: 0, y: 0 },
        InputEvent::PointerMove {
            x: 0xff_ffff,
            y: 0xff_ffff,
        },
        InputEvent::Button {
            button: Button::Middle,
            pressed: true,
        },
        InputEvent::Button {
            button: Button::Right,
            pressed: false,
        },
        InputEvent::Wheel { delta: i32::MIN },
        InputEvent::Wheel { delta: -1 },
        InputEvent::Wheel { delta: i32::MAX },
    ];
    for event in events {
        assert_eq!(InputEvent::unpack(event.pack()), Some(event));
    }
    let huge = InputEvent::PointerMove {
        x: u32::MAX,
        y: 1 << 24,
    };
    assert_eq!(
        InputEvent::unpack(huge.pack()),
        Some(InputEvent::PointerMove {
            x: 0xff_ffff,
            y: 0xff_ffff
        })
    );
    assert_eq!(InputEvent::unpack(0), None);
    assert_eq!(InputEvent::unpack(5), None);
    assert_eq!(InputEvent::unpack(BUTTON | 3 << 8), None);
}

fn wheel(delta: i32) -> InputEvent {
    InputEvent::Wheel { delta }
}

#[test]
fn the_queue_hands_events_out_in_order_and_drops_and_counts_the_oldest_when_full() {
    let queue = EventQueue::<4>::new();
    assert!(queue.is_empty() && queue.pop().is_none());
    let kept: Vec<bool> = (0..6).map(|delta| queue.push(wheel(delta))).collect();
    assert_eq!(kept, [true, true, true, true, false, false]);
    assert_eq!(queue.len(), 4);
    assert_eq!(queue.dropped(), 2);
    let popped: Vec<_> = std::iter::from_fn(|| queue.pop()).collect();
    assert_eq!(popped, [wheel(2), wheel(3), wheel(4), wheel(5)]);
    for round in 0..1000 {
        assert!(queue.push(wheel(round)));
        assert!(queue.push(wheel(-round)));
        assert_eq!(queue.pop(), Some(wheel(round)));
        assert_eq!(queue.pop(), Some(wheel(-round)));
    }
    assert_eq!(queue.dropped(), 2);
    assert!(queue.is_empty());
}

#[test]
fn consumers_on_other_threads_take_each_event_once_and_the_rest_are_counted_dropped() {
    const EVENTS: i32 = 200_000;
    let queue = Arc::new(EventQueue::<64>::new());
    let done = Arc::new(AtomicBool::new(false));
    let consumers: Vec<_> = (0..3)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let done = Arc::clone(&done);
            std::thread::spawn(move || {
                let mut taken = Vec::new();
                loop {
                    match queue.pop() {
                        Some(InputEvent::Wheel { delta }) => taken.push(delta),
                        Some(other) => panic!("{other:?} was never pushed"),
                        None if done.load(Ordering::Acquire) => break,
                        None => std::hint::spin_loop(),
                    }
                }
                taken
            })
        })
        .collect();
    for delta in 0..EVENTS {
        if delta % 2 == 0 {
            while queue.len() >= 64 {
                std::hint::spin_loop();
            }
        }
        queue.push(wheel(delta));
    }
    done.store(true, Ordering::Release);
    let mut taken: Vec<i32> = consumers
        .into_iter()
        .flat_map(|consumer| {
            let taken = consumer.join().unwrap();
            assert!(taken.windows(2).all(|pair| pair[0] < pair[1]));
            taken
        })
        .collect();
    while let Some(InputEvent::Wheel { delta }) = queue.pop() {
        taken.push(delta);
    }
    taken.sort_unstable();
    let before = taken.len();
    taken.dedup();
    assert_eq!(taken.len(), before, "an event was taken twice");
    assert_eq!(taken.len() as u64 + queue.dropped(), EVENTS as u64);
    assert!(taken.iter().all(|delta| (0..EVENTS).contains(delta)));
}

#[test]
fn a_repeat_waits_for_room_for_its_whole_sequence() {
    let mut state = InputState::new();
    let mut keyboard = Device::keyboard();
    send(&mut state, &mut keyboard, &press(key::UP), SCREEN, 0);
    let mut full = Recorded {
        full: true,
        ..Recorded::default()
    };
    state.repeat_key(timing(400), &mut full);
    assert!(full.bytes.is_empty());
    assert_eq!(state.next_repeat(), Some(400));
    let mut room = Recorded::default();
    state.repeat_key(timing(401), &mut room);
    assert_eq!(room.bytes, b"\x1b[A");
    state.set_cursor_keys(CursorKeys::Application);
    state.repeat_key(timing(441), &mut room);
    assert_eq!(room.bytes, b"\x1b[A\x1bOA");
}

#[test]
fn shift_with_the_page_keys_and_the_wheel_move_the_scrollback_view() {
    let mut state = InputState::new();
    let mut keyboard = Device::keyboard();
    send(
        &mut state,
        &mut keyboard,
        &press(key::LEFT_SHIFT),
        SCREEN,
        0,
    );
    let mut moves = Vec::new();
    for code in [key::PAGE_UP, key::PAGE_DOWN, key::HOME, key::END] {
        let sent = send(&mut state, &mut keyboard, &tap(code), SCREEN, 0);
        assert!(sent.bytes.is_empty());
        moves.extend(sent.views);
    }
    assert_eq!(
        moves,
        [
            ViewMove::PageBack,
            ViewMove::PageForward,
            ViewMove::Oldest,
            ViewMove::Live
        ]
    );
    send(
        &mut state,
        &mut keyboard,
        &release(key::LEFT_SHIFT),
        SCREEN,
        0,
    );
    let unshifted = send(&mut state, &mut keyboard, &tap(key::PAGE_UP), SCREEN, 0);
    assert_eq!(unshifted.bytes, b"\x1b[5~");
    assert!(unshifted.views.is_empty());
    let mut mouse = Device::mouse();
    let wheel_events = [
        RawEvent::new(event_type::REL, relative::WHEEL, 2),
        SYN,
        RawEvent::new(event_type::REL, relative::WHEEL, -1),
        SYN,
    ];
    let wheeled = send(&mut state, &mut mouse, &wheel_events, SCREEN, 0);
    assert_eq!(
        wheeled.views,
        [
            ViewMove::Back(2 * WHEEL_LINES),
            ViewMove::Forward(WHEEL_LINES)
        ]
    );
}

struct ScriptedKeyboard {
    events: Vec<RawEvent>,
}

impl InputDevice for ScriptedKeyboard {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            keyboard: true,
            ..Capabilities::default()
        }
    }

    fn poll(&mut self, budget: usize, event: &mut dyn FnMut(RawEvent)) -> usize {
        let taken = budget.min(self.events.len());
        for raw in self.events.drain(..taken) {
            event(raw);
        }
        taken
    }
}

#[test]
fn an_installed_keyboard_s_keys_reach_the_console_and_the_event_queue() {
    let _interrupts = crate::interrupt::lock_timer_interrupts_for_test();
    let _kernel = crate::thread::host::lock_kernel_state_for_test();
    console::set_settings::<TestPlatform>(console::line_discipline::Termios::SERIAL_CONSOLE, true);
    crate::tests::console_output();
    let line: Vec<RawEvent> = [35, 23]
        .into_iter()
        .chain(std::iter::repeat_n(key::A, 40))
        .chain([key::BACKSPACE, key::ENTER])
        .flat_map(tap)
        .collect();
    let keys = line.len() / 2;
    assert!(has_room());
    install(Box::new(ScriptedKeyboard { events: line })).unwrap_or_else(|_| panic!("no room"));
    assert!(summary().keyboards >= 1);
    subscribe();
    while next_event().is_some() {}
    let dropped = dropped_events();
    for _ in 0..2 {
        assert!(poll::<TestPlatform>().is_some());
    }
    let mut buffer = [0u8; 64];
    let read = console::read_input(&mut buffer);
    let mut expected = b"hi".to_vec();
    expected.extend([b'a'; 39]);
    expected.push(b'\n');
    assert_eq!(&buffer[..read], expected);
    let echo = crate::tests::console_output();
    assert!(echo.starts_with(b"hiaaa") && echo.ends_with(b"a\x08 \x08\n"));
    let queued: Vec<_> = std::iter::from_fn(next_event).collect();
    assert_eq!(
        queued.len() as u64 + dropped_events() - dropped,
        keys as u64
    );
    assert_eq!(queued[0], key_event(35, true, Modifiers::NONE));
    console::flush_input();
}

struct NeverDry {
    polls: Arc<std::sync::atomic::AtomicUsize>,
}

impl InputDevice for NeverDry {
    fn capabilities(&self) -> Capabilities {
        Device::mouse().capabilities
    }

    fn poll(&mut self, budget: usize, event: &mut dyn FnMut(RawEvent)) -> usize {
        self.polls.fetch_add(1, Ordering::Relaxed);
        for _ in 0..budget {
            event(SYN);
        }
        budget
    }
}

#[test]
fn a_device_that_never_runs_dry_cannot_keep_the_poller_awake() {
    let devices = LeafSpinLock::new_leaf(Some(Box::new(Input {
        sources: [const { None }; MAX_DEVICES],
        state: InputState::new(),
    })));
    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    if let Some(input) = devices.lock().as_mut() {
        input.sources[0] = Some(Source {
            device: Box::new(NeverDry {
                polls: Arc::clone(&polls),
            }),
            capabilities: Device::mouse().capabilities,
            report: PendingReport::default(),
            interrupting: false,
        });
    }
    let next = poll_devices::<TestPlatform>(&devices, None, SCREEN, timing(1000));
    assert_eq!(next, Some(1000 + ACTIVE_POLL_MS * 1000));
    assert_eq!(polls.load(Ordering::Relaxed), MAX_POLL_ROUNDS);
    let empty = LeafSpinLock::new_leaf(None);
    assert_eq!(
        poll_devices::<TestPlatform>(&empty, None, SCREEN, timing(0)),
        None
    );
}
