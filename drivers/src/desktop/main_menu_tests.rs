use super::protocol::{EventRecord, Request, request, size};
use super::tests::{BlankTerminal, at, painted};
use super::*;
use crate::input::keymap::key_for_ascii;
use alloc::vec;

#[derive(Default)]
struct Recorded(Vec<(u16, EventRecord)>);

impl EventSink for Recorded {
    fn send(&mut self, client: u16, record: EventRecord) {
        self.0.push((client, record));
    }
}

impl Recorded {
    fn chosen(&self) -> Vec<(u16, WindowId, i32, i32)> {
        self.0
            .iter()
            .filter(|(_, record)| record.kind == event::MENU_CHOSEN)
            .map(|&(client, record)| (client, record.window, record.a, record.b))
            .collect()
    }

    fn of_kind(&self, kind: u16) -> Vec<(u16, EventRecord)> {
        self.0
            .iter()
            .copied()
            .filter(|(_, record)| record.kind == kind)
            .collect()
    }

    fn take(&mut self) -> Vec<(u16, EventRecord)> {
        core::mem::take(&mut self.0)
    }
}

const CLIENT: u16 = 3;
const OTHER: u16 = 7;
const MAC_PAINT: &[u8] = b"Mmac-paint\nIAbout mac-paint\t\n-\nIQuit mac-paint\tCmd+Q\n\
MFile\nIClear\tCmd+K\nIClose\tCmd+W\nMHelp\nImac-paint Help\t\n";
const EDIT: &[u8] = b"MEdit\nIUndo\tCmd+Z\nDRedo\tShift+Cmd+Z\n-\nICut\tCmd+X\n";

fn shown(desktop: &mut Desktop, client: u16) -> WindowId {
    let created = desktop.create(client, 0, 0, 400, 300, 0).unwrap();
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

fn set(desktop: &mut Desktop, window: WindowId, pool: &[u8]) -> Result<(), WindowError> {
    desktop.set_menu(CLIENT, u32::from(window), pool, &mut Damage::new())
}

fn with_menu(desktop: &mut Desktop, client: u16, pool: &[u8]) -> WindowId {
    let window = shown(desktop, client);
    desktop
        .set_menu(client, u32::from(window), pool, &mut Damage::new())
        .unwrap();
    window
}

fn move_to(desktop: &mut Desktop, x: u32, y: u32, events: &mut Recorded) {
    desktop.pointer_moved(x, y, &mut Damage::new(), events);
}

fn click(desktop: &mut Desktop, x: u32, y: u32, events: &mut Recorded) {
    move_to(desktop, x, y, events);
    let mut damage = Damage::new();
    desktop.left_button(true, &mut damage, events);
    desktop.left_button(false, &mut damage, events);
}

fn click_in(desktop: &mut Desktop, rect: Rect, events: &mut Recorded) {
    click(
        desktop,
        rect.x + rect.width / 2,
        rect.y + rect.height / 2,
        events,
    );
}

fn tap(desktop: &mut Desktop, code: u16, events: &mut Recorded) {
    let mut damage = Damage::new();
    desktop.key(code, true, Modifiers::NONE, &mut damage, events);
    desktop.key(code, false, Modifiers::NONE, &mut damage, events);
}

fn lit(pixels: &[u32], rect: Rect, colour: u32) -> bool {
    (rect.y..rect.bottom()).any(|y| (rect.x..rect.right()).any(|x| at(pixels, x, y) == colour))
}

#[test]
fn the_request_number_layout_and_event_kind_are_the_contract_s() {
    assert_eq!(request::SET_MENU, 0x4100_7715);
    assert_eq!(size::SET_MENU, 256);
    assert_eq!(size::LARGEST, 260);
    assert_eq!(event::MENU_CHOSEN, 14);
    let mut bytes = [0u8; 256];
    bytes[..4].copy_from_slice(&65u32.to_ne_bytes());
    bytes[4..8].copy_from_slice(&6u32.to_ne_bytes());
    bytes[8..14].copy_from_slice(b"MFile\n");
    bytes[255] = b'x';
    match Request::parse(request::SET_MENU, &bytes) {
        Some(Request::SetMenu {
            window: 65,
            length: 6,
            pool,
        }) => {
            assert_eq!(&pool[..6], b"MFile\n");
            assert_eq!(pool[247], b'x');
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(Request::parse(request::SET_MENU, &bytes[..255]), None);
}

#[test]
fn a_pool_is_checked_against_the_contract_s_rules() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, CLIENT);
    let six_menus = b"MA\nMB\nMC\nMD\nME\nMF\n".to_vec();
    let mut seven_menus = six_menus.clone();
    seven_menus.extend_from_slice(b"MG\n");
    let mut sixteen_entries = b"MA\n".to_vec();
    sixteen_entries.extend(b"-\n".repeat(16));
    let mut seventeen_entries = sixteen_entries.clone();
    seventeen_entries.extend_from_slice(b"IOne more\t\n");
    let mut longest = vec![b'M'];
    longest.extend(b"a".repeat(246));
    longest.push(b'\n');
    let mut too_long = longest.clone();
    too_long.insert(1, b'a');
    assert_eq!((longest.len(), too_long.len()), (248, 249));
    let good: [&[u8]; 8] = [
        MAC_PAINT,
        EDIT,
        b"M\n",
        b"MA\nI\t\n",
        b"MA\nMB\n-\nI\xc3\xa9t\xc3\xa9\tCmd+.\n",
        &six_menus,
        &sixteen_entries,
        &longest,
    ];
    for pool in good {
        assert_eq!(set(&mut desktop, window, pool), Ok(()), "{pool:?}");
    }
    let bad: [&[u8]; 16] = [
        &seven_menus,
        &seventeen_entries,
        &too_long,
        b"IOpen\t\n",
        b"-\nMA\n",
        b"M\xff\n",
        b"MA\nI\xc3\t\n",
        b"MA\x01\n",
        b"MA\nI\xc2\x85\t\n",
        b"MA\nIA\tB\tC\n",
        b"MA\nIA\n",
        b"MA\nXA\n",
        b"MA\n-x\n",
        b"MA\n\n",
        b"MA",
        b"\n",
    ];
    for pool in bad {
        assert_eq!(
            set(&mut desktop, window, pool),
            Err(WindowError::Invalid),
            "{pool:?}"
        );
    }
    assert_eq!(set(&mut desktop, window, MAC_PAINT), Ok(()));
    assert_eq!(
        set(&mut desktop, window, b"MA\n\n"),
        Err(WindowError::Invalid)
    );
    assert_eq!(desktop.menu_title(0), Some(Rect::new(104, 0, 88, 28)));
    assert_eq!(set(&mut desktop, window, b""), Ok(()));
    assert_eq!(desktop.menu_title(0), None);
}

#[test]
fn only_the_client_s_own_normal_windows_take_a_menu() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, CLIENT);
    let id = u32::from(window);
    let mut damage = Damage::new();
    assert_eq!(
        desktop.set_menu(OTHER, id, MAC_PAINT, &mut damage),
        Err(WindowError::Invalid)
    );
    assert_eq!(
        desktop.set_menu(CLIENT, 0xffff_ffff, MAC_PAINT, &mut damage),
        Err(WindowError::Invalid)
    );
    let dialog = desktop.create(CLIENT, 1, id, 200, 100, 0).unwrap().window;
    assert_eq!(
        desktop.set_menu(CLIENT, u32::from(dialog), MAC_PAINT, &mut damage),
        Err(WindowError::Invalid)
    );
    let terminal = u32::from(desktop.terminal_window());
    assert_eq!(
        desktop.set_menu(KERNEL_CLIENT, terminal, MAC_PAINT, &mut damage),
        Err(WindowError::Invalid)
    );
    assert_eq!(desktop.set_menu(CLIENT, id, MAC_PAINT, &mut damage), Ok(()));
}

#[test]
fn the_titles_start_at_104_side_by_side_in_the_bar_s_font() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, CLIENT);
    let mut damage = Damage::new();
    desktop
        .set_menu(CLIENT, u32::from(window), MAC_PAINT, &mut damage)
        .unwrap();
    assert!(
        damage
            .rects()
            .iter()
            .all(|rect| rect.y == 0 && rect.bottom() <= 28),
        "{:?}",
        damage.rects()
    );
    assert_eq!(desktop.menu_title(0), Some(Rect::new(104, 0, 88, 28)));
    assert_eq!(desktop.menu_title(1), Some(Rect::new(192, 0, 48, 28)));
    assert_eq!(desktop.menu_title(2), Some(Rect::new(240, 0, 48, 28)));
    assert_eq!(desktop.menu_title(3), None);
    let pixels = painted(&desktop);
    assert!(lit(&pixels, Rect::new(112, 6, 72, 16), colour::TEXT));
    assert!(lit(&pixels, Rect::new(200, 6, 32, 16), colour::TEXT));
    assert!(lit(&pixels, Rect::new(248, 6, 32, 16), colour::TEXT));
    for empty in [
        Rect::new(104, 0, 184, 6),
        Rect::new(104, 0, 8, 27),
        Rect::new(184, 0, 16, 27),
        Rect::new(288, 0, 296, 27),
    ] {
        assert!(
            (empty.y..empty.bottom())
                .all(|y| (empty.x..empty.right()).all(|x| at(&pixels, x, y) == colour::BAR)),
            "{empty:?}"
        );
    }
}

#[test]
fn titles_past_the_clock_s_left_less_16_are_dropped_with_those_after_them() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = shown(&mut desktop, CLIENT);
    let mut pool = Vec::new();
    for title in [
        "Twenty glyphs title1",
        "Twenty glyphs title2",
        "Twenty glyphs title3",
    ] {
        pool.push(b'M');
        pool.extend_from_slice(title.as_bytes());
        pool.push(b'\n');
    }
    pool.extend_from_slice(b"MX\n");
    set(&mut desktop, window, &pool).unwrap();
    assert_eq!(desktop.clock_rect().x - 16, 584);
    assert_eq!(desktop.menu_title(0), Some(Rect::new(104, 0, 176, 28)));
    assert_eq!(desktop.menu_title(1), Some(Rect::new(280, 0, 176, 28)));
    assert_eq!(desktop.menu_title(2), None);
    assert_eq!(desktop.menu_title(3), None);
}

#[test]
fn a_click_on_a_title_opens_its_menu_under_it_and_a_click_on_an_item_chooses_it() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    move_to(&mut desktop, 210, 10, &mut events);
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 193, 2), colour::BAR_HOVER);
    assert_eq!(at(&pixels, 105, 2), colour::BAR);
    let mut damage = Damage::new();
    desktop.left_button(true, &mut damage, &mut events);
    desktop.left_button(false, &mut damage, &mut events);
    assert_eq!(desktop.main_menu_area(), Some(Rect::new(192, 32, 160, 66)));
    assert!(
        damage
            .rects()
            .iter()
            .all(|rect| rect.bottom() <= 32 + 66 + 4),
        "{:?}",
        damage.rects()
    );
    assert_eq!(desktop.focused_window(), Some(window));
    assert_eq!(
        desktop.main_menu_entry(1),
        Some(Rect::new(192, 65, 160, 32))
    );
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 193, 2), colour::BAR_PRESSED);
    assert_eq!(at(&pixels, 272, 32), colour::MENU_BORDER);
    assert_eq!(at(&pixels, 272, 40), colour::MENU);
    assert!(lit(&pixels, Rect::new(204, 41, 40, 16), colour::TEXT));
    assert!(lit(&pixels, Rect::new(300, 41, 40, 16), colour::TEXT_DIM));
    assert!(!lit(&pixels, Rect::new(340, 33, 11, 64), colour::TEXT_DIM));
    move_to(&mut desktop, 250, 80, &mut events);
    assert_eq!(at(&painted(&desktop), 196, 70), colour::MENU_HOVER);
    events.take();
    click(&mut desktop, 250, 80, &mut events);
    assert_eq!(events.chosen(), [(CLIENT, window, 1, 1)]);
    assert_eq!(desktop.main_menu_area(), None);
    assert!(events.of_kind(event::BUTTON).is_empty());
}

#[test]
fn entry_indices_count_separators_and_separators_choose_nothing() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    click(&mut desktop, 120, 10, &mut events);
    assert_eq!(desktop.main_menu_area(), Some(Rect::new(104, 32, 208, 75)));
    assert_eq!(desktop.main_menu_entry(1), Some(Rect::new(104, 65, 208, 9)));
    assert_eq!(
        desktop.main_menu_entry(2),
        Some(Rect::new(104, 74, 208, 32))
    );
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 112, 69), colour::MENU_BORDER);
    assert_eq!(at(&pixels, 111, 69), colour::MENU);
    assert_eq!(at(&pixels, 303, 69), colour::MENU_BORDER);
    assert_eq!(at(&pixels, 304, 69), colour::MENU);
    click(&mut desktop, 200, 69, &mut events);
    assert!(events.chosen().is_empty());
    assert!(desktop.main_menu_area().is_some());
    click(&mut desktop, 200, 90, &mut events);
    assert_eq!(events.chosen(), [(CLIENT, window, 0, 2)]);
}

#[test]
fn a_disabled_item_is_dim_and_ignores_the_pointer() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    with_menu(&mut desktop, CLIENT, EDIT);
    click(&mut desktop, 120, 10, &mut events);
    assert_eq!(desktop.main_menu_area(), Some(Rect::new(104, 32, 176, 107)));
    let redo = desktop.main_menu_entry(1).unwrap();
    move_to(&mut desktop, 150, redo.y + 16, &mut events);
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 107, redo.y + 5), colour::MENU);
    assert!(lit(
        &pixels,
        Rect::new(116, redo.y + 8, 32, 16),
        colour::TEXT_DIM
    ));
    assert!(!lit(
        &pixels,
        Rect::new(116, redo.y + 8, 32, 16),
        colour::TEXT
    ));
    click(&mut desktop, 150, redo.y + 16, &mut events);
    assert!(events.chosen().is_empty());
    assert!(desktop.main_menu_area().is_some());
}

#[test]
fn up_and_down_skip_disabled_items_and_separators_and_enter_chooses() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = with_menu(&mut desktop, CLIENT, EDIT);
    click(&mut desktop, 120, 10, &mut events);
    events.take();
    tap(&mut desktop, key::DOWN, &mut events);
    tap(&mut desktop, key::DOWN, &mut events);
    let cut = desktop.main_menu_entry(3).unwrap();
    assert_eq!(at(&painted(&desktop), 107, cut.y + 4), colour::MENU_HOVER);
    tap(&mut desktop, key::DOWN, &mut events);
    tap(&mut desktop, key::UP, &mut events);
    let typed = key_for_ascii(b'a').unwrap().0;
    tap(&mut desktop, typed, &mut events);
    assert!(events.of_kind(event::KEY).is_empty());
    tap(&mut desktop, key::ENTER, &mut events);
    assert_eq!(events.chosen(), [(CLIENT, window, 0, 3)]);
    assert_eq!(desktop.main_menu_area(), None);
    click(&mut desktop, 120, 10, &mut events);
    tap(&mut desktop, key::UP, &mut events);
    tap(&mut desktop, key::UP, &mut events);
    tap(&mut desktop, key::ENTER, &mut events);
    assert_eq!(events.chosen().last(), Some(&(CLIENT, window, 0, 0)));
    click(&mut desktop, 120, 10, &mut events);
    events.take();
    tap(&mut desktop, key::ESC, &mut events);
    assert_eq!(desktop.main_menu_area(), None);
    assert!(events.chosen().is_empty());
    tap(&mut desktop, typed, &mut events);
    let typed_down = events
        .of_kind(event::KEY)
        .into_iter()
        .filter(|(_, record)| record.a == i32::from(typed) && record.b == 1)
        .count();
    assert_eq!(typed_down, 1);
}

#[test]
fn left_right_and_hovering_another_title_switch_the_open_menu() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    click(&mut desktop, 200, 10, &mut events);
    assert_eq!(desktop.main_menu_area().map(|rect| rect.x), Some(192));
    tap(&mut desktop, key::RIGHT, &mut events);
    assert_eq!(desktop.main_menu_area(), Some(Rect::new(240, 32, 168, 34)));
    tap(&mut desktop, key::RIGHT, &mut events);
    assert_eq!(desktop.main_menu_area().map(|rect| rect.x), Some(104));
    tap(&mut desktop, key::LEFT, &mut events);
    assert_eq!(desktop.main_menu_area().map(|rect| rect.x), Some(240));
    tap(&mut desktop, key::ENTER, &mut events);
    assert_eq!(events.chosen(), [(CLIENT, window, 2, 0)]);
    click(&mut desktop, 200, 10, &mut events);
    move_to(&mut desktop, 120, 10, &mut events);
    assert_eq!(desktop.main_menu_area().map(|rect| rect.x), Some(104));
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 105, 2), colour::BAR_PRESSED);
    assert_eq!(at(&pixels, 193, 2), colour::BAR);
    click(&mut desktop, 120, 10, &mut events);
    assert_eq!(desktop.main_menu_area(), None);
}

#[test]
fn a_press_outside_closes_the_menu_and_is_delivered() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    click(&mut desktop, 200, 10, &mut events);
    events.take();
    let content = desktop.content_rect(window).unwrap();
    click_in(&mut desktop, content, &mut events);
    assert_eq!(desktop.main_menu_area(), None);
    assert!(events.chosen().is_empty());
    assert_eq!(events.of_kind(event::BUTTON).len(), 2);
}

#[test]
fn a_menu_wider_than_the_room_right_of_its_title_stays_8_px_inside_the_screen() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let mut pool = b"MA\nI".to_vec();
    pool.extend(b"x".repeat(140));
    pool.extend_from_slice(b"\t\n");
    with_menu(&mut desktop, CLIENT, &pool);
    click(&mut desktop, 110, 10, &mut events);
    assert_eq!(desktop.main_menu_area(), Some(Rect::new(96, 32, 1176, 34)));
}

#[test]
fn focus_moving_to_another_window_closes_the_menu_and_swaps_the_titles() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let first = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    with_menu(&mut desktop, OTHER, b"MOther\nIThing\t\n");
    assert_eq!(desktop.menu_title(0), Some(Rect::new(104, 0, 56, 28)));
    assert_eq!(desktop.menu_title(1), None);
    click(&mut desktop, 110, 10, &mut events);
    assert!(desktop.main_menu_area().is_some());
    let first_rect = desktop.window_rect(first).unwrap();
    click(
        &mut desktop,
        first_rect.x + 8,
        first_rect.y + 100,
        &mut events,
    );
    assert_eq!(desktop.focused_window(), Some(first));
    assert_eq!(desktop.main_menu_area(), None);
    assert_eq!(desktop.menu_title(0), Some(Rect::new(104, 0, 88, 28)));
    assert_eq!(desktop.menu_title(2), Some(Rect::new(240, 0, 48, 28)));
    click(&mut desktop, 200, 10, &mut events);
    assert!(desktop.main_menu_area().is_some());
    let mut damage = Damage::new();
    desktop.key(key::TAB, true, Modifiers::SUPER, &mut damage, &mut events);
    assert!(desktop.main_menu_area().is_some());
    let terminal = desktop.window();
    click(&mut desktop, terminal.x + 8, terminal.y + 300, &mut events);
    assert!(desktop.keys_go_to_terminal());
    assert_eq!(desktop.main_menu_area(), None);
    assert_eq!(desktop.menu_title(0), None);
    let pixels = painted(&desktop);
    assert!(!lit(&pixels, Rect::new(104, 0, 480, 27), colour::TEXT));
}

#[test]
fn destroying_the_window_closes_its_menu_and_its_titles() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    click(&mut desktop, 200, 10, &mut events);
    let mut damage = Damage::new();
    desktop
        .destroy(CLIENT, u32::from(window), &mut damage, &mut events)
        .unwrap();
    assert_eq!(desktop.main_menu_area(), None);
    assert_eq!(desktop.menu_title(0), None);
    assert!(
        damage
            .rects()
            .iter()
            .any(|rect| rect.overlaps(&Rect::new(192, 32, 160, 66)))
    );
    assert!(
        damage
            .rects()
            .iter()
            .any(|rect| rect.overlaps(&Rect::new(104, 0, 88, 28)))
    );
    let again = shown(&mut desktop, CLIENT);
    assert_eq!(slot_of(again), slot_of(window));
    assert_eq!(desktop.menu_title(0), None);
}

#[test]
fn a_focused_dialog_dims_the_titles_and_makes_them_inert() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    click(&mut desktop, 200, 10, &mut events);
    let dialog = desktop
        .create(CLIENT, 1, u32::from(window), 200, 100, 0)
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
    assert_eq!(desktop.focused_window(), Some(dialog));
    assert_eq!(desktop.main_menu_area(), None);
    assert_eq!(desktop.menu_title(0), Some(Rect::new(104, 0, 88, 28)));
    move_to(&mut desktop, 120, 10, &mut events);
    let pixels = painted(&desktop);
    assert!(lit(&pixels, Rect::new(112, 6, 72, 16), colour::TEXT_DIM));
    assert!(!lit(&pixels, Rect::new(104, 0, 184, 27), colour::TEXT));
    assert_eq!(at(&pixels, 105, 2), colour::BAR);
    click(&mut desktop, 120, 10, &mut events);
    assert_eq!(desktop.main_menu_area(), None);
    desktop
        .destroy(CLIENT, u32::from(dialog), &mut Damage::new(), &mut events)
        .unwrap();
    assert_eq!(desktop.focused_window(), Some(window));
    let pixels = painted(&desktop);
    assert!(lit(&pixels, Rect::new(112, 6, 72, 16), colour::TEXT));
    click(&mut desktop, 120, 10, &mut events);
    assert!(desktop.main_menu_area().is_some());
}

#[test]
fn the_windows_and_linux_profiles_show_no_menu() {
    for profile in [Profile::Windows, Profile::Linux] {
        let mut desktop = Desktop::new(1280, 800).unwrap();
        let mut events = Recorded::default();
        desktop.set_profile(profile, &mut Damage::new(), &mut events);
        let window = shown(&mut desktop, CLIENT);
        let before = painted(&desktop);
        let mut damage = Damage::new();
        desktop
            .set_menu(CLIENT, u32::from(window), MAC_PAINT, &mut damage)
            .unwrap();
        assert!(damage.is_empty(), "{profile:?}");
        assert!(painted(&desktop) == before, "{profile:?}");
        assert_eq!(desktop.menu_title(0), None);
        desktop.set_profile(Profile::Macos, &mut Damage::new(), &mut events);
        assert_eq!(desktop.menu_title(0), Some(Rect::new(104, 0, 88, 28)));
    }
}

#[test]
fn super_and_a_letter_the_profile_does_not_take_reach_the_focused_window() {
    for profile in Profile::ALL {
        let mut desktop = Desktop::new(1280, 800).unwrap();
        let mut events = Recorded::default();
        desktop.set_profile(profile, &mut Damage::new(), &mut events);
        let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
        for letter in *b"qk" {
            let (code, _) = key_for_ascii(letter).unwrap();
            events.take();
            let mut damage = Damage::new();
            desktop.key(
                key::LEFT_SUPER,
                true,
                Modifiers::SUPER,
                &mut damage,
                &mut events,
            );
            desktop.key(code, true, Modifiers::SUPER, &mut damage, &mut events);
            desktop.key(code, false, Modifiers::SUPER, &mut damage, &mut events);
            desktop.key(
                key::LEFT_SUPER,
                false,
                Modifiers::NONE,
                &mut damage,
                &mut events,
            );
            let pressed = events
                .of_kind(event::KEY)
                .into_iter()
                .filter(|(_, record)| record.a == i32::from(code) && record.b == 1)
                .collect::<Vec<_>>();
            assert_eq!(pressed.len(), 1, "{profile:?} {letter}");
            let (client, record) = pressed[0];
            assert_eq!((client, record.window), (CLIENT, window));
            assert_ne!(record.c & protocol::modifier::SUPER, 0);
            assert!(!desktop.launcher_open(), "{profile:?} {letter}");
            assert_eq!(desktop.focused_window(), Some(window));
        }
        let mut damage = Damage::new();
        desktop.key(
            key::LEFT_SUPER,
            true,
            Modifiers::SUPER,
            &mut damage,
            &mut events,
        );
        desktop.key(
            key::LEFT_SUPER,
            false,
            Modifiers::NONE,
            &mut damage,
            &mut events,
        );
        assert!(desktop.launcher_open(), "{profile:?}");
    }
}

#[test]
fn setting_the_menu_again_keeps_the_open_menu_and_drops_a_hover_now_disabled() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    click(&mut desktop, 200, 10, &mut events);
    tap(&mut desktop, key::DOWN, &mut events);
    let disabled: &[u8] = b"Mmac-paint\nIAbout mac-paint\t\n-\nIQuit mac-paint\tCmd+Q\n\
MFile\nDClear\tCmd+K\nIClose\tCmd+W\nMHelp\nImac-paint Help\t\n";
    set(&mut desktop, window, disabled).unwrap();
    assert_eq!(desktop.main_menu_area(), Some(Rect::new(192, 32, 160, 66)));
    tap(&mut desktop, key::ENTER, &mut events);
    assert!(events.chosen().is_empty());
    tap(&mut desktop, key::DOWN, &mut events);
    tap(&mut desktop, key::ENTER, &mut events);
    assert_eq!(events.chosen(), [(CLIENT, window, 1, 1)]);
    click(&mut desktop, 200, 10, &mut events);
    set(&mut desktop, window, b"").unwrap();
    assert_eq!(desktop.main_menu_area(), None);
    assert_eq!(desktop.menu_title(0), None);
}

#[test]
fn drawing_a_rect_is_the_same_as_drawing_the_whole_screen_with_a_menu_open() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    with_menu(&mut desktop, CLIENT, MAC_PAINT);
    click(&mut desktop, 120, 10, &mut events);
    move_to(&mut desktop, 200, 90, &mut events);
    let whole = painted(&desktop);
    let mut parts = vec![0; 1280 * 800];
    for y in (0..800).step_by(100) {
        for x in (0..1280).step_by(128) {
            desktop.draw(&mut parts, 1280, Rect::new(x, y, 128, 100), &BlankTerminal);
        }
    }
    assert!(whole == parts);
}

#[test]
fn get_and_choose_menu_have_linux_s_encoding_and_parse_their_fields() {
    assert_eq!(request::GET_MENU, 0xc100_7716);
    assert_eq!(request::CHOOSE_MENU, 0x400c_7717);
    assert_eq!((size::GET_MENU, size::CHOOSE_MENU), (256, 12));
    let mut bytes = [0u8; 256];
    bytes[..4].copy_from_slice(&65u32.to_ne_bytes());
    assert_eq!(
        Request::parse(request::GET_MENU, &bytes),
        Some(Request::GetMenu { window: 65 })
    );
    let mut choose = [0u8; 12];
    for (index, word) in [65u32, 1, 2].into_iter().enumerate() {
        choose[index * 4..index * 4 + 4].copy_from_slice(&word.to_ne_bytes());
    }
    assert_eq!(
        Request::parse(request::CHOOSE_MENU, &choose),
        Some(Request::ChooseMenu {
            window: 65,
            menu: 1,
            entry: 2
        })
    );
    assert_eq!(Request::parse(request::CHOOSE_MENU, &choose[..8]), None);
}

#[test]
fn any_window_s_menu_pool_reads_back_as_it_was_set() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let paint = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    let bare = shown(&mut desktop, OTHER);
    assert_eq!(desktop.menu_pool(u32::from(paint)), Ok(MAC_PAINT));
    assert_eq!(desktop.menu_pool(u32::from(bare)), Ok(&b""[..]));
    assert_eq!(desktop.menu_pool(9999), Err(WindowError::NoSuchWindow));
    set(&mut desktop, paint, EDIT).unwrap();
    assert_eq!(desktop.menu_pool(u32::from(paint)), Ok(EDIT));
}

#[test]
fn a_command_chooses_an_enabled_item_of_any_window_s_menu() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let paint = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    let edit = with_menu(&mut desktop, OTHER, EDIT);
    desktop.set_time(10_000);
    let mut events = Recorded::default();
    let mut choose = |desktop: &mut Desktop, window: u32, menu: u32, entry: u32| {
        desktop.choose_menu(window, menu, entry, &mut Damage::new(), &mut events)
    };
    assert_eq!(choose(&mut desktop, u32::from(paint), 1, 0), Ok(()));
    assert_eq!(choose(&mut desktop, u32::from(edit), 0, 3), Ok(()));
    for (window, menu, entry) in [
        (paint, 0, 1),
        (paint, 1, 2),
        (paint, 3, 0),
        (paint, u32::MAX, 0),
        (paint, 0, u32::MAX),
        (edit, 0, 1),
        (edit, 0, 2),
    ] {
        assert_eq!(
            choose(&mut desktop, u32::from(window), menu, entry),
            Err(WindowError::Invalid),
            "{menu} {entry}"
        );
    }
    assert_eq!(
        choose(&mut desktop, 9999, 0, 0),
        Err(WindowError::NoSuchWindow)
    );
    assert_eq!(
        events.chosen(),
        [(CLIENT, paint, 1, 0), (OTHER, edit, 0, 3)]
    );
}

#[test]
fn a_command_s_choice_closes_the_open_menu_and_waits_for_a_person() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut events = Recorded::default();
    let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    click(&mut desktop, 200, 10, &mut events);
    assert!(desktop.main_menu_area().is_some());
    events.take();
    let id = u32::from(window);
    assert_eq!(
        desktop.choose_menu(id, 1, 0, &mut Damage::new(), &mut events),
        Err(WindowError::Busy)
    );
    assert!(desktop.main_menu_area().is_some());
    desktop.set_time(1000);
    assert_eq!(
        desktop.choose_menu(id, 1, 0, &mut Damage::new(), &mut events),
        Ok(())
    );
    assert_eq!(desktop.main_menu_area(), None);
    assert_eq!(events.chosen(), [(CLIENT, window, 1, 0)]);
}

#[test]
fn a_command_s_choice_waits_for_room_in_the_client_s_queue() {
    struct Full;
    impl EventSink for Full {
        fn send(&mut self, _: u16, _: EventRecord) {
            panic!("nothing is sent");
        }

        fn room(&self, _: u16) -> usize {
            0
        }
    }
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let window = with_menu(&mut desktop, CLIENT, MAC_PAINT);
    desktop.set_time(10_000);
    assert_eq!(
        desktop.choose_menu(u32::from(window), 1, 0, &mut Damage::new(), &mut Full),
        Err(WindowError::QueueFull)
    );
}
