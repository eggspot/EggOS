use crate::input::{Input, Modifiers, key};
use crate::text;
use crate::text_area;
use crate::theme::{DARK, Token};
use crate::tree::{WidgetId, WidgetTree};
use crate::widget::{self, Escape, Kind, LINE_HEIGHT, Length, Place, field_inner};

const WIDTH: i32 = 320;
const HEIGHT: i32 = 200;
const ACUTE_KEY: u16 = 13;
const E_KEY: u16 = 18;
const A_KEY: u16 = 30;

fn fixture(kind: Kind, height: i32) -> (WidgetTree<8, 1>, WidgetId) {
    let mut tree = WidgetTree::<8, 1>::new();
    let root = tree
        .add(None, Place::fill(), Kind::panel(Token::Menu))
        .unwrap();
    let id = tree
        .add(
            Some(root),
            Place::at(16, 40, Length::Px(200), Length::Px(height)),
            kind,
        )
        .unwrap();
    tree.resize(WIDTH, HEIGHT);
    tree.set_focus(Some(id), 0);
    (tree, id)
}

fn field() -> (WidgetTree<8, 1>, WidgetId) {
    fixture(Kind::text_field("", Escape::Clear), 28)
}

fn typed(tree: &mut WidgetTree<8, 1>, code: u16, character: char) {
    for pressed in [true, false] {
        tree.handle(
            Input::Key {
                code,
                pressed,
                modifiers: Modifiers(0),
                character: u32::from(character),
            },
            0,
        );
    }
}

fn dead(tree: &mut WidgetTree<8, 1>, accent: Option<char>) {
    tree.handle(Input::DeadKey { accent }, 0);
}

fn pixels(tree: &WidgetTree<8, 1>) -> Vec<u32> {
    let mut pixels = vec![0u32; (WIDTH * HEIGHT) as usize];
    tree.render(&mut pixels, WIDTH as u32, HEIGHT as u32, 1);
    pixels
}

fn at(pixels: &[u32], x: i32, y: i32) -> u32 {
    pixels[(y * WIDTH + x) as usize]
}

fn caret_cell(tree: &WidgetTree<8, 1>, id: WidgetId, before: &str) -> (i32, i32) {
    let rect = tree.rect(id).unwrap();
    (
        field_inner(rect).x + text::width(before),
        rect.y + (rect.height - LINE_HEIGHT) / 2,
    )
}

#[test]
fn a_dead_key_shows_its_accent_on_the_accent_colour_in_place_of_the_caret() {
    let (mut tree, id) = field();
    typed(&mut tree, A_KEY, 'a');
    let (x, y) = caret_cell(&tree, id, "a");
    let before = pixels(&tree);
    assert_eq!(at(&before, x - 1, y + LINE_HEIGHT - 1), DARK.text, "the caret");
    tree.take_damage();
    dead(&mut tree, Some('´'));
    assert_eq!(tree.dead_key(), Some('´'));
    assert!(!tree.take_damage().is_empty());
    let shown = pixels(&tree);
    let cell = widget::dead_key_cell(x, y, '´');
    assert!(cell.width >= widget::DEAD_KEY_MIN_WIDTH);
    assert_eq!(at(&shown, x, y + LINE_HEIGHT - 1), DARK.accent);
    assert_eq!(
        at(&shown, cell.right() - 1, y + LINE_HEIGHT - 1),
        DARK.accent
    );
    assert_ne!(at(&shown, x - 1, y + LINE_HEIGHT - 1), DARK.text, "no caret");
    let accent_ink = (cell.x..cell.right())
        .flat_map(|column| (cell.y..cell.bottom()).map(move |row| (column, row)))
        .filter(|&(column, row)| at(&shown, column, row) == DARK.icon_dark)
        .count();
    assert!(accent_ink > 0, "the accent is drawn in icon_dark");
    assert_eq!(tree.text_of(id), Some("a"));
}

#[test]
fn the_next_letter_replaces_the_highlight_and_brings_the_caret_back() {
    let (mut tree, id) = field();
    typed(&mut tree, A_KEY, 'a');
    dead(&mut tree, Some('´'));
    typed(&mut tree, E_KEY, 'é');
    assert_eq!(tree.dead_key(), None);
    assert_eq!(tree.text_of(id), Some("aé"));
    let (mut typed_directly, _) = field();
    typed(&mut typed_directly, A_KEY, 'a');
    typed(&mut typed_directly, E_KEY, 'é');
    assert_eq!(pixels(&tree), pixels(&typed_directly));
}

#[test]
fn backspace_after_a_dead_key_removes_the_cell_and_deletes_nothing() {
    let (mut tree, id) = field();
    typed(&mut tree, A_KEY, 'a');
    let before = pixels(&tree);
    dead(&mut tree, Some('´'));
    dead(&mut tree, None);
    assert_eq!(tree.dead_key(), None);
    assert_eq!(tree.text_of(id), Some("a"));
    assert_eq!(pixels(&tree), before);
}

#[test]
fn a_modifier_leaves_the_accent_waiting_and_another_key_drops_it() {
    let (mut tree, _) = field();
    dead(&mut tree, Some('¨'));
    tree.handle(
        Input::Key {
            code: key::LEFT_SHIFT,
            pressed: true,
            modifiers: Modifiers(Modifiers::SHIFT),
            character: 0,
        },
        0,
    );
    assert_eq!(tree.dead_key(), Some('¨'));
    typed(&mut tree, key::LEFT, '\0');
    assert_eq!(tree.dead_key(), None);
}

#[test]
fn the_highlight_goes_with_the_focus_and_only_text_widgets_take_it() {
    let (mut tree, id) = field();
    dead(&mut tree, Some('^'));
    tree.handle(Input::FocusOut, 0);
    assert_eq!(tree.dead_key(), None);
    tree.handle(Input::FocusIn, 0);
    dead(&mut tree, Some('^'));
    tree.set_focus(None, 0);
    assert_eq!(tree.dead_key(), None);
    dead(&mut tree, Some('^'));
    assert_eq!(tree.dead_key(), None, "nothing focused shows it");
    tree.set_focus(Some(id), 0);
    dead(&mut tree, Some('^'));
    assert_eq!(tree.dead_key(), Some('^'));
}

#[test]
fn a_text_area_shows_the_accent_at_its_caret() {
    let (mut tree, id) = fixture(Kind::text_area(0, ""), 92);
    tree.set_text(id, "ab");
    let caret = {
        let Some(Kind::TextArea(area)) = tree.kind(id).copied() else {
            panic!("not a text area");
        };
        area
    };
    let rect = tree.rect(id).unwrap();
    let before = pixels(&tree);
    dead(&mut tree, Some('`'));
    assert_eq!(tree.dead_key(), Some('`'));
    let shown = pixels(&tree);
    let inner = text_area::inner(rect);
    let x = inner.x + text::width(&"ab"[..caret.caret]);
    let cell = widget::dead_key_cell(x, inner.y, '`');
    assert_eq!(
        at(&shown, cell.x, cell.bottom() - 1),
        DARK.accent
    );
    assert_ne!(shown, before);
    typed(&mut tree, key::SPACE, '`');
    assert_eq!(tree.dead_key(), None);
}
