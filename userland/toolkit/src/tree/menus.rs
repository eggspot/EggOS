//! The menu bar and the menu button (docs/ux/toolkit-v0.5.md section 3): opening, moving,
//! choosing and closing by pointer and keys, and the bar as the window's main menu in the
//! macOS profile.

use super::{Kind, Response, WidgetId, WidgetTree};
use crate::canvas::Canvas;
use crate::geometry::Rect;
use crate::input::key;
use crate::menu::{Entry, Menu, MenuKey, POOL_CAPACITY};

impl<const N: usize, const A: usize> WidgetTree<N, A> {
    /// The window's menu bar: the first shown one.
    pub fn menu_bar(&self) -> Option<WidgetId> {
        self.ids()
            .find(|&id| matches!(self.kind(id), Some(Kind::MenuBar(_))) && self.is_shown(id))
    }

    /// Whether the menu bar is the window's main menu in the top bar (the macOS profile): it is
    /// then not drawn and takes no room, and [`Self::menu_chosen`] answers its items.
    pub fn set_global_menu(&mut self, global: bool) {
        if self.global_menu == global {
            return;
        }
        self.close_menu();
        self.global_menu = global;
        self.layout();
        self.damage_all();
    }

    /// Writes the menu bar as `SET_MENU`'s pool into `out`: its length, `None` without a bar.
    pub fn main_menu(&self, out: &mut [u8; POOL_CAPACITY]) -> Option<usize> {
        match self.kind(self.menu_bar()?) {
            Some(Kind::MenuBar(bar)) => Some(bar.pool(out)),
            _ => None,
        }
    }

    /// The top bar's `MenuChosen` (menu `menu`, entry `entry`, separators counted) as the menu
    /// bar's choice: [`Response::Invoked`] for an enabled item.
    pub fn menu_chosen(&mut self, menu: usize, entry: usize) -> Response {
        let Some(id) = self.menu_bar() else {
            return Response::Nothing;
        };
        let Some(Kind::MenuBar(bar)) = self.kind(id).copied() else {
            return Response::Nothing;
        };
        if menu >= bar.count || !self.is_enabled(id) {
            return Response::Nothing;
        }
        match bar.menus[menu].get(entry) {
            Some(Entry::Item {
                enabled: true, id: item, ..
            }) => {
                self.close_menu();
                Response::Invoked(id, *item)
            }
            _ => Response::Nothing,
        }
    }

    pub(super) fn menu_open(&self) -> bool {
        self.open_menu().is_some()
    }

    pub(super) fn bar_has_keyboard(&self) -> bool {
        self.menu
            .and_then(|id| self.kind(id))
            .is_some_and(|kind| matches!(kind, Kind::MenuBar(bar) if bar.has_keyboard()))
    }

    pub(super) fn menu_keyboard(&self) -> Option<WidgetId> {
        self.menu.filter(|_| self.bar_has_keyboard())
    }

    fn window_rect(&self) -> Rect {
        Rect::new(0, 0, self.size.0, self.size.1)
    }

    pub(super) fn open_menu(&self) -> Option<(WidgetId, Menu)> {
        let id = self.menu?;
        let rect = self.rect(id)?;
        let window = self.window_rect();
        match self.kind(id)? {
            Kind::MenuBar(bar) => Some((id, bar.menu(rect, window)?)),
            Kind::MenuButton(button) if button.open => Some((id, button.menu(rect, window))),
            _ => None,
        }
    }

    fn damage_menu(&mut self) {
        if let Some((id, menu)) = self.open_menu() {
            self.damage.add(menu.rect());
            self.damage_widget(id);
        } else if let Some(id) = self.menu {
            self.damage_widget(id);
        }
    }

    fn change_menu(&mut self, id: WidgetId, change: impl FnOnce(&mut Kind)) {
        self.damage_menu();
        if let Some(node) = self.node_mut(id) {
            change(&mut node.kind);
        }
        self.damage_menu();
    }

    /// Closes the open menu and disarms the bar.
    pub(super) fn close_menu(&mut self) {
        let Some(id) = self.menu else {
            return;
        };
        self.change_menu(id, |kind| match kind {
            Kind::MenuBar(bar) => {
                bar.open = None;
                bar.highlighted = None;
                bar.mnemonics = false;
            }
            Kind::MenuButton(button) => {
                button.open = false;
                button.highlighted = None;
            }
            _ => {}
        });
        self.menu = None;
        self.menu_pressed = false;
    }

    fn open_title(&mut self, id: WidgetId, title: usize, first: bool) {
        self.close_popup();
        self.hide_tooltip();
        if self.menu != Some(id) {
            self.close_menu();
        }
        self.menu = Some(id);
        let window = self.window_rect();
        let rect = self.rect(id).unwrap_or_default();
        self.change_menu(id, |kind| {
            if let Kind::MenuBar(bar) = kind {
                bar.open = Some(title);
                bar.highlighted = None;
                if first {
                    bar.highlighted = bar.menu(rect, window).and_then(|menu| menu.first_enabled());
                }
            }
        });
    }

    pub(super) fn bar_title_pressed(&mut self, id: WidgetId, title: usize) -> Response {
        self.open_title(id, title, false);
        self.menu_pressed = true;
        Response::Nothing
    }

    pub(super) fn open_button_menu(&mut self, id: WidgetId) {
        if self.menu == Some(id) {
            self.close_menu();
            return;
        }
        self.close_popup();
        self.close_menu();
        self.hide_tooltip();
        self.menu = Some(id);
        self.change_menu(id, |kind| {
            if let Kind::MenuButton(button) = kind {
                button.open = true;
                button.highlighted = None;
            }
        });
    }

    /// Arms the menu bar: its first title highlighted, the keyboard on it, its mnemonics shown.
    pub(super) fn arm_menu_bar(&mut self) {
        if self.global_menu {
            return;
        }
        let Some(id) = self.menu_bar().filter(|&id| self.is_enabled(id)) else {
            return;
        };
        self.close_popup();
        self.close_menu();
        self.hide_tooltip();
        self.menu = Some(id);
        self.change_menu(id, |kind| {
            if let Kind::MenuBar(bar) = kind {
                bar.open = None;
                bar.highlighted = Some(0);
                bar.mnemonics = true;
            }
        });
        if let Some(focus) = self.focus {
            self.damage_widget(focus);
        }
    }

    fn set_highlighted(&mut self, id: WidgetId, highlighted: Option<usize>) {
        self.change_menu(id, |kind| match kind {
            Kind::MenuBar(bar) => bar.highlighted = highlighted,
            Kind::MenuButton(button) => button.highlighted = highlighted,
            _ => {}
        });
    }

    pub(super) fn menu_pointer_moved(&mut self, x: i32, y: i32) {
        let Some((id, menu)) = self.open_menu() else {
            return;
        };
        if let (Some(Kind::MenuBar(bar)), Some(rect)) = (self.kind(id).copied(), self.rect(id))
            && let Some(title) = bar.title_at(rect, x, y)
        {
            if bar.open != Some(title) {
                self.open_title(id, title, false);
            }
            return;
        }
        let item = menu.item_at(x, y);
        if menu.rect().contains(x, y) && item != menu.chosen() {
            self.set_highlighted(id, item);
        }
    }

    pub(super) fn menu_pressed_at(&mut self, x: i32, y: i32) -> Response {
        let Some((id, menu)) = self.open_menu() else {
            return Response::Nothing;
        };
        if menu.rect().contains(x, y) {
            self.menu_pressed = true;
            self.set_highlighted(id, menu.item_at(x, y));
            return Response::Nothing;
        }
        if let (Some(Kind::MenuBar(bar)), Some(rect)) = (self.kind(id).copied(), self.rect(id))
            && let Some(title) = bar.title_at(rect, x, y)
        {
            match bar.open == Some(title) {
                true => self.close_menu(),
                false => {
                    self.open_title(id, title, false);
                    self.menu_pressed = true;
                }
            }
            return Response::Nothing;
        }
        self.close_menu();
        if self.rect(id).is_some_and(|rect| rect.contains(x, y)) {
            self.pressed = None;
        }
        Response::Nothing
    }

    pub(super) fn menu_released_at(&mut self, x: i32, y: i32) -> Response {
        self.menu_pressed = false;
        let Some((id, menu)) = self.open_menu() else {
            return Response::Nothing;
        };
        match menu.item_at(x, y) {
            Some(index) => self.choose_entry(id, &menu, index),
            None => Response::Nothing,
        }
    }

    fn choose_entry(&mut self, id: WidgetId, menu: &Menu, index: usize) -> Response {
        let Some(item) = menu.enabled(index) else {
            return Response::Nothing;
        };
        self.close_menu();
        Response::Invoked(id, item)
    }

    fn switch_title(&mut self, id: WidgetId, forward: bool) {
        let Some(Kind::MenuBar(bar)) = self.kind(id).copied() else {
            return;
        };
        let count = bar.count.max(1);
        let here = bar.open.or(bar.highlighted).unwrap_or(0);
        let next = match forward {
            true => (here + 1) % count,
            false => (here + count - 1) % count,
        };
        match bar.open {
            Some(_) => {
                self.open_title(id, next, true);
                self.set_mnemonics(id);
            }
            None => self.set_highlighted(id, Some(next)),
        }
    }

    fn set_mnemonics(&mut self, id: WidgetId) {
        self.change_menu(id, |kind| {
            if let Kind::MenuBar(bar) = kind {
                bar.mnemonics = true;
            }
        });
    }

    fn open_by_mnemonic(&mut self, id: WidgetId, letter: char) -> bool {
        let Some(Kind::MenuBar(bar)) = self.kind(id).copied() else {
            return false;
        };
        let Some(title) = bar.title_with_mnemonic(letter) else {
            return false;
        };
        self.open_title(id, title, true);
        self.set_mnemonics(id);
        true
    }

    /// What key `code` typing `character` does to the menus: `None` when they do not take it.
    pub(super) fn menu_key(&mut self, code: u16, character: u32, _now_ms: u32) -> Option<Response> {
        let letter = char::from_u32(character).filter(char::is_ascii_alphanumeric);
        let button_open = self
            .menu
            .is_some_and(|id| matches!(self.kind(id), Some(Kind::MenuButton(_))));
        if matches!(code, key::F10 | key::MENU) && !button_open && !self.global_menu {
            match self.bar_has_keyboard() {
                true => self.close_menu(),
                false => self.arm_menu_bar(),
            }
            return Some(Response::Nothing);
        }
        if self.alt_held
            && !button_open
            && !self.global_menu
            && let Some(letter) = letter
            && let Some(bar) = self.menu_bar().filter(|&id| self.is_enabled(id))
            && self.open_by_mnemonic(bar, letter)
        {
            return Some(Response::Nothing);
        }
        let id = self.menu?;
        match self.kind(id).copied()? {
            Kind::MenuBar(bar) if bar.has_keyboard() => Some(self.bar_key(id, bar, code, letter)),
            Kind::MenuButton(button) if button.open => self.button_menu_key(id, code),
            _ => None,
        }
    }

    fn bar_key(
        &mut self,
        id: WidgetId,
        bar: crate::menu::Bar,
        code: u16,
        letter: Option<char>,
    ) -> Response {
        let Some(open) = bar.open else {
            match code {
                key::LEFT => self.switch_title(id, false),
                key::RIGHT => self.switch_title(id, true),
                key::DOWN | key::ENTER | key::SPACE => {
                    self.open_title(id, bar.highlighted.unwrap_or(0), true);
                    self.set_mnemonics(id);
                }
                key::ESC => self.close_menu(),
                _ => {
                    if let Some(letter) = letter {
                        self.open_by_mnemonic(id, letter);
                    }
                }
            }
            return Response::Nothing;
        };
        let Some((_, mut menu)) = self.open_menu() else {
            return Response::Nothing;
        };
        match code {
            key::LEFT => self.switch_title(id, false),
            key::RIGHT => self.switch_title(id, true),
            key::ESC => {
                self.change_menu(id, |kind| {
                    if let Kind::MenuBar(bar) = kind {
                        bar.open = None;
                        bar.highlighted = Some(open);
                        bar.mnemonics = true;
                    }
                });
            }
            key::ENTER | key::SPACE => {
                if let Some(index) = menu.chosen() {
                    return self.choose_entry(id, &menu, index);
                }
            }
            key::UP | key::DOWN | key::HOME | key::END => {
                menu.key(code);
                self.set_highlighted(id, menu.chosen());
            }
            _ => {
                if let Some(letter) = letter {
                    let (next, count) = menu.with_mnemonic(letter);
                    match (next, count) {
                        (Some(index), 1) => return self.choose_entry(id, &menu, index),
                        (Some(index), _) => self.set_highlighted(id, Some(index)),
                        _ => {}
                    }
                }
            }
        }
        Response::Nothing
    }

    fn button_menu_key(&mut self, id: WidgetId, code: u16) -> Option<Response> {
        let (_, mut menu) = self.open_menu()?;
        match code {
            key::TAB => {
                self.close_menu();
                None
            }
            key::ESC => {
                self.close_menu();
                Some(Response::Nothing)
            }
            key::ENTER | key::SPACE => Some(match menu.chosen() {
                Some(index) => self.choose_entry(id, &menu, index),
                None => Response::Nothing,
            }),
            key::UP | key::DOWN | key::HOME | key::END => {
                if menu.key(code) == MenuKey::Nothing {
                    self.set_highlighted(id, menu.chosen());
                }
                Some(Response::Nothing)
            }
            _ => Some(Response::Nothing),
        }
    }

    /// A key on a focused, closed menu button.
    pub(super) fn menu_button_key(&mut self, id: WidgetId, code: u16) -> Response {
        let opens = matches!(code, key::SPACE | key::ENTER) || code == key::DOWN && self.alt_held;
        match opens {
            true => {
                self.open_button_menu(id);
                Response::Nothing
            }
            false => Response::Unused {
                code,
                shift: self.shift_held,
            },
        }
    }

    /// Chooses entry `index` of the menu of menu bar or menu button `id`, by an agent: the
    /// bar's menu is `menu`.
    pub(super) fn invoke_entry(&mut self, id: WidgetId, menu: usize, index: usize) -> Response {
        let entries: &[Entry] = match self.kind(id) {
            Some(Kind::MenuBar(bar)) if menu < bar.count => bar.menus[menu],
            Some(Kind::MenuButton(button)) => button.entries,
            _ => return Response::Nothing,
        };
        match entries.get(index) {
            Some(Entry::Item {
                enabled: true, id: item, ..
            }) => {
                let item = *item;
                if self.menu == Some(id) {
                    self.close_menu();
                }
                Response::Invoked(id, item)
            }
            _ => Response::Nothing,
        }
    }

    /// Opens or closes the menu of menu bar title `title` or of menu button `id`, by an agent.
    pub(super) fn toggle_menu(&mut self, id: WidgetId, title: usize) {
        match self.kind(id).copied() {
            Some(Kind::MenuBar(bar)) if bar.open == Some(title) && self.menu == Some(id) => {
                self.close_menu()
            }
            Some(Kind::MenuBar(_)) => self.open_title(id, title, false),
            Some(Kind::MenuButton(_)) => self.open_button_menu(id),
            _ => {}
        }
    }

    pub(super) fn paint_menu(&self, canvas: &mut Canvas<'_>, clip: Rect) {
        if let Some((_, menu)) = self.open_menu() {
            let previous = canvas.narrow(menu.rect().intersection(&clip));
            menu.paint(canvas);
            canvas.restore(previous);
        }
    }
}
