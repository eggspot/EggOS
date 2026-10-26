use super::*;
use crate::tests::TestPlatform;
use alloc::vec;
use alloc::vec::Vec;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Shown {
    flushed: Vec<Vec<Rect>>,
    failing: bool,
}

struct FakeScanout {
    mode: Mode,
    pixels: Vec<u32>,
    shown: Arc<Mutex<Shown>>,
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
}

fn display(width: u32, height: u32) -> (Display, Arc<Mutex<Shown>>) {
    let shown = Arc::new(Mutex::new(Shown::default()));
    let scanout = FakeScanout {
        mode: Mode::new(width, height),
        pixels: vec![0; (width * height) as usize],
        shown: Arc::clone(&shown),
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
