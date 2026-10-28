use crate::canvas::Canvas;
use crate::colour;
use crate::geometry::Rect;
use crate::input::{Input, Modifiers, MouseButton, key};
use crate::tree::{PointerShape, Response, WidgetId, WidgetTree};
use crate::widget::{Escape, Kind, Length, Place, card_rect};

const ITEMS: &[&str] = &["Appearance", "Display", "Keyboard", "Mouse", "About"];

fn no_art(_: &mut Canvas<'_>, _: usize, _: Rect) {}

struct Form {
    tree: WidgetTree<24>,
    field: WidgetId,
    list: WidgetId,
    cards: WidgetId,
    ok: WidgetId,
    check: WidgetId,
    radios: WidgetId,
    off: WidgetId,
    value: WidgetId,
    scroll: WidgetId,
}

fn form() -> Form {
    let mut tree = WidgetTree::<24>::new();
    let side = tree
        .add(
            None,
            Place::at(0, 0, Length::Px(120), Length::Fill(0)),
            Kind::panel(colour::DOCK),
        )
        .unwrap();
    let field = tree
        .add(
            Some(side),
            Place::at(8, 8, Length::Fill(8), Length::Px(28)),
            Kind::text_field("Search", Escape::Clear),
        )
        .unwrap();
    let list = tree
        .add(
            Some(side),
            Place::at(0, 44, Length::Fill(0), Length::Px(96)),
            Kind::list(ITEMS, 32, 4, 16),
        )
        .unwrap();
    let pane = tree
        .add(
            None,
            Place::at(120, 0, Length::Fill(0), Length::Fill(0)),
            Kind::panel(colour::MENU),
        )
        .unwrap();
    let scroll = tree
        .add(
            Some(pane),
            Place::fill(),
            Kind::ScrollView {
                offset: 0,
                content_height: 0,
            },
        )
        .unwrap();
    let cards = tree
        .add(
            Some(scroll),
            Place::at(16, 16, Length::Content, Length::Content),
            Kind::cards(&["One", "Two", "Three"], 1, (64, 48), 8, no_art),
        )
        .unwrap();
    let column = tree
        .add(
            Some(scroll),
            Place::at(16, 80, Length::Fill(16), Length::Content),
            Kind::Column { gap: 4 },
        )
        .unwrap();
    let check = tree
        .add(
            Some(column),
            Place::at(0, 0, Length::Content, Length::Content),
            Kind::checkbox("Check", false),
        )
        .unwrap();
    let off = tree
        .add(
            Some(column),
            Place::at(0, 0, Length::Content, Length::Content),
            Kind::button("Off", false),
        )
        .unwrap();
    let radios = tree
        .add(
            Some(column),
            Place::at(0, 0, Length::Content, Length::Content),
            Kind::radios(&["1x", "2x"], 0, true),
        )
        .unwrap();
    let value = tree
        .add(
            Some(scroll),
            Place::right(16, 200),
            Kind::label("Right", colour::TEXT),
        )
        .unwrap();
    let ok = tree
        .add(
            Some(scroll),
            Place::at(16, 300, Length::Content, Length::Content),
            Kind::button("OK", true),
        )
        .unwrap();
    tree.set_enabled(off, false);
    tree.set_default_button(ok);
    tree.resize(400, 200);
    tree.take_damage();
    Form {
        tree,
        field,
        list,
        cards,
        ok,
        check,
        radios,
        off,
        value,
        scroll,
    }
}

fn press(tree: &mut WidgetTree<24>, code: u16, character: u32, shift: bool) -> Response {
    let modifiers = Modifiers(if shift { Modifiers::SHIFT } else { 0 });
    let response = tree.handle(
        Input::Key {
            code,
            pressed: true,
            modifiers,
            character,
        },
        0,
    );
    tree.handle(
        Input::Key {
            code,
            pressed: false,
            modifiers,
            character,
        },
        0,
    );
    response
}

fn click(tree: &mut WidgetTree<24>, x: i32, y: i32, time_ms: u32) -> Response {
    tree.handle(Input::PointerMove { x, y }, time_ms);
    let pressed = tree.handle(
        Input::Button {
            x,
            y,
            button: MouseButton::Left,
            pressed: true,
        },
        time_ms,
    );
    let released = tree.handle(
        Input::Button {
            x,
            y,
            button: MouseButton::Left,
            pressed: false,
        },
        time_ms,
    );
    if released == Response::Nothing {
        pressed
    } else {
        released
    }
}

fn centre(rect: Rect) -> (i32, i32) {
    (rect.x + rect.width / 2, rect.y + rect.height / 2)
}

#[test]
fn widgets_are_laid_out_by_their_places_columns_and_scroll_views() {
    let form = form();
    let tree = &form.tree;
    assert_eq!(tree.rect(form.field), Some(Rect::new(8, 8, 104, 28)));
    assert_eq!(tree.rect(form.list), Some(Rect::new(0, 44, 120, 96)));
    assert_eq!(
        tree.rect(form.cards),
        Some(Rect::new(136, 16, 64 * 3 + 8 * 2, 48))
    );
    assert_eq!(
        tree.rect(form.check)
            .map(|rect| (rect.x, rect.y, rect.height)),
        Some((136, 80, 28))
    );
    assert_eq!(
        tree.rect(form.off).map(|rect| (rect.y, rect.width)),
        Some((112, 96))
    );
    assert_eq!(tree.rect(form.radios).map(|rect| rect.y), Some(144));
    let value = tree.rect(form.value).unwrap();
    assert_eq!(value.right(), 400 - 16);
    assert_eq!(value.y, 200);
    let Some(Kind::ScrollView { content_height, .. }) = tree.kind(form.scroll) else {
        panic!("the pane scrolls");
    };
    assert_eq!(*content_height, 328);
}

#[test]
fn a_scroll_view_moves_its_children_with_the_wheel_and_clips_their_hits() {
    let mut form = form();
    let tree = &mut form.tree;
    tree.handle(
        Input::Wheel {
            x: 300,
            y: 100,
            notches: -1,
        },
        0,
    );
    assert_eq!(tree.rect(form.cards).map(|rect| rect.y), Some(16 - 72));
    let ok = tree.rect(form.ok).unwrap();
    assert_eq!(ok.y, 300 - 72);
    tree.handle(
        Input::Wheel {
            x: 300,
            y: 100,
            notches: -10,
        },
        0,
    );
    assert_eq!(tree.rect(form.ok).map(|rect| rect.y), Some(300 - 128));
    assert!(!tree.take_damage().is_empty());
    let (x, y) = centre(tree.rect(form.ok).unwrap());
    assert_eq!(click(tree, x, y, 0), Response::Activated(form.ok));
    tree.handle(
        Input::Wheel {
            x: 300,
            y: 100,
            notches: 10,
        },
        0,
    );
    assert_eq!(click(tree, x, 190, 0), Response::Nothing);
    assert_eq!(click(tree, x, 300 - 4, 0), Response::Nothing);
}

#[test]
fn a_button_activates_on_a_release_inside_only_and_a_disabled_one_never() {
    let mut form = form();
    let tree = &mut form.tree;
    let (x, y) = centre(tree.rect(form.ok).unwrap());
    tree.handle(
        Input::Wheel {
            x: 300,
            y: 100,
            notches: -10,
        },
        0,
    );
    let (x, y) = (x, y - 128);
    assert_eq!(click(tree, x, y, 0), Response::Activated(form.ok));
    tree.handle(
        Input::Button {
            x,
            y,
            button: MouseButton::Left,
            pressed: true,
        },
        0,
    );
    tree.handle(Input::PointerMove { x: 5, y: 5 }, 0);
    let released = tree.handle(
        Input::Button {
            x: 5,
            y: 5,
            button: MouseButton::Left,
            pressed: false,
        },
        0,
    );
    assert_eq!(released, Response::Nothing);
    tree.handle(
        Input::Wheel {
            x: 300,
            y: 100,
            notches: 10,
        },
        0,
    );
    let (x, y) = centre(tree.rect(form.off).unwrap());
    assert_eq!(click(tree, x, y, 0), Response::Nothing);
    assert_ne!(tree.focus(), Some(form.off));
}

#[test]
fn tab_moves_the_focus_in_order_skipping_disabled_widgets_and_wrapping() {
    let mut form = form();
    let tree = &mut form.tree;
    let mut order = Vec::new();
    for _ in 0..7 {
        press(tree, key::TAB, 9, false);
        order.push(tree.focus().unwrap());
    }
    assert_eq!(
        order,
        [
            form.field,
            form.list,
            form.cards,
            form.check,
            form.radios,
            form.ok,
            form.field
        ]
    );
    press(tree, key::TAB, 9, true);
    assert_eq!(tree.focus(), Some(form.ok));
}

#[test]
fn cards_move_the_ring_with_arrows_and_select_with_enter_space_or_a_click() {
    let mut form = form();
    let tree = &mut form.tree;
    tree.set_focus(Some(form.cards), 0);
    assert_eq!(press(tree, key::RIGHT, 0, false), Response::Nothing);
    assert_eq!(tree.selected(form.cards), Some(1));
    assert_eq!(
        press(tree, key::ENTER, 13, false),
        Response::Chosen(form.cards, 2)
    );
    assert_eq!(press(tree, key::RIGHT, 0, false), Response::Nothing);
    assert_eq!(press(tree, key::SPACE, 32, false), Response::Nothing);
    let kind = *tree.kind(form.cards).unwrap();
    let first = card_rect(&kind, tree.rect(form.cards).unwrap(), 0);
    let (x, y) = centre(first);
    assert_eq!(click(tree, x, y, 0), Response::Chosen(form.cards, 0));
    assert_eq!(click(tree, x, y, 1000), Response::Nothing);
}

#[test]
fn checkboxes_toggle_and_radios_follow_the_arrows_without_wrapping() {
    let mut form = form();
    let tree = &mut form.tree;
    let (x, y) = centre(tree.rect(form.check).unwrap());
    assert_eq!(click(tree, x, y, 0), Response::Toggled(form.check, true));
    assert_eq!(
        press(tree, key::SPACE, 32, false),
        Response::Toggled(form.check, false)
    );
    tree.set_focus(Some(form.radios), 0);
    assert_eq!(
        press(tree, key::RIGHT, 0, false),
        Response::Chosen(form.radios, 1)
    );
    assert_eq!(press(tree, key::RIGHT, 0, false), Response::Nothing);
    assert_eq!(
        press(tree, key::UP, 0, false),
        Response::Chosen(form.radios, 0)
    );
}

#[test]
fn a_list_selects_by_click_and_keys_opens_by_enter_and_double_click_and_filters() {
    let mut form = form();
    let tree = &mut form.tree;
    let list = tree.rect(form.list).unwrap();
    assert_eq!(
        click(tree, 20, list.y + 40, 0),
        Response::Chosen(form.list, 1)
    );
    assert_eq!(
        click(tree, 20, list.y + 40, 100),
        Response::Opened(form.list, 1)
    );
    assert_eq!(
        press(tree, key::DOWN, 0, false),
        Response::Chosen(form.list, 2)
    );
    assert_eq!(
        press(tree, key::END, 0, false),
        Response::Chosen(form.list, 4)
    );
    let Some(Kind::List(state)) = tree.kind(form.list) else {
        panic!("a list");
    };
    assert_eq!(state.offset, 5 * 32 - 96);
    assert_eq!(
        press(tree, key::ENTER, 13, false),
        Response::Opened(form.list, 4)
    );
    assert_eq!(
        press(tree, key::HOME, 0, false),
        Response::Chosen(form.list, 0)
    );
    assert!(tree.set_shown_items(form.list, 0b10100));
    assert_eq!(tree.selected(form.list), Some(2));
    assert_eq!(
        press(tree, key::DOWN, 0, false),
        Response::Chosen(form.list, 4)
    );
    assert!(!tree.set_shown_items(form.list, 0));
}

#[test]
fn a_text_field_edits_at_the_caret_and_esc_clears_it() {
    let mut form = form();
    let tree = &mut form.tree;
    tree.set_focus(Some(form.field), 0);
    for character in "Kyb".chars() {
        assert_eq!(
            press(tree, 30, character as u32, false),
            Response::Edited(form.field)
        );
    }
    press(tree, key::LEFT, 0, false);
    assert_eq!(
        press(tree, 18, 'e' as u32, false),
        Response::Edited(form.field)
    );
    assert_eq!(tree.text_of(form.field), Some("Kyeb"));
    press(tree, key::HOME, 0, false);
    assert_eq!(
        press(tree, key::DELETE, 0, false),
        Response::Edited(form.field)
    );
    press(tree, key::END, 0, false);
    assert_eq!(
        press(tree, key::BACKSPACE, 8, false),
        Response::Edited(form.field)
    );
    assert_eq!(tree.text_of(form.field), Some("ye"));
    assert_eq!(
        tree.handle(
            Input::Key {
                code: 46,
                pressed: true,
                modifiers: Modifiers(Modifiers::CTRL),
                character: 3
            },
            0
        ),
        Response::Unused {
            code: 46,
            shift: false
        }
    );
    assert_eq!(
        press(tree, key::ESC, 27, false),
        Response::Edited(form.field)
    );
    assert_eq!(tree.text_of(form.field), Some(""));
    assert_eq!(
        press(tree, key::ESC, 27, false),
        Response::Unused {
            code: key::ESC,
            shift: false
        }
    );
    assert_eq!(
        press(tree, key::ENTER, 13, false),
        Response::Activated(form.ok)
    );
    let field = tree.rect(form.field).unwrap();
    tree.handle(
        Input::PointerMove {
            x: field.x + 4,
            y: field.y + 4,
        },
        0,
    );
    assert_eq!(tree.pointer_shape(), PointerShape::IBeam);
}

#[test]
fn a_long_text_scrolls_to_keep_the_caret_inside_the_field() {
    let mut form = form();
    let tree = &mut form.tree;
    tree.set_focus(Some(form.field), 0);
    for _ in 0..40 {
        press(tree, 30, 'm' as u32, false);
    }
    let Some(Kind::TextField(field)) = tree.kind(form.field) else {
        panic!("a field");
    };
    assert_eq!(field.scroll, 40 * 8 - (104 - 16));
    press(tree, key::HOME, 0, false);
    let Some(Kind::TextField(field)) = tree.kind(form.field) else {
        panic!("a field");
    };
    assert_eq!(field.scroll, 0);
}

#[test]
fn the_caret_blinks_every_500_ms_while_the_window_has_the_focus() {
    let mut form = form();
    let tree = &mut form.tree;
    assert_eq!(tree.next_deadline(0), None);
    tree.set_focus(Some(form.field), 1000);
    tree.take_damage();
    assert_eq!(tree.next_deadline(1100), Some(1500));
    tree.tick(1499);
    assert!(tree.take_damage().is_empty());
    tree.tick(1500);
    assert_eq!(tree.take_damage().rects().len(), 1);
    tree.handle(Input::FocusOut, 1600);
    assert_eq!(tree.next_deadline(1600), None);
}

#[test]
fn rendering_draws_the_tokens_and_the_focus_ring_only_while_the_window_is_focused() {
    let mut form = form();
    let mut pixels = vec![0u32; 400 * 200];
    form.tree.render(&mut pixels, 400, 200, 1);
    assert_eq!(pixels[0], colour::DOCK);
    assert_eq!(pixels[300], colour::MENU);
    let list = form.tree.rect(form.list).unwrap();
    let at = |pixels: &[u32], x: i32, y: i32| pixels[(y * 400 + x) as usize];
    assert_eq!(at(&pixels, list.x + 1, list.y + 1), colour::SELECTION);
    assert_ne!(at(&pixels, list.x, list.y), colour::FOCUS_RING);
    form.tree.set_focus(Some(form.list), 0);
    form.tree.render(&mut pixels, 400, 200, 1);
    assert_eq!(at(&pixels, list.x + 1, list.y + 5), colour::FOCUS_RING);
    form.tree.handle(Input::FocusOut, 0);
    form.tree.render(&mut pixels, 400, 200, 1);
    assert_eq!(at(&pixels, list.x + 1, list.y + 5), colour::SELECTION);
    let mut doubled = vec![0u32; 800 * 400];
    form.tree.render(&mut doubled, 800, 400, 2);
    for y in 0..200 {
        for x in 0..400 {
            assert_eq!(doubled[(y * 2 + 1) * 800 + x * 2 + 1], pixels[y * 400 + x]);
        }
    }
}

#[test]
fn painting_the_damage_alone_gives_the_same_frame_as_painting_everything() {
    let mut form = form();
    let mut pixels = vec![0u32; 400 * 200];
    form.tree.render(&mut pixels, 400, 200, 1);
    let tree = &mut form.tree;
    let (x, y) = centre(tree.rect(form.check).unwrap());
    click(tree, x, y, 0);
    press(tree, key::TAB, 9, false);
    press(tree, key::RIGHT, 0, false);
    tree.handle(Input::PointerMove { x: 140, y: 30 }, 0);
    let damage = tree.take_damage();
    assert!(!damage.is_empty());
    {
        let mut canvas = Canvas::new(&mut pixels, 400, 200, 1).unwrap();
        for rect in damage.rects() {
            let previous = canvas.narrow(*rect);
            tree.paint(&mut canvas);
            canvas.restore(previous);
        }
    }
    let mut whole = vec![0u32; 400 * 200];
    tree.render(&mut whole, 400, 200, 1);
    assert!(pixels == whole);
}

#[test]
fn a_known_string_renders_to_the_same_pixels() {
    let mut pixels = vec![0u32; 120 * 16];
    let mut canvas = Canvas::new(&mut pixels, 120, 16, 1).unwrap();
    canvas.text(0, 0, "Settings · Égal 0123", 1, 1);
    let hash = pixels
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, &pixel| {
            (hash ^ u64::from(pixel)).wrapping_mul(0x0100_0000_01b3)
        });
    assert_eq!(hash, 8151596901170438906);
}

fn tile(canvas: &mut Canvas<'_>, rect: Rect, data: u32) {
    canvas.fill(rect, data);
}

#[test]
fn a_progress_bar_fills_its_done_part_with_the_accent_and_a_custom_widget_gets_its_data() {
    let mut tree = WidgetTree::<4>::new();
    let bar = tree
        .add(
            None,
            Place::at(10, 10, Length::Px(100), Length::Px(8)),
            Kind::Progress { permille: 0 },
        )
        .unwrap();
    tree.add(
        None,
        Place::at(0, 30, Length::Px(4), Length::Px(4)),
        Kind::Custom {
            painter: tile,
            data: 0x0012_3456,
        },
    )
    .unwrap();
    tree.resize(120, 40);
    let mut pixels = vec![0u32; 120 * 40];
    tree.render(&mut pixels, 120, 40, 1);
    assert_eq!(pixels[14 * 120 + 50], colour::PROGRESS_TRACK);
    assert_eq!(pixels[31 * 120 + 1], 0x0012_3456);
    tree.take_damage();
    tree.set_progress(bar, 250);
    assert!(!tree.take_damage().is_empty());
    tree.set_progress(bar, 250);
    assert!(tree.take_damage().is_empty());
    tree.render(&mut pixels, 120, 40, 1);
    assert_eq!(pixels[14 * 120 + 20], colour::ACCENT);
    assert_eq!(pixels[14 * 120 + 50], colour::PROGRESS_TRACK);
    tree.set_progress(bar, 4000);
    tree.render(&mut pixels, 120, 40, 1);
    assert_eq!(pixels[14 * 120 + 105], colour::ACCENT);
}
