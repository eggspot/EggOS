//! The standard About panel (`docs/ux/macos-gui-v0.3.md` section 8, D175):
//! `orderFrontStandardAboutPanel:` and `orderFrontStandardAboutPanelWithOptions:` show the
//! layer's own fixed-size window, drawn by `eggos_macos_gui::about`, with the app's icon, name,
//! version and copyright from its `Info.plist` and the options. It is not one of the app's
//! windows: it carries the main menu, Esc and the close control destroy it, and choosing About
//! again makes it anew where it was. Agents read it as an element tree.

use super::{application, event, image, menu, responds, server, store, with_text};
use crate::foundation::{self, LONGEST_PATH};
use crate::libsystem::Exclusive;
use crate::linux::{self, O_CLOEXEC, O_RDONLY};
use crate::objc::abi::{Id, Sel};
use crate::objc::{sel, send, send1};
use eggos_element_tree::{
    Bounds, Element, MAX_TREE_BYTES, Outcome, Query, Role, TreeWriter, query_kind, tree_answer,
};
use eggos_macos_gui::about::{
    self, CHROME, CREDITS_BYTES, ICON, ICON_FILE_BYTES, ICON_PIXELS, MAX_CREDIT_LINES, Panel,
    VERSION_BYTES, WIDTH,
};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::input::{Input, MouseButton, key};
use eggos_toolkit::text::Text;

const NAME_BYTES: usize = 128;
const COPYRIGHT_BYTES: usize = 256;
const TITLE_PREFIX: &str = "About ";
const OWN_ICON: &[u8] = b"own";
const GENERIC_ICON: &[u8] = b"generic";
const IMAGE_ICON: &[u8] = b"image";
const CREDITS_FILE: &str = "/Credits.txt";
const CREDITS_NAME: &[u8] = b"Credits";
const LINE_VALUE_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IconSource {
    Generic,
    Own,
    Image,
}

struct AboutPanel {
    buffer: Option<server::Buffer>,
    name: Text<NAME_BYTES>,
    version: Text<VERSION_BYTES>,
    copyright: Text<COPYRIGHT_BYTES>,
    icon_source: IconSource,
    icon: [u32; ICON_PIXELS],
    credits: Text<CREDITS_BYTES>,
    lines: [(usize, usize); MAX_CREDIT_LINES],
    line_count: usize,
    offset: i32,
}

static PANEL: Exclusive<AboutPanel> = Exclusive::new(AboutPanel {
    buffer: None,
    name: Text::new(),
    version: Text::new(),
    copyright: Text::new(),
    icon_source: IconSource::Generic,
    icon: [0; ICON_PIXELS],
    credits: Text::new(),
    lines: [(0, 0); MAX_CREDIT_LINES],
    line_count: 0,
    offset: 0,
});

static APPLICATION_ICON: Exclusive<usize> = Exclusive::new(0);

impl AboutPanel {
    fn panel(&self) -> Panel<'_> {
        Panel {
            name: self.name.as_str(),
            version: self.version.as_str(),
            copyright: self.copyright.as_str(),
        }
    }

    fn has_credits(&self) -> bool {
        !self.credits.is_empty()
    }

    fn height(&self) -> i32 {
        let panel = self.panel();
        match self.has_credits() {
            true => about::height_with_credits(panel.body_lines()),
            false => panel.height(),
        }
    }

    fn lines(&self) -> &[(usize, usize)] {
        &self.lines[..self.line_count]
    }

    fn paint(&self) {
        let Some(buffer) = self.buffer else {
            return;
        };
        // SAFETY: the panel's buffer is mapped while it is in the panel, and only this thread,
        // under the panel's lock, draws on it.
        let pixels = unsafe { buffer.pixels() };
        if let Some(mut canvas) = Canvas::new(pixels, buffer.width, buffer.height, 1) {
            let icon = (self.icon_source != IconSource::Generic).then_some(&self.icon);
            let panel = self.panel();
            panel.paint(&mut canvas, icon);
            if self.has_credits() {
                about::paint_credits(
                    &mut canvas,
                    self.credits.as_str(),
                    self.lines(),
                    panel.body_lines(),
                    self.offset,
                );
            }
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

fn option_object(options: Id, key: &str) -> Id {
    if options.is_null() {
        return options;
    }
    send1(options, sel!("objectForKey:"), super::string(key) as usize)
}

fn image_icon(object: Id, icon: &mut [u32; ICON_PIXELS]) -> bool {
    if object.is_null() || !super::is(object, &super::NSIMAGE) {
        return false;
    }
    image::with_pixels(object, |pixels, width, height| {
        about::fit_icon(pixels, width, height, icon);
    })
    .is_some()
}

fn choose_icon(panel: &mut AboutPanel, options: Id) {
    let given = option_object(options, "ApplicationIcon");
    let set = APPLICATION_ICON.with(|icon| *icon) as Id;
    panel.icon_source = if image_icon(given, &mut panel.icon) || image_icon(set, &mut panel.icon) {
        IconSource::Image
    } else if read_own_icon(&mut panel.icon) {
        IconSource::Own
    } else {
        IconSource::Generic
    };
}

fn read_credits_file(out: &mut Text<CREDITS_BYTES>) {
    let bundle = send(
        crate::foundation::class_pointer(&crate::foundation::NSBUNDLE).cast(),
        sel!("mainBundle"),
    );
    if bundle.is_null() {
        return;
    }
    let mut path = [0u8; LONGEST_PATH + 1];
    let length = with_text(send(bundle, sel!("resourcePath")), |folder| {
        let total = folder.len() + CREDITS_FILE.len();
        if folder.is_empty() || total > LONGEST_PATH {
            return 0;
        }
        path[..folder.len()].copy_from_slice(folder.as_bytes());
        path[folder.len()..total].copy_from_slice(CREDITS_FILE.as_bytes());
        total
    });
    if length == 0 {
        return;
    }
    let Ok(fd) = linux::open(&path[..length], O_RDONLY | O_CLOEXEC, 0) else {
        return;
    };
    let mut file = [0u8; CREDITS_BYTES];
    let mut filled = 0;
    while filled < file.len() {
        match linux::read(fd, &mut file[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    let _ = linux::close(fd);
    let text = match core::str::from_utf8(&file[..filled]) {
        Ok(text) => text,
        Err(error) => core::str::from_utf8(&file[..error.valid_up_to()]).unwrap_or(""),
    };
    out.set(about::cut_credits(text));
}

fn gather_credits(panel: &mut AboutPanel, options: Id) {
    panel.credits.set("");
    panel.offset = 0;
    let given = option_object(options, "Credits");
    let text = if given.is_null() {
        given
    } else if foundation::is_string(given) {
        given
    } else if responds(given, sel!("string")) {
        send(given, sel!("string"))
    } else {
        core::ptr::null_mut()
    };
    match text.is_null() {
        true => read_credits_file(&mut panel.credits),
        false => with_text(text, |text| panel.credits.set(about::cut_credits(text))),
    }
    let AboutPanel {
        credits,
        lines,
        line_count,
        ..
    } = panel;
    *line_count = match credits.is_empty() {
        true => 0,
        false => about::wrap_credits(credits.as_str(), lines),
    };
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
    if copyright.is_empty() {
        copyright.set(about::first_line(info_text(b"CFBundleGetInfoString")));
    }
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
    choose_icon(panel, options);
    gather_credits(panel, options);
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
    let height = PANEL.with(|panel| panel.height());
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
        Input::Wheel { x, y, notches } => PANEL.with(|panel| {
            let region = about::credits_region(panel.panel().body_lines());
            if panel.has_credits() && region.contains(x, y) {
                let moved = about::credits_wheel(panel.offset, notches, panel.line_count);
                scroll_to(panel, moved);
            }
        }),
        Input::Button {
            x,
            y,
            button: MouseButton::Left,
            pressed: true,
        } => PANEL.with(|panel| {
            let region = about::credits_region(panel.panel().body_lines());
            let Some(track) = about::credits_track(region, panel.line_count) else {
                return;
            };
            if panel.has_credits() && track.contains(x, y) {
                let moved = about::credits_track_press(y - track.y, panel.line_count, panel.offset);
                scroll_to(panel, moved);
            }
        }),
        _ => {}
    }
}

fn scroll_to(panel: &mut AboutPanel, offset: i32) {
    if offset != panel.offset {
        panel.offset = offset;
        panel.paint();
    }
}

/// `setApplicationIconImage:`: the image the About panel shows from now on (D375), an open
/// panel's icon redrawn.
pub extern "C" fn set_application_icon_image(_: Id, _: Sel, icon: Id) {
    APPLICATION_ICON.with(|kept| {
        let mut slot = *kept as Id;
        store(&mut slot, icon);
        *kept = slot as usize;
    });
    PANEL.with(|panel| {
        if panel.buffer.is_some() {
            choose_icon(panel, core::ptr::null_mut());
            panel.paint();
        }
    });
}

/// `applicationIconImage`: what `setApplicationIconImage:` set, nil otherwise.
pub extern "C" fn application_icon_image(_: Id, _: Sel) -> Id {
    APPLICATION_ICON.with(|kept| *kept) as Id
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
    let root = Element::new(1, 0, Role::Window, Bounds::new(0, 0, WIDTH, panel.height()))
        .named(title.as_str().as_bytes());
    let icon = Element::new(
        2,
        1,
        Role::Graphic,
        Bounds::new(ICON.x, ICON.y, ICON.width, ICON.height),
    )
    .named(panel.name.as_str().as_bytes())
    .valued(match panel.icon_source {
        IconSource::Own => OWN_ICON,
        IconSource::Image => IMAGE_ICON,
        IconSource::Generic => GENERIC_ICON,
    });
    if !tree.push(&root) || !tree.push(&icon) {
        return;
    }
    let mut id = 3;
    let pushed = push_line(tree, &mut id, shown.name, Some(shown.name_line()))
        && push_line(tree, &mut id, shown.version, shown.version_line())
        && push_line(tree, &mut id, shown.copyright, shown.copyright_line());
    if pushed && panel.has_credits() {
        describe_credits(panel, tree, id);
    }
}

fn push_number(text: &mut Text<LINE_VALUE_BYTES>, number: usize) {
    let mut digits = [0u8; 20];
    let mut count = 0;
    let mut rest = number;
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        count += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    digits[..count].reverse();
    text.push_str(core::str::from_utf8(&digits[..count]).unwrap_or(""));
}

fn line_value(first: usize, lines: usize, out: &mut [u8; LINE_VALUE_BYTES]) -> &[u8] {
    let mut text = Text::<LINE_VALUE_BYTES>::new();
    text.set("line ");
    push_number(&mut text, first + 1);
    text.push_str(" of ");
    push_number(&mut text, lines);
    out[..text.len()].copy_from_slice(text.as_str().as_bytes());
    &out[..text.len()]
}

fn describe_credits(panel: &AboutPanel, tree: &mut TreeWriter<'_>, mut id: u32) {
    let body = panel.panel().body_lines();
    let region = about::credits_region(body);
    let inner = region.inset(1);
    let first = about::first_credit_line(panel.offset);
    let mut value = [0u8; LINE_VALUE_BYTES];
    let group_id = id;
    let group = Element::new(
        group_id,
        1,
        Role::Group,
        Bounds::new(region.x, region.y, region.width, region.height),
    )
    .named(CREDITS_NAME)
    .valued(line_value(first, panel.line_count, &mut value));
    if !tree.push(&group) {
        return;
    }
    id += 1;
    let text = panel.credits.as_str();
    for (index, &(start, end)) in panel.lines().iter().enumerate() {
        let words = &text[start..end];
        let width = eggos_toolkit::text::width(words);
        let cell = about::credits_cell(region, index, width, panel.offset);
        if words.is_empty() || cell.y < inner.y || cell.bottom() > inner.bottom() {
            continue;
        }
        let element = Element::new(
            id,
            group_id,
            Role::Label,
            Bounds::new(cell.x, cell.y, cell.width, cell.height),
        )
        .named(words.as_bytes());
        if !tree.push(&element) {
            return;
        }
        id += 1;
    }
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
