use super::protocol::{DiscardEvents, EventRecord, WindowAction, event};
use super::tests::{BlankTerminal, at, painted, pattern};
use super::*;
use crate::input::keymap::Modifiers;
use alloc::vec;

#[derive(Default)]
struct Recorded(Vec<(u16, EventRecord)>);

impl EventSink for Recorded {
    fn send(&mut self, client: u16, record: EventRecord) {
        self.0.push((client, record));
    }
}

const CLIENT: u16 = 5;
const WIDE: u32 = 658;

fn desktop(profile: Profile) -> Desktop {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    desktop.set_profile(profile, &mut Damage::new(), &mut DiscardEvents);
    desktop
}

fn shown(desktop: &mut Desktop, outside_width: u32) -> WindowId {
    let created = desktop
        .create(CLIENT, 0, 0, outside_width - CHROME.0, 300, 0)
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

fn move_to(desktop: &mut Desktop, id: WindowId, x: i32, y: i32) {
    desktop
        .manage(
            u32::from(id),
            WindowAction::Move as u32,
            x,
            y,
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .unwrap();
}

fn click(desktop: &mut Desktop, x: u32, y: u32) -> Action {
    let mut damage = Damage::new();
    desktop.pointer_moved(x, y, &mut damage, &mut DiscardEvents);
    let action = desktop.left_button(true, &mut damage, &mut DiscardEvents);
    desktop.left_button(false, &mut damage, &mut DiscardEvents);
    action
}

fn tick(desktop: &mut Desktop, now_ms: u32) {
    desktop.tick(now_ms, &mut Damage::new(), &mut DiscardEvents);
}

fn shape(desktop: &Desktop) -> PointerShape {
    desktop.pointer().unwrap().2
}

fn content_point(desktop: &Desktop, id: WindowId) -> (u32, u32) {
    let content = desktop.content_rect(id).unwrap();
    (content.x + content.width / 2, content.y + 20)
}

#[test]
fn a_window_hangs_off_the_left_keeping_its_grab_and_the_controls_on_that_side() {
    let mut desktop = desktop(Profile::Windows);
    let id = shown(&mut desktop, WIDE);
    let work = desktop.work_area();
    let x = |desktop: &Desktop| desktop.window_bounds(id).unwrap().x;
    move_to(&mut desktop, id, -5000, 100);
    assert_eq!(x(&desktop), work.x as i32 - 542);
    move_to(&mut desktop, id, 5000, 100);
    assert_eq!(x(&desktop), work.right() as i32 - 48);
    move_to(&mut desktop, id, 0, -100);
    assert_eq!(desktop.window_bounds(id).unwrap().y, work.y);
    move_to(&mut desktop, id, 0, 5000);
    assert_eq!(
        desktop.window_bounds(id).unwrap().y,
        work.bottom() - TITLE_HEIGHT - 1
    );
    let mut desktop = self::desktop(Profile::Macos);
    let id = shown(&mut desktop, WIDE);
    let work = desktop.work_area();
    let x = |desktop: &Desktop| desktop.window_bounds(id).unwrap().x;
    move_to(&mut desktop, id, -600, 100);
    assert_eq!(x(&desktop), work.x as i32 - 600);
    move_to(&mut desktop, id, -5000, 100);
    assert_eq!(x(&desktop), work.x as i32 - 610);
    move_to(&mut desktop, id, 5000, 100);
    assert_eq!(x(&desktop), work.right() as i32 - 116);
    assert_eq!(
        windows::hanging_x_range(100, work, ControlSide::Right),
        (i64::from(work.x), i64::from(work.right() - 100)),
        "a narrow window stays wholly inside"
    );
    assert_eq!(windows::kept_when_hanging(ControlSide::Right, 2), (232, 96));
    assert_eq!(windows::kept_when_hanging(ControlSide::Left, 2), (96, 232));
}

#[test]
fn wm_list_reports_the_negative_x_and_a_smaller_work_area_clamps_again() {
    let mut desktop = desktop(Profile::Windows);
    let id = shown(&mut desktop, WIDE);
    move_to(&mut desktop, id, -600, 100);
    let listed = desktop.listed_window(1).unwrap();
    assert_eq!(listed.id, id);
    assert_eq!(listed.rect.x, -542);
    assert_eq!(desktop.window_rect(id).unwrap().x, 0, "the part on the screen");
    move_to(&mut desktop, id, 5000, 100);
    desktop.set_profile(Profile::Macos, &mut Damage::new(), &mut DiscardEvents);
    let work = desktop.work_area();
    assert_eq!(
        desktop.window_bounds(id).unwrap().x,
        work.right() as i32 - 116
    );
}

#[test]
fn dragging_the_title_bar_stops_at_the_left_limit_and_keeps_the_offset() {
    let mut desktop = desktop(Profile::Windows);
    let id = shown(&mut desktop, WIDE);
    let title = desktop.title_bar_of(id).unwrap();
    let mut damage = Damage::new();
    let grab = (title.x + 560, title.y + 10);
    desktop.pointer_moved(grab.0, grab.1, &mut damage, &mut DiscardEvents);
    desktop.left_button(true, &mut damage, &mut DiscardEvents);
    let x = |desktop: &Desktop| desktop.window_bounds(id).unwrap().x;
    desktop.pointer_moved(0, grab.1, &mut damage, &mut DiscardEvents);
    assert_eq!(x(&desktop), -542, "the limit");
    desktop.pointer_moved(10, grab.1, &mut damage, &mut DiscardEvents);
    assert_eq!(x(&desktop), -542, "the pointer went on, the window stayed");
    desktop.pointer_moved(30, grab.1, &mut damage, &mut DiscardEvents);
    assert_eq!(x(&desktop), -530, "the offset was kept");
    desktop.left_button(false, &mut damage, &mut DiscardEvents);
}

#[test]
fn a_hanging_window_is_hit_drawn_and_damaged_by_its_signed_position() {
    let mut desktop = desktop(Profile::Windows);
    let id = shown(&mut desktop, WIDE);
    move_to(&mut desktop, id, -5000, 100);
    let slot = desktop.find(id).unwrap();
    let window = *desktop.window_at(slot);
    assert_eq!(window.bounds().x, -542);
    assert_eq!(window.outside(), Rect::new(0, 100, 116, window.outside().height));
    assert_eq!(window.content_origin(), (-541, 100 + 1 + i64::from(TITLE_HEIGHT)));
    let content_top = 100 + 1 + TITLE_HEIGHT;
    assert_eq!(
        window.part_at(0, content_top + 5, ControlSide::Right),
        Some(Part::Content)
    );
    assert_eq!(
        window.part_at(0, 110, ControlSide::Right),
        Some(Part::TitleBar)
    );
    assert_eq!(window.resize_edges(0, content_top + 50), 0, "the left edge is off screen");
    assert_eq!(window.resize_edges(116, content_top + 50), edge::RIGHT);
    assert!(
        window
            .control(Part::Close, ControlSide::Right)
            .is_some_and(|close| close.x > 0 && close.right() <= 116)
    );
    assert!(window.content_to_screen(Rect::new(0, 0, 10, 10)).is_empty());
    assert_eq!(
        window.content_to_screen(Rect::new(540, 0, 20, 10)),
        Rect::new(0, content_top, 19, 10)
    );
    let mut events = Recorded::default();
    let mut damage = Damage::new();
    desktop.pointer_moved(3, content_top + 7, &mut damage, &mut events);
    desktop.left_button(true, &mut damage, &mut events);
    desktop.left_button(false, &mut damage, &mut events);
    let pressed = events
        .0
        .iter()
        .find(|(_, record)| record.kind == event::BUTTON)
        .map(|(_, record)| (record.a, record.b))
        .unwrap();
    assert_eq!(pressed, (544, 7));
    let mut damage = Damage::new();
    desktop
        .commit(
            CLIENT,
            u32::from(id),
            &[Rect::new(540, 0, 20, 10)],
            &mut damage,
            &mut DiscardEvents,
        )
        .unwrap();
    assert_eq!(damage.rects(), &[Rect::new(0, content_top, 19, 10)]);
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 0, content_top + 1), pattern(id, 541, 1));
    assert_eq!(at(&pixels, 114, content_top + 1), pattern(id, 655, 1));
    let mut parts = vec![0; 1280 * 800];
    for y in (0..800).step_by(100) {
        for x in (0..1280).step_by(40) {
            desktop.draw(&mut parts, 1280, Rect::new(x, y, 40, 100), &BlankTerminal);
        }
    }
    assert!(pixels == parts, "drawn in pieces as a whole");
}

#[test]
fn working_takes_the_arrow_s_place_until_the_window_maps_the_app_ends_or_5_s_pass() {
    let mut desktop = desktop(Profile::Windows);
    tick(&mut desktop, 1000);
    let mut damage = Damage::new();
    desktop.pointer_moved(5, 400, &mut damage, &mut DiscardEvents);
    assert_eq!(shape(&desktop), PointerShape::Arrow);
    let started = desktop.launch(AppId::Settings, &mut damage, &mut DiscardEvents);
    assert_eq!(started, Action::Start(AppId::Settings));
    assert_eq!(shape(&desktop), PointerShape::Working);
    tick(&mut desktop, 1000 + APP_STARTING_MS - 1);
    assert_eq!(shape(&desktop), PointerShape::Working);
    tick(&mut desktop, 1000 + APP_STARTING_MS);
    assert_eq!(shape(&desktop), PointerShape::Arrow);
    tick(&mut desktop, 10_000);
    desktop.launch(AppId::Settings, &mut damage, &mut DiscardEvents);
    tick(&mut desktop, 13_000);
    desktop.launch(AppId::Settings, &mut damage, &mut DiscardEvents);
    tick(&mut desktop, 17_999);
    assert_eq!(shape(&desktop), PointerShape::Working, "the second start restarts it");
    tick(&mut desktop, 18_000);
    assert_eq!(shape(&desktop), PointerShape::Arrow);
    tick(&mut desktop, 20_000);
    desktop.launch(AppId::Settings, &mut damage, &mut DiscardEvents);
    assert_eq!(shape(&desktop), PointerShape::Working);
    shown(&mut desktop, 400);
    desktop.pointer_moved(5, 400, &mut damage, &mut DiscardEvents);
    assert_eq!(shape(&desktop), PointerShape::Arrow, "the first window mapped");
    tick(&mut desktop, 30_000);
    desktop.launch(AppId::Settings, &mut damage, &mut DiscardEvents);
    assert!(desktop.is_app_starting());
    desktop.started_app_ended(&mut damage);
    assert_eq!(shape(&desktop), PointerShape::Arrow, "its process ended");
}

#[test]
fn working_yields_to_every_more_specific_shape() {
    let mut desktop = desktop(Profile::Windows);
    let id = shown(&mut desktop, WIDE);
    tick(&mut desktop, 1000);
    let mut damage = Damage::new();
    desktop.launch(AppId::Settings, &mut damage, &mut DiscardEvents);
    let title = desktop.title_bar_of(id).unwrap();
    desktop.pointer_moved(title.x + 100, title.y + 10, &mut damage, &mut DiscardEvents);
    assert_eq!(shape(&desktop), PointerShape::Working, "over a title bar");
    let (x, y) = content_point(&desktop, id);
    desktop.pointer_moved(x, y, &mut damage, &mut DiscardEvents);
    assert_eq!(shape(&desktop), PointerShape::Working, "content whose client chose the arrow");
    desktop
        .set_pointer(CLIENT, u32::from(id), 7, &mut damage)
        .unwrap();
    assert_eq!(shape(&desktop), PointerShape::Hand);
    desktop
        .set_pointer(CLIENT, u32::from(id), 9, &mut damage)
        .unwrap();
    assert_eq!(shape(&desktop), PointerShape::Working, "a client's own Working");
    desktop
        .set_pointer(CLIENT, u32::from(id), 0, &mut damage)
        .unwrap();
    let outside = desktop.window_rect(id).unwrap();
    desktop.pointer_moved(outside.right(), y, &mut damage, &mut DiscardEvents);
    assert_eq!(shape(&desktop), PointerShape::ResizeEastWest);
    desktop.set_unread_since(CLIENT, Some(1000), &mut damage);
    tick(&mut desktop, 1000 + UNREAD_EVENTS_BUSY_MS);
    desktop.pointer_moved(x, y, &mut damage, &mut DiscardEvents);
    assert_eq!(shape(&desktop), PointerShape::Busy);
}

#[test]
fn a_type_launch_shows_no_working() {
    let mut catalogue = Catalogue::new();
    let hello = b"eggos-app 1\nname=Hello\nsource=linux\nrun=/bin/hello\n";
    assert!(catalogue.add_installed(b"Hello", hello));
    let mut desktop = desktop(Profile::Windows);
    let mut damage = Damage::new();
    desktop.set_catalogue(catalogue, &mut damage, &mut DiscardEvents);
    let app = desktop.catalogue().app_named("Hello").unwrap();
    desktop.pointer_moved(5, 400, &mut damage, &mut DiscardEvents);
    assert_eq!(
        desktop.launch(app, &mut damage, &mut DiscardEvents),
        Action::Type(app)
    );
    assert!(!desktop.is_app_starting());
}

#[test]
fn a_client_with_an_event_unread_for_2_s_is_not_responding_until_it_reads() {
    let mut desktop = desktop(Profile::Windows);
    let id = shown(&mut desktop, WIDE);
    let mut damage = Damage::new();
    let (x, y) = content_point(&desktop, id);
    desktop.pointer_moved(x, y, &mut damage, &mut DiscardEvents);
    tick(&mut desktop, 1000);
    desktop.set_unread_since(CLIENT, Some(1000), &mut damage);
    tick(&mut desktop, 2999);
    assert!(!desktop.is_not_responding(id));
    assert!(desktop.listed_window(1).unwrap().responding);
    assert_eq!(shape(&desktop), PointerShape::Arrow);
    tick(&mut desktop, 3000);
    assert!(desktop.is_not_responding(id));
    assert!(!desktop.listed_window(1).unwrap().responding);
    assert_eq!(shape(&desktop), PointerShape::Busy);
    assert!(desktop.client_not_responding(CLIENT));
    assert_eq!(
        desktop.refuse_if_not_responding(u32::from(id)),
        Err(WindowError::NotResponding)
    );
    assert_eq!(
        desktop.not_responding_window(u32::from(id)).map(|window| window.id),
        Some(id)
    );
    let title = desktop.title_bar_of(id).unwrap();
    desktop.pointer_moved(title.x + 100, title.y + 10, &mut damage, &mut DiscardEvents);
    assert_eq!(shape(&desktop), PointerShape::Arrow, "the title bar keeps the arrow");
    desktop.pointer_moved(x, y, &mut damage, &mut DiscardEvents);
    tick(&mut desktop, 9000);
    desktop.set_unread_since(CLIENT, None, &mut damage);
    assert!(!desktop.is_not_responding(id), "a read clears it at once");
    assert_eq!(shape(&desktop), PointerShape::Arrow);
    assert_eq!(desktop.refuse_if_not_responding(u32::from(id)), Ok(()));
    tick(&mut desktop, 10_000);
    desktop.set_unread_since(CLIENT, Some(10_000), &mut damage);
    tick(&mut desktop, 11_000);
    desktop.set_unread_since(CLIENT, None, &mut damage);
    tick(&mut desktop, 11_500);
    desktop.set_unread_since(CLIENT, Some(11_500), &mut damage);
    tick(&mut desktop, 13_499);
    assert!(!desktop.is_not_responding(id));
    tick(&mut desktop, 13_500);
    assert!(desktop.is_not_responding(id), "the oldest unread event's clock");
    desktop.set_unread_since(CLIENT, None, &mut damage);
    tick(&mut desktop, 1_000_000);
    assert!(!desktop.is_not_responding(id), "no pending event: idle, not busy");
    assert_eq!(desktop.next_change_ms(), None);
}

#[test]
fn every_injected_input_to_a_window_not_responding_is_refused_and_moving_it_is_not() {
    let mut desktop = desktop(Profile::Windows);
    let id = shown(&mut desktop, WIDE);
    tick(&mut desktop, 1000);
    let mut damage = Damage::new();
    desktop.set_unread_since(CLIENT, Some(1000), &mut damage);
    tick(&mut desktop, 1000 + UNREAD_EVENTS_BUSY_MS);
    assert!(desktop.is_not_responding(id));
    let left = crate::input::button::LEFT;
    let inputs = [
        InjectedInput::PointerMove { x: 10, y: 10 },
        InjectedInput::Click {
            x: 10,
            y: 10,
            button: left,
        },
        InjectedInput::DoubleClick {
            x: 10,
            y: 10,
            button: left,
        },
        InjectedInput::Drag {
            x: 10,
            y: 10,
            to_x: 40,
            to_y: 40,
            button: left,
        },
        InjectedInput::Wheel {
            x: 10,
            y: 10,
            notches: 1,
            sideways: 0,
        },
        InjectedInput::Key {
            code: key::ENTER,
            modifiers: Modifiers::NONE,
        },
        InjectedInput::Character('a'),
    ];
    let mut events = Recorded::default();
    for input in inputs {
        assert_eq!(
            desktop.send_input(u32::from(id), input, &mut events),
            Err(WindowError::NotResponding),
            "{input:?}"
        );
    }
    assert!(events.0.is_empty(), "nothing reaches the app's queue");
    move_to(&mut desktop, id, 100, 120);
    assert_eq!(desktop.window_bounds(id).map(|bounds| bounds.x), Some(100));
    desktop.set_unread_since(CLIENT, None, &mut damage);
    assert_eq!(
        desktop.send_input(u32::from(id), InjectedInput::Character('a'), &mut events),
        Ok(Injected::Queued)
    );
}

#[test]
fn a_press_on_a_window_busy_for_5_s_offers_to_end_its_app() {
    let mut desktop = desktop(Profile::Windows);
    let id = shown(&mut desktop, WIDE);
    let (x, y) = content_point(&desktop, id);
    tick(&mut desktop, 1000);
    let mut damage = Damage::new();
    desktop.set_unread_since(CLIENT, Some(1000), &mut damage);
    let busy_from = 1000 + UNREAD_EVENTS_BUSY_MS;
    tick(&mut desktop, busy_from + END_APP_OFFER_MS - 1);
    click(&mut desktop, 1000, 650);
    assert_ne!(desktop.focused_window(), Some(id));
    click(&mut desktop, x, y);
    assert_eq!(desktop.end_app_area(), None, "4999 ms: the press is queued");
    assert_eq!(desktop.focused_window(), Some(id));
    tick(&mut desktop, busy_from + END_APP_OFFER_MS);
    click(&mut desktop, x, y);
    let dialog = desktop.end_app_area().unwrap();
    let work = desktop.work_area();
    let shown_part = desktop.window_rect(id).unwrap().clipped(1280, 800);
    assert_eq!((dialog.width, dialog.height), END_APP_SIZE);
    assert_eq!(
        dialog.x + dialog.width / 2,
        shown_part.x + shown_part.width / 2
    );
    assert!(dialog.y >= work.y && dialog.bottom() <= work.bottom());
    assert_eq!(desktop.end_app_focus(), Some(EndAppButton::Wait));
    let keys = |desktop: &mut Desktop, code| {
        desktop.key(
            code,
            true,
            Modifiers::NONE,
            &mut Damage::new(),
            &mut DiscardEvents,
        )
    };
    keys(&mut desktop, key::ENTER);
    assert_eq!(desktop.end_app_area(), None);
    click(&mut desktop, x, y);
    keys(&mut desktop, key::ESC);
    assert_eq!(desktop.end_app_area(), None);
    click(&mut desktop, x, y);
    click(&mut desktop, 1000, 650);
    assert_eq!(desktop.end_app_area(), None, "a click outside waits");
    assert!(desktop.window_rect(id).is_some());
    click(&mut desktop, x, y);
    desktop.set_unread_since(CLIENT, None, &mut damage);
    assert_eq!(desktop.end_app_area(), None, "a read closes it by itself");
    desktop.set_unread_since(CLIENT, Some(1000), &mut damage);
    click(&mut desktop, x, y);
    let end = desktop.end_app_button(EndAppButton::EndApp).unwrap();
    assert_eq!(
        click(&mut desktop, end.x + 10, end.y + 10),
        Action::EndClient(CLIENT)
    );
    assert!(desktop.window_rect(id).is_none(), "gone in the same tick");
    assert_eq!(desktop.end_app_area(), None);
}

#[test]
fn the_end_app_dialog_of_eggway_says_every_linux_window_app_ends_with_it() {
    let mut desktop = desktop(Profile::Windows);
    let id = shown(&mut desktop, WIDE);
    let (x, y) = content_point(&desktop, id);
    tick(&mut desktop, 1000);
    let mut damage = Damage::new();
    desktop.set_unread_since(CLIENT, Some(1000), &mut damage);
    tick(&mut desktop, 1000 + UNREAD_EVENTS_BUSY_MS + END_APP_OFFER_MS);
    assert_eq!(desktop.end_app_warnings(), None);
    click(&mut desktop, x, y);
    assert_eq!(
        desktop.end_app_warnings(),
        Some(&[super::responsiveness::END_APP_WARNING][..])
    );
    desktop.set_linux_apps_client(CLIENT);
    assert_eq!(
        desktop.end_app_warnings(),
        Some(
            &[
                super::responsiveness::END_APP_WARNING,
                super::responsiveness::END_APP_LINUX_APPS_WARNING
            ][..]
        )
    );
    let cells = super::responsiveness::END_APP_LINUX_APPS_WARNING.chars().count() as u32;
    let width = cells * crate::font::GLYPH_WIDTH as u32;
    assert!(12 + width <= END_APP_SIZE.0 - 12, "fits its line");
    assert!(60 + crate::font::GLYPH_HEIGHT as u32 <= 80, "above the buttons");
    desktop.forget_client(CLIENT, &mut damage);
    desktop.set_unread_since(CLIENT, Some(20_000), &mut damage);
    tick(&mut desktop, 20_000 + UNREAD_EVENTS_BUSY_MS + END_APP_OFFER_MS);
    click(&mut desktop, x, y);
    assert_eq!(
        desktop.end_app_warnings(),
        Some(&[super::responsiveness::END_APP_WARNING][..]),
        "a client that disconnected is no longer eggway"
    );
}

#[test]
fn the_launcher_s_caret_blinks_then_stays_and_reduce_motion_keeps_it_shown() {
    let mut desktop = desktop(Profile::Windows);
    let t0 = 100_000;
    tick(&mut desktop, t0);
    desktop.toggle_launcher(&mut Damage::new(), &mut DiscardEvents);
    let drawn = |desktop: &mut Desktop, at: u32| {
        tick(desktop, at);
        desktop.launcher_caret_drawn().unwrap()
    };
    assert!(drawn(&mut desktop, t0));
    assert!(drawn(&mut desktop, t0 + 499));
    assert!(!drawn(&mut desktop, t0 + 500));
    assert!(!drawn(&mut desktop, t0 + 999));
    assert!(drawn(&mut desktop, t0 + 1000));
    desktop.set_time(t0 + 700);
    desktop.key(
        key::A,
        true,
        Modifiers::NONE,
        &mut Damage::new(),
        &mut DiscardEvents,
    );
    assert!(drawn(&mut desktop, t0 + 700));
    assert!(drawn(&mut desktop, t0 + 1199));
    let caret = desktop.launcher_caret().unwrap();
    let mut damage = Damage::new();
    desktop.tick(t0 + 1200, &mut damage, &mut DiscardEvents);
    assert_eq!(desktop.launcher_caret_drawn(), Some(false));
    assert_eq!(damage.rects(), &[caret], "only the caret's own rectangle");
    assert_eq!((caret.width, caret.height), (1, 16));
    for after in [10_000, 10_250, 10_500, 60_000] {
        assert!(drawn(&mut desktop, t0 + 700 + after), "{after}");
    }
    assert_eq!(desktop.next_change_ms(), None, "no timer while it is steady");
    desktop.set_time(t0 + 70_000);
    desktop.key(
        key::BACKSPACE,
        true,
        Modifiers::NONE,
        &mut Damage::new(),
        &mut DiscardEvents,
    );
    assert_eq!(desktop.next_change_ms(), Some(t0 + 70_500));
    desktop.set_reduce_motion(true, &mut Damage::new());
    assert!(drawn(&mut desktop, t0 + 70_600));
    assert_eq!(desktop.next_change_ms(), None);
    assert!(caret_shown_at(0, 600, true));
    assert!(!caret_shown_at(0, 600, false));
}

fn tap(desktop: &mut Desktop, code: u16, modifiers: Modifiers) -> Action {
    let mut damage = Damage::new();
    let action = desktop.key_typing(code, true, modifiers, 0, &mut damage, &mut DiscardEvents);
    desktop.key_typing(code, false, Modifiers::NONE, 0, &mut damage, &mut DiscardEvents);
    action
}

#[test]
fn shift_tapped_five_times_within_two_seconds_each_toggles_sticky_keys() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut damage = Damage::new();
    for _ in 0..4 {
        assert_eq!(tap(&mut desktop, key::LEFT_SHIFT, Modifiers::SHIFT), Action::None);
    }
    assert_eq!(
        tap(&mut desktop, key::RIGHT_SHIFT, Modifiers::SHIFT),
        Action::ToggleStickyKeys
    );
    for _ in 0..4 {
        tap(&mut desktop, key::LEFT_SHIFT, Modifiers::SHIFT);
    }
    tap(&mut desktop, 30, Modifiers::NONE);
    assert_eq!(tap(&mut desktop, key::LEFT_SHIFT, Modifiers::SHIFT), Action::None);
    for _ in 0..3 {
        tap(&mut desktop, key::LEFT_SHIFT, Modifiers::SHIFT);
    }
    desktop.tick(STICKY_TOGGLE_WITHIN_MS + 1, &mut damage, &mut DiscardEvents);
    assert_eq!(tap(&mut desktop, key::LEFT_SHIFT, Modifiers::SHIFT), Action::None);
    for _ in 0..3 {
        tap(&mut desktop, key::LEFT_SHIFT, Modifiers::SHIFT);
    }
    assert_eq!(
        tap(&mut desktop, key::LEFT_SHIFT, Modifiers::SHIFT),
        Action::ToggleStickyKeys
    );
    assert_eq!(
        tap(
            &mut desktop,
            key::LEFT_SHIFT,
            Modifiers::from_bits(Modifiers::SHIFT.bits() | Modifiers::CTRL.bits())
        ),
        Action::None
    );
}

#[test]
fn the_sticky_keys_toggle_runs_settings_set() {
    let desktop = Desktop::new(1280, 800).unwrap();
    let words = |on: bool| {
        desktop
            .sticky_keys_command(on)
            .map(|command| command.words().map(<[u8]>::to_vec).collect::<Vec<_>>())
    };
    assert_eq!(
        words(true),
        Some(vec![
            b"/bin/settings".to_vec(),
            b"set".to_vec(),
            b"accessibility.sticky_keys".to_vec(),
            b"true".to_vec(),
        ])
    );
    assert_eq!(words(false).unwrap()[3], b"false");
}
