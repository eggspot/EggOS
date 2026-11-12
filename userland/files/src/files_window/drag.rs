//! Dragging files (D395, docs/ux/files-drag-v0.5.md): a row picked up past 6 pixels, the
//! ghost under the pointer, the folder row, the list's own folder or the sidebar place it
//! would land on highlighted, a resting pointer springing into a folder after 1000 ms, and
//! the release moving, copying, linking or trashing as the modifiers held say; a drag that
//! leaves the window handed to the window server, and drags from other windows landing on the
//! same targets.

use super::*;

/// How long the pointer rests on a folder before the window browses into it.
pub const DRAG_SPRING_MS: u32 = 1000;
/// The longest name the ghost shows before it is cut.
pub const DRAG_WORDS_MAX_GLYPHS: usize = 30;
const DRAG_THRESHOLD: i32 = 6;
const GHOST_OFFSET: i32 = 12;
const GHOST_HEIGHT: i32 = 24;
const GHOST_PADDING: i32 = 8;
const OUTLINE: i32 = 2;

/// Where a drag would land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Spot {
    /// The folder row at this index.
    Row(usize),
    /// The folder shown.
    Whole,
    /// The sidebar place at this index.
    Place(usize),
}

/// The window's own drag.
#[derive(Debug, Clone, Copy)]
pub(super) struct Picked {
    path: Path,
    from: (i32, i32),
    at: (i32, i32),
    started: bool,
    spot: Option<Spot>,
    spot_since: u32,
}

/// A drag from another window over this one.
#[derive(Debug, Clone, Copy)]
pub(super) struct Arriving {
    serial: u32,
    path: Option<Path>,
    count: usize,
    spot: Option<Spot>,
    spot_since: u32,
}

/// What a drop does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropAction {
    Move,
    Copy,
    Link,
    Trash,
}

/// The action the modifiers held at the release ask for (section 3): Mod with Shift or Alt
/// links, Ctrl or Alt copies, Shift or Super moves, nothing moves on one file system and
/// copies across; a drop on the Trash trashes whatever is held.
pub fn drop_action(modifiers: Modifiers, same_file_system: bool, onto_trash: bool) -> DropAction {
    if onto_trash {
        return DropAction::Trash;
    }
    let held = modifiers.0;
    let ctrl = held & Modifiers::CTRL != 0;
    let super_key = held & Modifiers::SUPER != 0;
    let alt = held & Modifiers::ALT != 0;
    let shift = held & Modifiers::SHIFT != 0;
    if (ctrl || super_key) && (shift || alt) {
        DropAction::Link
    } else if ctrl || alt {
        DropAction::Copy
    } else if shift || super_key {
        DropAction::Move
    } else if same_file_system {
        DropAction::Move
    } else {
        DropAction::Copy
    }
}

/// The ghost's words for `count` items, the first named `name`: the name, cut at 30 glyphs
/// to 27 and `...`, or `<N> items`.
pub fn drag_words(name: &[u8], count: usize) -> Name {
    let mut words = Name::default();
    if count >= 2 {
        let _ = write!(words, "{count} items");
        return words;
    }
    let text = core::str::from_utf8(name).unwrap_or("");
    match text.chars().count() > DRAG_WORDS_MAX_GLYPHS {
        true => {
            let end = text.char_indices().nth(DRAG_WORDS_MAX_GLYPHS - 3).map_or(text.len(), |(at, _)| at);
            let _ = write!(words, "{}...", &text[..end]);
        }
        false => {
            let _ = words.write_str(text);
        }
    }
    words
}

impl FilesWindow {
    pub(super) fn pick_row(&mut self, index: usize, x: i32, y: i32) {
        if self.rename.is_some() || self.panel.is_some() || self.running.is_some() {
            return;
        }
        let Some(path) = self.entry_path(index) else {
            return;
        };
        self.picked = Some(Picked {
            path,
            from: (x, y),
            at: (x, y),
            started: false,
            spot: None,
            spot_since: self.now_ms,
        });
    }

    fn folder_of_spot(&self, spot: Spot) -> Option<Location> {
        match spot {
            Spot::Row(index) => self.entry_path(index).map(Location::Folder),
            Spot::Whole => self.folder().copied().map(Location::Folder),
            Spot::Place(index) => self.places().get(index).map(|place| place.location()),
        }
    }

    fn droppable_row(&self, files: &mut impl Files, index: usize, dragged: Option<&Path>) -> bool {
        let Some(path) = self.entry_path(index) else {
            return false;
        };
        if dragged.is_some_and(|dragged| is_within(path.as_bytes(), dragged.as_bytes())) {
            return false;
        }
        !is_within(path.as_bytes(), HOST_SHARE)
            && files
                .status_following(path.as_bytes())
                .is_ok_and(|status| status.kind == files::Kind::Directory)
    }

    fn spot_at(&self, files: &mut impl Files, x: i32, y: i32, dragged: Option<&Path>) -> Option<Spot> {
        if let Some(index) = self.place_at(x, y) {
            let place = *self.places().get(index)?;
            let from_share = dragged.is_some_and(|path| is_within(path.as_bytes(), HOST_SHARE));
            return match (place, place.location()) {
                (Place::HostShare, _) => None,
                (Place::Trash, _) if from_share || self.is_trash() => None,
                (_, Location::Folder(folder))
                    if dragged.is_some_and(|path| parent(path.as_bytes()) == folder.as_bytes()) =>
                {
                    None
                }
                _ => Some(Spot::Place(index)),
            };
        }
        if !self.rows().contains(x, y) {
            return None;
        }
        if let Some(index) = self.row_at(x, y)
            && self.droppable_row(files, index, dragged)
            && dragged.is_none_or(|path| self.entry_path(index).is_none_or(|row| row != *path))
        {
            return Some(Spot::Row(index));
        }
        let folder = self.folder()?;
        let own = dragged.is_some_and(|path| parent(path.as_bytes()) == folder.as_bytes());
        let blocked = own || self.is_trash() || self.on_share();
        (!blocked).then_some(Spot::Whole)
    }

    fn damage_drag(&mut self) {
        self.damage_middle();
        self.damage.add(self.sidebar());
    }

    /// The pointer moved during a press on a row: past the threshold the drag starts; outside
    /// the window it is handed to the server; inside, the landing spot follows. Whether the
    /// window's drag took the move.
    pub(super) fn drag_moved(&mut self, files: &mut impl Files, x: i32, y: i32) -> bool {
        let Some(mut picked) = self.picked else {
            return false;
        };
        picked.at = (x, y);
        if !picked.started {
            if (x - picked.from.0).abs() + (y - picked.from.1).abs() < DRAG_THRESHOLD {
                self.picked = Some(picked);
                return false;
            }
            picked.started = true;
            self.menu = None;
        }
        let (width, height) = (self.width, self.height);
        if x < 0 || y < 0 || x >= width || y >= height {
            let words = drag_words(picked.path.file_name(), 1);
            self.picked = None;
            self.ask(Request::CarryOut(picked.path, words));
            self.damage_drag();
            return true;
        }
        let spot = self.spot_at(files, x, y, Some(&picked.path));
        if spot != picked.spot {
            picked.spot = spot;
            picked.spot_since = self.now_ms;
        }
        self.picked = Some(picked);
        self.damage_drag();
        true
    }

    /// The left button went up: a started drag drops where it would land, with `modifiers`.
    /// Whether the window's drag took the release.
    pub(super) fn drag_released(&mut self, files: &mut impl Files, modifiers: Modifiers) -> bool {
        let Some(picked) = self.picked.take() else {
            return false;
        };
        if !picked.started {
            return false;
        }
        self.damage_drag();
        if let Some(spot) = picked.spot {
            self.drop_paths(files, &[picked.path], spot, modifiers);
        }
        true
    }

    /// Esc during a drag: it ends with nothing done. Whether one was on.
    pub(super) fn drag_cancelled(&mut self) -> bool {
        let started = self.picked.take().is_some_and(|picked| picked.started);
        let arriving = self.arriving.take().is_some();
        if started || arriving {
            self.damage_drag();
        }
        started
    }

    /// The pointer resting on a folder springs into it (section 5).
    pub(super) fn drag_tick(&mut self, files: &mut impl Files) {
        let resting = match (self.picked, self.arriving) {
            (Some(picked), _) if picked.started => picked.spot.map(|spot| (spot, picked.spot_since)),
            (_, Some(arriving)) => arriving.spot.map(|spot| (spot, arriving.spot_since)),
            _ => None,
        };
        let Some((spot, since)) = resting else {
            return;
        };
        if self.now_ms.wrapping_sub(since) < DRAG_SPRING_MS || spot == Spot::Whole {
            return;
        }
        let springs = match spot {
            Spot::Place(index) => !matches!(self.places().get(index), Some(Place::Trash | Place::HostShare)),
            _ => true,
        };
        if !springs {
            return;
        }
        let Some(location) = self.folder_of_spot(spot) else {
            return;
        };
        self.go(files, location);
        if let Some(picked) = self.picked.as_mut() {
            picked.spot = None;
            picked.spot_since = self.now_ms;
        }
        if let Some(arriving) = self.arriving.as_mut() {
            arriving.spot = None;
            arriving.spot_since = self.now_ms;
        }
    }

    /// When the window next wants a tick for a resting drag.
    pub(super) fn drag_deadline(&self) -> Option<u32> {
        let since = match (self.picked, self.arriving) {
            (Some(picked), _) if picked.started && picked.spot.is_some() => picked.spot_since,
            (_, Some(arriving)) if arriving.spot.is_some() => arriving.spot_since,
            _ => return None,
        };
        Some(since.wrapping_add(DRAG_SPRING_MS))
    }

    /// A drag from another window is over this one at (`x`, `y`): `paths` its payload's paths
    /// (none for text), read once per `serial` by the program.
    pub fn drag_over(&mut self, files: &mut impl Files, x: i32, y: i32, serial: u32, paths: &[Path]) {
        let first = paths.first().copied();
        let spot = match first {
            Some(path) => self.spot_at(files, x, y, Some(&path)),
            None => None,
        };
        let since = match self.arriving {
            Some(arriving) if arriving.serial == serial && arriving.spot == spot => arriving.spot_since,
            _ => self.now_ms,
        };
        self.arriving = Some(Arriving {
            serial,
            path: first,
            count: paths.len(),
            spot,
            spot_since: since,
        });
        self.pointer = Some((x, y));
        self.damage_drag();
    }

    /// The drag from another window left this one.
    pub fn drag_left(&mut self) {
        if self.arriving.take().is_some() {
            self.damage_drag();
        }
    }

    /// The drag from another window was let go here at (`x`, `y`) with `modifiers` held.
    pub fn drag_dropped(&mut self, files: &mut impl Files, x: i32, y: i32, paths: &[Path], modifiers: Modifiers) {
        self.arriving = None;
        self.damage_drag();
        let Some(first) = paths.first() else {
            return;
        };
        if let Some(spot) = self.spot_at(files, x, y, Some(first)) {
            self.drop_paths(files, paths, spot, modifiers);
        }
    }

    /// The window's own drag that left it ended; `landed` on a window, so the folder shown may
    /// have changed: it reloads now (and, as always, when the window next gains focus).
    pub fn carried_ended(&mut self, files: &mut impl Files, landed: bool) {
        if landed {
            self.reload(files);
        }
    }

    fn drop_paths(&mut self, files: &mut impl Files, paths: &[Path], spot: Spot, modifiers: Modifiers) {
        let Some(location) = self.folder_of_spot(spot) else {
            return;
        };
        for path in paths {
            let done = match location {
                Location::Trash => self.drop_into_trash(files, path),
                Location::Folder(folder) => self.drop_into(files, path, folder, modifiers),
            };
            if !done {
                break;
            }
        }
        self.reload(files);
    }

    fn drop_into_trash(&mut self, files: &mut impl Files, path: &Path) -> bool {
        match self.trash.put(files, path.as_bytes(), self.clock) {
            Ok(_) => true,
            Err(error) => {
                self.show_notice(format_args!(
                    "Could not delete {} ({}).",
                    Shown(path.file_name()),
                    errno_text(error)
                ));
                false
            }
        }
    }

    fn drop_into(&mut self, files: &mut impl Files, path: &Path, folder: Path, modifiers: Modifiers) -> bool {
        if is_within(folder.as_bytes(), HOST_SHARE) {
            self.show_notice(format_args!("The host share is read-only."));
            return false;
        }
        let name = path.file_name();
        let is_folder = files
            .status(path.as_bytes())
            .is_ok_and(|status| status.kind == files::Kind::Directory);
        if is_folder && is_within(folder.as_bytes(), path.as_bytes()) {
            self.show_notice(format_args!("Cannot move a folder into itself."));
            return false;
        }
        let same_file_system = match (files.status(path.as_bytes()), files.status(folder.as_bytes())) {
            (Ok(from), Ok(into)) => from.device == into.device,
            _ => false,
        };
        let from_trash = self.trash.holds(path.as_bytes());
        let action = match from_trash {
            true => DropAction::Move,
            false => drop_action(modifiers, same_file_system, false),
        };
        if action == DropAction::Link {
            let made = free_path(files, folder.as_bytes(), |number| link_name(name, is_folder, number))
                .and_then(|link| files.make_link(path.as_bytes(), link.as_bytes()));
            if let Err(error) = made {
                self.show_notice(format_args!(
                    "Could not make a link to {} ({}).",
                    Shown(name),
                    errno_text(error)
                ));
                return false;
            }
            return true;
        }
        let target = match free_path(files, folder.as_bytes(), |number| copy_name(name, is_folder, number)) {
            Ok(target) => target,
            Err(error) => {
                self.show_notice(format_args!("Could not copy {} ({}).", Shown(name), errno_text(error)));
                return false;
            }
        };
        let moving = action == DropAction::Move;
        if moving && same_file_system && files.rename(path.as_bytes(), target.as_bytes()).is_ok() {
            if from_trash {
                self.trash.forget(files, path.file_name());
            }
            return true;
        }
        let job = match moving {
            true => Job::moving(path, &target),
            false => Job::copy(path, &target),
        };
        let after = After::Paste {
            folder,
            name: Name::from(target.file_name()).unwrap_or_default(),
            cut: moving,
        };
        self.start(files, job, after);
        false
    }

    fn ghost_words(&self) -> Option<(Name, (i32, i32))> {
        match (self.picked, self.arriving) {
            (Some(picked), _) if picked.started => Some((drag_words(picked.path.file_name(), 1), picked.at)),
            _ => None,
        }
    }

    fn spot_rect(&self, spot: Spot) -> Rect {
        match spot {
            Spot::Row(index) => self.row_rect(index),
            Spot::Whole => self.rows(),
            Spot::Place(index) => self.place_rect(index),
        }
    }

    /// Draws the landing spot's highlight and the ghost, above the list and the sidebar.
    pub(super) fn paint_drag(&self, canvas: &mut Canvas<'_>) {
        let spot = match (self.picked, self.arriving) {
            (Some(picked), _) if picked.started => picked.spot,
            (_, Some(arriving)) => arriving.spot,
            _ => None,
        };
        if let Some(spot) = spot {
            let rect = self.spot_rect(spot);
            if spot != Spot::Whole {
                canvas.fill(rect, colour::MENU_HOVER);
            }
            for edge in [
                Rect::new(rect.x, rect.y, rect.width, OUTLINE),
                Rect::new(rect.x, rect.bottom() - OUTLINE, rect.width, OUTLINE),
                Rect::new(rect.x, rect.y, OUTLINE, rect.height),
                Rect::new(rect.right() - OUTLINE, rect.y, OUTLINE, rect.height),
            ] {
                canvas.fill(edge, colour::ACCENT);
            }
        }
        if let Some((words, (x, y))) = self.ghost_words() {
            let shown = core::str::from_utf8(words.as_bytes()).unwrap_or("");
            let width = 2 * GHOST_PADDING + eggos_toolkit::text::width(shown);
            let ghost = Rect::new(x + GHOST_OFFSET, y + GHOST_OFFSET, width, GHOST_HEIGHT);
            eggos_toolkit::tooltip::paint_tile(canvas, ghost, shown, "");
        }
    }

    /// Whether a drag is on, so the pointer is the hand.
    pub fn dragging(&self) -> bool {
        self.picked.is_some_and(|picked| picked.started) || self.arriving.is_some()
    }

    /// The modifiers held at the last key the window saw, which a drop from another window
    /// is read with.
    pub fn held_modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// How many paths the drag from another window carries.
    pub fn arriving_count(&self) -> usize {
        self.arriving.map_or(0, |arriving| arriving.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers(0);

    #[test]
    fn the_modifiers_at_the_release_choose_the_action() {
        assert_eq!(drop_action(NONE, true, false), DropAction::Move);
        assert_eq!(drop_action(NONE, false, false), DropAction::Copy);
        assert_eq!(drop_action(Modifiers(Modifiers::CTRL), true, false), DropAction::Copy);
        assert_eq!(drop_action(Modifiers(Modifiers::ALT), true, false), DropAction::Copy);
        assert_eq!(drop_action(Modifiers(Modifiers::SHIFT), false, false), DropAction::Move);
        assert_eq!(drop_action(Modifiers(Modifiers::SUPER), false, false), DropAction::Move);
        let link = Modifiers(Modifiers::CTRL | Modifiers::SHIFT);
        assert_eq!(drop_action(link, true, false), DropAction::Link);
        let finder_link = Modifiers(Modifiers::SUPER | Modifiers::ALT);
        assert_eq!(drop_action(finder_link, true, false), DropAction::Link);
        assert_eq!(drop_action(link, true, true), DropAction::Trash);
    }

    #[test]
    fn the_ghost_names_one_item_or_counts_many() {
        assert_eq!(drag_words(b"notes.txt", 1).as_bytes(), b"notes.txt");
        assert_eq!(drag_words(b"notes.txt", 3).as_bytes(), b"3 items");
        let long = drag_words(b"a-name-that-is-much-longer-than-thirty-glyphs.txt", 1);
        assert_eq!(long.as_bytes().len(), 30);
        assert!(long.as_bytes().ends_with(b"..."));
    }
}
