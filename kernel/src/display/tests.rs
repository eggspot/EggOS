use super::*;
use crate::tests::TestPlatform;
use alloc::vec;
use alloc::vec::Vec;
use std::sync::{Arc, Mutex};

type PointerCall = (&'static str, (u32, u32), (u32, u32));

#[derive(Default)]
struct Shown {
    flushed: Vec<Vec<Rect>>,
    failing: bool,
    pointer: Vec<PointerCall>,
}

struct FakeScanout {
    mode: Mode,
    pixels: Vec<u32>,
    shown: Arc<Mutex<Shown>>,
    pointer_image: Option<Vec<u32>>,
}

impl Scanout for FakeScanout {
    fn mode(&self) -> Mode {
        self.mode
    }
    fn pixels(&mut self) -> &mut [u32] {
        &mut self.pixels
    }
    fn flush(&mut self, rects: &[Rect], _: &mut Deadline) -> Result<usize, &'static str> {
        let mut shown = self.shown.lock().unwrap();
        if shown.failing {
            return Err("the fake device failed");
        }
        shown.flushed.push(rects.to_vec());
        Ok(rects.len())
    }
    fn draws_pointer(&self) -> bool {
        self.pointer_image.is_some()
    }
    fn pointer_image(&mut self) -> Option<&mut [u32]> {
        self.pointer_image.as_deref_mut()
    }
    fn show_pointer(
        &mut self,
        hotspot: (u32, u32),
        at: (u32, u32),
        _: &mut Deadline,
    ) -> Result<(), &'static str> {
        let mut shown = self.shown.lock().unwrap();
        shown.pointer.push(("show", hotspot, at));
        Ok(())
    }
    fn move_pointer(&mut self, at: (u32, u32), _: &mut Deadline) -> Result<(), &'static str> {
        self.shown
            .lock()
            .unwrap()
            .pointer
            .push(("move", (0, 0), at));
        Ok(())
    }
    fn hide_pointer(&mut self, _: &mut Deadline) -> Result<(), &'static str> {
        self.shown
            .lock()
            .unwrap()
            .pointer
            .push(("hide", (0, 0), (0, 0)));
        Ok(())
    }
}

fn display(width: u32, height: u32) -> (Display, Arc<Mutex<Shown>>) {
    let shown = Arc::new(Mutex::new(Shown::default()));
    let scanout = FakeScanout {
        mode: Mode::new(width, height),
        pixels: vec![0; (width * height) as usize],
        shown: Arc::clone(&shown),
        pointer_image: None,
    };
    (Display::new(Box::new(scanout)), shown)
}

fn deadline() -> Deadline {
    Deadline::after::<TestPlatform>(FLUSH_TIMEOUT_MS)
}

#[test]
fn a_fill_writes_only_its_clipped_rect_and_damages_it() {
    let (mut display, _) = display(8, 4);
    display.draw(|framebuffer| framebuffer.fill(Rect::new(6, 2, 5, 5), 0x00ff_0000));
    let pixels = display.draw(|framebuffer| framebuffer.pixels().to_vec());
    for (index, &pixel) in pixels.iter().enumerate() {
        let (x, y) = (index % 8, index / 8);
        let inside = x >= 6 && y >= 2;
        assert_eq!(pixel, if inside { 0x00ff_0000 } else { 0 }, "({x}, {y})");
    }
    assert_eq!(display.damage.rects(), &[Rect::new(6, 2, 2, 2)]);
}

#[test]
fn a_flush_shows_the_damage_once_and_nothing_twice() {
    let (mut display, shown) = display(16, 16);
    assert_eq!(display.flush(&mut deadline()), Ok(0));
    display.draw(|framebuffer| {
        framebuffer.damage(Rect::new(0, 0, 2, 2));
        framebuffer.damage(Rect::new(1, 1, 2, 2));
        framebuffer.damage(Rect::new(10, 10, 20, 20));
    });
    assert_eq!(display.flush(&mut deadline()), Ok(2));
    assert_eq!(
        shown.lock().unwrap().flushed,
        [vec![Rect::new(0, 0, 3, 3), Rect::new(10, 10, 6, 6)]]
    );
    assert_eq!(display.flush(&mut deadline()), Ok(0));
    assert_eq!(display.flushes(), 1);
}

#[test]
fn damage_a_failed_flush_did_not_show_stays_for_the_next() {
    let (mut display, shown) = display(16, 16);
    shown.lock().unwrap().failing = true;
    display.draw(|framebuffer| framebuffer.fill(Rect::new(0, 0, 4, 4), 1));
    assert!(display.flush(&mut deadline()).is_err());
    shown.lock().unwrap().failing = false;
    assert_eq!(display.flush(&mut deadline()), Ok(1));
    assert_eq!(shown.lock().unwrap().flushed, [vec![Rect::new(0, 0, 4, 4)]]);
    assert_eq!(display.flushes(), 1);
}

#[test]
fn the_first_frame_hash_is_the_one_the_qemu_tests_check() {
    let frame = vec![FIRST_FRAME_COLOUR; 1280 * 800];
    assert_eq!(pixel_hash(&frame), FIRST_FRAME_HASH_1280X800);
    assert_eq!(pixel_hash(&[]), 0xcbf2_9ce4_8422_2325);
    assert_eq!(pixel_hash(&[0x6463_6261]), 0xfc17_9f83_ee07_24dd);
}

#[test]
fn the_first_frame_is_the_boot_screen_whose_stages_light_until_the_desktop_ends_it() {
    let (mut display, shown) = display(1280, 800);
    display.draw(|framebuffer| framebuffer.show_boot_screen());
    let screen = BootScreen::new(1280, 800).unwrap();
    let mut expected = vec![0; 1280 * 800];
    screen.draw(&mut expected, 1280, Rect::new(0, 0, 1280, 800));
    let hash = display.draw(|framebuffer| pixel_hash(framebuffer.pixels()));
    assert_eq!(hash, pixel_hash(&expected));
    assert_eq!(display.boot_screen_hash(), Some(hash));
    assert_ne!(hash, FIRST_FRAME_HASH_1280X800);
    assert_eq!(display.flush(&mut deadline()), Ok(1));
    display.draw(|framebuffer| framebuffer.light_boot_stages(2));
    assert_eq!(display.damage.rects(), &[screen.segment(1)]);
    assert_eq!(display.flush(&mut deadline()), Ok(1));
    let lit = display.draw(|framebuffer| pixel_hash(framebuffer.pixels()));
    assert_eq!(display.boot_screen_hash(), Some(lit));
    assert_ne!(lit, hash);
    display.draw(|framebuffer| {
        assert!(framebuffer.boot_screen_shown());
        framebuffer.end_boot_screen();
        framebuffer.light_boot_stages(3);
        assert!(!framebuffer.boot_screen_shown());
    });
    assert!(display.damage.is_empty());
    assert_eq!(display.boot_screen_hash(), None);
    assert_eq!(shown.lock().unwrap().flushed.len(), 2);
}

#[test]
fn a_display_too_small_for_the_boot_screen_starts_in_the_background_colour() {
    let (mut display, _) = display(16, 4);
    display.draw(|framebuffer| framebuffer.show_boot_screen());
    let pixels = display.draw(|framebuffer| framebuffer.pixels().to_vec());
    assert!(pixels.iter().all(|&pixel| pixel == FIRST_FRAME_COLOUR));
    assert_eq!(display.boot_screen_hash(), None);
}

#[test]
fn a_hash_taken_a_row_at_a_time_is_the_whole_frame_s() {
    let frame: Vec<u32> = (0..64).map(|pixel| pixel * 0x0101_0101).collect();
    let rows = frame.chunks(8).fold(PIXEL_HASH_START, continue_pixel_hash);
    assert_eq!(rows, pixel_hash(&frame));
}

#[test]
fn a_display_that_draws_the_pointer_gets_an_image_per_shape_and_moves_otherwise() {
    let (mut display, shown) = display(64, 64);
    let mut deadline = deadline();
    assert_eq!(
        display.show_pointer(Some((1, 2, PointerShape::Arrow)), &mut deadline),
        Ok(())
    );
    assert!(shown.lock().unwrap().pointer.is_empty());
    let shown = Arc::new(Mutex::new(Shown::default()));
    let mut display = Display::new(Box::new(FakeScanout {
        mode: Mode::new(64, 64),
        pixels: vec![0; 64 * 64],
        shown: Arc::clone(&shown),
        pointer_image: Some(vec![0; 64 * 64]),
    }));
    assert!(display.draws_pointer());
    for wanted in [
        Some((10, 20, PointerShape::Arrow)),
        Some((10, 20, PointerShape::Arrow)),
        Some((11, 21, PointerShape::Arrow)),
        Some((11, 21, PointerShape::IBeam)),
        Some((12, 22, PointerShape::Hidden)),
        Some((13, 23, PointerShape::Hidden)),
        Some((14, 24, PointerShape::Hand)),
        None,
    ] {
        display.show_pointer(wanted, &mut deadline).unwrap();
    }
    assert_eq!(
        shown.lock().unwrap().pointer,
        [
            ("show", (0, 0), (10, 20)),
            ("move", (0, 0), (11, 21)),
            ("show", (4, 8), (11, 21)),
            ("hide", (0, 0), (0, 0)),
            ("show", (5, 0), (14, 24)),
            ("hide", (0, 0), (0, 0)),
        ]
    );
}

#[test]
fn at_scale_two_programs_draw_half_the_size_and_a_flush_doubles_each_pixel() {
    let (mut display, shown) = display(8, 4);
    assert!(!display.set_scale(3));
    assert!(display.set_scale(2));
    assert_eq!(display.scale(), 2);
    assert_eq!(display.mode(), Mode::new(4, 2));
    display.flush(&mut deadline()).unwrap();
    display.draw(|framebuffer| framebuffer.fill(Rect::new(1, 1, 1, 1), 0x0012_3456));
    assert_eq!(display.flush(&mut deadline()), Ok(1));
    assert_eq!(
        shown.lock().unwrap().flushed.last().unwrap(),
        &vec![Rect::new(2, 2, 2, 2)]
    );
    assert!(display.set_scale(1));
    assert_eq!(display.mode(), Mode::new(8, 4));
    let (mut odd, _) = display_of_odd_size();
    assert!(!odd.set_scale(2));
}

fn display_of_odd_size() -> (Display, Arc<Mutex<Shown>>) {
    display(7, 4)
}

#[test]
fn doubling_copies_a_rect_as_squares() {
    let logical = [1, 2, 3, 4, 5, 6];
    let mut device = [0u32; 24];
    double_into(&logical, 3, &mut device, 6, Rect::new(1, 0, 2, 2), 2);
    assert_eq!(
        device,
        [
            0, 0, 2, 2, 3, 3, 0, 0, 2, 2, 3, 3, 0, 0, 5, 5, 6, 6, 0, 0, 5, 5, 6, 6,
        ]
    );
}
