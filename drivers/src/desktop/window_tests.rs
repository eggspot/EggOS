use super::protocol::{
    DiscardEvents, EventRecord, MAX_COMMIT_RECTS, MAX_WINDOWS_PER_CLIENT, Request, create_reply,
    request, set_size_reply, size,
};
use super::tests::{BlankTerminal, at, painted, pattern};
use super::*;
use alloc::vec;

#[derive(Default)]
struct Recorded(Vec<(u16, EventRecord)>);

impl EventSink for Recorded {
    fn send(&mut self, client: u16, record: EventRecord) {
        self.0.push((client, record));
    }
}

impl Recorded {
    fn kinds(&self) -> Vec<(u16, u16)> {
        self.0
            .iter()
            .map(|(client, record)| (*client, record.kind))
            .collect()
    }

    fn take(&mut self) -> Vec<(u16, EventRecord)> {
        core::mem::take(&mut self.0)
    }
}

const CLIENT: u16 = 3;
const OTHER: u16 = 7;

fn shown(desktop: &mut Desktop, client: u16, width: u32, height: u32) -> WindowId {
    let created = desktop.create(client, 0, 0, width, height, 0).unwrap();
    desktop
        .commit(
            client,
            u32::from(created.window),
            &[],
            &mut Damage::new(),
            &mut Recorded::default(),
        )
        .unwrap();
    created.window
}

fn press(desktop: &mut Desktop, x: u32, y: u32, events: &mut Recorded) {
    let mut damage = Damage::new();
    desktop.pointer_moved(x, y, &mut damage, events);
    desktop.left_button(true, &mut damage, events);
}

fn release(desktop: &mut Desktop, events: &mut Recorded) {
    desktop.left_button(false, &mut Damage::new(), events);
}

fn click(desktop: &mut Desktop, x: u32, y: u32, events: &mut Recorded) {
    press(desktop, x, y, events);
    release(desktop, events);
}

#[test]
fn a_window_shows_on_its_first_commit_cascaded_on_top_and_focused() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let created = desktop.create(CLIENT, 0, 0, 400, 300, 0).unwrap();
    assert_eq!((created.width, created.height), (400, 300));
    assert!(!desktop.is_shown(created.window));
    assert_eq!(desktop.stacking().count(), 1);
    let mut damage = Damage::new();
    desktop
        .commit(
            CLIENT,
            u32::from(created.window),
            &[],
            &mut damage,
            &mut events,
        )
        .unwrap();
    assert!(desktop.is_shown(created.window));
    assert_eq!(
        desktop.window_rect(created.window),
        Some(Rect::new(311 + 32, 163 + 32, 402, 330))
    );
    assert_eq!(desktop.focused_window(), Some(created.window));
    assert!(!desktop.keys_go_to_terminal());
    assert_eq!(events.kinds(), [(CLIENT, event::FOCUS_IN)]);
    assert!(
        damage
            .rects()
            .iter()
            .any(|rect| rect.overlaps(&Rect::new(343, 195, 402, 330)))
    );
    let second = shown(&mut desktop, OTHER, 400, 300);
    assert_eq!(
        desktop.window_rect(second),
        Some(Rect::new(311 + 64, 163 + 64, 402, 330))
    );
    let stack: Vec<_> = desktop.stacking().collect();
    assert_eq!(stack, [desktop.terminal_window(), created.window, second]);
}

#[test]
fn cascading_restarts_at_the_work_area_s_corner_when_the_next_step_does_not_fit() {
    for (width, height, expected) in [
        (
            1280,
            800,
            [
                (343, 195),
                (375, 227),
                (407, 259),
                (439, 291),
                (32, 60),
                (64, 92),
            ],
        ),
        (
            1024,
            768,
            [
                (215, 179),
                (247, 211),
                (279, 243),
                (32, 60),
                (64, 92),
                (96, 124),
            ],
        ),
    ] {
        let mut desktop = Desktop::new(width, height).unwrap();
        for (index, (x, y)) in expected.into_iter().enumerate() {
            let window = shown(&mut desktop, index as u16, 600, 400);
            let rect = desktop.window_rect(window).unwrap();
            assert_eq!(
                (rect.x, rect.y),
                (x, y),
                "window {index} at {width}x{height}"
            );
        }
    }
}

#[test]
fn the_only_window_is_centred_and_a_too_large_one_is_clamped_to_the_work_area() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut damage = Damage::new();
    desktop.minimise_terminal(&mut damage, &mut DiscardEvents);
    let window = shown(&mut desktop, CLIENT, 5000, 5000);
    let rect = desktop.window_rect(window).unwrap();
    assert_eq!((rect.width, rect.height), (1280 - 32, 700 - 32));
    assert_eq!((rect.x, rect.y), (16, 28 + 16));
    let tiny = desktop.create(CLIENT, 0, 0, 1, 1, 0).unwrap();
    assert_eq!((tiny.width, tiny.height), (158, 62));
    assert_eq!(clamp_content_size(200, 122, 400, 300), (166, 60));
    assert_eq!(clamp_content_size(150, 72, 400, 300), (116, 10));
}

#[test]
fn clicking_a_window_raises_and_focuses_it_and_the_wallpaper_takes_focus_away() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let first = shown(&mut desktop, CLIENT, 400, 300);
    let second = shown(&mut desktop, OTHER, 400, 300);
    let first_rect = desktop.window_rect(first).unwrap();
    click(
        &mut desktop,
        first_rect.x + 5,
        first_rect.y + 100,
        &mut events,
    );
    assert_eq!(desktop.focused_window(), Some(first));
    assert_eq!(desktop.stacking().last(), Some(first));
    let kinds = events.kinds();
    assert!(kinds.contains(&(OTHER, event::FOCUS_OUT)));
    assert!(kinds.contains(&(CLIENT, event::FOCUS_IN)));
    assert!(kinds.contains(&(CLIENT, event::BUTTON)));
    events.take();
    click(&mut desktop, 400, 300, &mut events);
    assert_eq!(desktop.focused_window(), Some(first));
    click(&mut desktop, 20, 700, &mut events);
    assert_eq!(desktop.focused_window(), None);
    let terminal = desktop.window();
    click(
        &mut desktop,
        terminal.right() - 20,
        terminal.y + 300,
        &mut events,
    );
    assert!(desktop.keys_go_to_terminal());
    assert_eq!(desktop.stacking().last(), Some(desktop.terminal_window()));
    let _ = second;
}

#[test]
fn the_close_control_asks_the_client_and_closes_nothing_itself() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = shown(&mut desktop, CLIENT, 400, 300);
    let close = desktop.close_control(window).unwrap();
    click(&mut desktop, close.x + 10, close.y + 14, &mut events);
    assert_eq!(events.kinds().last(), Some(&(CLIENT, event::CLOSE_REQUEST)));
    assert!(desktop.is_shown(window));
    let mut damage = Damage::new();
    desktop
        .destroy(CLIENT, u32::from(window), &mut damage, &mut events)
        .unwrap();
    assert!(!desktop.is_shown(window));
    assert!(desktop.keys_go_to_terminal());
    assert!(damage.rects().iter().any(|rect| rect.overlaps(&close)));
    assert_eq!(
        desktop.destroy(CLIENT, u32::from(window), &mut damage, &mut events),
        Err(WindowError::Invalid)
    );
}

#[test]
fn dragging_the_title_bar_moves_a_window_and_keeps_it_in_the_work_area() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = shown(&mut desktop, CLIENT, 400, 300);
    let title = Rect::new(343 + 100, 195 + 10, 1, 1);
    press(&mut desktop, title.x, title.y, &mut events);
    let mut damage = Damage::new();
    desktop.pointer_moved(title.x + 50, title.y + 70, &mut damage, &mut events);
    assert_eq!(
        desktop.window_rect(window),
        Some(Rect::new(393, 265, 402, 330))
    );
    assert!(
        damage
            .rects()
            .iter()
            .any(|rect| rect.overlaps(&Rect::new(343, 195, 1, 1)))
    );
    desktop.pointer_moved(0, 0, &mut damage, &mut events);
    assert_eq!(
        desktop.window_rect(window),
        Some(Rect::new(0, 28, 402, 330))
    );
    release(&mut desktop, &mut events);
    desktop.pointer_moved(900, 700, &mut damage, &mut events);
    assert_eq!(
        desktop.window_rect(window),
        Some(Rect::new(0, 28, 402, 330))
    );
    assert!(
        !events
            .kinds()
            .iter()
            .any(|&(_, kind)| kind == event::BUTTON),
        "the title bar is chrome: its clicks are not the client's"
    );
}

#[test]
fn a_press_in_the_content_holds_the_pointer_until_the_last_button_is_up() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = shown(&mut desktop, CLIENT, 400, 300);
    let content = desktop.content_rect(window).unwrap();
    let mut damage = Damage::new();
    desktop.pointer_moved(content.x + 10, content.y + 20, &mut damage, &mut events);
    assert_eq!(
        events
            .take()
            .iter()
            .map(|(_, record)| (record.kind, record.a, record.b))
            .collect::<Vec<_>>(),
        [
            (event::POINTER_ENTER, 10, 20),
            (event::POINTER_MOVE, 10, 20)
        ]
    );
    desktop.left_button(true, &mut damage, &mut events);
    desktop.pointer_moved(content.x - 5, content.y - 40, &mut damage, &mut events);
    let moved = events.take();
    let last = moved.last().unwrap().1;
    assert_eq!(
        (last.kind, last.a, last.b, last.c),
        (event::POINTER_MOVE, -5, -40, 1)
    );
    desktop.left_button(false, &mut damage, &mut events);
    let after: Vec<_> = events
        .take()
        .iter()
        .map(|(_, record)| record.kind)
        .collect();
    assert_eq!(after, [event::BUTTON, event::POINTER_LEAVE]);
    assert_eq!(
        desktop.window_rect(window),
        Some(Rect::new(343, 195, 402, 330))
    );
}

#[test]
fn keys_and_the_wheel_go_to_the_focused_window_s_client() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = shown(&mut desktop, CLIENT, 400, 300);
    let mut damage = Damage::new();
    desktop.key(30, true, Modifiers::SHIFT, &mut damage, &mut events);
    let (client, record) = events.take()[0];
    assert_eq!(client, CLIENT);
    assert_eq!(
        (
            record.kind,
            record.window,
            record.a,
            record.b,
            record.c,
            record.d
        ),
        (event::KEY, window, 30, 1, modifier::SHIFT, u32::from(b'A'))
    );
    let content = desktop.content_rect(window).unwrap();
    desktop.pointer_moved(content.x + 8, content.y + 9, &mut damage, &mut events);
    events.take();
    desktop.wheel(-2, &mut events);
    let (_, wheel) = events.take()[0];
    assert_eq!(
        (wheel.kind, wheel.a, wheel.b, wheel.d as i32),
        (event::WHEEL, 8, 9, -2)
    );
    let terminal = desktop.window();
    click(&mut desktop, terminal.x + 10, terminal.y + 100, &mut events);
    events.take();
    desktop.key(30, true, Modifiers::NONE, &mut damage, &mut events);
    assert!(events.0.is_empty());
}

#[test]
fn key_codepoints_follow_the_us_keymap_with_shift_caps_lock_and_ctrl() {
    let caps = Modifiers::CAPS_LOCK;
    assert_eq!(codepoint(30, Modifiers::NONE), u32::from(b'a'));
    assert_eq!(codepoint(30, caps), u32::from(b'A'));
    assert_eq!(codepoint(30, caps.with(Modifiers::SHIFT)), u32::from(b'a'));
    assert_eq!(codepoint(2, caps), u32::from(b'1'));
    assert_eq!(codepoint(2, Modifiers::SHIFT), u32::from(b'!'));
    assert_eq!(codepoint(46, Modifiers::CTRL), 3);
    assert_eq!(codepoint(key::ENTER, Modifiers::NONE), 13);
    assert_eq!(codepoint(key::F1, Modifiers::NONE), 0);
    assert_eq!(
        key_modifiers(Modifiers::CTRL.with(Modifiers::SUPER).with(caps)),
        modifier::CTRL | modifier::SUPER | modifier::CAPS_LOCK
    );
}

#[test]
fn minimising_a_window_moves_focus_shows_a_chip_and_the_chip_restores_it() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = shown(&mut desktop, CLIENT, 400, 300);
    let mut damage = Damage::new();
    desktop
        .set_title(
            CLIENT,
            u32::from(window),
            b"A long window title, cut",
            &mut damage,
        )
        .unwrap();
    desktop.set_profile(Profile::Linux, &mut damage, &mut DiscardEvents);
    let rect = desktop.window_rect(window).unwrap();
    let minimise = desktop.control_of(window, Part::Minimise).unwrap();
    click(&mut desktop, minimise.x + 10, minimise.y + 10, &mut events);
    assert!(!desktop.is_shown(window));
    assert!(desktop.keys_go_to_terminal());
    let records: Vec<_> = events
        .take()
        .iter()
        .map(|(_, record)| (record.kind, record.a))
        .collect();
    assert!(records.contains(&(event::STATE_CHANGED, 1)));
    assert!(records.contains(&(event::FOCUS_OUT, 0)));
    let chip = desktop.chip_of(window).unwrap();
    assert_eq!(chip, Rect::new(104, 4, 16 * 8 + 16, 20));
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 106, 14), colour::BAR_HOVER);
    click(&mut desktop, chip.x + 10, chip.y + 10, &mut events);
    assert!(desktop.is_shown(window));
    assert_eq!(desktop.focused_window(), Some(window));
    assert_eq!(desktop.window_rect(window), Some(rect));
    assert!(desktop.chip_of(window).is_none());
}

#[test]
fn a_dialog_sits_on_its_parent_takes_its_input_and_goes_with_it() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let parent = shown(&mut desktop, CLIENT, 400, 300);
    assert_eq!(
        desktop.create(OTHER, 1, u32::from(parent), 200, 100, 0),
        Err(WindowError::Invalid)
    );
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
            &mut events,
        )
        .unwrap();
    let parent_rect = desktop.window_rect(parent).unwrap();
    let dialog_rect = desktop.window_rect(dialog).unwrap();
    assert_eq!(
        (dialog_rect.x, dialog_rect.y),
        (
            parent_rect.x + (402 - 202) / 2,
            parent_rect.y + (330 - 130) / 2
        )
    );
    click(&mut desktop, 20, 700, &mut events);
    assert_eq!(desktop.focused_window(), None);
    events.take();
    click(
        &mut desktop,
        parent_rect.x + 5,
        parent_rect.bottom() - 10,
        &mut events,
    );
    assert_eq!(desktop.focused_window(), Some(dialog));
    let stack: Vec<_> = desktop.stacking().collect();
    assert_eq!(&stack[1..], [parent, dialog]);
    assert!(!events.kinds().contains(&(CLIENT, event::BUTTON)));
    let mut damage = Damage::new();
    desktop
        .destroy(CLIENT, u32::from(parent), &mut damage, &mut events)
        .unwrap();
    assert!(!desktop.is_shown(dialog) && desktop.window_rect(dialog).is_none());
    assert_eq!(desktop.windows_of(CLIENT), 0);
}

#[test]
fn requests_check_the_window_s_owner_role_flags_and_the_limits() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut damage = Damage::new();
    let terminal = u32::from(desktop.terminal_window());
    assert_eq!(
        desktop.set_title(CLIENT, terminal, b"mine", &mut damage),
        Err(WindowError::Invalid)
    );
    for role in [2, 3, 4] {
        assert_eq!(
            desktop.create(CLIENT, role, 0, 10, 10, 0),
            Err(WindowError::NotAllowed)
        );
    }
    assert_eq!(
        desktop.create(CLIENT, 5, 0, 10, 10, 0),
        Err(WindowError::Invalid)
    );
    assert_eq!(
        desktop.create(CLIENT, 0, 0, 10, 10, 4),
        Err(WindowError::Invalid)
    );
    let ids: Vec<_> = (0..MAX_WINDOWS_PER_CLIENT)
        .map(|_| desktop.create(CLIENT, 0, 0, 10, 10, 0).unwrap().window)
        .collect();
    assert_eq!(
        desktop.create(CLIENT, 0, 0, 10, 10, 0),
        Err(WindowError::TableFull)
    );
    let mut made = 1 + MAX_WINDOWS_PER_CLIENT;
    for client in 10..14 {
        while desktop.create(client, 0, 0, 10, 10, 0).is_ok() {
            made += 1;
        }
    }
    assert_eq!(made, MAX_WINDOWS);
    assert_eq!(
        desktop.create(20, 0, 0, 10, 10, 0),
        Err(WindowError::TableFull)
    );
    let window = u32::from(ids[0]);
    assert_eq!(
        desktop.set_title(CLIENT, window, &[0xff, 0xfe], &mut damage),
        Err(WindowError::Invalid)
    );
    assert_eq!(
        desktop.set_title(CLIENT, window, &[b'x'; 65], &mut damage),
        Err(WindowError::Invalid)
    );
    assert_eq!(
        desktop.set_state(CLIENT, window, 3, &mut damage, &mut DiscardEvents),
        Err(WindowError::Invalid)
    );
    assert_eq!(
        desktop.set_pointer(CLIENT, window, 6, &mut damage),
        Err(WindowError::Invalid)
    );
    assert_eq!(desktop.set_pointer(CLIENT, window, 1, &mut damage), Ok(()));
    assert_eq!(
        desktop.commit(OTHER, window, &[], &mut damage, &mut DiscardEvents),
        Err(WindowError::Invalid)
    );
    desktop.destroy_client(CLIENT, &mut damage, &mut DiscardEvents);
    assert_eq!(desktop.windows_of(CLIENT), 0);
    let again = desktop.create(CLIENT, 0, 0, 10, 10, 0).unwrap().window;
    assert_ne!(again, ids[0], "a slot's next window has another generation");
    assert_eq!(slot_of(again), slot_of(ids[0]));
}

#[test]
fn a_commit_damages_only_its_rects_on_screen_and_a_resize_waits_for_the_next() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, CLIENT, 400, 300);
    let content = desktop.content_rect(window).unwrap();
    let mut damage = Damage::new();
    desktop
        .commit(
            CLIENT,
            u32::from(window),
            &[Rect::new(10, 20, 30, 40), Rect::new(390, 290, 50, 50)],
            &mut damage,
            &mut DiscardEvents,
        )
        .unwrap();
    assert_eq!(
        damage.rects(),
        [
            Rect::new(content.x + 10, content.y + 20, 30, 40),
            Rect::new(content.x + 390, content.y + 290, 10, 10)
        ]
    );
    let mut damage = Damage::new();
    let size = desktop
        .set_size(
            CLIENT,
            u32::from(window),
            500,
            200,
            &mut damage,
            &mut DiscardEvents,
        )
        .unwrap();
    assert_eq!(size, (500, 200));
    assert_eq!(
        desktop.window_rect(window),
        Some(Rect::new(343, 195, 502, 230))
    );
    let pixels = painted(&desktop);
    assert_eq!(
        at(&pixels, content.x + 5, content.y + 5),
        colour::WINDOW_BACKGROUND
    );
    let mut damage = Damage::new();
    desktop
        .commit(
            CLIENT,
            u32::from(window),
            &[Rect::new(0, 0, 1, 1)],
            &mut damage,
            &mut DiscardEvents,
        )
        .unwrap();
    assert_eq!(damage.rects(), [Rect::new(content.x, content.y, 500, 200)]);
    let pixels = painted(&desktop);
    assert_eq!(
        at(&pixels, content.x + 5, content.y + 5),
        pattern(window, 5, 5)
    );
}

#[test]
fn a_window_shows_its_buffer_inside_its_chrome_and_hides_what_it_covers() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, CLIENT, 400, 300);
    let content = desktop.content_rect(window).unwrap();
    let pixels = painted(&desktop);
    for (x, y) in [(0, 0), (17, 3), (399, 200), (200, 296)] {
        assert_eq!(
            at(&pixels, content.x + x, content.y + y),
            pattern(window, x, y),
            "content pixel ({x}, {y})"
        );
    }
    assert_eq!(
        at(&pixels, content.x + 2, content.bottom() - 1),
        colour::BORDER_ACTIVE
    );
    assert_eq!(
        at(&pixels, content.x + 3, content.bottom() - 1),
        pattern(window, 3, 299)
    );
    assert_eq!(at(&pixels, 343 + 12 + 6, 195 + 14), colour::BUTTON_CLOSE);
    assert_eq!(at(&pixels, 343 + 32 + 6, 195 + 14), colour::BUTTON_MINIMISE);
    assert_eq!(at(&pixels, 311 + 12 + 6, 163 + 14), colour::BUTTON_INACTIVE);
    assert_eq!(at(&pixels, 400, 174), colour::TITLE_INACTIVE);
}

#[test]
fn painting_in_many_rects_equals_painting_the_whole_screen_with_three_windows() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let first = shown(&mut desktop, CLIENT, 300, 200);
    shown(&mut desktop, OTHER, 500, 250);
    let mut damage = Damage::new();
    desktop
        .set_title(CLIENT, u32::from(first), b"First", &mut damage)
        .unwrap();
    let whole = painted(&desktop);
    let mut parts = vec![0; 1280 * 800];
    for y in (0..800).step_by(100) {
        for x in (0..1280).step_by(128) {
            desktop.draw(&mut parts, 1280, Rect::new(x, y, 128, 100), &BlankTerminal);
        }
    }
    assert!(whole == parts);
    let mut fast = whole.clone();
    let content = desktop.content_rect(first).unwrap();
    fast[content.y as usize * 1280 + content.x as usize] = 1;
    desktop.draw(
        &mut fast,
        1280,
        Rect::new(content.x, content.y, 4, 4),
        &BlankTerminal,
    );
    assert!(fast == whole);
}

#[test]
fn request_numbers_and_layouts_are_the_contract_s() {
    assert_eq!(request::CREATE, 0xc020_7701);
    assert_eq!(request::SET_TITLE, 0x4048_7702);
    assert_eq!(request::SET_SIZE, 0xc018_7703);
    assert_eq!(request::SET_MINIMUM_SIZE, 0x400c_7704);
    assert_eq!(request::SET_POSITION, 0x400c_7705);
    assert_eq!(request::SET_STATE, 0x4008_7706);
    assert_eq!(request::SET_POINTER, 0x4008_7707);
    assert_eq!(request::COMMIT, 0x4088_7708);
    assert_eq!(request::DESTROY, 0x4004_7709);
    assert_eq!(request::GET_WORK_AREA, 0x8014_770a);
    assert_eq!(request::GET_PROFILE, 0x8004_770d);
    assert_eq!(request::SET_PROFILE, 0x4004_770e);
    assert_eq!(request::APPS_CHANGED, 0x0000_770f);
    assert_eq!(request::OPEN_APP, 0x4044_7710);
    assert_eq!(
        Request::parse(request::APPS_CHANGED, &[]),
        Some(Request::AppsChanged)
    );
    let mut open = [0u8; 68];
    open[..4].copy_from_slice(&5u32.to_ne_bytes());
    open[4..9].copy_from_slice(b"Paint");
    match Request::parse(request::OPEN_APP, &open) {
        Some(Request::OpenApp { length: 5, name }) => assert_eq!(&name[..5], b"Paint"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        Request::parse(request::SET_PROFILE, &2u32.to_ne_bytes()),
        Some(Request::SetProfile { profile: 2 })
    );
    assert_eq!(
        Request::parse(request::GET_PROFILE, &[0; 4]),
        Some(Request::GetProfile)
    );
    assert_eq!(size::COMMIT, 8 + 16 * MAX_COMMIT_RECTS);
    assert_eq!(
        (
            create_reply::WIDTH,
            create_reply::WINDOW,
            create_reply::OFFSET
        ),
        (8, 20, 24)
    );
    assert_eq!((set_size_reply::WIDTH, set_size_reply::OFFSET), (4, 16));
    let mut create = [0u8; 32];
    for (offset, value) in [(0, 1u32), (4, 65), (8, 400), (12, 300), (16, 2)] {
        create[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    assert_eq!(
        Request::parse(request::CREATE, &create),
        Some(Request::Create {
            role: 1,
            parent: 65,
            width: 400,
            height: 300,
            flags: 2
        })
    );
    assert_eq!(Request::parse(request::CREATE, &create[..31]), None);
    assert_eq!(Request::parse(0x5401, &create), None);
    let mut commit = [0u8; 136];
    commit[4..8].copy_from_slice(&2u32.to_ne_bytes());
    commit[8 + 16 + 8..8 + 16 + 12].copy_from_slice(&7u32.to_ne_bytes());
    let Some(Request::Commit { count, rects, .. }) = Request::parse(request::COMMIT, &commit)
    else {
        panic!("a commit parses");
    };
    assert_eq!((count, rects[1]), (2, Rect::new(0, 0, 7, 0)));
    let mut position = [0u8; 12];
    position[4..8].copy_from_slice(&(-3i32).to_ne_bytes());
    assert_eq!(
        Request::parse(request::SET_POSITION, &position),
        Some(Request::SetPosition {
            window: 0,
            x: -3,
            y: 0
        })
    );
}
