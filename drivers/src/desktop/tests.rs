use super::*;
use alloc::vec;

fn painted(desktop: &Desktop) -> Vec<u32> {
    let mut pixels = vec![0; desktop.width as usize * desktop.height as usize];
    desktop.draw(&mut pixels, desktop.width as usize, desktop.screen());
    pixels
}

fn at(pixels: &[u32], x: u32, y: u32) -> u32 {
    pixels[y as usize * 1280 + x as usize]
}

fn fnv(pixels: &[u32]) -> u64 {
    pixels
        .iter()
        .flat_map(|pixel| pixel.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

fn click(desktop: &mut Desktop, x: u32, y: u32) -> Action {
    let mut damage = Damage::new();
    desktop.pointer_moved(x, y, &mut damage);
    let action = desktop.left_button(true, &mut damage);
    desktop.left_button(false, &mut damage);
    action
}

#[test]
fn the_window_starts_centred_in_the_work_area_as_the_design_places_it() {
    let desktop = Desktop::new(1280, 800).unwrap();
    assert_eq!(desktop.window(), Rect::new(311, 199, 658, 430));
    assert_eq!(
        desktop.text_area(),
        TextArea {
            x: 320,
            y: 236,
            columns: 80,
            rows: 24
        }
    );
    assert_eq!(
        Desktop::new(1024, 768).unwrap().window(),
        Rect::new(183, 183, 658, 430)
    );
    assert!(Desktop::new(640, 400).is_none());
}

#[test]
fn the_wallpaper_bar_and_window_show_their_tokens() {
    let desktop = Desktop::new(1280, 800).unwrap();
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 5, 28), gradient_pixel(28, 800));
    assert_eq!(at(&pixels, 0, 799), colour::WALLPAPER_BOTTOM);
    assert_eq!(at(&pixels, 1050, 300), colour::WALLPAPER_EGG);
    assert_eq!(at(&pixels, 1050, 696), colour::ACCENT);
    assert_eq!(at(&pixels, 100, 10), colour::BAR);
    assert_eq!(at(&pixels, 100, 27), colour::BAR_EDGE);
    assert_eq!(at(&pixels, 17, 14), colour::ACCENT);
    assert_eq!(at(&pixels, 311 + 4, 199), colour::BORDER_ACTIVE);
    assert_eq!(at(&pixels, 311 + 3, 199), gradient_pixel(199, 800));
    assert_eq!(at(&pixels, 400, 210), colour::TITLE_ACTIVE);
    assert_eq!(at(&pixels, 311 + 12 + 6, 199 + 14), colour::BUTTON_INACTIVE);
    assert_eq!(at(&pixels, 311 + 32 + 6, 199 + 14), colour::BUTTON_MINIMISE);
    assert_eq!(at(&pixels, 330, 240), TERM_BACKGROUND);
    let below = at(&pixels, 500, 629);
    assert_eq!(below, half(gradient_pixel(629, 800)), "{below:#x}");
    assert!(desktop.clock_text().contains("up 0:00"));
}

fn half(pixel: u32) -> u32 {
    (pixel >> 1) & 0x007f_7f7f
}

#[test]
fn the_default_desktop_s_hash_is_pinned() {
    let pixels = painted(&Desktop::new(1280, 800).unwrap());
    assert_eq!(fnv(&pixels), DEFAULT_DESKTOP_HASH, "{:#x}", fnv(&pixels));
}

const DEFAULT_DESKTOP_HASH: u64 = 0xdb10_a868_0234_3949;

#[test]
fn drawing_a_rect_is_the_same_as_drawing_the_whole_screen() {
    let desktop = Desktop::new(1280, 800).unwrap();
    let whole = painted(&desktop);
    let mut parts = vec![0; 1280 * 800];
    for y in (0..800).step_by(100) {
        for x in (0..1280).step_by(128) {
            desktop.draw(&mut parts, 1280, Rect::new(x, y, 128, 100));
        }
    }
    assert!(whole == parts);
}

#[test]
fn dragging_the_title_bar_moves_the_window_within_the_screen() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut damage = Damage::new();
    desktop.pointer_moved(600, 210, &mut damage);
    desktop.left_button(true, &mut damage);
    desktop.pointer_moved(700, 260, &mut damage);
    assert_eq!(desktop.window(), Rect::new(411, 249, 658, 430));
    assert!(!damage.is_empty());
    desktop.pointer_moved(5000, 0, &mut damage);
    assert_eq!(desktop.window(), Rect::new(1280 - 658, 28, 658, 430));
    desktop.left_button(false, &mut damage);
    desktop.pointer_moved(0, 600, &mut damage);
    assert_eq!(desktop.window().x, 1280 - 658);
}

#[test]
fn a_click_outside_the_window_takes_focus_and_inside_gives_it_back() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    click(&mut desktop, 50, 400);
    assert!(!desktop.focused() && !desktop.keys_go_to_terminal());
    click(&mut desktop, 400, 400);
    assert!(desktop.focused() && desktop.keys_go_to_terminal());
}

#[test]
fn minimise_hides_the_window_behind_a_chip_that_restores_it() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let control = desktop.minimise_control();
    click(&mut desktop, control.x + 5, control.y + 10);
    assert!(desktop.minimised() && !desktop.terminal_shown());
    assert!(!desktop.keys_go_to_terminal());
    let pixels = painted(&desktop);
    assert_eq!(at(&pixels, 400, 400), gradient_pixel(400, 800));
    assert_eq!(at(&pixels, 105, 14), colour::BAR_HOVER);
    let chip = desktop.chip();
    click(&mut desktop, chip.x + 10, chip.y + 10);
    assert!(!desktop.minimised() && desktop.focused() && desktop.terminal_shown());
    assert_eq!(desktop.window(), Rect::new(311, 199, 658, 430));
}

#[test]
fn the_close_control_is_disabled() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut damage = Damage::new();
    desktop.pointer_moved(311 + 15, 199 + 10, &mut damage);
    desktop.left_button(true, &mut damage);
    desktop.pointer_moved(311 + 100, 199 + 100, &mut damage);
    assert_eq!(desktop.window(), Rect::new(311, 199, 658, 430));
    assert!(!desktop.minimised());
}

#[test]
fn the_power_menu_powers_off_or_restarts_by_pointer_or_keys() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let button = desktop.power_button();
    assert_eq!(button, Rect::new(1244, 0, 28, 28));
    assert_eq!(click(&mut desktop, button.x + 4, 4), Action::None);
    assert!(desktop.menu_open() && !desktop.keys_go_to_terminal());
    let menu = desktop.menu();
    assert_eq!(menu, Rect::new(1112, 32, 160, 66));
    assert_eq!(
        click(&mut desktop, menu.x + 20, menu.y + 40),
        Action::Restart
    );
    assert!(!desktop.menu_open());
    click(&mut desktop, button.x + 4, 4);
    assert_eq!(
        click(&mut desktop, menu.x + 20, menu.y + 10),
        Action::PowerOff
    );
    click(&mut desktop, button.x + 4, 4);
    click(&mut desktop, button.x + 4, 4);
    assert!(!desktop.menu_open());
    click(&mut desktop, button.x + 4, 4);
    let mut damage = Damage::new();
    assert_eq!(desktop.key(key::DOWN, true, &mut damage), Action::None);
    assert_eq!(desktop.key(key::DOWN, true, &mut damage), Action::None);
    assert_eq!(desktop.key(key::ENTER, true, &mut damage), Action::Restart);
    click(&mut desktop, button.x + 4, 4);
    desktop.key(key::ESC, true, &mut damage);
    assert!(!desktop.menu_open() && desktop.keys_go_to_terminal());
}

#[test]
fn the_clock_shows_uptime_and_damages_only_its_rect_when_it_changes() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let mut damage = Damage::new();
    assert!(!desktop.set_uptime(0, &mut damage));
    assert!(damage.is_empty());
    assert!(desktop.set_uptime(61, &mut damage));
    assert_eq!(desktop.clock_text().trim(), "up 1:01");
    assert_eq!(damage.rects(), &[desktop.clock_rect()]);
    for (seconds, text) in [
        (3599, "up 59:59"),
        (3600, "up 1:00:00"),
        (35_999, "up 9:59:59"),
        (36_000, "up 10h 00m"),
        (359_999, "up 99h 59m"),
        (360_000, "up 4d 04h"),
        (100 * 86_400 - 1, "up 99d 23h"),
        (100 * 86_400, "up 100d"),
        (u64::MAX, "up 999999d"),
    ] {
        desktop.set_uptime(seconds, &mut damage);
        assert_eq!(desktop.clock_text().trim(), text);
    }
    assert_eq!(desktop.clock_rect(), Rect::new(600, 6, 80, 16));
}

#[test]
fn the_egg_is_taller_above_its_equator_and_square_roots_are_exact() {
    assert_eq!(isqrt(0), 0);
    assert_eq!(isqrt(65_536), 256);
    assert_eq!(isqrt(65_535), 255);
    assert_eq!(egg_half_width(800, 0), 0);
    assert_eq!(egg_half_width(800, 464), 288);
    assert!(egg_half_width(800, 300) > egg_half_width(800, 700));
    assert_eq!(egg_half_width(800, 800), 0);
}
