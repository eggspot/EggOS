//! The standard About panel (`docs/ux/macos-gui-v0.3.md` section 8, D175):
//! `orderFrontStandardAboutPanel:` and `orderFrontStandardAboutPanelWithOptions:` show the
//! layer's own fixed-size window, drawn by `eggos_macos_gui::about`, with the app's icon, name,
//! version and copyright from its `Info.plist` and the options. It is not one of the app's
//! windows: it carries the main menu, Esc and the close control destroy it, and choosing About
//! again makes it anew where it was. Agents read it as an element tree.

use super::{application, event, menu, server, with_text};
use crate::foundation::{self, LONGEST_PATH};
use crate::libsystem::Exclusive;
use crate::linux::{self, O_CLOEXEC, O_RDONLY};
use crate::objc::abi::Id;
use crate::objc::{sel, send1};
use eggos_element_tree::{
    Bounds, Element, MAX_TREE_BYTES, Outcome, Query, Role, TreeWriter, query_kind, tree_answer,
};
use eggos_macos_gui::about::{
    self, CHROME, ICON, ICON_FILE_BYTES, ICON_PIXELS, Panel, VERSION_BYTES, WIDTH,
};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::input::{Input, key};
use eggos_toolkit::text::Text;

const NAME_BYTES: usize = 128;
const COPYRIGHT_BYTES: usize = 256;
const TITLE_PREFIX: &str = "About ";
const OWN_ICON: &[u8] = b"own";
const GENERIC_ICON: &[u8] = b"generic";

struct AboutPanel {
    buffer: Option<server::Buffer>,
    name: Text<NAME_BYTES>,
    version: Text<VERSION_BYTES>,
    copyright: Text<COPYRIGHT_BYTES>,
    own_icon: bool,
    icon: [u32; ICON_PIXELS],
}

static PANEL: Exclusive<AboutPanel> = Exclusive::new(AboutPanel {
    buffer: None,
    name: Text::new(),
    version: Text::new(),
    copyright: Text::new(),
    own_icon: false,
    icon: [0; ICON_PIXELS],
});

impl AboutPanel {
    fn panel(&self) -> Panel<'_> {
        Panel {
            name: self.name.as_str(),
            version: self.version.as_str(),
            copyright: self.copyright.as_str(),
        }
    }

    fn paint(&self) {
        let Some(buffer) = self.buffer else {
            return;
        };
        // SAFETY: the panel's buffer is mapped while it is in the panel, and only this thread,
        // under the panel's lock, draws on it.
        let pixels = unsafe { buffer.pixels() };
        if let Some(mut canvas) = Canvas::new(pixels, buffer.width, buffer.height, 1) {
            let icon = self.own_icon.then_some(&self.icon);
            self.panel().paint(&mut canvas, icon);
        }
        server::commit(buffer.window);
    }
}

/// The panel's server window, 0 when it is not shown.
pub fn server_id() -> u32 {
    PANEL.with(|panel| panel.buffer.map_or(0, |buffer| buffer.window))
}

/// Whether `window` is the panel's server window.
pub fn is_panel(window: u32) -> bool {
    window != 0 && server_id() == window
}

fn option_text(options: Id, key: &str, work: impl FnOnce(Option<&str>)) {
    if options.is_null() {
        return work(None);
    }
    let value = send1(options, sel!("objectForKey:"), super::string(key) as usize);
    if value.is_null() || !foundation::is_string(value) {
        return work(None);
    }
    with_text(value, |text| work(Some(text)))
}

fn info_text(key: &[u8]) -> &'static str {
    foundation::info_string(key)
        .and_then(|value| core::str::from_utf8(value).ok())
        .unwrap_or("")
}

fn read_own_icon(icon: &mut [u32; ICON_PIXELS]) -> bool {
    let mut path = [0u8; LONGEST_PATH];
    let Some(path) = foundation::own_icon_path(&mut path) else {
        return false;
    };
    let Ok(fd) = linux::open(path, O_RDONLY | O_CLOEXEC, 0) else {
        return false;
    };
    let mut file = [0u8; ICON_FILE_BYTES + 1];
    let mut filled = 0;
    while filled < file.len() {
        match linux::read(fd, &mut file[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    let _ = linux::close(fd);
    about::icon_pixels(&file[..filled], icon)
}

fn gather(panel: &mut AboutPanel, options: Id) {
    panel.name.set(super::app_name());
    option_text(options, "ApplicationName", |name| {
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            panel.name.set(name);
        }
    });
    let mut short = Text::<VERSION_BYTES>::new();
    let mut build = Text::<VERSION_BYTES>::new();
    let mut copyright = Text::<COPYRIGHT_BYTES>::new();
    short.set(info_text(b"CFBundleShortVersionString"));
    build.set(info_text(b"CFBundleVersion"));
    copyright.set(about::first_line(info_text(b"NSHumanReadableCopyright")));
    option_text(options, "Version", |text| {
        if let Some(text) = text {
            short.set(text);
        }
    });
    option_text(options, "ApplicationVersion", |text| {
        if let Some(text) = text {
            build.set(text);
        }
    });
    option_text(options, "Copyright", |text| {
        if let Some(text) = text {
            copyright.set(about::first_line(text));
        }
    });
    let mut line = [0u8; VERSION_BYTES];
    let version = about::version_line(short.as_str(), build.as_str(), &mut line).unwrap_or("");
    panel.version.set(version);
    panel.copyright.set(copyright.as_str());
    panel.own_icon = read_own_icon(&mut panel.icon);
}

fn destroy(panel: &mut AboutPanel) {
    if let Some(buffer) = panel.buffer.take() {
        // SAFETY: the buffer left the panel, so nothing draws on it any more.
        unsafe { server::destroy(&buffer) };
    }
}

fn title_of(name: &str) -> Text<NAME_BYTES> {
    let mut title = Text::<NAME_BYTES>::new();
    title.set(TITLE_PREFIX);
    title.push_str(name);
    title
}

/// Shows the panel, with `options` (an NSDictionary, or nil) overriding what the bundle says:
/// a panel already shown is destroyed and made again at its place.
pub fn show(options: Id) {
    let previous = PANEL.with(|panel| panel.buffer.map(|buffer| buffer.window));
    let place = previous.and_then(server::placement);
    PANEL.with(|panel| {
        destroy(panel);
        gather(panel, options);
    });
    let height = PANEL.with(|panel| panel.panel().height());
    let Ok(buffer) = server::create(
        server::ROLE_NORMAL,
        0,
        WIDTH as u32,
        height as u32,
        server::FIXED_SIZE,
    ) else {
        application::report_no_window_server();
        return;
    };
    let title = PANEL.with(|panel| title_of(panel.name.as_str()));
    server::set_title(buffer.window, title.as_str());
    let work = server::work_area().unwrap_or((0, 0, 0, 0));
    let (x, y) = match place {
        Some((x, y, _, _)) => (x - work.0, y - work.1),
        None => (
            (work.2 - (WIDTH + CHROME.0)).max(0) / 2,
            (work.3 - (height + CHROME.1)).max(0) / 2,
        ),
    };
    server::set_position(buffer.window, x, y);
    menu::send_to_server(buffer.window);
    PANEL.with(|panel| {
        panel.buffer = Some(buffer);
        panel.paint();
    });
}

/// Destroys the panel, if it is shown.
pub fn close() {
    PANEL.with(destroy);
}

/// Handles one of the panel's input records: Esc and the close control close it, a key with
/// Cmd goes to the main menu's key equivalents, and nothing else does anything.
pub fn input(input: Input, time_ms: u32) {
    match input {
        Input::CloseRequest => close(),
        Input::Key {
            code: key::ESC,
            pressed: true,
            ..
        } => close(),
        Input::Key {
            code,
            pressed,
            modifiers,
            character,
        } => {
            let event = event::key(
                core::ptr::null_mut(),
                code,
                pressed,
                modifiers.0,
                character,
                time_ms,
            );
            let application = application::application();
            if !event.is_null() && !application.is_null() {
                send1(application, sel!("sendEvent:"), event as usize);
            }
        }
        _ => {}
    }
}

fn push_line(
    tree: &mut TreeWriter<'_>,
    id: &mut u32,
    text: &str,
    placed: Option<about::Placed>,
) -> bool {
    let Some(placed) = placed else {
        return true;
    };
    let rect = placed.rect;
    let element = Element::new(
        *id,
        1,
        Role::Label,
        Bounds::new(rect.x, rect.y, rect.width, rect.height),
    )
    .named(text.as_bytes());
    *id += 1;
    tree.push(&element)
}

fn describe(panel: &AboutPanel, tree: &mut TreeWriter<'_>) {
    let shown = panel.panel();
    let title = title_of(panel.name.as_str());
    let root = Element::new(1, 0, Role::Window, Bounds::new(0, 0, WIDTH, shown.height()))
        .named(title.as_str().as_bytes());
    let icon = Element::new(
        2,
        1,
        Role::Graphic,
        Bounds::new(ICON.x, ICON.y, ICON.width, ICON.height),
    )
    .named(panel.name.as_str().as_bytes())
    .valued(if panel.own_icon {
        OWN_ICON
    } else {
        GENERIC_ICON
    });
    if !tree.push(&root) || !tree.push(&icon) {
        return;
    }
    let mut id = 3;
    let _ = push_line(tree, &mut id, shown.name, Some(shown.name_line()))
        && push_line(tree, &mut id, shown.version, shown.version_line())
        && push_line(tree, &mut id, shown.copyright, shown.copyright_line());
}

/// Answers a query about the panel: its tree, or "the element cannot" for every action.
pub fn answer(query: &Query) {
    if query.kind != query_kind::TREE {
        server::answer_query(query.serial, Outcome::NotSupported.value(), &[]);
        return;
    }
    let block = crate::libsystem::heap::allocate(MAX_TREE_BYTES, false);
    if block.is_null() {
        server::answer_query(query.serial, tree_answer::PIXELS_ONLY, &[]);
        return;
    }
    // SAFETY: a new block of `MAX_TREE_BYTES` bytes, this function's until it is freed below.
    let bytes = unsafe { core::slice::from_raw_parts_mut(block, MAX_TREE_BYTES) };
    let length = PANEL.with(|panel| {
        let mut tree = TreeWriter::new(&mut *bytes);
        describe(panel, &mut tree);
        tree.bytes().len()
    });
    server::answer_query(query.serial, tree_answer::TREE, &bytes[..length]);
    crate::libsystem::heap::release(block);
}

/// A menu item chosen in the top bar while the panel had the focus: its action goes to the
/// application and its delegate (the panel has no responders).
pub fn chosen(shown: usize, entry: usize) {
    menu::chosen(core::ptr::null_mut(), shown, entry);
}
