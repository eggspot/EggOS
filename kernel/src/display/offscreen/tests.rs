use super::*;
use crate::display::{Display, FIRST_FRAME_COLOUR, FIRST_FRAME_HASH_1280X800, pixel_hash};
use crate::tests::TestPlatform;
use alloc::string::String;

fn asked(line: &str) -> Option<Result<Mode, Refusal>> {
    requested_mode(line.as_bytes())
}

fn reported(outcome: Option<Result<Mode, Refusal>>) -> String {
    let mut out = String::new();
    report(&mut out, outcome).unwrap();
    out
}

#[test]
fn a_line_without_the_parameter_asks_for_nothing() {
    for line in [
        "",
        "console=ttyS0",
        "xeggos.display=offscreen",
        "eggos.displayx=offscreen",
        "eggos.mcp_allow=run",
        "eggos.display",
    ] {
        assert_eq!(asked(line), None, "{line}");
    }
}

#[test]
fn offscreen_alone_is_the_default_mode_and_a_size_names_its_own() {
    assert_eq!(DEFAULT_MODE, Mode::new(1280, 800));
    assert_eq!(asked("eggos.display=offscreen"), Some(Ok(DEFAULT_MODE)));
    assert_eq!(
        asked("eggos.display=offscreen:1024x768"),
        Some(Ok(Mode::new(1024, 768)))
    );
    assert_eq!(Mode::new(1024, 1024).bytes(), MAX_BYTES);
    assert_eq!(
        asked("eggos.display=offscreen:1024x1024"),
        Some(Ok(Mode::new(1024, 1024)))
    );
}

#[test]
fn sizes_over_the_ceiling_are_too_large_before_the_desktop_sees_them() {
    assert_eq!(MAX_BYTES, 4 << 20);
    for size in ["1280x820", "1025x1024", "99999x99999", "2048x2048"] {
        assert_eq!(
            asked(&format!("eggos.display=offscreen:{size}")),
            Some(Err(Refusal::TooLarge)),
            "{size}"
        );
    }
}

#[test]
fn sizes_the_desktop_does_not_fit_are_too_small() {
    for size in ["99999x1", "1x99999", "0x0", "640x400", "1280x1", "1x800"] {
        assert_eq!(
            asked(&format!("eggos.display=offscreen:{size}")),
            Some(Err(Refusal::TooSmall)),
            "{size}"
        );
    }
}

#[test]
fn anything_but_the_two_spellings_is_not_understood() {
    for value in [
        "offscreen:",
        "offscreen:x",
        "offscreen:1024",
        "offscreen:1024x",
        "offscreen:x768",
        "offscreen:+1024x768",
        "offscreen:-1024x768",
        "offscreen:1024x768x2",
        "offscreen:123456x1",
        "offscreen:1x123456",
        "offscreen:01280x0800",
        "offscreen:1280x0800",
        "offscreen:1280X800",
        "offscreen=1280x800",
        "offscreen1280x800",
        "offscreens",
        "onscreen",
        "OFFSCREEN",
        "",
    ] {
        assert_eq!(
            requested_mode(format!("eggos.display={value}").as_bytes()),
            Some(Err(Refusal::NotUnderstood)),
            "{value:?}"
        );
    }
}

#[test]
fn the_first_word_decides() {
    assert_eq!(
        asked("eggos.display=offscreen eggos.display=onscreen"),
        Some(Ok(DEFAULT_MODE))
    );
    assert_eq!(
        asked("eggos.display=onscreen eggos.display=offscreen"),
        Some(Err(Refusal::NotUnderstood))
    );
}

#[test]
fn the_word_is_found_beside_the_grants_in_either_order() {
    for line in [
        "eggos.display=offscreen eggos.mcp_allow=view,write,run,install",
        "eggos.mcp_allow=view,write,run,install eggos.display=offscreen",
        "eggos.mcp_allow=run\teggos.display=offscreen\n",
        "  eggos.display=offscreen\t eggos.mcp_allow=run  ",
    ] {
        assert_eq!(asked(line), Some(Ok(DEFAULT_MODE)), "{line:?}");
    }
}

#[test]
fn the_scanout_is_black_and_its_flush_shows_every_rectangle_and_nothing_else() {
    let mut scanout = OffscreenScanout::new(Mode::new(16, 8)).unwrap();
    assert_eq!(scanout.mode(), Mode::new(16, 8));
    assert_eq!(scanout.pixels().len(), 16 * 8);
    assert!(scanout.pixels().iter().all(|&pixel| pixel == 0));
    let mut deadline = Deadline::after::<TestPlatform>(1);
    let rects = [Rect::new(0, 0, 2, 2), Rect::new(4, 4, 1, 1)];
    assert_eq!(scanout.flush(&rects, &mut deadline), Ok(2));
    assert_eq!(scanout.flush(&[], &mut deadline), Ok(0));
}

#[test]
fn the_desktop_s_first_frame_on_it_is_the_one_a_screen_shows() {
    let scanout = OffscreenScanout::new(DEFAULT_MODE).unwrap();
    let mut display = Display::new(Box::new(scanout));
    display.draw(|framebuffer| framebuffer.fill(DEFAULT_MODE.rect(), FIRST_FRAME_COLOUR));
    let mut deadline = Deadline::after::<TestPlatform>(1);
    assert_eq!(display.flush(&mut deadline), Ok(1));
    assert_eq!(display.flushes(), 1);
    let hash = display.draw(|framebuffer| pixel_hash(framebuffer.pixels()));
    assert_eq!(hash, FIRST_FRAME_HASH_1280X800);
}

#[test]
fn a_scanout_is_made_only_when_the_line_asks_for_one() {
    assert!(requested_scanout(b"console=ttyS0").is_none());
    let made = requested_scanout(b"eggos.display=offscreen:1024x768").unwrap();
    assert_eq!(made.unwrap().mode(), Mode::new(1024, 768));
    let refused = requested_scanout(b"eggos.display=offscreen:2048x2048").unwrap();
    assert_eq!(refused.unwrap_err(), Refusal::TooLarge);
}

#[test]
fn a_display_already_in_use_refuses_the_scanout() {
    let scanout = || OffscreenScanout::new(Mode::new(16, 8)).unwrap();
    assert_eq!(installed(scanout(), |_| Ok(())), Ok(Mode::new(16, 8)));
    assert_eq!(installed(scanout(), Err), Err(Refusal::DisplayInUse));
}

#[test]
fn the_console_says_the_mode_or_a_fixed_reason_and_never_the_host_s_value() {
    assert_eq!(reported(None), "");
    assert_eq!(
        reported(Some(Ok(DEFAULT_MODE))),
        "boot: offscreen display 1280x800\n"
    );
    assert!(reported(Some(Ok(DEFAULT_MODE))).starts_with(LINE_PREFIX));
    for refusal in [
        Refusal::NotUnderstood,
        Refusal::TooLarge,
        Refusal::TooSmall,
        Refusal::NoMemory,
        Refusal::DisplayInUse,
    ] {
        assert_eq!(
            reported(Some(Err(refusal))),
            format!("boot: eggos.display refused: {}\n", refusal.description())
        );
    }
    let hostile = "eggos.display=offscreen:\x1b[2J";
    let line = reported(asked(hostile));
    assert_eq!(
        line,
        format!(
            "boot: eggos.display refused: {}\n",
            Refusal::NotUnderstood.description()
        )
    );
    assert!(!line.contains('\x1b'));
}
