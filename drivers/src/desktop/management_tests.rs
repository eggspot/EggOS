use super::protocol::{DiscardEvents, EventRecord, WindowAction, flags};
use super::tests::{BlankTerminal, at};
use super::*;
use crate::input::keymap::Modifiers;

#[derive(Default)]
struct Recorded(Vec<(u16, EventRecord)>);

impl EventSink for Recorded {
    fn send(&mut self, client: u16, record: EventRecord) {
        self.0.push((client, record));
    }
}

impl Recorded {
    fn kinds(&mut self) -> Vec<u16> {
        core::mem::take(&mut self.0)
            .into_iter()
            .map(|(_, record)| record.kind)
            .collect()
    }

    fn of_kind(&self, kind: u16) -> Vec<EventRecord> {
        self.0
            .iter()
            .filter(|(_, record)| record.kind == kind)
            .map(|(_, record)| *record)
            .collect()
    }
}

const CLIENT: u16 = 3;

fn shown_with(desktop: &mut Desktop, width: u32, height: u32, window_flags: u32) -> WindowId {
    let created = desktop
        .create(CLIENT, 0, 0, width, height, window_flags)
        .unwrap();
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

fn shown(desktop: &mut Desktop, width: u32, height: u32) -> WindowId {
    shown_with(desktop, width, height, 0)
}

fn move_to(desktop: &mut Desktop, x: u32, y: u32, events: &mut dyn EventSink) {
    desktop.pointer_moved(x, y, &mut Damage::new(), events);
}

fn press(desktop: &mut Desktop, x: u32, y: u32, events: &mut dyn EventSink) {
    move_to(desktop, x, y, events);
    desktop.left_button(true, &mut Damage::new(), events);
}

fn release(desktop: &mut Desktop, events: &mut dyn EventSink) {
    desktop.left_button(false, &mut Damage::new(), events);
}

fn click(desktop: &mut Desktop, x: u32, y: u32, events: &mut dyn EventSink) {
    press(desktop, x, y, events);
    release(desktop, events);
}

fn key(
    desktop: &mut Desktop,
    code: u16,
    pressed: bool,
    held: Modifiers,
    events: &mut dyn EventSink,
) {
    desktop.key(code, pressed, held, &mut Damage::new(), events);
}

fn tap(desktop: &mut Desktop, code: u16, held: Modifiers, events: &mut dyn EventSink) {
    key(desktop, code, true, held, events);
    key(desktop, code, false, held, events);
}

fn painted(desktop: &Desktop) -> Vec<u32> {
    let screen = desktop.screen();
    let mut pixels = vec![0; (screen.width * screen.height) as usize];
    desktop.draw(&mut pixels, screen.width as usize, screen, &BlankTerminal);
    pixels
}

#[test]
fn the_resize_ring_is_two_outside_four_inside_two_along_the_title_with_sixteen_px_corners() {
    let outside = Rect::new(100, 100, 400, 300);
    let at = |x, y| resize_edges(outside, x, y);
    assert_eq!(at(98, 250), edge::LEFT);
    assert_eq!(at(103, 250), edge::LEFT);
    assert_eq!(at(104, 250), 0);
    assert_eq!(at(97, 250), 0);
    assert_eq!(at(501, 250), edge::RIGHT);
    assert_eq!(at(496, 250), edge::RIGHT);
    assert_eq!(at(495, 250), 0);
    assert_eq!(at(502, 250), 0);
    assert_eq!(at(300, 98), edge::TOP);
    assert_eq!(at(300, 101), edge::TOP);
    assert_eq!(at(300, 102), 0, "the title bar keeps its drag below 2 px");
    assert_eq!(at(300, 401), edge::BOTTOM);
    assert_eq!(at(300, 396), edge::BOTTOM);
    assert_eq!(at(300, 395), 0);
    assert_eq!(at(98, 98), edge::LEFT | edge::TOP);
    assert_eq!(at(110, 99), edge::LEFT | edge::TOP, "the corner runs 16 px");
    assert_eq!(at(114, 99), edge::TOP);
    assert_eq!(at(501, 401), edge::RIGHT | edge::BOTTOM);
    assert_eq!(at(486, 397), edge::RIGHT | edge::BOTTOM);
    assert_eq!(at(98, 390), edge::LEFT | edge::BOTTOM);
    assert_eq!(at(499, 100), edge::RIGHT | edge::TOP);
    assert_eq!(at(200, 200), 0);
    for (edges, shape) in [
        (edge::TOP, PointerShape::ResizeNorthSouth),
        (edge::BOTTOM, PointerShape::ResizeNorthSouth),
        (edge::LEFT, PointerShape::ResizeEastWest),
        (edge::RIGHT, PointerShape::ResizeEastWest),
        (
            edge::LEFT | edge::TOP,
            PointerShape::ResizeNorthWestSouthEast,
        ),
        (
            edge::RIGHT | edge::BOTTOM,
            PointerShape::ResizeNorthWestSouthEast,
        ),
        (
            edge::RIGHT | edge::TOP,
            PointerShape::ResizeNorthEastSouthWest,
        ),
        (
            edge::LEFT | edge::BOTTOM,
            PointerShape::ResizeNorthEastSouthWest,
        ),
        (0, PointerShape::Arrow),
    ] {
        assert_eq!(resize_pointer(edges), shape);
    }
}

#[test]
fn resizing_keeps_the_opposite_edges_the_minimum_and_the_work_area() {
    let work = Rect::new(0, 28, 1280, 700);
    let start = Rect::new(200, 200, 400, 300);
    let minimum = (160, 92);
    let resized_by = |edges, dx, dy| resized(start, edges, dx, dy, minimum, work);
    assert_eq!(
        resized_by(edge::RIGHT | edge::BOTTOM, 100, 60),
        Rect::new(200, 200, 500, 360)
    );
    assert_eq!(
        resized_by(edge::LEFT | edge::TOP, -50, -30),
        Rect::new(150, 170, 450, 330)
    );
    assert_eq!(
        resized_by(edge::LEFT, 1000, 0),
        Rect::new(440, 200, 160, 300),
        "the minimum holds the left edge back"
    );
    assert_eq!(
        resized_by(edge::TOP, 0, -1000),
        Rect::new(200, 28, 400, 472),
        "the top stops at the work area"
    );
    assert_eq!(
        resized_by(edge::RIGHT | edge::BOTTOM, 5000, 5000),
        Rect::new(200, 200, 1080, 528),
        "the work area's right and bottom"
    );
    assert_eq!(
        resized_by(edge::BOTTOM, 0, -1000),
        Rect::new(200, 200, 400, 92)
    );
    assert_eq!(
        resized_by(edge::LEFT, -1000, 0),
        Rect::new(0, 200, 600, 300)
    );
    assert_eq!(
        resized_by(edge::TOP, 7, 0),
        start,
        "a vertical edge ignores dx"
    );
    assert_eq!(
        minimum_outside((300, 10), work),
        (302, 92),
        "a client's minimum adds the chrome"
    );
    assert_eq!(
        minimum_outside((5000, 5000), Rect::new(0, 28, 360, 612)),
        (360, 612),
        "the minimum gives way to a small work area"
    );
}

#[test]
fn a_corner_drag_resizes_live_sending_the_latest_size_and_the_ack_keeps_the_user_s_size() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let outside = desktop.window_rect(window).unwrap();
    let mut events = Recorded::default();
    let (x, y) = (outside.right() - 2, outside.bottom() - 2);
    press(&mut desktop, x, y, &mut events);
    assert_eq!(
        desktop.pointer().map(|pointer| pointer.2),
        Some(PointerShape::ResizeNorthWestSouthEast)
    );
    move_to(&mut desktop, x + 50, y + 30, &mut events);
    move_to(&mut desktop, x + 100, y + 60, &mut events);
    let sizes: Vec<_> = events
        .of_kind(event::RESIZE)
        .iter()
        .map(|record| (record.a, record.b))
        .collect();
    assert_eq!(sizes, [(450, 330), (500, 360)]);
    assert_eq!(
        desktop.window_rect(window),
        Some(Rect::new(outside.x, outside.y, 502, 390))
    );
    let mut damage = Damage::new();
    assert_eq!(
        desktop.set_size(
            CLIENT,
            u32::from(window),
            450,
            330,
            &mut damage,
            &mut DiscardEvents
        ),
        Ok((450, 330))
    );
    assert_eq!(
        desktop
            .content_rect(window)
            .map(|rect| (rect.width, rect.height)),
        Some((500, 360)),
        "a stale answer replaces the buffer, not the size the user gave"
    );
    release(&mut desktop, &mut events);
    assert_eq!(
        desktop.set_size(
            CLIENT,
            u32::from(window),
            500,
            360,
            &mut damage,
            &mut DiscardEvents
        ),
        Ok((500, 360))
    );
    assert_eq!(
        desktop.set_size(
            CLIENT,
            u32::from(window),
            420,
            320,
            &mut damage,
            &mut DiscardEvents
        ),
        Ok((420, 320))
    );
    assert_eq!(
        desktop
            .content_rect(window)
            .map(|rect| (rect.width, rect.height)),
        Some((420, 320)),
        "with no resize waiting, the client's own size is the window's"
    );
}

#[test]
fn until_the_client_commits_at_the_new_size_the_new_area_shows_the_window_background() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let outside = desktop.window_rect(window).unwrap();
    let (x, y) = (outside.right() - 2, outside.bottom() - 2);
    press(&mut desktop, x, y, &mut DiscardEvents);
    move_to(&mut desktop, x + 100, y + 60, &mut DiscardEvents);
    release(&mut desktop, &mut DiscardEvents);
    move_to(&mut desktop, 5, 400, &mut DiscardEvents);
    let content = desktop.content_rect(window).unwrap();
    let pixels = painted(&desktop);
    assert_eq!(
        at(&pixels, content.x + 450, content.y + 50),
        colour::WINDOW_BACKGROUND
    );
    assert_eq!(
        at(&pixels, content.x + 5, content.y + 5),
        super::tests::pattern(window, 5, 5),
        "the old buffer stays, clipped"
    );
}

#[test]
fn the_maximise_control_fills_the_work_area_and_a_second_press_restores_the_window() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let before = desktop.window_rect(window).unwrap();
    let control = desktop.control_of(window, Part::Maximise).unwrap();
    assert_eq!(control, Rect::new(before.x + 48, before.y, 20, 28));
    let mut events = Recorded::default();
    click(&mut desktop, control.x + 10, control.y + 14, &mut events);
    assert!(desktop.is_maximised(window));
    assert_eq!(
        desktop.window_rect(window),
        Some(Rect::new(0, 28, 1280, 700))
    );
    let records = events.of_kind(event::RESIZE);
    assert_eq!((records[0].a, records[0].b), (1278, 670));
    let state = events.of_kind(event::STATE_CHANGED);
    assert_eq!(state[0].a, WindowState::Maximised.value() as i32);
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 0, 28), colour::BORDER_ACTIVE, "square corners");
    let control = desktop.control_of(window, Part::Maximise).unwrap();
    let mut events = Recorded::default();
    click(&mut desktop, control.x + 10, control.y + 14, &mut events);
    assert!(!desktop.is_maximised(window));
    assert_eq!(desktop.window_rect(window), Some(before));
    assert_eq!(
        events.of_kind(event::STATE_CHANGED)[0].a,
        WindowState::Normal.value() as i32
    );
}

#[test]
fn a_double_click_on_the_title_bar_maximises_within_400_ms_and_4_px_only() {
    assert!(is_double_click((10, 10, 1000), (14, 6, 1400)));
    assert!(!is_double_click((10, 10, 1000), (15, 10, 1100)));
    assert!(!is_double_click((10, 10, 1000), (10, 10, 1401)));
    assert!(is_double_click((10, 10, u32::MAX - 100), (10, 10, 50)));
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let title = desktop.title_bar_of(window).unwrap();
    let (x, y) = (title.x + title.width / 2, title.y + 14);
    desktop.set_time(1000);
    click(&mut desktop, x, y, &mut DiscardEvents);
    desktop.set_time(1500);
    click(&mut desktop, x, y, &mut DiscardEvents);
    assert!(!desktop.is_maximised(window), "500 ms apart is two clicks");
    desktop.set_time(1800);
    click(&mut desktop, x + 3, y - 2, &mut DiscardEvents);
    assert!(desktop.is_maximised(window));
    let title = desktop.title_bar_of(window).unwrap();
    let (x, y) = (title.x + title.width / 2, title.y + 14);
    desktop.set_time(5000);
    click(&mut desktop, x, y, &mut DiscardEvents);
    desktop.set_time(5100);
    click(&mut desktop, x, y, &mut DiscardEvents);
    assert!(!desktop.is_maximised(window));
}

#[test]
fn dragging_a_maximised_window_restores_it_under_the_pointer_after_8_px() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let before = desktop.window_rect(window).unwrap();
    desktop
        .set_state(
            CLIENT,
            u32::from(window),
            2,
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .unwrap();
    press(&mut desktop, 640, 40, &mut DiscardEvents);
    move_to(&mut desktop, 646, 40, &mut DiscardEvents);
    assert!(desktop.is_maximised(window));
    move_to(&mut desktop, 650, 40, &mut DiscardEvents);
    release(&mut desktop, &mut DiscardEvents);
    assert!(!desktop.is_maximised(window));
    let after = desktop.window_rect(window).unwrap();
    assert_eq!((after.width, after.height), (before.width, before.height));
    assert_eq!(after.x, 650 - 640 * before.width / 1280);
    assert_eq!(after.y, 28);
}

#[test]
fn super_arrows_maximise_restore_minimise_and_snap_the_focused_window() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let before = desktop.window_rect(window).unwrap();
    let held = Modifiers::SUPER;
    tap(&mut desktop, key::UP, held, &mut DiscardEvents);
    assert!(desktop.is_maximised(window));
    tap(&mut desktop, key::DOWN, held, &mut DiscardEvents);
    assert_eq!(desktop.window_rect(window), Some(before));
    tap(&mut desktop, key::LEFT, held, &mut DiscardEvents);
    assert_eq!(
        desktop.window_rect(window),
        Some(Rect::new(0, 28, 640, 700))
    );
    tap(&mut desktop, key::RIGHT, held, &mut DiscardEvents);
    assert_eq!(
        desktop.window_rect(window),
        Some(Rect::new(640, 28, 640, 700))
    );
    let pixels = painted(&desktop);
    assert_eq!(
        at(&pixels, 640, 28),
        colour::BORDER_ACTIVE,
        "snapped: square"
    );
    assert_ne!(
        at(&pixels, 638, 400),
        at(&painted(&Desktop::new(1280, 800).unwrap()), 638, 400),
        "the shadow stays"
    );
    tap(&mut desktop, key::DOWN, held, &mut DiscardEvents);
    assert!(desktop.is_minimised(window));
    assert_eq!(desktop.focused_window(), Some(desktop.terminal_window()));
}

#[test]
fn maximising_a_minimised_window_shows_it_maximised() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let mut damage = Damage::new();
    desktop
        .set_state(
            CLIENT,
            u32::from(window),
            1,
            &mut damage,
            &mut DiscardEvents,
        )
        .unwrap();
    let mut events = Recorded::default();
    desktop
        .set_state(CLIENT, u32::from(window), 2, &mut damage, &mut events)
        .unwrap();
    assert!(!desktop.is_minimised(window) && desktop.is_maximised(window));
    assert_eq!(desktop.focused_window(), Some(window));
    let states: Vec<i32> = events
        .of_kind(event::STATE_CHANGED)
        .iter()
        .map(|record| record.a)
        .collect();
    assert_eq!(states, [0, 2]);
}

#[test]
fn a_fixed_size_window_has_no_resize_zones_and_ignores_maximise() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown_with(&mut desktop, 400, 300, flags::FIXED_SIZE);
    let outside = desktop.window_rect(window).unwrap();
    move_to(
        &mut desktop,
        outside.right() - 2,
        outside.bottom() - 2,
        &mut DiscardEvents,
    );
    assert_eq!(
        desktop.pointer().map(|pointer| pointer.2),
        Some(PointerShape::Arrow)
    );
    let control = desktop.control_of(window, Part::Maximise).unwrap();
    click(
        &mut desktop,
        control.x + 10,
        control.y + 14,
        &mut DiscardEvents,
    );
    assert!(!desktop.is_maximised(window));
    assert_eq!(
        desktop.set_state(
            CLIENT,
            u32::from(window),
            2,
            &mut Damage::new(),
            &mut DiscardEvents
        ),
        Err(WindowError::Invalid)
    );
    let pixels = painted(&desktop);
    let circle = Rect::new(control.x + 4, control.y + 8, 12, 12);
    assert_eq!(
        at(&pixels, circle.x + 6, circle.y + 1),
        colour::BUTTON_INACTIVE
    );
    tap(&mut desktop, key::UP, Modifiers::SUPER, &mut DiscardEvents);
    assert!(!desktop.is_maximised(window));
}

#[test]
fn the_controls_are_close_minimise_maximise_on_the_left_and_mirrored_on_the_right() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let outside = desktop.window_rect(window).unwrap();
    let hits = |desktop: &Desktop| {
        [Part::Close, Part::Minimise, Part::Maximise]
            .map(|part| desktop.control_of(window, part).unwrap().x - outside.x)
    };
    assert_eq!(hits(&desktop), [8, 28, 48]);
    let pixels = painted(&desktop);
    assert_eq!(
        at(&pixels, outside.x + 52 + 6, outside.y + 9),
        colour::BUTTON_MAXIMISE
    );
    desktop.set_profile(Profile::Windows, &mut Damage::new(), &mut DiscardEvents);
    let w = outside.width;
    assert_eq!(hits(&desktop), [w - 28, w - 68, w - 48]);
    let pixels = painted(&desktop);
    for (x, colour) in [
        (w - 24, colour::BUTTON_CLOSE),
        (w - 44, colour::BUTTON_MAXIMISE),
        (w - 64, colour::BUTTON_MINIMISE),
    ] {
        assert_eq!(at(&pixels, outside.x + x + 6, outside.y + 9), colour);
    }
    let mut events = Recorded::default();
    click(
        &mut desktop,
        outside.x + w - 20,
        outside.y + 14,
        &mut events,
    );
    assert_eq!(events.kinds(), [event::CLOSE_REQUEST]);
}

#[test]
fn hovering_a_control_draws_its_glyph() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let control = desktop.control_of(window, Part::Close).unwrap();
    let circle = (control.x + 4, control.y + 8);
    let glyph_corner = (circle.0 + 4, circle.1 + 4);
    move_to(&mut desktop, 5, 400, &mut DiscardEvents);
    assert_eq!(
        at(&painted(&desktop), glyph_corner.0, glyph_corner.1),
        colour::BUTTON_CLOSE
    );
    move_to(
        &mut desktop,
        control.x + 10,
        control.y + 24,
        &mut DiscardEvents,
    );
    assert_eq!(
        at(&painted(&desktop), glyph_corner.0, glyph_corner.1),
        colour::GLYPH
    );
}

#[test]
fn alt_tab_lists_windows_most_recently_focused_first_and_releasing_alt_chooses() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let terminal = desktop.terminal_window();
    let first = shown(&mut desktop, 400, 300);
    let second = shown(&mut desktop, 400, 300);
    let content = desktop.content_rect(first).unwrap();
    click(
        &mut desktop,
        content.x + 5,
        content.bottom() - 5,
        &mut DiscardEvents,
    );
    let alt = Modifiers::ALT;
    let mut events = Recorded::default();
    key(&mut desktop, key::LEFT_ALT, true, alt, &mut events);
    tap(&mut desktop, key::TAB, alt, &mut events);
    assert_eq!(
        desktop.switcher_entries(),
        Some((vec![first, second, terminal], 1))
    );
    assert!(!desktop.keys_go_to_terminal());
    assert_eq!(events.kinds(), [event::KEY, event::FOCUS_OUT]);
    let area = desktop.switcher_area().unwrap();
    assert_eq!(area, Rect::new(460, 28 + (700 - 114) / 2, 360, 3 * 32 + 18));
    let pixels = painted(&desktop);
    assert_eq!(
        at(&pixels, area.x + 3, area.y + 1 + 8 + 32 + 12),
        colour::FOCUS_RING
    );
    tap(
        &mut desktop,
        key::TAB,
        alt.with(Modifiers::SHIFT),
        &mut events,
    );
    tap(
        &mut desktop,
        key::TAB,
        alt.with(Modifiers::SHIFT),
        &mut events,
    );
    assert_eq!(desktop.switcher_entries().map(|entries| entries.1), Some(2));
    key(
        &mut desktop,
        key::LEFT_ALT,
        false,
        Modifiers::NONE,
        &mut events,
    );
    assert!(!desktop.switcher_open());
    assert_eq!(desktop.focused_window(), Some(terminal));
    assert_eq!(desktop.stacking().last(), Some(terminal));
    key(&mut desktop, key::LEFT_ALT, true, alt, &mut events);
    tap(&mut desktop, key::TAB, alt, &mut events);
    assert_eq!(
        desktop.switcher_entries(),
        Some((vec![terminal, first, second], 1))
    );
    tap(&mut desktop, key::ESC, alt, &mut events);
    assert!(!desktop.switcher_open());
    assert_eq!(desktop.focused_window(), Some(terminal));
    key(
        &mut desktop,
        key::LEFT_ALT,
        false,
        Modifiers::NONE,
        &mut events,
    );
    tap(&mut desktop, key::ESC, alt, &mut events);
    assert_eq!(
        desktop.focused_window(),
        Some(first),
        "Alt+Esc cycles without the popup"
    );
    assert!(!desktop.switcher_open());
}

#[test]
fn the_switcher_lists_minimised_windows_and_choosing_one_restores_it() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    desktop
        .set_state(
            CLIENT,
            u32::from(window),
            1,
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .unwrap();
    let alt = Modifiers::ALT;
    tap(&mut desktop, key::TAB, alt, &mut DiscardEvents);
    assert_eq!(
        desktop.switcher_entries(),
        Some((vec![desktop.terminal_window(), window], 1))
    );
    key(
        &mut desktop,
        key::LEFT_ALT,
        false,
        Modifiers::NONE,
        &mut DiscardEvents,
    );
    assert!(!desktop.is_minimised(window));
    assert_eq!(desktop.focused_window(), Some(window));
}

#[test]
fn close_keys_follow_the_profile_and_spare_a_window_that_takes_ctrl_keys() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let mut events = Recorded::default();
    tap(&mut desktop, key::F4, Modifiers::ALT, &mut events);
    assert_eq!(events.of_kind(event::CLOSE_REQUEST).len(), 1);
    let mut events = Recorded::default();
    tap(&mut desktop, key::W, Modifiers::SUPER, &mut events);
    assert_eq!(
        events.of_kind(event::CLOSE_REQUEST).len(),
        1,
        "Cmd+W on macOS habits"
    );
    let mut events = Recorded::default();
    tap(&mut desktop, key::W, Modifiers::CTRL, &mut events);
    assert_eq!(events.of_kind(event::CLOSE_REQUEST).len(), 0);
    assert_eq!(events.of_kind(event::KEY).len(), 2);
    desktop.set_profile(Profile::Windows, &mut Damage::new(), &mut DiscardEvents);
    let mut events = Recorded::default();
    tap(&mut desktop, key::W, Modifiers::CTRL, &mut events);
    assert_eq!(events.of_kind(event::CLOSE_REQUEST).len(), 1);
    let takes_ctrl = shown_with(&mut desktop, 300, 200, flags::TAKES_CTRL_KEYS);
    assert_eq!(desktop.focused_window(), Some(takes_ctrl));
    let mut events = Recorded::default();
    tap(&mut desktop, key::W, Modifiers::CTRL, &mut events);
    assert_eq!(events.of_kind(event::CLOSE_REQUEST).len(), 0);
    tap(&mut desktop, key::F4, Modifiers::ALT, &mut events);
    assert_eq!(events.of_kind(event::CLOSE_REQUEST).len(), 1);
    let _ = window;
    assert!(shortcut(key::F4, Modifiers::ALT, Profile::Linux, true).is_some());
    assert!(shortcut(key::TAB, Modifiers::SUPER, Profile::Windows, false).is_none());
    assert!(shortcut(key::A, Modifiers::ALT, Profile::Macos, false).is_none());
    assert_eq!(
        shortcut(
            key::TAB,
            Modifiers::ALT
                .with(Modifiers::SHIFT)
                .with(Modifiers::CAPS_LOCK),
            Profile::Macos,
            false
        ),
        Some(Shortcut::Switch {
            back: true,
            held: shortcuts::Held::Alt
        })
    );
}

#[test]
fn a_window_that_does_not_answer_shows_the_busy_pointer_over_its_content_only() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let content = desktop.content_rect(window).unwrap();
    let title = desktop.title_bar_of(window).unwrap();
    move_to(
        &mut desktop,
        content.x + 50,
        content.y + 50,
        &mut DiscardEvents,
    );
    desktop.set_time(10_000);
    tap(&mut desktop, key::F4, Modifiers::ALT, &mut DiscardEvents);
    desktop.tick(10_500, &mut Damage::new(), &mut DiscardEvents);
    assert!(desktop.is_not_responding(window));
    assert_eq!(
        desktop.pointer(),
        Some((content.x + 50, content.y + 50, PointerShape::Busy))
    );
    move_to(
        &mut desktop,
        title.x + 100,
        title.y + 10,
        &mut DiscardEvents,
    );
    assert_eq!(
        desktop.pointer().map(|(_, _, shape)| shape),
        Some(PointerShape::Arrow)
    );
    move_to(
        &mut desktop,
        content.x + 60,
        content.y + 60,
        &mut DiscardEvents,
    );
    assert_eq!(
        desktop.pointer().map(|(_, _, shape)| shape),
        Some(PointerShape::Busy)
    );
    desktop.close_answered(CLIENT, &mut Damage::new());
    assert_eq!(
        desktop.pointer().map(|(_, _, shape)| shape),
        Some(PointerShape::Arrow)
    );
}

#[test]
fn an_unanswered_close_request_says_not_responding_and_closes_after_2_s() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    desktop.set_time(10_000);
    tap(&mut desktop, key::F4, Modifiers::ALT, &mut DiscardEvents);
    let mut damage = Damage::new();
    assert!(!desktop.tick(10_499, &mut damage, &mut DiscardEvents));
    assert!(!desktop.is_not_responding(window));
    assert!(!desktop.tick(10_500, &mut damage, &mut DiscardEvents));
    assert!(desktop.is_not_responding(window));
    assert!(
        damage
            .rects()
            .contains(&desktop.title_bar_of(window).unwrap())
    );
    assert!(desktop.tick(12_000, &mut damage, &mut DiscardEvents));
    assert!(desktop.window_rect(window).is_none());
    assert_eq!(desktop.focused_window(), Some(desktop.terminal_window()));

    let answered = shown(&mut desktop, 400, 300);
    desktop.set_time(20_000);
    tap(&mut desktop, key::F4, Modifiers::ALT, &mut DiscardEvents);
    desktop.tick(20_600, &mut Damage::new(), &mut DiscardEvents);
    assert!(desktop.is_not_responding(answered));
    desktop.close_answered(CLIENT, &mut Damage::new());
    assert!(!desktop.is_not_responding(answered));
    assert!(!desktop.tick(30_000, &mut Damage::new(), &mut DiscardEvents));
    assert!(
        desktop.window_rect(answered).is_some(),
        "an answered request is the client's"
    );
    tap(&mut desktop, key::F4, Modifiers::ALT, &mut DiscardEvents);
    assert!(!desktop.tick(31_999, &mut Damage::new(), &mut DiscardEvents));
    assert!(
        desktop.tick(32_000, &mut Damage::new(), &mut DiscardEvents),
        "the clock runs from the request"
    );
}

#[test]
fn a_clock_behind_a_close_request_counts_as_no_time_waited() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    desktop.set_time(10_000);
    tap(&mut desktop, key::F4, Modifiers::ALT, &mut DiscardEvents);
    assert!(!desktop.tick(9_990, &mut Damage::new(), &mut DiscardEvents));
    assert!(!desktop.is_not_responding(window));
    assert!(desktop.window_rect(window).is_some());
    assert!(desktop.tick(12_000, &mut Damage::new(), &mut DiscardEvents));

    let wrapping = shown(&mut desktop, 400, 300);
    desktop.set_time(u32::MAX - 100);
    tap(&mut desktop, key::F4, Modifiers::ALT, &mut DiscardEvents);
    assert!(!desktop.tick(400, &mut Damage::new(), &mut DiscardEvents));
    assert!(
        desktop.is_not_responding(wrapping),
        "the clock wraps through 0"
    );
}

#[test]
fn the_terminal_declines_close_requests_from_every_key() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    tap(&mut desktop, key::F4, Modifiers::ALT, &mut DiscardEvents);
    assert!(!desktop.tick(100_000, &mut Damage::new(), &mut DiscardEvents));
    assert!(desktop.terminal_shown());
    assert!(desktop.takes_key(key::F4, Modifiers::ALT));
    assert!(!desktop.takes_key(key::W, Modifiers::CTRL));
}

#[test]
fn the_pointer_is_drawn_once_it_moves_in_the_shape_the_place_asks_for() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    assert_eq!(desktop.pointer(), None, "hidden until it first moves");
    let window = shown(&mut desktop, 400, 300);
    let mut damage = Damage::new();
    desktop.pointer_moved(10, 400, &mut damage, &mut DiscardEvents);
    assert_eq!(desktop.pointer(), Some((10, 400, PointerShape::Arrow)));
    assert!(damage.rects().contains(&Rect::new(10, 400, 12, 19)));
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 10, 400), 0x0000_0000);
    assert_eq!(at(&pixels, 11, 402), 0x00ff_ffff);
    let content = desktop.content_rect(window).unwrap();
    desktop
        .set_pointer(CLIENT, u32::from(window), 1, &mut Damage::new())
        .unwrap();
    move_to(
        &mut desktop,
        content.x + 50,
        content.y + 50,
        &mut DiscardEvents,
    );
    assert_eq!(
        desktop.pointer(),
        Some((content.x + 50, content.y + 50, PointerShape::IBeam))
    );
    let pixels = painted(&desktop);
    assert_eq!(
        at(&pixels, content.x + 50 - 4, content.y + 50 - 8),
        0x0000_0000
    );
    let title = desktop.title_bar_of(window).unwrap();
    move_to(
        &mut desktop,
        title.x + 200,
        title.y + 14,
        &mut DiscardEvents,
    );
    assert_eq!(
        desktop.pointer().map(|pointer| pointer.2),
        Some(PointerShape::Arrow)
    );
    let outside = desktop.window_rect(window).unwrap();
    for (x, y, shape) in [
        (outside.x - 1, outside.y + 100, PointerShape::ResizeEastWest),
        (
            outside.x + 100,
            outside.bottom(),
            PointerShape::ResizeNorthSouth,
        ),
        (
            outside.right(),
            outside.y,
            PointerShape::ResizeNorthEastSouthWest,
        ),
    ] {
        move_to(&mut desktop, x, y, &mut DiscardEvents);
        assert_eq!(desktop.pointer().map(|pointer| pointer.2), Some(shape));
    }
    let mut damage = Damage::new();
    desktop.pointer_moved(0, 799, &mut damage, &mut DiscardEvents);
    assert_eq!(desktop.pointer(), Some((0, 799, PointerShape::Arrow)));
    let mut fast = painted(&desktop);
    let whole = fast.clone();
    for rect in damage.rects() {
        desktop.draw(&mut fast, 1280, *rect, &BlankTerminal);
    }
    assert!(
        fast == whole,
        "the damage covers where it was and where it is"
    );
}

#[test]
fn the_pointer_shapes_are_the_design_s_bitmaps() {
    for (shape, size, hotspot) in [
        (PointerShape::Arrow, (12, 19), (0, 0)),
        (PointerShape::ResizeNorthSouth, (9, 19), (4, 9)),
        (PointerShape::ResizeEastWest, (19, 9), (9, 4)),
        (PointerShape::ResizeNorthWestSouthEast, (16, 16), (7, 7)),
        (PointerShape::ResizeNorthEastSouthWest, (16, 16), (8, 7)),
        (PointerShape::IBeam, (9, 16), (4, 8)),
    ] {
        assert_eq!(
            pointer::size_and_hotspot(shape),
            (size, hotspot),
            "{shape:?}"
        );
    }
    let colour_at = |shape, x, y| pointer::pixel_at(shape, (100, 100), x, y);
    assert_eq!(colour_at(PointerShape::ResizeEastWest, 91, 100), Some(0));
    assert_eq!(
        colour_at(PointerShape::ResizeEastWest, 92, 100),
        Some(0x00ff_ffff)
    );
    assert_eq!(colour_at(PointerShape::ResizeEastWest, 91, 99), None);
    assert_eq!(
        colour_at(PointerShape::ResizeNorthEastSouthWest, 107, 93),
        Some(0)
    );
    assert_eq!(
        colour_at(PointerShape::ResizeNorthEastSouthWest, 92, 93),
        None
    );
    assert_eq!(
        pointer::area(PointerShape::IBeam, 2, 3),
        Rect::new(0, 0, 7, 11)
    );
}

#[test]
fn the_window_menu_opens_with_alt_space_and_its_keys_move_size_and_maximise() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let before = desktop.window_rect(window).unwrap();
    let alt = Modifiers::ALT;
    tap(&mut desktop, key::SPACE, alt, &mut DiscardEvents);
    assert!(desktop.window_menu_open());
    assert_eq!(
        desktop.window_menu_area(),
        Some(Rect::new(before.x + 8, before.y + 29 + 4, 160, 6 * 32 + 2))
    );
    let none = Modifiers::NONE;
    tap(&mut desktop, key::DOWN, none, &mut DiscardEvents);
    tap(&mut desktop, key::ENTER, none, &mut DiscardEvents);
    assert!(!desktop.window_menu_open(), "Size leaves the items");
    let mut events = Recorded::default();
    tap(&mut desktop, key::RIGHT, none, &mut events);
    tap(&mut desktop, key::DOWN, none, &mut events);
    assert_eq!(
        desktop
            .window_rect(window)
            .map(|rect| (rect.width, rect.height)),
        Some((before.width + 8, before.height + 8))
    );
    assert_eq!(events.of_kind(event::RESIZE).len(), 2);
    assert_eq!(
        desktop.pointer(),
        None,
        "the pointer stays hidden until it moves"
    );
    tap(&mut desktop, key::ESC, none, &mut DiscardEvents);
    assert_eq!(
        desktop.window_rect(window),
        Some(before),
        "Esc puts it back"
    );
    tap(&mut desktop, key::SPACE, alt, &mut DiscardEvents);
    tap(&mut desktop, key::ENTER, none, &mut DiscardEvents);
    for _ in 0..5 {
        tap(&mut desktop, key::LEFT, none, &mut DiscardEvents);
    }
    tap(&mut desktop, key::ENTER, none, &mut DiscardEvents);
    assert_eq!(
        desktop.window_rect(window).map(|rect| rect.x),
        Some(before.x - 40)
    );
    tap(&mut desktop, key::SPACE, alt, &mut DiscardEvents);
    for _ in 0..3 {
        tap(&mut desktop, key::DOWN, none, &mut DiscardEvents);
    }
    tap(&mut desktop, key::ENTER, none, &mut DiscardEvents);
    assert!(desktop.is_maximised(window));
    tap(&mut desktop, key::SPACE, alt, &mut DiscardEvents);
    tap(&mut desktop, key::ENTER, none, &mut DiscardEvents);
    assert!(
        !desktop.is_maximised(window),
        "Restore is the first item that applies"
    );
}

#[test]
fn a_right_click_on_the_title_bar_opens_the_window_menu_at_the_pointer() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let title = desktop.title_bar_of(window).unwrap();
    let (x, y) = (title.x + 150, title.y + 10);
    move_to(&mut desktop, x, y, &mut DiscardEvents);
    desktop.button(button::RIGHT, true, &mut Damage::new(), &mut DiscardEvents);
    desktop.button(button::RIGHT, false, &mut Damage::new(), &mut DiscardEvents);
    let menu = desktop.window_menu_area().unwrap();
    assert_eq!((menu.x, menu.y), (x, y));
    let mut events = Recorded::default();
    click(
        &mut desktop,
        menu.x + 20,
        menu.y + 1 + 5 * 32 + 10,
        &mut events,
    );
    assert!(!desktop.window_menu_open());
    assert_eq!(events.of_kind(event::CLOSE_REQUEST).len(), 1);
    move_to(&mut desktop, x, y, &mut DiscardEvents);
    desktop.button(button::RIGHT, true, &mut Damage::new(), &mut DiscardEvents);
    desktop.button(button::RIGHT, false, &mut Damage::new(), &mut DiscardEvents);
    click(&mut desktop, 5, 500, &mut DiscardEvents);
    assert!(!desktop.window_menu_open(), "a press outside closes it");
}

#[test]
fn super_d_minimises_every_window_and_again_restores_them() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    tap(&mut desktop, key::D, Modifiers::SUPER, &mut DiscardEvents);
    assert!(desktop.minimised() && desktop.is_minimised(window));
    assert_eq!(desktop.focused_window(), None);
    tap(&mut desktop, key::D, Modifiers::SUPER, &mut DiscardEvents);
    assert!(!desktop.minimised() && !desktop.is_minimised(window));
    assert_eq!(desktop.focused_window(), Some(window));
}

#[test]
fn f11_shows_the_desktop_in_the_macos_profile_only() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    tap(&mut desktop, key::F11, Modifiers::NONE, &mut DiscardEvents);
    assert!(desktop.is_minimised(window));
    tap(&mut desktop, key::F11, Modifiers::NONE, &mut DiscardEvents);
    assert!(!desktop.is_minimised(window));
    for profile in [Profile::Windows, Profile::Linux] {
        assert_eq!(shortcut(key::F11, Modifiers::NONE, profile, false), None);
    }
    assert_eq!(
        shortcut(key::F11, Modifiers::SHIFT, Profile::Macos, false),
        None
    );
}

#[test]
fn super_x_opens_the_power_menu_for_the_keys() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    tap(&mut desktop, key::X, Modifiers::SUPER, &mut DiscardEvents);
    assert!(desktop.menu_open());
    let action = desktop.key(
        key::ENTER,
        true,
        Modifiers::NONE,
        &mut Damage::new(),
        &mut DiscardEvents,
    );
    assert_eq!(action, Action::PowerOff);
}

#[test]
fn a_dialog_of_a_minimised_parent_does_not_take_focus_when_it_first_shows() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let parent = shown(&mut desktop, 400, 300);
    desktop
        .set_state(
            CLIENT,
            u32::from(parent),
            1,
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .unwrap();
    let focused = desktop.focused_window();
    let dialog = desktop
        .create(CLIENT, 1, u32::from(parent), 200, 100, 0)
        .unwrap()
        .window;
    desktop
        .commit(
            CLIENT,
            u32::from(dialog),
            &[],
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .unwrap();
    assert_eq!(desktop.focused_window(), focused);
    assert!(!desktop.is_shown(dialog));
    desktop
        .set_state(
            CLIENT,
            u32::from(parent),
            0,
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .unwrap();
    assert_eq!(desktop.focused_window(), Some(dialog));
}

#[test]
fn wm_lists_shown_windows_and_manages_any_of_them() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    desktop
        .set_title(CLIENT, u32::from(window), b"first", &mut Damage::new())
        .unwrap();
    let _unshown = desktop.create(CLIENT, 0, 0, 100, 100, 0).unwrap();
    let terminal = desktop.listed_window(0).unwrap();
    assert_eq!(terminal.id, desktop.terminal_window());
    assert_eq!(terminal.client, KERNEL_CLIENT);
    assert_eq!(&terminal.title[..terminal.title_length], b"Terminal");
    let info = desktop.listed_window(1).unwrap();
    assert_eq!((info.id, info.state), (window, WindowState::Normal));
    assert_eq!(desktop.listed_window(2), Err(WindowError::NoSuchWindow));
    let manage = |desktop: &mut Desktop, action: WindowAction, a: i32, b: i32| {
        desktop.manage(
            u32::from(window),
            action as u32,
            a,
            b,
            &mut Damage::new(),
            &mut DiscardEvents,
        )
    };
    manage(&mut desktop, WindowAction::Maximise, 0, 0).unwrap();
    assert_eq!(
        desktop.listed_window(1).unwrap().state,
        WindowState::Maximised
    );
    manage(&mut desktop, WindowAction::Restore, 0, 0).unwrap();
    manage(&mut desktop, WindowAction::Move, 100, 200).unwrap();
    assert_eq!(
        desktop.window_rect(window).map(|rect| (rect.x, rect.y)),
        Some((100, 200))
    );
    manage(&mut desktop, WindowAction::Size, 500, 260).unwrap();
    assert_eq!(
        desktop
            .content_rect(window)
            .map(|rect| (rect.width, rect.height)),
        Some((500, 260))
    );
    manage(&mut desktop, WindowAction::Minimise, 0, 0).unwrap();
    assert!(desktop.is_minimised(window));
    manage(&mut desktop, WindowAction::Focus, 0, 0).unwrap();
    assert_eq!(desktop.focused_window(), Some(window));
    let mut events = Recorded::default();
    desktop
        .manage(u32::from(window), 4, 0, 0, &mut Damage::new(), &mut events)
        .unwrap();
    assert_eq!(events.kinds(), [event::CLOSE_REQUEST]);
    assert_eq!(
        desktop.manage(
            u32::from(desktop.terminal_window()),
            4,
            0,
            0,
            &mut Damage::new(),
            &mut DiscardEvents
        ),
        Err(WindowError::NotAllowed)
    );
    assert_eq!(
        desktop.manage(
            u32::from(window),
            7,
            0,
            0,
            &mut Damage::new(),
            &mut DiscardEvents
        ),
        Err(WindowError::Invalid)
    );
}

#[test]
fn painting_with_popups_and_the_pointer_in_many_rects_equals_painting_the_whole_screen() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let _first = shown(&mut desktop, 400, 300);
    let _second = shown(&mut desktop, 500, 200);
    move_to(&mut desktop, 600, 410, &mut DiscardEvents);
    tap(&mut desktop, key::TAB, Modifiers::ALT, &mut DiscardEvents);
    let whole = painted(&desktop);
    let mut pieces = vec![0; whole.len()];
    for y in (0..800).step_by(100) {
        for x in (0..1280).step_by(128) {
            desktop.draw(&mut pieces, 1280, Rect::new(x, y, 128, 100), &BlankTerminal);
        }
    }
    assert!(pieces == whole);
}

#[test]
fn a_pressed_control_darkens_and_acts_only_when_released_over_it() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, 400, 300);
    let close = desktop.control_of(window, Part::Close).unwrap();
    let fill = (close.x + 4 + 6, close.y + 8 + 1);
    let mut events = Recorded::default();
    press(&mut desktop, close.x + 10, close.y + 14, &mut events);
    assert!(
        events.of_kind(event::CLOSE_REQUEST).is_empty(),
        "not before the release"
    );
    assert_eq!(
        at(&painted(&desktop), fill.0, fill.1),
        paint::quarter(colour::BUTTON_CLOSE)
    );
    move_to(&mut desktop, 5, 400, &mut events);
    assert_eq!(
        at(&painted(&desktop), fill.0, fill.1),
        colour::BUTTON_CLOSE,
        "off the control while pressed"
    );
    move_to(&mut desktop, close.x + 10, close.y + 14, &mut events);
    release(&mut desktop, &mut events);
    assert_eq!(events.of_kind(event::CLOSE_REQUEST).len(), 1);
    let minimise = desktop.control_of(window, Part::Minimise).unwrap();
    press(&mut desktop, minimise.x + 10, minimise.y + 14, &mut events);
    move_to(&mut desktop, 5, 400, &mut events);
    release(&mut desktop, &mut events);
    assert!(desktop.is_shown(window), "released elsewhere: nothing");
    assert_eq!(paint::quarter(colour::BUTTON_CLOSE), 0x00ac_3f39);
}
