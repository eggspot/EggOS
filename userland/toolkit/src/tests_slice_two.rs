use crate::geometry::Rect;
use crate::input::{Input, Modifiers, MouseButton, key};
use crate::menu::{Entry, POOL_CAPACITY};
use crate::text::Text;
use crate::theme::{self, DARK, LIGHT, LIGHT_HIGH_CONTRAST, Palette, Token};
use crate::tree::{PointerShape, Response, WidgetId, WidgetTree};
use crate::widget::{Align, Column, DropMode, Escape, FormFactor, Kind, Length, Place, SortOrder};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_element_tree::{Action, MAX_TREE_BYTES, Outcome, Role, TreeWriter, elements, flags};

const WIDTH: i32 = 480;
const HEIGHT: i32 = 360;

fn moved<const N: usize, const A: usize>(
    tree: &mut WidgetTree<N, A>,
    x: i32,
    y: i32,
    time_ms: u32,
) -> Response {
    tree.handle(Input::PointerMove { x, y }, time_ms)
}

fn button<const N: usize, const A: usize>(
    tree: &mut WidgetTree<N, A>,
    x: i32,
    y: i32,
    pressed: bool,
    time_ms: u32,
) -> Response {
    tree.handle(
        Input::Button {
            x,
            y,
            button: MouseButton::Left,
            pressed,
        },
        time_ms,
    )
}

fn click<const N: usize, const A: usize>(
    tree: &mut WidgetTree<N, A>,
    x: i32,
    y: i32,
    time_ms: u32,
) -> (Response, Response) {
    let pressed = button(tree, x, y, true, time_ms);
    (pressed, button(tree, x, y, false, time_ms))
}

fn key_with<const N: usize, const A: usize>(
    tree: &mut WidgetTree<N, A>,
    code: u16,
    character: u32,
    modifiers: u32,
) -> Response {
    let response = tree.handle(
        Input::Key {
            code,
            pressed: true,
            modifiers: Modifiers(modifiers),
            character,
        },
        0,
    );
    tree.handle(
        Input::Key {
            code,
            pressed: false,
            modifiers: Modifiers(modifiers),
            character,
        },
        0,
    );
    response
}

fn key<const N: usize, const A: usize>(tree: &mut WidgetTree<N, A>, code: u16) -> Response {
    key_with(tree, code, 0, 0)
}

fn typed<const N: usize, const A: usize>(
    tree: &mut WidgetTree<N, A>,
    code: u16,
    character: char,
) -> Response {
    key_with(tree, code, character as u32, 0)
}

fn pixels<const N: usize, const A: usize>(tree: &WidgetTree<N, A>) -> Vec<u32> {
    let mut pixels = vec![0u32; (WIDTH * HEIGHT) as usize];
    tree.render(&mut pixels, WIDTH as u32, HEIGHT as u32, 1);
    pixels
}

fn at(pixels: &[u32], x: i32, y: i32) -> u32 {
    pixels[(y * WIDTH + x) as usize]
}

type Described = (u32, u32, Role, u16, String, String);

fn described<const N: usize, const A: usize>(tree: &WidgetTree<N, A>) -> Vec<Described> {
    let mut bytes = vec![0u8; MAX_TREE_BYTES];
    let mut writer = TreeWriter::new(&mut bytes);
    tree.describe(&mut writer);
    elements(writer.bytes())
        .map(|element| {
            let element = element.unwrap();
            (
                element.id,
                element.parent,
                element.role,
                element.flags,
                String::from_utf8(element.name.to_vec()).unwrap(),
                String::from_utf8(element.value.to_vec()).unwrap(),
            )
        })
        .collect()
}

fn with_role(described: &[Described], role: Role) -> Vec<Described> {
    described
        .iter()
        .filter(|element| element.2 == role)
        .cloned()
        .collect()
}

fn surface<const N: usize, const A: usize>(tree: &mut WidgetTree<N, A>) -> WidgetId {
    tree.add(None, Place::fill(), Kind::panel(Token::Menu))
        .unwrap()
}

struct TooltipFixture {
    tree: WidgetTree<16>,
    save: WidgetId,
    open: WidgetId,
    save_tip: WidgetId,
}

fn tooltip_fixture() -> TooltipFixture {
    let mut tree = WidgetTree::<16>::new();
    let root = surface(&mut tree);
    let place = |x| Place::at(x, 60, Length::Px(96), Length::Px(28));
    let save = tree
        .add(Some(root), place(16), Kind::button("Save", false))
        .unwrap();
    let save_tip = tree
        .add(
            Some(save),
            Place::at(0, 0, Length::Content, Length::Content),
            Kind::tooltip("Save", "Mod+S"),
        )
        .unwrap();
    let open = tree
        .add(Some(root), place(120), Kind::button("Open", false))
        .unwrap();
    tree.add(
        Some(open),
        Place::at(0, 0, Length::Content, Length::Content),
        Kind::tooltip("Open", ""),
    )
    .unwrap();
    tree.resize(WIDTH, HEIGHT);
    TooltipFixture {
        tree,
        save,
        open,
        save_tip,
    }
}

#[test]
fn a_tooltip_shows_below_its_owner_after_600_ms_and_is_listed_whether_or_not_it_shows() {
    let mut fixture = tooltip_fixture();
    let tree = &mut fixture.tree;
    assert_eq!(tree.rect(fixture.save_tip), Some(Rect::new(22, 92, 84, 24)));
    let listed = with_role(&described(tree), Role::Tooltip);
    assert_eq!(listed.len(), 2);
    assert_eq!(
        (listed[0].4.as_str(), listed[0].5.as_str()),
        ("Save", "Mod+S")
    );
    assert_eq!(moved(tree, 60, 70, 0), Response::Nothing);
    tree.tick(599);
    assert_eq!(tree.shown_tooltip(), None);
    tree.tick(600);
    assert_eq!(tree.shown_tooltip(), Some(fixture.save_tip));
    let pixels = pixels(tree);
    assert_eq!(at(&pixels, 22, 100), DARK.menu_border);
    assert_eq!(at(&pixels, 26, 100), DARK.menu);
}

#[test]
fn a_tooltip_hops_along_a_row_and_a_press_spends_it_until_the_pointer_leaves() {
    let mut fixture = tooltip_fixture();
    let tree = &mut fixture.tree;
    moved(tree, 60, 70, 0);
    tree.tick(600);
    moved(tree, 70, 70, 650);
    assert!(tree.shown_tooltip().is_some());
    moved(tree, 160, 70, 700);
    let open_tip = tree.shown_tooltip().unwrap();
    assert_eq!(tree.rect(open_tip), Some(Rect::new(148, 92, 40, 24)));
    tree.handle(Input::PointerLeave, 800);
    assert_eq!(tree.shown_tooltip(), None);
    moved(tree, 160, 70, 2000);
    tree.tick(2599);
    assert_eq!(tree.shown_tooltip(), None);
    tree.tick(2600);
    assert!(tree.shown_tooltip().is_some());
    moved(tree, 60, 70, 3000);
    tree.tick(3600);
    assert_eq!(button(tree, 60, 70, true, 3700), Response::Nothing);
    assert_eq!(tree.shown_tooltip(), None);
    assert_eq!(
        button(tree, 60, 70, false, 3710),
        Response::Activated(fixture.save)
    );
    tree.tick(5000);
    assert_eq!(tree.shown_tooltip(), None);
    moved(tree, 160, 70, 6000);
    moved(tree, 60, 70, 6000);
    tree.tick(6600);
    assert_eq!(tree.shown_tooltip(), Some(fixture.save_tip));
    key(tree, key::A);
    assert_eq!(tree.shown_tooltip(), None);
    let _ = fixture.open;
}

#[test]
fn a_tooltip_follows_the_keyboard_focus_and_goes_above_or_inside_the_window_when_it_must() {
    let mut fixture = tooltip_fixture();
    let tree = &mut fixture.tree;
    tree.move_focus(false, 0);
    assert_eq!(tree.focus(), Some(fixture.save));
    tree.tick(600);
    assert_eq!(tree.shown_tooltip(), Some(fixture.save_tip));
    tree.handle(
        Input::Key {
            code: key::TAB,
            pressed: true,
            modifiers: Modifiers(0),
            character: 9,
        },
        700,
    );
    assert_eq!(tree.focus(), Some(fixture.open));
    tree.tick(1299);
    assert_eq!(tree.shown_tooltip(), None);
    tree.tick(1300);
    assert!(tree.shown_tooltip().is_some());
    let mut tree = WidgetTree::<8>::new();
    let low = tree
        .add(
            None,
            Place::at(16, 330, Length::Px(96), Length::Px(28)),
            Kind::button("Low", false),
        )
        .unwrap();
    let low_tip = tree
        .add(Some(low), Place::fill(), Kind::tooltip("Low", ""))
        .unwrap();
    let edge = tree
        .add(
            None,
            Place::at(0, 60, Length::Px(40), Length::Px(28)),
            Kind::button("E", false),
        )
        .unwrap();
    let edge_tip = tree
        .add(
            Some(edge),
            Place::fill(),
            Kind::tooltip("A longer name", ""),
        )
        .unwrap();
    tree.resize(WIDTH, HEIGHT);
    assert_eq!(tree.rect(low_tip).unwrap().y, 330 - 4 - 24);
    assert_eq!(tree.rect(edge_tip).unwrap().x, 8);
}

#[test]
fn on_a_phone_a_long_press_shows_the_tooltip_and_on_a_wearable_none_shows() {
    let mut fixture = tooltip_fixture();
    let tree = &mut fixture.tree;
    tree.set_form_factor(FormFactor::Phone);
    button(tree, 60, 70, true, 0);
    tree.tick(500);
    assert_eq!(tree.shown_tooltip(), Some(fixture.save_tip));
    tree.tick(2000);
    assert_eq!(tree.shown_tooltip(), None);
    assert_eq!(button(tree, 60, 70, false, 2100), Response::Nothing);
    let mut fixture = tooltip_fixture();
    let tree = &mut fixture.tree;
    tree.set_form_factor(FormFactor::Wearable);
    moved(tree, 60, 70, 0);
    tree.tick(600);
    assert_eq!(tree.shown_tooltip(), None);
    assert_eq!(with_role(&described(tree), Role::Tooltip).len(), 2);
}

const fn item(label: &'static str, key: &'static str, id: u8, enabled: bool) -> Entry {
    Entry::Item {
        label,
        key,
        enabled,
        id,
    }
}

static FILE: [Entry; 4] = [
    item("&New", "Mod+N", 1, true),
    item("&Open...", "Mod+O", 2, true),
    Entry::Separator,
    item("&Quit", "Mod+Q", 3, true),
];
static EDIT: [Entry; 2] = [
    item("&Undo", "Mod+Z", 10, false),
    item("Cu&t", "Mod+X", 11, true),
];
static VIEW: [Entry; 1] = [item("&Wrap", "", 20, true)];
static FORMAT: [Entry; 2] = [item("Bold", "", 1, true), item("Wrap", "", 2, true)];

struct MenuFixture {
    tree: WidgetTree<16>,
    bar: WidgetId,
    field: WidgetId,
}

fn menu_fixture() -> MenuFixture {
    let mut tree = WidgetTree::<16>::new();
    let root = surface(&mut tree);
    let bar = tree
        .add(
            Some(root),
            Place::at(0, 0, Length::Fill(0), Length::Content),
            Kind::menu_bar(&[("&File", &FILE), ("&Edit", &EDIT), ("&View", &VIEW)]),
        )
        .unwrap();
    let field = tree
        .add(
            Some(root),
            Place::at(16, 60, Length::Px(200), Length::Px(28)),
            Kind::text_field("", Escape::Clear),
        )
        .unwrap();
    tree.resize(WIDTH, HEIGHT);
    tree.set_focus(Some(field), 0);
    MenuFixture { tree, bar, field }
}

#[test]
fn the_menu_bar_opens_by_a_press_switches_by_hover_and_chooses_by_a_release() {
    let mut fixture = menu_fixture();
    let tree = &mut fixture.tree;
    let pixels_closed = pixels(tree);
    assert_eq!(at(&pixels_closed, 10, 27), DARK.menu_border);
    assert_eq!(at(&pixels_closed, 10, 20), DARK.menu);
    assert_eq!(button(tree, 20, 14, true, 0), Response::Nothing);
    let open = pixels(tree);
    assert_eq!(at(&open, 10, 20), DARK.menu_hover);
    assert_eq!(at(&open, 2, 60), DARK.menu);
    let listed = described(tree);
    let bar = listed
        .iter()
        .find(|element| element.2 == Role::MenuBar)
        .unwrap();
    assert_ne!(bar.3 & flags::EXPANDED, 0);
    assert!(
        listed.iter().any(|element| element.2 == Role::MenuItem
            && element.4 == "New"
            && element.5 == "Mod+N")
    );
    assert_eq!(button(tree, 20, 14, false, 10), Response::Nothing);
    moved(tree, 60, 14, 20);
    let cut = 29 + 1 + 32 + 16;
    moved(tree, 40, cut, 30);
    assert_eq!(button(tree, 40, cut, true, 40), Response::Nothing);
    assert_eq!(
        button(tree, 40, cut, false, 50),
        Response::Invoked(fixture.bar, 11)
    );
    assert_eq!(at(&pixels(tree), 10, 20), DARK.menu);
    button(tree, 20, 14, true, 60);
    button(tree, 20, 14, false, 70);
    assert_eq!(button(tree, 300, 200, true, 80), Response::Nothing);
    assert_eq!(button(tree, 300, 200, false, 90), Response::Nothing);
    assert_eq!(at(&pixels(tree), 10, 20), DARK.menu);
}

#[test]
fn f10_arms_the_bar_and_the_arrows_enter_and_mnemonics_drive_it() {
    let mut fixture = menu_fixture();
    let tree = &mut fixture.tree;
    assert_eq!(key(tree, key::F10), Response::Nothing);
    assert_eq!(at(&pixels(tree), 2, 20), DARK.menu_hover);
    assert_ne!(
        described(tree)
            .iter()
            .find(|element| element.0 == crate::tree::element_of(fixture.field))
            .unwrap()
            .3
            & flags::FOCUSED,
        0
    );
    key(tree, key::RIGHT);
    key(tree, key::RIGHT);
    key(tree, key::RIGHT);
    key(tree, key::DOWN);
    key(tree, key::DOWN);
    key(tree, key::DOWN);
    key(tree, key::RIGHT);
    assert_eq!(key(tree, key::ENTER), Response::Invoked(fixture.bar, 11));
    assert_eq!(typed(tree, key::A, 'a'), Response::Edited(fixture.field));
    tree.handle(
        Input::Key {
            code: key::LEFT_ALT,
            pressed: true,
            modifiers: Modifiers(Modifiers::ALT),
            character: 0,
        },
        0,
    );
    assert_eq!(
        key_with(tree, key::F, 'f' as u32, Modifiers::ALT),
        Response::Nothing
    );
    assert_eq!(
        key_with(tree, key::Q, 'q' as u32, Modifiers::ALT),
        Response::Invoked(fixture.bar, 3)
    );
    key(tree, key::F10);
    assert_eq!(typed(tree, key::Z, 'z'), Response::Nothing);
    key(tree, key::ESC);
    assert_eq!(typed(tree, key::A, 'a'), Response::Edited(fixture.field));
}

#[test]
fn in_the_macos_profile_the_bar_is_the_main_menu_and_takes_no_room() {
    let mut fixture = menu_fixture();
    let tree = &mut fixture.tree;
    let mut pool = [0u8; POOL_CAPACITY];
    let length = tree.main_menu(&mut pool).unwrap();
    assert_eq!(
        core::str::from_utf8(&pool[..length]).unwrap(),
        "MFile\nINew\tMod+N\nIOpen...\tMod+O\n-\nIQuit\tMod+Q\nMEdit\nDUndo\tMod+Z\nICut\tMod+X\nMView\nIWrap\t\n"
    );
    tree.set_global_menu(true);
    assert_eq!(tree.rect(fixture.bar).unwrap().height, 0);
    assert_eq!(tree.menu_chosen(1, 1), Response::Invoked(fixture.bar, 11));
    assert_eq!(tree.menu_chosen(1, 0), Response::Nothing);
    assert_eq!(with_role(&described(tree), Role::MenuBar).len(), 1);
    tree.set_global_menu(false);
    assert_eq!(tree.rect(fixture.bar).unwrap().height, 28);
}

#[test]
fn a_menu_button_opens_below_and_chooses_by_keys_and_by_an_agent() {
    let mut tree = WidgetTree::<8>::new();
    let root = surface(&mut tree);
    let format = tree
        .add(
            Some(root),
            Place::at(16, 100, Length::Content, Length::Content),
            Kind::menu_button("Format", &FORMAT),
        )
        .unwrap();
    let next = tree
        .add(
            Some(root),
            Place::at(200, 100, Length::Content, Length::Content),
            Kind::button("Next", false),
        )
        .unwrap();
    tree.resize(WIDTH, HEIGHT);
    assert_eq!(tree.rect(format), Some(Rect::new(16, 100, 96, 28)));
    assert_eq!(at(&pixels(&tree), 90, 111), DARK.text);
    assert_eq!(button(&mut tree, 40, 114, true, 0), Response::Nothing);
    assert_eq!(at(&pixels(&tree), 20, 114), DARK.button_pressed);
    key(&mut tree, key::DOWN);
    key(&mut tree, key::DOWN);
    assert_eq!(key(&mut tree, key::ENTER), Response::Invoked(format, 2));
    key(&mut tree, key::SPACE);
    assert_eq!(key(&mut tree, key::ESC), Response::Nothing);
    key(&mut tree, key::SPACE);
    key(&mut tree, key::TAB);
    assert_eq!(tree.focus(), Some(next));
    let entry = crate::tree::element_of(format) + 1;
    assert_eq!(
        tree.act(entry, Action::Invoke, "", 0),
        (Outcome::Done, Response::Invoked(format, 1))
    );
}

fn files_cell(_data: u32, row: usize, column: usize, out: &mut Text<64>) {
    let _ = match column {
        0 => core::fmt::Write::write_fmt(out, format_args!("Row {row}")),
        1 => core::fmt::Write::write_fmt(out, format_args!("{row} KB")),
        _ => core::fmt::Write::write_str(out, "2026-10-31"),
    };
}

const FILES_COLUMNS: [Column; 3] = [
    Column::new("Name", 200, Align::Start),
    Column::new("Size", 80, Align::End),
    Column::new("Modified", 160, Align::Start),
];

fn table_fixture(rows: usize) -> (WidgetTree<8>, WidgetId) {
    table_with_cells(rows, files_cell)
}

static COUNTED: AtomicUsize = AtomicUsize::new(0);

fn counted_cell(data: u32, row: usize, column: usize, out: &mut Text<64>) {
    COUNTED.fetch_add(1, Ordering::Relaxed);
    files_cell(data, row, column, out);
}

fn table_with_cells(rows: usize, cell: crate::widget::CellText) -> (WidgetTree<8>, WidgetId) {
    let mut tree = WidgetTree::<8>::new();
    let root = surface(&mut tree);
    let table = tree
        .add(
            Some(root),
            Place::at(16, 40, Length::Px(400), Length::Px(220)),
            Kind::table("Files", &FILES_COLUMNS, rows, cell, 0),
        )
        .unwrap();
    tree.resize(WIDTH, HEIGHT);
    (tree, table)
}

fn table_of(tree: &WidgetTree<8>, id: WidgetId) -> crate::table::Table {
    match tree.kind(id) {
        Some(Kind::Table(table)) => *table,
        _ => panic!("not a table"),
    }
}

#[test]
fn a_table_draws_its_header_rows_and_both_bars_and_lists_its_shown_rows() {
    let (tree, table) = table_fixture(1000);
    let pixels = pixels(&tree);
    assert_eq!(at(&pixels, 100, 67), DARK.menu_border);
    assert_eq!(at(&pixels, 215, 50), DARK.menu_border);
    assert_eq!(at(&pixels, 215, 44), DARK.menu);
    assert_eq!(at(&pixels, 100, 80), DARK.selection);
    assert_eq!(at(&pixels, 410, 70), DARK.bar_pressed);
    assert_eq!(at(&pixels, 100, 255), DARK.bar_pressed);
    assert_eq!(at(&pixels, 370, 255), DARK.progress_track);
    let listed = described(&tree);
    let element = listed
        .iter()
        .find(|element| element.0 == crate::tree::element_of(table))
        .unwrap();
    assert_eq!(
        (element.2, element.4.as_str(), element.5.as_str()),
        (Role::Table, "Files", "row 1 of 1000")
    );
    assert_eq!(with_role(&listed, Role::ColumnHeader).len(), 3);
    let rows = with_role(&listed, Role::Row);
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[0].4, "Row 0");
    assert_eq!(rows[0].5, "Row 0\t0 KB\t2026-10-31");
    assert_ne!(rows[0].3 & flags::SELECTED, 0);
}

#[test]
fn a_table_selects_by_clicks_and_keys_and_opens_by_a_double_click_or_enter() {
    let (mut tree, table) = table_fixture(1000);
    assert_eq!(click(&mut tree, 100, 100, 0).0, Response::Chosen(table, 1));
    assert_eq!(key(&mut tree, key::DOWN), Response::Chosen(table, 2));
    assert_eq!(key(&mut tree, key::END), Response::Chosen(table, 999));
    assert_eq!(table_of(&tree, table).offset_y, 1000 * 24 - 184);
    assert_eq!(key(&mut tree, key::PAGE_UP), Response::Chosen(table, 992));
    assert_eq!(key(&mut tree, key::HOME), Response::Chosen(table, 0));
    assert_eq!(table_of(&tree, table).offset_y, 0);
    assert_eq!(
        click(&mut tree, 100, 100, 5000).0,
        Response::Chosen(table, 1)
    );
    assert_eq!(
        click(&mut tree, 100, 100, 5100).0,
        Response::Opened(table, 1)
    );
    assert_eq!(key(&mut tree, key::ENTER), Response::Opened(table, 1));
}

#[test]
fn a_header_click_sorts_ascending_then_flips_and_draws_the_arrow() {
    let (mut tree, table) = table_fixture(1000);
    assert_eq!(
        click(&mut tree, 100, 50, 0).1,
        Response::Sorted(table, 0, SortOrder::Ascending)
    );
    moved(&mut tree, 470, 350, 0);
    let up = pixels(&tree);
    assert_eq!(at(&up, 203, 51), DARK.accent);
    assert_eq!(at(&up, 203, 56), DARK.menu);
    assert_eq!(at(&up, 199, 55), DARK.accent);
    assert_eq!(
        click(&mut tree, 100, 50, 1000).1,
        Response::Sorted(table, 0, SortOrder::Descending)
    );
    moved(&mut tree, 470, 350, 1000);
    let down = pixels(&tree);
    assert_eq!(at(&down, 203, 56), DARK.accent);
    assert_eq!(at(&down, 203, 51), DARK.menu);
    assert_eq!(
        click(&mut tree, 250, 50, 2000).1,
        Response::Sorted(table, 1, SortOrder::Ascending)
    );
    moved(&mut tree, 470, 350, 2000);
    assert_eq!(at(&pixels(&tree), 228, 51), DARK.accent);
    let headers = with_role(&described(&tree), Role::ColumnHeader);
    assert_eq!(headers[1].5, "ascending");
    tree.set_sort(table, None);
    assert_eq!(table_of(&tree, table).sort, None);
}

#[test]
fn a_column_resizes_by_its_handle_down_to_its_minimum_and_esc_puts_it_back() {
    let (mut tree, table) = table_fixture(1000);
    moved(&mut tree, 214, 50, 0);
    assert_eq!(tree.pointer_shape(), PointerShape::ResizeEW);
    button(&mut tree, 215, 50, true, 0);
    assert_eq!(moved(&mut tree, 255, 50, 0), Response::Nothing);
    assert_eq!(table_of(&tree, table).columns[0].width, 240);
    assert_eq!(
        button(&mut tree, 255, 50, false, 0),
        Response::ColumnResized(table, 0, 240)
    );
    button(&mut tree, 255, 50, true, 0);
    moved(&mut tree, 0, 50, 0);
    assert_eq!(
        button(&mut tree, 0, 50, false, 0),
        Response::ColumnResized(table, 0, 40)
    );
    button(&mut tree, 55, 50, true, 0);
    moved(&mut tree, 100, 50, 0);
    assert_eq!(key(&mut tree, key::ESC), Response::Nothing);
    assert_eq!(table_of(&tree, table).columns[0].width, 40);
    assert_eq!(button(&mut tree, 100, 50, false, 0), Response::Nothing);
    let header = crate::tree::element_of(table) + 1;
    assert_eq!(
        tree.act(header, Action::SetText, "120", 0),
        (Outcome::Done, Response::ColumnResized(table, 0, 120))
    );
    assert_eq!(
        tree.act(header, Action::SetText, "x", 0).0,
        Outcome::NotSupported
    );
}

#[test]
fn a_table_scrolls_both_ways_by_the_wheel_shift_and_the_sideways_wheel() {
    let (mut tree, table) = table_fixture(1000);
    tree.handle(
        Input::Wheel {
            x: 100,
            y: 150,
            notches: -1,
        },
        0,
    );
    assert_eq!(table_of(&tree, table).offset_y, 72);
    tree.handle(
        Input::Key {
            code: key::LEFT_SHIFT,
            pressed: true,
            modifiers: Modifiers(Modifiers::SHIFT),
            character: 0,
        },
        0,
    );
    tree.handle(
        Input::Wheel {
            x: 100,
            y: 150,
            notches: -1,
        },
        0,
    );
    assert_eq!(table_of(&tree, table).offset_x, 48);
    tree.handle(
        Input::Key {
            code: key::LEFT_SHIFT,
            pressed: false,
            modifiers: Modifiers(0),
            character: 0,
        },
        0,
    );
    tree.handle(
        Input::SidewaysWheel {
            x: 100,
            y: 150,
            notches: 1,
        },
        0,
    );
    assert_eq!(table_of(&tree, table).offset_x, 48);
    tree.handle(
        Input::SidewaysWheel {
            x: 100,
            y: 150,
            notches: -1,
        },
        0,
    );
    assert_eq!(table_of(&tree, table).offset_x, 0);
    click(&mut tree, 100, 100, 0);
    key(&mut tree, key::RIGHT);
    assert_eq!(table_of(&tree, table).offset_x, 48);
    key(&mut tree, key::LEFT);
    assert_eq!(table_of(&tree, table).offset_x, 0);
}

#[test]
fn an_agent_selects_any_row_of_a_table_and_an_empty_one_says_so() {
    let (mut tree, table) = table_fixture(1000);
    let element = crate::tree::element_of(table);
    assert_eq!(
        tree.act(element, Action::SetText, "500", 0),
        (Outcome::Done, Response::Chosen(table, 499))
    );
    let shown = with_role(&described(&tree), Role::Row);
    assert!(shown.iter().any(|row| row.4 == "Row 499"));
    tree.set_rows(table, 0);
    let listed = described(&tree);
    assert_eq!(with_role(&listed, Role::Row).len(), 0);
    assert!(listed.iter().any(|element| element.5 == "0 rows"));
}

#[test]
fn a_table_asks_for_the_cells_of_the_rows_it_shows_only() {
    for rows in [1000, 10_000] {
        let (mut tree, table) = table_with_cells(rows, counted_cell);
        tree.set_scroll(table, 0, 24);
        let mut pixels = vec![0u32; 400 * 300];
        COUNTED.store(0, Ordering::Relaxed);
        tree.render(&mut pixels, 400, 300, 1);
        assert_eq!(COUNTED.load(Ordering::Relaxed), 24);
    }
}

const PANGRAM: &str = "the quick brown fox jumps over the lazy dog";

fn area_fixture() -> (WidgetTree<16, 1>, WidgetId) {
    let mut tree = WidgetTree::<16, 1>::new();
    let root = surface(&mut tree);
    let area = tree
        .add(
            Some(root),
            Place::at(16, 40, Length::Px(200), Length::Px(92)),
            Kind::text_area(0, "Describe it"),
        )
        .unwrap();
    tree.resize(WIDTH, HEIGHT);
    click(&mut tree, 100, 50, 0);
    (tree, area)
}

fn area_of(tree: &WidgetTree<16, 1>, id: WidgetId) -> crate::text_area::TextArea {
    match tree.kind(id) {
        Some(Kind::TextArea(area)) => *area,
        _ => panic!("not a text area"),
    }
}

#[test]
fn a_text_area_wraps_at_words_and_moves_by_the_lines_it_shows() {
    let (mut tree, area) = area_fixture();
    tree.set_text(area, PANGRAM);
    assert_eq!(area_of(&tree, area).lines, 2);
    assert_eq!(area_of(&tree, area).caret, 43);
    key(&mut tree, key::HOME);
    assert_eq!(area_of(&tree, area).caret, 31);
    key(&mut tree, key::UP);
    assert_eq!(area_of(&tree, area).caret, 0);
    key(&mut tree, key::END);
    assert_eq!(area_of(&tree, area).caret, 30);
    key(&mut tree, key::DOWN);
    assert_eq!(area_of(&tree, area).caret, 43);
    key(&mut tree, key::UP);
    assert_eq!(area_of(&tree, area).caret, 30);
    key(&mut tree, key::LEFT);
    assert_eq!(area_of(&tree, area).caret, 29);
    assert_eq!(area_of(&tree, area).wanted_x, None);
    key_with(&mut tree, key::END, 0, Modifiers::CTRL);
    assert_eq!(area_of(&tree, area).caret, 43);
    key_with(&mut tree, key::HOME, 0, Modifiers::CTRL);
    assert_eq!(area_of(&tree, area).caret, 0);
}

#[test]
fn enter_makes_a_line_and_the_bar_appears_without_changing_the_wrap() {
    let (mut tree, area) = area_fixture();
    tree.set_text(area, PANGRAM);
    assert_eq!(key(&mut tree, key::ENTER), Response::Edited(area));
    assert_eq!(area_of(&tree, area).lines, 3);
    assert_eq!(typed(&mut tree, key::A, 'a'), Response::Edited(area));
    key(&mut tree, key::ENTER);
    key(&mut tree, key::ENTER);
    key(&mut tree, key::ENTER);
    assert_eq!(area_of(&tree, area).lines, 6);
    assert_eq!(area_of(&tree, area).scroll, 40);
    assert_eq!(at(&pixels(&tree), 210, 60), DARK.progress_track);
    tree.handle(
        Input::Wheel {
            x: 100,
            y: 60,
            notches: 1,
        },
        0,
    );
    assert_eq!(area_of(&tree, area).scroll, 0);
    assert_eq!(tree.text_of(area).map(str::len), Some(PANGRAM.len() + 5));
}

#[test]
fn a_word_wider_than_the_line_breaks_between_characters_and_the_store_has_a_limit() {
    let (mut tree, area) = area_fixture();
    tree.set_text(
        area,
        "supercalifragilisticexpialidocioussupercalifragilistic again",
    );
    assert_eq!(area_of(&tree, area).lines, 2);
    let long = "x".repeat(70_000);
    tree.set_text(area, &long);
    assert_eq!(tree.text_of(area).map(str::len), Some(65_536));
}

#[test]
fn a_text_area_selects_copies_cuts_and_pastes_and_keeps_line_breaks() {
    let (mut tree, area) = area_fixture();
    tree.set_text(area, PANGRAM);
    click(&mut tree, 60, 50, 1000);
    assert_eq!(area_of(&tree, area).caret, 7);
    tree.handle(
        Input::Key {
            code: key::LEFT_SHIFT,
            pressed: true,
            modifiers: Modifiers(Modifiers::SHIFT),
            character: 0,
        },
        0,
    );
    click(&mut tree, 40, 70, 3000);
    tree.handle(
        Input::Key {
            code: key::LEFT_SHIFT,
            pressed: false,
            modifiers: Modifiers(0),
            character: 0,
        },
        0,
    );
    assert_eq!(area_of(&tree, area).caret, 34);
    assert_eq!(area_of(&tree, area).anchor, 7);
    assert_eq!(
        key_with(&mut tree, key::C, 'c' as u32, Modifiers::CTRL),
        Response::Copied(area)
    );
    assert_eq!(tree.copied(), &PANGRAM[7..34]);
    assert_eq!(
        key_with(&mut tree, key::X, 'x' as u32, Modifiers::CTRL),
        Response::Cut(area)
    );
    assert_eq!(
        key_with(&mut tree, key::V, 'v' as u32, Modifiers::CTRL),
        Response::PasteWanted(area)
    );
    tree.set_text(area, "");
    assert_eq!(tree.paste(area, "a\tb\r\nc", 0), Response::Edited(area));
    assert_eq!(tree.text_of(area), Some("a\tb\nc"));
}

#[test]
fn tab_leaves_esc_goes_to_the_window_and_mod_enter_reaches_the_default_button() {
    let (mut tree, area) = area_fixture();
    let ok = tree
        .add(
            None,
            Place::at(300, 300, Length::Content, Length::Content),
            Kind::button("OK", true),
        )
        .unwrap();
    tree.set_default_button(ok);
    tree.resize(WIDTH, HEIGHT);
    assert_eq!(
        key(&mut tree, key::ESC),
        Response::Unused {
            code: key::ESC,
            shift: false
        }
    );
    assert_eq!(key(&mut tree, key::ENTER), Response::Edited(area));
    assert_eq!(
        key_with(&mut tree, key::ENTER, 13, Modifiers::CTRL),
        Response::Activated(ok)
    );
    let element = crate::tree::element_of(area);
    assert_eq!(
        tree.act(element, Action::SetText, "hello", 0),
        (Outcome::Done, Response::Edited(area))
    );
    let listed = described(&tree);
    let shown = listed
        .iter()
        .find(|element| element.2 == Role::TextArea)
        .unwrap();
    assert_eq!(
        (shown.4.as_str(), shown.5.as_str()),
        ("Describe it", "hello")
    );
    tree.set_text(area, &"y".repeat(300));
    let shown = with_role(&described(&tree), Role::TextArea);
    assert_eq!(shown[0].5.len(), 255);
    assert!(shown[0].5.ends_with("..."));
    key(&mut tree, key::TAB);
    assert_eq!(tree.focus(), Some(ok));
    let mut without = WidgetTree::<16>::new();
    assert_eq!(
        without.add(None, Place::fill(), Kind::text_area(0, "")),
        None
    );
}

#[test]
fn a_scroll_view_scrolls_sideways_with_its_own_bar_and_the_wheel_goes_to_the_innermost() {
    static ROWS: [&str; 32] = ["row"; 32];
    let mut tree = WidgetTree::<8>::new();
    let view = tree
        .add(
            None,
            Place::at(0, 0, Length::Px(320), Length::Px(200)),
            Kind::scroll_view(),
        )
        .unwrap();
    let wide = tree
        .add(
            Some(view),
            Place::at(0, 0, Length::Px(600), Length::Px(40)),
            Kind::panel(Token::Menu),
        )
        .unwrap();
    let list = tree
        .add(
            Some(view),
            Place::at(0, 60, Length::Px(320), Length::Px(120)),
            Kind::list(&ROWS, 24, 8, 24),
        )
        .unwrap();
    tree.add(
        Some(view),
        Place::at(0, 360, Length::Px(10), Length::Px(40)),
        Kind::panel(Token::Menu),
    )
    .unwrap();
    tree.resize(WIDTH, HEIGHT);
    let pixels = pixels(&tree);
    assert_eq!(at(&pixels, 314, 50), DARK.bar_pressed);
    assert_eq!(at(&pixels, 50, 196), DARK.bar_pressed);
    tree.handle(
        Input::SidewaysWheel {
            x: 100,
            y: 20,
            notches: 1,
        },
        0,
    );
    assert_eq!(tree.scroll(view), Some((72, 0)));
    assert_eq!(tree.rect(wide).unwrap().x, -72);
    tree.handle(
        Input::SidewaysWheel {
            x: 100,
            y: 20,
            notches: 10,
        },
        0,
    );
    assert_eq!(tree.scroll(view), Some((288, 0)));
    tree.set_scroll(view, 0, 0);
    button(&mut tree, 100, 196, true, 0);
    moved(&mut tree, 200, 196, 0);
    button(&mut tree, 200, 196, false, 0);
    assert_eq!(tree.scroll(view), Some((192, 0)));
    tree.set_scroll(view, 0, 0);
    tree.handle(
        Input::SidewaysWheel {
            x: 100,
            y: 100,
            notches: 1,
        },
        0,
    );
    assert_eq!(tree.scroll(view), Some((72, 0)));
    tree.handle(
        Input::Wheel {
            x: 100,
            y: 100,
            notches: -1,
        },
        0,
    );
    assert_eq!(tree.scroll(list), Some((0, 72)));
    assert_eq!(tree.scroll(view), Some((72, 0)));
}

static ORDERED: [&str; 5] = ["alpha", "beta", "gamma", "delta", "epsilon"];
static PLACES: [&str; 2] = ["Places", "Trash"];

fn drag_fixture() -> (WidgetTree<8>, WidgetId, WidgetId) {
    let mut tree = WidgetTree::<8>::new();
    let root = surface(&mut tree);
    let list = tree
        .add(
            Some(root),
            Place::at(16, 40, Length::Px(200), Length::Px(120)),
            Kind::list(&ORDERED, 24, 8, 24),
        )
        .unwrap();
    let places = tree
        .add(
            Some(root),
            Place::at(240, 40, Length::Px(200), Length::Px(120)),
            Kind::list(&PLACES, 24, 8, 24),
        )
        .unwrap();
    tree.set_drag_source(list, true);
    tree.set_drop_target(list, Some(DropMode::Between));
    tree.set_drop_target(places, Some(DropMode::Onto));
    tree.resize(WIDTH, HEIGHT);
    (tree, list, places)
}

#[test]
fn a_row_dragged_past_the_threshold_shows_a_ghost_and_drops_between_rows() {
    let (mut tree, list, _) = drag_fixture();
    button(&mut tree, 100, 76, true, 0);
    assert_eq!(moved(&mut tree, 103, 78, 0), Response::Nothing);
    assert_eq!(
        button(&mut tree, 103, 78, false, 0),
        Response::Chosen(list, 1)
    );
    button(&mut tree, 100, 76, true, 1000);
    assert_eq!(
        moved(&mut tree, 100, 83, 1000),
        Response::DragStarted(list, 1)
    );
    assert_eq!(tree.pointer_shape(), PointerShape::Hand);
    assert_eq!(at(&pixels(&tree), 112, 107), DARK.menu_border);
    moved(&mut tree, 100, 130, 1000);
    assert_eq!(at(&pixels(&tree), 100, 136), DARK.accent);
    moved(&mut tree, 100, 116, 1000);
    assert_eq!(at(&pixels(&tree), 100, 112), DARK.accent);
    assert_eq!(
        button(&mut tree, 100, 116, false, 1000),
        Response::Dropped {
            source: list,
            part: 1,
            target: list,
            at: 3
        }
    );
    assert_eq!(tree.pointer_shape(), PointerShape::Arrow);
}

#[test]
fn a_row_drops_onto_another_list_and_every_other_ending_cancels() {
    let (mut tree, list, places) = drag_fixture();
    button(&mut tree, 100, 76, true, 0);
    moved(&mut tree, 100, 90, 0);
    moved(&mut tree, 300, 52, 0);
    assert_eq!(at(&pixels(&tree), 240, 52), DARK.accent);
    assert_eq!(
        button(&mut tree, 300, 52, false, 0),
        Response::Dropped {
            source: list,
            part: 1,
            target: places,
            at: 0
        }
    );
    button(&mut tree, 100, 76, true, 0);
    moved(&mut tree, 100, 90, 0);
    moved(&mut tree, 300, 110, 0);
    assert_eq!(
        button(&mut tree, 300, 110, false, 0),
        Response::DragCancelled(list)
    );
    button(&mut tree, 100, 76, true, 0);
    moved(&mut tree, 100, 90, 0);
    assert_eq!(key(&mut tree, key::ESC), Response::DragCancelled(list));
    button(&mut tree, 100, 76, true, 0);
    moved(&mut tree, 100, 90, 0);
    assert_eq!(
        tree.handle(Input::FocusOut, 0),
        Response::DragCancelled(list)
    );
    tree.handle(Input::FocusIn, 0);
    button(&mut tree, 100, 76, true, 0);
    assert_eq!(moved(&mut tree, 100, 90, 0), Response::DragStarted(list, 1));
    tree.cancel_drag();
    assert_eq!(button(&mut tree, 100, 90, false, 0), Response::Nothing);
}

#[test]
fn on_a_phone_a_long_press_picks_a_row_up_with_its_ghost_above_the_finger() {
    let (mut tree, list, _) = drag_fixture();
    tree.set_form_factor(FormFactor::Phone);
    button(&mut tree, 100, 76, true, 0);
    assert_eq!(tree.tick(500), Response::DragStarted(list, 1));
    assert_eq!(at(&pixels(&tree), 112, 50), DARK.menu_border);
}

fn wcag(first: u32, second: u32) -> f64 {
    let luminance = |pixel: u32| {
        let channel = |shift: u32| {
            let value = f64::from((pixel >> shift) & 0xff) / 255.0;
            match value <= 0.039_28 {
                true => value / 12.92,
                false => ((value + 0.055) / 1.055).powf(2.4),
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    };
    let (a, b) = (luminance(first), luminance(second));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn the_light_palette_s_pairs_have_the_contrast_the_design_states() {
    let pairs: [(u32, u32, f64); 18] = [
        (LIGHT.text, LIGHT.menu, 16.0),
        (LIGHT.text_dim, LIGHT.menu, 5.4),
        (LIGHT.text, LIGHT.field, 17.3),
        (LIGHT.text, LIGHT.selection, 12.9),
        (LIGHT.accent, LIGHT.menu, 4.7),
        (LIGHT.accent, LIGHT.button, 5.1),
        (LIGHT.accent, LIGHT.selection, 3.8),
        (LIGHT.text_dim, LIGHT.field, 5.8),
        (LIGHT.text, LIGHT.menu_hover, 14.0),
        (LIGHT.text_dim, LIGHT.menu_hover, 4.7),
        (LIGHT.term_foreground, LIGHT.term_background, 14.9),
        (LIGHT.icon_dark, LIGHT.button_primary, 10.4),
        (LIGHT.text, LIGHT.bar, 14.7),
        (LIGHT.text_dim, LIGHT.bar, 5.0),
        (LIGHT.text, LIGHT.bar_hover, 13.3),
        (LIGHT.text_dim, LIGHT.title_inactive, 5.1),
        (LIGHT.icon_badge, LIGHT.icon_windows, 4.9),
        (LIGHT.text, LIGHT.button_hover, 15.5),
    ];
    for (ink, ground, stated) in pairs {
        let ratio = (wcag(ink, ground) * 10.0).round() / 10.0;
        assert_eq!(ratio, stated, "{ink:06x} on {ground:06x}");
    }
    for palette in [&LIGHT_HIGH_CONTRAST, &theme::DARK_HIGH_CONTRAST] {
        assert!(wcag(palette.text, palette.menu) >= 7.0);
        assert!(wcag(palette.text_dim, palette.menu) >= 7.0);
    }
}

fn mixed_window(palette: &'static Palette) -> Vec<u32> {
    let mut tree = WidgetTree::<16>::new();
    let root = surface(&mut tree);
    tree.add(
        Some(root),
        Place::at(16, 16, Length::Content, Length::Content),
        Kind::label("Label", Token::Text),
    );
    let save = tree
        .add(
            Some(root),
            Place::at(16, 60, Length::Px(96), Length::Px(28)),
            Kind::button("Save", false),
        )
        .unwrap();
    tree.add(Some(save), Place::fill(), Kind::tooltip("Save", "Mod+S"));
    tree.add(
        Some(root),
        Place::at(16, 140, Length::Px(200), Length::Px(28)),
        Kind::text_field("", Escape::Clear),
    );
    tree.add(
        Some(root),
        Place::at(16, 200, Length::Px(216), Length::Px(28)),
        Kind::slider("Speed", 1, 10, 1, 5, ""),
    );
    tree.resize(WIDTH, HEIGHT);
    tree.set_palette(palette);
    moved(&mut tree, 60, 70, 0);
    tree.tick(600);
    pixels(&tree)
}

#[test]
fn the_light_palette_repaints_every_widget_and_the_dark_frame_is_unchanged() {
    let light = mixed_window(&LIGHT);
    assert_eq!(at(&light, 300, 300), LIGHT.menu);
    assert_eq!(at(&light, 20, 74), LIGHT.button_hover);
    assert_eq!(at(&light, 16, 74), LIGHT.button_border);
    assert_eq!(at(&light, 100, 154), LIGHT.field);
    assert_eq!(at(&light, 20, 214), LIGHT.accent);
    assert_eq!(at(&light, 88, 214), LIGHT.button_primary);
    assert_eq!(at(&light, 22, 100), LIGHT.menu_border);
    assert_eq!(at(&light, 26, 100), LIGHT.menu);
    let dark = mixed_window(&DARK);
    assert_eq!(at(&dark, 300, 300), crate::colour::MENU);
    assert_eq!(at(&dark, 100, 154), crate::colour::FIELD);
    assert_eq!(at(&dark, 20, 214), crate::colour::ACCENT);
}

#[test]
fn set_place_moves_a_widget_and_what_it_holds_and_draws_both_places_again() {
    let mut tree = WidgetTree::<4>::new();
    let root = surface(&mut tree);
    let bottom = tree
        .add(
            Some(root),
            Place::at(0, 300, Length::Fill(0), Length::Px(40)),
            Kind::panel(Token::Menu),
        )
        .unwrap();
    let button = tree
        .add(
            Some(bottom),
            Place::at(16, 6, Length::Px(96), Length::Px(28)),
            Kind::button("Clear", false),
        )
        .unwrap();
    tree.resize(WIDTH, HEIGHT);
    tree.take_damage();
    tree.set_place(bottom, Place::at(0, 200, Length::Fill(0), Length::Px(40)));
    assert_eq!(tree.rect(bottom), Some(Rect::new(0, 200, WIDTH, 40)));
    assert_eq!(tree.rect(button), Some(Rect::new(16, 206, 96, 28)));
    let damage = tree.take_damage();
    let covers = |x: i32, y: i32| damage.rects().iter().any(|rect| rect.contains(x, y));
    assert!(covers(20, 310));
    assert!(covers(20, 210));
    tree.set_place(bottom, Place::at(0, 200, Length::Fill(0), Length::Px(40)));
    assert!(tree.take_damage().is_empty());
}
