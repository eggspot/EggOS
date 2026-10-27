use super::protocol::DiscardEvents;
use super::tests::{fnv, painted};
use super::*;
use crate::input::keymap::Modifiers;

const CLIENT: u16 = 0;
const HELLO: &[u8] = b"eggos-app 1\nname=Hello\nsource=linux\nrun=/bin/hello\nsynonyms=greeter\n";
const PAINT: &[u8] = b"eggos-app 1\nname=Paint\nsource=windows\nrun=paint.exe\n";

fn catalogue() -> Catalogue {
    let mut catalogue = Catalogue::new();
    assert!(catalogue.add_installed(b"Hello", HELLO));
    assert!(catalogue.add_installed(b"Paint", PAINT));
    catalogue
}

fn desktop(profile: Profile) -> Desktop {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut damage = Damage::new();
    desktop.set_catalogue(catalogue(), &mut damage);
    desktop.set_profile(profile, &mut damage, &mut DiscardEvents);
    desktop
}

fn window(desktop: &mut Desktop, title: &str) -> WindowId {
    let created = desktop.create(CLIENT, 0, 0, 400, 300, 0).unwrap();
    let id = u32::from(created.window);
    let mut damage = Damage::new();
    desktop
        .set_title(CLIENT, id, title.as_bytes(), &mut damage)
        .unwrap();
    desktop
        .commit(CLIENT, id, &[], &mut damage, &mut DiscardEvents)
        .unwrap();
    created.window
}

fn press(desktop: &mut Desktop, x: u32, y: u32) -> Action {
    let mut damage = Damage::new();
    desktop.pointer_moved(x, y, &mut damage, &mut DiscardEvents);
    let action = desktop.left_button(true, &mut damage, &mut DiscardEvents);
    desktop.left_button(false, &mut damage, &mut DiscardEvents);
    action
}

fn click_in(desktop: &mut Desktop, rect: Rect) -> Action {
    press(desktop, rect.x + rect.width / 2, rect.y + rect.height / 2)
}

fn key(desktop: &mut Desktop, code: u16, pressed: bool, modifiers: Modifiers) -> Action {
    desktop.key(
        code,
        pressed,
        modifiers,
        &mut Damage::new(),
        &mut DiscardEvents,
    )
}

fn tap(desktop: &mut Desktop, code: u16, modifiers: Modifiers) -> Action {
    let action = key(desktop, code, true, modifiers);
    key(desktop, code, false, modifiers);
    action
}

fn type_text(desktop: &mut Desktop, text: &str) {
    for character in text.bytes() {
        let code = (0..crate::input::key::MAX)
            .find(|&code| {
                crate::input::keymap::printable(code).is_some_and(|[plain, _]| plain == character)
            })
            .unwrap();
        tap(desktop, code, Modifiers::from_bits(0));
    }
}

fn super_alone(desktop: &mut Desktop) {
    key(desktop, key::LEFT_SUPER, true, Modifiers::SUPER);
    key(desktop, key::LEFT_SUPER, false, Modifiers::from_bits(0));
}

fn names(desktop: &Desktop) -> Vec<String> {
    let (apps, _) = desktop.launcher_entries().unwrap();
    apps.iter()
        .map(|&app| desktop.catalogue().name(app).to_owned())
        .collect()
}

#[test]
fn the_dock_lists_the_pinned_apps_and_its_settings_icon_starts_settings() {
    let mut desktop = desktop(Profile::Macos);
    let settings = desktop.dock_icon(AppId::Settings).unwrap();
    let terminal = desktop.dock_icon(AppId::Terminal).unwrap();
    assert_eq!(terminal, Rect::new(560, 738, 48, 48));
    assert_eq!(settings.x, terminal.x + 2 * 56);
    assert_eq!(
        click_in(&mut desktop, settings),
        Action::Start(AppId::Settings)
    );
    let opened = window(&mut desktop, "Settings");
    assert_eq!(desktop.focused_window(), Some(opened));
    {
        let target = desktop.dock_icon(AppId::Terminal).unwrap();
        click_in(&mut desktop, target)
    };
    assert!(desktop.focused());
    assert_eq!(
        {
            let target = desktop.dock_icon(AppId::Settings).unwrap();
            click_in(&mut desktop, target)
        },
        Action::None,
        "a running app's icon focuses its window"
    );
    assert_eq!(desktop.focused_window(), Some(opened));
}

#[test]
fn super_alone_or_cmd_space_opens_the_launcher_and_typing_filters_it() {
    let mut desktop = desktop(Profile::Macos);
    super_alone(&mut desktop);
    assert!(desktop.launcher_open() && !desktop.keys_go_to_terminal());
    assert_eq!(
        names(&desktop),
        ["Files", "Settings", "Terminal", "Hello", "Paint"]
    );
    type_text(&mut desktop, "pref");
    assert_eq!(desktop.launcher_query(), Some("pref"));
    assert_eq!(names(&desktop), ["Settings"]);
    tap(&mut desktop, key::BACKSPACE, Modifiers::from_bits(0));
    tap(&mut desktop, key::BACKSPACE, Modifiers::from_bits(0));
    tap(&mut desktop, key::BACKSPACE, Modifiers::from_bits(0));
    assert_eq!(names(&desktop), ["Files", "Settings", "Paint"]);
    assert_eq!(
        tap(&mut desktop, key::ENTER, Modifiers::from_bits(0)),
        Action::None
    );
    assert_eq!(desktop.notice_text(), Some(FILES_MISSING));
    assert!(!desktop.launcher_open());
    tap(&mut desktop, key::SPACE, Modifiers::SUPER);
    assert!(desktop.launcher_open());
    type_text(&mut desktop, "greet");
    let hello = desktop.catalogue().installed_named("Hello").unwrap();
    assert_eq!(
        tap(&mut desktop, key::ENTER, Modifiers::from_bits(0)),
        Action::Type(hello)
    );
    assert!(desktop.focused(), "a console app runs in the terminal");
    super_alone(&mut desktop);
    super_alone(&mut desktop);
    assert!(!desktop.launcher_open(), "Super again closes it");
}

#[test]
fn super_with_another_key_does_not_open_the_launcher() {
    let mut desktop = desktop(Profile::Windows);
    key(&mut desktop, key::LEFT_SUPER, true, Modifiers::SUPER);
    tap(&mut desktop, key::UP, Modifiers::SUPER);
    key(
        &mut desktop,
        key::LEFT_SUPER,
        false,
        Modifiers::from_bits(0),
    );
    assert!(!desktop.launcher_open());
    tap(&mut desktop, key::SPACE, Modifiers::SUPER);
    assert!(!desktop.launcher_open(), "Cmd+Space is the macOS profile's");
}

#[test]
fn the_launcher_keys_move_the_selection_and_escape_closes_it() {
    let mut desktop = desktop(Profile::Linux);
    super_alone(&mut desktop);
    assert_eq!(desktop.launcher_entries().unwrap().1, Some(0));
    tap(&mut desktop, key::DOWN, Modifiers::from_bits(0));
    tap(&mut desktop, key::DOWN, Modifiers::from_bits(0));
    tap(&mut desktop, key::UP, Modifiers::from_bits(0));
    assert_eq!(desktop.launcher_entries().unwrap().1, Some(1));
    type_text(&mut desktop, "zz");
    assert_eq!(desktop.launcher_entries().unwrap(), (&[][..], None));
    assert_eq!(
        tap(&mut desktop, key::ENTER, Modifiers::from_bits(0)),
        Action::None
    );
    tap(&mut desktop, key::ESC, Modifiers::from_bits(0));
    assert!(!desktop.launcher_open() && desktop.keys_go_to_terminal());
}

#[test]
fn a_click_on_a_launcher_row_launches_it_and_outside_closes_it() {
    let mut desktop = desktop(Profile::Macos);
    super_alone(&mut desktop);
    let row = desktop.launcher_row_of(AppId::Settings).unwrap();
    assert_eq!(click_in(&mut desktop, row), Action::Start(AppId::Settings));
    super_alone(&mut desktop);
    press(&mut desktop, 5, 400);
    assert!(!desktop.launcher_open());
}

#[test]
fn the_windows_profile_has_a_taskbar_whose_start_opens_the_launcher_above_it() {
    let mut desktop = desktop(Profile::Windows);
    let start = desktop.start_button().unwrap();
    assert_eq!(start.y, 760);
    assert_eq!(start.width, 40);
    click_in(&mut desktop, start);
    assert!(desktop.launcher_open());
    let launcher = desktop.launcher_area().unwrap();
    assert_eq!(launcher.bottom(), 760 - 8);
    click_in(&mut desktop, start);
    assert!(!desktop.launcher_open(), "Start again closes it");
    assert_eq!(desktop.work_area(), Rect::new(0, 0, 1280, 760));
    assert_eq!(desktop.power_button(), Rect::new(1232, 760, 40, 40));
    assert_eq!(desktop.clock_rect(), Rect::new(1144, 772, 80, 16));
    assert!(desktop.menu().bottom() <= 756);
}

#[test]
fn a_taskbar_item_focuses_its_window_minimises_it_when_focused_and_restores_it() {
    let mut desktop = desktop(Profile::Windows);
    let settings = window(&mut desktop, "Settings");
    let terminal = desktop.terminal_window();
    let item = desktop.window_entry(terminal).unwrap();
    click_in(&mut desktop, item);
    assert_eq!(desktop.focused_window(), Some(terminal));
    let item = desktop.window_entry(settings).unwrap();
    click_in(&mut desktop, item);
    assert_eq!(desktop.focused_window(), Some(settings));
    click_in(&mut desktop, item);
    assert!(desktop.is_minimised(settings));
    {
        let target = desktop.window_entry(settings).unwrap();
        click_in(&mut desktop, target)
    };
    assert!(!desktop.is_minimised(settings));
    assert_eq!(desktop.focused_window(), Some(settings));
}

#[test]
fn a_minimised_window_restores_from_its_dock_icon_and_its_chip() {
    let mut desktop = desktop(Profile::Macos);
    let other = window(&mut desktop, "Test");
    let mut damage = Damage::new();
    desktop
        .manage(u32::from(other), 1, 0, 0, &mut damage, &mut DiscardEvents)
        .unwrap();
    let icon = desktop.window_entry(other).unwrap();
    assert!(icon.x > desktop.dock_icon(AppId::Settings).unwrap().right() + 16);
    click_in(&mut desktop, icon);
    assert!(!desktop.is_minimised(other));
    desktop.set_profile(Profile::Linux, &mut damage, &mut DiscardEvents);
    desktop
        .manage(u32::from(other), 1, 0, 0, &mut damage, &mut DiscardEvents)
        .unwrap();
    let chip = desktop.window_entry(other).unwrap();
    assert_eq!(chip.y, 4);
    click_in(&mut desktop, chip);
    assert!(!desktop.is_minimised(other));
}

#[test]
fn the_linux_profile_s_activities_item_opens_the_launcher_with_the_dash() {
    let mut desktop = desktop(Profile::Linux);
    {
        let target = desktop.system_item().unwrap();
        click_in(&mut desktop, target)
    };
    assert!(desktop.launcher_open());
    let launcher = desktop.launcher_area().unwrap();
    let dash_settings = Rect::new(launcher.x + 8, launcher.y + 12 + 2 * 56, 48, 48);
    assert_eq!(
        click_in(&mut desktop, dash_settings),
        Action::Start(AppId::Settings)
    );
}

#[test]
fn desktop_icons_select_on_a_click_and_launch_on_a_double_click_or_enter() {
    let mut desktop = desktop(Profile::Windows);
    let hello = desktop.catalogue().installed_named("Hello").unwrap();
    let cell = desktop.desktop_icon(hello).unwrap();
    assert_eq!(cell, Rect::new(16, 16 + 3 * 96, 96, 96));
    assert_eq!(click_in(&mut desktop, cell), Action::None);
    assert_eq!(desktop.selected_desktop_icon(), Some(hello));
    assert!(desktop.desktop_focused() && !desktop.keys_go_to_terminal());
    assert_eq!(click_in(&mut desktop, cell), Action::Type(hello));
    assert!(desktop.focused());
    click_in(&mut desktop, cell);
    tap(&mut desktop, key::UP, Modifiers::from_bits(0));
    assert_eq!(desktop.selected_desktop_icon(), Some(AppId::Settings));
    assert_eq!(
        tap(&mut desktop, key::ENTER, Modifiers::from_bits(0)),
        Action::Start(AppId::Settings)
    );
}

#[test]
fn the_desktop_icons_start_on_the_right_in_the_macos_profile() {
    let desktop = desktop(Profile::Macos);
    let terminal = desktop.desktop_icon(AppId::Terminal).unwrap();
    assert_eq!(terminal, Rect::new(1280 - 16 - 96, 28 + 16, 96, 96));
    assert_eq!(desktop.desktop_icon(AppId::HostShare), None);
}

#[test]
fn a_right_click_on_an_icon_opens_its_menu_whose_open_launches_it() {
    let mut desktop = desktop(Profile::Linux);
    let paint = desktop.catalogue().installed_named("Paint").unwrap();
    let cell = desktop.desktop_icon(paint).unwrap();
    let mut damage = Damage::new();
    desktop.pointer_moved(cell.x + 40, cell.y + 40, &mut damage, &mut DiscardEvents);
    desktop.button(button::RIGHT, true, &mut damage, &mut DiscardEvents);
    desktop.button(button::RIGHT, false, &mut damage, &mut DiscardEvents);
    let menu = desktop.icon_menu_area().unwrap();
    assert_eq!(menu.height, 2 * 32 + 2, "Open and Uninstall...");
    assert_eq!(
        press(&mut desktop, menu.x + 20, menu.y + 40),
        Action::None,
        "Uninstall... waits for item 36"
    );
    assert!(desktop.icon_menu_area().is_some());
    assert_eq!(
        press(&mut desktop, menu.x + 20, menu.y + 10),
        Action::Type(paint)
    );
    assert!(desktop.icon_menu_area().is_none());
}

#[test]
fn files_says_it_is_not_in_the_build_until_it_is() {
    let mut desktop = desktop(Profile::Macos);
    let files = desktop.dock_icon(AppId::Files).unwrap();
    assert_eq!(click_in(&mut desktop, files), Action::None);
    assert_eq!(desktop.notice_text(), Some(FILES_MISSING));
    tap(&mut desktop, key::ESC, Modifiers::from_bits(0));
    assert_eq!(desktop.notice_text(), None);
    let mut catalogue = catalogue();
    catalogue.set_files_present(true);
    desktop.set_catalogue(catalogue, &mut Damage::new());
    assert_eq!(click_in(&mut desktop, files), Action::Start(AppId::Files));
}

#[test]
fn a_notice_closes_after_three_seconds() {
    let mut desktop = desktop(Profile::Linux);
    let mut damage = Damage::new();
    desktop.set_time(1000);
    desktop.show_notice(LAUNCH_FAILED, &mut damage);
    desktop.tick(1000 + NOTICE_MS - 1, &mut damage, &mut DiscardEvents);
    assert!(desktop.notice_area().is_some());
    desktop.tick(1000 + NOTICE_MS, &mut damage, &mut DiscardEvents);
    assert!(desktop.notice_area().is_none());
}

#[test]
fn a_tooltip_shows_after_the_pointer_rests_on_a_dock_icon() {
    let mut desktop = desktop(Profile::Macos);
    let icon = desktop.dock_icon(AppId::Settings).unwrap();
    let mut damage = Damage::new();
    desktop.set_time(0);
    desktop.pointer_moved(icon.x + 10, icon.y + 10, &mut damage, &mut DiscardEvents);
    desktop.tick(TOOLTIP_DELAY_MS - 1, &mut damage, &mut DiscardEvents);
    assert_eq!(desktop.tooltip_rect(), None);
    desktop.tick(TOOLTIP_DELAY_MS, &mut damage, &mut DiscardEvents);
    let tooltip = desktop.tooltip_rect().unwrap();
    assert_eq!(tooltip.width, 8 * 8 + 16);
    assert_eq!(tooltip.bottom(), icon.y - 2 - 4);
    desktop.pointer_moved(5, 400, &mut damage, &mut DiscardEvents);
    assert_eq!(desktop.tooltip_rect(), None);
}

#[test]
fn switching_profiles_lays_the_shell_out_again_and_keeps_windows_in_the_work_area() {
    let mut desktop = desktop(Profile::Linux);
    let test = window(&mut desktop, "Test");
    let mut damage = Damage::new();
    desktop
        .manage(u32::from(test), 2, 0, 0, &mut damage, &mut DiscardEvents)
        .unwrap();
    desktop
        .manage(
            desktop.terminal_window().into(),
            5,
            400,
            5000,
            &mut damage,
            &mut DiscardEvents,
        )
        .unwrap();
    for profile in [Profile::Windows, Profile::Macos, Profile::Linux] {
        desktop.set_profile(profile, &mut damage, &mut DiscardEvents);
        let work = desktop.work_area();
        assert_eq!(desktop.window_rect(test), Some(work), "{profile:?}");
        let terminal = desktop.window();
        assert!(terminal.bottom() <= work.bottom() && terminal.y >= work.y);
        for panel in desktop.layout().panels() {
            assert!(!panel.overlaps(&terminal), "{profile:?}");
        }
    }
}

#[test]
fn the_switcher_and_the_taskbar_show_each_window_s_app_icon() {
    let mut desktop = desktop(Profile::Windows);
    let settings = window(&mut desktop, "Settings");
    let unnamed = window(&mut desktop, "win-paint.exe");
    let slot = |id: WindowId| slot_of(id);
    assert_eq!(desktop.window_icon(slot(settings)), Icon::Settings);
    assert_eq!(desktop.window_icon(TERMINAL_SLOT), Icon::Terminal);
    assert_eq!(
        desktop.window_icon(slot(unnamed)),
        Icon::App {
            letter: b'W',
            badge: None
        }
    );
    let paint = window(&mut desktop, "Paint");
    assert_eq!(
        desktop.window_icon(slot(paint)),
        Icon::App {
            letter: b'P',
            badge: Some(Source::Windows)
        }
    );
}

#[test]
fn the_shell_s_pictures_are_pinned_in_each_profile() {
    let mut found = Vec::new();
    for profile in Profile::ALL {
        let mut desktop = desktop(profile);
        window(&mut desktop, "Settings");
        let test = window(&mut desktop, "Test");
        desktop
            .manage(
                u32::from(test),
                1,
                0,
                0,
                &mut Damage::new(),
                &mut DiscardEvents,
            )
            .unwrap();
        let shell = fnv(&painted(&desktop));
        super_alone(&mut desktop);
        let launcher = fnv(&painted(&desktop));
        found.push((profile, shell, launcher));
    }
    assert_eq!(found, PINNED, "{found:#x?}");
}

const PINNED: [(Profile, u64, u64); 3] = [
    (
        Profile::Windows,
        0x8cdb_8825_a916_bd68,
        0xa1fd_ee99_e308_058e,
    ),
    (Profile::Macos, 0x8f54_8421_82e4_30b4, 0x2c88_cb3e_5222_e143),
    (Profile::Linux, 0x7179_d12c_39d6_9bbf, 0x8156_c303_de78_fbb1),
];

#[test]
fn drawing_rects_equals_drawing_the_whole_shell() {
    for profile in Profile::ALL {
        let mut desktop = desktop(profile);
        window(&mut desktop, "Settings");
        super_alone(&mut desktop);
        let whole = painted(&desktop);
        let mut parts = vec![0; 1280 * 800];
        for y in (0..800).step_by(80) {
            for x in (0..1280).step_by(160) {
                desktop.draw(
                    &mut parts,
                    1280,
                    Rect::new(x, y, 160, 80),
                    &super::tests::BlankTerminal,
                );
            }
        }
        assert!(whole == parts, "{profile:?}");
    }
}
