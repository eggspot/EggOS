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
    desktop.set_catalogue(catalogue(), &mut damage, &mut DiscardEvents);
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
        press(&mut desktop, menu.x + 20, menu.y + 10),
        Action::Type(paint)
    );
    assert!(desktop.icon_menu_area().is_none());
}

#[test]
fn uninstall_in_an_installed_app_s_menu_asks_the_kernel_for_the_installer() {
    let mut desktop = desktop(Profile::Linux);
    let paint = desktop.catalogue().installed_named("Paint").unwrap();
    let cell = desktop.desktop_icon(paint).unwrap();
    let mut damage = Damage::new();
    desktop.pointer_moved(cell.x + 40, cell.y + 40, &mut damage, &mut DiscardEvents);
    desktop.button(button::RIGHT, true, &mut damage, &mut DiscardEvents);
    desktop.button(button::RIGHT, false, &mut damage, &mut DiscardEvents);
    let menu = desktop.icon_menu_area().unwrap();
    assert_eq!(
        press(&mut desktop, menu.x + 20, menu.y + 40),
        Action::Uninstall(paint)
    );
    assert!(desktop.icon_menu_area().is_none());
    let command = desktop.uninstall_command(paint).unwrap();
    assert_eq!(
        command.as_bytes(),
        b"/usr/libexec/app-window\0uninstall\0Paint"
    );

    desktop.button(button::RIGHT, true, &mut damage, &mut DiscardEvents);
    desktop.button(button::RIGHT, false, &mut damage, &mut DiscardEvents);
    assert!(desktop.icon_menu_area().is_some());
    tap(&mut desktop, key::DOWN, Modifiers::from_bits(0));
    tap(&mut desktop, key::DOWN, Modifiers::from_bits(0));
    tap(&mut desktop, key::UP, Modifiers::from_bits(0));
    assert_eq!(
        tap(&mut desktop, key::ENTER, Modifiers::from_bits(0)),
        Action::Type(paint)
    );
}

#[test]
fn an_icon_menu_whose_app_was_uninstalled_closes() {
    let mut desktop = desktop(Profile::Linux);
    let paint = desktop.catalogue().installed_named("Paint").unwrap();
    let cell = desktop.desktop_icon(paint).unwrap();
    let mut damage = Damage::new();
    desktop.pointer_moved(cell.x + 40, cell.y + 40, &mut damage, &mut DiscardEvents);
    desktop.button(button::RIGHT, true, &mut damage, &mut DiscardEvents);
    desktop.button(button::RIGHT, false, &mut damage, &mut DiscardEvents);
    assert!(desktop.icon_menu_area().is_some());
    let mut without_paint = Catalogue::new();
    assert!(without_paint.add_installed(b"Hello", HELLO));
    desktop.set_catalogue(without_paint, &mut damage, &mut DiscardEvents);
    assert!(desktop.icon_menu_area().is_none());
    assert!(!desktop.popup_open());
}

#[test]
fn a_failed_start_clears_the_starting_guard_with_its_notice() {
    let mut desktop = desktop(Profile::Macos);
    let settings = desktop.dock_icon(AppId::Settings).unwrap();
    assert_eq!(
        click_in(&mut desktop, settings),
        Action::Start(AppId::Settings)
    );
    assert_eq!(click_in(&mut desktop, settings), Action::None);
    desktop.launch_failed(&mut Damage::new());
    assert_eq!(desktop.notice_text(), Some(LAUNCH_FAILED));
    tap(&mut desktop, key::ESC, Modifiers::from_bits(0));
    assert_eq!(
        click_in(&mut desktop, settings),
        Action::Start(AppId::Settings)
    );
}

#[test]
fn the_launcher_offers_install_from_host_share_while_the_share_is_mounted() {
    let mut desktop = desktop(Profile::Macos);
    let mut catalogue = catalogue();
    catalogue.set_host_share(true);
    desktop.set_catalogue(catalogue, &mut Damage::new(), &mut DiscardEvents);
    assert!(
        desktop
            .catalogue()
            .launcher_apps()
            .any(|app| app == AppId::InstallFromHostShare)
    );
    assert_eq!(
        desktop.launch(
            AppId::InstallFromHostShare,
            &mut Damage::new(),
            &mut DiscardEvents
        ),
        Action::Start(AppId::InstallFromHostShare)
    );
    assert_eq!(
        desktop.launch(
            AppId::InstallFromHostShare,
            &mut Damage::new(),
            &mut DiscardEvents
        ),
        Action::None
    );
}

#[test]
fn files_says_it_is_not_installed_until_it_is() {
    let mut desktop = desktop(Profile::Macos);
    let files = desktop.dock_icon(AppId::Files).unwrap();
    assert_eq!(click_in(&mut desktop, files), Action::None);
    assert_eq!(desktop.notice_text(), Some(FILES_MISSING));
    tap(&mut desktop, key::ESC, Modifiers::from_bits(0));
    assert_eq!(desktop.notice_text(), None);
    let mut catalogue = catalogue();
    catalogue.set_files_present(true);
    desktop.set_catalogue(catalogue, &mut Damage::new(), &mut DiscardEvents);
    assert_eq!(click_in(&mut desktop, files), Action::Start(AppId::Files));
}

#[test]
fn the_host_share_opens_files_on_it_and_without_files_lists_it_in_the_terminal() {
    let mut desktop = desktop(Profile::Macos);
    let mut catalogue = catalogue();
    catalogue.set_host_share(true);
    desktop.set_catalogue(catalogue, &mut Damage::new(), &mut DiscardEvents);
    assert_eq!(
        desktop.launch(AppId::HostShare, &mut Damage::new(), &mut DiscardEvents),
        Action::Type(AppId::HostShare)
    );
    let mut catalogue = self::catalogue();
    catalogue.set_host_share(true);
    catalogue.set_files_present(true);
    desktop.set_catalogue(catalogue, &mut Damage::new(), &mut DiscardEvents);
    assert_eq!(
        desktop.launch(AppId::HostShare, &mut Damage::new(), &mut DiscardEvents),
        Action::Start(AppId::HostShare)
    );
    assert_eq!(
        desktop.launch(AppId::HostShare, &mut Damage::new(), &mut DiscardEvents),
        Action::None
    );
    match desktop.launch_command(AppId::HostShare) {
        Some(Launch::Start(command)) => {
            assert_eq!(command.as_bytes(), b"/bin/files\0/media/host")
        }
        other => panic!("{other:?}"),
    }
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

struct Screen {
    desktop: Desktop,
    pixels: Vec<u32>,
}

impl Screen {
    fn new(profile: Profile) -> Self {
        let desktop = desktop(profile);
        let pixels = painted(&desktop);
        Self { desktop, pixels }
    }

    fn redraw(&mut self, damage: &Damage, at: &str) {
        for &rect in damage.rects() {
            self.desktop
                .draw(&mut self.pixels, 1280, rect, &super::tests::BlankTerminal);
        }
        assert!(
            self.pixels == painted(&self.desktop),
            "stale pixels after {at}"
        );
    }

    fn step(&mut self, at: &str, change: impl FnOnce(&mut Desktop, &mut Damage)) {
        let mut damage = Damage::new();
        change(&mut self.desktop, &mut damage);
        self.redraw(&damage, at);
    }

    fn point(&mut self, at: &str, x: u32, y: u32, button: Option<bool>) {
        self.step(at, |desktop, damage| {
            desktop.pointer_moved(x, y, damage, &mut DiscardEvents);
            if let Some(pressed) = button {
                desktop.left_button(pressed, damage, &mut DiscardEvents);
            }
        });
    }

    fn click(&mut self, at: &str, rect: Rect) {
        let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        self.point(at, x, y, None);
        self.point(at, x, y, Some(true));
        self.point(at, x, y, Some(false));
    }

    fn key(&mut self, at: &str, code: u16, modifiers: Modifiers) {
        self.step(at, |desktop, damage| {
            for pressed in [true, false] {
                desktop.key(code, pressed, modifiers, damage, &mut DiscardEvents);
            }
        });
    }
}

#[test]
fn what_the_shell_damages_is_all_that_changes_on_screen() {
    let mut screen = Screen::new(Profile::Macos);
    let settings = screen.desktop.dock_icon(AppId::Settings).unwrap();
    screen.click("the dock's Settings", settings);
    screen.step("Settings shown", |desktop, damage| {
        let created = desktop.create(CLIENT, 0, 0, 400, 300, 0).unwrap();
        let id = u32::from(created.window);
        desktop.set_title(CLIENT, id, b"Settings", damage).unwrap();
        desktop
            .commit(CLIENT, id, &[], damage, &mut DiscardEvents)
            .unwrap();
    });
    screen.point("the pointer parked", 1278, 798, None);
    screen.step("the tooltip's time", |desktop, damage| {
        desktop.tick(0, damage, &mut DiscardEvents);
    });
    let files = screen.desktop.dock_icon(AppId::Files).unwrap();
    screen.point("the pointer on Files", files.x + 4, files.y + 4, None);
    screen.step("Files' tooltip", |desktop, damage| {
        desktop.tick(TOOLTIP_DELAY_MS + 1, damage, &mut DiscardEvents);
    });
    assert!(screen.desktop.tooltip_rect().is_some());
    screen.click("the dock's Files", files);
    screen.key("Cmd+Space", key::SPACE, Modifiers::SUPER);
    screen.key("a letter", 25, Modifiers::from_bits(0));
    screen.step("the catalogue read again", |desktop, damage| {
        desktop.set_catalogue(catalogue(), damage, &mut DiscardEvents);
    });
    screen.key("Down", key::DOWN, Modifiers::from_bits(0));
    screen.key("Esc", key::ESC, Modifiers::from_bits(0));
    screen.step("the Windows profile", |desktop, damage| {
        desktop.set_profile(Profile::Windows, damage, &mut DiscardEvents);
    });
    let start = screen.desktop.start_button().unwrap();
    screen.click("Start", start);
    screen.click("Start again", start);
    let terminal = screen.desktop.terminal_window();
    let item = screen.desktop.window_entry(terminal).unwrap();
    screen.click("the terminal's taskbar item", item);
    screen.click("the terminal's taskbar item again", item);
    let item = screen.desktop.window_entry(terminal).unwrap();
    screen.click("the minimised terminal's item", item);
    let hello = screen.desktop.catalogue().installed_named("Hello").unwrap();
    let cell = screen.desktop.desktop_icon(hello).unwrap();
    screen.click("a desktop icon", cell);
    screen.key("Up on the desktop", key::UP, Modifiers::from_bits(0));
    screen.step("the Linux profile", |desktop, damage| {
        desktop.set_profile(Profile::Linux, damage, &mut DiscardEvents);
    });
    let activities = screen.desktop.system_item().unwrap();
    screen.click("Activities", activities);
    screen.click("Activities again", activities);
    screen.point("the pointer on the wallpaper", 600, 700, None);
}

#[test]
fn a_window_destroyed_under_its_tooltip_leaves_no_stale_item() {
    let mut screen = Screen::new(Profile::Windows);
    let test = window(&mut screen.desktop, "Test");
    screen.pixels = painted(&screen.desktop);
    let item = screen.desktop.window_entry(test).unwrap();
    screen.point("the pointer on Test's item", item.x + 4, item.y + 4, None);
    screen.step("Test's tooltip", |desktop, damage| {
        desktop.tick(TOOLTIP_DELAY_MS, damage, &mut DiscardEvents);
    });
    assert!(screen.desktop.tooltip_rect().is_some());
    screen.step("Test destroyed", |desktop, damage| {
        desktop
            .destroy(CLIENT, u32::from(test), damage, &mut DiscardEvents)
            .unwrap();
    });
    assert_eq!(screen.desktop.tooltip_rect(), None);
    screen.step("the clock ticks on", |desktop, damage| {
        desktop.tick(10 * TOOLTIP_DELAY_MS, damage, &mut DiscardEvents);
    });
}

#[derive(Default)]
struct Focus(Vec<u16>);

impl protocol::EventSink for Focus {
    fn send(&mut self, _client: u16, record: protocol::EventRecord) {
        if matches!(
            record.kind,
            protocol::event::FOCUS_IN | protocol::event::FOCUS_OUT
        ) {
            self.0.push(record.kind);
        }
    }
}

#[test]
fn a_window_shown_while_the_launcher_is_open_gets_one_focus_in() {
    let mut desktop = desktop(Profile::Macos);
    super_alone(&mut desktop);
    let mut focus = Focus::default();
    let created = desktop.create(CLIENT, 0, 0, 400, 300, 0).unwrap();
    let mut damage = Damage::new();
    desktop
        .commit(
            CLIENT,
            u32::from(created.window),
            &[],
            &mut damage,
            &mut focus,
        )
        .unwrap();
    desktop.key(
        key::ESC,
        true,
        Modifiers::from_bits(0),
        &mut damage,
        &mut focus,
    );
    assert_eq!(focus.0, [protocol::event::FOCUS_IN]);
    assert_eq!(desktop.focused_window(), Some(created.window));
}

#[test]
fn a_window_too_large_for_the_new_work_area_shrinks_into_it() {
    let mut desktop = desktop(Profile::Linux);
    let test = window(&mut desktop, "Test");
    let mut damage = Damage::new();
    desktop
        .manage(u32::from(test), 5, 0, 28, &mut damage, &mut DiscardEvents)
        .unwrap();
    desktop
        .manage(
            u32::from(test),
            6,
            1200,
            740,
            &mut damage,
            &mut DiscardEvents,
        )
        .unwrap();
    assert_eq!(desktop.window_rect(test).unwrap().height, 770);
    desktop.set_profile(Profile::Macos, &mut damage, &mut DiscardEvents);
    let rect = desktop.window_rect(test).unwrap();
    let work = desktop.work_area();
    assert!(rect.height <= work.height && rect.y >= work.y && rect.bottom() <= work.bottom());
}

#[test]
fn a_second_click_while_an_app_starts_does_not_start_it_twice() {
    let mut desktop = desktop(Profile::Macos);
    let settings = desktop.dock_icon(AppId::Settings).unwrap();
    desktop.set_time(0);
    assert_eq!(
        click_in(&mut desktop, settings),
        Action::Start(AppId::Settings)
    );
    desktop.set_time(300);
    assert_eq!(click_in(&mut desktop, settings), Action::None);
    desktop.set_time(2300);
    assert_eq!(
        click_in(&mut desktop, settings),
        Action::Start(AppId::Settings)
    );
    let opened = window(&mut desktop, "Settings");
    let mut damage = Damage::new();
    desktop
        .destroy(CLIENT, u32::from(opened), &mut damage, &mut DiscardEvents)
        .unwrap();
    desktop.set_time(2400);
    assert_eq!(
        click_in(&mut desktop, settings),
        Action::Start(AppId::Settings)
    );
}
