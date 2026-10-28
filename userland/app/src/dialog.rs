//! The installer's window (docs/ux/desktop-v0.3-gui.md section 9, as D134 settles it), built
//! with the toolkit: the chooser of a folder's programs, the prompt, the progress, the result
//! and the uninstall confirmation, each a fresh widget tree in one 480 by 240 window. Kept
//! apart from the system calls so host tests, and the QEMU test's expected frames, draw the
//! same window the program shows.

use crate::messages::Line;
use eggos_app_manifest::Source;
use eggos_toolkit::canvas::{Canvas, INSET_R4, INSET_R6};
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::key;
use eggos_toolkit::text;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Kind, Length, PROGRESS_DONE, Place};

/// The window's content size at scale 1.
pub const SIZE: (u32, u32) = (480, 240);
/// The widgets one screen of the window holds at most: Settings' capacity, so the program
/// that serves both windows instantiates one widget tree type.
pub const CAPACITY: usize = 48;
/// The window's title while installing.
pub const INSTALL_TITLE: &str = "Install";
/// The window's title while uninstalling.
pub const UNINSTALL_TITLE: &str = "Uninstall";
/// The most programs the chooser lists.
pub const MAX_CHOICES: usize = 4;

const PADDING: i32 = 16;
const ICON: i32 = 48;
const TEXT_X: i32 = 80;
const LINE_PITCH: i32 = 20;
const LINE_HEIGHT: i32 = 16;
const BUTTON_GAP: i32 = 8;
const MAX_BUTTONS: usize = 6;
const MAX_LINES: usize = 8;
const MAX_LABELS: usize = 10;
const CHOICE_TOP: i32 = 44;
const CHOICE_PITCH: i32 = 32;
const PROGRESS_TOP: i32 = 48;
const STATUS_TOP: i32 = 64;
const TEXT_WIDTH: i32 = SIZE.0 as i32 - TEXT_X - PADDING;
const LABEL_BYTES: usize = 64;
const BADGE: Rect = Rect::new(30, 26, 16, 20);

/// What a button of the window asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Install the program the prompt shows.
    Install,
    /// Stop: before the install, or while it copies.
    Cancel,
    /// Close the window.
    Close,
    /// Open the app just installed.
    Open,
    /// Uninstall the app the confirmation names.
    Uninstall,
    /// The chooser's program at this index.
    Choose(usize),
}

/// The tile the window shows: the app's letter and its system's badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppIcon {
    pub letter: u8,
    pub source: Option<Source>,
}

impl AppIcon {
    /// The icon of an app called `name` from `source`: its first letter or digit, upper-cased,
    /// else `?`, as the desktop draws it (D133).
    pub fn of(name: &str, source: Option<Source>) -> Self {
        let letter = match name.as_bytes().first() {
            Some(&first) if first.is_ascii_alphanumeric() => first.to_ascii_uppercase(),
            _ => b'?',
        };
        Self { letter, source }
    }

    fn data(self) -> u32 {
        let source = match self.source {
            None => 0,
            Some(Source::Windows) => 1,
            Some(Source::Macos) => 2,
            Some(Source::Linux) => 3,
        };
        u32::from(self.letter) | source << 8
    }
}

fn badge(source: u32) -> Option<(u32, &'static str, u32)> {
    match source {
        1 => Some((colour::ICON_WINDOWS, "W", colour::TEXT)),
        2 => Some((colour::ICON_MAC, "M", colour::ICON_DARK)),
        3 => Some((colour::ICON_LINUX, "L", colour::ICON_DARK)),
        _ => None,
    }
}

fn paint_icon(canvas: &mut Canvas<'_>, rect: Rect, data: u32) {
    canvas.rounded(rect, &INSET_R6, colour::ICON_TILE);
    canvas.outline(rect, &INSET_R6, 1, colour::ICON_TILE_BORDER);
    let letter = [(data & 0xff) as u8];
    let letter = core::str::from_utf8(&letter).unwrap_or("?");
    canvas.text(rect.x + 8, rect.y + 8, letter, colour::TEXT, 2);
    if let Some((fill, mark, ink)) = badge(data >> 8) {
        let area = BADGE.offset(rect.x, rect.y);
        canvas.rounded(area, &INSET_R4, fill);
        let x = area.x + (area.width - text::width(mark)) / 2;
        canvas.text(x, area.y + 2, mark, ink, 1);
    }
}

/// Splits `words` into lines that fit `width` pixels and a label's 64 bytes, breaking at
/// spaces where it can; at most `lines.len()` lines, the last cut.
pub fn wrap<'a>(words: &'a str, width: i32, lines: &mut [&'a str]) -> usize {
    let mut rest = words.trim_start();
    let mut count = 0;
    while !rest.is_empty() && count < lines.len() {
        let (mut end, _) = text::fit(rest, width);
        end = end.min(LABEL_BYTES);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        if end < rest.len() {
            if let Some(space) = rest[..end].rfind(' ').filter(|&space| space > 0) {
                end = space;
            } else if end == 0 {
                end = rest.chars().next().map_or(rest.len(), char::len_utf8);
            }
        }
        lines[count] = rest[..end].trim_end();
        count += 1;
        rest = rest[end..].trim_start();
    }
    count
}

/// One screen of the installer's window. See the module documentation.
#[derive(Debug, Clone)]
pub struct Dialog {
    tree: WidgetTree<CAPACITY>,
    root: Option<WidgetId>,
    buttons: [Option<(WidgetId, Request)>; MAX_BUTTONS],
    escape: Request,
    progress: Option<WidgetId>,
    status: Option<WidgetId>,
    labels: [Option<WidgetId>; MAX_LABELS],
    lines: usize,
}

impl Dialog {
    fn new(escape: Request) -> Self {
        let mut tree = WidgetTree::new();
        let root = tree.add(None, Place::fill(), Kind::panel(colour::WINDOW_BACKGROUND));
        Self {
            tree,
            root,
            buttons: [None; MAX_BUTTONS],
            escape,
            progress: None,
            status: None,
            labels: [None; MAX_LABELS],
            lines: 0,
        }
    }

    fn icon(&mut self, icon: AppIcon) {
        self.tree.add(
            self.root,
            Place::at(PADDING, PADDING, Length::Px(ICON), Length::Px(ICON)),
            Kind::Custom {
                painter: paint_icon,
                data: icon.data(),
            },
        );
    }

    fn line(&mut self, words: &str, ink: u32) -> Option<WidgetId> {
        if self.lines >= MAX_LINES {
            return None;
        }
        let top = PADDING + self.lines as i32 * LINE_PITCH;
        self.lines += 1;
        let id = self.tree.add(
            self.root,
            Place::at(TEXT_X, top, Length::Fill(PADDING), Length::Px(LINE_HEIGHT)),
            Kind::label(words, ink),
        );
        self.keep_label(id)
    }

    fn wrapped(&mut self, words: &str, ink: u32, most: usize) {
        let mut lines = [""; MAX_LINES];
        let count = wrap(words, TEXT_WIDTH, &mut lines[..most.min(MAX_LINES)]);
        for line in &lines[..count] {
            self.line(line, ink);
        }
    }

    fn add_button(&mut self, id: Option<WidgetId>, request: Request) {
        if let (Some(id), Some(slot)) = (id, self.buttons.iter_mut().find(|slot| slot.is_none())) {
            *slot = Some((id, request));
        }
    }

    fn bottom_buttons(&mut self, buttons: &[(&str, Request)]) {
        let top = SIZE.1 as i32 - PADDING - CONTROL_HEIGHT;
        let mut margin = PADDING;
        for (index, (label, request)) in buttons.iter().enumerate().rev() {
            let primary = index == buttons.len() - 1;
            let kind = Kind::button(label, primary);
            let width = kind.content_size().0;
            let id = self.tree.add(self.root, Place::right(margin, top), kind);
            margin += width + BUTTON_GAP;
            self.add_button(id, *request);
            if primary && let Some(id) = id {
                self.tree.set_default_button(id);
                self.tree.set_focus(Some(id), 0);
            }
        }
    }

    fn finish(mut self) -> Self {
        self.tree.resize(SIZE.0 as i32, SIZE.1 as i32);
        self
    }

    /// The chooser: the programs `names` (at most [`MAX_CHOICES`]; `more` others not shown)
    /// found in `folder`.
    pub fn choose(folder: &str, names: &[&str], more: usize) -> Self {
        let mut dialog = Self::new(Request::Cancel);
        dialog.icon(AppIcon::of("?", None));
        let mut heading = Line::new();
        let _ = core::fmt::Write::write_fmt(
            &mut heading,
            format_args!("Install a program from {folder}"),
        );
        dialog.line(heading.as_str(), colour::TEXT);
        match names.is_empty() {
            true => {
                dialog.line("It holds no program EggOS can run.", colour::TEXT_DIM);
            }
            false => {
                for (index, name) in names.iter().take(MAX_CHOICES).enumerate() {
                    let id = dialog.tree.add(
                        dialog.root,
                        Place::at(
                            TEXT_X,
                            CHOICE_TOP + index as i32 * CHOICE_PITCH,
                            Length::Fill(PADDING),
                            Length::Px(CONTROL_HEIGHT),
                        ),
                        Kind::button(name, false),
                    );
                    dialog.add_button(id, Request::Choose(index));
                }
            }
        }
        if more > 0 {
            let mut line = Line::new();
            let _ = core::fmt::Write::write_fmt(
                &mut line,
                format_args!("and {more} more: app install <path> installs any"),
            );
            let id = dialog.tree.add(
                dialog.root,
                Place::at(
                    TEXT_X,
                    CHOICE_TOP + MAX_CHOICES as i32 * CHOICE_PITCH,
                    Length::Fill(PADDING),
                    Length::Px(LINE_HEIGHT),
                ),
                Kind::label(line.as_str(), colour::TEXT_DIM),
            );
            dialog.keep_label(id);
        }
        dialog.bottom_buttons(&[("Cancel", Request::Cancel)]);
        if let Some((id, _)) = dialog.buttons[0].filter(|_| !names.is_empty()) {
            dialog.tree.set_focus(Some(id), 0);
        }
        dialog.finish()
    }

    /// The prompt: `Install <name>?` over `details` in `text.dim`.
    pub fn prompt(icon: AppIcon, name: &str, details: &[&str]) -> Self {
        let mut dialog = Self::new(Request::Cancel);
        dialog.icon(icon);
        let mut question = Line::new();
        let _ = core::fmt::Write::write_fmt(&mut question, format_args!("Install {name}?"));
        dialog.line(question.as_str(), colour::TEXT);
        for detail in details {
            dialog.line(detail, colour::TEXT_DIM);
        }
        dialog.bottom_buttons(&[("Cancel", Request::Cancel), ("Install", Request::Install)]);
        dialog.finish()
    }

    /// What a program the machine cannot run, or a failed install or uninstall, shows:
    /// `message` and `Close`.
    pub fn message(icon: AppIcon, message: &str) -> Self {
        let mut dialog = Self::new(Request::Close);
        dialog.icon(icon);
        dialog.wrapped(message, colour::TEXT, MAX_LINES);
        dialog.bottom_buttons(&[("Close", Request::Close)]);
        dialog.finish()
    }

    /// The progress: the bar and the line under it, with `Cancel`.
    pub fn progress(icon: AppIcon, name: &str, status: &str) -> Self {
        let mut dialog = Self::new(Request::Cancel);
        dialog.icon(icon);
        let mut heading = Line::new();
        let _ = core::fmt::Write::write_fmt(&mut heading, format_args!("Installing {name}"));
        dialog.line(heading.as_str(), colour::TEXT);
        dialog.progress = dialog.tree.add(
            dialog.root,
            Place::at(
                TEXT_X,
                PROGRESS_TOP,
                Length::Fill(PADDING),
                Length::Px(eggos_toolkit::widget::PROGRESS_HEIGHT),
            ),
            Kind::Progress { permille: 0 },
        );
        let status = dialog.tree.add(
            dialog.root,
            Place::at(
                TEXT_X,
                STATUS_TOP,
                Length::Fill(PADDING),
                Length::Px(LINE_HEIGHT),
            ),
            Kind::label(status, colour::TEXT_DIM),
        );
        dialog.status = dialog.keep_label(status);
        dialog.bottom_buttons(&[("Cancel", Request::Cancel)]);
        dialog.finish()
    }

    /// Shows `done` of `total` bytes copied, and `status` under the bar.
    pub fn show_progress(&mut self, done: u64, total: u64, status: &str) {
        let permille = match total {
            0 => PROGRESS_DONE,
            _ => (done.min(total) * u64::from(PROGRESS_DONE) / total) as u16,
        };
        if let Some(bar) = self.progress {
            self.tree.set_progress(bar, permille);
        }
        if let Some(line) = self.status {
            self.tree.set_text(line, status);
        }
    }

    /// The copy is over: `Cancel` is drawn dimmed and ignored.
    pub fn copy_over(&mut self) {
        for (id, request) in self.buttons.iter().flatten() {
            if *request == Request::Cancel {
                self.tree.set_enabled(*id, false);
            }
        }
        self.escape = Request::Install;
    }

    /// The result: `Installed <name>.` with `Close` and `Open`.
    pub fn installed(icon: AppIcon, name: &str) -> Self {
        let mut dialog = Self::new(Request::Close);
        dialog.icon(icon);
        let mut line = Line::new();
        let _ = core::fmt::Write::write_fmt(&mut line, format_args!("Installed {name}."));
        dialog.line(line.as_str(), colour::TEXT);
        dialog.line(
            "It is in the launcher and on the desktop.",
            colour::TEXT_DIM,
        );
        dialog.bottom_buttons(&[("Close", Request::Close), ("Open", Request::Open)]);
        dialog.finish()
    }

    /// The uninstall confirmation: `Uninstall <name>?`, what it removes, `Cancel` and
    /// `Uninstall`.
    pub fn confirm_uninstall(icon: AppIcon, name: &str) -> Self {
        let mut dialog = Self::new(Request::Cancel);
        dialog.icon(icon);
        let mut question = Line::new();
        let _ = core::fmt::Write::write_fmt(&mut question, format_args!("Uninstall {name}?"));
        dialog.line(question.as_str(), colour::TEXT);
        let mut removes = Line::new();
        let _ = core::fmt::Write::write_fmt(
            &mut removes,
            format_args!("Removes: the launcher entry, the desktop icon and /Applications/{name}"),
        );
        dialog.wrapped(removes.as_str(), colour::TEXT_DIM, 3);
        dialog.bottom_buttons(&[
            ("Cancel", Request::Cancel),
            ("Uninstall", Request::Uninstall),
        ]);
        dialog.finish()
    }

    /// The window's widgets.
    pub fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        &mut self.tree
    }

    /// The window's widgets, to draw them.
    pub fn widgets(&self) -> &WidgetTree<CAPACITY> {
        &self.tree
    }

    /// What `response` asks: a button's request, or Esc's.
    pub fn respond(&self, response: Response) -> Option<Request> {
        match response {
            Response::Activated(id) => self
                .buttons
                .iter()
                .flatten()
                .find(|(button, _)| *button == id)
                .map(|(_, request)| *request),
            Response::Unused { code: key::ESC, .. } => match self.escape {
                Request::Install => None,
                escape => Some(escape),
            },
            _ => None,
        }
    }

    /// Where the button asking `request` is.
    pub fn button_rect(&self, request: Request) -> Option<Rect> {
        let (id, _) = self
            .buttons
            .iter()
            .flatten()
            .find(|(_, asked)| *asked == request)?;
        self.tree.rect(*id)
    }

    /// The texts of the window's lines, top to bottom.
    pub fn texts(&self) -> impl Iterator<Item = &str> + '_ {
        self.labels
            .iter()
            .flatten()
            .filter_map(|id| self.tree.text_of(*id))
    }

    /// The text of the button asking `request`.
    pub fn button_text(&self, request: Request) -> Option<&str> {
        let (id, _) = self
            .buttons
            .iter()
            .flatten()
            .find(|(_, asked)| *asked == request)?;
        self.tree.text_of(*id)
    }

    fn keep_label(&mut self, id: Option<WidgetId>) -> Option<WidgetId> {
        if let (Some(id), Some(slot)) = (id, self.labels.iter_mut().find(|slot| slot.is_none())) {
            *slot = Some(id);
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_toolkit::input::{Input, Modifiers};

    fn render(dialog: &Dialog) -> Vec<u32> {
        let (width, height) = SIZE;
        let mut pixels = vec![0; (width * height) as usize];
        dialog.widgets().render(&mut pixels, width, height, 1);
        pixels
    }

    fn press(dialog: &mut Dialog, code: u16) -> Option<Request> {
        let response = dialog.tree().handle(
            Input::Key {
                code,
                pressed: true,
                modifiers: Modifiers(0),
                character: 0,
            },
            0,
        );
        dialog.tree().handle(
            Input::Key {
                code,
                pressed: false,
                modifiers: Modifiers(0),
                character: 0,
            },
            0,
        );
        dialog.respond(response)
    }

    #[test]
    fn the_prompt_asks_with_its_details_and_enter_installs() {
        let icon = AppIcon::of("win-paint", Some(Source::Windows));
        assert_eq!(icon.letter, b'W');
        let mut dialog = Dialog::prompt(
            icon,
            "win-paint",
            &[
                "Windows program, x86_64",
                "From: /media/host/win-paint.exe",
                "To: /Applications/win-paint",
            ],
        );
        let texts: Vec<&str> = dialog.texts().collect();
        assert_eq!(
            texts,
            [
                "Install win-paint?",
                "Windows program, x86_64",
                "From: /media/host/win-paint.exe",
                "To: /Applications/win-paint"
            ]
        );
        let install = dialog.button_rect(Request::Install).unwrap();
        let cancel = dialog.button_rect(Request::Cancel).unwrap();
        assert_eq!(install.right(), SIZE.0 as i32 - 16);
        assert_eq!(install.bottom(), SIZE.1 as i32 - 16);
        assert!(cancel.right() + 8 <= install.x);
        assert_eq!(press(&mut dialog, key::ENTER), Some(Request::Install));
        assert_eq!(press(&mut dialog, key::ESC), Some(Request::Cancel));
        let pixels = render(&dialog);
        let at = |x: i32, y: i32| pixels[(y * SIZE.0 as i32 + x) as usize];
        assert_eq!(at(20, 40), colour::ICON_TILE);
        assert_eq!(at(16 + 31, 16 + 40), colour::ICON_WINDOWS);
        assert_eq!(at(300, 150), colour::WINDOW_BACKGROUND);
    }

    #[test]
    fn the_progress_follows_the_bytes_and_cancel_dims_when_the_copy_ends() {
        let mut dialog = Dialog::progress(
            AppIcon::of("tool", Some(Source::Linux)),
            "tool",
            "Copying tool...",
        );
        dialog.tree().take_damage();
        dialog.show_progress(50, 100, "Copying tool...");
        assert!(!dialog.tree().take_damage().is_empty());
        let pixels = render(&dialog);
        let row = (PROGRESS_TOP + 4) as usize * SIZE.0 as usize;
        assert_eq!(pixels[row + TEXT_X as usize + 10], colour::ACCENT);
        assert_eq!(pixels[row + SIZE.0 as usize - 30], colour::PROGRESS_TRACK);
        assert_eq!(press(&mut dialog, key::ESC), Some(Request::Cancel));
        dialog.copy_over();
        dialog.show_progress(100, 100, "Registering...");
        assert_eq!(dialog.texts().last(), Some("Registering..."));
        assert_eq!(press(&mut dialog, key::ESC), None);
        assert_eq!(press(&mut dialog, key::ENTER), None);
        dialog.show_progress(0, 0, "Registering...");
    }

    #[test]
    fn results_and_confirmations_offer_their_buttons() {
        let mut installed = Dialog::installed(AppIcon::of("tool", None), "tool");
        assert_eq!(installed.texts().next(), Some("Installed tool."));
        assert_eq!(press(&mut installed, key::ENTER), Some(Request::Open));
        assert_eq!(press(&mut installed, key::ESC), Some(Request::Close));
        let mut confirm = Dialog::confirm_uninstall(AppIcon::of("tool", None), "tool");
        let texts: Vec<&str> = confirm.texts().collect();
        assert_eq!(texts[0], "Uninstall tool?");
        assert!(texts[1].starts_with("Removes: the launcher entry"));
        assert!(texts.concat().contains("/Applications/tool"));
        assert_eq!(confirm.button_text(Request::Uninstall), Some("Uninstall"));
        assert_eq!(press(&mut confirm, key::ENTER), Some(Request::Uninstall));
        let message = Dialog::message(
            AppIcon::of("win-paint", Some(Source::Windows)),
            "EggOS cannot run this on riscv64: the Windows layer runs on x86_64 and aarch64 only",
        );
        let lines: Vec<&str> = message.texts().collect();
        assert!(lines.len() >= 2);
        assert_eq!(
            lines.join(" "),
            "EggOS cannot run this on riscv64: the Windows layer runs on x86_64 and aarch64 only"
        );
        assert!(message.button_rect(Request::Close).is_some());
        assert!(message.button_rect(Request::Install).is_none());
    }

    #[test]
    fn the_chooser_lists_programs_and_a_click_chooses_one() {
        let mut dialog =
            Dialog::choose("/media/host", &["win-paint.exe", "tool", "a", "b", "c"], 3);
        assert!(dialog.button_rect(Request::Choose(3)).is_some());
        assert!(dialog.button_rect(Request::Choose(4)).is_none());
        assert_eq!(
            dialog.button_text(Request::Choose(0)),
            Some("win-paint.exe")
        );
        assert_eq!(press(&mut dialog, key::ENTER), Some(Request::Choose(0)));
        assert!(dialog.texts().any(|line| line.starts_with("and 3 more")));
        let empty = Dialog::choose("/media/host", &[], 0);
        assert_eq!(
            empty.texts().nth(1),
            Some("It holds no program EggOS can run.")
        );
    }

    #[test]
    fn wrapping_breaks_at_spaces_within_the_width_and_the_label_s_bytes() {
        let mut lines = [""; 4];
        let count = wrap("one two three four five six", 60, &mut lines);
        assert!(count >= 2);
        for line in &lines[..count] {
            assert!(text::width(line) <= 60, "{line}");
            assert!(!line.starts_with(' ') && !line.ends_with(' '));
        }
        assert_eq!(lines[..count].join(" "), "one two three four five six");
        let long = "x".repeat(200);
        let count = wrap(&long, 10_000, &mut lines);
        assert!(lines[..count].iter().all(|line| line.len() <= 64));
        assert_eq!(wrap("", 100, &mut lines), 0);
        let mut one = [""; 1];
        assert_eq!(wrap("a b c", 1, &mut one), 1);
        assert_eq!(one[0], "a");
    }
}
